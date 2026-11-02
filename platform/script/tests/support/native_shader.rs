//! Native compiler validation without a GPU dispatch or external toolchain.
#[cfg(target_os = "macos")]
pub fn metal(source: &str, precise: bool) -> Result<(), String> {
    use std::ffi::{c_char, c_void, CStr};
    type Id = *mut c_void;
    #[link(name = "Metal", kind = "framework")]
    extern "C" { fn MTLCreateSystemDefaultDevice() -> Id; }
    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Id;
        fn objc_msgSend();
    }
    unsafe {
        let send: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(objc_msgSend as *const ());
        let string_init: unsafe extern "C" fn(Id, Id, *const u8, usize, usize) -> Id = std::mem::transmute(objc_msgSend as *const ());
        let set_bool: unsafe extern "C" fn(Id, Id, bool) = std::mem::transmute(objc_msgSend as *const ());
        let compile: unsafe extern "C" fn(Id, Id, Id, Id, *mut Id) -> Id = std::mem::transmute(objc_msgSend as *const ());
        let sel = |s: &'static [u8]| sel_registerName(s.as_ptr().cast());
        let pool = send(objc_getClass(b"NSAutoreleasePool\0".as_ptr().cast()), sel(b"new\0"));
        let device = MTLCreateSystemDefaultDevice();
        assert!(!device.is_null(), "Metal device unavailable; native validation did not run");
        let string = send(objc_getClass(b"NSString\0".as_ptr().cast()), sel(b"alloc\0"));
        let string = string_init(string, sel(b"initWithBytes:length:encoding:\0"), source.as_ptr(), source.len(), 4usize);
        let options = send(objc_getClass(b"MTLCompileOptions\0".as_ptr().cast()), sel(b"new\0"));
        set_bool(options, sel(b"setFastMathEnabled:\0"), !precise);
        let mut error: Id = std::ptr::null_mut();
        let lib = compile(device, sel(b"newLibraryWithSource:options:error:\0"), string, options, &mut error as *mut Id);
        let result = if lib.is_null() {
            let desc = send(error, sel(b"localizedDescription\0"));
            let bytes = send(desc, sel(b"UTF8String\0"));
            Err(if bytes.is_null() { "Metal compiler failed without a diagnostic".into() }
                else { CStr::from_ptr(bytes.cast()).to_string_lossy().into_owned() })
        } else { Ok(()) };
        for obj in [lib, options, string, device] { send(obj, sel(b"release\0")); }
        send(pool, sel(b"drain\0"));
        result
    }
}

#[cfg(target_os = "windows")]
pub fn hlsl(source: &str, entry: &str, target: &str, precise: bool) -> Result<(), String> {
    use std::ffi::{c_char, c_void, CString};
    #[repr(C)]
    struct Blob { vtable: *const Vtable }
    #[repr(C)]
    struct Vtable {
        query: usize,
        add_ref: usize,
        release: unsafe extern "system" fn(*mut Blob) -> u32,
        data: unsafe extern "system" fn(*mut Blob) -> *const u8,
        size: unsafe extern "system" fn(*mut Blob) -> usize,
    }
    #[link(name = "d3dcompiler")]
    extern "system" {
        fn D3DCompile(src: *const u8, size: usize, name: *const c_char, defines: *const c_void,
            include: *const c_void, entry: *const c_char, target: *const c_char, flags: u32,
            flags2: u32, code: *mut *mut Blob, errors: *mut *mut Blob) -> i32;
    }
    let entry = CString::new(entry).unwrap();
    let target = CString::new(target).unwrap();
    unsafe {
        let mut code = std::ptr::null_mut();
        let mut errors = std::ptr::null_mut();
        let hr = D3DCompile(source.as_ptr(), source.len(), std::ptr::null(), std::ptr::null(), std::ptr::null(),
            entry.as_ptr(), target.as_ptr(), (1 << 11) | if precise { 1 << 13 } else { 0 }, 0, &mut code, &mut errors);
        let diagnostic = if errors.is_null() { format!("D3DCompile HRESULT {hr:#x}") } else {
            let vt = &*(*errors).vtable;
            String::from_utf8_lossy(std::slice::from_raw_parts((vt.data)(errors), (vt.size)(errors))).into_owned()
        };
        for blob in [code, errors] { if !blob.is_null() { ((*(*blob).vtable).release)(blob); } }
        if hr < 0 { Err(diagnostic) } else { Ok(()) }
    }
}
