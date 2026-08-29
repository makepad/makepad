#![allow(unstable_name_collisions)]

use {
    makepad_micro_proc_macro::{TokenBuilder, TokenParser},
    proc_macro::{Delimiter, Span, TokenStream},
    std::fmt::Write,
};

pub fn script_mod_impl(input: TokenStream) -> TokenStream {
    let mut tb = TokenBuilder::new();
    let ts = script_impl(input);
    tb.add("pub fn script_mod(vm:&mut ScriptVm)->ScriptValue{");
    tb.add("    let sb=").stream(Some(ts)).add(";");
    tb.add("    vm.eval(sb)");
    tb.add("}");
    tb.end()
}

pub fn script_apply_eval_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();

    // Parse: cx, target, { script code }
    // First get the cx expression
    let cx_expr = parser.eat_any_ident().expect("Expected cx identifier");
    parser.eat_punct_alone(',');

    // Get the target expression (could be self.draw_bg or similar)
    let mut target_tb = TokenBuilder::new();
    while !parser.eat_punct_alone(',') {
        if let Some(tt) = parser.current.clone() {
            target_tb.extend(tt);
            parser.advance();
        } else {
            break;
        }
    }
    let target_stream = target_tb.end();

    // The rest is the script code (already includes braces from eat_level)
    // Prepend __script_source__ to make it __script_source__{...}
    let script_code: TokenStream = {
        let mut tb = TokenBuilder::new();
        tb.add("__script_source__");
        tb.stream(Some(parser.eat_level()));
        tb.end()
    };

    // Generate the script_impl output (the ScriptMod struct)
    // Use script_impl_expr to NOT add semicolon - we want to return the expression value
    let script_mod = script_impl(script_code);

    // Build: cx.with_vm(|vm| { let script = ScriptMod{...}; target.script_apply_eval(vm, script) })
    tb.ident(&cx_expr).add(".with_vm(|vm|{");
    tb.add("let script =").stream(Some(script_mod)).add(";");
    tb.stream(Some(target_stream));
    tb.add(".script_apply_eval(vm, script)");
    tb.add("})");

    tb.end()
}

pub fn script_impl(input: TokenStream) -> TokenStream {
    let mut parser = TokenParser::new(input);
    let mut tb = TokenBuilder::new();

    if let Some(span) = parser.span() {
        let (s, values) = token_parser_to_whitespace_matching_string(&mut parser, span);

        tb.add("ScriptMod {");
        tb.add("    cargo_manifest_path: env!(")
            .string("CARGO_MANIFEST_DIR")
            .add(").trim_start_matches(")
            .string("\\\\?\\")
            .add(").to_string(),");
        tb.add("    module_path :")
            .ident_with_span("module_path", span)
            .add("!().to_string(),");
        tb.add("    file:")
            .ident_with_span("file", span)
            .add("!().to_string().replace(")
            .string("\\")
            .add(",")
            .string("/")
            .add("),");
        tb.add("    line:line!() as usize,");
        tb.add("    column:column!() as usize,");

        tb.add("    code:").string(&s).add(".to_string(),");
        tb.add("    values:{");
        // Deliberately obscure name: `#(expr)` interpolations are spliced in
        // here verbatim, so a user variable named like ours (it was `v`)
        // would resolve to the half-built Vec instead of their value.
        tb.add("        let mut __script_interp_vals = Vec::new();");
        for value in &values {
            tb.add("__script_interp_vals.push( {")
                .stream(Some(value.clone()))
                .add("}.script_to_value(vm) );");
        }
        tb.add("    __script_interp_vals}");
        tb.add("}");
    } else {
        tb.add("ScriptMod::default()");
    }
    tb.end()
}

