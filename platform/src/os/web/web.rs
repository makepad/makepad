use {
    self::super::{from_wasm::*, to_wasm::*, web_media::CxWebMedia},
    crate::{
        cx::{Cx, OsType},
        cx_api::{CxOsApi, CxOsOp, OpenUrlInPlace},
        draw_pass::CxDrawPassParent,
        event::{
            DragEvent, DragItem, DragResponse, DropEvent, Event, KeyModifiers, MouseDownEvent,
            MouseMoveEvent, MouseUpEvent, NetworkResponse, ScrollEvent, TextClipboardEvent,
            TimerEvent, ToWasmMsgEvent, TouchUpdateEvent,
            VideoDecodingErrorEvent, VideoPlaybackCompletedEvent, VideoPlaybackPreparedEvent,
            VideoPlaybackResourcesReleasedEvent, VideoSource, VideoTextureUpdatedEvent, WindowGeom,
        },
        file_dialogs::{
            assemble_virtual_files, FileDialog, FileDialogAction, VirtualFileData,
        },
        makepad_live_id::*,
        makepad_wasm_bridge::{FromWasm, FromWasmMsg, ToWasm, ToWasmMsg, WasmDataU8},
        permission::{Permission, PermissionResult, PermissionStatus},
        storage::{
            StorageError, StorageEstimate, StorageList, StorageOp, StorageRequestId,
            StorageRequestKind, StorageResult, StorageStat,
        },
        thread::{lock_from_ui, SignalToUI},
        HttpError, HttpProgress, HttpResponse, Vec2d,
    },
    std::{
        cell::RefCell,
        panic,
        rc::Rc,
        sync::{Arc, Mutex},
    },
};

impl Cx {
    fn web_key_modifiers(modifiers: u32) -> KeyModifiers {
        KeyModifiers {
            shift: modifiers & 1 != 0,
            control: modifiers & 2 != 0,
            alt: modifiers & 4 != 0,
            logo: modifiers & 8 != 0,
        }
    }

    fn web_virtual_files(
        files: Vec<WVirtualFile>,
        limits: crate::VirtualFileLimits,
    ) -> Result<Vec<crate::VirtualFile>, String> {
        assemble_virtual_files(
            files
                .into_iter()
                .map(|file| VirtualFileData {
                    name: file.name,
                    mime: file.mime,
                    bytes: file.bytes.into_vec_u8(),
                })
                .collect(),
            limits,
        )
    }

    fn web_file_dialog_accept(dialog: &FileDialog) -> String {
        let mut accept = Vec::<String>::new();
        for filter in &dialog.filters {
            for extension in &filter.extensions {
                let extension = extension.trim().trim_start_matches('*').trim_start_matches('.');
                if extension.is_empty() {
                    return String::new();
                }
                let extension = format!(".{extension}");
                if !accept.iter().any(|item| item.eq_ignore_ascii_case(&extension)) {
                    accept.push(extension);
                }
            }
        }
        accept.join(",")
    }

    /// The direct path stays unavailable: WebGL readback must not synchronously
    /// stall the browser's UI thread. Use `request_render_texture_capture`.
    pub fn debug_read_render_texture(
        &mut self,
        _texture: &crate::texture::Texture,
    ) -> Option<(usize, usize, Vec<u8>)> {
        None
    }

    /// Queue a WebGL2 PBO/fence readback. The bridge polls the fence on later
    /// animation frames and returns raw RGBA8 bytes without blocking this call.
    pub fn request_render_texture_capture(&mut self, texture: &crate::texture::Texture) -> bool {
        let tid = texture.texture_id();
        let Some(alloc) = self.textures[tid].alloc.as_ref() else { return false };
        if alloc.width == 0 || alloc.height == 0 {
            return false;
        }
        self.os.from_wasm(FromWasmRequestRenderTextureCapture {
            texture_id: tid.0,
        });
        true
    }

    #[allow(clippy::type_complexity)]
    pub fn take_render_texture_captures(
        &mut self,
    ) -> Vec<(crate::texture::TextureId, usize, usize, Vec<u8>)> {
        std::mem::take(&mut self.os.render_texture_captures)
    }

    fn normalize_web_pathname(pathname: &str) -> String {
        let trimmed = pathname.trim();
        if trimmed.is_empty() {
            "/".to_string()
        } else if trimmed.starts_with('/') {
            trimmed.to_string()
        } else {
            format!("/{}", trimmed)
        }
    }

    fn split_web_location(url: &str) -> (String, String, String) {
        let mut input = url.trim();

        if let Some(scheme_idx) = input.find("://") {
            let after_scheme = &input[(scheme_idx + 3)..];
            input = match after_scheme.find(['/', '?', '#']) {
                Some(path_idx) => &after_scheme[path_idx..],
                None => "/",
            };
        }

        let (before_hash, hash) = match input.split_once('#') {
            Some((base, hash)) => (base, format!("#{}", hash)),
            None => (input, String::new()),
        };
        let (path, search) = match before_hash.split_once('?') {
            Some((path, query)) => (path, format!("?{}", query)),
            None => (before_hash, String::new()),
        };

        (Self::normalize_web_pathname(path), search, hash)
    }

    fn update_web_location_state(
        &mut self,
        pathname: String,
        search: String,
        hash: String,
    ) -> bool {
        let OsType::Web(params) = &mut self.os_type else {
            return false;
        };

        if params.pathname == pathname && params.search == search && params.hash == hash {
            return false;
        }

        params.pathname = pathname;
        params.search = search;
        params.hash = hash;
        true
    }

