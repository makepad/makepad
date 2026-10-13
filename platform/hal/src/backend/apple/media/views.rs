//! Native views over a window: the system browser (a `WKWebView`) and camera previews. Moved from
//! os/apple/apple_webview.rs (macOS, iOS) and the iOS app's camera preview layers. The views are
//! AppKit (macOS) or UIKit (iOS, tvOS) objects, subviews of the window's view: they live on the
//! window system's main thread, in a table of that thread, and every command reaches them there
//! in order. A session's views go with the session. tvOS has no WebKit: no browser there.
//!
//! Rects arrive in window points with y up from the window's bottom (AppKit's placement); a
//! UIKit parent places from the top, so they are flipped by the parent's height.

use makepad_apple_sys::*;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::media::{view_kind, view_op};
use crate::types::Id;

#[cfg(not(target_os = "tvos"))]
#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

/// A rect in window points, y up from the window's bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[cfg(not(target_os = "tvos"))]
fn make_request(url: &str) -> Option<ObjcId> {
    unsafe {
        let url_string = str_to_nsstring(url);
        if url_string == nil {
            return None;
        }
        let ns_url: ObjcId = msg_send![class!(NSURL), URLWithString: url_string];
        if ns_url == nil {
            return None;
        }
        let request: ObjcId = msg_send![class!(NSURLRequest), requestWithURL: ns_url];
        if request == nil {
            None
        } else {
            Some(request)
        }
    }
}

#[cfg(not(target_os = "tvos"))]
fn history_go(web_view: ObjcId, delta: i32) {
    if web_view == nil || delta == 0 {
        return;
    }
    unsafe {
        for _ in 0..delta.unsigned_abs() {
            if delta < 0 {
                let can_go_back: BOOL = msg_send![web_view, canGoBack];
                if can_go_back == YES {
                    let () = msg_send![web_view, goBack];
                }
            } else {
                let can_go_forward: BOOL = msg_send![web_view, canGoForward];
                if can_go_forward == YES {
                    let () = msg_send![web_view, goForward];
                }
            }
        }
    }
}

/// The browser's web view sits in a host view the size of the visible part, at the offset that
/// keeps the page anchored when its top or left is clipped.
#[cfg_attr(target_os = "tvos", allow(dead_code))]
fn clipped_browser_layout(unclipped: Rect, clipped: Rect, visible: bool) -> (Rect, Rect, bool) {
    let web_view_rect = Rect { x: unclipped.x - clipped.x, y: unclipped.y - clipped.y, w: unclipped.w, h: unclipped.h };
    let is_visible = visible && clipped.w > 0.0 && clipped.h > 0.0;
    (clipped, web_view_rect, is_visible)
}

fn ns_rect(r: Rect) -> NSRect {
    NSRect { origin: NSPoint { x: r.x, y: r.y }, size: NSSize { width: r.w.max(0.0), height: r.h.max(0.0) } }
}

/// A rect of the client (window points, y up) in the coordinates of `parent`.
#[cfg(target_os = "macos")]
fn in_parent(_parent: ObjcId, r: Rect) -> Rect {
    r
}

#[cfg(any(target_os = "ios", target_os = "tvos"))]
fn in_parent(parent: ObjcId, r: Rect) -> Rect {
    let bounds: NSRect = unsafe { msg_send![parent, bounds] };
    Rect { y: bounds.size.height - r.y - r.h, ..r }
}

/// A plain layer-backed view of the window system (`NSView` / `UIView`) to host a native view
/// or layer; `clip` masks its content to its bounds. Hidden until placed.
fn new_host_view(rect: Rect, clip: bool) -> ObjcId {
    unsafe {
        #[cfg(target_os = "macos")]
        let view: ObjcId = msg_send![class!(NSView), alloc];
        #[cfg(any(target_os = "ios", target_os = "tvos"))]
        let view: ObjcId = msg_send![class!(UIView), alloc];
        let view: ObjcId = msg_send![view, initWithFrame: ns_rect(rect)];
        if view == nil {
            return nil;
        }
        // UIKit views always have a layer
        #[cfg(target_os = "macos")]
        let () = msg_send![view, setWantsLayer: YES];
        if clip {
            let layer: ObjcId = msg_send![view, layer];
            if layer != nil {
                let () = msg_send![layer, setMasksToBounds: YES];
            }
        }
        view
    }
}

