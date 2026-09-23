//! Skeuomorphic surfaces: the Material shading library and the relief buffer.
//!
//! Every moulded part here is a `ReliefView`: its own bevel, face and cast
//! shadow are analytic (the Material Bench's dark "glossy" preset), and the
//! light the LEDs, the indicator dashes and the selected segment give their
//! neighbours comes from the window's relief buffer.
pub use makepad_widgets;

use makepad_widgets::*;

mod profile;

app_main!(App);

const LED_COUNT: usize = 16;
const LED_SWEEP: f64 = 2.35619;
const KNOB_BOX: f64 = 280.0;
const LED_RING: f64 = 64.0;
const LED_BOX: f64 = 30.0;

fn led_angle(i: usize) -> f64 {
    -LED_SWEEP + i as f64 * (2.0 * LED_SWEEP) / (LED_COUNT - 1) as f64
}

fn led_left(i: usize) -> f64 {
    KNOB_BOX * 0.5 + led_angle(i).sin() * LED_RING - LED_BOX * 0.5
}

fn led_top(i: usize) -> f64 {
    KNOB_BOX * 0.5 - led_angle(i).cos() * LED_RING - LED_BOX * 0.5
}

script_mod! {
    use mod.prelude.widgets.*

    // Every part here is this surface: the dark glossy material.
    let Surface = ReliefView{
        draw_bg +: {
            light_ink: vec4(1.0, 1.0, 1.0, 1.0)
            shadow_ink: vec4(0.0196, 0.0275, 0.0392, 1.0)
            glow_ink: vec4(0.302, 0.816, 0.882, 1.0)
            ground: #x191b1e
        }
    }

    // A small emitter: dark dimple when off, a lit bead with a halo when on.
    let Led = Surface{
        width: 30
        height: 30
        inset: 10
        disc: true
        depth: 0.3
        emissive: vec4(0.30, 0.82, 0.88, 3.2)
        draw_bg +: {
            ground: #x080809
            // A small bead: a short halo, so it dies well inside its quad.
            inner: vec4(0.72, 9.0, 0.0, 0.24)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let r = min(c.x, c.y) - self.inset
                var o = self.layer(self.ground.xyz)
                if self.lit > 0.001 {
                    let d = self.sd(p) + 0.4
                    let core = (1.0 - smoothstep(-px, px, d)) * self.lit
                    let hot = (1.0 - smoothstep(0.0, r * 1.2, -self.sd(p) - r * 0.35)) * self.disc
                    let col = mix(self.glow_ink.xyz * 1.15, vec3(0.92, 1.0, 1.0), (1.0 - hot) * 0.75 * self.disc + 0.25)
                    o = vec4(col * core, core) + o * (1.0 - core)
                    // The bead's own bloom, faded before the quad edge.
                    var edge = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
                    if self.disc > 0.5 {
                        edge = min(c.x, c.y) - length(p - c)
                    }
                    let bloom = exp(-max(self.sd(p), 0.0) / 4.0) * 0.55 * self.lit * smoothstep(0.0, self.inset, edge) * (1.0 - core)
                    o = o + vec4(self.glow_ink.xyz * bloom, bloom) * (1.0 - o.w)
                }
                return vec4(o.xyz + self.dither() * step(0.002, o.w), o.w)
            }
        }
    }

    // The Bluetooth key's under-cap light.
    let BtLight = Surface{
        margin: Inset{top: 98}
        width: 124
        height: 30
        inset: 13
        radius: 2
        depth: 4.0
        lit: 1.0
        emissive: vec4(0.24, 0.45, 1.0, 5.0)
        draw_bg +: {
            glow_ink: vec4(0.24, 0.45, 1.0, 1.0)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let d = self.sd(p)
                let edge = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
                let win = smoothstep(0.0, self.inset, edge)
                // The line: white-hot at its centre, blue at its ends.
                let core = 1.0 - smoothstep(-px, px, d)
                let along = abs(p.x - c.x) / max(c.x - self.inset, 1.0)
                let line = mix(vec3(0.75, 0.86, 1.0), self.glow_ink.xyz * 1.2, smoothstep(0.2, 1.0, along))
                // The bloom: wide and soft upward onto the cap, tighter below.
                let up = step(p.y, c.y)
                let reach = mix(5.0, 11.0, up)
                let bloom = exp(-max(d, 0.0) / reach) * 0.75 * win * (1.0 - core) * self.lit
                let a = core * self.lit
                let o = vec4(line * a, a) + vec4(self.glow_ink.xyz * bloom, bloom) * (1.0 - a)
                return vec4(o.xyz + self.dither() * step(0.002, o.w), o.w)
            }
        }
    }

    // The indicator dash under a switch.
    let Dash = Led{
        width: 64
        height: 24
        inset: 9
        radius: 2
        disc: false
        emissive: vec4(0.30, 0.82, 0.88, 9.0)
    }

    // A recessed panel: a shallow step down into the housing.
    let Panel = Surface{
        width: Fill
        height: Fill
        depth: -2.5
        radius: 12
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {
            ground: #x151719
            inner: vec4(0.9, 10.0, 0.0, 0.55)
            // The references' panels carry a faint noise texture.
            grain: 0.012
            // A large face: a whisper of the face gradient, a soft hairline.
            tune: vec4(0.12, 0.30, 1.10, 3.0)
        }
    }

    let SwitchLabel = Label{
        draw_text.color: #xd4d9df
        draw_text.text_style: theme.font_bold{font_size: 12}
    }

    let SwitchButton = Surface{
        width: 176
        height: 108
        inset: 22
        radius: 5
        depth: 2.5
        cursor: MouseCursor.Hand
        draw_bg +: {
            ground: #x1a1b1e
        }
    }

    let SegCell = Surface{
        width: 92
        height: 74
        inset: 1
        radius: 3
        depth: 1.5
        press: -4.5
        flow: Overlay
        align: Align{x: 0.5 y: 0.5}
        cursor: MouseCursor.Hand
        draw_bg +: {
            ground: #x191b1e
        }
    }

    // The glow behind a selected segment's icon: no face, only the halo and
    // the light it gives the cells either side.
    let SegGlow = Surface{
        width: 64
        height: 64
        inset: 16
        disc: true
        depth: 0.0
        emissive: vec4(0.30, 0.82, 0.88, 3.6)
        draw_bg +: {
            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let d = length(p - c) - (min(c.x, c.y) - self.inset)
                let edge = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
                let a = Material.tail(d, 8.0, 1.0) * 0.26 * self.lit * smoothstep(0.0, self.inset, edge)
                return vec4(self.glow_ink.xyz * a + self.dither() * step(0.002, a), a)
            }
        }
    }

    let SegIcon = Icon{
        draw_icon.color: #x101114
        icon_walk: Walk{width: 24 height: 24}
    }

    // The knob: one surface of revolution (the bench's revolve engine),
    // throwing its own cast shadow over the panel, turned by `value`.
    let Knob = Surface{
        width: 280
        height: 280
        inset: 94
        disc: true
        // The revolve stands R x profile depth tall (46 x 0.7).
        depth: 32.2
        cursor: MouseCursor.Hand
        draw_bg +: {
            ground: #x101113
            // A tall part: a darker, longer cast shadow and a firmer contact ring.
            shadow: vec4(1.0, 11.0, 1.0, 0.95)
            profile: texture_2d(float)
            value: uniform(0.34)
            pdepth: uniform(0.70)
            ptr_ink: uniform(vec4(0.96, 0.98, 1.0, 1.0))

            // .x slope (-dh/dr over the normalised radius), .y height,
            // .z the outermost radius still that high. Two nearest taps,
            // lerped by hand: float textures are not filtered everywhere.
            profile_at: fn(rn: float) -> vec3 {
                let x = clamp(rn, 0.0, 1.0) * 255.0
                let i0 = floor(x)
                let fr = x - i0
                let t0 = self.profile.sample_nearest(vec2((i0 + 0.5) / 256.0, 0.5))
                let t1 = self.profile.sample_nearest(vec2((min(i0 + 1.0, 255.0) + 0.5) / 256.0, 0.5))
                return mix(t0.xyz, t1.xyz, fr)
            }

            revolve_rmax: fn(z: float) -> float {
                return self.profile.sample_nearest(vec2((floor(clamp(z, 0.0, 1.0) * 255.0) + 0.5) / 256.0, 0.5)).z
            }

            // The bench's castRevolve: the knob laid down along the light,
            // eight slices by height, each swept over its eighth of the climb.
            cast_revolve: fn(q: vec2, r: float, sdir: vec2, len: float, blur: float, fall: float) -> float {
                var sh = 0.0
                var i = 0.0
                loop {
                    if i >= 8.0 { break }
                    let z0 = i / 8.0
                    let rr = self.revolve_rmax(z0 + 0.0625) * r
                    let pa = q - sdir * (len * z0)
                    let ba = sdir * (len * 0.125)
                    let tt = clamp(dot(pa, ba) / max(dot(ba, ba), 0.000001), 0.0, 1.0)
                    let qq = pa - ba * tt
                    let dc = length(qq) - rr
                    sh = max(sh, Material.tail(dc, blur * mix(0.3, 1.0, z0 + 0.125 * tt), fall) * step(0.5, rr))
                    i = i + 1.0
                }
                return sh
            }

            pixel: fn() {
                let p = self.pos * self.rect_size
                let c = self.rect_size * 0.5
                let q = p - c
                let r = min(c.x, c.y) - self.inset
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let d = length(q) - r
                var gs = vec2(0.0, 1.0)
                if length(q) > 0.001 {
                    gs = q / length(q)
                }
                let l = self.light
                let sdir = Material.shadow_dir(l)
                let tanel = max(l.z, 0.05) / max(length(l.xy), 0.05)
                let blur = max(self.shadow.y, 0.001)
                let fall = self.shadow.z
                let hk = r * max(self.pdepth, 0.001)
                let outside = smoothstep(-3.0 * px, 0.0, d)
                let edge = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
                let win = smoothstep(0.0, 30.0, edge)
                let dark = self.cast_revolve(q, r, sdir, hk / tanel, blur, fall) * outside * win
                let contact = exp(-max(d, 0.0) / (blur * 0.30)) * outside
                let a1 = clamp(dark * self.shadow.x + contact * self.shadow.w, 0.0, 1.0)
                let under = vec4(self.shadow_ink.xyz * a1, a1)

                // The revolve, filtered at the pixel footprint.
                let rn = clamp(1.0 + d / max(r, 0.001), 0.0, 1.0)
                let pw = 1.5 * px / max(r, 0.001)
                let pv = (self.profile_at(rn - pw) + self.profile_at(rn) + self.profile_at(rn + pw)) / 3.0
                let dome = pv.x * self.pdepth
                let pvy = pv.y

                // Self shadow: the rim standing between a point and the light.
                let ldir = normalize(l.xy + vec2(0.000001, 0.000001))
                let tanel2 = max(l.z, 0.02) / max(length(l.xy), 0.02)
                var occ = 0.0
                var i = 1.0
                loop {
                    if i > 8.0 { break }
                    let t = (i - 0.5) / 8.0 * r * 0.55
                    let q2 = q + ldir * t
                    let ht = self.profile_at(clamp(1.0 + (length(q2) - r) / max(r, 0.001), 0.0, 1.0)).y
                    occ = occ + clamp(((ht - pvy) * hk - t * tanel2) / max(hk * 0.25, 0.001), 0.0, 1.0)
                    i = i + 1.0
                }
                let selfsh = clamp(occ / 3.0, 0.0, 1.0) * step(d, 0.0)

                let k = r / 56.0
                let convex = self.relief.z * k
                let depth = self.relief.z * 1.2 * k
                let uv = q / (2.0 * r) + vec2(0.5, 0.5)
                let spec_scale = clamp(r / (px * 40.0), 0.15, 1.0)
                var face = Material.face(self.ground.xyz, d, gs, uv, depth, convex, dome, 0.0, 1.0, l, self.relief, self.finish, self.tune, self.inner.x, self.light_ink.xyz, self.shadow_ink.xyz, spec_scale)
                face = mix(face, self.shadow_ink.xyz, selfsh * self.shadow.x * 0.7)
                let n = normalize(vec3(gs * dome * convex, 1.0))
                face = face + self.env_spill(n)

                // The pointer: a dot at the value, on the 270 degree sweep.
                let spin = (self.value * 2.0 - 1.0) * 2.35619
                let dir = vec2(sin(spin), -cos(spin))
                let dot_r = max(7.0 * r / 56.0, 1.2) * 0.6
                let pd = length(q - dir * 0.62 * r) - dot_r
                face = mix(face, self.ptr_ink.xyz, 1.0 - smoothstep(-px, px, pd))

                let cov = 1.0 - smoothstep(-px, px, d)
                let dith = self.dither()
                face = face + dith
                let under2 = vec4(under.xyz + dith * step(0.002, under.w), under.w)
                return vec4(face * cov, cov) + under2 * (1.0 - cov)
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Skeuomorph"
                window.inner_size: vec2(1060, 740)
                pass.clear_color: #x18191c
                body +: {
                    backdrop := Surface{
                        width: Fill
                        height: Fill
                        proxy: false
                        flow: Right
                        padding: Inset{left: 36 right: 36 top: 36 bottom: 36}
                        spacing: 36
                        draw_bg +: {
                            ground: #x181a1c
                            grain: 0.022
                            pixel: fn() {
                                let v = length((self.pos - vec2(0.5, 0.42)) * vec2(1.0, 1.25))
                                var col = self.ground.xyz * (1.12 - 0.42 * v * v)
                                col = col * (1.0 - self.env_ao(0.0)) + self.env_spill(vec3(0.0, 0.0, 1.0))
                                return vec4(col + self.dither(), 1.0)
                            }
                        }

                        View{
                            width: Fill
                            height: Fill
                            flow: Down
                            spacing: 10

                            View{
                                width: Fill
                                height: 380
                                flow: Down
                                align: Align{x: 0.5 y: 0.5}

                                bt_house := Surface{
                                    width: 220
                                    height: 220
                                    inset: 32
                                    radius: 34
                                    depth: 3.0
                                    flow: Overlay
                                    align: Align{x: 0.5 y: 0.5}
                                    draw_bg +: {
                                        ground: #x101113
                                    }

                                    bt_well := Surface{
                                        width: 136
                                        height: 136
                                        depth: -3.0
                                        radius: 24
                                        flow: Overlay
                                        align: Align{x: 0.5 y: 0.0}
                                        draw_bg +: {
                                            ground: #x08090a
                                        }

                                        bt_cap := Surface{
                                            margin: Inset{top: 4}
                                            width: 128
                                            height: 116
                                            inset: 6
                                            radius: 20
                                            depth: 4.0
                                            flow: Overlay
                                            align: Align{x: 0.5 y: 0.5}
                                            draw_bg +: {
                                                ground: #x181a1c
                                            }

                                            Icon{
                                                draw_icon.svg: crate_resource("self:resources/bluetooth.svg")
                                                draw_icon.color: #x0b0c0e
                                                icon_walk: Walk{width: 34 height: 34}
                                            }
                                        }

                                        // The light under the cap: a thin line where the cap meets
                                        // the well, drawn over the cap's lower edge so its bloom
                                        // wraps up onto the cap and fades up its face.
                                        bt_strip := BtLight{}
                                    }
                                }

                                bt_led := Led{
                                    margin: Inset{top: 12}
                                    width: 44
                                    height: 44
                                    inset: 18
                                    lit: 1.0
                                    emissive: vec4(0.22, 0.42, 1.0, 3.0)
                                    draw_bg +: {
                                        glow_ink: vec4(0.22, 0.42, 1.0, 1.0)
                                    }
                                }
                            }

                            View{
                                width: Fill
                                height: Fill
                                flow: Right
                                align: Align{x: 0.5 y: 0.3}
                                spacing: 40

                                View{
                                    width: Fit
                                    height: Fit
                                    flow: Down
                                    align: Align{x: 0.5}
                                    SwitchLabel{text: "ON"}
                                    on_button := SwitchButton{}
                                    on_dash := Dash{lit: 1.0 margin: Inset{top: -14}}
                                }
                                View{
                                    width: Fit
                                    height: Fit
                                    flow: Down
                                    align: Align{x: 0.5}
                                    SwitchLabel{text: "OFF"}
                                    off_button := SwitchButton{}
                                    off_dash := Dash{lit: 0.0 margin: Inset{top: -14}}
                                }
                            }
                        }

                        View{
                            width: 480
                            height: Fill
                            flow: Down
                            spacing: 30

                            Panel{
                                height: 330
                                View{
                                    width: 280
                                    height: 280
                                    flow: Overlay

                                    knob := Knob{}
                                    led0 := Led{margin: Inset{left: #(led_left(0)) top: #(led_top(0))}}
                                    led1 := Led{margin: Inset{left: #(led_left(1)) top: #(led_top(1))}}
                                    led2 := Led{margin: Inset{left: #(led_left(2)) top: #(led_top(2))}}
                                    led3 := Led{margin: Inset{left: #(led_left(3)) top: #(led_top(3))}}
                                    led4 := Led{margin: Inset{left: #(led_left(4)) top: #(led_top(4))}}
                                    led5 := Led{margin: Inset{left: #(led_left(5)) top: #(led_top(5))}}
                                    led6 := Led{margin: Inset{left: #(led_left(6)) top: #(led_top(6))}}
                                    led7 := Led{margin: Inset{left: #(led_left(7)) top: #(led_top(7))}}
                                    led8 := Led{margin: Inset{left: #(led_left(8)) top: #(led_top(8))}}
                                    led9 := Led{margin: Inset{left: #(led_left(9)) top: #(led_top(9))}}
                                    led10 := Led{margin: Inset{left: #(led_left(10)) top: #(led_top(10))}}
                                    led11 := Led{margin: Inset{left: #(led_left(11)) top: #(led_top(11))}}
                                    led12 := Led{margin: Inset{left: #(led_left(12)) top: #(led_top(12))}}
                                    led13 := Led{margin: Inset{left: #(led_left(13)) top: #(led_top(13))}}
                                    led14 := Led{margin: Inset{left: #(led_left(14)) top: #(led_top(14))}}
                                    led15 := Led{margin: Inset{left: #(led_left(15)) top: #(led_top(15))}}
                                    Icon{
                                        margin: Inset{left: 133 top: 232}
                                        draw_icon.svg: crate_resource("self:resources/speaker.svg")
                                        draw_icon.color: #x0f1113
                                        icon_walk: Walk{width: 14 height: 14}
                                    }
                                }
                            }

                            Panel{
                                height: Fill
                                seg := Surface{
                                    width: Fit
                                    height: Fit
                                    inset: 12
                                    radius: 8
                                    depth: 1.5
                                    flow: Right
                                    padding: Inset{left: 16 right: 16 top: 16 bottom: 16}
                                    draw_bg +: {
                                        ground: #x0a0b0c
                                    }

                                    seg0 := SegCell{
                                        glow0 := SegGlow{}
                                        icon0 := SegIcon{draw_icon.svg: crate_resource("self:resources/grid.svg")}
                                    }
                                    seg1 := SegCell{
                                        glow1 := SegGlow{}
                                        icon1 := SegIcon{draw_icon.svg: crate_resource("self:resources/ball.svg")}
                                    }
                                    seg2 := SegCell{
                                        glow2 := SegGlow{}
                                        icon2 := SegIcon{draw_icon.svg: crate_resource("self:resources/image.svg")}
                                    }
                                    seg3 := SegCell{
                                        glow3 := SegGlow{}
                                        icon3 := SegIcon{draw_icon.svg: crate_resource("self:resources/clip.svg")}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    profile: Option<Texture>,
    #[rust(0.34)]
    knob_value: f64,
    #[rust]
    knob_drag: Option<(Vec2d, f64)>,
    #[rust(2usize)]
    selected: usize,
}

const SEGS: [(LiveId, LiveId, LiveId); 4] = [
    (live_id!(seg0), live_id!(glow0), live_id!(icon0)),
    (live_id!(seg1), live_id!(glow1), live_id!(icon1)),
    (live_id!(seg2), live_id!(glow2), live_id!(icon2)),
    (live_id!(seg3), live_id!(glow3), live_id!(icon3)),
];

fn led_id(i: usize) -> LiveId {
    LiveId::from_str(&format!("led{i}"))
}

impl App {
    fn view_action(&self, cx: &Cx, actions: &Actions, path: &[LiveId]) -> Option<ViewAction> {
        let uid = self.ui.widget(cx, path).widget_uid();
        actions.find_widget_action(uid).map(|a| a.cast::<ViewAction>())
    }

    fn tapped(&self, cx: &Cx, actions: &Actions, path: &[LiveId]) -> bool {
        matches!(self.view_action(cx, actions, path), Some(ViewAction::FingerUp(fe)) if fe.is_over)
    }

    fn apply_knob(&mut self, cx: &mut Cx) {
        let knob = self.ui.relief_view(cx, ids!(knob));
        if let Some(profile) = &self.profile {
            knob.set_bg_texture(cx, live_id!(profile), profile);
        }
        knob.set_bg_uniform(cx, live_id!(value), &[self.knob_value as f32]);
        let lit_to = self.knob_value * (LED_COUNT - 1) as f64 + 0.5;
        for i in 0..LED_COUNT {
            let on = if (i as f64) < lit_to { 1.0 } else { 0.0 };
            self.ui.relief_view(cx, &[led_id(i)]).set_lit(cx, on);
        }
    }

    fn apply_selection(&mut self, cx: &mut Cx) {
        for (i, (seg, glow, icon)) in SEGS.iter().enumerate() {
            let on = i == self.selected;
            self.ui.relief_view(cx, &[*seg]).set_held(cx, if on { 1.0 } else { 0.0 });
            self.ui.relief_view(cx, &[*glow]).set_lit(cx, if on { 1.0 } else { 0.0 });
            let color: Vec4f = if on {
                vec4(0.80, 0.97, 1.0, 1.0)
            } else {
                vec4(0.063, 0.067, 0.078, 1.0)
            };
            let mut icon = self.ui.widget(cx, &[*icon]);
            script_apply_eval!(cx, icon, { draw_icon +: { color: #(color) } });
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // SKEUO_NO_RELIEF=1: the same page without neighbour terms (the
        // draw-call comparison).
        if std::env::var_os("SKEUO_NO_RELIEF").is_some() {
            set_relief_enabled(cx, false);
        }
        self.profile = Some(profile::knob_profile_texture(cx));
        self.apply_knob(cx);
        self.apply_selection(cx);
    }

    // R redraws the whole page without moving anything: with
    // MAKEPAD_RELIEF_STATS=1 the log shows the relief buffer left alone.
    fn handle_key_down(&mut self, cx: &mut Cx, e: &KeyEvent) {
        if e.key_code == KeyCode::KeyR {
            cx.redraw_all();
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        for (button, dash) in [
            (live_id!(on_button), live_id!(on_dash)),
            (live_id!(off_button), live_id!(off_dash)),
        ] {
            if self.tapped(cx, actions, &[button]) {
                let dash = self.ui.relief_view(cx, &[dash]);
                let lit = if dash.lit() > 0.5 { 0.0 } else { 1.0 };
                dash.set_lit(cx, lit);
            }
        }
        for i in 0..SEGS.len() {
            if self.tapped(cx, actions, &[SEGS[i].0]) && self.selected != i {
                self.selected = i;
                self.apply_selection(cx);
            }
        }
        match self.view_action(cx, actions, ids!(knob)) {
            Some(ViewAction::FingerDown(fd)) => {
                self.knob_drag = Some((fd.abs, self.knob_value));
            }
            Some(ViewAction::FingerMove(fm)) => {
                if let Some((start, value)) = self.knob_drag {
                    let delta = (start.y - fm.abs.y) + (fm.abs.x - start.x);
                    self.knob_value = (value + delta / 220.0).clamp(0.0, 1.0);
                    self.apply_knob(cx);
                }
            }
            Some(ViewAction::FingerUp(_)) => {
                self.knob_drag = None;
            }
            _ => {}
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        // SKEUO_STATS=1: after every drawn frame, what the relief buffer did.
        if let Event::Draw(_) = event {
            if std::env::var_os("SKEUO_STATS").is_some() {
                log_frame_stats(cx);
            }
        }
    }
}

/// The relief counters and the body's draw calls, for the rest and batching
/// proofs.
fn log_frame_stats(cx: &Cx) {
    for window in cx.windows.id_iter() {
        let Some(root) = cx.windows[window]
            .main_pass_id
            .and_then(|pass| cx.passes[pass].main_draw_list_id)
        else {
            continue;
        };
        let mut calls = 0;
        let mut stack = vec![root];
        let mut seen = std::collections::HashSet::new();
        while let Some(list) = stack.pop() {
            if cx.draw_lists.is_id_freed(list) || !seen.insert(list) {
                continue;
            }
            let dl = &cx.draw_lists[list];
            for item in 0..dl.draw_items.len() {
                let kind = &dl.draw_items[item].kind;
                if let Some(sub) = kind.sub_list() {
                    stack.push(sub);
                } else if kind.draw_call().is_some() {
                    calls += 1;
                }
            }
        }
        match relief_stats(cx, window) {
            Some(st) => log!(
                "relief: proxies={} rebuilds={} source_painted={} source_dirty={} output_painted={} output_dirty={} body_draw_calls={}",
                st.proxies, st.rebuilds, st.source_painted, st.source_dirty, st.output_painted, st.output_dirty, calls
            ),
            None => log!("relief: off body_draw_calls={}", calls),
        }
    }
}
