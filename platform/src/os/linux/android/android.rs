use std::cell::Cell;

#[cfg(use_vulkan)]
use self::super::super::vulkan::CxVulkan;
use crate::event::LongPressEvent;
#[allow(unused)]
use makepad_jni_sys as jni_sys;

use {
    self::super::super::{
        gl_sys,
        gl_sys::LibGl,
        //libc_sys,
        openxr::{CxOpenXr, CxOpenXrOptions},
    },
    self::super::{
        super::egl_sys::{self, LibEgl},
        super::libc_sys,
        android_camera_player::AndroidCameraPlayer,
        android_jni::{self, *},
        android_keycodes::android_to_makepad_key_code,
        android_media::CxAndroidMedia,
        android_video_playback::{force_native_video, force_software_video, AndroidVideoConfig},
        ndk_sys,
    },
    crate::{
        cx::{Cx, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_pass::CxDrawPassParent,
        draw_pass::{DrawPassClearColor, DrawPassClearDepth, DrawPassId},
        event::{
            keyboard::{CharOffset, FullTextState, ImeAction, ImeActionEvent},
            video_playback::CameraPreviewMode,
            Event,
            KeyCode,
            KeyEvent,
            KeyModifiers,
            NetworkResponse,
            SelectionHandleDragEvent,
            TextClipboardEvent,
            //TimerEvent,
            TextInputEvent,
            //TouchPoint,
            TouchUpdateEvent,
            VideoDecodingErrorEvent,
            VideoPlaybackCompletedEvent,
            VideoPlaybackPreparedEvent,
            VideoPlaybackResourcesReleasedEvent,
            VideoSource,
            //HttpRequest,
            //HttpMethod,
            VideoTextureUpdatedEvent,
            VideoYuvTexturesReady,
            VirtualKeyboardEvent,
            WindowGeom,
            WindowGeomChangeEvent,
        },
        gpu_info::GpuPerformance,
        makepad_live_id::*,
        makepad_math::*,
        media_api::CxMediaApi,
        os::cx_native::EventFlow,
        os::linux::gl_video_upload::upload_yuv_to_gl,
        shared_framebuf::{PollTimer, PollTimers},
        texture::TextureFormat,
        texture::TextureId,
        //makepad_live_compiler::LiveFileChange,
        thread::SignalToUI,
        video::{VideoEncodeError, MAX_VIDEO_DEVICE_INDEX},
        video_decode::software_video::SoftwareVideoPlayer,
        web_socket::WebSocketMessage,
        //web_socket::WebSocket,
        window::CxWindowPool,
        HttpError,
        HttpResponse,
    },
    jni_sys::jobject,
    makepad_network::{
        ServerWebSocketMessage as WebSocketMessageImpl, WebSocketParser as WebSocketImpl,
    },
    makepad_studio_protocol::{AppToStudio, GPUSample},
    std::cell::RefCell,
    std::collections::HashMap,
    std::ffi::CString,
    std::rc::Rc,
    std::sync::mpsc,
    //std::os::raw::{c_void},
    std::time::Instant,
};

/*
fn android_debug_log(msg:&str){
    use std::ffi::c_int;
    extern "C" {
        pub fn __android_log_write(prio: c_int, tag: *const u8, text: *const u8) -> c_int;
    }
    let msg = format!("{}\0", msg);
    unsafe{__android_log_write(3, "Makepad\0".as_ptr(), msg.as_ptr())};
}*/

impl Cx {
    /// Main event loop for the Android platform.
    /// This method waits for messages from the Java side, particularly the RenderLoop message,
    /// which is sent on Android Choreographer callbacks to sync with vsync.
    /// It handles all incoming messages, processes other events, and manages drawing operations.
    pub fn main_loop(&mut self, from_java_rx: mpsc::Receiver<FromJavaMessage>) {
        self.gpu_info.performance = GpuPerformance::Tier1;

        self.call_event_handler(&Event::Startup);
        self.redraw_all();

        self.start_network_live_file_watcher();

        while !self.os.quit {
            if self.os.in_xr_mode {
                if self.openxr_render_loop(&from_java_rx) {
                    continue;
                };
            }

            // Wait for the next message, blocking until one is received.
            // This ensures we're in sync with the Android Choreographer when we receive a RenderLoop message.
            match from_java_rx.recv() {
                Ok(FromJavaMessage::RenderLoop) => {
                    while let Ok(msg) = from_java_rx.try_recv() {
                        self.handle_message(msg);
                    }
                    self.handle_other_events();
                    self.handle_drawing();
                }
                Ok(message) => {
                    self.handle_message(message);
                    // Dispatch platform ops immediately after non-RenderLoop messages.
                    // This ensures SyncImeState reaches Java before the IME's next buffer query.
                    self.handle_platform_ops();
                }
                Err(e) => {
                    crate::error!("Error receiving message: {:?}", e);
                    break;
                }
            }
        }
        self.os
            .openxr
            .destroy_instance(&self.os.display.as_ref().unwrap().libgl)
            .ok();
        from_java_messages_clear()
    }

    pub(crate) fn handle_message(&mut self, msg: FromJavaMessage) {
        match msg {
            FromJavaMessage::SwitchedActivity(activity_handle, activity_thread_id) => {
                self.os.activity_thread_id = Some(activity_thread_id);
                if self.os.in_xr_mode {
                    if let Err(e) = self.os.openxr.create_instance(activity_handle) {
                        crate::error!("OpenXR init failed: {}", e);
                    }
                }
            }
            FromJavaMessage::RenderLoop => {
                // This should not happen here, as it's handled in the main loop
            }
            FromJavaMessage::BackPressed => {
                self.call_event_handler(&Event::BackPressed {
                    handled: Cell::new(false),
                });
            }
            FromJavaMessage::SurfaceCreated { window } => {
                #[cfg(not(use_vulkan))]
                unsafe {
                    self.os.display.as_mut().unwrap().update_surface(window);
                }

                #[cfg(use_vulkan)]
                {
                    if let Some(display) = self.os.display.as_mut() {
                        unsafe {
                            if !display.window.is_null() {
                                ndk_sys::ANativeWindow_release(display.window);
                            }
                        }
                        display.window = window;
                    }
                    if let Some(vulkan) = self.os.vulkan.as_mut() {
                        let width = self.os.display_size.x.max(1.0) as u32;
                        let height = self.os.display_size.y.max(1.0) as u32;
                        if let Err(err) = vulkan.update_surface(window, width, height) {
                            crate::error!("Android Vulkan surface create/update failed: {err}");
                        }
                    }
                }
            }
            FromJavaMessage::SurfaceDestroyed => {
                #[cfg(not(use_vulkan))]
                unsafe {
                    self.os.display.as_mut().unwrap().destroy_surface();
                }

                #[cfg(use_vulkan)]
                {
                    if let Some(display) = self.os.display.as_mut() {
                        unsafe {
                            if !display.window.is_null() {
                                ndk_sys::ANativeWindow_release(display.window);
                                display.window = std::ptr::null_mut();
                            }
                        }
                    }
                    if let Some(vulkan) = self.os.vulkan.as_mut() {
                        vulkan.suspend_surface();
                    }
                }
            }
            FromJavaMessage::SurfaceChanged {
                window,
                width,
                height,
            } => {
                if self.os.in_xr_mode && self.os.openxr.session.is_none() {
                    if self.os.openxr.libxr.is_none() {
                        let activity_handle = makepad_android_state::get_activity();
                        self.os.openxr.create_instance(activity_handle).ok();
                    }
                    if let Err(e) = self.os.openxr.create_session(
                        self.os.display.as_ref().unwrap(),
                        CxOpenXrOptions {
                            buffer_scale: 1.5,
                            multisamples: 4,
                            remove_hands_from_depth: false,
                        },
                        &self.os_type,
                    ) {
                        crate::error!("OpenXR create_xr_session failed: {}", e);
                    }
                }

                #[cfg(not(use_vulkan))]
                unsafe {
                    self.os.display.as_mut().unwrap().update_surface(window);
                }

                #[cfg(use_vulkan)]
                {
                    if let Some(display) = self.os.display.as_mut() {
                        unsafe {
                            if !display.window.is_null() && display.window != window {
                                ndk_sys::ANativeWindow_release(display.window);
                            }
                        }
                        display.window = window;
                    }
                }

                #[cfg(use_vulkan)]
                {
                    let width_u32 = width.max(1) as u32;
                    let height_u32 = height.max(1) as u32;
                    if let Some(vulkan) = self.os.vulkan.as_mut() {
                        if let Err(err) = vulkan.update_surface(window, width_u32, height_u32) {
                            crate::error!("Android Vulkan surface update failed: {err}");
                        }
                    } else {
                        match CxVulkan::new(window, width_u32, height_u32) {
                            Ok(vulkan) => {
                                crate::log!("Android Vulkan backend initialized");
                                self.os.vulkan = Some(vulkan);
                            }
                            Err(err) => {
                                crate::error!(
                                    "Android Vulkan backend init failed, falling back to OpenGL: {err}"
                                );
                            }
                        }
                    }
                }

                self.os.display_size = dvec2(width as f64, height as f64);
                let window_id = CxWindowPool::id_zero();
                let window = &mut self.windows[window_id];
                let old_geom = window.window_geom.clone();

                let dpi_factor = window.dpi_override.unwrap_or(self.os.dpi_factor);
                let size = self.os.display_size / dpi_factor;
                window.window_geom = WindowGeom {
                    dpi_factor,
                    can_fullscreen: false,
                    xr_is_presenting: false,
                    is_fullscreen: true,
                    is_topmost: true,
                    position: dvec2(0.0, 0.0),
                    inner_size: size,
                    outer_size: size,
                };
                let new_geom = window.window_geom.clone();
                self.call_event_handler(&Event::WindowGeomChange(WindowGeomChangeEvent {
                    window_id,
                    new_geom,
                    old_geom,
                }));
                if let Some(main_pass_id) = self.windows[window_id].main_pass_id {
                    self.redraw_pass_and_child_passes(main_pass_id);
                }
                self.redraw_all();
                self.os.first_after_resize = true;
                self.call_event_handler(&Event::ClearAtlasses);
            }
            FromJavaMessage::LongClick {
                abs,
                pointer_id,
                time,
            } => {
                let window = &mut self.windows[CxWindowPool::id_zero()];
                let dpi_factor = window.dpi_override.unwrap_or(self.os.dpi_factor);
                let e = Event::LongPress(LongPressEvent {
                    abs: abs / dpi_factor,
                    uid: pointer_id,
                    window_id: CxWindowPool::id_zero(),
                    time,
                });
                self.call_event_handler(&e);
            }
            FromJavaMessage::Touch(mut touches) => {
                let time = touches[0].time;
                let window = &mut self.windows[CxWindowPool::id_zero()];
                let dpi_factor = window.dpi_override.unwrap_or(self.os.dpi_factor);
                for touch in &mut touches {
                    touch.abs /= dpi_factor;
                    touch.radius /= dpi_factor;
                }

                // Check for outside-click popup dismiss on touch start
                if touches
                    .iter()
                    .any(|t| t.state == crate::event::finger::TouchState::Start)
                {
                    if let Some(popup_window_id) = self.find_popup_to_dismiss_on_touch(&touches) {
                        self.dismiss_popup_window(
                            popup_window_id,
                            crate::event::PopupDismissReason::OutsideClick,
                        );
                    }
                }

                self.fingers.process_touch_update_start(time, &touches);
                let e = Event::TouchUpdate(TouchUpdateEvent {
                    time,
                    window_id: CxWindowPool::id_zero(),
                    touches,
                    modifiers: Default::default(),
                });
                self.call_event_handler(&e);
                let e = if let Event::TouchUpdate(e) = e {
                    e
                } else {
                    panic!()
                };
                self.fingers.process_touch_update_end(&e.touches);
            }
            FromJavaMessage::Character { character } => {
                if let Some(character) = char::from_u32(character) {
                    let e = Event::TextInput(TextInputEvent {
                        input: character.to_string(),
                        replace_last: false,
                        was_paste: false,
                        ..Default::default()
                    });
                    self.call_event_handler(&e);
                }
            }
            FromJavaMessage::KeyDown {
                keycode,
                meta_state,
            } => {
                let e: Event;
                let makepad_keycode = android_to_makepad_key_code(keycode);
                if !makepad_keycode.is_unknown() {
                    let control = meta_state & ANDROID_META_CTRL_MASK != 0;
                    let alt = meta_state & ANDROID_META_ALT_MASK != 0;
                    let shift = meta_state & ANDROID_META_SHIFT_MASK != 0;
                    let is_shortcut = control || alt;
                    if is_shortcut {
                        if makepad_keycode == KeyCode::KeyC {
                            let response = Rc::new(RefCell::new(None));
                            e = Event::TextCopy(TextClipboardEvent {
                                response: response.clone(),
                            });
                            self.call_event_handler(&e);
                            // let response = response.borrow();
                            // if let Some(response) = response.as_ref(){
                            //     to_java.copy_to_clipboard(response);
                            // }
                        } else if makepad_keycode == KeyCode::KeyX {
                            let response = Rc::new(RefCell::new(None));
                            let e = Event::TextCut(TextClipboardEvent {
                                response: response.clone(),
                            });
                            self.call_event_handler(&e);
                        } else if makepad_keycode == KeyCode::KeyV {
                            let content = unsafe { android_jni::to_java_paste_from_clipboard() };
                            if !content.is_empty() {
                                e = Event::TextInput(TextInputEvent {
                                    input: content,
                                    replace_last: false,
                                    was_paste: true,
                                    ..Default::default()
                                });
                                self.call_event_handler(&e);
                            }
                        }
                    } else {
                        if makepad_keycode == KeyCode::Back {
                            self.call_event_handler(&Event::BackPressed {
                                handled: Cell::new(false),
                            });
                        }

                        e = Event::KeyDown(KeyEvent {
                            key_code: makepad_keycode,
                            is_repeat: false,
                            modifiers: KeyModifiers {
                                shift,
                                control,
                                alt,
                                ..Default::default()
                            },
                            time: self.os.timers.time_now(),
                        });
                        self.call_event_handler(&e);
                    }
                }
            }
            FromJavaMessage::KeyUp {
                keycode,
                meta_state,
            } => {
                let makepad_keycode = android_to_makepad_key_code(keycode);
                let control = meta_state & ANDROID_META_CTRL_MASK != 0;
                let alt = meta_state & ANDROID_META_ALT_MASK != 0;
                let shift = meta_state & ANDROID_META_SHIFT_MASK != 0;

                let e = Event::KeyUp(KeyEvent {
                    key_code: makepad_keycode,
                    is_repeat: false,
                    modifiers: KeyModifiers {
                        shift,
                        control,
                        alt,
                        ..Default::default()
                    },
                    time: self.os.timers.time_now(),
                });
                self.call_event_handler(&e);
            }
            FromJavaMessage::ResizeTextIME {
                keyboard_height,
                is_open,
            } => {
                let keyboard_height = (keyboard_height as f64) / self.os.dpi_factor;
                if !is_open {
                    self.os.keyboard_closed = keyboard_height;
                }
                if is_open {
                    self.call_event_handler(&Event::VirtualKeyboard(
                        VirtualKeyboardEvent::DidShow {
                            height: keyboard_height - self.os.keyboard_closed,
                            time: self.os.timers.time_now(),
                        },
                    ))
                } else {
                    self.text_ime_was_dismissed();
                    self.call_event_handler(&Event::VirtualKeyboard(
                        VirtualKeyboardEvent::DidHide {
                            time: self.os.timers.time_now(),
                        },
                    ))
                }
            }
            FromJavaMessage::HttpResponse {
                request_id,
                metadata_id,
                status_code,
                headers,
                body,
            } => {
                let out = vec![NetworkResponse::HttpResponse {
                    request_id: LiveId(request_id),
                    response: HttpResponse::from_header_string(
                        LiveId(metadata_id),
                        status_code,
                        headers,
                        Some(body),
                    ),
                }];
                self.handle_script_network_events(&out);
                let e = Event::NetworkResponses(out);
                self.call_event_handler(&e);
            }
            FromJavaMessage::HttpRequestError {
                request_id,
                metadata_id,
                error,
                ..
            } => {
                let out = vec![NetworkResponse::HttpError {
                    request_id: LiveId(request_id),
                    error: HttpError {
                        message: error,
                        metadata_id: LiveId(metadata_id),
                    },
                }];
                self.handle_script_network_events(&out);
                let e = Event::NetworkResponses(out);
                self.call_event_handler(&e);
            }
            FromJavaMessage::WebSocketMessage { message, sender } => {
                let ws_message_parser = self
                    .os
                    .websocket_parsers
                    .entry(sender.0)
                    .or_insert_with(|| WebSocketImpl::new());
                ws_message_parser.parse(&message, |result| match result {
                    Ok(WebSocketMessageImpl::Text(text_msg)) => {
                        let message = WebSocketMessage::String(text_msg.to_string());
                        sender.1.send(message).unwrap();
                    }
                    Ok(WebSocketMessageImpl::Binary(data)) => {
                        let message = WebSocketMessage::Binary(data.to_vec());
                        sender.1.send(message).unwrap();
                    }
                    Err(e) => {
                        println!("Websocket message parse error {:?}", e);
                    }
                    _ => (),
                });
            }
            FromJavaMessage::WebSocketClosed { sender } => {
                self.os.websocket_parsers.remove(&sender.0);
                let message = WebSocketMessage::Closed;
                sender.1.send(message).ok();
            }
            FromJavaMessage::WebSocketError { error, sender } => {
                self.os.websocket_parsers.remove(&sender.0);
                let message = WebSocketMessage::Error(error);
                sender.1.send(message).ok();
            }
            FromJavaMessage::MidiDeviceOpened { name, midi_device } => {
                self.os
                    .media
                    .android_midi()
                    .lock()
                    .unwrap()
                    .midi_device_opened(name, midi_device);
            }
            FromJavaMessage::PermissionResult {
                permission,
                request_id,
                status,
            } => {
                // Convert string permission back to enum
                let perm = string_to_permission(&permission);
                if let Some(perm) = perm {
                    let permission_status = match status {
                        0 => crate::permission::PermissionStatus::NotDetermined,
                        1 => crate::permission::PermissionStatus::Granted,
                        2 => crate::permission::PermissionStatus::DeniedCanRetry,
                        3 => crate::permission::PermissionStatus::DeniedPermanent,
                        _ => {
                            crate::log!("Unknown permission status code: {}", status);
                            crate::permission::PermissionStatus::DeniedPermanent
                            // Default to most restrictive
                        }
                    };

                    self.call_event_handler(&Event::PermissionResult(
                        crate::permission::PermissionResult {
                            permission: perm,
                            request_id,
                            status: permission_status,
                        },
                    ));
                }
            }
            FromJavaMessage::VideoPlaybackPrepared {
                video_id,
                video_width,
                video_height,
                duration,
                surface_texture,
            } => {
                let e = Event::VideoPlaybackPrepared(VideoPlaybackPreparedEvent {
                    video_id: LiveId(video_id),
                    video_width,
                    video_height,
                    duration,
                    is_seekable: duration > 0,
                    video_tracks: if video_width > 0 && video_height > 0 {
                        vec!["video".to_string()]
                    } else {
                        vec![]
                    },
                    audio_tracks: vec!["audio".to_string()],
                });

                self.os
                    .video_surfaces
                    .insert(LiveId(video_id), surface_texture);
                self.call_event_handler(&e);
            }
            FromJavaMessage::VideoPlaybackCompleted { video_id } => {
                let e = Event::VideoPlaybackCompleted(VideoPlaybackCompletedEvent {
                    video_id: LiveId(video_id),
                });
                self.call_event_handler(&e);
            }
            FromJavaMessage::VideoPlayerReleased { video_id } => {
                let live_id = LiveId(video_id);
                if let Some(decoder_ref) = self.os.video_surfaces.remove(&live_id) {
                    unsafe {
                        let env = attach_jni_env();
                        android_jni::to_java_cleanup_video_decoder_ref(env, decoder_ref);
                    }
                }
                if let Some(mut asp) = self.os.software_video_players.remove(&live_id) {
                    asp.player.cleanup();
                }
                self.os.video_configs.remove(&live_id);

                let e =
                    Event::VideoPlaybackResourcesReleased(VideoPlaybackResourcesReleasedEvent {
                        video_id: live_id,
                    });
                self.call_event_handler(&e);
            }
            FromJavaMessage::VideoDecodingError { video_id, error } => {
                let live_id = LiveId(video_id);
                let force_native = force_native_video();
                if !force_native && !self.os.software_video_players.contains_key(&live_id) {
                    if let Some(config) = self.os.video_configs.get(&live_id).cloned() {
                        crate::log!(
                            "VIDEO: Android native decode failed for {}, falling back to software video: {}",
                            live_id.0,
                            error
                        );
                        let asp = AndroidSoftwarePlayer {
                            player: SoftwareVideoPlayer::new(
                                live_id,
                                config.texture_id,
                                config.source,
                                config.autoplay,
                                config.should_loop,
                            ),
                            tex_y_id: config.tex_y_id,
                            tex_u_id: config.tex_u_id,
                            tex_v_id: config.tex_v_id,
                            yuv_matrix: 0.0,
                        };
                        self.os.software_video_players.insert(live_id, asp);
                        self.redraw_all();
                        return;
                    }
                }

                let e = Event::VideoDecodingError(VideoDecodingErrorEvent {
                    video_id: live_id,
                    error,
                });
                self.call_event_handler(&e);
            }
            FromJavaMessage::CameraPreviewSurfaceReady {
                video_id,
                window,
                width: _,
                height: _,
            } => {
                let live_id = LiveId(video_id);
                if let Some(player) = self.os.camera_players.get_mut(&live_id) {
                    player.set_preview_window(Some(window));
                } else {
                    if let Some(old) = self
                        .os
                        .pending_camera_preview_windows
                        .insert(live_id, window)
                    {
                        unsafe {
                            ndk_sys::ANativeWindow_release(old);
                        }
                    }
                }
            }
            FromJavaMessage::CameraPreviewSurfaceDestroyed { video_id } => {
                let live_id = LiveId(video_id);
                if let Some(player) = self.os.camera_players.get_mut(&live_id) {
                    player.set_preview_window(None);
                }
                if let Some(window) = self.os.pending_camera_preview_windows.remove(&live_id) {
                    unsafe {
                        ndk_sys::ANativeWindow_release(window);
                    }
                }
            }
            FromJavaMessage::Pause => {
                self.call_event_handler(&Event::Pause);
            }
            FromJavaMessage::Resume => {
                if self.os.fullscreen {
                    unsafe {
                        let env = attach_jni_env();
                        android_jni::to_java_set_full_screen(env, true);
                    }
                }
                self.redraw_all();
                self.reinitialise_media();
                self.call_event_handler(&Event::Resume);
            }

            FromJavaMessage::Start => {
                self.call_event_handler(&Event::Foreground);
            }
            FromJavaMessage::Stop => {
                self.call_event_handler(&Event::Background);
            }
            FromJavaMessage::Destroy => {
                if !self.os.ignore_destroy {
                    self.call_event_handler(&Event::Shutdown);
                    self.os.quit = true;
                }

                self.os.ignore_destroy = false;
            }
            FromJavaMessage::WindowFocusChanged { has_focus } => {
                let window_id = CxWindowPool::id_zero();
                if has_focus {
                    self.call_event_handler(&Event::WindowGotFocus(window_id));
                } else {
                    self.call_event_handler(&Event::WindowLostFocus(window_id));
                }
            }
            FromJavaMessage::ClipboardAction { action } => {
                if action == "copy" {
                    let response = Rc::new(RefCell::new(None));
                    let e = Event::TextCopy(TextClipboardEvent {
                        response: response.clone(),
                    });
                    self.call_event_handler(&e);
                    // Get the copied text from the widget's response
                    if let Some(text) = response.borrow().as_ref() {
                        // Copy to clipboard
                        unsafe {
                            to_java_copy_to_clipboard(text.clone());
                        }
                    };
                } else if action == "cut" {
                    let response = Rc::new(RefCell::new(None));
                    let e = Event::TextCut(TextClipboardEvent {
                        response: response.clone(),
                    });
                    self.call_event_handler(&e);
                    // Get the cut text from the widget's response
                    if let Some(text) = response.borrow().as_ref() {
                        // Copy to clipboard
                        unsafe {
                            to_java_copy_to_clipboard(text.clone());
                        }
                    };
                } else if action == "select_all" {
                    // Simulate Ctrl+A keypress to trigger select_all in widgets
                    let e = Event::KeyDown(KeyEvent {
                        key_code: KeyCode::KeyA,
                        is_repeat: false,
                        modifiers: KeyModifiers {
                            shift: false,
                            control: true, // Ctrl modifier
                            alt: false,
                            logo: false,
                        },
                        time: self.seconds_since_app_start(),
                    });
                    self.call_event_handler(&e);
                }
            }
            FromJavaMessage::ClipboardPaste { content } => {
                let e = Event::TextInput(TextInputEvent {
                    input: content,
                    replace_last: false,
                    was_paste: true,
                    ..Default::default()
                });
                self.call_event_handler(&e);
            }
            FromJavaMessage::SelectionHandleDrag {
                handle,
                phase,
                abs,
                time,
            } => {
                let window = &self.windows[CxWindowPool::id_zero()];
                let dpi_factor = window.dpi_override.unwrap_or(self.os.dpi_factor);
                let e = Event::SelectionHandleDrag(SelectionHandleDragEvent {
                    handle,
                    phase,
                    abs: abs / dpi_factor,
                    time,
                });
                self.call_event_handler(&e);
            }
            FromJavaMessage::ImeTextStateChanged {
                full_text,
                selection_start,
                selection_end,
                composing_start,
                composing_end,
            } => {
                let sel_start = CharOffset::from_utf16_index(&full_text, selection_start as usize);
                let sel_end = CharOffset::from_utf16_index(&full_text, selection_end as usize);

                let composition = if composing_start >= 0 && composing_end >= 0 {
                    let comp_start =
                        CharOffset::from_utf16_index(&full_text, composing_start as usize);
                    let comp_end = CharOffset::from_utf16_index(&full_text, composing_end as usize);
                    Some(comp_start..comp_end)
                } else {
                    None
                };

                let e = Event::TextInput(TextInputEvent {
                    full_state_sync: Some(FullTextState {
                        text: full_text,
                        selection: sel_start..sel_end,
                        composition,
                    }),
                    ..Default::default()
                });
                self.call_event_handler(&e);
            }
            FromJavaMessage::ImeEditorAction { action_code } => {
                let action = ImeAction::from_android_action_code(action_code);
                let e = Event::ImeAction(ImeActionEvent { action });
                self.call_event_handler(&e);
            }
            FromJavaMessage::Init(_) => {}
        }
    }

    pub(crate) fn handle_drawing(&mut self) {
        if self.any_passes_dirty()
            || self.need_redrawing()
            || !self.new_next_frames.is_empty()
            || self.demo_time_repaint
        {
            let time_now = self.os.timers.time_now();
            if !self.new_next_frames.is_empty() {
                self.call_next_frame_event(time_now);
            }
            if self.need_redrawing() {
                self.call_draw_event(time_now);
                self.compile_shaders_for_active_backend();
            }

            if self.os.first_after_resize {
                self.os.first_after_resize = false;
                self.redraw_all();
            }

            self.handle_repaint();
        }
    }

    fn compile_shaders_for_active_backend(&mut self) {
        #[cfg(use_vulkan)]
        {
            // Vulkan mode is currently a staged path:
            // run WGSL->SPIR-V compilation via the OpenGL shader compile entry point.
            self.opengl_compile_shaders();
            return;
        }

        #[cfg(not(use_vulkan))]
        {
            self.opengl_compile_shaders();
        }
    }

    fn draw_pass_to_window_for_active_backend(&mut self, draw_pass_id: DrawPassId) {
        #[cfg(use_vulkan)]
        {
            if self.os.vulkan.is_some() {
                let mut vulkan = self.os.vulkan.take().unwrap();
                let result = vulkan.draw_pass_and_present(self, draw_pass_id);
                self.os.vulkan = Some(vulkan);
                if let Err(err) = result {
                    crate::error!("Android Vulkan draw/present failed: {err}");
                }
            } else {
                self.draw_pass_to_fullscreen(draw_pass_id);
            }
            return;
        }

        #[cfg(not(use_vulkan))]
        {
            self.draw_pass_to_fullscreen(draw_pass_id);
        }
    }

    fn present_window_for_active_backend(&mut self) {
        #[cfg(use_vulkan)]
        {
            // Vulkan path presents in draw_pass_to_window_for_active_backend.
            // If Vulkan failed to initialize, keep the OpenGL fallback functional.
            if self.os.vulkan.is_none() {
                unsafe {
                    if let Some(display) = &mut self.os.display {
                        (display.libegl.eglSwapBuffers.unwrap())(
                            display.egl_display,
                            display.surface,
                        );
                    }
                }
            }
            return;
        }

        #[cfg(not(use_vulkan))]
        unsafe {
            if let Some(display) = &mut self.os.display {
                (display.libegl.eglSwapBuffers.unwrap())(display.egl_display, display.surface);
            }
        }
    }

    /// Processes events that need to be checked regularly, regardless of incoming messages.
    /// This includes timers, signals, video updates, live edits, and platform operations.
    pub(crate) fn handle_other_events(&mut self) {
        // Timers
        let events = self.os.timers.get_dispatch();
        for event in events {
            self.handle_script_timer(&event);
            self.call_event_handler(&Event::Timer(event));
        }

        // Signals
        if SignalToUI::check_and_clear_ui_signal() {
            self.handle_media_signals();
            self.handle_script_signals();
            self.call_event_handler(&Event::Signal);
        }
        if SignalToUI::check_and_clear_action_signal() {
            self.handle_action_receiver();
        }

        self.dispatch_network_runtime_events();

        // Native video updates (SurfaceTexture path)
        let to_dispatch = self.get_video_updates();
        for video_id in to_dispatch {
            let current_position_ms = unsafe {
                let env = attach_jni_env();
                android_jni::to_java_get_video_position(env, video_id) as u128
            };
            let e = Event::VideoTextureUpdated(VideoTextureUpdatedEvent {
                video_id,
                current_position_ms,
                yuv: crate::event::video_playback::VideoYuvMetadata {
                    enabled: false,
                    matrix: 0.0,
                    biplanar: false,
                    rotation_steps: 0.0,
                },
            });
            self.call_event_handler(&e);
        }

        // Camera player updates
        self.poll_camera_players();

        // Software AV1 fallback updates (rav1d path)
        self.poll_software_video_players();

        // Live edits
        self.run_live_edit_if_needed("android");

        // Platform operations
        self.handle_platform_ops();
    }

    fn get_video_updates(&mut self) -> Vec<LiveId> {
        let mut videos_to_update = Vec::new();
        for (live_id, surface_texture) in self.os.video_surfaces.iter_mut() {
            unsafe {
                let env = attach_jni_env();
                let updated = android_jni::to_java_update_tex_image(env, *surface_texture);
                if updated {
                    videos_to_update.push(*live_id);
                }
            }
        }
        videos_to_update
    }

    fn poll_camera_players(&mut self) {
        if self.os.camera_players.is_empty() {
            return;
        }

        let mut players = std::mem::take(&mut self.os.camera_players);
        let has_texture_players = players.values().any(AndroidCameraPlayer::uses_textures);
        let gl = if has_texture_players {
            Some(self.os.gl() as *const LibGl)
        } else {
            None
        };
        let mut events = Vec::new();

        for (_video_id, player) in players.iter_mut() {
            match player.check_prepared() {
                Some(Ok((width, height, duration, is_seekable, video_tracks, audio_tracks))) => {
                    events.push(Event::VideoPlaybackPrepared(VideoPlaybackPreparedEvent {
                        video_id: player.video_id,
                        video_width: width,
                        video_height: height,
                        duration,
                        is_seekable,
                        video_tracks,
                        audio_tracks,
                    }));
                }
                Some(Err(err)) => {
                    events.push(Event::VideoDecodingError(VideoDecodingErrorEvent {
                        video_id: player.video_id,
                        error: err,
                    }));
                }
                None => {}
            }

            if let Some(gl) = gl {
                if player.poll_frame(unsafe { &*gl }, &mut self.textures) {
                    events.push(Event::VideoTextureUpdated(VideoTextureUpdatedEvent {
                        video_id: player.video_id,
                        current_position_ms: 0,
                        yuv: crate::event::video_playback::VideoYuvMetadata {
                            enabled: true,
                            matrix: 1.0,
                            biplanar: false,
                            rotation_steps: player.yuv_rotation_steps(),
                        },
                    }));
                }
            }
        }

        self.os.camera_players = players;
        for event in events {
            self.call_event_handler(&event);
        }
    }

    fn poll_software_video_players(&mut self) {
        if self.os.software_video_players.is_empty() {
            return;
        }

        let gl: *const LibGl = self.os.gl();
        let mut players = std::mem::take(&mut self.os.software_video_players);
        let mut events = Vec::new();

        for (_video_id, asp) in players.iter_mut() {
            match asp.player.check_prepared() {
                Some(Ok((width, height, duration, is_seekable, video_tracks, audio_tracks))) => {
                    events.push(Event::VideoPlaybackPrepared(VideoPlaybackPreparedEvent {
                        video_id: asp.player.video_id,
                        video_width: width,
                        video_height: height,
                        duration,
                        is_seekable,
                        video_tracks,
                        audio_tracks,
                    }));
                }
                Some(Err(err)) => {
                    events.push(Event::VideoDecodingError(VideoDecodingErrorEvent {
                        video_id: asp.player.video_id,
                        error: err,
                    }));
                }
                None => {}
            }

            if asp.player.poll_frame() {
                if let Some(planes) = asp.player.take_yuv_frame() {
                    asp.yuv_matrix = planes.matrix.as_f32();
                    upload_yuv_to_gl(
                        unsafe { &*gl },
                        &mut self.textures,
                        asp.tex_y_id,
                        asp.tex_u_id,
                        asp.tex_v_id,
                        &planes,
                    );
                    events.push(Event::VideoTextureUpdated(VideoTextureUpdatedEvent {
                        video_id: asp.player.video_id,
                        current_position_ms: asp.player.current_position_ms(),
                        yuv: crate::event::video_playback::VideoYuvMetadata {
                            enabled: true,
                            matrix: asp.yuv_matrix,
                            biplanar: false,
                            rotation_steps: 0.0,
                        },
                    }));
                }
            }

            if asp.player.check_eos() {
                events.push(Event::VideoPlaybackCompleted(VideoPlaybackCompletedEvent {
                    video_id: asp.player.video_id,
                }));
            }
        }

        self.os.software_video_players = players;
        for event in events {
            self.call_event_handler(&event);
        }
    }

    pub fn android_entry<F>(activity: *const std::ffi::c_void, startup: F)
    where
        F: FnOnce() -> Box<Cx> + Send + 'static,
    {
        let activity_thread_id = unsafe { libc_sys::syscall(libc_sys::SYS_GETTID) as u64 };
        let activity_handle = unsafe { android_jni::fetch_activity_handle(activity) };

        let already_running = android_jni::from_java_messages_already_set();

        if already_running {
            android_jni::jni_update_activity(activity_handle);
            // maybe send activity update?
            android_jni::send_from_java_message(FromJavaMessage::SwitchedActivity(
                activity_handle,
                activity_thread_id,
            ));

            return;
        }

        let (from_java_tx, from_java_rx) = mpsc::channel();

        std::panic::set_hook(Box::new(|info| {
            crate::log!("Custom panic hook: {}", info);
        }));

        android_jni::jni_set_activity(activity_handle);
        android_jni::jni_set_from_java_tx(from_java_tx);

        // lets start a thread
        std::thread::spawn(move || {
            // SAFETY: This attaches the current thread to the JVM. It's safe as long as we're in the correct thread.
            unsafe { attach_jni_env() };
            let mut cx = startup();
            let mut libegl = LibEgl::try_load().expect("Cant load LibEGL");

            #[cfg(use_vulkan)]
            crate::log!(
                "Android backend mode: Vulkan renderer + OpenGL shader compiler compatibility path"
            );
            #[cfg(not(use_vulkan))]
            crate::log!("Android backend mode: OpenGL renderer");

            cx.os.activity_thread_id = Some(activity_thread_id);
            cx.os.render_thread_id =
                Some(unsafe { libc_sys::syscall(libc_sys::SYS_GETTID) as u64 });

            let window = loop {
                // Here use blocking method `recv` to reduce CPU usage during cold start.
                match from_java_rx.recv() {
                    Ok(FromJavaMessage::Init(params)) => {
                        cx.os.dpi_factor = params.density;
                        cx.os_type = OsType::Android(params);
                    }
                    Ok(FromJavaMessage::SurfaceChanged {
                        window,
                        width,
                        height,
                    }) => {
                        cx.os.display_size = dvec2(width as f64, height as f64);
                        break window;
                    }
                    _ => (),
                }
            };

            // SAFETY:
            // The LibEgl instance (libegl) has been properly loaded and initialized earlier.
            // We're not requesting a robust context (false), which is usually fine for most applications.
            #[cfg(not(quest))]
            let (egl_context, egl_config, egl_display) = unsafe {
                egl_sys::create_egl_context(
                    &mut libegl,
                    std::ptr::null_mut(), /* EGL_DEFAULT_DISPLAY */
                )
                .expect("Cant create EGL context")
            };

            #[cfg(quest)]
            let (egl_context, egl_config, egl_display) = unsafe {
                egl_sys::create_egl_context_openxr(
                    &mut libegl,
                    std::ptr::null_mut(), /* EGL_DEFAULT_DISPLAY */
                )
                .expect("Cant create EGL context")
            };

            // SAFETY: This is loading OpenGL function pointers. It's safe as long as we have a valid EGL context.
            let libgl = LibGl::try_load(|s| {
                for s in s {
                    let s = CString::new(*s).unwrap();
                    let p = unsafe { libegl.eglGetProcAddress.unwrap()(s.as_ptr()) };
                    if !p.is_null() {
                        return p;
                    }
                }
                0 as _
            })
            .expect("Cant load openGL functions");

            // SAFETY: Create an EGL surface to keep GL APIs available on Android.
            // In Vulkan mode this must not bind the native window, or Vulkan surface creation will fail.
            #[cfg(not(use_vulkan))]
            let surface = unsafe {
                (libegl.eglCreateWindowSurface.unwrap())(
                    egl_display,
                    egl_config,
                    window as _,
                    std::ptr::null_mut(),
                )
            };

            #[cfg(use_vulkan)]
            let surface = unsafe {
                let pbuffer_attribs = [
                    egl_sys::EGL_WIDTH as i32,
                    1,
                    egl_sys::EGL_HEIGHT as i32,
                    1,
                    egl_sys::EGL_NONE as i32,
                ];
                (libegl.eglCreatePbufferSurface.unwrap())(
                    egl_display,
                    egl_config,
                    pbuffer_attribs.as_ptr(),
                )
            };

            if unsafe {
                (libegl.eglMakeCurrent.unwrap())(egl_display, surface, surface, egl_context)
            } == 0
            {
                panic!();
            }

            //libgl.enable_debugging();

            //cx.maybe_warn_hardware_support();

            cx.os.display = Some(CxAndroidDisplay {
                libegl,
                libgl,
                egl_display,
                egl_config,
                egl_context,
                surface,
                window,
            });

            #[cfg(use_vulkan)]
            {
                match CxVulkan::new(
                    window,
                    cx.os.display_size.x.max(1.0) as u32,
                    cx.os.display_size.y.max(1.0) as u32,
                ) {
                    Ok(vulkan) => {
                        crate::log!("Android Vulkan backend initialized on startup");
                        cx.os.vulkan = Some(vulkan);
                    }
                    Err(err) => {
                        crate::error!(
                            "Android Vulkan backend init failed on startup, continuing with OpenGL: {err}"
                        );
                    }
                }
            }

            cx.main_loop(from_java_rx);
            cx.stop_studio_websocket();

            #[cfg(use_vulkan)]
            {
                let _ = cx.os.vulkan.take();
            }

            let display = cx.os.display.take().unwrap();

            // SAFETY: These calls clean up EGL resources. They're safe as long as we have valid EGL objects.
            unsafe {
                if !display.window.is_null() {
                    ndk_sys::ANativeWindow_release(display.window);
                }
                (display.libegl.eglMakeCurrent.unwrap())(
                    display.egl_display,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                (display.libegl.eglDestroySurface.unwrap())(display.egl_display, display.surface);
                (display.libegl.eglDestroyContext.unwrap())(
                    display.egl_display,
                    display.egl_context,
                );
                (display.libegl.eglTerminate.unwrap())(display.egl_display);
            }
        });
    }

    pub fn start_network_live_file_watcher(&mut self) {

        /*
        log!("WATCHING NETWORK FOR FILE WATCHER");
        let studio_uid: Option<&'static str> = std::option_env!("MAKEPAD_STUDIO_UID");
        if studio_uid.is_none(){
            return
        }
        let studio_uid:u64 = studio_uid.unwrap().parse().unwrap_or(0);
        std::thread::spawn(move || {
            let discovery = UdpSocket::bind("0.0.0.0:41533").unwrap();
            discovery.set_read_timeout(Some(Duration::new(0, 1))).unwrap();
            discovery.set_broadcast(true).unwrap();

            let mut other_uid = [0u8; 8];
            let mut host_addr = None;
            // nonblockingly (timeout=1ns) check our discovery socket for peers
            'outer: loop{
                while let Ok((_, mut addr)) = discovery.recv_from(&mut other_uid) {
                    let recv_uid = u64::from_be_bytes(other_uid);
                    log!("GOT ADDR {} {}",studio_uid, recv_uid);
                    if studio_uid == recv_uid {
                        // we found our host. lets connect to it
                        host_addr = Some(addr);
                        break 'outer;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let host_addr = host_addr.unwrap();
            // ok we can connect
            log!("WE CAN CONNECT {:?}", host_addr);
        });*/
    }

    /*
     pub fn from_java_on_paste_from_clipboard(&mut self, content: Option<String>, to_java: AndroidToJava) {
         if let Some(text) = content {
             let e = Event::TextInput(
                 TextInputEvent {
                     input: text,
                     replace_last: false,
                     was_paste: true,
                     ..Default::default()
                 }
             );
             self.call_event_handler(&e);
             self.after_every_event(&to_java);
         }
     }

     pub fn from_java_on_cut_to_clipboard(&mut self, to_java: AndroidToJava) {
         let e = Event::TextCut(
             TextClipboardEvent {
                 response: Rc::new(RefCell::new(None))
             }
         );
         self.call_event_handler(&e);
         self.after_every_event(&to_java);
     }
    */

    pub fn android_load_dependencies(&mut self) {
        for (path, dep) in &mut self.dependencies {
            if let Some(data) = unsafe { to_java_load_asset(path) } {
                dep.data = Some(Ok(Rc::new(data)))
            } else {
                let message = format!("cannot load dependency {}", path);
                crate::error!("Android asset failed: {}", message);
                dep.data = Some(Err(message));
            }
        }
    }

    pub fn draw_pass_to_fullscreen(&mut self, draw_pass_id: DrawPassId) {
        let draw_list_id = self.passes[draw_pass_id].main_draw_list_id.unwrap();

        self.setup_render_pass(draw_pass_id);

        // keep repainting in a loop
        self.passes[draw_pass_id].paint_dirty = false;
        //let panning_offset = if self.os.keyboard_visible {self.os.keyboard_panning_offset} else {0};

        let gl = self.os.gl();
        unsafe {
            (gl.glViewport)(
                0,
                0,
                self.os.display_size.x as i32,
                self.os.display_size.y as i32,
            );
        }

        let clear_color = if self.passes[draw_pass_id].color_textures.len() == 0 {
            self.passes[draw_pass_id].clear_color
        } else {
            match self.passes[draw_pass_id].color_textures[0].clear_color {
                DrawPassClearColor::InitWith(color) => color,
                DrawPassClearColor::ClearWith(color) => color,
            }
        };
        let clear_depth = match self.passes[draw_pass_id].clear_depth {
            DrawPassClearDepth::InitWith(depth) => depth,
            DrawPassClearDepth::ClearWith(depth) => depth,
        };

        if !self.passes[draw_pass_id].dont_clear {
            unsafe {
                //(gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, 0);
                (gl.glClearDepthf)(clear_depth as f32);
                (gl.glClearColor)(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
                (gl.glClear)(gl_sys::COLOR_BUFFER_BIT | gl_sys::DEPTH_BUFFER_BIT);
            }
        }
        Self::set_default_depth_and_blend_mode(gl);

        let mut zbias = 0.0;
        let zbias_step = self.passes[draw_pass_id].zbias_step;

        self.render_view(draw_pass_id, draw_list_id, &mut zbias, zbias_step);

        //to_java.swap_buffers();
        //unsafe {
        //direct_app.drm.swap_buffers_and_wait(&direct_app.egl);
        //}
    }

    pub(crate) fn handle_repaint(&mut self) {
        //opengl_cx.make_current();
        let mut passes_todo = Vec::new();
        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for draw_pass_id in &passes_todo {
            self.passes[*draw_pass_id].set_time(self.os.timers.time_now() as f32);
            match self.passes[*draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {
                    // cant happen
                }
                CxDrawPassParent::Window(window_id) => {
                    // Skip popup window passes — they are drawn as overlays
                    // after their parent window pass below.
                    if self.windows[window_id].is_popup {
                        continue;
                    }
                    let start = self.seconds_since_app_start();
                    let metrics = self.collect_gpu_pass_metrics(*draw_pass_id);
                    self.draw_pass_to_window_for_active_backend(*draw_pass_id);

                    // Draw popup window passes as overlays on the same surface
                    for popup_pass_id in &passes_todo.clone() {
                        if let CxDrawPassParent::Window(pw_id) = self.passes[*popup_pass_id].parent
                        {
                            let pw = &self.windows[pw_id];
                            if pw.is_popup && pw.popup_parent == Some(window_id) {
                                let saved = self.passes[*popup_pass_id].dont_clear;
                                self.passes[*popup_pass_id].dont_clear = true;
                                self.draw_pass_to_fullscreen(*popup_pass_id);
                                self.passes[*popup_pass_id].dont_clear = saved;
                            }
                        }
                    }

                    let end = self.seconds_since_app_start();
                    Cx::send_studio_message(AppToStudio::GPUSample(GPUSample {
                        start,
                        end,
                        draw_calls: metrics.draw_calls,
                        instances: metrics.instances,
                        vertices: metrics.vertices,
                        instance_bytes: metrics.instance_bytes,
                        uniform_bytes: metrics.uniform_bytes,
                        vertex_buffer_bytes: metrics.vertex_buffer_bytes,
                        texture_bytes: metrics.texture_bytes,
                    }));
                    self.present_window_for_active_backend();
                }
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_texture(*draw_pass_id, None);
                }
                CxDrawPassParent::None => {
                    self.draw_pass_to_texture(*draw_pass_id, None);
                }
            }
        }

        let timestamp_ns = (self.os.timers.time_now().max(0.0) * 1_000_000_000.0) as u64;
        for index in 0..MAX_VIDEO_DEVICE_INDEX {
            if let Err(err) = self.video_encoder_capture_texture_frame(index, timestamp_ns) {
                if err != VideoEncodeError::UnsupportedSource
                    && err != VideoEncodeError::EncoderNotStarted
                {
                    crate::error!(
                        "android video texture capture failed on slot {}: {:?}",
                        index,
                        err
                    );
                }
            }
        }
    }

    fn handle_platform_ops(&mut self) -> EventFlow {
        while let Some(op) = self.platform_ops.pop() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    let dpi_factor = window.dpi_override.unwrap_or(self.os.dpi_factor);
                    let size = self.os.display_size / dpi_factor;
                    window.window_geom = WindowGeom {
                        dpi_factor,
                        can_fullscreen: false,
                        xr_is_presenting: false,
                        is_fullscreen: true,
                        is_topmost: true,
                        position: dvec2(0.0, 0.0),
                        inner_size: size,
                        outer_size: size,
                    };
                    window.is_created = true;
                    //let ret = unsafe{ndk_sys::ANativeWindow_setFrameRate(self.os.display.as_ref().unwrap().window, 120.0, 0)};
                    //crate::log!("{}",ret);
                    let new_geom = window.window_geom.clone();
                    let old_geom = window.window_geom.clone();
                    self.call_event_handler(&Event::WindowGeomChange(WindowGeomChangeEvent {
                        window_id,
                        new_geom,
                        old_geom,
                    }));
                }
                CxOsOp::CreatePopupWindow {
                    window_id,
                    parent_window_id,
                    position,
                    size,
                    grab_keyboard,
                } => {
                    let dpi_factor = self.windows[parent_window_id]
                        .dpi_override
                        .unwrap_or(self.os.dpi_factor);
                    let window = &mut self.windows[window_id];
                    window.window_geom = WindowGeom {
                        dpi_factor,
                        can_fullscreen: false,
                        xr_is_presenting: false,
                        is_fullscreen: false,
                        is_topmost: true,
                        position,
                        inner_size: size,
                        outer_size: size,
                    };
                    window.is_popup = true;
                    window.popup_parent = Some(parent_window_id);
                    window.popup_position = Some(position);
                    window.popup_size = Some(size);
                    window.popup_grab_keyboard = grab_keyboard;
                    window.is_created = true;
                }
                CxOsOp::CloseWindow(window_id) => {
                    let window = &mut self.windows[window_id];
                    if window.is_popup {
                        window.is_created = false;
                        window.is_popup = false;
                        window.popup_parent = None;
                        window.popup_position = None;
                        window.popup_size = None;
                    }
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    self.os
                        .timers
                        .timers
                        .insert(timer_id, PollTimer::new(interval, repeats));
                }
                CxOsOp::StopTimer(timer_id) => {
                    self.os.timers.timers.remove(&timer_id);
                }
                CxOsOp::ShowTextIME(_area, _pos, config) => unsafe {
                    android_jni::to_java_configure_keyboard(&config);
                    android_jni::to_java_show_keyboard(true);
                },
                CxOsOp::HideTextIME => unsafe {
                    android_jni::to_java_show_keyboard(false);
                },
                CxOsOp::SyncImeState {
                    text,
                    selection,
                    composition: _,
                } => {
                    let sel_start_utf16 = selection.start.to_utf16_index(&text) as i32;
                    let sel_end_utf16 = selection.end.to_utf16_index(&text) as i32;
                    unsafe {
                        android_jni::to_java_update_ime_text_state(
                            &text,
                            sel_start_utf16,
                            sel_end_utf16,
                        );
                    }
                }
                CxOsOp::CopyToClipboard(content) => unsafe {
                    android_jni::to_java_copy_to_clipboard(content);
                },
                CxOsOp::SetPrimarySelection(_) => {}
                CxOsOp::ShowSelectionHandles { start, end } => unsafe {
                    // Rust positions are in logical points; Android overlay APIs expect physical pixels.
                    let dpi_factor = self.windows[CxWindowPool::id_zero()]
                        .dpi_override
                        .unwrap_or(self.os.dpi_factor);
                    android_jni::to_java_show_selection_handles(start * dpi_factor, end * dpi_factor);
                },
                CxOsOp::UpdateSelectionHandles { start, end } => unsafe {
                    let dpi_factor = self.windows[CxWindowPool::id_zero()]
                        .dpi_override
                        .unwrap_or(self.os.dpi_factor);
                    android_jni::to_java_update_selection_handles(
                        start * dpi_factor,
                        end * dpi_factor,
                    );
                },
                CxOsOp::HideSelectionHandles => unsafe {
                    android_jni::to_java_hide_selection_handles();
                },
                CxOsOp::AccessibilityUpdate(_) => {}
                CxOsOp::ShowClipboardActions {
                    has_selection,
                    rect,
                    keyboard_shift,
                } => unsafe {
                    android_jni::to_java_show_clipboard_actions(
                        has_selection,
                        rect,
                        keyboard_shift,
                        self.os.dpi_factor,
                    );
                },
                CxOsOp::HideClipboardActions => unsafe {
                    android_jni::to_java_dismiss_clipboard_actions();
                },
                CxOsOp::AttachCameraNativePreview { video_id, area } => {
                    let rect = area.clipped_rect(self);
                    let left = (rect.pos.x * self.os.dpi_factor) as i32;
                    let top = (rect.pos.y * self.os.dpi_factor) as i32;
                    let right = ((rect.pos.x + rect.size.x) * self.os.dpi_factor) as i32;
                    let bottom = ((rect.pos.y + rect.size.y) * self.os.dpi_factor) as i32;
                    unsafe {
                        android_jni::to_java_attach_camera_preview(
                            video_id, left, top, right, bottom,
                        );
                    }
                }
                CxOsOp::UpdateCameraNativePreview {
                    video_id,
                    area,
                    visible,
                } => {
                    let rect = area.clipped_rect(self);
                    let left = (rect.pos.x * self.os.dpi_factor) as i32;
                    let top = (rect.pos.y * self.os.dpi_factor) as i32;
                    let right = ((rect.pos.x + rect.size.x) * self.os.dpi_factor) as i32;
                    let bottom = ((rect.pos.y + rect.size.y) * self.os.dpi_factor) as i32;
                    unsafe {
                        android_jni::to_java_update_camera_preview(
                            video_id, left, top, right, bottom, visible,
                        );
                    }
                }
                CxOsOp::DetachCameraNativePreview { video_id } => {
                    unsafe {
                        android_jni::to_java_detach_camera_preview(video_id);
                    }
                    if let Some(player) = self.os.camera_players.get_mut(&video_id) {
                        player.set_preview_window(None);
                    }
                    if let Some(window) = self.os.pending_camera_preview_windows.remove(&video_id) {
                        unsafe {
                            ndk_sys::ANativeWindow_release(window);
                        }
                    }
                }
                CxOsOp::CheckPermission {
                    permission,
                    request_id,
                } => {
                    self.handle_permission_check(permission, request_id);
                }
                CxOsOp::RequestPermission {
                    permission,
                    request_id,
                } => {
                    self.handle_permission_request(permission, request_id);
                }
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => unsafe {
                    android_jni::to_java_http_request(request_id, request);
                },
                CxOsOp::PrepareVideoPlayback(
                    video_id,
                    source,
                    camera_preview_mode,
                    external_texture_id,
                    texture_id,
                    autoplay,
                    should_loop,
                ) => {
                    // Camera source: use NDK camera player with YUV plane textures
                    if let VideoSource::Camera(input_id, format_id) = source {
                        let tex_y = self.textures.alloc(TextureFormat::VideoYuvPlane);
                        let tex_u = self.textures.alloc(TextureFormat::VideoYuvPlane);
                        let tex_v = self.textures.alloc(TextureFormat::VideoYuvPlane);
                        let tex_y_id = tex_y.texture_id();
                        let tex_u_id = tex_u.texture_id();
                        let tex_v_id = tex_v.texture_id();
                        let camera_access = self.os.media.android_camera();
                        let native_preview =
                            matches!(camera_preview_mode, CameraPreviewMode::Native);
                        let preview_window = if native_preview {
                            self.os.pending_camera_preview_windows.remove(&video_id)
                        } else {
                            if let Some(window) =
                                self.os.pending_camera_preview_windows.remove(&video_id)
                            {
                                unsafe {
                                    ndk_sys::ANativeWindow_release(window);
                                }
                            }
                            None
                        };
                        let player = AndroidCameraPlayer::new(
                            video_id,
                            tex_y_id,
                            tex_u_id,
                            tex_v_id,
                            input_id,
                            format_id,
                            native_preview,
                            preview_window,
                            camera_access,
                        );
                        self.os.camera_players.insert(video_id, player);
                        self.call_event_handler(&Event::VideoYuvTexturesReady(
                            VideoYuvTexturesReady {
                                video_id,
                                tex_y,
                                tex_u,
                                tex_v,
                            },
                        ));
                        continue;
                    }

                    // Allocate YUV textures internally for software decode path
                    let tex_y = self.textures.alloc(TextureFormat::VideoYuvPlane);
                    let tex_u = self.textures.alloc(TextureFormat::VideoYuvPlane);
                    let tex_v = self.textures.alloc(TextureFormat::VideoYuvPlane);
                    let tex_y_id = tex_y.texture_id();
                    let tex_u_id = tex_u.texture_id();
                    let tex_v_id = tex_v.texture_id();
                    self.os.video_configs.insert(
                        video_id,
                        AndroidVideoConfig {
                            video_id,
                            source: source.clone(),
                            texture_id,
                            tex_y_id,
                            tex_u_id,
                            tex_v_id,
                            autoplay,
                            should_loop,
                        },
                    );

                    let force_software = force_software_video();
                    if force_software {
                        crate::log!(
                            "VIDEO: MAKEPAD_FORCE_SOFTWARE_VIDEO set, using software video decoder"
                        );
                        self.os.software_video_players.insert(
                            video_id,
                            AndroidSoftwarePlayer {
                                player: SoftwareVideoPlayer::new(
                                    video_id,
                                    texture_id,
                                    source,
                                    autoplay,
                                    should_loop,
                                ),
                                tex_y_id,
                                tex_u_id,
                                tex_v_id,
                                yuv_matrix: 0.0,
                            },
                        );
                        // Notify widget so it can bind textures to shader slots
                        self.call_event_handler(&Event::VideoYuvTexturesReady(
                            VideoYuvTexturesReady {
                                video_id,
                                tex_y,
                                tex_u,
                                tex_v,
                            },
                        ));
                        continue;
                    }
                    // Notify widget so it can bind textures to shader slots
                    // (needed if native decode fails and we fall back to software)
                    self.call_event_handler(&Event::VideoYuvTexturesReady(VideoYuvTexturesReady {
                        video_id,
                        tex_y,
                        tex_u,
                        tex_v,
                    }));

                    unsafe {
                        let env = attach_jni_env();
                        android_jni::to_java_prepare_video_playback(
                            env,
                            video_id,
                            source,
                            external_texture_id,
                            autoplay,
                            should_loop,
                        );
                    }
                }
                CxOsOp::BeginVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(asp) = self.os.software_video_players.get_mut(&video_id) {
                        asp.player.play();
                    } else {
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_begin_video_playback(env, video_id);
                        }
                    }
                }
                CxOsOp::PauseVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(asp) = self.os.software_video_players.get_mut(&video_id) {
                        asp.player.pause();
                    } else {
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_pause_video_playback(env, video_id);
                        }
                    }
                }
                CxOsOp::ResumeVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(asp) = self.os.software_video_players.get_mut(&video_id) {
                        asp.player.resume();
                    } else {
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_resume_video_playback(env, video_id);
                        }
                    }
                }
                CxOsOp::MuteVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(asp) = self.os.software_video_players.get(&video_id) {
                        asp.player.mute();
                    } else {
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_mute_video_playback(env, video_id);
                        }
                    }
                }
                CxOsOp::UnmuteVideoPlayback(video_id) => {
                    if self.os.camera_players.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(asp) = self.os.software_video_players.get(&video_id) {
                        asp.player.unmute();
                    } else {
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_unmute_video_playback(env, video_id);
                        }
                    }
                }
                CxOsOp::CleanupVideoPlaybackResources(video_id) => {
                    if let Some(mut player) = self.os.camera_players.remove(&video_id) {
                        player.cleanup();
                        unsafe {
                            android_jni::to_java_detach_camera_preview(video_id);
                        }
                        if let Some(window) =
                            self.os.pending_camera_preview_windows.remove(&video_id)
                        {
                            unsafe {
                                ndk_sys::ANativeWindow_release(window);
                            }
                        }
                        self.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                            VideoPlaybackResourcesReleasedEvent { video_id },
                        ));
                        continue;
                    }
                    if let Some(mut asp) = self.os.software_video_players.remove(&video_id) {
                        asp.player.cleanup();
                        self.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                            VideoPlaybackResourcesReleasedEvent { video_id },
                        ));
                    }
                    if let Some(decoder_ref) = self.os.video_surfaces.remove(&video_id) {
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_cleanup_video_decoder_ref(env, decoder_ref);
                            android_jni::to_java_cleanup_video_playback_resources(env, video_id);
                        }
                    } else {
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_cleanup_video_playback_resources(env, video_id);
                        }
                    }
                    self.os.video_configs.remove(&video_id);
                }
                CxOsOp::SeekVideoPlayback(video_id, position_ms) => {
                    if self.os.camera_players.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(asp) = self.os.software_video_players.get_mut(&video_id) {
                        asp.player.seek_to(position_ms);
                    } else {
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_seek_video_playback(env, video_id, position_ms);
                        }
                    }
                }
                CxOsOp::SetVideoVolume(video_id, volume) => {
                    if self.os.camera_players.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(asp) = self.os.software_video_players.get(&video_id) {
                        asp.player.set_volume(volume);
                    }
                }
                CxOsOp::SetVideoPlaybackRate(video_id, rate) => {
                    if self.os.camera_players.contains_key(&video_id) {
                        continue;
                    }
                    if let Some(asp) = self.os.software_video_players.get(&video_id) {
                        asp.player.set_playback_rate(rate);
                    }
                }
                CxOsOp::PrepareAudioPlayback(video_id, source, autoplay, should_loop) => {
                    // Android: treat same as video but without a texture
                    let _ = (video_id, source, autoplay, should_loop);
                    // TODO: implement via MediaPlayer when needed
                }
                CxOsOp::XrStartPresenting => {
                    self.os.ignore_destroy = true;
                    if !self.os.in_xr_mode {
                        self.os.in_xr_mode = true;
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_switch_activity(env);
                        }
                    }
                }
                CxOsOp::XrStopPresenting => {
                    self.os.ignore_destroy = true;
                    if self.os.in_xr_mode {
                        self.os.in_xr_mode = false;
                        unsafe {
                            let env = attach_jni_env();
                            android_jni::to_java_switch_activity(env);
                        }
                    }
                }
                CxOsOp::XrAdvertiseAnchor(anchor) => {
                    self.os.openxr.advertise_anchor(anchor);
                }
                CxOsOp::XrSetLocalAnchor(anchor) => {
                    self.os.openxr.set_local_anchor(anchor);
                }
                CxOsOp::XrDiscoverAnchor(id) => {
                    self.os.openxr.discover_anchor(id);
                }
                CxOsOp::FullscreenWindow(_window_id) => {
                    self.os.fullscreen = true;
                    unsafe {
                        let env = attach_jni_env();
                        android_jni::to_java_set_full_screen(env, true);
                    }
                }
                CxOsOp::NormalizeWindow(_window_id) => {
                    self.os.fullscreen = false;
                    unsafe {
                        let env = attach_jni_env();
                        android_jni::to_java_set_full_screen(env, false);
                    }
                }
                CxOsOp::SetCursor(_) => {
                    // no need
                }
                e => {
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                }
            }
        }
        EventFlow::Poll
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        super::android_network::install_network_backend_shim();
        self.package_root = Some("makepad".to_string());
    }

    fn spawn_thread<F>(&mut self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        std::thread::spawn(f);
    }

    fn seconds_since_app_start(&self) -> f64 {
        Instant::now()
            .duration_since(self.os.start_time)
            .as_secs_f64()
    }

    fn open_url(&mut self, _url: &str, _in_place: OpenUrlInPlace) {
        crate::error!("open_url not implemented on this platform");
    }

    fn in_xr_mode(&self) -> bool {
        self.os.in_xr_mode
    }

    fn micro_zbias_step(&self) -> f32 {
        if self.os.in_xr_mode {
            -0.0001
        } else {
            0.00001
        }
    }
}

