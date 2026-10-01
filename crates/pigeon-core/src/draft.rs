//! Drafts: the new files in drop folders that a machine has not published
//! yet, announced to the other machines with their size and the seconds
//! left before they are published, so that two members adding the same
//! path learn of each other in time. An announcement names every draft of
//! its machine, replacing the one before, and is signed by that machine.

use iroh_base::{SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::clock::MachineId;
use crate::identity::{GroupId, MachineCert};
use crate::path::GroupPath;

/// A new file waiting in a drop folder.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Draft {
    pub path: GroupPath,
    pub size: u64,
    /// Seconds until it is published, as its machine sent it.
    pub due_in: u64,
}

/// Every draft one machine holds now.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Drafts {
    pub machine: MachineId,
    pub drafts: Vec<Draft>,
}

/// Drafts with the certificate of their machine and its signature.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SignedDrafts {
    pub drafts: Drafts,
    pub cert: MachineCert,
    pub signature: Signature,
}

fn signed_message(group: &GroupId, drafts: &Drafts) -> Vec<u8> {
    let mut message = b"pigeon drafts ".to_vec();
    message.extend_from_slice(&group.0);
    message.extend(postcard::to_stdvec(drafts).unwrap_or_default());
    message
}

/// Why signed drafts are not what they claim to be.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DraftsError {
    #[error("the machine certificate is not signed by its member key")]
    Cert,
    #[error("the drafts name another machine than their certificate")]
    Machine,
    #[error("the drafts are not signed by their machine")]
    Signature,
}

impl SignedDrafts {
    /// Signs `drafts` with the machine key that `cert` vouches for.
    #[must_use]
    pub fn sign(group: &GroupId, drafts: Drafts, cert: MachineCert, machine: &SecretKey) -> Self {
        let signature = machine.sign(&signed_message(group, &drafts));
        Self {
            drafts,
            cert,
            signature,
        }
    }

    /// Checks the certificate, that it names the drafts' machine, and the
    /// machine's signature.
    ///
    /// # Errors
    /// Returns the first check that fails.
    pub fn verify(&self, group: &GroupId) -> Result<(), DraftsError> {
        if !self.cert.is_valid(group) {
            return Err(DraftsError::Cert);
        }
        if self.cert.machine != self.drafts.machine {
            return Err(DraftsError::Machine);
        }
        self.drafts
            .machine
            .verify(&signed_message(group, &self.drafts), &self.signature)
            .map_err(|_| DraftsError::Signature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{GroupSecret, member_key};
    use crate::name::MemberName;

    #[test]
    fn drafts_verify_only_as_their_machine_signed_them() {
        let group = GroupSecret([9; 32]).id();
        let name = MemberName::parse("alice").unwrap();
        let member = member_key(&group, &name);
        let machine = SecretKey::from_bytes(&[3; 32]);
        let cert = MachineCert::issue(&group, name, &member, machine.public());
        let drafts = Drafts {
            machine: machine.public(),
            drafts: vec![Draft {
                path: GroupPath::parse("inbox/a.txt").unwrap(),
                size: 12,
                due_in: 280,
            }],
        };
        let signed = SignedDrafts::sign(&group, drafts, cert, &machine);
        assert_eq!(signed.verify(&group), Ok(()));
        let mut forged = signed.clone();
        forged.drafts.drafts[0].size = 13;
        assert_eq!(forged.verify(&group), Err(DraftsError::Signature));
        let mut moved = signed.clone();
        moved.drafts.machine = SecretKey::from_bytes(&[4; 32]).public();
        assert_eq!(moved.verify(&group), Err(DraftsError::Machine));
        assert_eq!(
            signed.verify(&GroupSecret([8; 32]).id()),
            Err(DraftsError::Cert)
        );
    }
}
