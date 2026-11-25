//! Third-party build scripts under the two policies (relax-compiler sandbox/script.rs): the test
//! writes workspaces into a temp folder whose script packages are registry packages of a vendored
//! crates-io directory (nothing is downloaded), and runs each case's app under `Policy::Open`
//! (bare `relax`'s, where the script is host code with the person's rights) and under
//! `Policy::Sandboxed` (`makepad`'s, where it runs confined): this binary re-executes itself as
//! the runner with each policy. Under Open every escape acts (its marker appears, or it reports what
//! it reached), which proves the case live; under Sandboxed it is refused and nothing acts.
//!
//! - soft: escapes a script meets as ordinary errors: writes outside OUT_DIR (an absolute path,
//!   its own package, `OUT_DIR/../..`), a symlink the package ships, a rename out of OUT_DIR, a
//!   removal outside, `set_current_dir` then a relative write, `/dev/fd`, a file outside every
//!   root, `sh`, `$RUSTC run`, `$CARGO metadata`, the run's folder. Its manifest claims the
//!   `keep` role, which grants nothing. Beside it, a registry script whose build-dependency's own
//!   script escapes (nested runs decide for themselves), and a path script whose registry
//!   build-dependency escapes (the closure rule): that script reads its workspace, and the
//!   target folder three ways (directly, through a workspace symlink, up from OUT_DIR) finds
//!   nothing.
//! - positive: the shapes of the ecosystem's scripts (a version probe, thiserror's probe files
//!   and failed compile probe, typenum's large generated file, cfg and feature lines, a 255-byte
//!   name listed, `canonicalize(OUT_DIR)`, an ancestor walk that finds no workspace, `$RUSTC`
//!   questions with a target file, an `@file` and `-o`, stdin inherited, an exit handler, a tail
//!   with no newline before `process::exit(0)`): the same output under both.
//! - store: a registry package of a workspace served from a source store (`run --src`) reads and
//!   lists its own source, also from its virtual current folder; a link entry it ships reads as
//!   nothing under both; the workspace beside it is the person's, read under Open only.
//! - refused by name: symlink, hard link, `system`, fork, `posix_spawn`, `dlopen`, threads, a TCP
//!   connect to a listener the test holds, a name lookup: under Sandboxed the script fails naming
//!   the C function and no marker appears (the listener sees nothing). A raw `syscall` never
//!   reaches the kernel (Linux: futexes alone, ENOSYS for any other number).
//! - failures: a constructor (refused under Sandboxed, run under Open), a wild write (contained in
//!   the arena under Sandboxed), `process::exit(3)`, a panic, `main` returning `Err`.
//! - decision: a confined script's own run given its units file without its folders, or with the
//!   registry bit cleared (with and without the confined environment), refuses to run; the file
//!   as written runs the script.
//! - output lines: a confined script's `cargo:` lines act in the run with the person's rights, so
//!   under Sandboxed each that reaches past its folders fails the build, named: link-search paths
//!   outside (absolute, `../..`, the target folder, a shipped link out, `rustc-flags -L`), link-lib
//!   names that are paths or of another kind (`rustc-flags -l`, a flag other than `-l`/`-L`),
//!   every `rustc-link-arg` form and an unknown `cargo::` key, `rustc-env` of a name of every family
//!   that steers the run's process or its tools (and one without `=`), `rerun-if-changed` paths
//!   outside. Under Open the build goes on, and where the effect shows it shows: the program sees
//!   each variable, a change in a folder outside reruns the script, and a library placed in the
//!   working folder is loaded; under Sandboxed it is not. The positive script's own folders,
//!   variables, inputs and a plain system library pass under both.
use std::path::{Path, PathBuf};
use std::process::Command;

const AS_RUNNER: &str = "RELAX_CONFINED_SCRIPTS_RUNNER";

