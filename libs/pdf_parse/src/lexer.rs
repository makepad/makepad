use crate::object::*;

/// Error type for PDF parsing.
#[derive(Clone, Debug)]
pub struct PdfError {
    pub msg: String,
}

impl PdfError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "PDF error: {}", self.msg)
    }
}

impl std::error::Error for PdfError {}

pub type PdfResult<T> = Result<T, PdfError>;

/// A cursor into a PDF byte buffer.
pub struct Lexer<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn is_eof(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn peek_byte(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    pub fn read_byte(&mut self) -> PdfResult<u8> {
        if self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            Ok(b)
        } else {
            Err(PdfError::new("unexpected end of data"))
        }
    }

    /// Skip whitespace and comments.
    pub fn skip_whitespace(&mut self) {
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            if b == b'%' {
                // comment: skip to end of line
                while self.pos < self.data.len()
                    && self.data[self.pos] != b'\n'
                    && self.data[self.pos] != b'\r'
                {
                    self.pos += 1;
                }
            } else if is_whitespace(b) {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    /// Check if next bytes match the given string (without consuming).
    pub fn starts_with(&self, s: &[u8]) -> bool {
        self.data[self.pos..].starts_with(s)
    }

    /// Read a PDF token and return a PdfObj.
    /// Handles: numbers, booleans, null, names, strings, arrays, dicts, and indirect refs.
    pub fn read_object(&mut self) -> PdfResult<PdfObj> {
        self.skip_whitespace();
        if self.is_eof() {
            return Err(PdfError::new("unexpected end of data"));
        }

        let b = self.data[self.pos];
        match b {
            // Literal string
            b'(' => self.read_literal_string(),
            // Hex string or dict
            b'<' => {
                if self.pos + 1 < self.data.len() && self.data[self.pos + 1] == b'<' {
                    self.read_dict()
                } else {
                    self.read_hex_string()
                }
            }
            // Array
            b'[' => self.read_array(),
            // Name
            b'/' => self.read_name(),
            // Number or indirect ref
            b'+' | b'-' | b'.' | b'0'..=b'9' => self.read_number_or_ref(),
            // Keywords: true, false, null
            b't' if self.starts_with(b"true") => {
                self.pos += 4;
                Ok(PdfObj::Bool(true))
            }
            b'f' if self.starts_with(b"false") => {
                self.pos += 5;
                Ok(PdfObj::Bool(false))
            }
            b'n' if self.starts_with(b"null") => {
                self.pos += 4;
                Ok(PdfObj::Null)
            }
            _ => Err(PdfError::new(format!(
                "unexpected byte 0x{:02x} '{}' at offset {}",
                b, b as char, self.pos
            ))),
        }
    }

    fn read_literal_string(&mut self) -> PdfResult<PdfObj> {
        self.pos += 1; // skip '('
        let mut result = Vec::new();
        let mut depth = 1u32;
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            match b {
                b'(' => {
                    depth += 1;
                    result.push(b'(');
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(PdfObj::Str(result));
                    }
                    result.push(b')');
                }
                b'\\' => {
                    if self.pos >= self.data.len() {
                        return Err(PdfError::new("unexpected end in string escape"));
                    }
                    let esc = self.data[self.pos];
                    self.pos += 1;
                    match esc {
                        b'n' => result.push(b'\n'),
                        b'r' => result.push(b'\r'),
                        b't' => result.push(b'\t'),
                        b'b' => result.push(8),  // backspace
                        b'f' => result.push(12), // form feed
                        b'(' => result.push(b'('),
                        b')' => result.push(b')'),
                        b'\\' => result.push(b'\\'),
                        b'0'..=b'7' => {
                            // Octal escape: up to 3 digits
                            let mut val = (esc - b'0') as u8;
                            for _ in 0..2 {
                                if self.pos < self.data.len()
                                    && self.data[self.pos] >= b'0'
                                    && self.data[self.pos] <= b'7'
                                {
                                    val = val * 8 + (self.data[self.pos] - b'0');
                                    self.pos += 1;
                                } else {
                                    break;
                                }
                            }
                            result.push(val);
                        }
                        b'\r' => {
                            // line continuation
                            if self.pos < self.data.len() && self.data[self.pos] == b'\n' {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {
                            // line continuation
                        }
                        _ => {
                            // unknown escape: just include the character
                            result.push(esc);
                        }
                    }
                }
                _ => result.push(b),
            }
        }
        Err(PdfError::new("unterminated literal string"))
    }

    fn read_hex_string(&mut self) -> PdfResult<PdfObj> {
        self.pos += 1; // skip '<'
        let mut hex_bytes = Vec::new();
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            if b == b'>' {
                self.pos += 1;
                break;
            }
            if is_whitespace(b) {
                self.pos += 1;
                continue;
            }
            hex_bytes.push(b);
            self.pos += 1;
        }
        // Pad with trailing 0 if odd number of hex digits
        if hex_bytes.len() % 2 != 0 {
            hex_bytes.push(b'0');
        }
        let mut result = Vec::with_capacity(hex_bytes.len() / 2);
        for chunk in hex_bytes.chunks(2) {
            let hi = hex_digit(chunk[0]).ok_or_else(|| PdfError::new("invalid hex digit"))?;
            let lo = hex_digit(chunk[1]).ok_or_else(|| PdfError::new("invalid hex digit"))?;
            result.push((hi << 4) | lo);
        }
        Ok(PdfObj::Str(result))
    }

    fn read_name(&mut self) -> PdfResult<PdfObj> {
        self.pos += 1; // skip '/'
        let mut name = String::new();
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            if is_whitespace(b) || is_delimiter(b) {
                break;
            }
            if b == b'#' && self.pos + 2 < self.data.len() {
                // hex escape in name
                let hi = hex_digit(self.data[self.pos + 1]);
                let lo = hex_digit(self.data[self.pos + 2]);
                if let (Some(h), Some(l)) = (hi, lo) {
                    name.push(((h << 4) | l) as char);
                    self.pos += 3;
                    continue;
                }
            }
            name.push(b as char);
            self.pos += 1;
        }
        Ok(PdfObj::Name(name))
    }

    fn read_array(&mut self) -> PdfResult<PdfObj> {
        self.pos += 1; // skip '['
        let mut items = Vec::new();
        loop {
            self.skip_whitespace();
            if self.is_eof() {
                return Err(PdfError::new("unterminated array"));
            }
            if self.data[self.pos] == b']' {
                self.pos += 1;
                return Ok(PdfObj::Array(items));
            }
            items.push(self.read_object()?);
        }
    }

    fn read_dict(&mut self) -> PdfResult<PdfObj> {
        self.pos += 2; // skip '<<'
        let mut dict = PdfDict::new();
        loop {
            self.skip_whitespace();
            if self.is_eof() {
                return Err(PdfError::new("unterminated dict"));
            }
            if self.starts_with(b">>") {
                self.pos += 2;
                return Ok(PdfObj::Dict(dict));
            }
            // Read key (must be a name)
            let key = self.read_object()?;
            let key_name = match key {
                PdfObj::Name(n) => n,
                _ => {
                    return Err(PdfError::new(format!(
                        "dict key must be a name, got {:?}",
                        key
                    )))
                }
            };
            // Read value
            let value = self.read_object()?;
            dict.map.insert(key_name, value);
        }
    }

    /// Read a number. If followed by another number and 'R', it's an indirect ref.
    fn read_number_or_ref(&mut self) -> PdfResult<PdfObj> {
        let save_pos = self.pos;
        let num = self.read_number_value()?;

        // Check if this could be the start of an indirect reference (N G R)
        if let PdfObj::Int(obj_num) = num {
            if obj_num >= 0 {
                let save_pos2 = self.pos;
                self.skip_whitespace();
                if !self.is_eof() && self.data[self.pos].is_ascii_digit() {
                    let gen = self.read_number_value()?;
                    if let PdfObj::Int(gen_num) = gen {
                        self.skip_whitespace();
                        if !self.is_eof() && self.data[self.pos] == b'R' {
                            self.pos += 1;
                            return Ok(PdfObj::Ref(ObjRef {
                                num: obj_num as u32,
                                gen: gen_num as u16,
                            }));
                        }
                    }
                }
                // Not an indirect ref: restore position
                self.pos = save_pos2;
            }
        }

        // If we couldn't parse a number at all, something went wrong
        if let PdfObj::Null = num {
            self.pos = save_pos;
            return Err(PdfError::new("expected number"));
        }

        Ok(num)
    }

    fn read_number_value(&mut self) -> PdfResult<PdfObj> {
        let start = self.pos;
        let mut has_dot = false;
        if self.pos < self.data.len()
            && (self.data[self.pos] == b'+' || self.data[self.pos] == b'-')
        {
            self.pos += 1;
        }
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            if b == b'.' {
                if has_dot {
                    break;
                }
                has_dot = true;
                self.pos += 1;
            } else if b.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        let s = std::str::from_utf8(&self.data[start..self.pos])
            .map_err(|_| PdfError::new("invalid number encoding"))?;
        if has_dot {
            let val: f64 = s
                .parse()
                .map_err(|_| PdfError::new(format!("invalid real: {}", s)))?;
            Ok(PdfObj::Real(val))
        } else {
            let val: i64 = s
                .parse()
                .map_err(|_| PdfError::new(format!("invalid integer: {}", s)))?;
            Ok(PdfObj::Int(val))
        }
    }

    /// Read a keyword (sequence of non-whitespace, non-delimiter bytes).
    pub fn read_keyword(&mut self) -> PdfResult<String> {
        self.skip_whitespace();
        let start = self.pos;
        while self.pos < self.data.len() {
            let b = self.data[self.pos];
            if is_whitespace(b) || is_delimiter(b) {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Err(PdfError::new("expected keyword"));
        }
        Ok(std::str::from_utf8(&self.data[start..self.pos])
            .unwrap_or("???")
            .to_string())
    }

    /// Peek at the next non-whitespace byte without consuming.
    pub fn peek_non_ws(&mut self) -> Option<u8> {
        let save = self.pos;
        self.skip_whitespace();
        let result = self.peek_byte();
        self.pos = save;
        result
    }

    /// Find a byte sequence searching backwards from the end.
    pub fn rfind(data: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.len() > data.len() {
            return None;
        }
        for i in (0..=data.len() - needle.len()).rev() {
            if data[i..].starts_with(needle) {
                return Some(i);
            }
        }
        None
    }
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0 | 12)
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}
