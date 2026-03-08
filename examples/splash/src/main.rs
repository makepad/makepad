pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

live_design!{
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_widgets::base::*;
    
    
    App = {{App}} {
        ui: <Root> {
            main_window = <Window> {
                body = <View> {
                    width: Fill,
                    height: Fill
                    
                    show_bg: true
                    draw_bg: {
                        instance line_a: 0.0
                        instance line_b: 0.0
                        
                        pixel(self) -> vec4 {
                            let p = self.pos;
                            let c = 0.5 + 0.5 * sin(self.time * 1.0 + p.yx * 10.0);
                            let color = mix(#f04, #0af, c.x);
                            return Pal::premul(vec4(color.rgb, 1.0))
                        }
                    }
                }
            }
        }
    }
}

impl LiveHook for App {
    fn after_new_from_doc(&mut self, _cx: &mut Cx) {
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {
        
    }
}
