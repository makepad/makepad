//! Lint levels follow rustc's scoping (rustc_lint levels.rs: a level per HIR node): a real lint
//! (`unused_mut`, which the borrow checker reports) under attributes on statements, block
//! expressions, arms, parameters, closures' statements, items, impls, modules (outer and inner),
//! the crate root, `#[expect]`, and `--cap-lints`. Each case runs alone in a child process (the
//! lint sink is the process's).

fn run_alone(name: &str, child: &str, flags: &str) {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1", "--format=terse"])
        .env(child, "1").env("MAKEPAD_RUN_LINT_FLAGS", flags)
        .output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
}

/// The bindings `unused_mut` named, as (errors, warnings).
fn unused_mut_in(source: &str) -> (Vec<String>, Vec<String>) {
    crate::lint::enable();
    let (errors, unchecked) = super::mir_source_tests::borrowck_source_checked(source, 2021);
    assert!(errors.is_empty() && unchecked.is_empty(), "{errors:?} {unchecked:?}");
    let (lint_errors, warnings) = crate::lint::take();
    let names = |v: Vec<String>| -> Vec<String> {
        let mut n: Vec<String> = v.iter().filter(|l| l.ends_with("[unused_mut]")).filter_map(|l| l.split("(`").nth(1).and_then(|r| r.split('`').next()).map(str::to_string)).collect();
        n.sort();
        n
    };
    (names(lint_errors), names(warnings))
}

/// widgets lib.rs:148 (`#[allow(unused_mut)] let mut window = ..`, a false `unused_mut` that
/// failed the gates) and the other scopes inside a crate: only what no attribute allows warns.
#[test]
fn unused_mut_follows_the_attributes_around_it() {
    const CHILD: &str = "RELAX_LINT_SCOPE_TEST";
    if std::env::var(CHILD).is_err() {
        return run_alone("borrowck::lint_scope_tests::unused_mut_follows_the_attributes_around_it", CHILD, "");
    }
    let (errors, warnings) = unused_mut_in(r#"
        pub fn stmt() { #[allow(unused_mut)] let mut a_stmt = 1; let _ = a_stmt; }
        pub fn block() { #[allow(unused_mut)] { let mut b_block = 1; let _ = b_block; } }
        pub fn arm(o: Option<u32>) -> u32 { match o { #[allow(unused_mut)] Some(mut c_arm) => c_arm, None => 0 } }
        pub fn param(#[allow(unused_mut)] mut d_param: u32) -> u32 { d_param }
        #[allow(unused_mut)]
        pub fn item() { let mut e_item = 1; let _ = e_item; }
        pub struct S;
        #[allow(unused_mut)]
        impl S { pub fn m(&self) { let mut f_impl = 1; let _ = f_impl; } }
        #[allow(unused_mut)]
        pub mod outer { pub fn g() { let mut g_mod = 1; let _ = g_mod; } }
        pub mod inner { #![allow(unused_mut)] pub fn h() { let mut h_mod_inner = 1; let _ = h_mod_inner; } }
        pub fn closure() -> u32 { let k = || { #[allow(unused_mut)] let mut i_closure = 1; i_closure }; k() }
        pub fn expect() { #[expect(unused_mut)] let mut j_expect = 1; let _ = j_expect; }
        pub fn control() { let mut k_control = 1; let _ = k_control; }
        #[allow(unused_mut)]
        pub fn warn_inside() { #[warn(unused_mut)] let mut l_warn = 1; let _ = l_warn; }
        pub fn deny() { #[deny(unused_mut)] let mut m_deny = 1; let _ = m_deny; }
    "#);
    assert_eq!(warnings, ["k_control", "l_warn"], "warned");
    assert_eq!(errors, ["m_deny"], "denied");
}

/// The crate root's `#![allow]`, a `deny` inside it, and `--cap-lints warn` over all of it.
#[test]
fn unused_mut_follows_the_crate_root_and_cap_lints() {
    const CHILD: &str = "RELAX_LINT_CAP_TEST";
    if std::env::var(CHILD).is_err() {
        return run_alone("borrowck::lint_scope_tests::unused_mut_follows_the_crate_root_and_cap_lints", CHILD, "--cap-lints warn");
    }
    let (errors, warnings) = unused_mut_in(r#"
        #![allow(unused_mut)]
        pub fn crate_level() { let mut a_crate = 1; let _ = a_crate; }
        pub fn deny_inside() { #[deny(unused_mut)] let mut b_deny_capped = 1; let _ = b_deny_capped; }
    "#);
    assert_eq!(warnings, ["b_deny_capped"], "warned");
    assert!(errors.is_empty(), "--cap-lints warn left errors: {errors:?}");
}

/// WARNZERO class 7: a lint group on a let (cargo_makepad android/mod.rs:600,
/// `#[allow(unused)] let mut host_os = ..` shadowed by a cfg'd one) and an attribute on a let a
/// `macro_rules!` expansion writes (hal cmds.rs:98 and events.rs:91: `let mut n` that only a
/// repetition mutates).
#[test]
fn unused_mut_follows_groups_and_macro_written_lets() {
    const CHILD: &str = "RELAX_LINT_GROUP_TEST";
    if std::env::var(CHILD).is_err() {
        return run_alone("borrowck::lint_scope_tests::unused_mut_follows_groups_and_macro_written_lets", CHILD, "");
    }
    let (errors, warnings) = unused_mut_in(r#"
        pub fn group() -> u32 { #[allow(unused)] let mut a_group = 1; let a_group = 2; a_group }
        macro_rules! make {
            ($name:ident $(, $f:ident)*) => {
                pub fn $name() -> u32 {
                    let mut total = 0;
                    $( total += 1; let $f = 0; let _ = $f; )*
                    { #[allow(unused_mut, unused_assignments)] let mut b_macro = 1u32; $( b_macro += 1; let $f = 0; let _ = $f; )* total += b_macro; }
                    { let mut c_macro_control = 1u32; total += c_macro_control; }
                    total
                }
            };
        }
        make!(none);
        pub fn d_control() { let mut d_control = 1; let _ = d_control; }
    "#);
    assert_eq!(warnings, ["c_macro_control", "d_control"], "warned");
    assert!(errors.is_empty(), "{errors:?}");
}
