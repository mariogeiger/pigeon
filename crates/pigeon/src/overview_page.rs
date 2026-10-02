//! The web UI's Overview page of a group: how it stands here in one line,
//! with its errors and incompatible machines if any; its members, and the
//! button to leave; its key; the editor of this machine's `config.toml`,
//! which `config_editor.js` previews on each keystroke and saves whole,
//! offering times to a pin line missing one; the folders kept elsewhere;
//! and the raw ids, folded.

use maud::{Markup, html};
use pigeon_sync::Amount;
use serde_json::{Value, json};

use crate::config_preview::{Freed, count};
use crate::form::form;
use crate::pages::{self, Bar, action, encode, fields, file_link, fill, layout, short_time};
use crate::render::{cell, size};

/// What the Overview page shows of a group.
pub struct Overview<'a> {
    pub status: &'a Value,
    pub members: &'a Value,
    pub key: &'a str,
    /// What `config show` gives.
    pub config: &'a Value,
    pub places: &'a Value,
}

/// The items of a list, none unless it is one.
fn items(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

/// The group in one line: the member, the root, whether this machine
/// joined, and how many other machines it talks to.
fn status_line(status: &Value) -> String {
    let online = u64::try_from(items(&status["peers"]).len()).unwrap_or(u64::MAX);
    format!(
        "{} · {} · {} · {} online",
        cell("", &status["member"]),
        cell("", &status["root"]),
        cell("", &status["join"]["state"]),
        count(online, "other machine"),
    )
}

/// The machines that run a version of pigeon this one cannot talk to, each
/// with its member, whether it is older or newer, and its version.
fn incompatible_section(machines: &[Value]) -> Markup {
    let told = |machine: &Value, column: &str| cell(column, &machine[column]);
    html! {
        section {
            h2 { "Incompatible machines" }
            p class="mark" {
                "These machines run a version of pigeon whose protocol this one does not speak, so they cannot sync with it. Run "
                code { "pigeon update" }
                " on each machine that is older, or on this one if one is newer."
            }
            table {
                tr { th { "member" } th { "machine" } th { "is" } th { "version" } th { "commit" } th { "protocol" } }
                @for machine in machines {
                    tr {
                        td { (told(machine, "member")) }
                        td { (told(machine, "machine")) }
                        td { (told(machine, "standing")) }
                        td { (told(machine, "version")) }
                        td { (told(machine, "commit")) }
                        td { (told(machine, "protocol")) }
                    }
                }
            }
        }
    }
}

/// Each member with their machines and how many are online, and, for this
/// member, the button to leave.
fn members_section(group: &str, me: &str, members: &Value) -> Markup {
    let back = format!("/g/{group}/overview");
    html! {
        section {
            h2 { "Members" }
            table class="members" {
                tr { th { "member" } th { "machines" } th { "online" } th { "joined" } th {} }
                @for member in items(members) {
                    @let name = member["name"].as_str().unwrap_or_default();
                    tr {
                        td { (name) @if name == me { " (you)" } }
                        td { (items(&member["machines"]).len()) }
                        td { (items(&member["online"]).len()) }
                        td { (short_time(member["joined"].as_str().unwrap_or_default())) }
                        td {
                            @if name == me {
                                div data-confirm="Leave the group on this machine? It stops syncing and forgets the group's key and state, keeping its files; your name stays a member's." {
                                    (form(action("group", "leave"), &back, fill(group, &[], &[])))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The editor of `config.toml`, kept as typed while the page refreshes.
fn editor(config: &Value) -> Markup {
    let text = config["text"].as_str().unwrap_or_default();
    let rows = text.lines().count() + 2;
    html! {
        section id="config" data-keep data-version=(cell("", &config["version"])) {
            h2 { "Configuration" }
            p class="quiet" {
                "This machine's " code { (cell("", &config["path"])) }
                ": the last selection line matching a file decides it. Nothing changes until you save, which applies it to this group only, as "
                code { "pigeon daemon reload" } " does."
            }
            p class="notice conflict" hidden {
                "config.toml changed elsewhere. "
                button type="button" class="load" { "Load it" }
                " "
                button type="button" class="overwrite" { "Keep my edits, to overwrite it" }
            }
            div class="code" {
                pre aria-hidden="true" {}
                textarea spellcheck="false" rows=(rows) aria-label="config.toml" { (text) }
            }
            p class="error problem" hidden {}
            div class="fix" {}
            p { button type="button" class="save" { "Save" } }
            div class="preview" { p { "Computing the preview…" } }
            script src="/config_editor.js" defer {}
        }
    }
}

/// The folders this machine keeps elsewhere, and the forms to place one
/// or bring one back.
fn places_section(group: &str, places: &Value) -> Markup {
    let back = format!("/g/{group}/overview");
    html! {
        section {
            h2 { "Places" }
            @if !items(places).is_empty() {
                (pages::table(action("selection", "places"), places, &|_| None, None))
            }
            @for verb in ["place", "unplace"] {
                (form(action("selection", verb), &back, fill(group, &[], &[])))
            }
        }
    }
}

/// The Overview page of `bar`'s group.
#[must_use]
pub fn overview(bar: &Bar<'_>, shown: &Overview<'_>) -> Markup {
    let group = bar.group;
    let status = shown.status;
    let back = format!("/g/{group}/overview");
    let me = status["member"].as_str().unwrap_or_default();
    let ids = json!({
        "machine": status["machine"],
        "patches": status["patches"],
        "pending": status["pending"],
        "fetching": status["fetching"],
        "relay": status["relay"],
    });
    let body = html! {
        p { (status_line(status)) }
        @for error in items(&status["errors"]) {
            p class="error" { (cell("", error)) }
        }
        @if status["join"]["state"] != "joined" {
            p class="mark" {
                (status["join"]["reason"].as_str().unwrap_or("This machine has not joined yet."))
            }
            (form(action("member", "claim"), &back, fill(group, &[], &[])))
        }
        @if !items(&status["incompatible"]).is_empty() {
            (incompatible_section(items(&status["incompatible"])))
        }
        (members_section(group, me, shown.members))
        section {
            h2 { "Group key" }
            p class="quiet" { "Share it with a new member so that their machine can join." }
            div class="key" {
                input type="text" readonly value=(shown.key) aria-label="group key";
                button type="button" onclick="navigator.clipboard.writeText(this.previousElementSibling.value)" { "Copy" }
            }
        }
        (editor(shown.config))
        (places_section(group, shown.places))
        details {
            summary { "Details" }
            (fields(&ids))
            p class="quiet" { "Machines that cannot connect directly talk through a relay, which sees only ciphertext: iroh's public relays, and the group's own if it names one, served by " code { "pigeon relay" } "." }
            (form(action("group", "relay"), &back, fill(group, &[], &[])))
        }
    };
    layout(group, Some(bar), &body)
}

/// What a rule of a preview does, and whether it decides nothing.
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

/// The field `name` of the rule of `preview` at the position `rule`.
fn rule_field<'a>(preview: &'a Value, rule: &Value, name: &str) -> Option<&'a str> {
    let index = usize::try_from(rule.as_u64()?).ok()?;
    preview["rules"][index][name].as_str()
}

/// What a pin may hold its files at: now, the time it holds, or a time
/// some of its files have a version at, which `config_editor.js` labels
/// in local time; or any local time.
fn pin_choice(rule: &Value) -> Markup {
    let time = rule["time"].as_str().unwrap_or_default();
    let times = items(&rule["times"]);
    let listed = times.iter().any(|known| known["time"] == time);
    html! {
        select class="pin" aria-label="Hold these files as they were at" {
            @if time.is_empty() { option value="" disabled selected { "choose a time…" } }
            option value="now" { "now" }
            @if !listed && !time.is_empty() { option value=(time) data-time=(time) selected { (time) } }
            @if !times.is_empty() {
                optgroup label="a version" {
                    @for known in times {
                        @let at = known["time"].as_str().unwrap_or_default();
                        @let files = known["files"].as_u64().unwrap_or_default();
                        option value=(at) data-time=(at) data-files=(count(files, "file")) selected[at == time] {
                            (at) " · " (count(files, "file"))
                        }
                    }
                }
            }
        }
        " "
        input type="datetime-local" step="1" class="pin-time" data-time=(time) aria-label="or at a time";
    }
}

/// For each pin line that does not read, the times it may hold its files
/// at, which rewrite the line.
#[must_use]
pub fn unfinished_pins(pins: &Value) -> Markup {
    html! {
        @for pin in items(pins) {
            @let span = &pin["span"];
            p class="pin-fix" data-start=[span[0].as_u64()] data-end=[span[1].as_u64()] data-pattern=(cell("", &pin["pattern"])) {
                "Pin "
                @if let Some(pattern) = pin["pattern"].as_str().filter(|pattern| !pattern.is_empty()) {
                    code { (pattern) } " "
                }
                "at " (pin_choice(pin))
            }
        }
    }
}

/// Each rule with what it does, where the text spells it, and, for a pin,
/// what it may hold its files at.
fn rules_table(preview: &Value) -> Markup {
    html! {
        table class="rules" {
            tr { th { "rule" } th { "effect" } th {} }
            @for rule in items(&preview["rules"]) {
                @let (text, masked) = effect(rule);
                @let span = &rule["span"];
                tr class=[masked.then_some("masked")] data-start=[span[0].as_u64()] data-end=[span[1].as_u64()] data-pattern=(cell("", &rule["pattern"])) {
                    td { code { (cell("", &rule["rule"])) } }
                    td class="effect" { (text) }
                    td { @if rule["times"].is_array() && span.is_array() { (pin_choice(rule)) } }
                }
            }
        }
    }
}

/// A preview's amount as text.
fn amount(amount: &Value, sign: &str) -> String {
    let files = amount["files"].as_u64().unwrap_or_default();
    let bytes = amount["bytes"].as_u64().unwrap_or_default();
    format!("{sign}{}, {sign}{}", count(files, "file"), size(bytes))
}

/// A changed file: its folder linking to the Files page, its name to the
/// file's page, its size and the rule that decides it.
fn changed_file(group: &str, preview: &Value, file: &Value) -> Markup {
    let path = file["path"].as_str().unwrap_or_default();
    let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
    html! {
        li {
            @if !folder.is_empty() {
                a href={ "/g/" (group) "?under=" (encode(folder)) } { (folder) "/" }
            }
            a href=(file_link(group, path)) { (name) }
            " " (size(file["size"].as_u64().unwrap_or_default()))
            " · " code { (rule_field(preview, &file["rule"], "pattern").unwrap_or("no rule")) }
        }
    }
}

/// The preview panel: the totals now and after saving, the warnings for
/// this member's own files, each rule's effect, and each delta's largest
/// files.
fn panel(group: &str, preview: &Value) -> Markup {
    let changing: Vec<&Value> = items(&preview["deltas"])
        .iter()
        .filter(|delta| delta["total"]["files"].as_u64() > Some(0))
        .collect();
    html! {
        h3 { "Selection preview" }
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
        @for freed in items(&preview["own_freed"]) {
            p class="mark" {
                "warning: "
                @match rule_field(preview, &freed["rule"], "pattern") {
                    Some(pattern) => { code { (pattern) } " will no longer be here" }
                    None => "files that no rule matches will no longer be here",
                }
                " (" (amount(&freed["total"], "")) " of your own)"
            }
        }
        (rules_table(preview))
        @if changing.is_empty() {
            p { "Saving changes no file on this machine." }
        }
        @for delta in changing {
            @let name = delta["delta"].as_str().unwrap_or_default();
            @let sign = match name { "download" => "+", "free" => "-", _ => "" };
            h4 { (name) ": " (amount(&delta["total"], sign)) }
            ul {
                @let largest = items(&delta["largest"]);
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

/// The total of the delta named `name` in `preview`.
fn delta_total(preview: &Value, name: &str) -> Amount {
    items(&preview["deltas"])
        .iter()
        .find(|delta| delta["delta"] == name)
        .map_or_else(Amount::default, |delta| {
            let total = &delta["total"];
            Amount {
                files: total["files"].as_u64().unwrap_or_default(),
                bytes: total["bytes"].as_u64().unwrap_or_default(),
            }
        })
}

/// What the editor shows of `preview`: the version of the file, the
/// panel, the save button's label, and the question to ask first when
/// saving frees space.
#[must_use]
pub fn preview_parts(group: &str, preview: &Value) -> Value {
    let downloaded = delta_total(preview, "download");
    let free = delta_total(preview, "free");
    let mut parts = Vec::new();
    if downloaded.bytes > 0 {
        parts.push(format!("+{}", size(downloaded.bytes)));
    }
    if free.bytes > 0 {
        parts.push(format!("-{}", size(free.bytes)));
    }
    let save = if parts.is_empty() {
        "Save".to_owned()
    } else {
        format!("Save: {}", parts.join(", "))
    };
    let freed = Freed::new([(group.to_owned(), free)]);
    json!({
        "version": preview["version"],
        "panel": panel(group, preview).into_string(),
        "save": save,
        "confirm": (!freed.is_empty()).then(|| freed.question()),
    })
}

#[cfg(test)]
#[path = "overview_page_tests.rs"]
mod tests;
