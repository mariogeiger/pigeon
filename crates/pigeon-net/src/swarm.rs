//! Fetching one blob from several machines at once. Each machine reports,
//! live, the chunks it holds, so one still downloading the blob serves the
//! pieces it already has. Several lanes to every machine take pieces as the
//! board plans them, so the machines that deliver fastest, on the local
//! network, then direct, then relayed, deliver the most, and a duplicate
//! lane on a faster machine cuts short a slow last piece. A fetch fails once
//! no machine can deliver what is missing, or what this machine holds of
//! the blob stopped growing for a while.

use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail, ensure};
use iroh::endpoint::Connection;
use iroh_blobs::Hash;
use iroh_blobs::api::Store;
use iroh_blobs::api::proto::Bitfield;
use iroh_blobs::protocol::{GetRequest, ObserveRequest};
use iroh_blobs::util::connection_pool::ConnectionPool;
use n0_future::StreamExt;
use pigeon_core::clock::MachineId;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::board::{Board, Verdict};

/// How many pieces a fetch asks of one machine at once.
const LANES: usize = 8;

struct Fetch {
    hash: Hash,
    store: Store,
    board: Mutex<Board>,
    changes: watch::Sender<()>,
}

fn lock(board: &Mutex<Board>) -> std::sync::MutexGuard<'_, Board> {
    board.lock().expect("no panic holds a board")
}

impl Fetch {
    fn change<R>(&self, apply: impl FnOnce(&mut Board) -> R) -> R {
        let result = apply(&mut lock(&self.board));
        self.changes.send_replace(());
        result
    }

    async fn held(&self) -> Bitfield {
        self.store
            .observe(self.hash)
            .await
            .unwrap_or_else(|_| Bitfield::empty())
    }

    /// Fetches from `machine` until its lanes all fail, or its answer about
    /// what it holds fails before it said anything.
    async fn fetch_from(self: Arc<Self>, pool: ConnectionPool, machine: MachineId) {
        if let Ok(connection) = pool.get_or_connect(machine).await {
            let mut lanes = JoinSet::new();
            for _ in 0..LANES {
                lanes.spawn(self.clone().lane(machine, (*connection).clone()));
            }
            let mut ended = pin!(async move { while lanes.join_next().await.is_some() {} });
            tokio::select! {
                () = self.observe(machine, (*connection).clone()) => {
                    if lock(&self.board).knows(machine) {
                        ended.await;
                    }
                }
                () = &mut ended => {}
            }
        }
        self.change(|board| board.lose(machine));
    }

    async fn observe(&self, machine: MachineId, connection: Connection) {
        let remote = self.store.remote();
        let mut offers = pin!(remote.observe(connection, ObserveRequest::new(self.hash)));
        while let Some(Ok(bitfield)) = offers.next().await {
            self.change(|board| board.offer(machine, &bitfield));
        }
    }

    /// Fetches one piece after another from `machine`, until one fails.
    async fn lane(self: Arc<Self>, machine: MachineId, connection: Connection) {
        let mut changes = self.changes.subscribe();
        let mut finished = false;
        loop {
            changes.borrow_and_update();
            let taken = lock(&self.board).take(machine, finished);
            let Some((index, missing)) = taken else {
                if changes.changed().await.is_err() {
                    return;
                }
                continue;
            };
            let request = GetRequest::blob_ranges(self.hash, missing);
            let got = tokio::select! {
                got = self.store.remote().execute_get(connection.clone(), request).complete() => got.is_ok(),
                () = self.until_held(index) => true,
            };
            let held = self.held().await;
            if !self.change(|board| board.finish(index, &held)) || !got {
                return;
            }
            finished = true;
        }
    }

    async fn until_held(&self, index: u64) {
        let mut changes = self.changes.subscribe();
        loop {
            changes.borrow_and_update();
            if lock(&self.board).holds(index) {
                return;
            }
            if changes.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

/// Fetches `hash` into `store` from `machines` over `pool`, resuming what
/// the store holds; `rotation` is where this machine starts among pieces
/// equally rare.
///
/// # Errors
///
/// Fails if no machine can deliver what is missing, or the store came to
/// hold no more of the blob for `stall`, or the store fails.
pub(crate) async fn fetch(
    store: &Store,
    pool: &ConnectionPool,
    rotation: u64,
    hash: Hash,
    machines: Vec<MachineId>,
    stall: Duration,
) -> Result<()> {
    let held = store.observe(hash).await?;
    if held.is_complete() {
        return Ok(());
    }
    let fetch = Arc::new(Fetch {
        hash,
        store: store.clone(),
        board: Mutex::new(Board::new(rotation, &held, machines.iter().copied())),
        changes: watch::Sender::new(()),
    });
    let mut changes = fetch.changes.subscribe();
    let mut fetching = JoinSet::new();
    for machine in machines {
        fetching.spawn(fetch.clone().fetch_from(pool.clone(), machine));
    }
    let mut grown = held.ranges;
    let mut deadline = tokio::time::Instant::now() + stall;
    loop {
        changes.borrow_and_update();
        let verdict = lock(&fetch.board).verdict();
        match verdict {
            Verdict::Done => break,
            Verdict::Hopeless => bail!("no machine reached holds {hash}"),
            Verdict::Waiting => tokio::select! {
                result = changes.changed() => result?,
                () = tokio::time::sleep_until(deadline) => {
                    let held = fetch.held().await.ranges;
                    ensure!(held != grown, "no machine delivered more of {hash} for {stall:?}");
                    grown = held;
                    deadline = tokio::time::Instant::now() + stall;
                }
            },
        }
    }
    fetching.abort_all();
    ensure!(
        store.observe(hash).await?.is_complete(),
        "{hash} arrived incomplete"
    );
    Ok(())
}
