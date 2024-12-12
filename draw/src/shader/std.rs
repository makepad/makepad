pub use {
    crate::{
        makepad_derive_live::*,
        makepad_platform::*,
        //live_traits::*,
    }
};

live_design!{
    link shaders;
    
    pub PI = 3.141592653589793
    pub E = 2.718281828459045
    pub LN2 = 0.6931471805599453
    pub LN10 = 2.302585092994046
    pub LOG2E = 1.4426950408889634
    pub LOG10E = 0.4342944819032518
    pub SQRT1_2 = 0.70710678118654757
    pub TORAD = 0.017453292519943295
    pub GOLDEN = 1.618033988749895
    
    pub GaussShadow = {
        // ported from https://madebyevan.com/shaders/fast-rounded-rectangle-shadows/
        // License: CC0 (http://creativecommons.org/publicdomain/zero/1.0/)
        fn gaussian(x:float, sigma:float )->float{
            let pi = 3.141592653589793;
            return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * pi) * sigma);
        }
                    
        // This approximates the error function, needed for the gaussian integral
        fn erf_vec2(x0:vec2)->vec2 {
            let s = sign(x0);
            let a = abs(x0);
            let x1 = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
            x1 *= x1;
            return s - s / (x1 * x1);
        }
        
        fn erf_vec4(x0:vec4)->vec4 {
            let s = sign(x0);
            let a = abs(x0);
            let x1 = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
            x1 *= x1;
            return s - s / (x1 * x1);
        }
                    
        // Return the blurred mask along the x dimension
        fn rounded_box_shadow_x(x:float, y:float, sigma:float, corner:float, half_size:vec2)->float{
            let delta = min(half_size.y - corner - abs(y), 0.0);
            let curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
            let integral = 0.5 + 0.5 * erf_vec2((x + vec2(-curved, curved)) * (sqrt(0.5) / sigma));
            return integral.y - integral.x;
        }
                    
        // Return the mask for the shadow of a box from lower to upper
        fn rounded_box_shadow(lower:vec2, upper:vec2, point:vec2, sigma:float, corner:float)->float{
            // Center everything to make the math easier
            let center = (lower + upper) * 0.5;
            let half_size = (upper - lower) * 0.5;
            point -= center;
                            
            // The signal is only non-zero in a limited range, so don't waste samples
            let low = point.y - half_size.y;
            let high = point.y + half_size.y;
            let start = clamp(-3.0 * sigma, low, high);
            let end = clamp(3.0 * sigma, low, high);
                            
            // Accumulate samples (we can get away with surprisingly few samples)
            let step = (end - start) / 4.0;
            let y = start + step * 0.5;
            let value = 0.0;
            for i in 0..4{
                value += rounded_box_shadow_x(point.x, point.y - y, sigma, corner, half_size) * gaussian(y, sigma) * step;
                y += step;
            }
                            
            return value;
        }
        
        fn box_shadow(lower:vec2, upper:vec2, point:vec2, sigma:float)->float {
            let query = vec4(point - lower, point - upper);
            let integral = 0.5 + 0.5 * erf_vec4(query * (sqrt(0.5) / sigma));
            return (integral.z - integral.x) * (integral.w - integral.y);
        }
    }
    
    pub Math = {
        fn rotate_2d(v: vec2, a: float) -> vec2 {
            let ca = cos(a);
            let sa = sin(a);
            return vec2(v.x * ca - v.y * sa, v.x * sa + v.y * ca);
        }
        
        fn random_2d(v: vec2)->float {
           return fract(sin(dot(v.xy, vec2(12.9898,78.233))) * 43758.5453);
        }
    }
    
    pub Pal = {
        
        fn premul(v: vec4) -> vec4 {
            return vec4(v.x * v.w, v.y * v.w, v.z * v.w, v.w);
        }
        
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
    
    pub Sdf2d = struct {
        field pos: vec2
        field result: vec4
        field last_pos: vec2
        field start_pos: vec2
        field shape: float
        field clip: float
        field has_clip: float
        field old_shape: float
        field blur: float
        field aa: float
        field scale_factor: float
        field dist: float
        
        fn antialias(p: vec2) -> float {
            return 1.0 / length(vec2(length(dFdx(p)), length(dFdy(p))));
        }
        
        fn viewport(pos: vec2) -> Self {
            return Self {
                pos: pos
                result: vec4(0.)
                last_pos: vec2(0.)
                start_pos: vec2(0.)
                shape: 1e+20
                clip: -1e+20
                has_clip: 0.0
                old_shape: 1e+20
                blur: 0.00001
                aa: antialias(pos)
                scale_factor: 1.0
                dist: 0.0
            };
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
            self.scale_factor *= f;
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
        
        fn fill_keep_premul(inout self, source: vec4) -> vec4 {
            let f = self.calc_blur(self.shape);
            self.result = source * f + self.result * (1. - source.a * f);
            if self.has_clip > 0.5 {
                let f2 = 1.0 - self.calc_blur(-self.clip);
                self.result = source * f2 + self.result * (1. - source.a * f2);
            }
            return self.result;
        }
                
        fn fill_premul(inout self, color: vec4) -> vec4 {
            self.fill_keep_premul(color);
            self.old_shape = self.shape = 1e+20;
            self.clip = -1e+20;
            self.has_clip = 0.;
            return self.result;
        }
        
        fn fill_keep(inout self, color: vec4) -> vec4 {
            return self.fill_keep_premul(vec4(color.rgb * color.a, color.a))
        }
        
        fn fill(inout self, color: vec4) -> vec4 {
            return self.fill_premul(vec4(color.rgb * color.a, color.a))
        }
        
        fn stroke_keep(inout self, color: vec4, width: float) -> vec4 {
            let f = self.calc_blur(abs(self.shape) - width / self.scale_factor);
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
            let f = self.calc_blur(abs(self.shape) - width / self.scale_factor);
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
            self.old_shape = self.shape = min(self.dist, self.old_shape);
        }
        
        fn intersect(inout self) {
            self.old_shape = self.shape = max(self.dist, self.old_shape);
        }
        
        fn subtract(inout self) {
            self.old_shape = self.shape = max(-self.dist, self.old_shape);
        }
        
        fn gloop(inout self, k: float) {
            let h = clamp(0.5 + 0.5 * (self.old_shape - self.dist) / k, 0.0, 1.0);
            self.old_shape = self.shape = mix(self.old_shape, self.dist, h) - k * h * (1.0 - h);
        }
        
        fn blend(inout self, k: float) {
            self.old_shape = self.shape = mix(self.old_shape, self.dist, k);
        }
        
        fn circle(inout self, x: float, y: float, r: float) {
            let c = self.pos - vec2(x, y);
            let len = sqrt(c.x * c.x + c.y * c.y);
            self.dist = (len - r) / self.scale_factor;
            self.old_shape = self.shape;
            self.shape = min(self.shape, self.dist);
        }
         
        fn arc2(inout self, x: float, y: float, r: float, s:float, e:float)->vec4{
            let c = self.pos - vec2(x, y);
            let pi = 3.141592653589793; // FIX THIS BUG
            
            //let circle = (sqrt(c.x * c.x + c.y * c.y) - r)*ang;
            
            // ok lets do atan2
            let ang = (atan(c.y,c.x)+pi)/(2.0*pi);
            let ces = (e-s)*0.5;
            let ang2 = 1.0 - abs(ang - ces)+ces
            return mix(vec4(0.,0.,0.,1.0),vec4(1.0),ang2);
        }
        
        
        fn hline(inout self, y: float, h:float) {
            let c = self.pos.y - y;
            self.dist = -h+abs(c) / self.scale_factor;
            self.old_shape = self.shape;
            self.shape = min(self.shape, self.dist);
        }
        
        fn box(inout self, x: float, y: float, w: float, h: float, r: float) {
            let p = self.pos - vec2(x, y);
            let size = vec2(0.5 * w, 0.5 * h);
            let bp = max(abs(p - size.xy) - (size.xy - vec2(2. * r, 2. * r).xy), vec2(0., 0.));
            self.dist = (length(bp) - 2. * r) / self.scale_factor;
            self.old_shape = self.shape;
            self.shape = min(self.shape, self.dist);
        }
        fn box_y(inout self, x: float, y: float, w: float, h: float, r_top: float, r_bottom: float) {
            let size = vec2(0.5 * w, 0.5 * h);
            let p_r = self.pos - vec2(x, y);
            let p = abs(p_r - size.xy) - size.xy;
            
            let bp_top = max(p + vec2(2. * r_top, 2. * r_top).xy, vec2(0., 0.));
            let bp_bottom = max(p + vec2(2. * r_bottom, 2. * r_bottom).xy, vec2(0., 0.));
            
            self.dist = mix(
                (length(bp_top) - 2. * r_top),
                (length(bp_bottom) - 2. * r_bottom),
                step(0.5 * h, p_r.y)
            ) / self.scale_factor;
            
            self.old_shape = self.shape;
            self.shape = min(self.shape, self.dist);
        }
        
        fn box_x(inout self, x: float, y: float, w: float, h: float, r_left: float, r_right: float) {
            let size = vec2(0.5 * w, 0.5 * h);
            let p_r = self.pos - vec2(x, y);
            let p = abs(p_r - size.xy) - size.xy;
            
            let bp_left = max(p + vec2(2. * r_left, 2. * r_left).xy, vec2(0., 0.));
            let bp_right = max(p + vec2(2. * r_right, 2. * r_right).xy, vec2(0., 0.));
            
            self.dist = mix(
                (length(bp_left) - 2. * r_left),
                (length(bp_right) - 2. * r_right),
                step(0.5 * w, p_r.x)
            ) / self.scale_factor;
            
            self.old_shape = self.shape;
            self.shape = min(self.shape, self.dist);
        }
        
        fn box_all(
            inout self,
            x: float,
            y: float,
            w: float,
            h: float,
            r_left_top: float,
            r_right_top: float,
            r_right_bottom: float,
            r_left_bottom: float
        ) {
            let size = vec2(0.5 * w, 0.5 * h);
            let p_r = self.pos - vec2(x, y);
            let p = abs(p_r - size.xy) - size.xy;
            
            let bp_lt = max(p + vec2(2. * r_left_top, 2. * r_left_top).xy, vec2(0., 0.));
            let bp_rt = max(p + vec2(2. * r_right_top, 2. * r_right_top).xy, vec2(0., 0.));
            let bp_rb = max(p + vec2(2. * r_right_bottom, 2. * r_right_bottom).xy, vec2(0., 0.));
            let bp_lb = max(p + vec2(2. * r_left_bottom, 2. * r_left_bottom).xy, vec2(0., 0.));
            
            self.dist = mix(
                mix(
                    (length(bp_lt) - 2. * r_left_top),
                    (length(bp_lb) - 2. * r_left_bottom),
                    step(0.5 * h, p_r.y)
                ),
                mix(
                    (length(bp_rt) - 2. * r_right_top),
                    (length(bp_rb) - 2. * r_right_bottom),
                    step(0.5 * h, p_r.y)
                ),
                step(0.5 * w, p_r.x)
            ) / self.scale_factor;
            
            self.old_shape = self.shape;
            self.shape = min(self.shape, self.dist);
        }
        
        
        fn rect(inout self, x: float, y: float, w: float, h: float) {
            let s = vec2(w, h) * 0.5;
            let d = abs(vec2(x, y) - self.pos + s) - s;
            let dm = min(d, vec2(0., 0.));
            self.dist = max(dm.x, dm.y) + length(max(d, vec2(0., 0.)));
            self.old_shape = self.shape;
            self.shape = min(self.shape, self.dist);
        }
        
        fn hexagon(inout self, x: float, y: float, r: float) {
            let dx = abs(x - self.pos.x) * 1.15;
            let dy = abs(y - self.pos.y);
            self.dist = max(dy + cos(60.0 * TORAD) * dx - r, dx - r);
            self.old_shape = self.shape;
            self.shape = min(self.shape, self.dist);
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
            self.dist = length(pa - ba * h) / self.scale_factor;
            self.old_shape = self.shape;
            self.shape = min(self.shape, self.dist);
            self.clip = max(self.clip, self.dist * s);
            self.has_clip = 1.0;
            self.last_pos = p;
        }
        
        fn close_path(inout self) {
            self.line_to(self.start_pos.x, self.start_pos.y);
        }
    }
}
