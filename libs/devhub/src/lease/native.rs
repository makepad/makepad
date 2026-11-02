//! JIT first: lanes iterate and check under the JIT (`devhub live start <app> --visible` and
//! `live look`, `devhub test <lane> <pkg> [--live]`). A lane's lease command that builds a package
//! with cargo (build, run, test, bench) is refused, except:
//! - the JIT's own crates ([`crate::jit_misses::jit_own`]);
//! - a package with an open JIT miss (the run names it);
//! - a platform without a JIT: no JIT leg ([`crate::orch::verify::legs`]) runs on its os and arch;
//! - a test devhub's measured routing sends to cargo (`native:<class>`, the JIT measured slower end
//!   to end: [`super::master::LeaseMaster::test_route`]);
//! - `--native "<reason>"`: native-vs-JIT baselines and timed perf series.
//!
//! Every verdict but the JIT's own crates is a line of `native.jsonl` (what `devhub prof` counts
//! per lane). CI, gates, inspect leases and devhub's own lanes (landings, buckets, prebuilds, legs,
//! shows, live sessions) keep their own rules.

use super::sched::NodeView;
use super::Kind;
use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

pub const LEDGER: &str = "native.jsonl";

/// A command's words, with `;` for each command separator (`;`, `|`, `&`, a newline, a paren).
/// Quotes, brackets and commas only separate words: `bash -c '…'` texts and Python argv lists
/// read like shell lines.
fn words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut chars = text.chars().peekable();
    let flush = |word: &mut String, out: &mut Vec<String>| if !word.is_empty() { out.push(std::mem::take(word)); };
    while let Some(c) = chars.next() {
        match c {
            '#' if word.is_empty() => {
                while chars.next_if(|c| *c != '\n').is_some() {}
            }
            '\\' if chars.peek() == Some(&'\n') => { chars.next(); flush(&mut word, &mut out); }
            ';' | '|' | '&' | '\n' | '(' | ')' => {
                flush(&mut word, &mut out);
                if out.last().is_some_and(|w| w != ";") { out.push(";".into()); }
            }
            c if c.is_whitespace() || "'\"`[]{},\\".contains(c) => flush(&mut word, &mut out),
            c => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

/// One cargo build in a command.
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    /// `-p`'s package; `*`: none named (the package of the command's dir, else the workspace's)
    pub pkg: String,
    /// build, run, test or bench
    pub verb: &'static str,
    /// `--target`'s triple; empty: the host's
    pub target: String,
}

/// What `cmd` builds with cargo: each `cargo build|run|test|bench` (never check, clippy or doc)
/// in its words, a `-c` text, or a `.sh`/`.py` script it names, read with `script` (one level).
pub fn builds(cmd: &[String], script: &dyn Fn(&str) -> Option<String>) -> Vec<Built> {
    let mut text = cmd.join(" ");
    for w in cmd.iter().filter(|w| w.ends_with(".sh") || w.ends_with(".py")) {
        if let Some(t) = script(w) { text.push('\n'); text.push_str(&t); }
    }
    let w = words(&text);
    let mut out: Vec<Built> = Vec::new();
    let mut i = 0;
    while i < w.len() {
        if w[i] != "cargo" && !w[i].ends_with("/cargo") { i += 1; continue; }
        let mut j = i + 1;
        while w.get(j).is_some_and(|x| x.starts_with('+') || matches!(x.as_str(), "-q" | "--quiet" | "-v" | "-vv" | "--locked" | "--frozen" | "--offline")) { j += 1; }
        let verb = match w.get(j).map(String::as_str) {
            Some("build" | "b") => "build",
            Some("run" | "r") => "run",
            Some("test" | "t") => "test",
            Some("bench") => "bench",
            _ => { i = j; continue; }
        };
        let (mut pkgs, mut target) = (Vec::new(), String::new());
        let mut k = j + 1;
        while k < w.len() && w[k] != ";" && w[k] != "--" {
            let x = w[k].as_str();
            if (x == "-p" || x == "--package" || x == "--target") && k + 1 < w.len() {
                if x == "--target" { target = w[k + 1].clone(); } else { pkgs.push(w[k + 1].clone()); }
                k += 1;
            } else if let Some(t) = x.strip_prefix("--target=") {
                target = t.to_string();
            } else if let Some(p) = x.strip_prefix("--package=").or_else(|| x.strip_prefix("-p").filter(|p| !p.is_empty() && !p.starts_with('-'))) {
                pkgs.push(p.to_string());
            }
            k += 1;
        }
        if pkgs.is_empty() { pkgs.push("*".into()); }
        for pkg in pkgs {
            let b = Built { pkg, verb, target: target.clone() };
            if !out.contains(&b) { out.push(b); }
        }
        i = k;
    }
    out
}

/// The package whose manifest is in `dir` (`[package] name`).
fn package_at(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') { in_package = line == "[package]"; continue; }
        let Some(v) = line.strip_prefix("name").map(str::trim_start).and_then(|v| v.strip_prefix('=')).filter(|_| in_package) else { continue };
        return Some(v.trim().trim_matches('"').to_string());
    }
    None
}

