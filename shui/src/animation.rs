use crate::cx_shared::*;
use crate::cxdrawing::*;
use crate::area::*;
use crate::math::*;

#[derive(Clone,Debug)]
pub struct AnimArea{
    pub area:Area,
    pub start_time:f64,
    pub duration:f64
}

#[derive(Clone,Debug)]
pub struct AnimState<T>{
    pub id:T,
    pub mode:AnimMode,
    pub tracks:Vec<AnimTrack>
}

#[derive(Clone)]
pub struct Animation<T>{
    current_id:T,
    next_id:Option<T>,
    area:Area,
    pub states:Vec<AnimState<T>>
}

impl<T> Animation<T>
where T: std::cmp::PartialEq + std::clone::Clone
{

    pub fn new(current_id:T, states:Vec<AnimState<T>>)->Animation<T>{
        Animation{
            current_id:current_id,
            next_id:None,
            area:Area::Empty,
            states:states
        }
    }

    pub fn change_state(&mut self, cx:&mut Cx, state_id:T){
        let cd = &mut cx.drawing;
        let state_index_op = self.states.iter().position(|v| v.id == state_id);

        if state_index_op.is_none(){
            println!("Starting animation state does not exist in states");
            return;
        }
        let state_index = state_index_op.unwrap();

        // alright first we find area, it already exists
        if let Some(anim) = cd.animations.iter_mut().find(|v| v.area == self.area){
            //do we cut the animation in right now?
            if self.states[state_index].mode.cut(){
                anim.start_time = std::f64::NAN;
                self.current_id = self.states[state_index].id.clone();
                self.next_id = None;
                anim.duration = self.states[state_index].mode.duration();
            }
            else{ // queue it
                self.next_id = Some(self.states[state_index].id.clone());
                let prev_anim_state = self.find_state(self.current_id.clone()).unwrap();
                // lets ask an animation state how long it is
                anim.duration = prev_anim_state.mode.duration() + self.states[state_index].mode.duration()
            }
        }
        else{ // its new
            self.current_id = self.states[state_index].id.clone();
            self.next_id = None;
            cd.animations.push(AnimArea{
                area:self.area.clone(),
                start_time:std::f64::NAN,
                duration:self.states[state_index].mode.duration()
            })
        }
    }

    pub fn find_state(&mut self, id:T)->Option<&mut AnimState<T>>{
        self.states.iter_mut().find(|v| v.id == id)
    }

    pub fn state(&self)->T{
        self.current_id.clone()
    }

    pub fn set_area(&mut self, cd:&mut CxDrawing, area:&Area){
        // alright first we find area, it already exists
        if let Some(anim) = cd.animations.iter_mut().find(|v| v.area == self.area){
            anim.area = area.clone()
        }
        //TODO also update mousecaptures
        self.area = area.clone();
    }

    fn fetch_calc_track(&mut self, cd:&mut CxDrawing, ident:&str, time:f64)->Option<(f64,&mut AnimTrack)>{
        // alright first we find area in running animations
        let anim_index_opt = cd.animations.iter().position(|v| v.area == self.area);
        if anim_index_opt.is_none(){
            return None
        }
        let anim_index = anim_index_opt.unwrap();
        // initialize start time
        if cd.animations[anim_index].start_time.is_nan(){
            cd.animations[anim_index].start_time = time;
        }
        let start_time = cd.animations[anim_index].start_time;
        
        // fetch current state
        let current_state_opt = self.find_state(self.current_id.clone());
        if current_state_opt.is_none(){  // remove anim
            cd.animations.remove(anim_index);
            return None
        }
        let current_state = current_state_opt.unwrap();
        // 
        // then we need to compute the track time
        let time = current_state.mode.compute_time(time - start_time);

        for track in &mut current_state.tracks{
            if track.ident() == ident{
                return Some((time,track));
            }
        }
        None
    } 

    fn fetch_last_track(&mut self, ident:&str)->Option<&AnimTrack>{
         // fetch current state
        let current_state_opt = self.find_state(self.current_id.clone());
        if current_state_opt.is_none(){  // remove anim
            return None
        }
        let current_state = current_state_opt.unwrap();

        for track in &current_state.tracks{
            if track.ident() == ident{
                return Some(track);
            }
        }
        None
    }

    pub fn calc_float(&mut self, cd:&mut CxDrawing, ident:&str, time:f64, init:f32)->f32{
        if let Some((time,track)) = self.fetch_calc_track(cd, ident, time){
            match track{
                AnimTrack::Float(ft)=>{
                    let ret = AnimTrack::compute_track_value::<f32>(time, &ft.track, &mut ft.cut_init, init);
                    ft.last_calc = Some(ret);
                    return ret
                },
                _=>()
            }
        }
        return 0.0
    } 

    pub fn last_float(&mut self, ident:&str)->f32{
        if let Some(track) = self.fetch_last_track(ident){
            match track{
                AnimTrack::Float(ft)=>{
                    if let Some(last_calc) = ft.last_calc{
                        return last_calc;
                    }
                    else if ft.track.len()>0{ // grab the last key in the track
                        return ft.track.last().unwrap().1
                    }
                },
                _=>()
            }
        }
        return 0.0
    }

    pub fn calc_vec2(&mut self, cd:&mut CxDrawing, track_ident:&str, time:f64, init:Vec2)->Vec2{
        if let Some((time,track)) = self.fetch_calc_track(cd, track_ident, time){
            match track{
                AnimTrack::Vec2(ft)=>{
                    let ret =  AnimTrack::compute_track_value::<Vec2>(time, &ft.track, &mut ft.cut_init, init);
                    ft.last_calc = Some(ret.clone());
                    return ret
                },
                _=>()
            }
        }
        return vec2(0.0,0.0)
    }

    pub fn last_vec2(&mut self, ident:&str)->Vec2{
        if let Some(track) = self.fetch_last_track(ident){
            match track{
                AnimTrack::Vec2(ft)=>{
                    if let Some(last_calc) = &ft.last_calc{
                        return last_calc.clone();
                    }
                    else if ft.track.len()>0{ // grab the last key in the track
                        return ft.track.last().unwrap().1.clone()
                    }
                },
                _=>()
            }
        }
        return vec2(0.0,0.0)
    }

    pub fn calc_vec3(&mut self, cd:&mut CxDrawing, area_name:&str, time:f64, init:Vec3)->Vec3{
        if let Some((time,track)) = self.fetch_calc_track(cd, area_name, time){
            match track{
                AnimTrack::Vec3(ft)=>{
                    let ret =  AnimTrack::compute_track_value::<Vec3>(time, &ft.track, &mut ft.cut_init, init);
                    ft.last_calc = Some(ret.clone());
                    return ret    
                },
                _=>()
            }
        }
        return vec3(0.0,0.0,0.0)
    }

    pub fn last_vec3(&mut self, ident:&str)->Vec3{
        if let Some(track) = self.fetch_last_track(ident){
            match track{
                AnimTrack::Vec3(ft)=>{
                    if let Some(last_calc) = &ft.last_calc{
                        return last_calc.clone();
                    }
                    else if ft.track.len()>0{ // grab the last key in the track
                        return ft.track.last().unwrap().1.clone()
                    }
                },
                _=>()
            }
        }
        return vec3(0.0,0.0,0.0)
    }

    pub fn calc_vec4(&mut self, cd:&mut CxDrawing, area_name:&str, time:f64, init:Vec4)->Vec4{
        if let Some((time,track)) = self.fetch_calc_track(cd, area_name, time){
            match track{
                AnimTrack::Vec4(ft)=>{
                    let ret =  AnimTrack::compute_track_value::<Vec4>(time, &ft.track, &mut ft.cut_init, init);
                    ft.last_calc = Some(ret.clone());
                    return ret                    
                },
                _=>()
            }
        }
        return vec4(0.0,0.0,0.0,0.0)
    }

    pub fn last_vec4(&mut self, ident:&str)->Vec4{
        if let Some(track) = self.fetch_last_track(ident){
            match track{
                AnimTrack::Vec4(ft)=>{
                    if let Some(last_calc) =&ft.last_calc{
                        return last_calc.clone();
                    }
                    else if ft.track.len()>0{ // grab the last key in the track
                        return ft.track.last().unwrap().1.clone()
                    }
                },
                _=>()
            }
        }
        return vec4(0.0,0.0,0.0,0.0)
    }    
}

