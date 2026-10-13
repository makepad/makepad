//! Splash modules a document imports (Motion documents, their scenes, Canvas
//! documents): any Splash file whose last expression is an object of its
//! exports, published to a library and pinned by revision like any asset.
//!
//! ```splash
//! let looks = import(asset("<id>", "<rev>"))   // a pinned library revision
//! looks.film_grain(0.05)
//! ```
//!
//! A module is ordinary Splash; it may import modules the same way. The
//! loader finds every `import(asset(...))` in the text (literal id and rev),
//! resolves them and what they import through the host (the asset library,
//! a CLI's files), evaluates them in the document's VM dependencies first,
//! each once, and `import(asset(id, rev))` returns the evaluated exports.
//! Nothing here is privileged: the stdlib's modules are the same kind of
//! thing (kernels import them as `use lib("id", "rev") as name`, see
//! makepad-script-compute's `module`).
//!
//! Which modules a text refers to is read from its syntax tree
//! (`makepad_script::syntax`), never from its lines: a name inside a
//! comment or a string is not a reference.
//!
//! Module loading is host orchestration (a resolver, scanning, evaluation
//! order) over the VM's public API, not part of the compiler: this crate is
//! compiled with the app, never built into a loader with the VM.

use makepad_script::*;
use makepad_script::syntax;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// What a module hands to the modules that use it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Exports {
    /// The value of its last expression, an object of exports (`{rig: rig}`).
    #[default]
    Value,
    /// Every `let` and `fn` at its top level.
    Scope,
}

/// A module's source and identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleSource {
    /// A built-in's name, or the asset id of an imported revision.
    pub name: String,
    /// A built-in's version, or the asset revision.
    pub version: String,
    pub text: Arc<str>,
    /// The file name the module's messages carry; `module:<key>` when `None`.
    pub file: Option<String>,
    /// The line number its first line has in messages: 1 for a file people
    /// read; 0 is read as 1.
    pub first_line: usize,
    pub exports: Exports,
}

impl ModuleSource {
    pub fn new(name: &str, version: &str, text: impl Into<Arc<str>>) -> Self {
        Self { name: name.into(), version: version.into(), text: text.into(), file: None, first_line: 0, exports: Exports::Value }
    }

    /// The file its messages name (`lib/foliage.splash`), with lines counted from 1.
    pub fn with_file(mut self, file: &str) -> Self {
        self.file = Some(file.into());
        self.first_line = 1;
        self
    }

    /// Export every top-level `let` and `fn` instead of the last expression.
    pub fn exporting_scope(mut self) -> Self {
        self.exports = Exports::Scope;
        self
    }
}

/// How a document refers to a module: a host's named module
/// (`mod.motion.modules.<name>`, Canvas built-ins), a pinned library
/// revision (`import(asset(id, rev))`), or a path in the host's document tree.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModuleRef {
    Named(String),
    Asset { id: String, rev: String },
    /// A path in the host document context; `from` is the referring module.
    Path { from: String, path: String },
}

impl ModuleRef {
    pub fn key(&self) -> String {
        match self {
            ModuleRef::Named(n) => n.clone(),
            ModuleRef::Asset { id, rev } => format!("{id}@{rev}"),
            ModuleRef::Path { from, path } => format!("path:{from}:{path}"),
        }
    }
}

/// Where module sources come from: the host resolves library revisions (its
/// asset store, a tool's files) and may name modules of its own.
pub trait ModuleResolver: Send + Sync {
    fn named(&self, _name: &str) -> Option<ModuleSource> {
        None
    }
    fn asset(&self, _id: &str, _rev: &str) -> Option<ModuleSource> {
        None
    }
    /// Resolve a document path to its pinned source, without consulting an alias head.
    fn path(&self, _from: &str, path: &str) -> Result<ModuleSource, String> {
        Err(format!("no document context for import({path:?})"))
    }
    /// Host policy for a document and every imported module, before evaluation.
    fn validate(&self, _source: &str, _file: &str) -> Result<(), Vec<String>> { Ok(()) }
    /// Logical name of the root document, when the host pins one.
    fn document_file(&self) -> Option<&str> { None }
    /// Install host document reference constructors in this VM.
    fn install(&self, _vm: &mut ScriptVm, _module: ScriptObject) {}
    /// A hint appended when a named module is missing.
    fn named_hint(&self, _name: &str) -> String {
        String::new()
    }
}

/// No modules (a document that imports none needs no host).
pub struct NoModules;

