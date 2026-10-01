//! Group paths: `/`-separated, NFC-normalized names relative to a group's
//! root, each valid on Windows, Linux, and macOS, with the caseless key under
//! which two paths are the same claim.

use std::fmt;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// A portable path inside a group, such as `src/@mario/notes.txt`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct GroupPath(String);

/// A path compared without case: two paths with one key are one claim.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct PathKey(String);

/// Why a name cannot be published as it is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("the path is empty")]
    Empty,
    #[error("{0:?} is not a file name")]
    Relative(String),
    #[error("{name:?} contains {character:?}, which Windows forbids")]
    Character { name: String, character: char },
    #[error("{0:?} is a name Windows reserves")]
    Reserved(String),
    #[error("{0:?} ends with a space or a dot, which Windows drops")]
    Trailing(String),
    #[error("{0:?} is longer than 255 bytes")]
    Long(String),
}

const FORBIDDEN: &[char] = &['<', '>', ':', '"', '\\', '|', '?', '*'];
const RESERVED: &[&str] = &["con", "prn", "aux", "nul"];

fn check_name(name: &str) -> Result<(), PathError> {
    if name.is_empty() || name == "." || name == ".." {
        return Err(PathError::Relative(name.to_owned()));
    }
    if let Some(character) = name
        .chars()
        .find(|c| FORBIDDEN.contains(c) || c.is_control())
    {
        return Err(PathError::Character {
            name: name.to_owned(),
            character,
        });
    }
    let stem = name.split('.').next().unwrap_or(name).to_lowercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|digit| {
            digit.len() == 1 && ('1'..='9').contains(&digit.chars().next().unwrap_or('0'))
        })
    };
    if RESERVED.contains(&stem.as_str()) || numbered("com") || numbered("lpt") {
        return Err(PathError::Reserved(name.to_owned()));
    }
    if name.ends_with(' ') || name.ends_with('.') {
        return Err(PathError::Trailing(name.to_owned()));
    }
    if name.len() > 255 {
        return Err(PathError::Long(name.to_owned()));
    }
    Ok(())
}

impl GroupPath {
    /// Normalizes `text` to NFC and checks that every name in it is portable.
    ///
    /// # Errors
    /// Returns the first rule a name breaks.
    pub fn parse(text: &str) -> Result<Self, PathError> {
        let normalized: String = text.nfc().collect();
        if normalized.is_empty() {
            return Err(PathError::Empty);
        }
        for name in normalized.split('/') {
            check_name(name)?;
        }
        Ok(Self(normalized))
    }

    /// Joins `names` with `/` and parses the result.
    ///
    /// # Errors
    /// Returns the first rule a name breaks.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<Self, PathError> {
        Self::parse(&names.into_iter().collect::<Vec<_>>().join("/"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The names from the root down to the file.
    #[must_use]
    pub fn names(&self) -> impl DoubleEndedIterator<Item = &str> + Clone {
        self.0.split('/')
    }

    /// The last name.
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    /// The caseless key of this path.
    #[must_use]
    pub fn key(&self) -> PathKey {
        PathKey(self.0.to_lowercase())
    }

    /// Whether this path lies inside the folder `prefix`, compared exactly.
    #[must_use]
    pub fn is_inside(&self, prefix: &str) -> bool {
        prefix.is_empty()
            || self
                .0
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    }

    /// Replaces the folder `from` at the start of this path with `to`.
    ///
    /// # Errors
    /// Returns why the moved path is not portable.
    pub fn moved(&self, from: &str, to: &str) -> Result<Self, PathError> {
        let rest = if from.is_empty() {
            self.0.as_str()
        } else {
            self.0
                .strip_prefix(from)
                .map_or(self.0.as_str(), |r| r.trim_start_matches('/'))
        };
        if to.is_empty() {
            Self::parse(rest)
        } else {
            Self::parse(&format!("{to}/{rest}"))
        }
    }
}

impl PathKey {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this path is the folder `folder` or lies inside it.
    #[must_use]
    pub fn is_within(&self, folder: &PathKey) -> bool {
        self.0
            .strip_prefix(&folder.0)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }
}

impl TryFrom<String> for GroupPath {
    type Error = PathError;
    fn try_from(text: String) -> Result<Self, PathError> {
        Self::parse(&text)
    }
}

impl From<GroupPath> for String {
    fn from(path: GroupPath) -> String {
        path.0
    }
}

impl fmt::Display for GroupPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for GroupPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_to_nfc() {
        let decomposed = "caf\u{65}\u{301}.txt";
        assert_eq!(
            GroupPath::parse(decomposed).unwrap().as_str(),
            "caf\u{e9}.txt"
        );
    }

    #[test]
    fn keys_ignore_case() {
        let a = GroupPath::parse("Docs/Read.ME").unwrap();
        let b = GroupPath::parse("docs/read.me").unwrap();
        assert_ne!(a, b);
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn rejects_names_some_system_refuses() {
        for bad in [
            "a/../b", "", "a//b", "a:b", "x/con", "LPT3.txt", "dot.", "space ", "tab\t",
        ] {
            assert!(GroupPath::parse(bad).is_err(), "{bad:?}");
        }
        for good in [
            "console",
            "com10",
            "lpt",
            ".pigeon/members/mario",
            "a b/c.d",
        ] {
            assert!(GroupPath::parse(good).is_ok(), "{good:?}");
        }
    }

    #[test]
    fn folders_are_prefixes_of_whole_names() {
        let path = GroupPath::parse("src/@mario/a.txt").unwrap();
        assert!(path.is_inside("src"));
        assert!(path.is_inside("src/@mario"));
        assert!(path.is_inside(""));
        assert!(!path.is_inside("sr"));
        assert_eq!(
            path.moved("src", "lib").unwrap().as_str(),
            "lib/@mario/a.txt"
        );
        assert_eq!(path.moved("src", "").unwrap().as_str(), "@mario/a.txt");
    }
}
