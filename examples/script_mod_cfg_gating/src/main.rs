//! Demonstrates Cargo-feature gating inside a single `script_mod!` block.
//!
//! Run variants:
//!
//!     cargo run -p makepad-example-script-mod-cfg-gating
//!         baseline UI only
//!
//!     cargo run -p makepad-example-script-mod-cfg-gating --features=pro
//!         adds the PRO panel inline (single-statement form)
//!
//!     cargo run -p makepad-example-script-mod-cfg-gating --features=debug_panel
//!         adds the debug-stats panel (brace-grouped form)
//!
//!     cargo run -p makepad-example-script-mod-cfg-gating --features=pro,debug_panel
//!         both
//!
//! The DSL inside `script_mod!` is the same in every build — Cargo features
//! select which fragments rustc compiles. `cfg_fragments` on the resulting
//! `ScriptMod` records the per-fragment selection so hot-reload can mirror it.

pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let state = {
        counter: 0
        clicks: 0
    }
    mod.state = state

    // The `#[cfg(...)]` attributes below pick which DSL fragments contribute to
    // this `script_mod!` body. The selected `code` string is what the VM sees
    // at startup; everything else is dropped at compile time.

    #[cfg(feature = "pro")]
    let pro_label_text = "PRO build — extra controls below"

    #[cfg(not(feature = "pro"))]
    let pro_label_text = "Standard build"

    startup() do #(App::script_component(vm)) {
        ui: Root {
            on_startup: || {
                ui.main_view.render()
            }
            main_window := Window {
                window.inner_size: vec2(520, 360)
                body +: {
                    main_view := View {
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 12
                        align: Center
                        padding: Inset { left: 16.0 right: 16.0 top: 16.0 bottom: 16.0 }
                        on_render: || {
                            mode_label := Label {
                                text: pro_label_text
                                draw_text.text_style.font_size: 18
                            }
                            counter_label := Label {
                                text: "Count: " + state.counter
                                draw_text.text_style.font_size: 28
                            }
                            increment_button := Button {
                                text: "Increment"
                            }

                            // Single-statement form: the whole `pro_button := Button {...}`
                            // statement is included only when `--features=pro` is on.
                            #[cfg(feature = "pro")]
                            pro_button := Button { text: "PRO action (×5)" }

                            // Brace-grouped form: any number of statements can
                            // be gated together. Outer braces are stripped at
                            // expansion time so the DSL parser sees a clean
                            // statement list.
                            #[cfg(feature = "debug_panel")] {
                                debug_divider := Label {
                                    text: "— debug stats —"
                                    draw_text.text_style.font_size: 12
                                }
                                debug_panel := View {
                                    width: Fill
                                    height: Fit
                                    flow: Down
                                    spacing: 4
                                    align: Center
                                    debug_clicks := Label {
                                        text: "Clicks: " + state.clicks
                                        draw_text.text_style.font_size: 14
                                    }
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
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(increment_button)).clicked(actions) {
            script_eval!(cx, {
                mod.state.counter += 1
                mod.state.clicks += 1
                ui.main_view.render()
            });
        }

        // The PRO button only exists when `--features=pro` is on. Guard the
        // event handler with the same cfg so the binary doesn't try to look up
        // an id that never got registered.
        #[cfg(feature = "pro")]
        {
            if self.ui.button(cx, ids!(pro_button)).clicked(actions) {
                script_eval!(cx, {
                    mod.state.counter += 5
                    mod.state.clicks += 1
                    ui.main_view.render()
                });
            }
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
