//! The sky artwork: one code-native quad that paints the condition palette
//! (class × day/night, blended through dawn and dusk), a broad sun or moon
//! glow kept out of the text zone, feathered cloud banks that follow the
//! cloud cover, fine sparse precipitation scaled by intensity, and
//! low-frequency fog — from the numbers in [`SkyInputs`]. No textures, any
//! size. `time` drifts the clouds; the view advances it only while the
//! full face is on screen (the tile stays still between data updates).
use crate::model::{SkyInputs, SkyKind};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*

    mod.widgets.DrawSky = set_type_default() do #(DrawSky::script_shader(vm)) {
        ..mod.draw.DrawQuad
        kind: 0.0
        phase: 1.0
        cover: 0.2
        precip: 0.0
        time: 0.0
        border_radius: theme.container_corner_radius
        // Text zones in the quad's own points (x, y, w, h; w = 0 means
        // none): 0-1 hold large text (the hero, the tile's temperature;
        // 3:1), 2-4 small text (4.5:1), 5-6 the glass panels (soft). The
        // art (glow, clouds, precipitation, fog, stars) never enters a
        // text zone and thins to a third under a soft one, and a black
        // scrim darkens each only as far as white text needs: from 0.10,
        // capped at 0.28 (large) or 0.40 (small, over the brightest days).
        zone0: vec4(0.0, 0.0, 0.0, 0.0)
        zone1: vec4(0.0, 0.0, 0.0, 0.0)
        zone2: vec4(0.0, 0.0, 0.0, 0.0)
        zone3: vec4(0.0, 0.0, 0.0, 0.0)
        zone4: vec4(0.0, 0.0, 0.0, 0.0)
        zone5: vec4(0.0, 0.0, 0.0, 0.0)
        zone6: vec4(0.0, 0.0, 0.0, 0.0)
        feather: 14.0
        soft_feather: 64.0

        hash: fn(p: vec2) -> float {
            return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453)
        }
        noise: fn(p: vec2) -> float {
            let i = floor(p)
            let f = fract(p)
            let u = f * f * (3.0 - 2.0 * f)
            let a = self.hash(i)
            let b = self.hash(i + vec2(1.0, 0.0))
            let c = self.hash(i + vec2(0.0, 1.0))
            let d = self.hash(i + vec2(1.0, 1.0))
            return mix(mix(a, b, u.x), mix(c, d, u.x), u.y)
        }
        zone_mask: fn(z: vec4, p: vec2, f: float) -> float {
            if z.z <= 0.0 { return 0.0 }
            let mx = smoothstep(z.x - f, z.x, p.x) * (1.0 - smoothstep(z.x + z.z, z.x + z.z + f, p.x))
            let my = smoothstep(z.y - f, z.y, p.y) * (1.0 - smoothstep(z.y + z.w, z.y + z.w + f, p.y))
            return mx * my
        }
        fbm: fn(p: vec2) -> float {
            return self.noise(p) * 0.55 + self.noise(p * 2.03 + vec2(1.7, 9.2)) * 0.30 + self.noise(p * 4.01 + vec2(8.3, 2.8)) * 0.15
        }
        // The class palettes (top, bottom) by day and by night; see the design's table.
        top_day: fn(k: float) -> vec3 {
            if k < 0.5 { return vec3(0.157, 0.412, 0.671) }
            if k < 1.5 { return vec3(0.227, 0.439, 0.624) }
            if k < 2.5 { return vec3(0.408, 0.486, 0.557) }
            if k < 3.5 { return vec3(0.271, 0.357, 0.447) }
            if k < 4.5 { return vec3(0.475, 0.561, 0.624) }
            if k < 5.5 { return vec3(0.486, 0.549, 0.596) }
            if k < 6.5 { return vec3(0.235, 0.286, 0.384) }
            return vec3(0.306, 0.392, 0.478)
        }
        bot_day: fn(k: float) -> vec3 {
            if k < 0.5 { return vec3(0.439, 0.710, 0.863) }
            if k < 1.5 { return vec3(0.616, 0.745, 0.816) }
            if k < 2.5 { return vec3(0.655, 0.706, 0.733) }
            if k < 3.5 { return vec3(0.498, 0.588, 0.651) }
            if k < 4.5 { return vec3(0.745, 0.796, 0.820) }
            if k < 5.5 { return vec3(0.706, 0.749, 0.769) }
            if k < 6.5 { return vec3(0.443, 0.502, 0.569) }
            return vec3(0.518, 0.592, 0.651)
        }
        top_night: fn(k: float) -> vec3 {
            if k < 0.5 { return vec3(0.031, 0.059, 0.169) }
            if k < 1.5 { return vec3(0.082, 0.125, 0.224) }
            if k < 2.5 { return vec3(0.125, 0.169, 0.231) }
            if k < 3.5 { return vec3(0.082, 0.129, 0.212) }
            if k < 4.5 { return vec3(0.149, 0.220, 0.290) }
            if k < 5.5 { return vec3(0.176, 0.216, 0.267) }
            if k < 6.5 { return vec3(0.063, 0.094, 0.165) }
            return vec3(0.141, 0.188, 0.267)
        }
        bot_night: fn(k: float) -> vec3 {
            if k < 0.5 { return vec3(0.137, 0.247, 0.408) }
            if k < 1.5 { return vec3(0.259, 0.333, 0.412) }
            if k < 2.5 { return vec3(0.337, 0.380, 0.435) }
            if k < 3.5 { return vec3(0.231, 0.322, 0.416) }
            if k < 4.5 { return vec3(0.384, 0.463, 0.525) }
            if k < 5.5 { return vec3(0.416, 0.463, 0.502) }
            if k < 6.5 { return vec3(0.227, 0.278, 0.380) }
            return vec3(0.322, 0.376, 0.443)
        }
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            let uv = self.pos
            let aspect = self.rect_size.x / max(self.rect_size.y, 1.0)
            let q = vec2(uv.x * aspect, uv.y)
            let k = self.kind
            let day = clamp(self.phase, 0.0, 1.0)
            let cover = clamp(self.cover, 0.0, 1.0)
            let precip = clamp(self.precip, 0.0, 1.0)
            let is_rain = step(2.5, k) * step(k, 3.5)
            let is_snow = step(3.5, k) * step(k, 4.5)
            let is_fog = step(4.5, k) * step(k, 5.5)
            let is_thunder = step(5.5, k) * step(k, 6.5)
            let is_open = 1.0 - step(1.5, k)
            let px = self.pos * self.rect_size
            let f = self.feather
            let large = max(self.zone_mask(self.zone0, px, f), self.zone_mask(self.zone1, px, f))
            let small = max(max(self.zone_mask(self.zone2, px, f), self.zone_mask(self.zone3, px, f)), self.zone_mask(self.zone4, px, f))
            let soft = max(self.zone_mask(self.zone5, px, self.soft_feather), self.zone_mask(self.zone6, px, self.soft_feather))
            let art = (1.0 - max(large, small)) * (1.0 - 0.65 * soft)

            // The palette: night below, day above, blended through the phase.
            let top = mix(self.top_night(k), self.top_day(k), day)
            let bot = mix(self.bot_night(k), self.bot_day(k), day)
            let mut col = mix(top, bot, uv.y)

            // Dawn and dusk warm a clear or partly cloudy horizon; cloud cover suppresses it.
            let twilight = 4.0 * day * (1.0 - day) * is_open * (1.0 - cover * 0.8)
            let warm_top = vec3(0.224, 0.294, 0.510)
            let warm_mid = vec3(0.682, 0.506, 0.584)
            let warm_bot = vec3(0.878, 0.678, 0.553)
            let warm = mix(mix(warm_top, warm_mid, smoothstep(0.0, 0.55, uv.y)), warm_bot, smoothstep(0.55, 1.0, uv.y))
            col = mix(col, warm, twilight)
            let flat = col

            // Stars, thinning as the sky clouds over or brightens.
            let cell = floor(q * 16.0)
            let h = self.hash(cell)
            let cp = fract(q * 16.0) - vec2(0.5, 0.5)
            let star = (1.0 - smoothstep(0.03, 0.06, length(cp))) * step(0.95, h)
            col = mix(col, vec3(1.0, 1.0, 1.0), star * (1.0 - day) * (1.0 - cover) * 0.8 * art)

            // A broad glow outside the text zone: the sun by day, the moon by night.
            let glow_pos = vec2(aspect * 0.86, 0.14)
            let gd = length(q - glow_pos)
            let sun_vis = (1.0 - cover * 0.7) * (1.0 - is_fog * 0.6)
            let sun_glow = exp(-gd * gd * 6.0) * 0.55 * day * sun_vis * art
            col = mix(col, vec3(1.0, 0.92, 0.72), sun_glow)
            let moon_glow = exp(-gd * gd * 14.0) * 0.35 * (1.0 - day) * sun_vis * art
            col = mix(col, vec3(0.86, 0.88, 0.92), moon_glow)

            // Feathered cloud banks: value noise thresholded by cover, drifting slowly.
            let drift = vec2(self.time * 0.012, 0.0)
            let n1 = self.fbm(q * 2.6 + drift + vec2(3.1, 0.4))
            let n2 = self.fbm(q * 5.1 - drift * 0.6 + vec2(11.0, 7.0))
            let bank = smoothstep(0.62 - cover * 0.42, 0.78 - cover * 0.28, n1 * 0.7 + n2 * 0.3)
            // Heavy cover flattens the banks toward the palette: an overcast
            // sky is a grey field with soft patches, not white puffs.
            let cloud_lum = mix(0.30, mix(0.97, 0.70, cover), day) * mix(1.0, 0.62, (is_rain + is_thunder) * 0.9 + is_snow * 0.2)
            let cloud_col = vec3(cloud_lum, cloud_lum, cloud_lum + 0.02) * mix(1.0, 0.85, is_thunder)
            let shade = 1.0 - 0.18 * smoothstep(0.35, 0.9, uv.y) * bank
            col = mix(col, cloud_col * shade, bank * (0.45 + cover * 0.15) * art)

            // Fine, sparse precipitation: streaks of rain, dots of snow, scaled by intensity.
            let rp = vec2(q.x * 46.0 + q.y * 9.0 - self.time * 0.9, q.y * 15.0 + self.time * 2.2)
            let rrow = self.hash(vec2(floor(rp.x), 0.0))
            let streak = (1.0 - smoothstep(0.02, 0.08, abs(fract(rp.x) - 0.5))) * step(0.72, fract(rp.y + rrow)) * step(0.55, rrow)
            col = mix(col, vec3(0.80, 0.88, 1.0), streak * (is_rain + is_thunder) * (0.25 + precip * 0.55) * smoothstep(0.2, 0.7, uv.y) * art)
            let sp = vec2(q.x * 18.0 + sin(q.y * 6.0 + self.time * 0.4) * 0.3, q.y * 18.0 + self.time * 0.9)
            let sc = fract(sp) - vec2(0.5, 0.5)
            let sh = self.hash(floor(sp))
            let flake = (1.0 - smoothstep(0.05, 0.09, length(sc))) * step(0.72, sh)
            col = mix(col, vec3(1.0, 1.0, 1.0), flake * is_snow * (0.35 + precip * 0.55) * smoothstep(0.15, 0.6, uv.y) * art)

            // Low-frequency fog: a soft veil that thickens toward the ground.
            let fog_n = self.fbm(q * 1.4 + vec2(self.time * 0.006, 0.0))
            let fog_col = mix(vec3(0.42, 0.46, 0.50), vec3(0.80, 0.83, 0.86), day)
            col = mix(col, fog_col, is_fog * (0.35 + 0.3 * fog_n) * smoothstep(0.2, 0.95, uv.y) * art)

            // The text-zone scrim: from the flat palette under the zone,
            // as much black as white text needs (linear luminance 0.30 for
            // 3:1 large type, 0.183 for 4.5:1 small), from 0.10 once any
            // is needed, capped.
            let lin = flat * flat
            let lum = max(dot(lin, vec3(0.2126, 0.7152, 0.0722)), 0.001)
            let need_large = clamp(1.0 - 0.30 / lum, 0.0, 0.28)
            let need_small = clamp(1.0 - 0.183 / lum, 0.0, 0.40)
            let scrim_large = max(need_large, 0.10 * smoothstep(0.0, 0.03, need_large)) * large
            let scrim_small = max(need_small, 0.10 * smoothstep(0.0, 0.03, need_small)) * max(small, soft)
            col = col * (1.0 - max(scrim_large, scrim_small))

            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, self.border_radius)
            sdf.fill(vec4(col.x, col.y, col.z, 1.0))
            return sdf.result
        }
    }

    mod.widgets.SkyViewBase = #(SkyView::register_widget(vm))
    mod.widgets.SkyView = set_type_default() do mod.widgets.SkyViewBase {
        width: Fill height: Fill
        draw_sky: mod.widgets.DrawSky {}
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSky {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    kind: f32,
    #[live(1.0)]
    phase: f32,
    #[live(0.2)]
    cover: f32,
    #[live]
    precip: f32,
    #[live]
    time: f32,
    #[live(12.0)]
    border_radius: f32,
    #[live]
    zone0: Vec4f,
    #[live]
    zone1: Vec4f,
    #[live]
    zone2: Vec4f,
    #[live]
    zone3: Vec4f,
    #[live]
    zone4: Vec4f,
    #[live]
    zone5: Vec4f,
    #[live]
    zone6: Vec4f,
    #[live(14.0)]
    feather: f32,
    #[live(64.0)]
    soft_feather: f32,
}

/// How many large-text, small-text and soft (panel) zones the sky keeps.
pub const LARGE_ZONES: usize = 2;
pub const SMALL_ZONES: usize = 3;
pub const SOFT_ZONES: usize = 2;
const _: () = assert!(LARGE_ZONES + SMALL_ZONES + SOFT_ZONES == 7);

#[derive(Script, ScriptHook, Widget)]
pub struct SkyView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_sky: DrawSky,
}

