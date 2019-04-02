use render::*;

#[derive(Clone, Element)]
pub struct ScrollBar{

    pub sb: Quad,
    pub bar_size:f32,
    pub min_handle_size:f32, //minimum size of the handle in pixels
    pub axis:Axis,
    pub animator:Animator,
    pub anim_over:Anim,
    pub anim_scrolling:Anim,

    pub _visible:bool,
    pub _hit_state:HitState,
    pub _sb_area:Area,
    pub _bar_side_margin:f32,
    pub _view_area:Area,
    pub _view_total:f32, // the total view area
    pub _view_visible:f32, // the visible view area
    pub _scroll_size:f32, // the size of the scrollbar
    pub _scroll_pos:f32, // scrolling position non normalised
    pub _drag_point:Option<f32>, // the point in pixels where we are dragging
}

impl Style for ScrollBar{
    fn style(cx:&mut Cx)->Self{
        let sh = Self::def_shader(cx);
        Self{
            bar_size:12.0,
            min_handle_size:140.0,

            axis:Axis::Horizontal,
            animator:Animator::new(Anim::new(AnimMode::Cut{duration:0.5}, vec![
                AnimTrack::to_vec4("sb.color",color("#5"))
            ])),
            anim_over:Anim::new(AnimMode::Cut{duration:0.05}, vec![
                AnimTrack::to_vec4("sb.color",color("#7"))
            ]),
            anim_scrolling:Anim::new(AnimMode::Cut{duration:0.05}, vec![
                AnimTrack::to_vec4("sb.color",color("#9"))
            ]),
            sb:Quad{
                shader_id:cx.add_shader(sh, "ScrollBar.sb"),
                ..Style::style(cx)
            },

            _visible:false,
            _view_area:Area::Empty,
            _view_total:0.0,
            _view_visible:0.0,
            _bar_side_margin:6.0,
            _scroll_size:0.0,
            _scroll_pos:0.0,
            _drag_point:None,
            _hit_state:HitState{
                no_scrolling:true,
                ..Default::default()
            },
            _sb_area:Area::Empty,
        }
    }
}


impl ScrollBar{
    pub fn def_shader(cx:&mut Cx)->Shader{
        let mut sh = Quad::def_quad_shader(cx);
        sh.add_ast(shader_ast!({

            let is_vertical:float<Instance>;

            let norm_handle:float<Instance>;
            let norm_scroll:float<Instance>;

            const border_radius:float = 1.5;

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
                    df_box(1., h*norm_scroll, w*0.5, h*norm_handle, border_radius);
                }
                else{
                    df_box(w*norm_scroll, 1., w*norm_handle, h*0.5, border_radius);
                }
                return df_fill_keep(color);
            }
        }));
        sh
    }

    // reads back normalized scroll position info
    fn get_normalized_scroll_pos(&self)->(f32,f32){
        // computed handle size normalized
        let vy = self._view_visible / self._view_total;
        if !self._visible{
            return (0.0,0.0);
        }
        let norm_handle = vy.max(self.min_handle_size/self._scroll_size);
        let norm_scroll = (1.-norm_handle) * ((self._scroll_pos / self._view_total) / (1.-vy));
        return (norm_scroll, norm_handle)
    }

    // sets the scroll pos from finger position
    fn set_scroll_pos_from_finger(&mut self, cx:&mut Cx, finger:f32)->ScrollBarEvent{
        let vy = self._view_visible / self._view_total;
        let norm_handle = vy.max(self.min_handle_size/self._scroll_size);
        
        let new_scroll_pos = (
            (self._view_total * (1.-vy) * (finger / self._scroll_size)) / (1.-norm_handle)
        ).max(0.).min(self._view_total - self._view_visible);

        let changed = self._scroll_pos != new_scroll_pos;
        self._scroll_pos = new_scroll_pos;
        if changed{
            self.update_shader_scroll_pos(cx);
            return self.make_scroll_event();
        }
        return ScrollBarEvent::None;
    }

    // writes the norm_scroll value into the shader
    fn update_shader_scroll_pos(&mut self, cx:&mut Cx){
        let (norm_scroll, _) = self.get_normalized_scroll_pos();
        self._sb_area.write_float(cx,"norm_scroll", norm_scroll);
    }

    // turns scroll_pos into an event on this.event
    fn make_scroll_event(&mut self)->ScrollBarEvent{
        match self.axis{
            Axis::Horizontal=>{
                ScrollBarEvent::ScrollHorizontal{
                        scroll_pos:self._scroll_pos,
                        view_total:self._view_total,
                        view_visible:self._view_visible
                }
            },
            Axis::Vertical=>{
                ScrollBarEvent::ScrollVertical{
                        scroll_pos:self._scroll_pos,
                        view_total:self._view_total,
                        view_visible:self._view_visible
                }
            }
        }
    }
   

    // public facing API

    pub fn get_scroll_pos(&self)->f32{
        return self._scroll_pos;
    }

    pub fn set_scroll_pos(&mut self, cx:&mut Cx, scroll_pos:f32){
        // clamp scroll_pos to
        let scroll_pos = scroll_pos.min(self._view_total - self._view_visible).max(0.); 
        if self._scroll_pos != scroll_pos{
            self._scroll_pos = scroll_pos;
            self.update_shader_scroll_pos(cx);
        }
    }
}


