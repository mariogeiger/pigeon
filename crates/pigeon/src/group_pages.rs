//! The web UI's pages on one group's tree and its pending work: browsing
//! folders, one file with its history, the requests with their
//! differences, and the set-aside list.

use std::collections::BTreeSet;
use std::fmt::Write;

use maud::{Markup, html};
use serde_json::Value;

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
fn file_link(group: &str, path: &str) -> String {
    format!("/g/{group}/file?path={}", encode(path))
}

fn fill<'a>(
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

/// The folder `under`: its subfolders, its files, and the forms that act
/// on it.
#[must_use]
pub fn files(group: &str, under: &str, files: &Value) -> Markup {
    let prefix = if under.is_empty() {
        String::new()
    } else {
        format!("{under}/")
    };
    let mut folders = BTreeSet::new();
    let mut here = Vec::new();
    for item in files.as_array().map(Vec::as_slice).unwrap_or_default() {
        let path = item["path"].as_str().unwrap_or_default();
        match path.strip_prefix(&prefix).unwrap_or(path).split_once('/') {
            Some((folder, _)) => {
                folders.insert(folder.to_owned());
            }
            None => here.push(item.clone()),
        }
    }
    let back = format!("/g/{group}/files?under={}", encode(under));
    let pattern = format!("/{prefix}");
    let names: Vec<&str> = under.split('/').filter(|name| !name.is_empty()).collect();
    let folder_link = |path: &str| format!("/g/{group}/files?under={}", encode(path));
    let body = html! {
        p {
            a href=(folder_link("")) { (group) }
            @for (index, name) in names.iter().enumerate() {
                " / " a href=(folder_link(&names[..=index].join("/"))) { (name) }
            }
        }
        ul {
            @for folder in &folders {
                li { a href=(folder_link(&format!("{prefix}{folder}"))) { (folder) "/" } }
            }
        }
        (table(action("file", "list"), &Value::Array(here), &|item| {
            item["path"].as_str().map(|path| file_link(group, path))
        }))
        (form(action("file", "write"), &back, fill(group, &[], &[("path", &prefix)])))
        @if !under.is_empty() {
            h2 { "This folder" }
            @for verb in ["follow", "download", "unfollow"] {
                (form(action("selection", verb), &back, fill(group, &[("pattern", &pattern)], &[])))
            }
            (form(action("file", "rename"), &back, fill(group, &[("from", under)], &[("to", under)])))
            (form(action("file", "delete"), &back, fill(group, &[("path", under)], &[])))
        }
    };
    layout("Files", Some(group), &body)
}

/// One file: how this machine holds it, its versions, and what can be done
/// to it.
#[must_use]
pub fn file(group: &str, path: &str, file: &Value, history: &Value) -> Markup {
    let back = file_link(group, path);
    let pattern = format!("/{path}");
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
    use serde_json::json;

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode("@alice/a b&c.txt"), "@alice/a%20b%26c.txt");
        assert_eq!(encode("é"), "%C3%A9");
    }

    #[test]
    fn a_folder_shows_its_subfolders_and_its_own_files() {
        let list = json!([
            {"path": "docs/a.txt", "owner": "alice", "content": {"size": 1}},
            {"path": "docs/sub/b.txt", "owner": "bob", "content": {"size": 2}},
        ]);
        let page = files("cheapmo", "docs", &list).into_string();
        assert!(page.contains(r#"<a href="/g/cheapmo/files?under=docs/sub">sub/</a>"#));
        assert!(page.contains(r#"<a href="/g/cheapmo/file?path=docs/a.txt">docs/a.txt</a>"#));
        assert!(!page.contains("docs/sub/b.txt"));
        assert!(page.contains(r#"<input type="hidden" name="pattern" value="/docs/">"#));
    }
}
