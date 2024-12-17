
use std::time::Instant;

use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
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
                    button1 = <Button> {
                        text: "Click me 123"
                        draw_text:{color:#fff}
                    }
                    button2 = <Button> {
                        text: "Click me 345"
                        draw_text:{color:#fff}
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
    #[rust] timer: Timer,
    #[rust] timer1: Timer,
    #[rust] timer2: Timer,
    #[rust] 
    time_elapsed: Option<Instant>,

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
            // self.timer = cx.start_timeout(3.0);
            // self.timer1 = cx.start_timeout(2.0);
            // self.timer2 = cx.start_timeout(3.0);
            self.timer = cx.start_interval(1.0);
            self.time_elapsed = Some(Instant::now());
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        if self.timer.is_event(event).is_some() {
            log!("this timer run correct! ");
            log!("Timer took {:?} seconds", self.time_elapsed.unwrap().elapsed());
        }

        if self.timer1.is_event(event).is_some() {
            log!("this timer111 run correct! ");
            log!("Timer took {:?} seconds", self.time_elapsed.unwrap().elapsed());
        }

        if self.timer2.is_event(event).is_some() {
            log!("this timer222 run correct! ");
            log!("Timer took {:?} seconds", self.time_elapsed.unwrap().elapsed());
        }
    }
}
