//! Statements: the signed files in the folder `.pigeon` that record
//! members, the group's relay, and the changes waiting for the group as
//! suggestions, with their paths and bodies. A path is a statement's, or
//! lies in one kind's folder, as its key says, whatever its case.

use std::sync::LazyLock;

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use crate::clock::Stamp;
use crate::name::MemberName;
use crate::patch::{Change, Content, VersionRef};
use crate::path::{GroupPath, PathKey};

/// The folder that holds every statement.
pub const STATEMENTS: &str = ".pigeon";
const MEMBERS: &str = "members";
const RELAYS: &str = "relays";
const SUGGESTIONS: &str = "suggestions";

/// The path of `names` in the statements folder.
fn statement_path<'a>(names: impl IntoIterator<Item = &'a str>) -> GroupPath {
    GroupPath::from_names(std::iter::once(STATEMENTS).chain(names))
        .expect("statement names are portable")
}

static STATEMENT_FOLDER: LazyLock<PathKey> = LazyLock::new(|| statement_path([]).key());
static MEMBER_FOLDER: LazyLock<PathKey> = LazyLock::new(|| statement_path([MEMBERS]).key());
static RELAY_FOLDER: LazyLock<PathKey> = LazyLock::new(|| statement_path([RELAYS]).key());
static SUGGESTION_FOLDER: LazyLock<PathKey> = LazyLock::new(|| statement_path([SUGGESTIONS]).key());

/// Whether `key` is the statements folder or lies inside it: a path only
/// pigeon writes, which no rule decides.
#[must_use]
pub fn is_statement(key: &PathKey) -> bool {
    key.is_within(&STATEMENT_FOLDER)
}

/// What follows `folder` in `key`, if `key` lies inside it.
fn inside<'a>(key: &'a PathKey, folder: &PathKey) -> Option<&'a str> {
    key.below(folder).filter(|rest| !rest.is_empty())
}

/// The file whose first valid creation claims `name` for its member key.
#[must_use]
pub fn member_path(name: &MemberName) -> GroupPath {
    statement_path([MEMBERS, name.as_str()])
}

/// The member whose file `path` is, if it is a member file.
#[must_use]
pub fn member_of_path(path: &GroupPath) -> Option<MemberName> {
    MemberName::parse(inside(&path.key(), &MEMBER_FOLDER)?).ok()
}

/// The file of the relay choice made in the patch stamped `stamp`; the
/// latest choice holds.
#[must_use]
pub fn relay_path(stamp: &Stamp) -> GroupPath {
    statement_path([RELAYS, &format!("{}.json", stamp.label())])
}

/// Whether `path` lies where relay choices do.
#[must_use]
pub fn is_relay_path(path: &GroupPath) -> bool {
    inside(&path.key(), &RELAY_FOLDER).is_some()
}

/// The file of the suggestion first made in the patch stamped `stamp`;
/// deleting it decides the suggestion.
#[must_use]
pub fn suggestion_path(stamp: &Stamp) -> GroupPath {
    statement_path([SUGGESTIONS, &format!("{}.json", stamp.label())])
}

/// Whether `path` lies where suggestions do.
#[must_use]
pub fn is_suggestion_path(path: &GroupPath) -> bool {
    inside(&path.key(), &SUGGESTION_FOLDER).is_some()
}

/// What a member file says: the key its name is bound to.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MemberStatement {
    pub name: MemberName,
    pub key: PublicKey,
}

/// What a relay file says: the URL of the relay the group's machines use
/// when they cannot connect directly, or none for iroh's public relays.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RelayStatement {
    pub url: Option<String>,
}

/// Why a change waits for the group as a suggestion rather than
/// publishing itself.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Reason {
    /// A change the rules leave to the group: any change but its own
    /// member's in a personal path, and but a new file elsewhere.
    OutsideRules,
    /// A name no portable path can hold, or one that collides by case.
    Unportable(String),
    /// A patch the ledger rejected, such as a lost claim.
    Rejected(String),
    /// The losing side of concurrent changes.
    Superseded,
}

/// One change of a suggestion, at a path that is not always a valid group
/// path, its content, or `None` for a deletion, and the versions it
/// replaces and continues.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SuggestedChange {
    pub path: String,
    pub content: Option<Content>,
    pub replaces: Option<Stamp>,
    pub continues: Option<VersionRef>,
}

impl From<&Change> for SuggestedChange {
    fn from(change: &Change) -> Self {
        Self {
            path: change.path.as_str().to_owned(),
            content: change.content,
            replaces: change.replaces,
            continues: change.continues.clone(),
        }
    }
}

/// What a suggestion file says: changes that anyone may validate, which
/// publishes them, or discard, and why they wait.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Suggestion {
    pub changes: Vec<SuggestedChange>,
    pub reason: Reason,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(text: &str) -> GroupPath {
        GroupPath::parse(text).unwrap()
    }

    #[test]
    fn member_paths_round_trip() {
        let name = MemberName::parse("mario").unwrap();
        assert_eq!(member_of_path(&member_path(&name)), Some(name));
        assert_eq!(
            member_of_path(&path(".Pigeon/Members/Mario"))
                .unwrap()
                .as_str(),
            "mario"
        );
        assert_eq!(member_of_path(&path(".pigeon/relays/x")), None);
        assert_eq!(member_of_path(&path(".pigeon/members")), None);
    }

    #[test]
    fn statements_are_the_folder_and_what_lies_inside_it_whatever_the_case() {
        for statement in [".pigeon", ".pigeon/members/mario", ".PIGEON/notes.txt"] {
            assert!(is_statement(&path(statement).key()), "{statement}");
        }
        for other in [".pigeonx/a", "a/.pigeon/b", ".pigeonignore"] {
            assert!(!is_statement(&path(other).key()), "{other}");
        }
    }

    #[test]
    fn relay_and_suggestion_paths_lie_in_their_folders() {
        let stamp = Stamp {
            time: 7,
            machine: iroh_base::SecretKey::from_bytes(&[1; 32]).public(),
        };
        assert!(is_relay_path(&relay_path(&stamp)));
        assert!(!is_relay_path(&suggestion_path(&stamp)));
        assert!(is_suggestion_path(&suggestion_path(&stamp)));
        assert!(!is_suggestion_path(&relay_path(&stamp)));
        assert!(is_suggestion_path(&path(".pigeon/Suggestions/a.json")));
        assert!(!is_suggestion_path(&path(".pigeon/suggestions")));
        assert!(is_statement(&relay_path(&stamp).key()));
    }
}
