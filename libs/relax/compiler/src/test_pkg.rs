//! `makepad test [options] <package> [--lib] [--bins] [--bin <name>]... [--tests] [--test <name>]...
//! [--examples] [--example <name>]... [cargo feature flags] [-- <libtest args>]`: what `cargo test -p <package>` runs,
//! under the JIT. The lib's unit tests (the lib built cfg(test)) are a program of their own; the
//! selected bins' and examples' unit tests and integration tests (beside the plain lib) are one program, as
//! `cargo test` builds them in one go, so every target after the first restores its collected
//! image. Each target runs in a process of its own, one after the other (`makepad run --harness
//! --subject <its source>`, run.rs): its own main, statics, environment, threads, exit status
//! and errors, as under cargo. A file of the program that does not parse stops the run there, as
//! cargo builds no target then. Without a selection all of them run (cargo test's default minus
//! doc tests, which are counted and said). `--tests` is cargo's set (the lib and bins as unit tests,
//! the integration tests, the examples with `test = true`), and a selector that matches no target
//! is an error that names the package's targets.
//! A `harness = false` target runs as a plain program (`makepad run --plain-test --subject ..`),
//! as cargo runs it: its exit status is its verdict.
//! `--live` keeps one harness target (default: the lib, else the package's only harness target) running across source edits.
//! Every run builds with the cargo profile `cargo test` takes: `test` (debug assertions and
//! overflow checks as the workspace's `[profile.test]` / `[profile.dev]` set them, both on by
//! default), `release` with `--release`, another with `--profile <name>` (run.rs `--profile`,
//! resolve.rs `profile`). Before, every test ran release (relax issue 141).

pub fn cmd_test_package(args: &[String], grants: &[String]) -> i32 {
    stop::install();
    // Loader options go before each run's package; feature flags go after it. This invocation's
    // grants (`--allow-*`, which the session took out of `args`) are each run's: words of its
    // command line (options.rs GRANTS).
    let mut i = 0;
    let mut pre: Vec<String> = grants.to_vec();
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
    let mut all_bins = false;
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
                all_bins = true;
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
    let package_targets = match crate::resolve::test_targets(&sess, &crate::run::workspace_dir(), &package,
        &feature_flags, bins.as_deref().unwrap_or_default(), tests.as_deref().unwrap_or_default(), examples.as_deref().unwrap_or_default()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("relax test: {e}");
            return 101;
        }
    };
    let crate::resolve::TestTargets { lib: has_lib, bins: bin_targets, tests: targets, all_bins: every_bin, .. } = &package_targets;
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
    let asked = TestSelection { lib, bins: bins.as_deref(), tests: tests.as_deref(), examples: examples.as_deref(), all_bins, all_tests, all_examples };
    let SelectedTests { runs, graph, runs_tests, default_selection } = match select_tests(&package, asked, &package_targets) {
        Ok(selected) => selected,
        Err((named, e)) => {
            eprintln!("{e}");
            // under --live a selector that matched nothing is a usage error; a missing named target is not
            return if live && !named { 2 } else { 101 };
        }
    };
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
    // cargo builds every bin of the package beside an integration test (its CARGO_BIN_EXE_<name>),
    // and a bin rustc rejects fails `cargo test --test <name>`. A test starts the bin with
    // `makepad run --unattended`, which checks types before it runs and borrows only lazily
    // beside it, and a bin no test starts is never checked at all: each bin is checked here as
    // `makepad check` checks it, in a process of its own beside the runs, and its errors fail the
    // command at its end, as a path dependency's do (check.rs start_test_deps). The bins are no
    // crates of the test program.
    let mut bin_checks = BinChecks(Vec::new());
    if runs_tests && !live {
        for (name, _, _) in every_bin {
            match start_bin_check(&exe, &profile, &pre, &package, name, &flags) {
                Ok(child) => bin_checks.0.push(BinCheck { name: name.clone(), child: Some(child), t0: std::time::Instant::now() }),
                Err(e) => {
                    eprintln!("relax test: starting the check of bin `{name}`: {e}");
                    return 101;
                }
            }
        }
    }
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
        // ended by a signal: the run it started has ended with it, and so does the command
        if let Some(sig) = stop::signalled() { return 128 + sig; }
        let what = match &run {
            Some((path, _)) => path.strip_prefix(pkg_dir.as_str()).map_or(path.as_str(), |r| r.trim_start_matches(['/', '\\'])).to_string(),
            None => "the lib's unit tests".to_string(),
        };
        let c = match status {
            Ok(s) => match s.code() {
                Some(c) => c,
                // ended by a signal of its own (a crash, a kill from outside): it printed no test
                // result or panic for it, so this line is the only word of it (a bucket's listing
                // printed its list, then failed and said nothing else: 11-18)
                None => {
                    eprintln!("relax test: the run of {what} ended by {}: no test result is from it", ended_by(&s));
                    101
                }
            },
            Err(e) => {
                eprintln!("relax test: starting {}: {e}", exe.display());
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
            } else if c != 101 {
                // (the harness's failing tests end the run so, named above it)
                eprintln!("relax test: the run of {what} exited with status {c}");
            }
            code = 101;
        }
    }
    if finish_bin_checks(&mut bin_checks, &package, pre.iter().any(|w| w == "--verbose")) {
        code = 101;
    }
    if let Some(sig) = stop::signalled() { return 128 + sig; }
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

