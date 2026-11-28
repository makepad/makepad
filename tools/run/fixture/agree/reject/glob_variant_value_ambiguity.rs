// rejects: E0659
// rejects-at: 31:13 E0659
// rejects-at: 32:15 E0659
// rejects-at: 39:13 E0659
// rejects-at: 40:15 E0659
// rejects-at: 48:13 E0659
// rejects-at: 49:15 E0659
// rejects-at: 56:13 E0659
// rejects-at: 57:15 E0659
// rejects-at: 64:13 E0659
// rejects-at: 65:15 E0659
// rejects-at: 72:13 E0659
// rejects-at: 73:15 E0659
// rejects-at: 80:13 E0659
// rejects-at: 81:15 E0659

pub mod unit { pub enum A { Same, Other } }
pub mod tuple { pub enum A { Same(u32), Other } }
pub mod braced { pub enum A { Same { x: u32 } } }
#[allow(non_upper_case_globals)]
pub mod b { pub const Same: u32 = 1; }
#[allow(non_upper_case_globals)]
pub mod c { pub const Same: u32 = 2; }
#[allow(non_upper_case_globals)]
pub mod statics { pub static Same: u32 = 1; }

#[allow(unused_imports)]
pub fn unit_first(v: u32) -> u32 {
    use unit::A::*;
    use b::*;
    let _ = Same;
    match v { Same => 1, _ => 2 }
}

#[allow(unused_imports)]
pub fn unit_last(v: u32) -> u32 {
    use b::*;
    use unit::A::*;
    let _ = Same;
    match v { Same => 1, _ => 2 }
}

#[allow(unused_imports)]
pub fn two_consts(v: u32) -> u32 {
    use braced::A::*;
    use b::*;
    use c::*;
    let _ = Same;
    match v { Same => 1, _ => 2 }
}

#[allow(unused_imports)]
pub fn tuple_first(v: u32) -> u32 {
    use tuple::A::*;
    use b::*;
    let _ = Same;
    match v { Same => 1, _ => 2 }
}

#[allow(unused_imports)]
pub fn tuple_last(v: u32) -> u32 {
    use b::*;
    use tuple::A::*;
    let _ = Same;
    match v { Same => 1, _ => 2 }
}

#[allow(unused_imports)]
pub fn static_first(v: u32) -> u32 {
    use statics::*;
    use b::*;
    let _ = Same;
    match v { Same => 1, _ => 2 }
}

#[allow(unused_imports)]
pub fn static_last(v: u32) -> u32 {
    use b::*;
    use statics::*;
    let _ = Same;
    match v { Same => 1, _ => 2 }
}
