//! Timestamps as bdo writes, reads and shows them.
//!
//! Stored timestamps keep the form chrono's `to_rfc3339()` wrote
//! (`2026-11-18T03:04:05.123456789+00:00`): the tracking queries compare them
//! as strings, so a new form would put old and new rows on different sides of
//! a cutoff. Everything that writes a timestamp goes through here.

use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

/// The current instant.
pub fn now() -> Timestamp {
    Timestamp::now()
}

/// The instant `hours` before now.
pub fn hours_ago(hours: i64) -> Timestamp {
    now() - SignedDuration::from_hours(hours)
}

/// The instant `days` (of 24 hours) before now.
pub fn days_ago(days: i64) -> Timestamp {
    hours_ago(days.saturating_mul(24))
}

/// RFC 3339 in UTC with `+00:00`, and 0, 3, 6 or 9 fractional digits: the
/// form stored in the tracking DB, the trust store and the config.
pub fn rfc3339(at: Timestamp) -> String {
    let nanos = at.subsec_nanosecond().unsigned_abs();
    let fraction = if nanos == 0 {
        String::new()
    } else if nanos.is_multiple_of(1_000_000) {
        format!(".{:03}", nanos / 1_000_000)
    } else if nanos.is_multiple_of(1_000) {
        format!(".{:06}", nanos / 1_000)
    } else {
        format!(".{nanos:09}")
    };
    format!("{}{fraction}+00:00", at.strftime("%Y-%m-%dT%H:%M:%S"))
}

/// `rfc3339(now())`.
pub fn now_rfc3339() -> String {
    rfc3339(now())
}

/// UTC to the second, with no offset (`2026-11-18T03:04:05`). Compared as a
/// string, it sorts before every stored timestamp of the same second.
pub fn utc_seconds(at: Timestamp) -> String {
    at.strftime("%Y-%m-%dT%H:%M:%S").to_string()
}

/// UTC to the second, with `Z` (`2026-11-18T03:04:05Z`).
pub fn utc_seconds_z(at: Timestamp) -> String {
    at.strftime("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A timestamp with an offset (`Z`, `+00:00`, `+01:00`, ...).
pub fn parse(text: &str) -> Option<Timestamp> {
    text.parse().ok()
}

/// A timestamp with no offset, `T` or space separated, read as UTC.
pub fn parse_naive_utc(text: &str) -> Option<Timestamp> {
    resolve_naive(text, &TimeZone::UTC)
}

/// A timestamp with no offset, `T` separated, read on the local clock. In a
/// repeated hour it is the earlier of the two instants.
pub fn parse_naive_local(text: &str) -> Option<Timestamp> {
    resolve_naive(text, &TimeZone::system())
}

fn resolve_naive(text: &str, zone: &TimeZone) -> Option<Timestamp> {
    let civil = DateTime::strptime("%Y-%m-%dT%H:%M:%S", text)
        .or_else(|_| DateTime::strptime("%Y-%m-%d %H:%M:%S", text))
        .ok()?;
    zone.to_ambiguous_timestamp(civil).earlier().ok()
}

/// `at` on the local clock, in a `strftime` format.
pub fn local(at: Timestamp, format: &str) -> String {
    at.to_zoned_datetime(TimeZone::system())
        .strftime(format)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        parse(text).unwrap()
    }

    #[test]
    fn test_rfc3339_writes_what_chrono_wrote() {
        // chrono's `to_rfc3339()` for the same instants.
        for (instant, stored) in [
            ("2026-01-02T03:04:05Z", "2026-01-02T03:04:05+00:00"),
            ("2026-01-02T03:04:05.1Z", "2026-01-02T03:04:05.100+00:00"),
            (
                "2026-01-02T03:04:05.000120Z",
                "2026-01-02T03:04:05.000120+00:00",
            ),
            (
                "2026-01-02T03:04:05.123456789Z",
                "2026-01-02T03:04:05.123456789+00:00",
            ),
            (
                "2026-01-02T12:04:05.5+09:00",
                "2026-01-02T03:04:05.500+00:00",
            ),
        ] {
            assert_eq!(rfc3339(at(instant)), stored, "{instant}");
            assert_eq!(parse(stored), Some(at(instant)), "{stored}");
        }
    }

    #[test]
    fn test_second_forms() {
        let instant = at("2026-01-02T03:04:05.9+00:00");
        assert_eq!(utc_seconds(instant), "2026-01-02T03:04:05");
        assert_eq!(utc_seconds_z(instant), "2026-01-02T03:04:05Z");
    }

    #[test]
    fn test_parse_naive_utc_takes_both_separators_and_nothing_more() {
        let expected = Some(at("2026-01-02T03:04:05Z"));
        assert_eq!(parse_naive_utc("2026-01-02T03:04:05"), expected);
        assert_eq!(parse_naive_utc("2026-01-02 03:04:05"), expected);
        assert_eq!(parse_naive_utc("2026-01-02T03:04:05+00:00"), None);
        assert_eq!(parse_naive_utc("not a time"), None);
        assert_eq!(parse("2026-01-02T03:04:05"), None);
    }

    #[test]
    fn test_days_ago_is_whole_days() {
        let before = now();
        let gap = before.duration_since(days_ago(2)).as_secs();
        // `days_ago` reads the clock after `before`, so the gap is just under.
        assert!((2 * 86_400 - 5..=2 * 86_400).contains(&gap), "{gap}");
    }
}
