//! Suggestions: changes the rules leave to the group, each held by a
//! statement in the suggestions folder until anyone validates it, which
//! publishes its changes, or discards it; deleting the statement decides
//! it, and only the first decision counts. This machine suggests what its
//! disk holds outside the rules, keeps it on disk until the decision, and
//! keeps the live suggestions parsed, with the contents they give the
//! paths its selection follows, for anyone here to compare.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use pigeon_core::clock::{MachineId, Stamp};
use pigeon_core::ledger::Version;
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Change, Content, VersionRef};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::selection::Cutoff;
use pigeon_core::statement::{
    Reason, SuggestedChange, Suggestion, is_statement, is_suggestion_path, suggestion_path,
};
use pigeon_store::disk::{self, fs_path};
use pigeon_store::index::{IndexEntry, hash_file};
use pigeon_store::state::Kept;
use serde::Serialize;

use crate::disk_sync::file_stat;
use crate::engine::{Engine, Inner, Work};
use crate::views::as_text;

/// A live suggestion: its statement's version and what it says.
#[derive(Clone, Debug)]
pub(crate) struct Live {
    pub version: Version,
    pub suggestion: Suggestion,
}

/// A suggestion as anyone may decide it, its statement and reason
/// serialized as text.
#[derive(Clone, Debug, Serialize)]
pub struct SuggestionView {
    /// The version of its statement this view shows, which deciding names,
    /// and interfaces by its text, as its id.
    #[serde(rename = "id", serialize_with = "as_text")]
    pub statement: VersionRef,
    pub author: MemberName,
    pub machine: MachineId,
    pub time: String,
    #[serde(serialize_with = "as_text")]
    pub reason: Reason,
    /// Whether validating it may place it at another path.
    pub placeable: bool,
    pub changes: Vec<SuggestedChangeView>,
}

/// One change of a suggestion.
#[derive(Clone, Debug, Serialize)]
pub struct SuggestedChangeView {
    /// The path it changes, not always portable.
    pub path: String,
    /// What it is, as text: a new file, a new version, a move from a path
    /// or a deletion.
    pub what: String,
    /// The content it gives the path, or `None` for a deletion.
    pub content: Option<Content>,
    pub replaces: Option<Stamp>,
    /// The version it moves, if it moves one.
    pub continues: Option<VersionRef>,
    /// Whether its path's current version is another than the one it
    /// replaces.
    pub outdated: bool,
}

/// What `change` is, as text.
fn what(change: &SuggestedChange) -> String {
    match (&change.content, &change.continues) {
        (None, _) => "a deletion".to_owned(),
        (Some(_), Some(moved)) => format!("a move from {}", moved.path),
        (Some(_), None) if change.replaces.is_none() => "a new file".to_owned(),
        (Some(_), None) => "a new version".to_owned(),
    }
}

/// What deciding suggestions does with their changes.
#[derive(Clone, Copy)]
enum Decision<'a> {
    /// Publishes them, the one content change of a single suggestion at
    /// the path given.
    Validate(Option<&'a GroupPath>),
    /// Publishes none; the history keeps them.
    Discard,
}

/// What a suggested path is compared by: its key when it is portable.
fn path_key(path: &str) -> String {
    GroupPath::parse(path).map_or_else(|_| path.to_owned(), |path| path.key().as_str().to_owned())
}

/// Whether `suggestion` holds exactly `changes`, by path and content.
fn holds(suggestion: &Suggestion, changes: &[SuggestedChange]) -> bool {
    suggestion.changes.len() == changes.len()
        && changes.iter().all(|change| {
            suggestion.changes.iter().any(|held| {
                path_key(&held.path) == path_key(&change.path) && held.content == change.content
            })
        })
}

/// Whether `suggestion` changes one of the paths `changes` do.
fn touches(suggestion: &Suggestion, changes: &[SuggestedChange]) -> bool {
    suggestion.changes.iter().any(|held| {
        changes
            .iter()
            .any(|change| path_key(&held.path) == path_key(&change.path))
    })
}

