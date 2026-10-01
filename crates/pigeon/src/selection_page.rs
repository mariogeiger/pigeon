//! The web UI's Selection page: an editor holding a draft of every rule,
//! which `selection.js` previews on each keystroke and saves whole, beside
//! the preview of what saving would download, free and freeze here; then
//! the folders kept elsewhere.

use std::collections::BTreeSet;

use maud::{Markup, html};
use pigeon_core::path::GroupPath;
use pigeon_core::selection::{Cutoff, Rule, exact_pattern, folder_pattern};
use serde_json::{Value, json};

use crate::form::form;
use crate::group_pages::{encode, file_link, fill};
use crate::pages::{self, action, layout};
use crate::render::size;

/// `count` followed by `noun`, plural unless it is one.
fn count(count: u64, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}

/// One rule as the editor's row data.
fn row(rule: &Rule) -> Value {
    let (mode, time) = match rule.cutoff {
        Cutoff::PlusInfinity => ("follow", None),
        Cutoff::At(time) => ("pin", Some(pigeon_core::clock::rfc3339(time))),
        Cutoff::MinusInfinity => ("free", None),
    };
    json!({ "mode": mode, "time": time, "pattern": rule.pattern })
}

/// The patterns the add field offers: every folder and file of `files`.
fn suggestions(files: &Value) -> BTreeSet<String> {
    let mut patterns = BTreeSet::new();
    for file in files.as_array().map(Vec::as_slice).unwrap_or_default() {
        let Some(Ok(path)) = file["path"].as_str().map(GroupPath::parse) else {
            continue;
        };
        patterns.insert(exact_pattern(&path));
        let mut folder = path.as_str();
        while let Some((parent, _)) = folder.rsplit_once('/') {
            if let Ok(parent_path) = GroupPath::parse(parent) {
                patterns.insert(folder_pattern(&parent_path));
            }
            folder = parent;
        }
    }
    patterns
}

/// The Selection page: the editor of `rules`, at `version`, with the
/// patterns of `files` to add, then the folders of `places`.
#[must_use]
pub fn selection(
    group: &str,
    rules: &[Rule],
    version: &str,
    files: &Value,
    places: &Value,
) -> Markup {
    let back = format!("/g/{group}/selection");
    let rows: Vec<Value> = rules.iter().map(row).collect();
    let rows = Value::Array(rows).to_string();
    let body = html! {
        section id="editor" class="editor" data-keep data-version=(version) data-rules=(rows) {
            div {
                h2 { "Rules" }
                p { "The last matching rule decides each file. Nothing changes until you save." }
                p class="notice conflict" hidden {
                    "The selection changed elsewhere. "
                    button type="button" class="reload" { "Reload it" }
                    " "
                    button type="button" class="overwrite" { "Keep my draft, to overwrite it" }
                }
                ol class="rules" {}
                form class="add" {
                    input type="text" name="pattern" list="patterns" placeholder="Add a folder, a file or a pattern such as *.iso";
                    button { "Add" }
                }
                datalist id="patterns" {
                    @for pattern in suggestions(files) { option value=(pattern) {} }
                }
                p { button type="button" class="save" { "Save" } }
            }
            aside class="preview" { p { "Computing the preview…" } }
            template {
                li {
                    button type="button" class="up" title="Move up" { "↑" }
                    button type="button" class="down" title="Move down" { "↓" }
                    select class="mode" {
                        option value="follow" { "follow" }
                        option value="pin" { "pin" }
                        option value="free" { "free" }
                    }
                    span class="when" {
                        " at "
                        select class="at" {
                            option value="now" { "now" }
                            optgroup class="versions" label="a version" {}
                            option value="time" { "a time" }
                        }
                        input type="datetime-local" step="1" class="time";
                    }
                    input type="text" class="pattern" list="patterns";
                    span class="effect" {}
                    button type="button" class="remove" title="Remove" { "✕" }
                    span class="error" hidden {}
                }
            }
            script src="/selection.js" defer {}
        }
        section {
            h2 { "Places" }
            (pages::table(action("selection", "places"), places, &|_| None))
            @for verb in ["place", "unplace"] {
                (form(action("selection", verb), &back, fill(group, &[], &[])))
            }
        }
    };
    layout("Selection", Some(group), &body)
}

/// A preview's amount as text.
fn amount(amount: &Value, sign: &str) -> String {
    let files = amount["files"].as_u64().unwrap_or_default();
    let bytes = amount["bytes"].as_u64().unwrap_or_default();
    format!("{sign}{}, {sign}{}", count(files, "file"), size(bytes))
}

