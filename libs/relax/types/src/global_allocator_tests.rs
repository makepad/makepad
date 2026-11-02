//! rustc_metadata creader.rs inject_allocator_crate: one `#[global_allocator]` per crate and one per
//! compiled binary (the binary and the crates it links). Each test target is its own binary, so two
//! targets of a package that each define one do not conflict.
use crate::program::Program;

fn load(prog: &mut Program, name: &str, source: &str, deps: &[u32]) -> u32 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("relax-global-alloc-{}-{n}.rs", std::process::id()));
    std::fs::write(&root, source).unwrap();
    let deps = deps.iter().map(|&d| {
        let dep_name = prog.syms.str(prog.crates[d as usize].name).to_string();
        (prog.syms.intern(&dep_name), d)
    }).collect();
    let sess = prog.session.clone();
    let k = prog.load_crate(name, root.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&sess, &[]), deps).unwrap();
    std::fs::remove_file(root).unwrap();
    k
}

#[test]
fn one_global_allocator_per_crate_and_per_binary() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    let alloc = "#[global_allocator]\nstatic A: u8 = 0;\n";
    let dep = load(&mut prog, "dep", alloc, &[]);
    let plain = load(&mut prog, "plain", "pub fn f() {}\n", &[]);
    // two test targets, each with its own allocator: two binaries, no conflict
    let t1 = load(&mut prog, "t1", alloc, &[plain]);
    let t2 = load(&mut prog, "t2", alloc, &[plain]);
    // a binary that links a dependency's allocator and defines its own
    let t3 = load(&mut prog, "t3", alloc, &[dep]);
    // a crate with two allocators of its own
    let t4 = load(&mut prog, "t4", "#[global_allocator]\nstatic A: u8 = 0;\n#[global_allocator]\nstatic B: u8 = 0;\n", &[]);
    prog.resolve_imports();
    let own_of = |ks: &[u32]| {
        let mut own = vec![false; prog.crates.len()];
        for &k in ks { own[k as usize] = true; }
        own
    };
    let check = |ks: &[u32]| crate::item_checks::global_allocators_and_lang_items(&prog, &own_of(ks), ks);
    assert_eq!(check(&[t1, t2]), Vec::<String>::new());
    // the lib being checked alone (no binary): its allocator is no conflict
    assert_eq!(crate::item_checks::global_allocators_and_lang_items(&prog, &own_of(&[dep]), &[]), Vec::<String>::new());
    let e = check(&[t3]);
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].ends_with("the `#[global_allocator]` in this crate conflicts with global allocator in: dep"), "{e:?}");
    let e = check(&[t4]);
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].ends_with("cannot define multiple global allocators"), "{e:?}");
    // all of them at once (`makepad test` checks every test target of a package together)
    assert_eq!(check(&[t1, t2, t3, t4]).len(), 2);
}
