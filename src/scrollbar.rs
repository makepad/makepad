use shui::*;

#[derive(Clone, Element)]
pub struct ScrollBar{
    pub hit_state:HitState,
    pub bar_size:f32,
    pub sb_area:Area,
    pub view_area:Area,
    pub sb_layout:Layout,
    pub sb: Quad,

    pub orientation:ScrollBarOrientation,
    pub anim:Animation<ScrollBarState>,
    pub event:ScrollBarEvent,
    // state
    pub visible:bool,

    pub view_total:f32, // the total view area
    pub view_visible:f32, // the visible view area
    pub scroll_size:f32, // the size of the scrollbar

    pub scroll_pos:f32, // scrolling position non normalised
    pub min_handle_size:f32, //minimum size of the handle in pixels
    pub drag_point:f32, // the point in pixels where we are dragging
}

#[derive(Clone, PartialEq)]
pub enum ScrollBarState{
    Default,
    Over,
    Scrolling
}

impl Style for ScrollBar{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_shader(cx);
        Self{
            bar_size:12.0,
            visible:false,
            
            // scrollbars
            view_area:Area::Empty,
            view_total:0.0,
            view_visible:0.0,
            scroll_size:0.0,
                      
            scroll_pos:0.0,
            min_handle_size:140.0,
            drag_point:0.0,

            orientation:ScrollBarOrientation::Horizontal,
            hit_state:HitState{
                no_scrolling:true,
                ..Default::default()
            },
            sb_area:Area::Empty,
            sb_layout:Layout{
                align:Align::center(),
                w:Computed,
                h:Computed,
                margin:Margin::i32(1),
                ..Layout::paddedf(16.0,14.0,16.0,14.0)
            },
            anim:Animation::new(
                ScrollBarState::Default,
                vec![
                    AnimState::new(
                        ScrollBarState::Default,
                        AnimMode::Cut{duration:0.5}, 
                        vec![
                            AnimTrack::to_vec4("sb.color",color("#5"))
                        ]
                    ),
                    AnimState::new(
                        ScrollBarState::Over,
                        AnimMode::Cut{duration:0.05}, 
                        vec![
                            AnimTrack::to_vec4("sb.color",color("#7"))
                        ]
                    ),
                    AnimState::new(
                        ScrollBarState::Scrolling,
                        AnimMode::Cut{duration:0.05}, 
                        vec![
                            AnimTrack::to_vec4("sb.color",color("#9"))
                        ]
                    ) 
                ]
            ),
            sb:Quad{
                shader_id:cx.add_shader(sh),
                ..Style::style(cx)
            },
            event:ScrollBarEvent::None
        }
    }
}


