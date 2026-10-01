//! The statuses a row of the Files page shows, one emoji each, with the
//! legend that explains them: whether this machine is catching up with a
//! followed file or keeps a frozen copy, whether changes become requests,
//! the edits and drafts waiting with the time left, and the drafts of one
//! path that rival each other. A file up to date, or neither followed nor
//! held, shows nothing.

use maud::{Markup, html};
use serde_json::Value;

/// One status of a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Updating,
    Frozen,
    ByRequest,
    Waiting,
    Deleting,
    Drafted,
    Rival,
    Overtaken,
}

impl Status {
    pub const ALL: [Self; 8] = [
        Self::Updating,
        Self::Frozen,
        Self::ByRequest,
        Self::Waiting,
        Self::Deleting,
        Self::Drafted,
        Self::Rival,
        Self::Overtaken,
    ];

    #[must_use]
    pub fn emoji(self) -> &'static str {
        match self {
            Self::Updating => "⏬",
            Self::Frozen => "🧊",
            Self::ByRequest => "🔒",
            Self::Waiting => "⏳",
            Self::Deleting => "🗑️",
            Self::Drafted => "✍️",
            Self::Rival => "⚠️",
            Self::Overtaken => "🛑",
        }
    }

    #[must_use]
    pub fn meaning(self) -> &'static str {
        match self {
            Self::Updating => {
                "followed, but missing or older here: the current version is on its way"
            }
            Self::Frozen => "a copy kept here, frozen: it no longer syncs",
            Self::ByRequest => {
                "changes become requests: another member's file, or a published drop file"
            }
            Self::Waiting => "your edit waits to be published",
            Self::Deleting => "your deletion waits to be published",
            Self::Drafted => "another member is adding this file",
            Self::Rival => "another draft of the same path: the first published wins",
            Self::Overtaken => {
                "another draft of the same path is published first: rename yours to keep it"
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
/// those published first say that this one will be set aside.
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
                    Some(format!("{author} also adds {path} and publishes first: your copy will be set aside, rename it to keep both")),
                    &html! { (author) " " (countdown(&rival["due_in"])) }))
            } @else {
                (mark(Status::Rival,
                    Some(format!("{author} also adds {path}: the first published wins")),
                    &html! { (author) " " (countdown(&rival["due_in"])) }))
            }
        }
    }
}

/// The statuses of the file at one path: its published version `file`,
/// the edit of it `waiting` here, and the `drafts` of it other machines
/// announced.
#[must_use]
pub fn file_status(file: Option<&Value>, waiting: Option<&Value>, drafts: &[&Value]) -> Markup {
    let empty = html! {};
    html! {
        @if let Some(file) = file {
            @let pinned = file["cutoff"]["At"].is_u64();
            @let held = file["held"] == true;
            @if file["cutoff"] == "PlusInfinity" && (!held || file["outdated"] == true) {
                (mark(Status::Updating, None, &empty))
            }
            @if pinned && held { (mark(Status::Frozen, None, &empty)) }
            @if !pinned && file["writable"] == false { (mark(Status::ByRequest, None, &empty)) }
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

/// The edits waiting in a folder.
#[must_use]
pub fn folder_status(waiting: usize) -> Markup {
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
        let file = |cutoff: Value, held: bool, outdated: bool, writable: bool| json!({"cutoff": cutoff, "held": held, "outdated": outdated, "writable": writable});
        let followed = json!("PlusInfinity");
        let pinned = json!({"At": 5});
        assert_eq!(shown(&file(followed.clone(), true, false, true)), "");
        assert_eq!(shown(&file(json!("MinusInfinity"), false, false, true)), "");
        assert!(shown(&file(followed.clone(), false, false, true)).contains("⏬"));
        assert!(shown(&file(followed.clone(), true, true, true)).contains("⏬"));
        assert!(shown(&file(pinned.clone(), true, true, false)).contains("🧊"));
        assert!(!shown(&file(pinned.clone(), true, true, false)).contains("🔒"));
        assert_eq!(shown(&file(pinned, false, false, false)), "");
        assert!(shown(&file(followed, true, false, false)).contains("🔒"));
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
        assert_eq!(folder_status(0).into_string(), "");
        assert!(folder_status(3).into_string().contains("⏳ 3"));
        assert_eq!(
            legend().into_string().matches("<li>").count(),
            Status::ALL.len()
        );
    }
}
