// rejects: constant expression depends on a generic parameter
pub fn f<T>() -> usize { [0u8; std::mem::size_of::<T>()].len() }
