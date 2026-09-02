//! Civil dates as a day number, and the fight to read the ones banks write.
//!
//! A ledger only ever needs whole days: no clocks, no zones, no leap
//! seconds. So a date is an `i32` count of days from 1970-01-01, which
//! sorts, subtracts and indexes into a month bucket without a calendar
//! library, and is four bytes in a row of a hundred thousand.
//!
//! The hard part is not arithmetic, it is `03/04/2024`. That is the 3rd of
//! April in Europe and the 4th of March in America, and the file rarely
//! says which. Guessing per row silently scatters transactions across
//! months. So [`sniff_date_format`] reads the WHOLE column and only then
//! decides — a single row with a day above 12 settles it for every other
//! row, and when nothing settles it the caller is told, so the import
//! screen can ask instead of inventing an answer.

use std::fmt;

/// Days since 1970-01-01. Negative reaches back before it.
pub type Day = i32;

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

pub fn year_of(day: Day) -> i32 {
    to_ymd(day).0
}

pub fn month_of(day: Day) -> u32 {
    to_ymd(day).1
}

/// 0 = Monday. (1970-01-01 was a Thursday.)
pub fn weekday(day: Day) -> u32 {
    (day.rem_euclid(7) as u32 + 3) % 7
}

pub fn is_weekend(day: Day) -> bool {
    weekday(day) >= 5
}

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

/// A month as a sortable integer key, `year * 12 + (month - 1)` — what
/// budgets and monthly rollups are keyed by.
pub type MonthKey = i32;

pub fn month_key(day: Day) -> MonthKey {
    let (y, m, _) = to_ymd(day);
    y * 12 + (m as i32 - 1)
}

pub fn month_key_start(key: MonthKey) -> Day {
    from_ymd(key.div_euclid(12), key.rem_euclid(12) as u32 + 1, 1)
}

pub const MONTH_NAMES: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August",
    "September", "October", "November", "December",
];

pub const MONTH_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub const WEEKDAY_ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

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

/// How the dates in an imported column are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DateFormat {
    /// The order of the numeric fields.
    pub order: FieldOrder,
    /// Two-digit years, which need a century guess.
    pub two_digit_year: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FieldOrder {
    /// `2024-03-04`, ISO 8601. The default because it is the only order
    /// that cannot be misread.
    #[default]
    Ymd,
    /// `04/03/2024` — most of the world.
    Dmy,
    /// `03/04/2024` — the United States.
    Mdy,
}

/// What a column of dates turned out to be, and whether we are sure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateSniff {
    pub format: DateFormat,
    /// False when every row was ambiguous (no day above 12 anywhere), so
    /// the order is a guess and the import screen must offer the choice.
    pub certain: bool,
    /// Rows that parsed under the chosen format.
    pub parsed: usize,
    /// Rows that did not parse at all.
    pub failed: usize,
}

/// Read a whole column of dates and work out how it is written.
///
/// The rule that does the work: in `a/b/c`, if any row has `a > 12` the
/// first field must be a day, and if any row has `b > 12` the second must
/// be. One such row decides the column. If none does — a statement whose
/// every transaction lands in the first twelve days of a month — the order
/// stays a guess, `certain` is false, and the caller asks.
pub fn sniff_date_format<'a>(cells: impl Iterator<Item = &'a str>) -> DateSniff {
    let mut first_over_12 = false;
    let mut second_over_12 = false;
    let mut iso = 0usize;
    let mut two_digit = 0usize;
    let mut numeric = 0usize;
    let mut total = 0usize;
    let mut samples: Vec<[u32; 3]> = Vec::new();

    for cell in cells {
        let cell = cell.trim();
        if cell.is_empty() {
            continue;
        }
        total += 1;
        let Some((fields, year_digits)) = split_numeric_date(cell) else {
            // Named-month forms ("4 Mar 2024") are self-describing and
            // vote for nothing.
            continue;
        };
        numeric += 1;
        if year_digits == 2 {
            two_digit += 1;
        }
        if fields[0] > 31 {
            iso += 1; // a 4-digit year leading: 2024-03-04
        } else {
            if fields[0] > 12 {
                first_over_12 = true;
            }
            if fields[1] > 12 {
                second_over_12 = true;
            }
            samples.push(fields);
        }
    }

    let order = if iso > 0 && iso >= numeric / 2 {
        FieldOrder::Ymd
    } else if first_over_12 {
        FieldOrder::Dmy
    } else if second_over_12 {
        FieldOrder::Mdy
    } else {
        // Nothing decisive. Day-first is the world's convention and the
        // safer default; `certain: false` is what actually matters here.
        FieldOrder::Dmy
    };
    let format = DateFormat { order, two_digit_year: two_digit > numeric / 2 };
    let certain = matches!(order, FieldOrder::Ymd) || first_over_12 || second_over_12;
    DateSniff { format, certain, parsed: numeric, failed: total - numeric }
}

