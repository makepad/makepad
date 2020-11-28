// a bunch o buttons to select the world
use makepad_render::*;
use crate::skybox::SkyBox;

#[derive(Clone)]
pub struct FieldWorld {
    pub view: View,
    pub area: Area,
    pub sky_box: SkyBox
}

impl FieldWorld {
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
            self::angle: 0.5;
            self::width: 0.3;
            self::alpha: 0.114;
            self::shader: Shader {
                
                use makepad_render::shader_std::prelude::*;
                use makepad_worlds::worldview::uniforms::*;
                
                default_geometry: makepad_render::shader_std::quad_2d;
                geometry geom: vec2;
                
                instance in_path: float;
                instance depth: float;
                instance axis: float;
                varying color: vec4;
                fn vertex() -> vec4 {
                    let pos = vec2(0.0, -0.5);
                    let scale = vec2(0.2, 0.2);
                    let dir = vec2(0.0, 0.8);
                    let smaller = vec2(.85, 0.85);
                    let path = in_path;
                    let nodesize = vec2(1.);
                    let z = 0.0;
                    let last_z = 0.0;
                    for i from 0 to 14 {
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
                        
                        dir = Math::rotate_2d(dir, angle * TORAD);
                        pos += dir * scale;
                        scale = scale * smaller;
                        path = floor(path / 2.);
                    }
                    let size = vec2(0.01, 0.01);
                    
                    let m = Math::rotate_2d(
                        vec2(1.0, self::width) * (geom.xy * nodesize - vec2(1.0, 0.5)),
                        atan(
                            dir.y,
                            dir.x
                        ) 
                    ); 

                    let v = vec4(
                        m * scale.xy + pos.xy,
                        -1.5+mix(last_z, z, geom.y),
                        1.
                    ); 
                    
                    return camera_projection * (camera_view * view_transform * v);
                }
                
                fn pixel() -> vec4 {
                    let color = vec4(0.);
                    if depth > 11.{
                        color = mix(self::leaf_1,self::leaf_2,sin(0.01*in_path));
                    }
                    else{
                        color = self::color;
                    }
                    return vec4(color.xyz * self::alpha, self::alpha); 
                }
            }
        "#)
    }
    
    pub fn handle_field_world(&mut self, _cx: &mut Cx, _event: &mut Event) {
        // lets see.
        
    }
    
    pub fn draw_field_world(&mut self, cx: &mut Cx) {
        self.sky_box.draw_sky_box(cx);
    }
}