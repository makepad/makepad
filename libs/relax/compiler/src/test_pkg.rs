//! `makepad test [options] <package> [--lib] [--bins] [--bin <name>]... [--tests] [--test <name>]...
//! [--examples] [--example <name>]... [cargo feature flags] [-- <libtest args>]`: what `cargo test -p <package>` runs,
//! under the JIT. The lib's unit tests (the lib built cfg(test)) are a program of their own; the
//! selected bins' and examples' unit tests and integration tests (beside the plain lib) are one program, as
//! `cargo test` builds them in one go, so every target after the first restores its collected
//! image. Each target runs in a process of its own, one after the other (`makepad run --harness
//! --subject <its source>`, run.rs): its own main, statics, environment, threads, exit status
//! and errors, as under cargo. A file of the program that does not parse stops the run there, as
//! cargo builds no target then. Without a selection all of them run (cargo test's default minus
//! doc tests, which are counted and said).
//! A `harness = false` target runs as a plain program (`makepad run --plain-test --subject ..`),
//! as cargo runs it: its exit status is its verdict.
//! `--live` keeps one harness target (default: the lib, else the package's only harness target) running across source edits.
//! Every run builds with the cargo profile `cargo test` takes: `test` (debug assertions and
//! overflow checks as the workspace's `[profile.test]` / `[profile.dev]` set them, both on by
//! default), `release` with `--release`, another with `--profile <name>` (run.rs `--profile`,
//! resolve.rs `profile`). Before, every test ran release (relax issue 141).

