//! A draft selection as text, one rule per line as the rule prints, such as
//! `follow /docs/` or `pin now /report/`; blank lines and lines starting
//! with `#` say nothing. Reads and writes the rules, and previews what
//! saving a draft would change, with the times each pin can choose.

use anyhow::{Result, anyhow};
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_sync::views::PinTime;
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

/// `rules` as a draft, one line each.
#[must_use]
pub fn format(rules: &[Rule]) -> String {
    rules.iter().map(|rule| rule.to_string() + "\n").collect()
}

/// Reads every rule line of `text`, `now` standing for `pin now`.
#[must_use]
pub fn parse(text: &str, now: u64) -> Vec<Line> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim_end_matches('\r')))
        .filter(|(_, line)| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|(number, line)| Line {
            number,
            rule: Rule::parse(line, || now),
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
    let times = lines
        .iter()
        .map(|line| match &line.rule {
            Ok(Rule {
                pattern,
                cutoff: Cutoff::At(_),
            }) => engine.pin_times(pattern).map(Some),
            _ => Ok(None),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(preview_json(&lines, &valid, &preview, &times))
}

/// `preview` as JSON whose rules are `lines`, the valid ones at `valid`,
/// each pin with the `times` it can choose.
fn preview_json(
    lines: &[Line],
    valid: &[usize],
    preview: &Preview,
    times: &[Option<Vec<PinTime>>],
) -> Value {
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
                    "rule": rule.to_string(),
                    "pattern": rule.pattern,
                    "matches": effect.map(|effect| effect.matches),
                    "decides": effect.map(|effect| effect.decides),
                    "times": times[index],
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
        assert!(text.starts_with("follow /docs/\npin 2026-10-01T12:00:00."));
        assert!(text.ends_with(" /my report/\nfree *.iso\n"));
        assert_eq!(rules(&text, 0).unwrap(), given);
    }

    #[test]
    fn pin_now_takes_the_time_given_and_comments_say_nothing() {
        let text = "# keep the report\n\n  pin   now  /report/\r\nfree  *.iso\n";
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
        let text = "keep /a/\nfollow /ok/\n\nfree !a\n";
        let lines = parse(text, 0);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].rule.as_ref().unwrap_err().contains("not a mode"));
        assert!(lines[1].rule.is_ok());
        assert_eq!(lines[2].number, 4);
        let error = rules(text, 0).unwrap_err().to_string();
        assert!(error.starts_with("line 1: "), "{error}");
        assert!(error.contains("; line 4: "), "{error}");
    }
}
