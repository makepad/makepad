// rejects: expected unit struct, unit variant or constant, found tuple variant `Role::Member`
// rejects: expected unit struct, unit variant or constant, found tuple variant `Role::EmptyTup`
// rejects-at: 9:9 E0532
// rejects-at: 10:9 E0532

pub enum Role { Member(u32), EmptyTup() }
pub fn take(r: Role) {
    match r {
        Role::Member => {},
        Role::EmptyTup => {},
        _ => {}
    }
}
