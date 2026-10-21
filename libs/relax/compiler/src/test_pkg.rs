//! `makepad test [options] <package> [--lib] [--tests] [--test <name>]... [--no-fail-fast]
//! [cargo feature flags] [-- <libtest args>]`: what `cargo test -p <package>` runs, under the JIT.
//! The lib's unit tests (the lib built cfg(test)) and the integration tests (the plain lib and
//! every tests/ target) are two programs, as under cargo; each is `makepad run --harness` in its
//! own process (run.rs), one after the other. Without a selection both run (cargo test's default
//! minus doc tests and the bins' unit tests).

pub fn cmd_test_package(args: &[String]) -> i32 {
    // options before the package go to each run unchanged
    let mut i = 0;
    let mut pre: Vec<String> = Vec::new();
    while i < args.len() && args[i].starts_with('-') && args[i] != "-p" && args[i] != "--package" {
        if matches!(args[i].as_str(), "--cfg" | "--report-to" | "--run-dir") && i + 1 < args.len() {
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
        eprintln!("usage: makepad test <package> [--lib] [--tests] [--test <name>] [--no-fail-fast] [cargo flags] [-- <test args>]");
        return 2;
    };
    let mut lib = false;
    let mut tests: Option<Vec<String>> = None;
    let mut no_fail_fast = false;
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
            "--no-fail-fast" => no_fail_fast = true,
            "--release" => {}
            "--features" | "-F" | "--cfg" | "--domain" => {
                flags.push(w.clone());
                if let Some(v) = words.next() {
                    flags.push(v.clone());
                }
            }
            _ => flags.push(w.clone()),
        }
    }
    let sess = std::sync::Arc::new(crate::Session::default());
    let (has_lib, targets) = match crate::resolve::test_targets(&sess, &crate::run::workspace_dir(), &package) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("makepad test: {e}");
            return 101;
        }
    };
    if !lib && tests.is_none() {
        lib = has_lib.is_some();
        tests = (!targets.is_empty()).then(Vec::new);
    }
    let mut runs: Vec<Vec<String>> = Vec::new();
    if lib {
        runs.push(vec!["--lib".to_string()]);
    }
    if let Some(t) = &tests {
        if t.is_empty() {
            if !targets.is_empty() {
                runs.push(vec!["--tests".to_string()]);
            }
        } else {
            runs.push(t.iter().flat_map(|n| ["--test".to_string(), n.clone()]).collect());
        }
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("makepad test: the loader's own path: {e}");
            return 101;
        }
    };
    let mut code = 0;
    for sel in runs {
        let mut a: Vec<String> = vec!["run".to_string(), "--harness".to_string()];
        if no_fail_fast {
            a.push("--no-fail-fast".to_string());
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
            code = 101;
            if !no_fail_fast {
                break;
            }
        }
    }
    code
}
