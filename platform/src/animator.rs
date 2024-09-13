
use {
    std::f64::consts::PI,
    crate::{
        makepad_live_compiler::{
            LiveRef,
            LiveNodeReader,
            LiveNodeOrigin,
            LiveValue,
            LiveTypeInfo,
            LiveTypeField,
            LivePropType,
            LiveFieldKind,
            LiveModuleId,
            LiveType,
            LiveNode,
            LiveIdAsProp,
            LiveNodeSliceApi,  
            LiveNodeVecApi
        },
        scope::Scope,
        makepad_live_tokenizer::{LiveErrorOrigin, live_error_origin},
        makepad_live_id::*,
        makepad_derive_live::*,
        makepad_math::*,
        cursor::MouseCursor,
        event::{Event, NextFrame},
        cx::Cx,
        live_traits::*,
    },
};

pub trait AnimatorImpl {
    
    fn animator_cut(&mut self, cx: &mut Cx, state: &[LiveId; 2]){
        self.animator_cut_with_scope(cx, state, &mut Scope::empty())
    }
    fn animator_play(&mut self, cx: &mut Cx, state: &[LiveId; 2]){
        self.animator_play_with_scope(cx, state, &mut Scope::empty())
    }
    fn animator_cut_with_scope(&mut self, cx: &mut Cx, state: &[LiveId; 2], scope:&mut Scope);
    fn animator_play_with_scope(&mut self, cx: &mut Cx, state: &[LiveId; 2], scope:&mut Scope);
    fn animator_toggle_with_scope(&mut self, cx: &mut Cx, is_state_1: bool, animate: Animate, state1: &[LiveId; 2], state2: &[LiveId; 2], scope:&mut Scope) {
        if is_state_1 {
            if let Animate::Yes = animate {
                self.animator_play_with_scope(cx, state1, scope)
            }
            else {
                self.animator_cut_with_scope(cx, state1, scope)
            }
        }
        else {
            if let Animate::Yes = animate {
                self.animator_play_with_scope(cx, state2, scope)
            }
            else {
                self.animator_cut_with_scope(cx, state2, scope)
            }
        }
    }
    fn animator_toggle(&mut self, cx: &mut Cx, is_state_1: bool, animate: Animate, state1: &[LiveId; 2], state2: &[LiveId; 2]) {
        self.animator_toggle_with_scope(cx, is_state_1, animate, state1, state2, &mut Scope::empty())
    }
    fn animator_in_state(&self, cx: &Cx, check_state_pair: &[LiveId; 2]) -> bool;
    fn animator_apply_state(&mut self, cx: &mut Cx, scope:&mut Scope);
    fn animator_after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]);
    fn animator_handle_event(&mut self, cx: &mut Cx, event: &Event) -> AnimatorAction{
        self.animator_handle_event_with_scope(cx, event, &mut Scope::empty())
    }
    fn animator_handle_event_with_scope(&mut self, cx: &mut Cx, event: &Event, scope:&mut Scope) -> AnimatorAction;
}

#[derive(Debug, Clone, Copy)]
pub enum Animate {
    Yes,
    No
}

// deserialisable DSL structure
#[derive(Debug, Clone, Live, LiveHook)]
pub struct KeyFrame {
    #[live(Ease::Linear)]
    pub ease: Ease,
    
    #[live(1.0)]
    pub time: f64,
    
    #[live(LiveValue::None)]
    pub value: LiveValue,
}
impl LiveRegister for KeyFrame{}

#[derive(Copy, Clone, Debug, PartialEq, Live, LiveHook)]
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
//pub type StatePair = [LiveId; 2];

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


#[derive(Clone, Copy, Debug, PartialEq, Live, LiveHook)]
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

#[derive(Default)]
pub struct Animator {
    pub ignore_missing: bool,
    pub live_ptr: LiveRef,
    pub state: Option<Vec<LiveNode >>,
    pub next_frame: NextFrame,
}

