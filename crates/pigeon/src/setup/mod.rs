//! `pigeon setup`: sets pigeon up on this machine step by step, in French,
//! redrawing a checklist that each step marks from the machine's real
//! state, so that running it again resumes. It is a client of the daemon's
//! API like the command line, each step a command one may run alone:
//! start at login, run the daemon, keep, join or found a group, choose
//! what to follow, as a server follows everything, and open the web UI.

mod ask;
mod checklist;
mod group;
mod installation;
mod root;
mod tree;

use std::io::IsTerminal;

use anyhow::{Context, Result, bail};
use dialoguer::console::Term;
use pigeon_core::path::GroupPath;
use pigeon_core::statement::STATEMENTS;
use serde_json::{Map, Value, json};

use self::checklist::{Checklist, Mark};
use self::group::{Membership, Origin};
use self::tree::{File, Tree};
use crate::home::Home;
use crate::{api, client, render, service};

const RUST: &str = "Rust";
const BUILT: &str = "pigeon compilé";
const INSTALLED: &str = "installé";
const SERVICE: &str = "Démarrage auto";
const DAEMON: &str = "Daemon en marche";
const GROUP: &str = "Groupe";
const NAME: &str = "Nom";
const ROOT: &str = "Dossier racine";
const FOLLOW: &str = "Quoi suivre";
const UI: &str = "Interface web";

/// Calls `noun verb` with the object `args` on the daemon of `home`.
fn call(home: &Home, noun: &str, verb: &str, args: Value) -> Result<Value> {
    let args: Map<String, Value> = serde_json::from_value(args)?;
    client::call(home, noun, verb, &args)
}

/// Sets pigeon up on this machine, asking on this terminal.
///
/// # Errors
///
/// Fails without a terminal, or if the daemon cannot run.
pub fn run(home: &Home) -> Result<()> {
    let term = Term::stdout();
    if !term.is_term() || !std::io::stdin().is_terminal() {
        bail!("pigeon setup asks questions: run it in a terminal");
    }
    let mut list = Checklist::new(
        term,
        &[
            RUST, BUILT, INSTALLED, SERVICE, DAEMON, GROUP, NAME, ROOT, FOLLOW, UI,
        ],
    );
    installation(&mut list)?;
    start_at_login(home, &mut list)?;
    run_daemon(home, &mut list)?;
    let membership = group::step(home, &mut list)?;
    follow(home, &mut list, &membership)?;
    open_ui(home, &mut list)
}

/// Marks what the shell finds of pigeon's installation.
fn installation(list: &mut Checklist) -> Result<()> {
    match installation::cargo_version() {
        Some(version) => list.set(RUST, Mark::Done, version)?,
        None => list.set(RUST, Mark::Failed, "curl https://sh.rustup.rs -sSf | sh")?,
    }
    list.set(BUILT, Mark::Done, crate::VERSION)?;
    let running = std::env::current_exe().context("finding the running program")?;
    match installation::on_path() {
        Some(found) if installation::same_file(&found, &running) => {
            list.set(INSTALLED, Mark::Done, found.display().to_string())
        }
        Some(found) => list.set(
            INSTALLED,
            Mark::Failed,
            format!("le shell trouve d'abord {}", found.display()),
        ),
        None => list.set(
            INSTALLED,
            Mark::Failed,
            format!(
                "{} n'est pas dans le PATH : . \"$HOME/.cargo/env\"",
                running.parent().unwrap_or(&running).display()
            ),
        ),
    }
}

/// Describes when the service starts the daemon.
fn service_detail() -> &'static str {
    if service::lingers() {
        "à l'ouverture de session et au démarrage"
    } else {
        "à l'ouverture de session"
    }
}

/// Offers to start the daemon at login.
fn start_at_login(home: &Home, list: &mut Checklist) -> Result<()> {
    if service::enabled() {
        return list.set(SERVICE, Mark::Done, service_detail());
    }
    let term = list.term().clone();
    if client::answers(home) {
        term.write_line("Un daemon lancé à la main tourne : le service le remplacera.")?;
    }
    if !ask::yes(
        &term,
        "Lancer pigeon automatiquement à l'ouverture de session ?",
        true,
    )? {
        return list.set(SERVICE, Mark::Skipped, "pigeon service install");
    }
    install_service(home, list, false)
}

