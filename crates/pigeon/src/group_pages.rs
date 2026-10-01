//! The web UI's page of one file, with the changes waiting at it and the
//! difference each makes, its history and what can be done to it, and the
//! addresses and form fillings the group's pages share.

use std::fmt::Write;

use maud::{Markup, html};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::exact_pattern;
use serde_json::Value;

use crate::file_status::{Status, change_title};
use crate::files_page::waiting_note;
use crate::form::{Fill, form};
use crate::pages::{Bar, action, fields, layout, table};

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

/// What the page of one file shows: its current version, its history, the
/// edit of it waiting here, and the changes waiting at it, each with its
/// difference.
pub struct Shown<'a> {
    pub file: &'a Value,
    pub history: &'a Value,
    pub waiting: &'a Value,
    pub changes: &'a [(Value, Markup)],
}

/// The forms that resolve a waiting change at `back`'s file, for the
/// member `me`.
fn change_forms(group: &str, back: &str, change: &Value, me: &str) -> Markup {
    let entry = change["entry"].as_str().unwrap_or_default();
    let path = change["path"].as_str().unwrap_or_default();
    let fixed = [("entry", entry)];
    let aside = change["waits"].is_object();
    let asks = aside && change["owner"] != me;
    html! {
        @if change["waits"] != "Applying" {
            (form(action("change", "apply"), back, fill(group, &fixed, &[])))
            @if asks { (form(action("change", "ask"), back, fill(group, &fixed, &[]))) }
            (form(action("change", "place"), back, fill(group, &fixed, &[("to", path)])))
            (form(action("change", "discard"), back, fill(group, &fixed, &[])))
        }
    }
}

/// One file: how this machine holds it, the changes waiting at it, its
/// edit waiting to be published, its versions, and what can be done to it,
/// for the member `me`.
#[must_use]
pub fn file(bar: &Bar<'_>, me: &str, path: &str, shown: &Shown) -> Markup {
    let group = bar.group;
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
        @if shown.file.is_null() {
            p { "No current version: the file was deleted, or never published." }
        } @else {
            (fields(shown.file))
            @if let Some(address) = raw(shown.file) {
                p { a href=(address) { "Download the current version" } }
            }
        }
        @if !shown.changes.is_empty() {
            h2 { (Status::Change.emoji()) " Waiting" }
            ul class="changes" {
                @for (change, diff) in shown.changes {
                    li {
                        div class="line" {
                            span class="what" { (change_title(change, me)) }
                            (change_forms(group, &back, change, me))
                        }
                        details class="diff" open {
                            summary { "Difference with the current version" }
                            (diff)
                        }
                    }
                }
            }
        }
        @if shown.waiting.is_object() {
            p { (waiting_note(group, &back, path, shown.waiting)) }
        }
        h2 { "History" }
        (table(action("file", "history"), shown.history, &raw))
        h2 { "Actions" }
        (form(action("file", "write"), &back, fill(group, &[("path", path)], &[])))
        (form(action("file", "rename"), &back, fill(group, &[("from", path)], &[("to", path)])))
        (form(action("file", "delete"), &back, fill(group, &[("path", path)], &[])))
        @for verb in ["follow", "download", "unfollow", "pin"] {
            (form(action("selection", verb), &back, fill(group, &[("pattern", &pattern)], &[])))
        }
    };
    layout(path, Some(bar), &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode("+alice/a b&c.txt"), "%2Balice/a%20b%26c.txt");
        assert_eq!(encode("é"), "%C3%A9");
    }
}
