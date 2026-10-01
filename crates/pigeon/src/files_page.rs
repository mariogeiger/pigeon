//! The web UI's Files page: one folder's subfolders and files, each with a
//! box that follows it, checked when every file under it is followed and
//! mixed when only some are, and the edits waiting there, with the time
//! left before they are published and the buttons that publish them now.

use std::collections::BTreeMap;

use maud::{Markup, html};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{exact_pattern, folder_pattern};
use serde_json::Value;

use crate::form::form;
use crate::group_pages::{encode, file_link, fill};
use crate::pages::{action, layout};
use crate::render::{cell, field, header};

/// How many of the files under a box are followed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Followed {
    All,
    Some,
    None,
}

impl Followed {
    fn of(followed: impl IntoIterator<Item = bool>) -> Self {
        let (mut any, mut all) = (false, true);
        for one in followed {
            any |= one;
            all &= one;
        }
        match (any, all) {
            (false, _) => Self::None,
            (true, true) => Self::All,
            (true, false) => Self::Some,
        }
    }

    fn state(self) -> &'static str {
        match self {
            Self::All => "checked",
            Self::Some => "mixed",
            Self::None => "unchecked",
        }
    }
}

/// Whether the selection follows the file `item` describes.
fn follows(item: &Value) -> bool {
    item["cutoff"] == "PlusInfinity"
}

/// Seconds as minutes and seconds.
fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// One name of the folder: a published file, a waiting edit, or both.
#[derive(Default)]
struct Entry<'a> {
    file: Option<&'a Value>,
    waiting: Option<&'a Value>,
}

/// A subfolder: whether each file under it is followed, and how many
/// edits wait in it.
#[derive(Default)]
struct Subfolder {
    followed: Vec<bool>,
    waiting: usize,
}

/// The folder `prefix` names, sorted into its subfolders and its own files.
struct Folder<'a> {
    subfolders: BTreeMap<&'a str, Subfolder>,
    entries: BTreeMap<&'a str, Entry<'a>>,
}

impl<'a> Folder<'a> {
    fn new(prefix: &str, files: &'a [Value], waiting: &'a [Value]) -> Self {
        let mut folder = Self {
            subfolders: BTreeMap::new(),
            entries: BTreeMap::new(),
        };
        for (item, is_waiting) in files
            .iter()
            .map(|item| (item, false))
            .chain(waiting.iter().map(|item| (item, true)))
        {
            let path = item["path"].as_str().unwrap_or_default();
            let rest = path.strip_prefix(prefix).unwrap_or(path);
            if let Some((name, _)) = rest.split_once('/') {
                let subfolder = folder.subfolders.entry(name).or_default();
                subfolder.followed.push(follows(item));
                subfolder.waiting += usize::from(is_waiting);
            } else {
                let entry = folder.entries.entry(path).or_default();
                if is_waiting {
                    entry.waiting = Some(item);
                } else {
                    entry.file = Some(item);
                }
            }
        }
        folder
    }
}

/// The box that follows `path`, when it is a valid path.
fn follow_box(path: &str, pattern: fn(&GroupPath) -> String, followed: Followed) -> Markup {
    let Ok(path) = GroupPath::parse(path) else {
        return html! {};
    };
    html! {
        input type="checkbox" title="Follow" data-pattern=(pattern(&path))
            data-state=(followed.state()) checked[followed == Followed::All];
    }
}

/// The button that publishes the edits waiting at `path` now, asking
/// first when publishing freezes them.
fn publish_button(group: &str, back: &str, path: &str, freezes: bool) -> Markup {
    let question = format!(
        "Publishing freezes what waits in a drop folder at {}: it then changes only through requests. Publish now?",
        if path.is_empty() { group } else { path }
    );
    html! {
        div data-confirm=[freezes.then_some(question)] {
            (form(action("file", "publish"), back, fill(group, &[("path", path)], &[])))
        }
    }
}

/// What a file's last cell says: a frozen copy, and its waiting edit.
fn file_state(group: &str, back: &str, path: &str, entry: &Entry) -> Markup {
    let frozen = entry.file.is_some_and(|file| file["cutoff"]["At"].is_u64());
    html! {
        @if frozen { span class="mark" { "frozen copy" } " " }
        @if let Some(waiting) = entry.waiting { (waiting_note(group, back, path, waiting)) }
    }
}

/// The edit waiting at `path`: the time left before it is published, and
/// the button that publishes it now.
pub fn waiting_note(group: &str, back: &str, path: &str, waiting: &Value) -> Markup {
    let seconds = waiting["due_in"].as_u64().unwrap_or_default();
    html! {
        @if waiting["deleted"] == true { "deletion " }
        "waiting · published in " span data-due=(seconds) { (clock(seconds)) }
        (publish_button(group, back, path, waiting["freezes"] == true))
    }
}

/// The question asked when a box is unchecked.
fn unfollow_dialog() -> Markup {
    html! {
        dialog id="unfollow" {
            form method="dialog" {
                p { "Stop following: keep the current copy here, frozen, or free the space?" }
                button value="keep" { "Keep the copy" }
                " " button value="free" { "Free the space" }
                " " button value="" { "Cancel" }
            }
        }
    }
}

