// Function-pointer Debug prints the address, independently of ABI, safety,
// arity, and the traits implemented by its argument and return types.
macro_rules! check_callback {
    ($function:ident, $ty:ty) => {{
        #[derive(Clone, Copy, Debug)]
        struct Callback {
            function: Option<$ty>,
        }

        let pointer: $ty = $function;
        let address = format!("{:p}", pointer as *const ());
        assert_eq!(format!("{:?}", pointer), address);
        assert_eq!(format!("{:p}", pointer), address);
        assert_eq!(format!("{:?}", &pointer as &dyn std::fmt::Debug), address);

        let value = Some(pointer);
        let expected = format!("Some({})", address);
        assert_eq!(format!("{:?}", value), expected);
        assert_eq!(format!("{:?}", &value as &dyn std::fmt::Debug), expected);
        let callback = Callback { function: value };
        let cloned = callback.clone();
        assert_eq!(format!("{:?}", cloned), format!("Callback {{ function: {} }}", expected));
        assert_eq!(format!("{:?}", &callback as &dyn std::fmt::Debug), format!("{:?}", cloned));
        assert!(callback.function.is_some());

        let empty: Option<$ty> = None;
        assert_eq!(format!("{:?}", empty), "None");
        assert_eq!(format!("{:?}", &empty as &dyn std::fmt::Debug), "None");
        assert_eq!(format!("{:?}", Callback { function: empty }), "Callback { function: None }");
    }};
}

macro_rules! callback_suite {
    ($module:ident, [$($qualifier:tt)*]) => {
        mod $module {
            use std::ffi::c_void;

            // Deliberately do not implement Debug/Clone/Copy for these types.
            #[repr(C)]
            enum Scope {
                Object,
            }

            #[repr(C)]
            struct Opaque {
                _value: u8,
            }

            $($qualifier)* fn f0() -> u32 {
                let _ = Scope::Object;
                0
            }
            $($qualifier)* fn f1(_data: *mut c_void) -> u32 { 0 }
            $($qualifier)* fn f2(_data: *const Opaque, _out: *mut *const Opaque) {}
            $($qualifier)* fn f3(data: *mut c_void, _size: usize, _scope: Scope) -> *mut c_void {
                data
            }
            $($qualifier)* fn f4(data: *mut c_void, _size: usize, _align: usize, _scope: Scope) -> *mut c_void {
                data
            }
            $($qualifier)* fn f5(_data: *mut c_void, original: *mut c_void, _size: usize, _align: usize, _scope: Scope) -> *mut c_void {
                original
            }
            $($qualifier)* fn f6(_severity: u32, _kind: u32, _info: *const Opaque, _data: *mut c_void, _size: usize, _out: *mut *const Opaque) -> u32 {
                0
            }
            $($qualifier)* fn f12(_a: u8, _b: u8, _c: u8, _d: u8, _e: u8, _f: u8, _g: u8, _h: u8, _i: u8, _j: u8, _k: u8, _l: *mut c_void) {}

            #[test]
            fn debug_callbacks() {
                check_callback!(f0, $($qualifier)* fn() -> u32);
                check_callback!(f1, $($qualifier)* fn(*mut c_void) -> u32);
                check_callback!(f2, $($qualifier)* fn(*const Opaque, *mut *const Opaque));
                check_callback!(f3, $($qualifier)* fn(*mut c_void, usize, Scope) -> *mut c_void);
                check_callback!(f4, $($qualifier)* fn(*mut c_void, usize, usize, Scope) -> *mut c_void);
                check_callback!(f5, $($qualifier)* fn(*mut c_void, *mut c_void, usize, usize, Scope) -> *mut c_void);
                check_callback!(f6, $($qualifier)* fn(u32, u32, *const Opaque, *mut c_void, usize, *mut *const Opaque) -> u32);
                check_callback!(f12, $($qualifier)* fn(u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, u8, *mut c_void));
            }
        }
    };
}

callback_suite!(rust_safe, []);
callback_suite!(rust_unsafe, [unsafe]);
callback_suite!(c_safe, [extern "C"]);
callback_suite!(c_unsafe, [unsafe extern "C"]);
callback_suite!(system_safe, [extern "system"]);
callback_suite!(system_unsafe, [unsafe extern "system"]);
callback_suite!(c_unwind_safe, [extern "C-unwind"]);
callback_suite!(c_unwind_unsafe, [unsafe extern "C-unwind"]);
callback_suite!(system_unwind_safe, [extern "system-unwind"]);
callback_suite!(system_unwind_unsafe, [unsafe extern "system-unwind"]);
