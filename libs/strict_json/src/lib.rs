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
                    // Debug uses a shortest round-trippable representation,
                    // including scientific notation for extreme magnitudes.
                    // Display expands tiny/large floats past our digit caps.
                    let _ = write!(out, "{f:?}");
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
    parse_with(bytes, max_depth, false)
}

/// `parse_depth` for files written by other tools that repeat a key in one
/// object: the last value of a repeated key replaces the earlier one (in its
/// place). Every other rule is unchanged.
pub fn parse_depth_last_key_wins(bytes: &[u8], max_depth: u32) -> Result<Value, &'static str> {
    parse_with(bytes, max_depth, true)
}

fn parse_with(bytes: &[u8], max_depth: u32, last_key_wins: bool) -> Result<Value, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid utf-8")?;
    let mut p = P { b: text.as_bytes(), i: 0, max_depth, last_key_wins };
    p.skip_ws();
    let v = p.value(0)?;
    p.skip_ws();
    if p.i != p.b.len() {
        return Err("trailing data");
    }
    Ok(v)
}

/// One component of a value's path from the root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step<'a> {
    Key(&'a str),
    Index(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Keep the entire value, including all descendants.
    Keep,
    /// Visit children of a container; omit scalar values.
    Open,
    /// Validate and omit the entire value without allocating it.
    Skip,
    /// Keep a string only; validate and omit any other value.
    KeepString,
}

/// Project selected values while validating the entire input. The callback is
/// called at the root and each child of an opened container, once per value.
/// Kept and skipped subtrees need no further picks. A skipped root is `Null`.
/// Array positions in paths refer to the input, before omitted elements.
pub fn parse_picked(
    bytes: &[u8],
    max_depth: u32,
    last_key_wins: bool,
    pick: &dyn Fn(&[Step<'_>]) -> Pick,
) -> Result<Value, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid utf-8")?;
    let mut p = P { b: text.as_bytes(), i: 0, max_depth, last_key_wins };
    p.skip_ws();
    let value = p.picked(0, &[], pick)?.unwrap_or(Value::Null);
    p.skip_ws();
    if p.i != p.b.len() {
        return Err("trailing data");
    }
    Ok(value)
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
    max_depth: u32,
    last_key_wins: bool,
}

struct StringChars<'a> {
    p: P<'a>,
    done: bool,
}

impl Iterator for StringChars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        if self.done { return None; }
        let c = self.p.next().ok()?;
        let c = match c {
            b'"' => { self.done = true; return None; }
            b'\\' => match self.p.next().ok()? {
                b'"' => '"', b'\\' => '\\', b'/' => '/',
                b'b' => '\u{08}', b'f' => '\u{0c}', b'n' => '\n', b'r' => '\r', b't' => '\t',
                b'u' => {
                    let hi = self.p.hex4().ok()?;
                    let cp = if (0xd800..0xdc00).contains(&hi) {
                        self.p.expect(b'\\').ok()?;
                        self.p.expect(b'u').ok()?;
                        0x10000 + ((hi - 0xd800) << 10) + (self.p.hex4().ok()? - 0xdc00)
                    } else { hi };
                    char::from_u32(cp)?
                }
                _ => return None,
            },
            c if c < 0x80 => c as char,
            _ => {
                let len = match c { 0xc0..=0xdf => 2, 0xe0..=0xef => 3, _ => 4 };
                let start = self.p.i - 1;
                self.p.i = start + len;
                std::str::from_utf8(&self.p.b[start..self.p.i]).ok()?.chars().next()?
            }
        };
        Some(c)
    }
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

    fn picked(
        &mut self,
        depth: u32,
        path: &[Step<'_>],
        pick: &dyn Fn(&[Step<'_>]) -> Pick,
    ) -> Result<Option<Value>, &'static str> {
        if depth > self.max_depth {
            return Err("nesting too deep");
        }
        match pick(path) {
            Pick::Keep => return self.value(depth).map(Some),
            Pick::KeepString if self.peek() == Some(b'"') => return self.value(depth).map(Some),
            Pick::Open if matches!(self.peek(), Some(b'{') | Some(b'[')) => {}
            _ => {
                self.skip_value(depth)?;
                return Ok(None);
            }
        }
        let object = self.next()? == b'{';
        let start = self.i;
        let end = if object { b'}' } else { b']' };
        let mut pairs: Vec<(usize, String, Value)> = Vec::new();
        let mut items = Vec::new();
        let mut index = 0;
        self.skip_ws();
        if self.peek() != Some(end) {
            loop {
                self.skip_ws();
                let mut child_path = path.to_vec();
                let key;
                let key_start = self.i;
                if object {
                    key = self.string()?;
                    if !self.last_key_wins && self.first_key(start, key_start, depth)?.is_some() {
                        return Err("duplicate key");
                    }
                    self.skip_ws();
                    self.expect(b':')?;
                    child_path.push(Step::Key(&key));
                } else {
                    key = String::new();
                    child_path.push(Step::Index(index));
                }
                self.skip_ws();
                let value = self.picked(depth + 1, &child_path, pick)?;
                if object {
                    let previous = pairs.iter().position(|(_, k, _)| *k == key);
                    match (previous, value) {
                        (Some(i), Some(value)) => pairs[i].2 = value,
                        (Some(i), None) => { pairs.remove(i); }
                        (None, Some(value)) => {
                            let first = if self.last_key_wins { self.first_key(start, key_start, depth)?.unwrap_or(key_start) } else { key_start };
                            pairs.push((first, key, value));
                        }
                        (None, None) => {}
                    }
                } else if let Some(value) = value {
                    items.push(value);
                }
                index += 1;
                self.skip_ws();
                if self.peek() == Some(end) {
                    break;
                }
                self.expect(b',')?;
            }
        }
        self.expect(end)?;
        Ok(Some(if object {
            pairs.sort_unstable_by_key(|(first, _, _)| *first);
            Value::Obj(pairs.into_iter().map(|(_, key, value)| (key, value)).collect())
        } else { Value::Arr(items) }))
    }

    /// Revisit earlier members instead of retaining skipped keys. This has
    /// quadratic object-key cost, as does the ordinary parser's duplicate check.
    fn first_key(&self, start: usize, key_start: usize, depth: u32) -> Result<Option<usize>, &'static str> {
        let mut prior = P { b: self.b, i: start, max_depth: self.max_depth, last_key_wins: true };
        prior.skip_ws();
        while prior.i < key_start {
            let mut left = StringChars { p: P { b: self.b, i: prior.i + 1, max_depth: 0, last_key_wins: true }, done: false };
            let mut right = StringChars { p: P { b: self.b, i: key_start + 1, max_depth: 0, last_key_wins: true }, done: false };
            if left.by_ref().eq(right.by_ref()) {
                return Ok(Some(prior.i));
            }
            prior.scan_string(None)?;
            prior.skip_ws();
            prior.expect(b':')?;
            prior.skip_ws();
            prior.skip_value(depth + 1)?;
            prior.skip_ws();
            prior.expect(b',')?;
            prior.skip_ws();
        }
        Ok(None)
    }

    fn skip_value(&mut self, depth: u32) -> Result<(), &'static str> {
        if depth > self.max_depth {
            return Err("nesting too deep");
        }
        match self.peek().ok_or("unexpected end")? {
            b'"' => self.scan_string(None),
            b'{' | b'[' => {
                let object = self.next()? == b'{';
                let start = self.i;
                let end = if object { b'}' } else { b']' };
                self.skip_ws();
                if self.peek() != Some(end) {
                    loop {
                        self.skip_ws();
                        if object {
                            let key_start = self.i;
                            self.scan_string(None)?;
                            if !self.last_key_wins && self.first_key(start, key_start, depth)?.is_some() {
                                return Err("duplicate key");
                            }
                            self.skip_ws();
                            self.expect(b':')?;
                            self.skip_ws();
                        }
                        self.skip_value(depth + 1)?;
                        self.skip_ws();
                        if self.peek() == Some(end) { break; }
                        self.expect(b',')?;
                    }
                }
                self.expect(end)
            }
            _ => self.value(depth).map(|_| ()),
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
            let dup = pairs.iter().position(|(k, _)| *k == key);
            if dup.is_some() && !self.last_key_wins {
                return Err("duplicate key");
            }
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let v = self.value(depth + 1)?;
            match dup {
                Some(i) => pairs[i].1 = v,
                None => pairs.push((key, v)),
            }
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
        let mut out = String::new();
        self.scan_string(Some(&mut out))?;
        Ok(out)
    }

    fn scan_string(&mut self, mut out: Option<&mut String>) -> Result<(), &'static str> {
        self.expect(b'"')?;
        loop {
            let c = self.next()?;
            match c {
                b'"' => return Ok(()),
                b'\\' => match self.next()? {
                    b'"' => if let Some(out) = out.as_mut() { out.push('"'); },
                    b'\\' => if let Some(out) = out.as_mut() { out.push('\\'); },
                    b'/' => if let Some(out) = out.as_mut() { out.push('/'); },
                    b'b' => if let Some(out) = out.as_mut() { out.push('\u{08}'); },
                    b'f' => if let Some(out) = out.as_mut() { out.push('\u{0c}'); },
                    b'n' => if let Some(out) = out.as_mut() { out.push('\n'); },
                    b'r' => if let Some(out) = out.as_mut() { out.push('\r'); },
                    b't' => if let Some(out) = out.as_mut() { out.push('\t'); },
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
                        if let Some(out) = out.as_mut() { out.push(char::from_u32(cp).ok_or("bad escape")?); };
                    }
                    _ => return Err("bad escape"),
                },
                c if c < 0x20 => return Err("control in string"),
                c if c < 0x80 => if let Some(out) = out.as_mut() { out.push(c as char); },
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
                    if let Some(out) = out.as_mut() {
                        out.push_str(std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "invalid utf-8")?);
                    }
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
    fn picked_children_exclude_skipped_strings_and_array_members() {
        use Step::{Index, Key};
        let bytes = br#"{"secret":"XYZ\"}\\{\n\u0058","a":["XYZ",{"secret":"XYZ","b":{"secret":"XYZ","a":"ok"}}]}"#;
        let calls = std::cell::Cell::new(0);
        let value = parse_picked(bytes, 8, false, &|path| {
            calls.set(calls.get() + 1);
            match path {
                [] | [Key("a")] | [Key("a"), Index(1)] | [Key("a"), Index(1), Key("b")] => Pick::Open,
                [Key("a"), Index(1), Key("b"), Key("a")] => Pick::Keep,
                _ => Pick::Skip,
            }
        }).unwrap();
        assert_eq!(value.to_json(), r#"{"a":[{"b":{"a":"ok"}}]}"#);
        assert!(!value.to_json().bytes().any(|b| b"XYZ".contains(&b)));
        assert_eq!(calls.get(), 9, "one pick per visited value, including the root");
        assert_eq!(parse_picked(bytes, 8, false, &|_| Pick::Keep), parse_depth(bytes, 8));
        // Projected leaves are built as the ordinary parser builds them.
        let mixed = br#"{"a":"\ud83d\ude00\n","b":1.5,"c":[-2,1e3,null,true],"secret":{"a":"XYZ"}}"#;
        let whole = parse_depth(mixed, 8).unwrap();
        let projected = parse_picked(mixed, 8, false, &|path| match path {
            [] => Pick::Open,
            [Key("a" | "b" | "c")] => Pick::Keep,
            _ => Pick::Skip,
        }).unwrap();
        for key in ["a", "b", "c"] {
            assert_eq!(projected.get(key), whole.get(key), "{key}");
        }
        assert!(projected.get("secret").is_none());
        assert_eq!(parse_picked(b"true", 8, false, &|_| Pick::Open).unwrap(), Value::Null);
        assert_eq!(parse_picked(br#"{"secret":"XYZ"}"#, 8, false, &|_| Pick::KeepString).unwrap(), Value::Null);
    }

    #[test]
    fn skipped_values_obey_validation_and_duplicate_rules() {
        let invalid: &[&[u8]] = &[
            br#""\q""#, br#""\ud800""#, br#""\udc00""#, br#""\ud800\u0041""#,
            br#""\uXXXX""#, b"\"a\x01b\"", b"\"unterminated", &[b'"', 0xff, b'"'],
            b"01", b"1.", b"1e", b"9e999", b"1e9999", b"9223372036854775808",
            b"[1,]", b"{\"a\":}", b"[[[[0]]]]", b"{} trailing",
            br#"{"a":1,"\u0061":2}"#, br#"{"a":{"b":0,"b":1}}"#,
        ];
        for bytes in invalid {
            let expected = parse_depth(bytes, 3);
            assert!(expected.is_err());
            for mode in [Pick::Skip, Pick::Open] {
                assert_eq!(parse_picked(bytes, 3, false, &|_| mode), expected);
            }
        }
        let bytes = r#"{"a":{"secret":"XYZ"},"\u0061":"ok","b":["\ud83d\ude00","é"]}"#.as_bytes();
        let value = parse_picked(bytes, 8, true, &|path| match path {
            [] => Pick::Open,
            [Step::Key("a")] => Pick::KeepString,
            _ => Pick::Skip,
        }).unwrap();
        assert_eq!(value.to_json(), r#"{"a":"ok"}"#);
        assert_eq!(parse_picked(br#"{"a":"ok","a":0}"#, 8, true, &|path| {
            if path.is_empty() { Pick::Open } else { Pick::KeepString }
        }).unwrap().to_json(), "{}");
        let reordered = parse_picked(br#"{"a":0,"b":"b","a":"a","a":false,"a":"last"}"#, 8, true, &|path| {
            if path.is_empty() { Pick::Open } else { Pick::KeepString }
        }).unwrap();
        assert_eq!(reordered.to_json(), r#"{"a":"last","b":"b"}"#);
        assert!(parse_picked(r#"{"\ud83d\ude00":0,"😀":1}"#.as_bytes(), 8, false, &|_| Pick::Skip).is_err());
        assert_eq!(parse_picked(bytes, 8, true, &|_| Pick::Skip).unwrap(), Value::Null);
    }

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
    fn finite_float_writer_roundtrips_with_bounded_numbers() {
        let check = |value: f64| {
            let text = Value::F64(value).to_json();
            assert!(text.len() <= 25, "oversized float: {text}");
            let Value::F64(decoded) = parse(text.as_bytes()).unwrap() else {
                panic!("float lost its type: {text}");
            };
            assert_eq!(decoded.to_bits(), value.to_bits(), "{text}");
        };
        for value in [0.0, -0.0, 1.0, -1.0, 1e-300, 1e300, f64::MAX,
            f64::MIN, f64::MIN_POSITIVE, f64::from_bits(1)] {
            check(value);
        }
        let mut bits = 0x6a09_e667_f3bc_c909u64;
        for _ in 0..4096 {
            bits = bits.wrapping_mul(6364136223846793005).wrapping_add(1);
            let value = f64::from_bits(bits);
            if value.is_finite() {
                check(value);
            }
        }
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
