//! Binding a group's endpoint: over the internet with iroh's address lookup
//! plus local-network discovery limited to the group, or on this host alone
//! with an in-memory lookup, for tests. Both start with no relay, which the
//! node then chooses.

use std::fmt::Write;

use anyhow::Result;
use iroh::address_lookup::MemoryLookup;
use iroh::endpoint::{RelayMode, presets};
use iroh::{Endpoint, RelayMap, SecretKey};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use pigeon_core::identity::GroupId;

/// The local-network service name of a group, so that discovery reports
/// only machines of the same group.
#[must_use]
pub fn service_name(group: &GroupId) -> String {
    group.0[..6]
        .iter()
        .fold(String::from("pigeon-"), |mut name, byte| {
            let _ = write!(name, "{byte:02x}");
            name
        })
}

/// Binds an endpoint reachable over the internet and on the local network.
///
/// # Errors
///
/// Fails if no socket can be bound.
pub async fn bind_internet(
    secret: SecretKey,
    group: &GroupId,
) -> Result<(Endpoint, MdnsAddressLookup)> {
    let mdns = MdnsAddressLookup::builder()
        .service_name(service_name(group))
        .build(secret.public())?;
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret)
        .relay_mode(RelayMode::Custom(RelayMap::empty()))
        .address_lookup(mdns.clone())
        .bind()
        .await?;
    Ok((endpoint, mdns))
}

/// Binds an endpoint reachable only through `lookup`, which learns its
/// address.
///
/// # Errors
///
/// Fails if no socket can be bound.
pub async fn bind_local(secret: SecretKey, lookup: &MemoryLookup) -> Result<Endpoint> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .relay_mode(RelayMode::Custom(RelayMap::empty()))
        .address_lookup(lookup.clone())
        .bind()
        .await?;
    lookup.add_endpoint_info(endpoint.addr());
    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pigeon_core::identity::GroupSecret;

    #[test]
    fn each_group_discovers_under_its_own_name() {
        let a = service_name(&GroupSecret([1; 32]).id());
        let b = service_name(&GroupSecret([2; 32]).id());
        assert!(a.starts_with("pigeon-") && a.len() == 19);
        assert_ne!(a, b);
    }
}
