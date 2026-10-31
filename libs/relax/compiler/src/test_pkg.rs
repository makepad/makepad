//! `makepad test [options] <package> [--lib] [--bins] [--bin <name>]... [--tests] [--test <name>]...
//! [cargo feature flags] [-- <libtest args>]`: what `cargo test -p <package>` runs,
//! under the JIT. The lib's unit tests (the lib built cfg(test)), the bins' unit tests, and the
//! integration tests (the plain lib and every tests/ target) are separate programs, as under cargo;
//! each is `makepad run --harness` in its own process (run.rs), one after the other. Without a
//! selection all three run (cargo test's default minus doc tests, which are counted and said).
//! A `harness = false` target runs as a program of its own (`makepad run --plain-test`), as cargo
//! runs it: its exit status is its verdict.

pub fn cmd_test_package(args: &[String]) -> i32 {
    // options before the package go to each run unchanged
    let mut i = 0;
    let mut pre: Vec<String> = Vec::new();
    while i < args.len() && args[i].starts_with('-') && args[i] != "-p" && args[i] != "--package" {
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
    let mut flags: Vec<String> = Vec::new();
    let mut prog: Vec<String> = Vec::new();
    let mut words = args[i + 1..].iter();
    while let Some(w) = words.next() {
        match w.as_str() {
            "--" => {
                prog.extend(words.by_ref().cloned());
                break;
            }
            "--lib" => lib = true,
            "--bins" => {
                bins.get_or_insert_with(Vec::new);
            }
            "--bin" => match words.next() {
                Some(b) => bins.get_or_insert_with(Vec::new).push(b.clone()),
                None => {
                    eprintln!("makepad test: --bin needs a target name");
                    return 2;
                }
            },
            "--tests" => {
                tests.get_or_insert_with(Vec::new);
            }
            "--test" => match words.next() {
                Some(t) => tests.get_or_insert_with(Vec::new).push(t.clone()),
                None => {
                    eprintln!("makepad test: --test needs a target name");
                    return 2;
                }
            },
            // every target runs anyway: one failing says nothing about the next
            "--no-fail-fast" => {}
            "--release" => {}
            "--features" | "-F" | "--cfg" | "--domain" => {
                flags.push(w.clone());
                if let Some(v) = words.next() {
                    flags.push(v.clone());
                }
            }
            s if s.starts_with("--bin=") => {
                bins.get_or_insert_with(Vec::new).push(s[6..].to_string());
            }
            s if s.starts_with("--test=") => {
                tests.get_or_insert_with(Vec::new).push(s[7..].to_string());
            }
            _ => flags.push(w.clone()),
        }
    }
    let sess = std::sync::Arc::new(crate::Session::default());
    let (has_lib, bin_targets, targets) = match crate::resolve::test_targets(&sess, &crate::run::workspace_dir(), &package) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("makepad test: {e}");
            return 101;
        }
    };
    let default_selection = !lib && bins.is_none() && tests.is_none();
    if default_selection {
        lib = has_lib.is_some();
        bins = (!bin_targets.is_empty()).then(Vec::new);
        tests = (!targets.is_empty()).then(Vec::new);
    }
    // each run: the selection, and for a `harness = false` target its source path (it runs as a
    // program of its own, its exit status the verdict, as cargo runs it)
    let mut runs: Vec<(Vec<String>, Option<String>)> = Vec::new();
    if lib {
        runs.push((vec!["--lib".to_string()], None));
    }
    // (harness targets, plain targets) of a selection: every target for an empty one
    let split = |all: &[(String, String, bool)], names: &[String]| -> Result<(Vec<String>, Vec<(String, String)>), String> {
        for n in names {
            if !all.iter().any(|t| &t.0 == n) {
                return Err(format!("no target named `{n}` in package `{package}`"));
            }
        }
        let chosen = all.iter().filter(|t| names.is_empty() || names.contains(&t.0));
        let (h, p): (Vec<_>, Vec<_>) = chosen.partition(|t| t.2);
        Ok((h.into_iter().map(|t| t.0.clone()).collect(), p.into_iter().map(|t| (t.0.clone(), t.1.clone())).collect()))
    };
    if let Some(b) = &bins {
        let (h, p) = match split(&bin_targets, b) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("makepad test: {e}");
                return 101;
            }
        };
        if !h.is_empty() {
            let mut sel = vec!["--bins".to_string()];
            if !b.is_empty() {
                sel.extend(h.iter().flat_map(|n| ["--bin".to_string(), n.clone()]));
            }
            runs.push((sel, None));
        }
        for (n, path) in p {
            runs.push((vec!["--bins".to_string(), "--bin".to_string(), n], Some(path)));
        }
    }
    if let Some(t) = &tests {
        let (h, p) = match split(&targets, t) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("makepad test: {e}");
                return 101;
            }
        };
        if !h.is_empty() {
            if t.is_empty() {
                runs.push((vec!["--tests".to_string()], None));
            } else {
                runs.push((h.iter().flat_map(|n| ["--test".to_string(), n.clone()]).collect(), None));
            }
        }
        for (n, path) in p {
            runs.push((vec!["--test".to_string(), n], Some(path)));
        }
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("makepad test: the loader's own path: {e}");
            return 101;
        }
    };
    let pkg_dir = crate::resolve::package_dir(&sess, &crate::run::workspace_dir(), &package).unwrap_or_default();
    let mut code = 0;
    for (sel, plain) in runs {
        let mut a: Vec<String> = vec!["run".to_string()];
        match &plain {
            Some(path) => {
                let rel = path.strip_prefix(pkg_dir.as_str()).map_or(path.as_str(), |r| r.trim_start_matches(['/', '\\']));
                eprintln!("     Running {rel} (harness = false)");
                a.push("--plain-test".to_string());
                a.push(path.clone());
            }
            None => a.push("--harness".to_string()),
        }
        a.extend(pre.iter().cloned());
        a.push(package.clone());
        a.extend(flags.iter().cloned());
        a.extend(sel);
        a.push("--".to_string());
        a.extend(prog.iter().cloned());
        let status = std::process::Command::new(&exe).args(&a).status();
        let c = match status {
            Ok(s) => s.code().unwrap_or(101),
            Err(e) => {
                eprintln!("makepad test: starting {}: {e}", exe.display());
                101
            }
        };
        if c != 0 {
            if plain.is_some() {
                eprintln!("error: test failed (harness = false target exited with status {c})");
            }
            code = 101;
        }
    }
    // cargo test's default selection also runs the lib's doc tests; makepad test does not: say
    // how many there are, so the counts compare honestly
    if default_selection {
        if let Some(lib_root) = &has_lib {
            let n = count_doctests(lib_root);
            if n > 0 {
                eprintln!("makepad test: {n} doc test{} not run (makepad test runs no doc tests; `cargo test --doc` does)", if n == 1 { "" } else { "s" });
            }
        }
    }
    code
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
