use {
    crate::{
        area::Area,
        cx::Cx,
        cx_api::CxOsApi,
        draw_pass::{CxDrawPassParent, DrawPassId},
        event::{
            DrawEvent, Event, KeyFocusEvent, NextFrameEvent, TextClipboardEvent,
            TimerEvent, TriggerEvent,
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
                CxDrawPassParent::Window(_) if self.passes[draw_pass_id].main_draw_list_id.is_some() => {
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

    /// Whether this pass's `repaint_next` (a shader read `draw_pass.time`, or
    /// a draw call had to be skipped) may re-dirty it.
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
    /// are live while their window is open; parentless passes compare against the current cycle.
    /// Explicit `repaint_pass` users are untouched: this gates only the time
    /// repaint, and dirty propagation to parents is unchanged.
    fn pass_live_for_time_repaint(&self, pass_id: DrawPassId) -> bool {
        let pass = &self.passes[pass_id];
        let Some(list_id) = pass.main_draw_list_id else {
            return false;
        };
        // A closed window can't paint, so its pass, or a child pass dirtying it, would stay dirty forever.
        if let CxDrawPassParent::Window(window_id) = pass.parent {
            return self.windows.is_valid(window_id) && self.windows[window_id].is_created;
        }
        let reference = match self.pass_root_window(pass_id) {
            Some(window_id) if self.windows.is_valid(window_id) => {
                if !self.windows[window_id].is_created {
                    return false;
                }
                self.windows[window_id]
                    .main_pass_id
                    .and_then(|main_pass_id| self.passes[main_pass_id].main_draw_list_id)
                    .map(|main_list_id| self.draw_lists[main_list_id].redraw_id)
                    .unwrap_or(self.redraw_id)
            }
            _ => self.redraw_id,
        };
        self.draw_lists[list_id].redraw_id >= reference
    }

    pub(crate) fn compute_pass_repaint_order(&mut self, passes_todo: &mut Vec<DrawPassId>) {
        passes_todo.clear();

        // A pass whose last handle was dropped lets go of its attachments
        // and its place in the tree now, on every backend: its slot is only
        // reset when it is reused, and until then it kept its colour and
        // depth textures (full-frame targets) alive.
        for draw_pass_id in self.passes.id_iter() {
            if self.passes.0.is_free(draw_pass_id.0) {
                let pass = &mut self.passes[draw_pass_id];
                if !pass.color_textures.is_empty() || pass.depth_texture.is_some() {
                    pass.color_textures.clear();
                    pass.depth_texture = None;
                    pass.main_draw_list_id = None;
                    pass.parent = crate::draw_pass::CxDrawPassParent::None;
                    pass.attached_by = None;
                    pass.paint_dirty = false;
                    pass.incomplete = false;
                    pass.kept_recording = false;
                }
            }
        }

        // Resolve attachment liveness through the whole consumer chain before
        // propagating dirtiness. A descendant's own attachment can still be
        // current inside an orphaned capture; it must not revive that capture.
        // Mark a walk inactive while visiting it so recycled parent cycles
        // cannot keep obsolete passes alive. Each slot is visited once.
        let slot_cap = self.passes.id_iter().count();
        let mut live = vec![None; slot_cap];
        let mut path = Vec::new();
        for draw_pass_id in self.passes.id_iter() {
            let mut walk = draw_pass_id;
            let active = loop {
                if let Some(active) = live[walk.0] {
                    break active;
                }
                live[walk.0] = Some(false);
                path.push(walk);
                if self.pass_attachment_is_stale(walk) && !self.passes[walk].kept_recording {
                    break false;
                }
                match self.passes[walk].parent {
                    CxDrawPassParent::DrawPass(parent) => walk = parent,
                    _ => break true,
                }
            };
            for id in path.drain(..) {
                live[id.0] = Some(active);
            }
        }

        // An orphaned child pass — attached by a draw list that has since
        // been recorded without it (`Cx::pass_attachment_is_stale`) — is not
        // painted, is neither a source nor a sink of dirtiness below, and has
        // its flag cleared so an idle app does not keep requesting frames on
        // its behalf. The one exception is an explicit `repaint_pass`, which
        // paints it this once.
        let mut time_owed = false;
        for draw_pass_id in self.passes.id_iter() {
            let requested = self.passes[draw_pass_id].repaint_requested;
            if !requested && live[draw_pass_id.0] != Some(true) {
                self.passes[draw_pass_id].paint_dirty = false;
                // an orphan's picture is final as it stands: nothing it was drawing waits on it
                self.passes[draw_pass_id].incomplete = false;
            }
            let repaint_next = std::mem::take(&mut self.passes[draw_pass_id].repaint_next);
            let pass = &self.passes[draw_pass_id];
            if !(repaint_next || pass.time_owed) {
                continue;
            }
            let live_for_time = live[draw_pass_id.0] == Some(true)
                && self.pass_live_for_time_repaint(draw_pass_id);
            // A pass that per-window pacing skipped is still dirty, so it keeps owing its frame,
            // but one that can't paint anymore (closed window, stale child) owes nothing.
            if !pass.paint_dirty || !live_for_time {
                self.passes[draw_pass_id].time_owed = false;
            }
            if repaint_next && live_for_time {
                self.passes[draw_pass_id].paint_dirty = true;
                self.passes[draw_pass_id].time_owed = true;
            }
            time_owed |= self.passes[draw_pass_id].time_owed;
        }

        // we need this because we don't mark the entire deptree of passes dirty every small paint
        loop {
            // loop untill we don't propagate anymore
            let mut altered = false;
            for draw_pass_id in self.passes.id_iter() {
                if live[draw_pass_id.0] != Some(true) {
                    continue;
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
                if self.passes[draw_pass_id].live_with_parent
                    && !self.passes[draw_pass_id].paint_dirty
                    && live[draw_pass_id.0] == Some(true)
                {
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
            let requested = std::mem::take(&mut self.passes[draw_pass_id].repaint_requested);
            if self.passes[draw_pass_id].paint_dirty
                && (live[draw_pass_id.0] == Some(true) || requested)
            {
                passes_todo.push(draw_pass_id);
            }
        }
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
        self.pass_time_rebase = !time_owed;
        self.demo_time_repaint = false;
    }

    /// Whether a pass's own list was recorded by the current drawing (any of
    /// its draw contexts).
    fn recorded_by_this_drawing(&self, id: DrawPassId) -> bool {
        self.passes[id].main_draw_list_id.is_some_and(|l| !self.draw_lists.is_id_freed(l) && self.draw_lists[l].redraw_id > self.drawing_start)
    }

    /// The frame is drawn again before it painted (the glyph wait, a first
    /// frame waiting for its pipelines): what the last draw event recorded
    /// and attached and has not painted is taken, each pass with the redraw
    /// its list was recorded in, for the next drawing to keep what it does not
    /// record again ([`Cx::keep_unrepeated_recordings`]). The draw event's own
    /// redraws are used, not the current drawing's: a draw context made
    /// outside a draw event since (while the wait pumps events) is a drawing
    /// of its own and would hide them.
    #[allow(dead_code)]
    pub(crate) fn frame_drawn_again(&mut self) {
        let (start, end) = self.redraw_keep.drawn;
        let drawn = |cx: &Self, id: DrawPassId| {
            cx.passes[id].main_draw_list_id.is_some_and(|l| !cx.draw_lists.is_id_freed(l) && cx.draw_lists[l].redraw_id > start && cx.draw_lists[l].redraw_id <= end)
        };
        let taken: Vec<(DrawPassId, u64)> = self
            .passes
            .id_iter()
            .filter(|id| !self.passes.0.is_free(id.0) && drawn(self, *id))
            .filter_map(|id| {
                let pass = &self.passes[id];
                let attached = pass.attached_by.is_some() && !self.pass_attachment_is_stale(id);
                (pass.paint_dirty && attached).then(|| (id, self.draw_lists[pass.main_draw_list_id.unwrap()].redraw_id))
            })
            .collect();
        self.redraw_keep.taken = taken;
    }

    /// After a drawing (`call_draw_event`): a pass an earlier drawing of this
    /// frame recorded and attached (taken by [`Cx::frame_drawn_again`]), or
    /// one kept before and not yet painted whole, which this drawing neither
    /// recorded nor attached again, stays attached (`kept_recording`) and
    /// paints what was recorded. The native backends painted every drawing;
    /// a pass recorded only on some frames (a tile written into a ring when
    /// one is due, a cache drawn now and then) was dropped unpainted when the
    /// frame was drawn again, and what it wrote never came. Not kept: a pass
    /// this drawing recorded again (it is its own), and one whose colour
    /// target a pass this drawing recorded writes (the newer recording owns
    /// that picture). A depth target does not count: it is the drawing's
    /// scratch, often one buffer shared by passes that do not share a picture.
    pub(crate) fn keep_unrepeated_recordings(&mut self) {
        self.redraw_keep.drawn = (self.drawing_start, self.redraw_id);
        let taken = std::mem::take(&mut self.redraw_keep.taken);
        let carried = std::mem::take(&mut self.redraw_keep.kept);
        let targets_of = |pass: &crate::draw_pass::CxDrawPass| -> Vec<crate::texture::TextureId> {
            pass.color_textures.iter().map(|a| a.texture.texture_id()).collect()
        };
        for kept in &carried {
            if !self.passes.0.is_free(kept.pass.0) {
                self.passes[kept.pass].kept_recording = false;
            }
        }
        let fresh: Vec<KeptRecording> = taken.into_iter().filter(|(pass, _)| !self.passes.0.is_free(pass.0)).map(|(pass, redraw)| KeptRecording { pass, redraw, painted: self.passes[pass].painted_serial }).collect();
        let candidates: Vec<KeptRecording> = carried.into_iter().chain(fresh).collect();
        let mut kept: Vec<KeptRecording> = Vec::new();
        for candidate in candidates {
            let id = candidate.pass;
            if self.passes.0.is_free(id.0) || kept.iter().any(|k| k.pass == id) {
                continue;
            }
            let Some(list) = self.passes[id].main_draw_list_id else { continue };
            if self.draw_lists.is_id_freed(list) || self.draw_lists[list].redraw_id != candidate.redraw || !self.pass_attachment_is_stale(id) {
                continue;
            }
            let targets = targets_of(&self.passes[id]);
            let newer = self.passes.id_iter().any(|other| {
                other != id && !self.passes.0.is_free(other.0) && self.recorded_by_this_drawing(other) && targets_of(&self.passes[other]).iter().any(|t| targets.contains(t))
            });
            if !newer {
                self.passes[id].kept_recording = true;
                kept.push(candidate);
            }
        }
        self.redraw_keep.kept = kept;
    }

    /// After a build: a kept pass that has painted whole since it was kept
    /// is an orphan again; one that has not (a window that could not take a
    /// frame, a pipeline still compiling) stays kept.
    #[allow(dead_code)]
    pub(crate) fn release_painted_recordings(&mut self) {
        let kept = std::mem::take(&mut self.redraw_keep.kept);
        for k in kept {
            if self.passes.0.is_free(k.pass.0) {
                continue;
            }
            if self.passes[k.pass].painted_serial > k.painted {
                self.passes[k.pass].kept_recording = false;
            } else {
                self.redraw_keep.kept.push(k);
            }
        }
    }

    /// The passes other than `consumer` that render into `texture` and can
    /// paint: they have a draw list (a withdrawn or frozen one has none) and
    /// are still attached (an orphan, its attaching list recorded again
    /// without it, does not paint unless it is kept).
    #[allow(dead_code)]
    pub(crate) fn producers_that_paint(&self, texture: crate::texture::TextureId, consumer: DrawPassId) -> Vec<DrawPassId> {
        self.passes
            .id_iter()
            .filter(|p| {
                let pass = &self.passes[*p];
                *p != consumer
                    && pass.main_draw_list_id.is_some()
                    && (!self.pass_attachment_is_stale(*p) || pass.kept_recording)
                    && pass.color_textures.iter().any(|a| a.texture.texture_id() == texture)
            })
            .collect()
    }

    /// The `draw_pass.time` for a repaint stamped `now`. Its origin only moves while
    /// nothing time-driven is on screen, so the f32 stays precise however long the app runs.
    pub(crate) fn pass_time(&mut self, now: f64) -> f32 {
        if std::mem::take(&mut self.pass_time_rebase) {
            self.pass_time_origin = now;
        }
        (now - self.pass_time_origin).max(0.0) as f32
    }

    /// Where popups draw as overlays on their parent's surface (Android, iOS), a dirty
    /// popup has to repaint its parent window too. Window passes sort last, so we append.
    #[allow(dead_code)]
    pub(crate) fn queue_popup_parents(&self, passes_todo: &mut Vec<DrawPassId>) {
        for i in 0..passes_todo.len() {
            let CxDrawPassParent::Window(window_id) = self.passes[passes_todo[i]].parent else {
                continue;
            };
            if !self.windows.is_valid(window_id) {
                continue;
            }
            let window = &self.windows[window_id];
            if !window.is_created || !window.is_popup {
                continue;
            }
            let Some(parent_pass_id) = window
                .popup_parent
                .filter(|parent_id| {
                    self.windows.is_valid(*parent_id)
                        && self.windows[*parent_id].is_created
                        && !self.windows[*parent_id].is_popup
                })
                .and_then(|parent_id| self.windows[parent_id].main_pass_id)
            else {
                continue;
            };
            if self.passes[parent_pass_id].main_draw_list_id.is_some()
                && !passes_todo.contains(&parent_pass_id)
            {
                passes_todo.push(parent_pass_id);
            }
        }
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
            if let Some(backlog) = self.studio_backlog.as_mut() {
                if crate::web_socket::is_studio_socket_response(&response) {
                    backlog.push_back(response);
                    continue;
                }
            }
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
                | NetworkResponse::WsError { .. } => self.handle_script_web_socket_event(&response),
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
        if let Ok(task) = self.task_pool().submit_internal(crate::thread::Lane::Heavy, move || {
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
        // Remote coordinates already integrate the injected deltas. Pinned captures retain
        // that virtual position; the HAL applies the actual OS pointer lock separately.
        let _ = seed;
        let abs = raw;
        let lock_delta = if self.fingers.has_pinned_capture() { delta } else { crate::makepad_math::DVec2::default() };
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
        if self.fingers.has_pinned_capture() {
            self.push_unique_platform_op(crate::cx_api::CxOsOp::PinMousePointer(false));
        }
    }

    /// Dispatch a StudioToApp message as an event. Handles input, clipboard,
    /// screenshot, widget dump, and kill. Returns true on Kill (caller should
    /// shut down). Callers handle stdin-specific variants (Swapchain,
    /// WindowGeomChange, Tick) before delegating here.
    /// A host-forwarded pointer position — host logical points, relative to
    /// the host's origin — as the app's layout sees it: relative to the
    /// window, then through the window's dpi override (a native window's
    /// events take the same remap in the platform callbacks).
    pub(crate) fn stdin_pointer_abs(
        &self,
        host: crate::makepad_math::DVec2,
        window_pos: crate::makepad_math::DVec2,
        window_id: crate::window::WindowId,
    ) -> crate::makepad_math::DVec2 {
        let mut abs = crate::makepad_math::dvec2(host.x - window_pos.x, host.y - window_pos.y);
        self.dpi_override_scale(&mut abs, window_id);
        abs
    }

    /// A host's `StudioToApp::MouseCancel`: a real cancellation, not a
    /// release. Every capture of the mouse is cancelled and hears
    /// `Event::FingerCancel` (which hidden widgets admit too); then the
    /// button is released without any MouseUp being dispatched.
    pub(crate) fn dispatch_hosted_mouse_cancel(
        &mut self,
        e: &makepad_studio_protocol::RemoteMouseUp,
        window_id: crate::window::WindowId,
        pos: crate::makepad_math::DVec2,
    ) {
        let digit_id = live_id!(mouse).into();
        let button = crate::event::MouseButton::from_bits_retain(e.button_raw_bits);
        self.fingers.cancel_digit(digit_id);
        self.call_event_handler(&Event::FingerCancel(crate::event::FingerCancelEvent {
            window_id,
            digit_id,
            device: crate::event::DigitDevice::Mouse { button },
            abs: self.stdin_pointer_abs(crate::makepad_math::dvec2(e.x, e.y), pos, window_id),
            time: e.time,
            modifiers: e.modifiers.into_key_modifiers(),
        }));
        self.fingers.mouse_up(button);
        self.fingers.cycle_hover_area(live_id!(mouse).into());
    }

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
                    abs: self.stdin_pointer_abs(crate::makepad_math::dvec2(e.x, e.y), pos, window_id),
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
                    abs: self.stdin_pointer_abs(crate::makepad_math::dvec2(e.x, e.y), pos, window_id),
                    window_id,
                    modifiers: e.modifiers.into_key_modifiers(),
                    time: e.time,
                    handled: Cell::new(Area::Empty),
                }));
                self.fingers.cycle_hover_area(live_id!(mouse).into());
                self.fingers.switch_captures();
            }
            StudioToApp::MouseCancel(e) => {
                self.dispatch_hosted_mouse_cancel(&e, window_id, pos);
                self.update_pointer_capture_pacing();
                self.send_studio_key_focus_rect_response();
            }
            StudioToApp::MouseUp(e) => {
                let event = crate::event::MouseUpEvent {
                    abs: self.stdin_pointer_abs(crate::makepad_math::dvec2(e.x, e.y), pos, window_id),
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
                    abs: self.stdin_pointer_abs(crate::makepad_math::dvec2(e.x, e.y), pos, window_id),
                    scroll: crate::makepad_math::dvec2(e.sx, e.sy),
                    window_id,
                    modifiers: e.modifiers.into_key_modifiers(),
                    handled_x: Cell::new(false),
                    handled_y: Cell::new(false),
                    is_mouse: e.is_mouse,
                    time: e.time,
                    phase: e.phase,
                }));
            }
            StudioToApp::Pinch(e) => {
                self.call_event_handler(&Event::Pinch(crate::event::PinchEvent {
                    abs: self.stdin_pointer_abs(crate::makepad_math::dvec2(e.x, e.y), pos, window_id),
                    window_id,
                    scale: e.scale,
                    phase: e.phase,
                    modifiers: e.modifiers.into_key_modifiers(),
                    time: e.time,
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
                self.call_event_handler(&Event::TextInput(e));
            }
            StudioToApp::DropFiles(e) => {
                let abs = self.stdin_pointer_abs(crate::makepad_math::dvec2(e.x, e.y), pos, window_id);
                let modifiers = e.modifiers.into_key_modifiers();
                let items = std::sync::Arc::new(
                    e.paths
                        .into_iter()
                        .map(|path| crate::event::DragItem::FilePath { path, internal_id: None })
                        .collect::<Vec<_>>(),
                );
                self.call_event_handler(&Event::Drag(crate::event::DragEvent {
                    modifiers,
                    handled: std::sync::Arc::new(std::sync::Mutex::new(false)),
                    abs,
                    items: items.clone(),
                    response: std::sync::Arc::new(std::sync::Mutex::new(crate::event::DragResponse::None)),
                }));
                self.call_event_handler(&Event::Drop(crate::event::DropEvent {
                    modifiers,
                    handled: std::sync::Arc::new(std::sync::Mutex::new(false)),
                    abs,
                    items,
                }));
                self.call_event_handler(&Event::DragEnd);
            }
            StudioToApp::TextCopy => {
                let response = Rc::new(RefCell::new(None));
                self.call_event_handler(&Event::TextCopy(TextClipboardEvent {
                    response: response.clone(),
                }));
                let text = response.borrow().clone();
                // a copy or cut of nothing leaves the host's clipboard as it is
                if let Some(text) = text.filter(|t| !t.is_empty()) {
                    Cx::send_studio_message(AppToStudio::SetClipboard(text));
                }
            }
            StudioToApp::TextCut => {
                let response = Rc::new(RefCell::new(None));
                self.call_event_handler(&Event::TextCut(TextClipboardEvent {
                    response: response.clone(),
                }));
                let text = response.borrow().clone();
                // a copy or cut of nothing leaves the host's clipboard as it is
                if let Some(text) = text.filter(|t| !t.is_empty()) {
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
                if crate::ime::HostedBack::parse(&data).is_some_and(|back| back.handled.is_none()) {
                    let event = Event::BackPressed { handled: std::cell::Cell::new(false) };
                    self.call_event_handler(&event);
                    let Event::BackPressed { handled } = event else { unreachable!() };
                    let reply = crate::ime::HostedBack { handled: Some(handled.get()) }.to_json();
                    // a domain answers its parent (the WM) over the HAL; a hosted process over its socket
                    #[cfg(not(target_arch = "wasm32"))]
                    let sent = self.hal_send_parent_message(&reply);
                    #[cfg(target_arch = "wasm32")]
                    let sent = false;
                    if !sent {
                        Self::send_studio_message(AppToStudio::Custom(reply));
                    }
                } else if let Some(shown) = makepad_studio_protocol::HostedVisible::parse(&data) {
                    let hidden = !shown.visible;
                    if self.hosted_hidden != hidden {
                        self.hosted_hidden = hidden;
                        if !hidden {
                            // What the host could not show meanwhile is stale.
                            self.redraw_all();
                        }
                    }
                } else if let Some(window) = crate::ime::HostedWindow::parse(&data) {
                    #[cfg(not(target_arch = "wasm32"))]
                    if window.op == crate::ime::hosted_window_op::CLOSE_REQUEST {
                        self.hal_hosted_close_request(window.window_id);
                    }
                    #[cfg(target_arch = "wasm32")]
                    let _ = window;
                } else if let Some(fullscreen) = crate::ime::HostedFullscreen::parse(&data) {
                    // A fullscreen window draws no caption (widgets `Window`), and its geometry
                    // says it is fullscreen (`WindowHandle::is_fullscreen`), as a native one's does.
                    let window_id = self.windows.id_iter().find(|w| w.id() == fullscreen.window_id);
                    if let Some(window_id) = window_id {
                        if self.windows[window_id].hosted_fullscreen != fullscreen.on {
                            self.windows[window_id].hosted_fullscreen = fullscreen.on;
                            let old_geom = self.windows[window_id].window_geom.clone();
                            self.windows[window_id].window_geom.is_fullscreen = fullscreen.on;
                            let new_geom = self.windows[window_id].window_geom.clone();
                            self.call_event_handler(&Event::WindowGeomChange(crate::event::WindowGeomChangeEvent { window_id, old_geom, new_geom }));
                            self.redraw_all();
                        }
                    }
                } else if let Some(query) = makepad_studio_protocol::HostedDragQuery::parse(&data) {
                    // The host asks the app's window drag query (the one a
                    // native backend asks for a borderless window).
                    use crate::event::WindowDragQueryResponse as R;
                    let response = std::rc::Rc::new(std::cell::Cell::new(R::NoAnswer));
                    // The window the point is in (a multi-window app's later windows sit at
                    // their own origins in the child's window space), in its own points.
                    let host = crate::makepad_math::dvec2(query.x, query.y);
                    let (window_id, pos) = if self.windows.len() == 0 {
                        (crate::window::CxWindowPool::id_zero(), crate::makepad_math::dvec2(0.0, 0.0))
                    } else {
                        self.windows.window_id_contains(host)
                    };
                    let abs = if self.windows.len() == 0 { host } else { self.stdin_pointer_abs(host, pos, window_id) };
                    self.call_event_handler(&Event::WindowDragQuery(crate::event::WindowDragQueryEvent {
                        window_id,
                        abs,
                        response: response.clone(),
                    }));
                    let answer = match response.get() {
                        R::NoAnswer => makepad_studio_protocol::drag_answer::NO_ANSWER,
                        R::Client => makepad_studio_protocol::drag_answer::CLIENT,
                        R::Caption => makepad_studio_protocol::drag_answer::CAPTION,
                        R::SysMenu => makepad_studio_protocol::drag_answer::SYS_MENU,
                    };
                    // What a double click there does: the window's own declaration.
                    let fullscreen = self.windows.len() != 0 && self.windows.is_valid(window_id) && self.windows[window_id].double_click_fullscreen;
                    let double_click = if fullscreen { makepad_studio_protocol::drag_double::FULLSCREEN } else { makepad_studio_protocol::drag_double::MAXIMISE };
                    Self::send_studio_message(AppToStudio::Custom(
                        makepad_studio_protocol::HostedDragAnswer { id: query.id, response: answer, double_click }.to_json(),
                    ));
                } else {
                    self.call_event_handler(&Event::Custom(data));
                }
            }
            StudioToApp::Relay(relay) => self.handle_host_relay(relay),
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

    /// Wake and redraw after an installed compiler patch, including while idle.
    #[cfg(relax)]
    fn poll_live_patch(&mut self) {
        use std::sync::atomic::{AtomicBool, Ordering};
        static INSTALLED: AtomicBool = AtomicBool::new(false);
        static CHANGED: AtomicBool = AtomicBool::new(false);
        extern "C" fn wake() {
            CHANGED.store(true, Ordering::Release);
            crate::thread::SignalToUI::set_internal_signal();
        }
        if !INSTALLED.swap(true, Ordering::Relaxed) {
            std::relax::on_patch(wake);
            // the first look at the design files' times
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            self.queue_patched_script_mod_files();
        }
        if CHANGED.swap(false, Ordering::AcqRel) {
            // a design (script_mod!) edit is not re-run by the patch itself
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            self.queue_patched_script_mod_files();
            self.redraw_all();
        }
    }

    /// Drain the global control channel and dispatch each message as an event.
    /// Must be called from the event loop (not from inside an event handler).
    /// Also services the `--remote` HTTP control surface, which every backend
    /// therefore gets by calling this one function.
    pub fn poll_control_channel(&mut self) {
        #[cfg(relax)]
        self.poll_live_patch();
        use crate::makepad_math::dvec2;
        use crate::web_socket::CONTROL_CHANNEL;
        use crate::window::CxWindowPool;
        crate::remote::poll(self);
        // makepad inspect's queries for a VM of this thread that is idle now (`src`, `vm get`)
        crate::makepad_script::inspect_query::serve_idle();
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
                self.live_edit_apply = crate::makepad_script::Apply::Reload;
                self.call_event_handler(&Event::LiveEdit);
                self.redraw_all();
                if self.pending_script_reapply {
                    self.pending_script_reapply = false;
                    self.call_event_handler(&Event::ScriptReapply);
                    self.redraw_all();
                }
            }
            LiveEditTrigger::Manual => {
                // A style reload re-evaluates Splash modules under the names
                // they had, so their bodies' script addresses now hold new
                // code: the object and function-address shader caches would
                // hand back the previous shaders (or another template's).
                // Only the code-keyed cache stays valid.
                if self.pending_style_reload {
                    self.draw_shaders.reset_for_live_reload();
                }
                // Clear `pending_script_reapply` defensively — LiveEdit's
                // script_mod re-run clobbers heap overrides anyway, and an
                // app-level handler that re-broadcasts sets a fresh flag
                // that lands on the next tick.
                self.pending_script_reapply = false;
                // The DSL did not change, so re-apply with `Rebake`: the
                // re-run only exists to pick up new `SAFE_INSET_PAD_*`
                // values, and imperative runtime state must survive it.
                self.live_edit_apply = crate::makepad_script::Apply::Rebake;
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
        let _phase = crate::thread::ui_event_phase(event);
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
        // A claim during this event cancelled presses that were already
        // handled earlier in it: each loser gets its terminal cancelled
        // FingerUp now, before any further input, from a dispatch of its own.
        let pending = self.fingers.take_pending_cancels(0.0);
        if !pending.is_empty() {
            let now = self.seconds_since_app_start();
            for mut cancel in pending {
                cancel.time = now;
                self.inner_call_event_handler(&Event::FingerCancel(cancel));
            }
        }
        if perf_timing {
            if let Some(t0) = perf_t0 {
                self.perf_monitor.add(
                    crate::perf_monitor::PERF_CHANNEL_EVENT,
                    ((Cx::monotonic_now() - t0).max(0.0) * 1_000_000.0) as u64,
                );
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        if self.collect.is_some() {
            self.collect_after_draw();
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

    /// Deliver everything the last event or draw left queued (actions
    /// posted from other threads and from handlers, triggers, a key-focus
    /// change requested while drawing, window geometry changes, a cleared
    /// hover) now, at the current time. The virtual clock does this before
    /// it advances a frame: otherwise such work ran with whatever event came
    /// next, before or after the advance depending on the machine's load,
    /// and a timer it started (a caret blink) landed a frame apart from run
    /// to run.
    #[cfg(not(target_arch = "wasm32"))]
    #[cfg_attr(target_os = "android", allow(dead_code))] // only the remote bridge settles events, and Android has none
    pub(crate) fn settle_deferred_events(&mut self) {
        self.handle_action_receiver();
        self.handle_triggers();
        self.handle_actions();
        self.inner_key_focus_change();
        self.handle_pending_window_geom_changes();
        self.handle_pending_clear_hover();
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
        #[cfg(not(target_arch = "wasm32"))]
        if matches!(event, Event::Shutdown) {
            self.splash_trace_exit();
        }
        // Under `makepad run` with an agent watching (makepad inspect), the dispatch is a retry
        // boundary: a panic in it parks for the agent, and when the agent resumes it with
        // `retry` (after a hot patch) the event is dispatched again with the patched code.
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(r) = relax_retry::hooks() {
            let name = event.name();
            loop {
                (r.enter)(name.as_ptr(), name.len());
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.call_event_handler_once(event)));
                (r.leave)();
                match res {
                    Ok(()) => return,
                    Err(p) => {
                        if !(r.take)() {
                            std::panic::resume_unwind(p);
                        }
                    }
                }
            }
        }
        self.call_event_handler_once(event)
    }

    fn call_event_handler_once(&mut self, event: &Event) {
        let _phase = crate::thread::ui_event_phase(event);
        if self.event_dispatch_is_reentrant(event) {
            return;
        }
        crate::remote::note_user_event(self, event);
        match event {
            Event::Signal => crate::frame_sources::signal(),
            Event::Timer(t) => crate::frame_sources::timer_fired(t.timer_id),
            _ => {}
        }
        if let Event::Signal = event {
            for hook in self.signal_hooks.clone() {
                hook(self);
            }
        }
        if let Event::Shutdown = event {
            // every quit path ends here (a window close, a lost session): the shutdown starts
            crate::cx::relax_quit_started();
            // An open `/cap/start` file is finished before the app goes.
            crate::remote::finish_capture_on_shutdown(self);
        }
        if let Some(drag) = self.drag_drop.internal_drag_event(event) {
            // The pointer event goes out first and the drag one is appended, the
            // way every other backend orders it: a widget that ends its gesture on
            // FingerUp never sees one otherwise, and stays stuck mid-drag.
            self.drag_drop.suspend_internal_drag();
            self.call_event_handler(event);
            self.drag_drop.resume_internal_drag();
            match drag {
                crate::event::InternalDragEvent::Drag(event) => {
                    self.call_event_handler(&Event::Drag(event));
                    self.drag_drop.cycle_drag();
                }
                crate::event::InternalDragEvent::Drop(event) => {
                    self.call_event_handler(&Event::Drop(event));
                    self.drag_drop.cycle_drag();
                    self.call_event_handler(&Event::DragEnd);
                    self.drag_drop.cycle_drag();
                }
                crate::event::InternalDragEvent::End => {
                    self.call_event_handler(&Event::DragEnd);
                    self.drag_drop.cycle_drag();
                }
            }
            return;
        }
        if matches!(event, Event::Startup) {
            self.initialize_memory_budget();
            // The workers boot now, before the app exists, so the first job
            // never waits for a thread (a Web Worker takes hundreds of ms).
            self.warm_task_pool();
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
        // The exploded z-layer view is LIVE: the intercept claims only
        // its own keys and the orbit drag (on raw screen coordinates), then
        // the router re-addresses every other pointer event to the plane
        // its ray lands on so ordinary dispatch — hover, wheel scrolling,
        // the tweaker's pick — works on the exploded app. (After the pin
        // hook: leaving mid-drag must never strand a hidden cursor.)
        let intercepted = self.sploded_intercept(event);
        // Settle ownership before widget dispatch, including presses consumed by a
        // platform overlay so their release cannot cancel a second thing underneath.
        let widget_owner = self.cancel_scopes.resolve_widget_owner(event, intercepted, |lookup| {
            self.cancel_scope_resolver.and_then(|resolve| resolve(self, lookup))
        });
        self.cancel_scopes.handle_event(event, intercepted, widget_owner);
        if intercepted {
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
            self.close_task_pool();
            self.thread_spawner.close_runtime();
        }
    }

    /// A platform caught a panic that unwound out of an event or draw
    /// dispatch and goes on. What an unwound frame cannot put back itself
    /// is put back here: the script VM has parked itself (the `with_vm`
    /// family catches, parks and resumes) and the draw contexts returned
    /// their stacks on drop, so what remains is a draw whose redraw list
    /// went down with the frame — everything is asked to draw again — and
    /// a check that the VM really did come back.
    #[allow(dead_code)]
    pub(crate) fn recover_after_caught_panic(&mut self) {
        self.in_draw_event = false;
        // contained at the turn: no dispatch is running any more. An unwind that skipped the
        // dispatch guard (a fault turned into a panic in code without a landing pad for it) left
        // the flag set, and every later event -- NextFrame, Draw, QuitRequested, Shutdown -- was
        // rejected as re-entrant: the app looked alive but never drew again nor shut down (its
        // pool threads leaked, so `session relaunch` was refused)
        self.event_handler_dispatch_active.set(false);
        self.perf_monitor.event_depth.set(0);
        if self.script_vm.is_none() {
            crate::error!(
                "the script VM did not survive an unwound event handler: every later script entry will be refused as re-entrant"
            );
        }
        self.redraw_all();
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
        let time = crate::remote::app_clock::now().unwrap_or(time);
        let mut draw_event = DrawEvent::default();
        std::mem::swap(&mut draw_event, &mut self.new_draw_event);
        draw_event.time = time;
        // one drawing, however many draw contexts the handler makes
        self.drawing_id += 1;
        self.drawing_start = self.redraw_id;
        self.in_draw_event = true;
        crate::frame_trace::note_drawn();
        // The flag comes down whether the draw returned or unwound: a
        // platform that catches the panic and goes on must not have every
        // later `redraw` refused as "already drawing".
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.call_event_handler(&Event::Draw(draw_event))
        }));
        self.in_draw_event = false;
        if let Err(payload) = drawn {
            std::panic::resume_unwind(payload);
        }
        self.validate_scoped_sweep_locks_after_draw();
        self.keep_unrepeated_recordings();
        if let Some(mut hook) = self.post_draw_hook.take() {
            hook(self);
            if self.post_draw_hook.is_none() {
                self.post_draw_hook = Some(hook);
            }
        }

        // The frame's recording boundary: every draw list has finished
        // recording. The geometries dropped since the last redraw are freed
        // now, except those a live draw call still names (see
        // `CxGeometryPool`); the scan runs only when something was dropped.
        if self.geometries.has_unreleased() {
            let referenced = self.draw_lists.referenced_geometries();
            self.geometries.release_unreferenced(&referenced);
        }

        if Cx::has_studio_web_socket() {
            self.try_send_studio_widget_tree_dump_responses();
            self.try_send_studio_widget_snapshot_responses();
        }
    }

    pub(crate) fn call_next_frame_event(&mut self, time: f64) {
        #[cfg(target_arch = "wasm32")]
        self.dispatch_clipboard_reads();
        // Under the virtual clock a NextFrame is a frame of `/step`, never
        // of the backend's paint beat; the pending set waits for one.
        if crate::remote::app_clock::enabled() && !self.app_clock.dispatching_frame {
            return;
        }
        let time = crate::remote::app_clock::now().unwrap_or(time);
        let mut set = HashSet::default();
        std::mem::swap(&mut set, &mut self.new_next_frames);

        self.performance_stats.process_frame_data(time);

        self.call_event_handler(&Event::NextFrame(NextFrameEvent {
            set,
            time: time,
            frame: if crate::remote::app_clock::enabled() {
                crate::remote::app_clock::frame()
            } else {
                self.repaint_id
            },
        }));
    }
}

/// Does the pass `consumer` sample the colour target of another pass that
/// has yet to paint, or painted with draws left out (still dirty)?
///
/// The repaint order only knows the parent chain (a parent is painted after
/// its children), but a pass can sample any other pass's target: siblings
/// under one host (a compositor's source and output passes), or a target a
/// later pass in the order renders. Such a consumer records pixels that are
/// about to change, so the backends keep it dirty and paint it again after
/// its source, as they do for their own skipped draws. Without this a
/// consumer painted before its source (or while a pipeline of the source
/// was still compiling, which only showed under load) came out clean with
/// a black or stale sample, and a host waiting for a clean paint read it.
/// Cheap: only render-target textures are looked up, against the passes.
#[allow(dead_code)]
pub(crate) fn samples_incomplete_target(
    passes: &crate::draw_pass::CxDrawPassPool,
    textures: &crate::texture::CxTexturePool,
    texture: crate::texture::TextureId,
    consumer: DrawPassId,
) -> bool {
    sampled_picture(passes, textures, texture, consumer).to_paint
}

/// The picture a pass samples from other passes' colour target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct SampledPicture {
    /// A producer still has to paint it ([`samples_incomplete_target`]): the
    /// consumer paints again after it.
    pub(crate) to_paint: bool,
    /// A producer's picture is unfinished (`CxDrawPass::incomplete`: painted
    /// with a draw left out, or keeping an older picture, while a pipeline
    /// compiles), dirty or not: the consumer's paint is not whole. Nothing is
    /// owed a repaint for it (the pipeline's answer repaints every pass).
    pub(crate) unfinished: bool,
}

/// What `consumer` samples in `texture`, in one look over the passes.
#[allow(dead_code)]
pub(crate) fn sampled_picture(
    passes: &crate::draw_pass::CxDrawPassPool,
    textures: &crate::texture::CxTexturePool,
    texture: crate::texture::TextureId,
    consumer: DrawPassId,
) -> SampledPicture {
    let mut picture = SampledPicture::default();
    if !textures[texture].format.is_render() {
        return picture;
    }
    for producer in passes.id_iter() {
        let pass = &passes[producer];
        if producer == consumer || !(pass.paint_dirty || pass.incomplete) {
            continue;
        }
        if pass.color_textures.iter().any(|attachment| attachment.texture.texture_id() == texture) {
            picture.to_paint |= pass.paint_dirty;
            picture.unfinished |= pass.incomplete;
        }
    }
    picture
}

/// A drawing kept for a frame drawn again (`Cx::keep_unrepeated_recordings`):
/// the pass, the redraw its list was recorded in, and its `painted_serial`
/// when it was kept.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct KeptRecording {
    pub(crate) pass: DrawPassId,
    pub(crate) redraw: u64,
    pub(crate) painted: u64,
}

/// What [`Cx::frame_drawn_again`] took and what is kept.
#[derive(Default)]
pub(crate) struct RedrawKeep {
    pub(crate) taken: Vec<(DrawPassId, u64)>,
    pub(crate) kept: Vec<KeptRecording>,
    /// The redraws the last draw event's draw contexts took: after the first,
    /// up to the second.
    pub(crate) drawn: (u64, u64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{draw_list::{DrawList, DrawListId}, draw_pass::DrawPass};
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn a_pass_sampling_a_target_still_to_paint_is_incomplete() {
        use crate::draw_pass::DrawPassClearColor;
        use crate::makepad_math::vec4;
        use crate::texture::{Texture, TextureFormat, TextureSize};
        // A compositor's shape: two sibling passes, the consumer samples the
        // producer's target. The consumer is painted first among equals; if
        // the producer is still dirty then (not painted yet, or it left draws
        // out while a pipeline compiled), the consumer's sample is not final.
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let target = |cx: &mut Cx| {
            Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 {
                size: TextureSize::Fixed { width: 4, height: 4 },
                initial: true,
            })
        };
        let consumer = DrawPass::new(&mut cx);
        let producer = DrawPass::new(&mut cx);
        let (consumer_target, producer_target) = (target(&mut cx), target(&mut cx));
        consumer.add_color_texture(&mut cx, &consumer_target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        producer.add_color_texture(&mut cx, &producer_target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        let (c, p) = (consumer.draw_pass_id(), producer.draw_pass_id());
        let sampled = producer_target.texture_id();

        cx.passes[p].paint_dirty = true;
        assert!(samples_incomplete_target(&cx.passes, &cx.textures, sampled, c));
        // painted: the sample is final
        cx.passes[p].paint_dirty = false;
        assert!(!samples_incomplete_target(&cx.passes, &cx.textures, sampled, c));
        // a pass reading its own target (ping-pong) is not waiting on itself
        cx.passes[c].paint_dirty = true;
        assert!(!samples_incomplete_target(&cx.passes, &cx.textures, consumer_target.texture_id(), c));
        // only render targets have a producer
        let image = Texture::new_with_format(&mut cx, TextureFormat::VecBGRAu8_32 {
            width: 1, height: 1, data: Some(vec![0]), updated: crate::texture::TextureUpdated::Full,
        });
        assert!(!samples_incomplete_target(&cx.passes, &cx.textures, image.texture_id(), c));
    }

    #[test]
    fn a_pass_sampling_an_unfinished_picture_is_not_whole() {
        use crate::draw_pass::DrawPassClearColor;
        use crate::makepad_math::vec4;
        use crate::texture::{Texture, TextureFormat, TextureSize};
        // The producer painted with a draw left out (or kept an older
        // picture) while its pipeline compiled: what it shows is not what it
        // recorded, dirty or not.
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let consumer = DrawPass::new(&mut cx);
        let producer = DrawPass::new(&mut cx);
        let target = Texture::new_with_format(&mut cx, TextureFormat::RenderBGRAu8 {
            size: TextureSize::Fixed { width: 4, height: 4 },
            initial: true,
        });
        producer.add_color_texture(&mut cx, &target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        let (c, p) = (consumer.draw_pass_id(), producer.draw_pass_id());
        let sampled = |cx: &Cx, by| sampled_picture(&cx.passes, &cx.textures, target.texture_id(), by);
        let picture = |to_paint, unfinished| SampledPicture { to_paint, unfinished };
        cx.passes[p].hold_picture();
        assert_eq!(sampled(&cx, c), picture(false, true));
        assert!(!samples_incomplete_target(&cx.passes, &cx.textures, target.texture_id(), c), "nothing to repaint for it");
        assert_eq!(sampled(&cx, p), picture(false, false), "not waiting on itself");
        // dirty as well (to paint after the consumer, or held for a queued
        // shader): the consumer paints again and is not whole either
        cx.passes[p].paint_dirty = true;
        assert_eq!(sampled(&cx, c), picture(true, true));
        // painted whole: the sample is final
        cx.passes[p].painted(true, 3);
        cx.passes[p].paint_dirty = false;
        assert_eq!(sampled(&cx, c), picture(false, false));
        // withdrawn after a partial paint: nothing draws from it until it records again
        cx.passes[p].painted(false, 4);
        assert_eq!(sampled(&cx, c), picture(false, true));
        producer.withdraw(&mut cx);
        assert_eq!(sampled(&cx, c), picture(false, false));
        // orphaned (nothing attaches it any more): its picture is final as it stands
        let list = DrawList::new(&mut cx);
        let recording_gen = cx.next_uniform_gen();
        let uniforms_gen = cx.next_uniform_gen();
        cx.draw_lists[list.id()].clear_draw_items(1, recording_gen, uniforms_gen);
        cx.attach_child_pass(p, c, Some(list.id()));
        cx.passes[p].painted(false, 5);
        let recording_gen = cx.next_uniform_gen();
        let uniforms_gen = cx.next_uniform_gen();
        cx.draw_lists[list.id()].clear_draw_items(2, recording_gen, uniforms_gen);
        cx.compute_pass_repaint_order(&mut Vec::new());
        assert_eq!(sampled(&cx, c), picture(false, false));
    }

    #[test]
    fn only_a_producer_with_a_draw_list_can_paint_a_given_back_target() {
        use crate::draw_pass::DrawPassClearColor;
        use crate::makepad_math::vec4;
        use crate::texture::{Texture, TextureFormat, TextureSize};
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let consumer = DrawPass::new(&mut cx);
        let (producer, _list) = pass_with_current_list(&mut cx);
        let target = Texture::new_with_format(&mut cx, TextureFormat::RenderBGRAu8 {
            size: TextureSize::Fixed { width: 4, height: 4 },
            initial: true,
        });
        producer.add_color_texture(&mut cx, &target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        let (c, p) = (consumer.draw_pass_id(), producer.draw_pass_id());
        assert_eq!(cx.producers_that_paint(target.texture_id(), c), vec![p]);
        assert!(cx.producers_that_paint(target.texture_id(), p).is_empty(), "not its own producer");
        // orphaned (its attaching list recorded again without it): it does
        // not paint either, unless it is kept
        let host = DrawList::new(&mut cx);
        let record = |cx: &mut Cx, redraw: u64| {
            let recording_gen = cx.next_uniform_gen();
            let uniforms_gen = cx.next_uniform_gen();
            cx.draw_lists[host.id()].clear_draw_items(redraw, recording_gen, uniforms_gen);
        };
        record(&mut cx, 5);
        cx.attach_child_pass(p, c, Some(host.id()));
        assert_eq!(cx.producers_that_paint(target.texture_id(), c), vec![p]);
        record(&mut cx, 6);
        assert!(cx.producers_that_paint(target.texture_id(), c).is_empty());
        cx.passes[p].kept_recording = true;
        assert_eq!(cx.producers_that_paint(target.texture_id(), c), vec![p]);
        cx.passes[p].kept_recording = false;
        // withdrawn (its attachments stay): it cannot paint, so nothing is owed
        producer.withdraw(&mut cx);
        assert!(cx.producers_that_paint(target.texture_id(), c).is_empty());
    }

    #[test]
    fn a_paint_moves_painted_serial_only_when_whole() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let pass = DrawPass::new(&mut cx);
        let p = pass.draw_pass_id();
        // a draw left out: the serial stays, the picture is unfinished
        cx.passes[p].painted(false, 7);
        assert_eq!(cx.passes[p].painted_serial, 0);
        assert!(cx.passes[p].incomplete);
        // whole: the serial moves and the picture is final
        cx.passes[p].painted(true, 8);
        assert_eq!(cx.passes[p].painted_serial, 8);
        assert!(!cx.passes[p].incomplete);
        // kept its older picture for a compiling pipeline: nothing to paint
        // until the answer, and unfinished for whoever samples it
        cx.passes[p].paint_dirty = true;
        cx.passes[p].hold_picture();
        assert!(!cx.passes[p].paint_dirty);
        assert!(cx.passes[p].incomplete);
        assert_eq!(cx.passes[p].painted_serial, 8);
    }

    #[test]
    fn a_drawing_is_one_draw_event_however_many_draw_contexts_it_makes() {
        let mut cx = Cx::new(Box::new(|cx, event| {
            if let Event::Draw(_) = event {
                // a main window's draw context, then a second window's
                cx.begin_draw_context();
                cx.begin_draw_context();
            }
        }));
        let first = cx.drawing_id();
        cx.call_draw_event(0.0);
        assert_eq!(cx.drawing_id(), first + 1);
        assert_eq!(cx.redraw_id, cx.drawing_start + 2, "both draw contexts belong to it");
        // a draw context made outside a draw event (an offscreen bake) is a drawing of its own
        cx.begin_draw_context();
        assert_eq!(cx.drawing_id(), first + 2);
        cx.begin_draw_context();
        assert_eq!(cx.drawing_id(), first + 3);
    }

    /// The scripted drawing of the redraw tests: a main window's draw context
    /// records the UI, then a second window's (an output window) records a
    /// host list that attaches a history pass every drawing and a ring's
    /// tile write when a tile is due. `tile_in_first_context` records the
    /// tile under the first context instead.
    struct RedrawScript {
        ui: DrawListId,
        host_list: DrawListId,
        host: DrawPassId,
        ring: (DrawPassId, DrawListId),
        history: (DrawPassId, DrawListId),
        newer: Option<(DrawPassId, DrawListId)>,
        tile_due: bool,
        tile_in_first_context: bool,
    }

    fn scripted_cx() -> (Cx, Rc<std::cell::RefCell<Option<RedrawScript>>>) {
        let script: Rc<std::cell::RefCell<Option<RedrawScript>>> = Rc::new(std::cell::RefCell::new(None));
        let s = script.clone();
        let cx = Cx::new(Box::new(move |cx, event| {
            if !matches!(event, Event::Draw(_)) {
                return;
            }
            let s = s.borrow();
            let Some(s) = s.as_ref() else { return };
            let record = |cx: &mut Cx, list: DrawListId| {
                let recording_gen = cx.next_uniform_gen();
                let uniforms_gen = cx.next_uniform_gen();
                let redraw = cx.redraw_id;
                cx.draw_lists[list].clear_draw_items(redraw, recording_gen, uniforms_gen);
            };
            let tile = |cx: &mut Cx| {
                record(cx, s.ring.1);
                cx.passes[s.ring.0].paint_dirty = true;
            };
            cx.begin_draw_context();
            record(cx, s.ui);
            if s.tile_due && s.tile_in_first_context {
                tile(cx);
            }
            cx.begin_draw_context();
            record(cx, s.host_list);
            record(cx, s.history.1);
            cx.passes[s.history.0].paint_dirty = true;
            cx.attach_child_pass(s.history.0, s.host, Some(s.host_list));
            if s.tile_due {
                if !s.tile_in_first_context {
                    tile(cx);
                }
                cx.attach_child_pass(s.ring.0, s.host, Some(s.host_list));
            }
            if let Some((newer, list)) = s.newer {
                record(cx, list);
                cx.passes[newer].paint_dirty = true;
                cx.attach_child_pass(newer, s.host, Some(s.host_list));
            }
        }));
        (cx, script)
    }

    #[test]
    fn a_frame_drawn_again_paints_what_an_earlier_drawing_of_it_recorded() {
        let (mut cx, script) = scripted_cx();
        let host = DrawPass::new(&mut cx);
        let (ui, host_list) = (DrawList::new(&mut cx), DrawList::new(&mut cx));
        let (ring, ring_list) = pass_with_current_list(&mut cx);
        let (history, history_list) = pass_with_current_list(&mut cx);
        let (ring_id, history_id) = (ring.draw_pass_id(), history.draw_pass_id());
        *script.borrow_mut() = Some(RedrawScript {
            ui: ui.id(), host_list: host_list.id(), host: host.draw_pass_id(),
            ring: (ring_id, ring_list.id()), history: (history_id, history_list.id()),
            newer: None, tile_due: true, tile_in_first_context: true,
        });
        let set = |f: &dyn Fn(&mut RedrawScript)| f(script.borrow_mut().as_mut().unwrap());
        let mut todo = Vec::new();

        // The first drawing records the tile, under the first of its two
        // draw contexts; its glyphs are not there yet.
        cx.call_draw_event(0.0);
        cx.frame_drawn_again();
        set(&|s| s.tile_due = false);
        // Drawn again (no tile due) and still waiting for glyphs: drawn a
        // third time.
        cx.call_draw_event(0.0);
        assert!(cx.pass_attachment_is_stale(ring_id), "the later drawing dropped the tile");
        cx.frame_drawn_again();
        cx.call_draw_event(0.0);
        cx.compute_pass_repaint_order(&mut todo);
        assert!(todo.contains(&ring_id), "the tile the first drawing recorded paints");
        assert!(todo.contains(&history_id));
        assert!(cx.pass_attachment_is_stale(ring_id), "kept, its attachment stamp as it was recorded");

        // A build that could not paint it (a window that took no frame):
        // kept through the next frame, which records no tile either.
        cx.release_painted_recordings();
        cx.call_draw_event(0.0);
        cx.compute_pass_repaint_order(&mut todo);
        assert!(todo.contains(&ring_id), "kept until it paints whole");
        // Painted whole: an orphan again, as any pass a frame does not record.
        cx.repaint_id += 1;
        let repaint = cx.repaint_id;
        cx.passes[ring_id].painted(true, repaint);
        cx.passes[ring_id].paint_dirty = false;
        cx.release_painted_recordings();
        cx.call_draw_event(0.0);
        cx.passes[ring_id].paint_dirty = true;
        cx.compute_pass_repaint_order(&mut todo);
        assert!(!todo.contains(&ring_id));
        assert!(!cx.passes[ring_id].kept_recording);

        // A pass the later drawing records again is its own.
        set(&|s| s.tile_due = true);
        cx.call_draw_event(0.0);
        cx.frame_drawn_again();
        cx.call_draw_event(0.0);
        assert!(!cx.passes[ring_id].kept_recording);
    }

    #[test]
    fn a_draw_context_made_while_the_wait_pumps_does_not_hide_the_drawing_taken() {
        let (mut cx, script) = scripted_cx();
        let host = DrawPass::new(&mut cx);
        let (ui, host_list) = (DrawList::new(&mut cx), DrawList::new(&mut cx));
        let (ring, ring_list) = pass_with_current_list(&mut cx);
        let (history, history_list) = pass_with_current_list(&mut cx);
        let ring_id = ring.draw_pass_id();
        *script.borrow_mut() = Some(RedrawScript {
            ui: ui.id(), host_list: host_list.id(), host: host.draw_pass_id(),
            ring: (ring_id, ring_list.id()), history: (history.draw_pass_id(), history_list.id()),
            newer: None, tile_due: true, tile_in_first_context: false,
        });
        cx.call_draw_event(0.0);
        // the glyph wait pumps events, and something makes a draw context
        // outside a draw event (a drawing of its own) before the take
        cx.begin_draw_context();
        cx.frame_drawn_again();
        script.borrow_mut().as_mut().unwrap().tile_due = false;
        cx.call_draw_event(0.0);
        assert!(cx.passes[ring_id].kept_recording, "the tile the first drawing recorded is kept");
    }

    #[test]
    fn a_frame_drawn_again_keeps_a_pass_whose_depth_buffer_a_newer_pass_shares() {
        use crate::draw_pass::{DrawPassClearColor, DrawPassClearDepth};
        use crate::makepad_math::vec4;
        use crate::texture::{Texture, TextureFormat, TextureSize};
        let (mut cx, script) = scripted_cx();
        let host = DrawPass::new(&mut cx);
        let (ui, host_list) = (DrawList::new(&mut cx), DrawList::new(&mut cx));
        let (ring, ring_list) = pass_with_current_list(&mut cx);
        let (history, history_list) = pass_with_current_list(&mut cx);
        let (newer, newer_list) = pass_with_current_list(&mut cx);
        let size = TextureSize::Fixed { width: 4, height: 4 };
        let colour = |cx: &mut Cx| Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: size.clone(), initial: true });
        let (ring_target, newer_target) = (colour(&mut cx), colour(&mut cx));
        let depth = Texture::new_with_format(&mut cx, TextureFormat::DepthD32 { size: size.clone(), initial: true });
        let clear = DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0));
        ring.set_color_texture(&mut cx, &ring_target, clear.clone());
        newer.set_color_texture(&mut cx, &newer_target, clear);
        ring.set_depth_texture(&mut cx, &depth, DrawPassClearDepth::ClearWith(1.0));
        newer.set_depth_texture(&mut cx, &depth, DrawPassClearDepth::ClearWith(1.0));
        let ring_id = ring.draw_pass_id();
        *script.borrow_mut() = Some(RedrawScript {
            ui: ui.id(), host_list: host_list.id(), host: host.draw_pass_id(),
            ring: (ring_id, ring_list.id()), history: (history.draw_pass_id(), history_list.id()),
            newer: None, tile_due: true, tile_in_first_context: false,
        });
        cx.call_draw_event(0.0);
        cx.frame_drawn_again();
        {
            let mut s = script.borrow_mut();
            let s = s.as_mut().unwrap();
            s.tile_due = false;
            s.newer = Some((newer.draw_pass_id(), newer_list.id()));
        }
        cx.call_draw_event(0.0);
        assert!(cx.passes[ring_id].kept_recording, "a shared depth buffer is scratch, not a contested picture");
    }

    #[test]
    fn a_frame_drawn_again_does_not_keep_a_pass_over_a_newer_recording_of_its_target() {
        use crate::draw_pass::DrawPassClearColor;
        use crate::makepad_math::vec4;
        use crate::texture::{Texture, TextureFormat, TextureSize};
        let (mut cx, script) = scripted_cx();
        let host = DrawPass::new(&mut cx);
        let (ui, host_list) = (DrawList::new(&mut cx), DrawList::new(&mut cx));
        let (ring, ring_list) = pass_with_current_list(&mut cx);
        let (history, history_list) = pass_with_current_list(&mut cx);
        let (newer, newer_list) = pass_with_current_list(&mut cx);
        let target = Texture::new_with_format(&mut cx, TextureFormat::RenderBGRAu8 {
            size: TextureSize::Fixed { width: 4, height: 4 },
            initial: true,
        });
        let clear = DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0));
        ring.set_color_texture(&mut cx, &target, clear.clone());
        newer.set_color_texture(&mut cx, &target, clear);
        let ring_id = ring.draw_pass_id();
        *script.borrow_mut() = Some(RedrawScript {
            ui: ui.id(), host_list: host_list.id(), host: host.draw_pass_id(),
            ring: (ring_id, ring_list.id()), history: (history.draw_pass_id(), history_list.id()),
            newer: None, tile_due: true, tile_in_first_context: false,
        });
        cx.call_draw_event(0.0);
        cx.frame_drawn_again();
        {
            let mut s = script.borrow_mut();
            let s = s.as_mut().unwrap();
            s.tile_due = false;
            s.newer = Some((newer.draw_pass_id(), newer_list.id()));
        }
        cx.call_draw_event(0.0);
        assert!(!cx.passes[ring_id].kept_recording, "the newer recording owns the target");
    }

    #[test]
    fn orphaned_child_pass_is_not_repainted_until_reattached() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let parent = DrawPass::new(&mut cx);
        let child = DrawPass::new(&mut cx);
        let list = DrawList::new(&mut cx);
        let (parent_id, child_id) = (parent.draw_pass_id(), child.draw_pass_id());
        let mut todo = Vec::new();

        // Recorded at redraw 1, attaching the child: painted, and its
        // liveness rides on the parent.
        let recording_gen = cx.next_uniform_gen();
        let uniforms_gen = cx.next_uniform_gen();
        cx.draw_lists[list.id()].clear_draw_items(1, recording_gen, uniforms_gen);
        cx.attach_child_pass(child_id, parent_id, Some(list.id()));
        cx.passes[child_id].live_with_parent = true;
        cx.passes[parent_id].paint_dirty = true;
        cx.compute_pass_repaint_order(&mut todo);
        assert!(todo.contains(&child_id));
        assert!(todo.contains(&parent_id));

        // Recorded again at redraw 2 without the child: orphaned. Neither the
        // parent's repaint nor a direct dirty flag paints it, and the flag is
        // cleared so nothing keeps the frame loop awake for it.
        let recording_gen = cx.next_uniform_gen();
        let uniforms_gen = cx.next_uniform_gen();
        cx.draw_lists[list.id()].clear_draw_items(2, recording_gen, uniforms_gen);
        cx.passes[parent_id].paint_dirty = true;
        cx.passes[child_id].paint_dirty = true;
        cx.compute_pass_repaint_order(&mut todo);
        assert!(!todo.contains(&child_id));
        assert!(todo.contains(&parent_id));
        assert!(!cx.passes[child_id].paint_dirty);

        // An explicit request paints an orphan this once.
        cx.repaint_pass(child_id);
        cx.compute_pass_repaint_order(&mut todo);
        assert!(todo.contains(&child_id));
        cx.passes[child_id].paint_dirty = true;
        cx.compute_pass_repaint_order(&mut todo);
        assert!(!todo.contains(&child_id));

        // Re-attached by the current recording: live again.
        cx.attach_child_pass(child_id, parent_id, Some(list.id()));
        cx.passes[child_id].paint_dirty = true;
        cx.compute_pass_repaint_order(&mut todo);
        assert!(todo.contains(&child_id));

        // A freed attaching list orphans too.
        drop(list);
        cx.passes[child_id].paint_dirty = true;
        cx.compute_pass_repaint_order(&mut todo);
        assert!(!todo.contains(&child_id));
    }

    fn pass_with_current_list(cx: &mut Cx) -> (DrawPass, DrawList) {
        let pass = DrawPass::new(cx);
        let list = DrawList::new(cx);
        let recording_gen = cx.next_uniform_gen();
        let uniforms_gen = cx.next_uniform_gen();
        let redraw_id = cx.redraw_id;
        cx.draw_lists[list.id()].clear_draw_items(redraw_id, recording_gen, uniforms_gen);
        cx.passes[pass.draw_pass_id()].main_draw_list_id = Some(list.id());
        (pass, list)
    }

    #[test]
    fn time_repaint_only_repaints_the_passes_that_asked() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (animated, _animated_list) = pass_with_current_list(&mut cx);
        let (idle, _idle_list) = pass_with_current_list(&mut cx);
        let (animated, idle) = (animated.draw_pass_id(), idle.draw_pass_id());
        let mut todo = Vec::new();

        cx.passes[animated].repaint_next = true;
        cx.demo_time_repaint = true;
        cx.compute_pass_repaint_order(&mut todo);
        assert_eq!(todo, vec![animated]);
        assert!(!cx.passes[idle].paint_dirty);

        // The flag is used up: without another request nothing repaints.
        cx.passes[animated].paint_dirty = false;
        cx.compute_pass_repaint_order(&mut todo);
        assert!(todo.is_empty());
    }

    #[test]
    fn pass_clock_rebases_only_while_nothing_owes_a_time_frame() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let (pass, _list) = pass_with_current_list(&mut cx);
        let pass = pass.draw_pass_id();
        let mut todo = Vec::new();

        assert_eq!(cx.pass_time(100.0), 0.0);
        assert_eq!(cx.pass_time(100.5), 0.5);

        // An animating pass holds the origin, even on a beat that skips painting it.
        cx.passes[pass].repaint_next = true;
        cx.compute_pass_repaint_order(&mut todo);
        assert_eq!(cx.pass_time(101.0), 1.0);
        cx.compute_pass_repaint_order(&mut todo);
        assert_eq!(cx.pass_time(101.5), 1.5);

        // Once it painted without asking again, the next repaint starts the clock over.
        cx.passes[pass].paint_dirty = false;
        cx.compute_pass_repaint_order(&mut todo);
        assert_eq!(cx.pass_time(200.0), 0.0);
        assert_eq!(cx.pass_time(200.25), 0.25);

        // A pass that still owes a frame but was left behind by a redraw can't paint, so it owes nothing.
        cx.passes[pass].repaint_next = true;
        cx.compute_pass_repaint_order(&mut todo);
        assert_eq!(cx.pass_time(201.0), 1.0);
        cx.redraw_id += 1;
        cx.compute_pass_repaint_order(&mut todo);
        assert_eq!(cx.pass_time(300.0), 0.0);
    }

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

    /// A host's `StudioToApp::MouseCancel` reaches the child as a real
    /// cancellation — `Event::FingerCancel`, which hidden widgets admit —
    /// never as a MouseUp, and the mouse's press is retired afterwards.
    #[test]
    fn a_hosted_mouse_cancel_is_a_finger_cancel_not_a_mouse_up() {
        let seen = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let log = seen.clone();
        let mut cx = Cx::new(Box::new(move |_cx, event| {
            log.borrow_mut().push(event.name().to_string());
        }));
        let window = crate::window::WindowId(0, 0);
        cx.fingers.mouse_down(crate::event::MouseButton::PRIMARY, window);
        cx.fingers.capture_digit(live_id!(mouse).into(), Area::Empty, Area::Empty, 0.0, crate::makepad_math::dvec2(5.0, 5.0));
        assert!(cx.fingers.any_areas_captured());
        // The studio arm is exactly this plus the app-only capture pacing.
        cx.dispatch_hosted_mouse_cancel(
            &makepad_studio_protocol::RemoteMouseUp {
                button_raw_bits: crate::event::MouseButton::PRIMARY.bits(),
                x: 5.0,
                y: 5.0,
                ..Default::default()
            },
            window,
            crate::makepad_math::dvec2(0.0, 0.0),
        );
        let seen = seen.borrow();
        assert!(seen.iter().any(|n| n == "FingerCancel"), "no FingerCancel: {seen:?}");
        assert!(!seen.iter().any(|n| n == "MouseUp"), "the cancel was dispatched as a MouseUp: {seen:?}");
        assert!(!cx.fingers.any_areas_captured(), "the cancelled press was not retired");
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

/// Hosted-child frame demand, shared by every `--stdin-loop` backend. The
/// host ticks a child only while the child says it has something to do:
/// after a Tick it announces a frame (`RequestAnimationFrame`) or the
/// delay of its next timer (`WakeIn`), and a worker thread that raises a
/// UI signal wakes its loop to service the new work. An idle child needs no periodic Tick.
#[cfg(not(target_arch = "wasm32"))]
mod hosted_demand {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A wake request is on its way to the host and no Tick has answered it.
    static WAKE_SENT: AtomicBool = AtomicBool::new(false);

    pub fn request_frame() {
        if !WAKE_SENT.swap(true, Ordering::AcqRel) {
            super::Cx::send_studio_message(super::AppToStudio::RequestAnimationFrame);
        }
    }

    pub fn wake_from_thread() {
        // A HAL client services signals in its own pump, then announces visible work.
        // Its host socket also wakes that pump: asking the host for a frame here would
        // turn each incoming Tick into another frame request, even while idle or hidden.
        crate::thread::wake_ui_event_loop();
    }

    pub fn tick_began() {
        WAKE_SENT.store(false, Ordering::Release);
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Cx {
    /// Wake a hosted child on worker signals. Socket-read loops ask the host for a
    /// Tick; the HAL wakes its event pump and announces demand after servicing signals.
    pub(crate) fn stdin_install_wake(&mut self) {
        // Each host message used to raise the UI signal too: a full-app
        // `Event::Signal` per Tick (a hosted Stage pumped 100+ times a second idle).
        self.studio_socket_wakes_loop_only();
        hosted_demand::tick_began();
        crate::makepad_network::install_ui_waker(Some(crate::makepad_network::UiWaker::new(
            hosted_demand::wake_from_thread,
        )));
    }

    /// A Tick is being consumed: wakes raised from now on are new ones.
    pub(crate) fn stdin_tick_began(&mut self) {
        hosted_demand::tick_began();
    }

    /// A hidden hosted child keeps its frame work (next-frame callbacks, draws) until the
    /// shared HostedVisible dispatch shows it again -- except while a remote request needs a
    /// frame (a grab, a step, input that waits for its frame): the remote is never starved.
    pub(crate) fn hosted_frames_held(&self) -> bool {
        self.hosted_hidden && !crate::remote::needs_ticks()
    }

    /// At most one frame request until the host answers with a Tick, none while frames are held.
    pub(crate) fn stdin_request_frame(&self) {
        if !self.hosted_frames_held() {
            hosted_demand::request_frame();
        }
    }

    /// End of a Tick: tell the host what this child still needs. A pending
    /// frame (next-frame callbacks, a redraw) asks for the next Tick; else
    /// the earliest timer says in how long. Nothing pending says nothing.
    pub(crate) fn stdin_announce_demand(&self, timers: &crate::os::shared_framebuf::PollTimers) {
        // As in call_next_frame_event, a virtual clock advances callbacks only on /step.
        let next_frames_due = !self.new_next_frames.is_empty() && !crate::remote::app_clock::enabled();
        if !self.hosted_frames_held() && (next_frames_due || self.need_redrawing()) {
            self.stdin_request_frame();
        } else if let Some(delay) = timers.next_due_in() {
            Self::send_studio_message(AppToStudio::WakeIn(delay));
        }
    }
}

/// Hosted-child input pacing, shared by every `--stdin-loop` backend.
impl Cx {
    /// Collapse one drained host batch before dispatch: every `Tick` but
    /// the last goes (one draw per drain, however far behind the child
    /// fell), and a `MouseMove` that another `MouseMove` follows directly
    /// is replaced by it (a frame can only show the pointer's latest
    /// position). Everything else keeps its order, so a Down/Up/Scroll
    /// still sees the move that preceded it.
    #[cfg(not(target_arch = "wasm32"))]
    /// Only the Ticks of a batch fold into its last; every pointer sample
    /// stays (a touch child's velocity tracker needs each one).
    #[allow(dead_code)]
    pub(crate) fn stdin_coalesce_host_ticks(msgs: &mut Vec<StudioToApp>) {
        let ticks = msgs.iter().filter(|msg| matches!(msg, StudioToApp::Tick)).count();
        if ticks < 2 {
            return;
        }
        let mut seen = 0;
        msgs.retain(|msg| {
            if matches!(msg, StudioToApp::Tick) {
                seen += 1;
                seen == ticks
            } else {
                true
            }
        });
    }

    #[allow(dead_code)]
    pub(crate) fn stdin_coalesce_host_batch(msgs: &mut Vec<StudioToApp>) {
        let ticks = msgs
            .iter()
            .filter(|msg| matches!(msg, StudioToApp::Tick))
            .count();
        let move_runs = msgs.windows(2).any(|pair| {
            matches!(
                (&pair[0], &pair[1]),
                (StudioToApp::MouseMove(_), StudioToApp::MouseMove(_))
            )
        });
        if ticks < 2 && !move_runs {
            return;
        }
        let mut seen_ticks = 0;
        let mut out: Vec<StudioToApp> = Vec::with_capacity(msgs.len());
        for msg in msgs.drain(..) {
            match msg {
                StudioToApp::Tick => {
                    seen_ticks += 1;
                    if seen_ticks == ticks {
                        out.push(StudioToApp::Tick);
                    }
                }
                StudioToApp::MouseMove(e) => {
                    if matches!(out.last(), Some(StudioToApp::MouseMove(_))) {
                        out.pop();
                    }
                    out.push(StudioToApp::MouseMove(e));
                }
                other => out.push(other),
            }
        }
        *msgs = out;
    }

    /// Pull every host batch that is already queued into `into`, so a
    /// child that fell behind sees its whole backlog at once and can
    /// coalesce it. Returns true when the socket closed or failed; the
    /// caller dispatches what it has and then leaves its loop.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn stdin_drain_host_batches(&mut self, into: &mut Vec<StudioToApp>) -> bool {
        use crate::makepad_micro_serde::*;
        use crate::web_socket::WebSocketMessage;
        use makepad_studio_protocol::StudioToAppVec;
        loop {
            match self.try_recv_studio_websocket_message() {
                Some(WebSocketMessage::Binary(data)) => {
                    match StudioToAppVec::decode_bin(&data) {
                        Ok((msgs, errors)) => {
                            for error in errors {
                                crate::error!("{error}");
                            }
                            into.extend(msgs.0)
                        }
                        Err(err) => crate::error!(
                            "Cant parse studio websocket binary payload in --stdin-loop: {:?}",
                            err
                        ),
                    }
                }
                Some(WebSocketMessage::String(text)) => match StudioToApp::deserialize_json(&text) {
                    Ok(msg) => into.push(msg),
                    Err(_) => {
                        if !text.trim().is_empty() {
                            crate::warning!(
                                "Ignoring unexpected studio websocket text: {}",
                                text.trim()
                            );
                        }
                    }
                },
                Some(WebSocketMessage::Error(err)) => {
                    crate::error!("Studio websocket error in --stdin-loop: {}", err);
                    return true;
                }
                Some(WebSocketMessage::Closed) => return true,
                Some(WebSocketMessage::Opened) => {}
                None => return false,
            }
        }
    }
}

/// makepad inspect's retry boundary hooks (dlsym'd once; absent in a rustc build): set while an
/// agent watches the app.
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod relax_retry {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    pub struct Hooks {
        on: &'static AtomicBool,
        pub enter: extern "C" fn(*const u8, usize),
        pub leave: extern "C" fn(),
        pub take: extern "C" fn() -> bool,
    }

    pub fn hooks() -> Option<&'static Hooks> {
        static H: OnceLock<Option<Hooks>> = OnceLock::new();
        H.get_or_init(|| {
            extern "C" {
                fn dlsym(handle: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
            }

            #[cfg(target_os = "macos")]
            let default = -2isize as *mut std::ffi::c_void;
            #[cfg(not(target_os = "macos"))]
            let default = std::ptr::null_mut();
            let get = |n: &std::ffi::CStr| unsafe { dlsym(default, n.as_ptr()) };
            let (on, enter, leave, take) = (get(c"relax_retry_on"), get(c"relax_retry_enter"), get(c"relax_retry_leave"), get(c"relax_retry_take"));
            if on.is_null() || enter.is_null() || leave.is_null() || take.is_null() {
                return None;
            }
            Some(unsafe {
                Hooks {
                    on: &*(on as *const AtomicBool),
                    enter: std::mem::transmute::<*mut std::ffi::c_void, extern "C" fn(*const u8, usize)>(enter),
                    leave: std::mem::transmute::<*mut std::ffi::c_void, extern "C" fn()>(leave),
                    take: std::mem::transmute::<*mut std::ffi::c_void, extern "C" fn() -> bool>(take),
                }
            })
        })
        .as_ref()
        .filter(|h| h.on.load(Ordering::Relaxed))
    }
}
