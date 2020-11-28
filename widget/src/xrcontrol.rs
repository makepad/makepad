
use makepad_render::*;
use makepad_microserde::*;
use std::collections::{HashMap, BTreeMap};

#[derive(Clone, Eq, Copy, Ord, PartialOrd, PartialEq, Hash, Default)]
pub struct XRUserId(pub u32);

#[derive(Clone, Default, Debug, SerBin, DeBin)]
pub struct XRChannelUser {
    head_transform: Transform,
    left_input: XRInput,
    right_input: XRInput,
    window_mat: Mat4,
}

#[derive(Clone, Default)]
pub struct XRChannel {
    pub self_id: XRUserId,
    pub self_user: XRChannelUser,
    pub users: HashMap<XRUserId, XRChannelUser>,
    pub last_times: HashMap<XRUserId, f64>
}

#[derive(Clone)]
pub struct XRCursor {
    pub quad: DrawQuad,
    pub _pt: Vec2
}

impl XRCursor {
    fn new(cx: &mut Cx) -> Self {
        Self {
            _pt: Vec2::default(),
            quad: DrawQuad::new(cx, live_shader!(cx, self::shader_cursor))
                .with_draw_depth(3.0)
                .with_rect_size(Vec2::all(10.)),
        }
    }
    
    fn set_pt(&mut self, cx: &mut Cx, pt: Vec2) {
        self._pt = pt;
        let pt = pt - 0.5 * self.quad.rect_size.x;
        self.quad.set_rect_pos(cx, pt);
    }
    
    fn draw_cursor(&mut self, cx: &mut Cx) {
        self.quad.draw_quad(cx);
    }
}

#[derive(Clone)]
pub enum XRAvatarState {
    Joining(f32),
    Leaving(f32),
    Gone,
    Present
}

impl XRAvatarState {
    fn gone(&self) -> bool {
        match self {
            Self::Gone => true,
            _ => false
        }
    }
    
    fn get_space(&self) -> f32 {
        match self {
            Self::Joining(v) => 1.0 - *v,
            Self::Leaving(v) => *v,
            Self::Gone => 0.0,
            Self::Present => 1.0
        }
    }
    
    fn tick(&mut self) {
        match self {
            Self::Joining(f) => {
                if *f < 0.001 {
                    *self = Self::Present;
                }
                else {
                    *self = Self::Joining(*f * 0.99);
                }
            }
            Self::Leaving(f) => {
                if *f < 0.001 {
                    *self = Self::Gone;
                }
                else {
                    *self = Self::Leaving(*f * 0.9);
                }
            },
            _ => ()
        }
    }
    
    fn leave(&mut self) {
        match self {
            Self::Present => {
                *self = Self::Leaving(1.0);
            }
            Self::Joining(f) => {
                *self = Self::Leaving(1.0 - *f);
            }
            _ => ()
        }
    }
    
    fn join(&mut self) {
        match self {
            Self::Gone => {
                *self = Self::Joining(1.0);
            }
            Self::Leaving(f) => {
                *self = Self::Joining(1.0 - *f);
            }
            _ => ()
        }
    }
}

#[derive(Clone)]
pub struct XRAvatar {
    state: XRAvatarState,
    left_hand: DrawCube,
    right_hand: DrawCube,
    head: DrawCube,
    angle: f32,
    ui: DrawCube,
    last_user: Option<XRChannelUser>
}

impl XRAvatar {
    fn new(cx: &mut Cx) -> Self {
        let ds = default_shader!();
        
        let hand_size = Vec3 {x: 0.02, y: 0.02, z: 0.12};
        
        Self {
            state: XRAvatarState::Joining(1.0),

            left_hand: DrawCube::new(cx, ds)
                .with_cube_size(hand_size),

            right_hand: DrawCube::new(cx, ds)
                .with_cube_size(hand_size),

            last_user: None,
            angle: 180.0,

            head: DrawCube::new(cx, ds)
                .with_cube_size(Vec3 {x: 0.20, y: 0.08, z: 0.10}),

            ui: DrawCube::new(cx, ds),
        }
    }
    
