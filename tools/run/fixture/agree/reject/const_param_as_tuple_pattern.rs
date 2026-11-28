// rejects: expected tuple struct or tuple variant, found const parameter `N`
// rejects: expected tuple struct or tuple variant, found unit variant `Role::Host`
// rejects: expected a pattern, found a function call
// rejects: expected unit struct, unit variant or constant, found const parameter `N`
// rejects-at: 14:21 E0532
// rejects-at: 15:15 E0532
// rejects-at: 16:15 E0532
// rejects-at: 17:15 E0532

macro_rules! pass { ($p:path) => { $p }; }
pub enum Role { Host }
pub fn helper(_: u32) {}
pub fn take<const N: u32>(v: u32, r: Role) {
    match v { pass!(N) => {}, _ => {} }
    match v { N(_) => {}, _ => {} }
    match r { Role::Host() => {}, _ => {} }
    match v { self::helper(_) => {}, _ => {} }
}
