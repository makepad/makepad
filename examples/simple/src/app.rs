
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <View>{
                    flow: Down,
                    spacing: 10,
                    align: {
                        x: 0.5,
                        y: 0.5
                    },
                    show_bg: true,
                    draw_bg:{
                        fn pixel(self) -> vec4 {
                            
                            let center = vec2(0.5, 0.5);
                            let uv = self.pos - center;
                            let radius = length(uv);
                            let angle = atan(uv.y, uv.x);
                            let color1 = mix(#f00, #00f, 0.5 + 10.5 * cos(angle + self.time));
                            let color2 = mix(#0f0, #ff0, 0.5 + 0.5 * sin(angle + self.time));
                            return mix(color1, color2, radius);
                        }
                    }
                    button_1 = <Button> {
                        text: "Click me 😊"
                        draw_text:{color:#fff, text_style:{font_size:28}}
                    }
                    text_input = <TextInput> {
                        width: 100,
                        flow: RightWrap,
                        text: "Lorem ipsum"
                        draw_text:{color:#fff, text_style:{font_size:28}}
                    }
                    button_2 = <Button> {
                        text: "Click me 345 1234"
                        draw_text:{color:#fff, text_style:{font_size:28}}
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
    fn handle_startup(&mut self, cx:&mut Cx){
        cx.start_timeout(0.1);
    }
    
    fn handle_timer(&mut self, cx:&mut Cx, _te:&TimerEvent){
       cx.switch_to_xr();
    }
    
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(b0)).clicked(&actions) {
            self.counter += 1;
            cx.switch_to_xr();
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