/// What a rule line of a preview does, and whether it decides nothing.
fn effect(rule: &Value) -> (String, bool) {
    let matching = rule["matches"].as_u64().unwrap_or_default();
    let decided = rule["decides"]["files"].as_u64().unwrap_or_default();
    let bytes = rule["decides"]["bytes"].as_u64().unwrap_or_default();
    let text = format!("matches {}", count(matching, "file"));
    match (matching, decided) {
        (0, _) => (text, true),
        (_, 0) => (format!("{text} · masked"), true),
        _ => (
            format!("{text} · decides {decided} ({})", size(bytes)),
            false,
        ),
    }
}

/// The field `name` of the rule line `rule` of `preview` names.
fn rule_field<'a>(preview: &'a Value, rule: &Value, name: &str) -> Option<&'a str> {
    let index = usize::try_from(rule.as_u64()?).ok()?;
    preview["rules"][index][name].as_str()
}

/// A changed file: its folder linking to the Files page, its name to the
/// file's page, its size and the rule that decides it.
fn changed_file(group: &str, preview: &Value, file: &Value) -> Markup {
    let path = file["path"].as_str().unwrap_or_default();
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
    html! {
        li {
            @if !folder.is_empty() {
                a href={ "/g/" (group) "/files?under=" (encode(folder)) } { (folder) "/" }
            }
            a href=(file_link(group, path)) { (name) }
            " " (size(file["size"].as_u64().unwrap_or_default()))
            " · " code { (rule_field(preview, &file["rule"], "pattern").unwrap_or("no rule")) }
        }
    }
}

