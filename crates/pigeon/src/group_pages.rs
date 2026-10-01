//! The web UI's pages on one group's files and its pending work: one file
//! with its history, the requests with their differences, and the
//! set-aside list.

use std::fmt::Write;

use maud::{Markup, html};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::exact_pattern;
use serde_json::Value;

use crate::files_page::waiting_note;
use crate::form::{Fill, form};
use crate::pages::{action, fields, layout, table};
use crate::render::cell;

/// `text` as a URL query value.
#[must_use]
pub fn encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/@".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// A file's address in the web UI.
pub fn file_link(group: &str, path: &str) -> String {
    format!("/g/{group}/file?path={}", encode(path))
}

pub fn fill<'a>(
    group: &'a str,
    fixed: &'a [(&'a str, &'a str)],
    defaults: &'a [(&'a str, &'a str)],
) -> Fill<'a> {
    Fill {
        group: Some(group),
        fixed,
        defaults,
    }
}

/// One file: how this machine holds it, its edit waiting to be
/// published, its versions, and what can be done
/// to it.
#[must_use]
pub fn file(group: &str, path: &str, file: &Value, history: &Value, waiting: &Value) -> Markup {
    let back = file_link(group, path);
    let pattern = GroupPath::parse(path)
        .map(|path| exact_pattern(&path))
        .unwrap_or_default();
    let raw = |item: &Value| {
        let time = item["stamp"]["time"].as_u64()?;
        item["content"]
            .is_object()
            .then(|| format!("/g/{group}/raw?path={}&time={time}", encode(path)))
    };
    let body = html! {
        @if file.is_null() {
            p { "No current version: the file was deleted, or never published." }
        } @else {
            (fields(file))
            @if let Some(address) = raw(file) {
                p { a href=(address) { "Download the current version" } }
            }
        }
        @if waiting.is_object() {
            p { (waiting_note(group, &back, path, waiting)) }
        }
        h2 { "History" }
        (table(action("file", "history"), history, &raw))
        h2 { "Actions" }
        (form(action("file", "write"), &back, fill(group, &[("path", path)], &[])))
        (form(action("file", "rename"), &back, fill(group, &[("from", path)], &[("to", path)])))
        (form(action("file", "delete"), &back, fill(group, &[("path", path)], &[])))
        @for verb in ["follow", "download", "unfollow", "pin"] {
            (form(action("selection", verb), &back, fill(group, &[("pattern", &pattern)], &[])))
        }
    };
    layout(path, Some(group), &body)
}

/// One request and the difference each of its changes makes.
pub struct RequestCard {
    pub request: Value,
    pub changes: Vec<(String, Markup)>,
    /// Whether this member is the one to accept or refuse it.
    pub decides: bool,
}

/// Every request, newest first, with the forms to answer the ones
/// addressed to this member.
#[must_use]
pub fn requests(group: &str, cards: &[RequestCard]) -> Markup {
    let back = format!("/g/{group}/requests");
    let body = html! {
        @if cards.is_empty() { p { "No requests." } }
        @for card in cards.iter().rev() {
            @let request = &card.request;
            @let statement = &request["statement"];
            section {
                h2 {
                    (cell("", &statement["mode"])) " by " (cell("", &request["author"]))
                    " to " (cell("", &statement["owner"]))
                }
                p { (cell("", &request["time"])) " · " (cell("", &request["path"])) }
                @if let Some(message) = statement["message"].as_str().filter(|m| !m.is_empty()) {
                    blockquote { (message) }
                }
                @if request["outdated"] == true {
                    p class="mark" { "Based on an old version." }
                }
                @if request["applied"] == true {
                    p { "Applied." }
                } @else if let Some(decision) = request["decision"].as_str() {
                    p { "Decision: " (decision) }
                }
                @for (path, diff) in &card.changes {
                    h3 { a href=(file_link(group, path)) { (path) } }
                    (diff)
                }
                @if card.decides {
                    @let path = request["path"].as_str().unwrap_or_default();
                    (form(action("request", "accept"), &back, fill(group, &[("request", path)], &[])))
                    (form(action("request", "refuse"), &back, fill(group, &[("request", path)], &[])))
                }
            }
        }
    };
    layout("Requests", Some(group), &body)
}

/// Every set-aside item, with what it would change and the forms that
/// resolve it.
#[must_use]
pub fn aside(group: &str, items: &[(Value, Markup)]) -> Markup {
    let back = format!("/g/{group}/aside");
    let body = html! {
        @if items.is_empty() { p { "Nothing set aside." } }
        @for (item, diff) in items {
            @let id = cell("", &item["id"]);
            @let path = item["path"].as_str().unwrap_or_default();
            section {
                h2 { (path) }
                p { (cell("", &item["reason"])) " · " (cell("", &item["time"])) }
                (diff)
                (form(action("aside", "request"), &back, fill(group, &[("id", &id)], &[])))
                (form(action("aside", "restore"), &back, fill(group, &[("id", &id)], &[("to", path)])))
                (form(action("aside", "discard"), &back, fill(group, &[("id", &id)], &[])))
            }
        }
    };
    layout("Set aside", Some(group), &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode("_alice/a b&c.txt"), "_alice/a%20b%26c.txt");
        assert_eq!(encode("é"), "%C3%A9");
    }
}
