use makepad_widgets::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    // TEST = dep("crate://self/resources/test.mp4")
    TEST2 = dep("crate://self/resources/test2.mp4")
    // TEST3 = dep("crate://self/resources/test3.mp4")

    App = {{App}} {
        ui: <DesktopWindow>{
            window: { inner_size: vec2(564, 945) },
            show_bg: true

            flow: Down,
            spacing: 20,
            align: { x: 0.5, y: 0.5 }

            width: Fill,
            height: Fill

            draw_bg: { color: #2 }
            
            <Video> {
                source: (TEST2)
                width: 400, height: 709.5
                is_looping: true
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
    }
}

impl AppMain for App{
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {   
        if let Event::Draw(event) = event {
            return self.ui.draw_widget_all(&mut Cx2d::new(cx, event));
        }
        
        self.ui.handle_widget_event(cx, event);
    }
}