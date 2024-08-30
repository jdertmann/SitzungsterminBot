use std::borrow::Cow;
use std::time::Duration;

use chrono::prelude::*;
use chrono_tz::Europe;
use lazy_static::lazy_static;
use scraper::selectable::Selectable;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use teloxide::requests;
use thiserror::Error;
use tokio::task::spawn_blocking;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to retrieve website: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("invalid website content")]
    ParseError(Cow<'static, str>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::FromRow)]
pub struct Session {
    pub date: NaiveDate,
    pub time: String,
    pub r#type: String,
    pub lawsuit: String,
    pub hall: String,
    pub reference: String,
    pub note: String,
}

fn get_url(name: &str) -> String {
    format!("https://www.{name}.nrw.de/behoerde/sitzungstermine/index.php")
}

fn get_inner_text(e: &ElementRef) -> String {
    let mut text = String::new();
    for s in e.text() {
        text.push_str(s);
    }
    text
}

lazy_static! {
    static ref TR_SELECTOR: Selector =
        Selector::parse("table#sitzungsTermineTable tr[id].dataRow").unwrap();
    static ref NAME_SELECTOR: Selector = Selector::parse("meta[name=Copyright]").unwrap();
    static ref DATES_SELECTOR: Selector = Selector::parse("#startDate > option").unwrap();
}

async fn parse_index_page(url_name: &str, client: &reqwest::Client) -> Result<(String, Vec<(NaiveDate, String)>), Error> {
    let url = get_url(url_name);
    log::info!("Get site {url}");
    let result = client.get(url).send().await?;
    let html = result.text().await?;
    let name = url_name.to_string();

    spawn_blocking(move || {
        let document = Html::parse_document(&html);

        if let Some(error) = document.errors.first() {
            return Err(Error::ParseError(error.clone()));
        }

        let full_name = document
            .select(&NAME_SELECTOR)
            .next()
            .ok_or(Error::ParseError("meta tag not on site".into()))?
            .attr("content")
            .ok_or(Error::ParseError("invalid meta tag".into()))?
            .to_string();

        let urls = document.select(&DATES_SELECTOR).filter_map(|elem| {
            let date_unix = elem.value().attr("value")?;
            let date_unix : i64 = match date_unix.trim().parse() {
                Ok(timestamp) => timestamp,
                Err(_) => {
                    log::warn!("Timestamp {date_unix} is no valid number.");
                    return None
                }
            };

            let date = DateTime::from_timestamp(date_unix, 0)?.with_timezone(&Europe::Berlin).date_naive();
            let url = format!("https://www.{name}.nrw.de/behoerde/sitzungstermine/index.php?startDate={date_unix}&termsPerPage=0");
            Some((date, url))
        }).collect();

        Ok((full_name,urls))
    }).await.unwrap()
}

fn parse_row(tr: ElementRef, date: NaiveDate) -> Session {
    macro_rules! get_cell_content {
        ($sel:literal) => {{
            lazy_static! {
                static ref SELECTOR: Selector = Selector::parse($sel).unwrap();
            }

            match tr.select(&SELECTOR).next() {
                Some(td) => get_inner_text(&td),
                None => String::new(),
            }
        }};
    }

    Session {
        date,
        time: get_cell_content!("td.termDate"),
        r#type: get_cell_content!("td.termType"),
        lawsuit: get_cell_content!("td.termLawsuit"),
        hall: get_cell_content!("td.termHall"),
        reference: get_cell_content!("td.termReference"),
        note: get_cell_content!("td.termNote"),
    }
}

async fn parse_table(url: &str, date: NaiveDate, client: &reqwest::Client) -> Result<Vec<Session>, Error> {
    log::info!("Get site {url}");
    let result = client.get(url).send().await?;
    let html = result.text().await?;

    spawn_blocking(move || {
        let document = Html::parse_document(&html);

        if let Some(error) = document.errors.first() {
            return Err(Error::ParseError(error.clone()));
        }

        let entries: Vec<_> = document
            .select(&TR_SELECTOR)
            .map(|tr| parse_row(tr, date))
            .collect();
        log::debug!("Got {} entries", entries.len());
        Ok(entries)
    })
    .await
    .unwrap()
}

pub type Schedule = Vec<Session>;

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct CourtInfo {
    pub full_name: String,
    pub schedule: Schedule,
}

pub async fn get_court_info(url_name: &str) -> Result<CourtInfo, Error> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(5)).build()?;
    let mut schedule = Vec::new();
    let (full_name, urls) = parse_index_page(url_name, &client).await?;
    log::debug!("Found urls: {:?}", urls);
    for (date, url) in urls {
        schedule.extend(parse_table(&url, date, &client).await?)
    }

    Ok(CourtInfo {
        full_name,
        schedule,
    })
}
