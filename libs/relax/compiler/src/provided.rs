//! Provided crates: a crate rustc built into the binary
//! that runs Relax (the `makepad` loader builds makepad-script into itself). Relax still loads
//! its source (types, signatures, generic and inline bodies) but binds the fns the crate marks
//! `#[relax_api]` (relax-api) to the compiled code: each gets a bridge body
//! (lower::bridge_func) with Relax's own ABI that packs its arguments into one uniform C call,
//! `extern "C" fn(args: *const *mut u8, ret: *mut u8) -> u32`, of the wrapper the attribute
//! generated into the crate. Callers, fn pointers and vtables need nothing else: the slot is an
//! ordinary Relax fn.
//!
//! The attribute also marks plain data (`plain`: rustc's size/align/field offsets, checked against
//! Relax's) and opaque handles (`handle`: rustc's size/align/drop and listed trait ops). A marked
//! op must be C-shaped (`unbound_reason`); every unmarked fn of the crate compiles from source when
//! JIT code reaches it.
//!
//! Each op is registered under its key (`core::any::type_name` of the fn item, which Relax
//! computes the same way) with rustc's signature (`type_name` of its fn pointer type) and the
//! layout of each parameter and of the result; a mismatch is a clear "rebuild makepad" error at
//! bind time, never a wrong call.
//!
//! Before binding, every registered crate present in the program must match the loader's
//! library source closure (srchash.rs). A mismatch offers a loader update with the changed
//! inputs; it never binds the stale library or selects a second source copy.

use crate::lang_items::LangItem;
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::types::{TyId, TyKind};
use std::collections::HashMap;
use relax_session::sync::Mutex;

/// A generated shim: arguments moved out of `args[i]`, the result written to `ret`; 0 = returned,
/// 1 = panicked (the message is then read with the crate's LastPanicFn), 2 = the calling thread's
/// own panic unwound through it (relax_api::CallerPanic: the caller goes on with it).
pub type ShimFn = unsafe extern "C" fn(*const *mut u8, *mut u8) -> u32;
/// Writes the text of the last panic a shim caught on this thread: (ptr, len), valid until the
/// thread's next shim call.
pub type LastPanicFn = unsafe extern "C" fn(*mut *const u8, *mut usize);

/// A provided type rustc lays out (its fields are the crate's business): size, align, and
/// its `drop_in_place` when it needs drop.
pub type DropFn = unsafe extern "C" fn(*mut u8);

/// One op of a provided crate: rustc's signature (`type_name` of its fn pointer type; empty for a
/// handle's trait ops), per parameter then the result rustc's [size, align, pointee size, pointee
/// align], and the wrapper.
#[derive(Clone, Copy)]
pub struct Shim {
    pub sig: &'static str,
    pub slots: &'static [[u64; 4]],
    pub f: ShimFn,
}

pub struct ProvidedCrate {
    /// the crate's name as Relax loads it (`makepad_script`)
    pub name: String,
    /// key (`type_name` of the fn item; a handle's trait ops `<path as Clone>::clone` ..) -> op
    pub shims: HashMap<String, Shim>,
    /// opaque handles by def path: (size, align, drop, a struct's field offsets in declaration
    /// order: same_layout_handles, primitive-repr enum layout table)
    pub types: HashMap<String, (u64, u64, Option<DropFn>, &'static [u64], &'static [u64])>,
    /// plain types by def path: rustc's (size, align, field offsets, enum layout table).
    /// Size, align, struct field offsets and enum layout are checked against Relax's at
    /// unit creation and when a run adds provided crates.
    pub layouts: HashMap<String, (u64, u64, &'static [u64], &'static [u64])>,
    /// statics marked `#[relax_api(per_side)]`, by def path: JIT code keeps its own instance
    /// (lower.rs check_provided_static)
    pub per_side: std::collections::HashSet<String>,
    /// Statics marked bare `#[relax_api]`, by def path: (the loader's address, rustc's size, align).
    pub shared: HashMap<String, (usize, u64, u64)>,
    pub last_panic: LastPanicFn,
    /// the source identity rustc built it from (srchash.rs; the loader's build.rs): what
    /// check_sources compares the program's copy of the crate with
    pub source: u64,
    /// what JIT code sees of the binding (`binding_identity`): the cache keys of code that can
    /// reach the crate carry this, not `source` (cache/items.rs apply_bound)
    pub binding: u64,
    /// Per-file and feature/version identity embedded by the loader build.
    pub manifest: &'static [u8],
    /// the crate's `#[relax_api]` entries as the loader's linker laid them down (its
    /// `relax_api::entries()`, in their order): the program's `relax_api` table takes these where
    /// rustc's link would put the crate's own (sections.rs lay_out)
    pub table: Vec<&'static relax_api::Entry>,
}

impl Clone for ProvidedCrate {
    fn clone(&self) -> Self {
        ProvidedCrate { name: self.name.clone(), shims: self.shims.clone(), types: self.types.clone(), layouts: self.layouts.clone(), per_side: self.per_side.clone(), shared: self.shared.clone(), last_panic: self.last_panic, source: self.source, binding: self.binding, manifest: self.manifest, table: self.table.clone() }
    }
}

static REGISTRY: Mutex<Vec<ProvidedCrate>> = Mutex::new(Vec::new());

/// The binary's const-verified Option layouts, keyed by normalised type name.
/// Collected once before crate registration; tagged Options must match before sharing bytes.
static OPTION_LAYOUTS: std::sync::OnceLock<HashMap<String, [u64; 6]>> = std::sync::OnceLock::new();

/// Installs the loader's complete Option table before any provided crate is registered.
pub fn register_option_layouts(layouts: HashMap<String, [u64; 6]>) {
    OPTION_LAYOUTS.set(layouts).unwrap_or_else(|_| panic!("Option layouts already registered"));
}

pub(crate) fn registered(name: &str) -> bool {
    let name = name.replace('-', "_");
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.name == name)
}

/// Registers a provided crate (before `cli_main`): its `#[relax_api]` entries as the loader read
/// them from its own binary (relax_api::entries; `table`: all of them, as laid down), and the
/// panic-text accessor.
pub fn register(
    name: &str,
    shims: HashMap<String, Shim>,
    types: HashMap<String, (u64, u64, Option<DropFn>, &'static [u64], &'static [u64])>,
    layouts: HashMap<String, (u64, u64, &'static [u64], &'static [u64])>,
    per_side: std::collections::HashSet<String>,
    shared: HashMap<String, (usize, u64, u64)>,
    last_panic: LastPanicFn,
    source: u64,
    manifest: &'static [u8],
    table: Vec<&'static relax_api::Entry>,
) {
    let name = name.replace('-', "_");
    let binding = binding_identity(&name, &shims, &types, &layouts, &per_side, &shared);
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).push(ProvidedCrate { name, shims, types, layouts, per_side, shared, last_panic, source, binding, manifest, table });
}

/// The identity of what JIT code depends on in a provided crate beyond its source text (which
/// the interface fingerprints and body keys already cover, as for any crate): which fns are
/// bound and their rustc signatures and argument layouts (the bridge bodies, lower.rs
/// bridge_func, call shims by symbol), the opaque handles' rustc size, align, drop and field
/// offsets, the plain types' rustc layouts, and the per-side/shared statics with shared rustc
/// sizes and alignments (never addresses, which differ per process), and the binary's Option
/// layout table. The program's copy of the crate is the loader's own source (check_sources),
/// so a change of these follows a source
/// change; a change of the crate's native fn bodies alone (a HAL fix) keeps them, and with them
/// every stored build of the code above the crate.
fn binding_identity(
    name: &str,
    shims: &HashMap<String, Shim>,
    types: &HashMap<String, (u64, u64, Option<DropFn>, &'static [u64], &'static [u64])>,
    layouts: &HashMap<String, (u64, u64, &'static [u64], &'static [u64])>,
    per_side: &std::collections::HashSet<String>,
    shared: &HashMap<String, (usize, u64, u64)>,
) -> u64 {
    let mut h = crate::sip::Sip128::new();
    h.str("provided-binding");
    h.str(name);
    let mut keys: Vec<&String> = shims.keys().collect();
    keys.sort_unstable();
    h.u32(keys.len() as u32);
    for k in keys {
        let s = &shims[k];
        h.str(k);
        h.str(s.sig);
        h.u32(s.slots.len() as u32);
        for slot in s.slots { for x in slot { h.u64(*x); } }
    }
    let mut keys: Vec<&String> = types.keys().collect();
    keys.sort_unstable();
    h.u32(keys.len() as u32);
    for k in keys {
        let (size, align, drop, offsets, enum_layout) = &types[k];
        h.str(k);
        h.u64(*size);
        h.u64(*align);
        h.u8(drop.is_some() as u8);
        h.u32(offsets.len() as u32);
        for x in offsets.iter() { h.u64(*x); }
        h.u32(enum_layout.len() as u32);
        for x in enum_layout.iter() { h.u64(*x); }
    }
    let mut keys: Vec<&String> = layouts.keys().collect();
    keys.sort_unstable();
    h.u32(keys.len() as u32);
    for k in keys {
        let (size, align, offsets, enum_layout) = &layouts[k];
        h.str(k);
        h.u64(*size);
        h.u64(*align);
        h.u32(offsets.len() as u32);
        for x in offsets.iter() { h.u64(*x); }
        h.u32(enum_layout.len() as u32);
        for x in enum_layout.iter() { h.u64(*x); }
    }
    let mut keys: Vec<&String> = per_side.iter().collect();
    keys.sort_unstable();
    h.u32(keys.len() as u32);
    for k in keys {
        h.str(k);
    }
    let mut keys: Vec<&String> = shared.keys().collect();
    keys.sort_unstable();
    h.u32(keys.len() as u32);
    for k in keys {
        let (_, size, align) = &shared[k];
        h.str(k);
        h.u64(*size);
        h.u64(*align);
    }
    let options = OPTION_LAYOUTS.get();
    let mut keys: Vec<&String> = options.into_iter().flat_map(|m| m.keys()).collect();
    keys.sort_unstable();
    h.u32(keys.len() as u32);
    if let Some(options) = options {
        for k in keys {
            h.str(k);
            for word in options[k] { h.u64(word); }
        }
    }
    h.finish128() as u64
}

/// The registered crates whose source in the program is what the loader was built from
/// (check_sources); only they bind.
static MATCHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// A provided library changed on disk. These are source paths, including removed files, not
/// a parsed diagnostic. They also identify the landing and the inputs of the loader rebuild.
/// `features`: the closure's packages whose features differ too ([`features_line`]); a
/// features-only difference is no change a rebuild applies, and check_manifest refuses it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvidedChange {
    pub crate_name: String,
    pub files: Vec<String>,
    pub features: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceCheckError {
    Changed(ProvidedChange),
    Invalid(String),
}

impl From<String> for SourceCheckError {
    fn from(message: String) -> Self { Self::Invalid(message) }
}

fn check_manifest(name: &str, source: u64, manifest: &[u8], current: &crate::srchash::Manifest, pkgs: &[crate::srchash::SrcPkg]) -> Result<(), SourceCheckError> {
    if crate::srchash::manifest_hash(current) == source { return Ok(()); }
    // what the program's graph builds of the closure (srchash reached_by): a dependency that only
    // the loader's host-shims brings in is no change
    let built = crate::srchash::reached_by(&crate::srchash::decode_manifest(manifest)?, current);
    if crate::srchash::diff_entries(&built, current).next().is_none() { return Ok(()); }
    let mut files = std::collections::BTreeSet::new();
    let mut features = Vec::new();
    for key @ (package, path) in crate::srchash::diff_entries(&built, current) {
        let package = package.strip_prefix("host:").unwrap_or(package);
        let Some((pname, _version)) = package.rsplit_once('@') else { continue };
        if path == "\0features" {
            features.push(features_line(pname, built.get(key), current.get(key)));
            continue;
        }
        for pkg in pkgs.iter().filter(|p| p.name.replace('-', "_") == pname) {
            // a version edit changes the manifest; a feature turned on need not edit any file
            let path = if path == "\0version" { "Cargo.toml" } else { path.as_str() };
            files.insert(crate::vfs::normalize_path(&std::path::Path::new(&pkg.dir).join(path).to_string_lossy()));
        }
    }
    // The program's graph has the loader's features (run.rs unify_for), so what is left is a
    // feature the program turns on that the loader's build does not: no rebuild of the same
    // loader changes that, so it is no update to offer.
    if files.is_empty() {
        return Err(SourceCheckError::Invalid(format!("the program builds {name}'s closure with features this JIT was built without ({}): tools/run/Cargo.toml sets the JIT's", features.join("; "))));
    }
    Err(SourceCheckError::Changed(ProvidedChange { crate_name: name.into(), files: files.into_iter().collect(), features }))
}

