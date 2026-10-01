//! The user's pigeon folder: one data directory per group, the secret token
//! that guards the API, and the address the daemon listens on.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use data_encoding::BASE32_NOPAD;
use pigeon_store::config::{DataDir, write_private};

/// The environment variable that moves the pigeon folder.
pub const HOME_VARIABLE: &str = "PIGEON_HOME";

/// The user's pigeon folder.
#[derive(Clone, Debug)]
pub struct Home {
    path: PathBuf,
}

impl Home {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `$PIGEON_HOME`, or `pigeon` in the user's data folder.
    ///
    /// # Errors
    ///
    /// Fails if the system names no data folder.
    pub fn locate() -> Result<Self> {
        if let Some(path) = std::env::var_os(HOME_VARIABLE) {
            return Ok(Self::new(path));
        }
        let data = dirs::data_dir()
            .ok_or_else(|| anyhow!("this system has no data folder: set {HOME_VARIABLE}"))?;
        Ok(Self::new(data.join("pigeon")))
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The folder holding one data directory per group.
    #[must_use]
    pub fn groups_path(&self) -> PathBuf {
        self.path.join("groups")
    }

    /// The data directory of the group `name`.
    #[must_use]
    pub fn group(&self, name: &str) -> DataDir {
        DataDir::new(self.groups_path().join(name))
    }

    /// The names of the groups with a data directory, sorted.
    ///
    /// # Errors
    ///
    /// Fails if the groups folder exists but cannot be read.
    pub fn group_names(&self) -> Result<Vec<String>> {
        let path = self.groups_path();
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.with_context(|| format!("reading {}", path.display()))?;
            if entry.path().join("config.json").is_file()
                && let Some(name) = entry.file_name().to_str()
            {
                names.push(name.to_owned());
            }
        }
        names.sort();
        Ok(names)
    }

    fn token_path(&self) -> PathBuf {
        self.path.join("token")
    }

    fn address_path(&self) -> PathBuf {
        self.path.join("address")
    }

    /// The secret token the API demands, created on first use.
    ///
    /// # Errors
    ///
    /// Fails if the token cannot be read or created.
    ///
    /// # Panics
    ///
    /// Panics if the operating system has no source of randomness.
    pub fn token(&self) -> Result<String> {
        let path = self.token_path();
        match std::fs::read_to_string(&path) {
            Ok(token) => Ok(token.trim().to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut bytes = [0; 32];
                getrandom::fill(&mut bytes).expect("the system provides randomness");
                let token = BASE32_NOPAD.encode(&bytes).to_ascii_lowercase();
                write_private(&path, token.as_bytes())?;
                Ok(token)
            }
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Records where the daemon listens.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written.
    pub fn save_address(&self, address: SocketAddr) -> Result<()> {
        write_private(&self.address_path(), address.to_string().as_bytes())?;
        Ok(())
    }

    /// Where the daemon last said it listens.
    ///
    /// # Errors
    ///
    /// Fails, naming the command to run, if no daemon ever started.
    pub fn address(&self) -> Result<SocketAddr> {
        let path = self.address_path();
        let text = std::fs::read_to_string(&path)
            .map_err(|_| anyhow!("the daemon has never run: start it with `pigeon daemon`"))?;
        text.trim()
            .parse()
            .with_context(|| format!("{} holds no address", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_created_once_and_groups_need_a_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::new(dir.path());
        let token = home.token().unwrap();
        assert_eq!(token.len(), 52);
        assert_eq!(home.token().unwrap(), token);
        std::fs::create_dir_all(home.groups_path().join("empty")).unwrap();
        std::fs::create_dir_all(home.groups_path().join("cheapmo")).unwrap();
        std::fs::write(home.group("cheapmo").config_path(), "{}").unwrap();
        assert_eq!(home.group_names().unwrap(), ["cheapmo"]);
        assert!(
            home.address()
                .unwrap_err()
                .to_string()
                .contains("pigeon daemon")
        );
        let address: SocketAddr = "127.0.0.1:4242".parse().unwrap();
        home.save_address(address).unwrap();
        assert_eq!(home.address().unwrap(), address);
    }
}
