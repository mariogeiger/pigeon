//! Tests of the network on this host: admitted machines exchange what the
//! other lacks and every later patch, a machine without the group secret
//! learns nothing, and blobs move only between admitted machines.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::address_lookup::MemoryLookup;
use iroh_base::SecretKey;
use iroh_blobs::store::mem::MemStore;
use pigeon_core::clock::Stamp;
use pigeon_core::identity::{GroupSecret, MachineCert, member_key};
use pigeon_core::ledger::Ledger;
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Patch, SignedPatch};
use pigeon_net::bind::bind_local;
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
}

struct Machine {
    secret: GroupSecret,
    key: SecretKey,
    log: Arc<Held>,
    node: Node,
    received: mpsc::Receiver<Received>,
    blobs: MemStore,
}

impl Machine {
    async fn start(secret: GroupSecret, lookup: &MemoryLookup) -> Self {
        let key = SecretKey::generate();
        let endpoint = bind_local(key.clone(), lookup).await.unwrap();
        let log = Arc::new(Held(Mutex::new(Ledger::new(secret.id()))));
        let blobs = MemStore::new();
        let (node, received) = Node::spawn(endpoint, secret.clone(), log.clone(), &blobs);
        Self {
            secret,
            key,
            log,
            node,
            received,
            blobs,
        }
    }

    fn patch(&self, time: u64) -> SignedPatch {
        let group = self.secret.id();
        let name = MemberName::parse("mario").unwrap();
        let member = member_key(&group, &name, "pw");
        let cert = MachineCert::issue(&group, name, &member, self.key.public());
        let patch = Patch {
            stamp: Stamp {
                time,
                machine: self.key.public(),
            },
            changes: Vec::new(),
            applies: None,
        };
        SignedPatch::sign(&group, patch, cert, &self.key)
    }

    fn hold(&self, patch: &SignedPatch) {
        self.log.0.lock().unwrap().insert(patch.clone()).unwrap();
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
    let secret = GroupSecret([5; 32]);
    let mut a = Machine::start(secret.clone(), &lookup).await;
    let mut b = Machine::start(secret, &lookup).await;
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
    let mut a = Machine::start(GroupSecret([5; 32]), &lookup).await;
    let mut intruder = Machine::start(GroupSecret([6; 32]), &lookup).await;
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
    let secret = GroupSecret([5; 32]);
    let mut a = Machine::start(secret.clone(), &lookup).await;
    let b = Machine::start(secret, &lookup).await;
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
