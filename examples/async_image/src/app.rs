
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
                    image = <Image> {
                        width: 280., height: 180.,
                        source: dep("crate://self/resources/placeholder.jpg")
                    }
                    async_load= <Button> {
                        text: "Click me 123"
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
 }
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(async_load)).clicked(&actions) {
            // lets async load a jpeg
            self.ui.image(id!(image)).load_image_file_by_path_async(cx, std::path::Path::new("./examples/async_image/resources/placeholder.jpg")).unwrap();
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
