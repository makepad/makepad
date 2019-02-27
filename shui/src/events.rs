use crate::cx::*;
use crate::cxdrawing::*;

#[derive(Clone, Default,Debug)]
pub struct FingerEvent{
    pub x:f32,
    pub y:f32,
    pub digit:u32,
    pub button:u32,
    pub touch:bool,
    pub x_wheel:f32,
    pub y_wheel:f32
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
    Animate(AnimateEvent),
    CloseRequested,
    Resized(ResizedEvent),
    CapturedMove(CapturedEvent),
    FingerDown(FingerEvent),
    FingerMove(FingerEvent),
    FingerHover(FingerEvent),
    FingerUp(FingerEvent),
    FingerWheel(FingerEvent)
}

impl Default for Event{
    fn default()->Event{
        Event::None
    }
}

impl Event{
    pub fn hits(&self, area:&Area, cx:&mut Cx)->&Event{
        match self{
            Event::Animate(_)=>{
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
                if area.contains(fe.x, fe.y, cx){
                    return self;
                }
            },
            Event::FingerDown(fe)=>{
                if area.contains(fe.x, fe.y, cx){
                    return self;
                }
            },
            Event::FingerUp(fe)=>{
                if area.contains(fe.x, fe.y, cx){
                    return self;
                }
            },
            _=>()
        };
        return &Event::None;
    }
}