/// One package's feature difference: `<name> features: <the loader's> -> <the program's>`, with
/// `(none)` for none (`makepad_script_compute features: vm -> (none)`).
fn features_line(name: &str, built: Option<&String>, current: Option<&String>) -> String {
    let list = |v: Option<&String>| v.map(String::as_str).filter(|s| !s.is_empty()).unwrap_or("(none)").to_string();
    format!("{name} features: {} -> {}", list(built), list(current))
}

/// Whether turning `feature` on in the package at `dir` only adds code: None, or the first use
/// that changes code which builds without it ("<rel path>:<line>: `<the cfg text>` <why>").
/// Unreadable or unlexable source is refused too, since its uses cannot be checked.
pub fn non_additive_use(dir: &str, feature: &str) -> Option<String> {
    use crate::lexer::{self, Tok, T};

    fn group_end(toks: &[Tok], open: usize) -> Option<usize> {
        let mut depth = 0;
        for (i, t) in toks.iter().enumerate().skip(open) {
            match t.kind {
                T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::CloseParen | T::CloseBracket | T::CloseBrace => {
                    depth -= 1;
                    if depth == 0 { return Some(i); }
                }
                _ => {}
            }
        }
        None
    }

    // Top-level commas separate a cfg_attr predicate from its attributes, and the attributes
    // from one another. Commas in any/all/derive belong to their own group.
    fn parts(toks: &[Tok]) -> Vec<&[Tok]> {
        let mut out = Vec::new();
        let (mut start, mut i) = (0, 0);
        while i < toks.len() {
            match toks[i].kind {
                T::OpenParen | T::OpenBracket | T::OpenBrace => i = group_end(toks, i).unwrap_or(toks.len()),
                T::Comma => { out.push(&toks[start..i]); start = i + 1; }
                _ => {}
            }
            i += 1;
        }
        if start < toks.len() { out.push(&toks[start..]); }
        out
    }

    // (present, under not). Only literal values following the feature key count; token text
    // in comments, doc comments and unrelated string literals never enters this scan.
    fn predicate(src: &str, toks: &[Tok], feature: &str) -> (bool, bool) {
        let mut found = false;
        let mut not_depth = 0;
        let mut groups = Vec::new();
        for (i, t) in toks.iter().enumerate() {
            match t.kind {
                T::OpenParen => {
                    let negated = i > 0 && toks[i - 1].text(src) == "not";
                    groups.push(negated);
                    if negated { not_depth += 1; }
                }
                T::CloseParen => { if groups.pop() == Some(true) { not_depth -= 1; } }
                T::Ident | T::RawIdent if t.text(src).trim_start_matches("r#") == "feature" => {
                    if toks.get(i + 1).is_some_and(|t| t.kind == T::Eq) {
                        if let Some(value) = toks.get(i + 2).filter(|t| matches!(t.kind, T::Str | T::RawStr)) {
                            if crate::lit::unescape_str(value.text(src)) == feature {
                                found = true;
                                if not_depth > 0 { return (true, true); }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        (found, false)
    }

    fn attribute(src: &str, toks: &[Tok], feature: &str) -> Option<(u32, u32, &'static str)> {
        if toks.get(1)?.kind != T::OpenParen { return None; }
        let end = group_end(toks, 1)?;
        let reason = |why| Some((toks[0].lo, toks[end].hi, why));
        match toks[0].text(src) {
            "cfg" => {
                if predicate(src, &toks[2..end], feature).1 { return reason("turns code off when it is on"); }
            }
            "cfg_attr" => {
                let args = parts(&toks[2..end]);
                let (pred, attrs) = args.split_first()?;
                let (found, negated) = predicate(src, pred, feature);
                if negated { return reason("turns code off when it is on"); }
                let derives_only = attrs.iter().all(|a| {
                    a.first().is_some_and(|t| t.text(src) == "derive")
                        && a.get(1).is_some_and(|t| t.kind == T::OpenParen)
                        && group_end(a, 1) == Some(a.len() - 1)
                });
                if found && !derives_only { return reason("changes an item's attributes"); }
                for attr in attrs {
                    if let Some(found) = attribute(src, attr, feature) { return Some(found); }
                }
            }
            _ => {}
        }
        None
    }

    let dir = std::path::Path::new(dir);
    for (rel, _) in crate::srchash::identity_files(dir) {
        if !rel.ends_with(".rs") { continue; }
        let src = match crate::vfs::read_to_string(dir.join(&rel)) {
            Ok(src) => src,
            Err(e) => return Some(format!("{rel}:1: cannot read feature uses: {e}")),
        };
        let mut toks = Vec::new();
        // The cfg syntax and literal tokens are edition-independent.
        if let Err(e) = lexer::lex(src.as_bytes(), 2021, &mut toks) {
            let (line, _) = lexer::line_col(src.as_bytes(), e.pos);
            return Some(format!("{rel}:{line}: cannot lex feature uses: {}", e.msg));
        }
        toks.retain(|t| !t.kind.is_trivia());
        for (i, t) in toks.iter().enumerate() {
            let found = if t.kind == T::Pound {
                let open = i + 1 + usize::from(toks.get(i + 1).is_some_and(|t| t.kind == T::Not));
                if toks.get(open).is_none_or(|t| t.kind != T::OpenBracket) { continue; }
                let Some(end) = group_end(&toks, open) else { continue };
                attribute(&src, &toks[open + 1..end], feature)
            } else if t.kind == T::Ident && t.text(&src) == "cfg"
                && toks.get(i + 1).is_some_and(|t| t.kind == T::Not)
                && toks.get(i + 2).is_some_and(|t| matches!(t.kind, T::OpenParen | T::OpenBracket | T::OpenBrace))
            {
                let Some(end) = group_end(&toks, i + 2) else { continue };
                predicate(&src, &toks[i + 3..end], feature).0.then_some((t.lo, toks[end].hi, "switches code at run time"))
            } else { None };
            if let Some((lo, hi, why)) = found {
                let (line, _) = lexer::line_col(src.as_bytes(), lo);
                return Some(format!("{rel}:{line}: `{}` {why}", &src[lo as usize..hi as usize]));
            }
        }
    }
    None
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::srchash::{encode_manifest, manifest_hash, Manifest, SrcPkg};

    #[test]
    fn provided_source_outcome_preserves_changed_inputs() {
        let pkgs = [SrcPkg { name: "fixture-dep".into(), version: "1".into(), dir: "fixture".into(), features: Vec::new(), deps: Vec::new(), proc_macro: false }];
        let mut built = Manifest::new();
        // More than the diagnostic's display limit: no input is lost from the typed outcome.
        for i in 0..24 { built.insert(("fixture_dep@1".into(), format!("src/file{i}.rs")), "old".into()); }
        built.insert(("fixture_dep@1".into(), "src/removed.rs".into()), "removed".into());
        built.insert(("fixture_dep@1".into(), "src/unchanged.rs".into()), "same".into());
        built.insert(("host:fixture_dep@1".into(), "src/file0.rs".into()), "old".into());
        built.insert(("fixture_dep@1".into(), "\0features".into()), "".into());
        let source = manifest_hash(&built);
        let encoded = encode_manifest(&built);
        assert_eq!(check_manifest("fixture", source, &encoded, &built, &pkgs), Ok(()));

        let mut current = built.clone();
        for i in 0..24 { current.insert(("fixture_dep@1".into(), format!("src/file{i}.rs")), "new".into()); }
        current.remove(&("fixture_dep@1".into(), "src/removed.rs".into()));
        current.insert(("fixture_dep@1".into(), "src/added.rs".into()), "added".into());
        current.insert(("host:fixture_dep@1".into(), "src/file0.rs".into()), "new".into());
        current.insert(("fixture_dep@1".into(), "\0features".into()), "enabled".into());
        let Err(SourceCheckError::Changed(change)) = check_manifest("fixture", source, &encoded, &current, &pkgs) else { panic!("source edits must be an update offer") };
        assert_eq!(change.crate_name, "fixture");
        let mut expected: Vec<_> = (0..24).map(|i| format!("fixture/src/file{i}.rs")).collect();
        expected.extend(["fixture/src/removed.rs".into(), "fixture/src/added.rs".into()]);
        expected.sort();
        assert_eq!(change.files, expected, "a feature difference names no manifest");
        assert_eq!(change.features, ["fixture_dep features: (none) -> enabled"]);

        // A feature-only mismatch is no update a rebuild applies: refused, with the features and
        // no file to rebuild from; a version change still watches the manifest; corrupt embedded
        // data remains an error.
        let mut features = built.clone();
        features.insert(("fixture_dep@1".into(), "\0features".into()), "enabled".into());
        assert_eq!(check_manifest("fixture", source, &encoded, &features, &pkgs), Err(SourceCheckError::Invalid("the program builds fixture's closure with features this JIT was built without (fixture_dep features: (none) -> enabled): tools/run/Cargo.toml sets the JIT's".into())));
        let mut version = built.clone();
        version.insert(("fixture_dep@1".into(), "\0version".into()), "2".into());
        assert_eq!(check_manifest("fixture", source, &encoded, &version, &pkgs), Err(SourceCheckError::Changed(ProvidedChange { crate_name: "fixture".into(), files: vec!["fixture/Cargo.toml".into()], features: vec![] })), "version changes still watch the manifest");
        assert!(matches!(check_manifest("fixture", source, &[1], &current, &pkgs), Err(SourceCheckError::Invalid(_))));
    }

    #[test]
    fn feature_additivity_checks_cfg_tokens() {
        struct Temp(std::path::PathBuf);
        impl Drop for Temp { fn drop(&mut self) { std::fs::remove_dir_all(&self.0).unwrap(); } }
        let dir = Temp(std::env::temp_dir().join(format!("relax-feature-uses-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
        std::fs::create_dir_all(dir.0.join("src")).unwrap();
        std::fs::write(dir.0.join("Cargo.toml"), "[package]\nname = 'fixture'\nversion = '1.0.0'\n[features]\nF = []\n").unwrap();
        let path = dir.0.join("src/lib.rs");
        let check = || non_additive_use(&dir.0.to_string_lossy(), "F");
        std::fs::write(&path, r###"#[cfg(feature = "F")]
fn added() {}
struct Fields { #[cfg(all(unix, any(feature = "F", test)))] extra: u32 }
fn statements() { #[cfg(feature = "F")] let _ = 1; }
#[cfg_attr(feature = "F", derive(Debug, Clone), derive(Copy))]
struct Derived;
// #[cfg(not(feature = "F"))]
/* cfg!(feature = "F") */
/// #[cfg_attr(feature = "F", repr(C))]
const TEXT: &str = r#"cfg!(feature = "F")"#;
const ESCAPED: &str = "#[cfg(not(feature = \"F\"))]";
#[cfg(not(feature = "other"))]
fn other_feature() {}
"###).unwrap();
        assert_eq!(check(), None, "items, fields, statements and derives add code; comments and strings are inert");
        for (form, cfg, why) in [
            ("negated cfg", "cfg(not(feature = \"F\"))", "turns code off when it is on"),
            ("nested negation", "cfg(all(unix, any(test, not(feature = \"F\"))))", "turns code off when it is on"),
            ("cfg expression", "cfg!(all(unix, feature = \"F\"))", "switches code at run time"),
            ("attribute predicate", "cfg_attr(any(test, feature = \"F\"), repr(C))", "changes an item's attributes"),
            ("mixed attributes", "cfg_attr(feature = \"F\", derive(Debug), repr(C))", "changes an item's attributes"),
        ] {
            let body = if cfg.starts_with("cfg!") { format!("const FLAG: bool = {cfg};") } else { format!("#[{cfg}]\nstruct Item;") };
            std::fs::write(&path, format!("// line one\n\n{body}\n")).unwrap();
            assert_eq!(check(), Some(format!("src/lib.rs:3: `{cfg}` {why}")), "{form}");
        }
    }

    #[test]
    fn a_dependency_only_the_loaders_shims_bring_in_is_no_change() {
        use crate::srchash::closure_manifest;
        // relax-api's relax-api-phase: optional behind host-shims (the loader's build enables
        // it, a program never does) and a dependency on Apple's targets only
        let pkg = |name: &str, features: &[&str], deps: Vec<usize>| SrcPkg { name: name.into(), version: "1".into(), dir: format!("/.cargo/registry/{name}"),
            features: features.iter().map(|f| f.to_string()).collect(), deps, proc_macro: false };
        let built = closure_manifest(&[pkg("fixture", &["host-shims"], vec![1]), pkg("fixture-phase", &[], vec![])], 0);
        let (source, encoded) = (manifest_hash(&built), encode_manifest(&built));
        let check = |pkgs: &[SrcPkg]| check_manifest("fixture", source, &encoded, &closure_manifest(pkgs, 0), pkgs);
        assert_eq!(check(&[pkg("fixture", &[], vec![])]), Ok(()), "Linux: the program's graph builds the crate without the dependency");
        assert_eq!(check(&[pkg("fixture", &[], vec![1]), pkg("fixture-phase", &[], vec![])]), Ok(()), "Apple: both graphs have it");
        // what the program builds still counts
        let changed = |result: Result<(), SourceCheckError>| matches!(result, Err(SourceCheckError::Changed(_)));
        assert!(changed(check(&[pkg("fixture", &[], vec![1]), pkg("fixture-other", &[], vec![])])), "a dependency only the program has");
        let mut moved = [pkg("fixture", &[], vec![1]), pkg("fixture-phase", &[], vec![])];
        moved[1].version = "2".into();
        assert!(changed(check(&moved)), "another version of the dependency");
        assert!(matches!(check(&[pkg("fixture", &["other"], vec![])]), Err(SourceCheckError::Invalid(m)) if m.contains("fixture features: (none) -> other")), "a feature the identity keeps: refused, no rebuild offered");
    }
}

/// The one crate a test harness binds. Its other crates are the tests' subject, built from source;
/// the CPU budget is the process's one, and a copy built from source would admit the tests' pool
/// beside the JIT, neither seeing the other.
pub const HARNESS_BINDS: &str = "makepad_cpu_budget";

/// The registered crates' closures as the loader's build resolved them (`only`: that crate's),
/// for a resolve that binds them: the program's graph builds their packages with the loader's
/// features (resolve.rs Unify), as cargo unifies a package's features across a workspace. Read
/// from the embedded manifests once; the registry is complete before the first resolve.
pub fn unify_groups(only: Option<&str>) -> Vec<crate::resolve::Unify> {
    static GROUPS: std::sync::OnceLock<Vec<crate::resolve::Unify>> = std::sync::OnceLock::new();
    let groups = GROUPS.get_or_init(|| {
        let reg: Vec<(String, &'static [u8])> = REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|p| (p.name.clone(), p.manifest)).collect();
        let _k = crate::zone_alloc::kept();
        reg.into_iter().filter_map(|(name, manifest)| {
            // the manifest's (package, path, value) fields (srchash encode_manifest), read in
            // place: only the packages' feature entries are kept, not every identity file's hash
            let mut g = crate::resolve::Unify::default();
            let mut b = manifest;
            let mut field = || -> Option<&'static str> {
                let n = usize::try_from(u64::from_le_bytes(b.get(..8)?.try_into().ok()?)).ok()?;
                let s = std::str::from_utf8(b.get(8..8 + n)?).ok()?;
                b = &b[8 + n..];
                Some(s)
            };
            while let (Some(package), Some(path), Some(features)) = (field(), field(), field()) {
                if path != "\0features" || package.starts_with("host:") { continue; }
                let Some((pname, version)) = package.rsplit_once('@') else { continue };
                let id = (pname.to_string(), version.to_string());
                if pname == name { g.root = id.clone(); }
                g.pkgs.push((id, features.split(", ").filter(|f| !f.is_empty()).map(str::to_string).collect()));
            }
            (!g.root.0.is_empty()).then_some(g)
        }).collect()
    });
    groups.iter().filter(|g| only.is_none_or(|o| g.root.0 == o)).cloned().collect()
}

const IDENTITY_THREADS: usize = 4;

/// Compares registered crates (`only`: that one) against the embedded library closure; absent
/// crates skip.
pub fn check_sources(pkgs: &[crate::srchash::SrcPkg], only: Option<&str>) -> Result<(), SourceCheckError> {
    let reg: Vec<_> = REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter()
        .filter(|p| only.is_none_or(|o| p.name == o))
        .map(|p| (p.name.clone(), p.source, p.manifest)).collect();
    let mut verified = Vec::new();
    let mut files = std::collections::HashMap::new();
    // the identity files of every package folder the registered crates reach, read and hashed
    // by a few threads (a folder is hashed once whatever number of closures reach it)
    let mut dirs: Vec<String> = Vec::new();
    for (name, _, _) in &reg {
        for (root, _) in pkgs.iter().enumerate().filter(|(_, k)| k.name.replace('-', "_") == *name) {
            for d in crate::srchash::closure_dirs(pkgs, root) { if !dirs.contains(&d) { dirs.push(d); } }
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done: Vec<Mutex<Option<Vec<(String, u64)>>>> = dirs.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..dirs.len().min(IDENTITY_THREADS) {
            scope.spawn(|| {
                let _z = crate::zone_alloc::compiler_thread();
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(dir) = dirs.get(k) else { break };
                    *done[k].lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::srchash::identity_files(std::path::Path::new(dir)));
                }
            });
        }
    });
    for (dir, list) in dirs.iter().zip(done) {
        if let Some(list) = list.into_inner().unwrap_or_else(|e| e.into_inner()) { files.insert(dir.clone(), list); }
    }
    for (name, source, manifest) in &reg {
        for (root, _) in pkgs.iter().enumerate().filter(|(_, k)| k.name.replace('-', "_") == *name) {
            let current = crate::srchash::closure_manifest_with(pkgs, root, &mut files);
            check_manifest(name, *source, manifest, &current, pkgs)?;
            if !verified.contains(name) { verified.push(name.clone()); }
        }
    }
    let mut matched = MATCHED.lock().unwrap_or_else(|e| e.into_inner());
    for name in verified {
        if !matched.contains(&name) {
            crate::zone_alloc::kept_with(|| matched.push(name.clone()));
        }
    }
    Ok(())
}

/// The boot's source check, running beside the load (`check_sources_later`).
static PENDING: Mutex<Option<std::thread::JoinHandle<Result<(), SourceCheckError>>>> = Mutex::new(None);

/// `check_sources`, on a thread of its own. The boot starts it as early as `check_sources` could
/// run and goes on loading; nothing may bind a provided crate before `finish_sources` has
/// returned Ok (`take_for`, `take_for_crates` and the boot's own join before the unit is built
/// call it). The hashes, the manifests and the error are those of `check_sources`.
pub fn check_sources_later(pkgs: std::sync::Arc<Vec<crate::srchash::SrcPkg>>, only: Option<&'static str>) {
    let _k = crate::zone_alloc::kept();
    let spawned = crate::run_threads::spawn("run-provided-sources", move || {
        let _z = crate::zone_alloc::compiler_thread();
        let t = std::time::Instant::now();
        let verdict = check_sources(&pkgs, only);
        if crate::preload::trace() { eprintln!("boot-trace: provided sources checked in {:.1} ms (beside the load)", t.elapsed().as_secs_f64() * 1e3); }
        verdict
    });
    match spawned {
        Ok(h) => *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(h),
        Err(e) => *VERDICT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(SourceCheckError::Invalid(format!("provided source check: {e}")))),
    }
}

/// The verdict of the check `check_sources_later` started, once it has ended.
static VERDICT: Mutex<Option<Result<(), SourceCheckError>>> = Mutex::new(None);

/// Waits for the check `check_sources_later` started, if one is running: its verdict (every
/// caller gets it, the proc-macro world's unit as well as the program's). Ok when none started.
pub fn finish_sources() -> Result<(), SourceCheckError> {
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = pending.take() {
        let verdict = h.join().unwrap_or_else(|_| Err(SourceCheckError::Invalid("provided source check: the thread panicked".into())));
        *VERDICT.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::zone_alloc::kept_clone(&verdict));
    }
    VERDICT.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or(Ok(()))
}

/// Reads what `check_sources` reads, for the identity of every registered crate in the program,
/// and nothing else, each package folder once. A check for another target binds nothing, but the
/// run on that target (the phone) compares these identities: `pack-src` records the files this
/// reads so its archive holds them (run.rs calls it only while the read log records).
pub fn read_identity_files(pkgs: &[crate::srchash::SrcPkg]) {
    let reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    let mut files = std::collections::HashMap::new();
    for p in reg.iter() {
        for (root, _) in pkgs.iter().enumerate().filter(|(_, k)| k.name.replace('-', "_") == p.name) {
            let _ = crate::srchash::closure_manifest_with(pkgs, root, &mut files);
        }
    }
}

/// The loader's provided crates present in a source-only program, matched by name as
/// `take_for` matches them, without a source-identity check or any host binding table.
pub(crate) fn registered_in(prog: &Program) -> Vec<u32> {
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter()
        .filter_map(|p| prog.crates.iter().position(|c| prog.syms.str(c.name) == p.name).map(|i| i as u32))
        .collect()
}

/// The registered crates as (crate index in `prog`, entry) whose source matched (check_sources);
/// crates not in the program are left out. Taken once per unit (Unit::new).
pub fn take_for(prog: &Program) -> Vec<(u32, ProvidedCrate)> {
    // nothing binds before the source check has passed (a failed one binds nothing)
    if finish_sources().is_err() { return Vec::new(); }
    let matched = MATCHED.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for p in REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if !matched.contains(&p.name) {
            continue;
        }
        if let Some(i) = prog.crates.iter().position(|c| prog.syms.str(c.name) == p.name) {
            out.push((i as u32, p.clone()));
        }
    }
    out
}

/// As [`take_for`], for the crates of `prog` named by `only` (indices a run-time load added) that
/// are not bound yet.
pub fn take_for_crates(prog: &Program, only: &[u32]) -> Vec<(u32, ProvidedCrate)> {
    if finish_sources().is_err() { return Vec::new(); }
    let matched = MATCHED.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for p in REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        if !matched.contains(&p.name) {
            continue;
        }
        for &i in only {
            if prog.syms.str(prog.crates[i as usize].name) == p.name {
                out.push((i, p.clone()));
            }
        }
    }
    out
}

/// Is crate `krate` of `prog` a registered provided crate?
fn is_provided(prog: &Program, krate: u32) -> bool {
    let name = prog.syms.str(prog.crates[krate as usize].name);
    REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.name == name)
}

/// `#[repr(transparent)]` on an ADT (Relax's AdtT records repr(C)/int only; transparent lays
/// out as its one non-ZST field, which `trusted` checks).
fn is_transparent(prog: &Program, d: DefId) -> bool {
    let def = prog.def(d);
    let f = &prog.files[def.file as usize];
    f.ast.list(f.ast.item(def.item).attrs).iter().any(|a| {
        a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "repr" && (a.toks.lo + 1..a.toks.hi).any(|i| f.tok_text(i) == "transparent")
    })
}

/// Identity flags follow rustc_middle/src/ty/adt.rs:371; host layout checks remain required.
/// Is `t` the same bytes on both sides of the boundary (rustc-built crate, Relax-built caller)?
/// `krate` is the provided crate: pointers to its own types are opaque handles.
pub fn trusted(prog: &Program, tcx: &Tcx, krate: u32, t: TyId, depth: u32) -> bool {
    trusted_in(prog, tcx, krate, t, depth, false, false)
}

/// The same bytes on both sides, including cells and atomics of trusted values.
pub fn shared_class(prog: &Program, tcx: &Tcx, krate: u32, t: TyId) -> bool {
    trusted_in(prog, tcx, krate, t, 0, false, true)
}

/// How JIT code reaches static `d` (of type `t`) when its crate is provided.
pub enum StaticUse {
    NotProvided,
    Shared(usize),
    OwnInstance,
    Copy,
    Refused(String),
}

struct SharedStaticRange {
    start: u64,
    /// Inclusive last address; a zero-sized static owns just its start.
    last: u64,
    /// Largest last address through this entry, so misses skip earlier disjoint ranges.
    max_last: u64,
    key: String,
}

pub(crate) struct SharedStaticIndex(Vec<SharedStaticRange>);

/// The shared provided static holding `addr` (a pointer to it stored in constant data): its
/// `static:` key and start address.
pub fn shared_at(u: &crate::jit::Unit, addr: u64) -> Option<(String, u64)> {
    // Queried for every otherwise unattributed pointer word. Sort once per Unit rather than
    // scanning every crate's map on each miss; bind_added_provided invalidates the index.
    let index = u.provided_static_index.get_or_init(|| {
        let _k = crate::zone_alloc::kept();
        let mut ranges: Vec<_> = u.provided.iter().flat_map(|(_, p)| p.shared.iter()).map(|(key, &(start, size, _))| {
            let start = start as u64;
            SharedStaticRange { start, last: start.saturating_add(size.saturating_sub(1)), max_last: 0, key: key.clone() }
        }).collect();
        ranges.sort_unstable_by(|a, b| (a.start, a.last, &a.key).cmp(&(b.start, b.last, &b.key)));
        let mut max_last = 0;
        for r in &mut ranges {
            max_last = max_last.max(r.last);
            r.max_last = max_last;
        }
        SharedStaticIndex(ranges)
    });
    let end = index.0.partition_point(|r| r.start <= addr);
    // Retain aliases and zero-sized statics even if they overlap a nonempty range.
    index.0[..end].iter().rev().take_while(|r| r.max_last >= addr)
        .find(|r| r.last >= addr).map(|r| (r.key.clone(), r.start))
}

pub fn static_use(u: &crate::jit::Unit, d: DefId, t: TyId) -> StaticUse {
    let prog = &u.prog;
    let tcx = &u.tcx;
    let def = prog.def(d);
    let krate = def.krate;
    let Some((_, p)) = u.provided.iter().find(|(k, _)| *k == krate) else { return StaticUse::NotProvided };
    let path = prog.def_path(d);
    let key = norm(&path);
    let class_2 = || {
        let ts = crate::typeck::ty_to_string(false, prog, tcx, t);
        if let Some(reason) = option_layout_reason(prog, tcx, t) {
            return StaticUse::Refused(format!("static `{path}` (`{ts}`): {reason}"));
        }
        StaticUse::Refused(format!("static `{path}` (`{ts}`) is laid out and run by rustc's std on the loader's side and by Relax's on this side (a lock, a lazy cell, a collection, a box or an Rc/Arc): reach it through an op of the crate (#[relax_api] fn), or mark it `#[relax_api(per_side)]`"))
    };
    if let Some(&(addr, size, align)) = p.shared.get(&key) {
        if !shared_class(prog, tcx, krate, t) {
            return class_2();
        }
        let l = u.layout_of(t);
        if size != u64::from(l.size) || align != u64::from(l.align) {
            return StaticUse::Refused(format!("static `{path}` of provided crate `{}`: the loader and this program disagree on the static's layout (program size {} align {}, loader size {size} align {align}): rebuild makepad from this tree", p.name, l.size, l.align));
        }
        return StaticUse::Shared(addr);
    }
    if p.per_side.contains(&key) || thread_local_storage(prog, tcx, d, t) {
        return StaticUse::OwnInstance;
    }
    let item = prog.files[def.file as usize].ast.item(def.item);
    if !matches!(item.kind, crate::ast::ItemKind::Static(true, ..))
        && tcx.is_freeze(prog, crate::tcx::TypingEnv::fully_monomorphized(prog), t)
        && trusted(prog, tcx, krate, t, 0)
    {
        return StaticUse::Copy;
    }
    if shared_class(prog, tcx, krate, t) {
        StaticUse::Refused(format!("static `{path}` of provided crate `{}` is mutable state the loader's code also uses: mark it `#[relax_api]` (one instance, the loader's) or `#[relax_api(per_side)]` (each side its own)", p.name))
    } else {
        class_2()
    }
}

/// Static `d` (of type `t`) is a thread-local's storage: a `#[thread_local]` static, or the
/// runtime key `thread_local!` keeps for a value with drop glue or a lazy initialiser (a
/// `std::thread::LazyKey` static). Each side has its own per-thread storage by construction
/// (rustc's lives in its TLS, laid out by its std; Relax's in the JIT's), so JIT code compiled
/// from a provided crate's source keeps its own: a thread-local is never shared across the
/// boundary, whatever its type (native_vec::LANES on an app's model thread, 10-30).
pub fn thread_local_storage(prog: &Program, tcx: &Tcx, d: DefId, t: TyId) -> bool {
    let def = prog.def(d);
    let ast = &prog.files[def.file as usize].ast;
    if crate::lang_items::has_attr(prog, def.file, ast.list(ast.item(def.item).attrs), "thread_local") {
        return true;
    }
    matches!(tcx.tys.kind(t), TyKind::Adt(a, _, _) if matches!(prog.def_path(*a).as_str(), "std::thread::local::LazyKey" | "std::thread::LazyKey"))
}

/// Diagnose plain fields with the same rules and depth as `trusted` on their ADT.
pub fn first_untrusted_field(prog: &Program, tcx: &Tcx, krate: u32, d: DefId) -> Option<(String, TyId)> {
    tcx.adts.get(&d)?.variants.iter().flat_map(|v| &v.fields)
        .find(|f| !trusted_in(prog, tcx, krate, f.ty, 1, true, false))
        .map(|f| (prog.syms.str(f.name).to_string(), f.ty))
}

/// Locate the nested part that refuses crossing for diagnostics only; `trusted_in` owns the rules.
pub fn untrusted_path(prog: &Program, tcx: &Tcx, krate: u32, t: TyId) -> String {
    let mut ty = t;
    let mut path = String::new();
    for _ in 0..8 {
        ty = match tcx.tys.kind(ty) {
            TyKind::Array(e, _) | TyKind::Slice(e) => {
                path.push_str("[_]");
                *e
            }
            TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) => *x,
            // std's own types are named, not opened (their fields are no API)
            TyKind::Adt(d, a, _) if a.is_empty() && tcx.adts.contains_key(d) && !is_std(prog, prog.def(*d).krate) => {
                let Some((name, field_ty)) = first_untrusted_field(prog, tcx, krate, *d) else { break };
                path.push('.');
                path.push_str(&name);
                field_ty
            }
            _ => break,
        };
    }
    if !path.is_empty() {
        format!("`{}: {}`", path, crate::typeck::ty_to_string(false, prog, tcx, ty))
    } else {
        String::new()
    }
}

/// `field`: `t` is a field of a provided type, where a pointer to a trait object may pass
/// through (rustc stores and hands it back, never calls through it: the vtable is Relax's; the
/// VM's `host: &mut dyn ScriptHost`). As a bound fn's own parameter it may not.
/// `cells`: shared static storage may contain cells and atomics of trusted values.
fn trusted_in(prog: &Program, tcx: &Tcx, krate: u32, t: TyId, depth: u32, field: bool, cells: bool) -> bool {
    trusted_options_in(prog, tcx, krate, t, depth, field, cells, None)
}

/// Cross-target declaration scans use the crate's typed entries for nested Options too;
/// the machine's layout words cannot establish another target's coverage.
fn trusted_options_in(prog: &Program, tcx: &Tcx, krate: u32, t: TyId, depth: u32, field: bool, cells: bool, covered: Option<&std::collections::HashSet<TyId>>) -> bool {
    if depth > 8 {
        return false;
    }
    let tr = |t: TyId| trusted_options_in(prog, tcx, krate, t, depth + 1, false, cells, covered);
    // a pointer to the crate's own values (rustc made them: the pointer is a handle) or to
    // trusted ones
    let own_pub = |x: TyId| matches!(tcx.tys.kind(x), TyKind::Adt(d, a, _) if a.is_empty() && prog.def(*d).krate == krate && prog.def(*d).is_pub);
    let pointee = |x: TyId| match tcx.tys.kind(x) {
        TyKind::Str => true,
        // the crate's own public types as elements: rustc's stride (TYPES / LAYOUTS), read only
        // through the crate's fns
        TyKind::Slice(e) => tr(*e) || own_pub(*e),
        // generic ones too: the pointer is an address, their fields stay unread (lower.rs); so
        // are another provided crate's (rustc-built into the same binary: the census's &mut ScriptVm)
        TyKind::Adt(d, _, _) => prog.def(*d).krate == krate || is_provided(prog, prog.def(*d).krate) || tr(x),
        TyKind::Dyn(..) => field,
        _ => tr(x),
    };
    match tcx.tys.kind(t) {
        TyKind::Bool | TyKind::Char | TyKind::Int(_) | TyKind::Float(_) | TyKind::Never => true,
        // Tuples share bytes only when every element does.
        TyKind::Tuple(v) => tcx.tys.list(*v).iter().all(|x| tr(*x)),
        TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) => pointee(*x),
        TyKind::FnPtr(_, _, header, _) => header.abi.is_c_like(),
        TyKind::Array(e, _) => tr(*e),
        TyKind::Adt(d, a, _) => {
            if cells {
                let args = tcx.tys.list(*a);
                match prog.def_path(*d).as_str() {
                    // Relax's AtomicU*/AtomicI* names alias Atomic<T> (std/core/sync/atomic.rs).
                    "core::cell::UnsafeCell" | "core::cell::SyncUnsafeCell" | "core::cell::Cell" | "core::sync::atomic::Atomic" => {
                        return args.len() == 1 && tr(args[0]);
                    }
                    "core::sync::atomic::AtomicBool" => return args.is_empty() && tr(tcx.tys.bool_),
                    "core::sync::atomic::AtomicPtr" => return args.len() == 1 && tr(tcx.tys.intern(TyKind::Ptr(true, args[0]))),
                    _ => {}
                }
            }
            // Box<T> (global allocator): both sides allocate with the system malloc, but a box
            // OWNS its value: JIT-compiled code that drops it runs T's drop glue. That glue is
            // rustc's when T is marked `#[relax_api(handle)]` (its entry carries rustc's
            // drop_in_place) or `#[relax_api(plain)]` (layout checked); a private T has no entry, so
            // Relax would lay it out and drop it by its own rules (String/Vec fields: a bogus
            // free). Such a box is no handle: the type holding it stays opaque (TYPES), dropped
            // and cloned by the crate.
            if tcx.adts.get(d).is_some_and(|a| a.is_box()) {
                // only a Box in the global allocator: rustc's side frees any other through an
                // allocator value Relax would have laid out
                let args = tcx.tys.list(*a);
                return args.len() <= 2
                    && args.get(1).is_none_or(|al| is_global_alloc(tcx, *al))
                    && args.first().is_some_and(|x| match tcx.tys.kind(*x) {
                        TyKind::Str => true,
                        TyKind::Slice(e) => tr(*e) || own_pub(*e),
                        TyKind::Dyn(..) => false,
                        TyKind::Adt(d2, _, _) if prog.def(*d2).krate == krate => own_pub(*x) || tr(*x),
                        _ => pointee(*x),
                    });
            }
            let std_crate = is_std(prog, prog.def(*d).krate);
            // PhantomData<T>: zero-sized, the same on both sides
            if tcx.adts.get(d).is_some_and(|a| a.is_phantom_data()) {
                return true;
            }
            if tcx.lang_items.get(LangItem::Option) == Some(*d) && tcx.tys.list(*a).len() == 1 {
                // Tagged Options need the const-verified six-word agreement, independently
                // of a slot's size/align. Niche verification is a separate step.
                let payload = tcx.tys.list(*a)[0];
                return tr(payload) && option_entry_covered(prog, tcx, t, payload, covered);
            }
            if !a.is_empty() {
                return false;
            }
            let adt = match tcx.adts.get(d) {
                Some(x) => x,
                None => return false,
            };
            // std's own types: fieldless enums only (Ordering), checked against LAYOUTS;
            // other crates' plain data (LiveId, the math vectors): as the crate's own
            if std_crate && !(adt.is_enum && adt.variants.iter().all(|v| v.fields.is_empty())) {
                return false;
            }
            if adt.is_union {
                return false;
            }
            let fields_ok = |v: &crate::tcx::VariantT| v.fields.iter().all(|f| trusted_options_in(prog, tcx, krate, f.ty, depth + 1, true, cells, covered));
            if adt.is_enum {
                if adt.variants.iter().all(|v| v.fields.is_empty()) {
                    return true;
                }
                // repr(C) / repr(int) data enums: laid out by RFC 2195
                if adt.repr_c || adt.repr_int.is_some() {
                    return adt.variants.iter().all(fields_ok);
                }
                // Option-like over a reference: the null niche (rustc's guaranteed layout)
                let opt_like = adt.variants.len() == 2
                    && adt.variants.iter().filter(|v| v.fields.is_empty()).count() == 1
                    && adt.variants.iter().any(|v| v.fields.len() == 1 && matches!(tcx.tys.kind(v.fields[0].ty), TyKind::Ref(..)) && fields_ok(v));
                return opt_like;
            }
            // structs, any repr: Relax lays them out as rustc does (layout.rs); each op's entry
            // carries rustc's size/align per slot and a mismatch refuses the binding
            let _ = is_transparent;
            adt.variants.iter().all(fields_ok)
        }
        _ => false,
    }
}

fn is_std(prog: &Program, krate: u32) -> bool {
    matches!(prog.syms.str(prog.crates[krate as usize].name), "core" | "alloc" | "std")
}

/// Class G: the language guarantees the null niche for this payload.
fn option_guaranteed(prog: &Program, tcx: &Tcx, payload: TyId) -> bool {
    match tcx.tys.kind(payload) {
        TyKind::Ref(..) | TyKind::FnPtr(..) => true,
        TyKind::Adt(d, a, _) => {
            let args = tcx.tys.list(*a);
            (tcx.adts.get(d).is_some_and(|a| a.is_box())
                && (1..=2).contains(&args.len()) && args.get(1).is_none_or(|al| is_global_alloc(tcx, *al)))
                || (is_std(prog, prog.def(*d).krate) && (prog.name(*d) == "NonNull" || prog.name(*d).starts_with("NonZero")))
        }
        _ => false,
    }
}

/// A direct tag at offset zero and all six words must agree; a niche entry states no tag.
fn option_layout_matches(l: &crate::layout::Layout, jit: &[u128; 6], entry: Option<&[u64; 6]>) -> bool {
    l.enc == crate::layout::TagEnc::Direct
        && matches!(l.tag, Some((0, relax_backend::rir::Mem::Int(w, _))) if w > 0 && u128::from(w) == jit[3])
        && entry.is_some_and(|e| e[3] > 0 && e.map(u128::from) == *jit)
}

/// Only the layout verdict is cached: payload trust still uses the caller's crate/depth rules.
fn option_layout_verdict(prog: &Program, tcx: &Tcx, t: TyId, payload: TyId) -> Result<(), String> {
    let cell = tcx.provided_option_layouts.get(&t).unwrap_or_else(|| {
        let _kept = crate::zone_alloc::kept();
        tcx.provided_option_layouts.insert_first(t, std::sync::Arc::new(std::sync::OnceLock::new()))
    });
    cell.get_or_init(|| {
        let _kept = crate::zone_alloc::kept();
        if option_guaranteed(prog, tcx, payload) { return Ok(()); }
        let probe = crate::layout::Layouts::new();
        let l = probe.of(&crate::layout::LayoutCx::new(prog), tcx, t);
        // Class N keeps its existing acceptance until the niche verification step.
        if matches!(l.enc, crate::layout::TagEnc::Niche { .. }) { return Ok(()); }
        let adt = &tcx.adts[&tcx.lang_items.get(LangItem::Option).unwrap()];
        let none = adt.variants.iter().position(|v| v.fields.is_empty()).unwrap();
        let some = adt.variants.iter().position(|v| v.fields.len() == 1).unwrap();
        let payload_off = l.variant_fields.get(some).and_then(|f| f.first()).copied().unwrap_or(0);
        let tag_bytes = match l.tag { Some((_, relax_backend::rir::Mem::Int(w, _))) => w, _ => 0 };
        let jit = [l.size as u128, l.align as u128, payload_off as u128, tag_bytes as u128,
            adt.tag_for_variant(tcx, prog, none as u32).unwrap().val,
            adt.tag_for_variant(tcx, prog, some as u32).unwrap().val];
        let name = crate::typeck::type_name(prog, tcx, t);
        let entry = OPTION_LAYOUTS.get().and_then(|m| m.get(&norm(&name)));
        if option_layout_matches(l, &jit, entry) { return Ok(()); }
        let exported = entry.map_or_else(|| "no entry".to_string(), |e| format!("{:?}", e));
        let payload = crate::typeck::type_name(prog, tcx, payload);
        Err(format!("Option layout mismatch for `{name}`: JIT {jit:?} (tag {:?}, encoding {:?}), rustc {exported}; words are [size, align, payload offset, tag width, None tag, Some tag]. List `{payload}` in a `relax_api::option_layouts!` in the module that spells it, or the crate's list when its names resolve there", l.tag, l.enc))
    }).clone()
}

/// Options listed by this crate on the checked target (the guard unions every crate's). Query declared types only: the
/// zero-sized marker's initializer and the host-only layout oracle need not run.
pub(crate) fn option_listed_types(prog: &Program, tcx: &Tcx, krate: u32) -> std::collections::HashSet<TyId> {
    let mut covered = std::collections::HashSet::new();
    for (i, _) in prog.defs.iter().enumerate().filter(|(_, d)| d.krate == krate && d.kind == DefKind::Const) {
        let Some(t) = tcx.const_ty(prog, DefId(i as u32)) else { continue };
        let TyKind::Adt(marker, args, _) = tcx.tys.kind(t) else { continue };
        if prog.def_path(*marker) != "relax_api::OptionListed" { continue; }
        let [option] = tcx.tys.list(*args) else { continue };
        if matches!(tcx.tys.kind(*option), TyKind::Adt(d, a, _) if tcx.lang_items.get(LangItem::Option) == Some(*d) && tcx.tys.list(*a).len() == 1)
            && !tcx.has_param(*option)
        {
            // Markers instantiate lifetimes as 'static; declarations may use bound ones.
            // Both keys are interned with regions erased, never compared by spelling.
            covered.insert(option_entry_type(prog, tcx, *option));
        }
    }
    covered
}

/// Resolve aliases and const arguments as layouts do, then intern one lifetime-insensitive
/// identity for both the marker's type and the declaration's type.
fn option_entry_type(prog: &Program, tcx: &Tcx, t: TyId) -> TyId {
    let t = tcx.tys.erase_all_regions(t);
    tcx.tys.erase_all_regions(tcx.normalize_erasing_regions(prog, crate::tcx::TypingEnv::fully_monomorphized(prog), t))
}

fn option_entry_covered(prog: &Program, tcx: &Tcx, t: TyId, payload: TyId, covered: Option<&std::collections::HashSet<TyId>>) -> bool {
    let Some(covered) = covered else { return option_layout_verdict(prog, tcx, t, payload).is_ok() };
    if option_guaranteed(prog, tcx, payload) { return true; }
    let layouts = crate::layout::Layouts::new();
    match layouts.of(&crate::layout::LayoutCx::new(prog), tcx, t).enc {
        crate::layout::TagEnc::Niche { .. } => true,
        crate::layout::TagEnc::Direct => covered.contains(&option_entry_type(prog, tcx, t)),
        _ => false,
    }
}

/// Every concrete tagged Option spelled by this crate's declarations, even when no body
/// reaches the item. One failure per type per crate, with a source occurrence.
/// Only signatures, fields and static types are queried; no function body is lowered.
pub(crate) fn option_entry_rows(prog: &Program, tcx: &Tcx, krate: u32, covered: Option<&std::collections::HashSet<TyId>>) -> Vec<String> {
    use crate::ast::{ItemKind, SelfParam, VariantData};
    let crate_name = prog.syms.str(prog.crates[krate as usize].name);
    let root = &prog.mods[prog.crates[krate as usize].root_mod as usize];
    let list_file = option_entry_path(&prog.file_paths[root.file as usize]);
    let mut rows = std::collections::BTreeMap::new();
    let layouts = crate::layout::Layouts::new();
    for (i, def) in prog.defs.iter().enumerate().filter(|(_, d)| d.krate == krate
        && matches!(d.kind, DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn | DefKind::Struct | DefKind::Enum | DefKind::Static | DefKind::ForeignStatic)) {
        let d = DefId(i as u32);
        let file = &prog.files[def.file as usize];
        let ast = &file.ast;
        let item = ast.item(def.item);
        let mut roots = Vec::new();
        match def.kind {
            DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn => {
                let Some(sig) = tcx.fn_sig(prog, d).filter(|s| s.n_generics == 0) else { continue };
                let ItemKind::Fn(s, _, _) = item.kind else { continue };
                let written = ast.fn_sig(s);
                let mut types = Vec::new();
                if let Some(receiver) = written.self_param.get() {
                    types.push(match receiver { SelfParam::Typed(_, t) => Some(t), _ => None });
                }
                types.extend(ast.list(written.params).iter().map(|p| Some(p.ty)));
                for (j, &t) in sig.params.iter().enumerate() {
                    roots.push((t, types.get(j).copied().flatten(), item.lo));
                }
                roots.push((sig.ret, written.ret.get(), item.lo));
            }
            DefKind::Struct | DefKind::Enum => {
                let Some(adt) = tcx.adts.get(&d).filter(|a| a.n_generics == 0) else { continue };
                let data: Vec<_> = match &item.kind {
                    ItemKind::Struct(_) => vec![ast.as_struct(&item.kind).unwrap().data],
                    ItemKind::Enum(_) => ast.list(ast.as_enum(&item.kind).unwrap().variants).iter().map(|v| v.data).collect(),
                    _ => continue,
                };
                for (vi, v) in adt.variants.iter().enumerate() {
                    let fields = match data.get(vi) {
                        Some(VariantData::Tuple(f) | VariantData::Struct(f)) => ast.list(*f),
                        _ => &[],
                    };
                    for (j, f) in v.fields.iter().enumerate() {
                        let source = fields.get(j);
                        roots.push((f.ty, source.map(|f| f.ty), source.map_or(item.lo, |f| f.ty_span.0)));
                    }
                }
            }
            DefKind::Static | DefKind::ForeignStatic => {
                let Some(t) = tcx.const_ty(prog, d) else { continue };
                let source = match item.kind { ItemKind::Static(_, t, _) => Some(t), _ => None };
                roots.push((t, source, item.lo));
            }
            _ => continue,
        }
        let cells = matches!(def.kind, DefKind::Static | DefKind::ForeignStatic);
        let mut seen = std::collections::HashSet::new();
        while let Some((t, source, pos)) = roots.pop() {
            if !seen.insert(t) { continue; }
            let source = source.map(|mut s| {
                while let crate::ast::Ty::Paren(inner) = ast.ty(s) { s = *inner; }
                s
            });
            let written = source.map(|s| ast.ty(s));
            let pos = match written { Some(crate::ast::Ty::Path(p)) => p.lo, _ => pos };
            let mut children = Vec::new();
            let mut spellings = Vec::new();
            match tcx.tys.kind(t) {
                TyKind::Adt(ad, args, _) => {
                    children.extend_from_slice(tcx.tys.list(*args));
                    let path = match written {
                        Some(crate::ast::Ty::Path(p)) if tcx.resolve_type_path_def(prog, def.file, def.scope, p) == Some(*ad) => Some(p),
                        _ => None,
                    };
                    if let Some(p) = path {
                        if let Some(seg) = ast.segs(p.segs).last() {
                            if let Some(crate::ast::GenericArgs::Angle(args)) = ast.gargs(seg.args()) {
                                for a in ast.list(*args) {
                                    match a {
                                        crate::ast::GenericArg::Type(t) => spellings.push(Some(*t)),
                                        crate::ast::GenericArg::Const(_) => spellings.push(None),
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    if tcx.lang_items.get(LangItem::Option) == Some(*ad) && children.len() == 1 {
                        let payload = children[0];
                        if !tcx.has_param(payload) && trusted_options_in(prog, tcx, krate, payload, 1, false, cells, covered)
                            && layouts.of(&crate::layout::LayoutCx::new(prog), tcx, t).enc == crate::layout::TagEnc::Direct
                            && covered.map_or_else(|| option_layout_verdict(prog, tcx, t, payload).is_err(),
                                |listed| !listed.contains(&option_entry_type(prog, tcx, t)))
                        {
                            // keyed by the type too: two types of one name (a private `AbsInfo` in two
                            // modules) each need their own entry
                            let name = norm(&crate::typeck::type_name(prog, tcx, t));
                            rows.entry((name, option_entry_type(prog, tcx, t))).or_insert_with(|| {
                                let payload = path.and_then(|p| option_payload_spelling(file, p))
                                    .unwrap_or_else(|| option_local_spelling(&crate::typeck::type_name(prog, tcx, payload), crate_name));
                                let loc = prog.source_location(def.file, pos);
                                let spelling_file = option_entry_path(loc.source.path());
                                format!("option-entry\tFAIL\t{}\tOption<{}>\t{}:{} ({})\tadd `{}` to a relax_api::option_layouts! in {} (or the crate's list in {} when its names resolve there)",
                                    crate_name, payload, spelling_file, loc.source.line_col(loc.span.lo).0,
                                    key(prog, tcx, d), payload, spelling_file, list_file)
                            });
                        }
                    }
                }
                TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Slice(x) | TyKind::Array(x, _) => {
                    children.push(*x);
                    spellings.push(match written {
                        Some(crate::ast::Ty::Ref(_, _, x) | crate::ast::Ty::Ptr(_, x) | crate::ast::Ty::Slice(x) | crate::ast::Ty::Array(x, _)) => Some(*x),
                        _ => None,
                    });
                }
                TyKind::Tuple(args) => {
                    children.extend_from_slice(tcx.tys.list(*args));
                    if let Some(crate::ast::Ty::Tuple(args)) = written { spellings.extend(ast.list(*args).iter().copied().map(Some)); }
                }
                TyKind::FnPtr(args, ret, _, _) => {
                    children.extend_from_slice(tcx.tys.list(*args));
                    children.push(*ret);
                    if let Some(crate::ast::Ty::Fn(f)) = written {
                        let f = ast.fn_ptr(*f);
                        spellings.extend(ast.list(f.params).iter().copied().map(Some));
                        spellings.push(f.ret.get());
                    }
                }
                _ => {}
            }
            roots.extend(children.into_iter().enumerate().map(|(j, t)| (t, spellings.get(j).copied().flatten(), pos)));
        }
    }
    rows.into_values().collect()
}

/// Keep diagnostics relative to the working tree when it contains the source.
fn option_entry_path(path: &str) -> String {
    let path = std::path::Path::new(path);
    std::env::current_dir().ok().and_then(|root| path.strip_prefix(root).ok().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The actual payload text of a resolved Option path. Aliases fall back to Relax's name.
fn option_payload_spelling(file: &crate::parser::ParsedFile, path: &crate::ast::Path) -> Option<String> {
    let seg = file.ast.segs(path.segs).last()?;
    let (toks, base) = file.part_at(seg.name.lo).map_or((&file.toks, 0), |p| (&p.toks, p.tok_base));
    let start = toks.iter().position(|t| t.lo == seg.name.lo)?;
    let open = base + start as u32 + 1;
    if file.tok_text(open) != "<" { return None; }
    let lo = file.tok(open).hi;
    // break_and_eat rewrites >>, >= and >>= in place, losing the first closing >'s token.
    // bytes also resolves expansion text beyond src, including lazy bodies' expansions.
    let bytes = file.bytes(lo, u32::MAX);
    let mut depth = 1;
    let mut delimiters = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => delimiters += 1,
            b')' | b']' | b'}' => delimiters -= 1,
            b'<' if delimiters == 0 => depth += 1,
            b'>' if delimiters == 0 && (i == 0 || bytes[i - 1] != b'-') => {
                depth -= 1;
                if depth == 0 {
                    let text = std::str::from_utf8(&bytes[..i]).ok()?;
                    let mut out = String::new();
                    let mut space = false;
                    let mut prev = None;
                    for ch in text.chars() {
                        if ch.is_whitespace() { space = true; continue; }
                        if space && prev.is_some() && !matches!(prev, Some('(' | '[' | '<'))
                            && !matches!(ch, ')' | ']' | '>' | ',' | ';')
                        {
                            out.push(' ');
                        }
                        out.push(ch);
                        prev = Some(ch);
                        space = false;
                    }
                    return Some(out);
                }
            }
            _ => {}
        }
    }
    None
}

/// Relax's own-crate paths must be nameable from that crate's source, even inside a tuple.
fn option_local_spelling(name: &str, krate: &str) -> String {
    let prefix = format!("{krate}::");
    let mut out = String::with_capacity(name.len());
    let mut copied = 0;
    for (at, _) in name.match_indices(&prefix) {
        if !name[..at].chars().next_back().is_none_or(|c| c.is_whitespace() || matches!(c, '(' | '[' | '<' | '&' | ',')) {
            continue;
        }
        out.push_str(&name[copied..at]);
        out.push_str("crate::");
        copied = at + prefix.len();
    }
    out.push_str(&name[copied..]);
    out
}

/// Find a refused Option within a rejected type for binding/lowering diagnostics. This does
/// not decide trust or open std's private fields; it reuses the cached layout verdict.
pub(crate) fn option_layout_reason(prog: &Program, tcx: &Tcx, t: TyId) -> Option<String> {
    fn walk(prog: &Program, tcx: &Tcx, t: TyId, depth: u32) -> Option<String> {
        if depth > 8 { return None; }
        let next = |x| walk(prog, tcx, x, depth + 1);
        match tcx.tys.kind(t) {
            TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Slice(x) | TyKind::Array(x, _) => next(*x),
            TyKind::Tuple(args) => tcx.tys.list(*args).iter().find_map(|x| next(*x)),
            TyKind::Adt(d, a, _) => {
                let args = tcx.tys.list(*a);
                if tcx.lang_items.get(LangItem::Option) == Some(*d) && args.len() == 1 {
                    let reason = tcx.provided_option_layouts.get(&t)
                        .and_then(|cell| cell.get().and_then(|verdict| verdict.as_ref().err().cloned()));
                    return reason.or_else(|| next(args[0]));
                }
                if is_std(prog, prog.def(*d).krate) {
                    return args.iter().find_map(|x| next(*x));
                }
                let adt = tcx.adts.get(d)?;
                for (vi, v) in adt.variants.iter().enumerate() {
                    for fi in 0..v.fields.len() {
                        let ft = tcx.field_ty(prog, crate::tcx::TypingEnv::fully_monomorphized(prog), *d, vi as u32, fi as u32, args);
                        if let Some(reason) = next(ft) { return Some(reason); }
                    }
                }
                None
            }
            _ => None,
        }
    }
    walk(prog, tcx, t, 0)
}

/// `alloc::alloc::Global` (lang `global_alloc_ty`): the allocator of a Box or Vec that crosses.
pub(crate) fn is_global_alloc(tcx: &Tcx, t: TyId) -> bool {
    matches!(tcx.tys.kind(t), TyKind::Adt(d, ..) if tcx.lang_items.get(LangItem::GlobalAlloc) == Some(*d))
}

/// The key an op is registered under, as `#[relax_api]` records it (relax-api's
/// key_code): a trait impl's method `<T as Trait>::m`, an inherent method `<T>::m` (T as
/// `type_name` prints it), a free fn its path -- [`norm`]ed.
pub fn key(prog: &Program, tcx: &Tcx, d: DefId) -> String {
    let def = prog.def(d);
    let name = prog.syms.str(def.name);
    if def.parent != crate::program::NO_DEF && prog.def(def.parent).kind == DefKind::Impl {
        if let Some(&st) = tcx.impl_self.get(&def.parent) {
            let ty = crate::typeck::type_name(prog, tcx, st);
            return norm(&match tcx.impl_trait.get(&def.parent) {
                Some(&tr) => {
                    let mut t = crate::typeck::item_path(prog, tcx, tr);
                    if let Some(a) = tcx.impl_trait_args.get(&def.parent).filter(|a| !a.is_empty()) {
                        t.push_str(&format!("<{}>", a.iter().map(|x| crate::typeck::type_name(prog, tcx, *x)).collect::<Vec<_>>().join(", ")));
                    }
                    format!("<{} as {}>::{}", ty, t, name)
                }
                None => format!("<{}>::{}", ty, name),
            });
        }
    }
    norm(&crate::typeck::item_path(prog, tcx, d))
}

/// A key or signature as both sides compare it: no lifetimes, no spaces, no empty `<>` (rustc
/// prints `ScriptVm<'_>`, Relax `ScriptVm`), std's items as `core::<name>` (rustc's
/// `alloc::string::String` is Relax's `core::string::String`, `core::ops::deref::Deref` its
/// `core::ops::Deref`; same-named std items stay apart by the slots' layouts).
pub fn norm(s: &str) -> String {
    let s = sig_norm(&s.replace(" as ", " @ "));
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == ':' && out.ends_with(':') {
            for k in ["alloc:", "std:"] {
                let at = out.len() - k.len();
                if out.ends_with(k) && !out[..at].ends_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                    out.truncate(at);
                    out.push_str("core:");
                    break;
                }
            }
        }
        match c {
            ',' if out.ends_with('<') || out.ends_with(',') => {}
            '>' if out.ends_with(',') => {
                out.pop();
                if out.ends_with('<') {
                    out.pop();
                } else {
                    out.push('>');
                }
            }
            '>' if out.ends_with('<') => {
                out.pop();
            }
            _ => out.push(c),
        }
    }
    // std's items by crate and name only: Relax's std puts them in other modules
    // (`core::ops::Deref`, rustc `core::ops::deref::Deref`)
    let b = out.as_bytes();
    let mut std_out = String::with_capacity(out.len());
    let mut i = 0;
    while i < b.len() {
        let boundary = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] == b':');
        if boundary && out[i..].starts_with("core::") {
            // the segments: keep the last one
            let mut j = i + 6;
            let mut last = j;
            loop {
                let mut k = j;
                while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'_') {
                    k += 1;
                }
                if k > j && out[k..].starts_with("::") && k + 2 < b.len() && (b[k + 2].is_ascii_alphabetic() || b[k + 2] == b'_') {
                    j = k + 2;
                    last = j;
                } else {
                    break;
                }
            }
            std_out.push_str("core::");
            i = last;
            continue;
        }
        std_out.push(b[i] as char);
        i += 1;
    }
    std_out.replace('@', " as ")
}

/// The host bridge key for Clone / PartialEq on an opaque type.
pub fn opaque_method_key(prog: &Program, d: DefId, method: &str) -> String {
    match method {
        "eq" => norm(&format!("<{} as PartialEq>::eq", prog.def_path(d))),
        _ => norm(&format!("<{} as Clone>::clone", prog.def_path(d))),
    }
}

/// The signature checked at bind time: `type_name` of the fn's fn pointer type as Relax reads the
/// source, lifetimes and spacing dropped (rustc's side: the same of `#[relax_api]`'s record).
pub fn sig(prog: &Program, tcx: &Tcx, d: DefId) -> String {
    let Some(s) = tcx.fn_sig(prog, d) else { return String::new() };
    let params: Vec<String> = s.params.iter().map(|p| crate::typeck::type_name(prog, tcx, *p)).collect();
    let unit = matches!(tcx.tys.kind(s.ret), TyKind::Tuple(v) if v.is_empty());
    let unsafe_ = if s.header.safety == crate::types::Safety::Unsafe { "unsafe " } else { "" };
    let text = if unit { format!("{unsafe_}fn({})", params.join(", ")) } else { format!("{unsafe_}fn({}) -> {}", params.join(", "), crate::typeck::type_name(prog, tcx, s.ret)) };
    norm(&text)
}

/// A signature without lifetimes or spaces (rustc prints `&'_ T` in fn pointer types).
pub fn sig_norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\'' {
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            continue;
        }
        if !b[i].is_ascii_whitespace() {
            out.push(b[i] as char);
        }
        i += 1;
    }
    out
}

/// The first parameter or result that refuses binding, shared with lowering diagnostics.
pub fn first_untrusted_signature_type(prog: &Program, tcx: &Tcx, krate: u32, d: DefId, opaque: &dyn Fn(DefId) -> bool) -> Option<TyId> {
    let s = tcx.fn_sig(prog, d)?;
    let crosses = |t: TyId| trusted(prog, tcx, krate, t, 0) || matches!(tcx.tys.kind(t), TyKind::Adt(d, a, _) if a.is_empty() && opaque(*d));
    s.params.iter().chain(std::iter::once(&s.ret)).copied().find(|t| !crosses(*t))
}

/// Why `d` (a fn of provided crate `krate`) is compiled from source instead of bound to a shim;
/// None: it is bound.
pub fn unbound_reason(prog: &Program, tcx: &Tcx, krate: u32, d: DefId, opaque: &dyn Fn(DefId) -> bool) -> Option<&'static str> {
    let def = prog.def(d);
    if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) {
        return Some("not a fn");
    }
    let s = tcx.fn_sig(prog, d)?;
    if s.n_generics > 0 {
        return Some("generic");
    }
    if s.header.abi.is_c_like() || s.header.c_variadic || s.track_caller {
        return Some("abi");
    }
    if crate::jit::inline_attrs(prog, d) & relax_codegen::opt::ATTR_ALWAYS != 0 {
        return Some("#[inline(always)]: compiled on the caller's side");
    }
    let parent = prog.def(def.parent);
    if parent.kind == DefKind::Trait {
        return Some("trait default body");
    }
    // Eq's derived assertion is a rustc-internal type-check helper, not a callable API.
    if parent.kind == DefKind::Impl
        && prog.syms.str(def.name) == "assert_fields_are_eq"
        && tcx.impl_trait.get(&def.parent).is_some_and(|t| prog.def_path(*t) == "core::cmp::Eq")
    {
        return Some("Eq assertion (compiler internal)");
    }
    // `Drop::drop` runs inside the type's drop shim (rustc refuses an explicit call, E0040)
    if parent.kind == DefKind::Impl && tcx.impl_trait.get(&def.parent).is_some_and(|t| tcx.lang_items.get(LangItem::Drop) == Some(*t)) {
        return Some("Drop::drop (in the drop shim)");
    }
    if let Some(t) = first_untrusted_signature_type(prog, tcx, krate, d, opaque) {
        if option_layout_reason(prog, tcx, t).is_some() {
            return Some("Option layout mismatch");
        }
        return Some("signature not C-shaped");
    }
    None
}

