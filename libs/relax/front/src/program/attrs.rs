//! Attribute resolution (rustc_resolve macros.rs `early_resolve_ident_in_lexical_scope` for
//! attributes): `#[name]` must be a built-in attribute, a registered tool's, a derive helper or
//! an attribute macro in scope; `#[a::b]` resolves `a` as a tool or a module path.
use super::*;
use crate::builtin_attrs::{is_builtin_attr, BUILTIN_TOOLS};
use crate::lexer::T;

/// The std derives, which have no helper attributes.
const STD_DERIVES: &[&str] = &["Clone", "Copy", "Debug", "Default", "Eq", "Hash", "Ord", "PartialEq", "PartialOrd"];

impl Program {
    /// E0433 / "cannot find attribute" for the attributes of the own crates' files.
    pub fn attribute_errors(&self, own: &[bool]) -> Vec<String> {
        let mut out = Vec::new();
        let mut by_file: Vec<Vec<DefId>> = vec![Vec::new(); self.files.len()];
        for (i, d) in self.defs.iter().enumerate() {
            if own.get(d.krate as usize).copied().unwrap_or(false) && (d.file as usize) < by_file.len() {
                by_file[d.file as usize].push(DefId(i as u32));
            }
        }
        for (fi, f) in self.files.iter().enumerate() {
            let krate = self.file_crate.get(fi).copied().unwrap_or(u32::MAX);
            if !own.get(krate as usize).copied().unwrap_or(false) || f.token_input || fi == 0 {
                continue;
            }
            let module = match self.mods.iter().position(|m| m.file == fi as u32 && m.krate == krate) {
                Some(m) => m as u32,
                None => continue,
            };
            let text = |i: u32| f.tok_text(i).trim_start_matches("r#");
            // the attributes of the file and of the definitions it holds (cfg-stripped items own
            // none: they define nothing)
            let mut ranges: Vec<Attr> = f.ast.root_attrs.iter().copied().collect();
            let n_root = ranges.len();
            // what each attribute is on: 0 the crate, 1 a struct / enum / union, 2 another item, 3 a field or variant
            let mut targets: Vec<u8> = vec![0; n_root];
            for &d in &by_file[fi] {
                let def = self.def(d);
                if def.item.0 == u32::MAX || self.item_defs.get(&(def.file, def.item.0)) != Some(&d) {
                    continue;
                }
                let it = f.ast.item(def.item);
                let tgt = if matches!(def.kind, DefKind::Struct | DefKind::Enum | DefKind::Union) { 1 } else { 2 };
                ranges.extend_from_slice(f.ast.list(it.attrs));
                targets.resize(ranges.len(), tgt);
                let fields = |data: VariantData, out: &mut Vec<Attr>| {
                    if let VariantData::Tuple(l) | VariantData::Struct(l) = data {
                        for fd in f.ast.list(l) {
                            out.extend_from_slice(f.ast.list(fd.attrs));
                        }
                    }
                };
                if let Some(st) = f.ast.as_struct(&it.kind) {
                    fields(st.data, &mut ranges);
                } else if let Some(u) = f.ast.as_union(&it.kind) {
                    fields(VariantData::Struct(u.fields), &mut ranges);
                } else if let Some(e) = f.ast.as_enum(&it.kind) {
                    for v in f.ast.list(e.variants) {
                        ranges.extend_from_slice(f.ast.list(v.attrs));
                        fields(v.data, &mut ranges);
                    }
                }
                targets.resize(ranges.len(), 3);
            }
            // registered tools; derive helpers of non-std derives are not tracked: such a file
            // keeps its inert attributes
            let mut tools: Vec<&str> = Vec::new();
            let mut helpers = false;
            let mut std_helpers: Vec<&str> = Vec::new();
            for a in &ranges {
                let (lo, hi) = (a.toks.lo, a.toks.hi);
                if hi <= lo || hi as usize > f.toks.len() {
                    continue;
                }
                match text(lo) {
                    "register_tool" => {
                        for i in lo + 1..hi {
                            if f.tok(i).kind == T::Ident {
                                tools.push(text(i));
                            }
                        }
                    }
                    "derive" => {
                        for i in lo + 1..hi {
                            if f.tok(i).kind == T::Ident && text(i) == "Default" {
                                std_helpers.push("default");
                            }
                            if f.tok(i).kind == T::Ident && !STD_DERIVES.contains(&text(i)) && f.tok(i + 1).kind != T::PathSep {
                                helpers = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if helpers {
                continue;
            }
            for (ai, a) in ranges.iter().enumerate() {
                let (lo, hi) = (a.toks.lo, a.toks.hi);
                if hi <= lo || hi as usize > f.toks.len() {
                    continue;
                }
                // the path: idents joined by `::`
                let mut segs: Vec<&str> = Vec::new();
                let mut i = lo;
                // `#[unsafe(name ..)]`
                if f.tok(i).kind == T::KwUnsafe && f.tok(i + 1).kind == T::OpenParen {
                    i += 2;
                }
                let first_i = i;
                let mut global = false;
                while i < hi {
                    match f.tok(i).kind {
                        T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper | T::KwSelfType => segs.push(text(i)),
                        T::PathSep => {
                            if i == first_i {
                                global = true;
                            }
                        }
                        _ => break,
                    }
                    i += 1;
                }
                if segs.is_empty() {
                    continue;
                }
                let pos = f.tok(first_i).lo;
                if segs.len() == 1 && !global {
                    let target = targets.get(ai).copied().unwrap_or(3);
                    match (segs[0], target) {
                        ("derive", 2) => out.push(self.record_error(fi as u32, pos, "parse", "E0774: `derive` may only be applied to `struct`s, `enum`s and `union`s".into())),
                        ("derive" | "test" | "test_case" | "bench" | "global_allocator", 0) => out.push(self.record_error(fi as u32, pos, "parse", format!("`{}` attribute cannot be used at crate level", segs[0]))),
                        _ => {}
                    }
                }
                // the attribute's shape (rustc_attr_parsing): `name` alone, or `name = "string"`
                if segs.len() == 1 && !global {
                    let wrapped = first_i != lo;
                    let end = if wrapped { hi - 1 } else { hi };
                    if let Some(m) = attr_shape_error(f, segs[0], i, end) {
                        out.push(self.record_error(fi as u32, pos, "parse", m));
                    }
                }
                // `#[name = value]`: the value is a literal (a macro call only where it expands)
                if f.tok(i).kind == T::Eq && f.tok(i).lo < f.src_len {
                    if let Some(m) = attr_value_error(f, i + 1, if first_i != lo { hi - 1 } else { hi }, ai < n_root && matches!(segs[0], "crate_name" | "crate_type")) {
                        out.push(self.record_error(fi as u32, f.tok(i + 1).lo, "parse", m));
                    }
                } else if matches!(f.tok(i).kind, T::OpenParen) && is_builtin_attr(segs[0]) && segs.len() == 1 && !matches!(segs[0], "cfg_attr" | "derive" | "macro_use" | "test" | "bench" | "rustc_on_unimplemented" | "link") {
                    if let Some(m) = attr_list_error(f, i + 1, if first_i != lo { hi - 1 } else { hi }) {
                        out.push(self.record_error(fi as u32, f.tok(i + 1).lo, "parse", m));
                    }
                }
                let tool = |n: &str| BUILTIN_TOOLS.contains(&n) || tools.contains(&n);
                let msg = if segs.len() == 1 && !global {
                    let n = segs[0];
                    if is_builtin_attr(n) || std_helpers.contains(&n) || crate::macro_reg::has_attr(n) {
                        continue;
                    }
                    format!("cannot find attribute `{n}` in this scope")
                } else {
                    if !global && tool(segs[0]) {
                        continue;
                    }
                    let first = segs[0];
                    let kind = |k: &str| match k { "crate" => SegKind::Crate, "self" => SegKind::SelfValue, "super" => SegKind::Super, _ => SegKind::Ident };
                    let path: Vec<(SegKind, Sym)> = match segs.iter().map(|s| self.syms.get(s).map(|y| (kind(s), y))).collect::<Option<Vec<_>>>() {
                        Some(p) => p,
                        None => {
                            // a name never interned anywhere resolves nowhere
                            out.push(self.record_error(fi as u32, pos, "resolve", format!("E0433: failed to resolve: use of unresolved module or unlinked crate `{first}`")));
                            continue;
                        }
                    };
                    let edition = self.crates[krate as usize].edition;
                    let mut cur = None;
                    let mut fail = None;
                    for k in 1..=path.len() {
                        match self.resolve_mod_path(module, global, &path[..k], edition < 2018, false) {
                            Some(d) => cur = Some(d),
                            None => {
                                fail = Some(k - 1);
                                break;
                            }
                        }
                    }
                    match fail {
                        None => continue,
                        Some(0) => format!("E0433: failed to resolve: use of unresolved module or unlinked crate `{first}`"),
                        Some(k) => {
                            // a macro of another crate's module, or of this one: not a type-namespace item
                            let container = cur.unwrap_or(NO_DEF);
                            if container == NO_DEF || self.def(container).krate != krate {
                                continue;
                            }
                            if self.def(container).kind == DefKind::Mod && self.lookup_macro_in_container(container, path[k].1) {
                                continue;
                            }
                            format!("E0433: failed to resolve: could not find `{}` in `{}`", segs[k], segs[k - 1])
                        }
                    }
                };
                out.push(self.record_error(fi as u32, pos, "resolve", msg));
            }
        }
        out
    }
}

fn is_lit_tok(k: T) -> bool {
    matches!(k, T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte | T::KwTrue | T::KwFalse)
}

/// The value tokens `[lo, hi)` of `#[name = value]`: one literal (or `-` and a number), or a macro
/// call outside the crate's inner attributes (it expands to the literal).
fn attr_value_error(f: &ParsedFile, lo: u32, hi: u32, crate_level: bool) -> Option<String> {
    let n = hi.saturating_sub(lo);
    if n == 0 {
        return None;
    }
    if n == 1 && is_lit_tok(f.tok(lo).kind) {
        return None;
    }
    if n == 2 && f.tok(lo).kind == T::Minus && matches!(f.tok(lo + 1).kind, T::Int | T::Float) {
        return None;
    }
    if !crate_level {
        // path `!` group
        let mut i = lo;
        while i < hi && matches!(f.tok(i).kind, T::Ident | T::RawIdent | T::PathSep | T::KwCrate | T::KwSelfValue | T::KwSuper) {
            i += 1;
        }
        if i > lo && i + 1 < hi && f.tok(i).kind == T::Not && matches!(f.tok(i + 1).kind, T::OpenParen | T::OpenBracket | T::OpenBrace) && matches!(f.tok(hi - 1).kind, T::CloseParen | T::CloseBracket | T::CloseBrace) {
            return None;
        }
    }
    Some("attribute value must be a literal".to_string())
}

/// The entries of a builtin attribute's `( .. )` list at tokens `[lo, hi)` (the closing `)` is
/// the last): `name = value` entries hold a literal (rustc_attr_parsing ArgParser).
fn attr_list_error(f: &ParsedFile, lo: u32, hi: u32) -> Option<String> {
    let end = hi.saturating_sub(1); // the `)`
    let mut depth = 0u32;
    let mut entry = lo;
    let mut i = lo;
    while i <= end {
        let k = if i == end { T::Comma } else { f.tok(i).kind };
        match k {
            T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
            T::CloseParen | T::CloseBracket | T::CloseBrace => depth = depth.saturating_sub(1),
            _ => {}
        }
        if k == T::Comma && depth == 0 {
            // the entry [entry, i): `path = value`
            let mut j = entry;
            while j < i && matches!(f.tok(j).kind, T::Ident | T::RawIdent | T::PathSep) {
                j += 1;
            }
            if j > entry && j < i && f.tok(j).kind == T::Eq {
                let v = j + 1;
                if i > v && !(i - v == 1 && is_lit_tok(f.tok(v).kind)) {
                    let found = if f.tok(v).kind == T::Ident && i > v + 1 && f.tok(v + 1).kind == T::Not { "macro call" } else { "expression" };
                    return Some(format!("expected a literal (`1u8`, `1.0f32`, `\"string\"`, etc.) here, found {found}"));
                }
            }
            entry = i + 1;
        }
        i += 1;
    }
    None
}

/// Attributes that take no arguments.
const WORD_ATTRS: &[&str] = &[
    "cold", "no_mangle", "track_caller", "non_exhaustive", "no_std", "no_main", "no_implicit_prelude",
    "automatically_derived", "no_link", "no_core", "thread_local", "ffi_pure", "ffi_const", "no_builtins",
];
/// Attributes that take a string: `#[name = "value"]`.
const NAME_VALUE_STR_ATTRS: &[&str] = &[
    "crate_name", "crate_type", "recursion_limit", "type_length_limit", "windows_subsystem", "link_section",
    "export_name", "link_name",
];

/// The tokens `[i, end)` after the path of `#[name ..]` against the attribute's template.
fn attr_shape_error(f: &ParsedFile, name: &str, i: u32, end: u32) -> Option<String> {
    if WORD_ATTRS.contains(&name) {
        if i < end {
            return Some(format!("E0565: malformed `{name}` attribute input"));
        }
        return None;
    }
    if NAME_VALUE_STR_ATTRS.contains(&name) {
        let malformed = || Some(format!("E0539: malformed `{name}` attribute input"));
        if !(i + 2 == end && f.tok(i).kind == T::Eq) {
            // `name = macro!(..)` is the literal check's business
            if i < end && f.tok(i).kind == T::Eq {
                return None;
            }
            return malformed();
        }
        let v = f.tok(i + 1);
        if !matches!(v.kind, T::Str | T::RawStr) {
            return malformed();
        }
        if name == "windows_subsystem" {
            let t = f.tok_text(i + 1);
            if t != "\"console\"" && t != "\"windows\"" {
                return Some(format!("invalid windows subsystem {t}, only `windows` and `console` are allowed"));
            }
        }
    }
    None
}
