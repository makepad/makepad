//! A CSV reader that survives what banks actually export.
//!
//! RFC 4180 is a page long and describes maybe half of the files a bank
//! will hand you. The rest of this module is the other half: a UTF-8 BOM
//! that would otherwise make the first header `\u{feff}Date`; semicolons
//! because the country uses a comma for decimals; tabs; CRLF; quoted fields
//! with embedded newlines; `""` escapes inside quotes; a preamble of
//! account-header junk above the real header row; and ragged rows.
//!
//! Nothing here allocates per field beyond the field itself, and the whole
//! file is read into memory on purpose — the largest statement export
//! anyone has is a few megabytes, and random access to the rows is what the
//! import preview needs.

/// The delimiter a file turned out to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delimiter {
    Comma,
    Semicolon,
    Tab,
    Pipe,
}

impl Delimiter {
    pub fn byte(self) -> u8 {
        match self {
            Delimiter::Comma => b',',
            Delimiter::Semicolon => b';',
            Delimiter::Tab => b'\t',
            Delimiter::Pipe => b'|',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Delimiter::Comma => "Comma",
            Delimiter::Semicolon => "Semicolon",
            Delimiter::Tab => "Tab",
            Delimiter::Pipe => "Pipe",
        }
    }

    pub const ALL: [Delimiter; 4] =
        [Delimiter::Comma, Delimiter::Semicolon, Delimiter::Tab, Delimiter::Pipe];
}

/// A parsed file: rows of fields, plus what we had to work out to read it.
#[derive(Clone, Debug)]
pub struct Csv {
    pub rows: Vec<Vec<String>>,
    pub delimiter: Delimiter,
    /// Rows skipped above the header (bank preamble).
    pub preamble: usize,
}

impl Csv {
    /// The row we believe holds column names.
    pub fn header(&self) -> &[String] {
        self.rows.first().map(|r| r.as_slice()).unwrap_or(&[])
    }

    /// Everything below the header.
    pub fn records(&self) -> &[Vec<String>] {
        self.rows.get(1..).unwrap_or(&[])
    }

    /// One column of the records, for sniffing formats. Short rows yield
    /// an empty string rather than being skipped, so the row index and the
    /// value index stay in step.
    pub fn column(&self, index: usize) -> impl Iterator<Item = &str> {
        self.records()
            .iter()
            .map(move |row| row.get(index).map(|s| s.as_str()).unwrap_or(""))
    }

    pub fn width(&self) -> usize {
        self.rows.iter().map(|r| r.len()).max().unwrap_or(0)
    }
}

