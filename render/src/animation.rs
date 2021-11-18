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
    
    #[live(LiveValue::None)]
    pub value: LiveValue,
}

#[derive(Default)]
pub struct Animator {
    pub state_id: Option<Id>,
    pub play: Option<Play>,
    pub live_ptr: Option<LivePtr>,
    pub state: Option<Vec<LiveNode >>,
}

// OK so.. now the annoying bit
impl Animator {
    
    pub fn has_state(&self) -> bool {
        self.state.is_some()
    }
    
    pub fn swap_out_state(&mut self) -> Vec<LiveNode> {
        self.state.take().unwrap()
    }
    
    pub fn swap_in_state(&mut self, state: Vec<LiveNode>) {
        self.state = Some(state);
    }
    
    // this find the last keyframe value from an array node
    pub fn last_keyframe_value_from_array(index: usize, nodes: &[LiveNode]) -> Option<usize> {
        if let Some(index) = nodes.last_child(index) {
            if nodes[index].value.is_bare_class() {
                if let Ok(index) = nodes.child_by_name(index, id!(value)) {
                    return Some(index)
                }
            }
            else {
                return Some(index)
            }
        }
        return None
    }
    
    pub fn first_keyframe_time_from_array(index: usize, nodes: &[LiveNode]) -> f64 {
        if let Some(index) = nodes.first_child(index) {
            if nodes[index].value.is_bare_class() {
                if let Ok(index) = nodes.child_by_name(index, id!(time)) {
                    return match nodes[index].value {
                        LiveValue::Float(v) => v,
                        LiveValue::Int(v) => v as f64,
                        _ => 1.0
                    }
                }
            }
        }
        return 1.0
    }
    
    pub fn tween_live_values(a: &LiveValue, b: &LiveValue, mix: f64) -> LiveValue {
        if a.variant_id() != b.variant_id() {
            println!("Key frame value types are incompatible!");
            return LiveValue::None
        }
        match a {
            LiveValue::Int(va) => if let LiveValue::Int(vb) = b {
                LiveValue::Int((((vb - va) as f64) * mix + *va as f64) as i64)
            } else {LiveValue::None}
            LiveValue::Float(va) => if let LiveValue::Float(vb) = b {
                LiveValue::Float((vb - va) * mix + va)
            } else {LiveValue::None}
            LiveValue::Color(va) => if let LiveValue::Color(vb) = b {
                LiveValue::Color(Vec4::from_lerp(Vec4::from_u32(*va), Vec4::from_u32(*vb), mix as f32).to_u32())
            } else {LiveValue::None}
            LiveValue::Vec2(va) => if let LiveValue::Vec2(vb) = b {
                LiveValue::Vec2(Vec2::from_lerp(*va, *vb, mix as f32))
            } else {LiveValue::None}
            LiveValue::Vec3(va) => if let LiveValue::Vec3(vb) = b {
                LiveValue::Vec3(Vec3::from_lerp(*va, *vb, mix as f32))
            } else {LiveValue::None}
            _ => LiveValue::None
        }
    }
    
    // this find the last keyframe value from an array node
    pub fn compute_timeline_value(cx: &mut Cx, index: usize, nodes: &[LiveNode], time: f64) -> LiveValue {
        // OK so. we have an array with keyframes
        if nodes[index].value.is_array() {
            let mut node_iter = nodes.first_child(index);
            let mut prev_kf: Option<KeyFrame> = None;
            while let Some(node_index) = node_iter {
                let next_kf = if nodes[node_index].value.is_value_type() { // we hit a bare value node
                    KeyFrame {
                        ease: Ease::Linear,
                        time: if prev_kf.is_some() {1.0}else {0.0},
                        value: nodes[node_index].value.clone()
                    }
                }
                else { // try to deserialize a keyframe
                    KeyFrame::new_apply(cx, ApplyFrom::New, node_index, nodes)
                };
                
                if let Some(prev_kf) = prev_kf {
                    if prev_kf.time >= time && time <= next_kf.time {
                        let nt = (time - prev_kf.time) / (next_kf.time - prev_kf.time);
                        let mix = next_kf.ease.map(nt);
                        return Self::tween_live_values(&prev_kf.value, &next_kf.value, mix)
                    }
                }
                prev_kf = Some(next_kf);
                node_iter = nodes.next_child(node_index);
            }
            if let Some(prev_kf) = prev_kf {
                return prev_kf.value
            }
        }
        else {
            return nodes[index].value.clone();
        }
        return LiveValue::None
    }
    
    // this creates a pure value strip of the state in current_state
    pub fn cut_state_to(&mut self, cx: &mut Cx, state_id: Id) {
        let live_registry = cx.live_registry.borrow();
        let (nodes, index) = live_registry.ptr_to_nodes_index(self.live_ptr.unwrap());
        
        self.state_id = Some(state_id);
        
        let state = if let Some(state) = &mut self.state {
            state.truncate(0);
            state
        }
        else {
            self.state = Some(Vec::new());
            self.state.as_mut().unwrap()
        };
        
        if let Ok(mut index) = nodes.child_by_name(index, state_id) {
            // lets iterate index
            let mut stack_depth = 0;
            while index < nodes.len() {
                let node = &nodes[index];
                if stack_depth == 1 && node.id == id!(from) { // skip this one
                    index = nodes.skip_node(index)
                }
                else if node.value.is_array() {
                    if let Some(last_value) = Self::last_keyframe_value_from_array(index, nodes) {
                        state.extend_from_slice(live_bare!{
                            [node.id]: [(nodes[last_value].value.clone())]
                        });
                    }
                    index = nodes.skip_node(index);
                }
                else {
                    if node.value.is_open() {
                        state.push(node.clone());
                        stack_depth += 1;
                    }
                    else if node.value.is_close() {
                        state.push(node.clone());
                        stack_depth -= 1;
                        if stack_depth == 0 {
                            break;
                        }
                    }
                    else { // array with single value containing this as state
                        state.extend_from_slice(live_bare!{
                            [node.id]: [(node.value.clone())]
                        });
                    }
                    index += 1;
                }
                
            }
        }
    }
    
