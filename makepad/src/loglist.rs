use render::*;
use widget::*;
use editor::*;
use hub::*;
use crate::appstorage::*;
use crate::buildmanager::*;

#[derive(Clone)]
pub struct LogList {
    pub view: ScrollView,
    pub item_draw: LogItemDraw,
    pub list: ListLogic
}

#[derive(Clone)]
pub struct LogItemDraw {
    pub text: Text,
    pub item_bg: Quad,
    pub code_icon: CodeIcon,
    pub path_color: ColorId,
    pub message_color: ColorId,
}

impl LogItemDraw {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            item_bg: Quad::proto(cx),
            text: Text {
                wrapping: Wrapping::Word,
                ..Text::proto(cx)
            },
            code_icon: CodeIcon::proto(cx),
            path_color: Theme::color_text_defocus(),
            message_color: Theme::color_text_focus(),
        }
    }
    
    pub fn layout_item() -> LayoutId {uid!()}
    pub fn text_style_item() ->TextStyleId{uid!()}

    pub fn theme(cx: &mut Cx) {
        
        Self::layout_item().set_base(cx, Layout {
            walk: Walk::wh(Width::Fill, Height::Fix(20.)),
            align: Align::left_center(),
            padding: Padding::zero(), // {l: 2., t: 3., b: 2., r: 0.},
            line_wrap: LineWrap::None,
            ..Default::default()
        });
        
        Self::text_style_item().set_base(cx, Theme::text_style_normal().base(cx));
    }
    
    pub fn get_default_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        Anim::new(Play::Chain {duration: 0.01}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![
                (1.0, if marked {Theme::color_bg_marked().base(cx)} else if counter & 1 == 0 {Theme::color_bg_selected().base(cx)}else {Theme::color_bg_odd().base(cx)})
            ])
        ])
    }
    
    pub fn get_default_anim_cut(cx: &Cx, counter: usize, marked: bool) -> Anim {
        Anim::new(Play::Cut {duration: 0.01}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![
                (0.0, if marked {Theme::color_bg_marked().base(cx)} else if counter & 1 == 0 {Theme::color_bg_selected().base(cx)}else {Theme::color_bg_odd().base(cx)})
            ])
        ])
    }
    
    pub fn get_over_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        let over_color = if marked {Theme::color_bg_marked_over().base(cx)} else if counter & 1 == 0 {Theme::color_bg_selected_over().base(cx)}else {Theme::color_bg_odd_over().base(cx)};
        Anim::new(Play::Cut {duration: 0.02}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![
                (0., over_color),
            ])
        ])
    }
    
    pub fn draw_log_path(&mut self, cx: &mut Cx, path: &str, row: usize) {
        self.text.color = self.path_color.base(cx);
        self.text.draw_text(cx, &format!("{}:{} - ", path, row));
    }
    
    pub fn draw_log_body(&mut self, cx: &mut Cx, body: &str) {
        self.text.color = self.message_color.base(cx);
        if body.len()>500 {
            self.text.draw_text(cx, &body[0..500]);
        }
        else {
            self.text.draw_text(cx, &body);
        }
    }
    
    pub fn draw_log_item(&mut self, cx: &mut Cx, index: usize, list_item: &mut ListItem, log_item: &HubLogItem) {
        
        list_item.animator.init(cx, | cx | LogItemDraw::get_default_anim(cx, index, false));
        
        self.item_bg.color = list_item.animator.last_color(cx, Quad::instance_color());
        
        let bg_inst = self.item_bg.begin_quad(cx, Self::layout_item().base(cx)); //&self.get_line_layout());
        
        match log_item {
            HubLogItem::LocPanic(loc_msg) => {
                self.code_icon.draw_icon(cx, CodeIconType::Panic);
                cx.turtle_align_y();
                self.draw_log_path(cx, &loc_msg.path, loc_msg.row);
                self.draw_log_body(cx, &loc_msg.body);
                
            },
            HubLogItem::LocError(loc_msg) => {
                self.code_icon.draw_icon(cx, CodeIconType::Error);
                cx.turtle_align_y();
                self.draw_log_path(cx, &loc_msg.path, loc_msg.row);
                self.draw_log_body(cx, &loc_msg.body);
            },
            HubLogItem::LocWarning(loc_msg) => {
                self.code_icon.draw_icon(cx, CodeIconType::Warning);
                cx.turtle_align_y();
                self.draw_log_path(cx, &loc_msg.path, loc_msg.row);
                self.draw_log_body(cx, &loc_msg.body);
            },
            HubLogItem::LocMessage(loc_msg) => {
                self.draw_log_path(cx, &loc_msg.path, loc_msg.row);
                self.draw_log_body(cx, &loc_msg.body);
            },
            HubLogItem::Error(msg) => {
                self.code_icon.draw_icon(cx, CodeIconType::Error);
                cx.turtle_align_y();
                self.draw_log_body(cx, &msg);
            },
            HubLogItem::Warning(msg) => {
                self.code_icon.draw_icon(cx, CodeIconType::Warning);
                cx.turtle_align_y();
                self.draw_log_body(cx, &msg);
            },
            HubLogItem::Message(msg) => {
                self.draw_log_body(cx, &msg);
            }
        }
        
        let bg_area = self.item_bg.end_quad(cx, &bg_inst);
        list_item.animator.set_area(cx, bg_area);
    }
    
    pub fn draw_status_line(&mut self, cx: &mut Cx, counter: usize, bm: &BuildManager) {
        // draw status line
        self.item_bg.color = if counter & 1 == 0 {Theme::color_bg_selected().base(cx)}else {Theme::color_bg_odd().base(cx)};
        let bg_inst = self.item_bg.begin_quad(cx, Self::layout_item().base(cx));
        
        if !bm.is_any_cargo_running() {
            self.text.color = self.path_color.base(cx);
            self.code_icon.draw_icon(cx, CodeIconType::Ok);
            cx.turtle_align_y();
            if bm.is_any_artifact_running() {
                self.text.draw_text(cx, "Running - ");
                for ab in &bm.active_builds {
                    if ab.run_uid.is_some() {
                        let bt = &ab.build_target;
                        self.text.draw_text(cx, &format!("{}/{}/{}:{} ", bt.workspace, bt.project, bt.package, bt.config));
                    }
                }
            }
            else {
                self.text.draw_text(cx, "Done ");
                for ab in &bm.active_builds {
                    let bt = &ab.build_target;
                    self.text.draw_text(cx, &format!("{}/{}/{}:{} ", bt.workspace, bt.project, bt.package, bt.config));
                }
            }
        }
        else {
            self.code_icon.draw_icon(cx, CodeIconType::Wait);
            cx.turtle_align_y();
            self.text.color = self.path_color.base(cx);
            self.text.draw_text(cx, &format!("Building ({}) ", bm.artifacts.len()));
            for ab in &bm.active_builds {
                if ab.build_uid.is_some() {
                    let bt = &ab.build_target;
                    self.text.draw_text(cx, &format!("{}/{}/{}:{} ", bt.workspace, bt.project, bt.package, bt.config));
                }
            }
            if bm.exec_when_done {
                self.text.draw_text(cx, " - starting when done");
            }
        }
        self.item_bg.end_quad(cx, &bg_inst);
    }
    
    pub fn draw_filler(&mut self, cx: &mut Cx, counter: usize) {
        let view_total = cx.get_turtle_bounds();
        self.item_bg.color = if counter & 1 == 0 {Theme::color_bg_selected().base(cx)} else {Theme::color_bg_odd().base(cx)};
        self.item_bg.draw_quad(cx, Self::layout_item().base(cx).walk);
        cx.set_turtle_bounds(view_total); // do this so it doesnt impact the turtle
    }
}