    // incoming to_wasm. There is absolutely no other entrypoint
    // to general rust codeflow than this function. Only the allocators and init
    pub fn process_to_wasm(&mut self, msg_ptr: u32) -> u32 {
        // A panic=abort trap cannot run dispatch guards. Every JS ingress is
        // a fresh top-level dispatch, so clear the bookkeeping it may leave.
        self.reset_event_dispatch_state();
        let mut to_wasm_msg = ToWasmMsg::take_ownership(msg_ptr);
        let mut network_responses = Vec::new();
        let mut storage_responses = Vec::new();
        self.os.from_wasm = Some(FromWasmMsg::new());
        let mut to_wasm = to_wasm_msg.as_ref();
        let mut is_animation_frame = None;
        while !to_wasm.was_last_block() {
            let block_id = LiveId(to_wasm.read_u64());
            let skip = to_wasm.read_block_skip();
            match block_id {
                live_id!(ToWasmInit) => {
                    let tw = ToWasmInit::read_to_wasm(&mut to_wasm);
                    #[cfg(target_feature = "atomics")]
                    crate::web_alloc::prefill_main_thread_cache();
                    self.cpu_cores = (tw.cpu_cores as usize).max(1);
                    // A browser may accept a 4 GiB wasm maximum, but elastic
                    // caches must still target the 1 GiB deployment class; a
                    // phone gets the fixed phone budget regardless of what the
                    // bridge could reserve.
                    self.memory_budget_bytes = if tw.browser_info.is_phone {
                        crate::cx::PHONE_WEB_MEMORY_BUDGET_BYTES
                    } else {
                        (tw.wasm_memory_max_pages as usize)
                            .saturating_mul(64 * 1024)
                            .clamp(64 * 1024 * 1024, 1024 * 1024 * 1024)
                    };
                    crate::thread::set_web_available_parallelism(self.cpu_cores);
                    self.gpu_info.init_from_info(
                        tw.gpu_info.min_uniform_vectors,
                        tw.gpu_info.vendor,
                        tw.gpu_info.renderer,
                    );
                    self.os_type = tw.browser_info.into();
                    self.xr_capabilities = tw.xr_capabilities.into();
                    let mut new_geom: WindowGeom = tw.window_info.into();
                    self.os.native_window_geom = new_geom.clone();
                    if let Some(id_zero) = self.windows.current_id_zero() {
                        let window = &mut self.windows[id_zero];
                        window.os_dpi_factor = Some(new_geom.dpi_factor);
                        new_geom = window.native_window_geom_to_layout(new_geom);
                        window.window_geom = new_geom.clone();
                    }
                    self.os.window_geom = new_geom;
                    //self.default_inner_window_size = self.os.window_geom.inner_size;

                    self.set_physical_keyboard_state(true);
                    self.call_event_handler(&Event::Startup);
                    self.redraw_all();
                }

                live_id!(ToWasmResizeWindow) => {
                    let tw = ToWasmResizeWindow::read_to_wasm(&mut to_wasm);
                    if let Some(event) = self.windows.web_resize_window_geom(
                        &mut self.os.native_window_geom,
                        &mut self.os.window_geom,
                        tw.window_info.into(),
                    ) {
                        self.call_event_handler(&Event::WindowGeomChange(event));
                        self.redraw_all();
                    }
                }

                live_id!(ToWasmAnimationFrame) => {
                    let tw = ToWasmAnimationFrame::read_to_wasm(&mut to_wasm);
                    is_animation_frame = Some(tw.time);
                    if self.new_next_frames.len() != 0 {
                        self.call_next_frame_event(tw.time);
                    }
                }

                live_id!(ToWasmTouchUpdate) => {
                    let mut e: TouchUpdateEvent = ToWasmTouchUpdate::read_to_wasm(&mut to_wasm).into();
                    let window_id = e.window_id;
                    for touch in e.touches.iter_mut() {
                        self.dpi_override_scale(&mut touch.abs, window_id);
                    }
                    self.fingers.process_touch_update_start(e.time, &e.touches);
                    let e = Event::TouchUpdate(e);
                    self.call_event_handler(&e);
                    let e = if let Event::TouchUpdate(e) = e {
                        e
                    } else {
                        panic!()
                    };
                    self.fingers.process_touch_update_end(&e.touches);
                }

                live_id!(ToWasmMouseDown) => {
                    let mut e: MouseDownEvent = ToWasmMouseDown::read_to_wasm(&mut to_wasm).into();
                    self.dpi_override_scale(&mut e.abs, e.window_id);
                    self.fingers.process_tap_count(e.abs, e.time);
                    self.fingers.mouse_down(e.button, e.window_id);
                    self.call_event_handler(&Event::MouseDown(e))
                }

                live_id!(ToWasmMouseMove) => {
                    let mut e: MouseMoveEvent = ToWasmMouseMove::read_to_wasm(&mut to_wasm).into();
                    self.dpi_override_scale(&mut e.abs, e.window_id);
                    self.call_event_handler(&Event::MouseMove(e.into()));
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                    self.fingers.switch_captures();
                }

                live_id!(ToWasmMouseUp) => {
                    let mut e: MouseUpEvent = ToWasmMouseUp::read_to_wasm(&mut to_wasm).into();
                    self.dpi_override_scale(&mut e.abs, e.window_id);
                    let button = e.button;
                    self.call_event_handler(&Event::MouseUp(e.into()));
                    self.fingers.mouse_up(button);
                    self.fingers.cycle_hover_area(live_id!(mouse).into());
                }

                live_id!(ToWasmScroll) => {
                    let mut e: ScrollEvent = ToWasmScroll::read_to_wasm(&mut to_wasm).into();
                    self.dpi_override_scale(&mut e.abs, e.window_id);
                    self.call_event_handler(&Event::Scroll(e.into()));
                }

                live_id!(ToWasmKeyDown) => {
                    let tw = ToWasmKeyDown::read_to_wasm(&mut to_wasm);
                    self.keyboard.process_key_down(tw.key.clone().into());
                    self.call_event_handler(&Event::KeyDown(tw.key.into()));
                }

                live_id!(ToWasmKeyUp) => {
                    let tw = ToWasmKeyUp::read_to_wasm(&mut to_wasm);
                    self.keyboard.process_key_up(tw.key.clone().into());
                    self.call_event_handler(&Event::KeyUp(tw.key.into()));
                }

                live_id!(ToWasmTextInput) => {
                    let tw = ToWasmTextInput::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&Event::TextInput(tw.into()));
                }

                live_id!(ToWasmTextCopy) => {
                    let response = Rc::new(RefCell::new(None));
                    self.call_event_handler(&Event::TextCopy(TextClipboardEvent {
                        response: response.clone(),
                    }));
                    let response = response.borrow_mut().take();
                    if let Some(response) = response {
                        self.os.from_wasm(FromWasmTextCopyResponse { response });
                    }
                }

                live_id!(ToWasmStorageResult) => {
                    let tw = ToWasmStorageResult::read_to_wasm(&mut to_wasm);
                    let request_id = StorageRequestId(
                        tw.request_id_lo as u64 | ((tw.request_id_hi as u64) << 32),
                    );
                    let Some(op) = StorageOp::from_u32(tw.op) else {
                        if let Some(response) = self.finish_web_storage_protocol_error(
                            request_id,
                            format!("storage response had unknown operation {}", tw.op),
                        ) {
                            storage_responses.push(response);
                        }
                        to_wasm.block_skip(skip);
                        continue;
                    };
                    let result = if !tw.error.is_empty() {
                        Err(if tw.error_kind == 1 {
                            StorageError::QuotaExceeded(tw.error)
                        } else {
                            StorageError::Backend(tw.error)
                        })
                    } else {
                        Ok(match op {
                            StorageOp::Get | StorageOp::GetRange => StorageResult::Value(
                                tw.found.then(|| tw.value.into_vec_u8()),
                            ),
                            StorageOp::Set | StorageOp::Delete => StorageResult::Unit,
                            StorageOp::List => StorageResult::List(StorageList {
                                keys: tw.keys,
                                next_cursor: tw.has_next.then_some(tw.next),
                            }),
                            StorageOp::Stat => StorageResult::Stat(tw.found.then_some(
                                StorageStat {
                                    len: tw.length_lo as u64
                                        | ((tw.length_hi as u64) << 32),
                                },
                            )),
                            StorageOp::Estimate => StorageResult::Estimate(StorageEstimate {
                                usage: tw.usage_lo as u64 | ((tw.usage_hi as u64) << 32),
                                quota: tw.quota_lo as u64 | ((tw.quota_hi as u64) << 32),
                            }),
                        })
                    };
                    if let Some(response) =
                        self.finish_web_storage_request(request_id, op, result)
                    {
                        storage_responses.push(response);
                    }
                }
                live_id!(ToWasmRenderTextureCapture) => {
                    let tw = ToWasmRenderTextureCapture::read_to_wasm(&mut to_wasm);
                    if tw.error.is_empty() {
                        if let Some(texture_id) = self.textures.id_at_index(tw.texture_id) {
                            self.os.render_texture_captures.push((
                                texture_id,
                                tw.width,
                                tw.height,
                                tw.data.into_vec_u8(),
                            ));
                            self.redraw_all();
                        }
                    } else {
                        crate::error!("web render texture capture failed: {}", tw.error);
                        if let Some(texture_id) = self.textures.id_at_index(tw.texture_id) {
                            // Wake the waiting owner immediately. Zero geometry is
                            // the existing capture tuple's unambiguous failure form.
                            self.os.render_texture_captures.push((
                                texture_id,
                                0,
                                0,
                                Vec::new(),
                            ));
                            self.redraw_all();
                        }
                    }
                }

                live_id!(ToWasmSignal) => {
                    let tw = ToWasmSignal::read_to_wasm(&mut to_wasm);
                    if tw.flags & 1 != 0 {
                        self.handle_media_signals();
                        self.handle_script_signals();
                        self.call_event_handler(&Event::Signal);
                        self.dispatch_network_runtime_events();
                    }
                    if tw.flags & 2 != 0 {
                        self.handle_action_receiver();
                    }
                }

                live_id!(ToWasmAppLifecycle) => {
                    let tw = ToWasmAppLifecycle::read_to_wasm(&mut to_wasm);
                    match tw.state {
                        0 => {
                            self.call_event_handler(&Event::Foreground);
                            self.redraw_all();
                        }
                        1 => {
                            self.call_event_handler(&Event::Background);
                        }
                        2 => {
                            self.call_event_handler(&Event::Pause);
                        }
                        3 => {
                            self.call_event_handler(&Event::Resume);
                            self.redraw_all();
                        }
                        4 => {
                            self.call_event_handler(&Event::Shutdown);
                            self.close_task_pool();
                            self.thread_spawner.close_runtime();
                        }
                        _ => {}
                    }
                }

                live_id!(ToWasmTimerFired) => {
                    let tw = ToWasmTimerFired::read_to_wasm(&mut to_wasm);
                    let e = TimerEvent {
                        timer_id: tw.timer_id as u64,
                        time: None,
                    };
                    self.handle_script_timer(&e);
                    self.call_event_handler(&Event::Timer(e));
                }

                live_id!(ToWasmWindowGotFocus) => {
                    self.call_window_zero_focus_event(true);
                }

                live_id!(ToWasmWindowLostFocus) => {
                    self.call_window_zero_focus_event(false);
                }

                live_id!(ToWasmRedrawAll) => {
                    self.redraw_all();
                }

                live_id!(ToWasmWebGLShadersDone) => {
                    let tw = ToWasmWebGLShadersDone::read_to_wasm(&mut to_wasm);
                    self.os.webgl_shaders_pending =
                        self.os.webgl_shaders_pending.saturating_sub(tw.count);
                }

                live_id!(ToWasmPaintDirty) => {
                    if let Some(window_id) = self.windows.current_id_zero() {
                        if let Some(main_pass_id) = self.windows[window_id].main_pass_id {
                            self.passes[main_pass_id].paint_dirty = true;
                        }
                    }
                }

                live_id!(ToWasmLiveFileChange) => {
                    let tw = ToWasmLiveFileChange::read_to_wasm(&mut to_wasm);
                    self.script_data
                        .live_reload
                        .queue_file_change(tw.file_name, tw.content);
                }

                live_id!(ToWasmLocationChange) => {
                    let tw = ToWasmLocationChange::read_to_wasm(&mut to_wasm);
                    if self.update_web_location_state(tw.pathname, tw.search, tw.hash) {
                        self.call_event_handler(&Event::Signal);
                    }
                }

                live_id!(ToWasmFileDrag) => {
                    let tw = ToWasmFileDrag::read_to_wasm(&mut to_wasm);
                    let mut abs = if tw.left {
                        crate::dvec2(-100000.0, -100000.0)
                    } else {
                        crate::dvec2(tw.x, tw.y)
                    };
                    if let Some(window_id) = self.windows.current_id_zero() {
                        self.dpi_override_scale(&mut abs, window_id);
                    }
                    let items = (0..tw.file_count)
                        .map(|_| {
                            DragItem::VirtualFile(crate::VirtualFile {
                                name: String::new(),
                                mime: String::new(),
                                bytes: Arc::from(Vec::<u8>::new()),
                                size: 0,
                            })
                        })
                        .collect();
                    self.call_event_handler(&Event::Drag(DragEvent {
                        modifiers: Self::web_key_modifiers(tw.modifiers),
                        handled: Arc::new(Mutex::new(false)),
                        abs,
                        items: Arc::new(items),
                        response: Arc::new(Mutex::new(DragResponse::None)),
                    }));
                    self.drag_drop.cycle_drag();
                    if tw.left {
                        self.call_event_handler(&Event::DragEnd);
                        self.drag_drop.cycle_drag();
                    }
                }

                live_id!(ToWasmFileDrop) => {
                    let tw = ToWasmFileDrop::read_to_wasm(&mut to_wasm);
                    match Self::web_virtual_files(tw.files, self.file_dialogs.limits()) {
                        Ok(files) => {
                            let mut abs = crate::dvec2(tw.x, tw.y);
                            if let Some(window_id) = self.windows.current_id_zero() {
                                self.dpi_override_scale(&mut abs, window_id);
                            }
                            self.call_event_handler(&Event::Drop(DropEvent {
                                modifiers: Self::web_key_modifiers(tw.modifiers),
                                handled: Arc::new(Mutex::new(false)),
                                abs,
                                items: Arc::new(
                                    files.into_iter().map(DragItem::VirtualFile).collect(),
                                ),
                            }));
                            self.drag_drop.cycle_drag();
                        }
                        Err(error) => crate::error!("web file drop rejected: {error}"),
                    }
                    self.call_event_handler(&Event::DragEnd);
                    self.drag_drop.cycle_drag();
                }

                live_id!(ToWasmFileDropError) => {
                    let tw = ToWasmFileDropError::read_to_wasm(&mut to_wasm);
                    crate::error!("web file drop rejected: {}", tw.error);
                    self.call_event_handler(&Event::DragEnd);
                    self.drag_drop.cycle_drag();
                }

                live_id!(ToWasmFileDialogResult) => {
                    let tw = ToWasmFileDialogResult::read_to_wasm(&mut to_wasm);
                    let id = LiveId::from_lo_hi(tw.id_lo, tw.id_hi);
                    let pending = self.file_dialogs.finish(id);
                    let limits = pending
                        .as_ref()
                        .map(|pending| pending.limits)
                        .unwrap_or_else(|| self.file_dialogs.limits());
                    if pending.is_none() {
                        crate::error!("web file dialog returned unknown id {:?}", id);
                    }
                    let action = if tw.cancelled || !tw.error.is_empty() {
                        if !tw.error.is_empty() {
                            crate::error!("web file dialog failed: {}", tw.error);
                        }
                        FileDialogAction::FileCancelled { id }
                    } else {
                        match Self::web_virtual_files(tw.files, limits) {
                            Ok(files) if !files.is_empty() => {
                                FileDialogAction::FileLoaded { id, files }
                            }
                            Ok(_) => FileDialogAction::FileCancelled { id },
                            Err(error) => {
                                crate::error!("web file dialog rejected: {error}");
                                FileDialogAction::FileCancelled { id }
                            }
                        }
                    };
                    self.action(action);
                    self.handle_actions();
                }

                live_id!(ToWasmHTTPResponse) => {
                    let tw = ToWasmHTTPResponse::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponse::HttpResponse {
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: HttpResponse::from_header_string(
                            LiveId::from_lo_hi(tw.metadata_id_lo, tw.metadata_id_hi),
                            tw.status as u16,
                            tw.headers,
                            Some(tw.body.into_vec_u8()),
                        ),
                    });
                }

                live_id!(ToWasmHttpRequestError) => {
                    let tw = ToWasmHttpRequestError::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponse::HttpError {
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        error: HttpError {
                            metadata_id: LiveId::from_lo_hi(tw.metadata_id_lo, tw.metadata_id_hi),
                            message: tw.error,
                        },
                    });
                }

