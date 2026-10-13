//! `macro_rules!`: definitions, matching and transcription. Expansion happens while parsing
//! (the parser appends the transcribed tokens to its token list and parses them in place), so
//! expanded code lives in the calling file's syntax tree like hand-written code. Expanded token
//! positions map back to the call site for diagnostics.

use crate::lexer::{Tok, T};
use crate::parser::{PResult, Parser};
use crate::fxhash::HashMap;

/// A token of a macro definition (owned text: the definition may live in another file).
#[derive(Clone, Debug, PartialEq)]
pub struct MTok {
    pub kind: T,
    pub text: String,
    /// position of the token where the macro is defined (its file; hygiene: a body token's
    /// own context), u32::MAX when unknown
    pub pos: u32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Frag {
    Expr,
    Ident,
    Ty,
    Tt,
    Pat,
    PatParam,
    Path,
    Literal,
    Block,
    Item,
    Stmt,
    Vis,
    Lifetime,
    Meta,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MNode {
    Tok(MTok),
    Var(String, Frag),
    /// `$( .. ) sep op` (op: b'*', b'+', b'?')
    Rep(Vec<MNode>, Option<MTok>, u8),
    Group(MTok, MTok, Vec<MNode>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum TNode {
    Tok(MTok),
    Var(String),
    Rep(Vec<TNode>, Option<MTok>, u8),
    Group(MTok, MTok, Vec<TNode>),
    Crate,
}

#[derive(Clone)]
pub struct MacroDef {
    pub name: String,
    /// crate that defines it (`$crate`)
    pub krate: String,
    /// `#[macro_export]`: visible to other crates
    pub exported: bool,
    pub arms: Vec<(Vec<MNode>, Vec<TNode>)>,
    /// a Relax built-in (builtins.rs `Builtin` as u8; 0: a `macro_rules!` macro)
    pub builtin: u8,
    /// where a `macro_rules!` is defined: (file path, source position) -- its expansions'
    /// identifiers resolve local variables there (definition-site hygiene)
    pub def_file: String,
    pub def_pos: u32,
    /// Exact definition input, for prescan/outline/parser reuse. Images already
    /// contain parsed arms and need not persist this cold-parse acceleration.
    pub(crate) input: Option<std::sync::Arc<RuleInput>>,
}

#[derive(Clone)]
pub(crate) struct RuleInput {
    tokens: Vec<MTok>,
    local_inner: bool,
}

pub type MacroRef = std::sync::Arc<MacroDef>;

/// The crate name of a macro-table crate key (`<name>\u{1}lib`: a lib shadowed by its bin).
fn krate_name(k: &str) -> &str {
    k.split('\u{1}').next().unwrap_or(k)
}

/// `#[macro_export(local_inner_macros)]`: every single-segment macro call in the bodies is
/// `$crate::name!` (rustc), so a body's `inner!(..)` names the defining crate's macro wherever
/// it expands (downcast-rs: `impl_downcast!` calls itself; Relax left those calls unexpanded).
pub fn local_inner_macros(d: &mut MacroDef) {
    if let Some(input) = &mut d.input { std::sync::Arc::make_mut(input).local_inner = true; }
    fn walk(v: &mut Vec<TNode>) {
        let mut i = 0;
        while i < v.len() {
            match &mut v[i] {
                TNode::Rep(inner, _, _) | TNode::Group(_, _, inner) => walk(inner),
                _ => {}
            }
            let call = matches!(&v[i], TNode::Tok(t) if matches!(t.kind, T::Ident | T::RawIdent) && t.text != "macro_rules")
                && matches!(v.get(i + 1), Some(TNode::Tok(t)) if t.kind == T::Not)
                && !(i > 0 && matches!(&v[i - 1], TNode::Tok(t) if t.kind == T::PathSep || t.text == "$"));
            if call {
                let pos = match &v[i] {
                    TNode::Tok(t) => t.pos,
                    _ => u32::MAX,
                };
                v.insert(i, TNode::Tok(MTok { kind: T::PathSep, text: "::".to_string(), pos }));
                v.insert(i, TNode::Crate);
                i += 3;
            }
            i += 1;
        }
    }
    for (_, body) in d.arms.iter_mut() {
        walk(body);
    }
}

/// One `use` declaration's binding, as the preload scan reads it: in module `module` (module path
/// as `module_path!` prints it, `krate::a::b`), the path `path` (first segment as written:
/// `crate`, `self`, `super`, `$crate` as `__relax_crate_<name>`, `""` for a leading `::`) is
/// bound under `bind`, or glob-imported (`bind` None). `extern crate x as y` is `["", x]` as `y`.
#[derive(Clone, Debug)]
pub struct UseItem {
    pub module: String,
    pub path: Vec<String>,
    pub bind: Option<String>,
    pub is_pub: bool,
}

/// One crate's module tree and imports (for macro path resolution before name resolution:
/// rustc resolves macro paths with the same import resolution as items).
#[derive(Default)]
pub struct CrateMods {
    /// the crate root's module path (the crate name)
    pub root: String,
    /// the crate root's source file: two crates of one name (a package's lib and bin) are told
    /// apart by it
    pub src: String,
    /// every module path of the crate
    pub mods: crate::fxhash::HashSet<String>,
    /// `use` / `extern crate` bindings per module
    pub uses: HashMap<String, Vec<UseItem>>,
    /// extern prelude: names the crate refers to other crates by -> their crate names
    pub externs: HashMap<String, String>,
    /// names `use .. as name` binds under another name
    pub renames: crate::fxhash::HashSet<String>,
    /// the crate's resolved scopes, computed once on first use (CrateScope::compute)
    pub scope: std::sync::OnceLock<CrateScope>,
}

/// A crate's module scopes resolved once (rustc_resolve's import fixpoint, for the macro and
/// module namespaces): per module, the modules / crates and the macros each name denotes --
/// non-glob bindings (child modules, `use` / `extern crate`, the root's `#[macro_export]`s)
/// ahead of glob ones. Lookups are then hash probes.
#[derive(Default)]
pub struct CrateScope {
    /// module -> name -> (crate, module) of its non-glob bindings; its glob imports' source
    /// modules in source order (searched on demand: copying every module's names into every
    /// glob importer costs more than the few path lookups that need them)
    tnon: HashMap<String, HashMap<String, (String, String)>>,
    tglobs: HashMap<String, Vec<(String, String)>>,
    /// module -> name -> macro, non-glob then glob
    mnon: HashMap<String, HashMap<String, MacroRef>>,
    mglob: HashMap<String, HashMap<String, MacroRef>>,
    /// this computation's identity (unique per process, never 0): a memoized answer that went
    /// through another crate's scope holds while that crate still has this very scope
    id: u64,
    /// module -> name -> the answer of `Res::type_in` (a glob walk can visit hundreds of
    /// modules; the same few names are asked from the same modules over and over)
    memo: std::sync::RwLock<HashMap<String, HashMap<String, TypeInMemo>>>,
}

static SCOPE_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A `type_in` answer: the module whose non-glob bindings had the name (None: no module had
/// it), and every crate the walk visited with its scope's id then (0: no scope). The answer
/// holds while each of those crates has the same scope (scopes never change once computed).
struct TypeInMemo {
    at: Option<(String, String)>,
    deps: Box<[(String, u64)]>,
}

/// Answers verified in this table, rather than in the shared CrateScope. Cloning a
/// publication starts an empty cache; changing its module graph clears it. A hit
/// therefore needs no walk over the other crates consulted by the original query.
#[derive(Default)]
struct TypeSnapshot(std::sync::RwLock<HashMap<String, HashMap<String, HashMap<String, Option<(String, String)>>>>>);

impl Clone for TypeSnapshot {
    fn clone(&self) -> Self { Self::default() }
}

#[derive(Default)]
struct ExpansionSnapshot {
    crates: std::sync::RwLock<HashMap<String, std::sync::Arc<ExpansionIndex>>>,
    aliases: std::sync::RwLock<HashMap<String, bool>>,
}

impl Clone for ExpansionSnapshot {
    fn clone(&self) -> Self {
        Self { crates: std::sync::RwLock::new(self.crates.read().unwrap().clone()),
            aliases: std::sync::RwLock::new(self.aliases.read().unwrap().clone()) }
    }
}

/// Pure fallback answers, shared by files checked against this publication.
/// The outer key is the querying crate: a traversal that consulted that crate's
/// local environment is kept only in MacroScope, never in this shared cache.
#[derive(Default)]
struct ImportSnapshot(std::sync::RwLock<HashMap<Name, HashMap<ImportKey, ImportAnswer>>>);

impl Clone for ImportSnapshot {
    fn clone(&self) -> Self { Self::default() }
}

#[derive(Default)]
struct ReplaySnapshot(std::sync::RwLock<HashMap<u64, bool>>);
impl Clone for ReplaySnapshot {
    fn clone(&self) -> Self { Self::default() }
}

/// An interned string of the import state (crate, module, bound and macro names). Ids belong to
/// the `MacroNames` of the table lineage the state was made under.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug)]
pub struct Name(u32);

/// `GLOB` and `GLOB_SEP`, interned first by every `MacroNames`.
const GLOB_ID: Name = Name(0);
const GLOB_SEP_ID: Name = Name(1);

const NAME_SHARDS: usize = 16;
/// Chunk c of the id -> string storage holds `64 << c` names.
const NAME_CHUNKS: usize = 26;

/// Append-only string interner, shared by every snapshot of one macro table lineage (parallel
/// outline/expansion workers use it). Lookups of known strings take one shard's read lock; an id
/// indexes its string with no lock at all.
pub struct MacroNames {
    shards: [std::sync::RwLock<HashMap<Box<str>, Name>>; NAME_SHARDS],
    next: std::sync::atomic::AtomicU32,
    chunks: [std::sync::OnceLock<Box<[std::sync::OnceLock<Box<str>>]>>; NAME_CHUNKS],
}

impl Default for MacroNames {
    fn default() -> Self {
        let names = Self {
            shards: Default::default(),
            next: Default::default(),
            chunks: std::array::from_fn(|_| std::sync::OnceLock::new()),
        };
        assert!(names.id(GLOB) == GLOB_ID && names.id(GLOB_SEP) == GLOB_SEP_ID);
        names
    }
}

impl MacroNames {
    fn slot(id: u32) -> (usize, usize, usize) {
        let n = id as usize + 64;
        let hi = (usize::BITS - 1 - n.leading_zeros()) as usize;
        (hi - 6, n - (1 << hi), 1 << hi)
    }

    fn shard(s: &str) -> usize {
        use std::hash::BuildHasher;
        (crate::fxhash::FxBuild::default().hash_one(s) >> 60) as usize
    }

    /// The id of `s` (interned on first sight).
    pub fn id(&self, s: &str) -> Name {
        let shard = &self.shards[Self::shard(s)];
        if let Some(&n) = shard.read().unwrap().get(s) { return n; }
        let mut shard = shard.write().unwrap();
        if let Some(&n) = shard.get(s) { return n; }
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (chunk, at, size) = Self::slot(id);
        let chunk = self.chunks[chunk].get_or_init(|| (0..size).map(|_| std::sync::OnceLock::new()).collect());
        let _ = chunk[at].set(s.into());
        shard.insert(s.into(), Name(id));
        Name(id)
    }

    /// The id of `s` if it was interned.
    pub fn get(&self, s: &str) -> Option<Name> {
        self.shards[Self::shard(s)].read().unwrap().get(s).copied()
    }

    pub fn str(&self, n: Name) -> &str {
        let (chunk, at, _) = Self::slot(n.0);
        self.chunks[chunk].get().expect("name chunk")[at].get().expect("name slot")
    }
}

/// `f` with the strings of `path` (on the stack for the short paths there are).
fn with_strs<R>(names: &MacroNames, path: &[Name], f: impl FnOnce(&[&str]) -> R) -> R {
    let mut near = [""; 8];
    if path.len() <= near.len() {
        for (slot, &n) in near.iter_mut().zip(path) { *slot = names.str(n); }
        f(&near[..path.len()])
    } else {
        let all: Vec<&str> = path.iter().map(|&n| names.str(n)).collect();
        f(&all)
    }
}

/// `owner` and its parent modules join `modules` (stops at the first one already there).
fn add_import_module_id(modules: &mut crate::fxhash::HashSet<Name>, names: &MacroNames, owner: Name) {
    let mut module = owner;
    while modules.insert(module) {
        let Some((parent, _)) = names.str(module).rsplit_once("::") else { break; };
        module = names.id(parent);
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum LookupInput {
    Definitions(Name),
    Prelude(Name),
    Import(Name, Name, Name),
    GeneratedModule(Name, Name),
    Alias(Name),
}
enum InputValue {
    Definitions(Vec<MacroRef>),
    Prelude(Vec<String>),
    Import(Option<Vec<String>>),
    Present(bool),
}

/// Inputs actually read by one completed lookup. Cached import states share a
/// certificate for their whole fixpoint; dependencies on older certificates form
/// a DAG. Validation memoizes each certificate once per publication, including
/// misses, without replaying the import graph or duplicating its transitive reads.
struct LookupInputs {
    id: u64,
    graph: std::sync::Arc<()>,
    reads: HashMap<LookupInput, InputValue>,
    dependencies: HashMap<u64, std::sync::Arc<LookupInputs>>,
}
impl LookupInputs {
    fn new(table: &MacroTable) -> Self {
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self { id: IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed), graph: table.lookup_graph.clone(),
            reads: Default::default(), dependencies: Default::default() }
    }
    fn read(&mut self, table: &MacroTable, key: LookupInput) {
        let names = &*table.names;
        self.reads.entry(key).or_insert_with_key(|key| match *key {
            LookupInput::Definitions(name) => InputValue::Definitions(table.defs.get(names.str(name)).cloned().unwrap_or_default()),
            LookupInput::Prelude(krate) => InputValue::Prelude(table.macro_use.get(names.str(krate)).cloned().unwrap_or_default()),
            LookupInput::Import(krate, module, name) => InputValue::Import(table.exp_uses.get(names.str(krate)).and_then(|u| u.get(&(names.str(module).to_string(), names.str(name).to_string()))).cloned()),
            LookupInput::GeneratedModule(krate, module) => InputValue::Present(table.expansion_module(names.str(krate), names.str(module))),
            LookupInput::Alias(name) => InputValue::Present(table.expansion_alias(names.str(name))),
        });
    }
    fn depend(&mut self, inputs: &std::sync::Arc<Self>) {
        self.dependencies.entry(inputs.id).or_insert_with(|| inputs.clone());
    }
    fn same_reads(&self, table: &MacroTable) -> bool {
        let names = &*table.names;
        std::sync::Arc::ptr_eq(&self.graph, &table.lookup_graph) && self.reads.iter().all(|(key, value)| match (*key, value) {
            (LookupInput::Definitions(name), InputValue::Definitions(was)) => {
                let now = table.defs.get(names.str(name)).map_or(&[][..], |v| &v[..]);
                now.len() == was.len() && now.iter().zip(was).all(|(a, b)| std::sync::Arc::ptr_eq(a, b))
            }
            (LookupInput::Prelude(krate), InputValue::Prelude(was)) => table.macro_use.get(names.str(krate)).map_or(&[][..], |v| &v[..]) == was,
            (LookupInput::Import(krate, module, name), InputValue::Import(was)) => table.exp_uses.get(names.str(krate)).and_then(|u| u.get(&(names.str(module).to_string(), names.str(name).to_string()))) == was.as_ref(),
            (LookupInput::GeneratedModule(krate, module), InputValue::Present(was)) => table.expansion_module(names.str(krate), names.str(module)) == *was,
            (LookupInput::Alias(name), InputValue::Present(was)) => table.expansion_alias(names.str(name)) == *was,
            _ => unreachable!(),
        })
    }
    fn unchanged(&self, table: &MacroTable) -> bool {
        if let Some(&ok) = table.replay_snapshot.0.read().unwrap().get(&self.id) { return ok; }
        // The memo can form a long history of completed queries. Walk it without
        // consuming stack proportional to that history.
        let mut pending = vec![(self, false)];
        while let Some((inputs, finish)) = pending.pop() {
            if table.replay_snapshot.0.read().unwrap().contains_key(&inputs.id) { continue; }
            if !finish {
                if !inputs.same_reads(table) {
                    table.replay_snapshot.0.write().unwrap().insert(inputs.id, false);
                    continue;
                }
                pending.push((inputs, true));
                pending.extend(inputs.dependencies.values().map(|dep| (dep.as_ref(), false)));
            } else {
                let ok = inputs.dependencies.values().all(|dep| table.replay_snapshot.0.read().unwrap().get(&dep.id) == Some(&true));
                table.replay_snapshot.0.write().unwrap().insert(inputs.id, ok);
            }
        }
        table.replay_snapshot.0.read().unwrap()[&self.id]
    }
}

impl Drop for LookupInputs {
    fn drop(&mut self) {
        let mut pending = self.dependencies.drain().map(|(_, dep)| dep).collect::<Vec<_>>();
        while let Some(dep) = pending.pop() {
            if let Some(mut dep) = std::sync::Arc::into_inner(dep) {
                pending.extend(dep.dependencies.drain().map(|(_, dep)| dep));
            }
        }
    }
}

fn import_modules<'a>(owners: impl Iterator<Item = &'a String>) -> crate::fxhash::HashSet<String> {
    let mut modules = crate::fxhash::HashSet::default();
    for owner in owners {
        add_import_module(&mut modules, owner);
    }
    modules
}

/// `owner` and its parent modules join `modules` (stops at the first one already there: its
/// parents are too).
fn add_import_module(modules: &mut crate::fxhash::HashSet<String>, owner: &str) {
    let mut module = owner;
    while modules.insert(module.to_string()) {
        let Some((parent, _)) = module.rsplit_once("::") else { break; };
        module = parent;
    }
}

/// Macros by name, in definition order across the crates loaded so far (dependencies load
/// first), with each crate's module tree and imports.
#[derive(Default, Clone)]
pub struct MacroTable {
    pub defs: HashMap<String, Vec<MacroRef>>,
    /// `use path::{orig as alias}` per crate: crate -> alias -> (first path segment, orig).
    /// One shared map per crate: a snapshot of the table (one per published crate, for its
    /// parses) copies 1 pointer per crate, not every alias of every crate.
    pub aliases: HashMap<String, std::sync::Arc<HashMap<String, (String, String)>>>,
    /// per crate: `#[macro_use] extern crate dep;` crates, in source order (the macro_use prelude)
    pub macro_use: HashMap<String, Vec<String>>,
    /// names some `use .. as name` binds under another name, in any crate loaded so far (a name
    /// neither defined by a macro_rules nor renamed to cannot resolve to one: no scope walk)
    pub renames: std::sync::Arc<crate::fxhash::HashSet<String>>,
    /// names that can denote a module or crate in some crate loaded so far (module names, crate
    /// and extern-prelude names, import renames): an import of another name binds no module
    pub mod_names: std::sync::Arc<crate::fxhash::HashSet<String>>,
    /// per crate: modules and imports
    pub crates: HashMap<String, std::sync::Arc<CrateMods>>,
    /// the std has the S2 fmt ABI (core::fmt::rt): format macros expand to it (fmtmac.rs)
    pub s2_fmt: bool,
    /// per crate: `use` bindings parses of its files made, macro expansions' included (module,
    /// bound name, path) -- what the prescan's scopes cannot know (MacroScope::lookup)
    pub exp_uses: HashMap<String, std::sync::Arc<crate::fxhash::HashMap<(String, String), Vec<String>>>>,
    /// crates whose load finished (seal_crate): their scopes see every import they will have
    pub sealed: crate::fxhash::HashSet<String>,
    type_snapshot: TypeSnapshot,
    expansion_snapshot: ExpansionSnapshot,
    import_snapshot: ImportSnapshot,
    /// the interner of the import state's names (MacroScope, MacroUse): shared by clones
    pub names: std::sync::Arc<MacroNames>,
    // Prescanned module scopes are immutable between crate publications. Direct
    // graph edits invalidate this identity as well as the ordinary lookup caches.
    lookup_graph: std::sync::Arc<()>,
    replay_snapshot: ReplaySnapshot,
}

impl MacroTable {
    pub fn define(&mut self, d: MacroDef) {
        self.push(std::sync::Arc::new(d));
    }

    /// The import `use path::{orig as alias}` of crate `krate` binding `alias`.
    pub fn alias(&self, krate: &str, alias: &str) -> Option<&(String, String)> {
        self.aliases.get(krate)?.get(alias)
    }

    /// Records an import alias of crate `krate` (copies that crate's map only if a snapshot
    /// still shares it).
    pub fn add_alias(&mut self, krate: String, alias: String, v: (String, String)) {
        std::sync::Arc::make_mut(self.aliases.entry(krate).or_default()).insert(alias, v);
    }

    pub fn push(&mut self, d: MacroRef) {
        self.replay_snapshot.0.get_mut().unwrap().clear();
        self.import_snapshot.0.get_mut().unwrap().clear();
        self.defs.entry(d.name.clone()).or_default().push(d);
    }

    /// Adds crate `name`'s module tree and imports.
    pub fn add_crate(&mut self, name: String, cm: std::sync::Arc<CrateMods>) {
        // the expansion indexes and alias answers depend on exp_uses only, which a crate's
        // module tree does not change: they stay (clearing them rebuilt every crate's index
        // after each crate load)
        let expansion = std::mem::take(&mut self.expansion_snapshot);
        self.clear_lookup_cache();
        self.expansion_snapshot = expansion;
        if cm.renames.iter().any(|r| !self.renames.contains(r)) {
            let set = std::sync::Arc::make_mut(&mut self.renames);
            for r in &cm.renames {
                set.insert(r.clone());
            }
        }
        {
            let set = std::sync::Arc::make_mut(&mut self.mod_names);
            set.insert(name.clone());
            for m in &cm.mods {
                set.insert(m.rsplit_once("::").map_or(m.as_str(), |x| x.1).to_string());
            }
            for e in cm.externs.keys() {
                set.insert(e.clone());
            }
            for r in &cm.renames {
                set.insert(r.clone());
            }
        }
        // a package's bin after its lib (cargo names both after the package): the lib stays
        // reachable under `<name>\u{1}lib`, for the bin's `<name>::..` paths (Res::extern_crate)
        if let Some(old) = self.crates.get(&name) {
            if !old.src.is_empty() && !cm.src.is_empty() && old.src != cm.src {
                let old = old.clone();
                self.crates.insert(format!("{}\u{1}lib", name), old);
            }
        }
        self.crates.insert(name, cm);
    }

    /// Also used when a collected suffix removes module graphs directly.
    pub(crate) fn clear_lookup_cache(&mut self) {
        self.lookup_graph = std::sync::Arc::new(());
        self.replay_snapshot.0.get_mut().unwrap().clear();
        self.type_snapshot.0.get_mut().unwrap().clear();
        self.expansion_snapshot = ExpansionSnapshot::default();
        self.import_snapshot.0.get_mut().unwrap().clear();
    }

    fn expansion_index(&self, krate: &str) -> Option<std::sync::Arc<ExpansionIndex>> {
        if let Some(index) = self.expansion_snapshot.crates.read().unwrap().get(krate) { return Some(index.clone()); }
        let uses = self.exp_uses.get(krate)?;
        let index = std::sync::Arc::new(ExpansionIndex {
            names: uses.keys().map(|(_, bind)| bind.clone()).collect(),
            modules: import_modules(uses.keys().map(|(module, _)| module)),
        });
        self.expansion_snapshot.crates.write().unwrap().insert(krate.into(), index.clone());
        Some(index)
    }

    fn expansion_module(&self, krate: &str, module: &str) -> bool {
        self.expansion_index(krate).is_some_and(|index| index.modules.contains(module))
    }

    fn expansion_alias(&self, name: &str) -> bool {
        if let Some(&found) = self.expansion_snapshot.aliases.read().unwrap().get(name) { return found; }
        let found = self.exp_uses.keys().any(|krate| self.expansion_index(krate).is_some_and(|index| index.names.contains(name)));
        self.expansion_snapshot.aliases.write().unwrap().insert(name.into(), found);
        found
    }

    /// The definition `path::name!` names in module `module` of crate `cur` (`path` empty: an
    /// unqualified `name!`).
    pub fn lookup(&self, name: &str, cur: &str, path: &[String], module: &str) -> Option<MacroRef> {
        lookup_in(self, &[], name, cur, path, module)
    }

    /// A parse's `use` bindings (MacroUse::uses) join crate `k`'s expansion-made imports (glob
    /// entries merge).
    pub fn add_exp_uses(&mut self, k: &str, uses: impl IntoIterator<Item = ((String, String), Vec<String>)>) {
        self.replay_snapshot.0.get_mut().unwrap().clear();
        self.import_snapshot.0.get_mut().unwrap().clear();
        let t = std::sync::Arc::make_mut(self.exp_uses.entry(k.to_string()).or_default());
        // Imports are only added here, so crate k's expansion index (its binding names and
        // their modules) and the alias answers grow in place instead of being rebuilt from all
        // of k's imports on the next lookup (one rebuild per parsed file: 1.3 GB of copies on
        // a cold Stage).
        let crates = self.expansion_snapshot.crates.get_mut().unwrap();
        let mut index = crates.get_mut(k).map(std::sync::Arc::make_mut);
        let aliases = self.expansion_snapshot.aliases.get_mut().unwrap();
        for (key, path) in uses {
            if let Some(index) = index.as_mut() {
                if !index.names.contains(&key.1) { index.names.insert(key.1.clone()); }
                add_import_module(&mut index.modules, &key.0);
            }
            if let Some(found) = aliases.get_mut(&key.1) { *found = true; }
            if key.1 == GLOB {
                let e = t.entry(key).or_default();
                for g in split_globs(&path) {
                    push_glob(e, g);
                }
            } else {
                t.insert(key, path);
            }
        }
    }

    /// Crate `k`'s expansion imports were replaced or removed (not through add_exp_uses): its
    /// cached expansion index and every alias answer go.
    pub(crate) fn forget_expansion_index(&mut self, k: &str) {
        self.expansion_snapshot.crates.get_mut().unwrap().remove(k);
        self.expansion_snapshot.aliases.get_mut().unwrap().clear();
    }

    /// Crate `k` is loaded: the imports its macro expansions wrote (`exp_uses`: derive output's
    /// `use Base as alias;`, `pub use self::m::ancestry as Name;`) join its prescanned ones, with
    /// the modules they sit in, so the crates loaded after it resolve macro paths through them
    /// (rustc resolves expansion-made imports like any other, rustc_resolve ident.rs). Its scope is
    /// computed afresh on next use.
    pub fn seal_crate(&mut self, k: &str) {
        self.sealed.insert(k.to_string());
        let (Some(eu), Some(old)) = (self.exp_uses.get(k).cloned(), self.crates.get(k).cloned()) else { return };
        let mut uses = old.uses.clone();
        let mut renames = old.renames.clone();
        let mut mods = old.mods.clone();
        let mut added = false;
        let mut binds: Vec<(&(String, String), &Vec<String>)> = eu.iter().collect();
        binds.sort();
        for ((m, b), path) in binds {
            // Glob-only generated modules need their parent bindings too.
            let mut p = m.as_str();
            while p.len() > old.root.len() && mods.insert(p.to_string()) {
                added = true;
                match p.rsplit_once("::") {
                    Some((parent, _)) => p = parent,
                    None => break,
                }
            }
            let v = uses.entry(m.clone()).or_default();
            if b == GLOB {
                for g in split_globs(path) {
                    if !v.iter().any(|u| u.bind.is_none() && u.path == g) {
                        v.push(UseItem { module: m.clone(), path: g, bind: None, is_pub: true });
                        added = true;
                    }
                }
                continue;
            }
            if v.iter().any(|u| u.bind.as_deref() == Some(b.as_str()) && &u.path == path) {
                continue;
            }
            v.push(UseItem { module: m.clone(), path: path.clone(), bind: Some(b.clone()), is_pub: true });
            if path.last() != Some(b) {
                renames.insert(b.clone());
            }
            added = true;
        }
        if !added {
            return;
        }
        let cm = CrateMods { root: old.root.clone(), src: old.src.clone(), mods, uses, externs: old.externs.clone(), renames, scope: Default::default() };
        self.add_crate(k.to_string(), std::sync::Arc::new(cm));
        // a crate not loaded yet whose scope a parse ahead of its load computed (preloaded files'
        // lookups) copied this crate's names as they were: computed afresh on next use
        let stale: Vec<String> = self.crates.iter().filter(|(n, c)| !self.sealed.contains(n.as_str()) && c.scope.get().is_some()).map(|(n, _)| n.clone()).collect();
        for n in stale {
            let c = self.crates[&n].clone();
            let fresh = CrateMods { root: c.root.clone(), src: c.src.clone(), mods: c.mods.clone(), uses: c.uses.clone(), externs: c.externs.clone(), renames: c.renames.clone(), scope: Default::default() };
            self.crates.insert(n, std::sync::Arc::new(fresh));
        }
    }
}

/// The bind name under which a module's expansion-made glob imports are recorded (never an
/// identifier), and the separator between their paths.
pub const GLOB: &str = "*";
const GLOB_SEP: &str = "\u{0}";

fn push_glob(v: &mut Vec<String>, path: Vec<String>) {
    if split_globs(v).contains(&path) {
        return;
    }
    if !v.is_empty() {
        v.push(GLOB_SEP.to_string());
    }
    v.extend(path);
}

fn split_globs(v: &[String]) -> Vec<Vec<String>> {
    if v.is_empty() {
        return Vec::new();
    }
    v.split(|s| s == GLOB_SEP).map(|p| p.to_vec()).collect()
}

/// The definition `path::name!` names in module `module` of crate `cur`, seeing `table` and the
/// parse's own definitions so far (`local`), as rustc_resolve (ident.rs visit_scopes, MacroNS):
/// unqualified, the textual macro_rules scope (the crate's own definitions, latest first), then
/// the module's non-glob imports, then its glob imports, then the macro_use prelude
/// (`#[macro_use] extern crate`), then the std prelude. A path resolves its module part with
/// the same import resolution (`crate::m!` after `pub use crate::dep::*`); `#[macro_export]`
/// macros are items of their crate's root. Nothing depends on which other crates the program
/// loaded: only this crate's imports and what they name.
pub static SCOPE_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn lookup_in<S: AsRef<str>>(table: &MacroTable, local: &[MacroRef], name: &str, cur: &str, path: &[S], module: &str) -> Option<MacroRef> {
    lookup_in_t(table, local, name, cur, path, module)
}

fn lookup_in_t<S: AsRef<str>>(table: &MacroTable, local: &[MacroRef], name: &str, cur: &str, path: &[S], module: &str) -> Option<MacroRef> {
    // a name no macro_rules has and no import renames to (Relax's built-in macros: format!,
    // vec!, write! -- the parser takes those): nothing to resolve
    if !table.defs.contains_key(name) && !local.iter().any(|d| d.name == name) && !table.renames.contains(name) {
        return None;
    }
    let r = Res { table };
    let all = || {
        let base = table.defs.get(name).map_or(&[][..], |v| &v[..]);
        base.iter().chain(local.iter().filter(|d| d.name == name)).rev()
    };
    if !path.is_empty() {
        let (k, m) = r.resolve_mod(cur, module, path)?;
        if k == cur && table.crates.get(k).is_some_and(|c| c.root == m) {
            if let Some(d) = local.iter().rev().find(|d| d.exported && d.krate == cur && d.name == name) {
                return Some(d.clone());
            }
        }
        if let Some(d) = r.macro_in(k, m, name) {
            return Some(d);
        }
        // Relax's std does not mirror std's module layout for its macros (`std::mem::offset_of!`
        // is core's root macro re-exported there): a std path names the std family's macro
        if matches!(k, "std" | "core" | "alloc") {
            return r.std_prelude(name);
        }
        return None;
    }
    // textual scope: the crate's own macro_rules
    if let Some(d) = all().find(|d| d.krate == cur) {
        return Some(d.clone());
    }
    // the module's imports (non-glob, then glob)
    if let Some(d) = r.macro_in(cur, module, name) {
        return Some(d);
    }
    if let Some(us) = table.macro_use.get(cur) {
        for u in us.iter().rev() {
            if let Some(d) = r.exported(u, name) {
                return Some(d);
            }
        }
    }
    r.std_prelude(name)
}

struct Res<'a> {
    table: &'a MacroTable,
}

/// The (crate, module) pairs one `type_in` query has visited: a query visits a handful, so they
/// sit on the stack and compare by content; a query that walks a large glob web spills to a hash
/// set. Nothing is allocated or copied per lookup (the strings are borrowed from the scopes).
struct Seen<'b> {
    near: [(&'b str, &'b str); 16],
    n: usize,
    spill: Option<crate::fxhash::HashSet<(&'b str, &'b str)>>,
}

impl<'b> Seen<'b> {
    fn new() -> Self {
        Seen { near: [("", ""); 16], n: 0, spill: None }
    }

    /// Adds (k, m); false when it was there already.
    fn insert(&mut self, k: &'b str, m: &'b str) -> bool {
        if let Some(s) = &mut self.spill {
            return s.insert((k, m));
        }
        if self.near[..self.n].iter().any(|&(a, b)| a == k && b == m) {
            return false;
        }
        if self.n < self.near.len() {
            self.near[self.n] = (k, m);
            self.n += 1;
        } else {
            let mut s: crate::fxhash::HashSet<(&'b str, &'b str)> = self.near.iter().copied().collect();
            s.insert((k, m));
            self.spill = Some(s);
        }
        true
    }

    /// Every (crate, module) visited, in no particular order.
    fn each(&self, mut f: impl FnMut(&'b str, &'b str)) {
        match &self.spill {
            Some(s) => s.iter().for_each(|&(k, m)| f(k, m)),
            None => self.near[..self.n].iter().for_each(|&(k, m)| f(k, m)),
        }
    }
}

impl<'a> Res<'a> {
    /// `#[macro_export] macro_rules! name` of crate `k` (an item of its root).
    fn exported(&self, k: &str, name: &str) -> Option<MacroRef> {
        self.table.defs.get(name)?.iter().rev().find(|d| d.krate == krate_name(k) && d.exported).cloned()
    }

    fn std_prelude(&self, name: &str) -> Option<MacroRef> {
        ["std", "core", "alloc"].iter().find_map(|k| self.exported(k, name))
    }

    fn crate_root<'b>(&self, k: &'b str) -> (&'b str, &'b str)
    where
        'a: 'b,
    {
        (k, self.table.crates.get(k).map_or(k, |c| c.root.as_str()))
    }

    /// An extern-prelude name of crate `k` (a dependency, or std / core / alloc).
    fn extern_crate<'b>(&self, k: &str, n: &'b str) -> Option<(&'b str, &'b str)>
    where
        'a: 'b,
    {
        if let Some(c) = self.table.crates.get(k).and_then(|c| c.externs.get(n)) {
            // a bin naming its own package's lib (same crate name): the lib, not itself
            if c == k {
                let lib = format!("{}\u{1}lib", c);
                if let Some((lib, _)) = self.table.crates.get_key_value(&lib) {
                    return Some(self.crate_root(lib));
                }
            }
            return Some(self.crate_root(c));
        }
        if matches!(n, "std" | "core" | "alloc") {
            return Some(self.crate_root(n));
        }
        None
    }

    /// Crate `k`'s scopes (computed on first use; its dependencies' first).
    fn scope(&self, k: &str) -> Option<&'a CrateScope> {
        let cm = self.table.crates.get(k)?;
        Some(cm.scope.get_or_init(|| {
            let t = std::time::Instant::now();
            let sc = CrateScope::compute(self.table, k, cm);
            SCOPE_NS.fetch_add(t.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            if crate::preload::trace() {
                let n: usize = sc.mnon.values().map(|m| m.len()).sum::<usize>() + sc.mglob.values().map(|m| m.len()).sum::<usize>() + sc.tnon.values().map(|m| m.len()).sum::<usize>() + sc.tglobs.values().map(|m| m.len()).sum::<usize>();
                eprintln!("boot-trace-mscope: {} {:.2} ms, {} entries", k, t.elapsed().as_secs_f64() * 1e3, n);
            }
            sc
        }))
    }

    /// The module a name denotes in module `m` of crate `k`.
    fn type_in<'b>(&self, k: &'b str, m: &'b str, n: &str) -> Option<(&'b str, &'b str)>
    where
        'a: 'b,
    {
        let binding = |at: &Option<(String, String)>| {
            let (c, at) = at.as_ref()?;
            let t = self.table.crates.get(c.as_str())?.scope.get()?.tnon.get(at.as_str())?.get(n)?;
            Some((t.0.as_str(), t.1.as_str()))
        };
        if let Some(at) = self.table.type_snapshot.0.read().unwrap().get(k).and_then(|c| c.get(m)).and_then(|t| t.get(n)) {
            return binding(at);
        }
        let remember = |at| {
            self.table.type_snapshot.0.write().unwrap().entry(k.to_string()).or_default()
                .entry(m.to_string()).or_default().insert(n.to_string(), at);
        };
        let sc = self.scope(k)?;
        let scope_id = |c: &str| self.table.crates.get(c).and_then(|cm| cm.scope.get()).map_or(0, |s| s.id);
        if let Some(e) = sc.memo.read().unwrap().get(m).and_then(|t| t.get(n)) {
            if e.deps.iter().all(|(c, id)| scope_id(c) == *id) {
                remember(e.at.clone());
                return binding(&e.at);
            }
        }
        let mut seen = Seen::new();
        let found = self.glob_find(k, m, n, &mut seen, None);
        let mut deps: Vec<(String, u64)> = Vec::new();
        seen.each(|c, _| {
            if !deps.iter().any(|d| d.0 == c) {
                deps.push((c.to_string(), scope_id(c)));
            }
        });
        let at = found.map(|(c, at, _)| (c.to_string(), at.to_string()));
        remember(at.clone());
        sc.memo.write().unwrap().entry(m.to_string()).or_default().insert(n.to_string(), TypeInMemo { at, deps: deps.into_boxed_slice() });
        found.map(|(_, _, t)| (t.0.as_str(), t.1.as_str()))
    }

    /// `type_in` through glob sources (each module once per query); `partial`: the crate whose
    /// scope is being computed and its tables so far.
    fn type_in_seen<'b>(&self, k: &'b str, m: &'b str, n: &str, seen: &mut Seen<'b>, partial: Option<(&str, &'b CrateScope)>) -> Option<(&'b str, &'b str)>
    where
        'a: 'b,
    {
        self.glob_find(k, m, n, seen, partial).map(|(_, _, t)| (t.0.as_str(), t.1.as_str()))
    }

    /// `type_in_seen`, with the (crate, module) whose non-glob bindings had the name.
    fn glob_find<'b>(&self, k: &'b str, m: &'b str, n: &str, seen: &mut Seen<'b>, partial: Option<(&str, &'b CrateScope)>) -> Option<(&'b str, &'b str, &'b (String, String))>
    where
        'a: 'b,
    {
        if !seen.insert(k, m) {
            return None;
        }
        let sc: &'b CrateScope = match partial {
            Some((pk, psc)) if pk == k => psc,
            _ => self.scope(k)?,
        };
        if let Some(t) = sc.tnon.get(m).and_then(|t| t.get(n)) {
            return Some((k, m, t));
        }
        for (k2, m2) in sc.tglobs.get(m).map_or(&[][..], |v| &v[..]) {
            if let Some(t) = self.glob_find(k2, m2, n, seen, partial) {
                return Some(t);
            }
        }
        None
    }

    /// The macro a name denotes in module `m` of crate `k`.
    fn macro_in(&self, k: &str, m: &str, n: &str) -> Option<MacroRef> {
        let sc = self.scope(k)?;
        sc.mnon.get(m).and_then(|t| t.get(n)).or_else(|| sc.mglob.get(m).and_then(|t| t.get(n))).cloned()
    }

    /// The module path `path` names from module `m` of crate `k` (type namespace), with
    /// `type_in` answering for the crate's own modules.
    fn resolve_mod<'b, S: AsRef<str>>(&self, k: &'b str, m: &'b str, path: &'b [S]) -> Option<(&'b str, &'b str)>
    where
        'a: 'b,
    {
        resolve_path(self, k, m, path, &|kk, mm, n| self.type_in(kk, mm, n))
    }
}

