//! Specimen page for the vertical text-centering contract of boxed labels.
//!
//! Every widget on this page draws one line of text inside a box that the
//! widget itself owns (a button face, a tab, a dropdown, a fixed-height view).
//! The contract the `tests/ui.rs` suite pins down is: the *ink* of that line —
//! the cap-height band, from the top of a capital `H` down to the baseline —
//! is centered in the box, not the font's ascender/descender line box.
//!
//! The specimens are painted flat (no bevel, no gradient, no dither) and use
//! `H` as their label, because the test measures sub-pixel edges out of the
//! rendered PNG: a flat face gives clean box edges, and `H` has a flat cap top
//! and sits flat on the baseline, so its ink band *is* the cap band.
//! Nothing about the *geometry* is instrumented: walk, padding, align and
//! font size are the stock ones.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let FlatButton = Button{
        text: "H"
        width: 120

        draw_bg +: {
            border_size: 0.0
            color: #x606060
            color_hover: #x606060
            color_down: #x606060
            color_focus: #x606060
            color_disabled: #x606060
            color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            border_color: #x606060
            border_color_hover: #x606060
            border_color_down: #x606060
            border_color_focus: #x606060
            border_color_disabled: #x606060
            border_color_2: vec4(-1.0, -1.0, -1.0, -1.0)
        }

        draw_text +: {
            color: #xffffff
            color_hover: #xffffff
            color_down: #xffffff
            color_focus: #xffffff
            color_disabled: #xffffff
        }
    }

    let FlatDropDown = DropDown{
        labels: ["H"]
        width: 120

        draw_bg +: {
            border_size: 0.0
            color: #x606060
            color_hover: #x606060
            color_down: #x606060
            color_focus: #x606060
            color_disabled: #x606060
            color_2: vec4(-1.0, -1.0, -1.0, -1.0)
            border_color: #x606060
            border_color_hover: #x606060
            border_color_down: #x606060
            border_color_focus: #x606060
            border_color_disabled: #x606060
            border_color_2: vec4(-1.0, -1.0, -1.0, -1.0)
        }

        draw_text +: {
            color: #xffffff
            color_hover: #xffffff
            color_down: #xffffff
            color_focus: #xffffff
            color_disabled: #xffffff
        }
    }

    // Fixed row heights, so that everything below a row — the dock in
    // particular, whose tab the suite measures against the widget rect the app
    // reports, rounded to whole logical pixels — lands on a whole pixel.
    let Row = View{
        width: Fill
        height: 84
        flow: Right
        spacing: 24
        padding: Inset{left: 24, top: 12, right: 24, bottom: 12}
    }

    let DockBody = SolidView{
        width: Fill
        height: Fill
        draw_bg.color: #x202020
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Text centering specimens"
                window.inner_size: vec2(760, 560)
                pass.clear_color: #x000000
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Down
                        show_bg: true
                        draw_bg.color: #x000000

                        Row{
                            btn_08 := FlatButton{draw_text.text_style.font_size: 8}
                            btn_10 := FlatButton{}
                            btn_14 := FlatButton{draw_text.text_style.font_size: 14}
                        }

                        Row{
                            btn_24 := FlatButton{draw_text.text_style.font_size: 24}
                            btn_tall := FlatButton{height: 48}
                            btn_bold := FlatButton{draw_text.text_style: theme.font_bold{font_size: theme.font_size_p}}
                        }

                        Row{
                            // An icon and a label share this button's row, and
                            // the icon is the taller of the two, so the row —
                            // and with it the label's box — is as tall as the
                            // icon. The icon is painted in the face color so
                            // that only the label registers as ink: it takes
                            // up its space, it just doesn't get measured.
                            btn_icon := FlatButton{
                                width: 150
                                draw_icon +: {
                                    svg: crate_resource("self:resources/square.svg")
                                    color: #x606060
                                }
                            }
                            drop_10 := FlatDropDown{}
                            drop_14 := FlatDropDown{draw_text.text_style.font_size: 14}
                            label_box := SolidView{
                                width: 120
                                height: 48
                                align: Center
                                draw_bg.color: #x606060
                                boxed_label := Label{
                                    text: "H"
                                    draw_text.color: #xffffff
                                }
                            }
                        }

                        specimen_dock := Dock{
                            width: Fill
                            height: Fill
                            margin: Inset{left: 24, top: 12, right: 24, bottom: 24}

                            root := DockTabs{
                                tabs: [@tab_one, @tab_two]
                                selected: 0
                                closable: false
                            }

                            tab_one := DockTab{name: "H" template: @PermanentTab kind: @KindOne}
                            tab_two := DockTab{name: "Hxy" template: @PermanentTab kind: @KindTwo}

                            KindOne := DockBody{}
                            KindTwo := DockBody{}
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
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        // The light and dark desktop themes share every number that decides
        // vertical placement (space factor, font sizes, tab height, line
        // spacing); they differ only in color. The suite still renders both,
        // so a future divergence shows up as a failing assertion rather than
        // as a surprise in someone's app.
        if std::env::var("MAKEPAD_TEXT_CENTER_THEME").as_deref() == Ok("light") {
            crate::makepad_widgets::theme_mod(vm);
            script_eval!(vm, {
                mod.theme = mod.themes.light
            });
            crate::makepad_widgets::widgets_mod(vm);
        } else {
            crate::makepad_widgets::script_mod(vm);
        }
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