    fn update_avatar(&mut self, cx: &mut Cx, user: Option<&XRChannelUser>, ui_rect: Rect) {
        
        let personal_mat = Mat4::mul(
            &Mat4::scaled_translation(self.state.get_space(), 0.0, 0.0, 0.0),
            &Mat4::txyz_s_ry_rx_txyz(
                Vec3 {x: 0.0, y: 0.0, z: 1.5},
                1.0,
                self.angle,
                0.0,
                Vec3 {x: 0.0, y: 0.0, z: -1.5},
            )
        );
        
        let user = if let Some(xe) = user {
            self.last_user = Some(xe.clone());
            xe
        } else {
            if let Some(xe) = &self.last_user {xe} else {return}
        };

        self.left_hand.set_transform(cx, Mat4::mul(&user.left_input.ray.to_mat4(), &personal_mat));
        self.right_hand.set_transform(cx, Mat4::mul(&user.right_input.ray.to_mat4(), &personal_mat));
        self.head.set_transform(cx, Mat4::mul(&user.head_transform.to_mat4(), &personal_mat));

        self.ui.set_transform(cx, Mat4::mul(&user.window_mat, &personal_mat));
        self.ui.set_cube_pos(cx, (ui_rect.pos + 0.5 * ui_rect.size).to_vec3() );
        self.ui.set_cube_size(cx, Vec3 {x: ui_rect.size.x, y: ui_rect.size.y, z: 25.0});
    }
    
    fn draw_avatar(&mut self, cx: &mut Cx) {
        self.left_hand.draw_cube(cx);
        self.right_hand.draw_cube(cx);
        self.head.draw_cube(cx);
        self.ui.draw_cube(cx);
    }
}

#[derive(Clone)]
pub struct XRControl {
    pub cursor_view: View,
    pub space_view: View,
    pub last_xr_update: Option<XRUpdateEvent>,
    
    pub xr_avatars: BTreeMap<XRUserId, XRAvatar>,
    
    pub left_hand: DrawCube,
    pub right_hand: DrawCube,
    pub left_cursor: XRCursor,
    pub right_cursor: XRCursor,
    pub smooth_window: Option<Transform>,
    pub window_mat: Option<Mat4>,
}

pub enum XRControlEvent {
    None
}

impl XRControl {
    pub fn new(cx: &mut Cx) -> Self {
        let ds = default_shader!();
        let hand_size = Vec3 {x: 0.02, y: 0.02, z: 0.12};
        Self {
            space_view: View::new(),
            cursor_view: View::new(),
            last_xr_update: None,
            left_hand: DrawCube::new(cx, ds)
                .with_cube_size(hand_size),
            right_hand: DrawCube::new(cx, ds)
                .with_cube_size(hand_size),
            left_cursor: XRCursor::new(cx),
            right_cursor: XRCursor::new(cx),
            xr_avatars: BTreeMap::new(),
            smooth_window: None,
            window_mat: None,
        }
    }
    
    fn get_window_matrix(view_rect: Rect, align: Vec2, translate: Vec3) -> Mat4 {
        Mat4::txyz_s_ry_rx_txyz(
            Vec3 {x: -view_rect.size.x * align.x, y: -view_rect.size.y * align.y, z: 0.0},
            -0.0005,
            -180.0,
            -30.0,
            // this is the position. lets make it 0
            translate
            //Vec3 {x: -0.20, y: -0.45, z: -0.3},
        )
    }
    
