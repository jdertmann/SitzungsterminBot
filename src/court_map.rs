use regex::Regex;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

use crate::{court::Court, MessageSender};

type Map = HashMap<String, Court>;

#[derive(Clone)]
pub struct CourtMap {
    notification_queue: MessageSender,
    courts: Arc<RwLock<Map>>,
    redis: redis::Client,
    validate_regex: Regex,
}

impl CourtMap {
    pub fn new(notification_queue: MessageSender, redis: redis::Client) -> Self {
        Self {
            notification_queue,
            courts: Default::default(),
            redis,
            validate_regex: Regex::new("^[a-zA-Z0-9\\-]+$").unwrap(),
        }
    }

    fn validate_name(&self, name: &str) -> bool {
        self.validate_regex.is_match(name)
    }

    pub async fn get(&self, name: &str) -> Option<Court> {
        if !self.validate_name(name) {
            return None;
        }

        {
            let courts = self.courts.read().await;
            if let Some(court) = courts.get(name) {
                return Some(court.clone());
            }
        }

        let court = Court::new(
            name.to_string(),
            self.notification_queue.clone(),
            self.redis.clone(),
        );
        let court2 = court.clone();
        {
            let mut courts = self.courts.write().await;
            courts.insert(name.to_string(), court);
        }
        Some(court2)
    }
}
