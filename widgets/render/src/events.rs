use crate::cx::*;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct KeyModifier{
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool
}

#[derive(Clone, Default,Debug, PartialEq)]
pub struct FingerDownEvent{
    pub abs:Vec2,
    pub rel:Vec2,
    pub rect:Rect,
    pub digit:usize,
    pub handled:bool,
    pub is_touch:bool,
    pub modifier:KeyModifier
}

#[derive(Clone, Default,Debug, PartialEq)]
pub struct FingerMoveEvent{
    pub abs:Vec2,
    pub abs_start:Vec2,
    pub rel:Vec2,
    pub rel_start:Vec2,
    pub rect:Rect,
    pub is_over:bool,
    pub digit:usize,
    pub is_touch:bool,
    pub modifier:KeyModifier
}

impl FingerMoveEvent{
    pub fn move_distance(&self)->f32{
        ((self.abs_start.x - self.abs.x).powf(2.) + (self.abs_start.y - self.abs.y).powf(2.)).sqrt()
    }
}

#[derive(Clone, Default,Debug, PartialEq)]
pub struct FingerUpEvent{
    pub abs:Vec2,
    pub abs_start:Vec2,
    pub rel:Vec2,
    pub rel_start:Vec2,
    pub rect:Rect,
    pub digit:usize,
    pub is_over:bool,
    pub is_touch:bool,
    pub modifier:KeyModifier
}

#[derive(Clone,Debug, PartialEq)]
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

#[derive(Clone,Debug,Default)]
pub struct HitState{
    pub use_multi_touch:bool,
    pub no_scrolling:bool,
    pub margin:Option<Margin>,
    pub finger_down_abs_start:Vec<Vec2>,
    pub finger_down_rel_start:Vec<Vec2>,
    pub was_over_last_call:bool
}

#[derive(Clone, Default,Debug, PartialEq)]
pub struct FingerHoverEvent{
    pub abs:Vec2,
    pub rel:Vec2,
    pub rect:Rect,
    pub handled:bool,
    pub hover_state:HoverState,
    pub modifier:KeyModifier
}

