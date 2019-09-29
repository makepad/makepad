//use syn::Type;
use render::*;
use widget::*;
use editor::*;
use crate::rustcompiler::*;
use crate::appwindow::*;
use std::collections::HashMap;
use serde::*;
use workspacelib::*;

pub struct AppGlobal {
    pub file_tree_data: String,
    pub file_tree_reload_signal: Signal,
    pub text_buffers: TextBuffers,
    pub rust_compiler: RustCompiler,
    pub state: AppState,
    pub index_file_read: FileRead,
    pub app_state_file_read: FileRead,
}

pub struct App {
    pub hub_server: Option<HubServer>,
    pub hub_client: Option<HubClient>,
    pub app_window_state_template: AppWindowState,
    pub app_window_template: AppWindow,
    pub app_global: AppGlobal,
    pub windows: Vec<AppWindow>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub windows: Vec<AppWindowState>
}


impl AppGlobal {
    pub fn handle_construct(&mut self, cx: &mut Cx) {
        if cx.platform_type.is_desktop() {
            self.text_buffers.root_path = "./edit_repo/".to_string();
        }
        
        self.rust_compiler.init(cx, &mut self.text_buffers);
    }
    
    pub fn save_state(&mut self, cx: &mut Cx) {
        let json = serde_json::to_string(&self.state).unwrap();
        cx.file_write(&format!("{}makepad_state.json", self.text_buffers.root_path), json.as_bytes());
    }
}

impl App {
    
    pub fn style(cx: &mut Cx) -> Self {
        set_dark_style(cx);
        Self {
            hub_server: None,
            hub_client: None,
            app_window_template: AppWindow::style(cx),
            app_window_state_template: AppWindowState {
                window_inner_size: Vec2::zero(),
                window_position: Vec2::zero(),
                dock_items: DockItem::Splitter {
                    axis: Axis::Vertical,
                    align: SplitterAlign::First,
                    pos: 150.0,
                    first: Box::new(DockItem::TabControl {
                        current: 0,
                        tabs: vec![DockTab {
                            closeable: false,
                            title: "Files".to_string(),
                            item: Panel::FileTree
                        }]
                    }),
                    last: Box::new(DockItem::Splitter {
                        axis: Axis::Horizontal,
                        align: SplitterAlign::Last,
                        pos: 150.0,
                        first: Box::new(DockItem::TabControl {
                            current: 1,
                            tabs: vec![
                                DockTab {
                                    closeable: false,
                                    title: "Edit".to_string(),
                                    item: Panel::FileEditorTarget
                                },
                                DockTab {
                                    closeable: true,
                                    title: "main.rs".to_string(),
                                    item: Panel::FileEditor {path: "examples/quad_example/src/main.rs".to_string(), editor_id: 1}
                                }
                            ],
                        }),
                        last: Box::new(DockItem::TabControl {
                            current: 0,
                            tabs: vec![
                                DockTab {
                                    closeable: false,
                                    title: "Local Terminal".to_string(),
                                    item: Panel::LocalTerminal {start_path: "./".to_string(), terminal_id: 1}
                                },
                                DockTab {
                                    closeable: false,
                                    title: "Rust Compiler".to_string(),
                                    item: Panel::RustCompiler
                                },
                                DockTab {
                                    closeable: false,
                                    title: "Keyboard".to_string(),
                                    item: Panel::Keyboard
                                }
                            ]
                        })
                    })
                },
            },
            windows: vec![],
            app_global: AppGlobal {
                rust_compiler: RustCompiler::style(cx),
                text_buffers: TextBuffers {
                    root_path: "./".to_string(),
                    storage: HashMap::new()
                },
                index_file_read: FileRead::default(),
                app_state_file_read: FileRead::default(),
                file_tree_data: String::new(),
                file_tree_reload_signal: cx.new_signal(),
                state: AppState::default()
            }
        }
    }
    
    pub fn default_layout(&mut self, cx: &mut Cx) {
        self.app_global.state.windows = vec![self.app_window_state_template.clone()];
        self.windows = vec![self.app_window_template.clone()];
        cx.send_signal(self.app_global.file_tree_reload_signal, 0);
        cx.redraw_child_area(Area::All);
    }
    
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                self.app_global.handle_construct(cx);
                
                let key = [7u8, 4u8, 5u8, 1u8];
                let mut hub_server = HubServer::start_hub_server_default(&key);
                hub_server.start_announce_server_default(&key);
                
                self.hub_server = Some(hub_server);
                
                if let Ok(address) = HubClient::wait_for_announce(&key){
                    if let Ok(hub_client) = HubClient::connect_to_hub(&key, address){
                        self.hub_client = Some(hub_client);
                        // now we need to route the hub client
                    }
                }

                self.app_global.index_file_read = cx.file_read(&format!("{}index.json", self.app_global.text_buffers.root_path));
                if cx.platform_type.is_desktop() {
                    self.app_global.app_state_file_read = cx.file_read(&format!("{}makepad_state.json", self.app_global.text_buffers.root_path));
                }
                else {
                    self.default_layout(cx);
                }

            },
            Event::FileRead(fr) => {
                // lets see which file we loaded
                if let Some(utf8_data) = self.app_global.index_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        self.app_global.file_tree_data = utf8_data.to_string();
                        cx.send_signal(self.app_global.file_tree_reload_signal, 0);
                    }
                }
                else if let Some(utf8_data) = self.app_global.app_state_file_read.resolve_utf8(fr) {
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
                                let create_pos;
                                if last_pos.x < -1000. || last_pos.y < -1000. {
                                    create_pos = None;
                                }
                                else {
                                    create_pos = Some(last_pos);
                                }
                                self.windows.push(AppWindow {
                                    desktop_window: DesktopWindow {window: Window {
                                        create_inner_size: Some(size),
                                        create_position: create_pos,
                                        ..Window::style(cx)
                                    }, ..self.app_window_template.desktop_window.clone()},
                                    ..self.app_window_template.clone()
                                })
                            }
                            cx.send_signal(self.app_global.file_tree_reload_signal, 0);
                            cx.redraw_child_area(Area::All);
                        }
                    }
                    else { // load default window
                        self.default_layout(cx);
                    }
                }
                else if self.app_global.text_buffers.handle_file_read(cx, &fr) {
                    // this should work already
                    //cx.redraw_child_area(Area::All);
                }
            },
            
            _ => ()
        }
        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.handle_app_window(cx, event, window_index, &mut self.app_global);
            // break;
        }
    }
    
    pub fn draw_app(&mut self, cx: &mut Cx) {
        //return;
        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.draw_app_window(cx, window_index, &mut self.app_global);
            // break;
        }
    }
}
