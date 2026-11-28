// rejects: expected unit struct, unit variant or constant, found tuple struct
// rejects-at: 13:15 E0532
// rejects-at: 14:15 E0532
// rejects-at: 15:15 E0532
// rejects-at: 16:21 E0532

pub struct Pair(pub u32);
pub struct EmptyTup();
pub mod m { pub use super::Pair; }
macro_rules! pass { ($p:path) => { $p }; }

pub fn take(p: Pair, e: EmptyTup) {
    match p { self::Pair => {}, _ => {} }
    match p { m::Pair => {}, _ => {} }
    match e { self::EmptyTup => {}, _ => {} }
    match p { pass!(Pair) => {}, _ => {} }
}
