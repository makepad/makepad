//! `makepad test [options] <package> [--lib] [--bins] [--bin <name>]... [--tests] [--test <name>]...
//! [cargo feature flags] [-- <libtest args>]`: what `cargo test -p <package>` runs,
//! under the JIT. The lib's unit tests (the lib built cfg(test)) are a program of their own; the
//! selected bins' unit tests and integration tests (beside the plain lib) are one program, as
//! `cargo test` builds them in one go, so every target after the first restores its collected
//! image. Each target runs in a process of its own, one after the other (`makepad run --harness
//! --subject <its source>`, run.rs): its own main, statics, environment, threads, exit status
//! and errors, as under cargo. A file of the program that does not parse stops the run there, as
//! cargo builds no target then. Without a selection all of them run (cargo test's default minus
//! doc tests, which are counted and said).
//! A `harness = false` target runs as a plain program (`makepad run --plain-test --subject ..`),
//! as cargo runs it: its exit status is its verdict.
//! `--live` keeps one harness target (default: the lib, else the package's only harness target) running across source edits.

pub fn cmd_test_package(args: &[String]) -> i32 {
    // Loader options go before each run's package; feature flags go after it.
    let mut i = 0;
    let mut pre: Vec<String> = Vec::new();
    let mut flags: Vec<String> = Vec::new();
    let mut feature_flags: Vec<String> = Vec::new();
    let mut live = false;
    while i < args.len() && args[i].starts_with('-') && args[i] != "-p" && args[i] != "--package" {
        if args[i] == "--live" {
            live = true;
            i += 1;
            continue;
        }
        if is_feature_flag(&args[i]) {
            let takes_value = matches!(args[i].as_str(), "--features" | "-F");
            feature_flags.push(args[i].clone());
            flags.push(args[i].clone());
            i += 1;
            if takes_value {
                let Some(v) = args.get(i) else {
                    eprintln!("makepad test: {} needs a value", args[i - 1]);
                    return 2;
                };
                feature_flags.push(v.clone());
                flags.push(v.clone());
                i += 1;
            }
            continue;
        }
        if matches!(args[i].as_str(), "--cfg" | "--report-to" | "--run-dir" | "--workspace-dir") && i + 1 < args.len() {
            pre.push(args[i].clone());
            i += 1;
        }
        pre.push(args[i].clone());
        i += 1;
    }
    if i < args.len() && (args[i] == "-p" || args[i] == "--package") {
        i += 1;
    }
    let Some(package) = args.get(i).cloned() else {
        eprintln!("usage: makepad test <package> [--lib] [--bins] [--bin <name>] [--tests] [--test <name>] [cargo flags] [-- <test args>]");
        return 2;
    };
    let mut lib = false;
    let mut bins: Option<Vec<String>> = None;
    let mut tests: Option<Vec<String>> = None;
    let mut selections = 0;
    let mut all_tests = false;
    let mut extra_package = false;
    let mut prog: Vec<String> = Vec::new();
    let mut words = args[i + 1..].iter();
    while let Some(w) = words.next() {
        match w.as_str() {
            "--" => {
                prog.extend(words.by_ref().cloned());
                break;
            }
            "--live" => live = true,
            "--lib" => { lib = true; selections += 1; }
            "--bins" => {
                bins.get_or_insert_with(Vec::new);
            }
            "--bin" => match words.next() {
                Some(b) => { bins.get_or_insert_with(Vec::new).push(b.clone()); selections += 1; }
                None => {
                    eprintln!("makepad test: --bin needs a target name");
                    return 2;
                }
            },
            "--tests" => {
                all_tests = true;
                tests.get_or_insert_with(Vec::new);
            }
            "--test" => match words.next() {
                Some(t) => { tests.get_or_insert_with(Vec::new).push(t.clone()); selections += 1; }
                None => {
                    eprintln!("makepad test: --test needs a target name");
                    return 2;
                }
            },
            // every target runs anyway: one failing says nothing about the next
            "--no-fail-fast" => {}
            "--release" => {}
            "--features" | "-F" => {
                let Some(v) = words.next() else {
                    eprintln!("makepad test: {w} needs a value");
                    return 2;
                };
                feature_flags.extend([w.clone(), v.clone()]);
                flags.extend([w.clone(), v.clone()]);
            }
            "--cfg" | "--domain" => {
                flags.push(w.clone());
                if let Some(v) = words.next() {
                    flags.push(v.clone());
                }
            }
            s if s.starts_with("--bin=") => {
                bins.get_or_insert_with(Vec::new).push(s[6..].to_string());
                selections += 1;
            }
            s if s.starts_with("--test=") => {
                tests.get_or_insert_with(Vec::new).push(s[7..].to_string());
                selections += 1;
            }
            s if is_feature_flag(s) => {
                feature_flags.push(w.clone());
                flags.push(w.clone());
            }
            "--verbose" | "--silent" => pre.push(w.clone()),
            _ => {
                extra_package |= !w.starts_with('-');
                flags.push(w.clone());
            }
        }
    }
    if live {
        if package.starts_with('-') || package.ends_with(".rs") || all_tests || selections > 1 || extra_package
            || pre.iter().chain(&flags).any(|s| matches!(s.as_str(), "--tests" | "--bins" | "--bin" | "--test" | "--lib" | "--no-live" | "--workspace" | "--all-targets" | "-p" | "--package")
                || s.starts_with("--package=") || s.starts_with("--bin=") || s.starts_with("--test=")) {
            return live_usage();
        }
        pre.push("--live".to_string());
    }
    let sess = std::sync::Arc::new(crate::Session::default());
    let live_default = live && !lib && bins.is_none() && tests.is_none();
    let (has_lib, bin_targets, targets) = match crate::resolve::test_targets(&sess, &crate::run::workspace_dir(), &package,
        &feature_flags, bins.as_deref().unwrap_or_default(), tests.as_deref().unwrap_or_default()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("makepad test: {e}");
            return 101;
        }
    };
    if live_default {
        if has_lib.as_ref().is_some_and(|l| l.test) {
            lib = true;
        } else {
            let harness: Vec<_> = bin_targets.iter().filter(|t| t.2).map(|t| ("--bin", &t.0))
                .chain(targets.iter().filter(|t| t.2).map(|t| ("--test", &t.0))).collect();
            if harness.len() != 1 {
                let names = harness.iter().map(|(flag, name)| format!("{flag} {name}")).collect::<Vec<_>>().join(", ");
                eprintln!("makepad test --live: package `{package}` has no lib and {} harness targets ({names}): select one", harness.len());
                return 2;
            }
            let (flag, name) = harness[0];
            if flag == "--bin" { bins = Some(vec![name.clone()]); }
            else { tests = Some(vec![name.clone()]); }
        }
    }
    if live && lib && has_lib.is_none() { return live_usage(); }
    let default_selection = !lib && bins.is_none() && tests.is_none();
    if default_selection {
        lib = has_lib.as_ref().is_some_and(|l| l.test);
        bins = (!bin_targets.is_empty()).then(Vec::new);
        tests = (!targets.is_empty()).then(Vec::new);
    }
    // each run: None the lib's unit tests (`--lib`), else one bin or test target out of the
    // package's test graph: its source path, and whether it is a `harness = false` one (a plain
    // program, its exit status the verdict, as cargo runs it)
    let mut runs: Vec<Option<(String, bool)>> = Vec::new();
    if lib {
        runs.push(None);
    }
    // a selection's targets, harness ones first: every target for an empty one
    let chosen = |all: &[(String, String, bool)], names: &[String]| -> Result<Vec<(String, bool)>, String> {
        for n in names {
            if !all.iter().any(|t| &t.0 == n) {
                return Err(format!("no target named `{n}` in package `{package}`"));
            }
        }
        let (h, p): (Vec<_>, Vec<_>) = all.iter().filter(|t| names.is_empty() || names.contains(&t.0)).partition(|t| t.2);
        Ok(h.into_iter().chain(p).map(|t| (t.1.clone(), !t.2)).collect())
    };
    for (sel, all) in [(&bins, &bin_targets), (&tests, &targets)] {
        let Some(names) = sel else { continue };
        match chosen(all, names) {
            Ok(r) => runs.extend(r.into_iter().map(Some)),
            Err(e) => {
                eprintln!("makepad test: {e}");
                return 101;
            }
        }
    }
    // the test program of the selected bin and integration test targets, as `cargo test` builds
    // them: each named (the `harness = false` ones too)
    let mut graph: Vec<String> = Vec::new();
    for (sel, all, flag) in [(&bins, &bin_targets, "--bin"), (&tests, &targets, "--test")] {
        let Some(names) = sel else { continue };
        for t in all.iter().filter(|t| names.is_empty() || names.contains(&t.0)) {
            if flag == "--bin" && graph.is_empty() {
                graph.push("--bins".to_string());
            }
            graph.extend([flag.to_string(), t.0.clone()]);
        }
    }
    if live && (runs.len() != 1 || runs[0].as_ref().is_some_and(|r| r.1)) { return live_usage(); }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("makepad test: the loader's own path: {e}");
            return 101;
        }
    };
    let pkg_dir = crate::resolve::package_dir(&sess, &crate::run::workspace_dir(), &package).unwrap_or_default();
    let mut code = 0;
    // a filter no target matched is no pass (a target exits NO_TESTS_EXIT when it matched none)
    let (mut matched, mut unmatched) = (false, false);
    for run in runs {
        let mut a: Vec<String> = vec!["run".to_string()];
        match &run {
            Some((path, true)) => {
                let rel = path.strip_prefix(pkg_dir.as_str()).map_or(path.as_str(), |r| r.trim_start_matches(['/', '\\']));
                eprintln!("     Running {rel} (harness = false)");
                a.extend(["--plain-test".to_string(), "--subject".to_string(), path.clone()]);
            }
            Some((path, false)) => a.extend(["--harness".to_string(), "--subject".to_string(), path.clone()]),
            None => a.push("--harness".to_string()),
        }
        a.extend(pre.iter().cloned());
        a.push(package.clone());
        a.extend(flags.iter().cloned());
        match &run {
            Some(_) => a.extend(graph.iter().cloned()),
            None => a.push("--lib".to_string()),
        }
        a.push("--".to_string());
        a.extend(prog.iter().cloned());
        let status = std::process::Command::new(&exe).args(&a).env(makepad_remote::hot::TEST_PACKAGE_ENV, "1").status();
        let c = match status {
            Ok(s) => s.code().unwrap_or(101),
            Err(e) => {
                eprintln!("makepad test: starting {}: {e}", exe.display());
                101
            }
        };
        if c == makepad_remote::hot::NO_TESTS_EXIT {
            unmatched = true;
            continue;
        }
        matched = true;
        if c != 0 {
            if c == makepad_remote::hot::JIT_ERROR_EXIT { return c; }
            // the program did not load (a file that does not parse, named above): every target
            // of it would say the same, and cargo test runs none then
            if c == makepad_remote::hot::LOAD_ERROR_EXIT { return 101; }
            if run.as_ref().is_some_and(|r| r.1) {
                eprintln!("error: test failed (harness = false target exited with status {c})");
            }
            code = 101;
        }
    }
    if unmatched && !matched {
        eprintln!("error: no test matched {}", prog.join(" "));
        return makepad_remote::hot::NO_TESTS_EXIT;
    }
    // cargo test's default selection also runs the lib's doc tests; makepad test does not: say
    // how many there are, so the counts compare honestly
    if default_selection {
        if let Some(lib_target) = has_lib.as_ref().filter(|l| l.doctest) {
            let n = count_doctests(&lib_target.path);
            if n > 0 {
                eprintln!("makepad test: {n} doc test{} not run (makepad test runs no doc tests; `cargo test --doc` does)", if n == 1 { "" } else { "s" });
            }
        }
    }
    code
}

