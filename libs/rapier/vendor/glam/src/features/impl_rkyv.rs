#[cfg(feature = "bytecheck")]
macro_rules! impl_rkyv {
    (@bytecheck $type:ty) => {
        // SAFETY: All bit patterns are valid for these primitive types.
        // https://docs.rs/bytecheck/0.8.1/src/bytecheck/lib.rs.html#352
        unsafe impl<C: Fallible +?Sized> rkyv::bytecheck::CheckBytes<C> for $type {
            #[inline]
            unsafe fn check_bytes(
                _value: *const Self,
                _: &mut C,
            ) -> Result<(), C::Error> {
                Ok(())
            }
        }
    };

    ($type:ty) => {
        impl_rkyv_derive!(@serialize $type);
        impl_rkyv_derive!(@archive_deserialize $type);
        impl_rkyv!(@bytecheck $type);
    };
}

#[cfg(not(feature = "bytecheck"))]
macro_rules! impl_rkyv {
    ($type:ty) => {
        impl_rkyv_derive!(@serialize $type);
        impl_rkyv_derive!(@archive_deserialize $type);
    };
}

macro_rules! impl_rkyv_derive {
    (@serialize $type:ty) => {
        impl<S: Fallible + ?Sized> Serialize<S> for $type {
            #[inline]
            fn serialize(&self, _: &mut S) -> Result<Self::Resolver, S::Error> {
                Ok(())
            }
        }
    };

    (@archive_deserialize $type:ty) => {
        // SAFETY: All glam types have a fully defined data layout.
        unsafe impl rkyv::traits::NoUndef for $type {}
        // SAFETY: All glam types have a stable, well-defined layout that is identical on all
        // targets.
        unsafe impl rkyv::Portable for $type {}
        impl Archive for $type {
            type Archived = $type;
            type Resolver = ();

            #[inline]
            fn resolve(&self, _: Self::Resolver, out: Place<Self::Archived>) {
                out.write(*self)
            }
        }

        impl<D: Fallible + ?Sized> Deserialize<$type, D> for $type {
            #[inline]
            fn deserialize(&self, _: &mut D) -> Result<$type, D::Error> {
                Ok(*self)
            }
        }
    };
}

mod f32 {
    use crate::{Affine2, Affine3A, Mat2, Mat3, Mat3A, Mat4, Quat, Vec2, Vec3, Vec3A, Vec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};
    impl_rkyv!(Affine2);
    impl_rkyv!(Affine3A);
    impl_rkyv!(Mat2);
    impl_rkyv!(Mat3);
    impl_rkyv!(Mat3A);
    impl_rkyv!(Mat4);
    impl_rkyv!(Quat);
    impl_rkyv!(Vec2);
    impl_rkyv!(Vec3);
    impl_rkyv!(Vec3A);
    impl_rkyv!(Vec4);
}

mod f64 {
    use crate::{DAffine2, DAffine3, DMat2, DMat3, DMat4, DQuat, DVec2, DVec3, DVec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};

    impl_rkyv!(DAffine2);
    impl_rkyv!(DAffine3);
    impl_rkyv!(DMat2);
    impl_rkyv!(DMat3);
    impl_rkyv!(DMat4);
    impl_rkyv!(DQuat);
    impl_rkyv!(DVec2);
    impl_rkyv!(DVec3);
    impl_rkyv!(DVec4);
}

mod i8 {
    use crate::{I8Vec2, I8Vec3, I8Vec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};

    impl_rkyv!(I8Vec2);
    impl_rkyv!(I8Vec3);
    impl_rkyv!(I8Vec4);
}

mod i16 {
    use crate::{I16Vec2, I16Vec3, I16Vec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};

    impl_rkyv!(I16Vec2);
    impl_rkyv!(I16Vec3);
    impl_rkyv!(I16Vec4);
}

mod i32 {
    use crate::{IVec2, IVec3, IVec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};

    impl_rkyv!(IVec2);
    impl_rkyv!(IVec3);
    impl_rkyv!(IVec4);
}

mod i64 {
    use crate::{I64Vec2, I64Vec3, I64Vec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};

    impl_rkyv!(I64Vec2);
    impl_rkyv!(I64Vec3);
    impl_rkyv!(I64Vec4);
}

mod u8 {
    use crate::{U8Vec2, U8Vec3, U8Vec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};

    impl_rkyv!(U8Vec2);
    impl_rkyv!(U8Vec3);
    impl_rkyv!(U8Vec4);
}

mod u16 {
    use crate::{U16Vec2, U16Vec3, U16Vec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};

    impl_rkyv!(U16Vec2);
    impl_rkyv!(U16Vec3);
    impl_rkyv!(U16Vec4);
}

mod u32 {
    use crate::{UVec2, UVec3, UVec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};

    impl_rkyv!(UVec2);
    impl_rkyv!(UVec3);
    impl_rkyv!(UVec4);
}

mod u64 {
    use crate::{U64Vec2, U64Vec3, U64Vec4};
    use rkyv::{rancor::Fallible, Archive, Deserialize, Place, Serialize};

    impl_rkyv!(U64Vec2);
    impl_rkyv!(U64Vec3);
    impl_rkyv!(U64Vec4);
}
