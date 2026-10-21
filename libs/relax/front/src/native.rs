//! `--allow-c`: native dependencies built by real cargo.
//!
//! Without the flag nothing native from a third party runs. With it, the packages whose build
//! script compiles or links native code (`links =`, a build-dependency on cc / cmake / pkg-config /
//! bindgen / vcpkg, or a script that reads `DEP_*` metadata of those) are built by cargo itself, so
//! their build scripts run verbatim: `cargo check --release -p <package> --message-format=json`
//! in the run's workspace, same Cargo.lock, same cargo flags. Cargo's `RUSTC_WRAPPER` is this
//! executable (`wrapper`): it compiles with the real rustc only what the native scripts need (the
//! scripts of the native packages and their build-dependency closure) and answers every other
//! crate with empty outputs, so cargo decides features, cfgs and env for the whole graph and
//! Relax compiles the Rust. Cargo's `build-script-executed` messages give the cfgs, rustc-env and
//! OUT_DIR the Relax compile of those crates uses and the static archives and system libraries
//! the scripts link; the archives are linked into ONE shared library with the system linker,
//! which the run loads by path before the program starts.
//!
//! The cargo step and the link are cached under `<relax dir>/native/<key>/`: the key covers the
//! cargo flags, Cargo.lock, the native packages (versions, features, sources), the C compiler and
//! the target. A run with an unchanged key starts no cargo.

use crate::unitgraph::{BuildScript, Graph};
use std::process::{Command, Stdio};

/// What the run asks of cargo.
pub struct Request<'a> {
    pub workspace: &'a str,
    pub package: &'a str,
    pub cargo_flags: &'a [String],
    pub relax_dir: &'a str,
    pub triple: &'a str,
}

/// What cargo's build script of one native package produced.
pub struct ScriptOut {
    pub krate: usize,
    pub cfgs: Vec<String>,
    pub env: Vec<(String, String)>,
    pub out_dir: String,
    /// system libraries and frameworks the script links (name, framework)
    pub system: Vec<(String, bool)>,
}

/// The shared library the native archives were linked into.
pub struct NativeLib {
    pub path: String,
    /// the names of the static libraries inside it (`#[link(name = .., kind = "static")]`)
    pub statics: Vec<String>,
    /// the native packages ("name version")
    pub crates: Vec<String>,
}

pub struct Built {
    pub scripts: Vec<ScriptOut>,
    pub lib: Option<NativeLib>,
}

const BUILD_DEPS: [&str; 6] = ["cc", "cmake", "pkg_config", "bindgen", "vcpkg", "nasm_rs"];

/// Why a package's build script is native, if it is.
pub fn native_reason(g: &Graph, bs: &BuildScript) -> Option<String> {
    let (dir, name, _) = &g.pkg[bs.krate];
    let manifest = crate::vfs::read_to_string(format!("{}/Cargo.toml", dir)).unwrap_or_default();
    let links = manifest.lines().any(|l| {
        let l = l.trim();
        l.strip_prefix("links").is_some_and(|r| r.trim_start().starts_with('='))
    });
    if links {
        return Some(format!("{}: links =", name));
    }
    for (_, ext) in &bs.build_deps {
        if BUILD_DEPS.contains(&ext.as_str()) {
            return Some(format!("{}: build-dependency {}", name, ext));
        }
    }
    let src = crate::vfs::read_to_string(&bs.src_path).unwrap_or_default();
    if src.contains("DEP_") {
        return Some(format!("{}: reads DEP_ metadata", name));
    }
    None
}

fn fnv(data: &[u8], mut h: u64) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn hash_dir(dir: &std::path::Path, h: &mut u64) {
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let n = e.file_name();
        let n = n.to_string_lossy();
        if n == "target" || n == ".git" {
            continue;
        }
        let p = e.path();
        *h = fnv(n.as_bytes(), *h);
        match e.file_type() {
            Ok(t) if t.is_dir() => hash_dir(&p, h),
            Ok(_) => {
                if let Ok(d) = std::fs::read(&p) {
                    *h = fnv(&d, *h);
                }
            }
            Err(_) => {}
        }
    }
}

