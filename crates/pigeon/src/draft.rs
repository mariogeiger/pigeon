//! A draft selection as text, one rule per line: `follow /docs/`, `frozen
//! now /report/` or `frozen <RFC 3339 time> /report/`, and `free *.iso`;
//! blank lines and lines starting with `#` say nothing. Reads and writes
//! the rules, and previews what saving a draft would change.

use anyhow::{Result, anyhow};
use pigeon_core::clock::{parse_rfc3339, rfc3339};
use pigeon_core::selection::{Cutoff, Rule, compile};
use pigeon_sync::{Engine, Preview};
use serde_json::{Value, json};

/// One rule line of a draft, read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    /// Its number, from 1.
    pub number: usize,
    /// The rule, or why the line is none.
    pub rule: Result<Rule, String>,
}

/// `rule` as one line of a draft.
#[must_use]
pub fn format_rule(rule: &Rule) -> String {
    let mode = match rule.cutoff {
        Cutoff::PlusInfinity => "follow".to_owned(),
        Cutoff::At(time) => format!("frozen {}", rfc3339(time)),
        Cutoff::MinusInfinity => "free".to_owned(),
    };
    format!("{mode} {}", rule.pattern)
}

/// `rules` as a draft, one line each.
#[must_use]
pub fn format(rules: &[Rule]) -> String {
    rules.iter().map(|rule| format_rule(rule) + "\n").collect()
}

/// The first word of `text` and what follows it.
fn word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    text.split_once(char::is_whitespace)
        .map_or((text, ""), |(first, rest)| (first, rest.trim_start()))
}

/// Reads one rule, `now` standing for the time of `frozen now`.
fn rule(line: &str, now: u64) -> Result<Rule, String> {
    let (mode, rest) = word(line);
    let (cutoff, pattern) = match mode {
        "follow" => (Cutoff::PlusInfinity, rest),
        "free" => (Cutoff::MinusInfinity, rest),
        "frozen" => {
            let (when, pattern) = word(rest);
            let time = match when {
                "now" => now,
                "" => return Err("frozen needs a time: now or an RFC 3339 time".to_owned()),
                time => parse_rfc3339(time)?,
            };
            (Cutoff::At(time), pattern)
        }
        _ => {
            return Err(format!(
                "{mode:?} is not a mode: write follow, frozen or free"
            ));
        }
    };
    if pattern.is_empty() {
        return Err(format!("{mode} needs a pattern, such as /docs/ or *.pdf"));
    }
    compile(pattern).map_err(|error| error.to_string())?;
    Ok(Rule {
        pattern: pattern.to_owned(),
        cutoff,
    })
}

/// Reads every rule line of `text`, `now` standing for `frozen now`.
#[must_use]
pub fn parse(text: &str, now: u64) -> Vec<Line> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim_end_matches('\r')))
        .filter(|(_, line)| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|(number, line)| Line {
            number,
            rule: rule(line, now),
        })
        .collect()
}

/// The rules of `text`, if every line is one.
///
/// # Errors
///
/// Fails naming each line that is not a rule.
pub fn rules(text: &str, now: u64) -> Result<Vec<Rule>> {
    let lines = parse(text, now);
    let errors: Vec<String> = lines
        .iter()
        .filter_map(|line| {
            let error = line.rule.as_ref().err()?;
            Some(format!("line {}: {error}", line.number))
        })
        .collect();
    if !errors.is_empty() {
        return Err(anyhow!("{}", errors.join("; ")));
    }
    Ok(lines
        .into_iter()
        .filter_map(|line| line.rule.ok())
        .collect())
}

/// What saving the draft `text` would change, each rule line with its
/// effect or why it is no rule; the lines that are no rule do nothing.
///
/// # Errors
///
/// Fails if the index cannot be read.
pub async fn preview(engine: &Engine, text: &str) -> Result<Value> {
    let lines = parse(text, engine.now());
    let valid: Vec<usize> = (0..lines.len())
        .filter(|&index| lines[index].rule.is_ok())
        .collect();
    let rules = valid
        .iter()
        .filter_map(|&index| lines[index].rule.clone().ok())
        .collect();
    let preview = engine.preview(rules).await?;
    Ok(preview_json(&lines, &valid, &preview))
}

