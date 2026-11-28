// rejects: cannot find unit struct, unit variant or constant `Record` in module `self`
// rejects: cannot find unit struct, unit variant or constant `Record` in module `m`
// rejects: cannot find unit struct, unit variant or constant `Empty` in module `self`
// rejects: cannot find unit struct, unit variant or constant `Record` in this scope
// rejects-at: 16:21 E0532
// rejects-at: 17:18 E0532
// rejects-at: 18:21 E0532
// rejects-at: 19:21 E0532

pub struct Record { pub value: u32 }
pub struct Empty {}
pub mod m { pub use super::Record; }
macro_rules! pass { ($p:path) => { $p }; }

pub fn take(r: Record, e: Empty) {
    match r { self::Record => {}, _ => {} }
    match r { m::Record => {}, _ => {} }
    match e { self::Empty => {}, _ => {} }
    match r { pass!(Record) => {}, _ => {} }
}
