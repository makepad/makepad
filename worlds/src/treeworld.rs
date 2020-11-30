// a bunch o buttons to select the world
use makepad_render::*;
use crate::skybox::SkyBox;

#[derive(Clone)]
pub struct TreeWorld {
    pub view: View,
    pub area: Area,
    pub sky_box: SkyBox
}



/*
Low    quest1
Medium quest2
High   pcbase
Ultra  pchigh 
*/
impl TreeWorld {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            view: View::new(),
            area: Area::Empty,
            sky_box: SkyBox::new(cx),
        }  
    }  
      
    pub fn style(cx: &mut Cx) {
        
        live_body!(cx, r#"
            
            self::color: #E27D3A;
            self::leaf_1: #C1FF00;
            self::leaf_2: #009713;
            self::angle: 0.500;
            self::off:0.183;
            self::width: 0.3;
            self::alpha: 0.114;
         
            self::shader: Shader { 
                use makepad_render::shader_std::prelude::*;
                use makepad_worlds::worldview::uniforms::*;
                
                uniform max_depth: float;

                default_geometry: makepad_render::shader_std::quad_2d;

                geometry geom: vec2;
                
                instance in_path: float;
                instance depth: float;

                varying color: vec4; 
                fn vertex() -> vec4 {  
                    let pos = vec2(0.0, 0.5);    
                    let scale = vec2(0.2, 0.2);
                    let dir = vec2(0.0, 0.8);
                    let smaller = vec2(.85, 0.85);
                    let path = in_path;
                    let nodesize = vec2(1.);
                    let z = 0.0;
                    let last_z = 0.0;
                    let z_base = -1.5;  
                    for i from 0 to 20 {
                        if float(i) >= depth { 
                            break;
                        }
                         
                        let turn_right = mod (path, 2.);
                        let turn_fwd = mod (path, 8.);
                        let angle = 50.*self::angle;
                        last_z = z; 
                        if (turn_right > 0.) {
                            angle = -1.0 * angle;
                        }
                        if(turn_fwd > 3.){ 
                            z += 0.4 * scale.x;  
                        }
                        else{
                             z -= 0.4 * scale.x;
                        }
                        z += sin(time + 10. * pos.x)*0.01;
                        angle += sin(time + 10. * pos.x) * 5.;
                          
                        let d_left = max(0.1 - length(left_input_pos - vec3(pos, z_base + z)), 0.) * 300.0;
                        let d_right = max(0.1 - length(right_input_pos - vec3(pos, z_base + z)), 0.) * 300.0;

                        angle -= d_left;
                        angle += d_right;
                        
                        dir = Math::rotate_2d(dir, angle * TORAD);
                        pos += dir * scale;
                        scale = scale * smaller;
                        path = floor(path / 2.);
                    }
                    let size = vec2(0.01, 0.01);
                    
                    let m = Math::rotate_2d(
                        vec2(1.0, self::width) * (geom.xy * nodesize - vec2(5.0*self::off, 0.5)),
                        atan(
                            dir.y,
                            dir.x
                        ) 
                    );  

                    let v = vec4(
                        m * scale.xy + pos.xy,
                        z_base + mix(last_z, z, geom.y),
                        1.
                    ); 
                    
                    return camera_projection * (camera_view * (view_transform * v));
                }
                
                fn pixel() -> vec4 {
                    let color = vec4(0.); 
                    if depth > max_depth{ 
                        color = mix(self::leaf_1,self::leaf_2,sin(0.01*in_path));
                    }
                    else{
                        color = self::color;//vec4(abs(right_input_pos.x),abs(right_input_pos.y), abs(right_input_pos.z), 1.0);//self::color;
                    }
                    return vec4(color.xyz * self::alpha, self::alpha); 
                }
                /*
                fn blend(a:vec4, b:vec4) -> vec4{
                    blend_premultiply_alpha(a,b);
                    blend_multiply(a,b);
                }*/
            }
        "#)
    }
    
    pub fn handle_tree_world(&mut self, _cx: &mut Cx, _event: &mut Event) {
        // lets see.
        
    } 
    
    pub fn draw_tree_world(&mut self, cx: &mut Cx) {
        
        self.sky_box.draw_sky_box(cx);
        
        let mut many = cx.begin_many_instances(live_shader!(cx, self::shader), 2);
        
        let max_depth = match cx.gpu_info.performance{
            GpuPerformance::Tier1=>10.0,
            GpuPerformance::Tier2=>11.0,
            GpuPerformance::Tier3=>12.0,
            GpuPerformance::Tier4=>16.0,
            GpuPerformance::Tier5=>18.0,
        };  

        fn recur(many: &mut ManyInstances, path: f32, depth: f32, max_depth: f32) {
            let data = [path, depth];
            many.instances.extend_from_slice(&data);
            if depth > max_depth {return}
            recur(many, path, depth + 1.0, max_depth);
            recur(many, path + (2.0f32).powf(depth), depth + 1.0, max_depth);
        }  
        recur(&mut many, 0., 0., max_depth);

        self.area = cx.end_many_instances(many);
        // write the uniform on the area
        write_draw_input!(cx, self.area, self::shader::max_depth, max_depth);
    }
}