//! A group's configuration on one machine, `config.toml`: the member it
//! speaks for, its root, and its selection, retention and places. pigeon
//! writes the file whole, after a header saying how to edit it; the user
//! may edit it by hand, and the engine reads it when it starts, so that
//! `pigeon daemon reload` applies the edits. pigeon never overwrites edits
//! it has not read.

use std::ops::Deref;
use std::path::{Path, PathBuf};

use pigeon_core::name::MemberName;
use pigeon_core::places::{Place, Places};
use pigeon_core::retention::Retention;
use pigeon_core::selection::{Rule, Selection};
use serde::{Deserialize, Serialize};

use crate::error::{Result, StoreError};
use crate::group_dirs::{GroupDirs, read_if_present, write_private};

/// What `config.toml` starts with.
pub const HEADER: &str = "\
# pigeon's configuration of this group on this machine. Edit it, then apply
# it with `pigeon daemon reload`. Each selection line is `follow <pattern>`,
# `pin <RFC 3339 time> <pattern>` or `free <pattern>`, the last line
# matching a file deciding it; retention counts days, and its quota is a
# percentage of the disk. pigeon rewrites this file whole, without other
# comments, whenever it changes a setting.
";

/// A group's configuration on one machine.
#[derive(Clone, Debug)]
pub struct Config {
    /// The member this machine speaks for.
    pub member: MemberName,
    /// The folder holding the group's files.
    pub root: PathBuf,
    pub selection: Selection,
    pub retention: Retention,
    pub places: Places,
}

/// The configuration as the file spells it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spelled {
    member: MemberName,
    root: PathBuf,
    #[serde(default)]
    selection: Vec<String>,
    #[serde(default)]
    retention: Retention,
    #[serde(default)]
    places: Vec<Place>,
}

/// The selection of a member who chose none: their own folder.
fn own_folder(member: &MemberName) -> Selection {
    Selection::exactly([Rule::follow_own_folder(member)]).expect("an own folder is a pattern")
}

impl Config {
    /// The configuration of `member` joining with `root`, following their
    /// personal folder.
    #[must_use]
    pub fn new(member: MemberName, root: PathBuf) -> Self {
        Self {
            selection: own_folder(&member),
            member,
            root,
            retention: Retention::default(),
            places: Places::default(),
        }
    }

    /// Reads `text`, and says whether pigeon spells the configuration
    /// otherwise: it made an empty selection follow the member's personal
    /// folder.
    ///
    /// # Errors
    ///
    /// Returns what in the text is wrong.
    pub fn parse(text: &str) -> Result<(Self, bool), String> {
        let spelled: Spelled = toml::from_str(text).map_err(|error| error.to_string())?;
        if !spelled.root.is_absolute() {
            return Err(format!(
                "the root {} is not an absolute path",
                spelled.root.display()
            ));
        }
        let rules = spelled
            .selection
            .iter()
            .map(|line| {
                Rule::parse(line).map_err(|error| format!("the selection line {line:?}: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let empty = rules.is_empty();
        let selection = if empty {
            own_folder(&spelled.member)
        } else {
            Selection::exactly(rules).map_err(|error| error.to_string())?
        };
        let mut places = Places::default();
        for place in spelled.places {
            places
                .set(&spelled.root, place.folder, place.destination)
                .map_err(|error| format!("places: {error}"))?;
        }
        let config = Self {
            member: spelled.member,
            root: spelled.root,
            selection,
            retention: spelled.retention,
            places,
        };
        Ok((config, empty))
    }

    /// The text of the file: the header, then the configuration.
    ///
    /// # Errors
    ///
    /// Fails if a path is not valid Unicode, which the file cannot hold.
    pub fn render(&self) -> Result<String, String> {
        let spelled = Spelled {
            member: self.member.clone(),
            root: self.root.clone(),
            selection: self.selection.rules().map(Rule::to_string).collect(),
            retention: self.retention,
            places: self.places.iter().collect(),
        };
        let body = toml::to_string_pretty(&spelled).map_err(|error| error.to_string())?;
        Ok(format!("{HEADER}\n{body}"))
    }
}

/// The configuration in `text`, read from `path`.
fn parse_at(path: &Path, text: &str) -> Result<(Config, bool)> {
    Config::parse(text)
        .map_err(|reason| StoreError::Invalid(format!("{}: {reason}", path.display())))
}

/// The text of `config`, to write at `path`.
fn render_at(path: &Path, config: &Config) -> Result<String> {
    config
        .render()
        .map_err(|reason| StoreError::Invalid(format!("{}: {reason}", path.display())))
}

impl GroupDirs {
    /// Reads the group's configuration.
    ///
    /// # Errors
    ///
    /// Fails, naming the file and what in it is wrong, if it is missing or
    /// invalid.
    pub fn load_config(&self) -> Result<Config> {
        let path = self.config_path();
        let text = std::fs::read_to_string(&path).map_err(StoreError::io(&path))?;
        Ok(parse_at(&path, &text)?.0)
    }
}

/// `config.toml` and the configuration it holds, as pigeon last read or
/// wrote them.
#[derive(Debug)]
pub struct ConfigFile {
    path: PathBuf,
    text: String,
    config: Config,
}

impl ConfigFile {
    /// Reads the configuration of `group`, and writes it back if pigeon
    /// spells it otherwise.
    ///
    /// # Errors
    ///
    /// Fails, naming the file and what in it is wrong, if it is missing or
    /// invalid, or cannot be written back.
    pub fn open(group: &GroupDirs) -> Result<Self> {
        let path = group.config_path();
        let text = std::fs::read_to_string(&path).map_err(StoreError::io(&path))?;
        let (config, respelled) = parse_at(&path, &text)?;
        let mut file = Self { path, text, config };
        if respelled {
            file.save(file.config.clone())?;
        }
        Ok(file)
    }

    /// Writes `config` as the configuration of `group`, replacing any.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written.
    pub fn create(group: &GroupDirs, config: Config) -> Result<Self> {
        let path = group.config_path();
        let text = render_at(&path, &config)?;
        write_private(&path, text.as_bytes())?;
        Ok(Self { path, text, config })
    }

    /// The text pigeon last read or wrote.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Writes `config` and holds it, unless the file changed since pigeon
    /// last read or wrote it, whose edits writing would lose.
    ///
    /// # Errors
    ///
    /// Fails, naming the command that applies the edits, if the file
    /// changed, or if it cannot be written.
    pub fn save(&mut self, config: Config) -> Result<()> {
        if read_if_present(&self.path)?.as_deref() != Some(self.text.as_str()) {
            return Err(StoreError::Invalid(format!(
                "{} changed since pigeon read it: apply it with `pigeon daemon reload`, then try again",
                self.path.display()
            )));
        }
        let text = render_at(&self.path, &config)?;
        write_private(&self.path, text.as_bytes())?;
        self.text = text;
        self.config = config;
        Ok(())
    }
}

impl Deref for ConfigFile {
    type Target = Config;

    fn deref(&self) -> &Config {
        &self.config
    }
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
