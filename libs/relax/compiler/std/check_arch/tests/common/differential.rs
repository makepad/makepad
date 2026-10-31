const RANDOM_CASES: usize = 4096;
unsafe fn cast<T: Copy, U: Copy>(x: T) -> U {
    assert_eq!(size_of::<T>(), size_of::<U>());
    std::ptr::read_unaligned((&x as *const T).cast::<U>())
}

// Use literal const generics so rustc checks every selected immediate too.
macro_rules! immediate_cases {
    ($check:ident, $args:expr, [$($imm:literal),* $(,)?]) => {
        $($check::<$imm>($args);)*
    };
}
