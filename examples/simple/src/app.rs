use makepad_widgets::*;
        
live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
    
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down,
                    spacing:10,
                    align: {
                        x: 0.5,
                        y: 0.5
                    },
                    button1 = <Button> {
                        text: "Hello world "
                        draw_text:{color:#f00}
                    }
                    input1 = <TextInput> {
                        width: 100
                        text: "Click to count"
                    }
                    label1 = <Label> {
                        draw_text: {
                            color: #f
                        },
                        text: r#"Lorem ipsum dolor sit amet"#,
                        width: 200.0,
                    }
                }
            }
        }
    }
}  
              
app_main!(App); 
 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
 }
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions) {
            self.counter += 1;
            let label = self.ui.label(id!(label1));
            label.set_text_and_redraw(cx, &format!("Counter: {} Time:{}", self.counter, Cx::time_now()));
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}