use crate::makepad_widgets::*;

//#[cfg(feature = "nightly")]
 
live_design!{ 
    use link::widgets::*;
    use link::theme::*;
    
    use crate::mandelbrot::Mandelbrot;
    App = {{App}} {
        ui: <Root>{
            <Window> {
                draw_bg:{color:#f00}
                body = <Mandelbrot> {
                    width: Fill, height: Fill
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
        crate::mandelbrot::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