/// devhub's own lanes and agents run what they must (CI, landing, buckets, prebuilds, legs,
/// shows, live tests).
fn exempt_lane(lane: &str, agent: &str) -> bool {
    matches!(lane, super::sched::CI_LANE | super::sched::LANDING_LANE)
        || ["landq", "jitlegs", super::WARM_LANE_PREFIX, "show-", "live-"].iter().any(|p| lane.starts_with(p))
        || agent.starts_with("devhub-")
}

/// `x86_64` and `x64` alike: the names `--needs arch=` takes.
fn arch_word(arch: &str) -> String {
    match arch.to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => "x64".into(),
        "aarch64" => "arm64".into(),
        a => a.into(),
    }
}

/// The platform a lease runs on, (os, arch): its named node's, else its needs' (the pool's
/// default: x64 Linux; an arm64 Mac for `os=macos`).
pub fn platform(needs: &[(String, String)], node: &str, nodes: &[NodeView]) -> (String, String) {
    if let Some(c) = nodes.iter().find(|n| n.name == node).and_then(|n| n.caps.as_ref()) {
        return (c.os.to_ascii_lowercase(), arch_word(&c.arch));
    }
    let need = |k: &str| needs.iter().find(|(n, _)| n == k).map(|(_, v)| v.to_ascii_lowercase());
    let os = need("os").unwrap_or_else(|| "linux".into());
    let arch = need("arch").map(|a| arch_word(&a)).unwrap_or_else(|| if os == "macos" { "arm64" } else { "x64" }.into());
    (os, arch)
}

/// A cargo `--target`'s platform: wasm, iOS and the like name their own os, which no JIT runs on.
fn target_platform(triple: &str) -> (String, String) {
    let os = ["android", "windows", "darwin", "linux"].into_iter().find(|o| triple.contains(o)).map_or(triple, |o| if o == "darwin" { "macos" } else { o });
    (os.to_string(), arch_word(triple.split('-').next().unwrap_or("")))
}

/// Whether the JIT runs on this platform: a JIT leg verifies it.
fn jit_runs(os: &str, arch: &str) -> bool {
    crate::orch::verify::legs(true).iter().any(|leg| {
        let needs = super::sched::parse_needs(&leg.needs);
        needs.iter().any(|(k, v)| k == "os" && v.eq_ignore_ascii_case(os)) && needs.iter().any(|(k, v)| k == "arch" && arch_word(v) == arch)
    })
}

/// The JIT command for this package instead of its native build.
fn hint(lane: &str, b: &Built) -> String {
    let pkg = if b.pkg == "*" { "<pkg>" } else { &b.pkg };
    if matches!(b.verb, "test" | "bench") {
        format!("`devhub test {lane} {pkg} --live` while iterating, `devhub test {lane} {pkg}` for the final check")
    } else {
        format!("`devhub live start {pkg} --visible`, then `devhub live look <id>` after each save")
    }
}

