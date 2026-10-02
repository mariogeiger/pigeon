//! The arguments of one call, as the JSON object every interface sends:
//! checked against the action's definition once, then read by type.

use anyhow::{Context, Result, anyhow, bail};
use data_encoding::BASE64;
use pigeon_core::clock::parse_rfc3339;
use pigeon_core::patch::VersionRef;
use pigeon_core::path::GroupPath;
use serde_json::{Map, Value};

use crate::catalog::{Action, GROUP, Kind, Param, Scope};

/// Checked arguments of `action`.
#[derive(Clone, Debug)]
pub struct Args {
    pub action: &'static Action,
    values: Map<String, Value>,
}

/// `text`, the value of `name`, as a group path.
fn group_path(name: &str, text: &str) -> Result<GroupPath> {
    GroupPath::parse(text.trim_matches('/')).with_context(|| format!("--{name} {text:?}"))
}

/// `value` as `kind` holds it, or why it cannot be.
fn normalize(param: &Param, value: Value) -> Result<Value, String> {
    let name = param.name;
    match (param.kind, value) {
        (Kind::Flag, Value::Bool(on)) => Ok(Value::Bool(on)),
        (Kind::Flag, Value::String(text)) => match text.as_str() {
            "true" | "on" | "yes" => Ok(Value::Bool(true)),
            "false" | "off" | "no" => Ok(Value::Bool(false)),
            _ => Err(format!("--{name} is on or off, not {text:?}")),
        },
        (Kind::Flag, _) => Err(format!("--{name} has the wrong type")),
        (_, Value::String(text)) => Ok(Value::String(text)),
        (_, _) => Err(format!("--{name} is text")),
    }
}

impl Args {
    /// Checks `values` against `action`: every name is a parameter, every
    /// required one is there, and each holds its kind. Empty strings count
    /// as left out, as an empty form field does.
    ///
    /// # Errors
    ///
    /// Returns what is wrong, naming the command's help.
    pub fn new(action: &'static Action, values: Map<String, Value>) -> Result<Self> {
        let help = format!("see `{} --help`", action.command());
        let mut checked = Map::new();
        for (name, value) in values {
            let param = action
                .param(&name)
                .or((action.scope == Scope::Group && name == GROUP.name).then_some(&GROUP));
            let empty = value.as_str() == Some("")
                && param.is_none_or(|param| param.kind != Kind::Document);
            if value.is_null() || empty {
                continue;
            }
            let param =
                param.ok_or_else(|| anyhow!("{} takes no --{name}: {help}", action.command()))?;
            let value = normalize(param, value).map_err(|error| anyhow!("{error}: {help}"))?;
            checked.insert(name, value);
        }
        for param in action.params {
            if param.required && !checked.contains_key(param.name) {
                bail!("{} needs --{}: {help}", action.command(), param.name);
            }
        }
        Ok(Self {
            action,
            values: checked,
        })
    }

