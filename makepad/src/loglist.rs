use render::*;
use widget::*;
use editor::*;
use hub::*;
use crate::app::*;

#[derive(Clone)]
pub struct LogList {
    pub view: View<ScrollBar>,
    pub bg: Quad,
    pub text: Text,
    pub item_bg: Quad,
    pub code_icon: CodeIcon,
    pub row_height: f32,
    pub path_color: Color,
    pub message_color: Color,
    pub _active_workspace: String, 
    pub _active_package: String,
    pub _active_targets: Vec<CargoActiveTarget>,
    pub _exec_when_done: bool,
    pub _always_exec_when_done:bool,
    pub _log_items: Vec<LogItemDraw>,
    pub _artifacts: Vec<String>,
}

#[derive(Clone)]
pub struct CargoActiveTarget {
    target: String,
    artifact_path: Option<String>,
    cargo_uid: HubUid,
    artifact_uid: HubUid,
}

impl CargoActiveTarget {
    fn new(target: &str) -> CargoActiveTarget {
        CargoActiveTarget {
            target: target.to_string(),
            cargo_uid: HubUid::zero(),
            artifact_path: None,
            artifact_uid: HubUid::zero(),
        }
    }
}

#[derive(Clone)]
pub struct LogItemDraw {
    animator: Animator,
    item: HubLogItem,
    is_selected: bool
}

#[derive(Clone)]
pub enum LogListEvent {
    SelectLogItem {
        path: Option<String>,
        item: Option<String>,
        level: HubLogItemLevel
    },
    None,
}

