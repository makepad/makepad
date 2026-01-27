use crate::makepad_platform::*;
use std::f64::consts::PI;

script_mod!{
    mod.animator = {
        Animator: mod.std.set_type_default() do #(Animator::script_ext(vm)){
        }
        State: #(State::script_ext(vm)){
        }
        States: #(State::script_ext(vm)){
        }
        Play: #(Play::script_api(vm)),
        ..me.Play,
        Ease: #(Ease::script_api(vm)),
        ..me.Ease
    }
}

pub trait AnimatorImpl {
        
    fn animator_cut(&mut self, cx: &mut Cx, state: &[LiveId; 2]){
        self.animator_cut_scoped(cx, state, &mut Scope::empty())
    }
    fn animator_play(&mut self, cx: &mut Cx, state: &[LiveId; 2]){
        self.animator_play_scoped(cx, state, &mut Scope::empty())
    }
    fn animator_toggle_scoped(&mut self, cx: &mut Cx, is_state_1: bool, animate: Animate, state1: &[LiveId; 2], state2: &[LiveId; 2], scope:&mut Scope) {
        if is_state_1 {
            if let Animate::Yes = animate {
                self.animator_play_scoped(cx, state1, scope)
            }
            else {
                self.animator_cut_scoped(cx, state1, scope)
            }
        }
        else {
            if let Animate::Yes = animate {
                self.animator_play_scoped(cx, state2, scope)
            }
            else {
                self.animator_cut_scoped(cx, state2, scope)
            }
        }
    }
    fn animator_toggle(&mut self, cx: &mut Cx, is_state_1: bool, animate: Animate, state1: &[LiveId; 2], state2: &[LiveId; 2]) {
        self.animator_toggle_scoped(cx, is_state_1, animate, state1, state2, &mut Scope::empty())
    }

    fn animator_handle_event(&mut self, cx: &mut Cx, event: &Event) -> AnimatorAction{
        self.animator_handle_event_scoped(cx, event, &mut Scope::empty())
    }
    
    // implemented by proc macro
    fn animator_cut_scoped(&mut self, cx: &mut Cx, state: &[LiveId; 2], scope:&mut Scope);
    fn animator_play_scoped(&mut self, cx: &mut Cx, state: &[LiveId; 2], scope:&mut Scope);
    fn animator_in_state(&self, cx: &Cx, check_state_pair: &[LiveId; 2]) -> bool;
    fn animator_handle_event_scoped(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope) -> AnimatorAction;
}

#[derive(Debug, Clone, Copy)]
pub enum Animate {
    Yes,
    No
}

#[derive(Default, Script)]
struct States {
}

impl ScriptHook for States {
    fn on_custom_apply(&mut self, _vm: &mut ScriptVm, _apply: &Apply, _scope:&mut Scope, _value: ScriptValue) -> bool {
        true
    }
}

#[derive(Default, Script)]
struct State {
}

impl ScriptHook for State {
    fn on_custom_apply(&mut self, vm: &mut ScriptVm, _apply: &Apply, _scope:&mut Scope, value: ScriptValue) -> bool {
        true
    }
}


#[derive(Default, Script)]
pub struct Animator {
    #[rust] pub next_frame: NextFrame,
}

impl ScriptHook for Animator {
    fn on_custom_apply(&mut self, vm: &mut ScriptVm, _apply: &Apply, _scope:&mut Scope, value: ScriptValue) -> bool {
        let Some(obj) = value.as_object() else {
            return false;
        };
/*        
        let font_family_id = self.to_font_family_id();
        if !fonts.is_font_family_known(font_family_id) {
            let mut font_ids = Vec::new();
                        
            let len = vm.heap.vec_len(obj);
            for i in 0..len {
                let kv = vm.heap.vec_key_value(obj, i, &vm.thread.trap);
                let member = FontMember::script_from_value(vm, kv.value);
                                
                if let Some(ref handle_ref) = member.res {
                    let handle = handle_ref.as_handle();
                    let font_id: FontId = (handle.index() as u64).into();
                                        
                    if !fonts.is_font_known(font_id) {
                        let cx = vm.host.cx_mut();
                        if let Some(data) = cx.get_resource(handle) {
                            fonts.define_font(
                                font_id,
                                FontDefinition {
                                    data,
                                    index: 0,
                                    ascender_fudge_in_ems: member.asc,
                                    descender_fudge_in_ems: member.desc,
                                },
                            );
                        }
                    }
                    font_ids.push(font_id);
                }
            }
                        
            fonts.define_font_family(font_family_id, FontFamilyDefinition { font_ids });
        }*/ 
        true
    }
}

