use crate::module_loader::ModuleLoader;
use makepad_jni_sys as jni_sys;

use {
    self::super::{ndk_sys, ndk_utils},
    crate::{
        area::Area,
        cx::AndroidParams,
        event::{TouchPoint, TouchState, VideoSource},
        ime::{AutoCapitalize, AutoCorrect, InputMode, ReturnKeyType, TextInputConfig},
        makepad_live_id::*,
        makepad_math::*,
        makepad_network::HttpRequest,
        makepad_network::WebSocketMessage,
    },
    makepad_android_state::{get_activity, get_java_vm},
    std::ffi::c_uint,
    std::sync::Mutex,
    std::{
        cell::Cell,
        ffi::CString,
        sync::mpsc::{self, Sender},
    },
};

#[derive(Debug)]
pub enum TouchPhase {
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
        abs: Vec2d,
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
        is_open: bool,
    },
    HttpResponse {
        request_id: u64,
        metadata_id: u64,
        status_code: u16,
        headers: String,
        body: Vec<u8>,
    },
    HttpRequestError {
        request_id: u64,
        metadata_id: u64,
        error: String,
    },
    WebSocketMessage {
        message: Vec<u8>,
        sender: Box<(u64, Sender<WebSocketMessage>)>,
    },
    WebSocketClosed {
        sender: Box<(u64, Sender<WebSocketMessage>)>,
    },
    WebSocketError {
        error: String,
        sender: Box<(u64, Sender<WebSocketMessage>)>,
    },
    MidiDeviceOpened {
        name: String,
        midi_device: jni_sys::jobject,
    },
    PermissionResult {
        permission: String,
        request_id: i32,
        status: i32, // 0=NotDetermined, 1=Granted, 2=DeniedCanRetry, 3=DeniedPermanent
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
    ClipboardAction {
        action: String, // "copy", "cut", "select_all"
    },
    ClipboardPaste {
        content: String,
    },
    ImeTextStateChanged {
        full_text: String,
        selection_start: i32,
        selection_end: i32,
        composing_start: i32,
        composing_end: i32,
    },
    ImeEditorAction {
        action_code: i32,
    },
}
unsafe impl Send for FromJavaMessage {}

static MESSAGES_TX: Mutex<Option<mpsc::Sender<FromJavaMessage>>> = Mutex::new(None);

