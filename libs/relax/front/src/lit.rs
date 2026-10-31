//! Literal and format-string helpers of the front end: string/byte literal values, the
//! format-string grammar (`{}` pieces and specs), and two syntactic predicates used by typeck,
//! borrowck and lowering.

use crate::ast::*;

pub fn strip_paren(ast: &Ast, mut e: ExprId) -> ExprId {
    while let ExprKind::Paren(x) = &ast.expr(e).kind {
        e = *x;
    }
    e
}

/// Value of a (possibly raw) string literal token.
pub fn unescape_str(text: &str) -> String {
    let b = unescape_bytes_text(text);
    String::from_utf8_lossy(&b).into_owned()
}

pub fn unescape_bytes(text: &str) -> Vec<u8> {
    let b = unescape_bytes_text(text);
    if b.first() == Some(&0) {
        if let Some(p) = crate::builtins::payload_of(&b) {
            return (*p).clone();
        }
    }
    b
}

fn unescape_bytes_text(text: &str) -> Vec<u8> {
    // rustc reads sources with CRLF normalized to LF (rustc_span normalize_src): a literal that
    // spans lines of a CRLF file holds `\n`
    let norm;
    let text = if text.contains("\r\n") {
        norm = text.replace("\r\n", "\n");
        norm.as_str()
    } else {
        text
    };
    let t = text.as_bytes();
    let mut i = 0;
    // prefix b / c / r
    while i < t.len() && (t[i] == b'b' || t[i] == b'c') {
        i += 1;
    }
    let raw = i < t.len() && t[i] == b'r';
    if raw {
        i += 1;
        let mut hashes = 0;
        while i < t.len() && t[i] == b'#' {
            hashes += 1;
            i += 1;
        }
        let start = i + 1;
        let end = t.len() - 1 - hashes;
        let end = end.max(start);
        // strip suffix-free closing quote
        let mut e = end;
        while e > start && t[e] != b'"' {
            e -= 1;
        }
        return t[start..e].to_vec();
    }
    let q = t[i];
    i += 1;
    let mut out = Vec::new();
    while i < t.len() && t[i] != q {
        if t[i] == b'\\' && i + 1 < t.len() {
            i += 1;
            match t[i] {
                b'n' => out.push(b'\n'),
                b't' => out.push(b'\t'),
                b'r' => out.push(b'\r'),
                b'0' => out.push(0),
                b'\\' => out.push(b'\\'),
                b'\'' => out.push(b'\''),
                b'"' => out.push(b'"'),
                b'x' => {
                    let h = std::str::from_utf8(&t[i + 1..i + 3]).unwrap_or("0");
                    out.push(u8::from_str_radix(h, 16).unwrap_or(0));
                    i += 2;
                }
                b'u' => {
                    // \u{XXXX}
                    let mut j = i + 2;
                    let mut v = 0u32;
                    while j < t.len() && t[j] != b'}' {
                        if t[j] != b'_' {
                            v = v * 16 + (t[j] as char).to_digit(16).unwrap_or(0);
                        }
                        j += 1;
                    }
                    let c = char::from_u32(v).unwrap_or('\u{fffd}');
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    i = j;
                }
                b'\n' => {
                    // line continuation: skip whitespace
                    while i + 1 < t.len() && (t[i + 1] == b' ' || t[i + 1] == b'\n' || t[i + 1] == b'\t' || t[i + 1] == b'\r') {
                        i += 1;
                    }
                }
                c => out.push(c),
            }
            i += 1;
        } else {
            out.push(t[i]);
            i += 1;
        }
    }
    out
}

#[derive(Clone, Copy, Default)]
pub struct FmtSpec {
    pub fill: u32,
    /// 0 none, 1 left, 2 center, 3 right
    pub align: u8,
    pub plus: bool,
    pub alt: bool,
    pub zero: bool,
    pub width: u16,
    pub precision: Option<u16>,
    /// 0 Display, b'?' Debug, b'x', b'X', b'b', b'o', b'e'
    pub ty: u8,
}

impl FmtSpec {
    pub fn pack(&self) -> u64 {
        let mut v = 0u64;
        v |= self.width as u64;
        if let Some(p) = self.precision {
            v |= (p as u64) << 16;
            v |= 1 << 32;
        }
        v |= (self.align as u64) << 33;
        v |= (self.plus as u64) << 35;
        v |= (self.alt as u64) << 36;
        v |= (self.zero as u64) << 37;
        v |= (self.ty as u64) << 40;
        let fill = if self.fill == 0 { ' ' as u32 } else { self.fill };
        v |= (fill as u64 & 0x1f_ffff) << 48 >> 5 << 5;
        v
    }
}