                live_id!(ToWasmHttpResponseProgress) => {
                    let tw = ToWasmHttpResponseProgress::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponse::HttpProgress {
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        progress: HttpProgress {
                            loaded: tw.loaded as u64,
                            total: tw.total as u64,
                        },
                    });
                }

                live_id!(ToWasmHttpUploadProgress) => {
                    let tw = ToWasmHttpUploadProgress::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponse::HttpProgress {
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        progress: HttpProgress {
                            loaded: tw.loaded as u64,
                            total: tw.total as u64,
                        },
                    });
                }
                live_id!(ToWasmPermissionResult) => {
                    let tw = ToWasmPermissionResult::read_to_wasm(&mut to_wasm);
                    let permission = match tw.permission.as_str() {
                        "microphone" => Permission::AudioInput,
                        "camera" => Permission::Camera,
                        "geolocation" => Permission::Location,
                        _ => {
                            crate::log!("Unknown web permission: {}", tw.permission);
                            continue;
                        }
                    };
                    let status = match tw.status {
                        0 => PermissionStatus::NotDetermined,
                        1 => PermissionStatus::Granted,
                        2 => PermissionStatus::DeniedCanRetry,
                        3 => PermissionStatus::DeniedPermanent,
                        _ => PermissionStatus::DeniedPermanent,
                    };
                    self.call_event_handler(&Event::PermissionResult(PermissionResult {
                        permission,
                        request_id: tw.request_id as i32,
                        status,
                    }));
                }
                live_id!(ToWasmLocationUpdate) => {
                    let tw = ToWasmLocationUpdate::read_to_wasm(&mut to_wasm);
                    self.call_event_handler(&Event::LocationUpdate(
                        crate::event::LocationUpdateEvent {
                            lon: tw.lon,
                            lat: tw.lat,
                            accuracy_m: tw.accuracy_m,
                            altitude_m: tw.altitude_m,
                            speed_mps: tw.speed_mps,
                            heading_deg: tw.heading_deg,
                            time: tw.time,
                        },
                    ));
                }
                live_id!(ToWasmLocationError) => {
                    let tw = ToWasmLocationError::read_to_wasm(&mut to_wasm);
                    let error = if tw.code == 1 {
                        crate::event::LocationErrorEvent::PermissionDenied
                    } else {
                        crate::event::LocationErrorEvent::Unavailable(tw.message)
                    };
                    self.call_event_handler(&Event::LocationError(error));
                }
                /*
                live_id!(ToWasmWebSocketClose) => {
                    let tw = ToWasmWebSocketClose::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketClose
                    });
                }

                live_id!(ToWasmWebSocketOpen) => {
                    let tw = ToWasmWebSocketOpen::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketOpen
                    });
                }

                live_id!(ToWasmWebSocketError) => {
                    let tw = ToWasmWebSocketError::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketError(tw.error)
                    });
                }
                live_id!(ToWasmWebSocketString) => {
                    let tw = ToWasmWebSocketString::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketString(tw.data)
                    });
                }
                live_id!(ToWasmWebSocketBinary) => {
                    let tw = ToWasmWebSocketBinary::read_to_wasm(&mut to_wasm);
                    network_responses.push(NetworkResponseEvent{
                        request_id: LiveId::from_lo_hi(tw.request_id_lo, tw.request_id_hi),
                        response: NetworkResponse::WebSocketBinary(tw.data.into_vec_u8())
                    });
                }*/
                live_id!(ToWasmVideoPlaybackPrepared) => {
                    let tw = ToWasmVideoPlaybackPrepared::read_to_wasm(&mut to_wasm);
                    let video_id = LiveId::from_lo_hi(tw.video_id_lo, tw.video_id_hi);
                    let duration = (tw.duration_lo as u128) | ((tw.duration_hi as u128) << 32);
                    self.call_event_handler(&Event::VideoPlaybackPrepared(
                        VideoPlaybackPreparedEvent {
                            video_id,
                            video_width: tw.video_width,
                            video_height: tw.video_height,
                            duration,
                            is_seekable: duration > 0,
                            video_tracks: if tw.video_width > 0 && tw.video_height > 0 {
                                vec!["video".to_string()]
                            } else {
                                vec![]
                            },
                            audio_tracks: vec!["audio".to_string()],
                        },
                    ));
                }

                live_id!(ToWasmVideoTextureUpdated) => {
                    let tw = ToWasmVideoTextureUpdated::read_to_wasm(&mut to_wasm);
                    let video_id = LiveId::from_lo_hi(tw.video_id_lo, tw.video_id_hi);
                    let current_position_ms =
                        (tw.current_position_lo as u128) | ((tw.current_position_hi as u128) << 32);
                    self.call_event_handler(&Event::VideoTextureUpdated(
                        VideoTextureUpdatedEvent {
                            video_id,
                            current_position_ms,
                            yuv: crate::event::video_playback::VideoYuvMetadata {
                                enabled: false,
                                matrix: 0.0,
                                biplanar: false,
                                full_range: false,
                                rotation_steps: 0.0,
                            external: false,
                            array: false,
                            },
                        rgba_gl_2d: false,
                        },
                    ));
                    self.redraw_all();
                }

                live_id!(ToWasmVideoPlaybackCompleted) => {
                    let tw = ToWasmVideoPlaybackCompleted::read_to_wasm(&mut to_wasm);
                    let video_id = LiveId::from_lo_hi(tw.video_id_lo, tw.video_id_hi);
                    self.call_event_handler(&Event::VideoPlaybackCompleted(
                        VideoPlaybackCompletedEvent { video_id },
                    ));
                }

                live_id!(ToWasmVideoPlaybackResourcesReleased) => {
                    let tw = ToWasmVideoPlaybackResourcesReleased::read_to_wasm(&mut to_wasm);
                    let video_id = LiveId::from_lo_hi(tw.video_id_lo, tw.video_id_hi);
                    self.call_event_handler(&Event::VideoPlaybackResourcesReleased(
                        VideoPlaybackResourcesReleasedEvent { video_id },
                    ));
                }

                live_id!(ToWasmAudioDeviceList) => {
                    let tw = ToWasmAudioDeviceList::read_to_wasm(&mut to_wasm);
                    lock_from_ui(&self.os.web_audio()).to_wasm_audio_device_list(tw);
                }
                live_id!(ToWasmMidiPortList) => {
                    let tw = ToWasmMidiPortList::read_to_wasm(&mut to_wasm);
                    self.os
                        .web_midi()
                        .lock()
                        .unwrap()
                        .to_wasm_midi_port_list(tw);
                }
                live_id!(ToWasmMidiInputData) => {
                    let tw = ToWasmMidiInputData::read_to_wasm(&mut to_wasm);
                    self.os
                        .web_midi()
                        .lock()
                        .unwrap()
                        .to_wasm_midi_input_data(tw);
                }
                msg_id => {
                    // swap the message into an event to avoid a copy
                    let offset = to_wasm.u32_offset;
                    drop(to_wasm);
                    let event = Event::ToWasmMsg(ToWasmMsgEvent {
                        id: msg_id,
                        msg: to_wasm_msg,
                        offset,
                    });
                    self.call_event_handler(&event);
                    // and swap it back
                    if let Event::ToWasmMsg(ToWasmMsgEvent { msg, .. }) = event {
                        to_wasm_msg = msg
                    } else {
                        panic!()
                    };
                    to_wasm = to_wasm_msg.as_ref();
                }
            };
            to_wasm.block_skip(skip);
        }

        if let Some(time) = is_animation_frame {
            if self.need_redrawing() {
                self.call_draw_event(time);
            }
            self.handle_repaint(time);
        }

        if network_responses.len() != 0 {
            self.handle_script_network_events(&network_responses);
            self.call_event_handler(&Event::NetworkResponses(network_responses));
        }

        if !storage_responses.is_empty() {
            self.call_event_handler(&Event::Storage(storage_responses));
        }

        self.run_live_edit_if_needed("web");

        self.handle_platform_ops();
        self.handle_media_signals();

        if self.any_passes_dirty()
            || self.need_redrawing()
            || self.new_next_frames.len() != 0
            || self.demo_time_repaint
        {
            self.os.from_wasm(FromWasmRequestAnimationFrame {});
        }

        //return wasm pointer to caller
        self.os.from_wasm.take().unwrap().release_ownership()
    }

    pub fn handle_repaint(&mut self, time: f64) {
        let mut passes_todo = Vec::new();

        self.compute_pass_repaint_order(&mut passes_todo);
        self.repaint_id += 1;
        for draw_pass_id in &passes_todo {
            let uniforms_gen = self.next_uniform_gen();
            self.passes[*draw_pass_id].set_time(time as f32, uniforms_gen);
            match self.passes[*draw_pass_id].parent.clone() {
                CxDrawPassParent::Xr => {}
                CxDrawPassParent::Window(window_id) => {
                    // ONE canvas: only window zero paints. A second window's
                    // pass is recorded but never presented (see CreateWindow
                    // below) — settled here, so it neither errors nor keeps
                    // requesting frames.
                    if self.windows.current_id_zero() == Some(window_id) {
                        self.draw_pass_to_canvas(*draw_pass_id);
                    } else {
                        self.passes[*draw_pass_id].paint_dirty = false;
                    }
                }
                CxDrawPassParent::DrawPass(_) => {
                    //let dpi_factor = self.get_delegated_dpi_factor(parent_pass_id);
                    self.draw_pass_to_texture(*draw_pass_id);
                }
                CxDrawPassParent::None => {
                    self.draw_pass_to_texture(*draw_pass_id);
                }
            }
        }
    }

    // empty stub
    pub fn event_loop<F>(&mut self, mut _event_handler: F)
    where
        F: FnMut(&mut Cx, Event),
    {
    }

    fn handle_platform_ops(&mut self) {
        while let Some(op) = self.platform_ops.pop_front() {
            match op {
                CxOsOp::CreateWindow(window_id) => {
                    let title = {
                        let window = &mut self.windows[window_id];
                        window.create_title.clone()
                    };
                    // The browser gives an app one canvas, so the platform
                    // has one window: a second `Window` is NOT created — it
                    // never becomes `is_created`, its pass never paints —
                    // and that is said once. `OsType::is_single_window` says
                    // it up front, so an app hosts that surface in-page
                    // instead of asking.
                    if self.windows.current_id_zero().is_some_and(|zero| zero != window_id) {
                        if !self.os.second_window_reported {
                            self.os.second_window_reported = true;
                            crate::log!(
                                "web: one canvas, one window — {:?} {title:?} is not created (OsType::is_single_window)",
                                window_id
                            );
                        }
                        continue;
                    }

                    self.os.from_wasm(FromWasmSetDocumentTitle { title });

                    let event = self.windows.web_create_window_geom(
                        window_id,
                        &self.os.native_window_geom,
                        &mut self.os.window_geom,
                    );
                    self.call_event_handler(&Event::WindowGeomChange(event));

                    self.windows[window_id].is_created = true;
                    self.redraw_all();
                }
                CxOsOp::CreatePopupWindow {
                    window_id,
                    parent_window_id,
                    position,
                    size,
                    grab_keyboard,
                } => {
                    let parent_os_dpi = self.windows[parent_window_id].os_dpi_factor;
                    let mut geom = self.os.window_geom.clone();
                    geom.position = position;
                    geom.inner_size = size;
                    geom.outer_size = size;
                    let window = &mut self.windows[window_id];
                    window.os_dpi_factor = parent_os_dpi;
                    window.window_geom = geom;
                    window.is_popup = true;
                    window.popup_parent = Some(parent_window_id);
                    window.popup_position = Some(position);
                    window.popup_size = Some(size);
                    window.popup_grab_keyboard = grab_keyboard;
                    window.is_created = true;
                }
                CxOsOp::FullscreenWindow(_window_id) => {
                    self.os.from_wasm(FromWasmFullScreen {});
                }
                CxOsOp::NormalizeWindow(_window_id) => {
                    self.os.from_wasm(FromWasmNormalScreen {});
                }
                CxOsOp::SetWindowTitle(_window_id, title) => {
                    self.os.from_wasm(FromWasmSetDocumentTitle { title });
                }
                CxOsOp::SetWindowVisuals(_, _) => {}
                CxOsOp::XrStartPresenting => {
                    self.os.from_wasm(FromWasmXrStartPresenting {});
                }
                CxOsOp::XrSetRenderScale(_) => {}
                CxOsOp::XrStopPresenting => {
                    self.os.from_wasm(FromWasmXrStopPresenting {});
                }
                CxOsOp::ShowTextIME(area, cursor_rect, _config) => {
                    // Bottom of the caret line (matches the pre-rect point); the
                    // hidden-textarea IME anchor only takes a point.
                    let pos = area.clipped_rect(self).pos + cursor_rect.pos + cursor_rect.size;
                    let Some(window_id) = self
                        .get_window_id_of(&area)
                        .filter(|window_id| self.windows.is_valid(*window_id))
                        .or_else(|| self.windows.current_id_zero())
                    else {
                        continue;
                    };
                    let pos = self.windows[window_id].layout_vec2d_to_native_points(pos);
                    self.os
                        .from_wasm(FromWasmShowTextIME { x: pos.x, y: pos.y });
                }
                CxOsOp::HideTextIME => {
                    self.os.from_wasm(FromWasmHideTextIME {});
                }
                CxOsOp::CopyToClipboard(_) => {
                    crate::error!("Clipboard actions not supported in web")
                }
                CxOsOp::SetPrimarySelection(_) => {}
                CxOsOp::ShowSelectionHandles { .. } => {}
                CxOsOp::UpdateSelectionHandles { .. } => {}
                CxOsOp::HideSelectionHandles => {}
                CxOsOp::AccessibilityUpdate(_) => {}
                CxOsOp::StartDragging(items) => {
                    self.drag_drop.start_internal_drag(items);
                }
                CxOsOp::StartExternalDragging { .. } => {
                    crate::error!("external file dragging is not implemented on Web");
                    self.call_event_handler(&Event::DragEnd);
                }
                CxOsOp::SetCursor(cursor) => {
                    self.os.from_wasm(FromWasmSetMouseCursor::new(cursor));
                }
                CxOsOp::StartTimer {
                    timer_id,
                    interval,
                    repeats,
                } => {
                    self.os.from_wasm(FromWasmStartTimer {
                        repeats,
                        interval,
                        timer_id: timer_id as f64,
                    });
                }
                CxOsOp::StopTimer(timer_id) => {
                    self.os.from_wasm(FromWasmStopTimer {
                        timer_id: timer_id as f64,
                    });
                }
                CxOsOp::StartLocationUpdates => {
                    self.os.from_wasm(FromWasmStartLocationUpdates {});
                }
                CxOsOp::StopLocationUpdates => {
                    self.os.from_wasm(FromWasmStopLocationUpdates {});
                }
                CxOsOp::HttpRequest {
                    request_id,
                    request,
                } => {
                    let headers = request.get_headers_string();
                    self.os.from_wasm(FromWasmHTTPRequest {
                        request_id_lo: request_id.lo(),
                        request_id_hi: request_id.hi(),
                        metadata_id_lo: request.metadata_id.lo(),
                        metadata_id_hi: request.metadata_id.hi(),
                        url: request.url,
                        method: request.method.to_string().into(),
                        headers: headers,
                        body: WasmDataU8::from_vec_u8(request.body.unwrap_or(Vec::new())),
                    });
                }
                CxOsOp::CancelHttpRequest { request_id } => {
                    self.os.from_wasm(FromWasmCancelHTTPRequest {
                        request_id_lo: request_id.lo(),
                        request_id_hi: request_id.hi(),
                    });
                }
                CxOsOp::StorageRequest(request) => {
                    let request_id_lo = request.request_id.0 as u32;
                    let request_id_hi = (request.request_id.0 >> 32) as u32;
                    let namespace = request.namespace;
                    match request.kind {
                        StorageRequestKind::Get { key } => {
                            self.os.from_wasm(FromWasmStorageGet {
                                request_id_lo,
                                request_id_hi,
                                namespace,
                                key,
                            });
                        }
                        StorageRequestKind::Set { key, value } => {
                            self.os.from_wasm(FromWasmStorageSet {
                                request_id_lo,
                                request_id_hi,
                                namespace,
                                key,
                                value: WasmDataU8::from_vec_u8(value),
                            });
                        }
                        StorageRequestKind::Delete { key } => {
                            self.os.from_wasm(FromWasmStorageDelete {
                                request_id_lo,
                                request_id_hi,
                                namespace,
                                key,
                            });
                        }
                        StorageRequestKind::List {
                            prefix,
                            after,
                            limit,
                        } => {
                            let has_after = after.is_some();
                            self.os.from_wasm(FromWasmStorageList {
                                request_id_lo,
                                request_id_hi,
                                namespace,
                                prefix,
                                after: after.unwrap_or_default(),
                                has_after,
                                limit,
                            });
                        }
                        StorageRequestKind::GetRange {
                            key,
                            offset,
                            len,
                        } => {
                            self.os.from_wasm(FromWasmStorageGetRange {
                                request_id_lo,
                                request_id_hi,
                                namespace,
                                key,
                                offset_lo: offset as u32,
                                offset_hi: (offset >> 32) as u32,
                                len,
                            });
                        }
                        StorageRequestKind::Stat { key } => {
                            self.os.from_wasm(FromWasmStorageStat {
                                request_id_lo,
                                request_id_hi,
                                namespace,
                                key,
                            });
                        }
                        StorageRequestKind::Estimate => {
                            self.os.from_wasm(FromWasmStorageEstimate {
                                request_id_lo,
                                request_id_hi,
                                namespace,
                            });
                        }
                    }
                }
                CxOsOp::StorageRequestError {
                    request_id,
                    op,
                    error,
                } => {
                    if let Some(response) =
                        self.finish_web_storage_request(request_id, op, Err(error))
                    {
                        self.call_event_handler(&Event::Storage(vec![response]));
                    }
                }
                CxOsOp::CheckPermission {
                    permission,
                    request_id,
                } => match permission {
                    Permission::AudioInput | Permission::Camera | Permission::Location => {
                        let permission_str = match permission {
                            Permission::AudioInput => "microphone",
                            Permission::Camera => "camera",
                            Permission::Location => "geolocation",
                            Permission::HeadsetCamera | Permission::SceneAccess => unreachable!(),
                        };
                        self.os.from_wasm(FromWasmCheckPermission {
                            permission: permission_str.to_string(),
                            request_id: request_id as u32,
                        });
                    }
                    Permission::HeadsetCamera | Permission::SceneAccess => {
                        self.call_event_handler(&Event::PermissionResult(PermissionResult {
                            permission,
                            request_id,
                            status: PermissionStatus::DeniedPermanent,
                        }));
                    }
                },
                CxOsOp::RequestPermission {
                    permission,
                    request_id,
                } => match permission {
                    Permission::AudioInput | Permission::Camera | Permission::Location => {
                        let permission_str = match permission {
                            Permission::AudioInput => "microphone",
                            Permission::Camera => "camera",
                            Permission::Location => "geolocation",
                            Permission::HeadsetCamera | Permission::SceneAccess => unreachable!(),
                        };
                        self.os.from_wasm(FromWasmRequestPermission {
                            permission: permission_str.to_string(),
                            request_id: request_id as u32,
                        });
                    }
                    Permission::HeadsetCamera | Permission::SceneAccess => {
                        self.call_event_handler(&Event::PermissionResult(PermissionResult {
                            permission,
                            request_id,
                            status: PermissionStatus::DeniedPermanent,
                        }));
                    }
                },
                CxOsOp::PrepareVideoPlayback(
                    video_id,
                    source,
                    _camera_preview_mode,
                    _external_texture_id,
                    texture_id,
                    autoplay,
                    should_loop,
                ) => match source {
                    VideoSource::Network(url) => {
                        self.os.from_wasm(FromWasmPrepareVideoPlayback {
                            video_id_lo: video_id.lo(),
                            video_id_hi: video_id.hi(),
                            texture_id: texture_id.0,
                            source_url: url,
                            autoplay,
                            should_loop,
                        });
                    }
                    VideoSource::InMemory(_) => {
                        let error = "VideoSource::InMemory is not supported on web".to_string();
                        crate::error!("{}", error);
                        self.call_event_handler(&Event::VideoDecodingError(
                            VideoDecodingErrorEvent { video_id, error },
                        ));
                    }
                    VideoSource::Filesystem(_) => {
                        let error = "VideoSource::Filesystem is not supported on web".to_string();
                        crate::error!("{}", error);
                        self.call_event_handler(&Event::VideoDecodingError(
                            VideoDecodingErrorEvent { video_id, error },
                        ));
                    }
                    VideoSource::Camera(..) => {
                        let error = "VideoSource::Camera is not supported on web".to_string();
                        crate::error!("{}", error);
                        self.call_event_handler(&Event::VideoDecodingError(
                            VideoDecodingErrorEvent { video_id, error },
                        ));
                    }
                    VideoSource::PlaybackSession(..) | VideoSource::Session(..) => {
                        let error = "VideoSource::Session is not supported on web".to_string();
                        crate::error!("{}", error);
                        self.call_event_handler(&Event::VideoDecodingError(
                            VideoDecodingErrorEvent { video_id, error },
                        ));
                    }
                },
                CxOsOp::BeginVideoPlayback(video_id) => {
                    self.os.from_wasm(FromWasmBeginVideoPlayback {
                        video_id_lo: video_id.lo(),
                        video_id_hi: video_id.hi(),
                    });
                }
                CxOsOp::PauseVideoPlayback(video_id) => {
                    self.os.from_wasm(FromWasmPauseVideoPlayback {
                        video_id_lo: video_id.lo(),
                        video_id_hi: video_id.hi(),
                    });
                }
                CxOsOp::ResumeVideoPlayback(video_id) => {
                    self.os.from_wasm(FromWasmResumeVideoPlayback {
                        video_id_lo: video_id.lo(),
                        video_id_hi: video_id.hi(),
                    });
                }
                CxOsOp::MuteVideoPlayback(video_id) => {
                    self.os.from_wasm(FromWasmMuteVideoPlayback {
                        video_id_lo: video_id.lo(),
                        video_id_hi: video_id.hi(),
                    });
                }
                CxOsOp::UnmuteVideoPlayback(video_id) => {
                    self.os.from_wasm(FromWasmUnmuteVideoPlayback {
                        video_id_lo: video_id.lo(),
                        video_id_hi: video_id.hi(),
                    });
                }
                CxOsOp::SeekVideoPlayback(video_id, position_ms) => {
                    self.os.from_wasm(FromWasmSeekVideoPlayback {
                        video_id_lo: video_id.lo(),
                        video_id_hi: video_id.hi(),
                        position_ms_lo: (position_ms & 0xFFFFFFFF) as u32,
                        position_ms_hi: ((position_ms >> 32) & 0xFFFFFFFF) as u32,
                    });
                }
                CxOsOp::CleanupVideoPlaybackResources(video_id) => {
                    self.os.from_wasm(FromWasmCleanupVideoPlaybackResources {
                        video_id_lo: video_id.lo(),
                        video_id_hi: video_id.hi(),
                    });
                }
                CxOsOp::UpdateVideoSurfaceTexture(_) => {
                    // On web, texture updates happen in the JS animation frame loop
                }
                // New ops — no-op on Web (not yet wired to JS)
                CxOsOp::AttachCameraNativePreview { .. }
                | CxOsOp::UpdateCameraNativePreview { .. }
                | CxOsOp::DetachCameraNativePreview { .. } => {}
                CxOsOp::SetVideoVolume(_, _) => {}
                CxOsOp::SetVideoPlaybackRate(_, _) => {}
                CxOsOp::PrepareAudioPlayback(_, _, _, _) => {}
                // Track selection is currently implemented on Linux GStreamer only.
                CxOsOp::SelectVideoTrack(_, _) | CxOsOp::SelectAudioTrack(_, _) => {}
                CxOsOp::SelectFileDialog(dialog) => {
                    if !dialog.want_bytes {
                        crate::log!(
                            "web file dialog has no filesystem paths; returning FileLoaded bytes"
                        );
                    }
                    let limits = self.file_dialogs.limits();
                    self.os.from_wasm(FromWasmSelectFileDialog {
                        id_lo: dialog.id.lo(),
                        id_hi: dialog.id.hi(),
                        accept: Self::web_file_dialog_accept(&dialog),
                        multiple: dialog.multiple,
                        max_file_size: limits.max_file_size as f64,
                        max_total_size: limits.max_total_size as f64,
                    });
                }
                CxOsOp::SaveFileDialog(dialog) => {
                    crate::error!("web save file dialogs are not supported; download support is pending");
                    self.action(FileDialogAction::FileCancelled { id: dialog.id });
                    self.handle_actions();
                }
                e => {
                    crate::error!("Not implemented on this platform: CxOsOp::{:?}", e);
                } /*
                  CxOsOp::WebSocketOpen{request_id, request}=>{
                      let headers = request.get_headers_string();
                      self.os.from_wasm(FromWasmWebSocketOpen {
                          url: request.url,
                          method: request.method.to_string().into(),
                          headers: headers,
                          body: WasmDataU8::from_vec_u8(request.body.unwrap_or(Vec::new())),
                          request_id_lo: request_id.lo(),
                          request_id_hi: request_id.hi(),
                      });
                  }
                  CxOsOp::WebSocketSendBinary{request_id, data}=>{
                      self.os.from_wasm(FromWasmWebSocketSendBinary {
                          request_id_lo: request_id.lo(),
                          request_id_hi: request_id.hi(),
                          data: WasmDataU8::from_vec_u8(data)
                      });
                  }
                  CxOsOp::WebSocketSendString{request_id, data}=>{
                      self.os.from_wasm(FromWasmWebSocketSendString {
                          request_id_lo: request_id.lo(),
                          request_id_hi: request_id.hi(),
                          data
                      });
                  },*/
            }
        }
    }
}

