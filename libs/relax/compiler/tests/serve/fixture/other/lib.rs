#![no_std]
/// Another package's type.
pub struct Other { pub count: u32 }
/// A crate-local trait.
pub trait Read { fn read(&self) -> u32; }
impl Read for Other { fn read(&self) -> u32 { self.count } }
pub enum Choice { Empty, Value(u32), Named { count: u32 } }
