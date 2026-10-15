//! Native `bitflags!` (bitflags 2.x): Relax generates the items the crate's macro_rules
//! produce (bitflags 2.10: lib.rs `bitflags!`, public.rs, internal.rs; no optional features)
//! straight from the call's tokens -- no macro_rules matching or recursive transcription.
//! The generated items are the same as the crate's expansion, so the API, Debug/Display output,
//! iteration and from_bits rules are the crate's own (its `Flags`, `iter`, `parser` code is
//! used as is, from the crate compiled by Relax). Used when the call resolves to the `bitflags`
//! crate's macro (builtins::override_rules); the real crate stays for rustc.
//!
//! One difference that is not observable through the API: the hidden internal type is a sibling
//! item named `__relax_bitflags_<Name>` instead of `InternalBitFlags` inside `const _: () = {..}`.

use crate::pm::{Delimiter, TokenStream, TokenTree};

struct Flag {
    /// all attributes as written (`#[..]` text)
    attrs: String,
    /// the expression-safe ones (`#[cfg ..]`), for FLAGS entries / all() / from_name
    cfg_attrs: String,
    /// `None` for `const _`
    name: Option<String>,
    value: String,
}

struct Def {
    outer: String,
    vis: String,
    /// `impl Name: T` form (the user declares the struct)
    is_impl: bool,
    name: String,
    ty: String,
    flags: Vec<Flag>,
}

fn text(trees: &[TokenTree]) -> String {
    crate::builtins::render(&TokenStream { trees: trees.to_vec() })
}

fn is_punct(t: &TokenTree, c: char) -> bool {
    matches!(t, TokenTree::Punct(p) if p.as_char() == c)
}

fn is_ident(t: &TokenTree, s: &str) -> bool {
    matches!(t, TokenTree::Ident(i) if i.to_string() == s)
}

/// `#[..]` attributes at `*i`: (all as text, the `cfg` ones as text, do the `cfg`s hold).
/// With the crate's cfg set known, `cfg` attributes are evaluated here and left out of the text.
fn attrs(ts: &[TokenTree], i: &mut usize, cfg: Option<&crate::cfg::CfgSet>) -> (String, String, bool) {
    let mut all = String::new();
    let mut cfgs = String::new();
    let mut on = true;
    while *i + 1 < ts.len() && is_punct(&ts[*i], '#') {
        if let TokenTree::Group(g) = &ts[*i + 1] {
            if g.delimiter() == Delimiter::Bracket {
                let a = format!("#{} ", text(&[ts[*i + 1].clone()]));
                let inner = g.stream().trees;
                if matches!(inner.first(), Some(TokenTree::Ident(x)) if x.to_string() == "cfg") {
                    match (cfg, inner.get(1)) {
                        (Some(c), Some(TokenTree::Group(args))) => {
                            if !crate::builtins::cfg_holds(&args.stream().trees, Some(c)) {
                                on = false;
                            }
                        }
                        _ => {
                            cfgs.push_str(&a);
                            all.push_str(&a);
                        }
                    }
                } else {
                    all.push_str(&a);
                }
                *i += 2;
                continue;
            }
        }
        break;
    }
    (all, cfgs, on)
}

