mod courts;
mod database;
mod messages;
mod reply_queue;
mod scraper;

use std::sync::Arc;

use courts::Courts;
use dptree::deps;
use teloxide::adaptors::DefaultParseMode;
use teloxide::macros::BotCommands;
use teloxide::prelude::*;
use teloxide::types::{ParseMode, ReplyParameters};
use teloxide::utils::command::ParseError;
use thiserror::Error;
use tokio::sync::Mutex;

use crate::database::Database;
use crate::messages::{help, MarkdownString};

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

type Bot = DefaultParseMode<teloxide::Bot>;

async fn answer(
    bot: Bot,
    msg: Message,
    cmd: Command,
    courts: Arc<Mutex<Courts>>,
    database: Database,
) -> ResponseResult<()> {
    log::info!("{:?}", cmd);

    let reply_fn = || {
        let bot = bot.clone();
        move |reply: MarkdownString| async move {
            let r = bot
                .send_message(msg.chat.id, reply.to_string())
                .reply_parameters(ReplyParameters::new(msg.id))
                .send()
                .await;

            if let Err(e) = r {
                log::warn!("Couldn't send message: {e}");
            }
        }
    };

    macro_rules! reply_and_return {
        ($reply:expr) => {{
            bot.send_message(msg.chat.id, $reply.to_string())
                .reply_parameters(ReplyParameters::new(msg.id))
                .send()
                .await?;

            return Ok(());
        }};
    }

    macro_rules! get_court {
        ($court:expr) => {
            match courts.lock().await.get(&$court) {
                Ok(x) => x,
                Err(_) => reply_and_return!("Ungültiger Gerichtsname!"),
            }
        };
    }
    match cmd {
        Command::Help => {
            reply_and_return!(help())
        }
        Command::Subscribe {
            name,
            court,
            reference,
        } => {
            get_court!(court); // assert name is valid
            let sub_id = database
                .add_subscription(msg.chat.id, &court, &name, &reference)
                .await;

            let reply = match sub_id {
                Ok(Some(subscription_id)) => {
                    get_court!(court).confirm_subscription(subscription_id, reply_fn());
                    return Ok(());
                }
                Ok(None) => messages::subscription_exists(&name),
                Err(e) => {
                    log::error!("Database error: {e}");
                    messages::internal_error()
                }
            };

            reply_and_return!(reply)
        }
        Command::ListSubscriptions => {
            let reply = match database.get_subscriptions_by_chat(msg.chat.id).await {
                Ok(subs) => messages::list_subscriptions(&subs),
                Err(e) => {
                    log::error!("Database error: {e}");
                    messages::internal_error()
                }
            };

            reply_and_return!(reply)
        }
        Command::Unsubscribe { name } => {
            let reply = match database.remove_subscription(msg.chat.id, &name).await {
                Ok(removed) => messages::unsubscribed(removed),
                Err(e) => {
                    log::error!("Database error: {e}");
                    messages::internal_error()
                }
            };

            reply_and_return!(reply)
        }
        Command::GetSessions {
            court,
            date,
            reference,
        } => {
            get_court!(court).get_sessions(date, reference, reply_fn());
        }
        Command::ForceUpdate { court } => get_court!(court).update(true),
    }

    Ok(())
}

#[tokio::main]
async fn main() {
    env_logger::init();
    log::info!("Starting bot...");

    let bot = teloxide::Bot::from_env().parse_mode(ParseMode::MarkdownV2);
    let database_url = std::env::var("DATABASE_URL").unwrap();
    let database = Database::new(&database_url).await.unwrap();
    let courts = Arc::new(Mutex::new(Courts::new(bot.clone(), database.clone()).await));

    Dispatcher::builder(
        bot,
        Update::filter_message()
            .filter_command::<Command>()
            .endpoint(answer),
    )
    .dependencies(deps![courts.clone(), database])
    .default_handler(|_| async {})
    .enable_ctrlc_handler()
    .build()
    .dispatch()
    .await;

    let Ok(courts) = Arc::try_unwrap(courts) else {
        panic!("Weird Arc<Courts> flying around")
    };
    courts.into_inner().shutdown().await;
}
