use makepad_widgets::*;


live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    App = {{App}} {
        ui: <DesktopWindow> {
            
            show_bg: true
            
                flow: Down,
                spacing: 20,
                align: {
                    x: 0.0,
                    y: 0.0
                
            },
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(#7, #3, self.pos.y);
                }
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

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {
            return self.ui.draw_widget_all(&mut Cx2d::new(cx, event));
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        if self.ui.button(id!(button1)).clicked(&actions) {
        }
    }
}
