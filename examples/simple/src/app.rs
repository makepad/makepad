
use makepad_widgets::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down
                    show_bg: true,
                    draw_bg:{
                        fn plasma(self, pos: vec2) -> vec4 {
                            let color = sin(pos.x * 10.0 + self.time) * cos(pos.y * 10.0 + self.time);
                            return mix(#f0f, #f80, color);
                        }
                        fn pixel(self) -> vec4 {
                            return self.plasma(self.pos);
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
        if self.ui.button(id!(button1)).clicked(&actions){
           println!("Button 1 clicked")
        }
        if self.ui.button(id!(button2)).clicked(&actions){
           println!("Button 2 clicked")
        }
        if self.ui.button(id!(button3)).clicked(&actions){
           println!("Button 3 clicked")
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}