/// The module `path` names from module `m` of crate `k`; `type_in` gives the module a name
/// denotes in a module (the finished scopes, or the ones being computed).
fn resolve_path<'b, S: AsRef<str>>(r: &Res<'b>, k: &'b str, m: &'b str, path: &'b [S], type_in: &dyn Fn(&'b str, &'b str, &'b str) -> Option<(&'b str, &'b str)>) -> Option<(&'b str, &'b str)> {
    let s0 = path.first()?.as_ref();
    let (mut cur, rest) = if s0 == "crate" {
        (r.crate_root(k), &path[1..])
    } else if s0 == "self" {
        ((k, m), &path[1..])
    } else if s0 == "super" {
        ((k, m.rsplit_once("::").map(|x| x.0)?), &path[1..])
    } else if let Some(c) = s0.strip_prefix("__relax_crate_") {
        (r.crate_root(c), &path[1..])
    } else if s0.is_empty() {
        // `::dep` (2018: the extern prelude)
        (r.extern_crate(k, path.get(1)?.as_ref())?, &path[2.min(path.len())..])
    } else {
        let first = match type_in(k, m, s0) {
            Some(x) => x,
            None => r.extern_crate(k, s0)?,
        };
        (first, &path[1..])
    };
    for s in rest {
        cur = match s.as_ref() {
            "super" => (cur.0, cur.1.rsplit_once("::").map(|x| x.0)?),
            "self" => cur,
            _ => type_in(cur.0, cur.1, s.as_ref())?,
        };
    }
    Some(cur)
}

