//! Tests of the network on this host: admitted machines exchange what the
//! other lacks and every later patch, a machine without the group secret
//! learns nothing unless the member list recognizes it, recognized machines
//! receive the latest secret, and blobs move only between admitted machines,
//! each from several machines at once, which serve what they hold while
//! still downloading, and reach each other through the group's relay; a
//! machine speaking another version of the protocol is reported.

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
use pigeon_core::clock::Stamp;
use pigeon_core::identity::{
    GroupId, GroupSecret, MachineCert, Renewal, RenewedSecret, member_key,
};
use pigeon_core::ledger::Ledger;
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Change, Content, ContentHash, Patch, SignedPatch};
use pigeon_core::statement::member_path;
use pigeon_net::bind::bind_local;
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

    fn last_exclusion(&self) -> Option<Stamp> {
        self.0.lock().unwrap().last_exclusion()
    }
}

fn group() -> GroupId {
    GroupSecret([5; 32]).id()
}

fn first(byte: u8) -> RenewedSecret {
    RenewedSecret {
        secret: GroupSecret([byte; 32]),
        renewal: None,
    }
}

fn renewed(byte: u8, time: u64) -> RenewedSecret {
    let by = SecretKey::from_bytes(&[byte; 32]).public();
    RenewedSecret {
        secret: GroupSecret([byte; 32]),
        renewal: Some(Renewal {
            after: Stamp { time, machine: by },
            by,
        }),
    }
}

struct Machine {
    cert: MachineCert,
    key: SecretKey,
    log: Arc<Held>,
    node: Node,
    received: mpsc::Receiver<Received>,
    blobs: MemStore,
}

impl Machine {
    async fn start(secret: RenewedSecret, lookup: &MemoryLookup) -> Self {
        let key = SecretKey::generate();
        let endpoint = bind_local(key.clone(), lookup).await.unwrap();
        Self::on(endpoint, key, secret)
    }

