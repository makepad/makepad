#![allow(dead_code)]

use {
    std::rc::Rc,
    self::super::{
        jni_sys::{jclass, jsize, jint, jbyte, jlong, jstring, jfloat, jobject, jboolean, JNIEnv, JNI_ABORT},
    },
    crate::{
        area::Area,
        makepad_math::*,
        event::*,
        cx::{Cx, AndroidParams},
        network::*,
        makepad_live_id::LiveId
    },
    std::{
        cell::Cell,
        ffi::{CString},
        marker::PhantomData,
    },
};

/// This struct corresponds to the `Makepad.Callback` interface in Java (which is implemented by
/// the `MakepadSurface` class) and enables us to call methods on that interface while hiding as
/// much of the Java native interface from our Rust code as possible.
///
/// The lifetime is necessary here because object pointers in Java are not stable, so the object
/// pointer wrapped by this struct is really only valid for the duration of each native call.
pub struct AndroidToJava<'a> {
    env: *mut JNIEnv,
    callback: jobject,
    phantom: PhantomData<&'a ()>,
}

impl<'a> AndroidToJava<'a> {
    pub fn get_env(&self) -> *mut JNIEnv {
        self.env
    }
    
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
    
    /// Show software keyboard
    pub fn show_text_ime(&self) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("showTextIME").unwrap();
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
    
    /// Hide software keyboard
    pub fn hide_text_ime(&self) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("hideTextIME").unwrap();
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
    
