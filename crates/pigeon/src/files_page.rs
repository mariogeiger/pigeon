//! The web UI's Files page: the whole group as one tree, whose folders
//! `files.js` opens and closes in place. Each row has a box that follows
//! it, checked when every file under it is followed and mixed when only
//! some are, its size, author, time and statuses, one emoji each, which a
//! legend below the tree explains, and a menu of what can be done to it,
//! the suggestions at it and under it included, which the page renders
//! and `files.js` only shows. The drafts other machines announce, and the
//! suggestions at paths without a file, show greyed. Every change asks to
//! be confirmed, then publishes at once.

use std::collections::{BTreeMap, BTreeSet};

use maud::{Markup, html};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{exact_pattern, folder_pattern};
use serde_json::Value;

use crate::file_status::{
    Status, countdown, file_status, folder_status, legend, mode, suggested_status, suggestion_title,
};
use crate::file_tree::{self, Facts, Folder, Followed, Leaf, Row};
use crate::form::{asking, form};
use crate::pages::{Bar, PUBLISHES, action, deciding, encode, file_link, fill, layout, short_time};
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
    mode(item) == "follow"
}

/// A change suggested at a path: the place of its suggestion among all,
/// oldest first, the suggestion, and the change.
type Suggested<'a> = (usize, &'a Value, &'a Value);

/// One path of the group: its published file, the edit of it waiting
/// here, the drafts of it other machines announced, and the changes of it
/// suggested.
struct Entry<'a> {
    path: &'a str,
    file: Option<&'a Value>,
    waiting: Option<&'a Value>,
    drafts: Vec<&'a Value>,
    suggested: Vec<Suggested<'a>>,
}

impl Leaf for Entry<'_> {
    fn path(&self) -> &str {
        self.path
    }

    fn facts(&self) -> Facts<'_> {
        let size = match (self.file, self.waiting, self.drafts.first().copied()) {
            (Some(file), _, _) => &file["content"]["size"],
            (None, Some(item), _) | (None, None, Some(item)) => &item["size"],
            (None, None, None) => self
                .suggested
                .first()
                .map_or(&Value::Null, |(_, _, change)| &change["content"]["size"]),
        };
        Facts {
            size: size.as_u64().unwrap_or_default(),
            followed: self.file.or(self.waiting).map(follows),
            time: self.file.and_then(|file| file["time"].as_str()),
            waiting: self.waiting.is_some(),
            suggested: self.suggested.len(),
        }
    }
}