    /// The text of `name`, if given.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        self.values.get(name).and_then(Value::as_str)
    }

    /// The text of `name`, which the call must give.
    ///
    /// # Errors
    ///
    /// Fails if it is missing: a checked call gives every argument its
    /// action requires, but may leave out an optional one.
    pub fn required(&self, name: &str) -> Result<&str> {
        self.text(name)
            .ok_or_else(|| anyhow!("{} needs --{name}", self.action.command()))
    }

    /// The group path in `name`.
    ///
    /// # Errors
    ///
    /// Fails if it is missing or not a portable path.
    pub fn path(&self, name: &str) -> Result<GroupPath> {
        group_path(name, self.required(name)?)
    }

    /// The group path in `name`, if given.
    ///
    /// # Errors
    ///
    /// Fails if it is not a portable path.
    pub fn optional_path(&self, name: &str) -> Result<Option<GroupPath>> {
        self.text(name)
            .map(|text| group_path(name, text))
            .transpose()
    }

    /// The bytes in `name`, sent as base64.
    ///
    /// # Errors
    ///
    /// Fails if it is missing or not base64.
    pub fn bytes(&self, name: &str) -> Result<Vec<u8>> {
        BASE64
            .decode(self.required(name)?.as_bytes())
            .with_context(|| format!("--{name} is not base64"))
    }

    /// Whether the flag `name` is on.
    #[must_use]
    pub fn flag(&self, name: &str) -> bool {
        self.values.get(name).and_then(Value::as_bool) == Some(true)
    }

    /// The time in `name`, in NTP64: an RFC 3339 time, or `now` for the
    /// time `now` reads.
    ///
    /// # Errors
    ///
    /// Fails if it is missing, or neither `now` nor an RFC 3339 time.
    pub fn time(&self, name: &str, now: impl FnOnce() -> u64) -> Result<u64> {
        match self.required(name)? {
            "now" => Ok(now()),
            text => parse_rfc3339(text)
                .map_err(|error| anyhow!("--{name} is now or an RFC 3339 time: {error}")),
        }
    }

    /// The versions in `name`, as texts separated by spaces.
    ///
    /// # Errors
    ///
    /// Fails if it is missing or one of them names no version.
    pub fn versions(&self, name: &str) -> Result<Vec<VersionRef>> {
        self.required(name)?
            .split_whitespace()
            .map(|text| text.parse().map_err(|error| anyhow!("--{name}: {error}")))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::find;
    use serde_json::json;

    fn args(noun: &str, verb: &str, values: Value) -> Result<Args> {
        let Value::Object(values) = values else {
            unreachable!()
        };
        Args::new(find(noun, verb).unwrap(), values)
    }

    #[test]
    fn form_strings_become_typed_values() {
        let reload = args("daemon", "reload", json!({"yes": "on"})).unwrap();
        assert!(reload.flag("yes"));
        let restore = args(
            "file",
            "restore",
            json!({"pattern": "/docs/", "time": "1970-01-01T00:00:01Z", "group": "g"}),
        )
        .unwrap();
        assert_eq!(restore.time("time", || 7).unwrap(), 1 << 32);
        assert_eq!(restore.text("group"), Some("g"));
    }

    #[test]
    fn now_is_the_time_the_clock_reads() {
        let pin = args("selection", "pin", json!({"pattern": "/a/", "time": "now"})).unwrap();
        assert_eq!(pin.time("time", || 7).unwrap(), 7);
        let never = args(
            "selection",
            "pin",
            json!({"pattern": "/a/", "time": "never"}),
        )
        .unwrap();
        let error = never.time("time", || 7).unwrap_err().to_string();
        assert!(
            error.starts_with("--time is now or an RFC 3339 time: "),
            "{error}"
        );
    }

    #[test]
    fn missing_unknown_and_mistyped_arguments_name_the_help() {
        let missing = args("file", "write", json!({"path": "a", "content": ""})).unwrap_err();
        assert_eq!(
            missing.to_string(),
            "pigeon file write needs --content: see `pigeon file write --help`"
        );
        let unknown = args("group", "list", json!({"group": "g"})).unwrap_err();
        assert!(unknown.to_string().contains("takes no --group"));
        let mistyped = args("daemon", "reload", json!({"yes": "maybe"})).unwrap_err();
        assert!(mistyped.to_string().contains("on or off"));
    }

    #[test]
    fn paths_drop_surrounding_slashes_and_must_be_portable_when_given() {
        let delete = args("file", "delete", json!({"path": "/docs/a.txt"})).unwrap();
        assert_eq!(delete.path("path").unwrap().as_str(), "docs/a.txt");
        let bad = args("file", "delete", json!({"path": "a?b"})).unwrap();
        assert!(bad.path("path").is_err());
        let everything = args("file", "list", json!({})).unwrap();
        assert_eq!(everything.optional_path("under").unwrap(), None);
        let docs = args("file", "list", json!({"under": "/docs/"})).unwrap();
        assert_eq!(
            docs.optional_path("under").unwrap().unwrap().as_str(),
            "docs"
        );
        let bad = args("file", "list", json!({"under": "a?b"})).unwrap();
        assert!(bad.optional_path("under").is_err());
    }

    #[test]
    fn suggestions_are_versions_separated_by_spaces() {
        let machine = iroh::SecretKey::from_bytes(&[1; 32]).public();
        let shown: Vec<VersionRef> = (1..3)
            .map(|time| VersionRef {
                path: GroupPath::parse(&format!(".pigeon/suggestions/{time}.json")).unwrap(),
                stamp: pigeon_core::clock::Stamp { time, machine },
            })
            .collect();
        let ids = format!(" {}\n {} ", shown[0], shown[1]);
        let discard = args("suggestion", "discard", json!({ "suggestions": ids })).unwrap();
        assert_eq!(discard.versions("suggestions").unwrap(), shown);
        let wrong = args("suggestion", "discard", json!({"suggestions": "a.json"})).unwrap();
        assert!(wrong.versions("suggestions").is_err());
    }
}
