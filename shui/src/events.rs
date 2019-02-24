use crate::cx::*;
use crate::cxdrawing::*;

#[derive(Clone, Default,Debug)]
pub struct FingerEvent{
    pub pos:Vec2
}

#[derive(Clone, Default, Debug)]
pub struct CapturedEvent{
    pub area:Area,
    pub pos:Vec2
}

#[derive(Clone, Default, Debug)]
pub struct ResizedEvent{
    pub old_size:Vec2,
    pub old_dpi_factor:f32,
    pub new_size:Vec2,
    pub new_dpi_factor:f32
}

#[derive(Clone,Debug)]
pub enum Event{
    None,
    Redraw,
    Animate,
    CloseRequested,
    Resized(ResizedEvent),
    CapturedMove(CapturedEvent),
    FingerMove(FingerEvent),
    FingerDown(FingerEvent),
    FingerUp(FingerEvent),
}

impl Default for Event{
    fn default()->Event{
        Event::None
    }
}

impl Event{
    pub fn hits(&self, area:&Area, cx:&Cx)->&Event{
        match self{
            Event::Animate=>{
                for anim in &cx.animations{
                    if anim.area == *area{
                        return self
                    }
                }
            },
            Event::CapturedMove(cm)=>{
                if cm.area == *area{
                    return self;
                }
            },
            Event::FingerMove(fe)=>{
                if area.contains(&fe.pos,cx){
                    return self;
                }
            },
            Event::FingerDown(fe)=>{
                if area.contains(&fe.pos,cx){
                    return self;
                }
            },
            Event::FingerUp(fe)=>{
                if area.contains(&fe.pos,cx){
                    return self;
                }
            },
            _=>()
        };
        return &Event::None;
    }
}