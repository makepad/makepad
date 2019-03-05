use crate::cx::*;

#[derive(Clone, Default,Debug)]
pub struct FingerDownEvent{
    pub x:f32,
    pub y:f32,
    pub digit:usize,
    pub handled:bool,
    pub is_touch:bool
}

#[derive(Clone, Default,Debug)]
pub struct FingerMoveEvent{
    pub x:f32,
    pub y:f32,
    pub start_x:f32,
    pub start_y:f32,
    pub is_over:bool,
    pub digit:usize,
    pub is_touch:bool,
}

#[derive(Clone, Default,Debug)]
pub struct FingerUpEvent{
    pub x:f32,
    pub y:f32,
    pub start_x:f32,
    pub start_y:f32,
    pub digit:usize,
    pub is_over:bool,
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
    finger_down_start:Vec<Vec2>,
    was_over_last_call:bool
}

impl HitState{
    pub fn new()->HitState{
        HitState{
            finger_down_start:Vec::new(),
            was_over_last_call: false
        }
    }
}

#[derive(Clone, Default,Debug)]
pub struct FingerHoverEvent{
    pub x:f32,
    pub y:f32,
    pub handled:bool,
    pub hover_state:HoverState
}

#[derive(Clone, Default,Debug)]
pub struct FingerScrollEvent{
    pub x:f32,
    pub y:f32,
    pub dx:f32,
    pub dy:f32,
    pub handled:bool,
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
    AppFocus(bool),
    AnimationEnded(AnimateEvent),
    Animate(AnimateEvent),
    CloseRequested,
    Resized(ResizedEvent),
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

pub enum HitTouch{
    Single,
    Multi
}

impl Event{
    pub fn set_handled(&mut self, set:bool){
        match self{
            Event::FingerHover(fe)=>{
                fe.handled = set;
            },
            Event::FingerScroll(fe)=>{
                fe.handled = set;
            },
            Event::FingerDown(fe)=>{
                fe.handled = set;
            },
            _=>()
        }
    }

    pub fn handled(&self)->bool{
        match self{
            Event::FingerHover(fe)=>{
                fe.handled
            },
            Event::FingerScroll(fe)=>{
                fe.handled
            },
            Event::FingerDown(fe)=>{
                fe.handled
            }, 
            _=>false
        }
    }

    pub fn hits(&mut self, cx:&mut Cx, area:Area, hit_state:&mut HitState, hit_touch:HitTouch)->Event{
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
           
            Event::FingerHover(fe)=>{
                if hit_state.was_over_last_call{
                    if area.contains(fe.x, fe.y, &cx) && !fe.handled{
                        fe.handled = true;
                        if let HoverState::Out = fe.hover_state{
                            hit_state.was_over_last_call = false;
                        }
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
                    if area.contains(fe.x, fe.y, &cx) && !fe.handled{
                        fe.handled = true;
                        hit_state.was_over_last_call = true;
                        return Event::FingerHover(FingerHoverEvent{
                            hover_state:HoverState::In,
                            ..fe.clone()
                        })
                    }
                }
            },
            Event::FingerMove(fe)=>{
                // check wether our digit is captured, otherwise don't send
                if cx.captured_fingers[fe.digit] == area{
                    let start = if hit_state.finger_down_start.len() <= fe.digit{
                        vec2(0.,0.)
                    }
                    else{
                        hit_state.finger_down_start[fe.digit]
                    };
                    return Event::FingerMove(FingerMoveEvent{
                        start_x: start.x,
                        start_y: start.y,
                        is_over:area.contains(fe.x, fe.y, &cx),
                        ..fe.clone()
                    })
                }
            },
            Event::FingerDown(fe)=>{
                if !fe.handled && area.contains(fe.x, fe.y, &cx){
                    // scan if any of the fingers already captured this area
                    if let HitTouch::Single = hit_touch{
                        for fin_area in &cx.captured_fingers{
                            if *fin_area == area{
                                return Event::None;
                            }
                        }
                    }
                    cx.captured_fingers[fe.digit] = area;
                    // store the start point, make room in the vector for the digit.
                    if hit_state.finger_down_start.len() < fe.digit+1{
                        for _i in hit_state.finger_down_start.len()..(fe.digit+1){
                            hit_state.finger_down_start.push(vec2(0.,0.));
                        }
                    }
                    hit_state.finger_down_start[fe.digit] = vec2(fe.x, fe.y);
                    fe.handled = true;
                    return self.clone();
                }
            },
            Event::FingerUp(fe)=>{
                if cx.captured_fingers[fe.digit] == area{
                    cx.captured_fingers[fe.digit] = Area::Empty;
                    let start = if hit_state.finger_down_start.len() <= fe.digit{
                        vec2(0.,0.)
                    }
                    else{
                        hit_state.finger_down_start[fe.digit]
                    };
                    return Event::FingerUp(FingerUpEvent{
                        is_over:area.contains(fe.x, fe.y, &cx),
                        start_x: start.x,
                        start_y: start.y,
                        ..fe.clone()
                    })
                }
            },
            _=>()
        };
        return Event::None;
    }
}