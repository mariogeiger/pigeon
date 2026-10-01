//! Member names: 1 to 32 characters among `a`–`z` and `0`–`9`, valid in any
//! path on every system, so that `+<name>` can name a personal folder.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The character that, followed by a member's name, names their personal
/// folder.
pub const FOLDER_PREFIX: char = '+';

/// A valid member name.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct MemberName(String);

/// Why a string is not a member name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    #[error("a name needs 1 to 32 characters, this one has {0}")]
    Length(usize),
    #[error("a name uses only a-z and 0-9, not {0:?}")]
    Character(char),
}

impl MemberName {
    /// Checks `text` against the name rule.
    ///
    /// # Errors
    /// Returns why `text` breaks the rule.
    pub fn parse(text: &str) -> Result<Self, NameError> {
        let length = text.chars().count();
        if !(1..=32).contains(&length) {
            return Err(NameError::Length(length));
        }
        if let Some(bad) = text
            .chars()
            .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
        {
            return Err(NameError::Character(bad));
        }
        Ok(Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The personal folder name `+<name>`.
    #[must_use]
    pub fn folder(&self) -> String {
        format!("{FOLDER_PREFIX}{}", self.0)
    }
}

impl TryFrom<String> for MemberName {
    type Error = NameError;
    fn try_from(text: String) -> Result<Self, NameError> {
        Self::parse(&text)
    }
}

impl From<MemberName> for String {
    fn from(name: MemberName) -> String {
        name.0
    }
}

impl fmt::Display for MemberName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for MemberName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_lowercase_letters_and_digits() {
        assert!(MemberName::parse("mario").is_ok());
        assert!(MemberName::parse("a1").is_ok());
        assert!(MemberName::parse(&"z".repeat(32)).is_ok());
    }

    #[test]
    fn rejects_everything_else() {
        assert_eq!(MemberName::parse(""), Err(NameError::Length(0)));
        assert_eq!(
            MemberName::parse(&"z".repeat(33)),
            Err(NameError::Length(33))
        );
        assert_eq!(MemberName::parse("Mario"), Err(NameError::Character('M')));
        assert_eq!(MemberName::parse("a-b"), Err(NameError::Character('-')));
        assert_eq!(MemberName::parse("é"), Err(NameError::Character('é')));
    }
}
