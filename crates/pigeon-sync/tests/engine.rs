//! Engines of one group on this host: files reach the machines that hold
//! them and stay writable everywhere, two machines with one name are one
//! member, a restarted machine resumes without publishing again, the quota
//! drops history, keeping history keeps the past versions of others' files
//! too, a file fetched that the disk cannot take yet stays until it lands,
//! every machine follows the relay the group names, a root reached through
//! a link syncs both ways, an engine with nothing to do looks at the disk
//! no more, and a burst of files takes a look per gathering, not per file.

mod common;

use std::time::Duration;

use common::{
    eventually, eventually_reaches, group, group_with, is_read_only, joined, path, rule, shut_down,
};
use pigeon_core::retention::Retention;
use pigeon_core::selection::Cutoff;
use pigeon_net::relay::{relay_url, serve_relay};
use pigeon_sync::{Edit, JoinState};

#[tokio::test(flavor = "multi_thread")]
async fn a_personal_file_reaches_the_machines_that_hold_it() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    alice.edit("+alice/notes.txt", "one");
    eventually("bob sees alice's file in the ledger", || async {
        bob.engine.history(&path("+alice/notes.txt")).len() == 1
    })
    .await;
    assert_eq!(
        bob.read("+alice/notes.txt"),
        None,
        "bob does not hold +alice yet"
    );
    bob.engine
        .set_rule(rule("+alice/", Cutoff::PlusInfinity))
        .await
        .unwrap();
    eventually("bob holds alice's file", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("one")
    })
    .await;
    assert!(!is_read_only(&bob.file("+alice/notes.txt")));
    assert!(!is_read_only(&alice.file("+alice/notes.txt")));
    assert!(bob.read(".pigeon/members/alice").is_some());
    let listed = bob.engine.list(None).await.unwrap();
    assert!(
        listed
            .iter()
            .all(|file| !file.path.as_str().starts_with(".pigeon")),
        "statements are not listed: {listed:?}"
    );

    alice.edit("+alice/notes.txt", "two");
    eventually("bob follows alice's edit", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("two")
    })
    .await;
    std::fs::remove_file(alice.file("+alice/notes.txt")).unwrap();
    eventually("bob follows alice's deletion", || async {
        bob.read("+alice/notes.txt").is_none()
    })
    .await;

    alice.edit("+alice/kept.txt", "kept");
    eventually("bob holds the new file", || async {
        bob.read("+alice/kept.txt").is_some()
    })
    .await;
    bob.engine
        .set_rule(rule("+alice/", Cutoff::MinusInfinity))
        .await
        .unwrap();
    assert_eq!(
        bob.read("+alice/kept.txt"),
        None,
        "a released file leaves the disk"
    );
    for machine in &machines {
        assert!(machine.engine.status().errors.is_empty());
    }
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn two_machines_with_one_name_are_one_member() {
    let machines = group(&["alice", "alice"]).await;
    joined(&machines).await;
    for machine in &machines {
        assert_eq!(machine.engine.members().len(), 1);
    }
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_machine_resumes_without_publishing_again() {
    let mut machines = group(&["alice"]).await;
    joined(&machines).await;
    let alice = machines.remove(0);
    alice.edit("+alice/a.txt", "a");
    eventually("the file is published", || async {
        alice.engine.history(&path("+alice/a.txt")).len() == 1
    })
    .await;
    let patches = alice.engine.status().patches;
    let alice = alice.restart().await;
    assert_eq!(alice.engine.status().join, JoinState::Joined);
    alice.wait_past_settling().await;
    assert_eq!(alice.engine.status().patches, patches);
    assert_eq!(alice.read("+alice/a.txt").as_deref(), Some("a"));
    alice.edit("+alice/a.txt", "b");
    eventually("an edit after the restart is published", || async {
        alice.engine.history(&path("+alice/a.txt")).len() == 2
    })
    .await;
    alice.engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_quota_drops_past_versions_and_keeps_current_ones() {
    let machines = group(&["alice"]).await;
    joined(&machines).await;
    let alice = &machines[0];
    for text in ["one", "two"] {
        let edit = Edit::Write {
            path: path("+alice/a.txt"),
            bytes: text.as_bytes().to_vec(),
        };
        alice.engine.edit(vec![edit]).await.unwrap();
        eventually("alice's edit lands on her disk", || async {
            alice.read("+alice/a.txt").as_deref() == Some(text)
        })
        .await;
    }
    let history = alice.engine.history(&path("+alice/a.txt"));
    let [old, current] = &history[..] else {
        panic!("two versions: {history:?}")
    };
    let (old, current) = (old.content.unwrap(), current.content.unwrap());
    alice.wait_past_collection().await;
    assert!(alice.engine.read(&old).await.unwrap().is_some());
    let retention = Retention {
        quota_percent: 0,
        ..alice.engine.retention().await
    };
    alice.engine.set_retention(&retention).await.unwrap();
    eventually("the past version goes", || async {
        alice.engine.read(&old).await.unwrap().is_none()
    })
    .await;
    assert_eq!(
        alice.engine.read(&current).await.unwrap().as_deref(),
        Some(&b"two"[..])
    );
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keeping_history_keeps_past_versions_of_others_files() {
    let machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.engine
        .set_rule(rule("+alice/", Cutoff::PlusInfinity))
        .await
        .unwrap();
    let current = bob.engine.retention().await;
    let keep = |everything| Retention {
        everything,
        ..current
    };
    bob.engine.set_retention(&keep(true)).await.unwrap();
    for text in ["one", "two"] {
        alice.edit("+alice/a.txt", text);
        eventually("bob follows alice", || async {
            bob.read("+alice/a.txt").as_deref() == Some(text)
        })
        .await;
    }
    let history = bob.engine.history(&path("+alice/a.txt"));
    let old = history[0].content.unwrap();
    bob.wait_past_collection().await;
    assert!(bob.engine.read(&old).await.unwrap().is_some());
    bob.engine.set_retention(&keep(false)).await.unwrap();
    eventually("without it the past version goes", || async {
        bob.engine.read(&old).await.unwrap().is_none()
    })
    .await;
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    windows,
    ignore = "a read-only folder takes new files on Windows, so nothing holds the file back"
)]
async fn a_fetched_file_the_disk_cannot_take_yet_stays_until_it_lands_with_its_source_gone() {
    let mut machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let (bob, alice) = (machines.pop().unwrap(), machines.pop().unwrap());
    let locked = bob.file("+alice/locked");
    std::fs::create_dir_all(&locked).unwrap();
    let writable = std::fs::metadata(&locked).unwrap().permissions();
    let mut read_only = writable.clone();
    read_only.set_readonly(true);
    std::fs::set_permissions(&locked, read_only).unwrap();
    alice.edit("+alice/locked/plan.txt", "alice's plan");
    bob.follow("+alice/").await;
    eventually("bob fetched the plan but cannot write it", || async {
        bob.engine
            .status()
            .errors
            .iter()
            .any(|error| error.starts_with("+alice/locked/plan.txt"))
    })
    .await;
    alice.edit("+alice/open.txt", "open");
    eventually("bob writes another file", || async {
        bob.read("+alice/open.txt").is_some()
    })
    .await;
    bob.wait_past_collection().await;
    let plan = bob.engine.history(&path("+alice/locked/plan.txt"))[0]
        .content
        .unwrap();
    assert!(bob.engine.read(&plan).await.unwrap().is_some());
    alice.engine.shutdown().await.unwrap();
    std::fs::set_permissions(&locked, writable).unwrap();
    let bob = bob.restart().await;
    eventually("bob writes the plan it holds, alone", || async {
        bob.read("+alice/locked/plan.txt").as_deref() == Some("alice's plan")
    })
    .await;
    bob.engine.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn every_machine_follows_the_relay_the_group_names() {
    let mut machines = group(&["alice", "bob"]).await;
    joined(&machines).await;
    let server = serve_relay("127.0.0.1:0".parse().unwrap(), None)
        .await
        .unwrap();
    let url = relay_url(&server, "127.0.0.1").unwrap().to_string();
    let alice = &machines[0].engine;
    let wrong = alice.set_relay(Some("no url")).await.unwrap_err();
    assert_eq!(wrong.to_string(), "no url is no relay URL");
    alice.set_relay(Some(&url)).await.unwrap();
    for machine in &machines {
        eventually("the machine reaches the group's relay", || async {
            machine.engine.status().relay.as_deref() == Some(url.as_str())
        })
        .await;
    }
    let bob = machines.pop().unwrap().restart().await;
    eventually("a restarted machine follows it at once", || async {
        bob.engine.status().relay.as_deref() == Some(url.as_str())
    })
    .await;
    machines.push(bob);
    shut_down(machines).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_root_reached_through_a_link_syncs_both_ways() {
    let machines = group(&["alice"]).await;
    joined(&machines).await;
    let alice = &machines[0];
    let bob = alice
        .join_through_link(&alice.engine.group_key(), "bob")
        .await;
    joined(std::slice::from_ref(&bob)).await;
    for (machine, folder) in [(alice, "+bob/"), (&bob, "+alice/")] {
        machine
            .engine
            .set_rule(rule(folder, Cutoff::PlusInfinity))
            .await
            .unwrap();
    }
    alice.edit("+alice/notes.txt", "from alice");
    eventually("bob receives through his link", || async {
        bob.read("+alice/notes.txt").as_deref() == Some("from alice")
    })
    .await;
    bob.edit("+bob/notes.txt", "from bob");
    eventually("bob's edit through his link is published", || async {
        alice.read("+bob/notes.txt").as_deref() == Some("from bob")
    })
    .await;
    assert!(bob.engine.status().errors.is_empty());
    shut_down(machines.into_iter().chain([bob])).await;
}

#[tokio::test]
async fn an_engine_with_nothing_to_do_scans_the_disk_no_more() {
    let machines = group(&["alice"]).await;
    joined(&machines).await;
    let alice = &machines[0];
    alice.edit("+alice/notes.txt", "one");
    eventually("alice publishes her file", || async {
        alice.engine.history(&path("+alice/notes.txt")).len() == 1
    })
    .await;
    alice.wait_past_settling().await;
    let scans = alice.engine.status().scans;
    alice.wait_for_ticks(10).await;
    assert_eq!(alice.engine.status().scans, scans);
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_burst_of_files_in_one_folder_takes_a_look_per_gathering_not_per_file() {
    let machines = group_with(&["alice"], |options| {
        options.settle_draft = Duration::from_secs(600);
    })
    .await;
    joined(&machines).await;
    let alice = &machines[0];
    alice.edit("inbox/first.txt", "x");
    eventually("alice holds her first draft", || async {
        alice.engine.pending(None).await.len() == 1
    })
    .await;
    let scans = alice.engine.status().scans;
    let files: Vec<String> = (0..200).map(|n| format!("inbox/{n}.txt")).collect();
    for file in &files {
        alice.edit(file, "x");
    }
    eventually_reaches("alice holds every draft", files.len() + 1, || async {
        alice.engine.pending(None).await.len()
    })
    .await;
    let looks = alice.engine.status().scans - scans;
    assert!(looks < 20, "{} files took {looks} looks", files.len());
    shut_down(machines).await;
}
