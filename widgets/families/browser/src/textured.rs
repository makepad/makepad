//! The browser family shares one engine host per Cx. Widgets own its pages;
//! dropping the last widget also drops (and reaps) the host.
use crate::makepad_draw::*;
use crate::makepad_platform::native_layer::NativeLayerOperation;

pub(crate) struct WebFrame {
    pub texture: Texture,
    pub scale: Vec2f,
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use makepad_browser_view::{BrowserViews, ViewId, ViewOp};
    use std::{cell::RefCell, rc::{Rc, Weak}};

    struct SharedViews {
        views: BrowserViews,
        event: u64,
        changed: bool,
        service_requested: bool,
    }

    #[derive(Default)]
    struct Host(Weak<RefCell<SharedViews>>);

    #[derive(Default)]
    pub(crate) struct TexturedWebView {
        host: Option<Rc<RefCell<SharedViews>>>,
        view: Option<ViewId>,
        attempted: bool,
        realized: bool,
        hidden: bool,
        size: Option<(Vec2d, f64)>,
    }

    impl TexturedWebView {
        pub fn open(&mut self, cx: &mut Cx, url: &str) {
            if self.attempted || cx.script_data.std.host_io_only() {
                return;
            }
            self.attempted = true;
            let shared = cx.global::<Host>().0.upgrade().unwrap_or_else(|| {
                let profile = std::env::current_exe().ok()
                    .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "makepad-web-view".into());
                let shared = Rc::new(RefCell::new(SharedViews {
                    views: BrowserViews::new(&profile), event: 0, changed: false, service_requested: false,
                }));
                cx.global::<Host>().0 = Rc::downgrade(&shared);
                shared
            });
            self.view = shared.borrow_mut().views.open(url, false, 0x0000_0000);
            if let Some(view) = self.view.filter(|_| self.hidden) {
                shared.borrow_mut().views.send(view, ViewOp::Hidden { on: true });
            }
            self.host = Some(shared);
        }

        pub fn is_open(&self) -> bool { self.view.is_some() }

        pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> bool {
            let Some(host) = &self.host else { return false };
            let mut host = host.borrow_mut();
            // Every WebView sees the event; only the first pumps the shared
            // socket/rings. Later widgets still see the changed state.
            if host.event != cx.event_id() {
                host.event = cx.event_id();
                host.changed = host.views.handle_event(cx, event);
                host.service_requested = false;
            }
            host.changed
        }

        pub fn events(&mut self, previous: &crate::web_view::WebViewState) -> Vec<NativeLayerEventKind> {
            let (Some(host), Some(view)) = (&self.host, self.view) else { return Vec::new() };
            let mut host = host.borrow_mut();
            let mut events = Vec::new();
            if !self.realized {
                if let Some(frame) = host.views.frame(view) {
                    self.realized = true;
                    events.push(NativeLayerEventKind::Realized(NativeLayerMode::Textured(frame.texture)));
                }
            }
            if let Some(state) = host.views.state(view) {
                if state.title != previous.title { events.push(NativeLayerEventKind::Title(state.title.clone())); }
                if state.url != previous.url { events.push(NativeLayerEventKind::Url(state.url.clone())); }
                if state.loading != previous.loading { events.push(NativeLayerEventKind::Loading(state.loading)); }
                if (state.can_go_back, state.can_go_forward) != (previous.can_go_back, previous.can_go_forward) {
                    events.push(NativeLayerEventKind::Nav { can_back: state.can_go_back, can_forward: state.can_go_forward });
                }
                if state.error != previous.error {
                    if let Some(error) = &state.error { events.push(NativeLayerEventKind::Crashed(error.clone())); }
                }
            }
            // WebView has no tab strip. Requests for a new window navigate
            // this view, as a single embedded page; console output stays in
            // the engine log rather than accumulating in an unread queue.
            for url in host.views.take_popups(view) {
                host.views.send(view, ViewOp::Navigate { url });
            }
            host.views.take_console(view);
            events
        }

