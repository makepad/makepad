use {
    crate::{
        area::Area,
        cx::Cx,
        cx_api::CxOsApi,
        draw_pass::{CxDrawPassParent, DrawPassId},
        event::{
            DrawEvent, Event, KeyFocusEvent, NextFrameEvent, TextClipboardEvent, TimerEvent,
            TriggerEvent,
        },
        makepad_live_id::{live_id, LiveId},
        makepad_network::NetworkResponse,
    },
    makepad_studio_protocol::{
        hub_protocol::FrameCodec, AppToStudio, EventSample, RunViewFrameData, RunViewFrameRequest,
        RunViewKeyFocusRect, ScreenshotResponse, StudioToApp, WidgetQueryResponse, WidgetSnapshot,
        WidgetSnapshotResponse, WidgetTreeDumpResponse,
    },
    std::cell::{Cell, RefCell},
    std::collections::{HashMap, HashSet},
    std::rc::Rc,
};

struct EventDispatchGuard {
    active: Rc<Cell<bool>>,
    event_depth: Option<Rc<Cell<u32>>>,
}

impl Drop for EventDispatchGuard {
    fn drop(&mut self) {
        self.active.set(false);
        if let Some(depth) = &self.event_depth {
            depth.set(depth.get().saturating_sub(1));
        }
    }
}

/// File sinks for in-app frame captures (`Cx::capture_next_frame_to_file`).
/// A static mutex rather than Cx state because the metal completion handler
/// that produces the PNG runs off the main thread.
static SCREENSHOT_FILE_SINKS: std::sync::OnceLock<
    std::sync::Mutex<HashMap<u64, std::path::PathBuf>>,
> = std::sync::OnceLock::new();
static SCREENSHOT_FILE_NEXT_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1 << 63);

fn screenshot_file_sinks() -> &'static std::sync::Mutex<HashMap<u64, std::path::PathBuf>> {
    SCREENSHOT_FILE_SINKS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

impl Cx {
    #[allow(dead_code)]
    pub(crate) fn repaint_windows(&mut self) {
        for draw_pass_id in self.passes.id_iter() {
            match self.passes[draw_pass_id].parent {
                CxDrawPassParent::Window(_) => {
                    self.passes[draw_pass_id].paint_dirty = true;
                }
                _ => (),
            }
        }
    }

    #[allow(unused)]
    pub(crate) fn any_passes_dirty(&self) -> bool {
        for draw_pass_id in self.passes.id_iter() {
            if self.passes[draw_pass_id].paint_dirty {
                return true;
            }
        }
        false
    }

    /// The window a pass ultimately presents into (through any chain of
    /// offscreen parents); None for Xr/parentless passes.
    ///
    /// Shared by the per-window paced backends — the macOS display link and the
    /// Windows DXGI frame-latency beat — to decide which passes belong to the
    /// window whose flip is currently being serviced. The 64-step cap is a
    /// cycle guard: a malformed parent chain must not hang the paint loop.
    #[allow(dead_code)]
    pub(crate) fn pass_root_window(&self, pass_id: DrawPassId) -> Option<crate::window::WindowId> {
        let mut id = pass_id;
        for _ in 0..64 {
            match self.passes[id].parent.clone() {
                CxDrawPassParent::Window(window_id) => return Some(window_id),
                CxDrawPassParent::DrawPass(parent) => id = parent,
                _ => return None,
            }
        }
        None
    }

    /// Whether the time repaint (`demo_time_repaint`: some shader read
    /// `draw_pass.time`, so repaint every frame) may re-dirty this pass.
    ///
    /// It used to re-dirty EVERY pass with a main draw list — including
    /// passes their owner did not begin again on the latest redraw, which
    /// still hold a stale draw list, parent and target. Re-running one of
    /// those overwrites the fresh output of the pass that took its place:
    /// the VJ's warp-only beat re-ran the previous beat's whole tween chain
    /// and its stale warp stage (same depth, higher pool id) clobbered
    /// `warp_out` with the old t (audit hazard (a); 999/2000 beats in the
    /// timed alternation probe).
    ///
    /// "Begun on the latest redraw" is read off the draw lists: a pass's main
    /// draw list is rebuilt (`clear_draw_items(redraw_id)`) every time the
    /// pass is begun, so its `redraw_id` is the draw cycle that last began
    /// the pass. The reference is the pass's root window's own main list —
    /// not the global cycle — so a window that did not redraw this cycle
    /// keeps its time-animated child passes alive (multi-window), while a
    /// child left behind by a window that DID redraw is stale. Window passes
    /// are always live; parentless passes compare against the current cycle.
    /// Explicit `repaint_pass` users are untouched: this gates only the time
    /// repaint, and dirty propagation to parents is unchanged.
    fn pass_live_for_time_repaint(&self, pass_id: DrawPassId) -> bool {
        let pass = &self.passes[pass_id];
        let Some(list_id) = pass.main_draw_list_id else {
            return false;
        };
        if matches!(pass.parent, CxDrawPassParent::Window(_)) {
            return true;
        }
        let reference = match self.pass_root_window(pass_id) {
            Some(window_id) if self.windows.is_valid(window_id) => self.windows[window_id]
                .main_pass_id
                .and_then(|main_pass_id| self.passes[main_pass_id].main_draw_list_id)
                .map(|main_list_id| self.draw_lists[main_list_id].redraw_id)
                .unwrap_or(self.redraw_id),
            _ => self.redraw_id,
        };
        self.draw_lists[list_id].redraw_id >= reference
    }

