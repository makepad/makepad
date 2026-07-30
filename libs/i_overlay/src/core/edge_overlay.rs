use crate::build::builder::GraphBuilder;
use crate::core::edge_data::OverlayEdgeData;
use crate::core::extract::BooleanExtractionBuffer;
use crate::core::fill_rule::FillRule;
use crate::core::graph::OverlayNode;
use crate::core::overlay::{IntOverlayOptions, ShapeType};
use crate::core::overlay_rule::OverlayRule;
use crate::core::solver::Solver;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::segment::Segment;
use crate::split::solver::SplitSolver;
use crate::vector::edge::{DataVectorEdge, DataVectorShape};
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;
use i_key_sort::sort::key::SortKey;
use i_tree::{Expiration, LayoutNumber};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEdge<I: IntNumber, D> {
    pub a: IntPoint<I>,
    pub b: IntPoint<I>,
    pub data: D,
}

pub struct EdgeOverlay<I: IntNumber + Expiration, D: OverlayEdgeData> {
    pub solver: Solver,
    pub options: IntOverlayOptions<I::WideUInt>,
    pub boolean_buffer: Option<BooleanExtractionBuffer<I>>,
    data_store: D::Store,
    segments: Vec<Segment<ShapeCountBoolean, I, D>>,
    split_solver: SplitSolver<I>,
    graph_builder: GraphBuilder<ShapeCountBoolean, OverlayNode, I, D>,
}

impl<I, D> EdgeOverlay<I, D>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
    D: OverlayEdgeData,
{
    pub fn new(capacity: usize) -> Self {
        Self {
            solver: Default::default(),
            options: IntOverlayOptions::keep_output_points(),
            boolean_buffer: None,
            data_store: Default::default(),
            segments: Vec::with_capacity(capacity),
            split_solver: SplitSolver::new(),
            graph_builder: GraphBuilder::<ShapeCountBoolean, OverlayNode, I, D>::new(),
        }
    }

    pub fn add_edge(&mut self, edge: InputEdge<I, D>, shape_type: ShapeType) {
        if let Some(segment) =
            Segment::try_ab_and_data(edge.a, edge.b, shape_type, edge.data, &mut self.data_store)
        {
            self.segments.push(segment);
        }
    }

    pub fn add_edges<It>(&mut self, edges: It, shape_type: ShapeType)
    where
        It: IntoIterator<Item = InputEdge<I, D>>,
    {
        for edge in edges {
            self.add_edge(edge, shape_type);
        }
    }

    pub fn data_store(&self) -> &D::Store {
        &self.data_store
    }

    pub fn data_store_mut(&mut self) -> &mut D::Store {
        &mut self.data_store
    }

    pub fn into_data_store(self) -> D::Store {
        self.data_store
    }

    pub fn build_vectors(
        &mut self,
        overlay_rule: OverlayRule,
        fill_rule: FillRule,
    ) -> Vec<DataVectorEdge<I, D>> {
        self.split_solver
            .split_segments_with_store(&mut self.segments, &self.solver, &mut self.data_store);
        if self.segments.is_empty() {
            return Vec::new();
        }

        self.graph_builder
            .build_boolean_overlay(
                fill_rule,
                overlay_rule,
                self.options,
                &self.solver,
                &self.segments,
            )
            .extract_vectors_with_store(&mut self.data_store)
    }

    pub fn build_vector_shapes(
        &mut self,
        overlay_rule: OverlayRule,
        fill_rule: FillRule,
    ) -> Vec<DataVectorShape<I, D>> {
        self.split_solver
            .split_segments_with_store(&mut self.segments, &self.solver, &mut self.data_store);
        if self.segments.is_empty() {
            return Vec::new();
        }

        let mut buffer = self.boolean_buffer.take().unwrap_or_default();

        let shapes = self
            .graph_builder
            .build_boolean_overlay(
                fill_rule,
                overlay_rule,
                self.options,
                &self.solver,
                &self.segments,
            )
            .extract_vector_shapes_with_store(overlay_rule, &mut buffer, &mut self.data_store);

        self.boolean_buffer = Some(buffer);

        shapes
    }
}

impl<I, D> EdgeOverlay<I, D>
where
    I: IntNumber + Expiration + LayoutNumber + SortKey,
    D: OverlayEdgeData,
{
    pub fn edges(&self) -> impl Iterator<Item = [IntPoint<I>; 2]> + '_ {
        self.segments.iter().map(|e| [e.x_segment.a, e.x_segment.b])
    }
}
