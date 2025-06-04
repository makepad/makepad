use {
    crate::{
        cx_2d::Cx2d,
        cx_draw::CxDraw,
        geometry::GeometryQuad2D,
        makepad_platform::*,
        text::{
            color::Color,
            font::FontId,
            font_family::FontFamilyId,
            fonts::Fonts,
            geom::{Point, Rect, Size, Transform},
            layouter::{
                BorrowedLayoutParams, LaidoutGlyph, LaidoutRow, LaidoutText, LayoutOptions, Span,
                Style,
            },
            loader::{FontDefinition, FontFamilyDefinition},
            rasterizer::{AtlasKind, RasterizedGlyph},
            selection::{Cursor, Selection},
        },
        turtle::*,
        turtle::{Align, Walk},
    },
    std::{cell::RefCell, rc::Rc},
};

live_design! {
    use link::shaders::*;

    pub DrawText = {{DrawText}} {
        color: #ffff,

        uniform radius: float;
        uniform cutoff: float;
        uniform grayscale_atlas_size: vec2;
        uniform color_atlas_size: vec2;

        texture grayscale_texture: texture2d
        texture color_texture: texture2d

        varying pos: vec2
        varying t: vec2
        varying world: vec4
        
        fn vertex(self) -> vec4 {
            let p = mix(self.rect_pos, self.rect_pos + self.rect_size, self.geom_pos);
            let p_clipped = clamp(p, self.draw_clip.xy, self.draw_clip.zw);
            let p_normalized: vec2 = (p_clipped - self.rect_pos) / self.rect_size;

            self.pos = p_normalized;
            self.t = mix(self.t_min, self.t_max, p_normalized.xy);
            self.world = self.view_transform * vec4(
                p_clipped.x,
                p_clipped.y,
                self.glyph_depth + self.draw_zbias,
                1.
            );
            return self.camera_projection * (self.camera_view * (self.world));
        }

        fn sdf(self, scale: float, p: vec2) -> float {
            let s = sample2d(self.grayscale_texture, p).x;
            // 1.1 factor to compensate for text being magically too dark after the fontstack refactor
            s = clamp(((s - (1.0 - self.cutoff)) * self.radius / scale + 0.5)*1.1, 0.0, 1.0);
            return s;
        }

        fn get_color(self) -> vec4 {
            return self.color
        }
        
        fn fragment(self) -> vec4 {
            return depth_clip(self.world, self.pixel(), self.depth_clip);
        }
        
        fn pixel(self) -> vec4 {
            let dxt = length(dFdx(self.t));
            let dyt = length(dFdy(self.t));
            let color = #0000
            if self.texture_index == 0 {
                // TODO: Support non square atlases?
                let scale = (dxt + dyt) * self.grayscale_atlas_size.x * 0.5;
                let s = self.sdf(scale, self.t.xy);
                let c = self.get_color();
                return s * vec4(c.rgb * c.a, c.a);
            } else {
                let c = sample2d(self.color_texture, self.t);
                return vec4(c.rgb * c.a, c.a);
            }
        }
    }
}

#[derive(Live, LiveRegister)]
#[repr(C)]
pub struct DrawText {
    #[live]
    pub geometry: GeometryQuad2D,
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
    
    #[deref]
    pub draw_vars: DrawVars,
    #[calc]
    pub rect_pos: Vec2,
    #[calc]
    pub rect_size: Vec2,
    #[calc]
    pub draw_clip: Vec4,
    #[live(1.0)] 
    pub depth_clip: f32,
    #[calc]
    pub glyph_depth: f32,
    #[live]
    pub color: Vec4,
    #[calc]
    pub texture_index: f32,
    #[calc]
    pub t_min: Vec2,
    #[calc]
    pub t_max: Vec2,
}

impl LiveHook for DrawText {
    fn before_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars
            .before_apply_init_shader(cx, apply, index, nodes, &self.geometry);
    }

    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) {
        self.draw_vars
            .after_apply_update_self(cx, apply, index, nodes, &self.geometry);
    }
}

impl DrawText {
    pub fn draw_abs(&mut self, cx: &mut Cx2d, pos: DVec2, text: &str) {
        let text = self.layout(cx, 0.0, 0.0, None, false, Align::default(), text);
        self.draw_text(cx, Point::new(pos.x as f32, pos.y as f32), &text);
    }

    pub fn draw_walk(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        align: Align,
        text: &str,
    ) -> makepad_platform::Rect {
        let turtle_rect = cx.turtle().padded_rect();
        let max_width_in_lpxs = if !turtle_rect.size.x.is_nan() {
            Some(turtle_rect.size.x as f32)
        } else {
            None
        };
        let wrap = cx.turtle().layout().flow == Flow::RightWrap;

        let text = self.layout(cx, 0.0, 0.0, max_width_in_lpxs, wrap, align, text);
        self.draw_walk_laidout(cx, walk, &text)
    }

