//! `makepad check`'s warnings are rustc's, no more and no less: the gates fail on any warning, so
//! a lint the JIT's check reports where rustc's does not turns them red. Each case checks a small
//! package (its library is the check's root; each binary target is a crate of its own that uses
//! it, as cargo builds it) in a child process (the lint sink is the process's) and compares the
//! warnings with rustc's for the same package.

const CHILD: &str = "RELAX_CHECK_LINT_CHILD";

/// In the parent: runs the test `name` alone in a child and returns the warnings it printed, as
/// "message [lint]" (no position), and its errors, as "error: message [lint]". In the child: None
/// (the caller checks the package).
fn warnings_in_child(name: &str) -> Option<Vec<String>> {
    warnings_in_child_with(name, &[])
}

/// `warnings_in_child`, the child's variables `env` set.
fn warnings_in_child_with(name: &str, env: &[(&str, &str)]) -> Option<Vec<String>> {
    if std::env::var_os(CHILD).is_some() {
        return None;
    }
    let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1", "--format=terse"])
        .env(CHILD, "1")
        .env_remove("MAKEPAD_RUN_LINT_FLAGS")
        .envs(env.iter().copied()));
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
    // (the check prints its diagnostics on stderr; stdout has the test harness's lines)
    let mut v: Vec<String> = stderr.lines().chain(stdout.lines()).filter_map(|l| match l.split_once(": warning: ") {
        Some((_, w)) => Some(w.to_string()),
        None => l.split_once(": error: ").map(|(_, e)| format!("error: {e}")),
    }).collect();
    v.sort();
    Some(v)
}

/// Checks the package of `lib` and the binary targets `bins` (crate `pkg` and `bin0`, `bin1`, ..),
/// the library at the root, as `makepad check <package>` does; the check must pass.
fn check_package(lib: &str, bins: &[&str]) {
    assert_eq!(check_package_code(lib, bins), 0, "the check failed");
}

/// `check_package`'s check, returning its exit code.
fn check_package_code(lib: &str, bins: &[&str]) -> i32 {
    check_code(lib, bins, false)
}

/// The check of `lib` and `bins`; `harness`: every target is built with `--test` (cfg(test),
/// libtest's generated main), as `makepad check --tests` builds them.
fn check_code(lib: &str, bins: &[&str], harness: bool) -> i32 {
    check_code_uses(lib, bins, harness, Uses::Recorded)
}

/// How a check's load records its import uses (`check_code_uses`).
#[derive(PartialEq)]
enum Uses {
    /// on from before the load to the end of the check, as `makepad check` records them
    Recorded,
    /// never on (a test's run of its own binary, a run)
    Off,
    /// on at the load, then off and on again before the check: a new stretch, without the uses
    /// of the load
    Restarted,
}

