//use syn::Type;
use makepad_render::*;
use makepad_widget::*;
use makepad_tinyserde::*;
use crate::appwindow::*;
use crate::appstorage::*;
use crate::filetree::*;
use crate::buildmanager::*;
use crate::makepadstyle::*;

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
    
    pub fn command_about_makepad() -> CommandId {uid!()}
    pub fn command_preferences() -> CommandId {uid!()}
    pub fn command_new_file() -> CommandId {uid!()}
    pub fn command_new_window() -> CommandId {uid!()}
    pub fn command_add_folder_to_builder() -> CommandId {uid!()}
    pub fn command_save_as() -> CommandId {uid!()}
    pub fn command_rename() -> CommandId {uid!()}
    pub fn command_close_editor() -> CommandId {uid!()}
    pub fn command_remove_folder_from_builder() -> CommandId {uid!()}
    pub fn command_close_window() -> CommandId {uid!()}
    pub fn command_find() -> CommandId {uid!()}
    pub fn command_replace() -> CommandId {uid!()}
    pub fn command_find_in_files() -> CommandId {uid!()}
    pub fn command_replace_in_files() -> CommandId {uid!()}
    pub fn command_toggle_line_comment() -> CommandId {uid!()}
    pub fn command_toggle_block_comment() -> CommandId {uid!()}
    pub fn command_start_program() -> CommandId {uid!()}
    pub fn command_stop_program() -> CommandId {uid!()}
    pub fn command_bring_all_to_front() -> CommandId {uid!()}
    
    pub fn new(cx: &mut Cx) -> Self {
        let default_opt = StyleOptions {scale: 1.0, dark: true};
        set_widget_style(cx, &default_opt);
        set_makepad_style(cx, &default_opt);
        let ms = cx.new_signal();
        
        // set up the keyboard map
        Self::command_preferences().set_key(cx, KeyCode::Comma);
        Self::command_new_file().set_key(cx, KeyCode::KeyN);
        Self::command_new_window().set_key_shift(cx, KeyCode::KeyN);
        Self::command_save_as().set_key_shift(cx, KeyCode::KeyS);
        Self::command_close_editor().set_key(cx, KeyCode::KeyW);
        Self::command_close_window().set_key_shift(cx, KeyCode::KeyW);
        
        cx.command_default_keymap();
        
        Self {
            menu: Menu::main(vec![
                Menu::sub("Makepad", vec![
                    Menu::item("About Makepad", Self::command_about_makepad()),
                    Menu::line(),
                    Menu::item("Preferences", Self::command_preferences()),
                    Menu::line(),
                    Menu::item("Quit Makepad", Cx::command_quit()),
                ]),
                Menu::sub("File", vec![
                    Menu::item("New File", Self::command_new_file()),
                    Menu::item("New Window", Self::command_new_window()),
                    Menu::line(),
                    Menu::item("Add Folder to Builder", Self::command_add_folder_to_builder()),
                    Menu::line(),
                    Menu::item("Save As", Self::command_save_as()),
                    Menu::line(),
                    Menu::item("Rename", Self::command_rename()),
                    Menu::line(),
                    Menu::item("Close Editor", Self::command_close_editor()),
                    Menu::item("Remove Folder from Builder", Self::command_remove_folder_from_builder()),
                    Menu::item("Close Window", Self::command_close_window()),
                ]),
                Menu::sub("Edit", vec![
                    Menu::item("Undo", Cx::command_undo()),
                    Menu::item("Redo", Cx::command_redo()),
                    Menu::line(),
                    Menu::item("Cut", Cx::command_cut()),
                    Menu::item("Copy", Cx::command_copy()),
                    Menu::item("Paste", Cx::command_paste()),
                    Menu::line(),
                    Menu::item("Find", Self::command_find()),
                    Menu::item("Replace", Self::command_replace()),
                    Menu::line(),
                    Menu::item("Find in Files", Self::command_find_in_files()),
                    Menu::item("Replace in Files", Self::command_replace_in_files()),
                    Menu::line(),
                    Menu::item("Toggle Line Comment", Self::command_toggle_line_comment()),
                    Menu::item("Toggle Block Comment", Self::command_toggle_block_comment()),
                ]),
                Menu::sub("Selection", vec![
                    Menu::item("Select All", Cx::command_select_all()),
                ]),
                Menu::sub("View", vec![
                    Menu::item("Zoom In", Cx::command_zoom_in()),
                    Menu::item("Zoom Out", Cx::command_zoom_out()),
                ]),
                Menu::sub("Run", vec![
                    Menu::item("Start Program", Self::command_start_program()),
                    Menu::item("Stop Program", Self::command_stop_program()),
                ]),
                Menu::sub("Window", vec![
                    Menu::item("Minimize", Cx::command_minimize()),
                    Menu::item("Zoom", Cx::command_zoom()),
                    Menu::line(),
                    Menu::item("Bring All to Front", Self::command_bring_all_to_front()),
                ]),
                Menu::sub("Help", vec![
                    Menu::item("About Makepad", Self::command_about_makepad()),
                ])
            ]),
            menu_signal: ms,
            app_window_template: AppWindow::new(cx),
            app_window_state_template: AppWindowState {
                open_folders: Vec::new(),
                window_inner_size: Vec2::default(),
                window_position: Vec2::default(),
                dock_items: DockItem::Splitter {
                    axis: Axis::Vertical,
                    align: SplitterAlign::First,
                    pos: 150.0,
                    first: Box::new(DockItem::TabControl {
                        current: 0,
                        previous: 0,
                        tabs: vec![
                            DockTab {
                                closeable: false,
                                title: "Files".to_string(),
                                item: Panel::FileTree
                            },
                            DockTab {
                                closeable: false,
                                title: "".to_string(),
                                item: Panel::SearchResults
                            }
                        ]
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
                                },
                            ],
                        }),
                        last: Box::new(DockItem::Splitter {
                            axis: Axis::Vertical,
                            align: SplitterAlign::Last,
                            pos: 150.0,
                            first: Box::new(DockItem::TabControl {
                                current: 0,
                                previous: 0,
                                tabs: vec![
                                    
                                    DockTab {
                                        closeable: false,
                                        title: "Log".to_string(),
                                        item: Panel::LogList
                                    },
                                ]
                            }),
                            last: Box::new(DockItem::TabControl {
                                current: 0,
                                previous: 0,
                                tabs: vec![
                                    DockTab {
                                        closeable: false,
                                        title: "Item".to_string(),
                                        item: Panel::ItemDisplay
                                    },
                                    DockTab {
                                        closeable: false,
                                        title: "Keyboard".to_string(),
                                        item: Panel::Keyboard
                                    },
                                ]
                            })
                        })
                    })
                },
            },
            windows: vec![],
            build_manager: BuildManager::new(cx),
            state: AppState::default(),
            storage: AppStorage::new(cx)
        }
    }
    
    pub fn default_layout(&mut self, cx: &mut Cx) {
        self.state.windows = vec![self.app_window_state_template.clone()];
        self.windows = vec![self.app_window_template.clone()];
        cx.redraw_child_area(Area::All);
    }
    
    pub fn reload_style(&mut self, cx: &mut Cx) {
        set_widget_style(cx, &self.storage.settings.style_options);
        set_makepad_style(cx, &self.storage.settings.style_options);
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
                KeyCode::KeyD => if ke.modifiers.logo || ke.modifiers.control {
                    //let size = self.build_manager.search_index.calc_total_size();
                    //println!("Text Index size {}", size);
                },
                KeyCode::KeyR => if ke.modifiers.logo || ke.modifiers.control {
                    self.storage.reload_builders();
                },
                KeyCode::Key0 => if ke.modifiers.logo || ke.modifiers.control {
                    self.storage.settings.style_options.scale = 1.0;
                    self.reload_style(cx);
                    cx.reset_font_atlas_and_redraw();
                    self.storage.save_settings(cx);
                },
                KeyCode::Equals => if ke.modifiers.logo || ke.modifiers.control {
                    let scale = self.storage.settings.style_options.scale * 1.1;
                    self.storage.settings.style_options.scale = scale.min(3.0).max(0.3);
                    self.reload_style(cx);
                    cx.reset_font_atlas_and_redraw();
                    self.storage.save_settings(cx);
                },
                KeyCode::Minus => if ke.modifiers.logo || ke.modifiers.control {
                    let scale = self.storage.settings.style_options.scale / 1.1;
                    self.storage.settings.style_options.scale = scale.min(3.0).max(0.3);
                    self.reload_style(cx);
                    cx.reset_font_atlas_and_redraw();
                    self.storage.save_settings(cx);
                },
                _ => ()
            },
            Event::Signal(se) => {
                // process network messages for hub_ui
                if let Some(hub_ui) = &mut self.storage.hub_ui {
                    if let Some(_) = se.signals.get(&self.storage.hub_ui_message) {
                        if let Some(mut msgs) = hub_ui.get_messages() {
                            for htc in msgs.drain(..) {
                                self.storage.handle_hub_msg(cx, &htc, &mut self.windows, &mut self.state, &mut self.build_manager);
                                self.build_manager.handle_hub_msg(cx, &mut self.storage, &htc);
                            }
                            return
                        }
                    }
                }
                if let Some(_statusses) = se.signals.get(&self.storage.settings_changed) {
                    if self.storage.settings_old.builders != self.storage.settings.builders {
                        self.storage.reload_builders();
                    }
                    if self.storage.settings_old.style_options != self.storage.settings.style_options {
                        self.reload_style(cx);
                        cx.reset_font_atlas_and_redraw();
                    }
                    if self.storage.settings_old.builds != self.storage.settings.builds {
                        self.build_manager.restart_build(cx, &mut self.storage);
                    }
                }
            },
            Event::FileRead(fr) => {
                // lets see which file we loaded
                if let Some(utf8_data) = self.storage.file_tree_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        if let Ok(tree) = DeRon::deserialize_ron(utf8_data) {
                            for window in &mut self.windows {
                                let mut paths = Vec::new();
                                window.file_panel.file_tree.root_node = hub_to_tree(&tree, "", &mut paths);
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
                        if let Ok(state) = DeRon::deserialize_ron(utf8_data) {
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
                                        ..Window::new(cx)
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
                    for (window_index, window) in self.windows.iter_mut().enumerate() {
                        window.ensure_unique_tab_title_for_file_editors(cx, window_index, &mut self.state);
                    }
                }
                else if let Some(utf8_data) = self.storage.app_settings_file_read.resolve_utf8(fr) {
                    if let Ok(utf8_data) = utf8_data {
                        self.storage.load_settings(cx, utf8_data);
                    }
                    else { // create default settings file
                        let def_settings = AppSettings::initial();
                        let ron = def_settings.serialize_ron();
                        cx.file_write("makepad_settings.ron", ron.as_bytes());
                        self.storage.load_settings(cx, &ron);
                    }
                }
                else {
                    for atb in &mut self.storage.text_buffers {
                        if let Some(utf8_data) = atb.file_read.resolve_utf8(fr) {
                            if let Ok(utf8_data) = utf8_data {
                                atb.text_buffer.load_from_utf8(utf8_data);
                                atb.text_buffer.send_textbuffer_loaded_signal(cx);
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
