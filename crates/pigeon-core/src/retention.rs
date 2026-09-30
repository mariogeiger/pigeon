//! Retention: which past versions of a file a machine keeps, given their
//! dates: every version for a day, the last of each day for 30 days, the
//! last of each week for a year, and the last before a deletion for a year.

use std::collections::HashSet;

/// How long each tier lasts, in seconds; adjustable on each machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Retention {
    pub every: u64,
    pub daily: u64,
    pub weekly: u64,
    pub before_deletion: u64,
}

const DAY: u64 = 86_400;

impl Default for Retention {
    fn default() -> Self {
        Self {
            every: DAY,
            daily: 30 * DAY,
            weekly: 365 * DAY,
            before_deletion: 365 * DAY,
        }
    }
}

/// One version in a file's history: when it was made, in seconds, and
/// whether a deletion followed it directly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Dated {
    pub seconds: u64,
    pub deleted_next: bool,
}

impl Retention {
    /// Which of `history`, oldest first, to keep at time `now`. The last
    /// version is always kept: it is the file's current state.
    #[must_use]
    pub fn keep(&self, history: &[Dated], now: u64) -> Vec<bool> {
        let mut last_of_bucket: HashSet<(u8, u64)> = HashSet::new();
        let mut seen: HashSet<(u8, u64)> = HashSet::new();
        for (index, version) in history.iter().enumerate().rev() {
            for (tier, width) in [(0u8, DAY), (1u8, 7 * DAY)] {
                if seen.insert((tier, version.seconds / width)) {
                    last_of_bucket.insert((tier, index as u64));
                }
            }
        }
        history
            .iter()
            .enumerate()
            .map(|(index, version)| {
                let age = now.saturating_sub(version.seconds);
                index + 1 == history.len()
                    || age < self.every
                    || (age < self.daily && last_of_bucket.contains(&(0, index as u64)))
                    || (age < self.weekly && last_of_bucket.contains(&(1, index as u64)))
                    || (age < self.before_deletion && version.deleted_next)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dated(seconds: u64) -> Dated {
        Dated {
            seconds,
            deleted_next: false,
        }
    }

    #[test]
    fn tiers_thin_history_with_age() {
        let now = 400 * DAY;
        let history = [
            dated(now - 2 * DAY - 10),
            dated(now - 2 * DAY - 5),
            dated(now - 3600),
            dated(now - 60),
        ];
        assert_eq!(
            Retention::default().keep(&history, now),
            vec![false, true, true, true]
        );
    }

    #[test]
    fn weekly_and_deletion_tiers_last_a_year() {
        let now = 1000 * DAY;
        let mut deleted = dated(now - 300 * DAY);
        deleted.deleted_next = true;
        let history = [
            dated(now - 400 * DAY),
            deleted,
            dated(now - 200 * DAY - 1),
            dated(now - 200 * DAY),
        ];
        assert_eq!(
            Retention::default().keep(&history, now),
            vec![false, true, false, true]
        );
    }
}