/// `check_code`, the import uses recorded as `uses` says.
fn check_code_uses(lib: &str, bins: &[&str], harness: bool, uses: Uses) -> i32 {
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    // `makepad check`'s session (run.rs): constants go through the check-mode interpreter first
    let mut sess = crate::Session::default();
    sess.options.const_fuel = true;
    let sess = std::sync::Arc::new(sess);
    // as run.rs sets up a check: import and macro uses are recorded, every crate keeps the
    // user types its borrow check reads (the mutable-place fixups included)
    relax_types::macros::uses::record(true);
    relax_front::program::set_record_import_uses(uses != Uses::Off);
    crate::borrowck::event::set_check_scope();
    let dir = std::env::temp_dir().join(format!("relax-check-lints-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut prog = crate::program::Program::new(&sess);
    let (core, std_deps) = crate::load_std_crates(&sess, &mut prog).unwrap();
    let mut files = vec![dir.join("lib.rs")];
    std::fs::write(&files[0], lib).unwrap();
    let mut cfg = crate::program_cfg(&sess, &[]);
    if harness { cfg.set.push("test".to_string()); }
    let root = prog.load_crate_deferred("pkg", files[0].to_str().unwrap(), 2021, cfg, std_deps.clone()).unwrap();
    prog.crates[root as usize].harness = harness;
    let mut crates = vec![root];
    for (i, src) in bins.iter().enumerate() {
        let path = dir.join(format!("bin{i}.rs"));
        std::fs::write(&path, src).unwrap();
        let mut deps = std_deps.clone();
        deps.push((prog.syms.intern("pkg"), root));
        let mut bin_cfg = crate::program_cfg(&sess, &[]);
        if harness { bin_cfg.set.push("test".to_string()); }
        let k = prog.load_crate_deferred(&format!("bin{i}"), path.to_str().unwrap(), 2021, bin_cfg, deps).unwrap();
        prog.crates[k as usize].is_bin = true;
        prog.crates[k as usize].harness = harness;
        crates.push(k);
        files.push(path);
    }
    prog.prelude_crate = Some(core);
    prog.resolve_imports();
    // the front's errors (an import that does not resolve) go to the check, as run.rs passes them
    let pre = std::mem::take(&mut prog.errors);
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    if uses == Uses::Restarted {
        relax_front::program::set_record_import_uses(false);
        relax_front::program::set_record_import_uses(true);
    }
    let code = crate::check::check_crate(&mut u, root, &crates, pre, false);
    for f in &files {
        std::fs::remove_file(f).unwrap();
    }
    std::fs::remove_dir(&dir).unwrap();
    code
}

/// A check whose load did not record its import uses reports no import unused: it cannot tell
/// one (`Program::import_uses_recorded`). A test's run of its own binary, the variable
/// `MAKEPAD_RUN_LINT_FLAGS=-D warnings` set by the test, had checked its crate again with no
/// uses recorded: every import of relax-compiler read unused, each an error. The used and the
/// unused import alike are left out here; the other lints stay.
#[test]
fn a_check_whose_load_recorded_no_import_uses_reports_none_unused() {
    let lib = "use std::io::Write;\nuse std::collections::HashMap;\npub fn f(m: &HashMap<u8, u8>) -> usize { m.len() }\nfn unused() {}\n";
    const USES: &str = "RELAX_CHECK_LINT_USES";
    for (case, uses) in [("off", Uses::Off), ("restarted", Uses::Restarted), ("recorded", Uses::Recorded)] {
        if std::env::var(USES).as_deref() == Ok(case) {
            return assert_eq!(check_code_uses(lib, &[], false, uses), 0);
        }
    }
    for (case, expected) in [
        ("off", vec!["function `unused` is never used [dead_code]"]),
        ("restarted", vec!["function `unused` is never used [dead_code]"]),
        ("recorded", vec!["function `unused` is never used [dead_code]", "unused import: `std::io::Write` [unused_imports]"]),
    ] {
        let Some(warnings) = warnings_in_child_with("check_lint_tests::a_check_whose_load_recorded_no_import_uses_reports_none_unused", &[(USES, case)]) else { return };
        assert_eq!(warnings, expected, "{case}");
    }
}

/// `makepad check --test <name>` had reported every test of a test target and every helper they
/// call as never used: libtest's generated main names each `#[test]` fn (rustc_builtin_macros
/// test_harness.rs), so they are live; a helper no test reaches still is dead.
#[test]
fn a_test_targets_test_fns_are_used() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_test_targets_test_fns_are_used") else {
        return assert_eq!(check_code(r#"
            fn helper() -> u32 { 2 }
            fn unused() {}
            #[test]
            fn adds() { assert_eq!(helper(), 2); }
            mod nested {
                #[test]
                fn runs() {}
            }
        "#, &[], true), 0);
    };
    assert_eq!(warnings, ["function `unused` is never used [dead_code]"]);
}

/// `makepad check --tests` had reported a binary's `main` and all it calls as never used when it
/// builds the binary with `--test`: libtest's harness marks that `main` #[allow(dead_code)]
/// (rustc_builtin_macros test_harness.rs EntryPointCleaner), so it stays live; an item nothing
/// reaches still is dead.
#[test]
fn a_binary_built_as_a_test_keeps_its_main() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_binary_built_as_a_test_keeps_its_main") else {
        return assert_eq!(check_code("pub fn shared() -> u32 { 1 }", &["fn helper() -> u32 { 2 }\nfn unused_in_bin() {}\nfn main() { let _ = pkg::shared() + helper(); }\n"], true), 0);
    };
    assert_eq!(warnings, ["function `unused_in_bin` is never used [dead_code]"]);
}

