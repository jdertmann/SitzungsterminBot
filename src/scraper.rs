use std::borrow::Cow;
use std::time::Duration;

use chrono::prelude::*;
use chrono_tz::Europe;
use lazy_static::lazy_static;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use thiserror::Error;

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

#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct CourtData {
    pub full_name: String,
    pub sessions: Vec<Session>,
}

fn get_url(name: &str) -> String {
    format!("https://www.{name}.nrw.de/behoerde/sitzungstermine/index.php")
}

fn extract_text(e: &ElementRef) -> String {
    e.text().collect::<Vec<_>>().concat().trim().to_owned()
}

lazy_static! {
    static ref TR_SELECTOR: Selector =
        Selector::parse("table#sitzungsTermineTable tr[id].dataRow").unwrap();
    static ref NAME_SELECTOR: Selector = Selector::parse("meta[name=Copyright]").unwrap();
    static ref DATES_SELECTOR: Selector = Selector::parse("#startDate > option").unwrap();
}

struct IndexPageContent {
    full_name: String,
    urls: Vec<(NaiveDate, String)>,
}

async fn parse_index_page(
    url_name: &str,
    client: &reqwest::Client,
) -> Result<IndexPageContent, Error> {
    let url = get_url(url_name);
    log::info!("Get site {url}");
    let result = client.get(url).send().await?;
    let html = result.text().await?;
    let name = url_name.to_string();

    let document = Html::parse_document(&html);

    for error in &document.errors {
        log::info!("Parser error: {error}");
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

    Ok(IndexPageContent { full_name, urls })
}

fn parse_row(tr: ElementRef, date: NaiveDate) -> Session {
    macro_rules! get_cell {
        ($sel:literal) => {{
            lazy_static! {
                static ref SELECTOR: Selector = Selector::parse($sel).unwrap();
            }

            match tr.select(&SELECTOR).next() {
                Some(td) => extract_text(&td),
                None => String::new(),
            }
        }};
    }

    Session {
        date,
        time: get_cell!("td.termDate"),
        r#type: get_cell!("td.termType"),
        lawsuit: get_cell!("td.termLawsuit"),
        hall: get_cell!("td.termHall"),
        reference: get_cell!("td.termReference"),
        note: get_cell!("td.termNote"),
    }
}

async fn parse_table(
    url: &str,
    date: NaiveDate,
    client: &reqwest::Client,
) -> Result<Vec<Session>, Error> {
    log::info!("Fetch url {url}");
    let result = client.get(url).send().await?;
    let html = result.text().await?;
    let document = Html::parse_document(&html);

    for error in &document.errors {
        log::info!("Parser error: {error}");
    }

    let entries: Vec<_> = document
        .select(&TR_SELECTOR)
        .map(|tr| parse_row(tr, date))
        .collect();

    log::debug!("Got {} entries", entries.len());

    Ok(entries)
}

pub async fn get_court_data(url_name: &str) -> Result<CourtData, Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    let IndexPageContent { full_name, urls } = parse_index_page(url_name, &client).await?;

    let mut sessions = Vec::new();
    for (date, url) in urls {
        sessions.extend(parse_table(&url, date, &client).await?)
    }

    let data = CourtData {
        full_name,
        sessions,
    };
    Ok(data)
}