#[derive(Copy, Clone)]
pub enum AnimatorAction {
    Animating {redraw: bool},
    None
}

impl Animator{
    pub fn play(&mut self, _cx:&mut Cx, _state: &[LiveId;2])->Option<ScriptValue>{
        None
    }
    
    pub fn cut(&mut self, _cx:&mut Cx, _state: &[LiveId;2])->Option<ScriptValue>{
        None
    }
    
    pub fn in_state(&self, _cx:&Cx, _state: &[LiveId;2])->bool{
        false
    }   
    
    pub fn handle_event(&mut self, _cx:&mut Cx, _event:&Event, _act:&mut AnimatorAction)->Option<ScriptValue>{
        None
    }          
}


// deserialisable DSL structure
#[derive(Debug, Clone, Script, ScriptHook)]
pub struct KeyFrame {
    #[live(Ease::Linear)]
    pub ease: Ease,
            
    #[live(1.0)]
    pub time: f64,
            
    #[live(NIL)]
    pub value: ScriptValue,
}

#[derive(Copy, Clone, Debug, PartialEq, Script, ScriptHook)]
pub enum Play {
    #[pick {duration: 1.0}]
    Forward {duration: f64},
            
    Snap,
            
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
    /*
    pub fn duration(&self) -> f64 {
        match self {
            Self::Forward {duration, ..} => *duration,
            Self::Reverse {duration, ..} => *duration,
            Self::Loop {duration, ..} => *duration,
            Self::ReverseLoop {duration, ..} => *duration,
            Self::BounceLoop {duration, ..} => *duration,
        }
    }*/
            
    pub fn get_ended_time(&self, time: f64) -> (bool, f64) {
        match self {
            Self::Snap => (true, 1.0),
            Self::Forward {duration} => {
                if *duration == 0.0 {return (true, 1.0)}
                (time > *duration, time.min(*duration) / duration)
            },
            Self::Reverse {duration, end} => {
                if *duration == 0.0 {return (true, 1.0)}
                (time > *duration, end - (time.min(*duration) / duration))
            },
            Self::Loop {duration, end} => {
                if *duration == 0.0 {return (true, 1.0)}
                (false, (time / duration) % end)
            },
            Self::ReverseLoop {end, duration} => {
                if *duration == 0.0 {return (true, 1.0)}
                (false, end - (time / duration) % end)
            },
            Self::BounceLoop {end, duration} => {
                if *duration == 0.0 {return (true, 1.0)}
                let mut local_time = (time / duration) % (end * 2.0);
                if local_time > *end {
                    local_time = 2.0 * end - local_time;
                };
                (false, local_time)
            },
        }
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
pub enum Ease {
    #[pick] Linear,
    #[live] None,
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
    #[live {d1: 0.82, d2: 0.97, max: 100}] ExpDecay {d1: f64, d2: f64, max: usize},
            
    #[live {begin: 0.0, end: 1.0}] Pow {begin: f64, end: f64},
    #[live {cp0: 0.0, cp1: 0.0, cp2: 1.0, cp3: 1.0}] Bezier {cp0: f64, cp1: f64, cp2: f64, cp3: f64}
}

impl Ease {
    pub fn map(&self, t: f64) -> f64 {
        match self {
            Self::ExpDecay {d1, d2, max} => { // there must be a closed form for this
                if t > 0.999 {
                    return 1.0;
                }
                                
                // first we count the number of steps we'd need to decay
                let mut di = *d1;
                let mut dt = 1.0;
                let max_steps = (*max).min(1000);
                let mut steps = 0;
                // for most of the settings we use this takes max 15 steps or so
                while dt > 0.001 && steps < max_steps {
                    steps = steps + 1;
                    dt = dt * di;
                    di *= d2;
                }
                // then we know how to find the step, and lerp it
                let step = t * (steps as f64);
                let mut di = *d1;
                let mut dt = 1.0;
                let max_steps = max_steps as f64;
                let mut steps = 0.0;
                while dt > 0.001 && steps < max_steps {
                    steps += 1.0;
                    if steps >= step { // right step
                        let fac = steps - step;
                        return 1.0 - (dt * fac + (dt * di) * (1.0 - fac))
                    }
                    dt = dt * di;
                    di *= d2;
                }
                1.0
            }
            Self::Linear => {
                return t.max(0.0).min(1.0);
            },
            Self::Constant(t) => {
                return t.max(0.0).min(1.0);
            },
            Self::None => {
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
