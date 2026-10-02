//! Completion in the shell: the script each shell sources, which asks the
//! program itself what completes the word under the cursor, and the values
//! of the arguments the daemon lists, such as groups, members, the group's
//! paths, version times and suggestion ids.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};

use clap::ArgMatches;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
use clap_complete::env::Shells;
use serde_json::{Map, Value};

use crate::catalog::{Action, GROUP, Kind, Param, Scope, find};
use crate::home::Home;
use crate::{cli, client, render};

/// The environment variable that asks the program to complete rather
/// than run.
const VARIABLE: &str = "COMPLETE";

/// The shells pigeon completes in.
const SHELLS: &Shells<'static> = &Shells::builtins();

/// The names of the shells pigeon completes in.
pub fn shells() -> impl Iterator<Item = &'static str> {
    SHELLS.names()
}

/// Answers the shell, and exits, when it asks to complete the command
/// line; otherwise returns.
pub fn answer() {
    clap_complete::CompleteEnv::with_factory(cli::command)
        .var(VARIABLE)
        .complete();
}

/// Writes the completion script of `shell` to `out`.
///
/// # Errors
///
/// Fails if `shell` is none of [`shells`], or `out` refuses the script.
pub fn register(shell: &str, out: &mut dyn std::io::Write) -> anyhow::Result<()> {
    let completer = SHELLS.completer(shell).ok_or_else(|| {
        anyhow::anyhow!(
            "pigeon completes {}",
            shells().collect::<Vec<_>>().join(", ")
        )
    })?;
    completer.write_registration(VARIABLE, "pigeon", "pigeon", "pigeon", out)?;
    Ok(())
}

/// What completes the values of `param`, if an action lists them.
#[must_use]
pub fn completer(param: &'static Param) -> Option<ArgValueCompleter> {
    let (noun, verb) = param.listed_by?;
    let listing = find(noun, verb)?;
    Some(ArgValueCompleter::new(move |typed: &OsStr| {
        let typed = typed.to_string_lossy();
        let Some(items) = list(listing, &line()) else {
            return Vec::new();
        };
        values(param, listing, &items, &typed)
    }))
}

/// The words of the command line before the one being completed, as the
/// shell passes them after `--`.
fn line() -> Vec<OsString> {
    let words: Vec<OsString> = std::env::args_os()
        .skip_while(|word| word != "--")
        .skip(1)
        .collect();
    let current = std::env::var("_CLAP_COMPLETE_INDEX")
        .ok()
        .and_then(|index| index.parse().ok())
        .unwrap_or(words.len().saturating_sub(1));
    words.into_iter().take(current).collect()
}

/// The matches of the innermost command `words` name.
fn innermost(matches: &ArgMatches) -> &ArgMatches {
    matches
        .subcommand()
        .map_or(matches, |(_, inner)| innermost(inner))
}

/// The items `listing` returns, given the arguments of `words` that it
/// takes too, such as the group.
fn list(listing: &Action, words: &[OsString]) -> Option<Vec<Value>> {
    let matches = cli::command()
        .ignore_errors(true)
        .try_get_matches_from(words)
        .ok()?;
    let given = innermost(&matches);
    let scoped = (listing.scope == Scope::Group).then_some(&GROUP);
    let mut args = Map::new();
    for param in scoped.into_iter().chain(listing.params) {
        if let Ok(Some(value)) = given.try_get_one::<String>(param.name) {
            args.insert(param.name.to_owned(), Value::String(value.clone()));
        }
    }
    let result = client::call(&Home::locate().ok()?, listing.noun, listing.verb, &args).ok()?;
    match result {
        Value::Array(items) => Some(items),
        _ => None,
    }
}

/// The candidates for `param` that begin with `typed`, among the `items`
/// that `listing` returned: their first column, a path one folder at a
/// time.
fn values(
    param: &Param,
    listing: &Action,
    items: &[Value],
    typed: &str,
) -> Vec<CompletionCandidate> {
    let Some((column, described)) = listing.columns.split_first() else {
        return Vec::new();
    };
    let value = |item: &Value| render::cell(column, &render::field(item, column));
    match param.kind {
        Kind::Path => entries(items.iter().map(value), typed),
        Kind::Pattern => {
            let (anchor, rest) = match typed.strip_prefix('/') {
                Some(rest) => ("/", rest),
                None if typed.is_empty() => ("/", typed),
                None => ("", typed),
            };
            entries(items.iter().map(value), rest)
                .into_iter()
                .map(|entry| entry.add_prefix(anchor))
                .collect()
        }
        _ => items
            .iter()
            .filter_map(|item| {
                let candidate = value(item);
                candidate.starts_with(typed).then(|| {
                    CompletionCandidate::new(candidate)
                        .help(Some(description(item, described).into()))
                })
            })
            .collect(),
    }
}

