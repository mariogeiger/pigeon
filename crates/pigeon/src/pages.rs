//! The web UI's pages as HTML: the layout, tables of an action's columns,
//! text differences, and one function per page, each from data the
//! handlers fetched.

use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::catalog::{Action, find};
use crate::form::{Fill, form};
use crate::render::{cell, field, header};

const STYLE: &str = "
body { font: 15px system-ui, sans-serif; margin: 0 auto; max-width: 70rem; padding: 1rem; }
nav a { margin-right: 1rem; }
table { border-collapse: collapse; width: 100%; margin: 1rem 0; }
th, td { text-align: left; padding: .25rem .5rem; border-bottom: 1px solid #ddd; }
form.action { display: inline-flex; flex-wrap: wrap; gap: .5rem; align-items: end; margin: .25rem 0; }
details.action { margin: .5rem 0; }
details.action form { display: flex; }
label { display: flex; flex-direction: column; font-size: 13px; }
pre.diff { background: #f6f6f6; padding: .5rem; overflow-x: auto; }
.ins { background: #dfd; } .del { background: #fdd; }
.notice { background: #eef; padding: .5rem; } .error { background: #fdd; padding: .5rem; }
.mark { color: #a50; font-weight: bold; }
section { margin: 1.5rem 0; }
";

/// The action `noun verb`, which the catalog defines.
///
/// # Panics
///
/// Panics if the catalog lacks it, which its tests rule out.
#[must_use]
pub fn action(noun: &str, verb: &str) -> &'static Action {
    find(noun, verb).expect("the web UI uses actions of the catalog")
}

/// A whole page, which `live.js` keeps live when it is a group's.
#[must_use]
pub fn layout(title: &str, group: Option<&str>, body: &Markup) -> Markup {
    html! {
        (DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                title { (title) " · pigeon" }
                style { (PreEscaped(STYLE)) }
                script src="/live.js" defer {}
            }
            body data-group=[group] {
                nav {
                    a href="/" { "Groups" }
                    @if let Some(group) = group {
                        a href={ "/g/" (group) } { (group) }
                        a href={ "/g/" (group) "/files" } { "Files" }
                        a href={ "/g/" (group) "/requests" } { "Requests" }
                        a href={ "/g/" (group) "/aside" } { "Set aside" }
                        a href={ "/g/" (group) "/members" } { "Members" }
                        a href={ "/g/" (group) "/selection" } { "Selection" }
                        a href={ "/g/" (group) "/retention" } { "Retention" }
                    }
                }
                main {
                    h1 { (title) }
                    (body)
                }
            }
        }
    }
}

/// `items` as a table of `action`'s columns; `link` turns an item into the
/// address its first cell links to.
#[must_use]
pub fn table(action: &Action, items: &Value, link: &dyn Fn(&Value) -> Option<String>) -> Markup {
    let items = items.as_array().map(Vec::as_slice).unwrap_or_default();
    html! {
        @if items.is_empty() {
            p { "Nothing yet." }
        } @else {
            table {
                tr { @for column in action.columns { th { (header(column)) } } }
                @for item in items {
                    tr {
                        @for (index, column) in action.columns.iter().enumerate() {
                            td {
                                @let text = cell(column, field(item, column));
                                @match link(item).filter(|_| index == 0) {
                                    Some(address) => a href=(address) { (text) },
                                    None => (text),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The bytes as text, if they are UTF-8 without NUL.
fn as_text(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))
}

/// The largest file whose difference the web UI shows.
const DIFF_LIMIT: usize = 1 << 20;

/// One side of a comparison.
pub enum Side {
    /// No file: an addition's old side or a deletion's new side.
    NoFile,
    /// Content this machine does not hold.
    Unavailable,
    Bytes(Vec<u8>),
}

impl Side {
    fn bytes(&self) -> Option<&[u8]> {
        match self {
            Self::NoFile => Some(&[]),
            Self::Unavailable => None,
            Self::Bytes(bytes) => Some(bytes),
        }
    }
}

/// What changes from `old` to `new`, as a line diff for text.
#[must_use]
pub fn diff(old: &Side, new: &Side) -> Markup {
    let (Some(old), Some(new)) = (old.bytes(), new.bytes()) else {
        return html! { p { "The content is not on this machine yet." } };
    };
    let (Some(old), Some(new)) = (as_text(old), as_text(new)) else {
        return html! { p { "Binary content." } };
    };
    if old.len() + new.len() > DIFF_LIMIT {
        return html! { p { "Too large to compare." } };
    }
    let diff = TextDiff::from_lines(old, new);
    html! {
        pre class="diff" {
            @for group in diff.grouped_ops(3) {
                @for op in &group {
                    @for change in diff.iter_changes(op) {
                        @let (class, sign) = match change.tag() {
                            ChangeTag::Delete => ("del", "-"),
                            ChangeTag::Insert => ("ins", "+"),
                            ChangeTag::Equal => ("", " "),
                        };
                        span class=(class) { (sign) " " (change.value().trim_end_matches('\n')) "\n" }
                    }
                }
                "⋯\n"
            }
        }
    }
}

/// The groups on this machine, and the forms to found or join one.
#[must_use]
pub fn home(groups: &Value) -> Markup {
    let link = |item: &Value| item["name"].as_str().map(|name| format!("/g/{name}"));
    let body = html! {
        (table(action("group", "list"), groups, &link))
        (form(action("group", "create"), "/", Fill::default()))
        (form(action("group", "join"), "/", Fill::default()))
    };
    layout("Groups", None, &body)
}

/// The fields of an object, one per row.
#[must_use]
pub fn fields(value: &Value) -> Markup {
    html! {
        table {
            @if let Some(fields) = value.as_object() {
                @for (name, value) in fields {
                    tr { th { (name) } td { (cell(name, value)) } }
                }
            }
        }
    }
}

/// How a group stands here, its key, the form that names the group's
/// relay, and, unless the member belongs, why and the form to claim a name
/// or log in with a new password.
#[must_use]
pub fn overview(group: &str, status: &Value, key: &str) -> Markup {
    let back = format!("/g/{group}");
    let fill = Fill {
        group: Some(group),
        ..Fill::default()
    };
    let body = html! {
        (fields(status))
        @if status["join"]["state"] != "joined" {
            p class="mark" {
                (status["join"]["reason"].as_str().unwrap_or("This machine has not joined yet."))
            }
            (form(action("member", "claim"), &back, fill))
        }
        section {
            h2 { "Group key" }
            p { "Share it with a new member so that their machine can join." }
            textarea readonly rows="3" cols="80" { (key) }
        }
        section {
            h2 { "Relay" }
            p { "Machines that cannot connect directly talk through a relay, which sees only ciphertext: iroh's public relays, or the group's own, served by " code { "pigeon relay" } "." }
            (form(action("group", "relay"), &back, fill))
        }
    };
    layout(group, Some(group), &body)
}

/// A generic page: a view's table and the forms that go with it.
#[must_use]
pub fn listing(group: &str, title: &str, view: &Action, items: &Value, forms: &Markup) -> Markup {
    let body = html! {
        (table(view, items, &|_| None))
        (forms)
    };
    layout(title, Some(group), &body)
}

/// This machine's retention, and the form to change it, filled with the
/// current values.
#[must_use]
pub fn retention(group: &str, retention: &Value) -> Markup {
    let back = format!("/g/{group}/retention");
    let current: Vec<(&str, String)> = action("retention", "set")
        .params
        .iter()
        .map(|param| (param.name, cell(param.name, &retention[param.name])))
        .collect();
    let defaults: Vec<(&str, &str)> = current
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let fill = Fill {
        group: Some(group),
        fixed: &[],
        defaults: &defaults,
    };
    let body = html! {
        (fields(retention))
        (form(action("retention", "set"), &back, fill))
    };
    layout("Retention", Some(group), &body)
}
