// OK ANIMATION
use std::f64::consts::PI;
use crate::cx::*;

live_register!{
    Play: Enum {rust_type: {{Play}}}
    Ease: Enum {rust_type: {{Ease}}}
}

#[derive(Default)]
pub struct Animator{
    pub state_id: Id,
    pub live_ptr: Option<LivePtr>,
    pub current_state: Vec<GenNode>,
}

#[derive(Clone, LiveComponent, LiveComponentHooks, Debug, PartialEq)]
pub enum Play {
    #[live{scale:1.0}] 
    Chain {scale: f64},
    
    #[default{scale:1.0}] 
    Cut {scale: f64},
    
    #[live{scale:1.0, end:1.0}] 
    Loop {scale: f64, end:f64},
    
    #[live{scale:1.0, end:1.0}] 
    Reverse {scale: f64, end:f64},
    
    #[live{scale:1.0, end:1.0}] 
    Bounce {scale: f64, end:f64},
}

impl Play {
    pub fn scale(&self) -> f64 {
        match self {
            Play::Chain {scale, ..} => *scale,
            Play::Cut {scale, ..} => *scale,
            Play::Loop {scale, ..} => *scale,
            Play::Reverse {scale, ..} => *scale,
            Play::Bounce {scale, ..} => *scale,
        }
    }
    
    pub fn cut(&self) -> bool {
        match self {
            Play::Cut {..} => true,
            Play::Chain {..} => false,
            Play::Loop {..} => true,
            Play::Reverse {..} => true,
            Play::Bounce {..} => true,
        }
    }
    
    pub fn scale_time(&self, time: f64) -> f64 {
        match self {
            Play::Cut {scale, ..} => {
                time * scale
            },
            Play::Chain {scale, ..} => {
                time * scale
            },
            Play::Loop {scale, end, ..} => {
                (time * scale) % end
            },
            Play::Reverse {end, scale, ..} => {
                end - (time * scale) % end
            },
            Play::Bounce {end, scale, ..} => {
                let mut local_time = (time * scale) % (end * 2.0);
                if local_time > *end {
                    local_time = 2.0 * end - local_time;
                };
                local_time
            },
        }
    }
}


#[derive(Clone, LiveComponent, LiveComponentHooks, Debug, PartialEq)]
pub enum Ease {
    #[default] Lin,
    #[live] InQuad,
    #[live] OutQuad,
    #[live] InOutQuad,
    #[live] InCubic,
    #[live] OutCubic,
    #[live] InOutCubic,
    #[live] InQuart,
    #[live] OutQuart,
    #[live] InOutQuart,
    #[live] InQuint,
    #[live] OutQuint,
    #[live] InOutQuint,
    #[live] InSine,
    #[live] OutSine,
    #[live] InOutSine,
    #[live] InExp,
    #[live] OutExp,
    #[live] InOutExp,
    #[live] InCirc,
    #[live] OutCirc,
    #[live] InOutCirc,
    #[live] InElastic,
    #[live] OutElastic,
    #[live] InOutElastic,
    #[live] InBack,
    #[live] OutBack,
    #[live] InOutBack,
    #[live] InBounce,
    #[live] OutBounce,
    #[live] InOutBounce,
    #[live{begin:0.0,end:1.0}] Pow {begin: f64, end: f64},
    #[live{cp0:0.0,cp1:0.0, cp2:1.0,cp3:1.0}] Bezier {cp0: f64, cp1: f64, cp2: f64, cp3: f64}
}



impl Ease {
    pub fn map(&self, t: f64) -> f64 {
        match self {
            Ease::Lin => {
                return t.max(0.0).min(1.0);
            },
            Ease::Pow {begin, end} => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }
                let a = -1. / (begin * begin).max(1.0);
                let b = 1. + 1. / (end * end).max(1.0);
                let t2 = (((a - 1.) * -b) / (a * (1. - b))).powf(t);
                return (-a * b + b * a * t2) / (a * t2 - b);
            },
            
