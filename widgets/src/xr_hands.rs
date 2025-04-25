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
        cube:{
            color: #fff
        }
    }
}

pub struct XrPeer{
    id: LiveId,
    state: XrState,
}

#[derive(Live, LiveHook, Widget)]
pub struct XrHands {
    #[redraw] #[rust(DrawList::new(cx))] draw_list: DrawList,
    #[area] #[live] cube: DrawCube,
    #[live] label: DrawText,
    #[rust] peers: Vec<XrPeer>
}

impl XrHands{
        
        
    pub fn join_peer(&mut self, _cx:&mut Cx, id:LiveId, state:XrState){
        if let Some(peer) = self.peers.iter_mut().find(|v| v.id == id){
            peer.state = state;
        }
        else{
            self.peers.push(XrPeer{id, state});
        }
    }
        
    pub fn leave_peer(&mut self, _cx:&mut Cx, id:LiveId){
        self.peers.retain(|v| v.id != id);
    }
        
    pub fn update_peer(&mut self, cx:&mut Cx, id:LiveId, state:XrState){
        self.join_peer(cx, id, state);
    }
    
}

impl Widget for XrHands {
    fn handle_event(&mut self, cx: &mut Cx,event:&Event, _scope:&mut Scope){
        if let Event::XrUpdate(e) = event{
            if e.state.left_controller.clicked_menu(){
                cx.xr_advertise_anchor(e.state.left_controller.grip_pose);
            }
            /*if e.state.left_controller.clicked_thumbstick(){
                cx.xr_discover_anchor();
            }*/
            self.cube.redraw(cx);
        }
    }
    fn draw_3d(&mut self, cx: &mut Cx3d, _scope:&mut Scope)->DrawStep{
        self.draw_list.begin_always(cx);
        
        fn draw_hands(cx: &mut Cx3d, cube:&mut DrawCube, transform: &Mat4, xr_state:&XrState, head:bool){
            // alright so we have a shared anchor
            
            // lets draw our hand controllers
            if head{
                let mata = Mat4::mul(&xr_state.head_pose.to_mat4(), transform);
                cube.color = vec4(1.0,1.0,1.0,1.0);
                cube.cube_size = vec3(0.20,0.10,0.05);
                cube.cube_pos = vec3(0.0,0.0,0.0);
                cube.transform = mata;
                cube.depth_clip = 0.0;
                cube.draw(cx);
            }
            // lets draw our hand controllers
            
            let mata = Mat4::mul(&xr_state.left_controller.grip_pose.to_mat4(), transform);
            cube.color = vec4(0.5,0.5,0.5,1.0);
            cube.cube_size = vec3(0.05,0.05,0.05);
            cube.cube_pos = vec3(0.0,0.0,0.0);
            cube.transform = mata;
            cube.depth_clip = 0.0;
            cube.draw(cx);
                    
            let mata = Mat4::mul(&xr_state.right_controller.grip_pose.to_mat4(), transform);
            cube.color = vec4(0.5,0.5,0.5,1.0);
            cube.cube_size = vec3(0.05,0.05,0.05);
            cube.cube_pos = vec3(0.0,0.0,0.0);
            cube.transform = mata;
            cube.depth_clip = 0.0;
            cube.draw(cx);
                    
            // lets draw all the fingers
            for hand in [&xr_state.left_hand, &xr_state.right_hand]{
                if hand.in_view{
                    for (_index,joint) in hand.joints.iter().enumerate(){
                        let mat = Mat4::mul(&joint.pose.to_mat4(), transform);
                        cube.color = vec4(1.0,1.0,1.0,1.0);
                        cube.cube_size = vec3(0.01,0.01,0.015);
                        cube.cube_pos = vec3(0.0,0.0,0.0);
                        cube.transform = mat;
                        cube.depth_clip = 0.0;
                        cube.draw(cx);
                    }
                    // we should draw the fingertips
                    
                }
                for (i, knuckle) in XrHand::END_KNUCKLES.iter().enumerate(){
                    let joint = &hand.joints[*knuckle];
                    let tip = hand.tips[i];
                    let mat = Mat4::mul(&joint.pose.to_mat4(), transform);
                    cube.color = vec4(1.0,0.0,0.0,1.0);
                    cube.cube_size = vec3(0.01,0.01,0.002);
                    cube.cube_pos = vec3(0.0,0.0,-tip);
                    cube.transform = mat;
                    cube.depth_clip = 0.0;
                    cube.draw(cx);
                }
            }
        }
        
        // alright lets draw those hands
        let xr_state = cx.draw_event.xr_state.as_ref().unwrap();
        // alright lets draw some cubes!
        let speed = 32.0;
        
        let rot = (xr_state.time*speed).rem_euclid(360.0) as f32;
        self.cube.color = vec4(1.0,1.0,1.0,1.0);
        self.cube.cube_size = vec3(0.05,0.05,0.05);
        self.cube.cube_pos = vec3(0.0,0.0,0.0);
        self.cube.transform = Mat4::txyz_s_ry_rx_txyz(
            vec3(0.,0.,0.),
            1.0,
            rot,rot,
            vec3(0.,0.,-0.3)
        );
        self.cube.depth_clip = 1.0;
        self.cube.draw(cx);
        
        if let Some(shared_anchor) = xr_state.shared_anchor{
            let mata = shared_anchor.to_mat4();
            self.cube.color = vec4(0.0,1.0,0.0,1.0);
            self.cube.cube_size = vec3(0.05,0.05,0.05);
            self.cube.transform = mata;
            self.cube.depth_clip = 1.0;
            self.cube.draw(cx);
        }
        
        draw_hands(cx, &mut self.cube, &Mat4::identity(), &xr_state, false);
        
        let mut discovery = None;
        for peer in &self.peers{
            if peer.state.anchor_discovery > xr_state.anchor_discovery{
                discovery = Some(peer.state.anchor_discovery);
            }
            if let Some(my_anchor) = peer.state.shared_anchor{
                if let Some(other_anchor) = xr_state.shared_anchor{
                    let mat = Mat4::mul(&my_anchor.to_mat4().invert(), &other_anchor.to_mat4());
                    draw_hands(cx, &mut self.cube, &mat, &peer.state, true)
                }
            }
            else{
                draw_hands(cx, &mut self.cube, &Mat4::identity(), &peer.state, true)
            }
        }
        if let Some(discovery) = discovery{
            cx.xr_discover_anchor(discovery);
        }
                
        self.draw_list.end(cx);
        DrawStep::done()
    }
}

