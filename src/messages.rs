mod markdown_string;

use std::collections::HashSet;

use regex::Regex;

pub use self::markdown_string::MarkdownString;
use crate::database::Subscription;
use crate::scraper::{CourtData, Session};

struct ReferenceFilter {
    regex: Regex,
}

struct Paginator {
    pages: Vec<Vec<MarkdownString>>,
    current_page: Vec<MarkdownString>,
    current_page_len: usize,

    page_nr_max_len: usize,
    item_limit: usize,
    char_limit: usize,
    join: MarkdownString,
}

impl Paginator {
    fn page_nr(k: usize, n: usize) -> MarkdownString {
        format!("[Nachricht {}/{}]", k, n).as_str().into()
    }

    fn new(item_limit: usize, char_limit: usize, join: MarkdownString) -> Self {
        let page_nr_max_len = Self::page_nr(999, 999).len_parsed();
        Self {
            pages: vec![],
            current_page: vec![],
            current_page_len: 0,
            page_nr_max_len,
            item_limit,
            char_limit,
            join,
        }
    }

    fn initial_page_len(&self) -> usize {
        self.page_nr_max_len
    }

    fn current_page_len_with(&self, new_item: &MarkdownString) -> usize {
        self.current_page_len + self.join.len_parsed() + new_item.len_parsed()
    }

    fn try_push_to_current_page(&mut self, item: MarkdownString) -> Result<(), MarkdownString> {
        let new_page_len = self.current_page_len_with(&item);
        if self.current_page.len() < self.item_limit && new_page_len < self.char_limit {
            self.current_page.push(item);
            self.current_page_len = new_page_len;
            Ok(())
        } else {
            Err(item)
        }
    }

    // Returns false if the item is too long
    fn push(&mut self, item: MarkdownString) -> Result<(), MarkdownString> {
        let item = match self.try_push_to_current_page(item) {
            Ok(()) => return Ok(()),
            Err(item) => item,
        };

        // create new page
        self.pages.push(std::mem::take(&mut self.current_page));
        self.current_page_len = self.initial_page_len();

        self.try_push_to_current_page(item)
    }

    fn get_pages(self) -> impl Iterator<Item = MarkdownString> {
        let mut pages = self.pages;
        pages.push(self.current_page);
        let n_pages = pages.len();
        pages.into_iter().enumerate().map(move |(k, page)| {
            let mut content = MarkdownString::join(&page, &self.join);
            if n_pages > 1 {
                content += &self.join;
                content += &Self::page_nr(k + 1, n_pages)
            }
            content
        })
    }
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

pub fn invalid_date() -> MarkdownString {
    "Das angegebene Datum ist ungültig.".into()
}

fn list_sessions_prefix(court_data: &CourtData, num_items: usize) -> MarkdownString {
    let full_name = MarkdownString::from_str(&court_data.full_name).bold();
    let mut prefix = MarkdownString::new();
    match num_items {
        0 => {
            prefix += "Leider wurden keine Termine für das ";
            prefix += &full_name;
            prefix += ", die zu deinem Filter passen, gefunden.";
        }
        1 => {
            prefix += "Es wurde 1 Termin für das ";
            prefix += &full_name;
            prefix += " gefunden:";
        }
        count => {
            prefix += &format!("Es wurden {count} Termine für das ");
            prefix += &full_name;
            prefix += " gefunden:";
        }
    };
    prefix
}

pub fn list_sessions(court_data: &Option<CourtData>, reference: &str) -> Vec<MarkdownString> {
    let Some(court_data) = court_data else {
        return vec!["Leider sind keine Informationen für dieses Gericht verfügbar.".into()];
    };

    let reference = ReferenceFilter::new(reference);

    let items: Vec<_> = court_data
        .sessions
        .iter()
        .filter(|x| reference.matches(&x.reference))
        .map(session_info)
        .collect();

    let mut pages = Paginator::new(20, 4096, "\n\n".into());

    let prefix = list_sessions_prefix(court_data, items.len());
    pages.push(prefix).unwrap();

    for item in items {
        pages
            .push(item)
            .unwrap_or_else(|_| pages.push("[Eintrag zu lang]".into()).unwrap());
    }

    pages.get_pages().collect()
}

pub fn subscribed(
    name: &str,
    court_data: &Option<CourtData>,
    reference: &str,
) -> Vec<MarkdownString> {
    let mut result = "Dein Abo „".into();
    result += &MarkdownString::from_str(name).bold();
    result += "” wurde entgegengenommen. ";

    match court_data {
        Some(data) => {
            let reference = ReferenceFilter::new(reference);
            let items: Vec<_> = data
                .sessions
                .iter()
                .filter(|x| reference.matches(&x.reference))
                .map(session_info)
                .collect();

            match items.len() {
                0 => {
                    result +=
                        "Zur Zeit gibt es nichts zu melden, aber ich halt dich auf dem Laufenden!";
                }
                _ => {
                    result += "Hier schon mal eine Liste der anstehenden Termine:";

                    let mut pages = Paginator::new(20, 4096, "\n\n".into());

                    pages.push(result).unwrap();

                    for item in items {
                        pages
                            .push(item)
                            .unwrap_or_else(|_| pages.push("[Eintrag zu lang]".into()).unwrap());
                    }
                    pages
                        .push("Bei neuen Terminen werde ich dich benachrichtigen!".into())
                        .unwrap();

                    return pages.get_pages().collect();
                }
            }
        }
        None => {
            result += "Ich kann die Website des Gerichts leider nicht erreichen, aber ich halt dich auf dem Laufenden.";
        }
    }

    vec![result]
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

pub fn list_subscriptions(subscriptions: &[Subscription]) -> Vec<MarkdownString> {
    if subscriptions.is_empty() {
        vec!["Du hast zur Zeit keine Abos am laufen!".into()]
    } else {
        let mut pages = Paginator::new(20, 4096, "\n\n".into());
        pages
            .push("Hier ist eine Liste deiner Abos:".into())
            .unwrap();
        for sub in subscriptions {
            pages
                .push(subscription_entry(sub))
                .unwrap_or_else(|_| pages.push("[Eintrag zu lang]".into()).unwrap());
        }
        pages.get_pages().collect()
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
) -> Vec<MarkdownString> {
    let reference = ReferenceFilter::new(reference_filter);
    let old_sessions: HashSet<_> = old_sessions.iter().collect();

    let items: Vec<_> = new_sessions
        .iter()
        .filter(|session| reference.matches(&session.reference) && !old_sessions.contains(session))
        .map(session_info)
        .collect();

    if items.len() == 0 {
        return vec![];
    }

    let mut prefix = MarkdownString::new();
    prefix += "🔔 Zu deinem Abo „";
    prefix += &MarkdownString::from_str(subscription_name).bold();
    if items.len() == 1 {
        prefix += &format!("” ({full_court_name}) wurde ein neuer Termin veröffentlicht!");
    } else {
        prefix += &format!(
            "” ({full_court_name}) wurden {} neue Termine veröffentlicht!",
            items.len()
        );
    }

    let mut pages = Paginator::new(20, 4096, "\n\n".into());

    pages.push(prefix).unwrap();
    for item in items {
        pages
            .push(item)
            .unwrap_or_else(|_| pages.push("[Eintrag zu lang]".into()).unwrap());
    }
    return pages.get_pages().collect();
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
