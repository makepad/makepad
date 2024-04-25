use crate::{
    error::Error,
    extern_ref::{ExternRef, UnguardedExternRef},
    func::{FuncType, HostFuncTrampoline},
    func_ref::{FuncRef, UnguardedFuncRef},
    stack::{Stack, StackGuard, StackSlot},
    store::{Store, StoreId},
    val::ValType,
};

macro_rules! for_each_tuple {
    ($macro:ident) => {
        $macro!(0);
        $macro!(T0, 1);
        $macro!(T0, T1, 2);
        $macro!(T0, T1, T2, 3);
        $macro!(T0, T1, T2, T3, 4);
        $macro!(T0, T1, T2, T3, T4, 5);
        $macro!(T0, T1, T2, T3, T4, T5, 6);
        $macro!(T0, T1, T2, T3, T4, T5, T6, 7);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, 8);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, T8, 9);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, 10);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, 11);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, 12);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, 13);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, 14);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, 15);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, 16);
        $macro!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15, T16, 17);
    };
}

pub trait Wrap<T, U> {
    type Params: HostValList;
    type Results: HostValList;

    fn wrap(self) -> (FuncType, HostFuncTrampoline);
}

macro_rules! impl_wrap {
    ($($Ti:ident,)* $N:literal) => {
        impl<F, $($Ti,)* U> Wrap<($($Ti,)*), U> for F
        where
            F: Fn($($Ti,)*) -> U + Send + Sync + 'static,
            $($Ti: HostVal,)*
            U: HostResult
        {
            type Params = ($($Ti,)*);
            type Results = <U as HostResult>::Results;

            #[allow(non_snake_case)]
            fn wrap(self) -> (FuncType, HostFuncTrampoline) {
                Wrap::wrap(move |_store: &mut Store, $($Ti,)*| self($($Ti,)*))
            }
        }

        impl<F, $($Ti,)* U> Wrap<(&mut Store, $($Ti,)*), U> for F
        where
            F: Fn(&mut Store, $($Ti,)*) -> U + Send + Sync + 'static,
            $($Ti: HostVal,)*
            U: HostResult
        {
            type Params = ($($Ti,)*);
            type Results = <U as HostResult>::Results;

            #[allow(non_snake_case)]
            fn wrap(self) -> (FuncType, HostFuncTrampoline) {
                let type_ = FuncType::new(
                    Self::Params::types(),
                    Self::Results::types(),
                );
                let callee_stack_slot_count = type_.callee_stack_slot_count();
                (
                    type_,
                    HostFuncTrampoline::new(move |store, mut stack| -> Result<StackGuard, Error> {
                        let ($($Ti,)*) = unsafe {
                            let mut ptr = stack.ptr().offset(-(callee_stack_slot_count as isize));
                            Self::Params::read_from_stack(&mut ptr, store.id())
                        };
                        drop(stack);
                        let results = self(store, $($Ti,)*).into_result()?;
                        let mut stack = Stack::lock();
                        unsafe {
                            let mut ptr = stack.ptr().offset(-(callee_stack_slot_count as isize));
                            results.write_to_stack(&mut ptr, store.id())
                        }
                        Ok(stack)
                    })
                )
            }
        }
    }
}

for_each_tuple!(impl_wrap);

pub trait HostResult {
    type Results: HostValList;

    fn into_result(self) -> Result<Self::Results, Error>;
}

impl<T> HostResult for T
where
    T: HostVal,
{
    type Results = T;

    fn into_result(self) -> Result<Self::Results, Error> {
        Ok(self)
    }
}

macro_rules! impl_host_result {
    ($($Ti:ident,)* $N:literal) => {
        impl<$($Ti),*> HostResult for ($($Ti,)*)
        where
            $($Ti: HostVal),*
        {
            type Results = ($($Ti,)*);

            fn into_result(self) -> Result<Self::Results, Error> {
                Ok(self)
            }
        }

        impl<$($Ti),*> HostResult for Result<($($Ti,)*), Error>
        where
            $($Ti: HostVal),*
        {
            type Results = ($($Ti,)*);

            fn into_result(self) -> Result<Self::Results, Error> {
                self
            }
        }
    }
}