fn lockfile(workspace: &str) -> Vec<u8> {
    let mut p = std::path::Path::new(workspace).to_path_buf();
    loop {
        if let Ok(d) = std::fs::read(p.join("Cargo.lock")) {
            return d;
        }
        if !p.pop() {
            return Vec::new();
        }
    }
}

fn cc() -> String {
    std::env::var("CC").ok().filter(|c| !c.is_empty()).unwrap_or_else(|| "cc".to_string())
}

/// (name, version) of a cargo package id, either format.
fn id_name_version(id: &str) -> (String, String) {
    if id.contains('#') {
        let (_, n, v) = crate::unitgraph::parse_pkg_id(id);
        return (n, v);
    }
    let mut it = id.split_whitespace();
    (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string())
}

/// Builds the native packages `seeds` (indices into `g.build_scripts`) with cargo, or finds the
/// cached build, and links them. The error names what failed.
pub fn build(g: &Graph, seeds: &[usize], req: &Request) -> Result<Built, String> {
    let exe = std::env::current_exe().map_err(|e| format!("this executable: {e}"))?;
    let pk = |bs: &BuildScript| g.pkg[bs.krate].clone();
    // packages whose script runs for real: the native ones and the build-dependency closure of
    // those; packages whose lib compiles for real: that closure
    let mut script_pkgs: Vec<String> = Vec::new();
    let mut lib_pkgs: Vec<String> = Vec::new();
    for &s in seeds {
        let bs = &g.build_scripts[s];
        let name = pk(bs).1;
        if !script_pkgs.contains(&name) {
            script_pkgs.push(name);
        }
        let mut seen: Vec<usize> = Vec::new();
        let mut stack: Vec<usize> = bs.build_deps.iter().map(|d| d.0).collect();
        while let Some(i) = stack.pop() {
            if i >= g.raw.len() || seen.contains(&i) {
                continue;
            }
            seen.push(i);
            stack.extend(g.raw[i].deps.iter().map(|d| d.0));
        }
        for i in seen {
            let n = g.raw[i].pkg.1.clone();
            if !lib_pkgs.contains(&n) {
                lib_pkgs.push(n.clone());
            }
            if !script_pkgs.contains(&n) {
                script_pkgs.push(n);
            }
        }
    }
    script_pkgs.sort();
    lib_pkgs.sort();
    // cargo flags without `--release` (cargo check runs the release profile below)
    let flags: Vec<&String> = req.cargo_flags.iter().filter(|f| f.as_str() != "--release").collect();

    // ---- the key
    let mut h = fnv(req.triple.as_bytes(), 0xcbf29ce484222325);
    for f in &flags {
        h = fnv(f.as_bytes(), h);
        h = fnv(b"\0", h);
    }
    h = fnv(req.package.as_bytes(), h);
    h = fnv(&lockfile(req.workspace), h);
    let cc_version = Command::new(cc()).arg("--version").output().map(|o| o.stdout).unwrap_or_default();
    h = fnv(cc_version.split(|&b| b == b'\n').next().unwrap_or(&[]), h);
    let mut names: Vec<String> = Vec::new();
    for &s in seeds {
        let bs = &g.build_scripts[s];
        let (dir, name, ver) = pk(bs);
        names.push(format!("{} {}", name, ver));
        h = fnv(format!("{} {} {:?}", name, ver, bs.features).as_bytes(), h);
        hash_dir(std::path::Path::new(&dir), &mut h);
    }
    names.sort();
    let dir = format!("{}/native/{:016x}", req.relax_dir, h);
    let messages_path = format!("{}/messages.jsonl", dir);
    let ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let lib_path = format!("{}/libnative.{}", dir, ext);

    let messages = match std::fs::read_to_string(&messages_path) {
        Ok(m) if std::fs::metadata(&lib_path).is_ok() || !m.contains("\"linked_libs\"") => m,
        _ => {
            let m = run_cargo(g, req, &exe, &flags, &script_pkgs, &lib_pkgs, seeds)?;
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {}", dir, e))?;
            std::fs::write(&messages_path, &m).map_err(|e| format!("{}: {}", messages_path, e))?;
            // a stale library of an earlier link under this key is not trusted
            let _ = std::fs::remove_file(&lib_path);
            m
        }
    };

    // ---- what cargo's scripts produced
    let mut scripts = Vec::new();
    let mut statics: Vec<String> = Vec::new();
    let mut archives: Vec<String> = Vec::new();
    let mut search: Vec<String> = Vec::new();
    let mut fw_search: Vec<String> = Vec::new();
    let mut dylibs: Vec<String> = Vec::new();
    let mut frameworks: Vec<String> = Vec::new();
    for line in messages.lines() {
        let j = crate::json::parse(line.as_bytes()).map_err(|e| format!("cargo message: {e}"))?;
        let (n, v) = id_name_version(j.get("package_id").map(|p| p.str()).unwrap_or(""));
        let Some(&s) = seeds.iter().find(|&&s| {
            let p = pk(&g.build_scripts[s]);
            p.1 == n && p.2 == v
        }) else { continue };
        let strs = |k: &str| -> Vec<String> { j.get(k).map(|a| a.arr().iter().map(|x| x.str().to_string()).collect()).unwrap_or_default() };
        let mut out = ScriptOut { krate: g.build_scripts[s].krate, cfgs: strs("cfgs"), env: Vec::new(), out_dir: j.get("out_dir").map(|o| o.str().to_string()).unwrap_or_default(), system: Vec::new() };
        if let Some(e) = j.get("env") {
            for kv in e.arr() {
                if let [k, v] = kv.arr() {
                    out.env.push((k.str().to_string(), v.str().to_string()));
                }
            }
        }
        for p in strs("linked_paths") {
            match p.split_once('=') {
                Some(("framework", d)) => fw_search.push(d.to_string()),
                Some((_, d)) => search.push(d.to_string()),
                None => search.push(p),
            }
        }
        for l in strs("linked_libs") {
            let (kind, name) = match l.split_once('=') {
                Some((k, n)) => (k.to_string(), n.split(':').next().unwrap_or(n).to_string()),
                None => (String::new(), l.clone()),
            };
            match kind.as_str() {
                "static" => statics.push(name),
                "framework" => {
                    frameworks.push(name.clone());
                    out.system.push((name, true));
                }
                _ => {
                    dylibs.push(name.clone());
                    out.system.push((name, false));
                }
            }
        }
        scripts.push(ScriptOut { ..out });
    }
    // the out dirs of the scripts hold archives too (cc's output is `native=<out_dir>`)
    for name in &statics {
        let file = format!("lib{}.a", name);
        match search.iter().map(|d| format!("{}/{}", d, file)).find(|p| std::path::Path::new(p).is_file()) {
            Some(p) => {
                if !archives.contains(&p) {
                    archives.push(p);
                }
            }
            None => return Err(format!("--allow-c: a native build script links static library `{}`, but {} is in none of its search paths ({})", name, file, search.join(", "))),
        }
    }
    if archives.is_empty() && dylibs.is_empty() && frameworks.is_empty() {
        return Ok(Built { scripts, lib: None });
    }
    if !std::path::Path::new(&lib_path).is_file() {
        link(&lib_path, &archives, &search, &fw_search, &dylibs, &frameworks)?;
    }
    Ok(Built { scripts, lib: Some(NativeLib { path: lib_path, statics, crates: names }) })
}

