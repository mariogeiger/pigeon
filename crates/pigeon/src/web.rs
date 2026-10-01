//! The web UI's routes: each page fetches its data through the catalog's
//! views, or from the engine where it compares contents, and each form
//! posts to `/act/<noun>/<verb>`, which runs the action as the API does.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use data_encoding::BASE64;
use maud::{Markup, html};
use pigeon_core::patch::Content;
use pigeon_core::path::GroupPath;
use pigeon_sync::Engine;
use serde_json::{Map, Value, json};

use crate::api::{App, call};
use crate::catalog::{GROUP, Kind, find};
use crate::form::{BACK, Fill, form};
use crate::group_pages::{self, RequestCard};
use crate::pages::{self, Side, action, diff, layout};

/// Why a page cannot be shown, and where to go back to.
struct Failure {
    status: StatusCode,
    message: String,
    back: String,
}

impl Failure {
    fn new(status: StatusCode, message: impl Into<String>, back: &str) -> Self {
        Self {
            status,
            message: message.into(),
            back: back.to_owned(),
        }
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let body = html! {
            p class="error" { (self.message) }
            p { a href=(self.back) { "Back" } }
        };
        let page = layout("Something went wrong", None, &body).into_string();
        (self.status, Html(page)).into_response()
    }
}

type Page = Result<Html<String>, Failure>;

/// Runs a view for a page.
async fn view(app: &App, noun: &str, verb: &str, args: Value) -> Result<Value, Failure> {
    let Value::Object(args) = args else {
        unreachable!("views take objects")
    };
    call(app, noun, verb, args)
        .await
        .map_err(|(status, message)| Failure::new(status, message, "/"))
}

/// Gives `use_engine` the engine of `group`.
async fn with_engine<T>(
    app: &App,
    group: &str,
    use_engine: impl AsyncFnOnce(&Engine) -> T,
) -> Result<T, Failure> {
    let groups = app.daemon.groups().await;
    let engine = groups.get(group).ok_or_else(|| {
        Failure::new(
            StatusCode::NOT_FOUND,
            format!("No group {group} on this machine."),
            "/",
        )
    })?;
    Ok(use_engine(engine).await)
}

fn html_page(markup: &Markup) -> Html<String> {
    Html(markup.clone().into_string())
}

async fn home(State(app): State<Arc<App>>) -> Page {
    let groups = view(&app, "group", "list", json!({})).await?;
    Ok(html_page(&pages::home(&groups)))
}

async fn overview(State(app): State<Arc<App>>, Path(group): Path<String>) -> Page {
    let status = view(&app, "group", "status", json!({ "group": group })).await?;
    let key = view(&app, "group", "key", json!({ "group": group })).await?;
    let key = key["key"].as_str().unwrap_or_default();
    Ok(html_page(&pages::overview(&group, &status, key)))
}