fn parse(input: &TokenStream, cfg: Option<&crate::cfg::CfgSet>) -> Result<Vec<Def>, String> {
    let ts = &input.trees;
    let mut defs = Vec::new();
    let mut i = 0;
    while i < ts.len() {
        let (outer, _, _) = attrs(ts, &mut i, None);
        let mut vis = String::new();
        let is_impl;
        if i < ts.len() && is_ident(&ts[i], "impl") {
            is_impl = true;
            i += 1;
        } else {
            // vis: `pub`, `pub(..)`, or nothing
            if i < ts.len() && is_ident(&ts[i], "pub") {
                vis.push_str("pub");
                i += 1;
                if let Some(TokenTree::Group(g)) = ts.get(i) {
                    if g.delimiter() == Delimiter::Parenthesis {
                        vis.push_str(&text(&[ts[i].clone()]));
                        i += 1;
                    }
                }
            } else if i < ts.len() && is_ident(&ts[i], "crate") {
                vis.push_str("pub");
                i += 1;
            }
            if i >= ts.len() || !is_ident(&ts[i], "struct") {
                return Err("expected `struct Name: Type { .. }` or `impl Name: Type { .. }`".to_string());
            }
            is_impl = false;
            i += 1;
        }
        let name = match ts.get(i) {
            Some(TokenTree::Ident(n)) => n.to_string(),
            _ => return Err("expected the flags type name".to_string()),
        };
        i += 1;
        if i >= ts.len() || !is_punct(&ts[i], ':') {
            return Err(format!("expected `: Type` after `{}`", name));
        }
        i += 1;
        let ty_lo = i;
        while i < ts.len() && !matches!(&ts[i], TokenTree::Group(g) if g.delimiter() == Delimiter::Brace) {
            i += 1;
        }
        if i >= ts.len() {
            return Err(format!("expected `{{ const .. }}` after `{}: ..`", name));
        }
        let ty = text(&ts[ty_lo..i]);
        let body = match &ts[i] {
            TokenTree::Group(g) => g.stream().trees,
            _ => unreachable!(),
        };
        i += 1;
        let mut flags = Vec::new();
        let mut j = 0;
        while j < body.len() {
            let (fa, fc, on) = attrs(&body, &mut j, cfg);
            if j >= body.len() {
                break;
            }
            if !is_ident(&body[j], "const") {
                return Err("expected `const NAME = value;`".to_string());
            }
            j += 1;
            let fname = match body.get(j) {
                Some(TokenTree::Ident(n)) => {
                    let s = n.to_string();
                    if s == "_" {
                        None
                    } else {
                        Some(s)
                    }
                }
                _ => return Err("expected a flag name or `_`".to_string()),
            };
            j += 1;
            if j >= body.len() || !is_punct(&body[j], '=') {
                return Err("expected `=` in a flag".to_string());
            }
            j += 1;
            let vlo = j;
            while j < body.len() && !is_punct(&body[j], ';') {
                j += 1;
            }
            let value = text(&body[vlo..j]);
            j += 1;
            if on {
                flags.push(Flag { attrs: fa, cfg_attrs: fc, name: fname, value });
            }
        }
        defs.push(Def { outer, vis, is_impl, name, ty, flags });
    }
    Ok(defs)
}

const ALLOW: &str = "#[allow(dead_code, deprecated, unused_doc_comments, unused_attributes, unused_mut, unused_imports, non_upper_case_globals)]";

/// `__impl_bitflags!`: the 20 inherent methods with the given bodies.
fn impl_bitflags(o: &mut String, outer: &str, target: &str, t: &str, b: &[String; 20]) {
    let sigs = [
        "pub const fn empty() -> Self",
        "pub const fn all() -> Self",
        "pub const fn bits(&self) -> T",
        "pub const fn from_bits(bits: T) -> ::core::option::Option<Self>",
        "pub const fn from_bits_truncate(bits: T) -> Self",
        "pub const fn from_bits_retain(bits: T) -> Self",
        "pub fn from_name(name: &str) -> ::core::option::Option<Self>",
        "pub const fn is_empty(&self) -> bool",
        "pub const fn is_all(&self) -> bool",
        "pub const fn intersects(&self, other: Self) -> bool",
        "pub const fn contains(&self, other: Self) -> bool",
        "pub fn insert(&mut self, other: Self)",
        "pub fn remove(&mut self, other: Self)",
        "pub fn toggle(&mut self, other: Self)",
        "pub fn set(&mut self, other: Self, value: bool)",
        "pub const fn intersection(self, other: Self) -> Self",
        "pub const fn union(self, other: Self) -> Self",
        "pub const fn difference(self, other: Self) -> Self",
        "pub const fn symmetric_difference(self, other: Self) -> Self",
        "pub const fn complement(self) -> Self",
    ];
    o.push_str(&format!("#[allow(dead_code, deprecated, unused_attributes)] {} impl {} {{\n", outer, target));
    for k in 0..20 {
        o.push_str(&format!("#[inline] {} {{ {} }}\n", sigs[k].replace("-> T", &format!("-> {}", t)).replace("bits: T", &format!("bits: {}", t)), b[k]));
    }
    o.push_str("}\n");
}

