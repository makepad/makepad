//! Presentation widgets: the crossfading video program view and the tile
//! grid shared by every catalog surface.
//!
//! `VideoProgram` composites the two playback-slot textures with an
//! aspect-preserving letterbox per source and a mix factor driven by the cue
//! engine's timed fade — one instance in the output window, one as the
//! console preview (textures are shared handles).
//!
//! `VjTileGrid` is the ai-content gallery pattern: PortalList rows of fixed
//! card slots. Recycled rows restart from the template with no texture, so
//! every visible pass rebinds thumbnails from the entry list — textures are
//! keyed upstream by immutable revision, never by list position.

use makepad_asset_data::AssetId;
use makepad_widgets::*;
use crate::gen::{GenJob, GenJobState, GenJobTone};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawProgram::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex_a: texture_2d(float)
        tex_b: texture_2d(float)

        fit_uv: fn(uv: vec2, aspect: float) -> vec3 {
            let ra = self.rect_size.x / max(self.rect_size.y, 1.0)
            let w = min(1.0, aspect / ra)
            let h = min(1.0, ra / aspect)
            let u = vec2(
                (uv.x - (1.0 - w) * 0.5) / w,
                (uv.y - (1.0 - h) * 0.5) / h
            )
            let inside = step(0.0, u.x) * step(u.x, 1.0)
                * step(0.0, u.y) * step(u.y, 1.0)
            return vec3(u.x, u.y, inside)
        }

        src: fn(uv: vec2) -> vec4 {
            let fa = self.fit_uv(uv, self.aspect_a)
            let ua = clamp(vec2(fa.x, fa.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let sa = self.tex_a.sample_as_bgra(ua)
            let ia = fa.z * self.has_a
            let aa = sa.w * ia
            let a = vec4(sa.xyz * aa, aa)
            let fb = self.fit_uv(uv, self.aspect_b)
            let ub = clamp(vec2(fb.x, fb.y), vec2(0.0, 0.0), vec2(1.0, 1.0))
            let sb = self.tex_b.sample_as_bgra(ub)
            let ib = fb.z * self.has_b
            let ba = sb.w * ib
            let b = vec4(sb.xyz * ba, ba)
            let fade = a.mix(b, self.mix_ab)
            let over = vec4(b.xyz + a.xyz * (1.0 - b.w), 1.0)
            let mixed = vec4(fade.xyz, 1.0)
            return mixed.mix(over, self.overlay)
        }

        pulse: fn(base: float, link: float) -> float {
            let hit = 1.0 - self.fx_beat
            return mix(base, mix(base * 0.12, base, hit), link)
        }

        pixel: fn() {
            let p1 = self.pulse(self.fx_p1, self.fx_link1)
            let p2 = self.pulse(self.fx_p2, self.fx_link2)
            let uv = self.pos
            let c = uv - vec2(0.5, 0.5)
            let kind = self.fx_kind
            let tau = 6.2831853
            let rgb = self.src(uv)
            if kind < 0.5 {
                return rgb
            }
            if kind < 1.5 {
                let segs = mix(2.0, 12.0, p1)
                let ang = atan2(c.y, c.x) + p2 * tau
                let slice = tau / max(segs, 2.0)
                let folded = abs(modf(ang, slice) - slice * 0.5)
                let r = length(c)
                return self.src(vec2(cos(folded), sin(folded)) * r + vec2(0.5, 0.5))
            }
            if kind < 2.5 {
                let r = length(c)
                let ang = atan2(c.y, c.x) / tau + p2 + self.fx_time * 0.04
                let z = 0.18 / max(r, 0.02) + p1 * 1.6 + self.fx_time * 0.12
                return self.src(vec2(fract(ang), fract(z)))
            }
            if kind < 3.5 {
                let axis = step(0.5, p1)
                let mu = vec2(abs(c.x), c.y) * axis + vec2(c.x, abs(c.y)) * (1.0 - axis)
                return rgb.mix(self.src(mu + vec2(0.5, 0.5)), p2)
            }
            if kind < 4.5 {
                let a = p2 * tau
                let off = vec2(cos(a), sin(a)) * p1 * 0.08
                return vec4(self.src(uv + off).x, rgb.y, self.src(uv - off).z, 1.0)
            }
            if kind < 5.5 {
                let rate = mix(1.0, 12.0, p1)
                let duty = mix(0.04, 0.7, p2)
                let gate = step(fract(self.fx_time * rate), duty)
                return vec4(rgb.xyz * gate, 1.0)
            }
            if kind < 6.5 {
                let cells = mix(8.0, 96.0, 1.0 - p1)
                let snapped = floor(uv * cells) / cells
                return self.src(mix(uv, snapped, mix(0.35, 1.0, p2)))
            }
            if kind < 7.5 {
                let r = length(c)
                let fall = 1.0 - smoothstep(0.0, mix(0.2, 1.1, p2), r)
                let a = p1 * 6.0 * fall
                let cs = cos(a)
                let sn = sin(a)
                let w = vec2(c.x * cs - c.y * sn, c.x * sn + c.y * cs)
                return self.src(w + vec2(0.5, 0.5))
            }
            if kind < 8.5 {
                let r = length(c)
                let wave = sin(r * mix(6.0, 40.0, p2) - self.fx_time * 6.0)
                let dir = c / max(r, 0.0001)
                return self.src(uv + dir * wave * p1 * 0.08)
            }
            if kind < 9.5 {
                let slices = mix(4.0, 28.0, p1)
                let row = floor(uv.y * slices)
                let n = fract(sin(row * 12.9898 + self.fx_time * 7.1) * 43758.5453)
                return self.src(vec2(fract(uv.x + (n - 0.5) * p2 * 0.35), uv.y))
            }
            if kind < 10.5 {
                let a = p1 * tau
                let k = vec3(0.57735027, 0.57735027, 0.57735027)
                let rotated = rgb.xyz * cos(a)
                    + cross(k, rgb.xyz) * sin(a)
                    + k * dot(k, rgb.xyz) * (1.0 - cos(a))
                let grey = vec3(dot(rotated, vec3(0.299, 0.587, 0.114)))
                return vec4(grey.mix(rotated, mix(0.0, 1.6, p2)), 1.0)
            }
            if kind < 11.5 {
                let punch = 1.0 + p2 * (1.0 - self.fx_beat) * 0.55
                let z = mix(1.0, 2.4, p1) * punch
                return self.src(c / z + vec2(0.5, 0.5))
            }
            let r = length(c)
            let f = 1.0 + r * r * mix(0.2, 2.4, p1)
            return rgb.mix(self.src(c * f + vec2(0.5, 0.5)), p2)
        }
    }

    mod.widgets.VideoProgramBase = #(VideoProgram::register_widget(vm))
    mod.widgets.VideoProgram = set_type_default() do mod.widgets.VideoProgramBase{
        width: Fill
        height: Fill
    }

    // Same as asset-ui SpriteFitImage: Smallest keeps authored aspect
    // (no stretch), nearest keeps Doom/Quake texels crisp. The parent
    // aligning box is what actually centers portrait/strip frames.
    let SpriteTileImage = Image{
        width: Fill
        height: Fill
        fit: ImageFit.Smallest
        draw_bg +: {
            get_color_scale_pan: fn(scale: vec2, pan: vec2) {
                if self.image_dim_w > 0.0 {
                    let angle = self.rotation * 3.141592653589793 / 180.0
                    let cos_a = cos(-angle)
                    let sin_a = sin(-angle)
                    let c = (self.pos - vec2(0.5, 0.5)) * self.rect_size
                    let cr = vec2(c.x * cos_a - c.y * sin_a, c.x * sin_a + c.y * cos_a)
                    let iuv = cr / vec2(self.image_dim_w, self.image_dim_h) + vec2(0.5, 0.5)
                    let uv = iuv * scale + pan
                    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
                        return vec4(0.0, 0.0, 0.0, 0.0)
                    }
                    return self.image_texture.sample_nearest(uv)
                }
                let uv = self.pos * scale + pan
                return self.image_texture.sample_nearest(uv)
            }
        }
    }
    let TileCell = RoundedView{
        width: 164
        height: 104
        padding: 0
        cursor: MouseCursor.Hand
        draw_bg +: {
            color: #x10141a
            border_color: #xffffff12
            border_color_selected: #x3ee0b0
            selected: instance(0.0)
            border_size: 1.0
            border_radius: 7.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, self.border_radius)
                sdf.fill_keep(self.color)
                let stroke = self.border_color.mix(self.border_color_selected, self.selected)
                sdf.stroke(stroke, self.border_size + self.selected)
                return sdf.result
            }
        }
        View{
            width: Fill
            height: Fill
            flow: Overlay
            View{
                width: Fill
                height: Fill
                align: Align{x: 0.5 y: 0.5}
                grid_thumb := SpriteTileImage{}
            }
            View{
                width: Fill
                height: Fill
                flow: Down
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 4
                    padding: Inset{left: 6.0 right: 6.0 top: 5.0 bottom: 0.0}
                    align: Align{x: 0.0, y: 0.5}
                    grid_pad := Label{
                        text: ""
                        draw_text.color: #x3ee0b0
                        draw_text.text_style: theme.font_bold{font_size: 8}
                    }
                    View{width: Fill height: 1}
                    grid_state := Label{
                        text: ""
                        draw_text.color: #x3ee0b0
                        draw_text.text_style.font_size: 8
                    }
                }
                View{width: Fill height: Fill}
                SolidView{
                    width: Fill
                    height: Fit
                    flow: Down
                    padding: Inset{left: 6.0 right: 6.0 top: 8.0 bottom: 5.0}
                    draw_bg.color: #x000000b8
                    grid_title := Label{
                        width: Fill
                        text: ""
                        draw_text.color: #xf2f6fa
                        draw_text.text_style.font_size: 9
                    }
                    grid_sub := Label{
                        width: Fill
                        text: ""
                        draw_text.color: #xb4bec8
                        draw_text.text_style.font_size: 8
                    }
                }
            }
        }
    }

    let JobProgressBar = SolidView{
        width: Fill
        height: 6
        draw_bg +: {
            progress: uniform(0.0)
            color_track: uniform(#x28313b)
            color_fill: uniform(#x4f9ee8)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 2.0)
                sdf.fill(self.color_track)
                let w = max(clamp(self.progress, 0.0, 1.0) * self.rect_size.x, 4.0)
                let visible = clamp(self.progress * 1000.0, 0.0, 1.0)
                sdf.box(0.0, 0.0, w, self.rect_size.y, 2.0)
                sdf.fill(vec4(self.color_fill.rgb, self.color_fill.a * visible))
                return sdf.result
            }
        }
    }

    mod.widgets.VjJobListBase = #(VjJobList::register_widget(vm))
    mod.widgets.VjJobList = set_type_default() do mod.widgets.VjJobListBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            spacing: 4
            JobRow := RoundedView{
                width: Fill
                height: Fit
                flow: Down
                spacing: 4
                padding: 7
                draw_bg +: {
                    color: #x12161c
                    border_color: #xffffff12
                    border_size: 1.0
                    border_radius: 6.0
                }
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    spacing: 6
                    align: Align{y: 0.5}
                    job_title := Label{
                        width: Fill
                        text: ""
                        draw_text.color: #xe5edf5
                        draw_text.text_style.font_size: 9
                    }
                    job_progress_text := Label{
                        text: ""
                        draw_text.color: #x73b9ef
                        draw_text.text_style.font_size: 8
                    }
                    job_cancel := Button{text: "Stop"}
                }
                job_stage := Label{visible: false width: Fill text: ""}
                job_message := Label{visible: false width: Fill text: ""}
                job_meta := Label{visible: false width: Fill text: ""}
                job_elapsed := Label{visible: false text: ""}
                job_progress := JobProgressBar{}
            }
            JobEmpty := View{
                width: Fill
                height: 28
                align: Align{x: 0.5, y: 0.5}
                Label{
                    text: "empty"
                    draw_text.color: #x66727f
                }
            }
        }
    }

    mod.widgets.VjTileGridBase = #(VjTileGrid::register_widget(vm))
    mod.widgets.VjTileGrid = set_type_default() do mod.widgets.VjTileGridBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill
            height: Fill
            flow: Down
            spacing: 6
            Row := View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 8
                c1 := TileCell{}
                c2 := TileCell{}
                c3 := TileCell{}
                c4 := TileCell{}
                c5 := TileCell{}
                c6 := TileCell{}
                c7 := TileCell{}
                c8 := TileCell{}
            }
            Empty := View{
                width: Fill
                height: 60
                align: Align{x: 0.5, y: 0.5}
                empty_label := Label{
                    text: "no results"
                    draw_text.color: #x66727f
                }
            }
        }
    }

    let PadCell = RoundedView{
        width: Fill
        height: 58
        padding: 0
        cursor: MouseCursor.Hand
        draw_bg +: {
            color: #x0c0e12
            border_color: #xffffff14
            border_color_selected: #x3ee0b0
            selected: instance(0.0)
            border_size: 1.0
            border_radius: 5.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(1.0, 1.0, self.rect_size.x - 2.0, self.rect_size.y - 2.0, self.border_radius)
                sdf.fill_keep(self.color)
                sdf.stroke(
                    self.border_color.mix(self.border_color_selected, self.selected),
                    self.border_size + self.selected
                )
                return sdf.result
            }
        }
        View{
            width: Fill
            height: Fill
            flow: Overlay
            View{
                width: Fill
                height: Fill
                align: Align{x: 0.5 y: 0.5}
                grid_thumb := SpriteTileImage{}
            }
            View{
                width: Fill
                height: Fill
                flow: Down
                padding: 3
                View{
                    width: Fill
                    height: Fit
                    flow: Right
                    grid_pad := Label{
                        text: ""
                        draw_text.color: #x3ee0b0cc
                        draw_text.text_style: theme.font_bold{font_size: 7}
                    }
                    View{width: Fill height: 1}
                    grid_state := Label{
                        text: ""
                        draw_text.color: #x3ee0b0
                        draw_text.text_style.font_size: 7
                    }
                }
                View{width: Fill height: Fill}
                grid_title := Label{
                    visible: false
                    width: Fill
                    text: ""
                }
                grid_sub := Label{
                    visible: false
                    width: Fill
                    text: ""
                }
            }
        }
    }

    mod.widgets.VjPadMatrixBase = #(VjPadMatrix::register_widget(vm))
    mod.widgets.VjPadMatrix = set_type_default() do mod.widgets.VjPadMatrixBase{
        width: Fill
        height: Fit
        flow: Down
        spacing: 4
        pad_filter := TextInput{
            width: Fill
            empty_text: "filter"
        }
        View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 6
            View{
                width: Fill
                height: Fit
                flow: Down
                spacing: 4
                r0 := View{
                    width: Fill height: Fit flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
                r1 := View{
                    width: Fill height: Fit flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
                r2 := View{
                    width: Fill height: Fit flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
                r3 := View{
                    width: Fill height: Fit flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
                r4 := View{
                    width: Fill height: Fit flow: Right spacing: 4
                    c1 := PadCell{} c2 := PadCell{} c3 := PadCell{} c4 := PadCell{}
                    c5 := PadCell{} c6 := PadCell{} c7 := PadCell{} c8 := PadCell{}
                }
            }
            scroll_slot := View{
                width: 10
                height: Fill
                cursor: MouseCursor.Default
            }
        }
        draw_track +: { color: #x161a20 }
        draw_thumb +: { color: #x3ee0b0 }
    }
}

// ---------------------------------------------------------------------------
// crossfading program view
// ---------------------------------------------------------------------------

/// Two-source crossfade blit. Per the draw-shader layout law, only `#[live]`
/// instance fields sit after the `#[deref]`.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawProgram {
    #[deref]
    pub draw_super: DrawQuad,
    #[live]
    pub mix_ab: f32,
    #[live(1.7777)]
    pub aspect_a: f32,
    #[live(1.7777)]
    pub aspect_b: f32,
    #[live]
    pub has_a: f32,
    #[live]
    pub has_b: f32,
    /// 0 = A/B crossfade, 1 = B over A (alpha).
    #[live]
    pub overlay: f32,
    #[live]
    pub fx_kind: f32,
    #[live]
    pub fx_p1: f32,
    #[live]
    pub fx_p2: f32,
    #[live]
    pub fx_link1: f32,
    #[live]
    pub fx_link2: f32,
    #[live]
    pub fx_beat: f32,
    #[live]
    pub fx_time: f32,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct VideoProgram {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_program: DrawProgram,
    #[rust]
    area: Area,
    #[rust]
    tex_a: Option<Texture>,
    #[rust]
    tex_b: Option<Texture>,
}

impl VideoProgram {
    /// Bind the slot textures + fade state for this frame. `mix_ab` 0 = all
    /// A, 1 = all B; aspects preserve each source's shape independently.
    /// `overlay` composites B over A instead of crossfading.
    pub fn set_sources(
        &mut self,
        cx: &mut Cx,
        tex_a: Option<(Texture, f32)>,
        tex_b: Option<(Texture, f32)>,
        mix_ab: f32,
        overlay: bool,
    ) {
        self.draw_program.has_a = if tex_a.is_some() { 1.0 } else { 0.0 };
        self.draw_program.has_b = if tex_b.is_some() { 1.0 } else { 0.0 };
        if let Some((tex, aspect)) = tex_a {
            self.draw_program.aspect_a = aspect.max(0.05);
            self.tex_a = Some(tex);
        } else {
            self.tex_a = None;
        }
        if let Some((tex, aspect)) = tex_b {
            self.draw_program.aspect_b = aspect.max(0.05);
            self.tex_b = Some(tex);
        } else {
            self.tex_b = None;
        }
        self.draw_program.mix_ab = mix_ab.clamp(0.0, 1.0);
        self.draw_program.overlay = if overlay { 1.0 } else { 0.0 };
        self.area.redraw(cx);
    }

    pub fn set_fx(&mut self, cx: &mut Cx, fx: crate::fx::FxState, beat: f32, time: f32) {
        self.draw_program.fx_kind = fx.kind.as_f32();
        self.draw_program.fx_p1 = fx.p1.clamp(0.0, 1.0);
        self.draw_program.fx_p2 = fx.p2.clamp(0.0, 1.0);
        self.draw_program.fx_link1 = if fx.beat1 { 1.0 } else { 0.0 };
        self.draw_program.fx_link2 = if fx.beat2 { 1.0 } else { 0.0 };
        self.draw_program.fx_beat = beat.clamp(0.0, 1.0);
        self.draw_program.fx_time = time;
        self.area.redraw(cx);
    }
}

impl WidgetNode for VideoProgram {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.area
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for VideoProgram {
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        match &self.tex_a {
            Some(tex) => self.draw_program.draw_vars.set_texture(0, tex),
            None => self.draw_program.draw_vars.empty_texture(0),
        }
        match &self.tex_b {
            Some(tex) => self.draw_program.draw_vars.set_texture(1, tex),
            None => self.draw_program.draw_vars.empty_texture(1),
        }
        self.draw_program.draw_abs(cx, rect);
        self.area = self.draw_program.area();
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// generation job list
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct JobRowEntry {
    /// Engine tag the cancel button reports back.
    pub tag: u64,
    pub title: String,
    pub stage: String,
    pub message: String,
    pub meta: String,
    pub elapsed: String,
    pub progress: Option<f32>,
    pub progress_text: String,
    pub tone: GenJobTone,
    pub cancellable: bool,
}

impl JobRowEntry {
    pub fn from_job(job: &GenJob, now_ms: u64) -> JobRowEntry {
        let display = job.display(now_ms);
        let progress = display.progress_permille.map(|value| value as f32 / 1000.0);
        let progress_text = match display.progress_permille {
            Some(value) => format!("{:.1}%", value as f32 / 10.0),
            None => match &job.state {
                GenJobState::Submitting => "SENDING".to_string(),
                GenJobState::Pending => "WAITING".to_string(),
                GenJobState::CancelRequested => "STOPPING".to_string(),
                GenJobState::Failed(_) => "FAILED".to_string(),
                GenJobState::Cancelled => "CANCELLED".to_string(),
                _ => "—".to_string(),
            },
        };
        JobRowEntry {
            tag: job.tag,
            title: job.title.clone(),
            stage: display.stage,
            message: display.message,
            meta: format!("profile: {} · {}", job.profile_label, display.assignment),
            elapsed: format_elapsed(display.elapsed_ms, job.state.is_terminal()),
            progress,
            progress_text,
            tone: display.tone,
            cancellable: !job.state.is_terminal()
                && !matches!(&job.state, GenJobState::CancelRequested),
        }
    }
}

fn format_elapsed(elapsed_ms: u64, terminal: bool) -> String {
    let total_seconds = elapsed_ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if terminal {
        format!("total {minutes}:{seconds:02}")
    } else {
        format!("elapsed {minutes}:{seconds:02}")
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjJobList {
    #[deref]
    view: View,
    #[rust]
    entries: Vec<JobRowEntry>,
}

impl VjJobList {
    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<JobRowEntry>) {
        if self.entries != entries {
            self.entries = entries;
            self.view.redraw(cx);
        }
    }

    pub fn entry_at(&self, index: usize) -> Option<&JobRowEntry> {
        self.entries.get(index)
    }
}

impl Widget for VjJobList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else { continue };
            if self.entries.is_empty() {
                list.set_item_range(cx, 0, 1);
                while let Some(row_id) = list.next_visible_item(cx) {
                    if row_id >= 1 {
                        continue;
                    }
                    let item = list.item(cx, row_id, id!(JobEmpty));
                    item.draw_all(cx, &mut Scope::empty());
                }
                continue;
            }
            list.set_item_range(cx, 0, self.entries.len());
            while let Some(row_id) = list.next_visible_item(cx) {
                if row_id >= self.entries.len() {
                    continue;
                }
                let item = list.item(cx, row_id, id!(JobRow));
                if let Some(entry) = self.entries.get(row_id) {
                    item.label(cx, ids!(job_title)).set_text(cx, &entry.title);
                    item.label(cx, ids!(job_stage)).set_text(cx, &entry.stage);
                    let message = item.label(cx, ids!(job_message));
                    message.set_text(cx, &entry.message);
                    message.set_visible(cx, !entry.message.is_empty());
                    item.label(cx, ids!(job_meta)).set_text(cx, &entry.meta);
                    item.label(cx, ids!(job_elapsed)).set_text(cx, &entry.elapsed);
                    item.label(cx, ids!(job_progress_text))
                        .set_text(cx, &entry.progress_text);
                    let bar = item.view(cx, ids!(job_progress));
                    bar.set_uniform(cx, live_id!(progress), &[entry.progress.unwrap_or(0.0)]);
                    let fill: [f32; 4] = match entry.tone {
                        GenJobTone::Waiting => [0.76, 0.58, 0.24, 1.0],
                        GenJobTone::Active => [0.31, 0.62, 0.91, 1.0],
                        GenJobTone::Success => [0.35, 0.77, 0.63, 1.0],
                        GenJobTone::Failed => [0.88, 0.34, 0.31, 1.0],
                        GenJobTone::Cancelled => [0.42, 0.47, 0.53, 1.0],
                    };
                    bar.set_uniform(cx, live_id!(color_fill), &fill);
                    item.button(cx, ids!(job_cancel))
                        .set_visible(cx, entry.cancellable);
                }
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// tile grid
// ---------------------------------------------------------------------------

pub const GRID_SLOTS: usize = 8;
const CARD_W: f64 = 164.0;
const CARD_SPACING: f64 = 8.0;

#[derive(Clone)]
pub struct GridEntry {
    pub asset: AssetId,
    pub title: String,
    pub sub: String,
    pub state: String,
    pub pad: String,
    pub texture: Option<Texture>,
    pub frames: Vec<Texture>,
    pub fps: f32,
    pub active: bool,
}

fn entry_frame(entry: &GridEntry, time: f64) -> Option<Texture> {
    if entry.frames.len() > 1 {
        let fps = entry.fps.max(1.0) as f64;
        let i = ((time * fps).floor() as usize) % entry.frames.len();
        return Some(entry.frames[i].clone());
    }
    entry.texture.clone()
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjTileGrid {
    #[deref]
    view: View,
    #[rust]
    entries: Vec<GridEntry>,
    #[rust(1usize)]
    pub last_cols: usize,
    #[rust]
    anim_frame: NextFrame,
}

impl VjTileGrid {
    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<GridEntry>) {
        self.entries = entries;
        self.view.redraw(cx);
    }

    pub fn set_thumb(&mut self, cx: &mut Cx, asset: AssetId, texture: Texture) {
        self.set_thumb_anim(cx, asset, vec![texture], 0.0);
    }

    pub fn set_thumb_anim(&mut self, cx: &mut Cx, asset: AssetId, frames: Vec<Texture>, fps: f32) {
        let mut hit = false;
        for entry in &mut self.entries {
            if entry.asset == asset {
                entry.texture = frames.first().cloned();
                entry.frames = frames.clone();
                entry.fps = fps;
                hit = true;
            }
        }
        if hit {
            self.view.redraw(cx);
        }
    }

    pub fn has_anims(&self) -> bool {
        self.entries.iter().any(|e| e.frames.len() > 1)
    }

    pub fn entry_at(&self, index: usize) -> Option<&GridEntry> {
        self.entries.get(index)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Widget for VjTileGrid {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if self.anim_frame.is_event(event).is_some()
            && self.entries.iter().any(|e| e.frames.len() > 1)
        {
            self.view.redraw(cx);
            self.anim_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.entries.iter().any(|e| e.frames.len() > 1) {
            self.anim_frame = cx.new_next_frame();
        }
        let width = self.view.area().rect(cx).size.x;
        if width > CARD_W {
            self.last_cols = (((width + CARD_SPACING) / (CARD_W + CARD_SPACING)) as usize)
                .clamp(1, GRID_SLOTS);
        }
        let cols = self.last_cols.max(1);
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let list_ref = step.as_portal_list();
            let Some(mut list) = list_ref.borrow_mut() else { continue };
            if self.entries.is_empty() {
                list.set_item_range(cx, 0, 1);
                while let Some(row_id) = list.next_visible_item(cx) {
                    if row_id >= 1 {
                        continue;
                    }
                    let item = list.item(cx, row_id, id!(Empty));
                    item.draw_all(cx, &mut Scope::empty());
                }
                continue;
            }
            let rows = self.entries.len().div_ceil(cols);
            list.set_item_range(cx, 0, rows);
            let slots = [
                ids!(c1),
                ids!(c2),
                ids!(c3),
                ids!(c4),
                ids!(c5),
                ids!(c6),
                ids!(c7),
                ids!(c8),
            ];
            while let Some(row_id) = list.next_visible_item(cx) {
                // PortalList deliberately probes past a short item range to
                // fill the viewport. Instantiating one of those synthetic
                // rows gives it only hidden, zero-height cells, so it asks
                // for another forever and grows the widget tree without
                // bound. Only real catalog rows may create widgets.
                if row_id >= rows {
                    continue;
                }
                let item = list.item(cx, row_id, id!(Row));
                for (slot, path) in slots.iter().enumerate() {
                    let index = row_id * cols + slot;
                    let visible = slot < cols && index < self.entries.len();
                    item.view(cx, *path).set_visible(cx, visible);
                    if !visible {
                        continue;
                    }
                    let entry = &self.entries[index];
                    let mut cell = item.view(cx, *path);
                    cell.label(cx, ids!(grid_title)).set_text(cx, &entry.title);
                    cell.label(cx, ids!(grid_sub)).set_text(cx, &entry.sub);
                    cell.label(cx, ids!(grid_pad)).set_text(cx, &entry.pad);
                    let state = if entry.active {
                        format!("LIVE {}", entry.state)
                    } else {
                        entry.state.clone()
                    };
                    cell.label(cx, ids!(grid_state)).set_text(cx, &state);
                    // Recycled rows lose their texture: rebind every pass.
                    let now = cx.seconds_since_app_start();
                    let frame = entry_frame(entry, now);
                    cell.image(cx, ids!(grid_thumb)).set_texture(cx, frame);
                    let selected = if entry.active { 1.0 } else { 0.0 };
                    script_apply_eval!(cx, cell, {
                        draw_bg +: { selected: #(selected) }
                    });
                }
                item.draw_all(cx, &mut Scope::empty());
            }
        }
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// APC40 8×5 clip matrix
// ---------------------------------------------------------------------------

const PAD_ROWS: usize = 5;
const PAD_COLS: usize = 8;
pub const PAD_MATRIX: usize = PAD_ROWS * PAD_COLS;

pub fn filter_pad_indices(entries: &[GridEntry], filter: &str) -> Vec<usize> {
    let q = filter.trim().to_ascii_lowercase();
    if q.is_empty() {
        return (0..entries.len()).collect();
    }
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            entry.title.to_ascii_lowercase().contains(&q)
                || entry.sub.to_ascii_lowercase().contains(&q)
        })
        .map(|(index, _)| index)
        .collect()
}

pub fn clamp_pad_offset(offset: usize, filtered_len: usize) -> usize {
    if filtered_len <= PAD_MATRIX {
        return 0;
    }
    let max = ((filtered_len - PAD_MATRIX + PAD_COLS - 1) / PAD_COLS) * PAD_COLS;
    (offset / PAD_COLS * PAD_COLS).min(max)
}

#[derive(Script, ScriptHook, Widget)]
pub struct VjPadMatrix {
    #[deref]
    view: View,
    #[live]
    draw_track: DrawQuad,
    #[live]
    draw_thumb: DrawQuad,
    #[rust]
    entries: Vec<GridEntry>,
    #[rust]
    filtered: Vec<usize>,
    #[rust]
    filter: String,
    #[rust]
    pub bank: usize,
    #[rust]
    scroll_area: Area,
    #[rust]
    dragging: bool,
    #[rust]
    anim_frame: NextFrame,
}

impl VjPadMatrix {
    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<GridEntry>) {
        self.entries = entries;
        self.refilter();
        self.view.redraw(cx);
    }

    pub fn set_thumb(&mut self, cx: &mut Cx, asset: AssetId, texture: Texture) {
        self.set_thumb_anim(cx, asset, vec![texture], 0.0);
    }

    pub fn set_thumb_anim(&mut self, cx: &mut Cx, asset: AssetId, frames: Vec<Texture>, fps: f32) {
        let mut hit = false;
        for entry in &mut self.entries {
            if entry.asset == asset {
                entry.texture = frames.first().cloned();
                entry.frames = frames.clone();
                entry.fps = fps;
                hit = true;
            }
        }
        if hit {
            self.view.redraw(cx);
        }
    }

    pub fn visible_assets(&self) -> Vec<AssetId> {
        (0..40)
            .filter_map(|pad| self.visible_at(pad).map(|entry| entry.asset))
            .collect()
    }

    pub fn set_filter(&mut self, cx: &mut Cx, filter: String) {
        if self.filter == filter {
            return;
        }
        self.filter = filter;
        self.bank = 0;
        self.refilter();
        self.view.redraw(cx);
    }

    pub fn set_bank(&mut self, bank: usize) {
        self.bank = clamp_pad_offset(bank, self.filtered.len());
    }

    pub fn set_offset(&mut self, cx: &mut Cx, offset: usize) {
        let next = clamp_pad_offset(offset, self.filtered.len());
        if next != self.bank {
            self.bank = next;
            self.view.redraw(cx);
        }
    }

    pub fn nudge_rows(&mut self, cx: &mut Cx, rows: i32) {
        let delta = rows * PAD_COLS as i32;
        let next = (self.bank as i32 + delta).max(0) as usize;
        self.set_offset(cx, next);
    }

    pub fn entry_at(&self, index: usize) -> Option<&GridEntry> {
        self.filtered
            .get(index)
            .and_then(|source| self.entries.get(*source))
    }

    pub fn visible_at(&self, pad: usize) -> Option<&GridEntry> {
        self.entry_at(self.bank + pad)
    }

    pub fn len(&self) -> usize {
        self.filtered.len()
    }

    fn refilter(&mut self) {
        self.filtered = filter_pad_indices(&self.entries, &self.filter);
        self.bank = clamp_pad_offset(self.bank, self.filtered.len());
    }

    fn bind_pads(&mut self, cx: &mut Cx) {
        let rows = [ids!(r0), ids!(r1), ids!(r2), ids!(r3), ids!(r4)];
        let slots = [
            ids!(c1),
            ids!(c2),
            ids!(c3),
            ids!(c4),
            ids!(c5),
            ids!(c6),
            ids!(c7),
            ids!(c8),
        ];
        for (row, row_id) in rows.iter().enumerate() {
            let row_view = self.view.view(cx, *row_id);
            for (slot, path) in slots.iter().enumerate() {
                let pad = row * PAD_COLS + slot;
                let mut cell = row_view.view(cx, *path);
                cell.label(cx, ids!(grid_pad)).set_text(cx, &format!("{:02}", pad + 1));
                if let Some(entry) = self.visible_at(pad) {
                    cell.label(cx, ids!(grid_state))
                        .set_text(cx, if entry.active { "LIVE" } else { "" });
                    let now = cx.seconds_since_app_start();
                    let frame = entry_frame(entry, now);
                    cell.image(cx, ids!(grid_thumb)).set_texture(cx, frame);
                    let selected = if entry.active { 1.0 } else { 0.0 };
                    script_apply_eval!(cx, cell, {
                        draw_bg +: { selected: #(selected) }
                    });
                } else {
                    cell.label(cx, ids!(grid_state)).set_text(cx, "");
                    cell.image(cx, ids!(grid_thumb)).set_texture(cx, None);
                    script_apply_eval!(cx, cell, {
                        draw_bg +: { selected: 0.0 }
                    });
                }
            }
        }
    }
}

impl Widget for VjPadMatrix {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if self.anim_frame.is_event(event).is_some()
            && self.entries.iter().any(|e| e.frames.len() > 1)
        {
            self.view.redraw(cx);
            self.anim_frame = cx.new_next_frame();
        }
        if let Event::Scroll(scroll) = event {
            if self.view.area().rect(cx).contains(scroll.abs) {
                let step = if scroll.scroll.y > 0.5 {
                    1
                } else if scroll.scroll.y < -0.5 {
                    -1
                } else {
                    0
                };
                if step != 0 {
                    self.nudge_rows(cx, step);
                    scroll.handled_y.set(true);
                }
            }
        }
        match event.hits(cx, self.scroll_area) {
            Hit::FingerDown(fe) if fe.is_primary_hit() => {
                self.dragging = true;
                self.drag_to(cx, fe.abs.y);
            }
            Hit::FingerMove(fe) if self.dragging => self.drag_to(cx, fe.abs.y),
            Hit::FingerUp(_) => self.dragging = false,
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.entries.iter().any(|e| e.frames.len() > 1) {
            self.anim_frame = cx.new_next_frame();
        }
        self.bind_pads(cx);
        let step = self.view.draw_walk(cx, scope, walk);
        let slot = self.view.view(cx, ids!(scroll_slot)).area().rect(cx);
        if slot.size.y > 1.0 {
            self.draw_track.draw_abs(cx, slot);
            let rows = (self.filtered.len().div_ceil(PAD_COLS)).max(1);
            let visible = PAD_ROWS as f64;
            let total = rows as f64;
            let thumb_h = (slot.size.y * (visible / total)).clamp(16.0, slot.size.y);
            let travel = (slot.size.y - thumb_h).max(0.0);
            let max_off = clamp_pad_offset(usize::MAX, self.filtered.len()) as f64;
            let t = if max_off <= 0.0 {
                0.0
            } else {
                self.bank as f64 / max_off
            };
            let thumb = Rect {
                pos: dvec2(slot.pos.x, slot.pos.y + travel * t),
                size: dvec2(slot.size.x, thumb_h),
            };
            self.draw_thumb.draw_abs(cx, thumb);
            self.scroll_area = self.draw_track.area();
        }
        step
    }
}

impl VjPadMatrix {
    fn drag_to(&mut self, cx: &mut Cx, y: f64) {
        let slot = self.scroll_area.rect(cx);
        if slot.size.y <= 1.0 {
            return;
        }
        let t = ((y - slot.pos.y) / slot.size.y).clamp(0.0, 1.0);
        let max_off = clamp_pad_offset(usize::MAX, self.filtered.len());
        let offset = ((t * max_off as f64) / PAD_COLS as f64).round() as usize * PAD_COLS;
        self.set_offset(cx, offset);
    }
}

#[cfg(test)]
mod pad_matrix_tests {
    use super::*;

    fn entry(title: &str) -> GridEntry {
        GridEntry {
            asset: AssetId::from_bytes([0; 16]),
            title: title.to_string(),
            sub: String::new(),
            state: String::new(),
            pad: String::new(),
            texture: None,
            frames: Vec::new(),
            fps: 0.0,
            active: false,
        }
    }

    #[test]
    fn filter_and_offset_keep_a_fixed_midi_window() {
        let entries: Vec<_> = (0..50).map(|i| entry(&format!("clip {i}"))).collect();
        assert_eq!(filter_pad_indices(&entries, "").len(), 50);
        assert_eq!(filter_pad_indices(&entries, "clip 4").len(), 11);
        assert_eq!(clamp_pad_offset(0, 50), 0);
        assert_eq!(clamp_pad_offset(100, 50), 16);
        assert_eq!(clamp_pad_offset(7, 50), 0);
        assert_eq!(clamp_pad_offset(0, 10), 0);
    }
}
