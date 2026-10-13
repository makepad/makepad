//! Crate graph (from cargo's unit graph for now) and source file discovery by
//! following `mod` declarations from each crate root.

use crate::ast::*;
use crate::json;
use crate::lexer::T;
use crate::parser::ParsedFile;
use relax_session::sync::{Condvar, Mutex};

pub struct CrateUnit {
    pub name: String,
    pub src_path: String,
    pub edition: u16,
    pub features: Vec<String>,
    /// (unit index, extern crate name)
    pub deps: Vec<(usize, String)>,
    pub proc_macro: bool,
    pub build_script: bool,
    /// package version from cargo's pkg_id ("1.15.1")
    pub version: String,
}

/// Crates Relax provides itself (std/substitutes.txt: `<name> <version prefix> <root>`):
/// a unit of that crate at a matching version loads the provided root instead of its sources
/// (same name; deps become core/alloc/std only; features ignored). The rustc build keeps the
/// real crates. Also hands the graph to the proc-macro host (macro_reg::register_and_strip);
/// `additive`: the graph of a program started into a running one joins the registered graph.
pub fn apply_substitutes(sess: &std::sync::Arc<crate::Session>, units: &mut [CrateUnit], std_dir: &str, additive: bool) -> Result<(), String> {
    let table = match crate::vfs::read_to_string(format!("{}/substitutes.txt", std_dir)) {
        Ok(t) => t,
        Err(_) => {
            crate::macro_reg::register_and_strip(sess, units, std_dir, additive)?;
            return Ok(());
        }
    };
    for line in table.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 3 {
            continue;
        }
        let (name, ver, root) = (parts[0], parts[1], parts[2]);
        for u in units.iter_mut() {
            if u.name != name || u.proc_macro {
                continue;
            }
            let matches = u.version == ver || u.version.starts_with(&format!("{}.", ver));
            if !matches {
                return Err(format!("{}: crate `{}` is version {}, std/substitutes.txt provides {} -- update the table or the provided crate", u.src_path, name, u.version, ver));
            }
            u.src_path = format!("{}/{}", std_dir, root);
            u.edition = 2021;
            u.features.clear();
            u.deps.clear();
        }
    }
    // proc-macro crates run in the JIT proc-macro host; the app graph sees them empty
    crate::macro_reg::register_and_strip(sess, units, std_dir, additive)?;
    Ok(())
}

/// Unit indices with every crate after its dependencies.
pub fn topo_order(units: &[CrateUnit]) -> Vec<usize> {
    let mut out = Vec::with_capacity(units.len());
    let mut state = vec![0u8; units.len()]; // 0 new, 1 visiting, 2 done
    fn visit(i: usize, units: &[CrateUnit], state: &mut Vec<u8>, out: &mut Vec<usize>) {
        if state[i] != 0 {
            return;
        }
        state[i] = 1;
        for (d, _) in &units[i].deps {
            if *d < units.len() {
                visit(*d, units, state, out);
            }
        }
        state[i] = 2;
        out.push(i);
    }
    for i in 0..units.len() {
        visit(i, units, &mut state, &mut out);
    }
    out
}

pub struct SrcFile {
    pub crate_idx: u32,
    pub path: String,
    pub edition: u16,
    pub src: Vec<u8>,
    /// children resolve relative to the file's own dir (crate root, mod.rs, #[path] file)
    pub mod_rs: bool,
    /// reached through `mod` declarations whose cfg holds on x86_64 Linux
    pub active: bool,
}