impl CrateScope {
    /// Resolves crate `k`'s imports to a fixpoint (globs only add names; non-glob bindings win
    /// over glob ones), seeing its dependencies' finished scopes.
    fn compute(table: &MacroTable, k: &str, cm: &CrateMods) -> CrateScope {
        let r = Res { table };
        let t0 = std::time::Instant::now();
        let mut sc = CrateScope { id: SCOPE_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed), ..CrateScope::default() };
        let mut mods: Vec<String> = cm.mods.iter().cloned().collect();
        for m in cm.uses.keys() {
            if !cm.mods.contains(m) {
                mods.push(m.clone());
            }
        }
        mods.sort();
        mods.dedup();
        // child modules
        for m in &mods {
            if let Some((parent, name)) = m.rsplit_once("::") {
                sc.tnon.entry(parent.to_string()).or_default().insert(name.to_string(), (k.to_string(), m.clone()));
            }
        }
        // the root's `#[macro_export]` macros (the latest definition of each name)
        let mut exported: HashMap<String, MacroRef> = HashMap::default();
        for (n, ds) in &table.defs {
            if let Some(d) = ds.iter().rev().find(|d| d.krate == krate_name(k) && d.exported) {
                exported.insert(n.clone(), d.clone());
            }
        }
        sc.mnon.insert(cm.root.clone(), exported);
        let t_exp = t0.elapsed().as_secs_f64() * 1e3;
        let mut passes = 0;
        let mut copies = 0usize;
        let empty: Vec<UseItem> = Vec::new();
        // per glob import: how many names its source had when last copied (a source that has
        // not grown has nothing new to give)
        let mut copied: HashMap<(String, usize), usize> = HashMap::default();
        // imports whose answer is final (found, or resolved into another crate's finished scope)
        let mut settled: crate::fxhash::HashSet<(String, usize)> = crate::fxhash::HashSet::default();
        loop {
            let mut changed = false;
            passes += 1;
            for m in &mods {
                for (ui, u) in cm.uses.get(m).unwrap_or(&empty).iter().enumerate() {
                    if settled.contains(&(m.clone(), ui)) {
                        continue;
                    }
                    match &u.bind {
                        Some(b) => {
                            let last = u.path.last().map_or("", |x| x.as_str());
                            // only a name some module / crate has can bind one; only a macro name
                            // (or a rename) can bind a macro
                            let may_type = table.mod_names.contains(last) || last == "self" || last == "super" || last == "crate";
                            let may_macro = table.defs.contains_key(last) || table.renames.contains(last);
                            if !may_type && !may_macro {
                                settled.insert((m.clone(), ui));
                                continue;
                            }
                            // where the path's prefix leads: another crate's module is final
                            let prefix_crate = match u.path.split_last() {
                                Some((_, prefix)) if !prefix.is_empty() && !(prefix.len() == 1 && prefix[0].is_empty()) => {
                                    resolve_path(&r, k, m, prefix, &|kk, mm, n| sc.lookup_type(&r, k, kk, mm, n)).map(|x| x.0)
                                }
                                _ => None,
                            };
                            if prefix_crate.map_or(false, |c| c != k) {
                                settled.insert((m.clone(), ui));
                            }
                            // a module / crate under that name
                            if may_type && !sc.tnon.get(m).map_or(false, |t| t.contains_key(b)) {
                                let t = resolve_path(&r, k, m, &u.path, &|kk, mm, n| sc.lookup_type(&r, k, kk, mm, n)).map(|(a, b)| (a.to_string(), b.to_string()));
                                if let Some(t) = t {
                                    sc.tnon.entry(m.clone()).or_default().insert(b.clone(), t);
                                    changed = true;
                                }
                            }
                            // a macro under that name
                            if may_macro && !sc.mnon.get(m).map_or(false, |t| t.contains_key(b)) {
                                let d = match u.path.split_last() {
                                    Some((last, prefix)) if prefix.is_empty() || (prefix.len() == 1 && prefix[0].is_empty()) => {
                                        // `use name;` of a macro in scope (2018 `pub use mac;`)
                                        table.defs.get(last).and_then(|ds| ds.iter().rev().find(|d| d.krate == krate_name(k)).cloned())
                                    }
                                    Some((last, prefix)) => {
                                        let t = resolve_path(&r, k, m, prefix, &|kk, mm, n| sc.lookup_type(&r, k, kk, mm, n));
                                        t.and_then(|(k2, m2)| sc.lookup_macro(&r, k, k2, m2, last))
                                    }
                                    None => None,
                                };
                                if let Some(d) = d {
                                    sc.mnon.entry(m.clone()).or_default().insert(b.clone(), d);
                                    changed = true;
                                }
                            }
                        }
                        None => {
                            let t = resolve_path(&r, k, m, &u.path, &|kk, mm, n| sc.lookup_type(&r, k, kk, mm, n)).map(|(a, b)| (a.to_string(), b.to_string()));
                            let Some((k2, m2)) = t else { continue };
                            if (k2.as_str(), m2.as_str()) == (k, m.as_str()) {
                                continue;
                            }
                            // another crate's module: final, copied once
                            if k2 != k {
                                settled.insert((m.clone(), ui));
                            }
                            let size = sc.size_of(&r, k, &k2, &m2);
                            if copied.get(&(m.clone(), ui)) == Some(&size) {
                                continue;
                            }
                            copied.insert((m.clone(), ui), size);
                            // the source's modules: searched through it on demand
                            let srcs = sc.tglobs.entry(m.clone()).or_default();
                            if !srcs.iter().any(|x| x.0 == k2 && x.1 == m2) {
                                srcs.push((k2.clone(), m2.clone()));
                                changed = true;
                            }
                            // its macros, under the names it has
                            let macros = sc.all_macros_of(&r, k, &k2, &m2);
                            copies += macros.len();
                            for (n, v) in macros {
                                let has = sc.mnon.get(m).map_or(false, |t| t.contains_key(&n)) || sc.mglob.get(m).map_or(false, |t| t.contains_key(&n));
                                if !has {
                                    sc.mglob.entry(m.clone()).or_default().insert(n, v);
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        if crate::preload::trace() {
            eprintln!("boot-trace-mscope-detail: {} exported {:.2} ms, {} passes, {} copied, {} mods, {} uses", k, t_exp, passes, copies, mods.len(), cm.uses.values().map(|v| v.len()).sum::<usize>());
        }
        sc
    }

    /// The module a name denotes in module `mm` of crate `kk`: this crate's partial tables, a
    /// dependency's finished ones.
    fn lookup_type<'b>(&'b self, r: &Res<'b>, k: &str, kk: &'b str, mm: &'b str, n: &str) -> Option<(&'b str, &'b str)> {
        r.type_in_seen(kk, mm, n, &mut Seen::new(), Some((k, self)))
    }

    fn lookup_macro(&self, r: &Res, k: &str, kk: &str, mm: &str, n: &str) -> Option<MacroRef> {
        if kk == k {
            return self.mnon.get(mm).and_then(|t| t.get(n)).or_else(|| self.mglob.get(mm).and_then(|t| t.get(n))).cloned();
        }
        r.macro_in(kk, mm, n)
    }

    /// How many names module `mm` of crate `kk` has so far.
    fn size_of(&self, r: &Res, k: &str, kk: &str, mm: &str) -> usize {
        let sc: &CrateScope = if kk == k {
            self
        } else {
            match r.scope(kk) {
                Some(s) => s,
                None => return 0,
            }
        };
        sc.mnon.get(mm).map_or(0, |t| t.len()) + sc.mglob.get(mm).map_or(0, |t| t.len())
    }

    /// Every macro name module `mm` of crate `kk` has (for a glob import).
    fn all_macros_of(&self, r: &Res, k: &str, kk: &str, mm: &str) -> Vec<(String, MacroRef)> {
        let sc: &CrateScope = if kk == k {
            self
        } else {
            match r.scope(kk) {
                Some(s) => s,
                None => return Vec::new(),
            }
        };
        let mut macros = Vec::new();
        for t in [sc.mnon.get(mm), sc.mglob.get(mm)].into_iter().flatten() {
            for (n, v) in t {
                macros.push((n.clone(), v.clone()));
            }
        }
        macros
    }
}

/// The macros one parse sees: the table as the parse started (shared, read-only; files of a
/// crate parse in parallel) followed by the definitions the file made so far.
#[derive(Clone)]
pub struct MacroScope {
    pub table: std::sync::Arc<MacroTable>,
    pub local: Vec<MacroRef>,
    looked: std::cell::RefCell<Vec<Looked>>,
    /// `use` bindings this parse made (module, bound name, path; GLOB first = `::`): the crate's
    /// scopes come from the macro prescan of its files, which never sees an import a macro
    /// expansion wrote (derive output: `use makepad_draw::rr as __rt; .. __rt::m!(..)`)
    local_uses: std::cell::RefCell<std::sync::Arc<LocalImports>>,
    /// Initial bindings, then each changed entry's value after the write. Lookups
    /// record a journal position, so interleaved imports do not copy the whole map.
    uses_base: std::sync::Arc<LocalImports>,
    uses_log: std::cell::RefCell<Vec<((Name, Name), Vec<Name>)>>,
    memo: std::cell::RefCell<LookupMemo>,
    forwarders: std::cell::RefCell<HashMap<(String, String, String), MacroRef>>,
}

/// Deferred contexts share these bindings; individual lookups only retain a
/// journal position. Build the index lazily, then extend it as bindings are added,
/// including during replay, without enumerating the whole map after every write.
/// Names are ids of the owning table's `MacroNames`.
#[derive(Default)]
struct LocalImports {
    bindings: HashMap<(Name, Name), Vec<Name>>,
    index: std::sync::OnceLock<LocalImportIndex>,
}

/// The binding names and modules of a table's expansion-made imports of one crate.
#[derive(Default, Clone)]
struct ExpansionIndex {
    names: crate::fxhash::HashSet<String>,
    modules: crate::fxhash::HashSet<String>,
}

#[derive(Default, Clone)]
struct LocalImportIndex {
    names: crate::fxhash::HashSet<Name>,
    modules: crate::fxhash::HashSet<Name>,
}

impl Clone for LocalImports {
    fn clone(&self) -> Self { Self { bindings: self.bindings.clone(), index: Default::default() } }
}

impl LocalImports {
    fn index(&self, names: &MacroNames) -> &LocalImportIndex {
        self.index.get_or_init(|| {
            let mut modules = crate::fxhash::HashSet::default();
            for (module, _) in self.bindings.keys() { add_import_module_id(&mut modules, names, *module); }
            LocalImportIndex { names: self.bindings.keys().map(|&(_, b)| b).collect(), modules }
        })
    }
    fn entry(&mut self, names: &MacroNames, key: (Name, Name)) -> &mut Vec<Name> {
        if let Some(index) = self.index.get_mut() {
            index.names.insert(key.1);
            add_import_module_id(&mut index.modules, names, key.0);
        }
        self.bindings.entry(key).or_default()
    }
    /// The same bindings under another lineage's ids.
    fn remap(&self, from: &MacroNames, to: &MacroNames) -> Self {
        let tr = |n: Name| to.id(from.str(n));
        Self {
            bindings: self.bindings.iter().map(|(&(m, b), p)| ((tr(m), tr(b)), p.iter().map(|&x| tr(x)).collect())).collect(),
            index: Default::default(),
        }
    }
}

/// The paths of a module's glob imports, one entry (separated by GLOB_SEP_ID).
fn push_glob_ids(v: &mut Vec<Name>, path: Vec<Name>) {
    if !v.is_empty() && v.split(|&s| s == GLOB_SEP_ID).any(|p| p == path) {
        return;
    }
    if !v.is_empty() {
        v.push(GLOB_SEP_ID);
    }
    v.extend(path);
}

/// `paths` (a GLOB entry) joins `out`, each path once.
fn add_globs_ids(out: &mut Vec<Vec<Name>>, paths: &[Name]) {
    if paths.is_empty() { return; }
    for path in paths.split(|&s| s == GLOB_SEP_ID) {
        if !out.iter().any(|o| o == path) { out.push(path.to_vec()); }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct LookupKey {
    module: Name,
    path: Box<[Name]>,
    name: Name,
}

/// One immutable local environment on top of a publication. Both successful and
/// failed queries are memoized. Only top-level answers are recorded for replay,
/// once per query/environment; the import traversal is never a replay ledger.
#[derive(Clone, Default)]
struct LookupMemo {
    origin: Name,
    queries: HashMap<LookupKey, Option<MacroRef>>,
    imports: HashMap<ImportKey, ImportAnswer>,
}

/// The immutable macro environment of a deferred body, safe to send to a worker.
#[derive(Clone)]
pub struct MacroContext {
    table: std::sync::Arc<MacroTable>,
    local: Vec<MacroRef>,
    uses: std::sync::Arc<LocalImports>,
}
impl MacroContext {
    pub fn set_table(&mut self, table: std::sync::Arc<MacroTable>) {
        // the bindings' ids belong to the old table's names
        if !std::sync::Arc::ptr_eq(&self.table.names, &table.names) {
            self.uses = std::sync::Arc::new(self.uses.remap(&self.table.names, &table.names));
        }
        self.table = table;
    }
}

/// One lookup of a parse: what it asked, how many own definitions existed then, what it got.
#[derive(Clone)]
pub struct Looked {
    name: Name,
    path: Box<[Name]>,
    module: Name,
    nlocal: u32,
    /// The bindings it saw: uses_base and the first nuses entries of uses_log.
    nuses: u32,
    got: Option<MacroRef>,
    inputs: std::sync::Arc<LookupInputs>,
}

/// A finished parse's definitions (to append to the table in load order) and its lookups.
#[derive(Clone)]
pub struct MacroUse {
    trusted_image: bool,
    pub local: Vec<MacroRef>,
    looked: Vec<Looked>,
    uses_base: std::sync::Arc<LocalImports>,
    uses_log: Vec<((Name, Name), Vec<Name>)>,
    /// the interner `looked`, `uses_base` and `uses_log` are in (None: an image's, nothing to replay)
    names: Option<std::sync::Arc<MacroNames>>,
    /// the parse's `use` bindings (MacroScope::add_use), for the crate's table
    pub uses: crate::fxhash::HashMap<(String, String), Vec<String>>,
}

fn ptr_of(d: &Option<MacroRef>) -> usize {
    d.as_ref().map_or(0, |d| std::sync::Arc::as_ptr(d) as usize)
}

impl MacroScope {
    pub fn context(&self) -> MacroContext {
        MacroContext { table: self.table.clone(), local: self.local.clone(), uses: self.local_uses.borrow().clone() }
    }

    pub fn from_context(ctx: &MacroContext) -> Self {
        Self { table: ctx.table.clone(), local: ctx.local.clone(), local_uses: std::cell::RefCell::new(ctx.uses.clone()), looked: Default::default(), uses_base: ctx.uses.clone(), uses_log: Default::default(), memo: Default::default(), forwarders: Default::default() }
    }

    pub fn new(table: std::sync::Arc<MacroTable>) -> MacroScope {
        MacroScope { table, local: Vec::new(), looked: Default::default(), local_uses: Default::default(), uses_base: Default::default(), uses_log: Default::default(), memo: Default::default(), forwarders: Default::default() }
    }

    pub(crate) fn define_tokens(&mut self, name: &str, krate: &str, file: &str, pos: u32, exported: bool, local_inner: bool, tokens: &[MTok]) -> Result<(), String> {
        let same = self.table.defs.get(name).into_iter().flatten().rev().chain(self.local.iter().rev()).find(|d| {
            d.name == name && d.krate == krate && d.def_file == file && d.def_pos == pos && d.exported == exported && d.builtin == 0
                && d.input.as_ref().is_some_and(|input| input.local_inner == local_inner && input.tokens == tokens)
        }).cloned();
        if let Some(d) = same {
            self.local.push(d);
            *self.memo.get_mut() = LookupMemo::default();
        } else {
            let mut d = parse_def(name, krate, tokens)?;
            d.exported = exported;
            if local_inner { local_inner_macros(&mut d); }
            d.def_file = file.into();
            d.def_pos = pos;
            self.define(d);
        }
        Ok(())
    }

    pub(crate) fn forwarding_def(&self, name: &str, krate: &str, target: &str) -> Option<MacroRef> {
        let key = (name.into(), krate.into(), target.into());
        let mut cache = self.forwarders.borrow_mut();
        if let Some(d) = cache.get(&key) { return Some(d.clone()); }
        let d = forwarding_def(name, krate, target)?;
        cache.insert(key, d.clone());
        Some(d)
    }

    /// A `macro_rules!` the file defines. The prescan registered most definitions already;
    /// one equal to a registered one is that same definition (its identity is what `replays`
    /// compares).
    pub fn define(&mut self, d: MacroDef) {
        let same = self.table.defs.get(&d.name).and_then(|v| {
            v.iter().rev().find(|x| x.krate == d.krate && x.exported == d.exported && x.builtin == d.builtin && x.arms == d.arms && x.def_file == d.def_file && x.def_pos == d.def_pos).cloned()
        });
        self.local.push(same.unwrap_or_else(|| std::sync::Arc::new(d)));
        *self.memo.get_mut() = LookupMemo::default();
    }

    /// `use path as bind;` (or `use path;`) parsed in module `module`.
    pub fn add_use(&self, module: &str, bind: String, path: Vec<String>) {
        let names = &*self.table.names;
        let key = (names.id(module), names.id(&bind));
        let path: Vec<Name> = path.iter().map(|p| names.id(p)).collect();
        if self.local_uses.borrow().bindings.get(&key) == Some(&path) { return; }
        self.uses_log.borrow_mut().push((key, path.clone()));
        *std::sync::Arc::make_mut(&mut self.local_uses.borrow_mut()).entry(names, key) = path;
        *self.memo.borrow_mut() = LookupMemo::default();
    }

    /// `use path::*;` parsed in module `module`: kept under the bind name GLOB, the paths of all
    /// of the module's glob imports in one entry (separated by GLOB_SEP).
    pub fn add_glob_use(&self, module: &str, path: Vec<String>) {
        let names = &*self.table.names;
        let key = (names.id(module), GLOB_ID);
        let path: Vec<Name> = path.iter().map(|p| names.id(p)).collect();
        if self.local_uses.borrow().bindings.get(&key).is_some_and(|v| v.split(|&s| s == GLOB_SEP_ID).any(|p| p == path)) { return; }
        let mut uses = self.local_uses.borrow_mut();
        let v = std::sync::Arc::make_mut(&mut uses).entry(names, key);
        push_glob_ids(v, path);
        self.uses_log.borrow_mut().push((key, v.clone()));
        *self.memo.borrow_mut() = LookupMemo::default();
    }

    /// Resolve expansion-made imports by canonical scope, never by prepending glob paths.
    fn lookup_via_local_use(&self, name: Name, cur: Name, path: &[Name], module: Name, memo: &mut LookupMemo, inputs: &mut LookupInputs) -> Option<MacroRef> {
        let names = &*self.table.names;
        let name_s = names.str(name);
        // Keep builtin/unknown names off the import walk. An expansion can introduce a
        // rename not in the prescan's set yet, including one reached through a facade.
        if !self.table.defs.contains_key(name_s) && !self.table.renames.contains(name_s)
            && !self.local.iter().any(|d| d.name == name_s)
            && !self.local_uses.borrow().index(names).names.contains(&name) {
            inputs.read(&self.table, LookupInput::Alias(name));
            if !self.table.expansion_alias(name_s) { return None; }
        }
        ImportLookup {
            scope: self, origin: cur, memo: &mut memo.imports, inputs,
            states: Vec::new(), seen: Default::default(), ready: Default::default(),
        }.lookup(name, cur, path, module)
    }

    fn resolve(&self, name: Name, cur: Name, path: &[Name], module: Name, record: bool) -> Option<MacroRef> {
        let mut memo = self.memo.borrow_mut();
        if memo.origin != cur { *memo = LookupMemo { origin: cur, ..Default::default() }; }
        let key = LookupKey { module, path: path.into(), name };
        if let Some(r) = memo.queries.get(&key) { return r.clone(); }
        let names = &*self.table.names;
        let mut inputs = LookupInputs::new(&self.table);
        inputs.read(&self.table, LookupInput::Definitions(name));
        if path.is_empty() { inputs.read(&self.table, LookupInput::Prelude(cur)); }
        let r = with_strs(names, path, |path| lookup_in(&self.table, &self.local, names.str(name), names.str(cur), path, names.str(module)))
            .or_else(|| self.lookup_via_local_use(name, cur, path, module, &mut memo, &mut inputs));
        if record {
            self.looked.borrow_mut().push(Looked { name, path: key.path.clone(), module, nlocal: self.local.len() as u32, nuses: self.uses_log.borrow().len() as u32, got: r.clone(), inputs: std::sync::Arc::new(inputs) });
        }
        memo.queries.insert(key, r.clone());
        r
    }

    pub fn lookup(&self, name: &str, cur: &str, path: &[String], module: &str) -> Option<MacroRef> {
        let names = &*self.table.names;
        let path: Box<[Name]> = path.iter().map(|p| names.id(p)).collect();
        self.resolve(names.id(name), names.id(cur), &path, names.id(module), true)
    }

    pub fn finish(self) -> MacroUse {
        let names = self.table.names.clone();
        let uses = std::sync::Arc::unwrap_or_clone(self.local_uses.into_inner()).bindings.into_iter()
            .map(|((m, b), p)| ((names.str(m).to_string(), names.str(b).to_string()), p.iter().map(|&x| names.str(x).to_string()).collect()))
            .collect();
        MacroUse { trusted_image: false, local: self.local, looked: self.looked.into_inner(), uses_base: self.uses_base, uses_log: self.uses_log.into_inner(), names: Some(names), uses }
    }
}

/// One name in one actual module, separately for the module and macro namespaces. Import
/// paths are consumed segment by segment; neither aliases nor globs manufacture longer paths.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum ImportNs { Module, Macro }

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ImportKey {
    ns: ImportNs,
    krate: Name,
    module: Name,
    name: Name,
}

#[derive(Clone)]
enum ImportValue {
    Module(Name, Name),
    Macro(MacroRef),
}

struct ImportState {
    key: ImportKey,
    value: Option<ImportValue>,
    waiting: Vec<usize>,
    local: bool,
}

#[derive(Clone)]
struct ImportAnswer {
    value: Option<ImportValue>,
    local: bool,
    inputs: std::sync::Arc<LookupInputs>,
}

/// Demand-driven least fixpoint for imports missing from the prescan. `seen` interns every
/// (namespace, crate, module, name) once. A cycle subscribes to the existing state and adds no
/// binding; a later answer wakes its dependants, including those on the other side of a cycle.
/// Memoized states belong to the exact publication/local-import snapshot. Publish
/// them only after draining the fixpoint: a pending cycle is not a cached miss.
struct ImportLookup<'a> {
    scope: &'a MacroScope,
    origin: Name,
    memo: &'a mut HashMap<ImportKey, ImportAnswer>,
    inputs: &'a mut LookupInputs,
    states: Vec<ImportState>,
    seen: HashMap<ImportKey, usize>,
    ready: std::collections::VecDeque<usize>,
}

impl ImportLookup<'_> {
    fn ask(&mut self, ns: ImportNs, k: Name, m: Name, n: Name, parent: Option<usize>) -> Option<ImportValue> {
        let key = ImportKey { ns, krate: k, module: m, name: n };
        if let Some(answer) = self.memo.get(&key) {
            self.inputs.depend(&answer.inputs);
            if answer.local { if let Some(parent) = parent { self.states[parent].local = true; } }
            return answer.value.clone();
        }
        if let Some(answer) = self.scope.table.import_snapshot.0.read().unwrap().get(&self.origin).and_then(|memo| memo.get(&key)) {
            self.inputs.depend(&answer.inputs);
            return answer.value.clone();
        }
        let i = match self.seen.get(&key) {
            Some(&i) => i,
            None => {
                let i = self.states.len();
                self.seen.insert(key, i);
                self.states.push(ImportState { key, value: None, waiting: Vec::new(), local: k == self.origin });
                self.ready.push_back(i);
                i
            }
        };
        let state = &mut self.states[i];
        if let Some(parent) = parent {
            if !state.waiting.contains(&parent) { state.waiting.push(parent); }
        }
        state.value.clone()
    }

    fn module_name(&mut self, k: Name, m: Name, n: Name, parent: Option<usize>) -> Option<(Name, Name)> {
        match self.ask(ImportNs::Module, k, m, n, parent)? {
            ImportValue::Module(k, m) => Some((k, m)),
            ImportValue::Macro(_) => unreachable!(),
        }
    }

    fn path(&mut self, k: Name, m: Name, path: &[Name], parent: Option<usize>) -> Option<(Name, Name)> {
        if path.is_empty() { return Some((k, m)); }
        let scope = self.scope;
        let names = &*scope.table.names;
        let r = Res { table: &scope.table };
        let ids = |(k, m): (&str, &str)| (names.id(k), names.id(m));
        let (ks, ms) = (names.str(k), names.str(m));
        if let Some(at) = with_strs(names, path, |ps| r.resolve_mod(ks, ms, ps).map(ids)) { return Some(at); }
        let first = names.str(path[0]);
        let (mut at, rest) = match first {
            "crate" => (ids(r.crate_root(ks)), &path[1..]),
            "self" => ((k, m), &path[1..]),
            "super" => ((k, names.id(ms.rsplit_once("::")?.0)), &path[1..]),
            "" => (ids(r.extern_crate(ks, names.str(*path.get(1)?))?), &path[2..]),
            _ => {
                let at = if let Some(defining) = first.strip_prefix("__relax_crate_") {
                    ids(r.crate_root(defining))
                } else {
                    self.module_name(k, m, path[0], parent).or_else(|| r.extern_crate(ks, first).map(ids))?
                };
                (at, &path[1..])
            }
        };
        for &n in rest {
            at = match names.str(n) {
                "self" => at,
                "super" => (at.0, names.id(names.str(at.1).rsplit_once("::")?.0)),
                _ => self.module_name(at.0, at.1, n, parent)?,
            };
        }
        Some(at)
    }

    fn binding(&mut self, k: Name, m: Name, n: Name) -> Option<Vec<Name>> {
        let key = (m, n);
        let scope = self.scope;
        let mine = scope.local_uses.borrow();
        let table = &scope.table;
        let names = &*table.names;
        if k == self.origin { if let Some(path) = mine.bindings.get(&key) { return Some(path.clone()); } }
        self.inputs.read(table, LookupInput::Import(k, m, n));
        let (ks, ms, ns) = (names.str(k), names.str(m), names.str(n));
        let ids = |path: &Vec<String>| path.iter().map(|s| names.id(s)).collect::<Vec<Name>>();
        table.exp_uses.get(ks).and_then(|u| u.get(&(ms.to_string(), ns.to_string()))).map(ids)
            .or_else(|| table.crates.get(ks)?.uses.get(ms)?.iter().rev()
                .find(|u| u.bind.as_deref() == Some(ns)).map(|u| ids(&u.path)))
    }

    fn globs(&mut self, k: Name, m: Name) -> Vec<Vec<Name>> {
        let key = (m, GLOB_ID);
        let scope = self.scope;
        let mine = scope.local_uses.borrow();
        let table = &scope.table;
        let names = &*table.names;
        self.inputs.read(table, LookupInput::Import(k, m, GLOB_ID));
        let (ks, ms) = (names.str(k), names.str(m));
        let ids = |path: &[String]| path.iter().map(|s| names.id(s)).collect::<Vec<Name>>();
        let mut out = Vec::new();
        if k == self.origin { if let Some(paths) = mine.bindings.get(&key) { add_globs_ids(&mut out, paths); } }
        if let Some(paths) = table.exp_uses.get(ks).and_then(|u| u.get(&(ms.to_string(), GLOB.to_string()))) {
            add_globs_ids(&mut out, &ids(paths));
        }
        if let Some(uses) = table.crates.get(ks).and_then(|c| c.uses.get(ms)) {
            for u in uses.iter().filter(|u| u.bind.is_none()) {
                let path = ids(&u.path);
                if !out.contains(&path) { out.push(path); }
            }
        }
        out
    }

    /// Exactly the generated modules and their parents that seal_crate will publish. A
    /// syntactically plausible `m::n` is not a module unless an actual import owner names it.
    fn generated_module(&mut self, k: Name, m: Name) -> bool {
        let scope = self.scope;
        let names = &*scope.table.names;
        if k == self.origin && scope.local_uses.borrow().index(names).modules.contains(&m) { return true; }
        self.inputs.read(&scope.table, LookupInput::GeneratedModule(k, m));
        scope.table.expansion_module(names.str(k), names.str(m))
    }

    fn step(&mut self, i: usize) -> Option<ImportValue> {
        let ImportKey { ns, krate: k, module: m, name: n } = self.states[i].key;
        let scope = self.scope;
        let table = &scope.table;
        let names = &*table.names;
        let (ks, ms, nm) = (names.str(k), names.str(m), names.str(n));
        let r = Res { table };
        match ns {
            ImportNs::Module => {
                if let Some((k2, m2)) = r.type_in(ks, ms, nm) { return Some(ImportValue::Module(names.id(k2), names.id(m2))); }
                let child = names.id(&format!("{ms}::{nm}"));
                if self.generated_module(k, child) { return Some(ImportValue::Module(k, child)); }
            }
            ImportNs::Macro => {
                if k == self.origin && r.crate_root(ks).1 == ms {
                    if let Some(d) = scope.local.iter().rev().find(|d| d.exported && d.krate == ks && d.name == nm) {
                        return Some(ImportValue::Macro(d.clone()));
                    }
                }
                if let Some(d) = r.macro_in(ks, ms, nm) { return Some(ImportValue::Macro(d)); }
                if matches!(ks, "std" | "core" | "alloc") {
                    self.inputs.read(table, LookupInput::Definitions(n));
                    if let Some(d) = r.std_prelude(nm) { return Some(ImportValue::Macro(d)); }
                }
            }
        }
        if let Some(path) = self.binding(k, m, n) {
            return match ns {
                ImportNs::Module => self.path(k, m, &path, Some(i)).map(|(k, m)| ImportValue::Module(k, m)),
                ImportNs::Macro => {
                    let (&last, prefix) = path.split_last()?;
                    if prefix.is_empty() {
                        self.inputs.read(table, LookupInput::Definitions(last));
                        self.inputs.read(table, LookupInput::Prelude(k));
                        let local = if k == self.origin { &scope.local[..] } else { &[] };
                        if let Some(d) = lookup_in(table, local, names.str(last), ks, &[] as &[&str], ms) {
                            return Some(ImportValue::Macro(d));
                        }
                    }
                    let (k, m) = self.path(k, m, prefix, Some(i))?;
                    self.ask(ns, k, m, last, Some(i))
                }
            };
        }
        for path in self.globs(k, m) {
            if let Some((k, m)) = self.path(k, m, &path, Some(i)) {
                if let Some(value) = self.ask(ns, k, m, n, Some(i)) { return Some(value); }
            }
        }
        None
    }

    fn lookup(&mut self, name: Name, k: Name, path: &[Name], module: Name) -> Option<MacroRef> {
        let result = loop {
            if let Some((k, m)) = self.path(k, module, path, None) {
                if let Some(ImportValue::Macro(d)) = self.ask(ImportNs::Macro, k, m, name, None) {
                    if self.ready.is_empty() { break Some(d); }
                }
            }
            if self.ready.is_empty() { break None; }
            while let Some(i) = self.ready.pop_front() {
                if self.states[i].value.is_some() { continue; }
                if let Some(value) = self.step(i) {
                    self.states[i].value = Some(value);
                    self.ready.extend(self.states[i].waiting.iter().copied());
                }
            }
        };
        // Propagate local dependence through the same edges as bindings, including
        // cycles and failed queries. Negative answers need the same protection.
        let mut local: std::collections::VecDeque<_> = self.states.iter().enumerate().filter_map(|(i, s)| s.local.then_some(i)).collect();
        while let Some(i) = local.pop_front() {
            for parent in self.states[i].waiting.clone() {
                if !self.states[parent].local { self.states[parent].local = true; local.push_back(parent); }
            }
        }
        let mut shared = self.scope.table.import_snapshot.0.write().unwrap();
        let shared = shared.entry(self.origin).or_default();
        let inputs = std::sync::Arc::new(std::mem::replace(self.inputs, LookupInputs::new(&self.scope.table)));
        self.inputs.depend(&inputs);
        for state in self.states.drain(..) {
            let answer = ImportAnswer { value: state.value, local: state.local, inputs: inputs.clone() };
            if !state.local { shared.insert(state.key, answer.clone()); }
            self.memo.insert(state.key, answer);
        }
        result
    }
}

impl MacroUse {
    pub fn from_image(local: Vec<MacroRef>, uses: crate::fxhash::HashMap<(String, String), Vec<String>>) -> Self {
        Self { local, uses, looked: Vec::new(), uses_base: Default::default(), uses_log: Vec::new(), names: None, trusted_image: true }
    }

    /// Did every lookup get what it gets against `table` (the table as a sequential parse at
    /// this point in the load order sees it)? `cur`: the file's crate, `here`: its path. Another
    /// definition object counts as the same when it expands identically in this file: same
    /// crate, name, kind and rules, and neither defined in this file (only a same-file
    /// definition passes its positions on: hygiene marks, body origins). Built-ins that define
    /// macro_rules! (script_err_gen!) are found by the prescan without a position.
    pub fn replays(&self, table: &std::sync::Arc<MacroTable>, cur: &str, here: &str) -> bool {
        if self.trusted_image {
            return true;
        }
        let names = &*table.names;
        let from = self.names.as_ref().expect("a parse's names");
        // made under another table lineage: translate its ids (its certificates read the old ids)
        let foreign = !std::sync::Arc::ptr_eq(from, &table.names);
        let tr = |n: Name| if foreign { names.id(from.str(n)) } else { n };
        let mut scope = MacroScope::new(table.clone());
        *scope.local_uses.get_mut() = if foreign { std::sync::Arc::new(self.uses_base.remap(from, names)) } else { self.uses_base.clone() };
        let cur_id = names.id(cur);
        let mut applied = 0;
        self.looked.iter().all(|l| {
            if !foreign && l.inputs.unchanged(table) { return true; }
            // Reproduce both lookup paths, with the imports visible at this particular
            // call. A later import must not change the meaning of an earlier expansion.
            if scope.local.len() != l.nlocal as usize || applied != l.nuses as usize {
                // Lookups are recorded in source order: copy each local definition
                // once, not the whole prefix for every call.
                scope.local.extend_from_slice(&self.local[scope.local.len()..l.nlocal as usize]);
                // Include writes preceding certificate-skipped lookups too. Each
                // entry is applied once, and a later import never reaches back.
                if applied != l.nuses as usize {
                    let uses = std::sync::Arc::make_mut(scope.local_uses.get_mut());
                    for (key, path) in &self.uses_log[applied..l.nuses as usize] {
                        *uses.entry(names, (tr(key.0), tr(key.1))) = path.iter().map(|&p| tr(p)).collect();
                    }
                    applied = l.nuses as usize;
                }
                *scope.memo.get_mut() = LookupMemo::default();
            }
            let moved: Box<[Name]>;
            let path = if foreign { moved = l.path.iter().map(|&p| tr(p)).collect(); &moved[..] } else { &l.path[..] };
            let now = scope.resolve(tr(l.name), cur_id, path, tr(l.module), false);
            let ok = ptr_of(&now) == ptr_of(&l.got)
                || match (&now, &l.got) {
                    (Some(a), Some(b)) => a.def_file != here && b.def_file != here && same_expansion(a, b),
                    _ => false,
                };
            if !ok && crate::preload::trace() {
                let path: Vec<&str> = l.path.iter().map(|&p| from.str(p)).collect();
                eprintln!("boot-trace-replay: {} in {} path {:?} before {:?} now {:?}", from.str(l.name), here, path, l.got.as_ref().map(|d| (&d.name, &d.krate, &d.def_file, d.def_pos, d.builtin)), now.as_ref().map(|d| (&d.name, &d.krate, &d.def_file, d.def_pos, d.builtin)));
            }
            ok
        })
    }
}

/// Two definitions expand the same anywhere outside their files (token text, not positions).
fn same_expansion(a: &MacroDef, b: &MacroDef) -> bool {
    fn tok(a: &MTok, b: &MTok) -> bool {
        a.kind == b.kind && a.text == b.text
    }
    fn opt(a: &Option<MTok>, b: &Option<MTok>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(x), Some(y)) => tok(x, y),
            _ => false,
        }
    }
    fn m(a: &[MNode], b: &[MNode]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| match (x, y) {
                (MNode::Tok(p), MNode::Tok(q)) => tok(p, q),
                (MNode::Var(n1, f1), MNode::Var(n2, f2)) => n1 == n2 && f1 == f2,
                (MNode::Rep(v1, s1, o1), MNode::Rep(v2, s2, o2)) => o1 == o2 && opt(s1, s2) && m(v1, v2),
                (MNode::Group(o1, c1, v1), MNode::Group(o2, c2, v2)) => tok(o1, o2) && tok(c1, c2) && m(v1, v2),
                _ => false,
            })
    }
    fn t(a: &[TNode], b: &[TNode]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| match (x, y) {
                (TNode::Tok(p), TNode::Tok(q)) => tok(p, q),
                (TNode::Var(n1), TNode::Var(n2)) => n1 == n2,
                (TNode::Rep(v1, s1, o1), TNode::Rep(v2, s2, o2)) => o1 == o2 && opt(s1, s2) && t(v1, v2),
                (TNode::Group(o1, c1, v1), TNode::Group(o2, c2, v2)) => tok(o1, o2) && tok(c1, c2) && t(v1, v2),
                (TNode::Crate, TNode::Crate) => true,
                _ => false,
            })
    }
    a.name == b.name && a.krate == b.krate && a.builtin == b.builtin && a.arms.len() == b.arms.len() && a.arms.iter().zip(&b.arms).all(|(x, y)| m(&x.0, &y.0) && t(&x.1, &y.1))
}

fn is_open(k: T) -> bool {
    matches!(k, T::OpenParen | T::OpenBracket | T::OpenBrace)
}
fn is_close(k: T) -> bool {
    matches!(k, T::CloseParen | T::CloseBracket | T::CloseBrace)
}

/// Index of the token closing the group opened at `i`.
fn close_of(toks: &[MTok], i: usize) -> usize {
    let mut depth = 0i32;
    let mut j = i;
    while j < toks.len() {
        if is_open(toks[j].kind) {
            depth += 1;
        } else if is_close(toks[j].kind) {
            depth -= 1;
            if depth == 0 {
                return j;
            }
        }
        j += 1;
    }
    toks.len().saturating_sub(1)
}

fn frag_of(s: &str) -> Option<Frag> {
    Some(match s {
        "expr" | "expr_2021" => Frag::Expr,
        "ident" => Frag::Ident,
        "ty" => Frag::Ty,
        "tt" => Frag::Tt,
        "pat" => Frag::Pat,
        "pat_param" => Frag::PatParam,
        "path" => Frag::Path,
        "literal" => Frag::Literal,
        "block" => Frag::Block,
        "item" => Frag::Item,
        "stmt" => Frag::Stmt,
        "vis" => Frag::Vis,
        "lifetime" => Frag::Lifetime,
        "meta" => Frag::Meta,
        _ => return None,
    })
}

fn is_rep_op(t: &MTok) -> Option<u8> {
    match t.kind {
        T::Star => Some(b'*'),
        T::Plus => Some(b'+'),
        T::Question => Some(b'?'),
        _ => None,
    }
}

/// `$( .. )` at `i` (pointing at `$`): returns (inner range, separator, op, next index).
fn rep_at(toks: &[MTok], i: usize) -> Option<(usize, usize, Option<MTok>, u8, usize)> {
    let open = i + 1;
    let close = close_of(toks, open);
    let mut k = close + 1;
    if k < toks.len() {
        if let Some(op) = is_rep_op(&toks[k]) {
            return Some((open + 1, close, None, op, k + 1));
        }
        if k + 1 < toks.len() {
            if let Some(op) = is_rep_op(&toks[k + 1]) {
                let sep = toks[k].clone();
                k += 2;
                return Some((open + 1, close, Some(sep), op, k));
            }
        }
    }
    None
}

fn parse_matcher(toks: &[MTok]) -> Result<Vec<MNode>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t.kind == T::Dollar && i + 1 < toks.len() {
            let n = &toks[i + 1];
            if n.kind == T::OpenParen {
                let (lo, hi, sep, op, next) = rep_at(toks, i).ok_or("bad repetition in macro matcher")?;
                out.push(MNode::Rep(parse_matcher(&toks[lo..hi])?, sep, op));
                i = next;
                continue;
            }
            if matches!(n.kind, T::Ident | T::RawIdent) || n.text.chars().all(|c| c.is_alphanumeric() || c == '_') {
                if i + 3 < toks.len() + 1 && i + 2 < toks.len() && toks[i + 2].kind == T::Colon && i + 3 < toks.len() {
                    let f = frag_of(&toks[i + 3].text).ok_or_else(|| format!("unknown fragment specifier `{}`", toks[i + 3].text))?;
                    out.push(MNode::Var(n.text.clone(), f));
                    i += 4;
                    continue;
                }
            }
        }
        if is_open(t.kind) {
            let c = close_of(toks, i);
            out.push(MNode::Group(t.clone(), toks[c].clone(), parse_matcher(&toks[i + 1..c])?));
            i = c + 1;
            continue;
        }
        out.push(MNode::Tok(t.clone()));
        i += 1;
    }
    Ok(out)
}

