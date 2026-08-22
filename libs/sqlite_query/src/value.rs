//! SQLite value model, record (row) codec, sort order, affinity and collation.
//!
//! Byte layout follows <https://www.sqlite.org/fileformat.html> section 2.1
//! ("Record Format"). Nothing here is invented: serial types, varints and the
//! big-endian integer widths are the on-disk format as documented.

use crate::error::{Error, Result};
use std::cmp::Ordering;

// ---------------------------------------------------------------------------
// Value
// ---------------------------------------------------------------------------

/// One SQLite value. The five storage classes of the format.
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn text(s: impl Into<String>) -> Value {
        Value::Text(s.into())
    }
    pub fn blob(b: impl Into<Vec<u8>>) -> Value {
        Value::Blob(b.into())
    }
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            Value::Integer(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_real(&self) -> Option<f64> {
        match self {
            Value::Real(v) => Some(*v),
            Value::Integer(v) => Some(*v as f64),
            _ => None,
        }
    }
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_blob(&self) -> Option<&[u8]> {
        match self {
            Value::Blob(b) => Some(b),
            _ => None,
        }
    }
    pub fn into_blob(self) -> Option<Vec<u8>> {
        match self {
            Value::Blob(b) => Some(b),
            _ => None,
        }
    }
    /// Storage-class rank used by SQLite's sort order:
    /// NULL < numeric < text < blob.
    pub fn class(&self) -> u8 {
        match self {
            Value::Null => 0,
            Value::Integer(_) | Value::Real(_) => 1,
            Value::Text(_) => 2,
            Value::Blob(_) => 3,
        }
    }
    /// Truthiness for WHERE/CHECK: NULL is unknown (None), numbers are != 0,
    /// text/blobs convert with SQLite's prefix-number rules.
    pub fn truth(&self) -> Option<bool> {
        match self {
            Value::Null => None,
            Value::Integer(v) => Some(*v != 0),
            Value::Real(v) => Some(*v != 0.0),
            Value::Text(s) => Some(text_to_number_prefix(s.as_bytes()) != 0.0),
            Value::Blob(b) => Some(text_to_number_prefix(b) != 0.0),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        compare(self, other, Collation::Binary) == Ordering::Equal
    }
}

// ---------------------------------------------------------------------------
// Varint
// ---------------------------------------------------------------------------

/// Read a SQLite varint (big-endian, 1-9 bytes, high bit = continuation).
/// Returns (value, bytes consumed).
pub fn read_varint(buf: &[u8]) -> Result<(u64, usize)> {
    let mut v: u64 = 0;
    for i in 0..8 {
        let c = *buf
            .get(i)
            .ok_or_else(|| Error::corrupt("varint runs past the end of its buffer"))?;
        v = (v << 7) | (c & 0x7f) as u64;
        if c & 0x80 == 0 {
            return Ok((v, i + 1));
        }
    }
    let c = *buf
        .get(8)
        .ok_or_else(|| Error::corrupt("9-byte varint runs past the end of its buffer"))?;
    v = (v << 8) | c as u64;
    Ok((v, 9))
}

/// Append a SQLite varint. Returns the number of bytes written.
pub fn write_varint(out: &mut Vec<u8>, value: u64) -> usize {
    if value > 0x00ff_ffff_ffff_ffff {
        // 9-byte form: 8 groups of 7 bits then a full byte.
        let mut buf = [0u8; 9];
        buf[8] = (value & 0xff) as u8;
        let mut v = value >> 8;
        for i in (0..8).rev() {
            buf[i] = ((v & 0x7f) as u8) | 0x80;
            v >>= 7;
        }
        out.extend_from_slice(&buf);
        return 9;
    }
    let mut tmp = [0u8; 9];
    let mut n = 0;
    let mut v = value;
    loop {
        tmp[n] = (v & 0x7f) as u8;
        v >>= 7;
        n += 1;
        if v == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        let last = i == 0;
        out.push(if last { tmp[i] } else { tmp[i] | 0x80 });
    }
    n
}

pub fn varint_len(value: u64) -> usize {
    if value > 0x00ff_ffff_ffff_ffff {
        return 9;
    }
    let mut n = 1;
    let mut v = value >> 7;
    while v != 0 {
        n += 1;
        v >>= 7;
    }
    n
}

// ---------------------------------------------------------------------------
// Big-endian helpers (bounds-checked)
// ---------------------------------------------------------------------------

pub fn be_u16(buf: &[u8], at: usize) -> Result<u16> {
    let b = buf
        .get(at..at + 2)
        .ok_or_else(|| Error::corrupt("16-bit read past end of page"))?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

pub fn be_u32(buf: &[u8], at: usize) -> Result<u32> {
    let b = buf
        .get(at..at + 4)
        .ok_or_else(|| Error::corrupt("32-bit read past end of page"))?;
    Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

// ---------------------------------------------------------------------------
// Serial types
// ---------------------------------------------------------------------------

/// Number of content bytes a serial type occupies in the record body.
pub fn serial_type_size(serial_type: u64) -> usize {
    match serial_type {
        0 | 8 | 9 | 10 | 11 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 | 7 => 8,
        n if n % 2 == 0 => ((n - 12) / 2) as usize,
        n => ((n - 13) / 2) as usize,
    }
}

/// How text bytes are encoded in this database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}

/// What to do with text bytes that are not valid in the database encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextMode {
    /// Replace malformed sequences (historical mbtiles reader behavior).
    Lossy,
    /// Report [`Error::Corrupt`]. Used by the SQL engine so a row can never be
    /// silently rewritten with different bytes than it had.
    Strict,
}

fn decode_text(buf: &[u8], enc: TextEncoding, mode: TextMode) -> Result<String> {
    match enc {
        TextEncoding::Utf8 => match std::str::from_utf8(buf) {
            Ok(s) => Ok(s.to_string()),
            Err(_) if mode == TextMode::Lossy => Ok(String::from_utf8_lossy(buf).into_owned()),
            Err(_) => Err(Error::corrupt("TEXT value is not valid UTF-8")),
        },
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            if buf.len() % 2 != 0 {
                if mode == TextMode::Lossy {
                    return Ok(String::new());
                }
                return Err(Error::corrupt("UTF-16 TEXT value has an odd byte length"));
            }
            let units: Vec<u16> = buf
                .chunks_exact(2)
                .map(|c| {
                    if enc == TextEncoding::Utf16Le {
                        u16::from_le_bytes([c[0], c[1]])
                    } else {
                        u16::from_be_bytes([c[0], c[1]])
                    }
                })
                .collect();
            match String::from_utf16(&units) {
                Ok(s) => Ok(s),
                Err(_) if mode == TextMode::Lossy => Ok(String::from_utf16_lossy(&units)),
                Err(_) => Err(Error::corrupt("TEXT value is not valid UTF-16")),
            }
        }
    }
}

fn decode_value(serial_type: u64, body: &[u8], enc: TextEncoding, mode: TextMode) -> Result<Value> {
    let size = serial_type_size(serial_type);
    let b = body
        .get(..size)
        .ok_or_else(|| Error::corrupt("record value extends past the payload"))?;
    Ok(match serial_type {
        0 => Value::Null,
        1 => Value::Integer(b[0] as i8 as i64),
        2 => Value::Integer(i16::from_be_bytes([b[0], b[1]]) as i64),
        3 => {
            let sign = if b[0] & 0x80 != 0 { 0xff } else { 0x00 };
            Value::Integer(i32::from_be_bytes([sign, b[0], b[1], b[2]]) as i64)
        }
        4 => Value::Integer(i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64),
        5 => {
            let sign = if b[0] & 0x80 != 0 { 0xff } else { 0x00 };
            Value::Integer(i64::from_be_bytes([
                sign, sign, b[0], b[1], b[2], b[3], b[4], b[5],
            ]))
        }
        6 => Value::Integer(i64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ])),
        7 => Value::Real(f64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ])),
        8 => Value::Integer(0),
        9 => Value::Integer(1),
        10 | 11 => return Err(Error::corrupt("reserved serial type 10/11 in a record")),
        n if n % 2 == 0 => Value::Blob(b.to_vec()),
        _ => Value::Text(decode_text(b, enc, mode)?),
    })
}

