//! When an action runs. Times are the wall clock of this device, so a daily 02:00 stays 02:00
//! across a change of clock. A time that the clock skips runs when the skip ends, and a time that it
//! repeats runs at its first pass.

use chrono::{Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};

/// A scheduled run is late by at most this many seconds and still runs. A later one is skipped, because
/// the device was asleep or the app was closed.
pub const GRACE: i64 = 60;
/// A single run at a time is accepted this far ahead.
const MAX_AHEAD: u64 = 366 * 24 * 3600;

/// `day` counts from 0 on Sunday. `time` is `HH:MM`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Schedule {
    Once { at: u64 },
    Daily { time: String },
    Weekly { day: u8, time: String },
}

/// `HH:MM` with two digits each, and nothing else.
pub fn parse_time(text: &str) -> Option<NaiveTime> {
    let bytes = text.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return None;
    }
    let digits = |pair: &[u8]| {
        pair.iter()
            .all(u8::is_ascii_digit)
            .then(|| u32::from(pair[0] - b'0') * 10 + u32::from(pair[1] - b'0'))
    };
    NaiveTime::from_hms_opt(digits(&bytes[..2])?, digits(&bytes[3..])?, 0)
}

impl Schedule {
    pub fn validate(&self) -> Result<(), &'static str> {
        let valid = match self {
            Self::Once { at } => *at > 0,
            Self::Daily { time } => parse_time(time).is_some(),
            Self::Weekly { day, time } => *day <= 6 && parse_time(time).is_some(),
        };
        if valid {
            Ok(())
        } else {
            Err("scheduleInvalid")
        }
    }

    /// A schedule that a request may set: valid, and for a single run, still ahead and not too far.
    pub fn validate_new(&self, now: u64) -> Result<(), &'static str> {
        self.validate()?;
        match self {
            Self::Once { at } if *at <= now || *at > now + MAX_AHEAD => Err("scheduleInvalid"),
            _ => Ok(()),
        }
    }
}

/// The clock of this device, so the rules can be tested with a clock that has a change of hour.
pub trait Zone: Send + Sync + 'static {
    /// The wall clock at an instant.
    fn local(&self, unix: i64) -> Option<NaiveDateTime>;
    /// The instants that show this wall clock: none in a skipped hour, two in a repeated one.
    fn resolve(&self, local: NaiveDateTime) -> LocalResult<i64>;
}

pub struct System;

impl Zone for System {
    fn local(&self, unix: i64) -> Option<NaiveDateTime> {
        chrono::Local.timestamp_opt(unix, 0).single().map(|moment| moment.naive_local())
    }

    fn resolve(&self, local: NaiveDateTime) -> LocalResult<i64> {
        chrono::Local.from_local_datetime(&local).map(|moment| moment.timestamp())
    }
}

/// The instant that a wall-clock time has on a date.
fn instant(zone: &dyn Zone, date: NaiveDate, time: NaiveTime) -> Option<i64> {
    let wanted = date.and_time(time);
    match zone.resolve(wanted) {
        LocalResult::Single(moment) => Some(moment),
        LocalResult::Ambiguous(first, second) => Some(first.min(second)),
        // The clock skips this time. The first minute that exists is when the skip ends.
        LocalResult::None => (1..=180).find_map(|minutes| match zone.resolve(wanted + Duration::minutes(minutes)) {
            LocalResult::Single(moment) => Some(moment),
            LocalResult::Ambiguous(first, second) => Some(first.min(second)),
            LocalResult::None => None,
        }),
    }
}

/// The first run after an instant, or nothing when there is none.
pub fn next_after(schedule: &Schedule, after: i64, zone: &dyn Zone) -> Option<i64> {
    let (time, day) = match schedule {
        Schedule::Once { at } => return i64::try_from(*at).ok().filter(|at| *at > after),
        Schedule::Daily { time } => (parse_time(time)?, None),
        Schedule::Weekly { day, time } => (parse_time(time)?, Some(*day)),
    };
    let today = zone.local(after)?.date();
    // A skipped hour can move a run to the next day, so the scan starts a day early. A week and a day cover every weekday.
    (-1..=8).find_map(|offset| {
        let date = today.checked_add_signed(Duration::days(offset))?;
        if day.is_some_and(|day| date.weekday().num_days_from_sunday() != u32::from(day)) {
            return None;
        }
        instant(zone, date, time).filter(|moment| *moment > after)
    })
}

#[cfg(test)]
pub mod testing {
    use super::*;
    use chrono::{DateTime, NaiveDate};

    /// UTC, with no change of hour.
    pub struct Plain;

