//! The web UI's Files page: the whole group as one tree, whose folders
//! `files.js` opens and closes in place. Each row has a box that follows
//! it, checked when every file under it is followed and mixed when only
//! some are, its size, owner, time and statuses, one emoji each, which a
//! legend below the tree explains, and a menu of what can be done to it.
//! The drafts other machines announce show greyed with their author.

use std::collections::{BTreeMap, BTreeSet};

use maud::{Markup, html};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{exact_pattern, folder_pattern};
use serde_json::Value;

use crate::file_status::{countdown, file_status, folder_status, legend};
use crate::file_tree::{self, Facts, Folder, Followed, Leaf, Row};
use crate::form::form;
use crate::group_pages::{encode, file_link, fill};
use crate::pages::{action, layout};
use crate::render::size;

/// The value of a box's `data-state`.
fn state(followed: Followed) -> &'static str {
    match followed {
        Followed::All => "checked",
        Followed::Some => "mixed",
        Followed::None => "unchecked",
    }
}

/// Whether the selection follows the file `item` describes.
fn follows(item: &Value) -> bool {
    item["cutoff"] == "PlusInfinity"
}

/// One path of the group: its published file, the edit of it waiting
/// here, and the drafts of it other machines announced.
struct Entry<'a> {
    path: &'a str,
    file: Option<&'a Value>,
    waiting: Option<&'a Value>,
    drafts: Vec<&'a Value>,
}

impl Leaf for Entry<'_> {
    fn path(&self) -> &str {
        self.path
    }

    fn facts(&self) -> Facts<'_> {
        let size = match (self.file, self.waiting, self.drafts.first().copied()) {
            (Some(file), _, _) => &file["content"]["size"],
            (None, Some(item), _) | (None, None, Some(item)) => &item["size"],
            (None, None, None) => &Value::Null,
        };
        Facts {
            size: size.as_u64().unwrap_or_default(),
            followed: self.file.or(self.waiting).map(follows),
            time: self.file.and_then(|file| file["time"].as_str()),
            waiting: self.waiting.is_some(),
        }
    }
}

/// The files and the waiting edits, one entry per path.
fn entries<'a>(files: &'a [Value], waiting: &'a [Value]) -> Vec<Entry<'a>> {
    let mut entries: BTreeMap<&str, Entry> = BTreeMap::new();
    for item in files.iter().chain(waiting) {
        let path = item["path"].as_str().unwrap_or_default();
        let entry = entries.entry(path).or_insert(Entry {
            path,
            file: None,
            waiting: None,
            drafts: Vec::new(),
        });
        if item.get("here").is_none() {
            entry.file = Some(item);
        } else if item["here"] == true {
            entry.waiting = Some(item);
        } else {
            entry.drafts.push(item);
        }
    }
    entries.into_values().collect()
}

/// The box that follows `path`, when it is a valid path.
fn follow_box(path: &str, pattern: fn(&GroupPath) -> String, followed: Option<Followed>) -> Markup {
    let (Ok(path), Some(followed)) = (GroupPath::parse(path), followed) else {
        return html! {};
    };
    html! {
        input type="checkbox" title="Follow" data-pattern=(pattern(&path))
            data-state=(state(followed)) checked[followed == Followed::All];
    }
}

/// The button that publishes the edits waiting at `path` now, asking
/// first when publishing freezes them.
fn publish_button(group: &str, back: &str, path: &str, freezes: bool) -> Markup {
    html! {
        div data-confirm=[freezes.then(|| freeze_question(group, path))] {
            (form(action("file", "publish"), back, fill(group, &[("path", path)], &[])))
        }
    }
}

/// What publishing asks first when it freezes what waits at `path`.
fn freeze_question(group: &str, path: &str) -> String {
    format!(
        "Publishing freezes what waits in a drop folder at {}: it then changes only through requests. Publish now?",
        if path.is_empty() { group } else { path }
    )
}

