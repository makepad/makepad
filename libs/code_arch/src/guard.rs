//! Allocation-free structural pass before the repository's unbounded TOML
//! decoder. It bounds keys, implicit tables, containers, tokens and strings,
//! including malformed input. TOML itself remains the authoritative decoder.
use crate::{limit, ArchError, ErrorKind, Limits};

pub(crate) fn preflight(text: &str, limits: &Limits) -> Result<(), ArchError> {
    limit("input bytes", text.len(), limits.max_input_bytes)?;
    let mut scan = Scan {
        bytes: text.as_bytes(),
        pos: 0,
        values: 0,
        limits,
    };
    let mut scope = 0;
    loop {
        scan.whitespace(true);
        if scan.pos == scan.bytes.len() {
            return Ok(());
        }
        if scan.take(b'[') {
            let array = scan.take(b'[');
            scope = scan.key(0)?;
            scan.ws();
            scan.expect(b']')?;
            if array {
                scan.expect(b']')?;
            }
        } else {
            let depth = scan.key(scope)?;
            scan.ws();
            scan.expect(b'=')?;
            scan.value(depth)?;
        }
        scan.ws();
        scan.comment();
        if scan.pos < scan.bytes.len() && !matches!(scan.peek(), b'\r' | b'\n') {
            return Err(scan.syntax("expected newline"));
        }
    }
}

struct Scan<'a> {
    bytes: &'a [u8],
    pos: usize,
    values: usize,
    limits: &'a Limits,
}