// sself function parses tokens into a source-equal whitespaced output string
fn token_parser_to_whitespace_matching_string(
    parser: &mut TokenParser,
    span: Span,
) -> (String, Vec<TokenStream>) {
    let mut s = String::new();
    let mut values = Vec::new();

    tp_to_str(parser, span, &mut s, &mut values, &mut None);
    s.push(';');
    return (s, values);

    #[derive(Clone, Copy)]
    struct Lc {
        line: usize,
        column: usize,
    }

    impl Lc {
        fn _next_char(self) -> Self {
            Self {
                line: self.line,
                column: self.column + 1,
            }
        }
    }

    fn delim_to_pair(delim: Delimiter) -> (char, char) {
        match delim {
            Delimiter::Brace => ('{', '}'),
            Delimiter::Parenthesis => ('(', ')'),
            Delimiter::Bracket => ('[', ']'),
            Delimiter::None => (' ', ' '),
        }
    }

    fn tp_to_str(
        parser: &mut TokenParser,
        span: Span,
        out: &mut String,
        values: &mut Vec<TokenStream>,
        last_end: &mut Option<Lc>,
    ) {
        fn lc_from_start(span: Span) -> Lc {
            Lc {
                line: span.start().line(),
                column: span.start().column(),
            }
        }

        fn lc_from_end(span: Span) -> Lc {
            Lc {
                line: span.end().line(),
                column: span.end().column(),
            }
        }

        fn delta_whitespace(now: Lc, needed: Lc, out: &mut String) {
            if now.line == needed.line {
                for _ in now.column..needed.column {
                    out.push(' ');
                }
            } else {
                for _ in now.line..needed.line {
                    out.push('\n');
                }
                for _ in 1..needed.column {
                    out.push(' ');
                }
            }
        }

        if last_end.is_none() {
            *last_end = Some(lc_from_start(span));
        }

        // `///` doc comments reach the macro as `#[doc = "..."]` (and `//!`
        // as `#![doc = "..."]`). Given a bracket group's inner TokenStream,
        // return the doc text if it is such an attribute body.
        fn doc_text_of(inner: &TokenStream) -> Option<String> {
            let mut it = inner.clone().into_iter();
            match it.next() {
                Some(TokenTree::Ident(id)) if id.to_string() == "doc" => (),
                _ => return None,
            }
            match it.next() {
                Some(TokenTree::Punct(p)) if p.as_char() == '=' => (),
                _ => return None,
            }
            let lit = match it.next() {
                Some(TokenTree::Literal(lit)) => lit.to_string(),
                _ => return None,
            };
            // Undo string-literal quoting so the reconstructed source is
            // byte-identical to what the user wrote after `///`.
            if let Some(raw) = lit.strip_prefix('r') {
                let raw = raw.trim_start_matches('#');
                let raw = raw.strip_prefix('"')?;
                let raw = raw.trim_end_matches('#');
                return Some(raw.strip_suffix('"')?.to_string());
            }
            let body = lit.strip_prefix('"')?.strip_suffix('"')?;
            let mut text = String::with_capacity(body.len());
            let mut chars = body.chars();
            while let Some(c) = chars.next() {
                if c != '\\' {
                    text.push(c);
                    continue;
                }
                match chars.next() {
                    Some('n') => text.push('\n'),
                    Some('r') => text.push('\r'),
                    Some('t') => text.push('\t'),
                    Some('0') => text.push('\0'),
                    Some('u') => {
                        // \u{XXXX}
                        let mut hex = String::new();
                        for h in chars.by_ref() {
                            if h == '{' {
                                continue;
                            }
                            if h == '}' {
                                break;
                            }
                            hex.push(h);
                        }
                        if let Ok(v) = u32::from_str_radix(&hex, 16) {
                            if let Some(u) = char::from_u32(v) {
                                text.push(u);
                            }
                        }
                    }
                    Some(other) => text.push(other),
                    None => (),
                }
            }
            Some(text)
        }

        let mut last_tt = None;

        while !parser.eat_eot() {
            let span = parser.span().unwrap();
            if let Some(delim) = parser.open_group() {
                if delim == Delimiter::Bracket {
                    // Peek: is this the body of a doc attribute?
                    let inner = parser.eat_level();
                    if let Some(text) = doc_text_of(&inner) {
                        // The synthesized `#` (and `!` for the `//!` form)
                        // preceding this group was already emitted — remove.
                        if out.ends_with("#!") {
                            out.pop();
                            out.pop();
                        } else if out.ends_with('#') {
                            out.pop();
                        }
                        last_tt = None;
                        // Two Rust spellings share the #[doc] token form,
                        // but only ONE is the splash annotation grammar:
                        // `/** ... */` blocks. `///` lines fold back as
                        // plain `//` comments (inert to the runtime
                        // tokenizer — deliberately NOT an annotation).
                        // Recover which spelling this was from the span:
                        // a comment spanning lines is a block; on one line,
                        // width == text + 5 is `/**text*/`, width == text
                        // + 3 is `///text`. Anything inexact stays a block
                        // (never silently drop an annotation).
                        let start = lc_from_start(span);
                        let end = lc_from_end(span);
                        let text_chars = text.chars().count();
                        let is_line_doc = end.line == start.line
                            && end.column.saturating_sub(start.column) == text_chars + 3;
                        if is_line_doc {
                            out.push_str("//");
                            out.push_str(&text);
                        } else {
                            out.push_str("/**");
                            out.push_str(&text);
                            out.push_str("*/");
                        }
                        *last_end = Some(end);
                        continue;
                    }
                    // Not a doc attribute: re-render the bracket group
                    // literally from the consumed stream.
                    let start = lc_from_start(span);
                    let end = lc_from_end(span);
                    delta_whitespace(last_end.unwrap(), start, out);
                    out.push('[');
                    *last_end = Some(start._next_char());
                    let mut sub = TokenParser::new(inner);
                    tp_to_str(&mut sub, span, out, values, last_end);
                    delta_whitespace(
                        last_end.unwrap(),
                        Lc {
                            line: end.line,
                            column: end.column - 1,
                        },
                        out,
                    );
                    *last_end = Some(end);
                    out.push(']');
                    last_tt = None;
                    continue;
                }
                if let Some(TokenTree::Punct(last_punct)) = &last_tt {
                    if last_punct.as_char() == '#' && delim == Delimiter::Parenthesis {
                        last_tt = None;
                        out.pop();
                        let index = values.len();
                        write!(out, "#({index})").unwrap();
                        values.push(parser.eat_level());
                        continue;
                    }
                }

                let (gs, ge) = delim_to_pair(delim);
                let start = lc_from_start(span);
                let end = lc_from_end(span);
                delta_whitespace(last_end.unwrap(), start, out);
                out.push(gs);
                *last_end = Some(start._next_char());
                tp_to_str(parser, span, out, values, last_end);
                delta_whitespace(
                    last_end.unwrap(),
                    Lc {
                        line: end.line,
                        column: end.column - 1,
                    },
                    out,
                );
                *last_end = Some(end);
                out.push(ge);
            } else {
                if let Some(tt) = &parser.current {
                    {
                        last_tt = Some(tt.clone());
                        let start = lc_from_start(span);
                        delta_whitespace(last_end.unwrap(), start, out);
                    }

                    out.push_str(&tt.to_string());

                    *last_end = Some(lc_from_end(span));
                }
                parser.advance();
            }
        }
    }
}

use proc_macro::TokenTree;