#[cfg(not(target_os = "tvos"))]
struct Browser {
    current_url: String,
    attached: Option<(u32, Id)>,
    host_view: ObjcId,
    web_view: ObjcId,
}

#[cfg(not(target_os = "tvos"))]
impl Browser {
    fn new(url: &str) -> Browser {
        let mut browser = Browser { current_url: String::new(), attached: None, host_view: nil, web_view: nil };
        browser.ensure_web_view();
        browser.set_url(url);
        browser
    }

    fn ensure_web_view(&mut self) {
        if self.web_view != nil {
            return;
        }
        unsafe {
            let config: ObjcId = msg_send![class!(WKWebViewConfiguration), new];
            let web_view: ObjcId = msg_send![class!(WKWebView), alloc];
            let web_view: ObjcId = msg_send![web_view, initWithFrame: NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 1.0, height: 1.0 } } configuration: config];
            if web_view != nil {
                let () = msg_send![web_view, setHidden: YES];
                self.web_view = web_view;
            }
        }
    }

    fn ensure_host_view(&mut self) {
        if self.host_view != nil {
            return;
        }
        let host_view = new_host_view(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }, true);
        if host_view != nil {
            unsafe {
                let () = msg_send![host_view, setHidden: YES];
            }
            self.host_view = host_view;
        }
    }

    fn ensure_attached(&mut self, window: (u32, Id), parent_view: ObjcId) {
        self.ensure_web_view();
        self.ensure_host_view();
        if self.web_view == nil || self.host_view == nil {
            return;
        }
        unsafe {
            let host_super_view: ObjcId = msg_send![self.host_view, superview];
            if self.attached != Some(window) || host_super_view != parent_view {
                let () = msg_send![self.host_view, removeFromSuperview];
                let () = msg_send![parent_view, addSubview: self.host_view];
                self.attached = Some(window);
            }
            let web_super_view: ObjcId = msg_send![self.web_view, superview];
            if web_super_view != self.host_view {
                let () = msg_send![self.web_view, removeFromSuperview];
                let () = msg_send![self.host_view, addSubview: self.web_view];
            }
        }
    }

    fn update(&mut self, window: (u32, Id), parent_view: ObjcId, unclipped: Rect, clipped: Rect, visible: bool) {
        self.ensure_attached(window, parent_view);
        unsafe {
            if self.host_view == nil || self.web_view == nil {
                return;
            }
            let (host_rect, web_view_rect, is_visible) = clipped_browser_layout(unclipped, clipped, visible);
            let () = msg_send![self.host_view, setFrame: ns_rect(host_rect)];
            let () = msg_send![self.web_view, setFrame: ns_rect(web_view_rect)];
            let () = msg_send![self.web_view, setHidden: if is_visible { NO } else { YES }];
            let () = msg_send![self.host_view, setHidden: if is_visible { NO } else { YES }];
        }
    }

    fn detach(&mut self) {
        unsafe {
            if self.web_view != nil {
                let () = msg_send![self.web_view, removeFromSuperview];
                let () = msg_send![self.web_view, setHidden: YES];
            }
            if self.host_view != nil {
                let () = msg_send![self.host_view, removeFromSuperview];
                let () = msg_send![self.host_view, setHidden: YES];
            }
        }
        self.attached = None;
    }

    fn set_url(&mut self, url: &str) {
        if self.current_url == url {
            return;
        }
        self.ensure_web_view();
        let Some(request) = make_request(url) else {
            return;
        };
        unsafe {
            if self.web_view != nil {
                let () = msg_send![self.web_view, loadRequest: request];
                self.current_url.clear();
                self.current_url.push_str(url);
            }
        }
    }

    fn cleanup(&mut self) {
        unsafe {
            if self.web_view != nil {
                let () = msg_send![self.web_view, stopLoading];
            }
        }
        self.detach();
    }
}

/// A native preview of an open capture session (an `AVCaptureVideoPreviewLayer` in a view over
/// the window). Moved from `MacosNativeCameraPreview` (os/apple/macos/macos.rs).
struct Preview {
    attached: Option<(u32, Id)>,
    host_view: ObjcId,
    preview_layer: ObjcId,
}

impl Preview {
    fn ensure_attached(&mut self, window: (u32, Id), parent_view: ObjcId, rect: Rect) {
        unsafe {
            if self.attached != Some(window) || self.host_view == nil {
                self.detach();
                let host_view = new_host_view(rect, false);
                if host_view == nil {
                    return;
                }
                let () = msg_send![parent_view, addSubview: host_view];
                self.host_view = host_view;
                self.attached = Some(window);
            }
        }
    }

