use crate::{
    cx_2d::Cx2d, draw_list_2d::ManyInstances, font_atlas::{CxFontAtlas, TextShaper, Font}, geometry::GeometryQuad2D, makepad_platform::*,
    font_loader::FontLoader, text_shaper::GlyphInfo,
    glyph_rasterizer::{Command, Mode, Params},
};

const ZBIAS_STEP: f32 = 0.00001;

live_design!{
    use link::shaders::*;

    pub DrawText2 = {{DrawText2}} {
        color: #fff

        texture grayscale_texture: texture2d

        varying tex_coord1: vec2
        varying clipped: vec2
        varying pos: vec2

        fn vertex(self) -> vec4 {
            let min_pos = self.rect_pos;
            let max_pos = vec2(self.rect_pos.x + self.rect_size.x, self.rect_pos.y - self.rect_size.y)

            self.clipped = clamp(
                mix(min_pos, max_pos, self.geom_pos),
                self.draw_clip.xy,
                self.draw_clip.zw
            )

            let normalized: vec2 = (self.clipped - min_pos) / vec2(self.rect_size.x, -self.rect_size.y)

            self.tex_coord1 = mix(
                vec2(self.font_t1.x, 1.0-self.font_t1.y),
                vec2(self.font_t2.x, 1.0-self.font_t2.y),
                normalized.xy
            )
            self.pos = normalized;
            return self.camera_projection * (self.camera_view * (self.view_transform * vec4(
                self.clipped.x,
                self.clipped.y,
                self.char_depth + self.draw_zbias,
                1.
            )))
        }

        fn get_color(self) -> vec4 {
            return self.color;
        }
        fn blend_color(self, incol:vec4)->vec4{
            return incol
        }

        fn get_brightness(self)->float{
            return 1.0;
        }

        fn sample_color(self, scale:float, pos:vec2)->vec4{
            let brightness = self.get_brightness();
            let sdf_radius = 8.0;
            let sdf_cutoff = 0.25;
            let s = sample2d(self.grayscale_texture, pos).x;
            let curve = 0.5;
            //if (self.sdf_radius != 0.0) {
            // HACK(eddyb) harcoded atlas size (see asserts below).
            let texel_coords = pos.xy * 4096.0;
            s = clamp((s - (1.0 - sdf_cutoff)) * sdf_radius / scale + 0.5, 0.0, 1.0);
            //} else {
            //    s = pow(s, curve);
            //}
            let col = self.get_color();
            return self.blend_color(vec4(s * col.rgb * brightness * col.a, s * col.a));
        }

        fn pixel(self) -> vec4 {
            let texel_coords = self.tex_coord1.xy;
            let dxt = length(dFdx(texel_coords));
            let dyt = length(dFdy(texel_coords));
            let scale = (dxt + dyt) * 4096.0 *0.5;
            return self.sample_color(scale, self.tex_coord1.xy);
        }
    }
}

#[derive(Debug, Clone, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct TextStyle {
    #[live()] pub font: Font,
    #[live()] pub font2: Font,
    #[live(9.0)] pub font_size: f64,
    //#[live(1.0)] pub brightness: f32,
    //#[live(0.5)] pub curve: f32,
    #[live(0.88)] pub line_scale: f64,
    #[live(1.4)] pub line_spacing: f64,
    //#[live(1.1)] pub top_drop: f64,
    #[live(1.3)] pub height_factor: f64,
    #[live] pub is_secret: bool
}

#[derive(Live, LiveRegister)]
#[repr(C)]
pub struct DrawText2 {
    #[rust] pub many_instances: Option<ManyInstances>,


    #[live] pub geometry: GeometryQuad2D,
    #[live] pub text_style: TextStyle,

    #[live] pub ignore_newlines: bool,
    #[live] pub combine_spaces: bool,

    #[live(1.0)] pub font_scale: f64,
    #[live(1.0)] pub draw_depth: f32,

    #[deref] pub draw_vars: DrawVars,
    // these values are all generated
    #[live] pub color: Vec4,
    #[calc] pub font_t1: Vec2,
    #[calc] pub font_t2: Vec2,
    #[calc] pub rect_pos: Vec2,
    #[calc] pub rect_size: Vec2,
    #[calc] pub draw_clip: Vec4,
    #[calc] pub char_depth: f32,
}

impl LiveHook for DrawText2 {
    fn before_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars.before_apply_init_shader(cx, apply, index, nodes, &self.geometry);
    }
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars.after_apply_update_self(cx, apply, index, nodes, &self.geometry);
    }
}