/// The types whose values cross between the provided crates' rustc code and JIT-compiled code:
/// reachable from the signatures of the bound fns (`bound`) through references, arrays, tuples,
/// type arguments and the fields of provided-crate types (and Option/Box/PhantomData). The walk
/// does not enter other std types: what a rustc-built Vec or String holds is reached only
/// through bound fns, whose signatures are roots themselves. Every other type of a provided
/// crate is made and laid out by JIT-compiled code alone (an inline shell's result, a type only
/// generic or unbound code builds): no opaque layout, no field guard.
/// The walk does not enter an opaque handle's fields (`opaque`: rustc lays it out, Relax does not):
/// JIT-compiled code never reaches them (a field read, a pattern or a value built of the handle
/// is refused, lower.rs check_provided_*), so a type that lives only inside such a handle (the
/// kernel compiler's `ir::Program` inside `KernelBody`) is not the crate's to share: JIT code that
/// makes its own values of it lays them out itself.
pub fn crossing_types(prog: &Program, tcx: &Tcx, krates: &[u32], bound: &dyn Fn(DefId) -> bool, opaque: &dyn Fn(DefId) -> bool) -> std::collections::HashSet<TyId> {
    let mut out = std::collections::HashSet::new();
    let mut stack: Vec<TyId> = Vec::new();
    for (i, def) in prog.defs.iter().enumerate() {
        if !krates.contains(&def.krate) || !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) {
            continue;
        }
        let d = DefId(i as u32);
        if !bound(d) {
            continue;
        }
        if let Some(sg) = tcx.fn_sig(prog, d) {
            stack.extend(sg.params.iter().copied());
            stack.push(sg.ret);
        }
    }
    while let Some(t) = stack.pop() {
        if out.len() > 200_000 || !out.insert(t) {
            continue;
        }
        match tcx.tys.kind(t) {
            TyKind::Adt(d, a, _) => {
                let k = prog.def(*d).krate;
                if krates.contains(&k) {
                    if opaque(*d) {
                        continue;
                    }
                    if let Some(adt) = tcx.adts.get(d) {
                        for (vi, v) in adt.variants.iter().enumerate() {
                            for fi in 0..v.fields.len() {
                                stack.push(tcx.field_ty(prog, crate::tcx::TypingEnv::fully_monomorphized(&prog), *d, vi as u32, fi as u32, tcx.tys.list(*a)));
                            }
                        }
                    }
                    stack.extend(tcx.tys.list(*a).iter().copied());
                } else if tcx.lang_items.get(LangItem::Option) == Some(*d) || tcx.adts.get(d).is_some_and(|a| a.is_box() || a.is_phantom_data()) {
                    stack.extend(tcx.tys.list(*a).iter().copied());
                }
            }
            TyKind::Ref(_, x, _) | TyKind::Ptr(_, x) | TyKind::Slice(x) | TyKind::Array(x, _) => stack.push(*x),
            TyKind::Tuple(v) => stack.extend(tcx.tys.list(*v).iter().copied()),
            _ => {}
        }
    }
    out
}



