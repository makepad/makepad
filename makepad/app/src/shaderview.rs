use makepad_render::*;

// Shader code itself

fn shader() -> ShaderGen {Quad::def_quad_shader().compose(shader!{"
    const EPSILON: float = 1E-3;
    const T_MAX: float = 10.0;
    
    struct Sdf {
        rotation: mat3,
        stack_0: float,
        stack_1: float
    }
   
    impl Sdf {
        fn new() -> Sdf {
            let sdf: Sdf;
            sdf.rotation = mat3(1.0);
            sdf.stack_0 = 0.0;
            sdf.stack_1 = 0.0;
            return sdf;
        }
        
        fn cube(inout self, p: vec3) {
            p = transpose_mat3(self.rotation) * p;
            let q = abs(p) - 0.5;
            self.stack_0 = self.stack_1;
            self.stack_1 = min(max(q.x, max(q.y, q.z)), 0.0) + length(max(q, 0.0));
        }
        
        /*
        fn cylinder(inout self, p: vec3) {
            p = transpose_mat3(self.rotation) * p;
            vec2 d = abs(vec2(length(p.xz),p.y)) - vec2(h,r);
            return min(max(d.x,d.y),0.0) + length(max(d,0.0));
        }
        */
        
        fn difference(inout self) {
            self.stack_1 = max(-self.stack_0, self.stack_1);
        }
        
        fn intersection(inout self) {
            self.stack_1 = max(self.stack_0, self.stack_1);
        }
        
        fn rotate(inout self, axis: vec3, angle: float) {
            let u = normalize(axis);
            let s = sin(angle);
            let c = cos(angle);
            self.rotation = mat3(
                c + u.x * u.x * (1.0 - c),
                u.y * u.x * (1.0 - c) + u.z * s,
                u.z * u.x * (1.0 - c) - u.y * s,
                u.x * u.y * (1.0 - c) - u.z * s,
                c + u.y * u.y * (1.0 - c),
                u.z * u.y * (1.0 - c) + u.x * s,
                u.x * u.z * (1.0 - c) + u.y * s,
                u.y * u.z * (1.0 - c) - u.x * s,
                c + u.z * u.z * (1.0 - c)
            ) * self.rotation;
        }
        
        fn sphere(inout self, p: vec3) {
            p = transpose_mat3(self.rotation) * p;
            self.stack_0 = self.stack_1;
            self.stack_1 = length(p) - 0.5;
        }
        
        fn union(inout self) {
            self.stack_1 = min(self.stack_0, self.stack_1);
        }
        
        fn finish(self) -> float {
            return self.stack_1;
        }
    }

    fn transpose_mat3(m: mat3) -> mat3 {
        return mat3(
            m[0][0], m[1][0], m[2][0],
            m[0][1], m[1][1], m[2][1],
            m[0][2], m[1][2], m[2][2]
        );
    }
    
    fn sdf(p: vec3) -> float {
        let sdf = Sdf::new();
        sdf.rotate(vec3(1.0, 1.0, 1.0), 0.01 * frame);
        sdf.cube(p);
        return sdf.finish();    
    }
    
    fn estimate_normal(p: vec3) -> vec3 {
        return normalize(vec3(
            sdf(vec3(p.x + EPSILON, p.y, p.z)) - sdf(vec3(p.x - EPSILON, p.y, p.z)),
            sdf(vec3(p.x, p.y + EPSILON, p.z)) - sdf(vec3(p.x, p.y - EPSILON, p.z)),
            sdf(vec3(p.x, p.y, p.z + EPSILON)) - sdf(vec3(p.x, p.y, p.z - EPSILON))
        ));
    }
    
    fn march_ray(p: vec3, v: vec3, t_min: float, t_max: float) -> float {
        let t = t_min;
        for i from 0 to 100 {
            let d = sdf(p + t * v);
            if d <= EPSILON {
                return t;
            }
            t += d;
            if t >= t_max {
                break;   
            }
        }
        return t_max;
    }
    
    fn pixel() -> vec4 {
        let p = vec3(2.0 * pos - 1.0, 2.0);
        let v = vec3(0.0, 0.0, -1.0);
        let t = march_ray(p, v, 0.0, T_MAX);
        if t < T_MAX {
            let n = estimate_normal(p + t * v);
            return vec4((n + 1.0) / 2.0, 1.0);
        } else {
            return vec4(0.0);
        }
    }
"})}

// Makepad UI structure to render shader

#[derive(Clone)]
pub struct ShaderView {
    quad: Quad,
    area: Area,
    finger_hover: Vec2,
    finger_move: Vec2,
    finger_down: f32,
    frame: f32
}

impl ShaderView {
    pub fn bg() -> ShaderId {uid!()}
    pub fn finger_hover() -> Vec2Id {uid!()}
    pub fn finger_move() -> Vec2Id {uid!()}
    pub fn finger_down() -> FloatId {uid!()}
    pub fn frame() -> FloatId {uid!()}
    pub fn new(cx: &mut Cx) -> Self {
        
        Self::bg().set(cx, shader().compose(shader!{"
            instance finger_hover: ShaderView::finger_hover();
            instance finger_move: ShaderView::finger_move();
            instance finger_down: ShaderView::finger_down();
            instance frame: ShaderView::frame();
        "}));
        
        Self {
            quad: Quad::new(cx),
            area: Area::default(),
            finger_hover: Vec2::default(),
            finger_move: Vec2::default(),
            finger_down: 0.0,
            frame: 0.0
        }
    }
    
    pub fn handle_shader_view(&mut self, cx: &mut Cx, event: &mut Event) {
        match event.hits(cx, self.area, HitOpt::default()) {
            Event::FingerMove(fm) => {
                self.finger_move = fm.rel;
                cx.redraw_child_area(self.area);
            },
            Event::FingerHover(fm) => {
                self.finger_hover = fm.rel;
                cx.redraw_child_area(self.area);
            },
            Event::FingerDown(_fd) => {
                self.finger_down = 1.0;
                cx.redraw_child_area(self.area);
            },
            Event::FingerUp(_fu) => {
                self.finger_down = 0.0;
                cx.redraw_child_area(self.area);
            },
            _ => ()
        }
    }
    
    pub fn draw_shader_view(&mut self, cx: &mut Cx) {
        self.quad.shader = Self::bg().get(cx);
        let k = self.quad.draw_quad_abs(cx, cx.get_turtle_rect());
        k.push_vec2(cx, self.finger_hover);
        k.push_vec2(cx, self.finger_move);
        k.push_float(cx, self.finger_down);
        k.push_float(cx, self.frame);
        self.frame += 1.0;
        self.area = cx.update_area_refs(self.area, k.into());
        cx.redraw_child_area(self.area);
    }
}