impl ScrollBar{
    pub fn def_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_shader(cx);
        sh.add_ast(shader_ast!({

            let is_vertical:float<Instance>;

            let norm_handle:float<Instance>;
            let norm_scroll:float<Instance>;

            const border_radius:float = 2.0;

            fn vertex()->vec4{
                let clipped:vec2 = clamp(
                    geom*vec2(w, h) + vec2(x, y),
                    draw_list_clip.xy,
                    draw_list_clip.zw
                );
                pos = (clipped - vec2(x,y)) / vec2(w, h);
                return vec4(clipped,0.,1.) * camera_projection;
            }

            fn pixel()->vec4{
                df_viewport(pos * vec2(w, h));
                if is_vertical > 0.5{
                    df_box(0., h*norm_scroll, w, h*norm_handle, border_radius);
                }
                else{
                    df_box(w*norm_scroll, 0., w*norm_handle, h, border_radius);
                }
                return df_fill_keep(color);
            }
        }));
        sh
    }

    fn get_normalized_sizes(&self)->(f32,f32){
        // computed handle size normalized
        let vy = self.view_visible / self.view_total;
        if !self.visible{
            return (0.0,0.0);
        }
        let norm_handle = vy.max(self.min_handle_size/self.scroll_size);
        let norm_scroll = (1.-norm_handle) * ((self.scroll_pos / self.view_total) / (1.-vy));
        return (norm_scroll, norm_handle)
    }

    fn set_scroll_pos_from_finger(&mut self, finger:f32)->bool{
        let vy = self.view_visible / self.view_total;
        let norm_handle = vy.max(self.min_handle_size/self.scroll_size);
        let new_scroll_pos = ((self.view_total * (1.-vy) * (finger / self.scroll_size)) / (1.-norm_handle)).max(0.).min(self.view_total - self.view_visible);
        let changed = self.scroll_pos != new_scroll_pos;
        self.scroll_pos = new_scroll_pos;
        changed
    }

    fn do_scroll_event(&mut self, cx:&mut Cx){
        match self.orientation{
            ScrollBarOrientation::Horizontal=>{
                let (norm_scroll, _) = self.get_normalized_sizes();
                self.sb_area.write_float(cx,"norm_scroll", norm_scroll);
                self.event = ScrollBarEvent::ScrollHorizontal{
                        scroll_pos:self.scroll_pos,
                        view_total:self.view_total,
                        view_visible:self.view_visible
                };
            },
            ScrollBarOrientation::Vertical=>{
                let (norm_scroll, _) = self.get_normalized_sizes();
                self.sb_area.write_float(cx,"norm_scroll", norm_scroll);
                self.event = ScrollBarEvent::ScrollVertical{
                        scroll_pos:self.scroll_pos,
                        view_total:self.view_total,
                        view_visible:self.view_visible
                };
            }
        }
    }

    fn do_scroll_from_finger(&mut self, cx:&mut Cx, x:f32, y:f32){
        match self.orientation{
            ScrollBarOrientation::Horizontal=>{
                let actual_pos = x - self.drag_point;
                if self.set_scroll_pos_from_finger(actual_pos){
                    self.do_scroll_event(cx);
                }
            },
            ScrollBarOrientation::Vertical=>{
                let actual_pos = y - self.drag_point;
                if self.set_scroll_pos_from_finger(actual_pos){
                    self.do_scroll_event(cx);
                }
            }
        }
    }

    pub fn get_scroll_pos(&self)->f32{
        return self.scroll_pos;
    }

    pub fn set_scroll_pos(&mut self, cx:&mut Cx, scroll_pos:f32){
        // clamp scroll_pos to
        let scroll_pos = scroll_pos.min(self.view_total - self.view_visible).max(0.); 
        if self.scroll_pos != scroll_pos{
            self.scroll_pos = scroll_pos;
            self.do_scroll_event(cx);
        }
    }
}


impl ScrollBarLike<ScrollBar> for ScrollBar{
    fn new(cx: &mut Cx,orientation:ScrollBarOrientation)->ScrollBar{
        return ScrollBar{
            orientation:orientation,
            ..Style::style(cx)
        }
    }