    impl Zone for Plain {
        fn local(&self, unix: i64) -> Option<NaiveDateTime> {
            DateTime::from_timestamp(unix, 0).map(|moment| moment.naive_utc())
        }

        fn resolve(&self, local: NaiveDateTime) -> LocalResult<i64> {
            LocalResult::Single(local.and_utc().timestamp())
        }
    }

    /// New York's rules for 2026: standard time is UTC-5, and summer time (UTC-4) runs from
    /// 2026-03-08 02:00 to 2026-11-01 02:00 local. The hour 02:00 on the first date does not exist,
    /// and the hour 01:00 on the second one happens twice.
    pub struct Dst;

    fn utc(year: i32, month: u32, day: u32, hour: u32) -> i64 {
        NaiveDate::from_ymd_opt(year, month, day).unwrap().and_hms_opt(hour, 0, 0).unwrap().and_utc().timestamp()
    }

    impl Dst {
        fn offset(unix: i64) -> i64 {
            if (utc(2026, 3, 8, 7)..utc(2026, 11, 1, 6)).contains(&unix) {
                -4 * 3600
            } else {
                -5 * 3600
            }
        }
    }

    impl Zone for Dst {
        fn local(&self, unix: i64) -> Option<NaiveDateTime> {
            DateTime::from_timestamp(unix + Self::offset(unix), 0).map(|moment| moment.naive_utc())
        }

        fn resolve(&self, local: NaiveDateTime) -> LocalResult<i64> {
            let base = local.and_utc().timestamp();
            let mut found: Vec<i64> = [-5 * 3600, -4 * 3600]
                .into_iter()
                .map(|offset| base - offset)
                .filter(|unix| Self::offset(*unix) == base - unix)
                .collect();
            found.sort_unstable();
            match found.as_slice() {
                [] => LocalResult::None,
                [one] => LocalResult::Single(*one),
                [first, second] => LocalResult::Ambiguous(*first, *second),
                _ => unreachable!(),
            }
        }
    }

