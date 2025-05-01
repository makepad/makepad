use makepad_widgets::*;

live_design! {
    use link::widgets::*;

    PLACEHOLDER = dep("crate://self/resources/placeholder.jpg");

    ImageItem = <View> {
        width: 256,
        height: 256,

        image = <Image> {
            width: Fill,
            height: Fill,
            fit: Biggest,
            source: (PLACEHOLDER)
        }
    }

    App = {{App}} {
        ui: <Root> {
            <Window> {
                body = <View> {
                    <ImageItem> {}
                }
            }
        }
    }
}

#[derive(Live, LiveHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

app_main!(App);
