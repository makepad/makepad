//! Civil dates as a day number, and the fight to read the ones banks write.
//!
//! Day arithmetic lives in [`makepad_civil_time`] and is re-exported here so
//! the rest of finance keeps calling `crate::date`. A ledger only ever needs
//! whole days: no clocks, no zones, no leap seconds.
//!
//! The hard part is not arithmetic, it is `03/04/2024`. That is the 3rd of
//! April in Europe and the 4th of March in America, and the file rarely
//! says which. Guessing per row silently scatters transactions across
//! months. So [`sniff_date_format`] reads the WHOLE column and only then
//! decides — a single row with a day above 12 settles it for every other
//! row, and when nothing settles it the caller is told, so the import
//! screen can ask instead of inventing an answer.

pub use makepad_civil_time::{
    add_months, days_in_month, format_iso, format_month, format_short, from_ymd, is_weekend,
    month_end, month_key, month_key_start, month_of, month_start, to_ymd, weekday, year_of,
    DateRange, Day, MonthKey, MONTH_ABBR, MONTH_NAMES, WEEKDAY_ABBR,
};

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

/// Today, from the system clock. The one place time enters the app.
pub fn today() -> Day {
    let secs = makepad_widgets::Cx::time_now().max(0.0) as i64;
    (secs / 86_400) as Day
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
