//! Native layer hosts on the window system main thread. Content commands own the objects;
//! window frame packets place hosts between Metal segments in one presentation transaction.

use makepad_apple_sys::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::native_layer::{kind, layer_flags};
#[cfg(not(target_os = "tvos"))]
use crate::native_layer::op;
use crate::types::{Id, Layer};

#[cfg(not(target_os = "tvos"))]
#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

/// Read once; all geometry queries and formatting stay behind this branch.
pub fn trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MAKEPAD_NATIVE_LAYER_TRACE").is_ok_and(|v| v == "1"))
}
static UNPRESENTED: AtomicU64 = AtomicU64::new(0);
static COMMITS: AtomicU64 = AtomicU64::new(0);

pub fn trace_replaced(frame: &Frame) {
    if trace_enabled() && !frame.drawables.is_empty() {
        UNPRESENTED.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn trace_commit(trace: Option<(String, Instant)>, committed: Option<Instant>) {
    if let (Some((line, scheduled)), Some(committed)) = (trace, committed) {
        let lag = committed.duration_since(scheduled).as_micros();
        let commits = COMMITS.fetch_add(1, Ordering::Relaxed) + 1;
        let dropped = UNPRESENTED.load(Ordering::Relaxed);
        eprintln!("{line} main_lag_us={lag} unpresented_drops={dropped}");
        if commits % 120 == 0 {
            eprintln!("[native-layer] commits={commits} unpresented_drops={dropped}");
        }
    }
}

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
    if web_view == nil || delta == 0 { return; }
    unsafe {
        let history: ObjcId = msg_send![web_view, backForwardList];
        let item: ObjcId = msg_send![history, itemAtIndex: delta as i64];
        if item != nil { let () = msg_send![web_view, goToBackForwardListItem: item]; }
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

/// Frame geometry is top-left window pixels; native hosts use window points.
fn in_parent(parent: ObjcId, area: [f32; 4], dpi: f32) -> Rect {
    let scale = dpi.max(f32::EPSILON) as f64;
    let rect = Rect { x: area[0] as f64 / scale, y: area[1] as f64 / scale, w: area[2] as f64 / scale, h: area[3] as f64 / scale };
    #[cfg(target_os = "macos")]
    {
        let bounds: NSRect = unsafe { msg_send![parent, bounds] };
        Rect { y: bounds.size.height - rect.y - rect.h, ..rect }
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = parent; rect }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Transform([f64; 16]);
unsafe impl Encode for Transform {
    fn encode() -> Encoding { unsafe { Encoding::from_str("{CATransform3D=dddddddddddddddd}") } }
}
impl Transform {
    fn identity() -> Self {
        Self([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0])
    }
    fn mul(self, rhs: Self) -> Self {
        let mut out = [0.0; 16];
        for c in 0..4 { for r in 0..4 { for k in 0..4 { out[c * 4 + r] += self.0[k * 4 + r] * rhs.0[c * 4 + k]; } } }
        Self(out)
    }
}

/// Conjugate window-pixel geometry into the host layer's anchor-relative coordinates.
/// This handles affine transforms exactly, including AppKit's inverted Y axis, and passes
/// perspective through Core Animation as a best effort.
fn host_transform(g: &Layer, dpi: f32) -> Transform {
    let scale = dpi.max(f32::EPSILON) as f64;
    let mut basis = Transform::identity();
    basis.0[0] = scale;
    basis.0[5] = if cfg!(target_os = "macos") { -scale } else { scale };
    basis.0[10] = scale;
    basis.0[12] = (g.clip[0] + g.clip[2] * 0.5) as f64;
    basis.0[13] = (g.clip[1] + g.clip[3] * 0.5) as f64;
    let mut inverse = Transform::identity();
    for i in [0, 5, 10] { inverse.0[i] = 1.0 / basis.0[i]; }
    inverse.0[12] = -basis.0[12] * inverse.0[0];
    inverse.0[13] = -basis.0[13] * inverse.0[5];
    inverse.mul(Transform(g.transform.map(|x| x as f64))).mul(basis)
}

fn style(host: ObjcId, geometry: &Layer, dpi: f32) {
    if host == nil { return; }
    unsafe {
        let layer: ObjcId = msg_send![host, layer];
        if layer == nil { return; }
        let radius = geometry.radius.iter().copied().fold(0.0_f32, f32::max) / dpi.max(f32::EPSILON);
        let mut corners = 0_u64;
        // Radius order is top-left, top-right, bottom-right, bottom-left.
        #[cfg(target_os = "macos")]
        let bits = [4, 8, 2, 1];
        #[cfg(not(target_os = "macos"))]
        let bits = [1, 2, 8, 4];
        for (radius, bit) in geometry.radius.iter().zip(bits) { if *radius > 0.0 { corners |= bit; } }
        let () = msg_send![layer, setCornerRadius: radius as f64];
        let () = msg_send![layer, setMaskedCorners: corners];
        let () = msg_send![layer, setMasksToBounds: YES];
        let () = msg_send![layer, setOpacity: geometry.opacity.clamp(0.0, 1.0)];
        let () = msg_send![layer, setTransform: host_transform(geometry, dpi)];
    }
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
    sid: u32,
    id: Id,
    attached: Option<(u32, Id)>,
    host_view: ObjcId,
    web_view: ObjcId,
}

#[cfg(not(target_os = "tvos"))]
impl Browser {
    fn new(sid: u32, id: Id, url: &str) -> Browser {
        let mut browser = Browser { sid, id, current_url: String::new(), attached: None, host_view: nil, web_view: nil };
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
            let web_view: ObjcId = msg_send![super::web_state::class(), alloc];
            let web_view: ObjcId = msg_send![web_view, initWithFrame: NSRect { origin: NSPoint { x: 0.0, y: 0.0 }, size: NSSize { width: 1.0, height: 1.0 } } configuration: config];
            let () = msg_send![config, release];
            if web_view != nil {
                let () = msg_send![web_view, setHidden: YES];
                self.web_view = web_view;
                super::web_state::watch(web_view, self.sid, self.id);
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
            let layer: ObjcId = msg_send![self.host_view, layer];
            let () = msg_send![layer, setTransform: Transform::identity()];
            let (host_rect, web_view_rect, is_visible) = clipped_browser_layout(unclipped, clipped, visible);
            let () = msg_send![self.host_view, setFrame: ns_rect(host_rect)];
            let () = msg_send![self.web_view, setFrame: ns_rect(web_view_rect)];
            let () = msg_send![self.web_view, setHidden: if is_visible { NO } else { YES }];
            let () = msg_send![self.host_view, setHidden: if is_visible { NO } else { YES }];
            #[cfg(target_os = "macos")]
            if !is_visible && super::web_state::is_focused(self.web_view) {
                let window: ObjcId = msg_send![self.web_view, window];
                let () = msg_send![window, makeFirstResponder: parent_view];
            }
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

#[cfg(not(target_os = "tvos"))]
impl Drop for Browser {
    fn drop(&mut self) {
        self.cleanup();
        unsafe {
            if self.web_view != nil {
                super::web_state::unwatch(self.web_view);
                let () = msg_send![self.web_view, release];
            }
            if self.host_view != nil { let () = msg_send![self.host_view, release]; }
        }
    }
}

/// A native preview of an open capture session (an `AVCaptureVideoPreviewLayer` in a view over
/// the window). Moved from `MacosNativeCameraPreview` (os/apple/macos/macos.rs).
struct Preview {
    attached: Option<(u32, Id)>,
    host_view: ObjcId,
    preview_layer: ObjcId,
    session: ObjcId,
}

impl Preview {
    fn ensure_attached(&mut self, window: (u32, Id), parent_view: ObjcId, rect: Rect) {
        unsafe {
            if self.attached != Some(window) || self.host_view == nil {
                self.detach();
                let host_view = new_host_view(rect, true);
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
            if session == nil || self.host_view == nil { return; }
            if self.preview_layer != nil {
                if self.session != session {
                    let () = msg_send![self.preview_layer, setSession: session];
                    self.session = session;
                }
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
                    self.session = session;
                }
            }
        }
    }

    fn update(&mut self, window: (u32, Id), parent_view: ObjcId, session: ObjcId, rect: Rect, clipped: Rect, visible: bool) {
        self.ensure_attached(window, parent_view, clipped);
        self.ensure_layer(session);
        unsafe {
            if self.host_view != nil {
                let layer: ObjcId = msg_send![self.host_view, layer];
                let () = msg_send![layer, setTransform: Transform::identity()];
                let () = msg_send![self.host_view, setFrame: ns_rect(clipped)];
                let () = msg_send![self.host_view, setHidden: if visible { NO } else { YES }];
                if self.preview_layer != nil {
                    let () = msg_send![self.preview_layer, setFrame: ns_rect(Rect { x: rect.x - clipped.x, y: rect.y - clipped.y, w: rect.w, h: rect.h })];
                }
            }
        }
    }

    fn detach(&mut self) {
        unsafe {
            if self.preview_layer != nil {
                let () = msg_send![self.preview_layer, removeFromSuperlayer];
                self.preview_layer = nil;
                self.session = nil;
            }
            if self.host_view != nil {
                let () = msg_send![self.host_view, removeFromSuperview];
                let () = msg_send![self.host_view, release];
                self.host_view = nil;
            }
        }
        self.attached = None;
    }
}

impl Drop for Preview {
    fn drop(&mut self) { self.detach(); }
}

#[derive(Default)]
struct Layers {
    #[cfg(not(target_os = "tvos"))]
    browsers: HashMap<(u32, Id), Browser>,
    previews: HashMap<(u32, Id), Preview>,
    windows: HashMap<(u32, Id), WindowLayers>,
}

thread_local! {
    static LAYERS: RefCell<Layers> = RefCell::new(Layers::default());
}

pub fn create(sid: u32, id: Id, kind: u32, params: Vec<u8>) -> bool {
    LAYERS.with(|layers| {
        let mut layers = layers.borrow_mut();
        match kind {
            #[cfg(not(target_os = "tvos"))]
            kind::WEB => {
                let browser = Browser::new(sid, id, &String::from_utf8_lossy(&params));
                if browser.web_view == nil { return false; }
                layers.browsers.insert((sid, id), browser);
                true
            }
            kind::CAMERA_PREVIEW => {
                layers.previews.insert((sid, id), Preview { attached: None, host_view: nil, preview_layer: nil, session: nil });
                true
            }
            _ => { let _ = params; false }
        }
    })
}

pub fn operate(sid: u32, id: Id, operation: u32, arg: u64, data: Vec<u8>) {
    #[cfg(target_os = "tvos")]
    { let _ = (sid, id, operation, arg, data); }
    #[cfg(not(target_os = "tvos"))]
    LAYERS.with(|layers| {
        let mut layers = layers.borrow_mut();
        let Some(browser) = layers.browsers.get_mut(&(sid, id)) else { return; };
        unsafe {
            match operation {
                op::NAVIGATE => {
                    let url = String::from_utf8_lossy(&data);
                    if arg == 0 { browser.set_url(&url); }
                    else {
                        // Escaping UTF-16 units keeps arbitrary URLs inside the JS string.
                        let literal: String = url.encode_utf16().map(|c| format!("\\u{c:04x}")).collect();
                        let script = str_to_nsstring(&format!("window.location.replace(\"{literal}\")"));
                        let () = msg_send![browser.web_view, evaluateJavaScript: script completionHandler: nil];
                        browser.current_url = url.into_owned();
                    }
                }
                op::HISTORY => history_go(browser.web_view, arg as i32),
                op::RELOAD => { let () = msg_send![browser.web_view, reload]; }
                op::STOP => { let () = msg_send![browser.web_view, stopLoading]; }
                op::FOCUS => {
                    #[cfg(target_os = "macos")]
                    { let window: ObjcId = msg_send![browser.web_view, window]; let () = msg_send![window, makeFirstResponder: browser.web_view]; }
                    #[cfg(target_os = "ios")]
                    { let () = msg_send![browser.web_view, becomeFirstResponder]; }
                }
                op::BLUR => {
                    #[cfg(target_os = "macos")]
                    if let Some((sid, window)) = browser.attached.filter(|_| super::web_state::is_focused(browser.web_view)) {
                        if let Some(view) = crate::backend::apple::shell::window_view(sid, window) {
                            let window: ObjcId = msg_send![browser.web_view, window];
                            let () = msg_send![window, makeFirstResponder: view];
                        }
                    }
                    #[cfg(target_os = "ios")]
                    {
                        let () = msg_send![browser.web_view, resignFirstResponder];
                        if let Some((sid, window)) = browser.attached {
                            if let Some(view) = crate::backend::apple::shell::window_view(sid, window) {
                                let () = msg_send![view, becomeFirstResponder];
                            }
                        }
                    }
                }
                op::APPEARANCE => {
                    #[cfg(target_os = "macos")]
                    {
                        let name = str_to_nsstring(if arg & 1 != 0 { "NSAppearanceNameDarkAqua" } else { "NSAppearanceNameAqua" });
                        let appearance: ObjcId = msg_send![class!(NSAppearance), appearanceNamed: name];
                        let () = msg_send![browser.web_view, setAppearance: appearance];
                    }
                    #[cfg(target_os = "ios")]
                    { let () = msg_send![browser.web_view, setOverrideUserInterfaceStyle: if arg & 1 != 0 { 2_i64 } else { 1_i64 }]; }
                    if data.len() == 16 {
                        let c: Vec<f64> = data.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap()) as f64).collect();
                        #[cfg(target_os = "macos")]
                        let color: ObjcId = msg_send![class!(NSColor), colorWithSRGBRed: c[0] green: c[1] blue: c[2] alpha: c[3]];
                        #[cfg(target_os = "ios")]
                        let color: ObjcId = msg_send![class!(UIColor), colorWithRed: c[0] green: c[1] blue: c[2] alpha: c[3]];
                        let supported: BOOL = msg_send![browser.web_view, respondsToSelector: sel!(setUnderPageBackgroundColor:)];
                        if supported == YES { let () = msg_send![browser.web_view, setUnderPageBackgroundColor: color]; }
                    }
                }
                _ => {}
            }
        }
    });
}

/// Keeps a capture session alive until its geometry reaches the main thread.
pub struct Placement { pub geometry: Layer, pub session: usize, pub occluders: Vec<[f32; 4]> }
impl Drop for Placement {
    fn drop(&mut self) { if self.session != 0 { unsafe { let () = msg_send![self.session as ObjcId, release]; } } }
}

pub struct Surface { pub gap: usize, pub layer: RcObjcId, pub rect: [f32; 4] }
pub enum Item { Native(Id), Segment(usize) }
pub struct Frame {
    pub number: u64,
    pub scheduled: Option<Instant>,
    pub sid: u32,
    pub window: Id,
    pub parent: (u32, Id),
    pub dpi: f32,
    pub placements: Vec<Placement>,
    pub surfaces: Vec<Surface>,
    pub order: Vec<Item>,
    pub drawables: Vec<RcObjcId>,
    pub active: bool,
    pub preserve_hosts: bool,
    pub alive: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub active_state: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
impl Frame {
    pub fn new(sid: u32, window: Id, dpi: f32) -> Self {
        Self { number: 0, scheduled: None, sid, window, parent: (sid, window), dpi, placements: Vec::new(), surfaces: Vec::new(), order: Vec::new(), drawables: Vec::new(), active: false, preserve_hosts: false, alive: None, active_state: None }
    }
}
struct SegmentView {
    view: RcObjcId,
    layer: RcObjcId,
    hidden: Option<std::time::Instant>,
}
struct InputRegion { geometry: Layer, occluders: Vec<[f32; 4]>, view: ObjcId }
#[derive(Default)]
struct WindowLayers {
    parent: ObjcId,
    parent_key: (u32, Id),
    active: bool,
    dpi: f32,
    segments: HashMap<usize, SegmentView>,
    inputs: Vec<InputRegion>,
    native_ids: Vec<Id>,
}

/// A segment view never participates in native hit testing.
fn segment_class() -> &'static Class {
    static CLASS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let ptr = *CLASS.get_or_init(|| {
        #[cfg(target_os = "macos")]
        let base = class!(NSView);
        #[cfg(not(target_os = "macos"))]
        let base = class!(UIView);
        let mut decl = ClassDecl::new("HalNativeSegment", base).unwrap();
        #[cfg(target_os = "macos")]
        extern "C" fn hit(_: &Object, _: Sel, _: NSPoint) -> ObjcId { nil }
        #[cfg(not(target_os = "macos"))]
        extern "C" fn hit(_: &Object, _: Sel, _: NSPoint, _: ObjcId) -> ObjcId { nil }
        extern "C" fn display(_: &Object, _: Sel, _: ObjcId) {}
        extern "C" fn expire(_: &Object, _: Sel) {
            LAYERS.with(|layers| {
                let mut layers = layers.borrow_mut();
                for window in layers.windows.values_mut() {
                    window.segments.retain(|_, segment| {
                        if segment.hidden.is_some_and(|t| t.elapsed().as_secs_f64() >= 3.0) {
                            unsafe { let () = msg_send![segment.view.as_id(), removeFromSuperview]; }
                            false
                        } else { true }
                    });
                }
                layers.windows.retain(|_, tree| tree.active || !tree.segments.is_empty());
            });
        }
        unsafe {
            #[cfg(target_os = "macos")]
            decl.add_method(sel!(hitTest:), hit as extern "C" fn(&Object, Sel, NSPoint) -> ObjcId);
            #[cfg(not(target_os = "macos"))]
            decl.add_method(sel!(hitTest:withEvent:), hit as extern "C" fn(&Object, Sel, NSPoint, ObjcId) -> ObjcId);
            decl.add_method(sel!(displayLayer:), display as extern "C" fn(&Object, Sel, ObjcId));
            decl.add_method(sel!(expireSegments), expire as extern "C" fn(&Object, Sel));
        }
        decl.register() as *const Class as usize
    });
    unsafe { &*(ptr as *const Class) }
}

fn place_above(parent: ObjcId, view: ObjcId, previous: ObjcId) {
    unsafe {
        let current: ObjcId = msg_send![view, superview];
        if current == parent {
            if previous == nil { return; }
            let children: ObjcId = msg_send![parent, subviews];
            let index: usize = msg_send![children, indexOfObjectIdenticalTo: view];
            let before: usize = msg_send![children, indexOfObjectIdenticalTo: previous];
            if index > before { return; }
        }
        #[cfg(target_os = "macos")]
        { let () = msg_send![parent, addSubview: view positioned: 1i64 relativeTo: previous]; }
        #[cfg(not(target_os = "macos"))]
        {
            if previous == nil { let () = msg_send![parent, insertSubview: view atIndex: 0i64]; }
            else { let () = msg_send![parent, insertSubview: view aboveSubview: previous]; }
        }
    }
}

/// The caller owns the CATransaction. Only this function publishes input geometry, so hit
/// testing cannot get ahead of the pixels. No main-thread call waits for Metal here.
pub struct Presented {
    pub trace: Option<(String, Instant)>,
    pub drawables: Vec<RcObjcId>,
    pub active_state: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub active: bool,
}
pub fn frame(frame: Frame) -> Option<Presented> {
    let Frame { number, scheduled, sid, window, parent: parent_key, dpi, placements, surfaces, order, drawables, active, preserve_hosts, alive, active_state } = frame;
    if alive.is_some_and(|a| !a.load(std::sync::atomic::Ordering::Acquire)) { return None; }
    let parent = crate::backend::apple::shell::window_view(parent_key.0, parent_key.1)?;
    let trace = LAYERS.with(|layers| {
        let mut layers = layers.borrow_mut();
        for (&(owner, other), tree) in &mut layers.windows {
            if owner == sid && other != window {
                tree.inputs.retain(|input| !placements.iter().any(|p| p.geometry.native == input.geometry.native));
            }
        }
        let mut tree = layers.windows.remove(&(sid, window)).unwrap_or_default();
        tree.parent = parent;
        tree.parent_key = parent_key;
        tree.active = active;
        tree.dpi = dpi;
        if !preserve_hosts { tree.inputs.clear(); }
        for segment in tree.segments.values_mut() {
            if !surfaces.iter().any(|s| s.layer.as_id() == segment.layer.as_id()) && segment.hidden.is_none() {
                segment.hidden = Some(std::time::Instant::now());
                unsafe {
                    let view = segment.view.as_id();
                    let () = msg_send![view, setHidden: YES];
                    let () = msg_send![view, performSelector: sel!(expireSegments) withObject: nil afterDelay: 3.0f64];
                }
            }
        }
        for surface in &surfaces {
            let replace = tree.segments.get(&surface.gap).is_none_or(|v| v.layer.as_id() != surface.layer.as_id());
            if replace {
                if let Some(old) = tree.segments.remove(&surface.gap) {
                    unsafe { let () = msg_send![old.view.as_id(), removeFromSuperview]; }
                }
                unsafe {
                    let view: ObjcId = msg_send![segment_class(), alloc];
                    let view: ObjcId = msg_send![view, initWithFrame: ns_rect(in_parent(parent, surface.rect, dpi))];
                    #[cfg(target_os = "macos")]
                    {
                        let () = msg_send![view, setWantsLayer: YES];
                        let () = msg_send![view, setLayer: surface.layer.as_id()];
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        let () = msg_send![view, setOpaque: NO];
                        let () = msg_send![view, setUserInteractionEnabled: NO];
                        let layer: ObjcId = msg_send![view, layer];
                        let () = msg_send![layer, addSublayer: surface.layer.as_id()];
                    }
                    tree.segments.insert(surface.gap, SegmentView { view: RcObjcId::from_owned(NonNull::new(view).expect("segment view")), layer: surface.layer.clone(), hidden: None });
                }
            }
            let segment = tree.segments.get_mut(&surface.gap).unwrap();
            let was_hidden = segment.hidden.take().is_some();
            unsafe {
                let view = segment.view.as_id();
                if was_hidden {
                    let () = msg_send![class!(NSObject), cancelPreviousPerformRequestsWithTarget: view selector: sel!(expireSegments) object: nil];
                }
                let () = msg_send![view, setFrame: ns_rect(in_parent(parent, surface.rect, dpi))];
                let () = msg_send![view, setHidden: NO];
                // AppKit positions a view's backing layer itself, in its parent's coordinates.
                // UIKit's segment Metal layer is a sublayer and uses the view's local bounds.
                #[cfg(not(target_os = "macos"))]
                {
                    let bounds: NSRect = msg_send![view, bounds];
                    let () = msg_send![segment.layer.as_id(), setFrame: bounds];
                }
            }
        }
        let mut hosts = HashMap::new();
        let native_ids: Vec<_> = placements.iter().map(|p| p.geometry.native).collect();
        for placement in &placements {
            let g = &placement.geometry;
            let rect = in_parent(parent, g.rect, dpi);
            let clip = in_parent(parent, g.clip, dpi);
            let visible = g.flags & layer_flags::VISIBLE != 0 && clip.w > 0.0 && clip.h > 0.0;
            #[cfg(not(target_os = "tvos"))]
            if let Some(browser) = layers.browsers.get_mut(&(sid, g.native)) {
                browser.update((sid, window), parent, rect, clip, visible);
                style(browser.host_view, g, dpi);
                hosts.insert(g.native, browser.host_view);
                if visible { tree.inputs.push(InputRegion { geometry: *g, occluders: placement.occluders.clone(), view: browser.web_view }); }
            }
            if let Some(preview) = layers.previews.get_mut(&(sid, g.native)) {
                preview.update((sid, window), parent, placement.session as ObjcId, rect, clip, visible);
                style(preview.host_view, g, dpi);
                hosts.insert(g.native, preview.host_view);
            }
        }
        if !preserve_hosts {
            // Only this window's at-most-four previous placements need hiding. Retained web
            // objects in other tabs/sessions do no work on this window's per-frame path.
            for id in tree.native_ids.iter().filter(|id| !native_ids.contains(id)) {
                #[cfg(not(target_os = "tvos"))]
                if let Some(browser) = layers.browsers.get(&(sid, *id)) {
                    unsafe {
                        let () = msg_send![browser.host_view, setHidden: YES];
                        #[cfg(target_os = "macos")]
                        if super::web_state::is_focused(browser.web_view) {
                            let window: ObjcId = msg_send![browser.web_view, window];
                            let () = msg_send![window, makeFirstResponder: parent];
                        }
                    }
                }
                if let Some(preview) = layers.previews.get(&(sid, *id)) {
                    unsafe { let () = msg_send![preview.host_view, setHidden: YES]; }
                }
            }
            tree.native_ids = native_ids;
        }
        let mut previous = nil;
        for item in order {
            let view = match item {
                Item::Native(id) => hosts.get(&id).copied(),
                Item::Segment(gap) => tree.segments.get(&gap).filter(|s| s.hidden.is_none()).map(|s| s.view.as_id()),
            };
            if let Some(view) = view { place_above(parent, view, previous); previous = view; }
        }
        let trace = scheduled.map(|scheduled| {
            let mut line = format!("[native-layer] win={sid}:{} frame={number} seg={} dpi={dpi} layers=[", window.index, drawables.len());
            for p in &placements {
                let g = &p.geometry;
                let host = hosts.get(&g.native).copied().unwrap_or(nil);
                #[cfg(not(target_os = "tvos"))]
                let web = layers.browsers.get(&(sid, g.native)).map_or(nil, |b| b.web_view);
                #[cfg(target_os = "tvos")]
                let web = nil;
                let (host_rect, web_rect, visible, z) = unsafe {
                    let hidden: BOOL = if host == nil { YES } else { msg_send![host, isHidden] };
                    (trace_rect(host, parent), trace_rect(web, host), hidden == NO, trace_z(parent, host))
                };
                let host_layer = if host == nil { nil } else { unsafe { msg_send![host, layer] } };
                let layer_z = trace_layer_z(parent, host_layer);
                let _ = write!(line, "{}: rect={:?} clip={:?} host={host_rect:?} web={web_rect:?} vis={} z={z} layer_z={layer_z} occluders={:?}; ",
                    g.native.index, g.rect, g.clip, visible as u8, p.occluders);
            }
            line.push_str("] segs=[");
            for (index, drawable) in drawables.iter().enumerate() {
                let (rect, z, gap) = if index == 0 {
                    let bounds: NSRect = unsafe { msg_send![parent, bounds] };
                    ([0.0, 0.0, bounds.size.width as f32 * dpi, bounds.size.height as f32 * dpi], -1, 0)
                } else {
                    let surface = &surfaces[index - 1];
                    let view = tree.segments.get(&surface.gap).map_or(nil, |s| s.view.as_id());
                    (surface.rect, trace_z(parent, view), surface.gap)
                };
                let (w, h): (u64, u64) = unsafe {
                    let texture: ObjcId = msg_send![drawable.as_id(), texture];
                    (msg_send![texture, width], msg_send![texture, height])
                };
                let _ = write!(line, "{gap}: rect={rect:?} drawable={w}x{h} present_requested=1 z={z}; ");
                if index != 0 {
                    let segment = &tree.segments[&gap];
                    unsafe {
                        let view = segment.view.as_id();
                        let layer = segment.layer.as_id();
                        let actual: NSRect = msg_send![layer, frame];
                        let bounds: NSRect = msg_send![layer, bounds];
                        let backing: ObjcId = msg_send![view, layer];
                        let superlayer: ObjcId = msg_send![layer, superlayer];
                        let parent_layer: ObjcId = msg_send![parent, layer];
                        let win: ObjcId = msg_send![view, window];
                        let parent_win: ObjcId = msg_send![parent, window];
                        let hidden: BOOL = msg_send![layer, isHidden];
                        let drawable_size: CGSize = msg_send![layer, drawableSize];
                        let scale: f64 = msg_send![layer, contentsScale];
                        eprintln!("[native-layer] frame={number} surface={gap} view={:?} layer_frame={:?} layer_bounds={:?} backing={} attached={} window={} hidden={} layer_z={} scale={scale} drawable_size={:?}",
                            trace_rect(view, parent),
                            [actual.origin.x, actual.origin.y, actual.size.width, actual.size.height],
                            [bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height],
                            (backing == layer) as u8, (superlayer == parent_layer) as u8,
                            (win != nil && win == parent_win) as u8, (hidden == YES) as u8,
                            trace_layer_z(parent, backing), [drawable_size.width, drawable_size.height]);
                    }
                }
            }
            line.push(']');
            (line, scheduled)
        });
        if active || !tree.segments.is_empty() { layers.windows.insert((sid, window), tree); }
        trace
    });
    for (index, drawable) in drawables.iter().enumerate() {
        unsafe {
            if trace_enabled() {
                let gap = if index == 0 { 0 } else { surfaces[index - 1].gap };
                let () = msg_send![drawable.as_id(), addPresentedHandler: &objc_block!(move |d: ObjcId| {
                    let time: f64 = unsafe { msg_send![d, presentedTime] };
                    eprintln!("[native-layer] shown win={sid}:{} frame={number} surface={gap} time={time}", window.index);
                })];
            }
            let () = msg_send![drawable.as_id(), present];
        }
    }
    if !active {
        unsafe {
            let layer: ObjcId = msg_send![parent, layer];
            let () = msg_send![layer, setPresentsWithTransaction: NO];
        }
    }
    Some(Presented { trace, drawables, active_state, active })
}

// Read actual view frames, normalised to top-left points in their superview.
fn trace_rect(view: ObjcId, parent: ObjcId) -> [f64; 4] {
    if view == nil || parent == nil { return [0.0; 4]; }
    unsafe {
        let r: NSRect = msg_send![view, frame];
        #[cfg(target_os = "macos")]
        let y = {
            let flipped: BOOL = msg_send![parent, isFlipped];
            let bounds: NSRect = msg_send![parent, bounds];
            if flipped == YES { r.origin.y } else { bounds.size.height - r.origin.y - r.size.height }
        };
        #[cfg(not(target_os = "macos"))]
        let y = r.origin.y;
        [r.origin.x, y, r.size.width, r.size.height]
    }
}
fn trace_z(parent: ObjcId, view: ObjcId) -> i64 {
    if view == nil { return -1; }
    unsafe {
        let children: ObjcId = msg_send![parent, subviews];
        let index: usize = msg_send![children, indexOfObjectIdenticalTo: view];
        if index == usize::MAX || index == isize::MAX as usize { -1 } else { index as i64 }
    }
}
fn trace_layer_z(parent: ObjcId, layer: ObjcId) -> i64 {
    if layer == nil { return -1; }
    unsafe {
        let root: ObjcId = msg_send![parent, layer];
        let children: ObjcId = msg_send![root, sublayers];
        if children == nil { return -1; }
        let index: usize = msg_send![children, indexOfObjectIdenticalTo: layer];
        if index == usize::MAX || index == isize::MAX as usize { -1 } else { index as i64 }
    }
}

/// `point` is in the root view's own coordinates. Occluders are already in window pixels;
/// the native clip is before its transform, so invert the affine part before testing it.
pub fn hit_test(parent: ObjcId, point: NSPoint, event: ObjcId) -> Option<ObjcId> {
    LAYERS.with(|layers| {
        let layers = layers.borrow();
        let tree = layers.windows.values().find(|w| w.parent == parent)?;
        let bounds: NSRect = unsafe { msg_send![parent, bounds] };
        let hidden: BOOL = unsafe { msg_send![parent, isHidden] };
        if hidden != NO || point.x < 0.0 || point.y < 0.0 || point.x >= bounds.size.width || point.y >= bounds.size.height { return Some(nil); }
        let px = point.x as f32 * tree.dpi;
        #[cfg(target_os = "macos")]
        let py = unsafe { let b: NSRect = msg_send![parent, bounds]; (b.size.height - point.y) as f32 * tree.dpi };
        #[cfg(not(target_os = "macos"))]
        let py = point.y as f32 * tree.dpi;
        let trace_down = if trace_enabled() {
            #[cfg(target_os = "macos")]
            { unsafe {
                let event = if event == nil {
                    let app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
                    msg_send![app, currentEvent]
                } else { event };
                event != nil && { let kind: u64 = msg_send![event, type]; matches!(kind, 1 | 3 | 25) }
            } }
            #[cfg(not(target_os = "macos"))]
            { false }
        } else { false };
        let trace_hit = |id: Option<Id>| {
            if trace_down {
                let target = id.map_or_else(|| "makepad".to_owned(), |id| id.index.to_string());
                let rects: usize = tree.inputs.iter().map(|r| 1 + r.occluders.len()).sum();
                eprintln!("[native-layer] hit x={} y={} -> {target} (region rects {rects})", px / tree.dpi, py / tree.dpi);
            }
        };
        let inside = |r: [f32; 4], x: f32, y: f32| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3];
        for region in tree.inputs.iter().rev() {
            let g = &region.geometry;
            if g.flags & layer_flags::INPUT_PASSTHROUGH != 0 || region.occluders.iter().any(|r| inside(*r, px, py)) { continue; }
            let t = g.transform;
            let det = t[0] * t[5] - t[1] * t[4];
            if det.abs() < f32::EPSILON { continue; }
            let x = ((px - t[12]) * t[5] - (py - t[13]) * t[4]) / det;
            let y = ((py - t[13]) * t[0] - (px - t[12]) * t[1]) / det;
            if !inside(g.clip, x, y) { continue; }
            unsafe {
                #[cfg(target_os = "macos")]
                {
                    let _ = event;
                    // NSView hitTest takes a point in its superview's coordinates.
                    let host: ObjcId = msg_send![region.view, superview];
                    let b: NSRect = msg_send![host, bounds];
                    let p = NSPoint { x: ((x - g.clip[0]) / tree.dpi) as f64, y: b.size.height - ((y - g.clip[1]) / tree.dpi) as f64 };
                    let hit: ObjcId = msg_send![region.view, hitTest: p];
                    if hit != nil { trace_hit(Some(g.native)); return Some(hit); }
                }
                #[cfg(not(target_os = "macos"))]
                {
                    let p = NSPoint { x: ((x - g.rect[0]) / tree.dpi) as f64, y: ((y - g.rect[1]) / tree.dpi) as f64 };
                    let hit: ObjcId = msg_send![region.view, hitTest: p withEvent: event];
                    if hit != nil { trace_hit(Some(g.native)); return Some(hit); }
                }
            }
        }
        // The tree owns this view: do not let default subview traversal bypass the input mask.
        trace_hit(None);
        Some(parent)
    })
}

/// Remote input uses the same committed hit region as AppKit, on the main thread.
/// Return a Makepad hit to the caller's UI queue, where it retains remote origin and guards.
#[cfg(target_os = "macos")]
pub fn native_click(sid: u32, window: Id, x: f64, y: f64, valid: impl FnOnce() -> bool + Send + 'static,
    reply: impl FnOnce(Result<bool, &'static str>) + Send + 'static) {
    crate::backend::apple::shell::on_main(move || {
        if !valid() { reply(Err("native click expired or user intervened")); return; }
        let Some(parent) = crate::backend::apple::shell::window_view(sid, window) else {
            reply(Err("native window is gone")); return;
        };
        unsafe {
            let bounds: NSRect = msg_send![parent, bounds];
            if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 || x >= bounds.size.width || y >= bounds.size.height {
                reply(Err("native click outside window")); return;
            }
            let point = NSPoint { x, y: bounds.size.height - y };
            let location: NSPoint = msg_send![parent, convertPoint: point toView: nil];
            let win: ObjcId = msg_send![parent, window];
            let number: i64 = msg_send![win, windowNumber];
            let process: ObjcId = msg_send![class!(NSProcessInfo), processInfo];
            let timestamp: f64 = msg_send![process, systemUptime];
            let down: ObjcId = msg_send![class!(NSEvent), mouseEventWithType: 1u64 location: location modifierFlags: 0u64
                timestamp: timestamp windowNumber: number context: nil eventNumber: 0i64 clickCount: 1i64 pressure: 1.0f32];
            let hit = hit_test(parent, point, down).unwrap_or(parent);
            if hit == nil || hit == parent { reply(Ok(false)); return; }
            // Keep AppKit's press receiver for the release, even if the press changes layout.
            let retained = RcObjcId::from_unowned(NonNull::new(hit).unwrap());
            let () = msg_send![hit, mouseDown: down];
            let up: ObjcId = msg_send![class!(NSEvent), mouseEventWithType: 2u64 location: location modifierFlags: 0u64
                timestamp: timestamp windowNumber: number context: nil eventNumber: 1i64 clickCount: 1i64 pressure: 0.0f32];
            let () = msg_send![retained.as_id(), mouseUp: up];
            reply(Ok(true));
        }
    });
}

pub fn destroy(sid: u32, id: Id) {
    LAYERS.with(|layers| {
        let mut layers = layers.borrow_mut();
        for (&(owner, _), tree) in &mut layers.windows {
            if owner == sid { tree.inputs.retain(|input| input.geometry.native != id); }
        }
        #[cfg(not(target_os = "tvos"))]
        if let Some(mut browser) = layers.browsers.remove(&(sid, id)) { browser.cleanup(); }
        if let Some(mut preview) = layers.previews.remove(&(sid, id)) { preview.detach(); }
    });
}

pub fn close_window(sid: u32, window: Id) {
    LAYERS.with(|layers| {
        let mut layers = layers.borrow_mut();
        let keys: Vec<_> = layers.windows.iter().filter(|(key, tree)| **key == (sid, window) || tree.parent_key == (sid, window)).map(|(key, _)| *key).collect();
        for key in &keys {
            if let Some(tree) = layers.windows.remove(key) {
                if let Some(parent) = crate::backend::apple::shell::window_view(tree.parent_key.0, tree.parent_key.1) {
                    unsafe { let layer: ObjcId = msg_send![parent, layer]; let () = msg_send![layer, setPresentsWithTransaction: NO]; }
                }
                for segment in tree.segments.values() { unsafe { let () = msg_send![segment.view.as_id(), removeFromSuperview]; } }
            }
        }
        #[cfg(not(target_os = "tvos"))]
        for browser in layers.browsers.values_mut().filter(|b| b.attached.is_some_and(|key| key == (sid, window) || keys.contains(&key))) { browser.detach(); }
        for preview in layers.previews.values_mut().filter(|p| p.attached.is_some_and(|key| key == (sid, window) || keys.contains(&key))) { preview.detach(); }
    });
}

pub fn close_session(sid: u32) {
    LAYERS.with(|layers| {
        let mut layers = layers.borrow_mut();
        #[cfg(not(target_os = "tvos"))]
        layers.browsers.retain(|&(owner, _), browser| { if owner == sid { browser.cleanup(); } owner != sid });
        layers.previews.retain(|&(owner, _), preview| { if owner == sid { preview.detach(); } owner != sid });
        layers.windows.retain(|&(owner, _), tree| {
            if owner == sid {
                if let Some(parent) = crate::backend::apple::shell::window_view(tree.parent_key.0, tree.parent_key.1) {
                    unsafe { let layer: ObjcId = msg_send![parent, layer]; let () = msg_send![layer, setPresentsWithTransaction: NO]; }
                }
                for segment in tree.segments.values() { unsafe { let () = msg_send![segment.view.as_id(), removeFromSuperview]; } }
            }
            owner != sid
        });
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
