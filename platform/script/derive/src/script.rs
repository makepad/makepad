#![allow(unstable_name_collisions)]

use {
    makepad_micro_proc_macro::{error_span, TokenBuilder, TokenParser},
    proc_macro::{Delimiter, Span, TokenStream, TokenTree},
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

    let cx_expr = parser.eat_any_ident().expect("Expected cx identifier");
    parser.eat_punct_alone(',');

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

    let script_code: TokenStream = {
        let mut tb = TokenBuilder::new();
        tb.add("__script_source__");
        tb.stream(Some(parser.eat_level()));
        tb.end()
    };

    let script_mod = script_impl(script_code);

    tb.ident(&cx_expr).add(".with_vm(|vm|{");
    tb.add("let script =").stream(Some(script_mod)).add(";");
    tb.stream(Some(target_stream));
    tb.add(".script_apply_eval(vm, script)");
    tb.add("})");

    tb.end()
}

/// One piece of a body's rendered output.
///
/// `Text` is verbatim DSL source (whitespace-preserving). `Placeholder` is a
/// Rust expression captured from a `#( ... )` site. `Conditional` is a
/// `#[cfg(...)]`-gated sub-list of chunks; conditionals are recognised at any
/// depth and flattened into their enclosing chunk list at emit time (the
/// enclosing group's `{` / `}` end up as ordinary Text in the surrounding
/// chunks, while the gated content becomes its own Conditional sibling).
#[derive(Clone)]
enum Chunk {
    Text(String),
    Placeholder(TokenStream),
    Conditional {
        cfg_expr: TokenStream,
        chunks: Vec<Chunk>,
    },
}

pub fn script_impl(input: TokenStream) -> TokenStream {
    let tokens: Vec<TokenTree> = input.into_iter().collect();
    if tokens.is_empty() {
        let mut tb = TokenBuilder::new();
        tb.add("ScriptMod::default()");
        return tb.end();
    }

    let first_span = tokens[0].span();
    let initial_lc = Lc::start_of(first_span);

    let mut state = WalkState::new(initial_lc);
    if let Err(err) = walk(&tokens, &mut state) {
        return err;
    }
    // Trailing `;` to terminate the DSL statement (matches today's behaviour).
    state.buf.push(';');
    let chunks = state.into_chunks();

    if !chunks_contain_conditional(&chunks) {
        emit_no_conditional(&chunks, first_span)
    } else {
        emit_with_conditional(&chunks, first_span)
    }
}

fn chunks_contain_conditional(chunks: &[Chunk]) -> bool {
    chunks.iter().any(|c| match c {
        Chunk::Conditional { .. } => true,
        Chunk::Text(_) | Chunk::Placeholder(_) => false,
    })
}

// ----- core walker ------------------------------------------------------------

#[derive(Clone, Copy)]
struct Lc {
    line: usize,
    column: usize,
}

impl Lc {
    fn start_of(span: Span) -> Self {
        let s = span.start();
        Lc {
            line: s.line(),
            column: s.column(),
        }
    }

    fn end_of(span: Span) -> Self {
        let e = span.end();
        Lc {
            line: e.line(),
            column: e.column(),
        }
    }

