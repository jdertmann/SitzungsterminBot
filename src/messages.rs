mod markdown_string;

use std::collections::HashSet;

use regex::Regex;

pub use self::markdown_string::MarkdownString;
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

pub fn session_info(entry: &Session) -> MarkdownString {
    let datetime = format!("{}, {}", entry.date.format("%A, %-d. %B %C%y"), entry.time);

    let byline = if entry.lawsuit.is_empty() {
        entry.r#type.clone()
    } else {
        format!("{}, {}", entry.lawsuit, entry.r#type)
    };

    let hall = if entry.hall.is_empty() {
        "unbekannt"
    } else {
        entry.hall.as_str()
    };

    let mut result = MarkdownString::from_str(&datetime).bold();
    result += format!("\nSitzungssaal {}\n{}\nAktenzeichen: ", hall, byline).as_str();
    result += &MarkdownString::code_inline(&entry.reference);

    if !entry.note.is_empty() {
        result += format!("\nHinweis: {}", entry.note).as_str();
    }

    result
}

pub fn create_list(
    sessions: &[Session],
    filter: impl Fn(&Session) -> bool,
) -> (u64, MarkdownString) {
    let mut result = MarkdownString::new();
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

pub fn invalid_date() -> MarkdownString {
    "Das angegebene Datum ist ungültig.".into()
}

pub fn list_sessions(court_data: &Option<CourtData>, reference: &str) -> MarkdownString {
    let Some(court_data) = court_data else {
        return "Leider sind keine Informationen für dieses Gericht verfügbar.".into();
    };

    let reference = ReferenceFilter::new(reference);

    let (count, list) = create_list(&court_data.sessions, |session| {
        reference.matches(&session.reference)
    });

    let full_name = MarkdownString::from_str(&court_data.full_name).bold();
    let mut result = MarkdownString::new();

    match count {
        0 => {
            result += "Leider wurden keine Termine für das ";
            result += &full_name;
            result += ", die zu deinem Filter passen, gefunden.";
        }
        1 => {
            result += "Es wurde 1 Termin für das ";
            result += &full_name;
            result += " gefunden:";
            result += &list;
        }
        _ => {
            result += &format!("Es wurden {count} Termine für das ");
            result += &full_name;
            result += " gefunden:";
            result += &list;
        }
    }

    result
}

pub fn subscribed(name: &str, court_data: &Option<CourtData>, reference: &str) -> MarkdownString {
    let mut result = format!("Dein Abo „{name}” wurde entgegengenommen. ")
        .as_str()
        .into();
    match court_data {
        Some(data) => {
            let reference = ReferenceFilter::new(reference);
            let (count, list) = create_list(&data.sessions, |session| {
                reference.matches(&session.reference)
            });

            match count {
                0 => {
                    result +=
                        "Zur Zeit gibt es nichts zu melden, aber ich halt dich auf dem Laufenden!";
                }
                _ => {
                    result += "Hier schon mal eine Liste der anstehenden Termine:";
                    result += &list;
                    result += "\n\nBei neuen Terminen werde ich dich benachrichtigen!";
                }
            }
        }
        None => {
            result += "Ich kann die Website des Gerichts leider nicht erreichen, aber ich halt dich auf dem Laufenden.";
        }
    }

    result
}

pub fn subscription_exists(name: &str) -> MarkdownString {
    format!("Ein Abo mit dem Namen „{name}” existiert bereits!")
        .as_str()
        .into()
}

fn subscription_entry(s: &Subscription) -> MarkdownString {
    MarkdownString::from_str(&s.name).bold()
        + &format!(
            "\nGericht: {}\nAktenzeichen: {}",
            s.court, s.reference_filter
        )
}

pub fn list_subscriptions(subscriptions: &[Subscription]) -> MarkdownString {
    if subscriptions.is_empty() {
        "Du hast zur Zeit keine Abos am laufen!".into()
    } else {
        let mut result = "Hier ist eine Liste deiner Abos:".into();
        for sub in subscriptions {
            result += "\n\n";
            result += &subscription_entry(sub);
        }
        result
    }
}

pub fn unsubscribed(removed: bool) -> MarkdownString {
    if removed {
        "Abo wurde gelöscht 👍"
    } else {
        "Es wurde kein Abo mit diesem Namen gefunden."
    }
    .into()
}

pub fn sessions_updated(
    old_sessions: &[Session],
    new_sessions: &[Session],
    full_court_name: &str,
    subscription_name: &str,
    reference_filter: &str,
) -> Option<MarkdownString> {
    let reference = ReferenceFilter::new(reference_filter);

    let old_sessions: HashSet<_> = old_sessions.iter().collect();

    let (count, list) = create_list(new_sessions, |session| {
        reference.matches(&session.reference) && !old_sessions.contains(&session)
    });

    if count == 0 {
        return None;
    }

    let mut result = MarkdownString::new();
    result += "🔔 Für dein Abo „";
    result += &MarkdownString::from_str(subscription_name).bold();
    result += &format!("” ({full_court_name}) wurden neue Termine veröffentlicht!");
    result += &list;

    Some(result)
}

pub fn help() -> MarkdownString {
    let help = "
Unterstützte Befehle:
/help
/get_sessions <Gericht> <Datum> <Aktenzeichen>
/subscribe <beliebiger Name> <Gericht> <Aktenzeichen>
/list_subscriptions
/unsubscribe <Name>

Wenn ein Parameter Leerzeichen enthält, muss er in Anführungszeichen gesetzt werden.

Der Name des Gerichts muss sein wie in der URL der Website, also z.B. \"vg-koeln\".

Das Datum kann auch \"*\" sein, um jedes Datum zu erfassen.

Im Aktenzeichen steht \"?\" für ein beliebiges einzelnes Zeichen,  \"*\" für eine beliebige Zeichenkette.

Keine Gewähr für verpasste Termine!";

    help.into()
}

pub fn internal_error() -> MarkdownString {
    "Sorry, ein interner Fehler ist aufgetreten :((".into()
}
