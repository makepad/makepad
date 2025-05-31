use {
    crate::{
        makepad_widgets::*,
    },
};

use crate::makepad_draw::shader::draw_cube::DrawCube; 

live_design!{
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    
    pub DrawBullet = {{DrawBullet}} {
        color: #0 
                        
        fn get_color(self, dp: float)->vec4{
            let ambient = vec3(0.2,0.2,0.2) 
            let color = self.color.xyz * dp * self.color.w + ambient;
            if mod(self.tap_count,2.0)<0.5{
                return vec4(Pal::iq2(self.life-self.index*0.1)*(1.0-self.life),0.0)
            }
            return vec4(Pal::iq1(self.life-self.time+self.index*0.1),0.0);//mix(#f00,#4440,3*self.life);
        }
        fn get_size(self)->vec3{
            if mod(self.tap_count,2.0)<0.5{
                let size = (sin(self.time*3 + self.life*.03)+1.2)*.30;//pow(max(3.0-(self.life),0.0)/4.0,1.);
                return self.cube_size * size*vec3(1);
            }
            let size = pow(max(5.0-(self.life),0.0)/6.0,1.)*clamp((1.0-self.angle/2),0.0,1.0) ;
            return self.cube_size * size*vec3(0.1,0.5,10.0);//(6-self.index+1)*1.0);
        }
        fn get_pos(self)->vec3{
            if mod(self.tap_count,2.0)<0.5{
                let travel =(sin(self.life*1+self.time)+1.2)*0.3;
                return vec3(0.0, 0.0, -travel)
            }
            let travel = self.life * 0.4 * (self.angle_velo*8.0+ 0.5) ;
            return vec3(0.0, 0.0, -travel-0.08 + clamp((self.angle/2),0.0,1.0)*0.05)
        }
        
        fn fragment(self)->vec4{
            if mod(self.tap_count,2.0)<0.5{
                return self.pixel();
            }
            return depth_clip(self.world, self.pixel(), self.depth_clip);
        }
        cube_size:vec3(0.01,0.01,-0.015);
    }
    
    
    pub XrLasers = {{XrLasers}} {
    }
}


#[derive(Live, LiveHook, LiveRegister)]
#[repr(C)]
pub struct DrawBullet {
    #[deref] pub draw_super: DrawCube,
    #[live(0.0)] pub life: f32,
    #[live(0.0)] pub index: f32,
    #[live(0.0)] pub angle: f32,
    #[live(0.0)] pub angle_velo: f32,
    #[live(0.0)] pub pos_velo: f32,
    #[live(0.0)] pub tap_count:f32,
}


struct Bullet{
    shot_at: f64,
    index: usize,
    angle: f32,
    angle_velo: f32,
    pos_velo: f32,
    pose: Pose,
}

#[derive(Default)]
struct AngleVel{
    angle: f32,
    angle_velo: f32,
    pos: Vec3,
    pos_velo: f32,
}

#[derive(Default)]
struct Bullets{
    last_fired: f64,
    bullets: Vec<Bullet>,
    last_angles: Vec<[AngleVel;5]>
}

pub struct XrPeer{
    id: LiveId,
    ahead: bool,
    min_lag: f64,
    states: Vec<XrState>,
    bullets: Bullets
}

impl XrPeer{
    fn state(&self)->&XrState{
        self.states.last().as_ref().unwrap()        
    }
    fn tween(&self, _host_time:f64)->XrState{
        // TODO do frame tweening/prediction/packet smoothing
        if self.states.len()>=2{
            self.states[0].clone()
        }
        else{
            self.states[0].clone()
        }
    }
}

