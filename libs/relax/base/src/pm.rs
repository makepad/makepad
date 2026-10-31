//! The token model Relax's built-in macros and derive inputs use (pm_tokens.rs). Plus the byte
//! encoding the sandboxed JIT proc-macro host (pmhost.rs) uses.

pub use crate::pm_tokens::*;

// ---------------------------------------------------------------- bridge encoding
// The byte form a macro's input/output crosses the JIT boundary in; mirrored by
// std/proc_macro (`__relax`). span = 7 x u32 (lo, hi, start line/col, end line/col, file);
// stream = u32 count + trees; tree = u8 tag (0 group, 1 ident, 2 punct, 3 literal) + span +
// group: u8 delim (0 paren, 1 brace, 2 bracket, 3 none) + stream | ident: u8 raw + text |
// punct: u32 char + u8 spacing (0 joint, 1 alone) | literal: text; text = u32 len + bytes.

fn w32(w: &mut Vec<u8>, v: u32) {
    w.extend_from_slice(&v.to_le_bytes());
}

fn wspan(w: &mut Vec<u8>, s: &Span) {
    for v in [s.lo, s.hi, s.start.line as u32, s.start.column as u32, s.end.line as u32, s.end.column as u32, s.file] {
        w32(w, v);
    }
}

fn wtext(w: &mut Vec<u8>, s: &str) {
    w32(w, s.len() as u32);
    w.extend_from_slice(s.as_bytes());
}

fn wstream(w: &mut Vec<u8>, trees: &[TokenTree]) {
    w32(w, trees.len() as u32);
    for t in trees {
        match t {
            TokenTree::Group(g) => {
                w.push(0);
                wspan(w, &g.span);
                w.push(match g.delim {
                    Delimiter::Parenthesis => 0,
                    Delimiter::Brace => 1,
                    Delimiter::Bracket => 2,
                    Delimiter::None => 3,
                });
                wstream(w, &g.stream.trees);
            }
            TokenTree::Ident(i) => {
                w.push(1);
                wspan(w, &i.span);
                w.push(i.raw as u8);
                wtext(w, &i.name);
            }
            TokenTree::Punct(p) => {
                w.push(2);
                wspan(w, &p.span);
                w32(w, p.ch as u32);
                w.push(if p.spacing == Spacing::Joint { 0 } else { 1 });
            }
            TokenTree::Literal(l) => {
                w.push(3);
                wspan(w, &l.span);
                wtext(w, &l.text);
            }
        }
    }
}

/// `call` span + stream, as the macro side reads it.
pub fn encode(call: Span, ts: &TokenStream) -> Vec<u8> {
    let mut w = Vec::new();
    wspan(&mut w, &call);
    wstream(&mut w, &ts.trees);
    w
}

struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Rd<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.i)?;
        self.i += 1;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.b.get(self.i..self.i + 4)?;
        self.i += 4;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn span(&mut self) -> Option<Span> {
        let (lo, hi, sl, sc, el, ec) = (self.u32()?, self.u32()?, self.u32()?, self.u32()?, self.u32()?, self.u32()?);
        let file = self.u32()?;
        Some(Span { lo, hi, start: LineColumn { line: sl as usize, column: sc as usize }, end: LineColumn { line: el as usize, column: ec as usize }, file })
    }
    fn text(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        let s = self.b.get(self.i..self.i + n)?;
        self.i += n;
        Some(String::from_utf8_lossy(s).into_owned())
    }
    fn stream(&mut self, depth: u32) -> Option<TokenStream> {
        if depth > 1024 {
            return None;
        }
        let n = self.u32()? as usize;
        let mut trees = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            let tag = self.u8()?;
            let span = self.span()?;
            trees.push(match tag {
                0 => {
                    let delim = match self.u8()? {
                        0 => Delimiter::Parenthesis,
                        1 => Delimiter::Brace,
                        2 => Delimiter::Bracket,
                        _ => Delimiter::None,
                    };
                    let stream = self.stream(depth + 1)?;
                    TokenTree::Group(Group { delim, stream, span })
                }
                1 => {
                    let raw = self.u8()? != 0;
                    let name = self.text()?;
                    TokenTree::Ident(Ident { name, raw, span })
                }
                2 => {
                    let ch = char::from_u32(self.u32()?)?;
                    let spacing = if self.u8()? == 0 { Spacing::Joint } else { Spacing::Alone };
                    TokenTree::Punct(Punct { ch, spacing, span })
                }
                3 => TokenTree::Literal(Literal { text: self.text()?, span }),
                _ => return None,
            });
        }
        Some(TokenStream { trees })
    }
}

/// A macro's output stream (no call span). `None`: malformed bytes.
pub fn decode_stream(b: &[u8]) -> Option<TokenStream> {
    let mut r = Rd { b, i: 0 };
    let s = r.stream(0)?;
    if r.i != b.len() {
        return None;
    }
    Some(s)
}
