//! The web UI's forms, generated from the catalog: one input per argument
//! by its kind, with the arguments a page already knows filled in, folded
//! behind the action's description on a page, or asked at once above a
//! note on what it does in a dialog.

use maud::{Markup, html};

use crate::catalog::{Action, GROUP, Kind, Param, Scope};

/// The field that tells the form handler which page to return to.
pub const BACK: &str = "back";

/// What a page knows of a form's arguments.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fill<'a> {
    /// The group the form acts on.
    pub group: Option<&'a str>,
    /// Arguments set by the page and not shown.
    pub fixed: &'a [(&'a str, &'a str)],
    /// Arguments shown with a value to start from.
    pub defaults: &'a [(&'a str, &'a str)],
}

fn lookup<'a>(pairs: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, value)| *value)
}

fn input(param: &Param, value: Option<&str>) -> Markup {
    let name = param.name;
    let value = value.unwrap_or_default();
    let required = param.required;
    html! {
        label {
            span { (param.about) }
            @match param.kind {
                Kind::Flag => input type="checkbox" name=(name) value="true" checked[value == "true"];
                Kind::Bytes => input type="file" name=(name) required[required];
                Kind::Document => textarea name=(name) rows="6" required[required] { (value) }
                Kind::Text | Kind::Path | Kind::Pattern | Kind::Folder | Kind::Time => {
                    input type="text" name=(name) value=(value) required[required];
                }
            }
        }
    }
}

/// The label of the button that runs `action`: its verb, capitalized.
fn label(action: &Action) -> String {
    let mut chars = action.verb.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The form that runs `action` and then returns to `back`, with `note`
/// above its button, and whether it asks for anything.
fn asked(action: &Action, back: &str, fill: Fill<'_>, note: Option<&str>) -> (Markup, bool) {
    let visible: Vec<&Param> = action
        .params
        .iter()
        .filter(|param| lookup(fill.fixed, param.name).is_none())
        .collect();
    let target = format!("/act/{}/{}", action.noun, action.verb);
    let group = fill.group.filter(|_| action.scope == Scope::Group);
    let markup = html! {
        form class="action" method="post" action=(target) enctype="multipart/form-data" {
            input type="hidden" name=(BACK) value=(back);
            @if let Some(group) = group {
                input type="hidden" name=(GROUP.name) value=(group);
            }
            @for (name, value) in fill.fixed {
                input type="hidden" name=(name) value=(value);
            }
            @for param in &visible {
                (input(param, lookup(fill.defaults, param.name)))
            }
            @if let Some(note) = note { p { (note) } }
            button type="submit" title=(action.about) { (label(action)) }
        }
    };
    (markup, !visible.is_empty())
}

/// The form that runs `action` and then returns to `back`, folded behind
/// the action's description when it asks for anything, else a single
/// button.
#[must_use]
pub fn form(action: &Action, back: &str, fill: Fill<'_>) -> Markup {
    match asked(action, back, fill, None) {
        (markup, false) => markup,
        (markup, true) => html! {
            details class="action" {
                summary { (action.about) }
                (markup)
            }
        },
    }
}

/// The form that runs `action` and then returns to `back`, asking at once
/// for what the page does not know, then saying `note` above its button.
#[must_use]
pub fn asking(action: &Action, back: &str, fill: Fill<'_>, note: &str) -> Markup {
    asked(action, back, fill, Some(note)).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::find;

    #[test]
    fn forms_ask_for_what_the_page_does_not_know() {
        let write = find("file", "write").unwrap();
        let fill = Fill {
            group: Some("cheapmo"),
            fixed: &[("path", "+alice/a.txt")],
            defaults: &[],
        };
        let markup = form(write, "/g/cheapmo", fill).into_string();
        assert!(markup.contains(r#"action="/act/file/write""#));
        assert!(markup.contains(r#"<input type="hidden" name="group" value="cheapmo">"#));
        assert!(markup.contains(r#"<input type="hidden" name="path" value="+alice/a.txt">"#));
        assert!(markup.contains(r#"<input type="file" name="content" required>"#));
        let rename = find("file", "rename").unwrap();
        let fill = Fill {
            group: Some("cheapmo"),
            fixed: &[("from", "a")],
            defaults: &[("to", "b")],
        };
        let markup = form(rename, "/", fill).into_string();
        assert!(markup.contains(r#"<input type="text" name="to" value="b" required>"#));
        let follow = find("selection", "follow").unwrap();
        let fill = Fill {
            fixed: &[("pattern", "/docs/")],
            ..Fill::default()
        };
        let button = form(follow, "/", fill).into_string();
        assert!(!button.contains("<details"));
        assert!(button.contains(">Follow</button>"), "{button}");
        let asked = asking(rename, "/", Fill::default(), "It publishes at once.").into_string();
        assert!(!asked.contains("<details"), "{asked}");
        assert!(
            asked.contains("<p>It publishes at once.</p><button"),
            "{asked}"
        );
    }

    #[test]
    fn a_flag_starts_checked_and_a_document_may_be_required() {
        let flag = Param {
            name: "force",
            about: "Force",
            kind: Kind::Flag,
            required: false,
            listed_by: None,
        };
        assert!(input(&flag, Some("true")).into_string().contains("checked"));
        assert!(!input(&flag, None).into_string().contains("checked"));
        let document = Param {
            name: "text",
            about: "Text",
            kind: Kind::Document,
            required: true,
            listed_by: None,
        };
        assert!(input(&document, None).into_string().contains("required"));
    }
}
