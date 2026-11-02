// rejects: E0282
pub fn f<T: ?Sized>() -> usize { [0u8; std::mem::size_of::<*const T>()].len() }
