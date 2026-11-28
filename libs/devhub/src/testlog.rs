//! What a test run's output says it ran: libtest's `test result:` lines (cargo's and the JIT
//! harness's) and the `harness = false` targets (the JIT names them; under cargo, a target that
//! ran and said nothing of libtest's is one). A run that executed no test is no pass (a filter
//! that matched nothing passed silently, 11-03).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use makepad_toml_parser::{parse_toml, Toml};

/// The tally of one run's output, fed line by line (a stream) or whole.
#[derive(Default, Debug, PartialEq)]
pub struct Tally {
    /// `test result:` lines seen
    pub results: u64,
    /// tests that ran: passed + failed over those lines
    pub executed: u64,
    /// `harness = false` targets that ran (programs of their own: what they test is unknown)
    pub plain: u64,
    /// a target cargo said it runs (`Running <target> (<binary>)`) that said nothing of libtest's
    /// yet: a `harness = false` program, once the next target starts or the output ends (cargo
    /// does not name them: a scope test that printed no `test result:` was "no test ran", 11-07)
    quiet_target: bool,
    partial: Vec<u8>,
    /// the targets whose `test result:` said FAILED, in the order they ran ([`Self::failed_said`])
    pub failed: Vec<FailedTarget>,
    /// the target running now (`Running <target>`): its failed tests and first panics so far
    current: FailedTarget,
    /// `current` said `Running` and not yet its `test result:`
    open: bool,
    /// the line before was a panic's `panicked at <where>:`, its message on this one
    panic_at: Option<String>,
}

/// A target that failed: its name as cargo and the JIT say it runs (`tests/chrome.rs`, empty for
/// a run of one target), its failed tests, and the first lines of its panics, each `panicked at
/// <where>: <message>` once per place.
#[derive(Default, Debug, PartialEq, Clone)]
pub struct FailedTarget {
    pub target: String,
    pub tests: Vec<String>,
    pub errors: Vec<String>,
}

/// How many of a failed target's panics, and of its failed tests, its verdict line names; the
/// whole run's output says the rest (the lease's or job's kept log).
const SAID_ERRORS: usize = 2;
const SAID_TESTS: usize = 5;

