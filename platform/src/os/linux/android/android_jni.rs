use makepad_jni_sys as jni_sys;
use crate::module_loader::ModuleLoader;

use {
    std::sync::Mutex,
    std::{cell::Cell, ffi::CString, sync::mpsc::{self, Sender}},
    self::super::{
        ndk_sys,
        ndk_utils,
    },
    crate::{
        area::Area,
        cx::AndroidParams,
        event::{HttpRequest, TouchPoint, TouchState, VideoSource},
        makepad_live_id::*,
        makepad_math::*,
        WebSocketMessage,
    },
    makepad_android_state::{get_activity, get_java_vm},
};


#[derive(Debug)]
pub enum TouchPhase{
    Moved,
    Ended,
    Started,
    Cancelled,
}

#[derive(Debug)]
pub enum FromJavaMessage {
    Init(AndroidParams),
    SwitchedActivity(jni_sys::jobject, u64),
    BackPressed,
    SurfaceChanged {
        window: *mut ndk_sys::ANativeWindow,
        width: i32,
        height: i32,
    },
    SurfaceCreated {
        window: *mut ndk_sys::ANativeWindow,
    },
    SurfaceDestroyed,
    RenderLoop,
    LongClick {
        abs: DVec2,
        pointer_id: u64,
        // The SystemClock time (in seconds) when the LongClick occurred.
        time: f64,
    },
    Touch(Vec<TouchPoint>),
    Character {
        character: u32,
    },
    KeyDown {
        keycode: u32,
        meta_state: u32,
    },
    KeyUp {
        keycode: u32,
        meta_state: u32,
    },
    ResizeTextIME {
        keyboard_height: u32,
        is_open: bool
    },
    HttpResponse {
        request_id: u64,
        metadata_id: u64,
        status_code: u16,
        headers: String,
        body: Vec<u8>
    },
    HttpRequestError {
        request_id: u64,
        metadata_id: u64,
        error: String,
    },
    WebSocketMessage {
        message: Vec<u8>,
        sender: Box<(u64,Sender<WebSocketMessage>)>,
    },
    WebSocketClosed {
        sender: Box<(u64,Sender<WebSocketMessage>)>,
    },
    WebSocketError {
        error: String,
        sender: Box<(u64,Sender<WebSocketMessage>)>,
    },
    MidiDeviceOpened{
        name: String,
        midi_device: jni_sys::jobject
    },
    VideoPlaybackPrepared {
        video_id: u64,
        video_width: u32,
        video_height: u32,
        duration: u128,
        surface_texture: jni_sys::jobject,
    },
    VideoPlaybackCompleted {
        video_id: u64,
    },
    VideoPlayerReleased {
        video_id: u64,
    },
    VideoDecodingError {
        video_id: u64,
        error: String,
    },
    Pause,
    Resume,
    Start,
    Stop,
    Destroy,
    WindowFocusChanged {
        has_focus: bool,
    },
}
unsafe impl Send for FromJavaMessage {}

static MESSAGES_TX: Mutex<Option<mpsc::Sender<FromJavaMessage>>> = Mutex::new(None);

pub fn send_from_java_message(message: FromJavaMessage) {
    if let Ok(mut tx) = MESSAGES_TX.lock(){
        if let Some(tx) = tx.as_mut(){
            tx.send(message).ok();
        }
        else{
            crate::log!("Receiving message from java whilst already shutdown {:?}", message);
        }
    }
}

// Defined in https://developer.android.com/reference/android/view/KeyEvent#META_CTRL_MASK
pub const ANDROID_META_CTRL_MASK: u32 = 28672;
// Defined in  https://developer.android.com/reference/android/view/KeyEvent#META_SHIFT_MASK
pub const ANDROID_META_SHIFT_MASK: u32 = 193;
// Defined in  https://developer.android.com/reference/android/view/KeyEvent#META_ALT_MASK
pub const ANDROID_META_ALT_MASK: u32 = 50;

pub static mut SET_ACTIVITY_FN: unsafe fn(jni_sys::jobject) = |_| {};

