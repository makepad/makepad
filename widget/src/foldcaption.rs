 use makepad_render::*;
use crate::buttonlogic::*;

#[derive(Clone)]
pub enum FoldOpenState{
    Open,
    Opening(f32),
    Closed,
    Closing(f32)
}

impl FoldOpenState{
    fn get_value(&self)->f32{
        match self{
            FoldOpenState::Opening(fac)=>1.0 - *fac,
            FoldOpenState::Closing(fac)=>*fac,
            FoldOpenState::Open=>1.0,
            FoldOpenState::Closed=>0.0,
        }        
    }
    pub fn is_open(&self)->bool{
        match self{
            FoldOpenState::Opening(_)=>true,
            FoldOpenState::Closing(_)=>false,
            FoldOpenState::Open=>true,
            FoldOpenState::Closed=>false,
        }        
    }
    pub fn toggle(&mut self){
        *self = match self{
            FoldOpenState::Opening(fac)=>FoldOpenState::Closing(1.0 - *fac),
            FoldOpenState::Closing(fac)=>FoldOpenState::Opening(1.0 - *fac),
            FoldOpenState::Open=>FoldOpenState::Closing(1.0),
            FoldOpenState::Closed=>FoldOpenState::Opening(1.0),
        };
    }
    pub fn do_open(&mut self){
        *self = match self{
            FoldOpenState::Opening(fac)=>FoldOpenState::Opening(*fac),
            FoldOpenState::Closing(fac)=>FoldOpenState::Opening(1.0 - *fac),
            FoldOpenState::Open=>FoldOpenState::Open,
            FoldOpenState::Closed=>FoldOpenState::Opening(1.0),
        };
    }
    pub fn do_close(&mut self){
        *self = match self{
            FoldOpenState::Opening(fac)=>FoldOpenState::Closing(1.0 - *fac),
            FoldOpenState::Closing(fac)=>FoldOpenState::Closing(*fac),
            FoldOpenState::Open=>FoldOpenState::Closing(1.0),
            FoldOpenState::Closed=>FoldOpenState::Closed,
        };
    }
    pub fn do_time_step(&mut self, mul:f32)->bool{
        let mut redraw = false;
        *self = match self{
            FoldOpenState::Opening(fac) => {
                redraw = true;
                if *fac < 0.001 {
                    FoldOpenState::Open
                }
                else {
                    FoldOpenState::Opening(*fac * mul)
                }
            },
            FoldOpenState::Closing(fac) => {
                redraw = true;
                if *fac < 0.001 {
                    FoldOpenState::Closed
                }
                else {
                    FoldOpenState::Closing(*fac * mul)
                }
            },
            FoldOpenState::Open => {
                FoldOpenState::Open
            },
            FoldOpenState::Closed => {
               FoldOpenState::Closed
            }
        };   
        redraw     
    }
}

#[derive(Clone)]
pub struct FoldCaption {
    pub button: ButtonLogic,
    pub bg: Quad,
    pub text: Text,
    pub animator: Animator,
    pub open_state: FoldOpenState,
    pub _bg_area: Area,
    pub _text_area: Area,
    pub _bg_inst: Option<InstanceArea>
}

impl FoldCaption {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            button: ButtonLogic::default(),
            bg: Quad{
                z: 0.5,
                ..Quad::new(cx)
             },
            text: Text{
                z: 1.0,
                shader: live_shader!(cx, makepad_render::text::shader),
                ..Text::new(cx)
            },
            open_state: FoldOpenState::Open,
            animator: Animator::default(),
            _bg_area: Area::Empty,
            _bg_inst: None,
            _text_area: Area::Empty,
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            self::layout_bg: Layout {
                align: {fx:1.0,fy:0.5},
                walk: Walk {
                    width: Fill,
                    height: Compute,
                    margin: all(1.0),
                },
                padding: {l: 14.0, t: 8.0, r: 14.0, b: 8.0},
            }
            
            self::text_style_label: TextStyle {
                ..crate::widgetstyle::text_style_normal
            }
            
