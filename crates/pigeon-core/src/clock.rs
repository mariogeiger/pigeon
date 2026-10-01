//! Timestamps that order all patches totally: a hybrid logical clock time
//! from `uhlc`, never behind any time its machine received, with the
//! machine's key breaking ties.

use std::fmt;
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
        NTP64(self.time).to_string_rfc3339_lossy()
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

/// Reads an RFC 3339 time, such as `2026-10-01T12:00:00Z`, as NTP64.
///
/// # Errors
///
/// Returns why the text is not an RFC 3339 time.
pub fn parse_rfc3339(text: &str) -> Result<u64, String> {
    NTP64::parse_rfc3339(text)
        .map(|time| time.0)
        .map_err(|error| format!("{text:?} is not an RFC 3339 time: {error:?}"))
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
}