impl Tally {
    pub fn of(log: &str) -> Tally {
        let mut t = Tally::default();
        t.feed(log.as_bytes());
        t.feed(b"\n");
        t
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b != b'\n' {
                // a summary line is short; a long line is no summary
                if self.partial.len() < 512 { self.partial.push(b); }
                continue;
            }
            let line = std::mem::take(&mut self.partial);
            self.line(String::from_utf8_lossy(&line).trim());
        }
    }

    fn line(&mut self, line: &str) {
        if let Some(at) = self.panic_at.take() { self.panicked(at, line); }
        if line.starts_with("Running ") || line.starts_with("Doc-tests ") {
            let name = line.split_once(' ').map_or("", |(_, rest)| rest);
            let target = name.split(" (").next().unwrap_or(name).trim();
            // the open target's own processes (the tests that start its binary again) say its
            // `Running` line again: still that target, its failures so far kept (a devhub lib run
            // said its failed target only `failed`, 11-20)
            if self.open && self.current.target == target { return; }
            self.open = true;
            self.current = FailedTarget { target: target.to_string(), ..Default::default() };
            self.plain += u64::from(std::mem::take(&mut self.quiet_target));
            if line.ends_with("(harness = false)") {
                self.plain += 1;
            } else {
                self.quiet_target = line.starts_with("Running ");
            }
            return;
        }
        // libtest's first line of a target (`running 3 tests`, `running 1 test`)
        if line.strip_prefix("running ").and_then(|r| r.strip_suffix(" tests").or_else(|| r.strip_suffix(" test"))).is_some_and(|n| n.parse::<u64>().is_ok()) {
            self.quiet_target = false;
            return;
        }
        if let Some(name) = line.strip_prefix("test ").and_then(|t| t.strip_suffix(" ... FAILED")) {
            self.current.tests.push(name.to_string());
            return;
        }
        // `thread '<test>' (<tid>) panicked at <where>:`, its message on the next line (older
        // libtest: on the same line)
        if let Some(at) = line.find("panicked at ") {
            let said = &line[at..];
            match said.strip_suffix(':') {
                Some(at) => self.panic_at = Some(at.to_string()),
                None => self.panicked(said.to_string(), ""),
            }
            return;
        }
        let Some(rest) = line.strip_prefix("test result: ") else { return };
        self.quiet_target = false;
        self.open = false;
        let count = |word: &str| rest.split([';', '.']).filter_map(|p| p.trim().strip_suffix(word)?.trim().parse::<u64>().ok()).next();
        if let (Some(passed), Some(failed)) = (count(" passed"), count(" failed")) {
            self.results += 1;
            self.executed += passed + failed;
        }
        let target = self.current.target.clone();
        let done = std::mem::replace(&mut self.current, FailedTarget { target, ..Default::default() });
        if rest.starts_with("FAILED") { self.failed.push(done); }
    }

    /// A panic's place and message, once per place in its target (the JIT's harness says each
    /// twice).
    fn panicked(&mut self, at: String, message: &str) {
        let errors = &mut self.current.errors;
        if errors.len() < SAID_ERRORS && !errors.iter().any(|e| e.starts_with(&at)) {
            errors.push(if message.is_empty() { at } else { format!("{at}: {message}") });
        }
    }

    /// Each failed target, its failed tests and its first panics, one line each, indented: a run
    /// of 70 makepad-fab targets said only `failed`, and its 64 KB tail named none of its failing
    /// targets; its lane ran 35 min again to find them (11-20). Empty when none failed.
    pub fn failed_said(&self) -> String {
        let mut out = String::new();
        for f in &self.failed {
            let more = f.tests.len().saturating_sub(SAID_TESTS);
            let tests = f.tests.iter().take(SAID_TESTS).cloned().collect::<Vec<_>>().join(", ") + &if more > 0 { format!(" and {more} more") } else { String::new() };
            let target = if f.target.is_empty() { "its tests" } else { f.target.as_str() };
            out.push_str(&format!("  {target}: {}\n", if tests.is_empty() { "failed".to_string() } else { tests }));
            for e in &f.errors { out.push_str(&format!("    {}\n", crate::proto::clean(e, 240))); }
        }
        out
    }

    /// Why a test command's run is no pass although it exited 0: it executed no test. None when
    /// it ran one or a `harness = false` target, when it only lists them, or for any other command.
    pub fn no_pass(&self, cmd: &[String]) -> Option<String> {
        let at = test_word(cmd)?;
        let args = cmd[at..].iter().position(|w| w == "--").map_or(&[][..], |p| &cmd[at + p + 1..]);
        let plain = self.plain + u64::from(self.quiet_target);
        if self.executed > 0 || plain > 0 || args.iter().any(|a| a == "--list") { return None; }
        let why = if !filters(args).is_empty() {
            format!("no test matched {}", args.join(" "))
        } else if self.results == 0 {
            "no test ran: the output has no test result".to_string()
        } else {
            "no test ran".to_string()
        };
        let words = &cmd[at + 1..];
        let pkg = test_package(words);
        Some(match pkg.and_then(|pkg| bin_hint(pkg, words)) {
            Some(hint) => format!("{why}: {hint}"),
            None => why,
        })
    }
}

/// Libtest's valued options consume their next word; those values are not name filters.
fn filters(args: &[String]) -> Vec<String> {
    let mut words = args.iter();
    let mut filters = Vec::new();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--skip" | "--test-threads" | "--format" | "--color" | "--logfile" | "-Z" | "--shuffle-seed" => { words.next(); }
            w if !w.starts_with('-') => filters.push(word.clone()),
            _ => {}
        }
    }
    filters
}

/// A hint from the package's sources in this lane, only for a filtered, single-target run.
pub fn elsewhere_hint(repo: &Path, rev: &str, pkg: &str, words: &[String]) -> String {
    #[cfg(feature = "checkout")]
    {
        let Some(at) = words.iter().position(|w| w == "--") else { return String::new() };
        let args = &words[at + 1..];
        let filters = filters(args);
        if filters.is_empty() || selected_targets(words).is_empty() { return String::new(); }
        let Some(pin) = crate::lease::Pin::of_rev(rev) else { return String::new() };
        let Ok(tree) = crate::orch::ci::Tree::at(repo, &pin.pair()) else { return String::new() };
        let Some(dir) = crate::orch::ci::package_dirs(&tree).remove(pkg) else { return String::new() };
        let matches = matches_elsewhere(&repo.join(dir), words, &filters, args.iter().any(|w| w == "--exact"));
        let parts: Vec<String> = matches.iter().enumerate().map(|(i, (flag, count))| {
            if i == 0 { format!("{count} test{} in {flag}", if *count == 1 { "" } else { "s" }) } else { format!("{count} in {flag}") }
        }).collect();
        if parts.is_empty() { String::new() } else { format!(": it matches {}", parts.join(", ")) }
    }
    #[cfg(not(feature = "checkout"))]
    {
        let _ = (repo, rev, pkg, words);
        String::new()
    }
}

