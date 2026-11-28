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
    /// `expr_2021`: parsed as `expr`, but whatever the edition it never begins with `_` or
    /// `const` (rustc NonterminalKind::Expr(Expr2021))
    Expr2021,
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
    /// `$name`, and the position of its `$` where the macro is defined (u32::MAX when unknown):
    /// where rustc reports a parse or match error on the fragment substituted for it
    Var(String, u32),
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
    pub builtin: u16,
    /// where a `macro_rules!` is defined: (file path, source position) -- its expansions'
    /// identifiers resolve local variables there (definition-site hygiene)
    pub def_file: String,
    pub def_pos: u32,
    /// Authored invocation position for generated definitions. Keep def_pos for
    /// definition-site hygiene, but order textual scope at the invocation.
    pub scope_pos: Option<u32>,
    /// the module (path as `module_path!` prints it) the `macro_rules!` is written in; empty:
    /// unknown (made by an expansion, a built-in), visible wherever the crate is
    pub def_mod: String,
    /// the edition of the crate the definition is written in (rustc SyntaxExtension::edition:
    /// a token of its expansion has it, a substituted fragment keeps its own); 0: none of its
    /// own (a built-in), the invoking file's
    pub edition: u16,
    /// Exact definition input, for prescan/outline/parser reuse. Images already
    /// contain parsed arms and need not persist this cold-parse acceleration.
    pub(crate) input: Option<std::sync::Arc<RuleInput>>,
}

#[derive(Clone)]
pub(crate) struct RuleInput {
    tokens: Vec<MTok>,
    local_inner: bool,
}

impl MacroDef {
    /// The tokens of the definition's body (`(matcher) => {transcriber};*`) when the macro was
    /// parsed from source in this process.
    pub fn input_tokens(&self) -> Option<&[MTok]> { self.input.as_ref().map(|i| &i.tokens[..]) }
}

pub type MacroRef = std::sync::Arc<MacroDef>;

/// The crate name of a macro-table crate key (`<name>\u{1}lib`: a lib shadowed by its bin).
fn krate_name(k: &str) -> &str {
    k.split('\u{1}').next().unwrap_or(k)
}

/// Only a resolved crate can extend the extern prelude. Raw macro input may
/// mention unavailable crates whose tokens never become Rust declarations.
pub(crate) fn extern_target(externs: &HashMap<String, String>, name: &str) -> Option<String> {
    externs.get(name).cloned().or_else(|| matches!(name, "std" | "core" | "alloc").then(|| name.to_string()))
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

/// Visibility of a prescanned import or module declaration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UseVis {
    #[default]
    Private,
    Pub,
    Crate,
    Super,
    In(Vec<String>),
}

/// An expansion-made import recorded under its owning module and binding name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpUse {
    pub path: Vec<String>,
    pub vis: UseVis,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LocalExpUse {
    path: Vec<Name>,
    vis: UseVis,
}

impl LocalExpUse {
    fn remap(&self, tr: impl Fn(Name) -> Name) -> Self {
        Self { path: self.path.iter().copied().map(tr).collect(), vis: self.vis.clone() }
    }
}

impl UseVis {
    pub fn is_pub(&self) -> bool { matches!(self, Self::Pub) }
    pub fn is_restricted_or_pub(&self) -> bool { !matches!(self, Self::Private) }
}

/// A restriction is an ancestor prefix of the binding's owner. After a successful
/// visibility check the same prefix also belongs to the importing module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ScopeVis(u32);

impl ScopeVis {
    const PUB: Self = Self(u32::MAX);
    fn private(m: &str) -> Self { Self(m.len() as u32) }
    fn krate(root: &str, m: &str) -> Self { Self::in_ancestor(m, root).unwrap_or_else(|| Self::private(m)) }
    fn super_of(m: &str) -> Self { Self::private(m.rsplit_once("::").map_or(m, |(p, _)| p)) }
    fn in_ancestor(m: &str, ancestor: &str) -> Option<Self> { within(m, ancestor).then(|| Self::private(ancestor)) }
    fn is_pub(self) -> bool { self == Self::PUB }
    fn visible_to_req(self, k: &str, m: &str, asker_k: &str, req: Option<&str>) -> bool {
        self.is_pub() || (k == asker_k && req.is_some_and(|asker| within(asker, &m[..self.0 as usize])))
    }
    fn visible_to(self, k: &str, m: &str, asker_k: &str, asker: &str) -> bool {
        self.visible_to_req(k, m, asker_k, Some(asker))
    }
    fn min(self, other: Self) -> Self {
        if self.is_wider_than(other) { other } else { self }
    }
    fn is_wider_than(self, other: Self) -> bool {
        !other.is_pub() && (self.is_pub() || self.0 < other.0)
    }
}

fn within(m: &str, ancestor: &str) -> bool {
    m == ancestor || m.strip_prefix(ancestor).is_some_and(|rest| rest.starts_with("::"))
}

fn common_mod_prefix<'a>(mut a: &'a str, b: &str) -> &'a str {
    while !within(b, a) {
        let Some((parent, _)) = a.rsplit_once("::") else { return ""; };
        a = parent;
    }
    a
}

/// A transitive glob binding must be visible to the caller and each importer.
fn next_req<'a>(k: &str, m: &str, target: &str, req: Option<&'a str>) -> Option<&'a str> {
    if k != target { return None; }
    req.map(|r| common_mod_prefix(r, m)).filter(|p| !p.is_empty())
}

/// Test the import's declared visibility from the module requesting the binding.
fn exp_vis_visible(r: &Res, ks: &str, ms: &str, vis: &UseVis, req: Option<&str>) -> bool {
    let root = r.crate_root(ks).1;
    let resolved = vis.ancestor(root, ms).or_else(|| {
        let UseVis::In(path) = vis else { return None };
        let (tk, tm) = r.resolve_mod(ks, ms, path)?;
        (tk == ks).then(|| ScopeVis::in_ancestor(ms, tm)).flatten()
    }).unwrap_or_else(|| ScopeVis::private(ms));
    resolved.visible_to_req(ks, ms, ks, req)
}

impl UseVis {
    fn ancestor(&self, root: &str, module: &str) -> Option<ScopeVis> {
        let Self::In(path) = self else {
            return Some(match self {
                Self::Private => ScopeVis::private(module),
                Self::Pub => ScopeVis::PUB,
                Self::Crate => ScopeVis::krate(root, module),
                Self::Super => ScopeVis::super_of(module),
                Self::In(_) => unreachable!(),
            });
        };
        let (first, rest) = path.split_first()?;
        let (mut candidate, segments) = match first.as_str() {
            "crate" | "" => (root.to_string(), rest),
            "self" => (module.to_string(), rest),
            "super" => (module.to_string(), path.as_slice()),
            _ => (root.to_string(), path.as_slice()),
        };
        for segment in segments {
            match segment.as_str() {
                "self" => {}
                "super" => { if let Some(pos) = candidate.rfind("::") { candidate.truncate(pos); } }
                ident => { candidate.push_str("::"); candidate.push_str(ident); }
            }
        }
        ScopeVis::in_ancestor(module, &candidate)
    }

    fn resolve(&self, root: &str, module: &str) -> ScopeVis {
        self.ancestor(root, module).unwrap_or_else(|| ScopeVis::private(module))
    }
}

/// One `use` declaration's binding, as the preload scan reads it: in module `module` (module path
/// as `module_path!` prints it, `krate::a::b`), the path `path` (first segment as written:
/// `crate`, `self`, `super`, `$crate` as `__relax_crate_<name>`, `""` for a leading `::`) is
/// bound under `bind`, or glob-imported (`bind` None). `extern crate x as y` is `["", x]` as `y`.
#[derive(Clone, Debug, PartialEq)]
pub struct UseItem {
    pub module: String,
    pub path: Vec<String>,
    pub bind: Option<String>,
    pub vis: UseVis,
}

/// One crate's module tree and imports (for macro path resolution before name resolution:
/// rustc resolves macro paths with the same import resolution as items).
/// One `mod` item of a crate as the preload scan reads it: where it sits in its parent's file
/// (`pos`: its `mod` keyword, `end`: the end of the item -- its `;` or closing brace) and whether
/// it carries `#[macro_use]`. rustc's legacy `macro_rules!` scope is made of these
/// (rustc_resolve build_reduced_graph.rs: `MacroRulesScope`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModItem {
    pub pos: u32,
    pub end: u32,
    pub macro_use: bool,
    pub vis: UseVis,
}

#[derive(Default)]
pub struct CrateMods {
    /// the crate root's module path (the crate name)
    pub root: String,
    /// the crate root's source file: two crates of one name (a package's lib and bin) are told
    /// apart by it
    pub src: String,
    /// every module path of the crate
    pub mods: crate::fxhash::HashSet<String>,
    /// the `mod` item of each module (not the root)
    pub items: HashMap<String, ModItem>,
    /// Modules with `#![macro_use]`, owned by the file containing their body.
    /// Kept separate from the parent-owned `mod` item so a child-only edit replaces it.
    pub inner_macro_use: crate::fxhash::HashSet<String>,
    /// `use` / `extern crate` bindings per module
    pub uses: HashMap<String, Vec<UseItem>>,
    /// extern prelude: names the crate refers to other crates by -> their crate names
    pub externs: HashMap<String, String>,
    /// names `use .. as name` binds under another name
    pub renames: crate::fxhash::HashSet<String>,
    /// crate-level `#![recursion_limit = ".."]` from the root file
    pub recursion_limit: Option<u32>,
    /// the crate's resolved scopes, computed once on first use (CrateScope::compute)
    pub scope: std::sync::OnceLock<CrateScope>,
}

/// A file's imports as the prescan reads them: what it gives its crate's macro path resolution.
#[derive(Clone, Debug, Default)]
pub struct FileImports {
    pub uses: Vec<UseItem>,
    /// (alias, first path segment, original name): MacroTable::aliases
    pub aliases: Vec<(String, String, String)>,
    pub macro_use: Vec<String>,
    pub mods: Vec<String>,
    pub mod_items: Vec<(String, ModItem)>,
    pub inner_macro_use: Vec<String>,
}

impl FileImports {
    /// The same imports (`mod` items only move with the text around them).
    pub fn same_as(&self, other: &FileImports) -> bool {
        fn items(f: &FileImports) -> impl Iterator<Item = (&String, bool, &UseVis)> {
            f.mod_items.iter().map(|(m, i)| (m, i.macro_use, &i.vis))
        }
        self.uses == other.uses && self.aliases == other.aliases && self.macro_use == other.macro_use && self.mods == other.mods
            && self.inner_macro_use == other.inner_macro_use && items(self).eq(items(other))
    }
}

/// A live edit that changed a file's imports. rustc resolves macro paths with the same imports as
/// items; the crate's copy of them was read at load, and no load runs for an edit.
#[derive(Clone, Debug)]
pub struct ImportEdit {
    pub krate: String,
    /// the crate's root file (CrateMods::src): a package's lib and bin share the name
    pub root: String,
    pub was: FileImports,
    pub now: FileImports,
}

impl ImportEdit {
    /// What takes a refused or rolled-back edit's imports out again.
    pub fn undone(self) -> ImportEdit {
        ImportEdit { krate: self.krate, root: self.root, was: self.now, now: self.was }
    }
}

impl CrateMods {
    /// One scanned `use` / `extern crate` binding joins the crate.
    pub fn add_use(&mut self, u: UseItem) {
        if let Some(b) = &u.bind {
            if u.path.last() != Some(b) { self.renames.insert(b.clone()); }
            if u.module == self.root {
                if u.path.len() == 1 && u.path[0] == "crate" {
                    self.externs.insert(b.clone(), self.root.clone());
                } else if u.path.len() == 2 && u.path[0].is_empty() {
                    if let Some(target) = extern_target(&self.externs, &u.path[1]) { self.externs.insert(b.clone(), target); }
                }
            }
        }
        self.uses.entry(u.module.clone()).or_default().push(u);
    }

    /// A copy whose scopes are computed afresh on next use.
    fn unresolved(&self) -> CrateMods {
        CrateMods { root: self.root.clone(), src: self.src.clone(), mods: self.mods.clone(), items: self.items.clone(), inner_macro_use: self.inner_macro_use.clone(), uses: self.uses.clone(),
            externs: self.externs.clone(), renames: self.renames.clone(), recursion_limit: self.recursion_limit, scope: Default::default() }
    }

    fn replace_imports(&mut self, e: &ImportEdit) {
        for u in &e.was.uses {
            if let Some(v) = self.uses.get_mut(&u.module) { v.retain(|x| x != u); }
            // `extern crate x as y;` at the root named `y`; a dependency's own name stays
            let names_a_crate = u.path.len() == 1 && u.path[0] == "crate" || u.path.len() == 2 && u.path[0].is_empty();
            if let (true, Some(b)) = (u.module == self.root && names_a_crate && u.path.last() != u.bind.as_ref(), &u.bind) {
                if !e.now.uses.iter().any(|n| n.module == u.module && n.bind.as_ref() == Some(b)) { self.externs.remove(b); }
            }
        }
        for u in &e.now.uses {
            if !self.uses.get(&u.module).is_some_and(|v| v.contains(u)) { self.add_use(u.clone()); }
        }
        for m in &e.was.mods {
            if !e.now.mods.contains(m) { self.mods.remove(m); }
        }
        self.mods.extend(e.now.mods.iter().cloned());
        for (m, _) in &e.was.mod_items {
            if !e.now.mod_items.iter().any(|(n, _)| n == m) { self.items.remove(m); }
        }
        self.items.extend(e.now.mod_items.iter().cloned());
        for m in &e.was.inner_macro_use { self.inner_macro_use.remove(m); }
        self.inner_macro_use.extend(e.now.inner_macro_use.iter().cloned());
    }

    /// Is the `macro_rules!` `d` of this crate in rustc's legacy (textual) scope in module `from`
    /// at source position `at` (None: at any position of it)? A definition is in scope after it
    /// in its module and in the modules declared after it inside that module; it leaves its module
    /// only through `#[macro_use]` on the `mod` item, and then is in scope after that item in the
    /// parent (rustc_resolve build_reduced_graph.rs: `MacroRulesScope::Binding` chained through
    /// `define_macro`, `#[macro_use]` modules `restore_macro_rules_scope` to the parent; ident.rs
    /// `visit_scopes`). What the scan has no `mod` item for (a module an expansion made) is not
    /// known to be out of scope.
    pub fn in_textual_scope(&self, d: &MacroDef, from: &str, at: Option<u32>) -> bool {
        if d.def_mod.is_empty() { return true; }
        let mut m = d.def_mod.as_str();
        let mut e = d.scope_pos.unwrap_or(d.def_pos);
        loop {
            if from == m {
                return at.map_or(true, |a| a > e);
            }
            if let Some(rest) = from.strip_prefix(m).and_then(|r| r.strip_prefix("::")) {
                // `from` is inside `m`: in scope when the child `mod` item toward it is after `e`
                let child = &from[..m.len() + 2 + rest.find("::").unwrap_or(rest.len())];
                return self.items.get(child).map_or(true, |it| it.pos > e);
            }
            // `from` is outside `m`: only a `#[macro_use]` module hands its macros to its parent
            let Some(it) = self.items.get(m) else { return true };
            let Some((parent, _)) = m.rsplit_once("::") else { return false };
            if !it.macro_use && !self.inner_macro_use.contains(m) { return false; }
            e = it.end;
            m = parent;
        }
    }

    /// Rank an in-scope binding by its nearest containing scope and the position
    /// where it entered that scope. File-module definitions acquire the position
    /// of the exporting `mod`, not an offset in the child file. Unknown generated
    /// modules keep the resolver's existing precedence instead.
    pub(crate) fn textual_binding_key(&self, d: &MacroDef, from: &str) -> Option<(usize, Vec<u32>)> {
        if d.def_mod.is_empty() { return None; }
        let mut module = d.def_mod.as_str();
        let mut positions = vec![d.scope_pos.unwrap_or(d.def_pos)];
        loop {
            if from == module || from.strip_prefix(module).is_some_and(|rest| rest.starts_with("::")) {
                // Two macros exported through the same module have the same
                // outer position. Break ties by their order inside that module.
                positions.reverse();
                return Some((module.split("::").count(), positions));
            }
            positions.push(self.items.get(module)?.end);
            module = module.rsplit_once("::")?.0;
        }
    }
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
    tnon: HashMap<String, HashMap<String, (String, String, ScopeVis)>>,
    /// Glob sources and the visibility of each import.
    tglobs: HashMap<String, Vec<(String, String, ScopeVis)>>,
    /// module -> name -> macro, non-glob then glob
    mnon: HashMap<String, HashMap<String, (MacroRef, ScopeVis)>>,
    mglob: HashMap<String, HashMap<String, (MacroRef, ScopeVis)>>,
    /// this computation's identity (unique per process, never 0): a memoized answer that went
    /// through another crate's scope holds while that crate still has this very scope
    id: u64,
    /// module -> name -> the answer of `Res::type_in` (a glob walk can visit hundreds of
    /// modules; the same few names are asked from the same modules over and over)
    memo: std::sync::RwLock<HashMap<String, HashMap<String, TypeInMemo>>>,
    pub_memo: std::sync::RwLock<HashMap<String, HashMap<String, TypeInMemo>>>,
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

/// Set on the end of a recorded fragment group that is a `$t:ty` substitution.
pub(crate) const TY_FRAG: u32 = 1 << 31;
/// An `$e:expr` / `$l:literal` fragment (rustc's interpolated `NtExpr` / `NtLiteral` token): where the
/// grammar wants an identifier or a literal, it is "a metavariable". A fragment of one token carries
/// only its kind (META_ONLY: no token-tree group).
pub(crate) const EXPR_FRAG: u32 = 1 << 30;
pub(crate) const LIT_FRAG: u32 = 1 << 29;
pub(crate) const META_ONLY: u32 = 1 << 28;
/// A `$b:block` fragment (rustc's `MetaVarKind::Block`): no labeled / unsafe block or closure body.
pub(crate) const BLOCK_FRAG: u32 = 1 << 27;
/// A `$p:path` fragment, reparsed with type-mode generic arguments.
pub(crate) const PATH_FRAG: u32 = 1 << 26;
/// the `(` a transcription puts around an expression fragment of several tokens (rustc's invisible
/// delimiters): never a call's argument list (`!div $n / 2` with `$n` = `64 / 2` is no `div(..)`)
pub(crate) const WRAP_PAREN: u32 = 1 << 25;
/// a `vis` fragment that matched nothing, forwarded here (rustc's empty invisible group): no tokens,
/// only the token it stands before (`Parser::empty_vis`)
pub(crate) const EMPTY_VIS: u32 = 1 << 24;
/// A `$s:stmt` fragment (rustc's `MetaVarKind::Stmt`): the statement it begins ends with it, no `;`
/// looked for after it. Both meta bits, as no fragment is an expression and a literal at once.
pub(crate) const STMT_FRAG: u32 = EXPR_FRAG | LIT_FRAG;
pub(crate) const FRAG_FLAGS: u32 = TY_FRAG | EXPR_FRAG | LIT_FRAG | META_ONLY | BLOCK_FRAG | PATH_FRAG | WRAP_PAREN | EMPTY_VIS;

/// The first name interned by every macro table.
const GLOB_ID: Name = Name(0);

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
        assert!(names.id(GLOB) == GLOB_ID);
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
    Import(Option<Vec<ExpUse>>),
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
static LOOKUP_INPUT_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
impl LookupInputs {
    fn new(table: &MacroTable) -> Self {
        Self { id: LOOKUP_INPUT_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed), graph: table.lookup_graph.clone(),
            reads: Default::default(), dependencies: Default::default() }
    }
    /// the inputs of a decoded lookup (MacroUse::from_replay): of no table's lineage, never
    /// unchanged (its replay resolves the lookup again)
    fn detached() -> Self {
        Self { id: LOOKUP_INPUT_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed), graph: std::sync::Arc::new(()),
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
    pub exp_uses: HashMap<String, std::sync::Arc<crate::fxhash::HashMap<(String, String), Vec<ExpUse>>>>,
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

    /// Whether an import that module `module` of crate `cur` sees binds `name` in the module
    /// `path` names from there (empty: `module` itself). The aliases map is the crate's, from
    /// every module's imports; an import is in scope only in its module, in the modules inside
    /// it that glob-import it, and through `pub` re-exports (rustc_resolve ident.rs
    /// `visit_scopes`: a plain name's module scope is the current module, not its parents).
    /// Returns the module it asked when the path resolves.
    pub(crate) fn import_binds(&self, cur: &str, module: &str, path: &[String], name: &str) -> (bool, Option<(String, String)>) {
        let r = Res { table: self };
        let Some((k, m)) = (if path.is_empty() { Some((cur, module)) } else { r.resolve_mod(cur, module, path) }) else { return (false, None) };
        let mut seen = Seen::new();
        (r.import_binds(k, m, (cur == k).then_some(module), name, &mut seen), Some((k.to_string(), m.to_string())))
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
        // When a bin takes a lib's crate name, keep the lib under `<name>\u{1}lib`
        // so Res::extern_crate can find it for every dependent.
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
    pub fn add_exp_uses(&mut self, k: &str, uses: impl IntoIterator<Item = ((String, String), Vec<ExpUse>)>) {
        self.replay_snapshot.0.get_mut().unwrap().clear();
        self.import_snapshot.0.get_mut().unwrap().clear();
        let t = std::sync::Arc::make_mut(self.exp_uses.entry(k.to_string()).or_default());
        // Imports are only added here, so crate k's expansion index (its binding names and
        // their modules) and the alias answers grow in place instead of being rebuilt from all
        // of k's imports on the next lookup (one rebuild per parsed file: 1.3 GB of copies on
        // a large app's cold start).
        let crates = self.expansion_snapshot.crates.get_mut().unwrap();
        let mut index = crates.get_mut(k).map(std::sync::Arc::make_mut);
        let aliases = self.expansion_snapshot.aliases.get_mut().unwrap();
        for (key, entries) in uses {
            if let Some(index) = index.as_mut() {
                if !index.names.contains(&key.1) { index.names.insert(key.1.clone()); }
                add_import_module(&mut index.modules, &key.0);
            }
            if let Some(found) = aliases.get_mut(&key.1) { *found = true; }
            let stored = t.entry(key).or_default();
            for entry in entries {
                if stored.last() == Some(&entry) { continue; }
                if let Some(pos) = stored.iter().position(|u| u == &entry) {
                    stored.remove(pos);
                }
                stored.push(entry);
            }
        }
    }

    /// Crate `k`'s expansion imports were replaced or removed (not through add_exp_uses): its
    /// cached expansion index and every alias answer go.
    pub(crate) fn forget_expansion_index(&mut self, k: &str) {
        self.expansion_snapshot.crates.get_mut().unwrap().remove(k);
        self.expansion_snapshot.aliases.get_mut().unwrap().clear();
    }

    /// `e`'s file has its new imports. Every crate's scopes are computed afresh on next use: a
    /// `pub use` chains into the crates that glob or name it (rustc_resolve's import fixpoint
    /// covers them all).
    pub fn apply_import_edit(&mut self, e: &ImportEdit) {
        let contexts: Vec<String> = self.crates.iter().filter(|(_, c)| c.src == e.root).map(|(k, _)| k.clone()).collect();
        for k in contexts {
            let mut cm = self.crates[&k].unresolved();
            cm.replace_imports(e);
            self.add_crate(k, std::sync::Arc::new(cm));
        }
        // An alias map is per crate: another file's import can hold the same one.
        for (alias, qual, target) in &e.was.aliases {
            let bound = self.crates.values().filter(|c| c.root == e.krate).flat_map(|c| c.uses.values().flatten())
                .any(|u| u.bind.as_ref() == Some(alias) && u.path.last() == Some(target));
            if !bound && self.alias(&e.krate, alias).is_some_and(|(q, t)| q == qual && t == target) {
                if let Some(map) = self.aliases.get_mut(&e.krate) { std::sync::Arc::make_mut(map).remove(alias); }
            }
        }
        for (alias, qual, target) in &e.now.aliases {
            self.add_alias(e.krate.clone(), alias.clone(), (qual.clone(), target.clone()));
        }
        let macro_use = self.macro_use.entry(e.krate.clone()).or_default();
        macro_use.retain(|m| !e.was.macro_use.contains(m));
        macro_use.extend(e.now.macro_use.iter().cloned());
        self.forget_scopes();
    }

    fn forget_scopes(&mut self) {
        let computed: Vec<String> = self.crates.iter().filter(|(_, c)| c.scope.get().is_some()).map(|(n, _)| n.clone()).collect();
        for n in computed {
            let fresh = self.crates[&n].unresolved();
            self.crates.insert(n, std::sync::Arc::new(fresh));
        }
    }

    /// Crate `k` is loaded: the imports its macro expansions wrote (`exp_uses`: derive output's
    /// `use Base as alias;`, `pub use self::m::ancestry as Name;`) join its prescanned ones, with
    /// the modules they sit in, so the crates loaded after it resolve macro paths through them
    /// (rustc resolves expansion-made imports like any other, rustc_resolve ident.rs). Its scope is
    /// computed afresh on next use, including the macros its expansions defined.
    pub fn seal_crate(&mut self, k: &str) {
        self.sealed.insert(k.to_string());
        let Some(old) = self.crates.get(k).cloned() else { return };
        // rustc_resolve/src/build_reduced_graph.rs::define_macro puts every #[macro_export]
        // in the root, including expansion output. A scope computed before file publication
        // lacks those definitions; rebuild it before a dependent crate starts parsing.
        let mut added = old.scope.get().is_some();
        let eu = self.exp_uses.get(k);
        if !added && eu.is_none() { return; }
        let mut uses = old.uses.clone();
        let mut renames = old.renames.clone();
        let mut mods = old.mods.clone();
        let mut externs = old.externs.clone();
        let mut binds: Vec<_> = eu.into_iter().flat_map(|u| u.iter()).collect();
        binds.sort_by(|a, b| a.0.cmp(b.0));
        for ((m, b), entries) in binds {
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
            for entry in entries {
                let path = &entry.path;
                let bind = (b != GLOB).then_some(b.as_str());
                if bind.is_some() && m == &old.root {
                    if path.len() == 1 && path[0] == "crate" {
                        externs.insert(b.clone(), k.to_string());
                    } else if path.len() == 2 && path[0].is_empty() {
                        if let Some(target) = extern_target(&externs, &path[1]) { externs.insert(b.clone(), target); }
                    }
                }
                if v.iter().any(|u| u.bind.as_deref() == bind && &u.path == path && u.vis == entry.vis) { continue; }
                v.push(UseItem { module: m.clone(), path: path.clone(), bind: bind.map(str::to_owned), vis: entry.vis.clone() });
                if bind.is_some() && path.last() != Some(b) { renames.insert(b.clone()); }
                added = true;
            }
        }
        if !added {
            return;
        }
        let cm = CrateMods { root: old.root.clone(), src: old.src.clone(), mods, items: old.items.clone(), inner_macro_use: old.inner_macro_use.clone(), uses, externs, renames, recursion_limit: old.recursion_limit, scope: Default::default() };
        self.add_crate(k.to_string(), std::sync::Arc::new(cm));
        // a crate not loaded yet whose scope a parse ahead of its load computed (preloaded files'
        // lookups) copied this crate's names as they were: computed afresh on next use
        let stale: Vec<String> = self.crates.iter().filter(|(n, c)| !self.sealed.contains(n.as_str()) && c.scope.get().is_some()).map(|(n, _)| n.clone()).collect();
        for n in stale {
            let fresh = self.crates[&n].unresolved();
            self.crates.insert(n, std::sync::Arc::new(fresh));
        }
    }
}

/// The binding name for expansion-made glob imports.
pub const GLOB: &str = "*";

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
        if let Some(d) = r.macro_in(cur, module, k, m, name) {
            return Some(d);
        }
        // Relax's std does not mirror std's module layout for its macros (`std::mem::offset_of!`
        // is core's root macro re-exported there): a std path names the std family's macro
        if matches!(k, "std" | "core" | "alloc") {
            return r.std_prelude(name);
        }
        return None;
    }
    // textual scope: the crate's own macro_rules that are in scope in this module (the position
    // within it is the parser's: MacroScope has none; Parser::rules_in_textual_scope)
    let cm = table.crates.get(cur);
    if let Some(d) = all().find(|d| d.krate == cur && cm.map_or(true, |c| c.in_textual_scope(d, module, None))) {
        return Some(d.clone());
    }
    lookup_after_textual(table, name, cur, module)
}

/// Resolve the remaining scopes after the parser has rejected closed textual definitions.
fn lookup_after_textual(table: &MacroTable, name: &str, cur: &str, module: &str) -> Option<MacroRef> {
    let r = Res { table };
    let cm = table.crates.get(cur);
    // the module's imports (non-glob, then glob)
    if let Some(d) = r.macro_in(cur, module, cur, module, name) {
        return Some(d);
    }
    if let Some(us) = table.macro_use.get(cur) {
        for u in us.iter().rev() {
            let u_key = cm.and_then(|c| c.externs.get(u.as_str())).map(|s| s.as_str()).unwrap_or(u.as_str());
            let (_, root) = r.crate_root(u_key);
            if let Some(d) = r.exported_in(u_key, root, name) {
                return Some(d);
            }
        }
    }
    r.std_prelude(name)
}

