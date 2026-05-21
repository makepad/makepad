use std::collections::HashMap;
use std::str::Chars;

#[derive(Default)]
pub struct TomlParser {
    pub cur: char,
    pub pos: usize,
}

#[derive(PartialEq, Debug, Clone)]
pub struct TomlSpan {
    pub start: usize,
    pub len: usize,
}

#[derive(PartialEq, Debug)]
pub struct TomlTokWithSpan {
    pub span: TomlSpan,
    pub tok: TomlTok,
}

#[derive(PartialEq, Debug)]
pub enum TomlTok {
    Ident(String),
    Str(String),
    U64(u64),
    I64(i64),
    F64(f64),
    Bool(bool),
    Nan(bool),
    Inf(bool),
    Date(String),
    Equals,
    BlockOpen,
    BlockClose,
    ObjectOpen,
    ObjectClose,
    Comma,
    Bof,
    Eof,
}

#[derive(PartialEq, Debug, Clone)]
pub enum Toml {
    Str(String, TomlSpan),
    Bool(bool, TomlSpan),
    Num(f64, TomlSpan),
    Date(String, TomlSpan),
    Array(Vec<Toml>),
    /// An inline table `{ k = v, ... }` encountered in a value position (e.g.
    /// an array element). Top-level inline tables in `key = { ... }` form are
    /// instead flattened into dotted keys by `parse_key_value`.
    Table(HashMap<String, Toml>),
}

impl Toml {
    pub fn into_str(self) -> Option<String> {
        match self {
            Self::Str(v, _) => Some(v),
            _ => None,
        }
    }
}

pub struct TomlErr {
    pub msg: String,
    pub span: TomlSpan,
}

impl std::fmt::Debug for TomlErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Toml error: {}, start:{} len:{}",
            self.msg, self.span.start, self.span.len
        )
    }
}

pub fn parse_toml(data: &str) -> Result<HashMap<String, Toml>, TomlErr> {
    let i = &mut data.chars();
    let mut t = TomlParser::default();
    t.next(i);
    let mut out = HashMap::new();
    let mut local_scope = String::new();
    loop {
        let tok = t.next_tok(i)?;
        match tok.tok {
            TomlTok::Eof => {
                // at eof.
                return Ok(out);
            }
            TomlTok::BlockOpen => {
                // A `[table]` or `[[array-of-tables]]` header. `next_tok`
                // consumed the first `[`; `read_table_header` handles an
                // optional second `[`, the dotted key path (each segment a
                // bare key or a quoted string, e.g. `patch."https://..."` or
                // `target.'cfg(...)'`), and the closing `]` / `]]`.
                local_scope = t.read_table_header(i)?;
            }
            TomlTok::Str(key) => {
                // a key
                t.parse_key_value(&local_scope, key, i, &mut out)?;
            }
            TomlTok::Ident(key) => {
                // also a key
                t.parse_key_value(&local_scope, key, i, &mut out)?;
            }
            _ => return Err(t.err_token(tok)),
        }
    }
}

impl TomlParser {
    pub fn to_val(&mut self, tok: TomlTokWithSpan, i: &mut Chars) -> Result<Toml, TomlErr> {
        match tok.tok {
            TomlTok::BlockOpen => {
                let mut vals = Vec::new();
                loop {
                    let tok = self.next_tok(i)?;
                    match tok.tok {
                        TomlTok::BlockClose | TomlTok::Eof => {
                            break;
                        }
                        TomlTok::Comma => {}
                        _ => {
                            vals.push(self.to_val(tok, i)?);
                        }
                    }
                }
                Ok(Toml::Array(vals))
            }
            TomlTok::ObjectOpen => {
                // An inline table `{ key = val, ... }` in value position —
                // e.g. an array element like `resources = [{ src = "..." }]`.
                let mut map = HashMap::new();
                loop {
                    let tok = self.next_tok(i)?;
                    match tok.tok {
                        TomlTok::ObjectClose | TomlTok::Eof => break,
                        TomlTok::Comma => {}
                        TomlTok::Str(key) | TomlTok::Ident(key) => {
                            let eq = self.next_tok(i)?;
                            if eq.tok != TomlTok::Equals {
                                return Err(self.err_token(eq));
                            }
                            let val_tok = self.next_tok(i)?;
                            let val = self.to_val(val_tok, i)?;
                            map.insert(key, val);
                        }
                        _ => return Err(self.err_token(tok)),
                    }
                }
                Ok(Toml::Table(map))
            }
            TomlTok::Str(v) => Ok(Toml::Str(v, tok.span)),
            TomlTok::U64(v) => Ok(Toml::Num(v as f64, tok.span)),
            TomlTok::I64(v) => Ok(Toml::Num(v as f64, tok.span)),
            TomlTok::F64(v) => Ok(Toml::Num(v, tok.span)),
            TomlTok::Bool(v) => Ok(Toml::Bool(v, tok.span)),
            TomlTok::Nan(v) => Ok(Toml::Num(
                if v { -std::f64::NAN } else { std::f64::NAN },
                tok.span,
            )),
            TomlTok::Inf(v) => Ok(Toml::Num(
                if v {
                    -std::f64::INFINITY
                } else {
                    std::f64::INFINITY
                },
                tok.span,
            )),
            TomlTok::Date(v) => Ok(Toml::Date(v, tok.span)),
            _ => Err(self.err_token(tok)),
        }
    }

