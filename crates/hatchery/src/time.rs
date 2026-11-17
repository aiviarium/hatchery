//! Points in time as RFC 3339 UTC strings, and the short durations of
//! `min-age` (`3d`, `12h`).

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// Seconds since the Unix epoch, in UTC. Written as `2026-11-10T09:12:00Z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

/// Where the client reads the current time. Tests pass a fixed one.
pub type Clock = Arc<dyn Fn() -> Timestamp + Send + Sync>;

/// The system clock.
pub fn system_clock() -> Clock {
    Arc::new(Timestamp::now)
}

impl Timestamp {
    pub fn now() -> Timestamp {
        Timestamp::from(SystemTime::now())
    }

    pub fn from_secs(secs: i64) -> Timestamp {
        Timestamp(secs)
    }

    pub fn secs(self) -> i64 {
        self.0
    }

    pub fn plus(self, d: Duration) -> Timestamp {
        Timestamp(self.0.saturating_add(d.as_secs() as i64))
    }

    /// Parses `YYYY-MM-DDTHH:MM:SS` followed by `Z` or `+HH:MM`/`-HH:MM`,
    /// with optional fractional seconds, which are dropped.
    pub fn parse(s: &str) -> Result<Timestamp> {
        let bad = || Error::invalid("time", format!("{s:?} is not an RFC 3339 time"));
        let b = s.as_bytes();
        if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
            return Err(bad());
        }
        if b[13] != b':' || b[16] != b':' {
            return Err(bad());
        }
        let num = |r: std::ops::Range<usize>| -> Result<i64> {
            s.get(r)
                .filter(|p| p.bytes().all(|c| c.is_ascii_digit()))
                .and_then(|p| p.parse().ok())
                .ok_or_else(bad)
        };
        let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
            return Err(bad());
        }
        let mut rest = &s[19..];
        if let Some(frac) = rest.strip_prefix('.') {
            let digits = frac.bytes().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 {
                return Err(bad());
            }
            rest = &frac[digits..];
        }
        let offset = match rest {
            "Z" | "z" => 0,
            _ if rest.len() == 6
                && (rest.starts_with('+') || rest.starts_with('-'))
                && &rest[3..4] == ":" =>
            {
                let oh: i64 = rest[1..3].parse().map_err(|_| bad())?;
                let om: i64 = rest[4..6].parse().map_err(|_| bad())?;
                let o = oh * 3600 + om * 60;
                if rest.starts_with('-') { -o } else { o }
            }
            _ => return Err(bad()),
        };
        let days = days_from_civil(y, mo, d);
        Ok(Timestamp(days * 86_400 + h * 3600 + mi * 60 + se - offset))
    }
}

impl From<SystemTime> for Timestamp {
    fn from(t: SystemTime) -> Self {
        match t.duration_since(UNIX_EPOCH) {
            Ok(d) => Timestamp(d.as_secs() as i64),
            Err(e) => Timestamp(-(e.duration().as_secs() as i64)),
        }
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let days = self.0.div_euclid(86_400);
        let secs = self.0.rem_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        write!(
            f,
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            secs / 3600,
            secs % 3600 / 60,
            secs % 60
        )
    }
}

impl FromStr for Timestamp {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Timestamp::parse(s)
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Timestamp::parse(&s).map_err(serde::de::Error::custom)
    }
}

// Howard Hinnant's algorithms for the proleptic Gregorian calendar.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parses a duration such as `3d`, `12h`, `30m`, `45s`, `2w` or `0`.
pub fn parse_duration(s: &str) -> Result<Duration> {
    let s = s.trim();
    let bad = || {
        Error::invalid(
            "duration",
            format!("{s:?} is not a duration such as 3d, 12h or 30m"),
        )
    };
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).ok_or_else(bad)?);
    let n: u64 = num.parse().map_err(|_| bad())?;
    let secs = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return Err(bad()),
    };
    Ok(Duration::from_secs(n * secs))
}

/// Writes a duration in the largest unit that divides it.
pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    for (unit, size) in [("w", 7 * 86_400), ("d", 86_400), ("h", 3600), ("m", 60)] {
        if s != 0 && s.is_multiple_of(size) {
            return format!("{}{unit}", s / size);
        }
    }
    format!("{s}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_rfc3339() {
        for s in [
            "1970-01-01T00:00:00Z",
            "2026-11-10T09:12:00Z",
            "2000-02-29T23:59:59Z",
        ] {
            assert_eq!(Timestamp::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(Timestamp::parse("1970-01-01T00:00:00Z").unwrap().0, 0);
        assert_eq!(
            Timestamp::parse("2026-11-10T10:12:00.5+01:00").unwrap(),
            Timestamp::parse("2026-11-10T09:12:00Z").unwrap()
        );
        assert!(Timestamp::parse("2026-13-10T09:12:00Z").is_err());
        assert!(Timestamp::parse("yesterday").is_err());
    }

    #[test]
    fn parses_durations() {
        assert_eq!(
            parse_duration("3d").unwrap(),
            Duration::from_secs(3 * 86_400)
        );
        assert_eq!(parse_duration("0d").unwrap(), Duration::ZERO);
        assert_eq!(parse_duration("0").unwrap(), Duration::ZERO);
        assert!(parse_duration("3 days").is_err());
        assert_eq!(format_duration(Duration::from_secs(7 * 86_400)), "1w");
        assert_eq!(format_duration(Duration::from_secs(3 * 86_400)), "3d");
    }
}
