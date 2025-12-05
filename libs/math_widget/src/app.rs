use makepad_widgets::*;

live_design!{
    link widgets;
    use link::widgets::*;

    App = {{App}} {
        ui: <Root> {
            main_window = <Window> {
                body = <View> {
                    width: Fill,
                    height: Fill,
                    flow: Down,
                    align: {x: 0.5, y: 0.5},
                    <View> {
                        width: 200.0,
                        height: 150.0,
                        align: {
                            x: 0.5,
                            y: 0.5,
                        }
                        show_bg: true,
                        draw_bg: {
                            color: #000
                        },
                        <Math> {
                            width: Fit,
                            height: Fit,
                            text: "x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}"
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
        makepad_widgets::live_design(cx);
        crate::math::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}