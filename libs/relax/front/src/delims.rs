//! A file whose delimiters do not pair up. rustc builds token trees before anything parses
//! (rustc_parse lexer/tokentrees.rs, lexer/diagnostics.rs), so this is such a file's one error,
//! wherever a parser would run out of tokens. Asked only once a parse has failed: a file that
//! parses pairs up.

use crate::lexer::{T, Tok};

fn spelling(kind: T) -> &'static str {
    match kind {
        T::OpenParen => "(",
        T::CloseParen => ")",
        T::OpenBracket => "[",
        T::CloseBracket => "]",
        T::OpenBrace => "{",
        _ => "}",
    }
}

fn pairs(open: T, close: T) -> bool {
    matches!((open, close), (T::OpenParen, T::CloseParen) | (T::OpenBracket, T::CloseBracket) | (T::OpenBrace, T::CloseBrace))
}

/// rustc_span `SourceMap::span_to_margin`: the white space before the line's first token.
fn margin(src: &[u8], pos: u32) -> usize {
    let line = &src[src[..pos as usize].iter().rposition(|c| *c == b'\n').map_or(0, |i| i + 1)..pos as usize];
    line.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(line.len())
}

/// The brace blocks that closed, and the one-line `{}`s (an editor may have put those in).
#[derive(Default)]
struct Blocks {
    closed: Vec<(u32, u32)>,
    empty: Vec<u32>,
}

impl Blocks {
    /// rustc's `report_suspicious_mismatch_block`: the innermost block that closed at another
    /// indentation than it opened, outside any block that closed at its own: most likely the block
    /// that took the lost brace's partner.
    fn suspicious(&self, src: &[u8]) -> Option<(u32, u32)> {
        let mut blocks: Vec<(u32, u32, bool)> = self.closed.iter().map(|&(o, c)| (o, c, margin(src, o) == margin(src, c))).collect();
        blocks.sort_by_key(|b| b.0);
        for i in 0..blocks.len() {
            let (_, close, same) = blocks[i];
            if same {
                // (by position: what it contains comes right after it)
                for inner in blocks[i + 1..].iter_mut().take_while(|b| b.0 < close) { inner.2 = true; }
            }
        }
        blocks.iter().rev().find(|b| !b.2).map(|b| (b.0, b.1))
    }
}

/// `src`'s tokens up to its end: the position and text of the error its delimiters make, if they do.
pub fn delimiter_error(src: &[u8], toks: impl Iterator<Item = Tok>) -> Option<(u32, String)> {
    let starts = crate::source::line_starts(src);
    let at = |pos: u32| { let (l, c) = crate::source::line_col(src, &starts, pos); format!("{l}:{c}") };
    let mut open: Vec<(T, u32)> = Vec::new();
    let mut blocks = Blocks::default();
    let mut prev = T::Eof;
    let mismatch_hint = |blocks: &Blocks| match blocks.suspicious(src) {
        Some((o, c)) => format!("; the `{{` at {} might not be properly closed: the `}}` at {} matches it but has a different indentation", at(o), at(c)),
        None => blocks.closed.last().map_or(String::new(), |&(o, c)| format!("; the last block that closed is the `{{` at {} with the `}}` at {}", at(o), at(c))),
    };
    for t in toks {
        match t.kind {
            T::OpenParen | T::OpenBracket | T::OpenBrace => open.push((t.kind, t.lo)),
            T::CloseParen | T::CloseBracket | T::CloseBrace => match open.last().copied() {
                None => return Some((t.lo, format!("unexpected closing delimiter: `{}`{}", spelling(t.kind), mismatch_hint(&blocks)))),
                Some((o, pos)) if pairs(o, t.kind) => {
                    open.pop();
                    if o == T::OpenBrace {
                        blocks.closed.push((pos, t.lo));
                        if prev == T::OpenBrace && !src[pos as usize..t.lo as usize].contains(&b'\n') { blocks.empty.push(pos); }
                    }
                }
                Some((o, pos)) => {
                    // (rustc's candidate: the last open delimiter of this kind at this indentation)
                    let meant = open.iter().rev().find(|&&(d, p)| pairs(d, t.kind) && margin(src, p) == margin(src, t.lo));
                    let meant = meant.map_or(String::new(), |&(d, p)| format!("; it may be meant for the `{}` at {}", spelling(d), at(p)));
                    return Some((t.lo, format!("mismatched closing delimiter: `{}`: the `{}` at {} is unclosed{}", spelling(t.kind), spelling(o), at(pos), meant)));
                }
            },
            T::Eof => break,
            _ => {}
        }
        prev = t.kind;
    }
    let &(kind, innermost) = open.last()?;
    let unclosed = open.iter().rev().map(|&(d, p)| format!("`{}` at {}", spelling(d), at(p))).collect::<Vec<_>>().join(", ");
    Some(match blocks.suspicious(src) {
        Some((o, c)) => {
            let empty = blocks.empty.iter().find(|&&p| o < p && p < c);
            let empty = empty.map_or(String::new(), |&p| format!("; the block at {} is empty, you might not have meant to close it", at(p)));
            (o, format!("this file contains an unclosed delimiter: this `{{` might not be properly closed: the `}}` at {} matches it but has a different indentation{empty}; left unclosed: {unclosed}", at(c)))
        }
        None => (innermost, format!("this file contains an unclosed delimiter: this `{}` is not closed; left unclosed: {unclosed}", spelling(kind))),
    })
}

