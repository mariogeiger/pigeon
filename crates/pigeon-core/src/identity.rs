//! Keys and certificates: the group secret that admits new machines and its
//! renewals, the member key derived from a name, and the certificate by
//! which a member vouches for each of their machines.

use iroh_base::{PublicKey, SecretKey, Signature};
use serde::{Deserialize, Serialize};

use crate::clock::{MachineId, Stamp};
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

/// When a group secret replaced the one before: after the exclusion stamped
/// `after`, drawn by the machine `by`. Renewals order totally, so machines
/// that each draw one agree on the greatest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Renewal {
    pub after: Stamp,
    pub by: MachineId,
}

/// A group secret and the renewal that made it, none for the first secret.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RenewedSecret {
    pub secret: GroupSecret,
    pub renewal: Option<Renewal>,
}

impl RenewedSecret {
    /// Whether this secret replaces `other`: its renewal is later.
    #[must_use]
    pub fn supersedes(&self, other: &Self) -> bool {
        self.renewal > other.renewal
    }

    /// Whether the exclusion stamped `exclusion` calls for a new secret.
    #[must_use]
    pub fn predates(&self, exclusion: Stamp) -> bool {
        self.renewal.is_none_or(|renewal| renewal.after < exclusion)
    }
}

/// Derives a member's signing key from their name, so that the same name
/// makes the same member on any machine. Anyone holding the group key can
/// derive it: the members trust each other, and the key only tells which
/// name a machine speaks for.
#[must_use]
pub fn member_key(group: &GroupId, name: &MemberName) -> SecretKey {
    let mut hasher = blake3::Hasher::new_derive_key("pigeon 2026 member");
    hasher.update(&group.0);
    hasher.update(name.as_str().as_bytes());
    SecretKey::from_bytes(hasher.finalize().as_bytes())
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
    fn a_name_makes_one_member_per_group() {
        let group = GroupSecret([1; 32]).id();
        let other = GroupSecret([2; 32]).id();
        let mario = MemberName::parse("mario").unwrap();
        let laurent = MemberName::parse("laurent").unwrap();
        let key = member_key(&group, &mario).public();
        assert_eq!(key, member_key(&group, &mario).public());
        assert_ne!(key, member_key(&group, &laurent).public());
        assert_ne!(key, member_key(&other, &mario).public());
    }

    #[test]
    fn the_latest_renewal_wins_and_follows_every_exclusion() {
        let machine = |seed| SecretKey::from_bytes(&[seed; 32]).public();
        let stamp = |time| Stamp {
            time,
            machine: machine(9),
        };
        let renewed = |time, by, byte| RenewedSecret {
            secret: GroupSecret([byte; 32]),
            renewal: Some(Renewal {
                after: stamp(time),
                by: machine(by),
            }),
        };
        let first = RenewedSecret {
            secret: GroupSecret([0; 32]),
            renewal: None,
        };
        let (a, b, later) = (renewed(5, 1, 1), renewed(5, 2, 2), renewed(6, 1, 3));
        assert!(a.supersedes(&first) && !first.supersedes(&a));
        assert_eq!(a.supersedes(&b), !b.supersedes(&a));
        assert!(later.supersedes(&a) && later.supersedes(&b));
        assert!(!a.supersedes(&a));
        assert!(first.predates(stamp(1)));
        assert!(a.predates(stamp(6)) && !a.predates(stamp(5)));
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
