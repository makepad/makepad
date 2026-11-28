// rejects: E0530
// rejects-at: 17:15 E0530
// rejects-at: 24:15 E0530
// rejects-at: 31:19 E0530

pub mod a { pub enum A { Same { x: u32 } } }
#[allow(non_upper_case_globals)]
pub mod statics { pub static Same: u32 = 1; }
pub mod tuple { pub struct Same(pub u32); }
#[allow(non_upper_case_globals)]
pub mod constants { pub const Same: u32 = 1; }

#[allow(unused_imports)]
pub fn static_shadow(v: u32) {
    use a::A::*;
    use statics::*;
    match v { Same => {} }
}

#[allow(unused_imports)]
pub fn tuple_shadow(s: tuple::Same) {
    use a::A::*;
    use tuple::*;
    match s { Same => {} }
}

#[allow(unused_imports)]
pub fn const_shadow(v: u32) {
    use a::A::*;
    use constants::*;
    match v { mut Same => {} }
}