pub fn send_from_java_message(message: FromJavaMessage) {
    if let Ok(mut tx) = MESSAGES_TX.lock() {
        if let Some(tx) = tx.as_mut() {
            tx.send(message).ok();
        } else {
            crate::log!(
                "Receiving message from java whilst already shutdown {:?}",
                message
            );
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

pub fn from_java_messages_already_set() -> bool {
    MESSAGES_TX.lock().unwrap().is_some()
}

pub fn from_java_messages_clear() {
    *MESSAGES_TX.lock().unwrap() = None;
}

pub fn jni_update_activity(activity_handle: jni_sys::jobject) {
    unsafe { SET_ACTIVITY_FN(activity_handle) };
}

pub fn jni_set_activity(activity_handle: jni_sys::jobject) {
    unsafe {
        if let Some(func) = makepad_android_state::get_activity_setter_fn() {
            SET_ACTIVITY_FN = func;
        }
        SET_ACTIVITY_FN(activity_handle);
    }
}

pub fn jni_set_from_java_tx(from_java_tx: mpsc::Sender<FromJavaMessage>) {
    *MESSAGES_TX.lock().unwrap() = Some(from_java_tx);
}

pub unsafe fn fetch_activity_handle(activity: *const std::ffi::c_void) -> jni_sys::jobject {
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
static mut CHOREOGRAPHER_POST_CALLBACK_FN: Option<
    unsafe extern "C" fn(
        *mut ndk_sys::AChoreographer,
        Option<
            unsafe extern "C" fn(
                *mut ndk_sys::AChoreographerFrameCallbackData,
                *mut std::ffi::c_void,
            ),
        >,
        *mut std::ffi::c_void,
    ) -> i32,
> = None;

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
            let func: Option<ndk_sys::AChoreographerPostCallbackFn> =
                lib.get_symbol("AChoreographer_postVsyncCallback").ok();
            // Some runtimes/NDK combos may not expose postVsyncCallback even on API 33+.
            // Fall back to the older frame callback to keep rendering alive.
            CHOREOGRAPHER_POST_CALLBACK_FN =
                func.or(Some(ndk_sys::AChoreographer_postFrameCallback64 as _));
        } else if sdk_version >= 29 {
            CHOREOGRAPHER_POST_CALLBACK_FN = Some(ndk_sys::AChoreographer_postFrameCallback64 as _);
        } else {
            init_simple_render_loop(device_refresh_rate);
        }
        let has_choreographer_callback = match CHOREOGRAPHER_POST_CALLBACK_FN {
            Some(_) => true,
            None => false,
        };
        if has_choreographer_callback {
            post_vsync_callback();
        } else {
            init_simple_render_loop(device_refresh_rate);
        }
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
            post_callback(CHOREOGRAPHER, Some(vsync_callback), std::ptr::null_mut());
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
        abs: Vec2d {
            x: x as f64,
            y: y as f64,
        },
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
    let action_masked =
        unsafe { ndk_utils::call_int_method!(env, event, "getActionMasked", "()I") };
    let action_index = unsafe { ndk_utils::call_int_method!(env, event, "getActionIndex", "()I") };
    let touch_count = unsafe { ndk_utils::call_int_method!(env, event, "getPointerCount", "()I") };

    let time = unsafe { ndk_utils::call_long_method!(env, event, "getEventTime", "()J") } as i64;

    let mut touches = Vec::with_capacity(touch_count as usize);
    for touch_index in 0..touch_count {
        let id =
            unsafe { ndk_utils::call_int_method!(env, event, "getPointerId", "(I)I", touch_index) };
        let x = unsafe { ndk_utils::call_float_method!(env, event, "getX", "(I)F", touch_index) };
        let y = unsafe { ndk_utils::call_float_method!(env, event, "getY", "(I)F", touch_index) };
        let rotation_angle = unsafe {
            ndk_utils::call_float_method!(env, event, "getOrientation", "(I)F", touch_index)
        } as f64;
        let force = unsafe {
            ndk_utils::call_float_method!(env, event, "getPressure", "(I)F", touch_index)
        } as f64;

        // Get actual touch size from Android (returns diameter in pixels)
        let touch_major = unsafe {
            ndk_utils::call_float_method!(env, event, "getTouchMajor", "(I)F", touch_index)
        } as f64;
        let touch_minor = unsafe {
            ndk_utils::call_float_method!(env, event, "getTouchMinor", "(I)F", touch_index)
        } as f64;
        // Convert diameter to radius
        let radius = dvec2(touch_major / 2.0, touch_minor / 2.0);

        touches.push(TouchPoint {
            state: {
                if action_index == touch_index {
                    match action_masked {
                        0 | 5 => TouchState::Start,
                        1 | 6 => TouchState::Stop,
                        2 => TouchState::Move,
                        _ => return,
                    }
                } else {
                    TouchState::Move
                }
            },
            uid: id as u64,
            rotation_angle,
            force,
            radius,
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
        is_open: is_open != 0,
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
    let request_id = LiveId(request_id as u64);
    let metadata_id = LiveId(metadata_id as u64);

    if super::android_network::try_handle_http_response(
        request_id,
        metadata_id,
        status_code as u16,
        &headers,
        &body,
    ) {
        return;
    }

    send_from_java_message(FromJavaMessage::HttpResponse {
        request_id: request_id.0,
        metadata_id: metadata_id.0,
        status_code: status_code as u16,
        headers,
        body,
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
    let request_id = LiveId(request_id as u64);
    let metadata_id = LiveId(metadata_id as u64);

    if super::android_network::try_handle_http_error(request_id, metadata_id, &error) {
        return;
    }

    send_from_java_message(FromJavaMessage::HttpRequestError {
        request_id: request_id.0,
        metadata_id: metadata_id.0,
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
    if callback == 0 {
        return;
    }
    let message = unsafe { java_byte_array_to_vec(env, message) };
    let sender = unsafe { &*(callback as *const Box<(u64, Sender<WebSocketMessage>)>) };

    if super::android_network::try_handle_websocket_message(sender.0, &message, &sender.1) {
        return;
    }

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
    if callback == 0 {
        return;
    }
    let sender = unsafe { &*(callback as *const Box<(u64, Sender<WebSocketMessage>)>) };

    if super::android_network::try_handle_websocket_closed(sender.0, &sender.1) {
        return;
    }

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
    if callback == 0 {
        return;
    }
    let error = unsafe { jstring_to_string(_env, _error) };
    //let error = unsafe { jstring_to_string(env, error) };
    let sender = unsafe { &*(callback as *const Box<(u64, Sender<WebSocketMessage>)>) };

    if super::android_network::try_handle_websocket_error(sender.0, &error, &sender.1) {
        return;
    }

    send_from_java_message(FromJavaMessage::WebSocketError {
        error,
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
    surface_texture: jni_sys::jobject,
) {
    let env = attach_jni_env();

    let global_ref = (**env).NewGlobalRef.unwrap()(env, surface_texture);

    send_from_java_message(FromJavaMessage::VideoPlaybackPrepared {
        video_id: video_id as u64,
        video_width: video_width as u32,
        video_height: video_height as u32,
        duration: duration as u128,
        surface_texture: global_ref,
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
    error: jni_sys::jstring,
) {
    let error_string = unsafe { jstring_to_string(env, error) };
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
        midi_device,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onPermissionResult(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    permission: jni_sys::jstring,
    request_id: jni_sys::jint,
    status: jni_sys::jint,
) {
    send_from_java_message(FromJavaMessage::PermissionResult {
        permission: jstring_to_string(env, permission),
        request_id: request_id as i32,
        status,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onPermissionDenied(
    env: *mut jni_sys::JNIEnv,
    class: jni_sys::jclass,
    permission: jni_sys::jstring,
    request_id: jni_sys::jint,
) {
    Java_dev_makepad_android_MakepadNative_onPermissionResult(
        env, class, permission, request_id, 3,
    ); // 3 = DeniedPermanent (assume worst case)
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onClipboardAction(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    action: jni_sys::jstring,
) {
    send_from_java_message(FromJavaMessage::ClipboardAction {
        action: jstring_to_string(env, action),
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onClipboardPaste(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    content: jni_sys::jstring,
) {
    send_from_java_message(FromJavaMessage::ClipboardPaste {
        content: jstring_to_string(env, content),
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onImeTextStateChanged(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    full_text: jni_sys::jstring,
    selection_start: jni_sys::jint,
    selection_end: jni_sys::jint,
    composing_start: jni_sys::jint,
    composing_end: jni_sys::jint,
) {
    let text = jstring_to_string(env, full_text);
    send_from_java_message(FromJavaMessage::ImeTextStateChanged {
        full_text: text,
        selection_start: selection_start as i32,
        selection_end: selection_end as i32,
        composing_start: composing_start as i32,
        composing_end: composing_end as i32,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onImeEditorAction(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    action_code: jni_sys::jint,
) {
    send_from_java_message(FromJavaMessage::ImeEditorAction {
        action_code: action_code as i32,
    });
}

unsafe fn jstring_to_string(env: *mut jni_sys::JNIEnv, java_string: jni_sys::jstring) -> String {
    let chars = (**env).GetStringUTFChars.unwrap()(env, java_string, std::ptr::null_mut());
    let rust_string = std::ffi::CStr::from_ptr(chars)
        .to_str()
        .unwrap()
        .to_string();
    (**env).ReleaseStringUTFChars.unwrap()(env, java_string, chars);
    rust_string
}

unsafe fn java_string_array_to_vec(
    env: *mut jni_sys::JNIEnv,
    object_array: jni_sys::jobject,
) -> Vec<String> {
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

unsafe fn java_byte_array_to_vec(
    env: *mut jni_sys::JNIEnv,
    byte_array: jni_sys::jobject,
) -> Vec<u8> {
    let bytes = (**env).GetByteArrayElements.unwrap()(env, byte_array, std::ptr::null_mut());
    let length = (**env).GetArrayLength.unwrap()(env, byte_array);
    let mut out_bytes = Vec::new();
    let slice = std::slice::from_raw_parts(bytes as *const u8, length as usize);
    out_bytes.extend_from_slice(slice);
    (**env).ReleaseByteArrayElements.unwrap()(env, byte_array, bytes, jni_sys::JNI_ABORT);
    out_bytes
}

pub unsafe fn to_java_set_full_screen(env: *mut jni_sys::JNIEnv, fullscreen: bool) {
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "setFullScreen",
        "(Z)V",
        fullscreen as i32
    );
}

pub unsafe fn to_java_switch_activity(env: *mut jni_sys::JNIEnv) {
    ndk_utils::call_void_method!(env, get_activity(), "switchActivity", "()V");
}

pub(crate) unsafe fn to_java_load_asset(filepath: &str) -> Option<Vec<u8>> {
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
    let asset =
        ndk_sys::AAssetManager_open(mgr, file_path.as_ptr(), ndk_sys::AASSET_MODE_BUFFER as _);
    if asset.is_null() {
        return None;
    }
    let length = ndk_sys::AAsset_getLength64(asset);

    let mut buffer = Vec::new();
    buffer.resize(length as usize, 0u8);
    if ndk_sys::AAsset_read(asset, buffer.as_ptr() as *mut _, length as _) > 0 {
        ndk_sys::AAsset_close(asset);
        return Some(buffer);
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
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "copyToClipboard",
        "(Ljava/lang/String;)V",
        content
    );
}

pub unsafe fn to_java_paste_from_clipboard() -> String {
    let env = attach_jni_env();
    let result = ndk_utils::call_object_method!(
        env,
        get_activity(),
        "pasteFromClipboard",
        "()Ljava/lang/String;"
    );
    if result.is_null() {
        return String::new();
    }
    jstring_to_string(env, result)
}

pub unsafe fn to_java_show_clipboard_actions(
    has_selection: bool,
    rect: crate::makepad_math::Rect,
    keyboard_shift: f64,
    dpi_factor: f64,
) {
    let env = attach_jni_env();
    // Apply DPI scaling
    let left = (rect.pos.x * dpi_factor) as i32;
    let top = (rect.pos.y * dpi_factor) as i32;
    let right = ((rect.pos.x + rect.size.x) * dpi_factor) as i32;
    let bottom = ((rect.pos.y + rect.size.y) * dpi_factor) as i32;
    let shift = (keyboard_shift * dpi_factor) as i32;
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "showClipboardActions",
        "(ZIIIII)V",
        has_selection as jni_sys::jboolean as std::ffi::c_uint,
        left,
        top,
        right,
        bottom,
        shift
    );
}

pub unsafe fn to_java_dismiss_clipboard_actions() {
    let env = attach_jni_env();
    ndk_utils::call_void_method!(env, get_activity(), "dismissClipboardActions", "()V");
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
    recv: *const Box<(u64, std::sync::mpsc::Sender<WebSocketMessage>)>,
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
        let string_array = ndk_utils::call_object_method!(
            env,
            get_activity(),
            "getAudioDevices",
            "(J)[Ljava/lang/String;",
            flag
        );
        return java_string_array_to_vec(env, string_array);
    }
}

pub fn to_java_open_all_midi_devices(delay: jni_sys::jlong) {
    unsafe {
        let env = attach_jni_env();
        ndk_utils::call_void_method!(env, get_activity(), "openAllMidiDevices", "(J)V", delay);
    }
}

pub unsafe fn to_java_prepare_video_playback(
    env: *mut jni_sys::JNIEnv,
    video_id: LiveId,
    source: VideoSource,
    external_texture_handle: u32,
    autoplay: bool,
    should_loop: bool,
) {
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
        }
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

pub unsafe fn to_java_update_tex_image(
    env: *mut jni_sys::JNIEnv,
    video_decoder_ref: jni_sys::jobject,
) -> bool {
    let class = (**env).GetObjectClass.unwrap()(env, video_decoder_ref);
    let update_tex_image_cstring = CString::new("maybeUpdateTexImage").unwrap();
    let signature_cstring = CString::new("()Z").unwrap();
    let mid_update_tex_image = (**env).GetMethodID.unwrap()(
        env,
        class,
        update_tex_image_cstring.as_ptr(),
        signature_cstring.as_ptr(),
    );

    let updated = (**env).CallBooleanMethod.unwrap()(env, video_decoder_ref, mid_update_tex_image);
    (**env).DeleteLocalRef.unwrap()(env, class);

    updated != 0
}

pub unsafe fn to_java_begin_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(env, get_activity(), "beginVideoPlayback", "(J)V", video_id);
}

pub unsafe fn to_java_pause_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(env, get_activity(), "pauseVideoPlayback", "(J)V", video_id);
}

pub unsafe fn to_java_resume_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(env, get_activity(), "resumeVideoPlayback", "(J)V", video_id);
}

pub unsafe fn to_java_mute_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(env, get_activity(), "muteVideoPlayback", "(J)V", video_id);
}

pub unsafe fn to_java_unmute_video_playback(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(env, get_activity(), "unmuteVideoPlayback", "(J)V", video_id);
}

pub unsafe fn to_java_seek_video_playback(
    env: *mut jni_sys::JNIEnv,
    video_id: LiveId,
    position_ms: u64,
) {
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "seekVideoPlayback",
        "(JJ)V",
        video_id,
        position_ms as jni_sys::jlong
    );
}

pub unsafe fn to_java_get_video_position(env: *mut jni_sys::JNIEnv, video_id: LiveId) -> i64 {
    ndk_utils::call_long_method!(
        env,
        get_activity(),
        "getVideoPlaybackPosition",
        "(J)J",
        video_id
    )
}

pub unsafe fn to_java_cleanup_video_playback_resources(
    env: *mut jni_sys::JNIEnv,
    video_id: LiveId,
) {
    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "cleanupVideoPlaybackResources",
        "(J)V",
        video_id
    );
}

pub unsafe fn to_java_cleanup_video_decoder_ref(
    env: *mut jni_sys::JNIEnv,
    video_decoder_ref: jni_sys::jobject,
) {
    (**env).DeleteGlobalRef.unwrap()(env, video_decoder_ref);
}

pub unsafe fn to_java_check_permission(permission: &str) -> i32 {
    let env = attach_jni_env();
    let permission_str = CString::new(permission).unwrap();
    let permission_jstr = ((**env).NewStringUTF.unwrap())(env, permission_str.as_ptr());

    let result = ndk_utils::call_int_method!(
        env,
        get_activity(),
        "checkPermission",
        "(Ljava/lang/String;)I",
        permission_jstr
    );

    (**env).DeleteLocalRef.unwrap()(env, permission_jstr);
    result
}

pub unsafe fn to_java_request_permission(permission: &str, request_id: i32) {
    let env = attach_jni_env();
    let permission_str = CString::new(permission).unwrap();
    let permission_jstr = ((**env).NewStringUTF.unwrap())(env, permission_str.as_ptr());

    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "requestPermission",
        "(Ljava/lang/String;I)V",
        permission_jstr,
        request_id as jni_sys::jint
    );

    (**env).DeleteLocalRef.unwrap()(env, permission_jstr);
}

/// Configure keyboard/IME settings before showing the keyboard
pub unsafe fn to_java_configure_keyboard(config: &TextInputConfig) {
    let env = attach_jni_env();

    let input_mode = match config.soft_keyboard.input_mode {
        InputMode::Text => 0,
        InputMode::Ascii => 1,
        InputMode::Url => 2,
        InputMode::Numeric => 3,
        InputMode::Tel => 4,
        InputMode::Email => 5,
        InputMode::Decimal => 6,
        InputMode::Search => 7,
    };

    let autocapitalize = match config.soft_keyboard.autocapitalize {
        AutoCapitalize::None => 0,
        AutoCapitalize::Words => 1,
        AutoCapitalize::Sentences => 2,
        AutoCapitalize::AllCharacters => 3,
    };

    let autocorrect = match config.soft_keyboard.autocorrect {
        AutoCorrect::Default => 0,
        AutoCorrect::Enabled => 1,
        AutoCorrect::Disabled => 2,
    };

    let return_key_type = match config.soft_keyboard.return_key_type {
        ReturnKeyType::Default => 0,
        ReturnKeyType::Go => 1,
        ReturnKeyType::Search => 2,
        ReturnKeyType::Send => 3,
        ReturnKeyType::Done => 5,
    };

    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "configureKeyboard",
        "(IIIIZZ)V",
        input_mode as jni_sys::jint,
        autocapitalize as jni_sys::jint,
        autocorrect as jni_sys::jint,
        return_key_type as jni_sys::jint,
        config.is_multiline as jni_sys::jboolean as c_uint,
        config.is_secure as jni_sys::jboolean as c_uint
    );
}

/// Update IME text state for programmatic changes (Rust→Java)
pub unsafe fn to_java_update_ime_text_state(
    full_text: &str,
    selection_start: i32,
    selection_end: i32,
) {
    let env = attach_jni_env();
    let text_cstr = CString::new(full_text).unwrap();
    let text_jstr = ((**env).NewStringUTF.unwrap())(env, text_cstr.as_ptr());

    ndk_utils::call_void_method!(
        env,
        get_activity(),
        "updateImeTextState",
        "(Ljava/lang/String;II)V",
        text_jstr,
        selection_start as jni_sys::jint,
        selection_end as jni_sys::jint
    );

    (**env).DeleteLocalRef.unwrap()(env, text_jstr);
}
