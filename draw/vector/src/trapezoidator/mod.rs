use crate::geometry::{LineSegment, Point, Trapezoid};
use crate::internal_iter::InternalIterator;
use crate::path::{LinePathCommand, LinePathIterator};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::mem;
use std::ops::Range;

/// Converts a sequence of line path commands to a sequence of trapezoids. The line path commands
/// should define a set of closed contours.
#[derive(Clone, Debug, Default)]
pub struct Trapezoidator {
    event_queue: BinaryHeap<Event>,
    active_segments: Vec<ActiveSegment>,
}

impl Trapezoidator {
    /// Creates a new trapezoidator.
    pub fn new() -> Trapezoidator {
        Trapezoidator::default()
    }

    /// Returns an iterator over trapezoids corresponding to the given iterator over line path
    /// commands.
    pub fn trapezoidate<P: LinePathIterator>(&mut self, path: P) -> Option<Trapezoidate> {
        let mut initial_point = None;
        let mut current_point = None;
        if !path.for_each(&mut |command| {
            match command {
                LinePathCommand::MoveTo(p) => {
                    initial_point = Some(p);
                    current_point = Some(p);
                }
                LinePathCommand::LineTo(p) => {
                    let p0 = current_point.replace(p).unwrap();
                    if !self.push_events_for_segment(LineSegment::new(p0, p)) {
                        return false;
                    }
                }
                LinePathCommand::Close => {
                    let p = initial_point.take().unwrap();
                    let p0 = current_point.replace(p).unwrap();
                    if !self.push_events_for_segment(LineSegment::new(p0, p)) {
                        return false;
                    }
                }
            }
            true
        }){
            return None
        };
        Some(Trapezoidate {
            trapezoidator: self,
        })
    }

    /// Adds events for the given segment to the event queue.
    fn push_events_for_segment(&mut self, segment: LineSegment) -> bool {
        // Determine the winding, the leftmost point, and the rightmost point of the segment.
        //
        // The winding is used to determine which regions are considered inside, and which are
        // considered outside. A region is considered inside if its winding is non-zero.
        // Conceptually, the winding of a region is determined by casting an imaginary ray from
        // any point inside the region to infinity in any direction, and adding the windings of
        // all segments that are intersected by the ray. The winding of a segment is +1 if it
        // intersects the ray from left to right, and -1 if it intersects the ray from right to
        // left.
        let (winding, p0, p1) = match segment.p0.partial_cmp(&segment.p1) {
            None => {
                // The endpoints of the segment cannot be compared, so the segment is invalid.
                // This can happen if the semgent has NaN coordinates. In this case, the input
                // as a whole is invalid, so we bail out early.
                return false;
            },
            Some(Ordering::Less) => (1, segment.p0, segment.p1),
            Some(Ordering::Equal) => {
                // The endpoints of the segment are equal, so the segment is empty. Empty segments
                // do not affect the output of the trapezoidation algorithm, so they can safely be
                // ignored.
                return true;
            }
            Some(Ordering::Greater) => (-1, segment.p1, segment.p0),
        };
        // Add an event to the event queue for the leftmost point of the segment. This is where
        // the segment starts intersecting the sweepline.
        self.event_queue.push(Event {
            point: p0,
            pending_segment: Some(PendingSegment { winding, p1 }),
        });
        // Add an event to the event queue for the rightmost point of the segment. This is where
        // the segment stops intersecting the sweepline.
        self.event_queue.push(Event {
            point: p1,
            pending_segment: None,
        });
        true
    }

    /// Removes all events at the next point where an event occurs from the event queue.
    /// 
    /// Returns the point at which the events occur, or `None` if the event queue is empty.
    /// Appends the pending segments that start intersecting the sweepline at this point to
    /// `pending_segments`.
    fn pop_events_for_next_point(
        &mut self,
        pending_segments: &mut Vec<PendingSegment>,
    ) -> Option<Point> {
        // Pop an event from the event queue. This will be the first event at the next point.
        self.event_queue.pop().map(|event| {
            // If there is a segment that starts intersecting the sweepline at this point, add it
            // to `pending_segments`.
            if let Some(pending_segment) = event.pending_segment {
                pending_segments.push(pending_segment)
            }
            // Keep popping events while they occur at the same point as the first one.
            while let Some(&next_event) = self.event_queue.peek() {
                if next_event != event {
                    break;
                }
                self.event_queue.pop();
                // If there is a segment that starts intersecting the sweepline at this point, add
                // it to `pending_segments`.
                if let Some(pending_segment) = next_event.pending_segment {
                    pending_segments.push(pending_segment);
                }
            }
            event.point
        })
    }