fn to_android_permission(permission: crate::permission::Permission) -> &'static str {
    match permission {
        crate::permission::Permission::AudioInput => "android.permission.RECORD_AUDIO",
        crate::permission::Permission::Camera => "android.permission.CAMERA",
    }
}

impl Cx {
    fn find_popup_to_dismiss_on_touch(
        &self,
        touches: &[crate::event::finger::TouchPoint],
    ) -> Option<crate::window::WindowId> {
        for i in (0..self.windows.len()).rev() {
            let window_id = CxWindowPool::from_usize(i);
            let window = &self.windows[window_id];
            if !window.is_created || !window.is_popup {
                continue;
            }
            if let (Some(pos), Some(size)) = (window.popup_position, window.popup_size) {
                let rect = Rect {
                    pos: pos,
                    size: size,
                };
                for touch in touches {
                    if touch.state == crate::event::finger::TouchState::Start
                        && !rect.contains(touch.abs)
                    {
                        return Some(window_id);
                    }
                }
            }
        }
        None
    }

    fn dismiss_popup_window(
        &mut self,
        window_id: crate::window::WindowId,
        reason: crate::event::PopupDismissReason,
    ) {
        // First dismiss any child popups
        let children: Vec<crate::window::WindowId> = (0..self.windows.len())
            .filter_map(|i| {
                let child_id = CxWindowPool::from_usize(i);
                let w = &self.windows[child_id];
                if w.is_created && w.is_popup && w.popup_parent == Some(window_id) {
                    Some(child_id)
                } else {
                    None
                }
            })
            .collect();
        for child_id in children {
            self.dismiss_popup_window(child_id, crate::event::PopupDismissReason::ParentClosed);
        }
        self.call_event_handler(&Event::PopupDismissed(crate::event::PopupDismissedEvent {
            window_id,
            reason,
        }));
        self.call_event_handler(&Event::WindowClosed(crate::event::WindowClosedEvent {
            window_id,
        }));
        self.windows[window_id].is_created = false;
    }

