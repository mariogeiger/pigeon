//! What applying a text as a group's `config.toml` would change on this
//! machine, the text read by the parser `pigeon daemon reload` uses: what
//! its selection would download, free and pin, each rule with what it
//! matches and decides, where the text spells it and, for a pin, the times
//! it can choose, which a pin line that does not read is offered too;
//! what applying texts frees, which pigeon refuses until told to go ahead;
//! and the version of a text, which tells whether the file changed since
//! one read it.

use anyhow::{Result, anyhow};
use pigeon_core::clock::{parse_rfc3339, rfc3339};
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_store::config::Config;
use pigeon_sync::{Amount, Delta, Engine, Preview};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::confirm::Confirm;
use crate::render::size;

/// The version of the text of a configuration.
#[must_use]
pub fn version(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..16].to_owned()
}

/// The selection lines as a text spells them, each with its place.
#[derive(Deserialize)]
struct Spelled {
    #[serde(default)]
    selection: Vec<toml::Spanned<String>>,
}

/// Each selection line of `text` with where the text spells it, quotes
/// included, in UTF-16 code units, as a browser counts; none unless the
/// text is TOML.
fn lines(text: &str) -> Vec<(String, [usize; 2])> {
    let units = |end: usize| {
        text.get(..end)
            .map_or(0, |head| head.encode_utf16().count())
    };
    toml::from_str::<Spelled>(text)
        .map(|spelled| {
            spelled
                .selection
                .into_iter()
                .map(|line| {
                    let span = [units(line.span().start), units(line.span().end)];
                    (line.into_inner(), span)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Where `text` spells each selection line, as `lines` gives it.
fn spans(text: &str) -> Vec<[usize; 2]> {
    lines(text).into_iter().map(|(_, span)| span).collect()
}

/// The pattern a pin line names after its time, or after `pin` when its
/// time is missing or does not read.
fn pinned_pattern(rest: &str) -> &str {
    let rest = rest.trim();
    let (when, after) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(when, after)| (when, after.trim_start()));
    if parse_rfc3339(when).is_ok() {
        after
    } else {
        rest
    }
}

/// Each selection line of `text` that pins but does not read, with where
/// the text spells it, the pattern it names, if any, and the times at
/// which pinning the files that pattern matches, or every file without
/// one, holds something new.
#[must_use]
pub fn unfinished_pins(engine: &Engine, text: &str) -> Vec<Value> {
    lines(text)
        .into_iter()
        .filter_map(|(line, span)| {
            let rest = line.trim_start().strip_prefix("pin")?;
            let pin = rest.is_empty() || rest.starts_with(char::is_whitespace);
            if !pin || Rule::parse(&line).is_ok() {
                return None;
            }
            let pattern = pinned_pattern(rest);
            let matched = if pattern.is_empty() { "*" } else { pattern };
            let times = engine.pin_times(matched).unwrap_or_default();
            Some(json!({ "span": span, "pattern": pattern, "times": times }))
        })
        .collect()
}

/// The configuration `text` holds, and what applying it would change on
/// the machine `engine` runs.
///
/// # Errors
///
/// Fails, saying what in the text is wrong, if it is no configuration, or
/// if the index cannot be read.
pub async fn plan(engine: &Engine, text: &str) -> Result<(Config, Preview)> {
    let (config, _) = Config::parse(text).map_err(|reason| anyhow!("{reason}"))?;
    let preview = engine
        .preview(config.selection.rules().cloned().collect())
        .await?;
    Ok((config, preview))
}

/// The amount of `delta` in `preview`.
#[must_use]
pub fn total(preview: &Preview, delta: Delta) -> Amount {
    preview
        .deltas
        .iter()
        .find(|files| files.delta == delta)
        .map(|files| files.total)
        .unwrap_or_default()
}

/// `count` followed by `noun`, plural unless it is one.
#[must_use]
pub fn count(count: u64, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}

/// `amount` as text, such as `3 files, 1.2 MB`, empty when it is none.
#[must_use]
pub fn amount(amount: Amount) -> String {
    if amount.files == 0 {
        return String::new();
    }
    format!("{}, {}", count(amount.files, "file"), size(amount.bytes))
}

/// What applying configurations frees on this machine, group by group: a
/// question until the call is made again with `yes`.
#[derive(Debug)]
pub struct Freed(Vec<(String, Amount)>);

impl Freed {
    /// What `groups` free, each with its amount; a group freeing nothing
    /// is left out.
    pub fn new(groups: impl IntoIterator<Item = (String, Amount)>) -> Self {
        Self(
            groups
                .into_iter()
                .filter(|(_, amount)| amount.files > 0)
                .collect(),
        )
    }

    /// What `group` frees by its edits, which `preview` tells.
    #[must_use]
    pub fn by(group: &str, preview: &Preview) -> Self {
        Self::new([(group.to_owned(), total(preview, Delta::Free))])
    }

    /// Whether nothing is freed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Asks [`Freed::question`] unless nothing is freed or `yes`.
    ///
    /// # Errors
    ///
    /// Fails with the question if something is freed and not `yes`.
    pub fn refuse_unless(self, yes: bool) -> Result<()> {
        Confirm::unless(yes || self.is_empty(), || self.question())
    }

    /// The question to ask before applying the edits.
    #[must_use]
    pub fn question(&self) -> String {
        format!(
            "The edits free space on this machine: {}. Modified copies not yet published stay. Apply them?",
            self.groups()
        )
    }

    fn groups(&self) -> String {
        self.0
            .iter()
            .map(|(group, freed)| format!("{group} frees {}", amount(*freed)))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What `preview` downloads, frees and pins, each as text.
#[must_use]
pub fn summary(preview: &Preview) -> Value {
    preview
        .deltas
        .iter()
        .map(|files| (files.delta.to_string(), Value::from(amount(files.total))))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// What applying `text` would change, as JSON: the totals now and after,
/// each rule with its effect, where `text` spells it and the times a pin
/// can choose, each delta's largest files with their rules, and the
/// files of this member's own that would go.
///
/// # Errors
///
/// Fails, saying what in the text is wrong, if it is no configuration, or
/// if the index or a pin's versions cannot be read.
pub async fn preview(engine: &Engine, text: &str) -> Result<Value> {
    let (config, preview) = plan(engine, text).await?;
    let rules: Vec<&Rule> = config.selection.rules().collect();
    let spans = spans(text);
    let spans = (spans.len() == rules.len()).then_some(spans);
    let mut rows = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        let effect = preview.rules[index];
        let pin = match rule.cutoff {
            Cutoff::At(time) => Some((rfc3339(time), engine.pin_times(&rule.pattern)?)),
            _ => None,
        };
        rows.push(json!({
            "rule": rule.to_string(),
            "pattern": rule.pattern,
            "span": spans.as_ref().map(|spans| spans[index]),
            "matches": effect.matches,
            "decides": effect.decides,
            "time": pin.as_ref().map(|(time, _)| time),
            "times": pin.map(|(_, times)| times),
        }));
    }
    let mut value = serde_json::to_value(&preview)?;
    value["rules"] = Value::Array(rows);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_selection_line_is_found_in_utf16_units() {
        let text = "member = \"mario\"\n# é\nselection = [\n    \"follow /a/\",\n    \"pin 2026-10-01T12:00:00Z /b/\",\n]\n";
        let spans = spans(text);
        let units: Vec<u16> = text.encode_utf16().collect();
        let line = |[start, end]: [usize; 2]| String::from_utf16(&units[start..end]).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(line(spans[0]), "\"follow /a/\"");
        assert_eq!(line(spans[1]), "\"pin 2026-10-01T12:00:00Z /b/\"");
        assert!(super::spans("selection = [").is_empty());
    }

    #[test]
    fn a_pin_names_its_pattern_with_or_without_its_time() {
        assert_eq!(pinned_pattern(""), "");
        assert_eq!(pinned_pattern(" /docs/"), "/docs/");
        assert_eq!(pinned_pattern(" now /my docs/"), "now /my docs/");
        assert_eq!(pinned_pattern(" 2026-10-01T12:00:00Z /a/"), "/a/");
        assert_eq!(pinned_pattern(" 2026-10-01T12:00:00Z"), "");
    }

    #[test]
    fn amounts_and_what_edits_free_read_as_files_and_bytes() {
        let none = Amount::default();
        assert_eq!(amount(none), "");
        let one = Amount {
            files: 1,
            bytes: 2048,
        };
        assert_eq!(amount(one), "1 file, 2.0 KB");
        assert_eq!(count(3, "file"), "3 files");
        let freed = Freed::new([("a".to_owned(), one), ("b".to_owned(), none)]);
        assert!(
            freed
                .question()
                .starts_with("The edits free space on this machine: a frees 1 file, 2.0 KB.")
        );
        assert!(
            Freed::new([("b".to_owned(), none)])
                .refuse_unless(false)
                .is_ok()
        );
        let refusal = Freed::new([("a".to_owned(), one)])
            .refuse_unless(false)
            .unwrap_err();
        assert!(refusal.downcast_ref::<Confirm>().is_some());
        assert!(
            Freed::new([("a".to_owned(), one)])
                .refuse_unless(true)
                .is_ok()
        );
        assert_eq!(version("a"), version("a"));
        assert_ne!(version("a"), version("b"));
    }
}
