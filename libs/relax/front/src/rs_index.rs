//! Names defined by a Rust module before macro expansion.
use crate::pm_tokens::{Span as TokSpan, TokenStream, Delimiter, TokenTree};

#[derive(Clone, Debug, Default)]
pub struct NameIndex {
    pub items: Vec<IndexItem>,
    pub impls: Vec<IndexImpl>,
    pub macros: Vec<IndexMacro>,
    pub uses: Vec<IndexUse>,
    pub modules: Vec<IndexMod>,
    /// Inner `#![cfg(...)]` and `#![cfg_attr(...)]` attributes on this module.
    /// Outer attributes are retained on the individual entries below.
    pub cfgs: Vec<TokenStream>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Struct,
    Enum,
    Union,
    Trait,
    Fn,
    Const,
    Static,
    Type,
    Mod,
    Macro,
    UseReexport,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Vis {
    #[default]
    Private,
    Public,
    /// The restriction inside `pub(...)`, in token spelling.
    Restricted(String),
}

#[derive(Clone, Debug)]
pub struct IndexItem {
    pub name: String,
    pub kind: ItemKind,
    pub vis: Vis,
    /// The name token's origin (or the `*` for a glob reexport).
    pub span: TokSpan,
    /// Complete outer cfg/cfg_attr attributes, in their authored order.
    pub cfg: Option<TokenStream>,
}

#[derive(Clone, Debug)]
pub struct IndexImpl {
    pub trait_path: Option<String>,
    /// The qualified path, with each generic argument replaced by `_`.
    /// Non-path self types retain their shape, e.g. `&[_]` or `(_,_)`.
    pub self_path: String,
    pub span: TokSpan,
    pub cfg: Option<TokenStream>,
}

#[derive(Clone, Debug)]
pub struct IndexMacro {
    pub name: String,
    pub macro_export: bool,
    /// Zero-based position among retained module items, before use-tree expansion.
    pub order: usize,
    pub span: TokSpan,
    pub cfg: Option<TokenStream>,
}

#[derive(Clone, Debug)]
pub struct IndexUse {
    /// One leaf of a use tree, including `::*` for a glob.
    pub path: String,
    pub alias: Option<String>,
    pub glob: bool,
    pub vis: Vis,
    pub order: usize,
    pub span: TokSpan,
    pub cfg: Option<TokenStream>,
}

#[derive(Clone, Debug)]
pub struct IndexMod {
    pub name: String,
    /// The string literal spelling from `#[path = ...]`, including its quotes.
    pub path: Option<String>,
    pub inline: Option<Box<NameIndex>>,
    pub vis: Vis,
    pub order: usize,
    pub span: TokSpan,
    pub cfg: Option<TokenStream>,
}

fn tokens_equal(a: &TokenStream, b: &TokenStream) -> bool {
    a.trees.len() == b.trees.len()
        && a.trees.iter().zip(&b.trees).all(|(a, b)| {
            a.span() == b.span()
                && match (a, b) {
                    (TokenTree::Ident(a), TokenTree::Ident(b)) => a.name == b.name && a.raw == b.raw,
                    (TokenTree::Literal(a), TokenTree::Literal(b)) => a.text == b.text,
                    (TokenTree::Punct(a), TokenTree::Punct(b)) => {
                        a.as_char() == b.as_char() && a.spacing() == b.spacing()
                    }
                    (TokenTree::Group(a), TokenTree::Group(b)) => {
                        a.delimiter() == b.delimiter() && tokens_equal(&a.stream, &b.stream)
                    }
                    _ => false,
                }
        })
}

fn cfg_equal(a: &Option<TokenStream>, b: &Option<TokenStream>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => tokens_equal(a, b),
        _ => false,
    }
}

macro_rules! entry_eq {
    ($ty:ident, $($field:ident),+ $(,)?) => {
        impl PartialEq for $ty {
            fn eq(&self, other: &Self) -> bool {
                $(self.$field == other.$field &&)+ cfg_equal(&self.cfg, &other.cfg)
            }
        }
        impl Eq for $ty {}
    };
}
entry_eq!(IndexItem, name, kind, vis, span);
entry_eq!(IndexImpl, trait_path, self_path, span);
entry_eq!(IndexMacro, name, macro_export, order, span);
entry_eq!(IndexUse, path, alias, glob, vis, order, span);
entry_eq!(IndexMod, name, path, inline, vis, order, span);

impl PartialEq for NameIndex {
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
            && self.impls == other.impls
            && self.macros == other.macros
            && self.uses == other.uses
            && self.modules == other.modules
            && self.cfgs.len() == other.cfgs.len()
            && self.cfgs.iter().zip(&other.cfgs).all(|(a, b)| tokens_equal(a, b))
    }
}
impl Eq for NameIndex {}