impl Inner {
    /// Brings the parsed suggestions in line with the live ones, fetching
    /// those not here yet and the contents they give followed paths, then
    /// lets the disk show the group's version wherever it kept a
    /// suggestion that someone decided.
    pub(crate) async fn follow_suggestions(self: &Arc<Self>, work: &mut Work) {
        if let Err(error) = self.parse_suggestions(work).await {
            self.report(format!("reading the suggestions: {error:#}"));
        }
        match self.decided_kept() {
            Ok(keys) => self.refresh_keys(work, &keys).await,
            Err(error) => self.report(format!("following the decided suggestions: {error:#}")),
        }
    }

    async fn parse_suggestions(self: &Arc<Self>, work: &mut Work) -> Result<()> {
        let live: Vec<Version> = self
            .ledger
            .lock()
            .live()
            .filter(|version| is_suggestion_path(&version.path))
            .cloned()
            .collect();
        work.suggestions.retain(|key, held| {
            live.iter()
                .any(|version| version.path.key() == *key && version.stamp == held.version.stamp)
        });
        for version in live {
            let key = version.path.key();
            let Some(content) = version
                .content
                .filter(|_| !work.suggestions.contains_key(&key))
            else {
                continue;
            };
            if !self.blobs.has(&content.hash).await? {
                self.fetch(work, content.hash, version.stamp.machine, key);
                continue;
            }
            match self.read_statement::<Suggestion>(&content).await {
                Ok(suggestion) => {
                    for change in &suggestion.changes {
                        let (Some(content), Ok(path)) =
                            (change.content, GroupPath::parse(&change.path))
                        else {
                            continue;
                        };
                        if work.config.selection.cutoff(&path) == Cutoff::PlusInfinity
                            && !self.blobs.has(&content.hash).await?
                        {
                            self.fetch(work, content.hash, version.stamp.machine, path.key());
                        }
                    }
                    work.suggestions.insert(
                        key,
                        Live {
                            version,
                            suggestion,
                        },
                    );
                }
                Err(error) => self.report(format!("{}: {error:#}", version.path)),
            }
        }
        Ok(())
    }

    /// The keys where the disk keeps a suggestion someone decided, and
    /// those its validation placed elsewhere, which the disk holds from
    /// then on; a file no portable path names goes at once, unless it
    /// changed since.
    fn decided_kept(&self) -> Result<Vec<PathKey>> {
        let mut keys = Vec::new();
        for (path, kept) in self.state.kept()? {
            let decided = !self
                .ledger
                .lock()
                .head(&kept.statement.key())
                .is_some_and(Version::is_live);
            if !decided {
                continue;
            }
            let portable = GroupPath::parse(&path).ok();
            if let Some(placed) = self.placed(&kept)
                && portable
                    .as_ref()
                    .is_none_or(|path| path.key() != placed.key())
            {
                let key = placed.key();
                if self.state.index_entry(&key)?.is_none() {
                    let held = IndexEntry {
                        path: placed,
                        seen: None,
                        synced: None,
                    };
                    self.state.update_index([(&key, Some(&held))])?;
                }
                keys.push(key);
            }
            if let Some(path) = portable {
                keys.push(path.key());
                continue;
            }
            let location = self.root.join(&path);
            if let Some(content) = kept.content
                && hash_file(&location).is_ok_and(|hash| hash == content.hash)
            {
                disk::remove(&self.root, &location)?;
            }
            self.state.unkeep(&path)?;
        }
        Ok(keys)
    }

    /// Where the patch that decided the suggestion `kept` names put the
    /// content the disk kept, if it did.
    fn placed(&self, kept: &Kept) -> Option<GroupPath> {
        let ledger = self.ledger.lock();
        let decision = ledger.head(&kept.statement.key())?;
        let content = kept.content?;
        ledger
            .patch(&decision.stamp)?
            .patch
            .changes
            .iter()
            .find(|change| change.content == Some(content))
            .map(|change| change.path.clone())
    }

