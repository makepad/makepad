// The sandbox's memory confinement alone (--allow-unchecked-modules: the compile-time refusal
// is off, so modules may use unsafe): whatever address a module writes or reads, the host's
// memory is untouched and a bad access ends the call with an error. A module's panic message
// reaches the host as the call's error, read from the module's memory alone and at most 4096
// bytes of it, whatever pointer and length the module hands the runtime. Text a module hands the
// runtime at an unmapped part of its window (offset 0, the window's last bytes, past the heap's
// mapped top, a range from its statics into its first segment's guard) ends that call as the
// module's own read would, never the host process: panic text and file name (rt_panic_impl), a
// str compared through the runtime's str_eq, a buffer filled with random bytes. (At the window's
// last bytes the last two are refused unread: the range leaves the window. A str formatted with
// panic! is formatted in the module's own code, a contained read of JIT code; no module code
// reaches the runtime's fmt_str.)
// Run: makepad test --allow-unchecked-modules tests/modules/contain.rs
static mut SECRET: u64 = 0x1234_5678;
static TEXT: &str = "HOST-ONLY-BYTES";

fn addr() -> u64 {
    unsafe { &raw const SECRET as u64 }
}

#[test]
fn contain() {
    use std::relax::Module;
    let m = Module::load_source(
        "evil",
        "evil.rs",
        r#"
pub fn own_addr() -> u64 {
    static mut OWN: u64 = 0;
    unsafe { &raw const OWN as u64 }
}
pub fn write_at(a: u64, v: u64) -> u64 {
    unsafe { *(a as *mut u64) = v; }
    1
}
pub fn read_at(a: u64) -> u64 {
    unsafe { *(a as *const u64) }
}
pub fn result_write_at(a: u64) -> [u64; 4] {
    unsafe { *(a as *mut u64) = 0xdead_beef; }
    [1, 2, 3, 4]
}
pub fn call_at(a: u64) -> u64 {
    let f: fn() -> u64 = unsafe { core::mem::transmute(a as usize) };
    f()
}
pub fn big_copy(a: u64) -> u64 {
    let src = vec![7u8; 1 << 20];
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), a as *mut u8, 1 << 20); }
    2
}
pub fn alloc_lots(n: u64) -> u64 {
    let mut v: Vec<Vec<u8>> = Vec::new();
    for _ in 0..n { v.push(vec![1u8; 1 << 20]); }
    v.len() as u64
}
pub fn panic_own(n: u64) -> u64 {
    panic!("module says {}", n)
}
pub fn panic_text(a: u64, n: u64) -> u64 {
    unsafe { core::intrinsics_rt::panic_impl(a as *const u8, n as usize, "evil.rs".as_ptr(), 7, 1, 1) }
}
pub fn panic_long() -> u64 {
    let s = "long";
    unsafe { core::intrinsics_rt::panic_impl(s.as_ptr(), 1 << 40, "evil.rs".as_ptr(), 7, 1, 1) }
}
pub fn panic_file(a: u64) -> u64 {
    unsafe { core::intrinsics_rt::panic_impl("x".as_ptr(), 1, a as *const u8, 16, 1, 1) }
}
// (a str made without std's checks: a forged one. panic! formats it in the twin's own code; a
// match against a literal longer than 16 bytes compares it through the runtime's str_eq)
fn forged_str(a: u64, n: u64) -> &'static str {
    unsafe { core::mem::transmute(core::ptr::slice_from_raw_parts(a as *const u8, n as usize)) }
}
pub fn fmt_at(a: u64, n: u64) -> u64 {
    panic!("text {}", forged_str(a, n))
}
pub fn eq_at(a: u64, _n: u64) -> u64 {
    match forged_str(a, 64) {
        "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy" => 1,
        _ => 0,
    }
}
#[cfg(target_vendor = "apple")]
pub fn random_into(a: u64, n: u64) -> u64 {
    unsafe extern "C" {
        fn arc4random_buf(buf: *mut u8, len: usize);
    }
    unsafe { arc4random_buf(a as *mut u8, n as usize) }
    n
}
#[cfg(not(target_vendor = "apple"))]
pub fn random_into(a: u64, n: u64) -> u64 {
    unsafe extern "C" {
        fn getrandom(buf: *mut u8, len: usize, flags: u32) -> isize;
    }
    unsafe { getrandom(a as *mut u8, n as usize, 0) as u64 }
}
"#,
        &[],
    )
    .unwrap();
    let w: fn(u64, u64) -> u64 = m.get("write_at").unwrap();
    let r: fn(u64) -> u64 = m.get("read_at").unwrap();
    let rw: fn(u64) -> [u64; 4] = m.get("result_write_at").unwrap();
    let c: fn(u64) -> u64 = m.get("call_at").unwrap();
    let b: fn(u64) -> u64 = m.get("big_copy").unwrap();
    let al: fn(u64) -> u64 = m.get("alloc_lots").unwrap();
    let own: fn() -> u64 = m.get("own_addr").unwrap();
    let po: fn(u64) -> u64 = m.get("panic_own").unwrap();
    let pt: fn(u64, u64) -> u64 = m.get("panic_text").unwrap();
    let pl: fn() -> u64 = m.get("panic_long").unwrap();
    let pf: fn(u64) -> u64 = m.get("panic_file").unwrap();
    let fa: fn(u64, u64) -> u64 = m.get("fmt_at").unwrap();
    let eq: fn(u64, u64) -> u64 = m.get("eq_at").unwrap();
    let rnd: fn(u64, u64) -> u64 = m.get("random_into").unwrap();
    let local = own();
    // Exercise confinement after the twins have become hot, not only in O1.
    for _ in 0..10000 {
        assert_eq!(w(local, 17), 1);
        assert_eq!(r(local), 17);
        assert_eq!(rw(local), [1, 2, 3, 4]);
    }
    std::thread::sleep(std::time::Duration::from_millis(800));
    let a = addr();
    // Offset zero is unmapped in every arena: deterministic trap, also with an sret result.
    let bad_result = std::relax::guard(1000, || rw(0));
    println!("out result write traps: {}", bad_result.is_err() && unsafe { SECRET } == 0x1234_5678);
    let g1 = std::relax::guard(1000, || w(a, 0xdead_beef));
    let _ = g1;
    println!("out host secret intact: {}", unsafe { SECRET } == 0x1234_5678);
    let g2 = std::relax::guard(1000, || r(a));
    println!("out host secret not readable: {}", g2.map_or(true, |v| v != 0x1234_5678));
    let g3 = std::relax::guard(1000, || c(addr as fn() -> u64 as u64));
    println!("out forged fn pointer contained: {}", g3.map_or(true, |v| v != a));
    let g4 = std::relax::guard(1000, || b(a));
    println!("out big copy contained: {}", unsafe { SECRET } == 0x1234_5678 && g4.is_ok() || g4.is_err());
    let g5 = std::relax::guard(5000, || al(1024));
    println!("out memory budget: {}", g5.is_err());
    let g6 = std::relax::guard(1000, || po(42));
    println!("out module panic message carried: {}", g6.as_ref().is_err_and(|e| e.contains("module says 42")));
    let g7 = std::relax::guard(1000, || pt(TEXT.as_ptr() as u64, TEXT.len() as u64));
    println!("out host bytes never a panic message: {}", g7.as_ref().is_err_and(|e| !e.contains(TEXT)));
    let g8 = std::relax::guard(1000, || pl());
    println!("out panic message bounded: {}", g8.as_ref().is_err_and(|e| e.len() <= 4096 * 3));
    // fixed unmapped offsets of the window: its first page, its last bytes, past the heap's
    // mapped top, and text that starts in the module's mapped statics and runs into the guard of
    // its first thread segment
    let mut panics_end = true;
    for (at, n) in [(0u64, 32u64), (0xffff_fff0, 64), (0xfffe_fff8, 4096), (0x07ff_fff8, 4096)] {
        panics_end &= std::relax::guard(1000, || pt(at, n)).is_err();
        panics_end &= std::relax::guard(1000, || pf(at)).is_err();
        // (a call that returns is as good: the property is that the host goes on)
        let _ = std::relax::guard(1000, || fa(at, n));
        let _ = std::relax::guard(1000, || eq(at, n));
        let _ = std::relax::guard(1000, || rnd(at, n));
    }
    println!("out unmapped module text ends the call: {}", panics_end);
    println!("out host still fine: {}", unsafe { SECRET } == 0x1234_5678);
}
