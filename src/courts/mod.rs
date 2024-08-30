mod worker;

use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use futures_core::future::BoxFuture;
use lazy_static::lazy_static;
use regex::Regex;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{interval_at, Instant, MissedTickBehavior};

use crate::database::Database;
use crate::reply_queue::ReplyQueue;

enum Message {
    Update {
        force: bool,
    },
    GetSessions {
        date: String,
        reference: String,
        reply_fn: Box<dyn ReplyFn>,
    },
    ConfirmSubscription {
        subscription_id: i64,
        reply_fn: Box<dyn ReplyFn>,
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

pub trait ReplyFn: Send + 'static {
    fn reply(self: Box<Self>, msg: String) -> BoxFuture<'static, ()>;
}

impl<T, F: Future<Output = ()> + Send + 'static> ReplyFn for T
where
    T: (FnOnce(String) -> F) + Send + 'static,
{
    fn reply(self: Box<Self>, msg: String) -> BoxFuture<'static, ()> {
        Box::pin(self(msg)) as BoxFuture<'static, ()>
    }
}

pub struct Courts {
    map: HashMap<String, Court>,
    reply_queue: ReplyQueue,
    queue_join_handle: JoinHandle<()>,
    database: Database,
}

impl Courts {
    pub async fn new(bot: crate::Bot, database: Database) -> Self {
        let (reply_queue, queue_join_handle) = ReplyQueue::new(bot);

        let mut this = Self {
            reply_queue,
            queue_join_handle,
            map: Default::default(),
            database,
        };

        this.init_subscribed_courts().await;

        this
    }

    pub async fn init_subscribed_courts(&mut self) {
        match self.database.get_subscribed_courts().await {
            Ok(names) => {
                for name in names {
                    match self.get(&name) {
                        Ok(mut c) => c.init(),
                        Err(_) => log::warn!("Invalid court name in db: {name}"),
                    }
                }
            }
            Err(e) => {
                log::error!("Database error, cannot init court workers: {e}")
            }
        };
    }

    pub fn get<'a>(&'a mut self, court_name: &'a str) -> Result<CourtRef<'a>, InvalidCourtName> {
        if !COURT_NAME_REGEX.is_match(court_name) {
            return Err(InvalidCourtName(()));
        }
        Ok(CourtRef {
            courts: self,
            name: court_name,
        })
    }

    pub async fn shutdown(self) {
        drop(self.map);
        drop(self.reply_queue);

        // The reasoning is simple: once the courts are dropped,
        // the channels will be closed, the court workers will run to completion,
        // and finally all ReplyQueue instances will be dropped.
        // Eventually, the reply queue task will run to completion.
        let _ = self
            .queue_join_handle
            .await
            .inspect_err(|e| log::error!("Reply queue task failed! {e}"));
    }
}

#[derive(Debug, Error)]
#[error("invalid court name")]
pub struct InvalidCourtName(());

lazy_static! {
    static ref COURT_NAME_REGEX: Regex = Regex::new("^[a-zA-Z0-9\\-]{1,63}$").unwrap();
}

pub struct CourtRef<'a> {
    courts: &'a mut Courts,
    name: &'a str,
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
        let notify = self.courts.reply_queue.clone();
        let database = self.courts.database.clone();
        let worker = worker::CourtWorker {
            name,
            message_rx,
            reply_queue: notify,
            auto_update,
            database,
        };

        tokio::spawn(worker.run());

        Court { message_tx }
    }

    fn init(&mut self) {
        if !self.courts.map.contains_key(self.name) {
            self.courts.map.insert(self.name.to_owned(), self.create());
        }
    }

    fn send_msg(&mut self, mut msg: Message) {
        if let Some(court) = self.courts.map.get(self.name) {
            match court.message_tx.send(msg) {
                Ok(_) => return,
                Err(mpsc::error::SendError(msg_cp)) => {
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

        self.courts.map.insert(self.name.to_string(), court);
    }

    pub fn get_sessions(&mut self, date: String, reference: String, reply_fn: impl ReplyFn) {
        self.send_msg(Message::GetSessions {
            date,
            reference,
            reply_fn: Box::new(reply_fn),
        })
    }

    pub fn confirm_subscription(&mut self, subscription_id: i64, reply_fn: impl ReplyFn) {
        self.send_msg(Message::ConfirmSubscription {
            subscription_id,
            reply_fn: Box::new(reply_fn),
        })
    }

    pub fn update(&mut self, force: bool) {
        self.send_msg(Message::Update { force })
    }
}