fn main() {
    // A build script's run is this binary again, under the same policy.
    if let Ok(policy) = std::env::var(AS_RUNNER) {
        if policy == "open" {
            relax_base::host::init();
            // This fixture process has yet to start a session or build script.
            unsafe { relax_compiler::sandbox::replace_policy_in_test(relax_compiler::sandbox::Policy::Open) };
        }
        relax_runner::main(&makepad_run::HOST);
        return;
    }
    let t = std::env::temp_dir().join(format!("confined-scripts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&t);
    let policies = ["open", "sandboxed"];
    let places: Vec<Place> = policies.iter().map(|p| Place::new(&t.join(p))).collect();
    // every (policy, case) run, a few at a time
    let jobs: Vec<(usize, &Case)> = (0..policies.len()).flat_map(|p| CASES.iter().map(move |c| (p, c))).collect();
    let results: Vec<std::sync::Mutex<Option<Out>>> = jobs.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((p, case)) = jobs.get(i) else { break };
                let place = &places[*p];
                let env: &[(&str, &str)] = if case.name == "out_cwdlib" && cfg!(target_os = "linux") { &[("LD_LIBRARY_PATH", ".")] } else { &[] };
                let out = if case.name == "store" { run_store(place, policies[*p]) } else { run_in(&place.ws, policies[*p], &format!("a_{}", case.name), &[], None, env) };
                *results[i].lock().unwrap() = Some(out);
            });
        }
    });
    let out = |p: usize, name: &str| -> Out { results[jobs.iter().position(|(q, c)| *q == p && c.name == name).unwrap()].lock().unwrap().clone().unwrap() };
    let (open, sb) = (&places[0], &places[1]);
    let mut bad: Vec<String> = Vec::new();
    let show = |o: &Out| format!("ok {}\nstdout:\n{}\nstderr:\n{}", o.0, o.1, o.2);
    // soft: every escape acts under Open and is an ordinary error under Sandboxed
    let soft_sb = "abs=PermissionDenied own=PermissionDenied dotdot=PermissionDenied link_read=NotFound link_write=PermissionDenied rename=PermissionDenied remove=NotFound chdir=NotFound rel=PermissionDenied devfd=NotFound hidden_read=NotFound sh=NotFound rustc_run=NotFound cargo=NotFound run_dir=NotFound";
    let soft_open = soft_sb.split(' ').map(|kv| format!("{}=ok", kv.split('=').next().unwrap())).collect::<Vec<_>>().join(" ");
    let nested = |v: &str| format!("nested {v}");
    let pathy_sb = "helper=PermissionDenied ws_read=ok target_direct=NotFound target_link=NotFound target_up=NotFound out_rw=ok";
    let pathy_open = "helper=ok ws_read=ok target_direct=ok target_link=ok target_up=ok out_rw=ok";
    let (so, ss) = (out(0, "soft"), out(1, "soft"));
    if !so.0 || so.1.lines().collect::<Vec<_>>() != [soft_open.as_str(), &nested("ok"), pathy_open] {
        bad.push(format!("soft under Open: wanted every escape to act\n{}", show(&so)));
    }
    for m in ["abs", "renamed", "cwd", "sh", "nested", "helper"] {
        if !open.markers.join(m).exists() {
            bad.push(format!("soft under Open: no marker {m} (the case did not act)"));
        }
    }
    if open.victim.exists() || !open.secret.join("through").exists() {
        bad.push("soft under Open: the removal outside or the write through the shipped link did not act".to_string());
    }
    if !ss.0 || ss.1.lines().collect::<Vec<_>>() != [soft_sb, &nested("PermissionDenied"), pathy_sb] {
        bad.push(format!("soft under Sandboxed: wanted every escape refused\nwanted:\n{soft_sb}\n{}\n{pathy_sb}\n{}", nested("PermissionDenied"), show(&ss)));
    }
    if !sb.victim.exists() || sb.secret.join("through").exists() || sb.ws.join("vendor/c_soft/own.txt").exists() {
        bad.push("soft under Sandboxed: a removal outside, a write through the shipped link or into its package acted".to_string());
    }
    // positive: the same output under both, with the exit handler's line and the tail
    let (po, ps) = (out(0, "pos"), out(1, "pos"));
    if !po.0 || !ps.0 || po.1 != ps.1 || !ps.1.contains("tail tail") || !ps.1.contains("exit handler ran") {
        bad.push(format!("positive: wanted the same lines under both\nOpen: {}\nSandboxed: {}", show(&po), show(&ps)));
    }
    if !open.markers.join("atexit").exists() || sb.markers.join("atexit").exists() {
        bad.push("positive: the exit handler's write outside its folders acted under Sandboxed, or not under Open (it runs in the script's arena)".to_string());
    }
    if open.markers.join("o-out").exists() || sb.markers.join("o-out").exists() {
        bad.push("positive: a `$RUSTC -o` question wrote its file".to_string());
    }
    // store: its own source under both; the workspace beside it under Open only
    let (vo, vs) = (out(0, "store"), out(1, "store"));
    let store_line = |up: &str| format!("manifest 6|listing lib.rs|relative ok|meta true|link NotFound|up {up}");
    if !vo.0 || vo.1.trim() != store_line("ok") {
        bad.push(format!("store under Open: wanted {:?}\n{}", store_line("ok"), show(&vo)));
    }
    if !vs.0 || vs.1.trim() != store_line("NotFound") {
        bad.push(format!("store under Sandboxed: wanted {:?}\n{}", store_line("NotFound"), show(&vs)));
    }
    // refused by name: live under Open, a named refusal and no marker under Sandboxed
    for c in CASES.iter().filter(|c| c.kind == Kind::Named) {
        let (o, s) = (out(0, c.name), out(1, c.name));
        if !o.0 || !o.1.contains(&format!("{} live", c.name)) {
            bad.push(format!("{} under Open: it did not act\n{}", c.name, show(&o)));
        }
        if s.0 || !s.2.contains("reaches the C function `") {
            bad.push(format!("{} under Sandboxed: no named refusal\n{}", c.name, show(&s)));
        }
        if sb.markers.join(c.name).exists() {
            bad.push(format!("{}: its marker appeared under Sandboxed", c.name));
        }
    }
    // a raw system call: live under Open; under Sandboxed it never reaches the kernel (Linux's
    // `syscall` serves futexes alone, ENOSYS for any other number; macOS has none)
    let (yo, ys) = (out(0, "syscall"), out(1, "syscall"));
    if !yo.0 || !yo.1.contains("syscall live") {
        bad.push(format!("syscall under Open: it did not act\n{}", show(&yo)));
    }
    if ys.1.contains("syscall live") || !ys.0 && !ys.2.contains("reaches the C function `") {
        bad.push(format!("syscall under Sandboxed: wanted no system call made\n{}", show(&ys)));
    }
    for (place, policy, want) in [(open, "Open", 1), (sb, "Sandboxed", 0)] {
        let seen = place.connections();
        if seen != want {
            bad.push(format!("tcp under {policy}: the test's listener saw {seen} connection(s), wanted {want}"));
        }
    }
    // failures
    let (co, cs) = (out(0, "ctor"), out(1, "ctor"));
    if !co.0 || !open.markers.join("ctor").exists() {
        bad.push(format!("ctor under Open: the constructor did not run\n{}", show(&co)));
    }
    if cs.0 || !cs.2.contains("links constructors") || sb.markers.join("ctor").exists() {
        bad.push(format!("ctor under Sandboxed: wanted the constructor refused\n{}", show(&cs)));
    }
    let (wo, ws) = (out(0, "wild"), out(1, "wild"));
    if wo.0 || ws.0 || !ws.2.contains("faulted inside its arena") || ws.2.contains("a Relax bug") || ws.1.contains("wild survived") {
        bad.push(format!("wild: wanted a fault under both, contained in the arena under Sandboxed\nOpen: {}\nSandboxed: {}", show(&wo), show(&ws)));
    }
    for (name, code, says) in [("exit3", "exit 3", "exit three"), ("panic", "exit 101", "script panicked here"), ("main_err", "exit 1", "main said no")] {
        for p in 0..2 {
            let o = out(p, name);
            if o.0 || !o.2.contains(code) || !o.2.contains(says) {
                bad.push(format!("{name} under {}: wanted a failure with {code:?} and {says:?}\n{}", policies[p], show(&o)));
            }
        }
    }
    // decision: the confined script's own run refuses a units file its parent did not write
    decision(sb, &mut bad);
    // output lines: under Open the build goes on; under Sandboxed it fails naming every line
    let refuses = "ran confined and printed what a confined build script may not";
    for (name, lines) in OUT_LINES {
        let (o, s) = (out(0, name), out(1, name));
        if !o.0 || !o.1.contains("lines printed") {
            bad.push(format!("{name} under Open: wanted the build to go on\n{}", show(&o)));
        }
        let missing: Vec<String> = lines.iter().map(|l| sb.subst(l, 0)).filter(|l| !s.2.contains(&format!("`{l}`: "))).collect();
        if s.0 || !s.2.contains(refuses) || !missing.is_empty() {
            bad.push(format!("{name} under Sandboxed: wanted a refusal naming every line; not named: {missing:?}\n{}", show(&s)));
        }
    }
    // rustc-env: the program sees every variable under Open; under Sandboxed every name is refused
    // with the principle, and the line without `=` too
    let (eo, es) = (out(0, "out_env"), out(1, "out_env"));
    for n in ENV_NAMES {
        // (at compile time through env!, and at run time in the process it shares with the run)
        let want = format!("env {n} true true");
        if !eo.0 || !eo.1.lines().any(|l| l == want) {
            bad.push(format!("out_env under Open: wanted {want:?}\n{}", show(&eo)));
        }
        if es.0 || !es.2.contains(&format!("`cargo:rustc-env={n}=relaxfix-{n}`: `{n}`: a third-party build script sets variables for the program")) {
            bad.push(format!("out_env under Sandboxed: {n} not refused with the principle\n{}", show(&es)));
        }
    }
    if !es.2.contains("`cargo:rustc-env=RELAXFIX_NO_VALUE`: it names no variable") {
        bad.push(format!("out_env under Sandboxed: the line without `=` was not refused\n{}", show(&es)));
    }
    // rerun-if-changed outside: under Open a change there reruns the script (and none does not)
    let count = || std::fs::read_to_string(open.markers.join("out_reach")).map(|s| s.len()).unwrap_or(0);
    let (ro, rs) = (out(0, "out_reach"), out(1, "out_reach"));
    let first = count();
    let again = run(&open.ws, "open", "a_out_reach", &[]);
    let still = count();
    std::fs::write(open.outside.join("f"), "2").unwrap();
    let changed = run(&open.ws, "open", "a_out_reach", &[]);
    let then = count();
    if !ro.0 || !again.0 || !changed.0 || (first, still, then) != (1, 1, 2) {
        bad.push(format!("out_reach under Open: wanted the script to run once, stand, then rerun on the change outside: runs {first} {still} {then}\n{}\n{}", show(&again), show(&changed)));
    }
    if rs.0 || !rs.2.contains(&format!("`cargo:rerun-if-changed={}`: it lies outside", sb.outside.display())) || sb.markers.join("out_reach").exists() {
        bad.push(format!("out_reach under Sandboxed: wanted the outside path refused\n{}", show(&rs)));
    }
    // a library in the working folder: loaded under Open, never under Sandboxed
    let (lo, ls) = (out(0, "out_cwdlib"), out(1, "out_cwdlib"));
    if !lo.0 || !lo.1.contains("cwdlib loaded") {
        bad.push(format!("out_cwdlib under Open: wanted the working folder's library loaded\n{}", show(&lo)));
    }
    if !ls.0 || !ls.1.contains("cwdlib not loaded") {
        bad.push(format!("out_cwdlib under Sandboxed: wanted the run to go on without the working folder's library\n{}", show(&ls)));
    }
    let _ = std::fs::remove_dir_all(&t);
    if !bad.is_empty() {
        eprintln!("confined_scripts: FAILED\n{}", bad.join("\n"));
        std::process::exit(1);
    }
    println!("confined_scripts: ok ({} cases under both policies: escapes act under Open and are refused under Sandboxed; positives equal; the decision refuses units its parent did not write)", CASES.len());
}

