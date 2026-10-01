//! Tests of excluding across machines: excluding or leaving renews the
//! group secret, which then admits new machines while the old one admits
//! none, even on the excluded machine.

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

fn secret(machine: &Machine) -> GroupSecret {
    machine.data.secrets().unwrap().key.unwrap().secret
}

fn secret_of(key: &str) -> GroupSecret {
    key.parse::<GroupKey>().unwrap().secret
}

#[tokio::test(flavor = "multi_thread")]
async fn excluding_renews_the_secret_that_admits_new_machines() {
    let machines = group(&["alice", "bob"]).await;
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
    let late = bob.join_with(&old_key, "carol").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(late.engine.status().await.peers.is_empty());
    assert!(
        !bob.engine
            .members()
            .iter()
            .any(|member| member.name == name("carol"))
    );
    let dave = bob.join_with(&new_key, "dave").await;
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
    let machines = group(&["alice", "bob"]).await;
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
