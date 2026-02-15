#[macro_export]
macro_rules! implement_com {
    {
        for_struct: $for_struct:ident,
        identity: $identity:ident,
        wrapper_struct: $wrapper_struct:ident,
        interface_count: $interface_count:tt,
        interfaces: {
            $($iface_index:tt: $iface:ident),* $(,)?
        }
    } => {
        #[repr(C)]
        #[allow(non_camel_case_types)]
        pub(crate) struct $wrapper_struct {
            identity: &'static crate::windows::core::IInspectable_Vtbl,
            vtables: ($(&'static <$iface as crate::windows::core::Interface>::Vtable,)* ()),
            this: $for_struct,
            count: crate::windows::core::imp::WeakRefCount,
        }

        impl $wrapper_struct {
            const VTABLE_IDENTITY: crate::windows::core::IInspectable_Vtbl =
                crate::windows::core::IInspectable_Vtbl::new::<$wrapper_struct, $identity, 0>();

            const VTABLES: ($(<$iface as crate::windows::core::Interface>::Vtable,)* ()) = (
                $(<$iface as crate::windows::core::Interface>::Vtable::new::<$wrapper_struct, {-1 - $iface_index}>(),)*
                ()
            );
        }

        impl $for_struct {
            #[inline(always)]
            fn into_outer(self) -> $wrapper_struct {
                $wrapper_struct {
                    identity: &$wrapper_struct::VTABLE_IDENTITY,
                    vtables: ($(&$wrapper_struct::VTABLES.$iface_index,)* ()),
                    this: self,
                    count: crate::windows::core::imp::WeakRefCount::new(),
                }
            }
        }

        impl ::core::ops::Deref for $wrapper_struct {
            type Target = $for_struct;

            #[inline(always)]
            fn deref(&self) -> &Self::Target {
                &self.this
            }
        }

        impl crate::windows::core::IUnknownImpl for $wrapper_struct {
            type Impl = $for_struct;

            #[inline(always)]
            fn get_impl(&self) -> &Self::Impl {
                &self.this
            }

            #[inline(always)]
            fn get_impl_mut(&mut self) -> &mut Self::Impl {
                &mut self.this
            }

            #[inline(always)]
            fn into_inner(self) -> Self::Impl {
                self.this
            }

            unsafe fn QueryInterface(
                &self,
                iid: *const crate::windows::core::GUID,
                interface: *mut *mut ::core::ffi::c_void,
            ) -> crate::windows::core::HRESULT {
                unsafe {
                    if iid.is_null() || interface.is_null() {
                        return crate::windows::core::imp::E_POINTER;
                    }

                    let iid = *iid;

                    let interface_ptr: *const ::core::ffi::c_void = 'found: {
                        if iid == <crate::windows::core::IUnknown as crate::windows::core::Interface>::IID
                            || iid == <crate::windows::core::IInspectable as crate::windows::core::Interface>::IID
                            || iid == <crate::windows::core::imp::IAgileObject as crate::windows::core::Interface>::IID
                        {
                            break 'found &self.identity as *const _ as *const ::core::ffi::c_void;
                        }

                        $(
                            if <<$iface as crate::windows::core::Interface>::Vtable>::matches(&iid) {
                                break 'found &self.vtables.$iface_index as *const _ as *const ::core::ffi::c_void;
                            }
                        )*

                        #[cfg(windows)]
                        if iid == <crate::windows::core::imp::IMarshal as crate::windows::core::Interface>::IID {
                            return crate::windows::core::imp::marshaler(self.to_interface(), interface);
                        }

                        let tear_off_ptr = self.count.query(&iid, &self.identity as *const _ as *mut _);
                        if !tear_off_ptr.is_null() {
                            *interface = tear_off_ptr;
                            return crate::windows::core::HRESULT(0);
                        }

                        *interface = ::core::ptr::null_mut();
                        return crate::windows::core::imp::E_NOINTERFACE;
                    };

                    *interface = interface_ptr as *mut ::core::ffi::c_void;
                    self.count.add_ref();
                    crate::windows::core::HRESULT(0)
                }
            }

            #[inline(always)]
            fn AddRef(&self) -> u32 {
                self.count.add_ref()
            }

            #[inline(always)]
            unsafe fn Release(self_: *mut Self) -> u32 {
                let remaining = (*self_).count.release();
                if remaining == 0 {
                    _ = crate::windows::core::imp::Box::from_raw(self_);
                }
                remaining
            }

            #[inline(always)]
            fn is_reference_count_one(&self) -> bool {
                self.count.is_one()
            }

            unsafe fn GetTrustLevel(&self, value: *mut i32) -> crate::windows::core::HRESULT {
                if value.is_null() {
                    return crate::windows::core::imp::E_POINTER;
                }
                *value = 0;
                crate::windows::core::HRESULT(0)
            }

            fn to_object(&self) -> crate::windows::core::ComObject<Self::Impl> {
                self.count.add_ref();
                unsafe {
                    crate::windows::core::ComObject::from_raw(
                        ::core::ptr::NonNull::new_unchecked(self as *const Self as *mut Self),
                    )
                }
            }
        }

        impl crate::windows::core::ComObjectInner for $for_struct {
            type Outer = $wrapper_struct;

            fn into_object(self) -> crate::windows::core::ComObject<Self> {
                let boxed = crate::windows::core::imp::Box::<$wrapper_struct>::new(self.into_outer());
                unsafe {
                    let ptr = crate::windows::core::imp::Box::into_raw(boxed);
                    crate::windows::core::ComObject::from_raw(::core::ptr::NonNull::new_unchecked(ptr))
                }
            }
        }

        impl ::core::convert::From<$for_struct> for crate::windows::core::IUnknown {
            #[inline(always)]
            fn from(this: $for_struct) -> Self {
                let com_object = crate::windows::core::ComObject::new(this);
                com_object.into_interface()
            }
        }

        impl ::core::convert::From<$for_struct> for crate::windows::core::IInspectable {
            #[inline(always)]
            fn from(this: $for_struct) -> Self {
                let com_object = crate::windows::core::ComObject::new(this);
                com_object.into_interface()
            }
        }

        $(
            impl ::core::convert::From<$for_struct> for $iface {
                #[inline(always)]
                fn from(this: $for_struct) -> Self {
                    let com_object = crate::windows::core::ComObject::new(this);
                    com_object.into_interface()
                }
            }
        )*

        impl crate::windows::core::ComObjectInterface<crate::windows::core::IUnknown> for $wrapper_struct {
            #[inline(always)]
            fn as_interface_ref(&self) -> crate::windows::core::InterfaceRef<'_, crate::windows::core::IUnknown> {
                unsafe { ::core::mem::transmute(&self.identity) }
            }
        }

        impl crate::windows::core::ComObjectInterface<crate::windows::core::IInspectable> for $wrapper_struct {
            #[inline(always)]
            fn as_interface_ref(&self) -> crate::windows::core::InterfaceRef<'_, crate::windows::core::IInspectable> {
                unsafe { ::core::mem::transmute(&self.identity) }
            }
        }

        $(
            impl crate::windows::core::ComObjectInterface<$iface> for $wrapper_struct {
                #[inline(always)]
                fn as_interface_ref(&self) -> crate::windows::core::InterfaceRef<'_, $iface> {
                    unsafe { ::core::mem::transmute(&self.vtables.$iface_index) }
                }
            }
        )*

        $(
            impl crate::windows::core::AsImpl<$for_struct> for $iface {
                #[inline(always)]
                unsafe fn as_impl_ptr(&self) -> ::core::ptr::NonNull<$for_struct> {
                    let this = crate::windows::core::Interface::as_raw(self);
                    let this = (this as *mut *mut ::core::ffi::c_void).sub(1 + $iface_index) as *mut $wrapper_struct;
                    ::core::ptr::NonNull::new_unchecked(::core::ptr::addr_of!((*this).this) as *const $for_struct as *mut $for_struct)
                }
            }
        )*
    };
}