/// (success, stdout, stderr)
type Out = (bool, String, String);

#[derive(PartialEq)]
enum Kind {
    /// its own expectations (main)
    Own,
    /// live under Open (prints `<name> live`), refused naming a C function under Sandboxed
    Named,
}

struct Case {
    name: &'static str,
    kind: Kind,
    /// the registry package's build script (MARKERS, SECRET, VICTIM, PORT: the policy's)
    script: &'static str,
}

/// One policy's folders: the workspace, its markers, the folder outside every root, the folder
/// the soft case removes, and the TCP listener its scripts connect to.
struct Place {
    ws: PathBuf,
    markers: PathBuf,
    secret: PathBuf,
    victim: PathBuf,
    /// a folder outside every root that only the rerun cases name
    outside: PathBuf,
    listener: std::net::TcpListener,
}

impl Place {
    fn new(t: &Path) -> Place {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let (markers, secret, victim, outside) = (t.join("markers"), t.join("secret"), t.join("victim"), t.join("outside"));
        for d in [&markers, &secret, &victim, &outside] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(outside.join("f"), "1").unwrap();
        std::fs::write(secret.join("secret-entry"), "secret").unwrap();
        std::fs::write(victim.join("kept"), "").unwrap();
        let port = listener.local_addr().unwrap().port();
        let place = Place { ws: t.join("ws"), markers, secret, victim, outside, listener };
        place.workspace(port);
        place.store_workspace(&t.join("vws"));
        place
    }

    fn connections(&self) -> usize {
        std::iter::from_fn(|| self.listener.accept().ok()).count()
    }

    fn subst(&self, text: &str, port: u16) -> String {
        text.replace("MARKERS", &self.markers.to_string_lossy())
            .replace("SECRET", &self.secret.to_string_lossy())
            .replace("VICTIM", &self.victim.to_string_lossy())
            .replace("OUTSIDE", &self.outside.to_string_lossy())
            .replace("WSTARGET", &self.ws.join("target").to_string_lossy())
            .replace("PORT", &port.to_string())
    }

