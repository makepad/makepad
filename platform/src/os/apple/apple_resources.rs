use {
    crate::{
        cx::Cx,
        os::{apple::apple_sys::*, apple::apple_util::nsstring_to_string},
    },
    std::{fs::File, io::prelude::*, rc::Rc},
};

impl Cx {
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
