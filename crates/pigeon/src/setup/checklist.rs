//! The checklist `pigeon setup` redraws at each step: one line per step,
//! marked done, running, to do, failed or skipped, and the notes the
//! steps leave, such as a group key to share or a warning.

use anyhow::Result;
use dialoguer::console::Term;

/// How a step stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    Done,
    Running,
    Todo,
    Failed,
    Skipped,
}

impl Mark {
    fn symbol(self) -> &'static str {
        match self {
            Self::Done => "✓",
            Self::Running => "…",
            Self::Todo => "☐",
            Self::Failed => "✗",
            Self::Skipped => "–",
        }
    }
}

struct Line {
    label: &'static str,
    mark: Mark,
    detail: String,
}

/// The steps, how each stands, and the notes, drawn on a terminal.
pub struct Checklist {
    term: Term,
    lines: Vec<Line>,
    notes: Vec<String>,
}

impl Checklist {
    /// A checklist of the steps `labels`, all to do.
    #[must_use]
    pub fn new(term: Term, labels: &[&'static str]) -> Self {
        let lines = labels
            .iter()
            .map(|&label| Line {
                label,
                mark: Mark::Todo,
                detail: String::new(),
            })
            .collect();
        Self {
            term,
            lines,
            notes: Vec::new(),
        }
    }

    /// The terminal it is drawn on.
    #[must_use]
    pub fn term(&self) -> &Term {
        &self.term
    }

    /// Marks the step `label` and redraws.
    ///
    /// # Errors
    ///
    /// Fails if the terminal cannot be written.
    pub fn set(&mut self, label: &str, mark: Mark, detail: impl Into<String>) -> Result<()> {
        if let Some(line) = self.lines.iter_mut().find(|line| line.label == label) {
            line.mark = mark;
            line.detail = detail.into();
        }
        self.draw()
    }

    /// Adds a note under the steps, shown from the next redraw on.
    pub fn note(&mut self, text: impl Into<String>) {
        self.notes.push(text.into());
    }

    /// Clears the terminal and draws the checklist.
    ///
    /// # Errors
    ///
    /// Fails if the terminal cannot be written.
    pub fn draw(&self) -> Result<()> {
        self.term.clear_screen()?;
        self.term.write_str(&self.render())?;
        Ok(())
    }

    fn render(&self) -> String {
        let width = self
            .lines
            .iter()
            .map(|line| line.label.chars().count())
            .max();
        let mut text = String::from("pigeon setup\n\n");
        for line in &self.lines {
            let label = format!("{:width$}", line.label, width = width.unwrap_or(0));
            text += format!("{} {label}  {}", line.mark.symbol(), line.detail).trim_end();
            text += "\n";
        }
        if !self.notes.is_empty() {
            text += "\n";
        }
        for note in &self.notes {
            text += note;
            text += "\n";
        }
        text + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_step_shows_its_mark_and_detail_aligned_above_the_notes() {
        let mut list = Checklist::new(Term::stdout(), &["Rust", "Group"]);
        list.lines[0].mark = Mark::Done;
        list.lines[0].detail = "cargo 1.91".into();
        list.note("key: abc");
        assert_eq!(
            list.render(),
            "pigeon setup\n\n✓ Rust   cargo 1.91\n☐ Group\n\nkey: abc\n\n"
        );
    }
}
