use shui::*;

#[derive(Clone, Element)]
pub struct ScrollBar{
    pub hit_state:HitState,
    pub bar_size:f32,
    pub sb_area:Area,
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
            view_total:0.0,
            view_visible:0.0,
            scroll_size:0.0,
                      
            scroll_pos:0.0,
            min_handle_size:140.0,
            drag_point:0.0,

            orientation:ScrollBarOrientation::Horizontal,
            hit_state:HitState::new(),
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
        let norm_handle = vy.max(self.min_handle_size/self.scroll_size);
        let norm_scroll = (1.-norm_handle) * ((self.scroll_pos / self.view_total) / (1.-vy));
        return (norm_scroll, norm_handle)
    }

    fn set_scroll_pos_from_mouse(&mut self, mouse:f32)->bool{
        let vy = self.view_visible / self.view_total;
        let norm_handle = vy.max(self.min_handle_size/self.scroll_size);
        let new_scroll_pos = ((self.view_total * (1.-vy) * (mouse / self.scroll_size)) / (1.-norm_handle)).max(0.).min(self.view_total - self.view_visible);
        let changed = self.scroll_pos != new_scroll_pos;
        self.scroll_pos = new_scroll_pos;
        changed
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
        if !self.visible{
            return self.event.clone();
        }
        // todo process 2 finger scroll on the area of these scrollbars.


        match event.hits(cx, self.sb_area, &mut self.hit_state, HitTouch::Single){
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
                        if fe.rx < bar_x{ // clicked above

                        }
                        else if fe.rx > bar_w + bar_x{ // clicked below
                            log!(cx,"CLICKED LEFT");
                        }
                        else{ // clicked on
                            self.drag_point = fe.rx - bar_x; // store the drag delta
                        }
                    },
                    ScrollBarOrientation::Vertical=>{
                        // computed handle size normalized
                        let (norm_scroll, norm_handle) = self.get_normalized_sizes();
                        let bar_y = norm_scroll * self.scroll_size;
                        let bar_h = norm_handle * self.scroll_size;
                        if fe.ry < bar_y{ // clicked above

                        }
                        else if fe.ry > bar_h + bar_y{ // clicked below
                            log!(cx,"CLICKED BELOW");
                        }
                        else{ // clicked on
                            self.drag_point = fe.ry - bar_y; // store the drag delta
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
                // ok so. we have a total area,
                // and a visible area
                // normally the scrollbar is 
                match self.orientation{
                    ScrollBarOrientation::Horizontal=>{
                        let actual_pos = fe.rx - self.drag_point;
                        if self.set_scroll_pos_from_mouse(actual_pos){
                            let (norm_scroll, _) = self.get_normalized_sizes();
                            self.sb_area.write_float(cx,"norm_scroll", norm_scroll);
                            self.event = ScrollBarEvent::ScrollHorizontal{
                                    scroll_pos:self.scroll_pos,
                                    view_total:self.view_total,
                                    view_visible:self.view_visible
                            };
                        }
                    },
                    ScrollBarOrientation::Vertical=>{
                        let actual_pos = fe.ry - self.drag_point;
                        if self.set_scroll_pos_from_mouse(actual_pos){
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
            },
            _=>()
        };
        self.event.clone()
    }

    fn draw_with_view_size(&mut self, cx:&mut Cx, view_rect:Rect, view_total:Vec2){
        // pull the bg color from our animation system, uses 'default' value otherwise
        self.sb.color = self.anim.last_vec4("sb.color");

        match self.orientation{
             ScrollBarOrientation::Horizontal=>{
                self.visible = view_total.x > view_rect.w;
                if !self.visible{
                    return
                }
                // compute if we need a vertical one, so make space bottom right
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
                if !self.visible{
                    return
                }
                // check if we need a horizontal slider, ifso make space bottom right
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