/// The preview panel: the totals now and after saving, the warnings for
/// this member's own files, and each delta's largest files.
fn panel(group: &str, preview: &Value) -> Markup {
    let deltas = preview["deltas"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let changing: Vec<&Value> = deltas
        .iter()
        .filter(|delta| delta["total"]["files"].as_u64() > Some(0))
        .collect();
    html! {
        h2 { "Preview" }
        table {
            tr { th {} th { "files" } th { "size" } }
            @for (label, total) in [("now", &preview["now"]), ("after save", &preview["after"])] {
                tr {
                    td { (label) }
                    td { (total["files"].as_u64().unwrap_or_default()) }
                    td { (size(total["bytes"].as_u64().unwrap_or_default())) }
                }
            }
        }
        @for freed in preview["own_freed"].as_array().map(Vec::as_slice).unwrap_or_default() {
            p class="mark" {
                "warning: "
                @match rule_field(preview, &freed["rule"], "pattern") {
                    Some(pattern) => { code { (pattern) } " will no longer be here" }
                    None => "files that no rule matches will no longer be here",
                }
                " (" (amount(&freed["total"], "")) " of your own)"
            }
        }
        @if changing.is_empty() {
            p { "Saving changes no file on this machine." }
        }
        @for delta in changing {
            @let (label, sign) = match delta["delta"].as_str() {
                Some("download") => ("download", "+"),
                Some("free") => ("free", "-"),
                _ => ("frozen", ""),
            };
            h3 { (label) ": " (amount(&delta["total"], sign)) }
            ul {
                @let largest = delta["largest"].as_array().map(Vec::as_slice).unwrap_or_default();
                @for file in largest {
                    (changed_file(group, preview, file))
                }
                @let files = delta["total"]["files"].as_u64().unwrap_or_default();
                @let listed = u64::try_from(largest.len()).unwrap_or(u64::MAX);
                @if files > listed {
                    @let bytes = delta["total"]["bytes"].as_u64().unwrap_or_default();
                    @let shown: u64 = largest.iter().filter_map(|file| file["size"].as_u64()).sum();
                    li { "… and " (files - listed) " more (" (size(bytes.saturating_sub(shown))) ")" }
                }
            }
        }
    }
}

/// The total bytes of the delta named `name` in `preview`.
fn delta_bytes(preview: &Value, name: &str) -> (u64, u64) {
    preview["deltas"]
        .as_array()
        .and_then(|deltas| deltas.iter().find(|delta| delta["delta"] == name))
        .map_or((0, 0), |delta| {
            let total = &delta["total"];
            (
                total["files"].as_u64().unwrap_or_default(),
                total["bytes"].as_u64().unwrap_or_default(),
            )
        })
}

/// What the editor shows of `preview`: each rule row's effect or error,
/// the panel, the save button's label, and the question to ask first when
/// saving frees space.
#[must_use]
pub fn preview_parts(group: &str, preview: &Value) -> Value {
    let rows: Vec<Value> = preview["rules"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|rule| {
            if let Some(error) = rule["error"].as_str() {
                return json!({ "error": error });
            }
            let (text, masked) = effect(rule);
            json!({ "effect": text, "masked": masked, "times": rule["times"] })
        })
        .collect();
    let (_, downloaded_bytes) = delta_bytes(preview, "download");
    let (freed_files, freed_bytes) = delta_bytes(preview, "free");
    let mut parts = Vec::new();
    if downloaded_bytes > 0 {
        parts.push(format!("+{}", size(downloaded_bytes)));
    }
    if freed_bytes > 0 {
        parts.push(format!("-{}", size(freed_bytes)));
    }
    let save = if parts.is_empty() {
        "Save".to_owned()
    } else {
        format!("Save: {}", parts.join(", "))
    };
    let confirm = (freed_files > 0).then(|| {
        format!(
            "Saving removes {} ({}) from this machine. Modified copies not yet published stay. Save?",
            count(freed_files, "file"),
            size(freed_bytes)
        )
    });
    json!({
        "version": preview["version"],
        "rows": rows,
        "panel": panel(group, preview).into_string(),
        "save": save,
        "confirm": confirm,
    })
}

#[cfg(test)]
mod tests {
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
                { "line": 1, "rule": "follow +mario/", "pattern": "+mario/", "matches": 42, "decides": { "files": 40, "bytes": 2_000_000 } },
                { "line": 2, "rule": "free *.iso", "pattern": "*.iso", "matches": 3, "decides": { "files": 3, "bytes": 900_000 } },
                { "line": 3, "rule": "pin 2026-10-01T12:00:00Z /old/", "pattern": "/old/", "matches": 2, "decides": { "files": 0, "bytes": 0 }, "times": [{ "time": "2026-09-30T08:00:00Z", "files": 2 }] },
                { "line": 4, "error": "\"keep\" is not a mode" },
                { "line": 5, "rule": "follow /none/", "pattern": "/none/", "matches": 0, "decides": { "files": 0, "bytes": 0 } },
            ],
            "deltas": [
                { "delta": "download", "total": { "files": 1, "bytes": 180_000_000 }, "largest": [{ "path": "+mario/a b.txt", "size": 180_000_000, "rule": 0 }] },
                { "delta": "free", "total": { "files": 134, "bytes": 890_000_000 }, "largest": listed },
                { "delta": "freeze", "total": { "files": 0, "bytes": 0 }, "largest": [] },
            ],
            "own_freed": [{ "rule": 1, "total": { "files": 2, "bytes": 5000 } }, { "rule": null, "total": { "files": 1, "bytes": 10 } }],
        })
    }

    #[test]
    fn each_row_tells_what_its_rule_decides_or_why_it_is_none() {
        let parts = preview_parts("g", &preview());
        assert_eq!(
            parts["rows"],
            json!([
                { "effect": "matches 42 files · decides 40 (2.0 MB)", "masked": false, "times": null },
                { "effect": "matches 3 files · decides 3 (900.0 KB)", "masked": false, "times": null },
                { "effect": "matches 2 files · masked", "masked": true, "times": [{ "time": "2026-09-30T08:00:00Z", "files": 2 }] },
                { "error": "\"keep\" is not a mode" },
                { "effect": "matches 0 files", "masked": true, "times": null },
            ])
        );
        assert_eq!(parts["save"], "Save: +180.0 MB, -890.0 MB");
        assert!(
            parts["confirm"]
                .as_str()
                .unwrap()
                .starts_with("Saving removes 134 files (890.0 MB)")
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
        assert!(!panel.contains("frozen:"), "{panel}");
        assert!(panel.contains("… and 34 more (889.9 MB)"), "{panel}");
        assert!(panel.contains("href=\"/g/g/files?under=big\""), "{panel}");
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
        assert!(panel.contains("<code>*.iso</code></li>"), "{panel}");
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
    fn the_add_field_offers_every_folder_and_file() {
        let files = json!([{ "path": "a/b/c.txt" }, { "path": "a/[d].txt" }]);
        let offered: Vec<String> = suggestions(&files).into_iter().collect();
        assert_eq!(offered, ["/a/", "/a/\\[d\\].txt", "/a/b/", "/a/b/c.txt"]);
    }

    #[test]
    fn the_editor_holds_the_rules_and_has_no_emoji() {
        let rules = [
            Rule {
                pattern: "/docs/".into(),
                cutoff: Cutoff::PlusInfinity,
            },
            Rule {
                pattern: "/old/".into(),
                cutoff: Cutoff::At(1_790_856_000 << 32),
            },
        ];
        let page = selection("g", &rules, "v1", &json!([]), &json!([])).into_string();
        assert!(page.contains("data-version=\"v1\""), "{page}");
        assert!(page.contains("2026-10-01T12:00:00Z"), "{page}");
        assert!(page.contains("data-keep"), "{page}");
        let html = page + preview_parts("g", &preview())["panel"].as_str().unwrap();
        assert!(
            html.chars().all(
                |character| !matches!(u32::from(character), 0x1F000..=0x1FAFF | 0x2600..=0x27BF)
                    || character == '✕'
            ),
            "the page shows an emoji"
        );
    }
}