/// Index a lowered Rust module without expanding macros or inspecting bodies.
/// Lists preserve output order; grouped uses are expanded left to right.
/// Configuration tokens and all origin spans are preserved verbatim.
pub fn of_tokens(tokens: &TokenStream) -> NameIndex {
    let mut index = NameIndex::default();
    let mut order = 0;
    index_items(&tokens.trees, &mut index, &mut order, &None);
    index
}

fn ident(token: Option<&TokenTree>) -> &str {
    match token {
        Some(TokenTree::Ident(value)) => &value.name,
        _ => "",
    }
}

fn word(token: Option<&TokenTree>, expected: &str) -> bool {
    matches!(token, Some(TokenTree::Ident(value)) if !value.raw && value.name == expected)
}

fn punct(token: Option<&TokenTree>, expected: char) -> bool {
    matches!(token, Some(TokenTree::Punct(value)) if value.as_char() == expected)
}

fn group(token: Option<&TokenTree>, delim: Delimiter) -> Option<&TokenStream> {
    match token {
        Some(TokenTree::Group(value)) if value.delimiter() == delim => Some(&value.stream),
        _ => None,
    }
}

/// Delimiter::None is transparent to Rust's item parser. Flatten only those
/// groups. On this uncommon path, opaque body groups are cloned along with
/// the headers; their contents are not interpreted as module items.
fn transparent(tokens: &[TokenTree]) -> Vec<TokenTree> {
    let mut out = Vec::new();
    for token in tokens {
        if let Some(inner) = group(Some(token), Delimiter::None) {
            out.extend(transparent(&inner.trees));
        } else {
            out.push(token.clone());
        }
    }
    out
}

#[derive(Default)]
struct Attributes {
    cfg: Option<TokenStream>,
    path: Option<String>,
    macro_export: bool,
}

fn attributes(tokens: &[TokenTree], at: &mut usize, index: &mut NameIndex) -> Attributes {
    let mut attrs = Attributes::default();
    while punct(tokens.get(*at), '#') {
        let start = *at;
        let inner = punct(tokens.get(start + 1), '!');
        let end = start + if inner { 3 } else { 2 };
        let Some(body) = group(tokens.get(end - 1), Delimiter::Bracket) else { break };
        let body = &body.trees;
        match ident(body.first()) {
            "cfg" | "cfg_attr" => {
                let cfg = TokenStream { trees: tokens[start..end].to_vec() };
                if inner {
                    index.cfgs.push(cfg);
                } else {
                    attrs.cfg.get_or_insert_with(TokenStream::new).trees.extend(cfg.trees);
                }
            }
            "macro_export" if !inner => attrs.macro_export = true,
            "path" if !inner && punct(body.get(1), '=') => {
                if let Some(TokenTree::Literal(value)) = body.get(2) {
                    attrs.path = Some(value.text.clone());
                }
            }
            _ => {}
        }
        *at = end;
    }
    attrs
}

fn visibility(tokens: &[TokenTree], at: &mut usize) -> Vis {
    if !word(tokens.get(*at), "pub") { return Vis::Private; }
    *at += 1;
    if let Some(restriction) = group(tokens.get(*at), Delimiter::Parenthesis) {
        *at += 1;
        Vis::Restricted(restriction.to_string())
    } else {
        Vis::Public
    }
}

fn combine_cfg(inherited: &Option<TokenStream>, cfg: Option<TokenStream>) -> Option<TokenStream> {
    match (inherited, cfg) {
        (None, cfg) => cfg,
        (Some(inherited), cfg) => {
            let mut inherited = inherited.clone();
            if let Some(cfg) = cfg { inherited.trees.extend(cfg.trees); }
            Some(inherited)
        }
    }
}