/// The items of a list result.
fn listed(list: &Value) -> &[Value] {
    list.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The entry of `path` in `entries`, added empty if missing.
fn entry_at<'a, 'm>(
    entries: &'m mut BTreeMap<&'a str, Entry<'a>>,
    path: &'a str,
) -> &'m mut Entry<'a> {
    entries.entry(path).or_insert_with(|| Entry {
        path,
        file: None,
        waiting: None,
        drafts: Vec::new(),
        suggested: Vec::new(),
    })
}

/// The files, the waiting edits and the suggested changes, one entry per
/// path.
fn entries<'a>(
    files: &'a [Value],
    waiting: &'a [Value],
    suggestions: &'a [Value],
) -> Vec<Entry<'a>> {
    let mut entries = BTreeMap::new();
    let path = |item: &'a Value| item["path"].as_str().unwrap_or_default();
    for file in files {
        entry_at(&mut entries, path(file)).file = Some(file);
    }
    for item in waiting {
        let entry = entry_at(&mut entries, path(item));
        if item["here"] == true {
            entry.waiting = Some(item);
        } else {
            entry.drafts.push(item);
        }
    }
    for (order, suggestion) in suggestions.iter().enumerate() {
        for change in listed(&suggestion["changes"]) {
            let path = change["path"].as_str().unwrap_or_default();
            entry_at(&mut entries, path)
                .suggested
                .push((order, suggestion, change));
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

/// The edit waiting at `path`: the time left before it is published, and
/// the button that publishes it now.
pub fn waiting_note(group: &str, back: &str, path: &str, waiting: &Value) -> Markup {
    html! {
        @if waiting["deleted"] == true { "deletion " }
        "waiting · published in " (countdown(&waiting["due_in"]))
        (form(action("file", "publish"), back, fill(group, &[("path", path)], &[])))
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

/// The suggestions at `entries`, once each, oldest first, each with what
/// the first change of it there does and the forms that decide it, then,
/// when there are several, those that decide them all.
fn suggestion_lines<'a>(
    group: &str,
    back: &str,
    entries: impl Iterator<Item = &'a Entry<'a>>,
) -> Markup {
    let mut offered = BTreeMap::new();
    for (order, suggestion, change) in entries.flat_map(|entry| entry.suggested.iter()) {
        offered.entry(*order).or_insert((*suggestion, *change));
    }
    let id = |suggestion: &'a Value| suggestion["id"].as_str().unwrap_or_default();
    let ids: Vec<&str> = offered
        .values()
        .map(|(suggestion, _)| id(suggestion))
        .collect();
    let emoji = Status::Suggested.emoji();
    html! {
        @for (suggestion, change) in offered.values() {
            @let placeable_at = change["path"].as_str().filter(|_| suggestion["placeable"] == true);
            div class="change" {
                p { (emoji) " " (suggestion_title(suggestion, change)) }
                (deciding(group, back, &[id(suggestion)], placeable_at))
            }
        }
        @if ids.len() > 1 {
            div class="change" {
                p { (emoji) " " (ids.len()) " suggestions" }
                (deciding(group, back, &ids, None))
            }
        }
    }
}

/// A place of the tree its menu acts on: the group's root, a folder, or a
/// file.
#[derive(Clone, Copy)]
enum Place<'t, 'a> {
    Root(&'t Folder<Entry<'a>>),
    Folder(&'t Folder<Entry<'a>>),
    File(&'t Entry<'a>),
}

/// The menu of `place`, returning to `back`: its button, which tells
/// `files.js` the place's path, pattern and name, which of the menu's
/// choices it offers and where its history is, and the suggestions at it
/// and under it, each with the forms that decide it.
fn menu(group: &str, back: &str, place: Place<'_, '_>) -> Markup {
    let (path, under): (&str, Vec<&Entry>) = match place {
        Place::Root(folder) | Place::Folder(folder) => (&folder.path, folder.leaves().collect()),
        Place::File(entry) => (entry.path, vec![entry]),
    };
    let parsed = GroupPath::parse(path).ok();
    let (subject, pattern, offers, link) = match place {
        Place::Root(_) => (group.to_owned(), None, vec!["add"], None),
        Place::Folder(_) => (
            format!("{path}/"),
            parsed.as_ref().map(folder_pattern),
            vec!["rename", "add", "delete", "pin"],
            None,
        ),
        Place::File(entry) => {
            let published = entry.file.is_some();
            let editable = entry.file.or(entry.waiting).is_some();
            let offers = [
                ("rename", editable),
                ("replace", editable),
                ("delete", editable),
                ("pin", published),
                ("history", published),
            ];
            (
                path.to_owned(),
                parsed.as_ref().map(exact_pattern),
                offers
                    .into_iter()
                    .filter(|(_, offered)| *offered)
                    .map(|(offer, _)| offer)
                    .collect(),
                published.then(|| file_link(group, path)),
            )
        }
    };
    let waiting = under.iter().any(|entry| entry.waiting.is_some());
    let offers = offers
        .into_iter()
        .chain(waiting.then_some("publish"))
        .collect::<Vec<_>>();
    let suggested = under.iter().any(|entry| !entry.suggested.is_empty());
    html! {
        button type="button" class="more" title="Actions" data-path=(path) data-subject=(subject)
            data-pattern=[pattern] data-offers=(offers.join(" ")) data-link=[link] { "⋯" }
        @if suggested { template { (suggestion_lines(group, back, under.into_iter())) } }
    }
}

/// A dialog the menu opens, asking `question` of the place it was opened
/// for, then running the form `asked`, once `files.js` filled both for it.
fn dialog(id: &str, question: &Markup, asked: &Markup) -> Markup {
    html! {
        dialog id=(id) {
            p { (question) }
            (asked)
            button type="button" class="close" { "Cancel" }
        }
    }
}

/// The menu and the dialogs it opens, each filled by `files.js` for its
/// place, then the question unchecking a box asks, whose answers, like
/// checking one, change the selection in place.
fn dialogs(group: &str, back: &str) -> Markup {
    let note = format!("{PUBLISHES} The history keeps what it replaces.");
    let ask = |noun, verb, fixed, defaults| {
        asking(
            action(noun, verb),
            back,
            fill(group, fixed, defaults),
            &note,
        )
    };
    let run = |noun, verb, fixed| form(action(noun, verb), back, fill(group, fixed, &[]));
    let subject = html! { strong class="subject" {} };
    let path = [("path", "")];
    let pattern = [("pattern", "")];
    let pin = [("pattern", ""), ("time", "now")];
    html! {
        dialog id="menu" {
            p { (subject) }
            div class="suggestions" {}
            div class="choices" {
                @for (offer, label) in [("rename", "Rename…"), ("replace", "Replace…"), ("add", "Add file…"), ("delete", "Delete…")] {
                    div data-offer=(offer) { button type="button" data-open=(offer) { (label) } }
                }
                div data-offer="pin" { (run("selection", "pin", &pin)) }
                div data-offer="publish" { (run("file", "publish", &path)) }
                div data-offer="history" { a id="history" { "History" } }
            }
            button type="button" class="close" { "Cancel" }
        }
        (dialog("rename", &html! { "Rename " (subject) }, &ask("file", "rename", &[("from", "")], &[("to", "")])))
        (dialog("replace", &html! { "Replace " (subject) }, &ask("file", "write", &path, &[])))
        (dialog("add", &html! { "Add a file to " (subject) }, &ask("file", "write", &[], &path)))
        (dialog("delete", &html! { "Delete " (subject) "?" }, &ask("file", "delete", &path, &[])))
        dialog id="unfollow" {
            p { "Stop following: pin the copy here as it is now, or free the space?" }
            div data-in-place { (run("selection", "pin", &pin)) " " (run("selection", "free", &pattern)) }
            form method="dialog" { button { "Cancel" } }
        }
        div id="follow" data-in-place hidden { (run("selection", "follow", &pattern)) }
    }
}

/// One row of the tree, marked when it is the folder `under` the page
/// shows.
fn row(group: &str, back: &str, under: &str, row: &Row<Entry>, open: &BTreeSet<String>) -> Markup {
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
                    td { (folder_status(summary.waiting, summary.suggested)) }
                    td { (menu(group, back, Place::Folder(folder))) }
                }
            }
        }
        Row::File { leaf: entry, .. } => {
            let item = entry.file.or(entry.waiting);
            let facts = entry.facts();
            let author = match (entry.file, entry.drafts.first()) {
                (Some(file), _) => file["author"].as_str(),
                (None, Some(draft)) if entry.waiting.is_none() => draft["author"].as_str(),
                _ => None,
            };
            let name = path.rsplit('/').next().unwrap_or_default();
            html! {
                tr data-path=(path) class=[target.or(item.is_none().then_some("draft"))] hidden[hidden] {
                    td { (follow_box(path, exact_pattern, item.map(|item| if follows(item) { Followed::All } else { Followed::None }))) }
                    td style=(indent) {
                        @if entry.file.is_some() || (!entry.suggested.is_empty() && GroupPath::parse(path).is_ok()) {
                            a href=(file_link(group, path)) { (name) }
                        } @else { (name) }
                    }
                    td { (size(facts.size)) }
                    td { (author.unwrap_or_default()) }
                    td { @if let Some(time) = facts.time { (short_time(time)) } }
                    td {
                        (file_status(entry.file, entry.waiting, &entry.drafts))
                        (suggested_status(entry.suggested.iter().map(|(_, suggestion, change)| (*suggestion, *change))))
                    }
                    td {
                        @if item.is_some() || !entry.suggested.is_empty() {
                            (menu(group, back, Place::File(entry)))
                        }
                    }
                }
            }
        }
    }
}

