//! The ledger: every known patch, folded in stamp order into the accepted
//! versions of each path and their lineage across moves, the member list
//! with the key each name is bound to, and the names that tags claim.
//! Each patch is accepted or rejected as a whole against the state it lands
//! on, so every machine holding the same patches computes the same tree.
//! Any member changes any file: the ledger guards who speaks for a name and
//! that a suggestion is decided once, and leaves the rest to the group.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;

use iroh_base::PublicKey;

use crate::clock::{MachineId, Stamp};
use crate::identity::{GroupId, MachineCert};
use crate::name::MemberName;
use crate::ownership::{Ownership, classify};
use crate::patch::{Change, Content, SignatureError, SignedPatch, VersionRef};
use crate::path::{GroupPath, PathKey};
use crate::statement::{is_suggestion_path, member_of_path, member_path};

/// One accepted state of a path, the member who wrote it, and the
/// versions it replaces and, when it moved here, continues.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Version {
    pub stamp: Stamp,
    pub path: GroupPath,
    pub content: Option<Content>,
    pub author: MemberName,
    pub replaces: Option<Stamp>,
    pub continues: Option<VersionRef>,
}

impl Version {
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.content.is_some()
    }

    /// The reference other changes name this version by.
    #[must_use]
    pub fn reference(&self) -> VersionRef {
        VersionRef {
            path: self.path.clone(),
            stamp: self.stamp,
        }
    }
}

/// A name, bound to a member key by its earliest valid claim.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Member {
    pub key: PublicKey,
    pub joined: Stamp,
}

/// Why a patch cannot be accepted, phrased for the person who made it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error("{0} is not a member of this group")]
    UnknownMember(MemberName),
    #[error("the name {0} is taken by another key: choose another name")]
    OtherKey(MemberName),
    #[error("a path tagged +{0} already claims the name {0}: choose another name")]
    ClaimedByTag(MemberName),
    #[error("{path} is {owner}'s member file: only {owner} writes it")]
    ForeignMemberFile { path: GroupPath, owner: MemberName },
    #[error("the suggestion {0} was decided already, or changed since you saw it")]
    AlreadyDecided(GroupPath),
}

/// What accepting a patch changed, so that it can be undone exactly.
#[derive(Clone, Debug, Default)]
struct Effects {
    keys: Vec<PathKey>,
    claims: Vec<(MemberName, isize)>,
    joined: Option<MemberName>,
}

#[derive(Clone, Debug)]
enum Outcome {
    Accepted(Effects),
    Rejected(Rejection),
}

/// The folded state: every accepted version, members, and tag claims.
#[derive(Default)]
struct State {
    outcomes: BTreeMap<Stamp, Outcome>,
    files: HashMap<PathKey, Vec<Version>>,
    members: BTreeMap<MemberName, Member>,
    claims: HashMap<MemberName, usize>,
}

/// What an acceptable patch does: whether it joins its author, and the
/// changes that make versions.
struct Plan<'a> {
    joins: bool,
    changes: Vec<&'a Change>,
}

impl State {
    fn head(&self, key: &PathKey) -> Option<&Version> {
        self.files.get(key).and_then(|versions| versions.last())
    }

    fn check_author(
        &self,
        name: &MemberName,
        key: &PublicKey,
        changes: &[Change],
    ) -> Result<bool, Rejection> {
        if let Some(member) = self.members.get(name) {
            return if member.key == *key {
                Ok(false)
            } else {
                Err(Rejection::OtherKey(name.clone()))
            };
        }
        let own_file = member_path(name).key();
        let joins = changes
            .iter()
            .any(|change| change.path.key() == own_file && change.content.is_some());
        if !joins {
            return Err(Rejection::UnknownMember(name.clone()));
        }
        if self.claims.get(name).copied().unwrap_or(0) > 0 {
            return Err(Rejection::ClaimedByTag(name.clone()));
        }
        Ok(true)
    }

