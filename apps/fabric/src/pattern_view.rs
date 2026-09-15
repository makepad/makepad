use crate::body_view::DrawFabricLine;
use makepad_fabric_draft::{flatten, nest, offset, Layout as PatternLayout, Part, Pattern, Point};
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.FabricPatternViewBase = #(FabricPatternView::register_widget(vm))
    mod.widgets.FabricPatternView = set_type_default() do mod.widgets.FabricPatternViewBase {
        width: Fill
        height: Fill
        draw_bg +: {color: #x0d1218}
        draw_line +: {}
        draw_text +: {
            color: #xc5d0dc
            text_style: theme.font_regular{font_size: 8.5}
        }
    }
}

#[derive(Clone, Copy)]
struct PatternDrag {
    from: DVec2,
    pan: DVec2,
}

#[derive(Clone, Copy, Default)]
struct Bounds {
    min: DVec2,
    max: DVec2,
    valid: bool,
}

impl Bounds {
    fn include(&mut self, point: DVec2) {
        if !self.valid {
            self.min = point;
            self.max = point;
            self.valid = true;
        } else {
            self.min.x = self.min.x.min(point.x);
            self.min.y = self.min.y.min(point.y);
            self.max.x = self.max.x.max(point.x);
            self.max.y = self.max.y.max(point.y);
        }
    }

    fn size(self) -> DVec2 {
        let size = self.max - self.min;
        dvec2(size.x.max(1.0), size.y.max(1.0))
    }

    fn centre(self) -> DVec2 {
        (self.min + self.max) * 0.5
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FabricPatternView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[area]
    area: Area,
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_line: DrawFabricLine,
    #[live]
    draw_text: DrawText,
    #[rust]
    pattern: Option<Pattern>,
    #[rust]
    nested: Option<PatternLayout>,
    #[rust]
    error: String,
    #[rust]
    bounds: Bounds,
    #[rust(1.0)]
    zoom: f64,
    /// The view aspect the current nest was chosen for.
    #[rust(1.0)]
    nest_aspect: f64,
    #[rust]
    pan: DVec2,
    #[rust]
    drag: Option<PatternDrag>,
}

impl FabricPatternView {
    pub fn set_pattern(&mut self, cx: &mut Cx, pattern: Pattern) {
        self.pattern = Some(pattern);
        // Nested on the next draw, for the pane's shape.
        self.nested = None;
        self.error.clear();
        self.zoom = 1.0;
        self.pan = dvec2(0.0, 0.0);
        self.redraw(cx);
    }

    pub fn set_error(&mut self, cx: &mut Cx, error: impl Into<String>) {
        self.pattern = None;
        self.nested = None;
        self.error = error.into();
        self.redraw(cx);
    }

    /// Nest onto the fabric width whose finished layout has the pane's
    /// aspect ratio, so the pieces fill the view instead of a tall strip.
    fn ensure_nest(&mut self, rect: Rect) {
        let target = (rect.size.x - 24.0).max(1.0) / (rect.size.y - 24.0).max(1.0);
        if self.nested.is_some() && (self.nest_aspect / target).ln().abs() < 0.12 {
            return;
        }
        const WIDTHS: [f64; 9] = [
            900.0, 1200.0, 1500.0, 2000.0, 2500.0, 3000.0, 4000.0, 5000.0, 6500.0,
        ];
        let best = {
            let Some(pattern) = &self.pattern else { return };
            let mut best: Option<(f64, PatternLayout, Bounds)> = None;
            for width in WIDTHS {
                let layout = nest(pattern, width);
                let bounds = pattern_bounds(pattern, &layout);
                let size = bounds.size();
                let aspect = size.x.max(1.0) / size.y.max(1.0);
                let score = (aspect / target).ln().abs();
                if best.as_ref().map_or(true, |(other, _, _)| score < *other) {
                    best = Some((score, layout, bounds));
                }
            }
            best
        };
        if let Some((_, layout, bounds)) = best {
            self.nested = Some(layout);
            self.bounds = bounds;
            self.nest_aspect = target;
        }
    }

    fn to_screen(&self, point: DVec2, rect: Rect) -> DVec2 {
        let size = self.bounds.size();
        let fit = ((rect.size.x - 24.0) / size.x)
            .min((rect.size.y - 24.0) / size.y)
            .max(0.0001);
        rect.pos + rect.size * 0.5
            + self.pan
            + (point - self.bounds.centre()) * fit * self.zoom
    }

    fn part_offset<'a>(
        &'a self,
        layout: &'a PatternLayout,
        part_index: usize,
    ) -> (Point, f64) {
        layout
            .placements
            .iter()
            .find(|placement| placement.part == part_index)
            .map(|placement| (placement.offset, placement.rotation_deg))
            .unwrap_or((Point::default(), 0.0))
    }

    fn path_segments(
        &self,
        path: &makepad_fabric_draft::Path,
        offset_by: Point,
        rotation: f64,
        rect: Rect,
    ) -> Vec<(DVec2, DVec2)> {
        let points = flatten(path, 1.0);
        let transformed: Vec<DVec2> = points
            .iter()
            .map(|point| self.to_screen(place(*point, offset_by, rotation), rect))
            .collect();
        let mut segments: Vec<_> = transformed
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .collect();
        if path.closed {
            if let (Some(first), Some(last)) = (transformed.first(), transformed.last()) {
                segments.push((*last, *first));
            }
        }
        segments
    }
}

