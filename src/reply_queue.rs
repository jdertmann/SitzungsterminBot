use std::time::Duration;

use teloxide::prelude::*;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::messages::MarkdownString;
use crate::Bot;

#[derive(Clone)]
pub struct ReplyQueue(mpsc::UnboundedSender<(ChatId, MarkdownString)>);

impl ReplyQueue {
    async fn send(bot: &Bot, chat_id: ChatId, msg: MarkdownString) {
        let result = bot
            .send_message(chat_id, msg.to_string())
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await;

        if let Err(e) = result {
            log::warn!("Couldn't send message to {chat_id}: {e}")
        }
    }

    pub fn new(bot: Bot) -> (Self, JoinHandle<()>) {
        let (tx, mut rx) = mpsc::unbounded_channel::<(ChatId, MarkdownString)>();

        let handle = tokio::task::spawn(async move {
            let mut buffer = Vec::with_capacity(20);
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            while rx.recv_many(&mut buffer, 20).await > 0 {
                for (c, s) in buffer.iter() {
                    Self::send(&bot, *c, s.clone()).await;
                }
                buffer.clear();
                interval.tick().await;
            }

            log::info!("Reply queue task shut down.");
        });

        (Self(tx), handle)
    }

    pub fn queue(&self, chat_id: ChatId, msg: MarkdownString) {
        if self.0.send((chat_id, msg)).is_err() {
            log::error!("Queuing message failed!")
        }
    }
}