            Ease::InQuad => {
                return t * t;
            },
            Ease::OutQuad => {
                return t * (2.0 - t);
            },
            Ease::InOutQuad => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t;
                }
                else {
                    let t = t - 1.;
                    return -0.5 * (t * (t - 2.) - 1.);
                }
            },
            Ease::InCubic => {
                return t * t * t;
            },
            Ease::OutCubic => {
                let t2 = t - 1.0;
                return t2 * t2 * t2 + 1.0;
            },
            Ease::InOutCubic => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return 1. / 2. * (t * t * t + 2.);
                }
            },
            Ease::InQuart => {
                return t * t * t * t
            },
            Ease::OutQuart => {
                let t = t - 1.;
                return -(t * t * t * t - 1.);
            },
            Ease::InOutQuart => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return -0.5 * (t * t * t * t - 2.);
                }
            },
            Ease::InQuint => {
                return t * t * t * t * t;
            },
            Ease::OutQuint => {
                let t = t - 1.;
                return t * t * t * t * t + 1.;
            },
            Ease::InOutQuint => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return 0.5 * (t * t * t * t * t + 2.);
                }
            },
            Ease::InSine => {
                return -(t * PI * 0.5).cos() + 1.;
            },
            Ease::OutSine => {
                return (t * PI * 0.5).sin();
            },
            Ease::InOutSine => {
                return -0.5 * ((t * PI).cos() - 1.);
            },
            Ease::InExp => {
                if t < 0.001 {
                    return 0.;
                }
                else {
                    return 2.0f64.powf(10. * (t - 1.));
                }
            },
            Ease::OutExp => {
                if t > 0.999 {
                    return 1.;
                }
                else {
                    return -(2.0f64.powf(-10. * t)) + 1.;
                }
            },
            Ease::InOutExp => {
                if t<0.001 {
                    return 0.;
                }
                if t>0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * 2.0f64.powf(10. * (t - 1.));
                }
                else {
                    let t = t - 1.;
                    return 0.5 * (-(2.0f64.powf(-10. * t)) + 2.);
                }
            },
            Ease::InCirc => {
                return -((1. - t * t).sqrt() - 1.);
            },
            Ease::OutCirc => {
                let t = t - 1.;
                return (1. - t * t).sqrt();
            },
            Ease::InOutCirc => {
                let t = t * 2.;
                if t < 1. {
                    return -0.5 * ((1. - t * t).sqrt() - 1.);
                }
                else {
                    let t = t - 2.;
                    return 0.5 * ((1. - t * t).sqrt() + 1.);
                }
            },
            Ease::InElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t - 1.0;
                return -(2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
            },
            Ease::OutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                return 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
            },
            Ease::InOutElastic => {
                let p = 0.3;
                let s = p / 4.0; // c = 1.0, b = 0.0, d = 1.0
                if t < 0.001 {
                    return 0.;
                }
                if t > 0.999 {
                    return 1.;
                }
                let t = t * 2.0;
                if t < 1. {
                    let t = t - 1.0;
                    return -0.5 * (2.0f64.powf(10.0 * t) * ((t - s) * (2.0 * PI) / p).sin());
                }
                else {
                    let t = t - 1.0;
                    return 0.5 * 2.0f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + 1.0;
                }
            },
            Ease::InBack => {
                let s = 1.70158;
                return t * t * ((s + 1.) * t - s);
            },
            Ease::OutBack => {
                let s = 1.70158;
                let t = t - 1.;
                return t * t * ((s + 1.) * t + s) + 1.;
            },
            Ease::InOutBack => {
                let s = 1.70158;
                let t = t * 2.0;
                if t < 1. {
                    let s = s * 1.525;
                    return 0.5 * (t * t * ((s + 1.) * t - s));
                }
                else {
                    let t = t - 2.;
                    return 0.5 * (t * t * ((s + 1.) * t + s) + 2.);
                }
            },
            Ease::InBounce => {
                return 1.0 - Ease::OutBounce.map(1.0 - t);
            },
            Ease::OutBounce => {
                if t < (1. / 2.75) {
                    return 7.5625 * t * t;
                }
                if t < (2. / 2.75) {
                    let t = t - (1.5 / 2.75);
                    return 7.5625 * t * t + 0.75;
                }
                if t < (2.5 / 2.75) {
                    let t = t - (2.25 / 2.75);
                    return 7.5625 * t * t + 0.9375;
                }
                let t = t - (2.625 / 2.75);
                return 7.5625 * t * t + 0.984375;
            },
            Ease::InOutBounce => {
                if t <0.5 {
                    return Ease::InBounce.map(t * 2.) * 0.5;
                }
                else {
                    return Ease::OutBounce.map(t * 2. - 1.) * 0.5 + 0.5;
                }
            },
            Ease::Bezier {cp0, cp1, cp2, cp3} => {
                if t < 0. {
                    return 0.;
                }
                if t > 1. {
                    return 1.;
                }
                
                if (cp0 - cp1).abs() < 0.001 && (cp2 - cp3).abs() < 0.001 {
                    return t;
                }
                
                let epsilon = 1.0 / 200.0 * t;
                let cx = 3.0 * cp0;
                let bx = 3.0 * (cp2 - cp0) - cx;
                let ax = 1.0 - cx - bx;
                let cy = 3.0 * cp1;
                let by = 3.0 * (cp3 - cp1) - cy;
                let ay = 1.0 - cy - by;
                let mut u = t;
                
                for _i in 0..6 {
                    let x = ((ax * u + bx) * u + cx) * u - t;
                    if x.abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }
                    let d = (3.0 * ax * u + 2.0 * bx) * u + cx;
                    if d.abs() < 1e-6 {
                        break;
                    }
                    u = u - x / d;
                };
                
                if t > 1. {
                    return (ay + by) + cy;
                }
                if t < 0. {
                    return 0.0;
                }
                
                let mut w = 0.0;
                let mut v = 1.0;
                u = t;
                for _i in 0..8 {
                    let x = ((ax * u + bx) * u + cx) * u;
                    if (x - t).abs() < epsilon {
                        return ((ay * u + by) * u + cy) * u;
                    }
                    
                    if t > x {
                        w = u;
                    }
                    else {
                        v = u;
                    }
                    u = (v - w) * 0.5 + w;
                }
                
                return ((ay * u + by) * u + cy) * u;
            }
        }
    }
}
