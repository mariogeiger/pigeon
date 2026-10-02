//! Properties of the ledger over seeded random histories of every kind of
//! patch: joins, a second key claiming a taken name, a name a tag claims,
//! writes and deletions seen or unseen, moves, tagged paths, member files,
//! suggestions, their merges and their decisions. Folding a patch and then
//! undoing it gives back the state it landed on, and every arrival order,
//! one patch or many at a time, folds into the same state.

use iroh_base::SecretKey;

use super::*;
use crate::statement::{is_statement, suggestion_path};
use crate::test_machines::*;

const SEEDS: u64 = 256;
const PATCHES: usize = 48;
const PATHS: [&str; 9] = [
    "a", "A", "x/b", "x/c", "+mario/p", "+bob/q", "+zed/r", "d/+ana/s", "X/B",
];

/// The choices one history makes, drawn from its seed by splitmix64.
struct Choices(u64);

impl Choices {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % bound as u64).unwrap()
    }

    fn chance(&mut self, numerator: usize, denominator: usize) -> bool {
        self.below(denominator) < numerator
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for index in (1..items.len()).rev() {
            items.swap(index, self.below(index + 1));
        }
    }
}

/// The machines of the histories: two of mario's, bob's, ana's, another
/// key claiming the name bob, and zed's, whose name a tag may claim.
fn machines() -> Vec<Machine> {
    vec![
        machine("mario", 1),
        machine("mario", 5),
        machine("bob", 2),
        machine("ana", 3),
        keyed_machine("bob", &SecretKey::from_bytes(&[66; 32]), 4),
        machine("zed", 6),
    ]
}

/// The stamp a change replacing the head of `path` names: mostly the head
/// it saw, else an older version or nothing, as a machine that had not
/// seen the latest would.
fn replaced(choices: &mut Choices, known: &Ledger, path: &GroupPath) -> Option<Stamp> {
    let versions = known.versions(&path.key());
    match versions.last() {
        Some(head) if choices.chance(3, 4) => Some(head.stamp),
        Some(_) if choices.chance(1, 2) => Some(choices.pick(versions).stamp),
        _ => None,
    }
}

/// The versions `known` holds that a change may act on, in stamp order.
fn live(known: &Ledger, wanted: impl Fn(&Version) -> bool) -> Vec<Version> {
    let mut live: Vec<Version> = known
        .live()
        .filter(|version| wanted(version))
        .cloned()
        .collect();
    live.sort_by_key(|version| version.stamp);
    live
}

/// One change, or the two of a move, that `author` makes at `stamp` having
/// seen `known`.
fn changes(choices: &mut Choices, author: &Machine, stamp: Stamp, known: &Ledger) -> Vec<Change> {
    let byte = u8::try_from(choices.below(8)).unwrap() + 1;
    let pool = |choices: &mut Choices| GroupPath::parse(choices.pick(&PATHS)).unwrap();
    let with = |path: GroupPath, content: Option<u8>, replaces: Option<Stamp>| Change {
        path,
        content: content.map(crate::test_machines::content),
        replaces,
        continues: None,
    };
    match choices.below(10) {
        0 | 1 => {
            let path = pool(choices);
            let replaces = replaced(choices, known, &path);
            vec![with(path, Some(byte), replaces)]
        }
        2 => {
            let path = pool(choices);
            let replaces = replaced(choices, known, &path);
            vec![with(path, None, replaces)]
        }
        3 => {
            let files = live(known, |version| !is_statement(&version.path.key()));
            if files.is_empty() {
                return vec![with(pool(choices), Some(byte), None)];
            }
            let from = choices.pick(&files).clone();
            let elsewhere: Vec<&str> = PATHS
                .into_iter()
                .filter(|to| GroupPath::parse(to).unwrap().key() != from.path.key())
                .collect();
            moved(
                from.path.as_str(),
                from.stamp,
                choices.pick(&elsewhere),
                byte,
            )
        }
        4 => {
            let name = &choices.pick(&machines()).cert.name.clone();
            let path = member_path(if choices.chance(2, 3) {
                &author.cert.name
            } else {
                name
            });
            let replaces = replaced(choices, known, &path);
            vec![with(path, choices.chance(4, 5).then_some(0), replaces)]
        }
        5 | 6 => vec![with(suggestion_path(&stamp), Some(byte), None)],
        _ => {
            let suggestions = live(known, |version| is_suggestion_path(&version.path));
            if suggestions.is_empty() {
                return vec![with(suggestion_path(&stamp), Some(byte), None)];
            }
            let path = choices.pick(&suggestions).path.clone();
            let replaces = replaced(choices, known, &path);
            let content = choices.chance(1, 3).then_some(byte);
            vec![with(path, content, replaces)]
        }
    }
}

/// The history of `seed`: patches by random machines at random times,
/// each made having seen the ones before it, some with two changes.
fn history(seed: u64) -> Vec<SignedPatch> {
    let machines = machines();
    let mut choices = Choices(seed);
    let mut known = Ledger::new(group());
    let mut stamps = BTreeSet::new();
    let mut time = 0;
    let mut patches = Vec::new();
    for _ in 0..PATCHES {
        let author = choices.pick(&machines);
        time += u64::try_from(choices.below(2)).unwrap();
        let mut stamp = author.stamp(time.max(1));
        while !stamps.insert(stamp) {
            stamp.time += 1;
        }
        let mut made = if known.recognizes(&author.cert) || choices.chance(1, 5) {
            changes(&mut choices, author, stamp, &known)
        } else {
            vec![change(
                member_path(&author.cert.name).as_str(),
                Some(0),
                None,
            )]
        };
        if choices.chance(1, 4) {
            for other in changes(&mut choices, author, stamp, &known) {
                if made
                    .iter()
                    .all(|change| change.path.key() != other.path.key())
                {
                    made.push(other);
                }
            }
        }
        let patch = author.patch(stamp.time, made);
        known.insert(patch.clone()).unwrap();
        patches.push(patch);
    }
    patches
}

