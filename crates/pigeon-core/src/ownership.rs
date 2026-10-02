//! Who owns a file: reading its path from right to left, file name
//! included, the first tag `+<name>` naming a member makes the file that
//! member's, and a path naming none is a drop file; every tag read before
//! it claims its name.

use crate::name::{MemberName, OWNER_MARK};
use crate::path::GroupPath;

/// The rule governing a file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ownership {
    /// The path names `+<owner>`: the owner's machines publish its
    /// changes at once, and anyone else's become suggestions.
    Personal(MemberName),
    /// The path names no member: a new file publishes itself once it has
    /// settled, and any other change becomes a suggestion.
    Drop,
}

impl Ownership {
    #[must_use]
    pub fn owner(&self) -> Option<&MemberName> {
        match self {
            Self::Personal(owner) => Some(owner),
            Self::Drop => None,
        }
    }
}

/// The names that the tags `+<name>` of one file or folder name refer to,
/// from right to left, compared without case: each tag runs from the mark
/// over every letter and digit that follows.
pub fn tags(name: &str) -> impl Iterator<Item = MemberName> + '_ {
    name.rmatch_indices(OWNER_MARK)
        .filter_map(move |(at, mark)| {
            let rest = &name[at + mark.len()..];
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            MemberName::parse(&rest[..end].to_lowercase()).ok()
        })
}

/// Classifies `path`: reading from right to left, the first tag naming a
/// member makes the file that member's; with none, it is a drop file. Also
/// returns the names of the tags read before it, which a newcomer taking
/// them would turn into the file's owner.
pub fn classify(
    path: &GroupPath,
    is_member: impl Fn(&MemberName) -> bool,
) -> (Ownership, Vec<MemberName>) {
    let mut claimed = Vec::new();
    for name in path.names().rev().flat_map(tags) {
        if is_member(&name) {
            return (Ownership::Personal(name), claimed);
        }
        claimed.push(name);
    }
    (Ownership::Drop, claimed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str) -> MemberName {
        MemberName::parse(name).unwrap()
    }

    #[test]
    fn the_rightmost_tag_naming_a_member_decides() {
        let members = [member("mario"), member("emmy")];
        let is_member = |name: &MemberName| members.contains(name);
        let classify_text = |text: &str| classify(&GroupPath::parse(text).unwrap(), is_member);
        assert_eq!(
            classify_text("src/+mario/a").0,
            Ownership::Personal(member("mario"))
        );
        assert_eq!(
            classify_text("+Mario/+emmy/a").0,
            Ownership::Personal(member("emmy"))
        );
        assert_eq!(
            classify_text("+emmy/texte+mario.txt").0,
            Ownership::Personal(member("mario"))
        );
        assert_eq!(
            classify_text("+mario"),
            (Ownership::Personal(member("mario")), vec![])
        );
        assert_eq!(
            classify_text("+emmy/+build/a"),
            (Ownership::Personal(member("emmy")), vec![member("build")])
        );
        assert_eq!(
            classify_text("+build/a"),
            (Ownership::Drop, vec![member("build")])
        );
        assert_eq!(
            classify_text("docs/notes+mario2.txt"),
            (Ownership::Drop, vec![member("mario2")])
        );
        assert_eq!(classify_text("c++/+-x/a"), (Ownership::Drop, vec![]));
    }

    #[test]
    fn a_tag_takes_every_letter_and_digit_after_the_mark() {
        let names = |text: &str| tags(text).map(|name| name.to_string()).collect::<Vec<_>>();
        assert_eq!(names("a+mario+Emmy.txt"), ["emmy", "mario"]);
        assert_eq!(names("+mario_notes"), ["mario"]);
        assert_eq!(names("+marioNotes"), ["marionotes"]);
        assert!(names("c++").is_empty());
    }
}