    pub fn draw_walk_laidout(
        &mut self,
        cx: &mut Cx2d,
        walk: Walk,
        laidout_text: &LaidoutText,
    ) -> makepad_platform::Rect {
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
        });

        if self.debug {
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(&mut area, turtle_rect);
            cx.cx.debug.area(area, vec4(1.0, 1.0, 1.0, 1.0));
        }

        let origin_in_lpxs = Point::new(
            turtle_rect.pos.x as f32,
            turtle_rect.pos.y as f32,
        );
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
        mut f: impl FnMut(&mut Cx2d, makepad_platform::Rect),
    ) {
        let turtle_pos = cx.turtle().pos();
        let turtle_rect = cx.turtle().padded_rect();
        let origin_in_lpxs = Point::new(turtle_rect.pos.x as f32, turtle_pos.y as f32);
        let first_row_indent_in_lpxs = turtle_pos.x as f32 - origin_in_lpxs.x;
        let row_height = cx.turtle().row_height();
        
        // lets draw a debug rect
        /*
        if text_str.starts_with("markdownedited"){
            let mut area = Area::Empty;
            cx.add_aligned_rect_area(
                &mut area,
                makepad_platform::rect(
                    origin_in_lpxs.x as f64,
                    origin_in_lpxs.y as f64,
                    100.0 as f64,
                    row_height as f64,
                ),
            );
            cx.cx
            .debug
            .area(area, makepad_platform::vec4(1.0, 0.0, 0.0, 1.0));
        }*/
        
        let max_width_in_lpxs = if !turtle_rect.size.x.is_nan() {
            Some(turtle_rect.size.x as f32)
        } else {
            None
        };
        let wrap = cx.turtle().layout().flow == Flow::RightWrap;

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
        
        turtle.set_pos(new_turtle_pos);
        turtle.update_width_max(origin_in_lpxs.x as f64, used_size_in_lpxs.width as f64);
        turtle.update_height_max(origin_in_lpxs.y as f64, used_size_in_lpxs.height as f64);
        
        turtle.set_wrap_spacing((
           last_row.line_spacing_above_in_lpxs - last_row.ascender_in_lpxs
        )as f64);
        
        cx.emit_turtle_walk(makepad_platform::Rect {
            pos: new_turtle_pos,
            size: dvec2(
                used_size_in_lpxs.width as f64,
                used_size_in_lpxs.height as f64,
            ),
        });
        
        let shift = if let Some(row) = text.rows.get(0){
            if let Some(glyph) = row.glyphs.get(0){
                glyph.font_size_in_lpxs * self.temp_y_shift
            }
            else{0.0}
        }
        else{0.0};
                
        for rect_in_lpxs in text.selection_rects_in_lpxs(Selection {
            anchor: Cursor {
                index: 0,
                prefer_next_row: false,
            },
            cursor: Cursor {
                index: text.text.len(),
                prefer_next_row: false,
            },
        }) {
            let rect_in_lpxs = Rect::new(
                origin_in_lpxs + Size::from(rect_in_lpxs.origin) * self.font_scale,
                rect_in_lpxs.size * self.font_scale,
            );
            f(
                cx,
                makepad_platform::rect(
                    rect_in_lpxs.origin.x as f64,
                    rect_in_lpxs.origin.y as f64 + shift as f64,
                    rect_in_lpxs.size.width as f64,
                    rect_in_lpxs.size.height as f64,
                ),
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
        CxDraw::lazy_construct_fonts(cx);
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let mut fonts = fonts.borrow_mut();

        let text_len = text.len();
        fonts.get_or_layout(BorrowedLayoutParams {
            text,
            spans: &[Span {
                style: Style {
                    font_family_id: self.text_style.font_family.to_font_family_id(),
                    font_size_in_pts: self.text_style.font_size,
                    color: None,
                },
                len: text_len,
            }],
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
        let area = cx.end_many_instances(instances);
        self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, area);
    }

    fn update_draw_vars(&mut self, cx: &mut Cx2d) {
        let fonts = cx.fonts.borrow();
        let rasterizer = fonts.rasterizer().borrow();
        let sdfer_settings = rasterizer.sdfer().settings();
        self.draw_vars.user_uniforms[0] = sdfer_settings.radius;
        self.draw_vars.user_uniforms[1] = sdfer_settings.cutoff;
        let grayscale_atlas_size = rasterizer.grayscale_atlas().size();
        self.draw_vars.user_uniforms[2] = grayscale_atlas_size.width as f32;
        self.draw_vars.user_uniforms[3] = grayscale_atlas_size.height as f32;
        let color_atlas_size = rasterizer.color_atlas().size();
        self.draw_vars.user_uniforms[4] = color_atlas_size.width as f32;
        self.draw_vars.user_uniforms[5] = color_atlas_size.height as f32;
        self.draw_vars.texture_slots[0] = Some(fonts.grayscale_texture().clone());
        self.draw_vars.texture_slots[1] = Some(fonts.color_texture().clone());
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
                makepad_platform::rect(
                    origin_in_lpxs.x as f64,
                    (origin_in_lpxs.y - row.ascender_in_lpxs * self.font_scale) as f64,
                    width_in_lpxs as f64,
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
                    width_in_lpxs as f64,
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
                    (origin_in_lpxs.y - row.descender_in_lpxs * self.font_scale) as f64,
                    width_in_lpxs as f64,
                    1.0,
                ),
            );
            cx.cx
                .debug
                .area(area, makepad_platform::vec4(0.0, 0.0, 1.0, 1.0));
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
        };

        let atlas_image_bounds = glyph.atlas_image_bounds;
        let atlas_size = glyph.atlas_size;
        let t_min = tex_coord(glyph.atlas_image_bounds.min(), atlas_size);
        let t_max = tex_coord(glyph.atlas_image_bounds.max(), atlas_size);

        let atlas_image_padding = glyph.atlas_image_padding;
        let atlas_image_size = atlas_image_bounds.size;
        let origin_in_dpxs = glyph.origin_in_dpxs;
        let bounds_in_dpxs = Rect::new(
            Point::new(
                origin_in_dpxs.x - atlas_image_padding as f32,
                -origin_in_dpxs.y - atlas_image_size.height as f32 + (atlas_image_padding as f32),
            ),
            Size::new(atlas_image_size.width as f32, atlas_image_size.height as f32),
        );
        let bounds_in_lpxs = bounds_in_dpxs.apply_transform(
            Transform::from_scale_uniform(font_size_in_lpxs / glyph.dpxs_per_em * self.font_scale)
                .translate(origin_in_lpxs.x, origin_in_lpxs.y),
        );

        self.rect_pos = vec2(bounds_in_lpxs.origin.x, bounds_in_lpxs.origin.y) + vec2(0.0,self.temp_y_shift* font_size_in_lpxs);
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
        self.t_min = vec2(t_min.x, t_min.y);
        self.t_max = vec2(t_max.x, t_max.y);

        output.extend_from_slice(self.draw_vars.as_slice());
        self.glyph_depth += 0.000001;
    }
}

