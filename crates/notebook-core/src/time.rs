//! Calendar formatting of [`Timestamp`]s in UTC, shared by backup and export
//! file names.
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

/// `YYYYMMDD-HHMMSS` in UTC, the time stamp in backup and export names.
pub fn stamp(ts: Timestamp) -> String {
    let c = civil(ts);
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        c.year, c.month, c.day, c.hour, c.minute, c.second
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn civil_ms(ts: i64) -> (i64, u32, u32, u32, u32, u32, u32) {
        let c = civil(Timestamp(ts));
        (c.year, c.month, c.day, c.hour, c.minute, c.second, c.millis)
    }

    #[test]
    fn stamps() {
        // 2026-10-22T14:03:09Z, the backup naming example.
        assert_eq!(stamp(Timestamp(1_792_677_789_000)), "20261022-140309");
        // Milliseconds are dropped, not rounded.
        assert_eq!(stamp(Timestamp(1_792_677_789_999)), "20261022-140309");
        assert_eq!(stamp(Timestamp(0)), "19700101-000000");
    }

    #[test]
    fn leap_day() {
        // 2024-02-29T23:59:59.999Z, then the next millisecond.
        assert_eq!(civil_ms(1_709_251_199_999), (2024, 2, 29, 23, 59, 59, 999));
        assert_eq!(civil_ms(1_709_251_200_000), (2024, 3, 1, 0, 0, 0, 0));
        // 2000 is a leap year (divisible by 400).
        assert_eq!(civil_ms(951_782_400_000), (2000, 2, 29, 0, 0, 0, 0));
    }

    #[test]
    fn negative_timestamp() {
        assert_eq!(civil_ms(-1), (1969, 12, 31, 23, 59, 59, 999));
        // 1900 is not a leap year: Feb 28 is followed by Mar 1.
        assert_eq!(civil_ms(-2_208_988_800_000), (1900, 1, 1, 0, 0, 0, 0));
        assert_eq!(civil_ms(-2_203_891_200_000), (1900, 3, 1, 0, 0, 0, 0));
    }
}