    /// Suggests `changes` for `reason` and records that the disk keeps
    /// their contents until someone decides: a live suggestion that holds
    /// them already is enough, and one of this machine's that changes one
    /// of their paths takes them in its next version.
    pub(crate) async fn suggest(
        &self,
        work: &mut Work,
        changes: Vec<SuggestedChange>,
        reason: Reason,
    ) -> Result<()> {
        let held = work
            .suggestions
            .values()
            .find(|live| holds(&live.suggestion, &changes))
            .map(|live| live.version.path.clone());
        let statement = if let Some(statement) = held {
            statement
        } else {
            let me = self.me();
            let mine = work.suggestions.values().find(|live| {
                live.version.stamp.machine == me && touches(&live.suggestion, &changes)
            });
            let stamp = self.clock.stamp();
            let (path, mut merged) = match mine {
                Some(live) => (
                    live.version.path.clone(),
                    live.suggestion
                        .changes
                        .iter()
                        .filter(|held| {
                            !changes
                                .iter()
                                .any(|change| path_key(&held.path) == path_key(&change.path))
                        })
                        .cloned()
                        .collect(),
                ),
                None => (suggestion_path(&stamp), Vec::new()),
            };
            merged.extend(changes.iter().cloned());
            let suggestion = Suggestion {
                changes: merged,
                reason,
            };
            let body = serde_json::to_vec_pretty(&suggestion)?;
            self.publish_statement(work, stamp, path.clone(), body)
                .await?;
            if let Some(version) = self.ledger.lock().head(&path.key()).cloned() {
                work.suggestions.insert(
                    path.key(),
                    Live {
                        version,
                        suggestion,
                    },
                );
            }
            path
        };
        for change in &changes {
            let kept = Kept {
                statement: statement.clone(),
                content: change.content,
            };
            self.state.keep(&change.path, &kept)?;
        }
        work.protect_due = true;
        Ok(())
    }

    /// What the version `shown` of a suggestion says.
    async fn shown_suggestion(&self, work: &Work, shown: &VersionRef) -> Result<Suggestion> {
        if let Some(live) = work.suggestions.get(&shown.path.key())
            && live.version.stamp == shown.stamp
        {
            return Ok(live.suggestion.clone());
        }
        let version = self.ledger.lock().version(shown).cloned();
        match version.and_then(|version| version.content) {
            Some(content) if is_suggestion_path(&shown.path) => self.read_statement(&content).await,
            _ => bail!("no suggestion at {}", shown.path),
        }
    }

    /// Fails unless `to` is free, here and in the ledger, outside the
    /// statements folder.
    fn ensure_free(&self, to: &GroupPath) -> Result<()> {
        if is_statement(&to.key()) {
            bail!("{to} lies in the statements folder");
        }
        let live = self
            .ledger
            .lock()
            .head(&to.key())
            .is_some_and(Version::is_live);
        if live || file_stat(&fs_path(&self.root, to)).is_some() {
            bail!("{to} exists");
        }
        Ok(())
    }

    /// The change validating `change` makes, at `to` when given.
    fn validated(&self, change: SuggestedChange, to: Option<&GroupPath>) -> Result<Change> {
        let (path, continues) = match to {
            Some(to) => {
                self.ensure_free(to)?;
                (to.clone(), None)
            }
            None => match GroupPath::parse(&change.path) {
                Ok(path) => (path, change.continues),
                Err(error) => bail!(
                    "{} is no portable path ({error}): validate it at another path",
                    change.path
                ),
            },
        };
        let replaces = self.ledger.lock().head(&path.key()).map(|head| head.stamp);
        Ok(Change {
            path,
            content: change.content,
            replaces,
            continues,
        })
    }

