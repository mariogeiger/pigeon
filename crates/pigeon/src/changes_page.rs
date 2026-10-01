//! The web UI's Changes page: every change between this machine and a
//! file's owner, one line each, by who must act. A request is a signed
//! patch waiting for its owner, a set-aside item the same patch unsigned:
//! the requests addressed to this member, to accept or refuse; what this
//! member's machines set aside, to request, restore under another name or
//! discard; what others' machines set aside, which anyone may resolve the
//! same ways; the requests waiting for others; and, on demand, those done.
//! Each line's difference opens on click.

use maud::{Markup, html};
use pigeon_core::clock::rfc3339;
use pigeon_core::path::GroupPath;
use serde_json::Value;

use crate::form::form;
use crate::group_pages::{file_link, fill};
use crate::pages::{Bar, action, layout, short_time};
use crate::render::cell;

/// Who must act on a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// This member, its owner, to accept or refuse it.
    ToYou,
    /// Someone else.
    Waiting,
    /// Nobody: it was applied or refused.
    Done,
}

/// Who must act on `request`, as `request list` gives it, for the member
/// `me`.
#[must_use]
pub fn section(request: &Value, me: &str) -> Section {
    let statement = &request["statement"];
    if request["applied"] == true || !request["decision"].is_null() {
        Section::Done
    } else if statement["owner"] == me && statement["mode"] == "propose" {
        Section::ToYou
    } else {
        Section::Waiting
    }
}

/// How many changes wait for `me` to act: the requests addressed to them
/// and the items their machines set aside.
#[must_use]
pub fn waiting(requests: &Value, aside: &Value, me: &str) -> usize {
    let listed = |list: &Value| list.as_array().cloned().unwrap_or_default();
    let to_you = listed(requests)
        .iter()
        .filter(|request| section(request, me) == Section::ToYou)
        .count();
    to_you
        + listed(aside)
            .iter()
            .filter(|item| item["member"] == me)
            .count()
}

/// One request with the difference each of its changes makes.
pub struct RequestCard {
    pub request: Value,
    pub changes: Vec<(String, Markup)>,
}

/// One set-aside item with the difference it would make.
pub struct AsideCard {
    pub item: Value,
    pub diff: Markup,
}

/// `path` linking to its page when it is one of the group's.
fn path_link(group: &str, path: &str) -> Markup {
    html! {
        @if GroupPath::parse(path).is_ok() {
            a href=(file_link(group, path)) { (path) }
        } @else {
            (path)
        }
    }
}

