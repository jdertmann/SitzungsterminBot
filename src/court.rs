use chrono::prelude::*;
use redis::{AsyncCommands, RedisResult};
use serde::{Deserialize, Serialize};
use teloxide::types::ChatId;
use tokio::sync::mpsc;

use crate::{chat_list::ChatData, messages, scraper, MessageSender};

fn is_out_of_date(last_update: Option<DateTime<Utc>>) -> bool {
    const TRESHOLD_TIME: NaiveTime = NaiveTime::from_hms(7, 5, 0);

    let Some(last_update) = last_update else {
        return false;
    };

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
    reference: String
}


#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct CourtState {
    court_info: Result<scraper::CourtInfo, ()>,
    last_update: Option<DateTime<Utc>>,
    subscriptions: Vec<Subscription>,
}

type RedisConnection = redis::aio::MultiplexedConnection;
struct StateManager {
    redis: redis::Client,
    redis_key: String,
    redis_conn: Option<RedisConnection>,
    non_persistent_mode: bool,
}

impl StateManager {
    fn new(client: redis::Client, court_name: &str) -> Self {
        Self {
            redis: client,
            redis_key: format!("court_state:{court_name}"),
            redis_conn: None,
            non_persistent_mode: false
        }
    }

    async fn get_connection(&mut self) -> RedisResult<RedisConnection> {
        if self.redis_conn.is_some() {
            Ok(self.redis_conn.as_ref().unwrap().clone())
        } else {
            let conn = self.redis.get_multiplexed_async_connection().await?;
            Ok(self.redis_conn.insert(conn).clone())
        }
    }

    async fn load_state_inner(&mut self) -> RedisResult<Option<CourtState>> {
        let mut conn = self.get_connection().await?;
        let state_str: Option<String> = conn.get(&self.redis_key).await?;
        let state = match state_str {
            Some(state_str) => Some(serde_json::from_str(&state_str)?),
            None => None
        };
        Ok(state)
    }
    async fn load_state(&mut self) -> RedisResult<Option<CourtState>> {
        self.load_state_inner()
            .await
            .inspect_err(|e| log::warn!("Redis error while loading state: {e}"))
    }

    async fn save_state_inner(&mut self, state: &CourtState) -> RedisResult<()> {
        let mut conn = self.get_connection().await?;
        let state_str = serde_json::to_string(state).expect("Serialization failed");
        conn.set(&self.redis_key, state_str).await?;
        Ok(())
    }

    async fn save_state(&mut self, state: &CourtState) -> RedisResult<()> {
        if self.non_persistent_mode {
            log::info!("Didn't save court state due to non-persistance mode");
            return Ok(());
        }
        self.save_state_inner(state)
            .await
            .inspect_err(|e| log::warn!("Redis error while saving state: {e}"))
    }
}

struct CourtWorker {
    name: String,
    state: Option<CourtState>,
    message_rx: mpsc::UnboundedReceiver<Message>,
    notify: MessageSender,
    state_manager: StateManager,
}

impl CourtWorker {
    async fn update(&mut self, force: bool) -> &mut CourtState {
        if let Some(state) = &self.state {
            if !force && !is_out_of_date(state.last_update) {
                // to make the borrow checker happy ...
                return self.state.as_mut().unwrap();
            }
        }

        let last_update = Some(Utc::now()); // Better have last_update too old than too new
        let new_info: Result<_, ()> = scraper::get_court_info(&self.name)
            .await
            .map_err(|e| log::warn!("Failed to get info for court {}: {e}", &self.name));

        match &mut self.state {
            Some(state) => {
                for sub in &state.subscriptions {
                    let Some(msg) =
                        messages::handle_update(&state.court_info, &new_info, &sub.name, &sub.reference)
                    else {
                        continue;
                    };
                    self.notify.send(sub.chat_id, msg);
                }

                state.last_update = last_update;
                state.court_info = new_info;

                let _ = self.state_manager.save_state(&state).await;

                state
            }
            state => {
                let new_state = CourtState {
                    court_info: new_info,
                    last_update,
                    subscriptions: Vec::new(),
                };

                let _ = self.state_manager.save_state(&new_state).await;

                state.insert(new_state)
            }
        }
    }

    async fn handle_update(&mut self, force: bool) {
        self.update(force).await;
    }

