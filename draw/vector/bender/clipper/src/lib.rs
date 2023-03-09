use bender_arena::Arena;
use bender_geometry::linear_path::Command;
use bender_geometry::{LineSegment, Point, Polygon};
use bender_internal_iter::{InternalIterator, IntoInternalIterator};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, VecDeque};
use std::iter;
use std::mem;
use std::ops::{Add, AddAssign, Neg, Range};

#[derive(Clone, Debug)]
pub struct Clipper {
    active_edge_arena: Arena<ActiveEdge>,
    active_polygon_arena: Arena<ActivePolygon>,
    active_edges: Vec<usize>,
    event_queue: BinaryHeap<Event>,
}

impl Clipper {
    pub fn new() -> Self {
        Self {
            active_edge_arena: Arena::new(),
            active_polygon_arena: Arena::new(),
            active_edges: Vec::new(),
            event_queue: BinaryHeap::new(),
        }
    }

    pub fn clip_polygons<'a, 'b>(
        &'a mut self,
        operation: Operation,
        subject_polygons: &[Polygon],
        clip_polygons: &[Polygon],
        options: Options,
        pending_edges: &'b mut Vec<PendingEdge>,
        left_edges: &'b mut Vec<usize>,
        right_edges: &'b mut Vec<usize>,
        left_boundary_edge_indices: &'b mut Vec<usize>,
        right_boundary_edge_indices: &'b mut Vec<usize>,
    ) -> Clip<'a, 'b> {
        self.clip(
            operation,
            subject_polygons
                .iter()
                .flat_map(|polygon| polygon.commands()),
            clip_polygons.iter().flat_map(|polygon| polygon.commands()),
            options,
            pending_edges,
            left_edges,
            right_edges,
            left_boundary_edge_indices,
            right_boundary_edge_indices,
        )
    }

    pub fn clip<'a, 'b>(
        &'a mut self,
        operation: Operation,
        subject_commands: impl IntoInternalIterator<Item = Command>,
        clip_commands: impl IntoInternalIterator<Item = Command>,
        Options {
            subject_fill_rule,
            clip_fill_rule,
        }: Options,
        pending_edges: &'b mut Vec<PendingEdge>,
        left_edges: &'b mut Vec<usize>,
        right_edges: &'b mut Vec<usize>,
        left_boundary_edge_indices: &'b mut Vec<usize>,
        right_boundary_edge_indices: &'b mut Vec<usize>,
    ) -> Clip<'a, 'b> {
        self.push_events_for_path(PathKind::Subject, subject_commands.into_internal_iter());
        self.push_events_for_path(PathKind::Clip, clip_commands.into_internal_iter());
        Clip {
            clipper: self,
            operation,
            subject_fill_rule,
            clip_fill_rule,
            pending_edges,
            left_edges,
            right_edges,
            left_boundary_edge_indices,
            right_boundary_edge_indices,
        }
    }

    fn push_events_for_path(
        &mut self,
        kind: PathKind,
        commands: impl InternalIterator<Item = Command>,
    ) {
        let mut initial_point = None;
        let mut current_point = None;
        commands.for_each(&mut |command| {
            match command {
                Command::MoveTo(point) => {
                    if let Some(initial_point) = initial_point {
                        let current_point = current_point.unwrap();
                        if initial_point != current_point {
                            panic!();
                        }
                    }
                    initial_point = Some(point);
                    current_point = Some(point);
                }
                Command::Close => {
                    self.push_events_for_edge(kind, current_point.unwrap(), initial_point.unwrap());
                    current_point = initial_point;
                }
                Command::LineTo(point) => {
                    self.push_events_for_edge(kind, current_point.unwrap(), point);
                    current_point = Some(point);
                }
            }
            true
        });
    }

    fn push_events_for_edge(&mut self, kind: PathKind, start: Point, end: Point) {
        let (winding, start, end) = match start.partial_cmp(&end) {
            Some(Ordering::Less) => (1, start, end),
            Some(Ordering::Equal) => return,
            Some(Ordering::Greater) => (-1, end, start),
            None => panic!(),
        };
        self.event_queue.push(Event {
            vertex: start,
            pending_edge: Some(PendingEdge {
                windings: match kind {
                    PathKind::Subject => Windings {
                        subject_winding: winding,
                        clip_winding: 0,
                    },
                    PathKind::Clip => Windings {
                        subject_winding: 0,
                        clip_winding: winding,
                    },
                },
                end,
            }),
        });
        self.event_queue.push(Event {
            vertex: end,
            pending_edge: None,
        });
    }