/// Loads every lib/proc-macro unit of cargo's `--unit-graph` output, deduplicated by
/// source path (host and target copies of a crate parse the same files).
pub fn load_units(path: &str) -> Result<Vec<CrateUnit>, String> {
    let data = crate::vfs::read(path).map_err(|e| format!("{}: {}", path, e))?;
    let j = json::parse(&data)?;
    let units = match j.get("units") {
        Some(u) => u.arr(),
        None => return Err("no units".to_string()),
    };
    let mut out: Vec<CrateUnit> = Vec::new();
    let mut map: Vec<usize> = Vec::new(); // unit index -> out index or usize::MAX
    let mut build_pkgs: Vec<String> = Vec::new();
    for u in units {
        let t = match u.get("target") {
            Some(t) => t,
            None => continue,
        };
        let mut kind_lib = false;
        let mut kind_pm = false;
        let mut kind_build = false;
        if let Some(k) = t.get("kind") {
            for s in k.arr() {
                match s.str() {
                    "lib" | "rlib" => kind_lib = true,
                    "proc-macro" => kind_pm = true,
                    "custom-build" => kind_build = true,
                    _ => {}
                }
            }
        }
        if kind_build {
            if let Some(p) = u.get("pkg_id") {
                build_pkgs.push(p.str().to_string());
            }
        }
        let src_path = match t.get("src_path") {
            Some(s) => s.str().to_string(),
            None => String::new(),
        };
        if !(kind_lib || kind_pm) {
            map.push(usize::MAX);
            continue;
        }
        let mut found = usize::MAX;
        for i in 0..out.len() {
            if crate::vfs::normalize_path(&out[i].src_path) == crate::vfs::normalize_path(&src_path) {
                found = i;
                break;
            }
        }
        if found != usize::MAX {
            map.push(found);
            continue;
        }
        let edition = match t.get("edition") {
            Some(e) => e.str().parse::<u16>().unwrap_or(2015),
            None => 2015,
        };
        let mut features = Vec::new();
        if let Some(f) = u.get("features") {
            for s in f.arr() {
                features.push(s.str().to_string());
            }
        }
        let mut deps = Vec::new();
        if let Some(d) = u.get("dependencies") {
            for dep in d.arr() {
                let idx = match dep.get("index") {
                    Some(n) => n.num() as usize,
                    None => continue,
                };
                let name = match dep.get("extern_crate_name") {
                    Some(n) => n.str().to_string(),
                    None => String::new(),
                };
                deps.push((idx, name));
            }
        }
        let name = match t.get("name") {
            Some(n) => n.str().to_string(),
            None => String::new(),
        };
        let pkg = match u.get("pkg_id") {
            Some(p) => p.str().to_string(),
            None => String::new(),
        };
        let mut build_script = false;
        for b in &build_pkgs {
            if *b == pkg {
                build_script = true;
            }
        }
        // "...#smallvec@1.15.1" / "...#1.15.1"
        let version = match pkg.rfind('#') {
            Some(i) => {
                let v = &pkg[i + 1..];
                match v.rfind('@') {
                    Some(j) => v[j + 1..].to_string(),
                    None => v.to_string(),
                }
            }
            None => String::new(),
        };
        map.push(out.len());
        out.push(CrateUnit { name, src_path, edition, features, deps,  proc_macro: kind_pm, build_script, version });
    }
    // remap dependency indices to deduplicated crate indices; drop build-script deps
    for c in out.iter_mut() {
        let mut nd = Vec::new();
        for (i, n) in &c.deps {
            if *i < map.len() && map[*i] != usize::MAX {
                nd.push((map[*i], n.clone()));
            }
        }
        c.deps = nd;
    }

    Ok(out)
}

fn dir_of(path: &str) -> &str {
    // std::path::is_separator: '/' (unix), '/' or '\\' (Windows)
    match path.rfind(std::path::is_separator) {
        Some(i) => &path[..i],
        None => ".",
    }
}

fn stem_of(path: &str) -> &str {
    let f = match path.rfind(std::path::is_separator) {
        Some(i) => &path[i + 1..],
        None => path,
    };
    match f.rfind('.') {
        Some(i) => &f[..i],
        None => f,
    }
}

/// Text of a string literal token without quotes (paths never use escapes).
fn str_lit(src: &[u8], lo: u32, hi: u32) -> String {
    let s = &src[lo as usize..hi as usize];
    let mut a = 0;
    let mut b = s.len();
    while a < b && s[a] != b'"' {
        a += 1;
    }
    while b > a && s[b - 1] != b'"' {
        b -= 1;
    }
    if b > a + 1 {
        String::from_utf8_lossy(&s[a + 1..b - 1]).into_owned()
    } else {
        String::new()
    }
}

/// `#[path = "x"]` and `#[cfg_attr(.., path = "x")]` values of an attribute list.
fn path_attrs(p: &ParsedFile, attrs: &[Attr], cfg: &crate::cfg::CfgSet, out: &mut Vec<String>) {
    path_attrs_cfg(p.src_view(), attrs, cfg, out)
}

/// The `#[path = ".."]` of an item after cfg_attr expansion for `cfg` (libc picks its per-arch
/// module file with `#[cfg_attr(target_arch = "..", path = "..")]`; only the active one counts).
pub fn path_attrs_cfg(p: crate::cfg::Src, attrs: &[Attr], cfg: &crate::cfg::CfgSet, out: &mut Vec<String>) {
    let expanded = crate::cfg::expand_cfg_attr_list(p, attrs, cfg);
    for a in &expanded {
        let lo = a.toks.lo as usize;
        let hi = a.toks.hi as usize;
        if hi >= lo + 3 && p.tok(lo).kind == T::Ident && crate::cfg::tok_str(p, lo) == b"path" && p.tok(lo + 1).kind == T::Eq && p.tok(lo + 2).kind == T::Str {
            let t = p.tok(lo + 2);
            out.push(str_lit(p.src, t.lo, t.hi));
        }
    }
}

