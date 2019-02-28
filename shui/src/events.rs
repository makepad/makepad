use crate::cxdrawing_shared::*;
use crate::area::*;
use crate::math::*;

#[derive(Clone, Debug)]
pub enum MouseButton{
    None,
    Left,
    Right,
    Middle,
    Other(u8)
}

impl Default for MouseButton{
    fn default()->MouseButton{
        MouseButton::None
    }
}

#[derive(Clone, Default,Debug)]
pub struct FingerDownEvent{
    pub x:f32,
    pub y:f32,
    pub digit:u32,
    pub button:MouseButton,
    pub is_touch:bool
}

#[derive(Clone, Default,Debug)]
pub struct FingerMoveEvent{
    pub x:f32,
    pub y:f32,
    pub digit:u32,
    pub button:MouseButton,
    pub is_touch:bool,
}

#[derive(Clone, Default,Debug)]
pub struct FingerUpEvent{
    pub x:f32,
    pub y:f32,
    pub digit:u32,
    pub button:MouseButton,
    pub is_touch:bool
}

#[derive(Clone, Default,Debug)]
pub struct FingerHoverEvent{
    pub x:f32,
    pub y:f32
}

#[derive(Clone, Default,Debug)]
pub struct FingerScrollEvent{
    pub x:f32,
    pub y:f32,
    pub dx:f32,
    pub dy:f32
}

#[derive(Clone, Default, Debug)]
pub struct FingerCapturedEvent{
    pub area:Area,
    pub x:f32,
    pub y:f32,
    pub dx:f32,
    pub dy:f32
}

#[derive(Clone, Default, Debug)]
pub struct ResizedEvent{
    pub old_size:Vec2,
    pub old_dpi_factor:f32,
    pub new_size:Vec2,
    pub new_dpi_factor:f32
}

#[derive(Clone, Default, Debug)]
pub struct AnimateEvent{
    pub time:f64
}


#[derive(Clone, Default, Debug)]
pub struct RedrawEvent{
    pub area:Area
}

#[derive(Clone,Debug)]
pub enum Event{
    None,
    Init,
    Redraw,
    AnimationEnded(AnimateEvent),
    Animate(AnimateEvent),
    CloseRequested,
    Resized(ResizedEvent),
    FingerCaptured(FingerCapturedEvent),
    FingerDown(FingerDownEvent),
    FingerMove(FingerMoveEvent),
    FingerHover(FingerHoverEvent),
    FingerUp(FingerUpEvent),
    FingerScroll(FingerScrollEvent)
}

impl Default for Event{
    fn default()->Event{
        Event::None
    }
}

impl Event{
    pub fn hits(&self, area:&Area, cd:&CxDrawing)->&Event{
        match self{
            Event::Animate(_)=>{
                for anim in &cd.animations{
                    if anim.area == *area{
                        return self
                    }
                }
            },
            Event::AnimationEnded(_)=>{
                for anim in &cd.ended_animations{
                    if anim.area == *area{
                        return self
                    }
                }
            },
            Event::FingerCaptured(fc)=>{
                if fc.area == *area{
                    return self;
                }
            },
            Event::FingerMove(fe)=>{
                if area.contains(fe.x, fe.y, &cd){
                    return self;
                }
            },
            Event::FingerDown(fe)=>{
                if area.contains(fe.x, fe.y, &cd){
                    return self;
                }
            },
            Event::FingerUp(fe)=>{
                if area.contains(fe.x, fe.y, &cd){
                    return self;
                }
            },
            _=>()
        };
        return &Event::None;
    }
}