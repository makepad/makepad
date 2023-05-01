use crate::makepad_widgets::*;

//#[cfg(feature = "nightly")]

live_design!{
    import makepad_widgets::frame::*;
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_example_fractal_zoom::mandelbrot::Mandelbrot;
    App = {{App}} {
        ui: <DesktopWindow> {
            frame: {body = {
                <Mandelbrot> {
                    walk: {width: Fill, height: Fill}
                }
            }}
        }
    }
}
app_main!(App);

#[derive(Live, LiveHook)]
#[live_design_with {
    crate::makepad_widgets::live_design(cx);
    crate::mandelbrot::live_design(cx);
}]
pub struct App {
    ui: WidgetRef,
}

impl AppMain for App {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            self.ui.draw_widget(&mut Cx2d::new(cx, event));
        }
        
        self.ui.handle_widget_event(cx, event);
    }
}