impl SkyView {
    pub fn set_conditions(&mut self, cx: &mut Cx, inputs: SkyInputs) {
        let k = inputs.kind as i32 as f32;
        let (p, c, r) = (inputs.phase as f32, inputs.cover as f32, inputs.precip as f32);
        let d = &mut self.draw_sky;
        if d.kind != k || d.phase != p || d.cover != c || d.precip != r {
            d.kind = k;
            d.phase = p;
            d.cover = c;
            d.precip = r;
            d.redraw(cx);
        }
    }

    /// Where the text sits over this sky, in window points: large type,
    /// small type and the glass panels (soft). The art stays out of the
    /// text zones and thins under the soft ones, and each gets its scrim.
    /// Called after a draw; a change redraws once more.
    pub fn set_zones(&mut self, cx: &mut Cx, large: &[Rect], small: &[Rect], soft: &[Rect]) {
        let own = self.draw_sky.area().rect(cx);
        let local = |r: Option<&Rect>| -> Vec4f {
            match r {
                Some(r) if r.size.x > 0.0 && r.size.y > 0.0 => vec4(
                    ((r.pos.x - own.pos.x) * 2.0).round() as f32 * 0.5,
                    ((r.pos.y - own.pos.y) * 2.0).round() as f32 * 0.5,
                    (r.size.x * 2.0).round() as f32 * 0.5,
                    (r.size.y * 2.0).round() as f32 * 0.5,
                ),
                _ => vec4(0.0, 0.0, 0.0, 0.0),
            }
        };
        let next = [
            local(large.first()),
            local(large.get(1)),
            local(small.first()),
            local(small.get(1)),
            local(small.get(2)),
            local(soft.first()),
            local(soft.get(1)),
        ];
        let d = &mut self.draw_sky;
        let cur = [d.zone0, d.zone1, d.zone2, d.zone3, d.zone4, d.zone5, d.zone6];
        if cur != next {
            [d.zone0, d.zone1, d.zone2, d.zone3, d.zone4, d.zone5, d.zone6] = next;
            d.redraw(cx);
        }
    }

    /// The loading/unknown sky before any data: the unknown class by day.
    pub fn set_unknown(&mut self, cx: &mut Cx) {
        self.set_conditions(cx, SkyInputs { kind: SkyKind::Unknown, phase: 1.0, cover: 0.6, precip: 0.0 });
    }

    /// Advance the drift clock (seconds); the view calls this at 30 Hz
    /// only while the full face is on screen.
    pub fn set_time(&mut self, cx: &mut Cx, time: f64) {
        let t = time as f32;
        if (self.draw_sky.time - t).abs() > 0.001 {
            self.draw_sky.time = t;
            self.draw_sky.redraw(cx);
        }
    }
}

impl Widget for SkyView {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_sky.draw_walk(cx, walk);
        DrawStep::done()
    }
}
