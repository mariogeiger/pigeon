//! The edits waiting to be published: this machine's, and the drafts that
//! other machines announce, each with the other drafts of the same path,
//! whatever its case, and which of them are published first, which sets
//! this one aside. The engine announces this machine's drafts, the
//! new files in drop folders, whenever one appears, changes or goes.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use pigeon_core::draft::{Draft, Drafts, SignedDrafts};
use pigeon_core::name::MemberName;
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::selection::Cutoff;
use serde::Serialize;

use crate::engine::{Engine, Inner, Pending, Work};

/// How long past its due time an announced draft is still shown, while its
/// machine publishes it and announces that it is gone.
const LINGER: Duration = Duration::from_secs(60);

/// Another draft of the same path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rival {
    pub author: MemberName,
    pub path: GroupPath,
    pub due_in: u64,
    /// Whether it is published no later, which sets the other aside.
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
    /// Whether publishing freezes the file, as in a drop folder.
    pub freezes: bool,
    /// The cutoff the selection gives the file.
    pub cutoff: Cutoff,
    /// The other drafts of the same path, whatever its case.
    pub rivals: Vec<Rival>,
}

impl PendingView {
    /// Whether it is a draft: a new file in a drop folder.
    fn is_draft(&self) -> bool {
        self.freezes && !self.deleted
    }
}

/// Tells each draft of `views` the other drafts of its path, and which of
/// them are published no later.
fn mark_rivals(views: &mut [PendingView]) {
    let mut by_key: BTreeMap<PathKey, Vec<usize>> = BTreeMap::new();
    for (index, view) in views.iter().enumerate() {
        if view.is_draft() {
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
    /// Whether the edit `pending` is a draft, which other machines learn of.
    fn is_draft(&self, pending: &Pending) -> bool {
        pending.stat.is_some() && self.ledger.lock().freezes(&pending.path)
    }

    /// Announces this machine's drafts, signed, when they changed since
    /// they were last announced.
    pub(crate) fn announce_drafts(&self, work: &mut Work) {
        let mut drafts: Vec<&Pending> = work
            .pending
            .values()
            .filter(|pending| self.is_draft(pending))
            .collect();
        drafts.sort_by(|a, b| a.path.cmp(&b.path));
        let announced: Vec<(PathKey, u64, Instant)> = drafts
            .iter()
            .map(|pending| {
                let size = pending.stat.as_ref().map_or(0, |stat| stat.size);
                (pending.path.key(), size, pending.since)
            })
            .collect();
        if work.announced.as_ref() == Some(&announced) {
            return;
        }
        let drafts = Drafts {
            machine: self.me(),
            drafts: drafts
                .iter()
                .map(|pending| Draft {
                    path: pending.path.clone(),
                    size: pending.stat.as_ref().map_or(0, |stat| stat.size),
                    due_in: left(self.settle_time(&pending.path), pending.since.elapsed()),
                })
                .collect(),
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
        let within =
            |path: &GroupPath| under.is_none_or(|under| path.key().is_within(&under.key()));
        let mut views: Vec<PendingView> = work
            .pending
            .values()
            .map(|pending| PendingView {
                path: pending.path.clone(),
                author: inner.member.clone(),
                here: true,
                deleted: pending.stat.is_none(),
                size: pending.stat.as_ref().map_or(0, |stat| stat.size),
                due_in: left(inner.settle_time(&pending.path), pending.since.elapsed()),
                freezes: inner.ledger.lock().freezes(&pending.path),
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
                    freezes: true,
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

    fn view(path: &str, author: &str, due_in: u64, freezes: bool) -> PendingView {
        PendingView {
            path: GroupPath::parse(path).unwrap(),
            author: MemberName::parse(author).unwrap(),
            here: author == "alice",
            deleted: false,
            size: 1,
            due_in,
            freezes,
            cutoff: Cutoff::PlusInfinity,
            rivals: Vec::new(),
        }
    }

    #[test]
    fn drafts_of_one_path_in_any_case_are_rivals_and_the_later_is_set_aside() {
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
