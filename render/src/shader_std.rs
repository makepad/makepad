use crate::cx::*;
use crate::quad::Quad;
use crate::text::Text;
use crate::blit::Blit;
use crate::cube::Cube;

impl Cx{
    pub fn style(&mut self){
        define_shader_stdlib(self);
        Cube::style(self);
        Quad::style(self);
        Text::style(self);
        Blit::style(self);
    }
}

pub fn define_shader_stdlib(cx: &mut Cx) {
    
    let g = Geometry::from_geometry_gen(cx, GeometryGen::from_quad_2d(0.0,0.0,1.0,1.0));
    cx.live_styles.geometries.insert(live_id!(self::quad_2d), g);
    
    let g = Geometry::from_geometry_gen(cx, GeometryGen::from_cube_3d(1.0,1.0,1.0,1,1,1));
    cx.live_styles.geometries.insert(live_id!(self::cube_3d),g);
    
    //Text::style(cx);
    
    live!(cx, r#"
        
        self::geometry_3d: ShaderLib {
            geometry geom_pos: vec3;
            geometry geom_id: float;
            geometry geom_normal: vec3;
            geometry geom_uv: vec2;
        }
        
        self::prelude: ShaderLib {
            use self::uniforms::*;
            use self::lib::*;
        }
        
        self::uniforms: ShaderLib {
            uniform camera_projection: mat4 in pass;
            uniform camera_view: mat4 in pass;
            uniform camera_inv: mat4 in pass;
            uniform dpi_factor: float in pass;
            uniform dpi_dilate: float in pass;
            
            uniform view_transform: mat4 in view;
            
            uniform draw_clip: vec4 in draw;
            uniform draw_scroll: vec4 in draw;
            uniform draw_zbias: float in draw;
        }
        
        self::lib: ShaderLib {
            const PI: float = 3.141592653589793;
            const E: float = 2.718281828459045;
            const LN2: float = 0.6931471805599453;
            const LN10: float = 2.302585092994046;
            const LOG2E: float = 1.4426950408889634;
            const LOG10E: float = 0.4342944819032518;
            const SQRT1_2: float = 0.70710678118654757;
            const TORAD: float = 0.017453292519943295;
            const GOLDEN: float = 1.618033988749895;
            
            struct Df {
                pos: vec2,
                result: vec4,
                last_pos: vec2,
                start_pos: vec2,
                shape: float,
                clip: float,
                has_clip: float,
                old_shape: float,
                blur: float,
                aa: float,
                scale: float,
                field: float
            }
            
            impl Pal {
                fn iq(t: float, a: vec3, b: vec3, c: vec3, d: vec3) -> vec3 {
                    return a + b * cos(6.28318 * (c * t + d));
                }
                
                fn iq0(t: float) -> vec3 {
                    return mix(vec3(0., 0., 0.), vec3(1., 1., 1.), cos(t * PI) * 0.5 + 0.5);
                }
                
                fn iq1(t: float) -> vec3 {
                    return Pal::iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 1.), vec3(0., 0.33, 0.67));
                }
                
                fn iq2(t: float) -> vec3 {
                    return Pal::iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 1.), vec3(0., 0.1, 0.2));
                }
                
                fn iq3(t: float) -> vec3 {
                    return Pal::iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 1.), vec3(0.3, 0.2, 0.2));
                }
                
                fn iq4(t: float) -> vec3 {
                    return Pal::iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 0.5), vec3(0.8, 0.9, 0.3));
                }
                
                fn iq5(t: float) -> vec3 {
                    return Pal::iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 0.7, 0.4), vec3(0, 0.15, 0.20));
                }
                
                fn iq6(t: float) -> vec3 {
                    return Pal::iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(2., 1.0, 0.), vec3(0.5, 0.2, 0.25));
                }
                
                fn iq7(t: float) -> vec3 {
                    return Pal::iq(t, vec3(0.8, 0.5, 0.4), vec3(0.2, 0.4, 0.2), vec3(2., 1.0, 1.0), vec3(0., 0.25, 0.25));
                }
                
                fn hsv2rgb(c: vec4) -> vec4 { //http://gamedev.stackexchange.com/questions/59797/glsl-shader-change-hue-saturation-brightness
                    let K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
                    let p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
                    return vec4(c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y), c.w);
                }
                
                fn rgb2hsv(c: vec4) -> vec4 {
                    let K: vec4 = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
                    let p: vec4 = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
                    let q: vec4 = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
                    
                    let d: float = q.x - min(q.w, q.y);
                    let e: float = 1.0e-10;
                    return vec4(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x, c.w);
                }
            }
            
            impl Df {
                fn viewport(pos: vec2) -> Df {
                    
                    let df: Df;
                    df.pos = pos;
                    df.result = vec4(0.);
                    df.last_pos = vec2(0.);
                    df.start_pos = vec2(0.);
                    df.shape = 1e+20;
                    df.clip = -1e+20;
                    df.has_clip = 0.0;
                    df.old_shape = 1e+20;
                    df.blur = 0.00001;
                    df.aa = Df::antialias(pos);
                    df.scale = 1.0;
                    df.field = 0.0;
                    return df;
                }
                
                fn antialias(p: vec2) -> float {
                    return 1.0 / length(vec2(length(dFdx(p)), length(dFdy(p))));
                }
                
                fn translate(inout self, x: float, y: float) -> vec2 {
                    self.pos -= vec2(x, y);
                    return self.pos;
                }
                
                fn rotate(inout self, a: float, x: float, y: float) {
                    let ca = cos(-a);
                    let sa = sin(-a);
                    let p = self.pos - vec2(x, y);
                    self.pos = vec2(p.x * ca - p.y * sa, p.x * sa + p.y * ca) + vec2(x, y);
                }
                
                fn scale(inout self, f: float, x: float, y: float) {
                    self.scale *= f;
                    self.pos = (self.pos - vec2(x, y)) * f + vec2(x, y);
                }
                
                fn clear(inout self, color: vec4) {
                    self.result = vec4(color.rgb * color.a + self.result.rgb * (1.0 - color.a), color.a);
                }
                
                fn calc_blur(inout self, w: float) -> float {
                    let wa = clamp(-w * self.aa, 0.0, 1.0);
                    let wb = 1.0;
                    if self.blur > 0.001 {
                        wb = clamp(-w / self.blur, 0.0, 1.0);
                    }
                    return wa * wb;
                }
                
                fn fill_keep(inout self, color: vec4) -> vec4 {
                    let f = self.calc_blur(self.shape);
                    let source = vec4(color.rgb * color.a, color.a);
                    self.result = source * f + self.result * (1. - source.a * f);
                    if self.has_clip > 0.5 {
                        let f2 = 1.0 - self.calc_blur(-self.clip);
                        self.result = source * f2 + self.result * (1. - source.a * f2);
                    }
                    return self.result;
                }
                
                fn fill(inout self, color: vec4) -> vec4 {
                    self.fill_keep(color);
                    self.old_shape = self.shape = 1e+20;
                    self.clip = -1e+20;
                    self.has_clip = 0.;
                    return self.result;
                }
                
                fn stroke_keep(inout self, color: vec4, width: float) -> vec4 {
                    let f = self.calc_blur(abs(self.shape) - width / self.scale);
                    let source = vec4(color.rgb * color.a, color.a);
                    let dest = self.result;
                    self.result = source * f + dest * (1.0 - source.a * f);
                    return self.result;
                }
                
                fn stroke(inout self, color: vec4, width: float) -> vec4 {
                    self.stroke_keep(color, width);
                    self.old_shape = self.shape = 1e+20;
                    self.clip = -1e+20;
                    self.has_clip = 0.;
                    return self.result;
                }
                
                fn glow_keep(inout self, color: vec4, width: float) -> vec4 {
                    let f = self.calc_blur(abs(self.shape) - width / self.scale);
                    let source = vec4(color.rgb * color.a, color.a);
                    let dest = self.result;
                    self.result = vec4(source.rgb * f, 0.) + dest;
                    return self.result;
                }
                
                fn glow(inout self, color: vec4, width: float) -> vec4 {
                    self.glow_keep(color, width);
                    self.old_shape = self.shape = 1e+20;
                    self.clip = -1e+20;
                    self.has_clip = 0.;
                    return self.result;
                }
                
                fn union(inout self) {
                    self.old_shape = self.shape = min(self.field, self.old_shape);
                }
                
                fn intersect(inout self) {
                    self.old_shape = self.shape = max(self.field, self.old_shape);
                }
                
                fn subtract(inout self) {
                    self.old_shape = self.shape = max(-self.field, self.old_shape);
                }
                
                fn gloop(inout self, k: float) {
                    let h = clamp(0.5 + 0.5 * (self.old_shape - self.field) / k, 0.0, 1.0);
                    self.old_shape = self.shape = mix(self.old_shape, self.field, h) - k * h * (1.0 - h);
                }
                
                fn blend(inout self, k: float) {
                    self.old_shape = self.shape = mix(self.old_shape, self.field, k);
                }
                
                fn circle(inout self, x: float, y: float, r: float) {
                    let c = self.pos - vec2(x, y);
                    self.field = (length(c.xy) - r) / self.scale;
                    self.old_shape = self.shape;
                    self.shape = min(self.shape, self.field);
                }
                
                fn box(inout self, x: float, y: float, w: float, h: float, r: float) {
                    let p = self.pos - vec2(x, y);
                    let size = vec2(0.5 * w, 0.5 * h);
                    let bp = max(abs(p - size.xy) - (size.xy - vec2(2. * r, 2. * r).xy), vec2(0., 0.));
                    self.field = (length(bp) - 2. * r) / self.scale;
                    self.old_shape = self.shape;
                    self.shape = min(self.shape, self.field);
                }
                
                fn rect(inout self, x: float, y: float, w: float, h: float) {
                    let s = vec2(w, h) * 0.5;
                    let d = abs(vec2(x, y) - self.pos + s) - s;
                    let dm = min(d, vec2(0., 0.));
                    self.field = max(dm.x, dm.y) + length(max(d, vec2(0., 0.)));
                    self.old_shape = self.shape;
                    self.shape = min(self.shape, self.field);
                }
                
                fn hexagon(inout self, x: float, y: float, r: float) {
                    let dx = abs(x - self.pos.x) * 1.15;
                    let dy = abs(y - self.pos.y);
                    self.field = max(dy + cos(60.0 * TORAD) * dx - r, dx - r);
                    self.old_shape = self.shape;
                    self.shape = min(self.shape, self.field);
                }
                
                fn move_to(inout self, x: float, y: float) {
                    self.last_pos =
                    self.start_pos = vec2(x, y);
                }
                
                fn line_to(inout self, x: float, y: float) {
                    let p = vec2(x, y);
                    
                    let pa = self.pos - self.last_pos;
                    let ba = p - self.last_pos;
                    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
                    let s = sign(pa.x * ba.y - pa.y * ba.x);
                    self.field = length(pa - ba * h) / self.scale;
                    self.old_shape = self.shape;
                    self.shape = min(self.shape, self.field);
                    self.clip = max(self.clip, self.field * s);
                    self.has_clip = 1.0;
                    self.last_pos = p;
                }
                
                fn close_path(inout self) {
                    self.line_to(self.start_pos.x, self.start_pos.y);
                }
            }
        }"#
    );
}