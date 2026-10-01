//! The arguments of one call, as the JSON object every interface sends:
//! checked against the action's definition once, then read by type.

use anyhow::{Context, Result, anyhow, bail};
use data_encoding::BASE64;
use pigeon_core::clock::parse_rfc3339;
use pigeon_core::path::GroupPath;
use pigeon_core::statement::Mode;
use serde_json::{Map, Value};

use crate::catalog::{Action, GROUP, Kind, Param, Scope};

/// Checked arguments of `action`.
#[derive(Clone, Debug)]
pub struct Args {
    pub action: &'static Action,
    values: Map<String, Value>,
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
        (Kind::Choice(choices), Value::String(text)) => {
            if choices.contains(&text.as_str()) {
                Ok(Value::String(text))
            } else {
                Err(format!("--{name} is one of {}", choices.join(", ")))
            }
        }
        (Kind::Flag | Kind::Choice(_), _) => Err(format!("--{name} has the wrong type")),
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

    /// The text of the required argument `name`.
    ///
    /// # Errors
    ///
    /// Fails if it is missing, which a checked call rules out.
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
        let text = self.required(name)?;
        GroupPath::parse(text.trim_matches('/')).with_context(|| format!("--{name} {text:?}"))
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

    /// The time in `name`, in NTP64.
    ///
    /// # Errors
    ///
    /// Fails if it is missing or not an RFC 3339 time.
    pub fn time(&self, name: &str) -> Result<u64> {
        parse_rfc3339(self.required(name)?).map_err(|error| anyhow!("--{name}: {error}"))
    }

    /// The request mode, proposing unless told to force.
    #[must_use]
    pub fn mode(&self) -> Mode {
        if self.text("mode") == Some("force") {
            Mode::Force
        } else {
            Mode::Propose
        }
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
        let unfollow = args(
            "selection",
            "unfollow",
            json!({"pattern": "/a/", "free": "on"}),
        )
        .unwrap();
        assert!(unfollow.flag("free"));
        let discard = args(
            "change",
            "discard",
            json!({"entry": ".pigeon/aside/a.json", "group": "g"}),
        )
        .unwrap();
        assert_eq!(
            discard.path("entry").unwrap().as_str(),
            ".pigeon/aside/a.json"
        );
        assert_eq!(discard.text("group"), Some("g"));
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
        let mistyped = args("file", "delete", json!({"path": "a", "mode": "maybe"})).unwrap_err();
        assert!(mistyped.to_string().contains("one of propose, force"));
    }

    #[test]
    fn paths_drop_surrounding_slashes_and_must_be_portable() {
        let delete = args("file", "delete", json!({"path": "/docs/a.txt"})).unwrap();
        assert_eq!(delete.path("path").unwrap().as_str(), "docs/a.txt");
        assert_eq!(delete.mode(), Mode::Propose);
        let bad = args("file", "delete", json!({"path": "a?b"})).unwrap();
        assert!(bad.path("path").is_err());
    }
}
