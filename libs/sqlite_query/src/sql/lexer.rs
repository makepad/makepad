//! SQL tokenizer.
//!
//! Follows SQLite's lexical rules (<https://www.sqlite.org/lang.html>):
//! `--` and `/* */` comments, single-quoted strings with `''` escapes,
//! `x'..'` blob literals, `"id"`, `[id]` and `` `id` `` quoted identifiers, and
//! the `?`, `?NNN`, `:name`, `@name`, `$name` parameter forms.

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// Bare or quoted identifier (keywords arrive here too; the parser decides).
    Ident { text: String, quoted: bool },
    Int(i64),
    Real(f64),
    Str(String),
    Blob(Vec<u8>),
    /// `?` / `?12` / `:name` / `@name` / `$name`
    Param(ParamRef),
    Punct(&'static str),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParamRef {
    /// Anonymous `?`; numbered left to right by the parser.
    Next,
    Index(u32),
    Name(String),
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub pos: usize,
}

impl Token {
    /// Case-insensitive keyword test for an unquoted identifier.
    pub fn is_kw(&self, kw: &str) -> bool {
        match &self.tok {
            Tok::Ident { text, quoted } => !quoted && text.eq_ignore_ascii_case(kw),
            _ => false,
        }
    }
    pub fn is_punct(&self, p: &str) -> bool {
        matches!(&self.tok, Tok::Punct(x) if *x == p)
    }
    pub fn ident_text(&self) -> Option<&str> {
        match &self.tok {
            Tok::Ident { text, .. } => Some(text),
            _ => None,
        }
    }
}

const PUNCT3: [&str; 0] = [];
const PUNCT2: [&str; 6] = ["==", "!=", "<>", "<=", ">=", "||"];
const PUNCT2_SHIFT: [&str; 2] = ["<<", ">>"];
const PUNCT1: [&str; 16] = [
    "=", "<", ">", "+", "-", "*", "/", "%", "(", ")", ",", ".", ";", "&", "|", "~",
];

