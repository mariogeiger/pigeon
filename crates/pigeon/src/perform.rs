//! What each action does: the handler the catalog gives each action, which
//! turns the call's checked arguments into a call on the daemon or on one
//! group's engine and its result into JSON, and the dispatch of a call to
//! its action's handler.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use anyhow::{Context, Result};
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_sync::{Edit, Engine};
use serde::Serialize;
use serde_json::{Value, json};

use crate::args::Args;
use crate::catalog::GROUP;
use crate::config_preview;
use crate::daemon::{Daemon, Stop};

/// The result of an action as JSON, once it is carried out.
pub type Reply<'a> = Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>>;

/// What carries an action out, given the call's checked arguments.
#[derive(Clone, Copy, Debug)]
pub enum Handler {
    /// Acts on the daemon.
    Daemon(for<'a> fn(&'a Daemon, &'a Args) -> Reply<'a>),
    /// Acts on the engine of the group the call names, or of the
    /// machine's single group.
    Engine(for<'a> fn(&'a Engine, &'a Args) -> Reply<'a>),
}

/// Carries out the call `args` with its action's handler.
///
/// # Errors
///
/// Fails with a message for the user if the action cannot be done.
pub async fn perform(daemon: &Daemon, args: &Args) -> Result<Value> {
    match args.action.handler {
        Handler::Daemon(handler) => handler(daemon, args).await,
        Handler::Engine(handler) => {
            let groups = daemon.groups().await;
            let (_, engine) = groups.choose(args.text(GROUP.name))?;
            handler(engine, args).await
        }
    }
}

fn to_json(value: impl Serialize) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

fn root(args: &Args) -> Option<PathBuf> {
    args.text("root").map(PathBuf::from)
}

/// The group the call names, or the machine's single group.
async fn chosen_group(daemon: &Daemon, args: &Args) -> Result<String> {
    let groups = daemon.groups().await;
    Ok(groups.choose(args.text(GROUP.name))?.0.to_owned())
}

pub(crate) fn list_groups<'a>(daemon: &'a Daemon, _: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let groups = daemon.groups().await;
        let mut list = BTreeMap::new();
        for (name, engine) in groups.running() {
            let status = engine.status().await;
            let item = json!({
                "name": name,
                "member": status.member,
                "join": status.join,
                "peers": status.peers.len(),
                "root": status.root,
            });
            list.insert(name, item);
        }
        for (name, why) in groups.failed() {
            let state = format!("does not start: {why}");
            list.insert(name, json!({ "name": name, "join": { "state": state } }));
        }
        Ok(Value::Array(list.into_values().collect()))
    })
}

pub(crate) fn create_group<'a>(daemon: &'a Daemon, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let key = daemon
            .create(args.required("name")?, args.required("member")?, root(args))
            .await?;
        Ok(json!({ "key": key }))
    })
}

pub(crate) fn hear_names<'a>(daemon: &'a Daemon, args: &'a Args) -> Reply<'a> {
    Box::pin(async move { to_json(daemon.hear(args.required("key")?).await?) })
}

pub(crate) fn join_group<'a>(daemon: &'a Daemon, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let key = daemon
            .join(args.required("key")?, args.required("member")?, root(args))
            .await?;
        Ok(json!({ "key": key }))
    })
}

pub(crate) fn leave_group<'a>(daemon: &'a Daemon, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let group = match args.text(GROUP.name) {
            Some(group) => group.to_owned(),
            None => daemon.groups().await.name(None)?.to_owned(),
        };
        daemon.leave(&group).await?;
        Ok(Value::Null)
    })
}

pub(crate) fn claim_name<'a>(daemon: &'a Daemon, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let group = chosen_group(daemon, args).await?;
        daemon.claim(&group, args.required("member")?).await?;
        Ok(Value::Null)
    })
}

/// The path and the text of the `config.toml` of `group`.
fn read_config(daemon: &Daemon, group: &str) -> Result<(PathBuf, String)> {
    let path = daemon.home().group(group).config_path();
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok((path, text))
}

pub(crate) fn show_config<'a>(daemon: &'a Daemon, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let (path, text) = read_config(daemon, &chosen_group(daemon, args).await?)?;
        let version = config_preview::version(&text);
        Ok(json!({ "path": path, "version": version, "text": text }))
    })
}

pub(crate) fn preview_config<'a>(daemon: &'a Daemon, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let groups = daemon.groups().await;
        let (group, engine) = groups.choose(args.text(GROUP.name))?;
        let (_, current) = read_config(daemon, group)?;
        let text = args.text("text").unwrap_or(&current);
        let mut preview = config_preview::preview(engine, text).await?;
        preview["version"] = json!(config_preview::version(&current));
        Ok(preview)
    })
}

pub(crate) fn set_config<'a>(daemon: &'a Daemon, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let group = chosen_group(daemon, args).await?;
        let text = args.required("text")?;
        daemon
            .apply_config(&group, text, args.text("version"), args.flag("yes"))
            .await?;
        Ok(Value::Null)
    })
}

