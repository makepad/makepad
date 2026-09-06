//! Strict, dependency-free JSON for parsing server responses.
//!
//! Same fail-closed rules as the server transport's parser — valid UTF-8, no
//! trailing data, nesting depth bounded (8), duplicate object keys refused,
//! full escape handling with lone surrogates refused, leading zeros refused —
//! with one deliberate difference: this parser ACCEPTS floats, because the
//! server's read-only manifest projections emit them (bounds geometry). Float
//! syntax is still bounded: digit counts are capped and any non-finite result
//! (overflowing exponent) is refused, so no response can smuggle NaN/inf into
//! typed state.
//!
//! The writer half exists for request bodies (search) and test fixtures.

pub const MAX_DEPTH: u32 = 8;
const MAX_INT_DIGITS: usize = 19;
const MAX_FRAC_DIGITS: usize = 32;
const MAX_EXP_DIGITS: usize = 3;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    F64(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

/// Build an object from `(&str, Value)` pairs.
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

pub fn s(v: impl Into<String>) -> Value {
    Value::Str(v.into())
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Int(i) if *i >= 0 => Some(*i as u64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn write_into(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Int(i) => {
                use std::fmt::Write;
                let _ = write!(out, "{i}");
            }
            Value::F64(f) => {
                use std::fmt::Write;
                if f.is_finite() {
                    let _ = write!(out, "{f}");
                } else {
                    out.push_str("null");
                }
            }
            Value::Str(s) => escape_into(out, s),
            Value::Arr(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write_into(out);
                }
                out.push(']');
            }
            Value::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    escape_into(out, k);
                    out.push(':');
                    v.write_into(out);
                }
                out.push('}');
            }
        }
    }

    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_into(&mut out);
        out
    }
}

fn escape_into(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn parse(bytes: &[u8]) -> Result<Value, &'static str> {
    parse_depth(bytes, MAX_DEPTH)
}

