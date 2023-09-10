
#[macro_export]
macro_rules!implement_com{
    {
        for_struct: $ for_struct: ident,
        identity: $ identity: ident,
        wrapper_struct: $ wrapper_struct: ident,
        interface_count: $ interface_count: tt,
        interfaces: {
            $ ( $ iface_index: tt: $ iface: ident), *
        }
    } => {
        
        #[repr(C)]
        struct $ wrapper_struct {
            identity: *const crate::windows::core::IInspectable_Vtbl,
            vtables:
            ( $ (*const < $ iface as crate::windows::core::Interface> ::Vtable), *, ()),
            this: $ for_struct,
            count: crate::windows::core::imp::WeakRefCount,
        }
        
        impl $ wrapper_struct {
            const VTABLES:
            ( $ (< $ iface as crate::windows::core::Interface> ::Vtable), *, ()) =
            ( $ (< $ iface as crate::windows::core::Interface> ::Vtable ::new ::<Self, $ for_struct, {-1 - $ iface_index}>()), *, ());
            
            const IDENTITY: crate::windows::core::IInspectable_Vtbl = crate::windows::core::IInspectable_Vtbl::new::<Self,$identity,0> ();
            
            fn new(this: $ for_struct) -> Self {
                Self {
                    identity: &Self::IDENTITY,
                    vtables: ( $ (&Self::VTABLES. $ iface_index), *, ()),
                    this,
                    count: crate::windows::core::imp::WeakRefCount ::new(),
                }
            }
        }
        
        impl crate::windows::core::IUnknownImpl for $ wrapper_struct {
            type Impl = $ for_struct;
            
            fn get_impl(&self) -> &Self ::Impl {
                &self.this
            }
            
            #[allow(non_snake_case)]
            unsafe fn QueryInterface(&self, iid: &crate::windows::core::GUID, interface: *mut *const ::core ::ffi ::c_void) -> crate::windows::core::HRESULT {
                *interface =
                if iid == &<crate::windows::core::IUnknown as crate::windows::core::ComInterface> ::IID ||
                  iid == &<crate::windows::core::IInspectable as crate::windows::core::ComInterface> ::IID ||
                  iid == &<crate::windows::core::imp::IAgileObject as crate::windows::core::ComInterface> ::IID{
                    &self.identity as *const _ as *const _
                }
                $ (else if < $ iface as crate::windows::core::Interface> ::Vtable::matches(iid) {
                    &self.vtables. $ iface_index as *const _ as *const _
                }) *
                else {
                    std::ptr::null_mut()
                };
                
                if!(*interface).is_null() {
                    self.count.add_ref();
                    return crate::windows::core::HRESULT(0);
                }
                
                *interface = self.count.query(iid, &self.identity as *const _ as *mut _);
                
                if (*interface).is_null() {
                    crate::windows::core::HRESULT(-2147467262i32)
                }
                else {
                    crate::windows::core::HRESULT(0)
                }
            }
            
            #[allow(non_snake_case)]
            fn AddRef(&self) -> u32 {self.count.add_ref()}
            
            #[allow(non_snake_case)]
            unsafe fn Release(&self) -> u32 {
                let remaining = self.count.release();
                if remaining == 0 {
                    let _ = Box ::from_raw(self as *const Self as *mut Self);
                } remaining
            }
        }
        /*
        impl $ for_struct {
            unsafe fn cast<I: crate::windows_crate::core::Interface> (&self) -> crate::windows_crate::core::Result<I> {
                let boxed = (self as *const _ as *const *mut std::ffi::c_void).sub(1 + $interface_count) as *mut $ wrapper_struct;
                let mut result = None;
                < $ wrapper_struct as crate::windows_crate::core::IUnknownImpl>::QueryInterface(&*boxed, &I::IID, &mut result as *mut _ as _).and_some(result)
            }
        }*/
        
        impl std::convert::From< $ for_struct> for crate::windows::core::IUnknown {
            fn from(this: $ for_struct) -> Self {
                let this = $ wrapper_struct::new(this);
                let boxed = std::mem::ManuallyDrop::new(Box::new(this));
                unsafe {std::mem ::transmute(&boxed.identity)}
            }
        }
        
        $ (
            impl std::convert::From< $ for_struct> for $ iface {
                fn from(this: $ for_struct) -> Self {
                    let this = $ wrapper_struct::new(this);
                    let this = std::mem::ManuallyDrop::new(Box::new(this));
                    let vtable_ptr = &this.vtables. $ iface_index;
                    unsafe {std::mem ::transmute(vtable_ptr)}
                }
            }
            
            impl crate::windows::core::AsImpl< $ for_struct> for $ iface {
                unsafe fn as_impl(&self) -> & $ for_struct {
                    let this = crate::windows::core::Interface::as_raw(self);
                    unsafe {
                        let this = (this as *mut *mut ::core ::ffi ::c_void).sub(1 + $ iface_index) as *mut $ wrapper_struct ::< >;
                        &(*this).this
                    }
                }
            }
        ) *
        
    }
}

