#![allow(dead_code)]

mod jni;

use {
    jni::{jclass, jint, jlong, jobject, JNIEnv},
    std::{
        ffi::{c_char, c_int, c_void, CString},
        marker::PhantomData,
    },
};

macro_rules! log {
    ($($arg:tt)*) => {
        let tag = CString::new("Makepad").unwrap();
        let text = CString::new(format!($($arg)*)).unwrap();
        unsafe { __android_log_write(3, tag.as_ptr(), text.as_ptr()) };
    }
}

extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

#[link(name = "EGL")]
extern "C" {
    fn eglGetProcAddress(procname: *const c_char) -> *mut c_void;
}

/// This is a stub implementation of `Cx`, intended to be overridden and extended fir the full
/// embedding of Makepad.
#[derive(Debug)]
struct Cx;

impl Cx {
    /// Called when the MakepadActivity is started.
    pub fn new() -> Self {
        log!("Cx::new");
        Self
    }

    /// Called when EGL is initialized.
    pub fn init(&mut self, callback: Callback<'_>) {
        log!("Cx::init");
        gl::load_with(|s| {
            let s = CString::new(s).unwrap();
            unsafe { eglGetProcAddress(s.as_ptr()) }
        });
        callback.schedule_timeout(0, 1000);
    }

    /// Called when the MakepadSurface is resized.
    pub fn resize(&mut self, width: i32, height: i32, _callback: Callback<'_>) {
        log!("Cx::resize {} {}", width, height);
        unsafe {
            gl::Viewport(0, 0, width, height);
        }
    }

    /// Called when the MakepadSurface needs to be redrawn.
    pub fn draw(&mut self, callback: Callback<'_>) {
        log!("Cx::draw");
        unsafe {
            gl::Clear(gl::COLOR_BUFFER_BIT);
        }
        callback.swap_buffers();
    }

    /// Called when a touch event happened on the MakepadSurface.
    pub fn touch(&mut self, action: Action, pointers: &[Pointer], callback: Callback<'_>) {
        log!("Cx::touch {:?}: {:?}", action, pointers);
        unsafe {
            gl::ClearColor(pointers[0].x / 1000.0, pointers[0].y / 2000.0, 0.0, 1.0);
        }
        callback.schedule_redraw();
    }

    /// Called when a timeout expired.
    pub fn timeout(&mut self, id: i64, _callback: Callback<'_>) {
        log!("Cx::timeout {}", id);
    }
}

impl Drop for Cx {
    /// Called when MakepadActivity is stopped.
    fn drop(&mut self) {
        log!("Cx::drop");
    }
}

#[derive(Debug)]
enum Action {
    Down(usize),
    Up(usize),
    Move,
}

#[derive(Debug)]
struct Pointer {
    id: i32,
    x: f32,
    y: f32,
    orientation: f32,
    pressure: f32,
}

/// This struct corresponds to the `Makepad.Callback` interface in Java (which is implemented by
/// the `MakepadSurface` class) and enables us to call methods on that interface while hiding as
/// much of the Java native interface from our Rust code as possible.
///
/// The lifetime is necessary here because object pointers in Java are not stable, so the object
/// pointer wrapped by this struct is really only valid for the duration of each native call.
struct Callback<'a> {
    env: *mut JNIEnv,
    callback: jobject,
    phantom: PhantomData<&'a ()>,
}

impl<'a> Callback<'a> {
    /// Swaps the buffers of the MakepadSurface.
    fn swap_buffers(&self) {
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
    fn schedule_redraw(&self) {
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
    fn schedule_timeout(&self, id: i64, delay: i64) {
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
    fn cancel_timeout(&self, id: i64) {
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
pub unsafe extern "C" fn Java_nl_makepad_android_Makepad_newCx(_: JNIEnv, _: jclass) -> jlong {
    Box::into_raw(Box::new(Cx::new())) as jlong
}

#[no_mangle]
pub unsafe extern "C" fn Java_nl_makepad_android_Makepad_dropCx(_: JNIEnv, _: jclass, cx: jlong) {
    drop(Box::from_raw(cx as *mut Cx));
}

#[no_mangle]
pub unsafe extern "C" fn Java_nl_makepad_android_Makepad_init(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).init(Callback {
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
    (*(cx as *mut Cx)).resize(
        width,
        height,
        Callback {
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
    (*(cx as *mut Cx)).draw(Callback {
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
        0 | 5 => Action::Down(action_index as usize),
        1 | 6 => Action::Up(action_index as usize),
        2 => Action::Move,
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

        pointers.push(Pointer { id, x, y, orientation, pressure });
    }

    (*(cx as *mut Cx)).touch(
        action,
        &pointers,
        Callback {
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
    (*(cx as *mut Cx)).timeout(
        id,
        Callback {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}