    pub fn parse_key_value(
        &mut self,
        local_scope: &str,
        key: String,
        i: &mut Chars,
        out: &mut HashMap<String, Toml>,
    ) -> Result<(), TomlErr> {
        let tok = self.next_tok(i)?;
        if tok.tok != TomlTok::Equals {
            return Err(self.err_token(tok));
        }
        let tok = self.next_tok(i)?;
        // if we are an ObjectOpen we do a subscope
        if let TomlTok::ObjectOpen = tok.tok {
            let local_scope = if !local_scope.is_empty() {
                format!("{}.{}", local_scope, key)
            } else {
                key
            };
            loop {
                let tok = self.next_tok(i)?;
                match tok.tok {
                    TomlTok::ObjectClose | TomlTok::Eof => {
                        break;
                    }
                    TomlTok::Comma => {}
                    TomlTok::Str(key) => {
                        // a key
                        self.parse_key_value(&local_scope, key, i, out)?;
                    }
                    TomlTok::Ident(key) => {
                        // also a key
                        self.parse_key_value(&local_scope, key, i, out)?;
                    }
                    _ => return Err(self.err_token(tok)),
                }
            }
        } else {
            let val = self.to_val(tok, i)?;
            let key = if !local_scope.is_empty() {
                format!("{}.{}", local_scope, key)
            } else {
                key
            };
            out.insert(key, val);
        }
        Ok(())
    }

    pub fn next(&mut self, i: &mut Chars) {
        if let Some(c) = i.next() {
            self.cur = c;
            self.pos += 1;
        } else {
            self.cur = '\0';
        }
    }

    pub fn err_token(&self, tok: TomlTokWithSpan) -> TomlErr {
        TomlErr {
            msg: format!("Unexpected token {:?} ", tok),
            span: tok.span,
        }
    }

    pub fn err_parse(&self, what: &str) -> TomlErr {
        TomlErr {
            msg: format!("Cannot parse toml {} ", what),
            span: TomlSpan {
                start: self.pos,
                len: 0,
            },
        }
    }

