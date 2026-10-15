//! Minimal JSON reader (cargo's unit graph is the only input today).

pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        if let Json::Obj(v) = self {
            for (k, val) in v {
                if k == key {
                    return Some(val);
                }
            }
        }
        None
    }
    pub fn str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            _ => "",
        }
    }
    pub fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(v) => v,
            _ => &[],
        }
    }
    pub fn num(&self) -> f64 {
        match self {
            Json::Num(n) => *n,
            _ => 0.0,
        }
    }
}

pub fn parse(src: &[u8]) -> Result<Json, String> {
    let mut i = 0usize;
    let v = value(src, &mut i, 0)?;
    ws(src, &mut i);
    if i != src.len() { return Err("trailing JSON input".into()); }
    Ok(v)
}

fn ws(s: &[u8], i: &mut usize) {
    while *i < s.len() && matches!(s[*i], b' ' | b'\n' | b'\r' | b'\t') {
        *i += 1;
    }
}

fn value(s: &[u8], i: &mut usize, depth: usize) -> Result<Json, String> {
    if depth > 128 { return Err("JSON nesting limit exceeded".into()); }
    ws(s, i);
    match s.get(*i).copied().ok_or("unexpected end")? {
        b'{' => {
            *i += 1;
            let mut v = Vec::new(); ws(s,i);
            if s.get(*i) == Some(&b'}') { *i += 1; return Ok(Json::Obj(v)); }
            loop {
                ws(s,i); let k = string(s,i)?; ws(s,i);
                if s.get(*i) != Some(&b':') { return Err(format!("expected : at {}",i)); }
                *i += 1; let val = value(s,i,depth+1)?;
                if v.iter().any(|(key,_)|key == &k) { return Err("duplicate object key".into()); }
                v.push((k,val)); ws(s,i);
                match s.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b'}') => { *i += 1; return Ok(Json::Obj(v)); }
                    _ => return Err(format!("expected , or }} at {}",i)),
                }
            }
        }
        b'[' => {
            *i += 1; let mut v = Vec::new(); ws(s,i);
            if s.get(*i) == Some(&b']') { *i += 1; return Ok(Json::Arr(v)); }
            loop {
                v.push(value(s,i,depth+1)?); ws(s,i);
                match s.get(*i) {
                    Some(b',') => *i += 1,
                    Some(b']') => { *i += 1; return Ok(Json::Arr(v)); }
                    _ => return Err(format!("expected , or ] at {}",i)),
                }
            }
        }
        b'"' => Ok(Json::Str(string(s,i)?)),
        b't' if s.get(*i..*i+4) == Some(b"true") => { *i += 4; Ok(Json::Bool(true)) }
        b'f' if s.get(*i..*i+5) == Some(b"false") => { *i += 5; Ok(Json::Bool(false)) }
        b'n' if s.get(*i..*i+4) == Some(b"null") => { *i += 4; Ok(Json::Null) }
        b'-' | b'0'..=b'9' => {
            let start = *i;
            if s.get(*i) == Some(&b'-') { *i += 1; }
            match s.get(*i) {
                Some(b'0') => *i += 1,
                Some(b'1'..=b'9') => { while s.get(*i).is_some_and(u8::is_ascii_digit) { *i += 1; } }
                _ => return Err("invalid JSON number".into()),
            }
            if s.get(*i) == Some(&b'.') {
                *i += 1; let at = *i;
                while s.get(*i).is_some_and(u8::is_ascii_digit) { *i += 1; }
                if at == *i { return Err("invalid fraction".into()); }
            }
            if matches!(s.get(*i),Some(b'e'|b'E')) {
                *i += 1;
                if matches!(s.get(*i),Some(b'+'|b'-')) { *i += 1; }
                let at = *i; while s.get(*i).is_some_and(u8::is_ascii_digit) { *i += 1; }
                if at == *i { return Err("invalid exponent".into()); }
            }
            let n = std::str::from_utf8(&s[start..*i]).map_err(|_|"invalid number")?.parse::<f64>().map_err(|_|"invalid number")?;
            if !n.is_finite() { return Err("nonfinite JSON number".into()); }
            Ok(Json::Num(n))
        }
        _ => Err(format!("unexpected token at {}",i)),
    }
}

fn string(s: &[u8], i: &mut usize) -> Result<String, String> {
    if s.get(*i) != Some(&b'"') {
        return Err(format!("expected string at {}", i));
    }
    *i += 1;
    let mut out: Vec<u8> = Vec::new();
    while *i < s.len() {
        let c = s[*i];
        *i += 1;
        match c {
            b'"' => return String::from_utf8(out).map_err(|_|"invalid UTF-8 in JSON string".into()),
            b'\\' => {
                let e = *s.get(*i).ok_or("unfinished string escape")?;
                *i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'u' => {
                        fn hex(s: &[u8], i: &mut usize) -> Result<u32, String> {
                            let digits = s.get(*i..*i + 4).ok_or("short Unicode escape")?;
                            let n = u32::from_str_radix(std::str::from_utf8(digits).map_err(|_| "invalid Unicode escape")?, 16).map_err(|_| "invalid Unicode escape")?;
                            *i += 4; Ok(n)
                        }
                        let mut cp = hex(s, i)?;
                        if (0xd800..=0xdbff).contains(&cp) {
                            if s.get(*i..*i + 2) != Some(b"\\u") { return Err("missing low surrogate".into()); }
                            *i += 2;
                            let lo = hex(s, i)?;
                            if !(0xdc00..=0xdfff).contains(&lo) { return Err("invalid low surrogate".into()); }
                            cp = 0x10000 + ((cp - 0xd800) << 10) + lo - 0xdc00;
                        }
                        let ch = char::from_u32(cp).ok_or("invalid Unicode scalar")?;
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    b'"' | b'\\' | b'/' => out.push(e),
                    _ => return Err("invalid string escape".into()),
                }
            }
            0..=31 => return Err("unescaped control character".into()),
            _ => out.push(c),
        }
    }
    Err("unterminated string".to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn editor_unicode_and_truncated_commands() {
        assert_eq!(super::parse(br#""\ud83d\ude80""#).unwrap().str(), "🚀");
        for input in [b"{".as_slice(), b"[", b"{\"a\"", b"{\"a\":", b"{\"a\":1", b"[1", b"tru", b"falsehood", b"01", b"1.", b"1e999", b"{\"a\":1,\"a\":2}",br#""\u12"#.as_slice(), br#""\ud83d""#, br#""\udfff""#, br#""\"#, b"{} garbage"] {
            assert!(super::parse(input).is_err());
        }
    }
}