    /// Decides the suggestions `shown`, as their versions were shown, in
    /// one patch: deleting their statements, and as `decision` says,
    /// publishing their changes, the later suggestion winning at a path.
    async fn decide(
        self: &Arc<Self>,
        work: &mut Work,
        shown: &[VersionRef],
        decision: Decision<'_>,
    ) -> Result<()> {
        self.ensure_joined(work)?;
        if shown.is_empty() {
            bail!("no suggestion to decide");
        }
        let placed = "only a suggestion of one file's content can be placed at another path";
        if matches!(decision, Decision::Validate(Some(_))) && shown.len() != 1 {
            bail!(placed);
        }
        let mut shown = shown.to_vec();
        shown.sort_by_key(|statement| statement.stamp);
        let mut decisions = Vec::new();
        let mut published = BTreeMap::<PathKey, Change>::new();
        for statement in &shown {
            let suggestion = self.shown_suggestion(work, statement).await?;
            decisions.push(Change {
                path: statement.path.clone(),
                content: None,
                replaces: Some(statement.stamp),
                continues: None,
            });
            let Decision::Validate(to) = decision else {
                continue;
            };
            if to.is_some() && !suggestion.placeable() {
                bail!(placed);
            }
            for change in suggestion.changes {
                let change = self.validated(change, to)?;
                published.insert(change.path.key(), change);
            }
        }
        decisions.extend(published.into_values());
        self.ledger
            .lock()
            .check(&self.member, &self.cert.member, &decisions)?;
        let keys: Vec<PathKey> = decisions.iter().map(|change| change.path.key()).collect();
        self.publish_at(self.clock.stamp(), decisions)?;
        work.protect_due = true;
        self.refresh_keys(work, &keys).await;
        self.follow_suggestions(work).await;
        Ok(())
    }
}

impl Engine {
    /// Every live suggestion this machine has read, oldest first.
    pub async fn suggestions(&self) -> Vec<SuggestionView> {
        let inner = &self.inner;
        let work = inner.work.lock().await;
        let ledger = inner.ledger.lock();
        let mut views: Vec<SuggestionView> = work
            .suggestions
            .values()
            .map(|live| SuggestionView {
                statement: live.version.reference(),
                author: live.version.author.clone(),
                machine: live.version.stamp.machine,
                time: live.version.stamp.rfc3339(),
                reason: live.suggestion.reason.clone(),
                placeable: live.suggestion.placeable(),
                changes: live
                    .suggestion
                    .changes
                    .iter()
                    .map(|change| SuggestedChangeView {
                        path: change.path.clone(),
                        what: what(change),
                        content: change.content,
                        replaces: change.replaces,
                        continues: change.continues.clone(),
                        outdated: GroupPath::parse(&change.path).is_ok_and(|path| {
                            ledger.head(&path.key()).map(|head| head.stamp) != change.replaces
                        }),
                    })
                    .collect(),
            })
            .collect();
        views.sort_by_key(|view| view.statement.stamp);
        views
    }

    /// Validates the suggestions `shown`, as they were shown: publishes
    /// their changes, the one content change of a single suggestion at
    /// `to` when given, and decides them.
    ///
    /// # Errors
    ///
    /// Fails if the member has not joined, a suggestion was decided or
    /// changed since it was shown, a path is not portable and no `to`
    /// places it, or `to` is taken.
    pub async fn validate(&self, shown: &[VersionRef], to: Option<&GroupPath>) -> Result<()> {
        let mut work = self.inner.work.lock().await;
        self.inner
            .decide(&mut work, shown, Decision::Validate(to))
            .await
    }

    /// Discards the suggestions `shown`, as they were shown: decides them
    /// without publishing their changes, which the history keeps.
    ///
    /// # Errors
    ///
    /// Fails if the member has not joined, or a suggestion was decided or
    /// changed since it was shown.
    pub async fn discard(&self, shown: &[VersionRef]) -> Result<()> {
        let mut work = self.inner.work.lock().await;
        self.inner.decide(&mut work, shown, Decision::Discard).await
    }
}