pub enum FmtPiece {
    Lit(String),
    Arg { index: Option<usize>, named: Option<String>, spec: FmtSpec },
}

pub fn parse_fmt(s: &str) -> Vec<FmtPiece> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '{' {
            if i + 1 < c.len() && c[i + 1] == '{' {
                lit.push('{');
                i += 2;
                continue;
            }
            if !lit.is_empty() {
                out.push(FmtPiece::Lit(std::mem::take(&mut lit)));
            }
            let mut j = i + 1;
            let mut arg = String::new();
            while j < c.len() && c[j] != '}' && c[j] != ':' {
                arg.push(c[j]);
                j += 1;
            }
            let mut spec = FmtSpec::default();
            if j < c.len() && c[j] == ':' {
                j += 1;
                let mut sp = String::new();
                while j < c.len() && c[j] != '}' {
                    sp.push(c[j]);
                    j += 1;
                }
                spec = parse_spec(&sp);
            }
            let arg = arg.trim().to_string();
            let (index, named) = if arg.is_empty() {
                (None, None)
            } else if let Ok(n) = arg.parse::<usize>() {
                (Some(n), None)
            } else {
                (None, Some(arg))
            };
            out.push(FmtPiece::Arg { index, named, spec });
            i = j + 1;
        } else if c[i] == '}' {
            lit.push('}');
            i += if i + 1 < c.len() && c[i + 1] == '}' { 2 } else { 1 };
        } else {
            lit.push(c[i]);
            i += 1;
        }
    }
    if !lit.is_empty() {
        out.push(FmtPiece::Lit(lit));
    }
    out
}

fn parse_spec(s: &str) -> FmtSpec {
    let c: Vec<char> = s.chars().collect();
    let mut sp = FmtSpec::default();
    let mut i = 0;
    let is_align = |x: char| x == '<' || x == '^' || x == '>';
    if c.len() >= 2 && is_align(c[1]) {
        sp.fill = c[0] as u32;
        sp.align = match c[1] {
            '<' => 1,
            '^' => 2,
            _ => 3,
        };
        i = 2;
    } else if !c.is_empty() && is_align(c[0]) {
        sp.align = match c[0] {
            '<' => 1,
            '^' => 2,
            _ => 3,
        };
        i = 1;
    }
    if i < c.len() && c[i] == '+' {
        sp.plus = true;
        i += 1;
    }
    if i < c.len() && c[i] == '#' {
        sp.alt = true;
        i += 1;
    }
    if i < c.len() && c[i] == '0' {
        sp.zero = true;
        i += 1;
    }
    let mut w = 0u16;
    while i < c.len() && c[i].is_ascii_digit() {
        w = w * 10 + c[i].to_digit(10).unwrap() as u16;
        i += 1;
    }
    sp.width = w;
    if i < c.len() && c[i] == '.' {
        i += 1;
        let mut p = 0u16;
        while i < c.len() && c[i].is_ascii_digit() {
            p = p * 10 + c[i].to_digit(10).unwrap() as u16;
            i += 1;
        }
        sp.precision = Some(p);
    }
    if i < c.len() {
        sp.ty = c[i] as u8;
    }
    sp
}

/// Is `a` the left side of a destructuring assignment (`(a, b) = ..`, `[x, ..] = ..`,
/// `S { f, .. } = ..`, `_ = ..`)?
pub fn is_assignee_pattern(ast: &crate::ast::Ast, a: ExprId) -> bool {
    match &ast.expr(a).kind {
        // `(..) = e` (a parenthesized rest is the empty tuple pattern with a rest)
        ExprKind::Paren(x) if matches!(ast.expr(*x).kind, ExprKind::Range(OptExprId::NONE, OptExprId::NONE, false)) => true,
        ExprKind::Paren(x) => is_assignee_pattern(ast, *x),
        ExprKind::Tuple(_) | ExprKind::Array(_) | ExprKind::Struct(..) | ExprKind::Underscore => true,
        // a tuple-struct / variant constructor (`S(a, b) = v`), as rustc's assignee grammar
        ExprKind::Call(f, _) => matches!(ast.expr(*f).kind, ExprKind::Path(_)),
        _ => false,
    }
}