#[derive(Clone,Debug)]
pub struct FloatTrack{
    pub ident:String,
    pub cut_init:Option<f32>,
    pub last_calc:Option<f32>,
    pub track:Vec<(f64, f32)>
}

#[derive(Clone,Debug)]
pub struct Vec2Track{
    pub ident:String,
    pub cut_init:Option<Vec2>,
    pub last_calc:Option<Vec2>,
    pub track:Vec<(f64, Vec2)>
}

#[derive(Clone,Debug)]
pub struct Vec3Track{
    pub ident:String,
    pub cut_init:Option<Vec3>,
    pub last_calc:Option<Vec3>,
    pub track:Vec<(f64, Vec3)>
}

#[derive(Clone,Debug)]

pub struct Vec4Track{
    pub ident:String,
    pub cut_init:Option<Vec4>,
    pub last_calc:Option<Vec4>,
    pub track:Vec<(f64, Vec4)>
}

#[derive(Clone,Debug)]
pub enum AnimTrack{
    Float(FloatTrack),
    Vec2(Vec2Track),
    Vec3(Vec3Track),
    Vec4(Vec4Track),
}

impl AnimTrack{
    pub fn float(ident:&str, track:Vec<(f64,f32)>)->AnimTrack{
        AnimTrack::Float(FloatTrack{
            cut_init:None,
            last_calc:None,
            ident:ident.to_string(),
            track:track
        })
    }

