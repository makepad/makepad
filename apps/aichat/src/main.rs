//! aichat: the assistant as its own window, and as the window manager's
//! special child. Both are this binary: standalone it is a window with
//! the panel in it and whatever services are linked in-process (none,
//! until an app embeds it); under the WM (`--stdin-loop`) the WM seats it
//! in the pane slot and every other app's service reaches it over the bus
//! as studio `Custom` frames, which the panel turns into registry links.

pub use makepad_widgets;
use makepad_aichat::AiChatPanelAction;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(460, 760)
                window.title: "AI"
                pass +: { clear_color: theme.color_bg_app }
                body +: {
                    panel := AiChatPanel{
                        width: Fill
                        height: Fill
                    }
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

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        makepad_wm_api::set_title(cx, "AI");
        // The composer takes the keyboard as soon as the pane is up.
        self.ui.text_input(cx, ids!(panel.input)).set_key_focus(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for action in actions {
            if let Some(widget_action) = action.as_widget_action() {
                if let AiChatPanelAction::Close = widget_action.cast() {
                    // Under the WM the pane hides; standalone the window stays.
                    let _ = makepad_wm_api::send(cx, &makepad_wm_api::WmRequest::Close);
                }
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_wm_theme::apply(vm);
        makepad_aichat::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // Bus frames reach the panel through its own handle_event; the
        // WM's own frames are not ours to act on here yet.
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