/// The edit waiting at `path`: the time left before it is published, and
/// the button that publishes it now.
pub fn waiting_note(group: &str, back: &str, path: &str, waiting: &Value) -> Markup {
    html! {
        @if waiting["deleted"] == true { "deletion " }
        "waiting · published in " (countdown(&waiting["due_in"]))
        (publish_button(group, back, path, waiting["freezes"] == true))
    }
}

/// The folders open before one opens or closes any: those above `under`
/// and `under` itself, and each of the member's own folders with those
/// above it.
fn first_open(rows: &[Row<Entry>], member: &str, under: &str) -> BTreeSet<String> {
    let own = format!("+{member}");
    let mut open = BTreeSet::new();
    let mut above = |path: &str| {
        open.extend(file_tree::ancestors(path).map(str::to_owned));
        open.insert(path.to_owned());
    };
    for row in rows {
        if let Row::Folder { folder, .. } = row
            && folder.name() == own
        {
            above(&folder.path);
        }
    }
    if !under.is_empty() {
        above(under);
    }
    open
}

/// Whether `member` adds files to the folder `path` without asking: no
/// statement folder holds it, and the rightmost `+name` tag above it, if
/// any, is the member's own.
fn adds_freely(path: &str, member: &str) -> bool {
    !file_tree::contains(".pigeon", path)
        && path
            .split('/')
            .rev()
            .find_map(|name| name.strip_prefix('+'))
            .is_none_or(|tag| tag.eq_ignore_ascii_case(member))
}

/// A time in RFC 3339 to the minute.
fn short_time(time: &str) -> String {
    time.get(..16).unwrap_or(time).replacen('T', " ", 1)
}

/// The menu button of a row, which tells `files.js` what can be done:
/// `member` adds files to a folder freely or by request.
fn menu_button(
    kind: &str,
    path: &str,
    folder: Option<&Folder<Entry>>,
    entry: Option<&Entry>,
    member: &str,
) -> Markup {
    let addable = folder.map(|_| adds_freely(path, member));
    let pattern = GroupPath::parse(path).ok().map(|parsed| match kind {
        "file" => exact_pattern(&parsed),
        _ => folder_pattern(&parsed),
    });
    let writable = match (folder, entry) {
        (Some(folder), _) => folder
            .leaves()
            .filter_map(|entry| entry.file)
            .all(|file| file["writable"] != false),
        (None, Some(entry)) => entry.file.is_none_or(|file| file["writable"] != false),
        (None, None) => true,
    };
    let waiting = match (folder, entry) {
        (Some(folder), _) => folder.leaves().filter_map(|entry| entry.waiting).collect(),
        (None, Some(entry)) => entry.waiting.into_iter().collect(),
        (None, None) => Vec::new(),
    };
    let freezes = waiting.iter().any(|item| item["freezes"] == true);
    let published = entry.is_some_and(|entry| entry.file.is_some());
    html! {
        button type="button" class="more" title="Actions" data-kind=(kind) data-path=(path)
            data-pattern=[pattern] data-writable=(writable) data-waiting=(waiting.len())
            data-freezes=(freezes) data-published=(published) data-addable=[addable] { "⋯" }
    }
}