/// A fn only a `global_asm!`'s `sym` operand names is used, and so is a constant of its `const`
/// operand (rustc_passes dead.rs: the global_asm item is live, its operands' paths with it); a fn
/// nothing names still is dead. The check had reported the unwinder's native raise (jit.rs
/// native_raise, which only its stub calls) as never used.
#[test]
fn what_a_global_asm_names_is_used() {
    let Some(warnings) = warnings_in_child("check_lint_tests::what_a_global_asm_names_is_used") else {
        return check_package(r#"
            extern "C" fn named_by_asm() {}
            const ALIGN: usize = 3;
            fn unused() {}
            std::arch::global_asm!(".data", ".p2align {a}", ".quad {f}", ".text", f = sym named_by_asm, a = const ALIGN);
        "#, &[]);
    };
    assert_eq!(warnings, ["function `unused` is never used [dead_code]"]);
}

/// WARNZERO class 6 (994 warnings in 24 packages: apps/flow's makepad-flow-server.rs:96, ai-llm's
/// 18 bins, ..): every binary target's `main` is live (rustc_passes dead.rs
/// create_and_seed_worklist, per crate), not only the one the check is rooted at, so neither it
/// nor what it uses is dead; an item no `main` reaches still is.
#[test]
fn a_second_binary_targets_main_is_its_entry() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_second_binary_targets_main_is_its_entry") else {
        return check_package("pub fn shared() -> u32 { 1 }", &[
            "const K: u32 = 2;\nfn helper() -> u32 { K }\nfn unused_in_bin() {}\nfn main() { let _ = pkg::shared() + helper(); }\n",
            "fn main() { let _ = pkg::shared(); }\n",
        ]);
    };
    assert_eq!(warnings, ["function `unused_in_bin` is never used [dead_code]"]);
}

/// rustc lints each crate it compiles on its own: a binary's call of `Write::write_all` is no use
/// of the library's `use std::io::Write;` (the check had pooled the traits every checked body calls
/// methods of, so one crate's call hid another crate's unused trait import).
#[test]
fn another_crates_method_call_leaves_a_trait_import_unused() {
    let Some(warnings) = warnings_in_child("check_lint_tests::another_crates_method_call_leaves_a_trait_import_unused") else {
        return check_package("use std::io::Write;\npub fn f() -> u32 { 1 }\n", &[
            "use std::io::Write;\nfn main() { let mut v: Vec<u8> = Vec::new(); v.write_all(b\"x\").unwrap(); let _ = pkg::f(); }\n",
        ]);
    };
    assert_eq!(warnings, ["unused import: `std::io::Write` [unused_imports]"]);
}

/// The same for macro imports: a binary that glob-imports the library and invokes a macro of the same
/// name (its own) does not use the library's `use m::mm;` (the names written in a module that
/// glob-imports another had counted for the imports of a module of another crate).
#[test]
fn another_crates_macro_call_leaves_a_macro_import_unused() {
    let Some(warnings) = warnings_in_child("check_lint_tests::another_crates_macro_call_leaves_a_macro_import_unused") else {
        return check_package("mod m {\n    macro_rules! mm { () => {} }\n    pub(crate) use mm;\n    pub(crate) fn g() { mm!(); }\n}\nuse m::mm;\npub fn f() { m::g(); }\n", &[
            "use pkg::*;\nmacro_rules! mm { () => {} }\nfn main() { mm!(); f(); }\n",
        ]);
    };
    assert_eq!(warnings, ["unused import: `m::mm` [unused_imports]"]);
}

/// rustc's analysis stops before misc_checking_3 once a required analysis failed: a check with an
/// error reports no late lint (the private fn no one calls is no dead_code warning), while the
/// lints of the passes before it still come with the error -- the unused import, and the liveness
/// lints of a body that type-checked (the failed one's are not reported).
#[test]
fn a_failed_check_reports_no_late_lints() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_failed_check_reports_no_late_lints") else {
        return assert_ne!(check_package_code("use std::fmt::Debug;\nuse std::collections::HashMap;\nfn never_called() {}\npub fn f() { let x = 1; }\npub fn g() -> u32 { let y = 2; \"no\" }\n", &[]), 0);
    };
    assert_eq!(warnings, ["unused import: `std::collections::HashMap` [unused_imports]", "unused import: `std::fmt::Debug` [unused_imports]", "unused variable: `x` [unused_variables]"]);
}

