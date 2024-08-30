mod database_sql;

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use chrono::prelude::*;
use database_sql::CourtMeta;
use lazy_static::lazy_static;
use regex::Regex;
use teloxide::types::ChatId;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::SendError;
use tokio::time::{interval_at, Instant, MissedTickBehavior};

use self::database_sql::{Database, Error as DbError};
use crate::messages::DateFilter;
use crate::scraper::CourtInfo;
use crate::{messages, scraper, MessageSender};

pub const TRESHOLD_TIME: NaiveTime = NaiveTime::from_hms(8, 0, 0);

fn is_out_of_date(last_update: DateTime<Utc>) -> bool {
    let now = Utc::now().with_timezone(&chrono_tz::Europe::Berlin);

    let date = if now.time() < TRESHOLD_TIME {
        now.date_naive()
            .checked_sub_days(chrono::Days::new(1))
            .expect("Date out of range")
    } else {
        now.date_naive()
    };

    let treshold = date
        .and_time(TRESHOLD_TIME)
        .and_local_timezone(chrono_tz::Europe::Berlin)
        .single()
        .expect("Weird DST issues")
        .to_utc();

    return last_update < treshold;
}

struct CourtWorker {
    name: String,
    message_rx: mpsc::UnboundedReceiver<Message>,
    auto_update: tokio::time::Interval,
    notify: MessageSender,
    database: Database,
}

impl CourtWorker {
    async fn update(&mut self, force_update: bool) -> Result<CourtMeta, DbError> {
        log::debug!("{}: Check for update", self.name);

        let maybe_meta = self.database.get_court_meta(&self.name).await?;
        if let Some(meta) = dbg!(&maybe_meta) {
            if !force_update && !is_out_of_date(meta.last_update) {
                log::debug!("{}: Already up to date", self.name);
                return Ok(maybe_meta.unwrap());
            }
        }

        log::info!("{}: Out of date, updating", self.name);

        let last_update = Utc::now(); // Better have last_update too old than too new
        let new_info: Result<_, ()> = scraper::get_court_info(&self.name)
            .await
            .map_err(|e| log::warn!("Failed to get info for court {}: {e}", &self.name));

        if let Ok(CourtInfo {
            schedule: new_schedule,
            full_name,
        }) = &new_info
        {
            let old_schedule = self.database.get_sessions(&self.name, None, None).await?;

            for sub in self
                .database
                .get_confirmed_subscriptions_by_court(&self.name)
                .await?
            {
                let Some(msg) = messages::handle_update(
                    &old_schedule,
                    new_schedule,
                    full_name,
                    &sub.name,
                    &sub.reference_filter,
                ) else {
                    continue;
                };
                self.notify.send(ChatId(sub.chat_id), msg, None);
            }
        }

        let schedule = new_info.as_ref().ok().map(|x| &x.schedule[..]);
        let full_name = new_info.as_ref().ok().map(|x| &x.full_name[..]);
        dbg!(full_name);
        self.database
            .update_court_info(&self.name, &last_update, full_name, schedule)
            .await?;

        Ok(CourtMeta {
            last_update,
            full_name: new_info.ok().map(|x| x.full_name),
        })
    }

    async fn handle_update(&mut self, force_update: bool) {
        if let Err(e) = self.update(force_update).await {
            log::error!("Update failed: {e}")
        }
    }

    async fn update_and_get(
        &mut self,
        date_filter: Option<NaiveDate>,
    ) -> Result<Option<CourtInfo>, DbError> {
        let meta = self.update(false).await?;

        let info = match meta.full_name {
            None => None,
            Some(full_name) => {
                let schedule = self
                    .database
                    .get_sessions(&self.name, None, date_filter)
                    .await?;
                Some(CourtInfo {
                    full_name,
                    schedule,
                })
            }
        };

        Ok(info)
    }

    async fn database_error(&self, e: DbError, msg: &teloxide::types::Message) {
        log::error!("Database error: {e}");
        let _ = self.notify
            .send_direct(msg.chat.id, messages::internal_error(), Some(msg.id)).await;
    }

    async fn handle_get_sessions(
        &mut self,
        message: teloxide::types::Message,
        date: String,
        reference: String,
    ) {
        let Some(date) = DateFilter::new(&date) else {
            let _ = self
                .notify
                .send_direct(message.chat.id, messages::invalid_date(), Some(message.id))
                .await;
            return;
        };

        let info = match self.update_and_get(date.date).await {
            Ok(info) => info,
            Err(e) => {
                self.database_error(e, &message).await;
                return;
            }
        };

        let msg = messages::list_sessions(&info, &reference);

        self.notify
            .send_direct(message.chat.id, msg, Some(message.id))
            .await;
    }

