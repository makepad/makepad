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
                        width: 400.0,
                        height: 100.0,
                        align: {
                            x: 0.5,
                            y: 0.5,
                        }
                        show_bg: true,
                        draw_bg: {
                            color: #f00
                        },
                        <Math> {
                            width: Fit,
                            height: Fit,
                            text: "$ x = (-b +/- sqrt(b^2 - 4 a c)) / 2 a $"
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
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}