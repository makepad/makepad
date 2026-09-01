//! A page drawn small must not gain ink.
//!
//! Engraved notation is mostly hairlines: a staff line is 0.13 staff spaces, a
//! stem 0.12, a beam 0.50. Zoomed out far enough every one of them falls under
//! a physical pixel, and a raster target cannot draw a mark thinner than that.
//! Widening each stroke to the pixel floor at full strength multiplies the
//! page's ink by whatever the shortfall was — four or five times over, once
//! five staff lines, a stem per note and two beams per pair are all rounded up
//! together — which is exactly how a readable score turns into a black mass.
//!
//! This measures it. The score's primitives are projected exactly as
//! [`MakepadScoreRenderer`](makepad_score_render::MakepadScoreRenderer) submits
//! them, then rasterised on the device pixel grid with Makepad's own vector
//! coverage model, and the dark fraction of a crop *fixed in page coordinates*
//! is compared across an eight-to-one range of scales. The same music at half
//! the size must read with the same weight, not double it.
//!
//! The coverage model matches `DrawVector`: a filled path paints
//! `clamp(signed_distance_inside + aa/2)` and a stroke
//! `clamp((width + aa)/2 - distance)`, both in device pixels, where `aa` is the
//! baked antialiasing fringe. `DrawVector` bakes that fringe in path-local
//! (logical) units, so the score asks for `1 / device_scale` to land it on one
//! physical pixel. Noteheads go through `DrawGlyph`, whose coverage is analytic
//! and needs no floor; they are modelled as exact area coverage.

use makepad_score_render::*;

/// A retina display: the case where a logical-unit floor costs the most.
const DEVICE_SCALE: f64 = 2.0;
/// Logical pixels per staff space at 100% zoom, for a page fitted to a laptop
/// window (238 sp tall in ~816 logical points).
const FIT_PX_PER_SP: f64 = 3.43;
const MARGIN_LEFT: f64 = 17.0;
const MARGIN_RIGHT: f64 = 154.0;
const STAFF_SPAN: f64 = 18.0;

// ---------------------------------------------------------------- rasteriser

#[derive(Clone, Debug)]
enum Shape {
    Fill {
        points: Vec<[f64; 2]>,
        fringe: f64,
        alpha: f64,
    },
    Stroke {
        from: [f64; 2],
        to: [f64; 2],
        half_width: f64,
        alpha: f64,
    },
}

impl Shape {
    fn alpha_at(&self, p: [f64; 2]) -> f64 {
        match self {
            Self::Fill {
                points,
                fringe,
                alpha,
            } => (convex_signed_distance(points, p) + fringe * 0.5).clamp(0.0, 1.0) * alpha,
            Self::Stroke {
                from,
                to,
                half_width,
                alpha,
            } => (half_width - segment_distance(*from, *to, p)).clamp(0.0, 1.0) * alpha,
        }
    }

    fn bounds(&self) -> [f64; 4] {
        match self {
            Self::Fill { points, fringe, .. } => {
                let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
                for point in points {
                    bounds[0] = bounds[0].min(point[0]);
                    bounds[1] = bounds[1].min(point[1]);
                    bounds[2] = bounds[2].max(point[0]);
                    bounds[3] = bounds[3].max(point[1]);
                }
                [
                    bounds[0] - fringe,
                    bounds[1] - fringe,
                    bounds[2] + fringe,
                    bounds[3] + fringe,
                ]
            }
            Self::Stroke {
                from,
                to,
                half_width,
                ..
            } => [
                from[0].min(to[0]) - half_width,
                from[1].min(to[1]) - half_width,
                from[0].max(to[0]) + half_width,
                from[1].max(to[1]) + half_width,
            ],
        }
    }
}

/// Signed distance into a convex polygon, positive inside, in the polygon's
/// own units. Winding is derived from the signed area, so page-order
/// (clockwise, y down) and mathematical order both work.
fn convex_signed_distance(points: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let count = points.len();
    let mut twice_area = 0.0;
    for index in 0..count {
        let (a, b) = (points[index], points[(index + 1) % count]);
        twice_area += a[0] * b[1] - b[0] * a[1];
    }
    let winding = if twice_area >= 0.0 { 1.0 } else { -1.0 };
    let mut distance = f64::INFINITY;
    for index in 0..count {
        let (a, b) = (points[index], points[(index + 1) % count]);
        let edge = [b[0] - a[0], b[1] - a[1]];
        let length = (edge[0] * edge[0] + edge[1] * edge[1]).sqrt();
        if length <= 1e-12 {
            continue;
        }
        let cross = (edge[0] * (p[1] - a[1]) - edge[1] * (p[0] - a[0])) / length;
        distance = distance.min(winding * cross);
    }
    distance
}

