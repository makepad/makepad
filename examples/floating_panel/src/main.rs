pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let Card = RoundedView{
        width: Fill
        height: Fit
        flow: Down
        spacing: 10
        padding: 18
        draw_bg.color: #x151c27
        draw_bg.border_radius: 18.0
    }

    let Tag = RoundedView{
        width: Fit
        height: Fit
        padding: Inset{top: 6, bottom: 6, left: 10, right: 10}
        draw_bg.color: #x243041
        draw_bg.border_radius: 999.0
        label := Label{
            text: "Tag"
            draw_text.color: #xa8c2ff
            draw_text.text_style.font_size: 10
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Floating Panel Example"
                window.inner_size: vec2(780, 560)
                window.position: vec2(110, 90)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 18
                        padding: 28
                        draw_bg.color: #x0d1117

                        View{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 8

                            Tag{label.text: "macOS"}
                            Label{
                                text: "Floating panel windows"
                                draw_text.color: #xeff5ff
                                draw_text.text_style: theme.font_bold{font_size: 28}
                            }
                            Label{
                                text: "The secondary window is configured at startup as a non-activating floating panel. It stays above normal windows, hides the built-in caption strip, and uses a manual drag zone in its header."
                                draw_text.color: #x97a9c0
                                width: Fill
                            }
                        }

                        info_card := Card{
                            Label{
                                text: "What to try"
                                draw_text.color: #xffffff
                                draw_text.text_style: theme.font_bold{font_size: 16}
                            }
                            Label{
                                text: "1. Drag the panel by its top header area."
                                draw_text.color: #xc9d5e7
                            }
                            Label{
                                text: "2. Type into the panel input and press Enter."
                                draw_text.color: #xc9d5e7
                            }
                            Label{
                                text: "3. Click the panel button and watch this window update."
                                draw_text.color: #xc9d5e7
                            }
                        }

                        status_card := Card{
                            Label{
                                text: "Live status"
                                draw_text.color: #xffffff
                                draw_text.text_style: theme.font_bold{font_size: 16}
                            }
                            status_label := Label{
                                width: Fill
                                text: "Waiting for panel interaction."
                                draw_text.color: #x9fd3af
                            }
                            platform_note := Label{
                                width: Fill
                                text: "Configuring panel..."
                                draw_text.color: #x8aa0bc
                            }
                        }

                        config_card := Card{
                            Label{
                                text: "Panel config"
                                draw_text.color: #xffffff
                                draw_text.text_style: theme.font_bold{font_size: 16}
                            }
                            Label{
                                width: Fill
                                text: "kind: FloatingPanel"
                                draw_text.color: #xc9d5e7
                            }
                            Label{
                                width: Fill
                                text: "chrome: Borderless"
                                draw_text.color: #xc9d5e7
                            }
                            Label{
                                width: Fill
                                text: "level: Floating, non_activating: true"
                                draw_text.color: #xc9d5e7
                            }
                            Label{
                                width: Fill
                                text: "join_all_spaces: true, full_screen_auxiliary: true, becomes_key_only_if_needed: true"
                                draw_text.color: #xc9d5e7
                            }
                        }
                    }
                }
            }

            panel_window := Window{
                show_caption_bar: false
                window.title: "Inspector Panel"
                window.inner_size: vec2(340, 520)
                window.position: vec2(930, 120)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 14
                        padding: 16
                        draw_bg.color: #x111927

                        RoundedView{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 6
                            padding: 14
                            draw_bg.color: #x1c283a
                            draw_bg.border_radius: 16.0

                            Label{
                                text: "Floating inspector"
                                draw_text.color: #xffffff
                                draw_text.text_style: theme.font_bold{font_size: 15}
                            }
                            Label{
                                width: Fill
                                text: "Drag from this strip. The widget caption bar is hidden, so App::handle_event answers WindowDragQuery for this header."
                                draw_text.color: #xa8b9d3
                                draw_text.text_style.font_size: 11
                            }
                        }

                        RoundedView{
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 10
                            padding: 14
                            draw_bg.color: #x172133
                            draw_bg.border_radius: 16.0

                            Label{
                                text: "Interactive controls"
                                draw_text.color: #xffffff
                                draw_text.text_style: theme.font_bold{font_size: 14}
                            }
                            panel_input := TextInput{
                                width: Fill
                                height: 44
                                empty_text: "Type here and press Enter"
                                return_key_type: Done
                            }
                            ping_button := Button{
                                text: "Send update to main window"
                            }
                            panel_status := Label{
                                width: Fill
                                text: "Panel ready."
                                draw_text.color: #x94a9c8
                                draw_text.text_style.font_size: 11
                            }
                            drag_target := RoundedView{
                                width: Fill
                                height: 72
                                flow: Down
                                spacing: 6
                                padding: 12
                                cursor: MouseCursor.Hand
                                draw_bg.color: #x243041
                                draw_bg.border_radius: 14.0
                                Label{
                                    text: "Drag target"
                                    draw_text.color: #xffffff
                                    draw_text.text_style: theme.font_bold{font_size: 13}
                                }
                                drag_value := Label{
                                    width: Fill
                                    text: "Drag delta: 0, 0"
                                    draw_text.color: #xa8b9d3
                                    draw_text.text_style.font_size: 11
                                }
                            }
                        }
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
    #[rust]
    configured_panel: bool,
    #[rust]
    panel_window_id: Option<WindowId>,
    #[rust]
    panel_pings: usize,
    #[rust]
    drag_origin: Option<DVec2>,
}