/// One gate question: a lane's command and where it runs.
pub struct Ask<'a> {
    pub lane: &'a str,
    pub agent: &'a str,
    pub kind: Kind,
    pub cmd: &'a [String],
    /// the lane's worktree and the command's dir in it
    pub repo_dir: &'a Path,
    pub cwd: &'a str,
    /// `<branch>@<sha>[,…]`: the commits it runs (empty: the worktree's HEAD)
    pub rev: &'a str,
    /// `--native "<reason>"`
    pub reason: &'a str,
    /// the lease's class (`native:test:<pkg>[:<target>]` for a native test)
    pub class: &'a str,
    /// devhub's measured routing for a JIT test class: Some(why, both walls) when it picks cargo
    pub route: &'a dyn Fn(&str) -> Option<String>,
    /// [`platform`]
    pub os: &'a str,
    pub arch: &'a str,
}

/// The commits a lane's command runs: its pin, else its worktree's HEAD.
fn pair_of(repo_dir: &Path, rev: &str) -> Result<crate::state::Pair, String> {
    if let Some(pin) = super::Pin::of_rev(rev) {
        return Ok(crate::state::Pair::new(&pin.makepad, pin.commercial.as_deref().unwrap_or("")));
    }
    let o = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(repo_dir).output().map_err(|e| format!("git: {e}"))?;
    Ok(crate::state::Pair::new(String::from_utf8_lossy(&o.stdout).trim(), ""))
}

/// The rule (master only). Ok: the command may run, with a line to print (empty: nothing gated);
/// Err: refused, saying what to do instead.
pub fn check(home: &Path, a: &Ask, now: u64) -> Result<String, String> {
    // the JIT's own invocation (`cargo run --profile host -p makepad-run --bin makepad -- ..`)
    if matches!(a.kind, Kind::Inspect | Kind::Gate | Kind::Ci) || exempt_lane(a.lane, a.agent) || super::loaders::cargo_run_args(a.cmd).is_some() { return Ok(String::new()); }
    let dir = a.repo_dir.join(a.cwd);
    let read = |p: &str| -> Option<String> {
        let p = Path::new(p);
        if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) { return None; }
        std::fs::read_to_string(dir.join(p)).ok()
    };
    let mut built = builds(a.cmd, &read);
    if built.is_empty() { return Ok(String::new()); }
    if let Some(here) = package_at(&dir) {
        for b in built.iter_mut().filter(|b| b.pkg == "*") { b.pkg = here.clone(); }
    }
    let on_jit = |b: &Built| if b.target.is_empty() { jit_runs(a.os, a.arch) } else { let (os, arch) = target_platform(&b.target); jit_runs(&os, &arch) };
    if built.iter().any(on_jit) {
        let pair = pair_of(a.repo_dir, a.rev)?;
        let mut own = BTreeSet::new();
        for b in &built {
            if on_jit(b) && !own.contains(&b.pkg) && crate::jit_misses::jit_own(a.repo_dir, &pair, &b.pkg)? { own.insert(b.pkg.clone()); }
        }
        built.retain(|b| !(on_jit(b) && own.contains(&b.pkg)));
        if built.is_empty() { return Ok(String::new()); }
    }
    let (gated, elsewhere): (Vec<Built>, Vec<Built>) = built.into_iter().partition(on_jit);
    verdict(home, a, &gated, &elsewhere, now)
}

