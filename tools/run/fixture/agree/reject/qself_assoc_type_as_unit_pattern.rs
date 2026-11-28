// rejects: cannot find method or associated constant `Ty` in trait `Tr`
// rejects-at: 9:26 E0575

pub trait Tr { type Ty; }
pub struct S;
impl Tr for S { type Ty = u32; }

pub fn take(v: u32) {
    match v { <S as Tr>::Ty => {}, _ => {} }
}