    /// The layer showing `session`, once there is one to show.
    fn ensure_layer(&mut self, session: ObjcId) {
        unsafe {
            if self.preview_layer != nil || session == nil || self.host_view == nil {
                return;
            }
            let preview_layer: ObjcId = msg_send![class!(AVCaptureVideoPreviewLayer), layerWithSession: session];
            if preview_layer != nil {
                let gravity = str_to_nsstring("AVLayerVideoGravityResizeAspectFill");
                let () = msg_send![preview_layer, setVideoGravity: gravity];
                let layer: ObjcId = msg_send![self.host_view, layer];
                if layer != nil {
                    let () = msg_send![layer, addSublayer: preview_layer];
                    self.preview_layer = preview_layer;
                }
            }
        }
    }

    fn update(&mut self, window: (u32, Id), parent_view: ObjcId, session: ObjcId, rect: Rect, visible: bool) {
        self.ensure_attached(window, parent_view, rect);
        self.ensure_layer(session);
        unsafe {
            if self.host_view != nil {
                let () = msg_send![self.host_view, setFrame: ns_rect(rect)];
                let () = msg_send![self.host_view, setHidden: if visible { NO } else { YES }];
                if self.preview_layer != nil {
                    let () = msg_send![self.preview_layer, setFrame: ns_rect(Rect { x: 0.0, y: 0.0, w: rect.w, h: rect.h })];
                }
            }
        }
    }

    fn detach(&mut self) {
        unsafe {
            if self.preview_layer != nil {
                let () = msg_send![self.preview_layer, removeFromSuperlayer];
                self.preview_layer = nil;
            }
            if self.host_view != nil {
                let () = msg_send![self.host_view, removeFromSuperview];
                self.host_view = nil;
            }
        }
        self.attached = None;
    }
}

#[derive(Default)]
struct Views {
    #[cfg(not(target_os = "tvos"))]
    browsers: HashMap<(u32, u64), Browser>,
    previews: HashMap<(u32, u64), Preview>,
}

thread_local! {
    /// The native views of every session; the main thread's.
    static VIEWS: RefCell<Views> = RefCell::new(Views::default());
}

/// One view command, executed on the main thread.
pub struct ViewCmd {
    pub sid: u32,
    pub window: Id,
    pub op: u16,
    pub view: u64,
    pub area: [f32; 4],
    pub extra: u32,
    /// the browser's url or visible rect (no browser on tvOS)
    #[cfg_attr(target_os = "tvos", allow(dead_code))]
    pub data: Vec<u8>,
    /// the capture session a camera preview shows (retained for the command; 0 = none)
    pub session: usize,
}

fn rect_of(v: [f32; 4]) -> Rect {
    Rect { x: v[0] as f64, y: v[1] as f64, w: v[2] as f64, h: v[3] as f64 }
}

#[cfg(not(target_os = "tvos"))]
/// The visible rect of an update, 4 f32 in the payload (the area itself when absent).
fn visible_rect(data: &[u8], area: [f32; 4]) -> Rect {
    let f = |i: usize| data.get(i * 4..i * 4 + 4).map(|b| f32::from_le_bytes(b.try_into().expect("4 bytes")));
    match (f(0), f(1), f(2), f(3)) {
        (Some(x), Some(y), Some(w), Some(h)) => Rect { x: x as f64, y: y as f64, w: w as f64, h: h as f64 },
        _ => rect_of(area),
    }
}

