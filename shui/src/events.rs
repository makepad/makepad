use crate::math::*;
use crate::cx::*;
use crate::cxdrawing::*;

#[derive(Clone, Default,Debug)]
pub struct FingerEvent{
    pos:Vec2
}

#[derive(Clone, Default, Debug)]
pub struct CapturedMove{
    area:Area,
    pos:Vec2
}

#[derive(Clone,Debug)]
pub enum Ev{
    None,
    Redraw,
    Animate,
    FingerCapturedMove(CapturedMove),
    FingerMove(FingerEvent),
    FingerDown(FingerEvent),
    FingerUp(FingerEvent),
}

impl Default for Ev{
    fn default()->Ev{
        Ev::None
    }
}

impl Ev{
    pub fn hit(&self, area:&Area, cx:&Cx)->bool{
        match self{
            Ev::FingerCapturedMove(cm)=>{
                cm.area == *area
            },
            Ev::FingerMove(fe)=>area.contains(&fe.pos,cx),
            Ev::FingerDown(fe)=>area.contains(&fe.pos,cx),
            Ev::FingerUp(fe)=>area.contains(&fe.pos,cx),
            _=>false
        }
    }
}