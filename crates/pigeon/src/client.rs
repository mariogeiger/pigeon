//! The command line's side of the API: one authenticated call to the
//! daemon this user runs.

use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value};

use crate::home::Home;

/// The header carrying the token.
pub const TOKEN_HEADER: &str = "authorization";

/// Calls `noun verb` with `args` on the daemon listening at `address`.
///
/// # Errors
///
/// Fails, naming the command to run, if no daemon answers, or with the
/// daemon's message if the action fails.
pub fn call_at(
    address: SocketAddr,
    token: &str,
    noun: &str,
    verb: &str,
    args: &Map<String, Value>,
) -> Result<Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(format!("http://{address}/api/{noun}/{verb}"))
        .header(TOKEN_HEADER, format!("Bearer {token}"))
        .send_json(args)
        .map_err(|_| anyhow!("the daemon is not running: start it with `pigeon daemon`"))?;
    let status = response.status();
    let body: Value = response
        .body_mut()
        .with_config()
        .limit(u64::MAX)
        .read_json()
        .context("the daemon's answer is not JSON")?;
    if status.is_success() {
        return Ok(body);
    }
    match body.get("error").and_then(Value::as_str) {
        Some(error) => bail!("{error}"),
        None => bail!("the daemon answered {status}"),
    }
}

/// Calls `noun verb` with `args` on the daemon of `home`.
///
/// # Errors
///
/// As [`call_at`], and fails if the daemon never ran.
pub fn call(home: &Home, noun: &str, verb: &str, args: &Map<String, Value>) -> Result<Value> {
    call_at(home.address()?, &home.token()?, noun, verb, args)
}