impl DrawText2 {
    pub fn draw(&mut self, cx: &mut Cx2d, pos: DVec2, val: &str) {
        self.draw_inner(cx, pos, val, &mut *cx.fonts_atlas_rc.clone().0.borrow_mut());
        if self.many_instances.is_some() {
            self.end_many_instances(cx)
        }
    }

    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        let fonts_atlas_rc = cx.fonts_atlas_rc.clone();
        let fonts_atlas = fonts_atlas_rc.0.borrow();
        self.begin_many_instances_internal(cx, &*fonts_atlas);
    }

    fn begin_many_instances_internal(&mut self, cx: &mut Cx2d, fonts_atlas: &CxFontAtlas) {
        self.update_draw_call_vars(fonts_atlas);
        let mi = cx.begin_many_aligned_instances(&self.draw_vars);
        self.many_instances = mi;
    }

    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        if let Some(mi) = self.many_instances.take() {
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }

    pub fn update_draw_call_vars(&mut self, font_atlas: &CxFontAtlas) {
        self.draw_vars.texture_slots[0] = Some(font_atlas.texture_sdf.clone());
    }

    fn draw_inner(&mut self, cx: &mut Cx2d, position: DVec2, line: &str, font_atlas: &mut CxFontAtlas) {
        self.char_depth = self.draw_depth;

        // If the line is empty, there is nothing to draw.
        if line.is_empty() {
            return;
        }

        // If the font did not load, there is nothing to draw.
        let Some(font_id) = self.text_style.font.font_id else {
            return;
        };
        let mut font_ids = [0, 0];
        let font_ids = if let Some(font2_id) = self.text_style.font2.font_id {
            font_ids[0] = font_id;
            font_ids[1] = font2_id;
            &font_ids[..2]
        } else {
            font_ids[0] = font_id;
            &font_ids[..1]
        };

        // Borrow the font loader from the context.
        let font_loader_rc = cx.font_loader.clone();
        let mut font_loader = font_loader_rc.borrow_mut();
        let font_loader = &mut *font_loader;

        // Borrow the shape cache from the context.
        let text_shaper_rc = cx.text_shaper.clone();
        let mut text_shaper_ref = text_shaper_rc.borrow_mut();
        let text_shaper = &mut *text_shaper_ref;

        let font_size = self.text_style.font_size * self.font_scale;

        let origin = position;

        let glyph_infos = shape(false, line, font_ids, font_loader, text_shaper);

        self.draw_glyphs(
            cx,
            origin,
            font_size,
            &glyph_infos,
            font_loader,
            font_atlas,
        );
    }

    /// Draws a sequence of glyphs, defined by the given list of glyph infos, at the given position.
    fn draw_glyphs(
        &mut self,
        cx: &mut Cx2d,
        position: DVec2,
        font_size: f64,
        glyph_infos: &[GlyphInfo],
        font_loader: &mut FontLoader,
        font_atlas: &mut CxFontAtlas,
    ) {
        // If the position is invalid, there is nothing to draw.
        if position.x.is_infinite() || position.x.is_nan() {
            return;
        }

        // If the list of glyph infos is empty, there is nothing to draw.
        if glyph_infos.is_empty() {
            return;
        }

        // If the shader failed to compile, there is nothing to draw.
        if !self.draw_vars.can_instance() {
            return;
        }

        // Lock the instance buffer.
        if !self.many_instances.is_some() {
            self.begin_many_instances_internal(cx, font_atlas);
        }
        let Some(mi) = &mut self.many_instances else {
            return;
        };

        // Get the device pixel ratio.
        let device_pixel_ratio = cx.current_dpi_factor();

        // Compute the glyph padding.
        let glyph_padding_dpx = 2.0;
        let glyph_padding_lpx = glyph_padding_dpx / device_pixel_ratio;


        let mut position = position;
        for glyph_info in glyph_infos {
            let font = font_loader[glyph_info.font_id].as_mut().unwrap();
            let units_per_em = font.ttf_font.units_per_em;
            let ascender = units_to_lpxs(font.ttf_font.ascender, units_per_em, font_size) * self.text_style.line_scale;

            // Use the glyph id to get the glyph from the font.
            let glyph = font.owned_font_face.with_ref(|face| {
                font.ttf_font.get_glyph_by_id(face, glyph_info.glyph_id as usize).unwrap()
            });

            // Compute the position of the glyph.
            let glyph_position = dvec2(
                units_to_lpxs(glyph.bounds.p_min.x, units_per_em, font_size),
                units_to_lpxs(glyph.bounds.p_min.y, units_per_em, font_size),
            );

            // Compute the size of the bounding box of the glyph in logical pixels.
            let glyph_size_lpx = dvec2(
                units_to_lpxs(glyph.bounds.p_max.x - glyph.bounds.p_min.x, units_per_em, font_size),
                units_to_lpxs(glyph.bounds.p_max.y - glyph.bounds.p_min.y, units_per_em, font_size),
            );

            // Compute the size of the bounding box of the glyph in device pixels.
            let glyph_size_dpx = glyph_size_lpx * device_pixel_ratio;

            // Compute the padded size of the bounding box of the glyph in device pixels.
            let mut padded_glyph_size_dpx = glyph_size_dpx;
            if padded_glyph_size_dpx.x != 0.0 {
                padded_glyph_size_dpx.x = padded_glyph_size_dpx.x.ceil() + glyph_padding_dpx * 2.0;
            }
            if padded_glyph_size_dpx.y != 0.0 {
                padded_glyph_size_dpx.y = padded_glyph_size_dpx.y.ceil() + glyph_padding_dpx * 2.0;
            }

            // Compute the padded size of the bounding box of the glyph in logical pixels.
            let padded_glyph_size_lpx = padded_glyph_size_dpx / device_pixel_ratio;

            // Compute the left side bearing.
            let left_side_bearing = units_to_lpxs(glyph.horizontal_metrics.left_side_bearing, units_per_em, font_size);

            // Use the font size in device pixels to get the atlas page id from the font.
            let atlas_page_id = font.get_atlas_page_id(units_to_lpxs(1.0, units_per_em, font_size / self.font_scale) * device_pixel_ratio);

            // Use the atlas page id to get the atlas page from the font.
            let atlas_page = &mut font.atlas_pages[atlas_page_id];

            // Use the padded glyph size in device pixels to get the atlas glyph from the atlas page.
            let atlas_glyph = *atlas_page.atlas_glyphs.entry(glyph_info.glyph_id as usize).or_insert_with(|| {
                font_atlas
                    .alloc_atlas_glyph(
                        padded_glyph_size_dpx.x / self.font_scale,
                        padded_glyph_size_dpx.y / self.font_scale,
                        Command {
                            mode: Mode::Sdf,
                            params: Params {
                                font_id: glyph_info.font_id,
                                atlas_page_id,
                                glyph_id: glyph_info.glyph_id as usize,
                            },
                        }
                    )
            });

            // Compute the distance from the current position to the draw rectangle.
            let delta = dvec2(
                left_side_bearing - glyph_padding_lpx,
                ascender - glyph_position.y + glyph_padding_lpx
            );

            // Compute the advance width.
            let advance_width = compute_glyph_width(glyph_info.font_id, glyph_info.glyph_id, self.text_style.font_size, font_loader);

            // Emit the instance data.
            self.font_t1 = atlas_glyph.t1;
            self.font_t2 = atlas_glyph.t2;
            self.rect_pos = (position + delta).into();
            self.rect_size = padded_glyph_size_lpx.into();
            mi.instances.extend_from_slice(self.draw_vars.as_slice());

            self.char_depth += ZBIAS_STEP;

            // Advance to the next position.
            position.x += advance_width;
        }
    }
}

