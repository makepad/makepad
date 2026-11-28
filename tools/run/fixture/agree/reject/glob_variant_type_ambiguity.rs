// rejects: E0659
// rejects-at: 16:13 E0659
// rejects-at: 23:20 E0659
// rejects-at: 25:19 E0659
// rejects-at: 33:13 E0659

pub mod a { pub enum A { Same { x: u32 } } }
pub mod b { pub enum B { Same { y: u32 } } }
pub mod tuple { pub struct Same(pub u32); }
pub mod unit { pub enum B { Same, Other } }

#[allow(unused_imports)]
pub fn two_braced() {
    use a::A::*;
    use b::B::*;
    let _ = Same { x: 1 };
}

#[allow(unused_imports)]
pub mod tuple_beside_braced {
    use super::tuple::*;
    use super::a::A::*;
    pub fn take(_: Same) {}
    pub fn check(s: super::tuple::Same) {
        match s { Same { 0: _ } => {} }
    }
}

#[allow(unused_imports)]
pub fn unit_beside_braced() {
    use a::A::*;
    use unit::B::*;
    let _ = Same { x: 1 };
}