impl ModuleResolver for NoModules {}

/// Sources held in memory (tests, tools reading files, a host's cache).
#[derive(Clone, Debug, Default)]
pub struct MemoryModules {
    pub named: Vec<ModuleSource>,
    /// `(id, rev, source)`.
    pub assets: Vec<(String, String, ModuleSource)>,
    pub paths: Vec<(String, String, ModuleSource)>,
}

impl MemoryModules {
    /// The resolved modules as a resolver of their own (a document loaded
    /// again with what it loaded before).
    pub fn of(loaded: &[LoadedModule]) -> Self {
        let mut out = Self::default();
        for m in loaded {
            match &m.reference {
                ModuleRef::Named(_) => out.named.push(m.source.clone()),
                ModuleRef::Asset { id, rev } => out.assets.push((id.clone(), rev.clone(), m.source.clone())),
                ModuleRef::Path { from, path } => out.paths.push((from.clone(), path.clone(), m.source.clone())),
            }
        }
        out
    }

    /// A library revision from its text.
    pub fn add_asset(&mut self, id: &str, rev: &str, text: &str) {
        self.assets.push((id.into(), rev.into(), ModuleSource::new(id, rev, text)));
    }
}

impl ModuleResolver for MemoryModules {
    fn path(&self, from: &str, path: &str) -> Result<ModuleSource, String> {
        self.paths.iter().find(|(f, p, _)| f == from && p == path).map(|(_, _, m)| m.clone())
            .ok_or_else(|| format!("{from}: module {path} was not found"))
    }
    fn named(&self, name: &str) -> Option<ModuleSource> {
        self.named.iter().find(|m| m.name == name).cloned()
    }
    fn asset(&self, id: &str, rev: &str) -> Option<ModuleSource> {
        self.assets.iter().find(|(i, r, _)| i == id && r == rev).map(|(_, _, m)| m.clone())
    }
}

/// Named modules held in folders: `<folder>/<name>.splash`, the first folder that
/// has it. Each module exports every `let` and `fn` at its top level (a library
/// of functions needs no export list) and is evaluated under the file name
/// `<file_dir>/<name>.splash`, so its messages name that file. A name is
/// `[a-z0-9_]`: no path, no extension.
#[derive(Clone, Debug)]
pub struct FolderModules {
    pub folders: Vec<std::path::PathBuf>,
    /// The directory part of the file names messages carry (`lib` for `lib/foliage.splash`).
    pub file_dir: String,
}

impl FolderModules {
    pub fn new(folders: Vec<std::path::PathBuf>, file_dir: &str) -> Self {
        Self { folders, file_dir: file_dir.into() }
    }

    fn valid(name: &str) -> bool {
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    }
}

impl ModuleResolver for FolderModules {
    fn named(&self, name: &str) -> Option<ModuleSource> {
        if !Self::valid(name) {
            return None;
        }
        self.folders.iter().find_map(|folder| {
            let text = std::fs::read_to_string(folder.join(format!("{name}.splash"))).ok()?;
            Some(ModuleSource::new(name, "1", text).with_file(&format!("{}/{name}.splash", self.file_dir)).exporting_scope())
        })
    }
    fn named_hint(&self, name: &str) -> String {
        if !Self::valid(name) {
            return " A library name is a-z 0-9 _ (no path, no extension).".into();
        }
        format!(" There is no {name}.splash in {}.", self.folders.iter().map(|r| r.display().to_string()).collect::<Vec<_>>().join(" or "))
    }
}

/// The modules `source` refers to, in order of first mention: every
/// `import("path")`, `import(asset("id", "rev"))`, and every path `<named_prefix><name>...` when
/// the host has named modules (`mod.motion.modules.`, `use
/// mod.motion.modules.moji.*`). Comments and strings do not count. A text that
/// does not parse names nothing (the VM reports its syntax error when it runs).
pub fn scan(source: &str, named_prefix: Option<&str>) -> Vec<ModuleRef> {
    scan_from(source, named_prefix, "")
}

