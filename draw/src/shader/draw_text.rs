use {
    crate::{
        cx_2d::Cx2d,
        cx_draw::CxDraw,
        draw_list_2d::ManyInstances,
        makepad_platform::*,
        text::{
            builtins,
            color::Color,
            font::FontId,
            font_family::FontFamilyId,
            fonts::Fonts,
            geom::{Point, Rect as TextRect, Size, Transform},
            layouter::{
                BorrowedLayoutParams, LaidoutGlyph, LaidoutRow, LaidoutText, LayoutOptions,
                SelectionRect, Style,
            },
            loader::{FontDefinition, FontFamilyDefinition},
            rasterizer::{AtlasKind, RasterizedGlyph},
            selection::{Cursor, Selection},
        },
        turtle::*,
        turtle::{Align, Walk},
    },
    std::{borrow::Cow, cell::RefCell, rc::Rc},
};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.res.*

    mod.text = {
        let text = me
        FontFamily: mod.std.set_type_default() do #(FontFamily::script_component(vm))
        FontMember: mod.std.set_type_default() do #(FontMember::script_api(vm))
        TextStyle: mod.std.set_type_default() do #(TextStyle::script_api(vm)){
            font_size: 10
            font_family: text.FontFamily{
                latin := text.FontMember{res: crate_resource("self:../../widgets/resources/IBMPlexSans-Text.ttf") asc:-0.1 desc:0.0}
            }
            line_spacing: 1.2
        }
    }

    use mod.text.*

    mod.draw.DrawText = mod.std.set_type_default() do #(DrawText::script_shader(vm)){

        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)

        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)

        geom: vertex_buffer(geom.QuadVertex, geom.QuadGeom)

        color: #fff
        sdf_sharpness: 1.0
        sdf_luma_bias: 0.03

        pos: varying(vec2f)
        t: varying(vec2f)
        world: varying(vec4f)

        radius: uniform(float)
        cutoff: uniform(float)
        total_chars: instance(1000000.0)

        grayscale_texture: texture_2d(float)
        color_texture: texture_2d(float)
        msdf_texture: texture_2d(float)

        vertex: fn() {
            let p = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom.pos)
            let p_clipped = clamp(p, self.draw_clip.xy, self.draw_clip.zw)
            let p_normalized = (p_clipped - self.rect_pos) / self.rect_size

            self.pos = p_normalized;
            self.t = mix(self.t_min, self.t_max, p_normalized.xy)
            self.world = self.draw_list.view_transform * vec4(
                p_clipped.x,
                p_clipped.y,
                self.glyph_depth + self.draw_call.zbias,
                1.
            )
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * (self.world))
        }

        sdf: fn(scale, p, color) {
            let sampled = self.grayscale_texture.sample_as_bgra(p);
            let s = if self.atlas_plane < 0.5 {
                sampled.r
            } else if self.atlas_plane < 1.5 {
                sampled.g
            } else if self.atlas_plane < 2.5 {
                sampled.b
            } else {
                sampled.a
            };
            // Convert sampled SDF to coverage (0..1). scale is source texels per screen pixel.
            let safe_scale = max(scale, 0.0001);
            let luma = dot(color.rgb, vec3(0.299, 0.587, 0.114));
            var a = clamp(
                (s - (1.0 - self.cutoff)) * self.radius / safe_scale * self.sdf_sharpness + 0.5,
                0.0,
                1.0,
            );
            // Polarity compensation:
            // dark text on light backgrounds usually appears softer than the inverse,
            // so we bias coverage slightly by text luminance.
            let bias = (0.5 - luma) * self.sdf_luma_bias;
            a = clamp(a - bias, 0.0, 1.0);
            return a
        }

        msdf: fn(scale, p, color) {
            let s = self.msdf_texture.sample_as_bgra(p);
            // Use alpha as the coverage source to keep parity with SDF while RGB stores MSDF.
            let dist = s.a;
            let safe_scale = max(scale, 0.0001);
            let luma = dot(color.rgb, vec3(0.299, 0.587, 0.114));
            var a = clamp(
                (dist - (1.0 - self.cutoff)) * self.radius / safe_scale * self.sdf_sharpness + 0.5,
                0.0,
                1.0,
            );
            let bias = (0.5 - luma) * self.sdf_luma_bias;
            // Avoid lifting near-zero background alpha into visible gray quads on light text.
            if a > self.sdf_luma_bias * 0.5 {
                a = clamp(a - bias, 0.0, 1.0);
            }
            return a
        }

        get_color: fn() {
            return self.color
        }

        fragment: fn() {
            self.fb0 = self.pixel();
        }

        sample_text_pixel: fn() {
            let dxt = length(dFdx(self.t))
            let dyt = length(dFdy(self.t))
            if self.texture_index == 0 {
                let c = self.get_color()
                let scale = (dxt + dyt) * self.grayscale_texture.size().x * 0.5
                let tex_size = self.grayscale_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let s = self.sdf(scale, p, c)
                return s * vec4(c.rgb * c.a, c.a)
            } else if self.texture_index == 1 {
                let tex_size = self.color_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let c = self.color_texture.sample_as_bgra(p)
                return vec4(c.rgb * c.a, c.a)
            } else {
                let c = self.get_color()
                let scale = (dxt + dyt) * self.msdf_texture.size().x * 0.5
                let tex_size = self.msdf_texture.size()
                let half_texel = vec2(0.5 / tex_size.x, 0.5 / tex_size.y)
                let p = clamp(self.t.xy, self.t_min + half_texel, self.t_max - half_texel)
                let s = self.msdf(scale, p, c)
                return s * vec4(c.rgb * c.a, c.a)
            }
        }

        pixel: fn() {
            return self.sample_text_pixel()
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawText {
    #[rust]
    pub many_instances: Option<ManyInstances>,
    #[live]
    pub text_style: TextStyle,
    #[live(1.0)]
    pub font_scale: f32,
    #[live(1.0)]
    pub draw_depth: f32,
    #[live]
    pub debug: bool,

    #[live]
    pub temp_y_shift: f32,

    /// When true, successive draws extend the area instead of replacing it.
    /// Useful when drawing multiple text chunks that should be treated as one area.
    #[live]
    pub extend_area: bool,

    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub rect_pos: Vec2f,
    #[live]
    pub rect_size: Vec2f,
    #[live]
    pub draw_clip: Vec4f,
    #[live(1.0)]
    pub depth_clip: f32,
    #[live]
    pub glyph_depth: f32,
    #[live]
    pub texture_index: f32,
    #[live]
    pub char_index: f32,
    #[live(vec4(1., 1., 1., 1.))]
    pub color: Vec4f,
    #[live(1.0)]
    pub sdf_sharpness: f32,
    #[live(0.03)]
    pub sdf_luma_bias: f32,
    #[live]
    pub t_min: Vec2f,
    #[live]
    pub t_max: Vec2f,
    #[live]
    pub atlas_plane: f32,
    #[live]
    pub pad1: f32,
}

#[derive(Clone, Debug)]
pub struct PreparedTextGlyph {
    pub pen_x_in_lpxs: f32,
    pub offset_x_in_lpxs: f32,
    pub advance_in_lpxs: f32,
    pub font_size_in_lpxs: f32,
    pub rasterized: RasterizedGlyph,
}

#[derive(Clone, Debug)]
pub struct PreparedTextRun {
    pub width_in_lpxs: f32,
    pub ascender_in_lpxs: f32,
    pub descender_in_lpxs: f32,
    pub glyphs: Vec<PreparedTextGlyph>,
}

impl DrawText {
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: Vec2d, text: &str) {
        let text = self.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        self.draw_text(cx, Point::new(pos.x as f32, pos.y as f32), &text);
    }

    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        if self.many_instances.is_some() {
            return;
        }
        self.update_draw_vars(cx);
        self.many_instances = cx.begin_many_aligned_instances(&self.draw_vars);
    }

    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        if let Some(instances) = self.many_instances.take() {
            self.finish_many_instances(cx, instances);
        }
    }

    pub fn draw_rasterized_glyphs_abs(
        &mut self,
        cx: &mut Cx2d,
        glyphs: &[(Point<f32>, f32, RasterizedGlyph)],
        color: Vec4f,
    ) {
        if glyphs.is_empty() {
            return;
        }
        self.update_draw_vars(cx);
        if let Some(mut instances) = self.many_instances.take() {
            self.glyph_depth = self.draw_depth;
            self.color = color;
            for (origin_in_lpxs, font_size_in_lpxs, rasterized_glyph) in glyphs {
                self.draw_rasterized_glyph(
                    *origin_in_lpxs,
                    *font_size_in_lpxs,
                    None,
                    *rasterized_glyph,
                    &mut instances.instances,
                );
            }
            self.many_instances = Some(instances);
            return;
        }

        let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
            return;
        };

        self.glyph_depth = self.draw_depth;
        self.color = color;
        for (origin_in_lpxs, font_size_in_lpxs, rasterized_glyph) in glyphs {
            self.draw_rasterized_glyph(
                *origin_in_lpxs,
                *font_size_in_lpxs,
                None,
                *rasterized_glyph,
                &mut instances.instances,
            );
        }

        self.finish_many_instances(cx, instances);
    }

    pub fn draw_rasterized_glyph_abs(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        rasterized_glyph: RasterizedGlyph,
        color: Vec4f,
    ) {
        self.draw_rasterized_glyphs_abs(
            cx,
            &[(origin_in_lpxs, font_size_in_lpxs, rasterized_glyph)],
            color,
        );
    }

    pub fn prepare_single_line_run(&self, cx: &mut Cx2d, text: &str) -> Option<PreparedTextRun> {
        let laidout = self.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        let row = laidout.rows.first()?;
        if row.glyphs.is_empty() {
            return None;
        }

        let dpx_factor = cx.current_dpi_factor() as f32;
        let mut glyphs = Vec::with_capacity(row.glyphs.len());
        for glyph in &row.glyphs {
            let dpx_per_em = glyph.font_size_in_lpxs * dpx_factor;
            let Some(rasterized) = glyph.rasterize(dpx_per_em) else {
                continue;
            };

            glyphs.push(PreparedTextGlyph {
                pen_x_in_lpxs: glyph.origin_in_lpxs.x * self.font_scale,
                offset_x_in_lpxs: glyph.offset_in_lpxs() * self.font_scale,
                advance_in_lpxs: glyph.advance_in_lpxs() * self.font_scale,
                font_size_in_lpxs: glyph.font_size_in_lpxs,
                rasterized,
            });
        }
        if glyphs.is_empty() {
            return None;
        }

        Some(PreparedTextRun {
            width_in_lpxs: row.width_in_lpxs * self.font_scale,
            ascender_in_lpxs: row.ascender_in_lpxs * self.font_scale,
            descender_in_lpxs: row.descender_in_lpxs * self.font_scale,
            glyphs,
        })
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk, align: Align, text: &str) -> Rect {
        let turtle_rect = cx.turtle().inner_rect();
        let max_width_in_lpxs = if !turtle_rect.size.x.is_nan() {
            Some(turtle_rect.size.x as f32)
        } else {
            None
        };
        let wrap = cx.turtle().layout().flow
            == Flow::Right {
                row_align: RowAlign::Top,
                wrap: true,
            };

        let text = self.layout(cx, 0.0, 0.0, max_width_in_lpxs, wrap, align, text);
        self.draw_walk_laidout(cx, walk, &text)
    }

    pub fn draw_walk_laidout(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        laidout_text: &LaidoutText,
    ) -> Rect {
        use crate::text::geom::{Point, Size};
        use crate::turtle;

        let size_in_lpxs = laidout_text.size_in_lpxs * self.font_scale;
        let max_size_in_lpxs = Size::new(
            cx.turtle()
                .max_width(walk)
                .map_or(size_in_lpxs.width, |max_width| max_width as f32),
            cx.turtle()
                .max_height(walk)
                .map_or(size_in_lpxs.height, |max_height| max_height as f32),
        );
        let turtle_rect = cx.walk_turtle(Walk {
            abs_pos: walk.abs_pos,
            margin: walk.margin,
            width: turtle::Size::Fixed(max_size_in_lpxs.width as f64),
            height: turtle::Size::Fixed(max_size_in_lpxs.height as f64),
            metrics: Metrics {
                descender: -laidout_text.rows.last().unwrap().descender_in_lpxs as f64,
                line_gap: 0.0,
                line_scale: 1.0,
            },
        });

        if self.debug {
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(&mut area, turtle_rect);
            cx.cx.debug.area(area, vec4(1.0, 1.0, 1.0, 1.0));
        }

        let origin_in_lpxs = Point::new(turtle_rect.pos.x as f32, turtle_rect.pos.y as f32);
        self.draw_text(cx, origin_in_lpxs, &laidout_text);

        rect(
            origin_in_lpxs.x as f64,
            origin_in_lpxs.y as f64,
            size_in_lpxs.width as f64,
            size_in_lpxs.height as f64,
        )
    }

    pub fn draw_walk_resumable_with(
        &mut self,
        cx: &mut Cx2d,
        text_str: &str,
        mut f: impl FnMut(&mut Cx2d, Rect, f32),
    ) {
        let turtle_pos = cx.turtle().pos();
        let turtle_rect = cx.turtle().inner_rect();
        let origin_in_lpxs = Point::new(turtle_rect.pos.x as f32, turtle_pos.y as f32);
        let first_row_indent_in_lpxs = turtle_pos.x as f32 - origin_in_lpxs.x;
        let row_height = cx.turtle().next_row_offset();

        let max_width_in_lpxs = if !turtle_rect.size.x.is_nan() {
            Some(turtle_rect.size.x as f32)
        } else {
            None
        };
        let wrap = cx.turtle().layout().flow
            == Flow::Right {
                row_align: RowAlign::Top,
                wrap: true,
            };

        let text = self.layout(
            cx,
            first_row_indent_in_lpxs,
            row_height as f32,
            max_width_in_lpxs,
            wrap,
            Align::default(),
            text_str,
        );
        self.draw_text(cx, origin_in_lpxs, &text);

        let last_row = text.rows.last().unwrap();
        let new_turtle_pos = origin_in_lpxs
            + Size::new(
                last_row.width_in_lpxs,
                last_row.origin_in_lpxs.y - last_row.ascender_in_lpxs,
            ) * self.font_scale;
        let used_size_in_lpxs = text.size_in_lpxs * self.font_scale;
        let new_turtle_pos = dvec2(new_turtle_pos.x as f64, new_turtle_pos.y as f64);
        let turtle = cx.turtle_mut();

        turtle.move_to(dvec2(origin_in_lpxs.x as f64, origin_in_lpxs.y as f64));
        turtle.allocate_width(used_size_in_lpxs.width as f64);
        turtle.allocate_height(used_size_in_lpxs.height as f64);
        turtle.move_to(new_turtle_pos);

        turtle.set_wrap_spacing(
            (last_row.ascender_in_lpxs * last_row.line_spacing_scale - last_row.ascender_in_lpxs)
                as f64,
        );

        cx.emit_turtle_walk(Rect {
            pos: new_turtle_pos,
            size: dvec2(
                used_size_in_lpxs.width as f64,
                used_size_in_lpxs.height as f64,
            ),
        });

        let shift = if let Some(row) = text.rows.get(0) {
            if let Some(glyph) = row.glyphs.get(0) {
                glyph.font_size_in_lpxs * self.temp_y_shift
            } else {
                0.0
            }
        } else {
            0.0
        };

        for SelectionRect {
            rect_in_lpxs,
            ascender_in_lpxs,
        } in text.selection_rects(Selection {
            anchor: Cursor {
                index: 0,
                prefer_next_row: false,
            },
            cursor: Cursor {
                index: text.text.len(),
                prefer_next_row: false,
            },
        }) {
            let rect_in_lpxs = TextRect::new(
                origin_in_lpxs + Size::from(rect_in_lpxs.origin) * self.font_scale,
                rect_in_lpxs.size * self.font_scale,
            );
            f(
                cx,
                rect(
                    rect_in_lpxs.origin.x as f64,
                    rect_in_lpxs.origin.y as f64 + shift as f64,
                    rect_in_lpxs.size.width as f64,
                    rect_in_lpxs.size.height as f64,
                ),
                ascender_in_lpxs,
            )
        }
    }

    pub fn layout(
        &self,
        cx: &mut Cx,
        first_row_indent_in_lpxs: f32,
        first_row_min_line_spacing_below_in_lpxs: f32,
        max_width_in_lpxs: Option<f32>,
        wrap: bool,
        align: Align,
        text: &str,
    ) -> Rc<LaidoutText> {
        cx.load_all_script_resources();
        self.text_style.font_family.ensure_fonts_loaded(cx);
        CxDraw::lazy_construct_fonts(cx);
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let mut fonts = fonts.borrow_mut();

        fonts.get_or_layout(BorrowedLayoutParams {
            text,
            style: Style {
                font_family_id: self.text_style.font_family.to_font_family_id(),
                font_size_in_pts: self.text_style.font_size,
                color: None,
            },
            options: LayoutOptions {
                first_row_indent_in_lpxs,
                first_row_min_line_spacing_below_in_lpxs,
                max_width_in_lpxs,
                wrap,
                align: align.x as f32,
                line_spacing_scale: self.text_style.line_spacing as f32,
            },
        })
    }

    fn draw_text(&mut self, cx: &mut Cx2d, origin_in_lpxs: Point<f32>, text: &LaidoutText) {
        self.update_draw_vars(cx);
        if let Some(mut instances) = self.many_instances.take() {
            self.glyph_depth = self.draw_depth;
            for row in &text.rows {
                self.draw_row(
                    cx,
                    origin_in_lpxs + Size::from(row.origin_in_lpxs) * self.font_scale,
                    row,
                    &mut instances.instances,
                );
            }
            self.many_instances = Some(instances);
            return;
        }
        let Some(mut instances) = cx.begin_many_aligned_instances(&self.draw_vars) else {
            return;
        };
        self.glyph_depth = self.draw_depth;
        for row in &text.rows {
            self.draw_row(
                cx,
                origin_in_lpxs + Size::from(row.origin_in_lpxs) * self.font_scale,
                row,
                &mut instances.instances,
            );
        }
        self.finish_many_instances(cx, instances);
    }

    fn finish_many_instances(&mut self, cx: &mut Cx2d, instances: ManyInstances) {
        let new_area = cx.end_many_instances(instances);
        let old_area = self.draw_vars.area;
        if self.extend_area {
            let extended = old_area.extend_with(cx, new_area);
            self.draw_vars.area = cx.update_area_refs(old_area, extended);
        } else {
            self.draw_vars.area = cx.update_area_refs(old_area, new_area);
        }
    }

    fn update_draw_vars(&mut self, cx: &mut Cx2d) {
        self.draw_vars.append_group_id = cx.draw_call_group_content().0;
        let fonts = cx.fonts.borrow();
        let rasterizer = fonts.rasterizer().borrow();
        let sdfer_settings = rasterizer.sdfer().settings();
        self.draw_vars.dyn_uniforms[0] = sdfer_settings.radius;
        self.draw_vars.dyn_uniforms[1] = sdfer_settings.cutoff;
        self.draw_vars.texture_slots[0] = Some(fonts.grayscale_texture().clone());
        self.draw_vars.texture_slots[1] = Some(fonts.color_texture().clone());
        self.draw_vars.texture_slots[2] = Some(fonts.msdf_texture().clone());
    }

    fn draw_row(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        row: &LaidoutRow,
        out_instances: &mut Vec<f32>,
    ) {
        for glyph in &row.glyphs {
            self.draw_glyph(
                cx,
                origin_in_lpxs + Size::from(glyph.origin_in_lpxs) * self.font_scale,
                glyph,
                out_instances,
            );
        }

        let width_in_lpxs = row.width_in_lpxs * self.font_scale;
        if self.debug {
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                rect(
                    origin_in_lpxs.x as f64,
                    (origin_in_lpxs.y - row.ascender_in_lpxs * self.font_scale) as f64,
                    width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx.debug.area(area, vec4(1.0, 0.0, 0.0, 1.0));
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                rect(
                    origin_in_lpxs.x as f64,
                    origin_in_lpxs.y as f64,
                    width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx.debug.area(area, vec4(0.0, 1.0, 0.0, 1.0));
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                rect(
                    origin_in_lpxs.x as f64,
                    (origin_in_lpxs.y - row.descender_in_lpxs * self.font_scale) as f64,
                    width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx.debug.area(area, vec4(0.0, 0.0, 1.0, 1.0));
        }
    }

    fn draw_glyph(
        &mut self,
        cx: &mut Cx2d,
        origin_in_lpxs: Point<f32>,
        glyph: &LaidoutGlyph,
        output: &mut Vec<f32>,
    ) {
        use crate::text::geom::Point;
        let font_size_in_dpxs = glyph.font_size_in_lpxs * cx.current_dpi_factor() as f32;
        if let Some(rasterized_glyph) = glyph.rasterize(font_size_in_dpxs) {
            self.draw_rasterized_glyph(
                Point::new(
                    origin_in_lpxs.x + glyph.offset_in_lpxs() * self.font_scale,
                    origin_in_lpxs.y,
                ),
                glyph.font_size_in_lpxs,
                glyph.color,
                rasterized_glyph,
                output,
            );
        }
    }

    fn draw_rasterized_glyph(
        &mut self,
        origin_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        color: Option<Color>,
        glyph: RasterizedGlyph,
        output: &mut Vec<f32>,
    ) {
        fn tex_coord(point: Point<usize>, size: Size<usize>) -> Point<f32> {
            Point::new(
                point.x as f32 / size.width as f32,
                point.y as f32 / size.height as f32,
            )
        }

        let texture_index = match glyph.atlas_kind {
            AtlasKind::Grayscale => 0.0,
            AtlasKind::Color => 1.0,
            AtlasKind::Msdf => 2.0,
        };

        let atlas_image_bounds = glyph.atlas_image_bounds;
        let atlas_size = glyph.atlas_size;
        let t_min = tex_coord(glyph.atlas_image_bounds.min(), atlas_size);
        let t_max = tex_coord(glyph.atlas_image_bounds.max(), atlas_size);

        let atlas_image_padding = glyph.atlas_image_padding;
        let atlas_image_size = atlas_image_bounds.size;
        let origin_in_dpxs = glyph.origin_in_dpxs;
        let bounds_in_dpxs = TextRect::new(
            Point::new(
                origin_in_dpxs.x - atlas_image_padding as f32,
                -origin_in_dpxs.y - atlas_image_size.height as f32 + (atlas_image_padding as f32),
            ),
            Size::new(
                atlas_image_size.width as f32,
                atlas_image_size.height as f32,
            ),
        );
        let bounds_in_lpxs = bounds_in_dpxs.apply_transform(
            Transform::from_scale_uniform(font_size_in_lpxs / glyph.dpxs_per_em * self.font_scale)
                .translate(origin_in_lpxs.x, origin_in_lpxs.y),
        );

        self.rect_pos = vec2(bounds_in_lpxs.origin.x, bounds_in_lpxs.origin.y)
            + vec2(0.0, self.temp_y_shift * font_size_in_lpxs);
        self.rect_size = vec2(bounds_in_lpxs.size.width, bounds_in_lpxs.size.height);
        if let Some(color) = color {
            self.color = vec4(
                color.r as f32,
                color.g as f32,
                color.b as f32,
                color.a as f32,
            ) / 255.0;
        }
        self.texture_index = texture_index;
        self.atlas_plane = glyph.atlas_plane as f32;
        self.t_min = vec2(t_min.x, t_min.y);
        self.t_max = vec2(t_max.x, t_max.y);
        let slice = self.draw_vars.as_slice();

        output.extend_from_slice(slice);
        self.glyph_depth += 0.000001;
        self.char_index += 1.0;
    }

    /// Resets the character index counter to 0. Call this before drawing text
    /// when you want to track character positions for animation effects.
    pub fn reset_char_index(&mut self) {
        self.char_index = 0.0;
    }

    /// Sets the total_chars instance value on all instances in the area after drawing is complete.
    /// This allows the shader to know how many characters are in the buffer
    /// for fade-in animation effects.
    pub fn set_total_chars(&mut self, cx: &mut Cx, total: f32) {
        self.draw_vars
            .set_instance_on_area(cx, live_id!(total_chars), &[total]);
    }

    pub fn new_draw_call(&mut self, cx: &mut Cx2d) {
        self.update_draw_vars(cx);
        cx.new_draw_call(&self.draw_vars);
    }

    pub fn append_to_draw_call(&self, cx: &mut Cx2d) {
        cx.append_to_draw_call(&self.draw_vars);
    }
}

#[derive(Debug, Clone, Script, ScriptHook)]
pub struct TextStyle {
    #[live]
    pub font_family: FontFamily,
    #[live(10.0)]
    pub font_size: f32,
    #[live(1.0)]
    pub line_spacing: f32,
}

#[derive(Debug, Clone, Script, ScriptHook)]
pub struct FontMember {
    #[live]
    pub res: Option<ScriptHandleRef>,
    #[live]
    pub asc: f32,
    #[live]
    pub desc: f32,
}

#[derive(Debug, Clone, Script, PartialEq)]
pub struct FontFamily {
    #[rust]
    id: LiveId,
    #[rust]
    members: Vec<FontMemberDef>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FontMemberDef {
    handle: ScriptHandle,
    asc: f32,
    desc: f32,
}

impl FontFamily {
    fn to_font_family_id(&self) -> FontFamilyId {
        (self.id.0).into()
    }

    fn update_font_definitions(&self, cx: &mut Cx, fonts: &mut Fonts) {
        let mut font_ids = Vec::new();

        for member in &self.members {
            let font_id: FontId = (member.handle.index() as u64).into();

            if !fonts.is_font_known(font_id) {
                let font_data = cx
                    .get_resource_abs_path(member.handle)
                    .and_then(|path| builtins::get_builtin_font_data(&path))
                    .or_else(|| {
                        cx.get_resource(member.handle)
                            .map(|rc| Rc::new(Cow::Owned((*rc).clone())))
                    });

                if let Some(data) = font_data {
                    fonts.define_font(
                        font_id,
                        FontDefinition {
                            data,
                            index: 0,
                            ascender_fudge_in_ems: member.asc,
                            descender_fudge_in_ems: member.desc,
                        },
                    );
                }
            }

            if fonts.is_font_known(font_id) {
                font_ids.push(font_id);
            }
        }

        fonts.set_font_family_definition(
            self.to_font_family_id(),
            FontFamilyDefinition { font_ids },
        );
    }

    fn ensure_fonts_loaded(&self, cx: &mut Cx) {
        CxDraw::lazy_construct_fonts(cx);
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let mut fonts = fonts.borrow_mut();
        self.update_font_definitions(cx, &mut fonts);
    }
}

impl TextStyle {
    pub fn font_family_id(&self) -> FontFamilyId {
        self.font_family.to_font_family_id()
    }

    pub fn ensure_fonts_loaded(&self, cx: &mut Cx) {
        self.font_family.ensure_fonts_loaded(cx);
    }
}

impl ScriptHook for FontFamily {
    fn on_custom_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) -> bool {
        let Some(obj) = value.as_object() else {
            return false;
        };

        // Use the object index as the unique id
        self.id = LiveId(obj.index() as u64);
        self.members.clear();

        let len = vm.bx.heap.vec_len(obj);
        for i in 0..len {
            let kv = vm.bx.heap.vec_key_value(obj, i, NoTrap);
            let member = FontMember::script_from_value(vm, kv.value);
            if let Some(ref handle_ref) = member.res {
                self.members.push(FontMemberDef {
                    handle: handle_ref.as_handle(),
                    asc: member.asc,
                    desc: member.desc,
                });
            }
        }

        let cx = vm.host.cx_mut();
        cx.load_all_script_resources();
        CxDraw::lazy_construct_fonts(cx);
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let mut fonts = fonts.borrow_mut();
        self.update_font_definitions(cx, &mut fonts);

        true
    }
}
