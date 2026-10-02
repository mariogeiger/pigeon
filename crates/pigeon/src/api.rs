//! The localhost HTTP server: the JSON API at `/api/<noun>/<verb>` and the
//! web UI, both answering only requests addressed to localhost that carry
//! the user's secret token, so that no website can act in the user's name.

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;

use crate::args::Args;
use crate::catalog::{Kind, Param, find};
use crate::client::{CONFIRM, TOKEN_HEADER};
use crate::confirm::Confirm;
use crate::daemon::Daemon;
use crate::perform::perform;
use crate::upload::Upload;
use crate::web;

/// The cookie that carries the token in the web UI.
pub const COOKIE: &str = "pigeon_token";

/// How long the browser keeps the token cookie, in seconds: 400 days, the
/// longest browsers allow.
const COOKIE_SECONDS: u32 = 400 * 24 * 60 * 60;

/// What the server serves.
pub struct App {
    pub daemon: Daemon,
    pub token: String,
}

/// Whether two strings are equal, in a time that depends only on their
/// lengths.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |difference, (x, y)| difference | (x ^ y))
            == 0
}

/// Whether the request names localhost, which a page served from another
/// name, even one resolving to this machine, cannot.
fn addressed_to_localhost(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
    else {
        return false;
    };
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => host,
    };
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

/// The token a request presents in its authorization header or, unless it
/// is for the API, in its cookie: a browser attaches the cookie to whatever
/// a page asks of this address, so the API, which no page needs, does not
/// take it.
fn presented_token(headers: &HeaderMap, api: bool) -> Option<&str> {
    if let Some(bearer) = headers
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    {
        return Some(bearer.trim());
    }
    if api {
        return None;
    }
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

/// Whether a request that changes something was made by a page of this
/// server, or by no page: not by a page of another site, which the
/// browser says in `Sec-Fetch-Site` and `Origin`.
fn made_by_this_server(headers: &HeaderMap) -> bool {
    let fetched_from_elsewhere = headers
        .get("sec-fetch-site")
        .is_some_and(|site| !matches!(site.as_bytes(), b"same-origin" | b"none"));
    let origin_elsewhere = headers.get(header::ORIGIN).is_some_and(|origin| {
        let host = headers.get(header::HOST).map(HeaderValue::as_bytes);
        origin
            .as_bytes()
            .strip_prefix(b"http://")
            .is_none_or(|origin| Some(origin) != host)
    });
    !fetched_from_elsewhere && !origin_elsewhere
}

async fn guard(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    if !addressed_to_localhost(request.headers()) {
        return (StatusCode::FORBIDDEN, "pigeon answers only on localhost").into_response();
    }
    let changes = !matches!(*request.method(), Method::GET | Method::HEAD);
    if changes && !made_by_this_server(request.headers()) {
        return (
            StatusCode::FORBIDDEN,
            "pigeon refuses a page of another site",
        )
            .into_response();
    }
    let api = request.uri().path().starts_with("/api/");
    let open = request.uri().path() == "/open";
    let authorized =
        presented_token(request.headers(), api).is_some_and(|token| same(token, &app.token));
    if open || authorized {
        return next.run(request).await;
    }
    if api {
        let error = "missing or wrong token: the command line reads it from the pigeon folder";
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(json!({ "error": error })),
        )
            .into_response();
    }
    let page = "Open the web UI with the link that `pigeon ui` prints.";
    (StatusCode::UNAUTHORIZED, Html(page)).into_response()
}

#[derive(Deserialize)]
struct Open {
    token: String,
}

/// The link that opens the web UI served at `address`, whose `/open`
/// trades `token` for a cookie.
#[must_use]
pub fn open_link(address: SocketAddr, token: &str) -> String {
    format!("http://{address}/open?token={token}")
}

/// Trades the token in the link for a cookie, so that it leaves the
/// address bar.
async fn open(State(app): State<Arc<App>>, Query(query): Query<Open>) -> Response {
    if !same(&query.token, &app.token) {
        return (
            StatusCode::UNAUTHORIZED,
            "wrong token: run `pigeon ui` for the link",
        )
            .into_response();
    }
    let cookie = format!(
        "{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age={COOKIE_SECONDS}",
        app.token
    );
    ([(header::SET_COOKIE, cookie)], Redirect::to("/")).into_response()
}

/// Why a call is not done: its status and message, and, when making it
/// again with `yes` would do it, the question to ask first.
pub struct Refusal {
    pub status: StatusCode,
    pub message: String,
    pub confirm: Option<String>,
}

impl Refusal {
    fn plain(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            confirm: None,
        }
    }

    fn new(status: StatusCode, error: &anyhow::Error) -> Self {
        Self {
            status,
            message: format!("{error:#}"),
            confirm: error
                .downcast_ref::<Confirm>()
                .map(|confirm| confirm.question.clone()),
        }
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": self.message });
        if let Some(question) = self.confirm {
            body[CONFIRM] = json!(question);
        }
        (self.status, axum::Json(body)).into_response()
    }
}

