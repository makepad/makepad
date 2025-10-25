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
    
    pub XrHands = {{XrHands}} {
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
        draw_test:{
            color:#f00
            cube_size: vec3(0.02,0.02,0.02)
        }
        draw_head:{
            color:#fff
            cube_size: vec3(0.0,0.0,0.00)
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

pub struct XrPeer{
    id: LiveId,
    ahead: bool,
    min_lag: f64,
    states: Vec<XrState>,
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

const ALIGN_MODE: f64 = 0.4;
struct AlignMode{
    anchor: XrAnchor,
}

#[derive(Live, LiveHook, Widget)]
pub struct XrHands {
    #[redraw] #[rust(DrawList::new(cx))] draw_list: DrawList,
    #[live] draw_align: DrawCube,
    #[live] draw_test: DrawCube,
    #[live] draw_head: DrawCube,
    #[area] #[live] draw_knuckle: DrawCube,
    #[live] draw_controller: DrawCube,
    #[live] draw_grip: DrawCube,
    #[live] draw_aim: DrawCube,
    #[live] draw_tip: DrawCube,
                
    #[live] label: DrawText,
    #[rust] peers: Vec<XrPeer>,
    #[rust] align_last_click: f64,
    #[rust] align_mode: Option<AlignMode>,
}

impl XrHands{
    
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
            });
        }
    }
    
    fn handle_alignment(&mut self, cx:&mut Cx, e:&XrUpdateEvent){
        // the special space gesture
        
        if e.menu_pressed() || e.clicked_menu(){
            if e.state.time - self.align_last_click < ALIGN_MODE{
                // alright lets toggle align / settings mode
                if let Some(align_mode) = &mut self.align_mode{
                    // lets store the anchors
                    cx.xr_set_local_anchor(align_mode.anchor);
                    self.align_mode = None;
                }
                else if self.align_mode.is_none(){
                    // we have to load up the alignmode settings
                    let anchor = if let Some(anchor) = &e.state.anchor{
                        *anchor
                    }
                    else{
                        XrAnchor{
                            left: e.state.vec_in_head_space(vec3(-0.2,0.0,-0.4)),
                            right: e.state.vec_in_head_space(vec3(0.2,0.0,-0.4))
                        }
                    };
                                    
                    // or spawn the cubes somewhere meaningful
                    self.align_mode = Some(AlignMode{
                        anchor,
                    });
                }
                
            }
            self.align_last_click = e.state.time;
        }
        // possibly move the cubes around
        if let Some(align_mode) = &mut self.align_mode{
            if e.state.left_hand.pinch_not_index(){
                align_mode.anchor.left = e.state.left_hand.tip_pos_thumb();
            }
            if e.state.right_hand.pinch_not_index(){
                align_mode.anchor.right = e.state.right_hand.tip_pos_thumb();
            }
            if e.state.left_controller.triggered(){
                align_mode.anchor.left = e.state.left_controller.aim_pose.position;
            }
            if e.state.right_controller.triggered(){
                align_mode.anchor.right = e.state.right_controller.aim_pose.position;
            }
        }
    }
    
    fn draw_alignment(&mut self, cx:&mut Cx3d, _xr_state:&XrState){
        if let Some(align) = &self.align_mode{
            //if xr_state.time - align.start_time >  ALIGN_START_TIMEOUT{
                // lets draw cubes from the quaternions
                let cube = &mut self.draw_align;
                cube.color = color!(#00f);
                cube.transform = Pose::new(align.anchor.to_quat(),align.anchor.left).to_mat4();
                cube.depth_clip = 1.0;
                cube.draw(cx);
                cube.color = color!(#0f0);
                cube.transform = Pose::new(align.anchor.to_quat_rev(),align.anchor.right).to_mat4();
                cube.draw(cx);
            //}
        }
    }
}

impl Widget for XrHands {
    fn handle_event(&mut self, cx: &mut Cx,event:&Event, _scope:&mut Scope){
        if let Event::XrUpdate(e) = event{
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
                    if !hand.tip_active(i){continue;}
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
        
        
        fn draw_head(
            cx: &mut Cx3d, 
            cube:&mut DrawCube, 
            transform: &Mat4, 
            xr_state:&XrState, 
        ){
            let mata = Mat4::mul(&xr_state.head_pose.to_mat4(), transform);
            cube.color = vec4(1.0,1.0,1.0,1.0);
            cube.cube_pos = vec3(0.0,0.0,0.0);
            cube.transform = mata;
            cube.depth_clip = 0.0;
            cube.draw(cx);
        }
        
        fn _draw_democube(
            cx: &mut Cx3d, 
            cube:&mut DrawCube, 
            anchor:&XrAnchor, 
            ){
            let _speed = 32.0; 
            //let rot = (xr_state.time*speed).rem_euclid(360.0) as f32;
            cube.color = vec4(1.0,1.0,1.0,1.0);
            cube.cube_size = vec3(0.1,0.1,0.1);
            cube.cube_pos = vec3(0.0,0.1,0.0);
            cube.transform = anchor.to_mat4();
                
            /*Mat4::txyz_s_ry_rx_txyz(
                vec3(0.,0.,0.),
                1.0,
                rot,rot,
                vec3(0.,0.,-0.3)
            );*/
            cube.depth_clip = 1.0;
            cube.draw(cx);
        }
        // alright lets draw those hands
        let xr_state = cx.draw_event.xr_state.as_ref().unwrap();
        
        self.draw_alignment(cx, xr_state);
            
        draw_hands(cx, &mut self.draw_knuckle, &mut self.draw_tip, &Mat4::identity(), &xr_state);
        
        for peer in &mut self.peers{
            let peer_state = peer.tween(xr_state.time);
            if let Some(other_anchor) = &peer_state.anchor{
                if let Some(my_anchor) = &xr_state.anchor{
                    // alright we need a mapping mat4 from 2 anchors
                    let anchor_map = other_anchor.mapping_to(my_anchor);
                    draw_hands(cx, &mut self.draw_knuckle, &mut self.draw_tip, &anchor_map, &peer_state);
                    
                    draw_head(cx, &mut self.draw_head, &anchor_map, &peer_state);
                }
            }
            else{
                draw_hands(cx, &mut self.draw_knuckle, &mut self.draw_tip, &Mat4::identity(), &peer_state)
            }
        }
        //profile_end!(dt);         
        self.draw_list.end(cx);
        DrawStep::done()
    }
}

