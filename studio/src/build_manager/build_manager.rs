#![allow(unused_imports)]
#![allow(dead_code)]
use {
    crate::{
        makepad_micro_serde::*,
        makepad_platform::*,
        makepad_platform::os::cx_stdin::{
            HostToStdin,
            StdinToHost,
            StdinWindowSize
        },
        build_manager::{
            build_protocol::*,
            build_server::{BuildConnection, BuildServer},
            build_client::BuildClient
        },
        makepad_file_protocol::{
            FileNotification,
            FileRequest,
            FileResponse,
        },
        makepad_shell::*,
        makepad_widgets::*,
        makepad_widgets::list_view::ListView,
    },
    std::{
        collections::HashMap,
        env,
        cell::Cell,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::mpsc::{self, Receiver, Sender, TryRecvError},
        thread,
        path::PathBuf
    },
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    Icon = <View> {
        show_bg: true,
        width: 10,
        height: 10
    }
    
    LogIcon = <PageFlip> {
        active_page: log
        width: Fit,
        height: Fit,
        margin: {top: 1, left: 5, right: 5}
        wait = <Icon> {
            draw_bg: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.circle(5., 5., 4.)
                    sdf.fill(THEME_COLOR_TEXT_META)
                    sdf.move_to(3., 5.)
                    sdf.line_to(3., 5.)
                    sdf.move_to(5., 5.)
                    sdf.line_to(5., 5.)
                    sdf.move_to(7., 5.)
                    sdf.line_to(7., 5.)
                    sdf.stroke(#0, 0.8)
                    return sdf.result
                }
            }
        },
        log = <Icon> {
            draw_bg: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.circle(5., 5., 4.);
                    sdf.fill(THEME_COLOR_TEXT_META);
                    let sz = 1.;
                    sdf.move_to(5., 5.);
                    sdf.line_to(5., 5.);
                    sdf.stroke(#a, 0.8);
                    return sdf.result
                }
            }
        }
        error = <Icon> {
            draw_bg: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.circle(5., 5., 4.5);
                    sdf.fill(THEME_COLOR_ERROR);
                    let sz = 1.5;
                    sdf.move_to(5. - sz, 5. - sz);
                    sdf.line_to(5. + sz, 5. + sz);
                    sdf.move_to(5. - sz, 5. + sz);
                    sdf.line_to(5. + sz, 5. - sz);
                    sdf.stroke(#0, 0.8)
                    return sdf.result
                }
            }
        },
        warning = <Icon> {
            draw_bg: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.move_to(5., 1.);
                    sdf.line_to(9.25, 9.);
                    sdf.line_to(0.75, 9.);
                    sdf.close_path();
                    sdf.fill(THEME_COLOR_WARNING);
                    //  sdf.stroke(#be, 0.5);
                    sdf.move_to(5., 3.5);
                    sdf.line_to(5., 5.25);
                    sdf.stroke(#0, 1.0);
                    sdf.move_to(5., 7.25);
                    sdf.line_to(5., 7.5);
                    sdf.stroke(#0, 1.0);
                    return sdf.result
                }
            }
        }
        panic = <Icon> {
            draw_bg: {
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.move_to(5., 1.);
                    sdf.line_to(9., 9.);
                    sdf.line_to(1., 9.);
                    sdf.close_path();
                    sdf.fill(THEME_COLOR_PANIC);
                    let sz = 1.;
                    sdf.move_to(5. - sz, 6.25 - sz);
                    sdf.line_to(5. + sz, 6.25 + sz);
                    sdf.move_to(5. - sz, 6.25 + sz);
                    sdf.line_to(5. + sz, 6.25 - sz);
                    sdf.stroke(#0, 0.8);
                    return sdf.result
                }
            }
        }
    }
    
    LogItem = <RectView> {
        height: Fit,
        width: Fill
        padding: {top: 7, bottom: 7}
        
        draw_bg: {
            instance is_even: 0.0
            instance selected: 0.0
            instance hover: 0.0
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_BG_EDITOR,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    ),
                    THEME_COLOR_BG_SELECTED,
                    self.selected
                );
            }
        }
        animator: {
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {hover: 0.0}
                    }
                }
                on = {
                    cursor: Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                    },
                }
            }
            
            select = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 1.0}
                    }
                }
            }
        }
    }
    
    BuildItem = <RectView> {
        height: Fit,
        width: Fill
        padding: {top: 0, bottom: 0}
        
        draw_bg: {
            instance is_even: 0.0
            instance selected: 0.0
            instance hover: 0.0
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_BG_EDITOR,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    ),
                    THEME_COLOR_BG_SELECTED,
                    self.selected
                );
            }
        }
    }
    
    
    
    LogList = <ListView> {
        grab_key_focus: true
        auto_tail: true
        drag_scrolling: false
        height: Fill,
        width: Fill
        flow: Down
        Location = <LogItem> {
            icon = <LogIcon> {},
            location = <LinkLabel> {margin: 0, text: ""}
            body = <Label> {width: Fill, margin: {left: 5}, padding: 0, draw_text: {wrap: Word}}
        }
        Bare = <LogItem> {
            icon = <LogIcon> {},
            body = <Label> {width: Fill, margin: 0, padding: 0, draw_text: {wrap: Word}}
        }
        Empty = <RectView> {
            height: 20,
            width: Fill
            draw_bg: {
                instance is_even: 0.0
                fn pixel(self) -> vec4 {
                    return mix(
                        THEME_COLOR_BG_EDITOR,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    )
                }
            }
        }
    }
    
    RunList = <ItemView> {
        grab_key_focus: true
        drag_scrolling: false
        height: Fill,
        width: Fill
        flow: Down
        Target = <BuildItem> {
            padding: {top: 0, bottom: 0}
            //label = <Label> {width: Fill, margin:{left:35}, padding:0, draw_text: {wrap: Word}}
            check = <CheckBox> {
                width: Fill,
                height: 25,
                margin: {left: 25},
                label_walk: {margin: {top: 7}}
                draw_check: {check_type: Radio}
                draw_text: {text_style: <THEME_FONT_LABEL> {}}
            }
        }
        Binary = <BuildItem> {
            padding: {top: 0, bottom: 0}
            flow: Overlay
            fold = <FoldButton> {animator: {open = {default: no}}, height: 25, width: Fill margin: {left: 5}}
            label = <Label> {width: Fill, margin: {left: 20, top: 7}, padding: 0, draw_text: {wrap: Word}}
        }
        Empty = <RectView> {
            height: 20,
            width: Fill
            draw_bg: {
                instance is_even: 0.0
                fn pixel(self) -> vec4 {
                    return mix(
                        THEME_COLOR_BG_EDITOR,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    )
                }
            }
        }
    }
    
    
    BuildManager = {{BuildManager}} {
        recompile_timeout: 0.2
    }
}

