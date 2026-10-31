//! Generic checks over a drawn widget tree, read back from what the turtle
//! actually laid out: every widget's area rect, and every text run's real
//! metrics (the draw records them while a check is collecting, see
//! [`TextRunSink`]).
//!
//! Runtime, not test-only: a host draws its UI, calls [`check`] with the pane
//! rect and gets [`UiIssue`]s keyed by widget path (the `name :=` ids from
//! the pane's root). [`draw_and_check`] does the draw headlessly at a given
//! size, so a tool can check a UI at several pane sizes without a window.
//! [`layout_hash`] walks the same tree, so a caller can run the checks only
//! when the layout changed.
//!
//! The checks, all pane-relative where they compare sizes:
//! - `offscreen`: a widget's visible rect leaves the pane's safe area.
//! - `clipped`: text cut short (ellipsis, line limit), or glyphs a clip around
//!   it hides (read from each glyph's own draw clip). Text inside a widget that
//!   `clips_content` (scroll views, lists, inputs) or under an offscreen one is
//!   excused; text given a zero-size box is a hidden label, not a fault.
//! - `tiny` / `huge`: text cap height against the pane height.
//! - `overlap`: the given rects (or a container's direct children) intersect.
//! - `contrast`: text ink against the nearest ancestor plate, only where that
//!   plate's colour is a drawn instance value; never guessed. Only the
//!   instance named `color` is read, so a plate that paints a gradient or
//!   blends in its shader may be judged against the wrong colour.
use crate::*;
use makepad_draw::cx_2d::{TextRun, TextRunSink};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UiIssueKind {
    Offscreen,
    Clipped,
    Tiny,
    Huge,
    Overlap,
    Contrast,
}

impl UiIssueKind {
    pub fn name(self) -> &'static str {
        match self {
            UiIssueKind::Offscreen => "offscreen",
            UiIssueKind::Clipped => "clipped",
            UiIssueKind::Tiny => "tiny",
            UiIssueKind::Huge => "huge",
            UiIssueKind::Overlap => "overlap",
            UiIssueKind::Contrast => "contrast",
        }
    }
}

#[derive(Clone, Debug)]
pub struct UiIssue {
    /// Names of the widget and its ancestors from the checked root, unnamed
    /// ones left out, joined by `.`; `(root)` for the root itself.
    pub path: String,
    pub kind: UiIssueKind,
    pub detail: String,
}

impl std::fmt::Display for UiIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.kind.name(), self.path, self.detail)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct UiCheckOptions {
    /// The pane in the draw's coordinates.
    pub pane: Rect,
    /// Insets kept clear of the pane's edges (left, top, right, bottom).
    pub safe: [f64; 4],
    /// Text cap height below this fraction of the pane height is `tiny`.
    pub tiny: f64,
    /// Text cap height above this fraction of the pane height is `huge`.
    pub huge: f64,
    /// Minimum WCAG contrast ratio of text ink against its plate.
    pub contrast: f64,
    /// Slack in logical pixels before a rect counts as leaving another.
    pub tolerance: f64,
}

impl UiCheckOptions {
    pub fn for_pane(size: Vec2d) -> Self {
        Self {
            pane: Rect { pos: dvec2(0.0, 0.0), size },
            safe: [0.0; 4],
            tiny: 0.012,
            huge: 0.12,
            contrast: 3.0,
            tolerance: 1.0,
        }
    }

    fn safe_rect(&self) -> Rect {
        let [l, t, r, b] = self.safe;
        Rect {
            pos: self.pane.pos + dvec2(l, t),
            size: dvec2(self.pane.size.x - l - r, self.pane.size.y - t - b),
        }
    }
}

/// One visited widget.
struct Visit {
    path: Vec<LiveId>,
    parent: Option<usize>,
    widget: WidgetRef,
    /// The rect a viewer sees (clipped), `None` when not drawn or clipped away.
    rect: Option<Rect>,
    /// The rect as laid out, before any clip, `None` when not drawn.
    laid_out: Option<Rect>,
}


