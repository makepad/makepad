//! Vulkan bindings for Makepad's Linux and Android backends.
//!
//! Regenerate `vk.rs` and `commands.rs` with `tools/vulkan_codegen` and an
//! installed Khronos `vk.xml`. `tools/vulkan_codegen/apis.txt` is the API root
//! list. The generated types mirror the C ABI; pointer setters borrow no data,
//! so callers must retain their pointed-to values until the unsafe Vulkan call
//! finishes. Extension chains use concrete methods, not generic marker traits.
//!
//! Tables are loaded once per entry/instance/device. Clones share both the table
//! and loader lease. Vulkan objects themselves are destroyed by their owner.
//!
//! From the repository root, after building the generator in release mode:
//! `tools/vulkan_codegen/target/release/makepad-vulkan-codegen
//! /usr/share/vulkan/registry/vk.xml tools/vulkan_codegen/apis.txt libs/vulkan/src`.
//! The checked-in output uses registry 1.4.357. API roots include the Linux,
//! direct-display and Android/OpenXR backends, not the entire Vulkan registry.
pub mod vk;
pub mod commands;
use core::ffi::{c_char, c_int, c_void, CStr};
use std::sync::Arc;

#[link(name = "dl")]
extern "C" {
    fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}
struct Library(*mut c_void);
// The loader's handle is immutable and its symbol lookup is thread-safe. It is never closed:
// the loader and the ICDs it pulls in register process-exit and thread-exit handlers, and
// instance creation on another thread may be inside the library when the last Entry of this
// thread drops. A dlclose then unmaps code still running (crashes in vkCreateInstance and at
// exit). Every Entry therefore shares the one handle for the life of the process.
unsafe impl Send for Library {}
unsafe impl Sync for Library {}
#[derive(Clone)]
pub struct Entry {
    library: Arc<Library>,
    pub table: Arc<commands::EntryTable>,
}
#[derive(Clone)]
pub struct Instance {
    library: Arc<Library>,
    handle: vk::Instance,
    pub table: Arc<commands::InstanceTable>,
}
#[derive(Clone)]
pub struct Device {
    _library: Arc<Library>,
    handle: vk::Device,
    pub table: Arc<commands::DeviceTable>,
}
impl Entry {
    /// Load the system Vulkan loader without requiring SDK link-time paths.
    ///
    /// # Safety
    /// The system loader and ICDs must implement the Vulkan C ABI.
    pub unsafe fn load() -> Result<Self, String> {
        #[cfg(target_os = "android")]
        let name = c"libvulkan.so";
        #[cfg(not(target_os = "android"))]
        let name = c"libvulkan.so.1";
        static LIBRARY: std::sync::Mutex<Option<Arc<Library>>> = std::sync::Mutex::new(None);
        let library = {
            let mut shared = LIBRARY.lock().unwrap_or_else(|e| e.into_inner());
            match shared.as_ref() {
                Some(library) => library.clone(),
                None => {
                    let raw = dlopen(name.as_ptr(), 1); // RTLD_LAZY | RTLD_LOCAL
                    if raw.is_null() {
                        let error = dlerror();
                        return Err(if error.is_null() { "could not load Vulkan".into() }
                            else { CStr::from_ptr(error).to_string_lossy().into_owned() });
                    }
                    let library = Arc::new(Library(raw));
                    *shared = Some(library.clone());
                    library
                }
            }
        };
        let raw = library.0;
        let symbol = dlsym(raw, c"vkGetInstanceProcAddr".as_ptr());
        if symbol.is_null() { return Err("Vulkan loader lacks vkGetInstanceProcAddr".into()); }
        let get: commands::PfnGetInstanceProcAddr = core::mem::transmute(symbol);
        let mut table = commands::EntryTable::load(get);
        table.get_instance_proc_addr = Some(get);
        Ok(Self { library, table: Arc::new(table) })
    }
}
impl Instance {
    /// Adopt an instance created by Vulkan or OpenXR; destruction stays explicit.
    ///
    /// # Safety
    /// `handle` must be a live instance created through this loader.
    pub unsafe fn from_handle(entry: &Entry, handle: vk::Instance) -> Self {
        let get = entry.table.get_instance_proc_addr.expect("Vulkan instance lookup");
        Self { library: entry.library.clone(), handle,
            table: Arc::new(commands::InstanceTable::load(handle, get)) }
    }
    pub fn handle(&self) -> vk::Instance { self.handle }
}
impl Device {
    /// Adopt a device created by Vulkan or OpenXR; destruction stays explicit.
    ///
    /// # Safety
    /// `handle` must be a live logical device belonging to `instance`.
    pub unsafe fn from_handle(instance: &Instance, handle: vk::Device) -> Self {
        let get = instance.table.get_device_proc_addr.expect("Vulkan device lookup");
        Self { _library: instance.library.clone(), handle,
            table: Arc::new(commands::DeviceTable::load(handle, get)) }
    }
    pub fn handle(&self) -> vk::Device { self.handle }
}
