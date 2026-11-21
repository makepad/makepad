//! The tone curves (`mod.draw.ToneCurves`): the one copy every display
//! transform takes its curve from (the graph's tone map, the Sandbox
//! composite, graph passes, the path tracer's tone map, an app's own
//! composite), so two renderers' pictures of one scene under one light meet
//! on the display. It needs only the drawing layer, so a host without
//! widgets (the path tracer's own tests) registers it too.
use makepad_draw::*;

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // The tone curves, one of each for every host: DrawToneMap, the
    // Sandbox composite, the path tracer's tone map and graph passes (a pass's `helpers` may derive
    // from it: `helpers: mod.draw.ToneCurves{mine: fn..}`). Linear in.
    mod.draw.ToneCurves = {
        aces: fn(x: vec3) -> vec3 {
            let a = x * (x * 2.51 + vec3(0.03, 0.03, 0.03))
            let b = x * (x * 2.43 + vec3(0.59, 0.59, 0.59)) + vec3(0.14, 0.14, 0.14)
            return clamp(a / b, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
        }
        filmic: fn(x: vec3) -> vec3 {
            let a = 0.15
            let b = 0.5
            let c = 0.1
            let d = 0.2
            let e = 0.02
            let f = 0.3
            return ((x * (x * a + vec3(c * b, c * b, c * b)) + vec3(d * e, d * e, d * e)) / (x * (x * a + vec3(b, b, b)) + vec3(d * f, d * f, d * f))) - vec3(e / f, e / f, e / f)
        }
        // AgX (Sobotka) in three steps, so a host can grade between them:
        // into the AgX primaries and log2-encoded over [-12.47, 4.03] EV
        // (0..1), the sigmoid (the minimal polynomial fit), and back out
        // (display-encoded).
        agx_encode: fn(c: vec3) -> vec3 {
            let v = vec3(
                c.x * 0.842479 + c.y * 0.078434 + c.z * 0.079224,
                c.x * 0.042328 + c.y * 0.878469 + c.z * 0.079166,
                c.x * 0.042376 + c.y * 0.078434 + c.z * 0.879143
            )
            let lo = -12.47393
            let hi = 4.026069
            return (clamp(log2(max(v, vec3(0.0000001, 0.0000001, 0.0000001))), vec3(lo, lo, lo), vec3(hi, hi, hi)) - vec3(lo, lo, lo)) / (hi - lo)
        }
        agx_contrast: fn(x: vec3) -> vec3 {
            let x2 = x * x
            let x4 = x2 * x2
            return x4 * x2 * 15.5 - x4 * x * 40.14 + x4 * 31.96 - x2 * x * 6.868 + x2 * 0.4298 + x * 0.1191 - vec3(0.00232, 0.00232, 0.00232)
        }
        agx_decode: fn(s: vec3) -> vec3 {
            return vec3(
                s.x * 1.196879 + s.y * -0.098021 + s.z * -0.099029,
                s.x * -0.052897 + s.y * 1.151903 + s.z * -0.098935,
                s.x * -0.052968 + s.y * -0.098043 + s.z * 1.151073
            )
        }
        // The AgX look the HDR composites share (the Sandbox composite, a
        // host's own viewport composite): `contrast` as a slope about
        // mid-grey (0.18 encodes to 0.606) before the sigmoid, which rolls
        // both ends off, then a light "punch" that restores the contrast and
        // saturation plain AgX gives up (`saturation` 1: the stock look).
        // Linear in, display-encoded out (no extra sRGB OETF).
        agx_look: fn(c: vec3, contrast: float, saturation: float) -> vec3 {
            let e = clamp((self.agx_encode(c) - vec3(0.606, 0.606, 0.606)) * contrast + vec3(0.606, 0.606, 0.606), vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
            let o = self.agx_decode(self.agx_contrast(e))
            let p = pow(clamp(o, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), vec3(1.1, 1.1, 1.1))
            let luma = dot(p, vec3(0.2126, 0.7152, 0.0722))
            return clamp(vec3(luma, luma, luma) + (p - vec3(luma, luma, luma)) * (1.35 * saturation), vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
        }
        // agx_look with the punch kept inside the display gamut: the chroma is scaled about
        // the luma by at most what keeps every channel in 0..1, so a bright saturated colour
        // (an orange filament under a key) keeps its hue and its faces keep their values
        // apart, where agx_look clips its strong channel flat on every lit face.
        agx_look_fit: fn(c: vec3, contrast: float, saturation: float) -> vec3 {
            let e = clamp((self.agx_encode(c) - vec3(0.606, 0.606, 0.606)) * contrast + vec3(0.606, 0.606, 0.606), vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
            let o = self.agx_decode(self.agx_contrast(e))
            let p = pow(clamp(o, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), vec3(1.1, 1.1, 1.1))
            let luma = dot(p, vec3(0.2126, 0.7152, 0.0722))
            let d = p - vec3(luma, luma, luma)
            let hi = max(max(d.x, d.y), d.z)
            let lo = min(min(d.x, d.y), d.z)
            var k = 1.35 * saturation
            if hi > 0.000001 { k = min(k, (1.0 - luma) / hi) }
            if lo < -0.000001 { k = min(k, luma / (0.0 - lo)) }
            return clamp(vec3(luma, luma, luma) + d * k, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0))
        }
        // AgX to linear light again (display-encoded, then 2.2).
        agx: fn(c: vec3) -> vec3 {
            let o = self.agx_decode(self.agx_contrast(self.agx_encode(c)))
            return pow(max(o, vec3(0.0, 0.0, 0.0)), vec3(2.2, 2.2, 2.2))
        }
        // The shoulder (PDOOM R5) for a 2D frame: identity up to 1, so its
        // ordinary colours show exactly as authored; above, the hue is kept
        // (the brightest channel held at 1) and very bright light
        // desaturates toward white.
        shoulder: fn(c: vec3) -> vec3 {
            let x = max(c, vec3(0.0, 0.0, 0.0))
            let m = max(x.x, max(x.y, x.z))
            if m <= 1.0 {
                return x
            }
            let white = clamp((m - 1.0) / 8.0, 0.0, 1.0)
            return mix(x / m, vec3(1.0, 1.0, 1.0), white * white)
        }
    }
}

/// Register `mod.draw.ToneCurves`, once per VM however many hosts ask.
pub fn register(vm: &mut ScriptVm) {
    let draw = vm.value(vm.modules(), LiveId::from_str("draw")).as_object();
    let have = draw.is_some_and(|d| {
        let v = vm.value(d, LiveId::from_str("ToneCurves"));
        !v.is_nil() && !v.is_err()
    });
    if !have {
        script_mod(vm);
    }
}
