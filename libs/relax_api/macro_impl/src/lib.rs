//! Expansion of `#[relax_api]` (see relax-api for what the marks mean). The item is
//! passed through unchanged; under the using crate's `host-shims` feature the expansion adds,
//! next to it, the extern "C" wrappers and the `relax_api` link-section entries.

use makepad_micro_proc_macro::proc_macro::{Delimiter, Group, Spacing, TokenStream, TokenTree};
use makepad_micro_proc_macro::TokenBuilder;
use std::collections::BTreeMap;

/// The attribute: `attr` its arguments, `item` the marked item.
pub fn relax_api_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = idents(&attr);
    let toks: Vec<TokenTree> = item.into_iter().collect();
    let res = match item_kind(&toks) {
        Some("impl") => expand_impl(&toks, &args),
        Some("fn") => expand_fn(&toks, &args),
        Some("struct") | Some("enum") => expand_type(&toks, &args),
        Some("static") => expand_static(&toks, &attr),
        _ => Err("#[relax_api] goes on a fn, an impl block, a struct, an enum or a static".to_string()),
    };
    match res {
        Ok((item, extra)) => {
            let mut tb = TokenBuilder::new();
            tb.stream(Some(item));
            if !extra.is_empty() {
                // The generated mirrors copy user field types and discriminants. Parse
                // them as Rust tokens: TokenBuilder::add suffixes hex literals with u64.
                tb.stream(Some(extra.parse().expect("relax_api generated invalid tokens")));
            }
            tb.end()
        }
        Err(e) => {
            let mut tb = TokenBuilder::new();
            tb.add(&format!("compile_error!({:?});", format!("#[relax_api]: {e}")));
            tb.end()
        }
    }
}

fn idents(ts: &TokenStream) -> Vec<String> {
    ts.clone().into_iter().filter_map(|t| if let TokenTree::Ident(i) = t { Some(i.to_string()) } else { None }).collect()
}

fn is_ident(t: Option<&TokenTree>, s: &str) -> bool {
    matches!(t, Some(TokenTree::Ident(i)) if i.to_string() == s)
}

fn is_punct(t: Option<&TokenTree>, c: char) -> bool {
    matches!(t, Some(TokenTree::Punct(p)) if p.as_char() == c)
}

/// Skips outer attributes (`# [..]`) from `i`.
fn skip_attrs(toks: &[TokenTree], mut i: usize) -> usize {
    while is_punct(toks.get(i), '#') && matches!(toks.get(i + 1), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket) {
        i += 2;
    }
    i
}

fn item_kind(toks: &[TokenTree]) -> Option<&'static str> {
    let mut i = skip_attrs(toks, 0);
    loop {
        match toks.get(i) {
            Some(TokenTree::Ident(id)) => match id.to_string().as_str() {
                "impl" => return Some("impl"),
                "fn" => return Some("fn"),
                "struct" => return Some("struct"),
                "enum" => return Some("enum"),
                "static" => return Some("static"),
                _ => i += 1,
            },
            // pub(crate), extern "C" ..
            Some(_) => i += 1,
            None => return None,
        }
    }
}

/// Tokens as source text, lifetimes rewritten by `life` ('_ / 'static) or dropped (None).
fn text(toks: &[TokenTree], life: Option<&str>) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if let TokenTree::Punct(p) = t {
            if p.as_char() == '\'' {
                if let Some(TokenTree::Ident(name)) = toks.get(i + 1) {
                    // 'static stays (a fn pointer's elided output has no input to come from)
                    if name.to_string() == "static" {
                        out.push_str("'static ");
                    } else if let Some(l) = life {
                        out.push_str(l);
                        out.push(' ');
                    }
                    i += 2;
                    continue;
                }
            }
        }
        match t {
            TokenTree::Group(g) => {
                let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                let (o, c) = match g.delimiter() {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::Brace => ("{", "}"),
                    _ => ("", ""),
                };
                out.push_str(o);
                out.push_str(&text(&inner, life));
                out.push_str(c);
            }
            TokenTree::Punct(p) => {
                out.push(p.as_char());
                if p.spacing() == Spacing::Alone {
                    out.push(' ');
                }
                i += 1;
                continue;
            }
            other => out.push_str(&other.to_string()),
        }
        out.push(' ');
        i += 1;
    }
    out
}

/// Tokens as source text with every lifetime, named or elided, 'static: a fn pointer type
/// that names no lifetime (an elided output with two input lifetimes is no type), for the
/// signature text only (Relax compares it with lifetimes stripped).
fn text_static(toks: &[TokenTree]) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            TokenTree::Punct(p) if p.as_char() == '&' && !is_punct(toks.get(i + 1), '\'') => {
                out.extend(make("& 'static"));
            }
            TokenTree::Group(g) => {
                let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                let s = make(&text_static(&inner));
                let mut ng = Group::new(g.delimiter(), s.into_iter().collect());
                ng.set_span(g.span());
                out.push(TokenTree::Group(ng));
            }
            other => out.push(other.clone()),
        }
        i += 1;
    }
    text(&out, Some("'static"))
}

/// `Self` replaced by `with` (the impl's self type).
fn subst_self(toks: &[TokenTree], with: &[TokenTree]) -> Vec<TokenTree> {
    let mut out = Vec::new();
    for t in toks {
        match t {
            TokenTree::Ident(i) if i.to_string() == "Self" => out.extend(with.iter().cloned()),
            TokenTree::Group(g) => {
                let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                let s = subst_self(&inner, with);
                let mut ng = Group::new(g.delimiter(), s.into_iter().collect());
                ng.set_span(g.span());
                out.push(TokenTree::Group(ng));
            }
            other => out.push(other.clone()),
        }
    }
    out
}

fn split_commas(toks: &[TokenTree]) -> Vec<Vec<TokenTree>> {
    let mut out = vec![Vec::new()];
    let mut depth = 0i32;
    let mut prev_dash = false;
    for t in toks {
        if let TokenTree::Punct(p) = t {
            let dash = prev_dash;
            prev_dash = p.as_char() == '-';
            match p.as_char() {
                '<' => depth += 1,
                '>' if !dash => depth -= 1,
                ',' if depth == 0 => {
                    out.push(Vec::new());
                    continue;
                }
                _ => {}
            }
        } else {
            prev_dash = false;
        }
        out.last_mut().unwrap().push(t.clone());
    }
    if out.last().is_some_and(|v| v.is_empty()) {
        out.pop();
    }
    out
}

