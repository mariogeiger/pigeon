//! The protocol that never changes: any machine that asks learns which
//! pigeon this one runs, its version, commit and sync protocol, as JSON,
//! so two machines that speak no sync protocol in common can still tell
//! which of them is behind. A machine that refuses even this protocol
//! predates it, and so is behind.

use std::time::Duration;

use anyhow::{Context, Result};
use iroh::Endpoint;
use iroh::endpoint::{
    ConnectError, ConnectingError, Connection, ConnectionError, TransportErrorCode,
};
use iroh::protocol::{AcceptError, ProtocolHandler};
use pigeon_core::clock::MachineId;
use serde::{Deserialize, Serialize};

use crate::wire::SYNC_ALPN;

/// The protocol's name on the wire, the same in every version of pigeon.
pub const HELLO_ALPN: &[u8] = b"pigeon/hello";

/// The largest announcement read.
const MAX_ANNOUNCEMENT: usize = 4096;

/// How long asking a machine, or answering one, may take.
const ASKING: Duration = Duration::from_secs(10);

/// Which pigeon a machine runs. Fields a later version adds are ignored
/// and fields it drops read as empty, so every version reads every other.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Announcement {
    pub version: String,
    pub commit: String,
    /// The name of the sync protocol on the wire.
    pub protocol: String,
}

impl Announcement {
    /// The announcement of pigeon `version` built from `commit`, speaking
    /// this build's sync protocol.
    #[must_use]
    pub fn speaking_ours(version: &str, commit: &str) -> Self {
        Self {
            version: version.to_owned(),
            commit: commit.to_owned(),
            protocol: String::from_utf8_lossy(SYNC_ALPN).into_owned(),
        }
    }
}

/// What asking a machine which pigeon it runs told.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "answer", content = "announcement")]
pub enum Heard {
    Announced(Announcement),
    /// It refused the hello protocol, which only versions before it do.
    PreHello,
    /// It gave no readable answer in time.
    Unheard,
}

/// How the pigeon a machine runs compares with this one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Standing {
    /// It speaks an earlier sync protocol.
    Older,
    /// It speaks a later sync protocol.
    Newer,
    /// It predates the hello protocol, so it is older.
    PreHello,
    /// Nothing heard tells.
    Unknown,
}

/// The number `n` of the sync protocol named `pigeon/sync/n`.
fn sync_number(protocol: &[u8]) -> Option<u64> {
    std::str::from_utf8(protocol)
        .ok()?
        .strip_prefix("pigeon/sync/")?
        .parse()
        .ok()
}

impl Standing {
    /// How the machine that answered `heard` compares with this one.
    #[must_use]
    pub fn of(heard: &Heard) -> Self {
        match heard {
            Heard::Announced(theirs) => {
                match (
                    sync_number(theirs.protocol.as_bytes()),
                    sync_number(SYNC_ALPN),
                ) {
                    (Some(theirs), Some(ours)) if theirs < ours => Self::Older,
                    (Some(theirs), Some(ours)) if theirs > ours => Self::Newer,
                    _ => Self::Unknown,
                }
            }
            Heard::PreHello => Self::PreHello,
            Heard::Unheard => Self::Unknown,
        }
    }
}

/// The TLS alert by which a machine refuses every protocol offered, as
/// RFC 7301 numbers it.
const NO_APPLICATION_PROTOCOL: u8 = 120;

/// Whether the machine dialed refused the connection for speaking none of
/// the protocols offered.
pub(crate) fn refuses_every_protocol(error: &ConnectError) -> bool {
    let (ConnectError::Connection { source: closed, .. }
    | ConnectError::Connecting {
        source: ConnectingError::ConnectionError { source: closed, .. },
        ..
    }) = error
    else {
        return false;
    };
    matches!(
        closed,
        ConnectionError::ConnectionClosed(close)
            if close.error_code == TransportErrorCode::crypto(NO_APPLICATION_PROTOCOL)
    )
}

/// Asks `machine` which pigeon it runs.
pub async fn ask(endpoint: &Endpoint, machine: MachineId) -> Heard {
    let asking = async {
        let connection = match endpoint.connect(machine, HELLO_ALPN).await {
            Ok(connection) => connection,
            Err(error) if refuses_every_protocol(&error) => return Heard::PreHello,
            Err(_) => return Heard::Unheard,
        };
        let read = async {
            let mut recv = connection.accept_uni().await.ok()?;
            recv.read_to_end(MAX_ANNOUNCEMENT).await.ok()
        };
        let bytes = read.await;
        connection.close(0u32.into(), b"heard");
        bytes
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .map_or(Heard::Unheard, Heard::Announced)
    };
    tokio::time::timeout(ASKING, asking)
        .await
        .unwrap_or(Heard::Unheard)
}

/// Answers every machine that asks with `announcement`.
#[derive(Clone, Debug)]
pub struct Announcing(pub Announcement);

impl Announcing {
    async fn answer(&self, connection: &Connection) -> Result<()> {
        let bytes = serde_json::to_vec(&self.0).context("encoding the announcement")?;
        let mut send = connection.open_uni().await?;
        send.write_all(&bytes).await?;
        send.finish()?;
        connection.closed().await;
        Ok(())
    }
}

impl ProtocolHandler for Announcing {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        match tokio::time::timeout(ASKING, self.answer(&connection)).await {
            Ok(result) => result.map_err(|error| AcceptError::from_boxed(error.into())),
            Err(_) => Ok(()),
        }
    }
}