/// Beside errors the unused imports are still rustc's: an import that does not resolve is its
/// E0432 error, never an unused import (rustc_resolve counts an erroneous import as used); a trait
/// whose method a body that failed its check called before the error is used; an import nothing
/// names is unused.
#[test]
fn a_failed_import_is_no_unused_import() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_failed_import_is_no_unused_import") else {
        return assert_ne!(check_package_code("use nothing_here::Foo;\nuse std::collections::HashMap;\nuse std::fmt::Write;\npub fn f(s: &mut String) -> u32 { s.write_str(\"x\").unwrap(); let n: u32 = \"no\"; n }\n", &[]), 0);
    };
    assert_eq!(warnings, ["unused import: `std::collections::HashMap` [unused_imports]"]);
}

/// A name that does not resolve (E0425) is reported with a typo suggestion; the names that
/// suggestion looks at are no uses: the imports in scope that nothing names stay unused, as rustc
/// reports them beside the error.
#[test]
fn a_typo_suggestion_uses_no_import() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_typo_suggestion_uses_no_import") else {
        return assert_ne!(check_package_code("use std::collections::HashMap;\nuse std::collections::BTreeMap;\npub fn f() -> u32 { let m: BTreeMap<u32, u32> = BTreeMap::new(); m.len() as u32 + HashMapp }\n", &[]), 0);
    };
    assert_eq!(warnings, ["unused import: `std::collections::HashMap` [unused_imports]"]);
}

/// An import of something private never binds in rustc (E0603; finalize_imports plants a dummy
/// binding and counts it used): it is no unused import, while the others beside it still are.
#[test]
fn a_private_import_is_no_unused_import() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_private_import_is_no_unused_import") else {
        return assert_ne!(check_package_code("mod a { fn f() {} pub mod b { pub fn h() {} } }\nuse a::f;\nuse std::collections::HashMap;\nuse a::b::h;\npub fn g() {}\n", &[]), 0);
    };
    assert_eq!(warnings, ["unused import: `a::b::h` [unused_imports]", "unused import: `std::collections::HashMap` [unused_imports]"]);
}

/// A method error makes the traits in scope that have an item of its name used (rustc
/// report_method_error: the error is very possibly why the trait was not used); a trait without
/// one and an import of something else stay unused.
#[test]
fn a_method_error_uses_the_traits_in_scope() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_method_error_uses_the_traits_in_scope") else {
        return assert_ne!(check_package_code("use std::fmt::Write;\nuse std::ops::Add;\nuse std::collections::HashMap;\npub struct S;\npub fn g() { S.write_str(\"x\"); }\n", &[]), 0);
    };
    assert_eq!(warnings, ["unused import: `std::collections::HashMap` [unused_imports]", "unused import: `std::ops::Add` [unused_imports]"]);
}

