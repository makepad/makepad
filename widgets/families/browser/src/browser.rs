//! The system's own web view (WKWebView on macOS and iOS), placed over the
//! widget's rect. Chromium pages are not a widget: they live in the browser
//! host (libs/browser_view), out of the app's process.

use crate::{image::DrawImage, makepad_derive_widget::*, makepad_draw::*, widget::*};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.BrowserBase = #(Browser::register_widget(vm))

    mod.widgets.Browser = set_type_default() do mod.widgets.BrowserBase{
        width: Fill
        height: Fill
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Browser {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawImage,
    #[live]
    url: ArcStringMut,
    #[visible]
    #[live(true)]
    visible: bool,
    #[rust]
    last_url: String,
    #[rust]
    spawned: bool,
    #[rust]
    unsupported_logged: bool,
}

impl Browser {
    fn system_browser_id(&self) -> SystemBrowserId {
        SystemBrowserId(LiveId(self.uid.0))
    }

    fn supported() -> bool {
        cfg!(any(target_os = "macos", target_os = "ios"))
    }

    fn sync_system_browser(&mut self, cx: &mut Cx) {
        let id = self.system_browser_id();
        if !self.spawned {
            cx.system_browser(id).spawn(self.url.as_ref());
            self.spawned = true;
            self.last_url.clear();
            self.last_url.push_str(self.url.as_ref());
        }
        let url = self.url.as_ref();
        if self.last_url != url {
            cx.system_browser(id).set_url(url, false);
            self.last_url.clear();
            self.last_url.push_str(url);
        }
        let area = self.draw_bg.area();
        if self.visible && area.is_valid(cx) {
            cx.system_browser(id).update(area, true);
        } else {
            cx.system_browser(id).detach();
        }
    }

    fn set_url_internal(&mut self, cx: &mut Cx, url: &str) {
        self.url.set(url);
        if self.spawned {
            cx.system_browser(self.system_browser_id()).set_url(url, false);
            self.last_url.clear();
            self.last_url.push_str(url);
        }
        self.redraw(cx);
    }

    fn set_visible_internal(&mut self, cx: &mut Cx, visible: bool) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible && self.spawned {
            cx.system_browser(self.system_browser_id()).detach();
        }
        self.redraw(cx);
    }
}

impl Widget for Browser {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if matches!(event, Event::Shutdown) && self.spawned {
            cx.system_browser(self.system_browser_id()).close();
            self.spawned = false;
        }
        if let Event::WindowGeomChange(_) = event {
            self.redraw(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            if self.spawned {
                cx.system_browser(self.system_browser_id()).detach();
            }
            return DrawStep::done();
        }
        self.draw_bg.draw_vars.empty_texture(0);
        self.draw_bg.draw_walk(cx, walk);
        if Self::supported() {
            self.sync_system_browser(cx);
        } else if !std::mem::replace(&mut self.unsupported_logged, true) {
            log!("Browser: the system web view is only implemented on macOS and iOS");
        }
        DrawStep::done()
    }
}

impl BrowserRef {
    pub fn set_url(&self, cx: &mut Cx, url: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_url_internal(cx, url);
        }
    }

    pub fn set_visible(&self, cx: &mut Cx, visible: bool) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_visible_internal(cx, visible);
        }
    }
}