/// Where this binary's std keeps a `String`'s and a `Vec<T>`'s pointer, capacity and length (word
/// indices), and the capacity word of `None` of an `Option<String>` / `Option<Vec<T>>` (the
/// niche), read off live values. relax-compiler is rustc-built into the loader together with the
/// provided crates, so this is the layout of their `String` and `Vec`. None: not three words, or
/// `String` and `Vec<u64>` differ.
fn rustc_vec_words() -> Option<([usize; 3], usize)> {
    fn at(w: [usize; 3], ptr: usize, cap: usize, len: usize) -> Option<[usize; 3]> {
        let f = |x: usize| w.iter().position(|y| *y == x);
        let r = [f(ptr)?, f(cap)?, f(len)?];
        (r[0] != r[1] && r[1] != r[2] && r[0] != r[2]).then_some(r)
    }
    fn words<T>(x: &T) -> [usize; 3] {
        unsafe { *(x as *const T as *const [usize; 3]) }
    }
    let three = 3 * std::mem::size_of::<usize>();
    if std::mem::size_of::<String>() != three || std::mem::size_of::<Vec<u64>>() != three || std::mem::size_of::<Option<String>>() != three || std::mem::size_of::<Option<Vec<u64>>>() != three {
        return None;
    }
    let mut s = String::with_capacity(13);
    s.push_str("ab");
    let a = at(words(&s), s.as_ptr() as usize, s.capacity(), s.len())?;
    let mut v: Vec<u64> = Vec::with_capacity(11);
    v.extend([1, 2, 3]);
    let b = at(words(&v), v.as_ptr() as usize, v.capacity(), v.len())?;
    let (ns, nv): (Option<String>, Option<Vec<u64>>) = (std::hint::black_box(None), std::hint::black_box(None));
    let none = words(&ns)[a[1]];
    (a == b && words(&nv)[a[1]] == none).then_some((a, none))
}

