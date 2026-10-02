//! Tests of the Overview page: the editor holds the file, the preview
//! tells each rule's effect and offers each pin its times, and the page
//! shows the members with what one can do to each.

use super::*;

fn preview() -> Value {
    let listed: Vec<Value> = (0..100)
        .map(|index| json!({ "path": format!("big/f{index}.iso"), "size": 1000, "rule": 1 }))
        .collect();
    json!({
        "version": "v1",
        "now": { "files": 120, "bytes": 3_100_000_000_u64 },
        "after": { "files": 21, "bytes": 2_400_000_000_u64 },
        "rules": [
            { "rule": "follow +mario/", "pattern": "+mario/", "span": [10, 26], "matches": 42, "decides": { "files": 40, "bytes": 2_000_000 }, "time": null, "times": null },
            { "rule": "free *.iso", "pattern": "*.iso", "span": [30, 42], "matches": 3, "decides": { "files": 3, "bytes": 900_000 }, "time": null, "times": null },
            { "rule": "pin 2026-10-01T12:00:00Z /old/", "pattern": "/old/", "span": [46, 80], "matches": 2, "decides": { "files": 0, "bytes": 0 }, "time": "2026-10-01T12:00:00Z", "times": [{ "time": "2026-09-30T08:00:00Z", "files": 2 }] },
            { "rule": "follow /none/", "pattern": "/none/", "span": [84, 99], "matches": 0, "decides": { "files": 0, "bytes": 0 }, "time": null, "times": null },
        ],
        "deltas": [
            { "delta": "download", "total": { "files": 1, "bytes": 180_000_000 }, "largest": [{ "path": "+mario/a b.txt", "size": 180_000_000, "rule": 0 }] },
            { "delta": "free", "total": { "files": 134, "bytes": 890_000_000 }, "largest": listed },
            { "delta": "pin", "total": { "files": 0, "bytes": 0 }, "largest": [] },
        ],
        "own_freed": [{ "rule": 1, "total": { "files": 2, "bytes": 5000 } }, { "rule": null, "total": { "files": 1, "bytes": 10 } }],
    })
}

