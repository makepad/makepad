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

#[derive(Clone, DrawQuad)]
#[repr(C)]
pub struct DrawFoldCaption  {
    #[default_shader(self::shader_bg)]
    base: DrawQuad,
    hover: f32,
    down: f32,
    open: f32
}


#[derive(Clone)]
pub struct FoldCaption {
    pub button: ButtonLogic,
    pub bg: DrawFoldCaption,
    pub text: DrawText,
    pub animator: Animator,
    pub open_state: FoldOpenState,
}

impl FoldCaption {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            button: ButtonLogic::default(),
            
            bg: DrawFoldCaption::new(cx, default_shader!())
                .with_draw_depth(0.5),

            text: DrawText::new(cx, default_shader!())
                .with_draw_depth(1.0),
                
            open_state: FoldOpenState::Open,
            animator: Animator::default(),
        }
    }
    
    pub fn style(cx: &mut Cx) {
        
        DrawFoldCaption::register_draw_input(cx);
        
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
                    Float {keys:{1.0: 0.0}, bind_to: self::DrawFoldCaption::hover}
                    Float {keys:{1.0: 0.0}, bind_to: self::DrawFoldCaption::down}
                    Vec4 {keys:{1.0: #9}, bind_to: makepad_render::drawtext::DrawText::color}
                ]
            }
            
            self::anim_over: Anim {
                play: Cut {duration: 0.1},
                tracks:[
                    Float {keys:{0.0: 1.0, 1.0: 1.0}, bind_to: self::DrawFoldCaption::hover},
                    Float {keys:{1.0: 0.0}, bind_to: self::DrawFoldCaption::down},
                    Vec4 {keys:{0.0: #f}, bind_to: makepad_render::drawtext::DrawText::color}
                ]
            }
            
            self::anim_down: Anim {
                play: Cut {duration: 0.2},
                tracks:[
                    Float {keys:{0.0: 1.0, 1.0: 1.0}, bind_to: self::DrawFoldCaption::down},
                    Float {keys:{1.0: 1.0}, bind_to: self::DrawFoldCaption::hover},
                    Vec4 {keys:{0.0: #c}, bind_to: makepad_render::drawtext::DrawText::color},
                ]
            }
            
            self::shader_bg: Shader {
                
                use makepad_render::drawquad::shader::*;
                
                draw_input: self::DrawFoldCaption;
                
                const shadow: float = 3.0;
                const border_radius: float = 2.5;
                
                fn pixel() -> vec4 {
                    let sz = 3.;
                    let c = vec2(5.0,0.5*rect_size.y);
                    let df = Df::viewport(pos * rect_size);
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
        let open_state = &mut self.open_state;

        if let Some(ae) = event.is_animate(cx, animator) {
            self.bg.animate(cx, animator, ae.time);
            self.text.animate(cx, animator, ae.time);
        }

        self.button.handle_button_logic(cx, event, self.bg.area(), | cx, logic_event, area | match logic_event {
            ButtonLogicEvent::Down => {
                // lets toggle our anim state
                open_state.toggle();
                cx.redraw_child_area(area);
                animator.play_anim(cx, live_anim!(cx, self::anim_down));
            }
            ButtonLogicEvent::Default => animator.play_anim(cx, live_anim!(cx, self::anim_default)),
            ButtonLogicEvent::Over => animator.play_anim(cx, live_anim!(cx, self::anim_over))
        })
    }
    
    pub fn begin_fold_caption(&mut self, cx: &mut Cx)->f32{

        if self.animator.need_init(cx){
            self.animator.init(cx, live_anim!(cx, self::anim_default));
            self.bg.last_animate(&self.animator);
            self.text.last_animate(&self.animator);
        }
        
        let open_value = self.open_state.get_value();
        self.bg.open = open_value;
        self.bg.begin_quad(cx, live_layout!(cx, self::layout_bg));
        
        if self.open_state.do_time_step(0.6){
            cx.redraw_child_area(self.bg.area());
        }
        
        open_value
    }
        
    pub fn end_fold_caption(&mut self, cx:&mut Cx, label:&str){    
        cx.change_turtle_align_x_cab(0.0);
        cx.reset_turtle_pos();
        
        self.text.text_style = live_text_style!(cx, self::text_style_label);
        let wleft = cx.get_width_left();
        self.text.wrapping = Wrapping::Ellipsis(wleft - 10.0);
        self.text.draw_text_walk(cx, label);

        self.bg.end_quad(cx);
    }
}