/// What of makepad run's std is laid out as rustc's std lays it out (`rustc_vec_words`): the
/// same bytes on both sides, so a value built by either side's std is read, grown and freed by
/// the other's (both allocate through the loader's allocator; zone_alloc dealloc_layout frees a
/// block of any heap).
#[derive(Clone, Copy)]
pub struct StdSame {
    /// `Vec` (the ADT of `String`'s one field), with `String`
    vec: DefId,
    /// `Option<String>` / `Option<Vec<T>>`: rustc's niche (None in the capacity word)
    option: bool,
}

/// The String / Vec agreement of the two stds (`StdSame`), probed on a layout table of its own.
pub fn std_same(prog: &Program, tcx: &Tcx) -> Option<StdSame> {
    std_vec_same(prog, tcx, &crate::layout::Layouts::new())
}

fn std_vec_same(prog: &Program, tcx: &Tcx, probe: &crate::layout::Layouts) -> Option<StdSame> {
    let lcx = crate::layout::LayoutCx::new(prog);
    let sd = tcx.lang_items.get(LangItem::String)?;
    let sadt = tcx.adts.get(&sd)?;
    if sadt.variants.len() != 1 || sadt.variants[0].fields.len() != 1 {
        return None;
    }
    let st = tcx.tys.intern(TyKind::Adt(sd, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
    let vt = tcx.field_ty(prog, crate::tcx::TypingEnv::fully_monomorphized(prog), sd, 0, 0, &[]);
    let TyKind::Adt(vd, va, _) = tcx.tys.kind(vt) else { return None };
    let (vd, va) = (*vd, *va);
    let vadt = tcx.adts.get(&vd)?;
    // Vec<u8, Global>: the element and the global allocator (zero-sized; rustc's RawVec holds it
    // after the words, Relax's Vec after its three words too)
    let vargs = tcx.tys.list(va);
    if vargs.len() != 2 || !is_global_alloc(tcx, vargs[1]) || vadt.variants.len() != 1 {
        return None;
    }
    let pb = std::mem::size_of::<usize>() as u32;
    let (ls, lv) = (probe.of(&lcx, tcx, st), probe.of(&lcx, tcx, vt));
    if ls.size != 3 * pb || ls.fields.as_slice() != [0] || lv.size != 3 * pb {
        return None;
    }
    let mut ours = [usize::MAX; 3];
    for (i, f) in vadt.variants[0].fields.iter().enumerate() {
        let w = match prog.syms.str(f.name) {
            "ptr" => 0,
            "cap" => 1,
            "len" => 2,
            _ => continue,
        };
        let off = *lv.fields.get(i)?;
        if off % pb != 0 {
            return None;
        }
        ours[w] = (off / pb) as usize;
    }
    let (words, none) = rustc_vec_words()?;
    if ours != words {
        return None;
    }
    // Option<String>: None is rustc's value in the capacity word, Some's payload at 0
    let option = tcx.lang_items.get(LangItem::Option).is_some_and(|od| {
        let ot = tcx.tys.intern(TyKind::Adt(od, tcx.tys.mk_list(&[st]), crate::types::RegionArgsId::EMPTY));
        let lo = probe.of(&lcx, tcx, ot);
        let none_variant = tcx.adts.get(&od).and_then(|a| a.variants.iter().position(|v| v.fields.is_empty()));
        lo.size == 3 * pb
            && lo.tag.is_some_and(|(off, _)| off == words[1] as u32 * pb)
            && matches!(lo.enc, crate::layout::TagEnc::Niche { lo: l, hi: h, start, .. } if Some(l as usize) == none_variant && l == h && start as u64 == none as u64)
            && lo.variant_fields.iter().enumerate().all(|(vi, f)| Some(vi) == none_variant || f.as_slice() == [0])
    });
    Some(StdSame { vec: vd, option })
}

/// Parses Entry::enum_layout, rejecting bad widths, missing words and trailing data.
fn parse_enum_layout(table: &[u64]) -> Option<(u8, Vec<(u64, &[u64])>)> {
    let (&bytes, table) = table.split_first()?;
    let bytes = match bytes { 1 | 2 | 4 | 8 => bytes as u8, _ => return None };
    let (&count, mut table) = table.split_first()?;
    let count = usize::try_from(count).ok()?;
    if count > table.len() / 2 { return None; }
    let mut variants = Vec::with_capacity(count);
    for _ in 0..count {
        let (&tag, rest) = table.split_first()?;
        let (&fields, rest) = rest.split_first()?;
        let fields = usize::try_from(fields).ok()?;
        variants.push((tag, rest.get(..fields)?));
        table = &rest[fields..];
    }
    table.is_empty().then_some((bytes, variants))
}

/// Why the primitive-repr enum's tag or variant fields disagree with rustc's export.
/// Size and alignment are checked by the caller, for plain types and handles alike.
fn enum_layout_mismatch(prog: &Program, tcx: &Tcx, d: DefId, l: &crate::layout::Layout, table: &[u64]) -> Option<String> {
    let Some(adt) = tcx.adts.get(&d).filter(|a| a.is_enum) else { return Some("the JIT has no enum layout".into()) };
    let Some(repr) = adt.repr_int else { return Some("the enum has no primitive repr".into()) };
    if table.is_empty() { return Some("rustc's export has no enum layout".into()); }
    let Some((bytes, variants)) = parse_enum_layout(table) else { return Some("rustc's exported enum layout table is malformed".into()) };
    let (repr_bytes, signed) = tcx.tys.int_info(repr);
    if repr_bytes != bytes as u32
        || l.tag != Some((0, relax_backend::rir::Mem::Int(bytes, signed)))
        || l.enc != crate::layout::TagEnc::Direct
    {
        return Some("the tag width, position or encoding disagrees with rustc's export".into());
    }
    if variants.len() != adt.variants.len() || variants.len() != l.variant_fields.len() {
        return Some("the variant count disagrees with rustc's export".into());
    }
    // rustc's export sign-extends signed tags to u64; stores use the repr-width bits.
    let mask = u64::MAX >> (64 - bytes as u32 * 8);
    for (vi, (tag, offsets)) in variants.iter().enumerate() {
        let name = prog.syms.str(adt.variants[vi].name);
        if !adt.tag_for_variant(tcx, prog, vi as u32).is_some_and(|t| (t.val as u64 & mask) == (*tag & mask)) {
            return Some(format!("variant `{}`'s tag disagrees with rustc's export", name));
        }
        if offsets.len() != adt.variants[vi].fields.len()
            || offsets.len() != l.variant_fields[vi].len()
            || !offsets.iter().zip(&l.variant_fields[vi]).all(|(a, b)| *a == *b as u64)
        {
            return Some(format!("variant `{}`'s field offsets disagree with rustc's export", name));
        }
    }
    None
}

/// The reason a provided type's layout disagrees with its export, shared by plain types and
/// handle candidates. Empty tables on plain types retain their size/align-only check.
pub(crate) fn layout_mismatch(prog: &Program, tcx: &Tcx, d: DefId, l: &crate::layout::Layout, size: u32, align: u32, offs: &[u64], enum_layout: &[u64]) -> Option<String> {
    if l.size != size || l.align != align {
        return Some(format!("size/align disagree with rustc's export (JIT {}/{}, rustc {}/{})", l.size, l.align, size, align));
    }
    if !offs.is_empty() && (offs.len() != l.fields.len() || !offs.iter().zip(&l.fields).all(|(a, b)| *a == *b as u64)) {
        return Some("field offsets disagree with rustc's export".into());
    }
    if !enum_layout.is_empty() { return enum_layout_mismatch(prog, tcx, d, l, enum_layout); }
    None
}

/// The `#[relax_api(handle)]` structs and primitive-repr enums among `cands` (def, rustc's size,
/// align, struct field offsets, enum layout table) whose layout is the same on both sides:
/// Relax agrees with rustc's size, align, field offsets and enum tags, and every field of every
/// variant is the same bytes (`trusted` data, a `String` / `Vec<T>` of
/// such data when the two stds agree (std_vec_same), or another such handle). JIT-compiled code
/// may build these and read their fields; their drop and the trait ops the attribute lists stay
/// rustc's. `opaque`: the types Relax uses through the crate's fns only (the candidates among
/// them until they pass, a demoted plain type); `known`: handles found the same before (a crate
/// an app domain added after the boot).
pub fn same_layout_handles(prog: &Program, tcx: &Tcx, cands: &[(DefId, u32, u32, &[u64], &[u64])], opaque: &dyn Fn(DefId) -> bool, known: &dyn Fn(DefId) -> bool) -> Vec<DefId> {
    same_layout_handles_with_reasons(prog, tcx, cands, opaque, known).0
}

/// The same agreement gate, retaining the first rejection of each candidate at binding so a
/// later refusal can explain it without rerunning the layout or field-byte fixed point.
pub(crate) fn same_layout_handles_with_reasons(prog: &Program, tcx: &Tcx, cands: &[(DefId, u32, u32, &[u64], &[u64])], opaque: &dyn Fn(DefId) -> bool, known: &dyn Fn(DefId) -> bool) -> (Vec<DefId>, HashMap<DefId, String>) {
    let mut rejected = HashMap::new();
    if cands.is_empty() {
        return (Vec::new(), rejected);
    }
    let probe = crate::layout::Layouts::new();
    let lcx = crate::layout::LayoutCx::new(prog);
    let std_same = std_vec_same(prog, tcx, &probe);
    let mut ok: std::collections::HashSet<DefId> = cands
        .iter()
        .filter(|(d, size, align, offs, enum_layout)| {
            let adt = match tcx.adts.get(d) {
                Some(a) if !a.is_union && (a.is_enum || a.variants.len() == 1) => a,
                _ => {
                    rejected.insert(*d, "the JIT has no supported struct or enum layout".into());
                    return false;
                }
            };
            let t = tcx.tys.intern(TyKind::Adt(*d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY));
            let l = probe.of(&lcx, tcx, t);
            let reason = layout_mismatch(prog, tcx, *d, l, *size, *align, if adt.is_enum { &[] } else { offs }, &[])
                .or_else(|| {
                    if adt.is_enum { return enum_layout_mismatch(prog, tcx, *d, l, enum_layout); }
                    if offs.is_empty() { return Some("rustc's export has no struct field offsets".into()); }
                    (offs.len() != adt.variants[0].fields.len()).then(|| "field offsets disagree with rustc's export".into())
                });
            if let Some(reason) = reason {
                rejected.insert(*d, reason);
                return false;
            }
            true
        })
        .map(|c| c.0)
        .collect();
    // a field of a candidate that turns out not to be the same bytes takes its holders with it
    loop {
        let now = ok.clone();
        let same = |krate: u32, t: TyId| same_bytes(prog, tcx, krate, t, &|d| now.contains(&d) || known(d), std_same, opaque, 0);
        ok.retain(|d| {
            let krate = prog.def(*d).krate;
            let adt = tcx.adts.get(d).unwrap(); // admitted above
            for (vi, v) in adt.variants.iter().enumerate() {
                for (fi, f) in v.fields.iter().enumerate() {
                    let ft = tcx.field_ty(prog, crate::tcx::TypingEnv::fully_monomorphized(prog), *d, vi as u32, fi as u32, &[]);
                    if !same(krate, ft) {
                        let variant = if adt.is_enum { format!("variant `{}` ", prog.syms.str(v.name)) } else { String::new() };
                        let field = if v.shape == 1 { fi.to_string() } else { prog.syms.str(f.name).to_string() };
                        let ty = crate::typeck::ty_to_string(false, prog, tcx, ft);
                        let detail = option_layout_reason(prog, tcx, ft).map_or(String::new(), |reason| format!(": {reason}"));
                        rejected.insert(*d, format!("{}field `{}` of type `{}` is not the same bytes on both sides{}", variant, field, ty, detail));
                        return false;
                    }
                }
            }
            true
        });
        if ok.len() == now.len() {
            break;
        }
    }
    let mut v: Vec<DefId> = ok.into_iter().collect();
    v.sort_by_key(|d| d.0);
    (v, rejected)
}

/// Is field `t` of an opaque handle of provided crate `krate` the same bytes on both sides, so
/// JIT-compiled code reaches it at rustc's offset (Unit::provided_handle_field)? The rule of
/// `same_layout_handles` for one field: `trusted` data, a handle laid out as rustc does (`laid`),
/// or a `String` / `Vec<T>` of such data (and `Option` of one) when the two stds agree
/// (`std_same`): ScriptTokenizer's `tokens: Vec<ScriptTokenPos>` in a handle that its `State` keeps
/// opaque. A HashMap (rustc's std's own table) or an opaque handle's elements stay rustc's alone.
pub fn handle_field_same(prog: &Program, tcx: &Tcx, krate: u32, t: TyId, std_same: Option<StdSame>, laid: &dyn Fn(DefId) -> bool, opaque: &dyn Fn(DefId) -> bool) -> bool {
    same_bytes(prog, tcx, krate, t, laid, std_same, opaque, 0)
}

/// Is a field of type `t` (in a handle of provided crate `krate`) the same bytes on both sides
/// (same_layout_handles)?
fn same_bytes(prog: &Program, tcx: &Tcx, krate: u32, t: TyId, ok: &dyn Fn(DefId) -> bool, std_same: Option<StdSame>, opaque: &dyn Fn(DefId) -> bool, depth: u32) -> bool {
    if depth > 8 {
        return false;
    }
    if let TyKind::Adt(d, a, _) = tcx.tys.kind(t) {
        if ok(*d) {
            return true;
        }
        if opaque(*d) {
            return false;
        }
        if let Some(std) = std_same {
            let args = tcx.tys.list(*a);
            let same = |x: TyId| same_bytes(prog, tcx, krate, x, ok, std_same, opaque, depth + 1);
            // String, Vec<T> of such T in the global allocator (another allocator is laid out
            // by each side's own rules: rustc keeps it in its RawVec)
            let vec_like = |x: TyId| match tcx.tys.kind(x) {
                TyKind::Adt(xd, xa, _) => tcx.lang_items.get(LangItem::String) == Some(*xd) || (*xd == std.vec && match tcx.tys.list(*xa) {
                    [e, al] => same(*e) && is_global_alloc(tcx, *al),
                    _ => false,
                }),
                _ => false,
            };
            if vec_like(t) {
                return true;
            }
            if std.option && tcx.lang_items.get(LangItem::Option) == Some(*d) && args.len() == 1 && vec_like(args[0]) {
                return true;
            }
        }
    }
    trusted_in(prog, tcx, krate, t, 0, true, false)
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn option_layout_checks_every_tag_word() {
        use crate::layout::{Layout, TagEnc};
        use relax_backend::rir::Mem;
        let l = Layout {
            size: 32, align: 16, fields: vec![], tag: Some((0, Mem::Int(16, false))),
            enc: TagEnc::Direct, variant_fields: vec![vec![], vec![16]], leaves: None,
            niche: None, uninhabited: false,
        };
        let table = [32, 16, 16, 16, 0, 1];
        let jit = table.map(u128::from);
        assert!(option_layout_matches(&l, &jit, Some(&table)), "matching Option layout must be accepted");
        for (word, property) in [(3, "tag width"), (2, "payload offset"), (4, "None tag"), (5, "Some tag")] {
            let mut wrong = table;
            wrong[word] += 1;
            assert!(!option_layout_matches(&l, &jit, Some(&wrong)), "doctored {property} must be refused");
        }
        assert!(!option_layout_matches(&l, &jit, None), "a tagged Option with no entry must be refused");
    }

    #[test]
    fn same_layout_handles_checks_enum_agreement() {
        isolated_test!(same_layout_handles_checks_enum_agreement);
        std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
        struct Temp(std::path::PathBuf);
        impl Drop for Temp { fn drop(&mut self) { std::fs::remove_dir_all(&self.0).unwrap(); } }
        let dir = Temp(std::env::temp_dir().join(format!("relax-enum-agreement-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
        std::fs::create_dir_all(&dir.0).unwrap();
        let path = dir.0.join("lib.rs");
        std::fs::write(&path, r#"
#[repr(u8)]
pub enum Data { Empty = 3, Byte(u8), Named { byte: u8, wide: u64 } = 16, Last }
pub enum RustData { Empty, Value(u8, u64) }
#[repr(u8)]
pub enum Recursive { Empty, Values(Vec<S>) }
pub struct S { pub value: Recursive }
#[repr(C, u8)]
pub enum CData { Empty, Byte(u8), Wide(u64) }
#[repr(i8)]
pub enum Signed { Negative(u8) = -3, Empty }
"#).unwrap();
        let (prog, _, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "enum_agreement", false).unwrap();
        drop(dir);
        let mut tcx = Tcx::new();
        tcx.collect(&prog);
        assert!(tcx.errors.is_empty(), "enum agreement source must collect: {:?}", tcx.errors);
        let def = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
            let id = DefId(i as u32);
            (d.krate == krate && prog.name(id) == name).then_some(id)
        }).unwrap();
        let probe = crate::layout::Layouts::new();
        let lcx = crate::layout::LayoutCx::new(&prog);
        let layout = |d| probe.of(&lcx, &tcx, tcx.tys.intern(TyKind::Adt(d, crate::types::List::EMPTY, crate::types::RegionArgsId::EMPTY)));
        // This is the export rustc supplies when its layout agrees; mutations below isolate
        // each check without hard-coding the target's sizes or payload alignment.
        let export = |d| {
            let l = layout(d);
            let Some((_, relax_backend::rir::Mem::Int(bytes, _))) = l.tag else { panic!("fixture enum must have an integer tag") };
            let adt = tcx.adts.get(&d).unwrap();
            let mut table = vec![bytes as u64, adt.variants.len() as u64];
            for (vi, offsets) in l.variant_fields.iter().enumerate() {
                table.push(adt.tag_for_variant(&tcx, &prog, vi as u32).unwrap().signed_value() as u64);
                table.push(offsets.len() as u64);
                table.extend(offsets.iter().map(|off| *off as u64));
            }
            table
        };
        let check = |cands: &[(DefId, u32, u32, &[u64], &[u64])]| {
            same_layout_handles_with_reasons(&prog, &tcx, cands, &|d| cands.iter().any(|c| c.0 == d), &|_| false)
        };
        let check_enum = |d, table: &[u64]| {
            let l = layout(d);
            check(&[(d, l.size, l.align, &[], table)])
        };
        let data = def("Data");
        let table = export(data);
        assert_eq!(check_enum(data, &table).0, [data], "a repr(u8) data enum with matching layout must pass");
        let mut wrong = table.clone();
        wrong[6] += 1; // Byte's sole field, after Empty's tag/count and Byte's tag/count.
        let (same, rejected) = check_enum(data, &wrong);
        assert!(same.is_empty(), "a wrong field offset must keep the enum opaque");
        assert!(rejected[&data].contains("variant `Byte`'s field offsets"), "wrong offsets must name the variant: {:?}", rejected);
        wrong = table.clone();
        wrong[2] += 1;
        let (same, rejected) = check_enum(data, &wrong);
        assert!(same.is_empty(), "a wrong tag value must keep the enum opaque");
        assert!(rejected[&data].contains("variant `Empty`'s tag"), "wrong tags must name the variant: {:?}", rejected);
        wrong = table.clone();
        wrong[0] = 2;
        assert!(check_enum(data, &wrong).0.is_empty(), "a wrong tag width must keep the enum opaque");
        for (bad, why) in [
            (&table[..1], "missing variant count"),
            (&table[..table.len() - 1], "truncated variant"),
            (&[3, 0][..], "invalid tag width"),
            (&[1, u64::MAX][..], "oversized variant count"),
            (&[1, 1, 0, u64::MAX][..], "oversized field count"),
        ] {
            assert!(parse_enum_layout(bad).is_none(), "malformed table must fail parsing: {why}");
            assert!(check_enum(data, bad).0.is_empty(), "malformed table must keep the enum opaque: {why}");
        }
        wrong = table.clone();
        wrong.push(0);
        assert!(parse_enum_layout(&wrong).is_none(), "trailing data must fail parsing");
        assert!(check_enum(data, &wrong).0.is_empty(), "trailing data must keep the enum opaque");
        let rust = def("RustData");
        assert!(check_enum(rust, &export(rust)).0.is_empty(), "repr(Rust) must stay opaque even with its own layout table");
        for name in ["CData", "Signed"] {
            let d = def(name);
            assert_eq!(check_enum(d, &export(d)).0, [d], "primitive repr must accept matching C payload alignment and signed tags: {name}");
        }

        let (recursive, s) = (def("Recursive"), def("S"));
        let (el, sl) = (layout(recursive), layout(s));
        let et = export(recursive);
        let mut offsets: Vec<u64> = sl.fields.iter().map(|off| *off as u64).collect();
        let mut expected = vec![recursive, s];
        expected.sort_by_key(|d| d.0);
        assert_eq!(check(&[(recursive, el.size, el.align, &[], &et), (s, sl.size, sl.align, &offsets, &[])]).0, expected,
            "an enum holding Vec<S> and S holding that enum must pass together");
        offsets[0] += 1;
        let (same, rejected) = check(&[(recursive, el.size, el.align, &[], &et), (s, sl.size, sl.align, &offsets, &[])]);
        assert!(same.is_empty(),
            "a wrong S offset must reject both S and the enum whose later variant holds Vec<S>");
        assert!(rejected[&recursive].contains("variant `Values` field `0` of type `Vec<S, Global>` is not the same bytes"), "field-byte failure must name the variant, field and type: {:?}", rejected);
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&MATCHED, Mutex::new(Vec::new()));
    reset(&PENDING, Mutex::new(None));
    reset(&VERDICT, Mutex::new(None));
}
