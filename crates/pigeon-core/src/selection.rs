//! Selections: rules that map a gitignore pattern to a cutoff time, the last
//! matching rule winning, which say which version of each file a machine
//! holds. The statements folder is always followed.

use std::fmt;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};

use crate::path::GroupPath;
use crate::statement::STATEMENTS;

/// The time $t$ up to which a machine holds a file's last version.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Cutoff {
    /// $t = -\infty$: the file is not held.
    MinusInfinity,
    /// The file's last version dated up to this NTP64 time.
    At(u64),
    /// $t = +\infty$: the file follows every new version.
    PlusInfinity,
}

impl Cutoff {
    /// Whether a version dated `time` is dated up to this cutoff.
    #[must_use]
    pub fn covers(self, time: u64) -> bool {
        match self {
            Self::MinusInfinity => false,
            Self::At(cutoff) => time <= cutoff,
            Self::PlusInfinity => true,
        }
    }
}

/// One selection rule.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Rule {
    pub pattern: String,
    pub cutoff: Cutoff,
}

/// Why a pattern cannot be a rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("{0:?} is negated: give the pattern a cutoff instead of `!`")]
    Negated(String),
    #[error("{pattern:?} is not a gitignore pattern: {reason}")]
    Syntax { pattern: String, reason: String },
}

/// Compiles one pattern in the gitignore syntax.
///
/// # Errors
/// Returns why the pattern is not valid.
pub fn compile(pattern: &str) -> Result<Gitignore, PatternError> {
    if pattern.starts_with('!') {
        return Err(PatternError::Negated(pattern.to_owned()));
    }
    let syntax = |error: ignore::Error| PatternError::Syntax {
        pattern: pattern.to_owned(),
        reason: error.to_string(),
    };
    let mut builder = GitignoreBuilder::new("");
    builder.add_line(None, pattern).map_err(syntax)?;
    builder.build().map_err(syntax)
}

/// Whether `matcher` matches `path` or one of its folders.
#[must_use]
pub fn matches(matcher: &Gitignore, path: &GroupPath) -> bool {
    matcher
        .matched_path_or_any_parents(path.as_str(), false)
        .is_ignore()
}

/// A machine's selection: its rules, compiled.
#[derive(Clone, Default)]
pub struct Selection {
    rules: Vec<(Rule, Gitignore)>,
}

impl Selection {
    /// Compiles `rules`.
    ///
    /// # Errors
    /// Returns the first invalid pattern.
    pub fn new(rules: impl IntoIterator<Item = Rule>) -> Result<Self, PatternError> {
        let mut selection = Self::default();
        for rule in rules {
            selection.set(rule)?;
        }
        Ok(selection)
    }

    /// Makes `rule` the last rule, replacing any rule with the same pattern.
    ///
    /// # Errors
    /// Returns why the pattern is invalid.
    pub fn set(&mut self, rule: Rule) -> Result<(), PatternError> {
        let matcher = compile(&rule.pattern)?;
        self.rules.retain(|(old, _)| old.pattern != rule.pattern);
        self.rules.push((rule, matcher));
        Ok(())
    }

    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter().map(|(rule, _)| rule)
    }

    /// The cutoff of `path`: that of the last matching rule, $-\infty$ when
    /// none matches, and $+\infty$ for statements.
    #[must_use]
    pub fn cutoff(&self, path: &GroupPath) -> Cutoff {
        if path.is_inside(STATEMENTS) {
            return Cutoff::PlusInfinity;
        }
        self.rules
            .iter()
            .rev()
            .find(|(_, matcher)| matches(matcher, path))
            .map_or(Cutoff::MinusInfinity, |(rule, _)| rule.cutoff)
    }
}

impl fmt::Debug for Selection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.rules()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> GroupPath {
        GroupPath::parse(text).unwrap()
    }

    #[test]
    fn the_last_matching_rule_wins() {
        let selection = Selection::new([
            Rule {
                pattern: "/src/".into(),
                cutoff: Cutoff::PlusInfinity,
            },
            Rule {
                pattern: "*.bin".into(),
                cutoff: Cutoff::MinusInfinity,
            },
            Rule {
                pattern: "/src/old/".into(),
                cutoff: Cutoff::At(5),
            },
        ])
        .unwrap();
        assert_eq!(selection.cutoff(&path("src/a.rs")), Cutoff::PlusInfinity);
        assert_eq!(selection.cutoff(&path("src/a.bin")), Cutoff::MinusInfinity);
        assert_eq!(selection.cutoff(&path("src/old/a.bin")), Cutoff::At(5));
        assert_eq!(selection.cutoff(&path("doc/a.rs")), Cutoff::MinusInfinity);
        assert_eq!(
            selection.cutoff(&path(".pigeon/members/x")),
            Cutoff::PlusInfinity
        );
    }

    #[test]
    fn a_star_follows_everything() {
        let selection = Selection::new([Rule {
            pattern: "*".into(),
            cutoff: Cutoff::PlusInfinity,
        }])
        .unwrap();
        for file in ["a.txt", "@alice/notes.txt", "src/deep/a.rs"] {
            assert_eq!(selection.cutoff(&path(file)), Cutoff::PlusInfinity);
        }
    }

    #[test]
    fn setting_a_pattern_again_moves_it_last() {
        let mut selection = Selection::default();
        for (pattern, cutoff) in [
            ("a/", Cutoff::PlusInfinity),
            ("*.x", Cutoff::MinusInfinity),
            ("a/", Cutoff::At(1)),
        ] {
            selection
                .set(Rule {
                    pattern: pattern.into(),
                    cutoff,
                })
                .unwrap();
        }
        assert_eq!(selection.rules().count(), 2);
        assert_eq!(selection.cutoff(&path("a/b.x")), Cutoff::At(1));
        assert!(Cutoff::At(1).covers(1) && !Cutoff::At(1).covers(2));
        assert!(
            Selection::new([Rule {
                pattern: "!a".into(),
                cutoff: Cutoff::PlusInfinity
            }])
            .is_err()
        );
    }
}
