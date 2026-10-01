//! Tests of the network on this host: admitted machines exchange what the
//! other lacks and every later patch, a machine without the group secret
//! learns nothing unless the member list recognizes it, recognized machines
//! receive the latest secret, and blobs move only between admitted machines.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::address_lookup::MemoryLookup;
use iroh_base::SecretKey;
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
        let log = Arc::new(Held(Mutex::new(Ledger::new(group()))));
        let blobs = MemStore::new();
        let name = MemberName::parse("mario").unwrap();
        let member = member_key(&group(), &name, "pw");
        let cert = MachineCert::issue(&group(), name, &member, key.public());
        let (node, received) =
            Node::spawn(endpoint, group(), cert.clone(), secret, log.clone(), &blobs);
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
