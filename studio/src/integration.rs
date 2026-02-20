use {
    crate::{
        //app::AppData,
        makepad_widgets::*,
    },
    std::{
        //fmt::Write,
        env,
    },
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.IntegrationBase = #(Integration::register_widget(vm))

    mod.widgets.Integration = set_type_default() do mod.widgets.IntegrationBase {
        height: Fill
        width: Fill
        View {
            width: Fill
            height: Fill
            flow: Down
            spacing: 10.0

            github_token_input := TextInput {
                width: Fill
                height: Fit
                margin: Inset{left: 10.0 right: 10.0 top: 10.0}
                empty_text: "Enter GitHub API Token"
            }

            View {
                width: Fill
                height: Fit
                flow: Right
                margin: Inset{left: 10.0 right: 10.0}

                run_button := Button {
                    text: "Run"
                    width: Fit
                    height: Fit
                    margin: Inset{right: 10.0}
                }

                observe_checkbox := CheckBox {
                    text: "Observe"
                    width: Fit
                    height: Fit
                }
            }

            output_log := TextInput {
                width: Fill
                height: Fill
                margin: Inset{left: 10.0 right: 10.0 bottom: 10.0}
                empty_text: "Output Log"
                is_read_only: true
                is_multiline: true
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
struct Integration {
    #[deref]
    view: View,
}

impl WidgetMatchEvent for Integration {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, _scope: &mut Scope) {
        if self.view.button(cx, ids!(run_button)).clicked(actions) {
            println!("run button clicked");
            // Handle the run button click event
            // You can add your custom code here to respond to the button click
            // For example, you might initiate some process or update the UI
        }
    }
}

impl Widget for Integration {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk_all(cx, scope, walk);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.widget_match_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }
}