/// Split `04/03/2024`, `04-03-2024`, `04.03.2024` into its three numbers,
/// with the digit count of the field that looks like a year.
fn split_numeric_date(text: &str) -> Option<([u32; 3], usize)> {
    let head: &str = text.split_whitespace().next().unwrap_or(text);
    let mut fields = [0u32; 3];
    let mut widths = [0usize; 3];
    let mut index = 0usize;
    let mut digits = 0usize;
    let mut current = 0u32;
    for ch in head.chars() {
        if let Some(d) = ch.to_digit(10) {
            current = current.checked_mul(10)?.checked_add(d)?;
            digits += 1;
        } else if matches!(ch, '/' | '-' | '.') {
            if index >= 2 || digits == 0 {
                return None;
            }
            fields[index] = current;
            widths[index] = digits;
            index += 1;
            current = 0;
            digits = 0;
        } else {
            return None;
        }
    }
    if index != 2 || digits == 0 {
        return None;
    }
    fields[2] = current;
    widths[2] = digits;
    let year_digits = if widths[0] == 4 { widths[0] } else { widths[2] };
    Some((fields, year_digits))
}

/// Two digits to a century: the 69/70 split every system uses, biased so
/// that a statement from '99 is 1999 and one from '24 is 2024.
fn expand_year(year: u32) -> i32 {
    if year >= 100 {
        year as i32
    } else if year >= 70 {
        1900 + year as i32
    } else {
        2000 + year as i32
    }
}

/// Parse one cell under a known format. Also understands ISO and named
/// months regardless of `format`, since those are unambiguous.
pub fn parse_date(text: &str, format: DateFormat) -> Option<Day> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some((fields, _)) = split_numeric_date(text) {
        let (y, m, d) = if fields[0] > 31 {
            (fields[0], fields[1], fields[2]) // leading 4-digit year: ISO
        } else {
            match format.order {
                FieldOrder::Ymd => (fields[0], fields[1], fields[2]),
                FieldOrder::Dmy => (fields[2], fields[1], fields[0]),
                FieldOrder::Mdy => (fields[2], fields[0], fields[1]),
            }
        };
        return valid_ymd(expand_year(y), m, d);
    }
    parse_named_month(text)
}

/// `4 Mar 2024`, `Mar 4, 2024`, `4 March 2024`, `2024 Mar 4`.
fn parse_named_month(text: &str) -> Option<Day> {
    let cleaned: String = text
        .chars()
        .map(|c| if c == ',' { ' ' } else { c })
        .collect();
    let mut month = None;
    let mut numbers: Vec<u32> = Vec::new();
    for word in cleaned.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        if let Some(index) = MONTH_ABBR
            .iter()
            .position(|m| lower.starts_with(&m.to_ascii_lowercase()))
        {
            if month.is_none() {
                month = Some(index as u32 + 1);
                continue;
            }
        }
        let digits: String = word.chars().filter(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            if let Ok(value) = digits.parse::<u32>() {
                numbers.push(value);
            }
        }
    }
    let month = month?;
    if numbers.len() < 2 {
        return None;
    }
    // Whichever number could not be a day is the year.
    let (day, year) = if numbers[0] > 31 {
        (numbers[1], numbers[0])
    } else {
        (numbers[0], numbers[1])
    };
    valid_ymd(expand_year(year), month, day)
}

