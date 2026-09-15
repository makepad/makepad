//! Civil dates as a day number.
//!
//! A date is an [`i32`] count of days from 1970-01-01: no clocks, no zones,
//! no leap seconds. It sorts, subtracts and indexes into a month bucket
//! without a calendar library, and is four bytes in a row of a hundred
//! thousand.
//!
//! Conversion is Howard Hinnant's `days_from_civil` / `civil_from_days`,
//! exact for the whole proleptic Gregorian calendar.

use std::fmt;

/// Days since 1970-01-01. Negative reaches back before it.
pub type Day = i32;

/// A month as a sortable integer key, `year * 12 + (month - 1)` — what
/// budgets and monthly rollups are keyed by.
pub type MonthKey = i32;

/// Days from the civil date. Howard Hinnant's `days_from_civil`, which is
/// exact for the whole proleptic Gregorian calendar.
pub fn from_ymd(year: i32, month: u32, day: u32) -> Day {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64; // [0, 399]
    let mp = ((month as i64 + 9) % 12) as i64; // Mar = 0
    let doy = (153 * mp + 2) / 5 + day as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    (era as i64 * 146097 + doe - 719468) as Day
}

/// The civil date of a day number.
pub fn to_ymd(day: Day) -> (i32, u32, u32) {
    let z = day as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], Mar = 0
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    ((if m <= 2 { y + 1 } else { y }) as i32, m as u32, d as u32)
}

/// The Gregorian year of `day`.
pub fn year_of(day: Day) -> i32 {
    to_ymd(day).0
}

/// The Gregorian month of `day`, 1–12.
pub fn month_of(day: Day) -> u32 {
    to_ymd(day).1
}

/// 0 = Monday. (1970-01-01 was a Thursday.)
pub fn weekday(day: Day) -> u32 {
    (day.rem_euclid(7) as u32 + 3) % 7
}

/// Saturday or Sunday (`weekday` 5 or 6).
pub fn is_weekend(day: Day) -> bool {
    weekday(day) >= 5
}

/// Days in the Gregorian month: 28–31, with the leap-year rule for February.
pub fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 30,
    }
}

/// First day of the month containing `day`.
pub fn month_start(day: Day) -> Day {
    let (y, m, _) = to_ymd(day);
    from_ymd(y, m, 1)
}

/// Last day of the month containing `day`.
pub fn month_end(day: Day) -> Day {
    let (y, m, _) = to_ymd(day);
    from_ymd(y, m, days_in_month(y, m))
}

/// Move whole months, clamping the day of month — 31 January plus one
/// month is 28 February, which is what a monthly bill on the 31st does.
pub fn add_months(day: Day, months: i32) -> Day {
    let (y, m, d) = to_ymd(day);
    let total = y * 12 + (m as i32 - 1) + months;
    let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    from_ymd(ny, nm, d.min(days_in_month(ny, nm)))
}

/// Move whole days. Negative goes backward.
pub fn add_days(day: Day, days: i32) -> Day {
    day + days
}

/// The month containing `day` as a sortable key.
pub fn month_key(day: Day) -> MonthKey {
    let (y, m, _) = to_ymd(day);
    y * 12 + (m as i32 - 1)
}

/// First day of the month identified by `key`.
pub fn month_key_start(key: MonthKey) -> Day {
    from_ymd(key.div_euclid(12), key.rem_euclid(12) as u32 + 1, 1)
}

/// ISO-8601 week number (1–53). Week 1 contains the year's first Thursday;
/// weeks start on Monday. A date in early January can belong to week 52 or
/// 53 of the previous year.
pub fn iso_week(day: Day) -> u32 {
    let thursday = day - weekday(day) as Day + 3;
    let (year, _, _) = to_ymd(thursday);
    let jan1 = from_ymd(year, 1, 1);
    ((thursday - jan1) / 7 + 1) as u32
}

/// English full month names, January first.
pub const MONTH_NAMES: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August",
    "September", "October", "November", "December",
];

/// English abbreviated month names, Jan first.
pub const MONTH_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// English full weekday names, Monday first.
pub const WEEKDAY_NAMES: [&str; 7] = [
    "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday",
];

/// English abbreviated weekdays, Monday first.
pub const WEEKDAY_ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// English weekday name. `0` is Monday, matching [`weekday`].
pub fn weekday_name(weekday: u32) -> &'static str {
    WEEKDAY_NAMES[(weekday % 7) as usize]
}