fn index_items(tokens: &[TokenTree], index: &mut NameIndex, order: &mut usize, inherited: &Option<TokenStream>) {
    let flat;
    let tokens = if tokens.iter().any(|t| group(Some(t), Delimiter::None).is_some()) {
        flat = transparent(tokens);
        &flat
    } else { tokens };
    let mut at = 0;
    'items: while at < tokens.len() {
        if punct(tokens.get(at), ';') { at += 1; continue; }
        let mut attrs = attributes(tokens, &mut at, index);
        attrs.cfg = combine_cfg(inherited, attrs.cfg);
        if at == tokens.len() { break; }
        let vis = visibility(tokens, &mut at);
        // Item modifiers, including foreign ABI strings. `const` followed by
        // an identifier is a declaration, whereas `const unsafe fn` is not.
        loop {
            let modifier = ["unsafe", "async", "default", "auto", "safe"].iter()
                .any(|m| word(tokens.get(at), m));
            if modifier || word(tokens.get(at), "const")
                && ["fn", "unsafe", "async", "extern"].iter().any(|m| word(tokens.get(at + 1), m)) {
                at += 1;
            } else if word(tokens.get(at), "extern") && !word(tokens.get(at + 1), "crate") {
                at += 1;
                if matches!(tokens.get(at), Some(TokenTree::Literal(_))) { at += 1; }
                if let Some(foreign) = group(tokens.get(at), Delimiter::Brace) {
                    index_items(&foreign.trees, index, order, &attrs.cfg);
                    at += 1;
                    continue 'items;
                }
            } else { break; }
        }
        if at >= tokens.len() { break; }
        let start = at;
        let keyword = ident(tokens.get(at));
        let ordinal = *order;
        *order += 1;
        if word(tokens.get(at), "impl") {
            let end = header_end(tokens, at + 1);
            index.impls.push(implementation(&tokens[at + 1..end], tokens[at].span(), attrs.cfg));
            at = (end + 1).min(tokens.len());
            continue;
        }
        if word(tokens.get(at), "use") {
            let end = semicolon(tokens, at + 1);
            use_tree(&tokens[at + 1..end], "", &vis, ordinal, &attrs.cfg, index);
            at = (end + 1).min(tokens.len());
            continue;
        }
        if word(tokens.get(at), "macro_rules") && punct(tokens.get(at + 1), '!') {
            at += 2;
            if let Some(TokenTree::Ident(name)) = tokens.get(at) {
                index.macros.push(IndexMacro { name: name.name.clone(), macro_export: attrs.macro_export,
                    order: ordinal, span: name.span(), cfg: attrs.cfg.clone() });
                index.items.push(IndexItem { name: name.name.clone(), kind: ItemKind::Macro,
                    vis, span: name.span(), cfg: attrs.cfg });
            }
            at = macro_end(tokens, at + 1);
            continue;
        }
        if word(tokens.get(at), "extern") && word(tokens.get(at + 1), "crate") {
            let end = semicolon(tokens, at + 2);
            let name = if word(tokens.get(at + 3), "as") { at + 4 } else { at + 2 };
            if let Some(TokenTree::Ident(name)) = tokens.get(name) {
                index.items.push(IndexItem { name: name.name.clone(), kind: ItemKind::Mod,
                    vis, span: name.span(), cfg: attrs.cfg });
            }
            at = (end + 1).min(tokens.len());
            continue;
        }
        let kind = match keyword {
            "struct" => Some(ItemKind::Struct), "enum" => Some(ItemKind::Enum),
            "union" => Some(ItemKind::Union), "trait" => Some(ItemKind::Trait),
            "fn" => Some(ItemKind::Fn), "const" => Some(ItemKind::Const),
            "static" => Some(ItemKind::Static), "type" => Some(ItemKind::Type),
            "mod" => Some(ItemKind::Mod), "macro" => Some(ItemKind::Macro),
            _ => None,
        }.filter(|_| !matches!(tokens.get(at), Some(TokenTree::Ident(name)) if name.raw));
        if let Some(kind) = kind {
            // Impl blocks in anonymous const initializers are globally observable
            // (derive macros use this scope to avoid import-resolution cycles).
            // Block-local names themselves are not module declarations.
            if matches!(kind, ItemKind::Const | ItemKind::Static) {
                let end = semicolon(tokens, at + 1);
                for token in &tokens[at + 1..end] {
                    if let Some(body) = group(Some(token), Delimiter::Brace) {
                        let mut nested = NameIndex::default();
                        index_items(&body.trees, &mut nested, &mut 0, &attrs.cfg);
                        let locals = nested.items.iter().filter(|item| matches!(item.kind,
                            ItemKind::Struct | ItemKind::Enum | ItemKind::Union | ItemKind::Trait | ItemKind::Type))
                            .map(|item| item.name.as_str()).collect::<Vec<_>>();
                        index.impls.extend(nested.impls.iter().filter(|head| {
                            let local = |path: &str| locals.contains(&path.split([':', '<']).next().unwrap_or(path));
                            !local(&head.self_path) && !head.trait_path.as_deref().is_some_and(local)
                        }).cloned().map(|mut head| {
                            // These imports belong to the const block, so they
                            // cannot be published as module bindings. Resolve
                            // them before lifting its observable impl heads.
                            let resolve = |path: &mut String| {
                                for import in &nested.uses {
                                    let name = import.alias.as_deref().unwrap_or_else(|| import.path.rsplit("::").next().unwrap_or(""));
                                    if path == name { *path = import.path.clone(); break; }
                                    if let Some(rest) = path.strip_prefix(&format!("{name}::")) {
                                        *path = format!("{}::{rest}", import.path); break;
                                    }
                                }
                            };
                            resolve(&mut head.self_path);
                            if let Some(path) = &mut head.trait_path { resolve(path); }
                            head
                        }));
                    }
                }
            }
            at += 1;
            if kind == ItemKind::Static && word(tokens.get(at), "mut") { at += 1; }
            if let Some(TokenTree::Ident(name)) = tokens.get(at) {
                index.items.push(IndexItem { name: name.name.clone(), kind, vis: vis.clone(),
                    span: name.span(), cfg: attrs.cfg.clone() });
                if kind == ItemKind::Mod {
                    let inline = group(tokens.get(at + 1), Delimiter::Brace)
                        .map(|body| Box::new(of_tokens(body)));
                    index.modules.push(IndexMod { name: name.name.clone(), path: attrs.path, inline,
                        vis, order: ordinal, span: name.span(), cfg: attrs.cfg });
                }
            }
            at = match kind {
                ItemKind::Const | ItemKind::Static | ItemKind::Type => (semicolon(tokens, at + 1) + 1).min(tokens.len()),
                ItemKind::Macro => macro_end(tokens, at + 1),
                _ => (header_end(tokens, at + 1) + 1).min(tokens.len()),
            };
        } else {
            // A macro invocation can expand to anything. Its argument group
            // does not itself declare names in this module.
            at = macro_end(tokens, start);
        }
    }
}