    fn workspace(&self, port: u16) {
        let ws = &self.ws;
        let put = |rel: &str, text: &str| {
            let p = ws.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        let mut members: Vec<String> = CASES.iter().filter(|c| c.name != "store").map(|c| format!("\"a_{}\"", c.name)).collect();
        members.push("\"pathy\"".to_string());
        put("Cargo.toml", &format!("[workspace]\nresolver = \"2\"\nmembers = [{}]\n", members.join(", ")));
        put(".cargo/config.toml", "[source.crates-io]\nreplace-with = \"v\"\n[source.v]\ndirectory = \"vendor\"\n");
        for c in CASES.iter().filter(|c| c.name != "store" && c.name != "out_env") {
            let name = format!("c_{}", c.name);
            let script = match OUT_LINES.iter().find(|(n, _)| *n == c.name) {
                Some((_, lines)) => lines_script(lines),
                None => c.script.to_string(),
            };
            let meta = if c.name == "soft" { "[package.metadata.relax]\nbuild-script = \"keep\"\n" } else { "" };
            let features = if c.name == "pos" { "[features]\nextra = []\n" } else { "" };
            vendored(ws, &name, &format!("{meta}{features}"), "", &self.subst(&script, port), "pub const OUT: &str = env!(\"CASE_OUT\");\n");
            let (deps, prints) = match c.name {
                "soft" => ("c_soft = \"0.1.0\"\nc_nested_top = \"0.1.0\"\npathy = { path = \"../pathy\" }\n", "    println!(\"{}\", c_soft::OUT);\n    println!(\"{}\", c_nested_top::OUT);\n    println!(\"{}\", pathy::PATHY);\n"),
                "pos" => ("c_pos = { version = \"0.1.0\", features = [\"extra\"] }\n", "    println!(\"{}\", c_pos::OUT.replace('|', \"\\n\"));\n    println!(\"cfg {}\", c_pos::CFG);\n    println!(\"tail {}\", c_pos::TAIL);\n    println!(\"{}\", c_pos::ATEXIT);\n    println!(\"plain {}\", c_pos::PLAIN);\n"),
                _ => ("", ""),
            };
            let (deps, prints) = if deps.is_empty() { (format!("{name} = \"0.1.0\"\n"), format!("    println!(\"{{}}\", {name}::OUT);\n")) } else { (deps.to_string(), prints.to_string()) };
            put(&format!("a_{}/Cargo.toml", c.name), &package(&format!("a_{}", c.name), &deps));
            let main = if c.name == "out_cwdlib" { CWDLIB_MAIN.to_string() } else { format!("fn main() {{\n{prints}}}\n") };
            put(&format!("a_{}/src/main.rs", c.name), &main);
        }
        // the soft case's package ships a symlink to the folder outside every root
        std::os::unix::fs::symlink(&self.secret, ws.join("vendor/c_soft/shipped")).unwrap();
        // the positive case's lib: its cfg and its exit handler's and tail's lines
        put("vendor/c_pos/src/lib.rs", "pub const OUT: &str = env!(\"CASE_OUT\");\npub const CFG: bool = cfg!(any(pos_os_linux, pos_os_macos));\npub const TAIL: &str = env!(\"POS_TAIL\");\npub const ATEXIT: &str = env!(\"POS_ATEXIT\");\npub const PLAIN: &str = env!(\"POS_PLAIN\");\n");
        // nested: a registry script whose build-dependency's own script escapes
        vendored(ws, "c_nested_dep", "", "", &self.subst(NESTED_DEP, port), "pub const NESTED: &str = env!(\"NESTED\");\n");
        vendored(ws, "c_nested_top", "", "c_nested_dep = \"0.1.0\"\n", "fn main() {\n    println!(\"cargo:rustc-env=CASE_OUT=nested {}\", c_nested_dep::NESTED);\n}\n", "pub const OUT: &str = env!(\"CASE_OUT\");\n");
        // the closure rule: a path script whose registry build-dependency escapes
        vendored(ws, "c_reghelp", "", "", "", "pub fn escape(p: &str) -> String {\n    match std::fs::write(p, \"x\") {\n        Ok(_) => \"ok\".to_string(),\n        Err(e) => format!(\"{:?}\", e.kind()),\n    }\n}\n");
        std::fs::remove_file(ws.join("vendor/c_reghelp/build.rs")).unwrap();
        put("vendor/c_reghelp/Cargo.toml", "[package]\nname = \"c_reghelp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        put("pathy/Cargo.toml", "[package]\nname = \"pathy\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n[build-dependencies]\nc_reghelp = \"0.1.0\"\n");
        put("pathy/build.rs", &self.subst(PATHY, port));
        put("pathy/src/lib.rs", "pub const PATHY: &str = env!(\"PATHY\");\n");
        std::os::unix::fs::symlink("target", ws.join("tlink")).unwrap();
        // the output-line cases' packages ship a link to the folder outside every root
        for name in ["c_out_search", "c_out_rerun"] {
            std::os::unix::fs::symlink(&self.outside, ws.join("vendor").join(name).join("shipped")).unwrap();
        }
        // rustc-env: the program's own package, its script confined by its registry build-dependency
        put("a_out_env/Cargo.toml", "[package]\nname = \"a_out_env\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n[build-dependencies]\nc_reghelp = \"0.1.0\"\n");
        let mut script = String::from("fn main() {\n    let _ = c_reghelp::escape(&format!(\"{}/x\", std::env::var(\"OUT_DIR\").unwrap()));\n");
        let mut main = String::from("fn main() {\n    let seen: [(&str, Option<&str>); ENV_COUNT] = [\n");
        for n in ENV_NAMES {
            script.push_str(&format!("    println!(\"cargo:rustc-env={n}=relaxfix-{n}\");\n"));
            main.push_str(&format!("        ({n:?}, option_env!({n:?})),\n"));
        }
        script.push_str("    println!(\"cargo:rustc-env=RELAXFIX_NO_VALUE\");\n}\n");
        main.push_str("    ];\n    for (n, compile) in seen {\n        let want = format!(\"relaxfix-{n}\");\n        println!(\"env {n} {} {}\", compile == Some(want.as_str()), std::env::var_os(n).is_some_and(|v| v == want.as_str()));\n    }\n}\n");
        put("a_out_env/build.rs", &script);
        put("a_out_env/src/main.rs", &main.replace("ENV_COUNT", &ENV_NAMES.len().to_string()));
        // a library in the working folder (the run's: the workspace), a copy of a system one
        let (from, file) = cwd_library();
        std::fs::copy(from, ws.join(file)).unwrap();
    }

    /// The store case's workspace on disk, packed into a source store by `run_store`.
    fn store_workspace(&self, vws: &Path) {
        let put = |rel: &str, text: &str| {
            let p = vws.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        put("Cargo.toml", "[workspace]\nresolver = \"2\"\nmembers = [\"a_store\"]\n");
        put(".cargo/config.toml", "[source.crates-io]\nreplace-with = \"v\"\n[source.v]\ndirectory = \"vendor\"\n");
        vendored(vws, "c_store", "", "", STORE, "pub const OUT: &str = env!(\"CASE_OUT\");\n");
        std::os::unix::fs::symlink(&self.secret, vws.join("vendor/c_store/shipped")).unwrap();
        put("a_store/Cargo.toml", &package("a_store", "c_store = \"0.1.0\"\n"));
        put("a_store/src/main.rs", "fn main() {\n    println!(\"{}\", c_store::OUT);\n}\n");
    }
}

fn package(name: &str, deps: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\n{deps}")
}

/// A registry package of the vendored directory with a build script.
fn vendored(ws: &Path, name: &str, extra: &str, build_deps: &str, script: &str, lib: &str) {
    let put = |rel: &str, text: &str| {
        let p = ws.join("vendor").join(name).join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    put("Cargo.toml", &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n{extra}[build-dependencies]\n{build_deps}"));
    put(".cargo-checksum.json", "{\"files\":{},\"package\":null}");
    put("build.rs", script);
    put("src/lib.rs", lib);
}

/// `run --quiet <package>` in the workspace under the policy, without the cache.
fn run(ws: &Path, policy: &str, package: &str, pre: &[&str]) -> Out {
    run_in(ws, policy, package, pre, None, &[])
}

/// `run`, with `home` as Relax's home (a mounted workspace's run dir is there) and `env` set.
fn run_in(ws: &Path, policy: &str, package: &str, pre: &[&str], home: Option<&Path>, env: &[(&str, &str)]) -> Out {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut c = Command::new(std::env::current_exe().unwrap());
    if let Some(h) = home {
        c.env("RELAX_HOME", h);
    }
    c.envs(env.iter().copied());
    let out = c
        .arg("run")
        .args(pre)
        .args(["--quiet", package])
        .current_dir(ws)
        .env(AS_RUNNER, policy)
        .env("RELAX_CACHE", "off")
        .env("CARGO_NET_OFFLINE", "true")
        .env("RELAX_STD", root.join("libs/relax/compiler/std"))
        .env_remove("CARGO")
        // the run dir is the workspace's (`target/relax`), as a person's run has it
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .output()
        .unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// The store case: its workspace packed into a source store (one commit, links as link entries)
/// and run from it (`run --src`), mounted at a folder that is on no disk.
fn run_store(place: &Place, policy: &str) -> Out {
    let t = place.ws.parent().unwrap();
    let store = t.join("vws.store");
    pack(&t.join("vws"), &store);
    let root = t.join("vroot").to_string_lossy().into_owned();
    let mut o = run_in(t, policy, "a_store", &["--root", &root, "--src", &store.to_string_lossy()], Some(&t.join("home")), &[]);
    // (the run dir of a mounted workspace is its own; the scripts' outputs go there)
    o.2.push_str(&format!("\n(store {} mounted at {root})", store.display()));
    o
}

/// A folder as a store of one commit (gitstore.rs): its files as blobs, its symlinks as link
/// entries (mode 120000, the target's text), in git's tree order.
fn pack(dir: &Path, out: &Path) {
    use relax_compiler::gitstore as g;
    fn obj(kind: &str, data: &[u8]) -> g::Oid {
        let mut h = format!("{kind} {}\0", data.len()).into_bytes();
        h.extend_from_slice(data);
        g::sha1(&h)
    }
    fn tree_of(dir: &Path, objects: &mut Vec<(g::Oid, u8, Vec<u8>)>) -> g::Oid {
        let mut entries: Vec<(Vec<u8>, &str, g::Oid)> = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let ft = e.file_type().unwrap();
            let (mode, oid) = if ft.is_symlink() {
                let target = std::fs::read_link(e.path()).unwrap().into_os_string().into_encoded_bytes();
                let o = obj("blob", &target);
                objects.push((o, g::BLOB, target));
                ("120000", o)
            } else if ft.is_dir() {
                ("40000", tree_of(&e.path(), objects))
            } else {
                let data = std::fs::read(e.path()).unwrap();
                let o = obj("blob", &data);
                objects.push((o, g::BLOB, data));
                ("100644", o)
            };
            entries.push((e.file_name().into_encoded_bytes(), mode, oid));
        }
        // git's order: by name, a folder's as if it ended in '/'
        let key = |e: &(Vec<u8>, &str, g::Oid)| {
            let mut k = e.0.clone();
            if e.1 == "40000" {
                k.push(b'/');
            }
            k
        };
        entries.sort_by_key(key);
        let mut t = Vec::new();
        for (name, mode, oid) in &entries {
            t.extend_from_slice(mode.as_bytes());
            t.push(b' ');
            t.extend_from_slice(name);
            t.push(0);
            t.extend_from_slice(oid);
        }
        let o = obj("tree", &t);
        objects.push((o, g::TREE, t));
        o
    }
    let mut objects = Vec::new();
    let root = tree_of(dir, &mut objects);
    let c = format!("tree {}\nauthor a <a> 0 +0000\ncommitter a <a> 0 +0000\n\nfixture\n", g::hex(&root)).into_bytes();
    let commit = obj("commit", &c);
    objects.push((commit, g::COMMIT, c));
    objects.sort_by(|a, b| a.0.cmp(&b.0));
    objects.dedup_by(|a, b| a.0 == b.0);
    g::append(out, g::Append { objects: &objects, reference: g::Ref { key: "fixture".to_string(), commit, time: 1 } }, &|r| r.to_vec()).unwrap();
}

/// The decision's fixtures: the units file the Sandboxed positive run wrote for its script, given
/// to the script's run again as written (it runs the script), without its folders (refused), and
/// with the registry bit cleared (refused, with and without the confined environment).
fn decision(sb: &Place, bad: &mut Vec<String>) {
    let mut found = None;
    let mut stack = vec![sb.ws.join("target")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "units.json") {
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                if text.contains("\"relax_script\"") && text.contains("/c_pos#build-script-build@") {
                    found = Some((p, text));
                }
            }
        }
    }
    let Some((path, text)) = found else {
        bad.push("decision: no units file with a script's folders for c_pos under Sandboxed".to_string());
        return;
    };
    let run_dir = path.parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    let at = text.find(",\"relax_script\"").unwrap();
    let without = format!("{}}}", &text[..at]);
    let path_ids = |t: &str| t.replace("\"registry+file://", "\"path+file://");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out_dir = sb.ws.join("decision-out");
    std::fs::create_dir_all(&out_dir).unwrap();
    let child = |name: &str, units: &str, script_env: bool| -> Out {
        let file = sb.ws.join(format!("decision-{name}.json"));
        std::fs::write(&file, units).unwrap();
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args(["run", "--quiet", "--silent", "--run-dir"])
            .arg(&run_dir)
            .arg("--units")
            .arg(&file)
            .arg("build-script-build")
            .current_dir(sb.ws.join("vendor/c_pos"))
            .env(AS_RUNNER, "sandboxed")
            .env("RELAX_CACHE", "off")
            .env("RELAX_STD", root.join("libs/relax/compiler/std"))
            .env("OUT_DIR", &out_dir)
            .env("RELAX_BUILD_SCRIPT", "1")
            .env("RELAX_SCRIPT_DEPTH", "1")
            .env("CARGO_PKG_NAME", "c_pos")
            .env_remove("CARGO")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_TARGET_DIR");
        if script_env {
            c.env("RELAX_SCRIPT_ENV", "7:OUT_DIR");
        }
        let o = c.output().unwrap();
        (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
    };
    let cases = [
        ("as-written", text.clone(), true, ""),
        ("no-folders", without.clone(), true, "named no folders for it: refused"),
        ("path-ids", path_ids(&text), true, "parent confined its run, and this run finds no registry or git package"),
        ("path-ids-env", path_ids(&without), true, "handed a confined script's environment"),
    ];
    for (name, units, env, says) in cases {
        let o = child(name, &units, env);
        if says.is_empty() {
            // as written: the script runs, confined (given OUT_DIR alone, it stops at its first
            // read of another variable)
            if o.2.contains(": refused") || !o.2.contains("main panic") {
                bad.push(format!("decision {name}: the units file as written did not run the script\nstdout:\n{}\nstderr:\n{}", o.1, o.2));
            }
        } else if o.0 || !o.2.contains(says) {
            bad.push(format!("decision {name}: wanted a refusal saying {says:?}\nstdout:\n{}\nstderr:\n{}", o.1, o.2));
        }
    }
}

const CASES: &[Case] = &[
    Case { name: "soft", kind: Kind::Own, script: SOFT },
    Case { name: "pos", kind: Kind::Own, script: POS },
    Case { name: "store", kind: Kind::Own, script: "" },
    Case { name: "symlink", kind: Kind::Named, script: "fn main() {\n    std::os::unix::fs::symlink(\"SECRET\", \"MARKERS/symlink\").unwrap();\n    println!(\"cargo:rustc-env=CASE_OUT=symlink live\");\n}\n" },
    Case { name: "hard_link", kind: Kind::Named, script: "fn main() {\n    let f = format!(\"{}/f\", std::env::var(\"OUT_DIR\").unwrap());\n    std::fs::write(&f, \"x\").unwrap();\n    std::fs::hard_link(&f, \"MARKERS/hard_link\").unwrap();\n    println!(\"cargo:rustc-env=CASE_OUT=hard_link live\");\n}\n" },
    Case { name: "system", kind: Kind::Named, script: "extern \"C\" {\n    fn system(cmd: *const std::ffi::c_char) -> i32;\n}\nfn main() {\n    unsafe { system(b\"echo x > MARKERS/system\\0\".as_ptr() as *const std::ffi::c_char) };\n    println!(\"cargo:rustc-env=CASE_OUT=system live\");\n}\n" },
    Case { name: "fork", kind: Kind::Named, script: "use std::ffi::c_char;\nextern \"C\" {\n    fn fork() -> i32;\n    fn execv(path: *const c_char, argv: *const *const c_char) -> i32;\n    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;\n    fn _exit(code: i32) -> !;\n}\nfn main() {\n    let argv = [b\"sh\\0\".as_ptr() as *const c_char, b\"-c\\0\".as_ptr() as *const c_char, b\"echo x > MARKERS/fork\\0\".as_ptr() as *const c_char, std::ptr::null()];\n    unsafe {\n        let pid = fork();\n        if pid == 0 {\n            execv(b\"/bin/sh\\0\".as_ptr() as *const c_char, argv.as_ptr());\n            _exit(1);\n        }\n        let mut status = 0;\n        waitpid(pid, &mut status, 0);\n    }\n    println!(\"cargo:rustc-env=CASE_OUT=fork live\");\n}\n" },
    Case { name: "posix_spawn", kind: Kind::Named, script: "use std::ffi::c_char;\nextern \"C\" {\n    fn posix_spawn(pid: *mut i32, path: *const c_char, actions: *const u8, attrs: *const u8, argv: *const *const c_char, envp: *const *const c_char) -> i32;\n    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;\n}\nfn main() {\n    let argv = [b\"sh\\0\".as_ptr() as *const c_char, b\"-c\\0\".as_ptr() as *const c_char, b\"echo x > MARKERS/posix_spawn\\0\".as_ptr() as *const c_char, std::ptr::null()];\n    let envp = [std::ptr::null()];\n    let mut pid = 0;\n    unsafe {\n        posix_spawn(&mut pid, b\"/bin/sh\\0\".as_ptr() as *const c_char, std::ptr::null(), std::ptr::null(), argv.as_ptr(), envp.as_ptr());\n        let mut status = 0;\n        waitpid(pid, &mut status, 0);\n    }\n    println!(\"cargo:rustc-env=CASE_OUT=posix_spawn live\");\n}\n" },
    Case { name: "dlopen", kind: Kind::Named, script: "extern \"C\" {\n    fn dlopen(path: *const std::ffi::c_char, flags: i32) -> *mut u8;\n}\nfn main() {\n    let h = unsafe { dlopen(std::ptr::null(), 2) };\n    println!(\"cargo:rustc-env=CASE_OUT=dlopen {}\", if h.is_null() { \"null\" } else { \"live\" });\n}\n" },
    Case { name: "syscall", kind: Kind::Own, script: "extern \"C\" {\n    fn syscall(n: i64, ...) -> i64;\n}\n#[cfg(all(target_os = \"linux\", target_arch = \"x86_64\"))]\nconst GETPID: i64 = 39;\n#[cfg(all(target_os = \"linux\", target_arch = \"aarch64\"))]\nconst GETPID: i64 = 172;\n#[cfg(target_vendor = \"apple\")]\nconst GETPID: i64 = 20;\nfn main() {\n    let pid = unsafe { syscall(GETPID) };\n    println!(\"cargo:rustc-env=CASE_OUT=syscall {}\", if pid == std::process::id() as i64 { \"live\" } else { \"other\" });\n}\n" },
    Case { name: "thread", kind: Kind::Named, script: "fn main() {\n    let v = std::thread::spawn(|| 7).join().unwrap();\n    println!(\"cargo:rustc-env=CASE_OUT=thread {}\", if v == 7 { \"live\" } else { \"other\" });\n}\n" },
    Case { name: "tcp", kind: Kind::Named, script: "fn main() {\n    let c = std::net::TcpStream::connect((\"127.0.0.1\", PORT)).is_ok();\n    println!(\"cargo:rustc-env=CASE_OUT=tcp {}\", if c { \"live\" } else { \"refused\" });\n}\n" },
    Case { name: "lookup", kind: Kind::Named, script: "use std::net::ToSocketAddrs;\nfn main() {\n    let n = (\"localhost\", 80).to_socket_addrs().map(|a| a.count()).unwrap_or(0);\n    println!(\"cargo:rustc-env=CASE_OUT=lookup {}\", if n > 0 { \"live\" } else { \"none\" });\n}\n" },
    Case { name: "ctor", kind: Kind::Own, script: "extern \"C\" fn ctor() {\n    let _ = std::fs::write(\"MARKERS/ctor\", \"x\");\n}\n#[used]\n#[cfg_attr(target_os = \"linux\", link_section = \".init_array\")]\n#[cfg_attr(target_vendor = \"apple\", link_section = \"__DATA,__mod_init_func\")]\nstatic CTOR: extern \"C\" fn() = ctor;\nfn main() {\n    println!(\"cargo:rustc-env=CASE_OUT=ctor\");\n}\n" },
    Case { name: "wild", kind: Kind::Own, script: "fn main() {\n    unsafe { std::ptr::write_volatile(0x10 as *mut u8, 1) };\n    println!(\"cargo:rustc-env=CASE_OUT=wild survived\");\n}\n" },
    Case { name: "exit3", kind: Kind::Own, script: "fn main() {\n    print!(\"cargo:rustc-env=CASE_OUT=x\");\n    eprintln!(\"exit three\");\n    std::process::exit(3);\n}\n" },
    Case { name: "panic", kind: Kind::Own, script: "fn main() {\n    println!(\"cargo:rustc-env=CASE_OUT=x\");\n    panic!(\"script panicked here\");\n}\n" },
    Case { name: "main_err", kind: Kind::Own, script: "fn main() -> Result<(), String> {\n    println!(\"cargo:rustc-env=CASE_OUT=x\");\n    Err(\"main said no\".to_string())\n}\n" },
    Case { name: "out_search", kind: Kind::Own, script: "" },
    Case { name: "out_lib", kind: Kind::Own, script: "" },
    Case { name: "out_arg", kind: Kind::Own, script: "" },
    Case { name: "out_rerun", kind: Kind::Own, script: "" },
    Case { name: "out_env", kind: Kind::Own, script: "" },
    Case { name: "out_reach", kind: Kind::Own, script: "fn main() {\n    let n = std::fs::read_to_string(\"MARKERS/out_reach\").map(|s| s.len()).unwrap_or(0);\n    let _ = std::fs::write(\"MARKERS/out_reach\", \"x\".repeat(n + 1));\n    println!(\"cargo:rerun-if-changed=OUTSIDE\");\n    println!(\"cargo:rustc-env=CASE_OUT=out_reach\");\n}\n" },
    Case { name: "out_cwdlib", kind: Kind::Own, script: "fn main() {\n    println!(\"cargo:rustc-link-lib=dylib=relaxcwdfix\");\n    println!(\"cargo:rustc-env=CASE_OUT=out_cwdlib\");\n}\n" },
];

/// The output-line cases: what each script prints (MARKERS, SECRET, OUTSIDE, WSTARGET: the
/// policy's), each line refused under Sandboxed. A package of `out_search` and `out_rerun` ships
/// `shipped`, a link to OUTSIDE.
const OUT_LINES: &[(&str, &[&str])] = &[
    ("out_search", &["cargo:rustc-link-search=native=SECRET", "cargo:rustc-link-search=../..", "cargo:rustc-link-search=all=WSTARGET", "cargo:rustc-link-search=native=shipped", "cargo::rustc-flags=-L SECRET"]),
    ("out_lib", &["cargo:rustc-link-lib=dylib=../../relaxfix", "cargo:rustc-link-lib=framework=../../../tmp/relaxfix", "cargo:rustc-link-lib=dylib:+verbatim=/tmp/librelaxfix.so", "cargo:rustc-link-lib=link-arg=-lrelaxfix", "cargo:rustc-flags=-l ../relaxfix", "cargo:rustc-flags=-C relaxfix"]),
    ("out_arg", &["cargo:rustc-link-arg=-Wl,-plugin=SECRET/p.so", "cargo:rustc-link-arg-bins=-fuse-ld=SECRET/ld", "cargo:rustc-link-arg-bin=a_out_arg=-x", "cargo:rustc-link-arg-tests=-x", "cargo:rustc-link-arg-benches=-x", "cargo:rustc-link-arg-examples=-x", "cargo:rustc-link-arg-cdylib=-x", "cargo:rustc-cdylib-link-arg=-x", "cargo::relaxfix-unknown=1"]),
    // (under Open the key walks each: the vendor folder beside the package, a path in the target
    // folder that is not there, so no walk meets the run's own files)
    ("out_rerun", &["cargo:rerun-if-changed=OUTSIDE", "cargo:rerun-if-changed=..", "cargo:rerun-if-changed=WSTARGET/relaxfix-none", "cargo:rerun-if-changed=shipped"]),
];

/// A script that prints `lines`, then its case's line.
fn lines_script(lines: &[&str]) -> String {
    let mut s = String::from("fn main() {\n");
    for l in lines {
        s.push_str(&format!("    println!({l:?});\n"));
    }
    s.push_str("    println!(\"cargo:rustc-env=CASE_OUT=lines printed\");\n}\n");
    s
}

/// One name of each family a confined script's `rustc-env` may not set (relax-base resolve.rs
/// `script_env_refused`), a proxy in lower case too.
const ENV_NAMES: &[&str] = &["CC", "PATH", "LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "RELAX_ALLOW_C", "MAKEPAD_RELAXFIX", "CARGO_HOME", "RUSTC_WRAPPER", "RUSTFLAGS",
    "RUSTDOCFLAGS", "RUSTUP_TOOLCHAIN", "GIT_SSH_COMMAND", "SSH_AUTH_SOCK", "HOME", "TMPDIR", "TMP", "TEMP", "XDG_CONFIG_HOME", "SHELL", "HTTP_PROXY", "https_proxy",
    "ALL_PROXY", "NO_PROXY"];

/// A system library on disk to copy into the working folder as `librelaxcwdfix` (a small one,
/// harmless loaded twice): (its path, the copy's file name).
fn cwd_library() -> (&'static str, &'static str) {
    #[cfg(target_os = "linux")]
    let (candidates, file) = (["/usr/lib/libdl.so.2", "/lib/x86_64-linux-gnu/libdl.so.2", "/usr/lib/x86_64-linux-gnu/libdl.so.2", "/lib64/libdl.so.2", "/usr/lib64/libdl.so.2", "/lib/aarch64-linux-gnu/libdl.so.2", "/usr/lib/aarch64-linux-gnu/libdl.so.2"].as_slice(), "librelaxcwdfix.so");
    #[cfg(target_vendor = "apple")]
    let (candidates, file) = (["/usr/lib/libpcre2-8.dylib", "/usr/lib/libpcre2-posix.dylib", "/usr/lib/libffi-trampolines.dylib"].as_slice(), "librelaxcwdfix.dylib");
    let from = candidates.iter().find(|c| Path::new(c).is_file()).unwrap_or_else(|| panic!("confined_scripts: no system library on disk to copy among {candidates:?}"));
    (from, file)
}

/// The cwdlib case's program: whether the working folder's library is loaded in this process.
const CWDLIB_MAIN: &str = r#"#[cfg(target_os = "linux")]
fn loaded() -> bool {
    std::fs::read_to_string("/proc/self/maps").map(|m| m.contains("librelaxcwdfix.so")).unwrap_or(false)
}
#[cfg(target_vendor = "apple")]
fn loaded() -> bool {
    extern "C" {
        fn _dyld_image_count() -> u32;
        fn _dyld_get_image_name(i: u32) -> *const std::ffi::c_char;
    }
    (0..unsafe { _dyld_image_count() }).any(|i| {
        let p = unsafe { _dyld_get_image_name(i) };
        !p.is_null() && unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().ends_with("librelaxcwdfix.dylib")
    })
}
fn main() {
    println!("{}", c_out_cwdlib::OUT);
    println!("cwdlib {}", if loaded() { "loaded" } else { "not loaded" });
}
"#;

/// Escapes a script meets as ordinary errors (`soft`): `<name>=ok` when it acted, else the error's
/// kind.
const SOFT: &str = r#"fn r<T>(x: std::io::Result<T>) -> String {
    match x {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("{:?}", e.kind()),
    }
}
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let dir = env!("CARGO_MANIFEST_DIR");
    let mut res = Vec::new();
    res.push(("abs", r(std::fs::write("MARKERS/abs", "x"))));
    res.push(("own", r(std::fs::write(format!("{dir}/own.txt"), "x"))));
    res.push(("dotdot", r(std::fs::write(format!("{out}/../../dotdot"), "x"))));
    res.push(("link_read", r(std::fs::read_to_string(format!("{dir}/shipped/secret-entry")))));
    res.push(("link_write", r(std::fs::write(format!("{dir}/shipped/through"), "x"))));
    std::fs::write(format!("{out}/r"), "x").unwrap();
    res.push(("rename", r(std::fs::rename(format!("{out}/r"), "MARKERS/renamed"))));
    res.push(("remove", r(std::fs::remove_dir_all("VICTIM"))));
    res.push(("chdir", r(std::env::set_current_dir("MARKERS"))));
    res.push(("rel", r(std::fs::write("cwd", "x"))));
    res.push(("devfd", r(std::fs::metadata("/dev/fd/0"))));
    res.push(("hidden_read", r(std::fs::read_to_string("SECRET/secret-entry"))));
    res.push(("sh", r(std::process::Command::new("sh").args(["-c", "echo x > MARKERS/sh"]).status())));
    let rustc = std::env::var("RUSTC").unwrap();
    res.push(("rustc_run", r(std::process::Command::new(&rustc).args(["run", "--help"]).output())));
    res.push(("cargo", r(std::process::Command::new(std::env::var("CARGO").unwrap()).arg("metadata").output())));
    res.push(("run_dir", r(std::fs::read_dir(format!("{out}/..")))));
    let line: Vec<String> = res.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!("cargo:rustc-env=CASE_OUT={}", line.join(" "));
}
"#;

/// The nested case's build-dependency: its own script escapes.
const NESTED_DEP: &str = r#"fn main() {
    let r = match std::fs::write("MARKERS/nested", "x") {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("{:?}", e.kind()),
    };
    println!("cargo:rustc-env=NESTED={r}");
}
"#;

