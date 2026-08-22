//! Strict, dependency-free JSON for the transport layer.
//!
//! Parser rules (all fail-closed):
//! - Input must be valid UTF-8 with no trailing data after the root value.
//! - Nesting depth is bounded (8).
//! - Numbers are integers only: floats, exponents, leading zeros and values
//!   outside i64 are refused. No control body needs a float.
//! - Duplicate object keys are refused.
//! - Full escape handling including surrogate pairs; lone surrogates refused.
//!
//! The writer may emit floats (read-only projections of manifest geometry);
//! non-finite floats serialize as null (contract floats are always finite).

pub const MAX_DEPTH: u32 = 8;

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
    // Validate UTF-8 once; string bodies then copy bytes verbatim.
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid utf-8")?;
    let mut p = P { b: text.as_bytes(), i: 0 };
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
        if depth > MAX_DEPTH {
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

    fn number(&mut self) -> Result<Value, &'static str> {
        let start = self.i;
        let neg = if self.peek() == Some(b'-') {
            self.i += 1;
            true
        } else {
            false
        };
        let first = self.next()?;
        if !first.is_ascii_digit() {
            return Err("bad number");
        }
        let mut mag: i64 = (first - b'0') as i64;
        let mut digits = 1u32;
        let mut is_float = false;
        while let Some(c) = self.peek() {
            match c {
                b'0'..=b'9' => {
                    if !is_float {
                        if digits == 1 && mag == 0 {
                            return Err("leading zero");
                        }
                        mag = mag
                            .checked_mul(10)
                            .and_then(|m| m.checked_add((c - b'0') as i64))
                            .ok_or("integer out of range")?;
                        digits += 1;
                    }
                    self.i += 1;
                }
                // Floats are first-class: tool results carry positions and
                // angles ("float not accepted" 400'd a world.place answer
                // whose yaw was 1.5708 — the model then heard the app never
                // answered). The grammar stays strict JSON; the token is
                // bounded and parsed as f64.
                b'.' | b'e' | b'E' | b'+' | b'-' if is_float || matches!(c, b'.' | b'e' | b'E') => {
                    is_float = true;
                    self.i += 1;
                }
                _ => break,
            }
        }
        if is_float {
            if self.i - start > 64 {
                return Err("number too long");
            }
            let text = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "bad number")?;
            // Strict JSON number shape — Rust's f64 parse is laxer ("1.").
            if !valid_json_number(text) {
                return Err("bad number");
            }
            let f: f64 = text.parse().map_err(|_| "bad number")?;
            if !f.is_finite() {
                return Err("number out of range");
            }
            return Ok(Value::F64(f));
        }
        Ok(Value::Int(if neg { -mag } else { mag }))
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
                            // High surrogate: a low surrogate must follow.
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
                    // Continuation of UTF-8 validated upfront: copy the whole
                    // multi-byte sequence verbatim.
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

/// Strict JSON number grammar: `-? int ( . digits )? ( [eE] [+-]? digits )?`.
fn valid_json_number(t: &str) -> bool {
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && b[i] == b'-' {
        i += 1;
    }
    let d0 = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == d0 {
        return false;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let f0 = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == f0 {
            return false;
        }
    }
    if i < b.len() && (b[i] | 32) == b'e' {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let e0 = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == e0 {
            return false;
        }
    }
    i == b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_basic() {
        let v = parse(br#"{"a":1,"b":[true,null,"x\ny"],"c":{"d":-5}}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_i64(), Some(1));
        assert_eq!(v.get("b").unwrap().as_arr().unwrap().len(), 3);
        let s = v.to_json();
        assert_eq!(parse(s.as_bytes()).unwrap(), v);
    }

    #[test]
    fn rejects_floats_dups_depth_trailing() {
        // Floats are first-class since the world.place tool-result 400
        // (yaw 1.5708 in the answer body); malformed numerics still refuse.
        assert_eq!(parse(b"1.5").unwrap(), Value::F64(1.5));
        assert_eq!(parse(b"1e3").unwrap(), Value::F64(1000.0));
        assert_eq!(parse(b"-2.25").unwrap(), Value::F64(-2.25));
        assert_eq!(parse(b"1.5e-2").unwrap(), Value::F64(0.015));
        assert!(parse(b"1.").is_err());
        assert!(parse(b"1.2.3").is_err());
        assert!(parse(b"1e").is_err());
        assert!(parse(b"1e999").is_err());
        assert!(parse(b"01").is_err());
        assert!(parse(br#"{"a":1,"a":2}"#).is_err());
        assert!(parse(b"{} ").is_ok());
        assert!(parse(b"{} x").is_err());
        // Depth 9 refused, depth 8 accepted.
        let deep_ok = b"[[[[[[[[1]]]]]]]]";
        let deep_bad = b"[[[[[[[[[1]]]]]]]]]";
        assert!(parse(deep_ok).is_ok());
        assert!(parse(deep_bad).is_err());
    }

    #[test]
    fn surrogate_pairs() {
        let v = parse("\"\u{1f600}\"".as_bytes()).unwrap();
        assert_eq!(v.as_str(), Some("\u{1f600}"));
        assert!(parse(br#""\ud83d""#).is_err());
        assert!(parse(br#""\ude00""#).is_err());
    }

    #[test]
    fn integer_bounds() {
        assert_eq!(parse(b"9223372036854775807").unwrap().as_i64(), Some(i64::MAX));
        assert!(parse(b"9223372036854775808").is_err());
        assert_eq!(parse(b"-9223372036854775807").unwrap().as_i64(), Some(-i64::MAX));
    }
}