/// The verdict on what the command builds natively on a JIT platform (`gated`) and elsewhere,
/// appended to the ledger.
fn verdict(home: &Path, a: &Ask, gated: &[Built], elsewhere: &[Built], now: u64) -> Result<String, String> {
    let names = |bs: &[Built]| bs.iter().map(|b| b.pkg.clone()).fold(Vec::<String>::new(), |mut v, p| { if !v.contains(&p) { v.push(p); } v });
    let reason = a.reason.trim();
    let mut miss = None;
    let (word, pkgs, answer) = if gated.is_empty() {
        let pkgs = names(elsewhere);
        let b = &elsewhere[0];
        let on = if b.target.is_empty() { format!("{} {}", a.os, a.arch) } else { b.target.clone() };
        let answer = Ok(format!("devhub: native build of {} for {on}: no JIT runs there (no JIT leg verifies it), so cargo is the way", pkgs.join(", ")));
        ("platform", pkgs, answer)
    } else {
        let pkgs = names(gated);
        let list = pkgs.join(", ");
        miss = pkgs.iter().find_map(|p| crate::jit_misses::open(home, p).map(|id| (p.clone(), id)));
        // the class names the package it measures: `test:<pkg>[:<target>]`
        let routed = a.class.strip_prefix("native:").filter(|c| a.kind == Kind::Test && c.split(':').nth(1).is_some_and(|p| pkgs.iter().all(|q| q == p))).and_then(|c| (a.route)(c));
        let (word, answer) = if let Some((pkg, id)) = &miss {
            ("miss", Ok(format!("devhub: native build of {list}: {pkg} has the open JIT miss {id}, so cargo is the way until it closes")))
        } else if let Some(why) = &routed {
            ("measured", Ok(format!("devhub: native test of {list}: devhub's measured routing picked {why}")))
        } else if !reason.is_empty() {
            ("reason", Ok(format!("devhub: native build of {list} (--native \"{reason}\", logged)")))
        } else {
            ("refused", Err(format!("native build of {list} refused: lanes iterate and check under the JIT: {}. Native runs only for the JIT's own crates, a package with an open JIT miss, a platform without a JIT, a test devhub's measured routing sends to cargo, or with --native \"<reason>\" (a native-vs-JIT baseline, a timed perf series; logged)", hint(a.lane, &gated[0]))))
        };
        (word, pkgs, answer)
    };
    let line = format!("{{\"ms\":{now},\"lane\":{},\"agent\":{},\"kind\":{},\"pkgs\":[{}],\"platform\":{},\"verdict\":{},\"reason\":{},\"class\":{},\"miss\":{},\"cmd\":{}}}\n",
        json_str(a.lane), json_str(a.agent), json_str(a.kind.as_str()), pkgs.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), json_str(&format!("{} {}", a.os, a.arch)),
        json_str(word), json_str(reason), json_str(a.class), json_str(miss.as_ref().map_or("", |(_, id)| id.as_str())), json_str(&crate::proto::clean(&a.cmd.join(" "), 200)));
    use std::io::Write;
    let path = home.join(LEDGER);
    std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| f.write_all(line.as_bytes())).map_err(|e| format!("{}: {e}", path.display()))?;
    answer
}