/// `preview` as JSON whose rules are `lines`, the valid ones at `valid`.
fn preview_json(lines: &[Line], valid: &[usize], preview: &Preview) -> Value {
    let line_of = |rule: Option<usize>| rule.map(|rule| valid[rule]);
    let rules: Vec<Value> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let effect = valid
                .iter()
                .position(|&at| at == index)
                .map(|rule| preview.rules[rule]);
            match &line.rule {
                Ok(rule) => json!({
                    "line": line.number,
                    "rule": format_rule(rule),
                    "pattern": rule.pattern,
                    "matches": effect.map(|effect| effect.matches),
                    "decides": effect.map(|effect| effect.decides),
                }),
                Err(error) => json!({ "line": line.number, "error": error }),
            }
        })
        .collect();
    let deltas: Vec<Value> = preview
        .deltas
        .iter()
        .map(|delta| {
            let largest: Vec<Value> = delta
                .largest
                .iter()
                .map(|file| json!({ "path": file.path, "size": file.size, "rule": line_of(file.rule) }))
                .collect();
            json!({ "delta": delta.delta, "total": delta.total, "largest": largest })
        })
        .collect();
    let own_freed: Vec<Value> = preview
        .own_freed
        .iter()
        .map(|freed| json!({ "rule": line_of(freed.rule), "total": freed.total }))
        .collect();
    json!({
        "version": preview.version,
        "now": preview.now,
        "after": preview.after,
        "rules": rules,
        "deltas": deltas,
        "own_freed": own_freed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_read_back_as_they_print() {
        let given = vec![
            Rule {
                pattern: "/docs/".into(),
                cutoff: Cutoff::PlusInfinity,
            },
            Rule {
                pattern: "/my report/".into(),
                cutoff: Cutoff::At((1_790_856_000 << 32) + 3),
            },
            Rule {
                pattern: "*.iso".into(),
                cutoff: Cutoff::MinusInfinity,
            },
        ];
        let text = format(&given);
        assert!(text.starts_with("follow /docs/\nfrozen 2026-10-01T12:00:00."));
        assert!(text.ends_with(" /my report/\nfree *.iso\n"));
        assert_eq!(rules(&text, 0).unwrap(), given);
    }

    #[test]
    fn frozen_now_takes_the_time_given_and_comments_say_nothing() {
        let text = "# keep the report\n\n  frozen   now  /report/\r\nfree  *.iso\n";
        let lines = parse(text, 42);
        assert_eq!(
            lines,
            [
                Line {
                    number: 3,
                    rule: Ok(Rule {
                        pattern: "/report/".into(),
                        cutoff: Cutoff::At(42),
                    }),
                },
                Line {
                    number: 4,
                    rule: Ok(Rule {
                        pattern: "*.iso".into(),
                        cutoff: Cutoff::MinusInfinity,
                    }),
                },
            ]
        );
        assert_eq!(rules("", 0).unwrap(), []);
    }

    #[test]
    fn each_line_that_is_no_rule_says_why() {
        let text =
            "keep /a/\nfollow\nfrozen\nfrozen yesterday /a/\nfree !a\nfollow /a{b/\nfollow /ok/\n";
        let lines = parse(text, 0);
        let errors: Vec<&str> = lines
            .iter()
            .filter_map(|line| line.rule.as_ref().err().map(String::as_str))
            .collect();
        assert_eq!(errors.len(), 6, "{errors:?}");
        assert!(errors[0].contains("not a mode"));
        assert!(errors[1].contains("needs a pattern"));
        assert!(errors[2].contains("needs a time"));
        assert!(errors[3].contains("RFC 3339"));
        assert!(errors[4].contains("negated"));
        assert!(errors[5].contains("gitignore"));
        let error = rules(text, 0).unwrap_err().to_string();
        assert!(error.starts_with("line 1: "), "{error}");
        assert!(error.contains("; line 6: "), "{error}");
    }
}