pub(crate) fn reload<'a>(daemon: &'a Daemon, args: &'a Args) -> Reply<'a> {
    Box::pin(async move { Ok(Value::Array(daemon.reload(args.flag("yes")).await?)) })
}

pub(crate) fn stop<'a>(daemon: &'a Daemon, _: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        daemon.stop(Stop::Quit);
        Ok(Value::Null)
    })
}

pub(crate) fn restart<'a>(daemon: &'a Daemon, _: &'a Args) -> Reply<'a> {
    Box::pin(async move { Ok(json!({ "restarts": daemon.restart()? })) })
}

pub(crate) fn program<'a>(daemon: &'a Daemon, _: &'a Args) -> Reply<'a> {
    Box::pin(async move { Ok(json!({ "path": daemon.program().path })) })
}

pub(crate) fn show_status<'a>(engine: &'a Engine, _: &'a Args) -> Reply<'a> {
    Box::pin(async move { to_json(engine.status().await) })
}

pub(crate) fn show_key<'a>(engine: &'a Engine, _: &'a Args) -> Reply<'a> {
    Box::pin(async move { Ok(json!({ "key": engine.group_key() })) })
}

pub(crate) fn set_relay<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        engine.set_relay(args.text("url")).await?;
        Ok(Value::Null)
    })
}

pub(crate) fn list_members<'a>(engine: &'a Engine, _: &'a Args) -> Reply<'a> {
    Box::pin(async move { to_json(engine.members()) })
}

pub(crate) fn list_files<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let under = args.optional_path("under")?;
        to_json(engine.list(under.as_ref()).await?)
    })
}

pub(crate) fn list_history<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move { to_json(engine.history(&args.path("path")?)) })
}

pub(crate) fn list_pending<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let under = args.optional_path("under")?;
        to_json(engine.pending(under.as_ref()).await)
    })
}

pub(crate) fn publish<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        engine.publish(args.optional_path("path")?.as_ref()).await?;
        Ok(Value::Null)
    })
}

pub(crate) fn list_unportable<'a>(engine: &'a Engine, _: &'a Args) -> Reply<'a> {
    Box::pin(async move { to_json(engine.unportable().await) })
}

pub(crate) fn make_portable<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let renamed = engine.make_portable(args.required("path")?).await?;
        Ok(json!({ "renamed": renamed }))
    })
}

/// Makes the one edit `edit`, and gives what it made.
async fn apply_edit(engine: &Engine, edit: Edit) -> Result<Value> {
    to_json(engine.edit(vec![edit]).await?)
}

pub(crate) fn write_file<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let path = args.path("path")?;
        let bytes = args.bytes("content")?;
        apply_edit(engine, Edit::Write { path, bytes }).await
    })
}

pub(crate) fn delete_file<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let path = args.path("path")?;
        apply_edit(engine, Edit::Delete { path }).await
    })
}

pub(crate) fn rename_file<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let (from, to) = (args.path("from")?, args.path("to")?);
        apply_edit(engine, Edit::Rename { from, to }).await
    })
}

pub(crate) fn restore<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let time = args.time("time", || engine.now())?;
        to_json(engine.restore(args.required("pattern")?, time).await?)
    })
}

/// Holds the files of the call's pattern at `cutoff`.
async fn set_cutoff(engine: &Engine, args: &Args, cutoff: Cutoff) -> Result<Value> {
    let pattern = args.required("pattern")?.to_owned();
    engine.set_rule(Rule { pattern, cutoff }).await?;
    Ok(Value::Null)
}

pub(crate) fn follow<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(set_cutoff(engine, args, Cutoff::PlusInfinity))
}

pub(crate) fn pin<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let time = args.time("time", || engine.now())?;
        set_cutoff(engine, args, Cutoff::At(time)).await
    })
}

pub(crate) fn free<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(set_cutoff(engine, args, Cutoff::MinusInfinity))
}

pub(crate) fn list_pin_times<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move { to_json(engine.pin_times(args.required("pattern")?)?) })
}

pub(crate) fn list_places<'a>(engine: &'a Engine, _: &'a Args) -> Reply<'a> {
    Box::pin(async move { to_json(engine.places().await) })
}

pub(crate) fn place<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let destination = Path::new(args.required("destination")?);
        engine.place(args.path("folder")?, destination).await?;
        Ok(Value::Null)
    })
}

pub(crate) fn unplace<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        engine.unplace(&args.path("folder")?).await?;
        Ok(Value::Null)
    })
}

pub(crate) fn list_suggestions<'a>(engine: &'a Engine, _: &'a Args) -> Reply<'a> {
    Box::pin(async move { to_json(engine.suggestions().await) })
}

pub(crate) fn validate<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        let to = args.optional_path("to")?;
        engine
            .validate(&args.versions("suggestions")?, to.as_ref())
            .await?;
        Ok(Value::Null)
    })
}

pub(crate) fn discard<'a>(engine: &'a Engine, args: &'a Args) -> Reply<'a> {
    Box::pin(async move {
        engine.discard(&args.versions("suggestions")?).await?;
        Ok(Value::Null)
    })
}