pub fn path_attrs_src(p: crate::cfg::Src, attrs: &[Attr], out: &mut Vec<String>) {
    for a in attrs {
        let lo = a.toks.lo as usize;
        let hi = a.toks.hi as usize;
        let mut i = lo;
        while i + 2 < hi {
            let t = p.tok(i);
            if t.kind == T::Ident && &p.src[t.lo as usize..t.hi as usize] == b"path" {
                if p.tok(i + 1).kind == T::Eq && p.tok(i + 2).kind == T::Str {
                    let s = p.tok(i + 2);
                    out.push(str_lit(p.src, s.lo, s.hi));
                }
            }
            i += 1;
        }
    }
}

/// Pending module file.
struct Job {
    crate_idx: u32,
    path: String,
    edition: u16,
    mod_rs: bool,
    active: bool,
}

/// Finds all module files of the given crates by parsing roots and following
/// `mod x;`. Returns the files (with their sources) and the paths that were
/// declared but not found.
pub fn discover(sess: &std::sync::Arc<crate::Session>, crates: &[CrateUnit], cfgs: &[crate::cfg::CfgSet], threads: usize) -> (Vec<SrcFile>, Vec<String>, Vec<String>) {
    struct Shared {
        queue: Vec<Job>,
        active: usize,
        files: Vec<SrcFile>,
        missing: Vec<String>,
        errors: Vec<String>,
        seen: Vec<String>,
    }
    let sources = std::sync::Arc::new(crate::source::SourceRegistry::default());
    let contexts = crate::source_context::Context::for_units(crates, sources, sess);
    let state = Mutex::new(Shared {
        queue: Vec::new(),
        active: 0,
        files: Vec::new(),
        missing: Vec::new(),
        errors: Vec::new(),
        seen: Vec::new(),
    });
    let cv = Condvar::new();
    {
        let mut s = state.lock().unwrap();
        for i in 0..crates.len() {
            s.seen.push(crates[i].src_path.clone());
            s.queue.push(Job { crate_idx: i as u32, path: crates[i].src_path.clone(), edition: crates[i].edition, mod_rs: true, active: true });
        }
    }
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| { let _z = crate::zone_alloc::compiler_thread(); loop {
                let job = {
                    let mut s = state.lock().unwrap();
                    loop {
                        if let Some(j) = s.queue.pop() {
                            s.active += 1;
                            break Some(j);
                        }
                        if s.active == 0 {
                            break None;
                        }
                        s = cv.wait(s).unwrap();
                    }
                };
                let job = match job {
                    Some(j) => j,
                    None => {
                        cv.notify_all();
                        return;
                    }
                };
                let mut found: Vec<(Vec<String>, bool, bool)> = Vec::new();
                let mut err = None;
                let src = crate::vfs::read(&job.path).unwrap_or_default();
                let cfg = &cfgs[job.crate_idx as usize];
                let context = std::rc::Rc::new(crate::builtins::FileCtx::new(&crates[job.crate_idx as usize].name, &job.path, &crates[job.crate_idx as usize].name, cfg));
                match contexts[job.crate_idx as usize].parse(src.clone(), None, job.edition, None, context, false) {
                    Ok((p, _)) => {
                        let child_dir = if job.mod_rs {
                            dir_of(&job.path).to_string()
                        } else {
                            format!("{}/{}", dir_of(&job.path), stem_of(&job.path))
                        };
                        let base_dir = dir_of(&job.path).to_string();
                        let cfg = &cfgs[job.crate_idx as usize];
                        let act = job.active && crate::cfg::active(p.src_view(), &p.ast.root_attrs, cfg);
                        collect_mods(&p, cfg, act, &p.ast.root_items, &job.path, &child_dir, &base_dir, true, &mut found);
                    }
                    Err(message) => err = Some(message),
                }
                let mut s = state.lock().unwrap();
                if let Some(e) = err {
                    s.errors.push(e);
                }
                for (cands, mod_rs, act) in found {
                    let mut ok = false;
                    for c in &cands {
                        if crate::vfs::exists(c) {
                            ok = true;
                            let mut dup = false;
                            for x in &s.seen {
                                if x == c {
                                    dup = true;
                                    break;
                                }
                            }
                            if !dup {
                                s.seen.push(c.clone());
                                let ismodrs = mod_rs || c.ends_with("/mod.rs");
                                s.queue.push(Job { crate_idx: job.crate_idx, path: c.clone(), edition: job.edition, mod_rs: ismodrs, active: act });
                            }
                            // #[path] alternatives (cfg_attr) may all exist: take all
                            if !mod_rs {
                                break;
                            }
                        }
                    }
                    if !ok && !cands.is_empty() {
                        s.missing.push(cands[0].clone());
                    }
                }
                s.files.push(SrcFile { crate_idx: job.crate_idx, path: job.path, edition: job.edition, src, mod_rs: job.mod_rs, active: job.active });
                s.active -= 1;
                cv.notify_all();
            } });
        }
    });
    let s = state.into_inner().unwrap();
    let mut files = s.files;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    (files, s.missing, s.errors)
}

