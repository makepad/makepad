use render::*;
use widget::*;
use editor::*;
use hub::*;
use crate::app::*;
use crate::buildmanager::*;

#[derive(Clone)]
pub struct LogList {
    pub view: ScrollView,
    pub item_draw: LogItemDraw,
    pub list: List
}

#[derive(Clone)]
pub struct LogItemDraw {
    pub bg: Quad,
    pub text: Text,
    pub item_bg: Quad,
    pub code_icon: CodeIcon,
    pub row_height: f32,
    pub path_color: Color,
    pub message_color: Color,
    pub bg_even: Color,
    pub bg_odd: Color,
    pub bg_marked: Color,
    pub bg_odd_over: Color,
    pub bg_marked_over: Color,
    pub bg_selected: Color,
    pub bg_selected_over: Color
}

impl LogItemDraw {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            bg: Quad::style(cx),
            item_bg: Quad::style(cx),
            text: Text {
                wrapping: Wrapping::Word,
                ..Text::style(cx)
            },
            code_icon: CodeIcon::style(cx),
            path_color: color("#999"),
            message_color: color("#bbb"),
            row_height: 20.0,
            bg_even: cx.color("bg_selected"),
            bg_odd: cx.color("bg_odd"),
            bg_marked: cx.color("bg_marked"),
            bg_selected: cx.color("bg_selected"),
            bg_marked_over: cx.color("bg_marked_over"),
            bg_selected_over: cx.color("bg_selected_over"),
            bg_odd_over: cx.color("bg_odd_over")
        }
    }
    
    pub fn get_default_anim(&self, cx: &Cx, counter: usize, marked: bool) -> Anim {
        Anim::new(Play::Chain {duration: 0.01}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![
                (1.0, if marked {self.bg_marked} else if counter & 1 == 0 {self.bg_selected}else {self.bg_odd})
            ])
        ])
    }
    
    pub fn get_default_anim_cut(&self, cx: &Cx, counter: usize, marked: bool) -> Anim {
        Anim::new(Play::Cut {duration: 0.01}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![
                (0.0, if marked {self.bg_marked} else if counter & 1 == 0 {self.bg_selected}else {self.bg_odd})
            ])
        ])
    }
    
    pub fn get_over_anim(&self, cx: &Cx, counter: usize, marked: bool) -> Anim {
        let over_color = if marked {self.bg_marked_over} else if counter & 1 == 0 {self.bg_selected_over}else {self.bg_odd_over};
        Anim::new(Play::Cut {duration: 0.02}, vec![
            Track::color(cx.id("bg.color"), Ease::Lin, vec![
                (0., over_color),
            ])
        ])
    }
    
    pub fn get_line_layout(&self) -> Layout {
        Layout {
            width: Bounds::Fill,
            height: Bounds::Fix(self.row_height),
            padding: Padding {l: 2., t: 3., b: 2., r: 0.},
            line_wrap: LineWrap::None,
            ..Default::default()
        }
    }
    
    pub fn draw_log_item(&mut self, cx: &mut Cx, list_item: &mut ListItem, log_item: &HubLogItem) {
        self.item_bg.color = list_item.animator.last_color(cx.id("bg.color"));
        let bg_inst = self.item_bg.begin_quad(cx, &self.get_line_layout());
        
        match log_item.level {
            HubLogItemLevel::Error => {
                self.code_icon.draw_icon_walk(cx, CodeIconType::Error);
            },
            HubLogItemLevel::Warning => {
                self.code_icon.draw_icon_walk(cx, CodeIconType::Warning);
            },
            HubLogItemLevel::Panic => {
                self.code_icon.draw_icon_walk(cx, CodeIconType::Panic);
            },
            HubLogItemLevel::Log => {
                //self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
            }
        }
        
        if let Some(path) = &log_item.path {
            self.text.color = self.path_color;
            self.text.draw_text(cx, &format!("{}:{} - ", path, log_item.row));
        }
        self.text.color = self.message_color;
        self.text.draw_text(cx, &format!("{}", log_item.body));
        
        let bg_area = self.item_bg.end_quad(cx, &bg_inst);
        list_item.animator.update_area_refs(cx, bg_area);
    }
    
    pub fn draw_status_line(&mut self, cx: &mut Cx, counter: usize, bm: &BuildManager) {
        // draw status line
        self.item_bg.color = if counter & 1 == 0 {self.bg_even}else {self.bg_odd};
        let bg_inst = self.item_bg.begin_quad(cx, &self.get_line_layout());
        
        if !bm.is_any_cargo_running() {
            self.text.color = self.path_color;
            self.code_icon.draw_icon_walk(cx, CodeIconType::Ok);
            if bm.is_any_artifact_running() {
                self.text.draw_text(cx, "Running");
            }
            else {
                self.text.draw_text(cx, "Done");
            }
        }
        else {
            self.code_icon.draw_icon_walk(cx, CodeIconType::Wait);
            self.text.color = self.path_color;
            self.text.draw_text(cx, &format!("Building ({})", bm.artifacts.len()));
            if bm.exec_when_done {
                self.text.draw_text(cx, " - starting when done");
            }
        }
        self.item_bg.end_quad(cx, &bg_inst);
    }
    
    pub fn draw_filler(&mut self, cx:&mut Cx, counter:usize){
        let view_total = cx.get_turtle_bounds();
        self.item_bg.color = if counter & 1 == 0 {self.bg_even} else {self.bg_odd};
        self.item_bg.draw_quad_walk(cx, Bounds::Fill, Bounds::Fix(self.row_height), Margin::zero());
        cx.set_turtle_bounds(view_total); // do this so it doesnt impact the turtle
    }
}

