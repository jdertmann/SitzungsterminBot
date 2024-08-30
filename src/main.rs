mod chat_list;
mod court;
mod database;
mod messages;
mod scraper;

use std::sync::Arc;
use std::time::Duration;

use chat_list::ChatData;
// use chat_list::ChatData;
use court::Courts;
use database::Database;
use dptree::deps;
use teloxide::filter_command;
use teloxide::macros::BotCommands;
use teloxide::prelude::*;
use teloxide::types::{MessageId, ReplyParameters};
use teloxide::utils::command::ParseError;
use thiserror::Error;
use tokio::sync::{mpsc, Mutex};

#[derive(Clone)]
struct MessageSender {
    queue: mpsc::UnboundedSender<(ChatId, String, Option<MessageId>)>,
    direct: Bot,
}

impl MessageSender {
    async fn send_inner(bot: &Bot, chat_id: ChatId, msg: String, reply_to: Option<MessageId>) {
        let mut result = bot
            .send_message(chat_id, msg)
            .parse_mode(teloxide::types::ParseMode::MarkdownV2);

        if let Some(reply_to) = reply_to {
            result = result.reply_parameters(ReplyParameters::new(reply_to));
        }

        let result = result.await;

        if let Err(e) = result {
            log::warn!("Couldn't send message to {chat_id}: {e}")
        }
    }

    fn new(bot: Bot) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<(ChatId, String, Option<MessageId>)>();
        let direct = bot.clone();
        tokio::task::spawn(async move {
            let mut buffer = Vec::with_capacity(20);

            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                rx.recv_many(&mut buffer, 20).await;
                for (c, s, m) in buffer.iter() {
                    Self::send_inner(&bot, *c, s.to_string(), *m).await;
                }
                buffer.clear();
                interval.tick().await;
            }
        });

        Self { queue: tx, direct }
    }

    fn send(&self, chat_id: ChatId, msg: String, reply_to: Option<MessageId>) {
        let _ = self.queue.send((chat_id, msg, reply_to));
    }

    async fn send_direct(&self, chat_id: ChatId, msg: String, reply_to: Option<MessageId>) {
        Self::send_inner(&self.direct, chat_id, msg, reply_to).await;
    }
}

#[derive(Error, Debug)]
#[error("Error while parsing arguments in posix-shell manner")]
struct ShlexError;

fn split1(s: String) -> Result<(String,), ParseError> {
    let split = shlex::split(&s).ok_or(ParseError::IncorrectFormat(Box::new(ShlexError)))?;

    match split.len() {
        ..=0 => Err(ParseError::TooFewArguments {
            expected: 3,
            found: split.len(),
            message: String::from("Please use quotes like in posix-shells"),
        }),
        1 => {
            let [a] = split.try_into().unwrap();
            Ok((a,))
        }
        2.. => Err(ParseError::TooManyArguments {
            expected: 3,
            found: split.len(),
            message: String::from("Please use quotes like in posix-shells"),
        }),
    }
}
fn split3(s: String) -> Result<(String, String, String), ParseError> {
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
    #[command(description = "abonniere ein Verfahren.", parse_with = split3)]
    Subscribe {
        name: String,
        court: String,
        reference: String,
    },
    #[command(description = "zeige deine Abos an.")]
    ListSubscriptions,
    #[command(description = "entferne ein Abo.", parse_with= split1)]
    Unsubscribe {
        name: String,
    },
    #[command(description = "zeige Termine an.", parse_with = split3)]
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
    let database_url = std::env::var("DATABASE_URL").unwrap();
    let database = Database::new(&database_url).await.unwrap();
    let courts = Arc::new(Mutex::new(
        Courts::new(notification_queue, database.clone()).await,
    ));
    //let chat_data : Arc<RwLock<HashMap<ChatId, ChatData>>> = Default::default();

    let answer =
        |bot: Bot, msg: Message, cmd: Command, courts: Arc<Mutex<Courts>>, database: Database| {
            async move {
                log::info!("{:?}", cmd);
                let chat_id = msg.chat.id;
                macro_rules! get_court {
                    ($court:expr) => {
                        match courts.lock().await.get(&$court) {
                            Ok(x) => x,
                            Err(_) => {
                                bot.send_message(msg.chat.id, "Ungültiger Gerichtsname!")
                                    .reply_parameters(ReplyParameters::new(msg.id))
                                    .await?;
                                return Ok(());
                            }
                        }
                    };
                }
                match cmd {
                    Command::Help => {
                        bot.send_message(msg.chat.id, HELP_MESSAGE)
                            .reply_parameters(ReplyParameters::new(msg.id))
                            .await?;
                    }
                    Command::Subscribe {
                        name,
                        court,
                        reference,
                    } => {
                        get_court!(court); // assert name is valid
                        let reply = match database
                            .add_subscription(msg.chat.id, &court, &name, &reference)
                            .await
                        {
                            Ok(Some(subscription_id)) => {
                                get_court!(court).confirm_subscription(msg, subscription_id);
                                return Ok(());
                            }
                            Ok(None) => messages::subscription_exists(&name),
                            Err(e) => {
                                log::error!("Database error: {e}");
                                messages::internal_error()
                            }
                        };
                        bot.send_message(msg.chat.id, reply)
                            .reply_parameters(ReplyParameters::new(msg.id))
                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                            .await?;
                        return Ok(());
                    }
                    Command::ListSubscriptions => {
                        let reply = match database.get_subscriptions_by_chat(msg.chat.id).await {
                            Ok(subs) => messages::list_subscriptions(&subs),
                            Err(e) => {
                                log::error!("Database error: {e}");
                                messages::internal_error()
                            }
                        };

                        bot.send_message(msg.chat.id, reply)
                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                            .reply_parameters(ReplyParameters::new(msg.id))
                            .await?;
                        return Ok(());
                    }
                    Command::Unsubscribe { name } => {
                        let reply = match database.remove_subscription(msg.chat.id, &name).await {
                            Ok(removed) => messages::unsubscribed(removed),
                            Err(e) => {
                                log::error!("Database error: {e}");
                                messages::internal_error()
                            }
                        };

                        bot.send_message(msg.chat.id, reply)
                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                            .reply_parameters(ReplyParameters::new(msg.id))
                            .await?;
                        return Ok(());
                    }
                    Command::GetSessions {
                        court,
                        date,
                        reference,
                    } => get_court!(court).get_sessions(msg, date, reference),
                    Command::ForceUpdate { court } => get_court!(court).update(true),
                }
                Ok::<(), teloxide::RequestError>(())
            }
        };

    Dispatcher::builder(
        bot,
        Update::filter_message()
            .filter_command::<Command>()
            .endpoint(answer),
    )
    .dependencies(deps![courts, database])
    .default_handler(|_| async {})
    .enable_ctrlc_handler()
    .build()
    .dispatch()
    .await
}

const HELP_MESSAGE : &str = "
Unterstützte Befehle:
/help
/get_sessions <Gericht> <Datum> <Aktenzeichen>
/subscribe <beliebiger Name> <Gericht> <Aktenzeichen>

Wenn ein Parameter Leerzeichen enthält, muss er in Anführungszeichen gesetzt werden.

Der Name des Gerichts muss sein wie in der URL der Website, also z.B. \"vg-koeln\".

Das Datum kann auch \"*\" sein, um jedes Datum zu erfassen.

Im Aktenzeichen steht \"?\" für ein beliebiges einzelnes Zeichen,  \"*\" für eine beliebige Zeichenkette.

Keine Gewähr für verpasste Termine!";