/// Installs the service, starting at boot too when `linger`.
fn install_service(home: &Home, list: &mut Checklist, linger: bool) -> Result<()> {
    list.set(SERVICE, Mark::Running, "")?;
    match service::install(home, linger) {
        Ok(()) => list.set(SERVICE, Mark::Done, service_detail()),
        Err(error) => list.set(SERVICE, Mark::Failed, format!("{error:#}")),
    }
}

/// Starts the daemon unless it answers.
fn run_daemon(home: &Home, list: &mut Checklist) -> Result<()> {
    if client::answers(home) {
        return list.set(DAEMON, Mark::Done, home.address()?.to_string());
    }
    list.set(DAEMON, Mark::Running, "")?;
    match service::start_detached(home) {
        Ok(log) => {
            list.note(format!(
                "Le daemon tourne jusqu'au prochain redémarrage ; son journal : {}",
                log.display()
            ));
            list.set(DAEMON, Mark::Done, home.address()?.to_string())
        }
        Err(error) => {
            list.set(DAEMON, Mark::Failed, format!("{error:#}"))?;
            Err(error)
        }
    }
}

/// Chooses what this machine follows: everything for a server, or what
/// one checks in the tree of the group's files.
fn follow(home: &Home, list: &mut Checklist, membership: &Membership) -> Result<()> {
    let term = list.term().clone();
    let group = &membership.group;
    if ask::yes(
        &term,
        "Cette machine est un serveur toujours allumé ?",
        false,
    )? {
        install_service(home, list, true)?;
        call(
            home,
            "selection",
            "follow",
            json!({ "group": group, "pattern": "*" }),
        )?;
        call(
            home,
            "retention",
            "set",
            json!({ "group": group, "everything": "on" }),
        )?;
        return list.set(FOLLOW, Mark::Done, "tout, avec son historique (serveur)");
    }
    let tag = format!("+{}", membership.member);
    let own = format!("{tag}/");
    match membership.origin {
        Origin::Founded | Origin::Joined { heard: false } => {
            return list.set(
                FOLLOW,
                Mark::Skipped,
                format!("{own} ; le reste depuis l'interface web"),
            );
        }
        Origin::Kept if !ask::yes(&term, "Modifier ce que cette machine suit ?", false)? => {
            return list.set(FOLLOW, Mark::Done, "inchangé");
        }
        _ => {}
    }
    let files = call(home, "file", "list", json!({ "group": group }))?;
    let files: Vec<File> = files
        .as_array()
        .into_iter()
        .flatten()
        .filter(|file| {
            GroupPath::parse(file["path"].as_str().unwrap_or_default())
                .is_ok_and(|path| !path.is_inside(STATEMENTS))
        })
        .map(|file| {
            let path = file["path"].as_str().unwrap_or_default().to_owned();
            let locked = path.split('/').rev().skip(1).any(|folder| folder == tag);
            File {
                size: file["content"]["size"].as_u64().unwrap_or(0),
                followed: file["cutoff"] == "PlusInfinity",
                held: file["held"].as_bool().unwrap_or(false),
                locked,
                path,
            }
        })
        .collect();
    if files.is_empty() {
        return list.set(FOLLOW, Mark::Skipped, format!("{own} ; le groupe est vide"));
    }
    let tree = Tree::new(files);
    let Some(chosen) = tree.choose(&term)? else {
        return list.set(FOLLOW, Mark::Skipped, "inchangé");
    };
    for (pattern, follows) in &chosen.toggles {
        let verb = if *follows { "follow" } else { "unfollow" };
        call(
            home,
            "selection",
            verb,
            json!({ "group": group, "pattern": pattern }),
        )?;
    }
    if chosen.toggles.iter().any(|(_, follows)| !follows) {
        call(
            home,
            "selection",
            "follow",
            json!({ "group": group, "pattern": own }),
        )?;
    }
    list.set(
        FOLLOW,
        Mark::Done,
        format!("{} à télécharger", render::size(chosen.download)),
    )
}

/// Shows the link to the web UI, and opens it in a browser if one can.
fn open_ui(home: &Home, list: &mut Checklist) -> Result<()> {
    let link = api::open_link(home.address()?, &home.token()?);
    let opened = installation::open_in_browser(&link);
    list.note("Mettre pigeon à jour plus tard : pigeon update");
    let detail = if opened {
        format!("{link} (ouverte)")
    } else {
        link
    };
    list.set(UI, Mark::Done, detail)
}
