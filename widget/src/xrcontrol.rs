
use makepad_render::*;

#[derive(Clone)]
pub struct XRControl {
    pub cursor_view: View,
    pub ray_view: View,
    pub ray_cube: Cube,
    pub ray_cursor: Quad,
    pub animator: Animator,
    pub last_xr_update: Option<XRUpdateEvent>,
    pub cursor_size: f32,
    pub _left_ray_area: Area,
    pub _right_ray_area: Area,
    pub _left_cursor_area: Area,
    pub _right_cursor_area: Area,
    pub _left_cursor_pt: Vec2,
    pub _right_cursor_pt: Vec2,
    pub _left_ray_mat: Mat4,
    pub _right_ray_mat: Mat4
}

pub enum XRControlEvent {
    None
}

impl XRControl {
    pub fn new(cx: &mut Cx) -> Self {
        Self {
            ray_view: View::new(cx),
            cursor_view: View::new(cx),
            ray_cube: Cube::new(cx),
            ray_cursor: Quad {
                z: 3.0,
                ..Quad::new(cx)
            },
            animator: Animator::default(),
            last_xr_update: None,
            cursor_size: 10.,
            _left_ray_area: Area::Empty,
            _right_ray_area: Area::Empty,
            _left_cursor_area: Area::Empty,
            _right_cursor_area: Area::Empty,
            _left_cursor_pt: Vec2::default(),
            _right_cursor_pt: Vec2::default(),
            _left_ray_mat: Mat4::identity(),
            _right_ray_mat: Mat4::identity()
        }
    }
    
    pub fn style(cx: &mut Cx) {
        // lets define the shader
        live!(cx, r#"
            self::shader_ray_cube: Shader {
                use makepad_render::cube::shader::*;
            }
            
            self::shader_ray_cursor: Shader {
                use makepad_render::quad::shader::*;
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * vec2(w, h));
                    df.circle(0.5 * w, 0.5 * h, 0.5 * w);
                    return df.fill(#f);
                }
            }
        "#)
    }
    
