use makepad_render::*;
use makepad_widget::*;
use crate::codeicon::*;
use crate::searchindex::*;
use crate::appstorage::*;

#[derive(Clone)]
pub struct SearchResults {
    pub view: ScrollView,
    pub result_draw: SearchResultDraw,
    pub list: ListLogic,
    pub search_input: TextInput,
    pub do_select_first: bool,
    pub first_tbid: AppTextBufferId,
    pub results: Vec<SearchResult>
}

#[derive(Clone)]
pub struct SearchResultDraw {
    pub text_editor: TextEditor,
    pub text: Text,
    pub item_bg: Quad,
    pub code_icon: CodeIcon,
    pub shadow: ScrollShadow,
}

#[derive(Clone)]
pub enum SearchResultEvent {
    DisplayFile {
        text_buffer_id: AppTextBufferId,
        cursor: (usize, usize)
    },
    OpenFile {
        text_buffer_id: AppTextBufferId,
        cursor: (usize, usize)
    },
    None,
}

impl SearchResults {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            first_tbid: AppTextBufferId(0),
            search_input: TextInput::new(cx, TextInputOptions {multiline: false, read_only: false, empty_message: "search".to_string()}),
            result_draw: SearchResultDraw::new(cx),
            list: ListLogic {
                multi_select: false,
                ..ListLogic::default()
            },
            do_select_first: false,
            view: ScrollView::new(cx),
            results: Vec::new(),
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live!(cx, r#"
            self::color_path: #9;
            self::color_message: #b;
            self::color_bg: #1e;
            self::color_bg_marked: #11466e;
            self::color_bg_odd: #25;
            self::color_bg_marked_over: #11466e;
            self::color_bg_selected: #28;
            
            self::style_text_input: Style {
                makepad_widget::texteditor::layout_bg: Layout {
                    walk: Walk {
                        width: Compute,
                        height: Compute,
                        margin: {t: 6., l: 0., r: 0., b: 0.}
                    },
                    padding: all(7.),
                }
                
                makepad_widget::texteditor::color_bg: #34;
            }
            
            self::layout_item_closed: Layout {
                walk: {width: ComputeFill, height: Fix(37.0)},
                align: {fx: 0.0, fy: 0.0},
                padding: {l: 5., t: 3., b: 2., r: 0.},
                line_wrap:None,
            }
            
            self::layout_item_open: Layout {
                walk: {width: ComputeFill, height: Fix(85.)},
                align: {fx: 0.0, fy: 0.0},
                padding: {l: 5., t: 3., b: 2., r: 0.},
                line_wrap: None,
            }
            
            self::text_style_item: TextStyle {
                ..makepad_widget::widgetstyle::text_style_normal
            }
            
            self::style_text_editor: Style {
                makepad_widget::texteditor::gutter_width: 10.;
                makepad_widget::texteditor::padding_top: 0.;
            }
            
        "#)
    }
    
    pub fn set_search_input_value(&mut self, cx: &mut Cx, value: &str, first_tbid: AppTextBufferId, focus: bool) {
        self.search_input.set_value(cx, value);
        self.first_tbid = first_tbid;
        if focus {
            self.search_input.text_editor.set_key_focus(cx);
        }
        self.search_input.select_all(cx);
    }
    
    pub fn do_search(&mut self, cx: &mut Cx, search_index: &mut SearchIndex, storage: &mut AppStorage) -> Option<(AppTextBufferId, (usize, usize))> {
        let s = self.search_input.get_value();
        if s.len() > 0 {
            // lets search
            self.results = search_index.search(&s, self.first_tbid, cx, storage);
            self.do_select_first = true;
        }
        else {
            search_index.clear_markers(cx, storage);
            self.results.truncate(0);
        }
        self.list.set_list_len(0);
        self.view.redraw_view_area(cx);
        if self.results.len()>0 {
            let result = &self.results[0];
            let text_buffer = &mut storage.text_buffers[result.text_buffer_id.as_index()].text_buffer;
            let tok = &text_buffer.token_chunks[result.token as usize];
            Some((result.text_buffer_id, (tok.offset + tok.len, tok.offset)))
        }
        else {
            None
        }
    }
    
    pub fn handle_search_input(&mut self, cx: &mut Cx, event: &mut Event, search_index: &mut SearchIndex, storage: &mut AppStorage) -> bool {
        // if we have a text change, do a search.
        match self.search_input.handle_text_input(cx, event) {
            TextEditorEvent::KeyFocus => {
                return true
            },
            TextEditorEvent::Change => {
                self.do_search(cx, search_index, storage);
                return true
            },
            TextEditorEvent::Escape | TextEditorEvent::Search(_) => {
                cx.revert_key_focus();
            },
            _ => ()
        }
        return false
    }
    
