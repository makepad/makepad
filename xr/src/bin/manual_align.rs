pub use makepad_widgets;

use makepad_widgets::makepad_draw::DrawVector;
use makepad_widgets::makepad_platform::{TextureFormat, TextureUpdated, XrDepthAlignHeightMap};
use makepad_widgets::*;
use makepad_xr::*;
use std::{
    env, fs,
    path::{Path, PathBuf},
};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let DrawHeightMap = set_type_default() do #(DrawHeightMap::script_shader(vm)){
        ..mod.draw.DrawQuad
        height_texture: texture_2d(float)
        alpha: 0.9
        uv_min: vec2(0.0, 0.0)
        uv_max: vec2(1.0, 1.0)
        tint: vec4(0.82, 0.94, 1.0, 1.0)
        wall_band_start: 0.72

        pixel: fn() {
            let uv = self.uv_min + (self.uv_max - self.uv_min) * self.pos
            let sample = self.height_texture.sample(uv).x
            if sample <= 0.00001 {
                return #0000
            }
            let wall_mix = clamp(
                (sample - self.wall_band_start) / max(1.0 - self.wall_band_start, 0.0001),
                0.0,
                1.0
            )
            let lifted = pow(sample, mix(1.24, 0.76, wall_mix))
            let base = self.tint.xyz * (0.18 + lifted * 0.82)
            let wall = mix(base, self.tint.xyz, wall_mix * 0.75)
            return Pal.premul(vec4(wall, self.alpha * self.tint.w))
        }
    }

    let ManualAlignPreviewBase = #(ManualAlignPreview::register_widget(vm))
    let ManualAlignPreview = set_type_default() do ManualAlignPreviewBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #x09141d
        }
    }

    load_all_resources() do #(App::script_component(vm)) {
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1440, 920)
                pass.clear_color: #x0b1118
                body +: {
                    width: Fill
                    height: Fill
                    flow: Down
                    spacing: 12
                    padding: Inset{top: 18 bottom: 18 left: 18 right: 18}

                    RoundedView{
                        width: Fill
                        height: Fit
                        flow: Down
                        spacing: 6
                        padding: Inset{top: 16 bottom: 16 left: 16 right: 16}
                        draw_bg.color: #x16223c
                        draw_bg.border_radius: 12.0

                        Label{
                            text: "XR Manual Align"
                            draw_text.color: #xfff
                            draw_text.text_style: theme.font_bold{font_size: 20}
                        }

                        dump_path_label := Label{
                            text: "Dump: loading"
                            draw_text.color: #xb9c5eb
                            draw_text.text_style: theme.font_regular{font_size: 10}
                        }

                        save_status_label := Label{
                            text: "Save: waiting"
                            draw_text.color: #x8fd7b7
                            draw_text.text_style: theme.font_regular{font_size: 10}
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 10

                        shift_x_slider := Slider{
                            width: Fill
                            text: "X Shift (m)"
                            min: -8.0
                            max: 8.0
                            step: 0.01
                            precision: 1000
                            default: 0.0
                        }

                        shift_y_slider := Slider{
                            width: Fill
                            text: "Y Shift (m)"
                            min: -8.0
                            max: 8.0
                            step: 0.01
                            precision: 1000
                            default: 0.0
                        }

                        rotate_slider := Slider{
                            width: Fill
                            text: "Rotate (deg)"
                            min: -180.0
                            max: 180.0
                            step: 0.1
                            precision: 100
                            default: 0.0
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 10

                        band_min_slider := Slider{
                            width: Fill
                            text: "Band Min (m)"
                            min: -0.20
                            max: 2.20
                            step: 0.01
                            precision: 1000
                            default: 0.00
                        }

                        band_max_slider := Slider{
                            width: Fill
                            text: "Band Max (m)"
                            min: -0.20
                            max: 2.20
                            step: 0.01
                            precision: 1000
                            default: 2.00
                        }
                    }

                    View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 10
                        align: Align{y: 0.5}

                        save_button := Button{
                            width: 110
                            text: "Save"
                            draw_bg +: {
                                color: uniform(#x355fd1)
                                color_hover: uniform(#x4875ef)
                                color_down: uniform(#x284eb5)
                                border_radius: 8.0
                            }
                            draw_text +: {
                                color: #xfff
                                text_style +: {font_size: 11}
                            }
                        }

                        pose_label := Label{
                            width: Fill
                            text: "Pose: x 0.00 | y 0.00 | rot 0.0 deg"
                            draw_text.color: #xffd29a
                            draw_text.text_style: theme.font_regular{font_size: 11}
                        }
                    }

                    RoundedView{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 8
                        padding: Inset{top: 10 bottom: 10 left: 10 right: 10}
                        draw_bg.color: #x0d1424
                        draw_bg.border_radius: 12.0

                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 10

                            Label{
                                width: Fill
                                text: "Local"
                                draw_text.color: #x9ed5ff
                                draw_text.text_style: theme.font_bold{font_size: 11}
                            }

                            Label{
                                width: Fill
                                text: "Remote"
                                draw_text.color: #xffbb7e
                                draw_text.text_style: theme.font_bold{font_size: 11}
                            }

                            Label{
                                width: Fill
                                text: "Overlay"
                                draw_text.color: #xffe7a1
                                draw_text.text_style: theme.font_bold{font_size: 11}
                            }
                        }

                        preview := ManualAlignPreview{}
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawHeightMap {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    alpha: f32,
    #[live]
    uv_min: Vec2f,
    #[live]
    uv_max: Vec2f,
    #[live]
    tint: Vec4f,
    #[live]
    wall_band_start: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ManualAlignPose {
    shift_x_meters: f32,
    shift_y_meters: f32,
    rotation_radians: f32,
}

impl ManualAlignPose {
    fn rotation_degrees(self) -> f32 {
        self.rotation_radians.to_degrees()
    }

    fn to_mat4(self) -> Mat4f {
        Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), self.rotation_radians),
            vec3(self.shift_x_meters, 0.0, self.shift_y_meters),
        )
        .to_mat4()
    }
}

#[derive(Clone, Debug, Default)]
struct ManualAlignSidecar {
    dump_file: String,
    shift_x_meters: f32,
    shift_y_meters: f32,
    rotation_radians: f32,
}

impl ManualAlignSidecar {
    fn from_pose(dump_path: &Path, pose: ManualAlignPose) -> Self {
        Self {
            dump_file: dump_path.display().to_string(),
            shift_x_meters: pose.shift_x_meters,
            shift_y_meters: pose.shift_y_meters,
            rotation_radians: pose.rotation_radians,
        }
    }

    fn pose(&self) -> ManualAlignPose {
        ManualAlignPose {
            shift_x_meters: self.shift_x_meters,
            shift_y_meters: self.shift_y_meters,
            rotation_radians: self.rotation_radians,
        }
    }

    fn to_text(&self) -> String {
        format!(
            "dump_file: {}\nshift_x_meters: {:.6}\nshift_y_meters: {:.6}\nrotation_radians: {:.6}\n",
            self.dump_file,
            self.shift_x_meters,
            self.shift_y_meters,
            self.rotation_radians
        )
    }

    fn from_text(text: &str) -> Option<Self> {
        let mut sidecar = Self::default();
        for line in text.lines() {
            let (key, value) = line.split_once(':')?;
            let value = value.trim();
            match key.trim() {
                "dump_file" => sidecar.dump_file = value.to_string(),
                "shift_x_meters" => sidecar.shift_x_meters = value.parse().ok()?,
                "shift_y_meters" => sidecar.shift_y_meters = value.parse().ok()?,
                "rotation_radians" => sidecar.rotation_radians = value.parse().ok()?,
                _ => {}
            }
        }
        Some(sidecar)
    }
}

#[derive(Clone)]
struct LoadedDump {
    dump_path: PathBuf,
    sidecar_path: PathBuf,
    pair: XrNetAlignmentDescriptorDumpPair,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PreviewBounds {
    min_x: f32,
    max_x: f32,
    min_z: f32,
    max_z: f32,
}

impl PreviewBounds {
    fn span_x(self) -> f32 {
        (self.max_x - self.min_x).max(1.0e-5)
    }

    fn span_z(self) -> f32 {
        (self.max_z - self.min_z).max(1.0e-5)
    }

    fn expand(mut self, amount: f32) -> Self {
        self.min_x -= amount;
        self.max_x += amount;
        self.min_z -= amount;
        self.max_z += amount;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HeightBandPass {
    min_meters: f32,
    max_meters: f32,
}

impl Default for HeightBandPass {
    fn default() -> Self {
        Self {
            min_meters: 0.0,
            max_meters: 2.0,
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ManualAlignPreview {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[redraw]
    #[live]
    draw_map: DrawHeightMap,
    #[redraw]
    #[live]
    draw_vector: DrawVector,
    #[rust]
    area: Area,
    #[rust]
    local_texture: Option<Texture>,
    #[rust]
    remote_texture: Option<Texture>,
    #[rust]
    local_height_map: Option<XrDepthAlignHeightMap>,
    #[rust]
    transformed_remote_height_map: Option<XrDepthAlignHeightMap>,
    #[rust]
    preview_bounds: Option<PreviewBounds>,
    #[rust]
    band_pass: HeightBandPass,
    #[rust(1.18)]
    zoom: f32,
}

impl ManualAlignPreview {
    const PAD: f32 = 12.0;
    const PANEL_GAP: f32 = 10.0;
    const GRID_DIVISIONS: usize = 4;
    const CUTOUT_STEPS: usize = 40;

    fn set_height_maps(
        &mut self,
        cx: &mut Cx,
        local_height_map: Option<XrDepthAlignHeightMap>,
        transformed_remote_height_map: Option<XrDepthAlignHeightMap>,
        preview_bounds: Option<PreviewBounds>,
        band_pass: HeightBandPass,
    ) {
        if self.local_height_map == local_height_map
            && self.transformed_remote_height_map == transformed_remote_height_map
            && self.preview_bounds == preview_bounds
            && self.band_pass == band_pass
        {
            return;
        }
        self.local_height_map = local_height_map;
        self.transformed_remote_height_map = transformed_remote_height_map;
        self.preview_bounds = preview_bounds;
        self.band_pass = band_pass;
        if self.local_height_map.is_none() {
            self.local_texture = None;
        }
        if self.transformed_remote_height_map.is_none() {
            self.remote_texture = None;
        }
        self.area.redraw(cx);
    }

    fn preview_height_to_u8(
        height_map: &XrDepthAlignHeightMap,
        band_pass: HeightBandPass,
        value: f32,
    ) -> u8 {
        if !value.is_finite() {
            0
        } else {
            let relative_height = value - height_map.floor_y_meters;
            if relative_height < band_pass.min_meters || relative_height > band_pass.max_meters {
                return 0;
            }
            let span = (band_pass.max_meters - band_pass.min_meters).max(1.0e-5);
            let normalized = ((relative_height - band_pass.min_meters) / span).clamp(0.0, 1.0);
            1 + (normalized * 254.0).round() as u8
        }
    }

    fn ensure_height_texture(
        texture_slot: &mut Option<Texture>,
        cx: &mut Cx,
        height_map: &XrDepthAlignHeightMap,
        band_pass: HeightBandPass,
    ) {
        let map_width = height_map.size_x as usize;
        let map_height = height_map.size_z as usize;
        if map_width == 0
            || map_height == 0
            || height_map.heights_meters.len() != map_width * map_height
        {
            *texture_slot = None;
            return;
        }

        let needs_recreate = texture_slot.as_ref().is_none_or(|texture| {
            !matches!(
                texture.get_format(cx),
                TextureFormat::VecRu8 { width, height, .. }
                    if *width == map_width && *height == map_height
            )
        });

        let pixels = height_map
            .heights_meters
            .iter()
            .map(|value| Self::preview_height_to_u8(height_map, band_pass, *value))
            .collect::<Vec<_>>();

        if needs_recreate {
            *texture_slot = Some(Texture::new_with_format(
                cx,
                TextureFormat::VecRu8 {
                    width: map_width,
                    height: map_height,
                    data: Some(pixels),
                    unpack_row_length: None,
                    updated: TextureUpdated::Full,
                },
            ));
            return;
        }

        if let Some(texture) = texture_slot.as_ref() {
            let mut data = texture.take_vec_u8(cx);
            if data.len() != pixels.len() {
                data.resize(pixels.len(), 0);
            }
            data.copy_from_slice(&pixels);
            texture.put_back_vec_u8(cx, data, None);
        }
    }

    fn map_bounds(height_map: &XrDepthAlignHeightMap) -> Option<PreviewBounds> {
        let span_x = height_map.extent_x_meters();
        let span_z = height_map.extent_z_meters();
        if span_x <= 1.0e-5 || span_z <= 1.0e-5 {
            return None;
        }
        Some(PreviewBounds {
            min_x: height_map.origin_x,
            max_x: height_map.origin_x + span_x,
            min_z: height_map.origin_z,
            max_z: height_map.origin_z + span_z,
        })
    }

    fn preview_rect(rect: Rect, bounds: PreviewBounds, zoom: f32) -> Rect {
        let available_w = (rect.size.x as f32 - Self::PAD * 2.0).max(1.0);
        let available_h = (rect.size.y as f32 - Self::PAD * 2.0).max(1.0);
        let scale = (available_w / bounds.span_x()).min(available_h / bounds.span_z())
            * zoom.max(0.2).max(1.0e-5);
        let draw_w = bounds.span_x() * scale;
        let draw_h = bounds.span_z() * scale;
        Rect {
            pos: dvec2(
                (rect.pos.x as f32 + (rect.size.x as f32 - draw_w) * 0.5) as f64,
                (rect.pos.y as f32 + (rect.size.y as f32 - draw_h) * 0.5) as f64,
            ),
            size: dvec2(draw_w as f64, draw_h as f64),
        }
    }

    fn split_panel_rects(rect: Rect) -> [Rect; 3] {
        let panel_gap = Self::PANEL_GAP;
        let inner_w = (rect.size.x as f32 - panel_gap * 2.0).max(3.0);
        let panel_w = inner_w / 3.0;
        [
            Rect {
                pos: rect.pos,
                size: dvec2(panel_w as f64, rect.size.y),
            },
            Rect {
                pos: dvec2(rect.pos.x + (panel_w + panel_gap) as f64, rect.pos.y),
                size: dvec2(panel_w as f64, rect.size.y),
            },
            Rect {
                pos: dvec2(
                    rect.pos.x + ((panel_w + panel_gap) * 2.0) as f64,
                    rect.pos.y,
                ),
                size: dvec2(panel_w as f64, rect.size.y),
            },
        ]
    }

    fn world_to_preview(bounds: PreviewBounds, preview_rect: Rect, point: Vec2f) -> (f32, f32) {
        let nx = (point.x - bounds.min_x) / bounds.span_x();
        let nz = (point.y - bounds.min_z) / bounds.span_z();
        (
            preview_rect.pos.x as f32 + nx * preview_rect.size.x as f32,
            preview_rect.pos.y as f32 + nz * preview_rect.size.y as f32,
        )
    }

    fn height_map_draw_rect(
        bounds: PreviewBounds,
        preview_rect: Rect,
        height_map: &XrDepthAlignHeightMap,
    ) -> Rect {
        let (left, top) = Self::world_to_preview(
            bounds,
            preview_rect,
            vec2f(height_map.origin_x, height_map.origin_z),
        );
        let (right, bottom) = Self::world_to_preview(
            bounds,
            preview_rect,
            vec2f(
                height_map.origin_x + height_map.extent_x_meters(),
                height_map.origin_z + height_map.extent_z_meters(),
            ),
        );
        Rect {
            pos: dvec2(left as f64, top as f64),
            size: dvec2(
                (right - left).max(1.0) as f64,
                (bottom - top).max(1.0) as f64,
            ),
        }
    }

    fn draw_grid(&mut self, preview_rect: Rect) {
        let ox = preview_rect.pos.x as f32;
        let oy = preview_rect.pos.y as f32;
        let w = preview_rect.size.x as f32;
        let h = preview_rect.size.y as f32;
        self.draw_vector.set_color_hex(0x163042, 1.0);
        for step in 0..=Self::GRID_DIVISIONS {
            let t = step as f32 / Self::GRID_DIVISIONS as f32;
            let px = ox + w * t;
            let py = oy + h * t;
            self.draw_vector.move_to(px, oy);
            self.draw_vector.line_to(px, oy + h);
            self.draw_vector.move_to(ox, py);
            self.draw_vector.line_to(ox + w, py);
        }
        self.draw_vector.stroke(1.0);
    }

    fn draw_origin_cross(&mut self, bounds: PreviewBounds, preview_rect: Rect) {
        if bounds.min_x > 0.0 || bounds.max_x < 0.0 || bounds.min_z > 0.0 || bounds.max_z < 0.0 {
            return;
        }
        let (cx, cy) = Self::world_to_preview(bounds, preview_rect, vec2f(0.0, 0.0));
        self.draw_vector.set_color_hex(0xffcf6a, 1.0);
        self.draw_vector.move_to(cx - 6.0, cy);
        self.draw_vector.line_to(cx + 6.0, cy);
        self.draw_vector.move_to(cx, cy - 6.0);
        self.draw_vector.line_to(cx, cy + 6.0);
        self.draw_vector.stroke(1.2);
    }

    fn draw_cutout_ring(
        &mut self,
        bounds: PreviewBounds,
        preview_rect: Rect,
        height_map: &XrDepthAlignHeightMap,
        color_hex: u32,
    ) {
        let Some(center) = height_map.player_cutout_center else {
            return;
        };
        let (cx, cy) = Self::world_to_preview(bounds, preview_rect, center);
        let scale = preview_rect.size.x as f32 / bounds.span_x();
        let radius = height_map.player_cutout_radius_meters * scale;
        self.draw_vector.set_color_hex(color_hex, 1.0);
        for step in 0..=Self::CUTOUT_STEPS {
            let angle = step as f32 / Self::CUTOUT_STEPS as f32 * std::f32::consts::TAU;
            let px = cx + angle.cos() * radius;
            let py = cy + angle.sin() * radius;
            if step == 0 {
                self.draw_vector.move_to(px, py);
            } else {
                self.draw_vector.line_to(px, py);
            }
        }
        self.draw_vector.stroke(1.2);
    }

    fn draw_height_map_rect(
        &mut self,
        cx: &mut Cx2d,
        texture: &Texture,
        rect: Rect,
        alpha: f32,
        tint: Vec4f,
    ) {
        self.draw_map.alpha = alpha;
        self.draw_map.tint = tint;
        self.draw_map.uv_min = vec2f(0.0, 0.0);
        self.draw_map.uv_max = vec2f(1.0, 1.0);
        self.draw_map.draw_vars.set_texture(0, texture);
        self.draw_map.draw_abs(cx, rect);
    }

    fn draw_panel_contents(
        &mut self,
        cx: &mut Cx2d,
        panel_rect: Rect,
        bounds: PreviewBounds,
        draw_local: bool,
        draw_remote: bool,
        local_height_map: Option<&XrDepthAlignHeightMap>,
        remote_height_map: Option<&XrDepthAlignHeightMap>,
    ) {
        let preview_rect = Self::preview_rect(panel_rect, bounds, self.zoom);
        if draw_local {
            if let Some(local_height_map) = local_height_map {
                Self::ensure_height_texture(
                    &mut self.local_texture,
                    cx.cx,
                    local_height_map,
                    self.band_pass,
                );
                if let Some(texture) = self.local_texture.as_ref().cloned() {
                    self.draw_height_map_rect(
                        cx,
                        &texture,
                        Self::height_map_draw_rect(bounds, preview_rect, local_height_map),
                        0.92,
                        vec4f(0.74, 0.96, 1.0, 0.92),
                    );
                }
            }
        }
        if draw_remote {
            if let Some(remote_height_map) = remote_height_map {
                Self::ensure_height_texture(
                    &mut self.remote_texture,
                    cx.cx,
                    remote_height_map,
                    self.band_pass,
                );
                if let Some(texture) = self.remote_texture.as_ref().cloned() {
                    self.draw_height_map_rect(
                        cx,
                        &texture,
                        Self::height_map_draw_rect(bounds, preview_rect, remote_height_map),
                        if draw_local { 0.74 } else { 0.90 },
                        vec4f(1.0, 0.62, 0.24, if draw_local { 0.78 } else { 0.92 }),
                    );
                }
            }
        }

        self.draw_vector.begin();
        self.draw_grid(preview_rect);
        self.draw_origin_cross(bounds, preview_rect);
        if draw_local {
            if let Some(local_height_map) = local_height_map {
                self.draw_cutout_ring(bounds, preview_rect, local_height_map, 0x9ed5ff);
            }
        }
        if draw_remote {
            if let Some(remote_height_map) = remote_height_map {
                self.draw_cutout_ring(bounds, preview_rect, remote_height_map, 0xffbb7e);
            }
        }
        self.draw_vector.end(cx);
    }
}

impl Widget for ManualAlignPreview {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();

        let local_height_map = self.local_height_map.clone();
        let transformed_remote_height_map = self.transformed_remote_height_map.clone();
        let Some(bounds) = self.preview_bounds else {
            return DrawStep::done();
        };
        let [local_rect, remote_rect, overlay_rect] = Self::split_panel_rects(rect);
        self.draw_panel_contents(
            cx,
            local_rect,
            bounds,
            true,
            false,
            local_height_map.as_ref(),
            transformed_remote_height_map.as_ref(),
        );
        self.draw_panel_contents(
            cx,
            remote_rect,
            bounds,
            false,
            true,
            local_height_map.as_ref(),
            transformed_remote_height_map.as_ref(),
        );
        self.draw_panel_contents(
            cx,
            overlay_rect,
            bounds,
            true,
            true,
            local_height_map.as_ref(),
            transformed_remote_height_map.as_ref(),
        );
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerScroll(fs) => {
                let scroll = if fs.scroll.y.abs() > f64::EPSILON {
                    fs.scroll.y
                } else {
                    fs.scroll.x
                };
                let factor = if scroll > 0.0 { 1.10 } else { 1.0 / 1.10 };
                self.zoom = (self.zoom * factor).clamp(0.45, 8.0);
                self.area.redraw(cx);
            }
            _ => {
                if matches!(event, Event::Signal) {
                    self.area.redraw(cx);
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
    loaded_dump: Option<LoadedDump>,
    #[rust]
    pose: ManualAlignPose,
    #[rust]
    band_pass: HeightBandPass,
}

impl App {
    fn initial_dump_path() -> Option<PathBuf> {
        env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .or_else(Self::latest_dump_path)
    }

    fn latest_dump_path() -> Option<PathBuf> {
        let mut entries = fs::read_dir("xr/util/dumps")
            .ok()?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                metadata
                    .is_file()
                    .then_some((entry.path(), metadata.modified().ok()?))
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| right.1.cmp(&left.1));
        entries.into_iter().find_map(|(path, _)| {
            let name = path.file_name()?.to_str()?;
            (name.ends_with(".bin") && name != "manual-smoke.bin").then_some(path)
        })
    }

    fn sidecar_path_for_dump(dump_path: &Path) -> PathBuf {
        let stem = dump_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("align-pair");
        dump_path.with_file_name(format!("{stem}.manual_pose.ron"))
    }

    fn load_sidecar_pose(sidecar_path: &Path) -> Option<ManualAlignPose> {
        let text = fs::read_to_string(sidecar_path).ok()?;
        ManualAlignSidecar::from_text(&text).map(|sidecar| sidecar.pose())
    }

    fn load_dump_pair(path: &Path) -> Result<LoadedDump, String> {
        let bytes =
            fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        let pair = XrNetAlignmentDescriptorDumpPair::from_file_bytes(&bytes)
            .ok_or_else(|| format!("failed to decode {}", path.display()))?;
        Ok(LoadedDump {
            dump_path: path.to_path_buf(),
            sidecar_path: Self::sidecar_path_for_dump(path),
            pair,
        })
    }

    fn current_preview_bounds(&self) -> Option<PreviewBounds> {
        let loaded_dump = self.loaded_dump.as_ref()?;
        let local_bounds = loaded_dump
            .pair
            .local_descriptor
            .descriptor
            .height_map
            .as_ref()
            .and_then(ManualAlignPreview::map_bounds);
        let remote_height_map = loaded_dump
            .pair
            .remote_descriptor
            .descriptor
            .height_map
            .as_ref()?;
        let remote_extent_x = remote_height_map.extent_x_meters();
        let remote_extent_z = remote_height_map.extent_z_meters();
        if remote_extent_x <= 1.0e-5 || remote_extent_z <= 1.0e-5 {
            return local_bounds.map(|bounds| bounds.expand(0.12));
        }
        let remote_center = vec3(
            remote_height_map.origin_x + remote_extent_x * 0.5,
            0.0,
            remote_height_map.origin_z + remote_extent_z * 0.5,
        );
        let transformed_center = self
            .pose
            .to_mat4()
            .transform_vec4(vec4f(remote_center.x, 0.0, remote_center.z, 1.0))
            .to_vec3f();
        let remote_radius =
            (remote_extent_x * remote_extent_x + remote_extent_z * remote_extent_z).sqrt() * 0.5;
        let remote_bounds = PreviewBounds {
            min_x: transformed_center.x - remote_radius,
            max_x: transformed_center.x + remote_radius,
            min_z: transformed_center.z - remote_radius,
            max_z: transformed_center.z + remote_radius,
        };
        let mut bounds = local_bounds.unwrap_or(remote_bounds);
        bounds.min_x = bounds.min_x.min(remote_bounds.min_x);
        bounds.max_x = bounds.max_x.max(remote_bounds.max_x);
        bounds.min_z = bounds.min_z.min(remote_bounds.min_z);
        bounds.max_z = bounds.max_z.max(remote_bounds.max_z);
        Some(bounds.expand(0.12))
    }

    fn update_dump_label(&self, cx: &mut Cx, text: String) {
        self.ui.label(cx, ids!(dump_path_label)).set_text(cx, &text);
    }

    fn update_save_label(&self, cx: &mut Cx, text: String) {
        self.ui
            .label(cx, ids!(save_status_label))
            .set_text(cx, &text);
    }

    fn update_pose_label(&self, cx: &mut Cx) {
        self.ui.label(cx, ids!(pose_label)).set_text(
            cx,
            &format!(
                "Pose: x {:.3} | y {:.3} | rot {:.1} deg | band {:.2}..{:.2} m",
                self.pose.shift_x_meters,
                self.pose.shift_y_meters,
                self.pose.rotation_degrees(),
                self.band_pass.min_meters,
                self.band_pass.max_meters,
            ),
        );
    }

    fn set_slider_value(&self, cx: &mut Cx, id: LiveId, value: f64) {
        if let Some(mut slider) = self.ui.widget(cx, &[id]).borrow_mut::<Slider>() {
            slider.set_value(cx, value);
        }
    }

    fn slider_action_value(&self, cx: &mut Cx, actions: &Actions, id: LiveId) -> Option<f64> {
        let widget = self.ui.widget(cx, &[id]);
        let action = actions.find_widget_action(widget.widget_uid())?;
        match action.cast() {
            SliderAction::TextSlide(v) | SliderAction::Slide(v) | SliderAction::EndSlide(v) => {
                Some(v)
            }
            _ => None,
        }
    }

    fn sync_pose_to_ui(&self, cx: &mut Cx) {
        self.set_slider_value(cx, id!(shift_x_slider), self.pose.shift_x_meters as f64);
        self.set_slider_value(cx, id!(shift_y_slider), self.pose.shift_y_meters as f64);
        self.set_slider_value(cx, id!(rotate_slider), self.pose.rotation_degrees() as f64);
        self.set_slider_value(cx, id!(band_min_slider), self.band_pass.min_meters as f64);
        self.set_slider_value(cx, id!(band_max_slider), self.band_pass.max_meters as f64);
        self.update_pose_label(cx);
    }

    fn current_transformed_remote_height_map(&self) -> Option<XrDepthAlignHeightMap> {
        let loaded_dump = self.loaded_dump.as_ref()?;
        let transformed = xr_depth_align_transform_descriptor(
            &loaded_dump.pair.remote_descriptor.descriptor,
            &self.pose.to_mat4(),
        );
        transformed.height_map
    }

    fn refresh_preview(&mut self, cx: &mut Cx) {
        let local_height_map = self
            .loaded_dump
            .as_ref()
            .and_then(|loaded| loaded.pair.local_descriptor.descriptor.height_map.clone());
        let remote_height_map = self.current_transformed_remote_height_map();
        let preview_bounds = self.current_preview_bounds();
        if let Some(mut preview) = self
            .ui
            .widget(cx, ids!(preview))
            .borrow_mut::<ManualAlignPreview>()
        {
            preview.set_height_maps(
                cx,
                local_height_map,
                remote_height_map,
                preview_bounds,
                self.band_pass,
            );
        }
        self.update_pose_label(cx);
    }

    fn load_initial_dump(&mut self, cx: &mut Cx) {
        let Some(path) = Self::initial_dump_path() else {
            self.update_dump_label(cx, "Dump: no dump files found".to_string());
            self.update_save_label(cx, "Save: waiting for dump".to_string());
            return;
        };
        match Self::load_dump_pair(&path) {
            Ok(loaded_dump) => {
                self.pose = Self::load_sidecar_pose(&loaded_dump.sidecar_path).unwrap_or_default();
                self.loaded_dump = Some(loaded_dump.clone());
                self.update_dump_label(cx, format!("Dump: {}", loaded_dump.dump_path.display()));
                self.update_save_label(cx, format!("Save: {}", loaded_dump.sidecar_path.display()));
                self.sync_pose_to_ui(cx);
                self.refresh_preview(cx);
            }
            Err(err) => {
                self.update_dump_label(cx, format!("Dump: {err}"));
                self.update_save_label(cx, "Save: waiting for dump".to_string());
            }
        }
    }

    fn save_pose(&mut self, cx: &mut Cx) {
        let Some(loaded_dump) = self.loaded_dump.as_ref() else {
            self.update_save_label(cx, "Save: no dump loaded".to_string());
            return;
        };
        let sidecar = ManualAlignSidecar::from_pose(&loaded_dump.dump_path, self.pose);
        match fs::write(&loaded_dump.sidecar_path, sidecar.to_text().as_bytes()) {
            Ok(()) => {
                self.update_save_label(
                    cx,
                    format!("Save: wrote {}", loaded_dump.sidecar_path.display()),
                );
            }
            Err(err) => {
                self.update_save_label(
                    cx,
                    format!(
                        "Save: failed to write {}: {err}",
                        loaded_dump.sidecar_path.display()
                    ),
                );
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.load_initial_dump(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let mut changed = false;
        if let Some(value) = self.slider_action_value(cx, actions, id!(shift_x_slider)) {
            self.pose.shift_x_meters = value as f32;
            changed = true;
        }
        if let Some(value) = self.slider_action_value(cx, actions, id!(shift_y_slider)) {
            self.pose.shift_y_meters = value as f32;
            changed = true;
        }
        if let Some(value) = self.slider_action_value(cx, actions, id!(rotate_slider)) {
            self.pose.rotation_radians = (value as f32).to_radians();
            changed = true;
        }
        if let Some(value) = self.slider_action_value(cx, actions, id!(band_min_slider)) {
            self.band_pass.min_meters = value as f32;
            if self.band_pass.max_meters < self.band_pass.min_meters + 0.05 {
                self.band_pass.max_meters = self.band_pass.min_meters + 0.05;
            }
            changed = true;
        }
        if let Some(value) = self.slider_action_value(cx, actions, id!(band_max_slider)) {
            self.band_pass.max_meters = value as f32;
            if self.band_pass.min_meters > self.band_pass.max_meters - 0.05 {
                self.band_pass.min_meters = self.band_pass.max_meters - 0.05;
            }
            changed = true;
        }
        if changed {
            self.sync_pose_to_ui(cx);
            self.refresh_preview(cx);
        }
        if self.ui.button(cx, ids!(save_button)).clicked(actions) {
            self.save_pose(cx);
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
    }
}
