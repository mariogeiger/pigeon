//! Group paths: `/`-separated, NFC-normalized names relative to a group's
//! root, each valid on Windows, Linux, and macOS, with the caseless key under
//! which two paths are the same claim.

use std::fmt;

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// A portable path inside a group, such as `src/+mario/notes.txt`.
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
    #[error("{0:?} is not in NFC form, which systems write differently")]
    Unnormalized(String),
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

/// Checks that `name`, spelled exactly so, is one portable name of a
/// group path, as a name read from a disk must be.
///
/// # Errors
/// Returns the first rule it breaks.
pub fn check_disk_name(name: &str) -> Result<(), PathError> {
    if name.nfc().ne(name.chars()) {
        return Err(PathError::Unnormalized(name.to_owned()));
    }
    check_name(name)
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

    /// Whether this path is the folder `folder` or lies inside it, as
    /// their keys say.
    #[must_use]
    pub fn is_within(&self, folder: &GroupPath) -> bool {
        self.key().is_within(&folder.key())
    }

    /// This path, which is the folder `from` or lies inside it, moved with
    /// `from` to `to`.
    ///
    /// # Errors
    /// Returns why the moved path is not portable.
    pub fn moved(&self, from: &GroupPath, to: &GroupPath) -> Result<Self, PathError> {
        Self::from_names(to.names().chain(self.names().skip(from.names().count())))
    }
}

impl PathKey {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// What follows the folder `folder` in this path: empty for the folder
    /// itself, none when the path lies outside it.
    #[must_use]
    pub fn below(&self, folder: &PathKey) -> Option<&str> {
        match self.0.strip_prefix(&folder.0)? {
            "" => Some(""),
            rest => rest.strip_prefix('/'),
        }
    }

    /// Whether this path is the folder `folder` or lies inside it.
    #[must_use]
    pub fn is_within(&self, folder: &PathKey) -> bool {
        self.below(folder).is_some()
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
    fn a_disk_name_must_be_spelled_in_nfc() {
        assert!(check_disk_name("caf\u{e9}.txt").is_ok());
        assert_eq!(
            check_disk_name("caf\u{65}\u{301}.txt"),
            Err(PathError::Unnormalized("caf\u{65}\u{301}.txt".into()))
        );
        assert!(check_disk_name("a:b").is_err());
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
    fn folders_hold_whole_names_whatever_their_case() {
        let parse = |text| GroupPath::parse(text).unwrap();
        let path = parse("src/+mario/a.txt");
        for folder in ["src", "SRC/+Mario", "src/+mario/a.txt"] {
            assert!(path.is_within(&parse(folder)), "{folder}");
        }
        assert!(!path.is_within(&parse("sr")));
        assert!(!path.is_within(&parse("src/+mario/a")));
        assert_eq!(path.key().below(&parse("Src").key()), Some("+mario/a.txt"));
        assert_eq!(path.key().below(&path.key()), Some(""));
        assert_eq!(
            path.moved(&parse("SRC"), &parse("lib/x")).unwrap().as_str(),
            "lib/x/+mario/a.txt"
        );
        assert_eq!(
            path.moved(&path, &parse("b.txt")).unwrap().as_str(),
            "b.txt"
        );
    }
}