    /// Starts a machine that `lookup` knows only through `relay`.
    async fn relayed(secret: RenewedSecret, lookup: &MemoryLookup, relay: &RelayUrl) -> Self {
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
        timeout(Duration::from_secs(10), async {
            while machine.node.home_relay().as_ref() != Some(relay) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the machine reaches the relay");
        lookup
            .add_endpoint_info(EndpointAddr::new(machine.node.id()).with_relay_url(relay.clone()));
        machine
    }

    fn on(endpoint: Endpoint, key: SecretKey, secret: RenewedSecret) -> Self {
        let log = Arc::new(Held(Mutex::new(Ledger::new(group()))));
        let blobs = MemStore::new();
        let name = MemberName::parse("mario").unwrap();
        let member = member_key(&group(), &name);
        let cert = MachineCert::issue(&group(), name, &member, key.public());
        let (node, received) = Node::spawn(
            endpoint,
            group(),
            Some(cert.clone()),
            secret,
            log.clone(),
            &blobs,
        );
        Self {
            cert,
            key,
            log,
            node,
            received,
            blobs,
        }
    }

    fn signed(&self, time: u64, changes: Vec<Change>) -> SignedPatch {
        let patch = Patch {
            stamp: Stamp {
                time,
                machine: self.key.public(),
            },
            changes,
            applies: None,
        };
        SignedPatch::sign(&group(), patch, self.cert.clone(), &self.key)
    }

    fn patch(&self, time: u64) -> SignedPatch {
        self.signed(time, Vec::new())
    }

    fn join(&self, time: u64) -> SignedPatch {
        let change = Change {
            path: member_path(&self.cert.name),
            content: Some(Content {
                hash: ContentHash([0; 32]),
                size: 0,
                executable: false,
            }),
            replaces: None,
        };
        self.signed(time, vec![change])
    }

    fn hold(&self, patch: &SignedPatch) {
        self.log.0.lock().unwrap().insert(patch.clone()).unwrap();
    }

    async fn meet(&self, other: &Machine) {
        self.node.dial(other.node.id());
        timeout(Duration::from_secs(10), async {
            while !self.node.peers().contains(&other.node.id()) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the machines meet");
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
    let mut a = Machine::start(first(5), &lookup).await;
    let mut b = Machine::start(first(5), &lookup).await;
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
    let mut a = Machine::start(first(5), &lookup).await;
    let mut intruder = Machine::start(first(6), &lookup).await;
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
    let mut a = Machine::start(first(5), &lookup).await;
    let b = Machine::start(first(5), &lookup).await;
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

#[tokio::test]
async fn a_recognized_machine_with_an_old_secret_gets_every_later_one() {
    let lookup = MemoryLookup::new();
    let mut a = Machine::start(renewed(6, 10), &lookup).await;
    let b = Machine::start(first(5), &lookup).await;
    let joined = a.join(1);
    a.hold(&joined);
    b.hold(&joined);
    let b1 = b.patch(2);
    b.hold(&b1);
    let mut secrets = b.node.secret();
    b.node.dial(a.node.id());
    assert_eq!(a.next_times().await, [2]);
    timeout(
        Duration::from_secs(10),
        secrets.wait_for(|held| *held == renewed(6, 10)),
    )
    .await
    .expect("the newer secret arrives")
    .unwrap();
    assert!(!a.node.offer(first(7)));
    assert!(a.node.offer(renewed(8, 20)));
    timeout(
        Duration::from_secs(10),
        secrets.wait_for(|held| *held == renewed(8, 20)),
    )
    .await
    .expect("a later secret arrives during the session")
    .unwrap();
    assert!(b.node.offer(renewed(9, 20)));
    let mut theirs = a.node.secret();
    timeout(
        Duration::from_secs(10),
        theirs.wait_for(|held| *held == renewed(9, 20)),
    )
    .await
    .expect("the greatest renewal wins on both sides")
    .unwrap();
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
    let a = Machine::start(first(5), &lookup).await;
    let b = Machine::start(first(5), &lookup).await;
    let c = Machine::start(first(5), &lookup).await;
    let d = Machine::start(first(5), &lookup).await;
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
    let a = Machine::start(first(5), &lookup).await;
    let b = Machine::start(first(5), &lookup).await;
    let c = Machine::start(first(5), &lookup).await;
    let data = varied(4 << 20);
    let tag = a.blobs.add_bytes(data.clone()).temp_tag().await.unwrap();
    b.meet(&a).await;
    c.meet(&b).await;
    b.take(&a, tag.hash(), 0..1024).await;
    let node = c.node.clone();
    let (hash, from) = (tag.hash(), b.node.id());
    let fetching = tokio::spawn(async move { node.fetch(hash, vec![from]).await });
    timeout(Duration::from_secs(10), async {
        while !c
            .blobs
            .observe(tag.hash())
            .await
            .unwrap()
            .ranges
            .contains(&ChunkNum(0))
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the first piece arrives before the rest exists there");
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
    let a = Machine::relayed(first(5), &lookup, &url).await;
    let b = Machine::relayed(first(5), &lookup, &url).await;
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
    timeout(Duration::from_secs(10), async {
        while b.node.home_relay() != Some(moved.clone()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the machine moves to the new relay");
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
    let a = Machine::start(first(5), &lookup).await;
    let b = Machine::start(first(5), &lookup).await;
    let older = bind_local(SecretKey::generate(), &lookup).await.unwrap();
    let router = Router::builder(older.clone())
        .accept(b"pigeon/sync/1", Silent)
        .spawn();
    a.node.dial(older.id());
    a.meet(&b).await;
    timeout(Duration::from_secs(10), async {
        while a.node.incompatible() != [older.id()] {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the refusal is reported, and only for that machine");
    router.shutdown().await.unwrap();
    for machine in [a, b] {
        machine.node.shutdown().await.unwrap();
    }
}