impl CxOsApi for Cx {
    fn init_cx_os(&mut self) {
        super::web_network::install_network_backend_shim();
        self.package_root = Some(String::new());
        self.os.start_time = Self::monotonic_now();

        self.os.append_to_wasm_js(&[
            ToWasmInit::to_js_code(),
            ToWasmResizeWindow::to_js_code(),
            ToWasmAnimationFrame::to_js_code(),
            ToWasmTouchUpdate::to_js_code(),
            ToWasmMouseDown::to_js_code(),
            ToWasmMouseMove::to_js_code(),
            ToWasmMouseUp::to_js_code(),
            ToWasmScroll::to_js_code(),
            ToWasmKeyDown::to_js_code(),
            ToWasmKeyUp::to_js_code(),
            ToWasmTextInput::to_js_code(),
            ToWasmTextCopy::to_js_code(),
            ToWasmStorageResult::to_js_code(),
            ToWasmRenderTextureCapture::to_js_code(),
            ToWasmTimerFired::to_js_code(),
            ToWasmPaintDirty::to_js_code(),
            ToWasmRedrawAll::to_js_code(),
            ToWasmWebGLShadersDone::to_js_code(),
            ToWasmLiveFileChange::to_js_code(),
            ToWasmLocationChange::to_js_code(),
            ToWasmWindowGotFocus::to_js_code(),
            ToWasmWindowLostFocus::to_js_code(),
            ToWasmHTTPResponse::to_js_code(),
            ToWasmHttpRequestError::to_js_code(),
            ToWasmHttpResponseProgress::to_js_code(),
            ToWasmHttpUploadProgress::to_js_code(),
            ToWasmPermissionResult::to_js_code(),
            ToWasmLocationUpdate::to_js_code(),
            ToWasmLocationError::to_js_code(),
            ToWasmFileDrag::to_js_code(),
            ToWasmFileDrop::to_js_code(),
            ToWasmFileDropError::to_js_code(),
            ToWasmFileDialogResult::to_js_code(),
            /*ToWasmWebSocketOpen::to_js_code(),
            ToWasmWebSocketClose::to_js_code(),
            ToWasmWebSocketError::to_js_code(),
            ToWasmWebSocketString::to_js_code(),
            ToWasmWebSocketBinary::to_js_code(),*/
            ToWasmSignal::to_js_code(),
            ToWasmAppLifecycle::to_js_code(),
            ToWasmMidiInputData::to_js_code(),
            ToWasmMidiPortList::to_js_code(),
            ToWasmAudioDeviceList::to_js_code(),
            ToWasmVideoPlaybackPrepared::to_js_code(),
            ToWasmVideoTextureUpdated::to_js_code(),
            ToWasmVideoPlaybackCompleted::to_js_code(),
            ToWasmVideoPlaybackResourcesReleased::to_js_code(),
        ]);

        self.os.append_from_wasm_js(&[
            FromWasmStartTimer::to_js_code(),
            FromWasmStopTimer::to_js_code(),
            FromWasmFullScreen::to_js_code(),
            FromWasmNormalScreen::to_js_code(),
            FromWasmRequestAnimationFrame::to_js_code(),
            FromWasmSetDocumentTitle::to_js_code(),
            FromWasmSetMouseCursor::to_js_code(),
            FromWasmTextCopyResponse::to_js_code(),
            FromWasmStorageGet::to_js_code(),
            FromWasmStorageSet::to_js_code(),
            FromWasmStorageDelete::to_js_code(),
            FromWasmStorageList::to_js_code(),
            FromWasmStorageGetRange::to_js_code(),
            FromWasmStorageStat::to_js_code(),
            FromWasmStorageEstimate::to_js_code(),
            FromWasmShowTextIME::to_js_code(),
            FromWasmHideTextIME::to_js_code(),
            FromWasmSetVirtualFileLimits::to_js_code(),
            FromWasmSelectFileDialog::to_js_code(),
            FromWasmHTTPRequest::to_js_code(),
            FromWasmCancelHTTPRequest::to_js_code(),
            FromWasmCheckPermission::to_js_code(),
            FromWasmRequestPermission::to_js_code(),
            FromWasmStartLocationUpdates::to_js_code(),
            FromWasmStopLocationUpdates::to_js_code(),
            /*FromWasmWebSocketOpen::to_js_code(),
            FromWasmWebSocketSendString::to_js_code(),
            FromWasmWebSocketSendBinary::to_js_code(),*/
            FromWasmXrStartPresenting::to_js_code(),
            FromWasmXrStopPresenting::to_js_code(),
            FromWasmCompileWebGLShader::to_js_code(),
            FromWasmAllocArrayBuffer::to_js_code(),
            FromWasmAllocIndexBuffer::to_js_code(),
            FromWasmAllocVao::to_js_code(),
            FromWasmAllocTextureImage2D_BGRAu8_32::to_js_code(),
            FromWasmAllocTextureImage2D_Ru8::to_js_code(),
            FromWasmAllocTextureImage2D_RGBAf32::to_js_code(),
            FromWasmAllocTextureCube_BGRAu8_32::to_js_code(),
            FromWasmBeginRenderTexture::to_js_code(),
            FromWasmRequestRenderTextureCapture::to_js_code(),
            FromWasmBeginRenderCanvas::to_js_code(),
            FromWasmSetDefaultDepthAndBlendMode::to_js_code(),
            FromWasmDrawCall::to_js_code(),
            FromWasmOpenUrl::to_js_code(),
            FromWasmBrowserUpdateUrl::to_js_code(),
            FromWasmBrowserHistoryGo::to_js_code(),
            FromWasmUseMidiInputs::to_js_code(),
            FromWasmSendMidiOutput::to_js_code(),
            FromWasmQueryAudioDevices::to_js_code(),
            FromWasmStartAudioOutput::to_js_code(),
            FromWasmStopAudioOutput::to_js_code(),
            FromWasmQueryMidiPorts::to_js_code(),
            FromWasmPrepareVideoPlayback::to_js_code(),
            FromWasmBeginVideoPlayback::to_js_code(),
            FromWasmPauseVideoPlayback::to_js_code(),
            FromWasmResumeVideoPlayback::to_js_code(),
            FromWasmMuteVideoPlayback::to_js_code(),
            FromWasmUnmuteVideoPlayback::to_js_code(),
            FromWasmSeekVideoPlayback::to_js_code(),
            FromWasmCleanupVideoPlaybackResources::to_js_code(),
        ]);
    }

