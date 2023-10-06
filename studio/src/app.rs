use crate::{
    makepad_code_editor::code_editor::*,
    makepad_platform::*,
    makepad_draw::*,
    makepad_widgets::*,
    makepad_micro_serde::*,
    makepad_widgets::file_tree::*,
    file_system::file_system::*,
    build_manager::{
        run_view::*,
        log_list::{
            LogListAction
        },
        run_list::{
            RunListAction
        },
        build_manager::{
            BuildManager,
            BuildManagerAction
        },
    }
};
use std::fs::File;
use std::io::Write;

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_code_editor::code_editor::CodeEditor;
    
    import makepad_studio_core::build_manager::run_view::RunView;
    import makepad_studio_core::build_manager::log_list::LogList;
    import makepad_studio_core::build_manager::run_list::RunList;
    
    Logo = <Button> {
        draw_icon: {
            svg_file: dep("crate://self/resources/logo_makepad.svg"),
            fn get_color(self) -> vec4 {
                return #xffffff
            }
        }
        icon_walk: {width: 300.0, height: Fit}
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                return sdf.result
            }
        }
        margin: {top: 20.0, right: 0.0, bottom: 30.0, left: 0.0}
        padding: 0.0
        text: ""
    }
    
    App = {{App}} {
        ui: <Window> {
            caption_bar = {visible: true, caption_label = {label = {text: "Makepad Studio"}}},
            window: {inner_size: vec2(1600, 900)},
            window_menu = {
                main = Main {items: [app, file, edit, selection, view, run, window, help]}
                
                app = Sub {name: "Makepad Studio", items: [about, line, settings, line, quit]}
                about = Item {name: "About Makepad Studio", enabled: false}
                settings = Item {name: "Settings", enabled: false}
                quit = Item {name: "Quit Makepad Studio", key: KeyQ}
                
                file = Sub {name: "File", items: [new_file, new_window, line, save_as, line, rename, line, close_editor, close_window]}
                new_file = Item {name: "New File", enabled: false, shift: true, key: KeyN}
                new_window = Item {name: "New Window", enabled: false, shift: true, key: KeyN}
                save_as = Item {name: "Save As", enabled: false}
                rename = Item {name: "Rename", enabled: false}
                close_editor = Item {name: "Close Editor", enabled: false}
                close_window = Item {name: "Close Window", enabled: false}
                
                edit = Sub {name: "Edit", items: [undo, redo, line, cut, copy, paste, line, find, replace, line, find_in_files, replace_in_files]}
                undo = Item {name: "Undo", enabled: false}
                redo = Item {name: "Redo", enabled: false}
                cut = Item {name: "Cut", enabled: false}
                copy = Item {name: "Copy", enabled: false}
                paste = Item {name: "Paste", enabled: false}
                find = Item {name: "Find", enabled: false}
                replace = Item {name: "Replace", enabled: false}
                find_in_files = Item {name: "Find in Files", enabled: false}
                replace_in_files = Item {name: "Replace in Files", enabled: false}
                
                selection = Sub {name: "Selection", items: [select_all]}
                select_all = Item {name: "Select All", enabled: false}
                
                view = Sub {name: "View", items: [select_all]}
                zoom_in = Item {name: "Zoom In", enabled: false}
                zoom_out = Item {name: "Zoom Out", enabled: false}
                select_all = Item {name: "Enter Full Screen", enabled: false}
                
                run = Sub {name: "Run", items: [run_program]}
                run_program = Item {name: "Run Program", enabled: false}
                
                window = Sub {name: "Window", items: [minimize, zoom, line, all_to_front]}
                minimize = Item {name: "Minimize", enabled: false}
                zoom = Item {name: "Zoom", enabled: false}
                all_to_front = Item {name: "Bring All to Front", enabled: false}
                
                help = Sub {name: "Help", items: [about]}
                
                line = Line,
            }
            body = {dock = <Dock> {
                height: Fill,
                width: Fill
                
                root = Splitter {
                    axis: Horizontal,
                    align: FromA(230.0),
                    a: file_tree_tabs,
                    b: split1
                }
                
                split1 = Splitter {
                    axis: Vertical,
                    align: FromB(200.0),
                    a: split2,
                    b: log_tabs
                }
                
                split2 = Splitter {
                    axis: Horizontal,
                    align: Weighted(0.5),
                    a: edit_tabs,
                    b: run_tabs
                }
                
                
                
                file_tree_tabs = Tabs {
                    tabs: [file_tree, search, run_list],
                    selected: 2
                }
                
                edit_tabs = Tabs {
                    tabs: [edit_first],
                    selected: 0
                }
                
                log_tabs = Tabs {
                    tabs: [log_list],
                    selected: 0
                }
                
                run_tabs = Tabs {
                    tabs: [run_first],
                    selected: 0
                }
                
                file_tree = Tab {
                    name: "Explore",
                    closable: false,
                    kind: FileTree
                }
                
                search = Tab {
                    name: "Search"
                    closable: false,
                    kind: Search
                }
                
                run_first = Tab {
                    name: "View"
                    closable: false,
                    kind: RunFirst
                }
                edit_first = Tab {
                    name: "Edit"
                    closable: false,
                    kind: EditFirst
                }
                
                run_list = Tab {
                    name: "Run"
                    closable: false,
                    kind: RunList
                }
                
                file1 = Tab {
                    name: "app.rs",
                    closable: true,
                    kind: CodeEditor
                }
                
                log_list = Tab {
                    name: "Log",
                    closable: false,
                    kind: LogList
                }
                
                CodeEditor = <CodeEditor> {}
                EditFirst = <RectView> {
                    draw_bg: {color: #052329}
                    <View> {
                        width: Fill,
                        height: Fill
                        align: {
                            x: 0.5,
                            y: 0.5
                        }
                        flow: Down
                        
                            <Logo> {}
                        
                        <Label> {
                            text: "Welcome to\nMakepad \n\n欢迎来到\nMakepad"
                            width: Fit,
                            margin: {left: 200}
                            draw_text: {
                                text_style: {
                                    font_size: 20.0,
                                    height_factor: 1.0,
                                    font: {path: dep("crate://makepad-widgets/resources/GoNotoKurrent-Regular.ttf")}
                                },
                            }
                        }
                    }
                    
                }
                RunFirst = <RectView> {
                    draw_bg: {color: #4}
                    <View> {
                        width: Fill,
                        height: Fill
                        align: {
                            x: 0.5,
                            y: 0.5
                        }
                        flow: Down
                            <Logo> {
                            draw_icon: {
                                fn get_color(self) -> vec4 {
                                    return #7
                                }
                            }
                        }
                    }
                    
                }
                RunList = <RunList> {
                }
                Search = <RectView> {
                    draw_bg: {color: #2}
                }
                RunView = <RunView> {}
                FileTree = <FileTree> {}
                LogList = <LogList> {}
            }}
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
        crate::build_manager::run_list::live_design(cx);
        crate::build_manager::log_list::live_design(cx);
        crate::build_manager::run_view::live_design(cx);
        // for macos
        cx.start_stdin_service();
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.file_system.init(cx);
        self.build_manager.init(cx);
        self.build_manager.discover_external_ip(cx);
        self.build_manager.start_http_server();
        //self.file_system.request_open_file(live_id!(file1), "examples/news_feed/src/app.rs".into());
    }
}

app_main!(App);

impl App {
    fn open_code_file_by_path(&mut self, cx: &mut Cx, path: &str) {
        if let Some(file_id) = self.file_system.path_to_file_node_id(&path) {
            let dock = self.ui.dock(id!(dock));            
            let tab_id = dock.unique_tab_id(file_id.0.0);
            self.file_system.request_open_file(tab_id, file_id);
            dock.create_and_select_tab(cx, live_id!(edit_tabs), tab_id, live_id!(CodeEditor), "".to_string(), TabClosable::Yes);
            self.file_system.ensure_unique_tab_names(cx, &dock)
        }
    }
}

impl AppMain for App {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        
        let dock = self.ui.dock(id!(dock));
        let file_tree = self.ui.file_tree(id!(file_tree));
        let log_list = self.ui.portal_list(id!(log_list));
        let run_list = self.ui.flat_list(id!(run_list));
        
        if let Event::Draw(event) = event {
            
            //let dt = profile_start();
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
                else if let Some(mut run_view) = next.as_run_view().borrow_mut() {
                    let current_id = dock.drawing_item_id().unwrap();
                    run_view.draw(cx, current_id, &mut self.build_manager);
                }
                else if let Some(mut log_list) = log_list.has_widget(&next).borrow_mut() {
                    self.build_manager.draw_log(cx, &mut *log_list);
                }
                else if let Some(mut run_list) = run_list.has_widget(&next).borrow_mut() {
                    self.build_manager.draw_run_list(cx, &mut *run_list);
                }
                else if let Some(mut code_editor) = next.as_code_editor().borrow_mut() {
                    // lets fetch a session
                    let current_id = dock.drawing_item_id().unwrap();
                    if let Some(session) = self.file_system.get_session_mut(current_id) {
                        code_editor.draw(cx, session);
                    }
                }
            }
            //profile_end!(dt);
            return
        }
        
        if let Event::Destruct = event {
            self.build_manager.clear_active_builds();
        }
        
        if let Event::KeyDown(KeyEvent {
            key_code,
            modifiers: KeyModifiers {logo, control, ..},
            ..
        }) = event {
            if *control || *logo {
                if let KeyCode::Backtick = key_code {
                    self.build_manager.start_recompile(cx);
                }
                else if let KeyCode::KeyK = key_code {
                    self.build_manager.clear_log(cx, &dock, &mut self.file_system);
                    log_list.redraw(cx);
                }
            }
        }
                
        for action in self.file_system.handle_event(cx, event, &self.ui) {
            match action {
                FileSystemAction::TreeLoaded => {
                    self.open_code_file_by_path(cx, "examples/slides/src/app.rs");
                }
                FileSystemAction::RecompileNeeded => {
                    self.build_manager.start_recompile_timer(cx, &self.ui);
                }
                FileSystemAction::LiveReloadNeeded(live_file_change) => {
                    self.build_manager.live_reload_needed(live_file_change);
                    self.build_manager.clear_log(cx, &dock, &mut self.file_system);
                    log_list.redraw(cx);
                }
            }
        }
        
        // lets iterate over the editors and handle events
        for (item_id, item) in dock.borrow_mut().unwrap().visible_items() {
            if let Some(mut run_view) = item.as_run_view().borrow_mut() {
                run_view.handle_event(cx, event, item_id, &mut self.build_manager);
            }
            else if let Some(mut code_editor) = item.as_code_editor().borrow_mut() {
                if let Some(session) = self.file_system.get_session_mut(item_id) {
                    for action in code_editor.handle_event(cx, event, session) {
                        match action {
                            CodeEditorAction::TextDidChange => {
                                // lets write the file
                                self.file_system.request_save_file(item_id)
                            }
                        }
                    }
                }
            }
        }
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        
        for action in self.build_manager.handle_event(cx, event, &mut self.file_system) {
            match action {
                BuildManagerAction::RedrawLog => {
                    // if the log_list is tailing, set the new len
                    log_list.redraw(cx);
                }
                BuildManagerAction::StdinToHost {run_view_id, msg} => {
                    if let Some(mut run_view) = dock.item(run_view_id).as_run_view().borrow_mut() {
                        run_view.handle_stdin_to_host(cx, &msg, run_view_id, &mut self.build_manager);
                    }
                }
                BuildManagerAction::RedrawFile(file_id)=>{
                    self.file_system.redraw_view_by_file_id(cx, file_id, &dock);
                }
                BuildManagerAction::RecompileStarted=>{
                    
                    self.build_manager.clear_log(cx, &dock, &mut self.file_system);
                    
                    if let Some(mut dock) = dock.borrow_mut() {
                        for (_id, (_, item)) in dock.items().iter() {
                            if let Some(mut run_view) = item.as_run_view().borrow_mut() {
                                run_view.resend_framebuffer(cx);
                            }
                        }
                    }
                }
                _ => ()
            }
        }
        
            // process events on all run_views
        if let Some(mut dock) = dock.borrow_mut() {
            for (id, (_, item)) in dock.items().iter() {
                if let Some(mut run_view) = item.as_run_view().borrow_mut() {
                    run_view.pump_event_loop(cx, event, *id, &mut self.build_manager);
                }
            }
        }
        
        for (item_id, item) in run_list.items_with_actions(&actions) {
            for action in self.build_manager.handle_run_list(cx, &run_list, item_id, item, &actions) {
                match action {
                    RunListAction::Create(run_view_id, name) => {
                        let tab_bar_id = dock.find_tab_bar_of_tab(live_id!(run_first)).unwrap();
                        dock.create_and_select_tab(cx, tab_bar_id, run_view_id, live_id!(RunView), name, TabClosable::Yes);
                        dock.redraw(cx);
                    }
                    RunListAction::Destroy(run_view_id) => {
                        dock.close_tab(cx, run_view_id);
                        dock.redraw(cx);
                    }
                    _ => ()
                }
                log_list.redraw(cx);
            }
        }
        
        for (item_id, item) in log_list.items_with_actions(&actions) {
            for action in self.build_manager.handle_log_list(cx, &log_list, item_id, item, &actions) {
                match action {
                    LogListAction::JumpToError{file_name, start, length} => {
                        // lets find a tab if we have it otherwise open it
                        if let Some(file_id) = self.file_system.path_to_file_node_id(&file_name) {
                            if let Some(tab_id) = self.file_system.file_node_id_to_tab_id(file_id){
                                dock.select_tab(cx, tab_id);
                                // ok lets scroll into view
                                if let Some(mut editor) = dock.item(tab_id).as_code_editor().borrow_mut() {
                                    if let Some(session) = self.file_system.get_session_mut(tab_id) {
                                        editor.set_cursor_and_scroll(cx, start, length, session);
                                        editor.set_key_focus(cx);
                                    }
                                }
                            }
                            else{
                                // lets open the editor
                                let tab_id = dock.unique_tab_id(file_id.0.0);
                                self.file_system.request_open_file(tab_id, file_id);
                                // lets add a file tab 'somewhere'
                                dock.create_and_select_tab(cx, live_id!(edit_tabs), tab_id, live_id!(CodeEditor), "".to_string(), TabClosable::Yes);
                                // lets scan the entire doc for duplicates
                                self.file_system.ensure_unique_tab_names(cx, &dock)
                            }
                        }
                    }
                    _ => ()
                }
                log_list.redraw(cx);
            }
        }
        
        if let Some(tab_id) = dock.clicked_tab_close(&actions) {
            dock.close_tab(cx, tab_id);
            if self.build_manager.handle_tab_close(tab_id) {
                log_list.redraw(cx);
                run_list.redraw(cx);
            }
            self.file_system.remove_tab(tab_id);
            self.file_system.ensure_unique_tab_names(cx, &dock);
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
                        let tab_id = dock.unique_tab_id(internal_id.0);
                        dock.drop_clone(cx, drop.abs, *internal_id, tab_id);
                    }
                    else {
                        dock.drop_move(cx, drop.abs, *internal_id);
                    }
                    self.file_system.ensure_unique_tab_names(cx, &dock);
                }
                else { // external file, we have to create a new tab
                    if let Some(file_id) = self.file_system.path_to_file_node_id(&path) {
                        let tab_id = dock.unique_tab_id(file_id.0.0);
                        self.file_system.request_open_file(tab_id, file_id);
                        dock.drop_create(cx, drop.abs, tab_id, live_id!(CodeEditor), "".to_string(), TabClosable::Yes);
                        self.file_system.ensure_unique_tab_names(cx, &dock)
                    }
                }
            }
        }
        
        if let Some(file_id) = file_tree.should_file_start_drag(&actions) {
            
            let path = self.file_system.file_node_path(file_id);
            file_tree.file_start_drag(cx, file_id, DragItem::FilePath {
                path,
                internal_id: None
            });
        }
        
        if let Some(file_id) = file_tree.file_clicked(&actions) {
            // ok lets open the file
            let tab_id = dock.unique_tab_id(file_id.0.0);
            self.file_system.request_open_file(tab_id, file_id);
            // lets add a file tab 'somewhere'
            dock.create_and_select_tab(cx, live_id!(edit_tabs), tab_id, live_id!(CodeEditor), "".to_string(), TabClosable::Yes);
            // lets scan the entire doc for duplicates
            self.file_system.ensure_unique_tab_names(cx, &dock)
        }
        
        if let Some(mut dock_items) = dock.needs_save(){
            dock_items.retain(|di| {
                if let DockItemStore::Tab{kind,..} = di{
                    if kind.0 == live_id!(RunView){
                        return false
                    }
                }
                true 
            });
            let state = PersistentState{
                dock_items
            };
            // alright lets save it to disk
            let saved = state.serialize_ron();
            let mut f = File::create("makepad_state.ron").expect("Unable to create file");
            f.write_all(saved.as_bytes()).expect("Unable to write data");
        }
        
    }
}

#[derive(Clone, Debug, SerRon, DeRon)]
struct PersistentState{
    dock_items: Vec<DockItemStore>
}
