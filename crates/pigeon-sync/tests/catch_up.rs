//! Tests of how patches reach every machine: through a machine between two
//! that cannot reach each other, which tell why; once the clock of the
//! machine that refused them as dated too far ahead caught up; and from a
//! machine restarted with its clock behind its own patches, which still
//! dates its new patches after them and says so.

mod common;

use std::time::{Duration, SystemTime};

use common::*;
use iroh::address_lookup::MemoryLookup;
use pigeon_core::clock::{Stamp, ntp_time};
use pigeon_core::identity::MachineCert;
use pigeon_core::name::MemberName;
use pigeon_core::patch::{Patch, SignedPatch};
use pigeon_store::group_dirs::GroupDirs;
use pigeon_store::state::State;

#[tokio::test]
async fn patches_pass_through_a_machine_between_two_that_cannot_reach_each_other() {
    let near = MemoryLookup::new();
    let mut machines = group_on(&near, &["bob", "alice"], |_| {}).await;
    joined(&machines).await;
    let far = MemoryLookup::new();
    let bob = machines[0].engine.machine();
    far.add_endpoint_info(near.get_endpoint_info(bob).unwrap());
    let key = machines[0].engine.group_key().parse().unwrap();
    machines.push(start_with(key, "carol", options(&far)).await);
    joined(&machines).await;
    let (alice, carol) = (&machines[1], &machines[2]);
    alice.edit("+alice/note.txt", "from alice");
    carol.edit("+carol/note.txt", "from carol");
    eventually("each edit reaches the other side through bob", || async {
        carol.engine.history(&path("+alice/note.txt")).len() == 1
            && alice.engine.history(&path("+carol/note.txt")).len() == 1
    })
    .await;
    let carol_id = carol.engine.machine();
    eventually("alice tells why she cannot reach carol", || async {
        let status = alice.engine.status();
        !status.peers.contains(&carol_id)
            && status.unreached.iter().any(|unreached| {
                unreached.machine == carol_id
                    && unreached.member.as_ref().map(MemberName::as_str) == Some("carol")
            })
    })
    .await;
    shut_down(machines).await;
}

/// Stores in the state of the stopped machine of `member` a patch of its
/// own, dated `ahead` after now, as if its clock was set back since.
fn date_own_patch_ahead(dirs: &GroupDirs, member: &str, ahead: Duration) -> u64 {
    let secrets = dirs.secrets().unwrap();
    let group = secrets.key.unwrap().group;
    let machine = secrets.machine;
    let cert = MachineCert::derive(&group, MemberName::parse(member).unwrap(), machine.public());
    let time = ntp_time(SystemTime::now() + ahead);
    let patch = Patch {
        stamp: Stamp {
            time,
            machine: machine.public(),
        },
        changes: Vec::new(),
    };
    let signed = SignedPatch::sign(&group, patch, cert, &machine);
    State::open(&dirs.state_path())
        .unwrap()
        .add_patch(&signed)
        .unwrap();
    time
}

#[tokio::test]
async fn a_machine_whose_clock_was_set_back_dates_its_patches_after_its_own() {
    let machines = group(&["mario", "bob"]).await;
    joined(&machines).await;
    let [mario, bob]: [Machine; 2] = machines.try_into().ok().unwrap();
    let mut ahead = 0;
    let bob = bob
        .restart_after(|dirs, _| ahead = date_own_patch_ahead(dirs, "bob", Duration::from_secs(60)))
        .await;
    eventually("bob says his clock lags", || async {
        bob.engine
            .status()
            .errors
            .iter()
            .any(|error| error.contains("after its clock"))
    })
    .await;
    bob.edit("+bob/late.txt", "late");
    eventually("bob publishes his edit", || async {
        bob.engine.history(&path("+bob/late.txt")).len() == 1
    })
    .await;
    let history = bob.engine.history(&path("+bob/late.txt"));
    assert!(history[0].stamp.time > ahead);
    shut_down([mario, bob]).await;
}

#[tokio::test]
async fn patches_refused_as_dated_too_far_ahead_arrive_once_the_clock_caught_up() {
    let machines = group_with(&["mario", "bob"], |options| {
        options.max_drift = Duration::from_secs(1);
    })
    .await;
    joined(&machines).await;
    let [mario, bob]: [Machine; 2] = machines.try_into().ok().unwrap();
    let bob = bob
        .restart_after(|dirs, _| {
            date_own_patch_ahead(dirs, "bob", Duration::from_secs(4));
        })
        .await;
    bob.edit("+bob/early.txt", "early");
    eventually("mario refuses bob's patches for now", || async {
        mario
            .engine
            .status()
            .errors
            .iter()
            .any(|error| error.contains("ahead of this machine's clock"))
    })
    .await;
    assert!(mario.engine.history(&path("+bob/early.txt")).is_empty());
    eventually(
        "bob's patches arrive once mario's clock caught up",
        || async { mario.engine.history(&path("+bob/early.txt")).len() == 1 },
    )
    .await;
    shut_down([mario, bob]).await;
}