impl LogList {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            bg: Quad::style(cx),
            item_bg: Quad::style(cx),
            text: Text {
                wrapping: Wrapping::Word,
                ..Text::style(cx)
            },
            view: View {
                scroll_h: Some(ScrollBar::style(cx)),
                scroll_v: Some(ScrollBar {
                    smoothing: Some(0.15),
                    ..ScrollBar::style(cx)
                }),
                ..View::style(cx)
            },
            code_icon: CodeIcon::style(cx),
            path_color: color("#999"),
            message_color: color("#bbb"),
            row_height: 20.0,
            _always_exec_when_done:true,
            _exec_when_done: false,
            _log_items: Vec::new(),
            _artifacts: Vec::new(),
            _active_workspace: "makepad".to_string(),
            _active_package: "csvproc".to_string(),
            _active_targets: vec![
                CargoActiveTarget::new("check"),
                CargoActiveTarget::new("build"),
                //CargoActiveTarget::new("workspace")
            ]
        }
    }
    
    pub fn init(&mut self, _cx: &mut Cx) {
        /*
        for i in 0..100 {
            self._log_items.push(LogItemDraw {
                animator: Animator::new(Self::get_default_anim(cx, self._log_items.len(), false)),
                item: HubLogItem {
                    path: None, //Some("makepad/makepad/test".to_string()),
                    row: 0,
                    col: 0,
                    tail: 0,
                    head: 0,
                    body: format!("Hello world {}", i),
                    rendered: None,
                    explanation: None,
                    level: HubLogItemLevel::Log
                },
                is_selected: false
            });
        }*/
    }
    
    pub fn get_default_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        Anim::new(Play::Chain {duration: 0.01}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![(1.0, if marked {cx.color("bg_marked")} else if counter & 1 == 0 {cx.color("bg_selected")}else {cx.color("bg_odd")})])
        ])
    }
    
    pub fn get_over_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        let over_color = if marked {cx.color("bg_marked_over")} else if counter & 1 == 0 {cx.color("bg_selected_over")}else {cx.color("bg_odd_over")};
        Anim::new(Play::Cut {duration: 0.02}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![
                (0., over_color),
                (1., over_color)
            ])
        ])
    }
    
    fn gc_textbuffer_messages(&self, cx: &mut Cx, storage: &mut AppStorage) {
        // clear all files we missed
        for (_, atb) in &mut storage.text_buffers {
            if atb.text_buffer.messages.gc_id != cx.event_id {
                atb.text_buffer.messages.cursors.truncate(0);
                atb.text_buffer.messages.bodies.truncate(0);
                cx.send_signal(atb.text_buffer.signal, SIGNAL_TEXTBUFFER_MESSAGE_UPDATE);
            }
            else {
                cx.send_signal(atb.text_buffer.signal, SIGNAL_TEXTBUFFER_MESSAGE_UPDATE);
            }
        }
    }
    
    pub fn export_messages(&self, cx: &mut Cx, storage: &mut AppStorage) {
        
        for dm in &self._log_items {
            if let Some(path) = &dm.item.path {
                let text_buffer = storage.text_buffer_from_path(cx, &path);
                
                let messages = &mut text_buffer.messages;
                messages.mutation_id = text_buffer.mutation_id;
                if messages.gc_id != cx.event_id {
                    messages.gc_id = cx.event_id;
                    messages.cursors.truncate(0);
                    messages.bodies.truncate(0);
                }
                //println!("{:?}", dm.item.level);
                if dm.item.level == HubLogItemLevel::Log {
                    break
                }
                // search for insert point
                let mut inserted = false;
                if messages.cursors.len()>0 {
                    for i in (0..messages.cursors.len()).rev() {
                        if dm.item.head < messages.cursors[i].head && (i == 0 || dm.item.head >= messages.cursors[i - 1].head) {
                            messages.cursors.insert(i, TextCursor {
                                head: dm.item.head,
                                tail: dm.item.tail,
                                max: 0
                            });
                            inserted = true;
                            break;
                        }
                    }
                }
                if !inserted {
                    messages.cursors.push(TextCursor {
                        head: dm.item.head,
                        tail: dm.item.tail,
                        max: 0
                    })
                }
                
                text_buffer.messages.bodies.push(TextBufferMessage {
                    body: dm.item.body.clone(),
                    level: match dm.item.level {
                        HubLogItemLevel::Warning => TextBufferMessageLevel::Warning,
                        HubLogItemLevel::Error => TextBufferMessageLevel::Error,
                        HubLogItemLevel::Log => TextBufferMessageLevel::Log,
                        HubLogItemLevel::Panic => TextBufferMessageLevel::Log,
                    }
                });
            }
            if dm.item.level == HubLogItemLevel::Log {
                break
            }
        }
        self.gc_textbuffer_messages(cx, storage);
    }
    
    pub fn is_running_uid(&self, uid: &HubUid) -> bool {
        for active_target in &self._active_targets {
            if active_target.cargo_uid == *uid {
                return true
            }
            if active_target.artifact_uid == *uid {
                return true
            }
        }
        return false
    }
    
    pub fn is_any_cargo_running(&self) -> bool {
        for active_target in &self._active_targets {
            if active_target.cargo_uid != HubUid::zero() {
                return true
            }
        }
        return false
    }
    
    pub fn is_any_artifact_running(&self) -> bool {
        for active_target in &self._active_targets {
            if active_target.artifact_uid != HubUid::zero() {
                return true
            }
        }
        return false
    }
        
    pub fn handle_hub_msg(&mut self, cx: &mut Cx, storage: &mut AppStorage, htc: &HubToClientMsg) -> LogListEvent {
        //let hub_ui = storage.hub_ui.as_mut().unwrap();
        match &htc.msg {
            HubMsg::CargoPackagesResponse {uid: _, packages: _} => {
            },
            HubMsg::CargoExecBegin {uid} => if self.is_running_uid(uid) {
            }, 
            HubMsg::LogItem {uid, item} => if self.is_running_uid(uid) {
                for check_msg in &self._log_items {
                    if check_msg.item == *item && (item.level == HubLogItemLevel::Warning || item.level == HubLogItemLevel::Error) { // ignore duplicates
                        return LogListEvent::None
                    }
                }
                self._log_items.push(LogItemDraw {
                    animator: Animator::new(Self::get_default_anim(cx, self._log_items.len(), false)),
                    item: item.clone(),
                    is_selected: false
                });
                self.view.redraw_view_area(cx);
                self.export_messages(cx, storage);
            },
            
            HubMsg::CargoArtifact {uid, package_id, fresh: _} => if self.is_running_uid(uid) {
                self._artifacts.push(package_id.clone());
                self.view.redraw_view_area(cx);
            },
            HubMsg::CargoExecEnd {uid, artifact_path} => if self.is_running_uid(uid) {
                // if we didnt have any errors, check if we need to run
                for active_target in &mut self._active_targets {
                    if active_target.cargo_uid == *uid {
                        active_target.cargo_uid = HubUid::zero();
                        active_target.artifact_path = artifact_path.clone();
                    }
                }
                if !self.is_any_cargo_running() && self._exec_when_done {
                    self.exec_all_artifacts(storage)
                }
                self.view.redraw_view_area(cx);
            },
            HubMsg::ArtifactExecEnd {uid} => if self.is_running_uid(uid) {
                // if we didnt have any errors, check if we need to run
                for active_target in &mut self._active_targets {
                    if active_target.artifact_uid == *uid {
                        active_target.artifact_uid = HubUid::zero();
                    }
                }
                self.view.redraw_view_area(cx);
            },            _ => ()
        }
        LogListEvent::None
    }
    
    pub fn exec_all_artifacts(&mut self, storage: &mut AppStorage) {
        let hub_ui = storage.hub_ui.as_mut().unwrap();
        // otherwise execute all we have artifacts for
        for active_target in &mut self._active_targets {
            if let Some(artifact_path) = &active_target.artifact_path {
                let uid = hub_ui.alloc_uid();
                if active_target.artifact_uid != HubUid::zero() {
                    hub_ui.send(ClientToHubMsg {
                        to: HubMsgTo::Workspace(self._active_workspace.clone()),
                        msg: HubMsg::ArtifactKill {
                            uid: active_target.artifact_uid,
                        }
                    });
                }
                active_target.artifact_uid = uid;
                hub_ui.send(ClientToHubMsg {
                    to: HubMsgTo::Workspace(self._active_workspace.clone()),
                    msg: HubMsg::ArtifactExec {
                        uid: active_target.artifact_uid,
                        path: artifact_path.clone(),
                        args: Vec::new()
                    }
                });
            }
        }
    }
    
    pub fn artifact_exec(&mut self, storage: &mut AppStorage) {
        if self.is_any_cargo_running() {
            self._exec_when_done = true;
        }
        else {
            self.exec_all_artifacts(storage)
        }
    }
    
    pub fn restart_cargo(&mut self, cx: &mut Cx, storage: &mut AppStorage) {
        self._artifacts.truncate(0);
        self._log_items.truncate(0);
        self.gc_textbuffer_messages(cx, storage);
        
        let hub_ui = storage.hub_ui.as_mut().unwrap();
        self._exec_when_done = self._always_exec_when_done;
        for active_target in &mut self._active_targets {
            active_target.artifact_path = None;
            if active_target.cargo_uid != HubUid::zero() {
                hub_ui.send(ClientToHubMsg {
                    to: HubMsgTo::Workspace(self._active_workspace.clone()),
                    msg: HubMsg::CargoKill {
                        uid: active_target.cargo_uid,
                    }
                });
                active_target.cargo_uid = HubUid::zero();
            }
            if active_target.artifact_uid != HubUid::zero() {
                hub_ui.send(ClientToHubMsg {
                    to: HubMsgTo::Workspace(self._active_workspace.clone()),
                    msg: HubMsg::ArtifactKill {
                        uid: active_target.artifact_uid,
                    }
                });
                active_target.artifact_uid = HubUid::zero();
            }
        }
        
        for active_target in &mut self._active_targets {
            let uid = hub_ui.alloc_uid();
            hub_ui.send(ClientToHubMsg {
                to: HubMsgTo::Workspace(self._active_workspace.clone()),
                msg: HubMsg::CargoExec {
                    uid: uid.clone(),
                    package: self._active_package.clone(),
                    target: active_target.target.clone()
                }
            });
            active_target.cargo_uid = uid;
        }
    }
    
    pub fn next_error(&mut self, reverse: bool) -> Option<usize> {
        if self._log_items.len() == 0 {
            return None
        }
        if reverse {
            let mut selected_index = None;
            for (counter, item) in self._log_items.iter_mut().enumerate() {
                if item.is_selected {
                    selected_index = Some(counter);
                }
            }
            if let Some(selected_index) = selected_index {
                if selected_index > 0 {
                    return Some(selected_index - 1);
                }
                else {
                    return Some(self._log_items.len() - 1);
                }
            }
            else {
                return Some(self._log_items.len() - 1);
            }
        }
        else {
            let mut selected_index = None;
            for (counter, dm) in self._log_items.iter_mut().enumerate() {
                if dm.is_selected {
                    selected_index = Some(counter);
                }
            }
            if let Some(selected_index) = selected_index {
                if selected_index + 1 < self._log_items.len() {
                    return Some(selected_index + 1);
                }
                else {
                    return Some(0);
                }
            }
            else {
                return Some(0);
            }
        }
    }
    
    pub fn handle_log_list(&mut self, cx: &mut Cx, event: &mut Event, storage: &mut AppStorage) -> LogListEvent {
        // do shit here
        if self.view.handle_scroll_bars(cx, event) {
            // do zshit.
            self.view.redraw_view_area(cx);
        }
        
        let mut dm_to_select = None;
        
        match event {
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::Period => if ke.modifiers.logo || ke.modifiers.control {
                    dm_to_select = self.next_error(false);
                },
                KeyCode::Comma => if ke.modifiers.logo || ke.modifiers.control {
                    dm_to_select = self.next_error(true);
                },
                KeyCode::Backtick => if ke.modifiers.logo || ke.modifiers.control {
                    self.artifact_exec(storage);
                    self.view.redraw_view_area(cx);
                },
                _ => ()
            },
            _ => ()
        }
        
        //let mut unmark_nodes = false;
        for (counter, dm) in self._log_items.iter_mut().enumerate() {
            match event.hits(cx, dm.animator.area, HitOpt::default()) {
                Event::Animate(ae) => {
                    dm.animator.write_area(cx, dm.animator.area, "bg.", ae.time);
                },
                Event::AnimEnded(_) => {
                    dm.animator.end();
                },
                Event::FingerDown(_fe) => {
                    cx.set_down_mouse_cursor(MouseCursor::Hand);
                    // mark ourselves, unmark others
                    dm_to_select = Some(counter);
                },
                Event::FingerUp(_fe) => {
                },
                Event::FingerMove(_fe) => {
                },
                Event::FingerHover(fe) => {
                    cx.set_hover_mouse_cursor(MouseCursor::Hand);
                    match fe.hover_state {
                        HoverState::In => {
                            dm.animator.play_anim(cx, Self::get_over_anim(cx, counter, dm.is_selected));
                        },
                        HoverState::Out => {
                            dm.animator.play_anim(cx, Self::get_default_anim(cx, counter, dm.is_selected));
                        },
                        _ => ()
                    }
                },
                _ => ()
            }
        };
        
        if let Some(dm_to_select) = dm_to_select {
            
            for (counter, dm) in self._log_items.iter_mut().enumerate() {
                if counter != dm_to_select {
                    dm.is_selected = false;
                    dm.animator.play_anim(cx, Self::get_default_anim(cx, counter, false));
                }
            };
            
            let dm = &mut self._log_items[dm_to_select];
            dm.is_selected = true;
            dm.animator.play_anim(cx, Self::get_over_anim(cx, dm_to_select, true));
            
            // alright we clicked an item. now what. well
            if let Some(path) = &dm.item.path {
                let text_buffer = storage.text_buffer_from_path(cx, &path);
                text_buffer.messages.jump_to_offset = if dm.item.level == HubLogItemLevel::Log || dm.item.level == HubLogItemLevel::Panic {
                    text_buffer.text_pos_to_offset(TextPos {row: dm.item.row - 1, col: dm.item.col - 1})
                }
                else {
                    dm.item.tail
                };
                cx.send_signal(text_buffer.signal, SIGNAL_TEXTBUFFER_JUMP_TO_OFFSET);
            }
            let item = if let Some(rendered) = &dm.item.rendered {
                if let Some(explanation) = &dm.item.explanation {
                    Some(format!("{}{}", rendered, explanation))
                }
                else {
                    println!("DOING {}", rendered);
                    Some(rendered.clone())
                }
            }
            else {
                None
            };
            return LogListEvent::SelectLogItem {
                item: item,
                path: dm.item.path.clone(),
                level: dm.item.level.clone()
            }
        }
        LogListEvent::None
    }
    
    pub fn draw_log_list(&mut self, cx: &mut Cx) {
        if let Err(_) = self.view.begin_view(cx, Layout {
            direction: Direction::Down,
            ..Layout::default()
        }) {
            return
        }
        
        let bg_even = cx.color("bg_selected");
        let bg_odd = cx.color("bg_odd");
        
        // lets get the scroll position.
        let scroll_pos = self.view.get_scroll_pos(cx);
        
        // we need to find the first item to draw
        let start_item = (scroll_pos.y / self.row_height).floor() as usize;
        let start_scroll = (start_item as f32) * self.row_height;
        // lets jump the turtle forward by scrollpos.y
        cx.move_turtle(0., start_scroll);
        
        let item_layout = Layout {
            width: Bounds::Fill,
            height: Bounds::Fix(self.row_height),
            padding: Padding {l: 2., t: 3., b: 2., r: 0.},
            line_wrap: LineWrap::None,
            ..Default::default()
        };
        
        let view_rect = cx.get_turtle_rect();
        
        let mut counter = 0;
        for i in start_item..self._log_items.len() {
            
            let walk = cx.get_rel_turtle_walk();
            if walk.y - start_scroll > view_rect.h + self.row_height {
                // this is a virtual viewport, so bail if we are below the view
                let left = (self._log_items.len() - i) as f32 * self.row_height;
                cx.walk_turtle(Bounds::Fill, Bounds::Fix(left), Margin::zero(), None);
                break
            }
            
            let dm = &mut self._log_items[i];
            
            self.item_bg.color = dm.animator.last_color(cx.id("bg.color"));
            
            let bg_inst = self.item_bg.begin_quad(cx, &item_layout);
            
            match dm.item.level {
                HubLogItemLevel::Error => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Error);
                },
                HubLogItemLevel::Warning => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Warning);
                },
                HubLogItemLevel::Panic=>{
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Panic);
                },
                HubLogItemLevel::Log => {
                    //self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                }
            }
            
            if let Some(path) = &dm.item.path {
                self.text.color = self.path_color;
                self.text.draw_text(cx, &format!("{}:{} - ", path, dm.item.row));
            }
            self.text.color = self.message_color;
            self.text.draw_text(cx, &format!("{}", dm.item.body));
            
            let bg_area = self.item_bg.end_quad(cx, &bg_inst);
            dm.animator.update_area_refs(cx, bg_area);
            
            counter += 1;
        }
        
        // draw status line
        self.item_bg.color = if counter & 1 == 0 {bg_even}else {bg_odd};
        let bg_inst = self.item_bg.begin_quad(cx, &item_layout);
        
        if !self.is_any_cargo_running() {
            self.text.color = self.path_color;
            self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
            if self.is_any_artifact_running(){
                self.text.draw_text(cx, "Running");
            }
            else{
                self.text.draw_text(cx, "Done");
            }
        }
        else {
            self.code_icon.draw_icon_walk(cx, CodeIconType::Wait);
            self.text.color = self.path_color;
            self.text.draw_text(cx, &format!("Building ({})", self._artifacts.len()));
            if self._exec_when_done {
                self.text.draw_text(cx, " - starting when done");
            }
        }
        
        self.item_bg.end_quad(cx, &bg_inst);
        counter += 1;
        
        // draw filler nodes
        let view_total = cx.get_turtle_bounds();
        let mut y = view_total.y;
        while y < view_rect.h {
            self.item_bg.color = if counter & 1 == 0 {bg_even} else {bg_odd};
            self.item_bg.draw_quad_walk(cx, Bounds::Fill, Bounds::Fix(self.row_height), Margin::zero());
            cx.set_turtle_bounds(view_total); // do this so the scrollbar doesnt show up
            y += self.row_height;
            counter += 1;
        }
        
        self.view.end_view(cx);
    }
}