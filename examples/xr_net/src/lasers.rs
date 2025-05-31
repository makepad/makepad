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
            return vec4(Pal::iq1(self.life-self.time+self.index*0.1)*1.0,0.0);//mix(#f00,#4440,3*self.life);
        }
        fn get_size(self)->vec3{
            let size = pow(max(3.0-(self.life),0.0)/4.0,1.);
            return self.cube_size * size*vec3(0.1,1.2*sin(self.life),(6-self.index+1)*1.0);
        }
        fn get_pos(self)->vec3{
            let travel = self.life * 0.4;
            return vec3(0.0, 0.0, -travel)
        }
        cube_size:vec3(0.01,0.01,0.015);
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
}


struct Bullet{
    shot_at: f64,
    index: usize,
    pose: Pose,
}

#[derive(Default)]
struct Bullets{
    last_fired: f64,
    bullets: Vec<Bullet>,
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
    fn draw(&mut self, cx:&mut Cx3d, cube:&mut DrawBullet, xr_state:&XrState, anchor_map:&Mat4){
        if xr_state.time < self.last_fired{ // we rejoined
            self.last_fired = xr_state.time;
        }
        if xr_state.time - self.last_fired > 0.005 {
            for hand in xr_state.hands(){
                if !hand.in_view(){
                    continue
                }
                for (i,pose) in hand.end_knuckles().iter().enumerate(){
                    if !hand.tip_active(i){continue;}
                    
                    self.last_fired = xr_state.time;
                    self.bullets.push(Bullet{
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
        if let Event::XrUpdate(_) = event{
            self.redraw(cx);
        }
    }
    fn draw_3d(&mut self, cx: &mut Cx3d, _scope:&mut Scope)->DrawStep{
        self.draw_list.begin_always(cx);
        
        // alright lets draw those hands
        let xr_state = cx.draw_event.xr_state.as_ref().unwrap();
        
        //let dt = profile_start();
        self.bullets.draw(cx, &mut self.draw_bullet, &xr_state, &Mat4::identity());
        
        for peer in &mut self.peers{
            let peer_state = peer.tween(xr_state.time);
            if let Some(other_anchor) = &peer_state.anchor{
                if let Some(my_anchor) = &xr_state.anchor{
                    // alright we need a mapping mat4 from 2 anchors
                    let anchor_map = other_anchor.mapping_to(my_anchor);
                    peer.bullets.draw(cx, &mut self.draw_bullet, &peer_state, &anchor_map)
                }
            }
        }
        //profile_end!(dt);         
        self.draw_list.end(cx);
        DrawStep::done()
    }
}