    async fn handle_confirm_subscription(
        &mut self,
        message: teloxide::types::Message,
        subscription_id: i64,
    ) {
        let sub = match self.database.get_subscription_by_id(subscription_id).await {
            Ok(Some(sub)) => sub,
            Ok(None) => {
                log::info!("Subscription {subscription_id} does not exist, already deleted?");
                return;
            }
            Err(e) => {
                self.database_error(e, &message).await;
                return;
            }
        };

        let info = match self.update_and_get(None).await {
            Ok(info) => info,
            Err(e) => {
                self.database_error(e, &message).await;
                return;
            }
        };

        let msg = messages::subscribed(&sub.name, &info, &sub.reference_filter);

        self.notify
            .send_direct(message.chat.id, msg, Some(message.id))
            .await;

        if let Err(e) = self
            .database
            .set_subscription_confirmation_sent(subscription_id)
            .await
        {
            self.database_error(e, &message).await;
            return;
        }
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                _ = self.auto_update.tick() => self.handle_update(false).await,
                msg = self.message_rx.recv() => {
                    let Some(msg) = msg else {
                        // channel closed, no more messages
                        break
                    };
                    match msg {
                        Message::Update { force } => self.handle_update(force).await,
                        Message::GetSessions {
                            message,
                            date,
                            reference,
                        } => {
                            self.handle_get_sessions(message, date, reference)
                                .await
                        }
                        Message::ConfirmSubscription {
                            message,
                            subscription_id
                        } => {
                            self.handle_confirm_subscription(message, subscription_id).await
                        }
                        Message::Close => {
                            self.message_rx.close();
                        }
                    }
                }
            }
        }
    }
}

enum Message {
    Update {
        force: bool,
    },
    GetSessions {
        message: teloxide::types::Message,
        date: String,
        reference: String,
    },
    ConfirmSubscription {
        message: teloxide::types::Message,
        subscription_id: i64,
    },
    Close,
}

struct Court {
    message_tx: mpsc::UnboundedSender<Message>,
}

impl Drop for Court {
    fn drop(&mut self) {
        let _ = self.message_tx.send(Message::Close);
    }
}

pub struct Courts {
    courts: HashMap<String, Court>,
    notification_queue: MessageSender,
    database_url: String
}

pub struct CourtRef<'a> {
    courts: &'a mut Courts,
    name: &'a str
}

#[derive(Debug, Error)]
#[error("invalid court name")]
pub struct InvalidCourtName(());

lazy_static! {
    static ref COURT_NAME_REGEX: Regex = Regex::new("^[a-zA-Z0-9\\-]{1,63}$").unwrap();
}

impl Courts {
    pub fn new(notification_queue: MessageSender, database_url: String) -> Self {
        Self {
            notification_queue,
            courts: Default::default(),
            database_url
        }
    }

    pub fn get<'a>(&'a mut self, court_name: &'a str) -> Result<CourtRef<'a>, InvalidCourtName> {
        if !COURT_NAME_REGEX.is_match(court_name) {
            return Err(InvalidCourtName(()));
        }
        Ok(CourtRef {
            courts: self,
            name: court_name
        })
    }
}

impl<'a> CourtRef<'a> {
    fn create(&self) -> Court {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let period = {
            // to avoid peaks all 5 minutes, make the period "random"
            let mut hash = DefaultHasher::new();
            self.name.hash(&mut hash);
            Duration::from_secs(270 + hash.finish() % 60)
        };

        let mut auto_update = interval_at(Instant::now() + period, period);
        auto_update.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let name = self.name.to_string();
        let notify = self.courts.notification_queue.clone();
        let database_url = self.courts.database_url.clone();

        tokio::spawn(async move {
            let database =
                database_sql::Database::new(&database_url).await?;
            let worker = CourtWorker {
                name,
                message_rx,
                notify,
                auto_update,
                database,
            };
            Ok::<_, DbError>(worker.run().await)
        });

        Court { message_tx }
    }


    fn send_msg(&mut self, mut msg: Message) {
        if let Some(court) = self.courts.courts.get(self.name) {
            match court.message_tx.send(msg) {
                Ok(_) => return,
                Err(SendError(msg_cp)) => {
                    log::warn!(
                        "cannot send message to court worker task {}, recreating ...",
                        self.name
                    );
                    msg = msg_cp
                }
            }
        }
        

        let court = self.create();
        match court.message_tx.send(msg) {
            Ok(_) => (),
            Err(_) => log::error!("cannot send message to court worker task {}!", self.name),
        }

        self.courts.courts.insert(self.name.to_string(), court);
    }

    pub fn get_sessions(&mut self, message: teloxide::types::Message, date: String, reference: String ) {
        self.send_msg(Message::GetSessions { message, date, reference })
    }
    
    pub fn confirm_subscription(&mut self, message: teloxide::types::Message, subscription_id: i64 ) {
        self.send_msg(Message::ConfirmSubscription { message, subscription_id })
    }
    
    pub fn update(&mut self, force: bool ) {
        self.send_msg(Message::Update { force })
    }
}

