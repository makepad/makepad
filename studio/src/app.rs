use {
    crate::{
        makepad_platform::*,
        makepad_draw::*,
        makepad_widgets::*,
        makepad_widgets::dock::*,
        app_inner::AppInner,
        app_state::AppState,
    },
    //makepad_regex::regex::Regex
};

live_design!{
    import makepad_widgets::theme::*;
    import makepad_widgets::dock::*;
    import makepad_widgets::desktop_window::DesktopWindow;
    const FS_ROOT = ""
    App = {{App}} {
        ui: <DesktopWindow> {
            caption_bar = {visible: true, caption_label = {label = {label: "Makepad Studio"}}},
            dock = <Dock> {
                walk: {height: Fill, width: Fill}
            }
        }
        inner: {
            collab_client: {
                //bind: "127.0.0.1"
                path: (FS_ROOT)
            }
            build_manager: {
                //bind: "127.0.0.1"
                path: (FS_ROOT)
            }
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[live] inner: AppInner,
    #[rust(AppState::new())] app_state: AppState,
}


impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::build::build_manager::live_design(cx);
        crate::collab_client::live_design(cx);
        crate::rust_editor::live_design(cx);
        crate::log_view::live_design(cx);
        crate::shader_view::live_design(cx);
        crate::run_view::live_design(cx);
        crate::code_editor::code_editor_impl::live_design(cx);
        crate::editors::live_design(cx);
        crate::app_inner::live_design(cx);
    }
}


app_main!(App);

impl App {
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let dock = self.ui.get_dock(id!(dock));
        match event {
            Event::Draw(event) => {
                let cx = &mut Cx2d::new(cx, event);
                while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                    if let Some(mut dock) = dock.has_widget(&next).borrow_mut() {
                        // lets draw the dock
                        self.inner.draw_panel(cx, &mut *dock, &self.app_state, live_id!(root).into());
                    }
                }
                return
            }
            _ => ()
        }
        self.ui.handle_widget_event(cx, event);
        self.inner.handle_event(cx, event, &mut *dock.borrow_mut().unwrap(), &mut self.app_state);
    }
}
