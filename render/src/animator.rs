use crate::cx::*;

#[derive(Clone, Default, Ord, PartialOrd, PartialEq, Eq, Copy)]
pub struct AnimatorId(u64);

impl AnimatorId{
    fn is_empty(&self)->bool{self.0 == 0}
}

#[derive(Clone)]
pub struct AnimInfo {
    pub start_time: f64,
    pub total_time: f64
}

#[derive(Clone)]
pub enum AnimLastValue {
    Float(f32), 
    Vec2(Vec2), 
    Vec3(Vec3),
    Vec4(Vec4),
}

#[derive(Default, Clone)]
pub struct Animator {
    current: Option<Anim>,
    next: Option<Anim>,
    pub animator_id: AnimatorId,
    pub live_update_id: u64,
    pub last_values: Vec<(LiveItemId, AnimLastValue)>,
}

impl Cx{
    pub fn new_animator_id(&mut self)->AnimatorId{
        let res = AnimatorId(self.animator_id);
        self.animator_id += 1;
        res
    }
}

impl Animator {
    
    pub fn need_init(&mut self, cx: &mut Cx)->bool{
        self.live_update_id != cx.live_update_id
    }
    
    pub fn init(&mut self, cx: &mut Cx, def_anim:Anim){
        self.live_update_id = cx.live_update_id;
        // lets stop all animations if we had any
        if self.animator_id.is_empty() {
            self.animator_id = cx.new_animator_id();
        }
        else if let Some(anim_area) = cx.playing_animator_ids.get_mut(&self.animator_id) {
            anim_area.total_time = 0.;
        }
        self.set_anim_as_last_values(&def_anim);
    }
    
    pub fn set_anim_as_last_values(&mut self, anim: &Anim) {
        for track in &anim.tracks {
            // we dont have a last float, find it in the tracks
            let bind_id = track.bind_id();
            match track {
                Track::Vec4{keys,..} => {
                    let val = if keys.len()>0 {keys.last().unwrap().1}else {Vec4::default()};
                    if let Some((_name, value)) = self.last_values.iter_mut().find( | (name, _) | *name == bind_id) {
                        *value = AnimLastValue::Vec4(val);
                    }
                    else {
                        self.last_values.push((bind_id, AnimLastValue::Vec4(val)));
                    }
                },
                Track::Vec3{keys,..} => {
                    let val = if keys.len()>0 {keys.last().unwrap().1}else {Vec3::default()};
                    if let Some((_name, value)) = self.last_values.iter_mut().find( | (name, _) | *name == bind_id) {
                        *value = AnimLastValue::Vec3(val);
                    }
                    else {
                        self.last_values.push((bind_id, AnimLastValue::Vec3(val)));
                    }
                },
                Track::Vec2{keys,..} => {
                    let val = if keys.len()>0 {keys.last().unwrap().1}else {Vec2::default()};
                    if let Some((_name, value)) = self.last_values.iter_mut().find( | (name, _) | *name == bind_id) {
                        *value = AnimLastValue::Vec2(val);
                    }
                    else {
                        self.last_values.push((bind_id, AnimLastValue::Vec2(val)));
                    }
                },
                Track::Float{keys,..} => {
                    let val = if keys.len()>0 {keys.last().unwrap().1}else {0.};
                    if let Some((_name, value)) = self.last_values.iter_mut().find( | (name, _) | *name == bind_id) {
                        *value = AnimLastValue::Float(val); 
                    }
                    else {
                        self.last_values.push((bind_id, AnimLastValue::Float(val)));
                    }
                },
            }
        }
    }
    
    pub fn end(&mut self) {
        if let Some(current) = self.current.take() {
            self.set_anim_as_last_values(&current);
        }
    }
    
    pub fn end_and_set(&mut self, anim: Anim) {
        self.current = None;
        self.set_anim_as_last_values(&anim);
    }
    
    pub fn term_anim_playing(&mut self) -> bool {
        if let Some(current) = &self.current {
            return current.play.term();
        }
        return false
    }
    