/// The closure rule's path script: its registry build-dependency escapes; it reads its
/// workspace, and the target folder three ways.
const PATHY: &str = r#"fn r<T>(x: std::io::Result<T>) -> String {
    match x {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("{:?}", e.kind()),
    }
}
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let ws = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let mut res = Vec::new();
    res.push(("helper", c_reghelp::escape("MARKERS/helper")));
    res.push(("ws_read", r(std::fs::read_to_string(format!("{ws}/Cargo.toml")))));
    res.push(("target_direct", r(std::fs::read_dir(format!("{ws}/target")))));
    res.push(("target_link", r(std::fs::read_dir(format!("{ws}/tlink")))));
    res.push(("target_up", r(std::fs::read_dir(format!("{out}/../..")))));
    res.push(("out_rw", r(std::fs::write(format!("{out}/f"), "x").and_then(|_| std::fs::read(format!("{out}/f"))))));
    let line: Vec<String> = res.iter().map(|(k, v)| format!("{k}={v}")).collect();
    println!("cargo:rustc-env=PATHY={}", line.join(" "));
}
"#;

/// The ecosystem's script shapes (`pos`): the same lines under both policies.
const POS: &str = r#"use std::io::Write;
use std::process::{Command, Stdio};
extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}
extern "C" fn handler() {
    // (under makepad it runs in the script's arena: its write outside the roots is refused)
    let _ = std::fs::write("MARKERS/atexit", "x");
    println!("\ncargo:rustc-env=POS_ATEXIT=exit handler ran");
}
fn main() {
    unsafe { atexit(handler) };
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("name {}", std::env::var("CARGO_PKG_NAME").unwrap()));
    lines.push(format!("listed {}", std::env::vars().any(|(k, _)| k == "CARGO_PKG_VERSION")));
    // the version probe: its manifest, `$RUSTC --version` and `-vV`
    let toml = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap();
    lines.push(format!("toml {}", toml.lines().next().unwrap()));
    let rustc = std::env::var("RUSTC").unwrap();
    let v = Command::new(&rustc).arg("--version").output().unwrap();
    lines.push(format!("version {} {}", v.status.success(), String::from_utf8_lossy(&v.stdout).starts_with("rustc ")));
    let vv = Command::new(&rustc).arg("-vV").output().unwrap();
    lines.push(format!("vV host {}", String::from_utf8_lossy(&vv.stdout).lines().any(|l| l.starts_with("host: "))));
    // stdin inherited: the answer reads none
    let i = Command::new(&rustc).arg("--version").stdin(Stdio::inherit()).output().unwrap();
    lines.push(format!("inherit {}", i.status.success()));
    // thiserror: a private file, a probe folder made and removed, a compile probe that fails
    std::fs::write(out.join("private.rs"), "pub const P: u8 = 1;\n").unwrap();
    std::fs::create_dir(out.join("probe-dir")).unwrap();
    std::fs::remove_dir(out.join("probe-dir")).unwrap();
    std::fs::write(out.join("probe.rs"), "pub fn probe() {}\n").unwrap();
    let probe = Command::new(&rustc).args(["--crate-type", "lib", "--emit", "metadata", "--out-dir"]).arg(out.join("probe-out")).arg(out.join("probe.rs")).stderr(Stdio::null()).status().unwrap();
    lines.push(format!("probe {:?} out {}", probe.code(), out.join("probe-out").exists()));
    // questions that name files: nothing read, nothing written
    let target = Command::new(&rustc).args(["--print", "cfg", "--target", "SECRET/secret-entry"]).output().unwrap();
    lines.push(format!("target file {:?} {}", target.status.code(), String::from_utf8_lossy(&target.stdout).contains("secret")));
    let at = Command::new(&rustc).args(["--print", "cfg", "@SECRET/secret-entry"]).output().unwrap();
    lines.push(format!("at file {}", String::from_utf8_lossy(&at.stdout).contains("secret")));
    let o = Command::new(&rustc).args(["--crate-type", "lib", "-o", "MARKERS/o-out"]).arg(out.join("probe.rs")).stderr(Stdio::null()).status().unwrap();
    lines.push(format!("o {:?}", o.code()));
    // typenum: a large generated file
    let mut f = std::io::BufWriter::new(std::fs::File::create(out.join("big.rs")).unwrap());
    for i in 0..20000 {
        writeln!(f, "pub const C{i}: u32 = {i};").unwrap();
    }
    drop(f);
    lines.push(format!("big {}", std::fs::metadata(out.join("big.rs")).unwrap().len()));
    // cfg and feature lines
    println!("cargo:rustc-cfg=pos_os_{}", std::env::var("CARGO_CFG_TARGET_OS").unwrap());
    lines.push(format!("feature {}", std::env::var_os("CARGO_FEATURE_EXTRA").is_some()));
    // a 255-byte name, listed
    let long = "n".repeat(255);
    std::fs::write(out.join(&long), "").unwrap();
    lines.push(format!("long {}", std::fs::read_dir(&out).unwrap().any(|e| e.unwrap().file_name().len() == 255)));
    // canonicalize(OUT_DIR) stays in OUT_DIR
    let c = std::fs::canonicalize(&out).unwrap();
    lines.push(format!("canon {}", c.join("private.rs").is_file()));
    // an ancestor walk for a workspace: it ends, found or not
    let mut d = out.as_path();
    while let Some(p) = d.parent() {
        let _ = p.join("Cargo.toml").is_file();
        d = p;
    }
    lines.push("walk done".to_string());
    // output lines a confined script may print: its own folders, a plain system library, its own
    // variable, its inputs, cfg checks, metadata, a warning
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-search={}", env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rustc-link-search=src");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-flags=-l m -L {}", out.display());
    println!("cargo:rustc-env=POS_PLAIN=plain");
    std::fs::write(out.join("stamp"), "s").unwrap();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed={}", out.join("stamp").display());
    println!("cargo::rustc-check-cfg=cfg(pos_os_linux, pos_os_macos)");
    println!("cargo::metadata=pos=1");
    println!("cargo:poskey=1");
    println!("cargo:warning=pos warns");
    println!("cargo:rustc-env=CASE_OUT={}", lines.join("|"));
    // the tail: no newline, then exit (the handler's line follows it)
    print!("cargo:rustc-env=POS_TAIL=tail");
    std::process::exit(0);
}
"#;