pub fn from_java_messages_already_set()->bool{
    MESSAGES_TX.lock().unwrap().is_some()
}

pub fn from_java_messages_clear(){
    *MESSAGES_TX.lock().unwrap() = None;
}

pub fn jni_update_activity(activity_handle:jni_sys::jobject, ){
    unsafe{SET_ACTIVITY_FN(activity_handle)};
}

pub fn jni_set_activity(activity_handle:jni_sys::jobject){
    unsafe{
        if let Some(func) = makepad_android_state::get_activity_setter_fn() {
            SET_ACTIVITY_FN = func;
        }
        SET_ACTIVITY_FN(activity_handle);
    }
}

pub fn jni_set_from_java_tx(from_java_tx: mpsc::Sender<FromJavaMessage>){
    *MESSAGES_TX.lock().unwrap() = Some(from_java_tx);
}
    
pub unsafe fn fetch_activity_handle(activity:*const std::ffi::c_void)->jni_sys::jobject{
    let env = attach_jni_env();
   (**env).NewGlobalRef.unwrap()(env, activity as jni_sys::jobject)
}

pub unsafe fn attach_jni_env() -> *mut jni_sys::JNIEnv {
    let mut env: *mut jni_sys::JNIEnv = std::ptr::null_mut();
    let attach_current_thread = (**get_java_vm()).AttachCurrentThread.unwrap();

    let res = attach_current_thread(get_java_vm(), &mut env, std::ptr::null_mut());
    assert!(res == 0);

    env
}


unsafe fn create_native_window(surface: jni_sys::jobject) -> *mut ndk_sys::ANativeWindow {
    let env = attach_jni_env();

    ndk_sys::ANativeWindow_fromSurface(env, surface)
}

#[cfg(not(no_android_choreographer))]
static mut CHOREOGRAPHER: *mut ndk_sys::AChoreographer = std::ptr::null_mut();

#[cfg(not(no_android_choreographer))]
static mut CHOREOGRAPHER_POST_CALLBACK_FN: Option<unsafe extern "C" fn(*mut ndk_sys::AChoreographer, Option<unsafe extern "C" fn(*mut ndk_sys::AChoreographerFrameCallbackData, *mut std::ffi::c_void)>, *mut std::ffi::c_void) -> i32> = None;

/// Initializes the render loop which used the Android Choreographer when available to ensure proper vsync.
/// If `no_android_choreographer` is present (e.g. OHOS with non-compatiblity), we fallback to a simple loop with frame pacing.
/// This will be replaced by proper a vsync mechanism once we firgure it out for that OHOS.
#[allow(unused)]
#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_initChoreographer(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    device_refresh_rate: jni_sys::jfloat,
    sdk_version: jni_sys::jint,
) {
    // If the Choreographer is not available (e.g. OHOS), use a manual render loop
    #[cfg(no_android_choreographer)]
    {
        init_simple_render_loop(device_refresh_rate);
        return;
    }
    #[allow(unused)]
    #[cfg(not(no_android_choreographer))]
    {
        // Otherwise use the actual Choreographer
        CHOREOGRAPHER = ndk_sys::AChoreographer_getInstance();
        if sdk_version >= 33 {
            let lib = ModuleLoader::load("libandroid.so").expect("Failed to load libandroid.so");
            let func: Option<ndk_sys::AChoreographerPostCallbackFn> = lib.get_symbol("AChoreographer_postVsyncCallback").ok();
            CHOREOGRAPHER_POST_CALLBACK_FN = func;
        } else if sdk_version >= 29 {
            CHOREOGRAPHER_POST_CALLBACK_FN = Some(ndk_sys::AChoreographer_postFrameCallback64 as _);
        } else {
            init_simple_render_loop(device_refresh_rate);
        }
        post_vsync_callback();
    }
}

#[cfg(not(no_android_choreographer))]
unsafe extern "C" fn vsync_callback(
    _data: *mut ndk_sys::AChoreographerFrameCallbackData,
    _user_data: *mut std::ffi::c_void,
) {
    send_from_java_message(FromJavaMessage::RenderLoop);
    post_vsync_callback();
}

