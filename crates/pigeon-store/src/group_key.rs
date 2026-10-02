//! The group key: the string members share to admit a new machine. It holds
//! the group's name and identity, its current secret, and machines to dial
//! first, in base32 so that it survives chat apps and terminals.

use std::fmt;
use std::str::FromStr;

use data_encoding::BASE32_NOPAD;
use pigeon_core::clock::MachineId;
use pigeon_core::identity::{GroupId, GroupSecret};
use pigeon_core::name::MemberName;

/// Everything a new machine needs to find and join a group.
#[derive(Clone, PartialEq, Eq)]
pub struct GroupKey {
    /// The group's name, which also names its root folder; it follows the
    /// rules of member names so that it is valid in any path.
    pub name: MemberName,
    /// The identity every signature commits to, which never changes.
    pub group: GroupId,
    /// The secret that admits new machines.
    pub secret: GroupSecret,
    pub bootstrap: Vec<MachineId>,
}

/// A fresh random group secret.
///
/// # Panics
///
/// Panics if the operating system has no source of randomness.
#[must_use]
pub fn random_secret() -> GroupSecret {
    let mut secret = [0; 32];
    getrandom::fill(&mut secret).expect("the system provides randomness");
    GroupSecret(secret)
}

/// Why a string is not a group key.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GroupKeyError {
    #[error("a group key reads <name>-<base32>")]
    Shape,
    #[error("invalid group name: {0}")]
    Name(#[from] pigeon_core::name::NameError),
    #[error("the part after the name is not base32")]
    Encoding,
    #[error("the group key is truncated")]
    Length,
    #[error("the group key names an invalid machine")]
    Machine,
}

impl GroupKey {
    /// A key for a new group, whose identity derives from its first secret.
    #[must_use]
    pub fn generate(name: MemberName, bootstrap: Vec<MachineId>) -> Self {
        let secret = random_secret();
        Self {
            name,
            group: secret.id(),
            secret,
            bootstrap,
        }
    }
}

impl fmt::Display for GroupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut bytes = self.group.0.to_vec();
        bytes.extend_from_slice(&self.secret.0);
        for machine in &self.bootstrap {
            bytes.extend_from_slice(machine.as_bytes());
        }
        let encoded = BASE32_NOPAD.encode(&bytes).to_ascii_lowercase();
        write!(f, "{}-{encoded}", self.name)
    }
}

impl fmt::Debug for GroupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GroupKey({}, {:?})", self.name, self.bootstrap)
    }
}

impl FromStr for GroupKey {
    type Err = GroupKeyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (name, encoded) = text.trim().split_once('-').ok_or(GroupKeyError::Shape)?;
        let name = MemberName::parse(name)?;
        let bytes = BASE32_NOPAD
            .decode(encoded.to_ascii_uppercase().as_bytes())
            .map_err(|_| GroupKeyError::Encoding)?;
        if bytes.len() < 64 || bytes.len() % 32 != 0 {
            return Err(GroupKeyError::Length);
        }
        let (chunks, _) = bytes.as_chunks::<32>();
        let [group, secret, machines @ ..] = chunks else {
            return Err(GroupKeyError::Length);
        };
        let bootstrap = machines
            .iter()
            .map(|chunk| MachineId::from_bytes(chunk).map_err(|_| GroupKeyError::Machine))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            name,
            group: GroupId(*group),
            secret: GroupSecret(*secret),
            bootstrap,
        })
    }
}

impl serde::Serialize for GroupKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for GroupKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    fn name(text: &str) -> MemberName {
        MemberName::parse(text).unwrap()
    }

    #[test]
    fn round_trips_through_its_text() {
        let machines = vec![
            SecretKey::generate().public(),
            SecretKey::generate().public(),
        ];
        let key = GroupKey::generate(name("cheapmo"), machines);
        let text = key.to_string();
        assert!(text.starts_with("cheapmo-"));
        assert_eq!(text.parse::<GroupKey>().unwrap(), key);
        assert_eq!(
            text.to_ascii_uppercase().parse::<GroupKey>(),
            Err(GroupKeyError::Name(
                pigeon_core::name::NameError::Character('C')
            ))
        );
    }

    #[test]
    fn the_group_identity_reads_back_apart_from_the_secret() {
        let mut key = GroupKey::generate(name("g"), Vec::new());
        let group = key.group;
        key.secret = random_secret();
        let parsed: GroupKey = key.to_string().parse().unwrap();
        assert_eq!(parsed.group, group);
        assert_ne!(parsed.secret.id(), group);
        assert_eq!(parsed, key);
    }

    #[test]
    fn tolerates_surrounding_whitespace_and_needs_no_bootstrap() {
        let key = GroupKey::generate(name("g"), Vec::new());
        assert_eq!(format!("  {key}\n").parse::<GroupKey>().unwrap(), key);
    }

    #[test]
    fn rejects_malformed_keys() {
        assert_eq!("nodash".parse::<GroupKey>(), Err(GroupKeyError::Shape));
        assert_eq!("g-!!".parse::<GroupKey>(), Err(GroupKeyError::Encoding));
        assert_eq!("g-aaaaaaaa".parse::<GroupKey>(), Err(GroupKeyError::Length));
    }

    #[test]
    fn two_generated_secrets_differ() {
        let a = GroupKey::generate(name("g"), Vec::new());
        let b = GroupKey::generate(name("g"), Vec::new());
        assert_ne!(a.secret, b.secret);
    }
}
