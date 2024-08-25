use chrono::prelude::*;
use tokio::sync::{mpsc, oneshot, watch};

use crate::scraper;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum CourtState {
    Uninitialized,
    Valid(scraper::CourtInfo),
    Invalid,
}

enum Message {
    UpdateAndGet {
        force_update: bool,
        response_tx: oneshot::Sender<CourtState>,
    },
}

#[derive(Clone)]
struct CourtData {
    name: String,
    last_update: Option<DateTime<Utc>>,
    state: CourtState,
}

const TRESHOLD_TIME: NaiveTime = NaiveTime::from_hms(7, 5, 0);

impl CourtData {
    fn is_out_of_date(&self) -> bool {
        let Some(last_updated) = self.last_update else {
            return true;
        };

        let now = Utc::now().with_timezone(&chrono_tz::Europe::Berlin);

        let date = if now.time() < TRESHOLD_TIME {
            now.date_naive()
                .checked_sub_days(chrono::Days::new(1))
                .unwrap()
        } else {
            now.date_naive()
        };

        let treshold = date
            .and_time(TRESHOLD_TIME)
            .and_local_timezone(chrono_tz::Europe::Berlin)
            .unwrap()
            .to_utc();

        return last_updated < treshold;
    }

    async fn update(&mut self, force: bool) {
        if !force && matches!(&self.state, CourtState::Valid(_)) && !self.is_out_of_date() {
            return;
        }

        self.last_update = Some(Utc::now());
        self.state = match scraper::get_court_info(&self.name).await {
            Ok(x) => CourtState::Valid(x),
            Err(_) => CourtState::Invalid,
        }
    }
}

#[derive(Clone)]
pub struct Court {
    message_tx: mpsc::UnboundedSender<Message>,
    watch_rx: watch::Receiver<CourtState>,
}

impl Court {
    pub fn new(name: &str) -> Self {
        let (watch_tx, watch_rx) = watch::channel(CourtState::Uninitialized);
        let (message_tx, mut message_rx) = mpsc::unbounded_channel();
        let mut data = CourtData {
            name: name.to_string(),
            last_update: None,
            state: CourtState::Uninitialized,
        };

        tokio::spawn(async move {
            while let Some(msg) = message_rx.recv().await {
                match msg {
                    Message::UpdateAndGet {
                        force_update,
                        response_tx,
                    } => {
                        data.update(force_update).await;
                        watch_tx.send_if_modified(|x| {
                            if *x != data.state {
                                *x = data.state.clone();
                                true
                            } else {
                                false
                            }
                        });

                        let _ = response_tx.send(data.state.clone());
                    }
                }
            }
        });

        Court {
            message_tx,
            watch_rx,
        }
    }

    pub async fn update_and_get(&self, force_update: bool) -> CourtState {
        let (tx, rx) = oneshot::channel();
        self.message_tx
            .send(Message::UpdateAndGet {
                force_update,
                response_tx: tx,
            })
            .unwrap();
        rx.await.unwrap()
    }

    pub fn subscribe(&self) -> watch::Receiver<CourtState> {
        self.watch_rx.clone()
    }
}
