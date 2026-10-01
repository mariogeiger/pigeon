//! Running the daemon: start every group, listen on localhost, record the
//! address for the command line, and stop cleanly on Ctrl-C.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use pigeon_sync::Options;
use tokio::net::TcpListener;

use crate::api::{App, serve};
use crate::daemon::Daemon;
use crate::home::Home;

/// Runs the daemon of `home` on `port`, any free port for 0.
///
/// # Errors
///
/// Fails if the groups folder cannot be read or the port not bound.
pub async fn run(home: Home, port: u16) -> Result<()> {
    let token = home.token()?;
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))
        .await
        .with_context(|| format!("listening on localhost port {port}"))?;
    let address = listener.local_addr()?;
    let daemon = Daemon::start(home.clone(), Options::default()).await?;
    home.save_address(address)?;
    eprintln!("pigeon: listening on {address}; open the web UI with the link `pigeon ui` prints");
    let app = Arc::new(App { daemon, token });
    let stop = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    serve(app.clone(), listener, stop).await?;
    if let Some(app) = Arc::into_inner(app) {
        app.daemon.shutdown().await;
    }
    Ok(())
}
