use makepad_widgets::*;

// Below is an application using the SDF api to make an icon with 2 overlaid filled boxes
live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down
                    show_bg: true,
                    draw_bg:{
                        // this shader syntax is NOT Rust code but comparable to GLSL
                        fn pixel(self)->vec4{
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                            sdf.box(
                                0.,
                                0.,
                                self.rect_size.x,
                                self.rect_size.y,
                                2.
                            );
                            sdf.fill(#f3)
                            sdf.box(
                                10.,
                                10.,
                                50.,
                                50.,
                                3.
                            );
                            sdf.fill(#f00);
                            return sdf.result;
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
    fn handle_actions(&mut self, _cx: &mut Cx, actions:&Actions){
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}