use makepad_render::*;

// Shader code itself

fn shader() -> ShaderGen {Quad::def_quad_shader().compose(shader!{"
    const EPSILON: float = 1E-3;
    const T_MAX: float = 10.0;
    
    fn identity() -> mat4 {
        return mat4(
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0
        );
    }
    
    fn rotation(axis: vec3, angle: float) -> mat4 {
        let u = normalize(axis);
        let s = sin(angle);
        let c = cos(angle);
        return mat4(
            c + u.x * u.x * (1.0 - c),
            u.y * u.x * (1.0 - c) + u.z * s,
            u.z * u.x * (1.0 - c) - u.y * s,
            0.0,
            u.x * u.y * (1.0 - c) - u.z * s,
            c + u.y * u.y * (1.0 - c),
            u.z * u.y * (1.0 - c) + u.x * s,
            0.0,
            u.x * u.z * (1.0 - c) + u.y * s,
            u.y * u.z * (1.0 - c) - u.x * s,
            c + u.z * u.z * (1.0 - c),
            0.0,
            0.0,
            0.0,
            0.0,
            1.0
        );
    }
    
    fn cube(p: vec3) -> float {
        let q = abs(p) - 0.4;
        return min(max(q.x, max(q.y, q.z)), 0.0) + length(max(q, 0.0));
    }
    
    fn cylinder_x(p: vec3) -> float {
        let d = abs(vec2(length(p.yz), p.x)) - vec2(0.25, 1.0);
        return min(max(d.x, d.y), 0.0) + length(max(d, 0.0));
    }
    
    fn cylinder_y(p: vec3) -> float {
        let d = abs(vec2(length(p.xz), p.y)) - vec2(0.25, 1.0);
        return min(max(d.x, d.y), 0.0) + length(max(d, 0.0));
    }
    
    fn cylinder_z(p: vec3) -> float {
        let d = abs(vec2(length(p.xy), p.z)) - vec2(0.25, 1.0);
        return min(max(d.x, d.y), 0.0) + length(max(d, 0.0));
    }
    
    fn difference(d1: float, d2: float) -> float {
        return max(d1, -d2);
    }
    
    fn intersection(d1: float, d2: float) -> float {
        return max(d1, d2);
    }
    
    fn sphere(p: vec3) -> float {
        return length(p) - 0.5;
    }
    
    fn union(d1: float, d2: float) -> float {
        return min(d1, d2);
    }
    
    fn sdf(p: vec3) -> float {
        return union(
            intersection(cube(p), sphere(p)),
            union(union(cylinder_x(p), cylinder_y(p)), cylinder_z(p))
        );
    }
    
    fn compute_color(p: vec3) -> vec3 {
        
    }
    
    fn estimate_normal(p: vec3) -> vec3 {
        return normalize(vec3(
            sdf(vec3(p.x + EPSILON, p.y, p.z)) - sdf(vec3(p.x - EPSILON, p.y, p.z)),
            sdf(vec3(p.x, p.y + EPSILON, p.z)) - sdf(vec3(p.x, p.y - EPSILON, p.z)),
            sdf(vec3(p.x, p.y, p.z + EPSILON)) - sdf(vec3(p.x, p.y, p.z - EPSILON))
        ));
    }
    
    fn march_ray(p0: vec3, v: vec3) -> float {
        let t = 0.0;
        for i from 0 to 100 {
            let d = sdf(p0 + t * v);
            if d <= EPSILON {
                return t;
            }
            t += d;
            if t >= T_MAX {
                break;
            }
        }
        return T_MAX;
    }
    
    fn pixel() -> vec4 {
        let p0 = vec3(2.0 * pos - 1.0, 2.0);
        let v = vec3(0.0, 0.0, -1.0);
        let m = identity() * rotation(vec3(1.0, 1.0, 1.0), 0.01 * frame);
        p0 = (m * vec4(p0, 1.0)).xyz;
        v = (m * vec4(v, 0.0)).xyz;
        let t = march_ray(p0, v);
        if t < T_MAX {
            let p = p0 + t * v;
            let n = estimate_normal(p);
            let c = color!(#FFFFFF);
            let dx = cylinder_x(p);
            if dx <= EPSILON {
                c = mix(c, color!(#FF0010), 0.5);
            }
            let dy = cylinder_y(p);
            if dy <= EPSILON {
                c = mix(c, color!(#00FF02), 0.5);
            }
            let dz = cylinder_z(p);
            if dz <= EPSILON {
                c = mix(c, color!(blue), 0.5);
            }
            let k = 0.1 + 0.5 * vec4(abs(dot(n, vec3(0.0, 0.0, -1.0))));
            return k * c;
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

