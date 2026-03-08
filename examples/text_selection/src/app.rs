use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let app = startup() do #(App::script_component(vm)) {
        ui: Root {
            main_window := Window {
                window.inner_size: vec2(900, 860)
                body +: {
                    ScrollYView {
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 14
                        padding: 20

                        Label {
                            draw_text.text_style.font_size: 16.0
                            text: "Text Selection Demo"
                        }

                        Label {
                            draw_text.text_style.font_size: 10.0
                            draw_text.color: #888
                            text: "This demo shows selectable text, selectable input text, and non-selectable text."
                        }

                        RoundedView {
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 8
                            padding: Inset { top: 14, bottom: 14, left: 14, right: 14 }
                            draw_bg.color: #243246
                            draw_bg.radius: 8.0

                            Label {
                                draw_text.text_style.font_size: 12.0
                                text: "1) Selectable labels/text"
                            }

                            Label {
                                draw_text.text_style.font_size: 10.0
                                draw_text.color: #9ab
                                text: "Drag to select the text below."
                            }

                            selectable_text := Markdown {
                                width: Fill
                                height: Fit
                                selectable: true
                                body: "Selectable text block. Try long-press and drag on mobile, or drag with mouse on desktop.\n\nThis paragraph is intended to verify selection handles and drag updates on touch platforms."
                            }
                        }

                        RoundedView {
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 8
                            padding: Inset { top: 14, bottom: 14, left: 14, right: 14 }
                            draw_bg.color: #243246
                            draw_bg.radius: 8.0

                            Label {
                                draw_text.text_style.font_size: 12.0
                                text: "2) Selectable input text"
                            }

                            Label {
                                draw_text.text_style.font_size: 10.0
                                draw_text.color: #9ab
                                text: "Type text in the input, then select part of it."
                            }

                            input_text := TextInput {
                                width: Fill
                                height: 130
                                is_multiline: true
                                empty_text: "Type here and test text selection..."
                            }
                        }

                        RoundedView {
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 8
                            padding: Inset { top: 14, bottom: 14, left: 14, right: 14 }
                            draw_bg.color: #243246
                            draw_bg.radius: 8.0

                            Label {
                                draw_text.text_style.font_size: 12.0
                                text: "3) Non-selectable labels/text"
                            }

                            non_selectable_label := Label {
                                text: "This Label is non-selectable."
                            }

                            non_selectable_text := Markdown {
                                width: Fill
                                height: Fit
                                selectable: false
                                body: "This Markdown block is explicitly non-selectable.\n\nDragging here should not create a text selection."
                            }
                        }
                    }
                }
            }
        }
    }
    app
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