/// The folder `under`: its subfolders and files with their follow boxes,
/// the edits waiting in it, and the forms that act on it.
#[must_use]
pub fn files(group: &str, under: &str, files: &Value, waiting: &Value) -> Markup {
    let prefix = if under.is_empty() {
        String::new()
    } else {
        format!("{under}/")
    };
    let files = files.as_array().map(Vec::as_slice).unwrap_or_default();
    let waiting = waiting.as_array().map(Vec::as_slice).unwrap_or_default();
    let folder = Folder::new(&prefix, files, waiting);
    let back = format!("/g/{group}/files?under={}", encode(under));
    let names: Vec<&str> = under.split('/').filter(|name| !name.is_empty()).collect();
    let folder_link = |path: &str| format!("/g/{group}/files?under={}", encode(path));
    let columns = action("file", "list").columns;
    let freezes = waiting.iter().any(|item| item["freezes"] == true);
    let body = html! {
        p {
            a href=(folder_link("")) { (group) }
            @for (index, name) in names.iter().enumerate() {
                " / " a href=(folder_link(&names[..=index].join("/"))) { (name) }
            }
        }
        @if waiting.len() > 1 {
            p { (waiting.len()) " edits wait to be published here."
                (publish_button(group, &back, under, freezes)) }
        }
        @if folder.subfolders.is_empty() && folder.entries.is_empty() {
            p { "Nothing yet." }
        } @else {
            table {
                tr { th {} @for column in columns { th { (header(column)) } } th {} }
                @for (name, subfolder) in &folder.subfolders {
                    @let path = format!("{prefix}{name}");
                    tr {
                        td { (follow_box(&path, folder_pattern, Followed::of(subfolder.followed.iter().copied()))) }
                        td { a href=(folder_link(&path)) { (name) "/" } }
                        @for _ in &columns[1..] { td {} }
                        td { @if subfolder.waiting > 0 { (subfolder.waiting) " waiting" } }
                    }
                }
                @for (path, entry) in &folder.entries {
                    @let item = entry.file.or(entry.waiting).unwrap_or(&Value::Null);
                    tr {
                        td { (follow_box(path, exact_pattern, Followed::of([follows(item)]))) }
                        td {
                            @if entry.file.is_some() { a href=(file_link(group, path)) { (path) } }
                            @else { (path) }
                        }
                        @for column in &columns[1..] {
                            td { @if let Some(file) = entry.file { (cell(column, field(file, column))) } }
                        }
                        td { (file_state(group, &back, path, entry)) }
                    }
                }
            }
        }
        (unfollow_dialog())
        (form(action("file", "write"), &back, fill(group, &[], &[("path", &prefix)])))
        @if let Ok(path) = GroupPath::parse(under) {
            @let pattern = folder_pattern(&path);
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(path: &str, cutoff: &Value) -> Value {
        json!({"path": path, "owner": "alice", "content": {"size": 1}, "cutoff": cutoff})
    }

    #[test]
    fn a_box_is_checked_mixed_or_unchecked_as_the_files_under_it_are_followed() {
        let list = json!([
            file("docs/a.txt", &json!("PlusInfinity")),
            file("docs/all/b.txt", &json!("PlusInfinity")),
            file("docs/some/c.txt", &json!("PlusInfinity")),
            file("docs/some/d.txt", &json!({"At": 5})),
            file("docs/none/e.txt", &json!("MinusInfinity")),
            file("docs/[x].txt", &json!({"At": 5})),
        ]);
        let page = files("cheapmo", "docs", &list, &json!([])).into_string();
        let row = |name: &str| {
            let at = page.find(name).unwrap();
            page[page[..at].rfind("<tr>").unwrap()..at].to_owned()
        };
        assert!(
            row(">all/<").contains(r#"data-pattern="/docs/all/" data-state="checked" checked"#)
        );
        assert!(row(">some/<").contains(r#"data-state="mixed">"#));
        assert!(row(">none/<").contains(r#"data-state="unchecked">"#));
        assert!(
            row(">docs/a.txt<")
                .contains(r#"data-pattern="/docs/a.txt" data-state="checked" checked"#)
        );
        assert!(page.contains(r#"data-pattern="/docs/\[x\].txt" data-state="unchecked">"#));
        assert!(page.contains("frozen copy"));
        assert!(!page.contains("docs/all/b.txt"));
        assert!(page.contains(r#"<input type="hidden" name="pattern" value="/docs/">"#));
        assert!(page.contains(r#"<dialog id="unfollow">"#));
    }

    #[test]
    fn waiting_edits_show_in_their_folder_with_the_time_left_and_a_button() {
        let list = json!([file("@alice/a.txt", &json!("PlusInfinity"))]);
        let waiting = json!([
            {"path": "@alice/a.txt", "due_in": 2, "freezes": false, "deleted": false, "cutoff": "PlusInfinity"},
            {"path": "@alice/new/b.txt", "due_in": 3, "freezes": false, "deleted": false, "cutoff": "PlusInfinity"},
            {"path": "@alice/c.txt", "due_in": 192, "freezes": true, "deleted": false, "cutoff": "MinusInfinity"},
        ]);
        let page = files("cheapmo", "@alice", &list, &waiting).into_string();
        assert!(page.contains(r#"waiting · published in <span data-due="2">0:02</span>"#));
        assert!(page.contains(r#"<span data-due="192">3:12</span>"#));
        assert!(page.contains(">new/</a>") && page.contains("1 waiting"));
        assert!(page.contains("3 edits wait to be published here."));
        assert!(page.contains(r#"name="path" value="@alice/c.txt""#));
        assert_eq!(page.matches("data-confirm=").count(), 2, "{page}");
        assert!(page.contains("<td>@alice/c.txt</td>"));
        assert!(page.contains(r#"data-pattern="/@alice/c.txt" data-state="unchecked""#));
    }
}
