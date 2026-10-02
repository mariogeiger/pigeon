//! The user's pigeon folders, as the XDG base directories lay them out on
//! Linux: the configuration, which the user may edit, one `config.toml` per
//! group; the data, each group's secrets, state database and blobs; and
//! the state, `daemon.toml` with the secret token that guards the API and
//! the address the daemon last listened on, the daemon's log, and the
//! relay's certificates. Elsewhere, and in `$PIGEON_HOME`, one folder holds
//! all three.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use data_encoding::BASE32_NOPAD;
use pigeon_store::group_dirs::{GroupDirs, read_if_present, write_private};
use serde::{Deserialize, Serialize};

/// The environment variable that names one folder for all of pigeon's.
pub const HOME_VARIABLE: &str = "PIGEON_HOME";

/// What `daemon.toml` starts with.
const DAEMON_HEADER: &str = "\
# pigeon's daemon: the token that guards its API, which lets whoever holds
# it act as you, and the address it last listened on. pigeon writes this
# file.
";

/// What `daemon.toml` holds.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DaemonFile {
    token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    address: Option<SocketAddr>,
}

/// The names of the folders in `folder`, sorted, none if it is missing.
fn folder_names(folder: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", folder.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", folder.display()))?;
        if entry.path().is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The user's pigeon folders.
#[derive(Clone, Debug)]
pub struct Home {
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
}

impl Home {
    /// The folders all in the one folder `path`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            config: path.clone(),
            data: path.clone(),
            state: path,
        }
    }

    /// `$PIGEON_HOME`, or `pigeon` in the user's local configuration, data
    /// and state folders.
    ///
    /// # Errors
    ///
    /// Fails if the system names no data folder.
    pub fn locate() -> Result<Self> {
        if let Some(path) = std::env::var_os(HOME_VARIABLE) {
            return Ok(Self::new(path));
        }
        let data = dirs::data_local_dir()
            .ok_or_else(|| anyhow!("this system has no data folder: set {HOME_VARIABLE}"))?
            .join("pigeon");
        let pigeon = |folder: Option<PathBuf>| {
            folder.map_or_else(|| data.clone(), |folder| folder.join("pigeon"))
        };
        Ok(Self {
            config: pigeon(dirs::config_local_dir()),
            state: pigeon(dirs::state_dir()),
            data: data.clone(),
        })
    }

    /// The folders of the group `name`.
    #[must_use]
    pub fn group(&self, name: &str) -> GroupDirs {
        GroupDirs::new(
            self.config.join("groups").join(name),
            self.data.join("groups").join(name),
        )
    }

    /// The names of the groups, whose folders hold a configuration, sorted.
    ///
    /// # Errors
    ///
    /// Fails if the groups' configuration folder exists but cannot be read.
    pub fn group_names(&self) -> Result<Vec<String>> {
        let mut names = folder_names(&self.config.join("groups"))?;
        names.retain(|name| self.group(name).config_path().is_file());
        Ok(names)
    }

    /// Where a daemon started apart from any terminal writes its output.
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.state.join("daemon.log")
    }

    /// Where the relay keeps its certificates.
    #[must_use]
    pub fn relay_path(&self) -> PathBuf {
        self.state.join("relay")
    }

    fn daemon_path(&self) -> PathBuf {
        self.state.join("daemon.toml")
    }

    fn save_daemon_file(&self, file: &DaemonFile) -> Result<()> {
        let body = toml::to_string_pretty(file).context("writing the daemon's file")?;
        write_private(
            &self.daemon_path(),
            format!("{DAEMON_HEADER}\n{body}").as_bytes(),
        )?;
        Ok(())
    }

    /// What `daemon.toml` holds, none before the token is made.
    fn daemon_file(&self) -> Result<Option<DaemonFile>> {
        let path = self.daemon_path();
        read_if_present(&path)?
            .map(|text| {
                toml::from_str(&text).with_context(|| format!("reading {}", path.display()))
            })
            .transpose()
    }

    /// The secret token the API demands, created on first use.
    ///
    /// # Errors
    ///
    /// Fails if `daemon.toml` cannot be read or written.
    ///
    /// # Panics
    ///
    /// Panics if the operating system has no source of randomness.
    pub fn token(&self) -> Result<String> {
        if let Some(file) = self.daemon_file()? {
            return Ok(file.token);
        }
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes).expect("the system provides randomness");
        let token = BASE32_NOPAD.encode(&bytes).to_ascii_lowercase();
        self.save_daemon_file(&DaemonFile {
            token: token.clone(),
            address: None,
        })?;
        Ok(token)
    }

    /// Records where the daemon listens.
    ///
    /// # Errors
    ///
    /// Fails if `daemon.toml` cannot be read or written.
    pub fn save_address(&self, address: SocketAddr) -> Result<()> {
        let token = self.token()?;
        self.save_daemon_file(&DaemonFile {
            token,
            address: Some(address),
        })
    }

    /// Where the daemon last said it listens.
    ///
    /// # Errors
    ///
    /// Fails, naming the command to run, if no daemon ever started.
    pub fn address(&self) -> Result<SocketAddr> {
        self.daemon_file()?
            .and_then(|file| file.address)
            .ok_or_else(|| anyhow!("the daemon has never run: start it with `pigeon daemon`"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daemon_file_holds_the_token_made_once_and_the_address() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        let token = home.token().unwrap();
        assert_eq!(token.len(), 52);
        assert_eq!(home.token().unwrap(), token);
        assert!(
            home.address()
                .unwrap_err()
                .to_string()
                .contains("pigeon daemon")
        );
        let address: SocketAddr = "127.0.0.1:4242".parse().unwrap();
        home.save_address(address).unwrap();
        assert_eq!(home.address().unwrap(), address);
        assert_eq!(home.token().unwrap(), token);
        let text = std::fs::read_to_string(home.daemon_path()).unwrap();
        assert!(text.starts_with(DAEMON_HEADER), "{text}");
        assert!(text.contains(&format!("token = \"{token}\"")), "{text}");
        assert!(text.contains("address = \"127.0.0.1:4242\""), "{text}");
    }

    /// Folders apart, as on Linux.
    fn apart(dir: &Path) -> Home {
        Home {
            config: dir.join("config"),
            data: dir.join("data"),
            state: dir.join("state"),
        }
    }

    #[test]
    fn groups_need_a_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let home = apart(dir.path());
        assert!(home.group_names().unwrap().is_empty());
        for name in ["heard", "cheapmo"] {
            std::fs::create_dir_all(home.group(name).data()).unwrap();
        }
        let cheapmo = home.group("cheapmo");
        assert!(cheapmo.config_path().starts_with(dir.path().join("config")));
        assert!(cheapmo.secrets_path().starts_with(dir.path().join("data")));
        write_private(&cheapmo.config_path(), b"").unwrap();
        assert_eq!(home.group_names().unwrap(), ["cheapmo"]);
    }
}
