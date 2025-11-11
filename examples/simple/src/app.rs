
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
        
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                window: {title: "你好，こんにちは, Привет, Hello"},
                body = <View> {
                    padding: 100,
                    <View> {
                        width: 300,
                        height: 750,
                        flow: Right {
                            row_align: Bottom,
                            wrap: true,
                        },
                        show_bg: true,
                        draw_bg: {
                            color: #888
                        }
                        <Button> {
                            margin: 0.0,
                            width: 100,
                            height: 100,
                            metrics: {
                                descender: 50,
                            }
                        }
                        <Button> {
                            margin: 0.0,
                            width: 200,
                            height: 200,
                        }
                        <Button> {
                            margin: 0.0,
                            width: 200,
                            height: 200,
                            metrics: {
                                descender: 100.0,
                                line_scale: 1.1,
                            }
                        }
                        <Button> {
                            margin: 0.0,
                            width: 100,
                            height: 100,
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
        if self.ui.button(ids!(button_1)).clicked(&actions) {
            self.ui.button(ids!(button_1)).set_text(cx, "Clicked 😀");
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