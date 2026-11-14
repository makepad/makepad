// Memory confinement of hot code (RELAX_SANDBOX_NOCHECK=1: the compile-time refusal is off, so
// the module may use unsafe). A twin's O3 build optimises its body before the memory is confined
// (sandbox.rs early_rir / confine), so every kind of access the optimizer can leave is tried
// here once the code is hot: indexed, at a large constant offset, struct copies, loops the
// vectoriser takes, copies and fills of a known and of an unknown size, a local's address moved
// far outside it, a pointer loaded from memory, a forged fn pointer. Whatever address the
// module names, the host's memory is neither written nor read.
// Run: RELAX_SANDBOX_NOCHECK=1 RELAX_O3_CALLS=100 relax test tests/modules/contain_hot.rs
const WORDS: usize = 4096;
const MARK: u64 = 0x5ec0_0000_0000_0000;
static mut SECRET: [u64; WORDS] = [0; WORDS];
static mut CALLED: u64 = 0;

fn leak(_: u64) -> u64 {
    unsafe { CALLED += 1; }
    MARK | 77
}
fn intact() -> bool {
    unsafe { (&*(&raw const SECRET)).iter().enumerate().all(|(i, w)| *w == MARK | i as u64) && CALLED == 0 }
}
/// what a guarded call gave is not the host's
fn clean(r: Result<u64, String>) -> bool {
    r.map_or(true, |v| v >> 48 != MARK >> 48)
}

const SOURCE: &str = r#"
static mut OWN: [u64; 64] = [0; 64];
pub fn own_addr() -> u64 { unsafe { &raw const OWN as u64 } }
fn double(x: u64) -> u64 { x.wrapping_mul(2) }
pub fn fn_addr() -> u64 { double as fn(u64) -> u64 as u64 }
pub fn local_addr() -> u64 { let a = [0u64; 4]; core::hint::black_box(a.as_ptr()) as u64 }

pub fn index_read(base: u64, i: u64) -> u64 { unsafe { *(base as *const u64).add(i as usize) } }
pub fn index_write(base: u64, i: u64, v: u64) -> u64 { unsafe { *(base as *mut u64).add(i as usize) = v; } 1 }
pub fn const_off_read(base: u64) -> u64 { unsafe { *((base as *const u8).add(0x7fff_0000) as *const u64) } }
pub fn const_off_write(base: u64, v: u64) -> u64 { unsafe { *((base as *mut u8).add(0x7fff_0000) as *mut u64) = v; } 1 }
#[derive(Clone, Copy)]
pub struct Six { a: [u64; 6] }
pub fn struct_write(a: u64, v: u64) -> u64 { let b = Six { a: [v; 6] }; unsafe { *(a as *mut Six) = b; } 1 }
pub fn struct_read(a: u64) -> u64 { let b: Six = unsafe { *(a as *const Six) }; b.a[0] | b.a[5] }
pub fn sum_words(a: u64, n: u64) -> u64 {
    let s = unsafe { core::slice::from_raw_parts(a as *const u64, n as usize) };
    s.iter().fold(0u64, |x, y| x | *y)
}
pub fn fill_words(a: u64, n: u64, v: u64) -> u64 {
    let s = unsafe { core::slice::from_raw_parts_mut(a as *mut u64, n as usize) };
    for x in s.iter_mut() { *x = v; }
    n
}
pub fn fill_bytes(a: u64, n: u64, v: u64) -> u64 {
    let s = unsafe { core::slice::from_raw_parts_mut(a as *mut u8, n as usize) };
    for x in s.iter_mut() { *x = v as u8; }
    n
}
pub fn add_floats(a: u64, n: u64) -> u64 {
    let s = unsafe { core::slice::from_raw_parts_mut(a as *mut f32, n as usize) };
    for x in s.iter_mut() { *x = *x * 2.0 + 1.0; }
    n
}
pub fn copy_three(dst: u64, src: u64) -> u64 { unsafe { core::ptr::copy_nonoverlapping(src as *const u64, dst as *mut u64, 3); } 3 }
pub fn copy_bytes(dst: u64, src: u64, n: u64) -> u64 { unsafe { core::ptr::copy(src as *const u8, dst as *mut u8, n as usize); } n }
pub fn past_local_write(off: u64, v: u64) -> u64 {
    let mut a = [1u64, 2, 3, 4];
    let p = core::hint::black_box(a.as_mut_ptr());
    unsafe { *p.add(off as usize) = v; }
    a[0] + a[3]
}
pub fn past_local_read(off: u64) -> u64 {
    let a = [1u64, 2, 3, 4];
    unsafe { *core::hint::black_box(a.as_ptr()).add(off as usize) }
}
pub fn chase(a: u64) -> u64 { unsafe { let p = *(a as *const *const u64); *p } }
pub fn call_many(a: u64, n: u64) -> u64 {
    let f: fn(u64) -> u64 = unsafe { core::mem::transmute(a as usize) };
    let mut s = 0u64;
    for i in 0..n { s |= f(i); }
    s
}
"#;

