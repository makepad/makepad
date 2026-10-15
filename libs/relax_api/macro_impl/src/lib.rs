//! Expansion of `#[relax_api]` (see makepad-relax-api for what the marks mean). The item is
//! passed through unchanged; under the using crate's `host-shims` feature the expansion adds,
//! next to it, the extern "C" wrappers and the `relax_api` link-section entries.

use makepad_micro_proc_macro::proc_macro::{Delimiter, Group, Spacing, TokenStream, TokenTree};
use makepad_micro_proc_macro::TokenBuilder;

/// The attribute: `attr` its arguments, `item` the marked item.
pub fn relax_api_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = idents(&attr);
    let toks: Vec<TokenTree> = item.into_iter().collect();
    let res = match item_kind(&toks) {
        Some("impl") => expand_impl(&toks, &args),
        Some("fn") => expand_fn(&toks, &args),
        Some("struct") | Some("enum") => expand_type(&toks, &args),
        _ => Err("#[relax_api] goes on a fn, an impl block, a struct or an enum".to_string()),
    };
    match res {
        Ok((item, extra)) => {
            let mut tb = TokenBuilder::new();
            tb.stream(Some(item));
            if !extra.is_empty() {
                tb.add(&extra);
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
    let sig = Sig { name, is_unsafe, params, ret, never, generic, marked };
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

/// The wrapper and the entry of one op.
fn op_code(o: &Owner, s: &Sig) -> String {
    let path = call_path(o, &s.name);
    let id = fnv(&format!("{} {} {}", text(&o.self_ty, None), o.trait_.as_ref().map_or(String::new(), |t| text(t, None)), s.name));
    let w = format!("__relax_api_w{:016x}", id);
    let params: Vec<Vec<TokenTree>> = s.params.iter().map(|p| subst_self(p, &o.self_ty)).collect();
    let ret = subst_self(&s.ret, &o.self_ty);
    let args: Vec<String> = (0..params.len()).map(|k| format!("::core::ptr::read(*a.add({k}) as *const _)")).collect();
    let call = format!("{}({})", path, args.join(", "));
    let body = if s.never {
        format!("match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| -> () {{ unsafe {{ {call} }} }})) {{ Ok(()) => 0, Err(p) => ::makepad_relax_api::caught(p) }}")
    } else {
        format!("match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| unsafe {{ {call} }})) {{ Ok(v) => {{ ::core::ptr::write(r as *mut _, v); 0 }} Err(p) => ::makepad_relax_api::caught(p) }}")
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
    format!(
        "{CFG} #[doc(hidden)] #[allow(non_snake_case, unused_unsafe, clippy::all)] unsafe extern \"C\" fn {w}(a: *const *mut u8, r: *mut u8) -> u32 {{ let _ = r; {body} }}
{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static {w}_E: ::makepad_relax_api::Entry = ::makepad_relax_api::Entry {{
  kind: ::makepad_relax_api::OP, module: module_path!(),
  key: {key},
  sig: {{ fn s() -> &'static str {{ ::core::any::type_name::<{fnptr}>() }} s }},
  shim: Some({w}), slots: &[{slots}], size: 0, align: 0, offsets: &[], drop: None }};",
        key = key_code(o, &s.name),
        slots = slots.join(", "),
    )
}

fn expand_fn(toks: &[TokenTree], _args: &[String]) -> Result<(TokenStream, String), String> {
    let (s, _, _) = parse_fn(toks, 0).ok_or("cannot read the fn")?;
    if s.generic {
        return Err(format!("`{}` is generic: an op has one compiled body", s.name));
    }
    if s.params.iter().any(|p| p.iter().any(|t| is_ident(Some(t), "Self"))) {
        return Err(format!("`{}` is a method: put #[relax_api] on its impl block (and on the method inside it)", s.name));
    }
    let o = Owner { self_ty: Vec::new(), trait_: None };
    let extra = op_code(&o, &s);
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
        extra.push_str(&op_code(&owner, s));
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
    }
    let ty = format!("{name}");
    let id = fnv(&format!("type {name}"));
    let plain = args.iter().any(|a| a == "plain");
    let handle = args.iter().any(|a| a == "handle");
    if plain == handle {
        return Err(format!("`{name}`: say #[relax_api(plain)] or #[relax_api(handle, ..)]"));
    }
    let key = format!("{{ fn k() -> &'static str {{ concat!(module_path!(), \"::{name}\") }} k }}");
    let none = "{ fn s() -> &'static str { \"\" } s }";
    let mut extra = String::new();
    if plain {
        // field offsets of a struct, in declaration order
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
        extra.push_str(&format!(
            "{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_t{id:016x}: ::makepad_relax_api::Entry = ::makepad_relax_api::Entry {{
  kind: ::makepad_relax_api::PLAIN, module: module_path!(), key: {key}, sig: {none},
  shim: None, slots: &[], size: ::core::mem::size_of::<{ty}>() as u64, align: ::core::mem::align_of::<{ty}>() as u64,
  offsets: &[{}], drop: None }};",
            offsets.join(", ")
        ));
    } else {
        extra.push_str(&format!(
            "{CFG} #[doc(hidden)] #[allow(non_snake_case)] unsafe extern \"C\" fn __relax_api_d{id:016x}(p: *mut u8) {{ let _ = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| unsafe {{ ::core::ptr::drop_in_place(p as *mut {ty}) }})); }}
{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_t{id:016x}: ::makepad_relax_api::Entry = ::makepad_relax_api::Entry {{
  kind: ::makepad_relax_api::HANDLE, module: module_path!(), key: {key}, sig: {none},
  shim: None, slots: &[], size: ::core::mem::size_of::<{ty}>() as u64, align: ::core::mem::align_of::<{ty}>() as u64,
  offsets: &[], drop: if ::core::mem::needs_drop::<{ty}>() {{ Some(__relax_api_d{id:016x}) }} else {{ None }} }};"
        ));
        if args.iter().any(|a| a == "default") {
            // a derived Default: an ordinary op (JIT code may not build the handle itself)
            let h = fnv(&format!("{name} default"));
            extra.push_str(&format!(
                "\n{CFG} #[doc(hidden)] #[allow(non_snake_case, unused_unsafe)] unsafe extern \"C\" fn __relax_api_h{h:016x}(a: *const *mut u8, r: *mut u8) -> u32 {{ let _ = a; match ::std::panic::catch_unwind(<{ty} as ::core::default::Default>::default) {{ Ok(v) => {{ ::core::ptr::write(r as *mut {ty}, v); 0 }} Err(p) => ::makepad_relax_api::caught(p) }} }}
{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_h{h:016x}_E: ::makepad_relax_api::Entry = ::makepad_relax_api::Entry {{
  kind: ::makepad_relax_api::OP, module: module_path!(),
  key: {{ fn k() -> &'static str {{ ::core::any::type_name_of_val(&<{ty} as ::core::default::Default>::default) }} k }},
  sig: {{ fn s() -> &'static str {{ ::core::any::type_name::<fn() -> {ty}>() }} s }},
  shim: Some(__relax_api_h{h:016x}), slots: &[[::core::mem::size_of::<{ty}>() as u64, ::core::mem::align_of::<{ty}>() as u64, 0, 0]], size: 0, align: 0, offsets: &[], drop: None }};"
            ));
        }
        // the handle's trait ops, under the keys Relax's bridges look up
        let mut helper = |tag: &str, trait_key: &str, call: String| {
            let h = fnv(&format!("{name} {tag}"));
            extra.push_str(&format!(
                "\n{CFG} #[doc(hidden)] #[allow(non_snake_case, unused_unsafe)] unsafe extern \"C\" fn __relax_api_h{h:016x}(a: *const *mut u8, r: *mut u8) -> u32 {{ match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| unsafe {{ {call} }})) {{ Ok(v) => {{ ::core::ptr::write(r as *mut _, v); 0 }} Err(p) => ::makepad_relax_api::caught(p) }} }}
{CFG} {SECTION} #[doc(hidden)] #[allow(non_upper_case_globals)] static __relax_api_h{h:016x}_E: ::makepad_relax_api::Entry = ::makepad_relax_api::Entry {{
  kind: ::makepad_relax_api::OP, module: module_path!(),
  key: {{ fn k() -> &'static str {{ concat!(\"<\", module_path!(), \"::{name} as {trait_key}\") }} k }}, sig: {none},
  shim: Some(__relax_api_h{h:016x}), slots: &[], size: 0, align: 0, offsets: &[], drop: None }};"
            ));
        };
        if args.iter().any(|a| a == "clone") {
            helper("clone", "Clone>::clone", format!("<{ty} as ::core::clone::Clone>::clone(::core::ptr::read(*a.add(0) as *const _))"));
        }
        if args.iter().any(|a| a == "eq") {
            helper("eq", "PartialEq>::eq", format!("<{ty} as ::core::cmp::PartialEq>::eq(::core::ptr::read(*a.add(0) as *const _), ::core::ptr::read(*a.add(1) as *const _))"));
        }
        if args.iter().any(|a| a == "hash") {
            helper("hash", "Hash>::hash_into", format!("{{ let x: &{ty} = ::core::ptr::read(*a.add(0) as *const &{ty}); let st: *mut u8 = ::core::ptr::read(*a.add(1) as *const *mut u8); let w: unsafe extern \"C-unwind\" fn(*mut u8, *const u8, usize) = ::core::ptr::read(*a.add(2) as *const _); ::core::hash::Hash::hash(x, &mut ::makepad_relax_api::RelaxHasher(st, w)) }}"));
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