fn segment_distance(from: [f64; 2], to: [f64; 2], p: [f64; 2]) -> f64 {
    let edge = [to[0] - from[0], to[1] - from[1]];
    let length_squared = edge[0] * edge[0] + edge[1] * edge[1];
    let t = if length_squared <= 1e-12 {
        0.0
    } else {
        (((p[0] - from[0]) * edge[0] + (p[1] - from[1]) * edge[1]) / length_squared).clamp(0.0, 1.0)
    };
    let nearest = [from[0] + edge[0] * t, from[1] + edge[1] * t];
    ((p[0] - nearest[0]).powi(2) + (p[1] - nearest[1]).powi(2)).sqrt()
}

/// Mean composited ink over a crop given in device pixels. One sample per
/// device pixel, which is what a fragment shader evaluates.
fn ink_fraction(shapes: &[Shape], crop_px: [f64; 4]) -> f64 {
    let bounded: Vec<_> = shapes.iter().map(|shape| (shape, shape.bounds())).collect();
    let mut ink = 0.0;
    let mut pixels = 0u64;
    for y in crop_px[1].floor() as i64..crop_px[3].ceil() as i64 {
        for x in crop_px[0].floor() as i64..crop_px[2].ceil() as i64 {
            let p = [x as f64 + 0.5, y as f64 + 0.5];
            let mut transmitted = 1.0f64;
            for (shape, bounds) in &bounded {
                if p[0] < bounds[0] || p[0] > bounds[2] || p[1] < bounds[1] || p[1] > bounds[3] {
                    continue;
                }
                let alpha = shape.alpha_at(p);
                if alpha > 0.0 {
                    transmitted *= 1.0 - alpha;
                }
            }
            ink += 1.0 - transmitted;
            pixels += 1;
        }
    }
    ink / pixels as f64
}

// ------------------------------------------------------------- page contents

/// Two staves of beamed sixteenths: the densest ink a page normally carries,
/// and the passage the complaint was about.
struct Passage {
    staff_groups: Vec<Vec<Rect>>,
    rules: Vec<Rect>,
    beams: Vec<Beam>,
    brackets: Vec<Primitive>,
    /// Notehead centre and radii, in staff spaces.
    heads: Vec<[f64; 4]>,
}

fn dense_passage() -> Passage {
    let engraving = EngravingDefaults::default();
    let mut passage = Passage {
        staff_groups: Vec::new(),
        rules: Vec::new(),
        beams: Vec::new(),
        brackets: Vec::new(),
        heads: Vec::new(),
    };
    for staff in 0..2 {
        let top = 20.0 + staff as f64 * STAFF_SPAN;
        passage.staff_groups.push(
            (0..5)
                .map(|line| {
                    Rect::from_xywh(
                        MARGIN_LEFT,
                        top + line as f64 - engraving.staff_line_thickness * 0.5,
                        MARGIN_RIGHT - MARGIN_LEFT,
                        engraving.staff_line_thickness,
                    )
                })
                .collect(),
        );
        for bar in 0..5 {
            let x = MARGIN_LEFT + bar as f64 * 34.0;
            if x > MARGIN_RIGHT {
                break;
            }
            passage.rules.push(Rect::from_xywh(
                x,
                top,
                engraving.thin_barline_thickness,
                4.0,
            ));
        }
        let mut x = MARGIN_LEFT + 5.0;
        while x < MARGIN_RIGHT - 6.0 {
            let stems: Vec<f64> = (0..4).map(|note| x + note as f64 * 2.4).collect();
            let beam_y = top - 1.6;
            for (note, stem) in stems.iter().enumerate() {
                let head_y = top + 3.0 - (note as f64 % 3.0) * 0.5;
                passage.heads.push([*stem, head_y, 0.62, 0.44]);
                passage.rules.push(Rect::from_xywh(
                    stem + 0.58 - engraving.stem_thickness * 0.5,
                    beam_y,
                    engraving.stem_thickness,
                    head_y - beam_y,
                ));
            }
            for level in 0..2 {
                let dy = level as f64 * (engraving.beam_thickness + engraving.beam_spacing)
                    + engraving.beam_thickness * 0.5;
                passage.beams.push(Beam {
                    start: Point::new(stems[0] + 0.52, beam_y + dy),
                    end: Point::new(stems[3] + 0.64, beam_y + dy + 0.35),
                    thickness: engraving.beam_thickness,
                });
            }
            x += 4.0 * 2.4 + 1.4;
        }
    }
    passage.brackets.push(Primitive::Bracket {
        x: MARGIN_LEFT - 1.3,
        top: 20.0,
        bottom: 20.0 + STAFF_SPAN + 4.0,
        thickness: EngravingDefaults::default().bracket_thickness * 0.5,
        hook: 1.0,
    });
    passage
}

