use makepad_render::*;
use makepad_widget::*;
use makepad_hub::*;
use crate::codeicon::*;
use crate::searchindex::*;
use crate::appstorage::*;

#[derive(Clone)]
pub struct SearchResults {
    pub view: ScrollView,
    pub result_draw: SearchResultDraw,
    pub list: ListLogic,
    pub search_input: TextInput,
    pub results: Vec<SearchResult>
}

#[derive(Clone)]
pub struct SearchResultDraw {
    pub text_editor: TextEditor,
    pub text: Text,
    pub item_bg: Quad,
    pub code_icon: CodeIcon,
    pub path_color: ColorId,
    pub message_color: ColorId,
    pub shadow: ScrollShadow,
}

impl SearchResultDraw {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            text_editor: TextEditor::proto(cx),
            item_bg: Quad::proto(cx),
            text: Text {
                wrapping: Wrapping::Word,
                ..Text::proto(cx)
            },
            code_icon: CodeIcon::proto(cx),
            path_color: Theme::color_text_defocus(),
            message_color: Theme::color_text_focus(),
            shadow: ScrollShadow {z: 0.01, ..ScrollShadow::proto(cx)},
        }
    }
    
    pub fn layout_item() -> LayoutId {uid!()}
    pub fn text_style_item() -> TextStyleId {uid!()}
    pub fn layout_search_input() -> LayoutId {uid!()}
    
    pub fn style(cx: &mut Cx, opt: &StyleOptions) {
        
        Self::layout_item().set(cx, Layout {
            walk: Walk::wh(Width::Fill, Height::Fix(20. * opt.scale)),
            align: Align::left_center(),
            padding: Padding::zero(), // {l: 2., t: 3., b: 2., r: 0.},
            line_wrap: LineWrap::None,
            ..Default::default()
        });
        
        Self::text_style_item().set(cx, Theme::text_style_normal().get(cx));
    }
    
    pub fn get_default_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        Anim::new(Play::Chain {duration: 0.01}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![
                (1.0, if marked {Theme::color_bg_marked().get(cx)} else if counter & 1 == 0 {Theme::color_bg_selected().get(cx)}else {Theme::color_bg_odd().get(cx)})
            ])
        ])
    }
    
    pub fn get_default_anim_cut(cx: &Cx, counter: usize, marked: bool) -> Anim {
        Anim::new(Play::Cut {duration: 0.01}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![
                (0.0, if marked {Theme::color_bg_marked().get(cx)} else if counter & 1 == 0 {Theme::color_bg_selected().get(cx)}else {Theme::color_bg_odd().get(cx)})
            ])
        ])
    }
    
    pub fn get_over_anim(cx: &Cx, counter: usize, marked: bool) -> Anim {
        let over_color = if marked {Theme::color_bg_marked_over().get(cx)} else if counter & 1 == 0 {Theme::color_bg_selected_over().get(cx)}else {Theme::color_bg_odd_over().get(cx)};
        Anim::new(Play::Cut {duration: 0.02}, vec![
            Track::color(Quad::instance_color(), Ease::Lin, vec![
                (0., over_color),
            ])
        ])
    }
    
    pub fn draw_result(
        &mut self,
        cx: &mut Cx,
        index: usize,
        list_item: &mut ListItem,
        path: &str,
        text_buffer: &TextBuffer,
        token: u16
    ) {
        list_item.animator.init(cx, | cx | Self::get_default_anim(cx, index, false));
        
        self.item_bg.color = list_item.animator.last_color(cx, Quad::instance_color());
        
        let bg_inst = self.item_bg.begin_quad(cx, Self::layout_item().get(cx)); //&self.get_line_layout());
        
        let tok = &text_buffer.token_chunks[token as usize];
        let pos = text_buffer.offset_to_text_pos(tok.offset);
        self.text.color = self.path_color.get(cx);
        self.text.draw_text(cx, &format!("{}:{}", path, pos.row));
        // ok now we have to draw a code bubble
        
        //println!("{}", result.text_buffer_id.0);
        
        let bg_area = self.item_bg.end_quad(cx, &bg_inst);
        list_item.animator.set_area(cx, bg_area);
        
    }
    
    pub fn draw_filler(&mut self, cx: &mut Cx, counter: usize) {
        let view_total = cx.get_turtle_bounds();
        self.item_bg.color = if counter & 1 == 0 {Theme::color_bg_selected().get(cx)} else {Theme::color_bg_odd().get(cx)};
        self.item_bg.draw_quad(cx, Self::layout_item().get(cx).walk);
        cx.set_turtle_bounds(view_total); // do this so it doesnt impact the turtle
    }
}