    fn seconds_since_app_start(&self) -> f64 {
        (Self::monotonic_now() - self.os.start_time).max(0.0)
    }

    fn open_url(&mut self, url: &str, in_place: OpenUrlInPlace) {
        self.os.from_wasm(FromWasmOpenUrl {
            url: url.to_string(),
            in_place: if let OpenUrlInPlace::Yes = in_place {
                true
            } else {
                false
            },
        });
    }

    fn browser_update_url(&mut self, url: &str, replace: bool) {
        let (pathname, search, hash) = Self::split_web_location(url);
        self.update_web_location_state(pathname, search, hash);
        self.os.from_wasm(FromWasmBrowserUpdateUrl {
            url: url.to_string(),
            replace,
        });
    }

    fn browser_history_go(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        self.os.from_wasm(FromWasmBrowserHistoryGo {
            delta: delta as f64,
        });
    }

    fn default_window_size(&self) -> Vec2d {
        self.os.window_geom.inner_size
    }

    /*
    fn start_midi_input(&mut self) {
        self.platform.from_wasm(FromWasmStartMidiInput {
        });
    }

    fn spawn_audio_output<F>(&mut self, f: F) where F: FnMut(AudioTime, &mut dyn AudioOutputBuffer) + Send + 'static {
        let closure_ptr = Box::into_raw(Box::new(WebAudioOutputClosure {
            callback: Box::new(f),
            output_buffer: WebAudioOutputBuffer::default()
        }));
        self.platform.from_wasm(FromWasmSpawnAudioOutput {closure_ptr: closure_ptr as u32});
    }*/
}