    fn plan<'a>(
        &self,
        author: &MemberName,
        key: &PublicKey,
        changes: &'a [Change],
    ) -> Result<Plan<'a>, Rejection> {
        let joins = self.check_author(author, key, changes)?;
        let mut planned = Vec::new();
        for change in changes {
            if let Some(owner) = member_of_path(&change.path)
                && owner != *author
            {
                return Err(Rejection::ForeignMemberFile {
                    path: change.path.clone(),
                    owner,
                });
            }
            let live = self
                .head(&change.path.key())
                .filter(|version| version.is_live());
            if change.content.is_none() {
                let decides_what_was_seen =
                    live.is_some_and(|head| Some(head.stamp) == change.replaces);
                if is_suggestion_path(&change.path) && !decides_what_was_seen {
                    return Err(Rejection::AlreadyDecided(change.path.clone()));
                }
                if live.is_none() {
                    continue;
                }
            }
            planned.push(change);
        }
        Ok(Plan {
            joins,
            changes: planned,
        })
    }

    fn fold(&mut self, signed: &SignedPatch) {
        let patch = &signed.patch;
        let cert = &signed.cert;
        let outcome = match self.plan(&cert.name, &cert.member, &patch.changes) {
            Err(rejection) => Outcome::Rejected(rejection),
            Ok(plan) => {
                let mut effects = Effects::default();
                if plan.joins {
                    self.members.insert(
                        cert.name.clone(),
                        Member {
                            key: cert.member,
                            joined: patch.stamp,
                        },
                    );
                    effects.joined = Some(cert.name.clone());
                }
                for change in plan.changes {
                    let key = change.path.key();
                    let was_live = self.head(&key).is_some_and(Version::is_live);
                    let delta = isize::from(change.content.is_some()) - isize::from(was_live);
                    if delta != 0 {
                        let members = &self.members;
                        for name in classify(&change.path, |name| members.contains_key(name)).1 {
                            let count = self.claims.entry(name.clone()).or_default();
                            *count = count.saturating_add_signed(delta);
                            effects.claims.push((name, delta));
                        }
                    }
                    self.files.entry(key.clone()).or_default().push(Version {
                        stamp: patch.stamp,
                        path: change.path.clone(),
                        content: change.content,
                        author: cert.name.clone(),
                        replaces: change.replaces,
                        continues: change.continues.clone(),
                    });
                    effects.keys.push(key);
                }
                Outcome::Accepted(effects)
            }
        };
        self.outcomes.insert(patch.stamp, outcome);
    }

    fn undo(&mut self, stamp: &Stamp) {
        let Some(Outcome::Accepted(effects)) = self.outcomes.remove(stamp) else {
            return;
        };
        for key in effects.keys {
            if let Some(versions) = self.files.get_mut(&key) {
                versions.pop();
                if versions.is_empty() {
                    self.files.remove(&key);
                }
            }
        }
        for (name, delta) in effects.claims {
            let count = self.claims.entry(name.clone()).or_default();
            *count = count.saturating_add_signed(-delta);
            if *count == 0 {
                self.claims.remove(&name);
            }
        }
        if let Some(name) = effects.joined {
            self.members.remove(&name);
        }
    }
}

/// Every patch a machine knows, and the tree they fold into.
pub struct Ledger {
    group: GroupId,
    patches: BTreeMap<Stamp, SignedPatch>,
    by_machine: BTreeSet<(MachineId, u64)>,
    state: State,
}

impl Ledger {
    #[must_use]
    pub fn new(group: GroupId) -> Self {
        Self {
            group,
            patches: BTreeMap::new(),
            by_machine: BTreeSet::new(),
            state: State::default(),
        }
    }

    #[must_use]
    pub fn group(&self) -> &GroupId {
        &self.group
    }

    /// Adds a patch, folding it and every later patch again. Returns the
    /// stamps whose outcome was computed anew, empty if the patch was known.
    ///
    /// # Errors
    /// Refuses a patch whose signatures do not hold; it is not stored.
    pub fn insert(&mut self, signed: SignedPatch) -> Result<Vec<Stamp>, SignatureError> {
        let stamp = signed.stamp();
        if self.patches.contains_key(&stamp) {
            return Ok(Vec::new());
        }
        signed.verify(&self.group)?;
        self.patches.insert(stamp, signed);
        self.by_machine.insert((stamp.machine, stamp.time));
        let later: Vec<Stamp> = self
            .state
            .outcomes
            .range((Bound::Excluded(stamp), Bound::Unbounded))
            .map(|(later, _)| *later)
            .collect();
        for later in later.iter().rev() {
            self.state.undo(later);
        }
        let mut folded = vec![stamp];
        folded.extend(later);
        for stamp in &folded {
            self.state.fold(&self.patches[stamp]);
        }
        Ok(folded)
    }

    /// Checks what the current state says of a patch `author` would make now.
    ///
    /// # Errors
    /// Returns why the patch would be rejected.
    pub fn check(
        &self,
        author: &MemberName,
        key: &PublicKey,
        changes: &[Change],
    ) -> Result<(), Rejection> {
        self.state.plan(author, key, changes).map(|_| ())
    }

    /// The member whose personal path `path` lies in, if any.
    #[must_use]
    pub fn owner(&self, path: &GroupPath) -> Option<MemberName> {
        match classify(path, |name| self.state.members.contains_key(name)).0 {
            Ownership::Personal(owner) => Some(owner),
            Ownership::Drop => None,
        }
    }

    /// Whether `rejection` is the outcome of the patch `stamp`, or `Ok` if
    /// the patch was accepted; `None` when the patch is unknown.
    #[must_use]
    pub fn outcome(&self, stamp: &Stamp) -> Option<Result<(), &Rejection>> {
        self.state.outcomes.get(stamp).map(|outcome| match outcome {
            Outcome::Accepted(_) => Ok(()),
            Outcome::Rejected(rejection) => Err(rejection),
        })
    }