/// English month name. `1` is January.
pub fn month_name(month: u32) -> &'static str {
    MONTH_NAMES[(month.saturating_sub(1) % 12) as usize]
}

/// `2024-03-04` — the storage form, and the only unambiguous one.
pub fn format_iso(day: Day) -> String {
    let (y, m, d) = to_ymd(day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `4 Mar 2024` — the ledger form: unambiguous to a human, and short.
pub fn format_short(day: Day) -> String {
    let (y, m, d) = to_ymd(day);
    format!("{d} {} {y}", MONTH_ABBR[(m - 1) as usize])
}

/// `Mar 2024` — column headers on a budget.
pub fn format_month(key: MonthKey) -> String {
    let year = key.div_euclid(12);
    let month = key.rem_euclid(12) as usize;
    format!("{} {year}", MONTH_ABBR[month])
}

/// Parse `YYYY-MM-DD`. Rejects impossible dates rather than clamping.
pub fn parse_iso(text: &str) -> Option<Day> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if !bytes[..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let year: i32 = text[..4].parse().ok()?;
    let month: u32 = text[5..7].parse().ok()?;
    let day: u32 = text[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    Some(from_ymd(year, month, day))
}

/// One cell of a [`month_grid`]: the civil day, and whether it falls in
/// the requested month.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridDay {
    pub day: Day,
    /// True when this cell is in the month passed to [`month_grid`].
    pub in_month: bool,
}

/// A six-week, seven-column calendar page for `year`/`month`.
///
/// `first_weekday` is the weekday that occupies column 0, with the same
/// numbering as [`weekday`]: 0 = Monday. Days outside the month fill the
/// leading and trailing cells so the grid is always 6×7.
pub fn month_grid(year: i32, month: u32, first_weekday: u32) -> [[GridDay; 7]; 6] {
    let first_weekday = first_weekday % 7;
    let start = from_ymd(year, month, 1);
    let offset = (weekday(start) + 7 - first_weekday) % 7;
    let origin = start - offset as Day;
    let mut grid = [[GridDay { day: 0, in_month: false }; 7]; 6];
    for row in 0..6 {
        for col in 0..7 {
            let day = origin + (row * 7 + col) as Day;
            let (y, m, _) = to_ymd(day);
            grid[row][col] = GridDay {
                day,
                in_month: y == year && m == month,
            };
        }
    }
    grid
}

/// A closed range of days, which is what every report and filter is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateRange {
    pub start: Day,
    pub end: Day,
}

impl DateRange {
    /// Inclusive: `start` and `end` both count.
    pub fn contains(&self, day: Day) -> bool {
        day >= self.start && day <= self.end
    }

    /// Inclusive length in days.
    pub fn days(&self) -> i32 {
        self.end - self.start + 1
    }

    /// The whole calendar month identified by `key`.
    pub fn month(key: MonthKey) -> DateRange {
        let start = month_key_start(key);
        DateRange { start, end: month_end(start) }
    }

    /// The last `n` whole months ending with the month of `day`.
    pub fn last_months(day: Day, n: i32) -> DateRange {
        let end = month_end(day);
        let start = month_start(add_months(day, -(n - 1)));
        DateRange { start, end }
    }

    /// 1 January through 31 December of `year`.
    pub fn year(year: i32) -> DateRange {
        DateRange { start: from_ymd(year, 1, 1), end: from_ymd(year, 12, 31) }
    }
}

impl fmt::Display for DateRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} – {}", format_short(self.start), format_short(self.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_conversion_round_trips_across_centuries() {
        assert_eq!(from_ymd(1970, 1, 1), 0);
        assert_eq!(to_ymd(0), (1970, 1, 1));
        assert_eq!(from_ymd(2024, 2, 29), 19782);
        assert_eq!(to_ymd(19782), (2024, 2, 29));
        assert_eq!(from_ymd(1969, 12, 31), -1);
        assert_eq!(to_ymd(-1), (1969, 12, 31));
        // Every day of a leap year and a century year round trips.
        for day in from_ymd(1999, 1, 1)..=from_ymd(2001, 12, 31) {
            let (y, m, d) = to_ymd(day);
            assert_eq!(from_ymd(y, m, d), day);
        }
    }

    #[test]
    fn weekday_and_month_edges() {
        assert_eq!(weekday(from_ymd(1970, 1, 1)), 3); // Thursday
        assert_eq!(weekday(from_ymd(2024, 3, 4)), 0); // Monday
        assert!(is_weekend(from_ymd(2024, 3, 9)));
        assert_eq!(month_start(from_ymd(2024, 3, 15)), from_ymd(2024, 3, 1));
        assert_eq!(month_end(from_ymd(2024, 2, 15)), from_ymd(2024, 2, 29));
        assert_eq!(days_in_month(2023, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
    }

    #[test]
    fn leap_years_and_month_ends() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2024, 1), 31);
        assert_eq!(days_in_month(2024, 4), 30);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2400, 2), 29);
        assert_eq!(month_end(from_ymd(2023, 2, 1)), from_ymd(2023, 2, 28));
        assert_eq!(month_end(from_ymd(2024, 2, 1)), from_ymd(2024, 2, 29));
        assert_eq!(month_end(from_ymd(2024, 1, 31)), from_ymd(2024, 1, 31));
        assert_eq!(month_end(from_ymd(2024, 4, 1)), from_ymd(2024, 4, 30));
        assert_eq!(to_ymd(add_days(from_ymd(2024, 2, 28), 1)), (2024, 2, 29));
        assert_eq!(to_ymd(add_days(from_ymd(2023, 2, 28), 1)), (2023, 3, 1));
    }

    #[test]
    fn monthly_bills_clamp_to_the_end_of_short_months() {
        let jan31 = from_ymd(2024, 1, 31);
        assert_eq!(to_ymd(add_months(jan31, 1)), (2024, 2, 29));
        assert_eq!(to_ymd(add_months(jan31, 13)), (2025, 2, 28));
        assert_eq!(to_ymd(add_months(from_ymd(2024, 3, 15), -3)), (2023, 12, 15));
    }

    #[test]
    fn month_keys_sort_and_invert() {
        let key = month_key(from_ymd(2024, 3, 4));
        assert_eq!(month_key_start(key), from_ymd(2024, 3, 1));
        assert!(month_key(from_ymd(2024, 1, 1)) < month_key(from_ymd(2024, 2, 1)));
        assert_eq!(format_month(month_key(from_ymd(2024, 3, 4))), "Mar 2024");
        assert_eq!(format_iso(from_ymd(2024, 3, 4)), "2024-03-04");
        assert_eq!(format_short(from_ymd(2024, 3, 4)), "4 Mar 2024");
        assert_eq!(year_of(from_ymd(2024, 3, 4)), 2024);
        assert_eq!(month_of(from_ymd(2024, 3, 4)), 3);
    }

    #[test]
    fn add_days_crosses_month_and_year() {
        assert_eq!(add_days(from_ymd(1970, 1, 1), 0), 0);
        assert_eq!(add_days(from_ymd(1970, 1, 1), -1), from_ymd(1969, 12, 31));
        assert_eq!(add_days(from_ymd(2024, 12, 31), 1), from_ymd(2025, 1, 1));
        assert_eq!(add_days(from_ymd(2024, 2, 29), 1), from_ymd(2024, 3, 1));
        assert_eq!(add_days(from_ymd(2024, 3, 10), -10), from_ymd(2024, 2, 29));
    }

    #[test]
    fn iso_weeks_at_year_boundaries() {
        // 2026-01-01 is a Thursday, so it is week 1 of 2026.
        assert_eq!(iso_week(from_ymd(2026, 1, 1)), 1);
        // 2021-01-01 is a Friday: still week 53 of 2020.
        assert_eq!(iso_week(from_ymd(2021, 1, 1)), 53);
        assert_eq!(iso_week(from_ymd(2020, 12, 31)), 53);
        assert_eq!(iso_week(from_ymd(2026, 12, 28)), 53);
        // 2015-01-01 is a Thursday: week 1.
        assert_eq!(iso_week(from_ymd(2015, 1, 1)), 1);
        // 2020-01-01 is a Wednesday: week 1 (Thursday the 2nd is in week 1).
        assert_eq!(iso_week(from_ymd(2020, 1, 1)), 1);
        // Mid-year sanity: 2024-03-04 is a Monday in week 10.
        assert_eq!(iso_week(from_ymd(2024, 3, 4)), 10);
    }

    #[test]
    fn month_grid_is_always_six_by_seven() {
        // March 2024 starts on Friday. Monday-first: leading Thu 29 Feb … Fri 1 Mar.
        let march = month_grid(2024, 3, 0);
        assert_eq!(march.len(), 6);
        assert_eq!(march[0].len(), 7);
        assert_eq!(march[0][0], GridDay { day: from_ymd(2024, 2, 26), in_month: false });
        assert_eq!(march[0][4], GridDay { day: from_ymd(2024, 3, 1), in_month: true });
        assert_eq!(march[4][6], GridDay { day: from_ymd(2024, 3, 31), in_month: true });
        assert_eq!(march[5][0], GridDay { day: from_ymd(2024, 4, 1), in_month: false });
        let in_month: usize = march.iter().flatten().filter(|c| c.in_month).count();
        assert_eq!(in_month, 31);

        // Sunday-first (first_weekday = 6): March 2024 starts in column 5.
        let sunday_first = month_grid(2024, 3, 6);
        assert_eq!(sunday_first[0][0].day, from_ymd(2024, 2, 25));
        assert!(!sunday_first[0][0].in_month);
        assert_eq!(sunday_first[0][5], GridDay { day: from_ymd(2024, 3, 1), in_month: true });

        // February 2024 is a leap month that starts on Thursday.
        let feb = month_grid(2024, 2, 0);
        assert_eq!(feb[0][3], GridDay { day: from_ymd(2024, 2, 1), in_month: true });
        assert_eq!(feb.iter().flatten().filter(|c| c.in_month).count(), 29);
        assert_eq!(feb[4][3], GridDay { day: from_ymd(2024, 2, 29), in_month: true });

        // A month that starts on Monday has no leading padding.
        let april_2019 = month_grid(2019, 4, 0);
        assert_eq!(april_2019[0][0], GridDay { day: from_ymd(2019, 4, 1), in_month: true });
        assert_eq!(april_2019.iter().flatten().filter(|c| c.in_month).count(), 30);
    }

    #[test]
    fn names_are_english() {
        assert_eq!(weekday_name(0), "Monday");
        assert_eq!(weekday_name(3), "Thursday");
        assert_eq!(weekday_name(6), "Sunday");
        assert_eq!(weekday_name(7), "Monday");
        assert_eq!(month_name(1), "January");
        assert_eq!(month_name(2), "February");
        assert_eq!(month_name(12), "December");
        assert_eq!(WEEKDAY_ABBR[0], "Mon");
        assert_eq!(MONTH_ABBR[2], "Mar");
    }

    #[test]
    fn parse_iso_accepts_only_calendar_dates() {
        assert_eq!(parse_iso("2024-03-04"), Some(from_ymd(2024, 3, 4)));
        assert_eq!(parse_iso(" 2024-02-29 "), Some(from_ymd(2024, 2, 29)));
        assert_eq!(parse_iso("1970-01-01"), Some(0));
        assert_eq!(parse_iso("2023-02-29"), None);
        assert_eq!(parse_iso("2024-02-30"), None);
        assert_eq!(parse_iso("2024-13-01"), None);
        assert_eq!(parse_iso("2024-00-01"), None);
        assert_eq!(parse_iso("2024-03-32"), None);
        assert_eq!(parse_iso("2024-3-4"), None);
        assert_eq!(parse_iso("04/03/2024"), None);
        assert_eq!(parse_iso("2024/03/04"), None);
        assert_eq!(parse_iso(""), None);
        assert_eq!(format_iso(parse_iso("2024-03-04").unwrap()), "2024-03-04");
    }

    #[test]
    fn ranges_cover_what_reports_ask_for() {
        let day = from_ymd(2024, 3, 15);
        let last_3 = DateRange::last_months(day, 3);
        assert_eq!(last_3.start, from_ymd(2024, 1, 1));
        assert_eq!(last_3.end, from_ymd(2024, 3, 31));
        assert!(last_3.contains(from_ymd(2024, 2, 29)));
        assert!(!last_3.contains(from_ymd(2023, 12, 31)));
        assert_eq!(DateRange::year(2024).days(), 366);
        assert_eq!(DateRange::year(2023).days(), 365);
        let march = DateRange::month(month_key(from_ymd(2024, 3, 1)));
        assert_eq!(march.start, from_ymd(2024, 3, 1));
        assert_eq!(march.end, from_ymd(2024, 3, 31));
        assert_eq!(march.days(), 31);
    }
}
