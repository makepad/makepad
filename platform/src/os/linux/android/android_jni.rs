use crate::{
    event::{KeyCode},
};

use std::{cell::RefCell, sync::mpsc};
use self::super::ndk_sys;
use self::super::jni_sys;
use self::super::ndk_utils;

#[no_mangle]
pub unsafe extern "C" fn JNI_OnLoad(
    vm: *mut jni_sys::JavaVM,
    _: std::ffi::c_void,
) -> jni_sys::jint {
    VM = vm as *mut _ as _;

    jni_sys::JNI_VERSION_1_6 as _
}

/// Short recap on how miniquad on Android works
/// There is a MainActivity, a normal Java activity
/// It creates a View and pass a reference to a view to rust.
/// Rust spawn a thread that render things into this view as often as
/// possible.
/// Also MainActivty collects user input events and calls native rust functions.
///
/// This long explanation was to illustrate how we ended up with evets callback
/// and drawing in the different threads.
/// Message enum is used to send data from the callbacks to the drawing thread.
#[derive(Debug)]
pub enum TouchPhase{
    Moved,
    Ended,
    Started,
    Cancelled,
}

#[derive(Debug)]
pub enum FromJavaMessage {
    SurfaceChanged {
        window: *mut ndk_sys::ANativeWindow,
        width: i32,
        height: i32,
    },
    SurfaceCreated {
        window: *mut ndk_sys::ANativeWindow,
    },
    SurfaceDestroyed,
    Touch {
        phase: TouchPhase,
        touch_id: u64,
        x: f32,
        y: f32,
    },
    Character {
        character: u32,
    },
    KeyDown {
        keycode: KeyCode,
    },
    KeyUp {
        keycode: KeyCode,
    },
    Pause,
    Resume,
    Destroy,
}
unsafe impl Send for FromJavaMessage {}

thread_local! {
    static MESSAGES_TX: RefCell<Option<mpsc::Sender<FromJavaMessage>>> = RefCell::new(None);
}

fn send_from_java_message(message: FromJavaMessage) {
    MESSAGES_TX.with(|tx| {
        let mut tx = tx.borrow_mut();
        tx.as_mut().unwrap().send(message).unwrap();
    })
}

static mut ACTIVITY: jni_sys::jobject = std::ptr::null_mut();
static mut VM: *mut jni_sys::JavaVM = std::ptr::null_mut();

pub unsafe fn jni_init_globals(activity:*const std::ffi::c_void, from_java_tx: mpsc::Sender<FromJavaMessage>){
    let env = attach_jni_env();
    ACTIVITY = (**env).NewGlobalRef.unwrap()(env, activity as jni_sys::jobject);
    MESSAGES_TX.with(move |messages_tx| *messages_tx.borrow_mut() = Some(from_java_tx));
}
/*
pub unsafe fn console_debug(msg: *const ::std::os::raw::c_char) {
    ndk_sys::__android_log_write(
        ndk_sys::android_LogPriority_ANDROID_LOG_DEBUG as _,
        b"SAPP\0".as_ptr() as _,
        msg,
    );
}

pub unsafe fn console_info(msg: *const ::std::os::raw::c_char) {
    ndk_sys::__android_log_write(
        ndk_sys::android_LogPriority_ANDROID_LOG_INFO as _,
        b"SAPP\0".as_ptr() as _,
        msg,
    );
}

pub unsafe fn console_warn(msg: *const ::std::os::raw::c_char) {
    ndk_sys::__android_log_write(
        ndk_sys::android_LogPriority_ANDROID_LOG_WARN as _,
        b"SAPP\0".as_ptr() as _,
        msg,
    );
}

pub unsafe fn console_error(msg: *const ::std::os::raw::c_char) {
    ndk_sys::__android_log_write(
        ndk_sys::android_LogPriority_ANDROID_LOG_ERROR as _,
        b"SAPP\0".as_ptr() as _,
        msg,
    );
}*/

