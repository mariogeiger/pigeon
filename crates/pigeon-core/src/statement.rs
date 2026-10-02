//! Statements: the signed files in the hidden drop folder `.pigeon` that
//! record members, rebindings, the group's relay, and the changes waiting
//! for the group as suggestions, with their paths and bodies.

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use crate::clock::Stamp;
use crate::name::MemberName;
use crate::patch::{Content, VersionRef};
use crate::path::GroupPath;

/// The folder that holds every statement.
pub const STATEMENTS: &str = ".pigeon";

/// The file whose first valid creation claims `name` for its member key.
///
/// # Panics
/// Never: every part of the path is portable by construction.
#[must_use]
pub fn member_path(name: &MemberName) -> GroupPath {
    GroupPath::parse(&format!("{STATEMENTS}/members/{name}")).expect("member names are portable")
}

/// The member whose file `path` is, if it is a member file.
#[must_use]
pub fn member_of_path(path: &GroupPath) -> Option<MemberName> {
    let rest = path.as_str().to_lowercase();
    let name = rest.strip_prefix(&format!("{STATEMENTS}/members/"))?;
    MemberName::parse(name).ok()
}

/// The file whose creation, in the patch stamped `stamp`, binds `name` to
/// `key`, or to none, which excludes the member.
///
/// # Panics
/// Never: every part of the path is portable by construction.
#[must_use]
pub fn rebind_path(rebind: &RebindStatement, stamp: &Stamp) -> GroupPath {
    let key = rebind
        .key
        .map_or_else(|| NO_KEY.to_owned(), |key| key.to_string());
    GroupPath::parse(&format!(
        "{STATEMENTS}/{REBINDS}/{}/{}-{key}",
        rebind.name,
        stamp.label()
    ))
    .expect("names, labels and keys are portable")
}

const REBINDS: &str = "rebinds";
const NO_KEY: &str = "none";

/// Whether `path` lies where rebinding files do.
#[must_use]
pub fn is_rebind_path(path: &GroupPath) -> bool {
    path.as_str()
        .to_lowercase()
        .starts_with(&format!("{STATEMENTS}/{REBINDS}/"))
}

/// The rebinding a file at `path` states, if `path` is a well-formed
/// rebinding file.
#[must_use]
pub fn rebind_of_path(path: &GroupPath) -> Option<RebindStatement> {
    let lower = path.as_str().to_lowercase();
    let rest = lower.strip_prefix(&format!("{STATEMENTS}/{REBINDS}/"))?;
    let (name, file) = rest.split_once('/')?;
    let (_, key) = file.rsplit_once('-')?;
    let key = if key == NO_KEY {
        None
    } else {
        Some(key.parse().ok()?)
    };
    Some(RebindStatement {
        name: MemberName::parse(name).ok()?,
        key,
    })
}

/// The file of the relay choice made in the patch stamped `stamp`; the
/// latest choice holds.
///
/// # Panics
/// Never: every part of the path is portable by construction.
#[must_use]
pub fn relay_path(stamp: &Stamp) -> GroupPath {
    GroupPath::parse(&format!("{STATEMENTS}/{RELAYS}/{}.json", stamp.label()))
        .expect("labels are portable")
}

const RELAYS: &str = "relays";

/// Whether `path` lies where relay choices do.
#[must_use]
pub fn is_relay_path(path: &GroupPath) -> bool {
    path.as_str()
        .to_lowercase()
        .starts_with(&format!("{STATEMENTS}/{RELAYS}/"))
}

/// The file of the suggestion first made in the patch stamped `stamp`;
/// deleting it decides the suggestion.
///
/// # Panics
/// Never: every part of the path is portable by construction.
#[must_use]
pub fn suggestion_path(stamp: &Stamp) -> GroupPath {
    GroupPath::parse(&format!("{}/{}.json", suggestions_folder(), stamp.label()))
        .expect("labels are portable")
}

/// The folder holding suggestions.
#[must_use]
pub fn suggestions_folder() -> String {
    format!("{STATEMENTS}/suggestions")
}

/// Whether `path` lies where suggestions do.
#[must_use]
pub fn is_suggestion_path(path: &GroupPath) -> bool {
    path.as_str()
        .to_lowercase()
        .starts_with(&format!("{}/", suggestions_folder()))
}

/// What a member file says: the key its name is bound to.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MemberStatement {
    pub name: MemberName,
    pub key: PublicKey,
}

/// What a rebinding file says: the key a name is now bound to, if any.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RebindStatement {
    pub name: MemberName,
    pub key: Option<PublicKey>,
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

    #[test]
    fn member_paths_round_trip() {
        let name = MemberName::parse("mario").unwrap();
        assert_eq!(member_of_path(&member_path(&name)), Some(name));
        assert_eq!(
            member_of_path(&GroupPath::parse(".pigeon/members/Mario").unwrap())
                .unwrap()
                .as_str(),
            "mario"
        );
        assert_eq!(
            member_of_path(&GroupPath::parse(".pigeon/requests/x").unwrap()),
            None
        );
    }

    #[test]
    fn rebind_paths_round_trip() {
        let stamp = Stamp {
            time: 7,
            machine: iroh_base::SecretKey::from_bytes(&[1; 32]).public(),
        };
        let name = MemberName::parse("bob").unwrap();
        for key in [
            None,
            Some(iroh_base::SecretKey::from_bytes(&[2; 32]).public()),
        ] {
            let rebind = RebindStatement {
                name: name.clone(),
                key,
            };
            let path = rebind_path(&rebind, &stamp);
            assert!(is_rebind_path(&path));
            assert_eq!(rebind_of_path(&path), Some(rebind));
        }
        let stray = GroupPath::parse(".pigeon/rebinds/bob/notes.txt").unwrap();
        assert!(is_rebind_path(&stray));
        assert_eq!(rebind_of_path(&stray), None);
        assert!(!is_rebind_path(&member_path(&name)));
    }

    #[test]
    fn relay_paths_lie_in_their_folder() {
        let stamp = Stamp {
            time: 7,
            machine: iroh_base::SecretKey::from_bytes(&[1; 32]).public(),
        };
        assert!(is_relay_path(&relay_path(&stamp)));
        assert!(!is_relay_path(&suggestion_path(&stamp)));
        assert!(is_suggestion_path(&suggestion_path(&stamp)));
        assert!(!is_suggestion_path(&relay_path(&stamp)));
    }
}
