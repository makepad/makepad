
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
        
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <View> {
                    align: {
                        x: 0.5,
                        y: 0.5,
                    },
                    <Button> {
                        width: 256,
                        height: 256,
                        draw_bg: {
                            fn compute_erf7(x: float) -> float {
                                let x = x * 1.128379;
                                let xx = x * x;
                                let x = x + (0.24295 + (0.03395 + 0.0104 * xx) * xx) * (xx * x);
                                return x / sqrt(x * x + 1);
                            }

                            fn hypot(x: float, y: float) -> float {
                                return sqrt(x * x + y * y);
                            }

                            fn pixel(self) -> vec4 {
                                let p = self.pos * self.rect_size - 128;
                                let b = vec2(128, 64);
                                let r = 16;
                                let s = 16;
                                let r_max = 0.5 * min(b.x, b.y);
                                let r0 = min(hypot(r, 1.15 * s), r_max);
                                let r1 = min(hypot(r, 2 * s), r_max);
                                let exp = 2 * r1 / r0;
                                let s_inv = 1 / s;
                                let k = 0.5 * compute_erf7(s_inv * 0.5 * (max(b.x, b.y) - 0.5 * r));
                                let p0 = abs(p) - 0.5 * b + r1;
                                let p1 = max(p0, 0);
                                let d_neg = min(max(p0.x, p0.y), 0);
                                let d_pos = pow(pow(p1.x, exp) + pow(p1.y, exp), 1 / exp);
                                let d = d_neg + d_pos - r1;
                                let z = k * (compute_erf7(s_inv * (min(b.x, b.y) + d)) - compute_erf7(s_inv * d));
                                return vec4(z, z, z, 1);
                            }
                        }
                        draw_text: {
                            color: #F00,
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