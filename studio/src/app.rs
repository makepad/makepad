use {
    crate::{
        makepad_code_editor::code_editor::*,
        makepad_platform::*,
        makepad_draw::*,
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
        makepad_widgets::dock::*,
        file_system::file_system::*,
        run_view::*,
        build_manager::{
            build_manager::{
                BuildManager,
                BuildManagerAction
            },
        },
    },
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_widgets::file_tree::FileTree;
    import makepad_widgets::button::Button;
    import makepad_widgets::label::Label;
    import makepad_widgets::dock::*;
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_code_editor::code_editor::CodeEditor;
    
    import makepad_studio::run_view::RunView;
    import makepad_studio::build_manager::build_manager::LogList;

    Logo = <Button> {
        draw_icon: {
            svg_file: dep("crate://self/resources/logo_makepad.svg"),
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        #f00,
                        #0ff,
                        self.hover
                    ),
                    #fff,
                    self.pressed
                )
            }
        }
        icon_walk: { width: 100.0, height: Fit }
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        layout: { padding: 50.0 }
        label: "TEST"
    }
    
    App = {{App}} {
        ui: <DesktopWindow> {
            caption_bar = {visible: true, caption_label = {label = {label: "Makepad Studio"}}},
            dock = <Dock> {
                walk: {height: Fill, width: Fill}
                
                root = Splitter {
                    axis: Horizontal,
                    align: FromA(200.0),
                    a: file_tree,
                    b: split1
                }
                
                split1 = Splitter {
                    axis: Vertical,
                    align: FromB(200.0),
                    a: split2,
                    b: log_list
                }
                
                split2 = Splitter {
                    axis: Horizontal,
                    align: FromB(400.0),
                    a: open_files,
                    b: run_views
                }
                
                open_files = Tabs {
                    tabs: [welcome, file1],
                    no_close: true,
                    selected: 0
                }
                
                run_views = Tabs {
                    tabs: [run_view],
                    selected: 0
                }
                welcome = Tab {
                    name: "Welcome"
                    kind: Welcome
                }
                file1 = Tab {
                    name: "app.rs"
                    kind: CodeEditor
                }
                /*
                file3 = Tab {
                    name: "File3"
                    kind: Empty3
                }*/
                
                file_tree = Tab {
                    name: "FileTree",
                    kind: FileTree
                }
                
                log_list = Tab {
                    name: "LogList",
                    kind: LogList
                }
                
                run_view = Tab {
                    name: "Run",
                    no_close: true
                    kind: RunView
                }
                CodeEditor = <CodeEditor> {}
                Welcome = <Rect> {
                    draw_bg: {color: #052329}

                    <Frame> {
                        walk: { width: Fill, height: Fill}
                        layout: {
                            padding: 10.0
                            align: {
                                x: 0.5,
                                y: 0.5
                            }
                        }

                        <Logo> {}

                        <Label>{
                            label:"Welcome to\nMakepad\n\n欢迎来到\nMakepad"
                            draw_label: {
                                text_style: {
                                    font_size: 40.0,
                                    height_factor: 1.0,
                                    font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
                                },
                            }    
                        }

                    }

                }
                RunView = <RunView> {}
                FileTree = <FileTree> {}
                LogList = <LogList> {}
            }
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[live] build_manager: BuildManager,
    #[rust] file_system: FileSystem,
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::makepad_code_editor::live_design(cx);
        crate::build_manager::build_manager::live_design(cx);
        crate::run_view::live_design(cx);
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.file_system.init(cx);
        self.build_manager.init(cx);
        self.file_system.request_open_file(live_id!(file1), "examples/news_feed/src/app.rs".into());
    }
}

app_main!(App);

impl App {
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let dock = self.ui.get_dock(id!(dock));
        let file_tree = self.ui.get_file_tree(id!(file_tree));
        let run_view = self.ui.get_run_view(id!(run_view));
        let log_list = self.ui.get_list_view(id!(log_list));
        
        if let Event::Draw(event) = event {
            let cx = &mut Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                
                if let Some(mut file_tree) = file_tree.has_widget(&next).borrow_mut() {
                    file_tree.set_folder_is_open(cx, live_id!(root).into(), true, Animate::No);
                    self.file_system.draw_file_node(
                        cx,
                        live_id!(root).into(),
                        &mut *file_tree
                    );
                }
                else if let Some(mut run_view) = run_view.has_widget(&next).borrow_mut() {
                    run_view.draw(cx, &self.build_manager);
                }
                else if let Some(mut list_view) = log_list.has_widget(&next).borrow_mut() {
                    self.build_manager.draw_log_list(cx, &mut *list_view);
                }
                else if let Some(mut code_editor) = next.as_code_editor().borrow_mut() {
                    // lets fetch a session
                    let current_id = dock.get_drawing_item_id().unwrap();
                    if let Some(session) = self.file_system.get_session_mut(current_id) {
                        code_editor.draw(cx, session);
                    }
                }
            }
            return
        }
        
        self.file_system.handle_event(cx, event, &self.ui);
        
        if let Some(mut run_view) = run_view.borrow_mut() {
            run_view.handle_event(cx, event, &mut self.build_manager);
        }
        
        // lets iterate over the editors and handle events
        for (item_id, item) in dock.borrow_mut().unwrap().items().iter() {
            if let Some(mut code_editor) = item.as_code_editor().borrow_mut() {
                if let Some(session) = self.file_system.get_session_mut(item_id.id) {
                    for action in code_editor.handle_event(cx, event, session) {
                        match action {
                            CodeEditorAction::TextDidChange => {
                                // lets write the file
                                self.file_system.request_save_file(item_id.id)
                            }
                        }
                    }
                }
            }
        }
        
        for action in self.build_manager.handle_event(cx, event) {
            match action {
                BuildManagerAction::RedrawLog => {
                    log_list.redraw(cx);
                }
                BuildManagerAction::StdinToHost {cmd_id, msg} => if let Some(mut run_view) = run_view.borrow_mut() {
                    run_view.handle_stdin_to_host(cx, cmd_id, msg, &mut self.build_manager);
                }
                _ => ()
            }
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        // dock drag drop and tabs
        
        if let Some(tab_id) = dock.clicked_tab_close(&actions) {
            dock.close_tab(cx, tab_id);
        }
        
        if let Some(tab_id) = dock.should_tab_start_drag(&actions) {
            dock.tab_start_drag(cx, tab_id, DragItem::FilePath {
                path: "".to_string(), //String::from("file://") + &*path.into_unix_string().to_string_lossy(),
                internal_id: Some(tab_id)
            });
        }
        
        if let Some(drag) = dock.should_accept_drag(&actions) {
            if drag.items.len() == 1 {
                if drag.modifiers.logo {
                    dock.accept_drag(cx, drag, DragResponse::Copy);
                }
                else {
                    dock.accept_drag(cx, drag, DragResponse::Move);
                }
            }
        }
        
        if let Some(drop) = dock.has_drop(&actions) {
            if let DragItem::FilePath {path, internal_id} = &drop.items[0] {
                if let Some(internal_id) = internal_id { // from inside the dock
                    if drop.modifiers.logo {
                        dock.drop_clone(cx, drop.abs, *internal_id, LiveId::unique());
                    }
                    else {
                        dock.drop_move(cx, drop.abs, *internal_id);
                    }
                }
                else { // external file, we have to create a new tab
                    dock.drop_create(cx, drop.abs, LiveId::unique(), live_id!(Empty4), path.clone())
                }
            }
        }
        
        if let Some(file_id) = file_tree.should_file_start_drag(&actions) {
            let path = self.file_system.file_nodes.get(&file_id).unwrap().name.clone();
            file_tree.file_start_drag(cx, file_id, DragItem::FilePath {
                path,
                internal_id: None
            });
        }
        
        if let Some(file_id) = file_tree.file_clicked(&actions) {
            let unix_path = self.file_system.file_node_path(file_id);
            let tab_name = self.file_system.file_node_name(file_id);
            // ok lets open the file
            let tab_id = LiveId::unique();
            self.file_system.request_open_file(tab_id, unix_path);
            
            // lets add a file tab 'somewhere'
            dock.create_tab(cx, live_id!(open_files), tab_id, live_id!(CodeEditor), tab_name);
        }
    }
}