    /// Handle all events that occur at the given point. `right_segments` is a list of segments that
    /// start intersecting the sweepline at this point. `trapezoid_segments` is scratch space for a
    /// list of segments for which we potentially have to generate trapezoids.
    fn handle_events_for_point<F>(
        &mut self,
        point: Point,
        right_segments: &mut Vec<PendingSegment>,
        trapezoid_segments: &mut Vec<ActiveSegment>,
        f: &mut F,
    ) -> bool
    where
        F: FnMut(Trapezoid) -> bool,
    {
        // Find the range of active segments that are incident with the given point.
        let mut incident_segment_range = self.find_incident_segment_range(point);
        // If there is an active segment that lies below the current point, and the region below it
        // is considered outside, then this segment is the lower boundary of a trapezoid. We split
        // the segment where it intersects the sweepline, adding the part on the left to the list of
        // trapezoid segments, while keeping the part on the right in the list of active segments.
        if let Some(trapezoid_segment) =
            self.find_trapezoid_segment_below(point, incident_segment_range.start)
        {
            trapezoid_segments.push(trapezoid_segment);
        }
        // If there are any active segments that are incident with the given point, we remove them
        // from the list of active segments, and then split each segment where it intersects the
        // sweepline, adding the part on the left to the list of trapezoid segments, while adding
        // the part on the right to the list of right segments.
        self.remove_incident_segments(
            point,
            &mut incident_segment_range,
            right_segments,
            trapezoid_segments,
        );
        // Sort the right segments by their slope.
        self.sort_right_segments(point, right_segments);
        // Insert the right segments into the list of active segments, updating the range of
        // active segments that are incident with the given point accordingly.
        self.insert_right_segments(point, &mut incident_segment_range, right_segments);
        // If there is an active segment that lies above the current point, and the region below it
        // is considered inside, then this segment is the upper boundary of a trapezoid. We split the
        // the segment where it intersects the sweepline, adding the part on the left to the list of
        // trapezoid segments, while generating an event for the part on the right.
        if let Some(trapezoid_segment) =
            self.find_trapezoid_segment_above(point, incident_segment_range.end)
        {
            trapezoid_segments.push(trapezoid_segment);
        }
        // At this point, `trapezoid_segments` contains a list of segments that stop intersecting the
        // sweepline at the current point, and that potentially form trapezoid boundaries. We generate
        // trapezoids for these segments, and pass them to the given closure.
        self.generate_trapezoids(trapezoid_segments, f)
    }

    /// Finds the range of active segments that are incident with the given point.
    fn find_incident_segment_range(&self, point: Point) -> Range<usize> {
        Range {
            // Find the index of the first active segment that does not lie below the given point.
            start: self
                .active_segments
                .iter()
                .position(|active_segment| {
                    active_segment.segment.compare_to_point(point).unwrap() != Ordering::Less
                })
                .unwrap_or(self.active_segments.len()),
            // Find the index of the first active segment that lies above the given point.
            end: self
                .active_segments
                .iter()
                .rposition(|active_segment| {
                    active_segment.segment.compare_to_point(point).unwrap() != Ordering::Greater
                })
                .map_or(0, |index| index + 1),
        }
    }

    // Finds the first active segment that lies below the given point. If such a segment exists,
    // and the region below it is considered outside, then this segment is the lower boundary of a
    // trapezoid. We split the segment where it intersects the sweepline, keeping the part on the
    // right in the list of active segments, and returning the part on the left.
    fn find_trapezoid_segment_below(
        &mut self,
        point: Point,
        incident_segment_start: usize,
    ) -> Option<ActiveSegment> {
        if incident_segment_start == 0
            || !self.active_segments[incident_segment_start - 1].region_above.is_inside {
            return None;
        }
        let intersection = self.active_segments[incident_segment_start - 1]
            .segment
            .intersect_with_vertical_line(point.x)
            .unwrap_or(point);
        self.active_segments[incident_segment_start - 1].split_left_mut(intersection)
    }

