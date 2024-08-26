use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use crate::{court::Court, MessageSender};

type Map = HashMap<String, Court>;

#[derive(Clone)]
pub struct CourtMap {
    notification_queue: MessageSender,
    courts: Arc<Mutex<Map>>,
    redis: redis::Client,
}

impl CourtMap {
    pub fn new(notification_queue: MessageSender, redis: redis::Client) -> Self {
        Self {
            notification_queue,
            courts: Default::default(),
            redis
        }
    }

    pub async fn get(&self, name: &str) -> Court {
        let mut courts = self.courts.lock().await;
        courts
            .entry(name.to_string())
            .or_insert_with(|| {
                Court::new(name.to_string(), self.notification_queue.clone(), self.redis.clone())
            })
            .clone()
    }
}