    pub fn next_tok(&mut self, i: &mut Chars) -> Result<TomlTokWithSpan, TomlErr> {
        while self.cur == '\n'
            || self.cur == '\r'
            || self.cur == '\t'
            || self.cur == ' '
            || self.cur == '#'
        {
            if self.cur == '#' {
                while self.cur != '\n' && self.cur != '\0' {
                    self.next(i);
                }
                continue;
            }
            self.next(i);
        }
        let start = self.pos;
        match self.cur {
            '\0' => Ok(TomlTokWithSpan {
                tok: TomlTok::Eof,
                span: TomlSpan { start, len: 0 },
            }),
            ',' => {
                self.next(i);
                Ok(TomlTokWithSpan {
                    tok: TomlTok::Comma,
                    span: TomlSpan { start, len: 1 },
                })
            }
            '[' => {
                self.next(i);
                Ok(TomlTokWithSpan {
                    tok: TomlTok::BlockOpen,
                    span: TomlSpan { start, len: 1 },
                })
            }
            ']' => {
                self.next(i);
                Ok(TomlTokWithSpan {
                    tok: TomlTok::BlockClose,
                    span: TomlSpan { start, len: 1 },
                })
            }
            '{' => {
                self.next(i);
                Ok(TomlTokWithSpan {
                    tok: TomlTok::ObjectOpen,
                    span: TomlSpan { start, len: 1 },
                })
            }
            '}' => {
                self.next(i);
                Ok(TomlTokWithSpan {
                    tok: TomlTok::ObjectClose,
                    span: TomlSpan { start, len: 1 },
                })
            }
            '=' => {
                self.next(i);
                Ok(TomlTokWithSpan {
                    tok: TomlTok::Equals,
                    span: TomlSpan { start, len: 1 },
                })
            }
            '+' | '-' | '0'..='9' => {
                let mut num = String::new();
                let is_neg = if self.cur == '-' {
                    num.push(self.cur);
                    self.next(i);
                    true
                } else {
                    if self.cur == '+' {
                        self.next(i);
                    }
                    false
                };
                if self.cur == 'n' {
                    self.next(i);
                    if self.cur == 'a' {
                        self.next(i);
                        if self.cur == 'n' {
                            self.next(i);
                            return Ok(TomlTokWithSpan {
                                tok: TomlTok::Nan(is_neg),
                                span: TomlSpan {
                                    start,
                                    len: self.pos - start,
                                },
                            });
                        } else {
                            return Err(self.err_parse("nan"));
                        }
                    } else {
                        return Err(self.err_parse("nan"));
                    }
                }
                if self.cur == 'i' {
                    self.next(i);
                    if self.cur == 'n' {
                        self.next(i);
                        if self.cur == 'f' {
                            self.next(i);
                            return Ok(TomlTokWithSpan {
                                tok: TomlTok::Inf(is_neg),
                                span: TomlSpan {
                                    start,
                                    len: self.pos - start,
                                },
                            });
                        } else {
                            return Err(self.err_parse("inf"));
                        }
                    } else {
                        return Err(self.err_parse("nan"));
                    }
                }
                while self.cur >= '0' && self.cur <= '9' || self.cur == '_' {
                    if self.cur != '_' {
                        num.push(self.cur);
                    }
                    self.next(i);
                }
                if self.cur == '.' {
                    num.push(self.cur);
                    self.next(i);
                    while self.cur >= '0' && self.cur <= '9' || self.cur == '_' {
                        if self.cur != '_' {
                            num.push(self.cur);
                        }
                        self.next(i);
                    }
                    if let Ok(num) = num.parse() {
                        Ok(TomlTokWithSpan {
                            tok: TomlTok::F64(num),
                            span: TomlSpan {
                                start,
                                len: self.pos - start,
                            },
                        })
                    } else {
                        Err(self.err_parse("number"))
                    }
                } else if self.cur == '-' {
                    // lets assume its a date. whatever. i don't feel like more parsing today
                    num.push(self.cur);
                    self.next(i);
                    while self.cur >= '0' && self.cur <= '9'
                        || self.cur == ':'
                        || self.cur == '-'
                        || self.cur == 'T'
                    {
                        num.push(self.cur);
                        self.next(i);
                    }
                    return Ok(TomlTokWithSpan {
                        tok: TomlTok::Date(num),
                        span: TomlSpan {
                            start,
                            len: self.pos - start,
                        },
                    });
                } else {
                    if is_neg {
                        if let Ok(num) = num.parse() {
                            return Ok(TomlTokWithSpan {
                                tok: TomlTok::I64(num),
                                span: TomlSpan {
                                    start,
                                    len: self.pos - start,
                                },
                            });
                        } else {
                            return Err(self.err_parse("number"));
                        }
                    }
                    if let Ok(num) = num.parse() {
                        return Ok(TomlTokWithSpan {
                            tok: TomlTok::U64(num),
                            span: TomlSpan {
                                start,
                                len: self.pos - start,
                            },
                        });
                    } else {
                        return Err(self.err_parse("number"));
                    }
                }
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut ident = String::new();
                while self.cur >= 'a' && self.cur <= 'z'
                    || self.cur >= 'A' && self.cur <= 'Z'
                    || self.cur >= '0' && self.cur <= '9'
                    || self.cur == '_'
                    || self.cur == '-'
                    || self.cur == '.'
                    || self.cur == '\''
                    || self.cur == '('
                    || self.cur == ')'
                {
                    ident.push(self.cur);
                    self.next(i);
                }
                if ident == "true" {
                    return Ok(TomlTokWithSpan {
                        tok: TomlTok::Bool(true),
                        span: TomlSpan {
                            start,
                            len: self.pos - start,
                        },
                    });
                }
                if ident == "false" {
                    return Ok(TomlTokWithSpan {
                        tok: TomlTok::Bool(false),
                        span: TomlSpan {
                            start,
                            len: self.pos - start,
                        },
                    });
                }
                if ident == "inf" {
                    return Ok(TomlTokWithSpan {
                        tok: TomlTok::Inf(false),
                        span: TomlSpan {
                            start,
                            len: self.pos - start,
                        },
                    });
                }
                if ident == "nan" {
                    return Ok(TomlTokWithSpan {
                        tok: TomlTok::Nan(false),
                        span: TomlSpan {
                            start,
                            len: self.pos - start,
                        },
                    });
                }
                Ok(TomlTokWithSpan {
                    tok: TomlTok::Ident(ident),
                    span: TomlSpan {
                        start,
                        len: self.pos - start,
                    },
                })
            }

            '"' => {
                self.next(i);
                // Distinguish `"..."` (basic) from `"""..."""` (multi-line basic),
                // and the empty string `""`.
                let multiline = if self.cur == '"' {
                    self.next(i);
                    if self.cur == '"' {
                        self.next(i);
                        true
                    } else {
                        // `""` — an empty single-line basic string.
                        return Ok(TomlTokWithSpan {
                            tok: TomlTok::Str(String::new()),
                            span: TomlSpan {
                                start,
                                len: self.pos - start,
                            },
                        });
                    }
                } else {
                    false
                };
                if multiline {
                    // A newline immediately following the opening `"""` is
                    // trimmed (TOML spec).
                    if self.cur == '\r' {
                        self.next(i);
                    }
                    if self.cur == '\n' {
                        self.next(i);
                    }
                }
                let val = self.read_basic_string(i, multiline)?;
                Ok(TomlTokWithSpan {
                    tok: TomlTok::Str(val),
                    span: TomlSpan {
                        start,
                        len: self.pos - start,
                    },
                })
            }
            '\'' => {
                self.next(i);
                // Distinguish `'...'` (literal) from `'''...'''` (multi-line
                // literal), and the empty string `''`.
                let multiline = if self.cur == '\'' {
                    self.next(i);
                    if self.cur == '\'' {
                        self.next(i);
                        true
                    } else {
                        // `''` — an empty single-line literal string.
                        return Ok(TomlTokWithSpan {
                            tok: TomlTok::Str(String::new()),
                            span: TomlSpan {
                                start,
                                len: self.pos - start,
                            },
                        });
                    }
                } else {
                    false
                };
                if multiline {
                    if self.cur == '\r' {
                        self.next(i);
                    }
                    if self.cur == '\n' {
                        self.next(i);
                    }
                }
                let val = self.read_literal_string(i, multiline)?;
                Ok(TomlTokWithSpan {
                    tok: TomlTok::Str(val),
                    span: TomlSpan {
                        start,
                        len: self.pos - start,
                    },
                })
            }
            _ => Err(self.err_parse("tokenizer")),
        }
    }

