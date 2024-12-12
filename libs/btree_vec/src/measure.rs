pub trait Measure: Copy {
    fn empty() -> Self;

    fn combine(self, other: Self) -> Self;
}

impl Measure for () {
    fn empty() -> Self {
        ()
    }

    fn combine(self, _other: Self) -> Self {
        ()
    }
}

macro_rules! impl_measure_for_int {
    ($($t:ty),*) => {
        $(
            impl Measure for $t {
                fn empty() -> Self {
                    0
                }

                fn combine(self, other: Self) -> Self {
                    self + other
                }
            }
        )*
    };
}

impl_measure_for_int! { i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize }
