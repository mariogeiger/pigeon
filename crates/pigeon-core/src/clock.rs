//! Timestamps that order all patches totally: a hybrid logical clock time,
//! never behind the wall clock nor any time its machine stamped or
//! received, with the machine's key breaking ties.

use std::fmt::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};
use uhlc::NTP64;

/// The identity of a machine: its iroh public key.
pub type MachineId = PublicKey;

/// When and where a patch was made; also the patch's identity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Stamp {
    /// NTP64 time: seconds in the high 32 bits, fraction in the low 32.
    pub time: u64,
    pub machine: MachineId,
}

impl Stamp {
    /// The stamp's wall-clock time.
    #[must_use]
    pub fn system_time(&self) -> SystemTime {
        NTP64(self.time).to_system_time()
    }

    /// The stamp's time in RFC 3339.
    #[must_use]
    pub fn rfc3339(&self) -> String {
        rfc3339(self.time)
    }

    /// A short, path-safe identity: time in hex, then the machine's key.
    #[must_use]
    pub fn label(&self) -> String {
        format!("{:016x}-{}", self.time, &self.machine.to_string()[..16])
    }
}

impl fmt::Debug for Stamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.time, self.machine.fmt_short())
    }
}

/// Converts wall-clock time to NTP64.
#[must_use]
pub fn ntp_time(time: SystemTime) -> u64 {
    NTP64::from(
        time.duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO),
    )
    .0
}

/// The NTP64 time `time` in RFC 3339, such as `2026-10-01T12:00:00.5Z`,
/// with the fewest fraction digits that read back as exactly `time`.
#[must_use]
pub fn rfc3339(time: u64) -> String {
    let seconds = NTP64(time >> 32 << 32).to_string_rfc3339_lossy();
    let mut text = seconds[..seconds.find('.').unwrap_or(seconds.len() - 1)].to_owned();
    let fraction = u128::from(time & u64::from(u32::MAX));
    if fraction != 0 {
        let scales = std::iter::successors(Some((1, 10u128)), |&(digits, scale)| {
            (digits < 10).then_some((digits + 1, scale * 10))
        });
        let (digits, decimal) = scales
            .map(|(digits, scale)| (digits, scale, (fraction * scale + (1 << 31)) >> 32))
            .find(|&(_, scale, decimal)| ((decimal << 33) / scale).div_ceil(2) == fraction)
            .map_or((10, 0), |(digits, _, decimal)| (digits, decimal));
        let _ = write!(text, ".{decimal:0digits$}");
    }
    text + "Z"
}

/// Reads an RFC 3339 time, such as `2026-10-01T12:00:00Z`, as the NTP64
/// time nearest to it.
///
/// # Errors
///
/// Returns why the text is not an RFC 3339 time.
pub fn parse_rfc3339(text: &str) -> Result<u64, String> {
    let invalid = |error| format!("{text:?} is not an RFC 3339 time: {error:?}");
    let (whole, digits) = match text.split_once('.') {
        Some((head, rest)) => {
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            (format!("{head}{}", &rest[end..]), &rest[..end])
        }
        None => (text.to_owned(), ""),
    };
    let seconds = NTP64::parse_rfc3339(&whole).map_err(invalid)?.0;
    Ok(seconds + binary_fraction(digits))
}

/// The decimal fraction `0.digits` in units of $2^{-32}$, rounded to
/// nearest, exactly for any number of digits.
fn binary_fraction(digits: &str) -> u64 {
    let mut decimal: Vec<u8> = digits.bytes().map(|digit| digit - b'0').collect();
    let mut bits = 0u64;
    for _ in 0..33 {
        let mut carry = 0;
        for digit in decimal.iter_mut().rev() {
            let doubled = *digit * 2 + carry;
            *digit = doubled % 10;
            carry = doubled / 10;
        }
        bits = bits << 1 | u64::from(carry);
    }
    bits.div_ceil(2)
}

/// The NTP64 span of `duration`.
fn ntp_span(duration: Duration) -> u64 {
    NTP64::from(duration).0
}

/// A machine's hybrid logical clock: each stamp is the wall clock's time,
/// or one past the latest time the clock stamped or observed when that is
/// later, so its stamps only increase, across restarts too once it observes
/// the patches it holds.
pub struct Clock {
    machine: MachineId,
    max_drift: Duration,
    /// The latest time stamped or observed.
    latest: AtomicU64,
}

impl Clock {
    /// A clock for `machine`, admitting received times up to `max_drift`
    /// ahead of the wall clock.
    #[must_use]
    pub fn new(machine: MachineId, max_drift: Duration) -> Self {
        Self {
            machine,
            max_drift,
            latest: AtomicU64::new(0),
        }
    }

