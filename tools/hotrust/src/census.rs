//! Census of constructs outside the HotRust dialect (rules in local/plans/fast-rust.md),
//! counted per crate from the syntax tree.

use crate::ast::*;
use crate::lexer::T;
use crate::parser::Parser;

pub const N: usize = 40;

// counter indices
pub const C_FILES: usize = 0;
pub const C_LINES: usize = 1;
pub const C_FNS: usize = 2;
pub const C_MACRO_RULES: usize = 3; // R1
pub const C_MAC_BUILTIN: usize = 4; // R2 ok
pub const C_MAC_USER: usize = 5; // R1/R2
pub const C_DERIVE_STD: usize = 6;
pub const C_DERIVE_OURS: usize = 7;
pub const C_DERIVE_OTHER: usize = 8; // R3
pub const C_ATTR_UNKNOWN: usize = 9; // R3 attribute macros (candidates)
pub const C_INFER: usize = 10; // R4 collect/into/parse/... without turbofish
pub const C_INFER_ANNOT: usize = 11; // R4 of those, under an annotated let
pub const C_ADAPTER: usize = 12; // R5 closure adapters (unambiguous)
pub const C_ADAPTER_MAP: usize = 13; // R5 `.map(` (iterator or Option/Result)
pub const C_BLANKET: usize = 14; // R6
pub const C_HRTB: usize = 15; // R6
pub const C_GAT: usize = 16; // R6
pub const C_SPECIAL: usize = 17; // R6 default fn / negative impl
pub const C_IMPL_RET: usize = 18; // R7
pub const C_IMPL_ARG: usize = 19; // R7
pub const C_GENERIC_FNS: usize = 20;
pub const C_LONG_GENERIC: usize = 21; // R8 generic fn > 30 lines
pub const C_REF_OP_IMPL: usize = 22; // R8 operator impl on &T
pub const C_LONG_FN: usize = 23; // R9 fn > 300 lines
pub const C_ASYNC: usize = 24; // R10
pub const C_CATCH_UNWIND: usize = 25; // R11
pub const C_GLOB_REEXPORT: usize = 26; // R12
pub const C_GLOB_USE: usize = 27; // ok
pub const C_INCLUDE_GEN: usize = 28; // R13 include!/OUT_DIR
pub const C_ASM: usize = 29; // R13
pub const C_TRANSMUTE: usize = 30; // R14 (review)
pub const C_MAYBE_SIZED: usize = 31; // R16
pub const C_UNION: usize = 32;
pub const C_STATIC_MUT: usize = 33;
pub const C_UNSAFE: usize = 34;
pub const C_DYN: usize = 35;
pub const C_CLOSURES: usize = 36;
pub const C_TRAITS: usize = 37;
pub const C_IMPLS: usize = 38;
pub const C_CONST_GENERIC: usize = 39;

pub const NAMES: [&str; N] = [
    "files", "lines", "fns", "macro_rules", "mac_builtin", "mac_user", "derive_std", "derive_ours", "derive_other",
    "attr_unknown", "infer_sites", "infer_annot", "adapters", "dot_map", "blanket_impl", "hrtb", "gat", "special",
    "impl_ret", "impl_arg", "generic_fns", "long_generic", "ref_op_impl", "long_fn", "async", "catch_unwind",
    "glob_reexport", "glob_use", "include_gen", "asm", "transmute", "maybe_sized", "union", "static_mut", "unsafe",
    "dyn", "closures", "traits", "impls", "const_generic",
];

#[derive(Clone)]
pub struct Census {
    pub c: [u64; N],
    /// (kind, name) -> count, kind: 'm' user macro, 'd' other derive, 'a' unknown attr, 'p' adapter name
    pub names: Vec<(u8, String, u64)>,
}

