use {
    crate::{
        cx_2d::Cx2d,
        draw_list_2d::ManyInstances,
        geometry::GeometryQuad2D,
        makepad_platform::*,
        text::{
            color::Color,
            font::{AtlasKind, RasterizedGlyph},
            geom::{Point, Rect, Size, Transform},
            layout::{Cursor, LaidoutGlyph, LaidoutRow, LaidoutText},
            substr::Substr,
        },
        turtle::{Align, Walk},
    },
    std::rc::Rc,
};

live_design! {
    use link::shaders::*;

    pub DrawText2 = {{DrawText2}} {
        color: #ffff,

        uniform radius: float;
        uniform cutoff: float;
        uniform grayscale_atlas_size: vec2;
        uniform color_atlas_size: vec2;

        texture grayscale_texture: texture2d
        texture color_texture: texture2d

        varying t: vec2

        fn vertex(self) -> vec4 {
            let p = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom_pos);
            let p_clipped = clamp(p, self.draw_clip.xy, self.draw_clip.zw);
            let p_normalized: vec2 = (p_clipped - self.rect_pos) / self.rect_size;

            self.t = mix(self.t_min, self.t_max, p_normalized.xy);
            return self.camera_projection * (self.camera_view * (self.view_transform * vec4(
                p_clipped.x,
                p_clipped.y,
                self.draw_depth + self.draw_zbias,
                1.
            )));
        }

        fn sdf(self, scale: float, p: vec2) -> float {
            let s = sample2d(self.grayscale_texture, p).x;
            s = clamp((s - (1.0 - self.cutoff)) * self.radius / scale + 0.5, 0.0, 1.0);
            return s;
        }

        fn pixel(self) -> vec4 {
            let dxt = length(dFdx(self.t));
            let dyt = length(dFdy(self.t));
            if self.texture_index == 0 {
                // TODO: Support non square atlases?
                let scale = (dxt + dyt) * self.grayscale_atlas_size.x * 0.5;
                let s = self.sdf(scale, self.t.xy);
                let c = self.draw_color;
                return s * c;
            } else {
                let c = sample2d(self.color_texture, self.t);
                return vec4(c.rgb * c.a, c.a);
            }
        }
    }
}

#[derive(Live, LiveRegister)]
#[repr(C)]
pub struct DrawText2 {
    #[live]
    pub geometry: GeometryQuad2D,
    #[live]
    pub text_style: TextStyle,
    #[live]
    pub color: Vec4,
    #[live]
    pub debug: bool,

    #[deref]
    pub draw_vars: DrawVars,
    #[calc]
    pub rect_pos: Vec2,
    #[calc]
    pub rect_size: Vec2,
    #[calc]
    pub draw_clip: Vec4,
    #[calc]
    pub draw_depth: f32,
    #[calc]
    pub draw_color: Vec4,
    #[calc]
    pub texture_index: f32,
    #[calc]
    pub t_min: Vec2,
    #[calc]
    pub t_max: Vec2,
}

impl LiveHook for DrawText2 {
    fn before_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars
            .before_apply_init_shader(cx, apply, index, nodes, &self.geometry);
    }

    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars
            .after_apply_update_self(cx, apply, index, nodes, &self.geometry);
    }
}

impl DrawText2 {
    pub fn draw_walk(
        &mut self,
        cx: &mut Cx2d<'_>,
        walk: Walk,
        align: Align,
        text: impl Into<Substr>,
    ) -> makepad_platform::Rect {
        use crate::turtle;

        let laidout_text = self.layout_text(cx, walk, align, text);

        let max_width_in_lpxs = cx
            .turtle()
            .max_width(walk)
            .map_or(laidout_text.size_in_lpxs.width, |max_width| {
                max_width as f32
            });
        let max_height_in_lpxs = cx
            .turtle()
            .max_height(walk)
            .map_or(laidout_text.size_in_lpxs.height, |max_height| {
                max_height as f32
            });
        let rect = cx.walk_turtle(Walk {
            abs_pos: walk.abs_pos,
            margin: walk.margin,
            width: turtle::Size::Fixed(max_width_in_lpxs as f64),
            height: turtle::Size::Fixed(max_height_in_lpxs as f64),
        });

        if self.debug {
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                makepad_platform::rect(
                    cx.turtle().pos().x,
                    cx.turtle().pos().y,
                    max_width_in_lpxs as f64,
                    max_height_in_lpxs as f64,
                ),
            );
            cx.cx
                .debug
                .area(area, makepad_platform::vec4(1.0, 1.0, 1.0, 1.0));
        }

