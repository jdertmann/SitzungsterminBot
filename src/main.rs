mod court;
mod court_manager;
mod scraper;

use court::CourtState;
use court_manager::CourtManager;
use teloxide::prelude::*;

use teloxide::utils::markdown::{bold, escape};

#[tokio::main]
async fn main() {
    env_logger::init();
    log::info!("Starting bot...");

    let bot = Bot::from_env();
    let court_manager = CourtManager::new();

    teloxide::repl(bot, move |bot: Bot, msg: Message| {
        let court_manager = court_manager.clone();
        async move {
            let Some(text) = msg.text()  else { return Ok(()); };
            let Some((court, reference)) = text.split_once(' ')  else { return Ok(());};

            let court = court_manager.get(court).await;
            let info = match court.update_and_get(false).await {
                CourtState::Valid(i) => i, _ => panic!()
        };
            for entry in info.schedule {
                if entry.reference != reference  {continue;}
                let body = format!("\nDatum: {}\nUhrzeit: {}\nTermin: {}\nVerfahren: {}\nAktenzeichen: {}\nSitzungssaal: {}\nHinweis: {}", entry.date, entry.time, entry.type_, entry.lawsuit, entry.reference, entry.hall, entry.note);
                let message = format!("{}{}", bold(&escape(&info.full_name)), escape(&body));

                bot.send_message(msg.chat.id, message).parse_mode(teloxide::types::ParseMode::MarkdownV2).await?;
            }
            Ok(())
        }
    })
    .await;
}