    pub fn vec2(ident:&str, track:Vec<(f64,Vec2)>)->AnimTrack{
        AnimTrack::Vec2(Vec2Track{
            cut_init:None,
            last_calc:None,
            ident:ident.to_string(),
            track:track
        })
    }

    pub fn vec3(ident:&str, track:Vec<(f64,Vec3)>)->AnimTrack{
        AnimTrack::Vec3(Vec3Track{
            cut_init:None,
            last_calc:None,
            ident:ident.to_string(),
            track:track
        })
    }

    pub fn vec4(ident:&str, track:Vec<(f64,Vec4)>)->AnimTrack{
        AnimTrack::Vec4(Vec4Track{
            cut_init:None,
            last_calc:None,
            ident:ident.to_string(),
            track:track
        })
    }

    fn compute_track_value<T>(time:f64, track:&Vec<(f64,T)>, cut_init:&mut Option<T>, init:T) -> T
    where T:ComputeTrackValue<T> + Clone
    {
        if track.is_empty(){return init}
        // find the 2 keys we want
        for i in 0..track.len(){
            if time>= track[i].0{ // we found the left key
                let val1 = &track[i];
                if i == track.len() - 1{ // last key
                    return val1.1.clone()
                }
                let val2 = &track[i+1];
                // lerp it
                let f = ((time - val1.0)/(val2.0-val1.0)) as f32;
                return val1.1.lerp_prop(&val2.1, f);
            }
        }
        if cut_init.is_none(){
            *cut_init = Some(init);
        }
        let val2 = &track[0];
        let val1 = cut_init.as_mut().unwrap();
        let f = (time/val2.0) as f32;
        return  val1.lerp_prop(&val2.1, f)
    }

    pub fn ident(&self)->&String{
        match self{
            AnimTrack::Float(ft)=>{
                &ft.ident
            },
            AnimTrack::Vec2(ft)=>{
                &ft.ident
            }
            AnimTrack::Vec3(ft)=>{
                &ft.ident
            }
            AnimTrack::Vec4(ft)=>{
                &ft.ident
            }
        }
    }
/*
    fn duration(&self)->f64{
        match self{
            AnimTrack::Float(ft)=>{
                let last = ft.track.last();
                if last.is_none(){
                    0.0
                }
                else{
                    last.unwrap().0
                }
            },
            AnimTrack::Vec2(ft)=>{
                let last = ft.track.last();
                if last.is_none(){
                    0.0
                }
                else{
                    last.unwrap().0
                }
            }
            AnimTrack::Vec3(ft)=>{
                let last = ft.track.last();
                if last.is_none(){
                    0.0
                }
                else{
                    last.unwrap().0
                }
            }
            AnimTrack::Vec4(ft)=>{
                let last = ft.track.last();
                if last.is_none(){
                    0.0
                }
                else{
                    last.unwrap().0
                }
            }
        }
    }*/
}

impl<T> AnimState<T>{
    pub fn new(id:T, mode:AnimMode, tracks:Vec<AnimTrack>)->AnimState<T>{
        AnimState{
            id:id,
            mode:mode,
            tracks:tracks
        }
    }
}

#[derive(Clone,Debug)]
pub enum AnimMode{
    Single{speed:f64, cut:bool, len:f64},
    Loop{speed:f64, cut:bool, repeats:f64,len:f64},
    Reverse{speed:f64, cut:bool, repeats:f64,len:f64},
    Bounce{speed:f64, cut:bool, repeats:f64,len:f64},
    Forever{speed:f64, cut:bool},
    LoopForever{speed:f64, cut:bool, len:f64},
    ReverseForever{speed:f64, cut:bool, len:f64},
    BounceForever{speed:f64, cut:bool, len:f64},
}