            self::anim_default: Anim {
                play: Cut {duration: 0.1}
                tracks:[
                    Float {keys:{1.0: 0.0}, bind_to: self::shader_bg::hover}
                    Float {keys:{1.0: 0.0}, bind_to: self::shader_bg::down}
                    Color {keys:{1.0: #9}, bind_to: makepad_render::text::shader::color}
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.1},
                tracks:[
                    Float {keys:{0.0: 1.0, 1.0: 1.0}, bind_to: self::shader_bg::hover},
                    Float {keys:{1.0: 0.0}, bind_to: self::shader_bg::down},
                    Color {keys:{0.0: #f}, bind_to: makepad_render::text::shader::color}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks:[
                    Float {keys:{0.0: 1.0, 1.0: 1.0}, bind_to: self::shader_bg::down},
                    Float {keys:{1.0: 1.0}, bind_to: self::shader_bg::hover},
                    Color {keys:{0.0: #c}, bind_to: makepad_render::text::shader::color},
                ]
            }
            
            self::shader_bg: Shader {
                
                use makepad_render::quad::shader::*;
                
                instance hover: float;
                instance down: float;
                instance open: float;
                
                const shadow: float = 3.0;
                const border_radius: float = 2.5;
                
                fn pixel() -> vec4 {
                    let sz = 3.;
                    let c = vec2(5.0,0.5*h);
                    let df = Df::viewport(pos * vec2(w, h));
                    df.clear(#2);
                    // we have 3 points, and need to rotate around its center
                    df.rotate(open*0.5*PI+0.5*PI, c.x, c.y);
                    df.move_to(c.x - sz, c.y + sz);
                    df.line_to(c.x, c.y - sz);
                    df.line_to(c.x + sz, c.y + sz);
                    df.close_path();
                    df.fill(mix(#a,#f,hover));
                    
                    return df.result;
                }
            }
        "#);
    }
    
    pub fn handle_fold_caption(&mut self, cx: &mut Cx, event: &mut Event) -> ButtonEvent {
        //let mut ret_event = ButtonEvent::None;
        let animator = &mut self.animator;
        let text_area = self._text_area;
        let open_state = &mut self.open_state;
        let bg_area = self._bg_area;
        self.button.handle_button_logic(cx, event, self._bg_area, | cx, logic_event, area | match logic_event {
            ButtonLogicEvent::Animate(ae) => {
                animator.calc_area(cx, area, ae.time);
                animator.calc_area(cx, text_area, ae.time);
            },
            ButtonLogicEvent::AnimEnded(_) => animator.end(),
            ButtonLogicEvent::Down => {
                // lets toggle our anim state
                open_state.toggle();
                cx.redraw_child_area(bg_area);
                animator.play_anim(cx, live_anim!(cx, self::anim_down));
            }
            ButtonLogicEvent::Default => animator.play_anim(cx, live_anim!(cx, self::anim_default)),
            ButtonLogicEvent::Over => animator.play_anim(cx, live_anim!(cx, self::anim_over))
        })
    }
    
    pub fn begin_fold_caption(&mut self, cx: &mut Cx)->f32{
        self.bg.shader = live_shader!(cx, self::shader_bg);
        
        self.animator.init(cx, | cx | live_anim!(cx, self::anim_default));
        
        let bg_inst = self.bg.begin_quad(cx, live_layout!(cx, self::layout_bg));
        
        bg_inst.push_last_float(cx, &self.animator, live_item_id!(self::shader_bg::hover));
        bg_inst.push_last_float(cx, &self.animator, live_item_id!(self::shader_bg::down));
        let open_value = self.open_state.get_value();
        bg_inst.push_float(cx, self.open_state.get_value());
        
        self._bg_inst = Some(bg_inst);
        if self.open_state.do_time_step(0.6){
            cx.redraw_child_area(self._bg_area);
        }
        
        open_value
    }
        
    pub fn end_fold_caption(&mut self, cx:&mut Cx, label:&str){    
        cx.change_turtle_align_x_cab(0.0);
        cx.reset_turtle_pos();
        
        self.text.text_style = live_text_style!(cx, self::text_style_label);
        let wleft = cx.get_width_left();
        self.text.wrapping = Wrapping::Ellipsis(wleft - 10.0);
        self.text.color = self.animator.last_color(cx, live_item_id!(makepad_render::text::shader::color));
        self._text_area = self.text.draw_text(cx, label);

        self._bg_area = self.bg.end_quad(cx, self._bg_inst.take().unwrap());
        self.animator.set_area(cx, self._bg_area);
    }
}