// ---------------------------------------------------------------------------
// Record codec
// ---------------------------------------------------------------------------

/// Decode a record (row payload) into its column values.
pub fn parse_record(payload: &[u8], enc: TextEncoding, mode: TextMode) -> Result<Vec<Value>> {
    if payload.is_empty() {
        return Ok(Vec::new());
    }
    let (header_size, hdr_len) = read_varint(payload)?;
    let header_size = usize::try_from(header_size)
        .map_err(|_| Error::corrupt("record header size exceeds address space"))?;
    if header_size > payload.len() || header_size < hdr_len {
        return Err(Error::corrupt("record header size exceeds the payload"));
    }
    let mut serials = Vec::new();
    let mut hpos = hdr_len;
    while hpos < header_size {
        let (st, n) = read_varint(
            payload
                .get(hpos..header_size)
                .ok_or_else(|| Error::corrupt("record header truncated"))?,
        )?;
        serials.push(st);
        hpos += n;
    }
    let mut pos = header_size;
    let mut out = Vec::with_capacity(serials.len());
    for st in serials {
        let size = serial_type_size(st);
        let body = payload
            .get(pos..)
            .ok_or_else(|| Error::corrupt("record body truncated"))?;
        out.push(decode_value(st, body, enc, mode)?);
        pos += size;
    }
    Ok(out)
}

