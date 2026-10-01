//! The `pigeon relay` command: serving the group's relay until interrupted,
//! after printing the URL that `pigeon group relay` names. With a contact
//! email it serves HTTPS on port 443 with a certificate from Let's
//! Encrypt, kept under pigeon's home; without one, plain HTTP.

use std::net::{Ipv4Addr, SocketAddr};

use anyhow::Result;
use pigeon_net::relay::{Certified, relay_url, serve_relay};

use crate::home::Home;

/// Serves a relay that other machines reach at `hostname`, on HTTP port
/// `port`, and HTTPS too when Let's Encrypt can warn `contact`.
///
/// # Errors
///
/// Fails if a port cannot be bound or `hostname` makes no URL.
pub async fn run(home: &Home, hostname: &str, contact: Option<&str>, port: u16) -> Result<()> {
    let any = |port| SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let certified = contact.map(|contact| Certified {
        hostname: hostname.to_owned(),
        contact: contact.to_owned(),
        https: any(443),
        cache: home.path().join("relay"),
    });
    let server = serve_relay(any(port), certified).await?;
    let url = relay_url(&server, hostname)?;
    println!("Serving the relay {url}: name it with `pigeon group relay --url {url}`");
    tokio::signal::ctrl_c().await?;
    server.shutdown().await?;
    Ok(())
}
