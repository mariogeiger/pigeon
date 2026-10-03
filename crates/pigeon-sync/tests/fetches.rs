//! Tests of what a fetch no machine delivers reports: its failure while
//! this machine still wants the blob, and nothing once a decision made the
//! blob needless.

mod common;

use common::{Machine, eventually, group_on, joined, shut_down};
use iroh::address_lookup::MemoryLookup;
use pigeon_core::patch::{Content, VersionRef};

/// Has bob suggest an edit of alice's plan and stop, so that alice, who
/// follows the plan, reads the suggestion through carol, who holds its
/// statement but not the content it carries: alice then fetches that
/// content from machines that cannot deliver it. Returns alice, carol and
/// that content.
async fn suggested_out_of_reach() -> (Machine, Machine, Content) {
    let whole = MemoryLookup::new();
    let machines = group_on(&whole, &["alice", "bob", "carol"], |_| {}).await;
    joined(&machines).await;
    let [alice, bob, carol]: [Machine; 3] = machines.try_into().ok().unwrap();
    bob.follow("+alice/").await;
    alice.edit("+alice/plan.txt", "alice's plan");
    eventually("bob holds the plan", || async {
        bob.read("+alice/plan.txt").is_some()
    })
    .await;
    let alice = alice.restart_on(&MemoryLookup::new()).await;
    bob.edit("+alice/plan.txt", "bob's plan");
    eventually("carol reads bob's suggestion", || async {
        carol.engine.suggestions().await.len() == 1
    })
    .await;
    shut_down([bob]).await;
    let through_carol = MemoryLookup::new();
    through_carol.add_endpoint_info(whole.get_endpoint_info(carol.engine.machine()).unwrap());
    let alice = alice.restart_on(&through_carol).await;
    eventually("alice reads bob's suggestion", || async {
        alice.engine.suggestions().await.len() == 1
    })
    .await;
    let content = alice.engine.suggestions().await[0].changes[0]
        .content
        .unwrap();
    eventually("alice fetches what bob suggested", || async {
        alice.engine.status().fetching == 1
    })
    .await;
    (alice, carol, content)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fetch_no_machine_delivers_is_reported_while_the_blob_is_wanted() {
    let (alice, carol, content) = Box::pin(suggested_out_of_reach()).await;
    let failed = format!("fetching {}", content.hash);
    eventually("alice reports the fetch failed", || async {
        alice
            .engine
            .status()
            .errors
            .iter()
            .any(|error| error.starts_with(&failed))
    })
    .await;
    shut_down([alice, carol]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fetch_a_discard_made_needless_ends_unreported() {
    let (alice, carol, _) = Box::pin(suggested_out_of_reach()).await;
    let shown: Vec<VersionRef> = alice
        .engine
        .suggestions()
        .await
        .into_iter()
        .map(|view| view.statement)
        .collect();
    alice.engine.discard(&shown).await.unwrap();
    eventually("alice's fetch ends", || async {
        alice.engine.status().fetching == 0
    })
    .await;
    assert_eq!(alice.engine.status().errors, Vec::<String>::new());
    shut_down([alice, carol]).await;
}