fn parse_transcriber(toks: &[MTok]) -> Result<Vec<TNode>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t.kind == T::Dollar && i + 1 < toks.len() {
            let n = &toks[i + 1];
            if n.kind == T::OpenParen {
                let (lo, hi, sep, op, next) = rep_at(toks, i).ok_or("bad repetition in macro body")?;
                out.push(TNode::Rep(parse_transcriber(&toks[lo..hi])?, sep, op));
                i = next;
                continue;
            }
            if n.kind == T::KwCrate {
                out.push(TNode::Crate);
                i += 2;
                continue;
            }
            if matches!(n.kind, T::Ident | T::RawIdent) || n.text.chars().all(|c| c.is_alphanumeric() || c == '_') {
                out.push(TNode::Var(n.text.clone()));
                i += 2;
                continue;
            }
        }
        if is_open(t.kind) {
            let c = close_of(toks, i);
            out.push(TNode::Group(t.clone(), toks[c].clone(), parse_transcriber(&toks[i + 1..c])?));
            i = c + 1;
            continue;
        }
        out.push(TNode::Tok(t.clone()));
        i += 1;
    }
    Ok(out)
}

/// Arms of a `macro_rules!` body: `(matcher) => {transcriber};*`.
/// `macro_rules! alias { ($($t:tt)*) => { target!($($t)*) } }`: a macro imported under another
/// name whose target Relax knows by name only.
pub fn forwarding_def(alias: &str, krate: &str, target: &str) -> Option<MacroRef> {
    let text = format!("($($__t:tt)*) => {{ {}!($($__t)*) }}", target);
    let mut toks = Vec::new();
    crate::lexer::lex(text.as_bytes(), 2021, &mut toks).ok()?;
    let mt: Vec<MTok> = toks
        .iter()
        .filter(|t| t.kind != T::Eof)
        .map(|t| MTok { kind: t.kind, text: String::from_utf8_lossy(&text.as_bytes()[t.lo as usize..t.hi as usize]).into_owned(), pos: u32::MAX })
        .collect();
    parse_def(alias, krate, &mt).ok().map(std::sync::Arc::new)
}

