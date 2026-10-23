//! Source-read bindings across the settle swap: the swap forgets the old compile state
//! (`mem::forget`, never a drop) and destroys its compiler generation. A read on a thread that
//! had a load in that state must not reach into the destroyed generation. (The phone WM died
//! here: SIGSEGV in vfs::read_snapshot on run-owner, reading a forgotten preload's snapshot.)

/// A preload started by a Program that the swap then forgets, and a later read on the same
/// thread (the owner's `crate_meta` read of a Cargo.toml after the swap).
#[test]
fn a_forgotten_preload_leaves_no_read_binding_behind() {
    const CHILD: &str = "MAKEPAD_RUN_VFS_BIND_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // destroying a compiler generation is process-wide: only in an isolated child
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "vfs_bind_tests::a_forgotten_preload_leaves_no_read_binding_behind", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "child: {:?}\n{}{}", out.status, String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        return;
    }
    assert!(crate::zone_alloc::compiler_arena_enabled());
    let dir = std::env::temp_dir().join(format!("relax-vfs-bind-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.join("lib.rs");
    std::fs::write(&root, "pub fn f() -> u32 { 1 }\n").unwrap();
    let manifest = dir.join("Cargo.toml");
    std::fs::write(&manifest, "[package]\nname = \"vfs_bind\"\nversion = \"0.1.0\"\n").unwrap();
    // the loader scans and parses itself: no worker thread holds the generation (domain loads)
    crate::preload::SINGLE.with(|s| s.set(true));
    let old = {
        let _c = crate::zone_alloc::compiler();
        let session = std::sync::Arc::new(crate::Session::default());
        let cfg = crate::cfg::CfgSet::host(&session, &[]);
        let mut prog = crate::program::Program::new(&session);
        prog.preload(vec![crate::preload::PreCrate {
            name: "vfs_bind".into(), root: root.to_string_lossy().into_owned(), edition: 2021, cfg,
            deps: Vec::new(), proc_macro: false, image_deps: Vec::new(), image_dep_names: Vec::new(),
        }]);
        // the swap: the old compile state is forgotten, never dropped
        std::mem::forget(prog);
        crate::zone_alloc::compiler_generation()
    };
    // the swap's drained state: the front's fork/join workers (the demand plan's sections) end
    // before the old generation is destroyed (zone_swap.rs `unexpected_threads`)
    crate::pool::retire_idle();
    assert_eq!(crate::pool::stats().1, 0, "every pool worker ended");
    crate::zone_alloc::compiler_begin_generation();
    let (chunks, _) = crate::zone_alloc::compiler_destroy_generation(old);
    assert!(chunks > 0, "the forgotten state was in the destroyed generation");
    std::fs::write(&manifest, "[package]\nname = \"vfs_bind\"\nversion = \"0.2.0\"\n").unwrap();
    let text = crate::vfs::read_to_string(&manifest).unwrap();
    assert!(text.contains("0.2.0"), "a read after the load sees the file as it is now: {text}");
    std::fs::remove_dir_all(&dir).unwrap();
}