#[derive(Clone, Default,Debug, PartialEq)]
pub struct FingerScrollEvent{
    pub abs:Vec2,
    pub rel:Vec2,
    pub rect:Rect,
    pub scroll:Vec2,
    pub is_wheel:bool,
    pub handled:bool,
    pub modifier:KeyModifier
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct ResizedEvent{
    pub old_size:Vec2,
    pub old_dpi_factor:f32,
    pub new_size:Vec2,
    pub new_dpi_factor:f32
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct AnimateEvent{
    pub frame:u64,
    pub time:f64
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct FrameEvent{
    pub frame:u64,
    pub time:f64
}

#[derive(Clone, Default, Debug)]
pub struct RedrawEvent{
    pub area:Area
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileReadEvent{
    pub id:u64,
    pub data:Result<Vec<u8>, String>
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileWriteEvent{
    id:u64,
    error:Option<String>
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyEvent{
    pub key_code:KeyCode,
    pub key_char:char,
    pub is_repeat:bool,
    pub modifier:KeyModifier
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyFocusEvent{
    pub last:Area,
    pub focus:Area,
    pub is_lost:bool
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextInputEvent{
    pub input:String,
    pub replace_last:bool
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event{
    None,
    Construct,
    Destruct,
    Draw,
    AppFocus(bool),
    AnimationEnded(AnimateEvent),
    Animate(AnimateEvent),
    Frame(FrameEvent),
    CloseRequested,
    Resized(ResizedEvent),
    FingerDown(FingerDownEvent),
    FingerMove(FingerMoveEvent),
    FingerHover(FingerHoverEvent),
    FingerUp(FingerUpEvent),
    FingerScroll(FingerScrollEvent),
    FileRead(FileReadEvent),
    FileWrite(FileWriteEvent),
    KeyFocus(KeyFocusEvent),
    KeyDown(KeyEvent),
    KeyUp(KeyEvent),
    TextInput(TextInputEvent)
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

    pub fn hits(&mut self, cx:&mut Cx, area:Area, hit_state:&mut HitState)->Event{
        match self{
            Event::KeyFocus(kf)=>{
                if area == kf.last{
                    return Event::KeyFocus(KeyFocusEvent{
                        is_lost:true,   
                        ..kf.clone()
                    })
                }
                else if area == kf.focus{
                    return Event::KeyFocus(KeyFocusEvent{
                        is_lost:false,   
                        ..kf.clone()
                    })
                }
            },
            Event::KeyDown(_)=>{
                if area == cx.key_focus{
                    return self.clone();
                }
            },
            Event::KeyUp(_)=>{
                if area == cx.key_focus{
                    return self.clone();
                }
            },
            Event::TextInput(_)=>{
                if area == cx.key_focus{
                    return self.clone();
                }
            },
            Event::Animate(_)=>{
                for anim in &cx.playing_anim_areas{
                    if anim.area == area{
                        return self.clone()
                    }
                }
            },
            Event::Frame(_)=>{
                for area in &cx.frame_callbacks{
                    if area == area{
                        return self.clone()
                    }
                }
            },
            Event::AnimationEnded(_)=>{
                for anim in &cx.ended_anim_areas{
                    if anim.area == area{
                        return self.clone()
                    }
                }
            },
           
            Event::FingerHover(fe)=>{
                let rect = if hit_state.no_scrolling{
                    area.get_rect_no_scrolling(&cx)
                }
                else{
                    area.get_rect(&cx)
                };
                if hit_state.was_over_last_call{

                    if !fe.handled && rect.contains_with_margin(fe.abs.x, fe.abs.y, &hit_state.margin){
                        fe.handled = true;
                        if let HoverState::Out = fe.hover_state{
                            hit_state.was_over_last_call = false;
                        }
                        return Event::FingerHover(FingerHoverEvent{
                            rel:Vec2{x:fe.abs.x - rect.x, y:fe.abs.y - rect.y},
                            rect:rect,
                            ..fe.clone()
                        })
                    }
                    else{
                        hit_state.was_over_last_call = false;
                        return Event::FingerHover(FingerHoverEvent{
                            rel:Vec2{x:fe.abs.x - rect.x, y:fe.abs.y - rect.y},
                            rect:rect,
                            hover_state:HoverState::Out,
                            ..fe.clone()
                        })
                    }
                }
                else{
                    if !fe.handled && rect.contains_with_margin(fe.abs.x, fe.abs.y, &hit_state.margin){
                        fe.handled = true;
                        hit_state.was_over_last_call = true;
                        return Event::FingerHover(FingerHoverEvent{
                            rel:Vec2{x:fe.abs.x - rect.x, y:fe.abs.y - rect.y},
                            rect:rect,
                            hover_state:HoverState::In,
                            ..fe.clone()
                        })
                    }
                }
            },
            Event::FingerMove(fe)=>{
                // check wether our digit is captured, otherwise don't send
                if cx.captured_fingers[fe.digit] == area{
                    let abs_start = if hit_state.finger_down_abs_start.len() <= fe.digit{
                        Vec2::zero()
                    }
                    else{
                        hit_state.finger_down_abs_start[fe.digit]
                    };
                    let rel_start = if hit_state.finger_down_rel_start.len() <= fe.digit{
                        Vec2::zero()
                    }
                    else{
                        hit_state.finger_down_rel_start[fe.digit]
                    };
                    let rect = if hit_state.no_scrolling{
                        area.get_rect_no_scrolling(&cx)
                    }
                    else{
                        area.get_rect(&cx)
                    };
                    return Event::FingerMove(FingerMoveEvent{
                        abs_start: abs_start,
                        rel:Vec2{x:fe.abs.x - rect.x, y:fe.abs.y - rect.y},
                        rel_start: rel_start,
                        rect:rect,
                        is_over:rect.contains_with_margin(fe.abs.x, fe.abs.y, &hit_state.margin),
                        ..fe.clone()
                    })
                }
            },
            Event::FingerDown(fe)=>{
                if !fe.handled{
                    let rect = if hit_state.no_scrolling{
                        area.get_rect_no_scrolling(&cx)
                    }
                    else{
                        area.get_rect(&cx)
                    };
                    if rect.contains_with_margin(fe.abs.x, fe.abs.y, &hit_state.margin){
                        // scan if any of the fingers already captured this area
                        if !hit_state.use_multi_touch{
                            for fin_area in &cx.captured_fingers{
                                if *fin_area == area{
                                    return Event::None;
                                }
                            }
                        }
                        cx.captured_fingers[fe.digit] = area;
                        // store the start point, make room in the vector for the digit.
                        if hit_state.finger_down_abs_start.len() < fe.digit+1{
                            for _i in hit_state.finger_down_abs_start.len()..(fe.digit+1){
                                hit_state.finger_down_abs_start.push(Vec2{x:0., y:0.});
                            }
                        }
                        if hit_state.finger_down_rel_start.len() < fe.digit+1{
                            for _i in hit_state.finger_down_rel_start.len()..(fe.digit+1){
                                hit_state.finger_down_rel_start.push(Vec2{x:0., y:0.});
                            }
                        }
                        hit_state.finger_down_abs_start[fe.digit] = fe.abs;
                        hit_state.finger_down_rel_start[fe.digit] = Vec2{x:fe.abs.x - rect.x, y:fe.abs.y - rect.y};
                        fe.handled = true;
                        return Event::FingerDown(FingerDownEvent{
                            rel:Vec2{x:fe.abs.x - rect.x, y:fe.abs.y - rect.y},
                            rect:rect,
                            ..fe.clone()
                        })
                    }
                }
            },
            Event::FingerUp(fe)=>{
                if cx.captured_fingers[fe.digit] == area{
                    cx.captured_fingers[fe.digit] = Area::Empty;
                    let abs_start = if hit_state.finger_down_abs_start.len() <= fe.digit{
                        Vec2::zero()
                    }
                    else{
                        hit_state.finger_down_abs_start[fe.digit]
                    };
                    let rel_start = if hit_state.finger_down_rel_start.len() <= fe.digit{
                        Vec2::zero()
                    }
                    else{
                        hit_state.finger_down_rel_start[fe.digit]
                    };
                    let rect = if hit_state.no_scrolling{
                        area.get_rect_no_scrolling(&cx)
                    }
                    else{
                        area.get_rect(&cx)
                    };
                    return Event::FingerUp(FingerUpEvent{
                        is_over:rect.contains(fe.abs.x, fe.abs.y),
                        abs_start: abs_start,
                        rel_start: rel_start,
                        rel:Vec2{x:fe.abs.x - rect.x, y:fe.abs.y - rect.y},
                        rect:rect,
                        ..fe.clone()
                    })
                }
            },
            _=>()
        };
        return Event::None;
    }
}

// lowest common denominator keymap between desktop and web
#[derive(Clone, PartialEq, Debug)]
pub enum KeyCode {
    Escape,

    Backtick,
    Key0,
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
    Minus,
    Equals,

    Backspace,
    Tab,

    KeyQ,
    KeyW,
    KeyE,
    KeyR,
    KeyT,
    KeyY,
    KeyU,
    KeyI,
    KeyO,
    KeyP,
    LBracket,
    RBracket,
    Return,

    KeyA,
    KeyS,
    KeyD,
    KeyF,
    KeyG,
    KeyH,
    KeyJ,
    KeyK,
    KeyL,
    Semicolon,
    Quote,
    Backslash,

    KeyZ,
    KeyX,
    KeyC,
    KeyV,
    KeyB,
    KeyN,
    KeyM,
    Comma,
    Period,
    Slash,

    LeftControl,
    LeftAlt,
    LeftShift,
    LeftLogo,

    RightControl,
    RightShift,
    RightAlt,
    RightLogo,

    Space,
    Capslock,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
 
    PrintScreen,
    Scrolllock,
    Pause,
 
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,

    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,

    NumpadEquals,
    NumpadSubtract,
    NumpadAdd,
    NumpadDecimal,
    NumpadMultiply,
    NumpadDivide,
    Numlock,
    NumpadEnter,

    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
}