pub fn parse_def(name: &str, krate: &str, toks: &[MTok]) -> Result<MacroDef, String> {
    let mut arms = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i].kind == T::Semi {
            i += 1;
            continue;
        }
        if !is_open(toks[i].kind) {
            return Err(format!("macro `{}`: expected `(matcher) => {{..}}`", name));
        }
        let mc = close_of(toks, i);
        let matcher = parse_matcher(&toks[i + 1..mc])?;
        i = mc + 1;
        if i >= toks.len() || toks[i].kind != T::FatArrow {
            return Err(format!("macro `{}`: expected `=>`", name));
        }
        i += 1;
        if i >= toks.len() || !is_open(toks[i].kind) {
            return Err(format!("macro `{}`: expected a delimited body", name));
        }
        let bc = close_of(toks, i);
        let body = parse_transcriber(&toks[i + 1..bc])?;
        i = bc + 1;
        arms.push((matcher, body));
    }
    Ok(MacroDef { name: name.to_string(), krate: krate.to_string(), exported: false, arms, builtin: 0, def_file: String::new(), def_pos: 0,
        input: Some(std::sync::Arc::new(RuleInput { tokens: toks.to_vec(), local_inner: false })) })
}

#[derive(Clone, Debug)]
pub enum Bind {
    /// a matched fragment: token index range in the parser's token list
    One(usize, usize, Frag),
    Seq(Vec<Bind>),
}

