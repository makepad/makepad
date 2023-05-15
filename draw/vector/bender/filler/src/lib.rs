pub use bender_clipper::FillRule;

use bender_clipper;
use bender_clipper::{Clipper, Operation};
use bender_geometry::mesh::Writer;
use bender_geometry::{Mesh, Polygon};
use bender_tessellator;
use bender_tessellator::{ActiveEdge, Tessellator};

#[derive(Clone, Debug)]
pub struct Filler {
    clipper: Clipper,
    clipper_pending_edges: Vec<bender_clipper::PendingEdge>,
    clipper_left_edges: Vec<usize>,
    clipper_right_edges: Vec<usize>,
    clipper_left_boundary_edge_indices: Vec<usize>,
    clipper_right_boundary_edge_indices: Vec<usize>,
    tessellator: Tessellator,
    tessellator_pending_edges: Vec<bender_tessellator::PendingEdge>,
    tessellator_left_edges: Vec<ActiveEdge>,
}

impl Filler {
    pub fn new() -> Self {
        Self {
            clipper: Clipper::new(),
            clipper_pending_edges: Vec::new(),
            clipper_left_edges: Vec::new(),
            clipper_right_edges: Vec::new(),
            clipper_left_boundary_edge_indices: Vec::new(),
            clipper_right_boundary_edge_indices: Vec::new(),
            tessellator: Tessellator::new(),
            tessellator_pending_edges: Vec::new(),
            tessellator_left_edges: Vec::new(),
        }
    }

    pub fn fill(&mut self, polygons: &[Polygon], fill_rule: FillRule, output_mesh: &mut Mesh) {
        self.tessellator.tessellate(
            self.clipper.clip_polygons(
                Operation::Union,
                polygons,
                &[],
                bender_clipper::Options {
                    subject_fill_rule: fill_rule,
                    ..bender_clipper::Options::default()
                },
                &mut self.clipper_pending_edges,
                &mut self.clipper_left_edges,
                &mut self.clipper_right_edges,
                &mut self.clipper_left_boundary_edge_indices,
                &mut self.clipper_right_boundary_edge_indices,
            ),
            &mut Writer::new(output_mesh),
            &mut self.tessellator_pending_edges,
            &mut self.tessellator_left_edges,
        );
    }
}