/// Add a source hint only to the no-match verdict, preserving the existing bin explanation.
pub fn no_pass_hint(why: &mut String, repo: &Path, rev: &str, cmd: &[String]) {
    if !why.starts_with("no test matched ") { return; }
    let Some(at) = test_word(cmd) else { return };
    let words = &cmd[at + 1..];
    if let Some(pkg) = test_package(words) { why.push_str(&elsewhere_hint(repo, rev, pkg, words)); }
}

fn test_package(words: &[String]) -> Option<&str> {
    let end = words.iter().position(|w| w == "--").unwrap_or(words.len());
    let words = &words[..end];
    words.iter().position(|w| matches!(w.as_str(), "-p" | "--package"))
        .and_then(|at| words.get(at + 1)).or_else(|| words.first().filter(|w| !w.starts_with('-'))).map(String::as_str)
}

fn selected_targets(words: &[String]) -> BTreeSet<String> {
    let mut selected = BTreeSet::new();
    let mut words = words.iter().take_while(|w| w.as_str() != "--");
    while let Some(word) = words.next() {
        match word.as_str() {
            "--tests" | "--all-targets" => return BTreeSet::new(),
            "--lib" => { selected.insert(word.clone()); }
            "--bin" | "--test" => if let Some(name) = words.next() { selected.insert(format!("{word} {name}")); },
            _ => {}
        }
    }
    selected
}

/// Other targets containing matching `#[test]` functions. This is a source hint, without cfg
/// evaluation or macro expansion; unreadable files are skipped.
pub fn matches_elsewhere(pkg_dir: &Path, chosen: &[String], filters: &[String], exact: bool) -> Vec<(String, usize)> {
    let selected = selected_targets(chosen);
    if selected.is_empty() || filters.is_empty() { return Vec::new(); }
    let mut scan = TestScan { filters, exact, active: BTreeSet::new(), found: Vec::new() };
    // a `harness = false` target's tests never run: no filter selects them
    targets(pkg_dir).into_iter().filter(|t| t.harness && !selected.contains(&t.flag)).filter_map(|t| {
        let count = scan.file(&t.path, "", true);
        (count > 0).then_some((t.flag, count))
    }).collect()
}

/// The `#[test]` functions of each `harness = false` target of the package (`--test pixels` and
/// the names): cargo builds such a target without the test harness, so they are compiled out and
/// never run, silently (a test merged into a target that had become one main's batch, 11-26).
/// The same source scan as [`matches_elsewhere`].
pub fn silent_tests(pkg_dir: &Path) -> Vec<(String, Vec<String>)> {
    targets(pkg_dir).into_iter().filter(|t| !t.harness).filter_map(|t| {
        let mut scan = TestScan { filters: &[], exact: false, active: BTreeSet::new(), found: Vec::new() };
        scan.file(&t.path, "", true);
        (!scan.found.is_empty()).then_some((t.flag, scan.found))
    }).collect()
}

/// [`silent_tests`] of the packages `pkgs`, in the lane's worktree `repo` as its run at `pin`
/// shipped it: one line per target that holds some, naming them and what to do.
#[cfg(feature = "checkout")]
pub fn silent_tests_at(repo: &Path, pin: &crate::lease::Pin, pkgs: &[String]) -> Result<Vec<String>, String> {
    let dirs = crate::orch::ci::package_dirs(&crate::orch::ci::Tree::at_pin(repo, pin)?);
    Ok(pkgs.iter().filter_map(|pkg| Some((pkg, dirs.get(pkg)?))).flat_map(|(pkg, dir)| {
        silent_tests(&repo.join(dir)).into_iter().map(move |(flag, names)| format!(
            "{pkg} {flag} is `harness = false`: cargo builds it without the test harness, so its #[test] fn {} never run{}; make them cases of its main, or move them to a target with the harness",
            names.join(", "), if names.len() == 1 { "s" } else { "" }))
    }).collect())
}
#[cfg(not(feature = "checkout"))]
pub fn silent_tests_at(_: &Path, _: &crate::lease::Pin, _: &[String]) -> Result<Vec<String>, String> { Ok(Vec::new()) }

