//! Tests of rebinding names across machines: a new password retires the
//! old one on every machine until it logs in again, any member resets a
//! password, and excluding or leaving renews the group secret, which then
//! admits new machines while the old one admits none, even on the excluded
//! machine.

mod common;

use std::time::Duration;

use common::{Machine, eventually, group, joined};
use pigeon_core::identity::GroupSecret;
use pigeon_core::name::MemberName;
use pigeon_store::group_key::GroupKey;
use pigeon_sync::JoinState;

fn name(text: &str) -> MemberName {
    MemberName::parse(text).unwrap()
}

async fn state(machine: &Machine) -> JoinState {
    machine.engine.status().await.join
}

async fn rebound_by(machine: &Machine, by: &str) -> bool {
    matches!(state(machine).await, JoinState::Rebound(reason) if reason.starts_with(by))
}

fn secret(machine: &Machine) -> GroupSecret {
    machine.data.load_config().unwrap().key.secret
}

fn secret_of(key: &str) -> GroupSecret {
    key.parse::<GroupKey>().unwrap().secret
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_password_logs_out_every_machine_until_it_logs_in_with_it() {
    let machines = group(&[("alice", "a"), ("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [desktop, laptop, bob]: [Machine; 3] = machines.try_into().ok().unwrap();
    desktop
        .engine
        .set_password(&name("alice"), "new")
        .await
        .unwrap();
    assert!(rebound_by(&desktop, "alice").await);
    eventually("the laptop learns of the new password", || {
        rebound_by(&laptop, "alice")
    })
    .await;
    let desktop = desktop.log_in("new").await;
    eventually("the new password logs the desktop in", || async {
        state(&desktop).await == JoinState::Joined
    })
    .await;
    bob.engine
        .set_password(&name("alice"), "given")
        .await
        .unwrap();
    eventually("bob's reset logs the desktop out", || {
        rebound_by(&desktop, "bob")
    })
    .await;
    let laptop = laptop.log_in("given").await;
    eventually("the given password logs the laptop in", || async {
        state(&laptop).await == JoinState::Joined
    })
    .await;
    let alice = bob
        .engine
        .members()
        .into_iter()
        .find(|member| member.name == name("alice"))
        .unwrap();
    assert_eq!(alice.rebound.unwrap().by, name("bob"));
    for machine in [desktop, laptop, bob] {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn excluding_renews_the_secret_that_admits_new_machines() {
    let machines = group(&[("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [alice, bob]: [Machine; 2] = machines.try_into().ok().unwrap();
    let old_key = bob.engine.group_key();
    bob.engine.exclude(&name("alice")).await.unwrap();
    let new_key = bob.engine.group_key();
    assert_ne!(new_key, old_key);
    eventually("the renewed secret is kept", || async {
        secret(&bob) == secret_of(&new_key)
    })
    .await;
    eventually("alice learns she was excluded", || async {
        matches!(state(&alice).await, JoinState::Excluded(reason) if reason.starts_with("bob"))
    })
    .await;
    assert_eq!(secret(&alice), secret_of(&old_key));
    let late = bob.join_with(&old_key, "carol", "c").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(late.engine.status().await.peers.is_empty());
    assert!(
        !bob.engine
            .members()
            .iter()
            .any(|member| member.name == name("carol"))
    );
    let dave = bob.join_with(&new_key, "dave", "d").await;
    eventually("the new secret admits dave", || async {
        bob.engine
            .members()
            .iter()
            .any(|member| member.name == name("dave"))
    })
    .await;
    assert!(
        bob.engine
            .members()
            .iter()
            .any(|member| member.name == name("alice") && member.key.is_none())
    );
    for machine in [late, dave, bob, alice] {
        machine.engine.shutdown().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn leaving_excludes_oneself_and_the_others_renew_the_secret() {
    let machines = group(&[("alice", "a"), ("bob", "b")]).await;
    joined(&machines).await;
    let [alice, bob]: [Machine; 2] = machines.try_into().ok().unwrap();
    let before = secret(&alice);
    alice.engine.exclude(&name("alice")).await.unwrap();
    assert_eq!(
        state(&alice).await,
        JoinState::Excluded("alice left the group".to_owned())
    );
    eventually("bob renews the secret", || async { secret(&bob) != before }).await;
    assert_eq!(secret(&alice), before);
    assert!(alice.engine.exclude(&name("bob")).await.is_err());
    for machine in [alice, bob] {
        machine.engine.shutdown().await.unwrap();
    }
}
