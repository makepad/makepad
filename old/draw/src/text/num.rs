pub trait Zero {
    const ZERO: Self;
}

macro_rules! impl_zero {
    ($T:ty, $ZERO:expr) => {
        impl Zero for $T {
            const ZERO: Self = $ZERO;
        }
    };
}

impl_zero!(usize, 0);
impl_zero!(f32, 0.0);

pub trait One {
    const ONE: Self;
}

macro_rules! impl_one {
    ($T:ty, $ONE:expr) => {
        impl One for $T {
            const ONE: Self = $ONE;
        }
    };
}

impl_one!(usize, 1);
impl_one!(f32, 1.0);
