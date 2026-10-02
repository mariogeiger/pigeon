//! Tests of the ledger's guarantees: any member writes any file but
//! another's member file, tags name owners and claim names, a suggestion
//! is decided once, history follows moves, the later of concurrent
//! changes wins, joining, and convergence in any order.

use iroh_base::SecretKey;

use super::*;
use crate::statement::suggestion_path;
use crate::test_machines::*;

#[test]
fn any_member_writes_any_file_and_is_its_author() {
    let mario = machine("mario", 1);
    let bob = machine("bob", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    let create = mario.patch(3, vec![change("+mario/a", Some(1), None)]);
    ledger.insert(create.clone()).unwrap();
    for patch in [
        bob.patch(4, vec![change("+mario/a", Some(2), Some(create.stamp()))]),
        bob.patch(5, vec![change("x/+Mario/b", Some(2), None)]),
        bob.patch(6, vec![change("drop/c", Some(3), None)]),
        mario.patch(7, vec![change("drop/c", Some(4), Some(bob.stamp(6)))]),
    ] {
        ledger.insert(patch.clone()).unwrap();
        assert!(ledger.outcome(&patch.stamp()).unwrap().is_ok());
    }
    let head = ledger.head(&key("+mario/a")).unwrap();
    assert_eq!(
        (head.author.as_str(), head.content),
        ("bob", Some(content(2)))
    );
    assert_eq!(
        ledger.head(&key("drop/c")).unwrap().author.as_str(),
        "mario"
    );
}

#[test]
fn the_rightmost_tag_naming_a_member_names_the_owner() {
    let mario = machine("mario", 1);
    let bob = machine("bob", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    let owner = |path: &str| {
        ledger
            .owner(&GroupPath::parse(path).unwrap())
            .map(|name| name.as_str().to_owned())
    };
    assert_eq!(owner("+mario/+bob/a").as_deref(), Some("bob"));
    assert_eq!(owner("docs/texte+mario.txt").as_deref(), Some("mario"));
    assert_eq!(owner("+mario/+nobody/a").as_deref(), Some("mario"));
    assert_eq!(owner("drop/a"), None);
}

#[test]
fn the_later_of_concurrent_creations_wins_whatever_the_arrival_order() {
    let mario = machine("mario", 1);
    let bob = machine("bob", 2);
    let late = bob.patch(20, vec![change("Notes.txt", Some(2), None)]);
    let early = mario.patch(10, vec![change("notes.txt", Some(1), None)]);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    ledger.insert(late.clone()).unwrap();
    ledger.insert(early.clone()).unwrap();
    assert!(ledger.outcome(&early.stamp()).unwrap().is_ok());
    assert!(ledger.outcome(&late.stamp()).unwrap().is_ok());
    let head = ledger.head(&key("NOTES.TXT")).unwrap();
    assert_eq!(
        (head.author.as_str(), head.path.as_str()),
        ("bob", "Notes.txt")
    );
    let unseen: Vec<Stamp> = ledger
        .unseen_versions(&key("notes.txt"))
        .iter()
        .map(|version| version.stamp)
        .collect();
    assert_eq!(unseen, vec![early.stamp()]);
}

#[test]
fn a_suggestion_is_decided_once_and_as_it_was_seen() {
    let mario = machine("mario", 1);
    let bob = machine("bob", 2);
    let emmy = machine("emmy", 3);
    let mut ledger = Ledger::new(group());
    for (time, member) in (1..).zip([&mario, &bob, &emmy]) {
        ledger.insert(member.join(time)).unwrap();
    }
    let path = suggestion_path(&mario.stamp(10));
    let made = mario.patch(10, vec![change(path.as_str(), Some(1), None)]);
    ledger.insert(made.clone()).unwrap();
    let validate = bob.patch(
        11,
        vec![
            change(path.as_str(), None, Some(made.stamp())),
            change("drop/a", Some(5), None),
        ],
    );
    let discard = emmy.patch(12, vec![change(path.as_str(), None, Some(made.stamp()))]);
    let late_validate = emmy.patch(
        13,
        vec![
            change(path.as_str(), None, Some(made.stamp())),
            change("drop/b", Some(6), None),
        ],
    );
    for patch in [&late_validate, &discard, &validate] {
        ledger.insert(patch.clone()).unwrap();
    }
    assert!(ledger.outcome(&validate.stamp()).unwrap().is_ok());
    for patch in [&discard, &late_validate] {
        assert_eq!(
            rejection(&ledger, patch.stamp()),
            Rejection::AlreadyDecided(path.clone())
        );
    }
    assert!(ledger.head(&key("drop/a")).unwrap().is_live());
    assert!(ledger.head(&key("drop/b")).is_none());
    let renewed = suggestion_path(&mario.stamp(20));
    let first = mario.patch(20, vec![change(renewed.as_str(), Some(1), None)]);
    let update = mario.patch(
        21,
        vec![change(renewed.as_str(), Some(2), Some(first.stamp()))],
    );
    let stale = bob.patch(
        22,
        vec![change(renewed.as_str(), None, Some(first.stamp()))],
    );
    for patch in [&first, &update, &stale] {
        ledger.insert(patch.clone()).unwrap();
    }
    assert_eq!(
        rejection(&ledger, stale.stamp()),
        Rejection::AlreadyDecided(renewed.clone())
    );
    assert!(ledger.head(&renewed.key()).unwrap().is_live());
}

#[test]
fn history_follows_a_file_across_its_moves() {
    let mario = machine("mario", 1);
    let bob = machine("bob", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    let create = mario.patch(3, vec![change("a", Some(1), None)]);
    let unrelated = bob.patch(4, vec![change("b", Some(9), None)]);
    let gone = bob.patch(5, vec![change("b", None, Some(unrelated.stamp()))]);
    let moved = mario.patch(6, moved("a", create.stamp(), "b", 1));
    let edit = bob.patch(7, vec![change("b", Some(2), Some(moved.stamp()))]);
    for patch in [&create, &unrelated, &gone, &moved, &edit] {
        ledger.insert(patch.clone()).unwrap();
        assert!(ledger.outcome(&patch.stamp()).unwrap().is_ok());
    }
    let lineage: Vec<(&str, u64)> = ledger
        .lineage(&key("b"))
        .iter()
        .map(|version| (version.path.as_str(), version.stamp.time))
        .collect();
    assert_eq!(lineage, [("b", 7), ("b", 6), ("a", 3)]);
    assert!(!ledger.head(&key("a")).unwrap().is_live());
    let ahead = mario.patch(
        8,
        vec![Change {
            continues: Some(VersionRef {
                path: GroupPath::parse("c").unwrap(),
                stamp: mario.stamp(9),
            }),
            ..change("c", Some(3), None)
        }],
    );
    ledger.insert(ahead).unwrap();
    assert_eq!(ledger.lineage(&key("c")).len(), 1);
}

#[test]
fn a_patch_is_accepted_or_rejected_whole() {
    let mario = machine("mario", 1);
    let bob = machine("bob", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    ledger
        .insert(mario.patch(3, vec![change("+mario/a", Some(1), None)]))
        .unwrap();
    let mixed = bob.patch(
        4,
        vec![
            change("+bob/a", Some(2), None),
            change(".pigeon/members/mario", None, Some(mario.stamp(1))),
        ],
    );
    ledger.insert(mixed).unwrap();
    assert!(ledger.head(&key("+bob/a")).is_none());
    assert!(ledger.head(&key("+mario/a")).unwrap().is_live());
}

#[test]
fn a_name_belongs_to_its_first_key() {
    let mario = machine("mario", 1);
    let laptop = machine("mario", 2);
    let impostor = keyed_machine("mario", &SecretKey::from_bytes(&[40; 32]), 3);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    let second = laptop.patch(2, vec![change("+mario/x", Some(1), None)]);
    ledger.insert(second.clone()).unwrap();
    assert!(ledger.outcome(&second.stamp()).unwrap().is_ok());
    let other = impostor.join(3);
    ledger.insert(other.clone()).unwrap();
    assert_eq!(
        rejection(&ledger, other.stamp()),
        Rejection::OtherKey(mario.cert.name.clone())
    );
    let foreign = laptop.patch(4, vec![change(".pigeon/members/bob", Some(1), None)]);
    ledger.insert(foreign.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, foreign.stamp()),
        Rejection::ForeignMemberFile { .. }
    ));
}

#[test]
fn a_tag_claims_a_name_until_no_path_bears_it() {
    let mario = machine("mario", 1);
    let build = machine("build", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    let dropped = mario.patch(2, vec![change("docs/+Build/a", Some(1), None)]);
    ledger.insert(dropped.clone()).unwrap();
    assert_eq!(
        ledger
            .tag_claims()
            .map(MemberName::as_str)
            .collect::<Vec<_>>(),
        ["build"]
    );
    let blocked = build.join(3);
    ledger.insert(blocked.clone()).unwrap();
    assert!(matches!(
        rejection(&ledger, blocked.stamp()),
        Rejection::ClaimedByTag(_)
    ));
    ledger
        .insert(mario.patch(
            4,
            vec![change("docs/+Build/a", None, Some(dropped.stamp()))],
        ))
        .unwrap();
    assert_eq!(ledger.tag_claims().count(), 0);
    let joined = build.join(5);
    ledger.insert(joined.clone()).unwrap();
    assert!(ledger.outcome(&joined.stamp()).unwrap().is_ok());
    assert_eq!(
        ledger
            .owner(&GroupPath::parse("docs/+build/b").unwrap())
            .unwrap()
            .as_str(),
        "build"
    );
}

#[test]
fn between_one_members_machines_the_later_change_wins() {
    let desktop = machine("mario", 1);
    let laptop = machine("mario", 2);
    let mut ledger = Ledger::new(group());
    ledger.insert(desktop.join(1)).unwrap();
    let base = desktop.patch(2, vec![change("+mario/a", Some(1), None)]);
    ledger.insert(base.clone()).unwrap();
    let on_laptop = laptop.patch(4, vec![change("+mario/a", Some(3), Some(base.stamp()))]);
    let on_desktop = desktop.patch(3, vec![change("+mario/a", Some(2), Some(base.stamp()))]);
    ledger.insert(on_laptop.clone()).unwrap();
    ledger.insert(on_desktop.clone()).unwrap();
    let key = key("+mario/a");
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
    let mario = machine("mario", 1);
    let bob = machine("bob", 2);
    let patches = vec![
        mario.join(1),
        bob.join(2),
        bob.patch(3, vec![change("x", Some(1), None)]),
        mario.patch(
            4,
            vec![
                change("+mario/y", Some(2), None),
                change("X", Some(3), None),
            ],
        ),
        mario.patch(5, vec![change("+mario/y", None, None)]),
        bob.patch(6, vec![change("+bob/z", Some(4), None)]),
        bob.patch(7, vec![change(".pigeon/members/mario", None, None)]),
        mario.patch(8, vec![change("+mario/w", Some(5), None)]),
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
    assert_eq!(reference.head(&key("x")).unwrap().author.as_str(), "mario");
    assert!(reference.head(&key("+mario/w")).is_some());
    assert!(reference.outcome(&patches[6].stamp()).unwrap().is_err());
}

#[test]
fn a_vector_names_exactly_the_missing_patches() {
    let mario = machine("mario", 1);
    let bob = machine("bob", 2);
    let mut ledger = Ledger::new(group());
    for patch in [
        mario.join(1),
        bob.join(2),
        mario.patch(3, vec![]),
        bob.patch(4, vec![]),
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
    let mario = machine("mario", 1);
    let mut forged = mario.join(1);
    forged.patch.stamp.time = 2;
    assert_eq!(
        Ledger::new(group()).insert(forged),
        Err(SignatureError::Patch)
    );
}

#[test]
fn a_rebinding_file_of_an_older_pigeon_binds_no_name() {
    let (mario, bob) = (machine("mario", 1), machine("bob", 2));
    let mut ledger = Ledger::new(group());
    ledger.insert(mario.join(1)).unwrap();
    ledger.insert(bob.join(2)).unwrap();
    let exclusion = bob.patch(
        3,
        vec![change(".pigeon/rebinds/mario/1-none", Some(1), None)],
    );
    ledger.insert(exclusion.clone()).unwrap();
    assert!(ledger.outcome(&exclusion.stamp()).unwrap().is_ok());
    let later = mario.patch(4, vec![change("notes.txt", Some(2), None)]);
    ledger.insert(later.clone()).unwrap();
    assert!(ledger.outcome(&later.stamp()).unwrap().is_ok());
    assert!(ledger.recognizes(&mario.cert));
}
