//! Money is an integer number of minor units. Never a float.
//!
//! A balance is a sum of thousands of amounts, and every one of those sums
//! has to come out the way a bank would compute it. Binary floating point
//! cannot represent 0.10, so a ledger built on `f64` drifts: add a tenth a
//! thousand times and you are three cents short of a hundred. Everything
//! here is `i64` minor units — cents for USD/EUR, but also 0 decimals for
//! JPY and 3 for BHD, which is why the scale lives on the currency rather
//! than being assumed to be 100.
//!
//! `i64` cents reaches ±92 quadrillion. That is not a limit anyone hits,
//! and it makes every intermediate sum exact.

use std::fmt;

/// A currency, as much of ISO 4217 as a ledger needs: how many decimal
/// places it has, and how it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Currency {
    pub code: &'static str,
    pub symbol: &'static str,
    /// Decimal places — the power of ten between the major unit and the
    /// minor unit this crate stores.
    pub decimals: u8,
    /// Symbol before the number (`$12.34`) or after it (`12,34 €`).
    pub symbol_first: bool,
}

pub const USD: Currency =
    Currency { code: "USD", symbol: "$", decimals: 2, symbol_first: true };
pub const EUR: Currency =
    Currency { code: "EUR", symbol: "€", decimals: 2, symbol_first: false };
pub const GBP: Currency =
    Currency { code: "GBP", symbol: "£", decimals: 2, symbol_first: true };
pub const JPY: Currency =
    Currency { code: "JPY", symbol: "¥", decimals: 0, symbol_first: true };
pub const CHF: Currency =
    Currency { code: "CHF", symbol: "CHF", decimals: 2, symbol_first: true };

/// Every currency this build knows, for pickers and for parsing a code out
/// of an imported file.
pub const CURRENCIES: [Currency; 5] = [USD, EUR, GBP, JPY, CHF];

impl Default for Currency {
    /// A file that has not said otherwise. `Ledger` derives `Default`, and
    /// a currency-less amount is not a thing this app can represent.
    fn default() -> Currency {
        USD
    }
}

pub fn currency_by_code(code: &str) -> Option<Currency> {
    CURRENCIES
        .iter()
        .copied()
        .find(|c| c.code.eq_ignore_ascii_case(code))
}

/// How a number was written in the file we are reading. Bank exports differ
/// on every one of these axes, and guessing wrong turns 1.234,56 into 1.23.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct AmountFormat {
    /// `,` in `1.234,56` (continental) or `.` in `1,234.56` (anglo).
    pub decimal_comma: bool,
    /// A trailing `-` means negative: `1.234,56-` (SEPA, some German banks).
    pub trailing_minus: bool,
    /// `(1,234.56)` means negative (accounting convention).
    pub parens_negative: bool,
}

/// Parse a money amount out of a cell of an imported file.
///
/// Deliberately liberal: currency symbols, spaces (including the narrow
/// no-break space German banks use as a thousands separator), `+` signs and
/// thousands separators are all discarded, because every bank writes them
/// differently and none of them mean anything. What it will NOT do is
/// guess the decimal separator when the file has told us — pass the format
/// sniffed from the whole column ([`sniff_amount_format`]), never per cell.
/// Deciding per cell is how `1.234` becomes 1.23 in one row and 1234.00 in
/// the next.
pub fn parse_amount(text: &str, format: AmountFormat, decimals: u8) -> Option<i64> {
    let mut cleaned = String::with_capacity(text.len());
    let mut negative = false;
    for ch in text.chars() {
        match ch {
            '-' | '\u{2212}' => negative = true, // ASCII hyphen or real minus
            '(' if format.parens_negative => negative = true,
            '0'..='9' => cleaned.push(ch),
            ',' if format.decimal_comma => cleaned.push('.'),
            '.' if !format.decimal_comma => cleaned.push('.'),
            // Thousands separators and everything else: currency symbols,
            // spaces, NBSP, apostrophes (Swiss 1'234.56), `+`, `)`.
            _ => {}
        }
    }
    if cleaned.is_empty() || !cleaned.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    // More than one separator left means the extras were thousands marks
    // ("1.234.567,89" cleaned to "1.234.567.89"): keep the last one.
    let value = if cleaned.matches('.').count() > 1 {
        let last = cleaned.rfind('.').unwrap();
        let mut merged: String = cleaned[..last].replace('.', "");
        merged.push('.');
        merged.push_str(&cleaned[last + 1..]);
        merged
    } else {
        cleaned
    };
    let (whole, frac) = match value.split_once('.') {
        Some((w, f)) => (w, f),
        None => (value.as_str(), ""),
    };
    // A group of exactly three digits after the only separator, in a file
    // whose decimal separator we believe is the other character, was a
    // thousands separator: "1.234" is 1234, not 1.23.
    let scale = 10i64.checked_pow(decimals as u32)?;
    let whole_value: i64 = if whole.is_empty() { 0 } else { whole.parse().ok()? };
    let mut minor = whole_value.checked_mul(scale)?;
    if !frac.is_empty() {
        let digits: String = frac.chars().take(decimals as usize).collect();
        let mut fraction: i64 = if digits.is_empty() { 0 } else { digits.parse().ok()? };
        // Pad "5" to "50" for a 2-decimal currency.
        for _ in digits.len()..decimals as usize {
            fraction = fraction.checked_mul(10)?;
        }
        // Round rather than truncate on extra precision (a 4-decimal FX
        // amount landing in a 2-decimal account).
        let round_up = frac
            .chars()
            .nth(decimals as usize)
            .is_some_and(|c| c >= '5' && c <= '9');
        minor = minor.checked_add(fraction)?;
        if round_up {
            minor = minor.checked_add(1)?;
        }
    }
    if negative || format.trailing_minus && text.trim_end().ends_with('-') {
        minor = -minor;
    }
    Some(minor)
}