/// The modules `MacroScope::proc_macro_reach` finished, by (crate, module).
#[derive(Default)]
pub(crate) struct ProcReach(HashMap<(String, String), std::rc::Rc<crate::fxhash::HashSet<String>>>);

type MacroWalkSeen = crate::fxhash::HashSet<(String, String, String, usize)>;

struct Res<'a> {
    table: &'a MacroTable,
}

/// The (crate, module) pairs one `type_in` query has visited: a query visits a handful, so they
/// sit on the stack and compare by content; a query that walks a large glob web spills to a hash
/// set. Nothing is allocated or copied per lookup (the strings are borrowed from the scopes).
struct Seen<'b> {
    near: [(&'b str, &'b str, usize); 16],
    n: usize,
    spill: Option<HashMap<(&'b str, &'b str), usize>>,
}

impl<'b> Seen<'b> {
    fn new() -> Self { Self { near: [("", "", 0); 16], n: 0, spill: None } }

    fn insert(&mut self, k: &'b str, m: &'b str, req: Option<&str>) -> bool {
        let len = req.map_or(0, |r| common_mod_prefix(m, r).len());
        if self.spill.is_none() {
            for (prev_k, prev_m, widest) in &mut self.near[..self.n] {
                if (*prev_k, *prev_m) == (k, m) {
                    if *widest >= len { return false; }
                    *widest = len;
                    return true;
                }
            }
            if self.n < self.near.len() {
                self.near[self.n] = (k, m, len);
                self.n += 1;
                return true;
            }
            self.spill = Some(self.near.iter().map(|&(k, m, n)| ((k, m), n)).collect());
        }
        let spill = self.spill.as_mut().unwrap();
        if spill.get(&(k, m)).is_some_and(|&prev| prev >= len) { return false; }
        spill.insert((k, m), len);
        true
    }

    fn each(&self, mut f: impl FnMut(&'b str, &'b str)) {
        match &self.spill {
            Some(s) => s.keys().for_each(|&(k, m)| f(k, m)),
            None => self.near[..self.n].iter().for_each(|&(k, m, _)| f(k, m)),
        }
    }
}

impl<'a> Res<'a> {
    /// `#[macro_export] macro_rules! name` of crate `k` (an item of its root).
    fn exported(&self, k: &str, name: &str) -> Option<MacroRef> {
        self.table.defs.get(name)?.iter().rev().find(|d| d.krate == krate_name(k) && d.exported).cloned()
    }

    /// `#[macro_use] extern crate` imports public root macro bindings, including re-exports.
    fn exported_in(&self, k: &str, m: &str, name: &str) -> Option<MacroRef> {
        self.macro_in_vis(k, None, k, m, name).map(|(d, _)| d)
    }

    /// Resolve an import's prefix where it was written, then check its leaf
    /// against the visibility required by the caller and intervening imports.
    fn imported_macro<T>(&self, k: &str, m: &str, name: &str, req: Option<&str>, seen: &mut MacroWalkSeen,
        leaf: &impl Fn(&str, &str, &str) -> Option<T>) -> Option<T> {
        let depth = req.map_or(0, |r| common_mod_prefix(m, r).len());
        if !seen.insert((k.into(), m.into(), name.into(), depth)) { return None; }
        if let Some(value) = leaf(k, m, name) { return Some(value); }
        let cm = self.table.crates.get(k)?;
        let uses = cm.uses.get(m)?;
        let visible = |u: &UseItem| u.vis.resolve(&cm.root, m).visible_to_req(k, m, k, req);
        for u in uses.iter().filter(|u| u.bind.as_deref() == Some(name) && visible(u)) {
            let Some((target, prefix)) = u.path.split_last() else { continue; };
            let Some((tk, tm)) = self.import_prefix(k, m, prefix) else { continue; };
            if let Some(value) = self.imported_macro(tk, tm, target, next_req(k, m, tk, req), seen, leaf) { return Some(value); }
        }
        for u in uses.iter().filter(|u| u.bind.is_none() && visible(u)) {
            let Some((tk, tm)) = self.import_prefix(k, m, &u.path) else { continue; };
            if let Some(value) = self.imported_macro(tk, tm, name, next_req(k, m, tk, req), seen, leaf) { return Some(value); }
        }
        None
    }

