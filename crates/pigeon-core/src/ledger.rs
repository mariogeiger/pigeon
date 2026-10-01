//! The ledger: every known patch, folded in stamp order into the accepted
//! versions of each path, the member list with each name's current key, and
//! the names that folders claim.
//! Each patch is accepted or rejected as a whole against the state it lands
//! on, so every machine holding the same patches computes the same tree.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;

use iroh_base::PublicKey;

use crate::clock::{MachineId, Stamp};
use crate::folder::{Folder, classify};
use crate::identity::{GroupId, MachineCert};
use crate::name::MemberName;
use crate::patch::{Change, Content, SignatureError, SignedPatch};
use crate::path::{GroupPath, PathKey};
use crate::statement::{
    RebindStatement, is_rebind_path, member_of_path, member_path, rebind_of_path,
};

/// One accepted state of a path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Version {
    pub stamp: Stamp,
    pub path: GroupPath,
    pub content: Option<Content>,
    pub owner: MemberName,
    pub replaces: Option<Stamp>,
    pub applies: Option<GroupPath>,
}

impl Version {
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.content.is_some()
    }
}

/// A name, bound to a member key by its earliest valid claim and since
/// then by its last rebinding, or to none once the member is excluded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Member {
    pub key: Option<PublicKey>,
    pub joined: Stamp,
    pub rebound: Option<Rebinding>,
}

/// The patch that last rebound a name, and who signed it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rebinding {
    pub stamp: Stamp,
    pub by: MemberName,
}

/// Why a patch cannot be accepted, phrased for the person who made it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    #[error(transparent)]
    Signature(#[from] SignatureError),
    #[error("{0} is not a member of this group")]
    UnknownMember(MemberName),
    #[error(
        "the name {0} is taken: another password made it, so choose another name or retype yours"
    )]
    OtherKey(MemberName),
    #[error("a folder @{0} already claims the name {0}: choose another name")]
    ClaimedByFolder(MemberName),
    #[error("{0} no longer belongs to this group")]
    Excluded(MemberName),
    #[error("{0} must be named <stamp>-<key or none> in a member's rebinding folder")]
    Malformed(GroupPath),
    #[error("{path} is {owner}'s member file: only {owner} writes it")]
    ForeignMemberFile { path: GroupPath, owner: MemberName },
    #[error("{path} belongs to {owner}: propose the change as a request")]
    NotOwner { path: GroupPath, owner: MemberName },
    #[error("{0} is frozen: change it through a request, which you may force")]
    Frozen(GroupPath),
}

/// What accepting a patch changed, so that it can be undone exactly.
#[derive(Clone, Debug, Default)]
struct Effects {
    keys: Vec<PathKey>,
    claims: Vec<(MemberName, isize)>,
    joined: Option<MemberName>,
    rebound: Vec<(MemberName, Member)>,
    excluded: bool,
}

#[derive(Clone, Debug)]
enum Outcome {
    Accepted(Effects),
    Rejected(Rejection),
}

/// The folded state: every accepted version, members, and folder claims.
#[derive(Default)]
struct State {
    outcomes: BTreeMap<Stamp, Outcome>,
    files: HashMap<PathKey, Vec<Version>>,
    members: BTreeMap<MemberName, Member>,
    claims: HashMap<MemberName, usize>,
    exclusions: BTreeSet<Stamp>,
}

/// One validated change and the member who owns its path.
struct Planned<'a> {
    change: &'a Change,
    owner: MemberName,
}

