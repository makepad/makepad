// OK ANIMATION
use std::f64::consts::PI;
use crate::cx::*;

live_register!{
    Play: Enum {rust_type: {{Play}}}
    Ease: Enum {rust_type: {{Ease}}}
    KeyFrame: Struct {rust_type: {{KeyFrame}}}
}

// deserialisable DSL structure
#[derive(Debug, LiveComponent, LiveComponentHooks)]
pub struct KeyFrame {
    #[live(Ease::Linear)]
    pub ease: Ease,
    
    #[live(1.0)]
    pub time: f64,
    
    #[live(KeyFrameValue::None)]
    pub value: KeyFrameValue,
}


#[derive(Debug)]
pub enum KeyFrameValue {
    None,
    Float(f64),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    Id(Id)
}

pub struct Animator {
    pub state_id: Id,
    pub live_ptr: Option<LivePtr>,
    pub current_state: Option<Vec<LiveNode >>,
}

impl Default for Animator{
    fn default()->Self{
        Self{
            state_id: Id(0),
            live_ptr:None,
            current_state:Some(Vec::new())
        }
    }
}

// OK so.. now the annoying bit
impl Animator {
    
    pub fn has_state(&self)->bool{
        self.current_state.is_some()
    }
    
    pub fn swap_out_state(&mut self) -> Vec<LiveNode> {
        self.current_state.take().unwrap()
    }
    
    pub fn swap_in_state(&mut self, state: Vec<LiveNode>) {
        self.current_state = Some(state);
    }
    
    // this find the last keyframe value from an array node
    pub fn find_last_keyframe_value(index:usize, nodes:&[LiveNode])->Option<usize>{
        if nodes[index].value.is_array(){
            if let Some(index) = nodes.last_child(index){
                if nodes[index].value.is_bare_class(){
                    if let Ok(value) = nodes.child_by_name(index, id!(value)){
                        return Some(value)
                    }
                }
            }
        }
        return None
    }
    /*
    pub fn init_from_last_keyframe(&mut self, cx: &mut Cx,  nodes: &[LiveNode]) {
        
        let mut index = 0;
        let mut current_state = Vec::new();
        while index < nodes.len() {
            let node = &nodes[index];
            match node.value {
                LiveValue::Array => { // its a keyframe array. probably :)
                    if let Some(last_child) = nodes.last_child(index) {
                        if let LiveValue::BareClass = nodes[last_child].value {
                            
                            let mut kf = KeyFrame::new(cx);
                            kf.apply_index(cx, ApplyFrom::DataNew, last_child, nodes);
                            
                            current_state.push(LiveNode {
                                token_id: None,
                                id: node.id,
                                value: kf.value.to_live_value()
                            });
                        }
                        else if !nodes[last_child].value.is_tree() { // if its a bare value push it in ?
                            current_state.push(LiveNode {
                                token_id: None,
                                id: node.id,
                                value: nodes[last_child].value.clone()
                            });
                        }
                    }
                    index = nodes.skip_node(index);
                }
                _ => { // just copy the value
                    current_state.push(node.clone());
                    index += 1;
                }
            }
        }
        self.current_state = Some(current_state);
    }*/
    // alright so . we have the from info
    // we have values.. we have KeyFrames and KeyFrameValues
    // now make an animation engine :)
    
}

#[derive(Clone, LiveComponent, LiveComponentHooks, Debug, PartialEq)]
pub enum Play {
    #[default {duration: 1.0}]
    Forward {duration: f64},
    
    #[live {duration: 1.0, end: 1.0}]
    Reverse {duration: f64, end: f64},
    
    #[live {duration: 1.0, end: 1.0}]
    Loop {duration: f64, end: f64},
    
    #[live {duration: 1.0, end: 1.0}]
    ReverseLoop {duration: f64, end: f64},
    
    #[live {duration: 1.0, end: 1.0}]
    BounceLoop {duration: f64, end: f64},
}

impl Play {
    pub fn duration(&self) -> f64 {
        match self {
            Self::Forward {duration, ..} => *duration,
            Self::Reverse {duration, ..} => *duration,
            Self::Loop {duration, ..} => *duration,
            Self::ReverseLoop {duration, ..} => *duration,
            Self::BounceLoop {duration, ..} => *duration,
        }
    }
    
