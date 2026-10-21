// ported from N:rustc_expand/src/config.rs @8d1a764 (crate feature selection)
// Relax's program table; the item-lowering parts follow N:rustc_ast_lowering/src/item.rs @8d1a764 (cited per function)
//! The whole program: crates, parsed files, interned names, definitions and
//! module scopes. Item collection, `use` resolution (fixpoint) and path lookup.

use crate::ast::*;
use crate::cfg::{CfgSet, Src};
use crate::parser::ParsedFile;
use crate::fxhash::HashMap;

mod collected;
mod privacy;
pub use privacy::PrivacyError;
mod rsim;
pub use rsim::{SimDiag, SimKind};
mod attrs;
mod validate;

pub type Sym = u32;

/// Macros Relax implements itself (dialect rule R2); importing them is a no-op.
pub const BUILTIN_MACROS: &[&str] = &[
    "format", "format_args", "print", "println", "eprint", "eprintln", "write", "writeln", "panic", "vec",
    "assert", "assert_eq", "assert_ne", "debug_assert", "debug_assert_eq", "debug_assert_ne", "matches",
    "unreachable", "todo", "unimplemented", "cfg", "concat", "stringify", "line", "file", "column",
    "module_path", "include_str", "include_bytes", "env", "option_env", "thread_local", "msg_send", "sel",
    "class", "log", "error", "warning", "warn", "trace", "debug", "asm",
    // core::prelude::v1's other built-in macros (`use core::{compile_error, prelude::v1::derive}`)
    "compile_error", "include", "global_asm", "naked_asm", "derive", "test", "bench", "global_allocator",
    "test_case", "trace_macros", "log_syntax", "concat_idents", "cfg_accessible", "cfg_eval", "derive_const",
    "alloc_error_handler", "assert_matches", "const_format_args",
];

#[derive(Default)]
pub struct Interner {
    /// load-time names restored from a collected image, read in place (ids 0..image.len())
    image: Option<MappedSyms>,
    /// load-time names interned in this process (ids after the image's)
    map: HashMap<String, Sym>,
    strs: Vec<String>,
    /// names first met after the load, through a shared reference (identifiers that only the
    /// macro expansions of lazily parsed fn bodies make): ids from LATE_SYM up
    late: relax_session::sync::Mutex<(HashMap<String, Sym>, Vec<&'static str>)>,
    has_late: std::sync::atomic::AtomicBool,
}

/// An interner's names in a collected image: offsets into one string section, and an
/// open-addressed table (FxHash of the bytes, linear probing) of [id + 1 (0 = empty), the
/// hash's low 32 bits] pairs, so a probe compares strings only on a likely match.
struct MappedSyms {
    offsets: &'static [u32],
    bytes: &'static [u8],
    table: &'static [u32],
}
impl MappedSyms {
    fn len(&self) -> usize { self.offsets.len() - 1 }
    fn str(&self, i: usize) -> &'static str {
        let b = &self.bytes[self.offsets[i] as usize..self.offsets[i + 1] as usize];
        // The section was written from Strings by this build and is covered by the store checksum.
        unsafe { std::str::from_utf8_unchecked(b) }
    }
    fn get(&self, s: &str) -> Option<Sym> {
        let size = self.table.len() / 2;
        let (mut at, tag) = sym_slot(s, size);
        loop {
            let e = self.table[at * 2];
            if e == 0 { return None; }
            if self.table[at * 2 + 1] == tag && self.str(e as usize - 1) == s { return Some(e - 1); }
            at = (at + 1) & (size - 1);
        }
    }
}
/// (table slot, tag)
fn sym_slot(s: &str, table: usize) -> (usize, u32) {
    use std::hash::Hasher;
    let mut h = crate::fxhash::FxHasher::default();
    h.write(s.as_bytes());
    let h = h.finish();
    // the multiply-rotate mix leaves its best bits at the top
    ((h >> (64 - table.trailing_zeros())) as usize, h as u32)
}

/// First symbol of Interner::intern_late's names (disjoint from load-time interning).
pub const LATE_SYM: Sym = 0x4000_0000;