/// Requested selectors; an empty name list selects every target of that kind.
#[derive(Default)]
struct TestSelection<'a> {
    lib: bool,
    bins: Option<&'a [String]>,
    tests: Option<&'a [String]>,
    examples: Option<&'a [String]>,
    all_bins: bool,
    all_tests: bool,
    all_examples: bool,
}

struct SelectedTests {
    // None: the lib's unit tests; Some: a source path and whether it has no harness.
    runs: Vec<Option<(String, bool)>>,
    graph: Vec<String>,
    runs_tests: bool,
    default_selection: bool,
}

/// Selects runs and their graph together, rejecting empty explicit selectors before a build. The
/// error says whether it is a named target that is missing (`true`) or a selector that matched nothing.
fn select_tests(package: &str, asked: TestSelection<'_>, targets: &crate::resolve::TestTargets) -> Result<SelectedTests, (bool, String)> {
    let error = |named: bool, what: &str| {
        let mut names = Vec::new();
        if targets.lib.is_some() { names.push("--lib".to_string()); }
        for (flag, all) in [("--bin", &targets.all_bins), ("--test", &targets.tests), ("--example", &targets.all_examples)] {
            names.extend(all.iter().map(|t| format!("{flag} {}", t.0)));
        }
        let names = if names.is_empty() { "none".to_string() } else { names.join(", ") };
        (named, format!("relax test: {what} in package `{package}`; its test targets: {names}"))
    };
    let lib_test = targets.lib.as_ref().is_some_and(|l| l.test);
    let default_selection = !asked.lib && !asked.all_bins && !asked.all_tests && !asked.all_examples
        && asked.bins.is_none() && asked.tests.is_none() && asked.examples.is_none();
    // `--tests` takes cargo's test-enabled set; named selectors add to it in test_targets.
    let all_tests = default_selection || asked.all_tests;
    let mut runs = Vec::new();
    if asked.lib || (all_tests && lib_test) { runs.push(None); }
    // `--bins` / `--examples`: every bin / example, `test = true` or not (cargo's "all binary
    // targets" and "all example targets"), names beside them included
    let bin_targets = if asked.all_bins { &targets.all_bins } else { &targets.bins };
    let example_targets = if asked.all_examples { &targets.all_examples } else { &targets.examples };
    let mut graph = Vec::new();
    let mut runs_tests = false;
    // A kind's all-flag drops its names, as cargo's does (`--test nope --tests` runs the whole
    // set); a name of another kind is still checked (`--bin nope --tests` fails).
    for (names, all, flag, entire) in [
        (asked.bins, bin_targets, "--bin", asked.all_bins),
        (asked.tests, &targets.tests, "--test", asked.all_tests),
        (asked.examples, example_targets, "--example", asked.all_examples),
    ] {
        if !all_tests && !entire && names.is_none() { continue; }
        let names = if entire { &[][..] } else { names.unwrap_or_default() };
        for n in names {
            if !all.iter().any(|t| &t.0 == n) { return Err(error(true, &format!("no target named `{n}`"))); }
        }
        let chosen: Vec<_> = all.iter().filter(|t| all_tests || entire || names.is_empty() || names.contains(&t.0)).collect();
        // The graph stays in target order; the runs keep harness targets before plain ones.
        if !chosen.is_empty() {
            if flag == "--test" { runs_tests = true; }
            else { graph.push(if flag == "--bin" { "--bins" } else { "--examples" }.to_string()); }
        }
        for t in &chosen { graph.extend([flag.to_string(), t.0.clone()]); }
        let (h, p): (Vec<_>, Vec<_>) = chosen.into_iter().partition(|t| t.2);
        runs.extend(h.into_iter().chain(p).map(|t| Some((t.1.clone(), !t.2))));
    }
    for (missing, what) in [
        (asked.lib && targets.lib.is_none(), "no lib target"),
        (asked.all_bins && bin_targets.is_empty(), "no bin target"),
        (asked.all_examples && targets.all_examples.is_empty(), "no example target"),
        (asked.all_tests && !lib_test && targets.bins.is_empty() && targets.tests.is_empty() && targets.examples.is_empty(), "no test target"),
    ] {
        if missing { return Err(error(false, what)); }
    }
    Ok(SelectedTests { runs, graph, runs_tests, default_selection })
}