for_each_tuple!(impl_host_result);

pub trait HostValList {
    type Types: IntoIterator<Item = ValType>;

    fn types() -> Self::Types;

    unsafe fn read_from_stack(ptr: &mut *mut StackSlot, store_id: StoreId) -> Self;

    unsafe fn write_to_stack(self, ptr: &mut *mut StackSlot, store_id: StoreId);
}

impl<T> HostValList for T
where
    T: HostVal,
{
    type Types = [ValType; 1];

    fn types() -> Self::Types {
        [<T as HostVal>::type_()]
    }

    unsafe fn read_from_stack(ptr: &mut *mut StackSlot, store_id: StoreId) -> Self {
        T::read_from_stack(ptr, store_id)
    }

    unsafe fn write_to_stack(self, ptr: &mut *mut StackSlot, store_id: StoreId) {
        <Self as HostVal>::write_to_stack(self, ptr, store_id);
    }
}

macro_rules! impl_host_val_list {
    ($($Ti:ident,)* $N:literal) => {
        impl<$($Ti,)*> HostValList for ($($Ti,)*)
        where
            $($Ti: HostVal,)*
        {
            type Types = [ValType; $N];

            fn types() -> Self::Types {
                [$($Ti::type_()),*]
            }

            #[allow(unused_variables)]
            unsafe fn read_from_stack(ptr: &mut *mut StackSlot, store_id: StoreId) -> Self {
                ($($Ti::read_from_stack(ptr, store_id),)*)
            }

            #[allow(non_snake_case)]
            #[allow(unused_variables)]
            unsafe fn write_to_stack(self, ptr: &mut *mut StackSlot, store_id: StoreId) {
                let ($($Ti,)*) = self;
                $($Ti.write_to_stack(ptr, store_id);)*
            }
        }
    }
}

for_each_tuple!(impl_host_val_list);

pub trait HostVal {
    fn type_() -> ValType;

    unsafe fn read_from_stack(ptr: &mut *mut StackSlot, store_id: StoreId) -> Self;

    unsafe fn write_to_stack(self, ptr: &mut *mut StackSlot, store_id: StoreId);
}

macro_rules! impl_host_val {
    ($T:ty, $ValType:ident) => {
        impl HostVal for $T {
            fn type_() -> ValType {
                ValType::$ValType
            }

            unsafe fn read_from_stack(ptr: &mut *mut StackSlot, _store_id: StoreId) -> Self {
                let val = *ptr.cast::<$T>();
                *ptr = ptr.add(1);
                val
            }

            unsafe fn write_to_stack(self, ptr: &mut *mut StackSlot, _store_id: StoreId) {
                *ptr.cast::<$T>() = self;
                *ptr = ptr.add(1);
            }
        }
    };
}

macro_rules! impl_host_val_raw {
    ($T:ty, $RawT:ty, $ValType:ident) => {
        impl HostVal for $T {
            fn type_() -> ValType {
                ValType::$ValType
            }

            unsafe fn read_from_stack(ptr: &mut *mut StackSlot, store_id: StoreId) -> Self {
                let val = <$T>::from_unguarded(*ptr.cast::<$RawT>(), store_id);
                *ptr = ptr.add(1);
                val
            }

            unsafe fn write_to_stack(self, ptr: &mut *mut StackSlot, store_id: StoreId) {
                *ptr.cast::<$RawT>() = self.to_unguarded(store_id);
                *ptr = ptr.add(1);
            }
        }
    };
}

impl_host_val!(i32, I32);
impl_host_val!(i64, I64);
impl_host_val!(f32, F32);
impl_host_val!(f64, F64);
impl_host_val_raw!(FuncRef, UnguardedFuncRef, FuncRef);
impl_host_val_raw!(ExternRef, UnguardedExternRef, ExternRef);
