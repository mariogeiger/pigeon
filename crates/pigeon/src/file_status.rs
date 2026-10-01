//! The statuses a row of the Files page shows, one emoji each, with the
//! legend that explains them: whether this machine is catching up with a
//! followed file or keeps a frozen copy, whether changes become requests,
//! the edits and drafts waiting with the time left, the drafts of one
//! path that rival each other, and the changes waiting for someone. A file
//! up to date, or neither followed nor held, shows nothing.

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
    Change,
}

impl Status {
    pub const ALL: [Self; 9] = [
        Self::Updating,
        Self::Frozen,
        Self::ByRequest,
        Self::Waiting,
        Self::Deleting,
        Self::Drafted,
        Self::Rival,
        Self::Overtaken,
        Self::Change,
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
            Self::Change => "📬",
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
            Self::Change => {
                "a change waits for someone: its menu applies it, asks its owner, places it elsewhere or discards it"
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

/// Why pigeon set aside an item of `member`, as a reason of `change list`
/// says, for the member `me`.
#[must_use]
pub fn reason(reason: &Value, member: &str, me: &str) -> String {
    let (who, whose) = if member == me {
        ("you".to_owned(), "your".to_owned())
    } else {
        (member.to_owned(), format!("{member}'s"))
    };
    match reason {
        Value::String(name) if name == "NotWritable" => format!("{who} may not write it"),
        Value::String(name) if name == "Superseded" => {
            format!("another of {whose} machines changed it meanwhile")
        }
        Value::Object(fields) => fields
            .iter()
            .map(|(name, detail)| {
                let detail = detail.as_str().unwrap_or_default();
                match name.as_str() {
                    "Unportable" => format!("a name not every machine can hold: {detail}"),
                    "Rejected" => format!("the group rejected it: {detail}"),
                    _ => format!("{name}: {detail}"),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

/// What `change`, as `change list` gives it, is and who must act, for the
/// member `me`.
#[must_use]
pub fn change_title(change: &Value, me: &str) -> String {
    let author = change["author"].as_str().unwrap_or_default();
    let owner = change["owner"].as_str().unwrap_or_default();
    let what = if change["content"].is_null() {
        "deletion"
    } else {
        "change"
    };
    let message = change["message"]
        .as_str()
        .filter(|message| !message.is_empty());
    let said = message
        .map(|message| format!(": {message}"))
        .unwrap_or_default();
    match &change["waits"] {
        Value::String(waits) if waits == "Proposed" => {
            format!("{author} proposes a {what} to {owner}{said}")
        }
        Value::String(_) => format!("{author}'s {what} is on its way to {owner}'s machines{said}"),
        waits => format!(
            "set aside on {author}'s machine, as {}",
            reason(&waits["SetAside"]["reason"], author, me)
        ),
    }
}

/// The changes waiting at one path, for the member `me`.
#[must_use]
pub fn changes_status(changes: &[&Value], me: &str) -> Markup {
    html! {
        @for change in changes {
            @let author = change["author"].as_str().unwrap_or_default();
            @let owner = change["owner"].as_str().unwrap_or_default();
            (mark(Status::Change, Some(change_title(change, me)),
                &html! { @if author == owner { (author) } @else { (author) " → " (owner) } }))
        }
    }
}

/// The edits waiting to be published in a folder, and the changes waiting
/// for someone.
#[must_use]
pub fn folder_status(waiting: usize, changes: usize) -> Markup {
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
        @if changes > 0 {
            @let title = if changes == 1 {
                "1 change waits for someone here".to_owned()
            } else {
                format!("{changes} changes wait for someone here")
            };
            (mark(Status::Change, Some(title), &html! { (changes) }))
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
        assert_eq!(folder_status(0, 0).into_string(), "");
        assert!(folder_status(3, 0).into_string().contains("⏳ 3"));
        assert!(folder_status(0, 2).into_string().contains("📬 2"));
        assert_eq!(
            legend().into_string().matches("<li>").count(),
            Status::ALL.len()
        );
    }

    #[test]
    fn a_waiting_change_says_who_asks_whom_and_why() {
        let proposal = json!({"author": "bob", "owner": "alice", "content": {"size": 1},
            "message": "bread", "waits": "Proposed"});
        let shown = changes_status(&[&proposal], "alice").into_string();
        assert!(shown.contains("📬 bob → alice"), "{shown}");
        assert!(
            shown.contains("bob proposes a change to alice: bread"),
            "{shown}"
        );
        let aside = json!({"author": "papy", "owner": "alice", "content": {"size": 1},
            "message": "", "waits": {"SetAside": {"reason": "NotWritable", "machine": "m"}}});
        assert_eq!(
            change_title(&aside, "alice"),
            "set aside on papy's machine, as papy may not write it"
        );
        assert_eq!(
            change_title(&aside, "papy"),
            "set aside on papy's machine, as you may not write it"
        );
        let unportable = json!({"Unportable": "a colon"});
        assert_eq!(
            reason(&unportable, "papy", "alice"),
            "a name not every machine can hold: a colon"
        );
    }
}
