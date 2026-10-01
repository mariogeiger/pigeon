//! One group's presence on the network: an iroh endpoint that admits only
//! machines the member list recognizes or that prove they know the group
//! secret, keeps a sync session with every machine it reaches, passes the
//! latest group secret to recognized machines, and serves and fetches blobs
//! among admitted machines, each from several machines at once, through the
//! relays it is told to use when no direct connection works, and tells
//! which machines speak no protocol of its own.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use iroh::endpoint::{
    ConnectError, ConnectingError, Connection, ConnectionError, TransportErrorCode,
};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, RelayMap, RelayUrl};
use iroh_blobs::api::Store;
use iroh_blobs::provider::events::{
    AbortReason, ConnectMode, EventMask, EventSender, ProviderMessage,
};
use iroh_blobs::util::connection_pool::{self, ConnectionPool};
use iroh_blobs::{BlobsProtocol, Hash};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use n0_future::StreamExt;
use pigeon_core::clock::{MachineId, Stamp};
use pigeon_core::identity::{GroupId, MachineCert, RenewedSecret};
use tokio::sync::{broadcast, mpsc, watch};

use crate::swarm;
use crate::wire::{self, Hello, Message, Patches, SYNC_ALPN, Vector};

/// The patches a node holds, as its sessions need them.
pub trait Log: Send + Sync + 'static {
    /// The latest patch time known from each machine.
    fn vector(&self) -> Vector;
    /// Every patch held that `vector` does not cover, in stamp order.
    fn missing_from(&self, vector: &Vector) -> Patches;
    /// Whether the member list binds the certificate's name to its key now.
    fn recognizes(&self, cert: &MachineCert) -> bool;
    /// The stamp of the last exclusion, before which a secret admits no one.
    fn last_exclusion(&self) -> Option<Stamp>;
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
    group: GroupId,
    cert: MachineCert,
    secret: watch::Sender<RenewedSecret>,
    log: Arc<dyn Log>,
    outgoing: broadcast::Sender<Arc<Message>>,
    incoming: mpsc::Sender<Received>,
    attempts: Counts,
    connected: Counts,
    admitted: Mutex<HashSet<MachineId>>,
    wanted: Mutex<BTreeSet<MachineId>>,
    incompatible: Mutex<BTreeSet<MachineId>>,
    relays: tokio::sync::Mutex<Vec<RelayUrl>>,
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
            group: self.group,
            cert: self.cert.clone(),
            admission: self
                .secret
                .borrow()
                .secret
                .admission(&self.endpoint.id(), &remote),
            vector: self.log.vector(),
        }
    }

    /// Admits a machine that the member list recognizes, or that proves it
    /// knows the group secret held, unless an exclusion made that secret
    /// stale.
    fn admit(&self, remote: MachineId, hello: &Hello) -> Result<()> {
        ensure!(
            hello.group == self.group,
            "{remote} belongs to another group"
        );
        let recognized = hello.cert.machine == remote
            && hello.cert.is_valid(&self.group)
            && self.log.recognizes(&hello.cert);
        let knows = || {
            let held = self.secret.borrow();
            let stale = self
                .log
                .last_exclusion()
                .is_some_and(|exclusion| held.predates(exclusion));
            !stale && hello.admission == held.secret.admission(&remote, &self.endpoint.id())
        };
        ensure!(
            recognized || knows(),
            "{remote} is no member's machine and does not know the group secret"
        );
        lock(&self.admitted).insert(remote);
        Ok(())
    }

    /// Adopts `secret` if it supersedes the one held.
    fn offer(&self, secret: RenewedSecret) -> bool {
        self.secret.send_if_modified(|held| {
            let newer = secret.supersedes(held);
            if newer {
                *held = secret;
            }
            newer
        })
    }

    /// Runs one sync session until either side closes it: the dialer
    /// proves itself first, the acceptor answers only once convinced, then
    /// each sends its group secret and what the other lacks, then every
    /// later patch, and every later secret while the member list still
    /// recognizes the other machine.
    async fn session(self: Arc<Self>, connection: Connection, dialer: bool) -> Result<()> {
        let remote = connection.remote_id();
        let mut outgoing = self.outgoing.subscribe();
        let mut secrets = self.secret.subscribe();
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
            let secret = Message::Secret(secrets.borrow_and_update().clone());
            wire::write(&mut send, &secret).await?;
            let missing = Message::Patches(self.log.missing_from(&theirs.vector));
            wire::write(&mut send, &missing).await?;
            loop {
                let message = tokio::select! {
                    patches = outgoing.recv() => match patches {
                        Ok(message) => message,
                        Err(broadcast::error::RecvError::Lagged(_)) => Arc::new(
                            Message::Patches(self.log.missing_from(&theirs.vector)),
                        ),
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    },
                    changed = secrets.changed() => {
                        changed?;
                        let secret = secrets.borrow_and_update().clone();
                        if !self.log.recognizes(&theirs.cert) {
                            continue;
                        }
                        Arc::new(Message::Secret(secret))
                    }
                };
                wire::write(&mut send, &*message).await?;
            }
        };
        let reader = async {
            loop {
                match wire::read(&mut recv).await? {
                    Message::Secret(secret) => {
                        self.offer(secret);
                    }
                    Message::Patches(patches) if !patches.is_empty() => {
                        let received = Received {
                            from: remote,
                            patches,
                        };
                        if self.incoming.send(received).await.is_err() {
                            return Ok(());
                        }
                    }
                    Message::Patches(_) => {}
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
            let dialed = shared.endpoint.connect(machine, SYNC_ALPN).await;
            let incompatible = dialed.as_ref().is_err_and(speaks_no_protocol_of_ours);
            if incompatible {
                lock(&shared.incompatible).insert(machine);
            } else {
                lock(&shared.incompatible).remove(&machine);
            }
            if let Ok(connection) = dialed {
                let _ = shared.clone().session(connection, true).await;
            }
            count(&shared.attempts, machine, false);
        });
    }
}