/// What an acceptable patch does: whether it joins its author, its changes,
/// and the names it rebinds.
struct Plan<'a> {
    joins: bool,
    changes: Vec<Planned<'a>>,
    rebinds: Vec<RebindStatement>,
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
            return match member.key {
                Some(bound) if bound == *key => Ok(false),
                Some(_) => Err(Rejection::OtherKey(name.clone())),
                None => Err(Rejection::Excluded(name.clone())),
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
            return Err(Rejection::ClaimedByFolder(name.clone()));
        }
        Ok(true)
    }

    fn plan<'a>(
        &self,
        author: &MemberName,
        key: &PublicKey,
        changes: &'a [Change],
        applies: bool,
    ) -> Result<Plan<'a>, Rejection> {
        let joins = self.check_author(author, key, changes)?;
        let is_member =
            |name: &MemberName| self.members.contains_key(name) || (joins && name == author);
        let mut planned = Vec::new();
        let mut rebinds = Vec::new();
        for change in changes {
            if is_rebind_path(&change.path) && change.content.is_some() {
                let rebind = rebind_of_path(&change.path)
                    .ok_or_else(|| Rejection::Malformed(change.path.clone()))?;
                if !is_member(&rebind.name) {
                    return Err(Rejection::UnknownMember(rebind.name));
                }
                rebinds.push(rebind);
            }
            if let Some(owner) = member_of_path(&change.path)
                && owner != *author
            {
                return Err(Rejection::ForeignMemberFile {
                    path: change.path.clone(),
                    owner,
                });
            }
            let head = self
                .head(&change.path.key())
                .filter(|version| version.is_live());
            let owner = match (classify(&change.path, is_member).0, head) {
                (Folder::Personal(owner), _) => owner,
                (Folder::Drop, Some(head)) => {
                    if !applies && head.owner == *author {
                        return Err(Rejection::Frozen(change.path.clone()));
                    }
                    head.owner.clone()
                }
                (Folder::Drop, None) if change.content.is_none() => continue,
                (Folder::Drop, None) => author.clone(),
            };
            if owner != *author {
                return Err(Rejection::NotOwner {
                    path: change.path.clone(),
                    owner,
                });
            }
            planned.push(Planned { change, owner });
        }
        Ok(Plan {
            joins,
            changes: planned,
            rebinds,
        })
    }

    fn fold(&mut self, signed: &SignedPatch) {
        let patch = &signed.patch;
        let cert = &signed.cert;
        let outcome = match self.plan(
            &cert.name,
            &cert.member,
            &patch.changes,
            patch.applies.is_some(),
        ) {
            Err(rejection) => Outcome::Rejected(rejection),
            Ok(plan) => {
                let mut effects = Effects::default();
                if plan.joins {
                    self.members.insert(
                        cert.name.clone(),
                        Member {
                            key: Some(cert.member),
                            joined: patch.stamp,
                            rebound: None,
                        },
                    );
                    effects.joined = Some(cert.name.clone());
                }
                for Planned { change, owner } in plan.changes {
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
                        owner,
                        replaces: change.replaces,
                        applies: patch.applies.clone(),
                    });
                    effects.keys.push(key);
                }
                for rebind in plan.rebinds {
                    let member = self
                        .members
                        .get_mut(&rebind.name)
                        .expect("the plan checked that the name is a member");
                    effects.rebound.push((rebind.name, member.clone()));
                    member.key = rebind.key;
                    member.rebound = Some(Rebinding {
                        stamp: patch.stamp,
                        by: cert.name.clone(),
                    });
                    if rebind.key.is_none() {
                        effects.excluded = true;
                    }
                }
                if effects.excluded {
                    self.exclusions.insert(patch.stamp);
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
        for (name, previous) in effects.rebound.into_iter().rev() {
            self.members.insert(name, previous);
        }
        if effects.excluded {
            self.exclusions.remove(stamp);
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
        applies: bool,
    ) -> Result<(), Rejection> {
        self.state.plan(author, key, changes, applies).map(|_| ())
    }

    /// The owner a new file at `path` would have if `author` created it now.
    #[must_use]
    pub fn owner_of(&self, path: &GroupPath, author: &MemberName) -> MemberName {
        let head = self.head(&path.key()).filter(|version| version.is_live());
        match (
            classify(path, |name| self.state.members.contains_key(name)).0,
            head,
        ) {
            (Folder::Personal(owner), _) => owner,
            (Folder::Drop, Some(head)) => head.owner.clone(),
            (Folder::Drop, None) => author.clone(),
        }
    }

    /// Whether files at `path` freeze once published.
    #[must_use]
    pub fn freezes(&self, path: &GroupPath) -> bool {
        classify(path, |name| self.state.members.contains_key(name)).0 == Folder::Drop
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

    /// Whether `cert` names a member key its name is bound to now: whether
    /// the member list recognizes the machine it vouches for.
    #[must_use]
    pub fn recognizes(&self, cert: &MachineCert) -> bool {
        self.state
            .members
            .get(&cert.name)
            .is_some_and(|member| member.key == Some(cert.member))
    }

    /// The stamp of the last accepted patch that excluded a member, after
    /// which the group secret must be new.
    #[must_use]
    pub fn last_exclusion(&self) -> Option<Stamp> {
        self.state.exclusions.last().copied()
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