    fn pop_events_for_vertex(&mut self, pending_edges: &mut Vec<PendingEdge>) -> Option<Point> {
        if let Some(event) = self.event_queue.pop() {
            if let Some(pending_edge) = event.pending_edge {
                pending_edges.push(pending_edge);
            }
            loop {
                let next_event = if let Some(next_event) = self.event_queue.peek().cloned() {
                    next_event
                } else {
                    break;
                };
                if event != next_event {
                    break;
                }
                self.event_queue.pop();
                if let Some(pending_edge) = next_event.pending_edge {
                    pending_edges.push(pending_edge);
                }
            }
            return Some(event.vertex);
        }
        None
    }

    fn handle_events_for_vertex(
        &mut self,
        vertex: Point,
        is_interior: impl Fn(Windings) -> bool,
        f: &mut impl FnMut(Command) -> bool,
        pending_edges: &mut Vec<PendingEdge>,
        left_edges: &mut Vec<usize>,
        right_edges: &mut Vec<usize>,
        left_boundary_edge_indices: &mut Vec<usize>,
        right_boundary_edge_indices: &mut Vec<usize>,
    ) -> bool {
        let mut incident_edges_range = self.find_incident_edges_range(vertex);
        self.remove_and_split_incident_edges(
            vertex,
            &mut incident_edges_range,
            left_edges,
            pending_edges,
        );
        self.sort_and_splice_pending_edges(vertex, pending_edges);
        self.create_right_edges(
            vertex,
            incident_edges_range.start,
            is_interior,
            pending_edges,
            right_edges,
        );
        if !self.handle_boundary_edges(
            vertex,
            left_edges,
            right_edges,
            f,
            left_boundary_edge_indices,
            right_boundary_edge_indices,
        ) {
            return false;
        }
        self.destroy_left_edges(left_edges);
        self.insert_right_edges(&mut incident_edges_range.end, right_edges);
        self.intersect_active_edges(vertex, incident_edges_range);
        true
    }

    fn find_incident_edges_range(&mut self, vertex: Point) -> Range<usize> {
        Range {
            start: self
                .active_edges
                .iter()
                .position(|&active_edge| {
                    self.active_edge_arena[active_edge]
                        .edge
                        .compare_to_point(vertex)
                        .unwrap()
                        != Ordering::Less
                })
                .unwrap_or(self.active_edges.len()),
            end: self
                .active_edges
                .iter()
                .rposition(|&active_edge| {
                    self.active_edge_arena[active_edge]
                        .edge
                        .compare_to_point(vertex)
                        .unwrap()
                        != Ordering::Greater
                })
                .map_or(0, |index| index + 1),
        }
    }

    fn remove_and_split_incident_edges(
        &mut self,
        vertex: Point,
        incident_edges_range: &mut Range<usize>,
        left_edges: &mut Vec<usize>,
        pending_edges: &mut Vec<PendingEdge>,
    ) {
        left_edges.extend(self.active_edges.drain(incident_edges_range.clone()).map({
            let active_edge_arena = &mut self.active_edge_arena;
            move |incident_edge| {
                if let Some(pending_edge) = active_edge_arena[incident_edge].split_mut(vertex) {
                    pending_edges.push(pending_edge);
                }
                incident_edge
            }
        }));
        incident_edges_range.end = incident_edges_range.start;
    }

    fn sort_and_splice_pending_edges(
        &mut self,
        vertex: Point,
        pending_edges: &mut Vec<PendingEdge>,
    ) {
        pending_edges.sort_by(|&pending_edge_0, &pending_edge_1| {
            pending_edge_0.compare(pending_edge_1, vertex)
        });
        let mut index_0 = 0;
        for index_1 in 1..pending_edges.len() {
            let pending_edge_1 = pending_edges[index_1];
            let pending_edge_0 = &mut pending_edges[index_0];
            if pending_edge_0.overlaps(pending_edge_1, vertex) {
                if let Some(event) = pending_edge_0.splice_mut(pending_edge_1) {
                    self.event_queue.push(event);
                }
            } else {
                index_0 += 1;
                pending_edges[index_0] = pending_edge_1;
            }
        }
        pending_edges.truncate(index_0 + 1);
    }

