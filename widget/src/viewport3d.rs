// a bunch o buttons to select the world
use makepad_render::*;

#[derive(Clone)]
pub struct Viewport3D {
    pub pass: Pass,
    pub clear_color: Vec4,
    pub color_texture: Texture,
    pub depth_texture: Texture,
    pub view_2d: View,
    pub view_3d: View,
    pub measured_size: Vec2,
    pub camera_center: Vec3,
    pub camera_pos: Vec3,
    pub camera_rot: Vec3,
    pub camera_start: Option<(Vec3, Vec3)>,
    pub image: DrawImage
}

impl Viewport3D {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            pass: Pass::default(),
            camera_center: Vec3 {x: 0.0, y: 0.0, z: 1.1 + 1.5},
            camera_pos: Vec3 {x: 0.0, y: -0.5, z: -1.1},
            camera_rot: Vec3 {x: 0.0, y: 0.0, z: 0.0},
            camera_start: None,
            clear_color: Vec4::color("000"),
            color_texture: Texture::new(cx),
            depth_texture: Texture::new(cx),
            view_3d: View::new(),
            view_2d: View::new(),
            measured_size: Vec2::all(1.0),
            image: DrawImage::new(cx, default_shader!())
        }
    }
    
    pub fn style(cx: &mut Cx) {
        live_body!(cx, r#"
            self::pos: vec3(0., 0.0, -1.1);
        "#);
    }
     
    pub fn handle_viewport_2d(&mut self, cx: &mut Cx, event: &mut Event) {
        match event.hits(cx, self.view_2d.area(), HitOpt::default()) {
            Event::FingerHover(_fe) => {
                cx.set_hover_mouse_cursor(MouseCursor::Move);
            },
            Event::FingerDown(_fe) => { 
                
                cx.set_down_mouse_cursor(MouseCursor::Move);
                self.camera_start = Some((self.camera_pos, self.camera_rot));
            }, 
            Event::FingerUp(_fe) => {
            } 
            Event::FingerScroll(fe) => {
                self.camera_pos.z += fe.scroll.y / 300.0;
                self.camera_center.z = -self.camera_pos.z + 1.5;
                self.pass_set_matrix_mode(cx);
            }
            Event::FingerMove(fe) => {
                if let Some((_pos, rot)) = self.camera_start {
                    self.camera_rot = Vec3 {
                        x: rot.x + (fe.abs.y - fe.abs_start.y),
                        y: rot.y + (fe.abs.x - fe.abs_start.x),
                        z: rot.z
                    };
                    self.pass_set_matrix_mode(cx)
                }
            },
            _ => ()
        }
    }
    
    pub fn pass_set_matrix_mode(&mut self, cx: &mut Cx) {
        //self.pass.set_matrix_mode(cx, PassMatrixMode::Ortho);
        
        self.pass.set_matrix_mode(cx, PassMatrixMode::Projection {
            fov_y: 40.0,
            near: 0.1,
            far: 1000.0,
            cam: Mat4::txyz_s_ry_rx_txyz(
                self.camera_pos + self.camera_center,
                1.0,
                self.camera_rot.y,
                self.camera_rot.x,
                -self.camera_center,
            )
        });
    }
    
    pub fn begin_viewport_3d(&mut self, cx: &mut Cx) -> ViewRedraw {
        if !self.view_3d.view_will_redraw(cx) {
            return Err(())
        }
        
        self.pass.begin_pass(cx);
        self.pass.set_debug(cx, true);
        self.pass.set_size(cx, self.measured_size);
        self.pass_set_matrix_mode(cx);
        self.pass.add_color_texture(cx, self.color_texture, ClearColor::ClearWith(self.clear_color));
        self.pass.set_depth_texture(cx, self.depth_texture, ClearDepth::ClearWith(1.0));
        
        let _ = self.view_3d.begin_view(cx, Layout::default());
        
        Ok(())
    }
    
    pub fn end_viewport_3d(&mut self, cx: &mut Cx) {
        self.view_3d.end_view(cx);
        self.pass.end_pass(cx);
    }
    
    pub fn draw_viewport_2d(&mut self, cx: &mut Cx) {
        if self.view_2d.begin_view(cx, Layout::default()).is_err() {
            return
        };
        self.view_3d.redraw_view(cx);
        // blit the texture to a view rect
        self.measured_size = vec2(cx.get_width_total(), cx.get_height_total());
        self.image.texture = self.color_texture.into();
        self.image.draw_quad_rel(cx, Rect{pos:vec2(0.,0.), size:self.measured_size });
        
        self.view_2d.end_view(cx);
    }
}