// fn log_info(message: &str) {
//     use std::ffi::CString;

//     let msg = CString::new(message).unwrap_or_else(|_| panic!());

//     unsafe { console_info(msg.as_ptr()) };
// }
/*
impl MainThreadState {
    unsafe fn destroy_surface(&mut self) {
        (self.libegl.eglMakeCurrent.unwrap())(
            self.egl_display,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        (self.libegl.eglDestroySurface.unwrap())(self.egl_display, self.surface);
        self.surface = std::ptr::null_mut();
    }

    unsafe fn update_surface(&mut self, window: *mut ndk_sys::ANativeWindow) {
        if !self.window.is_null() {
            ndk_sys::ANativeWindow_release(self.window);
        }
        self.window = window;
        if self.surface.is_null() == false {
            self.destroy_surface();
        }

        self.surface = (self.libegl.eglCreateWindowSurface.unwrap())(
            self.egl_display,
            self.egl_config,
            window as _,
            std::ptr::null_mut(),
        );

        assert!(!self.surface.is_null());

        let res = (self.libegl.eglMakeCurrent.unwrap())(
            self.egl_display,
            self.surface,
            self.surface,
            self.egl_context,
        );

        assert!(res != 0);
    }

    

    fn frame(&mut self) {
        self.event_handler.update();

        if self.surface.is_null() == false {
            self.event_handler.draw();

            unsafe {
                (self.libegl.eglSwapBuffers.unwrap())(self.egl_display, self.surface);
            }
        }
    }
/*
    fn process_request(&mut self, request: crate::native::Request) {
        use crate::native::Request::*;
        unsafe {
            match request {
                SetFullscreen(fullscreen) => {
                    unsafe {
                        let env = attach_jni_env();
                        set_full_screen(env, fullscreen);
                    }
                    self.fullscreen = fullscreen;
                }
                ShowKeyboard(show) => unsafe {
                    let env = attach_jni_env();
                    ndk_utils::call_void_method!(
                        env,
                        ACTIVITY,
                        "showKeyboard",
                        "(Z)V",
                        show as i32
                    );
                },
                _ => {}
            }
        }
    }*/
}
*/
/// Get the JNI Env by calling ndk's AttachCurrentThread
///
/// Safety note: This function is not exactly correct now, it should be fixed!
///
/// AttachCurrentThread should be called at least once for any given thread that
/// wants to use the JNI and DetachCurrentThread should be called only once, when
/// the thread stack is empty and the thread is about to stop
///
/// calling AttachCurrentThread from the same thread multiple time is very cheap
///
/// BUT! there is no DetachCurrentThread call right now, this code:
/// `thread::spawn(|| attach_jni_env());` will lead to internal jni crash :/
/// thread::spawn(|| { attach_jni_env(); loop {} }); is basically what miniquad
/// is doing. this is not correct, but works
/// TODO: the problem here -
/// TODO:   thread::spawn(|| { Attach(); .. Detach() }); will not work as well.
/// TODO: JNI will check that thread's stack is still alive and will crash.
///
/// TODO: Figure how to get into the thread destructor to correctly call Detach
/// TODO: (this should be a GH issue)
/// TODO: for reference - grep for "pthread_setspecific" in SDL2 sources, SDL fixed it!
pub unsafe fn attach_jni_env() -> *mut jni_sys::JNIEnv {
    let mut env: *mut jni_sys::JNIEnv = std::ptr::null_mut();
    let attach_current_thread = (**VM).AttachCurrentThread.unwrap();

    let res = attach_current_thread(VM, &mut env, std::ptr::null_mut());
    assert!(res == 0);

    env
}