impl Cx {
    pub fn time_now() -> f64 {
        unsafe { js_time_now() }
    }

    pub fn monotonic_now() -> f64 {
        unsafe { js_monotonic_now() }
    }
}

#[link(wasm_import_module = "env")]
extern "C" {
    pub fn js_time_now() -> f64;
    pub fn js_monotonic_now() -> f64;
}

// storage buffers for graphics API related platform
pub struct CxOs {
    pub(crate) window_geom: WindowGeom,
    pub(crate) native_window_geom: WindowGeom,
    pub(crate) start_time: f64,

    pub from_wasm: Option<FromWasmMsg>,

    pub(crate) vertex_buffers: usize,
    pub(crate) index_buffers: usize,
    pub(crate) vaos: usize,
    /// WebGL programs queued for compile that JavaScript has not yet reported
    /// linked or failed (`ToWasmWebGLShadersDone`). While non-zero, draw calls
    /// on those programs are dropped by the browser side.
    pub(crate) webgl_shaders_pending: usize,

    pub(crate) to_wasm_js: Vec<String>,
    pub(crate) from_wasm_js: Vec<String>,

    pub(crate) media: CxWebMedia,
    pub(crate) render_texture_captures:
        Vec<(crate::texture::TextureId, usize, usize, Vec<u8>)>,
    /// The one-line notice that a second window maps to nothing has been
    /// given (`CxOsOp::CreateWindow`).
    pub(crate) second_window_reported: bool,
}

