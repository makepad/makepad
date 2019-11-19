use crate::cx::*;
use std::f64::consts::PI;

#[derive(Clone, Debug)]
pub struct AnimArea {
    pub area: Area,
    pub start_time: f64,
    pub total_time: f64
}

#[derive(Clone, Debug)]
pub struct Anim {
    pub mode: Play,
    pub tracks: Vec<Track>
}

#[derive(Clone)]
pub enum AnimLastValue {
    Float(f32),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    Color(Color),
}

#[derive(Default, Clone)]
pub struct Animator {
    current: Option<Anim>,
    next: Option<Anim>,
    pub area: Area,
    pub theme_update_id: usize,
    pub last_values: Vec<(InstanceType, AnimLastValue)>,
}

impl Animator {

    pub fn init<F>(&mut self, cx: &mut Cx, cb: F)
    where F: Fn(&Cx) -> Anim {
        if self.theme_update_id != cx.theme_update_id {
            self.theme_update_id = cx.theme_update_id;
            let anim = cb(cx);
            // lets stop all animations if we had any
            if let Some(anim_area) = cx.playing_anim_areas.iter_mut().find( | v | v.area == self.area) {
                anim_area.total_time = 0.;
            }
            self.set_anim_as_last_values(&anim);
        }
    }
    