    fn handle(&mut self, cx:&mut Cx, event:&mut Event)->ScrollBarEvent{

        self.event = ScrollBarEvent::None;

        // lets check if our view-area gets a mouse-scroll.
        match event{
            Event::FingerScroll(fe)=>{
                let rect = self.view_area.get_rect(cx, false);
                if rect.contains(fe.abs_x, fe.abs_y){ // handle mousewheel
                    // we should scroll in either x or y
                    match self.orientation{
                        ScrollBarOrientation::Horizontal=>{
                            let scroll_pos= self.get_scroll_pos();
                            self.set_scroll_pos(cx, scroll_pos + fe.scroll_x)
                        },
                        ScrollBarOrientation::Vertical=>{
                            let scroll_pos= self.get_scroll_pos();
                            self.set_scroll_pos(cx, scroll_pos + fe.scroll_y)
                        }
                    }        
                }
            },
            _=>()
        };
        if self.visible{
            match event.hits(cx, self.sb_area, &mut self.hit_state){
                Event::Animate(ae)=>{
                    self.anim.calc_area(cx, "sb.color", ae.time, self.sb_area);
                    //self.anim.calc_area(cx, "bg.handle_color", ae.time, self.sb_area);
                },
                Event::FingerDown(fe)=>{
                    self.anim.change_state(cx, ScrollBarState::Scrolling);
                    // so where are we 'down'? 
                    //let click_pos; // the position in the view area we clicked

                    match self.orientation{
                        ScrollBarOrientation::Horizontal=>{
                            //drag_start
                            let (norm_scroll, norm_handle) = self.get_normalized_sizes();
                            let bar_x = norm_scroll * self.scroll_size;
                            let bar_w = norm_handle * self.scroll_size;
                            if fe.rel_x < bar_x || fe.rel_x > bar_w + bar_x{ // clicked below
                                self.drag_point = bar_w * 0.5;
                                self.do_scroll_from_finger(cx, fe.rel_x, fe.rel_y);
                            }
                            else{ // clicked on
                                self.drag_point = fe.rel_x - bar_x; // store the drag delta
                            }
                        },
                        ScrollBarOrientation::Vertical=>{
                            // computed handle size normalized
                            let (norm_scroll, norm_handle) = self.get_normalized_sizes();
                            let bar_y = norm_scroll * self.scroll_size;
                            let bar_h = norm_handle * self.scroll_size;
                            if fe.rel_y < bar_y || fe.rel_y > bar_h + bar_y{ // clicked below or above
                                self.drag_point = bar_h * 0.5;
                                self.do_scroll_from_finger(cx, fe.rel_x, fe.rel_y);
                            }
                            else{ // clicked on
                                self.drag_point = fe.rel_y - bar_y; // store the drag delta
                            }
                        }
                    }        
                },
                Event::FingerHover(fe)=>{
                    if self.anim.state() != ScrollBarState::Scrolling{
                        match fe.hover_state{
                            HoverState::In=>{
                                self.anim.change_state(cx, ScrollBarState::Over);
                            },
                            HoverState::Out=>{
                                self.anim.change_state(cx, ScrollBarState::Default);
                            },
                            _=>()
                        }
                    }
                },
                Event::FingerUp(fe)=>{
                    self.event = ScrollBarEvent::ScrollDone;
                    if fe.is_over{
                        if !fe.is_touch{
                            self.anim.change_state(cx, ScrollBarState::Over);
                        }
                        else{
                            self.anim.change_state(cx, ScrollBarState::Default);
                        }
                    }
                    else{
                        self.anim.change_state(cx, ScrollBarState::Default);
                    }
                },
                Event::FingerMove(fe)=>{
                    self.do_scroll_from_finger(cx, fe.rel_x, fe.rel_y);
                },
                _=>()
            };
        }
        // see if we need to clamp
        let clamped_pos = self.scroll_pos.min(self.view_total - self.view_visible).max(0.); 
        if clamped_pos != self.scroll_pos{
            self.scroll_pos = clamped_pos;
            self.do_scroll_event(cx);
        }

        self.event.clone()
    }

    fn draw_with_view_size(&mut self, cx:&mut Cx, view_area:Area, view_rect:Rect, view_total:Vec2){
        // pull the bg color from our animation system, uses 'default' value otherwise
        self.sb.color = self.anim.last_vec4("sb.color");
        self.view_area = view_area;

        match self.orientation{
             ScrollBarOrientation::Horizontal=>{
                self.visible = view_total.x > view_rect.w;
                self.scroll_size = if view_total.y > view_rect.h{
                    view_rect.w - self.bar_size
                }
                else{
                    view_rect.w
                };

                self.view_total = view_total.x;
                self.view_visible = view_rect.w;
                
                self.sb_area = self.sb.draw_abs(
                    cx, true,   
                    view_rect.x, 
                    view_rect.y + view_rect.h - self.bar_size, 
                    self.scroll_size,
                    self.bar_size, 
                );
                self.sb_area.push_float(cx, "is_vertical", 0.0);
             },
             ScrollBarOrientation::Vertical=>{
                // compute if we need a horizontal one
                self.visible = view_total.y > view_rect.h;
                self.scroll_size = if view_total.x > view_rect.w{
                    view_rect.h - self.bar_size
                }
                else{
                    view_rect.h
                };
                self.view_total = view_total.y;
                self.view_visible = view_rect.h;
                
                self.sb_area = self.sb.draw_abs(
                    cx, true,   
                    view_rect.x + view_rect.w - self.bar_size, 
                    view_rect.y, 
                    self.bar_size,
                    self.scroll_size
                );
                self.sb_area.push_float(cx, "is_vertical", 1.0);
            }
        }
        
        // compute normalized sizes for the sahder
        let (norm_scroll, norm_handle) = self.get_normalized_sizes();
        // push the var added to the sb shader
        self.sb_area.push_float(cx, "norm_handle", norm_handle);
        self.sb_area.push_float(cx, "norm_scroll", norm_scroll);

        self.anim.set_area(cx, self.sb_area); // if our area changed, update animation
    }
}
