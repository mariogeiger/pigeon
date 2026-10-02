//! A file of any size goes in through the API or a web form and comes out
//! of the download: the server limits no body, and sends a download in
//! chunks rather than holding it.

mod common;

use common::{GROUP, alice_with_notes, eventually};
use iroh::address_lookup::MemoryLookup;
use serde_json::json;

/// 3 MiB, more than the 2 MiB a server limits a body to by default.
fn large() -> String {
    (0..3 << 20)
        .map(|at: u32| char::from(b'a' + (at % 26) as u8))
        .collect()
}

#[tokio::test]
async fn a_file_larger_than_two_megabytes_is_written_by_the_api_and_downloaded() {
    let internet = MemoryLookup::new();
    let (alice, _) = alice_with_notes(&internet).await;
    let text = large();
    alice
        .run(
            "file",
            "write",
            json!({"path": "+alice/large.txt", "content": alice.upload(&text)}),
        )
        .await;
    eventually("the large file is on disk", &[&alice], async || {
        alice.shows("+alice/large.txt", &text)
    })
    .await;
    let download = alice
        .page(&format!("/g/{GROUP}/raw?path=%2Balice/large.txt"))
        .await;
    assert!(
        download == text,
        "the download differs: {} bytes",
        download.len()
    );
}

#[tokio::test]
async fn a_file_larger_than_two_megabytes_is_uploaded_by_a_web_form() {
    let internet = MemoryLookup::new();
    let (alice, _) = alice_with_notes(&internet).await;
    let text = large();
    let fields = [
        ("back", "/g/family"),
        ("group", GROUP),
        ("path", "+alice/uploaded.txt"),
    ];
    let answer = alice
        .post_upload("file/write", &fields, Some(("content", text.as_bytes())))
        .await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    eventually("the upload is on disk", &[&alice], async || {
        alice.shows("+alice/uploaded.txt", &text)
    })
    .await;
}