#[derive(Copy, Clone)]
pub enum AnimatorAction {
    Animating {redraw: bool},
    None
}

impl LiveHook for Animator {}
impl LiveNew for Animator {
    fn new(_cx: &mut Cx) -> Self {Self::default()}
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            live_ignore: true,
            fields: Vec::new(),
            type_name: id_lut!(States)
        }
    }
}
impl LiveApply for Animator {
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, start_index: usize, nodes: &[LiveNode]) -> usize {

        if let Some(file_id) = apply.from.file_id() {
            self.live_ptr = Some(cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, start_index));
        }
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, live_id!(States));
            return nodes.skip_node(start_index);
        }
        // this just checks it only has instance subprops
        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            if nodes[index].id == live_id!(ignore_missing){
                self.ignore_missing = nodes[index].value.as_bool().unwrap_or(false);
            }
            else if !nodes[index].origin.has_prop_type(LivePropType::Instance) {
                cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
            }
            index = nodes.skip_node(index);
        }
        return index;
    }
}

impl AnimatorAction {
    pub fn must_redraw(&self) -> bool {
        match self {
            Self::Animating {redraw} => *redraw,
            _ => false
        }
    }
    pub fn is_animating(&self) -> bool {
        match self {
            Self::Animating {..} => true,
            _ => false
        }
    }
}
impl Animator {
    
    pub fn swap_out_state(&mut self) -> Option<Vec<LiveNode>> {
        self.state.take()
    }
    
    pub fn swap_in_state(&mut self, state: Vec<LiveNode>) {
        self.state = Some(state);
    }
    
