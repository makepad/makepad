//! Item checks of a crate (item_checks.rs). rustc_metadata creader.rs inject_allocator_crate: one
//! `#[global_allocator]` per crate and one per compiled binary (the binary and the crates it links).
//! Each test target is its own binary, so two targets of a package that each define one do not
//! conflict. Exported symbols clash the same way: in one crate or one binary. And E0428, two items
//! of one name in one module or block.
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

/// rustc_codegen_ssa: "symbol `x` is already defined" where two exports of one name meet, in one
/// crate or in one binary. A crate and its own test harness (`check --tests`: its files built
/// twice, into binaries cargo links apart) export the same symbols and do not clash: every
/// `#[no_mangle]` static of relax-compiler was refused in its own `--tests` check (11-27).
#[test]
fn one_exported_symbol_per_crate_and_per_binary() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    let export = "#[no_mangle]\npub static S: u8 = 0;\n";
    let lib = load(&mut prog, "lib", export, &[]);
    // the lib's own test harness, and an integration test that links the lib
    let harness = load(&mut prog, "lib_harness", export, &[]);
    let integration = load(&mut prog, "integration", "pub fn f() {}\n", &[lib]);
    // a binary that links the lib and exports the name too
    let bin = load(&mut prog, "bin", export, &[lib]);
    // a crate that exports it twice itself, the second under `#[export_name]`
    let twice = load(&mut prog, "twice", "#[no_mangle]\npub static S: u8 = 0;\nmod n {\n    #[export_name = \"S\"]\n    pub static T: u8 = 0;\n}\n", &[]);
    prog.resolve_imports();
    let own_of = |ks: &[u32]| {
        let mut own = vec![false; prog.crates.len()];
        for &k in ks { own[k as usize] = true; }
        own
    };
    let check = |ks: &[u32], binaries: &[u32]| crate::item_checks::duplicate_symbols(&prog, &own_of(ks), binaries);
    assert_eq!(check(&[lib, harness, integration], &[harness, integration]), Vec::<String>::new(), "a crate and its own test harness");
    assert_eq!(check(&[lib, harness], &[]), Vec::<String>::new(), "two libs checked with no binary");
    let e = check(&[lib, bin], &[bin]);
    assert_eq!(e.len(), 1, "a binary and a crate it links: {e:?}");
    assert!(e[0].ends_with("symbol `S` is already defined"), "{e:?}");
    let e = check(&[twice], &[]);
    assert_eq!(e.len(), 1, "two exports in one crate: {e:?}");
    assert!(e[0].ends_with("symbol `S` is already defined"), "{e:?}");
}

/// A test target may be named `alloc` (tests/alloc.rs): that is not the sysroot crate of the name
/// (front crates.rs own_crate_name), so its siblings do not link it, and their allocators and its
/// own do not conflict.
#[test]
fn a_test_target_named_alloc_is_not_linked_by_its_siblings() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    let alloc = "#[global_allocator]\nstatic A: u8 = 0;\n";
    let named_alloc = load(&mut prog, &relax_front::crates::own_crate_name("alloc".to_string()), alloc, &[]);
    let sibling = load(&mut prog, "sibling", alloc, &[]);
    for k in [named_alloc, sibling] { prog.crates[k as usize].harness = true; }
    prog.resolve_imports();
    let targets = [named_alloc, sibling];
    let mut own = vec![false; prog.crates.len()];
    for k in targets { own[k as usize] = true; }
    assert_eq!(crate::item_checks::global_allocators_and_lang_items(&prog, &own, &targets), Vec::<String>::new());
}

/// E0428 (rustc_resolve build_reduced_graph, report_conflict): a body's items are their block's,
/// each block a scope of its own. Two values or two types of one module or block collide (a fn
/// body that held two `static MET`s used to run, a lookup taking one for both, 11-26); a type and a
/// value of one name do not, nor sibling blocks, a block and the module around it, an inner block
/// and the outer one, or an impl's or a trait's item and a block's fn. Each error is at the second
/// item's start, its visibility and not its attributes, where rustc puts it.
#[test]
fn two_items_of_one_name_in_one_module_or_block_are_refused_and_no_others() {
    let sess = &std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(sess);
    let src = [
        "pub static MET: u32 = 9;",
        "#[allow(dead_code)]",
        "pub(crate) static MET: u32 = 1;",
        "pub fn one() -> u32 {",
        "    static MET: u32 = 0;",
        "    let f = || MET;",
        "    static MET: u32 = 1;",
        "    f() + MET",
        "}",
        "pub fn ns() -> u32 {",
        "    struct T { x: u32 }",
        "    static T: u32 = 0;",
        "    struct U;",
        "    fn U() {}",
        "    struct V;",
        "    enum V { A }",
        "    let c = || { static D: u32 = 0; static D: u32 = 1; D };",
        "    let g = { static G: u32 = 0; G } + { static G: u32 = 1; { static G: u32 = 2; G } };",
        "    struct W;",
        "    impl W { fn m(&self) {} }",
        "    fn m() {}",
        "    trait Tr { fn t(&self) {} }",
        "    fn t() {}",
        "    const K: u32 = { static Z: u32 = 1; static Z: u32 = 2; Z };",
        "    c() + g + K",
        "}",
    ].join("\n");
    let k = load(&mut prog, "dup", &src, &[]);
    prog.resolve_imports();
    let mut own = vec![false; prog.crates.len()];
    own[k as usize] = true;
    // (each as "line:col: message", the file's path cut off; by position, the order check_order
    // gives them)
    let judged = |files: Option<&[u32]>| {
        let j = files.map_or(crate::program::Judged::crates(&own), |fs| crate::program::Judged::files(&own, fs));
        let mut errors: Vec<String> = crate::item_checks::duplicate_items(&prog, j).iter().map(|e| e.split_once(".rs:").map_or(e.clone(), |(_, at)| at.to_string())).collect();
        errors.sort();
        errors
    };
    let errors = judged(None);
    let e0428 = |at: &str, name: &str, ns: &str, scope: &str| format!("{at}: E0428: the name `{name}` is defined multiple times -- note: `{name}` must be defined only once in the {ns} namespace of {scope}");
    let mut want = [
        e0428("3:1", "MET", "value", "this module"),
        e0428("7:5", "MET", "value", "this block"),
        e0428("14:5", "U", "value", "this block"),
        e0428("16:5", "V", "type", "this block"),
        e0428("17:37", "D", "value", "this block"),
        e0428("24:41", "Z", "value", "this block"),
    ];
    want.sort();
    assert_eq!(errors, want);
    // a live edit's check judges its files alone (reload.rs): the crate's one file finds them all,
    // no file none
    let root = prog.mods[prog.crates[k as usize].root_mod as usize].file;
    assert_eq!(judged(Some(&[root])), want, "the edited file's items");
    assert_eq!(judged(Some(&[])), Vec::<String>::new(), "no file judged");
}
