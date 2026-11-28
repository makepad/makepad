// rejects: expected unit struct, unit variant or constant, found tuple variant `Self::Member`
// rejects: expected unit struct, unit variant or constant, found tuple variant `Alias::Member`
// rejects: expected unit struct, unit variant or constant, found tuple variant `Role::Member`
// rejects-at: 12:22 E0533
// rejects-at: 13:22 E0533
// rejects-at: 14:22 E0533

pub enum Role { Member(u32) }
pub type Alias = Role;
impl Role {
    pub fn take(self) {
        match self { Self::Member => {}, _ => {} }
        match self { Alias::Member => {}, _ => {} }
        match self { <Role>::Member => {}, _ => {} }
    }
}
