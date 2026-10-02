//! The web UI's pages as HTML: the layout with its bar, tables of an
//! action's columns, text differences, an object's fields, the forms that
//! decide suggestions, the addresses and form fillings a group's pages
//! share, and the page of the groups.

use std::fmt::Write;

use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::catalog::{Action, find};
use crate::form::{Fill, asking, form};
use crate::render::{cell, field, header};

const STYLE: &str = "
body { font: 15px system-ui, sans-serif; margin: 0 auto; max-width: 70rem; padding: 1rem; }
nav { display: flex; flex-wrap: wrap; gap: .5rem 1rem; align-items: baseline; padding-bottom: .5rem; border-bottom: 1px solid #ddd; }
nav a { text-decoration: none; color: inherit; } nav a:hover { text-decoration: underline; }
nav .crumbs { font-size: 17px; } nav .tabs { display: flex; gap: 1rem; margin-left: auto; }
nav .current { font-weight: bold; }
table { border-collapse: collapse; width: 100%; margin: 1rem 0; }
th, td { text-align: left; padding: .25rem .5rem; border-bottom: 1px solid #ddd; }
form.action { display: inline-flex; flex-wrap: wrap; gap: .5rem; align-items: end; margin: .25rem 0; }
details.action { margin: .5rem 0; }
details.action form { display: flex; }
label { display: flex; flex-direction: column; font-size: 13px; }
pre.diff { background: #f6f6f6; padding: .5rem; overflow-x: auto; }
.ins { background: #dfd; } .del { background: #fdd; }
.notice { background: #eef; padding: .5rem; } .error { background: #fdd; padding: .5rem; white-space: pre-wrap; }
.mark { color: #a50; font-weight: bold; }
.quiet { color: #666; font-size: 13px; }
section { margin: 1.5rem 0; }
table.tree td { padding-top: .1rem; padding-bottom: .1rem; }
table.tree button.twist, table.tree button.more { border: 0; background: none; padding: 0; font: inherit; cursor: pointer; }
table.tree tr.draft { color: #888; }
table.tree tr.target { background: #ffd; }
table.tree .status { white-space: nowrap; margin-right: .5em; cursor: help; }
ul.legend { list-style: none; padding: 0; font-size: 13px; color: #555; }
table.tree form.action, dialog .change form.action, table.members form.action { display: inline; }
dialog .choices { display: flex; flex-direction: column; align-items: start; gap: .25rem; margin-bottom: .5rem; }
dialog .choices form { margin: 0; }
dialog form label { margin: .5rem 0; }
.key { display: flex; gap: .5rem; } .key input { flex: 1; font-family: monospace; }
.code { position: relative; font: 13px/1.5 ui-monospace, monospace; }
.code pre, .code textarea { margin: 0; padding: .5rem; border: 1px solid #ccc; font: inherit; white-space: pre-wrap; overflow-wrap: anywhere; box-sizing: border-box; width: 100%; tab-size: 4; }
.code pre { position: absolute; inset: 0; overflow: hidden; pointer-events: none; color: #222; background: #fcfcfc; }
.code textarea { position: relative; resize: vertical; background: transparent; color: transparent; caret-color: #000; min-height: 12rem; }
.t-comment { color: #888; } .t-table { color: #a3d; font-weight: bold; } .t-key { color: #05a; }
.t-string { color: #070; } .t-literal { color: #b50; } .t-mode { color: #070; font-weight: bold; }
table.rules code { white-space: pre-wrap; } table.rules .effect { color: #555; font-size: 13px; }
table.rules tr.masked { color: #999; }
ul.changes { list-style: none; padding: 0; }
ul.changes > li { border-bottom: 1px solid #eee; padding: .25rem 0; }
ul.changes .line { display: flex; flex-wrap: wrap; gap: .25rem .75rem; align-items: center; }
ul.changes .line .what { flex: 1; min-width: 16rem; }
ul.changes .line form.action, ul.changes .line details.action { margin: 0; }
ul.changes details.diff > summary { cursor: pointer; font-size: 13px; color: #555; }
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
#[must_use]
pub fn file_link(group: &str, path: &str) -> String {
    format!("/g/{group}/file?path={}", encode(path))
}

/// What a page of `group` knows of a form's arguments: the `fixed` ones,
/// set and not shown, and the `defaults` shown to start from.
#[must_use]
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

/// What a change confirmed on a group's page does.
pub const PUBLISHES: &str = "It publishes at once, for the whole group.";

/// The forms that decide the suggestions `ids` of `group` and then return
/// to `back`, and, when `placeable_at` names the path of the single file
/// they suggest, the one that validates them at another path, starting
/// from it.
#[must_use]
pub fn deciding(group: &str, back: &str, ids: &[&str], placeable_at: Option<&str>) -> Markup {
    let joined = ids.join(" ");
    let chosen = [("suggestions", joined.as_str()), ("to", "")];
    let validate = action("suggestion", "validate");
    html! {
        (form(validate, back, fill(group, &chosen, &[])))
        (form(action("suggestion", "discard"), back, fill(group, &chosen[..1], &[])))
        @if let Some(path) = placeable_at {
            details class="action" {
                summary { "Validate at another path…" }
                (asking(validate, back, fill(group, &[chosen[0], ("yes", "true")], &[("to", path)]), PUBLISHES))
            }
        }
    }
}

/// The pages of a group that the bar names as tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Files,
}

/// Where a group's page stands in the bar: its group, its tab if it is
/// one, and how many suggestions wait for the group.
#[derive(Clone, Copy, Debug)]
pub struct Bar<'a> {
    pub group: &'a str,
    pub tab: Option<Tab>,
    pub suggestions: usize,
}

/// The icon of every page: a bird.
const ICON: &str = "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><text y='.9em' font-size='90'>🐦</text></svg>";

/// The bar atop every page: pigeon, linking to the groups, then the
/// group's name and tabs, the current one in bold.
fn bar(bar: Option<&Bar<'_>>) -> Markup {
    html! {
        nav {
            span class="crumbs" {
                a href="/" { "🐦 pigeon" }
                @if let Some(bar) = bar {
                    " › " a href={ "/g/" (bar.group) } { (bar.group) }
                }
            }
            @if let Some(bar) = bar {
                span class="tabs" {
                    @for (tab, label, address) in [
                        (Tab::Files, "Files", ""),
                        (Tab::Overview, "Overview", "/overview"),
                    ] {
                        a class=[(bar.tab == Some(tab)).then_some("current")] href={ "/g/" (bar.group) (address) } {
                            (label)
                            @if tab == Tab::Files && bar.suggestions > 0 { " (" (bar.suggestions) ")" }
                        }
                    }
                }
            }
        }
    }
}

/// A whole page, which `live.js` keeps live when it is a group's.
#[must_use]
pub fn layout(title: &str, place: Option<&Bar<'_>>, body: &Markup) -> Markup {
    html! {
        (DOCTYPE)
        html {
            head {
                meta charset="utf-8";
                title { (title) " · pigeon" }
                link rel="icon" href=(ICON);
                style { (PreEscaped(STYLE)) }
                script src="/live.js" defer {}
            }
            body data-group=[place.map(|place| place.group)] {
                (bar(place))
                p id="updated" class="notice" hidden {
                    "pigeon has been updated. "
                    button type="button" { "Reload the page" }
                    " to use the new version."
                }
                main {
                    h1 { (title) }
                    (body)
                }
                footer { small { "pigeon " (crate::VERSION) } }
            }
        }
    }
}

/// What a row of a table links to and ends with.
pub type Link<'a> = &'a dyn Fn(&Value) -> Option<String>;
pub type Ending<'a> = &'a dyn Fn(&Value) -> Markup;

/// `items` as a table of `action`'s columns; `link` turns an item into the
/// address its first cell links to, and `ending`, when given, into a last
/// cell, such as a form acting on it.
#[must_use]
pub fn table(action: &Action, items: &Value, link: Link, ending: Option<Ending>) -> Markup {
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
                                @let text = cell(column, &field(item, column));
                                @match link(item).filter(|_| index == 0) {
                                    Some(address) => a href=(address) { (text) },
                                    None => (text),
                                }
                            }
                        }
                        @if let Some(ending) = ending { td { (ending(item)) } }
                    }
                }
            }
        }
    }
}

/// A time in RFC 3339 to the minute.
#[must_use]
pub fn short_time(time: &str) -> String {
    time.get(..16).unwrap_or(time).replacen('T', " ", 1)
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

/// The groups on this machine, and the forms to create or join one.
#[must_use]
pub fn home(groups: &Value) -> Markup {
    let link = |item: &Value| item["name"].as_str().map(|name| format!("/g/{name}"));
    let body = html! {
        (table(action("group", "list"), groups, &link, None))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode("+alice/a b&c.txt"), "%2Balice/a%20b%26c.txt");
        assert_eq!(encode("é"), "%C3%A9");
    }

    #[test]
    fn suggestions_are_decided_together_and_one_file_also_at_another_path() {
        let both = deciding("cheapmo", "/back", &["s1", "s2"], None).into_string();
        assert_eq!(
            both.matches(r#"name="suggestions" value="s1 s2""#).count(),
            2,
            "{both}"
        );
        assert!(!both.contains("<details"), "{both}");
        let one = deciding("cheapmo", "/back", &["s1"], Some("a.txt")).into_string();
        assert!(
            one.contains(r#"<input type="text" name="to" value="a.txt""#),
            "{one}"
        );
    }
}