#[test]
fn contain_hot() {
    use std::relax::{guard, Module};
    unsafe {
        for (i, w) in (&mut *(&raw mut SECRET)).iter_mut().enumerate() { *w = MARK | i as u64; }
    }
    let m = Module::load_source("hot", "hot.rs", SOURCE, &[]).unwrap();
    let own: fn() -> u64 = m.get("own_addr").unwrap();
    let fn_addr: fn() -> u64 = m.get("fn_addr").unwrap();
    let local_addr: fn() -> u64 = m.get("local_addr").unwrap();
    let index_read: fn(u64, u64) -> u64 = m.get("index_read").unwrap();
    let index_write: fn(u64, u64, u64) -> u64 = m.get("index_write").unwrap();
    let const_off_read: fn(u64) -> u64 = m.get("const_off_read").unwrap();
    let const_off_write: fn(u64, u64) -> u64 = m.get("const_off_write").unwrap();
    let struct_write: fn(u64, u64) -> u64 = m.get("struct_write").unwrap();
    let struct_read: fn(u64) -> u64 = m.get("struct_read").unwrap();
    let sum_words: fn(u64, u64) -> u64 = m.get("sum_words").unwrap();
    let fill_words: fn(u64, u64, u64) -> u64 = m.get("fill_words").unwrap();
    let fill_bytes: fn(u64, u64, u64) -> u64 = m.get("fill_bytes").unwrap();
    let add_floats: fn(u64, u64) -> u64 = m.get("add_floats").unwrap();
    let copy_three: fn(u64, u64) -> u64 = m.get("copy_three").unwrap();
    let copy_bytes: fn(u64, u64, u64) -> u64 = m.get("copy_bytes").unwrap();
    let past_local_write: fn(u64, u64) -> u64 = m.get("past_local_write").unwrap();
    let past_local_read: fn(u64) -> u64 = m.get("past_local_read").unwrap();
    let chase: fn(u64) -> u64 = m.get("chase").unwrap();
    let call_many: fn(u64, u64) -> u64 = m.get("call_many").unwrap();
    let (own, double) = (own(), fn_addr());
    // every function used as intended, until its twin is hot
    let mut sane = true;
    for k in 0..4000u64 {
        sane &= index_write(own, k % 32, k) == 1 && index_read(own, k % 32) == k;
        sane &= const_off_write(own.wrapping_sub(0x7fff_0000), k) == 1 && const_off_read(own.wrapping_sub(0x7fff_0000)) == k;
        sane &= struct_write(own + 64, k) == 1 && struct_read(own + 64) == k;
        sane &= fill_words(own + 128, 16, k) == 16 && sum_words(own + 128, 16) == k;
        sane &= fill_bytes(own + 256, 100, 3) == 100 && index_read(own + 256, 0) == 0x0303_0303_0303_0303;
        sane &= add_floats(own + 384, 12) == 12;
        sane &= copy_three(own + 8, own + 128) == 3 && index_read(own, 3) == k;
        sane &= copy_bytes(own + 9, own + 128, 37) == 37;
        sane &= past_local_write(k % 4, 9) >= 5 && past_local_read(k % 4) == k % 4 + 1;
        sane &= index_write(own, 40, own + 128) == 1 && chase(own + 320) == k;
        sane &= call_many(double, 5) == 14;
    }
    std::thread::sleep(std::time::Duration::from_millis(3000));
    println!("out as intended, hot: {}", sane && (0..200u64).all(|k| index_write(own, 1, k) == 1 && index_read(own, 1) == k && call_many(double, 5) == 14));
    let host = unsafe { &raw const SECRET as u64 } + (WORDS as u64 / 2) * 8;
    let leak_at = leak as fn(u64) -> u64 as u64;
    let local = local_addr();
    let g = |f: &dyn Fn() -> u64| guard(1000, || f());
    // by address
    let _ = g(&|| index_write(host, 0, 1));
    let _ = g(&|| index_write(own, host.wrapping_sub(own) / 8, 2));
    let _ = g(&|| const_off_write(host.wrapping_sub(0x7fff_0000), 3));
    let _ = g(&|| struct_write(host, 4));
    let _ = g(&|| fill_words(host, 64, 5));
    let _ = g(&|| fill_bytes(host, 600, 6));
    let _ = g(&|| add_floats(host, 64));
    let _ = g(&|| copy_three(host, own));
    let _ = g(&|| copy_bytes(host, own, 700));
    let _ = g(&|| copy_bytes(host, own, 1 << 20));
    let _ = g(&|| past_local_write(host.wrapping_sub(local) / 8, 7));
    println!("out nothing of the host written: {}", intact());
    let mut reads = vec![
        g(&|| index_read(host, 0)),
        g(&|| index_read(own, host.wrapping_sub(own) / 8)),
        g(&|| const_off_read(host.wrapping_sub(0x7fff_0000))),
        g(&|| struct_read(host)),
        g(&|| sum_words(host, 64)),
        g(&|| past_local_read(host.wrapping_sub(local) / 8)),
    ];
    // (copies out of the host, then read back from the module's own memory)
    let _ = g(&|| copy_three(own, host));
    reads.push(g(&|| index_read(own, 0)));
    let _ = g(&|| copy_bytes(own, host, 700));
    reads.push(g(&|| index_read(own, 5)));
    // (a pointer to the host stored in the module's memory, then followed)
    let _ = g(&|| index_write(own, 40, host));
    reads.push(g(&|| chase(own + 320)));
    println!("out nothing of the host read: {}", reads.into_iter().all(clean));
    println!("out a forged fn pointer does not reach the host: {}", clean(g(&|| call_many(leak_at, 3))) && intact());
    println!("out the module still runs: {}", g(&|| index_write(own, 1, 5) + index_read(own, 1)) == Ok(6));
    m.unload();
}