/// Executes the command on the main thread.
pub fn apply(cmd: ViewCmd) {
    let kind = cmd.op & 0xff00;
    let op = cmd.op & 0x00ff;
    let key = (cmd.sid, cmd.view);
    VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        match kind {
            #[cfg(not(target_os = "tvos"))]
            view_kind::BROWSER => match op {
                view_op::CREATE => {
                    let url = String::from_utf8_lossy(&cmd.data).into_owned();
                    let browser = views.browsers.entry(key).or_insert_with(|| Browser::new(&url));
                    browser.set_url(&url);
                }
                view_op::UPDATE => {
                    let Some(parent) = crate::backend::apple::shell::window_view(cmd.sid, cmd.window) else { return };
                    if let Some(browser) = views.browsers.get_mut(&key) {
                        let (unclipped, clipped) = (in_parent(parent, rect_of(cmd.area)), in_parent(parent, visible_rect(&cmd.data, cmd.area)));
                        browser.update((cmd.sid, cmd.window), parent, unclipped, clipped, cmd.extra != 0);
                    }
                }
                view_op::DETACH => {
                    if let Some(browser) = views.browsers.get_mut(&key) {
                        browser.detach();
                    }
                }
                view_op::SET_URL => {
                    if let Some(browser) = views.browsers.get_mut(&key) {
                        browser.set_url(&String::from_utf8_lossy(&cmd.data));
                    }
                }
                view_op::HISTORY => {
                    if let Some(browser) = views.browsers.get_mut(&key) {
                        history_go(browser.web_view, cmd.extra as i32);
                    }
                }
                view_op::CLOSE => {
                    if let Some(mut browser) = views.browsers.remove(&key) {
                        browser.cleanup();
                    }
                }
                _ => {}
            },
            view_kind::CAMERA => match op {
                view_op::UPDATE => {
                    if let Some(parent) = crate::backend::apple::shell::window_view(cmd.sid, cmd.window) {
                        let preview = views.previews.entry(key).or_insert(Preview { attached: None, host_view: nil, preview_layer: nil });
                        preview.update((cmd.sid, cmd.window), parent, cmd.session as ObjcId, in_parent(parent, rect_of(cmd.area)), cmd.extra != 0);
                    }
                }
                view_op::DETACH => {
                    if let Some(preview) = views.previews.get_mut(&key) {
                        preview.detach();
                    }
                }
                view_op::CLOSE => {
                    if let Some(mut preview) = views.previews.remove(&key) {
                        preview.detach();
                    }
                }
                _ => {}
            },
            _ => {}
        }
        // the session reference the command carried
        if cmd.session != 0 {
            unsafe {
                let _: () = msg_send![cmd.session as ObjcId, release];
            }
        }
    });
}

/// The session ended: its views are taken out of the windows and released (main thread).
pub fn close_session(sid: u32) {
    VIEWS.with(|views| {
        let mut views = views.borrow_mut();
        #[cfg(not(target_os = "tvos"))]
        let keys: Vec<(u32, u64)> = views.browsers.keys().filter(|k| k.0 == sid).copied().collect();
        #[cfg(not(target_os = "tvos"))]
        for k in keys {
            if let Some(mut b) = views.browsers.remove(&k) {
                b.cleanup();
            }
        }
        let keys: Vec<(u32, u64)> = views.previews.keys().filter(|k| k.0 == sid).copied().collect();
        for k in keys {
            if let Some(mut p) = views.previews.remove(&k) {
                p.detach();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{clipped_browser_layout, Rect};

    #[test]
    fn keeps_native_browser_anchored_when_top_is_clipped() {
        let unclipped_rect = Rect { x: 26.0, y: 262.0, w: 298.0, h: 420.0 };
        let clipped_rect = Rect { x: 26.0, y: 262.0, w: 298.0, h: 319.0 };

        let (host_rect, web_view_rect, is_visible) = clipped_browser_layout(unclipped_rect, clipped_rect, true);

        assert_eq!(host_rect, clipped_rect);
        assert_eq!((web_view_rect.x, web_view_rect.y), (0.0, 0.0));
        assert_eq!((web_view_rect.w, web_view_rect.h), (unclipped_rect.w, unclipped_rect.h));
        assert!(is_visible);
    }

    #[test]
    fn offsets_native_browser_inside_clip_when_bottom_is_clipped() {
        let unclipped_rect = Rect { x: 26.0, y: 120.0, w: 298.0, h: 420.0 };
        let clipped_rect = Rect { x: 26.0, y: 170.0, w: 298.0, h: 370.0 };

        let (_, web_view_rect, _) = clipped_browser_layout(unclipped_rect, clipped_rect, true);

        assert_eq!((web_view_rect.x, web_view_rect.y), (0.0, -50.0));
    }

    #[test]
    fn hides_when_clip_is_empty() {
        let unclipped_rect = Rect { x: 26.0, y: 262.0, w: 298.0, h: 420.0 };
        let clipped_rect = Rect { x: 26.0, y: 262.0, w: 0.0, h: 0.0 };

        let (_, _, is_visible) = clipped_browser_layout(unclipped_rect, clipped_rect, true);

        assert!(!is_visible);
    }
}