/// `__impl_public_bitflags!` bodies (the methods over `.0: T`).
fn public_bodies(c: &str, t: &str, public: &str, flags: &[Flag]) -> [String; 20] {
    let mut all = format!("let mut truncated = <{} as {}::Bits>::EMPTY; let mut i = 0; ", t, c);
    for f in flags {
        all.push_str(&format!("{} {{ {{ let flag = <{} as {}::Flags>::FLAGS[i].value().bits(); truncated = truncated | flag; i += 1; }} }} ", f.cfg_attrs, public, c));
    }
    all.push_str("let _ = i; Self(truncated)");
    let mut from_name = String::new();
    for f in flags {
        if let Some(n) = &f.name {
            from_name.push_str(&format!(
                "{} {{ if name == {:?} {{ return ::core::option::Option::Some(Self({}::{}.bits())); }} }} ",
                f.cfg_attrs, n, public, n
            ));
        }
    }
    from_name.push_str("let _ = name; ::core::option::Option::None");
    [
        format!("Self(<{} as {}::Bits>::EMPTY)", t, c),
        all,
        "self.0".to_string(),
        "let truncated = Self::from_bits_truncate(bits).0; if truncated == bits { ::core::option::Option::Some(Self(bits)) } else { ::core::option::Option::None }".to_string(),
        "Self(bits & Self::all().0)".to_string(),
        "Self(bits)".to_string(),
        from_name,
        format!("self.0 == <{} as {}::Bits>::EMPTY", t, c),
        "Self::all().0 | self.0 == self.0".to_string(),
        format!("self.0 & other.0 != <{} as {}::Bits>::EMPTY", t, c),
        "self.0 & other.0 == other.0".to_string(),
        "*self = Self(self.0).union(other);".to_string(),
        "*self = Self(self.0).difference(other);".to_string(),
        "*self = Self(self.0).symmetric_difference(other);".to_string(),
        "if value { self.insert(other); } else { self.remove(other); }".to_string(),
        "Self(self.0 & other.0)".to_string(),
        "Self(self.0 | other.0)".to_string(),
        "Self(self.0 & !other.0)".to_string(),
        "Self(self.0 ^ other.0)".to_string(),
        "Self::from_bits_truncate(!self.0)".to_string(),
    ]
}

/// `__impl_public_bitflags_forward!` bodies (the public type over its internal one).
fn forward_bodies(internal: &str) -> [String; 20] {
    let some = "::core::option::Option::Some";
    let none = "::core::option::Option::None";
    [
        format!("Self({}::empty())", internal),
        format!("Self({}::all())", internal),
        "self.0.bits()".to_string(),
        format!("match {}::from_bits(bits) {{ {s}(bits) => {s}(Self(bits)), {n} => {n}, }}", internal, s = some, n = none),
        format!("Self({}::from_bits_truncate(bits))", internal),
        format!("Self({}::from_bits_retain(bits))", internal),
        format!("match {}::from_name(name) {{ {s}(bits) => {s}(Self(bits)), {n} => {n}, }}", internal, s = some, n = none),
        "self.0.is_empty()".to_string(),
        "self.0.is_all()".to_string(),
        "self.0.intersects(other.0)".to_string(),
        "self.0.contains(other.0)".to_string(),
        "self.0.insert(other.0)".to_string(),
        "self.0.remove(other.0)".to_string(),
        "self.0.toggle(other.0)".to_string(),
        "self.0.set(other.0, value)".to_string(),
        "Self(self.0.intersection(other.0))".to_string(),
        "Self(self.0.union(other.0))".to_string(),
        "Self(self.0.difference(other.0))".to_string(),
        "Self(self.0.symmetric_difference(other.0))".to_string(),
        "Self(self.0.complement())".to_string(),
    ]
}