/// Decode only the first `n` columns of a record; cheaper than a full decode
/// when a scan filters on leading columns.
pub fn parse_record_prefix(
    payload: &[u8],
    n: usize,
    enc: TextEncoding,
    mode: TextMode,
) -> Result<Vec<Value>> {
    if payload.is_empty() || n == 0 {
        return Ok(Vec::new());
    }
    let (header_size, hdr_len) = read_varint(payload)?;
    let header_size = usize::try_from(header_size)
        .map_err(|_| Error::corrupt("record header size exceeds address space"))?;
    if header_size > payload.len() || header_size < hdr_len {
        return Err(Error::corrupt("record header size exceeds the payload"));
    }
    let mut hpos = hdr_len;
    let mut pos = header_size;
    let mut out = Vec::with_capacity(n);
    while hpos < header_size && out.len() < n {
        let (st, used) = read_varint(&payload[hpos..header_size])?;
        hpos += used;
        let size = serial_type_size(st);
        let body = payload
            .get(pos..)
            .ok_or_else(|| Error::corrupt("record body truncated"))?;
        out.push(decode_value(st, body, enc, mode)?);
        pos += size;
    }
    Ok(out)
}

fn serial_type_for(v: &Value) -> (u64, usize) {
    match v {
        Value::Null => (0, 0),
        Value::Integer(i) => {
            let i = *i;
            if i == 0 {
                (8, 0)
            } else if i == 1 {
                (9, 0)
            } else if i >= -128 && i <= 127 {
                (1, 1)
            } else if i >= -32768 && i <= 32767 {
                (2, 2)
            } else if i >= -8_388_608 && i <= 8_388_607 {
                (3, 3)
            } else if i >= -2_147_483_648 && i <= 2_147_483_647 {
                (4, 4)
            } else if i >= -140_737_488_355_328 && i <= 140_737_488_355_327 {
                (5, 6)
            } else {
                (6, 8)
            }
        }
        Value::Real(_) => (7, 8),
        Value::Text(s) => (13 + 2 * s.as_bytes().len() as u64, s.as_bytes().len()),
        Value::Blob(b) => (12 + 2 * b.len() as u64, b.len()),
    }
}

fn push_body(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Null => {}
        Value::Integer(i) => {
            let (st, size) = serial_type_for(v);
            let be = i.to_be_bytes();
            match st {
                8 | 9 => {}
                _ => out.extend_from_slice(&be[8 - size..]),
            }
        }
        Value::Real(f) => out.extend_from_slice(&f.to_be_bytes()),
        Value::Text(s) => out.extend_from_slice(s.as_bytes()),
        Value::Blob(b) => out.extend_from_slice(b),
    }
}