pub fn cmd_test_package(args: &[String]) -> i32 {
    stop::install();
    // Loader options go before each run's package; feature flags go after it.
    let mut i = 0;
    let mut pre: Vec<String> = Vec::new();
    let mut flags: Vec<String> = Vec::new();
    let mut feature_flags: Vec<String> = Vec::new();
    let mut live = false;
    // `--release` / `--profile <name>`: the cargo profile (cargo test's `test` without either)
    let mut release = false;
    let mut profile: Option<String> = None;
    while i < args.len() && args[i].starts_with('-') && args[i] != "-p" && args[i] != "--package" {
        if args[i] == "--live" {
            live = true;
            i += 1;
            continue;
        }
        if args[i] == "--release" {
            release = true;
            i += 1;
            continue;
        }
        if let Some(p) = args[i].strip_prefix("--profile=") {
            profile = Some(p.to_string());
            i += 1;
            continue;
        }
        if args[i] == "--profile" {
            let Some(p) = args.get(i + 1) else {
                eprintln!("relax test: --profile needs a profile name");
                return 2;
            };
            profile = Some(p.clone());
            i += 2;
            continue;
        }
        if is_feature_flag(&args[i]) {
            let takes_value = matches!(args[i].as_str(), "--features" | "-F");
            feature_flags.push(args[i].clone());
            flags.push(args[i].clone());
            i += 1;
            if takes_value {
                let Some(v) = args.get(i) else {
                    eprintln!("relax test: {} needs a value", args[i - 1]);
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
        eprintln!("usage: {tool} test <package> [--lib] [--bins] [--bin <name>] [--tests] [--test <name>] [--examples] [--example <name>] [cargo flags] [-- <test args>]", tool = crate::tool_name());
        return 2;
    };
    let mut lib = false;
    let mut bins: Option<Vec<String>> = None;
    let mut tests: Option<Vec<String>> = None;
    let mut examples: Option<Vec<String>> = None;
    let mut all_examples = false;
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
                    eprintln!("relax test: --bin needs a target name");
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
                    eprintln!("relax test: --test needs a target name");
                    return 2;
                }
            },
            // examples built with `--test`: all of them, or the named ones (cargo test's default
            // tests those with `test = true`)
            "--examples" => {
                all_tests = true;
                all_examples = true;
                examples.get_or_insert_with(Vec::new);
            }
            "--example" => match words.next() {
                Some(e) => { examples.get_or_insert_with(Vec::new).push(e.clone()); selections += 1; }
                None => {
                    eprintln!("relax test: --example needs a target name");
                    return 2;
                }
            },
            // every target runs anyway: one failing says nothing about the next
            "--no-fail-fast" => {}
            "--release" => release = true,
            "--profile" => match words.next() {
                Some(p) => profile = Some(p.clone()),
                None => {
                    eprintln!("relax test: --profile needs a profile name");
                    return 2;
                }
            },
            s if s.starts_with("--profile=") => profile = Some(s[10..].to_string()),
            "--features" | "-F" => {
                let Some(v) = words.next() else {
                    eprintln!("relax test: {w} needs a value");
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
            s if s.starts_with("--example=") => {
                examples.get_or_insert_with(Vec::new).push(s[10..].to_string());
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
            || examples.is_some() || pre.iter().chain(&flags).any(|s| matches!(s.as_str(), "--tests" | "--bins" | "--bin" | "--test" | "--lib" | "--no-live" | "--workspace" | "--all-targets" | "-p" | "--package")
                || s.starts_with("--package=") || s.starts_with("--bin=") || s.starts_with("--test=")) {
            return live_usage();
        }
        pre.push("--live".to_string());
    }
    let profile = match (release, profile) {
        (true, Some(p)) if p != "release" => {
            eprintln!("error: conflicting usage of --profile={p} and --release");
            return 101;
        }
        (true, _) => "release".to_string(),
        (false, p) => p.unwrap_or_else(|| "test".to_string()),
    };
    // a profile the workspace does not define (or cannot resolve) is the command's error, once
    if let Err(e) = crate::resolve::profile(&crate::run::workspace_dir(), &profile) {
        eprintln!("relax test: {e}");
        return 101;
    }
    let sess = std::sync::Arc::new(crate::Session::default());
    let live_default = live && !lib && bins.is_none() && tests.is_none();
    let crate::resolve::TestTargets { lib: has_lib, bins: bin_targets, tests: targets, examples: example_targets, all_examples: every_example } = match crate::resolve::test_targets(&sess, &crate::run::workspace_dir(), &package,
        &feature_flags, bins.as_deref().unwrap_or_default(), tests.as_deref().unwrap_or_default(), examples.as_deref().unwrap_or_default()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("relax test: {e}");
            return 101;
        }
    };
    // `--examples`: every example, `test = true` or not (an `--example <name>` beside it is one of them)
    let example_targets = if all_examples {
        examples = Some(Vec::new());
        every_example
    } else {
        example_targets
    };
    if live_default {
        if has_lib.as_ref().is_some_and(|l| l.test) {
            lib = true;
        } else {
            let harness: Vec<_> = bin_targets.iter().filter(|t| t.2).map(|t| ("--bin", &t.0))
                .chain(targets.iter().filter(|t| t.2).map(|t| ("--test", &t.0))).collect();
            if harness.len() != 1 {
                let names = harness.iter().map(|(flag, name)| format!("{flag} {name}")).collect::<Vec<_>>().join(", ");
                eprintln!("relax test --live: package `{package}` has no lib and {} harness targets ({names}): select one", harness.len());
                return 2;
            }
            let (flag, name) = harness[0];
            if flag == "--bin" { bins = Some(vec![name.clone()]); }
            else { tests = Some(vec![name.clone()]); }
        }
    }
    if live && lib && has_lib.is_none() { return live_usage(); }
    let default_selection = !lib && bins.is_none() && tests.is_none() && examples.is_none();
    if default_selection {
        lib = has_lib.as_ref().is_some_and(|l| l.test);
        bins = (!bin_targets.is_empty()).then(Vec::new);
        tests = (!targets.is_empty()).then(Vec::new);
        // the examples with `test = true` (relax issue 120: cargo runs their tests, makepad test skipped them)
        examples = (!example_targets.is_empty()).then(Vec::new);
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
    for (sel, all) in [(&bins, &bin_targets), (&tests, &targets), (&examples, &example_targets)] {
        let Some(names) = sel else { continue };
        match chosen(all, names) {
            Ok(r) => runs.extend(r.into_iter().map(Some)),
            Err(e) => {
                eprintln!("relax test: {e}");
                return 101;
            }
        }
    }
    // the test program of the selected bin, integration test and example targets, as `cargo
    // test` builds them: each named (the `harness = false` ones too); `--bins` / `--examples`
    // say that the names after them are targets built with `--test` (resolve.rs parse_flags)
    let mut graph: Vec<String> = Vec::new();
    for (sel, all, flag) in [(&bins, &bin_targets, "--bin"), (&tests, &targets, "--test"), (&examples, &example_targets, "--example")] {
        let Some(names) = sel else { continue };
        let mut first = true;
        for t in all.iter().filter(|t| names.is_empty() || names.contains(&t.0)) {
            if first && flag != "--test" {
                graph.push(if flag == "--bin" { "--bins" } else { "--examples" }.to_string());
            }
            first = false;
            graph.extend([flag.to_string(), t.0.clone()]);
        }
    }
    if live && (runs.len() != 1 || runs[0].as_ref().is_some_and(|r| r.1)) { return live_usage(); }
    let exe = match crate::dirs::loader_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("relax test: the loader's own path: {e}");
            return 101;
        }
    };
    let pkg_dir = crate::resolve::package_dir(&sess, &crate::run::workspace_dir(), &package).unwrap_or_default();
    let mut code = 0;
    // a filter no target matched is no pass (a target exits NO_TESTS_EXIT when it matched none)
    let (mut matched, mut unmatched) = (false, false);
    // the links the runs start from go when this command ends (selfexe.rs launch_link)
    struct Launches;
    impl Drop for Launches {
        fn drop(&mut self) {
            crate::selfexe::remove_launch();
        }
    }
    let _launches = Launches;
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
        a.extend(["--profile".to_string(), profile.clone()]);
        a.extend(pre.iter().cloned());
        a.push(package.clone());
        a.extend(flags.iter().cloned());
        match &run {
            Some(_) => a.extend(graph.iter().cloned()),
            None => a.push("--lib".to_string()),
        }
        a.push("--".to_string());
        a.extend(prog.iter().cloned());
        // started from a link named as its program: its process has the program's name, as cargo's
        // test executable's has (selfexe.rs launch_link)
        let dd = a.iter().position(|w| w == "--").unwrap_or(a.len());
        let launch = crate::selfexe::launch_link(&exe, &crate::selfexe::program_name(&a[..dd], &package));
        let mut cmd = std::process::Command::new(launch.as_deref().unwrap_or(&exe));
        cmd.args(&a).env(makepad_remote::hot::TEST_PACKAGE_ENV, "1");
        // the run leads a process group of its own, which a signal to this command ends (`stop`)
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        let status = cmd.spawn().and_then(|mut child| {
            stop::started(child.id());
            let status = child.wait();
            stop::ended();
            status
        });
        let c = match status {
            Ok(s) => s.code().unwrap_or(101),
            Err(e) => {
                eprintln!("relax test: starting {}: {e}", exe.display());
                101
            }
        };
        // ended by a signal: the run it started has ended with it, and so does the command
        if let Some(sig) = stop::signalled() { return 128 + sig; }
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
                eprintln!("relax test: {n} doc test{} not run ({tool} test runs no doc tests; `cargo test --doc` does)", if n == 1 { "" } else { "s" }, tool = crate::tool_name());
            }
        }
    }
    code
}