impl Default for CxOs {
    fn default() -> Self {
        Self {
            window_geom: WindowGeom::default(),
            native_window_geom: WindowGeom::default(),
            start_time: 0.0,

            from_wasm: Some(FromWasmMsg::new()),

            vertex_buffers: 0,
            index_buffers: 0,
            vaos: 0,
            webgl_shaders_pending: 0,

            to_wasm_js: Vec::new(),
            from_wasm_js: Vec::new(),

            media: CxWebMedia::default(),
            render_texture_captures: Vec::new(),
            second_window_reported: false,
        }
    }
}

impl CxOs {
    pub fn append_to_wasm_js(&mut self, strs: &[String]) {
        self.to_wasm_js.extend_from_slice(strs);
    }

    pub fn append_from_wasm_js(&mut self, strs: &[String]) {
        self.from_wasm_js.extend_from_slice(strs);
    }

    pub fn from_wasm(&mut self, from_wasm: impl FromWasm) {
        self.from_wasm.as_mut().unwrap().from_wasm(from_wasm);
    }
}

#[export_name = "wasm_get_js_message_bridge"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_get_js_message_bridge(cx_ptr: u32) -> u32 {
    let cx = &mut *(cx_ptr as *mut Cx);
    let mut msg = FromWasmMsg::new();
    let mut out = String::new();

    out.push_str("return {\n");
    out.push_str("ToWasmMsg:class extends ToWasmMsg{\n");
    for to_wasm in &cx.os.to_wasm_js {
        out.push_str(to_wasm);
    }
    out.push_str("},\n");
    out.push_str("FromWasmMsg:class extends FromWasmMsg{\n");
    for from_wasm in &cx.os.from_wasm_js {
        out.push_str(from_wasm);
    }
    out.push_str("}\n");
    out.push_str("}");
    msg.push_str(&out);
    msg.release_ownership()
}

#[export_name = "wasm_check_signal"]
#[cfg(target_arch = "wasm32")]
pub unsafe extern "C" fn wasm_check_signal() -> u32 {
    let mut x = 0;
    if SignalToUI::check_and_clear_ui_signal() {
        x |= 1
    }
    if SignalToUI::check_and_clear_action_signal() {
        x |= 2
    }
    x
}

#[export_name = "wasm_init_panic_hook"]
pub unsafe extern "C" fn init_panic_hook() {
    pub fn panic_hook(info: &panic::PanicHookInfo) {
        #[cfg(target_arch = "wasm32")]
        {
            #[link(wasm_import_module = "env")]
            extern "C" {
                fn js_console_error(u8_ptr: u32, len: u32);
            }
            let message = format!("__MAKEPAD_WASM_PANIC__:{}", info);
            unsafe { js_console_error(message.as_ptr() as u32, message.len() as u32) };
        }
        #[cfg(not(target_arch = "wasm32"))]
        crate::error!("{}", info)
    }
    panic::set_hook(Box::new(panic_hook));
}

#[no_mangle]
pub static mut BASE_ADDR: usize = 10;