    /// A unix time from a New York wall clock, for a time that exists once.
    pub fn ny(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        let local = NaiveDate::from_ymd_opt(year, month, day).unwrap().and_hms_opt(hour, minute, 0).unwrap();
        match Dst.resolve(local) {
            LocalResult::Single(moment) => moment,
            other => panic!("{local} is not a single time: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::*, *};
    use chrono::NaiveDate;

    fn daily(time: &str) -> Schedule {
        Schedule::Daily { time: time.into() }
    }

    fn weekly(day: u8, time: &str) -> Schedule {
        Schedule::Weekly { day, time: time.into() }
    }

    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        NaiveDate::from_ymd_opt(year, month, day).unwrap().and_hms_opt(hour, minute, 0).unwrap().and_utc().timestamp()
    }

    #[test]
    fn times_are_two_digits_on_the_clock() {
        for good in ["00:00", "02:30", "23:59"] {
            assert!(parse_time(good).is_some(), "{good}");
        }
        for bad in ["", "2:30", "24:00", "12:60", "12-30", "12:3", "1230", "12:30:00", "ab:cd", " 2:30", "+1:30", "٠٢:٣٠"] {
            assert!(parse_time(bad).is_none(), "{bad}");
        }
        assert!(weekly(6, "01:00").validate().is_ok());
        assert!(weekly(7, "01:00").validate().is_err());
        assert!(daily("25:00").validate().is_err());
    }

    #[test]
    fn a_single_run_must_be_ahead_and_not_far() {
        let now = 1_800_000_000;
        assert!(Schedule::Once { at: now + 60 }.validate_new(now).is_ok());
        assert!(Schedule::Once { at: now }.validate_new(now).is_err());
        assert!(Schedule::Once { at: now - 5 }.validate_new(now).is_err());
        assert!(Schedule::Once { at: now + MAX_AHEAD + 1 }.validate_new(now).is_err());
    }

    #[test]
    fn a_daily_run_is_the_next_time_on_the_clock() {
        let zone = Plain;
        let two = daily("02:00");
        assert_eq!(next_after(&two, utc(2026, 11, 23, 1, 0), &zone), Some(utc(2026, 11, 23, 2, 0)));
        // At the time itself the next one is tomorrow, so a run is never found twice.
        assert_eq!(next_after(&two, utc(2026, 11, 23, 2, 0), &zone), Some(utc(2026, 11, 24, 2, 0)));
        assert_eq!(next_after(&two, utc(2026, 11, 23, 23, 59), &zone), Some(utc(2026, 11, 24, 2, 0)));
        // Across a month and a year.
        assert_eq!(next_after(&two, utc(2026, 12, 31, 5, 0), &zone), Some(utc(2027, 1, 1, 2, 0)));
    }

    #[test]
    fn a_weekly_run_waits_for_its_day() {
        let zone = Plain;
        // 2026-11-23 is a Monday. Day 1 is Monday and day 0 is Sunday.
        let monday = weekly(1, "09:00");
        assert_eq!(next_after(&monday, utc(2026, 11, 23, 8, 0), &zone), Some(utc(2026, 11, 23, 9, 0)));
        assert_eq!(next_after(&monday, utc(2026, 11, 23, 9, 0), &zone), Some(utc(2026, 11, 30, 9, 0)));
        let sunday = weekly(0, "23:30");
        assert_eq!(next_after(&sunday, utc(2026, 11, 23, 8, 0), &zone), Some(utc(2026, 11, 29, 23, 30)));
        let saturday = weekly(6, "00:00");
        assert_eq!(next_after(&saturday, utc(2026, 11, 28, 0, 0), &zone), Some(utc(2026, 12, 5, 0, 0)));
    }

    #[test]
    fn a_single_run_has_no_next_after_it() {
        let once = Schedule::Once { at: 1_000 };
        assert_eq!(next_after(&once, 999, &Plain), Some(1_000));
        assert_eq!(next_after(&once, 1_000, &Plain), None);
    }

    #[test]
    fn a_time_that_the_clock_skips_runs_when_the_skip_ends() {
        // 2026-03-08: at 02:00 New York jumps to 03:00. 02:30 does not exist that day.
        let skipped = daily("02:30");
        let before = ny(2026, 3, 8, 1, 0);
        assert_eq!(next_after(&skipped, before, &Dst), Some(ny(2026, 3, 8, 3, 0)));
        // The day after, and the day before, the time is the plain one.
        assert_eq!(next_after(&skipped, ny(2026, 3, 8, 3, 0), &Dst), Some(ny(2026, 3, 9, 2, 30)));
        assert_eq!(next_after(&skipped, ny(2026, 3, 6, 12, 0), &Dst), Some(ny(2026, 3, 7, 2, 30)));
        // A daily time after the skip keeps its wall clock: 08:00 is 12:00 UTC before and 12:00 UTC... one hour earlier after.
        let morning = daily("08:00");
        assert_eq!(next_after(&morning, ny(2026, 3, 7, 12, 0), &Dst), Some(ny(2026, 3, 8, 8, 0)));
        assert_eq!(ny(2026, 3, 8, 8, 0) - ny(2026, 3, 7, 8, 0), 23 * 3600);
    }

    #[test]
    fn a_time_that_the_clock_repeats_runs_once_at_its_first_pass() {
        // 2026-11-01: at 02:00 summer time New York goes back to 01:00. 01:30 happens twice.
        let repeated = daily("01:30");
        let first = next_after(&repeated, ny(2026, 10, 31, 12, 0), &Dst).unwrap();
        assert_eq!(first, utc(2026, 11, 1, 5, 30));
        // After the first pass, the second pass an hour later is not a run: the next one is tomorrow.
        assert_eq!(next_after(&repeated, first, &Dst), Some(ny(2026, 11, 2, 1, 30)));
        assert_eq!(next_after(&repeated, first + 3600, &Dst), Some(ny(2026, 11, 2, 1, 30)));
        assert_eq!(ny(2026, 11, 2, 1, 30) - first, 25 * 3600);
    }

    #[test]
    fn a_weekly_run_across_a_change_of_clock_keeps_its_wall_time() {
        // Sunday 2026-03-08 has the skip. A weekly Sunday 09:00 is 09:00 on both sides.
        let sunday = weekly(0, "09:00");
        let before = next_after(&sunday, ny(2026, 3, 1, 10, 0), &Dst).unwrap();
        assert_eq!(before, ny(2026, 3, 8, 9, 0));
        assert_eq!(next_after(&sunday, before, &Dst), Some(ny(2026, 3, 15, 9, 0)));
        assert_eq!(ny(2026, 3, 15, 9, 0) - before, 7 * 24 * 3600);
    }

    #[test]
    fn the_test_clock_models_the_two_changes() {
        assert_eq!(Dst.resolve(NaiveDate::from_ymd_opt(2026, 3, 8).unwrap().and_hms_opt(2, 30, 0).unwrap()), LocalResult::None);
        assert!(matches!(
            Dst.resolve(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap().and_hms_opt(1, 30, 0).unwrap()),
            LocalResult::Ambiguous(_, _)
        ));
    }
}
