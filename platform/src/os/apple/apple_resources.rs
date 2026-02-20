use {
    crate::{
        cx::Cx,
        os::{apple::apple_sys::*, apple::apple_util::nsstring_to_string},
    },
    std::{fs::File, io::prelude::*, rc::Rc},
};

impl Cx {
    /// Loads resources as dependencies from the NSBundle's resource path.
    ///
    /// This is used for any Apple app bundle on iOS, macOS, or tvOS.
    #[allow(unused)]
    pub(crate) fn apple_bundle_load_dependencies(&mut self) {
        let bundle_path = unsafe {
            let main: ObjcId = msg_send![class!(NSBundle), mainBundle];
            let path: ObjcId = msg_send![main, resourcePath];
            nsstring_to_string(path)
        };

        for (path, dep) in &mut self.dependencies {
            if let Ok(mut file_handle) = File::open(format!("{}/{}", bundle_path, path)) {
                let mut buffer = Vec::<u8>::new();
                if file_handle.read_to_end(&mut buffer).is_ok() {
                    dep.data = Some(Ok(Rc::new(buffer)));
                } else {
                    dep.data = Some(Err("read_to_end failed".to_string()));
                }
            } else {
                dep.data = Some(Err("Bundled file open failed".to_string()));
            }
        }
    }

    /// Load a single file from the app bundle by relative path.
    ///
    /// Used by the script resource system to load fonts and other assets
    /// on iOS/tvOS devices where filesystem paths from the build machine
    /// are not available.
    #[allow(unused)]
    pub(crate) fn apple_bundle_load_file(&self, path: &str) -> Result<Rc<Vec<u8>>, String> {
        let bundle_path = unsafe {
            let main: ObjcId = msg_send![class!(NSBundle), mainBundle];
            let path: ObjcId = msg_send![main, resourcePath];
            nsstring_to_string(path)
        };

        let full_path = format!("{}/{}", bundle_path, path);
        let mut file = File::open(&full_path)
            .map_err(|e| format!("Bundle file open failed: {} ({})", full_path, e))?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|e| format!("Bundle file read failed: {} ({})", full_path, e))?;
        Ok(Rc::new(buffer))
    }
}
