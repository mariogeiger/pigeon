//! Statements: the signed files in the hidden drop folder `.pigeon` that
//! record members, requests, and decisions, with their paths and bodies.

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

/// What a member file says: the key its name is bound to.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MemberStatement {
    pub name: MemberName,
    pub key: PublicKey,
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
}