/// One target of a package: its selecting flag, its root source and whether cargo builds it with
/// the test harness.
struct Target {
    flag: String,
    path: PathBuf,
    harness: bool,
}

/// The package's lib, bins and test targets, as cargo finds them: the explicit tables and, unless
/// `autobins`/`autotests` turn it off, the files of `src/bin` and `tests`.
fn targets(pkg_dir: &Path) -> Vec<Target> {
    let Some(doc) = std::fs::read_to_string(pkg_dir.join("Cargo.toml")).ok().and_then(|s| parse_toml(&s).ok()) else { return Vec::new() };
    let Some(pkg) = doc.get_path(&["package", "name"]).and_then(Toml::as_str) else { return Vec::new() };
    let harness = |table: Option<&BTreeMap<String, Toml>>| table.and_then(|t| t.get("harness")).and_then(Toml::as_bool) != Some(false);
    let lib = doc.get_path(&["lib", "path"]).and_then(Toml::as_str).unwrap_or("src/lib.rs");
    let mut targets = vec![Target { flag: "--lib".to_string(), path: pkg_dir.join(lib), harness: harness(doc.root.get("lib").and_then(Toml::as_table)) }];
    for (kind, dir, auto) in [("bin", "src/bin", "autobins"), ("test", "tests", "autotests")] {
        let mut paths = BTreeMap::new();
        // Keep defaults available for explicit targets whose path is omitted, even with auto
        // discovery disabled. Explicit names override an automatically discovered target.
        if kind == "bin" && pkg_dir.join("src/main.rs").is_file() { paths.insert(pkg.to_string(), (pkg_dir.join("src/main.rs"), true)); }
        if let Ok(entries) = std::fs::read_dir(pkg_dir.join(dir)) {
            for entry in entries.flatten() {
                let path = entry.path();
                let source = if path.is_dir() { path.join("main.rs") } else { path.clone() };
                if !source.is_file() || source.extension().is_none_or(|e| e != "rs") { continue; }
                let name = if path.is_dir() { path.file_name() } else { path.file_stem() };
                if let Some(name) = name.and_then(|s| s.to_str()) { paths.insert(name.to_string(), (source, true)); }
            }
        }
        let defaults = paths.clone();
        if doc.get_path(&["package", auto]).and_then(Toml::as_bool) == Some(false) { paths.clear(); }
        let mut explicit = BTreeMap::new();
        for target in doc.root.get(kind).and_then(Toml::as_array_of_tables).unwrap_or(&[]) {
            let Some(name) = target.get("name").and_then(Toml::as_str).or((kind == "bin").then_some(pkg)) else { continue };
            let path = target.get("path").and_then(Toml::as_str).map(|p| pkg_dir.join(p))
                .or_else(|| defaults.get(name).map(|(p, _)| p.clone())).unwrap_or_else(|| pkg_dir.join(dir).join(format!("{name}.rs")));
            paths.retain(|_, (p, _)| p != &path);
            explicit.insert(name.to_string(), (path, harness(Some(target))));
        }
        paths.extend(explicit);
        targets.extend(paths.into_iter().map(|(name, (path, harness))| Target { flag: format!("--{kind} {name}"), path, harness }));
    }
    targets
}

/// A walk of a target's modules for `#[test]` functions: the ones whose full names match
/// `filters` (every one when there are none) are counted and named in `found`.
struct TestScan<'a> {
    filters: &'a [String],
    exact: bool,
    active: BTreeSet<PathBuf>,
    found: Vec<String>,
}

