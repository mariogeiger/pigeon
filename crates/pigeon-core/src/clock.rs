//! Timestamps that order all patches totally: a hybrid logical clock time
//! from `uhlc`, never behind any time its machine received, with the
//! machine's key breaking ties.

use std::fmt::{self, Write};
use std::time::{Duration, SystemTime};

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};
use uhlc::{HLC, HLCBuilder, ID, NTP64, Timestamp};

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

/// A machine's hybrid logical clock.
pub struct Clock {
    hlc: HLC,
    machine: MachineId,
}

impl Clock {
    /// A clock for `machine`, accepting received times up to `max_drift`
    /// ahead of its own.
    #[must_use]
    pub fn new(machine: MachineId, max_drift: Duration) -> Self {
        let mut id = [0u8; 16];
        id.copy_from_slice(&machine.as_bytes()[..16]);
        id[15] |= 1;
        let hlc = HLCBuilder::new()
            .with_id(ID::try_from(id).unwrap_or_else(|_| ID::rand()))
            .with_max_delta(max_drift)
            .build();
        Self { hlc, machine }
    }

    /// A new stamp, later than every stamp this clock made or received.
    #[must_use]
    pub fn stamp(&self) -> Stamp {
        Stamp {
            time: self.hlc.new_timestamp().get_time().0,
            machine: self.machine,
        }
    }

    /// Moves the clock past a received stamp.
    ///
    /// # Errors
    /// Fails when the stamp lies further ahead than the allowed drift,
    /// which means one of the two machines has a wrong clock.
    pub fn observe(&self, stamp: &Stamp) -> Result<(), String> {
        let received = Timestamp::new(NTP64(stamp.time), *self.hlc.get_id());
        self.hlc
            .update_with_timestamp(&received)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh_base::SecretKey;

    #[test]
    fn stamps_increase_and_follow_received_ones() {
        let machine = SecretKey::from_bytes(&[7; 32]).public();
        let clock = Clock::new(machine, Duration::from_secs(3600));
        let a = clock.stamp();
        let b = clock.stamp();
        assert!(a < b);
        let ahead = Stamp {
            time: b.time + (60 << 32),
            machine,
        };
        clock.observe(&ahead).unwrap();
        assert!(clock.stamp().time > ahead.time);
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
