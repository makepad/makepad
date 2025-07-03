
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
        
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <View>{
                    flow: Right,
                    spacing: 10,
                    align: {
                        x: 0.5,
                        y: 0.5
                    },
                    <Button> {
                        width: Fill,
                        text: "AAA"
                        draw_text:{text_style:{font_size:18}}
                    }
                    <Button> {
                        width: WeightedFill(200.0),
                        text: "BBB"
                        draw_text:{text_style:{font_size:18}}
                    }
                    <Button> {
                        width: Fill,
                        text: "CCC"
                        draw_text:{text_style:{font_size:18}}
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
    fn handle_startup(&mut self, _cx:&mut Cx){
    }
        
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button_1)).clicked(&actions) {
            self.ui.button(id!(button_1)).set_text(cx, "Clicked 😀");
            log!("hi");
            self.counter += 1;
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::XrUpdate(_e) = event{
            //log!("{:?}", e.now.left.trigger.analog);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}