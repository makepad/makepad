
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
                        fn gradient(self)->vec4{
                            let r = length(self.pos - vec2(0.5));
                            return mix(#f00, #00f, r);
                        }
                        fn pixel(self)->vec4{
                            return self.gradient();
                        }
                    }
                    button1 = <Button> {
                        text: "Button 1"
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

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, _cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions){
            let mut a = 0;
            let mut b = 1;
            for _i in 0..10{
                let c = a + b;
                a = b;
                b = c;
                println!("{}", b);
            }
        }
    }
}