/// The dialogs the menu opens, each filled by `files.js` for its row.
fn dialogs(group: &str, back: &str) -> Markup {
    let hidden = |name: &str| html! { input type="hidden" name=(name); };
    let start = html! {
        input type="hidden" name="back" value=(back);
        input type="hidden" name="group" value=(group);
    };
    let request = html! {
        div class="request" {
            p { "You may not write this: it becomes a request to its owner." }
            label { span { "Why, for the owner" } input type="text" name="message"; }
        }
    };
    let close = html! { button type="button" value="" class="close" { "Cancel" } };
    html! {
        dialog id="menu" {
            p { strong class="subject" {} }
            div class="choices" {
                button type="button" data-open="rename" { "Rename…" }
                button type="button" data-open="replace" { "Replace…" }
                button type="button" data-open="add" { "Add file…" }
                button type="button" data-open="delete" { "Delete…" }
                form id="download" method="post" action="/act/selection/download" enctype="multipart/form-data" {
                    (start) (hidden("pattern")) button { "Download once" }
                }
                form id="publish" method="post" action="/act/file/publish" enctype="multipart/form-data" {
                    (start) (hidden("path")) button { "Publish now" }
                }
                a id="history" { "History" }
            }
            (close)
        }
        dialog id="rename" {
            form method="post" action="/act/file/rename" enctype="multipart/form-data" {
                p { "Rename " strong class="subject" {} } (start) (hidden("from"))
                label { span { "New path" } input type="text" name="to" required; }
                (request) button { "Rename" } " " (close)
            }
        }
        dialog id="replace" {
            form method="post" action="/act/file/write" enctype="multipart/form-data" {
                p { "Replace " strong class="subject" {} } (start) (hidden("path"))
                label { span { "New content" } input type="file" name="content" required; }
                (request) button { "Replace" } " " (close)
            }
        }
        dialog id="add" {
            form method="post" action="/act/file/write" enctype="multipart/form-data" {
                p { "Add a file to " strong class="subject" {} } (start)
                label { span { "Path" } input type="text" name="path" required; }
                label { span { "Content" } input type="file" name="content" required; }
                (request) button { "Add" } " " (close)
            }
        }
        dialog id="delete" {
            form method="post" action="/act/file/delete" enctype="multipart/form-data" {
                p { "Delete " strong class="subject" {} "?" } (start) (hidden("path"))
                (request) button { "Delete" } " " (close)
            }
        }
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

/// One row of the tree, marked when it is the folder `under` the page
/// shows.
fn row(
    group: &str,
    member: &str,
    under: &str,
    row: &Row<Entry>,
    open: &BTreeSet<String>,
) -> Markup {
    let path = row.path();
    let target = (path == under).then_some("target");
    let hidden = file_tree::ancestors(path).any(|folder| !open.contains(folder));
    let indent = format!(
        "padding-left: {}em",
        0.5 + 1.25 * f64::from(u32::try_from(row.depth()).unwrap_or(0))
    );
    match row {
        Row::Folder { folder, .. } => {
            let is_open = open.contains(path);
            let summary = &folder.summary;
            html! {
                tr data-path=(path) class=[target] data-folder data-open[is_open] hidden[hidden] {
                    td { (follow_box(path, folder_pattern, summary.followed())) }
                    td style=(indent) {
                        button type="button" class="twist" {
                            span class="arrow" { @if is_open { "▾" } @else { "▸" } } " " (folder.name()) "/"
                        }
                    }
                    td { (size(summary.size)) }
                    td {}
                    td { @if let Some(time) = &summary.time { (short_time(time)) } }
                    td { (folder_status(summary.waiting)) }
                    td { (menu_button("folder", path, Some(folder), None, member)) }
                }
            }
        }
        Row::File { leaf: entry, .. } => {
            let item = entry.file.or(entry.waiting);
            let facts = entry.facts();
            let owner = match (entry.file, entry.drafts.first()) {
                (Some(file), _) => file["owner"].as_str(),
                (None, Some(draft)) if entry.waiting.is_none() => draft["author"].as_str(),
                _ => None,
            };
            let name = path.rsplit('/').next().unwrap_or_default();
            html! {
                tr data-path=(path) class=[target.or(item.is_none().then_some("draft"))] hidden[hidden] {
                    td { (follow_box(path, exact_pattern, item.map(|item| if follows(item) { Followed::All } else { Followed::None }))) }
                    td style=(indent) {
                        @if entry.file.is_some() { a href=(file_link(group, path)) { (name) } }
                        @else { (name) }
                    }
                    td { (size(facts.size)) }
                    td { (owner.unwrap_or_default()) }
                    td { @if let Some(time) = facts.time { (short_time(time)) } }
                    td { (file_status(entry.file, entry.waiting, &entry.drafts)) }
                    td { @if item.is_some() { (menu_button("file", path, None, Some(entry), member)) } }
                }
            }
        }
    }
}

/// The whole group as a tree: the folders above `under` open, and those
/// of `member`, until one opens or closes others.
#[must_use]
pub fn files(group: &str, member: &str, under: &str, files: &Value, waiting: &Value) -> Markup {
    let files = files.as_array().map(Vec::as_slice).unwrap_or_default();
    let waiting = waiting.as_array().map(Vec::as_slice).unwrap_or_default();
    let tree = Folder::root(entries(files, waiting));
    let rows = tree.rows();
    let open = first_open(&rows, member, under);
    let back = if under.is_empty() {
        format!("/g/{group}/files")
    } else {
        format!("/g/{group}/files?under={}", encode(under))
    };
    let here: Vec<&Value> = waiting
        .iter()
        .filter(|item| item["here"] != false)
        .collect();
    let freezes = here.iter().any(|item| item["freezes"] == true);
    let body = html! {
        p {
            button type="button" id="expand-all" { "Expand all" } " "
            button type="button" id="collapse-all" { "Collapse all" }
        }
        @if here.len() > 1 {
            p { (here.len()) " edits wait to be published." (publish_button(group, &back, "", freezes)) }
        }
        table class="tree" data-group=(group) data-under=(under) {
            tr { th {} th { "Name" } th { "Size" } th { "Owner" } th { "Time" } th { "State" } th {} }
            tr class="root" data-path="" {
                td {}
                td { strong { (group) } }
                td { (size(tree.summary.size)) }
                td {}
                td { @if let Some(time) = &tree.summary.time { (short_time(time)) } }
                td { (folder_status(tree.summary.waiting)) }
                td { (menu_button("root", "", Some(&tree), None, member)) }
            }
            @for one in &rows { (row(group, member, under, one, &open)) }
        }
        @if tree.is_empty() { p { "Nothing yet." } }
        (legend())
        (dialogs(group, &back))
        script src="/files.js" defer {}
    };
    layout("Files", Some(group), &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file(path: &str, cutoff: &Value) -> Value {
        json!({"path": path, "owner": "alice", "content": {"size": 1}, "cutoff": cutoff,
            "time": "2026-01-01T00:00:00Z", "held": true, "outdated": false, "writable": true})
    }

    fn row_of<'a>(page: &'a str, path: &str) -> &'a str {
        let at = page.find(&format!(r#"<tr data-path="{path}""#)).unwrap();
        &page[at..at + page[at..].find("</tr>").unwrap()]
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
        let page = files("cheapmo", "alice", "", &list, &json!([])).into_string();
        assert!(
            row_of(&page, "docs/all")
                .contains(r#"data-pattern="/docs/all/" data-state="checked" checked"#)
        );
        assert!(row_of(&page, "docs/some").contains(r#"data-state="mixed">"#));
        assert!(row_of(&page, "docs/none").contains(r#"data-state="unchecked">"#));
        assert!(
            row_of(&page, "docs/a.txt")
                .contains(r#"data-pattern="/docs/a.txt" data-state="checked" checked"#)
        );
        assert!(page.contains(r#"data-pattern="/docs/\[x\].txt" data-state="unchecked">"#));
        assert!(row_of(&page, "docs/[x].txt").contains("🧊"));
        assert!(!row_of(&page, "docs/a.txt").contains(r#"class="status""#));
        assert!(page.contains(r#"<ul class="legend">"#));
        assert!(!row_of(&page, "docs").contains("hidden"));
        assert!(row_of(&page, "docs/all").contains("hidden"));
        assert!(row_of(&page, "docs/all/b.txt").contains("hidden"));
        assert!(row_of(&page, "docs").contains("6 B"));
        assert!(page.contains(r#"<dialog id="unfollow">"#));
        let at = |path: &str| page.find(&format!(r#"<tr data-path="{path}""#)).unwrap();
        assert!(at("docs/some") < at("docs/a.txt"), "folders come first");
    }

    #[test]
    fn under_and_the_members_own_folder_start_open() {
        let list = json!([
            file("docs/deep/a.txt", &json!("PlusInfinity")),
            file("team/+alice/b.txt", &json!("PlusInfinity")),
            file("team/+bob/c.txt", &json!("PlusInfinity")),
        ]);
        let page = files("cheapmo", "alice", "docs/deep", &list, &json!([])).into_string();
        assert!(row_of(&page, "docs").contains("data-open"));
        assert!(!row_of(&page, "docs/deep/a.txt").contains("hidden"));
        assert!(!row_of(&page, "team/+alice/b.txt").contains("hidden"));
        assert!(row_of(&page, "team/+bob/c.txt").contains("hidden"));
        assert!(page.contains(r#"data-under="docs/deep""#));
        assert!(row_of(&page, "docs/deep").contains(r#"class="target""#));
        assert!(page.contains(r#"name="back" value="/g/cheapmo/files?under=docs/deep""#));
    }

    #[test]
    fn waiting_edits_show_in_their_folder_with_the_time_left() {
        let list = json!([file("+alice/a.txt", &json!("PlusInfinity"))]);
        let waiting = json!([
            {"path": "+alice/a.txt", "here": true, "due_in": 2, "freezes": false, "deleted": false, "cutoff": "PlusInfinity", "size": 3},
            {"path": "+alice/new/b.txt", "here": true, "due_in": 3, "freezes": false, "deleted": false, "cutoff": "PlusInfinity", "size": 4},
            {"path": "+alice/c.txt", "here": true, "due_in": 192, "freezes": true, "deleted": false, "cutoff": "MinusInfinity", "size": 5},
        ]);
        let page = files("cheapmo", "alice", "", &list, &waiting).into_string();
        assert!(page.contains(r#"⏳ <span data-due="2">0:02</span>"#));
        assert!(page.contains(r#"<span data-due="192">3:12</span>"#));
        assert!(row_of(&page, "+alice/new").contains("⏳ 1"));
        assert!(page.contains("3 edits wait to be published."));
        assert_eq!(page.matches("data-confirm=").count(), 1, "{page}");
        assert!(row_of(&page, "+alice").contains(r#"data-waiting="3" data-freezes="true""#));
        assert!(
            row_of(&page, "+alice/c.txt")
                .contains(r#"data-pattern="/+alice/c.txt" data-state="unchecked""#)
        );
        assert!(row_of(&page, "+alice/c.txt").contains(r#"data-published="false""#));
    }

    #[test]
    fn rival_drafts_warn_and_tell_the_later_its_copy_is_set_aside() {
        let waiting = json!([
            {"path": "inbox/Report.txt", "here": true, "author": "alice", "due_in": 200, "freezes": true,
             "deleted": false, "cutoff": "PlusInfinity", "size": 3,
             "rivals": [{"author": "bob", "path": "inbox/report.txt", "due_in": 42, "wins": true}]},
            {"path": "inbox/report.txt", "here": false, "author": "bob", "due_in": 42, "freezes": true,
             "deleted": false, "cutoff": "MinusInfinity", "size": 7,
             "rivals": [{"author": "alice", "path": "inbox/Report.txt", "due_in": 200, "wins": false}]},
        ]);
        let page = files("cheapmo", "alice", "", &json!([]), &waiting).into_string();
        let mine = row_of(&page, "inbox/Report.txt");
        assert!(mine.contains(r#"🛑 bob <span data-due="42">0:42</span>"#));
        assert!(mine.contains("rename it to keep both"));
        let theirs = row_of(&page, "inbox/report.txt");
        assert!(theirs.contains(r#"class="draft""#));
        assert!(theirs.contains(r#"✍️ bob <span data-due="42">0:42</span>"#));
        assert!(theirs.contains(r#"⚠️ alice <span data-due="200">3:20</span>"#));
        assert!(theirs.contains("<td>bob</td>") && theirs.contains("7 B"));
        assert!(!theirs.contains("checkbox") && !theirs.contains("⋯"));
        assert!(!theirs.contains("set aside"));
        assert!(!page.contains("edits wait to be published."));
    }
}
