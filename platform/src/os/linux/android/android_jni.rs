#![allow(dead_code)]

use {
    self::super::{
        jni_sys::{jclass, jint, jlong, jstring, jobject, JNIEnv},
    },
    crate::{
        cx::{Cx,AndroidParams},
    },
    std::{
        ffi::{CString},
        marker::PhantomData,
    },
};

#[derive(Debug)]
pub enum TouchAction {
    Down(usize),
    Up(usize),
    Move(usize),
}

#[derive(Debug)]
pub struct TouchPointer {
    pub id: i32,
    pub x: f32,
    pub y: f32,
    pub orientation: f32,
    pub pressure: f32,
}

/// This struct corresponds to the `Makepad.Callback` interface in Java (which is implemented by
/// the `MakepadSurface` class) and enables us to call methods on that interface while hiding as
/// much of the Java native interface from our Rust code as possible.
///
/// The lifetime is necessary here because object pointers in Java are not stable, so the object
/// pointer wrapped by this struct is really only valid for the duration of each native call.
pub struct AndroidCallback<'a> {
    env: *mut JNIEnv,
    callback: jobject,
    phantom: PhantomData<&'a ()>,
}

impl<'a> AndroidCallback<'a> {
    /// Swaps the buffers of the MakepadSurface.
    pub fn swap_buffers(&self) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("swapBuffers").unwrap();
            let signature = CString::new("()V").unwrap();
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
            ((**self.env).CallVoidMethod.unwrap())(self.env, self.callback, method_id);
        }
    }
    
    /// Schedules a call to `Cx::draw`.
    ///
    /// This works by marking the MakepadSurface as dirty and therefore *should* synchronize
    /// correctly with vsync.
    pub fn schedule_redraw(&self) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("scheduleRedraw").unwrap();
            let signature = CString::new("()V").unwrap();
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
            ((**self.env).CallVoidMethod.unwrap())(self.env, self.callback, method_id);
        }
    }
    
    /// Schedules a timeout with the given `id` and `delay`, where `delay` is given in
    /// milliseconds.
    ///
    /// It is your responsibility to make sure that timeout ids are unique.
    pub fn schedule_timeout(&self, id: i64, delay: i64) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("scheduleTimeout").unwrap();
            let signature = CString::new("(JJ)V").unwrap();
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
            ((**self.env).CallVoidMethod.unwrap())(self.env, self.callback, method_id, id, delay);
        }
    }
    
    /// Cancels the timeout with the given id.
    ///
    /// It is your responsibility to make sure that timeout ids are unique.
    pub fn cancel_timeout(&self, id: i64) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("cancelTimeout").unwrap();
            let signature = CString::new("(J)V").unwrap();
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
            ((**self.env).CallVoidMethod.unwrap())(self.env, self.callback, method_id, id);
        }
    }
}

// The functions here correspond to the static functions on the `Makepad` class in Java.


#[no_mangle]
pub unsafe extern "C" fn Java_nl_makepad_android_Makepad_dropCx(_: JNIEnv, _: jclass, app: jlong) {
    //log!("DROP!"); 
    //drop(Box::from_raw(app as *mut Cx));
}

unsafe fn jstring_to_string(env:*mut JNIEnv, java_string: jstring)->String{
    let chars = (**env).GetStringUTFChars.unwrap()(env, java_string, std::ptr::null_mut());
    let rust_string = std::ffi::CStr::from_ptr(chars).to_str().unwrap().to_string();
    (**env).ReleaseStringUTFChars.unwrap()(env, java_string, chars);
    rust_string
}

pub struct AndroidInitParams{
    pub cache_path: String,
}

#[no_mangle]
pub unsafe extern "C" fn Java_nl_makepad_android_Makepad_init(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    cache_path: jstring,
    callback: jobject,
) {
    let cache_path = 
    (*(cx as *mut Cx)).android_init(
        AndroidParams{
            cache_path: jstring_to_string(env, cache_path)
        },
        AndroidCallback {
          
        env,
        callback,
        phantom: PhantomData,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_nl_makepad_android_Makepad_resize(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    width: jint,
    height: jint,
    callback: jobject,
) {
    (*(cx as *mut Cx)).android_resize(
        width,
        height,
        AndroidCallback {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_nl_makepad_android_Makepad_draw(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).android_draw(AndroidCallback {
        env,
        callback,
        phantom: PhantomData,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_nl_makepad_android_Makepad_touch(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    event: jobject,
    callback: jobject,
) {
    let action_masked = unsafe {
        let class = ((**env).GetObjectClass.unwrap())(env, event);
        let name = CString::new("getActionMasked").unwrap();
        let signature = CString::new("()I").unwrap();
        let method_id =
        ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
        ((**env).CallIntMethod.unwrap())(env, event, method_id)
    };
    let action_index = unsafe {
        let class = ((**env).GetObjectClass.unwrap())(env, event);
        let name = CString::new("getActionIndex").unwrap();
        let signature = CString::new("()I").unwrap();
        let method_id =
        ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
        ((**env).CallIntMethod.unwrap())(env, event, method_id)
    };
    
    let action = match action_masked {
        0 | 5 => TouchAction::Down(action_index as usize),
        1 | 6 => TouchAction::Up(action_index as usize),
        2 => TouchAction::Move(action_index as usize),
        _ => return,
    };
    
    let pointer_count = unsafe {
        let class = ((**env).GetObjectClass.unwrap())(env, event);
        let name = CString::new("getPointerCount").unwrap();
        let signature = CString::new("()I").unwrap();
        let method_id =
        ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
        ((**env).CallIntMethod.unwrap())(env, event, method_id)
    };
    
    let mut pointers = Vec::with_capacity(pointer_count as usize);
    for pointer_index in 0..pointer_count {
        let id = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getPointerId").unwrap();
            let signature = CString::new("(I)I").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallIntMethod.unwrap())(env, event, method_id, pointer_index)
        };
        
        let x = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getX").unwrap();
            let signature = CString::new("(I)F").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallFloatMethod.unwrap())(env, event, method_id, pointer_index)
        };
        
        let y = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getY").unwrap();
            let signature = CString::new("(I)F").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallFloatMethod.unwrap())(env, event, method_id, pointer_index)
        };
        
        let orientation = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getOrientation").unwrap();
            let signature = CString::new("(I)F").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallFloatMethod.unwrap())(env, event, method_id, pointer_index)
        };
        
        let pressure = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getPressure").unwrap();
            let signature = CString::new("(I)F").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallFloatMethod.unwrap())(env, event, method_id, pointer_index)
        };
        
        pointers.push(TouchPointer {id, x, y, orientation, pressure});
        /*
        points.push(TouchPoint {
            state: {
                if action_index == index {
                    match action_masked {
                        0 | 5 => TouchAction::Down(action_index as usize),
                        1 | 6 => TouchAction::Up(action_index as usize),
                        2 => TouchAction::Move,
                        _ => return,
                    }
                }
                else {
                    TouchState::Move
                }
            },
            id,
            x,
            y,
            orientation,
            pressure
        });*/
    }
    
    (*(cx as *mut Cx)).android_touch(
        action,
        &pointers,
        AndroidCallback {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_nl_makepad_android_Makepad_timeout(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    id: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).android_timeout(
        id,
        AndroidCallback {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}