/// A crate's `const _: () = assert!(cfg!(..))` (E0080 under the JIT's check only): the check evaluates every const
/// item (rustc_hir_analysis check_crate), and `cfg!` in one is the literal the crate's cfg gives
/// (rustc_builtin_macros cfg.rs expand_cfg); MIR building had made every `cfg!` false.
#[test]
fn a_const_reads_cfg_as_the_crate_sets_it() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_const_reads_cfg_as_the_crate_sets_it") else {
        return check_package(r#"
            const _: () = assert!(cfg!(target_endian = "little"));
            pub const WIDTH: usize = if cfg!(target_pointer_width = "64") { 64 } else { 32 };
            const _: () = assert!(WIDTH == core::mem::size_of::<usize>() * 8);
            const _: () = assert!(!cfg!(target_endian = "big") && cfg!(any(target_arch = "x86_64", target_arch = "aarch64")));
            pub fn bytes() -> [u8; WIDTH] { [0; WIDTH] }
        "#, &[]);
    };
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// WARNZERO class 3: `std::env::set_var` and `remove_var` are unsafe fns marked
/// `#[rustc_deprecated_safe_2024]` (real std's env.rs): before edition 2024 a call needs no
/// unsafe block, and the block around one is used, not `unused_unsafe` (Relax std had declared
/// them safe). A block with no unsafe operation still is.
#[test]
fn an_unsafe_block_around_set_var_is_used() {
    let Some(warnings) = warnings_in_child("check_lint_tests::an_unsafe_block_around_set_var_is_used") else {
        return check_package(r#"
            pub fn wrapped() { unsafe { std::env::set_var("RELAX_CHECK_LINT_A", "1"); std::env::remove_var("RELAX_CHECK_LINT_A"); } }
            pub fn bare() { std::env::set_var("RELAX_CHECK_LINT_B", "1"); std::env::remove_var("RELAX_CHECK_LINT_B"); }
            pub fn needless() -> u32 { unsafe { 1 } }
        "#, &[]);
    };
    assert_eq!(warnings, ["unnecessary `unsafe` block [unused_unsafe]"]);
}

/// WARNZERO class 5 (relax-compiler lib.rs:153): a `#[global_allocator]` static is used, by the
/// allocator shims its expansion adds (rustc_builtin_macros global_allocator.rs).
#[test]
fn the_global_allocator_static_is_used() {
    let Some(warnings) = warnings_in_child("check_lint_tests::the_global_allocator_static_is_used") else {
        return check_package(r#"
            #[global_allocator]
            static GLOBAL: std::alloc::System = std::alloc::System;
            static UNUSED: std::alloc::System = std::alloc::System;
        "#, &[]);
    };
    assert_eq!(warnings, ["static `UNUSED` is never used [dead_code]"]);
}

/// A fn named only in a `#[rustc_legacy_const_generics]` argument is used, written at the call or in
/// a macro's expansion (zune-jpeg's `shuffle`): rustc gives no warning for either, and still warns
/// about the fn nothing names or only a dead fn names. x86_64: the callee is the x86 intrinsic.
#[cfg(target_arch = "x86_64")]
#[test]
fn a_fn_in_a_legacy_const_generic_argument_is_used() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_fn_in_a_legacy_const_generic_argument_is_used") else {
        return check_package(r#"
            use core::arch::x86_64::*;
            const fn at_the_call(x: i32) -> i32 { x }
            const fn in_a_macro(x: i32) -> i32 { x }
            const fn unused(x: i32) -> i32 { x }
            const NAMED: i32 = 216;
            const fn only_a_dead_fn_uses(x: i32) -> i32 { x }
            #[target_feature(enable = "avx2")]
            unsafe fn dead_user(v: __m256i) -> __m256i {
                _mm256_permute4x64_epi64(v, only_a_dead_fn_uses(216))
            }
            #[target_feature(enable = "avx2")]
            pub unsafe fn direct(v: __m256i) -> __m256i {
                _mm256_permute4x64_epi64(v, at_the_call(216))
            }
            #[target_feature(enable = "avx2")]
            pub unsafe fn named(v: __m256i) -> __m256i {
                _mm256_permute4x64_epi64(v, NAMED)
            }
            #[target_feature(enable = "avx2")]
            pub unsafe fn local(v: __m256i) -> __m256i {
                const fn in_the_block(x: i32) -> i32 { x }
                _mm256_permute4x64_epi64(v, in_the_block(216))
            }
            #[target_feature(enable = "avx2")]
            pub unsafe fn expanded(v: __m256i) -> __m256i {
                macro_rules! permute {
                    ($v:tt) => { _mm256_permute4x64_epi64($v, in_a_macro(216)) };
                }
                permute!(v)
            }
        "#, &[]);
    };
    assert_eq!(warnings, [
        "function `dead_user` is never used [dead_code]",
        "function `only_a_dead_fn_uses` is never used [dead_code]",
        "function `unused` is never used [dead_code]",
    ]);
}

/// `Formatter::write_char` is `fmt::Write`'s (real core has no inherent one, zerocopy's error.rs),
/// so the import is used.
#[test]
fn a_formatters_write_char_uses_the_write_import() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_formatters_write_char_uses_the_write_import") else {
        return check_package(r#"
            use core::fmt::{self, Write};
            pub struct S;
            impl fmt::Display for S {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_char('s') }
            }
        "#, &[]);
    };
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// `Formatter::write_str` is inherent: it does not use the import.
#[test]
fn a_formatters_write_str_leaves_the_write_import_unused() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_formatters_write_str_leaves_the_write_import_unused") else {
        return check_package(r#"
            use core::fmt::{self, Write};
            pub struct S;
            impl fmt::Display for S {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("s") }
            }
        "#, &[]);
    };
    assert_eq!(warnings, ["unused import: `Write` [unused_imports]"]);
}