/// `native check {"lane","agent","kind","cmd":[..],"repo_dir","cwd","rev","reason","class","needs","node"}`
/// → `{"note":".."}`, or the refusal as the error.
pub fn serve(home: &Path, line: &str, now: u64, nodes: &[NodeView], route: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let body = line.strip_prefix("check ").ok_or("native check {json}")?;
    let v = makepad_strict_json::parse_depth(body.as_bytes(), 8).map_err(|_| "native check: bad request")?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let kind = Kind::parse(&s("kind")).ok_or("native check: kind")?;
    let cmd: Vec<String> = v.get("cmd").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect();
    // a lease asked outside a worktree: its words alone, against the master's checkout
    let repo_dir = Some(std::path::PathBuf::from(s("repo_dir"))).filter(|d| d.join(".git").exists()).unwrap_or_else(crate::orch::repo);
    let (lane, agent, cwd, rev, reason, class) = (s("lane"), s("agent"), s("cwd"), s("rev"), s("reason"), s("class"));
    let (os, arch) = platform(&super::sched::parse_needs(&s("needs")), &s("node"), nodes);
    let ask = Ask { lane: &lane, agent: &agent, kind, cmd: &cmd, repo_dir: &repo_dir, cwd: if cwd.is_empty() { "." } else { &cwd }, rev: &rev, reason: &reason, class: &class, route, os: &os, arch: &arch };
    let note = check(home, &ask, now)?;
    Ok(format!("{{\"note\":{}}}", json_str(&note)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> { s.split(' ').map(str::to_string).collect() }

    fn built(pkg: &str, verb: &'static str, target: &str) -> Built { Built { pkg: pkg.into(), verb, target: target.into() } }

    #[test]
    fn refused_unless_a_miss_a_route_a_reason_or_no_jit() {
        let home = std::env::temp_dir().join(format!("devhub-native-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let cmd = argv("cargo test --release -p fab-model");
        let route = |class: &str| (class == "test:fab-model:lib").then(|| "cargo (cargo 2m, JIT 4m)".to_string());
        let ask = |reason, class, os| Ask { lane: "l1", agent: "", kind: Kind::Test, cmd: &cmd, repo_dir: &home, cwd: ".", rev: "", reason, class, route: &route, os, arch: "x64" };
        let test = [built("fab-model", "test", "")];
        let refused = verdict(&home, &ask("", "", "linux"), &test, &[], 1).unwrap_err();
        assert!(refused.contains("`devhub test l1 fab-model --live`") && refused.contains("--native"), "{refused}");
        assert!(verdict(&home, &ask("perf baseline", "", "linux"), &test, &[], 2).unwrap().contains("perf baseline"));
        assert!(verdict(&home, &ask("", "native:test:other:lib", "linux"), &test, &[], 3).is_err(), "another package's class routes nothing");
        assert!(verdict(&home, &ask("", "native:test:fab-model:lib", "linux"), &test, &[], 3).unwrap().contains("cargo 2m, JIT 4m"));
        assert!(verdict(&home, &ask("", "", "windows"), &[], &test, 4).unwrap().contains("for windows x64: no JIT runs there"));
        assert!(verdict(&home, &ask("", "", "linux"), &[], &[built("app", "build", "wasm32-unknown-unknown")], 5).unwrap().contains("for wasm32-unknown-unknown: no JIT runs there"));
        std::fs::write(home.join("jit_misses.jsonl"), "{\"id\":\"M7\",\"pkg\":\"fab-model\",\"target\":\"--lib\",\"error\":[\"e\"],\"closed_ms\":0}\n").unwrap();
        assert!(verdict(&home, &ask("", "", "linux"), &test, &[], 6).unwrap().contains("open JIT miss M7"));
        let ledger = std::fs::read_to_string(home.join(LEDGER)).unwrap();
        let verdicts: Vec<&str> = ledger.lines().map(|l| l.split("\"verdict\":\"").nth(1).unwrap().split('"').next().unwrap()).collect();
        assert_eq!(verdicts, ["refused", "reason", "refused", "measured", "platform", "platform", "miss"]);
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn platforms_from_needs_targets_and_the_jit_legs() {
        let p = |needs: &str| platform(&super::super::sched::parse_needs(needs), "", &[]);
        assert_eq!(p(""), ("linux".to_string(), "x64".to_string()));
        assert_eq!(p("os=macos"), ("macos".to_string(), "arm64".to_string()));
        assert_eq!(p("arch=x86_64,os=windows"), ("windows".to_string(), "x64".to_string()));
        assert_eq!(target_platform("aarch64-linux-android"), ("android".to_string(), "arm64".to_string()));
        assert_eq!(target_platform("x86_64-pc-windows-msvc"), ("windows".to_string(), "x64".to_string()));
        assert!(jit_runs("linux", "x64") && jit_runs("macos", "arm64"));
        assert!(!jit_runs("windows", "x64") && !jit_runs("wasm32-unknown-unknown", "wasm32"));
    }

    #[test]
    fn builds_reads_words_texts_and_scripts() {
        let none = |_: &str| None;
        let b = |cmd: &str| builds(&argv(cmd), &none);
        assert_eq!(b("cargo run --release -p makepad-example-fab -- --flag -p x"), vec![built("makepad-example-fab", "run", "")]);
        assert_eq!(b("env A=1 nice -n 10 cargo +nightly -q test --release --package=a -pb"), vec![built("a", "test", ""), built("b", "test", "")]);
        assert!(b("cargo check --release -p a").is_empty());
        assert_eq!(b("cargo build --release --target wasm32-unknown-unknown"), vec![built("*", "build", "wasm32-unknown-unknown")]);
        let script = |p: &str| (p == "drive.sh").then(|| "#!/bin/bash\n# cargo build -p commented\ncargo build --release \\\n  -q -p makepad-run --bin makepad || exit 1\nM=x; cargo run -p app; ./x -p y\n".to_string());
        assert_eq!(builds(&argv("bash drive.sh"), &script), vec![built("makepad-run", "build", ""), built("app", "run", "")]);
        let py = |_: &str| Some("subprocess.run([\"cargo\", \"test\", \"--release\", \"-p\", \"fab-model\"], check=True)".to_string());
        assert_eq!(builds(&argv("python3 t.py"), &py), vec![built("fab-model", "test", "")]);
    }
}