impl Census {
    pub fn new() -> Census {
        Census { c: [0; N], names: Vec::new() }
    }
    pub fn add_name(&mut self, kind: u8, name: &str) {
        for e in self.names.iter_mut() {
            if e.0 == kind && e.1 == name {
                e.2 += 1;
                return;
            }
        }
        self.names.push((kind, name.to_string(), 1));
    }
    pub fn merge(&mut self, o: &Census) {
        for i in 0..N {
            self.c[i] += o.c[i];
        }
        for e in &o.names {
            let mut found = false;
            for f in self.names.iter_mut() {
                if f.0 == e.0 && f.1 == e.1 {
                    f.2 += e.2;
                    found = true;
                    break;
                }
            }
            if !found {
                self.names.push(e.clone());
            }
        }
    }
}

const BUILTIN_MACROS: &[&str] = &[
    "format", "print", "println", "eprint", "eprintln", "write", "writeln", "panic", "format_args", "vec", "assert",
    "assert_eq", "assert_ne", "debug_assert", "debug_assert_eq", "debug_assert_ne", "matches", "unreachable", "todo",
    "unimplemented", "cfg", "concat", "stringify", "line", "file", "column", "module_path", "include_str",
    "include_bytes", "env", "option_env", "thread_local", "id", "live_id", "id_lut", "script_mod",
];

