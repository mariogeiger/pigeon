//! The web UI's routes: each page fetches its data through the catalog's
//! views, or from the engine where it compares contents, each form posts
//! to `/act/<noun>/<verb>`, which runs the action as the API does, the
//! configuration editor previews its text, and a group's event stream
//! tells its pages when to fetch themselves again and which program serves
//! them.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Form, Multipart, Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Json, Redirect, Response};
use axum::routing::{get, post};
use data_encoding::BASE64;
use futures_util::StreamExt;
use maud::{Markup, html};
use pigeon_core::patch::Content;
use pigeon_core::path::GroupPath;
use pigeon_sync::Engine;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::api::{App, Refusal, call};
use crate::catalog::{Kind, find};
use crate::config_preview;
use crate::confirm_page;
use crate::file_page;
use crate::files_page;
use crate::form::BACK;
use crate::overview_page::{self, Overview};
use crate::pages::{self, Bar, Side, Tab, diff, layout};

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
        .map_err(|refusal| Failure::new(refusal.status, refusal.message, "/"))
}

/// Gives `use_engine` the engine of `group`.
async fn with_engine<T>(
    app: &App,
    group: &str,
    use_engine: impl AsyncFnOnce(&Engine) -> T,
) -> Result<T, Failure> {
    let groups = app.daemon.groups().await;
    let (_, engine) = groups
        .choose(Some(group))
        .map_err(|error| Failure::new(StatusCode::NOT_FOUND, format!("{error:#}"), "/"))?;
    Ok(use_engine(engine).await)
}

fn html_page(markup: &Markup) -> Html<String> {
    Html(markup.clone().into_string())
}

async fn home(State(app): State<Arc<App>>) -> Page {
    let groups = view(&app, "group", "list", json!({})).await?;
    Ok(html_page(&pages::home(&groups)))
}

/// The bar of `group`'s page at `tab`, with the `suggestions` that wait
/// for the group.
fn bar_with<'a>(group: &'a str, tab: Option<Tab>, suggestions: &Value) -> Bar<'a> {
    Bar {
        group,
        tab,
        suggestions: suggestions.as_array().map_or(0, Vec::len),
    }
}

/// The bar of `group`'s page at `tab`.
async fn bar<'a>(app: &App, group: &'a str, tab: Option<Tab>) -> Result<Bar<'a>, Failure> {
    let suggestions = view(app, "suggestion", "list", json!({ "group": group })).await?;
    Ok(bar_with(group, tab, &suggestions))
}

async fn overview(State(app): State<Arc<App>>, Path(group): Path<String>) -> Page {
    let of_group = || json!({ "group": group });
    let status = view(&app, "group", "status", of_group()).await?;
    let members = view(&app, "member", "list", of_group()).await?;
    let key = view(&app, "group", "key", of_group()).await?;
    let config = view(&app, "config", "show", of_group()).await?;
    let places = view(&app, "selection", "places", of_group()).await?;
    let shown = Overview {
        status: &status,
        members: &members,
        key: key["key"].as_str().unwrap_or_default(),
        config: &config,
        places: &places,
    };
    let bar = bar(&app, &group, Some(Tab::Overview)).await?;
    Ok(html_page(&overview_page::overview(&bar, &shown)))
}

