use std::collections::HashSet;

use chrono::NaiveDate;
use regex::Regex;
use teloxide::utils::markdown::{bold, code_inline, escape};

use crate::database::Subscription;
use crate::scraper::{CourtInfo, Schedule, Session};

pub struct DateFilter {
    pub date: Option<NaiveDate>,
}

impl DateFilter {
    pub fn new(s: &str) -> Option<Self> {
        if s == "*" {
            return Some(Self { date: None });
        }

        let date = NaiveDate::parse_from_str(s, "%d.%m.%Y").ok()?;
        Some(DateFilter { date: Some(date) })
    }

    fn matches(&self, date: NaiveDate) -> bool {
        match &self.date {
            Some(match_date) => date == *match_date,
            None => true,
        }
    }
}

struct ReferenceFilter {
    regex: Regex,
}

impl ReferenceFilter {
    fn new(s: &str) -> Self {
        let regex_pattern = regex::escape(s).replace(r"\*", ".*").replace(r"\?", ".");
        let regex = Regex::new(&format!("^{regex_pattern}$")).unwrap();
        Self { regex }
    }

    fn matches(&self, reference: &str) -> bool {
        self.regex.is_match(reference)
    }
}

pub fn session_info(entry: &Session) -> String {
    let datetime = format!("{}, {}", entry.date.format("%A, %-d %B %C%y"), entry.time);

    let byline = if entry.lawsuit.is_empty() {
        entry.r#type.clone()
    } else {
        format!("{}, {}", entry.lawsuit, entry.r#type)
    };

    let mut result = format!(
        "{}\nSitzungssaal {}\n{}\nAktenzeichen: {}",
        bold(&escape(&datetime)),
        escape(if entry.hall.is_empty() {
            "unbekannt"
        } else {
            &entry.hall
        }),
        escape(&byline),
        code_inline(&entry.reference)
    );

    if !entry.note.is_empty() {
        result += &format!("\nHinweis: {}", escape(&entry.note));
    }

    result
}

pub fn create_list(schedule: &Schedule, filter: impl Fn(&Session) -> bool) -> (u64, String) {
    let mut result = String::new();
    let mut count = 0;
    for session in schedule {
        if !filter(session) {
            continue;
        }

        count += 1;

        result += "\n\n";
        result += &session_info(&session)
    }

    (count, result)
}

pub fn invalid_date() -> String {
    escape("Das angegebene Datum ist ungültig.")
}

pub fn list_sessions(info: &Option<CourtInfo>, reference: &str) -> String {
    let Some(info) = info else {
        return escape("Leider sind keine Informationen für dieses Gericht verfügbar.");
    };

    let reference = ReferenceFilter::new(reference);

    let (count, list) = create_list(&info.schedule, |session| {
        reference.matches(&session.reference)
    });

    match count {
        0 => format!(
            "Leider wurden keine Termine für das {}, die zu deinem Filter passen, gefunden\\.",
            bold(&escape(&info.full_name))
        ),
        1 => format!(
            "Es wurde 1 Termin für das {} gefunden:{}",
            bold(&escape(&info.full_name)),
            list
        ),
        _ => format!(
            "Es wurden {} Termine für das {} gefunden:{}",
            escape(&count.to_string()),
            bold(&escape(&info.full_name)),
            list
        ),
    }
}

pub fn subscribed(name: &str, court_info: &Option<CourtInfo>, reference: &str) -> String {
    let mut result = escape(&format!("Dein Abo „{name}” wurde entgegengenommen."));
    match court_info {
        Some(info) => {
            let reference = ReferenceFilter::new(reference);
            let (count, list) = create_list(&info.schedule, |session| {
                reference.matches(&session.reference)
            });

            match count {
                0 => {
                    result += &escape(
                        " Zur Zeit gibt es nichts zu melden, aber ich halt dich auf dem Laufenden!",
                    )
                }
                _ => {
                    result += &escape(" Hier schon mal eine Liste der anstehenden Termine:");
                    result += &list;
                    result += &escape("\n\nBei neuen Terminen werde ich dich benachrichtigen!");
                }
            }
        }
        None => {
            result += &escape(" Ich kann die Website des Gerichts leider nicht erreichen, aber ich halt dich auf dem Laufenden.");
        }
    }

    result
}

pub fn subscription_exists(name: &str) -> String {
    escape("Ein Abo mit diesem Namen existiert bereits!")
}

pub fn list_subscriptions(list: &[Subscription]) -> String {
    escape("Deine Abos:\n\n")
        + &list
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
}

pub fn unsubscribed(removed: bool) -> String {
    if removed {
        escape("Abo wurde gelöscht 👍")
    } else {
        escape("Es wurde kein Abo mit dem Namen gefunden.")
    }
}

pub fn handle_update(
    old_schedule: &Schedule,
    new_schedule: &Schedule,
    full_court_name: &str,
    subscription_name: &str,
    reference_filter: &str,
) -> Option<String> {
    let reference = ReferenceFilter::new(reference_filter);

    let old_sessions: HashSet<_> = old_schedule.into_iter().collect();

    let (count, list) = create_list(new_schedule, |session| {
        reference.matches(&session.reference) && !old_sessions.contains(&session)
    });

    if count == 0 {
        return None;
    }

    let msg = escape("Für dein Abo „")
        + &bold(&escape(subscription_name))
        + &escape(&format!("” ({full_court_name}) gibt es Neuigkeiten:"))
        + &list;

    Some(msg)
}

pub fn internal_error() -> String {
    escape("Sorry, ein interner Fehler ist aufgetreten :((")
}
