// Runtime support for the generated bindings. No COM layout is declared here:
// QueryInterface/AddRef/Release dispatch through the metadata-generated vtable.
pub use crate::Win32::System::Com::{IUnknown, IUnknown_Vtbl};
use ::core::{ffi::c_void, ptr::null_mut};
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}
impl GUID {
    pub const fn from_u128(value: u128) -> Self {
        Self {
            data1: (value >> 96) as u32,
            data2: (value >> 80) as u16,
            data3: (value >> 64) as u16,
            data4: (value as u64).to_be_bytes(),
        }
    }
    pub const fn zeroed() -> Self {
        Self::from_u128(0)
    }
    pub fn equals(&self, other: &Self) -> bool {
        self.data1 == other.data1
            && self.data2 == other.data2
            && self.data3 == other.data3
            && self.data4 == other.data4
    }
}
impl ::core::fmt::Debug for GUID {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        let value = ((self.data1 as u128) << 96)
            | ((self.data2 as u128) << 80)
            | ((self.data3 as u128) << 64)
            | u64::from_be_bytes(self.data4) as u128;
        ::core::fmt::LowerHex::fmt(&value, f)
    }
}
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct HRESULT(pub i32);
pub type Error = HRESULT;
impl HRESULT {
    pub const fn is_ok(self) -> bool {
        self.0 >= 0
    }
    pub const fn is_err(self) -> bool {
        self.0 < 0
    }
    pub const fn code(self) -> Self {
        self
    }
    pub const fn empty() -> Self {
        Self(-2147467259)
    }
    pub fn ok(self) -> Result<(), HRESULT> {
        if self.is_ok() {
            Ok(())
        } else {
            Err(self)
        }
    }
    pub fn from_thread() -> Self {
        unsafe { Self::from_win32(crate::Win32::Foundation::GetLastError().0) }
    }
    pub const fn from_win32(code: u32) -> Self {
        if code as i32 <= 0 {
            Self(code as i32)
        } else {
            Self(((code & 0xffff) | 0x80070000) as i32)
        }
    }
    pub fn unwrap(self) {
        self.ok().expect("Windows API failed");
    }
}
impl ::core::fmt::Debug for HRESULT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.write_str("HRESULT(")?;
        ::core::fmt::LowerHex::fmt(&(self.0 as u32), f)?;
        f.write_str(")")
    }
}
impl ::core::fmt::Display for HRESULT {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        ::core::fmt::Debug::fmt(self, f)
    }
}
impl PartialEq for HRESULT {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for HRESULT {}
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct BOOL(pub i32);
impl BOOL {
    pub fn as_bool(self) -> bool {
        self.0 != 0
    }
    pub fn ok(self) -> Result<(), HRESULT> {
        if self.as_bool() {
            Ok(())
        } else {
            Err(HRESULT::from_thread())
        }
    }
    pub fn default() -> Self {
        Self(0)
    }
}
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PCWSTR(pub *const u16);
impl PCWSTR {
    pub const fn null() -> Self {
        Self(::core::ptr::null())
    }
    pub const fn from_raw(raw: *const u16) -> Self {
        Self(raw)
    }
    pub fn as_ptr(self) -> *const u16 {
        self.0
    }
    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
    pub unsafe fn to_string(self) -> std::result::Result<String, std::string::FromUtf16Error> {
        let mut n = 0;
        while !self.0.is_null() && *self.0.add(n) != 0 {
            n += 1;
        }
        String::from_utf16(if n == 0 {
            &[]
        } else {
            ::core::slice::from_raw_parts(self.0, n)
        })
    }
}
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PWSTR(pub *mut u16);
impl PWSTR {
    pub const fn null() -> Self {
        Self(null_mut())
    }
    pub const fn from_raw(raw: *mut u16) -> Self {
        Self(raw)
    }
    pub fn as_ptr(self) -> *mut u16 {
        self.0
    }
    pub fn is_null(self) -> bool {
        self.0.is_null()
    }
    pub unsafe fn to_string(self) -> std::result::Result<String, std::string::FromUtf16Error> {
        PCWSTR(self.0).to_string()
    }
}
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PCSTR(pub *const u8);
impl PCSTR {
    pub const fn null() -> Self {
        Self(::core::ptr::null())
    }
    pub const fn from_raw(raw: *const u8) -> Self {
        Self(raw)
    }
    pub fn as_ptr(self) -> *const u8 {
        self.0
    }
    pub unsafe fn to_string(self) -> std::result::Result<String, std::str::Utf8Error> {
        let s = std::ffi::CStr::from_ptr(self.0 as *const i8).to_str()?;
        Ok(s.to_string())
    }
}
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PSTR(pub *mut u8);
#[repr(transparent)]
pub struct BSTR(pub *mut u16);

pub unsafe fn query(raw: *mut c_void, iid: &GUID) -> Result<*mut c_void, HRESULT> {
    if raw.is_null() {
        return Err(HRESULT(-2147467261));
    }
    let v = &**(raw as *const *const IUnknown_Vtbl);
    let mut out = null_mut();
    (v.QueryInterface)(raw, iid, &mut out).ok()?;
    if out.is_null() {
        Err(HRESULT(-2147467261))
    } else {
        Ok(out)
    }
}
pub unsafe fn add_ref(raw: *mut c_void) -> u32 {
    let v = &**(raw as *const *const IUnknown_Vtbl);
    (v.AddRef)(raw)
}
pub unsafe fn release(raw: *mut c_void) -> u32 {
    let v = &**(raw as *const *const IUnknown_Vtbl);
    (v.Release)(raw)
}

// A slot is a COM interface pointer. Every slot shares the same owning header,
// so IUnknown identity and the reference count remain stable across interfaces.
#[repr(C)]
pub struct ObjectSlot {
    pub vtable: *const c_void,
    pub owner: *mut ObjectHeader,
    pub implementation: *mut c_void,
    pub destroy: unsafe fn(*mut c_void),
    pub iids: Vec<GUID>,
}
pub struct ObjectHeader {
    count: std::sync::atomic::AtomicU32,
    slots: Vec<Box<ObjectSlot>>,
}
impl Drop for ObjectHeader {
    fn drop(&mut self) {
        for slot in &self.slots {
            unsafe {
                (slot.destroy)(slot.implementation);
            }
        }
    }
}
pub struct ObjectBuilder {
    header: Box<ObjectHeader>,
}
impl ObjectBuilder {
    pub fn new() -> Self {
        Self {
            header: Box::new(ObjectHeader {
                count: std::sync::atomic::AtomicU32::new(1),
                slots: Vec::new(),
            }),
        }
    }
    pub unsafe fn add(
        &mut self,
        vtable: *const c_void,
        implementation: *mut c_void,
        destroy: unsafe fn(*mut c_void),
        iids: Vec<GUID>,
    ) {
        let owner = &mut *self.header as *mut ObjectHeader;
        self.header.slots.push(Box::new(ObjectSlot {
            vtable,
            owner,
            implementation,
            destroy,
            iids,
        }));
    }
    pub fn finish(self) -> Result<*mut c_void, HRESULT> {
        if self.header.slots.is_empty() {
            return Err(HRESULT(-2147418113));
        }
        let pointer = &*self.header.slots[0] as *const ObjectSlot as *mut c_void;
        let _ = Box::into_raw(self.header);
        Ok(pointer)
    }
}
pub unsafe extern "system" fn object_query(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return HRESULT(-2147467261);
    }
    *out = null_mut();
    if iid.is_null() {
        return HRESULT(-2147467261);
    }
    let owner = (*(this as *mut ObjectSlot)).owner;
    if (*iid).equals(&IUnknown::IID) {
        *out = &*(&(*owner).slots)[0] as *const ObjectSlot as *mut c_void;
    } else {
        for slot in &(*owner).slots {
            for supported in &slot.iids {
                if (*iid).equals(supported) {
                    *out = &**slot as *const ObjectSlot as *mut c_void;
                    break;
                }
            }
            if !(*out).is_null() {
                break;
            }
        }
    }
    if (*out).is_null() {
        return HRESULT(-2147467262);
    }
    object_add_ref(this);
    HRESULT(0)
}
pub unsafe extern "system" fn object_add_ref(this: *mut c_void) -> u32 {
    let owner = (*(this as *mut ObjectSlot)).owner;
    let previous = (*owner)
        .count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if previous == u32::MAX {
        std::process::abort();
    }
    previous + 1
}
pub unsafe extern "system" fn object_release(this: *mut c_void) -> u32 {
    let owner = (*(this as *mut ObjectSlot)).owner;
    let remaining = (*owner)
        .count
        .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
        - 1;
    if remaining == 0 {
        drop(Box::from_raw(owner));
    }
    remaining
}
impl PartialEq for BOOL {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl Eq for BOOL {}
impl PartialEq for GUID {
    fn eq(&self, other: &Self) -> bool {
        self.equals(other)
    }
}
impl Eq for GUID {}
impl GUID {
    pub const fn to_u128(self) -> u128 {
        ((self.data1 as u128) << 96)
            | ((self.data2 as u128) << 80)
            | ((self.data3 as u128) << 64)
            | (u64::from_be_bytes(self.data4) as u128)
    }
}
impl BSTR {
    pub fn from_str(text: &str) -> Result<Self, HRESULT> {
        let mut wide = Vec::new();
        for c in text.encode_utf16() {
            wide.push(c);
        }
        let raw = unsafe {
            crate::Win32::Foundation::SysAllocStringLen(
                Some(PCWSTR(wide.as_ptr())),
                wide.len().try_into().expect("BSTR UTF-16 length"),
            )
        };
        if raw.is_null() && !wide.is_empty() {
            Err(HRESULT(-2147024882))
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for BSTR {
    fn drop(&mut self) {
        unsafe {
            crate::Win32::Foundation::SysFreeString(Some(self.0));
        }
    }
}

// Owned immutable WinRT strings; a null HSTRING is the empty string.
#[repr(transparent)]
pub struct HSTRING(*mut c_void);
impl HSTRING {
    pub unsafe fn from_raw(raw: *mut c_void) -> Self {
        Self(raw)
    }
    pub fn as_raw(&self) -> *mut c_void {
        self.0
    }
    pub fn from_str(text: &str) -> Result<Self, HRESULT> {
        let mut wide = Vec::new();
        for c in text.encode_utf16() {
            wide.push(c);
        }
        let raw = unsafe {
            crate::Win32::System::WinRT::WindowsCreateString(
                Some(PCWSTR(wide.as_ptr())),
                wide.len().try_into().expect("HSTRING length"),
            )?
        };
        Ok(Self(raw.0))
    }
    pub fn to_string(&self) -> String {
        unsafe {
            let mut length = 0;
            let raw = crate::Win32::System::WinRT::WindowsGetStringRawBuffer(
                Some(crate::Win32::System::WinRT::HSTRING(self.0)),
                Some(&mut length),
            )
            .0;
            if length == 0 {
                String::new()
            } else {
                String::from_utf16_lossy(::core::slice::from_raw_parts(raw, length as usize))
            }
        }
    }
    pub fn try_clone(&self) -> Result<Self, HRESULT> {
        let raw = unsafe {
            crate::Win32::System::WinRT::WindowsDuplicateString(Some(
                crate::Win32::System::WinRT::HSTRING(self.0),
            ))?
        };
        Ok(Self(raw.0))
    }
}
impl Drop for HSTRING {
    fn drop(&mut self) {
        unsafe {
            let _ = crate::Win32::System::WinRT::WindowsDeleteString(Some(
                crate::Win32::System::WinRT::HSTRING(self.0),
            ));
        }
    }
}

// Keep the implicit MTA alive for process lifetime, as required by WinRT
// activation on ordinary Rust workers. STA callers retain their apartment.
fn ensure_mta() -> Result<(), HRESULT> {
    static STATUS: std::sync::OnceLock<HRESULT> = std::sync::OnceLock::new();
    STATUS
        .get_or_init(|| unsafe {
            match crate::Win32::System::Com::CoIncrementMTAUsage() {
                Ok(_) => HRESULT(0),
                Err(error) => error,
            }
        })
        .ok()
}

pub unsafe fn activation_factory(class: &str, iid: &GUID) -> Result<*mut c_void, HRESULT> {
    ensure_mta()?;
    let name = HSTRING::from_str(class)?;
    crate::Win32::System::WinRT::RoGetActivationFactory(
        crate::Win32::System::WinRT::HSTRING(name.as_raw()),
        iid,
    )
}
pub unsafe fn activate(class: &str) -> Result<crate::Win32::System::WinRT::IInspectable, HRESULT> {
    ensure_mta()?;
    let name = HSTRING::from_str(class)?;
    crate::Win32::System::WinRT::RoActivateInstance(crate::Win32::System::WinRT::HSTRING(
        name.as_raw(),
    ))
}
