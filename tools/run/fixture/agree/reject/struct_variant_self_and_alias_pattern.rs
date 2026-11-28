// rejects: expected unit struct, unit variant or constant, found struct variant `Self::Viewer`
// rejects: expected unit struct, unit variant or constant, found struct variant `Alias::Viewer`
// rejects: expected unit struct, unit variant or constant, found struct variant `Role::Viewer`
// rejects: expected unit struct, unit variant or constant, found struct variant `<<Holder as HasRole>::Out>::Viewer`
// rejects-at: 18:22 E0533
// rejects-at: 23:15 E0533
// rejects-at: 24:15 E0533
// rejects-at: 25:15 E0533

pub enum Role { Host, Member(u32), Viewer { sharer: u32 } }
pub type Alias = Role;
pub struct Holder;
pub trait HasRole { type Out; }
impl HasRole for Holder { type Out = Role; }

impl Role {
    pub fn via_self(self) {
        match self { Self::Viewer => {}, _ => {} }
    }
}

pub fn aliases(r: Role) {
    match r { Alias::Viewer => {}, _ => {} }
    match r { <Role>::Viewer => {}, _ => {} }
    match r { <Holder as HasRole>::Out::Viewer => {}, _ => {} }
}
