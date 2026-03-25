#[cfg(target_os = "macos")]
use crate::window::WindowId;
use crate::{
    makepad_math::Rect,
    os::apple::{apple_sys::*, apple_util::str_to_nsstring},
};
use makepad_objc_sys::{class, msg_send};

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {}

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

#[cfg(target_os = "macos")]
pub(crate) struct MacosSystemBrowser {
    current_url: String,
    attached_window: Option<WindowId>,
    host_view: ObjcId,
    web_view: ObjcId,
}

#[cfg(target_os = "macos")]
impl MacosSystemBrowser {
    pub(crate) fn new(url: &str) -> Self {
        let mut browser = Self {
            current_url: String::new(),
            attached_window: None,
            host_view: nil,
            web_view: nil,
        };
        browser.ensure_web_view();
        browser.set_url(url, false);
        browser
    }

    fn ensure_web_view(&mut self) {
        if self.web_view != nil {
            return;
        }
        unsafe {
            let config: ObjcId = msg_send![class!(WKWebViewConfiguration), new];
            let web_view: ObjcId = msg_send![class!(WKWebView), alloc];
            let web_view: ObjcId = msg_send![web_view, initWithFrame: NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize { width: 1.0, height: 1.0 }
            } configuration: config];
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
        unsafe {
            let host_view: ObjcId = msg_send![class!(NSView), alloc];
            let host_view: ObjcId = msg_send![host_view, initWithFrame: NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize { width: 1.0, height: 1.0 }
            }];
            if host_view != nil {
                let () = msg_send![host_view, setWantsLayer: YES];
                let layer: ObjcId = msg_send![host_view, layer];
                if layer != nil {
                    let () = msg_send![layer, setMasksToBounds: YES];
                }
                let () = msg_send![host_view, setHidden: YES];
                self.host_view = host_view;
            }
        }
    }

    fn ensure_attached(&mut self, window_id: WindowId, parent_view: ObjcId) {
        self.ensure_web_view();
        self.ensure_host_view();
        if self.web_view == nil || self.host_view == nil {
            return;
        }
        unsafe {
            let host_super_view: ObjcId = msg_send![self.host_view, superview];
            if self.attached_window != Some(window_id) || host_super_view != parent_view {
                let () = msg_send![self.host_view, removeFromSuperview];
                let () = msg_send![parent_view, addSubview: self.host_view];
                self.attached_window = Some(window_id);
            }

            let web_super_view: ObjcId = msg_send![self.web_view, superview];
            if web_super_view != self.host_view {
                let () = msg_send![self.web_view, removeFromSuperview];
                let () = msg_send![self.host_view, addSubview: self.web_view];
            }
        }
    }

    pub(crate) fn update(
        &mut self,
        window_id: WindowId,
        parent_view: ObjcId,
        unclipped_rect: Rect,
        clipped_rect: Rect,
        visible: bool,
    ) {
        self.ensure_attached(window_id, parent_view);
        unsafe {
            if self.host_view == nil || self.web_view == nil {
                return;
            }
            let (host_rect, web_view_rect, is_visible) =
                clipped_browser_layout(unclipped_rect, clipped_rect, visible);
            let host_frame = NSRect {
                origin: NSPoint {
                    x: host_rect.pos.x,
                    y: host_rect.pos.y,
                },
                size: NSSize {
                    width: host_rect.size.x.max(0.0),
                    height: host_rect.size.y.max(0.0),
                },
            };
            let web_view_frame = NSRect {
                origin: NSPoint {
                    x: web_view_rect.pos.x,
                    y: web_view_rect.pos.y,
                },
                size: NSSize {
                    width: web_view_rect.size.x.max(0.0),
                    height: web_view_rect.size.y.max(0.0),
                },
            };
            let () = msg_send![self.host_view, setFrame: host_frame];
            let () = msg_send![self.web_view, setFrame: web_view_frame];
            let () = msg_send![self.web_view, setHidden: if is_visible { NO } else { YES }];
            let () = msg_send![self.host_view, setHidden: if is_visible { NO } else { YES }];
        }
    }

    pub(crate) fn detach(&mut self) {
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
        self.attached_window = None;
    }

    pub(crate) fn set_url(&mut self, url: &str, _replace: bool) {
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

    pub(crate) fn history_go(&mut self, delta: i32) {
        history_go(self.web_view, delta);
    }

    pub(crate) fn cleanup(&mut self) {
        unsafe {
            if self.web_view != nil {
                let () = msg_send![self.web_view, stopLoading];
            }
        }
        self.detach();
    }
}

