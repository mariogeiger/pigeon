//! Keys and certificates: the group secret that admits machines, the member
//! key derived from a name and password, and the certificate by which a
//! member vouches for each of their machines.

use argon2::Argon2;
use iroh_base::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::clock::MachineId;
use crate::name::MemberName;

/// The secret shared by a group's members; knowing it admits a new machine.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupSecret(pub [u8; 32]);

/// The public identity of a group, which every signature commits to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct GroupId(pub [u8; 32]);

impl GroupSecret {
    #[must_use]
    pub fn id(&self) -> GroupId {
        GroupId(blake3::derive_key("pigeon 2026 group id", &self.0))
    }

    /// Proof that the holder of this secret speaks, bound to both machines
    /// of a connection so that it cannot be replayed on another.
    #[must_use]
    pub fn admission(&self, from: &MachineId, to: &MachineId) -> [u8; 32] {
        let key = blake3::derive_key("pigeon 2026 admission", &self.0);
        let mut hasher = blake3::Hasher::new_keyed(&key);
        hasher.update(from.as_bytes());
        hasher.update(to.as_bytes());
        *hasher.finalize().as_bytes()
    }
}

impl std::fmt::Debug for GroupSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GroupSecret(..)")
    }
}

/// Derives a member's signing key from their name and password, so that the
/// same name and password make the same member on any machine.
///
/// # Panics
/// Never: Argon2's default parameters accept any password and a 32-byte
/// output.
#[must_use]
pub fn member_key(group: &GroupId, name: &MemberName, password: &str) -> SecretKey {
    let mut salt = b"pigeon member ".to_vec();
    salt.extend_from_slice(&group.0);
    salt.extend_from_slice(name.as_str().as_bytes());
    let mut seed = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), &salt, &mut seed)
        .expect("argon2 accepts a 32-byte output");
    SecretKey::from_bytes(&seed)
}

/// A member's statement that a machine is one of theirs.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MachineCert {
    pub name: MemberName,
    pub member: PublicKey,
    pub machine: MachineId,
    pub signature: Signature,
}

fn cert_message(group: &GroupId, name: &MemberName, machine: &MachineId) -> Vec<u8> {
    let mut message = b"pigeon machine ".to_vec();
    message.extend_from_slice(&group.0);
    message.extend_from_slice(machine.as_bytes());
    message.extend_from_slice(name.as_str().as_bytes());
    message
}

impl MachineCert {
    #[must_use]
    pub fn issue(
        group: &GroupId,
        name: MemberName,
        member: &SecretKey,
        machine: MachineId,
    ) -> Self {
        let signature = member.sign(&cert_message(group, &name, &machine));
        Self {
            name,
            member: member.public(),
            machine,
            signature,
        }
    }

    /// Whether the member key really signed this certificate.
    #[must_use]
    pub fn is_valid(&self, group: &GroupId) -> bool {
        self.member
            .verify(
                &cert_message(group, &self.name, &self.machine),
                &self.signature,
            )
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_and_password_give_the_same_member() {
        let group = GroupSecret([1; 32]).id();
        let name = MemberName::parse("mario").unwrap();
        let a = member_key(&group, &name, "hunter2");
        let b = member_key(&group, &name, "hunter2");
        let c = member_key(&group, &name, "hunter3");
        assert_eq!(a.public(), b.public());
        assert_ne!(a.public(), c.public());
    }

    #[test]
    fn certificates_bind_machine_and_group() {
        let group = GroupSecret([1; 32]).id();
        let other = GroupSecret([2; 32]).id();
        let name = MemberName::parse("mario").unwrap();
        let member = SecretKey::from_bytes(&[3; 32]);
        let machine = SecretKey::from_bytes(&[4; 32]).public();
        let cert = MachineCert::issue(&group, name, &member, machine);
        assert!(cert.is_valid(&group));
        assert!(!cert.is_valid(&other));
        let mut moved = cert.clone();
        moved.machine = SecretKey::from_bytes(&[5; 32]).public();
        assert!(!moved.is_valid(&group));
    }
}