    pub fn style(cx: &mut Cx) {
        // lets define the shader
        live_body!(cx, r#"
            self::shader_hand: Shader {
                use makepad_render::drawcube::shader::*;
            }
            
            self::shader_cursor: Shader {
                use makepad_render::drawquad::shader::*;
                fn pixel() -> vec4 {
                    let df = Df::viewport(pos * rect_size);
                    df.circle(0.5 * rect_size.x, 0.5 * rect_size.y, 0.5 * rect_size.x);
                    return df.fill(#f);
                }
            }
        "#)
    }
    
    pub fn process_avatar_state(&mut self, cx: &mut Cx, xr_channel: &XRChannel, ui_rect: Rect) {
        
        // compute ordered circle
        let mut circle = Vec::new();
        let mut insert = false;
        for (id, _) in &mut self.xr_avatars {
            if *id == xr_channel.self_id {
                insert = true
            }
            else if insert {
                circle.push(*id);
            }
        }
        for (id, _) in &mut self.xr_avatars {
            if *id == xr_channel.self_id {
                break
            }
            circle.push(*id);
        }
        
        // compute space circle takes up
        let mut total_space = 1.0;
        for id in &circle {
            if let Some(xa) = self.xr_avatars.get(id) {
                total_space += xa.state.get_space();
            }
        }
        
        let mut angle = 0.0;
        for id in &circle {
            if let Some(xa) = self.xr_avatars.get_mut(id) {
                angle += (360.0 / total_space) * xa.state.get_space();
                xa.angle = angle;
            }
        }
        
        // ok lets update the states
        for (id, _xe) in &xr_channel.users {
            if let Some(xa) = self.xr_avatars.get_mut(id) {
                xa.state.join();
            }
            else {
                self.xr_avatars.insert(*id, XRAvatar::new(cx));
                self.space_view.redraw_view(cx);
            }
        }
        
        let mut remove = Vec::new();
        for (id, xa) in &mut self.xr_avatars {
            
            xa.update_avatar(cx, xr_channel.users.get(id), ui_rect);
            xa.state.tick();
            
            if xr_channel.users.get(id).is_none() {
                xa.state.leave();
            }
            if xa.state.gone() {
                remove.push(*id);
                self.space_view.redraw_view(cx);
            }
        }
        for id in remove {
            self.xr_avatars.remove(&id);
            self.space_view.redraw_view(cx);
        }
    }
    
    pub fn handle_xr_control(
        &mut self,
        cx: &mut Cx,
        xr_event: &XRUpdateEvent,
        xr_channel: &mut XRChannel,
        window_view: &View
    ) -> Vec<Event> {
        
        // lets send our avatar over the socket
        let view_rect = window_view.get_rect(cx);
        // lets set the left_input matrix
        self.left_hand.set_transform(cx, xr_event.left_input.ray.to_mat4());
        self.right_hand.set_transform(cx, xr_event.right_input.ray.to_mat4());
        
        if xr_event.left_input.buttons[1].pressed {
            // if the distance between smooth and left is small, smooth it, otherwise set it
            if let Some(smooth_window) = &mut self.smooth_window {
                *smooth_window = Transform::from_lerp(*smooth_window, xr_event.left_input.ray, 0.2);
            }
            else {
                self.smooth_window = Some(xr_event.left_input.ray);
            }
            self.window_mat = Some(Mat4::mul(
                &Self::get_window_matrix(
                    view_rect,
                    Vec2 {x: 0.25, y: 0.6},
                    Vec3 {x: 0.0, y: 0.0, z: -0.1}
                ),
                &self.smooth_window.unwrap().to_mat4(),
            ));
        }
        else if xr_event.right_input.buttons[1].pressed {
            // lets calculate the angle
            if let Some(smooth_window) = &mut self.smooth_window {
                *smooth_window = Transform::from_lerp(*smooth_window, xr_event.right_input.ray, 0.2);
            }
            else {
                self.smooth_window = Some(xr_event.right_input.ray);
            }
            
            self.window_mat = Some(Mat4::mul(
                &Self::get_window_matrix(
                    view_rect,
                    Vec2 {x: 0.75, y: 0.6},
                    Vec3 {x: 0.0, y: 0.0, z: -0.1}
                ),
                &self.smooth_window.unwrap().to_mat4(),
            ));
        }
        else if self.window_mat.is_none() {
            self.window_mat = Some(Self::get_window_matrix(
                view_rect,
                Vec2 {x: 0.0, y: 1.0},
                Vec3 {x: -0.20, y: 0.75, z: -0.3}
            ));
        }
        
        // we do a scale
        if xr_event.left_input.buttons[1].pressed && xr_event.right_input.buttons[1].pressed {
            // check if last had both, ifnot we mark beginning of scaling
            
        }
        
        let window_mat = self.window_mat.unwrap();
        let inv_window_mat = window_mat.invert();
        window_view.set_view_transform(cx, &window_mat);
        
        xr_channel.self_user.left_input = xr_event.left_input.clone();
        xr_channel.self_user.right_input = xr_event.right_input.clone();
        xr_channel.self_user.head_transform = xr_event.head_transform.clone();
        xr_channel.self_user.window_mat = window_mat;
        
        // lets make a test with just us.
        self.process_avatar_state(cx, xr_channel, view_rect);
        
        self.last_xr_update = Some(xr_event.clone());
        
        fn get_intersect_pt(window_plane: &Plane, inv_window_mat: &Mat4, ray_mat: &Mat4) -> Vec2 {
            let origin = inv_window_mat.transform_vec4(ray_mat.transform_vec4(Vec4 {x: 0., y: 0., z: 0., w: 1.0}));
            let vector = inv_window_mat.transform_vec4(ray_mat.transform_vec4(Vec4 {x: 0., y: 0., z: 1., w: 1.0}));
            window_plane.intersect_line(origin.to_vec3(), vector.to_vec3()).to_vec2()
        }
        // we now simply need to intersect with the plane view_rect.w, view_rect.h, 0.
        let window_plane = Plane::from_points(
            Vec3 {x: 0., y: 0., z: 0.},
            Vec3 {x: view_rect.size.x, y: 0., z: 0.},
            Vec3 {x: 0., y: view_rect.size.y, z: 0.}
        );
        
        self.left_cursor.set_pt(cx, get_intersect_pt(&window_plane, &inv_window_mat, self.left_hand.get_transform()));
        self.right_cursor.set_pt(cx, get_intersect_pt(&window_plane, &inv_window_mat, self.right_hand.get_transform()));
        
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
                    input_type: FingerInputType::XR,
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
                        input_type: FingerInputType::XR,
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
                        input_type: FingerInputType::XR,
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
                    input_type: FingerInputType::XR,
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
        do_input_event(&mut events, cx, 0, self.left_cursor._pt, xr_event.time, &xr_event.left_input, &xr_event.last_left_input);
        do_input_event(&mut events, cx, 1, self.right_cursor._pt, xr_event.time, &xr_event.right_input, &xr_event.last_right_input);
        events
    }
    
    pub fn draw_xr_control(&mut self, cx: &mut Cx) {
        
        // THIS HAS A VERY STRANGE BUG. if i reverse these, the dots are broken on wasm+quest
        if self.space_view.begin_view(cx, Layout::abs_origin_zero()).is_ok() {
            self.space_view.lock_view_transform(cx, &Mat4::identity());
            
            self.left_hand.draw_cube(cx);
            self.right_hand.draw_cube(cx);
            
            for (_id, avatar) in &mut self.xr_avatars {
                avatar.draw_avatar(cx);
            }
            
            self.space_view.end_view(cx);
        }
        
        if self.cursor_view.begin_view(cx, Layout::abs_origin_zero()).is_ok() {
            self.left_cursor.draw_cursor(cx);
            self.right_cursor.draw_cursor(cx);
            self.cursor_view.end_view(cx);
        }
    }
} 
