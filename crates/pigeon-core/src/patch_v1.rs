//! Patches in the first format, which pigeon signed until 0.6: changes
//! without the version they continue, and the request a patch applied.
//! Machines keep verifying them, so the patches signed then stay valid.

use serde::Serialize;

use crate::clock::Stamp;
use crate::identity::GroupId;
use crate::patch::{Content, Patch};
use crate::path::GroupPath;

/// A change in the first format.
#[derive(Serialize)]
struct ChangeV1 {
    path: GroupPath,
    content: Option<Content>,
    replaces: Option<Stamp>,
}

/// A patch in the first format.
#[derive(Serialize)]
struct PatchV1 {
    stamp: Stamp,
    changes: Vec<ChangeV1>,
    applies: Option<GroupPath>,
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
