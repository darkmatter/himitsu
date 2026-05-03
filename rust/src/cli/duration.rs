//! Parse user-supplied expiration expressions into absolute timestamps.
//!
//! Accepted forms:
//!   * RFC 3339 timestamp: `2026-12-31T00:00:00Z`
//!   * Relative durations: `30d`, `12h`, `90d`, `1y`, `6mo`
//!   * The literal `never` — sentinel used by the CLI to clear expiry.

use chrono::{DateTime, Duration, Months, Utc};

use crate::error::{HimitsuError, Result};

/// Outcome of parsing an `--expires-at` argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpiresAt {
    /// Clear any existing expiry.
    Never,
    /// Resolved absolute instant.
    At(DateTime<Utc>),
}

/// Parse an expires-at expression relative to `now`.
///
/// Pure function for testability — `parse` delegates here with `Utc::now()`.
pub fn parse_with_now(input: &str, now: DateTime<Utc>) -> Result<ExpiresAt> {
    let s = input.trim();
    if s.is_empty() {
        return Err(HimitsuError::InvalidReference(
            "expires-at value is empty".into(),
        ));
    }

    if s.eq_ignore_ascii_case("never") {
        return Ok(ExpiresAt::Never);
    }

    // Try RFC 3339 first — must contain at least one digit and a dash to
    // reasonably look like a timestamp, otherwise we'd waste a parse attempt
    // on obvious duration strings like "30d".
    if looks_like_timestamp(s) {
        return DateTime::parse_from_rfc3339(s)
            .map(|dt| ExpiresAt::At(dt.with_timezone(&Utc)))
            .map_err(|e| {
                HimitsuError::InvalidReference(format!("invalid RFC 3339 timestamp '{s}': {e}"))
            });
    }

    let dur = parse_relative(s)?;
    match dur {
        RelativeDuration::Fixed(d) => Ok(ExpiresAt::At(now + d)),
        RelativeDuration::Months(m) => {
            let bumped = now.checked_add_months(Months::new(m)).ok_or_else(|| {
                HimitsuError::InvalidReference(format!("duration '{s}' overflows"))
            })?;
            Ok(ExpiresAt::At(bumped))
        }
        RelativeDuration::Years(y) => {
            let months = y.checked_mul(12).ok_or_else(|| {
                HimitsuError::InvalidReference(format!("duration '{s}' overflows"))
            })?;
            let bumped = now.checked_add_months(Months::new(months)).ok_or_else(|| {
                HimitsuError::InvalidReference(format!("duration '{s}' overflows"))
            })?;
            Ok(ExpiresAt::At(bumped))
        }
    }
}

/// Parse an expires-at expression relative to the current wall clock.
pub fn parse(input: &str) -> Result<ExpiresAt> {
    parse_with_now(input, Utc::now())
}

/// Format a remaining duration for human display.
///
/// Returns a `(message, severity)` tuple.
pub fn describe_remaining(
    expires_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> (String, ExpirySeverity) {
    let delta = expires_at.signed_duration_since(now);
    if delta < Duration::zero() {
        let ago = -delta;
        let days = ago.num_days();
        let msg = if days >= 1 {
            format!("EXPIRED {days} day{} ago", plural(days))
        } else {
            let hours = ago.num_hours().max(1);
            format!("EXPIRED {hours} hour{} ago", plural(hours))
        };
        return (msg, ExpirySeverity::Expired);
    }

    let days = delta.num_days();
    let msg = if days >= 1 {
        format!("expires in {days} day{}", plural(days))
    } else {
        let hours = delta.num_hours().max(1);
        format!("expires in {hours} hour{}", plural(hours))
    };
    let severity = if days <= 30 {
        ExpirySeverity::Soon
    } else {
        ExpirySeverity::Distant
    };
    (msg, severity)
}

/// Severity bucket used for coloring the expiry line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpirySeverity {
    Distant,
    Soon,
    Expired,
}

// ── Internals ──────────────────────────────────────────────────────────────

enum RelativeDuration {
    Fixed(Duration),
    Months(u32),
    Years(u32),
}

fn plural(n: i64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn looks_like_timestamp(s: &str) -> bool {
    // RFC 3339 always has at least one `-` for the date and a `T` or space
    // separator. This is a cheap heuristic to avoid parsing e.g. "30d" as
    // a timestamp and generating a confusing error.
    s.contains('-') && (s.contains('T') || s.contains(' '))
}

fn parse_relative(s: &str) -> Result<RelativeDuration> {
    // Split numeric prefix from unit suffix.
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| HimitsuError::InvalidReference(format!("missing unit in '{s}'")))?;

    if split == 0 {
        return Err(HimitsuError::InvalidReference(format!(
            "expected leading integer in duration '{s}'"
        )));
    }

    let (num_str, unit) = s.split_at(split);
    let n: i64 = num_str.parse().map_err(|_| {
        HimitsuError::InvalidReference(format!("invalid integer in duration '{s}'"))
    })?;
    if n < 0 {
        return Err(HimitsuError::InvalidReference(format!(
            "duration must be non-negative: '{s}'"
        )));
    }

    match unit {
        "s" => Ok(RelativeDuration::Fixed(Duration::seconds(n))),
        "m" => Ok(RelativeDuration::Fixed(Duration::minutes(n))),
        "h" => Ok(RelativeDuration::Fixed(Duration::hours(n))),
        "d" => Ok(RelativeDuration::Fixed(Duration::days(n))),
        "w" => Ok(RelativeDuration::Fixed(Duration::weeks(n))),
        "mo" => {
            let m = u32::try_from(n).map_err(|_| {
                HimitsuError::InvalidReference(format!("month count too large: '{s}'"))
            })?;
            Ok(RelativeDuration::Months(m))
        }
        "y" => {
            let y = u32::try_from(n).map_err(|_| {
                HimitsuError::InvalidReference(format!("year count too large: '{s}'"))
            })?;
            Ok(RelativeDuration::Years(y))
        }
        other => Err(HimitsuError::InvalidReference(format!(
            "unknown duration unit '{other}' in '{s}' (expected s/m/h/d/w/mo/y)"
        ))),
    }
}

