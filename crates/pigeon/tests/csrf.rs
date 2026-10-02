//! A page of another site cannot make the server do anything, with the
//! cookie the browser attaches or without, and the API does not take the
//! cookie at all.

mod common;

use common::{GROUP, alice_with_notes};
use iroh::address_lookup::MemoryLookup;
use serde_json::json;

const FOLLOW: [(&str, &str); 3] = [
    ("back", "/g/family"),
    ("group", GROUP),
    ("pattern", "/csrf/"),
];

#[tokio::test]
async fn a_form_of_another_site_changes_nothing() {
    let internet = MemoryLookup::new();
    let (alice, _) = alice_with_notes(&internet).await;
    let before = alice.selection().await;
    for header in [
        ("sec-fetch-site", "cross-site"),
        ("sec-fetch-site", "same-site"),
        ("origin", "http://evil.example"),
        ("origin", "null"),
    ] {
        let answer = alice
            .post_form_with(
                "selection/follow",
                &FOLLOW,
                vec![(header.0, header.1.to_owned())],
            )
            .await;
        assert_eq!(answer.status, 403, "{header:?}: {}", answer.body);
    }
    assert_eq!(alice.selection().await, before);
}

#[tokio::test]
async fn a_form_of_this_server_or_of_no_page_goes_through() {
    let internet = MemoryLookup::new();
    let (alice, _) = alice_with_notes(&internet).await;
    let before = alice.selection().await;
    let answer = alice
        .post_form_with(
            "selection/follow",
            &FOLLOW,
            vec![("sec-fetch-site", "same-origin".to_owned())],
        )
        .await;
    assert_eq!(answer.status, 303, "{}", answer.body);
    assert_ne!(alice.selection().await, before);
}

#[tokio::test]
async fn the_api_takes_the_token_in_its_header_and_not_the_cookie() {
    let internet = MemoryLookup::new();
    let (alice, _) = alice_with_notes(&internet).await;
    let body = json!({"group": GROUP, "pattern": "/cookie/"})
        .to_string()
        .into_bytes();
    let kind = "application/json".to_owned();
    let with_cookie = alice
        .send(
            "POST",
            "/api/selection/follow",
            vec![alice.cookie()],
            Some((kind.clone(), body.clone())),
        )
        .await;
    assert_eq!(with_cookie.status, 401, "{}", with_cookie.body);
    let bearer = ("authorization", format!("Bearer {}", alice.token()));
    let with_header = alice
        .send(
            "POST",
            "/api/selection/follow",
            vec![bearer],
            Some((kind, body)),
        )
        .await;
    assert_eq!(with_header.status, 200, "{}", with_header.body);
}
