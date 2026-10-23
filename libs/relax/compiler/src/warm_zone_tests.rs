//! The warm-launch accounting (relax_codegen::warm, MAKEPAD_RUN_WARM_STRICT=1) across the settle
//! swap: its state is process-wide, filled from compiler threads, and outlives the compiler
//! generation those threads ran in. (PGO7's cold Sandbox race: SIGSEGV in a memcmp of
//! `warm::instance`'s key probe, "use after compiler-zone destroy (generation 0)", after a swap.)

/// Entries recorded in the compiler phase, the generation destroyed (a swap), then more records
/// (the same keys probed again, enough new ones to grow the tables) and the report.
#[test]
fn warm_accounting_survives_a_compiler_generation_destroy() {
    const CHILD: &str = "MAKEPAD_RUN_WARM_ZONE_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // destroying a compiler generation is process-wide: only in an isolated child
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "warm_zone_tests::warm_accounting_survives_a_compiler_generation_destroy", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .env("MAKEPAD_RUN_WARM_STRICT", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "child: {:?}\n{}{}", out.status, String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        return;
    }
    assert!(crate::zone_alloc::compiler_arena_enabled());
    assert!(relax_codegen::warm::enabled());
    let record = |from: u32, n: u32| {
        for i in from..from + n {
            relax_codegen::warm::name(i, &format!("slot {i} name"));
            relax_codegen::warm::instance(i, 7, &[i, i + 1]);
            relax_codegen::warm::reason(i, &format!("reason {i}"));
            drop(relax_codegen::warm::Scope::body(7, &[i, i + 1], false, || format!("body {i}")));
        }
    };
    let old = {
        let _c = crate::zone_alloc::compiler();
        record(0, 2000);
        crate::zone_alloc::compiler_generation()
    };
    // the swap: a new generation, compiler TLS forgotten, the old generation destroyed
    crate::zone_alloc::compiler_begin_generation();
    relax_session::gen_local::bump();
    let (chunks, _) = crate::zone_alloc::compiler_destroy_generation(old);
    assert!(chunks > 0);
    {
        let _c = crate::zone_alloc::compiler();
        // the old keys probed again, then new ones past the tables' capacity
        record(0, 2000);
        record(2000, 20000);
    }
    relax_codegen::warm::classify(true);
    relax_codegen::warm::finish();
}
