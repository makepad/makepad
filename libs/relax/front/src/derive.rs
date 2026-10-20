// ported from N:rustc_builtin_macros/src/deriving/clone.rs @8d1a764
// ported from N:rustc_builtin_macros/src/deriving/copy.rs @8d1a764
// ported from N:rustc_builtin_macros/src/deriving/eq.rs @8d1a764
// ported from N:rustc_builtin_macros/src/deriving/partial_eq.rs @8d1a764
// ported from N:rustc_builtin_macros/src/deriving/generic/mod.rs @8d1a764
//! Builtin derives expand into ordinary impls before definition collection.
//!
//! Child module of `parser`.

use super::{PErr, PResult, Parser};
use crate::ast::*;
use crate::lexer::{Tok, T};
use crate::tg;
use crate::tokbuf::TokBuf as Gen;

/// a `>` without a token of its own (see Parser::closes)
const GT: usize = usize::MAX;

/// Derives this module expands.
pub const EXPANDED: &[&str] = &["Debug", "Default", "Hash", "PartialOrd", "Ord", "Clone", "Copy", "Eq", "PartialEq"];

/// The trait surface of compiler-owned derives, including marker/glue derives.
/// Hosted derives with the same spelling must still use their actual output.
pub fn trait_path(name: &str) -> Option<&'static str> {
    Some(match name {
        "Debug" => "fmt::Debug", "Default" => "default::Default", "Hash" => "hash::Hash",
        "PartialOrd" => "cmp::PartialOrd", "Ord" => "cmp::Ord", "PartialEq" => "cmp::PartialEq",
        "Eq" => "cmp::Eq", "Copy" => "marker::Copy", "Clone" => "clone::Clone",
        _ => return None,
    })
}

struct Shape {
    /// variant name (None: the struct itself)
    name: Option<String>,
    /// named fields (Some) or tuple fields (None names)
    fields: Vec<Option<String>>,
    kind: u8, // 0 unit, 1 tuple, 2 struct
    default_attr: bool,
}

/// Name of the anonymous const holding the declared default of field `field` of struct `ty`
/// (or of `ty`'s variant `variant`): `field: T = expr` (default_field_values).
pub fn default_const_name(ty: &str, variant: Option<&str>, field: &str) -> String {
    match variant {
        Some(v) => format!("__relax_default_{}_{}_{}", ty, v, field),
        None => format!("__relax_default_{}_{}", ty, field),
    }
}

/// Only plain structs take `#[rust(expr)]` defaults (enum variants' fields never grow live).
fn variant_is_none_live(groups: &[(Option<String>, Vec<FieldDef>)]) -> bool {
    groups.len() == 1 && groups[0].0.is_none()
}

impl<'a> Parser<'a> {