/*
pub unsafe fn android_main(){
    {
        use std::ffi::CString;
        use std::panic;

        panic::set_hook(Box::new(|info| {
            let msg = CString::new(format!("{:?}", info)).unwrap_or_else(|_| {
                CString::new(format!("MALFORMED ERROR MESSAGE {:?}", info.location())).unwrap()
            });
            console_error(msg.as_ptr());
        }));
    }

    //if conf.fullscreen {
        let env = attach_jni_env();
        set_full_screen(env, true);
    //}

    let (tx, rx) = mpsc::channel();

    MESSAGES_TX.with(move |messages_tx| *messages_tx.borrow_mut() = Some(tx));

    thread::spawn(move || {
        let mut libegl = LibEgl::try_load().expect("Cant load LibEGL");

        // skip all the messages until android will be able to actually open a window
        //
        // sometimes before launching an app android will show a permission dialog
        // it is important to create GL context only after a first SurfaceChanged
        let (window, screen_width, screen_height) = 'a: loop {
            match rx.try_recv() {
                Ok(Message::SurfaceChanged {
                    window,
                    width,
                    height,
                }) => {
                    break 'a (window, width as f32, height as f32);
                }
                _ => {}
            }
        };

        let (egl_context, egl_config, egl_display) = android_egl::create_egl_context(
            &mut libegl,
            std::ptr::null_mut(), /* EGL_DEFAULT_DISPLAY */
            false,
        )
        .expect("Cant create EGL context");

        assert!(!egl_display.is_null());
        assert!(!egl_config.is_null());

        unsafe {gl_sys::load_with( | s | {   
            let s = CString::new(s).unwrap();
            libegl.eglGetProcAddress(s.as_ptr()) 
        })};
        
        let surface = (libegl.eglCreateWindowSurface.unwrap())(
            egl_display,
            egl_config,
            window as _,
            std::ptr::null_mut(),
        );

        if (libegl.eglMakeCurrent.unwrap())(egl_display, surface, surface, egl_context) == 0 {
            panic!();
        }

        let (tx, requests_rx) = std::sync::mpsc::channel();
        /*
        crate::set_display(NativeDisplayData {
            high_dpi: conf.high_dpi,
            ..NativeDisplayData::new(screen_width as _, screen_height as _, tx, clipboard)
        });*/

        let mut s = MainThreadState {
            libegl,
            egl_display,
            egl_config,
            egl_context,
            surface,
            window,
            quit: false,
            fullscreen: true,
            /*keymods: KeyMods {
                shift: false,
                ctrl: false,
                alt: false,
                logo: false,
            },*/
        };

        while !s.quit {
            while let Ok(request) = requests_rx.try_recv() {
                s.process_request(request);
            }

            // process all the messages from the main thread
            while let Ok(msg) = rx.try_recv() {
                s.process_message(msg);
            }

            s.frame();

            thread::yield_now();
        }

        (s.libegl.eglMakeCurrent.unwrap())(
            s.egl_display,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        (s.libegl.eglDestroySurface.unwrap())(s.egl_display, s.surface);
        (s.libegl.eglDestroyContext.unwrap())(s.egl_display, s.egl_context);
        (s.libegl.eglTerminate.unwrap())(s.egl_display);
    });
}*/

#[no_mangle]
extern "C" fn jni_on_load(vm: *mut std::ffi::c_void) {
    unsafe {
        VM = vm as _;
    }
}

unsafe fn create_native_window(surface: jni_sys::jobject) -> *mut ndk_sys::ANativeWindow {
    let env = attach_jni_env();

    ndk_sys::ANativeWindow_fromSurface(env, surface)
}
/*
#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_activityOnCreate(
    _: *mut ndk_sys::JNIEnv,
    _: ndk_sys::jobject,
    activity: ndk_sys::jobject,
) {
    let env = attach_jni_env();
    ACTIVITY = (**env).NewGlobalRef.unwrap()(env, activity);
    android_main();
}
*/
#[no_mangle]
unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnResume(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
) {
    send_from_java_message(FromJavaMessage::Resume);
}

#[no_mangle]
unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnPause(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
) {
    send_from_java_message(FromJavaMessage::Pause);
}