    pub(crate) fn compute_pass_repaint_order(&mut self, passes_todo: &mut Vec<DrawPassId>) {
        passes_todo.clear();

        // we need this because we don't mark the entire deptree of passes dirty every small paint
        loop {
            // loop untill we don't propagate anymore
            let mut altered = false;
            for draw_pass_id in self.passes.id_iter() {
                if self.demo_time_repaint && self.pass_live_for_time_repaint(draw_pass_id) {
                    self.passes[draw_pass_id].paint_dirty = true;
                }
                if self.passes[draw_pass_id].paint_dirty {
                    let other = match self.passes[draw_pass_id].parent {
                        CxDrawPassParent::DrawPass(parent_pass_id) => Some(parent_pass_id),
                        _ => None,
                    };
                    if let Some(other) = other {
                        if !self.passes[other].paint_dirty {
                            self.passes[other].paint_dirty = true;
                            altered = true;
                        }
                    }
                }
            }
            // Liveness runs the OTHER way: a pass that declared itself
            // live-with-parent re-encodes whenever its consumer repaints.
            // The gauss chain rides this — realtime glass — while texture
            // caches, which exist to NOT re-render, never opt in.
            for draw_pass_id in self.passes.id_iter() {
                if self.passes[draw_pass_id].live_with_parent && !self.passes[draw_pass_id].paint_dirty {
                    if let CxDrawPassParent::DrawPass(parent_pass_id) = self.passes[draw_pass_id].parent {
                        if self.passes[parent_pass_id].paint_dirty {
                            self.passes[draw_pass_id].paint_dirty = true;
                            altered = true;
                        }
                    }
                }
            }
            if !altered {
                break;
            }
        }

        // EXECUTION ORDER IS THE DEPENDENCY TREE, deepest first. A pass's
        // parent is its CONSUMER — the pass that samples the texture it
        // renders — so every dirty pass must execute before its parent.
        // Distance-to-root gives exactly that: sort deepest first; the
        // stable sort keeps pool-id order between passes at equal depth
        // (siblings), which is the order this function always produced for
        // them. The old scan inserted a child directly before its parent
        // only when the parent was ALREADY in the list, so with three or
        // more levels of texture passes and adverse (recycled) pool ids a
        // grandchild could land AFTER the pass that consumes its output,
        // which then read a stale texture (the VJ's post/sim pass chains
        // hit exactly this). Parentless passes keep their old "run first"
        // contract via the depth bias.
        const ROOT_NONE_BIAS: u64 = 1 << 32;
        for draw_pass_id in self.passes.id_iter() {
            if self.passes[draw_pass_id].paint_dirty {
                passes_todo.push(draw_pass_id);
            }
        }
        let slot_cap = self.passes.id_iter().count();
        let depth_of = |start: DrawPassId| -> u64 {
            let mut depth = 0u64;
            let mut walk = start;
            loop {
                match self.passes[walk].parent {
                    CxDrawPassParent::DrawPass(parent_id) => {
                        depth += 1;
                        walk = parent_id;
                        if depth as usize > slot_cap {
                            // A cycle in stale parent links (recycled pass
                            // slots): stop counting rather than hang.
                            return depth;
                        }
                    }
                    CxDrawPassParent::None => return depth + ROOT_NONE_BIAS,
                    _ => return depth,
                }
            }
        };
        passes_todo.sort_by_key(|id| std::cmp::Reverse(depth_of(*id)));
        self.demo_time_repaint = false;
    }

    pub(crate) fn need_redrawing(&self) -> bool {
        self.new_draw_event.will_redraw()
    }

    pub(crate) fn dispatch_network_runtime_events(&mut self) {
        self.dispatch_storage_responses();
        use crate::makepad_math::dvec2;
        use crate::window::CxWindowPool;

        let mut responses = Vec::new();
        while let Some(response) = self.net.try_recv() {
            if let Some(msgs) = crate::web_socket::consume_studio_socket_response(&response) {
                let window_id = CxWindowPool::id_zero();
                let pos = dvec2(0.0, 0.0);
                for msg in msgs {
                    let _ = self.dispatch_studio_msg(msg, window_id, pos);
                }
                continue;
            }
            match &response {
                NetworkResponse::WsOpened { .. }
                | NetworkResponse::WsMessage { .. }
                | NetworkResponse::WsClosed { .. }
                | NetworkResponse::WsError { .. } => {
                    self.handle_script_web_socket_event(response.clone())
                }
                NetworkResponse::HttpResponse { .. }
                | NetworkResponse::HttpStreamChunk { .. }
                | NetworkResponse::HttpStreamComplete { .. }
                | NetworkResponse::HttpError { .. }
                | NetworkResponse::HttpProgress { .. } => {}
            }
            responses.push(response);
        }
        if !responses.is_empty() {
            self.handle_script_network_events(&responses);
            self.call_event_handler(&Event::NetworkResponses(responses));
        }
    }

    /// Capture the next presented frame of the main window to a PNG file.
    /// The readback + write happen on the GPU completion thread after the next
    /// repaint, so the caller should poll for the file to appear. Piggybacks on
    /// the studio screenshot pipeline: ids above `SCREENSHOT_FILE_ID_BASE` are
    /// routed to `SCREENSHOT_FILE_SINKS` instead of the studio connection.
    /// (Headless builds write frames to files on their own; this is for the
    /// live GPU-rendered app.)
    /// Returns the capture's request id, so the caller can later
    /// [`cancel_frame_capture`](Self::cancel_frame_capture) it.
    pub fn capture_next_frame_to_file(&mut self, path: std::path::PathBuf) -> u64 {
        let request_id = SCREENSHOT_FILE_NEXT_ID
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        screenshot_file_sinks().lock().unwrap().insert(request_id, path);
        self.screenshot_requests
            .push(makepad_studio_protocol::ScreenshotRequest {
                request_id,
                kind_id: 0,
            });
        self.redraw_all();
        request_id
    }

