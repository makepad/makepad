use crate::{offset, Layout, Path, Pattern, Placement, Point, Segment};
use std::cmp::Ordering;

const GAP: f64 = 15.0;

#[derive(Clone, Copy)]
struct Item {
    part: usize,
    min: Point,
    width: f64,
    height: f64,
    on_fold: bool,
}

pub(crate) fn nest(pattern: &Pattern, fabric_width_mm: f64) -> Layout {
    let width = if fabric_width_mm.is_finite() { fabric_width_mm.max(0.0) } else { 0.0 };
    let mut items: Vec<Item> = pattern
        .parts
        .iter()
        .enumerate()
        .map(|(part, piece)| {
            let (seam_min, _) = piece.outline.bounds();
            let (mut min, max) = offset(&piece.outline, piece.seam_allowance_mm).bounds();
            if piece.on_fold {
                min.x = seam_min.x;
            }
            Item {
                part,
                min,
                width: max.x - min.x,
                height: max.y - min.y,
                on_fold: piece.on_fold,
            }
        })
        .collect();
    items.sort_by(|a, b| {
        b.on_fold
            .cmp(&a.on_fold)
            .then_with(|| b.height.partial_cmp(&a.height).unwrap_or(Ordering::Equal))
            .then_with(|| a.part.cmp(&b.part))
    });

    let mut placements = Vec::with_capacity(items.len());
    let mut shelf_y = 0.0;
    let mut cursor_x = 0.0;
    let mut shelf_height: f64 = 0.0;
    let mut in_regular_shelves = false;
    for item in items {
        if item.on_fold {
            if in_regular_shelves {
                shelf_y += shelf_height + GAP;
                cursor_x = 0.0;
                shelf_height = 0.0;
            }
            placements.push(Placement {
                part: item.part,
                offset: Point::new(-item.min.x, shelf_y - item.min.y),
                rotation_deg: 0.0,
            });
            shelf_y += item.height + GAP;
            in_regular_shelves = false;
            continue;
        }

        in_regular_shelves = true;
        if cursor_x > 0.0 && cursor_x + item.width > width {
            shelf_y += shelf_height + GAP;
            cursor_x = 0.0;
            shelf_height = 0.0;
        }
        placements.push(Placement {
            part: item.part,
            offset: Point::new(cursor_x - item.min.x, shelf_y - item.min.y),
            rotation_deg: 0.0,
        });
        cursor_x += item.width + GAP;
        shelf_height = shelf_height.max(item.height);
        if item.width > width {
            shelf_y += shelf_height + GAP;
            cursor_x = 0.0;
            shelf_height = 0.0;
        }
    }
    let height = if placements.is_empty() {
        0.0
    } else if in_regular_shelves {
        shelf_y + shelf_height
    } else {
        (shelf_y - GAP).max(0.0)
    };
    placements.sort_by_key(|placement| placement.part);
    Layout { width_mm: width, height_mm: height, placements }
}

pub(crate) fn transform_point(point: Point, placement: &Placement) -> Point {
    if (placement.rotation_deg.rem_euclid(360.0) - 180.0).abs() < 0.01 {
        Point::new(placement.offset.x - point.x, placement.offset.y - point.y)
    } else {
        Point::new(placement.offset.x + point.x, placement.offset.y + point.y)
    }
}

pub(crate) fn transform_path(path: &Path, placement: &Placement) -> Path {
    let map = |point| transform_point(point, placement);
    Path {
        start: map(path.start),
        segments: path
            .segments
            .iter()
            .map(|segment| match *segment {
                Segment::Line { to } => Segment::Line { to: map(to) },
                Segment::Curve { c1, c2, to } => Segment::Curve { c1: map(c1), c2: map(c2), to: map(to) },
            })
            .collect(),
        closed: path.closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{designs, Measurements, Options};

    #[test]
    fn placements_do_not_overlap() {
        for design in designs() {
            let pattern = design.draft(&Measurements::sample(), &Options::default()).unwrap();
            let layout = nest(&pattern, 900.0);
            let boxes: Vec<(Point, Point)> = layout
                .placements
                .iter()
                .map(|placement| {
                    let (min, max) = pattern.parts[placement.part].outline.bounds();
                    (transform_point(min, placement), transform_point(max, placement))
                })
                .collect();
            for i in 0..boxes.len() {
                for j in (i + 1)..boxes.len() {
                    let separated = boxes[i].1.x <= boxes[j].0.x
                        || boxes[j].1.x <= boxes[i].0.x
                        || boxes[i].1.y <= boxes[j].0.y
                        || boxes[j].1.y <= boxes[i].0.y;
                    assert!(separated, "{} parts {} and {} overlap", design.id(), i, j);
                }
                let piece_width = boxes[i].1.x - boxes[i].0.x;
                if piece_width <= layout.width_mm {
                    assert!(boxes[i].0.x >= -1.0e-6 && boxes[i].1.x <= layout.width_mm + 1.0e-6);
                }
            }
        }
    }
}
