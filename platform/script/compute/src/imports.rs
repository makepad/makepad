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
//! thing (kernels import them as `use lib("id", "rev") as name`,
//! [`crate::module`]).

use makepad_script::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// A module's source and identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ModuleSource {
    /// A built-in's name, or the asset id of an imported revision.
    pub name: String,
    /// A built-in's version, or the asset revision.
    pub version: String,
    pub text: Arc<str>,
}

/// How a document refers to a module: a host's named module
/// (`mod.motion.modules.<name>`, Canvas built-ins) or a pinned library
/// revision (`import(asset(id, rev))`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModuleRef {
    Named(String),
    Asset { id: String, rev: String },
}

impl ModuleRef {
    pub fn key(&self) -> String {
        match self {
            ModuleRef::Named(n) => n.clone(),
            ModuleRef::Asset { id, rev } => format!("{id}@{rev}"),
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
            }
        }
        out
    }

    /// A library revision from its text.
    pub fn add_asset(&mut self, id: &str, rev: &str, text: &str) {
        self.assets.push((id.into(), rev.into(), ModuleSource { name: id.into(), version: rev.into(), text: text.into() }));
    }
}

impl ModuleResolver for MemoryModules {
    fn named(&self, name: &str) -> Option<ModuleSource> {
        self.named.iter().find(|m| m.name == name).cloned()
    }
    fn asset(&self, id: &str, rev: &str) -> Option<ModuleSource> {
        self.assets.iter().find(|(i, r, _)| i == id && r == rev).map(|(_, _, m)| m.clone())
    }
}

/// The modules `source` refers to, in order of first mention: every
/// `import(asset("id", "rev"))`, and every `<named_prefix><name>` when the
/// host has named modules (`mod.motion.modules.`). Line comments are skipped.
pub fn scan(source: &str, named_prefix: Option<&str>) -> Vec<ModuleRef> {
    let mut out: Vec<ModuleRef> = Vec::new();
    let mut push = |r: ModuleRef| {
        if !out.contains(&r) {
            out.push(r);
        }
    };
    for line in source.lines() {
        let line = match line.find("//") {
            Some(at) if !line[..at].contains('"') => &line[..at],
            _ => line,
        };
        if let Some(prefix) = named_prefix {
            let mut rest = line;
            while let Some(at) = rest.find(prefix) {
                let tail = &rest[at + prefix.len()..];
                let name: String = tail.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if !name.is_empty() {
                    push(ModuleRef::Named(name.clone()));
                }
                rest = &tail[name.len()..];
            }
        }
        let mut rest = line;
        while let Some(at) = rest.find("import(") {
            let tail = &rest[at + "import(".len()..];
            if let Some(r) = parse_asset_call(tail) {
                push(r);
            }
            rest = tail;
        }
    }
    out
}

/// `asset("id", "rev")` at the start of `text`.
fn parse_asset_call(text: &str) -> Option<ModuleRef> {
    let text = text.trim_start().strip_prefix("asset(")?;
    let (id, rest) = string_literal(text.trim_start())?;
    let rest = rest.trim_start().strip_prefix(',')?;
    let (rev, _) = string_literal(rest.trim_start())?;
    Some(ModuleRef::Asset { id, rev })
}