/// Convert a chrono `DateTime<Utc>` into a protobuf Timestamp.
pub fn to_proto_timestamp(dt: DateTime<Utc>) -> pbjson_types::Timestamp {
    pbjson_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

/// Convert a protobuf Timestamp into a chrono `DateTime<Utc>`.
pub fn from_proto_timestamp(ts: &pbjson_types::Timestamp) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(ts.seconds, ts.nanos as u32)
}

/// Does a timestamp look unset (zero seconds and nanos)? Protobuf default.
pub fn is_unset(ts: &pbjson_types::Timestamp) -> bool {
    ts.seconds == 0 && ts.nanos == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    fn epoch_plus(days: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(86_400 * days, 0).unwrap()
    }

    #[test]
    fn parse_days() {
        let now = epoch_plus(100);
        let r = parse_with_now("30d", now).unwrap();
        match r {
            ExpiresAt::At(dt) => assert_eq!(dt, epoch_plus(130)),
            _ => panic!("expected At"),
        }
    }

    #[test]
    fn parse_months() {
        let now = DateTime::parse_from_rfc3339("2026-01-15T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let r = parse_with_now("6mo", now).unwrap();
        match r {
            ExpiresAt::At(dt) => {
                assert_eq!(dt.year(), 2026);
                assert_eq!(dt.month(), 7);
                assert_eq!(dt.day(), 15);
            }
            _ => panic!("expected At"),
        }
    }

    #[test]
    fn parse_years() {
        let now = DateTime::parse_from_rfc3339("2026-04-12T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let r = parse_with_now("1y", now).unwrap();
        match r {
            ExpiresAt::At(dt) => {
                assert_eq!(dt.year(), 2027);
                assert_eq!(dt.month(), 4);
                assert_eq!(dt.day(), 12);
            }
            _ => panic!("expected At"),
        }
    }

    #[test]
    fn parse_never_literal() {
        assert_eq!(
            parse_with_now("never", epoch_plus(0)).unwrap(),
            ExpiresAt::Never
        );
        assert_eq!(
            parse_with_now("NEVER", epoch_plus(0)).unwrap(),
            ExpiresAt::Never
        );
    }

    #[test]
    fn parse_rfc3339() {
        let r = parse_with_now("2026-12-31T00:00:00Z", epoch_plus(0)).unwrap();
        match r {
            ExpiresAt::At(dt) => {
                assert_eq!(dt.year(), 2026);
                assert_eq!(dt.month(), 12);
                assert_eq!(dt.day(), 31);
            }
            _ => panic!("expected At"),
        }
    }

    #[test]
    fn parse_invalid_values() {
        assert!(parse_with_now("", epoch_plus(0)).is_err());
        assert!(parse_with_now("garbage", epoch_plus(0)).is_err());
        assert!(parse_with_now("30x", epoch_plus(0)).is_err());
        assert!(parse_with_now("-5d", epoch_plus(0)).is_err());
        assert!(parse_with_now("2026-13-99T00:00:00Z", epoch_plus(0)).is_err());
    }

    #[test]
    fn describe_distant_future() {
        let now = epoch_plus(0);
        let future = epoch_plus(90);
        let (msg, sev) = describe_remaining(future, now);
        assert_eq!(msg, "expires in 90 days");
        assert_eq!(sev, ExpirySeverity::Distant);
    }

    #[test]
    fn describe_soon() {
        let now = epoch_plus(0);
        let future = epoch_plus(12);
        let (msg, sev) = describe_remaining(future, now);
        assert_eq!(msg, "expires in 12 days");
        assert_eq!(sev, ExpirySeverity::Soon);
    }

    #[test]
    fn describe_expired() {
        let now = epoch_plus(10);
        let past = epoch_plus(7);
        let (msg, sev) = describe_remaining(past, now);
        assert_eq!(msg, "EXPIRED 3 days ago");
        assert_eq!(sev, ExpirySeverity::Expired);
    }

    #[test]
    fn proto_timestamp_roundtrip() {
        let now = DateTime::parse_from_rfc3339("2026-04-12T10:20:30Z")
            .unwrap()
            .with_timezone(&Utc);
        let ts = to_proto_timestamp(now);
        let back = from_proto_timestamp(&ts).unwrap();
        assert_eq!(back, now);
    }
}
