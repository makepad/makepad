// Default for raw pointers is null (real core since 1.88), also through derive(Default).
#[derive(Default, Debug)]
struct Handles {
    a: *const u8,
    b: *mut u64,
    n: u32,
}

#[test]
fn a01_ptr_default() {
    let p: *const i32 = Default::default();
    let q: *mut String = <*mut String>::default();
    println!("| const null {} mut null {}", p.is_null(), q.is_null());
    let h = Handles::default();
    println!("| derive {} {} {}", h.a.is_null(), h.b.is_null(), h.n);
    let v: Vec<*const u16> = vec![Default::default(); 3];
    println!("| vec {}", v.iter().all(|p| p.is_null()));
}
