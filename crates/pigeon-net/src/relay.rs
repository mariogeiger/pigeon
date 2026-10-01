//! Serving the group's relay, which carries, as ciphertext it cannot read,
//! the traffic of machines that cannot connect directly. With a hostname it
//! serves HTTPS with a certificate from Let's Encrypt, kept in a cache
//! folder; without one, plain HTTP, for a local network or behind a proxy
//! that adds TLS.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh::RelayUrl;
use iroh_relay::server::{AcmeConfig, CertConfig, RelayConfig, Server, ServerConfig, TlsConfig};

/// A relay's certificate: the hostname Let's Encrypt certifies, the
/// contact email it warns, where HTTPS is served, and where the
/// certificate is kept.
#[derive(Clone, Debug)]
pub struct Certified {
    pub hostname: String,
    pub contact: String,
    pub https: SocketAddr,
    pub cache: PathBuf,
}

/// Starts a relay serving HTTP on `http`, the address Let's Encrypt's
/// challenges reach when `certified`, and HTTPS too then; it runs until
/// dropped.
///
/// # Errors
///
/// Fails if an address cannot be bound.
pub async fn serve_relay(http: SocketAddr, certified: Option<Certified>) -> Result<Server> {
    let mut relay = RelayConfig::new(http);
    if let Some(certified) = certified {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server_config_builder = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_no_client_auth();
        let acme_config = AcmeConfig::letsencrypt(true)
            .domains(vec![certified.hostname])
            .contact(vec![format!("mailto:{}", certified.contact)])
            .cache_path(certified.cache);
        relay.tls = Some(TlsConfig::new(
            certified.https,
            CertConfig::LetsEncrypt {
                acme_config,
                server_config_builder,
            },
        ));
    }
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    Server::spawn(config).await.context("starting the relay")
}

/// The URL machines reach `server` at through `hostname`: HTTPS when it
/// has a certificate, HTTP otherwise.
///
/// # Errors
///
/// Fails if `hostname` makes no URL.
pub fn relay_url(server: &Server, hostname: &str) -> Result<RelayUrl> {
    let url = match (server.https_addr(), server.http_addr()) {
        (Some(https), _) => format!("https://{hostname}:{}", https.port()),
        (None, Some(http)) => format!("http://{hostname}:{}", http.port()),
        (None, None) => anyhow::bail!("the relay serves nothing"),
    };
    url.parse()
        .with_context(|| format!("{hostname} makes no relay URL"))
}