#[cfg(not(no_android_choreographer))]
pub unsafe fn post_vsync_callback() {
    if let Some(post_callback) = CHOREOGRAPHER_POST_CALLBACK_FN {
        if !CHOREOGRAPHER.is_null() && from_java_messages_already_set() {
            post_callback(
                CHOREOGRAPHER,
                Some(vsync_callback),
                std::ptr::null_mut(),
            );
        }
    }
}

fn init_simple_render_loop(device_refresh_rate: f32) {
    std::thread::spawn(move || {
        let mut last_frame_time = std::time::Instant::now();
        let target_frame_time = std::time::Duration::from_secs_f32(1.0 / device_refresh_rate);
        loop {
            let now = std::time::Instant::now();
            let elapsed = now - last_frame_time;
            
            if elapsed >= target_frame_time {
                let frame_start = std::time::Instant::now();
                send_from_java_message(FromJavaMessage::RenderLoop);
                let frame_duration = frame_start.elapsed();
                
                // Adaptive sleep: sleep less if the last frame took longer to process
                if frame_duration < target_frame_time {
                    std::thread::sleep(target_frame_time - frame_duration);
                }
                
                last_frame_time = now;
            } else {
                std::thread::sleep(target_frame_time - elapsed);
            }
        }
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onAndroidParams(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    cache_path: jni_sys::jstring,
    data_path: jni_sys::jstring,
    density: jni_sys::jfloat,
    is_emulator: jni_sys::jboolean,
    android_version: jni_sys::jstring,
    build_number: jni_sys::jstring,
    kernel_version: jni_sys::jstring,
) {
    send_from_java_message(FromJavaMessage::Init(AndroidParams {
        cache_path: jstring_to_string(env, cache_path),
        data_path: jstring_to_string(env, data_path),
        density: density as f64,
        is_emulator: is_emulator != 0,
        android_version: jstring_to_string(env, android_version),
        build_number: jstring_to_string(env, build_number),
        kernel_version: jstring_to_string(env, kernel_version),
        #[cfg(quest)]
        has_xr_mode: true,
        #[cfg(not(quest))]
        has_xr_mode: false,
    }));
}


#[no_mangle]
unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onBackPressed(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
) {
    // crate::log!("Java_dev_makepad_android_MakepadNative_onBackPressed");
    send_from_java_message(FromJavaMessage::BackPressed);
}

#[no_mangle]
unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnStart(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
) {
    send_from_java_message(FromJavaMessage::Start);
}

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
unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnStop(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
) {
    send_from_java_message(FromJavaMessage::Stop);
}

#[no_mangle]
unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnDestroy(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
) {
    send_from_java_message(FromJavaMessage::Destroy);
}

#[no_mangle]
unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_activityOnWindowFocusChanged(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    has_focus: jni_sys::jboolean,
) {
    send_from_java_message(FromJavaMessage::WindowFocusChanged {
        has_focus: has_focus != 0,
    });
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
pub extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnLongClick(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    x: jni_sys::jfloat,
    y: jni_sys::jfloat,
    pointer_id: jni_sys::jint,
    time_millis: jni_sys::jlong,
) {
    send_from_java_message(FromJavaMessage::LongClick {
        abs: DVec2 { x: x as f64, y: y as f64 },
        pointer_id: pointer_id as u64,
        time: time_millis as f64 / 1000.0,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnTouch(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    event: jni_sys::jobject,
) {
    let action_masked = unsafe {ndk_utils::call_int_method!(env, event, "getActionMasked", "()I")};
    let action_index = unsafe {ndk_utils::call_int_method!(env, event, "getActionIndex", "()I")};
    let touch_count = unsafe {ndk_utils::call_int_method!(env, event, "getPointerCount", "()I")};

    let time = unsafe {ndk_utils::call_long_method!(env, event, "getEventTime", "()J")} as i64;
    
    let mut touches = Vec::with_capacity(touch_count as usize);
    for touch_index in 0..touch_count {
        let id = unsafe {ndk_utils::call_int_method!(env, event, "getPointerId", "(I)I", touch_index)};
        let x = unsafe {ndk_utils::call_float_method!(env, event, "getX", "(I)F", touch_index)};
        let y = unsafe {ndk_utils::call_float_method!(env, event, "getY", "(I)F", touch_index)};
        let rotation_angle = unsafe {ndk_utils::call_float_method!(env, event, "getOrientation", "(I)F", touch_index)} as f64;
        let force = unsafe {ndk_utils::call_float_method!(env, event, "getPressure", "(I)F", touch_index)} as f64;

        touches.push(TouchPoint {
            state: {
                if action_index == touch_index {
                    match action_masked {
                        0 | 5 => TouchState::Start,
                        1 | 6 => TouchState::Stop,
                        2 => TouchState::Move,
                        _ => return,
                    }
                }
                else {
                    TouchState::Move
                }
            },
            uid: id as u64,
            rotation_angle,
            force,
            radius: dvec2(1.0, 1.0),
            handled: Cell::new(Area::Empty),
            sweep_lock: Cell::new(Area::Empty),
            abs: dvec2(x as f64, y as f64),
            time: time as f64 / 1000.0,
        });
    }
    send_from_java_message(FromJavaMessage::Touch(touches));
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnKeyDown(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    keycode: jni_sys::jint,
    meta_state: jni_sys::jint,
) {
    send_from_java_message(FromJavaMessage::KeyDown {
        keycode: keycode as u32,
        meta_state: meta_state as u32,
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnKeyUp(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    keycode: jni_sys::jint,
    meta_state: jni_sys::jint,
) {
    send_from_java_message(FromJavaMessage::KeyUp {
        keycode: keycode as u32,
        meta_state: meta_state as u32,
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnCharacter(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    character: jni_sys::jint,
) {
    send_from_java_message(FromJavaMessage::Character {
        character: character as u32,
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnResizeTextIME(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    keyboard_height: jni_sys::jint,
    is_open: jni_sys::jboolean,
) {
    send_from_java_message(FromJavaMessage::ResizeTextIME {
        keyboard_height: keyboard_height as u32,
        is_open: is_open != 0
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_onRenderLoop(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
) {
    send_from_java_message(FromJavaMessage::RenderLoop);
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_onHttpResponse(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    request_id: jni_sys::jlong,
    metadata_id: jni_sys::jlong,
    status_code: jni_sys::jint,
    headers: jni_sys::jstring,
    body: jni_sys::jobject,
) {
    let headers = unsafe { jstring_to_string(env, headers) };
    let body = unsafe { java_byte_array_to_vec(env, body) };

    send_from_java_message(FromJavaMessage::HttpResponse {
        request_id: request_id as u64,
        metadata_id: metadata_id as u64,
        status_code: status_code as u16,
        headers,
        body
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_onHttpRequestError(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    request_id: jni_sys::jlong,
    metadata_id: jni_sys::jlong,
    error: jni_sys::jstring,
) {
    let error = unsafe { jstring_to_string(env, error) };

    send_from_java_message(FromJavaMessage::HttpRequestError {
        request_id: request_id as u64,
        metadata_id: metadata_id as u64,
        error,
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_onWebSocketMessage(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    message: jni_sys::jobject,
    callback: jni_sys::jlong,
) {
    if callback == 0{
        return
    }
    let message = unsafe { java_byte_array_to_vec(env, message) };
    let sender = unsafe { &*(callback as *const Box<(u64,Sender<WebSocketMessage>)>) };

    send_from_java_message(FromJavaMessage::WebSocketMessage {
        message,
        sender: sender.clone(),
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_onWebSocketClosed(
    _env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    callback: jni_sys::jlong,
) {
    if callback == 0{
        return
    }
    let sender = unsafe { &*(callback as *const Box<(u64,Sender<WebSocketMessage>)>) };

    send_from_java_message(FromJavaMessage::WebSocketClosed {
        sender: sender.clone(),
    });
}

#[no_mangle]
extern "C" fn Java_dev_makepad_android_MakepadNative_onWebSocketError(
    _env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    _error: jni_sys::jstring,
    callback: jni_sys::jlong,
) {
    if callback == 0{
        return
    }
    //let error = unsafe { jstring_to_string(env, error) };
    let sender = unsafe { &*(callback as *const Box<(u64,Sender<WebSocketMessage>)>) };

    send_from_java_message(FromJavaMessage::WebSocketError {
        error:"".to_string(),
        sender: sender.clone(),
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onVideoPlaybackPrepared(
    _env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    video_id: jni_sys::jlong,
    video_width: jni_sys::jint,
    video_height: jni_sys::jint,
    duration: jni_sys::jlong,
    surface_texture: jni_sys::jobject
) {
    let env = attach_jni_env();

    let global_ref = (**env).NewGlobalRef.unwrap()(env, surface_texture);

    send_from_java_message(FromJavaMessage::VideoPlaybackPrepared {
        video_id: video_id as u64,
        video_width: video_width as u32,
        video_height: video_height as u32,
        duration: duration as u128,
        surface_texture: global_ref
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onVideoPlaybackCompleted(
    _env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    video_id: jni_sys::jlong,
) {
    send_from_java_message(FromJavaMessage::VideoPlaybackCompleted {
        video_id: video_id as u64,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onVideoPlayerReleased(
    _env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    video_id: jni_sys::jlong,
) {
    send_from_java_message(FromJavaMessage::VideoPlayerReleased {
        video_id: video_id as u64,
    });
}


#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onVideoDecodingError(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    video_id: jni_sys::jlong,
    error: jni_sys::jstring
) {
    let error_string =  unsafe { jstring_to_string(env, error) };
    send_from_java_message(FromJavaMessage::VideoDecodingError {
        video_id: video_id as u64,
        error: error_string,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onMidiDeviceOpened(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    name: jni_sys::jstring,
    midi_device: jni_sys::jobject,
) {
    send_from_java_message(FromJavaMessage::MidiDeviceOpened {
        name: jstring_to_string(env, name),
        midi_device
    });
}

unsafe fn jstring_to_string(env: *mut jni_sys::JNIEnv, java_string: jni_sys::jstring) -> String {
    let chars = (**env).GetStringUTFChars.unwrap()(env, java_string, std::ptr::null_mut());
    let rust_string = std::ffi::CStr::from_ptr(chars).to_str().unwrap().to_string();
    (**env).ReleaseStringUTFChars.unwrap()(env, java_string, chars);
    rust_string
}

unsafe fn java_string_array_to_vec(env: *mut jni_sys::JNIEnv, object_array: jni_sys::jobject) -> Vec<String> {
    if object_array == std::ptr::null_mut() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let length = (**env).GetArrayLength.unwrap()(env, object_array);
    for i in 0..length {
        let string = (**env).GetObjectArrayElement.unwrap()(env, object_array, i as jni_sys::jsize);
        out.push(jstring_to_string(env, string));
    }
    out
}

unsafe fn java_byte_array_to_vec(env: *mut jni_sys::JNIEnv, byte_array: jni_sys::jobject) -> Vec<u8> {
    let bytes = (**env).GetByteArrayElements.unwrap()(env, byte_array, std::ptr::null_mut());
    let length = (**env).GetArrayLength.unwrap()(env, byte_array);
    let mut out_bytes = Vec::new();
    let slice = std::slice::from_raw_parts(bytes as *const u8, length as usize);
    out_bytes.extend_from_slice(slice);
    (**env).ReleaseByteArrayElements.unwrap()(env, byte_array, bytes, jni_sys::JNI_ABORT);
    out_bytes
}

pub unsafe fn to_java_set_full_screen(env: *mut jni_sys::JNIEnv, fullscreen: bool) {
    ndk_utils::call_void_method!(env, get_activity(), "setFullScreen", "(Z)V", fullscreen as i32);
}

pub unsafe fn to_java_switch_activity(env: *mut jni_sys::JNIEnv) {
    ndk_utils::call_void_method!(env, get_activity(), "switchActivity", "()V");
}


pub(crate) unsafe fn to_java_load_asset(filepath: &str)->Option<Vec<u8>> {
    let env = attach_jni_env();

    let get_method_id = (**env).GetMethodID.unwrap();
    let get_object_class = (**env).GetObjectClass.unwrap();
    let call_object_method = (**env).CallObjectMethod.unwrap();

    let mid = (get_method_id)(
        env,
        get_object_class(env, get_activity()),
        b"getAssets\0".as_ptr() as _,
        b"()Landroid/content/res/AssetManager;\0".as_ptr() as _,
    );
    let asset_manager = (call_object_method)(env, get_activity(), mid);
    let mgr = ndk_sys::AAssetManager_fromJava(env, asset_manager);
    let file_path = CString::new(filepath).unwrap();
    let asset = ndk_sys::AAssetManager_open(mgr, file_path.as_ptr(), ndk_sys::AASSET_MODE_BUFFER as _);
    if asset.is_null() {
        return None;
    }
    let length = ndk_sys::AAsset_getLength64(asset);

    let mut buffer = Vec::new();
    buffer.resize(length as usize, 0u8);
    if ndk_sys::AAsset_read(asset, buffer.as_ptr() as *mut _, length as _) > 0 {
        ndk_sys::AAsset_close(asset);
        return Some(buffer)
    }
    return None;
}

pub unsafe fn to_java_show_keyboard(visible: bool) {
    let env = attach_jni_env();
    ndk_utils::call_void_method!(env, get_activity(), "showKeyboard", "(Z)V", visible as i32);
}

pub unsafe fn to_java_copy_to_clipboard(content: String) {
    let env = attach_jni_env();
    let content = CString::new(content.clone()).unwrap();
    let content = ((**env).NewStringUTF.unwrap())(env, content.as_ptr());
    ndk_utils::call_void_method!(env, get_activity(), "copyToClipboard", "(Ljava/lang/String;)V", content);
}

pub unsafe fn to_java_http_request(request_id: LiveId, request: HttpRequest) {
    let env = attach_jni_env();
    let url = CString::new(request.url.clone()).unwrap();
    let url = ((**env).NewStringUTF.unwrap())(env, url.as_ptr());

    let method = CString::new(request.method.to_string()).unwrap();
    let method = ((**env).NewStringUTF.unwrap())(env, method.as_ptr());

    let headers_string = request.get_headers_string();
    let headers = CString::new(headers_string.clone()).unwrap();
    let headers = ((**env).NewStringUTF.unwrap())(env, headers.as_ptr());

    let java_body = match &request.body {
        Some(body) => {
            let java_body = (**env).NewByteArray.unwrap()(env, body.len() as i32);
            (**env).SetByteArrayRegion.unwrap()(
                env,
                java_body,
                0,
                body.len() as i32,
                body.as_ptr() as *const jni_sys::jbyte,
            );
            java_body
        }
        None => std::ptr::null_mut(),
    };

    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "requestHttp",
        "(JJLjava/lang/String;Ljava/lang/String;Ljava/lang/String;[B)V",
        request_id.get_value() as jni_sys::jlong,
        request.metadata_id.get_value() as jni_sys::jlong,
        url,
        method,
        headers,
        java_body as jni_sys::jobject
    );
}

pub unsafe fn to_java_websocket_open(
    request_id: LiveId,
    request: HttpRequest,
    recv: *const Box<(u64,std::sync::mpsc::Sender<WebSocketMessage>)>
) {
    let env = attach_jni_env();
    let url = CString::new(request.url.clone()).unwrap();
    let url = ((**env).NewStringUTF.unwrap())(env, url.as_ptr());

    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "openWebSocket",
        "(JLjava/lang/String;J)V",
        request_id.get_value() as jni_sys::jlong,
        url,
        recv as jni_sys::jlong
    );
}

pub unsafe fn to_java_websocket_send_message(request_id: LiveId, message: Vec<u8>) {
    let env = attach_jni_env();
    let message_bytes = (**env).NewByteArray.unwrap()(env, message.len() as i32);
    (**env).SetByteArrayRegion.unwrap()(
        env,
        message_bytes,
        0,
        message.len() as i32,
        message.as_ptr() as *const jni_sys::jbyte,
    );

    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "sendWebSocketMessage",
        "(J[B)V",
        request_id.get_value() as jni_sys::jlong,
        message_bytes as jni_sys::jobject
    );
}

pub unsafe fn to_java_websocket_close(request_id: LiveId) {
    let env = attach_jni_env();

    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "closeWebSocket",
        "(J)V",
        request_id.get_value() as jni_sys::jlong
    );
}

pub fn to_java_get_audio_devices(flag: jni_sys::jlong) -> Vec<String> {
    unsafe {
        let env = attach_jni_env();
        let string_array = ndk_utils::call_object_method!(env, get_activity(), "getAudioDevices", "(J)[Ljava/lang/String;", flag);
        return java_string_array_to_vec(env, string_array);
    }
}

pub fn to_java_open_all_midi_devices(delay: jni_sys::jlong) {
    unsafe {
        let env = attach_jni_env();
        ndk_utils::call_void_method!(env, get_activity(), "openAllMidiDevices", "(J)V", delay);
    }
}

pub unsafe fn to_java_prepare_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId, source: VideoSource, external_texture_handle: u32, autoplay: bool, should_loop: bool) {
    let video_source = match source {
        VideoSource::InMemory(data) => {
            let source = &*data;

            let java_body = (**env).NewByteArray.unwrap()(env, source.len() as i32);
            (**env).SetByteArrayRegion.unwrap()(
                env,
                java_body,
                0,
                source.len() as i32,
                source.as_ptr() as *const jni_sys::jbyte,
            );

            java_body as jni_sys::jobject
        },
        VideoSource::Network(url) | VideoSource::Filesystem(url) => {
            let url = CString::new(url.clone()).unwrap();
            let url = ((**env).NewStringUTF.unwrap())(env, url.as_ptr());
            url
        }
    };

    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "prepareVideoPlayback",
        "(JLjava/lang/Object;IZZ)V",
        video_id.get_value() as jni_sys::jlong,
        video_source,
        external_texture_handle as jni_sys::jint,
        autoplay as jni_sys::jboolean as std::ffi::c_uint,
        should_loop as jni_sys::jboolean as std::ffi::c_uint
    );

    (**env).DeleteLocalRef.unwrap()(env, video_source);
}

pub unsafe fn to_java_update_tex_image(env: *mut jni_sys::JNIEnv, video_decoder_ref: jni_sys::jobject) -> bool {
    let class = (**env).GetObjectClass.unwrap()(env, video_decoder_ref);
    let update_tex_image_cstring = CString::new("maybeUpdateTexImage").unwrap();
    let signature_cstring = CString::new("()Z").unwrap();
    let mid_update_tex_image = (**env).GetMethodID.unwrap()(
        env,
        class,
        update_tex_image_cstring.as_ptr(),
        signature_cstring.as_ptr()
    );

    let updated = (**env).CallBooleanMethod.unwrap()(env, video_decoder_ref, mid_update_tex_image);
    (**env).DeleteLocalRef.unwrap()(env, class);

    updated != 0
}

pub unsafe fn to_java_begin_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "beginVideoPlayback",
        "(J)V",
        video_id
    );
}

pub unsafe fn to_java_pause_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "pauseVideoPlayback",
        "(J)V",
        video_id
    );
}

pub unsafe fn to_java_resume_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "resumeVideoPlayback",
        "(J)V",
        video_id
    );
}

pub unsafe fn to_java_mute_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "muteVideoPlayback",
        "(J)V",
        video_id
    );
}

pub unsafe fn to_java_unmute_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "unmuteVideoPlayback",
        "(J)V",
        video_id
    );
}

pub unsafe fn to_java_cleanup_video_playback_resources(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "cleanupVideoPlaybackResources",
        "(J)V",
        video_id
    );
}

pub unsafe fn to_java_cleanup_video_decoder_ref(env: *mut jni_sys::JNIEnv, video_decoder_ref: jni_sys::jobject) {
    (**env).DeleteGlobalRef.unwrap()(env, video_decoder_ref);
}