#[cfg(test)]
mod tests {
    use super::delimiter_error;

    fn error(src: &str) -> Option<(String, String)> {
        let mut toks = Vec::new();
        crate::lexer::lex(src.as_bytes(), 2021, &mut toks).map_err(|e| e.msg).unwrap();
        delimiter_error(src.as_bytes(), toks.into_iter()).map(|(pos, msg)| {
            let (l, c) = crate::source::line_col(src.as_bytes(), &crate::source::line_starts(src.as_bytes()), pos);
            (format!("{l}:{c}"), msg)
        })
    }

    #[test]
    fn delimiters_that_pair_up_are_no_error() {
        assert_eq!(error("mod a {\n    fn f(x: [u8; 2]) { (x); }\n}\n// } ( not code\nconst S: &str = \"}\";\n"), None);
    }

    /// A body lost its last brace: the next `}` closes it, at the wrong indentation, and the module
    /// stays open. rustc points at the body, which is where the brace went.
    #[test]
    fn a_lost_closing_brace_points_at_the_block_that_took_the_next_one() {
        let (at, msg) = error("#[derive(Clone)]\nstruct A;\nmod tests {\n    fn fine() {\n    }\n    fn lost() {\n        if a {\n    }\n}\n").unwrap();
        assert_eq!(at, "7:14");
        assert!(msg.starts_with("this file contains an unclosed delimiter: this `{` might not be properly closed: the `}` at 8:5 matches it"), "{msg}");
        assert!(msg.ends_with("left unclosed: `{` at 3:11"), "{msg}");
    }

    /// A block inside one that closed at its own indentation is no suspect.
    #[test]
    fn the_suspect_is_outside_any_block_that_closed_where_it_opened() {
        let (at, _) = error("fn fine() {\n  if a {\n    }\n}\nfn lost() {\n  if b {\n}\n").unwrap();
        assert_eq!(at, "6:8");
    }

    #[test]
    fn with_no_suspect_the_innermost_open_delimiter_is_the_place() {
        let (at, msg) = error("fn f() {\n    g(\n").unwrap();
        assert_eq!(at, "2:6");
        assert_eq!(msg, "this file contains an unclosed delimiter: this `(` is not closed; left unclosed: `(` at 2:6, `{` at 1:8");
    }

    #[test]
    fn an_empty_block_inside_the_suspect_is_named() {
        let (at, msg) = error("mod m {\n    fn lost() {\n        if a {}\n}\n").unwrap();
        assert_eq!(at, "2:15");
        assert!(msg.contains("the block at 3:14 is empty"), "{msg}");
    }

    #[test]
    fn a_closer_with_nothing_open_is_unexpected() {
        let (at, msg) = error("fn f() {\n    }\n}\n").unwrap();
        assert_eq!(at, "3:1");
        assert!(msg.starts_with("unexpected closing delimiter: `}`; the `{` at 1:8 might not be properly closed"), "{msg}");
    }

    #[test]
    fn a_closer_of_another_kind_is_mismatched_and_says_what_it_may_be_for() {
        let (at, msg) = error("fn f() {\n    g(a];\n}\n").unwrap();
        assert_eq!((at.as_str(), msg.as_str()), ("2:8", "mismatched closing delimiter: `]`: the `(` at 2:6 is unclosed"));
        let (at, msg) = error("fn f() {\n    g(a\n}\n").unwrap();
        assert_eq!((at.as_str(), msg.as_str()), ("3:1", "mismatched closing delimiter: `}`: the `(` at 2:6 is unclosed; it may be meant for the `{` at 1:8"));
    }
}
