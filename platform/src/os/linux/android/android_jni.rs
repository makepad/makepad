use {
    crate::makepad_math::*,
    std::ffi::CString,
    std::{cell::RefCell, cell::Cell, rc::Rc, sync::mpsc},
    self::super::{
        ndk_sys,
        jni_sys,
        ndk_utils,
    },
    crate::{
        makepad_live_id::*,
        area::Area,
        event::{TouchPoint, TouchState, HttpRequest},
        cx::AndroidParams,
    },
};

#[no_mangle]
pub unsafe extern "C" fn JNI_OnLoad(
    vm: *mut jni_sys::JavaVM,
    _: std::ffi::c_void,
) -> jni_sys::jint {
    VM = vm as *mut _ as _;

    jni_sys::JNI_VERSION_1_6 as _
}

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
    SurfaceChanged {
        window: *mut ndk_sys::ANativeWindow,
        width: i32,
        height: i32,
    },
    SurfaceCreated {
        window: *mut ndk_sys::ANativeWindow,
    },
    SurfaceDestroyed,
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
    MidiDeviceOpened{
        name: String,
        midi_device: jni_sys::jobject
    },
    VideoDecodingInitialized {
        video_id: u64,
        frame_rate: usize,
        video_width: u32,
        video_height: u32,
        color_format: String,
        duration: u128,
    },
    VideoStream {
        video_id: u64,
        frames_group: Vec<u8>,
    },
    VideoChunkDecoded {
        video_id: u64,
    },
    VideoDecodingError {
        video_id: u64,
        error: String,
    },
    Pause,
    Resume,
    Stop,
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

// Defined in https://developer.android.com/reference/android/view/KeyEvent#META_CTRL_MASK
pub const ANDROID_META_CTRL_MASK: u32 = 28672;
// Defined in  https://developer.android.com/reference/android/view/KeyEvent#META_SHIFT_MASK
pub const ANDROID_META_SHIFT_MASK: u32 = 193;
// Defined in  https://developer.android.com/reference/android/view/KeyEvent#META_ALT_MASK
pub const ANDROID_META_ALT_MASK: u32 = 50;

static mut ACTIVITY: jni_sys::jobject = std::ptr::null_mut();
static mut VM: *mut jni_sys::JavaVM = std::ptr::null_mut();

pub unsafe fn jni_init_globals(activity:*const std::ffi::c_void, from_java_tx: mpsc::Sender<FromJavaMessage>){
    let env = attach_jni_env();
    ACTIVITY = (**env).NewGlobalRef.unwrap()(env, activity as jni_sys::jobject);
    MESSAGES_TX.with(move |messages_tx| *messages_tx.borrow_mut() = Some(from_java_tx));
}

pub unsafe fn attach_jni_env() -> *mut jni_sys::JNIEnv {
    let mut env: *mut jni_sys::JNIEnv = std::ptr::null_mut();
    let attach_current_thread = (**VM).AttachCurrentThread.unwrap();

    let res = attach_current_thread(VM, &mut env, std::ptr::null_mut());
    assert!(res == 0);

    env
}

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