#[cfg(target_os = "macos")]
fn clipped_browser_layout(
    unclipped_rect: Rect,
    clipped_rect: Rect,
    visible: bool,
) -> (Rect, Rect, bool) {
    let web_view_rect = Rect {
        pos: unclipped_rect.pos - clipped_rect.pos,
        size: unclipped_rect.size,
    };
    let is_visible = visible && clipped_rect.size.x > 0.0 && clipped_rect.size.y > 0.0;
    (clipped_rect, web_view_rect, is_visible)
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::clipped_browser_layout;
    use crate::makepad_math::{dvec2, Rect};

    #[test]
    fn keeps_native_browser_anchored_when_top_is_clipped() {
        let unclipped_rect = Rect {
            pos: dvec2(26.0, 262.0),
            size: dvec2(298.0, 420.0),
        };
        let clipped_rect = Rect {
            pos: dvec2(26.0, 262.0),
            size: dvec2(298.0, 319.0),
        };

        let (host_rect, web_view_rect, is_visible) =
            clipped_browser_layout(unclipped_rect, clipped_rect, true);

        assert_eq!(host_rect, clipped_rect);
        assert_eq!(web_view_rect.pos, dvec2(0.0, 0.0));
        assert_eq!(web_view_rect.size, unclipped_rect.size);
        assert!(is_visible);
    }

    #[test]
    fn offsets_native_browser_inside_clip_when_bottom_is_clipped() {
        let unclipped_rect = Rect {
            pos: dvec2(26.0, 120.0),
            size: dvec2(298.0, 420.0),
        };
        let clipped_rect = Rect {
            pos: dvec2(26.0, 170.0),
            size: dvec2(298.0, 370.0),
        };

        let (_, web_view_rect, _) = clipped_browser_layout(unclipped_rect, clipped_rect, true);

        assert_eq!(web_view_rect.pos, dvec2(0.0, -50.0));
    }

    #[test]
    fn hides_when_clip_is_empty() {
        let unclipped_rect = Rect {
            pos: dvec2(26.0, 262.0),
            size: dvec2(298.0, 420.0),
        };
        let clipped_rect = Rect {
            pos: dvec2(26.0, 262.0),
            size: dvec2(0.0, 0.0),
        };

        let (_, _, is_visible) = clipped_browser_layout(unclipped_rect, clipped_rect, true);

        assert!(!is_visible);
    }
}

#[cfg(target_os = "ios")]
pub(crate) struct IosSystemBrowser {
    current_url: String,
    web_view: ObjcId,
}

#[cfg(target_os = "ios")]
impl IosSystemBrowser {
    pub(crate) fn new(url: &str) -> Self {
        let mut browser = Self {
            current_url: String::new(),
            web_view: nil,
        };
        browser.ensure_web_view();
        browser.set_url(url, false);
        browser
    }

    fn ensure_web_view(&mut self) {
        if self.web_view != nil {
            return;
        }
        unsafe {
            let config: ObjcId = msg_send![class!(WKWebViewConfiguration), new];
            let web_view: ObjcId = msg_send![class!(WKWebView), alloc];
            let web_view: ObjcId = msg_send![web_view, initWithFrame: NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize { width: 1.0, height: 1.0 }
            } configuration: config];
            if web_view != nil {
                let () = msg_send![web_view, setHidden: YES];
                self.web_view = web_view;
            }
        }
    }

    pub(crate) fn update(&mut self, parent_view: ObjcId, rect: Rect, visible: bool) {
        self.ensure_web_view();
        unsafe {
            if self.web_view == nil {
                return;
            }
            let super_view: ObjcId = msg_send![self.web_view, superview];
            if super_view != parent_view {
                let () = msg_send![self.web_view, removeFromSuperview];
                let () = msg_send![parent_view, addSubview: self.web_view];
            }
            let () = msg_send![parent_view, bringSubviewToFront: self.web_view];
            let frame = NSRect {
                origin: NSPoint {
                    x: rect.pos.x,
                    y: rect.pos.y,
                },
                size: NSSize {
                    width: rect.size.x.max(0.0),
                    height: rect.size.y.max(0.0),
                },
            };
            let () = msg_send![self.web_view, setFrame: frame];
            let () = msg_send![self.web_view, setHidden: if visible { NO } else { YES }];
        }
    }

    pub(crate) fn detach(&mut self) {
        unsafe {
            if self.web_view != nil {
                let () = msg_send![self.web_view, removeFromSuperview];
                let () = msg_send![self.web_view, setHidden: YES];
            }
        }
    }

    pub(crate) fn set_url(&mut self, url: &str, _replace: bool) {
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

    pub(crate) fn history_go(&mut self, delta: i32) {
        history_go(self.web_view, delta);
    }

    pub(crate) fn cleanup(&mut self) {
        unsafe {
            if self.web_view != nil {
                let () = msg_send![self.web_view, stopLoading];
            }
        }
        self.detach();
    }
}