/// The checks of the package's bins beside the runs (start_bin_check). A return before their
/// join (finish_bin_checks) ends them with the command.
struct BinChecks(Vec<BinCheck>);

struct BinCheck {
    name: String,
    child: Option<std::process::Child>,
    t0: std::time::Instant,
}

impl Drop for BinChecks {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let Some(mut child) = c.child.take() else { continue };
            // the check leads a process group of its own (start_bin_check)
            #[cfg(unix)]
            crate::reload::host::end_group(child.id());
            #[cfg(not(unix))]
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// `makepad check --root-only <package> --bin <bin>` with the runs' profile, loader options and
/// feature flags: the bin crate's bodies type- and borrow-checked as rustc builds the bin, the
/// crates under it read for their items only. Its output is kept for finish_bin_checks.
fn start_bin_check(exe: &std::path::Path, profile: &str, pre: &[String], package: &str, bin: &str, flags: &[String]) -> std::io::Result<std::process::Child> {
    let mut a: Vec<String> = ["check", "--no-serve", "--root-only", "--profile", profile].iter().map(|w| w.to_string()).collect();
    // the runs' loader options, but a report and the run's own words (finish_bin_checks says
    // what the check found)
    let mut words = pre.iter();
    while let Some(w) = words.next() {
        match w.as_str() {
            "--report-to" => { words.next(); }
            "--verbose" | "--quiet" | "--silent" => {}
            _ => a.push(w.clone()),
        }
    }
    a.extend([package.to_string(), "--bin".to_string(), bin.to_string()]);
    // what the runs take after the package that the build of the bin depends on
    let mut words = flags.iter();
    while let Some(w) = words.next() {
        if matches!(w.as_str(), "--cfg" | "--features" | "-F") {
            a.push(w.clone());
            a.extend(words.next().cloned());
        } else if is_feature_flag(w) || matches!(w.as_str(), "--offline" | "--frozen") {
            a.push(w.clone());
        }
    }
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&a).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn()
}