fn run_cargo(g: &Graph, req: &Request, exe: &std::path::Path, flags: &[&String], script_pkgs: &[String], lib_pkgs: &[String], seeds: &[usize]) -> Result<String, String> {
    let target = format!("{}/native/cargo-target", req.relax_dir);
    eprintln!("makepad run: --allow-c: building native dependencies with cargo (cached afterwards): {}", script_pkgs.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
    let mut cmd = Command::new("cargo");
    cmd.current_dir(req.workspace)
        .args(["check", "--release", "--message-format=json", "-p", req.package])
        .args(flags.iter().map(|f| f.as_str()))
        .env("CARGO_TARGET_DIR", &target)
        .env("CARGO_INCREMENTAL", "0")
        .env("RUSTC_WRAPPER", exe)
        .env("MAKEPAD_RUN_NATIVE_SCRIPTS", script_pkgs.join(","))
        .env("MAKEPAD_RUN_NATIVE_LIBS", lib_pkgs.join(","))
        .env_remove("RUSTC")
        .env_remove("MAKEPAD_RUN_AS_RUSTC")
        .env_remove("MAKEPAD_RUN_SELF_EXEC")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output().map_err(|e| format!("--allow-c needs cargo on PATH to build the native dependencies ({}): {e}", req.package))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        // cargo's own messages; a native script's failure is in `compiler-message`s and stderr
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(40).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        return Err(format!("--allow-c: cargo failed building the native dependencies ({}):\n{}", script_pkgs.join(", "), tail.join("\n")));
    }
    // the feature cross-check, and the messages worth keeping
    let mut keep = String::new();
    for line in stdout.lines() {
        let Ok(j) = crate::json::parse(line.as_bytes()) else { continue };
        let reason = j.get("reason").map(|r| r.str()).unwrap_or("");
        let (n, v) = id_name_version(j.get("package_id").map(|p| p.str()).unwrap_or(""));
        let seed = seeds.iter().find(|&&s| {
            let p = &g.pkg[g.build_scripts[s].krate];
            p.1 == n && p.2 == v
        });
        let Some(&s) = seed else { continue };
        match reason {
            "build-script-executed" => {
                keep.push_str(line);
                keep.push('\n');
            }
            "compiler-artifact" => {
                let is_lib = j.get("target").and_then(|t| t.get("kind")).is_some_and(|k| k.arr().iter().any(|x| x.str() == "lib" || x.str() == "rlib"));
                if is_lib {
                    let mut cargo_feats: Vec<String> = j.get("features").map(|a| a.arr().iter().map(|x| x.str().to_string()).collect()).unwrap_or_default();
                    let mut ours = g.build_scripts[s].features.clone();
                    cargo_feats.sort();
                    ours.sort();
                    if cargo_feats != ours {
                        return Err(format!("--allow-c: cargo and Relax resolve `{} {}` with different features (cargo {:?}, relax {:?})", n, v, cargo_feats, ours));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(keep)
}

fn link(lib_path: &str, archives: &[String], search: &[String], fw_search: &[String], dylibs: &[String], frameworks: &[String]) -> Result<(), String> {
    if let Some(d) = std::path::Path::new(lib_path).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = format!("{}.tmp{}", lib_path, std::process::id());
    let mut cmd = Command::new(cc());
    if cfg!(target_os = "macos") {
        cmd.args(["-dynamiclib", "-o", &tmp, "-Wl,-undefined,dynamic_lookup"]);
        for a in archives {
            cmd.arg(format!("-Wl,-force_load,{}", a));
        }
    } else {
        cmd.args(["-shared", "-fPIC", "-o", &tmp]);
        cmd.arg("-Wl,--whole-archive");
        cmd.args(archives);
        cmd.arg("-Wl,--no-whole-archive");
        // what the archives' C code needs from the C runtime
        cmd.args(["-lm", "-lpthread", "-ldl"]);
    }
    for d in search {
        cmd.arg(format!("-L{}", d));
    }
    for d in fw_search {
        cmd.arg(format!("-F{}", d));
    }
    for l in dylibs {
        cmd.arg(format!("-l{}", l));
    }
    for f in frameworks {
        cmd.args(["-framework", f]);
    }
    let out = cmd.output().map_err(|e| format!("--allow-c: cannot run the C compiler to link the native library: {e}"))?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("--allow-c: linking the native library failed ({} archive(s): {}):\n{}", archives.len(), archives.join(" "), String::from_utf8_lossy(&out.stderr)));
    }
    std::fs::rename(&tmp, lib_path).map_err(|e| format!("{}: {}", lib_path, e))
}

// ---- cargo's RUSTC_WRAPPER

/// This executable as cargo's `RUSTC_WRAPPER` (`MAKEPAD_RUN_NATIVE_SCRIPTS` set) or as a stubbed
/// build script. Returns the exit code when it acted, None when this is an ordinary start.
pub fn wrapper() -> Option<i32> {
    let scripts = std::env::var("MAKEPAD_RUN_NATIVE_SCRIPTS").ok()?;
    let args: Vec<String> = std::env::args().collect();
    // a build script stub (cargo runs what the wrapper left as the script): nothing to do
    let stem = std::path::Path::new(&args[0]).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if std::env::var_os("OUT_DIR").is_some() && (stem.starts_with("build_script_build") || stem.starts_with("build-script-build")) {
        return Some(0);
    }
    // cargo runs `<wrapper> <rustc> <args>`
    if args.len() < 2 || std::env::var_os("RUSTC_WRAPPER").is_none() && std::env::var_os("CARGO_PKG_NAME").is_none() {
        return None;
    }
    let rustc = &args[1];
    let rest = &args[2..];
    let value = |flag: &str| -> Option<String> {
        let mut i = 0;
        while i < rest.len() {
            if rest[i] == flag {
                return rest.get(i + 1).cloned();
            }
            if let Some(v) = rest[i].strip_prefix(&format!("{}=", flag)) {
                return Some(v.to_string());
            }
            i += 1;
        }
        None
    };
    let name = value("--crate-name");
    let real = match &name {
        None => true,
        Some(n) => {
            let pkg = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
            let list = |v: &str| std::env::var(v).unwrap_or_default().split(',').any(|x| x == pkg);
            if n == "build_script_build" { scripts.split(',').any(|x| x == pkg) } else { list("MAKEPAD_RUN_NATIVE_LIBS") }
        }
    };
    if real {
        return Some(match Command::new(rustc).args(rest).status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => {
                eprintln!("makepad run: native wrapper: cannot run {}: {}", rustc, e);
                1
            }
        });
    }
    // a stub: the files cargo expects, empty
    let name = name.unwrap();
    let out_dir = value("--out-dir").unwrap_or_else(|| ".".to_string());
    let mut extra = String::new();
    let mut i = 0;
    while i < rest.len() {
        if let Some(v) = rest[i].strip_prefix("-Cextra-filename=") {
            extra = v.to_string();
        } else if rest[i] == "-C" {
            if let Some(v) = rest.get(i + 1).and_then(|x| x.strip_prefix("extra-filename=")) {
                extra = v.to_string();
            }
        }
        i += 1;
    }
    let types: Vec<String> = rest.iter().enumerate().filter_map(|(i, a)| if a == "--crate-type" { rest.get(i + 1).cloned() } else { a.strip_prefix("--crate-type=").map(str::to_string) }).collect();
    let emit = value("--emit").unwrap_or_default();
    let json = value("--json").unwrap_or_default().contains("artifacts");
    let dylib_ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
    let mut made: Vec<(String, &str)> = Vec::new();
    for e in emit.split(',') {
        match e.split('=').next().unwrap_or("") {
            "dep-info" => made.push((format!("{}/{}{}.d", out_dir, name, extra), "dep-info")),
            "metadata" => made.push((format!("{}/lib{}{}.rmeta", out_dir, name, extra), "metadata")),
            "link" => {
                for t in &types {
                    let f = match t.as_str() {
                        "bin" => format!("{}/{}{}", out_dir, name, extra),
                        "rlib" | "lib" => format!("{}/lib{}{}.rlib", out_dir, name, extra),
                        "staticlib" => format!("{}/lib{}{}.a", out_dir, name, extra),
                        _ => format!("{}/lib{}{}.{}", out_dir, name, extra, dylib_ext),
                    };
                    made.push((f, "link"));
                }
            }
            _ => {}
        }
    }
    let exe = std::env::current_exe().ok();
    for (path, kind) in &made {
        let _ = std::fs::create_dir_all(&out_dir);
        let is_bin = kind == &"link" && types.iter().any(|t| t == "bin") && path.ends_with(&format!("{}{}", name, extra));
        if is_bin {
            // the stub script is this executable (wrapper(): a build script stub exits at once)
            let _ = std::fs::remove_file(path);
            if let Some(e) = &exe {
                if std::fs::hard_link(e, path).is_err() {
                    let _ = std::fs::copy(e, path);
                }
            }
        } else if *kind == "dep-info" {
            let src = rest.iter().find(|a| a.ends_with(".rs")).cloned().unwrap_or_default();
            let _ = std::fs::write(path, format!("{}: {}\n", path, src));
        } else {
            let _ = std::fs::write(path, b"");
        }
        if json {
            eprintln!("{{\"$message_type\":\"artifact\",\"artifact\":\"{}\",\"emit\":\"{}\"}}", path.replace('\\', "\\\\").replace('"', "\\\""), kind);
        }
    }
    Some(0)
}
