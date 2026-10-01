//! Running the daemon: start every group, listen on localhost, record the
//! address for the command line, print the link that opens the web UI, and
//! stop cleanly on Ctrl-C, when a service manager terminates it, or when
//! asked to, closing every connection, event streams included.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use pigeon_sync::Options;
use tokio::net::TcpListener;

use crate::api::{App, open_link, serve};
use crate::daemon::{Daemon, Stop};
use crate::home::Home;
use crate::program::Program;

/// The localhost port the daemon listens on unless told otherwise: fixed, so
/// that the web UI keeps one address a browser can bookmark.
pub const PORT: u16 = 6767;

/// Waits for Ctrl-C, or on Unix for the signal a service manager stops
/// its programs with.
async fn interrupted_or_terminated() -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminated =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            interrupted = tokio::signal::ctrl_c() => interrupted,
            _ = terminated.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

/// Runs the daemon of `home` on `port`, any free port for 0, until it
/// stops; returns the program to restart onto, if that is why it stopped.
///
/// # Errors
///
/// Fails if the groups folder cannot be read or the port not bound.
pub async fn run(home: Home, port: u16) -> Result<Option<Program>> {
    let token = home.token()?;
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .await
        .with_context(|| {
            format!("listening on localhost port {port}: if another program holds it, pass another with --port")
        })?;
    let address = listener.local_addr()?;
    let options = Options {
        announcement: crate::announcement(),
        ..Options::default()
    };
    let daemon = Daemon::start(home.clone(), options).await?;
    home.save_address(address)?;
    eprintln!(
        "pigeon {}: listening on {address}; open the web UI at {}",
        crate::VERSION,
        open_link(address, &token)
    );
    let mut stopping = daemon.stopping();
    let app = Arc::new(App { daemon, token });
    let interrupted = Arc::downgrade(&app);
    tokio::spawn(async move {
        if interrupted_or_terminated().await.is_ok()
            && let Some(app) = interrupted.upgrade()
        {
            app.daemon.stop(Stop::Quit);
        }
    });
    let stop = async move {
        let _ = stopping.wait_for(Option::is_some).await;
    };
    serve(app.clone(), listener, stop).await?;
    let restart = *app.daemon.stopping().borrow() == Some(Stop::Restart);
    let program = app.daemon.program().clone();
    if let Some(app) = Arc::into_inner(app) {
        app.daemon.shutdown().await;
    }
    Ok(restart.then_some(program))
}