    // this walks our timelines and updates the last array values with the computed time
    pub fn animate(&mut self, _cx: &mut Cx, _time: f64) {
        
    }
    
    // this outputs a set of arrays at the end of current_state containing the tracks
    pub fn create_timeline_to(&mut self, cx: &mut Cx, state_id: Id) {

        let live_registry_rc = cx.live_registry.clone();
        let live_registry = live_registry_rc.borrow();
        let (to_nodes, to_root_index) = live_registry.ptr_to_nodes_index(self.live_ptr.unwrap());
        
        let state_nodes = self.state.as_mut().unwrap();
        
        let mut state_index = 0;
        let mut to_index = to_nodes.child_by_name(to_root_index, state_id).unwrap();
        let mut stack_depth = 0;

        while state_index < state_nodes.len() {
            let state_node = &mut state_nodes[state_index];
            let to_node = &to_nodes[to_index];
            //println!("{}: {:?}", to_node.id, to_node.value);
            // ok so we co-walk the to_nodes
            if stack_depth == 1 && to_node.id == id!(from) {
                // ok now what. we need to see if we can find our 'from' id
                // if not we use 'all'
                let from_id = self.state_id.unwrap();
                if let Ok(from_index) = to_nodes.child_by_name(to_index, from_id){
                    self.play = Some(Play::new_apply(cx, ApplyFrom::New, from_index, to_nodes))
                }
                else if let Ok(from_index) = to_nodes.child_by_name(to_index, id!(all)){
                    self.play = Some(Play::new_apply(cx, ApplyFrom::New, from_index, to_nodes))
                }
                else{
                    
                }
                to_index = to_nodes.skip_node(to_index);
            }
            else {
                // we are an array. so we have to check if our first value has a time 0
                if to_node.value.is_array() {
                    if !state_node.value.is_array() {panic!()};
                    if state_node.id != to_node.id { // we have a desync we could someday fix that
                        println!("State node order desync: <state.id> {} <to_node.id> {}", state_node.id, to_node.id);
                        return
                    }
                    
                    let first_time = Self::first_keyframe_time_from_array(to_index, to_nodes);
                    
                    if first_time != 0.0 { // insert first key from the last value
                        
                        let (state_first, state_last) = state_nodes.child_range(state_index);
                        let (to_first, to_last) = to_nodes.child_range(to_index);
                        
                        // alright this thing is legit. So now
                        let current_value = state_nodes[state_last - 1].value.clone();
                        if !current_value.is_value_type(){
                            panic!()
                        }
                        // splicing nodes
                        state_nodes.splice(
                            state_first..state_last - 1,
                            to_nodes[to_first - 1..to_last].iter().cloned()
                        );
                        // lets look at our nodes
                        // overwrite the first node with our computed value
                        state_nodes[state_first].id = Id(0);
                        state_nodes[state_first].value = current_value;
                    }
                    else { //splice out all children except the last and replace with our array
                        let (state_first, state_last) = state_nodes.child_range(state_index);
                        let (to_first, to_last) = to_nodes.child_range(to_index);
                        // then we override that one
                        state_nodes.splice(
                            state_first..state_last-1,
                            to_nodes[to_first..to_last].iter().cloned()
                        );
                    }
                    to_index = to_nodes.skip_node(to_index);
                    state_index = state_nodes.skip_node(state_index);
                }
                else { // we have to create a timeline ourselves
                    if to_node.value.is_open() {
                        if stack_depth == 0 { // lets copy over the state id
                            state_node.id = to_node.id;
                        }
                        if !state_node.value.is_open() { // we have a desync we could someday fix that
                            println!("State node order desync: state_node {} is not open, to_node {} is", state_node.id, to_node.id);
                            return
                        }
                        stack_depth += 1;
                        state_index += 1;
                        to_index += 1;
                    }
                    else if to_node.value.is_close() {
                        if !state_node.value.is_close() { // we have a desync we could someday fix that
                            println!("State node order desync: state_node {} is not close, to_node is {}", state_node.id, to_node.id);
                            return
                        }
                        stack_depth -= 1;
                        state_index += 1;
                        to_index += 1;
                        if stack_depth == 0 {
                            break;
                        }
                    }
                    else { // create a 2 array item tween in timeline + last value
                        if !state_node.value.is_array() {panic!()};
                        if state_node.id != to_node.id { // we have a desync we could someday fix that
                            println!("State node order desync: <state.id> {} <to_node.id> {}", state_node.id, to_node.id);
                            return
                        }
                        let (state_first, state_last) = state_nodes.child_range(state_index);
                        let current_value = state_nodes[state_last - 1].value.clone();
                        if !current_value.is_value_type(){
                            panic!()
                        }
                        state_nodes.splice(
                            state_first..state_last - 1,
                            live_array!{
                                (current_value),
                                (to_node.value.clone())
                            }.iter().cloned()
                        );
                        to_index = to_nodes.skip_node(to_index);
                        state_index = state_nodes.skip_node(state_index);
                    }
                }
            }
        }
    }
    
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