    pub fn play_anim(&mut self, cx: &mut Cx, anim: Anim) {
        self.live_update_id = cx.live_update_id;
        // if our area is invalid, we should just set our default value
        if let Some(current) = &self.current {
            if current.play.term() { // can't override a term anim
                return
            }
        }

        if self.animator_id.is_empty() {
            self.animator_id = cx.new_animator_id();
        }

        // alright first we find area, it already exists
        if let Some(anim_info) = cx.playing_animator_ids.get_mut(&self.animator_id){

            if anim.play.cut() || self.current.is_none() {
                self.current = Some(anim);
                anim_info.start_time = std::f64::NAN;
                self.next = None;
                anim_info.total_time = self.current.as_ref().unwrap().play.total_time();
            }
            else { // queue it
                self.next = Some(anim);
                // lets ask an animation anim how long it is
                anim_info.total_time = self.current.as_ref().unwrap().play.total_time() + self.next.as_ref().unwrap().play.total_time()
            }
        }
        else{
            self.current = Some(anim);
            self.next = None;
            cx.playing_animator_ids.insert(self.animator_id, AnimInfo {
                start_time: std::f64::NAN,
                total_time: self.current.as_ref().unwrap().play.total_time()
            });
        }
    }
    
    pub fn handle_end(&mut self, cx: &Cx, time: f64)->bool{
        if let Some(anim_info) = cx.playing_animator_ids.get(&self.animator_id){
            if anim_info.start_time.is_nan() || time - anim_info.start_time < anim_info.total_time{
                return false;
            }
        }
        if let Some(current) = self.current.take() {
            self.set_anim_as_last_values(&current);
        }
        true
    }
    
    pub fn update_anim_track(&mut self, cx: &mut Cx, time: f64) -> Option<f64> {

        // alright first we find area in running animations

        // fetch current anim
        if self.current.is_none() { // remove anim
            cx.playing_animator_ids.remove(&self.animator_id);
            return None
        }
        
        if let Some(anim_info) = cx.playing_animator_ids.get_mut(&self.animator_id){
            if anim_info.start_time.is_nan(){
                anim_info.start_time = time;
            }
            
            let current_total_time = self.current.as_ref().unwrap().play.total_time();
        
            // process queueing
            if time - anim_info.start_time >= current_total_time && !self.next.is_none() {
                self.current = self.next.clone();
                self.next = None;
                // update animation slot
                anim_info.start_time += current_total_time;
                anim_info.total_time -= current_total_time;

                Some(self.current.as_ref().unwrap().play.compute_time(time - anim_info.start_time))
            }
            else {
                Some(self.current.as_ref().unwrap().play.compute_time(time - anim_info.start_time))
            }
            
        }
        else{
            return None
        }
    }
    
    pub fn find_track_index(&mut self, bind_id: LiveItemId) -> Option<usize> {
        // find our track
        for (track_index, track) in &mut self.current.as_ref().unwrap().tracks.iter().enumerate() {
            if track.bind_id() == bind_id {
                return Some(track_index);
            }
        }
        None
    }
    
