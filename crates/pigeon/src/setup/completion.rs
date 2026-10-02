//! Where `pigeon setup` turns on Tab completion: the shell the user logs
//! in with, the file it reads as it starts, and the line there that loads
//! pigeon's completion script while pigeon is installed.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// What every line that loads pigeon's completion holds, by which one
/// already there is found.
const MARK: &str = "pigeon completions";

/// The file a shell reads as it starts, and the line there that loads
/// pigeon's completion.
#[derive(Debug, PartialEq, Eq)]
pub struct Startup {
    pub shell: &'static str,
    pub file: PathBuf,
    pub line: &'static str,
}

impl Startup {
    /// The startup of the shell program `shell`, if pigeon completes in
    /// it, where `var` reads the environment.
    #[must_use]
    pub fn of(shell: &Path, var: impl Fn(&str) -> Option<PathBuf>) -> Option<Self> {
        let home = var("HOME")?;
        let (shell, file, line) = match shell.file_name()?.to_str()? {
            "zsh" => (
                "zsh",
                var("ZDOTDIR").unwrap_or(home).join(".zshrc"),
                r#"if command -v pigeon >/dev/null; then (( $+functions[compdef] )) || { autoload -Uz compinit && compinit; }; eval "$(pigeon completions zsh)"; fi"#,
            ),
            "bash" => (
                "bash",
                home.join(".bashrc"),
                r#"if command -v pigeon >/dev/null; then eval "$(pigeon completions bash)"; fi"#,
            ),
            "fish" => (
                "fish",
                var("XDG_CONFIG_HOME")
                    .unwrap_or_else(|| home.join(".config"))
                    .join("fish/config.fish"),
                "if command -q pigeon; pigeon completions fish | source; end",
            ),
            _ => return None,
        };
        Some(Self { shell, file, line })
    }

    /// The startup of the shell this user logs in with.
    #[must_use]
    pub fn current() -> Option<Self> {
        Self::of(&PathBuf::from(std::env::var_os("SHELL")?), |name| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
    }

    /// Whether the file already loads pigeon's completion.
    #[must_use]
    pub fn completes(&self) -> bool {
        std::fs::read_to_string(&self.file).is_ok_and(|text| text.contains(MARK))
    }

    /// Adds the line to the end of the file, creating it if need be.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be written.
    pub fn add(&self) -> Result<()> {
        if let Some(folder) = self.file.parent() {
            std::fs::create_dir_all(folder)
                .with_context(|| format!("creating {}", folder.display()))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)
            .with_context(|| format!("opening {}", self.file.display()))?;
        writeln!(
            file,
            "\n# Tab completion of pigeon's commands\n{}",
            self.line
        )
        .with_context(|| format!("writing {}", self.file.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<PathBuf> {
        let pairs: Vec<(String, PathBuf)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), PathBuf::from(value)))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn each_shell_reads_its_own_file() {
        let home = environment(&[("HOME", "/home/a")]);
        let file = |shell: &str| Startup::of(Path::new(shell), &home).map(|startup| startup.file);
        assert_eq!(file("/usr/bin/zsh"), Some("/home/a/.zshrc".into()));
        assert_eq!(file("/bin/bash"), Some("/home/a/.bashrc".into()));
        assert_eq!(
            file("/opt/homebrew/bin/fish"),
            Some("/home/a/.config/fish/config.fish".into())
        );
        assert_eq!(file("/bin/sh"), None);
        let moved = environment(&[("HOME", "/home/a"), ("ZDOTDIR", "/home/a/.zsh")]);
        assert_eq!(
            Startup::of(Path::new("zsh"), moved).map(|startup| startup.file),
            Some("/home/a/.zsh/.zshrc".into())
        );
    }

    #[test]
    fn the_line_is_added_once_to_a_file_that_may_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_str().unwrap().to_owned();
        let startup = Startup::of(Path::new("fish"), environment(&[("HOME", &home)])).unwrap();
        assert!(!startup.completes());
        startup.add().unwrap();
        assert!(startup.completes());
        let text = std::fs::read_to_string(&startup.file).unwrap();
        assert!(text.ends_with(&format!("{}\n", startup.line)), "{text}");
    }
}
