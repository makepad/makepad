// A repeat count may name its parent's generics when its value does not need them.
const fn four<T>() -> usize { 4 }
fn thin<T>() -> usize { [0u8; std::mem::size_of::<*const T>()].len() }
fn called<T>() -> usize { [0u8; four::<T>()].len() }
fn shadow<const N: usize>() -> usize { [0u8; { const N: usize = 3; N + 1 }].len() }
#[test] fn counts() { assert_eq!(thin::<u32>(), 8); assert_eq!(called::<String>(), 4); assert_eq!(shadow::<9>(), 4); }
