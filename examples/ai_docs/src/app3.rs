// this example application has only a vertical gradient background
use makepad_widgets::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*; 
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <ScrollXYView>{
                    flow: Down
                    // all containers can have a shader background with these options
                    show_bg: true,
                    draw_bg:{
                        // this shader syntax is NOT Rust code but comparable to GLSL. Do NOT write Rust code in these blocks
                        fn pixel(self)->vec4{
                            // this creates a red to green vertical gradient background
                            return mix(#f00,#0f0,self.pos.y)
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

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}