    #[must_use]
    pub fn patch(&self, stamp: &Stamp) -> Option<&SignedPatch> {
        self.patches.get(stamp)
    }

    /// Every patch in stamp order.
    pub fn patches(&self) -> impl Iterator<Item = &SignedPatch> {
        self.patches.values()
    }

    /// The last version of `key`, live or deleted.
    #[must_use]
    pub fn head(&self, key: &PathKey) -> Option<&Version> {
        self.state.head(key)
    }

    /// Every accepted version of `key`, oldest first.
    #[must_use]
    pub fn versions(&self, key: &PathKey) -> &[Version] {
        self.state.files.get(key).map_or(&[], Vec::as_slice)
    }

    /// The last version of `key` dated up to `time`.
    #[must_use]
    pub fn version_at(&self, key: &PathKey, time: u64) -> Option<&Version> {
        let versions = self.versions(key);
        let after = versions.partition_point(|version| version.stamp.time <= time);
        after.checked_sub(1).map(|index| &versions[index])
    }

    /// The version `reference` names, if it was accepted.
    #[must_use]
    pub fn version(&self, reference: &VersionRef) -> Option<&Version> {
        let versions = self.versions(&reference.path.key());
        versions
            .binary_search_by_key(&reference.stamp, |version| version.stamp)
            .ok()
            .map(|index| &versions[index])
    }

    /// The version `version` follows in its file's history: the one it
    /// continues when it moved, or else the previous version of its path.
    #[must_use]
    pub fn parent(&self, version: &Version) -> Option<&Version> {
        if let Some(continued) = &version.continues {
            return self
                .version(continued)
                .filter(|parent| parent.stamp < version.stamp);
        }
        let versions = self.versions(&version.path.key());
        let at = versions.partition_point(|earlier| earlier.stamp < version.stamp);
        at.checked_sub(1).map(|index| &versions[index])
    }

    /// The history of the file at `key`, newest first: its head and every
    /// version it descends from, across the paths it moved from.
    #[must_use]
    pub fn lineage(&self, key: &PathKey) -> Vec<&Version> {
        std::iter::successors(self.head(key), |version| self.parent(version)).collect()
    }

    /// Every path key that ever had a version.
    pub fn keys(&self) -> impl Iterator<Item = &PathKey> {
        self.state.files.keys()
    }

    /// The current version of every file that exists.
    pub fn live(&self) -> impl Iterator<Item = &Version> {
        self.state
            .files
            .values()
            .filter_map(|versions| versions.last())
            .filter(|version| version.is_live())
    }

    #[must_use]
    pub fn members(&self) -> &BTreeMap<MemberName, Member> {
        &self.state.members
    }

    /// The names that a live path's tag claims while no member holds them,
    /// under which nobody may join.
    pub fn tag_claims(&self) -> impl Iterator<Item = &MemberName> {
        self.state
            .claims
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(name, _)| name)
    }

    /// Whether `cert` names the member key its name is bound to: whether
    /// the member list recognizes the machine it vouches for.
    #[must_use]
    pub fn recognizes(&self, cert: &MachineCert) -> bool {
        self.state
            .members
            .get(&cert.name)
            .is_some_and(|member| member.key == cert.member)
    }

    /// The versions of `key` that a later version by another machine
    /// replaced without having seen them: the losers of concurrent changes.
    #[must_use]
    pub fn unseen_versions(&self, key: &PathKey) -> Vec<&Version> {
        self.versions(key)
            .windows(2)
            .filter(|pair| {
                pair[1].replaces != Some(pair[0].stamp)
                    && pair[1].stamp.machine != pair[0].stamp.machine
            })
            .map(|pair| &pair[0])
            .collect()
    }

    /// The latest time known from each machine.
    #[must_use]
    pub fn vector(&self) -> BTreeMap<MachineId, u64> {
        let mut vector = BTreeMap::new();
        for (machine, time) in &self.by_machine {
            vector.insert(*machine, *time);
        }
        vector
    }

    /// Every patch that `vector` does not cover.
    #[must_use]
    pub fn missing_from(&self, vector: &BTreeMap<MachineId, u64>) -> Vec<&SignedPatch> {
        let mut missing: Vec<&SignedPatch> = Vec::new();
        let machines: BTreeSet<MachineId> = self
            .by_machine
            .iter()
            .map(|(machine, _)| *machine)
            .collect();
        for machine in machines {
            let after = vector
                .get(&machine)
                .map_or(Bound::Included((machine, 0)), |time| {
                    Bound::Excluded((machine, *time))
                });
            for (_, time) in self
                .by_machine
                .range((after, Bound::Included((machine, u64::MAX))))
            {
                missing.push(
                    &self.patches[&Stamp {
                        time: *time,
                        machine,
                    }],
                );
            }
        }
        missing.sort_by_key(|signed| signed.stamp());
        missing
    }
}

#[cfg(test)]
#[path = "ledger_tests.rs"]
mod tests;
