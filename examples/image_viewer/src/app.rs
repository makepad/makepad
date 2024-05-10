use makepad_widgets::*;
use std::path::Path;

use crate::state::State;

live_design!(
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import crate::ui::Ui;

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
        let state = &mut self.state;

        state.load_images(Path::new("/Users/wyeworks/Downloads"));

        if let Some(image) = state.images.first() {
            state.select_image(image.clone());
        }
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