/// Encode column values into a record payload, exactly as SQLite would
/// (minimal integer widths, serial types 8/9 for the constants 0 and 1).
/// Only UTF-8 databases are written by this engine.
pub fn encode_record(values: &[Value]) -> Vec<u8> {
    let mut header = Vec::with_capacity(values.len() + 1);
    let mut body_len = 0usize;
    for v in values {
        let (st, size) = serial_type_for(v);
        write_varint(&mut header, st);
        body_len += size;
    }
    // The header size varint counts itself: solve the fixed point.
    let mut hdr_size_len = varint_len((header.len() + 1) as u64);
    loop {
        let total = header.len() + hdr_size_len;
        let n = varint_len(total as u64);
        if n == hdr_size_len {
            break;
        }
        hdr_size_len = n;
    }
    let mut out = Vec::with_capacity(header.len() + hdr_size_len + body_len);
    write_varint(&mut out, (header.len() + hdr_size_len) as u64);
    out.extend_from_slice(&header);
    for v in values {
        push_body(&mut out, v);
    }
    out
}

// ---------------------------------------------------------------------------
// Collation and comparison
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collation {
    Binary,
    NoCase,
    RTrim,
}

impl Collation {
    pub fn from_name(name: &str) -> Option<Collation> {
        match name.to_ascii_uppercase().as_str() {
            "BINARY" => Some(Collation::Binary),
            "NOCASE" => Some(Collation::NoCase),
            "RTRIM" => Some(Collation::RTrim),
            _ => None,
        }
    }
    pub fn compare(self, a: &str, b: &str) -> Ordering {
        match self {
            Collation::Binary => a.as_bytes().cmp(b.as_bytes()),
            Collation::NoCase => {
                let mut ai = a.bytes();
                let mut bi = b.bytes();
                loop {
                    match (ai.next(), bi.next()) {
                        (None, None) => return Ordering::Equal,
                        (None, Some(_)) => return Ordering::Less,
                        (Some(_), None) => return Ordering::Greater,
                        (Some(x), Some(y)) => {
                            let x = x.to_ascii_lowercase();
                            let y = y.to_ascii_lowercase();
                            if x != y {
                                return x.cmp(&y);
                            }
                        }
                    }
                }
            }
            Collation::RTrim => {
                let at = a.trim_end_matches(' ');
                let bt = b.trim_end_matches(' ');
                at.as_bytes().cmp(bt.as_bytes())
            }
        }
    }
}

fn cmp_int_real(i: i64, f: f64) -> Ordering {
    if f.is_nan() {
        return Ordering::Greater; // NaN sorts before every number
    }
    if f >= 9_223_372_036_854_775_808.0 {
        return Ordering::Less;
    }
    if f < -9_223_372_036_854_775_808.0 {
        return Ordering::Greater;
    }
    let fl = f.floor();
    let fi = fl as i64;
    match i.cmp(&fi) {
        Ordering::Equal => {
            if f > fl {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        }
        o => o,
    }
}

/// SQLite's total sort order across storage classes.
pub fn compare(a: &Value, b: &Value, coll: Collation) -> Ordering {
    let (ca, cb) = (a.class(), b.class());
    if ca != cb {
        return ca.cmp(&cb);
    }
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Integer(x), Value::Integer(y)) => x.cmp(y),
        (Value::Real(x), Value::Real(y)) => x.partial_cmp(y).unwrap_or_else(|| {
            // Only reachable for NaN, which SQLite stores as NULL.
            if x.is_nan() && y.is_nan() {
                Ordering::Equal
            } else if x.is_nan() {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }),
        (Value::Integer(x), Value::Real(y)) => cmp_int_real(*x, *y),
        (Value::Real(x), Value::Integer(y)) => cmp_int_real(*y, *x).reverse(),
        (Value::Text(x), Value::Text(y)) => coll.compare(x, y),
        (Value::Blob(x), Value::Blob(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

/// Compare two records column by column (index key order). Shorter keys that
/// are a prefix of the longer one compare Less, as in a b-tree descent.
pub fn compare_records(a: &[Value], b: &[Value], colls: &[Collation]) -> Ordering {
    let n = a.len().min(b.len());
    for i in 0..n {
        let coll = colls.get(i).copied().unwrap_or(Collation::Binary);
        match compare(&a[i], &b[i], coll) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    Ordering::Equal
}

// ---------------------------------------------------------------------------
// Affinity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    /// "NONE" in the documentation: values are stored exactly as given.
    Blob,
    Text,
    Numeric,
    Integer,
    Real,
}

/// Column affinity from a declared type, per the documented rules
/// (<https://www.sqlite.org/datatype3.html> section 3.1).
pub fn affinity_of(decl_type: &str) -> Affinity {
    let t = decl_type.to_ascii_uppercase();
    if t.contains("INT") {
        Affinity::Integer
    } else if t.contains("CHAR") || t.contains("CLOB") || t.contains("TEXT") {
        Affinity::Text
    } else if t.contains("BLOB") || t.is_empty() {
        Affinity::Blob
    } else if t.contains("REAL") || t.contains("FLOA") || t.contains("DOUB") {
        Affinity::Real
    } else {
        Affinity::Numeric
    }
}

/// Parse a leading number out of text the way SQLite's CAST does; returns 0.0
/// when there is no numeric prefix.
pub fn text_to_number_prefix(bytes: &[u8]) -> f64 {
    let s = String::from_utf8_lossy(bytes);
    let t = s.trim_start();
    let mut end = 0;
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut seen_digit = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        seen_digit = true;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            seen_digit = true;
        }
    }
    if seen_digit {
        end = i;
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            let mut j = i + 1;
            if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                j += 1;
            }
            let mut expd = false;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
                expd = true;
            }
            if expd {
                end = j;
            }
        }
    }
    if end == 0 {
        return 0.0;
    }
    t[..end].parse::<f64>().unwrap_or(0.0)
}