    /// Display clipboard actions menu
    pub fn show_clipboard_actions(&self, selected: &str) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("showClipboardActions").unwrap();
            let signature = CString::new("(Ljava/lang/String;)V").unwrap();
            let selected = CString::new(selected).unwrap();
            let selected = ((**self.env).NewStringUTF.unwrap())(self.env, selected.as_ptr());
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
            ((**self.env).CallVoidMethod.unwrap())(self.env, self.callback, method_id, selected);
        }
    }
    
    pub fn copy_to_clipboard(&self, selected: &str) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("copyToClipboard").unwrap();
            let signature = CString::new("(Ljava/lang/String;)V").unwrap();
            let selected = CString::new(selected).unwrap();
            let selected = ((**self.env).NewStringUTF.unwrap())(self.env, selected.as_ptr());
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
            ((**self.env).CallVoidMethod.unwrap())(self.env, self.callback, method_id, selected);
        }
    }
    
    pub fn paste_from_clipboard(&self) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            
            let name = CString::new("pasteFromClipboard").unwrap();
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
    
    /// reads an asset
    ///
    ///
    pub fn read_asset(&self, file: &str) -> Option<Vec<u8 >> {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            
            let name = CString::new("readAsset").unwrap();
            let signature = CString::new("(Ljava/lang/String;)[B").unwrap();
            let file = CString::new(file).unwrap();
            let file = ((**self.env).NewStringUTF.unwrap())(self.env, file.as_ptr());
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
            let byte_array = ((**self.env).CallObjectMethod.unwrap())(self.env, self.callback, method_id, file);
            if byte_array == std::ptr::null_mut() {
                return None
            }
            else {
                return Some(java_byte_array_to_vec(self.env, byte_array));
            }
        }
    }
    
    pub fn get_audio_devices(&self, flag: jlong) -> Vec<String> {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            
            let name = CString::new("getAudioDevices").unwrap();
            let signature = CString::new("(J)[Ljava/lang/String;").unwrap();
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
            let string_array = ((**self.env).CallObjectMethod.unwrap())(self.env, self.callback, method_id, flag);
            return java_string_array_to_vec(self.env, string_array);
        }
    }
    
    pub fn open_all_midi_devices(&self, delay: jlong) {
        unsafe {
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("openAllMidiDevices").unwrap();
            let signature = CString::new("(J)V").unwrap();
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
            ((**self.env).CallLongMethod.unwrap())(self.env, self.callback, method_id, delay);
        }
    }
    
    pub fn http_request(&self, request: HttpRequest) {
        unsafe {
            let url = CString::new(request.url.clone()).unwrap();
            let url = ((**self.env).NewStringUTF.unwrap())(self.env, url.as_ptr());
            
            let method = CString::new(request.method.to_string()).unwrap();
            let method = ((**self.env).NewStringUTF.unwrap())(self.env, method.as_ptr());
            
            let headers_string = request.get_headers_string();
            let headers = CString::new(headers_string).unwrap();
            let headers = ((**self.env).NewStringUTF.unwrap())(self.env, headers.as_ptr());
            
            let java_body = match &request.body {
                Some(body) => {
                    let java_body = (**self.env).NewByteArray.unwrap()(self.env, body.len() as i32);
                    (**self.env).SetByteArrayRegion.unwrap()(
                        self.env,
                        java_body,
                        0,
                        body.len() as i32,
                        body.as_ptr() as *const jbyte,
                    );
                    java_body
                }
                None => std::ptr::null_mut(),
            };
            
            let name = CString::new("requestHttp").unwrap();
            let signature = CString::new("(JLjava/lang/String;Ljava/lang/String;Ljava/lang/String;[B)V").unwrap();
            let method_id = (**self.env).GetMethodID.unwrap()(
                self.env,
                (**self.env).GetObjectClass.unwrap()(self.env, self.callback),
                name.as_ptr(),
                signature.as_ptr(),
            );
            
            (**self.env).CallVoidMethod.unwrap()(
                self.env,
                self.callback,
                method_id,
                request.id.get_value() as jlong,
                url,
                method,
                headers,
                java_body as jobject,
            );
        }
    }

    pub fn initialize_video_decoding(&self, video_id: LiveId, video: Rc<Vec<u8>>, chunk_size: usize) {
        unsafe {
            let video_data = &*video;
    
            let java_body = (**self.env).NewByteArray.unwrap()(self.env, video_data.len() as i32);
            (**self.env).SetByteArrayRegion.unwrap()(
                self.env,
                java_body,
                0,
                video_data.len() as i32,
                video_data.as_ptr() as *const jbyte,
            );
            
            let name = CString::new("initializeVideoDecoding").unwrap();
            let signature = CString::new("(J[BI)V").unwrap();
            let method_id = (**self.env).GetMethodID.unwrap()(
                self.env,
                (**self.env).GetObjectClass.unwrap()(self.env, self.callback),
                name.as_ptr(),
                signature.as_ptr(),
            );

            (**self.env).CallVoidMethod.unwrap()(
                self.env,
                self.callback,
                method_id,
                video_id.get_value() as jlong,
                java_body as jobject,
                chunk_size,
            );
        }
    }

    pub fn decode_video_chunk(&self, video_id: LiveId, start_timestamp: u128, end_timestamp: u128) {
        unsafe {
            let name = CString::new("decodeVideoChunk").unwrap();
            let signature = CString::new("(JJJ)V").unwrap();
            let method_id = (**self.env).GetMethodID.unwrap()(
                self.env,
                (**self.env).GetObjectClass.unwrap()(self.env, self.callback),
                name.as_ptr(),
                signature.as_ptr(),
            );

            (**self.env).CallVoidMethod.unwrap()(
                self.env,
                self.callback,
                method_id,
                video_id.get_value() as jlong,
                start_timestamp as jlong,
                end_timestamp as jlong,
            );
        }
    }

    pub fn cleanup_decoder(&self, video_id: i64) {
        unsafe {
            let name = CString::new("cleanupDecoder").unwrap();
            let signature = CString::new("(J)V").unwrap();
            let method_id = (**self.env).GetMethodID.unwrap()(
                self.env,
                (**self.env).GetObjectClass.unwrap()(self.env, self.callback),
                name.as_ptr(),
                signature.as_ptr(),
            );
    
            (**self.env).CallVoidMethod.unwrap()(
                self.env,
                self.callback,
                method_id,
                video_id,
            );
        }
    }    
}