fn vars_of(nodes: &[MNode], out: &mut Vec<String>) {
    for n in nodes {
        match n {
            MNode::Var(v, _) => out.push(v.clone()),
            MNode::Rep(x, _, _) | MNode::Group(_, _, x) => vars_of(x, out),
            MNode::Tok(_) => {}
        }
    }
}

fn tvars_of(nodes: &[TNode], out: &mut Vec<String>) {
    for n in nodes {
        match n {
            TNode::Var(v) => out.push(v.clone()),
            TNode::Rep(x, _, _) | TNode::Group(_, _, x) => tvars_of(x, out),
            _ => {}
        }
    }
}

impl<'a> Parser<'a> {
    fn mtok_eq(&self, i: usize, m: &MTok) -> bool {
        let t = self.toks.at(i);
        if t.kind != m.kind {
            return false;
        }
        match t.kind {
            T::Ident | T::RawIdent | T::Lifetime | T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte => self.tok_text(t) == m.text.as_bytes(),
            _ => true,
        }
    }

    /// Closing token index of the group opened at token `i` (bounded by `end`).
    fn tok_close_of(&self, i: usize, end: usize) -> Option<usize> {
        let mut depth = 0i32;
        let mut j = i;
        while j < end {
            let k = self.toks.at(j).kind;
            if is_open(k) {
                depth += 1;
            } else if is_close(k) {
                depth -= 1;
                if depth == 0 {
                    return Some(j);
                }
            }
            j += 1;
        }
        None
    }