    /// After a struct or enum: every field default value (`field: T = expr`) becomes an
    /// anonymous const of the field's type (rustc: default field values are anonymous consts),
    /// read by `S { a: 1, .. }` for the omitted fields. A generic type's defaults get none
    /// (a const item cannot take the type's parameters): such a `..` is a typeck error.
    pub(super) fn default_consts(&mut self, id: ItemId, tok_lo: usize) -> PResult<()> {
        let it = self.ast.item(id).clone();
        let mut groups: Vec<(Option<String>, Vec<FieldDef>)> = Vec::new();
        match &it.kind {
            ItemKind::Struct(r) if self.ast.row(*r).generics.params.is_empty() && matches!(self.ast.row(*r).data, VariantData::Struct(_)) => {
                let VariantData::Struct(f) = &self.ast.row(*r).data else { unreachable!() };
                groups.push((None, self.ast.list(*f).to_vec()))
            }
            ItemKind::Enum(r) if self.ast.row(*r).generics.params.is_empty() => {
                for v in self.ast.list(self.ast.row(*r).variants) {
                    if let VariantData::Struct(f) = &v.data {
                        groups.push((Some(String::from_utf8_lossy(self.bytes(v.name.lo, v.name.hi)).into_owned()), self.ast.list(*f).to_vec()));
                    }
                }
            }
            _ => return Ok(()),
        }
        let name = String::from_utf8_lossy(self.bytes(it.name.lo, it.name.hi)).trim_start_matches("r#").to_string();
        let mut g = Gen::new(self.edition);
        // live sessions (M10): a struct field's `#[rust(expr)]` initial value (makepad's) is its
        // default too, for running values when an edit adds the field (lower.rs ext_project)
        let live = variant_is_none_live(&groups) && self.session.options.live;
        for (variant, fields) in &groups {
            for f in fields {
                let Some(fname) = f.name else { continue };
                // the initial value's tokens: a byte span of the item's tokens, or the attribute's token range
                let init = match f.default {
                    Some(e) => {
                        let ex = self.ast.expr(e);
                        (ex.lo, ex.hi, None)
                    }
                    None if live => match self.rust_attr_expr(self.ast.list(f.attrs)) {
                        Some(r) => (0, 0, Some(r)),
                        None => continue,
                    },
                    None => continue,
                };
                let fname = String::from_utf8_lossy(self.bytes(fname.lo, fname.hi)).trim_start_matches("r#").to_string();
                // a `#[rust(expr)]` initial value is a run-time expression, not a constant of the
                // program: marked, so the const checker leaves it alone (a real default field
                // value is an anonymous const like any other)
                if f.default.is_none() {
                    tg!(&mut g; #[relax_live_default]);
                }
                tg!(&mut g; #[allow(non_upper_case_globals)] const #word(&default_const_name(&name, variant.as_deref(), &fname)) :);
                self.copy_bytes(&mut g, tok_lo, f.ty_span.0, f.ty_span.1);
                tg!(&mut g; =);
                match init.2 {
                    None => self.copy_bytes(&mut g, tok_lo, init.0, init.1),
                    Some((a, b)) => self.copy_toks(&mut g, a, b),
                }
                tg!(&mut g; ;);
            }
        }
        if g.is_empty() {
            return Ok(());
        }
        let start = self.splice_buf("field default", &g, it.name.lo)?;
        let items = self.in_expansion(start, |p| p.items_until(T::Eof))?;
        self.extra_items.extend(items);
        Ok(())
    }

    /// The tokens of a field attribute `#[rust(expr)]` (makepad's initial value): their range.
    fn rust_attr_expr(&self, attrs: &[Attr]) -> Option<(usize, usize)> {
        for a in attrs {
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            if hi < lo + 3 || self.tok_text(self.toks.at(lo)) != b"rust" || self.tok_text(self.toks.at(lo + 1)) != b"(" {
                continue;
            }
            let (s, e) = (self.toks.at(lo + 2).lo, self.toks.at(hi - 2).hi);
            if self.tok_text(self.toks.at(hi - 1)) != b")" || e <= s {
                continue;
            }
            return Some((lo + 2, hi - 1));
        }
        None
    }

    /// Token `i` as the source spelled it (a `>` the parser split off the token is restored).
    fn copy_tok(&self, g: &mut Gen, i: usize) {
        let t = self.toks.at(i);
        if t.kind == T::Gt && self.closes(i) == 2 {
            g.punct(T::Gt);
            g.punct(T::Gt);
        } else {
            g.put(t.kind, &String::from_utf8_lossy(self.tok_text(t)));
        }
    }

    /// Tokens `lo..hi`.
    pub(crate) fn copy_toks(&self, g: &mut Gen, lo: usize, hi: usize) {
        for i in lo..hi {
            self.copy_tok(g, i);
        }
    }

    /// The tokens of the byte span `lo..hi`, found by scanning on from token `from`.
    fn copy_bytes(&self, g: &mut Gen, from: usize, lo: u32, hi: u32) {
        let n = self.toks.len();
        let mut i = from;
        while i < n && self.toks.at(i).hi <= lo {
            i += 1;
        }
        while i < n && self.toks.at(i).hi <= hi {
            self.copy_tok(g, i);
            i += 1;
        }
    }

    /// After an item: its expandable derives become impl items in `extra_items`.
    pub(super) fn derive_after(&mut self, id: ItemId, tok_lo: usize) -> PResult<()> {
        let _timing = crate::parse_work::time(2);
        let s2 = match &self.macros {
            Some(m) => m.table.s2_fmt,
            None => false,
        };
        if !s2 {
            return Ok(());
        }
        let it = self.ast.item(id).clone();
        if !matches!(it.kind, ItemKind::Struct(..) | ItemKind::Enum(..) | ItemKind::Union(..)) {
            return Ok(());
        }
        let mut wanted: Vec<&'static str> = Vec::new();
        for a in self.ast.list(it.attrs) {
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            if hi <= lo || self.tok_text(self.toks.at(lo)) != b"derive" {
                continue;
            }
            for i in lo + 1..hi {
                let t = self.toks.at(i);
                if t.kind != T::Ident {
                    continue;
                }
                // last segment of a path (`fmt::Debug`): followed by `,` or `)`
                if i + 1 < hi && self.toks.at(i + 1).kind == T::PathSep {
                    continue;
                }
                let n = self.tok_text(t);
                if let Some(e) = EXPANDED.iter().find(|x| x.as_bytes() == n) {
                    // (written twice, a derive is expanded twice: the impls then conflict, E0119)
                    wanted.push(e);
                }
            }
        }
        if wanted.is_empty() {
            return Ok(());
        }
        let has_copy = wanted.contains(&"Copy");
        for &tr in &wanted {
            match tr {
                "Clone" => self.expand_deriving_clone(&it, tok_lo, has_copy)?,
                "Copy" => self.expand_deriving_copy(&it, tok_lo)?,
                "Eq" => self.expand_deriving_eq(&it, tok_lo)?,
                "PartialEq" => self.expand_deriving_partial_eq(&it, tok_lo)?,
                _ => {}
            }
        }
        wanted.retain(|tr| !matches!(*tr, "Clone" | "Copy" | "Eq" | "PartialEq"));
        if wanted.is_empty() { return Ok(()); }
        let _profile = crate::load_profile::span(self.session.runtime.profile, "derive-builtin", &wanted.join(","));
        let lo = self.ast.list(it.attrs).iter().map(|a| self.toks.at(a.toks.lo as usize).lo).min().unwrap_or(it.lo).min(it.lo);
        let key = crate::parse_work::key(self, "native-derive", lo, it.hi, false);
        if let Some(x) = crate::parse_work::get(&key) {
            return self.install_derive_buf(&Gen::from_parts(self.edition, x.text, x.toks), it.name.lo);
        }
        let name = String::from_utf8_lossy(self.bytes(it.name.lo, it.name.hi)).into_owned();
        let (gparams, where_buf) = self.derive_generics(tok_lo, &it);
        // field types naming a param's associated type (`x: T::Wide`): rustc's derive bounds
        // them too (`where T::Wide: Debug`, deriving/generic find_type_parameters)
        let mut assoc_tys: Vec<String> = Vec::new();
        {
            let tparams: Vec<&str> = gparams.iter().filter(|g| g.0 == 1).map(|g| g.1.as_str()).collect();
            let mut i = tok_lo;
            while i < self.toks.len() && self.toks.at(i).hi <= it.hi {
                let t = self.toks.at(i);
                if t.kind == T::Ident && t.lo > it.name.hi && i + 2 < self.toks.len() && self.toks.at(i + 1).kind == T::PathSep && self.toks.at(i + 2).kind == T::Ident && (i == 0 || self.toks.at(i - 1).kind != T::PathSep) {
                    let n = String::from_utf8_lossy(self.tok_text(t)).into_owned();
                    if tparams.contains(&n.as_str()) {
                        let mut path = n;
                        let mut j = i + 1;
                        while j + 1 < self.toks.len() && self.toks.at(j).kind == T::PathSep && self.toks.at(j + 1).kind == T::Ident {
                            path.push_str("::");
                            path.push_str(&String::from_utf8_lossy(self.tok_text(self.toks.at(j + 1))));
                            j += 2;
                        }
                        // (a path call `T::new(..)` is no type)
                        if !(j < self.toks.len() && self.toks.at(j).kind == T::OpenParen) && !assoc_tys.contains(&path) {
                            assoc_tys.push(path);
                        }
                        i = j;
                        continue;
                    }
                }
                i += 1;
            }
        }
        let shapes = self.derive_shapes(&it);
        let is_enum = matches!(it.kind, ItemKind::Enum(..));
        // the discriminant type (`#[repr(u8)]` ..; isize by default): derived orderings compare
        // discriminant values as this type (rustc's derive uses discriminant_value)
        let mut disc_ty = "isize".to_string();
        for a in self.ast.list(it.attrs) {
            let (lo, hi) = (a.toks.lo as usize, a.toks.hi as usize);
            if hi <= lo || self.tok_text(self.toks.at(lo)) != b"repr" {
                continue;
            }
            for i in lo + 1..hi {
                let n = self.tok_text(self.toks.at(i));
                if [&b"u8"[..], b"u16", b"u32", b"u64", b"u128", b"usize", b"i8", b"i16", b"i32", b"i64", b"i128", b"isize"].contains(&n) {
                    disc_ty = String::from_utf8_lossy(n).into_owned();
                }
            }
        }
        let mut all = Gen::new(self.edition);
        for tr in wanted {
            let _profile = crate::load_profile::span(self.session.runtime.profile, "derive-builtin-generate", tr);
            let path = trait_path(tr).expect("compiler derive trait");
            let mut body = Gen::new(self.edition);
            match tr {
                "Debug" => gen_debug(&mut body, &name, is_enum, &shapes),
                "Default" => {
                    if !gen_default(&mut body, &name, is_enum, &shapes) {
                        return Err(PErr { pos: it.name.lo, msg: "derive(Default) on an enum needs one `#[default]` unit variant".to_string() });
                    }
                }
                "Hash" => {
                    // the method's hasher parameter must not shadow a generic of the type
                    // (rustc's derive names it hygienically)
                    let mut h = "__H".to_string();
                    while gparams.iter().any(|g| g.1 == h) {
                        h.push('_');
                    }
                    gen_hash(&mut body, &name, is_enum, &shapes, &h)
                }
                "PartialOrd" => gen_ord(&mut body, &name, is_enum, &shapes, true, &disc_ty),
                _ => gen_ord(&mut body, &name, is_enum, &shapes, false, &disc_ty),
            };
            // impl<params + bound> Trait for Name<args> where ..
            tg!(&mut all; impl);
            if !gparams.is_empty() {
                all.punct(T::Lt);
                for (k, (kind, n, bounds)) in gparams.iter().enumerate() {
                    if k > 0 {
                        all.punct(T::Comma);
                    }
                    match kind {
                        0 => {
                            all.word(n);
                            if !bounds.is_empty() {
                                tg!(&mut all; : #append(bounds));
                            }
                        }
                        // (`#[derive(Default)]` on an enum takes its `#[default]` unit variant: no
                        // `T: Default` bounds, rustc deriving/default.rs skip_path_as_bound)
                        1 if tr == "Default" && is_enum => {
                            all.word(n);
                            if !bounds.is_empty() {
                                tg!(&mut all; : #append(bounds));
                            }
                        }
                        1 => {
                            tg!(&mut all; #word(n) :);
                            if !bounds.is_empty() {
                                tg!(&mut all; #append(bounds) +);
                            }
                            trait_path_tokens(&mut all, path);
                        }
                        _ => tg!(&mut all; const #word(n) : #append(bounds)),
                    }
                }
                all.punct(T::Gt);
            }
            trait_path_tokens(&mut all, path);
            tg!(&mut all; for #word(&name));
            if !gparams.is_empty() {
                all.punct(T::Lt);
                for (k, (_, n, _)) in gparams.iter().enumerate() {
                    if k > 0 {
                        all.punct(T::Comma);
                    }
                    all.word(n);
                }
                all.punct(T::Gt);
            }
            let mut wt = where_buf.clone();
            wt.trim_comma();
            let mut have_where = !wt.is_empty();
            all.append(&wt);
            for at in assoc_tys.iter().filter(|_| !(tr == "Default" && is_enum)) {
                if have_where {
                    all.punct(T::Comma);
                } else {
                    all.word("where");
                    have_where = true;
                }
                for (k, seg) in at.split("::").enumerate() {
                    if k > 0 {
                        all.punct(T::PathSep);
                    }
                    all.word(seg);
                }
                all.punct(T::Colon);
                trait_path_tokens(&mut all, path);
            }
            tg!(&mut all; { #append(&body) });
        }
        if let Some(t) = crate::preload::relax_trace() {
            if t.split(',').any(|p| p.strip_prefix("derive:").map_or(false, |n| name.contains(n))) {
                eprintln!("derive {} => {}", name, all.s);
            }
        }
        crate::parse_work::put(key, &crate::builtins::Expansion { text: all.s.clone(), toks: all.toks.clone(), origins: Vec::new() });
        self.install_derive_buf(&all, it.name.lo)
    }

    fn install_derive_buf(&mut self, g: &Gen, call: u32) -> PResult<()> {
        let source = self.source_span(call);
        let start = self.splice_buf("derive", g, call)?;
        self.token_origins.extend(self.toks.range(start, self.toks.len()).iter().filter(|t| t.kind != T::Eof)
            .map(|t| crate::source::TokenOrigin { lo: t.lo, hi: t.hi, source }));
        let items = self.in_expansion(start, |p| p.items_until(T::Eof))?;
        self.extra_items.extend(items);
        Ok(())
    }

    /// The item's generic params as (kind 0 lifetime / 1 type / 2 const, name, bounds or
    /// const type text) and its where clause text, from its tokens.
    fn derive_generics(&self, tok_lo: usize, it: &Item) -> (Vec<(u8, String, Gen)>, Gen) {
        let mut out = Vec::new();
        // the name token, then `<`
        let mut i = tok_lo;
        while i < self.toks.len() && !(self.toks.at(i).lo == it.name.lo && self.toks.at(i).hi == it.name.hi) {
            i += 1;
        }
        i += 1;
        let mut end_generics = i;
        if i < self.toks.len() && self.toks.at(i).kind == T::Lt {
            // split params at top-level commas up to the matching `>`
            let mut depth = 1i32;
            let mut j = i + 1;
            let mut cur: Vec<usize> = Vec::new();
            let mut params: Vec<Vec<usize>> = Vec::new();
            while j < self.toks.len() {
                let k = self.toks.at(j).kind;
                let before = depth;
                match k {
                    T::Lt => depth += 1,
                    T::Shl => depth += 2,
                    T::Gt | T::Shr => depth -= self.closes(j),
                    T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 100,
                    T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 100,
                    _ => {}
                }
                if depth <= 0 {
                    // `u16>>`: the first `>` (no token of its own) closes inside the param
                    for _ in 1..before.max(1) {
                        cur.push(GT);
                    }
                    break;
                }
                if k == T::Comma && depth == 1 {
                    params.push(std::mem::take(&mut cur));
                } else {
                    cur.push(j);
                }
                j += 1;
            }
            if !cur.is_empty() {
                params.push(cur);
            }
            end_generics = j + 1;
            for p in params {
                // drop attributes and defaults
                let mut toks: Vec<usize> = Vec::new();
                let mut k = 0;
                let mut ad = 0i32;
                while k < p.len() {
                    if p[k] == GT {
                        ad -= 1;
                        toks.push(GT);
                        k += 1;
                        continue;
                    }
                    let t = self.toks.at(p[k]);
                    match t.kind {
                        T::Lt => ad += 1,
                        T::Gt | T::Shr => ad -= self.closes(p[k]),
                        T::Shl => ad += 2,
                        _ => {}
                    }
                    if t.kind == T::Pound {
                        // #[..]
                        let mut d = 0;
                        k += 1;
                        while k < p.len() {
                            match self.toks.at(p[k]).kind {
                                T::OpenBracket => d += 1,
                                T::CloseBracket => {
                                    d -= 1;
                                    if d == 0 {
                                        k += 1;
                                        break;
                                    }
                                }
                                _ => {}
                            }
                            k += 1;
                        }
                        continue;
                    }
                    if t.kind == T::Eq && ad == 0 {
                        break;
                    }
                    toks.push(p[k]);
                    k += 1;
                }
                if toks.is_empty() {
                    continue;
                }
                if toks[0] == GT {
                    continue;
                }
                let first = self.toks.at(toks[0]);
                let txt = |s: &Self, v: &[usize]| -> Gen {
                    let mut r = Gen::new(s.edition);
                    for &x in v {
                        if x == GT {
                            r.punct(T::Gt);
                        } else {
                            s.copy_tok(&mut r, x);
                        }
                    }
                    r
                };
                let name_of = |s: &Self, x: usize| -> String { String::from_utf8_lossy(s.tok_text(s.toks.at(x))).into_owned() };
                if first.kind == T::Lifetime {
                    let n = name_of(self, toks[0]);
                    let b = if toks.len() > 2 { txt(self, &toks[2..]) } else { Gen::new(self.edition) };
                    out.push((0, n, b));
                } else if first.kind == T::KwConst {
                    let n = if toks.len() > 1 { name_of(self, toks[1]) } else { String::new() };
                    let ty = if toks.len() > 3 { txt(self, &toks[3..]) } else { Gen::new(self.edition) };
                    out.push((2, n, ty));
                } else {
                    let n = name_of(self, toks[0]);
                    let b = if toks.len() > 2 { txt(self, &toks[2..]) } else { Gen::new(self.edition) };
                    out.push((1, n, b));
                }
            }
        }
        // where clause: from `where` at depth 0 to the body `{` or `;` (tuple structs: after `( .. )`)
        let mut w = Gen::new(self.edition);
        let mut j = end_generics;
        let mut depth = 0i32;
        while j < self.toks.len() {
            let k = self.toks.at(j).kind;
            if depth == 0 && k == T::KwWhere {
                let mut e = j;
                let mut d2 = 0i32;
                while e < self.toks.len() {
                    let ke = self.toks.at(e).kind;
                    match ke {
                        T::Lt => d2 += 1,
                        T::Gt | T::Shr => d2 -= self.closes(e),
                        T::Shl => d2 += 2,
                        T::OpenParen | T::OpenBracket => d2 += 100,
                        T::CloseParen | T::CloseBracket => d2 -= 100,
                        _ => {}
                    }
                    if d2 <= 0 && (ke == T::OpenBrace || ke == T::Semi) {
                        break;
                    }
                    self.copy_tok(&mut w, e);
                    e += 1;
                }
                break;
            }
            match k {
                T::OpenParen | T::OpenBracket => depth += 1,
                T::CloseParen | T::CloseBracket => depth -= 1,
                T::OpenBrace | T::Semi if depth == 0 => break,
                _ => {}
            }
            j += 1;
        }
        (out, w)
    }

    /// How many `>` token `i` closes: the parser splits `>>` in place (the token becomes the
    /// second `>`), so a `>` right after a `>` byte no token covers counts twice.
    fn closes(&self, i: usize) -> i32 {
        let t = self.toks.at(i);
        if t.kind == T::Shr {
            return 2;
        }
        let prev_hi = if i > 0 { self.toks.at(i - 1).hi } else { 0 };
        if t.lo > prev_hi && self.bytes(t.lo - 1, t.lo) == b">" {
            2
        } else {
            1
        }
    }

    /// Is a field / variant with these attributes present for this target (`#[cfg(..)]`)?
    /// Without a build context (tools that parse without a target) everything counts.
    pub(super) fn cfg_present(&self, attrs: &[Attr]) -> bool {
        let c = match &self.bctx {
            Some(c) if !attrs.is_empty() => c,
            _ => return true,
        };
        // the attribute tokens may sit in macro-expansion text (positions past `src`): copy them
        // into a small source of their own for the cfg evaluator
        let mut src: Vec<u8> = Vec::new();
        let mut toks: Vec<Tok> = Vec::new();
        let mut local: Vec<Attr> = Vec::new();
        for a in attrs {
            let lo = toks.len() as u32;
            for i in a.toks.lo..a.toks.hi {
                let t = self.toks.at(i as usize);
                let b = self.tok_text(t);
                let p = src.len() as u32;
                src.extend_from_slice(b);
                src.push(b' ');
                toks.push(Tok { kind: t.kind, lo: p, hi: p + b.len() as u32 });
            }
            let mut na = a.clone();
            na.toks = TokRange { lo, hi: toks.len() as u32 };
            local.push(na);
        }
        crate::cfg::active(crate::cfg::Src { src: &src, plain: &toks, ext: &[], parts: None }, &local, &c.cfg)
    }

    fn derive_shapes(&self, it: &Item) -> Vec<Shape> {
        let shape_of = |s: &Self, d: &VariantData, name: Option<String>, attrs: &[Attr]| -> Shape {
            let default_attr = attrs.iter().any(|a| a.toks.hi > a.toks.lo && s.tok_text(s.toks.at(a.toks.lo as usize)) == b"default");
            match d {
                VariantData::Unit => Shape { name, fields: Vec::new(), kind: 0, default_attr },
                VariantData::Tuple(f) => Shape { name, fields: s.ast.list(*f).iter().filter(|x| s.cfg_present(s.ast.list(x.attrs))).map(|_| None).collect(), kind: 1, default_attr },
                VariantData::Struct(f) => Shape {
                    name,
                    fields: s.ast.list(*f).iter().filter(|x| s.cfg_present(s.ast.list(x.attrs))).map(|x| x.name.map(|n| String::from_utf8_lossy(s.bytes(n.lo, n.hi)).into_owned())).collect(),
                    kind: 2,
                    default_attr,
                },
            }
        };
        match &it.kind {
            ItemKind::Struct(r) => vec![shape_of(self, &self.ast.row(*r).data, None, &[])],
            ItemKind::Enum(r) => self.ast.list(self.ast.row(*r).variants)
                .iter()
                .filter(|v| self.cfg_present(self.ast.list(v.attrs)))
                .map(|v| shape_of(self, &v.data, Some(String::from_utf8_lossy(self.bytes(v.name.lo, v.name.hi)).into_owned()), self.ast.list(v.attrs)))
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// `::__relax_crate_core` and the path `segs` (`fmt::Debug`) below it.
fn trait_path_tokens(g: &mut Gen, segs: &str) {
    g.core();
    for seg in segs.split("::") {
        g.punct(T::PathSep);
        g.word(seg);
    }
}

/// `Self` or `Self::Variant`.
fn variant_path(g: &mut Gen, variant: Option<&str>) {
    g.word("Self");
    if let Some(v) = variant {
        g.punct(T::PathSep);
        g.word(v);
    }
}

/// `Self::V { a: p0, b: p1 }` / `Self::V(p0, p1)` / `Self::V` with binding prefix `p`.
fn pat(g: &mut Gen, s: &Shape, p: &str) {
    variant_path(g, s.name.as_deref());
    match s.kind {
        0 => {}
        1 => {
            g.open();
            for i in 0..s.fields.len() {
                if i > 0 {
                    g.punct(T::Comma);
                }
                g.word(&format!("{}{}", p, i));
            }
            g.close();
        }
        _ => {
            g.open_brace();
            for (i, n) in s.fields.iter().enumerate() {
                if i > 0 {
                    g.punct(T::Comma);
                }
                g.word(n.as_deref().unwrap_or("_"));
                g.punct(T::Colon);
                g.word(&format!("{}{}", p, i));
            }
            g.close_brace();
        }
    }
}

fn field_name(n: &Option<String>) -> String {
    let n = n.as_deref().unwrap_or("");
    n.strip_prefix("r#").unwrap_or(n).to_string()
}

fn gen_debug(g: &mut Gen, ty: &str, is_enum: bool, shapes: &[Shape]) {
    tg!(g; fn fmt (& self , f : & mut #core() :: fmt :: Formatter < '_ >) -> #core() :: fmt :: Result);
    g.open_brace();
    if is_enum && shapes.is_empty() {
        tg!(g; match * self { });
    } else {
        tg!(g; match self);
        g.open_brace();
        for s in shapes {
            let shown = s.name.clone().unwrap_or_else(|| ty.to_string());
            pat(g, s, "__s");
            g.punct(T::FatArrow);
            match s.kind {
                0 => tg!(g; f . write_str (#str_lit(&shown))),
                1 => {
                    tg!(g; f . debug_tuple (#str_lit(&shown)));
                    // rustc's derive(Debug) passes the last field as `&&self.f` (it may be unsized)
                    for i in 0..s.fields.len() {
                        tg!(g; . field);
                        g.open();
                        if i + 1 == s.fields.len() {
                            g.punct(T::And);
                        }
                        g.word(&format!("__s{}", i));
                        g.close();
                    }
                    tg!(g; . finish ());
                }
                _ => {
                    tg!(g; f . debug_struct (#str_lit(&shown)));
                    for (i, n) in s.fields.iter().enumerate() {
                        tg!(g; . field);
                        g.open();
                        g.str_lit(&field_name(n));
                        g.punct(T::Comma);
                        if i + 1 == s.fields.len() {
                            g.punct(T::And);
                        }
                        g.word(&format!("__s{}", i));
                        g.close();
                    }
                    tg!(g; . finish ());
                }
            }
            g.punct(T::Comma);
        }
        g.close_brace();
    }
    g.close_brace();
}

/// `fn default() -> Self { .. }`; false: an enum without a `#[default]` unit variant.
fn gen_default(g: &mut Gen, ty: &str, is_enum: bool, shapes: &[Shape]) -> bool {
    let Some(s) = (if is_enum { shapes.iter().find(|s| s.default_attr) } else { shapes.first() }) else { return false };
    tg!(g; fn default () -> Self);
    g.open_brace();
    g.word(ty);
    if let Some(v) = &s.name {
        g.punct(T::PathSep);
        g.word(v);
    }
    let default_call = |g: &mut Gen| tg!(g; #core() :: default :: Default :: default ());
    match s.kind {
        0 => {}
        1 => {
            g.open();
            for i in 0..s.fields.len() {
                if i > 0 {
                    g.punct(T::Comma);
                }
                default_call(g);
            }
            g.close();
        }
        _ => {
            g.open_brace();
            for (i, n) in s.fields.iter().enumerate() {
                if i > 0 {
                    g.punct(T::Comma);
                }
                g.word(n.as_deref().unwrap_or("_"));
                g.punct(T::Colon);
                default_call(g);
            }
            g.close_brace();
        }
    }
    g.close_brace();
    true
}

fn gen_hash(g: &mut Gen, _ty: &str, is_enum: bool, shapes: &[Shape], h: &str) {
    tg!(g; fn hash < #word(h) : #core() :: hash :: Hasher > (& self , state : & mut #word(h)));
    g.open_brace();
    if is_enum && shapes.is_empty() {
        tg!(g; match * self { });
    } else {
        tg!(g; match self);
        g.open_brace();
        for (vi, s) in shapes.iter().enumerate() {
            pat(g, s, "__s");
            tg!(g; =>);
            g.open_brace();
            if is_enum && shapes.len() > 1 {
                tg!(g; #core() :: hash :: Hash :: hash (& (#int(format!("{}isize", vi))), state) ;);
            }
            for i in 0..s.fields.len() {
                tg!(g; #core() :: hash :: Hash :: hash (#word(&format!("__s{}", i)), state) ;);
            }
            g.close_brace();
        }
        g.close_brace();
    }
    g.close_brace();
}

/// PartialOrd::partial_cmp (`partial`) or Ord::cmp: lexicographic over variants then fields.
fn gen_ord(g: &mut Gen, _ty: &str, is_enum: bool, shapes: &[Shape], partial: bool, disc_ty: &str) {
    let m = if partial { "partial_cmp" } else { "cmp" };
    let tr = if partial { "PartialOrd" } else { "Ord" };
    let ordering = |g: &mut Gen| tg!(g; #core() :: cmp :: Ordering);
    // the "equal" result
    let eq = |g: &mut Gen| {
        if partial {
            tg!(g; #core() :: option :: Option :: Some (#core() :: cmp :: Ordering :: Equal));
        } else {
            tg!(g; #core() :: cmp :: Ordering :: Equal);
        }
    };
    tg!(g; fn #word(m) (& self , other : & Self) ->);
    if partial {
        tg!(g; #core() :: option :: Option <);
        ordering(g);
        g.punct(T::Gt);
    } else {
        ordering(g);
    }
    g.open_brace();
    if is_enum && shapes.is_empty() {
        tg!(g; match * self { });
        g.close_brace();
        return;
    }
    if is_enum && shapes.len() > 1 {
        // variants order by their discriminant values (explicit `= n` included), compared as
        // the discriminant type
        for (local, of) in [("__si", "self"), ("__oi", "other")] {
            tg!(g; let #word(local) = unsafe { #core() :: intrinsics_mem :: discriminant_value (#word(of)) } as #word(disc_ty) ;);
        }
        tg!(g; if __si != __oi { return #core() :: cmp :: #word(tr) :: #word(m) (& __si , & __oi) ; });
    }
    tg!(g; match (self , other));
    g.open_brace();
    for s in shapes {
        g.open();
        pat(g, s, "__s");
        g.punct(T::Comma);
        pat(g, s, "__o");
        g.close();
        tg!(g; =>);
        g.open_brace();
        for i in 0..s.fields.len() {
            tg!(g; match #core() :: cmp :: #word(tr) :: #word(m) (#word(&format!("__s{}", i)) , #word(&format!("__o{}", i))));
            g.open_brace();
            eq(g);
            tg!(g; => { } __c => return __c ,);
            g.close_brace();
        }
        eq(g);
        g.close_brace();
    }
    if is_enum && shapes.len() > 1 {
        tg!(g; _ =>);
        eq(g);
        g.punct(T::Comma);
    }
    g.close_brace();
    g.close_brace();
}

struct TypeParameter { ty: TyId, bound_generic_params: Vec<GenericParam> }

impl Parser<'_> {
    fn deriving_fields(&self, it: &Item) -> Vec<(Option<String>, VariantData, Vec<FieldDef>)> {
        let mut out = Vec::new();
        let mut data = Vec::new();
        match it.kind {
            ItemKind::Struct(r) => data.push((None, self.ast.row(r).data)),
            ItemKind::Union(r) => data.push((None, VariantData::Struct(self.ast.row(r).fields))),
            ItemKind::Enum(r) => for v in self.ast.list(self.ast.row(r).variants) {
                if self.cfg_present(self.ast.list(v.attrs)) {
                    data.push((Some(String::from_utf8_lossy(self.bytes(v.name.lo, v.name.hi)).into_owned()), v.data));
                }
            },
            _ => unreachable!(),
        }
        for (name, d) in data {
            let mut fields = Vec::new();
            if let VariantData::Tuple(fs) | VariantData::Struct(fs) = d {
                for f in self.ast.list(fs) {
                    if self.cfg_present(self.ast.list(f.attrs)) { fields.push(*f); }
                }
            }
            out.push((name, d, fields));
        }
        out
    }

    fn is_packed(&self, it: &Item) -> bool {
        for a in self.ast.list(it.attrs) {
            if a.toks.lo == a.toks.hi || self.tok_text(self.toks.at(a.toks.lo as usize)) != b"repr" { continue; }
            for i in a.toks.lo..a.toks.hi {
                if self.tok_text(self.toks.at(i as usize)) == b"packed" { return true; }
            }
        }
        false
    }

    /// deriving/generic/mod.rs:379: visit field types, retaining binder scopes.
    fn find_type_parameters(&self, ty: TyId, names: &[String], bound: &mut Vec<GenericParam>, out: &mut Vec<TypeParameter>) {
        match *self.ast.ty(ty) {
            Ty::Path(path) => {
                let segments = self.ast.segs(path.segs);
                if let Some(first) = segments.first() {
                    let name = String::from_utf8_lossy(self.bytes(first.name.lo, first.name.hi));
                    if names.contains(&name.to_string()) && segments.len() > 1 {
                        out.push(TypeParameter { ty, bound_generic_params: bound.clone() });
                    }
                }
                if path.has_qself() { self.find_type_parameters(self.ast.qself(&path).unwrap().ty, names, bound, out); }
                for seg in segments { self.find_args_type_parameters(seg.args(), names, bound, out); }
            }
            Ty::Ref(_, _, t) | Ty::Ptr(_, t) | Ty::Slice(t) | Ty::Array(t, _) | Ty::Paren(t) => self.find_type_parameters(t, names, bound, out),
            Ty::Tuple(ts) => for &t in self.ast.list(ts) { self.find_type_parameters(t, names, bound, out); },
            Ty::Fn(f) => {
                let f = self.ast.fn_ptr(f);
                let len = bound.len(); bound.extend_from_slice(self.ast.list(f.hrtb));
                for &t in self.ast.list(f.params) { self.find_type_parameters(t, names, bound, out); }
                if let Some(t) = f.ret { self.find_type_parameters(t, names, bound, out); }
                bound.truncate(len);
            }
            Ty::ImplTrait(bs) | Ty::DynTrait(bs, _) => self.find_bounds_type_parameters(bs, names, bound, out),
            _ => {}
        }
    }
    fn find_args_type_parameters(&self, args: GenericArgsId, names: &[String], bound: &mut Vec<GenericParam>, out: &mut Vec<TypeParameter>) {
        match self.ast.gargs(args) {
            Some(GenericArgs::Angle(args)) => for &arg in self.ast.list(*args) {
                match arg {
                    GenericArg::Type(t) => self.find_type_parameters(t, names, bound, out),
                    GenericArg::Binding(_, args, t) => { self.find_args_type_parameters(args, names, bound, out); self.find_type_parameters(t, names, bound, out); }
                    GenericArg::Constraint(_, args, bs) => { self.find_args_type_parameters(args, names, bound, out); self.find_bounds_type_parameters(bs, names, bound, out); },
                    _ => {}
                }
            },
            Some(GenericArgs::Paren(ts, ret)) => {
                for &t in self.ast.list(*ts) { self.find_type_parameters(t, names, bound, out); }
                if let Some(t) = ret { self.find_type_parameters(*t, names, bound, out); }
            }
            None => {}
        }
    }
    fn find_bounds_type_parameters(&self, bs: List<Bound>, names: &[String], bound: &mut Vec<GenericParam>, out: &mut Vec<TypeParameter>) {
        for &b in self.ast.list(bs) {
            if let Bound::Trait { hrtb, path, .. } = b {
                let len = bound.len(); bound.extend_from_slice(self.ast.list(hrtb));
                for seg in self.ast.segs(path.segs) { self.find_args_type_parameters(seg.args(), names, bound, out); }
                bound.truncate(len);
            }
        }
    }

    /// deriving/generic/mod.rs:565: declaration bounds plus the derive's bounds
    /// on each type parameter and each field projection, with original binders.
    /// `trait_path` / `bound_path`: below `::__relax_crate_core` (`marker::Copy`).
    fn create_derived_impl(&mut self, it: &Item, tok_lo: usize, trait_path: &str, bound_path: &str, copy_bound: bool, unsafe_: bool, body: &Gen) -> PResult<()> {
        // deriving/generic/mod.rs:486-493: a `repr(packed)` enum gets no derived impl (its repr
        // is an error of its own)
        if matches!(it.kind, ItemKind::Enum(..)) && self.is_packed(it) { return Ok(()); }
        let (params, where_buf) = self.derive_generics(tok_lo, it);
        let mut names = Vec::new();
        let name = String::from_utf8_lossy(self.bytes(it.name.lo, it.name.hi)).into_owned();
        let mut g = Gen::new(self.edition);
        if unsafe_ {
            g.word("unsafe");
        }
        g.word("impl");
        if !params.is_empty() {
            g.punct(T::Lt);
            for (k, (kind, n, b)) in params.iter().enumerate() {
                if k > 0 {
                    g.punct(T::Comma);
                }
                match kind {
                    0 => {
                        g.word(n);
                        if !b.is_empty() {
                            tg!(&mut g; : #append(b));
                        }
                    }
                    1 => {
                        names.push(n.clone());
                        g.word(n);
                        g.punct(T::Colon);
                        trait_path_tokens(&mut g, bound_path);
                        if copy_bound {
                            g.punct(T::Plus);
                            trait_path_tokens(&mut g, "marker::Copy");
                        }
                        if !b.is_empty() {
                            tg!(&mut g; + #append(b));
                        }
                    }
                    _ => tg!(&mut g; const #word(n) : #append(b)),
                }
            }
            g.punct(T::Gt);
        }
        trait_path_tokens(&mut g, trait_path);
        tg!(&mut g; for #word(&name));
        if !params.is_empty() {
            g.punct(T::Lt);
            for (k, (_, n, _)) in params.iter().enumerate() {
                if k > 0 {
                    g.punct(T::Comma);
                }
                g.word(n);
            }
            g.punct(T::Gt);
        }
        g.append(&where_buf);
        tg!(&mut g; { #append(body) });
        let mut field_params = Vec::new();
        if !names.is_empty() {
            for (_, _, fields) in self.deriving_fields(it) {
                for f in fields { self.find_type_parameters(f.ty, &names, &mut Vec::new(), &mut field_params); }
            }
        }
        self.install_derive_buf(&g, it.name.lo)?;
        // (`items_until` hands earlier extras back with the new item: the newest id is the one just installed)
        let iid = *self.extra_items.iter().max_by_key(|i| i.0).expect("derived impl");
        let impl_row = match self.ast.item(iid).kind { ItemKind::Impl(r) => r, _ => unreachable!() };
        let mut generics = self.ast.row(impl_row).generics;
        if !field_params.is_empty() {
            // The generated type parameter has exactly the additional derive bounds
            // before its declared bounds. Reuse those paths without source scanning.
            let bounds_len = if copy_bound { 2 } else { 1 };
            let mut bounds = Vec::new();
            for p in self.ast.list(generics.params) {
                if let GenericParamKind::Type(bs, _) = p.kind {
                    bounds.extend_from_slice(&self.ast.list(bs)[..bounds_len]); break;
                }
            }
            let bs = self.ast.push_list(bounds);
            let mut predicates = self.ast.list(generics.where_).to_vec();
            for p in field_params {
                let hrtb = self.ast.push_list(p.bound_generic_params);
                predicates.push(WherePred::Bound { hrtb, ty: p.ty, bounds: bs });
            }
            generics.where_ = self.ast.push_list(predicates);
            self.ast.row_mut(impl_row).generics = generics;
        }
        Ok(())
    }

    fn expand_deriving_copy(&mut self, it: &Item, lo: usize) -> PResult<()> {
        self.create_derived_impl(it, lo, "marker::Copy", "marker::Copy", false, false, &Gen::new(self.edition))
    }
    fn expand_deriving_clone(&mut self, it: &Item, lo: usize, has_copy: bool) -> PResult<()> {
        let (params, _) = self.derive_generics(lo, it);
        let union = matches!(it.kind, ItemKind::Union(..));
        let simple = union || (has_copy && !params.iter().any(|p| p.0 == 1));
        let copy = union || self.is_packed(it);
        if simple {
            self.create_derived_impl(it, lo, "clone::TrivialClone", "clone::TrivialClone", copy, true, &Gen::new(self.edition))?;
        }
        let body = if simple { self.cs_clone_simple(it, lo, union) } else { self.cs_clone(it) };
        let mut g = Gen::new(self.edition);
        tg!(&mut g; #[inline] fn clone (& self) -> Self { #append(&body) });
        self.create_derived_impl(it, lo, "clone::Clone", "clone::Clone", copy, false, &g)
    }
    /// `let _: ::core::<path><field type>;` for each distinct field type
    fn assert_ty_bounds(&self, it: &Item, tok_lo: usize, path: &str) -> Gen {
        let mut seen = Vec::new(); let mut out = Gen::new(self.edition);
        for (_, _, fields) in self.deriving_fields(it) {
            for f in fields {
                if let Ty::Path(p) = *self.ast.ty(f.ty) {
                    let segs = self.ast.segs(p.segs);
                    if p.no_qself() && segs.len() == 1 && segs[0].args().is_none() {
                        let n = String::from_utf8_lossy(self.bytes(segs[0].name.lo, segs[0].name.hi)).into_owned();
                        if seen.contains(&n) { continue; } seen.push(n);
                    }
                }
                tg!(&mut out; let _ :);
                trait_path_tokens(&mut out, path);
                out.punct(T::Lt);
                self.copy_bytes(&mut out, tok_lo, f.ty_span.0, f.ty_span.1);
                tg!(&mut out; > ;);
            }
        }
        out
    }
    fn cs_clone_simple(&self, it: &Item, tok_lo: usize, union: bool) -> Gen {
        let mut out = Gen::new(self.edition);
        if union {
            tg!(&mut out; let _ : #core() :: clone :: AssertParamIsCopy < Self > ;);
        } else {
            out.append(&self.assert_ty_bounds(it, tok_lo, "clone::AssertParamIsClone"));
        }
        tg!(&mut out; * self);
        out
    }
    fn cs_clone(&self, it: &Item) -> Gen {
        let is_enum = matches!(it.kind, ItemKind::Enum(..));
        let packed = self.is_packed(it);
        let mut arms = Gen::new(self.edition);
        let mut any = false;
        for (name, data, fields) in self.deriving_fields(it) {
            let mut values = Vec::new(); let mut pattern = Vec::new();
            for (i, f) in fields.iter().enumerate() {
                let n = f.name.map_or(i.to_string(), |n| String::from_utf8_lossy(self.bytes(n.lo, n.hi)).into_owned());
                let mut value = Gen::new(self.edition);
                if let VariantData::Struct(_) = data {
                    value.word(&n);
                    value.punct(T::Colon);
                }
                tg!(&mut value; #core() :: clone :: Clone :: clone);
                value.open();
                if is_enum {
                    value.word(&format!("__self_{i}"));
                } else if packed {
                    tg!(&mut value; & { self . #word(&n) });
                } else {
                    tg!(&mut value; & self . #word(&n));
                }
                value.close();
                values.push(value);
                let mut pat = Gen::new(self.edition);
                if let VariantData::Struct(_) = data {
                    pat.word(&n);
                    pat.punct(T::Colon);
                }
                pat.word(&format!("__self_{i}"));
                pattern.push(pat);
            }
            // `value` of a struct field is `n: Clone::clone(..)` here; the plain value of a tuple field
            let mut built = Gen::new(self.edition);
            construct(&mut built, name.as_deref(), data, &values);
            if !is_enum { return built; }
            let mut p = Gen::new(self.edition);
            construct(&mut p, name.as_deref(), data, &pattern);
            tg!(&mut arms; #append(&p) => #append(&built) ,);
            any = true;
        }
        let mut out = Gen::new(self.edition);
        if !any {
            tg!(&mut out; match * self { });
        } else {
            tg!(&mut out; match self { #append(&arms) });
        }
        out
    }
    fn expand_deriving_eq(&mut self, it: &Item, lo: usize) -> PResult<()> {
        let assertions = self.assert_ty_bounds(it, lo, "cmp::AssertParamIsEq");
        let mut g = Gen::new(self.edition);
        tg!(&mut g; #[inline] fn assert_fields_are_eq (& self) { #append(&assertions) });
        self.create_derived_impl(it, lo, "cmp::Eq", "cmp::Eq", self.is_packed(it), false, &g)
    }
    fn expand_deriving_partial_eq(&mut self, it: &Item, lo: usize) -> PResult<()> {
        if matches!(it.kind, ItemKind::Union(..)) {
            return Err(PErr { pos: it.lo, msg: "this trait cannot be derived for unions".into() });
        }
        self.create_derived_impl(it, lo, "marker::StructuralPartialEq", "cmp::PartialEq", false, false, &Gen::new(self.edition))?;
        let body = self.get_substructure_equality_expr(it);
        let mut g = Gen::new(self.edition);
        tg!(&mut g; #[inline] fn eq (& self , other : & Self) -> bool { #append(&body) });
        self.create_derived_impl(it, lo, "cmp::PartialEq", "cmp::PartialEq", self.is_packed(it), false, &g)
    }
    fn maybe_scalar(&self, mut t: TyId) -> bool {
        // rustc_ast ast.rs:2463-2469 Ty::peel_refs: references and raw pointers
        loop { match *self.ast.ty(t) { Ty::Ref(_, _, inner) | Ty::Ptr(_, inner) => t = inner, _ => break } }
        match *self.ast.ty(t) {
            Ty::Tuple(ts) => ts.is_empty(),
            Ty::Path(p) if p.no_qself() && p.segs.len() == 1 => {
                let seg = self.ast.segs(p.segs)[0];
                seg.args().is_none() && matches!(self.bytes(seg.name.lo, seg.name.hi), b"i8" | b"i16" | b"i32" | b"i64" | b"i128" | b"u8" | b"u16" | b"u32" | b"u64" | b"u128" | b"f16" | b"f32" | b"f64" | b"f128" | b"char" | b"bool")
            }
            _ => false,
        }
    }
    fn get_substructure_equality_expr(&self, it: &Item) -> Gen {
        let is_enum = matches!(it.kind, ItemKind::Enum(..));
        let packed = self.is_packed(it);
        let variants = self.deriving_fields(it);
        let mut arms = Gen::new(self.edition);
        for (name, data, fields) in &variants {
            let mut check = Gen::new(self.edition);
            let mut checks = 0;
            for scalar in [true, false] {
                for (i, f) in fields.iter().enumerate() {
                    if self.maybe_scalar(f.ty) != scalar { continue; }
                    let n = f.name.map_or(i.to_string(), |n| String::from_utf8_lossy(self.bytes(n.lo, n.hi)).into_owned());
                    if checks > 0 {
                        check.punct(T::AndAnd);
                    }
                    checks += 1;
                    if is_enum {
                        tg!(&mut check; * #word(&format!("__self_{i}")) == * #word(&format!("__other_{i}")));
                    } else if packed {
                        tg!(&mut check; ({ self . #word(&n) }) == ({ other . #word(&n) }));
                    } else {
                        tg!(&mut check; self . #word(&n) == other . #word(&n));
                    }
                }
            }
            if checks == 0 {
                check.word("true");
            }
            if !is_enum { return check; }
            let mut left = Vec::new(); let mut right = Vec::new();
            for (i, f) in fields.iter().enumerate() {
                let n = f.name.map(|n| String::from_utf8_lossy(self.bytes(n.lo, n.hi)).into_owned());
                for (side, out) in [("__self_", &mut left), ("__other_", &mut right)] {
                    let mut x = Gen::new(self.edition);
                    if let Some(n) = &n {
                        x.word(n);
                        x.punct(T::Colon);
                    }
                    x.word(&format!("{side}{i}"));
                    out.push(x);
                }
            }
            let (mut l, mut r) = (Gen::new(self.edition), Gen::new(self.edition));
            construct(&mut l, name.as_deref(), *data, &left);
            construct(&mut r, name.as_deref(), *data, &right);
            tg!(&mut arms; (#append(&l) , #append(&r)) => #append(&check) ,);
        }
        let mut out = Gen::new(self.edition);
        if variants.is_empty() {
            tg!(&mut out; match * self { });
            return out;
        }
        if variants.len() == 1 {
            tg!(&mut out; match (self , other) { #append(&arms) });
            return out;
        }
        let mut discr = Gen::new(self.edition);
        tg!(&mut discr; unsafe { #core() :: intrinsics_mem :: discriminant_value (self) == #core() :: intrinsics_mem :: discriminant_value (other) });
        if variants.iter().all(|v| v.2.is_empty()) { return discr; }
        tg!(&mut out; (#append(&discr)) && match (self , other) { #append(&arms) _ => true });
        out
    }
}

fn construct(g: &mut Gen, variant: Option<&str>, data: VariantData, fields: &[Gen]) {
    variant_path(g, variant);
    match data {
        VariantData::Unit => {}
        VariantData::Tuple(_) => {
            g.open();
            for (k, f) in fields.iter().enumerate() {
                if k > 0 {
                    g.punct(T::Comma);
                }
                g.append(f);
            }
            g.close();
        }
        VariantData::Struct(_) => {
            g.open_brace();
            for (k, f) in fields.iter().enumerate() {
                if k > 0 {
                    g.punct(T::Comma);
                }
                g.append(f);
            }
            g.close_brace();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::*;
    use crate::program::{DefKind, Program};

    fn program(source: &str, label: &str) -> Program {
        let dir = std::env::temp_dir().join(format!("relax-round9-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lib.rs");
        std::fs::write(&path, source).unwrap();
        let mut p = Program::new(&std::sync::Arc::new(crate::Session::default()));
        std::sync::Arc::make_mut(&mut p.macros).s2_fmt = true;
        let cfg = crate::cfg::CfgSet::host(&p.session, &[]);
        p.load_crate(label, path.to_str().unwrap(), 2021, cfg, vec![]).unwrap();
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(dir).unwrap();
        p
    }

    #[test]
    fn round9_specialization_rejected_during_collection() {
        let p = program("trait Tr { type Out; const C: u8; fn f(); } struct S; default impl Tr for S { default type Out = u8; default const C: u8 = 1; default fn f() {} }", "specialization");
        assert_eq!(p.errors.len(), 4, "{:?}", p.errors);
        // rustc_ast_passes/src/feature_gate.rs:174,382 @8d1a764: "specialization is experimental"
        for e in &p.errors { assert!(e.contains("E0658") && e.contains("specialization is experimental"), "{e}"); }
        let p = program("trait Tr { type Out; const C: u8 = 1; fn f() {} } struct S; impl Tr for S { type Out = u8; } #[cfg(any())] default impl Tr for u8 {}", "final");
        assert!(p.errors.is_empty(), "{:?}", p.errors);
    }

    #[test]
    fn round9_derive_emits_impls_and_distinguishes_clone_forms() {
        let p = program("#[derive(Clone, Copy, Eq, PartialEq)] struct S { value: u32 } #[derive(Clone, Copy)] struct G<T>(T); #[derive(Clone, Copy)] union U<T: Copy> { value: T }", "impls");
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.impls.len(), 11); // S: 6, G: 2, U: 3
        let mut clone_bodies = Vec::new();
        for d in &p.defs {
            if d.kind != DefKind::AssocFn || p.syms.str(d.name) != "clone" { continue; }
            let f = &p.files[d.file as usize];
            let ItemKind::Fn(_, Some(b), _) = f.ast.item(d.item).kind else { panic!("derive body absent") };
            let block = f.ast.block(b);
            clone_bodies.push(String::from_utf8_lossy(f.bytes(block.lo, block.hi)).replace(' ', ""));
        }
        assert_eq!(clone_bodies.len(), 3);
        assert!(clone_bodies[0].contains("AssertParamIsClone<u32>"));
        assert!(!clone_bodies[0].contains("AssertParamIsCopy"));
        assert!(clone_bodies[1].contains("Clone::clone(&self.0)"));
        assert!(clone_bodies[2].contains("AssertParamIsCopy<Self>"));
    }

    #[test]
    fn round9_derive_keeps_field_projection_binders() {
        let p = program("trait Tr<'a> { type Out; } #[derive(Clone)] struct G<T: for<'a> Tr<'a>> { f: for<'a> fn(T::Out) }", "binders");
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.impls.len(), 1);
        let imp = &p.impls[0]; let f = &p.files[imp.file as usize];
        let Some(ImplItem { generics, .. }) = f.ast.as_impl(&f.ast.item(imp.item).kind).copied() else { unreachable!() };
        let predicates = f.ast.list(generics.where_);
        assert_eq!(predicates.len(), 1);
        let WherePred::Bound { hrtb, ty, .. } = predicates[0] else { unreachable!() };
        assert_eq!(hrtb.len(), 1);
        let Ty::Path(path) = f.ast.ty(ty) else { unreachable!() };
        assert_eq!(path.segs.len(), 2);
    }
}
