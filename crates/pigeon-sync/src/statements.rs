//! Statements, the files of the statements folder that machines write and
//! read rather than people: reading one's body from the blob store, and
//! publishing one as the next version of its path.

use anyhow::Result;
use pigeon_core::clock::Stamp;
use pigeon_core::patch::{Change, Content};
use pigeon_core::path::GroupPath;
use serde::de::DeserializeOwned;

use crate::engine::{Inner, Work};

impl Inner {
    /// Reads a statement's body from the blob store.
    pub(crate) async fn read_statement<T: DeserializeOwned>(&self, content: &Content) -> Result<T> {
        Ok(serde_json::from_slice(
            &self.blobs.read(&content.hash).await?,
        )?)
    }

    /// Publishes `body` at `path` with `stamp` as its patch's stamp,
    /// replacing the path's current version.
    pub(crate) async fn publish_statement(
        &self,
        work: &mut Work,
        stamp: Stamp,
        path: GroupPath,
        body: Vec<u8>,
    ) -> Result<()> {
        let content = self.add_content(work, body).await?;
        let replaces = self.ledger.lock().head(&path.key()).map(|head| head.stamp);
        let change = Change {
            path: path.clone(),
            content: Some(content),
            replaces,
            continues: None,
        };
        self.publish_at(stamp, vec![change]).await?;
        let _ = self.wake.send(vec![path.key()]);
        Ok(())
    }
}
