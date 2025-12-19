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
                        width: 200,
                        height: 200,
                        show_bg: true
                        draw_bg: {
                            color: #000
                        },
                        <Svg> {
                            width: Fit,
                            height: Fit,
                            text: r#"
                                <svg width='200' height='200' xmlns='http://www.w3.org/2000/svg'>
                                    <circle cx='100' cy='100' r='100' fill='#ff0000' />
                                    <rect x='50' y='50' width='100' height='100' fill='#0000ff'/>
                                    <text x='100' y='100' font-size='32' text-anchor='middle' dominant-baseline='middle' fill='#ffffff'>Hello, world!</text>
                                </svg>
                            "#
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
        crate::svg::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