#[derive(Debug, Clone, Live, LiveHook, LiveRegister)]
#[live_ignore]
pub struct TextStyle {
    #[live]
    pub font_family: FontFamily,
    #[live(10.0)]
    pub font_size: f32,
    #[live(1.0)]
    pub line_spacing: f32,
}

#[derive(Debug, Clone, Live, LiveRegister, PartialEq)]
pub struct FontFamily {
    #[rust]
    id: LiveId,
}

impl FontFamily {
    fn to_font_family_id(&self) -> FontFamilyId {
        (self.id.0).into()
    }
}

impl LiveHook for FontFamily {
    fn skip_apply(
        &mut self,
        cx: &mut Cx,
        _apply: &mut Apply,
        index: usize,
        nodes: &[LiveNode],
    ) -> Option<usize> {
        CxDraw::lazy_construct_fonts(cx);
        let fonts = cx.get_global::<Rc<RefCell<Fonts>>>().clone();
        let mut fonts = fonts.borrow_mut();

        let mut id = LiveId::seeded();
        let mut next_child_index = Some(index + 1);
        while let Some(child_index) = next_child_index {
            if let LiveValue::Font(font) = &nodes[child_index].value {
                id = id.id_append(font.to_live_id());
            }
            next_child_index = nodes.next_child(child_index);
        }
        self.id = id;

        let font_family_id = self.to_font_family_id();
        if !fonts.is_font_family_known(font_family_id) {
            let mut font_ids = Vec::new();
            let mut next_child_index = Some(index + 1);
            while let Some(child_index) = next_child_index {
                if let LiveValue::Font(font) = &nodes[child_index].value {
                    let font_id: FontId = (font.to_live_id().0).into();
                    if !fonts.is_font_known(font_id) {
                        // alright so if we have a multipart font we have to combine it here
                        let data = if font.paths.len()>1{
                            // combine them. TODO do this better.
                            let mut data = Vec::new();
                            for path in &*font.paths{
                                let dep = cx.get_dependency(path).unwrap();
                                data.extend(&*dep);
                            }
                            Rc::new(data)
                        }
                        else{
                            cx.get_dependency(font.paths[0].as_str()).unwrap().into()
                        };
                        fonts.define_font(
                            font_id,
                            FontDefinition {
                                data,
                                index: 0,
                                ascender_fudge_in_ems: font.ascender_fudge,
                                descender_fudge_in_ems: font.descender_fudge,
                            },
                        );
                    }
                    font_ids.push(font_id);
                }
                next_child_index = nodes.next_child(child_index);
            }
            fonts.define_font_family(font_family_id, FontFamilyDefinition { font_ids });
        }

        Some(nodes.skip_node(index))
    }
}