/// WARNZERO class 4 (inspect/heap.rs:537, jit.rs:2554): `asm!` is an unsafe operation (rustc
/// check_unsafety UseOfInlineAssembly), so the block around one is used.
#[test]
fn an_unsafe_block_around_asm_is_used() {
    let Some(warnings) = warnings_in_child("check_lint_tests::an_unsafe_block_around_asm_is_used") else {
        return check_package(r#"
            #[cfg(target_arch = "x86_64")]
            pub fn seven() -> u64 { let v: u64; unsafe { core::arch::asm!("mov {}, 7", out(reg) v, options(nomem, nostack)) }; v }
            #[cfg(target_arch = "aarch64")]
            pub fn seven() -> u64 { let v: u64; unsafe { core::arch::asm!("mov {}, #7", out(reg) v, options(nomem, nostack)) }; v }
            pub fn needless() -> u32 { unsafe { 2 } }
        "#, &[]);
    };
    assert_eq!(warnings, ["unnecessary `unsafe` block [unused_unsafe]"]);
}

/// WARNZERO class 1 (fab slice live_trace.rs:253 and :277, hal vulkan/device.rs:165): a tuple
/// field read through an overloaded deref (`self: Pin<&mut Self>`, `self: Arc<Self>`, `&Arc<S>`)
/// is a read of that struct's field; a tuple field nothing reads still is dead.
#[test]
fn a_tuple_field_read_through_pin_or_arc_is_read() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_tuple_field_read_through_pin_or_arc_is_read") else {
        return check_package(r#"
            use std::pin::Pin;
            use std::sync::Arc;
            struct Yield(bool);
            impl Yield { fn poll(mut self: Pin<&mut Self>) -> bool { if self.0 { return true } self.0 = true; false } }
            struct Wake(u32);
            impl Wake { fn wake(self: Arc<Self>) -> u32 { self.0 } }
            struct Life(u8, Option<u16>);
            fn life(l: &Arc<Life>) -> bool { l.1.is_some() && l.0 > 0 }
            struct Unread(u32);
            pub fn run() -> u32 {
                let mut y = Yield(false);
                let _ = Pin::new(&mut y).poll();
                let _ = Unread(3);
                Arc::new(Wake(1)).wake() + life(&Arc::new(Life(1, None))) as u32
            }
        "#, &[]);
    };
    assert_eq!(warnings, ["field `0` is never read [dead_code]"]);
}