impl TestScan<'_> {
    fn file(&mut self, path: &Path, prefix: &str, root: bool) -> usize {
        let Ok(key) = path.canonicalize() else { return 0 };
        if !self.active.insert(key.clone()) { return 0; }
        let count = std::fs::read_to_string(path).ok().map_or(0, |source| {
            let parent = path.parent().unwrap_or(Path::new(""));
            let dir = if root || path.file_name().is_some_and(|n| n == "mod.rs") { parent.to_path_buf() }
                else { parent.join(path.file_stem().unwrap_or_default()) };
            self.scope(&source_words(&source), prefix, &dir, parent)
        });
        self.active.remove(&key);
        count
    }

    fn scope(&mut self, words: &[&str], prefix: &str, dir: &Path, path_base: &Path) -> usize {
        let (mut i, mut count, mut test, mut path) = (0, 0, false, None);
        while i < words.len() {
            if words[i] == "#" && words.get(i + 1) == Some(&"[") {
                let end = group_end(words, i + 1, "[", "]");
                let attr = &words[i + 2..end];
                if attr == ["test"] { test = true; }
                if let ["path", "=", value] = attr { path = value.strip_prefix('"').and_then(|s| s.strip_suffix('"')); }
                i = end + 1;
                continue;
            }
            if words[i] == "mod" && i + 2 < words.len() {
                let name = words[i + 1].strip_prefix("r#").unwrap_or(words[i + 1]);
                let module = format!("{prefix}{name}::");
                match words[i + 2] {
                    ";" => {
                        let file = path.map(|p| path_base.join(p)).unwrap_or_else(|| {
                            let file = dir.join(format!("{name}.rs"));
                            if file.is_file() { file } else { dir.join(name).join("mod.rs") }
                        });
                        count += self.file(&file, &module, false);
                        i += 3;
                    }
                    "{" => {
                        let end = group_end(words, i + 2, "{", "}");
                        let child = path.map_or_else(|| dir.join(name), |p| path_base.join(p));
                        count += self.scope(&words[i + 3..end], &module, &child, &child);
                        i = end + 1;
                    }
                    _ => { i += 1; }
                }
                test = false;
                path = None;
                continue;
            }
            if words[i] == "fn" {
                if let Some(name) = words.get(i + 1).filter(|_| test) {
                    let full = format!("{prefix}{}", name.strip_prefix("r#").unwrap_or(name));
                    if self.filters.is_empty() || self.filters.iter().any(|f| if self.exact { full == *f } else { full.contains(f) }) {
                        count += 1;
                        self.found.push(full);
                    }
                }
                test = false;
                path = None;
            }
            // Function bodies, macro token trees and attribute arguments are not modules.
            if let Some(close) = match words[i] { "{" => Some("}"), "(" => Some(")"), "[" => Some("]"), _ => None } {
                let open = words[i];
                i = group_end(words, i, open, close) + 1;
                if open == "{" { test = false; path = None; }
            } else {
                if words[i] == ";" { test = false; path = None; }
                i += 1;
            }
        }
        count
    }
}

fn group_end(words: &[&str], start: usize, open: &str, close: &str) -> usize {
    let mut depth = 0;
    for (i, word) in words.iter().enumerate().skip(start) {
        if *word == open { depth += 1; }
        if *word == close { depth -= 1; if depth == 0 { return i; } }
    }
    words.len()
}

/// A text scan: preserve string literals for path attributes, skip comments, and keep braces
/// inside literals out of the module nesting. No Rust parser or cfg evaluator is needed.
fn source_words(source: &str) -> Vec<&str> {
    let b = source.as_bytes();
    let (mut i, mut words) = (0, Vec::new());
    while i < b.len() {
        if b[i].is_ascii_whitespace() { i += 1; continue; }
        if b[i..].starts_with(b"//") { while i < b.len() && b[i] != b'\n' { i += 1; } continue; }
        if b[i..].starts_with(b"/*") {
            let mut depth = 1;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i..].starts_with(b"/*") { depth += 1; i += 2; }
                else if b[i..].starts_with(b"*/") { depth -= 1; i += 2; }
                else { i += 1; }
            }
            continue;
        }
        let start = i;
        let raw = if b[i..].starts_with(b"br") || b[i..].starts_with(b"cr") { i + 1 } else { i };
        if b[raw] == b'r' {
            let mut quote = raw + 1;
            while b.get(quote) == Some(&b'#') { quote += 1; }
            if b.get(quote) == Some(&b'"') {
                let end = format!("\"{}", "#".repeat(quote - raw - 1));
                i = source[quote + 1..].find(&end).map_or(b.len(), |p| quote + 1 + p + end.len());
                words.push(&source[start..i]);
                continue;
            }
        }
        if b[i] == b'"' {
            i += 1;
            while i < b.len() {
                let c = b[i];
                i += 1;
                if c == b'\\' { i = (i + 1).min(b.len()); }
                else if c == b'"' { break; }
            }
        } else if b[i..].starts_with(b"'\\") {
            i += 1;
            while i < b.len() {
                let c = b[i];
                i += 1;
                if c == b'\\' { i = (i + 1).min(b.len()); }
                else if c == b'\'' { break; }
            }
        } else if b[i] == b'\'' && source[i + 1..].chars().next().is_some_and(|c| b.get(i + 1 + c.len_utf8()) == Some(&b'\'')) {
            i += 2 + source[i + 1..].chars().next().unwrap().len_utf8();
        } else if b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 128 {
            if b[i..].starts_with(b"r#") { i += 2; }
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 128) { i += 1; }
        } else { i += 1; }
        words.push(&source[start..i]);
    }
    words
}