    fn import_prefix<'b>(&self, k: &'b str, m: &'b str, path: &'b [String]) -> Option<(&'b str, &'b str)> where 'a: 'b {
        if path.is_empty() { Some((k, m)) } else { self.resolve_mod(k, m, path) }
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
        if let Some(c) = n.strip_prefix("__relax_crate_") {
            return Some(self.crate_root(c));
        }
        if let Some(c) = self.table.crates.get(k).and_then(|c| c.externs.get(n)) {
            // A bin cannot be a dependency, so every extern naming a shadowed lib refers to the lib.
            let lib = format!("{}\u{1}lib", c);
            if let Some((lib, _)) = self.table.crates.get_key_value(&lib) {
                return Some(self.crate_root(lib));
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
        self.cached_type(k, m, n, false)
    }

    fn type_in_as<'b>(&self, from: &str, req: Option<&str>, k: &'b str, m: &'b str, n: &str) -> Option<(&'b str, &'b str)>
    where 'a: 'b {
        if k == from && req.is_some_and(|r| within(r, m)) { return self.type_in(k, m, n); }
        let req = req.filter(|_| k == from);
        let sc = self.scope(k)?;
        if let Some((tk, tm, vis)) = sc.tnon.get(m).and_then(|t| t.get(n)) {
            return vis.visible_to_req(k, m, from, req).then_some((tk, tm));
        }
        if !sc.tglobs.contains_key(m) { return None; }
        if req.is_none() { return self.cached_type(k, m, n, true); }
        self.type_in_seen(k, m, n, req, None).map(|(tk, tm, _)| (tk, tm))
    }

    fn cached_type<'b>(&self, k: &'b str, m: &'b str, n: &str, public: bool) -> Option<(&'b str, &'b str)>
    where 'a: 'b {
        let binding = |at: &Option<(String, String)>| {
            let (c, at) = at.as_ref()?;
            let t = self.table.crates.get(c.as_str())?.scope.get()?.tnon.get(at.as_str())?.get(n)?;
            Some((t.0.as_str(), t.1.as_str()))
        };
        if !public {
            if let Some(at) = self.table.type_snapshot.0.read().unwrap().get(k).and_then(|c| c.get(m)).and_then(|t| t.get(n)) {
                return binding(at);
            }
        }
        let remember = |at| {
            if !public {
                self.table.type_snapshot.0.write().unwrap().entry(k.to_string()).or_default()
                    .entry(m.to_string()).or_default().insert(n.to_string(), at);
            }
        };
        let sc = self.scope(k)?;
        let memo = if public { &sc.pub_memo } else { &sc.memo };
        let scope_id = |c: &str| self.table.crates.get(c).and_then(|cm| cm.scope.get()).map_or(0, |s| s.id);
        if let Some(e) = memo.read().unwrap().get(m).and_then(|t| t.get(n)) {
            if e.deps.iter().all(|(c, id)| scope_id(c) == *id) {
                remember(e.at.clone());
                return binding(&e.at);
            }
        }
        let mut seen = Seen::new();
        let found = self.glob_find(k, m, n, (!public).then_some(m), &mut seen, None);
        let mut deps = Vec::new();
        seen.each(|c, _| {
            if !deps.iter().any(|(dep, _)| dep == c) { deps.push((c.to_string(), scope_id(c))); }
        });
        let at = found.map(|(c, at, _, _)| (c.to_string(), at.to_string()));
        remember(at.clone());
        memo.write().unwrap().entry(m.to_string()).or_default().insert(n.to_string(), TypeInMemo { at, deps: deps.into_boxed_slice() });
        found.map(|(_, _, t, _)| (t.0.as_str(), t.1.as_str()))
    }

    fn type_in_seen<'b>(&self, k: &'b str, m: &'b str, n: &str, req: Option<&str>, partial: Option<(&str, &'b CrateScope)>)
        -> Option<(&'b str, &'b str, ScopeVis)> where 'a: 'b {
        self.glob_find(k, m, n, req, &mut Seen::new(), partial)
            .map(|(_, _, t, vis)| (t.0.as_str(), t.1.as_str(), vis))
    }

    /// Return the defining module (for memo replay), its binding and the effective
    /// visibility through the globs that supplied it.
    fn glob_find<'b>(&self, k: &'b str, m: &'b str, n: &str, req: Option<&str>, seen: &mut Seen<'b>, partial: Option<(&str, &'b CrateScope)>)
        -> Option<(&'b str, &'b str, &'b (String, String, ScopeVis), ScopeVis)> where 'a: 'b {
        if !seen.insert(k, m, req) { return None; }
        let sc = match partial {
            Some((pk, sc)) if pk == k => sc,
            _ => self.scope(k)?,
        };
        if let Some(t) = sc.tnon.get(m).and_then(|t| t.get(n)) {
            return t.2.visible_to_req(k, m, k, req).then_some((k, m, t, t.2));
        }
        if partial.is_some_and(|(pk, _)| pk == k) && scope_waits_req(k, m, n, true, req) { return None; }
        for (k2, m2, edge) in sc.tglobs.get(m).into_iter().flatten() {
            if edge.visible_to_req(k, m, k, req) {
                if let Some((dk, dm, t, vis)) = self.glob_find(k2, m2, n, next_req(k, m, k2, req), seen, partial) {
                    return Some((dk, dm, t, edge.min(vis)));
                }
            }
        }
        None
    }

    fn macro_in_vis(&self, from: &str, req: Option<&str>, k: &str, m: &str, n: &str) -> Option<(MacroRef, ScopeVis)> {
        let sc = self.scope(k)?;
        sc.mnon.get(m).and_then(|t| t.get(n)).or_else(|| sc.mglob.get(m).and_then(|t| t.get(n)))
            .filter(|(_, vis)| vis.visible_to_req(k, m, from, req)).cloned()
    }

    fn macro_in(&self, from: &str, asker: &str, k: &str, m: &str, n: &str) -> Option<MacroRef> {
        self.macro_in_vis(from, Some(asker), k, m, n).map(|(d, _)| d)
    }

    fn import_binds<'b>(&self, k: &'b str, m: &'b str, req: Option<&str>, name: &str, seen: &mut Seen<'b>) -> bool
    where 'a: 'b {
        if !seen.insert(k, m, req) { return false; }
        let Some(cm) = self.table.crates.get(k) else { return false };
        let Some(sc) = self.scope(k) else { return false };
        if sc.mnon.get(m).and_then(|t| t.get(name)).is_some_and(|(_, v)| !v.visible_to_req(k, m, k, req)) { return false; }
        if cm.uses.get(m).into_iter().flatten().any(|u| {
            u.bind.as_deref() == Some(name) && u.vis.resolve(&cm.root, m).visible_to_req(k, m, k, req)
        }) {
            return true;
        }
        sc.tglobs.get(m).into_iter().flatten().any(|(k2, m2, edge)| {
            edge.visible_to_req(k, m, k, req) && self.import_binds(k2, m2, next_req(k, m, k2, req), name, seen)
        })
    }

    /// The module path `path` names from module `m` of crate `k` (type namespace), with
    /// `type_in` answering for the crate's own modules.
    fn resolve_mod<'b, S: AsRef<str>>(&self, k: &'b str, m: &'b str, path: &'b [S]) -> Option<(&'b str, &'b str)>
    where
        'a: 'b,
    {
        resolve_path(self, k, m, path, &|kk, mm, n| self.type_in_as(k, Some(m), kk, mm, n))
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
        // `::dep` (2018: the extern prelude); 2015: `::` is the crate root, `::m` its item `m`
        match path.get(1) {
            None => (r.crate_root(k), &path[1..]),
            Some(n) => match r.extern_crate(k, n.as_ref()) {
                Some(c) => (c, &path[2..]),
                None => (r.crate_root(k), &path[1..]),
            },
        }
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
                let vis = cm.items.get(m).map_or(ScopeVis::PUB, |it| it.vis.resolve(&cm.root, parent));
                sc.tnon.entry(parent.to_string()).or_default().insert(name.to_string(), (k.to_string(), m.clone(), vis));
            }
        }
        // the root's `#[macro_export]` macros (the latest definition of each name)
        let mut exported = HashMap::default();
        for (n, ds) in &table.defs {
            if let Some(d) = ds.iter().rev().find(|d| d.krate == krate_name(k) && d.exported) {
                exported.insert(n.clone(), (d.clone(), ScopeVis::PUB));
            }
        }
        sc.mnon.insert(cm.root.clone(), exported);
        let t_exp = t0.elapsed().as_secs_f64() * 1e3;
        let mut passes = 0;
        let mut copies = 0usize;
        let empty: Vec<UseItem> = Vec::new();
        // A wider binding needs another copy even when the source's name count is unchanged.
        let mod_index: HashMap<&str, usize> = mods.iter().enumerate().map(|(i, m)| (m.as_str(), i)).collect();
        let mut rev = vec![1usize; mods.len()];
        let mut copied: HashMap<(usize, usize), usize> = HashMap::default();
        // imports whose answer is final (found, or resolved into another crate's finished scope)
        let mut settled: crate::fxhash::HashSet<(usize, usize)> = crate::fxhash::HashSet::default();
        // rustc's fixpoint: a name a glob gives is undetermined while a single import of the module
        // may still bind it (ScopeRun). A stall makes the imports that failed without waiting stop
        // making names wait; one with none of those lifts the waits until the next binding
        let outer = SCOPE_RUN.with(|c| c.replace(Some(ScopeRun::new(table, k, &mods, cm))));
        let mut force = false;
        let (mut waits, mut determined, mut forced) = (0usize, 0usize, 0usize);
        loop {
            let mut changed = false;
            let mut waited = false;
            // the single imports of this pass that failed without waiting
            let mut failing: Vec<(usize, usize)> = Vec::new();
            passes += 1;
            for (mi, m) in mods.iter().enumerate() {
                for (ui, u) in cm.uses.get(m).unwrap_or(&empty).iter().enumerate() {
                    if settled.contains(&(mi, ui)) {
                        continue;
                    }
                    if force && changed {
                        force = false;
                        forced += 1;
                    }
                    scope_try(mi, ui, !force);
                    let import_vis = sc.resolve_use_vis(&r, k, &cm.root, m, &u.vis);
                    match &u.bind {
                        Some(b) => {
                            let (may_type, may_macro) = use_may_bind(table, u);
                            if !may_type && !may_macro {
                                settled.insert((mi, ui));
                                continue;
                            }
                            // where the path's prefix leads: another crate's module is final
                            let prefix_crate = match u.path.split_last() {
                                Some((_, prefix)) if !prefix.is_empty() && !(prefix.len() == 1 && prefix[0].is_empty()) => {
                                    sc.import_module(&r, k, m, prefix).map(|x| x.0)
                                }
                                _ => None,
                            };
                            if scope_take_wait() {
                                waited = true;
                                waits += 1;
                                continue;
                            }
                            if prefix_crate.map_or(false, |c| c != k) {
                                // (its answer is final: what it does not bind now, it never binds)
                                settled.insert((mi, ui));
                                scope_final();
                            }
                            // a module / crate under that name
                            let mut type_waited = false;
                            if may_type && sc.tnon.get(m).and_then(|t| t.get(b)).is_none_or(|old| import_vis.is_wider_than(old.2)) {
                                let target = sc.import_type(&r, k, m, &u.path).map(|(tk, tm, vis)| (tk.to_string(), tm.to_string(), import_vis.min(vis)));
                                type_waited = scope_take_wait();
                                if let Some(target) = target.filter(|_| !type_waited) {
                                    let bindings = sc.tnon.entry(m.clone()).or_default();
                                    if bindings.get(b).is_none_or(|old| old.0 == target.0 && old.1 == target.1 && target.2.is_wider_than(old.2)) {
                                        bindings.insert(b.clone(), target);
                                        changed = true;
                                    }
                                }
                            }
                            let mut macro_waited = false;
                            if may_macro && sc.mnon.get(m).and_then(|t| t.get(b)).is_none_or(|old| import_vis.is_wider_than(old.1)) {
                                let d = match u.path.split_last() {
                                    Some((last, prefix)) if prefix.is_empty() || (prefix.len() == 1 && prefix[0].is_empty()) => {
                                        // `use name;` of a macro in scope (2018 `pub use mac;`): the latest
                                        // macro_rules of the name in this module's textual scope (one of
                                        // the same name in another module is another macro)
                                        table.defs.get(last).and_then(|ds| ds.iter().rev().find(|d| d.krate == krate_name(k) && cm.in_textual_scope(d, m, None)).cloned())
                                            .map(|d| {
                                                let vis = if d.exported { ScopeVis::PUB } else { ScopeVis::krate(&cm.root, m) };
                                                (d, vis)
                                            })
                                    }
                                    Some((last, prefix)) => {
                                        let t = sc.import_module(&r, k, m, prefix);
                                        t.and_then(|(k2, m2)| sc.lookup_macro(&r, k, m, k2, m2, last))
                                    }
                                    None => None,
                                };
                                macro_waited = scope_take_wait();
                                if let Some((d, src_vis)) = d.filter(|_| !macro_waited) {
                                    let vis = import_vis.min(src_vis);
                                    let bindings = sc.mnon.entry(m.clone()).or_default();
                                    if bindings.get(b).is_none_or(|(old, v)| std::sync::Arc::ptr_eq(old, &d) && vis.is_wider_than(*v)) {
                                        bindings.insert(b.clone(), (d, vis));
                                        rev[mi] += 1;
                                        changed = true;
                                    }
                                }
                            }
                            if type_waited || macro_waited {
                                waited = true;
                                waits += 1;
                            } else {
                                failing.push((mi, ui));
                            }
                        }
                        None => {
                            let t = sc.import_module(&r, k, m, &u.path).map(|(a, b)| (a.to_string(), b.to_string()));
                            if scope_take_wait() {
                                waited = true;
                                waits += 1;
                                continue;
                            }
                            let Some((k2, m2)) = t else { continue };
                            if (k2.as_str(), m2.as_str()) == (k, m.as_str()) {
                                continue;
                            }
                            // another crate's module: final, copied once
                            if k2 != k {
                                settled.insert((mi, ui));
                            }
                            let src_rev = mod_index.get(m2.as_str()).map_or(0, |&j| rev[j]);
                            let srcs = sc.tglobs.entry(m.clone()).or_default();
                            match srcs.iter_mut().find(|x| x.0 == k2 && x.1 == m2) {
                                Some(x) if import_vis.is_wider_than(x.2) => { x.2 = import_vis; changed = true; }
                                Some(_) => {}
                                None => { srcs.push((k2.clone(), m2.clone(), import_vis)); changed = true; }
                            }
                            if k2 == k && copied.get(&(mi, ui)) == Some(&src_rev) { continue; }
                            let macros = sc.all_macros_of(&r, k, m, &k2, &m2);
                            if scope_take_wait() {
                                waited = true;
                                waits += 1;
                            } else if k == k2 {
                                copied.insert((mi, ui), src_rev);
                            }
                            copies += macros.len();
                            for (name, d, src_vis) in macros {
                                if sc.mnon.get(m).is_some_and(|t| t.contains_key(&name)) { continue; }
                                let vis = import_vis.min(src_vis);
                                let bindings = sc.mglob.entry(m.clone()).or_default();
                                if bindings.get(&name).is_none_or(|(_, old)| vis.is_wider_than(*old)) {
                                    bindings.insert(name, (d, vis));
                                    rev[mi] += 1;
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }
            if changed {
                if force {
                    force = false;
                    forced += 1;
                }
                continue;
            }
            // (a stall where nothing waited is the end)
            if force || !waited {
                break;
            }
            let failed = SCOPE_RUN.with(|c| {
                let mut c = c.borrow_mut();
                let run = c.as_mut().expect("the scope run of this compute");
                failing.iter().filter(|&&x| run.failed.insert(x)).count()
            });
            if failed > 0 {
                determined += failed;
                continue;
            }
            force = true;
        }
        SCOPE_RUN.with(|c| c.replace(outer));
        if crate::preload::trace() {
            eprintln!("boot-trace-mscope-detail: {} exported {:.2} ms, {} passes, {} copied, {} mods, {} uses; {} tries waited, {} failures determined, {} forced", k, t_exp, passes, copies, mods.len(), cm.uses.values().map(|v| v.len()).sum::<usize>(), waits, determined, forced);
        }
        sc
    }

    fn resolve_use_vis(&self, r: &Res, k: &str, root: &str, m: &str, vis: &UseVis) -> ScopeVis {
        if let Some(v) = vis.ancestor(root, m) { return v; }
        if let UseVis::In(path) = vis {
            if let Some((tk, tm)) = self.import_module(r, k, m, path) {
                if tk == k {
                    if let Some(v) = ScopeVis::in_ancestor(m, tm) { return v; }
                }
            }
        }
        ScopeVis::private(m)
    }

    fn lookup_type<'b>(&'b self, r: &Res<'b>, k: &str, m: &str, kk: &'b str, mm: &'b str, n: &str)
        -> Option<(&'b str, &'b str, ScopeVis)> {
        r.type_in_seen(kk, mm, n, (kk == k).then_some(m), Some((k, self)))
    }

    fn import_module<'b>(&'b self, r: &Res<'b>, k: &'b str, m: &'b str, path: &'b [String]) -> Option<(&'b str, &'b str)> {
        resolve_path(r, k, m, path, &|kk, mm, n| self.lookup_type(r, k, m, kk, mm, n).map(|(a, b, _)| (a, b)))
    }

    /// Only the leaf contributes its visibility. A public item can be exported
    /// through an intermediate module private to its importer.
    fn import_type<'b>(&'b self, r: &Res<'b>, k: &'b str, m: &'b str, path: &'b [String]) -> Option<(&'b str, &'b str, ScopeVis)> {
        let (last, prefix) = path.split_last()?;
        if matches!(last.as_str(), "crate" | "self" | "super") || last.starts_with("__relax_crate_") {
            return self.import_module(r, k, m, path).map(|(a, b)| (a, b, ScopeVis::PUB));
        }
        if prefix.is_empty() {
            return self.lookup_type(r, k, m, k, m, last)
                .or_else(|| r.extern_crate(k, last).map(|(a, b)| (a, b, ScopeVis::PUB)));
        }
        if prefix.len() == 1 && prefix[0].is_empty() {
            if let Some((a, b)) = r.extern_crate(k, last) { return Some((a, b, ScopeVis::PUB)); }
        }
        let (tk, tm) = self.import_module(r, k, m, prefix)?;
        self.lookup_type(r, k, m, tk, tm, last)
    }

    fn lookup_macro(&self, r: &Res, k: &str, asker: &str, kk: &str, mm: &str, n: &str) -> Option<(MacroRef, ScopeVis)> {
        if kk != k { return r.macro_in_vis(k, Some(asker), kk, mm, n); }
        self.mnon.get(mm).and_then(|t| t.get(n)).or_else(|| {
            if scope_waits_req(k, mm, n, false, Some(asker)) { return None; }
            self.mglob.get(mm)?.get(n)
        }).filter(|(_, v)| v.visible_to(kk, mm, k, asker)).cloned()
    }

    fn all_macros_of(&self, r: &Res, k: &str, asker: &str, kk: &str, mm: &str) -> Vec<(String, MacroRef, ScopeVis)> {
        let sc = if kk == k { self } else {
            let Some(sc) = r.scope(kk) else { return Vec::new(); };
            sc
        };
        let singles = sc.mnon.get(mm);
        let mut macros = Vec::new();
        for (glob, bindings) in [(false, singles), (true, sc.mglob.get(mm))] {
            for (name, (d, vis)) in bindings.into_iter().flatten() {
                if !vis.visible_to(kk, mm, k, asker) { continue; }
                if glob && (singles.is_some_and(|t| t.contains_key(name))
                    || (kk == k && scope_waits_req(k, mm, name, false, Some(asker)))) { continue; }
                macros.push((name.clone(), d.clone(), *vis));
            }
        }
        macros
    }
}

/// Whether `use` item `u` may bind a module / crate, and a macro: only a name some module / crate
/// has can bind one; only a macro name (or a rename) can bind a macro.
fn use_may_bind(table: &MacroTable, u: &UseItem) -> (bool, bool) {
    let last = u.path.last().map_or("", |x| x.as_str());
    (table.mod_names.contains(last) || last == "self" || last == "super" || last == "crate", table.defs.contains_key(last) || table.renames.contains(last))
}

/// This thread's CrateScope::compute (a dependency's scope, computed on demand inside one, runs
/// in its place and hands it back): the crate's single imports by module and the name they bind,
/// the import being resolved, the imports found to fail whatever the others bind, whether the
/// lookups wait (`block`: off for a forced step) and whether one of the import's did.
struct ScopeRun {
    krate: String,
    mods: Vec<String>,
    /// name -> the single imports binding it: (module index, use index, may bind a module, may bind
    /// a macro, `pub`)
    single: HashMap<String, Vec<(usize, usize, bool, bool, ScopeVis)>>,
    current: (usize, usize),
    block: bool,
    waited: bool,
    failed: crate::fxhash::HashSet<(usize, usize)>,
}

impl ScopeRun {
    fn new(table: &MacroTable, k: &str, mods: &[String], cm: &CrateMods) -> ScopeRun {
        let mut single: HashMap<String, Vec<(usize, usize, bool, bool, ScopeVis)>> = HashMap::default();
        for (mi, m) in mods.iter().enumerate() {
            for (ui, u) in cm.uses.get(m).map_or(&[][..], |v| &v[..]).iter().enumerate() {
                let Some(b) = &u.bind else { continue };
                let (may_type, may_macro) = use_may_bind(table, u);
                if may_type || may_macro {
                    single.entry(b.clone()).or_default().push((mi, ui, may_type, may_macro, u.vis.resolve(&cm.root, m)));
                }
            }
        }
        ScopeRun { krate: k.to_string(), mods: mods.to_vec(), single, current: (usize::MAX, 0), block: true, waited: false, failed: Default::default() }
    }
}

thread_local! {
    static SCOPE_RUN: std::cell::RefCell<Option<ScopeRun>> = const { std::cell::RefCell::new(None) };
}

/// The scope run tries import `ui` of module `mi`; `block`: its lookups wait.
fn scope_try(mi: usize, ui: usize, block: bool) {
    SCOPE_RUN.with(|c| {
        if let Some(run) = c.borrow_mut().as_mut() {
            run.current = (mi, ui);
            run.block = block;
            run.waited = false;
        }
    });
}

/// The import being tried has its final answer: it makes no name wait any more.
fn scope_final() {
    SCOPE_RUN.with(|c| {
        if let Some(run) = c.borrow_mut().as_mut() {
            let cur = run.current;
            run.failed.insert(cur);
        }
    });
}

/// Whether a lookup of the import being tried waited since the last ask (and clears it).
fn scope_take_wait() -> bool {
    SCOPE_RUN.with(|c| c.borrow_mut().as_mut().is_some_and(|run| std::mem::take(&mut run.waited)))
}

/// Whether name `n` of module `m` of crate `k`, missed by the module's own bindings of that
/// namespace (`type_ns`: modules and crates, else macros), waits: crate `k`'s scope is being
/// computed and a single import of `m` other than the one being tried may still bind it -- one
/// visible from the module of the one being tried (`pub`, or that module is inside `m`) whose
/// failure is not determined (rustc ident.rs: undetermined while such an import may still define
/// the name). Marks the try waited.
fn scope_waits_req(k: &str, m: &str, n: &str, type_ns: bool, req: Option<&str>) -> bool {
    SCOPE_RUN.with(|c| {
        let mut c = c.borrow_mut();
        let Some(run) = c.as_mut() else { return false };
        if !run.block {
            return false;
        }
        let Some(v) = run.single.get(n) else { return false };
        if run.krate != k {
            return false;
        }
        let waits = v.iter().any(|&(mi, ui, may_type, may_macro, vis)| {
            run.mods[mi] == m && (mi, ui) != run.current && (if type_ns { may_type } else { may_macro })
                && vis.visible_to_req(k, m, k, req) && !run.failed.contains(&(mi, ui))
        });
        if waits {
            run.waited = true;
        }
        waits
    })
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
    uses_log: std::cell::RefCell<Vec<((Name, Name), Vec<LocalExpUse>)>>,
    memo: std::cell::RefCell<LookupMemo>,
    forwarders: std::cell::RefCell<HashMap<(String, String, String), MacroRef>>,
    /// `builtin_macro`'s answers, by (crate, module, path), for the table they were read from
    builtin_paths: std::cell::RefCell<Option<(std::sync::Arc<MacroTable>, HashMap<(String, String, Vec<String>), Option<String>>)>>,
}

/// Deferred contexts share these bindings; individual lookups only retain a
/// journal position. Build the index lazily, then extend it as bindings are added,
/// including during replay, without enumerating the whole map after every write.
/// Names are ids of the owning table's `MacroNames`.
#[derive(Default)]
struct LocalImports {
    bindings: HashMap<(Name, Name), Vec<LocalExpUse>>,
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
    fn entry(&mut self, names: &MacroNames, key: (Name, Name)) -> &mut Vec<LocalExpUse> {
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
            bindings: self.bindings.iter().map(|(&(m, b), p)| ((tr(m), tr(b)), p.iter().map(|u| u.remap(tr)).collect())).collect(),
            index: Default::default(),
        }
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
    /// a lookup decoded from a file image's replay data (MacroUse::from_replay): its answer by
    /// what expansions depend on, not by object (`got` is None then)
    was: Option<Option<GotWas>>,
}

/// A recorded lookup's answer as a later run compares it: the definition's expansion_hash and
/// where it is written (None in Looked::was: nothing was found)
#[derive(Clone)]
struct GotWas {
    hash: u128,
    def_file: String,
    def_pos: u32,
}

/// A finished parse's definitions (to append to the table in load order) and its lookups.
#[derive(Clone)]
pub struct MacroUse {
    trusted_image: bool,
    pub local: Vec<MacroRef>,
    looked: Vec<Looked>,
    uses_base: std::sync::Arc<LocalImports>,
    uses_log: Vec<((Name, Name), Vec<LocalExpUse>)>,
    /// the interner `looked`, `uses_base` and `uses_log` are in (None: an image's, nothing to replay)
    names: Option<std::sync::Arc<MacroNames>>,
    /// the parse's `use` bindings (MacroScope::add_use), for the crate's table
    pub uses: crate::fxhash::HashMap<(String, String), Vec<ExpUse>>,
    /// decoded from a file image's replay data (from_replay): those bytes, recorded again as they are
    replay_image: Option<std::sync::Arc<[u8]>>,
}

fn ptr_of(d: &Option<MacroRef>) -> usize {
    d.as_ref().map_or(0, |d| std::sync::Arc::as_ptr(d) as usize)
}

impl MacroScope {
    pub fn context(&self) -> MacroContext {
        MacroContext { table: self.table.clone(), local: self.local.clone(), uses: self.local_uses.borrow().clone() }
    }

    pub fn from_context(ctx: &MacroContext) -> Self {
        Self { table: ctx.table.clone(), local: ctx.local.clone(), local_uses: std::cell::RefCell::new(ctx.uses.clone()), looked: Default::default(), uses_base: ctx.uses.clone(), uses_log: Default::default(), memo: Default::default(), forwarders: Default::default(), builtin_paths: Default::default() }
    }

    pub fn new(table: std::sync::Arc<MacroTable>) -> MacroScope {
        MacroScope { table, local: Vec::new(), looked: Default::default(), local_uses: Default::default(), uses_base: Default::default(), uses_log: Default::default(), memo: Default::default(), forwarders: Default::default(), builtin_paths: Default::default() }
    }

    pub(crate) fn define_tokens(&mut self, name: &str, krate: &str, edition: u16, file: &str, module: &str, pos: u32, scope_pos: Option<u32>, exported: bool, local_inner: bool, tokens: &[MTok]) -> Result<(), String> {
        let same = self.table.defs.get(name).into_iter().flatten().rev().chain(self.local.iter().rev()).find(|d| {
            d.name == name && d.krate == krate && d.edition == edition && d.def_file == file && d.def_pos == pos && d.scope_pos == scope_pos && d.exported == exported && d.builtin == 0
                && d.input.as_ref().is_some_and(|input| input.local_inner == local_inner && input.tokens == tokens)
        }).cloned();
        if let Some(d) = same {
            uses::defined(&d);
            self.local.push(d);
            *self.memo.get_mut() = LookupMemo::default();
        } else {
            let mut d = parse_def(name, krate, edition, tokens)?;
            d.exported = exported;
            if local_inner { local_inner_macros(&mut d); }
            d.def_file = file.into();
            d.def_pos = pos;
            d.scope_pos = scope_pos;
            d.def_mod = module.into();
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
        uses::defined(&d);
        let same = self.table.defs.get(&d.name).and_then(|v| {
            v.iter().rev().find(|x| x.krate == d.krate && x.exported == d.exported && x.edition == d.edition && x.builtin == d.builtin && x.arms == d.arms && x.def_file == d.def_file && x.def_pos == d.def_pos && x.scope_pos == d.scope_pos && x.def_mod == d.def_mod).cloned()
        });
        self.local.push(same.unwrap_or_else(|| std::sync::Arc::new(d)));
        *self.memo.get_mut() = LookupMemo::default();
    }

    /// `use path as bind;` (or `use path;`) parsed in module `module`.
    pub fn add_use(&self, module: &str, bind: String, path: Vec<String>, vis: UseVis) {
        let names = &*self.table.names;
        let key = (names.id(module), names.id(&bind));
        let entry = LocalExpUse { path: path.iter().map(|p| names.id(p)).collect(), vis };
        if self.local_uses.borrow().bindings.get(&key).is_some_and(|v| v.last() == Some(&entry)) { return; }
        let mut uses = self.local_uses.borrow_mut();
        let v = std::sync::Arc::make_mut(&mut uses).entry(names, key);
        if let Some(pos) = v.iter().position(|u| u == &entry) { v.remove(pos); }
        v.push(entry);
        self.uses_log.borrow_mut().push((key, v.clone()));
        *self.memo.borrow_mut() = LookupMemo::default();
        *self.builtin_paths.borrow_mut() = None;
    }

    /// A glob import parsed in this module. The same path may have several visibilities.
    pub fn add_glob_use(&self, module: &str, path: Vec<String>, vis: UseVis) {
        let names = &*self.table.names;
        let key = (names.id(module), GLOB_ID);
        let entry = LocalExpUse { path: path.iter().map(|p| names.id(p)).collect(), vis };
        if self.local_uses.borrow().bindings.get(&key).is_some_and(|v| v.contains(&entry)) { return; }
        let mut uses = self.local_uses.borrow_mut();
        let v = std::sync::Arc::make_mut(&mut uses).entry(names, key);
        v.push(entry);
        self.uses_log.borrow_mut().push((key, v.clone()));
        *self.memo.borrow_mut() = LookupMemo::default();
        *self.builtin_paths.borrow_mut() = None;
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
        ImportLookup::new(self, cur, &mut memo.imports, inputs).lookup(name, cur, path, module)
    }

    fn extern_shadowed(&self, origin: Name, cur: Name, module: Name, path: &[Name]) -> bool {
        let Some(&first) = path.first() else { return false };
        let names = &*self.table.names;
        let fs = names.str(first);
        if matches!(fs, "crate" | "self" | "super" | "") || fs.starts_with("__relax_crate_") {
            return false;
        }
        let (cur_s, mod_s) = (names.str(cur), names.str(module));
        if (Res { table: &self.table }).extern_crate(cur_s, fs).is_none() {
            return false;
        }
        if cur == origin {
            let local = self.local_uses.borrow();
            if !local.bindings.is_empty() {
                if local.bindings.contains_key(&(module, first))
                    || local.bindings.contains_key(&(module, GLOB_ID))
                    || names.get(&format!("{mod_s}::{fs}")).is_some_and(|c| local.index(names).modules.contains(&c))
                {
                    return true;
                }
            }
        }
        self.table.exp_uses.get(cur_s).is_some_and(|u| {
            u.contains_key(&(mod_s.to_string(), fs.to_string()))
                || u.contains_key(&(mod_s.to_string(), GLOB.to_string()))
                || self.table.expansion_module(cur_s, &format!("{mod_s}::{fs}"))
        })
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
        let shadowed = self.extern_shadowed(cur, cur, module, path);
        if let Some(&first) = path.first() {
            let fs = names.str(first);
            if !matches!(fs, "crate" | "self" | "super" | "") && !fs.starts_with("__relax_crate_")
                && (Res { table: &self.table }).extern_crate(names.str(cur), fs).is_some()
            {
                inputs.read(&self.table, LookupInput::Import(cur, module, first));
                inputs.read(&self.table, LookupInput::Import(cur, module, GLOB_ID));
                inputs.read(&self.table, LookupInput::GeneratedModule(cur, names.id(&format!("{}::{fs}", names.str(module)))));
            }
        }
        let r = (!shadowed)
            .then(|| with_strs(names, path, |path| lookup_in(&self.table, &self.local, names.str(name), names.str(cur), path, names.str(module))))
            .flatten()
            .or_else(|| self.lookup_via_local_use(name, cur, path, module, &mut memo, &mut inputs));
        if record {
            self.looked.borrow_mut().push(Looked { name, path: key.path.clone(), module, nlocal: self.local.len() as u32, nuses: self.uses_log.borrow().len() as u32, got: r.clone(), inputs: std::sync::Arc::new(inputs), was: None });
        }
        memo.queries.insert(key, r.clone());
        r
    }

    pub fn lookup(&self, name: &str, cur: &str, path: &[String], module: &str) -> Option<MacroRef> {
        let names = &*self.table.names;
        let path: Box<[Name]> = path.iter().map(|p| names.id(p)).collect();
        self.resolve(names.id(name), names.id(cur), &path, names.id(module), true)
    }

    /// The parser owns block positions; after excluding every textual candidate it must
    /// continue through imports and both preludes, rather than retain the closed definition.
    pub(crate) fn lookup_after_textual(&self, name: &str, cur: &str, module: &str) -> Option<MacroRef> {
        lookup_after_textual(&self.table, name, cur, module).or_else(|| {
            let names = &*self.table.names;
            let mut memo = self.memo.borrow_mut();
            let mut inputs = LookupInputs::new(&self.table);
            self.lookup_via_local_use(names.id(name), names.id(cur), &[], names.id(module), &mut memo, &mut inputs)
        })
    }

    /// `MacroTable::import_binds`, with the imports macro expansions wrote: this parse's and the
    /// crate's earlier ones (a binding of the name in that module, or any glob of theirs there:
    /// expansion-made globs are not followed, so one is taken to bind it).
    pub(crate) fn import_binds(&self, cur: &str, module: &str, path: &[String], name: &str) -> bool {
        let (bound, at) = self.table.import_binds(cur, module, path, name);
        if bound {
            return true;
        }
        let Some((k, m)) = at else { return false };
        if k != cur { return false; }
        let r = Res { table: &self.table };
        let req = Some(module);
        let names = &*self.table.names;
        let mid = names.id(&m);
        let local = self.local_uses.borrow();
        let any_local = |v: &[LocalExpUse]| v.iter().any(|u| exp_vis_visible(&r, &k, &m, &u.vis, req));
        let any_exp = |v: &[ExpUse]| v.iter().any(|u| exp_vis_visible(&r, &k, &m, &u.vis, req));
        if local.bindings.get(&(mid, names.id(name))).is_some_and(|v| any_local(v))
            || self.table.exp_uses.get(cur).and_then(|t| t.get(&(m.clone(), name.to_string()))).is_some_and(|v| any_exp(v))
        {
            return true;
        }
        if r.scope(&k).is_some_and(|sc| {
            sc.mnon.get(&m).and_then(|t| t.get(name)).is_some_and(|(_, v)| !v.visible_to_req(&k, &m, &k, req))
        }) {
            return false;
        }
        local.bindings.get(&(mid, GLOB_ID)).is_some_and(|v| any_local(v))
            || self.table.exp_uses.get(cur).and_then(|t| t.get(&(m.clone(), GLOB.to_string()))).is_some_and(|v| any_exp(v))
    }

    /// An explicit binding, or an attribute exported through a glob. The outer Option keeps
    /// an explicit import of the builtin from falling through to an outer scope's proc macro.
    pub(crate) fn proc_macro_import(&self, cur: &str, module: &str, uses: &[UseItem], name: &str, kind: crate::macro_reg::Kind) -> Option<Option<crate::macro_reg::Entry>> {
        let mut seen = crate::fxhash::HashSet::default();
        let mut explicit = false;
        for u in uses.iter().filter(|u| u.bind.as_deref() == Some(name)) {
            explicit = true;
            if let Some(entry) = self.proc_macro_path(cur, module, &u.path, kind, &mut seen) { return Some(Some(entry)); }
        }
        if explicit { return Some(None); }
        // A glob conflicts with a same-named prelude builtin (rustc_resolve ident.rs:
        // AmbiguityKind::GlobVsOuter). Keep that builtin for now (a check gap), but a
        // hosted macro with no builtin competitor is imported normally.
        let builtin = match kind {
            crate::macro_reg::Kind::Derive => crate::parser::derive::trait_path(name).is_some(),
            crate::macro_reg::Kind::Attr => matches!(name, "test" | "bench" | "test_case"),
            crate::macro_reg::Kind::Bang => false,
        };
        for u in uses.iter().filter(|u| u.bind.is_none()) {
            let mut path = u.path.clone();
            path.push(name.into());
            if let Some(entry) = self.proc_macro_path(cur, module, &path, kind, &mut seen) { return Some(if builtin { None } else { Some(entry) }); }
        }
        None
    }

    pub(crate) fn attr_path(&self, cur: &str, module: &str, path: &[String], seen: &mut MacroWalkSeen) -> Option<crate::macro_reg::Entry> {
        self.proc_macro_path(cur, module, path, crate::macro_reg::Kind::Attr, seen)
    }

    pub(crate) fn proc_macro_path(&self, cur: &str, module: &str, path: &[String], kind: crate::macro_reg::Kind, seen: &mut MacroWalkSeen) -> Option<crate::macro_reg::Entry> {
        let (name, prefix) = path.split_last()?;
        let r = Res { table: &self.table };
        let (k, m) = if prefix.is_empty() { (cur, module) } else { r.resolve_mod(cur, module, prefix)? };
        self.proc_macro_in(k, m, name, kind, (k == cur).then_some(module), seen)
    }

    /// `builtin_macro_path` of a lookup of its own (no walk in progress): the answer reads only
    /// the table, so each (crate, module, path) is walked once per table.
    pub fn builtin_macro(&self, cur: &str, module: &str, path: &[String]) -> Option<String> {
        let key = (cur.to_string(), module.to_string(), path.to_vec());
        if let Some((table, known)) = self.builtin_paths.borrow().as_ref() {
            if std::sync::Arc::ptr_eq(table, &self.table) {
                if let Some(r) = known.get(&key) { return r.clone(); }
            }
        }
        let r = self.builtin_macro_path(cur, module, path, &mut Default::default());
        let mut memo = self.builtin_paths.borrow_mut();
        let (_, known) = match &mut *memo {
            Some(m) if std::sync::Arc::ptr_eq(&m.0, &self.table) => m,
            m => m.insert((self.table.clone(), HashMap::default())),
        };
        known.insert(key, r.clone());
        r
    }

    fn resolve_mod_path(&self, origin: &str, k: &str, m: &str, prefix: &[String]) -> Option<(String, String)> {
        if prefix.is_empty() { return Some((k.to_string(), m.to_string())); }
        let r = Res { table: &self.table };
        let names = &*self.table.names;
        let origin_id = names.id(origin);
        let k_id = names.id(k);
        let m_id = names.id(m);
        let ids: Vec<Name> = prefix.iter().map(|s| names.id(s)).collect();
        if !self.extern_shadowed(origin_id, k_id, m_id, &ids) {
            if let Some((tk, tm)) = r.resolve_mod(k, m, prefix) {
                return Some((tk.to_string(), tm.to_string()));
            }
        }
        let mut memo = self.memo.borrow_mut();
        if memo.origin != origin_id { *memo = LookupMemo { origin: origin_id, ..Default::default() }; }
        let mut inputs = LookupInputs::new(&self.table);
        let (tk, tm) = ImportLookup::new(self, origin_id, &mut memo.imports, &mut inputs)
            .resolve_mod(k_id, m_id, &ids)?;
        Some((names.str(tk).to_string(), names.str(tm).to_string()))
    }

    /// Whether the qualified call `path` of a built-in the parser reads in place (`std::vec!`)
    /// reaches a built-in, as rustc's resolve of the path must: its module part names a module
    /// (else rustc's E0433 at the segment that names nothing), of a std crate, whose built-ins
    /// Relax takes by name, or one that re-exports the built-in (builtin_macro). A plain name: true.
    pub fn reaches_builtin(&self, cur: &str, module: &str, path: &[String]) -> bool {
        let Some((_, prefix)) = path.split_last() else { return false };
        if prefix.is_empty() { return true; }
        match self.resolve_mod_path(cur, cur, module, prefix) {
            None => false,
            Some((k, _)) => matches!(krate_name(&k), "core" | "std" | "alloc") || self.builtin_macro(cur, module, path).is_some(),
        }
    }

    /// Follow standard macro re-exports as well as re-exported library modules.
    pub(crate) fn builtin_macro_path(&self, cur: &str, module: &str, path: &[String], seen: &mut MacroWalkSeen) -> Option<String> {
        let (name, prefix) = path.split_last()?;
        let (k, m) = self.resolve_mod_path(cur, cur, module, prefix)?;
        self.builtin_macro_in(cur, &k, &m, name, (k == cur).then_some(module), seen)
    }

    fn builtin_macro_in(&self, origin: &str, k: &str, m: &str, name: &str, req: Option<&str>, seen: &mut MacroWalkSeen) -> Option<String> {
        let depth = req.map_or(0, |r| common_mod_prefix(m, r).len());
        if !seen.insert((k.into(), m.into(), name.into(), depth)) { return None; }
        let r = Res { table: &self.table };
        let standard = matches!(k, "core" | "std" | "alloc") && m == k
            && (crate::program::BUILTIN_MACROS.contains(&name) || name == "cfg_select" || (k == "std" && name == "dbg"));
        let smallvec = krate_name(k) == "smallvec" && name == "smallvec" && r.crate_root(k).1 == m;
        if standard || smallvec { return Some(name.to_string()); }
        if let Some(cm) = self.table.crates.get(k) {
            if let Some(uses) = cm.uses.get(m) {
                let visible = |u: &UseItem| exp_vis_visible(&r, k, m, &u.vis, req);
                for u in uses.iter().filter(|u| u.bind.as_deref() == Some(name) && visible(u)) {
                    let Some((target, prefix)) = u.path.split_last() else { continue };
                    let Some((tk, tm)) = self.resolve_mod_path(origin, k, m, prefix) else { continue };
                    if let Some(v) = self.builtin_macro_in(origin, &tk, &tm, target, next_req(k, m, &tk, req), seen) { return Some(v); }
                }
            }
        }
        let names = &*self.table.names;
        let mut exp_single: Vec<Vec<String>> = Vec::new();
        let mut exp_globs: Vec<Vec<String>> = Vec::new();
        if k == origin {
            let local = self.local_uses.borrow();
            if let Some(mid) = names.get(m) {
                if let Some(nid) = names.get(name) {
                    if let Some(entries) = local.bindings.get(&(mid, nid)) {
                        for u in entries.iter().rev() {
                            if exp_vis_visible(&r, k, m, &u.vis, req) {
                                exp_single.push(u.path.iter().map(|&id| names.str(id).to_string()).collect());
                            }
                        }
                    }
                }
                if let Some(entries) = local.bindings.get(&(mid, GLOB_ID)) {
                    for u in entries {
                        if exp_vis_visible(&r, k, m, &u.vis, req) {
                            exp_globs.push(u.path.iter().map(|&id| names.str(id).to_string()).collect());
                        }
                    }
                }
            }
        }
        if let Some(eu) = self.table.exp_uses.get(k) {
            if let Some(entries) = eu.get(&(m.to_string(), name.to_string())) {
                for u in entries.iter().rev() {
                    if exp_vis_visible(&r, k, m, &u.vis, req) { exp_single.push(u.path.clone()); }
                }
            }
            if let Some(entries) = eu.get(&(m.to_string(), GLOB.to_string())) {
                for u in entries {
                    if exp_vis_visible(&r, k, m, &u.vis, req) { exp_globs.push(u.path.clone()); }
                }
            }
        }
        for path in exp_single {
            let Some((target, prefix)) = path.split_last() else { continue };
            let Some((tk, tm)) = self.resolve_mod_path(origin, k, m, prefix) else { continue };
            if let Some(v) = self.builtin_macro_in(origin, &tk, &tm, target, next_req(k, m, &tk, req), seen) { return Some(v); }
        }
        if let Some(cm) = self.table.crates.get(k) {
            if let Some(uses) = cm.uses.get(m) {
                let visible = |u: &UseItem| exp_vis_visible(&r, k, m, &u.vis, req);
                for u in uses.iter().filter(|u| u.bind.is_none() && visible(u)) {
                    let Some((tk, tm)) = self.resolve_mod_path(origin, k, m, &u.path) else { continue };
                    if let Some(v) = self.builtin_macro_in(origin, &tk, &tm, name, next_req(k, m, &tk, req), seen) { return Some(v); }
                }
            }
        }
        for path in exp_globs {
            let Some((tk, tm)) = self.resolve_mod_path(origin, k, m, &path) else { continue };
            if let Some(v) = self.builtin_macro_in(origin, &tk, &tm, name, next_req(k, m, &tk, req), seen) { return Some(v); }
        }
        None
    }

    /// core's test attribute macros live in each edition's prelude. Resolve the
    /// container first so module aliases work and a local `core` cannot impersonate it.
    pub(crate) fn builtin_test_path(&self, cur: &str, module: &str, path: &[String]) -> bool {
        let Some((name, prefix)) = path.split_last() else { return false; };
        if !matches!(name.as_str(), "test" | "bench" | "test_case") { return false; }
        let r = Res { table: &self.table };
        let Some((k, m)) = r.resolve_mod(cur, module, prefix) else { return false; };
        matches!(k, "core" | "std") && m.strip_prefix(k).is_some_and(|m| matches!(m,
            "::prelude::v1" | "::prelude::rust_2015" | "::prelude::rust_2018" | "::prelude::rust_2021" | "::prelude::rust_2024"))
    }

    /// Compiler derives are exported by core and re-exported alongside their traits.
    /// Resolve the container and re-exports before recognizing the builtin spelling.
    pub(crate) fn builtin_derive_path(&self, cur: &str, module: &str, path: &[String], seen: &mut MacroWalkSeen) -> bool {
        let Some((name, prefix)) = path.split_last() else { return false; };
        let r = Res { table: &self.table };
        let is_builtin = |k: &str, m: &str, name: &str| {
            let suffix = m.strip_prefix(k).unwrap_or(m).trim_start_matches("::");
            if !matches!(k, "core" | "std") { return false; }
            let Some(trait_path) = crate::parser::derive::trait_path(name) else { return false; };
            let container = trait_path.rsplit_once("::").map(|(m, _)| m).unwrap_or("");
            suffix.is_empty() || suffix == container || matches!(suffix,
                "prelude::v1" | "prelude::rust_2015" | "prelude::rust_2018" | "prelude::rust_2021" | "prelude::rust_2024")
        };
        let Some((k, m)) = (if prefix.is_empty() { Some((cur, module)) } else { r.resolve_mod(cur, module, prefix) }) else { return false; };
        r.imported_macro(k, m, name, (k == cur).then_some(module), seen,
            &|k, m, name| is_builtin(k, m, name).then_some(())).is_some()
    }

    /// The proc macro `name` of `kind` that module `m` of crate `k` binds as another crate sees it:
    /// its crate's own export at its root, or one its `pub use` imports re-export.
    pub(crate) fn public_proc_macro(&self, k: &str, m: &str, name: &str, kind: crate::macro_reg::Kind) -> Option<crate::macro_reg::Entry> {
        self.proc_macro_in(k, m, name, kind, None, &mut Default::default())
    }

    fn proc_macro_in(&self, k: &str, m: &str, name: &str, kind: crate::macro_reg::Kind, req: Option<&str>, seen: &mut MacroWalkSeen) -> Option<crate::macro_reg::Entry> {
        let r = Res { table: &self.table };
        r.imported_macro(k, m, name, req, seen, &|k, m, name| {
            if r.crate_root(k).1 == m { crate::macro_reg::exported_proc_macro(krate_name(k), name, kind) } else { None }
        })
    }

    /// The names of `names` a `proc_macro_in` walk from module `m` of crate `k` can find a proc macro
    /// for, or more, never fewer: the names the module's imports bind, those its crate's root
    /// exports, and those of every module its glob imports read, through glob cycles. A name
    /// outside it has no proc macro there, so it needs no walk. (Every import counts, `pub` or not:
    /// the walk's visibility and its `seen` set only take names away.)
    pub(crate) fn proc_macro_reach(&self, k: &str, m: &str, names: &crate::fxhash::HashSet<String>, memo: &mut ProcReach) -> std::rc::Rc<crate::fxhash::HashSet<String>> {
        let first = (k.to_string(), m.to_string());
        if let Some(x) = memo.0.get(&first) { return x.clone(); }
        let r = Res { table: &self.table };
        // the modules the globs reach that are not finished: each one's own names and glob targets
        // (a module of this walk, or a finished set)
        let mut keys: Vec<(String, String)> = vec![first.clone()];
        let mut index: HashMap<(String, String), usize> = HashMap::default();
        index.insert(first.clone(), 0);
        let mut sets: Vec<crate::fxhash::HashSet<String>> = Vec::new();
        let mut reads: Vec<Vec<Result<usize, std::rc::Rc<crate::fxhash::HashSet<String>>>>> = Vec::new();
        let mut i = 0;
        while i < keys.len() {
            let (k, m) = keys[i].clone();
            let mut own = crate::fxhash::HashSet::default();
            if r.crate_root(&k).1 == m {
                for (n, _) in crate::macro_reg::exported_proc_macros(krate_name(&k)) {
                    if names.contains(&n) { own.insert(n); }
                }
            }
            let mut read = Vec::new();
            if let Some(uses) = self.table.crates.get(&k).and_then(|c| c.uses.get(&m)) {
                for u in uses.iter() {
                    if let Some(b) = u.bind.as_deref() {
                        if names.contains(b) { own.insert(b.to_string()); }
                        continue;
                    }
                    let target = if u.path.is_empty() { Some((k.as_str(), m.as_str())) } else { r.resolve_mod(&k, &m, &u.path) };
                    let Some((k2, m2)) = target else { continue };
                    let key = (k2.to_string(), m2.to_string());
                    if let Some(x) = memo.0.get(&key) { read.push(Err(x.clone())); continue; }
                    let j = match index.get(&key) {
                        Some(&j) => j,
                        None => { index.insert(key.clone(), keys.len()); keys.push(key); keys.len() - 1 }
                    };
                    read.push(Ok(j));
                }
            }
            sets.push(own);
            reads.push(read);
            i += 1;
        }
        // each module's names grow by those of the modules it reads until none grows (the sets only
        // grow, within `names`)
        loop {
            let mut grew = false;
            for a in (0..keys.len()).rev() {
                let mut add: Vec<String> = Vec::new();
                for x in &reads[a] {
                    let from = match x { Ok(j) => &sets[*j], Err(done) => &**done };
                    add.extend(from.iter().filter(|n| !sets[a].contains(*n)).cloned());
                }
                if !add.is_empty() { grew = true; sets[a].extend(add); }
            }
            if !grew { break; }
        }
        for (key, set) in keys.into_iter().zip(sets) { memo.0.insert(key, std::rc::Rc::new(set)); }
        memo.0[&first].clone()
    }

    pub(crate) fn proc_macro_use(&self, cur: &str, name: &str, kind: crate::macro_reg::Kind) -> Option<crate::macro_reg::Entry> {
        let r = Res { table: &self.table };
        for dep in self.table.macro_use.get(cur)?.iter().rev() {
            let Some((k, m)) = r.extern_crate(cur, dep) else { continue };
            if let Some(e) = self.proc_macro_in(k, m, name, kind, None, &mut Default::default()) { return Some(e); }
        }
        None
    }

    pub fn finish(self) -> MacroUse {
        let names = self.table.names.clone();
        let uses = std::sync::Arc::unwrap_or_clone(self.local_uses.into_inner()).bindings.into_iter()
            .map(|((m, b), entries)| {
                let entries = entries.into_iter().map(|u| ExpUse {
                    path: u.path.into_iter().map(|n| names.str(n).to_owned()).collect(), vis: u.vis,
                }).collect();
                ((names.str(m).to_owned(), names.str(b).to_owned()), entries)
            })
            .collect();
        MacroUse { trusted_image: false, local: self.local, looked: self.looked.into_inner(), uses_base: self.uses_base, uses_log: self.uses_log.into_inner(), names: Some(names), uses, replay_image: None }
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
    /// The required visibility inside `krate`; None is a public-only lookup.
    req: Option<Name>,
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
    queued: bool,
    /// Local module state `(Module, k, m, name, Some(m))` permanently settled as `None`
    /// after `ready` drained, unlocking extern-prelude fallback for its waiters.
    extern_open: bool,
    /// Self-named bare import (`pub use dep;`) whose in-scope globs settled without binding it.
    self_extern_open: bool,
    /// Deduplication bit for `ImportLookup::self_extern_wait`.
    self_extern_waiting: bool,
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
    /// `(parent, local_state)` pairs waiting for `local_state` to settle as `None`
    /// before falling back to `r.extern_crate` (deduplicated by `extern_wait_set`).
    extern_wait: Vec<(Option<usize>, usize)>,
    extern_wait_set: crate::fxhash::HashSet<(Option<usize>, usize)>,
    /// Self-named bare imports (`pub use dep;` in a module that also has globs)
    /// waiting for all non-self-bare work to settle before falling back to `r.extern_crate`.
    self_extern_wait: Vec<usize>,
    /// Set while `step` evaluates visible single imports if any single import deferred to
    /// `extern_wait` or `self_extern_wait`, so globs do not race ahead of it.
    step_waited_extern: bool,
}

impl<'a> ImportLookup<'a> {
    fn new(
        scope: &'a MacroScope,
        origin: Name,
        memo: &'a mut HashMap<ImportKey, ImportAnswer>,
        inputs: &'a mut LookupInputs,
    ) -> Self {
        Self {
            scope,
            origin,
            memo,
            inputs,
            states: Vec::new(),
            seen: Default::default(),
            ready: Default::default(),
            extern_wait: Vec::new(),
            extern_wait_set: Default::default(),
            self_extern_wait: Vec::new(),
            step_waited_extern: false,
        }
    }

    fn enqueue(&mut self, i: usize) {
        let s = &mut self.states[i];
        if s.value.is_none() && !s.extern_open && !s.queued {
            s.queued = true;
            self.ready.push_back(i);
        }
    }

    fn record_extern_wait(&mut self, parent: Option<usize>, j: usize) {
        self.step_waited_extern = true;
        if parent == Some(j) {
            return;
        }
        if self.extern_wait_set.insert((parent, j)) {
            self.extern_wait.push((parent, j));
        }
    }

    fn record_self_extern_wait(&mut self, p: usize) {
        self.step_waited_extern = true;
        if !self.states[p].self_extern_waiting {
            self.states[p].self_extern_waiting = true;
            self.self_extern_wait.push(p);
        }
    }

    fn ask(&mut self, ns: ImportNs, k: Name, m: Name, n: Name, req: Option<Name>, parent: Option<usize>) -> Option<ImportValue> {
        let key = ImportKey { ns, krate: k, module: m, name: n, req };
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
                self.states.push(ImportState {
                    key,
                    value: None,
                    waiting: Vec::new(),
                    local: k == self.origin,
                    queued: true,
                    extern_open: false,
                    self_extern_open: false,
                    self_extern_waiting: false,
                });
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

    /// Query `(ImportNs::Module, k, m, n, req)`. Returns `Ok((tk, tm))` if resolved,
    /// `Err(None)` if definitively absent from cache, or `Err(Some(state_idx))` if still
    /// pending in this fixpoint.
    fn ask_module(
        &mut self,
        k: Name,
        m: Name,
        n: Name,
        req: Option<Name>,
        parent: Option<usize>,
    ) -> Result<(Name, Name), Option<usize>> {
        let key = ImportKey { ns: ImportNs::Module, krate: k, module: m, name: n, req };
        if let Some(answer) = self.memo.get(&key) {
            self.inputs.depend(&answer.inputs);
            if answer.local {
                if let Some(parent) = parent { self.states[parent].local = true; }
            }
            return match answer.value {
                Some(ImportValue::Module(tk, tm)) => Ok((tk, tm)),
                None => Err(None),
                Some(ImportValue::Macro(_)) => unreachable!(),
            };
        }
        if let Some(answer) = self.scope.table.import_snapshot.0.read().unwrap()
            .get(&self.origin).and_then(|memo| memo.get(&key))
        {
            self.inputs.depend(&answer.inputs);
            return match answer.value {
                Some(ImportValue::Module(tk, tm)) => Ok((tk, tm)),
                None => Err(None),
                Some(ImportValue::Macro(_)) => unreachable!(),
            };
        }
        let i = match self.seen.get(&key) {
            Some(&i) => i,
            None => {
                let i = self.states.len();
                self.seen.insert(key, i);
                self.states.push(ImportState {
                    key,
                    value: None,
                    waiting: Vec::new(),
                    local: k == self.origin,
                    queued: true,
                    extern_open: false,
                    self_extern_open: false,
                    self_extern_waiting: false,
                });
                self.ready.push_back(i);
                i
            }
        };
        let state = &mut self.states[i];
        if let Some(parent) = parent {
            if !state.waiting.contains(&parent) { state.waiting.push(parent); }
        }
        match state.value {
            Some(ImportValue::Module(tk, tm)) => Ok((tk, tm)),
            None => Err(Some(i)),
            Some(ImportValue::Macro(_)) => unreachable!(),
        }
    }

    fn module_name(&mut self, k: Name, m: Name, n: Name, req: Option<Name>, parent: Option<usize>) -> Option<(Name, Name)> {
        self.ask_module(k, m, n, req, parent).ok()
    }

    /// Resolve a bare first segment `n` in `(k, m)`'s own scope first; fall back to
    /// `r.extern_crate` only once `(Module, k, m, n, Some(m))` is definitively `None`.
    fn bare_or_extern(&mut self, k: Name, m: Name, n: Name, parent: Option<usize>) -> Option<(Name, Name)> {
        match self.ask_module(k, m, n, Some(m), parent) {
            Ok(at) => Some(at),
            Err(pending) => {
                let names = &*self.scope.table.names;
                let r = Res { table: &self.scope.table };
                let ext = r.extern_crate(names.str(k), names.str(n))
                    .map(|(ek, em)| (names.id(ek), names.id(em)))?;
                match pending {
                    None => Some(ext),
                    Some(j) if self.states[j].extern_open => Some(ext),
                    Some(j) => {
                        self.record_extern_wait(parent, j);
                        None
                    }
                }
            }
        }
    }

    fn path(&mut self, k: Name, m: Name, path: &[Name], parent: Option<usize>) -> Option<(Name, Name)> {
        if path.is_empty() { return Some((k, m)); }
        let scope = self.scope;
        let names = &*scope.table.names;
        let r = Res { table: &scope.table };
        let ids = |(k, m): (&str, &str)| (names.id(k), names.id(m));
        let (ks, ms) = (names.str(k), names.str(m));
        let first = names.str(path[0]);
        let (mut at, rest) = match first {
            "crate" => (ids(r.crate_root(ks)), &path[1..]),
            "self" => ((k, m), &path[1..]),
            "super" => ((k, names.id(ms.rsplit_once("::")?.0)), &path[1..]),
            "" => match path.get(1).and_then(|&n| r.extern_crate(ks, names.str(n))) {
                Some(c) => (ids(c), &path[2..]),
                None => (ids(r.crate_root(ks)), &path[1..]),
            },
            _ => {
                let at = if let Some(defining) = first.strip_prefix("__relax_crate_") {
                    ids(r.crate_root(defining))
                } else {
                    self.bare_or_extern(k, m, path[0], parent)?
                };
                (at, &path[1..])
            }
        };
        for &n in rest {
            at = match names.str(n) {
                "self" => at,
                "super" => (at.0, names.id(names.str(at.1).rsplit_once("::")?.0)),
                _ => self.module_name(at.0, at.1, n, (at.0 == k).then_some(m), parent)?,
            };
        }
        Some(at)
    }

    /// Visible single imports, latest first, and hidden expansion-made paths that may shadow a glob.
    fn bindings(&mut self, k: Name, m: Name, n: Name, req: Option<&str>) -> (Vec<Vec<Name>>, Vec<Vec<Name>>) {
        let key = (m, n);
        let scope = self.scope;
        let mine = scope.local_uses.borrow();
        let table = &scope.table;
        let names = &*table.names;
        let r = Res { table };
        let (ks, ms, ns) = (names.str(k), names.str(m), names.str(n));
        let mut visible = Vec::new();
        let mut hidden = Vec::new();
        let push_unique = |dst: &mut Vec<Vec<Name>>, path: Vec<Name>| {
            if !dst.contains(&path) { dst.push(path); }
        };
        if k == self.origin {
            if let Some(entries) = mine.bindings.get(&key) {
                for u in entries.iter().rev() {
                    if exp_vis_visible(&r, ks, ms, &u.vis, req) {
                        push_unique(&mut visible, u.path.clone());
                    } else {
                        push_unique(&mut hidden, u.path.clone());
                    }
                }
            }
        }
        self.inputs.read(table, LookupInput::Import(k, m, n));
        let ids = |path: &[String]| path.iter().map(|s| names.id(s)).collect::<Vec<Name>>();
        if let Some(entries) = table.exp_uses.get(ks).and_then(|u| u.get(&(ms.to_string(), ns.to_string()))) {
            for u in entries.iter().rev() {
                let path = ids(&u.path);
                if exp_vis_visible(&r, ks, ms, &u.vis, req) {
                    push_unique(&mut visible, path);
                } else {
                    push_unique(&mut hidden, path);
                }
            }
        }
        if let Some(cm) = table.crates.get(ks) {
            for u in cm.uses.get(ms).into_iter().flatten().rev() {
                if u.bind.as_deref() == Some(ns) && exp_vis_visible(&r, ks, ms, &u.vis, req) {
                    push_unique(&mut visible, ids(&u.path));
                }
            }
        }
        (visible, hidden)
    }

    fn globs(&mut self, k: Name, m: Name, req: Option<&str>) -> Vec<Vec<Name>> {
        let key = (m, GLOB_ID);
        let scope = self.scope;
        let mine = scope.local_uses.borrow();
        let table = &scope.table;
        let names = &*table.names;
        let r = Res { table };
        self.inputs.read(table, LookupInput::Import(k, m, GLOB_ID));
        let (ks, ms) = (names.str(k), names.str(m));
        let ids = |path: &[String]| path.iter().map(|s| names.id(s)).collect::<Vec<Name>>();
        let mut out = Vec::new();
        let mut push_unique = |path: Vec<Name>| {
            if !out.contains(&path) { out.push(path); }
        };
        if k == self.origin {
            for u in mine.bindings.get(&key).into_iter().flatten() {
                if exp_vis_visible(&r, ks, ms, &u.vis, req) { push_unique(u.path.clone()); }
            }
        }
        if let Some(entries) = table.exp_uses.get(ks).and_then(|u| u.get(&(ms.to_string(), GLOB.to_string()))) {
            for u in entries {
                if exp_vis_visible(&r, ks, ms, &u.vis, req) { push_unique(ids(&u.path)); }
            }
        }
        if let Some(cm) = table.crates.get(ks) {
            for u in cm.uses.get(ms).into_iter().flatten().filter(|u| u.bind.is_none()
                && exp_vis_visible(&r, ks, ms, &u.vis, req)) {
                push_unique(ids(&u.path));
            }
        }
        out
    }

    /// Resolve one visible `ImportNs::Module` binding `path` in `(k, m)` for bound name `n`
    /// under caller visibility `req`. In-scope module bindings shadow same-named extern crates
    /// even when the caller cannot see them; leaf visibility is checked under `next_req`.
    fn resolve_module_binding(
        &mut self,
        k: Name,
        m: Name,
        n: Name,
        path: &[Name],
        req: Option<&str>,
        parent: Option<usize>,
    ) -> Option<ImportValue> {
        let scope = self.scope;
        let table = &scope.table;
        let names = &*table.names;
        let (ks, ms) = (names.str(k), names.str(m));
        let r = Res { table };
        let (&last, prefix) = path.split_last()?;
        let leaf = names.str(last);
        if matches!(leaf, "crate" | "self" | "super") || leaf.starts_with("__relax_crate_") {
            return self.path(k, m, path, parent).map(|(tk, tm)| ImportValue::Module(tk, tm));
        }
        let next = |target: Name| next_req(ks, ms, names.str(target), req).map(|p| names.id(p));
        // `::dep` queries the extern prelude directly in 2018+, unshadowed by local modules.
        if matches!(prefix, [root] if names.str(*root).is_empty()) {
            if let Some((ek, em)) = r.extern_crate(ks, leaf) {
                return Some(ImportValue::Module(names.id(ek), names.id(em)));
            }
        }
        if prefix.is_empty() {
            if last == n {
                // Self-named bare import (`pub use dep;`): check in-scope globs before the
                // extern prelude (asking `(k, m, n)` directly would self-cycle).
                let ext = r.extern_crate(ks, leaf)
                    .map(|(ek, em)| ImportValue::Module(names.id(ek), names.id(em)));
                let globs = self.globs(k, m, Some(ms));
                if !globs.is_empty() {
                    let mut all_settled = true;
                    for gpath in globs {
                        let Some((gk, gm)) = self.path(k, m, &gpath, parent) else {
                            all_settled = false;
                            continue;
                        };
                        match self.ask_module(gk, gm, last, (gk == k).then_some(m), parent) {
                            Ok(_) => return self.ask(ImportNs::Module, gk, gm, last, next(gk), parent),
                            Err(Some(j)) if !self.states[j].extern_open => {
                                all_settled = false;
                            }
                            Err(_) => {}
                        }
                    }
                    if !all_settled {
                        let ext = ext?;
                        let p = parent?;
                        if !self.states[p].self_extern_open {
                            self.record_self_extern_wait(p);
                            return None;
                        }
                        return Some(ext);
                    }
                }
                return ext;
            }
            // Bare leaf (`use foo as bar;`): check `(k, m)`'s own module scope first.
            // If bound there (even privately/restricted), suppress extern fallback and enforce `next(k)`.
            return match self.ask_module(k, m, last, Some(m), parent) {
                Ok(_) => self.ask(ImportNs::Module, k, m, last, next(k), parent),
                Err(pending) => {
                    let ext = r.extern_crate(ks, leaf)
                        .map(|(ek, em)| ImportValue::Module(names.id(ek), names.id(em)))?;
                    match pending {
                        None => Some(ext),
                        Some(j) if self.states[j].extern_open => Some(ext),
                        Some(j) => {
                            self.record_extern_wait(parent, j);
                            None
                        }
                    }
                }
            };
        }
        let (tk, tm) = self.path(k, m, prefix, parent)?;
        self.ask(ImportNs::Module, tk, tm, last, next(tk), parent)
    }

    fn resolve_macro_binding(&mut self, k: Name, m: Name, path: &[Name], req: Option<&str>, parent: Option<usize>) -> Option<ImportValue> {
        let scope = self.scope;
        let table = &scope.table;
        let names = &*table.names;
        let (ks, ms) = (names.str(k), names.str(m));
        let r = Res { table };
        let (&last, prefix) = path.split_last()?;
        if prefix.is_empty() {
            self.inputs.read(table, LookupInput::Definitions(last));
            self.inputs.read(table, LookupInput::Prelude(k));
            let last_s = names.str(last);
            let cm = table.crates.get(ks);
            let local = if k == self.origin { &scope.local[..] } else { &[] };
            if let Some(d) = table.defs.get(last_s).map_or(&[][..], |v| &v[..]).iter()
                .chain(local.iter().filter(|d| d.name == last_s)).rev()
                .find(|d| d.krate == krate_name(ks) && cm.map_or(true, |c| c.in_textual_scope(d, ms, None)))
                .filter(|d| d.exported || req.is_some())
            {
                return Some(ImportValue::Macro(d.clone()));
            }
            let inner_req = next_req(ks, ms, ks, req);
            if let Some(d) = r.macro_in_vis(ks, inner_req, ks, ms, last_s).map(|(d, _)| d)
                .or_else(|| table.macro_use.get(ks).and_then(|us| us.iter().rev().find_map(|u| {
                    let uk = cm.and_then(|c| c.externs.get(u.as_str())).map_or(u.as_str(), |s| s.as_str());
                    r.exported_in(uk, r.crate_root(uk).1, last_s)
                })))
                .or_else(|| r.std_prelude(last_s))
            {
                return Some(ImportValue::Macro(d));
            }
        }
        let (tk, tm) = self.path(k, m, prefix, parent)?;
        let next = next_req(ks, ms, names.str(tk), req).map(|p| names.id(p));
        self.ask(ImportNs::Macro, tk, tm, last, next, parent)
    }

    /// A hidden expansion-made single shadows globs only in the namespace it binds.
    fn hidden_binding_shadows(&mut self, ns: ImportNs, k: Name, m: Name, path: &[Name]) -> bool {
        let scope = self.scope;
        let table = &scope.table;
        let names = &*table.names;
        let (ks, ms) = (names.str(k), names.str(m));
        let r = Res { table };
        match ns {
            ImportNs::Module => with_strs(names, path, |ps| r.resolve_mod(ks, ms, ps).is_some()),
            ImportNs::Macro => {
                let Some((&last, prefix)) = path.split_last() else { return false };
                let last_s = names.str(last);
                if prefix.is_empty() {
                    self.inputs.read(table, LookupInput::Definitions(last));
                    self.inputs.read(table, LookupInput::Prelude(k));
                    let local = if k == self.origin { &scope.local[..] } else { &[] };
                    return lookup_in(table, local, last_s, ks, &[] as &[&str], ms).is_some();
                }
                with_strs(names, prefix, |ps| {
                    let (tk, tm) = r.resolve_mod(ks, ms, ps)?;
                    if tk == ks && k == self.origin && r.crate_root(tk).1 == tm
                        && scope.local.iter().any(|d| d.exported && d.krate == krate_name(ks) && d.name == last_s)
                    {
                        return Some(());
                    }
                    r.macro_in(ks, ms, tk, tm, last_s).map(|_| ())
                        .or_else(|| matches!(tk, "std" | "core" | "alloc").then(|| r.std_prelude(last_s)).flatten().map(|_| ()))
                }).is_some()
            }
        }
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
        let ImportKey { ns, krate: k, module: m, name: n, req } = self.states[i].key;
        let scope = self.scope;
        let table = &scope.table;
        let names = &*table.names;
        let (ks, ms, nm) = (names.str(k), names.str(m), names.str(n));
        let r = Res { table };
        let req = req.map(|n| names.str(n));
        let next = |target| next_req(ks, ms, names.str(target), req).map(|m| names.id(m));
        if let Some(sc) = r.scope(ks) {
            let direct = match ns {
                ImportNs::Module => sc.tnon.get(ms).and_then(|t| t.get(nm)).map(|t| t.2),
                ImportNs::Macro => sc.mnon.get(ms).and_then(|t| t.get(nm)).map(|t| t.1),
            };
            if direct.is_some_and(|v| !v.visible_to_req(ks, ms, ks, req)) { return None; }
        }
        match ns {
            ImportNs::Module => {
                if let Some((k2, m2)) = r.type_in_as(ks, req, ks, ms, nm) { return Some(ImportValue::Module(names.id(k2), names.id(m2))); }
                let child = names.id(&format!("{ms}::{nm}"));
                if self.generated_module(k, child) { return Some(ImportValue::Module(k, child)); }
            }
            ImportNs::Macro => {
                if k == self.origin && r.crate_root(ks).1 == ms {
                    if let Some(d) = scope.local.iter().rev().find(|d| d.exported && d.krate == ks && d.name == nm) {
                        return Some(ImportValue::Macro(d.clone()));
                    }
                }
                if let Some((d, _)) = r.macro_in_vis(ks, req, ks, ms, nm) { return Some(ImportValue::Macro(d)); }
                if matches!(ks, "std" | "core" | "alloc") {
                    self.inputs.read(table, LookupInput::Definitions(n));
                    if let Some(d) = r.std_prelude(nm) { return Some(ImportValue::Macro(d)); }
                }
            }
        }
        let (visible, hidden) = self.bindings(k, m, n, req);
        self.step_waited_extern = false;
        for path in visible {
            let found = match ns {
                ImportNs::Module => self.resolve_module_binding(k, m, n, &path, req, Some(i)),
                ImportNs::Macro => self.resolve_macro_binding(k, m, &path, req, Some(i)),
            };
            if let Some(value) = found { return Some(value); }
        }
        if hidden.iter().any(|path| self.hidden_binding_shadows(ns, k, m, path)) { return None; }
        if self.step_waited_extern {
            return None;
        }
        for path in self.globs(k, m, req) {
            if let Some((k, m)) = self.path(k, m, &path, Some(i)) {
                if let Some(value) = self.ask(ns, k, m, n, next(k), Some(i)) { return Some(value); }
            }
        }
        None
    }

    fn drain_ready(&mut self) {
        while let Some(i) = self.ready.pop_front() {
            self.states[i].queued = false;
            if self.states[i].value.is_some() || self.states[i].extern_open { continue; }
            if let Some(value) = self.step(i) {
                self.states[i].value = Some(value);
                let waiting = std::mem::take(&mut self.states[i].waiting);
                for &w in &waiting {
                    self.enqueue(w);
                }
                self.states[i].waiting = waiting;
            }
        }
    }

    fn open_extern_targets(&mut self, open_mask: &[bool]) -> bool {
        for (j, &open) in open_mask.iter().enumerate() {
            if open {
                self.states[j].extern_open = true;
            }
        }
        let mut progressed = false;
        let mut keep = Vec::new();
        for (p, j) in std::mem::take(&mut self.extern_wait) {
            if open_mask[j] {
                self.extern_wait_set.remove(&(p, j));
                if let Some(p) = p {
                    self.enqueue(p);
                }
                progressed = true;
            } else {
                keep.push((p, j));
            }
        }
        self.extern_wait = keep;
        progressed
    }

    fn unlock_externs(&mut self) -> bool {
        self.extern_wait.retain(|&(p, j)| {
            let keep = self.states[j].value.is_none()
                && !self.states[j].extern_open
                && p.is_none_or(|p| self.states[p].value.is_none() && !self.states[p].extern_open);
            if !keep {
                self.extern_wait_set.remove(&(p, j));
            }
            keep
        });
        self.self_extern_wait.retain(|&s| {
            let keep = self.states[s].value.is_none()
                && !self.states[s].extern_open
                && !self.states[s].self_extern_open;
            if !keep {
                self.states[s].self_extern_waiting = false;
            }
            keep
        });

        let n = self.states.len();
        if !self.extern_wait.is_empty() {
            let mut self_blocked = vec![false; n];
            if !self.self_extern_wait.is_empty() {
                let mut work = std::collections::VecDeque::new();
                for &s in &self.self_extern_wait {
                    if !self_blocked[s] {
                        self_blocked[s] = true;
                        work.push_back(s);
                    }
                }
                while let Some(u) = work.pop_front() {
                    for &w in &self.states[u].waiting {
                        if self.states[w].value.is_none() && !self.states[w].extern_open && !self_blocked[w] {
                            self_blocked[w] = true;
                            work.push_back(w);
                        }
                    }
                }
            }

            // Tier 1: release every unblocked extern target `j` that no OTHER pending extern
            // target `k != j` can reach via `extern_wait` + `.waiting` (direct or cyclic
            // self-waits on `j` alone can never resolve `j` locally).
            let reach = reach_two_sources(
                &self.states,
                self.extern_wait.iter().filter_map(|&(p, k)| {
                    let p = p?;
                    (p != k && !self_blocked[k]).then_some((p, k))
                }),
            );
            let mut open_mask = vec![false; n];
            let mut any_open = false;
            let mut unblocked_targets = Vec::new();
            let mut seen_target = vec![false; n];
            for &(_, j) in &self.extern_wait {
                if self_blocked[j] || seen_target[j] { continue; }
                seen_target[j] = true;
                let slots = reach[j];
                let blocked_by_other = (slots[0] != usize::MAX && slots[0] != j)
                    || (slots[1] != usize::MAX && slots[1] != j);
                if !blocked_by_other {
                    open_mask[j] = true;
                    any_open = true;
                } else {
                    unblocked_targets.push(j);
                }
            }
            if any_open {
                return self.open_extern_targets(&open_mask);
            }

            // Tier 2: every unblocked target lies in or depends on a mutual extern cycle
            // (e.g. `use a::*; use b::*;`). Open the lowest-index target in a sink SCC of the
            // dependency graph (source SCC of the influence graph) so the others can settle locally.
            if !unblocked_targets.is_empty() {
                unblocked_targets.sort_unstable();
                let mut target_slot = vec![usize::MAX; n];
                for (idx, &j) in unblocked_targets.iter().enumerate() {
                    target_slot[j] = idx;
                }
                let num_nodes = n + unblocked_targets.len();
                let mut adj = vec![Vec::new(); num_nodes];
                for u in 0..n {
                    if self.states[u].value.is_none() && !self.states[u].extern_open && !self_blocked[u] {
                        for &w in &self.states[u].waiting {
                            if self.states[w].value.is_none() && !self.states[w].extern_open && !self_blocked[w] {
                                adj[u].push(w);
                            }
                        }
                    }
                }
                for (idx, &j) in unblocked_targets.iter().enumerate() {
                    adj[j].push(n + idx);
                }
                for &(p, k) in &self.extern_wait {
                    let Some(p) = p else { continue };
                    if p != k && !self_blocked[p] && target_slot[k] != usize::MAX {
                        adj[n + target_slot[k]].push(p);
                    }
                }
                let roots: Vec<usize> = (n..num_nodes).collect();
                let sink = source_scc_roots(num_nodes, &adj, &roots);
                if let Some(&open_node) = sink.first() {
                    open_mask[unblocked_targets[open_node - n]] = true;
                    return self.open_extern_targets(&open_mask);
                }
            }
        }

        // Tier 3: no unblocked `extern_wait` remains. Release self-named bare imports
        // (`pub use dep;` with globs), stratified along `.waiting`, breaking any mutual cycle
        // by its lowest-index source SCC.
        if !self.self_extern_wait.is_empty() {
            let reach = reach_two_sources(
                &self.states,
                self.self_extern_wait.iter().map(|&s| (s, s)),
            );
            let mut progressed = false;
            let mut keep = Vec::new();
            for s in std::mem::take(&mut self.self_extern_wait) {
                let slots = reach[s];
                let blocked_by_other = (slots[0] != usize::MAX && slots[0] != s)
                    || (slots[1] != usize::MAX && slots[1] != s);
                if !blocked_by_other {
                    self.states[s].self_extern_waiting = false;
                    self.states[s].self_extern_open = true;
                    self.enqueue(s);
                    progressed = true;
                } else {
                    keep.push(s);
                }
            }
            self.self_extern_wait = keep;
            if progressed {
                return true;
            }
            if !self.self_extern_wait.is_empty() {
                let mut roots = self.self_extern_wait.clone();
                roots.sort_unstable();
                let mut adj = vec![Vec::new(); n];
                for u in 0..n {
                    if self.states[u].value.is_none() && !self.states[u].extern_open {
                        for &w in &self.states[u].waiting {
                            if self.states[w].value.is_none() && !self.states[w].extern_open {
                                adj[u].push(w);
                            }
                        }
                    }
                }
                let sink = source_scc_roots(n, &adj, &roots);
                if let Some(&s) = sink.first() {
                    self.states[s].self_extern_waiting = false;
                    self.states[s].self_extern_open = true;
                    self.self_extern_wait.retain(|&x| x != s);
                    self.enqueue(s);
                    return true;
                }
            }
        }
        false
    }

    fn finish_states(&mut self) {
        let mut local: std::collections::VecDeque<_> = self.states.iter().enumerate()
            .filter_map(|(i, s)| s.local.then_some(i)).collect();
        while let Some(i) = local.pop_front() {
            for parent in self.states[i].waiting.clone() {
                if !self.states[parent].local {
                    self.states[parent].local = true;
                    local.push_back(parent);
                }
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
    }

    fn lookup(&mut self, name: Name, k: Name, path: &[Name], module: Name) -> Option<MacroRef> {
        let origin = k;
        let mut result = None;
        loop {
            if result.is_none() {
                if let Some((tk, tm)) = self.path(k, module, path, None) {
                    if let Some(ImportValue::Macro(d)) = self.ask(ImportNs::Macro, tk, tm, name, (origin == tk).then_some(module), None) {
                        result = Some(d);
                    }
                }
            }
            if !self.ready.is_empty() {
                self.drain_ready();
                continue;
            }
            if !self.unlock_externs() {
                break;
            }
        }
        self.finish_states();
        result
    }

    fn resolve_mod(&mut self, k: Name, module: Name, path: &[Name]) -> Option<(Name, Name)> {
        let mut result = None;
        loop {
            if result.is_none() {
                if let Some(at) = self.path(k, module, path, None) {
                    result = Some(at);
                }
            }
            if !self.ready.is_empty() {
                self.drain_ready();
                continue;
            }
            if !self.unlock_externs() {
                break;
            }
        }
        self.finish_states();
        result
    }
}

/// For each unresolved, non-`extern_open` state, record up to two distinct source ids that
/// can reach it along `.waiting` edges from `seeds`. Tracking at most two sources per vertex
/// is exact for testing "does any source != j reach j?" in `O(|states| + |edges| + |seeds|)`.
fn reach_two_sources(
    states: &[ImportState],
    seeds: impl IntoIterator<Item = (usize, usize)>,
) -> Vec<[usize; 2]> {
    const NONE: usize = usize::MAX;
    let mut reach = vec![[NONE; 2]; states.len()];
    let mut work = std::collections::VecDeque::new();
    let offer = |reach: &mut [[usize; 2]], work: &mut std::collections::VecDeque<(usize, usize)>, u: usize, src: usize| {
        let slots = &mut reach[u];
        if slots[0] == src || slots[1] == src {
            return;
        }
        if slots[0] == NONE {
            slots[0] = src;
            work.push_back((u, src));
        } else if slots[1] == NONE {
            slots[1] = src;
            work.push_back((u, src));
        }
    };
    for (u, src) in seeds {
        if states[u].value.is_none() && !states[u].extern_open {
            offer(&mut reach, &mut work, u, src);
        }
    }
    while let Some((u, src)) = work.pop_front() {
        for &w in &states[u].waiting {
            if states[w].value.is_none() && !states[w].extern_open {
                offer(&mut reach, &mut work, w, src);
            }
        }
    }
    reach
}

/// Iterative Tarjan SCC over the subgraph reachable from `roots`. Returns every `r` in
/// `roots` (preserving `roots` order) whose SCC has in-degree 0 among visited SCCs.
fn source_scc_roots(num_nodes: usize, adj: &[Vec<usize>], roots: &[usize]) -> Vec<usize> {
    const UNVISITED: u32 = u32::MAX;
    let mut index = vec![UNVISITED; num_nodes];
    let mut low = vec![0u32; num_nodes];
    let mut on_stack = vec![false; num_nodes];
    let mut comp = vec![UNVISITED; num_nodes];
    let mut stack: Vec<usize> = Vec::new();
    let mut dfs: Vec<(usize, usize)> = Vec::new();
    let mut next_index = 0u32;
    let mut num_comps = 0u32;

    for &root in roots {
        if index[root] != UNVISITED { continue; }
        index[root] = next_index;
        low[root] = next_index;
        next_index += 1;
        stack.push(root);
        on_stack[root] = true;
        dfs.push((root, 0));
        while let Some(&(u, ei)) = dfs.last() {
            if ei < adj[u].len() {
                dfs.last_mut().unwrap().1 = ei + 1;
                let v = adj[u][ei];
                if index[v] == UNVISITED {
                    index[v] = next_index;
                    low[v] = next_index;
                    next_index += 1;
                    stack.push(v);
                    on_stack[v] = true;
                    dfs.push((v, 0));
                } else if on_stack[v] {
                    low[u] = low[u].min(index[v]);
                }
            } else {
                dfs.pop();
                if let Some(&(parent, _)) = dfs.last() {
                    low[parent] = low[parent].min(low[u]);
                }
                if low[u] == index[u] {
                    let cid = num_comps;
                    num_comps += 1;
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        comp[w] = cid;
                        if w == u { break; }
                    }
                }
            }
        }
    }

    let mut has_incoming = vec![false; num_comps as usize];
    for (u, edges) in adj.iter().enumerate() {
        let cu = comp[u];
        if cu == UNVISITED { continue; }
        for &v in edges {
            let cv = comp[v];
            if cv != UNVISITED && cu != cv {
                has_incoming[cv as usize] = true;
            }
        }
    }
    roots.iter().copied().filter(|&r| !has_incoming[comp[r] as usize]).collect()
}

impl MacroUse {
    pub fn from_image(local: Vec<MacroRef>, uses: crate::fxhash::HashMap<(String, String), Vec<ExpUse>>) -> Self {
        Self { local, uses, looked: Vec::new(), uses_base: Default::default(), uses_log: Vec::new(), names: None, trusted_image: true, replay_image: None }
    }

    /// The names of this parse's lookups that found no definition: textual ones, and ones by a
    /// path into this crate (`crate::`, `self::`, `super::`, or a first segment `is_crate` does not
    /// take for another crate: `m::inner::bar!()`). Another crate's path never names one of this
    /// crate's. Calls in code the cfg drops make no lookup.
    pub fn failed_lookups(&self, is_crate: impl Fn(&str) -> bool) -> std::collections::BTreeSet<String> {
        let Some(names) = &self.names else { return Default::default() };
        self.looked.iter().filter(|l| l.got.is_none() && matches!(l.was, None | Some(None)))
            .filter(|l| l.path.first().is_none_or(|&first| matches!(names.str(first), "crate" | "self" | "super") || !is_crate(names.str(first))))
            .map(|l| names.str(l.name).to_string()).collect()
    }

    /// The lookups of this parse for a later run's replay (`from_replay`): names as strings,
    /// each answer by its expansion_hash and position. None: nothing to replay (an image's).
    pub fn replay_bytes(&self) -> Option<Vec<u8>> {
        if let Some(b) = &self.replay_image { return Some(b.to_vec()); }
        let names = self.names.as_ref()?;
        let mut strs: Vec<String> = Vec::new();
        let mut ix: HashMap<Name, u32> = HashMap::default();
        let mut id = |n: Name, strs: &mut Vec<String>| *ix.entry(n).or_insert_with(|| { strs.push(names.str(n).to_string()); strs.len() as u32 - 1 });
        let mut w = Vec::new();
        let put = |w: &mut Vec<u8>, x: u32| w.extend_from_slice(&x.to_le_bytes());
        let put_s = |w: &mut Vec<u8>, s: &str| { w.extend_from_slice(&(s.len() as u32).to_le_bytes()); w.extend_from_slice(s.as_bytes()); };
        let mut hashes: HashMap<usize, u128> = HashMap::default();
        put(&mut w, self.looked.len() as u32);
        for l in &self.looked {
            put(&mut w, id(l.name, &mut strs));
            put(&mut w, l.path.len() as u32);
            for &p in l.path.iter() { put(&mut w, id(p, &mut strs)); }
            put(&mut w, id(l.module, &mut strs));
            put(&mut w, l.nlocal);
            put(&mut w, l.nuses);
            match &l.got {
                None => w.push(0),
                Some(d) => {
                    w.push(1);
                    let h = *hashes.entry(std::sync::Arc::as_ptr(d) as usize).or_insert_with(|| expansion_hash(d));
                    w.extend_from_slice(&h.to_le_bytes());
                    put_s(&mut w, &d.def_file);
                    put(&mut w, d.def_pos);
                }
            }
        }
        let mut bindings = |w: &mut Vec<u8>, strs: &mut Vec<String>, it: &mut dyn Iterator<Item = (&(Name, Name), &Vec<LocalExpUse>)>, n: usize| {
            put(w, n as u32);
            for ((m, b), entries) in it {
                put(w, id(*m, strs)); put(w, id(*b, strs));
                put(w, entries.len() as u32);
                for u in entries {
                    put(w, u.path.len() as u32);
                    for &p in &u.path { put(w, id(p, strs)); }
                    w.push(u.vis.tag());
                    if let UseVis::In(path) = &u.vis {
                        put(w, path.len() as u32);
                        for p in path { put_s(w, p); }
                    }
                }
            }
        };
        bindings(&mut w, &mut strs, &mut self.uses_base.bindings.iter(), self.uses_base.bindings.len());
        bindings(&mut w, &mut strs, &mut self.uses_log.iter().map(|(k, v)| (k, v)), self.uses_log.len());
        let mut out = Vec::with_capacity(w.len() + strs.iter().map(|s| s.len() + 4).sum::<usize>() + 4);
        put(&mut out, strs.len() as u32);
        for s in &strs { put_s(&mut out, s); }
        out.extend_from_slice(&w);
        Some(out)
    }

    /// An image's lookups (`local` and `uses` as from_image) with its replay data
    /// (replay_bytes): replayed against the loader's table like a speculative parse's, the
    /// answers compared by what their expansions depend on. None: the data does not decode.
    pub fn from_replay(local: Vec<MacroRef>, uses: crate::fxhash::HashMap<(String, String), Vec<ExpUse>>, bytes: &[u8]) -> Option<Self> {
        struct R<'a>(&'a [u8]);
        impl<'a> R<'a> {
            fn take(&mut self, n: usize) -> Option<&'a [u8]> { let x = self.0.get(..n)?; self.0 = &self.0[n..]; Some(x) }
            fn u32(&mut self) -> Option<u32> { Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?)) }
            fn str(&mut self) -> Option<&'a str> { let n = self.u32()? as usize; std::str::from_utf8(self.take(n)?).ok() }
        }
        let mut r = R(bytes);
        let names = std::sync::Arc::new(MacroNames::default());
        let n = r.u32()? as usize;
        if n > bytes.len() { return None; }
        let strs: Vec<Name> = (0..n).map(|_| Some(names.id(r.str()?))).collect::<Option<_>>()?;
        let name = |r: &mut R| -> Option<Name> { strs.get(r.u32()? as usize).copied() };
        let nl = r.u32()? as usize;
        if nl > bytes.len() { return None; }
        let inputs = std::sync::Arc::new(LookupInputs::detached());
        let mut looked = Vec::with_capacity(nl);
        for _ in 0..nl {
            let nm = name(&mut r)?;
            let np = r.u32()? as usize;
            if np > bytes.len() { return None; }
            let path: Box<[Name]> = (0..np).map(|_| name(&mut r)).collect::<Option<_>>()?;
            let module = name(&mut r)?;
            let (nlocal, nuses) = (r.u32()?, r.u32()?);
            if nlocal as usize > local.len() { return None; }
            let was = match r.take(1)?[0] {
                0 => None,
                1 => {
                    let hash = u128::from_le_bytes(r.take(16)?.try_into().ok()?);
                    let def_file = r.str()?.to_string();
                    Some(GotWas { hash, def_file, def_pos: r.u32()? })
                }
                _ => return None,
            };
            looked.push(Looked { name: nm, path, module, nlocal, nuses, got: None, inputs: inputs.clone(), was: Some(was) });
        }
        let bindings = |r: &mut R| -> Option<Vec<((Name, Name), Vec<LocalExpUse>)>> {
            let n = r.u32()? as usize;
            if n > bytes.len() { return None; }
            (0..n).map(|_| {
                let k = (name(r)?, name(r)?);
                let ne = r.u32()? as usize;
                if ne > bytes.len() { return None; }
                let entries = (0..ne).map(|_| {
                    let np = r.u32()? as usize;
                    if np > bytes.len() { return None; }
                    let path = (0..np).map(|_| name(r)).collect::<Option<_>>()?;
                    let vis = UseVis::from_tag(r.take(1)?[0], || {
                        let nv = r.u32()? as usize;
                        if nv > bytes.len() { return None; }
                        (0..nv).map(|_| Some(r.str()?.to_owned())).collect()
                    })?;
                    Some(LocalExpUse { path, vis })
                }).collect::<Option<_>>()?;
                Some((k, entries))
            }).collect()
        };
        let base = bindings(&mut r)?;
        let uses_log = bindings(&mut r)?;
        if !r.0.is_empty() || looked.iter().any(|l| l.nuses as usize > uses_log.len()) { return None; }
        let uses_base = std::sync::Arc::new(LocalImports { bindings: base.into_iter().collect(), index: Default::default() });
        Some(Self { trusted_image: false, local, looked, uses_base, uses_log, names: Some(names), uses, replay_image: Some(bytes.into()) })
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
                    for (key, entries) in &self.uses_log[applied..l.nuses as usize] {
                        *uses.entry(names, (tr(key.0), tr(key.1))) = entries.iter().map(|u| u.remap(tr)).collect();
                    }
                    applied = l.nuses as usize;
                }
                *scope.memo.get_mut() = LookupMemo::default();
            }
            let moved: Box<[Name]>;
            let path = if foreign { moved = l.path.iter().map(|&p| tr(p)).collect(); &moved[..] } else { &l.path[..] };
            let now = scope.resolve(tr(l.name), cur_id, path, tr(l.module), false);
            let ok = match &l.was {
                // (an image's lookup: the same definition by what its expansion depends on -- the
                // file's source is the image's, so a definition written in it at the same place
                // with the same rules expands the same, hygiene included)
                Some(was) => match (&now, was) {
                    (None, None) => true,
                    (Some(a), Some(w)) => expansion_hash(a) == w.hash
                        && (a.def_file != here && w.def_file != here || a.def_file == w.def_file && a.def_pos == w.def_pos),
                    _ => false,
                },
                None => ptr_of(&now) == ptr_of(&l.got)
                    || match (&now, &l.got) {
                        (Some(a), Some(b)) => a.def_file != here && b.def_file != here && same_expansion(a, b),
                        _ => false,
                    },
            };
            if !ok && crate::preload::trace() {
                let path: Vec<&str> = l.path.iter().map(|&p| from.str(p)).collect();
                eprintln!("boot-trace-replay: {} in {} path {:?} before {:?} now {:?}", from.str(l.name), here, path, l.got.as_ref().map(|d| (&d.name, &d.krate, &d.def_file, d.def_pos, d.builtin)), now.as_ref().map(|d| (&d.name, &d.krate, &d.def_file, d.def_pos, d.builtin)));
            }
            ok
        })
    }
}