impl Interner {
    /// Interns `s` through a shared reference (rare: names no loaded file's tokens hold).
    pub fn intern_late(&self, s: &str) -> Sym {
        if let Some(x) = self.get(s) {
            return x;
        }
        let mut l = self.late.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&x) = l.0.get(s) {
            return x;
        }
        let id = LATE_SYM + l.1.len() as Sym;
        let st: &'static str = Box::leak(s.to_string().into_boxed_str());
        l.1.push(st);
        l.0.insert(s.to_string(), id);
        self.has_late.store(true, std::sync::atomic::Ordering::Release);
        id
    }

    pub fn intern(&mut self, s: &str) -> Sym {
        if self.has_late.load(std::sync::atomic::Ordering::Relaxed) || self.image.is_some() {
            if let Some(x) = self.get(s) {
                return x;
            }
        }
        let _mem = crate::memstat::tag(crate::memstat::Tag::Syms);
        if let Some(&x) = self.map.get(s) {
            return x;
        }
        let id = self.len() as Sym;
        self.strs.push(s.to_string());
        self.map.insert(s.to_string(), id);
        id
    }
    pub fn get(&self, s: &str) -> Option<Sym> {
        if let Some(x) = self.image.as_ref().and_then(|m| m.get(s)) {
            return Some(x);
        }
        match self.map.get(s) {
            Some(x) => Some(*x),
            None if self.has_late.load(std::sync::atomic::Ordering::Acquire) => self.late.lock().unwrap_or_else(|e| e.into_inner()).0.get(s).copied(),
            None => None,
        }
    }
    pub fn str(&self, s: Sym) -> &str {
        let base = self.image.as_ref().map_or(0, |m| m.len());
        if (s as usize) < base {
            return self.image.as_ref().unwrap().str(s as usize);
        }
        match self.strs.get(s as usize - base) {
            Some(x) => x.as_str(),
            None if s >= LATE_SYM => self.late.lock().unwrap_or_else(|e| e.into_inner()).1.get((s - LATE_SYM) as usize).copied().unwrap_or("?"),
            None => "?",
        }
    }
    /// load-time names (not the late ones)
    pub fn len(&self) -> usize { self.image.as_ref().map_or(0, |m| m.len()) + self.strs.len() }
    /// load-time names in id order
    pub fn names(&self) -> impl Iterator<Item = &str> + '_ { (0..self.len() as Sym).map(|i| self.str(i)) }
    /// (names, heap bytes: both copies of every string plus the map's and list's tables)
    pub fn heap_bytes(&self) -> (usize, u64) {
        let s: usize = self.strs.iter().map(|x| x.capacity() * 2).sum();
        (self.len(), (s + self.strs.capacity() * 24 + self.map.capacity() * 29) as u64)
    }
    /// Three image sections: string offsets, string bytes, lookup table.
    pub fn write_image(&self, image: &mut crate::image::Writer<'_>, id: u32) {
        let n = self.len();
        let mut offsets = Vec::with_capacity(n + 1);
        let mut bytes = Vec::new();
        let size = (n * 2).next_power_of_two().max(2);
        let mut table = vec![0u32; size * 2];
        for (i, s) in self.names().enumerate() {
            offsets.push(u32::try_from(bytes.len()).expect("symbol section size"));
            bytes.extend_from_slice(s.as_bytes());
            let (mut at, tag) = sym_slot(s, size);
            while table[at * 2] != 0 { at = (at + 1) & (size - 1); }
            table[at * 2] = i as u32 + 1;
            table[at * 2 + 1] = tag;
        }
        offsets.push(u32::try_from(bytes.len()).expect("symbol section size"));
        image.add_owned(id, &offsets); image.add_bytes(id + 1, bytes); image.add_owned(id + 2, &table);
    }
    /// The certified image's names, in place: no string copies, no map build.
    pub fn from_image(image: &crate::image::Reader, k: usize, id: u32) -> Option<Self> {
        let offsets: &'static [u32] = image.slice(k, id)?;
        let bytes = image.bytes(k + 1, id + 1)?;
        let table: &'static [u32] = image.slice(k + 2, id + 2)?;
        if offsets.is_empty() || *offsets.last()? as usize != bytes.len() || table.len() < 4
            || !(table.len() / 2).is_power_of_two() || table.len() / 2 < offsets.len() { return None; }
        Some(Self { image: Some(MappedSyms { offsets, bytes, table }), ..Default::default() })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct DefId(pub u32);

pub const NO_DEF: DefId = DefId(u32::MAX);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Prim {
    Bool,
    Char,
    Str,
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    F32,
    F64,
}

pub const PRIMS: [(&str, Prim); 17] = [
    ("bool", Prim::Bool),
    ("char", Prim::Char),
    ("str", Prim::Str),
    ("i8", Prim::I8),
    ("i16", Prim::I16),
    ("i32", Prim::I32),
    ("i64", Prim::I64),
    ("i128", Prim::I128),
    ("isize", Prim::Isize),
    ("u8", Prim::U8),
    ("u16", Prim::U16),
    ("u32", Prim::U32),
    ("u64", Prim::U64),
    ("u128", Prim::U128),
    ("usize", Prim::Usize),
    ("f32", Prim::F32),
    ("f64", Prim::F64),
];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum DefKind {
    Mod,
    Fn,
    Struct,
    Union,
    Enum,
    Variant,
    Const,
    Static,
    Trait,
    Impl,
    TypeAlias,
    AssocFn,
    AssocConst,
    AssocTy,
    ForeignFn,
    ForeignStatic,
    ForeignTy,
    AnonConst,
    OpaqueTy,
    Closure,
    Field,
    MacroRules,
    Prim(Prim),
}

#[derive(Clone, Copy)]
pub struct Def {
    pub kind: DefKind,
    pub name: Sym,
    /// enclosing module (items), impl/trait (assoc items), enum (variants)
    pub parent: DefId,
    pub file: u32,
    pub item: ItemId,
    /// Mod: module index; Variant: variant index; Enum: variant list index;
    /// Impl: impl index; Trait: trait index
    pub sub: u32,
    pub is_pub: bool,
    pub krate: u32,
    /// module index whose scope resolves names used by this item
    pub scope: u32,
}

pub const IMPORT_PUB: u8 = 1;
pub const IMPORT_EXTERN: u8 = 2;
/// `Import::bound`: the namespaces this import bound, the injected prelude crate, an empty `{}` group
pub const BOUND_TYPES: u8 = 1;
pub const BOUND_VALUES: u8 = 2;
pub const BOUND_INJECTED: u8 = 4;
pub const BOUND_PREFIX_ONLY: u8 = 8;
/// the clash of this import with an earlier binding was reported (per namespace)
pub const CLASH_TYPES: u8 = 16;
pub const CLASH_VALUES: u8 = 32;
/// `use a::f::{self}`: only the type namespace of `f` (a module, enum or trait) is imported
pub const BOUND_SELF_ONLY: u8 = 64;

pub struct Module {
    pub def: DefId,
    /// lexical parent for anonymous block modules (lookups fall through)
    pub lexical_parent: Option<u32>,
    /// the named module that `self`/`super` refer to
    pub normal: u32,
    pub parent_mod: Option<u32>,
    pub krate: u32,
    pub file: u32,
    pub types: HashMap<Sym, DefId>,
    pub values: HashMap<Sym, DefId>,
    pub macros: HashMap<Sym, DefId>,
    /// glob imports: (target, `pub use`)
    pub globs: Vec<(DefId, bool)>,
    /// traits imported `as _` (in scope for method calls, no name)
    pub anon_traits: Vec<(DefId, bool)>,
    /// import bindings (name, type ns) -> IMPORT_PUB (`pub use`) | IMPORT_EXTERN (`extern crate`);
    /// an item's own binding has its item's visibility
    pub import_pub: HashMap<(Sym, bool), u8>,
}

/// How good an import path is to suggest: this crate's, then std's facade, then other crates',
/// then alloc / core (rustc shows std's paths for their items); shorter first.
fn path_rank(p: &str) -> (u8, usize) {
    let root = p.split("::").next().unwrap_or("");
    let r = match root {
        "crate" => 0,
        "std" => 1,
        "alloc" => 3,
        "core" => 4,
        _ => 2,
    };
    (r, p.matches("::").count())
}

/// Where a written-back `use` goes: after the file's leading comments, doc comments and inner
/// attributes (`#![..]`), at the start of a line.
fn use_insert_pos(src: &[u8]) -> u32 {
    let mut i = 0usize;
    loop {
        let start = i;
        while i < src.len() && (src[i] == b' ' || src[i] == b'\t' || src[i] == b'\r' || src[i] == b'\n') {
            i += 1;
        }
        if src[i..].starts_with(b"//") {
            while i < src.len() && src[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if src[i..].starts_with(b"/*") {
            if let Some(e) = src[i + 2..].windows(2).position(|w| w == b"*/") {
                i += e + 4;
                continue;
            }
        }
        if src[i..].starts_with(b"#![") {
            let mut d = 0i32;
            while i < src.len() {
                match src[i] {
                    b'[' => d += 1,
                    b']' => {
                        d -= 1;
                        if d == 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        // back to the start of the line of the first real token
        let mut p = i;
        while p > 0 && src[p - 1] != b'\n' {
            p -= 1;
        }
        let _ = start;
        return p as u32;
    }
}

/// Visibility of a binding as seen through glob imports (rustc ty::Visibility): None is
/// public, Some(m) is restricted to module m and its descendants.
pub type BindVis = Option<u32>;

pub struct ImplData {
    pub def: DefId,
    pub file: u32,
    pub item: ItemId,
    pub module: u32,
    pub items: Vec<DefId>,
}

/// What `Program::unbind_use` took out of one import (for `rebind_use`).
pub struct UseUndo {
    index: usize,
    module: u32,
    bind: Option<Sym>,
    done: bool,
    glob: bool,
    bound: u8,
    gone: Vec<(bool, Sym, DefId, u8)>,
}

pub struct Import {
    pub module: u32,
    pub global: bool,
    pub segs: Vec<(SegKind, Sym)>,
    /// binding name (None for globs)
    pub bind: Option<Sym>,
    pub glob: bool,
    pub is_pub: bool,
    pub done: bool,
    pub file: u32,
    pub pos: u32,
    /// `extern crate` (not `use`)
    pub ext: bool,
    /// namespaces this import has bound so far (bit 0 types, bit 1 values)
    pub bound: u8,
}

/// Module indexes of the std prelude (Program::prelude_ids).
struct PreludeIds {
    base: u32,
    /// rust_2015, rust_2018, rust_2021, rust_2024 (the base module where one is missing)
    editions: [u32; 4],
}

pub struct CrateInfo {
    pub name: Sym,
    pub root_mod: u32,
    pub edition: u16,
    /// Read once from the root attributes at load (and refreshed on root replacement).
    pub recursion_limit: usize,
    /// (extern name, crate index)
    pub deps: Vec<(Sym, u32)>,
    pub cfg: CfgSet,
    /// a proc-macro crate: other crates see only its macros (rustc exports nothing else)
    pub proc_macro: bool,
    /// a binary target: the check requires its `main`
    pub is_bin: bool,
    /// the crate's errors are reported (set by the check for its checked crates): validation-only
    /// checks run for it and for no other crate
    pub validate: bool,
    /// relaxed-Rust features this crate opts in to (relaxed.rs; empty = strict), read on first
    /// use (only a relaxable error or the borrow checker asks; strict programs never read it)
    relaxed: std::sync::OnceLock<Vec<String>>,
    /// the crate's `#![feature(..)]` declarations, read on first use (the check's gates)
    features: std::sync::OnceLock<crate::features::Features>,
    /// the crate's root source file (for its manifest)
    root_path: String,
    pub context: std::sync::Arc<crate::source_context::Context>,
}

fn recursion_limit(file: &ParsedFile) -> usize {
    crate::solver_work::count(crate::solver_work::RECURSION_SCANS);
    for attr in &file.ast.root_attrs {
        if attr.toks.hi == attr.toks.lo + 3 && file.tok_text(attr.toks.lo) == "recursion_limit"
            && file.tok_text(attr.toks.lo + 1) == "=" {
            if let Ok(limit) = file.tok_text(attr.toks.lo + 2).trim_matches('"').parse() { return limit; }
        }
    }
    128
}

impl CrateInfo {
    /// The crate's `#![feature(..)]` declarations.
    pub fn features(&self, prog: &Program) -> &crate::features::Features {
        self.features.get_or_init(|| crate::features::Features::read(prog, prog.mods[self.root_mod as usize].file, &self.cfg))
    }

    /// the crate's root source file
    pub fn root_path(&self) -> &str {
        &self.root_path
    }
    /// The relaxed-Rust features this crate opts in to (std crates: none).
    pub fn relaxed(&self, prog: &Program) -> &[String] {
        self.relaxed.get_or_init(|| {
            if matches!(prog.syms.str(self.name), "core" | "alloc" | "std" | "proc_macro") {
                Vec::new()
            } else {
                crate::relaxed::crate_features(&prog.session.options, &self.root_path)
            }
        })
    }
}

/// Reverse edges for post-boot import resolution. These are indexes of the existing
/// import/glob/wait graph, not lookup caches. Bulk resolution discards them; a
/// restored collected image builds them once on its first deferred load.
#[derive(Default)]
struct LoadGraph {
    globs: HashMap<u32, crate::fxhash::HashSet<u32>>,
    lexical: HashMap<u32, Vec<u32>>,
    stops: HashMap<usize, u32>,
    at_stop: HashMap<u32, crate::fxhash::HashSet<usize>>,
    waiters: HashMap<u32, crate::fxhash::HashSet<u32>>,
    modules: usize,
}

impl LoadGraph {
    fn stop(&mut self, i: usize, stop: Option<u32>) {
        if let Some(old) = self.stops.remove(&i) {
            if let Some(v) = self.at_stop.get_mut(&old) {
                v.remove(&i);
            }
        }
        if let Some(stop) = stop {
            self.stops.insert(i, stop);
            self.at_stop.entry(stop).or_default().insert(i);
        }
    }

    fn glob_closure(&self, seeds: impl IntoIterator<Item = u32>) -> crate::fxhash::HashSet<u32> {
        let mut found = crate::fxhash::HashSet::default();
        let mut todo: Vec<_> = seeds.into_iter().collect();
        while let Some(m) = todo.pop() {
            if !found.insert(m) {
                continue;
            }
            if let Some(v) = self.globs.get(&m) {
                todo.extend(v);
            }
            if let Some(v) = self.lexical.get(&m) {
                todo.extend(v);
            }
        }
        found
    }
}

/// Filter using MemoMap's safe snapshot API. The exclusive Program borrow excludes
/// readers, so replacing a table also frees its retired storage. No lookup is redone.
fn retain_load_memos<K: Clone + std::hash::Hash + Eq, V: Clone>(
    cache: &mut crate::conc::MemoMap<K, V>,
    mut keep: impl FnMut(&K, &V) -> bool,
) -> (usize, usize) {
    let mut entries = cache.snapshot();
    let before = entries.len();
    entries.retain(|k, v| keep(k, v));
    let kept = entries.len();
    if kept != before {
        *cache = Default::default();
        for (k, v) in entries {
            cache.insert(k, v);
        }
    }
    (kept, before - kept)
}

pub struct Program {
    collected_starts: Vec<collected::Start>,
    pub session: std::sync::Arc<crate::Session>,
    /// Stable authored source IDs, shared with preload/lowering workers for this load.
    pub sources: std::sync::Arc<crate::source::SourceRegistry>,
    pub demand_plan: Option<std::sync::Arc<crate::preload::DemandPlan>>,
    pub load_recipe: Vec<crate::preload::PreCrate>,
    pub module_probes: Vec<(String, bool)>,
    /// `mod name;` without `#[path]` that found `name.rs`: (file, item position, krate, the `name/mod.rs` alternative)
    /// -- a check reports E0761 when that exists too (rustc default_submod_path)
    /// per module being collected: the folder its `mod x;` items resolve in and whether that
    /// folder is the module file's own (rustc ModuleData::dir_path / DirOwnership); the modules
    /// of a fn body's blocks take their enclosing module's
    mod_dirs: crate::fxhash::HashMap<u32, (String, bool)>,
    source_versions: HashMap<u32, std::sync::Arc<crate::source_context::Context>>,
    /// Files committed by a live edit (commit_version). Their parse is eager (ParseSnapshot::
    /// parse_full), so their expression and block ids differ from a boot parse of the same
    /// text (declarations, then the skipped bodies' part, then generated bodies): the code
    /// cache never keys or resolves closures, sites or positions through them.
    edit_versions: Vec<u32>,
    /// Committed files used only during a warm checker crate recollection.
    check_files: Option<std::collections::HashMap<String, u32>>,
    parse_reuse: HashMap<u32, std::sync::Arc<crate::parse_work::Reuse>>,
    /// files read and lexed by the macro prescan, consumed by load_file
    pub prelexed: HashMap<String, (Vec<u8>, Vec<crate::lexer::Tok>)>,
    /// glob lookups after import resolution: (module, name, type ns) -> result
    pub glob_cache: crate::conc::MemoMap<(u32, Sym, bool), Option<(DefId, BindVis)>>,
    /// `glob_ambiguity` answers once imports are resolved: (module, name, namespace, used from, skip_direct)
    pub glob_amb_cache: crate::conc::MemoMap<(u32, Sym, bool, bool), u8>,
    /// precise lookups of a name in a glob's module (what `glob_ambiguity` compares) once imports are resolved
    pub glob_prec_cache: crate::conc::MemoMap<(u32, Sym, bool), Option<(DefId, BindVis)>>,
    /// the crates being checked (`makepad check`): the only ones whose paths are privacy-checked
    pub own_crates: Vec<bool>,
    /// rustc's glob ambiguities in the checked crates (the import replay, program/rsim.rs):
    /// (module, name, type namespace) -> 1 an error, 2 the lint `ambiguous_glob_imports`
    pub sim_ambiguity: Option<HashMap<(u32, Sym, bool), u8>>,
    /// what the replay reported, until the check emits it
    pub sim_diags: Option<Vec<SimDiag>>,
    /// lexical lookups (`lookup_in_scope`) once imports are resolved: (module, name, ns_type);
    /// cleared whenever a binding is defined or imports are resolved again
    scope_cache: crate::conc::MemoMap<(u32, Sym, bool), Option<DefId>>,
    /// the import bindings (module, bound name, type namespace) and the glob imports (module,
    /// glob target) a name resolution went through (rustc_resolve `record_use`); recorded only
    /// while `record_import_uses` is on (`makepad check`)
    import_used: relax_session::sync::Mutex<crate::fxhash::HashSet<(u32, Sym, bool)>>,
    glob_used: relax_session::sync::Mutex<crate::fxhash::HashSet<(u32, DefId)>>,
    /// extern prelude entries (crate, name) a path resolved through (recorded like `import_used`)
    extern_used: relax_session::sync::Mutex<crate::fxhash::HashSet<(u32, Sym)>>,
    /// the per-occurrence memo tables (scope lookups, path segments) are filled only once
    /// bodies are being checked: during collection nearly every lookup is unique, and a memo
    /// would cost more than it saves (set by jit::Unit::new)
    pub body_memos: std::sync::atomic::AtomicBool,
    /// `path_segs` per path occurrence: (file, segment count, first lo, a hash of every
    /// segment's span) -> the segments' kinds and symbols (a pure function of the file's text;
    /// cleared when a file is replaced)
    pub path_seg_cache: crate::conc::MemoMap<(u32, u32, u32, u64), std::sync::Arc<[(crate::ast::SegKind, Sym)]>>,
    /// traits each module's scope binds, with visibility (after import resolution)
    trait_bind_cache: crate::conc::MemoMap<u32, std::sync::Arc<Vec<(DefId, BindVis)>>>,
    /// imports are resolved (glob results no longer change)
    pub imports_done: bool,
    /// Rust file selection: imports left unresolved because their lookup stops in
    /// a skipped module (or one that depends on such an import), with the name
    /// each would bind (None: a glob). Used only by MAKEPAD_RUN_LAZY_VERIFY.
    pub deferred_imports: Vec<(u32, Option<Sym>)>,
    /// Rust file selection: modules whose file was skipped -> (file, its children's directory,
    /// is a mod.rs). load_deferred_module loads one after boot.
    pub skipped_files: HashMap<u32, (String, String, bool)>,
    /// After boot: a lookup that misses in one of these modules may be missing a deferred
    /// module's names. module -> the skipped modules it waits on (itself when skipped).
    deferred_waits: HashMap<u32, Vec<u32>>,
    /// After boot: a deferred (non-glob) import's binding -> the skipped modules its lookup
    /// stopped in. A miss of exactly that name loads them whatever their outlines bind.
    deferred_binds: HashMap<(u32, Sym), Vec<u32>>,
    /// (module, glob target) -> the import that made the glob: a module's globs are searched in
    /// their imports' source order, whatever order the import passes resolved them in (under
    /// lazy loading that order depends on what was loaded; two globs binding one name must not
    /// change meaning with it).
    glob_import: HashMap<(u32, DefId), u32>,
    /// imports by the module they are in (built by resolve_imports_fixpoint)
    imports_of_module: HashMap<u32, Vec<u32>>,
    /// The import fixpoint stalled: an import that found one namespace while a glob it
    /// looks through is still open takes what it found (resolve_imports_passes).
    import_settle: bool,
    load_graph: Option<LoadGraph>,
    /// module path of the file being loaded (`module_path!()`)
    pub cur_mod_path: String,
    /// the module files being loaded, outermost first (rustc_expand circular module check)
    pub module_load_stack: Vec<String>,
    /// `macro_rules!` definitions seen so far (files parse in load order)
    pub macros: crate::collected::Deferred<std::sync::Arc<crate::macros::MacroTable>>,
    /// per crate root: what the preload found (macro definitions, files; preload.rs)
    ahead: HashMap<(String, String), u32>,
    /// the preload's parse workers and the files submitted to them not yet loaded
    pool: Option<std::sync::Arc<crate::preload::Pool>>,
    pool_workers: Vec<std::thread::JoinHandle<()>>,
    pending: std::collections::HashSet<(u32, String)>,
    /// preload index of the crate being loaded
    cur_ahead: u32,
    cur_image: bool,
    /// per file: the body blocks with items below them (FilePrep::blocks), for collect
    file_blocks: crate::collected::ImageVec<BodyBlocks>,
    /// Exact loader metadata, encoded immediately to avoid retaining macro-table snapshots.
    pub front_ahead: Vec<Vec<u8>>,
    pub front_graph: Option<crate::crate_image::GraphRecipe>,
    pub front_restored: Vec<bool>,
    /// per crate: its dependencies as preloaded (Pool::dep_names), for its crate image
    pub front_dep_names: Vec<Vec<(String, String)>>,
    pub front_probes: Vec<Vec<(String, bool)>>,
    pub front_files: Vec<(u32, usize, Vec<u8>)>,
    /// Fragments expanded from already-recorded item macro calls. Their
    /// authored inputs and macro definitions belong to the ordinary files;
    /// replay them after loading an image, rather than treating their partial
    /// source buffers as additional files on disk.
    pub front_macro_fragments: crate::fxhash::HashSet<usize>,
    pub record_front: bool,
    /// Writes each published file's image and maps its arrays from it (ImageSink::file; None:
    /// files stay on the heap).
    pub front_sink: Option<std::sync::Arc<dyn crate::parser::ImageSink>>,
    /// lines of the loaded files (with expansion text)
    pub lines_loaded: usize,
    /// The files the boot loaded (they come first; the post-boot loader appends the files the
    /// lazy selection skipped). The crate interfaces of the code cache cover these, so a
    /// collected program restored with every file loaded keeps the boot's interfaces.
    pub boot_files: usize,
    exec_profile_files: std::sync::OnceLock<crate::conc::MemoMap<u32, std::sync::Arc<crate::exec_profile::File>>>,
    pub syms: Interner,
    /// shared with the proc-macro world for std files (SHARED_STD)
    pub files: Vec<std::sync::Arc<ParsedFile>>,
    /// the proc-macro world's program: its core/alloc/std files come from the program's load
    /// (SHARED_STD) when that load runs
    pub std_from_shared: bool,
    /// the std crates include core::simd / std::simd (relax_portable_simd; `std_simd_wanted`)
    pub std_simd: bool,
    /// item-level macro calls no macro resolved at their parse (file, item, module): expanded
    /// again once the crate is loaded (expand_pending: rustc's resolve/expand fixpoint for
    /// macros an expansion in a later file defines or imports), else errors
    pending_macs: Vec<(u32, ItemId, u32)>,
    /// `global_asm!` items (unless `--no-asm`): (file, item, module), assembled and placed when
    /// the program's code is set up (compiler global_asm.rs)
    pub global_asms: Vec<(u32, ItemId, u32)>,
    /// files whose lazily parsed part has its identifiers interned here (part_idents)
    part_interned: relax_session::sync::Mutex<crate::fxhash::HashSet<(u32, u32)>>,
    pub file_paths: Vec<String>,
    pub file_crate: Vec<u32>,
    pub crates: Vec<CrateInfo>,
    pub defs: crate::arena::Arena<Def>,
    pub mods: crate::collected::ImageVec<Module>,
    pub variants: crate::collected::ImageVec<Vec<DefId>>,
    pub impls: crate::collected::ImageVec<ImplData>,
    pub traits: crate::collected::ImageVec<Vec<DefId>>,
    pub imports: crate::collected::ImageVec<Import>,
    /// (file, block) -> anonymous module holding the block's items
    pub block_mods: crate::collected::ImageMap<(u32, u32), u32>,
    /// block module -> the fn (or assoc fn) whose body it is in (item paths: `main::Wrapper`)
    pub block_owner: crate::collected::ImageMap<u32, DefId>,
    /// (file, item) -> def
    pub item_defs: crate::collected::ImageMap<(u32, u32), DefId>,
    // Current ownership; old versions keep only entities not paired into the new file.
    /// Entities by file, for reloads. A restored program builds it on first use.
    reload: std::sync::OnceLock<ReloadFiles>,
    pub node_id_to_def_id: crate::collected::ImageMap<(u32, crate::def_collector::NodeId), DefId>,
    pub legacy_const_args: crate::collected::ImageMap<DefId, std::sync::Arc<crate::def_collector::LegacyOwner>>,
    pub(crate) legacy_const_attrs: std::sync::Arc<HashMap<DefId, Vec<u32>>>,
    declaration_versions: HashMap<u32, DeclarationState>,
    pub def_id_to_node_id: crate::collected::ImageMap<DefId, (u32, crate::def_collector::NodeId)>,
    pub impl_trait_in_trait_data: crate::collected::ImageMap<DefId, crate::assoc::ImplTraitInTraitData>,
    pub associated_types_for_impl_traits: crate::collected::ImageMap<DefId, Vec<DefId>>,
    pub opaque_to_rpitit: crate::collected::ImageMap<DefId, DefId>,
    pub opaque_types_defined_by: crate::collected::ImageMap<DefId, Vec<DefId>>,
    pub trait_item_def_id: crate::collected::ImageMap<DefId, DefId>,
    pub repeat_expr_counts: crate::collected::ImageSet<DefId>,
    pub(crate) collected_definitions: crate::collected::ImageSet<DefId>,
    /// Per collected owner: the call-path names its legacy const generic lowering would look
    /// up (def_collector::collect_definitions records them; an owner restored without them is
    /// walked). Not part of the collected image.
    pub(crate) legacy_call_names: HashMap<DefId, Vec<Sym>>,
    pub(crate) def_disambiguators: crate::collected::ImageMap<(DefId, DefKind, Option<Sym>), u32>,
    pub(crate) child_definitions: crate::collected::ImageMap<(DefId, DefKind, Option<Sym>, u32), DefId>,
    /// What a retained collected prefix lost to the on-demand definitions it dropped
    /// (retain_collected_prefix): the suffix's resolve links and lowers it again. Not part of the image.
    pub(crate) collected_relink: Option<collected::Relink>,
    /// The retained definitions the declaration tables collect again after such a resolve:
    /// the methods whose signatures named a dropped item, and their traits.
    pub collected_recollect: Vec<DefId>,
    pub prim_defs: Vec<DefId>,
    /// index of the crate whose `prelude` module supplies the std prelude
    pub prelude_crate: Option<u32>,
    /// the std prelude's module and its four edition modules, found once (`prelude_ids`)
    prelude_ids: std::sync::OnceLock<PreludeIds>,
    /// per module: the traits in scope for method calls (computed on first use)
    trait_scope: crate::conc::MemoMap<u32, std::sync::Arc<crate::fxhash::HashSet<DefId>>>,
    pub errors: Vec<String>,
}

#[derive(Default)]
struct ReloadFiles {
    defs: HashMap<u32, Vec<u32>>,
    impls: HashMap<u32, Vec<u32>>,
    mods: HashMap<u32, Vec<u32>>,
}

/// Declaration results are version state, including signature-owned RPITITs.
/// The front writer is the sole publisher; readers cannot observe a half commit.
struct DeclarationState {
    legacy: HashMap<DefId, std::sync::Arc<crate::def_collector::LegacyOwner>>,
    // Lead decision 52 (R1): the crates whose owners a scoped declaration pass re-lowers
    // (lower_legacy_const_generics_in); None: every crate.
    visible: Option<Vec<bool>>,
    environment: Option<(std::sync::Arc<HashMap<DefId, Vec<u32>>>, AssocState)>,
    errors: Vec<String>,
}
struct AssocState {
    rpitit: HashMap<DefId, crate::assoc::ImplTraitInTraitData>,
    associated: HashMap<DefId, Vec<DefId>>,
    opaque: HashMap<DefId, DefId>,
    trait_items: HashMap<DefId, DefId>,
    /// (index, items) of the traits / impls saved: all of them, or only those of the visible crates
    traits: Vec<(usize, Vec<DefId>)>,
    impl_items: Vec<(usize, Vec<DefId>)>,
}
impl DeclarationState {
    fn save(prog: &Program, file: u32, body_only: bool, visible: Option<&[bool]>) -> Self {
        let sees = |d: DefId| visible.map_or(true, |v| v.get(prog.def(d).krate as usize).copied().unwrap_or(true));
        let mut legacy = HashMap::default();
        if body_only {
            for &d in prog.defs_in_file(file) {
                if let Some(result) = prog.legacy_const_args.get(&DefId(d)) { legacy.insert(DefId(d), result.clone()); }
            }
        } else if visible.is_some() {
            for d in prog.defs_in_visible_files(visible) {
                if let Some(result) = prog.legacy_const_args.get(&d) { legacy.insert(d, result.clone()); }
            }
        } else {
            for (&owner, result) in &prog.legacy_const_args { legacy.insert(owner, result.clone()); }
        }
        // A scoped pass (assoc::collect_in) rewrites only what belongs to the visible crates, so
        // that is all the rollback needs; the maps are filtered, the item lists of other crates'
        // traits and impls are not copied.
        let environment = if body_only { None } else { Some((prog.legacy_const_attrs.clone(), AssocState {
            rpitit: prog.impl_trait_in_trait_data.iter().filter(|(k, _)| sees(**k)).map(|(k, v)| (*k, v.clone())).collect(),
            associated: prog.associated_types_for_impl_traits.iter().filter(|(k, _)| sees(**k)).map(|(k, v)| (*k, v.clone())).collect(),
            opaque: prog.opaque_to_rpitit.iter().filter(|(k, _)| sees(**k)).map(|(k, v)| (*k, v.clone())).collect(),
            trait_items: prog.trait_item_def_id.iter().filter(|(k, _)| sees(**k)).map(|(k, v)| (*k, v.clone())).collect(),
            traits: prog.traits.iter().enumerate().filter(|(_, items)| visible.is_none() || items.first().is_some_and(|&m| sees(m))).map(|(i, items)| (i, items.clone())).collect(),
            impl_items: prog.impls.iter().enumerate().filter(|(_, im)| sees(im.def)).map(|(i, im)| (i, im.items.clone())).collect(),
        })) };
        Self { legacy, visible: visible.map(|v| v.to_vec()), environment, errors: prog.errors.clone() }
    }
    fn restore(self, prog: &mut Program) {
        for result in self.legacy.values() { crate::def_collector::restore_legacy_children(prog, result); }
        if let Some((attrs, assoc)) = self.environment {
            match &self.visible {
                Some(visible) => {
                    for d in prog.defs_in_visible_files(Some(visible)) { prog.legacy_const_args.remove(&d); }
                }
                None => prog.legacy_const_args.clear(),
            }
            prog.legacy_const_attrs = attrs;
            match &self.visible {
                Some(visible) => {
                    let defs = &prog.defs;
                    let seen = |d: &DefId| visible.get(defs[d.0 as usize].krate as usize).copied().unwrap_or(true);
                    prog.impl_trait_in_trait_data.retain(|d, _| !seen(d));
                    prog.associated_types_for_impl_traits.retain(|d, _| !seen(d));
                    prog.opaque_to_rpitit.retain(|d, _| !seen(d));
                    prog.trait_item_def_id.retain(|d, _| !seen(d));
                    prog.impl_trait_in_trait_data.extend(assoc.rpitit);
                    prog.associated_types_for_impl_traits.extend(assoc.associated);
                    prog.opaque_to_rpitit.extend(assoc.opaque);
                    prog.trait_item_def_id.extend(assoc.trait_items);
                }
                None => {
                    prog.impl_trait_in_trait_data = assoc.rpitit.into_iter().collect();
                    prog.associated_types_for_impl_traits = assoc.associated.into_iter().collect();
                    prog.opaque_to_rpitit = assoc.opaque.into_iter().collect();
                    prog.trait_item_def_id = assoc.trait_items.into_iter().collect();
                }
            }
            for (i, items) in assoc.traits { prog.traits[i] = items; }
            for (i, items) in assoc.impl_items { prog.impls[i].items = items; }
        }
        prog.legacy_const_args.extend(self.legacy);
        prog.errors = self.errors;
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        if let Some(pool) = &self.pool { pool.stop(); }
        for worker in std::mem::take(&mut self.pool_workers) { let _ = worker.join(); }
    }
}

impl Program {
    /// The name of the crate `file` belongs to ("" before the file is assigned to one).
    pub fn file_crate_name(&self, file: u32) -> String {
        match self.file_crate.get(file as usize).and_then(|&k| self.crates.get(k as usize)) {
            Some(c) => self.syms.str(c.name).to_string(),
            None => String::new(),
        }
    }

    pub fn new(sess: &std::sync::Arc<crate::Session>) -> Program {
        let mut p = Program {
            collected_starts: Vec::new(),
            session: sess.clone(),
            sources: Default::default(),
            demand_plan: None,
            load_recipe: Vec::new(),
            module_probes: Vec::new(),
            mod_dirs: Default::default(),
            source_versions: Default::default(),
            edit_versions: Vec::new(),
            check_files: None,
            parse_reuse: Default::default(),
            syms: Interner::default(),
            files: Vec::new(),
            std_from_shared: false,
            std_simd: false,
            part_interned: Default::default(),
            pending_macs: Vec::new(),
            global_asms: Vec::new(),
            file_paths: Vec::new(),
            file_crate: Vec::new(),
            crates: Vec::new(),
            defs: Default::default(),
            mods: Default::default(),
            variants: Default::default(),
            impls: Default::default(),
            traits: Default::default(),
            imports: Default::default(),
            block_mods: Default::default(),
            block_owner: Default::default(),
            item_defs: Default::default(),
            reload: std::sync::OnceLock::from(ReloadFiles::default()),
            node_id_to_def_id: Default::default(),
            legacy_const_args: Default::default(),
            legacy_const_attrs: Default::default(),
            declaration_versions: Default::default(),
            def_id_to_node_id: Default::default(),
            impl_trait_in_trait_data: Default::default(),
            associated_types_for_impl_traits: Default::default(),
            opaque_to_rpitit: Default::default(),
            opaque_types_defined_by: Default::default(),
            trait_item_def_id: Default::default(),
            repeat_expr_counts: Default::default(),
            collected_definitions: Default::default(),
            legacy_call_names: Default::default(),
            def_disambiguators: Default::default(),
            child_definitions: Default::default(),
            collected_relink: None,
            collected_recollect: Vec::new(),
            prim_defs: Vec::new(),
            prelude_crate: None,
            prelude_ids: Default::default(),
            trait_scope: Default::default(),
            errors: Vec::new(),
            macros: Default::default(),
            glob_cache: Default::default(),
            glob_amb_cache: Default::default(),
            glob_prec_cache: Default::default(),
            own_crates: Vec::new(),
            sim_ambiguity: None,
            sim_diags: None,
            scope_cache: Default::default(),
            import_used: Default::default(),
            glob_used: Default::default(),
            extern_used: Default::default(),
            body_memos: Default::default(),
            path_seg_cache: Default::default(),
            trait_bind_cache: Default::default(),
            prelexed: Default::default(),
            ahead: Default::default(),
            pool: None,
            pool_workers: Vec::new(),
            pending: Default::default(),
            cur_ahead: 0,
            cur_image: false,
            file_blocks: Default::default(),
            front_ahead: Vec::new(),
            front_graph: None,
            front_restored: Vec::new(),
            front_dep_names: Vec::new(),
            front_probes: Vec::new(),
            front_files: Vec::new(),
            front_macro_fragments: crate::fxhash::HashSet::default(),
            record_front: false,
            front_sink: None,
            lines_loaded: 0,
            boot_files: usize::MAX,
            exec_profile_files: Default::default(),
            imports_done: false,
            deferred_imports: Vec::new(),
            skipped_files: HashMap::default(),
            deferred_waits: HashMap::default(),
            deferred_binds: HashMap::default(),
            glob_import: HashMap::default(),
            imports_of_module: HashMap::default(),
            import_settle: false,
            load_graph: None,
            cur_mod_path: String::new(),
            module_load_stack: Vec::new(),
        };
        for kw in ["Self", "self", "super", "crate", "prelude", "core", "std"] {
            p.syms.intern(kw);
        }
        for (name, prim) in PRIMS {
            let s = p.syms.intern(name);
            let d = p.add_def(Def { kind: DefKind::Prim(prim), name: s, parent: NO_DEF, file: 0, item: ItemId(0), sub: 0, is_pub: true, krate: 0, scope: 0 });
            p.prim_defs.push(d);
        }
        p
    }

    /// Rebuild once when restoring a collected image; ordinary loading maintains these
    /// at insertion. These indexes are derived state, not part of the image format.
    fn reload_files(&self) -> &ReloadFiles {
        self.reload.get_or_init(|| {
            let mut r = ReloadFiles::default();
            for (i, d) in self.defs.iter().enumerate() { r.defs.entry(d.file).or_default().push(i as u32); }
            for (i, d) in self.impls.iter().enumerate() { r.impls.entry(d.file).or_default().push(i as u32); }
            for (i, d) in self.mods.iter().enumerate() { r.mods.entry(d.file).or_default().push(i as u32); }
            r
        })
    }
    fn reload_files_mut(&mut self) -> &mut ReloadFiles {
        self.reload_files();
        self.reload.get_mut().unwrap()
    }
    /// Every def of the files of the visible crates (None: every crate's), from the per-file index.
    pub fn defs_in_visible_files(&self, visible: Option<&[bool]>) -> Vec<DefId> {
        (0..self.files.len() as u32)
            .filter(|&f| visible.map_or(true, |v| v.get(self.file_crate[f as usize] as usize).copied().unwrap_or(true)))
            .flat_map(|f| self.defs_in_file(f).iter().map(|&d| DefId(d))).collect()
    }
    pub fn defs_in_file(&self, file: u32) -> &[u32] {
        self.reload_files().defs.get(&file).map_or(&[], Vec::as_slice)
    }

    pub fn set_def_file(&mut self, def: DefId, file: u32) {
        let old = self.defs[def.0 as usize].file;
        if old == file { return; }
        let r = self.reload_files_mut();
        if let Some(ids) = r.defs.get_mut(&old) { ids.retain(|i| *i != def.0); }
        r.defs.entry(file).or_default().push(def.0);
        self.defs[def.0 as usize].file = file;
    }
    pub fn set_mod_file(&mut self, module: u32, file: u32) {
        let krate = self.mods[module as usize].krate as usize;
        if self.crates[krate].root_mod == module {
            self.crates[krate].recursion_limit = recursion_limit(&self.files[file as usize]);
        }
        let old = self.mods[module as usize].file;
        if old == file { return; }
        let r = self.reload_files_mut();
        if let Some(ids) = r.mods.get_mut(&old) { ids.retain(|i| *i != module); }
        r.mods.entry(file).or_default().push(module);
        self.mods[module as usize].file = file;
    }

    pub fn add_def(&mut self, d: Def) -> DefId {
        // an index not built yet finds the new definition when it is built
        if let Some(r) = self.reload.get_mut() { r.defs.entry(d.file).or_default().push(self.defs.len() as u32); }
        self.defs.push(d);
        DefId(self.defs.len() as u32 - 1)
    }

    /// The enclosing type-checking owner, through nested opaque definitions.
    pub fn opaque_owner(&self, mut d: DefId) -> DefId {
        while self.def(d).kind == DefKind::OpaqueTy { d = self.def(d).parent; }
        d
    }

    #[inline]
    pub fn def(&self, d: DefId) -> &Def {
        &self.defs[d.0 as usize]
    }

    pub fn name(&self, d: DefId) -> &str {
        self.syms.str(self.defs[d.0 as usize].name)
    }

    pub fn prim_def(&self, p: Prim) -> DefId {
        for i in 0..PRIMS.len() {
            if PRIMS[i].1 == p {
                return self.prim_defs[i];
            }
        }
        NO_DEF
    }

    pub fn src(&self, file: u32) -> Src<'_> {
        self.files[file as usize].src_view()
    }

    /// Cached per-file profile handle; no path lookup or global lock after its
    /// first use. File IDs are append-only, including deferred loads and edits.
    pub fn exec_profile_file(&self, file: u32) -> Option<std::sync::Arc<crate::exec_profile::File>> {
        if !self.session.runtime.profile { return None; }
        let files = self.exec_profile_files.get_or_init(Default::default);
        if let Some(f) = files.get(&file) { return Some(f); }
        let f = crate::exec_profile::file(self.file_paths.get(file as usize)?);
        f.note(crate::exec_profile::LOADED);
        files.insert(file, f.clone());
        Some(f)
    }

    /// A fn item's body block: the one in its file's tree, or its deferred body.
    /// Rust demand mode materializes only the queried item. Err: that body's parse error.
    pub fn fn_body(&self, file: u32, item: ItemId) -> Result<Option<BlockId>, String> {
        if let Some(f) = self.exec_profile_file(file) { f.note(crate::exec_profile::BODY); }
        let pf = &self.files[file as usize];
        let body = match &pf.ast.item(item).kind {
            ItemKind::Fn(_, Some(b), _) => Ok(Some(*b)),
            ItemKind::Fn(..) => match pf.lazy_body(item, &self.crates[self.file_crate[file as usize] as usize].cfg) {
                Some(r) => r.map(|x| Some(x.0)),
                None => Ok(None),
            },
            _ => Ok(None),
        };
        self.part_idents(file);
        body
    }

    /// Return a const/static initializer in its declaration's context.
    pub fn const_body(&self, file: u32, item: ItemId) -> Result<Option<ExprId>, String> {
        if let Some(f) = self.exec_profile_file(file) { f.note(crate::exec_profile::BODY); }
        let pf = &self.files[file as usize];
        Ok(match pf.ast.item(item).kind {
            ItemKind::Const(_, e) | ItemKind::Static(_, _, e) => e.get(),
            _ => None,
        })
    }

    /// The expression/pattern ranges of an item and its demanded body.
    pub fn body_ranges(&self, file: u32, item: ItemId) -> Result<([u32; 4], Option<[u32; 4]>), String> {
        if let Some(f) = self.exec_profile_file(file) { f.note(crate::exec_profile::BODY); }
        let pf = &self.files[file as usize];
        let r = pf.ast.item(item).ranges;
        let lazy = pf.lazy_body(item, &self.crates[self.file_crate[file as usize] as usize].cfg).transpose()?.map(|b| b.1);
        self.part_idents(file);
        Ok((r, lazy))
    }

    /// Interns the identifiers of a file's lazily parsed bodies' expansions (the file's own
    /// tokens were interned at load), once, after parsing them.
    pub fn part_idents(&self, file: u32) {
        let pf = &self.files[file as usize];
        if pf.lazy.is_none() {
            return;
        }
        if pf.part(&self.crates[self.file_crate[file as usize] as usize].cfg).is_none() { return; }
        // per program: a std file's part is shared with the proc-macro world (SharedParse),
        // each program interns its identifiers in its own table
        for part in pf.parts() {
            if part.toks.is_empty() { continue; }
            let mut interned = self.part_interned.lock().unwrap_or_else(|e| e.into_inner());
            if !interned.insert((file, part.tok_base)) { continue; }
            for t in part.toks.iter() {
                let t = &t;
                if matches!(t.kind, crate::lexer::T::Ident | crate::lexer::T::RawIdent | crate::lexer::T::Lifetime) {
                    let b = pf.bytes(t.lo, t.hi);
                    let b = if t.kind == crate::lexer::T::Lifetime { &b[1.min(b.len())..] } else { b.strip_prefix(b"r#").unwrap_or(b) };
                    if let Ok(s) = std::str::from_utf8(b) { self.syms.intern_late(s); }
                }
            }
        }
    }

    /// The expression id ranges of an item, its lazily parsed body's included (parsing it).
    pub fn item_expr_ranges(&self, file: u32, item: ItemId) -> Vec<(u32, u32)> {
        match self.body_ranges(file, item) {
            Ok((r, r2)) => {
                let mut v = vec![(r[0], r[1])];
                if let Some(r2) = r2 {
                    v.push((r2[0], r2[1]));
                }
                v
            }
            Err(_) => {
                let r = self.files[file as usize].ast.item(item).ranges;
                vec![(r[0], r[1])]
            }
        }
    }

    pub fn legacy_const_call(&self, mut owner: DefId, file: u32, expr: ExprId) -> Option<&crate::def_collector::LegacyConstArgs> {
        while owner != NO_DEF {
            if let Some(result) = self.legacy_const_args.get(&owner) {
                return if result.file == file { result.calls.get(&expr) } else { None };
            }
            owner = self.def(owner).parent;
        }
        None
    }

    pub fn ident(&mut self, file: u32, id: Ident) -> Sym {
        let f = &self.files[file as usize];
        let s = std::str::from_utf8(f.bytes(id.lo, id.hi)).unwrap_or("?");
        let s = match s.strip_prefix("r#") {
            Some(x) => x,
            None => s,
        };
        // every identifier of a loaded file is interned already (load_file)
        if let Some(x) = self.syms.get(s) {
            return x;
        }
        let owned = s.to_string();
        self.syms.intern(&owned)
    }

    pub fn why_def(&self, def: DefId, kind: &str) -> crate::why::Cause {
        let d = self.def(def);
        let pos = self.files[d.file as usize].ast.item(d.item).lo;
        let loc = self.source_location(d.file, pos);
        crate::why::Cause::new(loc.source.path(), loc.source.line_col(loc.span.lo).0, kind, &format!("{} [def:{}]", self.def_path(def), def.0))
    }

    pub fn why_expr(&self, file: u32, pos: u32, def: DefId, kind: &str) -> crate::why::Cause {
        let loc = self.source_location(file, pos);
        crate::why::Cause::new(loc.source.path(), loc.source.line_col(loc.span.lo).0, kind, &self.def_path(def))
    }

    /// Readable path of a def, for messages.
    pub fn def_path(&self, d: DefId) -> String {
        // the names borrowed outermost-last, then one String (a name per slot: every new slot)
        let mut parts: [&str; 64] = [""; 64];
        let mut n = 0;
        let mut len = 0;
        let mut cur = d;
        while cur != NO_DEF && n < 64 {
            let def = self.def(cur);
            parts[n] = self.syms.str(def.name);
            len += parts[n].len() + 2;
            n += 1;
            cur = def.parent;
        }
        let mut out = String::with_capacity(len);
        for (i, p) in parts[..n].iter().rev().enumerate() {
            if i > 0 { out.push_str("::"); }
            out.push_str(p);
        }
        out
    }

    /// Current module versions, one per authored path (excluding deferred macro fragments).
    pub fn current_source_files(&self) -> std::collections::BTreeMap<String, u32> {
        let mut files = std::collections::BTreeMap::new();
        for module in &self.mods {
            if let Some(path) = self.file_paths.get(module.file as usize) {
                files.insert(path.clone(), module.file);
            }
        }
        files
    }

    // ------------------------------------------------------------ loading

    /// Loads a crate from its root file: parses every active module file.
    pub fn load_crate(&mut self, name: &str, root: &str, edition: u16, cfg: CfgSet, deps: Vec<(Sym, u32)>) -> Result<u32, String> {
        // Standalone callers may consume declarations without resolving imports later.
        let first = self.defs.len();
        let krate = self.load_crate_deferred(name, root, edition, cfg, deps)?;
        let mut own = vec![false; self.crates.len()];
        own[krate as usize] = true;
        crate::assoc::collect_loaded(self, first, &own);
        Ok(krate)
    }

    /// A batch loader finishes associated declarations once, after resolving imports.
    pub fn load_crate_deferred(&mut self, name: &str, root: &str, edition: u16, cfg: CfgSet, deps: Vec<(Sym, u32)>) -> Result<u32, String> {
        self.mark_collected_start();
        let _mem = crate::memstat::tag(crate::memstat::Tag::ProgramItems);
        let krate = self.crates.len() as u32;
        if name == "core" {
            // S2 fmt ABI (std core has core::fmt::rt): format macros expand to it (fmtmac.rs);
            // every loader (driver, bring-up checker) goes through here
            let dir = std::path::Path::new(root).parent().map(|p| p.to_path_buf()).unwrap_or_default();
            std::sync::Arc::make_mut(&mut self.macros).s2_fmt = crate::vfs::exists(dir.join("fmt/mod.rs"));
        }
        let cname = self.syms.intern(name);
        let root_def = self.add_def(Def { kind: DefKind::Mod, name: cname, parent: NO_DEF, file: 0, item: ItemId(0), sub: 0, is_pub: true, krate, scope: 0 });
        let root_mod = self.new_module(root_def, None, None, krate, 0);
        self.defs[root_def.0 as usize].sub = root_mod;
        self.defs[root_def.0 as usize].scope = root_mod;
        self.crates.push(CrateInfo { name: cname, root_mod, edition, recursion_limit: 128, deps, cfg, proc_macro: false, is_bin: false, validate: false, relaxed: Default::default(), features: Default::default(), root_path: root.to_string(), context: crate::source_context::Context::new(root, self.sources.clone(), &self.session) });
        let kname = self.syms.str(cname).to_string();
        let akey = (kname.clone(), root.to_string());
        if !self.ahead.contains_key(&akey) {
            if crate::preload::trace() {
                eprintln!("boot-trace: {} ({}) was not preloaded", kname, root);
            }
            // A single-crate caller has not supplied its future consumers or
            // runtime entry names. Keep that boundary eager; a complete preload
            // graph (the run/makepad drivers) can safely close demand first.
            self.start_preload(vec![crate::preload::PreCrate {
                name: kname.clone(),
                root: root.to_string(),
                edition,
                cfg: self.crates[krate as usize].cfg.clone(),
                deps: Vec::new(),
                proc_macro: false,
                image_deps: Vec::new(),
                image_dep_names: Vec::new(),
            }], None, false, true);
        }
        let ci = self.ahead.remove(&akey).unwrap();
        let _reads = self.pool.as_ref().unwrap().bind_reads();
        self.crates[krate as usize].context = self.pool.as_ref().unwrap().context(ci);
        let ahead = self.pool.as_ref().unwrap().take_ahead(ci);
        self.module_probes.extend(ahead.probes.iter().cloned());
        if self.record_front {
            // A restored crate already owns its disk metadata; do not re-encode its macro
            // definitions into a second heap buffer during a warm boot.
            self.front_ahead.push(if ahead.image { Vec::new() } else { crate::crate_image::encode(&ahead) });
            self.front_restored.push(ahead.image || ahead.valid);
            self.front_dep_names.push(self.pool.as_ref().map(|p| p.dep_names(ci)).unwrap_or_default());
            self.front_probes.push(Vec::new());
        }
        self.cur_ahead = ci;
        self.cur_image = ahead.image;
        if self.pool_workers.is_empty() && !ahead.image {
            // no parse workers: the loader parses from the scan's tokens
        } else {
            self.pending.extend(ahead.files.iter().map(|p| (ci, p.clone())));
        }
        // the crate's macro_rules! before its files parse: rustc makes `#[macro_export]` macros
        // available at `crate::name` regardless of textual order, and a file can use macros of
        // modules declared before it (the preload's scan found them)
        {
            let _profile = crate::load_profile::span(self.session.runtime.profile, "crate-macro-publication", &ahead.name);
            let table = std::sync::Arc::make_mut(&mut self.macros);
            for d in &ahead.defs {
                table.push(d.clone());
            }
            for (k, v) in &ahead.aliases {
                table.add_alias(k.0.clone(), k.1.clone(), v.clone());
            }
            // the extern prelude is the crate's dependencies as this load names them (every
            // loader passes them here; a preload without them -- the bring-up checker's, the
            // proc-macro host's -- gets them now): macro paths through `dep::` resolve with it
            let mut externs: crate::fxhash::HashMap<String, String> = ahead.mods.externs.clone();
            for (s, ci) in &self.crates[krate as usize].deps {
                if let Some(c) = self.crates.get(*ci as usize) {
                    let n = self.syms.str(*s);
                    // (std / core / alloc are in every extern prelude already)
                    if !matches!(n, "std" | "core" | "alloc") {
                        externs.insert(n.to_string(), self.syms.str(c.name).to_string());
                    }
                }
            }
            // Resolve scopes against the loader's complete dependency imports. Do not retain
            // a scope the prescan computed against its earlier macro table.
            let mods = std::sync::Arc::new(crate::macros::CrateMods { root: ahead.mods.root.clone(), src: ahead.mods.src.clone(), mods: ahead.mods.mods.clone(), items: ahead.mods.items.clone(), uses: ahead.mods.uses.clone(), externs, renames: ahead.mods.renames.clone(), ..Default::default() });
            table.add_crate(ahead.name.clone(), mods);
            if !ahead.macro_use.is_empty() {
                let v = table.macro_use.entry(ahead.name.clone()).or_default();
                for u in &ahead.macro_use {
                    if !v.contains(u) {
                        v.push(u.clone());
                    }
                }
            }
        }
        if let Some(pool) = &self.pool { pool.start_parsing(ci, self.macros.clone()); }
        if !ahead.prelexed.is_empty() {
            // This raw-source optimization has no token-origin context. Lowered files
            // expand derives inside their normal token-input parse instead.
            let files: Vec<(&str, &[u8], &[crate::lexer::Tok])> = ahead.prelexed.iter().map(|(p, s, t)| (p.as_str(), &s[..], &t[..])).collect();
            crate::builtins::pre_expand_derives(&files, &self.crates[krate as usize].cfg, edition);
        }
        for (p, s, t) in ahead.prelexed {
            self.prelexed.insert(p, (s, t));
        }
        self.cur_mod_path = kname.clone();
        let file = self.load_file(root, edition, krate)?;
        self.set_def_file(root_def, file);
        self.set_mod_file(root_mod, file);
        let dir = dir_of(root).to_string();
        let items = self.files[file as usize].ast.root_items.clone();
        let r = self.collect_items(file, &items, root_mod, &dir, true);
        // the injected `extern crate std;` (`extern crate core;` under #![no_std]): in edition
        // 2015 the crate root binds it, so `::core::..` / `super::core` / `use std::..` resolve
        // (from 2018 on it is not nameable as `crate::std`)
        if edition == 2015 {
            let f = &self.files[file as usize];
            let no_std = f.ast.root_attrs.iter().any(|a| a.inner && a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "no_std");
            let inj = self.syms.intern(if no_std { "core" } else { "std" });
            let bound = self.mods[root_mod as usize].types.contains_key(&inj) || self.imports.iter().any(|i| i.module == root_mod && i.bind == Some(inj));
            if bound && self.crates[krate as usize].deps.iter().any(|x| x.0 == inj) { self.report_injected_clash(root_mod, inj); }
            if !bound && self.crates[krate as usize].deps.iter().any(|x| x.0 == inj) {
                self.imports.push(Import { module: root_mod, global: true, segs: vec![(SegKind::Ident, inj)], bind: Some(inj), glob: false, is_pub: false, done: false, file, pos: 0, ext: true, bound: BOUND_INJECTED });
            }
        }
        // files no module loads (cfg'd out in ways the scan does not see)
        let unused: Vec<String> = ahead.files.into_iter().filter(|p| self.pending.remove(&(ahead.idx, p.clone()))).collect();
        if let Some(pool) = &self.pool {
            pool.cancel(ahead.idx, &unused);
        }
        self.prelexed.clear();
        if self.ahead.is_empty() {
            // every preloaded crate is in: no worker thread outlives the load (a later fork)
            self.pool = None;
            for h in std::mem::take(&mut self.pool_workers) {
                let _ = h.join();
            }
            if self.demand_plan.as_ref().is_some_and(|p| p.counts().1 == 0) { self.demand_plan = None; }
        }
        r?;
        self.expand_pending(krate, edition);
        std::sync::Arc::make_mut(&mut self.macros).seal_crate(&kname);
        // The image contract covers this boot load only. Later generated files and live
        // module loads must not inherit the last boot crate's no-parse assertion.
        self.cur_image = false;
        Ok(krate)
    }

    /// Scan and lower `crates` ahead of load_crate. Each crate's parse workers start
    /// when its dependency macro environment is ready (preload.rs).
    pub fn preload(&mut self, crates: Vec<crate::preload::PreCrate>) {
        self.preload_images(crates, None);
    }

    pub fn preload_images(&mut self, crates: Vec<crate::preload::PreCrate>, images: Option<&dyn crate::crate_image::CrateImages>) {
        self.preload_images_bounded(crates, images, false, None);
    }

    /// Limit queued heap parses to the loading crate and its successor when the driver
    /// retires completed crates to disk. Other loaders retain the original scheduling.
    /// A collected suffix may supply the completed plan for its entire graph.
    pub fn preload_images_bounded(&mut self, crates: Vec<crate::preload::PreCrate>, images: Option<&dyn crate::crate_image::CrateImages>, bounded: bool, plan: Option<std::sync::Arc<crate::preload::DemandPlan>>) {
        self.start_preload_plan(crates, images, bounded, false, plan);
    }

    fn start_preload(&mut self, crates: Vec<crate::preload::PreCrate>, images: Option<&dyn crate::crate_image::CrateImages>, bounded: bool, eager_fallback: bool) {
        self.start_preload_plan(crates, images, bounded, eager_fallback, None);
    }

    fn start_preload_plan(&mut self, crates: Vec<crate::preload::PreCrate>, images: Option<&dyn crate::crate_image::CrateImages>, bounded: bool, eager_fallback: bool, plan: Option<std::sync::Arc<crate::preload::DemandPlan>>) {
        let plan = plan.or_else(|| (!eager_fallback && !self.session.options.source_eager && self.sources.overridden_paths().is_empty())
            .then(|| images.and_then(|i| i.take_demand_plan(&crates))).flatten());
        if let Some(plan) = &plan {
            // Failed collected restoration still owns the plan's source IDs.
            // Its AST/reuse cells and subsequent collection share that registry.
            if let Some(context) = plan.contexts.first() { self.sources = context.sources().clone(); }
        }
        self.front_graph = (self.record_front && self.crates.is_empty() && self.sources.overridden_paths().is_empty())
            .then(|| crate::crate_image::GraphRecipe::new(&crates));
        let mut session = crate::Session { options: self.session.options.clone(), runtime: self.session.runtime.clone() };
        session.options.source_eager |= eager_fallback;
        let sess = &std::sync::Arc::new(session);
        // Retire the old pool even when a failed load left crates waiting to start.
        if let Some(pool) = &self.pool { pool.stop(); }
        self.pool = None;
        for h in std::mem::take(&mut self.pool_workers) {
            let _ = h.join();
        }
        self.ahead = crates.iter().enumerate().map(|(i, c)| ((c.name.clone(), c.root.clone()), i as u32)).collect();
        if let Some(plan) = &plan {
            // A suffix retains the full recipe and dependency indices used by
            // the plan that certified its collected prefix.
            self.load_recipe = plan.crates.clone();
        } else { for c in &crates {
            if let Some(saved) = self.load_recipe.iter_mut().find(|old| old.name == c.name && old.root == c.root) { *saved = c.clone(); }
            else { self.load_recipe.push(c.clone()); }
        } }
        let (pool, workers) = crate::preload::start_with_plan(sess, crates, &self.macros, images, bounded, self.sources.clone(), plan);
        self.demand_plan = pool.demand_plan.clone();
        self.pool = Some(pool);
        self.pool_workers = workers;
    }

    /// Rebuild at canonical module positions before exposing the replacement to
    /// immutable resolver/type-checker readers. Used for verification and edits
    /// that change the set of reached files.
    pub fn replay_sources(&self, sources: std::sync::Arc<crate::source::SourceRegistry>, eager: bool, contexts: Option<Vec<std::sync::Arc<crate::source_context::Context>>>) -> Result<Program, String> {
        self.replay_watched_sources(sources, eager, contexts, &mut Vec::new())
    }

    /// Keep negative module probes even when collection fails: a warm editor
    /// must notice a missing module appearing after an unsuccessful edit.
    pub fn replay_watched_sources(&self, sources: std::sync::Arc<crate::source::SourceRegistry>, eager: bool, contexts: Option<Vec<std::sync::Arc<crate::source_context::Context>>>, probes: &mut Vec<(String, bool)>) -> Result<Program, String> {
        let mut session = crate::Session { options: self.session.options.clone(), runtime: self.session.runtime.clone() };
        // Eagerness belongs to this replay. Preserve the configured rs_lazy so
        // a later edit can build a lazy plan after an eager editor query.
        session.options.source_eager = eager;
        session.runtime.lazy_verify = false;
        let session = std::sync::Arc::new(session);
        let mut recipe = self.load_recipe.clone();
        for c in &mut recipe { c.cfg.session = session.clone(); }
        let mut next = Program::new(&session);
        next.sources = sources;
        next.load_recipe = recipe.clone();
        next.ahead = recipe.iter().enumerate().map(|(i, c)| ((c.name.clone(), c.root.clone()), i as u32)).collect();
        let (pool, workers) = crate::preload::start_with_contexts(&session, recipe, &next.macros, None, false, next.sources.clone(), contexts);
        next.demand_plan = pool.demand_plan.clone();
        next.pool = Some(pool);
        next.pool_workers = workers;
        for c in &self.crates {
            let deps = c.deps.iter().map(|(name, index)| (next.syms.intern(self.syms.str(*name)), *index)).collect();
            let mut cfg = c.cfg.clone(); cfg.session = session.clone();
            let k = match next.load_crate(self.syms.str(c.name), c.root_path(), c.edition, cfg, deps) {
                Ok(k) => k,
                Err(error) => {
                    probes.extend(next.module_probes.iter().cloned());
                    if let Some(plan) = &next.demand_plan { probes.extend(plan.probes.iter().cloned()); }
                    return Err(error);
                }
            };
            next.crates[k as usize].proc_macro = c.proc_macro;
            next.crates[k as usize].is_bin = c.is_bin;
        }
        next.prelude_crate = self.prelude_crate;
        next.resolve_imports();
        probes.extend(next.module_probes.iter().cloned());
        Ok(next)
    }

    /// Compare the complete lazy replay with a separately parsed eager program.
    /// Reached files reuse their original lowering cells; only the second program
    /// starts with fresh lowering contexts. Keep the original lazy program alive
    /// for execution: verification must not silently turn a run into eager mode.
    pub fn verify_lazy(&self, verify_file: fn(&ParsedFile, &ParsedFile)) -> Result<(), String> {
        let Some(plan) = &self.demand_plan else { return Ok(()); };
        plan.force_remaining()?;
        // A warm load's plan may cover only the rebuilt suffix. Replay also
        // needs the contexts of crates retained from the collected image.
        let contexts = self.crates.iter().map(|c| c.context.clone()).collect();
        let complete = self.replay_sources(self.sources.clone(), true, Some(contexts))?;
        let eager = self.replay_sources(self.sources.clone(), true, None)?;
        let actual = complete.program_snapshot();
        let expected = eager.program_snapshot();
        if actual != expected {
            let detail = actual.lines().zip(expected.lines()).enumerate().find(|(_, (a, b))| a != b)
                .map(|(line, (a, b))| format!("\nfirst difference at snapshot line {}:\nlazy: {a}\neager: {b}", line + 1)).unwrap_or_default();
            let source = complete.files.iter().zip(&eager.files).enumerate().find(|(_, (a, b))| a.src[..] != b.src[..])
                .map(|(i, (a, b))| {
                    let a = String::from_utf8_lossy(&a.src);
                    let b = String::from_utf8_lossy(&b.src);
                    let difference = a.split_whitespace().zip(b.split_whitespace()).enumerate().find(|(_, (a, b))| a != b)
                        .map(|(token, (a, b))| format!(" at token {}: lazy {a:?}, eager {b:?}", token + 1)).unwrap_or_default();
                    format!("\nfirst differing source: {}{difference}", complete.file_paths[i])
                }).unwrap_or_default();
            return Err(format!("MAKEPAD_RUN_LAZY_VERIFY: complete program items, IDs, resolutions, or diagnostics differ from eager{detail}{source}"));
        }
        if complete.file_paths != eager.file_paths { return Err("MAKEPAD_RUN_LAZY_VERIFY: file order differs from eager".into()); }
        for (left, right) in complete.files.iter().zip(&eager.files) { verify_file(left, right); }
        // Compare the actual lazy result as well. Completing the graph must not
        // mask a changed macro expansion, a missing import, or a different binding.
        let reached: std::collections::BTreeSet<_> = self.file_paths.iter().cloned().collect();
        let mut occurrences = std::collections::BTreeMap::new();
        for (file_index, (path, file)) in self.file_paths.iter().zip(&self.files).enumerate() {
            let krate = self.file_crate[file_index];
            let occurrence = occurrences.entry((krate, path)).or_insert(0);
            let Some((i, _)) = eager.file_paths.iter().enumerate().filter(|(i, p)| *p == path && eager.file_crate[*i] == krate).nth(*occurrence) else {
                return Err(format!("MAKEPAD_RUN_LAZY_VERIFY: lazy-only file {path}"));
            };
            *occurrence += 1;
            verify_file(file, &eager.files[i]);
        }
        let deferred = self.deferred_bindings();
        let expected = eager.resolution_snapshot(&reached, None, &deferred);
        let actual = self.resolution_snapshot(&reached, Some(&expected), &Default::default());
        if actual != expected {
            let first = actual.iter().find(|(key, values)| expected.get(*key) != Some(*values));
            let detail = first.map(|(key, values)| format!("\n{key}\nlazy: {values:?}\neager: {:?}", expected.get(key))).unwrap_or_default();
            return Err(format!("MAKEPAD_RUN_LAZY_VERIFY: a reached module resolves differently from eager{detail}"));
        }
        let errors: Vec<_> = eager.errors.iter().filter(|error| reached.iter().any(|path| error.starts_with(path))).collect();
        if self.errors.iter().collect::<Vec<_>>() != errors {
            return Err("MAKEPAD_RUN_LAZY_VERIFY: reached-file diagnostics differ from eager".into());
        }
        if self.session.options.rs_lazy && self.language_snapshot() != eager.language_snapshot() {
            return Err("MAKEPAD_RUN_LAZY_VERIFY: global language items or their diagnostics differ from eager".into());
        }
        let (reached, total) = plan.counts();
        eprintln!("rust-lazy-verify: passed; reached={reached} total={total}; items, IDs, ASTs, resolutions, diagnostics and available indexes match");
        Ok(())
    }

    fn language_snapshot(&self) -> (Vec<Option<String>>, Vec<String>) {
        let mut items = crate::lang_items::LangItems::default();
        let mut errors = Vec::new();
        let defs = (0..self.defs.len()).map(|i| DefId(i as u32)).collect::<Vec<_>>();
        items.collect(self, &defs, &mut errors);
        let items = items.items.iter().map(|d| d.map(|id| {
            let def = self.def(id);
            format!("{} {} {}", self.def_path(id), self.file_paths[def.file as usize], def.item.0)
        })).collect();
        (items, errors)
    }

    /// Module key -> (names whose import the selection deferred, a glob deferred).
    fn deferred_bindings(&self) -> std::collections::BTreeMap<String, (std::collections::BTreeSet<String>, bool)> {
        let mut out = std::collections::BTreeMap::<String, (std::collections::BTreeSet<String>, bool)>::new();
        for &(m, name) in &self.deferred_imports {
            let entry = out.entry(self.def_path(self.mods[m as usize].def)).or_default();
            match name { Some(name) => { entry.0.insert(self.syms.str(name).to_owned()); } None => entry.1 = true }
        }
        out
    }

    fn resolution_snapshot(&self, files: &std::collections::BTreeSet<String>, expected: Option<&std::collections::BTreeMap<String, Vec<String>>>,
        deferred: &std::collections::BTreeMap<String, (std::collections::BTreeSet<String>, bool)>) -> std::collections::BTreeMap<String, Vec<String>> {
        let key = |id: DefId| {
            if id == NO_DEF { return "none".to_owned(); }
            let d = self.def(id);
            format!("{} {:?} {} {} {}", self.def_path(id), d.kind, d.krate, self.file_paths.get(d.file as usize).map_or("", String::as_str), d.item.0)
        };
        let mut out = std::collections::BTreeMap::<_, Vec<_>>::new();
        for m in &self.mods {
            if !self.file_paths.get(m.file as usize).is_some_and(|p| files.contains(p)) { continue; }
            let module = key(m.def);
            // A deferred module's declaration still belongs to its reached parent
            // file. Its empty placeholder scope is excluded using eager's file map.
            if expected.is_some_and(|e| !e.contains_key(&module)) { continue; }
            // The lazy program's deferred imports bound nothing: compare the rest.
            let (skip, skip_globs) = deferred.get(&self.def_path(m.def)).map_or((None, false), |(names, globs)| (Some(names), *globs));
            let own = self.def_path(m.def);
            // Only import bindings can be deferred; the module's own items stay.
            let kept = |name: Sym, def: Option<DefId>| !skip.is_some_and(|names| names.contains(self.syms.str(name)))
                || def.is_some_and(|def| def != NO_DEF && self.def_path(def) == format!("{own}::{}", self.syms.str(name)));
            let mut bindings = Vec::new();
            for (ns, names) in ["type", "value", "macro"].iter().zip([&m.types, &m.values, &m.macros]) {
                for (name, def) in names { if kept(*name, Some(*def)) { bindings.push(format!("{ns} {} {}", self.syms.str(*name), key(*def))); } }
            }
            for (def, public) in &m.globs { if !skip_globs || self.def(*def).kind != DefKind::Mod || files.contains(self.file_paths.get(self.mods[self.def(*def).sub as usize].file as usize).map_or("", String::as_str)) { bindings.push(format!("glob {public} {}", key(*def))); } }
            for (def, public) in &m.anon_traits { bindings.push(format!("trait {public} {}", key(*def))); }
            for ((name, types), public) in &m.import_pub { if kept(*name, None) { bindings.push(format!("visibility {} {types} {}", self.syms.str(*name), public & IMPORT_PUB != 0)); } }
            bindings.sort();
            out.entry(module).or_default().push(format!("{bindings:?}"));
        }
        out
    }

    fn program_snapshot(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for d in &self.defs {
            let _ = writeln!(out, "{:?} {} {:?} {} {} {} {} {} {}", d.kind, d.name, d.parent, d.file, d.item.0, d.sub, d.is_pub, d.krate, d.scope);
        }
        for m in &self.mods {
            let _ = writeln!(out, "module {:?} {:?} {} {:?} {} {} {:?} {:?}", m.def, m.lexical_parent, m.normal, m.parent_mod, m.krate, m.file, m.globs, m.anon_traits);
            for ns in [&m.types, &m.values, &m.macros] {
                let mut entries: Vec<_> = ns.iter().collect(); entries.sort_by_key(|(s, _)| **s);
                let _ = writeln!(out, "{:?}", entries);
            }
            let mut vis: Vec<_> = m.import_pub.iter().collect(); vis.sort_by_key(|(s, _)| **s);
            let _ = writeln!(out, "{:?}", vis);
        }
        for i in &self.imports {
            let _ = writeln!(out, "{} {} {:?} {:?} {} {} {} {} {}", i.module, i.global, i.segs, i.bind, i.glob, i.is_pub, i.done, i.file, i.pos);
        }
        for i in &self.impls {
            let _ = writeln!(out, "impl {:?} {} {} {} {:?}", i.def, i.file, i.item.0, i.module, i.items);
        }
        let mut item_defs: Vec<_> = self.item_defs.iter().collect(); item_defs.sort_by_key(|(k, _)| **k);
        let mut block_mods: Vec<_> = self.block_mods.iter().collect(); block_mods.sort_by_key(|(k, _)| **k);
        let mut block_owner: Vec<_> = self.block_owner.iter().collect(); block_owner.sort_by_key(|(k, _)| **k);
        let _ = writeln!(out, "traits {:?}\nvariants {:?}\nitems {:?}\nblocks {:?}\nowners {:?}\nlanguage {:?}",
            self.traits.iter().collect::<Vec<_>>(), self.variants.iter().collect::<Vec<_>>(), item_defs, block_mods, block_owner, self.language_snapshot());
        let _ = writeln!(out, "{:?}\n{:?}\n{:?}", self.syms.names().collect::<Vec<_>>(), self.file_paths, self.errors);
        out
    }

    /// A file of crate `krate` at its publication (final: cfg-stripped, its sources bound, its
    /// speculative parse accepted): its image is written and its arrays become slices of the
    /// mapping before it is shared, so its parse arrays leave the heap here. The registry's
    /// authored bytes for the file become a view of the image too, and the read snapshots of
    /// every context sharing the registry drop their copy (preload workers and lazy expansion
    /// bind the first context, not the file's own crate's).
    fn map_image(&self, path: &str, krate: u32, pf: &mut ParsedFile) {
        let Some(sink) = &self.front_sink else { return };
        if pf.image.is_some() { return; }
        let _profile = crate::load_profile::span(self.session.runtime.profile, "front-image-write", path);
        if !sink.file(path, pf) { return; }
        pf.sink = Some(sink.clone());
        if let (crate::arena::Arena::Mapped(src), Some(c)) = (&pf.src, self.crates.get(krate as usize)) {
            if c.context.sources().map_authored(path, &src[..pf.src_len as usize]) {
                for c in &self.crates { c.context.release_read(path); }
            }
        }
    }

    fn load_file(&mut self, path: &str, edition: u16, krate: u32) -> Result<u32, String> {
        crate::why::event("load", path, 0, "", None);
        let _profile = crate::load_profile::span(self.session.runtime.profile, "load-file", path);
        if let Some(f) = self.check_files.as_ref().and_then(|m| m.get(path)) { return Ok(*f); }
        let kname = if (krate as usize) < self.crates.len() { self.syms.str(self.crates[krate as usize].name).to_string() } else { String::new() };
        let std_file = matches!(kname.as_str(), "core" | "alloc" | "std");
        if std_file && self.std_from_shared {
            if let Some(sp) = shared_std_wait(path) {
                let cfg = &self.crates[krate as usize].cfg.set;
                if sp.edition == edition && &sp.cfg == cfg && sp.mod_path == self.cur_mod_path && sp.used.replays(&self.macros, &kname, path) {
                    let table = std::sync::Arc::make_mut(&mut self.macros);
                    for d in &sp.used.local {
                        table.push(d.clone());
                    }
                    if !sp.used.uses.is_empty() {
                        table.add_exp_uses(&kname, sp.used.uses.iter().map(|(k, v)| (k.clone(), v.clone())));
                    }
                    for &(lo, hi) in &sp.idents {
                        let s = std::str::from_utf8(sp.pf.bytes(lo, hi)).unwrap_or("?");
                        self.syms.intern(s);
                    }
                    if self.record_front && !self.cur_image {
                        let prep = FilePrep { idents: sp.idents.clone(), blocks: sp.blocks.clone(), lines: sp.lines };
                        self.front_files.push((krate, self.files.len(), crate::crate_image::file_meta(path, &self.cur_mod_path, &sp.pf, &sp.used, &prep, true)));
                    }
                    self.files.push(sp.pf.clone());
                    self.file_paths.push(path.to_string());
                    self.exec_profile_file(self.file_paths.len() as u32 - 1);
                    self.file_crate.push(krate);
                    self.file_blocks.push(sp.blocks.clone());
                    self.lines_loaded += sp.lines;
                    return Ok(self.files.len() as u32 - 1);
                }
            }
        }
        let mut ahead = None;
        let mut src = None;
        let mut verify_image = None;
        let tr = crate::preload::trace();
        let tl = std::time::Instant::now();
        let mut why = "none";
        let pre = if self.pending.remove(&(self.cur_ahead, path.to_string())) { self.pool.as_ref().and_then(|p| p.wait_in_scope(&self.session, self.cur_ahead, path, Some(self.macros.clone()))) } else { None };
        // All file results were validated before this crate was published. A missing result
        // or changed module path here is an invariant violation, never a silent partial parse.
        assert!(!self.cur_image || pre.is_some(), "crate image omitted loaded file {}", path);
        if let Some(x) = pre {
            if let Some(error) = x.error { return Err(error); }
            if self.cur_image {
                assert_eq!(x.mod_path, self.cur_mod_path, "crate image module path: {}", path);
                assert!(x.parsed.is_some() && x.prep.is_some(), "incomplete crate image file {}", path);
            }
            verify_image = x.verify;
            why = if x.parsed.is_none() {
                "pmhost"
            } else if x.mod_path != self.cur_mod_path {
                "modpath"
            } else {
                "replay"
            };
            match x.parsed {
                Some((pf, used)) if x.mod_path == self.cur_mod_path && used.replays(&self.macros, &kname, path) => {
                    why = "preloaded";
                    ahead = Some((pf, used, x.prep));
                }
                Some((mut pf, _)) => {
                    PARSE_AGAIN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    pf.src.truncate(x.src_len);
                    src = Some(std::mem::take(&mut pf.src).into_vec());
                }
                None => {
                    PARSE_AGAIN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
            // Every rejection, including small files below boot-trace-file's
            // timing threshold, needs an attributable reason on the next run.
            if tr && ahead.is_none() {
                eprintln!("boot-trace-reparse: {path} ({why}) speculative_module={:?} published_module={:?}", x.mod_path, self.cur_mod_path);
            }
        }
        let (mut pf, used, prep) = match ahead {
            Some(x) => x,
            None => {
                // the macro prescan read and lexed most files already
                let (src, pre) = match (src, self.prelexed.remove(path)) {
                    (Some(s), _) => (s, None),
                    (None, Some((s, t))) => (s, Some(t)),
                    (None, None) => (self.crates[krate as usize].context.read_source(path)?.src.to_vec(), None),
                };
                let bctx = std::rc::Rc::new(crate::builtins::FileCtx::new(&kname, path, &self.cur_mod_path, &self.crates[krate as usize].cfg));
                let (pf, used) = self.crates[krate as usize].context.parse_input(src, pre, edition,
                    Some(crate::macros::MacroScope::new(self.macros.clone())), bctx, crate::parser::lazy_auto())?;
                (pf, used.unwrap(), None)
            }
        };
        if tr && tl.elapsed().as_secs_f64() > 0.002 {
            eprintln!("boot-trace-file: {:.1} ms {} ({})", tl.elapsed().as_secs_f64() * 1e3, path, why);
        }
        let _publish = crate::load_profile::span(self.session.runtime.profile, "file-publication", path);
        pf.bind_sources(path, &self.sources)?;
        // Deferred calls were not expanded by the worker. They must see the table at this
        // file's actual collection point, plus the body's captured local definitions/imports.
        let prep = match prep {
            Some(p) => p,
            None => prepare_file(&mut pf, &self.crates[krate as usize].cfg),
        };
        if let Some(verify) = verify_image {
            let bctx = std::rc::Rc::new(crate::builtins::FileCtx::new(&kname, path, &self.cur_mod_path, &self.crates[krate as usize].cfg));
            let (mut fresh, fused) = self.crates[krate as usize].context.parse_input(
                pf.src[..pf.src_len as usize].to_vec(), None, edition,
                Some(crate::macros::MacroScope::new(self.macros.clone())), bctx, crate::parser::lazy_auto(),
            ).unwrap_or_else(|e| panic!("front verify parse {}: {:?}", path, e));
            fresh.bind_sources(path, &self.sources).expect("front verify sources");
            let fprep = prepare_file(&mut fresh, &self.crates[krate as usize].cfg);
            verify(&pf, &fresh);
            assert_eq!(crate::crate_image::encode(&used), crate::crate_image::encode(&fused.unwrap()), "front MacroUse: {}", path);
            assert_eq!(crate::crate_image::encode(&prep), crate::crate_image::encode(&fprep), "front FilePrep: {}", path);
        }
        if self.record_front && !self.cur_image {
            self.front_files.push((krate, self.files.len(), crate::crate_image::file_meta(path, &self.cur_mod_path, &pf, &used, &prep, false)));
        }
        for &(lo, hi) in &prep.idents {
            let s = std::str::from_utf8(pf.bytes(lo, hi)).unwrap_or("?");
            self.syms.intern(s);
        }
        self.map_image(path, krate, &mut pf);
        let pf = std::sync::Arc::new(pf);
        // a std file of the program's load: the proc-macro world takes it from here
        if std_file && !self.std_from_shared && SHARING.load(std::sync::atomic::Ordering::Acquire) {
            shared_std_put(path, SharedParse { pf: pf.clone(), used: used.clone(), idents: prep.idents.clone(), blocks: prep.blocks.clone(), lines: prep.lines, edition, cfg: self.crates[krate as usize].cfg.set.clone(), mod_path: self.cur_mod_path.clone() });
        }
        // the file's definitions, in load order: what later files and crates see
        let table = std::sync::Arc::make_mut(&mut self.macros);
        for d in used.local {
            table.push(d);
        }
        if !used.uses.is_empty() {
            table.add_exp_uses(&kname, used.uses);
        }
        self.files.push(pf);
        self.file_paths.push(path.to_string());
        self.exec_profile_file(self.file_paths.len() as u32 - 1);
        self.file_crate.push(krate);
        self.file_blocks.push(prep.blocks);
        self.lines_loaded += prep.lines;
        Ok(self.files.len() as u32 - 1)
    }

    /// Hot reload: parses `src` as the next version of file `old` without changing anything
    /// the running program reads (the macro definitions it makes are applied by commit_version).
    /// The new version gets its own file index when committed, so every expression-id keyed
    /// thing of the old version (closure and coroutine types, checked bodies, code positions)
    /// keeps meaning the old text: old code finishes on the old AST, new code never collides
    /// with it.
    pub fn parse_version(&self, old: u32, path: &str, src: Vec<u8>) -> Result<FileVersion, String> {
        self.parse_snapshot(old, path).parse(src)
    }

    pub fn parse_snapshot(&self, old: u32, path: &str) -> ParseSnapshot {
        let edition = self.files[old as usize].edition;
        let krate = self.file_crate[old as usize];
        let kname = self.syms.str(self.crates[krate as usize].name).to_string();
        // the module path the file was loaded under (module_path!())
        let mod_path = match self.reload_files().mods.get(&old).into_iter().flatten().map(|&m| &self.mods[m as usize]).find(|m| m.lexical_parent.is_none()) {
            Some(m) if m.def.0 != u32::MAX && (m.def.0 as usize) < self.defs.len() && self.defs[m.def.0 as usize].kind == DefKind::Mod && self.defs[m.def.0 as usize].file != old => {
                self.def_path(m.def)
            }
            _ => kname.clone(),
        };
        let mut macros = self.macros.clone();
        let root = self.crates[krate as usize].root_path();
        if macros.crates.get(&kname).is_some_and(|c| c.src != root) {
            if let Some(context) = macros.crates.values().find(|c| c.src == root).cloned() {
                std::sync::Arc::make_mut(&mut macros).crates.insert(kname.clone(), context);
            }
        }
        ParseSnapshot { old, path: path.to_string(), edition, kname, mod_path,
            cfg: self.crates[krate as usize].cfg.clone(), macros,
            reuse: self.parse_reuse.get(&old).cloned().unwrap_or_default(),
            previous: Some(self.files[old as usize].clone()), context: self.crates[krate as usize].context.clone() }
    }

    /// A parse of a NEW file that becomes child module `child` of module `parent` (makepad
    /// inspect's probes and `eval`: code compiled in a module's scope): the parent's crate,
    /// edition, cfg and macro table; `module_path!()` names the child.
    pub fn child_parse_snapshot(&self, parent: u32, child: &str, path: &str) -> ParseSnapshot {
        let krate = self.mods[parent as usize].krate;
        let kname = self.syms.str(self.crates[krate as usize].name).to_string();
        let normal = self.mods[parent as usize].normal;
        let mod_path = format!("{}::{}", self.def_path(self.mods[normal as usize].def), child);
        ParseSnapshot { old: self.mods[normal as usize].file, path: path.to_string(), edition: self.crates[krate as usize].edition, kname, mod_path,
            cfg: self.crates[krate as usize].cfg.clone(), macros: self.macros.clone(), reuse: Default::default(), previous: None,
            context: self.crates[krate as usize].context.clone() }
    }

    /// Adds `v` (a child_parse_snapshot parse) as a new private module `name` of module `parent`
    /// and collects its items there: it is the parent's child, so the parent's private items,
    /// imports and `crate::` / `super::` paths are its own as for any child module; `name` is
    /// not bound in the parent (nothing of the program can name it). The parent's names are in
    /// scope unqualified through a glob of the parent (what `use super::*;` gives a child).
    /// Returns (module, file). The caller resolves the imports it added (resolve_imports_added).
    pub fn add_child_module(&mut self, parent: u32, name: &str, v: FileVersion) -> Result<(u32, u32), String> {
        let parent = self.mods[parent as usize].normal;
        let krate = self.mods[parent as usize].krate;
        for &(lo, hi) in &v.idents {
            let s = std::str::from_utf8(&v.pf.src[lo as usize..hi as usize]).unwrap_or("?").to_string();
            self.syms.intern(&s);
        }
        let table = std::sync::Arc::make_mut(&mut self.macros);
        for d in v.macros {
            table.push(d);
        }
        self.crates[krate as usize].context = v.context;
        let mut pf = v.pf;
        self.map_image(&v.path, krate, &mut pf);
        self.files.push(std::sync::Arc::new(pf));
        self.file_paths.push(v.path);
        self.exec_profile_file(self.file_paths.len() as u32 - 1);
        self.file_crate.push(krate);
        self.file_blocks.push(v.blocks);
        let f = self.files.len() as u32 - 1;
        let sym = self.syms.intern(name);
        let pdef = self.mods[parent as usize].def;
        let d = self.add_def(Def { kind: DefKind::Mod, name: sym, parent: pdef, file: f, item: ItemId(0), sub: 0, is_pub: false, krate, scope: parent });
        let m = self.new_module(d, Some(parent), None, krate, f);
        self.defs[d.0 as usize].sub = m;
        self.mods[m as usize].globs.push((pdef, false));
        if self.imports_done {
            self.scope_cache.clear();
        }
        let dir = dir_of(&self.file_paths[f as usize]).to_string();
        let items: Vec<ItemId> = self.files[f as usize].ast.root_items.iter().copied().collect();
        self.collect_items(f, &items, m, &dir, false)?;
        Ok((m, f))
    }

    /// Item-level macro calls of crate `krate` no macro resolved at their parse, expanded again
    /// now that every file of the crate is loaded (its expansions' macro_rules! and imports are in
    /// the table), to a fixpoint -- what rustc's resolve/expand loop does for macros an expansion
    /// later in the crate defines or re-exports (makepad-draw's DrawShader derive: the chain
    /// through a base shader's `pub use ..::ancestry as Base;` in another file). A call text is
    /// parsed again in its module and its items are collected there; what still names no macro
    /// is an error (rustc: cannot find macro).
    fn expand_pending(&mut self, krate: u32, edition: u16) {
        let sess = &self.session.clone();
        let kname = self.syms.str(self.crates[krate as usize].name).to_string();
        let mut rounds = 0;
        loop {
            let pend: Vec<(u32, ItemId, u32)> = {
                let (mine, other): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_macs).into_iter().partition(|p| self.mods[p.2 as usize].krate == krate);
                self.pending_macs = other;
                mine
            };
            if pend.is_empty() {
                return;
            }
            rounds += 1;
            let mut still = Vec::new();
            let mut progress = false;
            for (file, iid, module) in pend {
                if rounds > 16 {
                    still.push((file, iid, module));
                    continue;
                }
                let (text, path) = {
                    let f = &self.files[file as usize];
                    let it = f.ast.item(iid);
                    (f.bytes(it.lo, it.hi).to_vec(), self.file_paths[file as usize].clone())
                };
                let mod_path = self.def_path(self.mods[module as usize].def);
                let cfg = self.crates[krate as usize].cfg.clone();
                let bctx = std::rc::Rc::new(crate::builtins::FileCtx::new(&kname, &path, &mod_path, &cfg));
                let parsed = crate::parser::parse_owned_ctx(sess, text, None, edition, Some(crate::macros::MacroScope::new(self.macros.clone())), &kname, Some(bctx), false);
                let Ok((mut pf, used)) = parsed else {
                    still.push((file, iid, module));
                    continue;
                };
                let used = used.unwrap();
                let items = pf.ast.root_items.clone();
                // the same call again: nothing new resolves it yet
                if items.len() == 1 && matches!(pf.ast.item(items[0]).kind, ItemKind::Mac(_)) {
                    still.push((file, iid, module));
                    continue;
                }
                progress = true;
                let prep = prepare_file(&mut pf, &cfg);
                for &(lo, hi) in &prep.idents {
                    let s = std::str::from_utf8(pf.bytes(lo, hi)).unwrap_or("?").to_string();
                    self.syms.intern(&s);
                }
                let table = std::sync::Arc::make_mut(&mut self.macros);
                for d in used.local {
                    table.push(d);
                }
                if !used.uses.is_empty() {
                    table.add_exp_uses(&kname, used.uses);
                }
                self.front_macro_fragments.insert(self.files.len());
                self.map_image(&path, krate, &mut pf);
                self.files.push(std::sync::Arc::new(pf));
                self.file_paths.push(path);
                self.exec_profile_file(self.file_paths.len() as u32 - 1);
                self.file_crate.push(krate);
                self.file_blocks.push(prep.blocks);
                let nf = self.files.len() as u32 - 1;
                if let Err(e) = self.collect_items(nf, &items, module, "", false) {
                    self.errors.push(e);
                }
            }
            if !progress {
                for (file, iid, _) in still {
                    let f = &self.files[file as usize];
                    let it = f.ast.item(iid);
                    let name = match &it.kind {
                        ItemKind::Mac(m) => f.ast.segs(f.ast.row(*m).path.segs).last().map(|sg| f.text(sg.name).to_string()).unwrap_or_default(),
                        _ => String::new(),
                    };
                    self.errors.push(self.record_error(file, it.lo, "macro", format!("cannot find macro `{}` in this scope", name)));
                }
                return;
            }
            self.pending_macs.extend(still);
        }
    }

    /// Names imports bind to macros: an import whose last segment is a macro_rules! name, then
    /// (to a fixpoint) one whose last segment such an import binds. Macros resolve at their call
    /// (macros.rs); these imports name nothing in the type / value namespaces.
    fn macro_import_binds(&self) -> crate::fxhash::HashSet<Sym> {
        let mut set: crate::fxhash::HashSet<Sym> = crate::fxhash::HashSet::default();
        // (module, name) bindings known to be macros
        let mut bound: crate::fxhash::HashSet<(u32, Sym)> = crate::fxhash::HashSet::default();
        loop {
            let mut grew = false;
            for im in &self.imports {
                let (Some(b), Some((_, last))) = (im.bind, im.segs.last()) else { continue };
                if im.glob || bound.contains(&(im.module, b)) {
                    continue;
                }
                if !(set.contains(last) || self.macros.defs.contains_key(self.syms.str(*last))) {
                    continue;
                }
                // a path through a module of a crate (not its root) names a macro only when that
                // module has an import binding it (a macro is not an item of the module it is
                // written in): `pub use self::nu;` in `mod m` finds nothing
                if im.segs.len() > 1 {
                    let krate = self.mods[im.module as usize].krate;
                    let edition = self.crates[krate as usize].edition;
                    if let Some(parent) = self.resolve_mod_path(im.module, im.global, &im.segs[..im.segs.len() - 1], edition < 2018, true) {
                        let pd = self.def(parent);
                        if pd.kind == DefKind::Mod && self.crates[pd.krate as usize].root_mod != pd.sub && !bound.contains(&(pd.sub, *last)) {
                            continue;
                        }
                    }
                }
                bound.insert((im.module, b));
                set.insert(b);
                grew = true;
            }
            if !grew {
                return set;
            }
        }
    }

    /// Memory report: the per-occurrence name memos (entries, bytes).
    pub fn memo_mem_lines(&self) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut push = |name: &str, (n, b): (usize, usize)| out.push((format!("program {} ({})", name, n), b as u64));
        push("path_seg_cache", self.path_seg_cache.mem_stats(|_, v| 16 + v.len() * std::mem::size_of::<(crate::ast::SegKind, Sym)>()));
        push("scope_cache", self.scope_cache.mem_stats(|_, _| 0));
        push("glob_cache", self.glob_cache.mem_stats(|_, _| 0));
        push("trait_bind_cache", self.trait_bind_cache.mem_stats(|_, v| 40 + v.capacity() * std::mem::size_of::<(DefId, BindVis)>()));
        push("trait_scope", self.trait_scope.mem_stats(|_, v| 64 + v.capacity() * 5));
        out
    }

    /// The name-resolution memo tables (what resolving a path or a glob found, and the trait
    /// scopes) are dropped: derived from the collected tables, they refill on use, as after a
    /// restore. The entries go to `retired` (the caller frees them outside its locks). Safety
    /// as for `reclaim_caches`.
    pub unsafe fn detach_caches(&self, retired: &mut Vec<Box<dyn Send>>) {
        retired.push(Box::new(self.glob_cache.detach()));
        retired.push(Box::new(self.scope_cache.detach()));
        retired.push(Box::new(self.path_seg_cache.detach()));
        retired.push(Box::new(self.trait_bind_cache.detach()));
        retired.push(Box::new(self.trait_scope.detach()));
    }

    /// Frees the name-resolution cache tables earlier clears retired (hot reload and run-time
    /// loads clear them on every edit or load; each retired table is ~0.5 MB). Safety: no
    /// front-end reader may run (the caller holds the front-end write lock and the unit lock).
    pub unsafe fn reclaim_caches(&self) {
        self.glob_cache.reclaim();
        self.scope_cache.reclaim();
        self.path_seg_cache.reclaim();
        self.trait_bind_cache.reclaim();
    }

    /// Hot reload: makes a parsed version current. `pairs`: (old item, new item) for every item
    /// both versions have (reload.rs pairs them by identity). Returns the version's file index;
    /// every def of `old` (and its module, impl, item and block-module entries) now refers to it.
    /// The old file stays loaded for the code, types and bodies made from it.
    /// Whether file `f` is a version a live edit committed (`edit_versions`).
    pub fn is_edit_version(&self, f: u32) -> bool {
        self.edit_versions.contains(&f)
    }

    pub fn commit_version(&mut self, v: FileVersion, pairs: &[(u32, u32)]) -> Result<u32, String> {
        self.commit_version_inner(v, pairs, false, None)
    }

    /// commit_version for a transaction whose declaration pass is scoped to the crates that
    /// can see it (def_collector::lower_legacy_const_generics_in, lead decision 52 R1).
    pub fn commit_version_in(&mut self, v: FileVersion, pairs: &[(u32, u32)], visible: &[bool]) -> Result<u32, String> {
        self.commit_version_inner(v, pairs, false, Some(visible))
    }

    /// Body edit, after classification proved scopes/items unchanged. Finish the
    /// transaction with lower_body_declarations before publishing to readers.
    pub fn commit_body_version(&mut self, v: FileVersion, pairs: &[(u32, u32)]) -> Result<u32, String> {
        self.commit_version_inner(v, pairs, true, None)
    }

    fn commit_version_inner(&mut self, v: FileVersion, pairs: &[(u32, u32)], same_scopes: bool, visible: Option<&[bool]>) -> Result<u32, String> {
        let old = v.old;
        let t_save = std::time::Instant::now();
        let declarations = DeclarationState::save(self, old, same_scopes, visible);
        let save_ms = t_save.elapsed().as_secs_f64() * 1e3;
        // Old ASTs stay alive for running frames, but old reuse entries do not.
        self.parse_reuse.remove(&old);
        self.parse_reuse.insert(self.files.len() as u32, v.reuse);
        if !same_scopes {
            self.scope_cache.clear();
        }
        for &(lo, hi) in &v.idents {
            let s = std::str::from_utf8(&v.pf.src[lo as usize..hi as usize]).unwrap_or("?").to_string();
            self.syms.intern(&s);
        }
        // A path's segments depend only on its file's text (a file version never changes) and
        // on whether each name is interned yet; path_segs recomputes an entry that met a name
        // not interned, so the memo stays valid across a scoped item edit. Clearing (and freeing)
        // it cost ~0.7 s per item edit once boot filled it (lead decision 52 R1).
        if !same_scopes && visible.is_none() {
            self.path_seg_cache.clear();
        }
        let table = std::sync::Arc::make_mut(&mut self.macros);
        for d in v.macros {
            table.push(d);
        }
        let new = self.files.len() as u32;
        let fwd: HashMap<u32, u32> = pairs.iter().copied().collect();
        // block modules: the same (paired) items under a block (block ids shift with the edit)
        let mut by_items: HashMap<Vec<u32>, u32> = HashMap::default();
        for (b, (items, _)) in self.file_blocks[old as usize].iter() {
            if !items.is_empty() {
                if let Some(&m) = self.block_mods.get(&(old, *b)) {
                    if let Some(k) = items.iter().map(|i| fwd.get(&i.0).copied()).collect::<Option<Vec<u32>>>() {
                        by_items.insert(k, m);
                    }
                }
            }
        }
        for (b, (items, _)) in v.blocks.iter() {
            if !items.is_empty() {
                let k: Vec<u32> = items.iter().map(|i| i.0).collect();
                if let Some(&m) = by_items.get(&k) {
                    self.block_mods.insert((new, *b), m);
                }
            }
        }
        let krate = self.file_crate[old as usize];
        self.source_versions.entry(old).or_insert_with(|| self.crates[krate as usize].context.clone());
        self.source_versions.insert(new, v.context.clone());
        self.edit_versions.push(new);
        self.crates[krate as usize].context = v.context;
        let mut pf = v.pf;
        self.map_image(&v.path, krate, &mut pf);
        self.files.push(std::sync::Arc::new(pf));
        self.file_paths.push(v.path);
        self.exec_profile_file(self.file_paths.len() as u32 - 1);
        self.file_crate.push(krate);
        self.file_blocks.push(v.blocks);
        self.retarget_file(old, new, pairs);
        self.declaration_versions.insert(new, declarations);
        let t_collect = std::time::Instant::now();
        let collected = self.collect_version_definitions(new, !same_scopes, true, visible);
        if std::env::var_os("MAKEPAD_RUN_COMMIT_TRACE").is_some() {
            eprintln!("commit-trace: save {:.2} ms, collect definitions {:.2} ms, rest {:.2} ms", save_ms, t_collect.elapsed().as_secs_f64() * 1e3, t_save.elapsed().as_secs_f64() * 1e3 - save_ms - t_collect.elapsed().as_secs_f64() * 1e3);
        }
        if let Err(error) = collected {
            self.revert_version(old, new, pairs);
            return Err(error);
        }
        Ok(new)
    }

    /// `fresh`: `file` is a version index commit_version just made, so no node is keyed by it
    /// yet. `visible`: the crates RPITIT synthesis covers (assoc::collect_in; None: all).
    fn collect_version_definitions(&mut self, file: u32, synthesize: bool, fresh: bool, visible: Option<&[bool]>) -> Result<(), String> {
        if !fresh {
            self.node_id_to_def_id.retain(|(f, _), _| *f != file);
            self.def_id_to_node_id.retain(|_, (f, _)| *f != file);
        }
        // (the file's defs from the per-file index: no scan of the program's tables)
        for d in self.defs_in_file(file).to_vec() {
            let d = DefId(d);
            self.collected_definitions.remove(&d);
            self.legacy_call_names.remove(&d);
            self.repeat_expr_counts.remove(&d);
        }
        self.def_disambiguators.retain(|(d, _, _), _| self.defs[d.0 as usize].file != file);
        let mut owners = Vec::new();
        for item in 0..self.files[file as usize].ast.items.len() as u32 {
            if let Some(&def) = self.item_defs.get(&(file, item)) { owners.push((item, def)); }
        }
        owners.sort_unstable_by_key(|(item, _)| *item);
        for (_, def) in owners {
            // Variant owners must exist before collecting their fields/discriminants.
            if self.def(def).kind == DefKind::Enum { self.add_variants(def); }
            crate::def_collector::collect_definitions(self, def)?;
        }
        if synthesize { crate::assoc::collect_in(self, visible); }
        Ok(())
    }

    /// The crates that can see an edit of `files`: theirs and their transitive dependents
    /// (a crate names only its own items and its dependencies').
    pub fn crates_seeing_files(&self, files: impl Iterator<Item = u32>) -> Vec<bool> {
        let mut dependents = vec![Vec::new(); self.crates.len()];
        for (c, krate) in self.crates.iter().enumerate() {
            for &(_, dep) in &krate.deps {
                dependents[dep as usize].push(c);
            }
        }
        let mut visible = vec![false; self.crates.len()];
        let mut todo = Vec::new();
        for file in files {
            let c = self.file_crate[file as usize] as usize;
            if !visible[c] {
                visible[c] = true;
                todo.push(c);
            }
        }
        while let Some(c) = todo.pop() {
            for &dep in &dependents[c] {
                if !visible[dep] {
                    visible[dep] = true;
                    todo.push(dep);
                }
            }
        }
        visible
    }

    /// Called once after all body-only versions have been committed under the
    /// front write lock. Unchanged owners retain their resolution results.
    /// `files`: the versions the transaction committed. Only the owners defined in them moved to
    /// another file; every other owner's result already describes its file.
    pub fn lower_body_declarations(&mut self, changed: &[DefId], files: &[u32]) -> usize {
        let owners: Vec<DefId> = files.iter().flat_map(|&f| self.defs_in_file(f).iter().map(|&d| DefId(d)))
            .filter(|d| self.legacy_const_args.contains_key(d) && !changed.contains(d)).collect();
        for owner in owners {
            crate::def_collector::retarget_legacy_owner(self, owner);
        }
        crate::def_collector::lower_legacy_const_owners(self, changed)
    }

    /// Hot reload: a refused patch (or a rolled-back one) points every def of `new` back at
    /// `old` (the version stays loaded, unused). `pairs` as given to commit_version.
    pub fn revert_version(&mut self, old: u32, new: u32, pairs: &[(u32, u32)]) {
        if let Some(context) = self.source_versions.get(&old) {
            self.crates[self.file_crate[old as usize] as usize].context = context.clone();
        }
        self.parse_reuse.remove(&new);
        self.path_seg_cache.clear();
        self.scope_cache.clear();
        let back: Vec<(u32, u32)> = pairs.iter().map(|&(a, b)| (b, a)).collect();
        self.retarget_file(new, old, &back);
        self.collect_version_definitions(old, false, false, None).expect("previously collected definition body");
        if let Some(saved) = self.declaration_versions.remove(&new) { saved.restore(self); }
    }

    /// Hot reload: collects items a new file version added. `items`: (item, where it goes):
    /// `Ok(module)` for a module-level item, `Err(impl def)` for an associated item of an existing
    /// impl. Returns the new defs (names bound in their modules, impls registered).
    /// A live edit added variants at the end of enum `d` (M10): their defs, after the running
    /// ones. Returns how many variants the enum had (for `truncate_variants` on a refusal).
    pub fn add_variants(&mut self, d: DefId) -> usize {
        let (file, item, sub, dkrate, dscope) = {
            let df = self.def(d);
            (df.file, df.item, df.sub, df.krate, df.scope)
        };
        let li = sub as usize;
        let had = self.variants.get(li).map_or(0, |v| v.len());
        let names: Vec<Ident> = match &self.files[file as usize].ast.item(item).kind {
            ItemKind::Enum(r) => { let ast = &self.files[file as usize].ast; ast.list(ast.row(*r).variants).iter().map(|x| x.name).collect() }
            _ => return had,
        };
        let (krate, scope) = self.variants.get(li).and_then(|v| v.first()).map_or((dkrate, dscope), |&v0| (self.def(v0).krate, self.def(v0).scope));
        // The definition table describes the published source, including renamed
        // or reordered variants. Reload compares it with its running AdtT snapshot.
        for vi in 0..had.min(names.len()) {
            let name = self.ident(file, names[vi]);
            let vd = self.variants[li][vi];
            self.defs[vd.0 as usize].name = name;
        }
        for vi in had..names.len() {
            let vname = self.ident(file, names[vi]);
            let vd = self.add_def(Def { kind: DefKind::Variant, name: vname, parent: d, file, item, sub: vi as u32, is_pub: true, krate, scope });
            self.variants[li].push(vd);
        }
        had
    }

    /// Undoes `add_variants` (a refused edit): enum `d` has `n` variants again.
    pub fn truncate_variants(&mut self, d: DefId, n: usize) {
        let li = self.def(d).sub as usize;
        if let Some(v) = self.variants.get_mut(li) {
            v.truncate(n);
        }
    }

    pub fn add_items(&mut self, file: u32, items: &[(ItemId, Result<u32, DefId>)]) -> Result<Vec<DefId>, String> {
        if items.is_empty() {
            return Ok(Vec::new());
        }
        let first = self.defs.len();
        for &(iid, at) in items {
            match at {
                Ok(module) => self.collect_items(file, &[iid], module, "", false)?,
                Err(imp) => {
                    let krate = self.defs[imp.0 as usize].krate;
                    let module = self.defs[imp.0 as usize].scope;
                    if let Some(ad) = self.collect_assoc(file, iid, imp, krate, module)? {
                        let ii = self.defs[imp.0 as usize].sub as usize;
                        self.impls[ii].items.push(ad);
                        self.collect_body_items(file, iid, module, Some(ad))?;
                        crate::def_collector::collect_definitions(self, ad)?;
                    }
                }
            }
        }
        if self.imports_done {
            self.scope_cache.clear();
            self.glob_cache.clear(); self.glob_amb_cache.clear(); self.glob_prec_cache.clear();
        }
        // only the crates that can see `file` can see what it added (lead decision 52 R1)
        let visible = self.crates_seeing_files([file].into_iter());
        crate::assoc::collect_in(self, Some(&visible));
        Ok((first as u32..self.defs.len() as u32).map(DefId).collect())
    }

    /// Recollect one crate from the checker's current file versions. Other crates' ASTs
    /// and definitions stay resident. Old defs remain addressable until the caller drops
    /// their type tables and dependency edges; they are no longer bound in any namespace.
    pub fn recollect_check_crate(&mut self, krate: u32, files: std::collections::HashMap<String, u32>) -> Result<Vec<DefId>, String> {
        let root = self.crates[krate as usize].root_mod;
        let root_def = self.mods[root as usize].def;
        let path = self.crates[krate as usize].root_path.clone();
        let file = *files.get(&path).ok_or_else(|| format!("missing committed crate root {}", path))?;
        // Imports in dependent crates may still bind an old def (including glob caches).
        for im in &self.imports {
            if let Some(name) = im.bind {
                let m = &mut self.mods[im.module as usize];
                if m.types.get(&name).is_some_and(|d| self.defs[d.0 as usize].scope != im.module) { m.types.remove(&name); }
                if m.values.get(&name).is_some_and(|d| self.defs[d.0 as usize].scope != im.module) { m.values.remove(&name); }
            }
        }
        for m in &mut self.mods { m.globs.clear(); m.anon_traits.clear(); m.import_pub.clear(); }
        self.imports.retain(|i| self.mods[i.module as usize].krate != krate);
        for i in &mut self.imports { i.done = false; i.bound &= BOUND_INJECTED | BOUND_PREFIX_ONLY | BOUND_SELF_ONLY; }
        let old: Vec<_> = self.defs.iter().enumerate().filter(|(_, d)| d.krate == krate).map(|(i, _)| DefId(i as u32)).collect();
        self.unbind_defs(&old);
        self.impls.retain(|i| self.defs[i.def.0 as usize].krate != krate);
        for (i, imp) in self.impls.iter().enumerate() { self.defs[imp.def.0 as usize].sub = i as u32; }
        for m in &mut self.mods {
            if m.krate == krate {
                m.types.clear(); m.values.clear(); m.macros.clear(); m.globs.clear(); m.anon_traits.clear(); m.import_pub.clear();
            }
        }
        self.item_defs.retain(|_, d| self.defs[d.0 as usize].krate != krate);
        self.node_id_to_def_id.retain(|_, d| self.defs[d.0 as usize].krate != krate);
        self.def_id_to_node_id.retain(|d, _| self.defs[d.0 as usize].krate != krate);
        self.collected_definitions.retain(|d| self.defs[d.0 as usize].krate != krate);
        self.legacy_call_names.retain(|d, _| self.defs[d.0 as usize].krate != krate);
        self.repeat_expr_counts.retain(|d| self.defs[d.0 as usize].krate != krate);
        self.def_disambiguators.retain(|(d, _, _), _| self.defs[d.0 as usize].krate != krate);
        self.child_definitions.retain(|_, d| self.defs[d.0 as usize].krate != krate);
        self.impl_trait_in_trait_data.retain(|d, _| self.defs[d.0 as usize].krate != krate);
        self.associated_types_for_impl_traits.retain(|d, _| self.defs[d.0 as usize].krate != krate);
        self.opaque_types_defined_by.retain(|d, _| self.defs[d.0 as usize].krate != krate);
        self.opaque_to_rpitit.retain(|d, _| self.defs[d.0 as usize].krate != krate);
        self.trait_item_def_id.retain(|d, t| self.defs[d.0 as usize].krate != krate && self.defs[t.0 as usize].krate != krate);
        self.block_mods.retain(|_, m| self.mods[*m as usize].krate != krate);
        self.block_owner.retain(|_, d| self.defs[d.0 as usize].krate != krate);
        self.pending_macs.retain(|(f, _, _)| self.file_crate[*f as usize] != krate);
        // Tombstones cannot be rediscovered as callable bodies by a later scan.
        for d in old { if d != root_def { self.defs[d.0 as usize].kind = DefKind::Mod; self.defs[d.0 as usize].sub = root; } }
        self.set_def_file(root_def, file);
        self.set_mod_file(root, file);
        let items = self.files[file as usize].ast.root_items.clone();
        let first = self.defs.len();
        self.record_front = false;
        // Import failures are regenerated globally; preserve collection/macro errors
        // belonging to other crates, whose items are not recollected here.
        self.errors.retain(|e| !e.contains(": unresolved import `") && !self.file_paths.iter().zip(&self.file_crate).any(|(p, k)| *k == krate && e.starts_with(p)));
        self.trait_scope.clear();
        self.check_files = Some(files);
        self.imports_done = false;
        let result = self.collect_items(file, &items, root, dir_of(&path), true);
        self.check_files = None;
        result?;
        if self.crates[krate as usize].edition == 2015 {
            let pf = &self.files[file as usize];
            let no_std = pf.ast.root_attrs.iter().any(|a| a.inner && a.toks.hi > a.toks.lo && pf.tok_text(a.toks.lo) == "no_std");
            let inj = self.syms.intern(if no_std { "core" } else { "std" });
            let bound = self.mods[root as usize].types.contains_key(&inj) || self.imports.iter().any(|i| i.module == root && i.bind == Some(inj));
            if bound && self.crates[krate as usize].deps.iter().any(|x| x.0 == inj) { self.report_injected_clash(root, inj); }
            if !bound && self.crates[krate as usize].deps.iter().any(|x| x.0 == inj) {
                self.imports.push(Import { module: root, global: true, segs: vec![(SegKind::Ident, inj)], bind: Some(inj), glob: false, is_pub: false, done: false, file, pos: 0, ext: true, bound: BOUND_INJECTED });
            }
        }
        self.expand_pending(krate, self.crates[krate as usize].edition);
        let kname = self.syms.str(self.crates[krate as usize].name).to_string();
        std::sync::Arc::make_mut(&mut self.macros).seal_crate(&kname);
        let visible = self.crates_seeing_files([file].into_iter());
        self.resolve_imports_in(Some(&visible));
        Ok((first as u32..self.defs.len() as u32).map(DefId).collect())
    }

    /// Hot reload: takes defs out of their module's namespaces (items a version removed, or the
    /// items of a refused version); they stay loaded for the code that already uses them.
    /// Associated items leave their impl's item list.
    pub fn unbind_defs(&mut self, defs: &[DefId]) {
        if defs.is_empty() {
            return;
        }
        for &d in defs {
            let def = &self.defs[d.0 as usize];
            let (name, scope, parent) = (def.name, def.scope as usize, def.parent);
            let md = &mut self.mods[scope];
            if md.values.get(&name) == Some(&d) {
                md.values.remove(&name);
            }
            if md.types.get(&name) == Some(&d) {
                md.types.remove(&name);
            }
            if parent.0 != u32::MAX && (parent.0 as usize) < self.defs.len() && self.defs[parent.0 as usize].kind == DefKind::Impl {
                let ii = self.defs[parent.0 as usize].sub as usize;
                if ii < self.impls.len() && self.impls[ii].def == parent {
                    self.impls[ii].items.retain(|x| *x != d);
                }
            }
        }
        self.scope_cache.clear();
        self.glob_cache.clear(); self.glob_amb_cache.clear(); self.glob_prec_cache.clear();
    }

    /// Hot reload: the imports of the `use` item at `pos` of file `file` go (a removed or edited
    /// `use`): the names they bound to other modules' defs are unbound. Returns the undo record.
    pub fn unbind_use(&mut self, file: u32, pos: u32) -> Vec<UseUndo> {
        let mut out = Vec::new();
        for i in 0..self.imports.len() {
            let im = &self.imports[i];
            if im.file != file || im.pos != pos {
                continue;
            }
            let (m, bind, done, glob, bound) = (im.module, im.bind, im.done, im.glob, im.bound);
            let mut gone = Vec::new();
            // the bindings this import made (an item's own binding, or another import's, stays)
            if let (true, Some(name)) = (done, bind) {
                for (is_type, bit) in [(true, BOUND_TYPES), (false, BOUND_VALUES)] {
                    if bound & bit == 0 {
                        continue;
                    }
                    let md = &mut self.mods[m as usize];
                    let d = if is_type { md.types.remove(&name) } else { md.values.remove(&name) };
                    let flags = md.import_pub.remove(&(name, is_type)).unwrap_or(0);
                    if let Some(d) = d {
                        gone.push((is_type, name, d, flags));
                    }
                }
            }
            // a glob's names are looked up through it (glob_cache): taking it out of the module's
            // list is enough
            self.imports[i].done = true;
            self.imports[i].bind = None;
            self.imports[i].glob = false;
            self.imports[i].bound &= !(BOUND_TYPES | BOUND_VALUES);
            out.push(UseUndo { index: i, module: m, bind, done, glob, bound, gone });
        }
        self.scope_cache.clear();
        self.glob_cache.clear(); self.glob_amb_cache.clear(); self.glob_prec_cache.clear();
        out
    }

    /// Undoes `unbind_use` (a refused edit).
    pub fn rebind_use(&mut self, undo: Vec<UseUndo>) {
        for u in undo {
            let im = &mut self.imports[u.index];
            im.bind = u.bind;
            im.done = u.done;
            im.glob = u.glob;
            im.bound = u.bound;
            for (is_type, name, d, flags) in u.gone {
                let md = &mut self.mods[u.module as usize];
                if is_type { md.types.insert(name, d); } else { md.values.insert(name, d); }
                md.import_pub.insert((name, is_type), flags);
            }
        }
        self.scope_cache.clear();
        self.glob_cache.clear(); self.glob_amb_cache.clear(); self.glob_prec_cache.clear();
    }

    /// Hot reload: takes out the imports from index `i0` on (a refused edit's `use` items) and
    /// the names they bound (a name bound to a def of another module).
    pub fn unbind_imports_from(&mut self, i0: usize) {
        self.load_graph = None;
        if i0 >= self.imports.len() {
            return;
        }
        let gone: Vec<(u32, Sym, u8)> = self.imports.iter().skip(i0).filter(|im| im.done).filter_map(|im| im.bind.map(|b| (im.module, b, im.bound))).collect();
        self.imports.truncate(i0);
        for (m, name, bound) in gone {
            let md = &mut self.mods[m as usize];
            if bound & BOUND_VALUES != 0 {
                md.values.remove(&name);
                md.import_pub.remove(&(name, false));
            }
            if bound & BOUND_TYPES != 0 {
                md.types.remove(&name);
                md.import_pub.remove(&(name, true));
            }
        }
        self.imports_of_module.clear();
        for (j, im) in self.imports.iter().enumerate() {
            self.imports_of_module.entry(im.module).or_default().push(j as u32);
        }
        self.scope_cache.clear();
        self.glob_cache.clear(); self.glob_amb_cache.clear(); self.glob_prec_cache.clear();
    }

    /// Hot reload: binds defs again (a refused edit that had removed them).
    pub fn rebind_defs(&mut self, defs: &[DefId]) {
        for &d in defs {
            let def = &self.defs[d.0 as usize];
            let (name, scope, kind, parent) = (def.name, def.scope, def.kind, def.parent);
            match kind {
                DefKind::Fn | DefKind::Const | DefKind::Static => self.define(scope, false, name, d),
                DefKind::Struct | DefKind::Union | DefKind::Enum | DefKind::TypeAlias | DefKind::Trait | DefKind::Mod => self.define(scope, true, name, d),
                DefKind::AssocFn | DefKind::AssocConst | DefKind::AssocTy => {
                    if self.defs[parent.0 as usize].kind == DefKind::Impl {
                        let ii = self.defs[parent.0 as usize].sub as usize;
                        if !self.impls[ii].items.contains(&d) {
                            self.impls[ii].items.push(d);
                        }
                    }
                }
                _ => {}
            }
        }
        self.scope_cache.clear();
        self.glob_cache.clear(); self.glob_amb_cache.clear(); self.glob_prec_cache.clear();
    }

    fn retarget_file(&mut self, from: u32, to: u32, map: &[(u32, u32)]) {
        let m: HashMap<u32, u32> = map.iter().copied().collect();
        let mut renames: Vec<(DefId, Sym)> = Vec::new();
        self.reload_files_mut();
        fn reload(p: &mut std::sync::OnceLock<ReloadFiles>) -> &mut ReloadFiles { p.get_mut().unwrap() }
        for di in reload(&mut self.reload).defs.remove(&from).unwrap_or_default() {
            let di = di as usize;
            let d = &self.defs[di];
            let Some(&ni) = m.get(&d.item.0) else {
                reload(&mut self.reload).defs.entry(from).or_default().push(di as u32);
                continue;
            };
            reload(&mut self.reload).defs.entry(to).or_default().push(di as u32);
            let it = &self.files[to as usize].ast.items[ni as usize];
            // generated items are named by their position: the name follows the version
            if matches!(d.kind, DefKind::Const | DefKind::Static) {
                let n = &self.files[to as usize].src[it.name.lo as usize..it.name.hi as usize];
                let n = std::str::from_utf8(n).unwrap_or("").to_string();
                if !n.is_empty() && n.starts_with("__relax_") && self.syms.str(d.name) != n {
                    renames.push((DefId(di as u32), self.syms.intern(&n)));
                }
            }
            let d = &mut self.defs[di];
            d.file = to;
            d.item = ItemId(ni);
        }
        if !renames.is_empty() { self.scope_cache.clear(); }
        for (d, name) in renames {
            let old = self.defs[d.0 as usize].name;
            let scope = self.defs[d.0 as usize].scope as usize;
            let md = &mut self.mods[scope];
            if md.values.get(&old) == Some(&d) {
                md.values.remove(&old);
                md.values.insert(name, d);
            }
            if md.types.get(&old) == Some(&d) {
                md.types.remove(&old);
                md.types.insert(name, d);
            }
            self.defs[d.0 as usize].name = name;
        }
        for id in reload(&mut self.reload).impls.remove(&from).unwrap_or_default() {
            let im = &mut self.impls[id as usize];
            if let Some(&ni) = m.get(&im.item.0) { im.file = to; im.item = ItemId(ni); }
            reload(&mut self.reload).impls.entry(im.file).or_default().push(id);
        }
        for id in reload(&mut self.reload).mods.remove(&from).unwrap_or_default() {
            self.set_mod_file(id, to);
        }
        for (&a, &b) in m.iter() {
            if let Some(&d) = self.item_defs.get(&(from, a)) {
                self.item_defs.insert((to, b), d);
            }
        }
    }

    fn new_module(&mut self, def: DefId, parent_mod: Option<u32>, lexical_parent: Option<u32>, krate: u32, file: u32) -> u32 {
        let idx = self.mods.len() as u32;
        let normal = match lexical_parent {
            Some(lp) => self.mods[lp as usize].normal,
            None => idx,
        };
        if let Some(r) = self.reload.get_mut() { r.mods.entry(file).or_default().push(idx); }
        self.mods.push(Module {
            def,
            lexical_parent,
            normal,
            parent_mod,
            krate,
            file,
            types: HashMap::default(),
            values: HashMap::default(),
            macros: HashMap::default(),
            globs: Vec::new(),
            anon_traits: Vec::new(),
            import_pub: HashMap::default(),
        });
        idx
    }

    pub fn active(&self, file: u32, attrs: &[Attr]) -> bool {
        let krate = self.file_crate[file as usize];
        crate::cfg::active(self.src(file), attrs, &self.crates[krate as usize].cfg)
    }

    fn define(&mut self, module: u32, ns_type: bool, name: Sym, d: DefId) {
        if self.imports_done {
            self.scope_cache.clear();
        }
        let m = &mut self.mods[module as usize];
        if ns_type {
            m.types.insert(name, d);
        } else {
            m.values.insert(name, d);
        }
    }

    /// Collects items of one module body. `dir`: directory for child `mod x;` files.
    fn collect_items(&mut self, file: u32, items: &[ItemId], module: u32, dir: &str, mod_rs: bool) -> Result<(), String> {
        let _profile = crate::load_profile::span(self.session.runtime.profile, "scope-collect", &self.file_paths[file as usize]);
        let krate = self.mods[module as usize].krate;
        let mod_def = self.mods[module as usize].def;
        self.mod_dirs.insert(module, (dir.to_string(), mod_rs));
        for &iid in items {
            let (attrs_active, kind_tag) = {
                let it = self.files[file as usize].ast.item(iid);
                // a #[test] fn exists only in a test build (rustc's test harness removes it
                // otherwise, before name resolution)
                let test_off = matches!(it.kind, ItemKind::Fn(..))
                    && !self.crates[krate as usize].cfg.set.iter().any(|x| x == "test")
                    && crate::cfg::is_test(self.src(file), self.files[file as usize].ast.list(it.attrs));
                (!test_off && self.active(file, self.files[file as usize].ast.list(it.attrs)), 0)
            };
            let _ = kind_tag;
            if !attrs_active {
                continue;
            }
            self.reject_specialization(file, iid, None, false);
            let it_name = self.files[file as usize].ast.item(iid).name;
            let is_pub = !matches!(self.files[file as usize].ast.item(iid).vis, Vis::Private);
            let name = self.ident(file, it_name);
            // take a cheap descriptor of the kind to avoid holding a borrow
            let kind = item_tag(&self.files[file as usize].ast.item(iid).kind);
            let mk = |kind: DefKind| Def { kind, name, parent: mod_def, file, item: iid, sub: 0, is_pub, krate, scope: module };
            match kind {
                Tag::Fn => {
                    let d = self.add_def(mk(DefKind::Fn));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, false, name, d);
                    self.collect_body_items(file, iid, module, Some(d))?;
                }
                Tag::Struct => {
                    let d = self.add_def(mk(DefKind::Struct));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                    // tuple and unit structs also live in the value namespace
                    if let Some(StructItem { data, .. }) = { let ast = &self.files[file as usize].ast; ast.as_struct(&ast.item(iid).kind) } {
                        if !matches!(data, VariantData::Struct(_)) {
                            self.define(module, false, name, d);
                        }
                    }
                }
                Tag::Union => {
                    let d = self.add_def(mk(DefKind::Union));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                }
                Tag::Enum => {
                    let d = self.add_def(mk(DefKind::Enum));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                    let mut vars = Vec::new();
                    let n = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Enum(r) => self.files[file as usize].ast.row(*r).variants.len(),
                        _ => 0,
                    };
                    for vi in 0..n {
                        let vname_id = match &self.files[file as usize].ast.item(iid).kind {
                            ItemKind::Enum(r) => { let ast = &self.files[file as usize].ast; ast.list(ast.row(*r).variants)[vi].name }
                            _ => Ident::default(),
                        };
                        let vname = self.ident(file, vname_id);
                        let vd = self.add_def(Def { kind: DefKind::Variant, name: vname, parent: d, file, item: iid, sub: vi as u32, is_pub: true, krate, scope: module });
                        vars.push(vd);
                    }
                    let li = self.variants.len() as u32;
                    self.variants.push(vars);
                    self.defs[d.0 as usize].sub = li;
                    // items declared in a discriminant expression's block
                    self.collect_body_items(file, iid, module, Some(d))?;
                }
                Tag::Const | Tag::Static => {
                    let k = if kind == Tag::Const { DefKind::Const } else { DefKind::Static };
                    let d = self.add_def(mk(k));
                    self.item_defs.insert((file, iid.0), d);
                    // `const _: () = ...;` defines nothing nameable
                    if self.syms.str(name) != "_" {
                        self.define(module, false, name, d);
                    }
                    self.collect_body_items(file, iid, module, None)?;
                }
                Tag::TypeAlias => {
                    let d = self.add_def(mk(DefKind::TypeAlias));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                }
                Tag::Trait => {
                    let d = self.add_def(mk(DefKind::Trait));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                    let sub_items = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Trait(r) => { let ast = &self.files[file as usize].ast; ast.list(ast.row(*r).items).to_vec() }
                        _ => Vec::new(),
                    };
                    let mut list = Vec::new();
                    for si in sub_items {
                        let ad = self.collect_assoc(file, si, d, krate, module)?;
                        if let Some(ad) = ad {
                            list.push(ad);
                        }
                        self.collect_body_items(file, si, module, ad)?;
                        if let Some(d) = ad { crate::def_collector::collect_definitions(self, d)?; }
                    }
                    let ti = self.traits.len() as u32;
                    self.traits.push(list);
                    self.defs[d.0 as usize].sub = ti;
                }
                Tag::Impl => {
                    let d = self.add_def(mk(DefKind::Impl));
                    self.item_defs.insert((file, iid.0), d);
                    let sub_items = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Impl(r) => { let ast = &self.files[file as usize].ast; ast.list(ast.row(*r).items).to_vec() }
                        _ => Vec::new(),
                    };
                    let mut list = Vec::new();
                    for si in sub_items {
                        let ad = self.collect_assoc(file, si, d, krate, module)?;
                        if let Some(ad) = ad {
                            list.push(ad);
                        }
                        self.collect_body_items(file, si, module, ad)?;
                        if let Some(d) = ad { crate::def_collector::collect_definitions(self, d)?; }
                    }
                    let ii = self.impls.len() as u32;
                    if let Some(r) = self.reload.get_mut() { r.impls.entry(file).or_default().push(ii); }
                    self.impls.push(ImplData { def: d, file, item: iid, module, items: list });
                    self.defs[d.0 as usize].sub = ii;
                }
                Tag::Mod => {
                    let d = self.add_def(mk(DefKind::Mod));
                    self.item_defs.insert((file, iid.0), d);
                    self.define(module, true, name, d);
                    let m = self.new_module(d, Some(module), None, krate, file);
                    self.defs[d.0 as usize].sub = m;
                    let sname = self.syms.str(name).to_string();
                    let mut path_attr = Vec::new();
                    crate::crates::path_attrs_cfg(self.src(file), self.files[file as usize].ast.list(self.files[file as usize].ast.item(iid).attrs), &self.crates[krate as usize].cfg, &mut path_attr);
                    let inline = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Mod(Some(items)) => Some(self.files[file as usize].ast.list(*items).to_vec()),
                        _ => None,
                    };
                    // rustc_builtin_macros source_util.rs `include!`: the items of an included file
                    // resolve their `mod x;` in that file's folder (the module's relative
                    // ownership is kept)
                    let included;
                    let dir = match self.files[file as usize].authored_path(self.files[file as usize].ast.item(iid).lo) {
                        Some(p) if p != self.file_paths[file as usize] => {
                            included = if mod_rs { dir_of(&p).to_string() } else { format!("{}/{}", dir_of(&p), dir.rsplit('/').next().unwrap_or("")) };
                            included.as_str()
                        }
                        _ => dir,
                    };
                    match inline {
                        Some(inner) => {
                            // rustc_expand module.rs mod_dir_path: an inline module's `#[path]` is a folder
                            // relative to the module file's own folder
                            let d2 = if !path_attr.is_empty() {
                                let fdir = if mod_rs { dir.to_string() } else { dir_of(&self.file_paths[file as usize]).to_string() };
                                crate::crates::join_path(&fdir, &path_attr[0])
                            } else { format!("{}/{}", dir, sname) };
                            self.collect_items(file, &inner, m, &d2, true)?;
                        }
                        None => {
                            let base = if mod_rs { dir.to_string() } else { dir.to_string() };
                            let (cands, child_mod_rs): (Vec<String>, bool) = if !path_attr.is_empty() {
                                // Reference [items.mod.outlined.path]: relative to the file's folder, or,
                                // inside inline modules (and mod.rs files: mod_rs), to `dir`, which has
                                // the inline modules' folders in it
                                let fdir = if mod_rs { dir.to_string() } else { dir_of(&self.file_paths[file as usize]).to_string() };
                                let mut v = Vec::new();
                                for pa in &path_attr {
                                    v.push(crate::crates::join_path(&fdir, pa));
                                }
                                (v, true)
                            } else {
                                (vec![format!("{}/{}.rs", base, sname), format!("{}/{}/mod.rs", base, sname)], false)
                            };
                            let mut loaded = false;
                            for c in &cands {
                                let exists = self.check_files.as_ref().map_or(false, |m| m.contains_key(c)) || self.crates[krate as usize].context.exists(c);
                                self.module_probes.push((c.clone(), exists));
                                if self.record_front && !self.cur_image {
                                    self.front_probes[krate as usize].push((c.clone(), exists));
                                }
                                if exists {
                                    // rustc_expand module.rs `default_submod_path`: both `x.rs` and `x/mod.rs` exist
                                    if path_attr.is_empty() && cands.len() == 2 && *c == cands[0] && crate::lexer::validation_enabled() {
                                        let other = &cands[1];
                                        let other_exists = self.check_files.as_ref().map_or(false, |m| m.contains_key(other)) || self.crates[krate as usize].context.exists(other);
                                        self.module_probes.push((other.clone(), other_exists));
                                        if self.record_front && !self.cur_image {
                                            self.front_probes[krate as usize].push((other.clone(), other_exists));
                                        }
                                        if other_exists {
                                            let lo = self.files[file as usize].ast.item(iid).lo;
                                            self.errors.push(self.record_error(file, lo, "resolve", format!("E0761: file for module `{}` found at both \"{}\" and \"{}\"", sname, c, other)));
                                            loaded = true;
                                            break;
                                        }
                                    }
                                    if self.demand_plan.as_ref().is_some_and(|p| p.skips(self.crates[krate as usize].root_path(), c)) {
                                        let is_mod_rs = child_mod_rs || c.ends_with("/mod.rs");
                                        let cdir = if is_mod_rs { dir_of(c).to_string() } else { format!("{}/{}", dir_of(c), stem_of(c)) };
                                        self.skipped_files.insert(m, (c.clone(), cdir, is_mod_rs));
                                        loaded = true;
                                        break;
                                    }
                                    // rustc_expand module.rs: a file that includes itself through `mod` items
                                    let root = self.crates[krate as usize].root_path().to_string();
                                    if *c == root || self.module_load_stack.contains(c) {
                                        let mut chain = vec![root];
                                        chain.extend(self.module_load_stack.iter().cloned());
                                        chain.push(c.clone());
                                        let lo = self.files[file as usize].ast.item(iid).lo;
                                        self.errors.push(self.record_error(file, lo, "collect", format!("circular modules: {}", chain.join(" -> "))));
                                        loaded = true;
                                        break;
                                    }
                                    let edition = self.crates[krate as usize].edition;
                                    self.cur_mod_path = self.def_path(d);
                                    let f = self.load_file(c, edition, krate)?;
                                    self.set_mod_file(m, f);
                                    let is_mod_rs = child_mod_rs || c.ends_with("/mod.rs");
                                    let cdir = if is_mod_rs { dir_of(c).to_string() } else { format!("{}/{}", dir_of(c), stem_of(c)) };
                                    let items = self.files[f as usize].ast.root_items.clone();
                                    self.module_load_stack.push(c.clone());
                                    let collected = self.collect_items(f, &items, m, &cdir, is_mod_rs);
                                    self.module_load_stack.pop();
                                    collected?;
                                    loaded = true;
                                    break;
                                }
                            }
                            if !loaded {
                                // (rustc E0583; the check fails on it)
                                let pos = self.files[file as usize].ast.item(iid).lo;
                                let e = self.record_error(file, pos, "resolve", format!("E0583: file not found for module `{}`", sname));
                                self.errors.push(e);
                                return Err(format!("module file not found: {}", cands[0]));
                            }
                        }
                    }
                }
                Tag::ForeignMod => {
                    let sub_items = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::ForeignMod(r) => { let ast = &self.files[file as usize].ast; ast.list(ast.row(*r).items).to_vec() }
                        _ => Vec::new(),
                    };
                    for si in sub_items {
                        if !self.active(file, self.files[file as usize].ast.list(self.files[file as usize].ast.item(si).attrs)) {
                            continue;
                        }
                        self.reject_specialization(file, si, None, true);
        let n = self.files[file as usize].ast.item(si).name;
                        let sn = self.ident(file, n);
                        let ip = !matches!(self.files[file as usize].ast.item(si).vis, Vis::Private);
                        let k = match &self.files[file as usize].ast.item(si).kind {
                            ItemKind::Fn(..) => DefKind::ForeignFn,
                            ItemKind::Static(..) => DefKind::ForeignStatic,
                            // rustc_ast_lowering/src/item.rs:672: foreign type declarations.
                            ItemKind::TypeAlias { .. } => DefKind::ForeignTy,
                            _ => continue,
                        };
                        let d = self.add_def(Def { kind: k, name: sn, parent: mod_def, file, item: si, sub: 0, is_pub: ip, krate, scope: module });
                        self.item_defs.insert((file, si.0), d);
                        self.define(module, k != DefKind::ForeignFn && k != DefKind::ForeignStatic, sn, d);
                        crate::def_collector::collect_definitions(self, d)?;
                    }
                }
                Tag::Use => {
                    let (global, tree) = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::Use(r) => { let u = self.files[file as usize].ast.row(*r); (u.global, u.tree.clone()) }
                        _ => continue,
                    };
                    let pos = self.files[file as usize].ast.item(iid).lo;
                    let mut prefix = Vec::new();
                    self.flatten_use(file, module, global, &tree, &mut prefix, is_pub, pos);
                }
                Tag::ExternCrate => {
                    // `extern crate foo as bar;` binds `bar` (or `foo`) to the crate root
                    let rename = match &self.files[file as usize].ast.item(iid).kind {
                        ItemKind::ExternCrate(r) => *r,
                        _ => None,
                    };
                    let bind = match rename {
                        Some(r) => self.ident(file, r),
                        None => name,
                    };
                    let pos = self.files[file as usize].ast.item(iid).lo;
                    let krate = self.mods[module as usize].krate;
                    // `extern crate self as name;` names this crate (makepad-draw: derive output
                    // written against `makepad_draw::..` also compiles inside makepad-draw)
                    let this = self.syms.str(name) == "self";
                    if this && rename.is_none() && pos < self.files[file as usize].src_len {
                        let e = self.record_uncoded_error(file, pos, "resolve", "`extern crate self;` requires renaming".to_string());
                        self.errors.push(e);
                    }
                    if this && pos < self.files[file as usize].src_len {
                        let f = &self.files[file as usize];
                        let attrs: Vec<Attr> = f.ast.list(f.ast.item(iid).attrs).to_vec();
                        for a in attrs {
                            if a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "macro_use" {
                                let at = f.tok(a.toks.lo).lo;
                                let e = self.record_uncoded_error(file, at, "resolve", "`#[macro_use]` is not supported on `extern crate self`".to_string());
                                self.errors.push(e);
                            }
                        }
                    }
                    if this {
                        self.imports.push(Import { module, global: false, segs: vec![(SegKind::Crate, name)], bind: Some(bind), glob: false, is_pub, done: false, file, pos, ext: true, bound: 0 });
                    } else {
                        self.imports.push(Import { module, global: true, segs: vec![(SegKind::Ident, name)], bind: Some(bind), glob: false, is_pub, done: false, file, pos, ext: true, bound: 0 });
                    }
                    // at the crate root it also names the crate in the extern prelude (rustc:
                    // `extern crate alloc_no_stdlib as alloc;` makes `alloc::..` that crate in
                    // every module, ahead of a sysroot crate of the same name)
                    if rename.is_some() && self.crates[krate as usize].root_mod == module {
                        // build_reduced_graph_for_extern_crate: a macro-expanded `extern crate x as n;`
                        // at the root must not shadow an extern prelude name from `--extern`
                        if !this && self.syms.str(bind) != "_" && pos >= self.files[file as usize].src_len
                            && self.crates[krate as usize].deps.iter().any(|x| x.0 == bind)
                            && !self.imports.iter().any(|i| i.module == module && i.bind == Some(bind) && i.ext && i.pos < self.files[i.file as usize].src_len && i.file == file) {
                            let e = self.record_error(file, pos, "resolve", "macro-expanded `extern crate` items cannot shadow names passed with `--extern`".to_string());
                            self.errors.push(e);
                        }
                        if this {
                            self.crates[krate as usize].deps.insert(0, (bind, krate));
                        } else if let Some(&(_, c)) = self.crates[krate as usize].deps.iter().find(|x| x.0 == name) {
                            self.crates[krate as usize].deps.insert(0, (bind, c));
                        }
                    }
                }
                Tag::MacroRules => {
                    let d = self.add_def(mk(DefKind::MacroRules));
                    // a path names a `macro_rules!` macro only at the crate root, and only when it
                    // is `#[macro_export]`ed (rustc_resolve define_macro: the exported macro is a
                    // root binding; a textual macro has no module path unless a `use` re-exports it)
                    let f = &self.files[file as usize];
                    let exported = f.ast.list(f.ast.item(iid).attrs).iter().any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "macro_export");
                    if exported {
                        let root = self.crates[self.mods[module as usize].krate as usize].root_mod;
                        self.mods[root as usize].macros.insert(name, d);
                    }
                }
                Tag::Other => {
                    // an item-level `compile_error!("msg")` (a failed derive or built-in macro
                    // leaves one at its call) is an error at that position
                    let f = &self.files[file as usize];
                    let it = f.ast.item(iid);
                    if let ItemKind::Mac(m) = &it.kind {
                        let m = f.ast.row(*m);
                        let last = f.ast.segs(m.path.segs).last().map(|sg| f.text(sg.name).to_string()).unwrap_or_default();
                        if last == "global_asm" && !self.session.options.no_asm {
                            self.global_asms.push((file, iid, module));
                        } else if last == "naked_asm" && !self.session.options.no_asm {
                            self.errors.push(self.record_error(file, it.lo, "lower", "`naked_asm!` is allowed only as the body of a `#[unsafe(naked)]` function".to_string()));
                        } else if last == "global_asm" || last == "naked_asm" {
                            let e = relax_session::options::asm_blocked(&self.file_crate_name(file));
                            self.errors.push(self.record_error(file, it.lo, "lower", e));
                        } else if last == "compile_error" && m.toks.hi > m.toks.lo {
                            let lit = f.tok_text(m.toks.lo);
                            let msg = crate::lit::unescape_str(lit);
                            let e = self.record_error(file,it.lo,"macro",msg);
                            self.errors.push(e);
                        } else if !BUILTIN_MACROS.contains(&last.as_str()) && crate::builtins::macro_def(&last).is_none() && !crate::macro_reg::has_macro(&last) {
                            // no macro at the parse: maybe one a later expansion makes
                            self.pending_macs.push((file, iid, module));
                        }
                    }
                }
            }
            if let Some(&def) = self.item_defs.get(&(file, iid.0)) {
                crate::def_collector::collect_definitions(self, def)?;
            }
        }
        Ok(())
    }

    /// rustc_ast_passes/src/feature_gate.rs:168,374 @8d1a764. The stable
    /// configuration rejects specialization before any solver sees an impl.
    /// `owner`: the trait/impl of an associated item; `foreign`: an extern block item. A free or
    /// foreign fn/const/type alias (foreign static) and a trait item get ForbiddenDefault
    /// (ast_validation.rs:766-777,1518,1681,1720,1750,1779,2069-2078); another free item never
    /// consumes `default` (rustc_parse item.rs:229-247); a `default impl` and an impl item are
    /// the specialization gate (feature_gate.rs:168-176,374-383).
    fn reject_specialization(&mut self, file: u32, item: ItemId, owner: Option<DefId>, foreign: bool) {
        let ast = &self.files[file as usize].ast;
        let it = ast.item(item);
        if !it.default_ { return; }
        const FORBIDDEN: &str = "`default` is only allowed on items in trait impls";
        let message = match owner {
            None if matches!(it.kind, ItemKind::Impl { .. }) => "E0658: specialization is experimental".to_string(),
            None if matches!(it.kind, ItemKind::Fn(..) | ItemKind::Const(..) | ItemKind::TypeAlias { .. })
                || (foreign && matches!(it.kind, ItemKind::Static(..))) => FORBIDDEN.to_string(),
            None => {
                let descr = match &it.kind {
                    ItemKind::Use { .. } => "`use` import", ItemKind::ExternCrate(_) => "extern crate",
                    ItemKind::Fn(..) => "function", ItemKind::Struct(..) => "struct", ItemKind::Union(..) => "union",
                    ItemKind::Enum(..) => "enum", ItemKind::TypeAlias { .. } => "type alias",
                    ItemKind::Const(..) => "constant item", ItemKind::Static(..) => "static item",
                    ItemKind::Trait { .. } => "trait", ItemKind::TraitAlias(..) => "trait alias",
                    ItemKind::Impl { .. } => "implementation", ItemKind::Mod(_) => "module",
                    ItemKind::ForeignMod(..) => "extern block", ItemKind::MacroRules(_) => "macro definition",
                    ItemKind::Mac(_) => "item macro invocation",
                };
                let article = if matches!(it.kind, ItemKind::ExternCrate(_) | ItemKind::Enum(..) | ItemKind::Impl { .. } | ItemKind::ForeignMod(..) | ItemKind::Mac(_)) { "an" } else { "a" };
                format!("{article} {descr} cannot be `default`")
            }
            // a trait item: ForbiddenDefault, and the gate as for every associated item
            Some(owner) if self.def(owner).kind == DefKind::Trait => {
                let lo = it.lo;
                self.errors.push(self.record_error(file, lo, "collect", FORBIDDEN.to_string()));
                "E0658: specialization is experimental".to_string()
            }
            Some(_) => "E0658: specialization is experimental".to_string(),
        };
        let lo = it.lo;
        self.errors.push(self.record_error(file, lo, "collect", message));
    }

    fn collect_assoc(&mut self, file: u32, si: ItemId, owner: DefId, krate: u32, module: u32) -> Result<Option<DefId>, String> {
        if !self.active(file, self.files[file as usize].ast.list(self.files[file as usize].ast.item(si).attrs)) {
            return Ok(None);
        }
        self.reject_specialization(file, si, Some(owner), false);
        let n = self.files[file as usize].ast.item(si).name;
        let name = self.ident(file, n);
        let is_pub = !matches!(self.files[file as usize].ast.item(si).vis, Vis::Private);
        let kind = match &self.files[file as usize].ast.item(si).kind {
            ItemKind::Fn(..) => DefKind::AssocFn,
            ItemKind::Const(..) => DefKind::AssocConst,
            ItemKind::TypeAlias { .. } => DefKind::AssocTy,
            _ => return Ok(None),
        };
        let d = self.add_def(Def { kind, name, parent: owner, file, item: si, sub: 0, is_pub, krate, scope: module });
        self.item_defs.insert((file, si.0), d);
        Ok(Some(d))
    }

    /// Items declared inside a fn/const body get anonymous block modules.
    fn collect_body_items(&mut self, file: u32, iid: ItemId, module: u32, owner: Option<DefId>) -> Result<(), String> {
        let mut blocks = Vec::new();
        {
            let ast = &self.files[file as usize].ast;
            let kind = &ast.item(iid).kind;
            blocks_in_item_signature(ast, kind, &mut blocks);
            match kind {
                ItemKind::Fn(_, Some(b), _) => blocks.push(*b),
                // `const _: () = { impl X { .. } };` (bitflags and friends put impls there)
                ItemKind::Const(_, e) | ItemKind::Static(_, _, e) => if let Some(e) = e.get() { blocks_in_expr(ast, e, &mut blocks) },
                _ => {}
            }
        }
        for b in blocks {
            self.collect_block_items(file, b, module, owner)?;
        }
        Ok(())
    }

    /// Items in a body block (and blocks below it), in the order rustc's walk meets them: each
    /// block holding items is an anonymous module. The blocks to visit come from the file's
    /// FilePrep (blocks without items anywhere below them are not listed).
    fn collect_block_items(&mut self, file: u32, b: BlockId, module: u32, owner: Option<DefId>) -> Result<(), String> {
        let Some((items, subs)) = self.file_blocks[file as usize].get(&b.0).cloned() else { return Ok(()) };
        let mut inner = module;
        if !items.is_empty() {
            let krate = self.mods[module as usize].krate;
            let mdef = self.mods[module as usize].def;
            inner = self.new_module(mdef, Some(module), Some(module), krate, file);
            self.block_mods.insert((file, b.0), inner);
            if let Some(o) = owner {
                self.block_owner.insert(inner, o);
            }
            let (dir, mod_rs) = self.mod_dirs.get(&module).cloned().unwrap_or_else(|| (".".to_string(), true));
            self.collect_items(file, &items, inner, &dir, mod_rs)?;
        }
        for sb in subs {
            self.collect_block_items(file, sb, inner, owner)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn flatten_use(&mut self, file: u32, module: u32, global: bool, t: &UseTree, prefix: &mut Vec<(SegKind, Sym)>, is_pub: bool, pos: u32) {
        match t {
            UseTree::Path(seg, rest) => {
                let s = self.ident(file, seg.name);
                prefix.push((seg.kind(), s));
                let rest = *self.files[file as usize].ast.use_tree(*rest);
                self.flatten_use(file, module, global, &rest, prefix, is_pub, pos);
                prefix.pop();
            }
            UseTree::Name(seg, rename) => {
                let s = self.ident(file, seg.name);
                let mut segs = prefix.clone();
                // `use foo::{self}` imports `foo` itself
                let bind;
                let mut self_only = 0;
                if seg.kind() == SegKind::SelfValue && !segs.is_empty() {
                    self_only = BOUND_SELF_ONLY;
                    bind = match rename {
                        Some(r) => self.ident(file, *r),
                        None => segs[segs.len() - 1].1,
                    };
                } else {
                    segs.push((seg.kind(), s));
                    bind = match rename {
                        Some(r) => self.ident(file, *r),
                        None => s,
                    };
                }
                self.imports.push(Import { module, global, segs, bind: Some(bind), glob: false, is_pub, done: false, file, pos, ext: false, bound: self_only });
            }
            UseTree::Glob => {
                self.imports.push(Import { module, global, segs: prefix.clone(), bind: None, glob: true, is_pub, done: false, file, pos, ext: false, bound: 0 });
            }
            UseTree::Group(v) => {
                // `use a::b::{};` imports nothing, but its prefix is resolved (and privacy-checked)
                if v.is_empty() && !prefix.is_empty() {
                    self.imports.push(Import { module, global, segs: prefix.clone(), bind: None, glob: true, is_pub: false, done: false, file, pos, ext: false, bound: BOUND_PREFIX_ONLY });
                }
                for x in self.files[file as usize].ast.list(*v).to_vec() {
                    self.flatten_use(file, module, global, &x, prefix, is_pub, pos);
                }
            }
        }
    }

    // ------------------------------------------------------------ imports

    /// Resolves all imports to a fixpoint. Unresolved imports are reported.
    pub fn resolve_imports(&mut self) {
        self.resolve_imports_in(None);
    }

    fn resolve_imports_in(&mut self, visible: Option<&[bool]>) {
        self.imports_done = false;
        self.glob_cache.clear(); self.glob_amb_cache.clear(); self.glob_prec_cache.clear();
        self.trait_bind_cache.clear();
        self.scope_cache.clear();
        self.resolve_imports_fixpoint(0);
        self.imports_done = true;
        crate::assoc::collect_in(self, visible);
        crate::def_collector::lower_legacy_const_generics_in(self, visible);
    }

    /// A synthetic wrapper file of `krate` added to `module` (its items
    /// collected there); returns its file index.
    pub fn add_generated_file(&mut self, path: &str, mut pf: ParsedFile, krate: u32, module: u32) -> Result<u32, String> {
        pf.bind_sources(path, &self.sources)?;
        let prep = prepare_file(&mut pf, &self.crates[krate as usize].cfg);
        for &(lo, hi) in &prep.idents {
            let s = std::str::from_utf8(pf.bytes(lo, hi)).unwrap_or("?").to_string();
            self.syms.intern(&s);
        }
        self.map_image(path, krate, &mut pf);
        self.files.push(std::sync::Arc::new(pf));
        self.file_paths.push(path.to_string());
        self.exec_profile_file(self.file_paths.len() as u32 - 1);
        self.file_crate.push(krate);
        self.file_blocks.push(prep.blocks);
        let f = self.files.len() as u32 - 1;
        let items: Vec<ItemId> = self.files[f as usize].ast.root_items.iter().copied().collect();
        self.collect_items(f, &items, module, "", false)?;
        Ok(f)
    }

    /// A crate added to a resolved program (a run-time module): only its imports (from `i0`)
    /// are resolved and reported. The program's own imports and the scope caches stay as they
    /// are -- no existing module can see the new crate.
    pub fn resolve_imports_from(&mut self, i0: usize) {
        let visible = self.crates_seeing_files(self.imports.iter().skip(i0).map(|i| i.file));
        self.resolve_imports_from_in(i0, &visible);
        crate::def_collector::lower_legacy_const_generics_in(self, Some(&visible));
    }

    /// Reload already lowers legacy owners after the import transaction.
    pub fn resolve_imports_from_in(&mut self, i0: usize, visible: &[bool]) {
        self.imports_done = false;
        self.resolve_imports_fixpoint(i0);
        self.imports_done = true;
        crate::assoc::collect_in(self, Some(visible));
    }

    /// Append-only own-crate or restored-suffix load. Existing resolved links stay valid.
    pub fn resolve_imports_added(&mut self, i0: usize, first_def: usize) {
        self.imports_done = false;
        self.resolve_imports_fixpoint(i0);
        self.imports_done = true;
        let visible = self.crates_seeing_files(self.defs[first_def..].iter().map(|d| d.file));
        // a retained prefix's owners that lost their on-demand definitions are linked and lowered
        // again first, as the whole pass does them in definition order
        let relink = self.take_collected_relink();
        crate::assoc::collect_loaded_with(self, first_def, &visible, &relink.traits, &relink.impls);
        crate::def_collector::lower_legacy_const_generics_relinked(self, first_def, &visible, &relink.owners);
    }

    fn resolve_imports_fixpoint(&mut self, i0: usize) {
        self.resolve_imports_passes(i0);
        self.report_unresolved_imports(i0);
    }

    /// The import fixpoint's passes (no reporting).
    fn resolve_imports_passes(&mut self, i0: usize) {
        self.load_graph = None;
        let tr = crate::preload::trace();
        if i0 == 0 {
            self.imports_of_module.clear();
        }
        for j in i0..self.imports.len() {
            let m = self.imports[j].module;
            self.imports_of_module.entry(m).or_default().push(j as u32);
        }
        let mut passes = Vec::new();
        let mut settle = false;
        loop {
            self.import_settle = settle;
            let mut progress = false;
            let t0 = std::time::Instant::now();
            let (mut tried, mut ok) = (0, 0);
            for i in i0..self.imports.len() {
                if self.imports[i].done {
                    continue;
                }
                tried += 1;
                if self.try_import(i) {
                    self.imports[i].done = true;
                    progress = true;
                    ok += 1;
                }
            }
            if tr {
                passes.push(format!("{}/{} {:.1}ms{}", ok, tried, t0.elapsed().as_secs_f64() * 1e3, if settle { " (settle)" } else { "" }));
            }
            // a pass without progress: one more that settles the imports waiting on open globs
            // for their other namespace, then strict passes again while that makes progress
            if progress {
                settle = false;
            } else if settle {
                break;
            } else {
                settle = true;
            }
        }
        self.import_settle = false;
        if tr {
            eprintln!("boot-trace: resolve: {} imports ({} glob); passes (resolved/tried): {}", self.imports.len(), self.imports.iter().filter(|i| i.glob).count(), passes.join(", "));
        }
    }

    /// A module declared by a reached file whose own file the Rust selection skipped.
    fn skipped_module(&self, m: u32) -> bool {
        let module = &self.mods[m as usize];
        if module.parent_mod.is_none() || module.lexical_parent.is_some() { return false; }
        let d = self.def(module.def);
        d.kind == DefKind::Mod && module.file == d.file
            && matches!(self.files[d.file as usize].ast.item(d.item).kind, ItemKind::Mod(None))
    }

    /// The module an unresolved import's lookup stopped in: its deepest
    /// resolvable module prefix (the importing module for a first segment).
    fn import_stop(&self, i: usize) -> u32 {
        let im = &self.imports[i];
        let edition = self.crates[self.mods[im.module as usize].krate as usize].edition;
        let full = if im.glob { im.segs.len() } else { im.segs.len().saturating_sub(1) };
        for k in (1..=full).rev() {
            if let Some(d) = self.resolve_mod_path(im.module, im.global, &im.segs[..k], edition < 2018, true) {
                let def = self.def(d);
                return if def.kind == DefKind::Mod { def.sub } else { u32::MAX };
            }
        }
        im.module
    }

    /// Rust file selection skips modules no reached item looks into. An import
    /// stopping in such a module (or in one whose scope depends on another
    /// deferred import or glob) may still lack a binding; it is not an error.
    fn defer_skipped_imports(&mut self, i0: usize, pending: &[usize]) -> crate::fxhash::HashSet<usize> {
        let mut deferred = crate::fxhash::HashSet::default();
        if i0 == 0 { self.deferred_imports.clear(); }
        // a restored collected program has no plan but may still have skipped modules
        if !self.session.options.rs_lazy || (self.demand_plan.is_none() && self.skipped_files.is_empty()) { return deferred; }
        let mut incomplete: crate::fxhash::HashSet<u32> = (0..self.mods.len() as u32).filter(|&m| self.skipped_module(m)).collect();
        if incomplete.is_empty() { self.deferred_waits.clear(); self.deferred_binds.clear(); return deferred; }
        // Settling can finish an import in only one namespace. Its other binding may
        // still depend on a skipped file (for example a value found through a glob,
        // while the explicitly re-exported type is deferred). Keep that chain in the
        // miss graph too: rustc resolves a use independently in each namespace.
        let partial = self.imports.iter().enumerate().filter_map(|(i, im)| {
            let m = &self.mods[im.module as usize];
            (im.done && !im.glob && im.bind.is_some_and(|b| self.syms.str(b) != "_"
                && (!m.types.contains_key(&b) || !m.values.contains_key(&b)))).then_some(i)
        });
        let stops: Vec<(usize, u32)> = pending.iter().copied().chain(partial).map(|i| (i, self.import_stop(i))).collect();
        loop {
            let mut changed = false;
            for m in 0..self.mods.len() as u32 {
                if !incomplete.contains(&m) && self.mods[m as usize].globs.iter()
                    .any(|(t, _)| self.def(*t).kind == DefKind::Mod && incomplete.contains(&self.def(*t).sub)) {
                    incomplete.insert(m);
                    changed = true;
                }
            }
            for &(i, stop) in &stops {
                if deferred.contains(&i) || !incomplete.contains(&stop) { continue; }
                deferred.insert(i);
                changed |= incomplete.insert(self.imports[i].module);
            }
            if !changed { break; }
        }
        for &i in &deferred {
            self.imports[i].done = false;
            self.deferred_imports.push((self.imports[i].module, if self.imports[i].glob { None } else { self.imports[i].bind }));
        }
        self.compute_deferred_waits(&stops, &deferred);
        if i0 == 0 { self.load_graph = Some(self.load_graph_from_stops(stops)); }
        deferred
    }

    /// Which skipped modules each incomplete module waits on: a skipped module itself, an
    /// importer through its deferred imports' stops, a glob importer through its targets.
    fn compute_deferred_waits(&mut self, stops: &[(usize, u32)], deferred: &crate::fxhash::HashSet<usize>) {
        let mut waits: HashMap<u32, std::collections::BTreeSet<u32>> = HashMap::default();
        for &m in self.skipped_files.keys() { waits.entry(m).or_default().insert(m); }
        loop {
            let mut changed = false;
            for &(i, stop) in stops {
                if !deferred.contains(&i) { continue; }
                let Some(from) = waits.get(&stop).cloned() else { continue };
                let w = waits.entry(self.imports[i].module).or_default();
                let n = w.len();
                w.extend(from);
                changed |= w.len() != n;
            }
            for m in 0..self.mods.len() as u32 {
                let targets: Vec<u32> = self.mods[m as usize].globs.iter()
                    .filter(|(t, _)| self.def(*t).kind == DefKind::Mod).map(|(t, _)| self.def(*t).sub).collect();
                for t in targets {
                    let Some(from) = waits.get(&t).cloned() else { continue };
                    let w = waits.entry(m).or_default();
                    let n = w.len();
                    w.extend(from);
                    changed |= w.len() != n;
                }
            }
            if !changed { break; }
        }
        let mut binds: HashMap<(u32, Sym), Vec<u32>> = HashMap::default();
        for &(i, stop) in stops {
            let im = &self.imports[i];
            let (Some(bind), false) = (im.bind, im.glob) else { continue };
            if !deferred.contains(&i) { continue; }
            if let Some(w) = waits.get(&stop) { binds.entry((im.module, bind)).or_default().extend(w.iter().copied()); }
        }
        self.deferred_binds = binds;
        self.deferred_waits = waits.into_iter().map(|(m, w)| (m, w.into_iter().collect())).collect();
    }

    pub fn warm_skipped_indexes(&self) {
        for (m, (path, ..)) in &self.skipped_files {
            let c = &self.crates[self.mods[*m as usize].krate as usize];
            if let Ok(s) = c.context.read_source(path) { let _ = c.context.rust_outline(path, &s.src, c.edition, &c.cfg); }
        }
    }

    /// Whether the file of skipped module `m` may bind `name` for a lookup from outside it:
    /// an item, macro, child module or public import of that name, or a public glob import
    /// (unknown names). From the file's outline index (no parse); no index: true.
    pub fn skipped_declares(&self, m: u32, name: &str) -> bool {
        let Some((path, ..)) = self.skipped_files.get(&m) else { return false };
        // "": a deferred import of the missed name stopped in this module
        if name.is_empty() { return true; }
        let c = &self.crates[self.mods[m as usize].krate as usize];
        let index = {
            c.context.read_source(path).ok().and_then(|s| c.context.rust_outline(path, &s.src, c.edition, &c.cfg)).map(|o| std::sync::Arc::new(o.index.clone()))
        };
        let Some(ix) = index else { return true };
        let public = |v: &crate::rs_index::Vis| !matches!(v, crate::rs_index::Vis::Private);
        ix.items.iter().any(|i| i.name == name) || ix.macros.iter().any(|i| i.name == name)
            || ix.modules.iter().any(|i| i.name == name)
            || ix.uses.iter().any(|u| public(&u.vis) && (u.glob || u.alias.as_deref().unwrap_or_else(|| u.path.rsplit("::").next().unwrap_or("")) == name))
    }

    /// The skipped modules worth loading for `misses`: still skipped, and their outline can bind
    /// the missed name.
    pub fn misses_to_load(&self, misses: &[(u32, String)]) -> Vec<u32> {
        let mut mods = Vec::new();
        let trace = std::env::var_os("MAKEPAD_RUN_DEFERRED_TRACE").is_some();
        for (m, name) in misses {
            let skipped = self.skipped_files.contains_key(m);
            let declares = skipped && self.skipped_declares(*m, name);
            if trace {
                eprintln!("makepad-run deferred: miss `{}` in {} ({})", name, self.def_path(self.mods[*m as usize].def),
                    if !skipped { "not skipped" } else if declares { "load" } else { "outline does not bind it" });
            }
            if !mods.contains(m) && declares { mods.push(*m); }
        }
        mods
    }

    /// Unresolved or single-namespace imports participate in the miss graph. A
    /// settled import can still acquire its other namespace after a skipped load.
    fn import_may_wait(&self, i: usize) -> bool {
        let im = &self.imports[i];
        !im.done
            || (!im.glob
                && im.bind.is_some_and(|b| {
                    self.syms.str(b) != "_"
                        && (!self.mods[im.module as usize].types.contains_key(&b) || !self.mods[im.module as usize].values.contains_key(&b))
                }))
    }

    fn index_load_modules(&self, graph: &mut LoadGraph) {
        for m in graph.modules..self.mods.len() {
            let md = &self.mods[m];
            if let Some(p) = md.lexical_parent {
                graph.lexical.entry(p).or_default().push(m as u32);
            }
            for &(g, _) in &md.globs {
                let d = self.def(g);
                if d.kind == DefKind::Mod {
                    graph.globs.entry(d.sub).or_default().insert(m as u32);
                }
            }
        }
        graph.modules = self.mods.len();
    }

    fn make_load_graph(&self) -> LoadGraph {
        self.load_graph_from_stops((0..self.imports.len()).filter(|&i| self.import_may_wait(i)).map(|i| (i, self.import_stop(i))))
    }

    fn load_graph_from_stops(&self, stops: impl IntoIterator<Item = (usize, u32)>) -> LoadGraph {
        let mut graph = LoadGraph::default();
        self.index_load_modules(&mut graph);
        for (i, stop) in stops {
            graph.stop(i, Some(stop));
        }
        for (&m, waits) in &self.deferred_waits {
            for &s in waits {
                graph.waiters.entry(s).or_default().insert(m);
            }
        }
        graph
    }

    /// Follow binding changes through globs and imports that stopped at them. The
    /// source-ordered set preserves the full resolver's publication/pass order.
    fn load_import_cone(
        &self,
        graph: &LoadGraph,
        seeds: impl IntoIterator<Item = u32>,
        modules: &mut crate::fxhash::HashSet<u32>,
        imports: &mut std::collections::BTreeSet<usize>,
    ) {
        let mut todo: Vec<_> = seeds.into_iter().collect();
        while let Some(m) = todo.pop() {
            if !modules.insert(m) {
                continue;
            }
            if let Some(v) = graph.globs.get(&m) {
                todo.extend(v);
            }
            if let Some(v) = graph.lexical.get(&m) {
                todo.extend(v);
            }
            if let Some(v) = self.imports_of_module.get(&m) {
                imports.extend(v.iter().map(|&i| i as usize).filter(|&i| self.import_may_wait(i)));
            }
            if let Some(v) = graph.at_stop.get(&m) {
                for &i in v {
                    imports.insert(i);
                    todo.push(self.imports[i].module);
                }
            }
        }
    }

    /// Recompute the affected wait subgraph from empty sets, retaining unaffected
    /// boundary values. Starting empty removes the loaded stop even inside cycles.
    fn update_load_waits(
        &mut self,
        graph: &mut LoadGraph,
        modules: &crate::fxhash::HashSet<u32>,
        imports: &std::collections::BTreeSet<usize>,
    ) {
        for &m in modules {
            if let Some(old) = self.deferred_waits.remove(&m) {
                for s in old {
                    if let Some(v) = graph.waiters.get_mut(&s) {
                        v.remove(&m);
                    }
                }
            }
            if self.skipped_files.contains_key(&m) {
                self.deferred_waits.insert(m, vec![m]);
            }
        }
        let mut ordered: Vec<_> = modules.iter().copied().collect();
        ordered.sort_unstable();
        loop {
            let mut changed = false;
            for &m in &ordered {
                let mut waits = std::collections::BTreeSet::new();
                if self.skipped_files.contains_key(&m) {
                    waits.insert(m);
                }
                for &(g, _) in &self.mods[m as usize].globs {
                    let d = self.def(g);
                    if d.kind == DefKind::Mod {
                        if let Some(w) = self.deferred_waits.get(&d.sub) {
                            waits.extend(w);
                        }
                    }
                }
                if let Some(v) = self.imports_of_module.get(&m) {
                    for &i in v {
                        if let Some(stop) = graph.stops.get(&(i as usize)) {
                            if let Some(w) = self.deferred_waits.get(stop) {
                                waits.extend(w);
                            }
                        }
                    }
                }
                let waits: Vec<u32> = waits.into_iter().collect();
                if self.deferred_waits.get(&m).map_or(!waits.is_empty(), |old| *old != waits) {
                    self.deferred_waits.insert(m, waits);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for &m in modules {
            if let Some(w) = self.deferred_waits.get(&m) {
                for &s in w {
                    graph.waiters.entry(s).or_default().insert(m);
                }
            }
            if let Some(v) = self.imports_of_module.get(&m) {
                for &i in v {
                    if let Some(b) = self.imports[i as usize].bind {
                        self.deferred_binds.remove(&(m, b));
                    }
                }
            }
        }
        self.deferred_imports.retain(|(m, _)| !modules.contains(m));
        for &i in imports {
            let Some(stop) = graph.stops.get(&i) else { continue };
            let Some(w) = self.deferred_waits.get(stop).filter(|w| !w.is_empty()) else { continue };
            let im = &mut self.imports[i];
            im.done = false;
            self.deferred_imports.push((im.module, if im.glob { None } else { im.bind }));
            if let (Some(b), false) = (im.bind, im.glob) {
                self.deferred_binds.entry((im.module, b)).or_default().extend(w);
            }
        }
    }

    /// Loads a skipped file and resolves only its import dependency cone. Caller
    /// holds the unit/front write locks; no reader runs while bindings change.
    pub fn load_deferred_module(&mut self, m: u32) -> Result<usize, String> {
        self.load_deferred_module_changes(m).map(|(first, _)| first)
    }

    /// Also reports whether collection/import resolution changed the declaration
    /// surface, avoiding a whole-program binding census in the JIT's loader.
    pub fn load_deferred_module_changes(&mut self, m: u32) -> Result<(usize, bool), String> {
        if !self.skipped_files.contains_key(&m) {
            return Ok((self.defs.len(), false));
        }
        self.imports_done = false;
        let indexed = self.load_graph.is_none();
        let mut graph = self.load_graph.take().unwrap_or_else(|| self.make_load_graph());
        self.index_load_modules(&mut graph);
        let (path, cdir, is_mod_rs) = self.skipped_files.remove(&m).unwrap();
        let (d0, i0, m0) = (self.defs.len(), self.imports.len(), self.mods.len());
        let krate = self.mods[m as usize].krate;
        let edition = self.crates[krate as usize].edition;
        self.cur_mod_path = self.def_path(self.mods[m as usize].def);
        let f = self.load_file(&path, edition, krate)?;
        self.set_mod_file(m, f);
        let items = self.files[f as usize].ast.root_items.clone();
        self.collect_items(f, &items, m, &cdir, is_mod_rs)?;
        self.index_load_modules(&mut graph);
        for i in i0..self.imports.len() {
            self.imports_of_module.entry(self.imports[i].module).or_default().push(i as u32);
            graph.stop(i, Some(self.import_stop(i)));
        }
        let mut seeds = vec![m];
        seeds.extend(m0 as u32..self.mods.len() as u32);
        if let Some(v) = graph.waiters.get(&m) {
            seeds.extend(v);
        }
        let mut affected = crate::fxhash::HashSet::default();
        let mut imports = std::collections::BTreeSet::new();
        self.load_import_cone(&graph, seeds, &mut affected, &mut imports);
        let mut changed = crate::fxhash::HashSet::default();
        changed.insert(m);
        changed.extend(m0 as u32..self.mods.len() as u32);
        let (mut retried, mut settled, mut completed) = (0usize, 0usize, 0usize);
        let mut bindings_changed = self.defs.len() != d0 || self.imports.len() != i0;
        let done_before: Vec<_> = imports.iter().map(|&i| (i, self.imports[i].done)).collect();
        let mut settle = false;
        loop {
            self.import_settle = settle;
            let mut progress = false;
            // The cone already follows every possible completion through its
            // importers. New glob edges end in modules in that cone, so they
            // cannot add an outside consumer during these append-only passes.
            for &i in &imports {
                if self.imports[i].done {
                    continue;
                }
                retried += 1;
                settled += usize::from(settle);
                let module = self.imports[i].module;
                let md = &self.mods[module as usize];
                let before = (md.types.len(), md.values.len(), md.globs.len(), md.anon_traits.len());
                if self.try_import(i) {
                    self.imports[i].done = true;
                    progress = true;
                    completed += 1;
                }
                let md = &self.mods[module as usize];
                let after = (md.types.len(), md.values.len(), md.globs.len(), md.anon_traits.len());
                if before != after || self.imports[i].done {
                    bindings_changed |= before != after;
                    changed.insert(module);
                    if before.2 != after.2 {
                        // try_import inserts in source order, possibly before an
                        // already-resolved glob. The new edge is not always last.
                        for &(g, _) in &md.globs {
                            let d = self.def(g);
                            if d.kind == DefKind::Mod {
                                graph.globs.entry(d.sub).or_default().insert(module);
                            }
                        }
                    }
                }
            }
            if progress {
                settle = false;
            } else if settle {
                break;
            } else {
                settle = true;
            }
        }
        self.import_settle = false;
        for &i in &imports {
            graph.stop(i, self.import_may_wait(i).then(|| self.import_stop(i)));
        }
        self.update_load_waits(&mut graph, &affected, &imports);
        bindings_changed |= done_before.iter().any(|&(i, done)| self.imports[i].done != done);
        let invalid = graph.glob_closure(changed);
        // Scope/trait-scope also read the implicit prelude. A changed prelude
        // therefore invalidates those scopes even without an explicit glob edge.
        let prelude_changed = self.prelude_modules().iter().any(|m| invalid.contains(m));
        let files: crate::fxhash::HashSet<u32> = invalid.iter().map(|&m| self.mods[m as usize].file).collect();
        let scope = retain_load_memos(&mut self.scope_cache, |(m, _, _), v| !prelude_changed && !invalid.contains(m) && v.is_some());
        let glob = retain_load_memos(&mut self.glob_cache, |(m, _, _), v| !invalid.contains(m) && v.is_some());
        // Segments are file text, not resolution results. Invalidate the files of
        // changed scopes conservatively; unchanged files cannot acquire new text.
        let path_seg = retain_load_memos(&mut self.path_seg_cache, |(f, _, _, _), _| !files.contains(f));
        let trait_bind = retain_load_memos(&mut self.trait_bind_cache, |m, v| !invalid.contains(m) && !v.is_empty());
        let trait_scope = retain_load_memos(&mut self.trait_scope, |m, v| !prelude_changed && !invalid.contains(m) && !v.is_empty());
        self.imports_done = true;
        self.load_graph = Some(graph);
        if std::env::var_os("MAKEPAD_RUN_DEFERRED_TRACE").is_some() {
            let kept = scope.0 + glob.0 + path_seg.0 + trait_bind.0 + trait_scope.0;
            let dropped = scope.1 + glob.1 + path_seg.1 + trait_bind.1 + trait_scope.1;
            eprintln!("makepad-run deferred: front-load {} imports_retried={} imports_done={} settle_retries={} wait_modules={} modules_invalidated={} cache_kept={} cache_dropped={} prelude_changed={} scope={:?} glob={:?} path_seg={:?} trait_bind={:?} trait_scope={:?} index_built={}",
                self.def_path(self.mods[m as usize].def), retried, completed, settled, affected.len(),
                if prelude_changed { self.mods.len() } else { invalid.len() }, kept, dropped, prelude_changed,
                scope, glob, path_seg, trait_bind, trait_scope, indexed);
        }
        Ok((d0, bindings_changed))
    }

    /// After a batch of load_deferred_module calls: the crates that see the new files get
    /// what resolve_imports gives a loaded program -- trait-item links and synthesized RPITIT
    /// items and legacy const generic owners -- for what the batch can change: the definitions
    /// it added (assoc::collect_loaded, def_collector::lower_legacy_const_generics_added), as a
    /// declaration edit's reload scopes them (lead decision 52 R1). Without the links an impl
    /// item of a loaded module never answers its trait's projection (`<T as Deref>::Target` was
    /// {error}). `first_def`: the first definition the batch added.
    pub fn link_deferred_modules(&mut self, first_file: u32, first_def: usize) {
        if first_file as usize >= self.files.len() { return; }
        let visible = self.crates_seeing_files(first_file..self.files.len() as u32);
        crate::assoc::collect_loaded(self, first_def, &visible);
        crate::def_collector::lower_legacy_const_generics_added(self, first_def, &visible);
    }

    /// Unresolved imports (from `i0`) become errors.
    /// An unresolved import that still binds nothing reportable: a name bound in
    /// the other namespace, or a macro name (macros resolve by name at the call).
    fn import_reportable(&self, i: usize, macro_binds: &mut Option<crate::fxhash::HashSet<Sym>>) -> bool {
        let im = &self.imports[i];
        // bound in one namespace while waiting for the other: resolved
        if im.bound & (BOUND_TYPES | BOUND_VALUES) != 0 {
            return false;
        }
        // `use std::eprintln;` / `use alloc::format;`: built-in macros need no import
        if !im.glob {
            if let Some((_, last)) = im.segs.last() {
                // a macro is named by the module that holds it: `use m::assert;` of a module of
                // this crate that has no such macro (only `std`'s root and the crate root have
                // macros by name) is unresolved however the name is known elsewhere
                if im.segs.len() > 1 {
                    let krate = self.mods[im.module as usize].krate;
                    let edition = self.crates[krate as usize].edition;
                    if let Some(parent) = self.resolve_mod_path(im.module, im.global, &im.segs[..im.segs.len() - 1], edition < 2018, true) {
                        let pd = self.def(parent);
                        if pd.kind == DefKind::Mod && pd.krate == krate && self.crates[krate as usize].root_mod != pd.sub
                            && !self.mods[pd.sub as usize].macros.contains_key(last)
                            && !macro_binds.get_or_insert_with(|| self.macro_import_binds()).contains(last) {
                            return true;
                        }
                    }
                }
                if BUILTIN_MACROS.contains(&self.syms.str(*last)) {
                    return false;
                }
                // derives / proc macros of the graph's proc-macro crates (pmhost) and
                // Relax's built-in macros live outside the type/value namespaces
                if crate::macro_reg::has_macro(self.syms.str(*last)) || crate::builtins::is_macro_name(self.syms.str(*last)) {
                    return false;
                }
                // `use dep::some_macro;` of a `#[macro_export]` macro_rules! (macros resolve
                // by name at the call; this import names nothing in the other namespaces)
                // `pub use some_macro;` of a crate-local macro_rules! (2018 path-based
                // macro re-export within the crate)
                let kname = self.syms.str(self.crates[self.mods[im.module as usize].krate as usize].name);
                if self.macros.defs.get(self.syms.str(*last)).map_or(false, |v| v.iter().any(|d| d.exported || d.krate == kname)) {
                    return false;
                }
                // `use m::alias as other;` of a name another import binds to a macro
                // (derive output: `pub use __gen as ancestry;` .. `use x::ancestry as b;`)
                if macro_binds.get_or_insert_with(|| self.macro_import_binds()).contains(last) {
                    return false;
                }
            }
        }
        true
    }

    /// Unresolved imports (from `i0`) become errors.
    fn report_unresolved_imports(&mut self, i0: usize) {
        let mut macro_binds: Option<crate::fxhash::HashSet<Sym>> = None;
        let pending: Vec<usize> = (i0..self.imports.len()).filter(|&i| !self.imports[i].done).collect();
        let deferred = self.defer_skipped_imports(i0, &pending);
        let reportable: Vec<usize> = pending.into_iter().filter(|&i| self.import_reportable(i, &mut macro_binds)).collect();
        for i in reportable {
            if !deferred.contains(&i) {
                let im = &self.imports[i];
                let mut p = String::new();
                for (k, s) in &im.segs {
                    if !p.is_empty() {
                        p.push_str("::");
                    }
                    p.push_str(match k {
                        SegKind::Crate => "crate",
                        SegKind::SelfValue => "self",
                        SegKind::Super => "super",
                        _ => self.syms.str(*s),
                    });
                }
                if im.glob {
                    p.push_str("::*");
                }
                let f = &self.files[im.file as usize];
                let source_pos = f.diagnostic_pos(im.pos);
                let source_pos = source_pos.min(f.src_len);
                let end = f.src[source_pos as usize..f.src_len as usize].iter().position(|c| *c == b';').map_or(source_pos, |n| source_pos + n as u32);
                let name = p.rsplit("::").next().unwrap_or(&p);
                let pos = if f.token_input {
                    f.toks.iter().skip_while(|t| t.lo < im.pos).take_while(|t| t.kind != crate::lexer::T::Semi)
                        .filter(|t| f.bytes(t.lo, t.hi) == name.as_bytes()).last().map_or(im.pos, |t| t.lo)
                } else {
                    std::str::from_utf8(&f.src[source_pos as usize..end as usize]).ok().and_then(|s| s.rfind(name)).map_or(source_pos, |n|source_pos+n as u32)
                };
                let mut d = self.diagnostic(im.file, pos, "resolve", format!("unresolved import `{}`", p));
                self.suggest(&mut d, &self.source_location(im.file, pos), im.module);
                let location = self.source_location(im.file, im.pos);
                let (line, _) = location.source.line_col(location.span.lo);
                let human = format!("{}:{}: unresolved import `{}`", location.source.path(), line, p);
                self.errors.push(crate::diag::record(human,d));
            }
        }
    }

    fn try_import(&mut self, i: usize) -> bool {
        let module = self.imports[i].module;
        let global = self.imports[i].global;
        let glob = self.imports[i].glob;
        let segs = self.imports[i].segs.clone();
        let edition = self.crates[self.mods[module as usize].krate as usize].edition;
        let my_crate = self.mods[module as usize].krate;
        if glob {
            // `use *;` / `use ::*;` (2015): the path is just the `{{root}}` segment, the crate root
            // (rustc_resolve build_reduced_graph: an empty prefix imports the crate root)
            let target = if segs.is_empty() && edition < 2018 {
                self.mods[self.crates[my_crate as usize].root_mod as usize].def
            } else {
                match self.resolve_mod_path(module, global, &segs, edition < 2018, true) {
                    Some(t) => t,
                    None => return false,
                }
            };
            // only a module or an enum can be glob-imported (`use Alias::*` is unresolved)
            if !matches!(self.def(target).kind, DefKind::Mod | DefKind::Enum) {
                return false;
            }
            // a proc-macro crate exports only macros: a glob over it brings in no items
            let tk = self.def(target).krate;
            if tk != my_crate && self.crates[tk as usize].proc_macro {
                return true;
            }
            // (a prefix of an empty group: resolved, binds nothing)
            if self.imports[i].bound & BOUND_PREFIX_ONLY != 0 {
                return true;
            }
            let is_pub = self.imports[i].is_pub;
            self.glob_import.entry((module, target)).or_insert(i as u32);
            let order = |p: &Program, t: DefId| p.glob_import.get(&(module, t)).copied().unwrap_or(u32::MAX);
            let at = self.mods[module as usize].globs.partition_point(|(t, _)| order(self, *t) <= i as u32);
            self.mods[module as usize].globs.insert(at, (target, is_pub));
            return true;
        }
        let bind = self.imports[i].bind.unwrap_or(0);
        let (last_kind, last) = segs[segs.len() - 1];
        let mut tdef = None;
        let mut vdef = None;
        let mut src_mod: Option<u32> = None;
        if segs.len() == 1 && (global || (edition >= 2018 && last_kind == SegKind::Ident)) {
            // `use foo;` / `extern crate foo` / `use ::foo` -> extern crate, else item in scope
            if let Some(c) = self.extern_crate(module, last) {
                tdef = Some(self.mods[self.crates[c as usize].root_mod as usize].def);
            } else if !global {
                tdef = self.lookup_in_scope(module, last, true);
                vdef = if self.imports[i].bound & BOUND_SELF_ONLY != 0 { None } else { self.lookup_in_scope(module, last, false) };
            }
        } else if segs.len() == 1 && last_kind != SegKind::Ident {
            let m = match self.resolve_mod_path(module, global, &segs, edition < 2018, true) {
                Some(m) => m,
                None => return false,
            };
            tdef = Some(m);
        } else if segs.len() == 1 {
            // 2015 edition: `use x;` / `use {x, y};` name items of the crate root (Reference
            // items/use-declarations.md: "In the 2015 edition, `use` paths are relative to the crate
            // root"); extern crates are reached there too (2015 has them in the root)
            let root = self.mods[self.crates[my_crate as usize].root_mod as usize].def;
            tdef = self.lookup_in_container(root, last, true);
            vdef = self.lookup_in_container(root, last, false);
            if tdef.is_none() && vdef.is_none() {
                if let Some(c) = self.extern_crate(module, last) {
                    tdef = Some(self.mods[self.crates[c as usize].root_mod as usize].def);
                } else {
                    return self.lookup_macro_in_container(root, last);
                }
            }
        } else {
            let parent = match self.resolve_mod_path(module, global, &segs[..segs.len() - 1], edition < 2018, true) {
                Some(p) => p,
                None => return false,
            };
            if self.def(parent).kind == DefKind::Mod {
                src_mod = Some(self.def(parent).sub);
            }
            if last_kind == SegKind::SelfValue {
                // `use a::{self}` / `use a::{self as b}`: the module itself (use-declarations.md
                // [items.use.self])
                tdef = Some(parent);
            } else {
                tdef = self.lookup_in_container(parent, last, true);
                vdef = if self.imports[i].bound & BOUND_SELF_ONLY != 0 { None } else { self.lookup_in_container(parent, last, false) };
                // rustc ignores a namespace whose binding this module cannot see when the other
                // namespace is accessible (`use validate::validate;` where `validate` also holds
                // a private glob-imported module of that name); with neither accessible the
                // privacy error is reported
                if self.def(parent).kind == DefKind::Mod && tdef.is_some() && vdef.is_some() {
                    let sub = self.def(parent).sub;
                    let seen = |p: &Program, ns: bool| {
                        let mut q = GlobQuery { precise: true, ..Default::default() };
                        p.lookup_in_module_vis(sub, last, ns, &mut q).map_or(true, |(_, v)| p.vis_visible(v, module))
                    };
                    let (tv, vv) = (seen(self, true), seen(self, false));
                    if tv && !vv { vdef = None; } else if vv && !tv { tdef = None; }
                }
            }
            if tdef.is_none() && vdef.is_none() {
                // macros only (e.g. `use crate::log;` for macro_rules) count as resolved
                if self.imports[i].bound & BOUND_SELF_ONLY == 0 && self.lookup_macro_in_container(parent, last) {
                    return true;
                }
                return false;
            }
        }
        if tdef.is_none() && vdef.is_none() {
            return false;
        }
        // items of another proc-macro crate are invisible (only its macros are exported)
        let hidden = |p: &Program, d: Option<DefId>| d.map_or(false, |d| {
            let k = p.def(d).krate;
            k != my_crate && p.crates[k as usize].proc_macro && p.def(d).kind != DefKind::Mod
        });
        if hidden(self, tdef) {
            tdef = None;
        }
        if hidden(self, vdef) {
            vdef = None;
        }
        // a tuple struct with a field this module cannot see is imported without its constructor
        if let Some(d) = vdef {
            if self.def(d).krate == my_crate && self.tuple_ctor_hidden(d, module) {
                vdef = None;
            }
        }
        if tdef.is_none() && vdef.is_none() {
            return true;
        }
        if self.syms.str(bind) == "_" {
            // `use Trait as _;`: its methods are callable here
            if let Some(d) = tdef {
                if self.def(d).kind == DefKind::Trait {
                    let is_pub = self.imports[i].is_pub;
                    self.mods[module as usize].anon_traits.push((d, is_pub));
                }
            }
            return true;
        }
        let is_pub = self.imports[i].is_pub;
        if self.imports_done && (tdef.is_some() || vdef.is_some()) {
            self.scope_cache.clear();
        }
        let (ext, ipos, ifile, injected) = (self.imports[i].ext, self.imports[i].pos, self.imports[i].file, self.imports[i].bound & BOUND_INJECTED != 0);
        let flags = is_pub as u8 | if ext { IMPORT_EXTERN } else { 0 };
        for (ns_type, found) in [(true, tdef), (false, vdef)] {
            let Some(d) = found else { continue };
            let (bit, clash) = if ns_type { (BOUND_TYPES, CLASH_TYPES) } else { (BOUND_VALUES, CLASH_VALUES) };
            let m = &mut self.mods[module as usize];
            let taken = if ns_type { m.types.get(&bind).copied() } else { m.values.get(&bind).copied() };
            match taken {
                None => {
                    if ns_type { m.types.insert(bind, d); } else { m.values.insert(bind, d); }
                    m.import_pub.insert((bind, ns_type), flags);
                    self.imports[i].bound |= bit;
                }
                Some(old) => {
                    // the name is bound twice (rustc_resolve diagnostics.rs `report_conflict`)
                    if self.imports[i].bound & (clash | bit) == 0 && !injected {
                        let old_flags = m.import_pub.get(&(bind, ns_type)).copied();
                        self.imports[i].bound |= clash;
                        self.report_name_clash(bind, ns_type, old, old_flags, (ext, ipos, ifile));
                    }
                }
            }
        }
        // one namespace found while an import into the source module that binds the same name
        // is still pending (`mod parse;` + `pub use parse::parse;` at the root, imported as
        // `use crate::parse;` from a module resolved first): the other namespace may still come;
        // try again (rustc resolves each namespace of a `use` separately). The other namespace
        // may also come through a glob of `src`, from an import still open in the glob's
        // module or deeper (`use makepad_widgets::Rect`: the value `Rect`, a style fn of
        // `vector::*`, resolves while the struct `Rect` of `makepad_widgets_core::*` ->
        // `makepad_draw::*` -> .. -> `makepad_math::*` waits on an open glob): rustc's lookup
        // through a module with an unresolved glob is undetermined, not "not found"
        // (rustc_resolve ident.rs resolve_ident_in_module_unadjusted: Undetermined).
        if tdef.is_none() != vdef.is_none() && segs.len() > 1 && !self.import_settle {
            if let Some(src) = src_mod {
                if self.open_binding_through_globs(src, last, i) {
                    return false;
                }
            }
        }
        true
    }

    /// The injected `extern crate std;` (`core` under `#![no_std]`) of an edition 2015 root against
    /// a binding of the same name written in the crate: E0259 (an `extern crate`), E0254 (a `use`),
    /// E0260 (an item), reported on the written one.
    fn report_injected_clash(&mut self, root: u32, name: Sym) {
        let n = self.syms.str(name).to_string();
        let found = self.imports.iter().find(|i| i.module == root && i.bind == Some(name) && i.bound & BOUND_INJECTED == 0).map(|i| (i.ext, i.pos, i.file));
        let (code, file, pos) = match found {
            Some((true, pos, file)) => ("E0259", file, pos),
            Some((false, pos, file)) => ("E0254", file, pos),
            None => {
                let Some(&d) = self.mods[root as usize].types.get(&name) else { return };
                let od = self.def(d);
                if od.item.0 == u32::MAX { return; }
                let (f, item) = (od.file, od.item);
                let Some(it) = self.files.get(f as usize).and_then(|pf| pf.ast.items.get(item.0 as usize)) else { return };
                ("E0260", f, it.lo)
            }
        };
        let msg = format!("{code}: the name `{n}` is defined multiple times -- note: `{n}` must be defined only once in the type namespace of this module");
        let e = self.record_error(file, pos, "resolve", msg);
        self.errors.push(e);
    }

    /// E0252/E0254/E0255/E0259/E0260 (rustc_resolve diagnostics.rs `report_conflict`): the import
    /// `new` binds a name that `old` already bound in the module (`old_flags`: the import's flags,
    /// None for an item). The error is on the later of the two bindings.
    fn report_name_clash(&mut self, name: Sym, ns_type: bool, old: DefId, old_flags: Option<u8>, new: (bool, u32, u32)) {
        let (n_ext, npos, nfile) = new;
        let o_ext = old_flags.map_or(false, |f| f & IMPORT_EXTERN != 0);
        let o_item = old_flags.is_none();
        let code = if o_ext && n_ext {
            "E0259"
        } else if o_ext || n_ext {
            if o_item { "E0260" } else { "E0254" }
        } else if o_item {
            "E0255"
        } else {
            "E0252"
        };
        // an item bound earlier than the import resolved may come later in the source
        let (file, pos) = if o_item {
            let od = self.def(old);
            let item = od.item;
            let f = od.file;
            match self.files.get(f as usize).and_then(|pf| pf.ast.items.get(item.0 as usize)) {
                Some(it) if item.0 != u32::MAX && f == nfile && it.lo > npos => (f, it.lo),
                _ => (nfile, npos),
            }
        } else {
            (nfile, npos)
        };
        let ns = if ns_type { "type" } else { "value" };
        let n = self.syms.str(name).to_string();
        let msg = format!("{code}: the name `{n}` is defined multiple times -- note: `{n}` must be defined only once in the {ns} namespace of this module");
        let e = self.record_error(file, pos, "resolve", msg);
        self.errors.push(e);
    }

    /// Whether module `src` or a module its resolved globs reach (transitively) has an open
    /// import other than `except` that may bind `name`: one naming it, or a glob.
    fn open_binding_through_globs(&self, src: u32, name: Sym, except: usize) -> bool {
        let mut seen = crate::fxhash::HashSet::default();
        let mut stack = vec![src];
        while let Some(m) = stack.pop() {
            if !seen.insert(m) {
                continue;
            }
            // (an index by module, built per fixpoint run)
            let open = self.imports_of_module.get(&m).map_or(false, |v| {
                v.iter().any(|&j| {
                    let im = &self.imports[j as usize];
                    j as usize != except && !im.done && (im.bind == Some(name) || im.glob)
                })
            });
            if open {
                return true;
            }
            for &(g, _) in &self.mods[m as usize].globs {
                let gd = self.def(g);
                if gd.kind == DefKind::Mod {
                    stack.push(gd.sub);
                }
            }
        }
        false
    }

    /// Whether module `r` is `x` or inside it (lexically or as a child module).
    pub fn in_module(&self, r: u32, x: u32) -> bool {
        let mut cur = r;
        loop {
            if cur == x {
                return true;
            }
            let md = &self.mods[cur as usize];
            match md.lexical_parent.or(md.parent_mod) {
                Some(p) => cur = p,
                None => return false,
            }
        }
    }

    pub fn vis_visible(&self, v: BindVis, to: u32) -> bool {
        match v {
            None => true,
            Some(x) => self.in_module(to, x),
        }
    }

    /// The more restrictive of two visibilities (rustc: a glob import's binding has
    /// min(glob visibility, imported binding's visibility)).
    pub fn vis_min(&self, a: BindVis, b: BindVis) -> BindVis {
        match (a, b) {
            (None, x) | (x, None) => x,
            (Some(x), Some(y)) => {
                if self.in_module(x, y) {
                    Some(x)
                } else {
                    Some(y)
                }
            }
        }
    }

    /// Visibility of module `m`'s own binding `name` resolving to `d`.
    fn binding_vis(&self, m: u32, name: Sym, ns_type: bool, d: DefId) -> BindVis {
        let md = &self.mods[m as usize];
        let is_pub = match md.import_pub.get(&(name, ns_type)) {
            Some(p) => p & IMPORT_PUB != 0,
            None => self.def(d).is_pub,
        };
        if is_pub {
            None
        } else {
            Some(md.normal)
        }
    }

    /// Whether a resolution went through the import binding `name` of module `m`.
    pub fn import_binding_used(&self, m: u32, name: Sym) -> bool {
        let u = self.import_used.lock().unwrap_or_else(|e| e.into_inner());
        u.contains(&(m, name, true)) || u.contains(&(m, name, false))
    }

    /// The imports of the program (use items and `extern crate`).
    pub fn imports_iter(&self) -> impl Iterator<Item = &Import> { self.imports.iter() }

    /// Imports of the checked crates that name nothing rustc would resolve but Relax's lenient
    /// resolution accepts: a path through `alloc` without `extern crate alloc;` (not in the extern
    /// prelude), and a macro-named import through a module that does not exist. Run once the
    /// checked crates are known; each is rustc's E0432.
    pub fn strict_import_errors(&self, own: &[bool]) -> Vec<String> {
        let mut out = Vec::new();
        let mut macro_binds: Option<crate::fxhash::HashSet<Sym>> = None;
        for i in 0..self.imports.len() {
            let im = &self.imports[i];
            let krate = self.mods[im.module as usize].krate;
            if !own.get(krate as usize).copied().unwrap_or(false) || im.ext || im.segs.is_empty() { continue; }
            if im.pos >= self.files[im.file as usize].src_len { continue; }
            let edition = self.crates[krate as usize].edition;
            let first_alloc = !im.global && edition >= 2018 && im.segs[0].0 == SegKind::Ident && self.syms.str(im.segs[0].1) == "alloc"
                && !self.alloc_declared(krate) && self.lookup_in_module(im.module, im.segs[0].1, true, &mut Vec::new()).is_none();
            let macro_through_missing = !im.done && !im.glob && im.segs.len() > 1
                && self.resolve_mod_path(im.module, im.global, &im.segs[..im.segs.len() - 1], edition < 2018, true).is_none()
                && !self.import_reportable(i, &mut macro_binds);
            if !first_alloc && !macro_through_missing { continue; }
            let p: Vec<&str> = im.segs.iter().map(|(k, s)| match k { SegKind::Crate => "crate", SegKind::SelfValue => "self", SegKind::Super => "super", _ => self.syms.str(*s) }).collect();
            let f = &self.files[im.file as usize];
            let pos = f.diagnostic_pos(im.pos).min(f.src_len);
            out.push(self.record_error(im.file, pos, "resolve", format!("E0432: unresolved import `{}`", p.join("::"))));
        }
        out
    }

    /// `extern crate alloc;` is written at the root of `krate`.
    fn alloc_declared(&self, krate: u32) -> bool {
        let root = self.crates[krate as usize].root_mod;
        self.imports.iter().any(|i| i.ext && i.module == root && i.segs.len() == 1 && self.syms.str(i.segs[0].1) == "alloc")
    }

    /// Whether a path resolved through the extern prelude entry `name` of crate `krate`.
    pub fn extern_prelude_used(&self, krate: u32, name: Sym) -> bool {
        self.extern_used.lock().unwrap_or_else(|e| e.into_inner()).contains(&(krate, name))
    }

    /// Whether a resolution went through the glob import of `target` in module `m`.
    pub fn glob_import_used(&self, m: u32, target: DefId) -> bool {
        self.glob_used.lock().unwrap_or_else(|e| e.into_inner()).contains(&(m, target))
    }

    /// The paths a missing name could be imported by from module `from` (rustc's E0412/E0425
    /// "consider importing", but over the whole program: items that exist, not look-alikes): for
    /// each item named `name` in the namespace, the shortest path through public modules and
    /// public re-exports (globs included) from this crate (`crate::..`) or a dependency (by its
    /// extern name). One per item, shortest first.
    pub fn import_suggestions(&self, from: u32, name: Sym, ns_type: bool) -> Vec<String> {
        self.import_candidates(from, name, ns_type).into_iter().map(|x| x.1).collect()
    }

    /// `import_suggestions` with the items: (def, path).
    pub fn import_candidates(&self, from: u32, name: Sym, ns_type: bool) -> Vec<(DefId, String)> {
        let krate = self.mods[from as usize].krate;
        // roots: this crate, then its dependencies (extern prelude, std family included)
        let mut roots: Vec<(String, u32)> = vec![("crate".to_string(), self.crates[krate as usize].root_mod)];
        for (n, c) in &self.crates[krate as usize].deps {
            roots.push((self.syms.str(*n).to_string(), self.crates[*c as usize].root_mod));
        }
        let mut found: Vec<(DefId, String)> = Vec::new();
        let mut seen: crate::fxhash::HashSet<u32> = crate::fxhash::HashSet::default();
        let mut queue: std::collections::VecDeque<(u32, String, bool)> = std::collections::VecDeque::new();
        for (p, m) in roots {
            let own = self.mods[m as usize].krate == krate;
            queue.push_back((m, p, own));
        }
        let mut steps = 0;
        while let Some((m, path, own)) = queue.pop_front() {
            if !seen.insert(m) {
                continue;
            }
            steps += 1;
            if steps > 50_000 {
                break;
            }
            let md = &self.mods[m as usize];
            // the name here (a public binding, or any binding in this crate)
            let tbl = if ns_type { &md.types } else { &md.values };
            if let Some(&d) = tbl.get(&name) {
                let vis_ok = own || self.binding_vis(m, name, ns_type, d).is_none();
                let kind_ok = !matches!(self.def(d).kind, DefKind::Mod);
                if vis_ok && kind_ok {
                    let p = format!("{}::{}", path, self.syms.str(name));
                    match found.iter_mut().find(|x| x.0 == d) {
                        // the same item by a better path (std's facade over core / alloc, shorter)
                        Some(x) => {
                            if path_rank(&p) < path_rank(&x.1) {
                                x.1 = p;
                            }
                        }
                        None => found.push((d, p)),
                    }
                }
            }
            // public child modules and re-exported modules
            let mut subs: Vec<(Sym, DefId)> = md.types.iter().filter(|(_, d)| self.def(**d).kind == DefKind::Mod).map(|(n, d)| (*n, *d)).collect();
            subs.sort_by_key(|x| self.syms.str(x.0).to_string());
            for (n, d) in subs {
                let pub_ok = own || self.binding_vis(m, n, true, d).is_none();
                if !pub_ok {
                    continue;
                }
                let sub = self.def(d).sub;
                let own2 = own && self.mods[sub as usize].krate == krate;
                queue.push_back((sub, format!("{}::{}", path, self.syms.str(n)), own2));
            }
            // glob re-exports: the target's names are this module's
            // (a private glob of another crate's module into this crate is reached through
            // that crate's own path instead: `makepad_widgets::widget::X`, not `crate::widget::X`)
            for (t, p) in &md.globs {
                if self.def(*t).kind != DefKind::Mod {
                    continue;
                }
                let sub = self.def(*t).sub;
                let other = self.mods[sub as usize].krate != krate;
                if *p && !(own && other) || (own && !other) {
                    let own2 = own && !other;
                    queue.push_front((sub, path.clone(), own2));
                }
            }
        }
        found.sort_by_key(|x| (path_rank(&x.1), x.1.clone()));
        found
    }

    /// Relaxed Rust "imports" (relaxed-rust.md A9): a name missing from scope that names exactly
    /// one item in the program is that item, as if `use <path>;` were written at the top of the
    /// file -- when module `from` is the file's own module (an inline `mod` or a fn body's scope
    /// would need the `use` elsewhere) and the crate opted in. Records the note and the
    /// write-back; None otherwise (the strict error stands).
    pub fn relaxed_import(&self, from: u32, name: Sym, ns_type: bool, file: u32, pos: u32) -> Option<DefId> {
        let md = &self.mods[from as usize];
        if !self.crates[md.krate as usize].relaxed(self).iter().any(|f| f == "imports") {
            return None;
        }
        // the file's own module: a named module whose file is this one and that is no inline
        // module (its definition item is a `mod x;` in another file, or it is a crate root)
        if md.lexical_parent.is_some() || md.file != file {
            return None;
        }
        let def = self.def(md.def);
        let is_root = def.parent == NO_DEF;
        if !is_root && def.file == file {
            return None;
        }
        let c = self.import_candidates(from, name, ns_type);
        if c.len() != 1 {
            return None;
        }
        let (d, path) = c.into_iter().next().unwrap();
        let location = self.source_location(file, pos);
        let at = use_insert_pos(location.source.bytes());
        let (l, col) = location.source.line_col(location.span.lo);
        let code = if ns_type { "E0412" } else { "E0425" };
        crate::relaxed::note(crate::relaxed::Note {
            at: format!("{}:{}:{}", location.source.path(), l, col),
            feature: "imports",
            code,
            msg: format!("accepted only with relaxed imports: `{}` is not in scope; will write: `use {};`", self.syms.str(name), path),
            fix: Some((location.source.path().to_string(), vec![crate::relaxed::Edit { lo: at, hi: at, text: format!("use {};\n", path) }])),
        });
        Some(d)
    }

    pub fn extern_crate(&self, module: u32, name: Sym) -> Option<u32> {
        let krate = self.mods[module as usize].krate;
        for (n, c) in &self.crates[krate as usize].deps {
            if *n == name {
                if record_import_uses() { self.extern_used.lock().unwrap_or_else(|e| e.into_inner()).insert((krate, name)); }
                return Some(*c);
            }
        }
        // `$crate` from another crate's macro: `::__relax_crate_<name>` is that crate by identity
        if let Some(cn) = self.syms.str(name).strip_prefix("__relax_crate_") {
            for (i, c) in self.crates.iter().enumerate() {
                if self.syms.str(c.name) == cn {
                    return Some(i as u32);
                }
            }
        }
        None
    }

    /// Resolves a path to a container (module, enum, trait, type). `from_root`: 2015-style
    /// use paths are crate-relative.
    /// The module a visibility restricts an item declared in module `scope` to (None: `pub`,
    /// visible everywhere): private / `pub(self)` its own module, `pub` the crate root,
    /// `pub(super)` the parent, `pub(in path)` that module (rustc ty::Visibility::Restricted).
    pub fn vis_module(&self, file: u32, scope: u32, vis: &Vis) -> Option<u32> {
        let normal = self.mods[scope as usize].normal;
        match vis {
            Vis::Pub => None,
            Vis::Private | Vis::SelfMod => Some(normal),
            Vis::Crate => Some(self.crates[self.mods[normal as usize].krate as usize].root_mod),
            Vis::Super => Some(self.mods[normal as usize].parent_mod.map_or(normal, |p| self.mods[p as usize].normal)),
            Vis::In(p) => {
                let p = self.files[file as usize].ast.row(*p);
                let segs = path_segs(self, file, p);
                let d = self.resolve_mod_path(normal, p.global(), &segs, false, false)?;
                if self.def(d).kind == DefKind::Mod {
                    Some(self.def(d).sub)
                } else {
                    None
                }
            }
        }
    }

    /// Is module `m` (a body's scope) inside module `target` (or `target` itself)?
    pub fn mod_within(&self, m: u32, target: u32) -> bool {
        let target = self.mods[target as usize].normal;
        let mut cur = Some(m);
        let mut guard = 0;
        while let Some(x) = cur {
            if x == target || self.mods[x as usize].normal == target {
                return true;
            }
            let md = &self.mods[x as usize];
            cur = md.lexical_parent.or(md.parent_mod);
            guard += 1;
            if guard > 256 {
                break;
            }
        }
        false
    }

    pub fn resolve_mod_path(&self, module: u32, global: bool, segs: &[(SegKind, Sym)], from_root: bool, in_use: bool) -> Option<DefId> {
        let krate = self.mods[module as usize].krate;
        let mut cur: Option<DefId> = None;
        for (i, (kind, name)) in segs.iter().enumerate() {
            let next = if i == 0 {
                match kind {
                    SegKind::Crate => Some(self.mods[self.crates[krate as usize].root_mod as usize].def),
                    SegKind::SelfValue => Some(self.mods[self.mods[module as usize].normal as usize].def),
                    SegKind::Super => {
                        let n = self.mods[module as usize].normal;
                        let p = self.mods[n as usize].parent_mod?;
                        Some(self.mods[self.mods[p as usize].normal as usize].def)
                    }
                    SegKind::SelfType => None,
                    SegKind::Ident => {
                        if global {
                            match self.extern_crate(module, *name) {
                                Some(c) => Some(self.mods[self.crates[c as usize].root_mod as usize].def),
                                None => {
                                    let r = self.crates[krate as usize].root_mod;
                                    self.lookup_in_module(r, *name, true, &mut Vec::new())
                                }
                            }
                        } else if from_root && in_use {
                            let r = self.crates[krate as usize].root_mod;
                            match self.lookup_in_module(r, *name, true, &mut Vec::new()) {
                                Some(d) => Some(d),
                                None => match self.extern_crate(module, *name) {
                                    Some(c) => Some(self.mods[self.crates[c as usize].root_mod as usize].def),
                                    None => None,
                                },
                            }
                        } else {
                            match self.lookup_in_scope(module, *name, true) {
                                Some(d) => Some(d),
                                None => match self.extern_crate(module, *name) {
                                    Some(c) => Some(self.mods[self.crates[c as usize].root_mod as usize].def),
                                    None => None,
                                },
                            }
                        }
                    }
                }
            } else {
                let c = cur?;
                match kind {
                    SegKind::Super => {
                        let d = self.def(c);
                        if d.kind != DefKind::Mod {
                            return None;
                        }
                        let p = self.mods[d.sub as usize].parent_mod?;
                        Some(self.mods[p as usize].def)
                    }
                    SegKind::SelfValue => Some(c),
                    _ => self.lookup_in_container(c, *name, true),
                }
            };
            cur = next;
            cur?;
        }
        cur
    }

    /// Name in a container: module (items, imports, globs), enum (variants), trait (items).
    pub fn lookup_in_container(&self, c: DefId, name: Sym, ns_type: bool) -> Option<DefId> {
        let d = self.def(c);
        match d.kind {
            DefKind::Mod => {
                let r = self.lookup_in_module(d.sub, name, ns_type, &mut Vec::new());
                if r.is_none() { self.note_deferred_miss(d.sub, name); }
                r
            }
            DefKind::Enum => {
                for &v in &self.variants[d.sub as usize] {
                    if self.def(v).name == name {
                        return Some(v);
                    }
                }
                None
            }
            DefKind::Trait => {
                for &v in &self.traits[d.sub as usize] {
                    if self.def(v).name == name {
                        return Some(v);
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn lookup_macro_in_container(&self, c: DefId, name: Sym) -> bool {
        let d = self.def(c);
        if d.kind == DefKind::Mod {
            return self.mods[d.sub as usize].macros.contains_key(&name);
        }
        false
    }

    /// Items + imports of a module, then its globs.
    pub fn lookup_in_module(&self, m: u32, name: Sym, ns_type: bool, visiting: &mut Vec<u32>) -> Option<DefId> {
        let _ = visiting;
        self.lookup_in_module_vis(m, name, ns_type, &mut GlobQuery::default()).map(|x| x.0)
    }

    /// `name` in module `m` with its binding's visibility, through glob imports. A glob cycle
    /// is a fixpoint: a module still being looked through answers the previous pass's answer
    /// (None in the first pass) to the inner visit, and every such answer is recorded. When one
    /// of them differs from that module's final answer of the pass, the query runs again with
    /// the pass's answers as the previous ones (answers only grow: a found binding stays found,
    /// a wider one replaces a narrower); usually one pass suffices. Only the converged root
    /// answer is cached once imports are resolved.
    fn lookup_in_module_vis(&self, m: u32, name: Sym, ns_type: bool, q: &mut GlobQuery) -> Option<(DefId, BindVis)> {
        if !q.open.is_empty() {
            return self.lookup_in_module_pass(m, name, ns_type, q);
        }
        // a converged answer of an earlier query: returned as it is (inserting it again would put
        // a duplicate node at the head of its chain on every repeated lookup -- MemoMap::insert
        // does not replace -- and the chains of the fixed bucket table grew without bound)
        let cacheable = self.imports_done && !q.precise && !self.mods[m as usize].globs.is_empty()
            && !self.mods[m as usize].values.contains_key(&name) && !self.mods[m as usize].types.contains_key(&name);
        if cacheable {
            if let Some(r) = self.glob_cache.get(&(m, name, ns_type)) {
                return r;
            }
        }
        // first found wins between globs, so an answer can change once more after it is
        // found; the passes are bounded by the cycle (each pass settles at least one more
        // module of it) -- the bound only guards against a non-converging glob graph
        for pass in 0.. {
            q.memo.clear();
            q.hits.clear();
            let r = self.lookup_in_module_pass(m, name, ns_type, q);
            let stale = q.hits.iter().any(|(x, given)| q.memo.get(x).map_or(false, |fin| fin != given));
            if !stale || pass > q.prev.len() + 1 {
                if cacheable {
                    self.glob_cache.insert((m, name, ns_type), r);
                }
                return r;
            }
            q.prev = std::mem::take(&mut q.memo);
        }
        unreachable!()
    }

    fn lookup_in_module_pass(&self, m: u32, name: Sym, ns_type: bool, q: &mut GlobQuery) -> Option<(DefId, BindVis)> {
        let md = &self.mods[m as usize];
        let direct = if ns_type { md.types.get(&name) } else { md.values.get(&name) };
        if let Some(&d) = direct {
            if record_import_uses() && md.import_pub.contains_key(&(name, ns_type)) { self.import_used.lock().unwrap_or_else(|e| e.into_inner()).insert((m, name, ns_type)); }
            return Some((d, if q.precise { self.precise_vis(m, name, ns_type, d) } else { self.binding_vis(m, name, ns_type, d) }));
        }
        if md.globs.is_empty() {
            return None;
        }
        if q.open.contains(&m) {
            let given = q.prev.get(&m).copied().flatten();
            q.hits.push((m, given));
            return given;
        }
        if let Some(r) = q.memo.get(&m) {
            return *r;
        }
        // through globs: converged answers of earlier queries (glob graphs are wide and deep)
        if self.imports_done && !q.precise {
            if let Some(r) = self.glob_cache.get(&(m, name, ns_type)) {
                return r;
            }
        }
        q.open.push(m);
        let r = self.lookup_in_module_globs(m, name, ns_type, q);
        q.open.pop();
        q.memo.insert(m, r);
        r
    }

    /// Whether visibility `a` is wider than `b`.
    pub fn vis_wider(&self, a: BindVis, b: BindVis) -> bool {
        match (a, b) {
            (None, Some(_)) => true,
            (Some(x), Some(y)) => x != y && self.in_module(y, x),
            _ => false,
        }
    }

    fn lookup_in_module_globs(&self, m: u32, name: Sym, ns_type: bool, q: &mut GlobQuery) -> Option<(DefId, BindVis)> {
        let md = &self.mods[m as usize];
        // a glob imports only the bindings visible to the importing module, each with
        // min(glob visibility, binding visibility); several globs importing the same item keep
        // the most visible binding (rustc_resolve imports.rs / try_define): a module's private
        // `use` never leaks through `pub use module::*`
        let mut best: Option<(DefId, BindVis)> = None;
        for &(g, gpub) in &md.globs {
            let gd = self.def(g);
            let r = match gd.kind {
                DefKind::Mod => self.lookup_in_module_vis(gd.sub, name, ns_type, q),
                // a variant is in both namespaces (rustc define(.., TypeNS / ValueNS) in
                // build_reduced_graph_for_variant): `use Op::*` in a block shadows an outer
                // struct of the same name for struct literals and patterns too
                DefKind::Enum => self.lookup_in_container(g, name, false).map(|d| (d, None)),
                _ => None,
            };
            if let Some((d, v)) = r {
                if !self.vis_visible(v, m) {
                    continue;
                }
                let gv = if gpub { None } else { Some(md.normal) };
                let v = self.vis_min(gv, v);
                if record_import_uses() { self.glob_used.lock().unwrap_or_else(|e| e.into_inner()).insert((m, g)); }
                match best {
                    None => best = Some((d, v)),
                    Some((bd, bv)) => {
                        if bd == d && self.vis_wider(v, bv) {
                            best = Some((d, v));
                        }
                    }
                }
                if matches!(best, Some((_, None))) {
                    break;
                }
            }
        }
        best
    }

    /// Lexical lookup from a (possibly anonymous) module: block modules fall through to
    /// their parents; then the extern prelude, std prelude and primitive types.
    /// Traits whose methods a method call in `module` may use: the module's (and its lexical
    /// parents') items and imports, `as _` imports, glob imports (transitively) and the prelude.
    pub fn traits_in_scope(&self, module: u32) -> std::sync::Arc<crate::fxhash::HashSet<DefId>> {
        if let Some(x) = self.trait_scope.get(&module) {
            return x;
        }
        let mut set = crate::fxhash::HashSet::default();
        let mut visiting = Vec::new();
        let mut m = module;
        loop {
            self.module_traits(m, &mut set, &mut visiting);
            match self.mods[m as usize].lexical_parent {
                Some(p) => m = p,
                None => break,
            }
        }
        if let Some(pm) = self.edition_prelude(module) {
            self.module_traits(pm, &mut set, &mut visiting);
        }
        let a = std::sync::Arc::new(set);
        self.trait_scope.insert(module, a.clone());
        a
    }

    /// Traits module `m`'s scope gets from its own bindings, `as _` imports and globs, with
    /// each binding's visibility: through a glob only traits visible to the importing module
    /// count (a trait's methods are in scope only where its binding is, rustc
    /// traits_in_scope over the module's resolutions).
    fn module_traits(&self, m: u32, set: &mut crate::fxhash::HashSet<DefId>, visiting: &mut Vec<u32>) {
        let _ = visiting;
        for &(d, _) in self.module_trait_bindings(m, &mut TraitQuery::default()).iter() {
            set.insert(d);
        }
    }

    /// Traits module `m`'s scope binds (own bindings, `as _` imports, globs) with each
    /// binding's visibility; through a glob only traits visible to the importing module count
    /// (a trait's methods are in scope only where its binding is). `memo`: per-query results
    /// (in progress = empty, ending glob cycles).
    /// The traits module `m` binds (directly or through globs) with their visibility. A glob
    /// cycle is a fixpoint as in lookup_in_module_vis: a module still being walked gives the
    /// previous pass's set (empty in the first pass); a pass whose given sets differ from those
    /// modules' final sets runs again. Only the converged root set is cached.
    fn module_trait_bindings(&self, m: u32, q: &mut TraitQuery) -> std::sync::Arc<Vec<(DefId, BindVis)>> {
        if !q.open.is_empty() {
            return self.module_trait_bindings_pass(m, q);
        }
        if self.imports_done {
            if let Some(r) = self.trait_bind_cache.get(&m) {
                return r;
            }
        }
        for pass in 0.. {
            q.memo.clear();
            q.hits.clear();
            let r = self.module_trait_bindings_pass(m, q);
            let stale = q.hits.iter().any(|(x, given)| q.memo.get(x).map_or(false, |fin| **fin != **given));
            if !stale || pass > q.prev.len() + 1 {
                if self.imports_done {
                    self.trait_bind_cache.insert(m, r.clone());
                }
                return r;
            }
            q.prev = std::mem::take(&mut q.memo);
        }
        unreachable!()
    }

    fn module_trait_bindings_pass(&self, m: u32, q: &mut TraitQuery) -> std::sync::Arc<Vec<(DefId, BindVis)>> {
        if q.open.contains(&m) {
            let given = q.prev.get(&m).cloned().unwrap_or_default();
            q.hits.push((m, given.clone()));
            return given;
        }
        if let Some(r) = q.memo.get(&m) {
            return r.clone();
        }
        if self.imports_done {
            if let Some(r) = self.trait_bind_cache.get(&m) {
                return r;
            }
        }
        q.open.push(m);
        let md = &self.mods[m as usize];
        let mut out: Vec<(DefId, BindVis)> = Vec::new();
        let mut names: Vec<(&Sym, &DefId)> = md.types.iter().collect();
        names.sort_by_key(|x| *x.0);
        for (n, d) in names {
            if self.def(*d).kind == DefKind::Trait {
                out.push((*d, self.binding_vis(m, *n, true, *d)));
            }
        }
        for &(d, p) in &md.anon_traits {
            out.push((d, if p { None } else { Some(md.normal) }));
        }
        for &(g, gpub) in &md.globs {
            let gd = self.def(g);
            if gd.kind != DefKind::Mod {
                continue;
            }
            let gv = if gpub { None } else { Some(md.normal) };
            let inner = self.module_trait_bindings_pass(gd.sub, q);
            for &(d, v) in inner.iter() {
                // a glob-imported trait whose name an explicit binding of this module shadows
                // is not in scope (rustc: its methods are unavailable)
                if md.types.get(&self.def(d).name).is_some_and(|x| *x != d) { continue; }
                if self.vis_visible(v, m) {
                    let v = self.vis_min(gv, v);
                    // one entry per trait, the most visible binding
                    match out.iter().position(|x| x.0 == d) {
                        Some(i) => {
                            if self.vis_wider(v, out[i].1) {
                                out[i].1 = v;
                            }
                        }
                        None => out.push((d, v)),
                    }
                }
            }
        }
        q.open.pop();
        let r = std::sync::Arc::new(out);
        q.memo.insert(m, r.clone());
        r
    }

    pub fn lookup_in_scope(&self, module: u32, name: Sym, ns_type: bool) -> Option<DefId> {
        if !self.imports_done || !self.body_memos.load(std::sync::atomic::Ordering::Relaxed) {
            return self.lookup_in_scope_uncached(module, name, ns_type);
        }
        if let Some(r) = self.scope_cache.get(&(module, name, ns_type)) {
            if r.is_none() { self.note_scope_miss(module, name); }
            return r;
        }
        let r = self.lookup_in_scope_uncached(module, name, ns_type);
        self.scope_cache.insert((module, name, ns_type), r);
        r
    }

    /// A name a lookup did not find in module `m` may be in a module the Rust selection
    /// skipped: note which (the post-boot loader loads them if the build fails).
    fn note_deferred_miss(&self, m: u32, name: Sym) {
        if self.imports_done && !self.deferred_waits.is_empty() {
            // a deferred import of this very name: its stop modules, unconditionally ("")
            if let Some(stops) = self.deferred_binds.get(&(m, name)) { note_missing(stops, ""); }
            if let Some(waits) = self.deferred_waits.get(&m) { note_missing(waits, self.syms.str(name)); }
        }
    }

    fn note_scope_miss(&self, module: u32, name: Sym) {
        if !self.imports_done || self.deferred_waits.is_empty() { return; }
        let mut m = module;
        loop {
            self.note_deferred_miss(m, name);
            match self.mods[m as usize].lexical_parent {
                Some(p) => m = p,
                None => break,
            }
        }
    }

    /// `name` among the items of the body blocks of `owner` that `module` sits in (the anonymous
    /// modules from `module` up to where the owner's body starts): what rustc's innermost Item
    /// ribs hold, ahead of the owner's generic parameters.
    pub fn lookup_in_body_blocks(&self, module: u32, owner: DefId, name: Sym, ns_type: bool) -> Option<DefId> {
        let mut m = module;
        while self.block_owner.get(&m) == Some(&owner) {
            if let Some(d) = self.lookup_in_module(m, name, ns_type, &mut Vec::new()) {
                return Some(d);
            }
            m = self.mods[m as usize].lexical_parent?;
        }
        None
    }

    fn lookup_in_scope_uncached(&self, module: u32, name: Sym, ns_type: bool) -> Option<DefId> {
        let mut m = module;
        loop {
            if let Some(d) = self.lookup_in_module(m, name, ns_type, &mut Vec::new()) {
                return Some(d);
            }
            match self.mods[m as usize].lexical_parent {
                Some(p) => m = p,
                None => break,
            }
        }
        let r = self.lookup_in_scope_outside(module, name, ns_type);
        if r.is_none() { self.note_scope_miss(module, name); }
        r
    }

    /// The std prelude module (`prelude` of the prelude crate) and its per-edition modules
    /// (`rust_2015` .. `rust_2024`), found on the first lookup that sees them all.
    fn prelude_ids(&self) -> Option<&PreludeIds> {
        if let Some(p) = self.prelude_ids.get() { return Some(p); }
        let pc = self.prelude_crate?;
        let root = self.crates[pc as usize].root_mod;
        let pm = self.syms.get("prelude").and_then(|n| self.mods[root as usize].types.get(&n))?;
        let pmd = self.def(*pm);
        if pmd.kind != DefKind::Mod { return None; }
        let mut editions = [pmd.sub; 4];
        for (i, ed) in ["rust_2015", "rust_2018", "rust_2021", "rust_2024"].into_iter().enumerate() {
            if let Some(d) = self.syms.get(ed).and_then(|n| self.mods[pmd.sub as usize].types.get(&n)) {
                if self.def(*d).kind == DefKind::Mod { editions[i] = self.def(*d).sub; }
            }
        }
        Some(self.prelude_ids.get_or_init(|| PreludeIds { base: pmd.sub, editions }))
    }

    fn prelude_modules(&self) -> Vec<u32> {
        self.prelude_ids().map_or_else(Vec::new, |p| std::iter::once(p.base).chain(p.editions).collect())
    }

    /// The prelude the edition of `module`'s crate injects (rustc: `std::prelude::rust_20NN`);
    /// the plain `prelude` module for an edition the prelude crate has no module for.
    fn edition_prelude(&self, module: u32) -> Option<u32> {
        let p = self.prelude_ids()?;
        Some(match self.crates[self.mods[module as usize].krate as usize].edition {
            2015 => p.editions[0],
            2018 => p.editions[1],
            2021 => p.editions[2],
            2024 => p.editions[3],
            _ => p.base,
        })
    }

    /// The extern prelude, std prelude and primitive types (lookup_in_scope after the
    /// lexical modules).
    pub fn lookup_in_scope_outside(&self, module: u32, name: Sym, ns_type: bool) -> Option<DefId> {
        if ns_type {
            if let Some(c) = self.extern_crate(module, name) {
                return Some(self.mods[self.crates[c as usize].root_mod as usize].def);
            }
        }
        if let Some(pm) = self.edition_prelude(module) {
            if let Some(d) = self.lookup_in_module(pm, name, ns_type, &mut Vec::new()) {
                return Some(d);
            }
        }
        if ns_type {
            for i in 0..PRIMS.len() {
                if self.syms.get(PRIMS[i].0) == Some(name) {
                    return Some(self.prim_defs[i]);
                }
            }
        }
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    Fn,
    Struct,
    Union,
    Enum,
    Const,
    Static,
    TypeAlias,
    Trait,
    Impl,
    Mod,
    ForeignMod,
    Use,
    ExternCrate,
    MacroRules,
    Other,
}

fn item_tag(k: &ItemKind) -> Tag {
    match k {
        ItemKind::Fn(..) => Tag::Fn,
        ItemKind::Struct(..) => Tag::Struct,
        ItemKind::Union(..) => Tag::Union,
        ItemKind::Enum(..) => Tag::Enum,
        ItemKind::Const(..) => Tag::Const,
        ItemKind::Static(..) => Tag::Static,
        ItemKind::TypeAlias { .. } => Tag::TypeAlias,
        ItemKind::Trait { .. } => Tag::Trait,
        ItemKind::Impl { .. } => Tag::Impl,
        ItemKind::Mod(_) => Tag::Mod,
        ItemKind::ForeignMod(..) => Tag::ForeignMod,
        ItemKind::Use { .. } => Tag::Use,
        ItemKind::ExternCrate(_) => Tag::ExternCrate,
        ItemKind::MacroRules(_) => Tag::MacroRules,
        _ => Tag::Other,
    }
}

/// Blocks inside a type: array lengths and const generic arguments (`[u8; { const N: usize = 1; N }]`,
/// `S<{ .. }>`), which rustc's resolver visits like any expression.
pub fn blocks_in_ty(ast: &Ast, t: TyId, out: &mut Vec<BlockId>) {
    match ast.ty(t) {
        Ty::Path(p) => blocks_in_path(ast, p, out),
        Ty::Ref(_, _, x) | Ty::Ptr(_, x) | Ty::Slice(x) | Ty::Paren(x) => blocks_in_ty(ast, *x, out),
        Ty::Array(x, len) => {
            blocks_in_ty(ast, *x, out);
            blocks_in_expr(ast, *len, out);
        }
        Ty::Tuple(v) => {
            for x in ast.list(*v) { blocks_in_ty(ast, *x, out); }
        }
        Ty::Fn(f) => {
            let f = ast.fn_ptr(*f);
            for x in ast.list(f.params) { blocks_in_ty(ast, *x, out); }
            if let Some(r) = f.ret { blocks_in_ty(ast, r, out); }
        }
        Ty::ImplTrait(b) | Ty::DynTrait(b, _) => blocks_in_bounds(ast, *b, out),
        Ty::Never | Ty::Infer | Ty::Mac(_) | Ty::ImplicitSelf | Ty::CVarArgs => {}
    }
}

pub fn blocks_in_bounds(ast: &Ast, bounds: List<Bound>, out: &mut Vec<BlockId>) {
    for b in ast.list(bounds) {
        match b {
            Bound::Trait { path, .. } => blocks_in_path(ast, path, out),
            Bound::Lifetime(_) | Bound::Use(_) => {}
        }
    }
}

pub fn blocks_in_path(ast: &Ast, p: &Path, out: &mut Vec<BlockId>) {
    if let Some(q) = ast.qself(p) {
        blocks_in_ty(ast, q.ty, out);
        if let Some(tp) = &q.trait_path { blocks_in_path(ast, tp, out); }
    }
    for seg in ast.segs(p.segs) {
        blocks_in_gargs(ast, seg.args(), out);
    }
}

pub fn blocks_in_gargs(ast: &Ast, id: GenericArgsId, out: &mut Vec<BlockId>) {
    match ast.gargs(id) {
        Some(GenericArgs::Angle(v)) => {
            for a in ast.list(*v) {
                match a {
                    GenericArg::Type(t) => blocks_in_ty(ast, *t, out),
                    GenericArg::Const(e) => blocks_in_expr(ast, *e, out),
                    GenericArg::Binding(_, ga, t) => {
                        blocks_in_gargs(ast, *ga, out);
                        blocks_in_ty(ast, *t, out);
                    }
                    GenericArg::Constraint(_, ga, b) => {
                        blocks_in_gargs(ast, *ga, out);
                        blocks_in_bounds(ast, *b, out);
                    }
                    GenericArg::Lifetime(_) => {}
                }
            }
        }
        Some(GenericArgs::Paren(ps, r)) => {
            for x in ast.list(*ps) { blocks_in_ty(ast, *x, out); }
            if let Some(r) = r { blocks_in_ty(ast, *r, out); }
        }
        None => {}
    }
}

fn blocks_in_generics(ast: &Ast, g: &Generics, out: &mut Vec<BlockId>) {
    for p in ast.list(g.params) {
        match p.kind {
            GenericParamKind::Lifetime(_) => {}
            GenericParamKind::Type(b, d) => {
                blocks_in_bounds(ast, b, out);
                if let Some(d) = d { blocks_in_ty(ast, d, out); }
            }
            GenericParamKind::Const(t, d) => {
                blocks_in_ty(ast, t, out);
                if let Some(d) = d { blocks_in_expr(ast, d, out); }
            }
        }
    }
    for w in ast.list(g.where_) {
        if let WherePred::Bound { ty, bounds, .. } = w {
            blocks_in_ty(ast, *ty, out);
            blocks_in_bounds(ast, *bounds, out);
        }
    }
}

fn blocks_in_fields(ast: &Ast, fields: List<FieldDef>, out: &mut Vec<BlockId>) {
    for f in ast.list(fields) {
        blocks_in_ty(ast, f.ty, out);
        if let Some(d) = f.default { blocks_in_expr(ast, d, out); }
    }
}

fn blocks_in_variant_data(ast: &Ast, d: &VariantData, out: &mut Vec<BlockId>) {
    match d {
        VariantData::Unit => {}
        VariantData::Tuple(f) | VariantData::Struct(f) => blocks_in_fields(ast, *f, out),
    }
}

/// Blocks in an item's signature positions (not its body): parameter and return types, generics,
/// fields, enum discriminants, aliased types, impl headers, constant types.
pub fn blocks_in_item_signature(ast: &Ast, kind: &ItemKind, out: &mut Vec<BlockId>) {
    match kind {
        ItemKind::Fn(s, _, _) => {
            let sig = ast.fn_sig(*s);
            blocks_in_generics(ast, &sig.generics, out);
            for p in ast.list(sig.params) { blocks_in_ty(ast, p.ty, out); }
            if let Some(r) = sig.ret { blocks_in_ty(ast, r, out); }
        }
        ItemKind::Struct(_) => {
            let Some(it) = ast.as_struct(kind) else { return };
            blocks_in_generics(ast, &it.generics, out);
            blocks_in_variant_data(ast, &it.data, out);
        }
        ItemKind::Union(_) => {
            let Some(it) = ast.as_union(kind) else { return };
            blocks_in_generics(ast, &it.generics, out);
            blocks_in_fields(ast, it.fields, out);
        }
        ItemKind::Enum(_) => {
            let Some(it) = ast.as_enum(kind) else { return };
            blocks_in_generics(ast, &it.generics, out);
            for v in ast.list(it.variants) {
                blocks_in_variant_data(ast, &v.data, out);
                if let Some(d) = v.disc { blocks_in_expr(ast, d, out); }
            }
        }
        ItemKind::TypeAlias(_) => {
            let Some(it) = ast.as_alias(kind) else { return };
            blocks_in_generics(ast, &it.generics, out);
            blocks_in_bounds(ast, it.bounds, out);
            if let Some(t) = it.ty { blocks_in_ty(ast, t, out); }
        }
        ItemKind::Const(t, _) => {
            if let Some(t) = t.get() { blocks_in_ty(ast, t, out); }
        }
        ItemKind::Static(_, t, _) => blocks_in_ty(ast, *t, out),
        ItemKind::Impl(_) => {
            let Some(it) = ast.as_impl(kind) else { return };
            blocks_in_generics(ast, &it.generics, out);
            if let Some(tp) = &it.trait_ { blocks_in_path(ast, tp, out); }
            blocks_in_ty(ast, it.self_ty, out);
        }
        ItemKind::Trait(_) => {
            let Some(it) = ast.as_trait(kind) else { return };
            blocks_in_generics(ast, &it.generics, out);
            blocks_in_bounds(ast, it.supers, out);
        }
        _ => {}
    }
}

/// Blocks directly reachable from an expression (not through nested blocks' statements).
pub fn blocks_in_expr(ast: &Ast, e: ExprId, out: &mut Vec<BlockId>) {
    match &ast.expr(e).kind {
        ExprKind::Block(b, _) | ExprKind::Unsafe(b) | ExprKind::Async(_, b) | ExprKind::ConstBlock(b) | ExprKind::Loop(b, _) => {
            out.push(*b)
        }
        ExprKind::If(c, t, el) => {
            blocks_in_expr(ast, *c, out);
            out.push(*t);
            if let Some(x) = el.get() {
                blocks_in_expr(ast, x, out);
            }
        }
        ExprKind::While(row) => {
            let WhileRow { cond, body, .. } = ast.row(*row);
            blocks_in_expr(ast, *cond, out);
            out.push(*body);
        }
        ExprKind::For(row) => {
            let ForRow { iter, body, .. } = ast.row(*row);
            blocks_in_expr(ast, *iter, out);
            out.push(*body);
        }
        ExprKind::Match(x, arms) => {
            blocks_in_expr(ast, *x, out);
            for a in ast.list(*arms) {
                blocks_in_expr(ast, a.body, out);
            }
        }
        ExprKind::Closure(row) => {
            let ClosureRow { params, ret, body, .. } = ast.row(*row);
            for p in ast.list(*params) {
                if let Some(t) = p.ty { blocks_in_ty(ast, t, out); }
            }
            if let Some(r) = ret { blocks_in_ty(ast, *r, out); }
            blocks_in_expr(ast, *body, out)
        }
        ExprKind::Call(f, args) => {
            blocks_in_expr(ast, *f, out);
            for a in ast.list(*args) {
                blocks_in_expr(ast, *a, out);
            }
        }
        ExprKind::MethodCall(recv, row) => {
            blocks_in_expr(ast, *recv, out);
            blocks_in_gargs(ast, ast.row(*row).turbofish, out);
            for a in ast.list(ast.row(*row).args) {
                blocks_in_expr(ast, *a, out);
            }
        }
        ExprKind::Cast(x, t) => {
            blocks_in_expr(ast, *x, out);
            blocks_in_ty(ast, *t, out);
        }
        ExprKind::Path(p) => blocks_in_path(ast, p, out),
        ExprKind::Paren(x) | ExprKind::Unary(_, x) | ExprKind::AddrOf(_, _, x) | ExprKind::Try(x) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
            blocks_in_expr(ast, *x, out)
        }
        // rustc_ast/src/visit.rs walk_expr: repeat counts contain lexical blocks too.
        ExprKind::Repeat(a, b) | ExprKind::Binary(_, a, b) | ExprKind::Assign(a, b) | ExprKind::AssignOp(_, a, b) | ExprKind::Index(a, b) => {
            blocks_in_expr(ast, *a, out);
            blocks_in_expr(ast, *b, out);
        }
        ExprKind::Return(x) | ExprKind::Break(_, x) => {
            if let Some(x) = x.get() {
                blocks_in_expr(ast, x, out)
            }
        }
        ExprKind::Tuple(v) | ExprKind::Array(v) => {
            for x in ast.list(*v) {
                blocks_in_expr(ast, *x, out);
            }
        }
        ExprKind::Struct(row) => {
            let StructRow { path, fields, base, .. } = ast.row(*row);
            blocks_in_path(ast, path, out);
            for f in ast.list(*fields) {
                blocks_in_expr(ast, f.expr, out);
            }
            if let Some(b) = base.get() {
                blocks_in_expr(ast, b, out);
            }
        }
        _ => {}
    }
}

pub fn dir_of(path: &str) -> &str {
    // std::path::is_separator: '/' (unix), '/' or '\\' (Windows)
    match path.rfind(std::path::is_separator) {
        Some(i) => &path[..i],
        None => ".",
    }
}

pub fn stem_of(path: &str) -> &str {
    let f = match path.rfind(std::path::is_separator) {
        Some(i) => &path[i + 1..],
        None => path,
    };
    match f.rfind('.') {
        Some(i) => &f[..i],
        None => f,
    }
}

/// Removes fields and enum variants whose `#[cfg(..)]` is false, as rustc's cfg-strip does before
/// anything else sees the syntax tree: struct / union / enum-variant field lists, enum variant
/// lists, struct literals and struct patterns.
/// (Layouts, field indices, derives and literal checks then all see the target's real fields.)
pub fn strip_cfg_fields(pf: &mut ParsedFile, cfg: &CfgSet) {
    let view = crate::cfg::PackedSrc { src: &pf.src, toks: &pf.toks };
    let src = Src { src: &pf.src, plain: &[], ext: &[], parts: Some(&view) };
    let ast = &mut pf.ast;
    let kinds: Vec<ItemKind> = ast.items.iter().map(|it| it.kind).collect();
    // an item row's generics, their parameter list stripped
    macro_rules! strip_generics {
        ($r:expr) => {{
            let mut g = ast.row($r).generics;
            strip_attrs_list(src, cfg, ast.generic_params.vec_mut(), ast.base[19], (&ast.attrs, ast.base[21]), &mut g.params, |p| p.attrs);
            ast.row_mut($r).generics = g;
        }};
    }
    for kind in kinds {
        match kind {
            ItemKind::Struct(r) => {
                let mut data = ast.row(r).data;
                if let VariantData::Struct(f) | VariantData::Tuple(f) = &mut data {
                    strip_attrs_list(src, cfg, ast.field_defs.vec_mut(), ast.base[26], (&ast.attrs, ast.base[21]), f, |f| f.attrs);
                }
                ast.row_mut(r).data = data;
                strip_generics!(r);
            }
            ItemKind::Union(r) => {
                let mut f = ast.row(r).fields;
                strip_attrs_list(src, cfg, ast.field_defs.vec_mut(), ast.base[26], (&ast.attrs, ast.base[21]), &mut f, |f| f.attrs);
                ast.row_mut(r).fields = f;
                strip_generics!(r);
            }
            ItemKind::Enum(r) => {
                let mut vs = ast.row(r).variants;
                strip_attrs_list(src, cfg, ast.variants.vec_mut(), ast.base[27], (&ast.attrs, ast.base[21]), &mut vs, |v| v.attrs);
                ast.row_mut(r).variants = vs;
                let lo = vs.lo.wrapping_sub(ast.base[27]) as usize;
                for v in &mut ast.variants[lo..lo + vs.len()] {
                    if let VariantData::Struct(f) | VariantData::Tuple(f) = &mut v.data {
                        strip_attrs_list(src, cfg, ast.field_defs.vec_mut(), ast.base[26], (&ast.attrs, ast.base[21]), f, |f| f.attrs);
                    }
                }
                strip_generics!(r);
            }
            ItemKind::Fn(id, _, _) => {
                let sig = &mut ast.fn_sigs[(id.0 - ast.base[24]) as usize];
                strip_attrs_list(src, cfg, ast.params.vec_mut(), ast.base[25], (&ast.attrs, ast.base[21]), &mut sig.params, |p| p.attrs);
                let g = &mut sig.generics;
                strip_attrs_list(src, cfg, ast.generic_params.vec_mut(), ast.base[19], (&ast.attrs, ast.base[21]), &mut g.params, |p| p.attrs);
            }
            ItemKind::TraitAlias(r) => strip_generics!(r),
            ItemKind::TypeAlias(r) => strip_generics!(r),
            ItemKind::Trait(r) => strip_generics!(r),
            ItemKind::Impl(r) => strip_generics!(r),
            _ => {}
        }
    }
    let ParsedFile { src, toks, ast, .. } = pf;
    let view = crate::cfg::PackedSrc { src: &src[..], toks };
    strip_cfg_nodes(ast, Src { src: &src[..], plain: &[], ext: &[], parts: Some(&view) }, cfg);
}

/// cfg-strips the attribute-bearing parts of expressions and patterns (match arms, closure
/// params, struct literal fields, struct pattern fields) of a tree (a file's, or its lazily
/// parsed bodies').
pub fn strip_cfg_nodes(ast: &mut Ast, src: Src, cfg: &CfgSet) {
    for x in ast.exprs.iter_mut() {
        // match arms and closure params (flat_map_arm / flat_map_param)
        match &mut x.kind {
            ExprKind::Match(_, arms) => strip_attrs_list(src, cfg, ast.arms.vec_mut(), ast.base[11], (&ast.attrs, ast.base[21]), arms, |a| a.attrs),
            _ => {}
        }
    }
    for row in ast.closure_exprs.iter_mut() {
        strip_attrs_list(src, cfg, ast.closure_params.vec_mut(), ast.base[12], (&ast.attrs, ast.base[21]), &mut row.params, |p| p.attrs);
    }
    for row in ast.struct_exprs.iter_mut() {
        strip_attrs_list(src, cfg, ast.field_exprs.vec_mut(), ast.base[10], (&ast.attrs, ast.base[21]), &mut row.fields, |f| f.attrs);
    }
    for x in ast.pats.iter_mut() {
        if let Pat::Struct(_, fields, _) = x {
            strip_attrs_list(src, cfg, ast.field_pats.vec_mut(), ast.base[13], (&ast.attrs, ast.base[21]), fields, |f| f.attrs);
        }
    }
}

/// Drops the elements whose `#[cfg(..)]` attributes are false (a list of a tree being built:
/// `arr` from `base`, its attributes in `attr_arr`, the tree's attribute array, from its base).
fn strip_attrs_list<X>(src: Src, cfg: &CfgSet, arr: &mut Vec<X>, base: u32, attr_arr: (&[Attr], u32), l: &mut List<X>, attrs: impl Fn(&X) -> List<Attr>) {
    let lo = l.lo.wrapping_sub(base) as usize;
    let Some(v) = arr.get_mut(lo..lo + l.len as usize) else { return };
    if v.iter().all(|x| attrs(x).is_empty()) {
        return;
    }
    let keep: Vec<bool> = v.iter().map(|x| {
        let a = attrs(x);
        if a.is_empty() {
            return true;
        }
        let i = a.lo.wrapping_sub(attr_arr.1) as usize;
        crate::cfg::active(src, &attr_arr.0[i..i + a.len()], cfg)
    }).collect();
    let mut w = 0;
    for r in 0..keep.len() {
        if keep[r] {
            v.swap(w, r);
            w += 1;
        }
    }
    l.len = w as u32;
}

/// Per body block holding items (directly or in a block below it): (items, the blocks below it
/// that hold items), keyed by block id.
pub type BodyBlocks = HashMap<u32, (Vec<ItemId>, Vec<BlockId>)>;

/// The program's parse of a core/alloc/std file, shared with the proc-macro world (pmhost):
/// the world's own std copy was its biggest part (~33 MB of ~57 MB on counter, x64). The world
/// takes a file only with the same edition, cfg and module path and when the file's macro
/// lookups replay against its own table (as a preloaded parse must, Program::load_file).
pub struct SharedParse {
    pub pf: std::sync::Arc<ParsedFile>,
    pub used: crate::macros::MacroUse,
    pub idents: Vec<(u32, u32)>,
    pub blocks: BodyBlocks,
    pub lines: usize,
    pub edition: u16,
    pub cfg: Vec<String>,
    pub mod_path: String,
}

/// The program's std parses by path, while its load runs (`share_std_begin` .. `share_std_end`)
/// and after it (a world built again for hot reload takes them too).
/// `makepad check` records which imports its name resolutions go through (the lint
/// `unused_imports`); a run does not.
static RECORD_IMPORT_USES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn record_import_uses() -> bool { RECORD_IMPORT_USES.load(std::sync::atomic::Ordering::Relaxed) }
pub fn set_record_import_uses(on: bool) { RECORD_IMPORT_USES.store(on, std::sync::atomic::Ordering::Relaxed); }

static SHARED_STD: relax_session::sync::Mutex<Option<HashMap<String, std::sync::Arc<SharedParse>>>> = relax_session::sync::Mutex::new(None);

pub(crate) struct ZoneTaken(Option<HashMap<String, std::sync::Arc<SharedParse>>>);

pub(crate) fn zone_take() -> ZoneTaken {
    let mut shared = SHARED_STD.lock().unwrap_or_else(|e| e.into_inner());
    let taken = ZoneTaken(std::mem::take(&mut *shared));
    #[cfg(test)]
    assert!(shared.is_none());
    taken
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    let mut shared = SHARED_STD.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *shared, taken.0));
}
static SHARED_CV: relax_session::sync::Condvar = relax_session::sync::Condvar::new();
/// the program's std load is running: the world waits for its files
static SHARING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The program's load starts: its core/alloc/std parses are offered to the proc-macro world.
pub fn share_std_begin() {
    SHARED_STD.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::default);
    SHARING.store(true, std::sync::atomic::Ordering::Release);
}

/// Threads blocked until the proc-macro world is built (pmhost::world). While one waits, the
/// world must not wait for the program's std parses: the waiter may be the very parse worker
/// (a prescan's bang expansion) or loader those parses wait on. An old tree whose macro crates
/// differ from the loader's build (no native macros: every bang is JIT-hosted) stalled 10 s per
/// std file that way, with no CPU and no output.
static WORLD_WAITERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Held by a thread waiting for the proc-macro world: the world parses its std files itself.
pub struct WorldWait(());
impl WorldWait {
    pub fn begin() -> WorldWait {
        let _g = SHARED_STD.lock().unwrap_or_else(|e| e.into_inner());
        WORLD_WAITERS.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        SHARED_CV.notify_all();
        WorldWait(())
    }
}
impl Drop for WorldWait {
    fn drop(&mut self) {
        WORLD_WAITERS.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// The program's std crates are loaded: a world still waiting for a file the program did not
/// load parses it itself.
pub fn share_std_end() {
    let _g = SHARED_STD.lock().unwrap_or_else(|e| e.into_inner());
    SHARING.store(false, std::sync::atomic::Ordering::Release);
    SHARED_CV.notify_all();
}

fn shared_std_put(path: &str, sp: SharedParse) {
    let mut g = SHARED_STD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = g.as_mut() {
        m.insert(path.to_string(), std::sync::Arc::new(sp));
    }
    SHARED_CV.notify_all();
}

/// The program's parse of `path`, waiting for it while the program's std load runs.
fn shared_std_wait(path: &str) -> Option<std::sync::Arc<SharedParse>> {
    let mut g = SHARED_STD.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let m = g.as_ref()?;
        if let Some(sp) = m.get(path) {
            return Some(sp.clone());
        }
        if !SHARING.load(std::sync::atomic::Ordering::Acquire) || WORLD_WAITERS.load(std::sync::atomic::Ordering::Acquire) != 0 {
            return None;
        }
        // a program that registered a graph but loads no std (never expected): parse alone
        let (ng, to) = SHARED_CV.wait_timeout(g, std::time::Duration::from_secs(10)).unwrap_or_else(|e| e.into_inner());
        g = ng;
        if to.timed_out() && g.as_ref().map_or(true, |m| !m.contains_key(path)) {
            // never silent: a stall here is a wait cycle between the world and the load
            eprintln!("makepad: the proc-macro world waited 10 s for the program's parse of {}; parsing it itself", path);
            return None;
        }
    }
}

/// Owned parse inputs. No Program/Interner borrow survives the unit lock. MacroTable is
/// an immutable Arc snapshot (writers use Arc::make_mut); its scope memo is a OnceLock.
/// FileCtx and MacroScope are created below, exclusively on the parsing thread.
pub struct ParseSnapshot {
    pub old: u32,
    pub path: String,
    pub edition: u16,
    pub kname: String,
    pub mod_path: String,
    pub cfg: CfgSet,
    pub macros: std::sync::Arc<crate::macros::MacroTable>,
    pub reuse: std::sync::Arc<crate::parse_work::Reuse>,
    pub previous: Option<std::sync::Arc<ParsedFile>>,
    pub context: std::sync::Arc<crate::source_context::Context>,
}
impl ParseSnapshot {
    pub fn parse(&self, src: Vec<u8>) -> Result<FileVersion, String> {
        // classify may ask for old lazy bodies. Complete their OnceLock off the unit lock
        // too; concurrent lazy compilers use this same synchronized immutable result.
        // Identifier interning remains Program::part_idents' job under the unit lock.
        if let Some(old) = &self.previous {
            let _timing = crate::parse_work::time(1);
            old.part(&self.cfg);
        }
        let verify = std::env::var("MAKEPAD_RUN_PARSE_VERIFY").as_deref() == Ok("1");
        let full_src = verify.then(|| src.clone());
        let session = crate::parse_work::Session::new(self.reuse.clone());
        let mut v = self.parse_full(src)?;
        v.reuse = session.finish();
        if let Some(src) = full_src {
            let full = self.parse_full(src)?;
            // Exact arenas, source, tokens, expansion/hygiene maps and prepared indexes:
            // position mistakes must not be hidden by a normalized text comparison.
            assert_eq!(format!("{:?}", v.pf.ast), format!("{:?}", full.pf.ast), "parse reuse AST mismatch: {}", self.path);
            assert_eq!(&*v.pf.src, &*full.pf.src);
            assert_eq!(v.pf.toks.iter().map(|t| (t.kind, t.lo, t.hi)).collect::<Vec<_>>(), full.pf.toks.iter().map(|t| (t.kind, t.lo, t.hi)).collect::<Vec<_>>());
            assert_eq!(v.pf.exp_map, full.pf.exp_map);
            assert_eq!(v.pf.tok_origin, full.pf.tok_origin);
            assert_eq!(v.pf.token_origins, full.pf.token_origins);
            assert_eq!(v.pf.rules_exp, full.pf.rules_exp);
            assert_eq!(v.pf.body_origin, full.pf.body_origin);
            assert_eq!(v.idents, full.idents);
            let defs = |v: &FileVersion| v.macros.iter().map(|d| format!("{:?}:{:?}:{}:{}:{:?}:{}:{:?}", d.name, d.krate, d.exported, d.builtin, d.def_file, d.def_pos, d.arms)).collect::<Vec<_>>();
            assert_eq!(defs(&v), defs(&full));
            assert_eq!(v.blocks, full.blocks);
        }
        Ok(v)
    }

    /// A file whose items are `stream` (tokens built by the caller, a proc-macro-like
    /// expansion), with `src` as the file's own text (the tokens' file-0 spans point into it:
    /// diagnostics name it).
    pub fn parse_tokens(&self, src: Vec<u8>, stream: crate::pm_tokens::TokenStream) -> Result<FileVersion, String> {
        let Self { old, path, edition, kname, mod_path, cfg, macros, .. } = self;
        let bctx = std::rc::Rc::new(crate::builtins::FileCtx::new(kname, path, mod_path, cfg));
        let context = self.context.edited(path, src.into());
        let (mut pf, used) = crate::parser::parse_token_stream(stream, path, *edition, Some(crate::macros::MacroScope::new(macros.clone())), bctx, context.sources().clone(), false)
            .map_err(|(span, m)| format!("{}:{}: {}", path, span.lo, m))?;
        let prep = prepare_file(&mut pf, cfg);
        let local = used.map(|u| u.local).unwrap_or_default();
        Ok(FileVersion { old: *old, path: path.clone(), pf, blocks: prep.blocks, idents: prep.idents, macros: local, reuse: Default::default(), context })
    }

    pub fn parse_full(&self, src: Vec<u8>) -> Result<FileVersion, String> {
        let Self { old, path, edition, kname, mod_path, cfg, macros, .. } = self;
        let bctx = std::rc::Rc::new(crate::builtins::FileCtx::new(kname, path, mod_path, cfg));
        let context = if self.context.sources().find(path).and_then(|id| self.context.sources().get(id).ok()).is_some_and(|old| old.src.as_ref() == src) { self.context.clone() } else { self.context.edited(path, src.clone().into()) };
        let (mut pf, used) = context.parse(src, None, *edition,
            Some(crate::macros::MacroScope::new(macros.clone())), bctx, false)?;
        let used = used.unwrap();
        let prep = prepare_file(&mut pf, cfg);
        Ok(FileVersion { old: *old, path: path.clone(), pf, blocks: prep.blocks, idents: prep.idents, macros: used.local, reuse: Default::default(), context })
    }
}

/// A parsed next version of a loaded file (hot reload: Program::parse_version / commit_version).
pub struct FileVersion {
    pub old: u32,
    pub path: String,
    pub pf: ParsedFile,
    blocks: BodyBlocks,
    idents: Vec<(u32, u32)>,
    macros: Vec<crate::macros::MacroRef>,
    pub reuse: std::sync::Arc<crate::parse_work::Reuse>,
    pub context: std::sync::Arc<crate::source_context::Context>,
}

/// What the loader needs from a parsed file besides its tree, computed right after the parse
/// (on the parse worker for files parsed ahead): the tree is cfg-stripped (fields, body
/// statements), `idents` are the distinct identifier / lifetime names in first-occurrence order
/// (interning them in that order assigns the same symbols as interning every token), `blocks`
/// the body blocks item collection visits.
pub struct FilePrep {
    pub idents: Vec<(u32, u32)>,
    pub blocks: BodyBlocks,
    /// newlines in the file and its expansion text (the boot report's line count)
    pub lines: usize,
}

pub fn prepare_file(pf: &mut ParsedFile, cfg: &CfgSet) -> FilePrep {
    let _profile = crate::load_profile::span(cfg.session.runtime.profile, "prepare-file", "cfg / identifiers / bodies");
    let _timing = crate::parse_work::time(5);
    strip_cfg_fields(pf, cfg);
    let idents = ident_ranges(pf);
    let blocks = body_blocks(pf, cfg);
    let lines = pf.src.iter().filter(|&&b| b == b'\n').count();
    FilePrep { idents, blocks, lines }
}

/// Byte ranges of the distinct identifier names (raw identifiers without `r#`, lifetimes without
/// the quote), in first-occurrence order.
fn ident_ranges(pf: &ParsedFile) -> Vec<(u32, u32)> {
    let mut seen: crate::fxhash::HashSet<&[u8]> = Default::default();
    let mut out = Vec::new();
    for t in pf.toks.iter() {
        let (lo, hi) = match t.kind {
            crate::lexer::T::Lifetime => (t.lo + 1, t.hi),
            crate::lexer::T::Ident | crate::lexer::T::RawIdent => {
                let b = pf.bytes(t.lo, t.hi);
                if b.starts_with(b"r#") {
                    (t.lo + 2, t.hi)
                } else {
                    (t.lo, t.hi)
                }
            }
            _ => continue,
        };
        if seen.insert(pf.bytes(lo, hi)) {
            out.push((lo, hi));
        }
    }
    out
}

/// cfg-strips every body block (like rustc before type checking: inactive `let` / expression
/// statements are removed, active ones lose the wrapper) and indexes the blocks that hold items.
fn body_blocks(pf: &mut ParsedFile, cfg: &CfgSet) -> BodyBlocks {
    let mut out = BodyBlocks::default();
    let mut roots = Vec::new();
    let testing = cfg.set.iter().any(|x| x == "test");
    for it in &pf.ast.items {
        match &it.kind {
            // a #[test] fn outside a test build is not compiled: the items and imports in its
            // body do not exist (rustc removes it before resolution)
            ItemKind::Fn(..) if !testing && crate::cfg::is_test(pf.src_view(), pf.ast.list(it.attrs)) => {}
            ItemKind::Fn(_, Some(b), _) => {
                blocks_in_item_signature(&pf.ast, &it.kind, &mut roots);
                roots.push(*b)
            }
            ItemKind::Const(_, e) | ItemKind::Static(_, _, e) => {
                blocks_in_item_signature(&pf.ast, &it.kind, &mut roots);
                if let Some(e) = e.get() { blocks_in_expr(&pf.ast, e, &mut roots) }
            }
            _ => blocks_in_item_signature(&pf.ast, &it.kind, &mut roots),
        }
    }
    let ParsedFile { src, toks, ast, .. } = pf;
    let view = crate::cfg::PackedSrc { src: &src[..], toks };
    let src = Src { src: &src[..], plain: &[], ext: &[], parts: Some(&view) };
    for b in roots {
        walk_body_block(ast, src, cfg, b, &mut out);
    }
    out
}

/// cfg-strips a lazily parsed fn body (ParsedFile::part; it holds no items).
pub fn strip_body_block(ast: &mut Ast, src: Src, cfg: &CfgSet, b: BlockId) {
    let mut out = BodyBlocks::default();
    walk_body_block(ast, src, cfg, b, &mut out);
}

fn walk_body_block(ast_m: &mut Ast, src: Src, cfg: &CfgSet, b: BlockId, out: &mut BodyBlocks) -> bool {
    strip_cfg_stmts(ast_m, src, cfg, b);
    let mut items = Vec::new();
    let mut sub_blocks = Vec::new();
    {
        let ast = &*ast_m;
        for s in ast.stmts(b) {
            match s {
                Stmt::Item(i) => items.push(*i),
                Stmt::Expr(e, _) => blocks_in_expr(ast, *e, &mut sub_blocks),
                Stmt::Let(r) => {
                    let LetStmt { init, else_, ty, .. } = ast.row(*r);
                    if let Some(t) = ty {
                        blocks_in_ty(ast, *t, &mut sub_blocks);
                    }
                    if let Some(e) = init {
                        blocks_in_expr(ast, *e, &mut sub_blocks);
                    }
                    if let Some(eb) = else_ {
                        sub_blocks.push(*eb);
                    }
                }
                Stmt::Empty | Stmt::Attrs(..) => {}
            }
        }
    }
    let mut kept = Vec::new();
    for sb in sub_blocks {
        if walk_body_block(ast_m, src, cfg, sb, out) {
            kept.push(sb);
        }
    }
    if items.is_empty() && kept.is_empty() {
        return false;
    }
    out.insert(b.0, (items, kept));
    true
}

/// cfg-strips the statements of a fn-body block.
fn strip_cfg_stmts(ast: &mut Ast, src: Src, cfg: &CfgSet, b: BlockId) {
    let blk = ast.block(b);
    let needs = ast.list(blk.stmts).iter().any(|s| matches!(s, Stmt::Attrs(..)) || matches!(s, Stmt::Let(r) if !ast.row(*r).attrs.is_empty()));
    if !needs {
        return;
    }
    let mut out = Vec::with_capacity(blk.stmts.len());
    let mut kept_attrs = Vec::new();
    for s in ast.list(blk.stmts) {
        match s {
            Stmt::Attrs(r) => {
                let AttrsStmt { attrs, expr, semi } = *ast.row(*r);
                if crate::cfg::active(src, ast.list(attrs), cfg) {
                    out.push(Stmt::Expr(expr, semi));
                    // the statement's attributes are kept for the lint passes (lint levels, and the
                    // fact that a value had attributes: `#[cfg(..)] { x }` is not an unused-braces case)
                    kept_attrs.push((expr, attrs));
                }
            }
            Stmt::Let(r) if { let attrs = ast.row(*r).attrs; !attrs.is_empty() && !crate::cfg::active(src, ast.list(attrs), cfg) } => {}
            s => out.push(*s),
        }
    }
    let l = ast.push_list(out);
    ast.block_mut(b).stmts = l;
    ast.stmt_attrs.extend(kept_attrs);
}

/// Closures in an owner's expression ranges, including its lazily materialized body.
/// Other bodies appended to the file arena never contribute to this ordinal.
pub fn closure_exprs(prog: &Program, owner: DefId) -> Vec<u32> {
    let def = prog.def(owner);
    let file = &prog.files[def.file as usize];
    prog.item_expr_ranges(def.file, def.item).into_iter().flat_map(|(lo, hi)| lo..hi)
        .filter(|&i| matches!(file.ast.expr(crate::ast::ExprId(i)).kind, crate::ast::ExprKind::Closure { .. } | crate::ast::ExprKind::Async(..))).collect()
}

/// An owner-local closure ordinal; anonymous owners use a file-level fallback.
pub fn stable_closure_name(prog: &Program, file: u32, e: u32, owner: Option<DefId>) -> String {
    let Some(f) = prog.files.get(file as usize) else { return format!("{{closure@{}}}", e) };
    let path = owner.map(|d| hint_path(&prog.defs, &prog.syms, &prog.files, d)).unwrap_or_default();
    let n = owner.filter(|d| prog.def(*d).file == file)
        .and_then(|d| closure_exprs(prog, d).iter().position(|&x| x == e))
        .unwrap_or_else(|| (0..e).filter(|&i| matches!(f.ast.exprs.get(i as usize).map(|x| &x.kind), Some(crate::ast::ExprKind::Closure { .. }))).count());
    if path.is_empty() {
        format!("{}::{{closure#{}}}", prog.file_paths.get(file as usize).map(|p| p.rsplit('/').next().unwrap_or(p)).unwrap_or("?"), n)
    } else {
        format!("{}::{{closure#{}}}", path, n)
    }
}

pub fn hint_path(defs: &[Def], syms: &Interner, files: &[std::sync::Arc<ParsedFile>], d: DefId) -> String {
    fn ty_text(f: &ParsedFile, t: crate::ast::TyId, depth: u32) -> String {
        if depth > 8 {
            return "_".to_string();
        }
        match f.ast.ty(t) {
            Ty::Path(p) => f.ast.segs(p.segs).iter().map(|s| f.text(s.name)).collect::<Vec<_>>().join("::"),
            Ty::Ref(_, m, x) => format!("&{}{}", if *m { "mut " } else { "" }, ty_text(f, *x, depth + 1)),
            Ty::Ptr(m, x) => format!("*{} {}", if *m { "mut" } else { "const" }, ty_text(f, *x, depth + 1)),
            Ty::Slice(x) => format!("[{}]", ty_text(f, *x, depth + 1)),
            Ty::Array(x, _) => format!("[{}; _]", ty_text(f, *x, depth + 1)),
            Ty::Paren(x) => ty_text(f, *x, depth + 1),
            Ty::Tuple(v) => format!("({})", f.ast.list(*v).iter().map(|x| ty_text(f, *x, depth + 1)).collect::<Vec<_>>().join(", ")),
            Ty::DynTrait(..) => "dyn".to_string(),
            _ => "_".to_string(),
        }
    }
    let mut parts = Vec::new();
    let mut cur = d;
    while cur != NO_DEF && parts.len() < 64 {
        let def = &defs[cur.0 as usize];
        let mut name = syms.str(def.name).to_string();
        if def.kind == DefKind::Impl {
            if let Some(f) = files.get(def.file as usize) {
                if let Some(ImplItem { self_ty, trait_, .. }) = f.ast.as_impl(&f.ast.item(def.item).kind) {
                    name = match trait_ {
                        Some(t) => format!("<{} as {}>", ty_text(f, *self_ty, 0), f.ast.segs(t.segs).iter().map(|s| f.text(s.name)).collect::<Vec<_>>().join("::")),
                        None => format!("<{}>", ty_text(f, *self_ty, 0)),
                    };
                }
            }
        }
        parts.push(name);
        cur = def.parent;
    }
    parts.reverse();
    parts.join("::")
}

/// Files parsed ahead (preload.rs) that the loader parsed again (run.rs reports it).
pub static PARSE_AGAIN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A path's segments as (kind, symbol), memoised per occurrence in `path_seg_cache` once body
/// memos are on.
pub fn path_segs(prog: &Program, file: u32, p: &Path) -> std::sync::Arc<[(SegKind, Sym)]> {
    if !prog.body_memos.load(std::sync::atomic::Ordering::Relaxed) {
        return path_segs_uncached(prog, file, p).into();
    }
    let mut h: u64 = 0;
    for s in prog.files[file as usize].ast.segs(p.segs) {
        h = (h.rotate_left(5) ^ (((s.name.lo as u64) << 32) | s.name.hi as u64)).wrapping_mul(0xf135_7aea_2e62_a9c5);
    }
    let lo = prog.files[file as usize].ast.segs(p.segs).first().map_or(0, |s| s.name.lo);
    let key = (file, p.segs.len() as u32, lo, h);
    let stale = match prog.path_seg_cache.get(&key) {
        // (a name not interned when the entry was made may be now: Program::commit_version_in)
        Some(v) if v.iter().any(|&(_, sym)| sym == u32::MAX) => Some(v),
        Some(v) => return v,
        None => None,
    };
    let v: std::sync::Arc<[(SegKind, Sym)]> = path_segs_uncached(prog, file, p).into();
    if stale.as_deref() != Some(&*v) { prog.path_seg_cache.insert(key, v.clone()); }
    v
}

fn path_segs_uncached(prog: &Program, file: u32, p: &Path) -> Vec<(SegKind, Sym)> {
    let f = &prog.files[file as usize];
    let mut v = Vec::new();
    for s in prog.files[file as usize].ast.segs(p.segs) {
        let t = f.text(s.name);
        let t = match t.strip_prefix("r#") {
            Some(x) => x,
            None => t,
        };
        v.push((s.kind(), prog.syms.get(t).unwrap_or(u32::MAX)));
    }
    v
}

/// One lookup through glob imports (a fixpoint over glob cycles, see lookup_in_module_vis).
#[derive(Default)]
struct GlobQuery {
    /// bindings carry their written visibility (`pub(crate)`, `pub(super)`, ..) -- privacy checks
    precise: bool,
    memo: HashMap<u32, Option<(DefId, BindVis)>>,
    /// the previous pass's answers (what a module still being looked through answers)
    prev: HashMap<u32, Option<(DefId, BindVis)>>,
    open: Vec<u32>,
    /// (module still being looked through, the answer it gave)
    hits: Vec<(u32, Option<(DefId, BindVis)>)>,
}

/// One walk of trait bindings through glob imports (see GlobQuery).
#[derive(Default)]
struct TraitQuery {
    memo: HashMap<u32, std::sync::Arc<Vec<(DefId, BindVis)>>>,
    prev: HashMap<u32, std::sync::Arc<Vec<(DefId, BindVis)>>>,
    open: Vec<u32>,
    hits: Vec<(u32, std::sync::Arc<Vec<(DefId, BindVis)>>)>,
}

crate::gen_local! {
    /// (skipped module, name) pairs a lookup on this thread missed since the last take_missing.
    static MISSING: std::cell::RefCell<Vec<(u32, String)>>;
}
thread_local! {
    /// The latest missing_mark: a note is deduplicated only against the notes from there on,
    /// so the notes since any open mark hold every miss of its lookups, also one this thread
    /// noted before the mark (an earlier lookup of the same name).
    static MISSING_FROM: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
crate::gen_local! {
    /// The notes of MISSING[from..] as a set (push_missing's dedup), valid for (generation,
    /// from, len); a body's bound-variable walk notes thousands of misses in one window, and
    /// a scan of the window per note made that quadratic.
    static MISSING_SEEN: std::cell::RefCell<(u64, usize, usize, std::collections::HashSet<(u32, String)>)>;
}
thread_local! {
    /// bumped whenever MISSING is replaced (save, restore, take)
    static MISSING_GEN: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

fn bump_missing_gen() { let _ = MISSING_GEN.try_with(|g| g.set(g.get() + 1)); }

fn push_missing(m: &mut Vec<(u32, String)>, w: u32, name: &str) {
    let from = MISSING_FROM.try_with(|f| f.get()).unwrap_or(0).min(m.len());
    let gen = MISSING_GEN.try_with(|g| g.get()).unwrap_or(0);
    let pushed = MISSING_SEEN.try_with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.0 != gen || seen.1 != from || seen.2 != m.len() {
            seen.3.clear();
            for n in &m[from..] { seen.3.insert(n.clone()); }
            seen.0 = gen; seen.1 = from;
        }
        let key = (w, name.to_owned());
        if seen.3.contains(&key) { seen.2 = m.len(); return false; }
        m.push(key.clone());
        seen.3.insert(key);
        seen.2 = m.len();
        true
    });
    if pushed.is_err() && !m[from..].iter().any(|(x, n)| *x == w && n == name) { m.push((w, name.to_owned())); }
}

/// Signature collection runs on several threads: while it runs, misses are also gathered here.
static COLLECTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static COLLECT_MISSES: relax_session::sync::Mutex<Vec<(u32, String)>> = relax_session::sync::Mutex::new(Vec::new());

/// Start gathering the misses of a signature collection (any thread).
pub fn collect_misses_begin() {
    COLLECT_MISSES.lock().unwrap_or_else(|e| e.into_inner()).clear();
    COLLECTING.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// The (skipped module, name) misses since collect_misses_begin; stops gathering.
pub fn collect_misses_take() -> Vec<(u32, String)> {
    COLLECTING.store(false, std::sync::atomic::Ordering::SeqCst);
    std::mem::take(&mut *COLLECT_MISSES.lock().unwrap_or_else(|e| e.into_inner()))
}

/// This thread's miss notes so far (missing_confirm's / missing_since's mark).
pub fn missing_mark() -> usize {
    let n = MISSING.try_with(|m| m.borrow().len()).unwrap_or(0);
    let _ = MISSING_FROM.try_with(|f| f.set(n));
    n
}

/// A resolution that began at `mark` failed for good (a signature's type not found): its misses
/// are a collection's to load (only those: other misses may resolve some other way).
pub fn missing_confirm(mark: usize) {
    if !COLLECTING.load(std::sync::atomic::Ordering::Relaxed) { return; }
    let notes: Vec<(u32, String)> = MISSING.try_with(|m| m.borrow().get(mark..).map(<[_]>::to_vec).unwrap_or_default()).unwrap_or_default();
    let mut g = COLLECT_MISSES.lock().unwrap_or_else(|e| e.into_inner());
    for n in notes { if !g.contains(&n) { g.push(n); } }
}

fn note_missing(waits: &[u32], name: &str) {
    if matches!(name, "crate" | "self" | "super" | "Self") { return; }
    // try_with: a thread's TLS destructors can run compiled code (and lookups)
    let _ = MISSING.try_with(|m| {
        let mut m = m.borrow_mut();
        for &w in waits { push_missing(&mut m, w, name); }
    });
}

/// This thread's miss notes since `mark` (missing_mark), left in place.
pub fn missing_since(mark: usize) -> Vec<(u32, String)> {
    MISSING.try_with(|m| m.borrow().get(mark..).map(<[_]>::to_vec).unwrap_or_default()).unwrap_or_default()
}

/// Notes misses recorded earlier (a cached result's) as this thread's own: a build reading a
/// cached type-check failure fails for the same skipped modules as the run that computed it.
pub fn note_misses(misses: &[(u32, String)]) {
    let _ = MISSING.try_with(|m| {
        let mut m = m.borrow_mut();
        for (w, name) in misses { push_missing(&mut m, *w, name); }
    });
}

/// This thread's miss notes and dedup window, taken (cleared) for a nested build to start
/// clean; missing_restore puts them back when it ends, so the enclosing work keeps its notes.
pub fn missing_save() -> (Vec<(u32, String)>, usize) {
    let from = MISSING_FROM.try_with(|f| f.replace(0)).unwrap_or(0);
    bump_missing_gen();
    (MISSING.try_with(|m| std::mem::take(&mut *m.borrow_mut())).unwrap_or_default(), from)
}

pub fn missing_restore(saved: (Vec<(u32, String)>, usize)) {
    let _ = MISSING_FROM.try_with(|f| f.set(saved.1));
    bump_missing_gen();
    let _ = MISSING.try_with(|m| *m.borrow_mut() = saved.0);
}

/// The (skipped module, name) misses of this thread's lookups (cleared).
pub fn take_missing() -> Vec<(u32, String)> {
    let _ = MISSING_FROM.try_with(|f| f.set(0));
    bump_missing_gen();
    MISSING.try_with(|m| std::mem::take(&mut *m.borrow_mut())).unwrap_or_default()
}

#[cfg(test)]
mod missing_tests {
    use super::*;

    /// A lookup after a mark that misses a name this thread already missed before the mark
    /// still shows in the notes since the mark (missing_confirm / a failed type check's
    /// Body::deferred_misses); a nested build's save/restore keeps the enclosing notes.
    #[test]
    fn notes_since_a_mark_hold_repeated_misses_and_survive_a_nested_build() {
        let _ = take_missing();
        note_missing(&[3], "Foo");
        let mark = missing_mark();
        note_missing(&[3], "Foo");
        note_missing(&[3], "Foo");
        assert_eq!(missing_since(mark), vec![(3, "Foo".to_string())]);
        let saved = missing_save();
        assert!(missing_since(0).is_empty());
        note_missing(&[4], "Bar");
        assert_eq!(take_missing(), vec![(4, "Bar".to_string())]);
        missing_restore(saved);
        note_misses(&[(3, "Foo".to_string())]);
        assert_eq!(missing_since(mark), vec![(3, "Foo".to_string())]);
        assert_eq!(take_missing().len(), 2);
    }
}

#[cfg(test)]
mod deferred_load_tests {
    use super::*;
    use std::sync::Arc;

    // Run the original full import/defer fixpoint on the same publication order.
    // This also compares intermediate waits, before all skipped files are loaded.
    fn full_load(p: &mut Program, m: u32) {
        let (path, dir, mod_rs) = p.skipped_files.remove(&m).unwrap();
        p.imports_done = false;
        let krate = p.mods[m as usize].krate;
        p.cur_mod_path = p.def_path(p.mods[m as usize].def);
        let f = p.load_file(&path, 2021, krate).unwrap();
        p.set_mod_file(m, f);
        let items = p.files[f as usize].ast.root_items.clone();
        p.collect_items(f, &items, m, &dir, mod_rs).unwrap();
        p.resolve_imports_passes(0);
        let pending = (0..p.imports.len()).filter(|&i| !p.imports[i].done).collect::<Vec<_>>();
        p.deferred_imports.clear();
        p.defer_skipped_imports(1, &pending);
        p.scope_cache.clear();
        p.glob_cache.clear();
        p.path_seg_cache.clear();
        p.trait_bind_cache.clear();
        p.trait_scope.clear();
        p.imports_done = true;
    }

    fn module(p: &Program, name: &str) -> u32 {
        (0..p.mods.len() as u32).find(|&m| p.def_path(p.mods[m as usize].def) == name).unwrap()
    }

    fn lookups(p: &Program, names: &[String]) -> std::collections::BTreeMap<String, Vec<String>> {
        let mut out = std::collections::BTreeMap::new();
        for m in 0..p.mods.len() as u32 {
            let mut values = Vec::new();
            for name in names {
                let sym = p.syms.intern_late(name);
                for ty in [false, true] {
                    for result in [p.lookup_in_module(m, sym, ty, &mut Vec::new()), p.lookup_in_scope(m, sym, ty)] {
                        values.push(result.map_or_else(|| "-".into(), |d| format!("{}:{:?}", p.def_path(d), p.def(d).kind)));
                    }
                }
            }
            let mut traits = p.traits_in_scope(m).iter().map(|&d| p.def_path(d)).collect::<Vec<_>>();
            traits.sort();
            values.extend(traits);
            out.insert(p.def_path(p.mods[m as usize].def), values);
        }
        out
    }

    #[test]
    fn incremental_load_matches_full_resolution() {
        const SKIP: &str = "load_late.rs,load_other.rs,load_unrelated.rs,load_nested.rs,load_empty.rs";
        if std::env::var("MAKEPAD_RUN_LAZY_DEFER").as_deref() != Ok(SKIP) {
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "program::deferred_load_tests::incremental_load_matches_full_resolution",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("MAKEPAD_RUN_LAZY_DEFER", SKIP)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            return;
        }
        let dir = std::env::temp_dir().join(format!("relax-front-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname=\"load_test\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[lib]\npath=\"lib.rs\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("lib.rs"),
            r#"
            #[path = "load_late.rs"] pub mod late;
            #[path = "load_other.rs"] pub mod other;
            #[path = "load_unrelated.rs"] pub mod unrelated;
            #[path = "load_empty.rs"] pub mod empty;
            pub mod fallback { pub fn Pick() {} pub trait Shared {} pub trait Stable {} pub struct Hit; }
            pub mod exporter { pub use crate::late::{Pick, Shared}; pub use crate::fallback::*; pub use crate::late::*; }
            pub mod consumer { pub use crate::exporter::*; pub use crate::cycle::*; }
            pub mod cycle { pub use crate::consumer::*; }
            pub mod renamed { pub use crate::late::Missing as Alias; pub use crate::exporter::Pick; }
            pub mod chain { pub use crate::renamed::{Alias, Pick}; }
            pub mod prefix { pub use crate::late::inner::Thing; }
            pub mod new_glob { pub use crate::late::inner::*; pub use crate::fallback::*; }
            pub mod isolated { pub use crate::fallback::*; }
            pub mod waiting { pub use crate::unrelated::Unrelated; }
            pub mod prelude { pub use crate::late::*; }
            pub fn main() {}
        "#,
        )
        .unwrap();
        std::fs::write(
            dir.join("load_late.rs"),
            r#"
            pub struct Pick { pub value: u32 }
            pub struct Missing;
            pub trait Added {}
            pub trait Shared {}
            pub use crate::other::Next;
            pub mod inner { pub struct Thing; pub use crate::other::After; }
            #[path = "load_nested.rs"] pub mod nested;
        "#,
        )
        .unwrap();
        std::fs::write(dir.join("load_other.rs"), "pub struct Next; pub trait After {}").unwrap();
        std::fs::write(dir.join("load_unrelated.rs"), "pub struct Unrelated;").unwrap();
        std::fs::write(dir.join("load_nested.rs"), "pub trait Deep {}").unwrap();
        std::fs::write(dir.join("load_empty.rs"), "").unwrap();
        let root = dir.join("lib.rs").to_string_lossy().into_owned();
        let load = |lazy| {
            let mut session = crate::Session::default();
            session.options.rs_lazy = lazy;
            let session = Arc::new(session);
            let cfg = CfgSet::host(&session, &[]);
            let mut p = Program::new(&session);
            if lazy {
                p.preload(vec![crate::preload::PreCrate {
                    name: "load_test".into(),
                    root: root.clone(),
                    edition: 2021,
                    cfg: cfg.clone(),
                    deps: Vec::new(),
                    
                    proc_macro: false,
                    image_deps: Vec::new(),
                    image_dep_names: Vec::new(),
                }]);
            }
            p.load_crate("load_test", &root, 2021, cfg, Vec::new()).unwrap();
            p.resolve_imports();
            assert!(p.errors.is_empty(), "{:?}", p.errors);
            p.body_memos.store(true, std::sync::atomic::Ordering::Relaxed);
            p
        };
        for prelude in [false, true] {
            let mut incremental = load(true);
            let mut full = load(true);
            let mut eager = load(false);
            // Restored collected images have waits/import indexes, but no derived
            // reverse graph. Exercise that one-time reconstruction as well.
            if prelude {
                incremental.load_graph = None;
            }
            // Model an anonymous lexical child without asking the body parser to
            // parse an unrelated function. Both resolvers see the same scope.
            for p in [&mut incremental, &mut full, &mut eager] {
                let parent = module(p, "load_test::consumer");
                let file = p.mods[parent as usize].file;
                let name = p.syms.intern("lexical");
                let def = p.add_def(Def {
                    kind: DefKind::Mod,
                    name,
                    parent: p.mods[parent as usize].def,
                    file,
                    item: ItemId(0),
                    sub: 0,
                    is_pub: false,
                    krate: 0,
                    scope: parent,
                });
                let m = p.new_module(def, None, Some(parent), 0, file);
                p.defs[def.0 as usize].sub = m;
                if prelude {
                    p.prelude_crate = Some(0);
                }
            }
            let mut names: Vec<String> = eager.syms.names().map(str::to_owned).collect();
            names.push("AbsentEverywhere".into());
            assert_eq!(incremental.skipped_files.len(), 4);
            for name in ["load_test::empty", "load_test::late", "load_test::other", "load_test::late::nested", "load_test::unrelated"] {
                assert_eq!(lookups(&incremental, &names), lookups(&full, &names));
                let isolated = module(&incremental, "load_test::isolated");
                let hit = incremental.syms.get("Hit").unwrap();
                let stable = incremental.scope_cache.get(&(isolated, hit, true)).unwrap();
                let m = module(&incremental, name);
                let (_, changed) = incremental.load_deferred_module_changes(m).unwrap();
                assert_eq!(changed, name != "load_test::empty");
                full_load(&mut full, module(&incremental, name));
                if !prelude {
                    assert_eq!(incremental.scope_cache.get(&(isolated, hit, true)), Some(stable));
                    assert!(incremental.trait_bind_cache.get(&isolated).is_some());
                    assert!(incremental.trait_scope.get(&isolated).is_some());
                }
                assert!(!incremental.scope_cache.contains(&(isolated, incremental.syms.get("AbsentEverywhere").unwrap(), true)));
                assert_eq!(incremental.deferred_waits, full.deferred_waits, "{name}: waits");
                assert_eq!(incremental.deferred_binds, full.deferred_binds, "{name}: bindings");
                assert_eq!(
                    incremental.imports.iter().map(|i| i.done).collect::<Vec<_>>(),
                    full.imports.iter().map(|i| i.done).collect::<Vec<_>>()
                );
                assert_eq!(lookups(&incremental, &names), lookups(&full, &names), "{name}");
            }
            assert_eq!(lookups(&incremental, &names), lookups(&eager, &names));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