    pub fn calc_float(&mut self, cx: &mut Cx, bind_id: LiveItemId, time: f64) -> Option<f32> {
        let last = self.last_float(bind_id);
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(bind_id) {
                if let Track::Float{keys, cut_init, ease, ..} = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    let ret = Track::compute_track_float(time, keys, cut_init, last.unwrap_or(0.0), ease);
                    self.set_last_float(bind_id, ret);
                    return Some(ret)
                }
            }
        }
        None
    }
    
    pub fn last_float(&self, bind_id: LiveItemId) -> Option<f32> {
        if let Some((_, value)) = self.last_values.iter().find( | v | v.0 == bind_id) {
            if let AnimLastValue::Float(value) = value {
                return Some(*value)
            }
        }
        None
    }
    
    pub fn set_last_float(&mut self, bind_id: LiveItemId, value: f32) {
        if let Some((_, last)) = self.last_values.iter_mut().find( | v | v.0 == bind_id) {
            *last = AnimLastValue::Float(value);
        }
        else {
            self.last_values.push((bind_id, AnimLastValue::Float(value)))
        }
    }
    
    pub fn calc_vec2(&mut self, cx: &mut Cx, bind_id: LiveItemId, time: f64) -> Option<Vec2> {
        let last = self.last_vec2(bind_id);
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(bind_id) {
                if let Track::Vec2{keys, cut_init, ease, ..} = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    let ret = Track::compute_track_vec2(time, keys, cut_init, last.unwrap_or(vec2(0.,0.)), ease);
                    self.set_last_vec2(bind_id, ret);
                    return Some(ret);
                }
            }
        }
        None
    }
    
    pub fn last_vec2(&self, live_item_id: LiveItemId) -> Option<Vec2> {
        if let Some((_, value)) = self.last_values.iter().find( | v | v.0 == live_item_id) {
            if let AnimLastValue::Vec2(value) = value {
                return Some(*value)
            }
        }
        return None
    }
    
    pub fn set_last_vec2(&mut self, live_item_id: LiveItemId, value: Vec2) {
        if let Some((_, last)) = self.last_values.iter_mut().find( | v | v.0 == live_item_id) {
            *last = AnimLastValue::Vec2(value);
        }
        else {
            self.last_values.push((live_item_id, AnimLastValue::Vec2(value)))
        }
    }
    
    pub fn calc_vec3(&mut self, cx: &mut Cx, live_item_id: LiveItemId, time: f64) -> Option<Vec3> {
        let last = self.last_vec3(live_item_id);
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(live_item_id) {
                if let Track::Vec3{keys, cut_init, ease, ..} = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    let ret = Track::compute_track_vec3(time, keys, cut_init, last.unwrap_or(Vec3::all(0.)), ease);
                    self.set_last_vec3(live_item_id, ret);
                    return Some(ret)
                }
            }
        }
        None
    }
    
    pub fn last_vec3(&self, live_item_id: LiveItemId) -> Option<Vec3> {
        if let Some((_, value)) = self.last_values.iter().find( | v | v.0 == live_item_id) {
            if let AnimLastValue::Vec3(value) = value {
                return Some(*value)
            }
        }
        None
    }
    pub fn set_last_vec3(&mut self, live_item_id: LiveItemId, value: Vec3) {
        if let Some((_, last)) = self.last_values.iter_mut().find( | v | v.0 == live_item_id) {
            *last = AnimLastValue::Vec3(value);
        }
        else {
            self.last_values.push((live_item_id, AnimLastValue::Vec3(value)))
        }
    }
    
    pub fn calc_vec4(&mut self, cx: &mut Cx, live_item_id: LiveItemId, time: f64) -> Option<Vec4> {
        let last = self.last_vec4(live_item_id);
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(live_item_id) {
                if let Track::Vec4{keys, cut_init, ease, ..} = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    let ret = Track::compute_track_vec4(time, keys, cut_init, last.unwrap_or(Vec4::all(0.0)), ease);
                    self.set_last_vec4(live_item_id, ret);
                    return Some(ret)
                }
            }
        }
        return None
    }

    pub fn last_vec4(&self, live_item_id: LiveItemId) -> Option<Vec4> {
        if let Some((_, value)) = self.last_values.iter().find( | v | v.0 == live_item_id) {
            if let AnimLastValue::Vec4(value) = value {
                return Some(*value)
            }
        }
        None
    }
    
    pub fn set_last_vec4(&mut self, live_item_id: LiveItemId, value: Vec4) {
        if let Some((_, last)) = self.last_values.iter_mut().find( | v | v.0 == live_item_id) {
            *last = AnimLastValue::Vec4(value);
        }
        else {
            self.last_values.push((live_item_id, AnimLastValue::Vec4(value)))
        }
    }
}