async fn files(
    State(app): State<Arc<App>>,
    Path(group): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Page {
    let under = query
        .get("under")
        .map_or("", |under| under.trim_matches('/'));
    let status = view(&app, "group", "status", json!({ "group": group })).await?;
    let list = view(&app, "file", "list", json!({ "group": group })).await?;
    let waiting = view(&app, "file", "pending", json!({ "group": group })).await?;
    let suggestions = view(&app, "suggestion", "list", json!({ "group": group })).await?;
    let unportable = view(&app, "file", "unportable", json!({ "group": group })).await?;
    let member = status["member"].as_str().unwrap_or_default();
    let bar = bar_with(&group, Some(Tab::Files), &suggestions);
    Ok(html_page(&files_page::files(
        &bar,
        member,
        under,
        &list,
        &waiting,
        &suggestions,
        &unportable,
    )))
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
    let waiting = view(
        &app,
        "file",
        "pending",
        json!({ "group": group, "under": path }),
    )
    .await?;
    let waiting = waiting
        .as_array()
        .and_then(|edits| {
            edits
                .iter()
                .find(|edit| edit["path"] == path && edit["here"] == true)
        })
        .cloned()
        .unwrap_or(Value::Null);
    let suggested = with_engine(&app, &group, async |engine| {
        suggestion_cards(engine, path).await
    })
    .await?;
    let bar = bar(&app, &group, None).await?;
    let shown = file_page::Shown {
        file: &current,
        history: &history,
        waiting: &waiting,
        suggested: &suggested,
    };
    Ok(html_page(&file_page::file(&bar, path, &shown)))
}

/// The side of a comparison that `content` is.
async fn side(engine: &Engine, content: Option<&Content>) -> Side {
    match content {
        None => Side::NoFile,
        Some(content) if content.size > pages::DIFF_LIMIT => Side::TooLarge,
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

/// The changes of `path` suggested, each with its suggestion and the
/// difference it makes to the current version, as far as this machine
/// holds the contents.
async fn suggestion_cards(engine: &Engine, path: &str) -> Vec<(Value, Value, Markup)> {
    let current = GroupPath::parse(path)
        .ok()
        .and_then(|path| version_content(engine, &path, None));
    let mut cards = Vec::new();
    for suggestion in engine.suggestions().await {
        for change in suggestion
            .changes
            .iter()
            .filter(|change| change.path.as_str() == path)
        {
            let diff = change_diff(engine, current.as_ref(), change.content.as_ref()).await;
            let change = serde_json::to_value(change).unwrap_or(Value::Null);
            cards.push((
                serde_json::to_value(&suggestion).unwrap_or(Value::Null),
                change,
                diff,
            ));
        }
    }
    cards
}

/// What applying the text in the form field `text` as the group's
/// configuration would change, as the pieces the editor shows.
async fn config_preview(
    State(app): State<Arc<App>>,
    Path(group): Path<String>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let text = form.get("text").cloned().unwrap_or_default();
    let args = json!({ "group": group, "text": &text });
    let Value::Object(args) = args else {
        unreachable!("the arguments are an object")
    };
    match call(&app, "config", "preview", args).await {
        Ok(preview) => Json(overview_page::preview_parts(&group, &preview)).into_response(),
        Err(Refusal {
            status, message, ..
        }) => {
            let pins = with_engine(&app, &group, async |engine| {
                Value::Array(config_preview::unfinished_pins(engine, &text))
            })
            .await
            .unwrap_or(Value::Null);
            let fix = overview_page::unfinished_pins(&pins).into_string();
            (status, Json(json!({ "error": message, "fix": fix }))).into_response()
        }
    }
}

async fn raw(
    State(app): State<Arc<App>>,
    Path(group): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let back = format!("/g/{group}");
    let Some(Ok(path)) = query.get("path").map(|path| GroupPath::parse(path)) else {
        return Failure::new(StatusCode::BAD_REQUEST, "Which file?", &back).into_response();
    };
    let time = query.get("time").and_then(|time| time.parse().ok());
    let download = with_engine(&app, &group, async |engine| {
        let content = version_content(engine, &path, time)?;
        let stream = engine.stream(&content).await.ok().flatten()?;
        Some((stream, content.size))
    })
    .await;
    match download {
        Err(failure) => failure.into_response(),
        Ok(None) => Failure::new(
            StatusCode::NOT_FOUND,
            "This version is not on this machine: download it first.",
            &back,
        )
        .into_response(),
        Ok(Some((stream, size))) => {
            let name = path.file_name().replace('"', "");
            let disposition = format!("attachment; filename=\"{name}\"");
            (
                [
                    (header::CONTENT_TYPE, "application/octet-stream".to_owned()),
                    (header::CONTENT_DISPOSITION, disposition),
                    (header::CONTENT_LENGTH, size.to_string()),
                ],
                Body::from_stream(chunks(stream)),
            )
                .into_response()
        }
    }
}

/// What `reader` holds, in chunks as it is read, so that a file of any size
/// is sent without being held in memory.
fn chunks(
    reader: impl AsyncRead + Unpin + Send + 'static,
) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> {
    futures_util::stream::unfold(reader, |mut reader| async {
        let mut chunk = vec![0; DOWNLOAD_CHUNK];
        match reader.read(&mut chunk).await {
            Ok(0) => None,
            Ok(read) => {
                chunk.truncate(read);
                Some((Ok(Bytes::from(chunk)), reader))
            }
            Err(error) => Some((Err(error), reader)),
        }
    })
}

/// The most a download reads at once.
const DOWNLOAD_CHUNK: usize = 1 << 16;

/// Sends the hash of the program the daemon runs, for pages to notice a
/// restart onto another, then an event each time what the engine of
/// `group` shows may have changed, for its pages to fetch themselves
/// again; ends once the daemon stops.
async fn events(State(app): State<Arc<App>>, Path(group): Path<String>) -> Response {
    let changes = match with_engine(&app, &group, async |engine| engine.changes()).await {
        Ok(changes) => changes,
        Err(failure) => return failure.into_response(),
    };
    let program = Event::default()
        .event("program")
        .data(app.daemon.program().hash.to_hex().as_str());
    let state = (changes, app.daemon.stopping());
    let changes = futures_util::stream::unfold(state, |(mut changes, mut stopping)| async move {
        tokio::select! {
            result = changes.changed() => result.ok()?,
            _ = stopping.wait_for(Option::is_some) => return None,
        }
        let event = Event::default().data(changes.borrow_and_update().to_string());
        Some((event, (changes, stopping)))
    });
    let events = futures_util::stream::once(std::future::ready(program))
        .chain(changes)
        .map(Ok::<_, Infallible>);
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// A script of the web UI.
fn script(source: &'static str) -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        source,
    )
        .into_response()
}

/// The script that keeps a group's pages live.
async fn live_script() -> Response {
    script(include_str!("live.js"))
}

/// The script of the Files page's tree.
async fn files_script() -> Response {
    script(include_str!("files.js"))
}

/// The script of the configuration editor.
async fn config_editor_script() -> Response {
    script(include_str!("config_editor.js"))
}

/// Whether `back` is a page of this server, never another site.
fn local_page(back: &str) -> bool {
    back.starts_with('/') && !back.starts_with("//") && !back.contains('\\')
}

/// Whether an action's `result` says nothing its page does not show: no
/// result, or only the paths it published or renamed, which the group's
/// pages show by themselves.
fn shown_by_its_page(result: &Value) -> bool {
    match result {
        Value::Null => true,
        Value::Object(fields) => fields
            .keys()
            .all(|name| name == "published" || name == "renamed"),
        _ => false,
    }
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
    match call(&app, &noun, &verb, values.clone()).await {
        Ok(result) if shown_by_its_page(&result) => Redirect::to(&back).into_response(),
        Ok(result) => {
            let body = html! {
                (pages::fields(&result))
                p { a href=(back) { "Back" } }
            };
            let title = format!("{noun} {verb}");
            Html(layout(&title, None, &body).into_string()).into_response()
        }
        Err(Refusal {
            confirm: Some(question),
            ..
        }) => {
            let page = confirm_page::confirmation(&noun, &verb, &values, &question, &back);
            Html(page.into_string()).into_response()
        }
        Err(refusal) => Failure::new(refusal.status, refusal.message, &back).into_response(),
    }
}

/// The web UI's routes.
pub fn routes() -> Router<Arc<App>> {
    Router::new()
        .route("/", get(home))
        .route("/g/{group}", get(files))
        .route("/g/{group}/overview", get(overview))
        .route("/g/{group}/file", get(file))
        .route("/g/{group}/config/preview", post(config_preview))
        .route("/g/{group}/raw", get(raw))
        .route("/g/{group}/events", get(events))
        .route("/live.js", get(live_script))
        .route("/files.js", get(files_script))
        .route("/config_editor.js", get(config_editor_script))
        .route("/act/{noun}/{verb}", post(act))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_return_to_their_page_unless_the_result_tells_more() {
        assert!(shown_by_its_page(&Value::Null));
        assert!(shown_by_its_page(&json!({"published": ["a.txt"]})));
        assert!(shown_by_its_page(&json!({"renamed": "a_.txt"})));
        assert!(!shown_by_its_page(&json!({"published": [], "key": "k"})));
        assert!(!shown_by_its_page(&json!({"key": "k"})));
        assert!(!shown_by_its_page(&json!([])));
    }

    #[test]
    fn forms_return_only_to_local_pages() {
        assert!(local_page("/g/cheapmo?under=docs"));
        assert!(!local_page("//evil.example/"));
        assert!(!local_page("https://evil.example/"));
        assert!(!local_page("/\\evil.example"));
    }
}