#[test]
fn each_rule_tells_what_it_decides_and_a_pin_offers_its_times() {
    let parts = preview_parts("g", &preview());
    let panel = parts["panel"].as_str().unwrap();
    assert!(
        panel.contains("matches 42 files · decides 40 (2.0 MB)"),
        "{panel}"
    );
    assert!(panel.contains("matches 2 files · masked"), "{panel}");
    assert!(
        panel.contains(r#"data-start="46" data-end="80" data-pattern="/old/""#),
        "{panel}"
    );
    assert!(
        panel.contains(
            r#"<option value="2026-10-01T12:00:00Z" data-time="2026-10-01T12:00:00Z" selected>"#
        ),
        "{panel}"
    );
    assert!(panel.contains(r#"data-files="2 files""#), "{panel}");
    assert_eq!(panel.matches(r#"class="pin""#).count(), 1, "{panel}");
    assert_eq!(parts["save"], "Save: +180.0 MB, -890.0 MB");
    assert!(
        parts["confirm"]
            .as_str()
            .unwrap()
            .starts_with("The edits free space on this machine: g frees 134 files, 890.0 MB.")
    );
    assert_eq!(parts["version"], "v1");
}

#[test]
fn the_panel_lists_the_largest_files_then_counts_the_rest() {
    let panel = preview_parts("g", &preview())["panel"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(panel.contains("download: +1 file, +180.0 MB"), "{panel}");
    assert!(panel.contains("free: -134 files, -890.0 MB"), "{panel}");
    assert!(!panel.contains("pin:"), "{panel}");
    assert!(panel.contains("… and 34 more (889.9 MB)"), "{panel}");
    assert!(panel.contains("href=\"/g/g?under=big\""), "{panel}");
    assert!(
        panel.contains("href=\"/g/g/file?path=%2Bmario/a%20b.txt\""),
        "{panel}"
    );
    assert!(
        panel.contains(
            "warning: <code>*.iso</code> will no longer be here (2 files, 5.0 KB of your own)"
        ),
        "{panel}"
    );
    assert!(
        panel.contains("warning: files that no rule matches will no longer be here"),
        "{panel}"
    );
    let none = json!({ "now": {}, "after": {}, "rules": [], "deltas": [], "own_freed": [] });
    let parts = preview_parts("g", &none);
    assert!(
        parts["panel"]
            .as_str()
            .unwrap()
            .contains("Saving changes no file")
    );
    assert_eq!(parts["save"], "Save");
    assert!(parts["confirm"].is_null());
}

#[test]
fn the_page_holds_the_file_and_offers_to_leave() {
    let status = json!({
        "member": "alice", "root": "/cheapmo", "join": { "state": "joined" },
        "peers": ["m2", "m3"], "incompatible": [], "errors": ["disk full"],
        "unreached": [{"machine": "m4", "member": "bob", "error": "connecting to m4: timed out"}],
        "machine": "m1", "patches": 7,
    });
    let members = json!([
        { "name": "alice", "key": "k1", "joined": "2026-10-01T12:00:00Z", "machines": ["m1", "m2"], "online": ["m1", "m2"] },
        { "name": "bob", "key": "k2", "joined": "2026-10-01T13:00:00Z", "machines": ["m3"], "online": [] },
    ]);
    let config =
        json!({ "path": "/c/config.toml", "version": "v9", "text": "member = \"alice\"\n" });
    let shown = Overview {
        status: &status,
        members: &members,
        key: "cheapmo-key",
        config: &config,
        places: &json!([]),
    };
    let bar = Bar {
        group: "cheapmo",
        tab: Some(pages::Tab::Overview),
        suggestions: 2,
    };
    let page = overview(&bar, &shown).into_string();
    assert!(
        page.contains("alice · /cheapmo · joined · 2 other machines online"),
        "{page}"
    );
    assert!(page.contains(r#"action="/act/group/serve""#), "{page}");
    assert!(page.contains(r#"<p class="error">disk full</p>"#), "{page}");
    assert!(page.contains("Unreached machines"), "{page}");
    assert!(
        page.contains("<td>connecting to m4: timed out</td>"),
        "{page}"
    );
    assert!(page.contains(r#"data-version="v9""#), "{page}");
    assert!(page.contains("member = &quot;alice&quot;"), "{page}");
    assert!(page.contains(r#"value="cheapmo-key""#), "{page}");
    assert!(page.contains("<td>bob</td>"), "{page}");
    assert!(!page.contains("exclude"), "{page}");
    assert_eq!(
        page.matches(r#"action="/act/group/leave""#).count(),
        1,
        "{page}"
    );
    assert!(page.contains("Files (2)"), "{page}");
    assert!(
        page.contains(r#"class="current" href="/g/cheapmo/overview""#),
        "{page}"
    );
}

#[test]
fn a_pin_line_missing_its_time_is_offered_times() {
    let pins = json!([
        { "span": [3, 9], "pattern": "/docs/", "times": [{ "time": "2026-09-30T08:00:00Z", "files": 2 }] },
        { "span": [12, 18], "pattern": "", "times": [] },
    ]);
    let fixes = unfinished_pins(&pins).into_string();
    assert!(
        fixes.contains(r#"data-start="3" data-end="9" data-pattern="/docs/""#),
        "{fixes}"
    );
    assert!(fixes.contains("Pin <code>/docs/</code> at "), "{fixes}");
    assert!(
        fixes.contains(r#"<option value="" disabled selected>"#),
        "{fixes}"
    );
    assert!(fixes.contains(r#"data-files="2 files""#), "{fixes}");
    assert!(fixes.contains("Pin at "), "{fixes}");
}