        pub fn frame(&mut self, cx: &mut Cx, size: Vec2d, dpi: f64) -> Option<WebFrame> {
            let (Some(host), Some(view)) = (&self.host, self.view) else { return None };
            let mut host = host.borrow_mut();
            if self.size != Some((size, dpi)) {
                self.size = Some((size, dpi));
                host.views.set_size(view, size, dpi);
                // Service all pages together after layout, not all N rings
                // once for each of the N widgets drawn in this frame.
                if !host.service_requested {
                    host.service_requested = true;
                    cx.new_next_frame();
                }
            }
            let frame = host.views.frame(view)?;
            (frame.info.width > 0 && frame.info.height > 0).then(|| WebFrame {
                texture: frame.texture,
                scale: vec2(frame.info.width as f32 / frame.alloc[0] as f32,
                    frame.info.height as f32 / frame.alloc[1] as f32),
            })
        }

        pub fn send(&mut self, op: &NativeLayerOperation) {
            let (Some(host), Some(view)) = (&self.host, self.view) else { return };
            let mut host = host.borrow_mut();
            if matches!(op, NativeLayerOperation::Blur) { host.views.send(view, ViewOp::CaptureLost); }
            let op = match op {
                NativeLayerOperation::Navigate { url, .. } => ViewOp::Navigate { url: url.clone() },
                NativeLayerOperation::History(delta) => ViewOp::History { delta: *delta },
                NativeLayerOperation::Reload => ViewOp::Reload,
                NativeLayerOperation::Stop => ViewOp::Stop,
                NativeLayerOperation::Focus => ViewOp::Focus { on: true },
                NativeLayerOperation::Blur => ViewOp::Focus { on: false },
                _ => return,
            };
            host.views.send(view, op);
        }

        pub fn set_hidden(&mut self, hidden: bool) {
            if self.hidden == hidden { return; }
            self.hidden = hidden;
            if let (Some(host), Some(view)) = (&self.host, self.view) {
                let mut host = host.borrow_mut();
                if hidden { host.views.send(view, ViewOp::CaptureLost); }
                host.views.send(view, ViewOp::Hidden { on: hidden });
            }
        }

        pub fn forward_hit(&mut self, cx: &mut Cx, hit: &Hit, rect: Rect) {
            if let (Some(host), Some(view)) = (&self.host, self.view) {
                let mut host = host.borrow_mut();
                match hit {
                    Hit::FingerHoverOut(e) => {
                        let at = e.abs - rect.pos;
                        host.views.send(view, ViewOp::MouseMove {
                            x: at.x.round() as i32, y: at.y.round() as i32,
                            modifiers: makepad_browser_view::cef_input::cef_modifiers(e.modifiers, MouseButton::empty()),
                            leave: true,
                        });
                    }
                    Hit::FingerUp(e) if e.cancelled => host.views.send(view, ViewOp::CaptureLost),
                    _ => { host.views.forward_hit(cx, view, hit, |abs| Some(abs - rect.pos)); }
                }
            }
        }

        pub fn close(&mut self, cx: &mut Cx) {
            if let (Some(host), Some(view)) = (&self.host, self.view.take()) {
                let mut host = host.borrow_mut();
                host.views.close(view);
                host.views.service(cx);
            }
            *self = Self::default();
        }
    }

    impl Drop for TexturedWebView {
        fn drop(&mut self) {
            if let (Some(host), Some(view)) = (&self.host, self.view.take()) {
                host.borrow_mut().views.close(view);
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::TexturedWebView;

// The browser host is a native process. Keep the browser family available
// on wasm without linking the native frame-ring/process client.
#[cfg(target_arch = "wasm32")]
#[derive(Default)]
pub(crate) struct TexturedWebView;

#[cfg(target_arch = "wasm32")]
impl TexturedWebView {
    pub fn open(&mut self, _cx: &mut Cx, _url: &str) {}
    pub fn is_open(&self) -> bool { false }
    pub fn handle_event(&mut self, _cx: &mut Cx, _event: &Event) -> bool { false }
    pub fn events(&mut self, _previous: &crate::web_view::WebViewState) -> Vec<NativeLayerEventKind> { Vec::new() }
    pub fn frame(&mut self, _cx: &mut Cx, _size: Vec2d, _dpi: f64) -> Option<WebFrame> { None }
    pub fn send(&mut self, _op: &NativeLayerOperation) {}
    pub fn set_hidden(&mut self, _hidden: bool) {}
    pub fn forward_hit(&mut self, _cx: &mut Cx, _hit: &Hit, _rect: Rect) {}
    pub fn close(&mut self, _cx: &mut Cx) {}
}
