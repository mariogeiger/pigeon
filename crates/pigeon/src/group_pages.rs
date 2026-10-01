//! The web UI's page of one file, with its history and what can be done to
//! it, and the addresses and form fillings the group's pages share.

use std::fmt::Write;

use maud::{Markup, html};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::exact_pattern;
use serde_json::Value;

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

/// One file: how this machine holds it, its edit waiting to be
/// published, its versions, and what can be done
/// to it.
#[must_use]
pub fn file(bar: &Bar<'_>, path: &str, file: &Value, history: &Value, waiting: &Value) -> Markup {
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
