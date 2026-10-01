//! How the daemon runs in the background: as a service started at login,
//! a systemd user unit on Linux or a launchd agent on macOS, running the
//! installed program, which on Linux may also start at boot without a
//! login; or once, detached from the terminal that starts it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Map;

use crate::client;
use crate::home::{HOME_VARIABLE, Home};

/// The service's name: the systemd unit's and the launchd agent's.
const UNIT: &str = "pigeon.service";
const LABEL: &str = "com.github.mariogeiger.pigeon";

/// How long the daemon may take to start or stop answering.
const ANSWER: Duration = Duration::from_secs(20);

/// The service file of this system, or why it has none.
fn service_file() -> Result<PathBuf> {
    match std::env::consts::OS {
        "linux" => dirs::config_dir()
            .map(|config| config.join("systemd").join("user").join(UNIT))
            .ok_or_else(|| anyhow!("this system names no configuration folder")),
        "macos" => dirs::home_dir()
            .map(|home| {
                home.join("Library")
                    .join("LaunchAgents")
                    .join(format!("{LABEL}.plist"))
            })
            .ok_or_else(|| anyhow!("this system names no home folder")),
        os => bail!(
            "pigeon installs no service on {os} yet: add `pigeon daemon` to the programs started at login"
        ),
    }
}

/// The systemd user unit that runs `program`'s daemon on `home`, when it
/// is not the default one.
fn systemd_unit(program: &Path, home: Option<&Path>) -> String {
    let environment = home.map_or_else(String::new, |home| {
        format!("Environment=\"{HOME_VARIABLE}={}\"\n", home.display())
    });
    format!(
        "[Unit]\nDescription=pigeon\nAfter=network-online.target\n\n[Service]\n{environment}ExecStart=\"{}\" daemon\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        program.display()
    )
}

/// The launchd agent that runs `program`'s daemon on `home`, when it is
/// not the default one, and restarts it when it fails.
fn launchd_agent(program: &Path, home: Option<&Path>) -> String {
    let escape = |path: &Path| {
        path.display()
            .to_string()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
    };
    let environment = home.map_or_else(String::new, |home| {
        format!(
            "  <key>EnvironmentVariables</key>\n  <dict><key>{HOME_VARIABLE}</key><string>{}</string></dict>\n",
            escape(home)
        )
    });
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{LABEL}</string>\n  <key>ProgramArguments</key>\n  <array><string>{}</string><string>daemon</string></array>\n{environment}  <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n</dict>\n</plist>\n",
        escape(program)
    )
}

/// Runs `program` with `args`, quietly, and tells whether it succeeded.
fn succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Runs `program` with `args`, failing with its name if it fails.
fn run(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("running {program}"))?;
    if !status.success() {
        bail!("`{program} {}` failed", args.join(" "));
    }
    Ok(())
}

/// The launchd domain of this user's login session.
fn launchd_domain() -> Result<String> {
    let id = Command::new("id")
        .arg("-u")
        .output()
        .context("running id")?;
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&id.stdout).trim()
    ))
}

/// Whether the service starts the daemon at login.
#[must_use]
pub fn enabled() -> bool {
    match std::env::consts::OS {
        "linux" => succeeds("systemctl", &["--user", "is-enabled", "--quiet", UNIT]),
        _ => service_file().is_ok_and(|file| file.is_file()),
    }
}

/// Whether the service, rather than a daemon started by hand, runs now.
fn active() -> bool {
    match std::env::consts::OS {
        "linux" => succeeds("systemctl", &["--user", "is-active", "--quiet", UNIT]),
        "macos" => launchd_domain()
            .is_ok_and(|domain| succeeds("launchctl", &["print", &format!("{domain}/{LABEL}")])),
        _ => false,
    }
}

/// Whether the service starts at boot, without a login.
#[must_use]
pub fn lingers() -> bool {
    let user = std::env::var("USER").unwrap_or_default();
    Command::new("loginctl")
        .args(["show-user", &user, "--property=Linger"])
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "Linger=yes")
}