#[derive(Clone)]
pub enum SearchResultEvent {
    Select {
        loc_message: LocMessage,
    },
    None,
}

impl SearchResults {
    pub fn proto(cx: &mut Cx) -> Self {
        Self {
            search_input: TextInput::proto(cx, TextInputOptions::default()),
            result_draw: SearchResultDraw::proto(cx),
            list: ListLogic {
                ..ListLogic::default()
            },
            view: ScrollView::proto(cx),
            results: Vec::new(),
        }
    }
    
    pub fn style_text_input() -> StyleId {uid!()}
    
    pub fn style(cx: &mut Cx, opt: &StyleOptions) {
        cx.begin_style(Self::style_text_input());
        TextEditor::layout_bg().set(cx, Layout {
            walk: Walk {width: Width::Compute, height: Height::Compute, margin: Margin {t: 4., l: 14., r: 0., b: 0.}},
            padding: Padding::all(7.),
            ..Layout::default()
        });
        cx.end_style();
        
        SearchResultDraw::style(cx, opt);
    }
    
    pub fn handle_search_results(&mut self, cx: &mut Cx, event: &mut Event, search_index: &mut SearchIndex, storage: &AppStorage) -> SearchResultEvent {
        
        // if we have a text change, do a search.
        if let TextEditorEvent::Change = self.search_input.handle_text_input(cx, event) {
            let s = self.search_input.get_value();
            if s.len() > 0{
                // lets search
                self.results = search_index.begins_with(&s, storage);
                /*self.results.sort_by(|a, b| {
                    let cmp = a.text_buffer_id.cmp(&b.text_buffer_id);
                    if let std::cmp::Ordering::Equal = cmp{
                        return a.token.cmp(&b.token)
                    }
                    return cmp
                })*/
            }
            else{
                self.results.truncate(0);
            }
            self.view.redraw_view_area(cx);
        }
        
        self.list.set_list_len(self.results.len());
        
        if self.list.handle_list_scroll_bars(cx, event, &mut self.view) {
        }
        
        let mut select = ListSelect::None;
        // global key handle
        match event {
            Event::KeyDown(ke) => match ke.key_code {
                KeyCode::RBracket => if ke.modifiers.logo || ke.modifiers.control {
                    select = self.list.get_next_single_selection();
                    self.list.scroll_item_in_view = select.item_index();
                },
                KeyCode::LBracket => if ke.modifiers.logo || ke.modifiers.control {
                    // lets find the
                    select = self.list.get_prev_single_selection();
                    self.list.scroll_item_in_view = select.item_index();
                },
                _ => ()
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
                item.animator.play_anim(cx, SearchResultDraw::get_over_anim(cx, item_index, true));
            },
            ListLogicEvent::Deselect => {
                item.animator.play_anim(cx, SearchResultDraw::get_default_anim(cx, item_index, false));
            },
            ListLogicEvent::Cleanup => {
                item.animator.play_anim(cx, SearchResultDraw::get_default_anim_cut(cx, item_index, item.is_selected));
            },
            ListLogicEvent::Over => {
                item.animator.play_anim(cx, SearchResultDraw::get_over_anim(cx, item_index, item.is_selected));
            },
            ListLogicEvent::Out => {
                item.animator.play_anim(cx, SearchResultDraw::get_default_anim(cx, item_index, item.is_selected));
            }
        });
        
        match le {
            ListEvent::SelectSingle(_select_index) => {
                self.view.redraw_view_area(cx);
            },
            ListEvent::SelectMultiple => {},
            ListEvent::None => {
            }
        }
        SearchResultEvent::None
    }
    
    pub fn draw_tab_contents(&mut self, cx: &mut Cx, _search_index: &SearchIndex) {
        cx.begin_style(Self::style_text_input());
        self.search_input.draw_text_input(cx);
        cx.end_style();
    }
    
    pub fn draw_search_results(&mut self, cx: &mut Cx, storage: &AppStorage) {
        
        self.list.set_list_len(self.results.len()); //bm.log_items.len());
        
        self.result_draw.text.text_style = SearchResultDraw::text_style_item().get(cx);
        
        let row_height = SearchResultDraw::layout_item().get(cx).walk.height.fixed();
        
        if self.list.begin_list(cx, &mut self.view, false, row_height).is_err() {return}
        
        let mut counter = 0;
        for i in self.list.start_item..self.list.end_item {
            // lets get the path
            let result = &self.results[i];
            let tb = &storage.text_buffers[result.text_buffer_id.0 as usize];
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