fn string_literal(text: &str) -> Option<(String, &str)> {
    let body = text.strip_prefix('"')?;
    let end = body.find('"')?;
    Some((body[..end].to_string(), &body[end + 1..]))
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
    let exports = match (source.rfind('{'), source.trim_end().ends_with('}')) {
        (Some(at), true) => {
            let body = source[at + 1..].trim_end().trim_end_matches('}');
            body.split_whitespace().filter_map(|w| w.strip_suffix(':')).map(str::to_string).collect()
        }
        _ => Vec::new(),
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
        };
        let Some(source) = source else {
            let hint = match r {
                ModuleRef::Named(name) => resolver.named_hint(name),
                ModuleRef::Asset { .. } => " Publish the module to the library (a Motion asset tagged `module`) and pin its exact revision; a command-line tool takes it as `--module id:rev=file`.".into(),
            };
            errors.push(format!("module {} was not found.{hint}", r.key()));
            return;
        };
        stack.push(r.key());
        for dep in scan(&source.text, named_prefix) {
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

fn string_of(vm: &ScriptVm, v: ScriptValue) -> Option<String> {
    if let Some(s) = v.as_string() {
        return Some(vm.bx.heap.string(s).to_string());
    }
    v.as_inline_string(|s| s.to_string())
}

/// Evaluate the resolved modules in `vm` (in order: dependencies first) and
/// put `import(asset(id, rev))` on `module`, returning each imported
/// revision's exports (modules import modules through it too). Each module
/// runs with `prefix` (the host's `use` lines) before its text, under
/// `instructions`; a named module's exports go to `named` under its name.
pub fn load(vm: &mut ScriptVm, module: ScriptObject, prefix: &str, loaded: &[LoadedModule], named: Option<ScriptObject>, instructions: usize) -> Vec<ScriptErrorRecord> {
    let table: Rc<RefCell<Vec<(String, ScriptValue, ScriptObjectRef)>>> = Rc::new(RefCell::new(Vec::new()));
    let lookup = table.clone();
    vm.add_method(module, id_lut!(import), script_args!(a = NIL), move |vm, args| {
        let a = vm.bx.heap.value(args, id!(a).into(), NoTrap);
        let key = a.as_object().and_then(|o| {
            let id = string_of(vm, vm.bx.heap.value(o, id!(id).into(), NoTrap))?;
            let rev = string_of(vm, vm.bx.heap.value(o, id!(rev).into(), NoTrap))?;
            Some(format!("{id}@{rev}"))
        });
        match key.and_then(|k| lookup.borrow().iter().find(|(key, ..)| *key == k).map(|(_, v, _)| *v)) {
            Some(v) => v,
            None => script_err_invalid_args!(
                vm.bx.threads.cur_ref().trap,
                "import(asset(\"id\", \"rev\")) takes a literal library revision (the loader resolves it before the document runs)"
            ),
        }
    });
    let mut errors = Vec::new();
    for m in loaded {
        vm.bx.captured_errors = Some(Vec::new());
        let value = vm.with_instruction_limit(instructions, |vm| vm.eval(ScriptMod { file: module_file(&m.reference), code: format!("{prefix}{}", m.source.text), ..Default::default() }));
        errors.extend(vm.take_error_records());
        let Some(exports) = value.as_object() else {
            let message = format!("module {}: its last expression must be an object of its exports, like {{rig: rig}}", m.reference.key());
            errors.push(ScriptErrorRecord::plain(message));
            continue;
        };
        match (&m.reference, named) {
            (ModuleRef::Named(name), Some(named)) => {
                let key = LiveId::from_str_with_lut(name).unwrap_or_else(|_| LiveId::from_str(name));
                vm.bx.heap.set_value_def(named, key.into(), value);
            }
            (ModuleRef::Named(_), None) => {}
            (ModuleRef::Asset { .. }, _) => {
                let root = vm.bx.heap.new_object_ref(exports);
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
        let mut vm = ScriptVm { host: &mut *host, bx };
        let module = vm.new_module(id!(doc));
        // The host's `asset(id, rev)` (Motion's and Canvas's give the same fields).
        vm.add_method(module, id_lut!(asset), script_args!(id = NIL, rev = NIL), |vm, args| {
            let o = vm.bx.heap.new_object();
            for k in [id!(id), id!(rev)] {
                let v = vm.bx.heap.value(args, k.into(), NoTrap);
                vm.bx.heap.set_value_def(o, k.into(), v);
            }
            o.into()
        });
        let mut mem = MemoryModules::default();
        mem.add_asset("base", "1", "let k = 3.0\n{k: k}");
        mem.add_asset("looks", "2", "let base = import(asset(\"base\", \"1\"))\nlet twice = fn(x) { x * 2.0 * base.k }\n{twice: twice}");
        let src = "let looks = import(asset(\"looks\", \"2\"))\nlooks.twice(5.0)";
        let loaded = resolve(&scan(src, None), &mem, None).unwrap();
        let prefix = "use mod.doc.*\n";
        let errors = load(&mut vm, module, prefix, &loaded, None, 1_000_000);
        assert!(errors.is_empty(), "{errors:?}");
        vm.bx.captured_errors = Some(Vec::new());
        let v = vm.eval(ScriptMod { file: "doc".into(), code: format!("{prefix}{src}"), ..Default::default() });
        assert!(vm.take_errors().is_empty());
        assert_eq!(v.as_number(), Some(30.0));
    }
}
