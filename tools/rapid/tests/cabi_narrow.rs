// Narrow integer results (i8, i16, i32, u8, u16, bool) of C functions, called directly
// and through `extern "C" fn` pointers (from the libc symbol and from Rapid's own
// `extern "C"` bodies): C leaves the upper register bits undefined, so the caller must
// re-extend them. Output matches rustc.

extern "C" {
    fn abs(x: i32) -> i32;
    fn atoi(s: *const u8) -> i32;
    fn strtol(s: *const u8, end: *mut *const u8, base: i32) -> i64;
    // libc functions as pointers, the way std's sys layer gets optional APIs
    #[link_name = "dlsym"]
    fn dlsym_atoi(h: *mut u8, name: *const u8) -> unsafe extern "C" fn(*const u8) -> i32;
    #[link_name = "dlsym"]
    fn dlsym_abs(h: *mut u8, name: *const u8) -> unsafe extern "C" fn(i32) -> i32;
    // syscall wrappers typically return -1 in a 32-bit register (upper bits zero)
    #[link_name = "dlsym"]
    fn dlsym_close(h: *mut u8, name: *const u8) -> unsafe extern "C" fn(i32) -> i32;
    #[link_name = "dlsym"]
    fn dlsym_dup(h: *mut u8, name: *const u8) -> unsafe extern "C" fn(i32) -> i32;
    fn close(fd: i32) -> i32;
    fn dlsym(h: *mut u8, name: *const u8) -> *mut u8;
}

type IntFn = unsafe extern "C" fn(i32) -> i32;
type WakeFn = unsafe extern "C" fn(*mut u8, usize, u32) -> i32;

static mut WORD: u32 = 0;

/// os_sync_wake_by_address_any with no waiter returns -1 in w0 (x0's upper half zero).
#[cfg(target_os = "macos")]
fn os_sync_line() {
    let raw = unsafe { dlsym(RTLD_DEFAULT as *mut u8, cstr("os_sync_wake_by_address_any\0")) } as usize;
    if raw == 0 {
        println!("os_sync -1 true");
        return;
    }
    let wake: WakeFn = unsafe { core::mem::transmute(raw) };
    let r = unsafe { wake(&mut WORD as *mut u32 as *mut u8, 4, 0) };
    println!("os_sync {} {}", r, r < 0);
}

#[cfg(not(target_os = "macos"))]
fn os_sync_line() {
    println!("os_sync -1 true");
}

#[cfg(target_os = "macos")]
const RTLD_DEFAULT: isize = -2;
#[cfg(not(target_os = "macos"))]
const RTLD_DEFAULT: isize = 0;

// C bodies whose results have garbage-prone upper bits after their own arithmetic
extern "C" fn neg_i8(x: i32) -> i8 {
    (0 - x) as i8
}
extern "C" fn neg_i16(x: i32) -> i16 {
    (0 - x) as i16
}
extern "C" fn neg_i32(x: i64) -> i32 {
    (0 - x) as i32
}
extern "C" fn trunc_u8(x: i32) -> u8 {
    x as u8
}
extern "C" fn trunc_u16(x: i32) -> u16 {
    x as u16
}
extern "C" fn is_neg(x: i32) -> bool {
    x < 0
}

fn via_ptr_i32(f: extern "C" fn(i64) -> i32, x: i64) -> i64 {
    f(x) as i64
}

fn cstr(s: &str) -> *const u8 {
    s as *const str as *const u8
}

#[test]
fn direct_and_pointer_narrow_returns() {
    let a = unsafe { atoi(cstr("-1\0")) };
    let b = unsafe { abs(-7) };
    println!("direct {} {} {}", a, b, unsafe { strtol(cstr("-42\0"), 0 as *mut *const u8, 10) });
    let p_atoi = unsafe { dlsym_atoi(RTLD_DEFAULT as *mut u8, cstr("atoi\0")) };
    let p_abs = unsafe { dlsym_abs(RTLD_DEFAULT as *mut u8, cstr("abs\0")) };
    let c = unsafe { p_atoi(cstr("-1\0")) };
    let d = unsafe { p_atoi(cstr("-2147483648\0")) };
    println!("ptr libc {} {} {} {}", c, d, unsafe { p_abs(-5) }, c < 0);
    let p_close = unsafe { dlsym_close(RTLD_DEFAULT as *mut u8, cstr("close\0")) };
    let p_dup = unsafe { dlsym_dup(RTLD_DEFAULT as *mut u8, cstr("dup\0")) };
    let e1 = unsafe { p_close(-1) };
    let e2 = unsafe { p_dup(-5) };
    let e3 = unsafe { close(-1) };
    println!("ptr syscall {} {} direct {} {} {}", e1, e2, e3, e1 < 0, e1 == -1);
    // an untyped dlsym result transmuted to a fn pointer type (std's sys-layer pattern)
    let raw = unsafe { dlsym(RTLD_DEFAULT as *mut u8, cstr("close\0")) } as usize;
    let t_close: IntFn = unsafe { core::mem::transmute(raw) };
    let e4 = unsafe { t_close(-1) };
    println!("transmuted {} {} {}", e4, e4 < 0, e4 == -1);
    os_sync_line();
    let f8: extern "C" fn(i32) -> i8 = neg_i8;
    let f16: extern "C" fn(i32) -> i16 = neg_i16;
    let f32_: extern "C" fn(i64) -> i32 = neg_i32;
    let fu8: extern "C" fn(i32) -> u8 = trunc_u8;
    let fu16: extern "C" fn(i32) -> u16 = trunc_u16;
    let fb: extern "C" fn(i32) -> bool = is_neg;
    println!("ptr i8 {} {} i16 {} {}", f8(1), f8(-200), f16(1), f16(40000));
    println!("ptr i32 {} {} {}", f32_(1), f32_(-2147483648), via_ptr_i32(neg_i32, 5));
    println!("ptr u8 {} {} u16 {} {}", fu8(-1), fu8(300), fu16(-1), fu16(70000));
    println!("ptr bool {} {} {}", fb(-3), fb(3), fb(-3) as i32 + fb(3) as i32);
    println!("direct bodies {} {} {}", neg_i8(1), neg_i32(7), is_neg(-1));
}
