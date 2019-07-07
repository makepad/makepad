//use syn::Type;
use render::*;
use widget::*;
mod style;

mod graph;
use crate::graph::Graph;

use std::collections::HashMap;
use serde::*;

#[derive(Clone)]
struct AppWindow {
    desktop_window: DesktopWindow,
    graph: Graph,
}

struct AppGlobal {
    state: AppState,
    index_file_read: FileRead,
    app_state_file_read: FileRead,
}

struct App {
    app_window_state_template: AppWindowState,
    app_window_template: AppWindow,
    app_global: AppGlobal,
    windows: Vec<AppWindow>,
}

#[derive(Clone, Serialize, Deserialize)]
struct AppWindowState {
    window_position: Vec2,
    window_inner_size: Vec2,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct AppState {
    windows: Vec<AppWindowState>
}

main_app!(App, "HALLO WORLD");

impl Style for AppWindow {
    fn style(cx: &mut Cx) -> Self {
        Self {
            desktop_window: DesktopWindow::style(cx),
            graph: Graph{
                ..Style::style(cx)
            }
        }
    }
}

impl Style for App {

    fn style(cx: &mut Cx) -> Self {
        style::set_experiment_style(cx);
        Self {
            app_window_template: AppWindow::style(cx),
            app_window_state_template: AppWindowState {
                window_inner_size: Vec2::zero(),
                window_position: Vec2::zero(),
            },
            windows: vec![],
            app_global: AppGlobal {
                index_file_read: FileRead::default(),
                app_state_file_read: FileRead::default(),
                state: AppState::default()
            }
        }
    }
}

impl AppWindow {
    fn handle_app_window(&mut self, cx: &mut Cx, event: &mut Event, window_index: usize, app_global: &mut AppGlobal) {

        match self.desktop_window.handle_desktop_window(cx, event) {
            DesktopWindowEvent::EventForOtherWindow => {
                return
            },
            DesktopWindowEvent::WindowClosed => {
                return
            },
            DesktopWindowEvent::WindowGeomChange(wc) => {
                println!("new pos {:?}", wc.new_geom.position);
                if !app_global.app_state_file_read.is_pending() {
                    // store our new window geom
                    app_global.state.windows[window_index].window_position = wc.new_geom.position;
                    app_global.state.windows[window_index].window_inner_size = wc.new_geom.inner_size;
                    app_global.save_state(cx);
                }
            },
            _ => ()
        }

        self.graph.handle_graph(cx, event);
    }

    fn draw_app_window(&mut self, cx: &mut Cx, window_index: usize, app_global: &mut AppGlobal) {
        if let Err(()) = self.desktop_window.begin_desktop_window(cx) {
            return
        }

        // self.dock.draw_dock(cx);
        self.graph.draw_graph(cx);
        self.desktop_window.end_desktop_window(cx);
    }
}

impl AppGlobal {
    fn handle_construct(&mut self, cx: &mut Cx) {
    }

    fn save_state(&mut self, cx: &mut Cx) {
        println!("SAVE STATE");
        let json = serde_json::to_string(&self.state).unwrap();
        cx.file_write(&format!("{}experiment_state.json", "./".to_string()), json.as_bytes());
    }
}

impl App {
    fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                self.app_global.handle_construct(cx);
                self.app_global.app_state_file_read = cx.file_read(&format!("{}experiment_state.json", "./".to_string()));
            },
            Event::FileRead(fr) => {
                if let Some(utf8_data) = self.app_global.app_state_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        if let Ok(state) = serde_json::from_str(&utf8_data) {
                            self.app_global.state = state;

                            // create our windows with the serialized positions/size
                            for window_state in &self.app_global.state.windows {
                                let mut size = window_state.window_inner_size;

                                if size.x <= 10. {
                                    size.x = 800.;
                                }
                                if size.y <= 10. {
                                    size.y = 600.;
                                }
                                let last_pos = window_state.window_position;
                                let create_pos = Some(last_pos);
                                self.windows.push(AppWindow {
                                    desktop_window: DesktopWindow {window: Window {
                                        create_inner_size: Some(size),
                                        create_position: create_pos,
                                        ..Style::style(cx)
                                    }, ..Style::style(cx)},
                                    ..self.app_window_template.clone()
                                })
                            }
                            cx.redraw_child_area(Area::All);
                        }
                    }
                    else { // load default window
                        println!("DOING DEFAULT");
                        self.app_global.state.windows = vec![self.app_window_state_template.clone()];
                        self.windows = vec![self.app_window_template.clone()];

                        cx.redraw_child_area(Area::All);
                    }
                }
            },
            _ => ()
        }

        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.handle_app_window(cx, event, window_index, &mut self.app_global);
        }
    }

    fn draw_app(&mut self, cx: &mut Cx) {
        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.draw_app_window(cx, window_index, &mut self.app_global);
        }
    }
}