    fn check_audio_permission_status(&self) -> crate::permission::PermissionStatus {
        unsafe {
            let status = android_jni::to_java_check_permission("android.permission.RECORD_AUDIO");
            match status {
                0 => crate::permission::PermissionStatus::NotDetermined, // Never asked or permanently denied
                1 => crate::permission::PermissionStatus::Granted,
                2 => crate::permission::PermissionStatus::DeniedCanRetry, // User denied but can retry with rationale
                _ => {
                    crate::log!("Unknown permission check status: {}", status);
                    crate::permission::PermissionStatus::NotDetermined // Default to safest assumption
                }
            }
        }
    }

    fn check_camera_permission_status(&self) -> crate::permission::PermissionStatus {
        unsafe {
            let status = android_jni::to_java_check_permission("android.permission.CAMERA");
            match status {
                0 => crate::permission::PermissionStatus::NotDetermined,
                1 => crate::permission::PermissionStatus::Granted,
                2 => crate::permission::PermissionStatus::DeniedCanRetry,
                _ => {
                    crate::log!("Unknown permission check status: {}", status);
                    crate::permission::PermissionStatus::NotDetermined
                }
            }
        }
    }

    fn handle_permission_check(
        &mut self,
        permission: crate::permission::Permission,
        request_id: i32,
    ) {
        let status = match permission {
            crate::permission::Permission::AudioInput => self.check_audio_permission_status(),
            crate::permission::Permission::Camera => self.check_camera_permission_status(),
        };

        self.call_event_handler(&Event::PermissionResult(
            crate::permission::PermissionResult {
                permission,
                request_id,
                status,
            },
        ));
    }

