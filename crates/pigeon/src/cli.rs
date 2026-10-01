//! The command line: `pigeon <noun> <verb>` commands generated from the
//! catalog, which prompt for a missing argument only when a terminal is
//! attached and otherwise call the daemon's API, and ask before a reload
//! frees space; plus the commands that run the daemon, set pigeon up,
//! install its service, update it, serve a relay, link to the web UI, and
//! complete the shell.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, ValueHint, builder::PossibleValuesParser};
use clap_complete::Shell;
use data_encoding::BASE64;
use serde_json::{Map, Value};

use crate::catalog::{ACTIONS, Action, GROUP, Kind, NOUNS, Param, Scope, find};
use crate::home::Home;
use crate::{api, client, config_preview, relay, render, serve, service, setup, update};

fn param_arg(param: &Param) -> Arg {
    let arg = Arg::new(param.name).long(param.name).help(param.about);
    match param.kind {
        Kind::Flag => arg.action(ArgAction::SetTrue),
        Kind::Choice(choices) => arg.value_parser(PossibleValuesParser::new(choices)),
        Kind::Bytes | Kind::Document => arg.value_hint(ValueHint::FilePath).value_name("FILE"),
        Kind::Folder => arg.value_hint(ValueHint::DirPath).value_name("FOLDER"),
        Kind::Number => arg.value_name("NUMBER"),
        Kind::Text | Kind::Path | Kind::Pattern | Kind::Time => arg,
    }
}

fn action_command(action: &Action) -> Command {
    let mut command = Command::new(action.verb).about(action.about);
    if action.scope == Scope::Group {
        command = command.arg(param_arg(&GROUP).short('g'));
    }
    command.args(action.params.iter().map(param_arg))
}

/// The whole command line.
#[must_use]
pub fn command() -> Command {
    let mut root = Command::new("pigeon")
        .about("Peer-to-peer file sync for a trusted group")
        .version(crate::VERSION)
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Print results as JSON"),
        );
    for (noun, about) in NOUNS {
        let verbs = ACTIONS.iter().filter(|action| action.noun == *noun);
        root = root.subcommand(
            Command::new(*noun)
                .about(*about)
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommands(verbs.map(action_command)),
        );
    }
    root.mut_subcommand("daemon", |daemon| {
        daemon
            .subcommand_required(false)
            .arg_required_else_help(false)
            .args_conflicts_with_subcommands(true)
            .arg(
                Arg::new("port")
                    .long("port")
                    .help(format!("The localhost port, {} by default; 0 picks any free one", serve::PORT))
                    .value_parser(clap::value_parser!(u16)),
            )
    })
    .subcommand(
        Command::new("update")
            .about("Build the head of pigeon's main branch with cargo, and restart the daemon onto it if it changed")
            .arg(
                Arg::new("path")
                    .long("path")
                    .help("A clone of pigeon's repository to build instead")
                    .value_hint(ValueHint::DirPath)
                    .value_name("FOLDER")
                    .value_parser(clap::value_parser!(PathBuf)),
            ),
    )
    .subcommand(
        Command::new("relay")
            .about("Serve the group's relay, which carries as ciphertext what machines cannot send directly")
            .arg(
                Arg::new("hostname")
                    .long("hostname")
                    .required(true)
                    .help("The name or address other machines reach this one at"),
            )
            .arg(
                Arg::new("contact")
                    .long("contact")
                    .help("An email Let's Encrypt may warn: serve HTTPS on port 443 with its certificate"),
            )
            .arg(
                Arg::new("port")
                    .long("port")
                    .help("The HTTP port, which Let's Encrypt needs at 80")
                    .value_parser(clap::value_parser!(u16))
                    .default_value("80"),
            ),
    )
    .subcommand(
        Command::new("setup")
            .about("Set up pigeon on this machine step by step: start at login, join or found a group, choose what to follow"),
    )
    .subcommand(
        Command::new("service")
            .about("The service that starts the daemon at login")
            .subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                Command::new("install")
                    .about("Start the daemon at login, as a systemd user service or a launchd agent, replacing a daemon started by hand")
                    .arg(
                        Arg::new("linger")
                            .long("linger")
                            .action(ArgAction::SetTrue)
                            .help("Start it at boot too, without a login, as a server needs (Linux)"),
                    ),
            ),
    )
    .subcommand(Command::new("ui").about("Print the link that opens the web UI"))
    .subcommand(
        Command::new("completions")
            .about("Print the completion script of a shell")
            .arg(
                Arg::new("shell")
                    .required(true)
                    .value_parser(clap::value_parser!(Shell)),
            ),
    )
}

fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// Reads the local file `name`, or standard input for `-`.
fn read_local(name: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if name == "-" {
        std::io::stdin()
            .read_to_end(&mut bytes)
            .context("reading standard input")?;
    } else {
        bytes = std::fs::read(name).with_context(|| format!("reading {name}"))?;
    }
    Ok(bytes)
}

/// Asks the person at the terminal for `param`.
fn prompt(param: &Param) -> Result<String> {
    Ok(dialoguer::Input::<String>::new()
        .with_prompt(param.about)
        .interact_text()?)
}

/// The arguments of `action` from the command line, prompting for the
/// missing ones when a terminal is attached.
///
/// # Errors
///
/// Fails, naming the flag, if a required argument is missing and nobody
/// can be asked, or a content file cannot be read.
pub fn arguments(action: &Action, matches: &ArgMatches, ask: bool) -> Result<Map<String, Value>> {
    let mut args = Map::new();
    let scoped = (action.scope == Scope::Group).then_some(&GROUP);
    for param in scoped.into_iter().chain(action.params) {
        if param.kind == Kind::Flag {
            if matches.get_flag(param.name) {
                args.insert(param.name.to_owned(), Value::Bool(true));
            }
            continue;
        }
        let given = matches.get_one::<String>(param.name).cloned();
        let value = match given {
            Some(value) => value,
            None if !param.required => continue,
            None if ask => prompt(param)?,
            None => bail!(
                "{} needs --{}: pass it, or run the command in a terminal",
                action.command(),
                param.name
            ),
        };
        let value = match param.kind {
            Kind::Bytes => BASE64.encode(&read_local(&value)?),
            Kind::Document => String::from_utf8(read_local(&value)?)
                .with_context(|| format!("--{}: {value} is not UTF-8 text", param.name))?,
            _ => value,
        };
        args.insert(param.name.to_owned(), Value::String(value));
    }
    Ok(args)
}

/// What the edits of `group`'s config.toml free on this machine, as text,
/// if anything; nothing when the file does not read, which reloading then
/// names.
fn freed(home: &Home, group: &str) -> Option<String> {
    let mut args = Map::new();
    args.insert(GROUP.name.to_owned(), Value::String(group.to_owned()));
    let preview = client::call(home, "config", "preview", &args).ok()?;
    let total = &preview["deltas"]
        .as_array()?
        .iter()
        .find(|delta| delta["delta"] == "free")?["total"];
    let files = total["files"].as_u64().filter(|files| *files > 0)?;
    let bytes = total["bytes"].as_u64().unwrap_or_default();
    Some(format!(
        "{group} frees {}",
        config_preview::amount(pigeon_sync::Amount { files, bytes })
    ))
}

/// Asks the person at the terminal, when the edits that reloading applies
/// free space on this machine, whether to apply them, telling what each
/// group frees; adds `yes` to `args` if so.
///
/// # Errors
///
/// Fails if the groups cannot be listed, the question cannot be asked, or
/// the person declines.
fn confirm_reload(home: &Home, args: &mut Map<String, Value>) -> Result<()> {
    let groups = client::call(home, "group", "list", &Map::new())?;
    let freed: Vec<String> = groups
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| freed(home, group["name"].as_str()?))
        .collect();
    if freed.is_empty() {
        return Ok(());
    }
    eprintln!(
        "The edits free space on this machine: {}.",
        freed.join(", ")
    );
    let apply = dialoguer::Confirm::new()
        .with_prompt("Apply them?")
        .default(false)
        .interact()?;
    if !apply {
        bail!("nothing changed");
    }
    args.insert("yes".to_owned(), Value::Bool(true));
    Ok(())
}