/// `parse` with an explicit nesting cap, for bodies whose schema is known to
/// nest deeper than the default (a flow graph, for instance). Every other
/// rule is unchanged.
pub fn parse_depth(bytes: &[u8], max_depth: u32) -> Result<Value, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid utf-8")?;
    let mut p = P { b: text.as_bytes(), i: 0, max_depth };
    p.skip_ws();
    let v = p.value(0)?;
    p.skip_ws();
    if p.i != p.b.len() {
        return Err("trailing data");
    }
    Ok(v)
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
    max_depth: u32,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn next(&mut self) -> Result<u8, &'static str> {
        let c = self.b.get(self.i).copied().ok_or("unexpected end")?;
        self.i += 1;
        Ok(c)
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, c: u8) -> Result<(), &'static str> {
        if self.next()? == c {
            Ok(())
        } else {
            Err("unexpected character")
        }
    }

    fn expect_word(&mut self, w: &[u8]) -> Result<(), &'static str> {
        for &c in w {
            self.expect(c)?;
        }
        Ok(())
    }

    fn value(&mut self, depth: u32) -> Result<Value, &'static str> {
        if depth > self.max_depth {
            return Err("nesting too deep");
        }
        match self.peek().ok_or("unexpected end")? {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => Ok(Value::Str(self.string()?)),
            b't' => {
                self.expect_word(b"true")?;
                Ok(Value::Bool(true))
            }
            b'f' => {
                self.expect_word(b"false")?;
                Ok(Value::Bool(false))
            }
            b'n' => {
                self.expect_word(b"null")?;
                Ok(Value::Null)
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err("unexpected character"),
        }
    }

    fn object(&mut self, depth: u32) -> Result<Value, &'static str> {
        self.expect(b'{')?;
        let mut pairs: Vec<(String, Value)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Obj(pairs));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            if pairs.iter().any(|(k, _)| *k == key) {
                return Err("duplicate key");
            }
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let v = self.value(depth + 1)?;
            pairs.push((key, v));
            self.skip_ws();
            match self.next()? {
                b',' => continue,
                b'}' => return Ok(Value::Obj(pairs)),
                _ => return Err("expected , or }"),
            }
        }
    }

    fn array(&mut self, depth: u32) -> Result<Value, &'static str> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth + 1)?);
            self.skip_ws();
            match self.next()? {
                b',' => continue,
                b']' => return Ok(Value::Arr(items)),
                _ => return Err("expected , or ]"),
            }
        }
    }

    /// Bounded number: integer digits are capped and canonical (no leading
    /// zero); an optional fraction and exponent turn the value into an F64.
    /// Any non-finite parse result is refused.
    fn number(&mut self) -> Result<Value, &'static str> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let first = self.next()?;
        if !first.is_ascii_digit() {
            return Err("bad number");
        }
        let mut int_digits = 1usize;
        while let Some(c) = self.peek() {
            if !c.is_ascii_digit() {
                break;
            }
            if int_digits == 1 && first == b'0' {
                return Err("leading zero");
            }
            int_digits += 1;
            if int_digits > MAX_INT_DIGITS {
                return Err("integer too long");
            }
            self.i += 1;
        }
        let mut is_float = false;
        if self.peek() == Some(b'.') {
            is_float = true;
            self.i += 1;
            let mut frac_digits = 0usize;
            while let Some(c) = self.peek() {
                if !c.is_ascii_digit() {
                    break;
                }
                frac_digits += 1;
                if frac_digits > MAX_FRAC_DIGITS {
                    return Err("fraction too long");
                }
                self.i += 1;
            }
            if frac_digits == 0 {
                return Err("bad number");
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            is_float = true;
            self.i += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.i += 1;
            }
            let mut exp_digits = 0usize;
            while let Some(c) = self.peek() {
                if !c.is_ascii_digit() {
                    break;
                }
                exp_digits += 1;
                if exp_digits > MAX_EXP_DIGITS {
                    return Err("exponent too long");
                }
                self.i += 1;
            }
            if exp_digits == 0 {
                return Err("bad number");
            }
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "bad number")?;
        if is_float {
            let f: f64 = text.parse().map_err(|_| "bad number")?;
            if !f.is_finite() {
                return Err("non-finite number");
            }
            Ok(Value::F64(f))
        } else {
            let i: i64 = text.parse().map_err(|_| "integer out of range")?;
            Ok(Value::Int(i))
        }
    }

    fn string(&mut self) -> Result<String, &'static str> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let c = self.next()?;
            match c {
                b'"' => return Ok(out),
                b'\\' => match self.next()? {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{08}'),
                    b'f' => out.push('\u{0c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let hi = self.hex4()?;
                        let cp = if (0xd800..0xdc00).contains(&hi) {
                            self.expect(b'\\')?;
                            self.expect(b'u')?;
                            let lo = self.hex4()?;
                            if !(0xdc00..0xe000).contains(&lo) {
                                return Err("lone surrogate");
                            }
                            0x10000 + ((hi - 0xd800) << 10) + (lo - 0xdc00)
                        } else if (0xdc00..0xe000).contains(&hi) {
                            return Err("lone surrogate");
                        } else {
                            hi
                        };
                        out.push(char::from_u32(cp).ok_or("bad escape")?);
                    }
                    _ => return Err("bad escape"),
                },
                c if c < 0x20 => return Err("control in string"),
                c if c < 0x80 => out.push(c as char),
                c => {
                    // UTF-8 was validated upfront: copy the multi-byte
                    // sequence verbatim.
                    let len = match c {
                        0xc0..=0xdf => 2,
                        0xe0..=0xef => 3,
                        _ => 4,
                    };
                    let start = self.i - 1;
                    for _ in 1..len {
                        self.next()?;
                    }
                    out.push_str(
                        std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "invalid utf-8")?,
                    );
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, &'static str> {
        let mut v = 0u32;
        for _ in 0..4 {
            let c = self.next()?;
            let d = match c {
                b'0'..=b'9' => (c - b'0') as u32,
                b'a'..=b'f' => (c - b'a' + 10) as u32,
                b'A'..=b'F' => (c - b'A' + 10) as u32,
                _ => return Err("bad unicode escape"),
            };
            v = (v << 4) | d;
        }
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_basic() {
        let v = parse(br#"{"a":1,"b":[true,null,"x\ny"],"c":{"d":-5}}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_i64(), Some(1));
        assert_eq!(v.get("b").unwrap().as_arr().unwrap().len(), 3);
        let text = v.to_json();
        assert_eq!(parse(text.as_bytes()).unwrap(), v);
    }

    #[test]
    fn floats_accepted_bounded() {
        assert_eq!(parse(b"1.5").unwrap(), Value::F64(1.5));
        assert_eq!(parse(b"-0.25").unwrap(), Value::F64(-0.25));
        assert_eq!(parse(b"2e3").unwrap(), Value::F64(2000.0));
        // Overflowing exponent digits refused before evaluation.
        assert!(parse(b"1e9999").is_err());
        // In-range exponent producing infinity refused after evaluation.
        assert!(parse(b"9e999").is_err());
        assert!(parse(b"1.").is_err());
        assert!(parse(b"1e").is_err());
        assert!(parse(b".5").is_err());
        let long_frac = format!("0.{}", "1".repeat(64));
        assert!(parse(long_frac.as_bytes()).is_err());
    }

    #[test]
    fn rejects_dups_depth_trailing_leading_zero() {
        assert!(parse(b"01").is_err());
        assert!(parse(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse(b"{} ").is_ok());
        assert!(parse(b"{} x").is_err());
        let deep_ok = b"[[[[[[[[1]]]]]]]]";
        let deep_bad = b"[[[[[[[[[1]]]]]]]]]";
        assert!(parse(deep_ok).is_ok());
        assert!(parse(deep_bad).is_err());
    }

    #[test]
    fn surrogate_pairs_and_controls() {
        let v = parse("\"😀\"".as_bytes()).unwrap();
        assert_eq!(v.as_str(), Some("\u{1f600}"));
        assert!(parse(br#""\ud83d""#).is_err());
        assert!(parse(br#""\ude00""#).is_err());
        assert!(parse(b"\"a\x01b\"").is_err());
        assert!(parse(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn integer_bounds() {
        assert_eq!(parse(b"9223372036854775807").unwrap().as_i64(), Some(i64::MAX));
        assert!(parse(b"9223372036854775808").is_err());
        assert_eq!(parse(b"-42").unwrap().as_i64(), Some(-42));
        assert_eq!(parse(b"-42").unwrap().as_u64(), None);
    }
}
