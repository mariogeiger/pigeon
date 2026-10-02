//! The statuses a row of the Files page shows, one emoji each, with the
//! legend that explains them: whether this machine is catching up with a
//! followed file or keeps a pinned copy, the edits and drafts waiting with
//! the time left, the drafts of one path that rival each other, and the
//! changes suggested to the group. A file up to date, or neither followed
//! nor held, shows nothing.

use maud::{Markup, html};
use serde_json::Value;

/// One status of a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Updating,
    Pinned,
    Waiting,
    Deleting,
    Drafted,
    Rival,
    Overtaken,
    Suggested,
}

impl Status {
    pub const ALL: [Self; 8] = [
        Self::Updating,
        Self::Pinned,
        Self::Waiting,
        Self::Deleting,
        Self::Drafted,
        Self::Rival,
        Self::Overtaken,
        Self::Suggested,
    ];

    #[must_use]
    pub fn emoji(self) -> &'static str {
        match self {
            Self::Updating => "⏬",
            Self::Pinned => "📌",
            Self::Waiting => "⏳",
            Self::Deleting => "🗑️",
            Self::Drafted => "✍️",
            Self::Rival => "⚠️",
            Self::Overtaken => "🛑",
            Self::Suggested => "📬",
        }
    }

    #[must_use]
    pub fn meaning(self) -> &'static str {
        match self {
            Self::Updating => {
                "followed, but missing or older here: the current version is on its way"
            }
            Self::Pinned => "a copy kept here, pinned at a time: it no longer syncs",
            Self::Waiting => "your edit waits to be published",
            Self::Deleting => "your deletion waits to be published",
            Self::Drafted => "another member is adding this file",
            Self::Rival => {
                "another draft of the same path: the first published wins, the other becomes a suggestion"
            }
            Self::Overtaken => {
                "another draft of the same path is published first: yours becomes a suggestion, unless you rename it"
            }
            Self::Suggested => {
                "a change suggested to the group: anyone validates or discards it from its menu"
            }
        }
    }
}

/// Seconds as minutes and seconds.
#[must_use]
pub fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A countdown that `live.js` ticks.
#[must_use]
pub fn countdown(seconds: &Value) -> Markup {
    let seconds = seconds.as_u64().unwrap_or_default();
    html! { span data-due=(seconds) { (clock(seconds)) } }
}

/// `status` as its emoji followed by `detail`, explained on hover by
/// `title`, or by its meaning.
fn mark(status: Status, title: Option<String>, detail: &Markup) -> Markup {
    html! {
        span class="status" title=(title.unwrap_or_else(|| status.meaning().to_owned())) {
            (status.emoji()) " " (detail)
        }
    }
}

/// The other drafts of the path `item` waits at; for this machine's own,
/// those published first say that this one will become a suggestion.
fn rivals(item: &Value) -> Markup {
    let rivals = item["rivals"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let here = item["here"] == true;
    html! {
        @for rival in rivals {
            @let author = rival["author"].as_str().unwrap_or_default();
            @let path = rival["path"].as_str().unwrap_or_default();
            @if here && rival["wins"] == true {
                (mark(Status::Overtaken,
                    Some(format!("{author} also adds {path} and publishes first: yours will become a suggestion, rename it to keep both")),
                    &html! { (author) " " (countdown(&rival["due_in"])) }))
            } @else {
                (mark(Status::Rival,
                    Some(format!("{author} also adds {path}: the first published wins")),
                    &html! { (author) " " (countdown(&rival["due_in"])) }))
            }
        }
    }
}

/// The mode the selection gives the file `item` describes, as the first
/// word of a rule's line: follow, pin or free.
#[must_use]
pub fn mode(item: &Value) -> &str {
    let cutoff = item["cutoff"].as_str().unwrap_or_default();
    cutoff.split(' ').next().unwrap_or_default()
}

/// The statuses of the file at one path: its published version `file`,
/// the edit of it `waiting` here, and the `drafts` of it other machines
/// announced.
#[must_use]
pub fn file_status(file: Option<&Value>, waiting: Option<&Value>, drafts: &[&Value]) -> Markup {
    let empty = html! {};
    html! {
        @if let Some(file) = file {
            @let held = file["held"] == true;
            @if mode(file) == "follow" && (!held || file["outdated"] == true) {
                (mark(Status::Updating, None, &empty))
            }
            @if mode(file) == "pin" && held { (mark(Status::Pinned, None, &empty)) }
        }
        @if let Some(waiting) = waiting {
            @let status = if waiting["deleted"] == true { Status::Deleting } else { Status::Waiting };
            (mark(status, None, &countdown(&waiting["due_in"])))
            (rivals(waiting))
        } @else {
            @for draft in drafts {
                @let author = draft["author"].as_str().unwrap_or_default();
                (mark(Status::Drafted, Some(format!("{author} is adding this file")),
                    &html! { (author) " " (countdown(&draft["due_in"])) }))
                (rivals(draft))
            }
        }
    }
}

/// What the change `change` of `suggestion`, as `suggestion list` gives
/// them, does and why it waits.
#[must_use]
pub fn suggestion_title(suggestion: &Value, change: &Value) -> String {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let since = if change["outdated"] == true {
        "; the file changed since"
    } else {
        ""
    };
    format!(
        "{} suggests {}, as {}{since}",
        text(&suggestion["author"]),
        text(&change["what"]),
        text(&suggestion["reason"])
    )
}

/// The suggested changes of one path, each with its suggestion.
#[must_use]
pub fn suggested_status<'a>(suggested: impl IntoIterator<Item = (&'a Value, &'a Value)>) -> Markup {
    html! {
        @for (suggestion, change) in suggested {
            (mark(Status::Suggested, Some(suggestion_title(suggestion, change)),
                &html! { (suggestion["author"].as_str().unwrap_or_default()) }))
        }
    }
}

