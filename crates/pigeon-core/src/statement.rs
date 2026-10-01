//! Statements: the signed files in the hidden drop folder `.pigeon` that
//! record members, rebindings, requests, decisions, and the group's relay,
//! with their paths and bodies.

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use crate::clock::Stamp;
use crate::name::MemberName;
use crate::patch::Change;
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

/// The file of the request stamped `stamp`.
///
/// # Panics
/// Never: every part of the path is portable by construction.
#[must_use]
pub fn request_path(stamp: &Stamp) -> GroupPath {
    GroupPath::parse(&format!("{STATEMENTS}/requests/{}.json", stamp.label()))
        .expect("labels are portable")
}

/// The file of the owner's decision on the request at `request`.
///
/// # Panics
/// Never: every part of the path is portable by construction.
#[must_use]
pub fn decision_path(request: &GroupPath) -> GroupPath {
    GroupPath::parse(&format!("{STATEMENTS}/decisions/{}", request.file_name()))
        .expect("request names are portable")
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

/// Whether a request waits for acceptance or needs none.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Propose,
    Force,
}

/// A patch on one owner's files, waiting for that owner's machine.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RequestStatement {
    pub owner: MemberName,
    pub mode: Mode,
    pub changes: Vec<Change>,
    pub message: String,
}

/// The owner's answer to a proposal.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Accept,
    Refuse,
}

/// A decision file's body.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DecisionStatement {
    pub request: GroupPath,
    pub decision: Decision,
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
        assert!(!is_relay_path(&request_path(&stamp)));
    }
}