pub fn tokenize(sql: &str) -> Result<Vec<Token>> {
    let b = sql.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    let _ = PUNCT3;
    while i < b.len() {
        let c = b[i];
        // whitespace
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // comments
        if c == b'-' && i + 1 < b.len() && b[i + 1] == b'-' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let mut j = i + 2;
            loop {
                if j + 1 >= b.len() {
                    return Err(Error::sql("unterminated /* comment"));
                }
                if b[j] == b'*' && b[j + 1] == b'/' {
                    break;
                }
                j += 1;
            }
            i = j + 2;
            continue;
        }
        let pos = i;
        // string literal
        if c == b'\'' {
            let (s, next) = read_quoted(b, i, b'\'')?;
            let text = String::from_utf8(s)
                .map_err(|_| Error::sql("string literal is not valid UTF-8"))?;
            out.push(Token {
                tok: Tok::Str(text),
                pos,
            });
            i = next;
            continue;
        }
        // quoted identifiers
        if c == b'"' || c == b'`' {
            let (s, next) = read_quoted(b, i, c)?;
            out.push(Token {
                tok: Tok::Ident {
                    text: String::from_utf8_lossy(&s).into_owned(),
                    quoted: true,
                },
                pos,
            });
            i = next;
            continue;
        }
        if c == b'[' {
            let end = b[i + 1..]
                .iter()
                .position(|&x| x == b']')
                .ok_or_else(|| Error::sql("unterminated [identifier]"))?;
            let text = String::from_utf8_lossy(&b[i + 1..i + 1 + end]).into_owned();
            out.push(Token {
                tok: Tok::Ident { text, quoted: true },
                pos,
            });
            i = i + end + 2;
            continue;
        }
        // blob literal x'..'
        if (c == b'x' || c == b'X') && i + 1 < b.len() && b[i + 1] == b'\'' {
            let (s, next) = read_quoted(b, i + 1, b'\'')?;
            if s.len() % 2 != 0 {
                return Err(Error::sql("blob literal has an odd number of hex digits"));
            }
            let mut bytes = Vec::with_capacity(s.len() / 2);
            for pair in s.chunks(2) {
                let hi = hex_val(pair[0]).ok_or_else(|| Error::sql("bad hex digit in blob"))?;
                let lo = hex_val(pair[1]).ok_or_else(|| Error::sql("bad hex digit in blob"))?;
                bytes.push(hi * 16 + lo);
            }
            out.push(Token {
                tok: Tok::Blob(bytes),
                pos,
            });
            i = next;
            continue;
        }
        // identifier / keyword
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$') {
                i += 1;
            }
            out.push(Token {
                tok: Tok::Ident {
                    text: sql[start..i].to_string(),
                    quoted: false,
                },
                pos,
            });
            continue;
        }
        // number
        if c.is_ascii_digit() || (c == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let start = i;
            if c == b'0' && i + 1 < b.len() && (b[i + 1] == b'x' || b[i + 1] == b'X') {
                i += 2;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let v = u64::from_str_radix(&sql[start + 2..i], 16)
                    .map_err(|_| Error::sql("bad hexadecimal literal"))?;
                out.push(Token {
                    tok: Tok::Int(v as i64),
                    pos,
                });
                continue;
            }
            let mut is_real = false;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == b'.' {
                is_real = true;
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    is_real = true;
                    i = j;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text = &sql[start..i];
            if is_real {
                out.push(Token {
                    tok: Tok::Real(text.parse().map_err(|_| Error::sql("bad numeric literal"))?),
                    pos,
                });
            } else {
                match text.parse::<i64>() {
                    Ok(v) => out.push(Token {
                        tok: Tok::Int(v),
                        pos,
                    }),
                    Err(_) => out.push(Token {
                        tok: Tok::Real(
                            text.parse().map_err(|_| Error::sql("bad numeric literal"))?,
                        ),
                        pos,
                    }),
                }
            }
            continue;
        }
        // parameters
        if c == b'?' {
            i += 1;
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let p = if i > start {
                ParamRef::Index(
                    sql[start..i]
                        .parse()
                        .map_err(|_| Error::sql("bad parameter number"))?,
                )
            } else {
                ParamRef::Next
            };
            out.push(Token {
                tok: Tok::Param(p),
                pos,
            });
            continue;
        }
        if c == b':' || c == b'@' || c == b'$' {
            let start = i + 1;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if i == start {
                return Err(Error::sql("named parameter without a name"));
            }
            out.push(Token {
                tok: Tok::Param(ParamRef::Name(sql[start..i].to_string())),
                pos,
            });
            continue;
        }
        // operators
        let rest = &sql[i..];
        let mut matched = None;
        for p in PUNCT2.iter().chain(PUNCT2_SHIFT.iter()) {
            if rest.starts_with(p) {
                matched = Some(*p);
                break;
            }
        }
        if matched.is_none() {
            for p in PUNCT1.iter() {
                if rest.starts_with(p) {
                    matched = Some(*p);
                    break;
                }
            }
        }
        match matched {
            Some(p) => {
                out.push(Token {
                    tok: Tok::Punct(p),
                    pos,
                });
                i += p.len();
            }
            None => {
                return Err(Error::sql(format!(
                    "unexpected character {:?} at offset {i}",
                    c as char
                )))
            }
        }
    }
    out.push(Token {
        tok: Tok::Eof,
        pos: b.len(),
    });
    Ok(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Read a quoted run starting at `start` (which holds the opening quote),
/// handling doubled quotes. Returns (contents, index after the closing quote).
fn read_quoted(b: &[u8], start: usize, quote: u8) -> Result<(Vec<u8>, usize)> {
    let mut out = Vec::new();
    let mut i = start + 1;
    loop {
        if i >= b.len() {
            return Err(Error::sql("unterminated quoted token"));
        }
        if b[i] == quote {
            if i + 1 < b.len() && b[i + 1] == quote {
                out.push(quote);
                i += 2;
                continue;
            }
            return Ok((out, i + 1));
        }
        out.push(b[i]);
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s).unwrap().into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn basic_select() {
        let t = toks("SELECT a, b FROM t WHERE x = ?1 AND y <> 'it''s';");
        assert!(matches!(&t[0], Tok::Ident { text, .. } if text == "SELECT"));
        assert!(t.contains(&Tok::Param(ParamRef::Index(1))));
        assert!(t.contains(&Tok::Str("it's".into())));
        assert!(t.contains(&Tok::Punct("<>")));
    }

    #[test]
    fn literals() {
        assert_eq!(toks("x'01ff'")[0], Tok::Blob(vec![1, 255]));
        assert_eq!(toks("0x10")[0], Tok::Int(16));
        assert_eq!(toks("1.5e2")[0], Tok::Real(150.0));
        assert_eq!(toks("-3")[0], Tok::Punct("-"));
        assert_eq!(
            toks("\"quoted id\"")[0],
            Tok::Ident {
                text: "quoted id".into(),
                quoted: true
            }
        );
    }

    #[test]
    fn comments_skipped() {
        let t = toks("SELECT -- hi\n 1 /* there */ , 2");
        assert_eq!(t.len(), 5); // SELECT 1 , 2 EOF
    }

    #[test]
    fn bitwise_not_tokenizes() {
        assert_eq!(toks("~x")[0], Tok::Punct("~"));
    }

    #[test]
    fn errors_are_clean() {
        assert!(tokenize("SELECT 'unterminated").is_err());
        assert!(tokenize("SELECT /* unterminated").is_err());
        assert!(tokenize("SELECT #").is_err());
    }
}
