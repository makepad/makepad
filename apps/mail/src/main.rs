//! Standalone Mail: read-only local Apple Mail search, with an explicit demo mode.

pub use makepad_widgets;
use makepad_ai_services::port::{AiServicePort, PortEvent};
use makepad_mail::{ai, view::MailView, local_view::LocalMailView, source::LocalConfig};
use makepad_strict_json::Value;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Mail"
                window.inner_size: vec2(1360, 860)
                pass +: { clear_color: theme.color_bg_app }
                body +: {
                    padding: 0 margin: 0 spacing: 0 flow: Overlay
                    mail := MailView{}
                    local_mail := LocalMailView{visible: false}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    ai_port: Option<AiServicePort>,
    #[rust]
    ai_context: String,
    #[rust]
    closing: bool,
    #[rust]
    local_mode: bool,
}

impl App {
    fn ai_summary(&self, cx: &mut Cx) -> String {
        self.ui
            .widget(cx, ids!(mail))
            .borrow::<MailView>()
            .map(|view| view.ai_summary())
            .unwrap_or_default()
    }

    fn ai_answer(&self, cx: &mut Cx, call: &makepad_ai_services::wire::ServiceCall) -> makepad_ai_services::wire::ToolResult {
        self.ui
            .widget(cx, ids!(mail))
            .borrow::<MailView>()
            .map(|view| view.ai_answer(call))
            .unwrap_or_else(|| {
                makepad_ai_services::wire::ToolResult::unavailable(&call.call_id, "mail is gone")
            })
    }

    fn refresh_ai_context(&mut self, cx: &mut Cx) {
        if self.ai_port.is_none() {
            return;
        }
        let text = self.ai_summary(cx);
        if text == self.ai_context {
            return;
        }
        self.ai_context = text.clone();
        if let Some(port) = self.ai_port.as_ref() {
            port.set_context(&text);
        }
    }

    fn drain_ai_port(&mut self, cx: &mut Cx, event: &Event) {
        let events = match self.ai_port.as_mut() {
            Some(port) => port.handle_event(cx, event),
            None => return,
        };
        for ev in events {
            match ev {
                PortEvent::Registered(endpoint) => {
                    log!("mail: AI service registered as {}", endpoint.as_str());
                    self.ai_context.clear();
                    self.refresh_ai_context(cx);
                }
                PortEvent::Call(call) => {
                    let result = self.ai_answer(cx, &call);
                    if let Some(port) = self.ai_port.as_ref() {
                        port.reply(result);
                    }
                }
                PortEvent::Cancel { .. } | PortEvent::ChatOpen { .. } => {}
                PortEvent::Subscribe { .. } | PortEvent::Unsubscribe { .. } => {}
            }
        }
    }
}

fn size_from_args(args: &[String]) -> Option<Vec2d> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = match arg.strip_prefix("--size=") {
            Some(value) => value,
            None if arg == "--size" => args.next()?,
            None => continue,
        };
        let (width, height) = value.split_once(['x', 'X'])?;
        return Some(dvec2(width.trim().parse().ok()?, height.trim().parse().ok()?));
    }
    None
}