    pub fn need_init(&self) -> bool {
        self.state.is_none()
    }
    
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event) -> AnimatorAction {
        
        if let Event::NextFrame(nf) = event {
            if !nf.set.contains(&self.next_frame) {
                return AnimatorAction::None
            }
            if self.state.is_none() {
                return AnimatorAction::None
            }
            let state_nodes = self.state.as_mut().unwrap();
            
            let mut state_index = state_nodes.child_by_name(0, live_id!(state).as_field()).unwrap();
            let mut stack_depth = 0;
            let mut ended = true;
            let mut redraw = false;
            while state_index < state_nodes.len() {
                let state_node = &state_nodes[state_index];
                if state_node.is_array() {
                    // ok so. lets compute our value and store it in the last slot
                    let (play_ended, play_redraw) = Self::update_timeline_value(cx, state_index, state_nodes, nf.time);
                    if !play_ended {
                        ended = false;
                    }
                    if play_redraw {
                        redraw = true;
                    }
                    state_index = state_nodes.skip_node(state_index);
                }
                else { // we have to create a timeline ourselves
                    if state_node.value.is_open() {
                        stack_depth += 1;
                        state_index += 1;
                    }
                    else if state_node.value.is_close() {
                        stack_depth -= 1;
                        state_index += 1;
                        if stack_depth == 0 {
                            break;
                        }
                    }
                    else {
                        state_index = state_nodes.skip_node(state_index);
                    }
                }
            }
            //println!("{}", state_nodes.to_string(0,100));
            if !ended {
                self.next_frame = cx.new_next_frame();
            }
            
            return AnimatorAction::Animating {redraw}
        }
        AnimatorAction::None
    }
    
    // this find the last keyframe value from an array node
    pub fn update_timeline_value(cx: &mut Cx, index: usize, nodes: &mut [LiveNode], ext_time: f64) -> (bool, bool) {
        // OK so. we have an array with keyframes
        if nodes[index].is_array() {
            let mut node_iter = nodes.first_child(index);
            
            // compute the animation time from the id
            let (ended, time, redraw, track_id) = if let Some(id_index) = node_iter {
                if let LiveValue::Id(track_id) = nodes[id_index].value {
                    // ok so now we have to find our id in tracks
                    let track_index = nodes.child_by_path(0, &[live_id!(tracks).as_field(), track_id.as_field()]).unwrap();
                    
                    let time_index = if let Some(time_index) = nodes.child_by_name(track_index, live_id!(time).as_field()) {time_index}
                    else {
                        return (true, false);
                    };
                    
                    let start_time = match &nodes[time_index].value {
                        LiveValue::Id(v) => {
                            assert!(*v == live_id!(void));
                            nodes[time_index].value = LiveValue::Float64(ext_time);
                            ext_time
                        }
                        LiveValue::Float64(time) => {
                            *time
                        }
                        _ => panic!()
                    };
                    
                    let play = if let Some(play_index) = nodes.child_by_name(track_index, live_id!(play).as_field()) {
                        Play::new_apply(cx, &mut ApplyFrom::New.into(), play_index, nodes)
                    }
                    else {
                        Play::new(cx)
                    };
                    node_iter = nodes.next_child(id_index);
                    
                    let (ended, time) = play.get_ended_time(ext_time - start_time);
                    
                    if ended { // mark ended step 1
                        if let Some(index) = nodes.child_by_name(track_index, live_id!(ended).as_field()) {
                            nodes[index].value = LiveValue::Int64(cx.event_id as i64);
                        }
                    }
                    
                    let redraw = if let Some(index) = nodes.child_by_name(track_index, live_id!(redraw).as_field()) {
                        if let LiveValue::Bool(redraw) = &nodes[index].value {
                            *redraw
                        }else {false}
                    }else {false};
                    
                    (ended, time, redraw, track_id)
                }
                else {panic!()}
            }
            else {panic!()};
            
            let default_ease = if let Some(ease_index) = nodes.child_by_path(0, &[live_id!(tracks).as_field(), track_id.as_field(), live_id!(ease).as_field()]) {
                Ease::new_apply(cx, &mut ApplyFrom::New.into(), ease_index, nodes)
            }
            else {
                Ease::Linear
            };
            
            let mut prev_kf: Option<KeyFrame> = None;
            let mut last_child_index = node_iter.unwrap();
            while let Some(node_index) = node_iter {
                if nodes[node_index + 1].is_close() { // at last slot
                    last_child_index = node_index;
                    break;
                }
                let next_kf = if nodes[node_index].is_value_type() { // we hit a bare value node
                    if prev_kf.is_some() {
                        KeyFrame {
                            ease: default_ease.clone(),
                            time: 1.0,
                            value: nodes[node_index].value.clone()
                        }
                    }
                    else {
                        KeyFrame {
                            ease: default_ease.clone(),
                            time: 0.0,
                            value: nodes[node_index].value.clone()
                        }
                    }
                }
                else { // try to deserialize a keyframe
                    let mut kf = KeyFrame::new_apply(cx, &mut ApplyFrom::New.into(), node_index, nodes);
                    if nodes.child_by_name(node_index, live_id!(ease).as_field()).is_none() {
                        kf.ease = default_ease.clone();
                    }
                    kf
                };
                
                if let Some(prev_kf) = prev_kf {
                    if time >= prev_kf.time && time <= next_kf.time {
                        let normalised_time = (time - prev_kf.time) / (next_kf.time - prev_kf.time);
                        let mix = next_kf.ease.map(normalised_time);
                        // find last one
                        while let Some(node_index) = node_iter {
                            last_child_index = node_index;
                            node_iter = nodes.next_child(node_index);
                        }
                        
                        let a = &prev_kf.value;
                        let b = &next_kf.value;
                        
                        let new_val = match a {
                            LiveValue::Int64(va) => match b {
                                LiveValue::Int64(vb) => {
                                    LiveValue::Float64(((vb - va) as f64) * mix + *va as f64)
                                }
                                LiveValue::Float64(vb) => {
                                    LiveValue::Float64(((vb - *va as f64) as f64) * mix + *va as f64)
                                }
                                _ => LiveValue::None
                            }
                            LiveValue::Float64(va) => match b {
                                LiveValue::Int64(vb) => {
                                    LiveValue::Float64(((*vb as f64 - va) as f64) * mix + *va as f64)
                                }
                                LiveValue::Float64(vb) => {
                                    LiveValue::Float64(((vb - va)) * mix + *va)
                                }
                                _ => LiveValue::None
                            }
                            LiveValue::Color(va) => match b {
                                LiveValue::Color(vb) => {
                                    LiveValue::Color(Vec4::from_lerp(Vec4::from_u32(*va), Vec4::from_u32(*vb), mix as f32).to_u32())
                                }
                                _ => LiveValue::None
                            }
                            LiveValue::Vec2(va) => match b {
                                LiveValue::Vec2(vb) => {
                                    LiveValue::Vec2(Vec2::from_lerp(*va, *vb, mix as f32))
                                }
                                _ => LiveValue::None
                            }
                            LiveValue::Vec3(va) => match b {
                                LiveValue::Vec3(vb) => {
                                    LiveValue::Vec3(Vec3::from_lerp(*va, *vb, mix as f32))
                                }
                                _ => LiveValue::None
                            }
                            LiveValue::Id(_) => match b {
                                LiveValue::Id(vb) => {
                                    LiveValue::Id(*vb)
                                }
                                _ => LiveValue::None
                            }
                            _ => LiveValue::None
                        };
                        if let LiveValue::None = &new_val {
                            cx.apply_key_frame_cannot_be_interpolated(live_error_origin!(), index, nodes, a, b);
                            return (ended, redraw)
                        }
                        nodes[last_child_index].value = new_val;
                        
                        return (ended, redraw)
                    }
                }
                prev_kf = Some(next_kf);
                last_child_index = node_index;
                node_iter = nodes.next_child(node_index);
            }
            if let Some(prev_kf) = prev_kf {
                nodes[last_child_index].value = prev_kf.value
            }
            return (ended, redraw)
        }
        (false, false)
    }
    
    
    pub fn last_keyframe_value_from_array(index: usize, nodes: &[LiveNode]) -> Option<usize> {
        if let Some(index) = nodes.last_child(index) {
            if nodes[index].value.is_object() {
                return nodes.child_by_name(index, live_id!(value).as_field());
            }
            else {
                return Some(index)
            }
        }
        return None
    }
    
    pub fn first_keyframe_time_from_array(reader: &LiveNodeReader) -> f64 {
        if let Some(reader) = reader.first_child() {
            if reader.is_object() {
                if let Some(reader) = reader.child_by_name(live_id!(time).as_field()) {
                    return match &reader.value {
                        LiveValue::Float64(v) => *v,
                        LiveValue::Int64(v) => *v as f64,
                        _ => 1.0
                    }
                }
            }
        }
        return 1.0
    }
    
    pub fn is_track_animating(&self, cx: &mut Cx, track_id: &[LiveId;1]) -> bool {
        if let Some(state) = self.state.as_ref() {
            if let Some(LiveValue::Int64(ended)) = state.child_value_by_path(0, &[live_id!(tracks).as_field(), track_id[0].as_field(), live_id!(ended).as_field()]) {
                if *ended == 0 || *ended == cx.event_id as i64 {
                    return true
                }
            }
        }
        false
    }
    
    pub fn animator_in_state(&self, cx: &Cx, check_state_pair: &[LiveId; 2]) -> bool {
        // if we aren't initialized, look if our state id is a default
        if self.need_init() {
            if let Some(live_ptr) = self.live_ptr {
                let live_registry_rc = cx.live_registry.clone();
                let live_registry = live_registry_rc.borrow();
                if live_registry.generation_valid(live_ptr) {
                    let (nodes, index) = live_registry.ptr_to_nodes_index(live_ptr);
                    if let Some(LiveValue::Id(default_id)) = nodes.child_value_by_path(index, &[check_state_pair[0].as_instance(), live_id!(default).as_field()]) {
                        return *default_id == check_state_pair[1];
                    }
                }
            }
        }
        else {
            let state = self.state.as_ref().unwrap();
            if let Some(LiveValue::Id(id)) = &state.child_value_by_path(0, &[live_id!(tracks).as_field(), check_state_pair[0].as_field(), live_id!(state_id).as_field()]) {
                return *id == check_state_pair[1];
            }
        }
        false
    }
    
    pub fn cut_to_live(&mut self, cx: &mut Cx, state_id: &[LiveId; 2]) {
        if let Some(live_ptr) = self.live_ptr {
            let live_registry_rc = cx.live_registry.clone();
            let live_registry = live_registry_rc.borrow();
            if live_registry.generation_valid(live_ptr) {
                // ok now we have to find
                let (nodes, index) = live_registry.ptr_to_nodes_index(live_ptr);
                
                self.init_as_needed(cx, index, nodes);
                
                if let Some(index) = nodes.child_by_path(index, &[state_id[0].as_instance(), state_id[1].as_instance()]) {
                    self.cut_to(cx, state_id, index, nodes);
                }
                else {
                    error!("cut_to_live {}.{} not found", state_id[0], state_id[1]);
                }
            }
            else {
                error!("cut_to_live generation invalid")
            }
        }
    }
    
    // hard cut / initialisate the state to a certain state
    pub fn cut_to(&mut self, cx: &mut Cx, state_pair: &[LiveId; 2], index: usize, nodes: &[LiveNode]) {
        
        if let Some(index) = nodes.child_by_name(index, live_id!(cursor).as_field()) {
            let cursor = MouseCursor::new_apply(cx, &mut ApplyFrom::New.into(), index, nodes);
            cx.set_cursor(cursor);
        }
        // if we dont have a state object, lets create a template
        let mut state = self.swap_out_state().unwrap_or(Vec::new());
        // ok lets fetch the track
        let track = state_pair[0];
        
        if state.len() == 0 {
            state.push_live(live!{
                tracks: {},
                state: {}
            });
        }
        
        state.replace_or_insert_last_node_by_path(0, &[live_id!(tracks).as_field(), track.as_field()], live_object!{
            [track]: {state_id: (state_pair[1]), ended: 1}
        });
        
        let mut path = Vec::new();
        path.push(live_id!(state).as_field());
        
        let mut reader = if let Some(reader) = LiveNodeReader::new(index, nodes).child_by_name(live_id!(apply).as_field()) {
            reader
        }
        else {
            cx.apply_animate_missing_apply_block(live_error_origin!(), index, nodes);
            self.swap_in_state(state);
            return
        };
        
        reader.walk();
        while !reader.is_eot() {
            if reader.is_array() {
                path.push(reader.prop());
                if let Some(last_value) = Self::last_keyframe_value_from_array(reader.index(), reader.nodes()) {
                    state.replace_or_insert_first_node_by_path(0, &path, live_array!{
                        [(track), (reader.nodes()[last_value].value.clone())]
                    });
                }
                path.pop();
                reader.skip();
            }
            else {
                if reader.is_expr() {
                    path.push(reader.prop());
                    state.replace_or_insert_last_node_by_path(0, &path, reader.node_slice());
                    path.pop();
                    reader.skip();
                    continue;
                }
                else if reader.is_open() {
                    path.push(reader.prop());
                    if reader.is_enum() {
                        state.replace_or_insert_last_node_by_path(0, &path, reader.node_slice());
                    }
                }
                else if reader.is_close() {
                    path.pop();
                }
                else {
                    path.push(reader.prop());
                    state.replace_or_insert_first_node_by_path(0, &path, live_array!{
                        [(track), (reader.value.clone())]
                    });
                    path.pop();
                }
                reader.walk();
            }
        }
        self.swap_in_state(state);
        // animate once
        //self.next_frame = cx.new_next_frame();
    }
    
    pub fn init_as_needed(&mut self, cx: &mut Cx, index: usize, nodes: &[LiveNode]) {
        if self.need_init() {
            let mut index = index + 1;
            while !nodes[index].is_close() {
                let track_id = nodes[index].id;
                if let Some(LiveValue::Id(state_id)) = nodes.child_value_by_path(index, &[live_id!(default).as_field()]) {
                    if let Some(index) = nodes.child_by_name(index, state_id.as_instance()) {
                        self.cut_to(cx, &[track_id, *state_id], index, nodes);
                    }
                }
                index = nodes.skip_node(index);
            }
        }
    }
    
    pub fn animate_to_live(&mut self, cx: &mut Cx, state_pair: &[LiveId; 2]) {
        if let Some(live_ptr) = self.live_ptr {
            let live_registry_rc = cx.live_registry.clone();
            let live_registry = live_registry_rc.borrow();
            if live_registry.generation_valid(live_ptr) {
                let (nodes, index) = live_registry.ptr_to_nodes_index(live_ptr);
                
                self.init_as_needed(cx, index, nodes);
                
                if let Some(index) = nodes.child_by_path(index, &[state_pair[0].as_instance(), state_pair[1].as_instance()]) {
                    self.animate_to(cx, state_pair, index, nodes)
                }
                else if !self.ignore_missing{
                    error!("animate_to_live {}.{} not found", state_pair[0], state_pair[1])
                }
            }
            else {
                error!("animate_to_live generation invalid");
            }
        }
    }
    
    pub fn animate_to(&mut self, cx: &mut Cx, state_pair: &[LiveId; 2], index: usize, nodes: &[LiveNode]) {
        
        if let Some(index) = nodes.child_by_name(index, live_id!(cursor).as_field()) {
            let cursor = MouseCursor::new_apply(cx, &mut ApplyFrom::New.into(), index, nodes);
            cx.set_cursor(cursor);
        }
        
        let mut state = self.swap_out_state().unwrap_or(Vec::new());
        if state.len() == 0 { // call cut first
            //self.cut_to(cx, state_id, index, nodes);
            //return
            panic!();
        }
        
        let track = state_pair[0];
        
        // ok we have to look up into state/tracks for our state_id what state we are in right now
        let from_id = if let Some(LiveValue::Id(id)) = state.child_value_by_path(0, &[live_id!(tracks).as_field(), track.as_field(), live_id!(state_id).as_field()]) {
            *id
        }
        else {
            cx.apply_error_animate_to_unknown_track(live_error_origin!(), index, nodes, track, state_pair[1]);
            return
        };
        
        let mut path = Vec::new();
        
        state.replace_or_insert_last_node_by_path(0, &[live_id!(tracks).as_field(), track.as_field()], live_object!{
            [track]: {state_id: (state_pair[1]), ended: 0, time: void},
        });
        
        // copy in from track
        if let Some(index) = nodes.child_by_name(index, live_id!(from).as_field()) {
            if let Some(index) = nodes.child_by_name(index, from_id.as_field()) {
                state.replace_or_insert_last_node_by_path(
                    0,
                    &[live_id!(tracks).as_field(), track.as_field(), live_id!(play).as_field()],
                    nodes.node_slice(index)
                );
            }
            else if let Some(index) = nodes.child_by_name(index, live_id!(all).as_field()) {
                state.replace_or_insert_last_node_by_path(
                    0,
                    &[live_id!(tracks).as_field(), track.as_field(), live_id!(play).as_field()],
                    nodes.node_slice(index)
                );
            }
        }
        
        // copy ease default if we have one
        if let Some(index) = nodes.child_by_name(index, live_id!(ease).as_field()) {
            state.replace_or_insert_last_node_by_path(0, &[live_id!(tracks).as_field(), track.as_field(), live_id!(ease).as_field()], nodes.node_slice(index));
        }
        
        if let Some(index) = nodes.child_by_name(index, live_id!(redraw).as_field()) {
            state.replace_or_insert_last_node_by_path(0, &[live_id!(tracks).as_field(), track.as_field(), live_id!(redraw).as_field()], nodes.node_slice(index));
        }
        
        path.push(live_id!(state).as_field());
        
        let mut reader = if let Some(reader) = LiveNodeReader::new(index, nodes).child_by_name(live_id!(apply).as_field()) {
            reader
        }
        else {
            cx.apply_animate_missing_apply_block(live_error_origin!(), index, nodes);
            return
        };
        reader.walk();
        while !reader.is_eot() {
            
            if reader.is_array() {
                path.push(reader.prop());
                let (first_index, last_index) = if let Some(state_child) = state.child_by_path(0, &path) {
                    if let Some(last_index) = state.last_child(state_child) {
                        (state_child + 1, last_index)
                    }
                    else {panic!()}
                }
                else {
                    cx.apply_error_animation_missing_state(live_error_origin!(), index, nodes, track, state_pair[1], &path);
                    path.pop();
                    reader.skip();
                    continue;
                };
                // verify we do the right track
                if let LiveValue::Id(check_id) = &state[first_index].value {
                    if *check_id != track {
                        cx.apply_error_wrong_animation_track_used(live_error_origin!(), index, nodes, path.last().unwrap().0, *check_id, track);
                        path.pop();
                        reader.skip();
                        continue;
                    }
                }
                else {
                    panic!()
                }
                let first_time = Self::first_keyframe_time_from_array(&reader);
                
                let mut timeline = Vec::new();
                timeline.open_array(LiveId(0));
                timeline.push_id(LiveId(0), track);
                if first_time != 0.0 { // insert first key from the last value
                    timeline.push_live(state.node_slice(last_index));
                }
                timeline.push_live(reader.children_slice());
                timeline.push_live(state.node_slice(last_index));
                timeline.close();
                state.replace_or_insert_last_node_by_path(0, &path, &timeline);
                
                path.pop();
                reader.skip();
            }
            else {
                if reader.is_expr() {
                    path.push(reader.prop());
                    state.replace_or_insert_last_node_by_path(0, &path, reader.node_slice());
                    path.pop();
                    reader.skip();
                    continue;
                }
                if reader.is_open() {
                    path.push(reader.prop());
                }
                else if reader.is_close() {
                    path.pop();
                }
                else {
                    path.push(reader.prop());
                    
                    let (first_index, last_index) = if let Some(state_child) = state.child_by_path(0, &path) {
                        if let Some(last_index) = state.last_child(state_child) {
                            (state_child + 1, last_index)
                        }
                        else {panic!()}
                    }
                    else {
                        // ok so if its missing state, shall we just do an array with 1 value?
                        
                        cx.apply_error_animation_missing_state(live_error_origin!(), index, nodes, track, state_pair[1], &path);
                        path.pop();
                        reader.skip();
                        continue;
                    };
                    // verify
                    if let LiveValue::Id(check_id) = &state[first_index].value {
                        if *check_id != track {
                            cx.apply_error_wrong_animation_track_used(live_error_origin!(), index, nodes, path.last().unwrap().0, *check_id, track);
                            path.pop();
                            reader.skip();
                            continue;
                        }
                    }
                    else {
                        panic!()
                    }
                    let mut timeline = Vec::new();
                    timeline.open_array(LiveId(0));
                    timeline.push_live(live_array!{(track)});
                    timeline.push_live(state.node_slice(last_index));
                    timeline.push_live(reader.node_slice());
                    //timeline.last_mut().unwrap().id = live_id!(0); // clean up property id
                    timeline.push_live(state.node_slice(last_index));
                    timeline.close();
                    state.replace_or_insert_last_node_by_path(0, &path, &timeline);
                    path.pop();
                }
                reader.walk();
            }
        }
        
        self.swap_in_state(state);
        
        self.next_frame = cx.new_next_frame();
    }
    
}