const MAX_DEPTH: usize = 96;

/// Depth-first over the visible widgets under `root`, parents before children.
fn walk(cx: &Cx, root: &WidgetRef, visit: &mut dyn FnMut(&[LiveId], Option<usize>, &WidgetRef, Option<Rect>, Option<Rect>) -> usize) {
    fn rec(
        cx: &Cx,
        widget: &WidgetRef,
        path: &mut Vec<LiveId>,
        pending: &mut Vec<(LiveId, WidgetRef)>,
        parent: Option<usize>,
        depth: usize,
        visit: &mut dyn FnMut(&[LiveId], Option<usize>, &WidgetRef, Option<Rect>, Option<Rect>) -> usize,
    ) {
        if !widget.visible() {
            return;
        }
        let area = widget.area();
        let valid = area.is_valid(cx);
        let rect = valid.then(|| area.clipped_rect(cx)).filter(|r| r.size.x > 0.0 && r.size.y > 0.0);
        let laid_out = valid.then(|| area.rect(cx)).filter(|r| r.size.x > 0.0 && r.size.y > 0.0);
        let index = visit(path, parent, widget, rect, laid_out);
        if depth >= MAX_DEPTH {
            return;
        }
        // One buffer for the whole walk: this node's children sit above its
        // parents' unvisited siblings and are cut off when done.
        let start = pending.len();
        widget.children(&mut |id, child| pending.push((id, child)));
        for k in start..pending.len() {
            let (id, child) = pending[k].clone();
            if id != LiveId(0) {
                path.push(id);
            }
            rec(cx, &child, path, pending, Some(index), depth + 1, visit);
            if id != LiveId(0) {
                path.pop();
            }
        }
        pending.truncate(start);
    }
    rec(cx, root, &mut Vec::new(), &mut Vec::new(), None, 0, visit);
}

fn collect(cx: &Cx, root: &WidgetRef) -> Vec<Visit> {
    let mut out: Vec<Visit> = Vec::new();
    walk(cx, root, &mut |path, parent, widget, rect, laid_out| {
        out.push(Visit { path: path.to_vec(), parent, widget: widget.clone(), rect, laid_out });
        out.len() - 1
    });
    out
}

fn path_string(path: &[LiveId]) -> String {
    if path.is_empty() {
        return "(root)".to_string();
    }
    let mut out = String::new();
    for id in path {
        if !out.is_empty() {
            out.push('.');
        }
        out.push_str(&crate::widget_tree::live_id_token(*id));
    }
    out
}

/// A hash of the laid-out tree: every visible widget's path and rect (to a
/// quarter pixel). Equal while the layout stands; a caller runs [`check`]
/// when it changes. Allocates only the walk's small stacks.
pub fn layout_hash(cx: &Cx, root: &WidgetRef) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    let mut count = 0usize;
    walk(cx, root, &mut |path, _, _, rect, _| {
        mix(path.len() as u64);
        for id in path {
            mix(id.0);
        }
        match rect {
            Some(r) => {
                for v in [r.pos.x, r.pos.y, r.size.x, r.size.y] {
                    mix((v * 4.0).round() as i64 as u64);
                }
            }
            None => mix(u64::MAX),
        }
        count += 1;
        count - 1
    });
    hash
}

/// `i` or an ancestor of it is a widget reported offscreen.
fn under_off(nodes: &[Visit], off: &[bool], mut i: usize) -> bool {
    loop {
        if off[i] {
            return true;
        }
        match nodes[i].parent {
            Some(p) => i = p,
            None => return false,
        }
    }
}

fn outside(inner: Rect, outer: Rect) -> [f64; 4] {
    [
        (outer.pos.x - inner.pos.x).max(0.0),
        (outer.pos.y - inner.pos.y).max(0.0),
        ((inner.pos.x + inner.size.x) - (outer.pos.x + outer.size.x)).max(0.0),
        ((inner.pos.y + inner.size.y) - (outer.pos.y + outer.size.y)).max(0.0),
    ]
}

