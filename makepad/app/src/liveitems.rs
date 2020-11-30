use makepad_render::*;
use makepad_widget::*;
use crate::mprstokenizer::*;
use crate::makepadstorage::*;
use crate::colorpicker::*;
use crate::floatslider::*;
use std::fmt;
use std::collections::HashMap;


#[derive(Clone, Default)]
pub struct LiveItemsList {
    pub live_on_self: bool,
    pub visible_editors: bool,
    pub changed: Signal,
    pub items: Vec<LiveItemId>,
    pub live_bodies: HashMap<LiveBodyId, usize>
}

impl LiveItemsList {
    pub fn new(cx: &mut Cx, live_on_self:bool) -> Self {
        LiveItemsList {
            live_on_self,
            visible_editors: false,
            changed: cx.new_signal(),
            live_bodies: HashMap::new(),
            items: Vec::new(),
        }
    }
} 

#[derive(Clone)]
pub struct LiveItemsView {
    pub view_bg: DrawColor,
    pub scroll_view: ScrollView,
    pub undo_id: u64,
    pub color_swatch: DrawColor,
    pub value_text: DrawText,
    pub fold_captions: Elements<usize, FoldCaption, FoldCaption>,
    pub color_pickers: Elements<usize, ColorPicker, ColorPicker>,
    pub float_sliders: Elements<usize, FloatSlider, FloatSlider>
}

impl LiveItemsView {
    pub fn items_changed() -> StatusId {uid!()}
    
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            scroll_view: ScrollView::new_standard_hv(cx),
            undo_id: 0,
            value_text: DrawText::new(cx, default_shader!())
                .with_draw_depth(1.0),
            color_swatch: DrawColor::new(cx, live_shader!(cx, self::shader_color_swatch))
                .with_draw_depth(1.0),
            fold_captions: Elements::new(FoldCaption::new(cx)),
            color_pickers: Elements::new(ColorPicker::new(cx)),
            float_sliders: Elements::new(FloatSlider::new(cx)),
            view_bg: DrawColor::new(cx, default_shader!()),
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            self::color_bg: #28;
            
            self::text_style_value: TextStyle {
                ..makepad_widget::widgetstyle::text_style_normal
            }
            
