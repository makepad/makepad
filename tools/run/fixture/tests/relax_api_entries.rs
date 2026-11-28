//! The #[relax_api] table of this crate as the loader reads it (needs the host-shims feature).
#![cfg(feature = "host-shims")]

#[test]
fn entries_are_linked() {
    let _ = relax_provided_fixture::bound();
    let e = relax_api::entries();
    let size = std::mem::size_of::<relax_api::Entry>();
    println!("COUNTS entries().len() {} span bytes {} entry size {} divisible {}", e.len(), e.len() * size, size, (e.len() * size) % size == 0);
    let mut counts = std::collections::BTreeMap::<(String, u32), usize>::new();
    for x in e {
        *counts.entry((x.module.split("::").next().unwrap_or("").to_string(), x.kind)).or_default() += 1;
    }
    for ((c, k), n) in &counts {
        println!("COUNTS crate {c} kind {k} = {n}");
    }
    let mut keys: Vec<String> = e.iter().filter(|e| e.kind != 0).map(|e| format!("{} {} | {} | slots {:?} size {} align {} offs {:?}", e.kind, (e.key)(), (e.sig)(), e.slots, e.size, e.align, e.offsets)).collect();
    keys.sort();
    for k in &keys {
        println!("{k}");
    }
    assert!(keys.iter().any(|k| k.contains("relax_provided_fixture::add |")));
    // a per_side static is keyed by its def path: one inside a fn body by that fn's, so two of one
    // name in two fns keep two keys, and a module-level one keeps its module's
    let per_side: Vec<&str> = e.iter().filter(|e| e.kind == relax_api::PER_SIDE).map(|e| (e.key)()).collect();
    for k in ["relax_provided_fixture::nested_hits_a::HITS", "relax_provided_fixture::nested_hits_b::HITS", "relax_provided_fixture::MEMO"] {
        assert!(per_side.contains(&k), "per_side key `{k}` not in the table: {per_side:?}");
    }
    assert!(!per_side.contains(&"relax_provided_fixture::HITS"), "a nested static keyed by its module alone: {per_side:?}");
    // `#[relax_api(run)]` reaches the loader in the op's entry (a program with a global allocator
    // is probed and routed to only when a crate it binds has such an op), and in no other entry
    let add = e.iter().find(|e| (e.key)() == "relax_provided_fixture::add").unwrap();
    assert!(add.kind == relax_api::OP && add.run, "the run op `add` says run in its entry");
    let said_run: Vec<&str> = e.iter().filter(|e| e.kind != relax_api::OP && e.run).map(|e| (e.key)()).collect();
    assert!(said_run.is_empty(), "entries other than ops say run: {said_run:?}");
    // call one op through its wrapper: add(2, 40)
    let mut a: i32 = 2;
    let mut b: i64 = 40;
    let args = [&mut a as *mut i32 as *mut u8, &mut b as *mut i64 as *mut u8];
    let mut r: i64 = 0;
    let rc = unsafe { (add.shim.unwrap())(args.as_ptr(), &mut r as *mut i64 as *mut u8) };
    assert_eq!((rc, r), (0, 42));
}