impl Bullets{
    fn draw(&mut self, cx:&mut Cx3d, cube:&mut DrawBullet, xr_state:&XrState, anchor_map:&Mat4, idx:usize,tc:f32){
        while self.last_angles.len() < idx + 1{
            self.last_angles.push(Default::default());
        }
        let last_angles = &mut self.last_angles[idx];
        if xr_state.time < self.last_fired{ // we rejoined
            self.last_fired = xr_state.time;
        }
        if xr_state.time - self.last_fired > 0.005 {
            for hand in xr_state.hands(){
                if !hand.in_view(){
                    continue
                }
                for (i,pose) in hand.end_knuckles().iter().enumerate(){
                    //if !hand.tip_active(i){continue;}
                    let base = hand.base_knuckles()[i];
                    
                    self.last_fired = xr_state.time;
                    
                    // lets make 2 vecs pointing forward and
                    let v1 = pose.orientation.rotate_vec3(&vec3(0.0,0.0,1.0));
                    let v2 = base.orientation.rotate_vec3(&vec3(0.0,0.0,1.0));
                    let angle = v1.dot(v2).acos();
                    let la = &mut last_angles[i];
                    
                    let pos_velo = (la.pos - base.position).length();
                    la.pos = pose.position;
                    
                    let angle_velo = la.angle - angle;
                    la.angle = angle;
                    la.angle_velo = 0.7*la.angle_velo + 0.3*angle_velo;
                    la.pos_velo = 0.7*la.pos_velo + 0.3 *pos_velo;
                    
                    
                    self.bullets.push(Bullet{
                        angle:v1.dot(v2).acos(),
                        angle_velo: la.angle_velo,
                        pos_velo: la.pos_velo,
                        shot_at: xr_state.time,
                        index: i,
                        pose:**pose
                    });
                    if self.bullets.len()>4500{
                        self.bullets.remove(0);
                    }
                }
            }
        }
        for bullet in &self.bullets{
            let mat = Mat4::mul(&bullet.pose.to_mat4(), anchor_map);
            cube.index = bullet.index as f32;
            cube.life = (xr_state.time - bullet.shot_at) as f32;
            cube.transform = mat;
            cube.depth_clip = 1.0;
            cube.angle = bullet.angle;
            cube.angle_velo = bullet.angle_velo;
            cube.pos_velo = bullet.pos_velo;
            cube.tap_count = tc;
            cube.draw(cx);
        }
    }
}

#[derive(Live, LiveHook, Widget)]
pub struct XrLasers {
    #[redraw] #[rust(DrawList::new(cx))] draw_list: DrawList,
    #[area] #[live] draw_bullet: DrawBullet,
    #[rust] peers: Vec<XrPeer>,
    #[rust] bullets: Bullets,
    #[rust] tap_count: f32,
}

impl XrLasers{
    
    pub fn leave_peer(&mut self, _cx:&mut Cx, id:LiveId){
        self.peers.retain(|v| v.id != id);
    }
        
    pub fn update_peer(&mut self, _cx:&mut Cx, id:LiveId, state:XrState, e:&XrUpdateEvent){
        if let Some(peer) = self.peers.iter_mut().find(|v| v.id == id){
            peer.ahead = state.time > e.state.time;
            peer.min_lag = peer.min_lag.min((state.time - e.state.time).abs());
            let peer_state = peer.state();
            if state.order_counter>peer_state.order_counter ||
            peer_state.order_counter - state.order_counter>128{
                peer.states.insert(0, state);
            }
            peer.states.truncate(2);
        }
        else{
            self.peers.push(XrPeer{
                id, 
                ahead: false,
                min_lag: 1.0,
                states: vec![state], 
                bullets:Bullets::default()
            });
        }
    }
}

impl Widget for XrLasers {
    fn handle_event(&mut self, cx: &mut Cx,event:&Event, _scope:&mut Scope){
        if let Event::XrUpdate(e) = event{
            if e.menu_pressed(){
                 self.tap_count += 1.0;
            }
            self.redraw(cx);
        }
    }
    fn draw_3d(&mut self, cx: &mut Cx3d, _scope:&mut Scope)->DrawStep{
        self.draw_list.begin_always(cx);
        
        // alright lets draw those hands
        let xr_state = cx.draw_event.xr_state.as_ref().unwrap();
        
        //let dt = profile_start();
        let mut idx = 0;
        self.bullets.draw(cx, &mut self.draw_bullet, &xr_state, &Mat4::identity(), idx, self.tap_count);
        idx += 1;
        for peer in &mut self.peers{
            let peer_state = peer.tween(xr_state.time);
            if let Some(other_anchor) = &peer_state.anchor{
                if let Some(my_anchor) = &xr_state.anchor{
                    // alright we need a mapping mat4 from 2 anchors
                    let anchor_map = other_anchor.mapping_to(my_anchor);
                    peer.bullets.draw(cx, &mut self.draw_bullet, &peer_state, &anchor_map, idx, self.tap_count);
                    idx += 1;
                }
            }
        }
        //profile_end!(dt);         
        self.draw_list.end(cx);
        DrawStep::done()
    }
}