/// Exactly what `MakepadScoreRenderer::draw` submits: device-grid snapping for
/// rules, the hairline floor with its ink alpha, and a one-physical-pixel AA
/// fringe.
fn submitted(passage: &Passage, transform: Transform, device_scale: f64) -> Vec<Shape> {
    let fringe = MIN_INK_DEVICE_PX;
    let mut shapes = Vec::new();
    let to_device = |point: Point| [point.x * device_scale, point.y * device_scale];
    let rect_points = |rect: Rect| {
        vec![
            [rect.min.x * device_scale, rect.min.y * device_scale],
            [rect.max.x * device_scale, rect.min.y * device_scale],
            [rect.max.x * device_scale, rect.max.y * device_scale],
            [rect.min.x * device_scale, rect.max.y * device_scale],
        ]
    };

    for group in &passage.staff_groups {
        for rule in project_staff_rules_on_grid(group, transform, 1.0, device_scale) {
            shapes.push(Shape::Fill {
                points: rect_points(rule.rect_px),
                fringe,
                alpha: rule.ink_alpha as f64,
            });
        }
    }
    for rect in &passage.rules {
        let rule = project_rule_on_grid(*rect, transform, 1.0, device_scale);
        shapes.push(Shape::Fill {
            points: rect_points(rule.rect_px),
            fringe,
            alpha: rule.ink_alpha as f64,
        });
    }
    for beam in &passage.beams {
        let ink = ink_floor(beam.thickness * transform.scale, device_scale);
        let start = transform.point(beam.start);
        let end = transform.point(beam.end);
        let half = ink.width * 0.5;
        shapes.push(Shape::Fill {
            points: vec![
                to_device(Point::new(start.x, start.y - half)),
                to_device(Point::new(end.x, end.y - half)),
                to_device(Point::new(end.x, end.y + half)),
                to_device(Point::new(start.x, start.y + half)),
            ],
            fringe,
            alpha: ink.alpha as f64,
        });
    }
    for bracket in &passage.brackets {
        let Primitive::Bracket {
            x,
            top,
            bottom,
            thickness,
            hook,
        } = bracket
        else {
            continue;
        };
        let ink = ink_floor(thickness * transform.scale, device_scale);
        // A stroke's painted half-extent is (width + fringe) / 2.
        let half_width = (ink.width * device_scale + MIN_INK_DEVICE_PX) * 0.5;
        let corners = [
            Point::new(x + hook, *top),
            Point::new(*x, *top),
            Point::new(*x, *bottom),
            Point::new(x + hook, *bottom),
        ]
        .map(|point| to_device(transform.point(point)));
        for pair in corners.windows(2) {
            shapes.push(Shape::Stroke {
                from: pair[0],
                to: pair[1],
                half_width,
                alpha: ink.alpha as f64,
            });
        }
    }
    shapes.extend(notehead_shapes(passage, transform, device_scale));
    shapes
}

/// `DrawGlyph` resolves an outline analytically at any size, so a notehead
/// needs no floor and keeps exact area coverage at every scale.
fn notehead_shapes(passage: &Passage, transform: Transform, device_scale: f64) -> Vec<Shape> {
    passage
        .heads
        .iter()
        .map(|head| {
            let centre = transform.point(Point::new(head[0], head[1]));
            let rx = head[2] * transform.scale * device_scale;
            let ry = head[3] * transform.scale * device_scale;
            Shape::Fill {
                points: (0..24)
                    .map(|step| {
                        let angle = step as f64 / 24.0 * std::f64::consts::TAU;
                        [
                            centre.x * device_scale + rx * angle.cos(),
                            centre.y * device_scale + ry * angle.sin(),
                        ]
                    })
                    .collect(),
                fringe: MIN_INK_DEVICE_PX,
                alpha: 1.0,
            }
        })
        .collect()
}

// ------------------------------------------------------------------ the test

#[test]
fn a_page_drawn_small_keeps_its_engraved_weight() {
    let passage = dense_passage();
    // Fixed in page coordinates: the upper staff and its beamed sixteenths, so
    // every scale measures the same music.
    let crop_sp = Rect::from_xywh(MARGIN_LEFT, 16.0, 60.0, 20.0);

    let mut measured = Vec::new();
    println!("\nzoom   device px/sp   ink");
    for zoom in [1.0, 0.5, 0.25, 0.12] {
        let transform = Transform {
            translation: Point::new(7.0, 11.0),
            scale: FIT_PX_PER_SP * zoom,
        };
        let crop = transform.rect(crop_sp);
        let ink = ink_fraction(
            &submitted(&passage, transform, DEVICE_SCALE),
            [
                crop.min.x * DEVICE_SCALE,
                crop.min.y * DEVICE_SCALE,
                crop.max.x * DEVICE_SCALE,
                crop.max.y * DEVICE_SCALE,
            ],
        );
        println!(
            "{zoom:<6} {:<14.2} {ink:.4}",
            transform.scale * DEVICE_SCALE
        );
        measured.push((zoom, ink));
    }

    let full_size = measured[0].1;
    assert!(
        full_size > 0.02,
        "the passage should carry real ink at full size, got {full_size:.4}"
    );
    for (zoom, ink) in measured.iter().copied().skip(1) {
        let ratio = ink / full_size;
        assert!(
            (0.80..=1.15).contains(&ratio),
            "at zoom {zoom} the same music reads {ratio:.2}x as heavy as at full size \
             ({ink:.4} vs {full_size:.4}); a smaller page must not gain ink"
        );
    }
}