impl ScrollBarLike<ScrollBar> for ScrollBar{

    fn handle_scroll_bar(&mut self, cx:&mut Cx, event:&mut Event)->ScrollBarEvent{
        // lets check if our view-area gets a mouse-scroll.
        match event{
            Event::FingerScroll(fe)=>{
                let rect = self._view_area.get_rect(cx, false);
                if rect.contains(fe.abs.x, fe.abs.y){ // handle mousewheel
                    // we should scroll in either x or y
                    match self.axis{
                        Axis::Horizontal=>{
                            let scroll_pos= self.get_scroll_pos();
                            self.set_scroll_pos(cx, scroll_pos + fe.scroll.x);
                            return self.make_scroll_event();
                        },
                        Axis::Vertical=>{
                            let scroll_pos= self.get_scroll_pos();
                            self.set_scroll_pos(cx, scroll_pos + fe.scroll.y);
                            return self.make_scroll_event();
                        }
                    }
                }
            },
            _=>()
        };
        if self._visible{
            match event.hits(cx, self._sb_area, &mut self._hit_state){
                Event::Animate(ae)=>{
                    self.animator.calc_area(cx, "sb.color", ae.time, self._sb_area);
                },
                Event::FingerDown(fe)=>{
                    self.animator.play_anim(cx, self.anim_scrolling.clone());

                    match self.axis{
                        Axis::Horizontal=>{
                            //drag_start
                            let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
                            let bar_x = norm_scroll * self._scroll_size;
                            let bar_w = norm_handle * self._scroll_size;
                            if fe.rel.x < bar_x || fe.rel.x > bar_w + bar_x{ // clicked below
                                self._drag_point = Some(bar_w * 0.5);
                                return self.set_scroll_pos_from_finger(cx, fe.rel.x - self._drag_point.unwrap());
                            }
                            else{ // clicked on
                                self._drag_point = Some(fe.rel.x - bar_x); // store the drag delta
                            }
                        },
                        Axis::Vertical=>{
                            // computed handle size normalized
                            let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();
                            let bar_y = norm_scroll * self._scroll_size;
                            let bar_h = norm_handle * self._scroll_size;
                            if fe.rel.y < bar_y || fe.rel.y > bar_h + bar_y{ // clicked below or above
                                self._drag_point = Some(bar_h * 0.5);
                                return self.set_scroll_pos_from_finger(cx, fe.rel.y - self._drag_point.unwrap());
                            }
                            else{ // clicked on
                                self._drag_point = Some(fe.rel.y - bar_y); // store the drag delta
                            }
                        }
                    }        
                },
                Event::FingerHover(fe)=>{
                    if self._drag_point.is_none(){
                        match fe.hover_state{
                            HoverState::In=>{
                                self.animator.play_anim(cx, self.anim_over.clone());
                            },
                            HoverState::Out=>{
                                self.animator.play_anim(cx, self.animator.default.clone());
                            },
                            _=>()
                        }
                    }
                },
                Event::FingerUp(fe)=>{
                    self._drag_point = None;
                    if fe.is_over{
                        if !fe.is_touch{
                            self.animator.play_anim(cx, self.anim_over.clone());
                        }
                        else{
                            self.animator.play_anim(cx, self.animator.default.clone());
                        }
                    }
                    else{
                        self.animator.play_anim(cx, self.animator.default.clone());
                    }
                    return ScrollBarEvent::ScrollDone;
                },
                Event::FingerMove(fe)=>{
                     // helper called by event code to scroll from a finger 
                    if self._drag_point.is_none(){
                        // state should never occur.
                        println!("Invalid state in scrollbar, fingerMove whilst drag_point is none")
                    }
                    else{
                        match self.axis{
                            Axis::Horizontal=>{
                                return self.set_scroll_pos_from_finger(cx, fe.rel.x - self._drag_point.unwrap());
                            },
                            Axis::Vertical=>{
                                return self.set_scroll_pos_from_finger(cx, fe.rel.y - self._drag_point.unwrap());
                            }
                        }
                    }
                },
                _=>()
            };
        }

        ScrollBarEvent::None
    }