impl Widget for FabricPatternView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.draw_abs(cx, rect);
        cx.push_clip_rect(rect);
        self.ensure_nest(rect);
        let (Some(pattern), Some(layout)) = (&self.pattern, &self.nested) else {
            let message = if self.error.is_empty() {
                "pattern preview"
            } else {
                &self.error
            };
            self.draw_text.color = Vec4f {
                x: 0.52,
                y: 0.58,
                z: 0.65,
                w: 1.0,
            };
            self.draw_text
                .draw_abs(cx, rect.pos + dvec2(14.0, 16.0), message);
            cx.pop_clip_rect();
            cx.add_aligned_rect_area(&mut self.area, rect);
            return DrawStep::done();
        };

        struct Stroke {
            from: DVec2,
            to: DVec2,
            color: Vec4f,
            width: f64,
        }
        let mut strokes = Vec::new();
        let mut labels = Vec::new();
        let cut_color = Vec4f {
            x: 0.93,
            y: 0.96,
            z: 0.99,
            w: 1.0,
        };
        let seam_color = Vec4f {
            x: 0.40,
            y: 0.49,
            z: 0.58,
            w: 0.72,
        };
        let mark_color = Vec4f {
            x: 1.0,
            y: 0.40,
            z: 0.18,
            w: 0.95,
        };

        for (part_index, part) in pattern.parts.iter().enumerate() {
            let (part_offset, rotation) = self.part_offset(layout, part_index);
            let cut_path = offset(&part.outline, part.seam_allowance_mm);
            for (from, to) in self.path_segments(&cut_path, part_offset, rotation, rect) {
                strokes.push(Stroke {
                    from,
                    to,
                    color: cut_color,
                    width: 1.25,
                });
            }
            for (from, to) in self.path_segments(&part.outline, part_offset, rotation, rect) {
                strokes.push(Stroke {
                    from,
                    to,
                    color: seam_color,
                    width: 0.75,
                });
            }
            for path in &part.internal {
                for (from, to) in self.path_segments(path, part_offset, rotation, rect) {
                    strokes.push(Stroke {
                        from,
                        to,
                        color: seam_color,
                        width: 0.75,
                    });
                }
            }
            for notch in &part.notches {
                let at = self.to_screen(place(*notch, part_offset, rotation), rect);
                strokes.push(Stroke {
                    from: at + dvec2(-3.0, -3.0),
                    to: at + dvec2(3.0, 3.0),
                    color: mark_color,
                    width: 1.0,
                });
                strokes.push(Stroke {
                    from: at + dvec2(-3.0, 3.0),
                    to: at + dvec2(3.0, -3.0),
                    color: mark_color,
                    width: 1.0,
                });
            }
            let grain_from = self.to_screen(place(part.grainline.0, part_offset, rotation), rect);
            let grain_to = self.to_screen(place(part.grainline.1, part_offset, rotation), rect);
            strokes.push(Stroke {
                from: grain_from,
                to: grain_to,
                color: mark_color,
                width: 1.0,
            });
            let label_at = part
                .labels
                .first()
                .map(|label| label.at)
                .unwrap_or(part.outline.start);
            labels.push((
                self.to_screen(place(label_at, part_offset, rotation), rect),
                format!(
                    "{} · {}",
                    part.name,
                    if part.on_fold {
                        "cut on fold".to_string()
                    } else {
                        format!("cut {}", part.cut_count)
                    }
                ),
            ));
        }

        self.draw_line.begin_many_instances(cx);
        for stroke in strokes {
            self.draw_line.color = stroke.color;
            self.draw_line
                .segment(cx, stroke.from, stroke.to, stroke.width);
        }
        self.draw_line.end_many_instances(cx);
        self.draw_text.color = Vec4f {
            x: 0.78,
            y: 0.84,
            z: 0.90,
            w: 1.0,
        };
        for (at, text) in labels {
            self.draw_text.draw_abs(cx, at + dvec2(4.0, -5.0), &text);
        }
        cx.pop_clip_rect();
        cx.add_aligned_rect_area(&mut self.area, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerDown(event) if event.device.is_primary_hit() => {
                self.drag = Some(PatternDrag {
                    from: event.abs,
                    pan: self.pan,
                });
            }
            Hit::FingerMove(event) => {
                if let Some(drag) = self.drag {
                    self.pan = drag.pan + event.abs - drag.from;
                    self.redraw(cx);
                }
            }
            Hit::FingerUp(_) => self.drag = None,
            Hit::FingerScroll(event) => {
                self.zoom = (self.zoom * (-event.scroll.y * 0.004).exp()).clamp(0.1, 30.0);
                self.redraw(cx);
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Grab)
            }
            _ => {}
        }
    }
}

fn place(point: Point, offset: Point, rotation_deg: f64) -> DVec2 {
    let angle = rotation_deg.to_radians();
    let (sin, cos) = angle.sin_cos();
    dvec2(
        point.x * cos - point.y * sin + offset.x,
        point.x * sin + point.y * cos + offset.y,
    )
}

fn pattern_bounds(pattern: &Pattern, layout: &PatternLayout) -> Bounds {
    let mut bounds = Bounds::default();
    for (part_index, part) in pattern.parts.iter().enumerate() {
        let (part_offset, rotation) = layout
            .placements
            .iter()
            .find(|placement| placement.part == part_index)
            .map(|placement| (placement.offset, placement.rotation_deg))
            .unwrap_or((Point::default(), 0.0));
        include_part(&mut bounds, part, part_offset, rotation);
    }
    if !bounds.valid && layout.width_mm > 0.0 && layout.height_mm > 0.0 {
        bounds.include(dvec2(0.0, 0.0));
        bounds.include(dvec2(layout.width_mm, layout.height_mm));
    }
    bounds
}

fn include_part(bounds: &mut Bounds, part: &Part, part_offset: Point, rotation: f64) {
    let cut = offset(&part.outline, part.seam_allowance_mm);
    for point in flatten(&cut, 1.0) {
        bounds.include(place(point, part_offset, rotation));
    }
}
