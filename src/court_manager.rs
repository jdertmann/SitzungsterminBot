use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use crate::court::Court;

type Map = HashMap<String, Court>;

#[derive(Clone, Default)]
pub struct CourtManager {
    courts: Arc<Mutex<Map>>,
}

impl CourtManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, name: &str) -> Court {
        let mut courts = self.courts.lock().await;
        courts
            .entry(name.to_string())
            .or_insert_with(|| Court::new(name))
            .clone()
    }
}
