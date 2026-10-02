//! What each action does: the one place where an action's checked
//! arguments become a call on the daemon or on one group's engine, and its
//! result becomes JSON.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use pigeon_core::name::MemberName;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_sync::{Edit, Engine};
use serde::Serialize;
use serde_json::{Value, json};

use crate::args::Args;
use crate::catalog::{GROUP, Scope};
use crate::config_preview;
use crate::daemon::{Daemon, Stop};

fn to_json(value: impl Serialize) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

/// The group a call names, or the only one.
///
/// # Errors
///
/// Fails, naming the command to run, if the group is unknown or the call
/// names none on a machine with several.
pub fn choose<'a>(
    groups: &'a BTreeMap<String, Engine>,
    name: Option<&str>,
) -> Result<(&'a str, &'a Engine)> {
    if let Some(name) = name {
        return groups
            .get_key_value(name)
            .map(|(name, engine)| (name.as_str(), engine))
            .ok_or_else(|| anyhow!("no group {name} on this machine: see `pigeon group list`"));
    }
    let mut all = groups.iter();
    match (all.next(), all.next()) {
        (Some((name, engine)), None) => Ok((name.as_str(), engine)),
        (None, _) => {
            bail!("this machine is in no group: run `pigeon group create` or `pigeon group join`")
        }
        (Some(_), Some(_)) => bail!(
            "this machine is in several groups: pass --{} with one of {}",
            GROUP.name,
            groups.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Carries out the call `args` and returns its result.
///
/// # Errors
///
/// Fails with a message for the user if the action cannot be done.
pub async fn perform(daemon: &Daemon, args: &Args) -> Result<Value> {
    let action = args.action;
    let root = || args.text("root").map(PathBuf::from);
    match (action.noun, action.verb) {
        ("group", "list") => {
            let groups = daemon.groups().await;
            let mut list = Vec::new();
            for (name, engine) in groups.iter() {
                let status = engine.status().await;
                list.push(json!({
                    "name": name,
                    "member": status.member,
                    "join": status.join,
                    "peers": status.peers.len(),
                    "root": status.root,
                }));
            }
            Ok(Value::Array(list))
        }
        ("group", "create") => {
            let key = daemon
                .create(args.required("name")?, args.required("member")?, root())
                .await?;
            Ok(json!({ "key": key }))
        }
        ("group", "names") => to_json(daemon.hear(args.required("key")?).await?),
        ("group", "join") => {
            let key = daemon
                .join(args.required("key")?, args.required("member")?, root())
                .await?;
            Ok(json!({ "key": key }))
        }
        ("member", "claim") => {
            let group = {
                let groups = daemon.groups().await;
                choose(&groups, args.text(GROUP.name))?.0.to_owned()
            };
            daemon.claim(&group, args.required("member")?).await?;
            Ok(Value::Null)
        }
        ("daemon", "stop") => {
            daemon.stop(Stop::Quit);
            Ok(Value::Null)
        }
        ("daemon", "restart") => Ok(json!({ "restarts": daemon.restart()? })),
        ("daemon", "reload") => Ok(Value::Array(daemon.reload(args.flag("yes")).await?)),
        ("config", verb) => on_config(daemon, args, verb).await,
        _ if action.scope == Scope::Group => {
            let groups = daemon.groups().await;
            let (_, engine) = choose(&groups, args.text(GROUP.name))?;
            perform_in_group(engine, args).await
        }
        _ => bail!("{} is not implemented", action.command()),
    }
}

/// Shows, previews or replaces a group's `config.toml`.
async fn on_config(daemon: &Daemon, args: &Args, verb: &str) -> Result<Value> {
    let group = {
        let groups = daemon.groups().await;
        choose(&groups, args.text(GROUP.name))?.0.to_owned()
    };
    if verb == "set" {
        let text = args.required("text")?;
        daemon
            .apply_config(&group, text, args.text("version"), args.flag("yes"))
            .await?;
        return Ok(Value::Null);
    }
    let path = daemon.home().group(&group).config_path();
    let current =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let version = config_preview::version(&current);
    match verb {
        "show" => Ok(json!({ "path": path, "version": version, "text": current })),
        "preview" => {
            let groups = daemon.groups().await;
            let (_, engine) = choose(&groups, Some(&group))?;
            let text = args.text("text").unwrap_or(&current);
            let mut preview = config_preview::preview(engine, text).await?;
            preview["version"] = json!(version);
            Ok(preview)
        }
        _ => bail!("{} is not implemented", args.action.command()),
    }
}

/// The member a call names.
fn member(args: &Args) -> Result<MemberName> {
    MemberName::parse(args.required("member")?).context("the member name")
}

/// Carries out a call on one group.
async fn perform_in_group(engine: &Engine, args: &Args) -> Result<Value> {
    let action = args.action;
    let rule = |cutoff| -> Result<Rule> {
        Ok(Rule {
            pattern: args.required("pattern")?.to_owned(),
            cutoff,
        })
    };
    match (action.noun, action.verb) {
        ("group", "status") => to_json(engine.status().await),
        ("group", "key") => Ok(json!({ "key": engine.group_key() })),
        ("member", "list") => to_json(engine.members()),
        ("member", "exclude") => {
            engine.exclude(&member(args)?).await?;
            Ok(Value::Null)
        }
        ("group", "relay") => {
            engine.set_relay(args.text("url")).await?;
            Ok(Value::Null)
        }
        ("group", "leave") => {
            engine.exclude(&engine.status().await.member).await?;
            Ok(Value::Null)
        }
        ("file", verb) => on_files(engine, args, verb).await,
        ("selection", "times") => to_json(engine.pin_times(args.required("pattern")?)?),
        ("selection", verb @ ("places" | "place" | "unplace")) => place(engine, args, verb).await,
        ("selection", verb) => {
            let cutoff = match verb {
                "follow" => Cutoff::PlusInfinity,
                "unfollow" if args.flag("free") => Cutoff::MinusInfinity,
                "download" | "unfollow" => Cutoff::At(engine.now()),
                "pin" => Cutoff::At(args.time("time")?),
                _ => bail!("{} is not implemented", action.command()),
            };
            engine.set_rule(rule(cutoff)?).await?;
            Ok(Value::Null)
        }
        ("suggestion", "list") => to_json(engine.suggestions().await),
        ("suggestion", "validate") => {
            let to = args.text("to").map(|_| args.path("to")).transpose()?;
            engine
                .validate(&args.versions("suggestions")?, to.as_ref())
                .await?;
            Ok(Value::Null)
        }
        ("suggestion", "discard") => {
            engine.discard(&args.versions("suggestions")?).await?;
            Ok(Value::Null)
        }
        _ => bail!("{} is not implemented", action.command()),
    }
}

/// Lists, shows, publishes, writes, deletes, renames or restores files.
async fn on_files(engine: &Engine, args: &Args, verb: &str) -> Result<Value> {
    match verb {
        "list" => {
            let under = args.text("under").map(|_| args.path("under")).transpose()?;
            to_json(engine.list(under.as_ref()).await?)
        }
        "history" => to_json(engine.history(&args.path("path")?)),
        "pending" => {
            let under = args.text("under").map(|_| args.path("under")).transpose()?;
            to_json(engine.pending(under.as_ref()).await)
        }
        "publish" => {
            let path = args.text("path").map(|_| args.path("path")).transpose()?;
            engine.publish(path.as_ref()).await?;
            Ok(Value::Null)
        }
        "write" | "delete" | "rename" => {
            let edit = match verb {
                "write" => Edit::Write {
                    path: args.path("path")?,
                    bytes: args.bytes("content")?,
                },
                "delete" => Edit::Delete {
                    path: args.path("path")?,
                },
                _ => Edit::Rename {
                    from: args.path("from")?,
                    to: args.path("to")?,
                },
            };
            to_json(engine.edit(vec![edit]).await?)
        }
        "restore" => to_json(
            engine
                .restore(args.required("pattern")?, args.time("time")?)
                .await?,
        ),
        _ => bail!("{} is not implemented", args.action.command()),
    }
}

/// Lists, places or unplaces folders kept at other destinations.
async fn place(engine: &Engine, args: &Args, verb: &str) -> Result<Value> {
    match verb {
        "places" => to_json(engine.places().await),
        "place" => {
            let destination = args
                .text("destination")
                .ok_or_else(|| anyhow!("which destination?"))?;
            engine
                .place(args.path("folder")?, std::path::Path::new(destination))
                .await?;
            Ok(Value::Null)
        }
        _ => {
            engine.unplace(&args.path("folder")?).await?;
            Ok(Value::Null)
        }
    }
}
