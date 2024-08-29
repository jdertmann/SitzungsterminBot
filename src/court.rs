mod database;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use chrono::prelude::*;
use database::Database;
use redis::RedisResult;
use serde::{Deserialize, Serialize};
use teloxide::types::ChatId;
use tokio::sync::mpsc;
use tokio::time::{interval_at, Instant, MissedTickBehavior};

use crate::chat_list::ChatData;
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

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
struct Subscription {
    name: String,
    chat_id: ChatId,
    reference: String,
}

struct CourtWorker {
    name: String,
    message_rx: mpsc::UnboundedReceiver<Message>,
    auto_update: tokio::time::Interval,
    notify: MessageSender,
    database: Database,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]

struct CourtState {
    last_update: DateTime<Utc>,
    info: Result<scraper::CourtInfo, ()>,
}

impl CourtWorker {
    async fn update_and_get(&mut self, force_update: bool) -> RedisResult<CourtState> {
        log::debug!("{}: Check for update", self.name);
        let maybe_state = self.database.load_court_state().await?;

        if let Some(state) = &maybe_state {
            if !force_update && !is_out_of_date(state.last_update) {
                log::debug!("{}: Already up to date", self.name);
                return Ok(maybe_state.unwrap());
            }
        }
        log::info!("{}: Running update", self.name);
        let last_update = Utc::now(); // Better have last_update too old than too new

        let new_info: Result<_, ()> = scraper::get_court_info(&self.name)
            .await
            .map_err(|e| log::warn!("Failed to get info for court {}: {e}", &self.name));

        let new_state = CourtState {
            info: new_info,
            last_update,
        };

        if Some(&new_state.info) != maybe_state.as_ref().map(|x| &x.info) {
            let old_info = match maybe_state {
                Some(state) => state.info,
                None => Err(()),
            };

            for sub in self.database.load_subscriptions().await? {
                let Some(msg) =
                    messages::handle_update(&old_info, &new_state.info, &sub.name, &sub.reference)
                else {
                    continue;
                };
                self.notify.send(sub.chat_id, msg, None);
            }
        }

        self.database.save_court_state(&new_state).await?;

        Ok(new_state)
    }

    async fn handle_update(&mut self, force: bool) {
        let _ = self.update_and_get(force).await;
    }

    async fn handle_get_sessions(
        &mut self,
        message: teloxide::types::Message,
        chat: ChatData,
        date: String,
        reference: String,
    ) {
        let msg = match self.update_and_get(false).await {
            Ok(state) => messages::list_sessions(&state.info, &reference, &date),
            Err(e) => {
                log::warn!("Failed to retrieve state: {e}");
                messages::internal_error()
            }
        };

        self.notify
            .send_direct(chat.get_id(), msg, Some(message.id))
            .await;
    }

    async fn handle_add_subscription(
        &mut self,
        message: teloxide::types::Message,
        chat: ChatData,
        name: String,
        reference: String,
    ) {
        let Ok(state) = self.update_and_get(false).await else {
            self.notify
                .send_direct(chat.get_id(), messages::internal_error(), Some(message.id))
                .await;
            return;
        };

        let sub = Subscription {
            chat_id: chat.get_id(),
            name,
            reference,
        };

        let msg = match self.database.save_subscription(&sub).await {
            Ok(()) => messages::subscribed(&sub.name, &state.info, &sub.reference),
            Err(e) => {
                log::warn!("Failed to save subscription: {e}");
                messages::internal_error()
            }
        };

        self.notify
            .send_direct(chat.get_id(), msg, Some(message.id))
            .await;
    }

    async fn handle_remove_subscription(
        &mut self,
        message: teloxide::types::Message,
        chat: ChatData,
        name: String,
    ) {
        let msg = match self
            .database
            .remove_subscription(&name, chat.get_id())
            .await
        {
            Ok(n) => messages::unsubscribed(n),
            Err(e) => {
                log::warn!("Failed to remove subscription: {e}");
                messages::internal_error()
            }
        };

        self.notify
            .send_direct(chat.get_id(), msg, Some(message.id))
            .await;
    }

    async fn run(mut self) {
        loop {
            tokio::select! {
                _ = self.auto_update.tick() => self.handle_update(false).await,
                msg = self.message_rx.recv() => {
                    let Some(msg) = msg else {break};
                    match msg {
                        Message::Update { force } => self.handle_update(force).await,
                        Message::GetSessions {
                            message,
                            chat,
                            date,
                            reference,
                        } => {
                            self.handle_get_sessions(message, chat, date, reference)
                                .await
                        }
                        Message::AddSubscription {
                            message,
                            chat,
                            name,
                            reference,
                        } => {
                            self.handle_add_subscription(message, chat, name, reference)
                                .await
                        }
                        Message::RemoveSubscription {
                            message,
                            chat,
                            name,
                        } => self.handle_remove_subscription(message, chat, name).await,
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
        chat: ChatData,
        date: String,
        reference: String,
    },
    AddSubscription {
        message: teloxide::types::Message,
        chat: ChatData,
        name: String,
        reference: String,
    },
    RemoveSubscription {
        message: teloxide::types::Message,
        chat: ChatData,
        name: String,
    },
}

#[derive(Clone)]
pub struct Court {
    message_tx: mpsc::UnboundedSender<Message>,
}

impl Court {
    pub fn new(name: String, notify: MessageSender, redis: redis::Client) -> Self {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let database = Database::new(redis, &name);
        let period = {
            // to avoid peaks all 5 minutes, make the period "random"
            let mut hash = DefaultHasher::new();
            name.hash(&mut hash);
            Duration::from_secs(270 + hash.finish() % 60)
        };
        let mut auto_update = interval_at(Instant::now() + period, period);
        auto_update.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let worker = CourtWorker {
            name,
            message_rx,
            notify,
            auto_update,
            database,
        };

        tokio::spawn(worker.run());

        Court { message_tx }
    }

    pub fn update(&self, force: bool) {
        let _ = self.message_tx.send(Message::Update { force });
    }

    pub fn get_sessions(
        &self,
        message: teloxide::types::Message,
        chat: ChatData,
        date: String,
        reference: String,
    ) {
        let msg = Message::GetSessions {
            message,
            chat,
            date,
            reference,
        };
        let _ = self.message_tx.send(msg);
    }

    pub fn add_subscription(
        &self,
        message: teloxide::types::Message,
        chat: ChatData,
        name: String,
        reference: String,
    ) {
        let msg = Message::AddSubscription {
            message,
            chat,
            name,
            reference,
        };
        let _ = self.message_tx.send(msg);
    }

    pub fn remove_subscription(
        &self,
        message: teloxide::types::Message,
        chat: ChatData,
        name: String,
    ) {
        let msg = Message::RemoveSubscription {
            message,
            chat,
            name,
        };
        let _ = self.message_tx.send(msg);
    }
}