fn semicolon(tokens: &[TokenTree], start: usize) -> usize {
    (start..tokens.len()).find(|&at| punct(tokens.get(at), ';')).unwrap_or(tokens.len())
}

fn header_end(tokens: &[TokenTree], start: usize) -> usize {
    let mut angles = 0usize;
    for at in start..tokens.len() {
        if punct(tokens.get(at), ';') { return at; }
        if punct(tokens.get(at), '<') { angles += 1; }
        if punct(tokens.get(at), '>') && !punct(at.checked_sub(1).and_then(|i| tokens.get(i)), '-') {
            angles = angles.saturating_sub(1);
        }
        if angles == 0 && group(tokens.get(at), Delimiter::Brace).is_some()
            && !punct(at.checked_sub(1).and_then(|i| tokens.get(i)), '!') { return at; }
    }
    tokens.len()
}

fn macro_end(tokens: &[TokenTree], start: usize) -> usize {
    for at in start..tokens.len() {
        if punct(tokens.get(at), ';') { return at + 1; }
        if let Some(TokenTree::Group(body)) = tokens.get(at) {
            // macro 2.0 has a parameter group followed by a body group.
            let next = if body.delimiter() == Delimiter::Parenthesis
                && group(tokens.get(at + 1), Delimiter::Brace).is_some() { at + 2 } else { at + 1 };
            return next + usize::from(punct(tokens.get(next), ';'));
        }
    }
    tokens.len()
}

fn use_tree(tokens: &[TokenTree], prefix: &str, vis: &Vis, order: usize, cfg: &Option<TokenStream>, index: &mut NameIndex) {
    let mut start = 0;
    for end in 0..=tokens.len() {
        if end != tokens.len() && !punct(tokens.get(end), ',') { continue; }
        let branch = &tokens[start..end];
        start = end + 1;
        if branch.is_empty() { continue; }
        let mut path = prefix.to_owned();
        let mut alias = None;
        let mut span = branch[0].span();
        let mut glob = false;
        let mut nested = false;
        let mut at = 0;
        while at < branch.len() {
            if let Some(body) = group(branch.get(at), Delimiter::Brace) {
                use_tree(&body.trees, &path, vis, order, cfg, index);
                nested = true;
                break;
            }
            if word(branch.get(at), "as") {
                if let Some(name) = branch.get(at + 1) {
                    alias = Some(ident(Some(name)).to_owned());
                    span = name.span();
                }
                break;
            }
            if word(branch.get(at), "self") && !path.is_empty() && at + 1 == branch.len()
                || word(branch.get(at), "self") && !path.is_empty() && word(branch.get(at + 1), "as") {
                path = path.trim_end_matches("::").to_owned();
            } else {
                path.push_str(&branch[at].to_string());
            }
            if !punct(branch.get(at), ':') { span = branch[at].span(); }
            glob |= punct(branch.get(at), '*');
            at += 1;
        }
        if nested { continue; }
        if *vis != Vis::Private {
            let name = alias.clone().unwrap_or_else(|| path.rsplit("::").next().unwrap_or("").trim_start_matches("r#").into());
            index.items.push(IndexItem { name, kind: ItemKind::UseReexport, vis: vis.clone(), span, cfg: cfg.clone() });
        }
        index.uses.push(IndexUse { path, alias, glob, vis: vis.clone(), order, span, cfg: cfg.clone() });
    }
}

