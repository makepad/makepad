//! Specimen page for the layout footprint of a `Modal`.
//!
//! A modal paints over the whole pass, on its own overlay draw list and its
//! own root turtle. It is therefore *not* laid out by whatever parent happens
//! to hold it, and must claim no space there — open or closed.
//!
//! Two identical columns stand side by side. Both are `flow: Down` with a
//! fixed header, a `height: Fill` body, and a fixed footer; the right-hand one
//! additionally parks three `Modal`s between its body and its footer, exactly
//! the way an app keeps its dialogs next to the page they belong to. The left
//! column is the control: the two bodies must measure the same.
//!
//! The bug this pins down: a `Fill` child of a `flow: Down` parent is a
//! *deferred fill*, and the parent hands every deferred fill an equal share of
//! the column's spare height at resolve time — whether or not the child then
//! draws anything at all. A `Modal` that reported `Fill`/`Fill` upward was
//! such a child, so three closed modals beside one real `Fill` body split the
//! spare height four ways. The body got a quarter, and anything below it in
//! its own subtree was laid out with what was left — which could be negative,
//! in which case it never drew at all.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Header = SolidView{
        width: Fill
        height: 40
        draw_bg +: { color: #x2a3350 }
    }

    let Footer = SolidView{
        width: Fill
        height: 40
        draw_bg +: { color: #x503030 }
    }

    let Body = SolidView{
        width: Fill
        height: Fill
        draw_bg +: { color: #x203020 }
    }

    let DialogCard = RoundedView{
        width: 220
        height: Fit
        flow: Down
        padding: 20
        spacing: 12
        draw_bg +: {
            color: #x16161b
            border_color: #xffffff30
            border_size: 1.0
            border_radius: 6.0
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Modal footprint"
                window.inner_size: vec2(520, 400)
                body +: {
                    View{
                        width: Fill
                        height: Fill
                        flow: Right
                        spacing: 10
                        padding: 10

                        // ---- control: the same column, no modals ----
                        plain_column := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            plain_header := Header{}
                            plain_body := Body{}
                            plain_footer := Footer{}
                        }

                        // ---- specimen: three modals parked in the column ----
                        modal_column := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            modal_header := Header{}
                            modal_body := Body{
                                flow: Down
                                align: Align{x: 0.5, y: 0.5}
                                open_button := Button{text: "Open"}
                            }
                            dialog_a := Modal{
                                content +: {
                                    DialogCard{
                                        Label{text: "DIALOG A"}
                                        close_a := Button{text: "Close A"}
                                    }
                                }
                            }
                            dialog_b := Modal{
                                content +: {
                                    DialogCard{
                                        Label{text: "DIALOG B"}
                                    }
                                }
                            }
                            dialog_c := Modal{
                                content +: {
                                    DialogCard{
                                        Label{text: "DIALOG C"}
                                    }
                                }
                            }
                            modal_footer := Footer{}
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

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(open_button)).clicked(actions) {
            self.ui.modal(cx, ids!(dialog_a)).open(cx);
        }
        if self.ui.button(cx, ids!(close_a)).clicked(actions) {
            self.ui.modal(cx, ids!(dialog_a)).close(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