    fn create_right_edges(
        &mut self,
        vertex: Point,
        incident_edges_start: usize,
        is_interior: impl Fn(Windings) -> bool,
        pending_edges: &[PendingEdge],
        right_edges: &mut Vec<usize>,
    ) {
        let mut lower_region = self.last_lower_region(incident_edges_start);
        right_edges.extend(pending_edges.iter().map(|&pending_edge| {
            let windings = lower_region.windings + pending_edge.windings;
            let upper_region = {
                Region {
                    is_interior: is_interior(windings),
                    windings,
                }
            };
            let active_edge = self.active_edge_arena.insert(ActiveEdge {
                is_boundary: lower_region.is_interior != upper_region.is_interior,
                windings: pending_edge.windings,
                edge: pending_edge.to_edge(vertex),
                upper_region,
                active_polygon: None,
            });
            lower_region = upper_region;
            active_edge
        }));
    }

    fn last_lower_region(&self, incident_edges_start: usize) -> Region {
        if 0 == incident_edges_start {
            Region {
                is_interior: false,
                windings: Windings {
                    subject_winding: 0,
                    clip_winding: 0,
                },
            }
        } else {
            self.active_edge_arena[self.active_edges[incident_edges_start - 1]].upper_region
        }
    }

    fn handle_boundary_edges(
        &mut self,
        vertex: Point,
        left_edges: &[usize],
        right_edges: &[usize],
        f: &mut impl FnMut(Command) -> bool,
        left_boundary_edge_indices: &mut Vec<usize>,
        right_boundary_edge_indices: &mut Vec<usize>,
    ) -> bool {
        self.find_boundary_edge_indices(left_edges, left_boundary_edge_indices);
        self.find_boundary_edge_indices(right_edges, right_boundary_edge_indices);
        for left_boundary_edge_index_pair in left_boundary_edge_indices.windows(2) {
            if !self.join_left_boundary_edge_pair(
                vertex,
                left_edges[left_boundary_edge_index_pair[0]],
                left_edges[left_boundary_edge_index_pair[1]],
                f,
            ) {
                return false;
            }
        }
        match (
            !left_boundary_edge_indices.is_empty(),
            !right_boundary_edge_indices.is_empty(),
        ) {
            (true, false) => {
                if !self.join_left_boundary_edge_pair(
                    vertex,
                    left_edges[*left_boundary_edge_indices.last().unwrap()],
                    left_edges[*left_boundary_edge_indices.first().unwrap()],
                    f,
                ) {
                    return false;
                }
            }
            (true, true) => {
                self.join_lower_boundary_edge_pair(
                    vertex,
                    left_edges[*left_boundary_edge_indices.first().unwrap()],
                    right_edges[*right_boundary_edge_indices.first().unwrap()],
                );
                self.join_upper_boundary_edge_pair(
                    vertex,
                    left_edges[*left_boundary_edge_indices.last().unwrap()],
                    right_edges[*right_boundary_edge_indices.last().unwrap()],
                );
            }
            (false, true) => {
                self.join_right_boundary_edge_pair(
                    vertex,
                    right_edges[*right_boundary_edge_indices.last().unwrap()],
                    right_edges[*right_boundary_edge_indices.first().unwrap()],
                );
            }
            _ => (),
        }
        for right_boundary_edge_index_pair in right_boundary_edge_indices.windows(2) {
            self.join_right_boundary_edge_pair(
                vertex,
                right_edges[right_boundary_edge_index_pair[0]],
                right_edges[right_boundary_edge_index_pair[1]],
            );
        }
        true
    }

    fn find_boundary_edge_indices(
        &mut self,
        active_edges: &[usize],
        boundary_edge_indices: &mut Vec<usize>,
    ) {
        boundary_edge_indices.extend(active_edges.iter().cloned().enumerate().filter_map(
            |(index, active_edge)| {
                if self.active_edge_arena[active_edge].is_boundary {
                    Some(index)
                } else {
                    None
                }
            },
        ));
    }