    // Removes all active segments that are incident with the given point from the list of active
    // segments, and then splits each segment where it intersects the sweepline, adding the part
    // on the left to the list of trapezoid segments, while adding the part on the right to the
    // list of right segments.
    fn remove_incident_segments(
        &mut self,
        point: Point,
        incident_segment_range: &mut Range<usize>,
        right_segments: &mut Vec<PendingSegment>,
        trapezoid_segments: &mut Vec<ActiveSegment>,
    ) {
        trapezoid_segments.extend(
            Iterator::map(
                self.active_segments.drain(incident_segment_range.clone()),
                |mut active_segment| {
                    if let Some(pending_segment) = active_segment.split_right_mut(point) {
                        right_segments.push(pending_segment);
                    }
                    active_segment
                },
            )
            .filter(|active_segment| active_segment.segment.p0.x != active_segment.segment.p1.x),
        );
        incident_segment_range.end = incident_segment_range.start;
    }

    /// Sorts the given list of right segments by their slope, using the given point as the leftmost
    /// endpoint.
    fn sort_right_segments(&mut self, point: Point, right_segments: &mut Vec<PendingSegment>) {
        right_segments.sort_by(|&right_segment_0, &right_segment_1| {
            right_segment_0.compare(right_segment_1, point).unwrap()
        });
        let mut index_0 = 0;
        for index_1 in 1..right_segments.len() {
            let right_segment_1 = right_segments[index_1];
            let right_segment_0 = &mut right_segments[index_0];
            if right_segment_0.overlaps(right_segment_1, point) {
                if let Some(event) = right_segment_0.splice_mut(right_segment_1) {
                    self.event_queue.push(event);
                }
            } else {
                index_0 += 1;
                right_segments[index_0] = right_segment_1;
            }
        }
        right_segments.truncate(index_0 + 1);
    }

    // Inserts the given right segments into the list of active segments, updating the range of
    // active segments that are incident with the given point accordingly.
    fn insert_right_segments(
        &mut self,
        point: Point,
        incident_segment_range: &mut Range<usize>,
        right_segments: &[PendingSegment],
    ) {
        let mut lower_region = if incident_segment_range.end == 0 {
            Region {
                is_inside: false,
                winding: 0,
            }
        } else {
            self.active_segments[incident_segment_range.end - 1].region_above
        };
        self.active_segments.splice(
            incident_segment_range.end..incident_segment_range.end,
            Iterator::map(right_segments.iter(), |right_segment| {
                let upper_region = {
                    let winding = lower_region.winding + right_segment.winding;
                    Region {
                        is_inside: winding != 0,
                        winding,
                    }
                };
                let right_segment = ActiveSegment {
                    winding: right_segment.winding,
                    segment: LineSegment::new(point, right_segment.p1),
                    region_above: upper_region,
                };
                lower_region = upper_region;
                right_segment
            }),
        );
        incident_segment_range.end += right_segments.len();
    }

    // Finds the first active segment that lies above the given point. If such a segment exists,
    // and the region below it is considered inside, then this segment is the upper boundary of a
    // trapezoid. We split the segment where it intersects the sweepline, generating an event for
    // the part on the right, and returning the part on the left.
    fn find_trapezoid_segment_above(
        &mut self,
        point: Point,
        incident_segment_end: usize,
    ) -> Option<ActiveSegment> {
        if incident_segment_end == self.active_segments.len()
            || incident_segment_end == 0 
            || !self.active_segments[incident_segment_end - 1].region_above.is_inside
        {
            return None;
        }
        let intersection = self.active_segments[incident_segment_end]
            .segment
            .intersect_with_vertical_line(point.x)
            .unwrap();
        if let Some(pending_segment) =
            self.active_segments[incident_segment_end].split_right_mut(intersection)
        {
            self.event_queue.push(Event {
                point: intersection,
                pending_segment: Some(pending_segment),
            });
        }
        Some(self.active_segments[incident_segment_end])
    }

    fn generate_trapezoids<F>(&self, trapezoid_segments: &[ActiveSegment], f: &mut F) -> bool
    where
        F: FnMut(Trapezoid) -> bool,
    {
        for trapezoid_segment_pair in trapezoid_segments.windows(2) {
            if !trapezoid_segment_pair[0].region_above.is_inside {
                continue;
            }
            let lower_segment = trapezoid_segment_pair[0].segment;
            let upper_segment = trapezoid_segment_pair[1].segment;
            if !f(Trapezoid {
                xs: [lower_segment.p0.x as f32, lower_segment.p1.x as f32],
                ys: [
                    lower_segment.p0.y as f32,
                    lower_segment.p1.y as f32,
                    upper_segment.p0.y as f32,
                    upper_segment.p1.y as f32,
                ],
            }) {
                return false;
            }
        }
        true
    }
}