    /// Read a `[table]` / `[[array-of-tables]]` header. `next_tok` has already
    /// consumed the first `[`. Handles an optional second `[`, a dotted key
    /// path whose segments may each be a bare key or a quoted string, and the
    /// closing `]` / `]]`. Returns the dotted path with quotes stripped, so
    /// `[a."b".c]` and `[a.b.c]` both yield `a.b.c`.
    pub fn read_table_header(&mut self, i: &mut Chars) -> Result<String, TomlErr> {
        let double = self.cur == '[';
        if double {
            self.next(i);
        }
        let mut path = String::new();
        loop {
            while self.cur == ' ' || self.cur == '\t' {
                self.next(i);
            }
            match self.cur {
                '\0' | '\n' | '\r' => return Err(self.err_parse("table header")),
                ']' => {
                    self.next(i);
                    if double {
                        if self.cur != ']' {
                            return Err(self.err_parse("table header"));
                        }
                        self.next(i);
                    }
                    return Ok(path);
                }
                // Dot separates segments; nothing to collect.
                '.' => {
                    self.next(i);
                }
                '"' => {
                    self.next(i);
                    let seg = self.read_basic_string(i, false)?;
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(&seg);
                }
                '\'' => {
                    self.next(i);
                    let seg = self.read_literal_string(i, false)?;
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(&seg);
                }
                _ => {
                    // A bare-key segment: read until a separator.
                    let mut seg = String::new();
                    while !matches!(
                        self.cur,
                        '.' | ']' | ' ' | '\t' | '\0' | '\n' | '\r'
                    ) {
                        seg.push(self.cur);
                        self.next(i);
                    }
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(&seg);
                }
            }
        }
    }

