//! Tests of the network on this host: machines that know the group secret
//! exchange what the other lacks and every later patch, a machine without
//! it learns nothing, and blobs move only between admitted machines,
//! each from several machines at once, which serve what they hold while
//! still downloading, and reach each other through the group's relay; a
//! machine speaking another version of the protocol is reported with the
//! pigeon it says it runs, or as predating the hello protocol, until it
//! updates and opens a session.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bao_tree::ChunkNum;
use iroh::address_lookup::MemoryLookup;
use iroh::endpoint::{Connection, RelayMode, presets};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayUrl};
use iroh_base::SecretKey;
use iroh_blobs::Hash;
use iroh_blobs::protocol::{ChunkRanges, GetRequest};
use iroh_blobs::store::mem::MemStore;
use pigeon_core::identity::{GroupSecret, MachineCert};
use pigeon_core::ledger::Ledger;
use pigeon_core::patch::SignedPatch;
use pigeon_core::test_machines;
use pigeon_net::bind::bind_local;
use pigeon_net::hello::{Announcement, Announcing, HELLO_ALPN, Heard, Standing};
use pigeon_net::relay::{relay_url, serve_relay};
use pigeon_net::wire::{Patches, Vector};
use pigeon_net::{Log, Node, Received};
use tokio::sync::mpsc;
use tokio::time::timeout;

struct Held(Mutex<Ledger>);

impl Log for Held {
    fn vector(&self) -> Vector {
        self.0.lock().unwrap().vector()
    }

    fn missing_from(&self, vector: &Vector) -> Patches {
        self.0
            .lock()
            .unwrap()
            .missing_from(vector)
            .into_iter()
            .cloned()
            .collect()
    }

    fn recognizes(&self, cert: &MachineCert) -> bool {
        self.0.lock().unwrap().recognizes(cert)
    }
}

fn secret(byte: u8) -> GroupSecret {
    GroupSecret([byte; 32])
}

