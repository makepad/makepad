use bender_arena::Arena;
use bender_geometry::mesh::Callbacks;
use bender_geometry::{LineSegment, Point};
use std::cmp::Ordering;

#[derive(Clone, Debug, Default)]
pub struct MonotoneTessellator {
    monotone_polygon_pool: Vec<MonotonePolygon>,
    monotone_polygon_arena: Arena<MonotonePolygon>,
}

impl MonotoneTessellator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_monotone_polygon(&mut self, index: u16, position: Point) -> usize {
        let mut monotone_polygon = if let Some(monotone_polygon) = self.monotone_polygon_pool.pop()
        {
            monotone_polygon
        } else {
            MonotonePolygon::new()
        };
        monotone_polygon.start(index, position);
        self.monotone_polygon_arena.insert(monotone_polygon)
    }

    pub fn finish_monotone_polygon(
        &mut self,
        monotone_polygon: usize,
        index: u16,
        callbacks: &mut impl Callbacks,
    ) {
        let mut monotone_polygon = self.monotone_polygon_arena.remove(monotone_polygon);
        monotone_polygon.finish(index, callbacks);
        self.monotone_polygon_pool.push(monotone_polygon);
    }

    pub fn push_vertex(
        &mut self,
        monotone_polygon: usize,
        side: Side,
        index: u16,
        position: Point,
        callbacks: &mut impl Callbacks,
    ) {
        self.monotone_polygon_arena[monotone_polygon].push_vertex(side, index, position, callbacks);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    Lower,
    Upper,
}

impl Default for Side {
    fn default() -> Self {
        Side::Lower
    }
}

#[derive(Clone, Debug, Default)]
struct MonotonePolygon {
    side: Side,
    vertex_stack: Vec<Vertex>,
}

impl MonotonePolygon {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&mut self, index: u16, position: Point) {
        self.vertex_stack.push(Vertex { index, position });
    }

    pub fn finish(&mut self, index: u16, callbacks: &mut impl Callbacks) {
        let mut vertex_1 = self.vertex_stack.pop().unwrap();
        while let Some(vertex_0) = self.vertex_stack.pop() {
            callbacks.triangle(vertex_0.index, vertex_1.index, index);
            vertex_1 = vertex_0;
        }
    }

    pub fn push_vertex(
        &mut self,
        side: Side,
        index: u16,
        position: Point,
        callbacks: &mut impl Callbacks,
    ) {
        if side == self.side {
            let mut vertex_1 = self.vertex_stack.pop().unwrap();
            loop {
                let vertex_0 = if let Some(vertex_0) = self.vertex_stack.last().cloned() {
                    vertex_0
                } else {
                    break;
                };
                match (
                    LineSegment::new(vertex_0.position, position)
                        .compare_to_point(vertex_1.position)
                        .unwrap(),
                    side,
                ) {
                    (Ordering::Less, Side::Lower) => break,
                    (Ordering::Equal, _) => break,
                    (Ordering::Greater, Side::Upper) => break,
                    _ => (),
                };
                self.vertex_stack.pop();
                callbacks.triangle(vertex_0.index, vertex_1.index, index);
                vertex_1 = vertex_0;
            }
            self.vertex_stack.push(vertex_1);
            self.vertex_stack.push(Vertex { index, position });
        } else {
            let vertex = self.vertex_stack.pop().unwrap();
            let mut vertex_1 = vertex;
            while let Some(vertex_0) = self.vertex_stack.pop() {
                callbacks.triangle(vertex_0.index, vertex_1.index, index);
                vertex_1 = vertex_0;
            }
            self.vertex_stack.push(vertex);
            self.vertex_stack.push(Vertex { index, position });
            self.side = side;
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Vertex {
    index: u16,
    position: Point,
}