#[no_mangle]
unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnDestroy(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
) {
    send_from_java_message(FromJavaMessage::Destroy);
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnSurfaceCreated(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    surface: jni_sys::jobject,
) {
    let window = unsafe { create_native_window(surface) };
    send_from_java_message(FromJavaMessage::SurfaceCreated { window });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnSurfaceDestroyed(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
) {
    send_from_java_message(FromJavaMessage::SurfaceDestroyed);
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnSurfaceChanged(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    surface: jni_sys::jobject,
    width: jni_sys::jint,
    height: jni_sys::jint,
) {
    let window = unsafe { create_native_window(surface) };

    send_from_java_message(FromJavaMessage::SurfaceChanged {
        window,
        width: width as _,
        height: height as _,
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnTouch(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    touch_id: jni_sys::jint,
    action: jni_sys::jint,
    x: jni_sys::jfloat,
    y: jni_sys::jfloat,
) {
    let phase = match action {
        0 => TouchPhase::Moved,
        1 => TouchPhase::Ended,
        2 => TouchPhase::Started,
        3 => TouchPhase::Cancelled,
        x => panic!("Unsupported touch phase: {}", x),
    };

    send_from_java_message(FromJavaMessage::Touch {
        phase,
        touch_id: touch_id as _,
        x: x as f32,
        y: y as f32,
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnKeyDown(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    _keycode: jni_sys::jint,
) {
    /*let keycode = keycodes::translate_keycode(keycode as _);

    send_message(Message::KeyDown { keycode });*/
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnKeyUp(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    _keycode: jni_sys::jint,
) {
    /*let keycode = keycodes::translate_keycode(keycode as _);

    send_message(Message::KeyUp { keycode });*/
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnCharacter(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    _character: jni_sys::jint,
) {
   /* send_message(Message::Character {
        character: character as u32,
    });*/
}

pub unsafe fn to_java_set_full_screen(env: *mut jni_sys::JNIEnv, fullscreen: bool) {
    ndk_utils::call_void_method!(env, ACTIVITY, "setFullScreen", "(Z)V", fullscreen as i32);
}


// According to documentation, AAssetManager_fromJava is as available as an
// AAssetManager_open, which was used before
// For some reason it is missing fron ndk_sys binding
extern "C" {
    pub fn AAssetManager_fromJava(
        env: *mut jni_sys::JNIEnv,
        assetManager: jni_sys::jobject,
    ) -> *mut ndk_sys::AAssetManager;
}

pub(crate) unsafe fn to_java_load_asset(filepath: *const ::std::os::raw::c_char)->Option<Vec<u8>> {
    let env = attach_jni_env();

    let get_method_id = (**env).GetMethodID.unwrap();
    let get_object_class = (**env).GetObjectClass.unwrap();
    let call_object_method = (**env).CallObjectMethod.unwrap();

    let mid = (get_method_id)(
        env,
        get_object_class(env, ACTIVITY),
        b"getAssets\0".as_ptr() as _,
        b"()Landroid/content/res/AssetManager;\0".as_ptr() as _,
    );
    let asset_manager = (call_object_method)(env, ACTIVITY, mid);
    let mgr = AAssetManager_fromJava(env, asset_manager);
    let asset = ndk_sys::AAssetManager_open(mgr, filepath, ndk_sys::AASSET_MODE_BUFFER as _);
    if asset.is_null() {
        return None;
    }
    let length = ndk_sys::AAsset_getLength64(asset);
    // TODO: memory leak right here! this buffer would never freed
    let mut buffer = Vec::new();
    buffer.resize(length as usize, 0u8);
    if ndk_sys::AAsset_read(asset, buffer.as_ptr() as *mut _, length as _) > 0 {
        ndk_sys::AAsset_close(asset);
        return Some(buffer)
    }
    return None;
    /*
    let buffer = libc::malloc(length as _);
    if ndk_sys::AAsset_read(asset, buffer, length as _) > 0 {
        ndk_sys::AAsset_close(asset);

        (*out).content_length = length as _;
        (*out).content = buffer as _;
    }*/
}
