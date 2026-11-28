// rejects: cannot find unit struct, unit variant or constant `Viewer` in module `self`
// rejects: cannot find unit struct, unit variant or constant `Viewer` in module `m`
// rejects: cannot find unit struct, unit variant or constant `Viewer` in this scope
// rejects-at: 14:21 E0532
// rejects-at: 15:18 E0532
// rejects-at: 16:21 E0532

pub enum Role { Viewer { sharer: u32 } }
use Role::Viewer;
pub mod m { pub use super::Role::Viewer; }
macro_rules! pass { ($p:path) => { $p }; }

pub fn take(r: Role) {
    match r { self::Viewer => {}, _ => {} }
    match r { m::Viewer => {}, _ => {} }
    match r { pass!(Viewer) => {}, _ => {} }
}
