//! The edits waiting to be published or suggested: this machine's, and the
//! drafts that other machines announce, each with the other drafts of the
//! same path, whatever its case, and which of them are published first,
//! which makes this one a suggestion. The engine announces this machine's
//! drafts, the new files at paths no member owns, whenever one appears,
//! changes or goes: those published soonest, as many as one announcement
//! carries.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use pigeon_core::draft::{Draft, Drafts, SignedDrafts};
use pigeon_core::name::MemberName;
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::selection::Cutoff;
use serde::Serialize;

use crate::batches::{BATCH_BYTES, batches, encoded_size};
use crate::engine::{Engine, Inner, Pending, Work};
use crate::views::as_text;

/// How long past its due time an announced draft is still shown, while its
/// machine publishes it and announces that it is gone.
const LINGER: Duration = Duration::from_secs(60);

/// Another draft of the same path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rival {
    pub author: MemberName,
    pub path: GroupPath,
    pub due_in: u64,
    /// Whether it is published no later, which makes the other a
    /// suggestion.
    pub wins: bool,
}

/// An edit waiting to stay unchanged long enough to be published.
#[derive(Clone, Debug, Serialize)]
pub struct PendingView {
    pub path: GroupPath,
    /// The member whose machine holds the edit.
    pub author: MemberName,
    /// Whether this machine holds it, rather than another that announced it.
    pub here: bool,
    /// Whether the edit deletes the file.
    pub deleted: bool,
    /// The bytes it holds.
    pub size: u64,
    /// Seconds until it is published, if it stays unchanged.
    pub due_in: u64,
    /// Whether it is a draft, a new file at a path no member owns, which
    /// other machines learn of and which waits longer.
    pub draft: bool,
    /// The mode the selection gives the file, as a rule's line spells it.
    #[serde(serialize_with = "as_text")]
    pub cutoff: Cutoff,
    /// The other drafts of the same path, whatever its case.
    pub rivals: Vec<Rival>,
}

/// Tells each draft of `views` the other drafts of its path, and which of
/// them are published no later.
fn mark_rivals(views: &mut [PendingView]) {
    let mut by_key: BTreeMap<PathKey, Vec<usize>> = BTreeMap::new();
    for (index, view) in views.iter().enumerate() {
        if view.draft {
            by_key.entry(view.path.key()).or_default().push(index);
        }
    }
    for indices in by_key.into_values() {
        for &index in &indices {
            let rivals: Vec<Rival> = indices
                .iter()
                .filter(|&&other| other != index)
                .map(|&other| Rival {
                    author: views[other].author.clone(),
                    path: views[other].path.clone(),
                    due_in: views[other].due_in,
                    wins: views[other].due_in <= views[index].due_in,
                })
                .collect();
            views[index].rivals = rivals;
        }
    }
}

/// Seconds left of `settle` once `elapsed` passed.
fn left(settle: Duration, elapsed: Duration) -> u64 {
    settle.saturating_sub(elapsed).as_secs()
}

impl Inner {
    /// Announces this machine's drafts published soonest, as many as one
    /// announcement carries, signed, when they changed since they were
    /// last announced.
    pub(crate) fn announce_drafts(&self, work: &mut Work) {
        let mut drafts: Vec<(&Pending, Draft)> = work
            .pending
            .values()
            .filter(|pending| self.is_draft(pending))
            .map(|pending| {
                let draft = Draft {
                    path: pending.path.clone(),
                    size: pending.size(),
                    due_in: left(self.settle_time(pending), pending.since.elapsed()),
                };
                (pending, draft)
            })
            .collect();
        drafts.sort_by(|(a, _), (b, _)| (a.since, &a.path).cmp(&(b.since, &b.path)));
        let drafts = batches(drafts, |(_, draft)| encoded_size(draft), BATCH_BYTES)
            .into_iter()
            .next()
            .unwrap_or_default();
        let announced: Vec<(PathKey, u64, Instant)> = drafts
            .iter()
            .map(|(pending, _)| (pending.path.key(), pending.size(), pending.since))
            .collect();
        if work.announced.as_ref() == Some(&announced) {
            return;
        }
        let drafts = Drafts {
            machine: self.me(),
            drafts: drafts.into_iter().map(|(_, draft)| draft).collect(),
        };
        let signed = SignedDrafts::sign(&self.group, drafts, self.cert.clone(), &self.machine);
        self.node.announce(signed);
        work.announced = Some(announced);
    }
}

impl Engine {
    /// The edits waiting to be published at `under` or inside it, or
    /// everywhere: this machine's and those other machines announced, by
    /// path.
    pub async fn pending(&self, under: Option<&GroupPath>) -> Vec<PendingView> {
        let inner = &self.inner;
        let work = inner.work.lock().await;
        let within = |path: &GroupPath| under.is_none_or(|under| path.is_within(under));
        let mut views: Vec<PendingView> = work
            .pending
            .values()
            .map(|pending| PendingView {
                path: pending.path.clone(),
                author: inner.member.clone(),
                here: true,
                deleted: pending.stat.is_none(),
                size: pending.size(),
                due_in: left(inner.settle_time(pending), pending.since.elapsed()),
                draft: inner.is_draft(pending),
                cutoff: work.config.selection.cutoff(&pending.path),
                rivals: Vec::new(),
            })
            .collect();
        for announced in inner.node.announced().into_values() {
            let elapsed = announced.at.elapsed();
            for draft in announced.drafts {
                let due = Duration::from_secs(draft.due_in);
                if elapsed > due + LINGER {
                    continue;
                }
                views.push(PendingView {
                    cutoff: work.config.selection.cutoff(&draft.path),
                    path: draft.path,
                    author: announced.author.clone(),
                    here: false,
                    deleted: false,
                    size: draft.size,
                    due_in: left(due, elapsed),
                    draft: true,
                    rivals: Vec::new(),
                });
            }
        }
        mark_rivals(&mut views);
        views.retain(|view| within(&view.path));
        views.sort_by(|a, b| (&a.path, !a.here, &a.author).cmp(&(&b.path, !b.here, &b.author)));
        views
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(path: &str, author: &str, due_in: u64, draft: bool) -> PendingView {
        PendingView {
            path: GroupPath::parse(path).unwrap(),
            author: MemberName::parse(author).unwrap(),
            here: author == "alice",
            deleted: false,
            size: 1,
            due_in,
            draft,
            cutoff: Cutoff::PlusInfinity,
            rivals: Vec::new(),
        }
    }

    #[test]
    fn drafts_of_one_path_in_any_case_are_rivals_and_the_later_is_suggested() {
        let mut views = [
            view("inbox/Report.txt", "alice", 200, true),
            view("inbox/report.TXT", "bob", 40, true),
            view("inbox/other.txt", "carol", 10, true),
            view("+alice/report.txt", "alice", 2, false),
        ];
        mark_rivals(&mut views);
        assert_eq!(views[0].rivals.len(), 1);
        assert_eq!(views[0].rivals[0].author.as_str(), "bob");
        assert_eq!(views[0].rivals[0].due_in, 40);
        assert!(views[0].rivals[0].wins);
        assert_eq!(views[1].rivals[0].path.as_str(), "inbox/Report.txt");
        assert!(!views[1].rivals[0].wins);
        assert!(views[2].rivals.is_empty());
        assert!(views[3].rivals.is_empty());
    }
}