    /// Forget a pending [`capture_next_frame_to_file`]. The GPU readback
    /// cannot be recalled once its frame presents, so a request that has
    /// already been drained keeps its sink entry but with an EMPTY path —
    /// the writer discards those bytes instead of writing a file the
    /// caller has stopped watching (and never mistakes them for a studio
    /// response). A request whose frame has not presented yet is dropped
    /// outright.
    pub fn cancel_frame_capture(&mut self, request_id: u64) {
        let queued = self
            .screenshot_requests
            .iter()
            .any(|request| request.request_id == request_id);
        let mut sinks = screenshot_file_sinks().lock().unwrap();
        if queued {
            self.screenshot_requests.retain(|request| request.request_id != request_id);
            sinks.remove(&request_id);
        } else if sinks.contains_key(&request_id) {
            sinks.insert(request_id, std::path::PathBuf::new());
        }
    }

    #[allow(dead_code)]
    pub(crate) fn take_studio_screenshot_request_ids(&mut self, kind_id: u32) -> Vec<u64> {
        self.take_studio_screenshot_request_ids_for_window(kind_id, None)
    }

    /// Drain the pending screenshot requests this pass can answer.
    ///
    /// `window_id` is the window the presenting pass belongs to (None for
    /// offscreen/stdin passes). A `--remote` `/g?w=N` grab only matches its own
    /// window, so a multi-window app can be captured window by window instead of
    /// whichever pass happens to present first. Studio and file-sink requests
    /// are untargeted and match any pass, exactly as before.
    #[allow(dead_code)]
    pub(crate) fn take_studio_screenshot_request_ids_for_window(
        &mut self,
        kind_id: u32,
        window_id: Option<usize>,
    ) -> Vec<u64> {
        let mut request_ids = Vec::new();
        self.screenshot_requests.retain(|request| {
            if request.kind_id == kind_id
                && crate::remote::grab_targets_window(request.request_id, window_id)
            {
                request_ids.push(request.request_id);
                false
            } else {
                true
            }
        });
        request_ids
    }

    #[allow(dead_code)]
    pub(crate) fn send_studio_screenshot_response(
        request_ids: Vec<u64>,
        width: u32,
        height: u32,
        png: Vec<u8>,
    ) {
        if request_ids.is_empty() {
            return;
        }
        // `--remote` grabs are answered on the requesting HTTP thread.
        let request_ids = crate::remote::deliver_grabs(
            request_ids,
            width,
            height,
            &png,
        );
        if request_ids.is_empty() {
            return;
        }
        // Ids registered by capture_next_frame_to_file get written to disk;
        // the rest (if any) still go to studio.
        let mut studio_ids = Vec::new();
        {
            let mut sinks = screenshot_file_sinks().lock().unwrap();
            for id in request_ids {
                if let Some(path) = sinks.remove(&id) {
                    if path.as_os_str().is_empty() {
                        // Cancelled after its frame was drained: discard.
                        continue;
                    }
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(err) = std::fs::write(&path, &png) {
                        crate::error!(
                            "capture_next_frame_to_file: write {} failed: {}",
                            path.display(),
                            err
                        );
                    }
                } else {
                    studio_ids.push(id);
                }
            }
        }
        if studio_ids.is_empty() {
            return;
        }
        Cx::send_studio_message(AppToStudio::Screenshot(ScreenshotResponse {
            request_ids: studio_ids,
            png,
            width,
            height,
        }));
    }

    pub(crate) fn queue_studio_run_view_frame_request(&mut self, request: RunViewFrameRequest) {
        self.run_view_frame_requests
            .retain(|existing| existing.window_id != request.window_id);
        self.run_view_frame_requests.push(request);
        self.redraw_all();
    }

    #[allow(dead_code)]
    pub(crate) fn take_studio_run_view_frame_request(
        &mut self,
        window_id: usize,
    ) -> Option<RunViewFrameRequest> {
        if self.run_view_frame_encode_in_flight {
            return None;
        }
        let index = self
            .run_view_frame_requests
            .iter()
            .rposition(|request| request.window_id == window_id)?;
        Some(self.run_view_frame_requests.swap_remove(index))
    }

    #[allow(dead_code)]
    pub(crate) fn encode_studio_run_view_frame_async(
        &mut self,
        request: RunViewFrameRequest,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) {
        if self.run_view_frame_encode_in_flight {
            return;
        }
        self.run_view_frame_encode_in_flight = true;
        let sender = self.run_view_frame_results.sender();
        if let Ok(task) = self.spawn_thread(move || {
            let result = Cx::prepare_studio_run_view_rgba(&request, width, height, rgba).and_then(
                |(width, height, rgba)| {
                    Cx::encode_rgba_as_png(width, height, &rgba).map(|png| RunViewFrameData {
                        window_id: request.window_id,
                        frame_id: request.frame_id,
                        width,
                        height,
                        codec: Some(FrameCodec::Png),
                        data: png,
                    })
                },
            );
            let _ = sender.send(result);
        }) {
            task.detach();
        }
    }

