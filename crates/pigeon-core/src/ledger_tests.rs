//! Tests of the ledger's guarantees: one writer per file, earliest claims,
//! freezing, joining, rebinding, concurrent changes, and convergence in any
//! order.

use iroh_base::SecretKey;

use super::*;
use crate::identity::{GroupSecret, MachineCert, member_key};
use crate::patch::{ContentHash, Patch};
use crate::statement::rebind_path;

struct Machine {
    group: GroupId,
    cert: MachineCert,
    key: SecretKey,
}

fn group() -> GroupId {
    GroupSecret([9; 32]).id()
}

fn machine(name: &str, password: &str, seed: u8) -> Machine {
    let group = group();
    let name = MemberName::parse(name).unwrap();
    let member = member_key(&group, &name, password);
    let key = SecretKey::from_bytes(&[seed; 32]);
    let cert = MachineCert::issue(&group, name, &member, key.public());
    Machine { group, cert, key }
}

fn content(byte: u8) -> Content {
    Content {
        hash: ContentHash([byte; 32]),
        size: 1,
        executable: false,
    }
}

fn change(path: &str, byte: Option<u8>, replaces: Option<Stamp>) -> Change {
    Change {
        path: GroupPath::parse(path).unwrap(),
        content: byte.map(content),
        replaces,
    }
}

impl Machine {
    fn stamp(&self, time: u64) -> Stamp {
        Stamp {
            time,
            machine: self.key.public(),
        }
    }

    fn patch(&self, time: u64, changes: Vec<Change>, applies: Option<&str>) -> SignedPatch {
        let patch = Patch {
            stamp: self.stamp(time),
            changes,
            applies: applies.map(|path| GroupPath::parse(path).unwrap()),
        };
        SignedPatch::sign(&self.group, patch, self.cert.clone(), &self.key)
    }

    fn join(&self, time: u64) -> SignedPatch {
        let path = member_path(&self.cert.name);
        self.patch(time, vec![change(path.as_str(), Some(0), None)], None)
    }

    fn rebind(&self, time: u64, name: &str, key: Option<PublicKey>) -> SignedPatch {
        let rebind = RebindStatement {
            name: MemberName::parse(name).unwrap(),
            key,
        };
        let path = rebind_path(&rebind, &self.stamp(time));
        self.patch(time, vec![change(path.as_str(), Some(0), None)], None)
    }
}

fn key(path: &str) -> PathKey {
    GroupPath::parse(path).unwrap().key()
}

fn rejection(ledger: &Ledger, stamp: Stamp) -> Rejection {
    ledger.outcome(&stamp).unwrap().unwrap_err().clone()
}

#[test]
fn only_the_owner_writes_a_personal_file() {
    let mario = machine("mario", "a", 1);
    let bob = machine("bob", "b", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    ledger
        .insert(mario.patch(3, vec![change("_mario/a", Some(1), None)], None))
        .unwrap();
    let edit = bob.patch(4, vec![change("_mario/a", Some(2), None)], None);
    ledger.insert(edit.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, edit.stamp()),
        Rejection::NotOwner { .. }
    ));
    let create = bob.patch(5, vec![change("x/_Mario/b", Some(2), None)], None);
    ledger.insert(create.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, create.stamp()),
        Rejection::NotOwner { .. }
    ));
    assert_eq!(
        ledger.head(&key("_mario/a")).unwrap().content,
        Some(content(1))
    );
}

#[test]
fn the_earliest_drop_claim_wins_whatever_the_arrival_order() {
    let mario = machine("mario", "a", 1);
    let bob = machine("bob", "b", 2);
    let late = bob.patch(20, vec![change("Notes.txt", Some(2), None)], None);
    let early = mario.patch(10, vec![change("notes.txt", Some(1), None)], None);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    ledger.insert(late.clone()).unwrap();
    assert!(ledger.outcome(&late.stamp()).unwrap().is_ok());
    ledger.insert(early.clone()).unwrap();
    assert!(ledger.outcome(&early.stamp()).unwrap().is_ok());
    assert!(matches!(
        rejection(&ledger, late.stamp()),
        Rejection::NotOwner { .. }
    ));
    let head = ledger.head(&key("NOTES.TXT")).unwrap();
    assert_eq!(head.owner.as_str(), "mario");
    assert_eq!(head.path.as_str(), "notes.txt");
}

