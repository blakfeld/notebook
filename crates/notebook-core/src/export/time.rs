//! Calendar formatting of [`Timestamp`]s for export, in UTC.
//!
//! Core has no date library; this converts Unix milliseconds to a civil date
//! with Howard Hinnant's `civil_from_days` algorithm, which is exact for the
//! proleptic Gregorian calendar over the whole `i64` day range used here.

use crate::note::Timestamp;

const MS_PER_DAY: i64 = 86_400_000;

/// A UTC calendar date and time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Civil {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub millis: u32,
}

pub(crate) fn civil(ts: Timestamp) -> Civil {
    let ms = ts.as_millis();
    let days = ms.div_euclid(MS_PER_DAY);
    // In [0, MS_PER_DAY), so it fits a u32.
    let time = ms.rem_euclid(MS_PER_DAY) as u32;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    // Both are small and non-negative: day in [1, 31], month in [1, 12].
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);

    Civil {
        year,
        month,
        day,
        hour: time / 3_600_000,
        minute: time / 60_000 % 60,
        second: time / 1000 % 60,
        millis: time % 1000,
    }
}

/// `YYYY-MM-DDTHH:MM:SS.sssZ`, as written in the frontmatter.
pub(crate) fn rfc3339(ts: Timestamp) -> String {
    let c = civil(ts);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        c.year, c.month, c.day, c.hour, c.minute, c.second, c.millis
    )
}

/// `YYYYMMDD-HHMMSS`, the export folder's time stamp.
pub(crate) fn folder_stamp(ts: Timestamp) -> String {
    let c = civil(ts);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_examples() {
        assert_eq!(
            rfc3339(Timestamp(1_729_519_385_123)),
            "2024-10-21T14:03:05.123Z"
        );
        assert_eq!(
            rfc3339(Timestamp(1_729_605_785_000)),
            "2024-10-22T14:03:05.000Z"
        );
        // 2026-10-21 14:03:05 UTC.
        assert_eq!(
            folder_stamp(Timestamp(1_792_591_385_000)),
            "20261021-140305"
        );
    }

    #[test]
    fn unix_epoch() {
        assert_eq!(rfc3339(Timestamp(0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(folder_stamp(Timestamp(0)), "19700101-000000");
    }

    #[test]
    fn leap_day() {
        // 2024-02-29T23:59:59.999Z, then the next millisecond.
        assert_eq!(
            rfc3339(Timestamp(1_709_251_199_999)),
            "2024-02-29T23:59:59.999Z"
        );
        assert_eq!(
            rfc3339(Timestamp(1_709_251_200_000)),
            "2024-03-01T00:00:00.000Z"
        );
        // 2000 is a leap year (divisible by 400).
        assert_eq!(
            rfc3339(Timestamp(951_782_400_000)),
            "2000-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn negative_timestamp() {
        assert_eq!(rfc3339(Timestamp(-1)), "1969-12-31T23:59:59.999Z");
        // 1900-01-01T00:00:00Z; 1900 is not a leap year.
        assert_eq!(
            rfc3339(Timestamp(-2_208_988_800_000)),
            "1900-01-01T00:00:00.000Z"
        );
        assert_eq!(
            rfc3339(Timestamp(-2_203_891_200_000)),
            "1900-03-01T00:00:00.000Z"
        );
    }
}