impl AnimMode{
    pub fn speed(&self)->f64{
        match self{
            AnimMode::Single{speed,..}=>*speed,
            AnimMode::Loop{speed,..}=>*speed,
            AnimMode::Reverse{speed,..}=>*speed,
            AnimMode::Bounce{speed,..}=>*speed,
            AnimMode::BounceForever{speed,..}=>*speed,
            AnimMode::Forever{speed,..}=>*speed,
            AnimMode::LoopForever{speed,..}=>*speed,
            AnimMode::ReverseForever{speed,..}=>*speed,
        }
    }
    pub fn duration(&self)->f64{
        match self{
            AnimMode::Single{len,speed,..}=>len*speed,
            AnimMode::Loop{len,speed,repeats,..}=>len*speed*repeats,
            AnimMode::Reverse{len,speed,repeats,..}=>len*speed*repeats,
            AnimMode::Bounce{len,speed,repeats,..}=>len*speed*repeats,
            AnimMode::BounceForever{..}=>std::f64::INFINITY,
            AnimMode::Forever{..}=>std::f64::INFINITY,
            AnimMode::LoopForever{..}=>std::f64::INFINITY,
            AnimMode::ReverseForever{..}=>std::f64::INFINITY,
        }
    }    
    pub fn cut(&self)->bool{
        match self{
            AnimMode::Single{cut,..}=>*cut,
            AnimMode::Loop{cut,..}=>*cut,
            AnimMode::Reverse{cut,..}=>*cut,
            AnimMode::Bounce{cut,..}=>*cut,
            AnimMode::BounceForever{cut,..}=>*cut,
            AnimMode::Forever{cut,..}=>*cut,
            AnimMode::LoopForever{cut,..}=>*cut,
            AnimMode::ReverseForever{cut,..}=>*cut,
        }
    }
    pub fn repeats(&self)->f64{
        match self{
            AnimMode::Single{..}=>1.0,
            AnimMode::Loop{repeats,..}=>*repeats,
            AnimMode::Reverse{repeats,..}=>*repeats,
            AnimMode::Bounce{repeats,..}=>*repeats,
            AnimMode::BounceForever{..}=>std::f64::INFINITY,
            AnimMode::Forever{..}=>std::f64::INFINITY,
            AnimMode::LoopForever{..}=>std::f64::INFINITY,
            AnimMode::ReverseForever{..}=>std::f64::INFINITY,
        }
    }
    
    pub fn compute_time(&self, time:f64)->f64{
        match self{
            AnimMode::Single{speed,..}=>{
                time * speed
            },
            AnimMode::Loop{len,speed,..}=>{
                (time / speed)  % len
            },
            AnimMode::Reverse{len,speed,..}=>{
                len - (time / speed)  % len
            },
            AnimMode::Bounce{len,speed,..}=>{ 
                let mut local_time = (time / speed)  % (len*2.0);
                if local_time > *len{
                    local_time = 2.0*len - local_time;
                };
                local_time
            },
            AnimMode::BounceForever{len,speed,..}=>{
                let mut local_time = (time / speed)  % (len*2.0);
                if local_time > *len{
                    local_time = 2.0*len - local_time;
                };
                local_time
            },
            AnimMode::Forever{speed,..}=>{
                let local_time = time / speed;
                local_time
            },
            AnimMode::LoopForever{len, speed, ..}=>{
                let local_time = (time / speed)  % len;
                local_time
            },
            AnimMode::ReverseForever{len, speed, ..}=>{
                let local_time = len - (time / speed)  % len;
                local_time
            },
        }
    }
}

trait ComputeTrackValue<T>{
    fn lerp_prop(&self, b:&T, f:f32)->T;
}

impl ComputeTrackValue<f32> for f32{
    fn lerp_prop(&self, b:&f32, f:f32)->f32{
        *self + (*b - *self) * f
    }
}

impl ComputeTrackValue<Vec2> for Vec2{
    fn lerp_prop(&self, b:&Vec2, f:f32)->Vec2{
        Vec2{
            x:self.x + (b.x - self.x) * f,
            y:self.y + (b.y - self.y) * f
        }
    }
}

impl ComputeTrackValue<Vec3> for Vec3{
    fn lerp_prop(&self, b:&Vec3, f:f32)->Vec3{
        Vec3{
            x:self.x + (b.x - self.x) * f,
            y:self.y + (b.y - self.y) * f,
            z:self.z + (b.z - self.z) * f
        }
    }
}

impl ComputeTrackValue<Vec4> for Vec4{
    fn lerp_prop(&self, b:&Vec4, f:f32)->Vec4{
        let of = 1.0-f;
        Vec4{
            x:self.x * of + b.x * f,
            y:self.y * of + b.y * f,
            z:self.z * of + b.z * f,
            w:self.w * of + b.w * f
        }
    }
}