/// Checks and carries out one call: shared by the API and the web UI's
/// forms.
///
/// # Errors
///
/// Returns why a call that cannot be done is not.
pub async fn call(
    app: &App,
    noun: &str,
    verb: &str,
    values: Map<String, Value>,
) -> Result<Value, Refusal> {
    let action = find(noun, verb).ok_or_else(|| Refusal {
        status: StatusCode::NOT_FOUND,
        message: format!("pigeon has no action {noun} {verb}: see `pigeon --help`"),
        confirm: None,
    })?;
    let args =
        Args::new(action, values).map_err(|error| Refusal::new(StatusCode::BAD_REQUEST, &error))?;
    perform(&app.daemon, &args)
        .await
        .map_err(|error| Refusal::new(StatusCode::BAD_REQUEST, &error))
}

/// The most a JSON body or a text field of a form may hold: a file's
/// content is in neither.
pub(crate) const TEXT_LIMIT: usize = 64 << 20;

/// What a request brings to `action`: its arguments and, when it takes a
/// file's content, the file that holds it, as it is received.
async fn arrive(
    app: &App,
    param: Option<&Param>,
    query: HashMap<String, String>,
    body: Body,
) -> Result<(Map<String, Value>, Option<Upload>), Refusal> {
    let Some(param) = param else {
        let bytes = axum::body::to_bytes(body, TEXT_LIMIT)
            .await
            .map_err(|_| Refusal::plain(StatusCode::PAYLOAD_TOO_LARGE, "the body is too large"))?;
        if bytes.is_empty() {
            return Ok((Map::new(), None));
        }
        return match serde_json::from_slice(&bytes) {
            Ok(Value::Object(values)) => Ok((values, None)),
            _ => Err(Refusal::plain(
                StatusCode::BAD_REQUEST,
                "the body is not a JSON object",
            )),
        };
    };
    let upload = Upload::receive(&app.daemon.home().uploads_path(), body.into_data_stream())
        .await
        .map_err(|error| Refusal::plain(StatusCode::BAD_REQUEST, format!("{error:#}")))?;
    let mut values: Map<String, Value> = query
        .into_iter()
        .map(|(name, text)| (name, Value::String(text)))
        .collect();
    let path = upload.path().display().to_string();
    values.insert(param.name.to_owned(), Value::String(path));
    Ok((values, Some(upload)))
}

async fn api(
    State(app): State<Arc<App>>,
    Path((noun, verb)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
    body: Body,
) -> Response {
    let param = find(&noun, &verb)
        .and_then(|action| action.params.iter().find(|param| param.kind == Kind::Bytes));
    let (values, _upload) = match arrive(&app, param, query, body).await {
        Ok(arrived) => arrived,
        Err(refusal) => return refusal.into_response(),
    };
    match call(&app, &noun, &verb, values).await {
        Ok(result) => axum::Json(result).into_response(),
        Err(refusal) => refusal.into_response(),
    }
}

/// The whole server, which takes a file of any size, received in chunks.
pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route(
            "/api/{noun}/{verb}",
            post(api).layer(DefaultBodyLimit::disable()),
        )
        .route("/open", get(open))
        .merge(web::routes())
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

/// Serves `app` on `listener` until `shutdown` completes.
///
/// # Errors
///
/// Fails if the listener fails.
pub async fn serve(
    app: Arc<App>,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    axum::serve(listener, router(app))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_localhost_names_pass() {
        let mut headers = HeaderMap::new();
        for (host, local) in [
            ("127.0.0.1:7000", true),
            ("localhost:7000", true),
            ("[::1]:7000", true),
            ("localhost", true),
            ("evil.example:7000", false),
            ("127.0.0.1.evil.example", false),
        ] {
            headers.insert(header::HOST, HeaderValue::from_static(host));
            assert_eq!(addressed_to_localhost(&headers), local, "{host}");
        }
    }

    #[test]
    fn only_a_page_of_this_server_or_no_page_changes_anything() {
        let made = |pairs: &[(&'static str, &'static str)]| {
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:7000"));
            for (name, value) in pairs {
                headers.insert(*name, HeaderValue::from_static(value));
            }
            made_by_this_server(&headers)
        };
        assert!(made(&[]));
        assert!(made(&[("origin", "http://127.0.0.1:7000")]));
        assert!(made(&[("sec-fetch-site", "same-origin")]));
        assert!(made(&[("sec-fetch-site", "none")]));
        assert!(!made(&[("origin", "http://evil.example")]));
        assert!(!made(&[("origin", "null")]));
        assert!(!made(&[("origin", "http://127.0.0.1:7001")]));
        assert!(!made(&[("sec-fetch-site", "cross-site")]));
        assert!(!made(&[("sec-fetch-site", "same-site")]));
    }

    #[test]
    fn the_token_comes_from_the_header_or_the_cookie() {
        let mut headers = HeaderMap::new();
        assert_eq!(presented_token(&headers, false), None);
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=b; pigeon_token=xyz"),
        );
        assert_eq!(presented_token(&headers, false), Some("xyz"));
        assert_eq!(presented_token(&headers, true), None);
        headers.insert(TOKEN_HEADER, HeaderValue::from_static("Bearer abc"));
        assert_eq!(presented_token(&headers, false), Some("abc"));
        assert_eq!(presented_token(&headers, true), Some("abc"));
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }
}
