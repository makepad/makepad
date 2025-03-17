
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
                            return vec4(0.0, 0.0, 0.0, 1.0); // mix(color1, color2, radius);
                        }
                    }
                    /*
                    b0= <Button2> {
                        text: "😊不对😭不对😊"
                        draw_text:{color:#fff, text_style: { font_size: 24.0 } }
                    }
                    button1 = <Button2> {
                        text: "Click me 234"
                        draw_text:{color:#fff}
                    }
                    button2 = <Button2> {
                        text: "Click me 345"
                        draw_text:{color:#fff}
                    }
                    */
                    text_input = <TextInput2> {
                        text: "Averylongwordtodemonstratedesperatebreaks The 😊错误 quick\n\nLet's force a new line\n brown fox😊 jumped over the lazy dog"
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
    fn handle_actions(&mut self, _cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions) {
            self.counter += 1;
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
