//! Asking before a call goes ahead: an action that cannot be undone, or
//! that reaches the whole group, refuses with its question unless the call
//! says `yes`, and every interface asks that same question.

use anyhow::Result;

use crate::args::Args;

/// The refusal of a call that would go ahead if made again with `yes`.
#[derive(Debug)]
pub struct Confirm {
    pub question: String,
}

impl std::fmt::Display for Confirm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} Pass --yes to go ahead.", self.question)
    }
}

impl std::error::Error for Confirm {}

impl Confirm {
    /// Refuses with `question` unless `yes`.
    ///
    /// # Errors
    ///
    /// Fails with the question if not `yes`.
    pub fn unless(yes: bool, question: impl FnOnce() -> String) -> Result<()> {
        if yes {
            return Ok(());
        }
        Err(Self {
            question: question(),
        }
        .into())
    }
}

impl Args {
    /// Lets the call go ahead if it says `yes`, and otherwise refuses with
    /// the question `question` tells.
    ///
    /// # Errors
    ///
    /// Fails with the question if the call does not say `yes`.
    pub fn confirm(&self, question: impl FnOnce() -> String) -> Result<()> {
        Confirm::unless(self.flag("yes"), question)
    }
}

/// `count` suggestions, as "this suggestion" or "these 3 suggestions".
#[must_use]
pub fn these(count: usize) -> String {
    match count {
        1 => "this suggestion".to_owned(),
        count => format!("these {count} suggestions"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_without_yes_is_refused_with_the_question_and_the_way_to_answer() {
        let refusal = Confirm::unless(false, || "Go on?".to_owned()).unwrap_err();
        assert_eq!(refusal.to_string(), "Go on? Pass --yes to go ahead.");
        assert_eq!(
            refusal.downcast_ref::<Confirm>().unwrap().question,
            "Go on?"
        );
    }

    #[test]
    fn a_call_with_yes_goes_ahead_without_asking() {
        Confirm::unless(true, || unreachable!("asked")).unwrap();
    }

    #[test]
    fn suggestions_are_counted_in_the_question() {
        assert_eq!(these(1), "this suggestion");
        assert_eq!(these(3), "these 3 suggestions");
    }
}
