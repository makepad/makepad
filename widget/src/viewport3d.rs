// a bunch o buttons to select the world
use makepad_render::*;

#[derive(Clone)]
pub struct Viewport3D {
    pub pass: Pass,
    pub clear_color: Color,
    pub color_texture: Texture,
    pub depth_texture: Texture,
    pub main_view: View,
    pub measured_size: Vec2,
    pub blit: Blit
}

impl Viewport3D {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            pass: Pass::default(),
            clear_color: Color::parse_hex_str("040").unwrap(),
            color_texture: Texture::new(cx),
            depth_texture: Texture::new(cx),
            main_view: View::new(cx),
            measured_size: Vec2::all(1.0),
            blit: Blit::new(cx)
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            self::pos: vec3(0.,0.0,-1.1);
        "#);
    }
    
    pub fn begin_viewport_3d(&mut self, cx: &mut Cx) -> ViewRedraw {
        if !self.main_view.view_will_redraw(cx) {
            return Err(())
        }
        self.pass.begin_pass(cx);
        self.pass.set_size(cx, self.measured_size);
        self.pass.set_matrix_mode(cx, PassMatrixMode::Projection {
            fov_y: 40.0,
            near: 1.0,
            far: 1000.0,
            cam:Mat4::rotate_tsrt(
                live_vec3!(cx, self::pos),
                1.0,
                Vec3 {x: 0.0, y: 0.0, z: 0.0},
                Vec3 {x: 0.0, y: 0.0, z: 0.0},
            )
        });
        self.pass.add_color_texture(cx, self.color_texture, ClearColor::ClearWith(self.clear_color));
        self.pass.set_depth_texture(cx, self.depth_texture, ClearDepth::ClearWith(1.0));
        
        let _ = self.main_view.begin_view(cx, Layout::default());

        Ok(())
    }
    
    pub fn end_viewport_3d(&mut self, cx: &mut Cx) {
        self.main_view.end_view(cx);
        self.pass.end_pass(cx);
    }
    
    pub fn draw_viewport_2d(&mut self, cx: &mut Cx) {
        // blit the texture to a view rect
        let i = self.blit.begin_blit_fill(cx, self.color_texture);
        self.blit.end_blit_fill(cx, &i);
        self.measured_size = Vec2{x:cx.get_width_total(), y: cx.get_height_total()};
    }
}
