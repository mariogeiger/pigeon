//! The web UI's forms, generated from the catalog: one input per argument
//! by its kind, with the arguments a page already knows filled in.

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
                Kind::Flag => input type="checkbox" name=(name) value="true";
                Kind::Bytes => input type="file" name=(name) required[required];
                Kind::Document => textarea name=(name) rows="6" { (value) }
                Kind::Choice(choices) => select name=(name) {
                    @for choice in choices {
                        option value=(choice) selected[*choice == value] { (choice) }
                    }
                },
                Kind::Text | Kind::Path | Kind::Pattern | Kind::Folder | Kind::Time => {
                    input type="text" name=(name) value=(value) required[required];
                }
            }
        }
    }
}

/// The form that runs `action` and then returns to `back`. A form with
/// nothing left to ask is a single button.
#[must_use]
pub fn form(action: &Action, back: &str, fill: Fill<'_>) -> Markup {
    let visible: Vec<&Param> = action
        .params
        .iter()
        .filter(|param| lookup(fill.fixed, param.name).is_none())
        .collect();
    let target = format!("/act/{}/{}", action.noun, action.verb);
    let group = fill.group.filter(|_| action.scope == Scope::Group);
    let body = html! {
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
            button type="submit" title=(action.about) { (action.verb) }
        }
    };
    if visible.is_empty() {
        body
    } else {
        html! {
            details class="action" {
                summary { (action.about) }
                (body)
            }
        }
    }
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
            defaults: &[("mode", "force")],
        };
        let markup = form(write, "/g/cheapmo", fill).into_string();
        assert!(markup.contains(r#"action="/act/file/write""#));
        assert!(markup.contains(r#"<input type="hidden" name="group" value="cheapmo">"#));
        assert!(markup.contains(r#"<input type="hidden" name="path" value="+alice/a.txt">"#));
        assert!(markup.contains(r#"<input type="file" name="content" required>"#));
        assert!(markup.contains(r#"<option value="force" selected>"#));
        let accept = find("request", "accept").unwrap();
        let fill = Fill {
            fixed: &[("request", ".pigeon/requests/x.json")],
            ..Fill::default()
        };
        assert!(!form(accept, "/", fill).into_string().contains("<details"));
    }
}
