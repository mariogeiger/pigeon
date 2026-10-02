//! The page that asks the question of a call the daemon refused unless
//! confirmed: a form that makes the same call again with `yes`, and a way
//! back.

use maud::{Markup, html};
use serde_json::{Map, Value};

use crate::form::BACK;
use crate::pages::layout;

/// The page asking `question` of the call `noun verb` with `values`,
/// which goes back to `back` either way.
#[must_use]
pub fn confirmation(
    noun: &str,
    verb: &str,
    values: &Map<String, Value>,
    question: &str,
    back: &str,
) -> Markup {
    let body = html! {
        p { (question) }
        form class="action" method="post" action={ "/act/" (noun) "/" (verb) } enctype="multipart/form-data" {
            input type="hidden" name=(BACK) value=(back);
            @for (name, value) in values {
                input type="hidden" name=(name) value=(value.as_str().unwrap_or_default());
            }
            input type="hidden" name="yes" value="true";
            button type="submit" { "Go ahead" }
            " " a href=(back) { "Cancel" }
        }
    };
    layout("Are you sure?", None, &body)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_question_comes_with_a_form_that_repeats_the_call_with_yes() {
        let values = json!({"group": "cheapmo", "path": "a&b.txt"});
        let page = confirmation(
            "file",
            "delete",
            values.as_object().unwrap(),
            "Delete a&b.txt?",
            "/g/cheapmo",
        )
        .into_string();
        assert!(page.contains("<p>Delete a&amp;b.txt?</p>"), "{page}");
        assert!(page.contains(r#"action="/act/file/delete""#), "{page}");
        assert!(
            page.contains(r#"name="path" value="a&amp;b.txt""#),
            "{page}"
        );
        assert!(page.contains(r#"name="back" value="/g/cheapmo""#), "{page}");
        assert!(page.contains(r#"name="yes" value="true""#), "{page}");
        assert!(
            page.contains(r#"<a href="/g/cheapmo">Cancel</a>"#),
            "{page}"
        );
    }
}
