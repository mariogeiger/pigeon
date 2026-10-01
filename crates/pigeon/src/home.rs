//! The user's pigeon folders, as the XDG base directories lay them out on
//! Linux: the configuration, which the user may edit, one `config.toml` per
//! group; the data, each group's secrets, state database and blobs; and
//! the state, `daemon.toml` with the secret token that guards the API and
//! the address the daemon last listened on, the daemon's log, and the
//! relay's certificates. Elsewhere, and in `$PIGEON_HOME`, one folder holds
//! all three. What older pigeons kept elsewhere moves here, once.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use data_encoding::BASE32_NOPAD;
use pigeon_store::group_dirs::{GroupDirs, move_into_place, read_if_present, write_private};
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

/// `home`, to which the folder `old` of an older pigeon moves unless
/// `home` exists; `old` itself while it cannot move, as while the daemon
/// that runs from it holds its files open.
fn moved_home(home: PathBuf, old: Option<PathBuf>) -> PathBuf {
    match old {
        Some(old) if old != home && old.is_dir() && !home.exists() => {
            if std::fs::rename(&old, &home).is_ok() {
                home
            } else {
                old
            }
        }
        _ => home,
    }
}

/// Removes the file at `path`, if there is one.
fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", path.display()))
        }
        _ => Ok(()),
    }
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
    /// and state folders, where it moves what older pigeons kept elsewhere.
    ///
    /// # Errors
    ///
    /// Fails if the system names no data folder, or what older pigeons kept
    /// cannot move.
    pub fn locate() -> Result<Self> {
        if let Some(path) = std::env::var_os(HOME_VARIABLE) {
            return Ok(Self::new(path));
        }
        let local = dirs::data_local_dir()
            .ok_or_else(|| anyhow!("this system has no data folder: set {HOME_VARIABLE}"))?;
        let roaming = dirs::data_dir().map(|data| data.join("pigeon"));
        let data = moved_home(local.join("pigeon"), roaming);
        let pigeon = |folder: Option<PathBuf>| {
            folder.map_or_else(|| data.clone(), |folder| folder.join("pigeon"))
        };
        let home = Self {
            config: pigeon(dirs::config_local_dir()),
            state: pigeon(dirs::state_dir()),
            data: data.clone(),
        };
        home.relocate()?;
        Ok(home)
    }

    /// Moves to the state folder what pigeon kept in the data folder until
    /// 0.2.4.
    fn relocate(&self) -> Result<()> {
        for name in ["daemon.toml", "daemon.log", "relay"] {
            move_into_place(&self.data.join(name), &self.state.join(name))?;
        }
        Ok(())
    }

    /// The folders of the group `name`.
    #[must_use]
    pub fn group(&self, name: &str) -> GroupDirs {
        GroupDirs::new(
            self.config.join("groups").join(name),
            self.data.join("groups").join(name),
        )
    }

    /// The names of the groups' data folders, sorted.
    ///
    /// # Errors
    ///
    /// Fails if the groups' data folder exists but cannot be read.
    pub fn folder_names(&self) -> Result<Vec<String>> {
        folder_names(&self.data.join("groups"))
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

    /// What `daemon.toml` holds, made once from the files `token` and
    /// `address` of an older pigeon; none before the token is made.
    fn daemon_file(&self) -> Result<Option<DaemonFile>> {
        let path = self.daemon_path();
        if let Some(text) = read_if_present(&path)? {
            let file =
                toml::from_str(&text).with_context(|| format!("reading {}", path.display()))?;
            return Ok(Some(file));
        }
        let (token_path, address_path) = (self.data.join("token"), self.data.join("address"));
        let Some(token) = read_if_present(&token_path)? else {
            return Ok(None);
        };
        let address = read_if_present(&address_path)?.and_then(|text| text.trim().parse().ok());
        let file = DaemonFile {
            token: token.trim().to_owned(),
            address,
        };
        self.save_daemon_file(&file)?;
        remove_if_present(&token_path)?;
        remove_if_present(&address_path)?;
        Ok(Some(file))
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

    #[test]
    fn an_older_token_and_address_become_the_daemon_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = apart(dir.path());
        write_private(&home.data.join("token"), b"abc\n").unwrap();
        write_private(&home.data.join("address"), b"127.0.0.1:6767").unwrap();
        assert_eq!(home.address().unwrap().port(), 6767);
        assert_eq!(home.token().unwrap(), "abc");
        assert!(home.daemon_path().starts_with(&home.state));
        assert!(!home.data.join("token").exists() && !home.data.join("address").exists());
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
        assert_eq!(home.folder_names().unwrap(), ["cheapmo", "heard"]);
        assert_eq!(home.group_names().unwrap(), ["cheapmo"]);
    }

    #[test]
    fn what_the_data_folder_held_until_0_2_4_moves_to_the_state_folder() {
        let dir = tempfile::tempdir().unwrap();
        let home = apart(dir.path());
        write_private(&home.data.join("daemon.toml"), b"token = \"abc\"\n").unwrap();
        write_private(&home.data.join("relay").join("cert"), b"").unwrap();
        home.relocate().unwrap();
        assert_eq!(home.token().unwrap(), "abc");
        assert!(home.relay_path().join("cert").is_file());
        assert!(!home.data.join("daemon.toml").exists() && !home.data.join("relay").exists());
        assert!(home.log_path().starts_with(&home.state));
        home.relocate().unwrap();
    }

    #[test]
    fn an_older_folder_moves_once_unless_the_new_one_exists() {
        let dir = tempfile::tempdir().unwrap();
        let (old, new) = (dir.path().join("roaming"), dir.path().join("local"));
        assert_eq!(moved_home(new.clone(), Some(old.clone())), new);
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("daemon.toml"), "").unwrap();
        assert_eq!(moved_home(new.clone(), Some(old.clone())), new);
        assert!(new.join("daemon.toml").is_file() && !old.exists());
        std::fs::create_dir(&old).unwrap();
        assert_eq!(moved_home(new.clone(), Some(old.clone())), new);
        assert!(old.exists(), "a folder that exists stays");
        assert_eq!(moved_home(new.clone(), Some(new.clone())), new);
    }
}
