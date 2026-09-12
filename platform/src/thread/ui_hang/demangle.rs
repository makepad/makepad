//! Bounded Rust symbol decoding for watchdog output. Runs after the sampled
//! thread resumes. Unsupported encodings retain the complete original symbol.
//! No loader calls, external demangler process, or dependency is required.

pub fn demangle(symbol: &str) -> String {
    let name = symbol.trim_start_matches('_');
    if let Some(encoded) = name.strip_prefix('R') {
        let mut parser = Parser {
            text: encoded,
            at: 0,
            depth: 0,
            fuel: 16384,
        };
        if let Some(path) = parser.path() {
            // A v0 symbol may end in an instantiating crate and a vendor suffix.
            if parser.at < encoded.len() && !encoded[parser.at..].starts_with('.') {
                if parser.path().is_none() {
                    return symbol.into();
                }
            }
            if parser.at == encoded.len() || encoded[parser.at..].starts_with('.') {
                return path;
            }
        }
    } else if let Some(mut rest) = name.strip_prefix("ZN") {
        let mut parts = Vec::new();
        while !rest.starts_with('E') {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let Some(len) = rest.get(..digits).and_then(|s| s.parse::<usize>().ok()) else {
                return symbol.into();
            };
            let Some(part) = rest.get(digits..).and_then(|s| s.get(..len)) else {
                return symbol.into();
            };
            rest = &rest[digits + len..];
            if !(part.len() == 17
                && part.starts_with('h')
                && part[1..].bytes().all(|b| b.is_ascii_hexdigit()))
            {
                parts.push(part);
            }
        }
        // Do not mislabel Itanium C++ template/type suffixes as Rust.
        if rest == "E" || rest.starts_with("E.") {
            let mut decoded = parts.join("::").replace("..", "::");
            for (from, to) in [
                ("$SP$", "@"),
                ("$BP$", "*"),
                ("$RF$", "&"),
                ("$LT$", "<"),
                ("$GT$", ">"),
                ("$LP$", "("),
                ("$RP$", ")"),
                ("$C$", ","),
                ("$u20$", " "),
                ("$u27$", "'"),
            ] {
                decoded = decoded.replace(from, to);
            }
            return decoded;
        }
    }
    symbol.into()
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
    depth: usize,
    fuel: usize,
}
impl Parser<'_> {
    fn byte(&mut self) -> Option<u8> {
        self.fuel = self.fuel.checked_sub(1)?;
        let byte = *self.text.as_bytes().get(self.at)?;
        self.at += 1;
        Some(byte)
    }
    fn eat(&mut self, byte: u8) -> bool {
        if self.text.as_bytes().get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn number(&mut self) -> Option<usize> {
        if self.eat(b'_') {
            return Some(0);
        }
        let mut n = 0usize;
        loop {
            let byte = self.byte()?;
            if byte == b'_' {
                return n.checked_add(1);
            }
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'z' => byte - b'a' + 10,
                b'A'..=b'Z' => byte - b'A' + 36,
                _ => return None,
            };
            n = n.checked_mul(62)?.checked_add(digit as usize)?;
        }
    }
    fn disambiguator(&mut self) -> Option<usize> {
        if self.eat(b's') {
            self.number()?.checked_add(1)
        } else {
            Some(0)
        }
    }
    fn ident(&mut self) -> Option<String> {
        // Own repository paths use ASCII identifiers. Preserve unfamiliar
        // punycode symbols verbatim instead of inventing a decoded name.
        if self.eat(b'u') {
            return None;
        }
        let start = self.at;
        while self
            .text
            .as_bytes()
            .get(self.at)
            .is_some_and(u8::is_ascii_digit)
        {
            self.at += 1;
        }
        let len: usize = self.text.get(start..self.at)?.parse().ok()?;
        self.eat(b'_');
        let end = self.at.checked_add(len)?;
        let name = self.text.get(self.at..end)?.to_owned();
        self.at = end;
        Some(name)
    }
    fn backref(&mut self, parse: fn(&mut Self) -> Option<String>) -> Option<String> {
        let marker = self.at - 1;
        let offset = self.number()?;
        if offset >= marker {
            return None;
        }
        let saved = self.at;
        self.at = offset;
        let result = parse(self);
        self.at = saved;
        result
    }
    fn path(&mut self) -> Option<String> {
        if self.depth >= 128 {
            return None;
        }
        self.depth += 1;
        let result = self.path_inner();
        self.depth -= 1;
        result.filter(|s| s.len() <= 16384)
    }
    fn path_inner(&mut self) -> Option<String> {
        match self.byte()? {
            b'C' => {
                self.disambiguator()?;
                self.ident()
            }
            b'N' => {
                let namespace = self.byte()?;
                let parent = self.path()?;
                let disambiguator = self.disambiguator()?;
                let ident = self.ident()?;
                if namespace.is_ascii_uppercase() {
                    let label = match namespace {
                        b'C' => "closure".into(),
                        b'S' => "shim".into(),
                        _ => (namespace as char).to_string(),
                    };
                    Some(format!("{parent}::{{{label}:{ident}#{disambiguator}}}"))
                } else if ident.is_empty() {
                    Some(parent)
                } else {
                    Some(format!("{parent}::{ident}"))
                }
            }
            b'M' => {
                self.disambiguator()?;
                self.path()?;
                self.ty()
            }
            b'X' => {
                self.disambiguator()?;
                self.path()?;
                let ty = self.ty()?;
                let tr = self.path()?;
                Some(format!("<{ty} as {tr}>"))
            }
            b'Y' => {
                let ty = self.ty()?;
                let tr = self.path()?;
                Some(format!("<{ty} as {tr}>"))
            }
            b'I' => {
                let path = self.path()?;
                let mut args = Vec::new();
                while !self.eat(b'E') {
                    args.push(if self.eat(b'L') {
                        self.number()?;
                        "'_".into()
                    } else if self.eat(b'K') {
                        self.constant()?
                    } else {
                        self.ty()?
                    });
                    if args.len() > 256 {
                        return None;
                    }
                }
                Some(format!("{path}::<{}>", args.join(", ")))
            }
            b'B' => self.backref(Self::path),
            _ => None,
        }
    }
    fn ty(&mut self) -> Option<String> {
        if self.depth >= 128 {
            return None;
        }
        self.depth += 1;
        let result = self.ty_inner();
        self.depth -= 1;
        result.filter(|s| s.len() <= 16384)
    }
    fn ty_inner(&mut self) -> Option<String> {
        let start = self.at;
        let tag = self.byte()?;
        let basic = match tag {
            b'a' => "i8",
            b'b' => "bool",
            b'c' => "char",
            b'd' => "f64",
            b'e' => "str",
            b'f' => "f32",
            b'h' => "u8",
            b'i' => "isize",
            b'j' => "usize",
            b'l' => "i32",
            b'm' => "u32",
            b'n' => "i128",
            b'o' => "u128",
            b's' => "i16",
            b't' => "u16",
            b'u' => "()",
            b'v' => "...",
            b'x' => "i64",
            b'y' => "u64",
            b'z' => "!",
            b'p' => "_",
            _ => "",
        };
        if !basic.is_empty() {
            return Some(basic.into());
        }
        match tag {
            b'R' | b'Q' => {
                if self.eat(b'L') {
                    self.number()?;
                }
                Some(format!(
                    "&{}{}",
                    if tag == b'Q' { "mut " } else { "" },
                    self.ty()?
                ))
            }
            b'P' | b'O' => Some(format!(
                "*{} {}",
                if tag == b'P' { "const" } else { "mut" },
                self.ty()?
            )),
            b'S' => Some(format!("[{}]", self.ty()?)),
            b'A' => {
                let ty = self.ty()?;
                Some(format!("[{ty}; {}]", self.constant()?))
            }
            b'T' => {
                let mut types = Vec::new();
                while !self.eat(b'E') {
                    types.push(self.ty()?);
                    if types.len() > 256 {
                        return None;
                    }
                }
                Some(format!(
                    "({}{})",
                    types.join(", "),
                    if types.len() == 1 { "," } else { "" }
                ))
            }
            b'B' => self.backref(Self::ty),
            _ => {
                self.at = start;
                self.path()
            }
        }
    }
    fn constant(&mut self) -> Option<String> {
        if self.eat(b'p') {
            return Some("_".into());
        }
        let ty = self.ty()?;
        let negative = self.eat(b'n');
        let start = self.at;
        while self
            .text
            .as_bytes()
            .get(self.at)
            .is_some_and(u8::is_ascii_hexdigit)
        {
            self.at += 1;
        }
        let digits = self.text.get(start..self.at)?;
        if !self.eat(b'_') {
            return None;
        }
        let value = u128::from_str_radix(digits, 16).ok()?;
        Some(format!("{}{value}{ty}", if negative { "-" } else { "" }))
    }
}
