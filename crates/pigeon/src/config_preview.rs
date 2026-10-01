//! What applying a text as a group's `config.toml` would change on this
//! machine, the text read by the parser `pigeon daemon reload` uses: what
//! its selection would download, free and freeze, each rule with what it
//! matches and decides, where the text spells it and, for a pin, the times
//! it can choose; and the version of a text, which tells whether the file
//! changed since one read it.

use anyhow::{Result, anyhow};
use pigeon_core::clock::rfc3339;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_store::config::Config;
use pigeon_sync::{Amount, Delta, Engine, Preview};
use serde::Deserialize;
use serde_json::{Value, json};

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

/// Where `text` spells each selection line, quotes included, in UTF-16
/// code units, as a browser counts; none unless every line reads.
fn spans(text: &str) -> Vec<[usize; 2]> {
    let units = |end: usize| {
        text.get(..end)
            .map_or(0, |head| head.encode_utf16().count())
    };
    toml::from_str::<Spelled>(text)
        .map(|spelled| {
            spelled
                .selection
                .iter()
                .map(|line| [units(line.span().start), units(line.span().end)])
                .collect()
        })
        .unwrap_or_default()
}

/// The configuration `text` holds, and what applying it would change on
/// the machine `engine` runs.
///
/// # Errors
///
/// Fails, saying what in the text is wrong, if it is no configuration, or
/// if the index cannot be read.
pub async fn plan(engine: &Engine, text: &str) -> Result<(Config, Preview)> {
    let (config, _) = Config::parse(text, engine.now()).map_err(|reason| anyhow!("{reason}"))?;
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

/// What `preview` downloads, frees and freezes, each as text.
#[must_use]
pub fn summary(preview: &Preview) -> Value {
    json!({
        "download": amount(total(preview, Delta::Download)),
        "free": amount(total(preview, Delta::Free)),
        "freeze": amount(total(preview, Delta::Freeze)),
    })
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
        let text = "member = \"mario\"\n# é\nselection = [\n    \"follow /a/\",\n    \"pin now /b/\",\n]\n";
        let spans = spans(text);
        let units: Vec<u16> = text.encode_utf16().collect();
        let line = |[start, end]: [usize; 2]| String::from_utf16(&units[start..end]).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(line(spans[0]), "\"follow /a/\"");
        assert_eq!(line(spans[1]), "\"pin now /b/\"");
        assert!(super::spans("selection = [").is_empty());
    }

    #[test]
    fn amounts_read_as_files_and_bytes() {
        let none = Amount::default();
        assert_eq!(amount(none), "");
        let one = Amount {
            files: 1,
            bytes: 2048,
        };
        assert_eq!(amount(one), "1 file, 2.0 KB");
        assert_eq!(count(3, "file"), "3 files");
        assert_eq!(version("a"), version("a"));
        assert_ne!(version("a"), version("b"));
    }
}
