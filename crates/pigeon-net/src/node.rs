//! One group's presence on the network: an iroh endpoint that admits only
//! machines proving they know the group secret, keeps a sync session with
//! every machine it reaches, and serves and fetches blobs among admitted
//! machines.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use iroh::Endpoint;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh_blobs::api::Store;
use iroh_blobs::api::downloader::Downloader;
use iroh_blobs::provider::events::{
    AbortReason, ConnectMode, EventMask, EventSender, ProviderMessage,
};
use iroh_blobs::{BlobsProtocol, Hash};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;
use pigeon_core::clock::MachineId;
use pigeon_core::identity::GroupSecret;
use tokio::sync::{broadcast, mpsc};

use crate::wire::{self, Hello, Patches, SYNC_ALPN, Vector};

/// The patches a node holds, as its sessions need them.
pub trait Log: Send + Sync + 'static {
    /// The latest patch time known from each machine.
    fn vector(&self) -> Vector;
    /// Every patch held that `vector` does not cover, in stamp order.
    fn missing_from(&self, vector: &Vector) -> Patches;
}

/// Patches a peer sent.
#[derive(Debug)]
pub struct Received {
    pub from: MachineId,
    pub patches: Patches,
}

type Counts = Mutex<HashMap<MachineId, usize>>;

struct Shared {
    endpoint: Endpoint,
    secret: GroupSecret,
    log: Arc<dyn Log>,
    outgoing: broadcast::Sender<Arc<Patches>>,
    incoming: mpsc::Sender<Received>,
    attempts: Counts,
    connected: Counts,
    admitted: Mutex<HashSet<MachineId>>,
    wanted: Mutex<BTreeSet<MachineId>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().expect("no panic holds a node lock")
}

fn count(counts: &Counts, machine: MachineId, up: bool) {
    let mut counts = lock(counts);
    let entry = counts.entry(machine).or_default();
    if up {
        *entry += 1;
    } else {
        *entry -= 1;
        if *entry == 0 {
            counts.remove(&machine);
        }
    }
}

impl Shared {
    fn hello(&self, remote: MachineId) -> Hello {
        Hello {
            group: self.secret.id(),
            admission: self.secret.admission(&self.endpoint.id(), &remote),
            vector: self.log.vector(),
        }
    }

    fn admit(&self, remote: MachineId, hello: &Hello) -> Result<()> {
        ensure!(
            hello.group == self.secret.id(),
            "{remote} belongs to another group"
        );
        ensure!(
            hello.admission == self.secret.admission(&remote, &self.endpoint.id()),
            "{remote} does not know the group secret"
        );
        lock(&self.admitted).insert(remote);
        Ok(())
    }

    /// Runs one sync session until either side closes it: the dialer
    /// proves itself first, the acceptor answers only once convinced, then
    /// each sends what the other lacks and every later patch.
    async fn session(self: Arc<Self>, connection: Connection, dialer: bool) -> Result<()> {
        let remote = connection.remote_id();
        let mut outgoing = self.outgoing.subscribe();
        let (mut send, mut recv, theirs) = if dialer {
            let (mut send, mut recv) = connection.open_bi().await?;
            wire::write(&mut send, &self.hello(remote)).await?;
            let theirs: Hello = wire::read(&mut recv).await?;
            self.admit(remote, &theirs)?;
            (send, recv, theirs)
        } else {
            let (mut send, mut recv) = connection.accept_bi().await?;
            let theirs: Hello = wire::read(&mut recv).await?;
            self.admit(remote, &theirs)?;
            wire::write(&mut send, &self.hello(remote)).await?;
            (send, recv, theirs)
        };
        count(&self.connected, remote, true);
        let writer = async {
            wire::write(&mut send, &self.log.missing_from(&theirs.vector)).await?;
            loop {
                match outgoing.recv().await {
                    Ok(patches) => wire::write(&mut send, &*patches).await?,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        wire::write(&mut send, &self.log.missing_from(&theirs.vector)).await?;
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
        };
        let reader = async {
            loop {
                let patches: Patches = wire::read(&mut recv).await?;
                if !patches.is_empty() {
                    let received = Received {
                        from: remote,
                        patches,
                    };
                    if self.incoming.send(received).await.is_err() {
                        return Ok(());
                    }
                }
            }
        };
        let result: Result<()> = tokio::select! {
            result = writer => result,
            result = reader => result,
        };
        count(&self.connected, remote, false);
        result
    }

    fn dial(self: &Arc<Self>, machine: MachineId) {
        if machine == self.endpoint.id()
            || lock(&self.attempts).contains_key(&machine)
            || lock(&self.connected).contains_key(&machine)
        {
            return;
        }
        count(&self.attempts, machine, true);
        let shared = self.clone();
        tokio::spawn(async move {
            if let Ok(connection) = shared.endpoint.connect(machine, SYNC_ALPN).await {
                let _ = shared.clone().session(connection, true).await;
            }
            count(&shared.attempts, machine, false);
        });
    }
}

#[derive(Clone)]
struct SyncProtocol(Arc<Shared>);

impl fmt::Debug for SyncProtocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SyncProtocol({})", self.0.endpoint.id())
    }
}

