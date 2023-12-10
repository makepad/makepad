use crate::makepad_widgets::*;

//#[cfg(feature = "nightly")]
 
live_design!{ 
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    import crate::mandelbrot::Mandelbrot;
    App = {{App}} {
        ui: <Window> {
            draw_bg:{color:#f00}
            body = <Mandelbrot> {
                width: Fill, height: Fill
            }
        } 
    }
}
app_main!(App);

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::mandelbrot::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event_no_scope(cx, event);
    }
}