    /// One fragment at token `i` (ending by `end`): its end index.
    fn match_frag(&mut self, f: Frag, i: usize, end: usize) -> Option<usize> {
        if i > end {
            return None;
        }
        let k = if i < end { self.toks.at(i).kind } else { T::Eof };
        match f {
            Frag::Ident => {
                if i < end && (matches!(k, T::Ident | T::RawIdent) || (T::KwAs as u8..=T::KwTry as u8).contains(&(k as u8))) && self.tok_text(self.toks.at(i)) != b"_" {
                    Some(i + 1)
                } else {
                    None
                }
            }
            Frag::Lifetime => (i < end && k == T::Lifetime).then_some(i + 1),
            Frag::Literal => {
                let mut j = i;
                if j < end && self.toks.at(j).kind == T::Minus {
                    j += 1;
                }
                if j < end && matches!(self.toks.at(j).kind, T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte | T::KwTrue | T::KwFalse) {
                    Some(j + 1)
                } else {
                    None
                }
            }
            Frag::Tt => {
                if i >= end {
                    return None;
                }
                if is_open(k) {
                    self.tok_close_of(i, end).map(|c| c + 1)
                } else if is_close(k) {
                    None
                } else {
                    Some(i + 1)
                }
            }
            Frag::Block => {
                if i < end && k == T::OpenBrace {
                    self.tok_close_of(i, end).map(|c| c + 1)
                } else {
                    None
                }
            }
            Frag::Vis => {
                if i < end && k == T::KwPub {
                    if i + 1 < end && self.toks.at(i + 1).kind == T::OpenParen {
                        return self.tok_close_of(i + 1, end).map(|c| c + 1);
                    }
                    return Some(i + 1);
                }
                Some(i)
            }
            _ => {
                if i >= end {
                    return None;
                }
                let save = self.pos;
                let undo = self.tok_undo.len();
                self.pos = i;
                let r: PResult<()> = match f {
                    Frag::Expr => self.expr().map(|_| ()),
                    Frag::Ty => self.ty().map(|_| ()),
                    Frag::Pat => self.pat_top().map(|_| ()),
                    Frag::PatParam => self.pat_no_or().map(|_| ()),
                    Frag::Path => self.path(false).map(|_| ()),
                    Frag::Stmt => self.stmt_fragment(),
                    Frag::Item => self.item_fragment(),
                    Frag::Meta => self.meta_fragment(),
                    _ => Ok(()),
                };
                let e = self.pos;
                self.pos = save;
                // a `>>` / `>=` the fragment's parse split and consumed whole is whole again (its
                // text belongs to the fragment); one the fragment ends inside of stays split, its
                // rest for the matcher (`Vec<$t:ty>` against `Vec<Option<u8>>`)
                while self.tok_undo.len() > undo {
                    let (k, t) = self.tok_undo.pop().unwrap();
                    if (k as usize) < e {
                        self.toks.edit(k as usize, |x| *x = t);
                    }
                }
                match r {
                    Ok(()) if e <= end && e > i => Some(e),
                    _ => None,
                }
            }
        }
    }

    fn match_seq(&mut self, nodes: &[MNode], i: &mut usize, end: usize, binds: &mut HashMap<String, Bind>) -> bool {
        for n in nodes {
            match n {
                MNode::Tok(m) => {
                    if *i >= end || !self.mtok_eq(*i, m) {
                        return false;
                    }
                    *i += 1;
                }
                MNode::Group(open, _, inner) => {
                    if *i >= end || self.toks.at(*i).kind != open.kind {
                        return false;
                    }
                    let c = match self.tok_close_of(*i, end) {
                        Some(c) => c,
                        None => return false,
                    };
                    let mut j = *i + 1;
                    if !self.match_seq(inner, &mut j, c, binds) || j != c {
                        return false;
                    }
                    *i = c + 1;
                }
                MNode::Var(name, f) => match self.match_frag(*f, *i, end) {
                    Some(e) => {
                        binds.insert(name.clone(), Bind::One(*i, e, *f));
                        *i = e;
                    }
                    None => return false,
                },
                MNode::Rep(inner, sep, op) => {
                    let mut names = Vec::new();
                    vars_of(inner, &mut names);
                    let mut iters: Vec<HashMap<String, Bind>> = Vec::new();
                    loop {
                        let start = *i;
                        let mut j = *i;
                        if !iters.is_empty() {
                            if let Some(s) = sep {
                                if j >= end || !self.mtok_eq(j, s) {
                                    break;
                                }
                                j += 1;
                            }
                        }
                        let mut b = HashMap::default();
                        if !self.match_seq(inner, &mut j, end, &mut b) || j == start {
                            break;
                        }
                        *i = j;
                        iters.push(b);
                        if *op == b'?' {
                            break;
                        }
                    }
                    if *op == b'+' && iters.is_empty() {
                        return false;
                    }
                    for nm in names {
                        let seq: Vec<Bind> = iters.iter().map(|b| b.get(&nm).cloned().unwrap_or(Bind::Seq(Vec::new()))).collect();
                        binds.insert(nm, Bind::Seq(seq));
                    }
                }
            }
        }
        true
    }

