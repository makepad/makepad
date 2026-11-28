// rejects: expected unit struct, unit variant or constant, found associated function `Role::make`
// rejects: expected unit struct, unit variant or constant, found associated function `Self::make`
// rejects-at: 11:22 E0533
// rejects-at: 12:22 E0533
// rejects-at: 13:22 E0533

pub enum Role { Host }
impl Role {
    pub fn make() -> Self { Self::Host }
    pub fn take(self) {
        match self { Role::make => {}, _ => {} }
        match self { Self::make => {}, _ => {} }
        match self { <Role>::make => {}, _ => {} }
    }
}
