// `b` was added and the literal below was not given it: E0063, so cargo builds no test above this.
pub struct D {
    pub a: u32,
    pub b: u32,
}

pub fn made() -> D {
    D { a: 4 }
}