/// The whole group as a tree, with the changes suggested at each path:
/// the folders above `under` open, and those of `member`, until one opens
/// or closes others.
#[must_use]
pub fn files(
    bar: &Bar<'_>,
    member: &str,
    under: &str,
    files: &Value,
    waiting: &Value,
    suggestions: &Value,
) -> Markup {
    let group = bar.group;
    let (files, waiting, suggestions) = (listed(files), listed(waiting), listed(suggestions));
    let tree = Folder::root(entries(files, waiting, suggestions));
    let rows = tree.rows();
    let open = first_open(&rows, member, under);
    let back = if under.is_empty() {
        format!("/g/{group}")
    } else {
        format!("/g/{group}?under={}", encode(under))
    };
    let here = waiting.iter().filter(|item| item["here"] != false).count();
    let body = html! {
        p {
            button type="button" id="expand-all" { "Expand all" } " "
            button type="button" id="collapse-all" { "Collapse all" }
        }
        @if here > 1 {
            p { (here) " edits wait to be published." (form(action("file", "publish"), &back, fill(group, &[("path", "")], &[]))) }
        }
        table class="tree" data-group=(group) data-under=(under) {
            tr { th {} th { "Name" } th { "Size" } th { "Author" } th { "Time" } th { "State" } th {} }
            tr class="root" data-path="" {
                td {}
                td { strong { (group) } }
                td { (size(tree.summary.size)) }
                td {}
                td { @if let Some(time) = &tree.summary.time { (short_time(time)) } }
                td { (folder_status(tree.summary.waiting, tree.summary.suggested)) }
                td { (menu(group, &back, Place::Root(&tree))) }
            }
            @for one in &rows { (row(group, &back, under, one, &open)) }
        }
        @if tree.is_empty() { p { "Nothing yet." } }
        (legend())
        (dialogs(group, &back))
        script src="/files.js" defer {}
    };
    layout("Files", Some(bar), &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: Bar = Bar {
        group: "cheapmo",
        tab: Some(crate::pages::Tab::Files),
        suggestions: 0,
    };
    use serde_json::json;

    fn file(path: &str, cutoff: &str) -> Value {
        json!({"path": path, "owner": "alice", "author": "alice", "content": {"size": 1}, "cutoff": cutoff,
            "time": "2026-01-01T00:00:00Z", "held": true, "outdated": false})
    }

    fn row_of<'a>(page: &'a str, path: &str) -> &'a str {
        let at = page.find(&format!(r#"<tr data-path="{path}""#)).unwrap();
        &page[at..at + page[at..].find("</tr>").unwrap()]
    }

    #[test]
    fn a_box_is_checked_mixed_or_unchecked_as_the_files_under_it_are_followed() {
        let list = json!([
            file("docs/a.txt", "follow"),
            file("docs/all/b.txt", "follow"),
            file("docs/some/c.txt", "follow"),
            file("docs/some/d.txt", "pin 2026-01-01T00:00:00Z"),
            file("docs/none/e.txt", "free"),
            file("docs/[x].txt", "pin 2026-01-01T00:00:00Z"),
        ]);
        let page = files(&BAR, "alice", "", &list, &json!([]), &json!([])).into_string();
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
        assert!(row_of(&page, "docs/[x].txt").contains("📌"));
        assert!(!row_of(&page, "docs/a.txt").contains(r#"class="status""#));
        assert!(page.contains(r#"<ul class="legend">"#));
        assert!(!row_of(&page, "docs").contains("hidden"));
        assert!(row_of(&page, "docs/all").contains("hidden"));
        assert!(row_of(&page, "docs/all/b.txt").contains("hidden"));
        assert!(row_of(&page, "docs").contains("6 B"));
        let unfollow = &page[page.find(r#"<dialog id="unfollow">"#).unwrap()..];
        let in_place = &unfollow[unfollow.find("<div data-in-place>").unwrap()..];
        assert!(
            in_place.contains(r#"action="/act/selection/pin""#),
            "{unfollow}"
        );
        assert!(in_place.contains(r#"<input type="hidden" name="time" value="now">"#));
        assert!(
            in_place.contains(r#"action="/act/selection/free""#),
            "{unfollow}"
        );
        assert!(page.contains(r#"<div id="follow" data-in-place hidden><form class="action" method="post" action="/act/selection/follow""#), "{page}");
        assert!(
            row_of(&page, "docs/a.txt").contains(
                r#"data-offers="rename replace delete pin history" data-link="/g/cheapmo/file?path=docs/a.txt">"#
            )
        );
        assert!(row_of(&page, "docs/all").contains(r#"data-subject="docs/all/""#));
        assert!(page.contains(r#"data-path="" data-subject="cheapmo" data-offers="add">"#));
        let at = |path: &str| page.find(&format!(r#"<tr data-path="{path}""#)).unwrap();
        assert!(at("docs/some") < at("docs/a.txt"), "folders come first");
    }

    #[test]
    fn under_and_the_members_own_folder_start_open() {
        let list = json!([
            file("docs/deep/a.txt", "follow"),
            file("team/+alice/b.txt", "follow"),
            file("team/+bob/c.txt", "follow"),
        ]);
        let page = files(&BAR, "alice", "docs/deep", &list, &json!([]), &json!([])).into_string();
        assert!(row_of(&page, "docs").contains("data-open"));
        assert!(!row_of(&page, "docs/deep/a.txt").contains("hidden"));
        assert!(!row_of(&page, "team/+alice/b.txt").contains("hidden"));
        assert!(row_of(&page, "team/+bob/c.txt").contains("hidden"));
        assert!(page.contains(r#"data-under="docs/deep""#));
        assert!(row_of(&page, "docs/deep").contains(r#"class="target""#));
        assert!(page.contains(r#"name="back" value="/g/cheapmo?under=docs/deep""#));
    }

    #[test]
    fn waiting_edits_show_in_their_folder_with_the_time_left() {
        let list = json!([file("+alice/a.txt", "follow")]);
        let waiting = json!([
            {"path": "+alice/a.txt", "here": true, "due_in": 2, "draft": false, "deleted": false, "cutoff": "follow", "size": 3},
            {"path": "+alice/new/b.txt", "here": true, "due_in": 3, "draft": false, "deleted": false, "cutoff": "follow", "size": 4},
            {"path": "+alice/c.txt", "here": true, "due_in": 192, "draft": false, "deleted": false, "cutoff": "free", "size": 5},
        ]);
        let page = files(&BAR, "alice", "", &list, &waiting, &json!([])).into_string();
        assert!(page.contains(r#"⏳ <span data-due="2">0:02</span>"#));
        assert!(page.contains(r#"<span data-due="192">3:12</span>"#));
        assert!(row_of(&page, "+alice/new").contains("⏳ 1"));
        assert!(page.contains("3 edits wait to be published."));
        assert!(!page.contains("data-confirm="), "{page}");
        assert!(row_of(&page, "+alice").contains(r#"data-offers="rename add delete pin publish""#));
        assert!(
            row_of(&page, "+alice/c.txt")
                .contains(r#"data-pattern="/+alice/c.txt" data-state="unchecked""#)
        );
        let new = row_of(&page, "+alice/c.txt");
        assert!(
            new.contains(r#"data-offers="rename replace delete publish">"#),
            "{new}"
        );
    }

    #[test]
    fn rival_drafts_warn_and_tell_the_later_it_becomes_a_suggestion() {
        let waiting = json!([
            {"path": "inbox/Report.txt", "here": true, "author": "alice", "due_in": 200, "draft": true,
             "deleted": false, "cutoff": "follow", "size": 3,
             "rivals": [{"author": "bob", "path": "inbox/report.txt", "due_in": 42, "wins": true}]},
            {"path": "inbox/report.txt", "here": false, "author": "bob", "due_in": 42, "draft": true,
             "deleted": false, "cutoff": "free", "size": 7,
             "rivals": [{"author": "alice", "path": "inbox/Report.txt", "due_in": 200, "wins": false}]},
        ]);
        let page = files(&BAR, "alice", "", &json!([]), &waiting, &json!([])).into_string();
        let mine = row_of(&page, "inbox/Report.txt");
        assert!(mine.contains(r#"🛑 bob <span data-due="42">0:42</span>"#));
        assert!(mine.contains("rename it to keep both"));
        let theirs = row_of(&page, "inbox/report.txt");
        assert!(theirs.contains(r#"class="draft""#));
        assert!(theirs.contains(r#"✍️ bob <span data-due="42">0:42</span>"#));
        assert!(theirs.contains(r#"⚠️ alice <span data-due="200">3:20</span>"#));
        assert!(theirs.contains("<td>bob</td>") && theirs.contains("7 B"));
        assert!(!theirs.contains("checkbox") && !theirs.contains("⋯"));
        assert!(!theirs.contains("becomes a suggestion"));
        assert!(!page.contains("edits wait to be published."));
    }

    #[test]
    fn suggestions_show_on_the_rows_they_change_with_what_decides_them() {
        let list = json!([file("docs/list.txt", "follow")]);
        let suggestion = |time: u64, author: &str, changes: Value| {
            json!({"id": format!("s{time}"), "author": author, "placeable": time == 1,
                "reason": "the rules leave it to the group", "changes": changes})
        };
        let suggestions = json!([
            suggestion(
                1,
                "bob",
                json!([{"path": "docs/list.txt", "what": "a new version", "content": {"size": 3},
                "replaces": {"time": 0}, "outdated": false}])
            ),
            suggestion(
                2,
                "papy",
                json!([
                    {"path": "docs/list.txt", "what": "a deletion", "content": null, "replaces": {"time": 0}, "outdated": false},
                    {"path": "docs/Plan.txt", "what": "a move from docs/list.txt", "content": {"size": 9},
                     "continues": {"path": "docs/list.txt"}, "outdated": false},
                ])
            ),
        ]);
        let page = files(&BAR, "alice", "", &list, &json!([]), &suggestions).into_string();
        let changed = row_of(&page, "docs/list.txt");
        assert!(
            changed.contains("📬 bob") && changed.contains("📬 papy"),
            "{changed}"
        );
        assert!(changed.contains("bob suggests a new version"), "{changed}");
        assert!(
            changed.contains(r#"<template><div class="change">"#),
            "{changed}"
        );
        assert!(
            changed.contains(r#"name="suggestions" value="s1""#),
            "{changed}"
        );
        assert!(
            changed.contains(r#"<input type="text" name="to" value="docs/list.txt""#),
            "{changed}"
        );
        assert!(
            changed.contains("Validate these 2 suggestions?"),
            "{changed}"
        );
        let moved = row_of(&page, "docs/Plan.txt");
        assert!(moved.contains(r#"class="draft""#), "{moved}");
        assert!(
            moved.contains("papy suggests a move from docs/list.txt"),
            "{moved}"
        );
        assert!(moved.contains(r#"data-offers="">"#), "{moved}");
        assert!(!moved.contains(r#"name="to" value="docs"#), "{moved}");
        assert!(moved.contains("9 B"), "{moved}");
        let folder = row_of(&page, "docs");
        assert!(folder.contains("📬 3"), "{folder}");
        let offered = folder.find(r#"value="s1""#).unwrap();
        assert!(
            offered < folder.find(r#"value="s2""#).unwrap(),
            "oldest first: {folder}"
        );
        assert!(
            folder.contains(r#"name="suggestions" value="s1 s2""#),
            "{folder}"
        );
        assert!(page.contains(PUBLISHES), "{page}");
        assert!(!page.contains(r#"id="elsewhere""#), "{page}");
    }
}