/// True when the whole string is a well-formed SQL numeric literal.
fn looks_numeric(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    let b = t.as_bytes();
    let mut i = 0;
    if b[i] == b'+' || b[i] == b'-' {
        i += 1;
    }
    let mut digits = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits = true;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits = true;
        }
    }
    if !digits {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let mut expd = false;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            expd = true;
        }
        if !expd {
            return false;
        }
    }
    i == b.len()
}

/// Convert text that looks fully numeric into INTEGER or REAL; leave anything
/// else untouched. This is SQLite's "apply numeric affinity" step.
pub fn apply_numeric_affinity(v: Value) -> Value {
    match v {
        Value::Text(ref s) if looks_numeric(s) => {
            let t = s.trim();
            if let Ok(i) = t.parse::<i64>() {
                Value::Integer(i)
            } else if let Ok(f) = t.parse::<f64>() {
                // REAL that is exactly an integer stays REAL, like SQLite.
                Value::Real(f)
            } else {
                v
            }
        }
        other => other,
    }
}

/// Apply a column's affinity to a value being stored or compared.
pub fn apply_affinity(v: Value, aff: Affinity) -> Value {
    match aff {
        Affinity::Blob => v,
        Affinity::Text => match v {
            Value::Integer(i) => Value::Text(i.to_string()),
            Value::Real(f) => Value::Text(format_real(f)),
            other => other,
        },
        Affinity::Numeric => apply_numeric_affinity(v),
        Affinity::Integer => match apply_numeric_affinity(v) {
            Value::Real(f) if f.floor() == f && f.abs() < 9.223_372_036_854_776e18 => {
                Value::Integer(f as i64)
            }
            other => other,
        },
        Affinity::Real => match apply_numeric_affinity(v) {
            Value::Integer(i) => Value::Real(i as f64),
            other => other,
        },
    }
}