fn live_usage() -> i32 {
    eprintln!("usage: makepad test --live <package> [--lib | --bin <name> | --test <name>] [-- <test args>] (exactly one harness target; default: the lib, else the package's only one)");
    2
}

fn is_feature_flag(flag: &str) -> bool {
    matches!(flag, "--features" | "--all-features" | "--no-default-features") || flag.starts_with("--features=") || flag.starts_with("-F")
}

/// The fenced code blocks rustdoc would run as doc tests in the doc comments of the lib's
/// sources (every .rs file under the lib root's folder): rustdoc's LangString rule, a block is
/// Rust when its info string has no token, or a Rust token (`rust`, `ignore`, `no_run`, ..), or
/// no other one (src/librustdoc/html/markdown.rs). `ignore` blocks count (cargo lists them as
/// ignored); indented code blocks are not counted.
fn count_doctests(lib_root: &str) -> usize {
    let dir = std::path::Path::new(lib_root).parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let mut n = 0;
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            // the bins beside the lib (src/main.rs, src/bin/) are no part of it
            if d == dir && (p.file_name().is_some_and(|f| f == "main.rs" || f == "bin")) {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                if let Ok(src) = std::fs::read_to_string(&p) {
                    n += doctests_in(&src);
                }
            }
        }
    }
    n
}

