use makepad_widgets::*;

live_design!(
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    Ui = {{Ui}} {
        align: {x: 0.5, y: 0.5}
        body = <View> {
            flow: Overlay,
            <Label> {
                margin: {top: 80, left: 20}
                text: "Hi, I'm the content behind!",
            }
            <TogglePanel> {
                open_content = {
                    <View> {
                        align: {x: 0.5, y: 0.5},
                        show_bg: true,
                        draw_bg: {
                            fn pixel() -> vec4 {
                                return #a22;
                            }
                        }
                        <Label> {
                            text: "There is content behind me...",
                        }
                    }
                }
            }
        }
    }
);

#[derive(Live, LiveHook, Widget)]
pub struct Ui {
    #[deref]
    deref: Window,
}

impl Widget for Ui {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.deref.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.deref.draw_walk(cx, scope, walk)
    }
}
