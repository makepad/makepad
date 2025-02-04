use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NonNan<T>(T);

impl<T> NonNan<T> {
    pub fn new(value: T) -> Option<Self>
    where
        T: Copy + PartialEq,
    {
        if value != value {
            return None;
        }
        Some(Self(value))
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Eq for NonNan<T> where T: PartialEq {}

impl<T> Hash for NonNan<T>
where
    T: Copy + ToBits,
{
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.0.to_bits().hash(hasher);
    }
}

trait ToBits {
    type Bits: Hash;

    fn to_bits(self) -> Self::Bits;
}

macro_rules! impl_to_bits {
    ($T:ty, $Bits:ty) => {
        impl ToBits for $T {
            type Bits = $Bits;
            fn to_bits(self) -> Self::Bits {
                self.to_bits()
            }
        }
    };
}

impl_to_bits!(f32, u32);
impl_to_bits!(f64, u64);