    fn draw_scroll_bar(&mut self, cx:&mut Cx, axis:Axis, view_area:Area, view_rect:Rect, view_total:Vec2)->f32{
        // pull the bg color from our animation system, uses 'default' value otherwise
        self.sb.color = self.animator.last_vec4("sb.color");
        self._sb_area = Area::Empty;
        self._view_area = view_area;
        self.axis = axis;

        match self.axis{
             Axis::Horizontal=>{
                self._visible = view_total.x > view_rect.w+0.1;
                self._scroll_size = if view_total.y > view_rect.h{
                    view_rect.w - self.bar_size
                }
                else{
                    view_rect.w
                } - self._bar_side_margin*2.;
                self._view_total = view_total.x;
                self._view_visible = view_rect.w;

                if self._visible{
                    self._sb_area = self.sb.draw_quad(
                        cx,  
                        self._bar_side_margin, 
                        view_rect.h - self.bar_size, 
                        self._scroll_size,
                        self.bar_size, 
                    );
                    self._sb_area.push_float(cx, "is_vertical", 0.0);
                }
             },
             Axis::Vertical=>{
                // compute if we need a horizontal one
                self._visible = view_total.y > view_rect.h+0.1;
                self._scroll_size = if view_total.x > view_rect.w {
                    view_rect.h - self.bar_size
                }
                else{
                    view_rect.h
                } - self._bar_side_margin*2.;
                self._view_total = view_total.y;
                self._view_visible = view_rect.h;
                if self._visible{
                    self._sb_area = self.sb.draw_quad(
                        cx,   
                        view_rect.w - self.bar_size, 
                        self._bar_side_margin, 
                        self.bar_size,
                        self._scroll_size
                    );
                    self._sb_area.push_float(cx, "is_vertical", 1.0);
                }
            }
        }
        // compute normalized sizes for the sahder
        let (norm_scroll, norm_handle) = self.get_normalized_scroll_pos();

        // see if we need to clamp
        let clamped_pos = self._scroll_pos.min(self._view_total - self._view_visible).max(0.); 
        if clamped_pos != self._scroll_pos{
            self._scroll_pos = clamped_pos;
        }

        // push the var added to the sb shader
        if self._visible{
            self._sb_area.push_float(cx, "norm_handle", norm_handle);
            self._sb_area.push_float(cx, "norm_scroll", norm_scroll);
            self.animator.set_area(cx, self._sb_area); // if our area changed, update animation
        }

        self._scroll_pos
    }
}
