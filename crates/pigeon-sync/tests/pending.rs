//! Edits waiting to settle: the engine lists them with the time left and
//! whether they are drafts, publishes them at once under a path, leaving
//! the files writable, and signals each change of what it shows.

mod common;

use std::time::Duration;

use common::{eventually, group_with, is_read_only, joined, path};

#[tokio::test(flavor = "multi_thread")]
async fn waiting_edits_are_listed_and_published_at_once_under_a_path() {
    let machines = group_with(&["alice"], |options| {
        options.settle_personal = Duration::from_secs(60);
        options.settle_draft = Duration::from_secs(600);
    })
    .await;
    joined(&machines).await;
    let alice = &machines[0];
    let mut changes = alice.engine.changes();
    changes.mark_unchanged();
    alice.edit("shared/drop.txt", "draft");
    alice.edit("+alice/notes.txt", "mine");
    eventually("both edits wait", || async {
        alice.engine.pending(None).await.len() == 2
    })
    .await;
    assert!(
        changes.has_changed().unwrap(),
        "waiting edits are signalled"
    );
    let pending = alice.engine.pending(None).await;
    let [notes, drop] = &pending[..] else {
        unreachable!()
    };
    assert_eq!(notes.path, path("+alice/notes.txt"));
    assert!(!notes.draft && !notes.deleted && notes.due_in <= 60);
    assert_eq!(drop.path, path("shared/drop.txt"));
    assert!(drop.draft && (60..=600).contains(&drop.due_in));
    let shared = alice.engine.pending(Some(&path("shared"))).await;
    assert_eq!(shared.len(), 1);

    assert!(alice.engine.publish(Some(&path("nowhere"))).await.is_err());
    changes.mark_unchanged();
    alice.engine.publish(Some(&path("shared"))).await.unwrap();
    assert_eq!(alice.engine.history(&path("shared/drop.txt")).len(), 1);
    assert!(!is_read_only(&alice.file("shared/drop.txt")));
    let left = alice.engine.pending(None).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].path, path("+alice/notes.txt"));
    eventually("the publication is signalled", || async {
        changes.has_changed().unwrap()
    })
    .await;

    alice.engine.publish(None).await.unwrap();
    assert!(alice.engine.pending(None).await.is_empty());
    std::fs::remove_file(alice.file("+alice/notes.txt")).unwrap();
    eventually("the deletion waits", || async {
        alice
            .engine
            .pending(None)
            .await
            .first()
            .is_some_and(|pending| pending.deleted)
    })
    .await;
    assert!(alice.engine.publish(None).await.is_ok());
    assert_eq!(alice.engine.history(&path("+alice/notes.txt")).len(), 2);
}
