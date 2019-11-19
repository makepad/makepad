//use syn::Type;
use render::*;
use widget::*;
use editor::*;
use crate::appwindow::*;
use crate::appstorage::*;
use crate::filetree::*;
use crate::buildmanager::*;
use crate::makepadtheme::*;

pub struct App {
    pub app_window_state_template: AppWindowState,
    pub app_window_template: AppWindow,
    pub menu: Menu,
    pub menu_signal: Signal,
    pub state: AppState,
    pub storage: AppStorage,
    pub build_manager: BuildManager,
    pub windows: Vec<AppWindow>,
}

impl App {
    
    pub fn proto(cx: &mut Cx) -> Self {

        set_dark_widget_theme(cx);
        set_dark_editor_theme(cx);
        set_dark_makepad_theme(cx);
        let ms = cx.new_signal();
        Self {
            menu: Menu::main(vec![
                Menu::sub("Makepad", "M", vec![
                    Menu::item("About Makepad", "", false, ms, 2),
                    Menu::line(),
                    Menu::item("Preferences", ",", false, ms, 2),
                    Menu::line(),
                    Menu::item("Quit Makepad", "q", false, ms, 0),
                ]),
                Menu::sub("File", "f", vec![
                    Menu::item("New File", "n", true, ms, 2),
                    Menu::item("New Window", "N", false, ms, 2),
                    Menu::line(),
                    Menu::item("Add Folder to Workspace", "", false, ms, 2),
                    Menu::line(),
                    Menu::item("Save As", "S", false, ms, 2),
                    Menu::line(),
                    Menu::item("Rename", "", false, ms, 2),
                    Menu::line(),
                    Menu::item("Close Editor", "w", false, ms, 2),
                    Menu::item("Remove Folder from Workspace", "", false, ms, 2),
                    Menu::item("Close Window", "W", false, ms, 2),
                ]),                 
                Menu::sub("Edit", "e", vec![
                    Menu::item("Undo", "z", false, ms, 2),
                    Menu::item("Redo", "Z", false, ms, 2),
                    Menu::line(),
                    Menu::item("Cut", "x", false, ms, 2),
                    Menu::item("Copy", "c", false, ms, 2),
                    Menu::item("Paste", "v", false, ms, 3),
                    Menu::line(),
                    Menu::item("Find", "", false, ms, 2),
                    Menu::item("Replace", "", false, ms, 2),
                    Menu::line(),
                    Menu::item("Find in Files", "", false, ms, 2),
                    Menu::item("Replace in Files", "", false, ms, 2),
                    Menu::line(),
                    Menu::item("Toggle Line Comment", "", false, ms, 3),
                    Menu::item("Toggle Block Comment", "", false, ms, 3),
                ]),
                Menu::sub("Selection", "s", vec![
                    Menu::item("Select All", "a", false, ms, 2),
                ]),
                Menu::sub("View", "v", vec![
                    Menu::item("Zoom In", "+", false, ms, 2),
                    Menu::item("Zoom Out", "-", false, ms, 2),
                ]),
                Menu::sub("Run", "s", vec![
                    Menu::item("Start Program", "`", false, ms, 2),
                    Menu::item("Stop Program", "~", false, ms, 2),
                ]),
                Menu::sub("Window", "w", vec![
                    Menu::item("Minimize", "m", false, ms, 2),
                    Menu::item("Zoom", "", false, ms, 2),
                    Menu::line(),
                    Menu::item("Bring All to Front", "", false, ms, 2),
                ]),
                Menu::sub("Help", "h", vec![
                    Menu::item("About Makepad", "",false, ms, 2),
                ])
            ]),
            menu_signal:ms,
            app_window_template: AppWindow::style(cx),
            app_window_state_template: AppWindowState {
                open_folders: Vec::new(),
                window_inner_size: Vec2::zero(),
                window_position: Vec2::zero(),
                dock_items: DockItem::Splitter {
                    axis: Axis::Vertical,
                    align: SplitterAlign::First,
                    pos: 150.0,
                    first: Box::new(DockItem::TabControl {
                        current: 0,
                        previous: 0,
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
                            current: 0,
                            previous: 0,
                            tabs: vec![
                                DockTab {
                                    closeable: false,
                                    title: "Edit".to_string(),
                                    item: Panel::FileEditorTarget
                                },/*
                                DockTab {
                                    closeable: true,
                                    title: "main.rs".to_string(),
                                    item: Panel::FileEditor {
                                        path: "examples/text_example/src/main.rs".to_string(),
                                        scroll_pos: Vec2::zero(),
                                        editor_id: 1
                                    }
                                }*/
                            ],
                        }),
                        last: Box::new(DockItem::TabControl {
                            current: 0,
                            previous: 0,
                            tabs: vec![
                                DockTab {
                                    closeable: false,
                                    title: "Keyboard".to_string(),
                                    item: Panel::Keyboard
                                },
                                DockTab {
                                    closeable: false,
                                    title: "Log List".to_string(),
                                    item: Panel::LogList
                                },
                                DockTab {
                                    closeable: false,
                                    title: "Log Item".to_string(),
                                    item: Panel::LogItem
                                },
                            ]
                        })
                    })
                },
            },
            windows: vec![],
            build_manager: BuildManager::new(cx),
            state: AppState::default(),
            storage: AppStorage::style(cx)
        }
    }
    
    pub fn default_layout(&mut self, cx: &mut Cx) {
        self.state.windows = vec![self.app_window_state_template.clone()];
        self.windows = vec![self.app_window_template.clone()];
        cx.redraw_child_area(Area::All);
    }
    
    pub fn handle_app(&mut self, cx: &mut Cx, event: &mut Event) {
        match event {
            Event::Construct => {
                self.storage.init(cx);
                if !cx.platform_type.is_desktop() {
                    self.default_layout(cx);
                }
            },
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::KeyR => if ke.modifiers.logo || ke.modifiers.control {
                    self.storage.reload_workspaces();
                },
                _ => ()
            },
            Event::Signal(se) => {
                // process network messages for hub_ui
                if let Some(hub_ui) = &mut self.storage.hub_ui {
                    if self.storage.hub_ui_message.is_signal(se) {
                        if let Some(mut msgs) = hub_ui.get_messages() {
                            for htc in msgs.drain(..) {
                                self.build_manager.handle_hub_msg(cx, &mut self.storage, &htc);
                                self.storage.handle_hub_msg(cx, htc, &mut self.windows, &mut self.state);
                            }
                            return
                        }
                    }
                }
                if self.storage.settings_changed.is_signal(se) {
                    // we have to reload settings.
                    self.storage.reload_workspaces();
                }
            },
            Event::FileRead(fr) => {
                // lets see which file we loaded
                if let Some(utf8_data) = self.storage.file_tree_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        if let Ok(tree) = ron::de::from_str(utf8_data) {
                            for window in &mut self.windows {
                                window.file_panel.file_tree.root_node = hub_to_tree(&tree);
                                if let FileNode::Folder {folder, state, name, ..} = &mut window.file_panel.file_tree.root_node {
                                    *name = "".to_string();
                                    *state = NodeState::Open;
                                    for node in folder.iter_mut() {
                                        if let FileNode::Folder {state, ..} = node {
                                            *state = NodeState::Open
                                        }
                                    }
                                }
                                window.file_panel.file_tree.view.redraw_view_area(cx);
                            }
                        }
                    }
                }
                else if let Some(utf8_data) = self.storage.app_state_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        if let Ok(state) = ron::de::from_str(&utf8_data) {
                            self.state = state;
                            self.windows.truncate(0);
                            // create our windows with the serialized positions/size
                            for window_state in &self.state.windows {
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
                                        ..Window::proto(cx)
                                    }, ..self.app_window_template.desktop_window.clone()},
                                    ..self.app_window_template.clone()
                                })
                            }
                            cx.redraw_child_area(Area::All);
                            
                        }
                    }
                    else { // load default window
                        self.default_layout(cx);
                    }
                }
                else if let Some(utf8_data) = self.storage.app_settings_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        self.storage.load_settings(cx, utf8_data);
                    }
                    else { // create default settings file
                        let ron = ron::ser::to_string_pretty(&self.storage.settings, ron::ser::PrettyConfig::default()).expect("cannot serialize settings");
                        cx.file_write("makepad_settings.ron", ron.as_bytes());
                        self.storage.load_settings(cx, &ron);
                    }
                }
                else {
                    for (_path, atb) in &mut self.storage.text_buffers {
                        if let Some(utf8_data) = atb.file_read.resolve_utf8(fr) {
                            if let Ok(utf8_data) = utf8_data {
                                atb.text_buffer.load_from_utf8(cx, utf8_data);
                                break;
                            }
                        }
                    }
                }
            },
            
            _ => ()
        }
        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.handle_app_window(cx, event, window_index, &mut self.state, &mut self.storage, &mut self.build_manager);
            // break;
        }
    }
    
    
    pub fn draw_app(&mut self, cx: &mut Cx) {
        
        //return;
        for (window_index, window) in self.windows.iter_mut().enumerate() {
            window.draw_app_window(cx, &self.menu, window_index, &mut self.state, &mut self.storage, &mut self.build_manager);
            // break;
        }
    }
}
