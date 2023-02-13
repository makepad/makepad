use super::monotone_tessellator::Side;
use super::MonotoneTessellator;
use bender_geometry::linear_path::Command;
use bender_geometry::mesh::Callbacks;
use bender_geometry::{LineSegment, Point};
use bender_internal_iter::{InternalIterator, IntoInternalIterator};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::mem;
use std::ops::{Add, AddAssign, Range};

#[derive(Clone, Debug)]
pub struct Tessellator {
    active_edges: Vec<ActiveEdge>,
    event_queue: BinaryHeap<Event>,
    monotone_tessellator: MonotoneTessellator,
}

impl Tessellator {
    pub fn new() -> Self {
        Self {
            active_edges: Vec::new(),
            event_queue: BinaryHeap::new(),
            monotone_tessellator: MonotoneTessellator::new(),
        }
    }

    pub fn tessellate(
        &mut self,
        commands: impl IntoInternalIterator<Item = Command>,
        callbacks: &mut impl Callbacks,
        pending_edges: &mut Vec<PendingEdge>,
        left_edges: &mut Vec<ActiveEdge>,
    ) {
        let mut initial_point = None;
        let mut current_point = None;
        commands.into_internal_iter().for_each(&mut |command| {
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
                    self.push_events_for_edge(current_point.unwrap(), initial_point.unwrap());
                    current_point = initial_point;
                }
                Command::LineTo(point) => {
                    self.push_events_for_edge(current_point.unwrap(), point);
                    current_point = Some(point);
                }
            }
            true
        });
        while let Some(vertex) = self.pop_events_for_vertex(pending_edges) {
            self.handle_events_for_vertex(vertex, callbacks, pending_edges, left_edges);
            pending_edges.clear();
            left_edges.clear();
        }
    }

    fn push_events_for_edge(&mut self, start: Point, end: Point) {
        let (start, end) = match start.partial_cmp(&end) {
            Some(Ordering::Less) => (start, end),
            Some(Ordering::Equal) => return,
            Some(Ordering::Greater) => (end, start),
            None => panic!(),
        };
        self.event_queue.push(Event {
            vertex: start,
            pending_edge: Some(PendingEdge {
                parity: Parity::Odd,
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
        callbacks: &mut impl Callbacks,
        pending_edges: &mut Vec<PendingEdge>,
        left_edges: &mut Vec<ActiveEdge>,
    ) {
        let mut incident_edges_range = self.find_incident_edges_range(vertex);
        self.fix_temporary_edges(vertex, &mut incident_edges_range);
        let incident_edges_start = incident_edges_range.start;
        self.remove_and_split_incident_edges(
            vertex,
            incident_edges_range,
            left_edges,
            pending_edges,
        );
        self.sort_and_splice_pending_edges(vertex, pending_edges);
        let vertex_index = callbacks.vertex(vertex.x(), vertex.y());
        let (lower_monotone_polygon, upper_monotone_polygon) = if left_edges.is_empty() {
            self.connect_left_vertex(incident_edges_start)
        } else {
            self.finish_left_monotone_polygons(vertex_index, left_edges, callbacks)
        };
        if let Some(lower_monotone_polygon) = lower_monotone_polygon {
            self.monotone_tessellator.push_vertex(
                lower_monotone_polygon,
                Side::Upper,
                vertex_index,
                vertex,
                callbacks,
            );
        }
        if let Some(upper_monotone_polygon) = upper_monotone_polygon {
            self.monotone_tessellator.push_vertex(
                upper_monotone_polygon,
                Side::Lower,
                vertex_index,
                vertex,
                callbacks,
            );
        }
        if pending_edges.is_empty() {
            self.connect_right_vertex(
                vertex_index,
                vertex,
                incident_edges_start,
                lower_monotone_polygon,
                upper_monotone_polygon,
            );
        } else {
            self.create_and_insert_right_edges(
                vertex_index,
                vertex,
                incident_edges_start,
                &pending_edges,
                lower_monotone_polygon,
                upper_monotone_polygon,
            );
        }
    }

    fn find_incident_edges_range(&self, vertex: Point) -> Range<usize> {
        Range {
            start: self
                .active_edges
                .iter()
                .position(|active_edge| {
                    active_edge.edge.compare_to_point(vertex).unwrap() != Ordering::Less
                })
                .unwrap_or(self.active_edges.len()),
            end: self
                .active_edges
                .iter()
                .rposition(|active_edge| {
                    active_edge.edge.compare_to_point(vertex).unwrap() != Ordering::Greater
                })
                .map_or(0, |index| index + 1),
        }
    }

    fn fix_temporary_edges(&mut self, vertex: Point, incident_edges_range: &mut Range<usize>) {
        while 0 < incident_edges_range.start
            && self.active_edges[incident_edges_range.start - 1].is_temporary
        {
            incident_edges_range.start -= 1;
            self.active_edges[incident_edges_range.start].split_mut(vertex);
        }
        while incident_edges_range.end < self.active_edges.len()
            && self.active_edges[incident_edges_range.end].is_temporary
        {
            self.active_edges[incident_edges_range.end].split_mut(vertex);
            incident_edges_range.end += 1;
        }
    }

    fn remove_and_split_incident_edges(
        &mut self,
        vertex: Point,
        incident_edges_range: Range<usize>,
        left_edges: &mut Vec<ActiveEdge>,
        pending_edges: &mut Vec<PendingEdge>,
    ) {
        left_edges.extend(
            self.active_edges
                .drain(incident_edges_range)
                .map(|mut incident_edge| {
                    if let Some(pending_edge) = incident_edge.split_mut(vertex) {
                        pending_edges.push(pending_edge);
                    }
                    incident_edge
                }),
        );
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

    fn connect_left_vertex(
        &mut self,
        incident_edges_start: usize,
    ) -> (Option<usize>, Option<usize>) {
        if !self
            .last_lower_region_parity(incident_edges_start)
            .is_interior()
        {
            return (None, None);
        }
        let active_edge_0 = self.active_edges[incident_edges_start - 1];
        let active_edge_1 = self.active_edges[incident_edges_start];
        if active_edge_0.edge.start() <= active_edge_1.edge.start() {
            let upper_monotone_polygon = self
                .monotone_tessellator
                .start_monotone_polygon(active_edge_1.start_index, active_edge_1.edge.start());
            (
                self.active_edges[incident_edges_start]
                    .lower_monotone_polygon
                    .replace(upper_monotone_polygon),
                Some(upper_monotone_polygon),
            )
        } else {
            let lower_monotone_polygon = self
                .monotone_tessellator
                .start_monotone_polygon(active_edge_0.start_index, active_edge_0.edge.start());
            (
                Some(lower_monotone_polygon),
                self.active_edges[incident_edges_start - 1]
                    .upper_monotone_polygon
                    .replace(lower_monotone_polygon),
            )
        }
    }

    fn finish_left_monotone_polygons(
        &mut self,
        vertex_index: u16,
        left_edges: &mut Vec<ActiveEdge>,
        callbacks: &mut impl Callbacks,
    ) -> (Option<usize>, Option<usize>) {
        for left_edge in left_edges[0..left_edges.len() - 1].iter().cloned() {
            if left_edge.upper_region_parity.is_interior() {
                self.monotone_tessellator.finish_monotone_polygon(
                    left_edge.upper_monotone_polygon.unwrap(),
                    vertex_index,
                    callbacks,
                );
            }
        }
        (
            left_edges.first().unwrap().lower_monotone_polygon,
            left_edges.last().unwrap().upper_monotone_polygon,
        )
    }

    fn connect_right_vertex(
        &mut self,
        vertex_index: u16,
        vertex: Point,
        incident_edges_start: usize,
        lower_monotone_polygon: Option<usize>,
        upper_monotone_polygon: Option<usize>,
    ) {
        let lower_region_parity = self.last_lower_region_parity(incident_edges_start);
        if !lower_region_parity.is_interior() {
            return;
        }
        self.active_edges.insert(
            incident_edges_start,
            ActiveEdge {
                is_temporary: true,
                parity: Parity::Even,
                start_index: vertex_index,
                edge: LineSegment::new(
                    vertex,
                    self.active_edges[incident_edges_start - 1]
                        .edge
                        .end()
                        .min(self.active_edges[incident_edges_start].edge.end()),
                ),
                upper_region_parity: lower_region_parity,
                lower_monotone_polygon,
                upper_monotone_polygon,
            },
        );
    }

    fn create_and_insert_right_edges(
        &mut self,
        vertex_index: u16,
        vertex: Point,
        incident_edges_start: usize,
        pending_edges: &[PendingEdge],
        mut lower_monotone_polygon: Option<usize>,
        upper_monotone_polygon: Option<usize>,
    ) {
        let mut lower_region_parity = self.last_lower_region_parity(incident_edges_start);
        self.active_edges.splice(
            incident_edges_start..incident_edges_start,
            pending_edges.iter().enumerate().map({
                let monotone_tessellator = &mut self.monotone_tessellator;
                move |(index, &pending_edge)| {
                    let upper_region_parity = lower_region_parity + pending_edge.parity;
                    let upper_monotone_polygon = if upper_region_parity.is_interior() {
                        if index == pending_edges.len() - 1 {
                            upper_monotone_polygon
                        } else {
                            Some(monotone_tessellator.start_monotone_polygon(vertex_index, vertex))
                        }
                    } else {
                        None
                    };
                    let active_edge = ActiveEdge {
                        is_temporary: false,
                        parity: pending_edge.parity,
                        start_index: vertex_index,
                        edge: pending_edge.to_edge(vertex),
                        upper_region_parity,
                        lower_monotone_polygon,
                        upper_monotone_polygon,
                    };
                    lower_region_parity = upper_region_parity;
                    lower_monotone_polygon = upper_monotone_polygon;
                    active_edge
                }
            }),
        );
    }

    fn last_lower_region_parity(&self, incident_edges_start: usize) -> Parity {
        if 0 == incident_edges_start {
            Parity::Even
        } else {
            self.active_edges[incident_edges_start - 1].upper_region_parity
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PendingEdge {
    parity: Parity,
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
        self.parity += other.parity;
        if self.end == other.end {
            return None;
        }
        Some(Event {
            vertex: self.end,
            pending_edge: Some(other),
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ActiveEdge {
    is_temporary: bool,
    parity: Parity,
    start_index: u16,
    edge: LineSegment,
    upper_region_parity: Parity,
    lower_monotone_polygon: Option<usize>,
    upper_monotone_polygon: Option<usize>,
}

impl ActiveEdge {
    fn split_mut(&mut self, vertex: Point) -> Option<PendingEdge> {
        let end = self.edge.end();
        if vertex == end {
            return None;
        }
        self.edge = LineSegment::new(self.edge.start(), vertex);
        Some(PendingEdge {
            parity: self.parity,
            end,
        })
    }
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

#[derive(Clone, Copy, Debug, PartialEq)]
enum Parity {
    Even,
    Odd,
}

impl Parity {
    fn is_interior(self) -> bool {
        self == Parity::Odd
    }
}

impl Add for Parity {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        match (self, other) {
            (Parity::Even, Parity::Even) => Parity::Even,
            (Parity::Even, Parity::Odd) => Parity::Odd,
            (Parity::Odd, Parity::Even) => Parity::Odd,
            (Parity::Odd, Parity::Odd) => Parity::Even,
        }
    }
}

impl AddAssign for Parity {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}
