
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
                        fn pixel(self)->vec4{
                            return mix(#a,#6,self.pos.y)
                        }
                    }
                    button1 = <Button> {
                        text: "Button 1"
                    }
                    button2 = <Button> {
                        text: "Button 2"
                    }
                    button3 = <Button> {
                        text: "Button 3"
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
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl MatchEvent for App{
    fn handle_actions(&mut self, _cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions){
           
        }
        if self.ui.button(id!(button2)).clicked(&actions){

        }
        if self.ui.button(id!(button3)).clicked(&actions){
           
        }
    }
} 
