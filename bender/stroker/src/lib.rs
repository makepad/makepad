pub use bender_filler::FillRule;
pub use bender_offsetter::{CapKind, JoinKind};

use bender_filler::Filler;
use bender_geometry::{Mesh, Polygon, Polyline};

#[derive(Clone, Debug)]
pub struct Stroker {
    filler: Filler,
    offset_polygons: Vec<Polygon>,
}

impl Stroker {
    pub fn new() -> Self {
        Self {
            filler: Filler::new(),
            offset_polygons: Vec::new(),
        }
    }

    pub fn stroke(
        &mut self,
        polylines: &[Polyline],
        Options {
            stroke_width,
            join_kind,
            cap_kind,
            miter_limit,
            arc_tolerance,
            fill_rule,
        }: Options,
        output_mesh: &mut Mesh,
    ) {
        for polyline in polylines {
            self.offset_polygons.push(bender_offsetter::offset_polyline(
                polyline,
                stroke_width / 2.0,
                bender_offsetter::Options {
                    join_kind,
                    cap_kind,
                    miter_limit,
                    arc_tolerance,
                },
            ));
        }
        self.filler
            .fill(&self.offset_polygons, fill_rule, output_mesh);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub stroke_width: f32,
    pub join_kind: JoinKind,
    pub cap_kind: CapKind,
    pub miter_limit: f32,
    pub arc_tolerance: f32,
    pub fill_rule: FillRule,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            stroke_width: 0.05,
            join_kind: JoinKind::default(),
            cap_kind: CapKind::default(),
            miter_limit: 10.0,
            arc_tolerance: 1E-2,
            fill_rule: FillRule::default(),
        }
    }
}