    pub fn get_time(&self, time: f64) -> f64 {
        match self {
            Self::Forward {duration} => {
                time / duration
            },
            Self::Reverse {duration, end} => {
                end - (time / duration)
            },
            Self::Loop {duration, end} => {
                (time / duration) % end
            },
            Self::ReverseLoop {end, duration} => {
                end - (time / duration) % end
            },
            Self::BounceLoop {end, duration} => {
                let mut local_time = (time / duration) % (end * 2.0);
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
    #[default] Linear,
    #[live] One,
    #[live(1.0)] Constant(f64),
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
    #[live {begin: 0.0, end: 1.0}] Pow {begin: f64, end: f64},
    #[live {cp0: 0.0, cp1: 0.0, cp2: 1.0, cp3: 1.0}] Bezier {cp0: f64, cp1: f64, cp2: f64, cp3: f64}
}

impl Ease {
    pub fn map(&self, t: f64) -> f64 {
        match self {
            Self::Linear => {
                return t.max(0.0).min(1.0);
            },
            Self::Constant(t) => {
                return t.max(0.0).min(1.0);
            },
            Self::One => {
                return 1.0;
            },
            Self::Pow {begin, end} => {
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
            
            Self::InQuad => {
                return t * t;
            },
            Self::OutQuad => {
                return t * (2.0 - t);
            },
            Self::InOutQuad => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t;
                }
                else {
                    let t = t - 1.;
                    return -0.5 * (t * (t - 2.) - 1.);
                }
            },
            Self::InCubic => {
                return t * t * t;
            },
            Self::OutCubic => {
                let t2 = t - 1.0;
                return t2 * t2 * t2 + 1.0;
            },
            Self::InOutCubic => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return 1. / 2. * (t * t * t + 2.);
                }
            },
            Self::InQuart => {
                return t * t * t * t
            },
            Self::OutQuart => {
                let t = t - 1.;
                return -(t * t * t * t - 1.);
            },
            Self::InOutQuart => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return -0.5 * (t * t * t * t - 2.);
                }
            },
            Self::InQuint => {
                return t * t * t * t * t;
            },
            Self::OutQuint => {
                let t = t - 1.;
                return t * t * t * t * t + 1.;
            },
            Self::InOutQuint => {
                let t = t * 2.0;
                if t < 1. {
                    return 0.5 * t * t * t * t * t;
                }
                else {
                    let t = t - 2.;
                    return 0.5 * (t * t * t * t * t + 2.);
                }
            },
            Self::InSine => {
                return -(t * PI * 0.5).cos() + 1.;
            },
            Self::OutSine => {
                return (t * PI * 0.5).sin();
            },
            Self::InOutSine => {
                return -0.5 * ((t * PI).cos() - 1.);
            },
            Self::InExp => {
                if t < 0.001 {
                    return 0.;
                }
                else {
                    return 2.0f64.powf(10. * (t - 1.));
                }
            },
            Self::OutExp => {
                if t > 0.999 {
                    return 1.;
                }
                else {
                    return -(2.0f64.powf(-10. * t)) + 1.;
                }
            },
            Self::InOutExp => {
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
            Self::InCirc => {
                return -((1. - t * t).sqrt() - 1.);
            },
            Self::OutCirc => {
                let t = t - 1.;
                return (1. - t * t).sqrt();
            },
            Self::InOutCirc => {
                let t = t * 2.;
                if t < 1. {
                    return -0.5 * ((1. - t * t).sqrt() - 1.);
                }
                else {
                    let t = t - 2.;
                    return 0.5 * ((1. - t * t).sqrt() + 1.);
                }
            },
            Self::InElastic => {
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
            Self::OutElastic => {
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
            Self::InOutElastic => {
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
            Self::InBack => {
                let s = 1.70158;
                return t * t * ((s + 1.) * t - s);
            },
            Self::OutBack => {
                let s = 1.70158;
                let t = t - 1.;
                return t * t * ((s + 1.) * t + s) + 1.;
            },
            Self::InOutBack => {
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
            Self::InBounce => {
                return 1.0 - Self::OutBounce.map(1.0 - t);
            },
            Self::OutBounce => {
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
            Self::InOutBounce => {
                if t <0.5 {
                    return Self::InBounce.map(t * 2.) * 0.5;
                }
                else {
                    return Self::OutBounce.map(t * 2. - 1.) * 0.5 + 0.5;
                }
            },
            Self::Bezier {cp0, cp1, cp2, cp3} => {
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