/// The edits waiting to be published in a folder, and the changes
/// suggested in it.
#[must_use]
pub fn folder_status(waiting: usize, suggested: usize) -> Markup {
    html! {
        @if waiting > 0 {
            @let title = if waiting == 1 {
                "1 edit waits to be published here".to_owned()
            } else {
                format!("{waiting} edits wait to be published here")
            };
            (mark(Status::Waiting, Some(title),
                &html! { (waiting) }))
        }
        @if suggested > 0 {
            @let title = if suggested == 1 {
                "1 change is suggested here".to_owned()
            } else {
                format!("{suggested} changes are suggested here")
            };
            (mark(Status::Suggested, Some(title), &html! { (suggested) }))
        }
    }
}

/// What each emoji means.
#[must_use]
pub fn legend() -> Markup {
    html! {
        ul class="legend" {
            @for status in Status::ALL {
                li { (status.emoji()) " " (status.meaning()) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shown(file: &Value) -> String {
        file_status(Some(file), None, &[]).into_string()
    }

    #[test]
    fn a_file_shows_only_what_departs_from_being_in_sync() {
        let file = |cutoff: Value, held: bool, outdated: bool| json!({"cutoff": cutoff, "held": held, "outdated": outdated});
        let followed = json!("follow");
        let pinned = json!("pin 2026-10-01T12:00:00Z");
        assert_eq!(mode(&file(pinned.clone(), true, false)), "pin");
        assert_eq!(shown(&file(followed.clone(), true, false)), "");
        assert_eq!(shown(&file(json!("free"), false, false)), "");
        assert!(shown(&file(followed.clone(), false, false)).contains("⏬"));
        assert!(shown(&file(followed, true, true)).contains("⏬"));
        assert!(shown(&file(pinned.clone(), true, true)).contains("📌"));
        assert_eq!(shown(&file(pinned, false, false)), "");
    }

    #[test]
    fn waiting_edits_drafts_and_rivals_show_with_the_time_left() {
        let waiting = json!({"here": true, "due_in": 200, "deleted": false,
            "rivals": [{"author": "bob", "path": "a/X", "due_in": 42, "wins": true}]});
        let mine = file_status(None, Some(&waiting), &[]).into_string();
        assert!(
            mine.contains(r#"⏳ <span data-due="200">3:20</span>"#),
            "{mine}"
        );
        assert!(
            mine.contains(r#"🛑 bob <span data-due="42">0:42</span>"#),
            "{mine}"
        );
        assert!(mine.contains("rename it to keep both"));
        let draft = json!({"here": false, "author": "bob", "due_in": 42,
            "rivals": [{"author": "alice", "path": "a/x", "due_in": 200, "wins": false}]});
        let theirs = file_status(None, None, &[&draft]).into_string();
        assert!(
            theirs.contains(r#"✍️ bob <span data-due="42">0:42</span>"#),
            "{theirs}"
        );
        assert!(theirs.contains("⚠️ alice") && !theirs.contains("🛑"));
        let gone = json!({"here": true, "due_in": 3, "deleted": true});
        assert!(
            file_status(None, Some(&gone), &[])
                .into_string()
                .contains("🗑️")
        );
        assert_eq!(folder_status(0, 0).into_string(), "");
        assert!(folder_status(3, 0).into_string().contains("⏳ 3"));
        assert!(folder_status(0, 2).into_string().contains("📬 2"));
        assert_eq!(
            legend().into_string().matches("<li>").count(),
            Status::ALL.len()
        );
    }

    #[test]
    fn a_suggestion_says_who_suggests_what_and_why() {
        let suggestion = json!({"author": "papy", "reason": "the rules leave it to the group"});
        let version = json!({"path": "a", "what": "a new version", "outdated": false});
        let shown = suggested_status([(&suggestion, &version)]).into_string();
        assert!(shown.contains("📬 papy"), "{shown}");
        assert!(
            shown.contains("papy suggests a new version, as the rules leave it to the group"),
            "{shown}"
        );
        let deletion = json!({"path": "a", "what": "a deletion", "outdated": true});
        assert_eq!(
            suggestion_title(&suggestion, &deletion),
            "papy suggests a deletion, as the rules leave it to the group; the file changed since"
        );
    }
}
