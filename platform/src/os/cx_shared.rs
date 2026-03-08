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
        AppToStudio, EventSample, ScreenshotResponse, StudioToApp, WidgetQueryResponse,
        WidgetTreeDumpResponse,
    },
    std::cell::{Cell, RefCell},
    std::collections::{HashMap, HashSet},
    std::rc::Rc,
};

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

    pub(crate) fn compute_pass_repaint_order(&mut self, passes_todo: &mut Vec<DrawPassId>) {
        passes_todo.clear();

        // we need this because we don't mark the entire deptree of passes dirty every small paint
        loop {
            // loop untill we don't propagate anymore
            let mut altered = false;
            for draw_pass_id in self.passes.id_iter() {
                if self.demo_time_repaint {
                    if self.passes[draw_pass_id].main_draw_list_id.is_some() {
                        self.passes[draw_pass_id].paint_dirty = true;
                    }
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
            if !altered {
                break;
            }
        }

        for draw_pass_id in self.passes.id_iter() {
            if self.passes[draw_pass_id].paint_dirty {
                let mut inserted = false;
                match self.passes[draw_pass_id].parent {
                    CxDrawPassParent::Window(_) | CxDrawPassParent::Xr => {}
                    CxDrawPassParent::DrawPass(dep_of_pass_id) => {
                        if draw_pass_id == dep_of_pass_id {
                            panic!()
                        }
                        for insert_before in 0..passes_todo.len() {
                            if passes_todo[insert_before] == dep_of_pass_id {
                                passes_todo.insert(insert_before, draw_pass_id);
                                inserted = true;
                                break;
                            }
                        }
                    }
                    CxDrawPassParent::None => {
                        // we need to be first
                        passes_todo.insert(0, draw_pass_id);
                        inserted = true;
                    }
                }
                if !inserted {
                    passes_todo.push(draw_pass_id);
                }
            }
        }
        self.demo_time_repaint = false;
    }

    pub(crate) fn need_redrawing(&self) -> bool {
        self.new_draw_event.will_redraw()
    }

    pub(crate) fn dispatch_network_runtime_events(&mut self) {
        let mut responses = Vec::new();
        while let Some(response) = self.net.try_recv() {
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

    #[allow(dead_code)]
    pub(crate) fn take_studio_screenshot_request_ids(&mut self, kind_id: u32) -> Vec<u64> {
        let mut request_ids = Vec::new();
        self.screenshot_requests.retain(|request| {
            if request.kind_id == kind_id {
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
        Cx::send_studio_message(AppToStudio::Screenshot(ScreenshotResponse {
            request_ids,
            png,
            width,
            height,
        }));
    }

    #[allow(dead_code)]
    pub(crate) fn send_studio_widget_tree_dump_response(&mut self, request_id: u64) {
        self.widget_tree_dump_requests.push(request_id);
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

    pub(crate) fn try_send_studio_widget_tree_dump_responses(&mut self) {
        if self.widget_tree_dump_requests.is_empty() {
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
            }
            StudioToApp::MouseMove(e) => {
                self.call_event_handler(&Event::MouseMove(crate::event::MouseMoveEvent {
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
                }));
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
            StudioToApp::WidgetTreeDump(request) => {
                self.send_studio_widget_tree_dump_response(request.request_id);
            }
            StudioToApp::WidgetQuery(request) => {
                self.send_studio_widget_query_response(request.request_id, request.query);
            }
            StudioToApp::Kill => {
                self.call_event_handler(&Event::Shutdown);
                return true;
            }
            StudioToApp::Custom(data) => {
                self.call_event_handler(&Event::Custom(data));
            }
            StudioToApp::KeepAlive | StudioToApp::None => {}
            other @ StudioToApp::LiveChange { .. } => {
                self.action(other);
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
    pub fn poll_control_channel(&mut self) {
        use crate::makepad_math::dvec2;
        use crate::web_socket::CONTROL_CHANNEL;
        use crate::window::CxWindowPool;
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

    pub(crate) fn inner_call_event_handler(&mut self, event: &Event) {
        self.event_id += 1;
        if (Cx::has_studio_web_socket() && !crate::web_socket::STUDIO_STDOUT_MODE.load(std::sync::atomic::Ordering::SeqCst)) || Cx::local_profile_capture_enabled() {
            let start = self.seconds_since_app_start();
            let mut event_handler = self.event_handler.take().unwrap();
            event_handler(self, event);
            self.event_handler = Some(event_handler);
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
            let mut event_handler = self.event_handler.take().unwrap();
            event_handler(self, event);
            self.event_handler = Some(event_handler);
        }

        if Cx::has_studio_web_socket() {
            self.try_send_studio_widget_tree_dump_responses();
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

    pub(crate) fn call_event_handler(&mut self, event: &Event) {
        self.inner_call_event_handler(event);
        self.inner_key_focus_change();
        self.handle_triggers();
        self.handle_actions();
        // Drain script task queues after each event dispatch cycle so
        // widget->script calls run immediately instead of waiting for tick/timer paths.
        self.handle_script_tasks();
        // Script callbacks can enqueue actions/triggers; flush them in the same cycle.
        self.inner_key_focus_change();
        self.handle_triggers();
        self.handle_actions();
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