/// Render a REAL the way SQLite's default output does (15 significant digits,
/// trailing ".0" for integral values).
pub fn format_real(f: f64) -> String {
    if f.is_infinite() {
        return if f > 0.0 { "Inf".into() } else { "-Inf".into() };
    }
    if f.is_nan() {
        return String::new();
    }
    let mut s = format!("{:.15e}", f);
    // Convert the exponent form back to SQLite's %!.15g rendering.
    if let Some(epos) = s.find('e') {
        let exp: i32 = s[epos + 1..].parse().unwrap_or(0);
        if exp >= -4 && exp < 15 {
            let digits = 15 - exp;
            s = format!("{:.*}", digits.max(0) as usize, f);
            while s.contains('.') && s.ends_with('0') {
                s.pop();
            }
            if s.ends_with('.') {
                s.push('0');
            }
            return s;
        }
        let mantissa = &s[..epos];
        let mut m = mantissa.to_string();
        while m.contains('.') && m.ends_with('0') {
            m.pop();
        }
        if m.ends_with('.') {
            m.pop();
        }
        return format!("{m}e{}{:02}", if exp < 0 { "-" } else { "+" }, exp.abs());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_roundtrip() {
        for v in [
            0u64,
            1,
            127,
            128,
            300,
            16383,
            16384,
            u32::MAX as u64,
            (1u64 << 56) - 1,
            1u64 << 56,
            u64::MAX,
        ] {
            let mut buf = Vec::new();
            let n = write_varint(&mut buf, v);
            assert_eq!(n, buf.len());
            assert_eq!(varint_len(v), n, "len mismatch for {v}");
            assert_eq!(read_varint(&buf).unwrap(), (v, n), "roundtrip {v}");
        }
    }

    #[test]
    fn varint_truncated_is_error() {
        assert!(read_varint(&[]).is_err());
        assert!(read_varint(&[0x81]).is_err());
        assert!(read_varint(&[0x81; 8]).is_err());
    }

    #[test]
    fn record_roundtrip() {
        let vals = vec![
            Value::Null,
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(-1),
            Value::Integer(i64::MIN),
            Value::Integer(1 << 40),
            Value::Real(1.5),
            Value::Text("hello".into()),
            Value::Blob(vec![0, 1, 2, 3]),
        ];
        let enc = encode_record(&vals);
        let back = parse_record(&enc, TextEncoding::Utf8, TextMode::Strict).unwrap();
        assert_eq!(back.len(), vals.len());
        for (a, b) in vals.iter().zip(back.iter()) {
            assert_eq!(compare(a, b, Collation::Binary), Ordering::Equal, "{a:?}");
        }
    }

    #[test]
    fn record_prefix_matches_full() {
        let vals = vec![
            Value::Integer(7),
            Value::Text("abc".into()),
            Value::Blob(vec![9; 100]),
        ];
        let enc = encode_record(&vals);
        let p = parse_record_prefix(&enc, 2, TextEncoding::Utf8, TextMode::Strict).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].as_text(), Some("abc"));
    }

    #[test]
    fn sort_order_classes() {
        let order = [
            Value::Null,
            Value::Integer(-5),
            Value::Real(0.5),
            Value::Integer(9),
            Value::Text("a".into()),
            Value::Blob(vec![0]),
        ];
        for i in 0..order.len() {
            for j in 0..order.len() {
                let want = i.cmp(&j);
                let got = compare(&order[i], &order[j], Collation::Binary);
                assert_eq!(got, want, "{:?} vs {:?}", order[i], order[j]);
            }
        }
    }

    #[test]
    fn int_real_exact_compare() {
        assert_eq!(
            compare(
                &Value::Integer(9007199254740993),
                &Value::Real(9007199254740992.0),
                Collation::Binary
            ),
            Ordering::Greater
        );
        assert_eq!(
            compare(&Value::Integer(1), &Value::Real(1.5), Collation::Binary),
            Ordering::Less
        );
        assert_eq!(
            compare(&Value::Integer(2), &Value::Real(1.5), Collation::Binary),
            Ordering::Greater
        );
    }

    #[test]
    fn affinities() {
        assert_eq!(affinity_of("INTEGER"), Affinity::Integer);
        assert_eq!(affinity_of("VARCHAR(10)"), Affinity::Text);
        assert_eq!(affinity_of("BLOB"), Affinity::Blob);
        assert_eq!(affinity_of(""), Affinity::Blob);
        assert_eq!(affinity_of("DOUBLE"), Affinity::Real);
        assert_eq!(affinity_of("DECIMAL(10,5)"), Affinity::Numeric);
        assert_eq!(affinity_of("FLOATING POINT"), Affinity::Integer); // documented quirk: contains INT
    }

    #[test]
    fn real_formatting() {
        assert_eq!(format_real(1.0), "1.0");
        assert_eq!(format_real(1.5), "1.5");
        assert_eq!(format_real(-0.25), "-0.25");
    }
}
