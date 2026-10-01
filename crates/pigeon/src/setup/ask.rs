//! The questions `pigeon setup` asks: yes or no, one choice
//! among several, a line of text, and a member name checked as it is
//! typed against the rules of names and the names already taken.

use anyhow::Result;
use dialoguer::console::{Key, Term, measure_text_width};
use dialoguer::{Input, Select};
use pigeon_core::name::MemberName;

/// Asks a yes-or-no `question`, `yes` being the answer to an empty line.
///
/// # Errors
///
/// Fails if the terminal cannot be read.
pub fn yes(term: &Term, question: &str, yes: bool) -> Result<bool> {
    let hint = if yes { "[Y/n]" } else { "[y/N]" };
    loop {
        term.write_str(&format!("{question} {hint} "))?;
        match term.read_line()?.trim().to_lowercase().as_str() {
            "" => return Ok(yes),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => term.write_line("Answer y (yes) or n (no).")?,
        }
    }
}

/// Asks to choose one of `choices`; returns its position.
///
/// # Errors
///
/// Fails if the terminal cannot be read.
pub fn choose(term: &Term, question: &str, choices: &[String]) -> Result<usize> {
    Ok(Select::new()
        .with_prompt(question)
        .items(choices)
        .default(0)
        .interact_on(term)?)
}

/// Asks for a line of text, starting from `initial`, until `problem` finds
/// none in it.
///
/// # Errors
///
/// Fails if the terminal cannot be read.
pub fn text(
    term: &Term,
    question: &str,
    initial: &str,
    problem: impl Fn(&str) -> Option<String>,
) -> Result<String> {
    Ok(Input::<String>::new()
        .with_prompt(question)
        .with_initial_text(initial)
        .validate_with(|text: &String| problem(text.trim()).map_or(Ok(()), Err))
        .interact_text_on(term)?
        .trim()
        .to_owned())
}

/// What keeps `text` from naming a new member, of a group where `taken`
/// are taken.
#[must_use]
pub fn name_problem(text: &str, taken: &[String]) -> Option<String> {
    if MemberName::parse(text).is_err() {
        Some("1 to 32 characters among a-z and 0-9".into())
    } else if taken.iter().any(|name| name == text) {
        Some("taken".into())
    } else {
        None
    }
}

/// Asks for a new member name, saying as it is typed whether it may be
/// taken in a group where `taken` are.
///
/// # Errors
///
/// Fails if the terminal cannot be read.
pub fn new_name(term: &Term, question: &str, taken: &[String]) -> Result<String> {
    let mut name = String::new();
    loop {
        let verdict = match name_problem(&name, taken) {
            None => "✓ free".to_owned(),
            Some(_) if name.is_empty() => String::new(),
            Some(problem) => format!("✗ {problem}"),
        };
        let after = format!("   {verdict}");
        term.clear_line()?;
        term.write_str(&format!("{question}: {name}{after}"))?;
        term.move_cursor_left(measure_text_width(&after))?;
        match term.read_key()? {
            Key::Char(character) if !character.is_control() => name.push(character),
            Key::Backspace => {
                name.pop();
            }
            Key::Enter if name_problem(&name, taken).is_none() => {
                term.write_line("")?;
                return Ok(name);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_name_follows_the_rules_of_names_and_is_not_taken() {
        let taken = vec!["mario".to_owned(), "lea".to_owned()];
        assert_eq!(name_problem("anna", &taken), None);
        assert_eq!(name_problem("mario", &taken).as_deref(), Some("taken"));
        for wrong in ["", "Anna", "an na", &"a".repeat(33)] {
            assert!(name_problem(wrong, &taken).is_some(), "{wrong}");
        }
    }
}