/// Work out how a column of amounts is written by looking at all of it.
///
/// The decision that matters is the decimal separator, and a single cell
/// often cannot settle it: `1.234` is ambiguous, `1.234,56` is not. So the
/// whole column votes — any cell with both separators, or with a comma
/// followed by exactly two digits at the end, is evidence.
pub fn sniff_amount_format<'a>(cells: impl Iterator<Item = &'a str>) -> AmountFormat {
    let mut comma_decimal = 0usize;
    let mut dot_decimal = 0usize;
    let mut trailing_minus = false;
    let mut parens = false;
    for cell in cells {
        let cell = cell.trim();
        if cell.is_empty() {
            continue;
        }
        if cell.ends_with('-') {
            trailing_minus = true;
        }
        if cell.starts_with('(') && cell.ends_with(')') {
            parens = true;
        }
        let last_comma = cell.rfind(',');
        let last_dot = cell.rfind('.');
        match (last_comma, last_dot) {
            // Both present: the LAST one is the decimal separator.
            (Some(c), Some(d)) => {
                if c > d {
                    comma_decimal += 1;
                } else {
                    dot_decimal += 1;
                }
            }
            // One separator with 1-2 trailing digits reads as a decimal;
            // with exactly 3 it reads as a thousands mark and says nothing.
            (Some(c), None) => {
                let tail = cell.len() - c - 1;
                if tail <= 2 {
                    comma_decimal += 1;
                }
            }
            (None, Some(d)) => {
                let tail = cell.len() - d - 1;
                if tail <= 2 {
                    dot_decimal += 1;
                }
            }
            (None, None) => {}
        }
    }
    AmountFormat {
        decimal_comma: comma_decimal > dot_decimal,
        trailing_minus,
        parens_negative: parens,
    }
}

/// `1234567` cents → `"12,345.67"`. Grouping and the decimal mark follow
/// the currency's convention, not the machine's locale: a ledger of euros
/// reads the same on every machine that opens the file.
pub fn format_minor(minor: i64, currency: Currency) -> String {
    let decimals = currency.decimals as usize;
    let negative = minor < 0;
    let magnitude = minor.unsigned_abs();
    let scale = 10u64.pow(decimals as u32);
    let whole = magnitude / scale;
    let frac = magnitude % scale;

    let (group, point) = if currency.decimals == 2 && !currency.symbol_first {
        ('.', ',') // continental: 1.234,56
    } else {
        (',', '.') // anglo: 1,234.56
    };

    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3 + 4);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(group);
        }
        grouped.push(ch);
    }
    let mut out = String::with_capacity(grouped.len() + decimals + 4);
    if negative {
        out.push('-');
    }
    out.push_str(&grouped);
    if decimals > 0 {
        out.push(point);
        out.push_str(&format!("{frac:0width$}", width = decimals));
    }
    out
}

/// With the currency's symbol attached, the way that currency writes it.
pub fn format_money(minor: i64, currency: Currency) -> String {
    let number = format_minor(minor, currency);
    if currency.symbol_first {
        // The sign stays outside the symbol: -$12.34, not $-12.34.
        match number.strip_prefix('-') {
            Some(rest) => format!("-{}{}", currency.symbol, rest),
            None => format!("{}{}", currency.symbol, number),
        }
    } else {
        format!("{} {}", number, currency.symbol)
    }
}

/// Short form for chart axes and dense cells: `12.3k`, `1.2M`. Keeps the
/// sign, drops the currency.
pub fn format_compact(minor: i64, currency: Currency) -> String {
    let scale = 10i64.pow(currency.decimals as u32);
    let major = minor as f64 / scale as f64;
    let magnitude = major.abs();
    let sign = if major < 0.0 { "-" } else { "" };
    if magnitude >= 1_000_000.0 {
        format!("{sign}{:.1}M", magnitude / 1_000_000.0)
    } else if magnitude >= 1_000.0 {
        format!("{sign}{:.1}k", magnitude / 1_000.0)
    } else {
        format!("{sign}{:.0}", magnitude)
    }
}

