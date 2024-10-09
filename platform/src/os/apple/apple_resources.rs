use {
    std::{
        rc::Rc,
        io::prelude::*,
        fs::File,
    },
    crate::{
        os::{
            apple::apple_sys::*,
            apple::apple_util::nsstring_to_string,
        },
        cx::Cx,
    }
};

impl Cx {
    /// Loads resources as dependencies from the NSBundle's resource path.
    ///
    /// This is used for any Apple app bundle on iOS, macOS, or tvOS.
    pub(crate) fn apple_bundle_load_dependencies(&mut self) {
        let bundle_path = unsafe{
            let main:ObjcId = msg_send![class!(NSBundle), mainBundle];
            let path:ObjcId = msg_send![main, resourcePath];
            nsstring_to_string(path)
        };

        for (path,dep) in &mut self.dependencies{
            if let Ok(mut file_handle) = File::open(format!("{}/{}",bundle_path,path)) {
                let mut buffer = Vec::<u8>::new();
                if file_handle.read_to_end(&mut buffer).is_ok() {
                    dep.data = Some(Ok(Rc::new(buffer)));
                }
                else{
                    dep.data = Some(Err("read_to_end failed".to_string()));
                }
            }
            else{
                dep.data = Some(Err("File open failed".to_string()));
            }
        }
    }
}