/// The `columns` of `item` that hold something, each after its header.
fn description(item: &Value, columns: &[&str]) -> String {
    columns
        .iter()
        .filter_map(|column| {
            let cell = render::cell(column, &render::field(item, column));
            (!cell.is_empty()).then(|| format!("{} {cell}", render::header(column)))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The entries of the tree of `paths` that complete `typed`: in the folder
/// `typed` ends in, the files, and the folders with a slash, whose names
/// begin with the rest of it; hidden ones only when that rest begins with
/// a dot.
fn entries(paths: impl Iterator<Item = String>, typed: &str) -> Vec<CompletionCandidate> {
    let folder = &typed[..typed.rfind('/').map_or(0, |slash| slash + 1)];
    let name = &typed[folder.len()..];
    let mut entries = BTreeSet::new();
    for path in paths {
        let Some(rest) = path.strip_prefix(folder) else {
            continue;
        };
        if !rest.starts_with(name) || (rest.starts_with('.') && !name.starts_with('.')) {
            continue;
        }
        let entry = rest.find('/').map_or(rest, |slash| &rest[..=slash]);
        entries.insert(format!("{folder}{entry}"));
    }
    entries.into_iter().map(CompletionCandidate::new).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shown(candidates: &[CompletionCandidate]) -> Vec<String> {
        candidates
            .iter()
            .map(|candidate| candidate.get_value().to_string_lossy().into_owned())
            .collect()
    }

    fn param(noun: &str, verb: &str, name: &str) -> &'static Param {
        find(noun, verb).unwrap().param(name).unwrap()
    }

    fn files() -> Vec<Value> {
        [
            "+mario/a.txt",
            "+mario/docs/plan.txt",
            "notes.md",
            ".pigeon/members/mario",
        ]
        .map(|path| json!({"path": path}))
        .into()
    }

    #[test]
    fn paths_complete_one_folder_at_a_time() {
        let path = param("file", "delete", "path");
        let list = find("file", "list").unwrap();
        assert_eq!(
            shown(&values(path, list, &files(), "")),
            ["+mario/", "notes.md"]
        );
        assert_eq!(
            shown(&values(path, list, &files(), "+mario/")),
            ["+mario/a.txt", "+mario/docs/"]
        );
        assert_eq!(
            shown(&values(path, list, &files(), "+mario/d")),
            ["+mario/docs/"]
        );
        assert_eq!(shown(&values(path, list, &files(), ".")), [".pigeon/"]);
    }

    #[test]
    fn patterns_complete_anchored_paths() {
        let pattern = param("selection", "follow", "pattern");
        let list = find("file", "list").unwrap();
        assert_eq!(
            shown(&values(pattern, list, &files(), "")),
            ["/+mario/", "/notes.md"]
        );
        assert_eq!(
            shown(&values(pattern, list, &files(), "/+mario/d")),
            ["/+mario/docs/"]
        );
        assert_eq!(shown(&values(pattern, list, &files(), "+m")), ["+mario/"]);
    }

    #[test]
    fn listed_values_are_described_by_the_other_columns() {
        let suggestions = param("suggestion", "discard", "suggestions");
        let list = find("suggestion", "list").unwrap();
        let items = [
            json!({"id": "6abe-048e", "author": "laurent", "reason": "not his", "changes": [{"path": "a.txt"}]}),
            json!({"id": "7c01-e34d", "author": "emmy"}),
        ];
        let candidates = values(suggestions, list, &items, "6");
        assert_eq!(shown(&candidates), ["6abe-048e"]);
        assert_eq!(
            candidates[0].get_help().unwrap().to_string(),
            "author laurent, reason not his, path a.txt"
        );
    }

    #[test]
    fn every_shell_has_a_script() {
        for shell in shells() {
            let mut script = Vec::new();
            register(shell, &mut script).unwrap();
            assert!(String::from_utf8(script).unwrap().contains(VARIABLE));
        }
    }
}
