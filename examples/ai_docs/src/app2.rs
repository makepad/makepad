// this example application has 2 buttons and a slider
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down // vertical stacked
                    <View>{ 
                        width: Fit // content sized
                        height: Fit
                        flow: Right, // horizontally stacked
                        spacing: 10.0 // spacing between items
                        button1 = <Button> {
                            text: "Button 1"
                            // this sets the text color of the button to red
                            draw_text:{color:#f00}
                        }
                        button2 = <Button> {
                            text: "Button 2"
                        }
                    }
                    <View>{ 
                        width: Fill // fill the parent container
                        height: 100 // fixed width
                        flow: Down, // vertical stacked
                        slider1 = <Slider> {
                            text: "Slider 1"
                            min: 0
                            max: 100
                            step: 0.1
                        }
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
}
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}
impl MatchEvent for App{
    fn handle_actions(&mut self, _cx: &mut Cx, actions:&Actions){
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}