    /// Read a basic (double-quoted) string body. The opening delimiter — `"`
    /// for single-line, `"""` for multi-line — has already been consumed, as
    /// has the newline that immediately follows a `"""`. Processes backslash
    /// escape sequences. `multiline` selects which closing delimiter to expect.
    pub fn read_basic_string(
        &mut self,
        i: &mut Chars,
        multiline: bool,
    ) -> Result<String, TomlErr> {
        let mut val = String::new();
        loop {
            match self.cur {
                '\0' => return Err(self.err_parse("string")),
                // A raw newline can't appear in a single-line basic string.
                '\n' | '\r' if !multiline => return Err(self.err_parse("string")),
                '"' => {
                    if !multiline {
                        self.next(i);
                        return Ok(val);
                    }
                    // Multi-line: the closing delimiter is a run of >= 3 `"`.
                    // Quotes beyond the final three belong to the content.
                    let mut quotes = 0;
                    while self.cur == '"' {
                        quotes += 1;
                        self.next(i);
                    }
                    if quotes >= 3 {
                        for _ in 0..(quotes - 3) {
                            val.push('"');
                        }
                        return Ok(val);
                    }
                    for _ in 0..quotes {
                        val.push('"');
                    }
                }
                '\\' => {
                    self.next(i);
                    match self.cur {
                        'b' => {
                            val.push('\u{0008}');
                            self.next(i);
                        }
                        't' => {
                            val.push('\t');
                            self.next(i);
                        }
                        'n' => {
                            val.push('\n');
                            self.next(i);
                        }
                        'f' => {
                            val.push('\u{000C}');
                            self.next(i);
                        }
                        'r' => {
                            val.push('\r');
                            self.next(i);
                        }
                        '"' => {
                            val.push('"');
                            self.next(i);
                        }
                        '\\' => {
                            val.push('\\');
                            self.next(i);
                        }
                        'u' | 'U' => {
                            let digits = if self.cur == 'u' { 4 } else { 8 };
                            self.next(i);
                            let mut code: u32 = 0;
                            for _ in 0..digits {
                                let d = self
                                    .cur
                                    .to_digit(16)
                                    .ok_or_else(|| self.err_parse("unicode escape"))?;
                                code = code * 16 + d;
                                self.next(i);
                            }
                            val.push(
                                char::from_u32(code)
                                    .ok_or_else(|| self.err_parse("unicode escape"))?,
                            );
                        }
                        // Multi-line "line-ending backslash": a `\` followed by
                        // whitespace trims that whitespace, newlines included.
                        ' ' | '\t' | '\n' | '\r' if multiline => {
                            while matches!(self.cur, ' ' | '\t' | '\n' | '\r') {
                                self.next(i);
                            }
                        }
                        '\0' => return Err(self.err_parse("string")),
                        // Unknown escape: keep the char, drop the backslash.
                        _ => {
                            val.push(self.cur);
                            self.next(i);
                        }
                    }
                }
                c => {
                    val.push(c);
                    self.next(i);
                }
            }
        }
    }

