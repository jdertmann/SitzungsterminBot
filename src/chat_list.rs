use std::{collections::{BTreeMap, HashMap}, sync::Arc};

use teloxide::types::ChatId;
use tokio::sync::{RwLock, Mutex};

#[derive(Clone)]
pub struct ChatData {
    id: ChatId,
    // inner: Arc<Mutex<ChatDataInner>>
}

impl ChatData {
    pub fn new(id: ChatId) -> Self {
        Self {
            id,
           // inner: Default::default()
        }
    }

    pub fn get_id(&self) -> ChatId {
        self.id
    }
}

#[derive(Default)]
struct ChatDataInner {
    subscriptions: BTreeMap<String, String> // non-authorative
}