#[derive(Clone)]
pub enum LogListEvent {
    SelectLocMessage {
        loc_message: LocMessage,
    },
    SelectMessages {
        items: String
    },
    None,
}

impl LogList {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            item_draw: LogItemDraw::proto(cx),
            list: ListLogic {
                tail_list: true,
                ..ListLogic::default()
            },
            view: ScrollView::proto(cx),
        }
    }
    
    pub fn theme(cx: &mut Cx) {
        LogItemDraw::theme(cx);
    }
    
    pub fn handle_log_list(&mut self, cx: &mut Cx, event: &mut Event, storage: &mut AppStorage, bm: &mut BuildManager) -> LogListEvent {
        
        if bm.log_items.len() < self.list.list_items.len() {
            self.list.tail_list = true;
        }
        
        self.list.set_list_len(bm.log_items.len());
        
        self.list.handle_list_scroll_bars(cx, event, &mut self.view);
        
        let mut select = ListSelect::None;
        let mut select_at_end = false;
        // global key handle
        match event {
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::Period => if ke.modifiers.logo || ke.modifiers.control {
                    select = self.list.get_next_single_selection();
                    self.list.scroll_item_in_view = select.item_index();
                    self.list.tail_list = false;
                    select_at_end = ke.modifiers.shift;
                },
                KeyCode::Comma => if ke.modifiers.logo || ke.modifiers.control {
                    // lets find the
                    select = self.list.get_prev_single_selection();
                    self.list.tail_list = false;
                    self.list.scroll_item_in_view = select.item_index();
                    select_at_end = ke.modifiers.shift;
                },
                KeyCode::KeyM => if ke.modifiers.logo || ke.modifiers.control {
                    select = ListSelect::All;
                },
                KeyCode::KeyT => if ke.modifiers.logo || ke.modifiers.control {
                    // lock scroll
                    self.list.tail_list = true;
                    self.view.redraw_view_area(cx);
                },
                KeyCode::KeyK => if ke.modifiers.logo || ke.modifiers.control {
                    // clear and tail log
                    self.list.tail_list = true;
                    bm.log_items.truncate(0);
                    self.view.redraw_view_area(cx);
                },
                KeyCode::Backtick => if ke.modifiers.logo || ke.modifiers.control {
                    if bm.active_builds.len() == 0 {
                        bm.restart_build(cx, storage);
                    }
                    bm.artifact_run(storage);
                    self.view.redraw_view_area(cx);
                },
                _ => ()
            },
            Event::Signal(se) => if bm.signal.is_signal(se) {
                // we have new things
                self.view.redraw_view_area(cx);
                //println!("SIGNAL!");
            },
            _ => ()
        }
        
        let le = self.list.handle_list_logic(cx, event, select, | cx, item_event, item, item_index | match item_event {
            ListLogicEvent::Animate(ae) => {
                item.animator.calc_area(cx, item.animator.area, ae.time);
            },
            ListLogicEvent::AnimEnded => {
                item.animator.end();
            },
            ListLogicEvent::Select => {
                item.animator.play_anim(cx, LogItemDraw::get_over_anim(cx, item_index, true));
            },
            ListLogicEvent::Deselect => {
                item.animator.play_anim(cx, LogItemDraw::get_default_anim(cx, item_index, false));
            },
            ListLogicEvent::Cleanup => {
                item.animator.play_anim(cx, LogItemDraw::get_default_anim_cut(cx, item_index, item.is_selected));
            },
            ListLogicEvent::Over => {
                item.animator.play_anim(cx, LogItemDraw::get_over_anim(cx, item_index, item.is_selected));
            },
            ListLogicEvent::Out => {
                item.animator.play_anim(cx, LogItemDraw::get_default_anim(cx, item_index, item.is_selected));
            }
        });
        
        match le {
            ListEvent::SelectSingle(select_index) => {
                self.view.redraw_view_area(cx);
                let log_item = &bm.log_items[select_index];
                if let Some(loc_message) = log_item.get_loc_message() {
                    if loc_message.path.len() == 0 {
                        return LogListEvent::SelectLocMessage {
                            loc_message: loc_message.clone(),
                        }
                    }
                    let text_buffer = storage.text_buffer_from_path(cx, &loc_message.path);
                    // check if we have a range:
                    if let Some((head, tail)) = loc_message.range {
                        if select_at_end {
                            text_buffer.messages.jump_to_offset = head;
                        }
                        else {
                            text_buffer.messages.jump_to_offset = tail;
                        }
                    }
                    else {
                        text_buffer.messages.jump_to_offset = text_buffer.text_pos_to_offset(TextPos {row: loc_message.row.max(1) - 1, col: loc_message.col.max(1) - 1})
                    }
                    cx.send_signal(text_buffer.signal, SIGNAL_TEXTBUFFER_JUMP_TO_OFFSET);
                    
                    LogListEvent::SelectLocMessage {
                        loc_message: loc_message.clone(),
                    }
                }
                else {
                    LogListEvent::SelectMessages {
                        items: log_item.get_body().clone(),
                    }
                }
            },
            ListEvent::SelectMultiple => {
                self.view.redraw_view_area(cx);
                let mut items = String::new();
                for select in &self.list.selection {
                    if let Some(loc_message) = bm.log_items[*select].get_loc_message() {
                        if let Some(rendered) = &loc_message.rendered {
                            items.push_str(rendered);
                            if items.len()>1000000 { // safety break
                                break;
                            }
                        }
                    }
                    else {
                        items.push_str(bm.log_items[*select].get_body());
                        if items.len()>1000000 { // safety break
                            break;
                        }
                    }
                }
                
                LogListEvent::SelectMessages {
                    items: items,
                }
            },
            ListEvent::None => {
                LogListEvent::None
            }
        }
    }
    
    pub fn draw_log_list(&mut self, cx: &mut Cx, bm: &BuildManager) {
        
        self.list.set_list_len(bm.log_items.len());
        
        self.item_draw.text.text_style = LogItemDraw::text_style_item().base(cx);
        
        let row_height = LogItemDraw::layout_item().base(cx).walk.height.fixed();
        
        if self.list.begin_list(cx, &mut self.view, row_height).is_err() {return}
        
        let mut counter = 0;
        for i in self.list.start_item..self.list.end_item {
            self.item_draw.draw_log_item(cx, i, &mut self.list.list_items[i], &bm.log_items[i]);
            counter += 1;
        }
        
        self.list.walk_turtle_to_end(cx, row_height);
        
        self.item_draw.draw_status_line(cx, counter, &bm);
        counter += 1;
        
        // draw filler nodes
        for _ in (self.list.end_item + 1)..self.list.end_fill {
            self.item_draw.draw_filler(cx, counter);
            counter += 1;
        }
        
        self.list.end_list(cx, &mut self.view);
    }
}