#[test]
fn a_published_drop_file_changes_only_through_a_request() {
    let mario = machine("mario", "a", 1);
    let bob = machine("bob", "b", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    let create = mario.patch(3, vec![change("drop/a", Some(1), None)], None);
    ledger.insert(create.clone()).unwrap();
    let edit = mario.patch(
        4,
        vec![change("drop/a", Some(2), Some(create.stamp()))],
        None,
    );
    ledger.insert(edit.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, edit.stamp()),
        Rejection::Frozen(_)
    ));
    let forced = mario.patch(
        5,
        vec![change("drop/a", None, Some(create.stamp()))],
        Some(".pigeon/requests/r"),
    );
    ledger.insert(forced.clone()).unwrap();
    assert!(ledger.outcome(&forced.stamp()).unwrap().is_ok());
    let by_bob = bob.patch(6, vec![change("drop/a", Some(3), None)], None);
    ledger.insert(by_bob.clone()).unwrap();
    assert!(ledger.outcome(&by_bob.stamp()).unwrap().is_ok());
    assert_eq!(ledger.head(&key("drop/a")).unwrap().owner.as_str(), "bob");
}

#[test]
fn a_patch_is_accepted_or_rejected_whole() {
    let mario = machine("mario", "a", 1);
    let bob = machine("bob", "b", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    ledger
        .insert(mario.patch(3, vec![change("_mario/a", Some(1), None)], None))
        .unwrap();
    let mixed = bob.patch(
        4,
        vec![
            change("_bob/a", Some(2), None),
            change("_mario/a", None, None),
        ],
        None,
    );
    ledger.insert(mixed).unwrap();
    assert!(ledger.head(&key("_bob/a")).is_none());
    assert!(ledger.head(&key("_mario/a")).unwrap().is_live());
}

#[test]
fn a_name_belongs_to_its_first_password() {
    let mario = machine("mario", "right", 1);
    let laptop = machine("mario", "right", 2);
    let impostor = machine("mario", "wrong", 3);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    let second = laptop.patch(2, vec![change("_mario/x", Some(1), None)], None);
    ledger.insert(second.clone()).unwrap();
    assert!(ledger.outcome(&second.stamp()).unwrap().is_ok());
    let other = impostor.join(3);
    ledger.insert(other.clone()).unwrap();
    assert_eq!(
        rejection(&ledger, other.stamp()),
        Rejection::OtherKey(mario.cert.name.clone())
    );
    let foreign = laptop.patch(4, vec![change(".pigeon/members/bob", Some(1), None)], None);
    ledger.insert(foreign.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, foreign.stamp()),
        Rejection::ForeignMemberFile { .. }
    ));
}

#[test]
fn a_folder_claims_a_name_until_it_is_empty() {
    let mario = machine("mario", "a", 1);
    let build = machine("build", "b", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    let dropped = mario.patch(2, vec![change("docs/_Build/a", Some(1), None)], None);
    ledger.insert(dropped.clone()).unwrap();
    let blocked = build.join(3);
    ledger.insert(blocked.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, blocked.stamp()),
        Rejection::ClaimedByFolder(_)
    ));
    ledger
        .insert(mario.patch(
            4,
            vec![change("docs/_Build/a", None, Some(dropped.stamp()))],
            Some("r"),
        ))
        .unwrap();
    let joined = build.join(5);
    ledger.insert(joined.clone()).unwrap();
    assert!(ledger.outcome(&joined.stamp()).unwrap().is_ok());
    let intrusion = mario.patch(6, vec![change("docs/_build/b", Some(1), None)], None);
    ledger.insert(intrusion.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, intrusion.stamp()),
        Rejection::NotOwner { .. }
    ));
}