/// A hash of what same_expansion compares: definitions with equal hashes expand the same anywhere
/// outside their files (name, crate, built-in kind, the rules' token kinds and texts).
fn expansion_hash(d: &MacroDef) -> u128 {
    fn tok(h: &mut crate::sip::Sip128, t: &MTok) { h.u8(t.kind as u8); h.str(&t.text); }
    fn opt(h: &mut crate::sip::Sip128, t: &Option<MTok>) { match t { None => h.u8(0), Some(t) => { h.u8(1); tok(h, t) } } }
    fn m(h: &mut crate::sip::Sip128, v: &[MNode]) {
        h.u32(v.len() as u32);
        for x in v {
            match x {
                MNode::Tok(t) => { h.u8(0); tok(h, t) }
                MNode::Var(n, f) => { h.u8(1); h.str(n); h.u8(*f as u8) }
                MNode::Rep(v, s, o) => { h.u8(2); h.u8(*o); opt(h, s); m(h, v) }
                MNode::Group(o, c, v) => { h.u8(3); tok(h, o); tok(h, c); m(h, v) }
            }
        }
    }
    fn t(h: &mut crate::sip::Sip128, v: &[TNode]) {
        h.u32(v.len() as u32);
        for x in v {
            match x {
                TNode::Tok(p) => { h.u8(0); tok(h, p) }
                TNode::Var(n, _) => { h.u8(1); h.str(n) }
                TNode::Rep(v, s, o) => { h.u8(2); h.u8(*o); opt(h, s); t(h, v) }
                TNode::Group(o, c, v) => { h.u8(3); tok(h, o); tok(h, c); t(h, v) }
                TNode::Crate => h.u8(4),
            }
        }
    }
    let mut h = crate::sip::Sip128::new();
    h.str("macro-expansion");
    h.str(&d.name); h.str(&d.krate); h.u32(d.builtin as u32); h.u32(d.edition as u32);
    h.u32(d.arms.len() as u32);
    for (a, b) in &d.arms { m(&mut h, a); t(&mut h, b); }
    h.finish128()
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
                (TNode::Var(n1, _), TNode::Var(n2, _)) => n1 == n2,
                (TNode::Rep(v1, s1, o1), TNode::Rep(v2, s2, o2)) => o1 == o2 && opt(s1, s2) && t(v1, v2),
                (TNode::Group(o1, c1, v1), TNode::Group(o2, c2, v2)) => tok(o1, o2) && tok(c1, c2) && t(v1, v2),
                (TNode::Crate, TNode::Crate) => true,
                _ => false,
            })
    }
    a.name == b.name && a.krate == b.krate && a.builtin == b.builtin && a.edition == b.edition && a.arms.len() == b.arms.len() && a.arms.iter().zip(&b.arms).all(|(x, y)| m(&x.0, &y.0) && t(&x.1, &y.1))
}