/// WARNZERO class 8 (hal linux_ipc.rs:69 and :74): `IoSlice` and `IoSliceMut` are FFI-safe, as
/// real std's are (repr(transparent) over an `iovec` / `WSABUF`; Relax std had plain slices),
/// so a `msghdr` of them in an `extern` block is no `improper_ctypes`; a slice still is.
#[test]
fn io_slices_are_ffi_safe() {
    let Some(warnings) = warnings_in_child("check_lint_tests::io_slices_are_ffi_safe") else {
        return check_package(r#"
            use std::io::{IoSlice, IoSliceMut};
            #[repr(C)]
            pub struct Msg<T> { iov: *mut T, len: usize }
            extern "C" {
                fn relax_lint_sendmsg(msg: *const Msg<IoSlice<'_>>) -> isize;
                fn relax_lint_recvmsg(msg: *mut Msg<IoSliceMut<'_>>) -> isize;
                fn relax_lint_slice(bytes: &[u8]);
            }
            pub unsafe fn send(m: *const Msg<IoSlice<'_>>) -> isize { relax_lint_sendmsg(m) }
            pub unsafe fn recv(m: *mut Msg<IoSliceMut<'_>>) -> isize { relax_lint_recvmsg(m) }
            pub unsafe fn slice(b: &[u8]) { relax_lint_slice(b) }
            pub fn parts(m: &Msg<IoSlice<'_>>) -> (*mut IoSlice<'static>, usize) { (m.iov as *mut IoSlice<'static>, m.len) }
        "#, &[]);
    };
    assert!(warnings.len() == 1 && warnings[0].contains("`[u8]`") && warnings[0].ends_with("[improper_ctypes]"), "{warnings:?}");
}

/// WARNZERO class 2: an import of a macro is used by a module that glob-imports its module
/// (sandbox blocks' `live_id` through `use super::*`), by a macro call by path
/// (`crate::kit::global_instance!()`), and by a `use` that goes through it (asset data's
/// `use crate::codec::{canon_enum, ..}` of `pub(crate) use canon_enum;`); one nothing reaches is
/// still unused.
#[test]
fn a_macro_import_is_used_through_globs_paths_and_uses() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_macro_import_is_used_through_globs_paths_and_uses") else {
        return check_package(r#"
            macro_rules! twice { ($e:expr) => { $e * 2 } }
            pub(crate) use twice;
            pub mod kit {
                macro_rules! thrice { ($e:expr) => { $e * 3 } }
                pub(crate) use thrice;
                macro_rules! never { () => { 0 } }
                pub(crate) use never;
            }
            pub mod parent {
                use crate::kit::thrice;
                pub mod child { use super::*; pub fn c() -> u32 { thrice!(1) } }
            }
            pub mod user {
                use crate::twice;
                pub fn u() -> u32 { twice!(2) + crate::kit::thrice!(3) }
            }
        "#, &[]);
    };
    let imports: Vec<&String> = warnings.iter().filter(|w| w.ends_with("[unused_imports]")).collect();
    assert_eq!(imports, ["unused import: `never` [unused_imports]"], "{warnings:?}");
}

/// WARNZERO (relax-compiler lower.rs, relax-types structural_traits.rs): a by-reference binding
/// of a guarded arm (`ref x`, or `x` under a reference default binding mode) is bound before the
/// guard, whose own local is a reference to it (rustc_mir_build bind_matched_candidate_for_guard),
/// so it is read even when nothing names it; a by-value one, or one of an unguarded arm, is not.
#[test]
fn a_by_ref_binding_of_a_guarded_arm_is_read() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_by_ref_binding_of_a_guarded_arm_is_read") else {
        return check_package(r#"
            pub enum Inst { Mov(u32, u32), Add(u32, u32, u32) }
            pub struct S { pub a: u32, pub b: u32 }
            pub fn by_ref(v: &[Inst], s: &S, t: &(String, u32), c: u32) -> u32 {
                let mut n = 0;
                for i in v {
                    match i {
                        Inst::Mov(d, x) if *x == c => {}
                        Inst::Add(d, x, y) if *x == c => n += *y,
                        _ => n += 1,
                    }
                }
                n += match s { S { a: owner, b } if *b > 1 => 1, _ => 0 };
                n + matches!(t, (name, x) if *x > 0) as u32
            }
            pub fn explicit_ref(t: (String, u32)) -> u32 { match t { (ref s, x) if x > 0 => 1, _ => 0 } }
            pub fn by_value(t: (u32, u32), o: Option<String>) -> u32 {
                let a = match t { (d, x) if x > 0 => 0, _ => 1 };
                let b = match o { Some(s) if a > 0 => 1, _ => 0 };
                a + b + matches!(t, (e, x) if x > 1) as u32
            }
            pub fn unguarded(v: &[Inst]) -> u32 { let mut n = 0; for i in v { if let Inst::Add(a, b, _) = i { n += *a; } } n }
        "#, &[]);
    };
    assert_eq!(warnings, [
        "unused variable: `b` [unused_variables]",
        "unused variable: `d` [unused_variables]",
        "unused variable: `e` [unused_variables]",
        "unused variable: `s` [unused_variables]",
    ]);
}

/// WARNZERO (csg_brep local_solve_nd.rs): a plain `let x = init;` is written where `init` makes
/// its value, at each tail that does not diverge (rustc_mir_build expr_into_dest), so a dead
/// initial value made inside an external macro (`vec![..]`) is no `unused_assignments`; one made
/// by a call, or in a local macro, is.
#[test]
fn a_dead_initial_value_from_an_external_macro_is_not_reported() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_dead_initial_value_from_an_external_macro_is_not_reported") else {
        return check_package(r#"
            macro_rules! fresh { () => { Vec::new() } }
            pub fn external(n: usize, r: &[Result<Vec<f64>, u32>]) -> f64 {
                let mut step = vec![0.0; n];
                let mut acc = 0.0;
                for e in r {
                    match e { Ok(s) => step = s.clone(), Err(_) => break }
                    acc += step[0];
                }
                acc
            }
            pub fn external_tails(c: bool, f: fn() -> Vec<u32>) -> usize { let mut v = if c { vec![1] } else { vec![] }; v = f(); v.len() }
            pub fn mixed_tails(c: bool, f: fn() -> Vec<u32>) -> usize { let mut w = if c { vec![1] } else { Vec::new() }; w = f(); w.len() }
            pub fn local_macro(f: fn() -> Vec<u32>) -> usize { let mut l: Vec<u32> = fresh!(); l = f(); l.len() }
            pub fn plain(f: fn() -> String) -> usize { let mut s = String::new(); s = f(); s.len() }
        "#, &[]);
    };
    assert_eq!(warnings, [
        "value assigned to `l` is never read [unused_assignments]",
        "value assigned to `s` is never read [unused_assignments]",
        "value assigned to `w` is never read [unused_assignments]",
    ]);
}

/// WARNZERO (midi_library vgm.rs): the bounds check of an array index reads the array
/// (rustc_mir_build len_of_slice_or_array: FakeRead ForIndex), so a written array element makes
/// the array used, and the write is dead only when nothing reads the array after it; a field
/// write reads nothing.
#[test]
fn writing_an_array_element_reads_the_array() {
    let Some(warnings) = warnings_in_child("check_lint_tests::writing_an_array_element_reads_the_array") else {
        return check_package(r#"
            pub fn in_loop(w: &[(u8, u8, u8)]) -> u32 {
                let mut regs = [[0u8; 256]; 2];
                let mut n = 0;
                for &(p, r, v) in w { regs[p as usize][r as usize] = v; n += 1; }
                n
            }
            pub fn last_write(i: usize) { let mut a = [0u32; 4]; a[i] = 5; }
            pub fn last_add(i: usize) { let mut b = [0u32; 4]; b[i] += 1; }
            pub fn field_write() { let mut t = (1u32, 2u32); t.0 = 5; }
        "#, &[]);
    };
    assert_eq!(warnings, [
        "value assigned to `a` is never read [unused_assignments]",
        "value assigned to `b` is never read [unused_assignments]",
        "value assigned to `t` is never read [unused_assignments]",
        "variable `t` is assigned to, but never used [unused_variables]",
    ]);
}

/// `let_underscore_lock` is deny-by-default (rustc_lint let_underscore.rs): a `_` anywhere in a
/// `let` pattern, or in a destructuring assignment, that drops a guard (or a `Result` of one) at
/// once fails the check; a named guard does not.
#[test]
fn a_lock_dropped_by_a_wildcard_fails_the_check() {
    let Some(warnings) = warnings_in_child("check_lint_tests::a_lock_dropped_by_a_wildcard_fails_the_check") else {
        return assert_eq!(check_package_code(r#"
            use std::sync::{Mutex, RwLock};
            pub fn locks(m: &Mutex<u32>, r: &RwLock<u32>) -> u32 {
                let _ = m.lock().unwrap();
                let _ = r.write();
                let (_, x) = (m.lock().unwrap(), 7);
                let y;
                (_, y) = (r.read().unwrap(), 8);
                let _held = m.lock().unwrap();
                x + y
            }
        "#, &[]), 1);
    };
    assert_eq!(warnings, ["error: non-binding let on a synchronization lock [let_underscore_lock]"; 4]);
}