impl ProtocolHandler for SyncProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote = connection.remote_id();
        count(&self.0.attempts, remote, true);
        let result = self.0.clone().session(connection, false).await;
        count(&self.0.attempts, remote, false);
        result.map_err(|error| AcceptError::from_boxed(error.into()))
    }
}

/// Lets only admitted machines fetch blobs.
fn blob_gate(shared: &Arc<Shared>) -> EventSender {
    let mask = EventMask {
        connected: ConnectMode::Intercept,
        ..EventMask::DEFAULT
    };
    let (sender, mut events) = EventSender::channel(32, mask);
    let shared = Arc::downgrade(shared);
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            if let ProviderMessage::ClientConnected(request) = event {
                let admitted = request.endpoint_id.is_some_and(|remote| {
                    shared
                        .upgrade()
                        .is_some_and(|shared| lock(&shared.admitted).contains(&remote))
                });
                let answer = if admitted {
                    Ok(())
                } else {
                    Err(AbortReason::Permission)
                };
                request.tx.send(answer).await.ok();
            }
        }
    });
    sender
}

/// How often a node dials the machines it wants but lacks.
pub const REDIAL: Duration = Duration::from_secs(15);

/// One group's node.
#[derive(Clone)]
pub struct Node {
    shared: Arc<Shared>,
    router: Router,
    downloader: Downloader,
}

impl Node {
    /// Starts serving sync and blobs on `endpoint`, and returns the node
    /// with the stream of patches its peers send.
    #[must_use]
    pub fn spawn(
        endpoint: Endpoint,
        secret: GroupSecret,
        log: Arc<dyn Log>,
        blobs: &Store,
    ) -> (Self, mpsc::Receiver<Received>) {
        let (incoming, received) = mpsc::channel(64);
        let shared = Arc::new(Shared {
            endpoint: endpoint.clone(),
            secret,
            log,
            outgoing: broadcast::channel(256).0,
            incoming,
            attempts: Counts::default(),
            connected: Counts::default(),
            admitted: Mutex::default(),
            wanted: Mutex::default(),
        });
        let downloader = blobs.downloader(&endpoint);
        let router = Router::builder(endpoint)
            .accept(SYNC_ALPN, SyncProtocol(shared.clone()))
            .accept(
                iroh_blobs::ALPN,
                BlobsProtocol::new(blobs, Some(blob_gate(&shared))),
            )
            .spawn();
        let redial = Arc::downgrade(&shared);
        tokio::spawn(async move {
            loop {
                let Some(shared) = redial.upgrade() else {
                    return;
                };
                let wanted: Vec<MachineId> = lock(&shared.wanted).iter().copied().collect();
                for machine in wanted {
                    shared.dial(machine);
                }
                drop(shared);
                tokio::time::sleep(REDIAL).await;
            }
        });
        (
            Self {
                shared,
                router,
                downloader,
            },
            received,
        )
    }

    /// This machine's id.
    #[must_use]
    pub fn id(&self) -> MachineId {
        self.shared.endpoint.id()
    }

    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.shared.endpoint
    }

    /// Sends patches to every connected peer.
    pub fn publish(&self, patches: Patches) {
        if !patches.is_empty() {
            let _ = self.shared.outgoing.send(Arc::new(patches));
        }
    }

    /// Opens a session with `machine` unless one is open or opening.
    pub fn dial(&self, machine: MachineId) {
        self.shared.dial(machine);
    }

    /// Sets the machines to keep a session with, dialing the new ones now.
    pub fn want(&self, machines: impl IntoIterator<Item = MachineId>) {
        let machines: BTreeSet<MachineId> = machines.into_iter().collect();
        for machine in &machines {
            self.shared.dial(*machine);
        }
        *lock(&self.shared.wanted) = machines;
    }

    /// Dials every machine that local-network discovery reports.
    pub fn follow(&self, mdns: &MdnsAddressLookup) {
        let shared = Arc::downgrade(&self.shared);
        let mdns = mdns.clone();
        tokio::spawn(async move {
            let mut events = mdns.subscribe().await;
            while let Some(event) = events.next().await {
                let Some(shared) = shared.upgrade() else {
                    return;
                };
                if let DiscoveryEvent::Discovered { endpoint_info, .. } = event {
                    shared.dial(endpoint_info.endpoint_id);
                }
            }
        });
    }

    /// The machines with an open session.
    #[must_use]
    pub fn peers(&self) -> Vec<MachineId> {
        lock(&self.shared.connected).keys().copied().collect()
    }

    /// Fetches a blob from any of `providers`, resuming what is held.
    ///
    /// # Errors
    ///
    /// Fails if no provider delivers the whole blob.
    pub async fn fetch(&self, hash: Hash, providers: Vec<MachineId>) -> Result<()> {
        if providers.is_empty() {
            bail!("no machine holds {hash}");
        }
        self.downloader
            .download(hash, providers)
            .await
            .with_context(|| format!("fetching {hash}"))
    }

    /// Closes every session and the endpoint.
    ///
    /// # Errors
    ///
    /// Fails if a protocol does not shut down cleanly.
    pub async fn shutdown(&self) -> Result<()> {
        self.router.shutdown().await?;
        Ok(())
    }
}