    fn handle_permission_request(
        &mut self,
        permission: crate::permission::Permission,
        request_id: i32,
    ) {
        let status = match permission {
            crate::permission::Permission::AudioInput => self.check_audio_permission_status(),
            crate::permission::Permission::Camera => self.check_camera_permission_status(),
        };
        match status {
            crate::permission::PermissionStatus::Granted => {
                self.call_event_handler(&Event::PermissionResult(
                    crate::permission::PermissionResult {
                        permission,
                        request_id,
                        status,
                    },
                ));
            }
            crate::permission::PermissionStatus::DeniedCanRetry
            | crate::permission::PermissionStatus::NotDetermined => unsafe {
                android_jni::to_java_request_permission(
                    to_android_permission(permission),
                    request_id,
                );
            },
            _ => {
                self.call_event_handler(&Event::PermissionResult(
                    crate::permission::PermissionResult {
                        permission,
                        request_id,
                        status,
                    },
                ));
            }
        }
    }
}

fn string_to_permission(permission_str: &str) -> Option<crate::permission::Permission> {
    match permission_str {
        "android.permission.RECORD_AUDIO" => Some(crate::permission::Permission::AudioInput),
        "android.permission.CAMERA" => Some(crate::permission::Permission::Camera),
        _ => None,
    }
}