pub struct BuildClientProcess {
    pub cmd_id: BuildCmdId,
    pub texture: Texture
}

pub struct BuildClientWrap {
    client: BuildClient,
    pub processes: HashMap<String, BuildClientProcess>,
}

#[derive(Live, LiveHook)]
pub struct BuildManager {
    #[live] path: String,
    #[live] recompile_timeout: f64,
    #[rust] pub clients: Vec<BuildClientWrap>,
    #[rust] recompile_timer: Timer,
    #[rust] pub log: Vec<LogItem>,
    #[rust] build_targets: Vec<BuildTarget>,
    #[rust] binaries: Vec<BuildBinary>
}

struct BuildBinary {
    open: f64,
    name: String
}

enum BuildTarget {
    Release,
    ReleaseView,
    IosSim,
    IosReal,
    Android,
    Wasm
}

impl BuildTarget {
    fn len() -> u64 {6}
    fn index(idx: u64) -> BuildTarget {
        match idx {
            0 => Self::Release,
            1 => Self::ReleaseView,
            2 => Self::IosSim,
            3 => Self::IosReal,
            4 => Self::Android,
            _ => Self::Wasm,
        }
    }
    fn name(&self) -> &'static str {
        match self {
            Self::Release => "Release",
            Self::ReleaseView => "Release-view",
            Self::IosSim => "iOS simulator",
            Self::IosReal => "iOS device",
            Self::Android => "Android adb",
            Self::Wasm => "WASM",
        }
    }
}