/// `__impl_public_bitflags_ops!`
fn ops(o: &mut String, outer: &str, p: &str) {
    let fmt = |o: &mut String, tr: &str| {
        o.push_str(&format!(
            "{outer} impl ::core::fmt::{tr} for {p} {{ fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {{ let inner = self.0; ::core::fmt::{tr}::fmt(&inner, f) }} }}\n"
        ));
    };
    fmt(o, "Binary");
    fmt(o, "Octal");
    fmt(o, "LowerHex");
    fmt(o, "UpperHex");
    o.push_str(&format!(
        "{outer} impl ::core::ops::BitOr for {p} {{ type Output = Self; #[inline] fn bitor(self, other: {p}) -> Self {{ self.union(other) }} }}
{outer} impl ::core::ops::BitOrAssign for {p} {{ #[inline] fn bitor_assign(&mut self, other: Self) {{ self.insert(other); }} }}
{outer} impl ::core::ops::BitXor for {p} {{ type Output = Self; #[inline] fn bitxor(self, other: Self) -> Self {{ self.symmetric_difference(other) }} }}
{outer} impl ::core::ops::BitXorAssign for {p} {{ #[inline] fn bitxor_assign(&mut self, other: Self) {{ self.toggle(other); }} }}
{outer} impl ::core::ops::BitAnd for {p} {{ type Output = Self; #[inline] fn bitand(self, other: Self) -> Self {{ self.intersection(other) }} }}
{outer} impl ::core::ops::BitAndAssign for {p} {{ #[inline] fn bitand_assign(&mut self, other: Self) {{ *self = Self::from_bits_retain(self.bits()).intersection(other); }} }}
{outer} impl ::core::ops::Sub for {p} {{ type Output = Self; #[inline] fn sub(self, other: Self) -> Self {{ self.difference(other) }} }}
{outer} impl ::core::ops::SubAssign for {p} {{ #[inline] fn sub_assign(&mut self, other: Self) {{ self.remove(other); }} }}
{outer} impl ::core::ops::Not for {p} {{ type Output = Self; #[inline] fn not(self) -> Self {{ self.complement() }} }}
{outer} impl ::core::iter::Extend<{p}> for {p} {{ fn extend<T: ::core::iter::IntoIterator<Item = Self>>(&mut self, iterator: T) {{ for item in iterator {{ self.insert(item) }} }} }}
{outer} impl ::core::iter::FromIterator<{p}> for {p} {{ fn from_iter<T: ::core::iter::IntoIterator<Item = Self>>(iterator: T) -> Self {{ use ::core::iter::Extend; let mut result = Self::empty(); result.extend(iterator); result }} }}
"
    ));
}

/// `__impl_public_bitflags_iter!`
fn iter(o: &mut String, outer: &str, c: &str, target: &str, public: &str) {
    o.push_str(&format!(
        "{outer} impl {target} {{
#[inline] pub const fn iter(&self) -> {c}::iter::Iter<{public}> {{ {c}::iter::Iter::__private_const_new(<{public} as {c}::Flags>::FLAGS, {public}::from_bits_retain(self.bits()), {public}::from_bits_retain(self.bits())) }}
#[inline] pub const fn iter_names(&self) -> {c}::iter::IterNames<{public}> {{ {c}::iter::IterNames::__private_const_new(<{public} as {c}::Flags>::FLAGS, {public}::from_bits_retain(self.bits()), {public}::from_bits_retain(self.bits())) }}
}}
{outer} impl ::core::iter::IntoIterator for {target} {{ type Item = {public}; type IntoIter = {c}::iter::Iter<{public}>; fn into_iter(self) -> Self::IntoIter {{ self.iter() }} }}
"
    ));
}

/// `__impl_public_bitflags_consts!`
fn consts(o: &mut String, outer: &str, c: &str, p: &str, t: &str, flags: &[Flag]) {
    o.push_str(&format!("{} impl {} {{\n", outer, p));
    for f in flags {
        if let Some(n) = &f.name {
            o.push_str(&format!("{} #[allow(deprecated, non_upper_case_globals)] pub const {}: Self = Self::from_bits_retain({});\n", f.attrs, n, f.value));
        }
    }
    o.push_str("}\n");
    o.push_str(&format!("{} impl {}::Flags for {} {{\nconst FLAGS: &'static [{}::Flag<{}>] = &[\n", outer, c, p, c, p));
    for f in flags {
        match &f.name {
            Some(n) => o.push_str(&format!("{} {{ #[allow(deprecated, non_upper_case_globals)] {}::Flag::new({:?}, {}::{}) }},\n", f.cfg_attrs, c, n, p, n)),
            None => o.push_str(&format!("{} {{ #[allow(deprecated, non_upper_case_globals)] {}::Flag::new(\"\", {}::from_bits_retain({})) }},\n", f.cfg_attrs, c, p, f.value)),
        }
    }
    o.push_str(&format!(
        "];\ntype Bits = {t};\nfn bits(&self) -> {t} {{ {p}::bits(self) }}\nfn from_bits_retain(bits: {t}) -> {p} {{ {p}::from_bits_retain(bits) }}\n}}\n"
    ));
}

/// The expansion of `bitflags! { .. }` (text, parsed in place). `c` is the path of the bitflags
/// crate (`crate` inside it, else `::__relax_crate_bitflags`).
pub fn expand(input: &TokenStream, c: &str, cfg: Option<&crate::cfg::CfgSet>) -> Result<String, String> {
    let defs = parse(input, cfg)?;
    let mut o = String::new();
    for d in &defs {
        let (p, t, outer) = (&d.name, &d.ty, d.outer.as_str());
        if d.is_impl {
            consts(&mut o, "", c, p, t, &d.flags);
            o.push_str(&format!("{} const _: () = {{\n", ALLOW));
            impl_bitflags(&mut o, outer, p, t, &public_bodies(c, t, p, &d.flags));
            ops(&mut o, "", p);
            iter(&mut o, "", c, p, p);
            o.push_str("};\n");
            continue;
        }
        let internal = format!("__relax_bitflags_{}", p);
        // __declare_public_bitflags
        o.push_str(&format!("{} {} struct {}({});\n", outer, d.vis, p, internal));
        consts(&mut o, "", c, p, t, &d.flags);
        // __declare_internal_bitflags + __impl_internal_bitflags
        o.push_str(&format!("{} #[doc(hidden)] #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)] #[repr(transparent)] {} struct {}({});\n", ALLOW, d.vis, internal, t));
        o.push_str(&format!(
            "impl {c}::__private::PublicFlags for {p} {{ type Primitive = {t}; type Internal = {i}; }}
impl ::core::default::Default for {i} {{ #[inline] fn default() -> Self {{ {i}::empty() }} }}
impl ::core::fmt::Debug for {i} {{ fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {{ if self.is_empty() {{ ::core::write!(f, \"{{:#x}}\", <{t} as {c}::Bits>::EMPTY) }} else {{ ::core::fmt::Display::fmt(self, f) }} }} }}
impl ::core::fmt::Display for {i} {{ fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {{ {c}::parser::to_writer(&{p}(*self), f) }} }}
impl ::core::str::FromStr for {i} {{ type Err = {c}::parser::ParseError; fn from_str(s: &str) -> ::core::result::Result<Self, Self::Err> {{ {c}::parser::from_str::<{p}>(s).map(|flags| flags.0) }} }}
impl ::core::convert::AsRef<{t}> for {i} {{ fn as_ref(&self) -> &{t} {{ &self.0 }} }}
impl ::core::convert::From<{t}> for {i} {{ fn from(bits: {t}) -> Self {{ Self::from_bits_retain(bits) }} }}
",
            c = c,
            p = p,
            t = t,
            i = internal
        ));
        impl_bitflags(&mut o, "", &internal, t, &public_bodies(c, t, p, &d.flags));
        ops(&mut o, "", &internal);
        iter(&mut o, "", c, &internal, p);
        o.push_str(&format!("impl {} {{ #[inline] pub fn bits_mut(&mut self) -> &mut {} {{ &mut self.0 }} }}\n", internal, t));
        // public type: forward, ops, iter
        impl_bitflags(&mut o, "", p, t, &forward_bodies(&internal));
        ops(&mut o, "", p);
        iter(&mut o, "", c, p, p);
    }
    Ok(o)
}
