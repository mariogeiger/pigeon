//! A machine that holds a group's key but no member name yet: it reaches
//! the group's machines with the group secret, keeps the patches they send
//! in the group's state, where the engine later finds them, and tells which
//! names it may join under, so that whoever chooses the name sees the group
//! first.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Result;
use iroh_blobs::store::mem::MemStore;
use iroh_mdns_address_lookup::MdnsAddressLookup;
use pigeon_core::clock::{Clock, Stamp};
use pigeon_core::identity::member_key;
use pigeon_core::ledger::Ledger;
use pigeon_core::name::MemberName;
use pigeon_net::{Node, Received};
use pigeon_store::group_dirs::GroupDirs;
use pigeon_store::group_key::GroupKey;
use pigeon_store::state::State;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::engine::{Options, SharedLedger, bind};
use crate::receive::{take, wanted};

/// The names of a group as a machine about to join it sees them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Names {
    /// Whether a machine of the group sent its patches, without which the
    /// names below are only those the key knows of.
    pub heard: bool,
    /// The members whose name still derives their key: joining as one of
    /// them adds a machine of theirs.
    pub members: Vec<MemberName>,
    /// Every name a new member cannot take: the members', past and
    /// present, and those a tag claims.
    pub taken: Vec<MemberName>,
}

impl Names {
    /// The names `ledger` holds.
    #[must_use]
    pub fn of(ledger: &Ledger, heard: bool) -> Self {
        let group = ledger.group();
        let members = ledger
            .members()
            .iter()
            .filter(|(name, member)| member.key == member_key(group, name).public())
            .map(|(name, _)| name.clone())
            .collect();
        let taken: BTreeSet<MemberName> = ledger
            .members()
            .keys()
            .chain(ledger.tag_claims())
            .cloned()
            .collect();
        Self {
            heard,
            members,
            taken: taken.into_iter().collect(),
        }
    }
}

struct Listening {
    key: GroupKey,
    state: State,
    ledger: Arc<SharedLedger>,
    clock: Clock,
    /// The first stamp of this run's clock.
    first_stamp: Stamp,
    node: Node,
    heard: watch::Sender<bool>,
    _mdns: Option<MdnsAddressLookup>,
}

impl Listening {
    /// Keeps sessions open with every machine the ledger or the key names.
    fn want_peers(&self) {
        let machines = wanted(&self.ledger.lock(), &self.key, self.node.id());
        self.node.want(machines);
    }

    /// Stores and folds the patches a peer sent that are new and not dated
    /// too far ahead, and tells why others were refused.
    fn receive(&self, received: Received) {
        let taken = take(
            &self.ledger,
            |new| self.state.add_patches(new),
            &self.clock,
            &self.first_stamp,
            received,
        );
        for refusal in &taken.refusals {
            eprintln!("pigeon: listening to {}: {refusal}", self.key.name);
        }
        if taken.stored {
            self.heard.send_replace(true);
        }
        self.want_peers();
    }
}

/// A group this machine listens to before joining it.
pub struct Listener {
    inner: Arc<Listening>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Listener {
    /// Starts listening to the group `key` admits, from the folders its
    /// engine will use.
    ///
    /// # Errors
    ///
    /// Fails if the group's secrets or state cannot be opened, or the
    /// endpoint bound.
    pub async fn start(dirs: &GroupDirs, key: GroupKey, options: &Options) -> Result<Self> {
        let machine = dirs.secrets()?.machine;
        let state = State::open(&dirs.state_path())?;
        let group = key.group;
        let ledger = state.ledger(group)?;
        let clock = Clock::new(machine.public(), options.max_drift);
        if let Some(newest) = ledger.newest() {
            clock.observe(newest);
        }
        let first_stamp = clock.stamp();
        let ledger = Arc::new(SharedLedger::new(ledger));
        let (endpoint, mdns) = bind(&machine, &group, &options.network).await?;
        let blobs = MemStore::new();
        let (node, received) = Node::spawn(
            endpoint,
            options.announcement.clone(),
            group,
            key.secret.clone(),
            ledger.clone(),
            &blobs,
            options.node,
        );
        if let Some(mdns) = &mdns {
            node.dial_discovered(mdns);
        }
        let inner = Arc::new(Listening {
            key,
            state,
            ledger,
            clock,
            first_stamp,
            node,
            heard: watch::Sender::new(false),
            _mdns: mdns,
        });
        inner.want_peers();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(run(inner.clone(), received, stopped));
        Ok(Self {
            inner,
            stop: Some(stop),
            task,
        })
    }

    /// The key this machine listens with.
    #[must_use]
    pub fn key(&self) -> &GroupKey {
        &self.inner.key
    }

    /// Whether a machine of the group has sent its patches yet.
    #[must_use]
    pub fn heard(&self) -> watch::Receiver<bool> {
        self.inner.heard.subscribe()
    }

    /// The names the patches received so far hold.
    #[must_use]
    pub fn names(&self) -> Names {
        Names::of(&self.inner.ledger.lock(), *self.inner.heard.borrow())
    }

    /// Stops listening and closes the state, for the engine to open.
    ///
    /// # Errors
    ///
    /// Fails if the network does not shut down cleanly.
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.task.await?;
        self.inner.node.shutdown().await
    }
}

async fn run(
    inner: Arc<Listening>,
    mut received: mpsc::Receiver<Received>,
    mut stopped: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = &mut stopped => return,
            Some(patches) = received.recv() => inner.receive(patches),
        }
    }
}
