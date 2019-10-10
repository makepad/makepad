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
pub struct Animator {
    current: Option<Anim>,
    next: Option<Anim>,
    pub area: Area,
    last_values: Vec<(CxId, f32, f32, f32, f32)>,
}

impl Animator {
    
    pub fn new(default: Anim) -> Animator {
        let mut anim = Animator {
            current: None,
            next: None,
            area: Area::Empty,
            last_values: Vec::new(),
        };
        anim.set_anim_as_last_values(&default);
        return anim
    }
    
    pub fn new_no_default()->Animator{
        Animator {
            current: None,
            next: None,
            area: Area::Empty,
            last_values: Vec::new(),
        }
    }
    
    pub fn set_anim_as_last_values(&mut self, anim: &Anim) {
        for track in &anim.tracks {
            // we dont have a last float, find it in the tracks
            let ident = track.ident();
            match track {
                Track::Color(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {Color::zero()};
                    if let Some((_name, r, g, b, a)) = self.last_values.iter_mut().find( | (name, _,_,_,_) | *name == ident) {
                        *r = val.r;
                        *g = val.g;
                        *b = val.b;
                        *a = val.a;
                    }
                    else {
                        self.last_values.push((ident.clone(), val.r, val.g, val.b, val.a));
                    }
                }
                Track::Vec4(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {Vec4::zero()};
                    if let Some((_name, x, y, z, w)) = self.last_values.iter_mut().find( | (name, _,_,_,_) | *name == ident) {
                        *x = val.x;
                        *y = val.y;
                        *z = val.z;
                        *w = val.w;
                    }
                    else {
                        self.last_values.push((ident.clone(), val.x, val.y, val.z, val.w));
                    }
                },
                Track::Vec3(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {Vec3::zero()};
                    if let Some((_name, x, y, z, _)) = self.last_values.iter_mut().find( | (name, _,_,_,_) | *name == ident) {
                        *x = val.x;
                        *y = val.y;
                        *z = val.z;
                    }
                    else {
                        self.last_values.push((ident.clone(), val.x, val.y, val.z, 0.));
                    }
                },
                Track::Vec2(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {Vec2::zero()};
                    if let Some((_name, x, y, _, _)) = self.last_values.iter_mut().find( | (name, _,_,_,_) | *name == ident) {
                        *x = val.x;
                        *y = val.y;
                    }
                    else {
                        self.last_values.push((ident.clone(), val.x, val.y, 0., 0.));
                    }
                },
                Track::Float(ft) => {
                    let val = if ft.track.len()>0 {ft.track.last().unwrap().1}else {0.};
                    if let Some((_name, x, _, _, _)) = self.last_values.iter_mut().find( | (name, _,_,_,_) | *name == ident) {
                        *x = val;
                    }
                    else {
                        self.last_values.push((ident.clone(), val, 0., 0., 0.));
                    }
                }
            }
        }
    }
    
    pub fn end(&mut self){
        if let Some(current) = self.current.take(){
            self.set_anim_as_last_values(&current);
        }
    }

