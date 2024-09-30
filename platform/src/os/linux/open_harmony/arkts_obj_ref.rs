#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use super::oh_sys::*;
use super::oh_util;
use napi_ohos::sys::*;
use std::ffi::*;
use std::ptr::null_mut;
use std::sync::mpsc;

#[derive(Clone, Debug)]
pub enum ArkTsObjErr {
    NullError,
    InvalidProperty,
    InvalidStringValue,
    InvalidNumberValue,
    InvalidFunction,
    InvalidObjectValue,
    CallJsFailed,
}

impl From<NulError> for ArkTsObjErr {
    fn from(_value: NulError) -> Self {
        ArkTsObjErr::NullError
    }
}

pub struct ArkTsObjRef {
    raw_env: napi_env,
    obj_ref: napi_ref,
    uv_loop: *mut uv_loop_t,
    val_tx: mpsc::Sender<Result<napi_value, ArkTsObjErr>>,
    val_rx: mpsc::Receiver<Result<napi_value, ArkTsObjErr>>,
    //---------- args for work cb
    fn_name: String,
    argc: usize,
    argv: *const napi_value,
    worker: *mut uv_work_t,
}

impl Drop for ArkTsObjRef {
    fn drop(&mut self) {
        if self.worker.is_null() == false {
            let layout = std::alloc::Layout::new::<uv_work_t>();
            unsafe { std::alloc::dealloc(self.worker as *mut u8, layout) };
        }
    }
}

impl ArkTsObjRef {
    pub fn new(env: napi_env, obj: napi_ref) -> Self {
        let (tx, rx) = mpsc::channel();
        let layout = std::alloc::Layout::new::<uv_work_t>();
        let req = unsafe { std::alloc::alloc(layout) as *mut uv_work_t };
        let uv_loop = oh_util::get_uv_loop(env).unwrap();
        ArkTsObjRef {
            raw_env: env,
            obj_ref: obj,
            uv_loop: uv_loop,
            val_tx: tx,
            val_rx: rx,
            //---------
            fn_name: "undefined".to_string(),
            argc: 0,
            argv: null_mut(),
            worker: req,
        }
    }

    fn get_ref_value(&self) -> Result<napi_value, ArkTsObjErr> {
        let mut result = null_mut();
        let napi_status =
            unsafe { napi_get_reference_value(self.raw_env, self.obj_ref, &mut result) };
        if napi_status != Status::napi_ok {
            crate::error!("failed to get value from reference");
            return Err(ArkTsObjErr::InvalidObjectValue);
        }
        return Ok(result);
    }

    extern "C" fn js_work_cb(_req: *mut uv_work_t) {}

    extern "C" fn js_after_work_cb(req: *mut uv_work_t, _status: c_int) {
        let ark_obj = unsafe { (*req).data as *const ArkTsObjRef };
        let fn_name = unsafe { (*ark_obj).fn_name.clone() };
        let argc = unsafe { (*ark_obj).argc };
        let argv = unsafe { (*ark_obj).argv };
        let raw_env = unsafe { (*ark_obj).raw_env };
        let obj_ref = unsafe { (*ark_obj).obj_ref };
        let val_tx = unsafe { (*ark_obj).val_tx.clone() };

        let mut arkts_obj = null_mut();

        let napi_status = unsafe { napi_get_reference_value(raw_env, obj_ref, &mut arkts_obj) };
        if napi_status != Status::napi_ok {
            crate::error!("failed to get value from reference");
            let _ = val_tx.send(Err(ArkTsObjErr::InvalidObjectValue));
            return;
        }

        let cname = CString::new(fn_name.clone()).unwrap();
        let mut js_fn = null_mut();
        let napi_status =
            unsafe { napi_get_named_property(raw_env, arkts_obj, cname.as_ptr(), &mut js_fn) };
        if napi_status != Status::napi_ok {
            crate::error!("failed to get function {} from arkts object", fn_name);
            let _ = val_tx.send(Err(ArkTsObjErr::InvalidProperty));
            return;
        }

        let mut napi_type: napi_valuetype = 0;
        let _ = unsafe { napi_typeof(raw_env, js_fn, &mut napi_type) };
        if napi_type != ValueType::napi_function {
            crate::error!("property {} is not function", fn_name);
            let _ = val_tx.send(Err(ArkTsObjErr::InvalidFunction));
            return;
        }

        let mut call_result = null_mut();
        let napi_status =
            unsafe { napi_call_function(raw_env, arkts_obj, js_fn, argc, argv, &mut call_result) };
        if napi_status != Status::napi_ok {
            crate::error!("failed to call js function:{}", fn_name);
            let _ = val_tx.send(Err(ArkTsObjErr::CallJsFailed));
            return;
        }
        let _ = val_tx.send(Ok(call_result));
    }

    pub fn get_property(&self, name: &str) -> Result<napi_value, ArkTsObjErr> {
        let object = self.get_ref_value()?;
        match oh_util::get_object_property(self.raw_env, object, &name) {
            Some(val) => Ok(val),
            None => Err(ArkTsObjErr::InvalidProperty),
        }
    }

    pub fn get_string(&self, name: &str) -> Result<String, ArkTsObjErr> {
        let property = self.get_property(name)?;
        match oh_util::get_value_string(self.raw_env, property) {
            Some(val) => Ok(val),
            None => Err(ArkTsObjErr::InvalidStringValue),
        }
    }

    pub fn get_number(&self, name: &str) -> Result<f64, ArkTsObjErr> {
        let property = self.get_property(name)?;
        match oh_util::get_value_f64(self.raw_env, property) {
            Some(val) => Ok(val),
            None => Err(ArkTsObjErr::InvalidNumberValue),
        }
    }

    fn as_ptr(&self) -> *const ArkTsObjRef {
        self as *const ArkTsObjRef
    }

    pub fn call_js_function(
        &mut self,
        name: &str,
        argc: usize,
        argv: *const napi_value,
    ) -> Result<napi_value, ArkTsObjErr> {
        self.fn_name = name.to_string();
        self.argc = argc;
        self.argv = argv;
        unsafe {
            (*(self.worker)).data = self.as_ptr() as *mut c_void;
        }

        let _ = unsafe {
            uv_queue_work(
                self.uv_loop,
                self.worker,
                Some(Self::js_work_cb),
                Some(Self::js_after_work_cb),
            )
        };
        let ret = match self.val_rx.recv() {
            Ok(r) => r,
            Err(e) => {
                crate::error!(
                    "failed to get result for js function {}, error = {}",
                    name,
                    e
                );
                Err(ArkTsObjErr::CallJsFailed)
            }
        };
        ret
    }

    pub fn raw(&self) -> napi_env {
        self.raw_env
    }
}