    fn prepare_studio_run_view_rgba(
        request: &RunViewFrameRequest,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(u32, u32, Vec<u8>), String> {
        let expected_len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if rgba.len() != expected_len {
            return Err(format!(
                "runview frame rgba size mismatch: got {} bytes for {}x{}",
                rgba.len(),
                width,
                height
            ));
        }

        let target_width = request.width.max(1);
        let target_height = request.height.max(1);

        let out = if width == target_width && height == target_height {
            rgba
        } else {
            let mut resized = vec![
                0u8;
                (target_width as usize)
                    .saturating_mul(target_height as usize)
                    .saturating_mul(4)
            ];
            for y in 0..target_height as usize {
                let src_y = ((y as u64) * (height as u64) / (target_height as u64)) as usize;
                for x in 0..target_width as usize {
                    let src_x = ((x as u64) * (width as u64) / (target_width as u64)) as usize;
                    let src = (src_y * width as usize + src_x) * 4;
                    let dst = (y * target_width as usize + x) * 4;
                    resized[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
                }
            }
            resized
        };

        Ok((target_width, target_height, out))
    }

    #[allow(dead_code)]
    pub(crate) fn flush_studio_run_view_frame_results(&mut self) {
        loop {
            let Ok(result) = self.run_view_frame_results.try_recv() else {
                break;
            };
            self.run_view_frame_encode_in_flight = false;
            match result {
                Ok(frame) => Cx::send_studio_message(AppToStudio::RunViewFrame(frame)),
                Err(err) => crate::error!("runview frame encode failed: {}", err),
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn send_studio_widget_tree_dump_response(&mut self, request_id: u64) {
        self.widget_tree_dump_requests.push(request_id);
        if self.in_draw_event {
            return;
        }
        self.try_send_studio_widget_tree_dump_responses();
    }

    #[allow(dead_code)]
    pub(crate) fn send_studio_widget_query_response(&self, request_id: u64, query: String) {
        let rects = if let Some(callback) = self.widget_query_callback {
            callback(self, &query)
        } else {
            Vec::new()
        };
        Cx::send_studio_message(AppToStudio::WidgetQuery(WidgetQueryResponse {
            request_id,
            query,
            rects,
        }));
    }

    #[allow(dead_code)]
    pub(crate) fn send_studio_widget_snapshot_response(&mut self, request_id: u64) {
        self.widget_snapshot_requests.push(request_id);
        if self.in_draw_event {
            return;
        }
        self.try_send_studio_widget_snapshot_responses();
    }

    fn studio_key_focus_rect_response(&self) -> RunViewKeyFocusRect {
        let area = self.key_focus();
        if !area.is_valid(self) {
            return RunViewKeyFocusRect::default();
        }
        let rect = area.rect(self);
        if rect.size.x <= 0.0 || rect.size.y <= 0.0 {
            return RunViewKeyFocusRect::default();
        }
        RunViewKeyFocusRect {
            x: Some(rect.pos.x),
            y: Some(rect.pos.y),
            width: Some(rect.size.x),
            height: Some(rect.size.y),
        }
    }

    fn send_studio_key_focus_rect_response(&self) {
        Cx::send_studio_message(AppToStudio::RunViewKeyFocusRect(
            self.studio_key_focus_rect_response(),
        ));
    }

    fn widget_tree_dump_ready(dump: &str) -> bool {
        for line in dump.lines() {
            let mut parts = line.split_whitespace();
            let Some(first) = parts.next() else {
                continue;
            };
            if first.starts_with('W') {
                continue;
            }
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() < 2 {
                continue;
            }
            let Some(h) = tokens.last().and_then(|v| v.parse::<i64>().ok()) else {
                continue;
            };
            let Some(w) = tokens
                .get(tokens.len().saturating_sub(2))
                .and_then(|v| v.parse::<i64>().ok())
            else {
                continue;
            };
            if w > 0 && h > 0 {
                return true;
            }
        }
        false
    }

    fn widget_snapshot_ready(widgets: &[WidgetSnapshot]) -> bool {
        widgets
            .iter()
            .any(|widget| widget.visible && widget.width > 0 && widget.height > 0)
    }

    pub(crate) fn try_send_studio_widget_tree_dump_responses(&mut self) {
        if self.widget_tree_dump_requests.is_empty() {
            return;
        }
        if self.in_draw_event {
            return;
        }
        let dump = if let Some(callback) = self.widget_tree_dump_callback {
            callback(self)
        } else {
            "W1 0\n".to_string()
        };
        if !Self::widget_tree_dump_ready(&dump) {
            return;
        }
        let request_ids: Vec<u64> = self.widget_tree_dump_requests.drain(..).collect();
        for request_id in request_ids {
            Cx::send_studio_message(AppToStudio::WidgetTreeDump(WidgetTreeDumpResponse {
                request_id,
                dump: dump.clone(),
            }));
        }
    }

    pub(crate) fn try_send_studio_widget_snapshot_responses(&mut self) {
        if self.widget_snapshot_requests.is_empty() {
            return;
        }
        if self.in_draw_event {
            return;
        }
        let widgets = if let Some(callback) = self.widget_snapshot_callback {
            callback(self)
        } else {
            Vec::new()
        };
        if !Self::widget_snapshot_ready(&widgets) {
            return;
        }
        let request_ids: Vec<u64> = self.widget_snapshot_requests.drain(..).collect();
        for request_id in request_ids {
            Cx::send_studio_message(AppToStudio::WidgetSnapshot(WidgetSnapshotResponse {
                request_id,
                widgets: widgets.clone(),
            }));
        }
    }

    /// Hardware-faithful synthetic mouse move (remote `/m?hw=1`): the event
    /// takes the SAME pointer-lock/pin transform hardware takes
    /// (`locked_mouse_transform`), so a pinned scrub behaves identically
    /// under injection. The ordinary injection path bypasses
    /// send_mouse_move entirely — which is how every bridge verification
    /// of the pin lied about the physical mouse.
    pub fn dispatch_hw_mouse_move(
        &mut self,
        window_id: crate::window::WindowId,
        raw: crate::makepad_math::DVec2,
        delta: crate::makepad_math::DVec2,
        seed: crate::makepad_math::DVec2,
        modifiers: crate::event::KeyModifiers,
        time: f64,
    ) {
        // `os::apple` does not exist in a headless build (see os/mod.rs), so
        // the pointer-lock transform has to be gated on the module's own cfg,
        // not on the target alone.
        #[cfg(all(target_os = "macos", not(headless)))]
        let (abs, lock_delta) = crate::os::apple::macos::macos_app::with_macos_app(|app| {
            app.locked_mouse_transform(raw, delta, seed)
        });
        #[cfg(not(all(target_os = "macos", not(headless))))]
        let (abs, lock_delta) = {
            let _ = (delta, seed);
            (raw, crate::makepad_math::DVec2::default())
        };
        self.call_event_handler(&Event::MouseMove(crate::event::MouseMoveEvent {
            abs,
            lock_delta,
            window_id,
            modifiers,
            time,
            handled: Cell::new(Area::Empty),
        }));
        self.fingers.cycle_hover_area(live_id!(mouse).into());
        self.fingers.switch_captures();
    }

    /// Hardware-faithful synthetic mouse up, part 1: release an active
    /// scrub pin at the platform layer first — exactly what
    /// macos_window::send_mouse_up does for a physical up.
    pub fn dispatch_hw_pin_release(&mut self) {
        #[cfg(all(target_os = "macos", not(headless)))]
        crate::os::apple::macos::macos_app::with_macos_app(|app| {
            if app.pointer_pin_mode {
                app.set_pointer_pin(false);
            }
        });
    }

    /// Dispatch a StudioToApp message as an event. Handles input, clipboard,
    /// screenshot, widget dump, and kill. Returns true on Kill (caller should
    /// shut down). Callers handle stdin-specific variants (Swapchain,
    /// WindowGeomChange, Tick) before delegating here.
    pub fn dispatch_studio_msg(
        &mut self,
        msg: StudioToApp,
        window_id: crate::window::WindowId,
        pos: crate::makepad_math::DVec2,
    ) -> bool {
        match msg {
            StudioToApp::MouseDown(e) => {
                // Synthetic input must take the same activation path as a
                // native click. In particular, an unfocused macOS window
                // must become key before its drag starts.
                self.activate_window_on_pointer_down(window_id);
                let event = crate::event::MouseDownEvent {
                    abs: crate::makepad_math::dvec2(e.x - pos.x, e.y - pos.y),
                    button: crate::event::MouseButton::from_bits_retain(e.button_raw_bits),
                    window_id,
                    modifiers: e.modifiers.into_key_modifiers(),
                    time: e.time,
                    handled: Cell::new(Area::Empty),
                };
                self.fingers.process_tap_count(event.abs, event.time);
                self.fingers.mouse_down(event.button, window_id);
                self.call_event_handler(&Event::MouseDown(event));
                self.update_pointer_capture_pacing();
            }
            StudioToApp::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(crate::event::MouseMoveEvent {
                lock_delta: Default::default(),
                    abs: crate::makepad_math::dvec2(e.x - pos.x, e.y - pos.y),
                    window_id,
                    modifiers: e.modifiers.into_key_modifiers(),
                    time: e.time,
                    handled: Cell::new(Area::Empty),
                }));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            StudioToApp::MouseUp(e) => {
                let event = crate::event::MouseUpEvent {
                    abs: crate::makepad_math::dvec2(e.x - pos.x, e.y - pos.y),
                    button: crate::event::MouseButton::from_bits_retain(e.button_raw_bits),
                    window_id,
                    modifiers: e.modifiers.into_key_modifiers(),
                    time: e.time,
                };
                let button = event.button;
                self.call_event_handler(&Event::MouseUp(event));
                self.fingers.mouse_up(button);
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.update_pointer_capture_pacing();
                self.send_studio_key_focus_rect_response();
            }
            StudioToApp::Scroll(e) => {
                self.call_event_handler(&Event::Scroll(crate::event::ScrollEvent {
                    abs: crate::makepad_math::dvec2(e.x - pos.x, e.y - pos.y),
                    scroll: crate::makepad_math::dvec2(e.sx, e.sy),
                    window_id,
                    modifiers: e.modifiers.into_key_modifiers(),
                    handled_x: Cell::new(false),
                    handled_y: Cell::new(false),
                    is_mouse: e.is_mouse,
                    time: e.time,
                    phase: crate::event::ScrollPhase::None,
                }));
            }
            StudioToApp::GameInput(states) => {
                // Replace wholesale rather than merge: Studio sends the whole
                // set, so a pad that unplugs disappears by being absent.
                self.game_input_remote = states.into_iter().map(|s| s.into()).collect();
            }
            StudioToApp::KeyDown(e) => {
                self.keyboard.process_key_down(e.clone());
                self.call_event_handler(&Event::KeyDown(e));
            }
            StudioToApp::KeyUp(e) => {
                self.keyboard.process_key_up(e.clone());
                self.call_event_handler(&Event::KeyUp(e));
            }
            StudioToApp::TextInput(e) => {
                #[cfg(all(target_vendor = "apple", not(headless)))]
                crate::os::apple::metal::note_input_event();
                self.call_event_handler(&Event::TextInput(e));
            }
            StudioToApp::TextCopy => {
                let response = Rc::new(RefCell::new(None));
                self.call_event_handler(&Event::TextCopy(TextClipboardEvent {
                    response: response.clone(),
                }));
                let text = response.borrow().clone();
                if let Some(text) = text {
                    Cx::send_studio_message(AppToStudio::SetClipboard(text));
                }
            }
            StudioToApp::TextCut => {
                let response = Rc::new(RefCell::new(None));
                self.call_event_handler(&Event::TextCut(TextClipboardEvent {
                    response: response.clone(),
                }));
                let text = response.borrow().clone();
                if let Some(text) = text {
                    Cx::send_studio_message(AppToStudio::SetClipboard(text));
                }
            }
            StudioToApp::Screenshot(request) => {
                self.screenshot_requests.push(request);
                self.redraw_all();
            }
            StudioToApp::RunViewFrameRequest(request) => {
                self.queue_studio_run_view_frame_request(request);
            }
            StudioToApp::WidgetTreeDump(request) => {
                self.send_studio_widget_tree_dump_response(request.request_id);
            }
            StudioToApp::WidgetQuery(request) => {
                self.send_studio_widget_query_response(request.request_id, request.query);
            }
            StudioToApp::WidgetSnapshot(request) => {
                self.send_studio_widget_snapshot_response(request.request_id);
            }
            StudioToApp::Kill => {
                self.call_event_handler(&Event::Shutdown);
                return true;
            }
            StudioToApp::Custom(data) => {
                self.call_event_handler(&Event::Custom(data));
            }
            StudioToApp::KeepAlive | StudioToApp::None => {}
            StudioToApp::LiveChange { file_name, content } => {
                self.script_data
                    .live_reload
                    .queue_file_change(file_name, content);
            }
            // Stdin-specific: Tick, Swapchain, WindowGeomChange are handled
            // by callers before delegating here. In windowed mode they are
            // no-ops (the native event loop handles them).
            StudioToApp::Tick
            | StudioToApp::Swapchain(_)
            | StudioToApp::WindowGeomChange { .. }
            | StudioToApp::TweakRay(_) => {}
        }
        false
    }

    /// Drain the global control channel and dispatch each message as an event.
    /// Must be called from the event loop (not from inside an event handler).
    /// Also services the `--remote` HTTP control surface, which every backend
    /// therefore gets by calling this one function.
    pub fn poll_control_channel(&mut self) {
        use crate::makepad_math::dvec2;
        use crate::web_socket::CONTROL_CHANNEL;
        use crate::window::CxWindowPool;
        crate::remote::poll(self);
        let msgs: Vec<StudioToApp> = {
            let lock = CONTROL_CHANNEL.lock().unwrap();
            if let Some(rx) = lock.as_ref() {
                rx.try_iter().collect()
            } else {
                return;
            }
        };
        let window_id = CxWindowPool::id_zero();
        let pos = dvec2(0.0, 0.0);
        for msg in msgs {
            self.dispatch_studio_msg(msg, window_id, pos);
        }
    }

    pub(crate) fn run_live_edit_if_needed(&mut self, _backend: &str) {
        // Three independent triggers, fanning out to two events. The
        // critical distinction between FileChange and Manual is whether
        // we follow LiveEdit up with an immediate ScriptReapply pass in
        // the SAME tick — manual triggers (rotation) defer it to the next
        // tick to keep each tick's work bounded, since rotation can fire
        // multiple WindowGeomChange events back-to-back during the
        // animation and each Apply walk over the full widget tree is
        // non-trivial on mobile hardware.
        //
        // 1. `LiveEditTrigger::FileChange` — file watcher delivered a
        //    hot-reloaded `script_mod!` block (or studio websocket sent
        //    a `LiveChange`). The DSL itself changed; shader caches may
        //    be stale, so `reset_for_live_reload` runs. Any preference
        //    re-broadcast in the LiveEdit handler propagates immediately
        //    via the same-tick `ScriptReapply` follow-up — file changes
        //    are a live-coding scenario where the user wants to see the
        //    update right away.
        //
        // 2. `LiveEditTrigger::Manual` — `cx.request_live_edit()` was
        //    called (canonical case: safe-area insets changed on iOS
        //    rotation, where `mod.widgets.SAFE_INSET_PAD_*` heap
        //    primitives need to be re-baked into `script_mod!` block
        //    expressions). The DSL did NOT change; we skip
        //    `reset_for_live_reload` (no shader code changed), and we
        //    skip the immediate ScriptReapply follow-up — if the
        //    LiveEdit handler set `pending_script_reapply` (e.g. an app
        //    re-broadcasting preferences), it lands on the next event-
        //    loop tick. Without this split, rotation incurred a visible
        //    1-2s lag from doing two full Apply walks per geom change.
        //
        // 3. `LiveEditTrigger::None` + `pending_script_reapply` — set by
        //    `cx.request_script_reapply()` after runtime mutations to a
        //    *shared* heap *object* (`script_eval!` overriding
        //    `mod.widgets.IMG_MSG_FIT.max`, etc.). Re-running script_mod
        //    would clobber those overrides; we fire `Event::ScriptReapply`
        //    which re-applies the captured `app_value` with
        //    `Apply::ScriptReapply` — no script_mod re-run, runtime
        //    overrides preserved, imperative-setter fields early-return.
        use crate::live_reload::LiveEditTrigger;
        match self.handle_live_edit() {
            LiveEditTrigger::FileChange => {
                self.draw_shaders.reset_for_live_reload();
                self.pending_script_reapply = false;
                self.call_event_handler(&Event::LiveEdit);
                self.redraw_all();
                if self.pending_script_reapply {
                    self.pending_script_reapply = false;
                    self.call_event_handler(&Event::ScriptReapply);
                    self.redraw_all();
                }
            }
            LiveEditTrigger::Manual => {
                // Clear `pending_script_reapply` defensively — LiveEdit's
                // script_mod re-run clobbers heap overrides anyway, and an
                // app-level handler that re-broadcasts sets a fresh flag
                // that lands on the next tick.
                self.pending_script_reapply = false;
                self.call_event_handler(&Event::LiveEdit);
                self.redraw_all();
            }
            LiveEditTrigger::None => {
                if self.pending_script_reapply {
                    self.pending_script_reapply = false;
                    self.call_event_handler(&Event::ScriptReapply);
                    self.redraw_all();
                }
            }
        }
    }

    // Same logic as headless::raster::encode_png_rgba which is behind
    // cfg(headless) and unavailable to the windowed backend.
    #[allow(dead_code)]
    pub fn encode_rgba_as_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
        use makepad_zune_png::{
            makepad_zune_core::{
                bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions,
            },
            PngEncoder,
        };
        let options = EncoderOptions::default()
            .set_width(width as usize)
            .set_height(height as usize)
            .set_depth(BitDepth::Eight)
            .set_colorspace(ColorSpace::RGBA);
        let mut encoder = PngEncoder::new(rgba, options);
        let mut out = Vec::new();
        encoder
            .encode(&mut out)
            .map_err(|err| format!("png encode failed: {err:?}"))?;
        Ok(out)
    }

    // event handler wrappers

    fn invoke_event_handler(&mut self, event: &Event) {
        let event_handler = self.event_handler.clone();
        // The active flag excludes aliasing, while the Rc keeps this stable
        // allocation alive even if the handler mutates `Cx`.
        unsafe {
            (&mut *event_handler.get())(self, event);
        }
    }

    fn event_dispatch_is_reentrant(&self, event: &Event) -> bool {
        if self.event_handler_dispatch_active.get() {
            crate::error!(
                "Rejected synchronous re-entry while dispatching event {}",
                event.name()
            );
            return true;
        }
        false
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn reset_event_dispatch_state(&mut self) {
        self.event_handler_dispatch_active.set(false);
        self.perf_monitor.event_depth.set(0);
    }

    pub(crate) fn inner_call_event_handler(&mut self, event: &Event) {
        if self.event_dispatch_is_reentrant(event) {
            return;
        }
        self.event_id += 1;
        // PerfMonitor "event" channel: time only the OUTERMOST dispatch —
        // Paint recurses into this from the Timer handler on macos.
        let perf_timing = self.perf_monitor.enabled();
        if perf_timing {
            self.perf_monitor
                .event_depth
                .set(self.perf_monitor.event_depth.get() + 1);
        }
        let dispatch_guard = EventDispatchGuard {
            active: self.event_handler_dispatch_active.clone(),
            event_depth: perf_timing.then(|| self.perf_monitor.event_depth.clone()),
        };
        self.event_handler_dispatch_active.set(true);
        let perf_t0 = (perf_timing && self.perf_monitor.event_depth.get() == 1)
            .then(Cx::monotonic_now);
        if (Cx::has_studio_web_socket()
            && !crate::web_socket::STUDIO_STDOUT_MODE.load(std::sync::atomic::Ordering::SeqCst))
            || Cx::local_profile_capture_enabled()
        {
            let start = self.seconds_since_app_start();
            self.invoke_event_handler(event);
            let end = self.seconds_since_app_start();
            Cx::send_studio_message(AppToStudio::EventSample(EventSample {
                event_u32: event.to_u32(),
                start: start,
                event_meta: if let Event::Timer(TimerEvent { timer_id, .. }) = event {
                    *timer_id
                } else {
                    0
                },
                end: end,
            }))
        } else {
            self.invoke_event_handler(event);
        }
        drop(dispatch_guard);
        if perf_timing {
            if let Some(t0) = perf_t0 {
                self.perf_monitor.add(
                    crate::perf_monitor::PERF_CHANNEL_EVENT,
                    ((Cx::monotonic_now() - t0).max(0.0) * 1_000_000.0) as u64,
                );
            }
        }

        if Cx::has_studio_web_socket() {
            self.try_send_studio_widget_tree_dump_responses();
            self.try_send_studio_widget_snapshot_responses();
        }

        // Reset widget query invalidation after all views have processed it.
        // We wait until event_id is at least 1 events past the invalidation event
        // to ensure the cache clear has propagated through the widget hierarchy
        // during the previous event cycle.
        if let Some(event_id) = self.widget_query_invalidation_event {
            if self.event_id > event_id + 1 {
                self.widget_query_invalidation_event = None;
            }
        }
    }

    fn inner_key_focus_change(&mut self) {
        if let Some((prev, focus)) = self.keyboard.cycle_key_focus_changed() {
            self.inner_call_event_handler(&Event::KeyFocus(KeyFocusEvent { prev, focus }));
        }
    }

    pub fn handle_triggers(&mut self) {
        // post op events like signals, triggers and key-focus
        let mut counter = 0;
        while self.triggers.len() != 0 {
            counter += 1;
            let mut triggers = HashMap::new();
            std::mem::swap(&mut self.triggers, &mut triggers);
            self.inner_call_event_handler(&Event::Trigger(TriggerEvent { triggers: triggers }));
            self.inner_key_focus_change();
            if counter > 100 {
                crate::error!("Trigger feedback loop detected");
                break;
            }
        }
    }

    pub fn handle_actions(&mut self) {
        // post op events like signals, triggers and key-focus
        let mut counter = 0;
        while self.new_actions.len() != 0 {
            counter += 1;
            let mut actions = Vec::new();
            std::mem::swap(&mut self.new_actions, &mut actions);
            self.inner_call_event_handler(&Event::Actions(actions));
            self.inner_key_focus_change();
            if counter > 100 {
                crate::error!("Action feedback loop detected");
                crate::error!("New actions {:#?}", self.new_actions);
                break;
            }
        }
    }

    /// Dispatch any `WindowGeomChange` events queued by code that ran during
    /// the current event dispatch (typically `Cx::set_window_dpi_override`
    /// called from a widget handler). Drained the same way as `handle_actions`
    /// — swap, dispatch each, repeat until quiescent. Each dispatch is a
    /// fresh `inner_call_event_handler` call after the previous dispatch has
    /// completed, so it is not rejected as synchronous re-entry.
    pub fn handle_pending_window_geom_changes(&mut self) {
        let mut counter = 0;
        while !self.pending_window_geom_changes.is_empty() {
            counter += 1;
            let mut events = Vec::new();
            std::mem::swap(&mut self.pending_window_geom_changes, &mut events);
            for event in events {
                self.inner_call_event_handler(&Event::WindowGeomChange(event));
                self.inner_key_focus_change();
            }
            if counter > 100 {
                crate::error!("WindowGeomChange feedback loop detected");
                break;
            }
        }
    }

    /// Clears all widgets' hover/pressed visuals by dispatching one
    /// `Event::ClearHover` once the current event and its actions finish,
    /// for when an overlay kept the normal hover-outs from arriving.
    pub fn clear_all_hovers(&mut self) {
        self.clear_hover_queued = true;
    }

    pub(crate) fn handle_pending_clear_hover(&mut self) {
        if self.clear_hover_queued {
            self.clear_hover_queued = false;
            self.inner_call_event_handler(&Event::ClearHover);
            self.handle_actions();
        }
    }

    pub(crate) fn call_event_handler(&mut self, event: &Event) {
        if self.event_dispatch_is_reentrant(event) {
            return;
        }
        if !matches!(event, Event::Shutdown) {
            crate::thread::service_scheduler(self, event);
        }
        // A scrub pin listens for the button-up ITSELF: release must never
        // depend on a widget hit path. Schedule the cursor release here,
        // but do NOT clear the capture's pin flag yet — the flag must
        // survive THIS dispatch so every suppression gate (hover, new
        // captures, the tweak pick pass) still stands down while the owner
        // receives its FingerUp; clearing early let the pick pass eat the
        // up. The flag dies WITH the capture in fingers.mouse_up, which
        // every platform calls right after this dispatch.
        if let Event::MouseUp(e) = event {
            if e.button.is_primary() && self.fingers.has_pinned_capture() {
                self.platform_ops
                    .push_back(crate::cx_api::CxOsOp::PinMousePointer(false));
            }
        }
        // The F10 exploded z-layer view is LIVE: the intercept claims only
        // its own keys and the orbit drag (on raw screen coordinates), then
        // the router re-addresses every other pointer event to the plane
        // its ray lands on so ordinary dispatch — hover, wheel scrolling,
        // the tweaker's pick — works on the exploded app. (After the pin
        // hook: a mid-drag F10 must never strand a hidden cursor.)
        if self.sploded_intercept(event) {
            return;
        }
        let routed = self.sploded_route(event);
        let event = routed.as_ref().unwrap_or(event);
        if let Event::PermissionResult(result) = event {
            self.handle_camera_permission_result(result);
        }
        self.inner_call_event_handler(event);
        // Dispatch any synthetic geom changes queued during the original
        // handler (e.g. runtime dpi_override updates) before triggers and
        // actions, so layout-dependent reactions see the new geometry.
        self.handle_pending_window_geom_changes();
        self.inner_key_focus_change();
        self.handle_triggers();
        self.handle_actions();
        // Drain script task queues after each event dispatch cycle so
        // widget->script calls run immediately instead of waiting for tick/timer paths.
        self.handle_script_tasks();
        // Script callbacks can enqueue actions/triggers; flush them in the same cycle.
        self.handle_pending_window_geom_changes();
        self.inner_key_focus_change();
        self.handle_triggers();
        self.handle_actions();
        self.handle_pending_clear_hover();
        if matches!(event, Event::Shutdown) {
            crate::thread::service_scheduler(self, event);
            self.thread_spawner.close_runtime();
        }
    }

    #[allow(dead_code)]
    pub(crate) fn set_physical_keyboard_state(&mut self, connected: bool) {
        self.keyboard.set_physical_keyboard_state(connected);
    }

    #[allow(dead_code)]
    pub(crate) fn update_physical_keyboard_state(&mut self, connected: bool) {
        if let Some(event) = self.keyboard.update_physical_keyboard_state(connected) {
            self.call_event_handler(&Event::PhysicalKeyboard(event));
        }
    }

    // helpers

    /*
    pub (crate) fn call_all_keys_up(&mut self) {
        let keys_down = self.keyboard.all_keys_up();
        for key_event in keys_down {
            self.call_event_handler(&Event::KeyUp(key_event))
        }
    }*/

    pub(crate) fn call_draw_event(&mut self, time: f64) {
        let mut draw_event = DrawEvent::default();
        std::mem::swap(&mut draw_event, &mut self.new_draw_event);
        draw_event.time = time;
        self.in_draw_event = true;

        self.call_event_handler(&Event::Draw(draw_event));
        self.in_draw_event = false;
        if let Some(mut hook) = self.post_draw_hook.take() {
            hook(self);
            if self.post_draw_hook.is_none() {
                self.post_draw_hook = Some(hook);
            }
        }

        if Cx::has_studio_web_socket() {
            self.try_send_studio_widget_tree_dump_responses();
            self.try_send_studio_widget_snapshot_responses();
        }
    }

    pub(crate) fn call_next_frame_event(&mut self, time: f64) {
        let mut set = HashSet::default();
        std::mem::swap(&mut set, &mut self.new_next_frames);

        self.performance_stats.process_frame_data(time);

        self.call_event_handler(&Event::NextFrame(NextFrameEvent {
            set,
            time: time,
            frame: self.repaint_id,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn platform_monotonic_time_moves_forward() {
        let wall = Cx::time_now();
        let start = Cx::monotonic_now();
        assert!(wall > 0.0);
        assert!((wall - start).abs() > 1_000_000.0);

        let mut previous = start;
        let mut later = start;
        for _ in 0..1_000_000 {
            std::hint::spin_loop();
            later = Cx::monotonic_now();
            assert!(later >= previous);
            previous = later;
            if later > start {
                break;
            }
        }
        assert!(later - start > 0.0);
    }

    #[test]
    fn synchronous_event_handler_reentry_is_rejected() {
        let calls = Rc::new(Cell::new(0));
        let handler_calls = calls.clone();
        let mut cx = Cx::new(Box::new(move |cx, _event| {
            handler_calls.set(handler_calls.get() + 1);
            cx.call_event_handler(&Event::Signal);
        }));

        cx.call_event_handler(&Event::Signal);
        assert_eq!(calls.get(), 1);
        assert!(!cx.event_handler_dispatch_active.get());

        cx.call_event_handler(&Event::Signal);
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn panicking_event_handler_is_restored() {
        let calls = Rc::new(Cell::new(0));
        let panic_once = Rc::new(Cell::new(true));
        let handler_calls = calls.clone();
        let handler_panic_once = panic_once.clone();
        let mut cx = Cx::new(Box::new(move |_cx, _event| {
            handler_calls.set(handler_calls.get() + 1);
            if handler_panic_once.replace(false) {
                panic!("intentional event-handler panic");
            }
        }));
        cx.perf_monitor.set_enabled(true);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cx.call_event_handler(&Event::Signal);
        }));
        assert!(result.is_err());
        assert!(!cx.event_handler_dispatch_active.get());
        assert_eq!(cx.perf_monitor.event_depth.get(), 0);

        cx.call_event_handler(&Event::Signal);
        assert_eq!(calls.get(), 2);
    }
}
