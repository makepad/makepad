pub use makepad_widgets;

use makepad_browser_view::{BrowserViews, ViewId};
use makepad_widgets::image::DrawImage;
use makepad_widgets::*;
use std::collections::HashMap;

app_main!(App);

const INITIAL_URL: &str = "https://makepad.nl";
const HOSTED_URL: &str = "https://www.google.nl";
const NATIVE_BROWSER_COUNT: usize = 20;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.HostedPageBase = #(HostedPage::register_widget(vm))
    mod.widgets.HostedPage = set_type_default() do mod.widgets.HostedPageBase{
        width: Fill
        height: Fill
    }

    mod.widgets.NativeBrowserListBase = #(NativeBrowserList::register_widget(vm))
    mod.widgets.NativeBrowserList = set_type_default() do mod.widgets.NativeBrowserListBase{
        width: Fill
        height: Fill

        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            drag_scrolling: true
            scroll_bar: ScrollBar{}

            BrowserCard := RoundedView{
                width: Fill
                height: 420
                margin: Inset{top: 8 bottom: 8 left: 0 right: 0}
                padding: Inset{top: 14 bottom: 14 left: 14 right: 14}
                flow: Down
                spacing: 10
                draw_bg.color: #x101521
                draw_bg.border_radius: 12.0

                header := View{
                    width: Fill
                    height: Fit
                    flow: Down
                    spacing: 4

                    title := Label{
                        text: "Browser"
                        draw_text.color: #xfff
                        draw_text.text_style: theme.font_bold{font_size: 12}
                    }

                    subtitle := Label{
                        text: ""
                        draw_text.color: #x9fb0d7
                        draw_text.text_style: theme.font_regular{font_size: 10}
                    }
                }

                browser := Browser{
                    width: Fill
                    height: Fill
                    url: "https://makepad.nl"
                }
            }
        }
    }

    let AppDock = Dock{
        width: Fill
        height: Fill

        root := DockTabs{
            tabs: [@cef_tab @native_tab]
            selected: 0
            closable: false
        }

        cef_tab := DockTab{
            name: "Hosted CEF"
            template: @PermanentTab
            kind: @TabCEF
        }

        native_tab := DockTab{
            name: "Native PortalList"
            template: @PermanentTab
            kind: @TabNative
        }

        TabCEF := View{
            width: Fill
            height: Fill

            cef_browser := mod.widgets.HostedPage{
                width: Fill
                height: Fill
            }
        }

        TabNative := View{
            width: Fill
            height: Fill
            padding: Inset{top: 12 bottom: 12 left: 12 right: 12}
            flow: Down
            spacing: 10

            RoundedView{
                width: Fill
                height: Fit
                flow: Down
                spacing: 4
                padding: Inset{top: 12 bottom: 12 left: 14 right: 14}
                draw_bg +: {
                    color: #x17191d
                }

                Label{
                    text: "Native browser PortalList"
                    draw_text.color: #xfff
                    draw_text.text_style: theme.font_bold{font_size: 12}
                }

                Label{
                    text: "This tab keeps 20 persistent system browsers attached to their Browser widgets while the Dock tab is active."
                    draw_text.color: #x94a7d4
                    draw_text.text_style.font_size: 10
                }
            }

            native_browser_list := mod.widgets.NativeBrowserList{}
        }
    }

    mod.gc.set_static(AppDock)
    mod.gc.run()

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1440, 960)
                body +: {
                    dock := AppDock{}
                }
            }
        }
    }
}

/// A page of the browser host (Chromium in its own process, the one CEF
/// path): drawn from its frame ring, sized to the widget, given its hits.
#[derive(Script, ScriptHook, Widget)]
pub struct HostedPage {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawImage,
    #[rust]
    views: Option<BrowserViews>,
    #[rust]
    view: Option<ViewId>,
}

impl Widget for HostedPage {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let Some(views) = &mut self.views else { return };
        if views.handle_event(cx, event) {
            self.draw_bg.redraw(cx);
        }
        let Some(view) = self.view else { return };
        let area = self.draw_bg.area();
        let hit = event.hits_with_capture_overload(cx, area, true);
        if let Hit::FingerDown(_) = hit {
            cx.set_key_focus(area);
        }
        let rect = area.is_valid(cx).then(|| area.rect(cx));
        views.forward_hit(cx, view, &hit, |abs| rect.map(|rect| abs - rect.pos));
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.peek_walk_turtle(walk);
        let views = self.views.get_or_insert_with(|| BrowserViews::new("makepad-example-browser"));
        if self.view.is_none() {
            self.view = views.open(HOSTED_URL, true, 0xff00_0000);
        }
        if let Some(view) = self.view {
            views.set_size(view, rect.size, cx.current_dpi_factor());
        }
        views.service(cx);
        match self.view.and_then(|view| views.frame(view)) {
            Some(frame) if frame.info.width > 0 => {
                self.draw_bg.image_scale = vec2(frame.info.width as f32 / frame.alloc[0] as f32, frame.info.height as f32 / frame.alloc[1] as f32);
                self.draw_bg.draw_vars.set_texture(0, &frame.texture);
            }
            _ => self.draw_bg.draw_vars.empty_texture(0),
        }
        self.draw_bg.draw_walk(cx, walk);
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

#[derive(Script, ScriptHook, Widget)]
pub struct NativeBrowserList {
    #[deref]
    view: View,
    #[rust]
    active: bool,
    #[rust]
    bindings: HashMap<WidgetUid, usize>,
    #[rust]
    browsers: HashMap<WidgetUid, BrowserRef>,
}

impl NativeBrowserList {
    fn set_active_internal(&mut self, cx: &mut Cx, active: bool) {
        if self.active == active {
            return;
        }
        self.active = active;
        for browser in self.browsers.values() {
            browser.set_visible(cx, active);
        }
        self.redraw(cx);
    }

    fn bind_item(&mut self, cx: &mut Cx, item: &WidgetRef, item_id: usize) {
        let widget_uid = item.widget_uid();
        let is_new_browser = !self.browsers.contains_key(&widget_uid);
        let browser = item.browser(cx, ids!(browser));
        browser.set_visible(cx, self.active);

        if self.bindings.get(&widget_uid) != Some(&item_id) {
            self.bindings.insert(widget_uid, item_id);
            item.label(cx, ids!(title))
                .set_text(cx, &format!("Native Browser {}", item_id + 1));
            item.label(cx, ids!(subtitle)).set_text(
                cx,
                &format!("PortalList row {} attached to {}", item_id + 1, INITIAL_URL),
            );
        }

        if is_new_browser {
            browser.set_url(cx, INITIAL_URL);
        }

        self.browsers.insert(widget_uid, browser);
    }
}

impl Widget for NativeBrowserList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, NATIVE_BROWSER_COUNT);
                while let Some(item_id) = list.next_visible_item(cx) {
                    let item_widget = list.item(cx, item_id, id!(BrowserCard));
                    self.bind_item(cx, &item_widget, item_id);
                    item_widget.draw_all_unscoped(cx);
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

impl App {
    fn set_native_tab_active(&mut self, cx: &mut Cx, active: bool) {
        self.ui
            .dock(cx, ids!(dock))
            .item(id!(native_tab))
            .widget(cx, ids!(native_browser_list))
            .borrow_mut::<NativeBrowserList>()
            .map(|mut list| list.set_active_internal(cx, active));
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            let Some(action) = action.as_widget_action() else {
                continue;
            };
            if let DockAction::TabWasPressed(tab_id) = action.cast() {
                self.set_native_tab_active(cx, tab_id == id!(native_tab));
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            self.set_native_tab_active(cx, false);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
