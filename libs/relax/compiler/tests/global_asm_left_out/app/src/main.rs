// The app's own global_asm! is placed and runs although gal-wide's is left out; a call to a
// left-out crate's symbol panics, naming that crate and why.
#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
std::arch::global_asm!(
    ".globl gal_app_add",
    "gal_app_add:",
    "  lea rax, [rdi + rsi]",
    "  ret",
);

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
extern "C" {
    fn gal_app_add(a: u64, b: u64) -> u64;
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn main() {
    println!("app: {}", unsafe { gal_app_add(2, 40) });
    report("wide", gal_wide::probe);
    report("user", gal_user::call);
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn report(what: &str, f: fn() -> u32) {
    match std::panic::catch_unwind(f) {
        Ok(v) => println!("{what}: {v}"),
        Err(e) => {
            let msg = e.downcast_ref::<String>().map(String::as_str).or_else(|| e.downcast_ref::<&str>().copied()).unwrap_or("?");
            println!("{what}: panicked: {msg}");
        }
    }
}

#[cfg(not(all(target_arch = "x86_64", target_os = "linux")))]
fn main() {
    println!("x86_64 Linux only");
}