    pub fn set_anim_as_last_values(&mut self, anim: &Anim) {
        for track in &anim.tracks {
            // we dont have a last float, find it in the tracks
            let ident = track.ident();
            match track {
                Track::Color(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {Color::zero()};
                    if let Some((_name, value)) = self.last_values.iter_mut().find( | (name, _) | *name == ident) {
                        *value = AnimLastValue::Color(val);
                    }
                    else {
                        self.last_values.push((ident.clone(), AnimLastValue::Color(val)));
                    }
                },
                Track::Vec4(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {Vec4::zero()};
                    if let Some((_name, value)) = self.last_values.iter_mut().find( | (name, _) | *name == ident) {
                        *value = AnimLastValue::Vec4(val);
                    }
                    else {
                        self.last_values.push((ident.clone(), AnimLastValue::Vec4(val)));
                    }
                },
                Track::Vec3(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {Vec3::zero()};
                    if let Some((_name, value)) = self.last_values.iter_mut().find( | (name, _) | *name == ident) {
                        *value = AnimLastValue::Vec3(val);
                    }
                    else {
                        self.last_values.push((ident.clone(), AnimLastValue::Vec3(val)));
                    }
                },
                Track::Vec2(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {Vec2::zero()};
                    if let Some((_name, value)) = self.last_values.iter_mut().find( | (name, _) | *name == ident) {
                        *value = AnimLastValue::Vec2(val);
                    }
                    else {
                        self.last_values.push((ident.clone(), AnimLastValue::Vec2(val)));
                    }
                },
                Track::Float(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {0.};
                    if let Some((_name, value)) = self.last_values.iter_mut().find( | (name, _) | *name == ident) {
                        *value = AnimLastValue::Float(val);
                    }
                    else {
                        self.last_values.push((ident.clone(), AnimLastValue::Float(val)));
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
            return current.mode.term();
        }
        return false
    }
    
    pub fn play_anim(&mut self, cx: &mut Cx, anim: Anim) {
        // if our area is invalid, we should just set our default value
        if let Some(current) = &self.current {
            if current.mode.term() { // can't override a term anim
                return
            }
        }
        
        if !self.area.is_valid(cx) {
            self.set_anim_as_last_values(&anim);
            self.current = Some(anim);
            return
        }
        // alright first we find area, it already exists
        if let Some(anim_area) = cx.playing_anim_areas.iter_mut().find( | v | v.area == self.area) {
            //do we cut the animation in right now?
            if anim.mode.cut() || self.current.is_none() {
                self.current = Some(anim);
                anim_area.start_time = std::f64::NAN;
                self.next = None;
                anim_area.total_time = self.current.as_ref().unwrap().mode.total_time();
            }
            else { // queue it
                self.next = Some(anim);
                // lets ask an animation anim how long it is
                anim_area.total_time = self.current.as_ref().unwrap().mode.total_time() + self.next.as_ref().unwrap().mode.total_time()
            }
        }
        else if self.area != Area::Empty { // its new
            self.current = Some(anim);
            self.next = None;
            cx.playing_anim_areas.push(AnimArea {
                area: self.area.clone(),
                start_time: std::f64::NAN,
                total_time: self.current.as_ref().unwrap().mode.total_time()
            })
        }
    }
    
    pub fn set_area(&mut self, cx: &mut Cx, area: Area) {
        if self.area != Area::Empty {
            cx.update_area_refs(self.area, area.clone());
        }
        self.area = area.clone();
    }
    
    
    pub fn update_anim_track(&mut self, cx: &mut Cx, time: f64) -> Option<f64> {
        // alright first we find area in running animations
        let anim_index_opt = cx.playing_anim_areas.iter().position( | v | v.area == self.area);
        if anim_index_opt.is_none() {
            return None
        }
        let anim_index = anim_index_opt.unwrap();
        
        // initialize start time
        if cx.playing_anim_areas[anim_index].start_time.is_nan() {
            cx.playing_anim_areas[anim_index].start_time = time;
        }
        let mut start_time = cx.playing_anim_areas[anim_index].start_time;
        
        // fetch current anim
        if self.current.is_none() { // remove anim
            cx.playing_anim_areas.remove(anim_index);
            return None
        }
        
        let current_total_time = self.current.as_ref().unwrap().mode.total_time();
        
        // process queueing
        if time - start_time >= current_total_time && !self.next.is_none() {
            self.current = self.next.clone();
            self.next = None;
            // update animation slot
            start_time += current_total_time;
            if let Some(anim) = cx.playing_anim_areas.iter_mut().find( | v | v.area == self.area) {
                anim.start_time = start_time;
                anim.total_time -= current_total_time;
            }
            Some(self.current.as_ref().unwrap().mode.compute_time(time - start_time))
        }
        else {
            Some(self.current.as_ref().unwrap().mode.compute_time(time - start_time))
        }
    }
    
    pub fn find_track_index(&mut self, ident: InstanceType) -> Option<usize> {
        // find our track
        for (track_index, track) in &mut self.current.as_ref().unwrap().tracks.iter().enumerate() {
            if track.ident() == ident {
                return Some(track_index);
            }
        }
        None
    }
    
    pub fn calc_float(&mut self, cx: &mut Cx, ident: InstanceFloat, time: f64) -> f32 {
        let last = Self::_last_float(ident, &self.last_values);
        let mut ret = last;
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(InstanceType::Float(ident)) {
                if let Track::Float(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    ret = Track::compute_track_float(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                }
            }
        }
        self.set_last_float(ident, ret);
        return ret
    }
    
    pub fn last_float(&self, _cx: &Cx, ident: InstanceFloat) -> f32 {
        Self::_last_float(ident, &self.last_values)
    }
    
    pub fn _last_float(ident: InstanceFloat, last_float: &Vec<(InstanceType, AnimLastValue)>) -> f32 {
        if let Some((_, value)) = last_float.iter().find( | v | v.0 == InstanceType::Float(ident)) {
            if let AnimLastValue::Float(value) = value {
                return *value
            }
        }
        return 0.0
    }
    
    pub fn set_last_float(&mut self, ident: InstanceFloat, value: f32) {
        Self::_set_last_float(ident, value, &mut self.last_values)
    }
    
    pub fn _set_last_float(ident: InstanceFloat, value: f32, last_values: &mut Vec<(InstanceType, AnimLastValue)>) {
        let ty_ident = InstanceType::Float(ident);
        if let Some((_, last)) = last_values.iter_mut().find( | v | v.0 == ty_ident) {
            *last = AnimLastValue::Float(value);
        }
        else {
            last_values.push((ty_ident, AnimLastValue::Float(value)))
        }
    }
    
    pub fn calc_vec2(&mut self, cx: &mut Cx, ident: InstanceVec2, time: f64) -> Vec2 {
        let last = Self::_last_vec2(ident, &self.last_values);
        let mut ret = last;
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(InstanceType::Vec2(ident)) {
                if let Track::Vec2(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    ret = Track::compute_track_vec2(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                }
            }
        }
        self.set_last_vec2(ident, ret);
        return ret
    }
    
    pub fn last_vec2(&self, _cx: &Cx, ident: InstanceVec2) -> Vec2 {
        Self::_last_vec2(ident, &self.last_values)
    }
    
    pub fn _last_vec2(ident: InstanceVec2, last_values: &Vec<(InstanceType, AnimLastValue)>) -> Vec2 {
        if let Some((_, value)) = last_values.iter().find( | v | v.0 == InstanceType::Vec2(ident)) {
            if let AnimLastValue::Vec2(value) = value {
                return *value
            }
        }
        return Vec2::zero()
    }
    
    pub fn set_last_vec2(&mut self, ident: InstanceVec2, value: Vec2) {
        Self::_set_last_vec2(ident, value, &mut self.last_values);
    }
    
    pub fn _set_last_vec2(ident: InstanceVec2, value: Vec2, last_values: &mut Vec<(InstanceType, AnimLastValue)>) {
        let ty_ident = InstanceType::Vec2(ident);
        if let Some((_, last)) = last_values.iter_mut().find( | v | v.0 == ty_ident) {
            *last = AnimLastValue::Vec2(value);
        }
        else {
            last_values.push((ty_ident, AnimLastValue::Vec2(value)))
        }
    }
    
    pub fn calc_vec3(&mut self, cx: &mut Cx, ident: InstanceVec3, time: f64) -> Vec3 {
        let last = Self::_last_vec3(ident, &self.last_values);
        let mut ret = last;
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(InstanceType::Vec3(ident)) {
                if let Track::Vec3(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    ret = Track::compute_track_vec3(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                }
            }
        }
        self.set_last_vec3(ident, ret);
        return ret
    }
    
    pub fn last_vec3(&self, _cx: &Cx, ident: InstanceVec3) -> Vec3 {
        Self::_last_vec3(ident, &self.last_values)
    }
    
    pub fn _last_vec3(ident: InstanceVec3, last_values: &Vec<(InstanceType, AnimLastValue)>) -> Vec3 {
        if let Some((_, value)) = last_values.iter().find( | v | v.0 == InstanceType::Vec3(ident)) {
            if let AnimLastValue::Vec3(value) = value {
                return *value
            }
        }
        return Vec3::zero()
    }
    
    pub fn set_last_vec3(&mut self, ident: InstanceVec3, value: Vec3) {
        Self::_set_last_vec3(ident, value, &mut self.last_values);
    }
    
    pub fn _set_last_vec3(ident: InstanceVec3, value: Vec3, last_values: &mut Vec<(InstanceType, AnimLastValue)>) {
        let ty_ident = InstanceType::Vec3(ident);
        if let Some((_, last)) = last_values.iter_mut().find( | v | v.0 == ty_ident) {
            *last = AnimLastValue::Vec3(value);
        }
        else {
            last_values.push((ty_ident, AnimLastValue::Vec3(value)))
        }
    }
    
    pub fn calc_vec4(&mut self, cx: &mut Cx, ident: InstanceVec4, time: f64) -> Vec4 {
        let last = Self::_last_vec4(ident, &self.last_values);
        let mut ret = last;
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(InstanceType::Vec4(ident)) {
                if let Track::Vec4(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    ret = Track::compute_track_vec4(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                }
            }
        }
        self.set_last_vec4(ident, ret);
        return ret
    }
    
    pub fn last_vec4(&self, _cx: &Cx, ident: InstanceVec4) -> Vec4 {
        Self::_last_vec4(ident, &self.last_values)
    }
    
    pub fn _last_vec4(ident: InstanceVec4, last_values: &Vec<(InstanceType, AnimLastValue)>) -> Vec4 {
        if let Some((_, value)) = last_values.iter().find( | v | v.0 == InstanceType::Vec4(ident)) {
            if let AnimLastValue::Vec4(value) = value {
                return *value
            }
        }
        return Vec4::zero()
    }
    
    pub fn set_last_vec4(&mut self, ident: InstanceVec4, value: Vec4) {
        Self::_set_last_vec4(ident, value, &mut self.last_values);
    }
    
    pub fn _set_last_vec4(ident: InstanceVec4, value: Vec4, last_values: &mut Vec<(InstanceType, AnimLastValue)>) {
        let ty_ident = InstanceType::Vec4(ident);
        if let Some((_, last)) = last_values.iter_mut().find( | v | v.0 == ty_ident) {
            *last = AnimLastValue::Vec4(value);
        }
        else {
            last_values.push((ty_ident, AnimLastValue::Vec4(value)))
        }
    }
    
    pub fn calc_color(&mut self, cx: &mut Cx, ident: InstanceColor, time: f64) -> Color {
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(InstanceType::Color(ident)) {
                if let Track::Color(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    let last = Self::_last_color(ident, &self.last_values);
                    let ret = Track::compute_track_color(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                    self.set_last_color(ident, ret);
                    return ret
                }
            }
        }
        
        return Color::zero();
    }
    
    pub fn last_color(&self, _cx: &Cx, ident: InstanceColor) -> Color {
        if let Some((_, value)) = self.last_values.iter().find( | v | v.0 == InstanceType::Color(ident)) {
            if let AnimLastValue::Color(value) = value {
                return *value
            }
        }
        Color::zero()
    }
    
    pub fn _last_color(ident: InstanceColor, last_values: &Vec<(InstanceType, AnimLastValue)>) -> Color {
        if let Some((_, value)) = last_values.iter().find( | v | v.0 == InstanceType::Color(ident)) {
            if let AnimLastValue::Color(value) = value {
                return *value
            }
        }
        
        return Color::zero()
    }
    
    pub fn set_last_color(&mut self, ident: InstanceColor, value: Color) {
        Self::_set_last_color(ident, value, &mut self.last_values);
    }
    
    pub fn _set_last_color(ident: InstanceColor, value: Color, last_values: &mut Vec<(InstanceType, AnimLastValue)>) {
        let ty_ident = InstanceType::Color(ident);
        if let Some((_, last)) = last_values.iter_mut().find( | v | v.0 == ty_ident) {
            *last = AnimLastValue::Color(value)
        }
        else {
            last_values.push((ty_ident, AnimLastValue::Color(value)))
        }
    }
    
    pub fn last_area(&mut self, _cx: &mut Cx, _area: Area, _time: f64) {
        
    }
    
    pub fn calc_area(&mut self, cx: &mut Cx, area: Area, time: f64) {
        
        if let Some(time) = self.update_anim_track(cx, time) {
            
            for track_index in 0..self.current.as_ref().unwrap().tracks.len() {
                //if let Some((time, track_index)) = self.fetch_calc_track(cx, ident, time) {
                match &mut self.current.as_mut().unwrap().tracks[track_index] {
                    Track::Color(ft) => {
                        let init = Self::_last_color(ft.ident, &self.last_values);
                        let ret = Track::compute_track_color(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                        Self::_set_last_color(ft.ident, ret, &mut self.last_values);
                        area.write_color(cx, ft.ident, ret);
                    },
                    Track::Vec4(ft) => {
                        let init = Self::_last_vec4(ft.ident, &self.last_values);
                        let ret = Track::compute_track_vec4(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                        Self::_set_last_vec4(ft.ident, ret, &mut self.last_values);
                        area.write_vec4(cx, ft.ident, ret);
                    },
                    Track::Vec3(ft) => {
                        let init = Self::_last_vec3(ft.ident, &self.last_values);
                        let ret = Track::compute_track_vec3(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                        Self::_set_last_vec3(ft.ident, ret, &mut self.last_values);
                        area.write_vec3(cx, ft.ident, ret);
                    },
                    Track::Vec2(ft) => {
                        let init = Self::_last_vec2(ft.ident, &self.last_values);
                        let ret = Track::compute_track_vec2(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                        Self::_set_last_vec2(ft.ident, ret, &mut self.last_values);
                        area.write_vec2(cx, ft.ident, ret);
                    },
                    Track::Float(ft) => {
                        let init = Self::_last_float(ft.ident, &self.last_values);
                        let ret = Track::compute_track_float(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                        Self::_set_last_float(ft.ident, ret, &mut self.last_values);
                        area.write_float(cx, ft.ident, ret);
                    }
                };
            }
            //}
        }
    }
}

#[derive(Clone, Debug)]
pub enum Ease {
    Lin,
    InQuad,
    OutQuad,
    InOutQuad,
    InCubic,
    OutCubic,
    InOutCubic,
    InQuart,
    OutQuart,
    InOutQuart,
    InQuint,
    OutQuint,
    InOutQuint,
    InSine,
    OutSine,
    InOutSine,
    InExp,
    OutExp,
    InOutExp,
    InCirc,
    OutCirc,
    InOutCirc,
    InElastic,
    OutElastic,
    InOutElastic,
    InBack,
    OutBack,
    InOutBack,
    InBounce,
    OutBounce,
    InOutBounce,
    Pow {begin: f64, end: f64},
    Bezier {cp0: f64, cp1: f64, cp2: f64, cp3: f64}
    /*
    Bounce{dampen:f64},
    Elastic{duration:f64, frequency:f64, decay:f64, ease:f64}, 
    */
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
            /* forgot the parameters to these functions
            Ease::Bounce{dampen}=>{
                if time < 0.{
                    return 0.;
                }
                if time > 1. {
                    return 1.;
                }

                let it = time * (1. / (1. - dampen)) + 0.5;
                let inlog = (dampen - 1.) * it + 1.0;
                if inlog <= 0. {
                    return 1.
                }
                let k = (inlog.ln() / dampen.ln()).floor();
                let d = dampen.powf(k);
                return 1. - (d * (it - (d - 1.) / (dampen - 1.)) - (it - (d - 1.) / (dampen - 1.)).powf(2.)) * 4.
            },
            Ease::Elastic{duration, frequency, decay, ease}=>{
                if time < 0.{
                    return 0.;
                }
                if time > 1. {
                    return 1.;
                }
                let mut easein = *ease;
                let mut easeout = 1.;
                if *ease < 0. {
                    easeout = -ease;
                    easein = 1.;
                }
                
                if time < *duration{
                    return Ease::Pow{begin:easein, end:easeout}.map(time / duration)
                }
                else {
                    // we have to snap the frequency so we end at 0
                    let w = ((0.5 + (1. - duration) * frequency * 2.).floor() / ((1. - duration) * 2.)) * std::f64::consts::PI * 2.;
                    let velo = (Ease::Pow{begin:easein, end:easeout}.map(1.001) - Ease::Pow{begin:easein, end:easeout}.map(1.) ) / (0.001 * duration);
                    return 1. + velo * ((((time - duration) * w).sin() / ((time - duration) * decay).exp()) / w)
                }
            },*/
            
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


#[derive(Clone, Debug)]
pub struct FloatTrack {
    pub ident: InstanceFloat,
    pub ease: Ease,
    pub cut_init: Option<f32>,
    pub track: Vec<(f64, f32)>
}

#[derive(Clone, Debug)]
pub struct Vec2Track {
    pub ident: InstanceVec2,
    pub ease: Ease,
    pub cut_init: Option<Vec2>,
    pub track: Vec<(f64, Vec2)>
}

#[derive(Clone, Debug)]
pub struct Vec3Track {
    pub ident: InstanceVec3,
    pub ease: Ease,
    pub cut_init: Option<Vec3>,
    pub track: Vec<(f64, Vec3)>
}

#[derive(Clone, Debug)]
pub struct Vec4Track {
    pub ident: InstanceVec4,
    pub ease: Ease,
    pub cut_init: Option<Vec4>,
    pub track: Vec<(f64, Vec4)>
}

#[derive(Clone, Debug)]
pub struct ColorTrack {
    pub ident: InstanceColor,
    pub ease: Ease,
    pub cut_init: Option<Color>,
    pub track: Vec<(f64, Color)>
}

#[derive(Clone, Debug)]
pub enum Track {
    Float(FloatTrack),
    Vec2(Vec2Track),
    Vec3(Vec3Track),
    Vec4(Vec4Track),
    Color(ColorTrack),
}

impl Track {
    
    pub fn float(ident: InstanceFloat, ease: Ease, track: Vec<(f64, f32)>) -> Track {
        Track::Float(FloatTrack {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    pub fn vec2(ident: InstanceVec2, ease: Ease, track: Vec<(f64, Vec2)>) -> Track {
        Track::Vec2(Vec2Track {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    pub fn vec3(ident: InstanceVec3, ease: Ease, track: Vec<(f64, Vec3)>) -> Track {
        Track::Vec3(Vec3Track {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    pub fn vec4(ident: InstanceVec4, ease: Ease, track: Vec<(f64, Vec4)>) -> Track {
        Track::Vec4(Vec4Track {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    
    pub fn color(ident: InstanceColor, ease: Ease, track: Vec<(f64, Color)>) -> Track {
        Track::Color(ColorTrack {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    
    fn compute_track_float(time: f64, track: &Vec<(f64, f32)>, cut_init: &mut Option<f32>, init: f32, ease: &Ease) -> f32 {
        if track.is_empty() {return init}
        fn lerp(a: f32, b: f32, f: f32) -> f32 {
            return a * (1.0 - f) + b * f;
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    fn compute_track_vec2(time: f64, track: &Vec<(f64, Vec2)>, cut_init: &mut Option<Vec2>, init: Vec2, ease: &Ease) -> Vec2 {
        if track.is_empty() {return init}
        fn lerp(a: Vec2, b: Vec2, f: f32) -> Vec2 {
            let nf = 1.0 - f;
            return Vec2 {x: a.x * nf + b.x * f, y: a.y * nf + b.y * f}
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    fn compute_track_vec3(time: f64, track: &Vec<(f64, Vec3)>, cut_init: &mut Option<Vec3>, init: Vec3, ease: &Ease) -> Vec3 {
        if track.is_empty() {return init}
        fn lerp(a: Vec3, b: Vec3, f: f32) -> Vec3 {
            let nf = 1.0 - f;
            return Vec3 {x: a.x * nf + b.x * f, y: a.y * nf + b.y * f, z: a.z * nf + b.z * f}
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    fn compute_track_vec4(time: f64, track: &Vec<(f64, Vec4)>, cut_init: &mut Option<Vec4>, init: Vec4, ease: &Ease) -> Vec4 {
        if track.is_empty() {return init}
        fn lerp(a: Vec4, b: Vec4, f: f32) -> Vec4 {
            let nf = 1.0 - f;
            return Vec4 {x: a.x * nf + b.x * f, y: a.y * nf + b.y * f, z: a.z * nf + b.z * f, w: a.w * nf + b.w * f}
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    fn compute_track_color(time: f64, track: &Vec<(f64, Color)>, cut_init: &mut Option<Color>, init: Color, ease: &Ease) -> Color {
        if track.is_empty() {return init}
        fn lerp(a: Color, b: Color, f: f32) -> Color {
            let nf = 1.0 - f;
            return Color {r: a.r * nf + b.r * f, g: a.g * nf + b.g * f, b: a.b * nf + b.b * f, a: a.a * nf + b.a * f}
        }
        // find the 2 keys we want
        for i in 0..track.len() {
            if time >= track[i].0 { // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1 { // last key
                    return val1.1.clone()
                }
                let val2 = &track[i + 1];
                // lerp it
                let f = ease.map((time - val1.0) / (val2.0 - val1.0)) as f32;
                return lerp(val1.1, val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return lerp(*val1, val2.1, f)
    }
    
    pub fn ident(&self) -> InstanceType {
        match self {
            Track::Float(ft) => {
                InstanceType::Float(ft.ident)
            },
            Track::Vec2(ft) => {
                InstanceType::Vec2(ft.ident)
            }
            Track::Vec3(ft) => {
                InstanceType::Vec3(ft.ident)
            }
            Track::Vec4(ft) => {
                InstanceType::Vec4(ft.ident)
            }
            Track::Color(ft) => {
                InstanceType::Color(ft.ident)
            }
        }
    }
    
    pub fn reset_cut_init(&mut self) {
        match self {
            Track::Color(at) => {
                at.cut_init = None;
            },
            Track::Vec4(at) => {
                at.cut_init = None;
            },
            Track::Vec3(at) => {
                at.cut_init = None;
            },
            Track::Vec2(at) => {
                at.cut_init = None;
            },
            Track::Float(at) => {
                at.cut_init = None;
            }
        }
    }
    
    pub fn ease(&self) -> &Ease {
        match self {
            Track::Float(ft) => {
                &ft.ease
            },
            Track::Vec2(ft) => {
                &ft.ease
            }
            Track::Vec3(ft) => {
                &ft.ease
            }
            Track::Vec4(ft) => {
                &ft.ease
            }
            Track::Color(ft) => {
                &ft.ease
            }
        }
    }
}

impl Anim {
    pub fn new(mode: Play, tracks: Vec<Track>) -> Anim {
        Anim {
            mode: mode,
            tracks: tracks
        }
    }
    
    pub fn empty() -> Anim {
        Anim {
            mode: Play::Cut {duration: 0.},
            tracks: vec![]
        }
    }
}

#[derive(Clone, Debug)]
pub enum Play {
    Chain {duration: f64},
    Cut {duration: f64},
    Single {duration: f64, cut: bool, term: bool, end: f64},
    Loop {duration: f64, cut: bool, term: bool, repeats: f64, end: f64},
    Reverse {duration: f64, cut: bool, term: bool, repeats: f64, end: f64},
    Bounce {duration: f64, cut: bool, term: bool, repeats: f64, end: f64},
    Forever {duration: f64, cut: bool, term: bool},
    LoopForever {duration: f64, cut: bool, term: bool, end: f64},
    ReverseForever {duration: f64, cut: bool, term: bool, end: f64},
    BounceForever {duration: f64, cut: bool, term: bool, end: f64},
}

impl Play {
    pub fn duration(&self) -> f64 {
        match self {
            Play::Chain {duration, ..} => *duration,
            Play::Cut {duration, ..} => *duration,
            Play::Single {duration, ..} => *duration,
            Play::Loop {duration, ..} => *duration,
            Play::Reverse {duration, ..} => *duration,
            Play::Bounce {duration, ..} => *duration,
            Play::BounceForever {duration, ..} => *duration,
            Play::Forever {duration, ..} => *duration,
            Play::LoopForever {duration, ..} => *duration,
            Play::ReverseForever {duration, ..} => *duration,
        }
    }
    pub fn total_time(&self) -> f64 {
        match self {
            Play::Chain {duration, ..} => *duration,
            Play::Cut {duration, ..} => *duration,
            Play::Single {end, duration, ..} => end * duration,
            Play::Loop {end, duration, repeats, ..} => end * duration * repeats,
            Play::Reverse {end, duration, repeats, ..} => end * duration * repeats,
            Play::Bounce {end, duration, repeats, ..} => end * duration * repeats,
            Play::BounceForever {..} => std::f64::INFINITY,
            Play::Forever {..} => std::f64::INFINITY,
            Play::LoopForever {..} => std::f64::INFINITY,
            Play::ReverseForever {..} => std::f64::INFINITY,
        }
    }
    
    pub fn cut(&self) -> bool {
        match self {
            Play::Cut {..} => true,
            Play::Chain {..} => false,
            Play::Single {cut, ..} => *cut,
            Play::Loop {cut, ..} => *cut,
            Play::Reverse {cut, ..} => *cut,
            Play::Bounce {cut, ..} => *cut,
            Play::BounceForever {cut, ..} => *cut,
            Play::Forever {cut, ..} => *cut,
            Play::LoopForever {cut, ..} => *cut,
            Play::ReverseForever {cut, ..} => *cut,
        }
    }
    
    pub fn repeats(&self) -> f64 {
        match self {
            Play::Chain {..} => 1.0,
            Play::Cut {..} => 1.0,
            Play::Single {..} => 1.0,
            Play::Loop {repeats, ..} => *repeats,
            Play::Reverse {repeats, ..} => *repeats,
            Play::Bounce {repeats, ..} => *repeats,
            Play::BounceForever {..} => std::f64::INFINITY,
            Play::Forever {..} => std::f64::INFINITY,
            Play::LoopForever {..} => std::f64::INFINITY,
            Play::ReverseForever {..} => std::f64::INFINITY,
        }
    }
    
    pub fn term(&self) -> bool {
        match self {
            Play::Cut {..} => false,
            Play::Chain {..} => false,
            Play::Single {term, ..} => *term,
            Play::Loop {term, ..} => *term,
            Play::Reverse {term, ..} => *term,
            Play::Bounce {term, ..} => *term,
            Play::BounceForever {term, ..} => *term,
            Play::Forever {term, ..} => *term,
            Play::LoopForever {term, ..} => *term,
            Play::ReverseForever {term, ..} => *term,
        }
    }
    
    pub fn compute_time(&self, time: f64) -> f64 {
        match self {
            Play::Cut {duration, ..} => {
                time / duration
            },
            Play::Chain {duration, ..} => {
                time / duration
            },
            Play::Single {duration, ..} => {
                time / duration
            },
            Play::Loop {end, duration, ..} => {
                (time / duration) % end
            },
            Play::Reverse {end, duration, ..} => {
                end - (time / duration) % end
            },
            Play::Bounce {end, duration, ..} => {
                let mut local_time = (time / duration) % (end * 2.0);
                if local_time > *end {
                    local_time = 2.0 * end - local_time;
                };
                local_time
            },
            Play::BounceForever {end, duration, ..} => {
                let mut local_time = (time / duration) % (end * 2.0);
                if local_time > *end {
                    local_time = 2.0 * end - local_time;
                };
                local_time
            },
            Play::Forever {duration, ..} => {
                let local_time = time / duration;
                local_time
            },
            Play::LoopForever {end, duration, ..} => {
                let local_time = (time / duration) % end;
                local_time
            },
            Play::ReverseForever {end, duration, ..} => {
                let local_time = end - (time / duration) % end;
                local_time
            },
        }
    }
}