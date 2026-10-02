//! The web UI's page of one file, with the changes suggested to it and the
//! difference each makes, its history back through the paths it moved
//! from, each version with the button that restores it, and what can be
//! done to it.

use maud::{Markup, html};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::exact_pattern;
use serde_json::Value;

use crate::file_status::{Status, suggestion_title};
use crate::files_page::waiting_note;
use crate::form::form;
use crate::pages::{Bar, action, deciding, encode, fields, file_link, fill, layout, table};

/// What the page of one file shows: its current version, its history, the
/// edit of it waiting here, and the changes of it suggested, each with its
/// suggestion and the difference it makes.
pub struct Shown<'a> {
    pub file: &'a Value,
    pub history: &'a Value,
    pub waiting: &'a Value,
    pub suggested: &'a [(Value, Value, Markup)],
}

/// The form that restores the file at `pattern` to the version `item` of
/// its history, asking first.
fn restore_form(group: &str, back: &str, pattern: &str, item: &Value) -> Markup {
    let time = item["time"].as_str().unwrap_or_default();
    html! {
        div data-confirm={ "Restore the version of " (time) "? It publishes it again as a new version, for the whole group." } {
            (form(action("file", "restore"), back, fill(group, &[("pattern", pattern), ("time", time)], &[])))
        }
    }
}

/// One file: how this machine holds it, the changes suggested to it, its
/// edit waiting to be published, its versions, and what can be done to it.
#[must_use]
pub fn file(bar: &Bar<'_>, path: &str, shown: &Shown) -> Markup {
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
    let restore = |item: &Value| restore_form(group, &back, &pattern, item);
    let body = html! {
        @if shown.file.is_null() {
            p { "No current version: the file was deleted, or never published." }
        } @else {
            (fields(shown.file))
            @if let Some(address) = raw(shown.file) {
                p { a href=(address) { "Download the current version" } }
            }
        }
        @if !shown.suggested.is_empty() {
            h2 { (Status::Suggested.emoji()) " Suggested" }
            ul class="changes" {
                @for (suggestion, change, diff) in shown.suggested {
                    li {
                        div class="line" {
                            span class="what" { (suggestion_title(suggestion, change)) }
                            (deciding(group, &back, &[suggestion["id"].as_str().unwrap_or_default()], (suggestion["placeable"] == true).then_some(path)))
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
        (table(action("file", "history"), shown.history, &raw, (!pattern.is_empty()).then_some(&restore)))
        h2 { "Actions" }
        (form(action("file", "write"), &back, fill(group, &[("path", path)], &[])))
        (form(action("file", "rename"), &back, fill(group, &[("from", path)], &[("to", path)])))
        (form(action("file", "delete"), &back, fill(group, &[("path", path)], &[])))
        @for verb in ["follow", "pin", "free"] {
            (form(action("selection", verb), &back, fill(group, &[("pattern", &pattern)], &[("time", "now")])))
        }
    };
    layout(path, Some(bar), &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_shows_its_suggestions_and_restores_each_version() {
        let bar = Bar {
            group: "cheapmo",
            tab: None,
            suggestions: 1,
        };
        let history = serde_json::json!([
            {"time": "2026-01-01T00:00:00Z", "path": "a.txt", "stamp": {"time": 1}, "content": {"size": 1}, "author": "bob"},
            {"time": "2026-01-02T00:00:00Z", "path": "b.txt", "stamp": {"time": 2}, "content": {"size": 2}, "author": "papy"},
        ]);
        let suggestion = serde_json::json!({"id": "b.txt@1-m", "author": "papy", "reason": "the rules leave it to the group", "placeable": true,
            "changes": [{"path": "b.txt", "what": "a new file", "content": {"size": 3}, "outdated": false}]});
        let change = suggestion["changes"][0].clone();
        let suggested = [(suggestion, change, html! {})];
        let shown = Shown {
            file: &history[1],
            history: &history,
            waiting: &Value::Null,
            suggested: &suggested,
        };
        let page = file(&bar, "b.txt", &shown).into_string();
        assert!(page.contains("papy suggests a new file"), "{page}");
        assert_eq!(
            page.matches(r#"name="suggestions" value="b.txt@1-m""#)
                .count(),
            3,
            "{page}"
        );
        assert!(page.contains(r#"name="to" value="b.txt">"#), "{page}");
        assert_eq!(
            page.matches(r#"action="/act/file/restore""#).count(),
            2,
            "{page}"
        );
        assert!(
            page.contains(r#"name="time" value="2026-01-01T00:00:00Z""#),
            "{page}"
        );
        assert!(page.contains(r#"name="pattern" value="/b.txt""#), "{page}");
    }
}
