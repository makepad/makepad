// rejects: expected unit struct, found self constructor `Self`
// rejects-at: 14:46 E0533
// rejects-at: 15:48 E0533
// rejects-at: 16:46 E0533
// rejects-at: 17:59 E0533
// rejects-at: 18:52 E0533

pub struct Pair(pub u32);
pub struct Braced { pub value: u32 }
pub type BracedAlias = Braced;
pub enum Role { Host }
pub trait NonAdt { fn take(self); }

impl Pair { pub fn take(self) { match self { Self => {} } } }
impl Braced { pub fn take(self) { match self { Self => {} } } }
impl Role { pub fn take(self) { match self { Self => {} } } }
impl BracedAlias { pub fn take_alias(self) { match self { Self => {} } } }
impl NonAdt for u32 { fn take(self) { match self { Self => {} } } }