    async fn handle_get_sessions(&mut self, chat: ChatData, date: String, reference: String) {
        let state = self.update(false).await;

        let msg = messages::list_sessions(&state.court_info, &reference, &date);
        self.notify.send_direct(chat.get_id(), msg).await;
    }

    async fn handle_add_subscription(&mut self, chat: ChatData, name: String, reference: String) {
        if self.state_manager.non_persistent_mode {
            self.notify.send_direct(chat.get_id(), messages::internal_error()).await;
            return;
        }

        self.update(false).await;
        let state = self.state.as_mut().unwrap();

        let sub = Subscription {
            chat_id: chat.get_id(),
            name,
            reference
        };
    
        state.subscriptions.push(sub.clone());

        if let Err(_) = self.state_manager.save_state(state).await {
            state.subscriptions.pop();
            self.notify.send_direct(chat.get_id(), messages::internal_error()).await;
            return;
        }

        let msg = messages::subscribed(&sub.name, &state.court_info, &sub.reference);

        self.notify.send_direct(chat.get_id(), msg).await;
    }

    async fn handle_remove_subscription(
        &mut self,
        chat: ChatData,
        name: Option<String>,
        reply_to_bot: bool,
    ) {
        if self.state_manager.non_persistent_mode {
            self.notify.send_direct(chat.get_id(), messages::internal_error()).await;
            return;
        }

        let Some(state) = &mut self.state else {
            return;
        };

        let old_len = state.subscriptions.len();

        state.subscriptions.retain(|sub| {
            sub.chat_id != chat.get_id() || (name.is_some() && Some(&sub.name) != name.as_ref())
        });

        if let Err(_) = self.state_manager.save_state(state).await {
            self.notify.send_direct(chat.get_id(), messages::internal_error()).await;
            return;
        }

        if state.subscriptions.len() < old_len && reply_to_bot {
            let msg = match name {
                None => "Removed all subscriptions".to_string(),
                Some(name) => format!("Removed subscription {name}"),
            };
            self.notify.send_direct(chat.get_id(), msg).await;
        }
    }

    async fn run(mut self) {
        match self.state_manager.load_state().await {
            Ok(state) => self.state = state,
            Err(_) => {self.state_manager.non_persistent_mode = true}
        }

        while let Some(msg) = self.message_rx.recv().await {
            match msg {
                Message::Update { force } => self.handle_update(force).await,
                Message::GetSessions {
                    chat,
                    date,
                    reference,
                } => self.handle_get_sessions(chat, date, reference).await,
                Message::AddSubscription {
                    chat,
                    name,
                    reference,
                } => self.handle_add_subscription(chat, name, reference).await,
                Message::RemoveSubscription {
                    chat,
                    name,
                    reply_to_bot,
                } => {
                    self.handle_remove_subscription(chat, name, reply_to_bot)
                        .await
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
        chat: ChatData,
        date: String,
        reference: String,
    },
    AddSubscription {
        chat: ChatData,
        name: String,
        reference: String,
    },
    RemoveSubscription {
        chat: ChatData,
        name: Option<String>, // remove all if none
        reply_to_bot: bool,
    },
}

#[derive(Clone)]
pub struct Court {
    message_tx: mpsc::UnboundedSender<Message>,
}

impl Court {
    pub fn new(name: String, notify: MessageSender, redis: redis::Client) -> Self {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let state_manager = StateManager::new(redis, &name);

        let worker = CourtWorker {
            name,
            state: None,
            message_rx,
            notify,
            state_manager
                };

        tokio::spawn(worker.run());

        Court { message_tx }
    }

    pub fn update(&self, force: bool) {
        let _ = self.message_tx.send(Message::Update { force });
    }

    pub fn get_sessions(&self, chat: ChatData, date: String, reference: String) {
        let msg = Message::GetSessions {
            chat,
            date,
            reference,
        };
        let _ = self.message_tx.send(msg);
    }

    pub fn add_subscription(&self, chat: ChatData, name: String, reference: String) {
        let msg = Message::AddSubscription {
            chat,
            name,
            reference,
        };
        let _ = self.message_tx.send(msg);
    }

    pub fn remove_subscription(&self, chat: ChatData, name: Option<String>, reply_to_bot: bool) {
        let msg = Message::RemoveSubscription {
            chat,
            name,
            reply_to_bot,
        };
        let _ = self.message_tx.send(msg);
    }
}