#[test]
fn between_one_members_machines_the_later_change_wins() {
    let desktop = machine("mario", "a", 1);
    let laptop = machine("mario", "a", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(desktop.join(1)).unwrap();
    let base = desktop.patch(2, vec![change("_mario/a", Some(1), None)], None);
    ledger.insert(base.clone()).unwrap();
    let on_laptop = laptop.patch(
        4,
        vec![change("_mario/a", Some(3), Some(base.stamp()))],
        None,
    );
    let on_desktop = desktop.patch(
        3,
        vec![change("_mario/a", Some(2), Some(base.stamp()))],
        None,
    );
    ledger.insert(on_laptop.clone()).unwrap();
    ledger.insert(on_desktop.clone()).unwrap();
    let key = key("_mario/a");
    assert_eq!(ledger.head(&key).unwrap().content, Some(content(3)));
    let unseen: Vec<Stamp> = ledger
        .unseen_versions(&key)
        .iter()
        .map(|v| v.stamp)
        .collect();
    assert_eq!(unseen, vec![on_desktop.stamp()]);
    assert_eq!(
        ledger.version_at(&key, 3).unwrap().content,
        Some(content(2))
    );
    assert!(ledger.version_at(&key, 1).is_none());
}

#[test]
fn every_arrival_order_gives_the_same_tree() {
    let mario = machine("mario", "a", 1);
    let bob = machine("bob", "b", 2);
    let patches = vec![
        mario.join(1),
        bob.join(2),
        bob.patch(3, vec![change("x", Some(1), None)], None),
        mario.patch(
            4,
            vec![
                change("_mario/y", Some(2), None),
                change("X", Some(3), None),
            ],
            None,
        ),
        mario.patch(5, vec![change("_mario/y", None, None)], None),
        bob.patch(6, vec![change("_bob/z", Some(4), None)], None),
        bob.rebind(7, "mario", None),
        mario.patch(8, vec![change("_mario/w", Some(5), None)], None),
    ];
    let reference = {
        let mut ledger = Ledger::new(group());
        for patch in &patches {
            ledger.insert(patch.clone()).unwrap();
        }
        ledger
    };
    let heads = |ledger: &Ledger| {
        let mut heads: Vec<(PathKey, Version)> = ledger
            .keys()
            .map(|key| (key.clone(), ledger.head(key).unwrap().clone()))
            .collect();
        heads.sort_by(|a, b| a.0.cmp(&b.0));
        heads
    };
    for rotation in 0..patches.len() {
        let mut order: Vec<&SignedPatch> = patches
            .iter()
            .cycle()
            .skip(rotation)
            .take(patches.len())
            .collect();
        for _ in 0..2 {
            let mut ledger = Ledger::new(group());
            for patch in &order {
                ledger.insert((*patch).clone()).unwrap();
            }
            assert_eq!(heads(&ledger), heads(&reference));
            assert_eq!(ledger.members(), reference.members());
            order.reverse();
        }
    }
    assert_eq!(reference.head(&key("x")).unwrap().owner.as_str(), "bob");
    assert!(reference.head(&key("_mario/w")).is_none());
}

#[test]
fn a_vector_names_exactly_the_missing_patches() {
    let mario = machine("mario", "a", 1);
    let bob = machine("bob", "b", 2);
    let mut ledger = Ledger::new(group());
    for patch in [
        mario.join(1),
        bob.join(2),
        mario.patch(3, vec![], None),
        bob.patch(4, vec![], None),
    ] {
        ledger.insert(patch).unwrap();
    }
    let mut vector = BTreeMap::new();
    vector.insert(mario.key.public(), 1);
    let missing: Vec<u64> = ledger
        .missing_from(&vector)
        .iter()
        .map(|p| p.stamp().time)
        .collect();
    assert_eq!(missing, vec![2, 3, 4]);
    assert!(ledger.missing_from(&ledger.vector()).is_empty());
}

#[test]
fn a_forged_patch_is_refused() {
    let mario = machine("mario", "a", 1);
    let mut forged = mario.join(1);
    forged.patch.stamp.time = 2;
    assert_eq!(
        Ledger::new(group()).insert(forged),
        Err(SignatureError::Patch)
    );
}

#[test]
fn a_new_password_rebinds_the_name_and_retires_the_old_key() {
    let old = machine("mario", "old", 1);
    let new = machine("mario", "new", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(old.join(1)).unwrap();
    ledger
        .insert(old.patch(2, vec![change("_mario/a", Some(1), None)], None))
        .unwrap();
    let early = new.patch(3, vec![change("_mario/b", Some(1), None)], None);
    ledger.insert(early.clone()).unwrap();
    assert_eq!(
        rejection(&ledger, early.stamp()),
        Rejection::OtherKey(old.cert.name.clone())
    );
    ledger
        .insert(old.rebind(4, "mario", Some(new.cert.member)))
        .unwrap();
    assert!(ledger.recognizes(&new.cert));
    assert!(!ledger.recognizes(&old.cert));
    let stale = old.patch(5, vec![change("_mario/a", Some(2), None)], None);
    ledger.insert(stale.clone()).unwrap();
    assert_eq!(
        rejection(&ledger, stale.stamp()),
        Rejection::OtherKey(old.cert.name.clone())
    );
    let edit = new.patch(6, vec![change("_mario/a", Some(3), None)], None);
    ledger.insert(edit.clone()).unwrap();
    assert!(ledger.outcome(&edit.stamp()).unwrap().is_ok());
    let member = &ledger.members()[&old.cert.name];
    assert_eq!(member.joined, old.stamp(1));
    assert_eq!(member.rebound.as_ref().unwrap().by.as_str(), "mario");
    assert_eq!(ledger.last_exclusion(), None);
}

#[test]
fn any_member_resets_a_password_or_excludes_without_a_vote() {
    let mario = machine("mario", "a", 1);
    let bob = machine("bob", "b", 2);
    let reset = machine("mario", "given", 3);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    ledger
        .insert(mario.patch(3, vec![change("_mario/a", Some(1), None)], None))
        .unwrap();
    ledger
        .insert(bob.rebind(4, "mario", Some(reset.cert.member)))
        .unwrap();
    assert!(ledger.recognizes(&reset.cert));
    assert_eq!(
        ledger.members()[&mario.cert.name]
            .rebound
            .as_ref()
            .unwrap()
            .by
            .as_str(),
        "bob"
    );
    let exclusion = bob.rebind(5, "mario", None);
    ledger.insert(exclusion.clone()).unwrap();
    assert_eq!(ledger.last_exclusion(), Some(exclusion.stamp()));
    assert!(!ledger.recognizes(&reset.cert));
    for patch in [
        reset.patch(6, vec![change("_mario/a", Some(2), None)], None),
        mario.join(7),
    ] {
        ledger.insert(patch.clone()).unwrap();
        assert_eq!(
            rejection(&ledger, patch.stamp()),
            Rejection::Excluded(mario.cert.name.clone())
        );
    }
    let head = ledger.head(&key("_mario/a")).unwrap();
    assert_eq!(
        (head.owner.as_str(), head.content),
        ("mario", Some(content(1)))
    );
    let intrusion = bob.patch(8, vec![change("_mario/b", Some(1), None)], None);
    ledger.insert(intrusion.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, intrusion.stamp()),
        Rejection::NotOwner { .. }
    ));
    let impostor = machine("mario", "z", 4);
    ledger.insert(impostor.join(9)).unwrap();
    assert!(ledger.members()[&mario.cert.name].key.is_none());
}

#[test]
fn a_rebinding_names_a_member_and_a_key() {
    let mario = machine("mario", "a", 1);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    let unknown = mario.rebind(2, "nobody", None);
    ledger.insert(unknown.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, unknown.stamp()),
        Rejection::UnknownMember(_)
    ));
    let stray = mario.patch(
        3,
        vec![change(".pigeon/rebinds/mario/notes.txt", Some(1), None)],
        None,
    );
    ledger.insert(stray.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, stray.stamp()),
        Rejection::Malformed(_)
    ));
    assert!(ledger.recognizes(&mario.cert));
}

#[test]
fn an_earlier_exclusion_undoes_what_the_excluded_member_did_after_it() {
    let mario = machine("mario", "a", 1);
    let bob = machine("bob", "b", 2);
    let late_edit = mario.patch(5, vec![change("_mario/a", Some(1), None)], None);
    let exclusion = bob.rebind(4, "mario", None);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    ledger.insert(late_edit.clone()).unwrap();
    assert!(ledger.head(&key("_mario/a")).is_some());
    ledger.insert(exclusion.clone()).unwrap();
    assert!(ledger.head(&key("_mario/a")).is_none());
    assert_eq!(ledger.last_exclusion(), Some(exclusion.stamp()));
}