    pub fn handle_search_results(&mut self, cx: &mut Cx, event: &mut Event, _search_index: &mut SearchIndex, storage: &mut AppStorage) -> SearchResultEvent {
        
        self.list.set_list_len(self.results.len());
        
        if self.list.handle_list_scroll_bars(cx, event, &mut self.view) {
        }
        
        let mut select = ListSelect::None;
        let mut dblclick = false;
        // global key handle
        match event {
            Event::KeyDown(ke) => if self.search_input.text_editor.has_key_focus(cx) {
                match ke.key_code {
                    KeyCode::ArrowDown => {
                        select = self.list.get_next_single_selection();
                        self.list.scroll_item_in_view = select.item_index();
                    },
                    KeyCode::ArrowUp => {
                        // lets find the
                        select = self.list.get_prev_single_selection();
                        self.list.scroll_item_in_view = select.item_index();
                    },
                    KeyCode::Return => {
                        if self.list.selection.len()>0 {
                            select = ListSelect::Single(self.list.selection[0]);
                            dblclick = true;
                        }
                    },
                    _ => ()
                }
            },
            _ => ()
        }
        
        if self.do_select_first {
            self.do_select_first = false;
            select = ListSelect::Single(0);
        }
        
        let le = self.list.handle_list_logic(cx, event, select, dblclick, | cx, item_event, item, _item_index | match item_event {
            ListLogicEvent::Animate(ae) => {
                item.animator.calc_area(cx, item.animator.area, ae.time);
            },
            ListLogicEvent::AnimEnded => {
                item.animator.end();
            },
            ListLogicEvent::Select => {
                item.animator.play_anim(cx, SearchResultDraw::get_over_anim(cx, true));
            },
            ListLogicEvent::Deselect => {
                item.animator.play_anim(cx, SearchResultDraw::get_default_anim(cx, false));
            },
            ListLogicEvent::Cleanup => {
                item.animator.play_anim(cx, SearchResultDraw::get_default_anim_cut(cx, item.is_selected));
            },
            ListLogicEvent::Over => {
                item.animator.play_anim(cx, SearchResultDraw::get_over_anim(cx, item.is_selected));
            },
            ListLogicEvent::Out => {
                item.animator.play_anim(cx, SearchResultDraw::get_default_anim(cx, item.is_selected));
            }
        });
        
        match le {
            ListEvent::SelectSingle(select_index) => {
                self.view.redraw_view_area(cx);
                let result = &self.results[select_index];
                let text_buffer = &mut storage.text_buffers[result.text_buffer_id.as_index()].text_buffer;
                if let Event::FingerDown(_) = event {
                    self.search_input.text_editor.set_key_focus(cx);
                }
                let tok = &text_buffer.token_chunks[result.token as usize];
                return SearchResultEvent::DisplayFile {
                    text_buffer_id: result.text_buffer_id, //storage.text_buffer_id_to_path.get(&result.text_buffer_id).expect("Path not found").clone(),
                    cursor: (tok.offset + tok.len, tok.offset)
                };
            },
            ListEvent::SelectDouble(select_index) => {
                // we need to get a filepath
                let result = &self.results[select_index];
                let text_buffer = &mut storage.text_buffers[result.text_buffer_id.as_index()].text_buffer;
                let tok = &text_buffer.token_chunks[result.token as usize];
                return SearchResultEvent::OpenFile {
                    text_buffer_id: result.text_buffer_id,
                    cursor: (tok.offset + tok.len, tok.offset)
                };
            },
            ListEvent::SelectMultiple => {},
            ListEvent::None => {
            }
        }
        SearchResultEvent::None
    }
    
    pub fn draw_search_result_tab(&mut self, cx: &mut Cx, _search_index: &SearchIndex) {
        live_style_begin!(cx, self::style_text_input);
        self.search_input.draw_text_input(cx);
        live_style_end!(cx, self::style_text_input);
    }
    
    pub fn draw_search_results(&mut self, cx: &mut Cx, storage: &AppStorage) {
        
        self.list.set_list_len(self.results.len()); //bm.log_items.len());
        
        self.result_draw.text.text_style = live_text_style!(cx, self::text_style_item);
        
        let row_height = live_layout!(cx, self::layout_item_closed).walk.height.fixed();
        
        if self.list.begin_list(cx, &mut self.view, false, row_height).is_err() {return}
        
        cx.new_instance_draw_call(self.result_draw.item_bg.shader, None, 0);
        
        live_style_begin!(cx, self::style_text_editor);
        self.result_draw.text_editor.apply_style(cx);
        self.result_draw.text_editor.new_draw_calls(cx, false);
        live_style_end!(cx, self::style_text_editor);
        
        let mut counter = 0;
        for i in self.list.start_item..self.list.end_item {
            // lets get the path
            let result = &self.results[i];
            let tb = &storage.text_buffers[result.text_buffer_id.as_index()];
            //println!("{} {}");
            self.result_draw.draw_result(
                cx,
                i,
                &mut self.list.list_items[i],
                &tb.full_path,
                &tb.text_buffer,
                result.token
                
            );
            counter += 1;
        }
        
        self.list.walk_turtle_to_end(cx, row_height);
        
        // draw filler nodes
        for _ in (self.list.end_item + 1)..self.list.end_fill {
            self.result_draw.draw_filler(cx, counter);
            counter += 1;
        }
        
        self.result_draw.shadow.draw_shadow_left(cx);
        self.result_draw.shadow.draw_shadow_top(cx);
        
        self.list.end_list(cx, &mut self.view);
    }
}

