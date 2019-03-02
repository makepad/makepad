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
pub enum HitState{
    In,
    Over,
    Out
}

impl Default for HitState{
    fn default()->HitState{
        HitState::Over
    }
}

#[derive(Clone, Default,Debug)]
pub struct FingerHoverEvent{
    pub x:f32,
    pub y:f32,
    pub state:HitState
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
    pub fn hits(&self, cx:&Cx, area:Area, hit_state:&mut bool)->Event{
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
                if *hit_state{
                    if area.contains(fe.x, fe.y, &cx){
                        return self.clone();
                    }
                    else{
                        *hit_state = false;
                        return Event::FingerHover(FingerHoverEvent{
                            state:HitState::Out,
                            ..fe.clone()
                        })
                    }
                }
                else{
                    if area.contains(fe.x, fe.y, &cx){
                        *hit_state = true;
                        return Event::FingerHover(FingerHoverEvent{
                            state:HitState::In,
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