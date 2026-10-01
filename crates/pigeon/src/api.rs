//! The localhost HTTP server: the JSON API at `/api/<noun>/<verb>` and the
//! web UI, both answering only requests addressed to localhost that carry
//! the user's secret token, so that no website can act in the user's name.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;

use crate::args::Args;
use crate::catalog::find;
use crate::client::TOKEN_HEADER;
use crate::daemon::Daemon;
use crate::perform::perform;
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

/// The token a request presents, in its authorization header or cookie.
fn presented_token(headers: &HeaderMap) -> Option<&str> {
    if let Some(bearer) = headers
        .get(TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    {
        return Some(bearer.trim());
    }
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

async fn guard(State(app): State<Arc<App>>, request: Request, next: Next) -> Response {
    if !addressed_to_localhost(request.headers()) {
        return (StatusCode::FORBIDDEN, "pigeon answers only on localhost").into_response();
    }
    let open = request.uri().path() == "/open";
    let authorized =
        presented_token(request.headers()).is_some_and(|token| same(token, &app.token));
    if open || authorized {
        return next.run(request).await;
    }
    if request.uri().path().starts_with("/api/") {
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

fn api_error(status: StatusCode, error: &str) -> Response {
    (status, axum::Json(json!({ "error": error }))).into_response()
}

/// Checks and carries out one call: shared by the API and the web UI's
/// forms.
///
/// # Errors
///
/// Returns the status and message of a call that cannot be done.
pub async fn call(
    app: &App,
    noun: &str,
    verb: &str,
    values: Map<String, Value>,
) -> Result<Value, (StatusCode, String)> {
    let action = find(noun, verb).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("pigeon has no action {noun} {verb}: see `pigeon --help`"),
        )
    })?;
    let args = Args::new(action, values)
        .map_err(|error| (StatusCode::BAD_REQUEST, format!("{error:#}")))?;
    perform(&app.daemon, &args)
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, format!("{error:#}")))
}

async fn api(
    State(app): State<Arc<App>>,
    Path((noun, verb)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let values = if body.is_empty() {
        Map::new()
    } else {
        match serde_json::from_slice(&body) {
            Ok(Value::Object(values)) => values,
            _ => return api_error(StatusCode::BAD_REQUEST, "the body is not a JSON object"),
        }
    };
    match call(&app, &noun, &verb, values).await {
        Ok(result) => axum::Json(result).into_response(),
        Err((status, error)) => api_error(status, &error),
    }
}

/// The whole server.
pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/api/{noun}/{verb}", post(api))
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
    use axum::http::HeaderValue;

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
    fn the_token_comes_from_the_header_or_the_cookie() {
        let mut headers = HeaderMap::new();
        assert_eq!(presented_token(&headers), None);
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("a=b; pigeon_token=xyz"),
        );
        assert_eq!(presented_token(&headers), Some("xyz"));
        headers.insert(TOKEN_HEADER, HeaderValue::from_static("Bearer abc"));
        assert_eq!(presented_token(&headers), Some("abc"));
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }
}