/// Collects candidate files of `mod x;` declarations. Each entry: candidate paths, and
/// whether they came from a #[path] attribute (such files act like mod.rs).
#[allow(clippy::too_many_arguments)]
fn collect_mods(p: &ParsedFile, cfg: &crate::cfg::CfgSet, active: bool, items: &[ItemId], file: &str, child_dir: &str, base_dir: &str, top: bool, out: &mut Vec<(Vec<String>, bool, bool)>) {
    for &id in items {
        let it = p.ast.item(id);
        let act = active && crate::cfg::active(p.src_view(), p.ast.list(it.attrs), cfg);
        if let ItemKind::Mod(body) = &it.kind {
            let name = String::from_utf8_lossy(&p.src[it.name.lo as usize..it.name.hi as usize]).into_owned();
            let name = match name.strip_prefix("r#") {
                Some(n) => n.to_string(),
                None => name,
            };
            let mut paths = Vec::new();
            path_attrs(p, p.ast.list(it.attrs), cfg, &mut paths);
            match body {
                None => {
                    if !paths.is_empty() {
                        let rel = if top { base_dir } else { child_dir };
                        let mut c = Vec::new();
                        for x in &paths {
                            c.push(join_path(&rel, x));
                        }
                        out.push((c, true, act));
                    } else {
                        out.push((vec![format!("{}/{}.rs", child_dir, name), format!("{}/{}/mod.rs", child_dir, name)], false, act));
                    }
                }
                Some(inner) => {
                    let d = if !paths.is_empty() { join_path(&child_dir, &paths[0]) } else { format!("{}/{}", child_dir, name) };
                    collect_mods(p, cfg, act, p.ast.list(*inner), file, &d, &d, false, out);
                }
            }
        }
    }
}

/// `#[path = ".."]` relative to `base` with std's Path::join semantics: an absolute path replaces
/// the base (rustc resolves the attribute that way: Reference items/modules.md [items.mod.outlined.path]).
pub fn join_path(base: &str, p: &str) -> String {
    std::path::Path::new(base).join(p).to_string_lossy().into_owned()
}

/// Per crate (by name, as the parser knows it): the compile-time environment `env!`/`option_env!`
/// see (cargo's per-crate variables and build-script rustc-env). Set by `makepad run`; empty
/// otherwise. Lookups fall back to the process environment like rustc.
static CRATE_ENV: relax_session::sync::Mutex<Vec<(String, Vec<(String, String)>)>> = relax_session::sync::Mutex::new(Vec::new());

pub fn set_crate_env(name: &str, env: Vec<(String, String)>) {
    // the table lives for the run: grown in the kept phase
    let _k = crate::zone_alloc::kept();
    let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut t = CRATE_ENV.lock().unwrap();
    t.retain(|e| e.0 != name);
    t.push((name.to_string(), env));
}

/// Value of `env!(key)` / `option_env!(key)` in crate `krate`.
pub fn crate_env_named(krate: &str, key: &str) -> Option<String> {
    crate_env_registered(krate, key).or_else(|| std::env::var(key).ok())
}

/// The value `makepad run` registered for the crate (cargo's per-crate vars, build-script
/// rustc-env), without the process-environment fallback.
pub fn crate_env_registered(krate: &str, key: &str) -> Option<String> {
    let t = CRATE_ENV.lock().unwrap();
    for (n, e) in t.iter() {
        if n == krate {
            for (k, v) in e {
                if k == key {
                    return Some(v.clone());
                }
            }
        }
    }
    None
}
