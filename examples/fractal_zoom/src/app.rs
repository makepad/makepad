use crate::makepad_widgets::*;

//#[cfg(feature = "nightly")]

live_design!{
    import makepad_widgets::frame::*;
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_example_fractal_zoom::mandelbrot::Mandelbrot;
    App = {{App}} {
        ui: <DesktopWindow> {
            <Mandelbrot> {
                walk: {width: Fill, height: Fill}
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
        if let Event::Draw(event) = event {
            self.ui.draw_widget_all(&mut Cx2d::new(cx, event));
            return
        }
        
        self.ui.handle_widget_event(cx, event);
    }
}