            self::shader_color_swatch: Shader {
                use makepad_render::drawcolor::shader::*;
                
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * rect_size);
                    df.box(0., 0., rect_size.x, rect_size.y, 1.);
                    df.fill(color);
                    return df.result;
                }
            }
            
            self::walk_color_swatch: Walk {
                margin: all(0.),
                width: Fix(12.),
                height: Fix(12.)
            }
            
            
        "#)
    }
    
    pub fn handle_live_items(&mut self, cx: &mut Cx, event: &mut Event, mtb: &mut MakepadTextBuffer) {
        
        self.scroll_view.handle_scroll_view(cx, event);
        
        match event {
            Event::Signal(se) => {
                // process network messages for hub_ui
                if let Some(_) = se.signals.get(&mtb.live_items_list.changed) {
                    self.scroll_view.redraw_view(cx);
                }
                if let Some(ids) = se.signals.get(&mtb.text_buffer.signal){
                    if ids.contains(&TextBuffer::token_chunks_changed()){
                        mtb.parse_live_bodies(cx);
                    }
                }
            },
            _ => ()
        }
        let mut any_caption_down = None;
        let mut do_open = false;
        for (index, live_item_id) in mtb.live_items_list.items.iter().enumerate() {
            let live_item_id = live_item_id.clone();
            // get tokens
            if let Some(tok) = cx.live_styles.tokens.get(&live_item_id) {
                let live_tokens_type = tok.live_tokens_type;
                //let start = tok.tokens[0].span.start;
                //let end = tok.tokens[0].span.end;
                
                if let Some(fold_caption) = self.fold_captions.get_mut(index) {
                    if fold_caption.handle_fold_caption(cx, event) == ButtonEvent::Down {
                        any_caption_down = Some(index);
                        do_open = fold_caption.open_state.is_open();
                    };
                }
                match live_tokens_type {
                    LiveTokensType::Float => {
                        if let Some(f) = self.float_sliders.get_mut(index) {
                            match f.handle_float_slider(cx, event) {
                                FloatSliderEvent::Change {scaled_value} => {
                                    let float = Float {value: scaled_value, ..Float::default()};
                                    MakepadStorage::handle_changed_float(
                                        cx,
                                        live_item_id,
                                        float.clone(),
                                        &mtb.live_items_list.live_bodies,
                                        &mut mtb.text_buffer,
                                    );
                                    MakepadStorage::send_websocket_message(cx, MakepadChannelMessage::ChangeFloat {
                                        live_item_id: live_item_id,
                                        float: float.clone(),
                                    });
                                },
                                FloatSliderEvent::DoneChanging => {
                                    self.undo_id += 1;
                                },
                                _ => ()
                            }
                        }
                    },
                    LiveTokensType::Vec4 => {
                        if let Some(f) = self.color_pickers.get_mut(index) {
                            match f.handle_color_picker(cx, event) {
                                ColorPickerEvent::Change {rgba} => {
                                    MakepadStorage::handle_changed_color(
                                        cx,
                                        live_item_id,
                                        rgba,
                                        &mtb.live_items_list.live_bodies,
                                        &mut mtb.text_buffer,
                                    );
                                    MakepadStorage::send_websocket_message(cx, MakepadChannelMessage::ChangeColor {
                                        live_item_id: live_item_id,
                                        rgba: rgba,
                                    });
                                },
                                ColorPickerEvent::DoneChanging => {
                                    self.undo_id += 1;
                                },
                                _ => ()
                            }
                        }
                    },
                    _ => ()
                }
                
            }
        }
        
        // handle the grouped caption folding
        if let Some(down_index) = any_caption_down {
            if let Event::FingerDown(fe) = event {
                if fe.modifiers.control || fe.modifiers.logo {
                    for index in 0..mtb.live_items_list.items.len() {
                        if let Some(fold_caption) = self.fold_captions.get_mut(index) {
                            if fe.modifiers.control {
                                if index != down_index {
                                    fold_caption.open_state.do_close();
                                }
                            }
                            else {
                                if do_open {
                                    fold_caption.open_state.do_open();
                                }
                                else {
                                    fold_caption.open_state.do_close();
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    pub fn draw_live_items(&mut self, cx: &mut Cx, mtb: &mut MakepadTextBuffer, _text_editor: &mut TextEditor) {
        
        if self.scroll_view.begin_view(cx, Layout {
            direction: Direction::Down,
            ..Layout::default()
        }).is_ok() {
            self.view_bg.color = live_vec4!(cx, self::color_bg);
            self.view_bg.draw_quad_abs(cx, cx.get_turtle_rect());//self.view_bg.begin_quad_fill(cx);
            self.view_bg.area().set_do_scroll(cx, false, false);
            
            //let layout_caption_bg = live_layout!(cx, self::layout_caption_bg);
            self.value_text.text_style = live_text_style!(cx, self::text_style_value);
            
            for (index, live_id) in mtb.live_items_list.items.iter().enumerate() {
                // get tokens
                if let Some(tok) = cx.live_styles.tokens.get(live_id) {
                    let ip = tok.ident_path.clone();
                    let live_tokens_type = tok.live_tokens_type;
                    //let wleft = cx.get_width_left() - 10.;
                    //self.caption_text.wrapping = Wrapping::Ellipsis(wleft);
                    
                    //let first_tok = tok.tokens[0];
                    match live_tokens_type {
                        LiveTokensType::Float => {
                            if let Some(f) = cx.live_styles.floats.get(live_id).cloned() {
                                let fold_caption = self.fold_captions.get_draw(cx, index, | _, t | t.clone());
                                let height_scale = fold_caption.begin_fold_caption(cx);
                                // draw our float value
                                //cx.change_turtle_align_x_ab(1.0);
                                //cx.move_turtle(5., 0.);
                                self.value_text.draw_text_walk(cx, &format!("{}", PrettyPrintedFloat3Decimals(f.value)));
                                // draw our value right aligned
                                ip.segs[1].with( | s | {
                                    fold_caption.end_fold_caption(cx, s);
                                });
                                self.float_sliders.get_draw(cx, index, | _, t | t.clone())
                                    .draw_float_slider(cx, f.value, f.min, f.max, f.step, height_scale);
                            }
                        },
                        LiveTokensType::Vec4 => {
                            if let Some(c) = cx.live_styles.vec4s.get(live_id).cloned() {
                                let fold_caption = self.fold_captions.get_draw(cx, index, | _, t | t.clone());
                                let height_scale = fold_caption.begin_fold_caption(cx);
                                // we need to draw a little color swatch.
                                self.color_swatch.color = c;
                                self.color_swatch.draw_quad_walk(cx, live_walk!(cx, self::walk_color_swatch));
                                
                                ip.segs[1].with( | s | {
                                    fold_caption.end_fold_caption(cx, s);
                                });
                                self.color_pickers.get_draw(cx, index, | _, t | t.clone()).draw_color_picker(cx, c, height_scale);
                            }
                        },
                        _ => ()
                    }
                }
            }
            
            //self.view_bg.end_quad_fill(cx, bg_inst);
            
            self.scroll_view.end_view(cx);
        }
    }
}

pub struct PrettyPrintedFloat3Decimals(pub f32);

impl fmt::Display for PrettyPrintedFloat3Decimals {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.0.abs().fract() < 0.00000001 {
            write!(f, "{}.000", self.0)
        } else {
            write!(f, "{:.3}", self.0)
        }
    }
}


impl MakepadTextBuffer {
    pub fn update_live_items(&mut self, cx: &mut Cx) {
        if !self.live_items_list.live_on_self{
            return 
        }
        self.live_items_list.items = cx.live_styles.get_live_items_for_file(&MakepadStorage::file_path_to_live_path(&self.full_path));
        self.live_items_list.visible_editors = false;
        for live_item_id in &self.live_items_list.items{
            if let Some(tok) = cx.live_styles.tokens.get(live_item_id){
                match tok.live_tokens_type{
                    LiveTokensType::Vec4 | LiveTokensType::Float=>{
                        self.live_items_list.visible_editors = true;
                    },
                    _=>()
                }
            }
        }
        
        cx.send_signal(self.live_items_list.changed, LiveItemsView::items_changed())
    }
    
    pub fn parse_live_bodies(&mut self, cx: &mut Cx) {
        if !self.live_items_list.live_on_self{
            return 
        }
        let mut tp = TokenParser::new(&self.text_buffer.flat_text, &self.text_buffer.token_chunks);
        // lets reset the data
        while tp.advance() {
            match tp.cur_type() {
                TokenType::Macro => {
                    // we need to find our live!(cx, r#".."#)
                    if tp.eat("live_body")
                        && tp.eat("!")
                        && tp.eat("(")
                        && tp.eat_token(TokenType::Identifier)
                        && tp.eat(",")
                        && tp.eat("r")
                        && tp.eat("#") {
                        // ok so this is where live body X starts.
                        // we can use that to map a live_style token to our editor
                         
                        if tp.cur_type() == TokenType::ParenOpen {
                            //shader_end = tp.cur_pair_offset();
                            
                            if let Some(live_body) = tp.cur_pair_as_string() {
                                let lc = tp.cur_line_col();
                                
                                // lets list this live_body in our macro list.
                                if let Ok(live_body_id) = cx.live_styles.update_live_body(
                                    &MakepadStorage::file_path_to_live_path(&self.full_path),
                                    lc.0 + 1,
                                    lc.1 - 8,
                                    live_body
                                ) {
                                    self.live_items_list.live_bodies.insert(live_body_id, tp.cur_offset() + 1);
                                }
                                //else {
                                    //eprintln!("LiveBody not found");
                                //};
                            }
                        }
                    }
                    
                    /*
                    if !only_update_shaders && tp.eat("pick") && tp.eat("!") {
                        if tp.cur_type() == TokenType::ParenOpen {
                            let range = tp.cur_pair_range();
                            tp.advance();
                            let in_shader = tp.cur_offset() < shader_end;
                            
                            // TODO parse 1,2,3,4 number arg version of pick!
                            
                            // ok so now we need to parse the color, and turn it to HSV
                            let color = if tp.cur_type() == TokenType::Color { // its a #color
                                let color = Color::parse_hex_str(&tp.cur_as_string()[1..]);
                                if let Ok(color) = color {color}else {Color::white()}
                            }
                            else { // its a named color
                                let color = Color::parse_name(&tp.cur_as_string());
                                if let Ok(color) = color {color}else {Color::white()}
                            };
                            // ok lets turn color into HSV and store it in data
                            self.live_macros.macros.push(LiveMacro::Pick {
                                in_shader,
                                range: (range.0 + 1, range.1),
                                hsva: color.to_hsv()
                            })
                        }
                    }
                    else if !only_update_shaders && tp.eat("slide") && tp.eat("!") {
                        if tp.cur_type() == TokenType::ParenOpen {
                            let in_shader = tp.cur_offset() < shader_end;
                            // now i just want the first number
                            let paren_range = tp.cur_pair_range();
                            tp.advance();
                            let mut value = 1.0;
                            let mut min = 0.0;
                            let mut max = 1.0;
                            let mut step = 0.0;
                            let range;
                            if tp.cur_type() == TokenType::Number {
                                // it could be followed by a min, max and step
                                value = if let Ok(v) = tp.cur_as_string().parse() {v}else {1.0};
                                range = tp.cur_range();
                                tp.advance();
                                if tp.eat(",") && tp.cur_type() == TokenType::Number {
                                    min = if let Ok(v) = tp.cur_as_string().parse() {v}else {0.0};
                                    tp.advance();
                                    if tp.eat(",") && tp.cur_type() == TokenType::Number {
                                        max = if let Ok(v) = tp.cur_as_string().parse() {v}else {1.0};
                                        tp.advance();
                                        if tp.eat(",") && tp.cur_type() == TokenType::Number {
                                            step = if let Ok(v) = tp.cur_as_string().parse() {v}else {0.0};
                                            tp.advance();
                                        }
                                    }
                                }
                            }
                            else {
                                range = (paren_range.0 + 1, paren_range.1);
                            }
                            self.live_macros.macros.push(LiveMacro::Slide {
                                in_shader,
                                value,
                                min,
                                max,
                                range,
                                step
                            })
                        }
                    }
                    */
                },
                _ => ()
            }
        }
        cx.send_signal(self.live_items_list.changed, LiveItemsView::items_changed());
    }
}

