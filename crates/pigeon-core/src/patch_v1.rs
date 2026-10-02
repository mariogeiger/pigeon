//! Patches in the first format, which pigeon signed until 0.6: changes
//! without the version they continue, and the request a patch applied.
//! Machines keep verifying them, so the patches signed then stay valid.

use iroh_base::Signature;
use serde::{Deserialize, Serialize};

use crate::clock::Stamp;
use crate::identity::{GroupId, MachineCert};
use crate::patch::{Change, Content, Format, Patch, SignedPatch};
use crate::path::GroupPath;

/// A change in the first format.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ChangeV1 {
    pub path: GroupPath,
    pub content: Option<Content>,
    pub replaces: Option<Stamp>,
}

/// A patch in the first format.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PatchV1 {
    pub stamp: Stamp,
    pub changes: Vec<ChangeV1>,
    pub applies: Option<GroupPath>,
}

/// A signed patch as the first format stored and sent it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SignedPatchV1 {
    pub patch: PatchV1,
    pub cert: MachineCert,
    pub signature: Signature,
}

impl From<SignedPatchV1> for SignedPatch {
    fn from(signed: SignedPatchV1) -> Self {
        let PatchV1 {
            stamp,
            changes,
            applies,
        } = signed.patch;
        let changes = changes
            .into_iter()
            .map(|change| Change {
                path: change.path,
                content: change.content,
                replaces: change.replaces,
                continues: None,
            })
            .collect();
        Self {
            patch: Patch { stamp, changes },
            cert: signed.cert,
            signature: signed.signature,
            signed_as: Format::V1 { applies },
        }
    }
}

/// The bytes the first format signed for `patch` applying `applies`, or
/// `None` when a change continues a version, which that format cannot say.
#[must_use]
pub fn signed_message(
    group: &GroupId,
    patch: &Patch,
    applies: Option<GroupPath>,
) -> Option<Vec<u8>> {
    let changes = patch
        .changes
        .iter()
        .map(|change| {
            change.continues.is_none().then(|| ChangeV1 {
                path: change.path.clone(),
                content: change.content,
                replaces: change.replaces,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let first = PatchV1 {
        stamp: patch.stamp,
        changes,
        applies,
    };
    let mut message = b"pigeon patch ".to_vec();
    message.extend_from_slice(&group.0);
    message.extend(postcard::to_stdvec(&first).ok()?);
    Some(message)
}
