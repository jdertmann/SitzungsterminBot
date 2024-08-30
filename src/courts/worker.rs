use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use teloxide::types::ChatId;
use tokio::sync::mpsc;

use super::Message;
use crate::database::{CourtMeta, Database, Error as DbError};
use crate::messages::MarkdownString;
use crate::reply_queue::ReplyQueue;
use crate::scraper::CourtData;
use crate::{messages, scraper};

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

    last_update < treshold
}

pub struct CourtWorker {
    pub name: String,
    pub message_rx: mpsc::UnboundedReceiver<Message>,
    pub auto_update: tokio::time::Interval,
    pub reply_queue: ReplyQueue,
    pub database: Database,
}

macro_rules! handle_db_error {
    ($e:expr) => {
        match $e {
            Ok(t) => t,
            Err(e) => {
                log::error!("Database error: {e}");
                return messages::internal_error().into();
            }
        }
    };
}

impl CourtWorker {
    async fn process_new_data(&mut self, new_data: &CourtData) -> Result<(), DbError> {
        let old_sessions = self.database.get_sessions(&self.name, None, None).await?;
        let subscriptions = self
            .database
            .get_confirmed_subscriptions_by_court(&self.name)
            .await?;

        for sub in subscriptions {
            let Some(msg) = messages::sessions_updated(
                &old_sessions,
                &new_data.sessions,
                &new_data.full_name,
                &sub.name,
                &sub.reference_filter,
            ) else {
                continue;
            };

            self.reply_queue.queue(ChatId(sub.chat_id), msg);
        }

        Ok(())
    }

    async fn update(&mut self, force_update: bool) -> Result<CourtMeta, DbError> {
        log::debug!("{}: Checking for update", self.name);

        if let Some(meta) = self.database.get_court_meta(&self.name).await? {
            if !force_update && !is_out_of_date(meta.last_update) {
                log::debug!("{}: Already up to date", self.name);
                return Ok(meta);
            }
        }

        log::info!("{}: Out of date, updating", self.name);

        let last_update = Utc::now(); // Better have last_update too old than too new
        let new_data = scraper::get_court_data(&self.name)
            .await
            .map_err(|e| log::warn!("Failed to get info for court {}: {e}", &self.name))
            .ok();

        if let Some(new_data) = &new_data {
            self.process_new_data(new_data).await?;
        }

        let sessions = new_data.as_ref().map(|x| &x.sessions[..]);
        let meta = CourtMeta {
            last_update,
            full_name: new_data.as_ref().map(|x| x.full_name.clone()),
        };

        self.database
            .update_court_data(&self.name, &meta, sessions)
            .await?;

        log::info!("Court {} has been updated", self.name);

        Ok(meta)
    }

    async fn get_court_data(
        &mut self,
        date_filter: Option<NaiveDate>,
    ) -> Result<Option<CourtData>, DbError> {
        let meta = self.update(false).await?;

        let Some(full_name) = meta.full_name else {
            // if full_name is None, the website was not available
            return Ok(None);
        };

        let sessions = self
            .database
            .get_sessions(&self.name, None, date_filter)
            .await?;

        let court_data = CourtData {
            full_name,
            sessions,
        };

        Ok(Some(court_data))
    }

    async fn handle_update(&mut self, force_update: bool) {
        if let Err(e) = self.update(force_update).await {
            log::error!("Update failed: {e}")
        }
    }

    async fn handle_get_sessions(&mut self, date: String, reference: String) -> MarkdownString {
        let date = if &date == "*" {
            None
        } else if let Ok(date) = NaiveDate::parse_from_str(&date, "%d.%m.%Y") {
            Some(date)
        } else {
            // Invalid date in input
            return messages::invalid_date();
        };

        let data = handle_db_error!(self.get_court_data(date).await);

        messages::list_sessions(&data, &reference)
    }

    async fn handle_confirm_subscription(
        &mut self,
        subscription_id: i64,
    ) -> Option<MarkdownString> {
        let sub = handle_db_error!(self.database.get_subscription_by_id(subscription_id).await);

        let Some(sub) = sub else {
            log::info!("Subscription {subscription_id} does not exist, already deleted?");
            return None;
        };

        let data = handle_db_error!(self.get_court_data(None).await);
        let reply = messages::subscribed(&sub.name, &data, &sub.reference_filter);

        handle_db_error!(
            self.database
                .set_subscription_confirmation_sent(subscription_id)
                .await
        );

        Some(reply)
    }

    pub async fn run(mut self) {
        log::info!("Starting worker task for {}", self.name);
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
                            date,
                            reference,
                            reply_fn
                        } => {
                            let reply = self.handle_get_sessions(date, reference).await;
                            reply_fn.reply(reply).await;
                        }
                        Message::ConfirmSubscription {
                            subscription_id,
                            reply_fn
                        } => {
                            let reply = self.handle_confirm_subscription(subscription_id).await;
                            if let Some(reply) = reply {
                                reply_fn.reply(reply).await;
                            }
                        }
                        Message::Close => {
                            self.message_rx.close();
                        }
                    }
                }
            }
        }
        log::info!("Worker task for {} shut down.", self.name);
    }
}
