use makepad_render::*;

#[derive(Clone)]
pub struct SkyBox {
    cube: DrawCube,
}

impl SkyBox {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            cube: DrawCube::new(cx, default_shader!())
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            self::sky_color: #0;
            self::edge_color: #1;
            self::floor_color: #8;
            
            self::size: vec3(200.0, 100.0, 200.0);
            self::pos: vec3(0.0, 50.0, 0.);
            
            self::shader_sky_box: Shader {
                use makepad_render::cube::shader::*;
                fn color_form_id() -> vec4 {
                    if geom_id>4.5 {
                        return #f00;
                    }
                    if geom_id>3.5 {
                        return #0f0;
                    }
                    if geom_id>2.5 {
                        return #00f;
                    }
                    if geom_id>1.5 {
                        return #0ff;
                    }
                    return #f0f;
                }
                varying t:float;
                fn vertex() -> vec4 {
                
                    let model_view = camera_view * view_transform * transform ;
                    return camera_projection * (model_view * vec4(
                        geom_pos.x * size.x + pos.x,
                        geom_pos.y * size.y + pos.y,
                        geom_pos.z * size.z + pos.z + draw_zbias,
                        1.
                    ));
                }
                
                fn pixel() -> vec4 { 
                    let x = geom_uv.x;
                    let y = geom_uv.y;
                    // walls
                    let sky = self::sky_color;
                    let edge = self::edge_color;
                    if geom_id>4.5 || geom_id > 3.5 || geom_id < 1.5 {
                        return mix(edge, sky, y);
                    }
                    // floor
                    if geom_id>2.5 {
                        let coord = geom_uv * 150.0;
                        let grid = abs(
                            fract(coord - 0.5) - 0.5
                        ) / (abs(dFdx(coord)) + abs(dFdy(coord)));
                        let line = min(grid.x, grid.y);
                        let grid2 = self::floor_color + 0.4 * vec4(vec3(1.0 - min(line, 1.0)), 1.0);
                        let uv2 = abs(2.0 * geom_uv - 1.0);
                        return mix(grid2, edge, min(max(uv2.x, uv2.y) + 0.7, 1.0));
                    }
                    return sky;
                }
            }
        "#)
    }
    
    pub fn draw_sky_box(&mut self, cx: &mut Cx) {
        self.cube.shader = live_shader!(cx, self::shader_sky_box);
        self.cube.cube_size = live_vec3!(cx, self::size);
        self.cube.cube_pos = live_vec3!(cx, self::pos);
        self.cube.draw_cube(cx);
    }
}