/// What a run of `--bin B`'s no tests was probably after, for a command's words after its `test`.
pub fn bin_hint(pkg: &str, words: &[String]) -> Option<String> {
    let bin = words.iter().take_while(|w| *w != "--").position(|w| w == "--bin").and_then(|at| words.get(at + 1))?;
    Some(format!("--bin selects that bin's unit tests; `makepad run {pkg} --bin {bin} -- args` or `devhub live start {pkg} --bin {bin} -- args` runs the program"))
}

/// Where a command's `test` subcommand is, including makepad invoked through cargo.
pub fn test_word(cmd: &[String]) -> Option<usize> {
    let w: Vec<&str> = cmd.iter().map(String::as_str).collect();
    (1..w.len()).find(|&i| w[i] == "test" && ((i == 1 && w[0] == "cargo") || w[i - 1].rsplit(['/', '\\']).next() == Some("makepad") || (w[i - 1] == "--" && w[..i].contains(&"makepad"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_filter_names_the_other_targets_it_matches() {
        let dir = std::env::temp_dir().join(format!("devhub-filter-hint-{}", std::process::id()));
        for path in ["src/b", "tests"] { std::fs::create_dir_all(dir.join(path)).unwrap(); }
        for (path, text) in [
            ("Cargo.toml", "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n"),
            ("src/lib.rs", "mod a;\n#[test]\nfn lib_one() {}\n"),
            ("src/a.rs", "#[test]\nfn the_lock_removes_x() {}\n"),
            ("src/main.rs", "mod b;\nmod c { #[test] fn the_lock_removes_y() {} }\n"),
            ("src/b/mod.rs", "#[test]\nfn the_lock_removes_z() {}\n"),
            ("tests/it.rs", "#[test]\nfn the_lock_removes_w() {}\n"),
        ] { std::fs::write(dir.join(path), text).unwrap(); }
        let matches = |chosen: &str, filter: &str, exact| matches_elsewhere(&dir, &args(chosen), &args(filter), exact);
        assert_eq!(matches("--lib", "the_lock_removes", false), vec![("--bin pkg".into(), 2), ("--test it".into(), 1)]);
        assert_eq!(matches("--bin pkg", "the_lock_removes", false), vec![("--lib".into(), 1), ("--test it".into(), 1)]);
        assert_eq!(matches("--bin pkg", "a::the_lock_removes_x", true), vec![("--lib".into(), 1)]);
        assert_eq!(matches("--lib", "c::the_lock_removes_y", true), vec![("--bin pkg".into(), 1)]);
        assert_eq!(matches("--lib", "unmatched", false), vec![]);
        assert_eq!(matches("--tests", "the_lock_removes", false), vec![]);
        assert_eq!(matches("", "the_lock_removes", false), vec![]);
        assert_eq!(silent_tests(&dir), vec![], "every target here has the test harness");
        // Explicit paths, default directory targets, nested file modules and inline modules
        // share the same naming rules. Comments and literals cannot introduce tests.
        for path in ["src/a", "src/bin/tool", "tests/suite", "src/c"] { std::fs::create_dir_all(dir.join(path)).unwrap(); }
        for (path, text) in [
            ("Cargo.toml", "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n[lib]\npath = \"src/lib.rs\"\n[[bin]]\nname = \"renamed\"\npath = \"src/main.rs\"\n[[test]]\nname = \"custom\"\npath = \"check.rs\"\n[[test]]\nname = \"it\"\nharness = false\n[[test]]\nname = \"batch\"\npath = \"batch.rs\"\nharness = false\n"),
            ("src/a.rs", "mod nested;\n#[path = \"other.rs\"]\nmod moved;\n"),
            ("src/a/nested.rs", "#[test] fn the_lock_removes_nested() {}"),
            ("src/other.rs", "#[test] fn the_lock_removes_moved() {}"),
            ("src/main.rs", "mod c { mod child; }\n// #[test] fn the_lock_removes_fake() {}\n/* #[test] fn the_lock_removes_fake() {} */\nconst TEXT: &str = r#\"#[test] fn the_lock_removes_fake() {}\"#;\nconst BYTE: u8 = b'\\\'';\n"),
            ("src/c/child.rs", "#[test] fn the_lock_removes_child() {}"),
            ("src/bin/extra.rs", "#[test] fn the_lock_removes_extra() {}"),
            ("src/bin/tool/main.rs", "#[test] fn the_lock_removes_tool() {}"),
            ("tests/suite/main.rs", "#[test] fn the_lock_removes_suite() {}"),
            ("check.rs", "#[test] fn the_lock_removes_custom() {}"),
            ("tests/it.rs", "mod inner { #[test] fn the_lock_removes_inner() {} }\n#[test]\nfn the_lock_removes_w() {}\nfn main() {}\n"),
            ("batch.rs", "// #[test] fn the_lock_removes_fake() {}\nfn main() {}\n"),
        ] { std::fs::write(dir.join(path), text).unwrap(); }
        assert_eq!(matches("--lib", "the_lock_removes", false), vec![("--bin extra".into(), 1), ("--bin renamed".into(), 1), ("--bin tool".into(), 1), ("--test custom".into(), 1), ("--test suite".into(), 1)],
            "`harness = false` (it, by its table without a path) leaves a target's tests out of the hint");
        assert_eq!(matches("--bin renamed", "a::", false), vec![("--lib".into(), 2)]);
        assert_eq!(matches("--lib", "c::child::the_lock_removes_child", true), vec![("--bin renamed".into(), 1)]);
        assert_eq!(silent_tests(&dir), vec![("--test it".to_string(), vec!["inner::the_lock_removes_inner".to_string(), "the_lock_removes_w".to_string()])],
            "a harness = false target's #[test] fns are named, nested ones by path; one with only a main (and a commented test) is not");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_run_that_executed_no_test_is_no_pass() {
        let ok = "running 3 tests\n...\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.00s\n";
        assert_eq!((Tally::of(ok).results, Tally::of(ok).executed), (2, 3));
        assert_eq!(Tally::of(ok).no_pass(&args("cargo test -p x -- foo")), None);
        let none = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.00s\n";
        assert_eq!(Tally::of(none).no_pass(&args("cargo test -p x -- foo --exact")).as_deref(), Some("no test matched foo --exact"));
        let jit = "cargo run --profile host -q -p makepad-run --bin makepad -- test x --lib -- --skip slow foo";
        assert_eq!(Tally::of(none).no_pass(&args(jit)).as_deref(), Some("no test matched --skip slow foo"));
        assert_eq!(Tally::of(none).no_pass(&args("cargo test -p x --lib -- --skip slow")).as_deref(), Some("no test ran"));
        assert_eq!(Tally::of("").no_pass(&args("cargo test -p x")).as_deref(), Some("no test ran: the output has no test result"));
        assert_eq!(Tally::of("").no_pass(&args("cargo test -p x -- --list")), None);
        assert_eq!(Tally::of("").no_pass(&args("bash bench.sh")), None, "not a test command");
        // the bin's own unit tests, none: it names what runs the program
        let hint = "--bin selects that bin's unit tests; `makepad run x --bin b -- args` or `devhub live start x --bin b -- args` runs the program";
        assert_eq!(Tally::of(none).no_pass(&args("cargo test --release -p x --bin b")), Some(format!("no test ran: {hint}")));
        assert_eq!(Tally::of(none).no_pass(&args("cargo run --profile host -p makepad-run --bin makepad -- test x --bin b -- foo")), Some(format!("no test matched foo: {hint}")));
        // the JIT's own single-file summary, and a harness = false target
        assert_eq!(Tally::of("test result: 10 passed; 0 failed. compiled 185 fns (35708 bytes)\n").executed, 10);
        assert_eq!(Tally::of("     Running tests/bench.rs (harness = false)\n").no_pass(&args("makepad test x")), None);
        // cargo's harness = false target: a target that ran and said nothing of libtest's, before
        // the next target or at the end; a libtest target that ran no test is none
        let cargo = |tail: &str| format!("     Running unittests src/lib.rs (target/release/deps/x-0123)\n\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n{tail}");
        let close = cargo("     Running tests/scope_close.rs (target/release/deps/scope_close-4567)\nscope close: 12 checks ok\n");
        assert_eq!(Tally::of(&close).no_pass(&args("cargo test -p x")), None);
        let between = format!("{close}   Doc-tests x\n\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n");
        assert_eq!((Tally::of(&between).plain, Tally::of(&between).no_pass(&args("cargo test -p x"))), (1, None));
        assert_eq!(Tally::of(&cargo("")).no_pass(&args("cargo test -p x")).as_deref(), Some("no test ran"));
        let one = "     Running tests/a.rs (target/release/deps/a-1)\n\nrunning 1 test\ntest t ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        assert_eq!((Tally::of(one).plain, Tally::of(one).executed), (0, 1));
        // fed in pieces, as a stream
        let mut t = Tally::default();
        for piece in ["test res", "ult: FAILED. 0 passed; 2 fa", "iled; 0 ignored\n"] { t.feed(piece.as_bytes()); }
        assert_eq!((t.results, t.executed), (1, 2));
        // each failed target, its failed tests and its first panic once per place (the JIT's
        // harness says each twice), in the order they ran, kept over the target's own processes
        // saying its `Running` line again; a passing target is none of them: a 70-target run said
        // only `failed` (11-20)
        let run = "     Running tests/chrome.rs\n\nrunning 2 tests\n\
            thread 'the_first_frame' (17) panicked at libs/makepad_test/src/runtime.rs:1724:5:\nwait: `path=\"ws_design\"` did not come\n\
            thread 'the_first_frame' (17) panicked at libs/makepad_test/src/runtime.rs:1724:5:\ntest panicked: wait: `path=\"ws_design\"` did not come\n\
            test the_first_frame ... FAILED\n     Running tests/chrome.rs\ntest the_second ... ok\n\nfailures:\n    the_first_frame\n\n\
            test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.00s\n\
            relax test: 0 warnings\n     Running tests/undo.rs\n\nrunning 1 test\ntest it_undoes ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored\n\
            \x20    Running tests/monitor.rs (target/release/deps/monitor-1)\n\nrunning 1 test\nthread 'old' panicked at tests/monitor.rs:9:5: assertion failed\n\
            test old ... FAILED\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored\n";
        let t = Tally::of(run);
        assert_eq!(t.failed.iter().map(|f| (f.target.as_str(), f.tests.clone())).collect::<Vec<_>>(), [("tests/chrome.rs", vec!["the_first_frame".to_string()]), ("tests/monitor.rs", vec!["old".to_string()])]);
        assert_eq!(t.failed[0].errors, ["panicked at libs/makepad_test/src/runtime.rs:1724:5: wait: `path=\"ws_design\"` did not come"], "once per place, with its message");
        assert_eq!(t.failed_said(), "  tests/chrome.rs: the_first_frame\n    panicked at libs/makepad_test/src/runtime.rs:1724:5: wait: `path=\"ws_design\"` did not come\n  tests/monitor.rs: old\n    panicked at tests/monitor.rs:9:5: assertion failed\n");
        let mut streamed = Tally::default();
        for piece in run.as_bytes().chunks(7) { streamed.feed(piece); }
        assert_eq!(streamed.failed, t.failed, "fed as a stream");
        assert_eq!(Tally::of(one).failed_said(), "", "none failed");
    }

    #[test]
    fn test_commands_are_told_apart() {
        assert_eq!(test_word(&args("cargo test --release -p makepad-devhub")), Some(1));
        assert_eq!(test_word(&args("cargo run --profile host -q -p makepad-run --bin makepad -- test makepad-devhub --lib")), Some(10));
        assert_eq!(test_word(&args("/x/target/host/makepad test relax-base")), Some(1));
        assert_eq!(test_word(&args("cargo run -p tool -- test")), None);
        assert_eq!(test_word(&args("bash run.sh test")), None);
        assert_eq!(test_word(&args("cargo build -p test")), None);
    }
}
