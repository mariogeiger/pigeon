//! The group's relay: the latest relay statement names a relay that
//! carries, for every machine of the group, the traffic no direct
//! connection can, alongside iroh's public relays, or names none, which
//! leaves the public relays alone.

use anyhow::{Context, Result, bail};
use iroh::endpoint::RelayMode;
use iroh::{RelayMap, RelayUrl};
use pigeon_core::statement::{RelayStatement, is_relay_path, relay_path};

use crate::engine::{Engine, Inner, JoinState, Network};

impl Inner {
    /// The relays used whether the group names one or not: iroh's public
    /// relays on the internet, none on this host alone.
    fn public_relays(&self) -> RelayMap {
        match self.options.network {
            Network::Internet => RelayMode::Default.relay_map(),
            Network::Local(_) => RelayMap::empty(),
        }
    }

    /// The public relays with the one the latest relay statement names, or
    /// `None` while its body has not arrived.
    async fn group_relays(&self) -> Result<Option<RelayMap>> {
        let latest = self
            .ledger
            .lock()
            .live()
            .filter(|version| is_relay_path(&version.path))
            .max_by_key(|version| version.stamp)
            .and_then(|version| version.content);
        let Some(content) = latest else {
            return Ok(Some(self.public_relays()));
        };
        if !self.blobs.has(&content.hash).await? {
            return Ok(None);
        }
        let statement: RelayStatement = self.read_statement(&content).await?;
        with_group_relay(self.public_relays(), statement.url.as_deref()).map(Some)
    }

    /// Makes the node use the relay the group names.
    pub(crate) async fn follow_relay(&self) {
        match self.group_relays().await {
            Ok(Some(relays)) => self.node.use_relays(&relays).await,
            Ok(None) => {}
            Err(error) => self.report(format!("following the group's relay: {error:#}")),
        }
    }
}

/// The relays `public` holds, with the group's relay at `url` if it names
/// one.
fn with_group_relay(public: RelayMap, url: Option<&str>) -> Result<RelayMap> {
    if let Some(url) = url {
        let url = url
            .parse::<RelayUrl>()
            .with_context(|| format!("the group's relay {url} is no URL"))?;
        public.extend(&RelayMap::from(url));
    }
    Ok(public)
}

impl Engine {
    /// Names `url` as a relay of every machine of the group, alongside
    /// iroh's public relays, or none for the public relays alone.
    ///
    /// # Errors
    ///
    /// Fails if `url` is no URL, this member does not belong to the group,
    /// or the statement cannot be stored.
    pub async fn set_relay(&self, url: Option<&str>) -> Result<()> {
        let url = url
            .map(|url| {
                url.parse::<RelayUrl>()
                    .with_context(|| format!("{url} is no relay URL"))
            })
            .transpose()?;
        let mut work = self.inner.work.lock().await;
        if work.join != JoinState::Joined {
            bail!("{} does not belong to the group now", self.inner.member);
        }
        let statement = RelayStatement {
            url: url.map(|url| url.to_string()),
        };
        let stamp = self.inner.clock.stamp();
        let body = serde_json::to_vec_pretty(&statement)?;
        self.inner
            .publish_statement(&mut work, stamp, relay_path(&stamp), body)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_groups_relay_joins_the_public_relays() {
        let public = || RelayMode::Default.relay_map();
        let both = with_group_relay(public(), Some("https://relay.example.org")).unwrap();
        assert_eq!(both.len(), public().len() + 1);
        assert!(both.contains(&"https://relay.example.org".parse().unwrap()));
        let urls: Vec<RelayUrl> = public().urls();
        assert!(urls.iter().all(|url| both.contains(url)));
        assert_eq!(with_group_relay(public(), None).unwrap(), public());
        assert!(with_group_relay(public(), Some("not a url")).is_err());
    }
}
