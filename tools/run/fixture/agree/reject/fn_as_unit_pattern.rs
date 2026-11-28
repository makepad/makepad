// rejects: expected unit struct, unit variant or constant, found function `self::helper`
// rejects: expected unit struct, unit variant or constant, found associated function `Tr::method`
// rejects-at: 14:9 E0532
// rejects-at: 15:9 E0532
// rejects-at: 16:9 E0532

pub struct Role;
pub trait Tr { fn method(); }
impl Tr for Role { fn method() {} }
pub fn helper() {}

pub fn take(r: Role) {
    match r {
        self::helper => {},
        Tr::method => {},
        <Role as Tr>::method => {},
        _ => {}
    }
}
