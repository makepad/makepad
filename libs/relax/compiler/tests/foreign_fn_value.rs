// Foreign fn items as values: fn-pointer coercion (return, argument, let), `as` casts to fn
// pointers / usize / raw pointers, stored in a static and a const, inside a generic fn, and
// the objc-sys shape (`extern "C" { fn f(); }` taken as `unsafe extern "C" fn()`, transmuted to
// the real signature, called). The value is the symbol's real address (= dlsym), as in rustc.
extern "C" {
    fn abs(x: i32) -> i32;
    fn labs(x: i64) -> i64;
    fn dlsym(handle: *mut u8, name: *const u8) -> *mut u8;
}
mod objc_like {
    extern "C" {
        pub fn abs();
    }
}
type Imp = unsafe extern "C" fn();
fn imp() -> Imp {
    objc_like::abs
}
static ABS_PTR: unsafe extern "C" fn(i32) -> i32 = abs;
const LABS_PTR: unsafe extern "C" fn(i64) -> i64 = labs;
fn call_with(f: unsafe extern "C" fn(i32) -> i32, x: i32) -> i32 {
    unsafe { f(x) }
}
fn generic_call<F: Fn(i32) -> i32>(f: F, x: i32) -> i32 {
    f(x)
}

#[test]
fn values() {
    // RTLD_DEFAULT: -2 on macOS, 0 on Linux
    let default = if cfg!(target_os = "macos") { -2isize as usize } else { 0 };
    let real = unsafe { dlsym(default as *mut u8, b"abs\0".as_ptr()) } as usize;
    let f: unsafe extern "C" fn(i32) -> i32 = abs;
    println!("let coercion {}", unsafe { f(-3) });
    println!("argument {}", call_with(abs, -4));
    println!("static {} const {}", unsafe { ABS_PTR(-6) }, unsafe { LABS_PTR(-7) });
    let g: extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(imp()) };
    println!("objc shape {}", g(-5));
    println!("generic {}", generic_call(|x| unsafe { abs(x) }, -8));
    let as_usize = abs as usize;
    let as_ptr = abs as *const u8 as usize;
    let via_fn = f as usize;
    let via_imp = imp() as usize;
    println!("same address: cast {} ptr {} coerced {} imp {} static {}", as_usize == real, as_ptr == real, via_fn == real, via_imp == real, ABS_PTR as usize == real);
}
