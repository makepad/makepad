
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

live_design!{
    use makepad_draw::shader::std::*;
    use makepad_widgets::base::*;
    use makepad_widgets::theme_desktop_dark::*;
    
    Integration = {{Integration}}{
        height: Fill, width: Fill
        <View> {
            width: Fill,
            height: Fill,
            flow: Down,
            spacing: 10.0,
    
            github_token_input = <TextInput> {
                width: Fill,
                height: Fit,
                margin: {left: 10.0, right: 10.0, top: 10.0},
                empty_message: "Enter GitHub API Token",
            }
    
            <View> {
                width: Fill,
                height: Fit,
                flow: Right,
                margin: {left: 10.0, right: 10.0},
    
                run_button = <Button> {
                    text: "Run",
                    width: Fit,
                    height: Fit,
                    margin: {right: 10.0},
                }
    
                observe_checkbox = <CheckBox> {
                    text: "Observe",
                    width: Fit,
                    height: Fit,
                }
            }
    
            output_log = <TextInput> {
                width: Fill,
                height: Fill,
                margin: {left: 10.0, right: 10.0, bottom: 10.0},
                empty_message: "Output Log",
                multiline: true,
                read_only: true,
            }
        }
    }
}


#[derive(Live, LiveHook, Widget)]
struct Integration{
    #[deref] view:View,
}

impl WidgetMatchEvent for Integration {
    fn handle_actions(&mut self, _cx: &mut Cx, actions: &Actions, _scope: &mut Scope){
        if self.view.button(id!(run_button)).clicked(actions){
            println!("run button clicked");
            // Handle the run button click event
            // You can add your custom code here to respond to the button click
            // For example, you might initiate some process or update the UI
        }
    }
}

impl Widget for Integration {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        self.view.draw_walk_all(cx, scope, walk);
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.widget_match_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }
}
