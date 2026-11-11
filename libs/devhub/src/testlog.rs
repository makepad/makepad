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
}

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
        if line.starts_with("Running ") || line.starts_with("Doc-tests ") {
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
        let Some(rest) = line.strip_prefix("test result: ") else { return };
        self.quiet_target = false;
        let count = |word: &str| rest.split([';', '.']).filter_map(|p| p.trim().strip_suffix(word)?.trim().parse::<u64>().ok()).next();
        if let (Some(passed), Some(failed)) = (count(" passed"), count(" failed")) {
            self.results += 1;
            self.executed += passed + failed;
        }
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
    let Some(doc) = std::fs::read_to_string(pkg_dir.join("Cargo.toml")).ok().and_then(|s| parse_toml(&s).ok()) else { return Vec::new() };
    let Some(pkg) = doc.get_path(&["package", "name"]).and_then(Toml::as_str) else { return Vec::new() };
    let lib = doc.get_path(&["lib", "path"]).and_then(Toml::as_str).unwrap_or("src/lib.rs");
    let mut targets = vec![("--lib".to_string(), pkg_dir.join(lib))];
    for (kind, dir, auto) in [("bin", "src/bin", "autobins"), ("test", "tests", "autotests")] {
        let mut paths = BTreeMap::new();
        // Keep defaults available for explicit targets whose path is omitted, even with auto
        // discovery disabled. Explicit names override an automatically discovered target.
        if kind == "bin" && pkg_dir.join("src/main.rs").is_file() { paths.insert(pkg.to_string(), pkg_dir.join("src/main.rs")); }
        if let Ok(entries) = std::fs::read_dir(pkg_dir.join(dir)) {
            for entry in entries.flatten() {
                let path = entry.path();
                let source = if path.is_dir() { path.join("main.rs") } else { path.clone() };
                if !source.is_file() || source.extension().is_none_or(|e| e != "rs") { continue; }
                let name = if path.is_dir() { path.file_name() } else { path.file_stem() };
                if let Some(name) = name.and_then(|s| s.to_str()) { paths.insert(name.to_string(), source); }
            }
        }
        let defaults = paths.clone();
        if doc.get_path(&["package", auto]).and_then(Toml::as_bool) == Some(false) { paths.clear(); }
        let mut explicit = BTreeMap::new();
        for target in doc.root.get(kind).and_then(Toml::as_array_of_tables).unwrap_or(&[]) {
            let Some(name) = target.get("name").and_then(Toml::as_str).or((kind == "bin").then_some(pkg)) else { continue };
            let path = target.get("path").and_then(Toml::as_str).map(|p| pkg_dir.join(p))
                .or_else(|| defaults.get(name).cloned()).unwrap_or_else(|| pkg_dir.join(dir).join(format!("{name}.rs")));
            paths.retain(|_, p| p != &path);
            explicit.insert(name.to_string(), path);
        }
        paths.extend(explicit);
        targets.extend(paths.into_iter().map(|(name, path)| (format!("--{kind} {name}"), path)));
    }
    let mut scan = TestScan { filters, exact, active: BTreeSet::new() };
    targets.into_iter().filter(|(flag, _)| !selected.contains(flag)).filter_map(|(flag, path)| {
        let count = scan.file(&path, "", true);
        (count > 0).then_some((flag, count))
    }).collect()
}

struct TestScan<'a> {
    filters: &'a [String],
    exact: bool,
    active: BTreeSet<PathBuf>,
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
                    count += usize::from(self.filters.iter().any(|f| if self.exact { full == *f } else { full.contains(f) }));
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
        // Explicit paths, default directory targets, nested file modules and inline modules
        // share the same naming rules. Comments and literals cannot introduce tests.
        for path in ["src/a", "src/bin/tool", "tests/suite", "src/c"] { std::fs::create_dir_all(dir.join(path)).unwrap(); }
        for (path, text) in [
            ("Cargo.toml", "[package]\nname = \"pkg\"\nversion = \"0.1.0\"\n[lib]\npath = \"src/lib.rs\"\n[[bin]]\nname = \"renamed\"\npath = \"src/main.rs\"\n[[test]]\nname = \"custom\"\npath = \"check.rs\"\n"),
            ("src/a.rs", "mod nested;\n#[path = \"other.rs\"]\nmod moved;\n"),
            ("src/a/nested.rs", "#[test] fn the_lock_removes_nested() {}"),
            ("src/other.rs", "#[test] fn the_lock_removes_moved() {}"),
            ("src/main.rs", "mod c { mod child; }\n// #[test] fn the_lock_removes_fake() {}\n/* #[test] fn the_lock_removes_fake() {} */\nconst TEXT: &str = r#\"#[test] fn the_lock_removes_fake() {}\"#;\nconst BYTE: u8 = b'\\\'';\n"),
            ("src/c/child.rs", "#[test] fn the_lock_removes_child() {}"),
            ("src/bin/extra.rs", "#[test] fn the_lock_removes_extra() {}"),
            ("src/bin/tool/main.rs", "#[test] fn the_lock_removes_tool() {}"),
            ("tests/suite/main.rs", "#[test] fn the_lock_removes_suite() {}"),
            ("check.rs", "#[test] fn the_lock_removes_custom() {}"),
        ] { std::fs::write(dir.join(path), text).unwrap(); }
        assert_eq!(matches("--lib", "the_lock_removes", false), vec![("--bin extra".into(), 1), ("--bin renamed".into(), 1), ("--bin tool".into(), 1), ("--test custom".into(), 1), ("--test it".into(), 1), ("--test suite".into(), 1)]);
        assert_eq!(matches("--bin renamed", "a::", false), vec![("--lib".into(), 2)]);
        assert_eq!(matches("--lib", "c::child::the_lock_removes_child", true), vec![("--bin renamed".into(), 1)]);
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