    pub fn handle_xr_control(&mut self, cx: &mut Cx, xr_event: &XRUpdateEvent, window_view: &View) -> Vec<Event> {
        
        let view_rect = window_view.get_rect(cx);
        
        let window_mat = Mat4::rotate_tsrt(
            Vec3 {x: 0., y: -view_rect.h, z: 0.0},
            -0.0005,
            Vec3 {x: -0.0, y: -180.0, z: 0.0},
            Vec3 {x: -0.20, y: -0.15, z: -0.3},
        );
        
        window_view.set_view_transform(cx, &window_mat);
        self.ray_view.set_view_transform(cx, &Mat4::identity());
        
        // lets set the left_input matrix
        self._left_ray_mat = Mat4::from_transform(xr_event.left_input.ray); // Mat4::from_mul(&Mat4::rotate(45.0, 0.0, 0.0), &Mat4::from_transform(xr_event.left_input.grip));
        self._right_ray_mat = Mat4::from_transform(xr_event.right_input.ray); //Mat4::from_mul(&Mat4::rotate(45.0, 0.0, 0.0), &Mat4::from_transform(xr_event.right_input.grip));
        
        self.ray_view.set_view_transform(cx, &Mat4::identity());
        self.last_xr_update = Some(xr_event.clone());
        
        self._left_ray_area.write_mat4(cx, live_id!(makepad_render::cube::shader::transform), &self._left_ray_mat);
        self._right_ray_area.write_mat4(cx, live_id!(makepad_render::cube::shader::transform), &self._right_ray_mat);
        
        // we have 2 points, 0,0,0 and 0,0,1? pointing straight back
        // then, we transform those with our left input ray
        let inv_window_mat = window_mat.invert();
        let right_origin = inv_window_mat.transform_vec4(self._right_ray_mat.transform_vec4(Vec4 {x: 0., y: 0., z: 0., w: 1.0}));
        let right_vector = inv_window_mat.transform_vec4(self._right_ray_mat.transform_vec4(Vec4 {x: 0., y: 0., z: 1., w: 1.0}));
        let left_origin = inv_window_mat.transform_vec4(self._left_ray_mat.transform_vec4(Vec4 {x: 0., y: 0., z: 0., w: 1.0}));
        let left_vector = inv_window_mat.transform_vec4(self._left_ray_mat.transform_vec4(Vec4 {x: 0., y: 0., z: 1., w: 1.0}));
        // now we have 2 points that make a line
        // we now simply need to intersect with the plane view_rect.w, view_rect.h, 0.
        let window_plane = Plane::from_points(
            Vec3 {x: 0., y: 0., z: 0.},
            Vec3 {x: view_rect.w, y: 0., z: 0.},
            Vec3 {x: 0., y: view_rect.h, z: 0.}
        );
        self._right_cursor_pt = window_plane.intersect_line(right_origin.to_vec3(), right_vector.to_vec3()).to_vec2();
        self._left_cursor_pt = window_plane.intersect_line(left_origin.to_vec3(), left_vector.to_vec3()).to_vec2();
        
        self._right_cursor_area.write_float(cx, live_id!(makepad_render::quad::shader::x), self._right_cursor_pt.x - 0.5 * self.cursor_size);
        self._right_cursor_area.write_float(cx, live_id!(makepad_render::quad::shader::y), self._right_cursor_pt.y - 0.5 * self.cursor_size);
        self._left_cursor_area.write_float(cx, live_id!(makepad_render::quad::shader::x), self._left_cursor_pt.x - 0.5 * self.cursor_size);
        self._left_cursor_area.write_float(cx, live_id!(makepad_render::quad::shader::y), self._left_cursor_pt.y - 0.5 * self.cursor_size);
        
        let mut events = Vec::new();
        
        fn do_input_event(events: &mut Vec<Event>, cx: &mut Cx, digit: usize, pt: Vec2, time: f64, input: &XRInput, last_input: &XRInput) {
            fn axis_not_zero(axis: f32) -> bool {
                axis < -0.01 || axis>0.01
            }
            if axis_not_zero(input.axes[2]) || axis_not_zero(input.axes[3]) {
                events.push(Event::FingerScroll(FingerScrollEvent {
                    window_id: 0,
                    digit: digit,
                    abs: pt,
                    rel: pt,
                    rect: Rect::default(),
                    handled_x: false,
                    handled_y: false,
                    scroll: Vec2 {x: input.axes[2] * 15.0, y: input.axes[3] * 15.0},
                    is_wheel: true,
                    modifiers: KeyModifiers::default(),
                    time: time
                }));
            }
            
            if input.buttons[0].pressed != last_input.buttons[0].pressed {
                // we have finger up or down
                if input.buttons[0].pressed {
                    events.push(Event::FingerDown(FingerDownEvent {
                        digit: digit,
                        window_id: 0,
                        tap_count: 0,
                        abs: pt,
                        rel: pt,
                        handled: false,
                        is_touch: true,
                        rect: Rect::default(),
                        modifiers: KeyModifiers::default(),
                        time: time
                    }));
                }
                else {
                    events.push(Event::FingerUp(FingerUpEvent {
                        digit: digit,
                        window_id: 0,
                        abs: pt,
                        rel: pt,
                        is_over: false,
                        is_touch: true,
                        rect: Rect::default(),
                        abs_start: Vec2::default(),
                        rel_start: Vec2::default(),
                        modifiers: KeyModifiers::default(),
                        time: time
                    }));
                }
                
            }
            else if input.buttons[0].pressed { // we have move
                events.push(Event::FingerMove(FingerMoveEvent {
                    digit: digit,
                    window_id: 0,
                    abs: pt,
                    rel: pt,
                    rect: Rect::default(),
                    abs_start: Vec2::default(),
                    rel_start: Vec2::default(),
                    is_over: false,
                    is_touch: true,
                    modifiers: KeyModifiers::default(),
                    time: time
                }));
            }
            else {
                cx.fingers[digit].over_last = Area::Empty;
                events.push(Event::FingerHover(FingerHoverEvent {
                    digit: digit,
                    any_down: false,
                    window_id: 0,
                    abs: pt,
                    rel: pt,
                    rect: Rect::default(),
                    handled: false,
                    hover_state: HoverState::Over,
                    modifiers: KeyModifiers::default(),
                    time: time
                }));
            }
        }
        do_input_event(&mut events, cx, 0, self._left_cursor_pt, xr_event.time, &xr_event.left_input, &xr_event.last_left_input);
        do_input_event(&mut events, cx, 1, self._right_cursor_pt, xr_event.time, &xr_event.right_input, &xr_event.last_right_input);
        events
    }
    
    pub fn draw_xr_control(&mut self, cx: &mut Cx) {
        self.ray_cube.shader = live_shader!(cx, self::shader_ray_cube);
        self.ray_cursor.shader = live_shader!(cx, self::shader_ray_cursor);
        
        if self.cursor_view.begin_view(cx, Layout::abs_origin_zero()).is_ok() {
            self._left_cursor_area = self.ray_cursor.draw_quad_rel(cx, Rect {
                x: self._left_cursor_pt.x - 0.5 * self.cursor_size,
                y: self._left_cursor_pt.y - 0.5 * self.cursor_size,
                w: self.cursor_size,
                h: self.cursor_size
            }).into();
            self._right_cursor_area = self.ray_cursor.draw_quad_rel(cx, Rect {
                x: self._right_cursor_pt.x - 0.5 * self.cursor_size,
                y: self._right_cursor_pt.y - 0.5 * self.cursor_size,
                w: self.cursor_size,
                h: self.cursor_size
            }).into();
            self.cursor_view.end_view(cx);
        }
        
        // if let Some(xr_event) = &self.last_xr_update{
        if self.ray_view.begin_view(cx, Layout::abs_origin_zero()).is_ok() {
            let ray_size = Vec3 {x: 0.02, y: 0.02, z: 0.12};
            let ray_pos = Vec3 {x: 0., y: 0., z: 0.0};
            
            self._left_ray_area = self.ray_cube.draw_cube(cx, ray_size, ray_pos, &self._left_ray_mat).into();
            self._right_ray_area = self.ray_cube.draw_cube(cx, ray_size, ray_pos, &self._right_ray_mat).into();
            
            self.ray_view.end_view(cx);
        }
        
    }
}
