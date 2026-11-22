//! The calendar: what a count of time since 1601-01-01 says when it is
//! read as a date rather than counted.
//!
//! Apart from `time`, which is the machine's clock: the tick, the cycle
//! counter and the wall clock they are calibrated against. This module
//! reads no clock and takes no reading of its own. It is arithmetic
//! over a number it is handed, which is why the NT layer uses it
//! directly where `crate::time` has to come through `nt::platform`
//! instead (the gala-um seam, docs/GALA_UM.md).
//!
//! The days-to-civil algorithm is Howard Hinnant's, from
//! "chrono-Compatible Low-Level Date Algorithms" (public domain). The
//! kernel's one copy of it, so that the clock on a taskbar and a date
//! inside a program's own telemetry (`nt::etw::tracelogging`) cannot
//! disagree about what day it is.

use core::fmt;

/// A moment of the calendar in UTC.
pub struct Civil {
    pub year: u32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl Civil {
    /// Days since 1601-01-01 and seconds into that day.
    pub fn from_days(days: u64, second_of_day: u32) -> Self {
        // 134_774 days from 1601-01-01 to 1970-01-01, and 719_468 from
        // there to the algorithm's own era.
        let z = days as i64 - 134_774 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
        let year = (yoe + era * 400 + i64::from(month <= 2)).clamp(0, 9999) as u32;
        let second = second_of_day.min(24 * 60 * 60 - 1);
        Civil {
            year,
            month,
            day,
            hour: (second / 3600) as u8,
            minute: (second / 60 % 60) as u8,
            second: (second % 60) as u8,
        }
    }

    /// An NT system time: 100 ns units since 1601-01-01 UTC, which is
    /// what `system_time` counts and what a FILETIME holds.
    pub fn from_filetime(t: u64) -> Self {
        let secs = t / 10_000_000;
        Self::from_days(secs / 86_400, (secs % 86_400) as u32)
    }

    /// Minutes since 1601-01-01 UTC.
    pub fn from_minutes(minute: u64) -> Self {
        Self::from_days(minute / (60 * 24), ((minute % (60 * 24)) * 60) as u32)
    }
}

impl fmt::Display for Civil {
    /// The one order that reads the same everywhere, which is what a log
    /// a reader sorts and compares needs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_system_time_reads_as_the_day_it_is() {
        use super::Civil;
        // The moment `user/etwfields_test` writes into a FILETIME field,
        // which its scenario reads back as this date.
        assert_eq!(
            std::format!("{}", Civil::from_filetime(134_398_485_030_000_000)),
            "2026-11-22 19:15:03"
        );
        // The two epochs either end of the conversion.
        assert_eq!(std::format!("{}", Civil::from_filetime(0)), "1601-01-01 00:00:00");
        assert_eq!(
            std::format!("{}", Civil::from_filetime(11_644_473_600 * 10_000_000)),
            "1970-01-01 00:00:00"
        );
        // A leap day, and the century that is not one.
        assert_eq!(
            std::format!("{}", Civil::from_filetime(13_227_408_000 * 10_000_000)),
            "2020-02-29 00:00:00"
        );
        assert_eq!(
            std::format!("{}", Civil::from_filetime(9_440_582_400 * 10_000_000)),
            "1900-03-01 00:00:00"
        );
        // The last second of a day, and the first of the next.
        assert_eq!(
            std::format!("{}", Civil::from_filetime(13_227_407_999 * 10_000_000)),
            "2020-02-28 23:59:59"
        );
    }
}
