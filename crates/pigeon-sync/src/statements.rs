//! Requests and decisions: statements in the statements folder through
//! which a member asks an owner to change one of the owner's files, anyone
//! decides it once, and the owner's machine applies what was forced,
//! accepted, or asked of the owner by the owner.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Result, bail};
use pigeon_core::ledger::Version;
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Change, Content};
use pigeon_core::path::{GroupPath, PathKey};
use pigeon_core::statement::{
    Decision, DecisionStatement, Mode, RequestStatement, STATEMENTS, decision_path, request_path,
};
use serde::de::DeserializeOwned;

use crate::engine::{Inner, JoinState, Wake, Work};

/// The folder holding requests.
pub(crate) fn requests_folder() -> String {
    format!("{STATEMENTS}/requests")
}

impl Inner {
    /// Reads a statement's body from the blob store.
    pub(crate) async fn read_statement<T: DeserializeOwned>(&self, content: &Content) -> Result<T> {
        Ok(serde_json::from_slice(
            &self.blobs.read(&content.hash).await?,
        )?)
    }

    /// Publishes a statement file with `stamp` as its patch's stamp.
    pub(crate) async fn publish_statement(
        &self,
        work: &mut Work,
        stamp: pigeon_core::clock::Stamp,
        path: GroupPath,
        body: Vec<u8>,
    ) -> Result<()> {
        let content = self.add_content(work, body).await?;
        let replaces = self.ledger.lock().head(&path.key()).map(|head| head.stamp);
        let change = Change {
            path: path.clone(),
            content: Some(content),
            replaces,
        };
        self.publish_at(stamp, vec![change], None)?;
        let _ = self.wake.send(Wake::Keys(vec![path.key()]));
        Ok(())
    }

    /// Asks the owners of the changed files to apply `changes`, a new
    /// file belonging to `adder` unless its path names its owner: one
    /// request per change, whose file is named after its patch's stamp;
    /// then applies at once those this member's machine may.
    pub(crate) async fn request(
        self: &Arc<Self>,
        work: &mut Work,
        changes: Vec<Change>,
        mode: Mode,
        message: &str,
        adder: &MemberName,
    ) -> Result<Vec<GroupPath>> {
        if work.join != JoinState::Joined {
            bail!("the member has not joined the group yet");
        }
        let mut paths = Vec::new();
        for change in changes {
            let owner = self.ledger.lock().owner_of(&change.path, adder);
            let stamp = self.clock.stamp();
            let path = request_path(&stamp);
            let statement = RequestStatement {
                owner,
                mode,
                changes: vec![change],
                message: message.to_owned(),
            };
            let body = serde_json::to_vec_pretty(&statement)?;
            self.publish_statement(work, stamp, path.clone(), body)
                .await?;
            paths.push(path);
        }
        self.apply_requests(work).await;
        Ok(paths)
    }

    /// Decides the proposal at `request`, which nobody decided yet,
    /// whoever it is addressed to.
    pub(crate) async fn decide(
        &self,
        work: &mut Work,
        request: &GroupPath,
        decision: Decision,
    ) -> Result<()> {
        let statement = self.request_statement(request).await?;
        if statement.mode == Mode::Force {
            bail!("{request} is forced: it needs no decision");
        }
        if self.applied_requests().contains(request) {
            bail!("{request} is applied already");
        }
        if let Some(decided) = self.decision(request).await {
            bail!("{request} is decided already: {decided:?}");
        }
        let body = serde_json::to_vec_pretty(&DecisionStatement {
            request: request.clone(),
            decision,
        })?;
        let stamp = self.clock.stamp();
        self.publish_statement(work, stamp, decision_path(request), body)
            .await
    }

    /// The live request at `request`.
    pub(crate) async fn request_statement(&self, request: &GroupPath) -> Result<RequestStatement> {
        let head = self.ledger.lock().head(&request.key()).cloned();
        match head.and_then(|head| head.content) {
            Some(content) if request.is_inside(&requests_folder()) => {
                self.read_statement(&content).await
            }
            _ => bail!("no request at {request}"),
        }
    }

    /// The decision on `request`, if anyone made one: the first, as its
    /// file freezes once published.
    pub(crate) async fn decision(&self, request: &GroupPath) -> Option<Decision> {
        let head = self
            .ledger
            .lock()
            .head(&decision_path(request).key())
            .cloned()?;
        let content = head.content?;
        let statement: DecisionStatement = self.read_statement(&content).await.ok()?;
        (statement.request == *request).then_some(statement.decision)
    }

    /// The requests an accepted patch already applied.
    pub(crate) fn applied_requests(&self) -> HashSet<GroupPath> {
        let ledger = self.ledger.lock();
        ledger
            .patches()
            .filter(|signed| ledger.outcome(&signed.stamp()) == Some(Ok(())))
            .filter_map(|signed| signed.patch.applies.clone())
            .collect()
    }

    /// Fetches the contents of every request to this member that is not
    /// refused, so that the owner can review it, and applies those forced
    /// or accepted and not yet applied once their contents are here.
    pub(crate) async fn apply_requests(self: &Arc<Self>, work: &mut Work) {
        if work.join != JoinState::Joined {
            return;
        }
        let folder = requests_folder();
        let requests: Vec<Version> = self
            .ledger
            .lock()
            .live()
            .filter(|version| version.path.is_inside(&folder))
            .cloned()
            .collect();
        let applied = self.applied_requests();
        for request in requests {
            if applied.contains(&request.path) {
                continue;
            }
            if let Err(error) = self.apply(work, &request).await {
                self.report(format!("applying {}: {error:#}", request.path));
            }
        }
    }

    async fn apply(self: &Arc<Self>, work: &mut Work, request: &Version) -> Result<()> {
        let Some(content) = request.content else {
            return Ok(());
        };
        if !self.blobs.has(&content.hash).await? {
            return Ok(());
        }
        let statement: RequestStatement = self.read_statement(&content).await?;
        let me = &self.member;
        if statement.owner != *me {
            return Ok(());
        }
        let wanted = match statement.mode {
            Mode::Force => Some(Decision::Accept),
            Mode::Propose if request.owner == *me => Some(Decision::Accept),
            Mode::Propose => self.decision(&request.path).await,
        };
        if wanted == Some(Decision::Refuse) {
            return Ok(());
        }
        let mut complete = true;
        for content in statement.changes.iter().filter_map(|change| change.content) {
            if !self.blobs.has(&content.hash).await? {
                complete = false;
                self.fetch(
                    work,
                    content.hash,
                    request.stamp.machine,
                    request.path.key(),
                );
            }
        }
        if !complete || wanted != Some(Decision::Accept) {
            return Ok(());
        }
        let changes: Vec<Change> = {
            let ledger = self.ledger.lock();
            statement
                .changes
                .into_iter()
                .map(|change| Change {
                    replaces: ledger.head(&change.path.key()).map(|head| head.stamp),
                    ..change
                })
                .collect()
        };
        self.ledger
            .lock()
            .check(me, &self.cert.member, &changes, true)?;
        let keys: Vec<PathKey> = changes.iter().map(|change| change.path.key()).collect();
        self.publish_at(self.clock.stamp(), changes, Some(request.path.clone()))?;
        self.refresh_keys(work, &keys).await;
        work.protect_due = true;
        Ok(())
    }
}