/// Everything a state holds, in an order that does not depend on how it
/// was built.
#[derive(PartialEq, Eq, Debug)]
struct Dump {
    outcomes: Vec<(Stamp, Result<(), Rejection>)>,
    files: Vec<(PathKey, Vec<Version>)>,
    members: Vec<(MemberName, Member)>,
    claims: Vec<(MemberName, usize)>,
}

fn dump(state: &State) -> Dump {
    let mut files: Vec<(PathKey, Vec<Version>)> = state
        .files
        .iter()
        .map(|(key, versions)| (key.clone(), versions.clone()))
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut claims: Vec<(MemberName, usize)> = state
        .claims
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(name, count)| (name.clone(), *count))
        .collect();
    claims.sort();
    Dump {
        outcomes: state
            .outcomes
            .iter()
            .map(|(stamp, outcome)| {
                let outcome = match outcome {
                    Outcome::Accepted(_) => Ok(()),
                    Outcome::Rejected(rejection) => Err(rejection.clone()),
                };
                (*stamp, outcome)
            })
            .collect(),
        files,
        members: state
            .members
            .iter()
            .map(|(name, member)| (name.clone(), member.clone()))
            .collect(),
        claims,
    }
}

#[test]
fn folding_a_patch_then_undoing_it_gives_back_the_state_it_landed_on() {
    for seed in 0..SEEDS {
        let mut patches = history(seed);
        patches.sort_by_key(SignedPatch::stamp);
        let mut state = State::default();
        let mut dumps = vec![dump(&state)];
        for patch in &patches {
            state.fold(patch);
            state.undo(&patch.stamp());
            assert_eq!(dump(&state), *dumps.last().unwrap(), "seed {seed}");
            state.fold(patch);
            dumps.push(dump(&state));
        }
        for patch in patches.iter().rev() {
            dumps.pop();
            state.undo(&patch.stamp());
            assert_eq!(dump(&state), *dumps.last().unwrap(), "seed {seed}");
        }
    }
}

#[test]
fn every_arrival_order_one_patch_or_many_at_a_time_folds_into_the_same_state() {
    for seed in 0..SEEDS {
        let patches = history(seed);
        let mut reference = Ledger::new(group());
        reference.extend(patches.clone());
        let expected = dump(&reference.state);
        let mut choices = Choices(seed ^ 0xA5A5);
        for _ in 0..3 {
            let mut order = patches.clone();
            choices.shuffle(&mut order);
            let mut one_by_one = Ledger::new(group());
            for patch in order.clone() {
                one_by_one.insert(patch).unwrap();
            }
            let mut batches = Ledger::new(group());
            while !order.is_empty() {
                let take = (choices.below(6) + 1).min(order.len());
                batches.extend(order.drain(..take));
            }
            for ledger in [&one_by_one, &batches] {
                assert_eq!(dump(&ledger.state), expected, "seed {seed}");
                assert_eq!(ledger.digests(), reference.digests(), "seed {seed}");
                assert!(ledger.missing_from(&reference.digests()).is_empty());
                assert!(reference.missing_from(&ledger.digests()).is_empty());
            }
        }
    }
}

#[test]
fn the_histories_reach_every_outcome_and_every_kind_of_accepted_change() {
    let mut rejections = std::collections::HashSet::new();
    let (mut moves, mut decisions, mut tags, mut joins) = (0, 0, 0, 0);
    for seed in 0..SEEDS {
        let mut ledger = Ledger::new(group());
        ledger.extend(history(seed));
        for (stamp, outcome) in &ledger.state.outcomes {
            match outcome {
                Outcome::Rejected(rejection) => {
                    rejections.insert(std::mem::discriminant(rejection));
                }
                Outcome::Accepted(effects) => {
                    joins += usize::from(effects.joined.is_some());
                    tags += usize::from(!effects.claims.is_empty());
                    let changes = &ledger.patches[stamp].patch.changes;
                    moves += changes
                        .iter()
                        .filter(|change| change.continues.is_some())
                        .count();
                    decisions += changes
                        .iter()
                        .filter(|change| {
                            is_suggestion_path(&change.path) && change.content.is_none()
                        })
                        .count();
                }
            }
        }
    }
    let name = MemberName::parse("bob").unwrap();
    let every_rejection = [
        Rejection::UnknownMember(name.clone()),
        Rejection::OtherKey(name.clone()),
        Rejection::ClaimedByTag(name.clone()),
        Rejection::ForeignMemberFile {
            path: member_path(&name),
            owner: name,
        },
        Rejection::AlreadyDecided(GroupPath::parse("x").unwrap()),
    ];
    for rejection in &every_rejection {
        assert!(
            rejections.contains(&std::mem::discriminant(rejection)),
            "no history meets {rejection:?}"
        );
    }
    assert!(moves > 0 && decisions > 0 && tags > 0 && joins > 0);
}
