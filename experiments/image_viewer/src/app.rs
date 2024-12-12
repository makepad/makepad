use makepad_widgets::*;
use std::path::Path;

use crate::state::State;

live_design!(
    use makepad_widgets::base::*;
    use makepad_widgets::theme_desktop_dark::*;
    use crate::ui::Ui;

    App = {{App}} {
        ui: <Ui> {}
    }
);

#[derive(Live, LiveHook)]
struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    state: State,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, _cx: &mut Cx) {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .expect("home not found");

        self.state.load_images(&Path::new(&home).join("Downloads"));
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui
            .handle_event(cx, event, &mut Scope::with_data(&mut self.state));
        self.match_event(cx, event);
    }
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
        crate::ui::live_design(cx);
    }
}

app_main!(App);
