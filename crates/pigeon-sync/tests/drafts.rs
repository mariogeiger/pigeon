//! Drafts announced among machines: a new file in a drop folder shows on
//! the other machines with its author before it is published, two drafts
//! of one path in any case are rivals, the later one learns it will be set
//! aside and renames its draft to keep both, and a draft that goes is no
//! longer shown.

mod common;

use std::time::Duration;

use common::{eventually, group_with, joined};
use pigeon_core::path::GroupPath;
use pigeon_core::statement::Mode;
use pigeon_sync::Edit;

fn path(text: &str) -> GroupPath {
    GroupPath::parse(text).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn drafts_of_one_path_warn_both_members_until_one_goes() {
    let machines = group_with(&["alice", "bob"], |options| {
        options.settle_drop = Duration::from_secs(600);
    })
    .await;
    joined(&machines).await;
    let (alice, bob) = (&machines[0], &machines[1]);
    alice.edit("shared/Report.txt", "alice's");
    eventually("bob hears alice's draft", || async {
        bob.engine
            .pending(None)
            .await
            .iter()
            .any(|view| !view.here && view.author.as_str() == "alice" && view.size == 7)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    bob.edit("shared/report.txt", "bob's");
    eventually("alice hears bob's rival draft", || async {
        let pending = alice.engine.pending(None).await;
        pending.len() == 2
            && pending.iter().all(|view| view.rivals.len() == 1)
            && pending.iter().any(|view| {
                view.here && !view.rivals[0].wins && view.rivals[0].author.as_str() == "bob"
            })
            && pending.iter().any(|view| !view.here && view.rivals[0].wins)
    })
    .await;
    eventually("bob learns his copy will be set aside", || async {
        bob.engine.pending(None).await.iter().any(|view| {
            view.here && view.rivals[0].wins && view.rivals[0].path.as_str() == "shared/Report.txt"
        })
    })
    .await;
    let renamed = bob
        .engine
        .edit(
            vec![Edit::Rename {
                from: path("shared/report.txt"),
                to: path("shared/report-bob.txt"),
            }],
            Mode::Propose,
            "",
        )
        .await
        .unwrap();
    assert_eq!(
        renamed.published,
        [path("shared/report-bob.txt"), path("shared/report.txt")]
    );
    assert_eq!(bob.read("shared/report-bob.txt").as_deref(), Some("bob's"));
    eventually("alice's draft has no rival left", || async {
        let pending = alice.engine.pending(None).await;
        pending.len() == 1 && pending[0].here && pending[0].rivals.is_empty()
    })
    .await;
    std::fs::remove_file(alice.file("shared/Report.txt")).unwrap();
    eventually("bob no longer sees alice's draft", || async {
        bob.engine.pending(None).await.is_empty()
    })
    .await;
}
