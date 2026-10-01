//! Retention: which past versions of a file a machine keeps, given their
//! dates: every version for a day, the last of each day for 30 days, the
//! last of each week for a year, and the last before a deletion for a year;
//! then a quota, a share of the disk, drops the oldest first.

use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

use crate::patch::{Content, ContentHash};

/// How long each tier lasts, in seconds, the share of the disk history may
/// fill, and whose files it covers; adjustable on each machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Retention {
    pub every: u64,
    pub daily: u64,
    pub weekly: u64,
    pub before_deletion: u64,
    /// The percentage of the disk that past versions may fill.
    pub quota_percent: u8,
    /// Whether history covers every file the machine downloads, not only
    /// the member's own.
    pub everything: bool,
}

const DAY: u64 = 86_400;

impl Default for Retention {
    fn default() -> Self {
        Self {
            every: DAY,
            daily: 30 * DAY,
            weekly: 365 * DAY,
            before_deletion: 365 * DAY,
            quota_percent: 20,
            everything: false,
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

/// The contents of `history`, past versions as `(seconds, content)`, that
/// fit in `quota` bytes when the oldest go first. A content counts once,
/// however many versions share it, and lasts as long as its newest version;
/// contents in `kept_anyway` cost nothing, since they stay regardless.
#[must_use]
pub fn within_quota<S: BuildHasher>(
    history: &[(u64, Content)],
    quota: u64,
    kept_anyway: &HashSet<ContentHash, S>,
) -> HashSet<ContentHash> {
    let mut newest: HashMap<ContentHash, (u64, u64)> = HashMap::new();
    for (seconds, content) in history {
        if kept_anyway.contains(&content.hash) {
            continue;
        }
        let entry = newest
            .entry(content.hash)
            .or_insert((*seconds, content.size));
        entry.0 = entry.0.max(*seconds);
    }
    let mut by_age: Vec<(u64, u64, ContentHash)> = newest
        .into_iter()
        .map(|(hash, (seconds, size))| (seconds, size, hash))
        .collect();
    by_age.sort_unstable_by(|a, b| b.cmp(a));
    let mut used = 0u64;
    by_age
        .into_iter()
        .take_while(|(_, size, _)| {
            used = used.saturating_add(*size);
            used <= quota
        })
        .map(|(_, _, hash)| hash)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(byte: u8, size: u64) -> Content {
        Content {
            hash: ContentHash([byte; 32]),
            size,
            executable: false,
        }
    }

    #[test]
    fn a_stored_retention_without_the_newer_settings_reads_with_defaults() {
        let stored = r#"{"every":1,"daily":2,"weekly":3,"before_deletion":4}"#;
        let retention: Retention = serde_json::from_str(stored).unwrap();
        assert_eq!(retention.every, 1);
        assert_eq!(retention.quota_percent, 20);
        assert!(!retention.everything);
    }

    #[test]
    fn the_quota_drops_the_oldest_contents_first() {
        let history = [
            (10, content(1, 40)),
            (20, content(2, 40)),
            (30, content(1, 40)),
            (40, content(3, 50)),
            (50, content(4, 30)),
        ];
        let kept = |quota, anyway: &[u8]| {
            let anyway: HashSet<ContentHash> =
                anyway.iter().map(|byte| ContentHash([*byte; 32])).collect();
            let mut kept: Vec<u8> = within_quota(&history, quota, &anyway)
                .into_iter()
                .map(|hash| hash.0[0])
                .collect();
            kept.sort_unstable();
            kept
        };
        assert_eq!(kept(1000, &[]), [1, 2, 3, 4]);
        assert_eq!(kept(120, &[]), [1, 3, 4]);
        assert_eq!(kept(119, &[]), [3, 4]);
        assert_eq!(kept(79, &[]), [4]);
        assert_eq!(kept(0, &[]), Vec::<u8>::new());
        assert_eq!(kept(90, &[3]), [1, 4]);
    }

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