/// The store case's script: its own source, read, listed and probed, also from its virtual
/// current folder; a link entry it ships; the workspace beside its folder.
const STORE: &str = r#"fn r<T>(x: std::io::Result<T>) -> String {
    match x {
        Ok(_) => "ok".to_string(),
        Err(e) => format!("{:?}", e.kind()),
    }
}
fn main() {
    let dir = env!("CARGO_MANIFEST_DIR");
    let mut res = Vec::new();
    res.push(format!("manifest {}", std::fs::read_to_string(format!("{dir}/Cargo.toml")).map(|t| t.lines().count()).unwrap_or(0)));
    let mut names: Vec<String> = std::fs::read_dir(format!("{dir}/src")).map(|d| d.map(|e| e.unwrap().file_name().into_string().unwrap()).collect()).unwrap_or_default();
    names.sort();
    res.push(format!("listing {}", names.join(",")));
    res.push(format!("relative {}", r(std::fs::read_to_string("src/lib.rs"))));
    res.push(format!("meta {}", std::fs::metadata(format!("{dir}/src/lib.rs")).map(|m| m.is_file()).unwrap_or(false)));
    res.push(format!("link {}", r(std::fs::read_to_string(format!("{dir}/shipped/secret-entry")))));
    res.push(format!("up {}", r(std::fs::read_to_string(format!("{dir}/../../Cargo.toml")))));
    println!("cargo:rustc-env=CASE_OUT={}", res.join("|"));
}
"#;