async fn files(
    State(app): State<Arc<App>>,
    Path(group): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Page {
    let under = query
        .get("under")
        .map_or("", |under| under.trim_matches('/'));
    let list = view(
        &app,
        "file",
        "list",
        json!({ "group": group, "under": under }),
    )
    .await?;
    Ok(html_page(&group_pages::files(&group, under, &list)))
}

async fn file(
    State(app): State<Arc<App>>,
    Path(group): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Page {
    let path = query.get("path").map_or("", |path| path.trim_matches('/'));
    let list = view(
        &app,
        "file",
        "list",
        json!({ "group": group, "under": path }),
    )
    .await?;
    let current = list
        .as_array()
        .and_then(|files| files.iter().find(|file| file["path"] == path))
        .cloned()
        .unwrap_or(Value::Null);
    let history = view(
        &app,
        "file",
        "history",
        json!({ "group": group, "path": path }),
    )
    .await?;
    Ok(html_page(&group_pages::file(
        &group, path, &current, &history,
    )))
}

/// The side of a comparison that `content` is.
async fn side(engine: &Engine, content: Option<&Content>) -> Side {
    match content {
        None => Side::NoFile,
        Some(content) => match engine.read(content).await {
            Ok(Some(bytes)) => Side::Bytes(bytes),
            _ => Side::Unavailable,
        },
    }
}

/// The difference between the contents `old` and `new`.
async fn change_diff(engine: &Engine, old: Option<&Content>, new: Option<&Content>) -> Markup {
    diff(&side(engine, old).await, &side(engine, new).await)
}

/// The content of the version of `path` with stamp time `time`, or of its
/// current version.
fn version_content(engine: &Engine, path: &GroupPath, time: Option<u64>) -> Option<Content> {
    let history = engine.history(path);
    let version = match time {
        Some(time) => history.iter().find(|version| version.stamp.time == time),
        None => history.last(),
    };
    version.and_then(|version| version.content)
}

async fn requests(State(app): State<Arc<App>>, Path(group): Path<String>) -> Page {
    let cards = with_engine(&app, &group, async |engine| {
        let me = engine.status().await.member;
        let mut cards = Vec::new();
        for request in engine.requests().await {
            let mut changes = Vec::new();
            for change in &request.statement.changes {
                let old = change.replaces.and_then(|stamp| {
                    engine
                        .history(&change.path)
                        .into_iter()
                        .find(|version| version.stamp == stamp)
                        .and_then(|version| version.content)
                });
                let markup = change_diff(engine, old.as_ref(), change.content.as_ref()).await;
                changes.push((change.path.as_str().to_owned(), markup));
            }
            let decides = request.statement.owner == me
                && request.statement.mode == pigeon_core::statement::Mode::Propose
                && request.decision.is_none()
                && !request.applied;
            cards.push(RequestCard {
                request: serde_json::to_value(&request).unwrap_or(Value::Null),
                changes,
                decides,
            });
        }
        cards
    })
    .await?;
    Ok(html_page(&group_pages::requests(&group, &cards)))
}

async fn aside(State(app): State<Arc<App>>, Path(group): Path<String>) -> Page {
    let items = with_engine(&app, &group, async |engine| {
        let mut items = Vec::new();
        for item in engine.aside().unwrap_or_default() {
            let markup = match GroupPath::parse(&item.item.path) {
                Ok(path) => {
                    let current = version_content(engine, &path, None);
                    change_diff(engine, current.as_ref(), item.item.content.as_ref()).await
                }
                Err(error) => html! { p class="mark" { (error) } },
            };
            items.push((serde_json::to_value(&item).unwrap_or(Value::Null), markup));
        }
        items
    })
    .await?;
    Ok(html_page(&group_pages::aside(&group, &items)))
}

async fn members(State(app): State<Arc<App>>, Path(group): Path<String>) -> Page {
    let list = view(&app, "member", "list", json!({ "group": group })).await?;
    let back = format!("/g/{group}/members");
    let fill = Fill {
        group: Some(&group),
        ..Fill::default()
    };
    let forms = html! {
        @for (noun, verb) in [
            ("member", "password"),
            ("member", "reset"),
            ("member", "exclude"),
            ("group", "leave"),
        ] {
            (form(action(noun, verb), &back, fill))
        }
    };
    let page = pages::listing(&group, "Members", action("member", "list"), &list, &forms);
    Ok(html_page(&page))
}

async fn selection(State(app): State<Arc<App>>, Path(group): Path<String>) -> Page {
    let rules = view(&app, "selection", "list", json!({ "group": group })).await?;
    let back = format!("/g/{group}/selection");
    let fill = Fill {
        group: Some(&group),
        ..Fill::default()
    };
    let forms = html! {
        @for verb in ["follow", "download", "unfollow", "pin"] {
            (form(action("selection", verb), &back, fill))
        }
    };
    let page = pages::listing(
        &group,
        "Selection",
        action("selection", "list"),
        &rules,
        &forms,
    );
    Ok(html_page(&page))
}

async fn retention(State(app): State<Arc<App>>, Path(group): Path<String>) -> Page {
    let retention = view(&app, "retention", "show", json!({ "group": group })).await?;
    Ok(html_page(&pages::retention(&group, &retention)))
}

async fn raw(
    State(app): State<Arc<App>>,
    Path(group): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let back = format!("/g/{group}/files");
    let Some(Ok(path)) = query.get("path").map(|path| GroupPath::parse(path)) else {
        return Failure::new(StatusCode::BAD_REQUEST, "Which file?", &back).into_response();
    };
    let time = query.get("time").and_then(|time| time.parse().ok());
    let bytes = with_engine(&app, &group, async |engine| {
        let content = version_content(engine, &path, time)?;
        engine.read(&content).await.ok().flatten()
    })
    .await;
    match bytes {
        Err(failure) => failure.into_response(),
        Ok(None) => Failure::new(
            StatusCode::NOT_FOUND,
            "This version is not on this machine: download it first.",
            &back,
        )
        .into_response(),
        Ok(Some(bytes)) => {
            let name = path.file_name().replace('"', "");
            let disposition = format!("attachment; filename=\"{name}\"");
            (
                [
                    (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
                    (header::CONTENT_DISPOSITION, disposition),
                ],
                bytes,
            )
                .into_response()
        }
    }
}

/// Whether `back` is a page of this server, never another site.
fn local_page(back: &str) -> bool {
    back.starts_with('/') && !back.starts_with("//") && !back.contains('\\')
}

/// Runs a form's action, then returns to its page, or shows the result
/// when there is one to read.
async fn act(
    State(app): State<Arc<App>>,
    Path((noun, verb)): Path<(String, String)>,
    mut multipart: Multipart,
) -> Response {
    let mut back = "/".to_owned();
    let mut values = Map::new();
    let kinds = find(&noun, &verb)
        .map(|action| action.params)
        .unwrap_or_default();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return Failure::new(StatusCode::BAD_REQUEST, error.to_string(), &back)
                    .into_response();
            }
        };
        let name = field.name().unwrap_or_default().to_owned();
        let kind = kinds
            .iter()
            .find(|param| param.name == name)
            .map(|param| param.kind);
        let uploaded = field.file_name().is_some_and(|file| !file.is_empty());
        let Ok(bytes) = field.bytes().await else {
            return Failure::new(StatusCode::BAD_REQUEST, "The upload broke off.", &back)
                .into_response();
        };
        if name == BACK {
            back = String::from_utf8_lossy(&bytes).into_owned();
            continue;
        }
        let value = if kind == Some(Kind::Bytes) {
            if !uploaded {
                continue;
            }
            BASE64.encode(&bytes)
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        };
        values.insert(name, Value::String(value));
    }
    if !local_page(&back) {
        "/".clone_into(&mut back);
    }
    let group = values
        .get(GROUP.name)
        .and_then(Value::as_str)
        .map(str::to_owned);
    match call(&app, &noun, &verb, values).await {
        Ok(Value::Null) => Redirect::to(&back).into_response(),
        Ok(result) => {
            let body = html! {
                (pages::fields(&result))
                p { a href=(back) { "Back" } }
            };
            let title = format!("{noun} {verb}");
            Html(layout(&title, group.as_deref(), &body).into_string()).into_response()
        }
        Err((status, message)) => Failure::new(status, message, &back).into_response(),
    }
}

/// The web UI's routes.
pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/", get(home))
        .route("/g/{group}", get(overview))
        .route("/g/{group}/files", get(files))
        .route("/g/{group}/file", get(file))
        .route("/g/{group}/requests", get(requests))
        .route("/g/{group}/aside", get(aside))
        .route("/g/{group}/members", get(members))
        .route("/g/{group}/selection", get(selection))
        .route("/g/{group}/retention", get(retention))
        .route("/g/{group}/raw", get(raw))
        .route("/act/{noun}/{verb}", post(act))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_return_only_to_local_pages() {
        assert!(local_page("/g/cheapmo/files?under=docs"));
        assert!(!local_page("//evil.example/"));
        assert!(!local_page("https://evil.example/"));
        assert!(!local_page("/\\evil.example"));
    }
}
