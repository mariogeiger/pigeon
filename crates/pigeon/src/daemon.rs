//! The groups running on this machine: one engine per group, started from
//! the data directories in the pigeon folder, and created when the user
//! founds or joins a group.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use pigeon_core::name::MemberName;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_store::config::GroupConfig;
use pigeon_store::group_key::GroupKey;
use pigeon_sync::{Engine, JoinState, Options};
use tokio::sync::{RwLock, RwLockReadGuard};

use crate::home::Home;

/// Every group on this machine.
pub struct Daemon {
    home: Home,
    options: Options,
    groups: RwLock<BTreeMap<String, Engine>>,
}

impl Daemon {
    /// Starts every group of `home`. A group that fails to start is
    /// reported and skipped, so that it never stops the others.
    ///
    /// # Errors
    ///
    /// Fails if the groups folder cannot be read.
    pub async fn start(home: Home, options: Options) -> Result<Self> {
        let mut groups = BTreeMap::new();
        for name in home.group_names()? {
            match Engine::start(&home.group(&name), options.clone()).await {
                Ok(engine) => {
                    groups.insert(name, engine);
                }
                Err(error) => eprintln!("pigeon: group {name} does not start: {error:#}"),
            }
        }
        Ok(Self {
            home,
            options,
            groups: RwLock::new(groups),
        })
    }

    #[must_use]
    pub fn home(&self) -> &Home {
        &self.home
    }

    /// The running groups, by name.
    pub async fn groups(&self) -> RwLockReadGuard<'_, BTreeMap<String, Engine>> {
        self.groups.read().await
    }

    /// Founds the group `name` with this machine as its first, and joins
    /// it as `member`; returns the group key to share.
    ///
    /// # Errors
    ///
    /// Fails if a name is invalid, the group exists here, or it does not
    /// start.
    pub async fn create(
        &self,
        name: &str,
        member: &str,
        password: &str,
        root: Option<PathBuf>,
    ) -> Result<String> {
        let name = MemberName::parse(name).context("the group name")?;
        let data = self.home.group(name.as_str());
        let machine = data.machine_key()?;
        let key = GroupKey::generate(name, vec![machine.public()]);
        self.add(key, member, password, root).await
    }

    /// Joins the group `key` admits as `member`; returns the group key.
    ///
    /// # Errors
    ///
    /// Fails if the key or name is invalid, the group exists here, or it
    /// does not start.
    pub async fn join(
        &self,
        key: &str,
        member: &str,
        password: &str,
        root: Option<PathBuf>,
    ) -> Result<String> {
        let key: GroupKey = key.parse().context("the group key")?;
        self.add(key, member, password, root).await
    }

    async fn add(
        &self,
        key: GroupKey,
        member: &str,
        password: &str,
        root: Option<PathBuf>,
    ) -> Result<String> {
        let member = MemberName::parse(member).context("the member name")?;
        let name = key.name.to_string();
        let mut groups = self.groups.write().await;
        if groups.contains_key(&name) || self.home.group_names()?.contains(&name) {
            bail!(
                "this machine is already in the group {name}: see `pigeon group status --group {name}`"
            );
        }
        let root = match root {
            Some(root) => root,
            None => dirs::home_dir()
                .ok_or_else(|| anyhow!("this system has no home folder: pass --root"))?
                .join(&name),
        };
        let data = self.home.group(&name);
        let machine = data.machine_key()?;
        data.save_config(&GroupConfig::join(key, member, password, root, &machine))?;
        match Engine::start(&data, self.options.clone()).await {
            Ok(engine) => {
                let key = engine.group_key();
                groups.insert(name, engine);
                Ok(key)
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(data.path());
                Err(error)
            }
        }
    }

    /// Makes this machine claim the name `member` in `group` with
    /// `password`, after losing its previous claim or to log in with a new
    /// password, and follows the name's personal folder.
    ///
    /// # Errors
    ///
    /// Fails if the group is unknown, the member has already joined, the
    /// name is invalid, or the group does not restart.
    pub async fn claim(&self, group: &str, member: &str, password: &str) -> Result<()> {
        let member = MemberName::parse(member).context("the member name")?;
        let mut groups = self.groups.write().await;
        let engine = groups
            .remove(group)
            .ok_or_else(|| anyhow!("no group {group} on this machine: see `pigeon group list`"))?;
        let status = engine.status().await;
        if status.join == JoinState::Joined {
            groups.insert(group.to_owned(), engine);
            bail!("{} has already joined {group}", status.member);
        }
        engine.shutdown().await?;
        let data = self.home.group(group);
        let config = data.load_config()?;
        let machine = data.machine_key()?;
        data.save_config(&GroupConfig::join(
            config.key,
            member.clone(),
            password,
            config.root,
            &machine,
        ))?;
        let engine = Engine::start(&data, self.options.clone()).await?;
        engine
            .set_rule(Rule {
                pattern: format!("{}/", member.folder()),
                cutoff: Cutoff::PlusInfinity,
            })
            .await?;
        groups.insert(group.to_owned(), engine);
        Ok(())
    }

    /// Gives this machine's member in `group` a new password, then logs
    /// this machine in with it; the member's other machines log in again.
    ///
    /// # Errors
    ///
    /// Fails if the group is unknown, the member does not belong to it, or
    /// the group does not restart.
    pub async fn set_password(&self, group: &str, password: &str) -> Result<()> {
        let member = {
            let groups = self.groups.read().await;
            let engine = groups.get(group).ok_or_else(|| {
                anyhow!("no group {group} on this machine: see `pigeon group list`")
            })?;
            let member = engine.status().await.member;
            engine.set_password(&member, password).await?;
            member
        };
        self.claim(group, member.as_str(), password).await
    }

    /// Stops every group.
    pub async fn shutdown(self) {
        for (name, engine) in self.groups.into_inner() {
            if let Err(error) = engine.shutdown().await {
                eprintln!("pigeon: group {name} did not stop cleanly: {error:#}");
            }
        }
    }
}