    pub fn end_and_set(&mut self, anim:Anim){
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
    
    pub fn update_area_refs(&mut self, cx: &mut Cx, area: Area) {
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
        if time - start_time >= current_total_time && !self.next.is_none(){
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
    
    pub fn find_track_index(&mut self, ident: CxId) -> Option<usize> {
        // find our track
        for (track_index, track) in &mut self.current.as_ref().unwrap().tracks.iter().enumerate() {
            if track.ident() == ident {
                return Some(track_index);
            }
        }
        None
    }
    
    pub fn calc_float(&mut self, cx: &mut Cx, ident_str: &str, time: f64) -> f32 {
        let ident = cx.id(ident_str);
        let last = self.last_float(ident);
        let mut ret = last;
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(ident) {
                if let Track::Float(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    ret = Track::compute_track_value::<f32>(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                }
            }
        }
        self.set_last_float(ident, ret);
        return ret
    }
    
    pub fn last_float(&self, ident: CxId) -> f32 {
        Self::_last_float(ident, &self.last_values)
    }
    
    pub fn _last_float(ident: CxId, last_float: &Vec<(CxId, f32, f32, f32, f32)>) -> f32 {
        if let Some(values) = last_float.iter().find( | v | v.0 == ident) {
            return values.1;
        }
        return 0.0
    }
    
    pub fn set_last_float(&mut self, ident: CxId, value: f32) {
        Self::_set_last_float(ident, value, &mut self.last_values)
    }
    
    pub fn _set_last_float(ident: CxId, value: f32, last_values: &mut Vec<(CxId, f32, f32, f32, f32)>) {
        if let Some(last) = last_values.iter_mut().find( | v | v.0 == ident) {
            last.1 = value;
        }
        else {
            last_values.push((ident, value, 0., 0., 0.))
        }
    }
    
    pub fn calc_vec2(&mut self, cx: &mut Cx, ident_str: &str, time: f64) -> Vec2 {
        let ident = cx.id(ident_str);
        let last = self.last_vec2(ident);
        let mut ret = last;
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(ident) {
                if let Track::Vec2(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    ret = Track::compute_track_value::<Vec2>(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                }
            }
        }
        self.set_last_vec2(ident, ret);
        return ret
    }
    
    pub fn last_vec2(&self, ident: CxId) -> Vec2 {
        Self::_last_vec2(ident, &self.last_values)
    }
    
    pub fn _last_vec2(ident: CxId, last_values: &Vec<(CxId, f32, f32, f32, f32)>) -> Vec2 {
        if let Some(value) = last_values.iter().find( | v | v.0 == ident) {
            return Vec2{x:value.1, y:value.2};
        }
        return Vec2::zero()
    }
    
    pub fn set_last_vec2(&mut self, ident: CxId, value: Vec2) {
        Self::_set_last_vec2(ident, value, &mut self.last_values);
    }
    
    pub fn _set_last_vec2(ident: CxId, value: Vec2, last_values: &mut Vec<(CxId, f32, f32, f32, f32)>) {
        if let Some(last) = last_values.iter_mut().find( | v | v.0 == ident) {
            last.1 = value.x;
            last.2 = value.x;
        }
        else {
            last_values.push((ident, value.x, value.y, 0., 0.))
        }
    }
    
    pub fn calc_vec3(&mut self, cx: &mut Cx, ident_str: &str, time: f64) -> Vec3 {
        let ident = cx.id(ident_str);
        let last = self.last_vec3(ident);
        let mut ret = last;
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(ident) {
                if let Track::Vec3(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    ret = Track::compute_track_value::<Vec3>(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                }
            }
        }
        self.set_last_vec3(ident, ret);
        return ret
    }
    
    pub fn last_vec3(&self, ident: CxId) -> Vec3 {
        Self::_last_vec3(ident, &self.last_values)
    }
    
    pub fn _last_vec3(ident: CxId, last_values: &Vec<(CxId, f32, f32, f32, f32)>) -> Vec3 {
        if let Some(value) = last_values.iter().find( | v | v.0 == ident) {
            return Vec3{x:value.1, y:value.2, z:value.3};
        }
        return Vec3::zero()
    }
    
    pub fn set_last_vec3(&mut self, ident: CxId, value: Vec3) {
        Self::_set_last_vec3(ident, value, &mut self.last_values);
    }
    
    pub fn _set_last_vec3(ident: CxId, value: Vec3, last_values: &mut Vec<(CxId, f32, f32, f32, f32)>) {
        if let Some(last) = last_values.iter_mut().find( | v | v.0 == ident) {
            last.1 = value.x;
            last.2 = value.y;
            last.3 = value.z;
        }
        else {
            last_values.push((ident, value.x, value.y, value.z, 0.))
        }
    }
    
    pub fn calc_vec4(&mut self, cx: &mut Cx, ident_str: &str, time: f64) -> Vec4 {
        let ident = cx.id(ident_str);
        let last = self.last_vec4(ident);
        let mut ret = last;
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(ident) {
                if let Track::Vec4(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    ret = Track::compute_track_value::<Vec4>(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                }
            }
        }
        self.set_last_vec4(ident, ret);
        return ret
    }
    
    pub fn last_vec4(&self, ident: CxId) -> Vec4 {
        Self::_last_vec4(ident, &self.last_values)
    }
    
    pub fn _last_vec4(ident: CxId, last_values: &Vec<(CxId, f32, f32, f32, f32)>) -> Vec4 {
        if let Some(value) = last_values.iter().find( | v | v.0 == ident) {
            return Vec4{x:value.1, y:value.2, z:value.3, w:value.4};
        }
        return Vec4::zero()
    }
    
    pub fn set_last_vec4(&mut self, ident: CxId, value: Vec4) {
        Self::_set_last_vec4(ident, value, &mut self.last_values);
    }
    
    pub fn _set_last_vec4(ident: CxId, value: Vec4, last_values: &mut Vec<(CxId, f32, f32, f32, f32)>) {
        if let Some(last) = last_values.iter_mut().find( | v | v.0 == ident) {
            last.1 = value.x;
            last.2 = value.y;
            last.3 = value.z;
            last.4 = value.w;
        }
        else {
            last_values.push((ident, value.x, value.y, value.z, value.w))
        }
    }
    
    pub fn calc_color(&mut self, cx: &mut Cx, ident_str: &str, time: f64) -> Color {
        let ident = cx.id(ident_str);
        let last = self.last_color(ident);
        let mut ret = last;
        if let Some(time) = self.update_anim_track(cx, time) {
            if let Some(track_index) = self.find_track_index(ident) {
                if let Track::Color(ft) = &mut self.current.as_mut().unwrap().tracks[track_index] {
                    ret = Track::compute_track_value::<Color>(time, &ft.track, &mut ft.cut_init, last, &ft.ease);
                }
            }
        }
        self.set_last_color(ident, ret);
        return ret
    }
        
    pub fn last_color(&self, ident: CxId) -> Color {
        Self::_last_color(ident, &self.last_values)
    }
    
    pub fn _last_color(ident: CxId, last_values: &Vec<(CxId, f32, f32, f32, f32)>) -> Color {
        if let Some(value) = last_values.iter().find( | v | v.0 == ident) {
            return Color{r:value.1, g:value.2, b:value.3, a:value.4}
        }
      
        return Color::zero()
    }
    
    pub fn set_last_color(&mut self, ident: CxId, value: Color) {
        Self::_set_last_color(ident, value, &mut self.last_values);
    }
    
    pub fn _set_last_color(ident: CxId, value: Color, last_values: &mut Vec<(CxId, f32, f32, f32, f32)>) {
        if let Some(last) = last_values.iter_mut().find( | v | v.0 == ident) {
            last.1 = value.r;
            last.2 = value.g;
            last.3 = value.b;
            last.4 = value.a;
        }
        else {
            last_values.push((ident, value.r, value.g, value.b, value.a))
        }
    }
    
    pub fn write_area(&mut self, cx: &mut Cx, area: Area, prefix: &str,  time: f64) {

        if let Some(time) = self.update_anim_track(cx, time) {
            
            for track_index in 0..self.current.as_ref().unwrap().tracks.len() {
                //if let Some((time, track_index)) = self.fetch_calc_track(cx, ident, time) {
                match &mut self.current.as_mut().unwrap().tracks[track_index] {
                    Track::Color(ft) => {
                        if let Some(begin) = cx.id_starts_with(ft.ident, prefix){
                            let init = Self::_last_color(ft.ident, &self.last_values);
                            let ret = Track::compute_track_value::<Color>(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                            Self::_set_last_color(ft.ident, ret, &mut self.last_values);
                            area.write_color(cx, &begin, ret);
                        }
                    },
                    Track::Vec4(ft) => {
                        if let Some(begin) = cx.id_starts_with(ft.ident, prefix){
                            let init = Self::_last_vec4(ft.ident, &self.last_values);
                            let ret = Track::compute_track_value::<Vec4>(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                            Self::_set_last_vec4(ft.ident, ret, &mut self.last_values);
                            area.write_vec4(cx, &begin, ret);
                        }
                    },
                    Track::Vec3(ft) => {
                        if let Some(begin) = cx.id_starts_with(ft.ident, prefix){
                            let init = Self::_last_vec3(ft.ident, &self.last_values);
                            let ret = Track::compute_track_value::<Vec3>(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                            Self::_set_last_vec3(ft.ident, ret, &mut self.last_values);
                            area.write_vec3(cx, &begin, ret);
                        }
                    },
                    Track::Vec2(ft) => {
                        if let Some(begin) = cx.id_starts_with(ft.ident, prefix){
                            let init = Self::_last_vec2(ft.ident, &self.last_values);
                            let ret = Track::compute_track_value::<Vec2>(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                            Self::_set_last_vec2(ft.ident, ret, &mut self.last_values);
                            area.write_vec2(cx, &begin, ret);
                        }
                    },
                    Track::Float(ft) => {
                        if let Some(begin) = cx.id_starts_with(ft.ident, prefix){
                            let init = Self::_last_float(ft.ident, &self.last_values);
                            let ret = Track::compute_track_value::<f32>(time, &ft.track, &mut ft.cut_init, init, &ft.ease);
                            Self::_set_last_float(ft.ident, ret, &mut self.last_values);
                            area.write_float(cx, &begin, ret);
                        }
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
    pub ident: CxId,
    pub ease: Ease,
    pub cut_init: Option<f32>,
    pub track: Vec<(f64, f32)>
}

#[derive(Clone, Debug)]
pub struct Vec2Track {
    pub ident: CxId,
    pub ease: Ease,
    pub cut_init: Option<Vec2>,
    pub track: Vec<(f64, Vec2)>
}

#[derive(Clone, Debug)]
pub struct Vec3Track {
    pub ident: CxId,
    pub ease: Ease,
    pub cut_init: Option<Vec3>,
    pub track: Vec<(f64, Vec3)>
}

#[derive(Clone, Debug)]
pub struct Vec4Track {
    pub ident: CxId,
    pub ease: Ease,
    pub cut_init: Option<Vec4>,
    pub track: Vec<(f64, Vec4)>
}

#[derive(Clone, Debug)]
pub struct ColorTrack {
    pub ident: CxId,
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
    pub fn float(ident: CxId, ease: Ease, track: Vec<(f64, f32)>) -> Track {
        Track::Float(FloatTrack {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    pub fn to_float(ident: CxId, value: f32) -> Track {
        Track::Float(FloatTrack {
            cut_init: None,
            ease: Ease::Lin,
            ident: ident,
            track: vec![(1.0, value)]
        })
    }
    
    pub fn vec2(ident: CxId, ease: Ease, track: Vec<(f64, Vec2)>) -> Track {
        Track::Vec2(Vec2Track {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    pub fn to_vec2(ident: CxId, value: Vec2) -> Track {
        Track::Vec2(Vec2Track {
            cut_init: None,
            ease: Ease::Lin,
            ident: ident,
            track: vec![(1.0, value)]
        })
    }
    
    pub fn vec3(ident: CxId, ease: Ease, track: Vec<(f64, Vec3)>) -> Track {
        Track::Vec3(Vec3Track {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    pub fn to_vec3(ident: CxId, value: Vec3) -> Track {
        Track::Vec3(Vec3Track {
            cut_init: None,
            ease: Ease::Lin,
            ident: ident,
            track: vec![(1.0, value)]
        })
    }
    
    pub fn vec4(ident: CxId, ease: Ease, track: Vec<(f64, Vec4)>) -> Track {
        Track::Vec4(Vec4Track {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    pub fn to_vec4(ident: CxId, value: Vec4) -> Track {
        Track::Vec4(Vec4Track {
            cut_init: None,
            ease: Ease::Lin,
            ident: ident,
            track: vec![(1.0, value)]
        })
    }
    
    pub fn color(ident: CxId, ease: Ease, track: Vec<(f64, Color)>) -> Track {
        Track::Color(ColorTrack {
            cut_init: None,
            ease: ease,
            ident: ident,
            track: track
        })
    }
    
    pub fn to_color(ident: CxId, value: Color) -> Track {
        Track::Color(ColorTrack {
            cut_init: None,
            ease: Ease::Lin,
            ident: ident,
            track: vec![(1.0, value)]
        })
    }
    
    fn compute_track_value<T>(time: f64, track: &Vec<(f64, T)>, cut_init: &mut Option<T>, init: T, ease: &Ease) -> T
    where T: ComputeTrackValue<T> + Clone
    {
        if track.is_empty() {return init}
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
                return val1.1.lerp_prop(&val2.1, f);
            }
        }
        if cut_init.is_none() {
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = ease.map(time / val2.0) as f32;
        return val1.lerp_prop(&val2.1, f)
    }
    
    pub fn ident(&self) -> CxId {
        match self {
            Track::Float(ft) => {
                ft.ident
            },
            Track::Vec2(ft) => {
                ft.ident
            }
            Track::Vec3(ft) => {
                ft.ident
            }
            Track::Vec4(ft) => {
                ft.ident
            }
            Track::Color(ft) => {
                ft.ident
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

trait ComputeTrackValue<T> {
    fn lerp_prop(&self, b: &T, f: f32) -> T;
}

impl ComputeTrackValue<f32> for f32 {
    fn lerp_prop(&self, b: &f32, f: f32) -> f32 {
        *self + (*b - *self) * f
    }
}

impl ComputeTrackValue<Vec2> for Vec2 {
    fn lerp_prop(&self, b: &Vec2, f: f32) -> Vec2 {
        Vec2 {
            x: self.x + (b.x - self.x) * f,
            y: self.y + (b.y - self.y) * f
        }
    }
}

impl ComputeTrackValue<Vec3> for Vec3 {
    fn lerp_prop(&self, b: &Vec3, f: f32) -> Vec3 {
        Vec3 {
            x: self.x + (b.x - self.x) * f,
            y: self.y + (b.y - self.y) * f,
            z: self.z + (b.z - self.z) * f
        }
    }
}

impl ComputeTrackValue<Vec4> for Vec4 {
    fn lerp_prop(&self, b: &Vec4, f: f32) -> Vec4 {
        let of = 1.0 - f;
        Vec4 {
            x: self.x * of + b.x * f,
            y: self.y * of + b.y * f,
            z: self.z * of + b.z * f,
            w: self.w * of + b.w * f
        }
    }
}


impl ComputeTrackValue<Color> for Color {
    fn lerp_prop(&self, b: &Color, f: f32) -> Color {
        let of = 1.0 - f;
        Color {
            r: self.r * of + b.r * f,
            g: self.g * of + b.g * f,
            b: self.b * of + b.b * f,
            a: self.a * of + b.a * f
        }
    }
}