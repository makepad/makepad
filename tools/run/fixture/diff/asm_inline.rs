// diff_fast corpus program: inline assembly on x86_64 and aarch64 -- register classes, explicit registers, inout /
// lateout / discarded outputs, template modifiers, local labels, sym calls with clobber_abi("C"), float and
// vector operands, values live across the block, noreturn. Every result line is `D ...`.
use std::arch::asm;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

extern "C" fn callee(x: u64) -> u64 { x.wrapping_mul(3).wrapping_add(1) }

#[cfg(target_arch = "x86_64")]
mod a {
    use super::*;
    // the call passes x in rdi on every OS (Windows' C ABI takes it in rcx): a SysV callee
    extern "sysv64" fn callee_sysv(x: u64) -> u64 { callee(x) }
    #[inline(never)]
    pub fn add(a: u64, b: u64) -> u64 { let r: u64; unsafe { asm!("lea {r}, [{a} + {b}*2 + 7]", a = in(reg) a, b = in(reg) b, r = lateout(reg) r, options(pure, nomem, nostack)) }; r }
    #[inline(never)]
    pub fn inout(mut x: u64, k: u32) -> u64 { unsafe { asm!("shl {x}, cl", "xor {x}, 0x55", x = inout(reg) x, in("cl") k as u8, options(nostack)) }; x }
    #[inline(never)]
    pub fn sub32(a: u32, b: u32) -> u32 { let r: u32; unsafe { asm!("mov {r:e}, {a:e}", "sub {r:e}, {b:e}", a = in(reg) a, b = in(reg) b, r = out(reg) r, options(pure, nomem, nostack)) }; r }
    #[inline(never)]
    pub fn byte(a: u8) -> u8 { let r: u8; unsafe { asm!("mov {r}, {a}", "add {r}, 3", a = in(reg_byte) a, r = out(reg_byte) r) }; r }
    #[inline(never)]
    pub fn loop_sum(n: u64) -> u64 {
        let r: u64;
        unsafe { asm!("xor {r:e}, {r:e}", "2:", "add {r}, {n}", "dec {n}", "jnz 2b", n = inout(reg) n => _, r = out(reg) r, options(nostack)) };
        r
    }
    #[inline(never)]
    pub fn mem(p: &mut [u64; 4]) { unsafe { asm!("mov rax, qword ptr [{p} + 8]", "add qword ptr [{p}], rax", "mov dword ptr [{p} + 24], 0x12345", p = in(reg) p.as_mut_ptr(), out("rax") _, options(nostack)) } }
    #[inline(never)]
    pub fn call(x: u64) -> u64 {
        let r: u64;
        unsafe { asm!("call {f}", f = sym callee_sysv, inout("rdi") x => _, out("rax") r, clobber_abi("sysv64")) };
        r
    }
    #[inline(never)]
    pub fn sqrt(x: f64) -> f64 { let r: f64; unsafe { asm!("sqrtsd {r}, {x}", x = in(xmm_reg) x, r = lateout(xmm_reg) r, options(pure, nomem, nostack)) }; r }
    #[inline(never)]
    pub fn fbits(x: f32) -> u32 { let r: u32; unsafe { asm!("mov {r:e}, {x:e}", x = in(reg) x, r = out(reg) r, options(pure, nomem, nostack)) }; r }
    #[inline(never)]
    pub fn cpuid0() -> u32 { let r: u32; unsafe { asm!("mov {t:r}, rbx", "cpuid", "mov rbx, {t:r}", t = out(reg) _, inout("eax") 0u32 => r, out("ecx") _, out("edx") _) }; r.min(1) }
    #[inline(never)]
    pub fn noret() -> ! { unsafe { asm!("ud2", options(noreturn)) } }
}