/// An iterator over trapezoids corresponding to the given iterator over line path commands.
#[derive(Debug)]
pub struct Trapezoidate<'a> {
    trapezoidator: &'a mut Trapezoidator,
}

impl<'a> InternalIterator for Trapezoidate<'a> {
    type Item = Trapezoid;

    fn for_each<F>(self, f: &mut F) -> bool
    where
        F: FnMut(Trapezoid) -> bool,
    {
        let mut right_segments = Vec::new();
        let mut trapezoid_segments = Vec::new();
        while let Some(point) = self.trapezoidator.pop_events_for_next_point(&mut right_segments) {
            let ok = self.trapezoidator.handle_events_for_point(
                point,
                &mut right_segments,
                &mut trapezoid_segments,
                f,
            );
            right_segments.clear();
            trapezoid_segments.clear();
            if !ok {
                return false;
            }
        }
        true
    }
}

// An event in the event queue.
#[derive(Clone, Copy, Debug)]
struct Event {
    // The point at which the event occurs.
    point: Point,
    // The pending segment that starts intersecting the sweepline at this point, if any.
    pending_segment: Option<PendingSegment>,
}

impl Eq for Event {}

impl Ord for Event {
    fn cmp(&self, other: &Event) -> Ordering {
        self.point.partial_cmp(&other.point).unwrap().reverse()
    }
}

impl PartialEq for Event {
    fn eq(&self, other: &Event) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Event) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A segment that is pending insertion into the list of active segments.
///
/// We only store the rightmost endpoint for these segments, since the leftmost endpoint is
/// determined implicitly by the point at which the event that inserts the segment occurs.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingSegment {
    // The winding of the segment.
    winding: i32,
    // The rightmost endpoint of the segment.
    p1: Point,
}

impl PendingSegment {
    fn to_segment(self, p0: Point) -> LineSegment {
        LineSegment::new(p0, self.p1)
    }

    fn overlaps(self, other: PendingSegment, p0: Point) -> bool {
        self.compare(other, p0) == Some(Ordering::Equal)
    }

    fn compare(self, other: PendingSegment, p0: Point) -> Option<Ordering> {
        if self.p1 <= other.p1 {
            other
                .to_segment(p0)
                .compare_to_point(self.p1)
                .map(|ordering| ordering.reverse())
        } else {
            self.to_segment(p0).compare_to_point(other.p1)
        }
    }

    fn splice_mut(&mut self, mut other: Self) -> Option<Event> {
        if other.p1 < self.p1 {
            mem::swap(self, &mut other);
        }
        self.winding += other.winding;
        if self.p1 == other.p1 {
            return None;
        }
        Some(Event {
            point: self.p1,
            pending_segment: Some(other),
        })
    }
}

/// A segment that currently intersects the sweepline,
#[derive(Clone, Copy, Debug, PartialEq)]
struct ActiveSegment {
    winding: i32,
    segment: LineSegment,
    region_above: Region,
}

impl ActiveSegment {
    // Splits this segment at the given point, returning the part on the left.
    fn split_left_mut(&mut self, p: Point) -> Option<ActiveSegment> {
        let p0 = self.segment.p0;
        if p == p0 {
            return None;
        }
        self.segment.p0 = p;
        Some(ActiveSegment {
            winding: self.winding,
            segment: LineSegment::new(p0, p),
            region_above: self.region_above,
        })
    }

    // Splits this segment at the given point, returning the part on the right.
    fn split_right_mut(&mut self, p: Point) -> Option<PendingSegment> {
        let p1 = self.segment.p1;
        if p == p1 {
            return None;
        }
        self.segment.p1 = p;
        Some(PendingSegment {
            winding: self.winding,
            p1,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Region {
    is_inside: bool,
    winding: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path::{Path, PathIterator};

    #[test]
    fn test_square() {
        let mut path = Path::new();
        path.move_to(Point::new(0.0, 0.0));
        path.line_to(Point::new(1.0, 0.0));
        path.line_to(Point::new(1.0, 1.0));
        path.line_to(Point::new(0.0, 1.0));
        path.close();
        let mut trapezoidator = Trapezoidator::new();
        let trapezoids: Vec<_> = trapezoidator
            .trapezoidate(path.commands().linearize(0.1))
            .unwrap()
            .collect();
        assert_eq!(trapezoids, [
            Trapezoid { xs: [0.0, 1.0], ys: [0.0, 0.0, 1.0, 1.0] }
        ]);
    }
}