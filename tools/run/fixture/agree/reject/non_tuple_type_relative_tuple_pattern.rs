// rejects: expected tuple struct or tuple variant, found unit struct `Self`
// rejects: expected tuple struct or tuple variant, found unit variant `Self::Host`
// rejects: expected tuple struct or tuple variant, found struct variant `Self::Viewer`
// rejects: expected tuple struct or tuple variant, found associated function `Self::make`
// rejects: expected tuple struct or tuple variant, found struct variant `Role::Viewer`
// rejects: expected tuple struct or tuple variant, found associated function `Role::make`
// rejects: expected tuple struct or tuple variant, found self constructor `Self`
// rejects-at: 24:22 E0164
// rejects-at: 25:22 E0164
// rejects-at: 26:22 E0164
// rejects-at: 27:22 E0164
// rejects-at: 28:22 E0164
// rejects-at: 34:5 E0164
// rejects-at: 39:5 E0164
// rejects-at: 44:46 E0164
// rejects-at: 46:48 E0164
// rejects-at: 46:48 the `Self` constructor can only be used with tuple or unit structs

pub enum Role { Host, Viewer { sharer: u32 } }
impl Role {
    pub fn make(_: u32) -> Self { Self::Host }

    pub fn patterns(self) {
        match self { Self::Host() => {}, _ => {} }
        match self { Self::Viewer(x) => { let _ = x; }, _ => {} }
        match self { Self::make(y) => { let _ = y; }, _ => {} }
        match self { Role::Viewer(x) => { let _ = x; }, _ => {} }
        match self { Role::make(y) => { let _ = y; }, _ => {} }
    }
}

pub fn assign_viewer(r: Role) {
    let v: u32;
    Role::Viewer(v) = r;
    let _ = v;
}
pub fn assign_make(r: Role) {
    let v: u32;
    Role::make(v) = r;
    let _ = v;
}

pub struct Unit;
impl Unit { pub fn take(self) { match self { Self() => {} } } }
pub struct Braced { pub x: u32 }
impl Braced { pub fn take(self) { match self { Self(x) => { let _ = x; } } } }
