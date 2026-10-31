// rustc accepts: moving out of `*self` where the type is `!` (the never type is Copy).
#![feature(never_type)]
trait Show { fn show(&self) -> u32; }
impl Show for ! { fn show(&self) -> u32 { *self } }
fn takes(n: &!) -> u32 { *n }
fn main() {
    let r: Result<u32, !> = Ok(3);
    match r { Ok(v) => assert_eq!(v, 3), Err(e) => { let _ = takes(&e); } }
}

#[test]
fn never_deref_move_accept() { main(); }
