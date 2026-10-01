//! Selections: rules that map a gitignore pattern to a cutoff time, the last
//! matching rule winning, which say which version of each file a machine
//! holds. The statements folder is always followed. Exact patterns name one
//! path literally, and a new exact rule drops the exact rules it masks,
//! while a selection replaced whole keeps its rules as given.

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

/// The gitignore pattern that matches exactly the path `path` and, were it
/// a folder, everything inside it.
#[must_use]
pub fn exact_pattern(path: &GroupPath) -> String {
    let mut pattern = String::from("/");
    for character in path.as_str().chars() {
        if matches!(character, '[' | ']' | '{' | '}' | '*' | '?' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern
}

/// The gitignore pattern that matches everything inside the folder `folder`.
#[must_use]
pub fn folder_pattern(folder: &GroupPath) -> String {
    exact_pattern(folder) + "/"
}

/// What an exact pattern names: one literal path, and whether only as a
/// folder.
#[derive(PartialEq, Eq, Debug)]
struct Scope {
    path: String,
    folder: bool,
}

impl Scope {
    /// The scope of `pattern`, when it is anchored and has no wildcard.
    fn of(pattern: &str) -> Option<Self> {
        let rest = pattern.strip_prefix('/')?;
        let (rest, folder) = match rest.strip_suffix('/') {
            Some(inner) => (inner, true),
            None => (rest, false),
        };
        let mut path = String::new();
        let mut characters = rest.chars();
        while let Some(character) = characters.next() {
            match character {
                '\\' => path.push(characters.next()?),
                '*' | '?' | '[' | ']' | '{' | '}' => return None,
                _ => path.push(character),
            }
        }
        GroupPath::parse(&path).ok()?;
        Some(Self { path, folder })
    }

    /// Whether every path `other` matches, this scope matches too.
    fn masks(&self, other: &Self) -> bool {
        let inside = other
            .path
            .strip_prefix(&self.path)
            .is_some_and(|rest| rest.starts_with('/'));
        inside || (other.path == self.path && (other.folder || !self.folder))
    }
}

/// A digest of `rules`, which changes whenever they do.
#[must_use]
pub fn version<'a>(rules: impl IntoIterator<Item = &'a Rule>) -> String {
    let rules: Vec<&Rule> = rules.into_iter().collect();
    let bytes = serde_json::to_vec(&rules).unwrap_or_default();
    blake3::hash(&bytes).to_hex()[..16].to_owned()
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

    /// Compiles `rules` and keeps them as given, masked ones included.
    ///
    /// # Errors
    /// Returns the first invalid pattern.
    pub fn exactly(rules: impl IntoIterator<Item = Rule>) -> Result<Self, PatternError> {
        let rules = rules
            .into_iter()
            .map(|rule| compile(&rule.pattern).map(|matcher| (rule, matcher)))
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    /// Makes `rule` the last rule, replacing any rule with the same pattern.
    /// An exact rule also drops the earlier exact rules whose paths it
    /// covers, which it masks so that they could never win again.
    ///
    /// # Errors
    /// Returns why the pattern is invalid.
    pub fn set(&mut self, rule: Rule) -> Result<(), PatternError> {
        let matcher = compile(&rule.pattern)?;
        let scope = Scope::of(&rule.pattern);
        self.rules.retain(|(old, _)| {
            let masked = scope
                .as_ref()
                .zip(Scope::of(&old.pattern))
                .is_some_and(|(new, old)| new.masks(&old));
            old.pattern != rule.pattern && !masked
        });
        self.rules.push((rule, matcher));
        Ok(())
    }

    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.rules.iter().map(|(rule, _)| rule)
    }

    /// The version of the rules, as `version` gives it.
    #[must_use]
    pub fn version(&self) -> String {
        version(self.rules())
    }

    /// The positions of the rules that match `path`, in order.
    pub fn matching<'a>(&'a self, path: &'a GroupPath) -> impl Iterator<Item = usize> + 'a {
        self.rules
            .iter()
            .enumerate()
            .filter(|(_, (_, matcher))| matches(matcher, path))
            .map(|(index, _)| index)
    }

    /// The position of the rule that decides `path`: the last matching
    /// one, none for statements, which are always followed.
    #[must_use]
    pub fn decider(&self, path: &GroupPath) -> Option<usize> {
        if path.is_inside(STATEMENTS) {
            return None;
        }
        self.rules
            .iter()
            .rposition(|(_, matcher)| matches(matcher, path))
    }

    /// The cutoff of `path`: that of the last matching rule, $-\infty$ when
    /// none matches, and $+\infty$ for statements.
    #[must_use]
    pub fn cutoff(&self, path: &GroupPath) -> Cutoff {
        if path.is_inside(STATEMENTS) {
            return Cutoff::PlusInfinity;
        }
        self.decider(path)
            .map_or(Cutoff::MinusInfinity, |index| self.rules[index].0.cutoff)
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
    fn the_decider_is_the_last_of_the_matching_rules() {
        let rule = |pattern: &str| Rule {
            pattern: pattern.into(),
            cutoff: Cutoff::PlusInfinity,
        };
        let selection = Selection::exactly([rule("/a/"), rule("*.x"), rule("/a/")]).unwrap();
        let file = path("a/b.x");
        assert_eq!(selection.matching(&file).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(selection.decider(&file), Some(2));
        assert_eq!(selection.decider(&path("c")), None);
        assert_eq!(selection.decider(&path(".pigeon/members/x")), None);
        let other = Selection::exactly([rule("/a/"), rule("*.x")]).unwrap();
        assert_ne!(selection.version(), other.version());
        assert_eq!(
            other.version(),
            Selection::new(other.rules().cloned()).unwrap().version()
        );
        assert_eq!(other.version(), version(other.rules()));
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

    #[test]
    fn an_exact_pattern_matches_only_its_path() {
        let file = path("a/[b] c.txt");
        let pattern = exact_pattern(&file);
        assert_eq!(pattern, "/a/\\[b\\] c.txt");
        let matcher = compile(&pattern).unwrap();
        assert!(matches(&matcher, &file));
        assert!(!matches(&matcher, &path("a/b c.txt")));
        let braced = path("a{b,c}.txt");
        let matcher = compile(&exact_pattern(&braced)).unwrap();
        assert!(matches(&matcher, &braced) && !matches(&matcher, &path("ab.txt")));
        assert_eq!(
            Scope::of(&pattern),
            Some(Scope {
                path: "a/[b] c.txt".into(),
                folder: false
            })
        );
        let folder = folder_pattern(&path("a"));
        assert_eq!(folder, "/a/");
        assert!(matches(&compile(&folder).unwrap(), &file));
        assert_eq!(
            Scope::of(&folder),
            Some(Scope {
                path: "a".into(),
                folder: true
            })
        );
        for glob in [
            "*.pdf", "a/", "/a/*", "/a?", "/[ab]/", "/{a,b}", "@mario/", "/",
        ] {
            assert_eq!(Scope::of(glob), None, "{glob}");
        }
    }

    #[test]
    fn an_exact_rule_drops_the_exact_rules_it_masks() {
        let rule = |pattern: &str, cutoff| Rule {
            pattern: pattern.into(),
            cutoff,
        };
        let earlier = [
            rule("/a/b.txt", Cutoff::MinusInfinity),
            rule("/a/c/", Cutoff::At(3)),
            rule("*.pdf", Cutoff::MinusInfinity),
            rule("/a", Cutoff::At(4)),
            rule("/ab.txt", Cutoff::MinusInfinity),
            rule("/b/", Cutoff::At(5)),
        ];
        let mut selection = Selection::new(earlier.clone()).unwrap();
        let cutoffs = |selection: &Selection| {
            ["a", "a/b.txt", "a/c/d", "a/e.pdf", "ab.txt", "b/x"]
                .map(|file| selection.cutoff(&path(file)))
        };
        let before = cutoffs(&selection);
        selection.set(rule("/a/", Cutoff::PlusInfinity)).unwrap();
        let patterns: Vec<&str> = selection
            .rules()
            .map(|rule| rule.pattern.as_str())
            .collect();
        assert_eq!(patterns, ["*.pdf", "/a", "/ab.txt", "/b/", "/a/"]);
        let unpruned = Selection::exactly(
            earlier
                .into_iter()
                .chain([rule("/a/", Cutoff::PlusInfinity)]),
        )
        .unwrap();
        assert_eq!(unpruned.rules().count(), 7);
        assert_eq!(cutoffs(&selection), cutoffs(&unpruned));
        assert_ne!(cutoffs(&selection), before);
        selection.set(rule("/a", Cutoff::MinusInfinity)).unwrap();
        let patterns: Vec<&str> = selection
            .rules()
            .map(|rule| rule.pattern.as_str())
            .collect();
        assert_eq!(patterns, ["*.pdf", "/ab.txt", "/b/", "/a"]);
    }
}