/// Runs the command line `matches` describes.
///
/// # Errors
///
/// Fails with a message for the user.
pub fn run(matches: &ArgMatches) -> Result<()> {
    let home = Home::locate()?;
    let json = matches.get_flag("json");
    let Some((noun, noun_matches)) = matches.subcommand() else {
        bail!("run `pigeon --help`");
    };
    match noun {
        "daemon" if noun_matches.subcommand().is_none() => {
            let port = noun_matches
                .get_one::<u16>("port")
                .copied()
                .unwrap_or(serve::PORT);
            let restart = tokio::runtime::Runtime::new()?.block_on(serve::run(home, port))?;
            restart.map_or(Ok(()), |program| Err(program.restart()))
        }
        "update" => update::run(
            &home,
            noun_matches
                .get_one::<PathBuf>("path")
                .map(PathBuf::as_path),
        ),
        "relay" => {
            let hostname = noun_matches
                .get_one::<String>("hostname")
                .ok_or_else(|| anyhow!("pigeon relay needs --hostname"))?;
            let contact = noun_matches
                .get_one::<String>("contact")
                .map(String::as_str);
            let port = noun_matches.get_one::<u16>("port").copied().unwrap_or(80);
            tokio::runtime::Runtime::new()?.block_on(relay::run(&home, hostname, contact, port))
        }
        "setup" => setup::run(&home),
        "service" => service::install(
            &home,
            noun_matches
                .subcommand_matches("install")
                .is_some_and(|install| install.get_flag("linger")),
        ),
        "ui" => {
            println!("{}", api::open_link(home.address()?, &home.token()?));
            Ok(())
        }
        "completions" => {
            let Some(shell) = noun_matches.get_one::<Shell>("shell").copied() else {
                bail!("run `pigeon completions --help`");
            };
            clap_complete::generate(shell, &mut command(), "pigeon", &mut std::io::stdout());
            Ok(())
        }
        _ => {
            let (verb, verb_matches) = noun_matches
                .subcommand()
                .ok_or_else(|| anyhow!("run `pigeon {noun} --help`"))?;
            let action = find(noun, verb).ok_or_else(|| anyhow!("run `pigeon {noun} --help`"))?;
            let mut args = arguments(action, verb_matches, interactive())?;
            if (noun, verb) == ("daemon", "reload") && !args.contains_key("yes") && interactive() {
                confirm_reload(&home, &mut args)?;
            }
            let result = client::call(&home, noun, verb, &args)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                print!("{}", render::text(action, &result));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        command().debug_assert();
    }

    #[test]
    fn flags_become_the_api_arguments() {
        let matches = command()
            .try_get_matches_from([
                "pigeon",
                "selection",
                "unfollow",
                "-g",
                "cheapmo",
                "--pattern",
                "/docs/",
                "--free",
            ])
            .unwrap();
        let (_, nouns) = matches.subcommand().unwrap();
        let (_, verbs) = nouns.subcommand().unwrap();
        let args = arguments(find("selection", "unfollow").unwrap(), verbs, false).unwrap();
        assert_eq!(
            Value::Object(args),
            serde_json::json!({"group": "cheapmo", "pattern": "/docs/", "free": true})
        );
    }

    #[test]
    fn a_missing_argument_without_a_terminal_names_its_flag() {
        let matches = command()
            .try_get_matches_from(["pigeon", "file", "delete"])
            .unwrap();
        let (_, nouns) = matches.subcommand().unwrap();
        let (_, verbs) = nouns.subcommand().unwrap();
        let error = arguments(find("file", "delete").unwrap(), verbs, false).unwrap_err();
        assert_eq!(
            error.to_string(),
            "pigeon file delete needs --path: pass it, or run the command in a terminal"
        );
    }

    #[test]
    fn content_is_read_from_a_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "hello").unwrap();
        let matches = command()
            .try_get_matches_from([
                "pigeon",
                "file",
                "write",
                "--path",
                "a.txt",
                "--content",
                file.to_str().unwrap(),
            ])
            .unwrap();
        let (_, nouns) = matches.subcommand().unwrap();
        let (_, verbs) = nouns.subcommand().unwrap();
        let args = arguments(find("file", "write").unwrap(), verbs, false).unwrap();
        assert_eq!(args["content"], Value::String(BASE64.encode(b"hello")));
    }
}
