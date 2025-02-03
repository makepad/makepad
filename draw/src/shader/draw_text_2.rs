use {
    crate::{
        cx_2d::Cx2d,
        text::{
            font::AllocatedGlyph,
            font_family::FontFamily,
            geom::{Point, Size, Transformation},
            shaper::Glyph,
        },
    },
    makepad_platform::*,
};

pub struct DrawText2 {
    p_min: DVec2,
    p_max: DVec2,
    t_min: DVec2,
    t_max: DVec2,
}

impl DrawText2 {
    pub fn draw(
        &mut self,
        cx: &mut Cx2d<'_>,
        p: &mut Point<f32>,
        text: &str,
        font_family: &FontFamily,
        font_size_in_pxs: f32,
    ) {
        for glyph in &*font_family.shape(text) {
            self.draw_glyph(cx, p, glyph, font_size_in_pxs);
        }
    }

    fn draw_glyph(
        &mut self,
        cx: &mut Cx2d<'_>,
        p: &mut Point<f32>,
        glyph: &Glyph,
        font_size_in_pxs: f32,
    ) {
        if let Some(allocated_glyph) = glyph.allocate(font_size_in_pxs) {
            self.draw_allocated_glyph(
                cx,
                Point::new(p.x + font_size_in_pxs, p.y),
                allocated_glyph,
                font_size_in_pxs,
            );
        }
        let advance_in_pxs = glyph.advance_in_ems * font_size_in_pxs;
        p.x = advance_in_pxs;
    }

    fn draw_allocated_glyph(
        &mut self,
        cx: &mut Cx2d<'_>,
        p: Point<f32>,
        glyph: AllocatedGlyph,
        font_size_in_pxs: f32,
    ) {
        fn tex_coord(point: Point<usize>, size: Size<usize>) -> Point<f32> {
            Point::new(
                (2 * point.x + 1) as f32 / (2 * size.width) as f32,
                (2 * point.y + 1) as f32 / (2 * size.height) as f32,
            )
        }

        fn point_to_dvec2(point: Point<f32>) -> DVec2 {
            dvec2(point.x as f64, point.y as f64)
        }

        let transform = Transformation::scaling_uniform(font_size_in_pxs / glyph.pxs_per_em)
            .concat(Transformation::translation(p.x, p.y));
        self.p_min = point_to_dvec2(glyph.bounds_in_pxs.min().transform(transform));
        self.p_max = point_to_dvec2(glyph.bounds_in_pxs.max().transform(transform));
        self.t_min = point_to_dvec2(tex_coord(glyph.image_bounds.min(), glyph.atlas_size));
        self.t_max = point_to_dvec2(tex_coord(glyph.image_bounds.max(), glyph.atlas_size));
    }
}
