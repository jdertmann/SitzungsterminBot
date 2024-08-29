use std::collections::HashSet;

use chrono::NaiveDate;
use regex::Regex;
use teloxide::utils::markdown::{bold, code_inline, escape};

use crate::scraper::{CourtInfo, Session};

struct DateFilter {
    date: Option<NaiveDate>,
}

impl DateFilter {
    fn new(s: &str) -> Option<Self> {
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

pub fn create_list(info: &CourtInfo, filter: impl Fn(&Session) -> bool) -> (u64, String) {
    let mut result = String::new();
    let mut count = 0;
    for session in &info.schedule {
        if !filter(session) {
            continue;
        }

        count += 1;

        result += "\n\n";
        result += &session_info(&session)
    }

    (count, result)
}

pub fn list_sessions(info: &Result<CourtInfo, ()>, reference: &str, date: &str) -> String {
    let Ok(info) = info else {
        return escape("Leider sind keine Informationen für dieses Gericht verfügbar.");
    };

    let reference = ReferenceFilter::new(reference);
    let Some(date) = DateFilter::new(date) else {
        return escape("Das angegebene Datum ist ungültig.");
    };

    let (count, list) = create_list(info, |session| {
        reference.matches(&session.reference) && date.matches(session.date)
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

pub fn subscribed(name: &str, court_info: &Result<CourtInfo, ()>, reference: &str) -> String {
    let mut result = escape(&format!("Dein Abo „{name}” wurde entgegengenommen."));
    match court_info {
        Ok(info) => {
            let reference = ReferenceFilter::new(reference);
            let (count, list) = create_list(info, |session| reference.matches(&session.reference));

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
        Err(_) => {
            result += &escape(" Ich kann die Website des Gerichts leider nicht erreichen, aber ich halt dich auf dem Laufenden.");
        }
    }

    result
}

pub fn unsubscribed(n: usize) -> String {
    if n == 0 {
        escape("Es wurde kein Abo mit dem Namen gefunden.")
    } else {
        escape("Abo wurde gelöscht 👍")
    }
}

pub fn handle_update(
    old_info: &Result<CourtInfo, ()>,
    new_info: &Result<CourtInfo, ()>,
    name: &str,
    reference: &str,
) -> Option<String> {
    let new_info = new_info.as_ref().ok()?;

    let reference = ReferenceFilter::new(reference);

    let old_sessions: HashSet<_> = match old_info {
        Ok(info) => info.schedule.iter().collect(),
        Err(_) => HashSet::new(),
    };

    let (count, list) = create_list(new_info, |session| {
        reference.matches(&session.reference) && !old_sessions.contains(&session)
    });

    if count == 0 {
        return None;
    }

    Some(escape(&format!("Für dein Abo „{name}” gibt es Neuigkeiten:")) + &list)
}

pub fn internal_error() -> String {
    escape(&format!("Sorry, ein interner Fehler ist aufgetreten :(("))
}