impl Scan<'_> {
    fn peek(&self) -> u8 {
        self.bytes.get(self.pos).copied().unwrap_or(0)
    }
    fn take(&mut self, byte: u8) -> bool {
        if self.pos < self.bytes.len() && self.peek() == byte {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn syntax(&self, message: &str) -> ArchError {
        let mut error = ArchError::new(ErrorKind::Syntax, "TOML", message);
        error.offset = Some(self.pos);
        error
    }
    fn expect(&mut self, byte: u8) -> Result<(), ArchError> {
        if self.take(byte) {
            Ok(())
        } else {
            Err(self.syntax("unexpected token"))
        }
    }
    fn ws(&mut self) {
        while self.pos < self.bytes.len() && matches!(self.peek(), b' ' | b'\t') {
            self.pos += 1;
        }
    }
    fn comment(&mut self) {
        if self.take(b'#') {
            while self.pos < self.bytes.len() && self.peek() != b'\n' {
                self.pos += 1;
            }
        }
    }
    fn whitespace(&mut self, lines: bool) {
        loop {
            self.ws();
            if !lines {
                break;
            }
            self.comment();
            if self.take(b'\n') || self.take(b'\r') {
                continue;
            }
            break;
        }
    }
    fn count(&mut self, depth: usize) -> Result<(), ArchError> {
        limit("depth", depth, self.limits.max_depth)?;
        self.values += 1;
        limit("values", self.values, self.limits.max_values)
    }
    fn key(&mut self, base: usize) -> Result<usize, ArchError> {
        let mut depth = base;
        loop {
            self.ws();
            depth += 1;
            self.count(depth)?;
            if matches!(self.peek(), b'"' | b'\'') {
                self.string(false)?;
            } else {
                let start = self.pos;
                while self.pos < self.bytes.len()
                    && (self.peek().is_ascii_alphanumeric() || matches!(self.peek(), b'_' | b'-'))
                {
                    self.pos += 1;
                    limit(
                        "string bytes",
                        self.pos - start,
                        self.limits.max_string_bytes,
                    )?;
                }
                if self.pos == start {
                    return Err(self.syntax("expected key"));
                }
            }
            self.ws();
            if !self.take(b'.') {
                return Ok(depth);
            }
        }
    }
    fn value(&mut self, depth: usize) -> Result<(), ArchError> {
        self.ws();
        self.count(depth)?;
        match self.peek() {
            b'"' | b'\'' => self.string(true),
            b'[' => {
                self.pos += 1;
                self.whitespace(true);
                if self.take(b']') {
                    return Ok(());
                }
                loop {
                    self.value(depth + 1)?;
                    self.whitespace(true);
                    if self.take(b']') {
                        return Ok(());
                    }
                    self.expect(b',')?;
                    self.whitespace(true);
                    if self.take(b']') {
                        return Ok(());
                    }
                }
            }
            b'{' => {
                self.pos += 1;
                self.ws();
                if self.take(b'}') {
                    return Ok(());
                }
                loop {
                    let child = self.key(depth)?;
                    self.expect(b'=')?;
                    self.value(child)?;
                    self.ws();
                    if self.take(b'}') {
                        return Ok(());
                    }
                    self.expect(b',')?;
                    self.ws();
                }
            }
            _ => {
                let start = self.pos;
                while self.pos < self.bytes.len()
                    && !matches!(self.peek(), b',' | b']' | b'}' | b'#' | b'\r' | b'\n')
                {
                    self.pos += 1;
                    limit(
                        "token bytes",
                        self.pos - start,
                        self.limits.max_string_bytes,
                    )?;
                }
                if self.pos == start {
                    Err(self.syntax("expected value"))
                } else {
                    Ok(())
                }
            }
        }
    }
    fn string(&mut self, allow_multi: bool) -> Result<(), ArchError> {
        let quote = self.peek();
        self.pos += 1;
        let multi = allow_multi && self.bytes.get(self.pos..self.pos + 2) == Some(&[quote, quote]);
        if multi {
            self.pos += 2;
        }
        if multi && !self.take(b'\n') && self.bytes.get(self.pos..self.pos + 2) == Some(b"\r\n") {
            self.pos += 2;
        }
        let mut decoded = 0;
        loop {
            limit("string bytes", decoded, self.limits.max_string_bytes)?;
            if self.pos == self.bytes.len() {
                return Err(self.syntax("unterminated string"));
            }
            if self.peek() == quote {
                if !multi {
                    self.pos += 1;
                    return Ok(());
                }
                let mut quotes = 0;
                while self.take(quote) {
                    quotes += 1;
                    if quotes > 5 {
                        return Err(self.syntax("too many closing quotes"));
                    }
                }
                if quotes >= 3 {
                    return limit(
                        "string bytes",
                        decoded + quotes - 3,
                        self.limits.max_string_bytes,
                    );
                }
                decoded += quotes;
            } else if quote == b'"' && self.take(b'\\') {
                match self.peek() {
                    b'u' | b'U' => {
                        let digits = if self.peek() == b'u' { 4 } else { 8 };
                        self.pos += 1;
                        let mut code = 0u32;
                        for _ in 0..digits {
                            let digit = (self.peek() as char)
                                .to_digit(16)
                                .ok_or_else(|| self.syntax("invalid Unicode escape"))?;
                            code = code * 16 + digit;
                            self.pos += 1;
                        }
                        decoded += char::from_u32(code)
                            .ok_or_else(|| self.syntax("invalid Unicode scalar"))?
                            .len_utf8();
                    }
                    b' ' | b'\t' | b'\n' | b'\r' if multi => {
                        self.ws();
                        if !self.take(b'\n') {
                            self.expect(b'\r')?;
                            self.expect(b'\n')?;
                        }
                        while self.pos < self.bytes.len()
                            && matches!(self.peek(), b' ' | b'\t' | b'\r' | b'\n')
                        {
                            self.pos += 1;
                        }
                    }
                    _ => {
                        if self.pos < self.bytes.len() {
                            self.pos += 1;
                            decoded += 1;
                        }
                    }
                }
            } else {
                if !multi && matches!(self.peek(), b'\r' | b'\n') {
                    return Err(self.syntax("newline in single-line string"));
                }
                self.pos += 1;
                decoded += 1;
            }
        }
    }
}
