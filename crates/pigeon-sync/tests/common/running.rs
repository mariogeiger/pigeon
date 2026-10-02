//! The engine a test's machine runs, which tells its status when a failing
//! test drops it: what each machine reported, waits for and fetches tells
//! why a wait timed out or a check failed. The status is read on a thread
//! of its own, so that a panic reading it, as of a lock the engine's own
//! panic poisoned, cannot abort the test it explains.

use std::ops::Deref;

use pigeon_sync::Engine;

/// An engine until it shuts down, which tells its status when a panic
/// drops it.
pub struct Running(Option<Engine>);

impl Running {
    pub fn new(engine: Engine) -> Self {
        Self(Some(engine))
    }

    /// Shuts the engine down.
    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        self.0.take().expect("an engine runs").shutdown().await
    }
}

impl Deref for Running {
    type Target = Engine;

    fn deref(&self) -> &Engine {
        self.0.as_ref().expect("an engine runs")
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let Some(engine) = &self.0 else {
            return;
        };
        if !std::thread::panicking() {
            return;
        }
        let status = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .spawn_scoped(scope, || format!("{:?}", engine.status()))
                .ok()
                .and_then(|reading| reading.join().ok())
        });
        match status {
            Some(status) => eprintln!("a machine as the test failed: {status}"),
            None => eprintln!("a machine as the test failed, whose status cannot be read"),
        }
    }
}