/// The `macro_rules!` definitions and uses a `makepad check` records for the `unused_macros` and
/// `unused_macro_rules` lints (rustc_resolve: a macro is used when an invocation resolves to it,
/// a rule when an expansion matched it).
pub mod uses {
    use super::{close_of, is_open, MTok, MacroDef};
    use crate::lexer::T;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};
    use relax_session::sync::Mutex;

    /// A definition: where it is written, and for each rule the position of its matcher and
    /// whether its transcriber holds a `compile_error!` (such a rule is never reported).
    pub struct Defined {
        pub file: String,
        pub pos: u32,
        pub name: String,
        pub exported: bool,
        pub rules: Vec<(u32, bool)>,
    }

    /// An invocation that resolved to a definition without matching any rule.
    pub const RESOLVED: u32 = u32::MAX;

    static ON: AtomicBool = AtomicBool::new(false);
    static DEFS: Mutex<Vec<Defined>> = Mutex::new(Vec::new());
    static USED: Mutex<Option<HashSet<(String, u32, u32)>>> = Mutex::new(None);

    pub fn record(on: bool) { ON.store(on, Ordering::Relaxed); }
    pub fn recording() -> bool { ON.load(Ordering::Relaxed) && !PREVIEW.with(|p| p.get()) }

    // This thread is expanding the file-selection outline (rs_expand), whose expansion text is its
    // own: a position in it is no position of the parsed file's, so a definition recorded there
    // is one the program never has (a `macro_rules!` an item macro writes, reported unused at
    // whatever another expansion put at that offset of the parsed file). The parse expands every
    // call again while recording (parse_work's reuse is off then) and records what the program
    // holds.
    thread_local! { static PREVIEW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

    /// `f` with this thread's recording off (the outline's expansion).
    pub fn unrecorded<R>(f: impl FnOnce() -> R) -> R {
        struct Restore(bool);
        impl Drop for Restore {
            fn drop(&mut self) { PREVIEW.with(|p| p.set(self.0)); }
        }
        let _restore = Restore(PREVIEW.with(|p| p.replace(true)));
        f()
    }

    pub(super) fn defined(d: &MacroDef) {
        if !recording() || d.builtin != 0 { return; }
        let Some(input) = &d.input else { return };
        let toks: &[MTok] = &input.tokens;
        let mut rules = Vec::new();
        let mut i = 0;
        while i < toks.len() {
            if toks[i].kind == T::Semi { i += 1; continue; }
            if !is_open(toks[i].kind) { break; }
            let mc = close_of(toks, i);
            let pos = toks[i].pos;
            i = mc + 1;
            if i >= toks.len() || toks[i].kind != T::FatArrow { break; }
            i += 1;
            if i >= toks.len() || !is_open(toks[i].kind) { break; }
            let bc = close_of(toks, i);
            let compile_error = toks[i..bc].windows(3).any(|w| w[0].text == "compile_error" && w[1].kind == T::Not && is_open(w[2].kind));
            rules.push((pos, compile_error));
            i = bc + 1;
        }
        let mut defs = DEFS.lock().unwrap_or_else(|e| e.into_inner());
        if !defs.iter().any(|x| x.file == d.def_file && x.pos == d.def_pos) {
            defs.push(Defined { file: d.def_file.clone(), pos: d.def_pos, name: d.name.clone(), exported: d.exported, rules });
        }
    }

    pub(super) fn used(d: &MacroDef, arm: u32) {
        if !recording() || d.builtin != 0 { return; }
        USED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert((d.def_file.clone(), d.def_pos, arm));
    }

    // The parentheses a transcription adds around a multi-token `$e:expr` fragment (the AST has
    // no interpolated-expression node): the lints must see through them.
    thread_local! { static SYNTH_PENDING: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) }; }
    static SYNTH: Mutex<Option<HashSet<(String, u32)>>> = Mutex::new(None);

    /// The transcriber added a paren at text offset `off` of the expansion being built.
    pub(super) fn synth_note(off: u32) {
        if recording() { SYNTH_PENDING.with(|p| p.borrow_mut().push(off)); }
    }

    /// Forget the parens noted so far (an expansion starts).
    pub fn synth_clear() {
        SYNTH_PENDING.with(|p| p.borrow_mut().clear());
    }

    /// The parens noted by the expansion just built on this thread (offsets in its text).
    pub fn synth_take() -> Vec<u32> {
        SYNTH_PENDING.with(|p| std::mem::take(&mut *p.borrow_mut()))
    }

    /// The expansion starts at `base` of `file`: its added parens `v` are at `base + offset`.
    pub fn synth_flush(file: &str, base: u32, v: Vec<u32>) {
        if v.is_empty() { return; }
        let mut g = SYNTH.lock().unwrap_or_else(|e| e.into_inner());
        let set = g.get_or_insert_with(Default::default);
        for off in v { set.insert((file.to_string(), base + off)); }
    }

    /// Whether the `(` at `pos` of `file` was added by a transcription around an expression fragment.
    pub fn is_synth_paren(file: &str, pos: u32) -> bool {
        SYNTH.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&(file.to_string(), pos)))
    }

    /// rustc_resolve ident.rs `resolve_ident_in_module_non_globs_unadjusted` (Shadowing::Unrestricted):
    /// a call through a module path (`crate::m!`) of a `#[macro_export]` macro_rules! of the calling
    /// crate: (calling file, call position, defining file, definition position). Whether the
    /// definition is macro-expanded is answered with the program's files (lint_early).
    static ABS_PATH_MACRO_CALLS: Mutex<Vec<(String, u32, String, u32)>> = Mutex::new(Vec::new());
    pub fn abs_path_macro_call(file: &str, pos: u32, d: &MacroDef) {
        if !recording() { return; }
        ABS_PATH_MACRO_CALLS.lock().unwrap_or_else(|e| e.into_inner()).push((file.to_string(), pos, d.def_file.clone(), d.def_pos));
    }
    pub fn take_abs_path_macro_calls() -> Vec<(String, u32, String, u32)> { std::mem::take(&mut *ABS_PATH_MACRO_CALLS.lock().unwrap_or_else(|e| e.into_inner())) }
    /// The single-name bang macro calls written in the checked crates (calling file, position, name):
    /// rustc resolves them in the lexical scopes (`early_resolve_ident_in_lexical_scope`), where a glob
    /// import's macro that an outer scope also provides is ambiguous (Program::macro_call_ambiguities).
    static PLAIN_MACRO_CALLS: Mutex<Vec<(String, u32, String)>> = Mutex::new(Vec::new());
    pub fn plain_macro_call(file: &str, pos: u32, name: &str) {
        if !recording() { return; }
        PLAIN_MACRO_CALLS.lock().unwrap_or_else(|e| e.into_inner()).push((file.to_string(), pos, name.to_string()));
    }
    pub fn take_plain_macro_calls() -> Vec<(String, u32, String)> { std::mem::take(&mut *PLAIN_MACRO_CALLS.lock().unwrap_or_else(|e| e.into_inner())) }
    static NAMED_POSITIONAL: Mutex<Vec<(String, u32, String)>> = Mutex::new(Vec::new());
    pub fn named_arg_positional(file: &str, pos: u32, name: &str) { NAMED_POSITIONAL.lock().unwrap_or_else(|e| e.into_inner()).push((file.to_string(), pos, name.to_string())); }
    pub fn take_named_arg_positional() -> Vec<(String, u32, String)> { std::mem::take(&mut *NAMED_POSITIONAL.lock().unwrap_or_else(|e| e.into_inner())) }
    /// rustc_builtin_macros source_util `ExpandInclude::make_expr`: tokens after the one
    /// expression of an `include!` in expression position (lint incomplete_include): the
    /// calling file and the call's position.
    static INCOMPLETE_INCLUDE: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());
    pub fn incomplete_include(file: &str, call: u32) { INCOMPLETE_INCLUDE.lock().unwrap_or_else(|e| e.into_inner()).push((file.to_string(), call)); }
    pub fn take_incomplete_include() -> Vec<(String, u32)> { std::mem::take(&mut *INCOMPLETE_INCLUDE.lock().unwrap_or_else(|e| e.into_inner())) }
    /// rustc_parse parse_param_general: a C-variadic `...` without a pattern where the parameter needs
    /// one (lint varargs_without_pattern, buffered for the crate root): the file and the `...`.
    static VARARGS_WITHOUT_PATTERN: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());
    pub fn varargs_without_pattern(file: &str, pos: u32) { VARARGS_WITHOUT_PATTERN.lock().unwrap_or_else(|e| e.into_inner()).push((file.to_string(), pos)); }
    pub fn take_varargs_without_pattern() -> Vec<(String, u32)> { std::mem::take(&mut *VARARGS_WITHOUT_PATTERN.lock().unwrap_or_else(|e| e.into_inner())) }
    /// rustc_expand InvocationCollector::check_attributes: the attributes written on a macro call (the
    /// file, the attribute's position, its name), whose feature gates are judged as an item's are.
    static MAC_CALL_ATTRS: Mutex<Vec<(String, u32, String)>> = Mutex::new(Vec::new());
    pub fn mac_call_attr(file: &str, pos: u32, name: &str) { MAC_CALL_ATTRS.lock().unwrap_or_else(|e| e.into_inner()).push((file.to_string(), pos, name.to_string())); }
    pub fn mac_call_attrs() -> Vec<(String, u32, String)> { MAC_CALL_ATTRS.lock().unwrap_or_else(|e| e.into_inner()).clone() }
    static CFG_ATTR_EMPTY: Mutex<Vec<(String, u32)>> = Mutex::new(Vec::new());
    pub fn cfg_attr_empty(file: &str, pos: u32) { CFG_ATTR_EMPTY.lock().unwrap_or_else(|e| e.into_inner()).push((file.to_string(), pos)); }
    pub fn take_cfg_attr_empty() -> Vec<(String, u32)> { std::mem::take(&mut *CFG_ATTR_EMPTY.lock().unwrap_or_else(|e| e.into_inner())) }

    /// A `;` that ends a macro_rules expansion parsed as an expression (rustc_expand
    /// ParserAnyMacro::make, semicolon_in_expressions_from_macros): the expansion's file, the
    /// `;` token's position, the call's position and the file of the macro's definition.
    pub struct TrailingSemi {
        pub file: String,
        pub semi: u32,
        pub call: u32,
        pub def_file: String,
    }

    static SEMIS: Mutex<Vec<TrailingSemi>> = Mutex::new(Vec::new());

    pub fn trailing_semi(file: &str, semi: u32, call: u32, def_file: &str) {
        SEMIS.lock().unwrap_or_else(|e| e.into_inner()).push(TrailingSemi { file: file.to_string(), semi, call, def_file: def_file.to_string() });
    }

    pub fn take_trailing_semis() -> Vec<TrailingSemi> {
        std::mem::take(&mut *SEMIS.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The recorded definitions and uses (file, definition position, rule or `RESOLVED`).
    pub fn take() -> (Vec<Defined>, HashSet<(String, u32, u32)>) {
        (std::mem::take(&mut *DEFS.lock().unwrap_or_else(|e| e.into_inner())), USED.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default())
    }

    /// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
    pub(crate) unsafe fn reset_statics() {
        use relax_session::reset_static as reset;
        reset(&ON, AtomicBool::new(false));
        reset(&DEFS, Mutex::new(Vec::new()));
        reset(&USED, Mutex::new(None));
        reset(&SYNTH, Mutex::new(None));
        reset(&ABS_PATH_MACRO_CALLS, Mutex::new(Vec::new()));
        reset(&PLAIN_MACRO_CALLS, Mutex::new(Vec::new()));
        reset(&NAMED_POSITIONAL, Mutex::new(Vec::new()));
        reset(&INCOMPLETE_INCLUDE, Mutex::new(Vec::new()));
        reset(&VARARGS_WITHOUT_PATTERN, Mutex::new(Vec::new()));
        reset(&MAC_CALL_ATTRS, Mutex::new(Vec::new()));
        reset(&CFG_ATTR_EMPTY, Mutex::new(Vec::new()));
        reset(&SEMIS, Mutex::new(Vec::new()));
    }
}

pub(crate) fn is_open(k: T) -> bool {
    matches!(k, T::OpenParen | T::OpenBracket | T::OpenBrace)
}
fn is_close(k: T) -> bool {
    matches!(k, T::CloseParen | T::CloseBracket | T::CloseBrace)
}

/// Index of the token closing the group opened at `i`.
pub(crate) fn close_of(toks: &[MTok], i: usize) -> usize {
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

/// rustc_expand mbe/quoted.rs: `pat` is `PatParam` (no top-level `|`) before edition 2021.
fn frag_of(s: &str, edition: u16) -> Option<Frag> {
    Some(match s {
        "expr" => Frag::Expr,
        "expr_2021" => Frag::Expr2021,
        "ident" => Frag::Ident,
        "ty" => Frag::Ty,
        "tt" => Frag::Tt,
        "pat" if edition < 2021 => Frag::PatParam,
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
pub(crate) fn rep_at(toks: &[MTok], i: usize) -> Option<(usize, usize, Option<MTok>, u8, usize)> {
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

fn parse_matcher(toks: &[MTok], edition: u16) -> Result<Vec<MNode>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        // rustc_expand mbe/macro_parser.rs: TtParser::match_one ignores matcher docs.
        if t.kind == T::DocComment { i += 1; continue; }
        if t.kind == T::Dollar && i + 1 < toks.len() {
            let n = &toks[i + 1];
            if n.kind == T::OpenParen {
                let (lo, hi, sep, op, next) = rep_at(toks, i).ok_or("bad repetition in macro matcher")?;
                if sep.is_some() && op == b'?' {
                    return Err("the `?` macro repetition operator does not take a separator".to_string());
                }
                let inner = parse_matcher(&toks[lo..hi], edition)?;
                // mbe/macro_rules.rs check_lhs_no_empty_seq: a repetition without separator must not match nothing
                if sep.is_none()
                    && inner.iter().all(|n| matches!(n, MNode::Var(_, Frag::Vis) | MNode::Rep(_, _, b'*' | b'?'))) {
                    return Err("repetition matches empty token tree".to_string());
                }
                out.push(MNode::Rep(inner, sep, op));
                i = next;
                continue;
            }
            if matches!(n.kind, T::Ident | T::RawIdent) || n.text.chars().all(|c| c.is_alphanumeric() || c == '_') {
                if i + 3 < toks.len() + 1 && i + 2 < toks.len() && toks[i + 2].kind == T::Colon && i + 3 < toks.len() {
                    let f = frag_of(&toks[i + 3].text, edition).ok_or_else(|| format!("invalid fragment specifier `{}`", toks[i + 3].text))?;
                    out.push(MNode::Var(n.text.clone(), f));
                    i += 4;
                    continue;
                }
                if matches!(n.kind, T::Ident | T::RawIdent) {
                    return Err(format!("missing fragment specifier for `${}`", n.text));
                }
            }
        }
        if is_open(t.kind) {
            let c = close_of(toks, i);
            out.push(MNode::Group(t.clone(), toks[c].clone(), parse_matcher(&toks[i + 1..c], edition)?));
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
                out.push(TNode::Var(n.text.clone(), t.pos));
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
    // (a forwarder has no edition of its own: its tokens are the caller's)
    parse_def(alias, krate, 2021, &mt).ok().map(|mut d| { d.edition = 0; std::sync::Arc::new(d) })
}

/// The edition the expansions of a `macro_rules!` of crate `krate` (written in `edition`) carry.
/// Relax's std crates stand in for rustc's library, which is edition 2024
/// (library/{core,alloc,std}/Cargo.toml): their macros' expansions are 2024 code, as rustc's
/// are, though Relax's std source is written in 2021.
fn expansion_edition(krate: &str, edition: u16) -> u16 {
    if matches!(krate, "core" | "alloc" | "std") { 2024 } else { edition }
}

pub fn parse_def(name: &str, krate: &str, edition: u16, toks: &[MTok]) -> Result<MacroDef, String> {
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
        let matcher = parse_matcher(&toks[i + 1..mc], edition)?;
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
    Ok(MacroDef { name: name.to_string(), krate: krate.to_string(), exported: false, arms, builtin: 0, def_file: String::new(), def_pos: 0, scope_pos: None, def_mod: String::new(),
        edition: expansion_edition(krate, edition), input: Some(std::sync::Arc::new(RuleInput { tokens: toks.to_vec(), local_inner: false })) })
}

/// What can come after a matcher fragment (rustc_expand mbe macro_rules.rs `check_matcher`).
#[derive(Clone, Debug)]
enum Follow {
    End,
    Tok(String),
    Open(String),
    Frag(Frag),
}

fn frag_name(f: Frag) -> &'static str {
    match f {
        Frag::Expr => "expr",
        Frag::Expr2021 => "expr_2021",
        Frag::Ident => "ident",
        Frag::Ty => "ty",
        Frag::Tt => "tt",
        Frag::Pat => "pat",
        Frag::PatParam => "pat_param",
        Frag::Path => "path",
        Frag::Literal => "literal",
        Frag::Block => "block",
        Frag::Item => "item",
        Frag::Stmt => "stmt",
        Frag::Vis => "vis",
        Frag::Lifetime => "lifetime",
        Frag::Meta => "meta",
    }
}

/// The tokens that can start `nodes`, and whether they can all match nothing.
fn first_of(nodes: &[MNode]) -> (Vec<Follow>, bool) {
    let mut out = Vec::new();
    for n in nodes {
        match n {
            MNode::Tok(t) => {
                out.push(Follow::Tok(t.text.clone()));
                return (out, false);
            }
            MNode::Var(_, f) => {
                out.push(Follow::Frag(*f));
                return (out, false);
            }
            MNode::Group(open, _, _) => {
                out.push(Follow::Open(open.text.clone()));
                return (out, false);
            }
            MNode::Rep(inner, _, op) => {
                let (f, nullable) = first_of(inner);
                out.extend(f);
                if !(*op == b'*' || *op == b'?' || nullable) {
                    return (out, false);
                }
            }
        }
    }
    (out, true)
}

fn follow_ok(frag: Frag, fol: &Follow, edition: u16) -> bool {
    // (a fragment follower is judged by its first tokens; only `tt` starts with anything)
    if let Follow::Frag(f) = fol {
        return !(*f == Frag::Tt && !matches!(frag, Frag::Ident | Frag::Tt | Frag::Item | Frag::Lifetime | Frag::Literal | Frag::Meta | Frag::Block));
    }
    let tok_in = |set: &[&str]| matches!(fol, Follow::Tok(t) if set.contains(&t.as_str()));
    match frag {
        Frag::Expr | Frag::Expr2021 | Frag::Stmt => matches!(fol, Follow::End) || tok_in(&["=>", ",", ";"]),
        Frag::Pat if edition >= 2021 => matches!(fol, Follow::End) || tok_in(&["=>", ",", "=", "if", "in"]),
        Frag::Pat => matches!(fol, Follow::End) || tok_in(&["=>", ",", "=", "|", "if", "in"]),
        Frag::PatParam => matches!(fol, Follow::End) || tok_in(&["=>", ",", "=", "|", "if", "in"]),
        Frag::Path | Frag::Ty => {
            matches!(fol, Follow::End | Follow::Frag(Frag::Block)) || matches!(fol, Follow::Open(o) if o == "{" || o == "[")
                || tok_in(&["{", "[", "=>", ",", ">", "=", ":", ";", "|", "as", "where", ">>", ">=", ">>="])
        }
        Frag::Vis => match fol {
            Follow::End | Follow::Open(_) => true,
            Follow::Tok(t) => t != "priv" && (t == "," || t.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_' || "&*!<:".contains(c)) || t == "::"),
            Follow::Frag(_) => true,
        },
        _ => true,
    }
}

fn follow_text(f: &Follow) -> String {
    match f {
        Follow::End => String::new(),
        Follow::Tok(t) | Follow::Open(t) => t.clone(),
        Follow::Frag(f) => format!("${}", frag_name(*f)),
    }
}

fn check_seq(nodes: &[MNode], after: &[Follow], edition: u16) -> Result<(), String> {
    for (i, n) in nodes.iter().enumerate() {
        let (mut followers, nullable) = first_of(&nodes[i + 1..]);
        if nullable {
            followers.extend_from_slice(after);
        }
        match n {
            MNode::Var(name, frag) => {
                for fol in &followers {
                    if !follow_ok(*frag, fol, edition) {
                        let fname = frag_name(*frag);
                        return Err(format!("`${name}:{fname}` is followed by `{}`, which is not allowed for `{fname}` fragments", follow_text(fol)));
                    }
                }
            }
            MNode::Group(_, _, inner) => check_seq(inner, &[Follow::End], edition)?,
            MNode::Rep(inner, sep, op) => {
                // (`?` repeats at most once: nothing of it follows itself)
                let mut inner_after: Vec<Follow> = match sep {
                    _ if *op == b'?' => Vec::new(),
                    Some(t) => vec![Follow::Tok(t.text.clone())],
                    None => Vec::new(),
                };
                inner_after.extend(followers);
                check_seq(inner, &inner_after, edition)?;
            }
            MNode::Tok(_) => {}
        }
    }
    Ok(())
}

/// A `macro_rules!` definition's matchers: no variable bound twice in one rule, and every
/// fragment followed only by what its fragment kind allows.
pub fn check_rules(d: &MacroDef, edition: u16) -> Result<(), String> {
    for (matcher, _) in &d.arms {
        let mut vars = Vec::new();
        vars_of(matcher, &mut vars);
        for (i, v) in vars.iter().enumerate() {
            if vars[..i].contains(v) {
                return Err(format!("duplicate matcher binding `${v}`"));
            }
        }
        check_seq(matcher, &[Follow::End], edition)?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub enum Bind {
    /// a matched fragment: token index range in the parser's token list
    /// token index range; the source span of a leading part of the token at the range's end that
    /// the fragment consumed (`>` of a `>>` it split: rustc break_and_eat)
    /// The last: transcribed without parentheses (an expression that is complete as a statement).
    One(usize, usize, Frag, Option<(u32, u32)>, bool),
    Seq(Vec<Bind>),
}

enum ArmMatch {
    Matched(HashMap<String, Bind>),
    Failed { index: usize, vis_taken: u32 },
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
            TNode::Var(v, _) => out.push(v.clone()),
            TNode::Rep(x, _, _) | TNode::Group(_, _, x) => tvars_of(x, out),
            _ => {}
        }
    }
}

/// A matcher flattened into locations (rustc_expand mbe/macro_parser.rs `MatcherLoc`, made by
/// `compute_locs`); a matcher position is an index into them.
enum MLoc<'m> {
    /// a token to match; `true`: a group's opening delimiter (a synthetic parenthesis around
    /// an interpolated expression does not match one)
    Token(&'m MTok, bool),
    /// `$(`: the `vars` metavariables declared inside are numbered from `first_var`; `after`
    /// is the location past the repetition, `depth` the number of repetitions around it
    Sequence { op: u8, vars: usize, after: usize, first_var: usize, depth: usize },
    /// the end of a repetition without a separator: repeat (from `first`) or leave
    KleeneNoSep { op: u8, first: usize },
    /// the end of a repetition with a separator, before it: take it and repeat, or leave
    Sep(&'m MTok),
    /// past a separator: repeat
    KleeneAfterSep { first: usize },
    /// `$name:frag`
    Var { name: &'m str, frag: Frag, var: usize, depth: usize },
    /// past the matcher's end
    Eof,
}

/// rustc `compute_locs`; the caller pushes the final `Eof`.
fn compute_locs<'m>(nodes: &'m [MNode], locs: &mut Vec<MLoc<'m>>, next_var: &mut usize, depth: usize) {
    for n in nodes {
        match n {
            MNode::Tok(t) => locs.push(MLoc::Token(t, false)),
            MNode::Group(open, close, inner) => {
                locs.push(MLoc::Token(open, true));
                compute_locs(inner, locs, next_var, depth);
                locs.push(MLoc::Token(close, false));
            }
            MNode::Rep(inner, sep, op) => {
                let at = locs.len();
                // overwritten once the repetition's end is known
                locs.push(MLoc::Eof);
                let first_var = *next_var;
                let first = locs.len();
                compute_locs(inner, locs, next_var, depth + 1);
                match sep {
                    Some(s) => {
                        locs.push(MLoc::Sep(s));
                        locs.push(MLoc::KleeneAfterSep { first });
                    }
                    None => locs.push(MLoc::KleeneNoSep { op: *op, first }),
                }
                locs[at] = MLoc::Sequence { op: *op, vars: *next_var - first_var, after: locs.len(), first_var, depth };
            }
            MNode::Var(name, frag) => {
                locs.push(MLoc::Var { name, frag: *frag, var: *next_var, depth });
                *next_var += 1;
            }
        }
    }
}

/// A matcher position: a location and the fragments matched on the way to it, one entry per
/// metavariable in declaration order (rustc `MatcherPos`); positions that split share them
/// until one adds to them.
struct MPos {
    idx: usize,
    matches: std::rc::Rc<Vec<Bind>>,
    /// the forwarded empty `vis` fragments it took before a token: (the token, how many)
    vis_taken: (usize, u32),
}

impl MPos {
    fn vis_taken_at(&self, i: usize) -> u32 {
        if self.vis_taken.0 == i { self.vis_taken.1 } else { 0 }
    }

    /// rustc `MatcherPos::push_match`: outside a repetition the next metavariable's entry; inside
    /// `depth` repetitions an element of the innermost sequence being built.
    fn push_match(&mut self, var: usize, depth: usize, m: Bind) {
        let matches = std::rc::Rc::make_mut(&mut self.matches);
        if depth == 0 {
            debug_assert_eq!(var, matches.len());
            matches.push(m);
            return;
        }
        let mut cur = &mut matches[var];
        for _ in 0..depth - 1 {
            match cur {
                Bind::Seq(v) => cur = v.last_mut().unwrap(),
                Bind::One(..) => unreachable!(),
            }
        }
        match cur {
            Bind::Seq(v) => v.push(m),
            Bind::One(..) => unreachable!(),
        }
    }
}

fn is_kw(k: T) -> bool {
    (T::KwAs as u8..=T::KwTry as u8).contains(&(k as u8))
}

fn is_lit(k: T) -> bool {
    matches!(k, T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte)
}

/// The reserved words the lexer gives as identifiers (rustc `Ident::is_reserved`: the unused
/// keywords, `gen` from edition 2024).
fn is_reserved_word(s: &[u8], edition: u16) -> bool {
    matches!(s, b"abstract" | b"become" | b"do" | b"final" | b"macro" | b"override" | b"priv" | b"typeof" | b"unsized" | b"virtual")
        || (edition >= 2024 && s == b"gen")
}

/// What a fragment parse failure names.
fn frag_desc(f: Frag) -> &'static str {
    match f {
        Frag::Expr | Frag::Expr2021 => "expression",
        Frag::Ident => "identifier",
        Frag::Ty => "type",
        Frag::Tt => "token tree",
        Frag::Pat | Frag::PatParam => "pattern",
        Frag::Path => "path",
        Frag::Literal => "literal",
        Frag::Block => "`{`",
        Frag::Item => "item",
        Frag::Stmt => "statement",
        Frag::Vis => "visibility",
        Frag::Lifetime => "lifetime",
        Frag::Meta => "meta item",
    }
}

impl<'a> Parser<'a> {
    fn mtok_eq(&self, i: usize, m: &MTok) -> bool {
        let t = self.toks.at(i);
        if t.kind != m.kind {
            return false;
        }
        // a forwarded fragment is one opaque nonterminal (rustc's invisible group), not its first
        // written token; ident, lifetime and tt fragments forward as their tokens and still match
        if self.forwarded_kind(t.lo).is_some() {
            return false;
        }
        match t.kind {
            T::Ident | T::RawIdent | T::Lifetime | T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte => self.tok_text(t) == m.text.as_bytes(),
            _ => true,
        }
    }

    /// Closing token index of the group opened at token `i` (bounded by `end`).
    pub(crate) fn tok_close_of(&self, i: usize, end: usize) -> Option<usize> {
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

    /// One fragment at token `i` (ending by `end`): its end index, or the fragment parser's error
    /// (rustc parse_nonterminal).
    fn match_frag(&mut self, f: Frag, i: usize, end: usize) -> Result<(usize, Option<(u32, u32)>, bool), String> {
        let expected = || -> Result<(usize, Option<(u32, u32)>, bool), String> { Err(format!("expected {}", frag_desc(f))) };
        if i > end {
            return expected();
        }
        let k = if i < end { self.toks.at(i).kind } else { T::Eof };
        // a forwarded `$s:stmt` matched as a statement again: the whole fragment, as one
        if f == Frag::Stmt && i < end && !self.stmt_frags.is_empty() && self.stmt_frags.contains(&{ self.toks.at(i).lo }) {
            let Some(&hi) = self.frag_groups.get(&{ self.toks.at(i).lo }) else { return expected() };
            return (i..end).find(|&j| self.toks.at(j).hi == hi).map(|j| (j + 1, None, false)).ok_or_else(|| format!("expected {}", frag_desc(f)));
        }
        match f {
            Frag::Ident => {
                if i < end && (matches!(k, T::Ident | T::RawIdent) || (T::KwAs as u8..=T::KwTry as u8).contains(&(k as u8))) && self.tok_text(self.toks.at(i)) != b"_" {
                    Ok((i + 1, None, false))
                } else {
                    expected()
                }
            }
            Frag::Lifetime => if i < end && k == T::Lifetime { Ok((i + 1, None, false)) } else { expected() },
            Frag::Literal => {
                let mut j = i;
                if j < end && self.toks.at(j).kind == T::Minus {
                    j += 1;
                }
                if j < end && matches!(self.toks.at(j).kind, T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte | T::KwTrue | T::KwFalse) {
                    Ok((j + 1, None, false))
                } else {
                    expected()
                }
            }
            Frag::Tt => {
                if i >= end {
                    return expected();
                }
                let tok_lo = self.toks.at(i).lo;
                if let Some(&group_end) = self.frag_groups.get(&tok_lo) {
                    return (i..end).find(|&j| self.toks.at(j).hi == group_end).map(|j| (j + 1, None, false)).ok_or_else(|| format!("expected {}", frag_desc(f)));
                }
                if is_open(k) {
                    self.tok_close_of(i, end).map(|c| (c + 1, None, false)).ok_or_else(|| format!("expected {}", frag_desc(f)))
                } else if is_close(k) {
                    expected()
                } else {
                    Ok((i + 1, None, false))
                }
            }
            Frag::Block => {
                if i < end && k == T::OpenBrace {
                    self.tok_close_of(i, end).map(|c| (c + 1, None, false)).ok_or_else(|| format!("expected {}", frag_desc(f)))
                } else {
                    expected()
                }
            }
            Frag::Vis => {
                if i < end && k == T::KwPub {
                    if i + 1 < end && self.toks.at(i + 1).kind == T::OpenParen {
                        return self.tok_close_of(i + 1, end).map(|c| (c + 1, None, false)).ok_or_else(|| format!("expected {}", frag_desc(f)));
                    }
                    return Ok((i + 1, None, false));
                }
                Ok((i, None, false))
            }
            _ => {
                if i >= end {
                    return expected();
                }
                let save = self.pos;
                let undo = self.tok_undo.len();
                self.pos = i;
                // a fragment match only checks syntax: macro calls inside it are not expanded
                // (rustc_expand mbe/macro_parser.rs parses nonterminals without expanding)
                let macros = self.macros.take();
                let mut bare = false;
                let r: PResult<()> = match f {
                    Frag::Expr => self.expr().map(|id| {
                        // rustc_ast classify::expr_requires_semi_to_be_stmt: such an expression
                        // is its own statement where the fragment starts one
                        { use crate::ast::ExprKind as K; bare = matches!(self.ast.expr(id).kind, K::If(..) | K::Match(..) | K::Block(..) | K::Unsafe(..) | K::ConstBlock(..) | K::TryBlock(..) | K::While(..) | K::Loop(..) | K::For(..)); }
                    }),
                    // `...` is no type start (rustc Token::can_begin_type): only a C-variadic parameter
                    Frag::Ty if k == T::DotDotDot => Err(crate::parser::PErr { pos: self.toks.at(i).lo, msg: "expected a type".to_string() }),
                    // rustc_parse nonterminal.rs: `ty` is parse_ty_no_question_mark_recover, `+`
                    // bounds allowed (a bare `Display + Send` is one type)
                    Frag::Ty => self.ty_plus().map(|_| ()),
                    Frag::Pat => self.pat_top().map(|_| ()),
                    Frag::PatParam => self.pat_no_or().map(|_| ()),
                    Frag::Path => self.path(false).map(|_| ()),
                    Frag::Stmt => self.stmt_fragment(),
                    Frag::Item => self.item_fragment(),
                    Frag::Meta => self.meta_fragment(),
                    _ => Ok(()),
                };
                self.macros = macros;
                let e = self.pos;
                self.pos = save;
                // a `>>` / `>=` the fragment's parse split and consumed whole is whole again (its
                // text belongs to the fragment); one the fragment ends inside of stays split, its
                // rest for the matcher (`Vec<$t:ty>` against `Vec<Option<u8>>`), and the part the
                // fragment took is its source span (rustc_parse break_and_eat / break_last_token)
                let ok = r.is_ok() && e <= end && e > i;
                let mut taken = None;
                let mut split = None;
                while self.tok_undo.len() > undo {
                    let (k, t) = self.tok_undo.pop().unwrap();
                    if !ok || (k as usize) < e {
                        self.toks.edit(k as usize, |x| *x = t);
                    } else if k as usize == e {
                        // the earliest entry holds the unsplit token
                        taken = Some((t.lo, self.toks.at(e).lo));
                        split = Some((k, t));
                    }
                }
                // Keep the remainder for this matcher, but restore it if the arm fails.
                if let Some(s) = split { self.tok_undo.push(s); }
                if ok {
                    Ok((e, taken, bare))
                } else {
                    Err(match r { Err(err) => err.msg, Ok(()) => format!("expected {}", frag_desc(f)) })
                }
            }
        }
    }

    /// Are tokens `[lo, hi)` one macro call `path ! (..)`?
    fn is_macro_call(&self, lo: usize, hi: usize) -> bool {
        let mut i = lo;
        if i < hi && self.toks.at(i).kind == T::PathSep { i += 1; }
        while i < hi && matches!(self.toks.at(i).kind, T::Ident | T::RawIdent | T::KwCrate | T::KwSelfValue | T::KwSuper) {
            i += 1;
            if i < hi && self.toks.at(i).kind == T::PathSep { i += 1; } else { break; }
        }
        i < hi && self.toks.at(i).kind == T::Not && i + 1 < hi && is_open(self.toks.at(i + 1).kind) && self.tok_close_of(i + 1, hi) == Some(hi - 1)
    }

    fn restore_match_tokens(&mut self, undo: usize) {
        while self.tok_undo.len() > undo {
            let (k, t) = self.tok_undo.pop().unwrap();
            self.toks.edit(k as usize, |x| *x = t);
        }
    }

    /// What the token at `i` begins when it starts a forwarded fragment: rustc forwards a
    /// matched fragment as one invisible group of its kind (`OpenInvisible(MetaVar(kind))`),
    /// whatever its first token; None for written tokens and the kinds that forward as their
    /// tokens (ident, lifetime, tt).
    fn forwarded_kind(&self, lo: u32) -> Option<Frag> {
        // nothing forwarded in this parse, or a written token: no per-kind lookups
        if self.fwd_starts.is_empty() || !self.fwd_starts.contains(&lo) {
            return None;
        }
        // an outer statement stays a statement when it begins with a captured expression
        if !self.stmt_frags.is_empty() && self.stmt_frags.contains(&lo) {
            return Some(Frag::Stmt);
        }
        if self.synth_parens.contains(&lo) {
            return Some(Frag::Expr);
        }
        if self.ty_frags.contains(&lo) {
            return Some(Frag::Ty);
        }
        if self.path_frags.contains(&lo) {
            return Some(Frag::Path);
        }
        if self.block_frags.contains(&lo) {
            return Some(Frag::Block);
        }
        match self.meta_frags.get(&lo) {
            Some(&"expr") => Some(Frag::Expr),
            Some(_) => Some(Frag::Literal),
            None => None,
        }
    }

    /// rustc_parse `token_descr`, using the fragment kind when transcription preserved it.
    fn token_descr(&self, t: Tok) -> String {
        let fragment = if t.kind == T::Interp { Some(Frag::Expr) } else { self.forwarded_kind(t.lo) };
        if let Some(frag) = fragment {
            return format!("`{}` metavariable", frag_name(frag));
        }
        let text = self.tok_text(t);
        // Transcription spells `$crate` with the defining crate's identity.
        if t.kind == T::Ident && (t.lo as usize) >= self.src.len() && text.starts_with(b"__relax_crate_") {
            return "reserved identifier `$crate`".into();
        }
        let prefix = if t.kind == T::DocComment {
            "doc comment "
        } else if t.kind == T::Ident || is_kw(t.kind) {
            let edition = self.edition_at(t.lo);
            match crate::lexer::keyword(text, edition) {
                Some(T::KwBox | T::KwTry | T::KwYield) => "reserved keyword ",
                Some(_) => "keyword ",
                None if is_reserved_word(text, edition) => "reserved keyword ",
                None if text == b"_" => "reserved identifier ",
                _ => "",
            }
        } else {
            ""
        };
        format!("{prefix}`{}`", String::from_utf8_lossy(text))
    }

    /// rustc_parse nonterminal.rs `nonterminal_may_begin_with`: can a fragment of kind `f`
    /// begin with the token at `i`? A matcher position waits for the fragment parser only
    /// where it can.
    fn may_begin(&self, f: Frag, i: usize, edition: u16) -> bool {
        let t = self.toks.at(i);
        let k = t.kind;
        if is_close(k) {
            return false;
        }
        if let Some(fk) = self.forwarded_kind(t.lo) {
            // a forwarded statement is no expression, type, pattern or literal (rustc
            // `can_begin_expr` & co. take no `MetaVarKind::Stmt`); where rustc would try another
            // kind on it and fail, the arm is skipped here
            if fk == Frag::Stmt {
                return matches!(f, Frag::Stmt | Frag::Tt);
            }
            return match f {
                Frag::Tt | Frag::Item | Frag::Stmt | Frag::Vis => true,
                Frag::Ident | Frag::Lifetime => false,
                Frag::Expr | Frag::Expr2021 => matches!(fk, Frag::Block | Frag::Expr | Frag::Literal | Frag::Path),
                Frag::Ty => matches!(fk, Frag::Ty | Frag::Path),
                // an expression that is a literal (a forwarded one-token expression)
                Frag::Literal => fk == Frag::Literal || (fk == Frag::Expr && !self.synth_parens.contains(&{ t.lo }) && is_lit(k)),
                Frag::Block => matches!(fk, Frag::Block | Frag::Expr | Frag::Literal),
                Frag::Path | Frag::Meta | Frag::Pat | Frag::PatParam => fk != Frag::Block,
            };
        }
        let any_ident = matches!(k, T::Ident | T::RawIdent) || is_kw(k);
        match f {
            Frag::Tt | Frag::Item | Frag::Stmt => true,
            Frag::Ident => any_ident && self.tok_text(t) != b"_",
            Frag::Lifetime => k == T::Lifetime,
            Frag::Literal => is_lit(k) || matches!(k, T::Minus | T::KwTrue | T::KwFalse),
            Frag::Block => k == T::OpenBrace,
            Frag::Path | Frag::Meta => any_ident || k == T::PathSep,
            Frag::Vis => any_ident || k == T::Comma || self.can_begin_type(i, edition),
            Frag::Ty => self.can_begin_type(i, edition),
            Frag::Pat | Frag::PatParam => {
                any_ident || is_lit(k)
                    || matches!(k, T::OpenParen | T::OpenBracket | T::And | T::Minus | T::AndAnd | T::DotDot | T::DotDotDot | T::PathSep | T::Lt | T::Shl | T::Interp)
                    || (k == T::Or && f == Frag::Pat)
            }
            // `let` never begins one; `const` and `_` only begin edition 2024's `expr`, never
            // `expr_2021` nor `expr` before edition 2024
            Frag::Expr | Frag::Expr2021 => {
                let current = f == Frag::Expr && edition >= 2024;
                k != T::KwLet
                    && (current || k != T::KwConst)
                    && (self.can_begin_expr_tok(i, edition) || (current && k == T::Ident && self.tok_text(t) == b"_"))
            }
        }
    }

    /// rustc_ast token.rs `Token::can_begin_expr`.
    fn can_begin_expr_tok(&self, i: usize, edition: u16) -> bool {
        let t = self.toks.at(i);
        match t.kind {
            T::Ident => {
                let s = self.tok_text(t);
                s != b"_" && (!is_reserved_word(s, edition) || s == b"do" || s == b"gen")
            }
            T::RawIdent | T::Lifetime | T::OpenParen | T::OpenBrace | T::OpenBracket | T::Not | T::Minus | T::Star | T::Or | T::OrOr
            | T::And | T::AndAnd | T::DotDot | T::DotDotDot | T::DotDotEq | T::Lt | T::Shl | T::PathSep | T::Pound | T::Interp => true,
            k if is_lit(k) => true,
            // the keywords that begin an expression, and the path-segment keywords
            T::KwAsync | T::KwBox | T::KwBreak | T::KwConst | T::KwContinue | T::KwFalse | T::KwFor | T::KwIf | T::KwLet | T::KwLoop
            | T::KwMatch | T::KwMove | T::KwReturn | T::KwTrue | T::KwTry | T::KwUnsafe | T::KwWhile | T::KwYield | T::KwStatic
            | T::KwSelfValue | T::KwSelfType | T::KwSuper | T::KwCrate => true,
            _ => false,
        }
    }

    /// rustc_ast token.rs `Token::can_begin_type`.
    fn can_begin_type(&self, i: usize, edition: u16) -> bool {
        let t = self.toks.at(i);
        match t.kind {
            T::Ident => {
                let s = self.tok_text(t);
                s == b"_" || s == b"typeof" || !is_reserved_word(s, edition)
            }
            T::RawIdent | T::OpenParen | T::OpenBracket | T::Not | T::Star | T::And | T::AndAnd | T::Question | T::Lifetime | T::Lt
            | T::Shl | T::PathSep => true,
            T::KwSelfValue | T::KwSelfType | T::KwSuper | T::KwCrate | T::KwFor | T::KwImpl | T::KwFn | T::KwUnsafe | T::KwExtern
            | T::KwDyn => true,
            _ => false,
        }
    }

    /// rustc_expand mbe/macro_parser.rs `TtParser::parse_tt`: tokens `[lo, hi)` against one
    /// arm's matcher locations. Every matcher position is advanced over a token at once (a
    /// repetition's end splits a position: one leaves the repetition, one repeats it), with
    /// no backtracking; the fragment parser runs only when exactly one position waits for a
    /// fragment and no position wants a token. An ordinary failure records the input position
    /// for the next arm to beat. A fragment parse error or ambiguity ends the call.
    fn parse_tt(&mut self, locs: &[MLoc], name: &str, edition: u16, lo: usize, hi: usize) -> Result<ArmMatch, String> {
        let mut cur = vec![MPos { idx: 0, matches: std::rc::Rc::new(Vec::new()), vis_taken: (usize::MAX, 0) }];
        let mut next: Vec<MPos> = Vec::new();
        let mut bb: Vec<MPos> = Vec::new();
        let mut i = lo;
        loop {
            next.clear();
            bb.clear();
            let eof = i >= hi;
            // All current matcher positions share the input cursor, including empty vis groups.
            let failure = ArmMatch::Failed { index: i, vis_taken: cur[0].vis_taken_at(i) };
            let mut eof_mps: Vec<MPos> = Vec::new();
            // parse_tt_inner
            while let Some(mut mp) = cur.pop() {
                match &locs[mp.idx] {
                    &MLoc::Token(t, open) => {
                        // an interpolated expression's synthetic parenthesis is no written group
                        if !eof && self.mtok_eq(i, t) && !(open && self.synth_parens.contains(&{ self.toks.at(i).lo })) {
                            mp.idx += 1;
                            next.push(mp);
                        }
                    }
                    &MLoc::Sequence { op, vars, after, first_var, depth } => {
                        // an empty sequence for each metavariable inside
                        for v in first_var..first_var + vars {
                            mp.push_match(v, depth, Bind::Seq(Vec::new()));
                        }
                        if op != b'+' {
                            // zero repetitions: skip it
                            cur.push(MPos { idx: after, matches: mp.matches.clone(), vis_taken: mp.vis_taken });
                        }
                        // one or more: enter it
                        mp.idx += 1;
                        cur.push(mp);
                    }
                    &MLoc::KleeneNoSep { op, first } => {
                        // end the repetition (fails quietly next round when it cannot)
                        cur.push(MPos { idx: mp.idx + 1, matches: mp.matches.clone(), vis_taken: mp.vis_taken });
                        if op != b'?' {
                            mp.idx = first;
                            cur.push(mp);
                        }
                    }
                    &MLoc::Sep(sep) => {
                        // end the repetition, or take the separator and repeat
                        cur.push(MPos { idx: mp.idx + 2, matches: mp.matches.clone(), vis_taken: mp.vis_taken });
                        if !eof && self.mtok_eq(i, sep) {
                            mp.idx += 1;
                            next.push(mp);
                        }
                    }
                    &MLoc::KleeneAfterSep { first } => {
                        mp.idx = first;
                        cur.push(mp);
                    }
                    &MLoc::Var { frag, .. } => {
                        // a forwarded empty `vis` is a token of its own for rustc (an invisible
                        // group), before a closing delimiter and at the end of the input too
                        if (!eof && self.may_begin(frag, i, edition)) || (frag == Frag::Vis && self.empty_vis_left(&mp, i) > 0) {
                            bb.push(mp);
                        }
                    }
                    MLoc::Eof => {
                        if eof {
                            eof_mps.push(mp);
                        }
                    }
                }
            }
            // at the end of the input exactly one position must be past the matcher's end (one
            // that waits for a forwarded empty `vis` first takes it)
            if eof && bb.is_empty() {
                return match eof_mps.len() {
                    0 => Ok(failure),
                    1 => {
                        let mp = eof_mps.pop().unwrap();
                        let matches = std::rc::Rc::try_unwrap(mp.matches).unwrap_or_else(|m| (*m).clone());
                        let mut binds = HashMap::default();
                        for loc in locs {
                            if let &MLoc::Var { name, var, .. } = loc {
                                binds.insert(name.to_string(), matches[var].clone());
                            }
                        }
                        Ok(ArmMatch::Matched(binds))
                    }
                    _ => Err("ambiguity: multiple successful parses".to_string()),
                };
            }
            match (next.len(), bb.len()) {
                (0, 0) => return Ok(failure),
                (_, 0) => {
                    std::mem::swap(&mut cur, &mut next);
                    i += 1;
                }
                (0, 1) => {
                    // the black-box parser: one fragment
                    let mut mp = bb.pop().unwrap();
                    let MLoc::Var { frag: declared, var, depth, .. } = locs[mp.idx] else { unreachable!() };
                    // `expr_2021` only differs in what may begin it
                    let frag = if declared == Frag::Expr2021 { Frag::Expr } else { declared };
                    let (e, taken, bare) = if frag == Frag::Vis && self.empty_vis_left(&mp, i) > 0 {
                        // the forwarded empty fragment itself, not the tokens after it
                        mp.vis_taken = (i, mp.vis_taken_at(i) + 1);
                        (i, None, false)
                    } else {
                        self.match_frag(frag, i, hi).map_err(|m| format!("{m} (while parsing argument for this `{}` macro fragment)", frag_name(declared)))?
                    };
                    mp.push_match(var, depth, Bind::One(i, e, frag, taken, bare));
                    mp.idx += 1;
                    i = e;
                    cur.push(mp);
                }
                (n, _) => {
                    let nts: Vec<String> = bb.iter().map(|mp| match locs[mp.idx] {
                        MLoc::Var { name, frag, .. } => format!("{} ('{}')", frag_name(frag), name),
                        _ => unreachable!(),
                    }).collect();
                    let nts = nts.join(" or ");
                    return Err(match n {
                        0 => format!("local ambiguity when calling macro `{name}`: multiple parsing options: built-in NTs {nts}."),
                        n => format!("local ambiguity when calling macro `{name}`: multiple parsing options: built-in NTs {nts} or {n} other option{}.", if n == 1 { "" } else { "s" }),
                    });
                }
            }
        }
    }

    /// The forwarded empty `vis` fragments before token `i` (Parser::empty_vis: by the token
    /// they stand before, so a token list out of text order -- a call's desugared doc comments
    /// among its tokens -- keeps them) that `mp` has not taken.
    fn empty_vis_before(&self, i: usize) -> u32 {
        if self.empty_vis.is_empty() {
            return 0;
        }
        self.empty_vis.get(&{ self.toks.at(i).lo }).copied().unwrap_or(0)
    }

    fn empty_vis_left(&self, mp: &MPos, i: usize) -> u32 {
        self.empty_vis_before(i).saturating_sub(mp.vis_taken_at(i))
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
        for (ni, n) in nodes.iter().enumerate() {
            match n {
                TNode::Tok(t) => out.push(t.kind, &t.text, None, t.pos),
                // rustc_expand mbe/quoted.rs: parse_tree keeps `$crate` one Ident;
                // rustc_parse parser/nonterminal.rs: get_macro_ident accepts it.
                TNode::Crate => out.push(T::Ident, krate_path, None, u32::MAX),
                TNode::Group(o, c, inner) => {
                    out.push(o.kind, &o.text, None, o.pos);
                    self.transcribe(inner, binds, idx, krate_path, out)?;
                    out.push(c.kind, &c.text, None, c.pos);
                }
                TNode::Var(v, var_pos) => match Self::lookup_bind(binds, v, idx) {
                    Some(Bind::One(lo, hi, f, taken, bare)) => {
                        // a macro-call expression stays unparenthesised only where it ends the
                        // statement-like position (`$e;` / last): `m!{..} * 2` would otherwise parse as
                        // a statement macro followed by `*2` (the fragment is one opaque expression)
                        let ends = nodes.get(ni + 1).map_or(true, |n| matches!(n, TNode::Tok(t) if t.text == ";"));
                        let bare_macro = *f == Frag::Expr && taken.is_none() && ends && self.is_macro_call(*lo, *hi);
                        let wrap = *f == Frag::Expr && hi - lo + usize::from(taken.is_some()) > 1 && !*bare && !bare_macro;
                        let wrap_at = wrap.then(|| {
                            let at = out.s.len() as u32 + 1;
                            uses::synth_note(at);
                            out.groups.push((at, at | WRAP_PAREN | META_ONLY));
                            out.push(T::OpenParen, "(", None, u32::MAX);
                            at
                        });
                        let group_start = if lo < hi {
                            let t = self.toks.at(*lo);
                            out.start(t.kind, &String::from_utf8_lossy(self.tok_text(t)), Some(t.lo))
                        } else if let Some((a, b)) = *taken {
                            let part = self.bytes(a, b);
                            out.start(lexed_kind(part), &String::from_utf8_lossy(part), Some(a))
                        } else {
                            out.s.len() as u32 + 1
                        } as usize;
                        let mut groups = Vec::new();
                        if *f == Frag::Vis && lo == hi && taken.is_none() {
                            // an empty `vis` goes on as one (rustc: an empty invisible group)
                            out.empty_vis += 1;
                        }
                        for k in *lo..*hi {
                            // the forwarded empty `vis` fragments inside go on with the copy (a
                            // group matched as one `tt`)
                            if k > *lo {
                                out.empty_vis += self.empty_vis_before(k);
                            }
                            let t = self.toks.at(k);
                            // A forwarded outer fragment must retain the invisible
                            // expression groups nested inside it. Their synthetic
                            // parentheses have no ordinary frag_groups entry.
                            let text = String::from_utf8_lossy(self.tok_text(t));
                            if self.synth_parens.contains(&{ t.lo }) {
                                let at = out.start(t.kind, &text, Some(t.lo));
                                out.groups.push((at, at | WRAP_PAREN | META_ONLY));
                            }
                            // rustc_expand mbe/transcribe.rs: transcribe_pnr preserves
                            // invisible groups inside every captured fragment, including
                            // a path inside a type-macro call subsequently matched as tt.
                            if let Some((end, flags)) = self.fragment_group(t.lo) {
                                groups.push((end, out.start(t.kind, &text, Some(t.lo)), flags));
                            } else if let Some(&kind) = self.meta_frags.get(&{ t.lo }) {
                                // a one-token expression or literal inside the copy stays one: its
                                // kind, and a proc macro's invisible group (builtins.rs
                                // group_fragments; const_format's `__str_const!` passes on the block
                                // holding `formatcp!`'s `"b"` argument)
                                let kind = if kind == "expr" { EXPR_FRAG } else { LIT_FRAG };
                                groups.push((t.hi, out.start(t.kind, &text, Some(t.lo)), kind | META_ONLY));
                            }
                            out.push(t.kind, &text, Some(t.lo), u32::MAX);
                            while groups.last().is_some_and(|&(end, _, _)| end == t.hi) {
                                let (_, start, flags) = groups.pop().unwrap();
                                out.groups.push((start, out.s.len() as u32 | flags));
                            }
                        }
                        if let Some((a, b)) = *taken {
                            let part = self.bytes(a, b);
                            out.push(lexed_kind(part), &String::from_utf8_lossy(part), Some(a), u32::MAX);
                        }
                        // a fragment of one of these kinds stays one token tree where the
                        // expansion is matched again (`$i:item` forwarded to `$t:tt`)
                        let opaque = !wrap && !matches!(f, Frag::Ident | Frag::Lifetime | Frag::Tt);
                        // (a `$t:ty` is a type nonterminal even when it is one token: where a bound is
                        // expected, it is not a trait -- rustc `parse_trait_bound`; the end carries TY_FRAG)
                        let ty_frag = *f == Frag::Ty && !wrap;
                        // rustc_expand mbe/transcribe.rs: transcribe_pnr wraps even a
                        // one-token path in MetaVarKind::Path invisible delimiters.
                        let path_frag = *f == Frag::Path;
                        let kind_flag = match f { Frag::Expr if !wrap => EXPR_FRAG, Frag::Literal => LIT_FRAG, Frag::Block if !wrap => BLOCK_FRAG, Frag::Path => PATH_FRAG, Frag::Stmt => STMT_FRAG, _ => 0 };
                        // (a `$s:stmt` of one token too: `break` is a statement of its own before the next)
                        let stmt_frag = *f == Frag::Stmt;
                        let group = opaque && out.s.len() > group_start && (hi - lo + usize::from(taken.is_some()) > 1 || ty_frag || path_frag || stmt_frag);
                        if group || (kind_flag != 0 && out.s.len() > group_start) {
                            out.groups.push((group_start as u32, out.s.len() as u32 | if ty_frag { TY_FRAG } else { 0 } | kind_flag | if group { 0 } else { META_ONLY }));
                        }
                        if wrap {
                            out.push(T::CloseParen, ")", None, u32::MAX);
                        }
                        // rustc_expand mbe/transcribe.rs transcribe_pnr: a fragment other than a
                        // `tt` goes on as one token or invisible group spanned at its `$x`, where a
                        // parse or match error on it is reported
                        let (start, end) = (wrap_at.unwrap_or(group_start as u32), out.s.len() as u32);
                        if *f != Frag::Tt && *var_pos != u32::MAX && end > start {
                            out.metas.push((start, *var_pos, end - start));
                        }
                    }
                    Some(Bind::Seq(_)) => return Err(format!("`${}` is still repeating here -- use it inside `$( .. )*`", v)),
                    // not a macro variable: `$x` literally (rare; keeps the text)
                    None => {
                        out.push(T::Dollar, "$", None, u32::MAX);
                        out.push(crate::lexer::keyword(v.as_bytes(), self.edition).unwrap_or(T::Ident), v, None, u32::MAX);
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
                                out.push(s.kind, &s.text, None, s.pos);
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
        if lo == 0 || hi < lo {
            return (lo, hi);
        }
        // (index of the token the docs precede, inner, text)
        let mut docs: Vec<(usize, bool, Vec<u8>)> = Vec::new();
        for j in lo..=hi {
            let (prev, t) = (self.toks.at(j - 1), self.toks.at(j));
            if prev.hi < t.lo {
                let gap = self.bytes(prev.hi, t.lo);
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
    pub fn expand_rules(&mut self, def: &MacroDef, lo: usize, hi: usize) -> Result<crate::parse_work::Rules, RulesError> {
        let _profile = crate::load_profile::span(self.session.runtime.profile, "macro-rules", &def.name);
        // where the input ends: just after its last token (rustc macro_parser.rs parse_tt: the
        // Eof failure is spanned at the end of the token before it; an empty input, at the call)
        let end = (lo < hi).then(|| self.toks.at(hi - 1).hi);
        let (lo, hi) = self.desugar_doc_comments(lo, hi);
        uses::used(def, uses::RESOLVED);
        // the definition's edition decides which tokens can begin an `expr` (rustc: the
        // metavariable's span edition)
        let edition = if def.edition == 0 { self.edition } else { def.edition };
        let mut best_failure = None;
        for (arm, (matcher, body)) in def.arms.iter().enumerate() {
            // rustc_expand mbe/macro_rules.rs: try_match_macro starts each arm with
            // a fresh parser. A failed arm must not leave grammar-split tokens behind.
            let undo = self.tok_undo.len();
            let mut locs = Vec::new();
            compute_locs(matcher, &mut locs, &mut 0, 0);
            locs.push(MLoc::Eof);
            // Failure: the next arm; an error (a fragment that did not parse, an ambiguity)
            // ends the call, as rustc's ErrorReported / Error do
            let binds = match self.parse_tt(&locs, &def.name, edition, lo, hi) {
                Ok(ArmMatch::Matched(binds)) => Some(binds),
                Ok(ArmMatch::Failed { index, vis_taken }) => {
                    let position = (index, vis_taken);
                    if best_failure.is_none_or(|(best, _)| position > best) {
                        // The token can be the remainder of an operator split by this arm.
                        best_failure = Some((position, self.toks.at(index)));
                    }
                    None
                }
                Err(m) => {
                    self.restore_match_tokens(undo);
                    return Err(RulesError { at: None, msg: m });
                }
            };
            if let Some(binds) = binds {
                uses::used(def, arm as u32);
                // `$crate` names the defining crate by identity (rustc: the `$crate` token keeps the
                // definition's hygiene), wherever the transcribed tokens end up: a caller that reaches
                // the macro only through a re-export, and also this crate's own expansions, whose
                // tokens can become the body of a macro another crate expands (a nested
                // `macro_rules!`, or tokens passed on to a macro that defines one). `crate` would
                // name the crate expanding that macro.
                let krate_path = format!("__relax_crate_{}", def.krate);
                let mut out = Transcript { edition: self.edition, ..Default::default() };
                let result = self.transcribe(body, &binds, &mut Vec::new(), &krate_path, &mut out);
                self.restore_match_tokens(undo);
                result.map_err(|msg| RulesError { at: None, msg })?;
                out.s.push(' ');
                return Ok((out.s, out.toks, out.origins, out.bodies, out.groups, out.metas));
            }
            self.restore_match_tokens(undo);
        }
        // rustc_expand mbe/diagnostics.rs failed_to_match_macro: at the best failure's token,
        // or where the input ended
        let (descr, at) = match best_failure {
            Some(((_, taken), t)) if self.empty_vis.get(&{ t.lo }).is_some_and(|&n| n > taken) => ("`vis` metavariable".into(), t.lo),
            Some(((i, _), t)) if i < hi => (self.token_descr(t), t.lo),
            _ => return Err(RulesError { at: end, msg: "unexpected end of macro invocation".into() }),
        };
        Err(RulesError { at: Some(at), msg: format!("no rules expected {descr}") })
    }
}

/// A `macro_rules!` call that did not expand: the message, and where rustc reports it when that
/// is not the call itself (the input token no arm could match, or just after the last token of
/// an input that ended too soon; None: the call).
#[derive(Debug)]
pub struct RulesError {
    pub at: Option<u32>,
    pub msg: String,
}

/// A transcription being written: the text (each token preceded by a space, unless it is a
/// fragment token that followed the previous one with nothing between at the call) with the tokens
/// over it (kind and position as the definition's and the call's tokens have them, no lexing), per
/// token substituted from a fragment (offset, its position), per token of the definition body
/// (offset, its position in the definition).
#[derive(Default)]
pub struct Transcript {
    /// the edition the text is read in: the expanding parser's, whichever edition a token was lexed in
    edition: u16,
    /// where the last token, a fragment token, ended at the call
    last: Option<u32>,
    s: String,
    toks: Vec<Tok>,
    origins: Vec<(u32, u32, u32)>,
    bodies: Vec<(u32, u32, u32)>,
    /// per substituted fragment other than a `tt`: (offset of its text, its `$x` in the
    /// definition, length of its text)
    metas: Vec<(u32, u32, u32)>,
    groups: Vec<(u32, u32)>,
    /// forwarded empty `vis` fragments waiting for the token they stand before (EMPTY_VIS)
    empty_vis: u32,
}

impl Transcript {
    /// rustc_expand mbe/transcribe.rs: a token substituted from a fragment keeps the spacing it
    /// was lexed with, and a proc macro reads it (`Spacing::Joint` before adjacent punctuation:
    /// `::<`, `!-`, `&&!` passed on through `$($t:tt)*`); a token of the body is `Alone`. A
    /// fragment token that followed the previous one with nothing between at the call follows it
    /// so here too: the text is then a copy of the call's, which lexes to the same tokens. A token
    /// a match split from one the lexer read whole (`>>`, the `0.1` of `x.0.1`) is spelled apart
    /// from its other part, as before.
    fn joins(&self, kind: T, text: &str, origin: Option<u32>) -> bool {
        let adjacent = origin.is_some() && self.last == origin;
        let op = |k: T| !k.is_ident_like() && !k.is_number() && !k.is_literal() && !k.is_trivia()
            && !matches!(k, T::Eof | T::Lifetime | T::RawBody | T::OpenParen | T::CloseParen | T::OpenBracket | T::CloseBracket | T::OpenBrace | T::CloseBrace);
        adjacent && self.toks.last().is_none_or(|prev| {
            let splittable = (op(prev.kind) && op(kind)) || (prev.kind.is_number() && (kind == T::Dot || kind.is_ident_like()));
            !splittable || {
                let joined = format!("{}{}", &self.s[prev.lo as usize..prev.hi as usize], text);
                let mut lexed = Vec::new();
                crate::lexer::lex(joined.as_bytes(), self.edition, &mut lexed).is_ok()
                    && matches!(&lexed[..], [a, b, ..] if a.kind == prev.kind && a.hi == prev.hi - prev.lo && b.kind == kind && b.hi as usize == joined.len())
            }
        })
    }

    /// Where a fragment token pushed next starts.
    fn start(&self, kind: T, text: &str, origin: Option<u32>) -> u32 {
        self.s.len() as u32 + u32::from(!self.joins(self.classify(kind, text), text, origin))
    }

    /// An identifier or keyword is classified by the edition of the expansion (`dyn`, `async`,
    /// `try` are keywords from 2018), as lexing the text would.
    fn classify(&self, kind: T, t: &str) -> T {
        if kind == T::Ident || (kind as u8 >= T::KwAs as u8 && kind as u8 <= T::KwTry as u8) {
            crate::lexer::keyword(t.as_bytes(), self.edition).unwrap_or(T::Ident)
        } else { kind }
    }

    fn push(&mut self, kind: T, t: &str, origin: Option<u32>, bp: u32) {
        let kind = self.classify(kind, t);
        if !self.joins(kind, t, origin) {
            self.s.push(' ');
        }
        self.last = origin.map(|o| o + t.len() as u32);
        let at = self.s.len() as u32;
        if let Some(o) = origin {
            self.origins.push((at, o, t.len() as u32));
        } else if bp != u32::MAX {
            self.bodies.push((at, bp, t.len() as u32));
        }
        self.s.push_str(t);
        // (before a doc comment they stand where no token starts: rustc's `vis` may not be
        // followed by `#`, so nothing matches them there)
        for _ in 0..std::mem::take(&mut self.empty_vis) {
            self.groups.push((at, at | EMPTY_VIS | META_ONLY));
        }
        // rustc_expand mbe/transcribe.rs: transcribe retains DocComment tokens.
        // Keep ours as lexer trivia until parser_from_cx desugars the next call;
        // a generated macro definition must still ignore docs in its matcher.
        if kind == T::DocComment {
            self.s.push('\n');
            self.last = None;
        } else {
            self.toks.push(Tok { kind, lo: at, hi: at + t.len() as u32 });
        }
    }
}

/// The kind of the text of a fragment's split token (a `>>` or `>>=` the fragment broke and took
/// part of): what lexing that text gives.
fn lexed_kind(part: &[u8]) -> T {
    let mut v = Vec::new();
    // a part of `>>`/`>>=` always lexes
    match crate::lexer::lex(part, 2021, &mut v) {
        Ok(()) => v.first().map_or(T::Gt, |t| t.kind),
        Err(_) => T::Gt,
    }
}

/// Owned tokens of a token range (definition bodies outlive the parse of their file).
pub fn mtoks(p: &Parser, toks: &[Tok]) -> Vec<MTok> {
    definition_tokens(toks, |lo, hi| p.bytes(lo, hi))
}

/// A definition's tokens with each `$crate` named as crate `krate`, the way a transcription
/// names it (`__relax_crate_<krate>`): for a `macro_rules!` whose `$crate` is not the crate that
/// holds it (Parser::dollar_crate_of). `$$crate` stays: it writes a `$crate` for a later macro.
pub(crate) fn name_dollar_crate(toks: Vec<MTok>, krate: &str) -> Vec<MTok> {
    let mut out: Vec<MTok> = Vec::with_capacity(toks.len());
    let mut escaped = false;
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t.kind == T::Dollar && !escaped && toks.get(i + 1).is_some_and(|n| n.kind == T::KwCrate) {
            out.push(MTok { kind: T::Ident, text: format!("__relax_crate_{krate}"), pos: t.pos });
            i += 2;
            continue;
        }
        // (`$$` is one escape: the `$` after it is the escaped one)
        escaped = t.kind == T::Dollar && !escaped;
        out.push(t.clone());
        i += 1;
    }
    out
}

/// rustc_expand mbe/quoted.rs: parse_tree retains DocComment tokens in definitions.
/// Recover the lexer trivia for both the prescan and the parser's definitions.
pub(crate) fn definition_tokens<'a>(toks: &[Tok], bytes: impl Fn(u32, u32) -> &'a [u8]) -> Vec<MTok> {
    let mut out = Vec::with_capacity(toks.len());
    let mut prev = toks.first().map_or(0, |t| t.lo);
    for t in toks {
        if prev < t.lo {
            let gap = bytes(prev, t.lo);
            crate::pm_tokens::for_each_gap_doc(gap, |d| out.push(MTok {
                kind: T::DocComment, text: String::from_utf8_lossy(&gap[d.lo..d.hi]).into_owned(), pos: prev + d.lo as u32,
            }));
        }
        out.push(MTok { kind: t.kind, text: String::from_utf8_lossy(bytes(t.lo, t.hi)).into_owned(), pos: t.lo });
        prev = t.hi;
    }
    out
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
    fn all_dependents_resolve_a_shadowed_lib() {
        let mut table = MacroTable::default();
        for (name, src, deps) in [
            ("alpha", "alpha/src/lib.rs", &[][..]),
            ("beta", "beta/src/lib.rs", &["alpha"][..]),
            ("alpha", "beta/src/main.rs", &["alpha", "beta"][..]),
        ] {
            let mut cm = CrateMods { root: name.into(), src: src.into(), ..Default::default() };
            cm.mods.insert(name.into());
            for &dep in deps { cm.externs.insert(dep.into(), dep.into()); }
            table.add_crate(name.into(), std::sync::Arc::new(cm));
        }

        let r = Res { table: &table };
        let (lib, _) = r.extern_crate("beta", "alpha").expect("beta should resolve its alpha dependency");
        assert_eq!(table.crates[lib].src, "alpha/src/lib.rs", "beta should resolve alpha to the library source");
        assert_eq!(r.extern_crate("alpha", "alpha").map(|(key, _)| key), Some(lib), "the alpha bin should resolve alpha to the same library key");
        assert_eq!(r.extern_crate("alpha", "beta").map(|(key, _)| key), Some("beta"), "the alpha bin should resolve the unshadowed beta dependency by its name");
    }

    #[test]
    fn replay_keeps_local_alias_and_each_lookup_import_snapshot() {
        let mut table = macro_table();
        table.push(definition("first", "one"));
        table.push(definition("second", "two"));
        let table = std::sync::Arc::new(table);
        let scope = MacroScope::new(table.clone());
        scope.add_use("app", "runtime".into(), vec!["first".into()], UseVis::Private);
        assert!(scope.lookup("value", "app", &["runtime".into()], "app").is_some());
        scope.add_use("app", "runtime".into(), vec!["second".into()], UseVis::Private);
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
        scope.add_glob_use("app", vec!["dep".into()], UseVis::Private);
        assert!(scope.lookup("value", "app", &[], "app").is_some());
        assert!(scope.finish().replays(&table, "app", "app.rs"));
    }

    #[test]
    fn replay_skips_unchanged_inputs_and_rechecks_changed_imports_and_misses() {
        let mut table = macro_table();
        table.push(definition("first", "one"));
        table.push(definition("second", "two"));
        table.add_exp_uses("app", [(("app::generated".into(), "selected".into()), vec![ExpUse { path: vec!["first".into(), "value".into()], vis: UseVis::Pub }])]);
        let table = std::sync::Arc::new(table);
        let scope = MacroScope::new(table.clone());
        let path = ["crate".into(), "generated".into()];
        assert_eq!(scope.lookup("selected", "app", &path, "app").unwrap().krate, "first");
        assert!(scope.lookup("absent", "app", &path, "app").is_none());
        let used = scope.finish();

        let mut next = (*table).clone();
        next.add_exp_uses("app", [(("app::other".into(), "unrelated".into()), vec![ExpUse { path: vec!["second".into(), "value".into()], vis: UseVis::Pub }])]);
        assert!(used.looked.iter().all(|l| l.inputs.unchanged(&next)));
        assert!(used.replays(&std::sync::Arc::new(next.clone()), "app", "app.rs"));

        // Invalidate a cached validation on the same mutable publication too.
        next.add_exp_uses("app", [(("app::generated".into(), "selected".into()), vec![ExpUse { path: vec!["second".into(), "value".into()], vis: UseVis::Pub }])]);
        assert!(!used.looked[0].inputs.unchanged(&next));
        assert!(!used.replays(&std::sync::Arc::new(next), "app", "app.rs"));

        let mut next = (*table).clone();
        next.add_exp_uses("app", [(("app::generated".into(), "absent".into()), vec![ExpUse { path: vec!["first".into(), "value".into()], vis: UseVis::Pub }])]);
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
            scope.add_use("app::generated", format!("alias{i}"), vec!["self".into(), format!("alias{}", i + 1)], UseVis::Pub);
        }
        scope.add_use("app::generated", "alias16".into(), vec!["dep".into(), "value".into()], UseVis::Pub);
        scope.add_use("app::generated", "cycle_a".into(), vec!["self".into(), "cycle_b".into()], UseVis::Pub);
        scope.add_use("app::generated", "cycle_b".into(), vec!["self".into(), "cycle_a".into()], UseVis::Pub);
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
        scope.add_glob_use("app", vec!["self".into(), "lib_rr".into()], UseVis::Pub);
        scope.add_glob_use("app::lib_rr", vec!["super".into()], UseVis::Pub);
        scope.add_glob_use("app::lib_rr", vec!["dep".into()], UseVis::Pub);
        assert!(scope.lookup("value", "app", &["crate".into(), "lib_rr".into()], "app").is_some());
        assert!(scope.lookup("missing", "app", &["dep".into()], "app::lib_rr").is_none());
        assert!(scope.lookup("unreachable", "app", &["dep".into()], "app::lib_rr").is_none());
        assert!(scope.lookup("value", "app", &["dep".into(), "dep".into()], "app::lib_rr").is_none());
        assert!(scope.lookup("missing", "app", &["crate".into(), "lib_rr".into()], "app").is_none());
        assert!(scope.finish().replays(&table, "app", "app.rs"));
    }

    #[test]
    fn expanded_module_alias_checks_the_leaf_visibility() {
        let mut table = macro_table();
        let mut cm = CrateMods { root: "dep".into(), ..Default::default() };
        let hidden = "dep::facade::hidden";
        for (module, vis) in [("dep::facade", UseVis::Pub), (hidden, UseVis::Private)] {
            cm.mods.insert(module.into());
            cm.items.insert(module.into(), ModItem { vis, ..Default::default() });
        }
        for (leaf, vis) in [("restricted", UseVis::Crate), ("public", UseVis::Pub)] {
            let module = format!("{hidden}::{leaf}");
            cm.mods.insert(module.clone());
            cm.items.insert(module.clone(), ModItem { vis, ..Default::default() });
            cm.add_use(UseItem { module, path: vec!["crate".into(), "value".into()], bind: Some("value".into()), vis: UseVis::Pub });
            table.add_exp_uses("dep", [(("dep::facade".into(), format!("{leaf}_alias")), vec![ExpUse { path: vec!["self".into(), "hidden".into(), leaf.into()], vis: UseVis::Pub }])]);
        }
        cm.externs.insert("other".into(), "first".into());
        let public_use = |module: &str, bind: &str, path: &[&str]| {
            ((module.to_string(), bind.to_string()), vec![ExpUse {
                path: path.iter().map(|s| s.to_string()).collect(), vis: UseVis::Pub,
            }])
        };
        table.add_exp_uses("dep", [
            public_use("dep::facade", "other", &["self", "hidden", "restricted"]),
            public_use("dep::facade", "bare_alias", &["other"]),
            public_use("dep::facade::hidden::restricted", "nested", &["crate", "facade", "hidden", "restricted"]),
            public_use("dep::facade", "qualified_alias", &["other", "nested"]),
        ]);
        table.add_exp_uses("first", [public_use("first", "nested", &["crate"])]);
        table.push(definition("first", "two"));
        table.add_crate("dep".into(), std::sync::Arc::new(cm));
        table.push(definition("dep", "one"));
        let scope = MacroScope::new(std::sync::Arc::new(table));
        let path = |root: &str, alias: &str| [root.into(), "facade".into(), alias.into()];
        assert!(scope.lookup("value", "dep", &path("crate", "restricted_alias"), "dep").is_some(), "crate-visible leaf");
        assert!(scope.lookup("value", "app", &path("dep", "public_alias"), "app").is_some(), "public leaf behind a private prefix");
        assert!(scope.lookup("value", "app", &path("dep", "restricted_alias"), "app").is_none(), "crate-visible leaf from another crate");
        for alias in ["qualified_alias", "bare_alias"] {
            assert!(scope.lookup("value", "app", &path("dep", alias), "app").is_none(), "restricted local module via {alias} from outside");
            assert_eq!(scope.lookup("value", "dep", &path("crate", alias), "dep").unwrap().krate, "dep", "restricted local module via {alias} from inside");
        }
        assert_eq!(scope.lookup("value", "dep", &["other".into()], "dep::facade").unwrap().krate, "dep", "direct call through a shadowed extern");
    }

    #[test]
    fn extern_prelude_shadowing_stratification_and_replay() {
        let mut table = macro_table();
        let mut dep = table.crates["dep"].unresolved();
        for (name, krate) in [("first", "first"), ("second", "second"), ("other", "first")] {
            dep.externs.insert(name.into(), krate.into());
        }
        dep.mods.insert("dep::facade".into());
        dep.items.insert("dep::facade".into(), ModItem { vis: UseVis::Pub, ..Default::default() });
        table.add_crate("dep".into(), std::sync::Arc::new(dep));
        let module_with_value = |table: &mut MacroTable, k: &str, module: &str, vis: UseVis, target: &str| {
            let mut cm = table.crates[k].unresolved();
            cm.mods.insert(module.into());
            cm.items.insert(module.into(), ModItem { vis, ..Default::default() });
            cm.add_use(UseItem { module: module.into(), path: vec!["crate".into(), target.into()],
                bind: Some("value".into()), vis: UseVis::Pub });
            table.add_crate(k.into(), std::sync::Arc::new(cm));
        };
        module_with_value(&mut table, "first", "first::sub", UseVis::Pub, "value");
        for k in ["first", "second"] { table.push(definition(k, "target")); }
        let entry = |bind: &str, path: &[&str]| {
            (("dep::facade".into(), bind.into()), vec![ExpUse {
                path: path.iter().map(|s| s.to_string()).collect(), vis: UseVis::Pub,
            }])
        };
        let scope_with = |base: &MacroTable, imports: &[(&str, &[&str])]| {
            let mut next = base.clone();
            next.add_exp_uses("dep", imports.iter().map(|(bind, path)| entry(bind, path)));
            MacroScope::new(std::sync::Arc::new(next))
        };
        let from_app = |scope: &MacroScope, alias: &str| {
            let path = ["dep".into(), "facade".into(), alias.into()];
            scope.lookup("value", "app", &path, "app").map(|d| d.krate.clone())
        };

        let chain = scope_with(&table, &[("second", &["first", "sub"]), ("alias", &["second"])]);
        assert_eq!(from_app(&chain, "alias").as_deref(), Some("first"), "the local second depends on the first extern fallback");
        let value = scope_with(&table, &[("second", &["crate", "not_a_mod"]), ("alias", &["second"])]);
        assert_eq!(from_app(&value, "alias").as_deref(), Some("second"), "a value-only binding leaves the extern module visible");
        let absolute = scope_with(&table, &[("first", &["", "second"]), ("abs_alias", &["", "first"])]);
        assert_eq!(from_app(&absolute, "first").as_deref(), Some("second"), "local first shadows the extern");
        assert_eq!(from_app(&absolute, "abs_alias").as_deref(), Some("first"), "absolute first bypasses that shadow");

        let bare = ("first", &["first"][..]);
        let plain = scope_with(&table, &[bare]);
        assert_eq!(from_app(&plain, "first").as_deref(), Some("first"), "self-named extern re-export");
        let same = scope_with(&table, &[bare, (GLOB, &["first"])]);
        assert_eq!(from_app(&same, "first").as_deref(), Some("first"), "both the glob and the re-export name the extern");
        let via_second = [bare, (GLOB, &["second"])];
        let unrelated = scope_with(&table, &via_second);
        assert_eq!(from_app(&unrelated, "first").as_deref(), Some("first"), "unrelated extern glob leaves first visible");
        let mut glob_source = table.clone();
        module_with_value(&mut glob_source, "second", "second::first", UseVis::Pub, "value");
        let public = scope_with(&glob_source, &via_second);
        assert_eq!(from_app(&public, "first").as_deref(), Some("second"), "the extern glob provides a public module first");
        // A restricted leaf inside the same crate is selectable there, and blocks the
        // same-named extern for a caller that cannot see the leaf.
        glob_source.push(definition("dep", "target"));
        let mut dep = glob_source.crates["dep"].unresolved();
        dep.mods.insert("dep::facade::provider".into());
        glob_source.add_crate("dep".into(), std::sync::Arc::new(dep));
        module_with_value(&mut glob_source, "dep", "dep::facade::provider::first", UseVis::Crate, "value");
        let restricted = scope_with(&glob_source, &[bare, (GLOB, &["self", "provider"])]);
        let inside = ["crate".into(), "facade".into(), "first".into()];
        assert_eq!(restricted.lookup("value", "dep", &inside, "dep").unwrap().krate, "dep");
        assert!(from_app(&restricted, "first").is_none(), "restricted leaf is hidden from another crate");

        let mut mutual = macro_table();
        mutual.push(definition("second", "target"));
        let scope = MacroScope::new(std::sync::Arc::new(mutual));
        scope.add_glob_use("app", vec!["first".into()], UseVis::Private);
        scope.add_glob_use("app", vec!["second".into()], UseVis::Private);
        assert_eq!(scope.lookup("value", "app", &[], "app").unwrap().krate, "second", "two extern globs can release a mutual wait");

        let mut sibling = macro_table();
        sibling.push(definition("dep", "target"));
        let mut local = (*definition("app", "target")).clone();
        local.name = "local_value".into();
        sibling.define(local);
        let mut app = sibling.crates["app"].unresolved();
        app.mods.insert("app::local".into());
        sibling.add_crate("app".into(), std::sync::Arc::new(app));
        module_with_value(&mut sibling, "app", "app::local::dep", UseVis::Pub, "local_value");
        let scope = MacroScope::new(std::sync::Arc::new(sibling));
        scope.add_use("app", "alias".into(), vec!["self".into(), "local".into()], UseVis::Private);
        scope.add_glob_use("app", vec!["dep".into()], UseVis::Private);
        scope.add_glob_use("app", vec!["alias".into()], UseVis::Private);
        assert_eq!(scope.lookup("value", "app", &[], "app").unwrap().krate, "app", "the later glob provides the local dep");

        // A lookup answered through a sibling glob while another import still waits on its
        // extern fallback must not publish that waiting import as absent.
        let mut waiting = table.clone();
        let mut bar = (*definition("dep", "target")).clone();
        bar.name = "bar".into();
        waiting.define(bar);
        let dep = waiting.crates["dep"].unresolved();
        waiting.add_crate("dep".into(), std::sync::Arc::new(dep));
        waiting.add_exp_uses("dep", [entry("second", &["first", "sub"])]);
        let waiting = std::sync::Arc::new(waiting);
        let scope = MacroScope::new(waiting.clone());
        scope.add_glob_use("app", vec!["dep".into(), "facade".into(), "second".into()], UseVis::Private);
        scope.add_glob_use("app", vec!["dep".into()], UseVis::Private);
        assert_eq!(scope.lookup("bar", "app", &[], "app").map(|d| d.krate.clone()).as_deref(), Some("dep"), "the sibling glob provides bar");
        assert_eq!(from_app(&scope, "second").as_deref(), Some("first"), "second still resolves in the same scope after that lookup");
        assert_eq!(from_app(&MacroScope::new(waiting), "second").as_deref(), Some("first"), "the shared import snapshot holds no unsettled second");

        // Same early-exit hazard in `ImportLookup::resolve_mod` (via `reaches_builtin`).
        let mut mod_wait = table.clone();
        let mut dep = mod_wait.crates["dep"].unresolved();
        dep.externs.insert("core".into(), "core".into());
        dep.add_use(UseItem { module: "dep".into(), path: vec!["core".into()],
            bind: Some("target".into()), vis: UseVis::Pub });
        mod_wait.add_crate("dep".into(), std::sync::Arc::new(dep));
        mod_wait.add_exp_uses("dep", [entry("second", &["first", "sub"])]);
        let mod_wait = std::sync::Arc::new(mod_wait);
        let scope = MacroScope::new(mod_wait.clone());
        scope.add_glob_use("app", vec!["dep".into(), "facade".into(), "second".into()], UseVis::Private);
        scope.add_glob_use("app", vec!["dep".into()], UseVis::Private);
        let target_builtin = ["target".into(), "stringify".into()];
        assert!(scope.reaches_builtin("app", "app", &target_builtin), "the sibling glob provides target");
        assert_eq!(from_app(&scope, "second").as_deref(), Some("first"), "second still resolves in the same scope after resolve_mod");
        assert_eq!(from_app(&MacroScope::new(mod_wait), "second").as_deref(), Some("first"), "the shared import snapshot holds no unsettled second after resolve_mod");

        // Same early-exit hazard for `self_extern_wait` (`pub use first;` with an in-scope glob).
        let mut self_wait = table.clone();
        let mut bar = (*definition("dep", "target")).clone();
        bar.name = "bar".into();
        self_wait.define(bar);
        let mut dep = self_wait.crates["dep"].unresolved();
        dep.mods.insert("dep::facade::empty".into());
        dep.items.insert("dep::facade::empty".into(), ModItem { vis: UseVis::Pub, ..Default::default() });
        self_wait.add_crate("dep".into(), std::sync::Arc::new(dep));
        self_wait.add_exp_uses("dep", [entry("first", &["first"]), entry(GLOB, &["self", "empty"])]);
        let self_wait = std::sync::Arc::new(self_wait);
        let scope = MacroScope::new(self_wait.clone());
        scope.add_glob_use("app", vec!["dep".into(), "facade".into(), "first".into()], UseVis::Private);
        scope.add_glob_use("app", vec!["dep".into()], UseVis::Private);
        assert_eq!(scope.lookup("bar", "app", &[], "app").map(|d| d.krate.clone()).as_deref(), Some("dep"), "the sibling glob provides bar");
        assert_eq!(from_app(&scope, "first").as_deref(), Some("first"), "self-bare first still resolves in the same scope");
        assert_eq!(from_app(&MacroScope::new(self_wait), "first").as_deref(), Some("first"), "the shared import snapshot holds no unsettled self-bare first");

        let table = std::sync::Arc::new(table);
        let scope = MacroScope::new(table.clone());
        let path = ["other".into()];
        assert_eq!(scope.lookup("value", "dep", &path, "dep::facade").unwrap().krate, "first");
        let used = scope.finish();
        assert!(used.replays(&table, "dep", "dep.rs"));
        let mut next = (*table).clone();
        next.add_exp_uses("dep", [entry("other", &["second"])]);
        let next = std::sync::Arc::new(next);
        assert!(!used.looked[0].inputs.unchanged(&next), "the new shadow changes the lookup inputs");
        assert!(!used.replays(&next, "dep", "dep.rs"), "the old extern result must be recomputed");
        assert_eq!(MacroScope::new(next).lookup("value", "dep", &path, "dep::facade").unwrap().krate, "second");
    }

    #[test]
    fn expansion_import_visibility_single_glob_and_seal() {
        let mut table = macro_table();
        let mut app = table.crates["app"].unresolved();
        for module in ["priv_glob", "priv_glob::child", "caller", "crate_glob", "widen",
            "shadow", "val_shadow", "single", "a", "a::limited"] {
            app.mods.insert(format!("app::{module}"));
        }
        table.add_crate("app".into(), std::sync::Arc::new(app));
        table.push(definition("dep", "one"));
        let mut consumer = CrateMods { root: "consumer".into(), ..Default::default() };
        consumer.externs.insert("app".into(), "app".into());
        table.add_crate("consumer".into(), std::sync::Arc::new(consumer));
        let base = std::sync::Arc::new(table);
        let scope = MacroScope::new(base.clone());
        for (module, vis) in [("app::priv_glob", UseVis::Private), ("app::crate_glob", UseVis::Crate),
            ("app::widen", UseVis::Private), ("app::widen", UseVis::Pub),
            ("app::a::limited", UseVis::In(vec!["crate".into(), "a".into()]))] {
            scope.add_glob_use(module, vec!["dep".into()], vis);
        }
        for (module, path) in [("app::shadow", ["dep", "value"]), ("app::val_shadow", ["crate", "not_a_macro"])] {
            scope.add_use(module, "value".into(), path.map(String::from).into(), UseVis::Private);
            scope.add_glob_use(module, vec!["dep".into()], UseVis::Pub);
        }
        scope.add_use("app::single", "alias".into(), vec!["dep".into(), "value".into()], UseVis::Private);
        let local_path = |name: &str| ["crate".into(), name.into()];
        let private = local_path("priv_glob");
        for (caller, visible) in [("app::priv_glob", true), ("app::priv_glob::child", true), ("app", false), ("app::caller", false)] {
            assert_eq!(scope.lookup("value", "app", &private, caller).is_some(), visible, "private glob from {caller}");
            assert_eq!(scope.import_binds("app", caller, &private, "value"), visible, "alias through private glob from {caller}");
        }
        for (owner, visible) in [("crate_glob", true), ("widen", true), ("shadow", false), ("val_shadow", true)] {
            assert_eq!(scope.lookup("value", "app", &local_path(owner), "app::caller").is_some(), visible, "sibling lookup in {owner}");
        }
        for (caller, visible) in [("app::single", true), ("app::caller", false)] {
            assert_eq!(scope.lookup("alias", "app", &local_path("single"), caller).is_some(), visible, "single import from {caller}");
            assert_eq!(scope.import_binds("app", caller, &local_path("single"), "alias"), visible, "alias fallback from {caller}");
        }
        let limited = ["crate".into(), "a".into(), "limited".into()];
        for (caller, visible) in [("app::a", true), ("app::caller", false)] {
            assert_eq!(scope.lookup("value", "app", &limited, caller).is_some(), visible, "restricted glob from {caller}");
        }

        let used = scope.finish();
        let bytes = used.replay_bytes().unwrap();
        let mut table = (*base).clone();
        table.add_exp_uses("app", used.uses.clone());
        table.seal_crate("app");
        let table = std::sync::Arc::new(table);
        let replay = MacroUse::from_replay(used.local, used.uses, &bytes).unwrap();
        assert!(replay.replays(&base, "app", "app.rs"));
        assert!(replay.replays(&table, "app", "app.rs"));
        let sealed = MacroScope::new(table);
        for (owner, name, visible) in [("priv_glob", "value", false), ("crate_glob", "value", false),
            ("widen", "value", true), ("single", "alias", false)] {
            let path = ["app".into(), owner.into()];
            assert_eq!(sealed.lookup(name, "consumer", &path, "consumer").is_some(), visible, "sealed {owner}::{name}");
        }
    }

    #[test]
    fn glob_visibility_widening_and_shadowing() {
        let root = "app";
        let mut cm = CrateMods { root: root.into(), ..Default::default() };
        for m in ["app", "app::a", "app::b", "app::c", "app::shadow", "app::sibling"] {
            cm.mods.insert(m.into());
        }
        cm.externs.insert("dep".into(), "dep".into());
        let mut add = |module: &str, prefix: &str, bind: Option<&str>, vis| {
            cm.add_use(UseItem { module: module.into(), path: prefix.split("::").map(str::to_owned).collect(), bind: bind.map(str::to_owned), vis });
        };
        // Sorted module order: b gets a private binding from dep before c
        // supplies the same binding publicly. a must revisit b after it widens.
        add("app::a", "crate::b", None, UseVis::Pub);
        add("app::b", "dep", None, UseVis::Private);
        add("app::b", "crate::c", None, UseVis::Pub);
        add("app::c", "dep", None, UseVis::Pub);
        add("app::shadow", "dep::value", Some("value"), UseVis::Private);
        add("app::shadow", "dep", None, UseVis::Pub);
        add("app::sibling", "crate::shadow", None, UseVis::Private);
        let mut table = macro_table();
        table.add_crate(root.into(), std::sync::Arc::new(cm));
        table.push(definition("dep", "one"));
        let table = std::sync::Arc::new(table);
        let scope = MacroScope::new(table.clone());
        let a = ["crate".into(), "a".into()];
        assert!(table.lookup("value", root, &a, root).is_some(), "downstream glob must receive the wider binding");
        assert!(table.lookup("value", root, &[], "app::sibling").is_none(), "private single import shadows public glob");
        assert!(scope.lookup("value", root, &[], "app::sibling").is_none(), "the fallback must also honor shadowing");
        let restricted = ScopeVis::super_of("app::a::b");
        assert!(restricted.visible_to(root, "app::a::b", root, "app::a::c"));
        assert!(!restricted.visible_to(root, "app::a::b", root, "app::ab"));
        assert!(!restricted.visible_to(root, "app::a::b", "dep", "app::a"));
        assert_eq!(restricted.min(ScopeVis::PUB), restricted);
        assert_eq!(common_mod_prefix("app::a::b", "app::ab"), root);
    }

    /// rustc's legacy macro_rules scope: a definition reaches its module after it and the modules
    /// declared after it, and leaves its module only through `#[macro_use]` on the `mod` item.
    #[test]
    fn textual_scope_is_lexical_and_leaves_modules_only_through_macro_use() {
        let mut cm = CrateMods { root: "c".into(), ..Default::default() };
        let item = |pos, end, macro_use| ModItem { pos, end, macro_use, vis: UseVis::Private };
        // c: macro a (pos 10); mod b { macro b1 (pos 20) } at 30..40 ; #[macro_use] mod m { macro m1 } at 50..60; mod x; at 70..71
        // c::b::inner, c::m::sub (with its own #[macro_use] mod deep { macro d1 } at 15..25)
        for (m, it) in [("c::b", item(30, 40, false)), ("c::m", item(50, 60, true)), ("c::x", item(70, 71, false)),
            ("c::m::deep", item(15, 25, true)), ("c::m::sib", item(30, 31, false))] {
            cm.mods.insert(m.into());
            cm.items.insert(m.into(), it);
        }
        let def = |module: &str, pos| {
            let mut d = parse_def("n", "c", 2021, &[]).unwrap_or_else(|_| unreachable!());
            d.def_mod = module.into();
            d.def_pos = pos;
            d
        };
        let a = def("c", 10);
        assert!(cm.in_textual_scope(&a, "c", Some(11)) && !cm.in_textual_scope(&a, "c", Some(9)));
        assert!(cm.in_textual_scope(&a, "c::x", None) && cm.in_textual_scope(&a, "c::b", None));
        let b1 = def("c::b", 20);
        // a sibling and the parent do not see a module's macro without #[macro_use]
        assert!(!cm.in_textual_scope(&b1, "c", None) && !cm.in_textual_scope(&b1, "c::x", None));
        assert!(cm.in_textual_scope(&b1, "c::b", Some(21)));
        // #[macro_use] mod m: in scope in the parent after the mod item, and in later siblings
        let m1 = def("c::m", 52);
        assert!(cm.in_textual_scope(&m1, "c", Some(61)) && !cm.in_textual_scope(&m1, "c", Some(49)));
        assert!(cm.in_textual_scope(&m1, "c::x", None) && !cm.in_textual_scope(&m1, "c::b", None));
        // a macro in a #[macro_use] mod nested in a module that is not #[macro_use] stays in it
        let d1 = def("c::m::deep", 16);
        assert!(cm.in_textual_scope(&d1, "c::m", Some(26)) && !cm.in_textual_scope(&d1, "c::m", Some(24)));
        assert!(cm.in_textual_scope(&d1, "c", Some(61)) && cm.in_textual_scope(&d1, "c::m::sib", None));
        let s1 = def("c::m::sib", 32);
        assert!(!cm.in_textual_scope(&s1, "c::m", None) && !cm.in_textual_scope(&s1, "c", None));
    }
}

#[cfg(test)]
mod spacing_tests {
    use super::*;
    use crate::pm_tokens::TokenTree;

    /// The expansion of the last call `name!(..)` in `src` of the `macro_rules! name` written there,
    /// both in a crate of `edition`: its text and tokens, or the matcher's error.
    fn expand_in(src: &str, name: &str, edition: u16) -> Result<(String, Vec<Tok>), String> {
        let mut toks = Vec::new();
        crate::lexer::lex(src.as_bytes(), edition, &mut toks).unwrap_or_else(|e| panic!("{}", e.msg));
        let text = |i: usize| &src[toks[i].lo as usize..toks[i].hi as usize];
        let close = |open: usize| {
            let mut depth = 0;
            (open..toks.len()).find(|&i| {
                match toks[i].kind {
                    T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                    T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
                    _ => {}
                }
                depth == 0
            }).unwrap()
        };
        let at = (2..toks.len()).find(|&i| text(i) == name && text(i - 2) == "macro_rules").unwrap();
        let end = close(at + 1);
        let rules = definition_tokens(&toks[at + 2..end], |lo, hi| &src.as_bytes()[lo as usize..hi as usize]);
        let def = parse_def(name, "c", edition, &rules)?;
        let call = (end..toks.len()).rev().find(|&i| text(i) == name).unwrap();
        let session = std::sync::Arc::new(crate::Session::default());
        let mut p = Parser::new(&session, src.as_bytes(), toks.clone(), edition);
        p.expand_rules(&def, call + 3, close(call + 2)).map(|(s, etoks, ..)| (s, etoks)).map_err(|e| e.msg)
    }

    /// The expansion of the last call `name!(..)` in `src` of the `macro_rules! name` written there:
    /// its text, and the punctuation inside the group after its `m!`, with the spacing a proc macro
    /// reads (pm_tokens::from_toks, as a call's input is built).
    fn expand(src: &str, name: &str) -> (String, String) {
        let (s, etoks) = expand_in(src, name, 2021).unwrap();
        // the transcription's text lexes back to its tokens (RELAX_VERIFY_TOKENS)
        let mut lexed = Vec::new();
        crate::lexer::lex(s.as_bytes(), 2021, &mut lexed).unwrap_or_else(|e| panic!("{}", e.msg));
        lexed.retain(|t| t.kind != T::Eof);
        assert_eq!(lexed.iter().map(|t| (t.kind, t.lo, t.hi)).collect::<Vec<_>>(), etoks.iter().map(|t| (t.kind, t.lo, t.hi)).collect::<Vec<_>>(), "{s}");
        let m = etoks.iter().position(|t| &s[t.lo as usize..t.hi as usize] == "m").unwrap();
        let stream = crate::pm_tokens::from_toks(s.as_bytes(), &etoks[m..], &|_, _| crate::pm_tokens::Span::DUMMY).unwrap();
        let Some(TokenTree::Group(g)) = stream.trees.get(2) else { panic!("no group after m! in {s}") };
        let puncts = g.stream().trees.iter().filter_map(|t| match t {
            TokenTree::Punct(p) => Some(format!("'{}' {:?}", p.as_char(), p.spacing())),
            _ => None,
        }).collect::<Vec<_>>().join(", ");
        (s, puncts)
    }

    /// rustc keeps the spacing a fragment token was lexed with through a transcription, and a proc
    /// macro reads it (relax issue 9: equator's `assert!` passes `$($tokens:tt)*` on to a proc macro that
    /// tells a turbofish `::<` from a comparison by the spacing of the second `:`).
    #[test]
    fn transcribed_tokens_keep_the_spacing_they_were_lexed_with() {
        let rustc = "':' Joint, ':' Joint, '<' Alone, '>' Alone, '!' Joint, '-' Alone, '&' Joint, '&' Joint, '!' Alone";
        let defs = "macro_rules! fwd { ($($t:tt)*) => { m!($($t)*) } }\nmacro_rules! body { () => { m!(f::<u32>() !-x &&!x) } }\n";
        assert_eq!(expand(&format!("{defs}fwd!(f::<u32>() !-x &&!x)"), "fwd").1, rustc, "forwarded through a tt repetition");
        // rustc transcribes a body token `Alone` (only a multi-char token's inner chars are Joint)
        assert_eq!(expand(&format!("{defs}body!()"), "body").1,
            "':' Joint, ':' Alone, '<' Alone, '>' Alone, '!' Alone, '-' Alone, '&' Joint, '&' Alone, '!' Alone", "written in the definition's body");
        // tokens apart where they were lexed stay apart
        assert_eq!(expand(&format!("{defs}fwd!(f :: < u32 > () ! - x & & ! x)"), "fwd").1,
            "':' Joint, ':' Alone, '<' Alone, '>' Alone, '!' Alone, '-' Alone, '&' Alone, '&' Alone, '!' Alone");
        // a match that split a token the lexer read whole (the float `0.1` of `x.0.1`) leaves its
        // parts apart in the text: it still lexes to the transcribed tokens (checked in `expand`)
        let (s, _) = expand("macro_rules! fld { ($e:expr) => { m!($e) } }\nfld!(x.0.1)", "fld");
        assert!(s.contains("x.0"), "{s}");
    }

    /// rustc nonterminal_may_begin_with: `expr_2021` never begins with `_`, whatever the edition,
    /// so in an edition-2024 crate a `_ => ..` arm after `$($m:expr_2021 => ..),+` is no ambiguity
    /// (relax issue 113); edition 2024's `expr` does begin with `_`, and there it is one.
    #[test]
    fn an_expr_2021_fragment_never_begins_with_an_underscore() {
        let def = "macro_rules! pick { ($($m:expr_2021 => $s:expr),+ , _ => $f:expr $(,)?) => { [$($s),+, $f] } }\n";
        let call = "pick!(a(1) => 1, b => 2, _ => 3,)";
        match expand_in(&format!("{def}{call}"), "pick", 2024) {
            Ok((s, _)) => assert!(s.contains('3'), "{s}"),
            Err(e) => panic!("expr_2021 at `_`: {e}"),
        }
        let e = expand_in(&format!("{}{call}", def.replace("expr_2021", "expr")), "pick", 2024).expect_err("expr at `_` in edition 2024");
        assert!(e.contains("local ambiguity") && e.contains("expr ('m') or 1 other option"), "{e}");
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    uses::reset_statics();
    reset(&SCOPE_NS, std::sync::atomic::AtomicU64::new(0));
}