    fn join_left_boundary_edge_pair(
        &mut self,
        vertex: Point,
        left_boundary_edge_0: usize,
        left_boundary_edge_1: usize,
        f: &mut impl FnMut(Command) -> bool,
    ) -> bool {
        let active_polygon_0 = if let Some(active_polygon) =
            self.active_edge_arena[left_boundary_edge_0].upper_active_polygon()
        {
            active_polygon
        } else {
            return true;
        };
        let active_polygon_1 = self.active_edge_arena[left_boundary_edge_1]
            .lower_active_polygon()
            .unwrap();
        if active_polygon_0 == active_polygon_1 {
            if !self.finish_left_active_polygon(vertex, active_polygon_0, f) {
                return false;
            }
        } else {
            self.concat_left_active_polygon_pair(vertex, active_polygon_0, active_polygon_1);
        }
        true
    }

    fn join_lower_boundary_edge_pair(
        &mut self,
        vertex: Point,
        lower_boundary_edge_0: usize,
        lower_boundary_edge_1: usize,
    ) {
        let active_polygon = if let Some(active_polygon) =
            self.active_edge_arena[lower_boundary_edge_0].lower_active_polygon()
        {
            active_polygon
        } else {
            return;
        };
        self.active_polygon_arena[active_polygon]
            .vertices
            .push_front(vertex);
        self.active_edge_arena[lower_boundary_edge_1].active_polygon = Some(active_polygon);
        self.active_polygon_arena[active_polygon].front_boundary_edge = lower_boundary_edge_1;
    }

    fn join_upper_boundary_edge_pair(
        &mut self,
        vertex: Point,
        upper_boundary_edge_0: usize,
        upper_boundary_edge_1: usize,
    ) {
        let active_polygon = if let Some(active_polygon) =
            self.active_edge_arena[upper_boundary_edge_0].upper_active_polygon()
        {
            active_polygon
        } else {
            return;
        };
        self.active_polygon_arena[active_polygon]
            .vertices
            .push_back(vertex);
        self.active_edge_arena[upper_boundary_edge_1].active_polygon = Some(active_polygon);
        self.active_polygon_arena[active_polygon].back_boundary_edge = upper_boundary_edge_1;
    }

    fn join_right_boundary_edge_pair(
        &mut self,
        vertex: Point,
        right_boundary_edge_0: usize,
        right_boundary_edge_1: usize,
    ) {
        if !self.active_edge_arena[right_boundary_edge_0]
            .upper_region
            .is_interior
        {
            return;
        }
        self.insert_active_polygon(ActivePolygon {
            vertices: Iterator::collect(iter::once(vertex)),
            front_boundary_edge: right_boundary_edge_1,
            back_boundary_edge: right_boundary_edge_0,
        });
    }

    fn finish_left_active_polygon(
        &mut self,
        vertex: Point,
        active_polygon: usize,
        f: &mut impl FnMut(Command) -> bool,
    ) -> bool {
        let vertices = self.active_polygon_arena.remove(active_polygon).vertices;
        if !f(Command::MoveTo(vertex)) {
            return false;
        }
        for vertex in vertices {
            if !f(Command::LineTo(vertex)) {
                return false;
            }
        }
        f(Command::Close)
    }

    fn concat_left_active_polygon_pair(
        &mut self,
        vertex: Point,
        active_polygon_0: usize,
        active_polygon_1: usize,
    ) {
        let ActivePolygon {
            vertices: vertices_0,
            front_boundary_edge,
            ..
        } = self.active_polygon_arena.remove(active_polygon_0);
        let ActivePolygon {
            vertices: mut vertices_1,
            back_boundary_edge,
            ..
        } = self.active_polygon_arena.remove(active_polygon_1);
        self.insert_active_polygon(ActivePolygon {
            vertices: {
                let mut vertices = vertices_0;
                vertices.push_back(vertex);
                vertices.append(&mut vertices_1);
                vertices
            },
            front_boundary_edge,
            back_boundary_edge,
        });
    }

