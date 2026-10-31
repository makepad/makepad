pub use makepad_widgets;

use makepad_widgets::*;
use std::collections::HashMap;

app_main!(App);

const INITIAL_URL: &str = "https://makepad.nl";
const NATIVE_BROWSER_COUNT: usize = 20;
const INITIAL_TAB: usize = if cfg!(target_os = "android") { 2 } else { 0 };

// Self-contained: the sandwich/input proof needs no network or external page.
const SANDWICH_URL: &str = concat!(
    "data:text/html;charset=utf-8,<!doctype html><html><head><meta name='viewport' content='width=device-width,initial-scale=1'><title>Sandwich page</title>",
    "<style>body{margin:0;padding:28px;background:aliceblue;color:midnightblue;font:20px sans-serif}",
    "input,button,a{font:inherit;margin:12px;padding:12px}section{height:480px}</style></head><body>",
    "<h1>Engine-host texture</h1><p>The popup covers this page; the rest accepts input.</p>",
    "<section><p style='padding-top:100px'>Type here:</p><input style='position:absolute;left:380px;top:160px;width:220px;height:32px;margin:0;padding:4px' placeholder='Text and IME input' onfocus='document.title=`Native input focused`' ",
    "oninput='document.title=this.value||`Sandwich page`'><button onclick='document.title=`Page clicked`'>Click page</button>",
    "<a href='%23second'>History entry</a></section><h2 id='second'>Page scroll target</h2>",
    "<section>Scroll inside the page, or beside it to move the Makepad list.</section></body></html>",
);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

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

                browser := WebView{
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
            tabs: [@cef_tab @native_tab @sandwich_tab]
            selected: #(INITIAL_TAB)
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

        sandwich_tab := DockTab{
            name: "Sandwich"
            template: @PermanentTab
            kind: @TabSandwich
        }

        TabCEF := View{
            width: Fill
            height: Fill

            cef_browser := WebView{
                prefer_textured: true
                url: "https://www.google.nl"
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
                    text: "This tab keeps 20 persistent web views. Their mode follows platform support."
                    draw_text.color: #x94a7d4
                    draw_text.text_style.font_size: 10
                }
            }

            native_browser_list := mod.widgets.NativeBrowserList{}
        }

        TabSandwich := View{
            width: Fill
            height: Fill
            flow: Overlay

            sandwich_scroll := ScrollYView{
                width: Fill
                height: Fill
                flow: Down
                spacing: 18
                padding: Inset{left: 24 right: 24 top: 100 bottom: 24}

                Label{
                    text: "Native layer sandwich"
                    draw_text.text_style: theme.font_bold{font_size: 24}
                }
                Label{
                    width: Fill
                    text: "Scroll the page to move the web view between ordinary Makepad content."
                }
                sandwich_web := WebView{
                    width: Fill
                    height: 540
                    border_radius: 12
                    url: #(SANDWICH_URL)
                }
                page_controls := View{
                    width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 8
                    page_back := Button{text: "Back"}
                    page_forward := Button{text: "Forward"}
                    page_reload := Button{text: "Reload"}
                    page_stop := Button{text: "Stop"}
                    page_status := Label{text: "Waiting for page"}
                }
                RoundedView{
                    width: Fill
                    height: 420
                    padding: Inset{left: 20 top: 20 right: 20 bottom: 20}
                    draw_bg +: {color: theme.color_bg_app}
                    Label{text: "Makepad content below the web view"}
                }
            }

            // Drawn after the native slot: scrolling the retained list underneath this
            // fixed header exercises view_shift and the upper surface in the same frame.
            sandwich_header := RoundedView{
                width: Fill
                height: 76
                padding: Inset{left: 24 right: 24 top: 18 bottom: 18}
                draw_bg +: {color: #x17191d border_radius: 12}
                Label{
                    text: "Sandwich: fixed Makepad header above native content"
                    draw_text.color: #fff
                    draw_text.text_style: theme.font_bold{font_size: 18}
                }
            }

            // Drawn after the scroll view, across its native layer's bounds.
            RoundedView{
                width: 300
                cursor: MouseCursor.Default
                height: Fit
                margin: Inset{left: 40 top: 180}
                padding: Inset{left: 18 right: 18 top: 18 bottom: 18}
                flow: Down
                spacing: 10
                draw_bg +: {color: #x293c62 border_radius: 12}
                Label{
                    text: "Floating popup"
                    draw_text.color: #fff
                    draw_text.text_style: theme.font_bold{font_size: 15}
                }
                Label{
                    width: Fill
                    text: "This popup is drawn after the web view."
                    draw_text.color: #fff
                }
                popup_button := Button{text: "Test popup input"}
                popup_status := Label{text: "Waiting for a click" draw_text.color: #fff}
            }
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
    browsers: HashMap<WidgetUid, WebViewRef>,
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
        let browser = item.web_view(cx, ids!(browser));
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
        let sandwich = self.ui.dock(cx, ids!(dock)).item(id!(sandwich_tab));
        let page = sandwich.web_view(cx, ids!(sandwich_web));
        if sandwich.button(cx, ids!(page_back)).clicked(actions) { page.history(cx, -1); }
        if sandwich.button(cx, ids!(page_forward)).clicked(actions) { page.history(cx, 1); }
        if sandwich.button(cx, ids!(page_reload)).clicked(actions) { page.reload(cx); }
        if sandwich.button(cx, ids!(page_stop)).clicked(actions) { page.stop(cx); }
        let state = page.state();
        sandwich.label(cx, ids!(page_status)).set_text(cx, &format!("{:?}: {}{}", state.mode, state.title, if state.loading { " (loading)" } else { "" }));
        if sandwich.button(cx, ids!(popup_button)).clicked(actions) {
            sandwich.label(cx, ids!(popup_status)).set_text(cx, "Popup received the click");
        }
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
            if std::env::args().any(|arg| arg == "--sandwich") {
                self.ui.dock(cx, ids!(dock)).select_tab(cx, id!(sandwich_tab));
            }
        }
        self.match_event(cx, event);
        static TRACE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let trace = *TRACE.get_or_init(|| std::env::var("MAKEPAD_NATIVE_LAYER_TRACE").is_ok_and(|v| v == "1"));
        if trace && matches!(event, Event::Draw(_)) {
            // Sample the offset the turtle is about to use. Drawing the scroll bars later
            // clamps their stored position, after this frame's rubber-band layout was drawn.
            let sandwich = self.ui.dock(cx, ids!(dock)).item(id!(sandwich_tab));
            // Dock creates the page during its first draw; its initial scroll is zero.
            let scroll_y = sandwich.view(cx, ids!(sandwich_scroll)).scroll_extent().map_or(0.0, |extent| extent.pos.y);
            eprintln!("[sandwich] next_frame={} scroll_y={scroll_y}", cx.frame_submission_serial() + 1);
        }
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if trace {
            if let Event::NativeLayer(e) = event { eprintln!("[sandwich] native_event={e:?}"); }
        }
    }
}