fn compute_glyph_width(
    font_id: usize,
    glyph_id: usize,
    font_size: f64,
    font_loader: &mut FontLoader,
) -> f64 {
    let font = font_loader[font_id].as_mut().unwrap();
    let units_per_em = font.ttf_font.units_per_em;
    let glyph_width = font.owned_font_face.with_ref(|face| {
        let glyph = font.ttf_font.get_glyph_by_id(face, glyph_id as usize).unwrap();
        glyph.horizontal_metrics.advance_width
    });
    units_to_lpxs(glyph_width, units_per_em, font_size)
}

fn units_to_lpxs(units: f64, units_per_em: f64, font_size: f64) -> f64 {
    const LPXS_PER_IN: f64 = 96.0;
    const PTS_PER_IN: f64 = 72.0;

    let ems = units / units_per_em;
    let pts = ems * font_size;
    let ins = pts / PTS_PER_IN;
    let lpxs = ins * LPXS_PER_IN;
    lpxs
}

fn shape<'a>(
    is_secret: bool,
    string: &str,
    font_ids: &[usize],
    font_loader: &mut FontLoader,
    text_shaper: &'a mut TextShaper,
) -> &'a [GlyphInfo] {
    text_shaper.get_or_shape_text(
        font_loader,
        is_secret,
        string,
        font_ids,
    )
}
