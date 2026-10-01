//! Excluding a member, themself included, by a rebinding statement that
//! binds the name to no key and that every machine folds into the member
//! list.

use anyhow::{Result, bail};
use pigeon_core::name::MemberName;
use pigeon_core::patch::Change;
use pigeon_core::statement::{RebindStatement, rebind_path};

use crate::engine::{Engine, Inner, JoinState, Wake, Work};

impl Inner {
    async fn exclude(&self, work: &mut Work, name: &MemberName) -> Result<()> {
        if work.join != JoinState::Joined {
            bail!("{} does not belong to the group now", self.config.member);
        }
        let statement = RebindStatement {
            name: name.clone(),
            key: None,
        };
        let stamp = self.clock.stamp();
        let path = rebind_path(&statement, &stamp);
        let content = self
            .add_content(work, serde_json::to_vec_pretty(&statement)?)
            .await?;
        let changes = vec![Change {
            path: path.clone(),
            content: Some(content),
            replaces: None,
        }];
        self.ledger.lock().check(
            &self.config.member,
            &self.config.cert.member,
            &changes,
            false,
        )?;
        self.publish_at(stamp, changes, None)?;
        work.join = self.join_state();
        self.renew_secret(work);
        let _ = self.wake.send(Wake::Keys(vec![path.key()]));
        Ok(())
    }
}

impl Engine {
    /// Binds `name` to no key: excluding a member, or leaving when `name`
    /// is this member. The name stays taken, and the group secret is
    /// renewed.
    ///
    /// # Errors
    ///
    /// Fails if this member does not belong to the group, `name` is no
    /// member, or the statement cannot be stored.
    pub async fn exclude(&self, name: &MemberName) -> Result<()> {
        let mut work = self.inner.work.lock().await;
        self.inner.exclude(&mut work, name).await
    }
}
