//! What a machine believes of its disk: a file is gone only where a folder
//! that could be read shows it gone, so that a folder pigeon cannot read
//! deletes nothing; a path its `.pigeonignore` files exclude is neither
//! published, written nor deleted on that machine; a rename that changes
//! only the case of a name moves the file; and a name some machine cannot
//! hold stays out of the group until a person renames it to the portable
//! name proposed.

mod common;

use std::time::Duration;

use common::{eventually, group_with, joined, path, shut_down};

fn rescanning(options: &mut pigeon_sync::Options) {
    options.rescan = Duration::from_millis(200);
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_folder_that_cannot_be_read_deletes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let machines = group_with(&["alice", "bob"], rescanning).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.follow("+alice/").await;
    alice.edit("+alice/a/notes.txt", "one");
    eventually("bob holds alice's file", || async {
        bob.read("+alice/a/notes.txt").as_deref() == Some("one")
    })
    .await;
    let folder = alice.file("+alice/a");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = std::fs::read_dir(&folder).is_err();
    alice.wait_past_settling().await;
    alice.wait_past_settling().await;
    let errors = alice.engine.status().await.errors;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    if unreadable {
        assert!(
            errors.iter().any(|error| error.contains("cannot be read")),
            "{errors:?}"
        );
    }
    assert_eq!(alice.engine.history(&path("+alice/a/notes.txt")).len(), 1);
    assert_eq!(bob.read("+alice/a/notes.txt").as_deref(), Some("one"));
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ignored_path_is_neither_published_written_nor_deleted() {
    let machines = group_with(&["alice", "bob"], rescanning).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.follow("+alice/").await;
    alice.edit("+alice/run.log", "one");
    alice.edit("+alice/notes.txt", "one");
    eventually("bob holds alice's files", || async {
        bob.read("+alice/run.log").as_deref() == Some("one")
            && bob.read("+alice/notes.txt").is_some()
    })
    .await;
    bob.edit(".pigeonignore", "*.log\n.pigeonignore\n");
    bob.wait_past_settling().await;
    alice.edit("+alice/run.log", "two");
    eventually("alice publishes her edit", || async {
        alice.engine.history(&path("+alice/run.log")).len() == 2
    })
    .await;
    bob.wait_past_settling().await;
    assert_eq!(bob.read("+alice/run.log").as_deref(), Some("one"));
    std::fs::remove_file(bob.file("+alice/run.log")).unwrap();
    bob.wait_past_settling().await;
    bob.wait_past_settling().await;
    assert_eq!(alice.engine.history(&path("+alice/run.log")).len(), 2);
    assert_eq!(alice.read("+alice/run.log").as_deref(), Some("two"));
    assert!(alice.engine.history(&path(".pigeonignore")).is_empty());
    for machine in &machines {
        assert!(machine.engine.status().await.errors.is_empty());
    }
    shut_down(machines).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_of_the_case_alone_moves_the_file() {
    let machines = group_with(&["alice", "bob"], rescanning).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.follow("+alice/").await;
    alice.edit("+alice/readme.md", "hello");
    eventually("bob holds alice's file", || async {
        bob.read("+alice/readme.md").is_some()
    })
    .await;
    std::fs::rename(
        alice.file("+alice/readme.md"),
        alice.file("+alice/README.md"),
    )
    .unwrap();
    let names = || {
        let mut names: Vec<String> = std::fs::read_dir(bob.file("+alice"))
            .into_iter()
            .flatten()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    };
    eventually("bob's file takes the new case", || async {
        names() == ["README.md"]
    })
    .await;
    assert_eq!(bob.read("+alice/README.md").as_deref(), Some("hello"));
    let history = alice.engine.history(&path("+alice/README.md"));
    assert_eq!(history.len(), 2, "{history:?}");
    alice.wait_past_settling().await;
    assert_eq!(alice.engine.history(&path("+alice/README.md")).len(), 2);
    for machine in &machines {
        assert!(machine.engine.status().await.errors.is_empty());
    }
    shut_down(machines).await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_name_some_machine_cannot_hold_waits_for_its_rename_to_the_name_proposed() {
    let machines = group_with(&["alice", "bob"], rescanning).await;
    joined(&machines).await;
    let [alice, bob] = &machines[..] else {
        unreachable!()
    };
    bob.follow("+alice/").await;
    alice.edit("+alice/what?.txt", "odd");
    alice.edit("+alice/caf\u{65}\u{301}.txt", "decomposed");
    let distinct = !alice.file("+alice/caf\u{e9}.txt").exists();
    let expected = if distinct { 2 } else { 1 };
    eventually("alice keeps the names out", || async {
        alice.engine.unportable().await.len() == expected
    })
    .await;
    alice.wait_past_settling().await;
    assert!(alice.engine.history(&path("+alice/what_.txt")).is_empty());
    let views = alice.engine.unportable().await;
    let proposals: Vec<(&str, &str)> = views
        .iter()
        .map(|view| (view.path.as_str(), view.proposal.as_str()))
        .collect();
    let mut wanted = vec![("+alice/what?.txt", "+alice/what_.txt")];
    if distinct {
        wanted.insert(0, ("+alice/caf\u{65}\u{301}.txt", "+alice/caf\u{e9}.txt"));
    }
    assert_eq!(proposals, wanted);
    for view in &views {
        let renamed = alice.engine.make_portable(&view.path).await.unwrap();
        assert_eq!(renamed.as_str(), view.proposal);
    }
    assert!(
        alice
            .engine
            .make_portable("+alice/what?.txt")
            .await
            .is_err()
    );
    eventually("bob holds the files under their portable names", || async {
        bob.read("+alice/what_.txt").as_deref() == Some("odd")
            && (!distinct || bob.read("+alice/caf\u{e9}.txt").as_deref() == Some("decomposed"))
    })
    .await;
    assert!(alice.engine.unportable().await.is_empty());
    assert!(!alice.file("+alice/what?.txt").exists());
    for machine in &machines {
        assert!(machine.engine.status().await.errors.is_empty());
    }
    shut_down(machines).await;
}
