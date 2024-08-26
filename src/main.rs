mod chat_list;
mod court;
mod court_map;
mod messages;
mod scraper;

use std::time::Duration;

use chat_list::ChatData;
// use chat_list::ChatData;
use court_map::CourtMap;
use teloxide::{
    macros::BotCommands,
    prelude::*,
    utils::command::{BotCommands as _, ParseError},
};

use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Clone)]
struct MessageSender {
    queue: mpsc::UnboundedSender<(ChatId, String)>,
    direct: Bot,
}

impl MessageSender {
    async fn send_inner(bot: &Bot, chat_id: ChatId, msg: String) {
        let result = bot
            .send_message(chat_id, msg)
            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
            .await;

        if let Err(e) = result {
            log::warn!("Couldn't send message to {chat_id}: {e}")
        }
    }
    fn new(bot: Bot) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<(ChatId, String)>();
        let direct = bot.clone();
        tokio::task::spawn(async move {
            let mut buffer = Vec::with_capacity(20);

            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                rx.recv_many(&mut buffer, 20).await;
                for (c, s) in buffer.iter() {
                    Self::send_inner(&bot, *c, s.to_string()).await;
                }
                buffer.clear();
                interval.tick().await;
            }
        });

        Self { queue: tx, direct }
    }

    fn send(&self, chat_id: ChatId, msg: String) {
        let _ = self.queue.send((chat_id, msg));
    }

    async fn send_direct(&self, chat_id: ChatId, msg: String) {
        Self::send_inner(&self.direct, chat_id, msg).await;
    }
}

#[derive(Error, Debug)]
#[error("Error while parsing arguments in posix-shell manner")]
struct ShlexError;

fn split(s: String) -> Result<(String, String, String), ParseError> {
    let split = shlex::split(&s).ok_or(ParseError::IncorrectFormat(Box::new(ShlexError)))?;

    match split.len() {
        ..=2 => Err(ParseError::TooFewArguments {
            expected: 3,
            found: split.len(),
            message: String::from("Please use quotes like in posix-shells"),
        }),
        3 => {
            let [a, b, c] = split.try_into().unwrap();
            Ok((a, b, c))
        }
        4.. => Err(ParseError::TooManyArguments {
            expected: 3,
            found: split.len(),
            message: String::from("Please use quotes like in posix-shells"),
        }),
    }
}

#[derive(BotCommands, Clone, Debug)]
#[command(
    rename_rule = "snake_case",
    parse_with = "split",
    description = "Diese Befehle werden unterstützt:"
)]
enum Command {
    #[command(description = "zeige diesen Text an.")]
    Help,
    #[command(description = "abonniere ein Verfahren.", parse_with = split)]
    Subscribe {
        name: String,
        court: String,
        reference: String,
    },
    #[command(description = "entferne ein Abonnement.")]
    Unsubscribe {
        name: String,
    },
    #[command(description = "zeige Termine an.", parse_with = split)]
    GetSessions {
        court: String,
        date: String,
        reference: String,
    },
    ForceUpdate {
        court: String,
    },
}

#[tokio::main]
async fn main() {
    env_logger::init();
    log::info!("Starting bot...");

    let bot = Bot::from_env();
    let notification_queue = MessageSender::new(bot.clone());
    let redis = redis::Client::open("redis://127.0.0.1/").unwrap();
    let court_map = CourtMap::new(notification_queue, redis);
    //let chat_data : Arc<RwLock<HashMap<ChatId, ChatData>>> = Default::default();

    let answer = move |bot: Bot, msg: Message, cmd: Command| {
        let court_map = court_map.clone();
        //let chat_data = chat_data.clone();
        async move {
            log::info!("{:?}", cmd);

            match cmd {
                Command::Help => {
                    let response = Command::descriptions().to_string();
                    bot.send_message(msg.chat.id, response).await?;
                }
                Command::Subscribe {
                    name,
                    court,
                    reference,
                } => court_map.get(&court).await.add_subscription(
                    ChatData::new(msg.chat.id),
                    name,
                    reference,
                ),
                Command::Unsubscribe { name } => {
                    todo!(); //court_manager.get(&court).await.remove_subscription(msg.chat.id, Some(name), true);
                }
                Command::GetSessions {
                    court,
                    date,
                    reference,
                } => court_map.get(&court).await.get_sessions(
                    ChatData::new(msg.chat.id),
                    date,
                    reference,
                ),
                Command::ForceUpdate { court } => court_map.get(&court).await.update(true),
            }
            Ok(())
        }
    };

    Command::repl(bot, answer).await;
}