impl App {
    fn configure_panel(&mut self, cx: &mut Cx) {
        let panel = self.ui.window(cx, ids!(panel_window));
        let mut macos = MacosWindowConfig::floating_panel();
        macos.chrome = MacosWindowChrome::Borderless;
        macos.becomes_key_only_if_needed = true;
        panel.configure_macos_window(cx, macos);
        self.panel_window_id = panel.window_id();

        let platform_note = match cx.os_type() {
            OsType::Macos => {
                "On macOS the secondary window is a non-activating NSPanel with a borderless style and floating window level."
            }
            _ => {
                "On non-macOS platforms the secondary window still opens, but the macOS-only floating-panel flags are ignored."
            }
        };
        self.ui
            .label(cx, ids!(platform_note))
            .set_text(cx, platform_note);
    }

    fn set_status(&self, cx: &mut Cx, message: &str) {
        self.ui.label(cx, ids!(status_label)).set_text(cx, message);
        self.ui.label(cx, ids!(panel_status)).set_text(cx, message);
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(ping_button)).clicked(actions) {
            self.panel_pings += 1;
            let suffix = if self.panel_pings == 1 { "" } else { "s" };
            let message = format!("Panel button pressed {} time{}.", self.panel_pings, suffix);
            self.set_status(cx, &message);
        }

        if let Some((text, _mods)) = self.ui.text_input(cx, ids!(panel_input)).returned(actions) {
            let message = if text.trim().is_empty() {
                "Panel input submitted an empty string.".to_string()
            } else {
                format!("Panel submitted: \"{}\"", text)
            };
            self.set_status(cx, &message);
        }

        if let Some(text) = self.ui.text_input(cx, ids!(panel_input)).changed(actions) {
            let message = if text.trim().is_empty() {
                "Panel input cleared.".to_string()
            } else {
                format!("Panel typing: \"{}\"", text)
            };
            self.set_status(cx, &message);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Startup = event {
            if !self.configured_panel {
                self.configured_panel = true;
                self.configure_panel(cx);
            }
        }

        if let Event::WindowDragQuery(dq) = event {
            if Some(dq.window_id) == self.panel_window_id {
                let panel = self.ui.window(cx, ids!(panel_window));
                let size = panel.get_inner_size(cx);
                if dq.abs.y < 58.0 && dq.abs.x < size.x - 18.0 {
                    dq.response.set(WindowDragQueryResponse::Caption);
                    cx.set_cursor(MouseCursor::Default);
                }
            }
        }

        let drag_area = self.ui.widget(cx, ids!(drag_target)).area();
        if drag_area.is_valid(cx) {
            let drag_rect = drag_area.rect(cx);
            match event {
                Event::MouseDown(mouse) if Some(mouse.window_id) == self.panel_window_id => {
                    if drag_rect.contains(mouse.abs) {
                        self.drag_origin = Some(mouse.abs);
                    }
                }
                Event::MouseMove(mouse) if Some(mouse.window_id) == self.panel_window_id => {
                    if let Some(origin) = self.drag_origin {
                        let delta = mouse.abs - origin;
                        let message = format!("Drag delta: {:.0}, {:.0}", delta.x, delta.y);
                        self.ui.label(cx, ids!(drag_value)).set_text(cx, &message);
                        self.set_status(cx, &message);
                    }
                }
                Event::MouseUp(mouse) if Some(mouse.window_id) == self.panel_window_id => {
                    if let Some(origin) = self.drag_origin.take() {
                        let delta = mouse.abs - origin;
                        let message = format!("Drag delta: {:.0}, {:.0}", delta.x, delta.y);
                        self.ui.label(cx, ids!(drag_value)).set_text(cx, &message);
                        self.set_status(cx, &message);
                    }
                }
                _ => {}
            }
        }

        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