    fn insert_active_polygon(&mut self, active_polygon: ActivePolygon) {
        let front_boundary_edge = active_polygon.front_boundary_edge;
        let back_boundary_edge = active_polygon.back_boundary_edge;
        let active_polygon = self.active_polygon_arena.insert(active_polygon);
        self.active_edge_arena[front_boundary_edge].active_polygon = Some(active_polygon);
        self.active_edge_arena[back_boundary_edge].active_polygon = Some(active_polygon);
    }

    fn destroy_left_edges(&mut self, left_edges: &[usize]) {
        for left_edge in left_edges.iter().cloned() {
            self.active_edge_arena.remove(left_edge);
        }
    }

    fn insert_right_edges(&mut self, incident_edges_end: &mut usize, right_edges: &[usize]) {
        self.active_edges.splice(
            *incident_edges_end..*incident_edges_end,
            right_edges.iter().cloned(),
        );
        *incident_edges_end += right_edges.len();
    }

    fn intersect_active_edges(&mut self, vertex: Point, incident_edges_range: Range<usize>) {
        if 0 < incident_edges_range.start && incident_edges_range.start < self.active_edges.len() {
            self.intersect_active_edge_pair(
                vertex,
                self.active_edges[incident_edges_range.start - 1],
                self.active_edges[incident_edges_range.start],
            );
        }
        if incident_edges_range.start < incident_edges_range.end
            && incident_edges_range.end < self.active_edges.len()
        {
            self.intersect_active_edge_pair(
                vertex,
                self.active_edges[incident_edges_range.end - 1],
                self.active_edges[incident_edges_range.end],
            );
        }
    }

    fn intersect_active_edge_pair(
        &mut self,
        vertex: Point,
        active_edge_0: usize,
        active_edge_1: usize,
    ) {
        let edge_0 = self.active_edge_arena[active_edge_0].edge;
        let edge_1 = self.active_edge_arena[active_edge_1].edge;
        if edge_0.end() == edge_1.end() {
            return;
        }
        let intersection = if let Some(intersection) = edge_0.intersect(edge_1) {
            edge_0
                .bounds()
                .intersect(edge_1.bounds())
                .clamp(intersection)
                .max(vertex)
        } else {
            return;
        };
        self.split_active_edge(active_edge_0, intersection);
        self.split_active_edge(active_edge_1, intersection);
    }