impl Default for CxOs {
    fn default() -> Self {
        Self {
            start_time: Instant::now(),
            first_after_resize: true,
            frame_time: 0,
            display_size: dvec2(100., 100.),
            dpi_factor: 1.5,
            keyboard_closed: 0.0,
            media: CxAndroidMedia::default(),
            display: None,
            #[cfg(use_vulkan)]
            vulkan: None,
            quit: false,
            fullscreen: false,
            timers: Default::default(),
            video_surfaces: HashMap::new(),
            video_configs: HashMap::new(),
            camera_players: HashMap::new(),
            pending_camera_preview_windows: HashMap::new(),
            software_video_players: HashMap::new(),
            websocket_parsers: HashMap::new(),
            openxr: CxOpenXr::default(),
            activity_thread_id: None,
            render_thread_id: None,
            ignore_destroy: false,
            in_xr_mode: false,
        }
    }
}

pub struct CxAndroidDisplay {
    pub libegl: LibEgl,
    pub libgl: LibGl,
    pub egl_display: egl_sys::EGLDisplay,
    pub egl_config: egl_sys::EGLConfig,
    pub egl_context: egl_sys::EGLContext,
    surface: egl_sys::EGLSurface,
    window: *mut ndk_sys::ANativeWindow,
    //event_handler: Box<dyn EventHandler>,
}