fn fmt_rect(r: Rect) -> String {
    format!("[{:.0},{:.0} {:.0}x{:.0}]", r.pos.x, r.pos.y, r.size.x, r.size.y)
}

fn srgb_luminance(c: [f32; 3]) -> f64 {
    let lin = |v: f32| {
        let v = v.clamp(0.0, 1.0) as f64;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
}

/// WCAG contrast ratio of two colours.
pub fn contrast_ratio(a: [f32; 3], b: [f32; 3]) -> f64 {
    let (la, lb) = (srgb_luminance(a), srgb_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The colour a view's background plate was drawn with, when it has one and
/// the colour is an instance value of its draw (anything else is unknown).
fn plate_color(cx: &Cx, widget: &WidgetRef) -> Option<[f32; 4]> {
    let view = widget.borrow::<View>()?;
    if !view.show_bg {
        return None;
    }
    let mut color = [0.0f32; 4];
    view.draw_bg.draw_vars.get_instance_on_area(cx, id!(color), &mut color).then_some(color)
}

/// Check a drawn tree. `runs` are the text runs collected during its draw.
pub fn check(cx: &Cx, root: &WidgetRef, runs: &[TextRun], opts: &UiCheckOptions) -> Vec<UiIssue> {
    let nodes = collect(cx, root);
    let mut issues = Vec::new();
    let safe = opts.safe_rect();
    let tol = opts.tolerance;

    // offscreen: the outermost widget that leaves the safe area reports. A
    // widget inside a scrolling container is judged by the part that shows
    // (it scrolls in); any other by its laid-out rect, which the clips of the
    // views around it would otherwise hide.
    let mut in_scroll = vec![false; nodes.len()];
    let mut off = vec![false; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        if let Some(p) = node.parent {
            in_scroll[i] = in_scroll[p] || nodes[p].widget.clips_content();
        }
        let Some(rect) = (if in_scroll[i] { node.rect } else { node.laid_out }) else { continue };
        let out = outside(rect, safe);
        if out.iter().any(|v| *v > tol) {
            off[i] = true;
            if node.parent.is_some_and(|p| off[p]) {
                continue;
            }
            issues.push(UiIssue {
                path: path_string(&node.path),
                kind: UiIssueKind::Offscreen,
                detail: format!(
                    "{} leaves the safe area {} by left {:.0} top {:.0} right {:.0} bottom {:.0}",
                    fmt_rect(rect), fmt_rect(safe), out[0], out[1], out[2], out[3]
                ),
            });
        }
    }

    // Text runs, by the widget that drew them.
    let by_uid: std::collections::HashMap<u64, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.widget.widget_uid().0, i)).collect();
    let owner_index = |uid: u64| by_uid.get(&uid).copied();
    for run in runs {
        let index = owner_index(run.owner);
        let path = match index {
            Some(i) => path_string(&nodes[i].path),
            None => format!("(widget {})", run.owner),
        };
        let cap = if run.cap_height > 0.0 { run.cap_height as f64 } else { run.font_size as f64 * 0.7 };
        let frac = cap / opts.pane.size.y.max(1.0);
        if frac < opts.tiny {
            issues.push(UiIssue {
                path: path.clone(),
                kind: UiIssueKind::Tiny,
                detail: format!("text cap height {:.1}px is {:.2}% of the pane height (min {:.2}%)", cap, frac * 100.0, opts.tiny * 100.0),
            });
        } else if frac > opts.huge {
            issues.push(UiIssue {
                path: path.clone(),
                kind: UiIssueKind::Huge,
                detail: format!("text cap height {:.1}px is {:.2}% of the pane height (max {:.2}%)", cap, frac * 100.0, opts.huge * 100.0),
            });
        }

        // clipped: the layout cut it short, or a clip around it hides part of
        // it (every glyph carries the clip it was drawn under). Text inside a
        // scrolling container scrolls in, and text under a widget already
        // reported offscreen has that as its cause.
        let excused = index.is_some_and(|i| in_scroll[i] || under_off(&nodes, &off, i));
        let mut why = None;
        if run.truncated {
            why = Some("the layout cut it short".to_string());
        } else if !excused && !run.declared_empty && run.area.is_valid(cx) {
            let shown = run.area.clipped_rect_union(cx);
            let laid = run.area.rect_union(cx);
            // What was laid out: the glyph quads, unless they are far bigger
            // than the text (a quad sized to its whole row); then the text's
            // own measure.
            let natural = run.rect.size;
            let reference = if laid.size.x <= natural.x * 1.5 + 4.0 && laid.size.y <= natural.y * 1.5 + 4.0 { laid.size } else { natural };
            // A glyph quad carries a pixel or two of padding around its ink.
            let slack = 1.0 + 0.15 * run.font_size as f64;
            if shown.size.x <= 0.0 || shown.size.y <= 0.0 {
                why = Some(format!("none of the {:.0}x{:.0} text shows", reference.x, reference.y));
            } else if reference.x - shown.size.x > slack || reference.y - shown.size.y > slack {
                why = Some(format!(
                    "{:.0}x{:.0} of text laid out, {:.0}x{:.0} shows ({})",
                    reference.x, reference.y, shown.size.x, shown.size.y, fmt_rect(shown)
                ));
            }
        }
        if let Some(why) = why {
            issues.push(UiIssue { path: path.clone(), kind: UiIssueKind::Clipped, detail: why });
        }

        // contrast against the nearest plate holding the run
        // Where the text really is comes from its area (a parent's alignment
        // moves it after the draw); a run with no area is not placed, so not
        // judged.
        let placed = run.area.is_valid(cx).then(|| run.area.rect_union(cx));
        let mut at = index.filter(|_| placed.is_some());
        let centre = placed.map(|r| r.pos + r.size * 0.5).unwrap_or_default();
        while let Some(i) = at {
            let node = &nodes[i];
            let holds = node.rect.is_some_and(|r| {
                centre.x >= r.pos.x && centre.x <= r.pos.x + r.size.x && centre.y >= r.pos.y && centre.y <= r.pos.y + r.size.y
            });
            if holds {
                if let Some(plate) = plate_color(cx, &node.widget) {
                    if plate[3] >= 0.95 && run.color[3] >= 0.5 {
                        let ratio = contrast_ratio([run.color[0], run.color[1], run.color[2]], [plate[0], plate[1], plate[2]]);
                        if ratio < opts.contrast {
                            issues.push(UiIssue {
                                path: path.clone(),
                                kind: UiIssueKind::Contrast,
                                detail: format!("contrast {:.2} against the plate of {} (min {:.1})", ratio, path_string(&node.path), opts.contrast),
                            });
                        }
                        break;
                    }
                    // A see-through plate decides nothing: look further out.
                }
            }
            at = node.parent;
        }
    }
    issues
}

/// `overlap` over named rects: every pair that intersects by more than the
/// slack on both axes.
pub fn overlaps(rects: &[(String, Rect)], tolerance: f64) -> Vec<UiIssue> {
    let mut issues = Vec::new();
    for (i, (name_a, a)) in rects.iter().enumerate() {
        for (name_b, b) in &rects[i + 1..] {
            let w = (a.pos.x + a.size.x).min(b.pos.x + b.size.x) - a.pos.x.max(b.pos.x);
            let h = (a.pos.y + a.size.y).min(b.pos.y + b.size.y) - a.pos.y.max(b.pos.y);
            if w > tolerance && h > tolerance {
                issues.push(UiIssue {
                    path: format!("{name_a}+{name_b}"),
                    kind: UiIssueKind::Overlap,
                    detail: format!("{} and {} intersect by {:.0}x{:.0}", fmt_rect(*a), fmt_rect(*b), w, h),
                });
            }
        }
    }
    issues
}

/// `overlap` between the direct children of a drawn container.
pub fn child_overlaps(cx: &Cx, container: &WidgetRef, tolerance: f64) -> Vec<UiIssue> {
    let mut rects: Vec<(String, Rect)> = Vec::new();
    container.children(&mut |id, child| {
        if !child.visible() {
            return;
        }
        let area = child.area();
        if !area.is_valid(cx) {
            return;
        }
        let rect = area.clipped_rect(cx);
        if rect.size.x > 0.0 && rect.size.y > 0.0 {
            rects.push((crate::widget_tree::live_id_token(id), rect));
        }
    });
    overlaps(&rects, tolerance)
}

/// A pass and draw list to draw a tree into again and again.
pub struct UiCheckTarget {
    pass: DrawPass,
    draw_list: DrawList2d,
    overlay: Overlay,
}

impl UiCheckTarget {
    pub fn new(cx: &mut Cx) -> Self {
        Self { pass: DrawPass::new(cx), draw_list: DrawList2d::new(cx), overlay: Overlay { draw_list: DrawList::new(cx) } }
    }

    /// Draw `root` into a pane of `size` and return the text runs laid out.
    pub fn draw(&mut self, cx: &mut Cx, root: &WidgetRef, size: Vec2d) -> Vec<TextRun> {
        use makepad_draw::cx_draw::CxDraw;
        self.pass.set_size(cx, size);
        let event = DrawEvent::default();
        let mut draw = CxDraw::new(cx, &event);
        let mut cx2d = Cx2d::new(&mut draw);
        cx2d.text_runs = Some(TextRunSink::default());
        cx2d.begin_pass(&self.pass, None);
        self.draw_list.begin_always(&mut cx2d);
        self.overlay.begin(&mut cx2d);
        cx2d.begin_root_turtle(size, Layout::flow_down());
        root.draw_all(&mut cx2d, &mut Scope::empty());
        cx2d.end_pass_sized_turtle();
        self.overlay.end(&mut cx2d);
        self.draw_list.end(&mut cx2d);
        cx2d.end_pass(&self.pass);
        cx2d.text_runs.take().map(|sink| sink.runs).unwrap_or_default()
    }
}

/// Draw `root` headlessly at `size` (twice, the second settling what the
/// first measured) and check it. Works on a `TestCx` with the real fonts.
pub fn draw_and_check(cx: &mut Cx, root: &WidgetRef, size: Vec2d, opts: &UiCheckOptions) -> Vec<UiIssue> {
    let mut target = UiCheckTarget::new(cx);
    target.draw(cx, root, size);
    let runs = target.draw(cx, root, size);
    check(cx, root, &runs, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(cx: &mut Cx, source: fn(&mut ScriptVm) -> ScriptValue) -> WidgetRef {
        cx.with_vm(|vm| {
            let value = source(vm);
            WidgetRef::script_from_value(vm, value)
        })
    }

    fn run(source: fn(&mut ScriptVm) -> ScriptValue, size: Vec2d, tweak: impl FnOnce(&mut UiCheckOptions)) -> Vec<UiIssue> {
        let mut cx = crate::checkout_test_cx();
        let root = scene(&mut cx, source);
        let mut opts = UiCheckOptions::for_pane(size);
        tweak(&mut opts);
        draw_and_check(&mut cx, &root, size, &opts)
    }

    fn of(issues: &[UiIssue], kind: UiIssueKind) -> Vec<&UiIssue> {
        issues.iter().filter(|i| i.kind == kind).collect()
    }

    const SIZE: Vec2d = Vec2d { x: 400.0, y: 300.0 };

    fn plain(vm: &mut ScriptVm) -> ScriptValue {
        crate::script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            View{
                width: Fill
                height: Fill
                flow: Down
                title := Label{ width: Fit height: Fit text: "Hello there" draw_text.text_style.font_size: 16 }
            }
        })
    }

    #[test]
    fn a_plain_label_has_no_issues() {
        let issues = run(plain, SIZE, |o| {
            o.tiny = 0.01;
            o.huge = 0.5;
        });
        assert!(issues.is_empty(), "{issues:?}");
    }

    fn wide(vm: &mut ScriptVm) -> ScriptValue {
        crate::script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            View{
                width: Fill
                height: Fill
                flow: Down
                box := View{
                    width: 120. height: 40. flow: Down
                    long := Label{ width: Fit height: Fit text: "Hello there" draw_text.text_style.font_size: 16 }
                }
                off := View{ width: 100. height: 30. margin: Inset{left: 350.} }
            }
        })
    }

    #[test]
    fn a_widget_past_the_pane_edge_is_offscreen_once_by_the_amount_it_leaves() {
        let issues = run(wide, SIZE, |_| {});
        let off = of(&issues, UiIssueKind::Offscreen);
        assert_eq!(off.len(), 1, "{issues:?}");
        assert_eq!(off[0].path, "off");
        assert!(off[0].detail.contains("right 50"), "{}", off[0].detail);
        // A safe-area inset counts too.
        let issues = run(plain, SIZE, |o| o.safe = [20.0, 0.0, 0.0, 0.0]);
        assert_eq!(of(&issues, UiIssueKind::Offscreen).len(), 1, "{issues:?}");
    }

    fn red(vm: &mut ScriptVm) -> ScriptValue {
        crate::script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            View{
                width: Fill
                height: Fill
                flow: Down
                plate := RoundedView{
                    width: 700. height: 40.
                    show_bg: true
                    draw_bg.color: #xff0000
                    over := Label{ width: Fit height: Fit text: "Hello there" draw_text.text_style.font_size: 16 }
                }
            }
        })
    }

    #[test]
    fn a_wide_plate_is_offscreen_and_reports_at_its_own_path() {
        let issues = run(red, SIZE, |o| {
            o.tiny = 0.0;
            o.huge = 1.0;
        });
        let off = of(&issues, UiIssueKind::Offscreen);
        assert_eq!(off.len(), 1, "only the outermost widget reports: {issues:?}");
        assert_eq!(off[0].path, "plate");
    }

    #[test]
    fn white_text_on_a_red_plate_is_checked_against_the_plate() {
        let issues = run(red, Vec2d { x: 800.0, y: 300.0 }, |o| {
            o.tiny = 0.0;
            o.huge = 1.0;
            o.contrast = 6.0;
        });
        assert!(issues.iter().all(|i| i.kind != UiIssueKind::Offscreen), "{issues:?}");
        // Red plate against the default (white) ink is about 4:1: under 6, over 3.
        assert_eq!(of(&issues, UiIssueKind::Contrast).len(), 1, "{issues:?}");
        let issues = run(red, Vec2d { x: 800.0, y: 300.0 }, |o| {
            o.tiny = 0.0;
            o.huge = 1.0;
            o.contrast = 3.0;
        });
        assert!(of(&issues, UiIssueKind::Contrast).is_empty(), "{issues:?}");
    }

    #[test]
    fn text_size_is_judged_against_the_pane_height() {
        let tiny = run(plain, SIZE, |o| o.tiny = 0.2);
        assert_eq!(of(&tiny, UiIssueKind::Tiny).len(), 1, "{tiny:?}");
        let huge = run(plain, SIZE, |o| {
            o.tiny = 0.0;
            o.huge = 0.01;
        });
        assert_eq!(of(&huge, UiIssueKind::Huge).len(), 1, "{huge:?}");
        let fine = run(plain, SIZE, |o| {
            o.tiny = 0.01;
            o.huge = 0.5;
        });
        assert!(fine.is_empty(), "{fine:?}");
    }

    fn ellipsis(vm: &mut ScriptVm) -> ScriptValue {
        crate::script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            View{
                width: Fill
                height: Fill
                flow: Down
                cut := Label{
                    width: 60. height: Fit
                    text: "A label that is much too long for its box"
                    max_lines: 1
                    text_overflow: Ellipsis
                    draw_text.text_style.font_size: 16
                }
            }
        })
    }

    #[test]
    fn an_ellipsized_label_is_clipped() {
        let issues = run(ellipsis, SIZE, |o| {
            o.tiny = 0.0;
            o.huge = 1.0;
        });
        let clipped = of(&issues, UiIssueKind::Clipped);
        assert_eq!(clipped.len(), 1, "{issues:?}");
        assert_eq!(clipped[0].path, "cut");
        assert_eq!(clipped[0].detail, "the layout cut it short");
    }

    fn scrolled(vm: &mut ScriptVm) -> ScriptValue {
        crate::script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            View{
                width: Fill
                height: Fill
                flow: Down
                list := ScrollYView{
                    width: Fill height: 100. flow: Down
                    row_a := RoundedView{ width: Fill height: 60. show_bg: true draw_bg.color: #x334455 }
                    row_b := RoundedView{ width: Fill height: 60. show_bg: true draw_bg.color: #x445566 }
                    row_c := RoundedView{ width: Fill height: 400. show_bg: true draw_bg.color: #x556677 }
                }
            }
        })
    }

    #[test]
    fn rows_a_scroll_view_hides_are_not_offscreen_but_the_same_rows_in_a_plain_view_are() {
        // 100 px high view in a 300 px pane: row_c runs far past the view and
        // past the pane (60 + 60 + 400 > 300). Scrolled, that is by design.
        let issues = run(scrolled, SIZE, |_| {});
        assert!(of(&issues, UiIssueKind::Offscreen).is_empty(), "{issues:?}");
        let issues = run(plain_rows, SIZE, |_| {});
        let off = of(&issues, UiIssueKind::Offscreen);
        assert_eq!(off.len(), 1, "{issues:?}");
        assert_eq!(off[0].path, "list.row_c");
    }

    fn plain_rows(vm: &mut ScriptVm) -> ScriptValue {
        crate::script_eval!(vm, {
            use mod.prelude.widgets.*
            use mod.widgets.*
            View{
                width: Fill
                height: Fill
                flow: Down
                list := View{
                    width: Fill height: 100. flow: Down
                    row_a := RoundedView{ width: Fill height: 60. show_bg: true draw_bg.color: #x334455 }
                    row_b := RoundedView{ width: Fill height: 60. show_bg: true draw_bg.color: #x445566 }
                    row_c := RoundedView{ width: Fill height: 400. show_bg: true draw_bg.color: #x556677 }
                }
            }
        })
    }

    #[test]
    fn overlap_reports_intersecting_rects_only() {
        let r = |x: f64, y: f64, w: f64, h: f64| Rect { pos: dvec2(x, y), size: dvec2(w, h) };
        let issues = overlaps(
            &[("a".into(), r(0.0, 0.0, 100.0, 50.0)), ("b".into(), r(90.0, 10.0, 100.0, 50.0)), ("c".into(), r(100.0, 0.0, 50.0, 50.0))],
            0.5,
        );
        let pairs: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(pairs, vec!["a+b", "b+c"]);
    }

    #[test]
    fn the_layout_hash_follows_the_layout() {
        let mut cx = crate::checkout_test_cx();
        let root = scene(&mut cx, plain);
        let mut target = UiCheckTarget::new(&mut cx);
        target.draw(&mut cx, &root, SIZE);
        let a = layout_hash(&cx, &root);
        target.draw(&mut cx, &root, SIZE);
        assert_eq!(a, layout_hash(&cx, &root), "same layout, same hash");
        target.draw(&mut cx, &root, Vec2d { x: 500.0, y: 300.0 });
        let b = layout_hash(&cx, &root);
        root.widget(&cx, ids!(title)).set_text(&mut cx, "A much longer title than before");
        target.draw(&mut cx, &root, Vec2d { x: 500.0, y: 300.0 });
        assert_ne!(b, layout_hash(&cx, &root), "a wider label changes the hash");
    }

    #[test]
    fn a_drawn_run_is_attributed_to_its_widget() {
        let mut cx = crate::checkout_test_cx();
        let root = scene(&mut cx, plain);
        let mut target = UiCheckTarget::new(&mut cx);
        let runs = target.draw(&mut cx, &root, SIZE);
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert!(runs[0].owner != 0, "the run is attributed to its widget");
    }
}
