// Relax issue 231: `#[unsafe(no_mangle)]`, `#[unsafe(export_name = ..)]` and
// `#[unsafe(link_section = ..)]` are unsafe code as their plain forms are (rustc_attr_parsing
// safety.rs: every use of an unsafe builtin attribute), so an `#[expect(unsafe_code)]` on them is
// fulfilled, and an exported fn is a dead-code root: what only it calls is used. Relax read such an
// attribute's path as `unsafe`: each expectation went unfulfilled and the exported fns' callees
// were reported never used. The warnings below are rustc 1.100's, at the attribute.
#![warn(unsafe_code)]

#[expect(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn rx231_exported() -> u32 {
    only_from_export()
}
fn only_from_export() -> u32 {
    7
}

#[expect(unsafe_code)]
#[unsafe(export_name = "rx231_named")]
pub extern "C" fn named() -> u32 {
    also_from_export()
}
fn also_from_export() -> u32 {
    8
}

#[expect(unsafe_code)]
#[cfg_attr(target_os = "macos", unsafe(link_section = "__DATA,__rx231"))]
#[cfg_attr(not(target_os = "macos"), unsafe(link_section = ".data.rx231"))]
#[unsafe(no_mangle)]
pub static RX231_SECTION: u32 = 1;

#[no_mangle]
pub extern "C" fn plain() -> u32 {
    from_plain()
}
fn from_plain() -> u32 {
    9
}

#[unsafe(no_mangle)]
pub static RX231_STATIC: u32 = 2;

fn never_called() {}

fn main() {
    println!("main");
}
