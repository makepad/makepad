// rejects: E0282
pub fn f<T>() -> usize { [0u8; std::mem::size_of::<T>()].len() }