#[derive(Clone)]
pub enum LogListEvent {
    SelectLogItem {
        path: Option<String>,
        item: Option<String>,
        level: HubLogItemLevel
    },
    SelectLogRange {
        items: String
    },
    None,
}

impl LogList {
    pub fn style(cx: &mut Cx) -> Self {
        Self {
            item_draw: LogItemDraw::style(cx),
            list: List::default(),
            view: ScrollView::style_hor_and_vert(cx),
        }
    }
    
    pub fn handle_log_list(&mut self, cx: &mut Cx, event: &mut Event, storage: &mut AppStorage, bm:&mut BuildManager) -> LogListEvent {
        let item_draw = &self.item_draw;
        self.list.set_list_len(cx, bm.log_items.len(), |cx, index|{
            item_draw.get_default_anim(cx, index, false)
        });
        
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
                //KeyCode::Backtick => if ke.modifiers.logo || ke.modifiers.control {
                //    self.artifact_exec(storage);
                //    self.style.view.redraw_view_area(cx);
                //},
                _ => ()
            },
            Event::Signal(se)=>if bm.signal.is_signal(se){
                // we have new things
                self.view.redraw_view_area(cx);
                println!("SIGNAL!");
            },
            _ => ()
        }
        
        let item_draw = &self.item_draw;
        let le = self.list.handle_selection(cx, event, select, | cx, item_event, item, item_index | {
            match item_event {
                ListItemEvent::ItemAnimate(ae) => {
                    item.animator.write_area(cx, item.animator.area, "bg.", ae.time);
                },
                ListItemEvent::ItemAnimEnded => {
                    item.animator.end();
                },
                ListItemEvent::ItemSelect => {
                    item.animator.play_anim(cx, item_draw.get_over_anim(cx, item_index, true));
                },
                ListItemEvent::ItemDeselect => {
                    item.animator.play_anim(cx, item_draw.get_default_anim(cx, item_index, false));
                },
                ListItemEvent::ItemCleanup => {
                    item.animator.play_anim(cx, item_draw.get_default_anim_cut(cx, item_index, false));
                },
                ListItemEvent::ItemOver => {
                    item.animator.play_anim(cx, item_draw.get_over_anim(cx, item_index, item.is_selected));
                },
                ListItemEvent::ItemOut => {
                    item.animator.play_anim(cx, item_draw.get_default_anim(cx, item_index, item.is_selected));
                }
            }
        });
        
        match le {
            ListEvent::SelectSingle(select_index) => {
                self.view.redraw_view_area(cx);
                let log_item = &bm.log_items[select_index];
                if let Some(path) = &log_item.path {
                    let text_buffer = storage.text_buffer_from_path(cx, &path);
                    text_buffer.messages.jump_to_offset = if log_item.level == HubLogItemLevel::Log || log_item.level == HubLogItemLevel::Panic {
                        text_buffer.text_pos_to_offset(TextPos {row: log_item.row - 1, col: log_item.col - 1})
                    }
                    else {
                        if select_at_end {log_item.head}else {log_item.tail}
                    };
                    cx.send_signal(text_buffer.signal, SIGNAL_TEXTBUFFER_JUMP_TO_OFFSET);
                }
                let item = if let Some(rendered) = &log_item.rendered {
                    if let Some(explanation) = &log_item.explanation {
                        Some(format!("{}{}", rendered, explanation))
                    }
                    else {
                        Some(rendered.clone())
                    }
                }
                else {
                    None
                };
                
                LogListEvent::SelectLogItem {
                    item: item,
                    path: log_item.path.clone(),
                    level: log_item.level.clone()
                }
            },
            ListEvent::SelectMultiple => {
                self.view.redraw_view_area(cx);
                let mut items = String::new();
                for select in &self.list.selection {
                    if let Some(rendered) = &bm.log_items[*select].rendered {
                        items.push_str(rendered);
                        if items.len()>1000000 { // safety break
                            break;
                        }
                    }
                }
                
                LogListEvent::SelectLogRange {
                    items: items,
                }
            },
            ListEvent::None => {
                LogListEvent::None
            }
        }
    } 
    
    pub fn draw_log_list(&mut self, cx: &mut Cx, bm: &BuildManager) {
                println!("REDRAW!");
        let item_draw = &self.item_draw;
        self.list.set_list_len(cx, bm.log_items.len(), |cx, index|{
            item_draw.get_default_anim(cx, index, false)
        });
        
        if let Err(_) = self.list.begin_list(cx, &mut self.view, self.item_draw.row_height){
            return
        }
        
        let mut counter = 0;
        for i in self.list.start_item..self.list.end_item{
            self.item_draw.draw_log_item(cx, &mut self.list.list_items[i], &bm.log_items[i]);
            counter += 1;
        }
        
        self.list.walk_turtle_to_end(cx, self.item_draw.row_height);
        
        self.item_draw.draw_status_line(cx, counter, &bm);
        counter += 1;
        
        // draw filler nodes
        for _ in (self.list.end_item + 1)..self.list.end_fill{
            self.item_draw.draw_filler(cx, counter);
            counter += 1;
        }

        self.list.end_list(cx, &mut self.view);
    }
}