    fn lookup_bind<'b>(binds: &'b HashMap<String, Bind>, name: &str, idx: &[usize]) -> Option<&'b Bind> {
        let mut b = binds.get(name)?;
        for &k in idx {
            match b {
                Bind::Seq(v) => b = v.get(k)?,
                Bind::One(..) => return Some(b),
            }
        }
        Some(b)
    }

    fn transcribe(&self, nodes: &[TNode], binds: &HashMap<String, Bind>, idx: &mut Vec<usize>, krate_path: &str, out: &mut Transcript) -> Result<(), String> {
        for n in nodes {
            match n {
                TNode::Tok(t) => out.push(&t.text, None, t.pos),
                TNode::Crate => out.push(krate_path, None, u32::MAX),
                TNode::Group(o, c, inner) => {
                    out.push(&o.text, None, o.pos);
                    self.transcribe(inner, binds, idx, krate_path, out)?;
                    out.push(&c.text, None, c.pos);
                }
                TNode::Var(v) => match Self::lookup_bind(binds, v, idx) {
                    Some(Bind::One(lo, hi, f)) => {
                        let wrap = *f == Frag::Expr && hi - lo > 1;
                        if wrap {
                            out.push("(", None, u32::MAX);
                        }
                        for k in *lo..*hi {
                            let t = self.toks.at(k);
                            out.push(&String::from_utf8_lossy(self.tok_text(t)), Some(t.lo), u32::MAX);
                        }
                        if wrap {
                            out.push(")", None, u32::MAX);
                        }
                    }
                    Some(Bind::Seq(_)) => return Err(format!("`${}` is still repeating here -- use it inside `$( .. )*`", v)),
                    // not a macro variable: `$x` literally (rare; keeps the text)
                    None => {
                        out.push("$", None, u32::MAX);
                        out.push(v, None, u32::MAX);
                    }
                },
                TNode::Rep(inner, sep, _) => {
                    let mut names = Vec::new();
                    tvars_of(inner, &mut names);
                    let mut count = None;
                    for nm in &names {
                        if let Some(Bind::Seq(v)) = Self::lookup_bind(binds, nm, idx) {
                            count = Some(v.len());
                            break;
                        }
                    }
                    let count = match count {
                        Some(c) => c,
                        None => return Err("repetition without a repeating macro variable".to_string()),
                    };
                    for k in 0..count {
                        if k > 0 {
                            if let Some(s) = sep {
                                out.push(&s.text, None, s.pos);
                            }
                        }
                        idx.push(k);
                        self.transcribe(inner, binds, idx, krate_path, out)?;
                        idx.pop();
                    }
                }
            }
        }
        Ok(())
    }

    /// The arguments [lo, hi) with their doc comments desugared as rustc does before matching
    /// (`parser_from_cx`, rustc_expand mbe/macro_rules.rs:1808 `tts.desugar_doc_comments()`):
    /// `/// x` is the tokens `#[doc = r"x"]`, `//! x` is `#![doc = r"x"]`. The lexer drops
    /// comments, so they are read from the source gaps between the argument tokens. Without
    /// doc comments the range is returned as it is; otherwise a copy is appended to the token
    /// list and its range returned.
    fn desugar_doc_comments(&mut self, lo: usize, hi: usize) -> (usize, usize) {
        let base = self.src.len() as u32;
        if lo == 0 || hi < lo || self.toks.at(hi).lo >= base {
            return (lo, hi);
        }
        // (index of the token the docs precede, inner, text)
        let mut docs: Vec<(usize, bool, Vec<u8>)> = Vec::new();
        for j in lo..=hi {
            let (prev, t) = (self.toks.at(j - 1), self.toks.at(j));
            if prev.hi < t.lo && t.lo <= base {
                let gap = &self.src[prev.hi as usize..t.lo as usize];
                if gap.windows(2).any(|w| w == b"//" || w == b"/*") {
                    crate::pm_tokens::for_each_gap_doc(gap, |d| docs.push((j, d.inner, d.text.to_vec())));
                }
            }
        }
        if docs.is_empty() {
            return (lo, hi);
        }
        let mut text = String::new();
        let mut spans = Vec::with_capacity(docs.len());
        for (_, inner, t) in &docs {
            let a = crate::pm_tokens::desugared_doc_attr(*inner, t);
            spans.push((text.len(), text.len() + a.len()));
            text.push_str(&a);
            text.push(' ');
        }
        let ext_base = (self.src.len() + self.ext.len()) as u32;
        self.ext.extend_from_slice(text.as_bytes());
        let call_lo = self.toks.at(lo - 1).lo;
        self.exp_map.push((ext_base, ext_base + text.len() as u32, call_lo));
        let mut out = Vec::with_capacity(hi - lo + docs.len() * 7);
        let mut d = 0;
        for j in lo..=hi {
            while d < docs.len() && docs[d].0 == j {
                let (a, b) = spans[d];
                let mut dt = Vec::new();
                let _ = crate::lexer::lex(&text.as_bytes()[a..b], self.edition, &mut dt);
                out.extend(dt.into_iter().filter(|t| t.kind != T::Eof).map(|t| Tok { kind: t.kind, lo: t.lo + a as u32 + ext_base, hi: t.hi + a as u32 + ext_base }));
                d += 1;
            }
            out.push(self.toks.at(j));
        }
        let start = self.toks.len();
        let n = out.len() - 1;
        for t in out {
            self.toks.push(t);
        }
        (start, start + n)
    }

    /// Expands a call of macro `def` with arguments in tokens [lo, hi): the expansion text
    /// (tokens joined by spaces) and, per token substituted from a fragment, (offset in the
    /// text, the fragment token's position) -- or an error naming the macro.
    pub fn expand_rules(&mut self, def: &MacroDef, lo: usize, hi: usize) -> Result<(String, Vec<(u32, u32, u32)>, Vec<(u32, u32, u32)>), String> {
        let _profile = crate::load_profile::span(self.session.runtime.profile, "macro-rules", &def.name);
        let (lo, hi) = self.desugar_doc_comments(lo, hi);
        for (matcher, body) in &def.arms {
            let mut binds = HashMap::default();
            let mut i = lo;
            if self.match_seq(matcher, &mut i, hi, &mut binds) && i == hi {
                // `$crate` names the defining crate itself, even where the caller reaches the macro
                // only through a re-export (no dependency of that name)
                let krate_path = if def.krate == self.krate { "crate".to_string() } else { format!("::__relax_crate_{}", def.krate) };
                let mut out = Transcript::default();
                self.transcribe(body, &binds, &mut Vec::new(), &krate_path, &mut out)?;
                out.s.push(' ');
                return Ok((out.s, out.origins, out.bodies));
            }
        }
        Err(format!("no rule of `{}!` matches these arguments -- check the macro's patterns", def.name))
    }
}

/// A transcription being written: the text (each token preceded by a space), per token
/// substituted from a fragment (offset, its position), per token of the definition body
/// (offset, its position in the definition).
#[derive(Default)]
struct Transcript {
    s: String,
    origins: Vec<(u32, u32, u32)>,
    bodies: Vec<(u32, u32, u32)>,
}

impl Transcript {
    fn push(&mut self, t: &str, origin: Option<u32>, bp: u32) {
        self.s.push(' ');
        if let Some(o) = origin {
            self.origins.push((self.s.len() as u32, o, t.len() as u32));
        } else if bp != u32::MAX {
            self.bodies.push((self.s.len() as u32, bp, t.len() as u32));
        }
        self.s.push_str(t);
    }
}

/// Owned tokens of a token range (definition bodies outlive the parse of their file).
pub fn mtoks(p: &Parser, toks: &[Tok]) -> Vec<MTok> {
    toks.iter().map(|t| MTok { kind: t.kind, text: String::from_utf8_lossy(p.tok_text(*t)).into_owned(), pos: t.lo }).collect()
}

#[cfg(test)]
mod replay_tests {
    use super::*;

    fn definition(krate: &str, target: &str) -> MacroRef {
        let mut d = std::sync::Arc::try_unwrap(forwarding_def("value", krate, target).unwrap()).ok().unwrap();
        d.exported = true;
        std::sync::Arc::new(d)
    }

    fn macro_table() -> MacroTable {
        let mut table = MacroTable::default();
        for name in ["app", "first", "second", "dep"] {
            let mut cm = CrateMods { root: name.into(), ..Default::default() };
            cm.mods.insert(name.into());
            if name == "app" {
                for dep in ["first", "second", "dep"] { cm.externs.insert(dep.into(), dep.into()); }
            }
            table.add_crate(name.into(), std::sync::Arc::new(cm));
        }
        table
    }

    #[test]
    fn replay_keeps_local_alias_and_each_lookup_import_snapshot() {
        let mut table = macro_table();
        table.push(definition("first", "one"));
        table.push(definition("second", "two"));
        let table = std::sync::Arc::new(table);
        let scope = MacroScope::new(table.clone());
        scope.add_use("app", "runtime".into(), vec!["first".into()]);
        assert!(scope.lookup("value", "app", &["runtime".into()], "app").is_some());
        scope.add_use("app", "runtime".into(), vec!["second".into()]);
        assert!(scope.lookup("value", "app", &["runtime".into()], "app").is_some());
        let used = scope.finish();
        assert!(used.replays(&table, "app", "app.rs"));
        let mut changed = macro_table();
        changed.push(definition("first", "changed"));
        changed.push(definition("second", "two"));
        assert!(!used.replays(&std::sync::Arc::new(changed), "app", "app.rs"));
    }

    #[test]
    fn replay_keeps_expansion_glob_imports() {
        let mut table = macro_table();
        table.push(definition("dep", "one"));
        let table = std::sync::Arc::new(table);
        let scope = MacroScope::new(table.clone());
        scope.add_glob_use("app", vec!["dep".into()]);
        assert!(scope.lookup("value", "app", &[], "app").is_some());
        assert!(scope.finish().replays(&table, "app", "app.rs"));
    }

    #[test]
    fn replay_skips_unchanged_inputs_and_rechecks_changed_imports_and_misses() {
        let mut table = macro_table();
        table.push(definition("first", "one"));
        table.push(definition("second", "two"));
        table.add_exp_uses("app", [(("app::generated".into(), "selected".into()), vec!["first".into(), "value".into()])]);
        let table = std::sync::Arc::new(table);
        let scope = MacroScope::new(table.clone());
        let path = ["crate".into(), "generated".into()];
        assert_eq!(scope.lookup("selected", "app", &path, "app").unwrap().krate, "first");
        assert!(scope.lookup("absent", "app", &path, "app").is_none());
        let used = scope.finish();

        let mut next = (*table).clone();
        next.add_exp_uses("app", [(("app::other".into(), "unrelated".into()), vec!["second".into(), "value".into()])]);
        assert!(used.looked.iter().all(|l| l.inputs.unchanged(&next)));
        assert!(used.replays(&std::sync::Arc::new(next.clone()), "app", "app.rs"));

        // Invalidate a cached validation on the same mutable publication too.
        next.add_exp_uses("app", [(("app::generated".into(), "selected".into()), vec!["second".into(), "value".into()])]);
        assert!(!used.looked[0].inputs.unchanged(&next));
        assert!(!used.replays(&std::sync::Arc::new(next), "app", "app.rs"));

        let mut next = (*table).clone();
        next.add_exp_uses("app", [(("app::generated".into(), "absent".into()), vec!["first".into(), "value".into()])]);
        assert!(!used.looked[1].inputs.unchanged(&next));
        assert!(!used.replays(&std::sync::Arc::new(next), "app", "app.rs"));
    }

    #[test]
    fn expansion_import_paths_follow_alias_chains_and_stop_at_cycles() {
        let mut table = macro_table();
        table.push(definition("dep", "one"));
        let table = std::sync::Arc::new(table);
        let scope = MacroScope::new(table.clone());
        for i in 0..16 {
            scope.add_use("app::generated", format!("alias{i}"), vec!["self".into(), format!("alias{}", i + 1)]);
        }
        scope.add_use("app::generated", "alias16".into(), vec!["dep".into(), "value".into()]);
        scope.add_use("app::generated", "cycle_a".into(), vec!["self".into(), "cycle_b".into()]);
        scope.add_use("app::generated", "cycle_b".into(), vec!["self".into(), "cycle_a".into()]);
        let path = ["crate".into(), "generated".into()];
        assert!(scope.lookup("alias0", "app", &path, "app::nested").is_some());
        assert!(scope.lookup("cycle_a", "app", &path, "app::nested").is_none());
        assert!(scope.finish().replays(&table, "app", "app.rs"));
    }

    #[test]
    fn expansion_glob_cycle_does_not_grow_the_lookup_path() {
        let mut table = macro_table();
        table.push(definition("dep", "one"));
        let mut unreachable = (*definition("first", "two")).clone();
        unreachable.name = "unreachable".into();
        table.define(unreachable);
        let table = std::sync::Arc::new(table);
        let scope = MacroScope::new(table.clone());
        scope.add_glob_use("app", vec!["self".into(), "lib_rr".into()]);
        scope.add_glob_use("app::lib_rr", vec!["super".into()]);
        scope.add_glob_use("app::lib_rr", vec!["dep".into()]);
        assert!(scope.lookup("value", "app", &["crate".into(), "lib_rr".into()], "app").is_some());
        assert!(scope.lookup("missing", "app", &["dep".into()], "app::lib_rr").is_none());
        assert!(scope.lookup("unreachable", "app", &["dep".into()], "app::lib_rr").is_none());
        assert!(scope.lookup("value", "app", &["dep".into(), "dep".into()], "app::lib_rr").is_none());
        assert!(scope.lookup("missing", "app", &["crate".into(), "lib_rr".into()], "app").is_none());
        assert!(scope.finish().replays(&table, "app", "app.rs"));
    }
}
