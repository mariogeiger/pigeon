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
use pigeon_core::clock::{Clock, MachineId};
use pigeon_core::identity::{RenewedSecret, member_key};
use pigeon_core::ledger::Ledger;
use pigeon_core::name::MemberName;
use pigeon_net::{Node, Received};
use pigeon_store::config::DataDir;
use pigeon_store::group_key::GroupKey;
use pigeon_store::state::State;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::engine::{Options, SharedLedger, bind};

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
            .filter(|(name, member)| member.key == Some(member_key(group, name).public()))
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
    node: Node,
    heard: watch::Sender<bool>,
    _mdns: Option<MdnsAddressLookup>,
}

impl Listening {
    /// Keeps sessions open with every machine the ledger or the key names.
    fn want_peers(&self) {
        let me = self.node.id();
        let vector = self.ledger.lock().vector();
        let machines: BTreeSet<MachineId> = vector
            .into_keys()
            .chain(self.key.bootstrap.iter().copied())
            .filter(|machine| *machine != me)
            .collect();
        self.node.want(machines);
    }

    /// Stores and folds the patches a peer sent that are new and not dated
    /// too far ahead.
    fn receive(&self, received: Received) -> Result<()> {
        for signed in received.patches {
            let known = self.ledger.lock().patch(&signed.stamp()).is_some();
            if known || self.clock.observe(&signed.stamp()).is_err() {
                continue;
            }
            if self.ledger.lock().insert(signed.clone()).is_ok() {
                self.state.add_patch(&signed)?;
            }
        }
        self.heard.send_replace(true);
        self.want_peers();
        Ok(())
    }
}

/// A group this machine listens to before joining it.
pub struct Listener {
    inner: Arc<Listening>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl Listener {
    /// Starts listening to the group `key` admits, from the data directory
    /// its engine will use.
    ///
    /// # Errors
    ///
    /// Fails if the data directory, its machine key or its state cannot be
    /// opened, or the endpoint bound.
    pub async fn start(data: &DataDir, key: GroupKey, options: &Options) -> Result<Self> {
        let machine = data.machine_key()?;
        let state = State::open(&data.state_path())?;
        let group = key.group;
        let ledger = Arc::new(SharedLedger::new(state.ledger(group)?));
        let (endpoint, mdns) = bind(&machine, &group, &options.network).await?;
        let blobs = MemStore::new();
        let secret = RenewedSecret {
            secret: key.secret.clone(),
            renewal: None,
        };
        let (node, received) = Node::spawn(endpoint, group, None, secret, ledger.clone(), &blobs);
        if let Some(mdns) = &mdns {
            node.follow(mdns);
        }
        let inner = Arc::new(Listening {
            key,
            state,
            ledger,
            clock: Clock::new(machine.public(), options.max_drift),
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
            Some(patches) = received.recv() => {
                if let Err(error) = inner.receive(patches) {
                    eprintln!("pigeon: listening to {}: {error:#}", inner.key.name);
                }
            }
        }
    }
}