pub enum BuildManagerAction {
    RedrawDoc, // {doc_id: DocumentId},
    StdinToHost {cmd_id: BuildCmdId, msg: StdinToHost},
    RedrawLog,
    ClearLog,
    None
}

const WHAT_TO_BUILD: &'static str = "makepad-example-news-feed";

impl BuildManager {
    
    pub fn draw_log(&self, cx: &mut Cx2d, list: &mut ListView) {
        //let dt = profile_start();
        list.set_item_range(cx, 0, self.log.len() as u64);
        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even = item_id & 1 == 0;
            fn map_level_to_icon(level: LogItemLevel) -> LiveId {
                match level {
                    LogItemLevel::Warning => live_id!(warning),
                    LogItemLevel::Error => live_id!(error),
                    LogItemLevel::Log => live_id!(log),
                    LogItemLevel::Wait => live_id!(wait),
                    LogItemLevel::Panic => live_id!(panic),
                }
            }
            if let Some(log_item) = self.log.get(item_id as usize) {
                match log_item {
                    LogItem::Bare(msg) => {
                        let item = list.item(cx, item_id, live_id!(Bare)).unwrap().as_view();
                        item.apply_over(cx, live!{draw_bg: {is_even: (if is_even {1.0} else {0.0})}});
                        item.page_flip(id!(icon)).set_active_page(map_level_to_icon(msg.level));
                        item.widget(id!(body)).set_text(&msg.line);
                        item.draw_widget_all(cx);
                    }
                    LogItem::Location(msg) => {
                        let item = list.item(cx, item_id, live_id!(Location)).unwrap().as_view();
                        item.apply_over(cx, live!{draw_bg: {is_even: (if is_even {1.0} else {0.0})}});
                        item.page_flip(id!(icon)).set_active_page(map_level_to_icon(msg.level));
                        item.widget(id!(location)).set_text(&format!("{}: {}:{}", msg.file_name, msg.range.start().line_index, msg.range.start().byte_index));
                        item.widget(id!(body)).set_text(&msg.msg);
                        item.draw_widget_all(cx);
                    }
                    _=>()
                }
            }
            else { // draw empty items
                let item = list.item(cx, item_id, live_id!(Empty)).unwrap().as_view();
                item.apply_over(cx, live!{draw_bg: {is_even: (if is_even {1.0} else {0.0})}});
                item.draw_widget_all(cx);
            }
        }
        //profile_end!(dt);
    }
    
    pub fn draw_run_list(&self, cx: &mut Cx2d, list: &mut ItemView) {
        let mut counter = 0;
        for (index, binary) in self.binaries.iter().enumerate() {
            let is_even = counter & 1 == 0;
            let item_id = LiveId(index as u64 * (BuildTarget::len() + 1));
            let item = list.item(cx, item_id, live_id!(Binary)).unwrap().as_view();
            item.apply_over(cx, live!{draw_bg: {is_even: (if is_even {1.0} else {0.0})}});
            item.widget(id!(label)).set_text(&binary.name);
            item.draw_widget_all(cx);
            counter += 1;
            if binary.open>0.001 {
                for i in 0..BuildTarget::len() {
                    let is_even = counter & 1 == 0;
                    let item_id = LiveId(index as u64 * (BuildTarget::len() + 1) + i as u64 + 1);
                    let bt = BuildTarget::index(i);
                    let item = list.item(cx, item_id, live_id!(Target)).unwrap().as_view();
                    let height = 25.0*binary.open;
                    item.apply_over(cx, live!{
                        height: (height)
                        draw_bg: {is_even: (if is_even {1.0} else {0.0})}
                    });
                    item.widget(id!(check)).set_text(bt.name());
                    item.draw_widget_all(cx);
                    counter += 1;
                }
            }
        }
        while list.space_left(cx)>0.0 {
            let is_even = counter & 1 == 0;
            let item_id = LiveId(self.binaries.len() as u64 * (BuildTarget::len() + 1) + counter);
            let item = list.item(cx, item_id, live_id!(Empty)).unwrap().as_view();
            let height = list.space_left(cx).min(20.0);
            item.apply_over(cx, live!{
                height: (height)
                draw_bg: {is_even: (if is_even {1.0} else {0.0})}
            });
            item.draw_widget_all(cx);
            counter += 1;
        }
    }
    
    pub fn handle_run_list(&mut self, cx: &mut Cx, item_id: LiveId, item: WidgetRef, actions: &WidgetActions) {
        // ok lets see if someone clicked our
        let targets = BuildTarget::len() as u64 + 1;
        let bin = item_id.0 / targets;
        let tgt = item_id.0 % targets;
        if tgt == 0 {
            if let Some(v) = item.fold_button(id!(fold)).animating(actions) {
                self.binaries[bin as usize].open = v;
                item.redraw(cx);
            }
        }
        else{
            if let Some(change) = item.check_box(id!(check)).changed(actions) {
                let binary = &self.binaries[bin as usize];
                log!("Run target {} {} {}", change, binary.name, BuildTarget::index(tgt - 1).name())
            }
        }
    }
    
    pub fn get_process(&mut self, cmd_id: BuildCmdId) -> Option<&mut BuildClientProcess> {
        for wrap in &mut self.clients {
            for process in wrap.processes.values_mut() {
                if process.cmd_id == cmd_id {
                    return Some(process)
                }
            }
        }
        return None
    }
    
    pub fn send_host_to_stdin(&self, cmd_id: Option<BuildCmdId>, msg: HostToStdin) {
        for wrap in &self.clients {
            for process in wrap.processes.values() {
                if cmd_id.is_none() || Some(process.cmd_id) == cmd_id {
                    wrap.client.send_cmd_with_id(process.cmd_id, BuildCmd::HostToStdin(msg.to_json()));
                    return;
                }
            }
        }
        log!("Send host to stdin process not found");
    }
    
    pub fn update_run_list(&mut self, _cx: &mut Cx) {
        let cwd = std::env::current_dir().unwrap();
        self.binaries.clear();
        match shell_env_cap(&[], &cwd, "cargo", &["run", "--bin"]) {
            Ok(_) => {}
            // we expect it on stderr
            Err(e) => {
                let mut after_av = false;
                for line in e.split("\n") {
                    if after_av {
                        let binary = line.trim().to_string();
                        if binary.len()>0 {
                            self.binaries.push(BuildBinary {
                                open: 0.0,
                                name: binary
                            });
                        }
                    }
                    if line.contains("Available binaries:") {
                        after_av = true;
                    }
                }
            }
        }
    }
    
    pub fn init(&mut self, cx: &mut Cx) {
        let mut client = BuildClientWrap {
            client: BuildClient::new_with_local_server(&self.path),
            processes: HashMap::new()
        };
        
        // lets update our runlist
        self.update_run_list(cx);
        
        let texture = Texture::new(cx);
        
        client.processes.insert(WHAT_TO_BUILD.into(), BuildClientProcess {
            texture,
            cmd_id: BuildCmdId(0)
        });
        
        self.clients.push(client);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }
    
    pub fn file_change(&mut self, _cx: &mut Cx) {
        for wrap in &mut self.clients {
            if let Some(process) = wrap.processes.get_mut(WHAT_TO_BUILD) {
                self.log.clear();
                process.cmd_id = wrap.client.send_cmd(BuildCmd::CargoRun {
                    what: WHAT_TO_BUILD.into(),
                });
            }
        }
    }
    
    pub fn clear_log(&mut self) {
        self.log.clear();
    }
    
    pub fn start_recompile_timer(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.recompile_timer);
        self.recompile_timer = cx.start_timeout(self.recompile_timeout);
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> Vec<BuildManagerAction> {
        let mut actions = Vec::new();
        self.handle_event_with(cx, event, &mut | _, action | actions.push(action));
        actions
    }
    /*
    pub fn handle_file_response(
        &mut self,
        cx: &mut Cx,
        response: &FileResponse,
    ) {
        match response {
            FileResponse::SaveFile(response) => {
                // lets see if we need to recompile at all
                
                // something changed for file_id
                let _file_id = response.clone().unwrap();
                cx.stop_timer(self.recompile_timer);
                self.recompile_timer = cx.start_timeout(self.recompile_timeout);
            }
            _ => {}
        }
    }*/
    
    pub fn handle_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_event: &mut dyn FnMut(&mut Cx, BuildManagerAction)) {
        if self.recompile_timer.is_event(event) {
            self.file_change(cx);
            self.clear_log();
            /*state.editor_state.messages.clear();
            for doc in &mut state.editor_state.documents.values_mut() {
                if let Some(inner) = &mut doc.inner {
                    inner.msg_cache.clear();
                }
            }*/
            dispatch_event(cx, BuildManagerAction::RedrawLog)
        }
        let mut any_msg = false;
        for wrap in &mut self.clients {
            let log = &mut self.log;
            //let editor_state = &mut state.editor_state;
            wrap.client.handle_event_with(cx, event, &mut | cx, wrap | {
                //let msg_id = editor_state.messages.len();
                // ok we have a cmd_id in wrap.msg
                match wrap.item {
                    LogItem::Location(loc) => {
                        log.push(LogItem::Location(loc));
                        dispatch_event(cx, BuildManagerAction::RedrawLog)
                        /*if let Some(doc_id) = editor_state.documents_by_path.get(UnixPath::new(&loc.file_name)) {
                            let doc = &mut editor_state.documents[*doc_id];
                            if let Some(inner) = &mut doc.inner {
                                inner.msg_cache.add_range(&inner.text, msg_id, loc.range);
                            }
                            dispatch_event(cx, BuildManagerAction::RedrawDoc {
                                doc_id: *doc_id
                            })
                        }*/
                        //editor_state.messages.push(BuildMsg::Location(loc));
                    }
                    LogItem::Bare(_) => {
                        log.push(wrap.item);
                        dispatch_event(cx, BuildManagerAction::RedrawLog)
                        //editor_state.messages.push(wrap.msg);
                    }
                    LogItem::StdinToHost(line) => {
                        let msg: Result<StdinToHost, DeJsonErr> = DeJson::deserialize_json(&line);
                        match msg {
                            Ok(msg) => {
                                dispatch_event(cx, BuildManagerAction::StdinToHost {
                                    cmd_id: wrap.cmd_id,
                                    msg
                                });
                            }
                            Err(_) => { // we should output a log string
                                log.push(LogItem::Bare(LogItemBare {
                                    level: LogItemLevel::Log,
                                    line:line.trim().to_string()
                                }));
                                /*editor_state.messages.push(BuildMsg::Bare(BuildMsgBare {
                                    level: BuildMsgLevel::Log,
                                    line
                                }));*/
                            }
                        }
                    }
                }
                any_msg = true;
            });
        }
        if any_msg {
            dispatch_event(cx, BuildManagerAction::RedrawLog)
        }
    }
}