pub(crate) struct AndroidSoftwarePlayer {
    pub player: SoftwareVideoPlayer,
    pub tex_y_id: TextureId,
    pub tex_u_id: TextureId,
    pub tex_v_id: TextureId,
    pub yuv_matrix: f32,
}

pub struct CxOs {
    pub first_after_resize: bool,
    pub display_size: Vec2d,
    pub dpi_factor: f64,
    pub keyboard_closed: f64,
    pub frame_time: i64,
    pub quit: bool,
    pub fullscreen: bool,
    pub(crate) start_time: Instant,
    pub(crate) timers: PollTimers,
    pub display: Option<CxAndroidDisplay>,
    #[cfg(use_vulkan)]
    pub(crate) vulkan: Option<CxVulkan>,
    pub(crate) media: CxAndroidMedia,
    pub(crate) video_surfaces: HashMap<LiveId, jobject>,
    pub(crate) video_configs: HashMap<LiveId, AndroidVideoConfig>,
    pub(crate) camera_players: HashMap<LiveId, AndroidCameraPlayer>,
    pub(crate) pending_camera_preview_windows: HashMap<LiveId, *mut ndk_sys::ANativeWindow>,
    pub(crate) software_video_players: HashMap<LiveId, AndroidSoftwarePlayer>,
    websocket_parsers: HashMap<u64, WebSocketImpl>,
    pub(crate) openxr: CxOpenXr,
    pub(crate) activity_thread_id: Option<u64>,
    pub(crate) render_thread_id: Option<u64>,
    pub(crate) ignore_destroy: bool,
    pub(crate) in_xr_mode: bool,
}

impl CxOs {
    pub(crate) fn gl(&self) -> &LibGl {
        &self.display.as_ref().unwrap().libgl
    }
}

impl CxAndroidDisplay {
    /// Make Makepad's EGL context current (with its surface).
    /// Required before creating shared GL contexts.
    pub fn make_current(&self) {
        unsafe {
            let res = (self.libegl.eglMakeCurrent.unwrap())(
                self.egl_display,
                self.surface,
                self.surface,
                self.egl_context,
            );
            assert!(
                res != 0,
                "eglMakeCurrent failed in CxAndroidDisplay::make_current"
            );
        }
    }

    #[cfg(not(use_vulkan))]
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

    #[cfg(not(use_vulkan))]
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
        if self.surface.is_null() {
            return;
        }

        let res = (self.libegl.eglMakeCurrent.unwrap())(
            self.egl_display,
            self.surface,
            self.surface,
            self.egl_context,
        );

        assert!(res != 0);
    }
}
