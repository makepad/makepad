#![allow(unused)]
use crate::{
    makepad_platform::*,
    button_logic::*,
    frame_component::*,
};

live_register!{
    use makepad_platform::shader::std::*;
    
    FoldButton: {{FoldButton}} {
        bg_quad: {
            instance opened: 0.0
            instance hover: 0.0
            
            uniform fade: 1.0
            
            fn pixel(self) -> vec4 {
                
                let sz = 3.;
                let c = vec2(5.0, 0.5 * self.rect_size.y);
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                sdf.clear(vec4(0.));
                // we have 3 points, and need to rotate around its center
                sdf.rotate(self.opened * 0.5 * PI + 0.5 * PI, c.x, c.y);
                sdf.move_to(c.x - sz, c.y + sz);
                sdf.line_to(c.x, c.y - sz);
                sdf.line_to(c.x + sz, c.y + sz);
                sdf.close_path();
                sdf.fill(mix(#a, #f, self.hover));
                return sdf.result * self.fade;
            }
        }
        
        abs_size: vec2(32, 12)
        abs_offset: vec2(4., 0.)
        
        walk: {
            width: 12,
            height: 12,
        }
        
        state:{
            
            default = {
                default:true,
                duration: 0.1
                apply: {bg_quad: {hover: 0.0}}
            }
            
            hover =  {
                duration: 0.0
                apply: {bg_quad: {hover: 1.0}}
            }
            
            closed = {
                track: zoom
                from: {all: Play::Exp {speed1: 0.96, speed2: 0.97}}
                redraw: true
                apply: {
                    opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]
                    bg_quad: {opened: (opened)}
                }
            }
            
            opened = {
                default:true,
                track: zoom
                from: {all: Play::Exp {speed1: 0.98, speed2: 0.95}}
                redraw: true
                apply: {
                    opened: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]
                    bg_quad: {opened: (opened)}
                }
            }
        }
        /*
        closed_state: {
            track: open
            duration: 0.2
            ease: Ease::OutExp
            apply: {
                opened: 0.0,
                bg_quad: {opened: (opened)}
            }
        }
        
        opened_state: {
            track: open,
            duration: 0.2
            ease: Ease::OutExp
            apply: {opened: 1.0,}
        }*/
    }
}

#[derive(Live, LiveHook)]
#[live_register(register_as_frame_component!(FoldButton))]
pub struct FoldButton {
    #[rust] button_logic: ButtonLogic,
    
    state: State,
    
    opened: f32,
    
    bg_quad: DrawQuad,
    abs_size: Vec2,
    abs_offset: Vec2,
    walk: Walk,
}

#[derive(Clone, FrameComponentAction)]
pub enum FoldButtonAction {
    None,
    Opening,
    Closing,
    Animating(f32)
}

impl FrameComponent for FoldButton {
    fn handle_component_event(&mut self, cx: &mut Cx, event: &mut Event, self_id: LiveId) -> FrameComponentActionRef {
        let mut a = Vec::new();
        self.handle_event_with_fn(cx, event, &mut | _, v | a.push(FrameActionItem::new(self_id, v.into())));
        FrameActions::Actions(a).into()
    }
    
    fn get_walk(&self) -> Walk {
        self.walk
    }
    
    fn draw_component(&mut self, cx: &mut Cx2d, walk: Walk) -> Result<(), LiveId> {
        self.draw_walk(cx, walk);
        Ok(())
    }
}

impl FoldButton {
    
    pub fn handle_event_with_fn(
        &mut self,
        cx: &mut Cx,
        event: &mut Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FoldButtonAction),
    ) {
        if self.state_handle_event(cx, event).is_animating() {
            if self.state.is_track_of_animating(cx, id!(closed)) {
                dispatch_action(cx, FoldButtonAction::Animating(self.opened))
            }
        };
        let res = self.button_logic.handle_event(cx, event, self.bg_quad.draw_vars.area);
        
        match res.state {
            ButtonState::Pressed => {
                if self.state.is_in_state(cx, id!(opened)) {
                    self.animate_state(cx, id!(closed));
                    dispatch_action(cx, FoldButtonAction::Closing)
                }
                else {
                    self.animate_state(cx, id!(opened));
                    dispatch_action(cx, FoldButtonAction::Opening)
                }
            }
            ButtonState::Default => self.animate_state(cx, id!(default)),
            ButtonState::Hover => self.animate_state(cx, id!(hover)),
            _ => ()
        };
    }
    
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.toggle_state(cx, is_open, animate, id!(opened), id!(closed))
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.bg_quad.draw_walk(cx, walk);
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: Vec2, fade: f32) {
        self.bg_quad.apply_over(cx, live!{fade: (fade)});
        self.bg_quad.draw_abs(cx, Rect {
            pos: pos + self.abs_offset,
            size: self.abs_size
        });
    }
}