    fn after_char(self) -> Self {
        Lc {
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

/// Walker state. `buf` accumulates text into the *current* Text chunk;
/// non-Text chunks (Placeholder, Conditional) get pushed to `chunks` with the
/// `buf` flushed before them.
struct WalkState {
    chunks: Vec<Chunk>,
    buf: String,
    last_end: Lc,
}

impl WalkState {
    fn new(last_end: Lc) -> Self {
        Self {
            chunks: Vec::new(),
            buf: String::new(),
            last_end,
        }
    }

    fn flush_buf(&mut self) {
        if !self.buf.is_empty() {
            self.chunks.push(Chunk::Text(std::mem::take(&mut self.buf)));
        }
    }

    fn into_chunks(mut self) -> Vec<Chunk> {
        self.flush_buf();
        self.chunks
    }
}

fn walk(tokens: &[TokenTree], state: &mut WalkState) -> Result<(), TokenStream> {
    let mut i = 0usize;
    let mut prev_was_hash = false;

    while i < tokens.len() {
        // Attribute detection at any depth — disambiguate `#[...]` shapes.
        if is_punct_char(&tokens[i], '#') {
            // `#![...]` — inner attribute, rejected.
            if matches!(tokens.get(i + 1), Some(tt) if is_punct_char(tt, '!')) {
                if matches!(tokens.get(i + 2), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket)
                {
                    return Err(error_span(
                        "script_mod! does not accept inner attributes; use #[cfg(...)] not #![cfg(...)]",
                        tokens[i].span(),
                    ));
                }
            }

            // `#[...]` — outer attribute (cfg, or rejected).
            if let Some(TokenTree::Group(g)) = tokens.get(i + 1) {
                if g.delimiter() == Delimiter::Bracket {
                    let attr_tokens: Vec<TokenTree> = g.stream().into_iter().collect();
                    let attr_span = g.span();

                    let head = match attr_tokens.first() {
                        Some(TokenTree::Ident(id)) => id.to_string(),
                        _ => {
                            return Err(error_span(
                                "script_mod! only accepts #[cfg(...)] attributes",
                                attr_span,
                            ));
                        }
                    };

                    if head == "cfg_attr" {
                        return Err(error_span(
                            "script_mod! does not support cfg_attr; use #[cfg(...)] directly",
                            attr_span,
                        ));
                    }
                    if head != "cfg" {
                        return Err(error_span(
                            "script_mod! only accepts #[cfg(...)] attributes",
                            attr_span,
                        ));
                    }

                    let cfg_paren = attr_tokens.get(1);
                    let cfg_stream = match cfg_paren {
                        Some(TokenTree::Group(pg)) if pg.delimiter() == Delimiter::Parenthesis => {
                            pg.stream()
                        }
                        _ => {
                            return Err(error_span(
                                "malformed cfg in script_mod!: expected `cfg(<predicate>)`",
                                attr_span,
                            ));
                        }
                    };
                    if attr_tokens.len() > 2 {
                        return Err(error_span(
                            "malformed cfg in script_mod!: unexpected tokens after `cfg(...)`",
                            attr_span,
                        ));
                    }

                    validate_cfg_expr(cfg_stream.clone())?;

                    // Emit the whitespace gap from the previous token's end to
                    // the cfg attribute's `#`, then flush. The gap serves as
                    // the DSL separator between the pre-text and the (skipped
                    // or included) conditional body. Without this, an active
                    // conditional whose pre-text ends with an identifier would
                    // smash directly into the conditional body's first token —
                    // e.g. `state` followed by `let` becomes `statelet` and
                    // the DSL parser can't bind the let.
                    let hash_start = Lc::start_of(tokens[i].span());
                    delta_whitespace(state.last_end, hash_start, &mut state.buf);
                    state.flush_buf();

                    // Consume the cfg attribute (the `#` and the bracket group).
                    let after_attr = i + 2;

                    // Now consume the next statement.
                    let (body_tokens, after_body) =
                        consume_statement(tokens, after_attr, attr_span)?;

                    // Recursively walk the body. Start last_end at body's first
                    // token's span start (so the body's chunks contain no
                    // leading whitespace; the gap before the body would have
                    // been pre-text, which we already trimmed away).
                    let body_first_lc = body_tokens
                        .first()
                        .map(|t| Lc::start_of(t.span()))
                        .unwrap_or(Lc::end_of(attr_span));
                    let mut inner = WalkState::new(body_first_lc);
                    walk(&body_tokens, &mut inner)?;
                    let body_chunks = inner.into_chunks();

                    state.chunks.push(Chunk::Conditional {
                        cfg_expr: cfg_stream,
                        chunks: body_chunks,
                    });

                    // Outer last_end: end of body's last token (or cfg-end if
                    // body was empty, which `consume_statement` rejects).
                    state.last_end = body_tokens
                        .last()
                        .map(|t| Lc::end_of(t.span()))
                        .unwrap_or(Lc::end_of(attr_span));

                    prev_was_hash = false;
                    i = after_body;
                    continue;
                }
            }

            // Falls through to regular token handling — `#` will go into buf
            // (or be popped if the next token forms a `#(...)` placeholder).
        }

        let tt = &tokens[i];

        if let TokenTree::Group(g) = tt {
            // `#( ... )` placeholder.
            if prev_was_hash && g.delimiter() == Delimiter::Parenthesis {
                state.buf.pop(); // drop the `#`
                state.flush_buf();
                state.chunks.push(Chunk::Placeholder(g.stream()));
                prev_was_hash = false;
                i += 1;
                continue;
            }

            // Regular group: emit `{`, recurse, emit `}`. Inner conditionals
            // get flattened into our chunk list.
            let span = g.span();
            let (open, close) = delim_to_pair(g.delimiter());
            let start = Lc::start_of(span);
            let end = Lc::end_of(span);
            delta_whitespace(state.last_end, start, &mut state.buf);
            state.buf.push(open);
            let inner_start_lc = start.after_char();
            state.last_end = inner_start_lc;

            let inner_tokens: Vec<TokenTree> = g.stream().into_iter().collect();
            let inner_first_lc = inner_tokens
                .first()
                .map(|t| Lc::start_of(t.span()))
                .unwrap_or(inner_start_lc);
            let mut inner = WalkState::new(inner_first_lc);
            walk(&inner_tokens, &mut inner)?;
            // Trailing whitespace inside group, from last inner token to one
            // column before the close delimiter.
            let close_before = Lc {
                line: end.line,
                column: end.column.saturating_sub(1),
            };
            delta_whitespace(inner.last_end, close_before, &mut inner.buf);

            // Merge inner state into outer state: leading Text chunks of
            // `inner.chunks` extend the outer buf; non-Text chunks become
            // outer siblings. Trailing inner.buf appends to outer buf.
            for chunk in inner.chunks {
                match chunk {
                    Chunk::Text(s) => state.buf.push_str(&s),
                    other => {
                        state.flush_buf();
                        state.chunks.push(other);
                    }
                }
            }
            state.buf.push_str(&inner.buf);

            state.last_end = end;
            state.buf.push(close);
            prev_was_hash = false;
            i += 1;
            continue;
        }

        // Regular non-group token.
        let span = tt.span();
        let start = Lc::start_of(span);
        delta_whitespace(state.last_end, start, &mut state.buf);
        let text = tt.to_string();
        state.buf.push_str(&text);
        state.last_end = Lc::end_of(span);
        prev_was_hash = matches!(tt, TokenTree::Punct(p) if p.as_char() == '#');
        i += 1;
    }

    Ok(())
}

/// Determine the token span that a `#[cfg(...)]` gates: either the next brace
/// group (outer braces stripped) or a single statement (terminated by the close
/// of its first encountered brace group, or by a depth-zero newline).
fn consume_statement(
    tokens: &[TokenTree],
    start: usize,
    attr_span: Span,
) -> Result<(Vec<TokenTree>, usize), TokenStream> {
    if start >= tokens.len() {
        return Err(error_span(
            "script_mod! cfg attribute has no following item",
            attr_span,
        ));
    }

    // Brace-grouped form: leading token is `{ ... }`.
    if let TokenTree::Group(g) = &tokens[start] {
        if g.delimiter() == Delimiter::Brace {
            let inner: Vec<TokenTree> = g.stream().into_iter().collect();
            return Ok((inner, start + 1));
        }
    }

    // Single-statement form.
    let mut collected = Vec::new();
    let mut i = start;
    let mut last_end_line: Option<usize> = None;
    let first_token_line = tokens[start].span().start().line();

    while i < tokens.len() {
        let tok = &tokens[i];
        let span = tok.span();
        let start_line = span.start().line();

        if let Some(last) = last_end_line {
            if start_line > last {
                break;
            }
        }

        collected.push(tok.clone());
        last_end_line = Some(span.end().line());
        i += 1;

        if let TokenTree::Group(g) = tok {
            if g.delimiter() == Delimiter::Brace {
                return Ok((collected, i));
            }
        }
    }

    if let Some(end_line) = last_end_line {
        if end_line > first_token_line {
            return Err(error_span(
                "script_mod! single-statement #[cfg(...)] cannot span multiple lines without a brace group — wrap the gated content in { … }",
                attr_span,
            ));
        }
    }

    if collected.is_empty() {
        return Err(error_span(
            "script_mod! cfg attribute has no following item",
            attr_span,
        ));
    }

    Ok((collected, i))
}

fn validate_cfg_expr(stream: TokenStream) -> Result<(), TokenStream> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    validate_cfg_predicate(&tokens)
}

fn validate_cfg_predicate(tokens: &[TokenTree]) -> Result<(), TokenStream> {
    let head = match tokens.first() {
        Some(TokenTree::Ident(id)) => id,
        _ => {
            let span = tokens
                .first()
                .map(|t| t.span())
                .unwrap_or_else(Span::call_site);
            return Err(error_span(
                "malformed cfg in script_mod!: expected an identifier",
                span,
            ));
        }
    };
    let head_str = head.to_string();
    let head_span = head.span();

    match head_str.as_str() {
        "feature" => {
            if tokens.len() != 3 {
                return Err(error_span(
                    "malformed cfg in script_mod!: `feature` must be followed by `= \"...\"`",
                    head_span,
                ));
            }
            if !is_punct_char(&tokens[1], '=') {
                return Err(error_span(
                    "malformed cfg in script_mod!: expected `=` after `feature`",
                    tokens[1].span(),
                ));
            }
            match &tokens[2] {
                TokenTree::Literal(_) => Ok(()),
                _ => Err(error_span(
                    "malformed cfg in script_mod!: `feature` value must be a string literal",
                    tokens[2].span(),
                )),
            }
        }
        "not" | "any" | "all" => {
            if tokens.len() != 2 {
                return Err(error_span(
                    "malformed cfg in script_mod!: combinator must be followed by a single `(...)` group",
                    head_span,
                ));
            }
            let inner = match &tokens[1] {
                TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis => g.stream(),
                _ => {
                    return Err(error_span(
                        "malformed cfg in script_mod!: combinator must be followed by `(...)`",
                        tokens[1].span(),
                    ));
                }
            };

            let inner_tokens: Vec<TokenTree> = inner.into_iter().collect();

            if head_str == "not" {
                validate_cfg_predicate(&inner_tokens)
            } else {
                if inner_tokens.is_empty() {
                    return Err(error_span(
                        "malformed cfg in script_mod!: `any`/`all` requires at least one inner predicate",
                        head_span,
                    ));
                }
                let mut cursor = 0usize;
                let mut start = 0usize;
                while cursor < inner_tokens.len() {
                    if is_punct_char(&inner_tokens[cursor], ',') {
                        if cursor > start {
                            validate_cfg_predicate(&inner_tokens[start..cursor])?;
                        }
                        start = cursor + 1;
                    }
                    cursor += 1;
                }
                if start < inner_tokens.len() {
                    validate_cfg_predicate(&inner_tokens[start..])?;
                }
                Ok(())
            }
        }
        other => Err(error_span(
            &format!(
                "script_mod! only supports cfg(feature = \"…\"), not(...), any(...), all(...); got `{}`",
                other
            ),
            head_span,
        )),
    }
}

fn is_punct_char(tt: &TokenTree, c: char) -> bool {
    matches!(tt, TokenTree::Punct(p) if p.as_char() == c)
}

// ----- emission ---------------------------------------------------------------

fn emit_struct_header(tb: &mut TokenBuilder, span: Span) {
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
}

/// Today's emission shape: a single literal `code` string with hardcoded
/// `#(N)` indices, a flat `values:` vec, and an empty `cfg_fragments` vec.
/// Used when no conditional chunks exist anywhere in the body.
fn emit_no_conditional(chunks: &[Chunk], span: Span) -> TokenStream {
    let mut tb = TokenBuilder::new();

    let mut code = String::new();
    let mut values: Vec<TokenStream> = Vec::new();
    for chunk in chunks {
        match chunk {
            Chunk::Text(s) => code.push_str(s),
            Chunk::Placeholder(ts) => {
                let _ = write!(code, "#({})", values.len());
                values.push(ts.clone());
            }
            Chunk::Conditional { .. } => {
                unreachable!("emit_no_conditional called with conditional chunks")
            }
        }
    }

    tb.add("ScriptMod {");
    emit_struct_header(&mut tb, span);
    tb.add("    code:").string(&code).add(".to_string(),");
    tb.add("    values:{");
    tb.add("        let mut v = Vec::new();");
    for value in &values {
        tb.add("v.push( {")
            .stream(Some(value.clone()))
            .add("}.script_to_value(vm) );");
    }
    tb.add("    v},");
    tb.add("    cfg_fragments: Vec::new(),");
    tb.add("}");

    tb.end()
}

/// Builder shape used whenever any conditional chunk exists (anywhere in the
/// tree, including nested inside groups). Every fragment uses runtime chunked
/// emission so `#(N)` placeholder indices renumber against the live
/// `values.len()` regardless of which conditionals rustc selected.
///
/// `cfg_fragments` is built up-front via N unconditional
/// `__cfg_fragments.push(cfg!(<expr>))` calls in pre-order. This keeps
/// `cfg_fragments.len()` invariant across feature toggles, which the
/// hot-reload extractor relies on for filtering.
fn emit_with_conditional(chunks: &[Chunk], span: Span) -> TokenStream {
    let mut tb = TokenBuilder::new();

    tb.add("{");
    tb.add("    use ::std::fmt::Write as _;");
    tb.add("    let mut __code = ::std::string::String::new();");
    tb.add("    let mut __values: ::std::vec::Vec<ScriptValue> = ::std::vec::Vec::new();");
    tb.add("    let mut __cfg_fragments: ::std::vec::Vec<bool> = ::std::vec::Vec::new();");

    // First pass: emit `__cfg_fragments.push(cfg!(<expr>))` for every
    // Conditional in pre-order, outside any cfg guard. Length invariant.
    emit_cfg_fragment_pushes(&mut tb, chunks);

    // Second pass: emit the actual chunked code-building, with `#[cfg(...)]`
    // guards wrapping each conditional's body.
    for chunk in chunks {
        emit_chunk(&mut tb, chunk);
    }

    tb.add("    ScriptMod {");
    emit_struct_header(&mut tb, span);
    tb.add("        code: __code,");
    tb.add("        values: __values,");
    tb.add("        cfg_fragments: __cfg_fragments,");
    tb.add("    }");
    tb.add("}");

    tb.end()
}

fn emit_cfg_fragment_pushes(tb: &mut TokenBuilder, chunks: &[Chunk]) {
    for chunk in chunks {
        if let Chunk::Conditional { cfg_expr, chunks: inner } = chunk {
            tb.add("    __cfg_fragments.push(cfg!(")
                .stream(Some(cfg_expr.clone()))
                .add("));");
            emit_cfg_fragment_pushes(tb, inner);
        }
    }
}

fn emit_chunk(tb: &mut TokenBuilder, chunk: &Chunk) {
    match chunk {
        Chunk::Text(s) => {
            if s.is_empty() {
                return;
            }
            tb.add("    __code.push_str(").string(s).add(");");
        }
        Chunk::Placeholder(ts) => {
            tb.add("    {");
            tb.add("        let __i = __values.len();");
            tb.add("        __values.push( {")
                .stream(Some(ts.clone()))
                .add("}.script_to_value(vm) );");
            tb.add("        let _ = ::std::write!(&mut __code, ");
            tb.string("#({})").add(", __i);");
            tb.add("    }");
        }
        Chunk::Conditional { cfg_expr, chunks } => {
            tb.add("    #[cfg(")
                .stream(Some(cfg_expr.clone()))
                .add(")]");
            tb.add("    {");
            for c in chunks {
                emit_chunk(tb, c);
            }
            tb.add("    }");
        }
    }
}
