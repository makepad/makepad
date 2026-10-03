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
    let v = value(src, &mut i)?;
    Ok(v)
}

fn ws(s: &[u8], i: &mut usize) {
    while *i < s.len() && matches!(s[*i], b' ' | b'\n' | b'\r' | b'\t') {
        *i += 1;
    }
}

fn value(s: &[u8], i: &mut usize) -> Result<Json, String> {
    ws(s, i);
    if *i >= s.len() {
        return Err("unexpected end".to_string());
    }
    match s[*i] {
        b'{' => {
            *i += 1;
            let mut v = Vec::new();
            ws(s, i);
            if s[*i] == b'}' {
                *i += 1;
                return Ok(Json::Obj(v));
            }
            loop {
                ws(s, i);
                let k = string(s, i)?;
                ws(s, i);
                if s[*i] != b':' {
                    return Err(format!("expected : at {}", i));
                }
                *i += 1;
                let val = value(s, i)?;
                v.push((k, val));
                ws(s, i);
                if s[*i] == b',' {
                    *i += 1;
                } else if s[*i] == b'}' {
                    *i += 1;
                    return Ok(Json::Obj(v));
                } else {
                    return Err(format!("expected , or }} at {}", i));
                }
            }
        }
        b'[' => {
            *i += 1;
            let mut v = Vec::new();
            ws(s, i);
            if s[*i] == b']' {
                *i += 1;
                return Ok(Json::Arr(v));
            }
            loop {
                v.push(value(s, i)?);
                ws(s, i);
                if s[*i] == b',' {
                    *i += 1;
                } else if s[*i] == b']' {
                    *i += 1;
                    return Ok(Json::Arr(v));
                } else {
                    return Err(format!("expected , or ] at {}", i));
                }
            }
        }
        b'"' => Ok(Json::Str(string(s, i)?)),
        b't' => {
            *i += 4;
            Ok(Json::Bool(true))
        }
        b'f' => {
            *i += 5;
            Ok(Json::Bool(false))
        }
        b'n' => {
            *i += 4;
            Ok(Json::Null)
        }
        _ => {
            let st = *i;
            while *i < s.len() && matches!(s[*i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                *i += 1;
            }
            let t = std::str::from_utf8(&s[st..*i]).unwrap_or("0");
            match t.parse::<f64>() {
                Ok(n) => Ok(Json::Num(n)),
                Err(_) => Err(format!("bad number at {}", st)),
            }
        }
    }
}

fn string(s: &[u8], i: &mut usize) -> Result<String, String> {
    if s[*i] != b'"' {
        return Err(format!("expected string at {}", i));
    }
    *i += 1;
    let mut out: Vec<u8> = Vec::new();
    while *i < s.len() {
        let c = s[*i];
        *i += 1;
        match c {
            b'"' => return Ok(String::from_utf8_lossy(&out).into_owned()),
            b'\\' => {
                let e = s[*i];
                *i += 1;
                match e {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'u' => {
                        let h = std::str::from_utf8(&s[*i..*i + 4]).unwrap_or("0");
                        *i += 4;
                        let cp = u32::from_str_radix(h, 16).unwrap_or(0xfffd);
                        let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    _ => out.push(e),
                }
            }
            _ => out.push(c),
        }
    }
    Err("unterminated string".to_string())
}
