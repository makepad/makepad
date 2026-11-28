// rejects: expected a type, found a trait
// rejects-at: 7:15 E0782

pub trait Tr { type Ty; }

pub fn take(v: u32) {
    match v { Tr::Ty => {}, _ => {} }
}
