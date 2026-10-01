//! The command line: `pigeon <noun> <verb>` commands generated from the
//! catalog, which prompt for a missing argument only when a terminal is
//! attached and otherwise call the daemon's API; plus the commands that run
//! the daemon, link to the web UI, and complete the shell.

use std::io::{IsTerminal, Read};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Arg, ArgAction, ArgMatches, Command, ValueHint, builder::PossibleValuesParser};
use clap_complete::Shell;
use data_encoding::BASE64;
use serde_json::{Map, Value};

use crate::catalog::{ACTIONS, Action, GROUP, Kind, NOUNS, Param, Scope, find};
use crate::home::Home;
use crate::{client, render, serve};

fn param_arg(param: &Param) -> Arg {
    let arg = Arg::new(param.name).long(param.name).help(param.about);
    match param.kind {
        Kind::Flag => arg.action(ArgAction::SetTrue),
        Kind::Choice(choices) => arg.value_parser(PossibleValuesParser::new(choices)),
        Kind::Bytes => arg.value_hint(ValueHint::FilePath).value_name("FILE"),
        Kind::Folder => arg.value_hint(ValueHint::DirPath).value_name("FOLDER"),
        Kind::Number => arg.value_name("NUMBER"),
        Kind::Text | Kind::Secret | Kind::Path | Kind::Pattern | Kind::Time => arg,
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
        .version(env!("CARGO_PKG_VERSION"))
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
    root.subcommand(
        Command::new("daemon")
            .about("Run pigeon: sync every group, answer the API, serve the web UI")
            .arg(
                Arg::new("port")
                    .long("port")
                    .help("The localhost port, any free one by default")
                    .value_parser(clap::value_parser!(u16))
                    .default_value("0"),
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

/// Reads the local file `name`, or standard input for `-`, as base64.
fn read_content(name: &str) -> Result<String> {
    let mut bytes = Vec::new();
    if name == "-" {
        std::io::stdin()
            .read_to_end(&mut bytes)
            .context("reading standard input")?;
    } else {
        bytes = std::fs::read(name).with_context(|| format!("reading {name}"))?;
    }
    Ok(BASE64.encode(&bytes))
}

/// Asks the person at the terminal for `param`.
fn prompt(param: &Param) -> Result<String> {
    let answer = if param.kind == Kind::Secret {
        dialoguer::Password::new()
            .with_prompt(param.about)
            .interact()?
    } else {
        dialoguer::Input::<String>::new()
            .with_prompt(param.about)
            .interact_text()?
    };
    Ok(answer)
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
        let value = if param.kind == Kind::Bytes {
            read_content(&value)?
        } else {
            value
        };
        args.insert(param.name.to_owned(), Value::String(value));
    }
    Ok(args)
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
        "daemon" => {
            let port = noun_matches.get_one::<u16>("port").copied().unwrap_or(0);
            tokio::runtime::Runtime::new()?.block_on(serve::run(home, port))
        }
        "ui" => {
            let address = home.address()?;
            println!("http://{address}/open?token={}", home.token()?);
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
            let args = arguments(action, verb_matches, interactive())?;
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