/// Split text into rows of fields with a known delimiter.
///
/// The state machine is RFC 4180's, with the tolerances real files need: a
/// quote inside an unquoted field is a literal quote (not an error), a
/// field that never closes its quote ends at end of input, and CRLF, LF and
/// a lone CR all end a row.
pub fn parse_with(text: &str, delimiter: Delimiter) -> Vec<Vec<String>> {
    let delim = delimiter.byte() as char;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    // A BOM would otherwise become part of the first header name.
    if text.starts_with('\u{feff}') {
        chars.next();
    }
    let mut any = false;

    while let Some(ch) = chars.next() {
        any = true;
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"'); // "" is one literal quote
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        match ch {
            '"' if field.is_empty() => in_quotes = true,
            c if c == delim => {
                row.push(std::mem::take(&mut field));
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            c => field.push(c),
        }
    }
    if any && (!field.is_empty() || !row.is_empty()) {
        row.push(field);
        rows.push(row);
    }
    // A trailing newline leaves one empty row; so does a blank line in the
    // middle of a bank's preamble. Neither is a record.
    rows.retain(|row| !(row.len() == 1 && row[0].trim().is_empty()));
    rows
}

/// Work out the delimiter by trying each one and asking which gives the
/// most consistent row width.
///
/// Counting occurrences is not enough: a file full of `"Smith, John"`
/// payees has plenty of commas inside quotes, and a German file has both
/// semicolons and commas. Parsing under each candidate and scoring the
/// result is what tells them apart — the right delimiter yields many
/// columns AND the same number in nearly every row.
pub fn sniff_delimiter(text: &str) -> Delimiter {
    let sample: String = text.lines().take(30).collect::<Vec<_>>().join("\n");
    let mut best = (Delimiter::Comma, -1.0f64);
    for candidate in Delimiter::ALL {
        let rows = parse_with(&sample, candidate);
        if rows.len() < 2 {
            continue;
        }
        let widths: Vec<usize> = rows.iter().map(|r| r.len()).collect();
        let modal = modal_width(&widths);
        if modal < 2 {
            continue; // one column means this character is not the delimiter
        }
        let consistent =
            widths.iter().filter(|w| **w == modal).count() as f64 / widths.len() as f64;
        // Consistency first, then column count as the tie-break: a file
        // read under the wrong delimiter is ragged, and a file read under
        // the right one usually has more columns than a partial split.
        let score = consistent * 100.0 + modal as f64;
        if score > best.1 {
            best = (candidate, score);
        }
    }
    best.0
}

fn modal_width(widths: &[usize]) -> usize {
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for width in widths {
        match counts.iter_mut().find(|(w, _)| w == width) {
            Some((_, n)) => *n += 1,
            None => counts.push((*width, 1)),
        }
    }
    counts.sort_by_key(|(width, count)| (std::cmp::Reverse(*count), *width));
    counts.first().map(|(w, _)| *w).unwrap_or(0)
}

/// Read a file: sniff the delimiter, drop any preamble above the header,
/// and pad ragged rows to the header's width.
///
/// The preamble is the reason this is not two lines. Plenty of banks print
/// "Account: 1234", a blank line and a date range above the actual table;
/// the header is the first row whose width matches the width most rows
/// have. Everything above it is dropped, and remembered so the import
/// screen can say so.
pub fn parse(text: &str) -> Csv {
    let delimiter = sniff_delimiter(text);
    let mut rows = parse_with(text, delimiter);
    let widths: Vec<usize> = rows.iter().map(|r| r.len()).collect();
    let modal = modal_width(&widths);
    let preamble = rows
        .iter()
        .position(|row| row.len() == modal && row.iter().any(|f| !f.trim().is_empty()))
        .unwrap_or(0);
    if preamble > 0 {
        rows.drain(..preamble);
    }
    for row in rows.iter_mut() {
        while row.len() < modal {
            row.push(String::new());
        }
    }
    Csv { rows, delimiter, preamble }
}

/// Quote a field for writing: only when it has to be, the way every other
/// tool does it, so a round trip through this module is a no-op.
pub fn escape_field(field: &str, delimiter: Delimiter) -> String {
    let needs = field.contains(delimiter.byte() as char)
        || field.contains('"')
        || field.contains('\n')
        || field.contains('\r')
        || field.starts_with(' ')
        || field.ends_with(' ');
    if !needs {
        return field.to_string();
    }
    let mut out = String::with_capacity(field.len() + 2);
    out.push('"');
    for ch in field.chars() {
        if ch == '"' {
            out.push('"');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

/// Write rows back out as RFC 4180 (CRLF, as the spec says).
pub fn write(rows: &[Vec<String>], delimiter: Delimiter) -> String {
    let mut out = String::new();
    for row in rows {
        for (i, field) in row.iter().enumerate() {
            if i > 0 {
                out.push(delimiter.byte() as char);
            }
            out.push_str(&escape_field(field, delimiter));
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rfc4180_including_the_awkward_parts() {
        let text = "a,b,c\r\n1,\"two, with comma\",3\r\n4,\"say \"\"hi\"\"\",6\r\n";
        let csv = parse(text);
        assert_eq!(csv.delimiter, Delimiter::Comma);
        assert_eq!(csv.header(), ["a", "b", "c"]);
        assert_eq!(csv.records()[0][1], "two, with comma");
        assert_eq!(csv.records()[1][1], "say \"hi\"");
    }

    #[test]
    fn a_quoted_field_may_contain_a_newline() {
        let text = "date,memo\n2024-01-01,\"line one\nline two\"\n";
        let csv = parse(text);
        assert_eq!(csv.records().len(), 1);
        assert_eq!(csv.records()[0][1], "line one\nline two");
    }

    #[test]
    fn strips_the_bom_so_the_first_header_is_usable() {
        let text = "\u{feff}Date,Amount\n2024-01-01,10.00\n";
        let csv = parse(text);
        assert_eq!(csv.header()[0], "Date");
    }

    #[test]
    fn tells_a_semicolon_file_from_a_comma_one() {
        // German: semicolon delimited, commas INSIDE the numbers.
        let german = "Datum;Beschreibung;Betrag\n04.03.2024;Miete;-1.234,56\n05.03.2024;Lohn;2.500,00\n";
        assert_eq!(parse(german).delimiter, Delimiter::Semicolon);
        assert_eq!(parse(german).records()[0][2], "-1.234,56");

        // Comma delimited with commas inside quoted payees.
        let anglo = "Date,Payee,Amount\n2024-03-04,\"Smith, John\",-25.00\n2024-03-05,\"Doe, Jane\",30.00\n";
        assert_eq!(parse(anglo).delimiter, Delimiter::Comma);
        assert_eq!(parse(anglo).records()[0][1], "Smith, John");

        let tabbed = "Date\tPayee\tAmount\n2024-03-04\tRent\t-100\n";
        assert_eq!(parse(tabbed).delimiter, Delimiter::Tab);
    }

    #[test]
    fn drops_the_junk_a_bank_prints_above_the_table() {
        let text = "Account Statement\n\nAccount:,1234567890\nPeriod:,Jan 2024\n\n\
                    Date,Description,Amount,Balance\n\
                    2024-01-02,Coffee,-4.50,995.50\n\
                    2024-01-03,Salary,2000.00,2995.50\n";
        let csv = parse(text);
        assert_eq!(csv.header(), ["Date", "Description", "Amount", "Balance"]);
        assert_eq!(csv.records().len(), 2);
        assert!(csv.preamble > 0);
    }

    #[test]
    fn ragged_rows_are_padded_so_column_access_never_panics() {
        let text = "a,b,c\n1,2,3\n4,5\n";
        let csv = parse(text);
        assert_eq!(csv.records()[1].len(), 3);
        assert_eq!(csv.column(2).collect::<Vec<_>>(), ["3", ""]);
    }

    #[test]
    fn round_trips_through_write() {
        let rows = vec![
            vec!["Date".into(), "Payee".into(), "Amount".into()],
            vec!["2024-03-04".into(), "Smith, John".into(), "-25.00".into()],
            vec!["2024-03-05".into(), "say \"hi\"".into(), "30.00".into()],
        ];
        let text = write(&rows, Delimiter::Comma);
        let back = parse(&text);
        assert_eq!(back.rows, rows);
    }

    #[test]
    fn an_empty_or_single_line_file_does_not_panic() {
        assert!(parse("").rows.is_empty());
        assert_eq!(parse("just one line\n").rows.len(), 1);
    }
}
