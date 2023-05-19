#![allow(dead_code)]

use {
    self::super::{
        jni_sys::{jclass, jsize, jint, jlong, jstring, jfloat, jobject, JNIEnv, JNI_ABORT},
    },
    crate::{
        area::Area,
        makepad_math::*,
        event::*,
        cx::{Cx, AndroidParams},
        makepad_error_log::*,
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
            let class = ((**self.env).GetObjectClass.unwrap())(self.env, self.callback);
            let name = CString::new("requestHttp").unwrap();
            let signature = CString::new("(Lyour/package/HttpRequest;)V").unwrap();
            let method_id = ((**self.env).GetMethodID.unwrap())(
                self.env,
                class,
                name.as_ptr(),
                signature.as_ptr(),
            );
    
            // TODO: Move part of this to a generic method for converting objects

            // Convert Rust HttpRequest to Java HttpRequest
            // Convert method
            let url = CString::new(request.url.clone()).unwrap();
            let method = CString::new(request.request_method.clone()).unwrap();
    
            // Convert headers from HashMap to Map<String, List<String>>
            let mut headers = ((**self.env).NewHashMap.unwrap())(self.env);
            for (key, values) in request.headers.iter() {
                let key_str = CString::new(key.clone()).unwrap();
                let list = ((**self.env).NewArrayList.unwrap())(self.env, values.len() as i32);
    
                for value in values.iter() {
                    let value_str = CString::new(value.clone()).unwrap();
                    let java_value = ((**self.env).NewStringUTF.unwrap())(self.env, value_str.as_ptr());
                    let add_method_id = ((**self.env).CallBooleanMethodID.unwrap())(
                        self.env,
                        list,
                        add_method_name.as_ptr(),
                        value_signature.as_ptr(),
                        java_value,
                    );
                }
    
                let java_key = ((**self.env).NewStringUTF.unwrap())(self.env, key_str.as_ptr());
                ((**self.env).CallObjectMethod.unwrap())(
                    self.env,
                    headers,
                    put_method_id,
                    java_key,
                    list,
                );
            }
    
            // TODO: this is a byte array not a sting
            // Convert body
            let request_body = CString::new(request.request_body.clone()).unwrap();
            let java_request_body = ((**self.env).NewStringUTF.unwrap())(self.env, request_body.as_ptr());
    
            // Create the Java HttpRequest object
            let java_request = ((**self.env).AllocObject.unwrap())(self.env, request_class);
            ((**self.env).SetObjectField.unwrap())(
                self.env,
                java_request,
                url_field_id,
                url.into_raw() as jobject,
            );
            ((**self.env).SetObjectField.unwrap())(
                self.env,
                java_request,
                method_field_id,
                method.into_raw() as jobject,
            );
            ((**self.env).SetObjectField.unwrap())(
                self.env,
                java_request,
                headers_field_id,
                headers.into_raw() as jobject,
            );
            ((**self.env).SetObjectField.unwrap())(
                self.env,
                java_request,
                body_field_id,
                java_request_body.into_raw() as jobject,
            );
    
            // Call the Java method with the Java HttpRequest object
            ((**self.env).CallVoidMethod.unwrap())(
                self.env,
                self.callback,
                method_id,
                java_request,
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

    let characters:Option<String> = unsafe {
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
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_copyToClipboard(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_copy_to_clipboard(
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_pasteFromClipboard(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    content: jstring,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_paste_from_clipboard(
        jstring_to_string(env, content),
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}

#[no_mangle]
pub unsafe extern "C" fn Java_dev_makepad_android_Makepad_cutToClipboard(
    env: *mut JNIEnv,
    _: jclass,
    cx: jlong,
    callback: jobject,
) {
    (*(cx as *mut Cx)).from_java_cut_to_clipboard(
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
    response: jobject,
    callback: jobject,
) {
    let rust_response = convert_response(env, content);
    
    (*(cx as *mut Cx)).from_java_on_http_response(
        rust_response,
        AndroidToJava {
            env,
            callback,
            phantom: PhantomData,
        },
    );
}


unsafe fn java_http_response_to_rust(env: *mut JNIEnv, response: jobject) -> HttpResponse {
    let class = (**env).GetObjectClass.unwrap()(env, response);
    
    let status_code_field_id = (**env).GetFieldID.unwrap()(env, class, "statusCode", "I");
    let status_code = (**env).GetIntField.unwrap()(env, response, status_code_field_id) as u16;
    
    let headers_field_id = (**env).GetFieldID.unwrap()(env, class, "headers", "Ljava/util/Map;");
    let headers = convert_headers(env, response, headers_field_id);
    
    let body_field_id = (**env).GetFieldID.unwrap()(env, class, "body", "[B");
    let body = convert_body(env, response, body_field_id);
    
    HttpResponse {
        status_code,
        headers,
        body,
    }
}

unsafe fn convert_headers(env: *mut JNIEnv, response: jobject, headers_field_id: jfieldID) -> Vec<(String, String)> {
    // TOODO: move part of this logic to a generic function that does Java Map -> Rust HashMap conversion
    let headers_map = (**env).GetObjectField.unwrap()(env, response, headers_field_id);
    
    let entry_set_method_id = (**env).GetMethodID.unwrap()(env, (**env).FindClass.unwrap()(env, "java/util/Map"), "entrySet", "()Ljava/util/Set;");
    let entry_set = (**env).CallObjectMethod.unwrap()(env, headers_map, entry_set_method_id);
    
    let iterator_method_id = (**env).GetMethodID.unwrap()(env, (**env).FindClass.unwrap()(env, "java/util/Set"), "iterator", "()Ljava/util/Iterator;");
    let iterator = (**env).CallObjectMethod.unwrap()(env, entry_set, iterator_method_id);
    
    let has_next_method_id = (**env).GetMethodID.unwrap()(env, (**env).FindClass.unwrap()(env, "java/util/Iterator"), "hasNext", "()Z");
    let next_method_id = (**env).GetMethodID.unwrap()(env, (**env).FindClass.unwrap()(env, "java/util/Iterator"), "next", "()Ljava/lang/Object;");
    
    let mut headers = Vec::new();
    
    while (**env).CallBooleanMethod.unwrap()(env, iterator, has_next_method_id) == JNI_TRUE {
        let entry = (**env).CallObjectMethod.unwrap()(env, iterator, next_method_id);
        let entry_class = (**env).GetObjectClass.unwrap()(env, entry);
        
        let get_key_method_id = (**env).GetMethodID.unwrap()(env, entry_class, "getKey", "()Ljava/lang/Object;");
        let get_value_method_id = (**env).GetMethodID.unwrap()(env, entry_class, "getValue", "()Ljava/lang/Object;");
        
        let key = (**env).CallObjectMethod.unwrap()(env, entry, get_key_method_id);
        let value = (**env).CallObjectMethod.unwrap()(env, entry, get_value_method_id);
        
        let key_string = jstring_to_string(env, key.into());
        let value_string = jstring_to_string(env, value.into());
        
        headers.push((key_string, value_string));
    }
    
    headers
}