    /// Read a literal (single-quoted) string body. The opening delimiter — `'`
    /// for single-line, `'''` for multi-line — has already been consumed, as
    /// has the newline that immediately follows a `'''`. Literal strings do NO
    /// escape processing: every character is taken verbatim.
    pub fn read_literal_string(
        &mut self,
        i: &mut Chars,
        multiline: bool,
    ) -> Result<String, TomlErr> {
        let mut val = String::new();
        loop {
            match self.cur {
                '\0' => return Err(self.err_parse("string")),
                '\n' | '\r' if !multiline => return Err(self.err_parse("string")),
                '\'' => {
                    if !multiline {
                        self.next(i);
                        return Ok(val);
                    }
                    let mut quotes = 0;
                    while self.cur == '\'' {
                        quotes += 1;
                        self.next(i);
                    }
                    if quotes >= 3 {
                        for _ in 0..(quotes - 3) {
                            val.push('\'');
                        }
                        return Ok(val);
                    }
                    for _ in 0..quotes {
                        val.push('\'');
                    }
                }
                c => {
                    val.push(c);
                    self.next(i);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_str<'a>(map: &'a HashMap<String, Toml>, key: &str) -> Option<&'a str> {
        match map.get(key) {
            Some(Toml::Str(v, _)) => Some(v.as_str()),
            _ => None,
        }
    }

    #[test]
    fn parses_single_line_strings() {
        let map = parse_toml("[package]\nname = \"robrix\"\nx = 'literal'\n").unwrap();
        assert_eq!(get_str(&map, "package.name"), Some("robrix"));
        assert_eq!(get_str(&map, "package.x"), Some("literal"));
    }

    #[test]
    fn parses_multiline_basic_string_and_following_keys() {
        // A `"""` value must not derail parsing of keys that come after it.
        let toml = "[package]\n\
                    version = \"1.0.0-alpha.1\"\n\
                    long_description = \"\"\"\n\
                    line one\n\
                    line two\n\
                    \"\"\"\n\
                    identifier = \"rs.robius.robrix\"\n";
        let map = parse_toml(toml).expect("multi-line basic string should parse");
        assert_eq!(get_str(&map, "package.version"), Some("1.0.0-alpha.1"));
        assert_eq!(get_str(&map, "package.identifier"), Some("rs.robius.robrix"));
        // Leading newline after `"""` is trimmed.
        assert_eq!(
            get_str(&map, "package.long_description"),
            Some("line one\nline two\n")
        );
    }

    #[test]
    fn parses_multiline_literal_string() {
        let toml = "[package]\n\
                    cmd = '''\n\
                    raw \\n not an escape\n\
                    '''\n\
                    after = \"ok\"\n";
        let map = parse_toml(toml).expect("multi-line literal string should parse");
        // Literal strings do not process escapes.
        assert_eq!(
            get_str(&map, "package.cmd"),
            Some("raw \\n not an escape\n")
        );
        assert_eq!(get_str(&map, "package.after"), Some("ok"));
    }

    #[test]
    fn parses_quoted_section_headers() {
        // `[patch."url"]` and `[target.'cfg(...)']` must not break parsing.
        let toml = "[patch.\"https://github.com/kevinaboos/makepad\"]\n\
                    makepad-widgets = \"1.0\"\n\
                    [target.'cfg(target_os = \"ios\")'.dependencies]\n\
                    foo = \"2.0\"\n\
                    [package.metadata.packager]\n\
                    identifier = \"rs.robius.robrix\"\n";
        let map = parse_toml(toml).expect("quoted section headers should parse");
        assert_eq!(
            get_str(&map, "package.metadata.packager.identifier"),
            Some("rs.robius.robrix")
        );
        // Quoted segments have their quotes stripped.
        assert_eq!(
            get_str(
                &map,
                "patch.https://github.com/kevinaboos/makepad.makepad-widgets"
            ),
            Some("1.0")
        );
    }

    #[test]
    fn basic_string_escapes_are_processed() {
        let map = parse_toml("[s]\na = \"tab\\there\"\nb = \"q\\\"q\"\n").unwrap();
        assert_eq!(get_str(&map, "s.a"), Some("tab\there"));
        assert_eq!(get_str(&map, "s.b"), Some("q\"q"));
    }

    #[test]
    fn array_of_tables_header_still_parses() {
        let map = parse_toml("[[bin]]\nname = \"app\"\n").unwrap();
        assert_eq!(get_str(&map, "bin.name"), Some("app"));
    }
}
