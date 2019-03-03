use crate::cx::*;

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

#[derive(Clone,Debug)]
pub enum HoverState{
    In,
    Over,
    Out
}

impl Default for HoverState{
    fn default()->HoverState{
        HoverState::Over
    }
}

#[derive(Clone,Debug)]
pub struct HitState{
    was_over_last_call:bool
}

impl HitState{
    pub fn new()->HitState{
        HitState{
            was_over_last_call: false
        }
    }
}

#[derive(Clone, Default,Debug)]
pub struct FingerHoverEvent{
    pub x:f32,
    pub y:f32,
    pub hover_state:HoverState
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
    AppInit,
    Construct,
    Destruct,
    Update,
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
    pub fn hits(&self, cx:&Cx, area:Area, hit_state:&mut HitState)->Event{
        match self{
            Event::Animate(_)=>{
                for anim in &cx.animations{
                    if anim.area == area{
                        return self.clone()
                    }
                }
            },
            Event::AnimationEnded(_)=>{
                for anim in &cx.ended_animations{
                    if anim.area == area{
                        return self.clone()
                    }
                }
            },
            Event::FingerCaptured(fc)=>{
                if fc.area == area{
                    return self.clone();
                }
            },
            
            Event::FingerHover(fe)=>{
                if hit_state.was_over_last_call{
                    if area.contains(fe.x, fe.y, &cx){
                        return self.clone();
                    }
                    else{
                        hit_state.was_over_last_call = false;
                        return Event::FingerHover(FingerHoverEvent{
                            hover_state:HoverState::Out,
                            ..fe.clone()
                        })
                    }
                }
                else{
                    if area.contains(fe.x, fe.y, &cx){
                        hit_state.was_over_last_call = true;
                        return Event::FingerHover(FingerHoverEvent{
                            hover_state:HoverState::In,
                            ..fe.clone()
                        })
                    }
                }

            },
            Event::FingerMove(fe)=>{
                if area.contains(fe.x, fe.y, &cx){
                    return self.clone();
                }
            },
            Event::FingerDown(fe)=>{
                if area.contains(fe.x, fe.y, &cx){
                    return self.clone();
                }
            },
            Event::FingerUp(fe)=>{
                if area.contains(fe.x, fe.y, &cx){
                    return self.clone();
                }
            },
            _=>()
        };
        return Event::None;
    }
}