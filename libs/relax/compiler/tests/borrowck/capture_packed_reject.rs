// Packed captures are truncated regardless of field alignment.
// rustc_hir_typeck/upvar.rs:1998. The upstream repr_packed.rs requires nightly.
//@ edition:2021
#[repr(packed)]
struct Packed { x: u8, y: u8 }

fn main() {
    let mut p = Packed { x: 0, y: 0 };
    let mut c = || { p.x += 1; };
    let other = &mut p.y; //~ ERROR cannot borrow
    c();
    *other += 1;
}
