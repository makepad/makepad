
use crate::os::linux::openxr_sys::*;
use std::ptr;

#[derive(Default)]
pub struct CxAndroidOpenXr{
    openxr: Option<LibOpenXr>
}

impl CxAndroidOpenXr{
    pub fn init(&mut self, activity_handle:u64){
        crate::log!("Initialising OpenXR!!");
        self.openxr = LibOpenXr::try_load();
        
        let loader_info = XrLoaderInitInfoAndroidKHR {
            ty: XrLoaderInitInfoAndroidKHR::TYPE,
            next: ptr::null(),
            application_vm: makepad_android_state::get_java_vm() as *mut _,
            application_context: activity_handle as *mut _,
        };
        // lets load em up!
        if let Some(openxr) = &self.openxr{
            unsafe{
                crate::log!("xrInitializeLoaderKHR");
                let result = (openxr.xrInitializeLoaderKHR)(&loader_info as *const _ as _,);
                crate::log!("xrInitializeLoaderKHR: {:?}", result);
                 
                let default = XrExtensionProperties{
                    ty: XrStructureType::EXTENSION_PROPERTIES,
                    next: 0 as *mut _,
                    extension_name: [0;MAX_EXTENSION_NAME_SIZE],
                    extension_version: 0
                };
                let (_result, exts) = xr_array_fetch(0, default, |cap, len, buf|{
                    (openxr.xrEnumerateInstanceExtensionProperties)(
                        ptr::null(),
                        cap,
                        len, 
                        buf
                    );
                });
                for ext in exts{
                    crate::log!("ext: {}", std::str::from_utf8(&ext.extension_name).unwrap());
                }
                //let result = (openxr.xrInitializeLoaderKHR)(&loader_info as *const _ as _,);
            }
        }
        else{
            crate::log!("OPEN XR DIDNT LOAD");
        }
    }
}