/// Why pigeon set an item of `member` aside, as a reason of `aside list`
/// says, for the member `me`.
fn reason(reason: &Value, member: &str, me: &str) -> String {
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
            .map(|(name, detail)| match name.as_str() {
                "Unportable" => format!("a name not every machine can hold: {}", cell("", detail)),
                "Rejected" => format!("the group rejected it: {}", cell("", detail)),
                _ => format!("{name}: {}", cell("", detail)),
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => cell("", other),
    }
}

/// One request's line: what it changes, who asks whom and when, the
/// buttons of `section`, and its differences, folded.
fn request_line(group: &str, back: &str, card: &RequestCard, section: Section) -> Markup {
    let request = &card.request;
    let statement = &request["statement"];
    let path = request["path"].as_str().unwrap_or_default();
    html! {
        li {
            div class="line" {
                span class="what" {
                    @for (index, (changed, _)) in card.changes.iter().enumerate() {
                        @if index > 0 { ", " }
                        (path_link(group, changed))
                    }
                    " · " (cell("", &request["author"])) " → " (cell("", &statement["owner"]))
                    " · " (short_time(request["time"].as_str().unwrap_or_default()))
                    @if statement["mode"] == "force" { " · forced" }
                    @if request["outdated"] == true { " · " span class="mark" { "based on an old version" } }
                    @if request["applied"] == true {
                        " · applied"
                    } @else if request["decision"] == "accept" {
                        " · accepted"
                    } @else if request["decision"] == "refuse" {
                        " · refused"
                    }
                }
                @if section == Section::ToYou {
                    (form(action("request", "accept"), back, fill(group, &[("request", path)], &[])))
                    (form(action("request", "refuse"), back, fill(group, &[("request", path)], &[])))
                }
            }
            details class="diff" {
                summary { "Difference" }
                @if let Some(message) = statement["message"].as_str().filter(|message| !message.is_empty()) {
                    blockquote { (message) }
                }
                @for (changed, diff) in &card.changes {
                    @if card.changes.len() > 1 { h4 { (changed) } }
                    (diff)
                }
            }
        }
    }
}

/// One set-aside item's line, for the member `me`: its path, whose, why
/// and when, the forms that resolve it, and its difference, folded.
fn aside_line(group: &str, back: &str, card: &AsideCard, me: &str) -> Markup {
    let item = &card.item;
    let file = item["file"].as_str().unwrap_or_default();
    let path = item["path"].as_str().unwrap_or_default();
    let member = item["member"].as_str().unwrap_or_default();
    let time = item["time"].as_u64().map(rfc3339).unwrap_or_default();
    html! {
        li {
            div class="line" {
                span class="what" {
                    (path_link(group, path))
                    @if member != me { " · " (member) }
                    " · " (reason(&item["reason"], member, me))
                    " · " (short_time(&time))
                }
                (form(action("aside", "request"), back, fill(group, &[("file", file)], &[])))
                (form(action("aside", "restore"), back, fill(group, &[("file", file)], &[("to", path)])))
                div data-confirm={ "Discard this edit of " (path) " for the whole group?" } {
                    (form(action("aside", "discard"), back, fill(group, &[("file", file)], &[])))
                }
            }
            details class="diff" {
                summary { "Difference" }
                (card.diff)
            }
        }
    }
}

/// The Changes page of `bar`'s group for the member `me`, with the
/// requests done when `done` is set.
#[must_use]
pub fn changes(
    bar: &Bar<'_>,
    me: &str,
    requests: &[RequestCard],
    aside: &[AsideCard],
    done: bool,
) -> Markup {
    let group = bar.group;
    let back = if done {
        format!("/g/{group}/changes?done")
    } else {
        format!("/g/{group}/changes")
    };
    let of = |wanted: Section| -> Vec<&RequestCard> {
        requests
            .iter()
            .rev()
            .filter(|card| section(&card.request, me) == wanted)
            .collect()
    };
    let (to_you, waiting, finished) = (of(Section::ToYou), of(Section::Waiting), of(Section::Done));
    let (yours, others): (Vec<&AsideCard>, Vec<&AsideCard>) =
        aside.iter().partition(|card| card.item["member"] == me);
    let body = html! {
        @if to_you.is_empty() && aside.is_empty() && waiting.is_empty() {
            p { "Nothing waits for anyone." }
        }
        @if !to_you.is_empty() {
            section {
                h2 { "To you" }
                p class="quiet" { "Changes others ask of your files: accept or refuse each." }
                ul class="changes" {
                    @for card in &to_you { (request_line(group, &back, card, Section::ToYou)) }
                }
            }
        }
        @if !yours.is_empty() {
            section {
                h2 { "Set aside, not sent" }
                p class="quiet" { "Edits your machines hold but may not publish as they are: request them from the owner, restore them under another name, or discard them." }
                ul class="changes" {
                    @for card in &yours { (aside_line(group, &back, card, me)) }
                }
            }
        }
        @if !others.is_empty() {
            section {
                h2 { "Set aside by others" }
                p class="quiet" { "Edits others' machines hold but may not publish as they are: anyone may resolve them for them, the same ways." }
                ul class="changes" {
                    @for card in &others { (aside_line(group, &back, card, me)) }
                }
            }
        }
        @if !waiting.is_empty() {
            section {
                h2 { "Waiting" }
                p class="quiet" { "Requests waiting for their owner." }
                ul class="changes" {
                    @for card in &waiting { (request_line(group, &back, card, Section::Waiting)) }
                }
            }
        }
        @if !finished.is_empty() {
            section {
                @if done {
                    h2 { "Done" }
                    ul class="changes" {
                        @for card in &finished { (request_line(group, &back, card, Section::Done)) }
                    }
                    p { a href={ "/g/" (group) "/changes" } { "Hide the requests done" } }
                } @else {
                    p { a href={ "/g/" (group) "/changes?done" } { "Show the " (finished.len()) " requests done" } }
                }
            }
        }
    };
    layout("Changes", Some(bar), &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(owner: &str, mode: &str, decision: &Value, applied: bool) -> Value {
        json!({
            "path": ".pigeon/requests/r.json", "author": "bob", "time": "2026-10-01T12:00:00Z",
            "statement": { "owner": owner, "mode": mode, "message": "" },
            "decision": decision, "applied": applied, "outdated": false,
        })
    }

    #[test]
    fn requests_go_to_whoever_must_act() {
        let mine = request("alice", "propose", &Value::Null, false);
        assert_eq!(section(&mine, "alice"), Section::ToYou);
        assert_eq!(section(&mine, "carol"), Section::Waiting);
        let forced = request("alice", "force", &Value::Null, false);
        assert_eq!(section(&forced, "alice"), Section::Waiting);
        let refused = request("alice", "propose", &json!("refuse"), false);
        assert_eq!(section(&refused, "alice"), Section::Done);
        let applied = request("alice", "force", &Value::Null, true);
        assert_eq!(section(&applied, "alice"), Section::Done);
        let requests = json!([mine, forced, refused]);
        let aside = json!([{ "member": "alice" }, { "member": "bob" }]);
        assert_eq!(waiting(&requests, &aside, "alice"), 2);
    }

    #[test]
    fn done_requests_show_only_on_demand_and_answers_only_to_their_owner() {
        let card = |request: Value| RequestCard {
            request,
            changes: vec![("+alice/a.txt".to_owned(), html! { "diff" })],
        };
        let requests = [
            card(request("alice", "propose", &Value::Null, false)),
            card(request("alice", "propose", &json!("accept"), true)),
        ];
        let bar = Bar {
            group: "g",
            tab: Some(crate::pages::Tab::Changes),
            waiting: 1,
        };
        let page = changes(&bar, "alice", &requests, &[], false).into_string();
        assert!(page.contains("To you"), "{page}");
        assert!(page.contains(r#"action="/act/request/accept""#), "{page}");
        assert!(page.contains("Show the 1 requests done"), "{page}");
        assert!(!page.contains("Set aside"), "{page}");
        let page = changes(&bar, "bob", &requests, &[], true).into_string();
        assert!(!page.contains(r#"action="/act/request/accept""#), "{page}");
        assert!(
            page.contains("Waiting") && page.contains("· applied"),
            "{page}"
        );
        let aside = [AsideCard {
            item: json!({
                "file": ".pigeon/aside/1.json", "member": "alice", "path": "+bob/x.txt",
                "reason": "NotWritable", "time": 0,
            }),
            diff: html! {},
        }];
        let page = changes(&bar, "alice", &[], &aside, false).into_string();
        assert!(
            page.contains("Set aside, not sent") && page.contains("you may not write it"),
            "{page}"
        );
        assert!(page.contains(r#"action="/act/aside/discard""#), "{page}");
        assert!(page.contains(".pigeon/aside/1.json"), "{page}");
        let page = changes(&bar, "carol", &[], &aside, false).into_string();
        assert!(
            page.contains("Set aside by others") && page.contains("alice may not write it"),
            "{page}"
        );
        assert!(page.contains(r#"action="/act/aside/restore""#), "{page}");
    }
}
