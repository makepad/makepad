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
use crate::tg;
use crate::tokbuf::TokBuf;

struct Flag {
    /// all attributes as written (`#[..]`)
    attrs: TokBuf,
    /// the expression-safe ones (`#[cfg ..]`), for FLAGS entries / all() / from_name
    cfg_attrs: TokBuf,
    /// `None` for `const _`
    name: Option<String>,
    value: TokBuf,
}

struct Def {
    outer: TokBuf,
    vis: TokBuf,
    /// `impl Name: T` form (the user declares the struct)
    is_impl: bool,
    name: String,
    ty: TokBuf,
    flags: Vec<Flag>,
}

fn toks(trees: &[TokenTree], edition: u16) -> Result<TokBuf, String> {
    crate::builtins::trees_to_buf(trees, edition)
}

fn is_punct(t: &TokenTree, c: char) -> bool {
    matches!(t, TokenTree::Punct(p) if p.as_char() == c)
}

fn is_ident(t: &TokenTree, s: &str) -> bool {
    matches!(t, TokenTree::Ident(i) if i.to_string() == s)
}

/// `#[..]` attributes at `*i`: (all, the `cfg` ones, do the `cfg`s hold).
/// With the crate's cfg set known, `cfg` attributes are evaluated here and left out.
fn attrs(ts: &[TokenTree], i: &mut usize, cfg: Option<&crate::cfg::CfgSet>, edition: u16) -> Result<(TokBuf, TokBuf, bool), String> {
    let mut all = TokBuf::new(edition);
    let mut cfgs = TokBuf::new(edition);
    let mut on = true;
    while *i + 1 < ts.len() && is_punct(&ts[*i], '#') {
        if let TokenTree::Group(g) = &ts[*i + 1] {
            if g.delimiter() == Delimiter::Bracket {
                let a = toks(&ts[*i..*i + 2], edition)?;
                let inner = g.stream().trees;
                if matches!(inner.first(), Some(TokenTree::Ident(x)) if x.to_string() == "cfg") {
                    match (cfg, inner.get(1)) {
                        (Some(c), Some(TokenTree::Group(args))) => {
                            if !crate::builtins::cfg_holds(&args.stream().trees, Some(c)) {
                                on = false;
                            }
                        }
                        _ => {
                            cfgs.append(&a);
                            all.append(&a);
                        }
                    }
                } else {
                    all.append(&a);
                }
                *i += 2;
                continue;
            }
        }
        break;
    }
    Ok((all, cfgs, on))
}