/// A signed amount with its currency, for display and for the few places
/// that carry an amount around on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Money {
    pub minor: i64,
    pub currency: Currency,
}

impl Money {
    pub fn new(minor: i64, currency: Currency) -> Money {
        Money { minor, currency }
    }

    pub fn zero(currency: Currency) -> Money {
        Money { minor: 0, currency }
    }

    pub fn is_negative(&self) -> bool {
        self.minor < 0
    }

    pub fn abs(&self) -> Money {
        Money { minor: self.minor.abs(), currency: self.currency }
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_money(self.minor, self.currency))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anglo() -> AmountFormat {
        AmountFormat::default()
    }

    fn continental() -> AmountFormat {
        AmountFormat { decimal_comma: true, ..AmountFormat::default() }
    }

    #[test]
    fn parses_the_shapes_banks_actually_export() {
        assert_eq!(parse_amount("1234.56", anglo(), 2), Some(123456));
        assert_eq!(parse_amount("1,234.56", anglo(), 2), Some(123456));
        assert_eq!(parse_amount("$1,234.56", anglo(), 2), Some(123456));
        assert_eq!(parse_amount("-1,234.56", anglo(), 2), Some(-123456));
        assert_eq!(parse_amount("1.234,56", continental(), 2), Some(123456));
        assert_eq!(parse_amount("1.234.567,89", continental(), 2), Some(123456789));
        assert_eq!(parse_amount("1'234.56", anglo(), 2), Some(123456)); // Swiss
        assert_eq!(parse_amount("12,34 €", continental(), 2), Some(1234));
        // Fewer decimals written than the currency has.
        assert_eq!(parse_amount("5.5", anglo(), 2), Some(550));
        assert_eq!(parse_amount("5", anglo(), 2), Some(500));
        // Zero-decimal currency.
        assert_eq!(parse_amount("1,250", anglo(), 0), Some(1250));
        // Junk is None, not zero: a failed parse must never post 0.00.
        assert_eq!(parse_amount("", anglo(), 2), None);
        assert_eq!(parse_amount("n/a", anglo(), 2), None);
        assert_eq!(parse_amount("--", anglo(), 2), None);
    }

    #[test]
    fn honours_the_negative_conventions() {
        let trailing = AmountFormat { trailing_minus: true, ..anglo() };
        assert_eq!(parse_amount("1234.56-", trailing, 2), Some(-123456));
        let parens = AmountFormat { parens_negative: true, ..anglo() };
        assert_eq!(parse_amount("(1,234.56)", parens, 2), Some(-123456));
        // A real Unicode minus, which some exports use.
        assert_eq!(parse_amount("\u{2212}12.00", anglo(), 2), Some(-1200));
    }

    #[test]
    fn extra_precision_rounds_rather_than_truncates() {
        assert_eq!(parse_amount("1.005", anglo(), 2), Some(101));
        assert_eq!(parse_amount("1.004", anglo(), 2), Some(100));
    }

    #[test]
    fn sniffing_reads_the_column_not_the_cell() {
        // Ambiguous alone; the column settles it.
        let german = ["1.234,56", "-89,10", "1.000,00"];
        assert!(sniff_amount_format(german.iter().copied()).decimal_comma);
        let anglo_col = ["1,234.56", "-89.10", "1,000.00"];
        assert!(!sniff_amount_format(anglo_col.iter().copied()).decimal_comma);
        // Thousands-only groups say nothing and must not flip the vote.
        let ambiguous = ["1.234", "5.678"];
        assert!(!sniff_amount_format(ambiguous.iter().copied()).decimal_comma);
        let trailing = ["1234.56-", "10.00"];
        assert!(sniff_amount_format(trailing.iter().copied()).trailing_minus);
    }

    #[test]
    fn formats_the_way_each_currency_is_written() {
        assert_eq!(format_money(123456, USD), "$1,234.56");
        assert_eq!(format_money(-123456, USD), "-$1,234.56");
        assert_eq!(format_money(123456, EUR), "1.234,56 €");
        assert_eq!(format_money(1250, JPY), "¥1,250");
        assert_eq!(format_minor(0, USD), "0.00");
        assert_eq!(format_minor(-5, USD), "-0.05");
        assert_eq!(format_compact(123456789, USD), "1.2M");
        assert_eq!(format_compact(-1234567, USD), "-12.3k");
    }

    #[test]
    fn a_thousand_dimes_are_exactly_a_hundred() {
        // The whole reason this module exists.
        let total: i64 = (0..1000).map(|_| 10i64).sum();
        assert_eq!(total, 10_000);
        assert_eq!(format_money(total, USD), "$100.00");
    }
}