pub fn scan_from(source: &str, named_prefix: Option<&str>, from: &str) -> Vec<ModuleRef> {
    let mut out: Vec<ModuleRef> = Vec::new();
    let Ok(tree) = syntax::parse(source) else { return out };
    let mut push = |r: ModuleRef| {
        if !out.contains(&r) {
            out.push(r);
        }
    };
    tree.walk(&mut |_, node| match &node.kind {
        syntax::Kind::Call { callee, args, .. } if callee.ident() == Some("import") => {
            if let Some(syntax::Node { kind: syntax::Kind::Call { callee, args, .. }, .. }) = args.first() {
                if callee.ident() == Some("asset") {
                    if let [id, rev] = args.as_slice() {
                        if let (Some(id), Some(rev)) = (id.string(), rev.string()) {
                            push(ModuleRef::Asset { id: id.to_string(), rev: rev.to_string() });
                        }
                    }
                }
            }
            if let [arg] = args.as_slice() {
                if let Some(path) = arg.string() { push(ModuleRef::Path { from: from.into(), path: path.into() }); }
            }
        }
        syntax::Kind::Ident(path) => {
            if let Some(prefix) = named_prefix {
                if let Some(rest) = path.strip_prefix(prefix) {
                    let name = rest.split('.').next().unwrap_or("");
                    if !name.is_empty() && name != "*" {
                        push(ModuleRef::Named(name.to_string()));
                    }
                }
            }
        }
        _ => {}
    });
    out
}

/// What a library records about a module: its one-line doc (the first
/// comment line), what it exports (the keys of its last expression) and
/// what it imports.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleInfo {
    pub doc: String,
    pub exports: Vec<String>,
    pub imports: Vec<ModuleRef>,
}

/// The library tag of a module asset.
pub const MODULE_TAG: &str = "module";

pub fn module_info(source: &str, named_prefix: Option<&str>) -> ModuleInfo {
    let doc = source.lines().map(str::trim).find(|l| l.starts_with("//")).map(|l| l.trim_start_matches('/').trim().to_string()).unwrap_or_default();
    let exports = match syntax::parse(source) {
        Ok(tree) => match tree.statements().last().and_then(|last| last.object()) {
            Some((_, fields)) => fields.iter().filter_map(|f| f.name.clone()).collect(),
            None => Vec::new(),
        },
        Err(_) => Vec::new(),
    };
    ModuleInfo { doc, exports, imports: scan(source, named_prefix) }
}

/// A resolved module.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedModule {
    pub reference: ModuleRef,
    pub source: ModuleSource,
}

/// Resolve `refs` and everything they import, dependencies first.
pub fn resolve(refs: &[ModuleRef], resolver: &dyn ModuleResolver, named_prefix: Option<&str>) -> Result<Vec<LoadedModule>, Vec<String>> {
    let mut done: Vec<LoadedModule> = Vec::new();
    let mut errors = Vec::new();
    fn visit(r: &ModuleRef, resolver: &dyn ModuleResolver, named_prefix: Option<&str>, stack: &mut Vec<String>, done: &mut Vec<LoadedModule>, errors: &mut Vec<String>) {
        if done.iter().any(|m| m.reference == *r) {
            return;
        }
        if stack.contains(&r.key()) {
            errors.push(format!("module {} imports itself (through {})", r.key(), stack.join(" -> ")));
            return;
        }
        let source = match r {
            ModuleRef::Named(name) => resolver.named(name),
            ModuleRef::Asset { id, rev } => resolver.asset(id, rev),
            ModuleRef::Path { from, path } => match resolver.path(from, path) {
                Ok(source) => Some(source),
                Err(error) => { errors.push(error); return; }
            },
        };
        let Some(source) = source else {
            let hint = match r {
                ModuleRef::Named(name) => resolver.named_hint(name),
                ModuleRef::Path { .. } => String::new(),
                ModuleRef::Asset { .. } => " Publish the module to the library (a Motion asset tagged `module`) and pin its exact revision; a command-line tool takes it as `--module id:rev=file`.".into(),
            };
            errors.push(format!("module {} was not found.{hint}", r.key()));
            return;
        };
        let file = source.file.as_deref().unwrap_or("");
        if let Err(mut diags) = resolver.validate(&source.text, file) { errors.append(&mut diags); return; }
        stack.push(r.key());
        for dep in scan_from(&source.text, named_prefix, file) {
            visit(&dep, resolver, named_prefix, stack, done, errors);
        }
        stack.pop();
        done.push(LoadedModule { reference: r.clone(), source });
    }
    for r in refs {
        visit(r, resolver, named_prefix, &mut Vec::new(), &mut done, &mut errors);
    }
    if errors.is_empty() {
        Ok(done)
    } else {
        Err(errors)
    }
}

/// The file name a module evaluates under (in VM messages).
pub fn module_file(r: &ModuleRef) -> String {
    format!("module:{}", r.key())
}

