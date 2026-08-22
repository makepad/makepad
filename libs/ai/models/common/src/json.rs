//! Minimal JSON value + parser used by H3 tokenizer.json and the
//! dtype-agnostic safetensors header reader. Moved out of
//! libs/diffusion so flux and h3 do not depend on each other for it.

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn parse(text: &str) -> std::result::Result<Json, String> {
        let mut parser = JsonParser {
            bytes: text.as_bytes(),
            pos: 0,
        };
        parser.skip_ws();
        let value = parser.parse_value(0)?;
        parser.skip_ws();
        if parser.pos != parser.bytes.len() {
            return Err(format!("trailing data at byte {}", parser.pos));
        }
        Ok(value)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries
                .iter()
                .find(|(entry_key, _)| entry_key == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_f64()
            .filter(|n| *n >= 0.0 && n.fract() == 0.0 && *n <= u32::MAX as f64)
            .map(|n| n as u32)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(entries) => Some(entries),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl JsonParser<'_> {
    fn skip_ws(&mut self) {
        while matches!(
            self.bytes.get(self.pos),
            Some(b' ' | b'\t' | b'\n' | b'\r')
        ) {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self, depth: usize) -> std::result::Result<Json, String> {
        if depth > 256 {
            return Err("json nesting too deep".to_owned());
        }
        match self.bytes.get(self.pos) {
            Some(b'{') => {
                self.pos += 1;
                let mut entries = Vec::new();
                self.skip_ws();
                if self.bytes.get(self.pos) == Some(&b'}') {
                    self.pos += 1;
                    return Ok(Json::Obj(entries));
                }
                loop {
                    self.skip_ws();
                    let key = self.parse_string()?;
                    self.skip_ws();
                    if self.bytes.get(self.pos) != Some(&b':') {
                        return Err(format!("expected ':' at byte {}", self.pos));
                    }
                    self.pos += 1;
                    self.skip_ws();
                    let value = self.parse_value(depth + 1)?;
                    entries.push((key, value));
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b'}') => {
                            self.pos += 1;
                            return Ok(Json::Obj(entries));
                        }
                        _ => return Err(format!("expected ',' or '}}' at byte {}", self.pos)),
                    }
                }
            }
            Some(b'[') => {
                self.pos += 1;
                let mut values = Vec::new();
                self.skip_ws();
                if self.bytes.get(self.pos) == Some(&b']') {
                    self.pos += 1;
                    return Ok(Json::Arr(values));
                }
                loop {
                    self.skip_ws();
                    values.push(self.parse_value(depth + 1)?);
                    self.skip_ws();
                    match self.bytes.get(self.pos) {
                        Some(b',') => self.pos += 1,
                        Some(b']') => {
                            self.pos += 1;
                            return Ok(Json::Arr(values));
                        }
                        _ => return Err(format!("expected ',' or ']' at byte {}", self.pos)),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.parse_string()?)),
            Some(b't') => self.parse_literal("true", Json::Bool(true)),
            Some(b'f') => self.parse_literal("false", Json::Bool(false)),
            Some(b'n') => self.parse_literal("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err(format!("unexpected input at byte {}", self.pos)),
        }
    }

    fn parse_literal(
        &mut self,
        literal: &str,
        value: Json,
    ) -> std::result::Result<Json, String> {
        if self.bytes[self.pos..].starts_with(literal.as_bytes()) {
            self.pos += literal.len();
            Ok(value)
        } else {
            Err(format!("expected {} at byte {}", literal, self.pos))
        }
    }

    fn parse_number(&mut self) -> std::result::Result<Json, String> {
        let start = self.pos;
        while matches!(
            self.bytes.get(self.pos),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| format!("invalid number at byte {}", start))?;
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("invalid number {:?} at byte {}", text, start))
    }

    fn parse_string(&mut self) -> std::result::Result<String, String> {
        if self.bytes.get(self.pos) != Some(&b'"') {
            return Err(format!("expected string at byte {}", self.pos));
        }
        self.pos += 1;
        let mut out = String::new();
        loop {
            let run_start = self.pos;
            while let Some(&byte) = self.bytes.get(self.pos) {
                if byte == b'"' || byte == b'\\' || byte < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            // Runs are delimited by ASCII bytes, so they sit on UTF-8
            // boundaries of the source &str.
            out.push_str(
                std::str::from_utf8(&self.bytes[run_start..self.pos])
                    .map_err(|_| format!("invalid utf-8 in string at byte {}", run_start))?,
            );
            match self.bytes.get(self.pos) {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let escape = *self
                        .bytes
                        .get(self.pos)
                        .ok_or_else(|| "unterminated escape".to_owned())?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.parse_hex4()?;
                            let ch = if (0xD800..=0xDBFF).contains(&unit) {
                                // Surrogate pair: require the low half.
                                if self.bytes.get(self.pos) != Some(&b'\\')
                                    || self.bytes.get(self.pos + 1) != Some(&b'u')
                                {
                                    return Err(format!(
                                        "lone high surrogate at byte {}",
                                        self.pos
                                    ));
                                }
                                self.pos += 2;
                                let low = self.parse_hex4()?;
                                if !(0xDC00..=0xDFFF).contains(&low) {
                                    return Err(format!(
                                        "invalid low surrogate at byte {}",
                                        self.pos
                                    ));
                                }
                                let code =
                                    0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                                char::from_u32(code)
                                    .ok_or_else(|| "invalid surrogate pair".to_owned())?
                            } else if (0xDC00..=0xDFFF).contains(&unit) {
                                return Err(format!("lone low surrogate at byte {}", self.pos));
                            } else {
                                char::from_u32(unit)
                                    .ok_or_else(|| "invalid \\u escape".to_owned())?
                            };
                            out.push(ch);
                        }
                        other => {
                            return Err(format!(
                                "unsupported escape '\\{}' at byte {}",
                                other as char, self.pos
                            ))
                        }
                    }
                }
                Some(_) => return Err(format!("control byte in string at byte {}", self.pos)),
                None => return Err("unterminated string".to_owned()),
            }
        }
    }

    fn parse_hex4(&mut self) -> std::result::Result<u32, String> {
        let end = self.pos + 4;
        if end > self.bytes.len() {
            return Err("truncated \\u escape".to_owned());
        }
        let text = std::str::from_utf8(&self.bytes[self.pos..end])
            .map_err(|_| "invalid \\u escape".to_owned())?;
        let value =
            u32::from_str_radix(text, 16).map_err(|_| format!("invalid \\u escape {:?}", text))?;
        self.pos = end;
        Ok(value)
    }
}
