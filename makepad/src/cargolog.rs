use render::*;
use widget::*;
use editor::*;
use hub::*;
use crate::app::*;

#[derive(Clone)]
pub struct CargoLog {
    pub view: View<ScrollBar>,
    pub bg: Quad,
    pub text: Text,
    pub item_bg: Quad,
    pub code_icon: CodeIcon,
    pub row_height: f32,
    pub path_color: Color,
    pub message_color: Color,
    pub _draw_messages: Vec<CargoMsg>,
    pub _artifacts: Vec<String>
}

#[derive(Clone)]
pub struct CargoMsg {
    animator: Animator,
    msg: HubCargoMsg,
    is_selected: bool
}

#[derive(Clone)]
pub enum CargoLogEvent {
    SelectMessage {path: String},
    None,
}

impl CargoLog {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            bg: Quad ::style(cx),
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
            code_icon: CodeIcon {
                ..CodeIcon::style(cx)
            },
            path_color: color("#999"),
            message_color: color("#bbb"),
            row_height: 20.0,
            _draw_messages: Vec::new(),
            _artifacts: Vec::new()
        }
    }
    pub fn init(&mut self, _cx: &mut Cx, _storage: &mut AppStorage) {
    }
    
    pub fn get_default_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        Anim::new(Play::Chain {duration: 0.01}, vec![
            Track::color("bg.color", Ease::Lin, vec![(1.0, if marked {cx.color("bg_marked")} else if counter & 1 == 0 {cx.color("bg_selected")}else {cx.color("bg_odd")})])
        ])
    }
    
    pub fn get_over_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        let over_color = if marked {cx.color("bg_marked_over")} else if counter & 1 == 0 {cx.color("bg_selected_over")}else {cx.color("bg_odd_over")};
        Anim::new(Play::Cut {duration: 0.02}, vec![
            Track::color("bg.color", Ease::Lin, vec![
                (0., over_color),
                (1., over_color)
            ])
        ])
    }
    
    fn clear_textbuffer_messages(&self, cx: &mut Cx, storage: &mut AppStorage) {
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
        
        for dm in &self._draw_messages {
            
            let text_buffer = storage.text_buffer_from_path(cx, &dm.msg.path);
            
            let messages = &mut text_buffer.messages;
            messages.mutation_id = text_buffer.mutation_id;
            if messages.gc_id != cx.event_id {
                messages.gc_id = cx.event_id;
                messages.cursors.truncate(0);
                messages.bodies.truncate(0);
            }
            
            if dm.msg.level == HubCargoMsgLevel::Log {
                break
            }
            if dm.msg.head == dm.msg.tail {
                messages.cursors.push(TextCursor {
                    head: dm.msg.head as usize,
                    tail: dm.msg.tail as usize,
                    max: 0
                })
            }
            else {
                messages.cursors.push(TextCursor {
                    head: dm.msg.head,
                    tail: dm.msg.tail,
                    max: 0
                })
            }
            
            //println!("PROCESING MESSAGES FOR {} {} {}", span.byte_start, span.byte_end+1, path);
            text_buffer.messages.bodies.push(TextBufferMessage {
                body: dm.msg.body.clone(),
                level: match dm.msg.level {
                    HubCargoMsgLevel::Warning => TextBufferMessageLevel::Warning,
                    HubCargoMsgLevel::Error => TextBufferMessageLevel::Error,
                    HubCargoMsgLevel::Log => TextBufferMessageLevel::Log,
                }
            });
            //}
        }
        self.clear_textbuffer_messages(cx, storage);
    }
    
    pub fn handle_hub_msg(&mut self, cx: &mut Cx, storage: &mut AppStorage, htc: &HubToClientMsg) {
        match &htc.msg {
            HubMsg::CargoPackagesResponse {uid, packages} => {
            },
            HubMsg::CargoClear{uid} => {
                self._draw_messages.truncate(0);
                self.view.redraw_view_area(cx);
            },
            HubMsg::CargoMsg {uid, msg} => {
                self._draw_messages.push(CargoMsg {
                    animator:  Animator::new(Self::get_default_anim(cx, self._draw_messages.len(), false)),
                    msg: msg.clone(),
                    is_selected: false
                });
                self.view.redraw_view_area(cx);
            },
            HubMsg::CargoArtifact{uid, package_id, fresh}=>{
                self._artifacts.push(package_id.clone());
                self.view.redraw_view_area(cx);
            },
            _ => ()
        }
    }
    
    pub fn handle_cargo_log(&mut self, cx: &mut Cx, event: &mut Event, storage: &mut AppStorage) -> CargoLogEvent {
        // do shit here
        if self.view.handle_scroll_bars(cx, event) {
            // do zshit.
        }
        
        let mut dm_to_select = None;
        
        match event {
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::F9 => { // start compile
                    let hub_ui = storage.hub_ui.as_mut().unwrap();
                    let uid = hub_ui.alloc_uid();
                    hub_ui.send(ClientToHubMsg {
                        to: HubMsgTo::Workspace("makepad".to_string()),
                        msg: HubMsg::CargoExec {
                            uid: uid.clone(),
                            package: "makepad".to_string(),
                            target: HubCargoTarget::Check
                        }
                    });
                },
                KeyCode::F8 => { // next error
                    if self._draw_messages.len() > 0 {
                        if ke.modifiers.shift {
                            let mut selected_index = None;
                            for (counter, item) in self._draw_messages.iter_mut().enumerate() {
                                if item.is_selected {
                                    selected_index = Some(counter);
                                }
                            }
                            if let Some(selected_index) = selected_index {
                                if selected_index > 0 {
                                    dm_to_select = Some(selected_index - 1);
                                }
                                else {
                                    dm_to_select = Some(self._draw_messages.len() - 1);
                                }
                            }
                            else {
                                dm_to_select = Some(self._draw_messages.len() - 1);
                            }
                        }
                        else {
                            let mut selected_index = None;
                            for (counter, dm) in self._draw_messages.iter_mut().enumerate() {
                                if dm.is_selected {
                                    selected_index = Some(counter);
                                }
                            }
                            if let Some(selected_index) = selected_index {
                                if selected_index + 1 < self._draw_messages.len() {
                                    dm_to_select = Some(selected_index + 1);
                                }
                                else {
                                    dm_to_select = Some(0);
                                }
                            }
                            else {
                                dm_to_select = Some(0);
                            }
                        }
                    }
                },
                _ => ()
            },
            _ => ()
        }
        
        //let mut unmark_nodes = false;
        for (counter, dm) in self._draw_messages.iter_mut().enumerate() {
            match event.hits(cx, dm.animator.area, HitOpt::default()) {
                Event::Animate(ae) => {
                    dm.animator.write_area(cx, dm.animator.area, "bg.", ae.time);
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
            
            for (counter, dm) in self._draw_messages.iter_mut().enumerate() {
                if counter != dm_to_select {
                    dm.is_selected = false;
                    dm.animator.play_anim(cx, Self::get_default_anim(cx, counter, false));
                }
            };
            
            let dm = &mut self._draw_messages[dm_to_select];
            dm.is_selected = true;
            dm.animator.play_anim(cx, Self::get_over_anim(cx, dm_to_select, true));
            
            // alright we clicked an item. now what. well
            if dm.msg.path != "" {
                let text_buffer = storage.text_buffer_from_path(cx, &dm.msg.path);
                text_buffer.messages.jump_to_offset = if dm.msg.level == HubCargoMsgLevel::Log {
                    text_buffer.text_pos_to_offset(TextPos {row: dm.msg.row - 1, col: dm.msg.col - 1})
                }
                else {
                    dm.msg.head
                };
                
                cx.send_signal(text_buffer.signal, SIGNAL_TEXTBUFFER_JUMP_TO_OFFSET);
                return CargoLogEvent::SelectMessage {path: dm.msg.path.clone()}
            }
        }
        CargoLogEvent::None
    }
    
    pub fn draw_cargo_output(&mut self, cx: &mut Cx) {
        if let Err(_) = self.view.begin_view(cx, Layout::default()) {
            return
        }
        
        let mut counter = 0;
        for dm in &mut self._draw_messages {
            self.item_bg.color = dm.animator.last_color("bg.color");
            
            let bg_inst = self.item_bg.begin_quad(cx, &Layout {
                width: Bounds::Fill,
                height: Bounds::Compute, //::Fix(self.row_height),
                padding: Padding {l: 2., t: 3., b: 2., r: 0.},
                line_wrap: LineWrap::NewLine,
                ..Default::default()
            });
            
            match dm.msg.level {
                HubCargoMsgLevel::Error => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Error);
                },
                HubCargoMsgLevel::Warning => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Warning);
                },
                HubCargoMsgLevel::Log => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                }
            }
            
            self.text.color = self.path_color;
            self.text.draw_text(cx, &format!("{}:{} - ", dm.msg.path, dm.msg.row));
            let walk = cx.get_rel_turtle_walk();
            cx.set_turtle_padding(Padding {l: walk.x, t: 3., b: 2., r: 0.});
            self.text.color = self.message_color;
            self.text.draw_text(cx, &format!("{}", dm.msg.body));
            
            for line in &dm.msg.more_lines {
                self.text.color = self.path_color;
                self.text.draw_text(cx, ".  ");
                self.text.color = self.message_color;
                self.text.draw_text(cx, line);
            }
            
            let bg_area = self.item_bg.end_quad(cx, &bg_inst);
            dm.animator.update_area_refs(cx, bg_area);
            
            cx.turtle_new_line();
            
            counter += 1;
        }
        
        let bg_even = cx.color("bg_selected");
        let bg_odd = cx.color("bg_odd");
        
        self.item_bg.color = if counter & 1 == 0 {bg_even}else {bg_odd};
        let bg_inst = self.item_bg.begin_quad(cx, &Layout {
            width: Bounds::Fill,
            height: Bounds::Compute, //Bounds::Fix(self.row_height),
            padding: Padding {l: 2., t: 3., b: 2., r: 0.},
            ..Default::default()
        });
        
        /*
        if self._rustc_done == true {
            self.text.color = self.path_color;
            match self._rustc_build_stages {
                BuildStage::NotRunning => {
                    self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                    self.text.draw_text(cx, "Done");
                }
                BuildStage::Building => {
                    if self._run_when_done {
                        self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                        self.text.draw_text(cx, "Running when ready");
                    }
                    else {
                        self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                        self.text.draw_text(cx, "Building");
                    }
                },
                BuildStage::Complete => {
                    if !self._program_running {
                        self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                        self.text.draw_text(cx, "Press F9 to run");
                    }
                    else if self._draw_messages.len() == 0 {
                        self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
                        self.text.draw_text(cx, "Application running");
                    }
                }
            };
        }
        else {
            self.code_icon.draw_icon_walk(cx, CodeIconType::Wait);
            self.text.color = self.path_color;
            self.text.draw_text(cx, &format!("Checking({})", self._rustc_artifacts.len()));
        }*/
        self.code_icon.draw_icon_walk(cx, CodeIconType::Wait);
        self.text.color = self.path_color;
        self.text.draw_text(cx, &format!("Checking({})", self._artifacts.len()));

        
        self.item_bg.end_quad(cx, &bg_inst);
        cx.turtle_new_line();
        counter += 1;
        
        // draw filler nodes
        
        let view_total = cx.get_turtle_bounds();
        let rect_now = cx.get_turtle_rect();
        let mut y = view_total.y;
        while y < rect_now.h {
            self.item_bg.color = if counter & 1 == 0 {bg_even}else {bg_odd};
            self.item_bg.draw_quad_walk(cx, Bounds::Fill, Bounds::Fix((rect_now.h - y).min(self.row_height)), Margin::zero());
            cx.turtle_new_line();
            y += self.row_height;
            counter += 1;
        }
        
        self.view.end_view(cx);
    }
}