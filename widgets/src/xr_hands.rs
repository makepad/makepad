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

#[derive(Live, LiveHook, Widget)]
pub struct XrHands {
    #[redraw] #[rust(DrawList::new(cx))] draw_list: DrawList,
    #[area] #[live] cube: DrawCube,
    #[live] label: DrawText
}

impl Widget for XrHands {
    fn handle_event(&mut self, cx: &mut Cx,event:&Event, _scope:&mut Scope){
        if let Event::XrUpdate(_e) = event{
            self.cube.redraw(cx);
        }
    }
            
    fn draw_3d(&mut self, cx: &mut Cx3d, _scope:&mut Scope)->DrawStep{
        self.draw_list.begin_always(cx);
        // alright lets draw those hands
        let xr_state = cx.draw_event.xr_state.as_ref().unwrap();
        // alright lets draw some cubes!
        let speed = 32.0;
        
        let rot = (xr_state.time*speed).rem_euclid(360.0) as f32;
        self.cube.color = vec4(1.0,1.0,1.0,1.0);
        self.cube.depth_clip = 1.0;
        self.cube.cube_size = vec3(0.05,0.05,0.05);
        self.cube.transform = Mat4::txyz_s_ry_rx_txyz(
            vec3(0.,0.,0.),
            1.0,
            rot,rot,
            vec3(0.,0.,-0.3)
        );
        self.cube.draw(cx);
        
        self.cube.depth_clip = 1.0;
                                
        // lets draw our hand controllers
        let mata = xr_state.left_controller.grip_pose.to_mat4();
        self.cube.cube_size = vec3(0.05,0.05,0.05);
        self.cube.transform = mata;
        self.cube.draw(cx);
        
        let mata = xr_state.right_controller.grip_pose.to_mat4();
        self.cube.cube_size = vec3(0.05,0.05,0.05);
        self.cube.transform = mata;
        self.cube.depth_clip = 0.0;
        self.cube.draw(cx);
        
        // lets draw all the fingers
        for hand in [&xr_state.left_hand, &xr_state.right_hand]{
            if hand.in_view{
                for (index,joint) in hand.joints.iter().enumerate(){
                    if XrHand::is_tip(index){
                        self.cube.cube_size = vec3(0.01,0.01,0.005);
                        self.cube.color = vec4(1.0,0.0,0.0,1.0)
                    }
                    else {
                        self.cube.cube_size = vec3(0.01,0.01,0.015);
                        self.cube.color = vec4(1.0,1.0,1.0,1.0)
                    }
                    let mat = joint.pose.to_mat4();
                    self.cube.transform = mat;
                    self.cube.draw(cx);
                }
            }
        }
        
        self.draw_list.end(cx);
        DrawStep::done()
    }
}