impl SearchResultDraw {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            text_editor: TextEditor {
                mark_unmatched_parens: false,
                draw_line_numbers: false,
                ..TextEditor::new(cx)
            },
            item_bg: Quad {z: 0.0001, ..Quad::new(cx)},
            text: Text {
                wrapping: Wrapping::Word,
                ..Text::new(cx)
            },
            code_icon: CodeIcon::new(cx),
            shadow: ScrollShadow {z: 0.01, ..ScrollShadow::new(cx)},
        }
    }
    
    
    pub fn get_default_anim(cx: &Cx, marked: bool) -> Anim {
        
        let default_color = if marked {
            live_color!(cx, self::color_bg_marked)
        } else {
            live_color!(cx, self::color_bg)
        };
        Anim {
            play: Play::Chain {duration: 0.01},
            tracks: vec![
                Track::Color {
                    live_id: live_id!(makepad_render::quad::shader::color),
                    ease: Ease::Lin,
                    keys: vec![(1.0, default_color)],
                    cut_init: None
                }
            ]
        }
    }
    
    pub fn get_default_anim_cut(cx: &Cx, marked: bool) -> Anim {
        Anim {
            play: Play::Cut {duration: 0.01},
            ..Self::get_default_anim(cx, marked)
        }
    }
    
    pub fn get_over_anim(cx: &Cx, marked: bool) -> Anim {
        let over_color = if marked {
            live_color!(cx, self::color_bg_marked_over)
        } else {
            live_color!(cx, self::color_bg_odd_over)
        };
        Anim {
            play: Play::Chain {duration: 0.02},
            tracks: vec![
                Track::Color {
                    live_id: live_id!(makepad_render::quad::shader::color),
                    ease: Ease::Lin,
                    keys: vec![(0.0, over_color)],
                    cut_init: None
                }
            ]
        }
    }
    
    pub fn draw_result(
        &mut self,
        cx: &mut Cx,
        _index: usize,
        list_item: &mut ListItem,
        path: &str,
        text_buffer: &TextBuffer,
        token: u32
    ) {
        let selected = list_item.is_selected;
        list_item.animator.init(cx, | cx | Self::get_default_anim(cx, selected));
        
        self.item_bg.color = list_item.animator.last_color(cx, live_id!(makepad_render::quad::shader::color));
        
        let bg_inst = self.item_bg.begin_quad(cx, if selected {
            live_layout!(cx, self::layout_item_open)
        }else {
            live_layout!(cx, self::layout_item_closed)
        }); //&self.get_line_layout());
        
        let window_up = if selected {2} else {1};
        let window_down = if selected {3} else {1};
        let (first_tok, delta) = text_buffer.scan_token_chunks_prev_line(token as usize, window_up);
        let last_tok = text_buffer.scan_token_chunks_next_line(token as usize, window_down);
        
        let tok = &text_buffer.token_chunks[token as usize];
        let pos = text_buffer.offset_to_text_pos(tok.offset);
        
        self.text.color = live_color!(cx, self::color_path);
        let split = path.split('/').collect::<Vec<&str >> ();
        self.text.draw_text(cx, &format!("{}:{} - {}", split.last().unwrap(), pos.row, split[0..split.len() - 1].join("/")));
        cx.turtle_new_line();
        cx.move_turtle(0., 5.);
        
        self.text_editor.search_markers_bypass.truncate(0);
        self.text_editor.search_markers_bypass.push(TextCursor {
            tail: tok.offset,
            head: tok.offset + tok.len,
            max: 0
        });
        
        self.text_editor.line_number_offset = (pos.row as isize + delta) as usize;
        self.text_editor.init_draw_state(cx, text_buffer);
        
        let mut first_ws = !selected;
        for index in first_tok..last_tok {
            let token_chunk = &text_buffer.token_chunks[index];
            if first_ws && token_chunk.token_type == TokenType::Whitespace {
                continue;
            }
            else {
                first_ws = false;
            }
            self.text_editor.draw_chunk(cx, index, &text_buffer.flat_text, token_chunk, &text_buffer.markers);
        }
        
        self.text_editor.draw_search_markers(cx);
        // ok now we have to draw a code bubble
        // its the 3 lines it consists of so.. we have to scan 'back from token to find the previous start
        // and scan to end
        
        //println!("{}", result.text_buffer_id.0);
        
        let bg_area = self.item_bg.end_quad(cx, bg_inst);
        list_item.animator.set_area(cx, bg_area);
        
    }
    
    pub fn draw_filler(&mut self, cx: &mut Cx, counter: usize) {
        let view_total = cx.get_turtle_bounds();
        self.item_bg.color = if counter & 1 == 0 {
            live_color!(cx, self::color_bg_selected)
        } else {
            live_color!(cx, self::color_bg_odd)
        };
        self.item_bg.draw_quad(cx, live_layout!(cx, self::layout_item_closed).walk);
        cx.set_turtle_bounds(view_total); // do this so it doesnt impact the turtle
    }
}

