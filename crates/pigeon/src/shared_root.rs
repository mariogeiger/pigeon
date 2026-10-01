//! A group's root folder sits at the same path on every machine: `/name` on
//! Linux and macOS, and `C:\name` on Windows, where programs running on drive
//! C: also resolve `/name`. Only an administrator may create a folder at the
//! top of the disk; when the system refuses, pigeon names the command that
//! creates it once.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The root of `group` on every machine of this system's kind.
#[must_use]
pub fn shared_root(group: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(format!("C:\\{group}"))
    } else {
        PathBuf::from(format!("/{group}"))
    }
}

/// Creates `root` unless it exists, and checks that this user can write it.
///
/// # Errors
///
/// Fails if the folder cannot be created or written; when the system
/// refuses, the error names the command an administrator runs once.
pub fn create_root(root: &Path) -> Result<()> {
    let probe = root.join(".~pigeon-probe");
    let created = std::fs::create_dir_all(root)
        .and_then(|()| std::fs::write(&probe, []))
        .and_then(|()| std::fs::remove_file(&probe));
    match created {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            bail!("{}", admin_command(std::env::consts::OS, root))
        }
        Err(error) => Err(error).with_context(|| format!("creating {}", root.display())),
    }
}

/// How an administrator of a system `os`, as named by
/// [`std::env::consts::OS`], gives this user the folder `root`.
fn admin_command(os: &str, root: &Path) -> String {
    let shown = root.display();
    let command = match (os, top_level_name(root)) {
        ("windows", _) => format!(
            "in a Command Prompt run as administrator: mkdir {shown} && icacls {shown} /grant \"%USERNAME%:(OI)(CI)F\""
        ),
        ("macos", Some(name)) => format!(
            "macOS keeps / read-only, so /etc/synthetic.conf links {shown} to a folder of your home: mkdir -p ~/{name} && printf '{name}\\t%s/{name}\\n' \"${{HOME#/}}\" | sudo tee -a /etc/synthetic.conf && sudo /System/Library/Filesystems/apfs.fs/Contents/Resources/apfs.util -t"
        ),
        _ => format!("sudo install -d -o \"$USER\" {shown}"),
    };
    format!(
        "only an administrator can give you {shown}; run this once, then try again: {command}; or choose another root folder"
    )
}

/// The name of `root` when it sits directly at the top of the disk.
fn top_level_name(root: &Path) -> Option<&str> {
    let parent = root.parent()?;
    (parent.parent().is_none()).then(|| root.file_name()?.to_str())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_is_named_after_the_group_at_the_top_of_the_disk() {
        let root = shared_root("cheapmo");
        if cfg!(windows) {
            assert_eq!(root, Path::new("C:\\cheapmo"));
        } else {
            assert_eq!(root, Path::new("/cheapmo"));
        }
    }

    #[test]
    fn each_system_names_its_own_command() {
        let top = Path::new("/cheapmo");
        let linux = admin_command("linux", top);
        assert!(linux.contains("sudo install -d -o \"$USER\" /cheapmo"));
        let macos = admin_command("macos", top);
        assert!(macos.contains("printf 'cheapmo\\t%s/cheapmo\\n' \"${HOME#/}\""));
        assert!(macos.contains("/etc/synthetic.conf"));
        assert!(macos.contains("mkdir -p ~/cheapmo"));
        let deep = admin_command("macos", Path::new("/srv/cheapmo"));
        assert!(deep.contains("sudo install -d"));
        assert!(!deep.contains("synthetic"));
        let windows = admin_command("windows", Path::new("C:\\cheapmo"));
        assert!(windows.contains("mkdir C:\\cheapmo && icacls C:\\cheapmo"));
    }

    #[test]
    fn a_writable_root_is_created_and_left_empty() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("a/cheapmo");
        create_root(&root).unwrap();
        create_root(&root).unwrap();
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn a_root_this_user_cannot_write_names_the_administrator_command() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        for root in [locked.join("cheapmo"), locked.clone()] {
            let error = create_root(&root).unwrap_err().to_string();
            assert!(error.contains("only an administrator"), "{error}");
            assert!(error.contains(&root.display().to_string()), "{error}");
        }
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