/// Waits until the daemon of `home` answers, or stops answering.
///
/// # Errors
///
/// Fails if it does not within a while.
pub fn wait_until_answering(home: &Home, answering: bool) -> Result<()> {
    let deadline = Instant::now() + ANSWER;
    while client::answers(home) != answering {
        if Instant::now() > deadline {
            bail!(
                "the daemon {} answering",
                if answering { "is not" } else { "is still" }
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Ok(())
}

/// Installs the service that starts the daemon of `home` at login, and
/// at boot too when `linger`, stopping a daemon started by hand first,
/// and starts it.
///
/// # Errors
///
/// Fails if this system has no service manager pigeon knows, or it
/// refuses.
pub fn install(home: &Home, linger: bool) -> Result<()> {
    let file = service_file()?;
    let program = std::env::current_exe().context("finding the running program")?;
    let custom = std::env::var_os(HOME_VARIABLE).map(|_| home.path());
    if client::answers(home) && !active() {
        client::call(home, "daemon", "stop", &Map::new())?;
        wait_until_answering(home, false)?;
    }
    let text = if cfg!(target_os = "macos") {
        launchd_agent(&program, custom)
    } else {
        systemd_unit(&program, custom)
    };
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)
            .with_context(|| format!("creating {}", folder.display()))?;
    }
    std::fs::write(&file, text).with_context(|| format!("writing {}", file.display()))?;
    if cfg!(target_os = "macos") {
        if linger {
            bail!("macOS starts agents at login only: turn on automatic login for this user");
        }
        let domain = launchd_domain()?;
        let _ = succeeds("launchctl", &["bootout", &format!("{domain}/{LABEL}")]);
        run(
            "launchctl",
            &["bootstrap", &domain, &file.display().to_string()],
        )?;
    } else {
        run("systemctl", &["--user", "daemon-reload"])?;
        run("systemctl", &["--user", "enable", UNIT])?;
        run("systemctl", &["--user", "restart", UNIT])?;
        if linger && !succeeds("loginctl", &["enable-linger"]) {
            bail!(
                "loginctl refused to let pigeon start at boot: run `sudo loginctl enable-linger \"$USER\"`"
            );
        }
    }
    wait_until_answering(home, true)
}

/// Starts the daemon of `home` once, detached from this terminal, its
/// output kept in the pigeon folder, and waits until it answers.
///
/// # Errors
///
/// Fails if the program cannot start or its daemon does not answer.
pub fn start_detached(home: &Home) -> Result<PathBuf> {
    let program = std::env::current_exe().context("finding the running program")?;
    std::fs::create_dir_all(home.path())
        .with_context(|| format!("creating {}", home.path().display()))?;
    let log = home.path().join("daemon.log");
    let output =
        std::fs::File::create(&log).with_context(|| format!("creating {}", log.display()))?;
    let mut command = Command::new(program);
    command
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().context("starting the daemon")?;
    wait_until_answering(home, true)?;
    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_runs_the_installed_daemon_on_its_folder() {
        let program = Path::new("/home/mario/.cargo/bin/pigeon");
        let unit = systemd_unit(program, None);
        assert!(unit.contains("ExecStart=\"/home/mario/.cargo/bin/pigeon\" daemon\n"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(!unit.contains(HOME_VARIABLE));
        let moved = systemd_unit(program, Some(Path::new("/srv/pigeon")));
        assert!(moved.contains("Environment=\"PIGEON_HOME=/srv/pigeon\"\n"));
        let agent = launchd_agent(Path::new("/Users/m&m/.cargo/bin/pigeon"), None);
        assert!(
            agent.contains(
                "<string>/Users/m&amp;m/.cargo/bin/pigeon</string><string>daemon</string>"
            )
        );
        assert!(agent.contains("<key>RunAtLoad</key><true/>"));
    }
}
