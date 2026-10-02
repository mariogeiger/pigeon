//! The command line's side of the API: one authenticated call to the
//! daemon this user runs, whose refusal may ask a question before the call
//! is made again with `yes`, or whether the daemon answers at all.

use std::fs::File;
use std::io::Read;
use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Map, Value};

use crate::catalog::{Action, Kind, Param, find};
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

/// How long a call waits to connect to a daemon.
const CONNECT_WITHIN: Duration = Duration::from_secs(5);

/// How long a call waits for the daemon's answer, which an action on a
/// large file or a whole group may take minutes to give, but not forever.
const ANSWER_WITHIN: Duration = Duration::from_mins(10);

/// How long a check that the daemon answers waits for it.
const CHECK_WITHIN: Duration = Duration::from_secs(5);

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
    call_within(address, token, noun, verb, args, ANSWER_WITHIN)
}

/// The argument of `action` that holds a file's content, which the request
/// carries as its body.
fn uploaded(action: &Action) -> Option<&'static Param> {
    action.params.iter().find(|param| param.kind == Kind::Bytes)
}

/// How a query holds the value of an argument, if it has one.
fn query_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// Sends the arguments but `param`, the name of a local file or `-` for
/// standard input, in the query, and the file as the body, read as it is
/// sent.
fn send_file(
    request: ureq::RequestBuilder<ureq::typestate::WithBody>,
    param: &Param,
    args: &Map<String, Value>,
) -> Result<Result<ureq::http::Response<ureq::Body>, ureq::Error>> {
    let source = args
        .get(param.name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("--{} is missing", param.name))?;
    let file: Box<dyn Read + Send> = if source == "-" {
        Box::new(std::io::stdin())
    } else {
        Box::new(File::open(source).with_context(|| format!("reading {source}"))?)
    };
    let query: Vec<(&str, String)> = args
        .iter()
        .filter(|(name, _)| name.as_str() != param.name)
        .filter_map(|(name, value)| Some((name.as_str(), query_text(value)?)))
        .collect();
    Ok(request
        .query_pairs(query.iter().map(|(name, text)| (*name, text.as_str())))
        .content_type("application/octet-stream")
        .send(ureq::SendBody::from_owned_reader(file)))
}

/// As [`call_at`], giving up on an answer after `within`.
fn call_within(
    address: SocketAddr,
    token: &str,
    noun: &str,
    verb: &str,
    args: &Map<String, Value>,
    within: Duration,
) -> Result<Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_WITHIN))
        .timeout_global(Some(within))
        .build()
        .into();
    let request = agent
        .post(format!("http://{address}/api/{noun}/{verb}"))
        .header(TOKEN_HEADER, format!("Bearer {token}"));
    let sent = match find(noun, verb).and_then(uploaded) {
        Some(param) => send_file(request, param, args)?,
        None => request.send_json(args),
    };
    let mut response =
        sent.map_err(|_| anyhow!("the daemon is not running: start it with `pigeon daemon`"))?;
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

/// Whether the daemon of `home` answers now, which it does without
/// asking any group's engine.
#[must_use]
pub fn answers(home: &Home) -> bool {
    let (Ok(address), Ok(token)) = (home.address(), home.token()) else {
        return false;
    };
    call_within(
        address,
        &token,
        "daemon",
        "program",
        &Map::new(),
        CHECK_WITHIN,
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_daemon_that_never_answers_is_given_up_on() {
        let silent = TcpListener::bind("127.0.0.1:0").unwrap();
        let started = Instant::now();
        let error = call_within(
            silent.local_addr().unwrap(),
            "token",
            "group",
            "list",
            &Map::new(),
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(error.to_string().contains("not running"), "{error}");
    }
}
