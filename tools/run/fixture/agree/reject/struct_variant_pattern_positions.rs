// rejects: expected unit struct, unit variant or constant, found struct variant `Role::Viewer`
// rejects: expected unit struct, unit variant or constant, found struct variant `Role::EmptyStruct`
// rejects-at: 13:28 E0533
// rejects-at: 16:25 E0533
// rejects-at: 17:29 E0533
// rejects-at: 18:9 E0533
// rejects-at: 19:12 E0533
// rejects-at: 21:15 E0533
// rejects-at: 24:13 E0533

pub enum Role { Host, Viewer { sharer: u32 }, EmptyStruct {} }

macro_rules! bad { () => { Role::Viewer }; }

pub fn positions(r: Role) {
    match (0, &r) { (0, Role::Viewer) => {}, _ => {} }
    match &r { Role::Host | Role::Viewer => {}, _ => {} }
    let Role::Viewer = r;
    if let Role::Viewer = r {}
    match &r { bad!() => {}, _ => {} }
    match r { Role::EmptyStruct => {}, _ => {} }
}

pub fn take(Role::Viewer: Role) {}