/// The TLS alert by which a machine refuses every protocol offered, as
/// RFC 7301 numbers it.
const NO_APPLICATION_PROTOCOL: u8 = 120;

/// Whether the machine dialed refused the connection for speaking none of
/// the protocols offered, as one running another version of pigeon does.
fn speaks_no_protocol_of_ours(error: &ConnectError) -> bool {
    let (ConnectError::Connection { source: closed, .. }
    | ConnectError::Connecting {
        source: ConnectingError::ConnectionError { source: closed, .. },
        ..
    }) = error
    else {
        return false;
    };
    matches!(
        closed,
        ConnectionError::ConnectionClosed(close)
            if close.error_code == TransportErrorCode::crypto(NO_APPLICATION_PROTOCOL)
    )
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

/// How long a blob connection to a machine stays open unused.
const IDLE: Duration = Duration::from_secs(30);
/// How long reaching a machine for blobs may take, relays included.
const CONNECT: Duration = Duration::from_secs(15);

/// One group's node.
#[derive(Clone)]
pub struct Node {
    shared: Arc<Shared>,
    router: Router,
    store: Store,
    pool: ConnectionPool,
}

impl Node {
    /// Starts serving sync and blobs on `endpoint` for the machine `cert`
    /// vouches for, and returns the node with the stream of patches its
    /// peers send.
    #[must_use]
    pub fn spawn(
        endpoint: Endpoint,
        group: GroupId,
        cert: MachineCert,
        secret: RenewedSecret,
        log: Arc<dyn Log>,
        blobs: &Store,
    ) -> (Self, mpsc::Receiver<Received>) {
        let (incoming, received) = mpsc::channel(64);
        let shared = Arc::new(Shared {
            endpoint: endpoint.clone(),
            group,
            cert,
            secret: watch::Sender::new(secret),
            log,
            outgoing: broadcast::channel(256).0,
            incoming,
            attempts: Counts::default(),
            connected: Counts::default(),
            admitted: Mutex::default(),
            wanted: Mutex::default(),
            incompatible: Mutex::default(),
            relays: tokio::sync::Mutex::default(),
        });
        let pool = ConnectionPool::new(
            endpoint.clone(),
            iroh_blobs::ALPN,
            connection_pool::Options {
                idle_timeout: IDLE,
                connect_timeout: CONNECT,
                ..connection_pool::Options::default()
            },
        );
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
                store: blobs.clone(),
                pool,
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

    /// The group secret held, which changes when a peer passes a later one.
    #[must_use]
    pub fn secret(&self) -> watch::Receiver<RenewedSecret> {
        self.shared.secret.subscribe()
    }

    /// Adopts `secret` and passes it to recognized peers if it supersedes
    /// the one held; returns whether it did.
    #[must_use]
    pub fn offer(&self, secret: RenewedSecret) -> bool {
        self.shared.offer(secret)
    }

    /// Sends patches to every connected peer.
    pub fn publish(&self, patches: Patches) {
        if !patches.is_empty() {
            let _ = self
                .shared
                .outgoing
                .send(Arc::new(Message::Patches(patches)));
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

    /// Makes `relays` the only relays that carry this node's traffic when no
    /// direct connection works.
    pub async fn use_relays(&self, relays: &RelayMap) {
        let mut held = self.shared.relays.lock().await;
        let wanted: Vec<_> = relays.relays();
        for config in &wanted {
            if !held.contains(&config.url) {
                self.endpoint()
                    .insert_relay(config.url.clone(), config.clone())
                    .await;
            }
        }
        for url in held.iter() {
            if !wanted.iter().any(|config| config.url == *url) {
                self.endpoint().remove_relay(url).await;
            }
        }
        *held = wanted
            .into_iter()
            .map(|config| config.url.clone())
            .collect();
    }

    /// The relay this machine is reachable through, once it reached one.
    #[must_use]
    pub fn home_relay(&self) -> Option<RelayUrl> {
        self.endpoint().addr().relay_urls().next().cloned()
    }

    /// The machines with an open session.
    #[must_use]
    pub fn peers(&self) -> Vec<MachineId> {
        lock(&self.shared.connected).keys().copied().collect()
    }

    /// The machines that refused the last dial for speaking no protocol of
    /// this machine's: they run another version of pigeon.
    #[must_use]
    pub fn incompatible(&self) -> Vec<MachineId> {
        lock(&self.shared.incompatible).iter().copied().collect()
    }

    /// Fetches a blob from `providers` at once, resuming what is held.
    ///
    /// # Errors
    ///
    /// Fails if no provider can deliver what is missing, or none delivered
    /// more for [`swarm::STALL`].
    pub async fn fetch(&self, hash: Hash, providers: Vec<MachineId>) -> Result<()> {
        let rotation = self.id().as_bytes()[..8]
            .iter()
            .fold(0, |rotation, byte| rotation << 8 | u64::from(*byte));
        swarm::fetch(&self.store, &self.pool, rotation, hash, providers)
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