// The functions here correspond to the static functions on the `Makepad` class in Java.

// Java_nl_makepad_android_Makepad_newCx is found in main_app.rs

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onDropCx(_: JNIEnv, _: jclass, _cx: jlong) {
    //log!("DROP!");
    //drop(Box::from_raw(app as *mut Cx));
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onPause(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_pause(AndroidToJava {env, callback, phantom: PhantomData});
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onResume(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_resume(AndroidToJava {env, callback, phantom: PhantomData});
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onNewGL(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_new_gl(AndroidToJava {env, callback, phantom: PhantomData});
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onFreeGL(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_free_gl(AndroidToJava {env, callback, phantom: PhantomData});
}

unsafe fn jstring_to_string(env: *mut JNIEnv, java_string: jstring) -> String {
    let chars = (**env).GetStringUTFChars.unwrap()(env, java_string, std::ptr::null_mut());
    let rust_string = std::ffi::CStr::from_ptr(chars).to_str().unwrap().to_string();
    (**env).ReleaseStringUTFChars.unwrap()(env, java_string, chars);
    rust_string
}

unsafe fn java_string_array_to_vec(env: *mut JNIEnv, object_array: jobject) -> Vec<String> {
    if object_array == std::ptr::null_mut() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let length = (**env).GetArrayLength.unwrap()(env, object_array);
    for i in 0..length {
        let string = (**env).GetObjectArrayElement.unwrap()(env, object_array, i as jsize);
        out.push(jstring_to_string(env, string));
    }
    out
}

unsafe fn java_byte_array_to_vec(env: *mut JNIEnv, byte_array: jobject) -> Vec<u8> {
    let bytes = (**env).GetByteArrayElements.unwrap()(env, byte_array, std::ptr::null_mut());
    let length = (**env).GetArrayLength.unwrap()(env, byte_array);
    let mut out_bytes = Vec::new();
    let slice = std::slice::from_raw_parts(bytes as *const u8, length as usize);
    out_bytes.extend_from_slice(slice);
    (**env).ReleaseByteArrayElements.unwrap()(env, byte_array, bytes, JNI_ABORT);
    out_bytes
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onInit(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    cache_path: jstring,
    density: jfloat,
    callback: jobject,
) {
    crate::makepad_error_log::init_panic_hook();
    (*(cx as *mut Cx)).from_java_on_init(
        AndroidParams {
            cache_path: jstring_to_string(env, cache_path),
            density: density as f64,
        },
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        }
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onResize(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    width: jint,
    height: jint,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_resize(
        width,
        height,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onDraw(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_draw(AndroidToJava {
        env,
        callback,
        phantom: PhantomData,
    });
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onKeyDown(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    event: jobject,
    callback: jobject,
) {
    let key_code = unsafe {
        let class = ((**env).GetObjectClass.unwrap())(env, event);
        let name = CString::new("getKeyCode").unwrap();
        let signature = CString::new("()I").unwrap();
        let method_id =
        ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
        ((**env).CallIntMethod.unwrap())(env, event, method_id)
    };
    
    let characters: Option<String> = unsafe {
        let class = ((**env).GetObjectClass.unwrap())(env, event);
        let name = CString::new("getCharacters").unwrap();
        let signature = CString::new("()Ljava/lang/String;").unwrap();
        let method_id =
        ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
        let string_value = ((**env).CallObjectMethod.unwrap())(env, event, method_id);
        
        if string_value == std::ptr::null_mut() {
            None
        } else {
            Some(jstring_to_string(env, string_value))
        }
    };
    
    let meta_state = unsafe {
        let class = ((**env).GetObjectClass.unwrap())(env, event);
        let name = CString::new("getMetaState").unwrap();
        let signature = CString::new("()I").unwrap();
        let method_id =
        ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
        ((**env).CallIntMethod.unwrap())(env, event, method_id)
    };
    
    (*(cx as *mut Cx)).from_java_on_key_down(
        key_code,
        characters,
        meta_state,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onTouch(
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
    
    let touch_count = unsafe {
        let class = ((**env).GetObjectClass.unwrap())(env, event);
        let name = CString::new("getPointerCount").unwrap();
        let signature = CString::new("()I").unwrap();
        let method_id =
        ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
        ((**env).CallIntMethod.unwrap())(env, event, method_id)
    };
    
    let mut touches = Vec::with_capacity(touch_count as usize);
    for touch_index in 0..touch_count {
        let id = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getPointerId").unwrap();
            let signature = CString::new("(I)I").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallIntMethod.unwrap())(env, event, method_id, touch_index)
        };
        
        let x = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getX").unwrap();
            let signature = CString::new("(I)F").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallFloatMethod.unwrap())(env, event, method_id, touch_index)
        };
        
        let y = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getY").unwrap();
            let signature = CString::new("(I)F").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallFloatMethod.unwrap())(env, event, method_id, touch_index)
        };
        
        let rotation_angle = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getOrientation").unwrap();
            let signature = CString::new("(I)F").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallFloatMethod.unwrap())(env, event, method_id, touch_index)
        } as f64;
        
        let force = unsafe {
            let class = ((**env).GetObjectClass.unwrap())(env, event);
            let name = CString::new("getPressure").unwrap();
            let signature = CString::new("(I)F").unwrap();
            let method_id =
            ((**env).GetMethodID.unwrap())(env, class, name.as_ptr(), signature.as_ptr());
            ((**env).CallFloatMethod.unwrap())(env, event, method_id, touch_index)
        } as f64;
        
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
    
    (*(cx as *mut Cx)).from_java_on_touch(
        touches,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onTimeout(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    id: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_timeout(
        id,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onMidiDeviceOpened(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    name: jstring,
    midi_device: jobject,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_midi_device_opened(
        jstring_to_string(env, name),
        midi_device,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onHideTextIME(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_hide_text_ime(
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onResizeTextIME(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    ime_height: jint,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_resize_text_ime(
        ime_height,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onPasteFromClipboard(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    content: jstring,
    callback: jobject,
) {
    let string_content = if content == std::ptr::null_mut() {
        None
    } else {
        Some(jstring_to_string(env, content))
    };
    
    (*(cx as *mut Cx)).from_java_on_paste_from_clipboard(
        string_content,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onCutToClipboard(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_cut_to_clipboard(
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onHttpResponse(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    request_id: jlong,
    status_code: jint,
    headers: jstring,
    body: jobject,
    callback: jobject,
) {
    let headers = jstring_to_string(env, headers);
    let body = java_byte_array_to_vec(env, body);
    
    (*(cx as *mut Cx)).from_java_on_http_response(
        request_id as u64,
        status_code as u16,
        headers,
        body,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onHttpRequestError(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    request_id: jlong,
    exception: jstring,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_http_request_error(
        request_id as u64,
        jstring_to_string(env, exception),
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onVideoDecodingInitialized(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    video_id: jlong,
    frame_rate: jint,
    video_width: jint,
    video_height: jint,
    color_format: jstring,
    duration: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_video_decoding_initialized(
        video_id as u64,
        frame_rate as usize,
        video_width as u32,
        video_height as u32,
        jstring_to_string(env, color_format),
        duration as u128,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onVideoStream(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    video_id: jlong,
    pixel_data: jobject,
    y_stride: jint,
    uv_stride: jint,
    timestamp: jlong,
    is_eoc: jboolean,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_on_video_stream(
        video_id as u64,
        java_byte_array_to_vec(env, pixel_data),
        (y_stride as usize, uv_stride as usize),
        timestamp as u128,
        is_eoc != 0,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_onHookPanic(_: *const std::ffi::c_void, _: *const std::ffi::c_void) {
    pub fn panic_hook(info: &std::panic::PanicInfo) {
        crate::error!("{}", info) 
    }
    std::panic::set_hook(Box::new(panic_hook));
}