const STD_DERIVES: &[&str] = &["Clone", "Copy", "Debug", "Default", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash"];
const OUR_DERIVES: &[&str] = &[
    "Script", "ScriptHook", "Widget", "WidgetRef", "WidgetSet", "WidgetRegister", "Animator", "SerBin", "DeBin", "SerJson",
    "DeJson", "SerRon", "DeRon", "DefaultNone", "ScriptHookDeref", "LiveRegisterWidget",
];

const KNOWN_ATTRS: &[&str] = &[
    "cfg", "cfg_attr", "derive", "allow", "warn", "deny", "forbid", "expect", "inline", "cold", "test", "ignore",
    "should_panic", "repr", "path", "doc", "no_mangle", "link", "link_name", "export_name", "used", "must_use",
    "deprecated", "non_exhaustive", "macro_export", "macro_use", "automatically_derived", "rustfmt", "clippy",
    "target_feature", "track_caller", "unsafe", "naked", "no_std", "no_implicit_prelude", "crate_type", "crate_name",
    "recursion_limit", "type_length_limit", "feature", "windows_subsystem", "proc_macro", "proc_macro_derive",
    "proc_macro_attribute", "global_allocator", "panic_handler", "link_section", "thread_local", "diagnostic",
    "debugger_visualizer", "collapse_debuginfo", "may_dangle", "instruction_set", "optimize", "coverage", "bench",
    "no_main", "no_builtins", "rustc_diagnostic_item", "rustc_on_unimplemented", "doc_cfg", "linkage",
];

const ADAPTERS: &[&str] = &[
    "filter", "filter_map", "flat_map", "flatten", "fold", "try_fold", "any", "all", "position", "rposition", "for_each",
    "try_for_each", "zip", "chain", "collect", "skip_while", "take_while", "map_while", "scan", "inspect", "min_by",
    "max_by", "min_by_key", "max_by_key", "find_map", "partition", "sum", "product", "cloned", "copied", "peekable",
    "windows", "chunks", "rev", "enumerate", "step_by", "last", "count", "nth", "unzip", "cycle", "fuse", "dedup_by_key",
];

const INFER_METHODS: &[&str] = &["collect", "into", "try_into", "parse", "sum", "product"];

const OP_TRAITS: &[&str] = &[
    "Add", "Sub", "Mul", "Div", "Rem", "Neg", "BitAnd", "BitOr", "BitXor", "Shl", "Shr", "AddAssign", "SubAssign",
    "MulAssign", "DivAssign",
];

fn contains(list: &[&str], s: &str) -> bool {
    for x in list {
        if *x == s {
            return true;
        }
    }
    false
}

pub struct Walker<'p, 'a> {
    p: &'p Parser<'a>,
    cfg: &'p crate::cfg::CfgSet,
    pub cen: Census,
    line_starts: Vec<u32>,
    /// generic params of the enclosing impl/fn (for blanket detection)
    in_annotated_let: bool,
    in_for_head: bool,
}

impl<'p, 'a> Walker<'p, 'a> {
    pub fn new(p: &'p Parser<'a>, cfg: &'p crate::cfg::CfgSet) -> Self {
        let mut ls = vec![0u32];
        for i in 0..p.src.len() {
            if p.src[i] == b'\n' {
                ls.push(i as u32 + 1);
            }
        }
        Walker { p, cfg, cen: Census::new(), line_starts: ls, in_annotated_let: false, in_for_head: false }
    }

    fn line(&self, pos: u32) -> u32 {
        // binary search
        let v = &self.line_starts;
        let mut lo = 0usize;
        let mut hi = v.len();
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if v[mid] <= pos {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo as u32
    }

    fn text(&self, id: Ident) -> &'a str {
        let s = &self.p.src[id.lo as usize..id.hi as usize];
        std::str::from_utf8(s).unwrap_or("?")
    }

    fn tok_text(&self, i: u32) -> &'a str {
        let t = self.p.toks[i as usize];
        std::str::from_utf8(&self.p.src[t.lo as usize..t.hi as usize]).unwrap_or("?")
    }

    pub fn file(&mut self) {
        let p = self.p;
        self.cen.c[C_FILES] += 1;
        self.cen.c[C_LINES] += self.line_starts.len() as u64;
        if !crate::cfg::active(crate::cfg::Src { src: p.src, toks: &p.toks }, &p.ast.root_attrs, self.cfg) {
            return;
        }
        self.attrs(&p.ast.root_attrs, true);
        // token-level checks
        for i in 0..p.toks.len() {
            let t = p.toks[i];
            if t.kind == T::Ident {
                let s = &p.src[t.lo as usize..t.hi as usize];
                if s == b"catch_unwind" || s == b"resume_unwind" {
                    self.cen.c[C_CATCH_UNWIND] += 1;
                } else if s == b"transmute" || s == b"transmute_copy" {
                    self.cen.c[C_TRANSMUTE] += 1;
                } else if s == b"OUT_DIR" {
                    self.cen.c[C_INCLUDE_GEN] += 1;
                }
            }
        }
        for &it in &p.ast.root_items {
            self.item(it, false);
        }
    }

    fn attrs(&mut self, attrs: &[Attr], on_item: bool) {
        for a in attrs {
            if a.toks.hi <= a.toks.lo {
                continue;
            }
            let name = self.tok_text(a.toks.lo);
            if name == "derive" {
                // derive(A, B, path::C)
                let mut i = a.toks.lo + 1;
                while i < a.toks.hi {
                    let t = self.p.toks[i as usize];
                    if t.kind == T::Ident {
                        // last segment only
                        let next = if i + 1 < a.toks.hi { self.p.toks[i as usize + 1].kind } else { T::Comma };
                        if next != T::PathSep {
                            let n = self.tok_text(i);
                            if contains(STD_DERIVES, n) {
                                self.cen.c[C_DERIVE_STD] += 1;
                            } else if contains(OUR_DERIVES, n) {
                                self.cen.c[C_DERIVE_OURS] += 1;
                                self.cen.add_name(b'o', n);
                            } else {
                                self.cen.c[C_DERIVE_OTHER] += 1;
                                self.cen.add_name(b'd', n);
                            }
                        }
                    }
                    i += 1;
                }
            } else if name == "cfg_attr" && crate::cfg::cfg_attr_holds(crate::cfg::Src { src: self.p.src, toks: &self.p.toks }, a, self.cfg) {
                // look for derive inside
                let mut i = a.toks.lo + 1;
                while i < a.toks.hi {
                    if self.p.toks[i as usize].kind == T::Ident && self.tok_text(i) == "derive" {
                        let mut j = i + 1;
                        while j < a.toks.hi && self.p.toks[j as usize].kind != T::CloseParen {
                            if self.p.toks[j as usize].kind == T::Ident {
                                let n = self.tok_text(j);
                                if !contains(STD_DERIVES, n) && !contains(OUR_DERIVES, n) {
                                    self.cen.c[C_DERIVE_OTHER] += 1;
                                    self.cen.add_name(b'd', n);
                                }
                            }
                            j += 1;
                        }
                    }
                    i += 1;
                }
            } else if on_item && !contains(KNOWN_ATTRS, name) && !name.starts_with("rustc_") {
                self.cen.c[C_ATTR_UNKNOWN] += 1;
                self.cen.add_name(b'a', name);
            }
        }
    }

    fn mac(&mut self, m: &MacCall) {
        let last = match m.path.segs.last() {
            Some(s) => self.text(s.name),
            None => "",
        };
        if contains(BUILTIN_MACROS, last) {
            self.cen.c[C_MAC_BUILTIN] += 1;
        } else {
            self.cen.c[C_MAC_USER] += 1;
            self.cen.add_name(b'm', last);
        }
        if last == "asm" || last == "global_asm" {
            self.cen.c[C_ASM] += 1;
        }
        if last == "include" {
            self.cen.c[C_INCLUDE_GEN] += 1;
        }
    }

    fn generics(&mut self, g: &Generics) {
        for gp in &g.params {
            match &gp.kind {
                GenericParamKind::Type(b, d) => {
                    self.bounds(b);
                    if let Some(t) = d {
                        self.ty(*t, false);
                    }
                }
                GenericParamKind::Const(t, _) => {
                    self.cen.c[C_CONST_GENERIC] += 1;
                    self.ty(*t, false);
                }
                GenericParamKind::Lifetime(_) => {}
            }
        }
        for w in &g.where_ {
            if let WherePred::Bound { hrtb, ty, bounds } = w {
                if !hrtb.is_empty() {
                    self.cen.c[C_HRTB] += 1;
                }
                self.ty(*ty, false);
                self.bounds(bounds);
            }
        }
    }

    fn bounds(&mut self, b: &[Bound]) {
        for x in b {
            if let Bound::Trait { hrtb, maybe, path, .. } = x {
                if !hrtb.is_empty() {
                    self.cen.c[C_HRTB] += 1;
                }
                if *maybe {
                    self.cen.c[C_MAYBE_SIZED] += 1;
                }
                self.path(path);
            }
        }
    }

    fn path(&mut self, p: &Path) {
        if let Some(q) = &p.qself {
            self.ty(q.ty, false);
        }
        for s in &p.segs {
            if let Some(a) = &s.args {
                match &**a {
                    GenericArgs::Angle(v) => {
                        for g in v {
                            match g {
                                GenericArg::Type(t) => self.ty(*t, false),
                                GenericArg::Binding(_, _, t) => self.ty(*t, false),
                                GenericArg::Constraint(_, b) => self.bounds(b),
                                GenericArg::Const(e) => self.expr(*e),
                                GenericArg::Lifetime(_) => {}
                            }
                        }
                    }
                    GenericArgs::Paren(v, r) => {
                        for t in v {
                            self.ty(*t, false);
                        }
                        if let Some(t) = r {
                            self.ty(*t, false);
                        }
                    }
                }
            }
        }
    }

    /// `pos`: 0 other, 1 fn return, 2 fn argument
    fn ty_pos(&mut self, id: TyId, pos: u8) {
        match self.p.ast.ty(id) {
            Ty::ImplTrait(b) => {
                if pos == 1 {
                    self.cen.c[C_IMPL_RET] += 1;
                } else {
                    self.cen.c[C_IMPL_ARG] += 1;
                }
                self.bounds(b);
            }
            _ => self.ty(id, pos != 0),
        }
    }

    fn ty(&mut self, id: TyId, sig: bool) {
        let p = self.p;
        match p.ast.ty(id) {
            Ty::Path(path) => self.path(path),
            Ty::Ref(_, _, t) | Ty::Ptr(_, t) | Ty::Slice(t) | Ty::Paren(t) => self.ty(*t, sig),
            Ty::Array(t, e) => {
                self.ty(*t, sig);
                self.expr(*e);
            }
            Ty::Tuple(v) => {
                for t in v {
                    self.ty(*t, sig);
                }
            }
            Ty::Fn(f) => {
                if !f.hrtb.is_empty() {
                    self.cen.c[C_HRTB] += 1;
                }
                for t in &f.params {
                    self.ty(*t, false);
                }
                if let Some(t) = f.ret {
                    self.ty(t, false);
                }
            }
            Ty::ImplTrait(b) => {
                if sig {
                    self.cen.c[C_IMPL_ARG] += 1;
                } else {
                    self.cen.c[C_IMPL_ARG] += 1;
                }
                self.bounds(b);
            }
            Ty::DynTrait(b, _) => {
                self.cen.c[C_DYN] += 1;
                self.bounds(b);
            }
            Ty::Mac(m) => self.mac(m),
            _ => {}
        }
    }

    fn fn_sig(&mut self, sig: &FnSig) {
        self.generics(&sig.generics);
        if let Some(SelfParam::Typed(_, t)) = &sig.self_param {
            self.ty(*t, false);
        }
        for prm in &sig.params {
            self.pat(prm.pat);
            self.ty_pos(prm.ty, 2);
        }
        if let Some(r) = sig.ret {
            self.ty_pos(r, 1);
        }
        if sig.asyncness {
            self.cen.c[C_ASYNC] += 1;
        }
    }

    fn is_generic(&self, g: &Generics) -> bool {
        for gp in &g.params {
            if !matches!(gp.kind, GenericParamKind::Lifetime(_)) {
                return true;
            }
        }
        false
    }

    fn item(&mut self, id: ItemId, in_generic_impl: bool) {
        let p = self.p;
        let it = p.ast.item(id);
        if !crate::cfg::active(crate::cfg::Src { src: p.src, toks: &p.toks }, &it.attrs, self.cfg) {
            return;
        }
        self.attrs(&it.attrs, true);
        match &it.kind {
            ItemKind::Use { tree, .. } => {
                let pubvis = !matches!(it.vis, Vis::Private);
                self.use_tree(tree, pubvis);
            }
            ItemKind::Fn(sig, body, default_) => {
                self.cen.c[C_FNS] += 1;
                if *default_ {
                    self.cen.c[C_SPECIAL] += 1;
                }
                self.fn_sig(sig);
                if let Some(b) = body {
                    let blk = p.ast.block(*b);
                    let lines = self.line(blk.hi) - self.line(blk.lo) + 1;
                    let generic = self.is_generic(&sig.generics) || in_generic_impl;
                    if self.is_generic(&sig.generics) {
                        self.cen.c[C_GENERIC_FNS] += 1;
                    }
                    if generic && lines > 30 {
                        self.cen.c[C_LONG_GENERIC] += 1;
                    }
                    if lines > 300 {
                        self.cen.c[C_LONG_FN] += 1;
                    }
                    self.block(*b);
                }
            }
            ItemKind::Struct(g, data) => {
                self.generics(g);
                self.variant_data(data);
            }
            ItemKind::Union(g, f) => {
                self.cen.c[C_UNION] += 1;
                self.generics(g);
                for fd in f {
                    self.ty(fd.ty, false);
                }
            }
            ItemKind::Enum(g, vars) => {
                self.generics(g);
                for v in vars {
                    self.attrs(&v.attrs, false);
                    self.variant_data(&v.data);
                    if let Some(e) = v.disc {
                        self.expr(e);
                    }
                }
            }
            ItemKind::TypeAlias { generics, bounds, ty } => {
                self.generics(generics);
                self.bounds(bounds);
                if let Some(t) = ty {
                    self.ty(*t, false);
                }
            }
            ItemKind::Const(t, e) => {
                if let Some(t) = t {
                    self.ty(*t, false);
                }
                if let Some(e) = e {
                    self.expr(*e);
                }
            }
            ItemKind::Static(m, t, e) => {
                if *m {
                    self.cen.c[C_STATIC_MUT] += 1;
                }
                self.ty(*t, false);
                if let Some(e) = e {
                    self.expr(*e);
                }
            }
            ItemKind::Trait { generics, supers, items, .. } => {
                self.cen.c[C_TRAITS] += 1;
                self.generics(generics);
                self.bounds(supers);
                for &i in items {
                    // GAT: associated type with generic params
                    if let ItemKind::TypeAlias { generics: g2, .. } = &p.ast.item(i).kind {
                        if !g2.params.is_empty() {
                            self.cen.c[C_GAT] += 1;
                        }
                    }
                    self.item(i, self.is_generic(generics));
                }
            }
            ItemKind::TraitAlias(g, b) => {
                self.generics(g);
                self.bounds(b);
            }
            ItemKind::Impl { generics, negative, trait_, self_ty, items, default_, .. } => {
                self.cen.c[C_IMPLS] += 1;
                if *negative || *default_ {
                    self.cen.c[C_SPECIAL] += 1;
                }
                self.generics(generics);
                if let Some(tp) = trait_ {
                    self.path(tp);
                    // blanket: self type is (a reference to / Box of) one of the impl's type params
                    if self.is_blanket(generics, *self_ty) {
                        self.cen.c[C_BLANKET] += 1;
                    }
                    if let Some(last) = tp.segs.last() {
                        let tn = self.text(last.name);
                        if contains(OP_TRAITS, tn) {
                            if let Ty::Ref(..) = p.ast.ty(*self_ty) {
                                self.cen.c[C_REF_OP_IMPL] += 1;
                            }
                        }
                    }
                }
                self.ty(*self_ty, false);
                let g = self.is_generic(generics);
                for &i in items {
                    self.item(i, g);
                }
            }
            ItemKind::Mod(Some(items)) => {
                for &i in items {
                    self.item(i, false);
                }
            }
            ItemKind::ForeignMod(_, items) => {
                for &i in items {
                    self.item(i, false);
                }
            }
            ItemKind::MacroRules(_) => {
                self.cen.c[C_MACRO_RULES] += 1;
            }
            ItemKind::Mac(m) => self.mac(m),
            _ => {}
        }
    }

    fn is_blanket(&self, g: &Generics, self_ty: TyId) -> bool {
        let mut t = self_ty;
        loop {
            match self.p.ast.ty(t) {
                Ty::Ref(_, _, inner) => t = *inner,
                Ty::Path(path) => {
                    if path.qself.is_some() || path.segs.len() != 1 || path.segs[0].args.is_some() {
                        // Box<T>
                        if path.segs.len() == 1 && self.text(path.segs[0].name) == "Box" {
                            if let Some(a) = &path.segs[0].args {
                                if let GenericArgs::Angle(v) = &**a {
                                    if v.len() == 1 {
                                        if let GenericArg::Type(inner) = v[0] {
                                            t = inner;
                                            continue;
                                        }
                                    }
                                }
                            }
                        }
                        return false;
                    }
                    let n = self.text(path.segs[0].name);
                    for gp in &g.params {
                        if let GenericParamKind::Type(..) = gp.kind {
                            if self.text(gp.name) == n {
                                return true;
                            }
                        }
                    }
                    return false;
                }
                _ => return false,
            }
        }
    }

    fn variant_data(&mut self, d: &VariantData) {
        match d {
            VariantData::Tuple(f) | VariantData::Struct(f) => {
                for fd in f {
                    self.attrs(&fd.attrs, false);
                    self.ty(fd.ty, false);
                }
            }
            VariantData::Unit => {}
        }
    }

    fn use_tree(&mut self, t: &UseTree, pubvis: bool) {
        match t {
            UseTree::Glob => {
                if pubvis {
                    self.cen.c[C_GLOB_REEXPORT] += 1;
                } else {
                    self.cen.c[C_GLOB_USE] += 1;
                }
            }
            UseTree::Path(_, rest) => self.use_tree(rest, pubvis),
            UseTree::Group(v) => {
                for x in v {
                    self.use_tree(x, pubvis);
                }
            }
            UseTree::Name(..) => {}
        }
    }

    fn block(&mut self, b: BlockId) {
        let p = self.p;
        let blk = p.ast.block(b);
        for s in &blk.stmts {
            match s {
                Stmt::Let { attrs, pat, ty, init, else_ } => {
                    if !crate::cfg::active(crate::cfg::Src { src: p.src, toks: &p.toks }, attrs, self.cfg) {
                        continue;
                    }
                    self.attrs(attrs, false);
                    self.pat(*pat);
                    if let Some(t) = ty {
                        self.ty(*t, false);
                    }
                    if let Some(e) = init {
                        let save = self.in_annotated_let;
                        self.in_annotated_let = ty.is_some();
                        self.expr(*e);
                        self.in_annotated_let = save;
                    }
                    if let Some(b) = else_ {
                        self.block(*b);
                    }
                }
                Stmt::Item(i) => self.item(*i, false),
                Stmt::Expr(e, _) => self.expr(*e),
                Stmt::Attrs(_, s) => {
                    if let Stmt::Expr(e, _) = &**s {
                        self.expr(*e);
                    }
                }
                Stmt::Empty => {}
            }
        }
    }

    fn pat(&mut self, id: PatId) {
        let p = self.p;
        match p.ast.pat(id) {
            Pat::Ident { sub: Some(s), .. } => self.pat(*s),
            Pat::TupleStruct(_, v) | Pat::Tuple(v) | Pat::Slice(v) | Pat::Or(v) => {
                for x in v {
                    self.pat(*x);
                }
            }
            Pat::Struct(_, f, _) => {
                for x in f {
                    self.pat(x.pat);
                }
            }
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.pat(*x),
            Pat::Mac(m) => self.mac(m),
            _ => {}
        }
    }

    fn expr(&mut self, id: ExprId) {
        let p = self.p;
        let e = p.ast.expr(id);
        match &e.kind {
            ExprKind::Lit(_) | ExprKind::Underscore | ExprKind::Continue(_) => {}
            ExprKind::Path(path) => {
                // Default::default() handled at call site
                self.path(path);
            }
            ExprKind::Unary(_, x) | ExprKind::Paren(x) | ExprKind::Try(x) | ExprKind::Box(x) => self.expr(*x),
            ExprKind::AddrOf(_, _, x) => self.expr(*x),
            ExprKind::Await(x) => {
                self.cen.c[C_ASYNC] += 1;
                self.expr(*x);
            }
            ExprKind::Binary(_, a, b) | ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) | ExprKind::Index(a, b)
            | ExprKind::Repeat(a, b) => {
                self.expr(*a);
                self.expr(*b);
            }
            ExprKind::Call(f, args) => {
                if let ExprKind::Path(path) = &p.ast.expr(*f).kind {
                    let n = path.segs.len();
                    if n >= 2 {
                        let a = self.text(path.segs[n - 2].name);
                        let b = self.text(path.segs[n - 1].name);
                        if (a == "Default" && b == "default") || (a == "From" && b == "from") {
                            self.cen.c[C_INFER] += 1;
                            if self.in_annotated_let {
                                self.cen.c[C_INFER_ANNOT] += 1;
                            }
                        }
                    }
                }
                self.expr(*f);
                for a in args {
                    self.expr(*a);
                }
            }
            ExprKind::MethodCall { recv, name, turbofish, args } => {
                let n = self.text(*name);
                if turbofish.is_none() && contains(INFER_METHODS, n) && args.is_empty() {
                    self.cen.c[C_INFER] += 1;
                    if self.in_annotated_let {
                        self.cen.c[C_INFER_ANNOT] += 1;
                    }
                }
                if n == "map" {
                    self.cen.c[C_ADAPTER_MAP] += 1;
                } else if contains(ADAPTERS, n) {
                    // enumerate/rev/step_by directly in a for head are allowed
                    let allowed_in_for = n == "enumerate" || n == "rev" || n == "step_by";
                    let count_it = !(self.in_for_head && allowed_in_for);
                    if count_it && !(n == "last" || n == "count" || n == "nth" || n == "windows" || n == "chunks") {
                        self.cen.c[C_ADAPTER] += 1;
                        self.cen.add_name(b'p', n);
                    }
                }
                let save = self.in_annotated_let;
                self.in_annotated_let = false;
                self.expr(*recv);
                for a in args {
                    self.expr(*a);
                }
                self.in_annotated_let = save;
            }
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => self.expr(*x),
            ExprKind::Cast(x, t) => {
                self.expr(*x);
                self.ty(*t, false);
            }
            ExprKind::Block(b, _) | ExprKind::Unsafe(b) | ExprKind::ConstBlock(b) => {
                if let ExprKind::Unsafe(_) = &e.kind {
                    self.cen.c[C_UNSAFE] += 1;
                }
                let save = self.in_annotated_let;
                self.in_annotated_let = false;
                self.block(*b);
                self.in_annotated_let = save;
            }
            ExprKind::Async(_, b) => {
                self.cen.c[C_ASYNC] += 1;
                self.block(*b);
            }
            ExprKind::If(c, t, el) => {
                self.expr(*c);
                self.block(*t);
                if let Some(x) = el {
                    self.expr(*x);
                }
            }
            ExprKind::Let(pt, x) => {
                self.pat(*pt);
                self.expr(*x);
            }
            ExprKind::Match(x, arms) => {
                self.expr(*x);
                for a in arms {
                    if !crate::cfg::active(crate::cfg::Src { src: p.src, toks: &p.toks }, &a.attrs, self.cfg) {
                        continue;
                    }
                    self.attrs(&a.attrs, false);
                    self.pat(a.pat);
                    if let Some(g) = a.guard {
                        self.expr(g);
                    }
                    self.expr(a.body);
                }
            }
            ExprKind::While(c, b, _) => {
                self.expr(*c);
                self.block(*b);
            }
            ExprKind::Loop(b, _) => self.block(*b),
            ExprKind::For(pt, it, b, _) => {
                self.pat(*pt);
                let save = self.in_for_head;
                self.in_for_head = true;
                self.expr(*it);
                self.in_for_head = save;
                self.block(*b);
            }
            ExprKind::Break(_, x) | ExprKind::Return(x) | ExprKind::Yield(x) => {
                if let Some(x) = x {
                    self.expr(*x);
                }
            }
            ExprKind::Closure { is_async, params, ret, body, .. } => {
                self.cen.c[C_CLOSURES] += 1;
                if *is_async {
                    self.cen.c[C_ASYNC] += 1;
                }
                for prm in params {
                    self.pat(prm.pat);
                    if let Some(t) = prm.ty {
                        self.ty(t, false);
                    }
                }
                if let Some(t) = ret {
                    self.ty(*t, false);
                }
                let save = self.in_for_head;
                self.in_for_head = false;
                self.expr(*body);
                self.in_for_head = save;
            }
            ExprKind::Tuple(v) | ExprKind::Array(v) => {
                for x in v {
                    self.expr(*x);
                }
            }
            ExprKind::Struct(path, fields, base) => {
                self.path(path);
                for f in fields {
                    self.expr(f.expr);
                }
                if let Some(b) = base {
                    self.expr(*b);
                }
            }
            ExprKind::Range(a, b, _) => {
                if let Some(a) = a {
                    self.expr(*a);
                }
                if let Some(b) = b {
                    self.expr(*b);
                }
            }
            ExprKind::Mac(m) => self.mac(m),
        }
    }
}
