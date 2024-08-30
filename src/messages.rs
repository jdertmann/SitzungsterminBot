use std::collections::HashSet;

use regex::Regex;
use teloxide::utils::markdown::{bold, code_inline, escape};

use crate::database::Subscription;
use crate::scraper::{CourtData, Session};

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
    let datetime = format!("{}, {}", entry.date.format("%A, %-d. %B %C%y"), entry.time);

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

pub fn create_list(sessions: &[Session], filter: impl Fn(&Session) -> bool) -> (u64, String) {
    let mut result = String::new();
    let mut count = 0;
    for session in sessions {
        if !filter(session) {
            continue;
        }

        count += 1;

        result += "\n\n";
        result += &session_info(session)
    }

    (count, result)
}

pub fn invalid_date() -> String {
    escape("Das angegebene Datum ist ungültig.")
}

pub fn list_sessions(court_data: &Option<CourtData>, reference: &str) -> String {
    let Some(court_data) = court_data else {
        return escape("Leider sind keine Informationen für dieses Gericht verfügbar.");
    };

    let reference = ReferenceFilter::new(reference);

    let (count, list) = create_list(&court_data.sessions, |session| {
        reference.matches(&session.reference)
    });

    match count {
        0 => format!(
            "Leider wurden keine Termine für das {}, die zu deinem Filter passen, gefunden\\.",
            bold(&escape(&court_data.full_name))
        ),
        1 => format!(
            "Es wurde 1 Termin für das {} gefunden:{}",
            bold(&escape(&court_data.full_name)),
            list
        ),
        _ => format!(
            "Es wurden {} Termine für das {} gefunden:{}",
            escape(&count.to_string()),
            bold(&escape(&court_data.full_name)),
            list
        ),
    }
}

pub fn subscribed(name: &str, court_data: &Option<CourtData>, reference: &str) -> String {
    let mut result = escape(&format!("Dein Abo „{name}” wurde entgegengenommen."));
    match court_data {
        Some(data) => {
            let reference = ReferenceFilter::new(reference);
            let (count, list) = create_list(&data.sessions, |session| {
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
    escape(&format!(
        "Ein Abo mit dem Namen „{name}” existiert bereits!"
    ))
}

pub fn list_subscriptions(list: &[Subscription]) -> String {
    if list.is_empty() {
        escape("Du hast zur Zeit keine Abos am laufen!")
    } else {
        escape("Hier ist eine Liste deiner Abos:\n\n")
            + &list
                .iter()
                .map(|s| {
                    bold(&escape(&s.name))
                        + &escape(&format!(
                            "\nGericht: {}\nAktenzeichen: {}",
                            s.court, s.reference_filter
                        ))
                })
                .collect::<Vec<_>>()
                .join("\n\n")
    }
}

pub fn unsubscribed(removed: bool) -> String {
    if removed {
        escape("Abo wurde gelöscht 👍")
    } else {
        escape("Es wurde kein Abo mit diesem Namen gefunden.")
    }
}

pub fn sessions_updated(
    old_sessions: &[Session],
    new_sessions: &[Session],
    full_court_name: &str,
    subscription_name: &str,
    reference_filter: &str,
) -> Option<String> {
    let reference = ReferenceFilter::new(reference_filter);

    let old_sessions: HashSet<_> = old_sessions.iter().collect();

    let (count, list) = create_list(new_sessions, |session| {
        reference.matches(&session.reference) && !old_sessions.contains(&session)
    });

    if count == 0 {
        return None;
    }

    let msg = escape("🔔 Für dein Abo „")
        + &bold(&escape(subscription_name))
        + &escape(&format!(
            "” ({full_court_name}) wurden neue Termine veröffentlicht!"
        ))
        + &list;

    Some(msg)
}

pub fn internal_error() -> String {
    escape("Sorry, ein interner Fehler ist aufgetreten :((")
}
