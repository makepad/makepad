#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use napi_ohos::sys::{napi_env, napi_value};
use std::io::{Error, ErrorKind, Result};

use super::oh_sys::*;

#[derive(Clone, Debug)]
pub struct RawFileMgr {
    native_resource_manager: *mut NativeResourceManager,
}

impl RawFileMgr {
    pub fn new(raw_env: napi_env, res_mgr: napi_value) -> RawFileMgr {
        let native_resource_manager =
            unsafe { OH_ResourceManager_InitNativeResourceManager(raw_env, res_mgr) };
        if native_resource_manager.is_null() {
            crate::log!("call OH_ResourceManager_InitNativeResourceManager failed");
        }
        Self {
            native_resource_manager,
        }
    }

    pub fn read_to_end<S: AsRef<str>>(&mut self, path: S, buf: &mut Vec<u8>) -> Result<usize> {
        crate::error!("OH_ResourceManager::read_to_end(): Here 0, path: {}", path.as_ref());
        if self.native_resource_manager.is_null() {
            crate::error!("OH_ResourceManager::read_to_end(): Here 1");
            return Err(Error::new(
                ErrorKind::NotConnected,
                "OH_ResourceManager_InitNativeResourceManager failed",
            ));
        }
        let path_cstring = std::ffi::CString::new(path.as_ref())?;
        let raw_file = unsafe {
            OH_ResourceManager_OpenRawFile(self.native_resource_manager, path_cstring.as_ptr())
        };
        crate::error!("OH_ResourceManager::read_to_end(): Here 2");
        if raw_file.is_null() {
            crate::error!("OH_ResourceManager::read_to_end(): Here 3");
            let msg = format!("open file {} failed", path.as_ref());
            return Err(Error::new(ErrorKind::NotConnected, msg));
        }
        let file_length = unsafe { OH_ResourceManager_GetRawFileSize(raw_file) };
        crate::error!("OH_ResourceManager::read_to_end(): Here 4");
        if file_length <= 0 {
            crate::error!("OH_ResourceManager::read_to_end(): Here 5");
            let _ = unsafe { OH_ResourceManager_CloseRawFile(raw_file) };
            buf.clear();
            return Ok(0);
        }
        buf.resize(file_length.try_into().unwrap(), 0 as u8);
        crate::error!("OH_ResourceManager::read_to_end(): Here 6. file_length: {file_length}");
        let read_length = unsafe {
            OH_ResourceManager_ReadRawFile(
                raw_file,
                buf.as_ptr() as *mut ::core::ffi::c_void,
                file_length.try_into().unwrap(),
            )
        };
        crate::error!("OH_ResourceManager::read_to_end(): Here 7. read_length: {read_length}");
        if i64::from(read_length) < file_length {
            crate::error!("OH_ResourceManager::read_to_end(): Here 8");
            buf.resize(read_length.try_into().unwrap(), 0 as u8);
        }
        let _ = unsafe { OH_ResourceManager_CloseRawFile(raw_file) };
        crate::error!("OH_ResourceManager::read_to_end(): Success, returning {}", read_length);
        return Ok(read_length.try_into().unwrap());
    }
}

impl Drop for RawFileMgr {
    fn drop(&mut self) {
        unsafe {
            OH_ResourceManager_ReleaseNativeResourceManager(self.native_resource_manager);
        }
    }
}
