use self::super::libc_sys::{dlclose, dlopen, dlsym, RTLD_LAZY, RTLD_LOCAL};
use std::{ffi::CString, ptr::NonNull};

/// A module loader for loading shared libraries.
pub struct ModuleLoader(::std::ptr::NonNull<::std::os::raw::c_void>);

impl ModuleLoader {
    /// Load a shared library by its path.
    pub fn load(path: &str) -> Result<Self, ()> {
        let path = CString::new(path).unwrap();

        // Open the shared library with lazy loading and local visibility.
        let module = unsafe { dlopen(path.as_ptr(), RTLD_LAZY | RTLD_LOCAL) };
        if module.is_null() {
            Err(())
        } else {
            Ok(ModuleLoader(unsafe { NonNull::new_unchecked(module) }))
        }
    }

    /// Get a symbol from the loaded module.
    pub fn get_symbol<F: Sized>(&self, name: &str) -> Result<F, ()> {
        let name = CString::new(name).unwrap();

        let symbol = unsafe { dlsym(self.0.as_ptr(), name.as_ptr()) };

        if symbol.is_null() {
            return Err(());
        }

        // Transmute the symbol to the desired type.
        Ok(unsafe { std::mem::transmute_copy::<_, F>(&symbol) })
    }
}

impl Drop for ModuleLoader {
    fn drop(&mut self) {
        // When the module loader is dropped, close the shared library.
        unsafe { dlclose(self.0.as_ptr()) };
    }
}
