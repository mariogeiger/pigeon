//! Running the disk work that blocks, walking folders and hashing files,
//! on the runtime's threads for blocking work, so that the sessions, the
//! fetches and the timers keep running meanwhile.

use std::path::PathBuf;

use anyhow::Result;
use pigeon_store::disk::Stat;
use pigeon_store::index::{Seen, observe};

/// What `work` returns, run on a thread for blocking work; a panic there
/// goes on here.
pub(crate) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => value,
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    }
}

/// What the file at `location` holds, as [`observe`] tells it from its
/// metadata `stat` and what was seen there before, on a thread for
/// blocking work.
pub(crate) async fn observed(
    location: PathBuf,
    stat: Stat,
    previous: Option<Seen>,
) -> Result<Seen> {
    Ok(blocking(move || observe(&location, stat, previous.as_ref())).await?)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn the_runtime_runs_on_while_work_blocks() {
        let (sender, receiver) = mpsc::channel();
        let answer = tokio::spawn(async move { sender.send(42).unwrap() });
        let heard = blocking(move || receiver.recv_timeout(Duration::from_secs(10))).await;
        assert_eq!(heard, Ok(42));
        answer.await.unwrap();
    }
}