/// A signal that ends `makepad test` (a driver's terminate: SIGTERM; SIGINT, SIGHUP) ends the
/// run it started, and everything that run started, before the command exits as the signal asks
/// (128 + the signal). The run leads a process group of its own and the handler ends that group,
/// as the build host ends the cargo it started (reload_host.rs end_group). Before, the command
/// alone ended: a `--live` session's run kept watching the sources and testing every save, about
/// 1 GB each (relax issue 88).
mod stop {
    use std::sync::atomic::{AtomicI32, AtomicU32, Ordering::SeqCst};

    /// the running child's pid (0: none)
    static CHILD: AtomicU32 = AtomicU32::new(0);
    /// the signal that ended the command (0: none)
    static SIGNAL: AtomicI32 = AtomicI32::new(0);

    #[cfg(unix)]
    extern "C" fn on_signal(sig: i32) {
        // async-signal-safe: atomics and kill(2)
        SIGNAL.store(sig, SeqCst);
        let pid = CHILD.load(SeqCst);
        if pid != 0 { crate::reload::host::end_group(pid); }
    }

    pub(super) fn install() {
        #[cfg(unix)]
        {
            extern "C" { fn signal(sig: i32, handler: usize) -> usize; }
            // SIGHUP, SIGINT, SIGTERM: the same numbers on Linux and macOS
            for sig in [1, 2, 15] {
                // SAFETY: the handler does only async-signal-safe work
                unsafe { signal(sig, on_signal as extern "C" fn(i32) as usize) };
            }
        }
    }

    /// The run `pid` started: a signal from now on ends its group, and one that came before
    /// it was recorded ends it now.
    pub(super) fn started(pid: u32) {
        CHILD.store(pid, SeqCst);
        if SIGNAL.load(SeqCst) != 0 { crate::reload::host::end_group(pid); }
    }

    /// The run was waited for: its pid is no longer this command's to signal.
    pub(super) fn ended() {
        CHILD.store(0, SeqCst);
    }

    pub(super) fn signalled() -> Option<i32> {
        Some(SIGNAL.load(SeqCst)).filter(|&s| s != 0)
    }
}

fn live_usage() -> i32 {
    eprintln!("usage: {tool} test --live <package> [--lib | --bin <name> | --test <name>] [-- <test args>] (exactly one harness target; default: the lib, else the package's only one)", tool = crate::tool_name());
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
