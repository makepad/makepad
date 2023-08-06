use {
    crate::{
        makepad_platform::*,
        makepad_draw::*,
        makepad_widgets::*,
        makepad_widgets::file_tree::*,
        makepad_widgets::dock::*,
        makepad_file_protocol::{
            FileRequest,
            FileResponse,
            FileClientAction,
        },
        file_client::FileClient,
        file_system::*,
        run_view::*,
        build::{
            build_manager::{
                BuildManager,
            },
        },
    },
};

live_design!{
    import makepad_widgets::theme::*;
    import makepad_widgets::frame::*;
    import makepad_widgets::file_tree::FileTree;
    import makepad_widgets::log_list::LogList;
    import makepad_widgets::dock::*;
    import makepad_widgets::desktop_window::DesktopWindow;

    import makepad_studio::run_view::RunView;

    const FS_ROOT = ""
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
                    align: FromB(100.0),
                    a: open_files,
                    b: run_view
                }
                
                open_files = Tabs {
                    tabs: [file1, file2, file3],
                    selected: 0
                }
                
                file1 = Tab {
                    name: "File1"
                    kind: Empty1
                }
                
                file2 = Tab {
                    name: "File2"
                    kind: Empty2
                }
                
                file3 = Tab {
                    name: "File3"
                    kind: Empty3
                }
                
                file_tree = Tab {
                    name: "FileTree",
                    kind: FileTree
                }
                
                log_list = Tab {
                    name: "LogList",
                    kind: Empty3
                }
                
                run_view = Tab {
                    name: "Run",
                    kind: RunView
                }
                
                Empty1 = <Rect> {draw_bg: {color: #533}}
                Empty2 = <Rect> {draw_bg: {color: #353}}
                Empty3 = <Rect> {draw_bg: {color: #335}}
                Empty4 = <Rect> {draw_bg: {color: #535}}
                RunView = <RunView>{}
                FileTree = <FileTree> {}
                //LogList = <LogList>{}
            }
        }
    }
}

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[live] file_client: FileClient,
    #[live] build_manager: BuildManager,
    #[rust] file_system: FileSystem
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::build::build_manager::live_design(cx);
        crate::file_client::live_design(cx);
        crate::shader_view::live_design(cx);
        crate::run_view::live_design(cx);
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.file_client.send_request(FileRequest::LoadFileTree {with_data: false});
        self.build_manager.init(cx);
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
        
        if let Event::Draw(event) = event{
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
                
                if let Some(mut run_view) = next.into_run_view().borrow_mut(){
                    log!("HERE!");
                    run_view.draw(cx, &self.build_manager);
                }
            }
            return
        }

        run_view.handle_event(cx, event, &mut self.build_manager);
        for _action in self.build_manager.handle_event(cx, event){
        }
        
        for action in self.file_client.handle_event(cx, event) {
            match action {
                FileClientAction::Response(response) => match response {
                    FileResponse::LoadFileTree(response) => {
                        self.file_system.load_file_tree(response.unwrap());
                        self.ui.get_file_tree(id!(file_tree)).redraw(cx);
                        // dock.select_tab(cx, dock, state, live_id!(file_tree).into(), live_id!(file_tree).into(), Animate::No);
                    }
                    response => {
                        self.build_manager.handle_file_response(cx, &response);
                        // self.editors.handle_collab_response(cx, &mut state.editor_state, response, &mut self.collab_client.request_sender())
                    }
                },
                FileClientAction::Notification(_notification) => {
                    //self.editors.handle_collab_notification(cx, &mut state.editor_state, notification)
                }
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
            let path = self.file_system.file_nodes.get(&file_id).unwrap().name.clone();
            // lets add a file tab 'somewhere'
            dock.create_tab(cx, live_id!(content2), LiveId::unique(), live_id!(Empty4), path);
        }
    }
}