fn parse(input: &TokenStream, cfg: Option<&crate::cfg::CfgSet>, edition: u16) -> Result<Vec<Def>, String> {
    let ts = &input.trees;
    let mut defs = Vec::new();
    let mut i = 0;
    while i < ts.len() {
        let (outer, _, _) = attrs(ts, &mut i, None, edition)?;
        let mut vis = TokBuf::new(edition);
        let is_impl;
        if i < ts.len() && is_ident(&ts[i], "impl") {
            is_impl = true;
            i += 1;
        } else {
            // vis: `pub`, `pub(..)`, or nothing
            if i < ts.len() && is_ident(&ts[i], "pub") {
                vis.word("pub");
                i += 1;
                if let Some(TokenTree::Group(g)) = ts.get(i) {
                    if g.delimiter() == Delimiter::Parenthesis {
                        vis.append(&toks(&ts[i..i + 1], edition)?);
                        i += 1;
                    }
                }
            } else if i < ts.len() && is_ident(&ts[i], "crate") {
                vis.word("pub");
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
        let ty = toks(&ts[ty_lo..i], edition)?;
        let body = match &ts[i] {
            TokenTree::Group(g) => g.stream().trees,
            _ => unreachable!(),
        };
        i += 1;
        let mut flags = Vec::new();
        let mut j = 0;
        while j < body.len() {
            let (fa, fc, on) = attrs(&body, &mut j, cfg, edition)?;
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
            let value = toks(&body[vlo..j], edition)?;
            j += 1;
            if on {
                flags.push(Flag { attrs: fa, cfg_attrs: fc, name: fname, value });
            }
        }
        defs.push(Def { outer, vis, is_impl, name, ty, flags });
    }
    Ok(defs)
}

fn allow(g: &mut TokBuf) {
    tg!(g; #[allow(dead_code, deprecated, unused_doc_comments, unused_attributes, unused_mut, unused_imports, non_upper_case_globals)]);
}

/// `::core::option::Option::Some` / `::None`
fn some(g: &mut TokBuf) {
    tg!(g; :: core :: option :: Option :: Some);
}
fn none(g: &mut TokBuf) {
    tg!(g; :: core :: option :: Option :: None);
}

/// The signature of inherent method `k` of `__impl_bitflags!` (`t`: the bits type).
fn sig(g: &mut TokBuf, k: usize, t: &TokBuf) {
    match k {
        0 => tg!(g; pub const fn empty () -> Self),
        1 => tg!(g; pub const fn all () -> Self),
        2 => tg!(g; pub const fn bits (& self) -> #append(t)),
        3 => tg!(g; pub const fn from_bits (bits : #append(t)) -> :: core :: option :: Option < Self >),
        4 => tg!(g; pub const fn from_bits_truncate (bits : #append(t)) -> Self),
        5 => tg!(g; pub const fn from_bits_retain (bits : #append(t)) -> Self),
        6 => tg!(g; pub fn from_name (name : & str) -> :: core :: option :: Option < Self >),
        7 => tg!(g; pub const fn is_empty (& self) -> bool),
        8 => tg!(g; pub const fn is_all (& self) -> bool),
        9 => tg!(g; pub const fn intersects (& self , other : Self) -> bool),
        10 => tg!(g; pub const fn contains (& self , other : Self) -> bool),
        11 => tg!(g; pub fn insert (& mut self , other : Self)),
        12 => tg!(g; pub fn remove (& mut self , other : Self)),
        13 => tg!(g; pub fn toggle (& mut self , other : Self)),
        14 => tg!(g; pub fn set (& mut self , other : Self , value : bool)),
        15 => tg!(g; pub const fn intersection (self , other : Self) -> Self),
        16 => tg!(g; pub const fn union (self , other : Self) -> Self),
        17 => tg!(g; pub const fn difference (self , other : Self) -> Self),
        18 => tg!(g; pub const fn symmetric_difference (self , other : Self) -> Self),
        _ => tg!(g; pub const fn complement (self) -> Self),
    }
}

/// `__impl_bitflags!`: the 20 inherent methods with the given bodies.
fn impl_bitflags(o: &mut TokBuf, outer: &TokBuf, target: &str, t: &TokBuf, b: &[TokBuf; 20]) {
    tg!(o; #[allow(dead_code, deprecated, unused_attributes)] #append(outer) impl #word(target) #open_brace());
    for k in 0..20 {
        tg!(o; #[inline]);
        sig(o, k, t);
        tg!(o; { #append(&b[k]) });
    }
    o.close_brace();
}

/// `__impl_public_bitflags!` bodies (the methods over `.0: T`).
fn public_bodies(c: &TokBuf, t: &TokBuf, public: &str, flags: &[Flag], edition: u16) -> [TokBuf; 20] {
    let nb = || TokBuf::new(edition);
    let empty = |g: &mut TokBuf| tg!(g; < #append(t) as #append(c) :: Bits > :: EMPTY);
    let mut all = nb();
    tg!(&mut all; let mut truncated =);
    empty(&mut all);
    tg!(&mut all; ; let mut i = 0 ;);
    for f in flags {
        tg!(&mut all; #append(&f.cfg_attrs) { { let flag = < #word(public) as #append(c) :: Flags > :: FLAGS [i] . value () . bits () ; truncated = truncated | flag ; i += 1 ; } });
    }
    tg!(&mut all; let _ = i ; Self (truncated));
    let mut from_name = nb();
    for f in flags {
        if let Some(n) = &f.name {
            tg!(&mut from_name; #append(&f.cfg_attrs));
            from_name.open_brace();
            tg!(&mut from_name; if name == #str_lit(n));
            from_name.open_brace();
            tg!(&mut from_name; return);
            some(&mut from_name);
            tg!(&mut from_name; (Self (#word(public) :: #word(n) . bits ())) ;);
            from_name.close_brace();
            from_name.close_brace();
        }
    }
    tg!(&mut from_name; let _ = name ;);
    none(&mut from_name);
    let mut v: Vec<TokBuf> = Vec::new();
    let mut b0 = nb();
    tg!(&mut b0; Self #open());
    empty(&mut b0);
    b0.close();
    v.push(b0);
    v.push(all);
    let mut push = |f: &dyn Fn(&mut TokBuf)| {
        let mut g = nb();
        f(&mut g);
        v.push(g);
    };
    push(&|g| tg!(g; self . 0));
    push(&|g| {
        tg!(g; let truncated = Self :: from_bits_truncate (bits) . 0 ; if truncated == bits);
        g.open_brace();
        some(g);
        tg!(g; (Self (bits)));
        g.close_brace();
        tg!(g; else);
        g.open_brace();
        none(g);
        g.close_brace();
    });
    push(&|g| tg!(g; Self (bits & Self :: all () . 0)));
    push(&|g| tg!(g; Self (bits)));
    let from_name = from_name;
    push(&|g| g.append(&from_name));
    push(&|g| {
        tg!(g; self . 0 ==);
        empty(g);
    });
    push(&|g| tg!(g; Self :: all () . 0 | self . 0 == self . 0));
    push(&|g| {
        tg!(g; self . 0 & other . 0 !=);
        empty(g);
    });
    push(&|g| tg!(g; self . 0 & other . 0 == other . 0));
    push(&|g| tg!(g; * self = Self (self . 0) . union (other) ;));
    push(&|g| tg!(g; * self = Self (self . 0) . difference (other) ;));
    push(&|g| tg!(g; * self = Self (self . 0) . symmetric_difference (other) ;));
    push(&|g| tg!(g; if value { self . insert (other) ; } else { self . remove (other) ; }));
    push(&|g| tg!(g; Self (self . 0 & other . 0)));
    push(&|g| tg!(g; Self (self . 0 | other . 0)));
    push(&|g| tg!(g; Self (self . 0 & ! other . 0)));
    push(&|g| tg!(g; Self (self . 0 ^ other . 0)));
    push(&|g| tg!(g; Self :: from_bits_truncate (! self . 0)));
    v.try_into().ok().expect("20 bitflags bodies")
}

/// `__impl_public_bitflags_forward!` bodies (the public type over its internal one).
fn forward_bodies(internal: &str, edition: u16) -> [TokBuf; 20] {
    let mut v: Vec<TokBuf> = Vec::new();
    let mut push = |f: &dyn Fn(&mut TokBuf)| {
        let mut g = TokBuf::new(edition);
        f(&mut g);
        v.push(g);
    };
    // `match INTERNAL::<f>(<arg>) { Some(bits) => Some(Self(bits)), None => None, }`
    let wrap = |g: &mut TokBuf, f: &str, arg: &str| {
        tg!(g; match #word(internal) :: #word(f) (#word(arg)));
        g.open_brace();
        some(g);
        tg!(g; (bits) =>);
        some(g);
        tg!(g; (Self (bits)) ,);
        none(g);
        tg!(g; =>);
        none(g);
        tg!(g; ,);
        g.close_brace();
    };
    push(&|g| tg!(g; Self (#word(internal) :: empty ())));
    push(&|g| tg!(g; Self (#word(internal) :: all ())));
    push(&|g| tg!(g; self . 0 . bits ()));
    push(&|g| wrap(g, "from_bits", "bits"));
    push(&|g| tg!(g; Self (#word(internal) :: from_bits_truncate (bits))));
    push(&|g| tg!(g; Self (#word(internal) :: from_bits_retain (bits))));
    push(&|g| wrap(g, "from_name", "name"));
    push(&|g| tg!(g; self . 0 . is_empty ()));
    push(&|g| tg!(g; self . 0 . is_all ()));
    push(&|g| tg!(g; self . 0 . intersects (other . 0)));
    push(&|g| tg!(g; self . 0 . contains (other . 0)));
    push(&|g| tg!(g; self . 0 . insert (other . 0)));
    push(&|g| tg!(g; self . 0 . remove (other . 0)));
    push(&|g| tg!(g; self . 0 . toggle (other . 0)));
    push(&|g| tg!(g; self . 0 . set (other . 0 , value)));
    push(&|g| tg!(g; Self (self . 0 . intersection (other . 0))));
    push(&|g| tg!(g; Self (self . 0 . union (other . 0))));
    push(&|g| tg!(g; Self (self . 0 . difference (other . 0))));
    push(&|g| tg!(g; Self (self . 0 . symmetric_difference (other . 0))));
    push(&|g| tg!(g; Self (self . 0 . complement ())));
    v.try_into().ok().expect("20 bitflags bodies")
}

/// `__impl_public_bitflags_ops!`
fn ops(o: &mut TokBuf, p: &str) {
    for tr in ["Binary", "Octal", "LowerHex", "UpperHex"] {
        tg!(o; impl :: core :: fmt :: #word(tr) for #word(p) {
            fn fmt (& self , f : & mut :: core :: fmt :: Formatter) -> :: core :: fmt :: Result {
                let inner = self . 0 ; :: core :: fmt :: #word(tr) :: fmt (& inner , f)
            }
        });
    }
    tg!(o;
        impl :: core :: ops :: BitOr for #word(p) { type Output = Self ; #[inline] fn bitor (self , other : #word(p)) -> Self { self . union (other) } }
        impl :: core :: ops :: BitOrAssign for #word(p) { #[inline] fn bitor_assign (& mut self , other : Self) { self . insert (other) ; } }
        impl :: core :: ops :: BitXor for #word(p) { type Output = Self ; #[inline] fn bitxor (self , other : Self) -> Self { self . symmetric_difference (other) } }
        impl :: core :: ops :: BitXorAssign for #word(p) { #[inline] fn bitxor_assign (& mut self , other : Self) { self . toggle (other) ; } }
        impl :: core :: ops :: BitAnd for #word(p) { type Output = Self ; #[inline] fn bitand (self , other : Self) -> Self { self . intersection (other) } }
        impl :: core :: ops :: BitAndAssign for #word(p) { #[inline] fn bitand_assign (& mut self , other : Self) { * self = Self :: from_bits_retain (self . bits ()) . intersection (other) ; } }
        impl :: core :: ops :: Sub for #word(p) { type Output = Self ; #[inline] fn sub (self , other : Self) -> Self { self . difference (other) } }
        impl :: core :: ops :: SubAssign for #word(p) { #[inline] fn sub_assign (& mut self , other : Self) { self . remove (other) ; } }
        impl :: core :: ops :: Not for #word(p) { type Output = Self ; #[inline] fn not (self) -> Self { self . complement () } }
        impl :: core :: iter :: Extend < #word(p) > for #word(p) { fn extend < T : :: core :: iter :: IntoIterator < Item = Self > > (& mut self , iterator : T) { for item in iterator { self . insert (item) } } }
        impl :: core :: iter :: FromIterator < #word(p) > for #word(p) { fn from_iter < T : :: core :: iter :: IntoIterator < Item = Self > > (iterator : T) -> Self { use :: core :: iter :: Extend ; let mut result = Self :: empty () ; result . extend (iterator) ; result } }
    );
}

/// `__impl_public_bitflags_iter!`
fn iter(o: &mut TokBuf, c: &TokBuf, target: &str, public: &str) {
    tg!(o; impl #word(target) {
        #[inline] pub const fn iter (& self) -> #append(c) :: iter :: Iter < #word(public) > {
            #append(c) :: iter :: Iter :: __private_const_new (< #word(public) as #append(c) :: Flags > :: FLAGS , #word(public) :: from_bits_retain (self . bits ()) , #word(public) :: from_bits_retain (self . bits ()))
        }
        #[inline] pub const fn iter_names (& self) -> #append(c) :: iter :: IterNames < #word(public) > {
            #append(c) :: iter :: IterNames :: __private_const_new (< #word(public) as #append(c) :: Flags > :: FLAGS , #word(public) :: from_bits_retain (self . bits ()) , #word(public) :: from_bits_retain (self . bits ()))
        }
    });
    tg!(o; impl :: core :: iter :: IntoIterator for #word(target) {
        type Item = #word(public) ; type IntoIter = #append(c) :: iter :: Iter < #word(public) > ;
        fn into_iter (self) -> Self :: IntoIter { self . iter () }
    });
}

/// `__impl_public_bitflags_consts!`
fn consts(o: &mut TokBuf, c: &TokBuf, p: &str, t: &TokBuf, flags: &[Flag]) {
    tg!(o; impl #word(p) #open_brace());
    for f in flags {
        if let Some(n) = &f.name {
            tg!(o; #append(&f.attrs) #[allow(deprecated, non_upper_case_globals)] pub const #word(n) : Self = Self :: from_bits_retain (#append(&f.value)) ;);
        }
    }
    o.close_brace();
    tg!(o; impl #append(c) :: Flags for #word(p) #open_brace());
    tg!(o; const FLAGS : & 'static [#append(c) :: Flag < #word(p) >] = & #open_bracket());
    for f in flags {
        match &f.name {
            Some(n) => tg!(o; #append(&f.cfg_attrs) { #[allow(deprecated, non_upper_case_globals)] #append(c) :: Flag :: new (#str_lit(n) , #word(p) :: #word(n)) } ,),
            None => tg!(o; #append(&f.cfg_attrs) { #[allow(deprecated, non_upper_case_globals)] #append(c) :: Flag :: new ("" , #word(p) :: from_bits_retain (#append(&f.value))) } ,),
        }
    }
    tg!(o; #close_bracket() ; type Bits = #append(t) ;
        fn bits (& self) -> #append(t) { #word(p) :: bits (self) }
        fn from_bits_retain (bits : #append(t)) -> #word(p) { #word(p) :: from_bits_retain (bits) });
    o.close_brace();
}

/// The expansion of `bitflags! { .. }` (tokens, parsed in place). `c` is the path of the bitflags
/// crate (`crate` inside it, else `::__relax_crate_bitflags`).
pub fn expand(input: &TokenStream, local: bool, cfg: Option<&crate::cfg::CfgSet>, edition: u16) -> Result<TokBuf, String> {
    let defs = parse(input, cfg, edition)?;
    let mut c = TokBuf::new(edition);
    if local {
        c.word("crate");
    } else {
        c.punct(crate::lexer::T::PathSep);
        c.word("__relax_crate_bitflags");
    }
    let c = &c;
    let none_outer = TokBuf::new(edition);
    let mut o = TokBuf::new(edition);
    for d in &defs {
        let (p, t) = (d.name.as_str(), &d.ty);
        if d.is_impl {
            consts(&mut o, c, p, t, &d.flags);
            allow(&mut o);
            tg!(&mut o; const _ : () = #open_brace());
            impl_bitflags(&mut o, &d.outer, p, t, &public_bodies(c, t, p, &d.flags, edition));
            ops(&mut o, p);
            iter(&mut o, c, p, p);
            tg!(&mut o; #close_brace() ;);
            continue;
        }
        let internal = format!("__relax_bitflags_{}", p);
        let i = internal.as_str();
        // __declare_public_bitflags
        tg!(&mut o; #append(&d.outer) #append(&d.vis) struct #word(p) (#word(i)) ;);
        consts(&mut o, c, p, t, &d.flags);
        // __declare_internal_bitflags + __impl_internal_bitflags
        allow(&mut o);
        tg!(&mut o; #[doc(hidden)] #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)] #[repr(transparent)] #append(&d.vis) struct #word(i) (#append(t)) ;);
        tg!(&mut o;
            impl #append(c) :: __private :: PublicFlags for #word(p) { type Primitive = #append(t) ; type Internal = #word(i) ; }
            impl :: core :: default :: Default for #word(i) { #[inline] fn default () -> Self { #word(i) :: empty () } }
            impl :: core :: fmt :: Debug for #word(i) {
                fn fmt (& self , f : & mut :: core :: fmt :: Formatter < '_ >) -> :: core :: fmt :: Result {
                    if self . is_empty () { :: core :: write ! (f , "{:#x}" , < #append(t) as #append(c) :: Bits > :: EMPTY) } else { :: core :: fmt :: Display :: fmt (self , f) }
                }
            }
            impl :: core :: fmt :: Display for #word(i) {
                fn fmt (& self , f : & mut :: core :: fmt :: Formatter < '_ >) -> :: core :: fmt :: Result { #append(c) :: parser :: to_writer (& #word(p) (* self) , f) }
            }
            impl :: core :: str :: FromStr for #word(i) {
                type Err = #append(c) :: parser :: ParseError ;
                fn from_str (s : & str) -> :: core :: result :: Result < Self , Self :: Err > { #append(c) :: parser :: from_str :: < #word(p) > (s) . map (| flags | flags . 0) }
            }
            impl :: core :: convert :: AsRef < #append(t) > for #word(i) { fn as_ref (& self) -> & #append(t) { & self . 0 } }
            impl :: core :: convert :: From < #append(t) > for #word(i) { fn from (bits : #append(t)) -> Self { Self :: from_bits_retain (bits) } }
        );
        impl_bitflags(&mut o, &none_outer, i, t, &public_bodies(c, t, p, &d.flags, edition));
        ops(&mut o, i);
        iter(&mut o, c, i, p);
        tg!(&mut o; impl #word(i) { #[inline] pub fn bits_mut (& mut self) -> & mut #append(t) { & mut self . 0 } });
        // public type: forward, ops, iter
        impl_bitflags(&mut o, &none_outer, p, t, &forward_bodies(i, edition));
        ops(&mut o, p);
        iter(&mut o, c, p, p);
    }
    Ok(o)
}