    /// A new stamp, later than every time this clock stamped or observed.
    #[must_use]
    pub fn stamp(&self) -> Stamp {
        let wall = ntp_time(SystemTime::now());
        let (Ok(previous) | Err(previous)) =
            self.latest
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |latest| {
                    Some(wall.max(latest.saturating_add(1)))
                });
        Stamp {
            time: wall.max(previous.saturating_add(1)),
            machine: self.machine,
        }
    }

    /// Moves the clock past `time`, so that every later stamp follows it.
    pub fn observe(&self, time: u64) {
        self.latest.fetch_max(time, Ordering::SeqCst);
    }

    /// Whether a received stamp lies within the allowed drift of the wall
    /// clock, or else how far ahead of it.
    ///
    /// # Errors
    ///
    /// Returns how far ahead of the wall clock the stamp lies when that is
    /// beyond the allowed drift, which means one of the two machines has a
    /// wrong clock.
    pub fn admits(&self, stamp: &Stamp) -> Result<(), Duration> {
        let wall = ntp_time(SystemTime::now());
        if stamp.time <= wall.saturating_add(ntp_span(self.max_drift)) {
            return Ok(());
        }
        Err(NTP64(stamp.time - wall).to_duration())
    }

    /// How far the latest time this clock stamped or observed lies ahead of
    /// the wall clock: how much the wall clock lags behind the patches.
    #[must_use]
    pub fn lead(&self) -> Duration {
        let wall = ntp_time(SystemTime::now());
        NTP64(self.latest.load(Ordering::SeqCst).saturating_sub(wall)).to_duration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    #[test]
    fn stamps_increase_and_follow_observed_times() {
        let machine = SecretKey::from_bytes(&[7; 32]).public();
        let clock = Clock::new(machine, Duration::from_secs(3600));
        let a = clock.stamp();
        let b = clock.stamp();
        assert!(a < b);
        assert!(clock.lead() < Duration::from_secs(1));
        let ahead = b.time + (60 << 32);
        clock.observe(ahead);
        clock.observe(b.time);
        assert!(clock.stamp().time > ahead);
        assert!(clock.lead() > Duration::from_secs(59));
    }

    #[test]
    fn a_clock_set_back_still_stamps_after_every_time_it_observed() {
        let machine = SecretKey::from_bytes(&[7; 32]).public();
        let clock = Clock::new(machine, Duration::from_secs(300));
        let held = ntp_time(SystemTime::now()) + (3600 << 32);
        clock.observe(held);
        let next = clock.stamp();
        assert_eq!(next.time, held + 1);
        assert!(clock.stamp() > next);
        let lead = clock.lead();
        assert!(lead > Duration::from_secs(3599) && lead <= Duration::from_secs(3600));
    }

    #[test]
    fn a_stamp_is_admitted_only_within_the_drift() {
        let machine = SecretKey::from_bytes(&[7; 32]).public();
        let clock = Clock::new(machine, Duration::from_secs(300));
        let now = ntp_time(SystemTime::now());
        let at = |seconds: u64| Stamp {
            time: now + (seconds << 32),
            machine,
        };
        assert_eq!(clock.admits(&at(0)), Ok(()));
        assert_eq!(clock.admits(&at(200)), Ok(()));
        let refused = clock.admits(&at(600)).unwrap_err();
        assert!(refused > Duration::from_secs(599) && refused <= Duration::from_secs(600));
        assert!(clock.stamp().time < at(1).time);
    }

    #[test]
    fn rfc3339_times_read_back_as_they_print() {
        let machine = SecretKey::from_bytes(&[7; 32]).public();
        let stamp = Stamp {
            time: parse_rfc3339("2026-10-01T12:00:00Z").unwrap(),
            machine,
        };
        assert_eq!(stamp.time >> 32, 1_790_856_000);
        assert_eq!(parse_rfc3339(&stamp.rfc3339()), Ok(stamp.time));
        assert!(parse_rfc3339("yesterday").is_err());
    }

    #[test]
    fn every_time_reads_back_exactly_from_its_rfc3339() {
        let second = parse_rfc3339("2026-10-01T12:00:00Z").unwrap();
        assert_eq!(rfc3339(second), "2026-10-01T12:00:00Z");
        assert_eq!(rfc3339(second + (1 << 31)), "2026-10-01T12:00:00.5Z");
        assert_eq!(
            parse_rfc3339("2026-10-01T12:00:00.25Z"),
            Ok(second + (1 << 30))
        );
        assert_eq!(
            parse_rfc3339("2026-10-01T11:59:59.99999999999999Z"),
            Ok(second)
        );
        for fraction in [1, 2, 3, 0x8000_0001, 0xFFFF_FFFE, 0xFFFF_FFFF, 0x1234_5678] {
            let time = second + fraction;
            let text = rfc3339(time);
            assert_eq!(parse_rfc3339(&text), Ok(time), "{text}");
            assert!(
                text.len() <= "2026-10-01T12:00:00.0123456789Z".len(),
                "{text}"
            );
        }
    }
}
