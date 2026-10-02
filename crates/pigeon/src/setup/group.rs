//! The group step of `pigeon setup`: keep a group this machine is in, join
//! one with its key, hearing it first to offer its members' names and
//! refuse the names taken, or create one; the root folder is chosen before
//! the daemon joins or creates the group, since it creates the folder then.

use anyhow::{Result, anyhow};
use pigeon_core::name::MemberName;
use pigeon_store::group_key::GroupKey;
use serde_json::{Value, json};

use super::checklist::{Checklist, Mark};
use super::{GROUP, NAME, ROOT, ask, call, root};
use crate::home::Home;

/// How this machine came to be in the group.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// It already was.
    Kept,
    /// It joined, having heard the group or not.
    Joined {
        heard: bool,
    },
    Created,
}

/// The group this machine is set up in.
pub struct Membership {
    pub group: String,
    pub member: String,
    pub origin: Origin,
}

/// Keeps, joins or creates a group, as one chooses.
///
/// # Errors
///
/// Fails if the terminal cannot be used or the daemon refuses.
pub fn step(home: &Home, list: &mut Checklist) -> Result<Membership> {
    let groups = call(home, "group", "list", json!({}))?;
    let groups = groups.as_array().cloned().unwrap_or_default();
    let mut choices: Vec<String> = groups
        .iter()
        .map(|group| {
            format!(
                "Continue with {} ({})",
                text(group, "name"),
                text(group, "member")
            )
        })
        .collect();
    choices.push("Join a group (key)".into());
    choices.push("Create a new group".into());
    let term = list.term().clone();
    let chosen = ask::choose(&term, "Group", &choices)?;
    if let Some(group) = groups.get(chosen) {
        let membership = Membership {
            group: text(group, "name"),
            member: text(group, "member"),
            origin: Origin::Kept,
        };
        list.set(GROUP, Mark::Done, &membership.group)?;
        list.set(NAME, Mark::Done, &membership.member)?;
        list.set(ROOT, Mark::Done, text(group, "root"))?;
        return Ok(membership);
    }
    if chosen == groups.len() {
        join(home, list)
    } else {
        create(home, list)
    }
}

/// The text field `name` of `value`.
fn text(value: &Value, name: &str) -> String {
    value[name].as_str().unwrap_or_default().to_owned()
}

/// The names in the list field `name` of `value`.
fn names(value: &Value, name: &str) -> Vec<String> {
    value[name]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect()
}

/// Joins a group with its key, under a member's name or a new one.
fn join(home: &Home, list: &mut Checklist) -> Result<Membership> {
    let term = list.term().clone();
    let key = ask::text(&term, "Group key", "", |text| {
        text.parse::<GroupKey>()
            .err()
            .map(|_| "not a group key".to_owned())
    })?;
    let group = key
        .parse::<GroupKey>()
        .map_err(|error| anyhow!("{error}"))?
        .name
        .as_str()
        .to_owned();
    list.set(
        GROUP,
        Mark::Running,
        format!("{group}: reaching the group…"),
    )?;
    let heard = call(home, "group", "names", json!({ "key": key }))?;
    let members = names(&heard, "members");
    let taken = names(&heard, "taken");
    let heard = heard["heard"].as_bool().unwrap_or(false);
    if !heard {
        list.note(format!(
            "⚠ No machine of {group} answered: the names taken are unknown."
        ));
    }
    list.set(GROUP, Mark::Running, &group)?;
    let mut choices = members.clone();
    choices.push("New name…".into());
    let chosen = if members.is_empty() {
        members.len()
    } else {
        ask::choose(
            &term,
            "Who are you? (a member adds a machine this way)",
            &choices,
        )?
    };
    let member = match members.get(chosen) {
        Some(member) => member.clone(),
        None => ask::new_name(&term, "New name", &taken)?,
    };
    list.set(NAME, Mark::Running, &member)?;
    let root = root::choose(&term, &group)?;
    list.set(ROOT, Mark::Done, root.display().to_string())?;
    list.set(GROUP, Mark::Running, format!("{group}: joining…"))?;
    match call(
        home,
        "group",
        "join",
        json!({ "key": key, "member": member, "root": root }),
    ) {
        Ok(_) => {
            list.set(NAME, Mark::Done, &member)?;
            list.set(GROUP, Mark::Done, &group)?;
            Ok(Membership {
                group,
                member,
                origin: Origin::Joined { heard },
            })
        }
        Err(error) => {
            list.set(GROUP, Mark::Failed, format!("{error:#}"))?;
            Err(error)
        }
    }
}

/// Creates a group, of which one becomes the first member.
fn create(home: &Home, list: &mut Checklist) -> Result<Membership> {
    let term = list.term().clone();
    let group = ask::text(&term, "Group name", "", |text| {
        MemberName::parse(text)
            .err()
            .map(|_| "1 to 32 characters among a-z and 0-9".to_owned())
    })?;
    list.set(GROUP, Mark::Running, &group)?;
    let member = ask::new_name(&term, "Your name in the group", &[])?;
    list.set(NAME, Mark::Running, &member)?;
    let root = root::choose(&term, &group)?;
    list.set(ROOT, Mark::Done, root.display().to_string())?;
    match call(
        home,
        "group",
        "create",
        json!({ "name": group, "member": member, "root": root }),
    ) {
        Ok(created) => {
            list.note(format!(
                "The group key, for the machines that join it (`pigeon group key` prints it again):\n  {}",
                text(&created, "key")
            ));
            list.set(NAME, Mark::Done, &member)?;
            list.set(GROUP, Mark::Done, &group)?;
            Ok(Membership {
                group,
                member,
                origin: Origin::Created,
            })
        }
        Err(error) => {
            list.set(GROUP, Mark::Failed, format!("{error:#}"))?;
            Err(error)
        }
    }
}