fn valid_ymd(year: i32, month: u32, day: u32) -> Option<Day> {
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    if !(1900..=2200).contains(&year) {
        return None;
    }
    Some(from_ymd(year, month, day))
}

/// A closed range of days, which is what every report and filter is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateRange {
    pub start: Day,
    pub end: Day,
}

impl DateRange {
    pub fn contains(&self, day: Day) -> bool {
        day >= self.start && day <= self.end
    }

    pub fn days(&self) -> i32 {
        self.end - self.start + 1
    }

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

    pub fn year(year: i32) -> DateRange {
        DateRange { start: from_ymd(year, 1, 1), end: from_ymd(year, 12, 31) }
    }
}

impl fmt::Display for DateRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} – {}", format_short(self.start), format_short(self.end))
    }
}

/// Today, from the system clock. The one place time enters the app.
pub fn today() -> Day {
    let secs = makepad_widgets::Cx::time_now().max(0.0) as i64;
    (secs / 86_400) as Day
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
    }

    #[test]
    fn the_ambiguous_column_is_settled_by_one_decisive_row() {
        // 13 can only be a day: the whole column is day-first.
        let eu = ["04/03/2024", "13/03/2024", "01/04/2024"];
        let sniff = sniff_date_format(eu.iter().copied());
        assert_eq!(sniff.format.order, FieldOrder::Dmy);
        assert!(sniff.certain);
        assert_eq!(parse_date("04/03/2024", sniff.format), Some(from_ymd(2024, 3, 4)));

        // 13 in the second field: month-first.
        let us = ["03/04/2024", "03/13/2024", "04/01/2024"];
        let sniff = sniff_date_format(us.iter().copied());
        assert_eq!(sniff.format.order, FieldOrder::Mdy);
        assert!(sniff.certain);
        // The same eight characters, read the other way round: month 03,
        // day 04 — which is the whole reason the column has to vote.
        assert_eq!(parse_date("03/04/2024", sniff.format), Some(from_ymd(2024, 3, 4)));
        assert_eq!(parse_date("04/01/2024", sniff.format), Some(from_ymd(2024, 4, 1)));

        // Nothing decisive: we guess, but we SAY we guessed.
        let ambiguous = ["03/04/2024", "05/06/2024"];
        let sniff = sniff_date_format(ambiguous.iter().copied());
        assert!(!sniff.certain);

        // ISO needs no guessing.
        let iso = ["2024-03-04", "2024-03-13"];
        let sniff = sniff_date_format(iso.iter().copied());
        assert_eq!(sniff.format.order, FieldOrder::Ymd);
        assert!(sniff.certain);
    }

    #[test]
    fn parses_the_forms_banks_write() {
        let dmy = DateFormat { order: FieldOrder::Dmy, two_digit_year: false };
        assert_eq!(parse_date("04.03.2024", dmy), Some(from_ymd(2024, 3, 4)));
        assert_eq!(parse_date("04-03-2024", dmy), Some(from_ymd(2024, 3, 4)));
        assert_eq!(parse_date("04/03/24", dmy), Some(from_ymd(2024, 3, 4)));
        // ISO and named months parse under any declared order.
        assert_eq!(parse_date("2024-03-04", dmy), Some(from_ymd(2024, 3, 4)));
        assert_eq!(parse_date("4 Mar 2024", dmy), Some(from_ymd(2024, 3, 4)));
        assert_eq!(parse_date("Mar 4, 2024", dmy), Some(from_ymd(2024, 3, 4)));
        assert_eq!(parse_date("4 March 2024", dmy), Some(from_ymd(2024, 3, 4)));
        // Impossible dates are rejected, not clamped.
        assert_eq!(parse_date("31/02/2024", dmy), None);
        assert_eq!(parse_date("00/01/2024", dmy), None);
        assert_eq!(parse_date("hello", dmy), None);
        // Two-digit years split at 70.
        assert_eq!(parse_date("01/01/99", dmy), Some(from_ymd(1999, 1, 1)));
        assert_eq!(parse_date("01/01/24", dmy), Some(from_ymd(2024, 1, 1)));
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
    }
}