fn fnv(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// One parsed fn: what the wrapper and the entry need.
struct Sig {
    name: String,
    is_unsafe: bool,
    /// parameter types (receiver as `&Self` / `&mut Self` / `Self`), Self not yet substituted
    params: Vec<Vec<TokenTree>>,
    ret: Vec<TokenTree>,
    never: bool,
    generic: bool,
    /// the fn's own `#[relax_api]` mark (inside an impl)
    marked: bool,
    cfg: Vec<String>,
}

/// Parses a fn item from `toks[i..]` (attributes included); returns the sig and the index
/// after the fn, plus the token range of its own `#[relax_api]` attribute(s) to strip.
fn parse_fn(toks: &[TokenTree], start: usize) -> Option<(Sig, usize, Vec<(usize, usize)>)> {
    let mut strip = Vec::new();
    let mut marked = false;
    let mut i = start;
    while is_punct(toks.get(i), '#') {
        if let Some(TokenTree::Group(g)) = toks.get(i + 1) {
            let inner: Vec<TokenTree> = g.stream().into_iter().collect();
            if is_ident(inner.first(), "relax_api") {
                marked = true;
                strip.push((i, i + 2));
            }
        }
        i += 2;
    }
    let mut is_unsafe = false;
    loop {
        match toks.get(i) {
            Some(TokenTree::Ident(id)) if id.to_string() == "fn" => break,
            Some(TokenTree::Ident(id)) => {
                if id.to_string() == "unsafe" {
                    is_unsafe = true;
                }
                i += 1;
            }
            Some(TokenTree::Punct(p)) if p.as_char() == '#' => {
                if let Some(TokenTree::Group(g)) = toks.get(i + 1) {
                    let inner: Vec<TokenTree> = g.stream().into_iter().collect();
                    if is_ident(inner.first(), "relax_api") {
                        marked = true;
                        strip.push((i, i + 2));
                    }
                }
                i += 2;
            }
            Some(TokenTree::Group(_)) | Some(TokenTree::Literal(_)) | Some(TokenTree::Punct(_)) => i += 1,
            None => return None,
        }
    }
    i += 1;
    let name = match toks.get(i) {
        Some(TokenTree::Ident(id)) => id.to_string(),
        _ => return None,
    };
    i += 1;
    let mut generic = false;
    if is_punct(toks.get(i), '<') {
        // a lifetime-only list is no generic op
        let mut depth = 0;
        let mut only_lifetimes = true;
        let mut prev_quote = false;
        while i < toks.len() {
            match &toks[i] {
                TokenTree::Punct(p) if p.as_char() == '<' => depth += 1,
                TokenTree::Punct(p) if p.as_char() == '>' && !is_punct(toks.get(i.wrapping_sub(1)), '-') => {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                TokenTree::Punct(p) if p.as_char() == '\'' => {
                    prev_quote = true;
                    i += 1;
                    continue;
                }
                TokenTree::Ident(_) if !prev_quote => only_lifetimes = false,
                _ => {}
            }
            prev_quote = false;
            i += 1;
        }
        generic = !only_lifetimes;
    }
    let params_group = match toks.get(i) {
        Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis => g.clone(),
        _ => return None,
    };
    i += 1;
    let mut ret = Vec::new();
    let mut never = false;
    if is_punct(toks.get(i), '-') && is_punct(toks.get(i + 1), '>') {
        i += 2;
        while i < toks.len() {
            match &toks[i] {
                TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => break,
                TokenTree::Ident(id) if id.to_string() == "where" => break,
                TokenTree::Punct(p) if p.as_char() == ';' => break,
                t => ret.push(t.clone()),
            }
            i += 1;
        }
        never = ret.len() == 1 && is_punct(ret.first(), '!');
    }
    // where clause / body / `;`
    while i < toks.len() {
        match &toks[i] {
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => {
                i += 1;
                break;
            }
            TokenTree::Punct(p) if p.as_char() == ';' => {
                i += 1;
                break;
            }
            TokenTree::Ident(id) if id.to_string() == "where" => generic = true,
            _ => {}
        }
        i += 1;
    }
    let mut params = Vec::new();
    for p in split_commas(&params_group.stream().into_iter().collect::<Vec<_>>()) {
        // receivers: self, mut self, &self, &'a self, &mut self, self: T
        let names_self = p.iter().any(|t| is_ident(Some(t), "self"));
        let colon = p.iter().position(|t| is_punct(Some(t), ':') && true);
        if names_self && colon.is_none() {
            let r: Vec<TokenTree> = p.iter().take_while(|t| !is_ident(Some(t), "self")).cloned().filter(|t| !is_ident(Some(t), "mut") || false).collect();
            // `&mut self` keeps its mut; `mut self` is a by-value Self
            let is_ref = r.iter().any(|t| is_punct(Some(t), '&'));
            let mut ty = Vec::new();
            if is_ref {
                ty.extend(r.iter().cloned());
                if p.iter().any(|t| is_ident(Some(t), "mut")) {
                    ty.extend(make("mut"));
                }
            }
            ty.extend(make("Self"));
            params.push(ty);
            continue;
        }
        let Some(c) = first_colon(&p) else { return None };
        params.push(p[c + 1..].to_vec());
    }
    let sig = Sig { name, is_unsafe, params, ret, never, generic, marked, cfg: layout_conditions(&toks[start..]) };
    Some((sig, i, strip))
}

/// The first `:` that is not part of `::`.
fn first_colon(p: &[TokenTree]) -> Option<usize> {
    let mut i = 0;
    while i < p.len() {
        if is_punct(p.get(i), ':') {
            if is_punct(p.get(i + 1), ':') {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

fn make(s: &str) -> Vec<TokenTree> {
    let mut tb = TokenBuilder::new();
    tb.add(s);
    tb.end().into_iter().collect()
}

/// Where the op lives, for its call path.
struct Owner {
    /// self type tokens (impl), or empty for a free fn
    self_ty: Vec<TokenTree>,
    /// trait tokens of a trait impl
    trait_: Option<Vec<TokenTree>>,
}

fn call_path(o: &Owner, name: &str) -> String {
    if o.self_ty.is_empty() {
        return name.to_string();
    }
    match &o.trait_ {
        Some(t) => format!("<{} as {}>::{}", text(&o.self_ty, Some("'_")), text(t, Some("'_")), name),
        None => format!("<{}>::{}", text(&o.self_ty, Some("'_")), name),
    }
}

/// The `key` fn of an op's entry. rustc's `type_name_of_val` of a fn item prints a trait impl's
/// method as `<T as Trait>::m` and a free fn by its path, which Relax prints the same way; an
/// inherent method it prints by where the impl sits (`m::<impl T>::f`, or `T<'_>::f` when the
/// impl is in T's module), so that one is `<T>::f` with T as `type_name` prints it.
fn key_code(o: &Owner, name: &str) -> String {
    if !o.self_ty.is_empty() && o.trait_.is_none() {
        let ty = text(&o.self_ty, Some("'_"));
        return format!("{{ fn k() -> &'static str {{ static K: ::std::sync::OnceLock<String> = ::std::sync::OnceLock::new(); K.get_or_init(|| format!(\"<{{}}>::{name}\", ::core::any::type_name::<{ty}>())) }} k }}");
    }
    format!("{{ fn k() -> &'static str {{ ::core::any::type_name_of_val(&{}) }} k }}", call_path(o, name))
}

const CFG: &str = "#[cfg(feature = \"host-shims\")]";
const SECTION: &str = "#[used] #[cfg_attr(target_vendor = \"apple\", link_section = \"__DATA,__relax_api\")] #[cfg_attr(all(not(target_vendor = \"apple\"), not(windows)), link_section = \"relax_api\")] #[cfg_attr(windows, link_section = \".relaxapi$m\")]";

/// Payload spelling -> the cfg predicates of each occurrence in this marked item.
type OptionSpellings = BTreeMap<String, Vec<String>>;

/// Explicit payload spellings use the attribute's const oracle and entry emitter too.
/// Per-payload cfg attributes let a crate keep its platform-specific types in one list.
pub fn option_layouts_impl(input: TokenStream) -> TokenStream {
    let toks: Vec<_> = input.into_iter().collect();
    let mut options = OptionSpellings::new();
    for payload in split_commas(&toks) {
        let cfg = layout_conditions(&payload);
        let ty = &payload[skip_attrs(&payload, 0)..];
        if ty.is_empty() || !option_nameable(ty) {
            return "compile_error!(\"option_layouts! expects concrete payload types\");".parse().unwrap();
        }
        options.entry(option_static(ty)).or_default().push(format!("all({})", cfg.join(", ")));
    }
    // An anonymous scope also permits repeated lists in the same module.
    let extra = option_code(fnv(&text(&toks, None)), &options, true);
    let code = format!("const _: () = {{ {extra} }};");
    code.parse().unwrap_or_else(|e| panic!("option_layouts generated invalid tokens ({e:?}): {code}"))
}

fn option_nameable(toks: &[TokenTree]) -> bool {
    toks.iter().enumerate().all(|(i, t)| match t {
        TokenTree::Ident(id) => is_punct(i.checked_sub(1).and_then(|n| toks.get(n)), '\'')
            || !matches!(id.to_string().as_str(), "Self" | "impl" | "_"),
        TokenTree::Group(g) => option_nameable(&g.stream().into_iter().collect::<Vec<_>>()),
        _ => true,
    })
}

/// Instantiate higher-ranked lifetimes too: rewriting `for<'a>` as
/// `for<'static>` would declare a reserved lifetime instead of naming a type.
fn option_static(toks: &[TokenTree]) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if is_ident(toks.get(i), "for") && is_punct(toks.get(i + 1), '<') {
            i += 2;
            while i < toks.len() && !is_punct(toks.get(i), '>') { i += 1; }
        } else if let TokenTree::Group(g) = &toks[i] {
            let inner = option_static(&g.stream().into_iter().collect::<Vec<_>>());
            let mut group = Group::new(g.delimiter(), inner.parse().unwrap_or_else(|e| panic!("Option type tokens ({e:?}): {inner}")));
            group.set_span(g.span());
            out.push(TokenTree::Group(group));
        } else {
            out.push(toks[i].clone());
        }
        i += 1;
    }
    text_static(&out)
}

/// Scan type tokens, including nested generic arguments, tuples, arrays and fn types.
/// Generic ops/types have already been excluded by the item expander. Keep Self
/// unsubstituted here: an associated type cannot be named outside its impl.
fn collect_options(toks: &[TokenTree], cfg: &[String], out: &mut OptionSpellings) {
    for (i, t) in toks.iter().enumerate() {
        if let TokenTree::Group(g) = t {
            collect_options(&g.stream().into_iter().collect::<Vec<_>>(), cfg, out);
        }
        if !is_ident(Some(t), "Option") || !is_punct(toks.get(i + 1), '<') { continue; }
        let mut depth = 1usize;
        for end in i + 2..toks.len() {
            if is_punct(toks.get(end), '<') {
                depth += 1;
            } else if is_punct(toks.get(end), '>') && !is_punct(toks.get(end - 1), '-') {
                depth -= 1;
                if depth == 0 {
                    let payload = &toks[i + 2..end];
                    if !payload.is_empty() && option_nameable(payload) {
                        let spelling = option_static(payload);
                        let condition = format!("all({})", cfg.join(", "));
                        let conditions = out.entry(spelling).or_default();
                        if !conditions.contains(&condition) { conditions.push(condition); }
                    }
                    break;
                }
            }
        }
    }
}

fn option_code(id: u64, options: &OptionSpellings, explicit: bool) -> String {
    let mut extra = String::new();
    for (n, (payload, conditions)) in options.iter().enumerate() {
        extra.push_str(&format!(
            "\n{CFG} #[cfg(any({conditions}))] {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)]
static __relax_api_o{id:016x}_{n}: ::relax_api::Entry = ::relax_api::Entry {{
  kind: ::relax_api::OPTION, module: module_path!(),
  key: {{ fn k() -> &'static str {{ ::core::any::type_name::<::core::option::Option<{payload}>>() }} k }},
  sig: {{ fn s() -> &'static str {{ \"{origin}\" }} s }},
  shim: None, slots: &[], size: 0, align: 0, offsets: &[],
  enum_layout: {{ const TABLE: [u64; 6] = ::relax_api::option_layout::<{payload}>(); &TABLE }},
  drop: None, addr: None }};
#[cfg(any({conditions}))] #[doc(hidden)] #[allow(non_upper_case_globals, dead_code)]
const __relax_api_l{id:016x}_{n}: ::relax_api::OptionListed<::core::option::Option<{payload}>> = ::relax_api::OptionListed(::core::marker::PhantomData);\n",
            conditions = conditions.join(", "),
            origin = if explicit { "option_layouts" } else { "" },
        ));
    }
    extra
}

fn collect_field_options(fields: &Group, cfg: &[String], out: &mut OptionSpellings) {
    for field in split_commas(&fields.stream().into_iter().collect::<Vec<_>>()) {
        let mut conditions = cfg.to_vec();
        conditions.extend(layout_conditions(&field));
        let mut at = skip_attrs(&field, 0);
        if fields.delimiter() == Delimiter::Brace {
            let Some(colon) = first_colon(&field[at..]) else { continue; };
            at += colon + 1;
        } else if is_ident(field.get(at), "pub") {
            at += 1;
            if matches!(field.get(at), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis) { at += 1; }
        }
        collect_options(&field[at..], &conditions, out);
    }
}

fn type_option_code(toks: &[TokenTree], body_start: usize, is_struct: bool, id: u64) -> String {
    let mut options = OptionSpellings::new();
    let cfg = layout_conditions(toks);
    if let Some(TokenTree::Group(g)) = toks[body_start..].iter().find(|t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace || g.delimiter() == Delimiter::Parenthesis)) {
        if is_struct {
            collect_field_options(g, &cfg, &mut options);
        } else {
            for variant in enum_variants(&g.stream().into_iter().collect::<Vec<_>>()) {
                let mut conditions = cfg.clone();
                conditions.extend(layout_conditions(&variant));
                if let Some(TokenTree::Group(fields)) = variant.get(skip_attrs(&variant, 0) + 1) {
                    if matches!(fields.delimiter(), Delimiter::Brace | Delimiter::Parenthesis) {
                        collect_field_options(fields, &conditions, &mut options);
                    }
                }
            }
        }
    }
    option_code(id, &options, false)
}

/// The wrapper's first statement: whose heap the op allocates in. `#[relax_api(run)]`: the
/// program's, so its end takes what the op made, whatever state it died in (a crate whose objects
/// are the program's, such as a script VM, says `run` on every attribute). Without `run`: the
/// host's, for a crate that holds what outlives a program or keeps nothing of one and frees what
/// a program held at its own end.
fn op_phase(args: &[String]) -> &'static str {
    if args.iter().any(|a| a == "run") {
        "let _phase = ::relax_api::run_op();"
    } else {
        "let _phase = ::relax_api::host_op();"
    }
}

/// The wrapper and the entry of one op.
fn op_code(o: &Owner, s: &Sig, phase: &str) -> String {
    let path = call_path(o, &s.name);
    let id = fnv(&format!("{} {} {}", text(&o.self_ty, None), o.trait_.as_ref().map_or(String::new(), |t| text(t, None)), s.name));
    let mut options = OptionSpellings::new();
    for ty in s.params.iter().chain(std::iter::once(&s.ret)) {
        collect_options(ty, &s.cfg, &mut options);
    }
    let w = format!("__relax_api_w{:016x}", id);
    let params: Vec<Vec<TokenTree>> = s.params.iter().map(|p| subst_self(p, &o.self_ty)).collect();
    let ret = subst_self(&s.ret, &o.self_ty);
    let args: Vec<String> = (0..params.len()).map(|k| format!("::core::ptr::read(*a.add({k}) as *const _)")).collect();
    let call = format!("{}({})", path, args.join(", "));
    let body = if s.never {
        format!("match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| -> () {{ unsafe {{ {call} }} }})) {{ Ok(()) => 0, Err(p) => ::relax_api::caught(p) }}")
    } else {
        format!("match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| unsafe {{ {call} }})) {{ Ok(v) => {{ ::core::ptr::write(r as *mut _, v); 0 }} Err(p) => ::relax_api::caught(p) }}")
    };
    let slot = |t: &[TokenTree]| -> String {
        let ty = text(t, Some("'static"));
        // a reference to a sized referent: its layout too
        let pointee = if is_punct(t.first(), '&') {
            let mut k = 1;
            if matches!(t.get(k), Some(TokenTree::Punct(p)) if p.as_char() == '\'') {
                k += 2;
            }
            if is_ident(t.get(k), "mut") {
                k += 1;
            }
            let r = &t[k..];
            let unsized_ = is_ident(r.first(), "str") || is_ident(r.first(), "dyn") || matches!(r.first(), Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Bracket && !g.stream().into_iter().any(|x| is_punct(Some(&x), ';')));
            if unsized_ || r.is_empty() {
                None
            } else {
                Some(text(r, Some("'static")))
            }
        } else {
            None
        };
        let p = match pointee {
            Some(r) => format!("::core::mem::size_of::<{r}>() as u64, ::core::mem::align_of::<{r}>() as u64"),
            None => "0, 0".to_string(),
        };
        format!("[::core::mem::size_of::<{ty}>() as u64, ::core::mem::align_of::<{ty}>() as u64, {p}]")
    };
    let mut slots: Vec<String> = params.iter().map(|p| slot(p)).collect();
    slots.push(if s.never {
        "[0, 1, 0, 0]".to_string()
    } else if ret.is_empty() {
        "[0, 1, 0, 0]".to_string()
    } else {
        slot(&ret)
    });
    let fnptr_params: Vec<String> = params.iter().map(|p| text_static(p)).collect();
    let fnptr = format!(
        "{}fn({}){}",
        if s.is_unsafe { "unsafe " } else { "" },
        fnptr_params.join(", "),
        if ret.is_empty() { String::new() } else { format!(" -> {}", text_static(&ret)) }
    );
    let mut extra = format!(
        "{CFG} #[doc(hidden)] #[allow(non_snake_case, unused_unsafe, clippy::all)] unsafe extern \"C\" fn {w}(a: *const *mut u8, r: *mut u8) -> u32 {{ let _ = r; {phase} {body} }}
{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static {w}_E: ::relax_api::Entry = ::relax_api::Entry {{
  kind: ::relax_api::OP, module: module_path!(),
  key: {key},
  sig: {{ fn s() -> &'static str {{ ::core::any::type_name::<{fnptr}>() }} s }},
  shim: Some({w}), slots: &[{slots}], size: 0, align: 0, offsets: &[], enum_layout: &[], drop: None, addr: None }};",
        key = key_code(o, &s.name),
        slots = slots.join(", "),
    );
    extra.push_str(&option_code(id, &options, false));
    extra
}

fn expand_fn(toks: &[TokenTree], args: &[String]) -> Result<(TokenStream, String), String> {
    let (s, _, _) = parse_fn(toks, 0).ok_or("cannot read the fn")?;
    if s.generic {
        return Err(format!("`{}` is generic: an op has one compiled body", s.name));
    }
    if s.params.iter().any(|p| p.iter().any(|t| is_ident(Some(t), "Self"))) {
        return Err(format!("`{}` is a method: put #[relax_api] on its impl block (and on the method inside it)", s.name));
    }
    let o = Owner { self_ty: Vec::new(), trait_: None };
    let extra = op_code(&o, &s, op_phase(args));
    Ok((toks.iter().cloned().collect(), extra))
}

fn expand_impl(toks: &[TokenTree], args: &[String]) -> Result<(TokenStream, String), String> {
    // impl [<generics>] [!]Trait for SelfTy [where ..] { body }
    let mut i = skip_attrs(toks, 0);
    while !is_ident(toks.get(i), "impl") {
        i += 1;
    }
    let impl_at = i;
    i += 1;
    if is_punct(toks.get(i), '<') {
        let mut depth = 0;
        let mut only_lifetimes = true;
        let mut prev_quote = false;
        while i < toks.len() {
            match &toks[i] {
                TokenTree::Punct(p) if p.as_char() == '<' => depth += 1,
                TokenTree::Punct(p) if p.as_char() == '>' && !is_punct(toks.get(i.wrapping_sub(1)), '-') => {
                    depth -= 1;
                    if depth == 0 {
                        i += 1;
                        break;
                    }
                }
                TokenTree::Punct(p) if p.as_char() == '\'' => {
                    prev_quote = true;
                    i += 1;
                    continue;
                }
                TokenTree::Ident(_) if !prev_quote => only_lifetimes = false,
                _ => {}
            }
            prev_quote = false;
            i += 1;
        }
        if !only_lifetimes {
            return Err("a generic impl has no one compiled body per op".to_string());
        }
    }
    let head_start = i;
    let body_at = toks.iter().position(|t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)).ok_or("impl without a body")?;
    let mut head: Vec<TokenTree> = toks[head_start..body_at].to_vec();
    if let Some(w) = head.iter().position(|t| is_ident(Some(t), "where")) {
        head.truncate(w);
    }
    let for_at = {
        let mut depth = 0i32;
        let mut at = None;
        for (k, t) in head.iter().enumerate() {
            match t {
                TokenTree::Punct(p) if p.as_char() == '<' => depth += 1,
                TokenTree::Punct(p) if p.as_char() == '>' && !(k > 0 && is_punct(head.get(k - 1), '-')) => depth -= 1,
                TokenTree::Ident(id) if depth == 0 && id.to_string() == "for" => at = Some(k),
                _ => {}
            }
        }
        at
    };
    let owner = match for_at {
        Some(k) => Owner { self_ty: head[k + 1..].to_vec(), trait_: Some(head[..k].to_vec()) },
        None => Owner { self_ty: head.clone(), trait_: None },
    };
    let _ = impl_at;
    let body = match &toks[body_at] {
        TokenTree::Group(g) => g.clone(),
        _ => unreachable!(),
    };
    // a macro's `$f:item` fragments arrive as invisible groups: their items are the impl's own
    let mut inner: Vec<TokenTree> = Vec::new();
    for t in body.stream() {
        match t {
            TokenTree::Group(g) if g.delimiter() == Delimiter::None => inner.extend(g.stream()),
            t => inner.push(t),
        }
    }
    // the fns, the marked ones
    let mut fns = Vec::new();
    let mut strip: Vec<(usize, usize)> = Vec::new();
    let mut k = 0;
    while k < inner.len() {
        let item_start = k;
        // find the next item's kind
        let mut j = skip_attrs(&inner, k);
        let mut kind = None;
        while j < inner.len() {
            match &inner[j] {
                // `const fn` / `const unsafe fn` are fns
                TokenTree::Ident(id) if id.to_string() == "const" && matches!(inner.get(j + 1), Some(TokenTree::Ident(n)) if matches!(n.to_string().as_str(), "fn" | "unsafe" | "extern" | "async")) => j += 1,
                TokenTree::Ident(id) if matches!(id.to_string().as_str(), "fn" | "type" | "const") => {
                    kind = Some(id.to_string());
                    break;
                }
                _ => j += 1,
            }
        }
        match kind.as_deref() {
            Some("fn") => {
                let (s, next, st) = parse_fn(&inner, item_start).ok_or_else(|| format!("cannot read a fn of the impl near `{}`", text(&inner[item_start..(item_start + 12).min(inner.len())], None)))?;
                strip.extend(st);
                fns.push(s);
                k = next;
            }
            Some(_) => {
                // type / const items: up to the next `;`
                while k < inner.len() && !is_punct(inner.get(k), ';') {
                    k += 1;
                }
                k += 1;
            }
            None => break,
        }
    }
    // `#[relax_api(marked)]`: only the fns marked inside, none when none is (a macro's impl,
    // whose fns are marked per invocation)
    let any_marked = args.iter().any(|a| a == "marked") || fns.iter().any(|f| f.marked);
    let mut extra = String::new();
    for s in &fns {
        if any_marked && !s.marked {
            continue;
        }
        if s.generic {
            if any_marked {
                return Err(format!("`{}` is generic: an op has one compiled body", s.name));
            }
            continue;
        }
        extra.push_str(&op_code(&owner, s, op_phase(args)));
        extra.push('\n');
    }
    // the impl, its fns' own marks stripped
    let kept: Vec<TokenTree> = inner.iter().enumerate().filter(|(n, _)| !strip.iter().any(|(a, b)| n >= a && n < b)).map(|(_, t)| t.clone()).collect();
    let mut ng = Group::new(Delimiter::Brace, kept.into_iter().collect());
    ng.set_span(body.span());
    let mut out: Vec<TokenTree> = toks[..body_at].to_vec();
    out.push(TokenTree::Group(ng));
    out.extend(toks[body_at + 1..].iter().cloned());
    Ok((out.into_iter().collect(), extra))
}

/// `#[relax_api] static [mut] NAME: T = ..;`: the loader's one instance, with its def path,
/// address and rustc layout. `#[relax_api(per_side)] static NAME: T = ..;`: an entry naming
/// the static so Relax gives JIT code its own instance instead of refusing the read.
fn expand_static(toks: &[TokenTree], attr: &TokenStream) -> Result<(TokenStream, String), String> {
    let mut args: Vec<TokenTree> = attr.clone().into_iter().collect();
    let shared = args.is_empty();
    if is_punct(args.last(), ',') {
        args.pop();
    }
    let per_side = args.len() == 1 && is_ident(args.first(), "per_side");
    let mut i = skip_attrs(toks, 0);
    while !is_ident(toks.get(i), "static") {
        i += 1;
    }
    i += 1;
    let mutable = is_ident(toks.get(i), "mut");
    if mutable {
        i += 1;
    }
    let name = match toks.get(i) {
        Some(TokenTree::Ident(id)) => id.to_string(),
        _ => return Err("a static without a name".to_string()),
    };
    if !shared && !per_side {
        return Err(format!("`{name}`: a static is marked #[relax_api] (one instance, the loader's) or #[relax_api(per_side)] (each side its own)"));
    }
    let id = fnv(&format!("static {name}"));
    let none = "{ fn s() -> &'static str { \"\" } s }";
    if per_side {
        if mutable {
            return Err("a `static mut` is not marked: use a static with interior mutability".to_string());
        }
        let key = format!("{{ fn k() -> &'static str {{ concat!(module_path!(), \"::{name}\") }} k }}");
        let extra = format!(
            "{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_s{id:016x}: ::relax_api::Entry = ::relax_api::Entry {{
  kind: ::relax_api::PER_SIDE, module: module_path!(), key: {key}, sig: {none},
  shim: None, slots: &[], size: 0, align: 0, offsets: &[], enum_layout: &[], drop: None, addr: None }};"
        );
        return Ok((toks.iter().cloned().collect(), extra));
    }
    if !is_punct(toks.get(i + 1), ':') {
        return Err(format!("`{name}`: cannot read the static's type"));
    }
    let start = i + 2;
    let mut end = start;
    let mut depth = 0i32;
    while end < toks.len() {
        if is_punct(toks.get(end), '=') && depth == 0 {
            break;
        }
        if is_punct(toks.get(end), '<') {
            depth += 1;
        } else if is_punct(toks.get(end), '>') && !is_punct(toks.get(end - 1), '-') {
            depth -= 1;
        }
        end += 1;
    }
    if end == start || end == toks.len() {
        return Err(format!("`{name}`: cannot read the static's type and initializer"));
    }
    let ty = toks[start..end].iter().cloned().collect::<TokenStream>().to_string();
    let key = format!(
        "{{ fn k() -> &'static str {{
    static K: ::std::sync::OnceLock<::std::string::String> = ::std::sync::OnceLock::new();
    K.get_or_init(|| {{
        let marker = ::core::any::type_name_of_val(&__relax_api_m{id:016x});
        let scope = marker.rsplit_once(\"::\").expect(\"a static marker's def path\").0;
        let mut key = ::std::string::String::from(scope);
        key.push_str(\"::{name}\");
        key
    }}).as_str()
}} k }}"
    );
    let extra = format!(
        "{CFG} #[doc(hidden)] #[allow(non_snake_case)] fn __relax_api_m{id:016x}() {{}}
{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_s{id:016x}: ::relax_api::Entry = ::relax_api::Entry {{
  kind: ::relax_api::SHARED, module: module_path!(), key: {key}, sig: {none},
  shim: None, slots: &[], size: ::core::mem::size_of::<{ty}>() as u64, align: ::core::mem::align_of::<{ty}>() as u64,
  offsets: &[], enum_layout: &[], drop: None, addr: Some({{ fn a() -> usize {{ (&raw const {name}) as usize }} a }}) }};"
    );
    Ok((toks.iter().cloned().collect(), extra))
}

/// Unlike `text`, keep named lifetimes and discriminant expressions as written.
fn source(toks: &[TokenTree]) -> String {
    toks.iter().cloned().collect::<TokenStream>().to_string()
}

/// The conditions under which an attribute leaves its item present. Normalise cfg_attr's
/// cfg effects too, without copying unrelated derives/docs onto the layout mirrors.
fn cfg_conditions(attr: &[TokenTree]) -> Vec<String> {
    let Some(TokenTree::Group(g)) = attr.get(1) else { return Vec::new(); };
    let inner: Vec<_> = g.stream().into_iter().collect();
    if is_ident(attr.first(), "cfg") {
        vec![source(&inner)]
    } else if is_ident(attr.first(), "cfg_attr") {
        let parts = split_commas(&inner);
        let Some(condition) = parts.first() else { return Vec::new(); };
        parts[1..].iter().flat_map(|a| cfg_conditions(a)).map(|c| format!("any(not({}), {c})", source(condition))).collect()
    } else {
        Vec::new()
    }
}

fn layout_conditions(toks: &[TokenTree]) -> Vec<String> {
    let mut conditions = Vec::new();
    for attr in toks[..skip_attrs(toks, 0)].chunks_exact(2) {
        if let TokenTree::Group(g) = &attr[1] {
            conditions.extend(cfg_conditions(&g.stream().into_iter().collect::<Vec<_>>()));
        }
    }
    conditions
}

fn layout_cfg(toks: &[TokenTree]) -> (String, String) {
    let conditions = layout_conditions(toks);
    if conditions.is_empty() {
        (String::new(), String::new())
    } else {
        let condition = format!("all({})", conditions.join(", "));
        (format!("#[cfg({condition})]"), format!("#[cfg(not({condition}))]"))
    }
}

/// Primitive repr uses RFC 2195's union of tag-prefixed structs. repr(C, int)
/// instead has a common payload offset (a tag followed by a payload union).
/// Explicit alignment belongs to the complete enum mirror, not to its tag/payloads.
fn enum_repr(toks: &[TokenTree]) -> Option<(String, bool, String)> {
    let mut tag = None;
    let mut c = false;
    let mut align = String::new();
    for attr in toks[..skip_attrs(toks, 0)].chunks_exact(2) {
        let TokenTree::Group(g) = &attr[1] else { continue; };
        let a: Vec<_> = g.stream().into_iter().collect();
        if !is_ident(a.first(), "repr") { continue; }
        let Some(TokenTree::Group(g)) = a.get(1) else { continue; };
        for part in split_commas(&g.stream().into_iter().collect::<Vec<_>>()) {
            if let Some(TokenTree::Ident(i)) = part.first() {
                match i.to_string().as_str() {
                    "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" => tag = Some(i.to_string()),
                    "C" => c = true,
                    "align" => align.push_str(&format!(", {}", source(&part))),
                    _ => {}
                }
            }
        }
    }
    tag.map(|t| (t, c, align))
}

/// Variant payloads are groups. Discriminants can contain comparisons/shifts as well
/// as turbofish/qualified paths, so `<` and `>` are not always generic delimiters here.
fn enum_variants(toks: &[TokenTree]) -> Vec<Vec<TokenTree>> {
    let mut parts = vec![Vec::new()];
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate() {
        if is_punct(Some(t), '<') && (depth > 0 || is_punct(i.checked_sub(1).and_then(|j| toks.get(j)), ':') || is_punct(i.checked_sub(1).and_then(|j| toks.get(j)), '=')) {
            depth += 1;
        } else if is_punct(Some(t), '>') && !is_punct(i.checked_sub(1).and_then(|j| toks.get(j)), '-') {
            depth = depth.saturating_sub(1);
        } else if is_punct(Some(t), ',') && depth == 0 {
            parts.push(Vec::new());
            continue;
        }
        parts.last_mut().unwrap().push(t.clone());
    }
    if parts.last().is_some_and(|p| p.is_empty()) { parts.pop(); }
    parts
}

fn expand_enum_layout(
    toks: &[TokenTree], name: &str, id: u64, generics: &str, lifetimes: &[String], body_start: usize,
) -> Result<(String, String), String> {
    let Some((tag, c_repr, align)) = enum_repr(toks) else { return Ok((String::new(), "&[]".to_string())); };
    let body = toks[body_start..].iter().position(|t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)).ok_or("cannot read enum body")? + body_start;
    let TokenTree::Group(g) = &toks[body] else { unreachable!(); };
    let where_clause = source(&toks[body_start..body]);
    let arguments = if lifetimes.is_empty() { String::new() } else { format!("<{}>", lifetimes.join(", ")) };
    let static_arguments = if lifetimes.is_empty() { String::new() } else { format!("<{}>", vec!["'static"; lifetimes.len()].join(", ")) };
    let ty = format!("{name}{static_arguments}");
    let self_ty = make(&format!("{name}{arguments}"));
    // Always keep all lifetimes used, including when cfg removes the only field using one.
    // This tail has size zero and alignment one and cannot change a repr(C) layout.
    let phantom = if lifetimes.is_empty() { String::new() } else {
        format!("::core::marker::PhantomData<({},)>,", lifetimes.iter().map(|l| format!("&{l} ()")).collect::<Vec<_>>().join(", "))
    };
    let prefix = format!("__relax_api_m{id:016x}");
    let mirror = format!("{prefix}_{}", if c_repr { "layout" } else { "union" });
    let tag_field = if c_repr { String::new() } else { format!("{tag},") };
    let payload_offset = if c_repr { format!("::core::mem::offset_of!({mirror}{static_arguments}, 1) as u64 + ") } else { String::new() };
    let attrs = format!("{CFG} #[doc(hidden)] #[allow(dead_code, non_camel_case_types, non_snake_case, non_upper_case_globals)]");
    let mut extra = String::new();
    let mut tags = String::new();
    let mut union_fields = String::new();
    let mut slices = String::new();
    for (n, v) in enum_variants(&g.stream().into_iter().collect::<Vec<_>>()).iter().enumerate() {
        let (cfg, _) = layout_cfg(v);
        let mut at = skip_attrs(v, 0);
        let Some(TokenTree::Ident(variant)) = v.get(at) else { return Err("cannot read enum variant".to_string()); };
        at += 1;
        let mut fields = String::new();
        let mut offsets = String::new();
        if let Some(TokenTree::Group(payload)) = v.get(at) {
            if matches!(payload.delimiter(), Delimiter::Parenthesis | Delimiter::Brace) {
                for (f, field) in split_commas(&payload.stream().into_iter().collect::<Vec<_>>()).iter().enumerate() {
                    let (field_cfg, absent_cfg) = layout_cfg(field);
                    let start = if payload.delimiter() == Delimiter::Brace {
                        first_colon(&field[skip_attrs(field, 0)..]).ok_or("cannot read enum field")? + skip_attrs(field, 0) + 1
                    } else { skip_attrs(field, 0) };
                    fields.push_str(&format!("{field_cfg} {},", source(&subst_self(&field[start..], &self_ty))));
                    // A disabled field is replaced by a zero-sized, align-one placeholder.
                    // Only its tuple index remains; its type and offset are cfg'd out.
                    // Consequently offset_of! uses literal indices even with cfg fields.
                    if !absent_cfg.is_empty() {
                        fields.push_str(&format!("{absent_cfg} ::core::marker::PhantomData<()>,"));
                    }
                    offsets.push_str(&format!("{field_cfg} {{ {payload_offset}::core::mem::offset_of!({prefix}_{n}{static_arguments}, {}) as u64 }},", f + usize::from(!c_repr)));
                }
                at += 1;
            }
        }
        // Copy the entire `= expr`, if present, and leave implicit discriminants implicit.
        if at < v.len() && !is_punct(v.get(at), '=') { return Err("cannot read enum discriminant".to_string()); }
        tags.push_str(&format!("{cfg} {variant} {},", source(&v[at..])));
        extra.push_str(&format!("{attrs} {cfg} #[repr(C)] struct {prefix}_{n}{generics}({tag_field} {fields} {phantom}) {where_clause};\n"));
        union_fields.push_str(&format!("{cfg} v{n}: ::core::mem::ManuallyDrop<{prefix}_{n}{arguments}>,"));
        let cast = if tag.starts_with('i') { "as i64 as u64" } else { "as u64" };
        slices.push_str(&format!("{cfg} {{ const OFFSETS: &[u64] = &[{offsets}]; &[{prefix}_tag::{variant} as {tag} {cast}, OFFSETS.len() as u64, {offsets}] }},"));
    }
    let union_align = if c_repr { "" } else { align.as_str() };
    extra.push_str(&format!("{attrs} #[repr({tag})] enum {prefix}_tag {{ {tags} }}\n\
        {attrs} #[repr(C{union_align})] union {prefix}_union{generics} {where_clause} {{ {union_fields} }}\n"));
    if c_repr {
        // RFC 2195 repr(C, int): payload structs have no tag and start at the same
        // offset in this wrapper, including a byte payload beside a wider variant.
        extra.push_str(&format!("{attrs} #[repr(C{align})] struct {mirror}{generics}({tag}, {prefix}_union{arguments}) {where_clause};\n"));
    }
    extra.push_str(&format!("{attrs} const _: () = assert!(::core::mem::size_of::<{ty}>() == ::core::mem::size_of::<{mirror}{static_arguments}>()\
            && ::core::mem::align_of::<{ty}>() == ::core::mem::align_of::<{mirror}{static_arguments}>());\n"));
    // Attribute-bearing array elements let rustc prune variants and fields before const
    // evaluation. Concatenate only the retained slices into one promoted static array.
    let table = format!("{{
        const VARIANTS: &[&[u64]] = &[{slices}];
        const LEN: usize = {{ let mut len = 2; let mut v = 0; while v < VARIANTS.len() {{ len += VARIANTS[v].len(); v += 1; }} len }};
        const TABLE: [u64; LEN] = {{
            let mut table = [0; LEN]; table[0] = ::core::mem::size_of::<{tag}>() as u64; table[1] = VARIANTS.len() as u64;
            let mut at = 2; let mut v = 0;
            while v < VARIANTS.len() {{ let mut f = 0; while f < VARIANTS[v].len() {{ table[at] = VARIANTS[v][f]; at += 1; f += 1; }} v += 1; }}
            table
        }};
        &TABLE
    }}");
    Ok((extra, table))
}

fn expand_type(toks: &[TokenTree], args: &[String]) -> Result<(TokenStream, String), String> {
    let mut i = skip_attrs(toks, 0);
    while !(is_ident(toks.get(i), "struct") || is_ident(toks.get(i), "enum")) {
        i += 1;
    }
    let is_struct = is_ident(toks.get(i), "struct");
    let name = match toks.get(i + 1) {
        Some(TokenTree::Ident(id)) => id.to_string(),
        _ => return Err("a type without a name".to_string()),
    };
    let mut body_start = i + 2;
    let mut generics = String::new();
    let mut lifetimes = Vec::new();
    if is_punct(toks.get(i + 2), '<') {
        let lifetimes_only = {
            let mut k = i + 3;
            let mut ok = true;
            while k < toks.len() && !is_punct(toks.get(k), '>') {
                if let TokenTree::Ident(_) = &toks[k] {
                    if !is_punct(toks.get(k - 1), '\'') {
                        ok = false;
                    }
                }
                k += 1;
            }
            ok
        };
        if !lifetimes_only {
            return Err(format!("`{name}` is generic: mark it per instance through an op instead"));
        }
        let end = toks[i + 3..].iter().position(|t| is_punct(Some(t), '>')).ok_or("cannot read lifetime parameters")? + i + 3;
        generics = source(&toks[i + 2..=end]);
        for param in split_commas(&toks[i + 3..end]) {
            if !is_punct(param.first(), '\'') || !matches!(param.get(1), Some(TokenTree::Ident(_))) {
                return Err("cannot read lifetime parameter".to_string());
            }
            lifetimes.push(source(&param[..2]));
        }
        body_start = end + 1;
    }
    let ty = if lifetimes.is_empty() { name.clone() } else { format!("{name}<{}>", vec!["'static"; lifetimes.len()].join(", ")) };
    let id = fnv(&format!("type {name}"));
    let plain = args.iter().any(|a| a == "plain");
    let handle = args.iter().any(|a| a == "handle");
    if plain == handle {
        return Err(format!("`{name}`: say #[relax_api(plain)] or #[relax_api(handle, ..)]"));
    }
    let key = format!("{{ fn k() -> &'static str {{ concat!(module_path!(), \"::{name}\") }} k }}");
    let none = "{ fn s() -> &'static str { \"\" } s }";
    let (mut extra, enum_layout) = if is_struct {
        (String::new(), "&[]".to_string())
    } else {
        expand_enum_layout(toks, &name, id, &generics, &lifetimes, body_start)?
    };
    extra.push_str(&type_option_code(toks, body_start, is_struct, id));
    // field offsets of a struct, in declaration order: a plain type's are checked against
    // Relax's; a handle's let Relax use rustc's layout when it is its own (provided.rs verify)
    let mut offsets = Vec::new();
    if is_struct {
        if let Some(TokenTree::Group(g)) = toks[i..].iter().find(|t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace || g.delimiter() == Delimiter::Parenthesis)) {
            let fields: Vec<TokenTree> = g.stream().into_iter().collect();
            for (n, f) in split_commas(&fields).iter().enumerate() {
                let fname = if g.delimiter() == Delimiter::Brace {
                    let c = first_colon(f).ok_or("cannot read a field")?;
                    match f[..c].iter().rev().find_map(|t| if let TokenTree::Ident(i) = t { Some(i.to_string()) } else { None }) {
                        Some(n) => n,
                        None => continue,
                    }
                } else {
                    n.to_string()
                };
                offsets.push(format!("::core::mem::offset_of!({ty}, {fname}) as u64"));
            }
        }
    }
    if plain {
        extra.push_str(&format!(
            "{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_t{id:016x}: ::relax_api::Entry = ::relax_api::Entry {{
  kind: ::relax_api::PLAIN, module: module_path!(), key: {key}, sig: {none},
  shim: None, slots: &[], size: ::core::mem::size_of::<{ty}>() as u64, align: ::core::mem::align_of::<{ty}>() as u64,
  offsets: &[{}], enum_layout: {enum_layout}, drop: None, addr: None }};",
            offsets.join(", ")
        ));
    } else {
        extra.push_str(&format!(
            "{CFG} #[doc(hidden)] #[allow(non_snake_case)] unsafe extern \"C\" fn __relax_api_d{id:016x}(p: *mut u8) {{ let _ = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| unsafe {{ ::core::ptr::drop_in_place(p as *mut {ty}) }})); }}
{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_t{id:016x}: ::relax_api::Entry = ::relax_api::Entry {{
  kind: ::relax_api::HANDLE, module: module_path!(), key: {key}, sig: {none},
  shim: None, slots: &[], size: ::core::mem::size_of::<{ty}>() as u64, align: ::core::mem::align_of::<{ty}>() as u64,
  offsets: &[{}], enum_layout: {enum_layout}, drop: if ::core::mem::needs_drop::<{ty}>() {{ Some(__relax_api_d{id:016x}) }} else {{ None }}, addr: None }};",
            offsets.join(", ")
        ));
        if args.iter().any(|a| a == "default") {
            // a derived Default: an ordinary op (JIT code may not build the handle itself)
            let h = fnv(&format!("{name} default"));
            let phase = op_phase(args);
            extra.push_str(&format!(
                "\n{CFG} #[doc(hidden)] #[allow(non_snake_case, unused_unsafe)] unsafe extern \"C\" fn __relax_api_h{h:016x}(a: *const *mut u8, r: *mut u8) -> u32 {{ let _ = a; {phase} match ::std::panic::catch_unwind(<{ty} as ::core::default::Default>::default) {{ Ok(v) => {{ ::core::ptr::write(r as *mut {ty}, v); 0 }} Err(p) => ::relax_api::caught(p) }} }}
{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_h{h:016x}_E: ::relax_api::Entry = ::relax_api::Entry {{
  kind: ::relax_api::OP, module: module_path!(),
  key: {{ fn k() -> &'static str {{ ::core::any::type_name_of_val(&<{ty} as ::core::default::Default>::default) }} k }},
  sig: {{ fn s() -> &'static str {{ ::core::any::type_name::<fn() -> {ty}>() }} s }},
  shim: Some(__relax_api_h{h:016x}), slots: &[[::core::mem::size_of::<{ty}>() as u64, ::core::mem::align_of::<{ty}>() as u64, 0, 0]], size: 0, align: 0, offsets: &[], enum_layout: &[], drop: None, addr: None }};"
            ));
        }
        // the handle's trait ops, under the keys Relax's bridges look up
        let mut helper = |tag: &str, trait_key: &str, call: String| {
            let h = fnv(&format!("{name} {tag}"));
            let phase = op_phase(args);
            extra.push_str(&format!(
                "\n{CFG} #[doc(hidden)] #[allow(non_snake_case, unused_unsafe)] unsafe extern \"C\" fn __relax_api_h{h:016x}(a: *const *mut u8, r: *mut u8) -> u32 {{ {phase} match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| unsafe {{ {call} }})) {{ Ok(v) => {{ ::core::ptr::write(r as *mut _, v); 0 }} Err(p) => ::relax_api::caught(p) }} }}
{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_h{h:016x}_E: ::relax_api::Entry = ::relax_api::Entry {{
  kind: ::relax_api::OP, module: module_path!(),
  key: {{ fn k() -> &'static str {{ concat!(\"<\", module_path!(), \"::{name} as {trait_key}\") }} k }}, sig: {none},
  shim: Some(__relax_api_h{h:016x}), slots: &[], size: 0, align: 0, offsets: &[], enum_layout: &[], drop: None, addr: None }};"
            ));
        };
        if args.iter().any(|a| a == "clone") {
            helper("clone", "Clone>::clone", format!("<{ty} as ::core::clone::Clone>::clone(::core::ptr::read(*a.add(0) as *const _))"));
        }
        if args.iter().any(|a| a == "eq") {
            helper("eq", "PartialEq>::eq", format!("<{ty} as ::core::cmp::PartialEq>::eq(::core::ptr::read(*a.add(0) as *const _), ::core::ptr::read(*a.add(1) as *const _))"));
        }
        if args.iter().any(|a| a == "hash") {
            helper("hash", "Hash>::hash_into", format!("{{ let x: &{ty} = ::core::ptr::read(*a.add(0) as *const &{ty}); let st: *mut u8 = ::core::ptr::read(*a.add(1) as *const *mut u8); let w: unsafe extern \"C-unwind\" fn(*mut u8, *const u8, usize) = ::core::ptr::read(*a.add(2) as *const _); ::core::hash::Hash::hash(x, &mut ::relax_api::RelaxHasher(st, w)) }}"));
        }
        let debug = args.iter().any(|a| a == "debug");
        let display = args.iter().any(|a| a == "display");
        if debug || display {
            let arms = format!(
                "{}{}",
                if debug { "0 => format!(\"{:?}\", x), 1 => format!(\"{:#?}\", x), " } else { "" },
                if display { "_ => format!(\"{}\", x)," } else { "_ => unreachable!(), " }
            );
            helper("fmt", "Fmt>::fmt_into", format!("{{ let x: &{ty} = ::core::ptr::read(*a.add(0) as *const &{ty}); let mode: u8 = ::core::ptr::read(*a.add(1) as *const u8); let out: *mut u8 = ::core::ptr::read(*a.add(2) as *const *mut u8); let push: unsafe extern \"C-unwind\" fn(*mut u8, *const u8, usize) = ::core::ptr::read(*a.add(3) as *const _); let s: String = match mode {{ {arms} }}; push(out, s.as_ptr(), s.len()) }}"));
        }
    }
    Ok((toks.iter().cloned().collect(), extra))
}