fn is_close_requested(json: &str) -> bool {
    let Ok(value) = makepad_strict_json::parse(json.as_bytes()) else {
        return false;
    };
    let Some(wm) = value.get("wm") else {
        return false;
    };
    match wm {
        Value::Obj(pairs) => pairs.iter().any(|(k, v)| {
            k == "CloseRequested" && matches!(v, Value::Arr(items) if items.is_empty())
        }),
        _ => false,
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let args: Vec<_> = std::env::args().collect();
        // `--size WxH` (or `--size=WxH`) sets the window size at start, so the
        // layout's breakpoints can be checked without resizing by hand.
        if let Some(size) = size_from_args(&args) {
            self.ui.window(cx, ids!(main_window)).resize(cx, size);
        }
        self.local_mode = !args.iter().any(|a| a == "--demo") && (cfg!(target_os = "macos") || args.iter().any(|a| a == "--mail-root"));
        if self.local_mode {
            let arg = |key: &str| args.iter().position(|a| a == key).and_then(|i| args.get(i+1)).map(std::path::PathBuf::from);
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from).unwrap_or_default();
            let config = LocalConfig::for_mail_root(
                arg("--mail-root").unwrap_or_else(|| home.join("Library/Mail")),
                arg("--mail-cache"),
            );
            // Resolve the initial, hidden child before the indexed tree has been drawn.
            let local = self.ui.child_by_path(ids!(local_mail));
            if let Some(mut view) = local.borrow_mut::<LocalMailView>() {
                view.configure(config);
                log!("mail: local view configured");
            } else {
                error!("mail: local view was not created");
                return;
            }
            self.ui.child_by_path(ids!(mail)).set_visible(cx, false);
            local.set_visible(cx, true);
            // Private mailbox contents are not registered with the demo AI bus.
            return;
        }
        self.ai_port = AiServicePort::open(cx, ai::manifest());
        if let Some(mut view) = self.ui.widget(cx, ids!(mail)).borrow_mut::<MailView>() {
            view.set_storage(cx.storage("mail"));
        }
    }

    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

#[cfg(target_os = "macos")]
fn native_mail_dark_appearance() -> bool {
    use makepad_widgets::makepad_platform::os::apple::apple_sys::*;
    // Use the same AppKit matching as the standalone workspace appearance
    // picker. Contrast variants resolve to the corresponding light/dark theme.
    unsafe {
        extern "C" {
            static NSAppearanceNameAqua: ObjcId;
            static NSAppearanceNameDarkAqua: ObjcId;
        }
        let main_thread: bool = msg_send![class!(NSThread), isMainThread];
        if !main_thread { return false; }
        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
        let app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let appearance: ObjcId = msg_send![app, effectiveAppearance];
        let names = [NSAppearanceNameAqua, NSAppearanceNameDarkAqua];
        let supported: ObjcId = msg_send![class!(NSArray), arrayWithObjects: names.as_ptr() count: names.len()];
        let best: ObjcId = msg_send![appearance, bestMatchFromAppearancesWithNames: supported];
        let dark = if best.is_null() { false } else {
            let result: BOOL = msg_send![best, isEqualToString: NSAppearanceNameDarkAqua];
            result == YES
        };
        let () = msg_send![pool, drain];
        dark
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(target_os = "macos")]
        if desktop_style::current_name(vm).is_none()
            && std::env::var_os("MAKEPAD_WM_THEME_SPLASH").is_none()
        {
            let dark = native_mail_dark_appearance();
            desktop_style::install(vm, desktop_style::StyleSheet::load_with_appearance(
                desktop_style::DesktopStyle::Macos, dark,
            ));
            log!("mail: using macOS {} appearance", if dark { "dark" } else { "light" });
        }
        makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        makepad_mail::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Custom(json) = event {
            if is_close_requested(json) {
                if self.local_mode {
                    if let Some(mut view) = self.ui.child_by_path(ids!(local_mail)).borrow_mut::<LocalMailView>() { view.shutdown(); }
                    cx.quit();
                    return;
                }
                let ready = self
                    .ui
                    .widget(cx, ids!(mail))
                    .borrow_mut::<MailView>()
                    .map(|mut view| view.request_close(cx))
                    .unwrap_or(true);
                if ready {
                    cx.quit();
                } else {
                    self.closing = true;
                }
                return;
            }
        }
        self.drain_ai_port(cx, event);
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if self.closing {
            let quit = self
                .ui
                .widget(cx, ids!(mail))
                .borrow::<MailView>()
                .map(|view| view.should_quit())
                .unwrap_or(true);
            if quit {
                cx.quit();
            }
        }
        self.refresh_ai_context(cx);
    }
}
