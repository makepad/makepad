use {
    crate::{
        makepad_derive_widget::*,
        makepad_draw::*,
        widget::*,
    },
};

use crate::makepad_draw::shader::draw_cube::DrawCube; 

live_design!{
    link widgets;
    use link::theme::*;
    use link::shaders::*;
    
    pub XrHands = {{XrHands}} {

        draw_bullet:{
            color: #0 
            fn get_color(self, dp: float)->vec4{
                let ambient = vec3(0.2,0.2,0.2) 
                let color = self.color.xyz * dp * self.color.w + ambient;
                return mix(#1100,#4440,self.life);
            }
            fn get_size(self)->vec3{
                return self.cube_size * self.data
            }
            fn get_size(self)->vec3{
                let size = max(3.0-(self.life),0.0)/4.0;
                return self.cube_size * size;
            }
            fn get_pos(self)->vec3{
                let travel = self.life * 0.25;
                return vec3(0.0, 0.0, -travel)
            }
            cube_size:vec3(0.01,0.01,0.015);
        }
        draw_knuckle:{
            color: #fff;
            cube_size:vec3(0.01,0.01,0.015);
        }
        draw_tip:{
            color: #f00;
            cube_size:vec3(0.01,0.01,0.001);
        }
        draw_align:{
            color:#f00
            cube_size: vec3(0.02,0.02,0.02)
        }
        draw_grip:{
            color: #777,
            cube_size: vec3(0.05,0.05,0.05)
        }
        draw_aim:{
            color: #777,
            cube_size: vec3(0.05,0.05,0.05)
        }
                    
    }
}

struct Bullet{
    shot_at: f64,
    pose: Pose,
}
#[derive(Default)]
struct Bullets{
    last_fired: f64,
    bullets: Vec<Bullet>,
}

pub struct XrPeer{
    id: LiveId,
    state: XrState,
    bullets: Bullets
}

impl Bullets{
    fn draw(&mut self, cx:&mut Cx3d, cube:&mut DrawCube, xr_state:&XrState, anchor_map:&Mat4){
        if xr_state.time < self.last_fired{ // we rejoined
            self.last_fired = xr_state.time;
        }
        if xr_state.time - self.last_fired > 0.01 {
            for hand in xr_state.hands(){
                if !hand.in_view(){
                    continue
                }
                for pose in hand.end_knuckles(){
                    self.last_fired = xr_state.time;
                    self.bullets.push(Bullet{
                        shot_at: xr_state.time,
                        pose:*pose
                    });
                    if self.bullets.len()>4500{
                        self.bullets.remove(0);
                    }
                }
            }
        }
        for bullet in &self.bullets{
            let mat = Mat4::mul(&bullet.pose.to_mat4(), anchor_map);

            cube.life = (xr_state.time - bullet.shot_at) as f32;
            cube.transform = mat;
            cube.depth_clip = 1.0;
            cube.draw(cx);
        }
    }
}

const ALIGN_RESET: f64 = 0.5;
struct AlignMode{
    start_time: f64,
    anchors: XrSceneAnchors,
}

#[derive(Live, LiveHook, Widget)]
pub struct XrHands {
    #[redraw] #[rust(DrawList::new(cx))] draw_list: DrawList,
    #[live] draw_align: DrawCube,
    #[area] #[live] draw_knuckle: DrawCube,
    #[live] draw_controller: DrawCube,
    #[live] draw_bullet: DrawCube,
    #[live] draw_grip: DrawCube,
    #[live] draw_aim: DrawCube,
    #[live] draw_tip: DrawCube,
                
    #[live] label: DrawText,
    #[rust] peers: Vec<XrPeer>,
    #[rust] align_mode: Option<AlignMode>,
    #[rust] bullets: Bullets,
}

impl XrHands{
    
    pub fn join_peer(&mut self, _cx:&mut Cx, id:LiveId, state:XrState){
        if let Some(peer) = self.peers.iter_mut().find(|v| v.id == id){
            peer.state = state;
        }
        else{
            self.peers.push(XrPeer{id, state, bullets:Bullets::default()});
        }
    }
        
    pub fn leave_peer(&mut self, _cx:&mut Cx, id:LiveId){
        self.peers.retain(|v| v.id != id);
    }
        
    pub fn update_peer(&mut self, cx:&mut Cx, id:LiveId, state:XrState){
        self.join_peer(cx, id, state);
    }
    
    fn handle_alignment(&mut self, _cx:&mut Cx, e:&XrUpdateEvent){
        // the special space gesture
        
        if e.menu_pressed() || e.clicked_menu(){
            // alright lets toggle align / settings mode
            if let Some(align_mode) = &mut self.align_mode{
                // close and save the aligncubes
                if e.state.time - align_mode.start_time < ALIGN_RESET{
                    align_mode.anchors = XrSceneAnchors{
                        left: e.state.vec_in_head_space(vec3(-0.2,0.0,-0.4)),
                        right: e.state.vec_in_head_space(vec3(0.2,0.0,-0.4))
                    }
                }
                else{
                    
                    self.align_mode = None;
                }
                
            }
            else if self.align_mode.is_none(){
                // we have to load up the alignmode settings
                let anchors = if let Some(anchors) = &e.state.scene_anchors{
                    *anchors
                }
                else{
                    XrSceneAnchors{
                        left: e.state.vec_in_head_space(vec3(-0.2,0.0,-0.4)),
                        right: e.state.vec_in_head_space(vec3(0.2,0.0,-0.4))
                    }
                };
                
                // or spawn the cubes somewhere meaningful
                self.align_mode = Some(AlignMode{
                    start_time: e.state.time,
                    anchors,
                });
            }
        }
        // possibly move the cubes around
        if let Some(align_mode) = &mut self.align_mode{
            if e.state.left_hand.pinch_only_index(){
                align_mode.anchors.left = e.state.left_hand.tip_pos_thumb();
            }
            if e.state.right_hand.pinch_only_index(){
                align_mode.anchors.right = e.state.right_hand.tip_pos_thumb();
            }
            if e.state.left_controller.triggered(){
                align_mode.anchors.left = e.state.left_controller.aim_pose.position;
            }
            if e.state.right_controller.triggered(){
                align_mode.anchors.right = e.state.right_controller.aim_pose.position;
            }
        }
    }
    
    fn draw_alignment(&mut self, cx:&mut Cx3d, _xr_state:&XrState){
        if let Some(align) = &self.align_mode{
            //if xr_state.time - align.start_time >  ALIGN_START_TIMEOUT{
                // lets draw cubes from the quaternions
                let cube = &mut self.draw_align;
                cube.color = color!(#00f);
                cube.transform = Mat4::translation(align.anchors.left);
                cube.depth_clip = 1.0;
                cube.draw(cx);
                cube.color = color!(#0f0);
                cube.transform = Mat4::translation(align.anchors.right);
                cube.draw(cx);
            //}
        }
    }
}

impl Widget for XrHands {
    fn handle_event(&mut self, cx: &mut Cx,event:&Event, _scope:&mut Scope){
        if let Event::XrUpdate(e) = event{
            if e.clicked_menu(){
                cx.xr_advertise_anchor(e.state.left_controller.grip_pose);
            }
            self.handle_alignment(cx, e);
            self.redraw(cx);
        }
    }
    fn draw_3d(&mut self, cx: &mut Cx3d, _scope:&mut Scope)->DrawStep{
        self.draw_list.begin_always(cx);
        
        fn draw_hands(
            cx: &mut Cx3d, 
            cube:&mut DrawCube, 
            tip: &mut DrawCube,
            transform: &Mat4, 
            xr_state:&XrState, 
        ){
            // lets draw all the fingers
            for hand in [&xr_state.left_hand, &xr_state.right_hand]{
                if hand.in_view(){
                    for (_index,joint) in hand.joints.iter().enumerate(){
                        let mat = Mat4::mul(&joint.to_mat4(), transform);
                        cube.cube_pos = vec3(0.0,0.0,0.0);
                        cube.transform = mat;
                        cube.depth_clip = 0.0;
                        cube.draw(cx);
                    }
                }
                for (i, knuckle) in XrHand::END_KNUCKLES.iter().enumerate(){
                    let joint = &hand.joints[*knuckle];
                    let mat = Mat4::mul(&joint.to_mat4(), transform);
                    tip.cube_pos = vec3(0.0,0.0,-hand.tips[i]);
                    tip.transform = mat;
                    tip.depth_clip = 0.0;
                    tip.draw(cx);
                }
            }
        }
        
        fn _draw_controllers(
            cx: &mut Cx3d, 
            draw_aim:&mut DrawCube, 
            draw_grip: &mut DrawCube,
            transform: &Mat4, 
            xr_state:&XrState, 
        ){
            // lets draw our hand controllers
            let mata = Mat4::mul(&xr_state.left_controller.aim_pose.to_mat4(), transform);
            draw_aim.cube_pos = vec3(0.0,0.0,0.0);
            draw_aim.transform = mata;
            draw_aim.depth_clip = 0.0;
            draw_aim.draw(cx);
                                
            let mata = Mat4::mul(&xr_state.right_controller.grip_pose.to_mat4(), transform);
            draw_grip.cube_pos = vec3(0.0,0.0,0.0);
            draw_grip.transform = mata;
            draw_grip.depth_clip = 0.0;
            draw_grip.draw(cx);
        }
        
        
        fn _draw_head(
            cx: &mut Cx3d, 
            cube:&mut DrawCube, 
            transform: &Mat4, 
            xr_state:&XrState, 
        ){
            let mata = Mat4::mul(&xr_state.head_pose.to_mat4(), transform);
            cube.color = vec4(1.0,1.0,1.0,1.0);
            cube.cube_size = vec3(0.20,0.10,0.05);
            cube.cube_pos = vec3(0.0,0.0,0.0);
            cube.transform = mata;
            cube.depth_clip = 0.0;
            cube.draw(cx);
        }
        
        fn _draw_democube(
            cx: &mut Cx3d, 
            cube:&mut DrawCube, 
            xr_state:&XrState, 
            ){
            let speed = 32.0; 
            let rot = (xr_state.time*speed).rem_euclid(360.0) as f32;
            cube.color = vec4(1.0,1.0,1.0,1.0);
            cube.cube_size = vec3(0.05,0.05,0.05);
            cube.cube_pos = vec3(0.0,0.0,0.0);
            cube.transform = Mat4::txyz_s_ry_rx_txyz(
                vec3(0.,0.,0.),
                1.0,
                rot,rot,
                vec3(0.,0.,-0.3)
            );
            cube.depth_clip = 1.0;
            cube.draw(cx);
        }
        
        // alright lets draw those hands
        let xr_state = cx.draw_event.xr_state.as_ref().unwrap();
        
        self.draw_alignment(cx, xr_state);

        draw_hands(cx, &mut self.draw_knuckle, &mut self.draw_tip, &Mat4::identity(), &xr_state);
        
        self.bullets.draw(cx, &mut self.draw_bullet, &xr_state, &Mat4::identity());
        /*
        let mut discovery = 0;
        for peer in &mut self.peers{
            if peer.state.anchor_discovery > xr_state.anchor_discovery{
                discovery = discovery.max(peer.state.anchor_discovery);
            }
            if let Some(my_anchor) = peer.state.scene_anchor{
                if let Some(other_anchor) = xr_state.scene_anchor{
                    let anchor_map = Mat4::mul(&my_anchor.to_mat4().invert(), &other_anchor.to_mat4());
                    draw_hands(cx, &mut self.draw_knuckle, &mut self.draw_tip, &anchor_map, &peer.state);
                    
                    peer.bullets.draw(cx, &mut self.draw_bullet, &peer.state, &anchor_map)
                    
                }
            }
            else{
                draw_hands(cx, &mut self.draw_knuckle, &mut self.draw_tip, &Mat4::identity(), &peer.state)
            }
        }
        if discovery>0{
            cx.xr_discover_anchor(discovery);
        }*/
                
        self.draw_list.end(cx);
        DrawStep::done()
    }
}

