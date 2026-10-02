//! The command line's side of the API: one authenticated call to the
//! daemon this user runs, whose refusal may ask a question before the call
//! is made again with `yes`, or whether the daemon answers at all.

use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value};

use crate::home::Home;

/// The header carrying the token.
pub const TOKEN_HEADER: &str = "authorization";

/// The field of a refusal that holds the question to ask before making
/// the call again with `yes`.
pub const CONFIRM: &str = "confirm";

/// A call the daemon refuses unless it is made again with `yes`, and the
/// question to ask first.
#[derive(Debug)]
pub struct Unconfirmed {
    pub message: String,
    pub question: String,
}

impl std::fmt::Display for Unconfirmed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Unconfirmed {}

/// Calls `noun verb` with `args` on the daemon listening at `address`.
///
/// # Errors
///
/// Fails, naming the command to run, if no daemon answers, or with the
/// daemon's message if the action fails, as [`Unconfirmed`] when making it
/// again with `yes` would do it.
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
    let Some(message) = body.get("error").and_then(Value::as_str) else {
        bail!("the daemon answered {status}");
    };
    match body.get(CONFIRM).and_then(Value::as_str) {
        Some(question) => Err(Unconfirmed {
            message: message.to_owned(),
            question: question.to_owned(),
        }
        .into()),
        None => bail!("{message}"),
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

/// Whether the daemon of `home` answers now.
#[must_use]
pub fn answers(home: &Home) -> bool {
    call(home, "group", "list", &Map::new()).is_ok()
}