fn implementation(tokens: &[TokenTree], span: TokSpan, cfg: Option<TokenStream>) -> IndexImpl {
    let mut start = 0;
    if punct(tokens.first(), '<') { start = angle_end(tokens, 0).min(tokens.len()); }
    let mut end = tokens.len();
    let mut split = None;
    let mut at = start;
    while at < end {
        if word(tokens.get(at), "where") { end = at; break; }
        if word(tokens.get(at), "for") { split = Some(at); }
        if punct(tokens.get(at), '<') { at = angle_end(tokens, at); }
        else { at += 1; }
    }
    let (trait_path, self_type) = if let Some(split) = split {
        (Some(erased_path(&tokens[start..split], false)), &tokens[split + 1..end])
    } else { (None, &tokens[start..end]) };
    IndexImpl { trait_path, self_path: erased_path(self_type, false), span, cfg }
}

fn angle_end(tokens: &[TokenTree], open: usize) -> usize {
    let mut depth = 0usize;
    for at in open..tokens.len() {
        if punct(tokens.get(at), '<') { depth += 1; }
        if punct(tokens.get(at), '>') && !punct(at.checked_sub(1).and_then(|i| tokens.get(i)), '-') {
            depth = depth.saturating_sub(1);
            if depth == 0 { return at + 1; }
        }
    }
    tokens.len()
}

fn erased_path(tokens: &[TokenTree], last: bool) -> String {
    if last {
        // Keep references/pointers when shortening a qualified self type.
        let mut prefix = 0;
        while punct(tokens.get(prefix), '&') || punct(tokens.get(prefix), '*') {
            prefix += 1;
            if punct(tokens.get(prefix), '\'') { prefix += 2; }
            if word(tokens.get(prefix), "mut") || word(tokens.get(prefix), "const") { prefix += 1; }
        }
        let mut segment = prefix;
        let mut at = prefix;
        while at < tokens.len() {
            if punct(tokens.get(at), '<') {
                at = angle_end(tokens, at);
            } else if punct(tokens.get(at), ':') && punct(tokens.get(at + 1), ':') {
                if !punct(tokens.get(at + 2), '<') { segment = at + 2; }
                at += 2;
            } else { at += 1; }
        }
        if segment > prefix {
            let mut result = erased_path(&tokens[..prefix], false);
            append_spelling(&mut result, &erased_path(&tokens[segment..], false));
            return result;
        }
    }
    let mut out = String::new();
    let mut at = 0;
    while at < tokens.len() {
        if punct(tokens.get(at), '<') {
            let end = angle_end(tokens, at);
            let mut count = 0;
            let mut value = false;
            let mut arg = at + 1;
            while arg + 1 < end {
                if punct(tokens.get(arg), ',') { count += usize::from(value); value = false; }
                else {
                    value = true;
                    if punct(tokens.get(arg), '<') { arg = angle_end(tokens, arg); continue; }
                }
                arg += 1;
            }
            count += usize::from(value);
            out.push('<');
            out.push_str(&vec!["_"; count].join(","));
            out.push('>');
            at = end;
        } else if let Some(TokenTree::Group(value)) = tokens.get(at) {
            match value.delimiter() {
                Delimiter::Parenthesis => {
                    let count = if value.stream.is_empty() { 0 } else {
                        value.stream.trees.iter().filter(|t| punct(Some(t), ',')).count()
                            + usize::from(!punct(value.stream.trees.last(), ','))
                    };
                    out.push('(');
                    out.push_str(&vec!["_"; count].join(","));
                    if count == 1 { out.push(','); }
                    out.push(')');
                }
                Delimiter::Bracket => out.push_str(if value.stream.trees.iter().any(|t| punct(Some(t), ';')) { "[_;_]" } else { "[_]" }),
                _ => out.push('_'),
            }
            at += 1;
        } else {
            append_spelling(&mut out, &tokens[at].to_string());
            at += 1;
        }
    }
    out
}

fn append_spelling(out: &mut String, text: &str) {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    if out.chars().last().is_some_and(word) && text.chars().next().is_some_and(word) {
        out.push(' ');
    }
    out.push_str(text);
}