/// Waits for the bins' checks and prints what they found, rustc's way: their diagnostics, and a
/// bin with errors as cargo names one it could not build. True: a bin did not pass.
fn finish_bin_checks(checks: &mut BinChecks, package: &str, verbose: bool) -> bool {
    let mut failed = false;
    for c in &mut checks.0 {
        let Some(child) = c.child.take() else { continue };
        let t_join = std::time::Instant::now();
        // a signal now ends the check (its group), as it ends a run
        stop::started(child.id());
        let out = child.wait_with_output();
        stop::ended();
        let (status, text) = match out {
            Ok(o) => (o.status.code(), format!("{}{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout))),
            Err(e) => (None, format!("relax test: waiting for the check of bin `{}`: {e}\n", c.name)),
        };
        if stop::signalled().is_some() { return true; }
        // the check's own lines (counts, timings) are Relax's, not the bin's
        let own = |l: &str| l.starts_with("relax ") || l.starts_with("relax:");
        let mut errors = None;
        for l in text.lines() {
            if verbose || !own(l) {
                eprintln!("{l}");
            }
            if let Some(r) = l.strip_prefix("relax check: ").filter(|r| r.contains(" body owners of the checked crates, ")) {
                errors = r.strip_suffix(" errors").and_then(|r| r.rsplit(' ').next()).and_then(|n| n.parse::<usize>().ok());
            }
        }
        if verbose {
            eprintln!("relax test: bin `{}` checked beside the runs: done {:.0} ms after its start, the end waited {:.0} ms for it", c.name, c.t0.elapsed().as_secs_f64() * 1e3, t_join.elapsed().as_secs_f64() * 1e3);
        }
        if status == Some(0) { continue; }
        failed = true;
        match errors.filter(|&n| n > 0) {
            Some(n) => eprintln!("error: could not compile `{package}` (bin \"{}\") due to {n} previous error{} (`{tool} test` checks the package's bins as cargo builds them beside its integration tests)",
                c.name, if n == 1 { "" } else { "s" }, tool = crate::tool_name()),
            None => eprintln!("error: could not check bin `{}` of `{package}`: `{tool} check` exited with {}", c.name,
                status.map_or("a signal".to_string(), |s| format!("status {s}")), tool = crate::tool_name()),
        }
    }
    failed
}

/// A signal that ends `makepad test` (a driver's terminate: SIGTERM; SIGINT, SIGHUP) ends the
/// run it started, and everything that run started, before the command exits as the signal asks
/// (128 + the signal). The run leads a process group of its own and the handler ends that group,
/// as the build host ends the cargo it started (reload_host.rs end_group). Before, the command
/// alone ended: a `--live` session's run kept watching the sources and testing every save, about
/// 1 GB each (relax issue 88).
/// How a run that has no exit code ended: its signal (and whether it dumped core) on unix.
fn ended_by(s: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return format!("signal {sig}{}", if s.core_dumped() { " (core dumped)" } else { "" });
        }
    }
    format!("{s}")
}

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
    fn a_selector_that_matches_nothing_names_the_real_targets() {
        use super::{select_tests, TestSelection};
        use crate::resolve::{LibTarget, TestTargets};

        let bin = ("app".to_string(), "src/main.rs".to_string(), true);
        let x = ("x".to_string(), "tests/x.rs".to_string(), true);
        let y = ("y".to_string(), "tests/y.rs".to_string(), false);
        let mut targets = TestTargets {
            lib: None, bins: vec![bin.clone()], tests: vec![x.clone()], examples: Vec::new(),
            all_bins: vec![bin.clone()], all_examples: Vec::new(),
        };
        let select = |asked: TestSelection<'_>, targets: &TestTargets| select_tests("sample", asked, targets).map(|s| s.runs).map_err(|e| e.1);
        assert_eq!(select(TestSelection { lib: true, ..Default::default() }, &targets),
            Err("relax test: no lib target in package `sample`; its test targets: --bin app, --test x".to_string()),
            "--lib without a lib names the bin and integration test");

        targets.lib = Some(LibTarget { path: "src/lib.rs".to_string(), test: true, doctest: true });
        assert_eq!(select(TestSelection { lib: true, ..Default::default() }, &targets), Ok(vec![None]),
            "--lib selects the lib alone");
        targets.bins.clear();
        targets.all_bins.clear();
        targets.tests.clear();
        assert_eq!(select(TestSelection { all_bins: true, bins: Some(&[]), ..Default::default() }, &targets),
            Err("relax test: no bin target in package `sample`; its test targets: --lib".to_string()),
            "--bins without bins names the lib");
        assert_eq!(select(TestSelection { all_examples: true, examples: Some(&[]), ..Default::default() }, &targets),
            Err("relax test: no example target in package `sample`; its test targets: --lib".to_string()),
            "--examples without examples names the lib");
        assert_eq!(select(TestSelection { all_tests: true, tests: Some(&[]), ..Default::default() }, &targets), Ok(vec![None]),
            "--tests includes unit tests in a lib-only package");
        targets.lib.as_mut().unwrap().test = false;
        assert_eq!(select(TestSelection { all_tests: true, ..Default::default() }, &targets),
            Err("relax test: no test target in package `sample`; its test targets: --lib".to_string()),
            "--tests respects test = false on the lib");
        assert_eq!(select(TestSelection { lib: true, ..Default::default() }, &targets), Ok(vec![None]),
            "an explicit --lib still selects a lib with test = false");
        targets.lib.as_mut().unwrap().test = true;
        targets.bins = vec![bin.clone()];
        targets.all_bins = vec![bin];
        targets.tests = vec![x, y];
        let mut expected = vec![None, Some(("src/main.rs".to_string(), false)),
            Some(("tests/x.rs".to_string(), false)), Some(("tests/y.rs".to_string(), true))];
        assert_eq!(select(TestSelection { all_tests: true, ..Default::default() }, &targets), Ok(expected.clone()),
            "--tests selects the lib, bin and both integration tests, including a plain target");
        assert_eq!(select(TestSelection { all_tests: true, tests: Some(&["x".to_string()]), ..Default::default() }, &targets), Ok(expected.clone()),
            "a named integration test beside --tests never narrows its set");
        let missing = "relax test: no target named `nope` in package `sample`; its test targets: --lib, --bin app, --test x, --test y".to_string();
        assert_eq!(select(TestSelection { tests: Some(&["nope".to_string()]), ..Default::default() }, &targets), Err(missing.clone()),
            "an unknown named target lists all available selectors");
        assert_eq!(select(TestSelection { all_tests: true, tests: Some(&["nope".to_string()]), ..Default::default() }, &targets), Ok(expected.clone()),
            "--tests drops the integration test names beside it, as cargo does: an unknown one is no error");
        assert_eq!(select(TestSelection { all_tests: true, bins: Some(&["nope".to_string()]), ..Default::default() }, &targets), Err(missing),
            "--tests still checks a bin name beside it");
        assert_eq!(select(TestSelection { all_bins: true, bins: Some(&["nope".to_string()]), ..Default::default() }, &targets),
            Ok(vec![Some(("src/main.rs".to_string(), false))]), "--bins drops the bin names beside it");

        let example = ("demo".to_string(), "examples/demo.rs".to_string(), true);
        targets.examples = vec![example.clone()];
        targets.all_examples = vec![example, ("plain".to_string(), "examples/plain.rs".to_string(), false)];
        expected.push(Some(("examples/demo.rs".to_string(), false)));
        let selected = select_tests("sample", TestSelection { all_tests: true, ..Default::default() }, &targets).expect("--tests has enabled targets");
        assert_eq!(selected.runs, expected, "--tests includes only the examples returned as test-enabled or named");
        assert_eq!(selected.graph, ["--bins", "--bin", "app", "--test", "x", "--test", "y", "--examples", "--example", "demo"],
            "the graph includes every selected target with the existing build flags");
        assert!(selected.runs_tests, "integration tests still require the separate bin checks");
        assert!(!selected.default_selection, "--tests does not report default-selection doc tests");
        assert_eq!(select(TestSelection::default(), &targets), Ok(expected),
            "the default selection still includes every test-enabled target");
        assert_eq!(select(TestSelection { tests: Some(&["nope".to_string()]), ..Default::default() }, &targets),
            Err("relax test: no target named `nope` in package `sample`; its test targets: --lib, --bin app, --test x, --test y, --example demo, --example plain".to_string()),
            "the alternatives include examples that can be selected explicitly even with test = false");
        assert_eq!(select(TestSelection { all_examples: true, ..Default::default() }, &targets),
            Ok(vec![Some(("examples/demo.rs".to_string(), false)), Some(("examples/plain.rs".to_string(), true))]),
            "--examples selects every example without selecting the lib, bins or integration tests");
        targets.all_bins.push(("tool".to_string(), "src/bin/tool.rs".to_string(), true));
        assert_eq!(select(TestSelection { all_bins: true, bins: Some(&["app".to_string()]), ..Default::default() }, &targets),
            Ok(vec![Some(("src/main.rs".to_string(), false)), Some(("src/bin/tool.rs".to_string(), false))]),
            "--bins takes every bin, a test = false one too, and a name beside it does not narrow it");
        assert!(select_tests("sample", TestSelection { tests: Some(&["nope".to_string()]), ..Default::default() }, &targets).is_err_and(|e| e.0),
            "a missing named target is told apart from a selector that matched nothing");

        let empty = TestTargets { lib: None, bins: Vec::new(), tests: Vec::new(), examples: Vec::new(), all_bins: Vec::new(), all_examples: Vec::new() };
        assert_eq!(select(TestSelection::default(), &empty), Ok(Vec::new()),
            "no selector on an empty package remains a successful empty selection");
        assert_eq!(select(TestSelection { all_tests: true, ..Default::default() }, &empty),
            Err("relax test: no test target in package `sample`; its test targets: none".to_string()),
            "an explicit --tests on an empty package is an error with no alternatives");
        assert!(select_tests("sample", TestSelection { all_bins: true, bins: Some(&[]), ..Default::default() }, &empty).is_err_and(|e| !e.0),
            "a selector that matched nothing is no missing named target (under --live a usage error)");
    }

    #[test]
    fn doctests_counted_as_rustdoc_reads_them() {
        let src = "/// ```\n/// assert!(true);\n/// ```\n/// ```text\n/// no\n/// ```\n//! ```rust,no_run\n//! x\n//! ```\n/// ```ignore\n/// ```\n/// ~~~sh\n/// ~~~\n//// ```\n// ```\n/** ```\n * a\n * ``` */\n";
        assert_eq!(super::doctests_in(src), 4);
    }
}