#[no_mangle] 
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onAndroidParams(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    cache_path: jni_sys::jstring,
    density: jni_sys::jfloat,
) {
    send_from_java_message(FromJavaMessage::Init(AndroidParams {
        cache_path: jstring_to_string(env, cache_path),
        density: density as f64,
    }));
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
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_surfaceOnTouch(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jclass,
    event: jni_sys::jobject,
) {
    let action_masked = unsafe {ndk_utils::call_int_method!(env, event, "getActionMasked", "()I")};
    let action_index = unsafe {ndk_utils::call_int_method!(env, event, "getActionIndex", "()I")};
    let touch_count = unsafe {ndk_utils::call_int_method!(env, event, "getPointerCount", "()I")};
    
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
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onVideoDecodingInitialized(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    video_id: jni_sys::jlong,
    frame_rate: jni_sys::jint,
    video_width: jni_sys::jint,
    video_height: jni_sys::jint,
    color_format: jni_sys::jstring,
    duration: jni_sys::jlong,
) {
    let color_format = unsafe { jstring_to_string(env, color_format) };

    send_from_java_message(FromJavaMessage::VideoDecodingInitialized {
        video_id: video_id as u64,
        frame_rate: frame_rate as usize,
        video_width: video_width as u32,
        video_height: video_height as u32,
        color_format,
        duration: duration as u128,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onVideoStream(
    env: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    video_id: jni_sys::jlong,
    frames_group: jni_sys::jobject,
) {
    let frames_group = unsafe { java_byte_buffer_to_vec(env, frames_group) };

    send_from_java_message(FromJavaMessage::VideoStream {
        video_id: video_id as u64,
        frames_group,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_MakepadNative_onVideoChunkDecoded(
    _: *mut jni_sys::JNIEnv,
    _: jni_sys::jobject,
    video_id: jni_sys::jlong,
) {
    send_from_java_message(FromJavaMessage::VideoChunkDecoded {
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

// TODO: cache method ids
unsafe fn java_byte_buffer_to_vec(env: *mut jni_sys::JNIEnv, byte_buffer: jni_sys::jobject) -> Vec<u8> {
    let byte_buffer_class = (**env).GetObjectClass.unwrap()(env, byte_buffer);
    let position_method_id = (**env).GetMethodID.unwrap()(env, byte_buffer_class, c_str("position"), c_str("()I"));
    let remaining_method_id = (**env).GetMethodID.unwrap()(env, byte_buffer_class, c_str("remaining"), c_str("()I"));
    let position = (**env).CallIntMethod.unwrap()(env, byte_buffer, position_method_id) as isize;
    let remaining = (**env).CallIntMethod.unwrap()(env, byte_buffer, remaining_method_id) as usize;

    let direct_buffer_ptr = (**env).GetDirectBufferAddress.unwrap()(env, byte_buffer) as *const u8;

    let slice = std::slice::from_raw_parts(direct_buffer_ptr.offset(position), remaining);

    let out_bytes = slice.to_vec();

    out_bytes
}

fn c_str(s: &str) -> *const u8 {
    // into_raw() to prevent CString from being deallocated
    CString::new(s).unwrap().into_raw()
}


pub unsafe fn to_java_set_full_screen(env: *mut jni_sys::JNIEnv, fullscreen: bool) {
    ndk_utils::call_void_method!(env, ACTIVITY, "setFullScreen", "(Z)V", fullscreen as i32);
}

pub(crate) unsafe fn to_java_load_asset(filepath: &str)->Option<Vec<u8>> {
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
    ndk_utils::call_void_method!(env, ACTIVITY, "showKeyboard", "(Z)V", visible as i32);
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
        ACTIVITY,
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

pub fn to_java_get_audio_devices(flag: jni_sys::jlong) -> Vec<String> {
    unsafe {
        let env = attach_jni_env();
        let string_array = ndk_utils::call_object_method!(env, ACTIVITY, "getAudioDevices", "(J)[Ljava/lang/String;", flag);
        return java_string_array_to_vec(env, string_array);
    }
}
  
pub fn to_java_open_all_midi_devices(delay: jni_sys::jlong) {
    unsafe {
        let env = attach_jni_env();
        ndk_utils::call_void_method!(env, ACTIVITY, "openAllMidiDevices", "(J)V", delay);
    }  
}

pub unsafe fn to_java_initialize_video_decoding(env: *mut jni_sys::JNIEnv, video_id: LiveId, video: Rc<Vec<u8>>) {
    let video_data = &*video;

    let java_body = (**env).NewByteArray.unwrap()(env, video_data.len() as i32);
    (**env).SetByteArrayRegion.unwrap()(
        env,
        java_body,
        0,
        video_data.len() as i32,
        video_data.as_ptr() as *const jni_sys::jbyte,
    );

    ndk_utils::call_void_method!(
        env,
        ACTIVITY,
        "initializeVideoDecoding",
        "(J[B)V",
        video_id.get_value() as jni_sys::jlong,
        java_body as jni_sys::jobject
    );
}

pub unsafe fn to_java_decode_next_video_chunk(env: *mut jni_sys::JNIEnv, video_id: LiveId, max_frames_to_decode: usize) {
    ndk_utils::call_void_method!(
        env,
        ACTIVITY,
        "decodeNextVideoChunk",
        "(JI)V",
        video_id.get_value() as jni_sys::jlong,
        max_frames_to_decode as jni_sys::jint
    );
}

pub unsafe fn to_java_fetch_next_video_frames(env: *mut jni_sys::JNIEnv, video_id: LiveId, number_frames: usize) {
    ndk_utils::call_void_method!(
        env,
        ACTIVITY,
        "fetchNextVideoFrames",
        "(JI)V",
        video_id.get_value() as jni_sys::jlong,
        number_frames as jni_sys::jint
    );
}

pub unsafe fn to_java_cleanup_video_decoding(env: *mut jni_sys::JNIEnv, video_id: LiveId) {
    ndk_utils::call_void_method!(
        env,
        ACTIVITY,
        "cleanupVideoDecoding",
        "(J)V",
        video_id
    );
}    
