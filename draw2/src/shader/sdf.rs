pub use {
    crate::{
        makepad_platform::*,
        //live_traits::*,
    }
};

script_mod!{
    use mod.pod.*
    use mod.math.*
    mod.sdf = {
        GaussShadow:{
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
                let mut x1 = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
                x1 *= x1;
                return s - s / (x1 * x1);
            }
            
            fn erf_vec4(x0:vec4)->vec4 {
                let s = sign(x0);
                let a = abs(x0);
                let mut x1 = 1.0 + (0.278393 + (0.230389 + 0.078108 * (a * a)) * a) * a;
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
            rounded_box_shadow: fn(lower:vec2, upper:vec2, point:vec2, sigma:float, corner:float)->float{
                // Center everything to make the math easier
                let center = (lower + upper) * 0.5;
                let half_size = (upper - lower) * 0.5;
                let point = point - center;
                                
                // The signal is only non-zero in a limited range, so don't waste samples
                let low = point.y - half_size.y;
                let high = point.y + half_size.y;
                let start = clamp(-3.0 * sigma, low, high);
                let end = clamp(3.0 * sigma, low, high);
                                
                // Accumulate samples (we can get away with surprisingly few samples)
                let step = (end - start) / 4.0;
                let mut y = start + step * 0.5;
                let mut value = 0.0;
                for i in 0..4{
                    value += rounded_box_shadow_x(point.x, point.y - y, sigma, corner, half_size) * gaussian(y, sigma) * step;
                    y += step;
                }
                                
                return value;
            }
            
            box_shadow: fn(lower:vec2, upper:vec2, point:vec2, sigma:float)->float {
                let query = vec4(point - lower, point - upper);
                let integral = 0.5 + 0.5 * erf_vec4(query * (sqrt(0.5) / sigma));
                return (integral.z - integral.x) * (integral.w - integral.y);
            }
        }
        
        Math:{
            rotate_2d: fn(v: vec2, a: float) -> vec2 {
                let ca = cos(a);
                let sa = sin(a);
                return vec2(v.x * ca - v.y * sa, v.x * sa + v.y * ca);
            }
            
            random_2d: fn(v: vec2)->float {
                return fract(sin(dot(v.xy, vec2(12.9898,78.233))) * 43758.5453);
            }
        }
        let Math = me.Math
        
        Pal:{
            
            premul: fn(v: vec4) -> vec4 {
                return vec4(v.x * v.w, v.y * v.w, v.z * v.w, v.w);
            }
            
            iq: fn(t: float, a: vec3, b: vec3, c: vec3, d: vec3) -> vec3 {
                return a + b * cos(6.28318 * (c * t + d));
            }
            
            iq0: fn(t: float) -> vec3 {
                return mix(vec3(0., 0., 0.), vec3(1., 1., 1.), cos(t * PI) * 0.5 + 0.5);
            }
            
            iq1: fn(t: float) -> vec3 {
                return Pal.iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 1.), vec3(0., 0.33, 0.67));
            }
            
            iq2: fn(t: float) -> vec3 {
                return Pal.iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 1.), vec3(0., 0.1, 0.2));
            }
            
            iq3: fn(t: float) -> vec3 {
                return Pal.iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 1.), vec3(0.3, 0.2, 0.2));
            }
            
            iq4: fn(t: float) -> vec3 {
                return Pal.iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 1., 0.5), vec3(0.8, 0.9, 0.3));
            }
            
            iq5: fn(t: float) -> vec3 {
                return Pal.iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(1., 0.7, 0.4), vec3(0, 0.15, 0.20));
            }
            
            iq6: fn(t: float) -> vec3 {
                return Pal.iq(t, vec3(0.5, 0.5, 0.5), vec3(0.5, 0.5, 0.5), vec3(2., 1.0, 0.), vec3(0.5, 0.2, 0.25));
            }
            
            iq7: fn(t: float) -> vec3 {
                return Pal.iq(t, vec3(0.8, 0.5, 0.4), vec3(0.2, 0.4, 0.2), vec3(2., 1.0, 1.0), vec3(0., 0.25, 0.25));
            }
            
            hsv2rgb: fn(c: vec4) -> vec4 { //http://gamedev.stackexchange.com/questions/59797/glsl-shader-change-hue-saturation-brightness
                let K = vec4(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
                let p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
                return vec4(c.z * mix(K.xxx, clamp(p - K.xxx, 0.0, 1.0), c.y), c.w);
            }
            
            rgb2hsv: fn(c: vec4) -> vec4 {
                let K: vec4 = vec4(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
                let p: vec4 = mix(vec4(c.bg, K.wz), vec4(c.gb, K.xy), step(c.b, c.g));
                let q: vec4 = mix(vec4(p.xyw, c.r), vec4(c.r, p.yzx), step(p.x, c.r));
                
                let d: float = q.x - min(q.w, q.y);
                let e: float = 1.0e-10;
                return vec4(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x, c.w);
            }
        }
        
        Sdf2d: struct {
            pos: vec2
            result: vec4
            last_pos: vec2
            start_pos: vec2
            shape: float
            clip: float
            has_clip: float
            old_shape: float
            blur: float
            aa: float
            scale_factor: float
            dist: float
            
            fn antialias(p: vec2) -> float {
                return 1.0 / length(vec2(length(dFdx(p)), length(dFdy(p))));
            }
            
            viewport: fn(pos: vec2) -> Self {
                return self (
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
                );
            }
            
            translate: fn(x: float, y: float) -> vec2 {
                self.pos -= vec2(x, y);
                return self.pos;
            }
            
            rotate: fn(a: float, x: float, y: float) {
                let ca = cos(-a);
                let sa = sin(-a);
                let p = self.pos - vec2(x, y);
                self.pos = vec2(p.x * ca - p.y * sa, p.x * sa + p.y * ca) + vec2(x, y);
            }
            
            scale: fn(f: float, x: float, y: float) {
                self.scale_factor *= f;
                self.pos = (self.pos - vec2(x, y)) * f + vec2(x, y);
            }
            
            clear: fn(color: vec4) {
                self.result = vec4(color.rgb * color.a + self.result.rgb * (1.0 - color.a), color.a);
            }
            
            calc_blur: fn(w: float) -> float {
                let wa = clamp(-w * self.aa, 0.0, 1.0);
                var wb = 1.0;
                if self.blur > 0.001 {
                    wb = clamp(-w / self.blur, 0.0, 1.0);
                }
                return wa * wb;
            }
            
            fill_keep_premul: fn(source: vec4) -> vec4 {
                let f = self.calc_blur(self.shape);
                self.result = source * f + self.result * (1. - source.a * f);
                if self.has_clip > 0.5 {
                    let f2 = 1.0 - self.calc_blur(-self.clip);
                    self.result = source * f2 + self.result * (1. - source.a * f2);
                }
                return self.result;
            }
                    
            fill_premul: fn(color: vec4) -> vec4 {
                self.fill_keep_premul(color);
                self.old_shape = 1e+20;
                self.shape = 1e+20;
                self.clip = -1e+20;
                self.has_clip = 0.;
                return self.result;
            }
            
            fill_keep: fn(color: vec4) -> vec4 {
                return self.fill_keep_premul(vec4(color.rgb * color.a, color.a))
            }
            
            fill: fn(color: vec4) -> vec4 {
                return self.fill_premul(vec4(color.rgb * color.a, color.a))
            }
            
            stroke_keep: fn(color: vec4, width: float) -> vec4 {
                let f = self.calc_blur(abs(self.shape) - width / self.scale_factor);
                let source = vec4(color.rgb * color.a, color.a);
                let dest = self.result;
                self.result = source * f + dest * (1.0 - source.a * f);
                return self.result;
            }
            
            stroke: fn(color: vec4, width: float) -> vec4 {
                self.stroke_keep(color, width);
                self.old_shape = 1e+20;
                self.shape = 1e+20;
                self.clip = -1e+20;
                self.has_clip = 0.;
                return self.result;
            }
            
            glow_keep: fn(color: vec4, width: float) -> vec4 {
                let f = self.calc_blur(abs(self.shape) - width / self.scale_factor);
                let source = vec4(color.rgb * color.a, color.a);
                let dest = self.result;
                self.result = vec4(source.rgb * f, 0.) + dest;
                return self.result;
            }
            
            glow: fn(color: vec4, width: float) -> vec4 {
                self.glow_keep(color, width);
                self.shape = 1e+20;
                self.old_shape = self.shape;
                self.clip = -1e+20;
                self.has_clip = 0.;
                return self.result;
            }
            
            union: fn() {
                self.shape = min(self.dist, self.old_shape);
                self.old_shape = self.shape;
            }
            
            intersect: fn() {
                self.shape = max(self.dist, self.old_shape);
                self.old_shape = self.shape;
            }
            
            subtract: fn() {
                self.shape = max(-self.dist, self.old_shape);
                self.old_shape = self.shape;
            }
            
            gloop: fn(k: float) {
                let h = clamp(0.5 + 0.5 * (self.old_shape - self.dist) / k, 0.0, 1.0);
                self.shape = mix(self.old_shape, self.dist, h) - k * h * (1.0 - h);
                self.old_shape = self.shape;
            }
            
            blend: fn(k: float) {
                self.shape = mix(self.old_shape, self.dist, k);
                self.old_shape = self.shape;
            }
            
            circle: fn(x: float, y: float, r: float) {
                let c = self.pos - vec2(x, y);
                let len = sqrt(c.x * c.x + c.y * c.y);
                self.dist = (len - r) / self.scale_factor;
                self.old_shape = self.shape;
                self.shape = min(self.shape, self.dist);
            }
    
            // A distance function for an arc with round caps
            arc_round_caps: fn(
                // The x-coordinate of the center of the arc
                x: float,
                // The y-coordinate of the center of the arc
                y: float,
                // The radius of the the arc
                radius: float,
                // The start angle of the arc, in radians
                start_angle: float,
                // The end angle of the arc, in radians
                end_angle: float,
                // The thickness of the arc
                thickness: float
            ) {
                let p = self.pos - vec2(x, y);
                let half_angle = (end_angle - start_angle) / 2.0;
                let p = Math.rotate_2d(p, -start_angle - half_angle);
                p.x = abs(p.x);
                let sin_half_angle = sin(half_angle);
                let cos_half_angle = cos(half_angle);
                let dist_to_arc = abs(length(p) - radius) - 0.5 * thickness;
                let cap_center = vec2(sin_half_angle, cos_half_angle) * radius;
                let dist_to_cap = length(p - cap_center) - 0.5 * thickness;
                if cos_half_angle * p.x < sin_half_angle * p.y {
                    self.dist = dist_to_arc;
                } else {
                    self.dist = dist_to_cap;
                }
                self.old_shape = self.shape;
                self.shape = min(self.shape, self.dist);
            }
    
            // A distance function for an arc with flat caps
            arc_flat_caps: fn(
                // The x-coordinate of the center of the arc
                x: float,
                // The y-coordinate of the center of the arc
                y: float,
                // The radius of the arc
                radius: float,
                // The start angle of the arc, in radians
                start_angle: float,
                // The end angle of the arc, in radians
                end_angle: float,
                // The thickness of the arc
                thickness: float
            ) {
                let p = self.pos - vec2(x, y);
                let half_angle = (end_angle - start_angle) / 2.0;
                let p = Math.rotate_2d(p, -start_angle - half_angle);
                p.x = abs(p.x);
                let p = Math.rotate_2d(p, half_angle);
                let dist_to_arc = abs(length(p) - radius) - thickness * 0.5;
                let dist_y_to_cap = max(0.0, abs(radius - p.y) - thickness * 0.5);
                let dist_to_cap =  sign(p.x) * length(vec2(p.x, dist_y_to_cap));
                self.dist = max(dist_to_arc, dist_to_cap);
                self.old_shape = self.shape;
                self.shape = min(self.shape, self.dist);
            }
             
            arc2: fn(x: float, y: float, r: float, s:float, e:float)->vec4{
                let c = self.pos - vec2(x, y);
                let pi = 3.141592653589793; // FIX THIS BUG
                
                //let circle = (sqrt(c.x * c.x + c.y * c.y) - r)*ang;
                
                // ok lets do atan2
                let ang = (atan(c.y,c.x)+pi)/(2.0*pi);
                let ces = (e-s)*0.5;
                let ang2 = 1.0 - abs(ang - ces)+ces
                return mix(vec4(0.,0.,0.,1.0),vec4(1.0),ang2);
            }
            
            
            hline: fn(y: float, h:float) {
                let c = self.pos.y - y;
                self.dist = -h+abs(c) / self.scale_factor;
                self.old_shape = self.shape;
                self.shape = min(self.shape, self.dist);
            }
            
            box: fn(x: float, y: float, w: float, h: float, r: float) {
                let p = self.pos - vec2(x, y);
                let size = vec2(0.5 * w, 0.5 * h);
                let bp = max(abs(p - size.xy) - (size.xy - vec2(2. * r, 2. * r).xy), vec2(0., 0.));
                self.dist = (length(bp) - 2. * r) / self.scale_factor;
                self.old_shape = self.shape;
                self.shape = min(self.shape, self.dist);
            }
            
            box_y: fn(x: float, y: float, w: float, h: float, r_top: float, r_bottom: float) {
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
            
            box_x: fn(x: float, y: float, w: float, h: float, r_left: float, r_right: float) {
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
            
            box_all: fn(
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
            
            
            rect: fn(x: float, y: float, w: float, h: float) {
                let s = vec2(w, h) * 0.5;
                let d = abs(vec2(x, y) - self.pos + s) - s;
                let dm = min(d, vec2(0., 0.));
                self.dist = max(dm.x, dm.y) + length(max(d, vec2(0., 0.)));
                self.old_shape = self.shape;
                self.shape = min(self.shape, self.dist);
            }
            
            hexagon: fn(x: float, y: float, r: float) {
                let dx = abs(x - self.pos.x) * 1.15;
                let dy = abs(y - self.pos.y);
                self.dist = max(dy + cos(60.0 * TORAD) * dx - r, dx - r);
                self.old_shape = self.shape;
                self.shape = min(self.shape, self.dist);
            }
            
            move_to: fn(x: float, y: float) {
                self.last_pos = vec2(x, y);
                self.start_pos = vec2(x, y);
            }
            
            line_to: fn(x: float, y: float) {
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
            
            close_path: fn() {
                self.line_to(self.start_pos.x, self.start_pos.y);
            }
        }
    }
}
