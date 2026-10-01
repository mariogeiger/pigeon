//! What each action does: the one place where an action's checked
//! arguments become a call on the daemon or on one group's engine, and its
//! result becomes JSON.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use pigeon_core::name::MemberName;
use pigeon_core::retention::Retention;
use pigeon_core::selection::{Cutoff, Rule};
use pigeon_core::statement::Decision;
use pigeon_sync::{Edit, Engine};
use serde::Serialize;
use serde_json::{Value, json};

use crate::args::Args;
use crate::catalog::{GROUP, Scope};
use crate::daemon::Daemon;
use crate::draft;

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
        ("daemon", "restart") => Ok(json!({ "restarts": daemon.restart()? })),
        _ if action.scope == Scope::Group => {
            let groups = daemon.groups().await;
            let (_, engine) = choose(&groups, args.text(GROUP.name))?;
            perform_in_group(engine, args).await
        }
        _ => bail!("{} is not implemented", action.command()),
    }
}

const DAY: u64 = 86_400;

/// A retention as the `retention set` arguments read it.
fn retention_in_days(retention: &Retention) -> Value {
    json!({
        "every": retention.every / DAY,
        "daily": retention.daily / DAY,
        "weekly": retention.weekly / DAY,
        "deletion": retention.before_deletion / DAY,
        "quota": retention.quota_percent,
        "everything": if retention.everything { "on" } else { "off" },
    })
}

/// The member a call names.
fn member(args: &Args) -> Result<MemberName> {
    MemberName::parse(args.required("member")?).context("the member name")
}

/// Carries out a call on one group.
async fn perform_in_group(engine: &Engine, args: &Args) -> Result<Value> {
    let action = args.action;
    let message = args.text("message").unwrap_or_default();
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
        ("selection", "list") => Ok(Value::String(draft::format(&engine.selection().await))),
        ("selection", "preview") => {
            draft::preview(engine, args.text("rules").unwrap_or_default()).await
        }
        ("selection", "set") => {
            let rules = draft::rules(args.text("rules").unwrap_or_default(), engine.now())?;
            engine.set_selection(rules, args.text("version")).await?;
            Ok(Value::Null)
        }
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
        ("request", "list") => to_json(engine.requests().await),
        ("request", verb @ ("accept" | "refuse")) => {
            let decision = if verb == "accept" {
                Decision::Accept
            } else {
                Decision::Refuse
            };
            engine.decide(&args.path("request")?, decision).await?;
            Ok(Value::Null)
        }
        ("aside", "list") => to_json(engine.aside()?),
        ("aside", "discard") => {
            engine.discard_aside(args.number("id")?).await?;
            Ok(Value::Null)
        }
        ("aside", "restore") => {
            engine
                .restore_aside(args.number("id")?, &args.path("to")?)
                .await?;
            Ok(Value::Null)
        }
        ("aside", "request") => {
            let requests = engine
                .request_aside(args.number("id")?, args.mode(), message)
                .await?;
            Ok(json!({ "requests": requests }))
        }
        ("retention", "show") => to_json(retention_in_days(&engine.retention()?)),
        ("retention", "set") => set_retention(engine, args).await,
        _ => bail!("{} is not implemented", action.command()),
    }
}

/// Lists, shows, publishes, writes, deletes or renames files.
async fn on_files(engine: &Engine, args: &Args, verb: &str) -> Result<Value> {
    let message = args.text("message").unwrap_or_default();
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
            to_json(engine.edit(vec![edit], args.mode(), message).await?)
        }
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

/// Changes the retention by the arguments given, keeping the others.
async fn set_retention(engine: &Engine, args: &Args) -> Result<Value> {
    let mut retention = engine.retention()?;
    let days = |name: &str, current: u64| {
        args.optional_number(name)
            .map_or(current, |days| days.saturating_mul(DAY))
    };
    retention.every = days("every", retention.every);
    retention.daily = days("daily", retention.daily);
    retention.weekly = days("weekly", retention.weekly);
    retention.before_deletion = days("deletion", retention.before_deletion);
    if let Some(quota) = args.optional_number("quota") {
        retention.quota_percent = u8::try_from(quota)
            .ok()
            .filter(|percent| *percent <= 100)
            .ok_or_else(|| anyhow!("--quota is a percentage, from 0 to 100"))?;
    }
    if let Some(switch) = args.text("everything") {
        retention.everything = switch == "on";
    }
    engine.set_retention(&retention).await?;
    to_json(retention_in_days(&retention))
}
