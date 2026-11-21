// A test binary run with name filters that run no test exits as libtest's does: 0, with its usual
// `running` and `test result` lines and nothing on stderr; so does one whose filter matches only
// ignored tests. Each test here runs its own binary again with filters (a test binary can, as
// cargo's can) and prints what that run's parent sees. `makepad test`'s targets exited 87 there
// (its own "a filter ran no test" verdict, which only `makepad test` gives over its targets): the
// name telling a target it was one of `makepad test`'s reached the children of its tests too.
// Differential file in the package form (agree.sh pkg_diff_of): `| ` lines must match under
// rustc --test and `makepad test <package>`.

use std::process::Command;

const CHILD: &str = "UNMATCHED_FILTER_CHILD";

fn run_again(args: &[&str]) {
    let out = Command::new(std::env::current_exe().unwrap()).args(args).arg("--test-threads=1").env(CHILD, "1").output().unwrap();
    println!("| {:?}: status {:?}", args, out.status.code());
    for l in String::from_utf8_lossy(&out.stdout).lines() {
        if l.starts_with("running ") {
            println!("| {l}");
        }
        if let Some(r) = l.strip_prefix("test result: ") {
            println!("| test result: {}", r.split("; finished in").next().unwrap());
        }
    }
    for l in String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.starts_with("error")) {
        println!("| stderr: {l}");
    }
}

#[test]
fn a01_filter_matches_no_test() {
    if std::env::var_os(CHILD).is_none() {
        run_again(&["no_test_has_this_name"]);
        run_again(&["a03_runs", "--exact", "--skip", "a03"]);
    }
}

#[test]
fn a02_filter_matches_only_ignored_tests() {
    if std::env::var_os(CHILD).is_none() {
        run_again(&["a04_ignored"]);
    }
}

#[test]
fn a03_runs() {
    if std::env::var_os(CHILD).is_none() {
        run_again(&["a03_runs"]);
    }
}

#[test]
#[ignore]
fn a04_ignored() {}