#[cfg(target_arch = "aarch64")]
mod a {
    use super::*;
    #[inline(never)]
    pub fn add(a: u64, b: u64) -> u64 { let r: u64; unsafe { asm!("add {r}, {a}, {b}, lsl #1", "add {r}, {r}, #7", a = in(reg) a, b = in(reg) b, r = lateout(reg) r, options(pure, nomem, nostack)) }; r }
    #[inline(never)]
    pub fn inout(mut x: u64, k: u32) -> u64 { unsafe { asm!("lsl {x}, {x}, {k}", "eor {x}, {x}, #0xff00", x = inout(reg) x, k = in(reg) k as u64, options(nostack)) }; x }
    #[inline(never)]
    pub fn sub32(a: u32, b: u32) -> u32 { let r: u32; unsafe { asm!("sub {r:w}, {a:w}, {b:w}", a = in(reg) a, b = in(reg) b, r = out(reg) r, options(pure, nomem, nostack)) }; r }
    #[inline(never)]
    pub fn byte(a: u8) -> u8 { let r: u8; unsafe { asm!("add {r:w}, {a:w}, #3", a = in(reg) a, r = out(reg) r) }; r }
    #[inline(never)]
    pub fn loop_sum(n: u64) -> u64 {
        let r: u64;
        unsafe { asm!("mov {r}, #0", "2:", "add {r}, {r}, {n}", "subs {n}, {n}, #1", "b.ne 2b", n = inout(reg) n => _, r = out(reg) r, options(nostack)) };
        r
    }
    #[inline(never)]
    pub fn mem(p: &mut [u64; 4]) { unsafe { asm!("ldr x9, [{p}, #8]", "ldr x10, [{p}]", "add x10, x10, x9", "str x10, [{p}]", "mov w9, #0x2345", "movk w9, #1, lsl #16", "str w9, [{p}, #24]", p = in(reg) p.as_mut_ptr(), out("x9") _, out("x10") _, options(nostack)) } }
    #[inline(never)]
    pub fn call(x: u64) -> u64 {
        let r: u64;
        unsafe { asm!("bl {f}", f = sym callee, inout("x0") x => r, clobber_abi("C")) };
        r
    }
    #[inline(never)]
    pub fn sqrt(x: f64) -> f64 { let r: f64; unsafe { asm!("fsqrt {r:d}, {x:d}", x = in(vreg) x, r = lateout(vreg) r, options(pure, nomem, nostack)) }; r }
    #[inline(never)]
    pub fn fbits(x: f32) -> u32 { let r: u32; unsafe { asm!("mov {r:w}, {x:w}", x = in(reg) x, r = out(reg) r, options(pure, nomem, nostack)) }; r }
    #[inline(never)]
    pub fn cpuid0() -> u32 { let r: u64; unsafe { asm!("mrs {}, fpcr", out(reg) r, options(nomem, nostack, preserves_flags)) }; (r != u64::MAX) as u32 }
    #[inline(never)]
    pub fn noret() -> ! { unsafe { asm!("brk #0", options(noreturn)) } }
}

#[inline(never)]
fn live_across(v: &[u64]) -> u64 {
    // many values live across the block: none may sit in its registers
    let (x0, x1, x2, x3, x4, x5) = (v[0], v[1], v[2], v[3], v[4], v[5]);
    let y = a::add(x0, x1);
    let z = a::inout(x2, 3);
    x0.wrapping_mul(x1) ^ x2.rotate_left(5) ^ x3 ^ x4.wrapping_add(x5) ^ y ^ z
}

fn work() {
    let vals = [0u64, 1, 7, 0xFFFF_FFFF, u64::MAX, 0x0123_4567_89AB_CDEF];
    for &x in &vals { for &y in &vals { d!("add {} {} = {}", x, y, a::add(x, y)); } }
    for &x in &vals { for k in [0u32, 1, 13, 63] { d!("inout {} {} = {}", x, k, a::inout(x, k)); } }
    for &x in &[0u32, 5, u32::MAX] { for &y in &[0u32, 9, 0x8000_0000] { d!("sub32 {} {} = {}", x, y, a::sub32(x, y)); } }
    for b in [0u8, 1, 200, 253, 255] { d!("byte {} = {}", b, a::byte(b)); }
    for n in [1u64, 2, 10, 1000] { d!("loop {} = {}", n, a::loop_sum(n)); }
    let mut m = [5u64, 37, 0, 0];
    a::mem(&mut m);
    d!("mem {:?}", m);
    for x in [0u64, 1, 41, u64::MAX] { d!("call {} = {}", x, a::call(x)); }
    for x in [0.0f64, 2.0, 1e300, -1.0] { d!("sqrt {} = {:?}", x, a::sqrt(x).to_bits()); }
    for x in [0.0f32, 1.5, -2.25] { d!("fbits {} = {:#x}", x, a::fbits(x)); }
    d!("cpuid0 {}", a::cpuid0());
    d!("live {}", live_across(&vals));
    if std::hint::black_box(false) { a::noret(); }
}