/// Waits up to ten seconds for `condition`, failing with `what` if it
/// never holds.
async fn until<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let waited = timeout(Duration::from_secs(10), async {
        while !condition().await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    assert!(waited.await.is_ok(), "timed out: {what}");
}

struct Machine {
    signer: test_machines::Machine,
    log: Arc<Held>,
    node: Node,
    received: mpsc::Receiver<Received>,
    blobs: MemStore,
}

impl Machine {
    async fn start(secret: GroupSecret, lookup: &MemoryLookup) -> Self {
        let key = SecretKey::generate();
        let endpoint = bind_local(key.clone(), lookup).await.unwrap();
        Self::on(endpoint, key, secret)
    }

    /// Starts a machine that `lookup` knows only through `relay`.
    async fn relayed(secret: GroupSecret, lookup: &MemoryLookup, relay: &RelayUrl) -> Self {
        let key = SecretKey::generate();
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(key.clone())
            .relay_mode(RelayMode::Custom(RelayMap::empty()))
            .address_lookup(lookup.clone())
            .bind()
            .await
            .unwrap();
        let machine = Self::on(endpoint, key, secret);
        machine
            .node
            .use_relays(&RelayMap::from(relay.clone()))
            .await;
        until("the machine reaches the relay", || async {
            machine.node.home_relay().as_ref() == Some(relay)
        })
        .await;
        lookup
            .add_endpoint_info(EndpointAddr::new(machine.node.id()).with_relay_url(relay.clone()));
        machine
    }

    fn on(endpoint: Endpoint, key: SecretKey, secret: GroupSecret) -> Self {
        let log = Arc::new(Held(Mutex::new(Ledger::new(test_machines::group()))));
        let blobs = MemStore::new();
        let signer = test_machines::signer("mario", key);
        let (node, received) = Node::spawn(
            endpoint,
            Announcement::speaking_ours("0.1.0", "test"),
            test_machines::group(),
            secret,
            log.clone(),
            &blobs,
        );
        Self {
            signer,
            log,
            node,
            received,
            blobs,
        }
    }

    fn patch(&self, time: u64) -> SignedPatch {
        self.signer.patch(time, Vec::new())
    }

    fn hold(&self, patch: &SignedPatch) {
        self.log.0.lock().unwrap().insert(patch.clone()).unwrap();
    }

    async fn meet(&self, other: &Machine) {
        self.node.dial(other.node.id());
        until("the machines meet", || async {
            self.node.peers().contains(&other.node.id())
        })
        .await;
    }

    /// Takes chunks `range` of `hash` from `other` alone.
    async fn take(&self, other: &Machine, hash: Hash, range: std::ops::Range<u64>) {
        let connection = self
            .node
            .endpoint()
            .connect(other.node.id(), iroh_blobs::ALPN)
            .await
            .unwrap();
        let ranges = ChunkRanges::from(ChunkNum(range.start)..ChunkNum(range.end));
        self.blobs
            .remote()
            .execute_get(connection, GetRequest::blob_ranges(hash, ranges))
            .await
            .unwrap();
    }

    async fn holds(&self, hash: Hash, data: &[u8]) {
        assert_eq!(
            self.blobs.blobs().get_bytes(hash).await.unwrap().to_vec(),
            data
        );
    }

    async fn next_times(&mut self) -> Vec<u64> {
        let received = timeout(Duration::from_secs(10), self.received.recv())
            .await
            .expect("patches arrive")
            .unwrap();
        received.patches.iter().map(|p| p.stamp().time).collect()
    }
}

#[tokio::test]
async fn admitted_machines_exchange_missing_then_live_patches() {
    let lookup = MemoryLookup::new();
    let mut a = Machine::start(secret(5), &lookup).await;
    let mut b = Machine::start(secret(5), &lookup).await;
    let (a1, a2, b1) = (a.patch(1), a.patch(2), b.patch(3));
    a.hold(&a1);
    a.hold(&a2);
    b.hold(&b1);
    b.hold(&a1);
    a.node.dial(b.node.id());
    assert_eq!(a.next_times().await, [3]);
    assert_eq!(b.next_times().await, [2]);
    assert_eq!(a.node.peers(), [b.node.id()]);
    let a3 = a.patch(4);
    a.hold(&a3);
    a.node.publish(vec![a3]);
    assert_eq!(b.next_times().await, [4]);
    a.node.shutdown().await.unwrap();
    b.node.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_machine_without_the_secret_learns_nothing() {
    let lookup = MemoryLookup::new();
    let mut a = Machine::start(secret(5), &lookup).await;
    let mut intruder = Machine::start(secret(6), &lookup).await;
    let a1 = a.patch(1);
    a.hold(&a1);
    intruder.node.dial(a.node.id());
    a.node.dial(intruder.node.id());
    assert!(
        timeout(Duration::from_secs(2), intruder.received.recv())
            .await
            .is_err()
    );
    assert!(
        timeout(Duration::from_millis(100), a.received.recv())
            .await
            .is_err()
    );
    assert!(a.node.peers().is_empty());
    let tag = a
        .blobs
        .add_bytes(b"secret".to_vec())
        .temp_tag()
        .await
        .unwrap();
    let stolen = intruder.node.fetch(tag.hash(), vec![a.node.id()]);
    assert!(
        timeout(Duration::from_secs(10), stolen)
            .await
            .unwrap()
            .is_err()
    );
    a.node.shutdown().await.unwrap();
    intruder.node.shutdown().await.unwrap();
}

#[tokio::test]
async fn admitted_machines_fetch_blobs() {
    let lookup = MemoryLookup::new();
    let mut a = Machine::start(secret(5), &lookup).await;
    let b = Machine::start(secret(5), &lookup).await;
    let data = vec![42u8; 3 << 20];
    let tag = a.blobs.add_bytes(data.clone()).temp_tag().await.unwrap();
    let b1 = b.patch(1);
    b.hold(&b1);
    b.node.dial(a.node.id());
    assert_eq!(a.next_times().await, [1]);
    timeout(
        Duration::from_secs(20),
        b.node.fetch(tag.hash(), vec![a.node.id()]),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        b.blobs
            .blobs()
            .get_bytes(tag.hash())
            .await
            .unwrap()
            .to_vec(),
        data
    );
    a.node.shutdown().await.unwrap();
    b.node.shutdown().await.unwrap();
}

fn varied(size: usize) -> Vec<u8> {
    (0..size)
        .map(|i| u8::try_from(i * 7 % 251).unwrap())
        .collect()
}

#[tokio::test]
async fn a_blob_held_in_halves_by_two_machines_arrives_whole() {
    let lookup = MemoryLookup::new();
    let a = Machine::start(secret(5), &lookup).await;
    let b = Machine::start(secret(5), &lookup).await;
    let c = Machine::start(secret(5), &lookup).await;
    let d = Machine::start(secret(5), &lookup).await;
    let data = varied(4 << 20);
    let tag = a.blobs.add_bytes(data.clone()).temp_tag().await.unwrap();
    b.meet(&a).await;
    c.meet(&a).await;
    b.take(&a, tag.hash(), 0..2048).await;
    c.take(&a, tag.hash(), 2048..4096).await;
    a.node.shutdown().await.unwrap();
    d.meet(&b).await;
    d.meet(&c).await;
    let nobody = Hash::new(b"held by nobody");
    let none = d.node.fetch(nobody, vec![b.node.id(), c.node.id()]);
    assert!(
        timeout(Duration::from_secs(5), none)
            .await
            .expect("a blob nobody holds fails at once")
            .is_err()
    );
    let both = d.node.fetch(tag.hash(), vec![b.node.id(), c.node.id()]);
    timeout(Duration::from_secs(20), both)
        .await
        .unwrap()
        .unwrap();
    d.holds(tag.hash(), &data).await;
    for machine in [b, c, d] {
        machine.node.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn a_machine_serves_what_it_holds_while_still_downloading() {
    let lookup = MemoryLookup::new();
    let a = Machine::start(secret(5), &lookup).await;
    let b = Machine::start(secret(5), &lookup).await;
    let c = Machine::start(secret(5), &lookup).await;
    let data = varied(4 << 20);
    let tag = a.blobs.add_bytes(data.clone()).temp_tag().await.unwrap();
    b.meet(&a).await;
    c.meet(&b).await;
    b.take(&a, tag.hash(), 0..1024).await;
    let node = c.node.clone();
    let (hash, from) = (tag.hash(), b.node.id());
    let fetching = tokio::spawn(async move { node.fetch(hash, vec![from]).await });
    until(
        "the first piece arrives before the rest exists there",
        || async {
            c.blobs
                .observe(tag.hash())
                .await
                .unwrap()
                .ranges
                .contains(&ChunkNum(0))
        },
    )
    .await;
    assert!(!fetching.is_finished());
    b.take(&a, tag.hash(), 1024..4096).await;
    timeout(Duration::from_secs(20), fetching)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    c.holds(tag.hash(), &data).await;
    for machine in [a, b, c] {
        machine.node.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn machines_known_only_by_the_groups_relay_reach_each_other() {
    let server = serve_relay("127.0.0.1:0".parse().unwrap(), None)
        .await
        .unwrap();
    let url = relay_url(&server, "127.0.0.1").unwrap();
    let lookup = MemoryLookup::new();
    let a = Machine::relayed(secret(5), &lookup, &url).await;
    let b = Machine::relayed(secret(5), &lookup, &url).await;
    let data = varied(1 << 20);
    let tag = a.blobs.add_bytes(data.clone()).temp_tag().await.unwrap();
    b.meet(&a).await;
    let fetched = b.node.fetch(tag.hash(), vec![a.node.id()]);
    timeout(Duration::from_secs(20), fetched)
        .await
        .unwrap()
        .unwrap();
    b.holds(tag.hash(), &data).await;
    let other = serve_relay("127.0.0.1:0".parse().unwrap(), None)
        .await
        .unwrap();
    let moved = relay_url(&other, "127.0.0.1").unwrap();
    b.node.use_relays(&RelayMap::from(moved.clone())).await;
    until("the machine moves to the new relay", || async {
        b.node.home_relay() == Some(moved.clone())
    })
    .await;
    for machine in [a, b] {
        machine.node.shutdown().await.unwrap();
    }
}

/// A protocol that answers nothing, served under another name.
#[derive(Clone, Debug)]
struct Silent;

impl ProtocolHandler for Silent {
    fn accept(
        &self,
        _connection: Connection,
    ) -> impl Future<Output = Result<(), AcceptError>> + Send {
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn a_machine_speaking_another_version_of_the_protocol_is_reported() {
    let lookup = MemoryLookup::new();
    let a = Machine::start(secret(5), &lookup).await;
    let b = Machine::start(secret(5), &lookup).await;
    let older = bind_local(SecretKey::generate(), &lookup).await.unwrap();
    let older_router = Router::builder(older.clone())
        .accept(b"pigeon/sync/1", Silent)
        .spawn();
    let newer = bind_local(SecretKey::generate(), &lookup).await.unwrap();
    let announcement = Announcement {
        version: "9.0.0".to_owned(),
        commit: "abc".to_owned(),
        protocol: "pigeon/sync/999".to_owned(),
    };
    let newer_router = Router::builder(newer.clone())
        .accept(b"pigeon/sync/999", Silent)
        .accept(HELLO_ALPN, Announcing(announcement.clone()))
        .spawn();
    a.node.dial(older.id());
    a.node.dial(newer.id());
    a.meet(&b).await;
    let mut expected = vec![
        (older.id(), Heard::PreHello),
        (newer.id(), Heard::Announced(announcement)),
    ];
    expected.sort_by_key(|(machine, _)| *machine);
    until(
        "each refusal is reported with what the machine told, and only for those machines",
        || async { a.node.incompatible() == expected },
    )
    .await;
    assert_eq!(Standing::of(&Heard::PreHello), Standing::PreHello);
    for (_, heard) in &expected {
        if let Heard::Announced(_) = heard {
            assert_eq!(Standing::of(heard), Standing::Newer);
        }
    }
    let ours = Announcement::speaking_ours("0.1.0", "test");
    let behind = Announcement {
        protocol: "pigeon/sync/1".to_owned(),
        ..ours.clone()
    };
    assert_eq!(Standing::of(&Heard::Announced(behind)), Standing::Older);
    assert_eq!(Standing::of(&Heard::Announced(ours)), Standing::Unknown);
    let tolerant: Announcement =
        serde_json::from_str(r#"{"version":"10.0.0","later":true}"#).unwrap();
    assert_eq!(tolerant.version, "10.0.0");
    assert!(tolerant.protocol.is_empty());
    for router in [older_router, newer_router] {
        router.shutdown().await.unwrap();
    }
    for machine in [a, b] {
        machine.node.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn a_machine_reported_incompatible_is_no_longer_once_it_updates_and_dials_in() {
    let lookup = MemoryLookup::new();
    let a = Machine::start(secret(5), &lookup).await;
    let key = SecretKey::generate();
    let older = bind_local(key.clone(), &lookup).await.unwrap();
    let older_router = Router::builder(older.clone())
        .accept(b"pigeon/sync/1", Silent)
        .spawn();
    a.node.dial(older.id());
    until("the older machine is reported", || async {
        !a.node.incompatible().is_empty()
    })
    .await;
    older_router.shutdown().await.unwrap();
    let updated = Machine::on(
        bind_local(key.clone(), &lookup).await.unwrap(),
        key,
        secret(5),
    );
    updated.node.dial(a.node.id());
    until(
        "once the machine speaks the protocol, it is no longer reported",
        || async { a.node.incompatible().is_empty() },
    )
    .await;
    for machine in [a, updated] {
        machine.node.shutdown().await.unwrap();
    }
}
