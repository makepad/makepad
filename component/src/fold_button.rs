#![allow(unused)]
use crate::{
    makepad_derive_frame::*,
    makepad_platform::*,
    button_logic::*,
    frame_traits::*,
};

live_register!{
    import makepad_platform::shader::std::*;
    
    FoldButton: {{FoldButton}} {
        bg: {
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
            
            hover ={
                default:off
                off = {
                    from: {all: Play::Forward{duration: 0.1}}
                    apply: {bg: {hover: 0.0}}
                }
                
                on =  {
                    from: {all: Play::Snap}
                    apply: {bg: {hover: 1.0}}
                }
            }
            
            open = {
                default: yes
                no ={
                    from: {all: Play::Forward{duration:0.2}}
                    ease: Ease::ExpDecay{d1:0.96,d2:0.97}
                    redraw: true
                    apply: {
                        opened: [{time: 0.0, value: 1.0}, {time: 1.0, value: 0.0}]
                        bg: {opened: (opened)}
                    }
                }
                yes = {
                    from: {all: Play::Forward{duration:0.2}}
                    ease: Ease::ExpDecay{d1:0.98,d2:0.95}
                    redraw: true
                    apply: {
                        opened: [{time: 0.0, value: 0.0}, {time: 1.0, value: 1.0}]
                        bg: {opened: (opened)}
                    }
                }
            }
        }
    }
}

#[derive(Live, LiveHook, FrameComponent)]
#[live_register(frame_component!(FoldButton))]
pub struct FoldButton {
    state: State,
    
    opened: f32,
    
    bg: DrawQuad,
    abs_size: Vec2,
    abs_offset: Vec2,
    walk: Walk,
}

#[derive(Clone, FrameAction)]
pub enum FoldButtonAction {
    None,
    Opening,
    Closing,
    Animating(f32)
}


impl FoldButton {
    
    pub fn handle_event(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, FoldButtonAction),
    ) {
        if self.state_handle_event(cx, event).is_animating() {
            if self.state.is_track_animating(cx, id!(open)) {
                dispatch_action(cx, FoldButtonAction::Animating(self.opened))
            }
        };
        
        let state = button_logic_handle_event(cx, event, self.bg.area(), &mut |_,_|{});
        if let Some(state) = state {
            match state {
                ButtonState::Pressed => {
                    if self.state.is_in_state(cx, ids!(open.yes)) {
                        self.animate_state(cx, ids!(open.no));
                        dispatch_action(cx, FoldButtonAction::Closing)
                    }
                    else {
                        self.animate_state(cx, ids!(open.yes));
                        dispatch_action(cx, FoldButtonAction::Opening)
                    }
                }
                ButtonState::Default => self.animate_state(cx, ids!(hover.off)),
                ButtonState::Hover => self.animate_state(cx, ids!(hover.on)),
            }
        };
    }
    
    pub fn set_is_open(&mut self, cx: &mut Cx, is_open: bool, animate: Animate) {
        self.toggle_state(cx, is_open, animate, ids!(open.yes), ids!(open.no))
    }
    
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.bg.draw_walk(cx, walk);
    }
    
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: Vec2, fade: f32) {
        self.bg.apply_over(cx, live!{fade: (fade)});
        self.bg.draw_abs(cx, Rect {
            pos: pos + self.abs_offset,
            size: self.abs_size
        });
    }
}