    fn split_active_edge(&mut self, active_edge: usize, mut vertex: Point) {
        if let Some(mut pending_edge) = self.active_edge_arena[active_edge].split_mut(vertex) {
            if pending_edge.end < vertex {
                pending_edge.windings = -pending_edge.windings;
                mem::swap(&mut pending_edge.end, &mut vertex);
            }
            self.event_queue.push(Event {
                vertex,
                pending_edge: Some(pending_edge),
            })
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Operation {
    Difference,
    Intersection,
    Union,
}

impl Operation {
    fn apply(self, subject_is_interior: bool, clip_is_interior: bool) -> bool {
        match self {
            Operation::Difference => subject_is_interior && !clip_is_interior,
            Operation::Intersection => subject_is_interior && clip_is_interior,
            Operation::Union => subject_is_interior || clip_is_interior,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub subject_fill_rule: FillRule,
    pub clip_fill_rule: FillRule,
}

#[derive(Clone, Copy, Debug)]
pub enum FillRule {
    NonZero,
    EvenOdd,
}

impl FillRule {
    fn apply(self, winding: i32) -> bool {
        match self {
            FillRule::NonZero => winding != 0,
            FillRule::EvenOdd => winding % 2 != 0,
        }
    }
}

impl Default for FillRule {
    fn default() -> Self {
        FillRule::NonZero
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PendingEdge {
    windings: Windings,
    end: Point,
}

impl PendingEdge {
    fn to_edge(self, start: Point) -> LineSegment {
        LineSegment::new(start, self.end)
    }

    fn overlaps(self, other: Self, start: Point) -> bool {
        self.compare(other, start) == Ordering::Equal
    }

    fn compare(self, other: Self, start: Point) -> Ordering {
        if self.end <= other.end {
            other
                .to_edge(start)
                .compare_to_point(self.end)
                .unwrap()
                .reverse()
        } else {
            self.to_edge(start).compare_to_point(other.end).unwrap()
        }
    }

    fn splice_mut(&mut self, mut other: Self) -> Option<Event> {
        if other.end <= self.end {
            mem::swap(self, &mut other);
        }
        self.windings += other.windings;
        if self.end == other.end {
            return None;
        }
        Some(Event {
            vertex: self.end,
            pending_edge: Some(other),
        })
    }
}

pub struct Clip<'a, 'b> {
    clipper: &'a mut Clipper,
    operation: Operation,
    subject_fill_rule: FillRule,
    clip_fill_rule: FillRule,
    pending_edges: &'b mut Vec<PendingEdge>,
    left_edges: &'b mut Vec<usize>,
    right_edges: &'b mut Vec<usize>,
    left_boundary_edge_indices: &'b mut Vec<usize>,
    right_boundary_edge_indices: &'b mut Vec<usize>,
}

impl<'a, 'b> InternalIterator for Clip<'a, 'b> {
    type Item = Command;

    fn for_each(mut self, f: &mut impl FnMut(Self::Item) -> bool) -> bool {
        while let Some(vertex) = self.clipper.pop_events_for_vertex(&mut self.pending_edges) {
            if !self.clipper.handle_events_for_vertex(
                vertex,
                {
                    let operation = self.operation;
                    let subject_fill_rule = self.subject_fill_rule;
                    let clip_fill_rule = self.clip_fill_rule;
                    move |windings: Windings| {
                        operation.apply(
                            subject_fill_rule.apply(windings.subject_winding),
                            clip_fill_rule.apply(windings.clip_winding),
                        )
                    }
                },
                f,
                self.pending_edges,
                self.left_edges,
                self.right_edges,
                self.left_boundary_edge_indices,
                self.right_boundary_edge_indices,
            ) {
                return false;
            }
            self.pending_edges.clear();
            self.left_edges.clear();
            self.right_edges.clear();
            self.left_boundary_edge_indices.clear();
            self.right_boundary_edge_indices.clear();
        }
        true
    }
}

#[derive(Clone, Copy, Debug)]
struct ActiveEdge {
    is_boundary: bool,
    windings: Windings,
    edge: LineSegment,
    upper_region: Region,
    active_polygon: Option<usize>,
}

impl ActiveEdge {
    fn lower_active_polygon(&self) -> Option<usize> {
        if self.upper_region.is_interior {
            None
        } else {
            self.active_polygon
        }
    }

    fn upper_active_polygon(&self) -> Option<usize> {
        if self.upper_region.is_interior {
            self.active_polygon
        } else {
            None
        }
    }

    fn split_mut(&mut self, vertex: Point) -> Option<PendingEdge> {
        let end = self.edge.end();
        if vertex == end {
            return None;
        }
        self.edge = LineSegment::new(self.edge.start(), vertex);
        Some(PendingEdge {
            windings: self.windings,
            end,
        })
    }
}

#[derive(Clone, Debug)]
struct ActivePolygon {
    vertices: VecDeque<Point>,
    front_boundary_edge: usize,
    back_boundary_edge: usize,
}

#[derive(Clone, Copy, Debug)]
struct Region {
    is_interior: bool,
    windings: Windings,
}

#[derive(Clone, Copy, Debug)]
struct Event {
    vertex: Point,
    pending_edge: Option<PendingEdge>,
}

impl Eq for Event {}

impl Ord for Event {
    fn cmp(&self, other: &Self) -> Ordering {
        self.vertex.partial_cmp(&other.vertex).unwrap().reverse()
    }
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug)]
struct Windings {
    subject_winding: i32,
    clip_winding: i32,
}

impl Add for Windings {
    type Output = Self;

    fn add(mut self, other: Self) -> Self::Output {
        self += other;
        self
    }
}

impl AddAssign for Windings {
    fn add_assign(&mut self, other: Windings) {
        self.subject_winding += other.subject_winding;
        self.clip_winding += other.clip_winding;
    }
}

impl Neg for Windings {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self {
            subject_winding: -self.subject_winding,
            clip_winding: -self.clip_winding,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum PathKind {
    Subject,
    Clip,
}