        let remaining_width_in_lpxs = max_width_in_lpxs - laidout_text.size_in_lpxs.width;
        let remaining_height_in_lpxs = max_height_in_lpxs - laidout_text.size_in_lpxs.height;
        let origin_in_lpxs = Point::new(
            rect.pos.x as f32 + align.x as f32 * remaining_width_in_lpxs,
            rect.pos.y as f32 + align.y as f32 * remaining_height_in_lpxs,
        );

        self.draw_laidout_text(cx, origin_in_lpxs, &laidout_text);

        makepad_platform::rect(
            origin_in_lpxs.x as f64,
            origin_in_lpxs.y as f64,
            laidout_text.size_in_lpxs.width as f64,
            laidout_text.size_in_lpxs.height as f64,
        )
    }

    fn draw_laidout_text(
        &mut self,
        cx: &mut Cx2d<'_>,
        origin_in_lpxs: Point<f32>,
        text: &LaidoutText,
    ) {
        self.update_draw_vars(cx);
        let mut instances: ManyInstances =
            cx.begin_many_aligned_instances(&self.draw_vars).unwrap();
        self.draw_depth = 1.0;
        for row in &text.rows {
            self.draw_laidout_row(
                cx,
                origin_in_lpxs + Size::from(row.origin_in_lpxs),
                row,
                &mut instances.instances,
            );
        }
        let area = cx.end_many_instances(instances);
        self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, area);
    }

    fn update_draw_vars(&mut self, cx: &mut Cx2d<'_>) {
        let fonts = cx.fonts.borrow();
        let settings = fonts.sdfer().borrow().settings();
        self.draw_vars.user_uniforms[0] = settings.radius;
        self.draw_vars.user_uniforms[1] = settings.cutoff;
        let grayscale_atlas_size = fonts.grayscale_atlas().borrow().size();
        self.draw_vars.user_uniforms[2] = grayscale_atlas_size.width as f32;
        self.draw_vars.user_uniforms[3] = grayscale_atlas_size.height as f32;
        let color_atlas_size = fonts.color_atlas().borrow().size();
        self.draw_vars.user_uniforms[4] = color_atlas_size.width as f32;
        self.draw_vars.user_uniforms[5] = color_atlas_size.height as f32;
        self.draw_vars.texture_slots[0] = Some(fonts.grayscale_texture().clone());
        self.draw_vars.texture_slots[1] = Some(fonts.color_texture().clone());
        drop(fonts);
    }

    fn draw_laidout_row(
        &mut self,
        cx: &mut Cx2d<'_>,
        origin_in_lpxs: Point<f32>,
        row: &LaidoutRow,
        output: &mut Vec<f32>,
    ) {
        for glyph in &row.glyphs {
            self.draw_laidout_glyph(
                cx,
                origin_in_lpxs + Size::from(glyph.origin_in_lpxs),
                glyph,
                output,
            );
        }

        if self.debug {
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                makepad_platform::rect(
                    origin_in_lpxs.x as f64,
                    (origin_in_lpxs.y - row.ascender_in_lpxs) as f64,
                    row.width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx
                .debug
                .area(area, makepad_platform::vec4(1.0, 0.0, 0.0, 1.0));

            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                makepad_platform::rect(
                    origin_in_lpxs.x as f64,
                    origin_in_lpxs.y as f64,
                    row.width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx
                .debug
                .area(area, makepad_platform::vec4(0.0, 1.0, 0.0, 1.0));

            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                makepad_platform::rect(
                    origin_in_lpxs.x as f64,
                    (origin_in_lpxs.y - row.descender_in_lpxs) as f64,
                    row.width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx
                .debug
                .area(area, makepad_platform::vec4(0.0, 0.0, 1.0, 1.0));
        }
    }

    fn draw_laidout_glyph(
        &mut self,
        cx: &mut Cx2d<'_>,
        origin_in_lpxs: Point<f32>,
        glyph: &LaidoutGlyph,
        output: &mut Vec<f32>,
    ) {
        let font_size_in_dpxs = glyph.font_size_in_lpxs * cx.current_dpi_factor() as f32;
        if let Some(rasterized_glyph) = glyph.rasterize(font_size_in_dpxs) {
            self.draw_rasterized_glyph(
                Point::new(origin_in_lpxs.x + glyph.offset_in_lpxs(), origin_in_lpxs.y),
                glyph.font_size_in_lpxs,
                glyph.color,
                rasterized_glyph,
                output,
            );
        }
    }

    fn draw_rasterized_glyph(
        &mut self,
        point_in_lpxs: Point<f32>,
        font_size_in_lpxs: f32,
        color: Option<Color>,
        glyph: RasterizedGlyph,
        output: &mut Vec<f32>,
    ) {
        fn tex_coord(point: Point<usize>, size: Size<usize>) -> Point<f32> {
            Point::new(
                (2 * point.x + 1) as f32 / (2 * size.width) as f32,
                (2 * point.y + 1) as f32 / (2 * size.height) as f32,
            )
        }

        let transform = Transform::from_scale_uniform(font_size_in_lpxs / glyph.dpxs_per_em)
            .translate(point_in_lpxs.x, point_in_lpxs.y);
        let bounds_in_lpxs = Rect::new(
            Point::new(glyph.bounds_in_dpxs.min().x, -glyph.bounds_in_dpxs.max().y),
            glyph.bounds_in_dpxs.size,
        )
        .apply_transform(transform);
        self.rect_pos = point_to_vec2(bounds_in_lpxs.origin);
        self.rect_size = size_to_vec2(bounds_in_lpxs.size);
        self.draw_color = color.map_or(self.color, color_to_vec4);
        self.texture_index = match glyph.atlas_kind {
            AtlasKind::Grayscale => 0.0,
            AtlasKind::Color => 1.0,
        };
        self.t_min = point_to_vec2(tex_coord(glyph.atlas_bounds.min(), glyph.atlas_size));
        self.t_max = point_to_vec2(tex_coord(glyph.atlas_bounds.max(), glyph.atlas_size));
        output.extend_from_slice(self.draw_vars.as_slice());
        self.draw_depth += 0.0001;
    }

    pub fn point_in_lpxs_to_cursor(
        &self,
        cx: &mut Cx2d<'_>,
        walk: Walk,
        align: Align,
        text: impl Into<Substr>,
        point_in_lpxs: DVec2,
    ) -> Cursor {
        let laidout_text = self.layout_text(cx, walk, align, text);
        laidout_text.point_in_lpxs_to_cursor(dvec2_to_point(point_in_lpxs))
    }

    pub fn cursor_to_point_in_lpxs(
        &self,
        cx: &mut Cx2d<'_>,
        walk: Walk,
        align: Align,
        text: impl Into<Substr>,
        cursor: Cursor,
    ) -> DVec2 {
        let laidout_text = self.layout_text(cx, walk, align, text);
        point_to_dvec2(laidout_text.cursor_to_point_in_lpxs(cursor))
    }

    fn layout_text(
        &self,
        cx: &mut Cx2d<'_>,
        walk: Walk,
        align: Align,
        text: impl Into<Substr>,
    ) -> Rc<LaidoutText> {
        use crate::text::layout::{LayoutOptions, LayoutParams, Span, Style};

        let text = text.into();
        let text_len = text.len();
        cx.fonts.borrow_mut().get_or_layout(LayoutParams {
            text,
            spans: [Span {
                style: Style {
                    font_family_id: self.text_style.font_family.clone().into(),
                    font_size_in_lpxs: self.text_style.font_size,
                    color: None,
                },
                range: 0..text_len,
            }]
            .into(),
            options: LayoutOptions {
                max_width_in_lpxs: cx
                    .turtle()
                    .max_width(walk)
                    .map(|max_width| max_width as f32),
                align: align.x as f32,
                line_spacing_scale: self.text_style.line_spacing as f32,
            },
        })
    }
}

#[derive(Debug, Clone, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct TextStyle {
    #[live]
    pub font_family: String,
    #[live]
    pub font_size: f32,
    #[live]
    pub line_spacing: f32,
}

fn dvec2_to_point(dvec: DVec2) -> Point<f32> {
    Point::new(dvec.x as f32, dvec.y as f32)
}

fn point_to_vec2(point: Point<f32>) -> Vec2 {
    makepad_platform::vec2(point.x, point.y)
}

fn point_to_dvec2(point: Point<f32>) -> DVec2 {
    makepad_platform::dvec2(point.x as f64, point.y as f64)
}

fn size_to_vec2(size: Size<f32>) -> Vec2 {
    makepad_platform::vec2(size.width, size.height)
}

fn color_to_vec4(color: Color) -> Vec4 {
    makepad_platform::vec4(
        color.r as f32,
        color.g as f32,
        color.b as f32,
        color.a as f32,
    ) / 255.0
}