/// The language core a stdlib module sees: `mod.std`, `mod.math` and `mod.pod`
/// (the `uses` of [`load`] for a module that writes none of its own).
pub fn language_core(vm: &mut ScriptVm) -> [ScriptObject; 3] {
    [vm.module(id!(std)), vm.module(id!(math)), vm.module(id!(pod))]
}

/// The file a loaded module's messages name.
pub fn file_of(m: &LoadedModule) -> String {
    m.source.file.clone().unwrap_or_else(|| module_file(&m.reference))
}

fn string_of(vm: &ScriptVm, v: ScriptValue) -> Option<String> {
    if let Some(s) = v.as_string() {
        return Some(vm.heap().string(s).to_string());
    }
    v.as_inline_string(|s| s.to_string())
}

/// Evaluate the resolved modules in `vm` (in order: dependencies first) and
/// put `import(path)` / `import(asset(id, rev))` on `module`, returning each
/// imported revision's exports (modules import modules through it too). Each module
/// runs under `instructions` with the names of `uses` in scope (the host's
/// `use <object>.*` lines, as objects: a module's text stays exactly as
/// written, so its messages name its own lines); a named module's exports go
/// to `named` under its name.
pub fn load(vm: &mut ScriptVm, module: ScriptObject, uses: &[ScriptObject], loaded: &[LoadedModule], named: Option<ScriptObject>, instructions: usize) -> Vec<ScriptErrorRecord> {
    let table: Rc<RefCell<Vec<(String, ScriptValue, ScriptObjectRef)>>> = Rc::new(RefCell::new(Vec::new()));
    let lookup = table.clone();
    vm.add_method(module, id_lut!(import), script_args!(a = NIL), move |vm, args| {
        let a = vm.value(args, id!(a));
        let key = if let Some(path) = string_of(vm, a) {
            let from = vm.bx.code_ref().ip_to_loc(vm.trap_ip()).map(|loc| loc.file().to_string()).unwrap_or_default();
            let local = ModuleRef::Path { from, path: path.clone() }.key();
            Some(if lookup.borrow().iter().any(|(key, ..)| *key == local) { local }
                else { ModuleRef::Path { from: String::new(), path }.key() })
        } else { a.as_object().and_then(|o| {
            let id = string_of(vm, vm.value(o, id!(id)))?;
            let rev = string_of(vm, vm.value(o, id!(rev)))?;
            Some(format!("{id}@{rev}"))
        }) };
        match key.and_then(|k| lookup.borrow().iter().find(|(key, ..)| *key == k).map(|(_, v, _)| *v)) {
            Some(v) => v,
            None => script_err_invalid_args!(
                vm.trap(),
                "import takes a literal document path or library revision resolved before evaluation"
            ),
        }
    });
    let mut errors = Vec::new();
    let mut evaluated: Vec<(String, String, ScriptValue, ScriptObjectRef)> = Vec::new();
    for m in loaded {
        let cached = evaluated.iter().find(|(name, version, ..)| *name == m.source.name && *version == m.source.version).map(|(_, _, value, _)| *value);
        let (exports, value) = if let Some(value) = cached { (value.as_object(), value) } else {
            vm.capture_errors();
            let file = file_of(m);
            let (value, body) = vm.with_instruction_limit(instructions, |vm| vm.eval_module(ScriptMod::new("", "", &file, m.source.first_line, 0, &m.source.text, &[]), uses));
            errors.extend(vm.take_error_records());
            match m.source.exports {
                Exports::Value => (value.as_object(), value),
                Exports::Scope => {
                    let object = vm.scope_exports(body, uses);
                    (object, object.map(ScriptValue::from).unwrap_or(NIL))
                }
            }
        };
        let Some(exports) = exports else {
            let message = match m.source.exports {
                Exports::Value => format!("module {}: its last expression must be an object of its exports, like {{rig: rig}}", m.reference.key()),
                Exports::Scope => format!("module {}: it did not run to its end, so it exports nothing", m.reference.key()),
            };
            errors.push(ScriptErrorRecord::plain(message));
            continue;
        };
        if cached.is_none() {
            evaluated.push((m.source.name.clone(), m.source.version.clone(), value, vm.new_object_ref(exports)));
        }
        match (&m.reference, named) {
            (ModuleRef::Named(name), Some(named)) => {
                let key = LiveId::from_str_with_lut(name).unwrap_or_else(|_| LiveId::from_str(name));
                vm.set_value_def(named, key, value);
            }
            (ModuleRef::Named(_), None) => {}
            (ModuleRef::Asset { .. } | ModuleRef::Path { .. }, _) => {
                let root = vm.new_object_ref(exports);
                table.borrow_mut().push((m.reference.key(), value, root));
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanning_finds_named_and_pinned_modules() {
        let src = "use mod.motion.modules.moji.*\n// use mod.motion.modules.ignored.*\nlet props = import(asset(\"abc\", \"r7\"))\nlet s = mod.motion.modules.stage.curtains";
        assert_eq!(scan(src, Some("mod.motion.modules.")), vec![ModuleRef::Named("moji".into()), ModuleRef::Asset { id: "abc".into(), rev: "r7".into() }, ModuleRef::Named("stage".into())]);
        assert_eq!(scan(src, None), vec![ModuleRef::Asset { id: "abc".into(), rev: "r7".into() }]);
    }

    #[test]
    fn resolution_orders_dependencies_and_teaches_on_misses() {
        let mut mem = MemoryModules::default();
        mem.add_asset("a", "1", "let b = import(asset(\"b\", \"2\"))\n{}");
        mem.add_asset("b", "2", "{}");
        let loaded = resolve(&[ModuleRef::Asset { id: "a".into(), rev: "1".into() }], &mem, None).unwrap();
        let keys: Vec<String> = loaded.iter().map(|m| m.reference.key()).collect();
        assert_eq!(keys, vec!["b@2", "a@1"]);
        let err = resolve(&[ModuleRef::Asset { id: "a".into(), rev: "9".into() }], &mem, None).unwrap_err();
        assert!(err[0].contains("was not found") && err[0].contains("--module"), "{err:?}");
        mem.add_asset("loop", "1", "let me = import(asset(\"loop\", \"1\"))\n{}");
        let err = resolve(&[ModuleRef::Asset { id: "loop".into(), rev: "1".into() }], &mem, None).unwrap_err();
        assert!(err[0].contains("imports itself"), "{err:?}");
    }

    #[test]
    fn module_info_lists_exports_and_imports() {
        let info = module_info("// Grain and gates\nlet g = import(asset(\"x\", \"1\"))\nlet grain = fn(k) { k }\n{grain: grain gate: 3}", None);
        assert_eq!(info.doc, "Grain and gates");
        assert_eq!(info.exports, vec!["grain", "gate"]);
        assert_eq!(info.imports, vec![ModuleRef::Asset { id: "x".into(), rev: "1".into() }]);
    }

    #[test]
    fn imported_modules_evaluate_once_and_import_returns_their_exports() {
        let mut host = Box::new(ScriptVmHost::new((), ()));
        let bx = Box::new(ScriptVmBase::new());
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx,
        };
        let module = vm.new_module(id!(doc));
        // The host's `asset(id, rev)` (Motion's and Canvas's give the same fields).
        vm.add_method(module, id_lut!(asset), script_args!(id = NIL, rev = NIL), |vm, args| {
            let o = vm.new_object();
            for k in [id!(id), id!(rev)] {
                let v = vm.value(args, k);
                vm.set_value_def(o, k, v);
            }
            o.into()
        });
        let mut mem = MemoryModules::default();
        mem.add_asset("base", "1", "let k = 3.0\n{k: k}");
        mem.add_asset("looks", "2", "let base = import(asset(\"base\", \"1\"))\nlet twice = fn(x) { x * 2.0 * base.k }\n{twice: twice}");
        let src = "let looks = import(asset(\"looks\", \"2\"))\nlooks.twice(5.0)";
        let loaded = resolve(&scan(src, None), &mem, None).unwrap();
        let errors = load(&mut vm, module, &[module], &loaded, None, 1_000_000);
        assert!(errors.is_empty(), "{errors:?}");
        vm.capture_errors();
        let v = vm.eval(ScriptMod::new("", "", "doc", 0, 0, &format!("use mod.doc.*\n{src}"), &[]));
        assert!(vm.take_errors().is_empty());
        assert_eq!(v.as_number(), Some(30.0));
    }

    #[test]
    fn scanning_reads_syntax_not_lines() {
        let src = "let s = \"import(asset(\\\"x\\\", \\\"1\\\"))\"\n/* use mod.lib.hidden.* */\nuse mod.lib.shown.*\nlet v = mod.lib.also.f(1)\nlet z = import(\n    asset(\"p\", \"2\")\n)\n";
        assert_eq!(scan(src, Some("mod.lib.")), vec![ModuleRef::Named("shown".into()), ModuleRef::Named("also".into()), ModuleRef::Asset { id: "p".into(), rev: "2".into() }]);
        assert!(scan("let broken = [", Some("mod.lib.")).is_empty());
    }

    #[test]
    fn scope_modules_export_their_top_level_names_under_their_own_file_and_lines() {
        let mut host = Box::new(ScriptVmHost::new((), ()));
        let bx = Box::new(ScriptVmBase::new());
        let mut vm = ScriptVm { host: host.host_context(), bx };
        let module = vm.new_module(id!(doc));
        let libs = vm.new_object();
        vm.set_value_def(module, id_lut!(libs), libs);
        let mut core = language_core(&mut vm).to_vec();
        let shared = vm.new_object();
        vm.set_value_def(shared, id_lut!(shared_name), ScriptValue::from_f64(7.0));
        core.push(shared);
        let mut mem = MemoryModules::default();
        mem.named.push(ModuleSource::new("base", "1", "// the base\nlet K = 3.0\nfn twice(x) { x * 2.0 * K + shared_name * 0.0 }\n").with_file("lib/base.splash").exporting_scope());
        mem.named.push(ModuleSource::new("top", "1", "use mod.doc.libs.base.*\nfn six() { twice(1.0) }\n").with_file("lib/top.splash").exporting_scope());
        mem.named.push(ModuleSource::new("broken", "1", "let a = 1\nlet b = nothing_here + 1\n").with_file("lib/broken.splash").exporting_scope());
        let src = "use mod.doc.libs.top.*\nsix() + K\n";
        let loaded = resolve(&scan(src, Some("mod.doc.libs.")), &mem, Some("mod.doc.libs.")).unwrap();
        assert_eq!(loaded.iter().map(|m| m.reference.key()).collect::<Vec<_>>(), vec!["base", "top"]);
        let errors = load(&mut vm, module, &core, &loaded, Some(libs), 1_000_000);
        assert!(errors.is_empty(), "{errors:?}");
        // The game's `use` brings in what a library defined, and what it used from another.
        vm.capture_errors();
        let v = vm.eval(ScriptMod::new("", "", "doc", 0, 0, &format!("use mod.doc.*\n{src}"), &[]));
        assert!(vm.take_errors().is_empty());
        assert_eq!(v.as_number(), Some(9.0));
        // The language core the libraries saw is not re-exported.
        let exports = vm.bx.heap_ref().value(libs, id!(base).into(), NoTrap).as_object().unwrap();
        let absent = vm.bx.heap_ref().value(exports, id!(shared_name).into(), NoTrap);
        assert!(absent.as_number().is_none(), "a name the module only used is not exported: {absent:?}");
        assert!(vm.bx.heap_ref().value(exports, id!(twice).into(), NoTrap).as_object().is_some());
        // A failing library names its own file and its own line.
        let loaded = resolve(&[ModuleRef::Named("broken".into())], &mem, None).unwrap();
        let errors = load(&mut vm, module, &core, &loaded, Some(libs), 1_000_000);
        assert!(errors.iter().any(|e| e.text().contains("lib/broken.splash:2:")), "{:?}", errors.iter().map(|e| e.text()).collect::<Vec<_>>());
        // A syntax error names the library's file, its line and column, exactly.
        mem.named.push(ModuleSource::new("bad", "1", "let a = 1\nlet 5 = 2\n").with_file("lib/bad.splash").exporting_scope());
        let loaded = resolve(&[ModuleRef::Named("bad".into())], &mem, None).unwrap();
        let errors = load(&mut vm, module, &core, &loaded, Some(libs), 1_000_000);
        let texts: Vec<&str> = errors.iter().map(|e| e.text()).collect();
        assert!(texts.iter().any(|t| t.starts_with("lib/bad.splash:2:6:")), "{texts:?}");
        // A cycle is named.
        mem.named.push(ModuleSource::new("c1", "1", "use mod.doc.libs.c2.*\n").exporting_scope());
        mem.named.push(ModuleSource::new("c2", "1", "use mod.doc.libs.c1.*\n").exporting_scope());
        let err = resolve(&[ModuleRef::Named("c1".into())], &mem, Some("mod.doc.libs.")).unwrap_err();
        assert!(err[0].contains("imports itself") && err[0].contains("c1 -> c2"), "{err:?}");
    }
}