fn doctests_in(src: &str) -> usize {
    let mut n = 0;
    let mut open: Option<&str> = None;
    let mut in_block_doc = false;
    for line in src.lines() {
        let t = line.trim_start();
        let doc = if let Some(r) = t.strip_prefix("///").or_else(|| t.strip_prefix("//!")) {
            if r.starts_with('/') { None } else { Some(r) }
        } else if in_block_doc {
            if t.contains("*/") {
                in_block_doc = false;
            }
            Some(t.trim_start_matches('*'))
        } else if let Some(r) = t.strip_prefix("/**").or_else(|| t.strip_prefix("/*!")) {
            in_block_doc = !r.contains("*/");
            Some(r)
        } else {
            None
        };
        let Some(text) = doc else { continue };
        let text = text.trim();
        let fence = if text.starts_with("```") { "```" } else if text.starts_with("~~~") { "~~~" } else { continue };
        match open {
            Some(f) if f == fence => open = None,
            Some(_) => {}
            None => {
                open = Some(fence);
                if is_rust_block(text.trim_start_matches(fence.chars().next().unwrap())) {
                    n += 1;
                }
            }
        }
    }
    n
}

fn is_rust_block(info: &str) -> bool {
    let mut rust = false;
    let mut other = false;
    for tok in info.split(|c: char| c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()) {
        let tok = tok.trim_start_matches('{').trim_end_matches('}').trim_start_matches('.');
        if matches!(tok, "rust" | "ignore" | "should_panic" | "no_run" | "compile_fail" | "test_harness" | "standalone_crate" | "allow_fail")
            || tok.starts_with("edition") || tok.starts_with("ignore-") {
            rust = true;
        } else {
            other = true;
        }
    }
    rust || !other
}

#[cfg(test)]
mod tests {
    #[test]
    fn doctests_counted_as_rustdoc_reads_them() {
        let src = "/// ```\n/// assert!(true);\n/// ```\n/// ```text\n/// no\n/// ```\n//! ```rust,no_run\n//! x\n//! ```\n/// ```ignore\n/// ```\n/// ~~~sh\n/// ~~~\n//// ```\n// ```\n/** ```\n * a\n * ``` */\n";
        assert_eq!(super::doctests_in(src), 4);
    }
}
