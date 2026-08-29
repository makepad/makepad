//! The data layer behind the folder size map: a recursive scan of a
//! directory's bytes, and the squarified-treemap geometry that turns a list
//! of sizes into non-overlapping rectangles whose areas are proportional to
//! them. Nothing here knows about widgets, drawing, or the rest of the app —
//! a [`Cell`] is just numbers a view can turn into quads, which is what
//! keeps this module runnable and testable on its own.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// A rectangle in treemap space. Plain `f64` so this module stays free of
/// any UI vector type — the view converts to its own types at the boundary.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// Whether `(px, py)` lands inside this rect. The right and bottom edges
    /// are excluded, so two rects tiled edge to edge never both claim the
    /// seam between them — a point on a shared border belongs to exactly
    /// one of the two, never both and never neither.
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Plain `w * h`. A degenerate slice (zero or negative on an edge) just
    /// falls out of this as zero or negative rather than needing its own
    /// case — callers that care about "does this rect draw at all" check
    /// the edges directly.
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
}

/// One scanned entry. Folders carry their children; files are leaves. This
/// is the tree [`scan`] hands back and [`layout`] consumes; nothing else in
/// this module mutates it.
#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Recursive byte total for a folder; the file's own size for a leaf.
    pub size: u64,
    /// Opaque kind tag supplied by the caller's `classify` callback — this
    /// module never decides what a file *is*, only how big it is and where
    /// it sits in the tree.
    pub kind: u8,
    pub children: Vec<Node>,
}

impl Node {
    /// Total entries in this subtree, including itself — the number a
    /// status line means by "12,000 items".
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Node::count).sum::<usize>()
    }
}

/// Progress a scan reports as it walks, so a caller can show a live "N
/// files, N bytes" line instead of a frozen spinner while a big tree is
/// still being counted.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanProgress {
    pub files: u64,
    pub bytes: u64,
}

/// How many entries pass between progress reports. A treemap of a real disk
/// can hold hundreds of thousands of entries; reporting every single one
/// would make the report channel itself the bottleneck, so this trades
/// reporting granularity for a scan that stays fast.
const PROGRESS_STRIDE: u64 = 512;

/// The bits [`scan_node`] threads through the walk instead of passing four
/// separate arguments at every recursive call.
struct ScanState<'a> {
    classify: &'a dyn Fn(&Path, bool) -> u8,
    cancel: &'a AtomicBool,
    progress: &'a dyn Fn(ScanProgress),
    since_report: u64,
    total: ScanProgress,
}

impl<'a> ScanState<'a> {
    /// Call this once per entry visited. Only actually invokes `progress`
    /// every [`PROGRESS_STRIDE`] entries — see that constant for why.
    fn tick(&mut self) {
        self.since_report += 1;
        if self.since_report >= PROGRESS_STRIDE {
            self.since_report = 0;
            (self.progress)(self.total);
        }
    }
}

/// Walk `root` recursively, folding each folder's size up from its children.
///
/// Symlinks are never followed: a link is recorded as its own leaf, sized by
/// the link itself and never by whatever it points at, and it is never
/// recursed into. That single rule is what keeps a cyclic link — a folder
/// somewhere under `root` linking back to one of its own ancestors — from
/// turning a scan into an infinite walk.
///
/// Returns `None` the moment `cancel` is found set, so a cancelled scan
/// never hands back a half-built tree that would go on to paint as a
/// plausible-looking but wrong picture.
pub fn scan(
    root: &Path,
    classify: &dyn Fn(&Path, bool) -> u8,
    cancel: &AtomicBool,
    progress: &dyn Fn(ScanProgress),
) -> Option<Node> {
    let mut state = ScanState {
        classify,
        cancel,
        progress,
        since_report: 0,
        total: ScanProgress::default(),
    };
    let node = scan_node(root, &mut state)?;
    // One last report so a caller that only reads the callback's argument
    // after the walk returns still sees the true final tally, rather than
    // whatever the last PROGRESS_STRIDE boundary happened to leave behind.
    (state.progress)(state.total);
    Some(node)
}

fn scan_node(path: &Path, state: &mut ScanState) -> Option<Node> {
    // Checked on every entry, not just every directory, so a folder holding
    // one huge flat pile of files still cancels within a fraction of a
    // second rather than only between directories.
    if state.cancel.load(Ordering::Relaxed) {
        return None;
    }
    let meta = fs::symlink_metadata(path).ok()?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    // `symlink_metadata` never follows the link, so a symlink's own
    // `is_dir()` reads false here even when it points at a directory —
    // exactly the leaf treatment a link needs.
    if meta.is_dir() {
        let mut children = Vec::new();
        let mut size = 0u64;
        // An unreadable folder (permissions, vanished mid-walk) is not a
        // scan failure. It is a folder we know exists and cannot see into,
        // so it is recorded with no children rather than aborting the walk.
        if let Ok(read_dir) = fs::read_dir(path) {
            for entry in read_dir.flatten() {
                let child = scan_node(&entry.path(), state)?;
                size += child.size;
                children.push(child);
            }
        }
        let node = Node {
            kind: (state.classify)(path, true),
            name,
            path: path.to_path_buf(),
            is_dir: true,
            size,
            children,
        };
        state.tick();
        Some(node)
    } else {
        // A symlink and a plain file are both leaves here: neither is
        // opened any further, so each is sized by what its own directory
        // entry reports, never by chasing a link to its target.
        let size = meta.len();
        state.total.files += 1;
        state.total.bytes += size;
        let node = Node {
            kind: (state.classify)(path, false),
            name,
            path: path.to_path_buf(),
            is_dir: false,
            size,
            children: Vec::new(),
        };
        state.tick();
        Some(node)
    }
}

/// The squarified treemap layout of `sizes` inside `rect`, one output rect
/// per input, in the same order as `sizes`. Bruls, Huizing & van Wijk
/// (2000): lay children out in rows along the rect's shorter side, closing a
/// row the moment adding the next child would make the row's worst aspect
/// ratio worse rather than better. That local greedy rule is what keeps
/// treemap cells close to square instead of degenerating into the thin
/// slivers a naive slice-and-dice layout produces.
pub fn squarify(sizes: &[u64], rect: Rect) -> Vec<Rect> {
    let n = sizes.len();
    let mut out = vec![
        Rect {
            x: rect.x,
            y: rect.y,
            w: 0.0,
            h: 0.0
        };
        n
    ];
    if n == 0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return out;
    }
    let total: f64 = sizes.iter().map(|&s| s as f64).sum();
    if total <= 0.0 {
        // Every input is zero: every output stays the zero-area rect `out`
        // was already filled with, and there is nothing to lay out.
        return out;
    }
    // The layout math below divides by a row's own thickness, which is
    // zero for a zero-size item. Rather than guard every division, the
    // zero entries are filtered out up front and left as the zero rects
    // `out` already holds; only the strictly positive sizes go through the
    // real algorithm, sorted descending as it requires.
    let mut order: Vec<usize> = (0..n).filter(|&i| sizes[i] > 0).collect();
    order.sort_unstable_by(|&a, &b| sizes[b].cmp(&sizes[a]));
    // Scale byte counts to areas that sum exactly to the container's area —
    // this is what makes every output rect's area proportional to its size.
    let scale = rect.area() / total;
    let scaled: Vec<f64> = order.iter().map(|&i| sizes[i] as f64 * scale).collect();
    let placed = squarify_rows(&scaled, rect);
    for (slot, &original) in order.iter().enumerate() {
        out[original] = placed[slot];
    }
    out
}

/// The core algorithm on pre-scaled, strictly positive, descending-sorted
/// areas. Returns one rect per input, in input order.
fn squarify_rows(areas: &[f64], mut rect: Rect) -> Vec<Rect> {
    let mut out = Vec::with_capacity(areas.len());
    let mut start = 0;
    while start < areas.len() {
        if rect.w <= 0.0 || rect.h <= 0.0 {
            // Floating-point drift can shave the leftover rect down to
            // nothing a touch early; whatever is left just becomes
            // zero-area rects instead of a division by zero.
            for _ in start..areas.len() {
                out.push(Rect {
                    x: rect.x,
                    y: rect.y,
                    w: 0.0,
                    h: 0.0,
                });
            }
            break;
        }
        // Grow the row one item at a time for as long as doing so does not
        // make its worst aspect ratio worse — the "squarified" rule.
        let mut end = start + 1;
        while end < areas.len() {
            let current = worst_ratio(&areas[start..end], rect);
            let grown = worst_ratio(&areas[start..end + 1], rect);
            if grown <= current {
                end += 1;
            } else {
                break;
            }
        }
        let row = &areas[start..end];
        out.extend(lay_out_row(row, rect));
        rect = leftover(row, rect);
        start = end;
    }
    out
}

/// One row's rects: a strip spanning the rect's shorter side, subdivided
/// among `areas` in proportion to their size, with the strip's thickness
/// along the longer side set so the strip's total area equals `sum(areas)`.
fn lay_out_row(areas: &[f64], rect: Rect) -> Vec<Rect> {
    if rect.w >= rect.h {
        lay_out_row_stacked(areas, rect)
    } else {
        lay_out_row_flowed(areas, rect)
    }
}

/// The rect is at least as wide as it is tall, so the row becomes a
/// vertical strip at the left edge, itself subdivided top to bottom.
fn lay_out_row_stacked(areas: &[f64], rect: Rect) -> Vec<Rect> {
    let covered: f64 = areas.iter().sum();
    let width = if rect.h > 0.0 { covered / rect.h } else { 0.0 };
    let mut y = rect.y;
    let mut out = Vec::with_capacity(areas.len());
    for &a in areas {
        let h = if width > 0.0 { a / width } else { 0.0 };
        out.push(Rect { x: rect.x, y, w: width, h });
        y += h;
    }
    out
}

/// The rect is taller than it is wide, so the row becomes a horizontal
/// strip at the top edge, itself subdivided left to right.
fn lay_out_row_flowed(areas: &[f64], rect: Rect) -> Vec<Rect> {
    let covered: f64 = areas.iter().sum();
    let height = if rect.w > 0.0 { covered / rect.w } else { 0.0 };
    let mut x = rect.x;
    let mut out = Vec::with_capacity(areas.len());
    for &a in areas {
        let w = if height > 0.0 { a / height } else { 0.0 };
        out.push(Rect { x, y: rect.y, w, h: height });
        x += w;
    }
    out
}

/// What remains of `rect` after placing a row for `areas`: the same rect
/// with the row's strip removed from whichever side it occupied. Clamped to
/// zero rather than left to go slightly negative under floating-point
/// rounding, so the next iteration's "is this rect degenerate" check is
/// exact instead of an epsilon comparison.
fn leftover(areas: &[f64], rect: Rect) -> Rect {
    let covered: f64 = areas.iter().sum();
    if rect.w >= rect.h {
        let width = if rect.h > 0.0 { covered / rect.h } else { 0.0 };
        Rect {
            x: rect.x + width,
            y: rect.y,
            w: (rect.w - width).max(0.0),
            h: rect.h,
        }
    } else {
        let height = if rect.w > 0.0 { covered / rect.w } else { 0.0 };
        Rect {
            x: rect.x,
            y: rect.y + height,
            w: rect.w,
            h: (rect.h - height).max(0.0),
        }
    }
}

/// The worst (largest) aspect ratio among the rects a row of `areas` would
/// produce in `rect` — the number [`squarify_rows`] compares before and
/// after adding one more item, to decide whether the row should keep
/// growing or close.
fn worst_ratio(areas: &[f64], rect: Rect) -> f64 {
    lay_out_row(areas, rect)
        .iter()
        .map(|r| {
            if r.w <= 0.0 || r.h <= 0.0 {
                f64::INFINITY
            } else {
                (r.w / r.h).max(r.h / r.w)
            }
        })
        .fold(0.0_f64, f64::max)
}

/// One drawable rectangle of the finished map — a folder or a file, already
/// positioned, with nothing left for a view to compute except paint it.
#[derive(Clone, Debug)]
pub struct Cell {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
    pub kind: u8,
    /// 0 for the current folder's own children, 1 for their children, and
    /// so on — how many group borders separate this cell from the root.
    pub depth: usize,
    pub rect: Rect,
    /// True when this cell is a folder drawn as a bordered group whose
    /// children are also present in the output (i.e. `depth < max_depth`);
    /// false for a file, and false for a folder deep enough that it is
    /// drawn as one flat rectangle instead of being opened up.
    pub is_group: bool,
}

/// Flatten `node`'s children into drawable cells inside `area`.
///
/// A folder shallower than `max_depth` becomes a bordered group: its own
/// cell is emitted first, then its children are squarified again inside the
/// space left after subtracting `group_inset` from every edge and an extra
/// `header` strip from the top — room for a view to print the folder's name
/// and size above its contents. A folder at `max_depth` or deeper is drawn
/// as one flat rectangle instead; opening it up further would only draw
/// borders too thin to mean anything.
///
/// A rect smaller than `min_side` on either edge is dropped entirely — group
/// and children alike, since a group too small to show its own header is
/// not worth opening up either. A map is a picture, not a list of
/// rectangles too small to see.
///
/// The output is in painter's order: a group's own cell always comes before
/// its children, so a caller drawing the vector front-to-back gets children
/// on top of their group for free, with no separate z-ordering step needed.
pub fn layout(
    node: &Node,
    area: Rect,
    max_depth: usize,
    group_inset: f64,
    header: f64,
    min_side: f64,
) -> Vec<Cell> {
    let mut out = Vec::new();
    layout_children(
        &node.children,
        area,
        0,
        max_depth,
        group_inset,
        header,
        min_side,
        &mut out,
    );
    out
}

#[allow(clippy::too_many_arguments)]
fn layout_children(
    children: &[Node],
    area: Rect,
    depth: usize,
    max_depth: usize,
    group_inset: f64,
    header: f64,
    min_side: f64,
    out: &mut Vec<Cell>,
) {
    if children.is_empty() || area.w <= 0.0 || area.h <= 0.0 {
        return;
    }
    let sizes: Vec<u64> = children.iter().map(|c| c.size).collect();
    let rects = squarify(&sizes, area);
    for (child, rect) in children.iter().zip(rects) {
        if rect.w < min_side || rect.h < min_side {
            // Invisible at this scale: drawing it would just be a sliver,
            // and if it is a folder its children would be smaller still.
            continue;
        }
        let is_group = child.is_dir && depth < max_depth;
        out.push(Cell {
            path: child.path.clone(),
            name: child.name.clone(),
            size: child.size,
            is_dir: child.is_dir,
            kind: child.kind,
            depth,
            rect,
            is_group,
        });
        if is_group {
            let inner = Rect {
                x: rect.x + group_inset,
                y: rect.y + group_inset + header,
                w: rect.w - 2.0 * group_inset,
                h: rect.h - 2.0 * group_inset - header,
            };
            // A negative or zero `inner` (the inset plus header ate the
            // whole rect) is caught by `layout_children`'s own guard above,
            // so nothing special is needed here beyond just recursing.
            layout_children(
                &child.children,
                inner,
                depth + 1,
                max_depth,
                group_inset,
                header,
                min_side,
                out,
            );
        }
    }
}

/// The deepest (i.e. last in painter's order) cell containing the point —
/// the one a mouse at that point is actually pointing at, not whichever
/// group happens to sit behind it.
pub fn hit(cells: &[Cell], x: f64, y: f64) -> Option<usize> {
    cells.iter().rposition(|c| c.rect.contains(x, y))
}

/// A byte count the way a tooltip reads it — "1.5 KB", "15 KB", "2.5 GB" —
/// using exactly the app's own 1000-based rounding (see `format_size` in
/// `model.rs`), because the same number should never look different just
/// for being shown in a different view of the same file.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else if value >= 10.0 {
        format!("{:.0} {}", value, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(name: &str, size: u64) -> Node {
        Node {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir: false,
            size,
            kind: 0,
            children: Vec::new(),
        }
    }

    fn dir(name: &str, children: Vec<Node>) -> Node {
        let size = children.iter().map(|c| c.size).sum();
        Node {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir: true,
            size,
            kind: 0,
            children,
        }
    }

    fn overlap_area(a: Rect, b: Rect) -> f64 {
        let x_overlap = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
        let y_overlap = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
        if x_overlap > 1e-6 && y_overlap > 1e-6 {
            x_overlap * y_overlap
        } else {
            0.0
        }
    }

    fn assert_no_overlaps(rects: &[Rect]) {
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let overlap = overlap_area(rects[i], rects[j]);
                assert!(
                    overlap < 1e-6,
                    "rects {} and {} overlap by {} ({:?} vs {:?})",
                    i,
                    j,
                    overlap,
                    rects[i],
                    rects[j]
                );
            }
        }
    }

    fn assert_inside(rect: Rect, container: Rect) {
        assert!(rect.x >= container.x - 1e-6);
        assert!(rect.y >= container.y - 1e-6);
        assert!(rect.x + rect.w <= container.x + container.w + 1e-6);
        assert!(rect.y + rect.h <= container.y + container.h + 1e-6);
    }

    // The exact example from Bruls, Huizing & van Wijk (2000): sizes that
    // sum to the container's area, so proportionality is easy to check by
    // hand as well as by assertion.
    #[test]
    fn squarify_known_case_is_valid() {
        let sizes = [6u64, 6, 4, 3, 2, 2, 1];
        let rect = Rect { x: 0.0, y: 0.0, w: 6.0, h: 4.0 };
        let rects = squarify(&sizes, rect);
        assert_eq!(rects.len(), sizes.len());
        let total: f64 = sizes.iter().map(|&s| s as f64).sum();
        for (i, r) in rects.iter().enumerate() {
            assert_inside(*r, rect);
            let expected = sizes[i] as f64 / total * rect.area();
            assert!(
                (r.area() - expected).abs() < 1e-6,
                "size {} -> area {} but expected {}",
                sizes[i],
                r.area(),
                expected
            );
        }
        assert_no_overlaps(&rects);
        let sum_areas: f64 = rects.iter().map(Rect::area).sum();
        assert!((sum_areas - rect.area()).abs() < 1e-6);
    }

    // The property that makes this a *squarified* treemap rather than a
    // slice-and-dice strip: equal-weight items come out close to square,
    // not as sixteen slivers running the length of the rect.
    #[test]
    fn squarify_keeps_cells_reasonably_square() {
        let sizes = [100u64; 16];
        let rect = Rect { x: 0.0, y: 0.0, w: 400.0, h: 300.0 };
        let rects = squarify(&sizes, rect);
        for r in &rects {
            let aspect = (r.w / r.h).max(r.h / r.w);
            assert!(aspect < 2.0, "aspect {} too extreme for {:?}", aspect, r);
        }
    }

    #[test]
    fn squarify_edge_cases() {
        let rect = Rect { x: 1.0, y: 2.0, w: 10.0, h: 5.0 };

        // Empty input gives empty output.
        assert!(squarify(&[], rect).is_empty());

        // A single item fills the whole rect exactly.
        let single = squarify(&[42], rect);
        assert_eq!(single.len(), 1);
        assert!((single[0].x - rect.x).abs() < 1e-9);
        assert!((single[0].y - rect.y).abs() < 1e-9);
        assert!((single[0].w - rect.w).abs() < 1e-9);
        assert!((single[0].h - rect.h).abs() < 1e-9);

        // All zero: every rect is zero-area, nothing panics or produces NaN.
        let zeros = squarify(&[0, 0, 0], rect);
        assert_eq!(zeros.len(), 3);
        for r in &zeros {
            assert_eq!(r.area(), 0.0);
            assert!(!r.w.is_nan() && !r.h.is_nan());
        }

        // Mixed zero and non-zero: the zero entries get zero area, the
        // rest still accounts for the whole rect between them.
        let mixed = squarify(&[10, 0, 5, 0], rect);
        assert_eq!(mixed.len(), 4);
        assert_eq!(mixed[1].area(), 0.0);
        assert_eq!(mixed[3].area(), 0.0);
        assert!(mixed[0].area() > 0.0 && mixed[2].area() > 0.0);
        let sum: f64 = mixed.iter().map(Rect::area).sum();
        assert!((sum - rect.area()).abs() < 1e-6);
        for r in &mixed {
            assert!(!r.w.is_nan() && !r.h.is_nan());
        }
    }

    #[test]
    fn layout_paints_groups_before_their_children() {
        let tree = dir(
            "root",
            vec![
                dir("sub", vec![leaf("a.txt", 100), leaf("b.txt", 200)]),
                leaf("c.txt", 50),
            ],
        );
        let area = Rect { x: 0.0, y: 0.0, w: 200.0, h: 100.0 };
        let cells = layout(&tree, area, 4, 2.0, 8.0, 1.0);

        let sub_index = cells.iter().position(|c| c.name == "sub").unwrap();
        let a_index = cells.iter().position(|c| c.name == "a.txt").unwrap();
        let b_index = cells.iter().position(|c| c.name == "b.txt").unwrap();
        assert!(sub_index < a_index);
        assert!(sub_index < b_index);
        assert_eq!(cells[sub_index].depth, 0);
        assert_eq!(cells[a_index].depth, 1);
        assert_eq!(cells[b_index].depth, 1);
        assert!(cells[sub_index].is_group);
        assert!(!cells[a_index].is_group);
    }

    #[test]
    fn layout_drops_slivers_below_min_side() {
        let tree = dir("root", vec![leaf("big.bin", 1_000_000), leaf("tiny.bin", 1)]);
        let area = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 1000.0 };
        let cells = layout(&tree, area, 4, 0.0, 0.0, 4.0);
        assert!(cells.iter().any(|c| c.name == "big.bin"));
        assert!(!cells.iter().any(|c| c.name == "tiny.bin"));
    }

    #[test]
    fn hit_finds_the_deepest_cell() {
        let tree = dir("root", vec![dir("sub", vec![leaf("a.txt", 100)])]);
        let area = Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let cells = layout(&tree, area, 4, 2.0, 5.0, 0.0);

        let group = cells.iter().position(|c| c.name == "sub").unwrap();
        let child = cells.iter().position(|c| c.name == "a.txt").unwrap();
        let group_rect = cells[group].rect;
        let child_rect = cells[child].rect;

        // A point in the group's margin (inside the border/header inset,
        // before the child's own rect begins) should hit the group.
        assert_eq!(hit(&cells, group_rect.x + 0.5, group_rect.y + 0.5), Some(group));

        // A point solidly inside the child should hit the child, not the
        // group sitting behind it, even though both rects contain it.
        let cx = child_rect.x + child_rect.w / 2.0;
        let cy = child_rect.y + child_rect.h / 2.0;
        assert!(group_rect.contains(cx, cy), "test setup: child not nested in group");
        assert_eq!(hit(&cells, cx, cy), Some(child));
    }

    #[test]
    fn scan_rolls_up_recursive_sizes_and_stops_symlink_cycles() {
        let root = std::env::temp_dir().join(format!("mpfiles-treemap-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub/subsub")).unwrap();
        fs::write(root.join("a.txt"), b"aaaaa").unwrap(); // 5 bytes
        fs::write(root.join("sub/b.txt"), b"bbbbbbb").unwrap(); // 7 bytes
        fs::write(root.join("sub/subsub/c.txt"), b"ccc").unwrap(); // 3 bytes

        #[cfg(unix)]
        let link_len: u64 = {
            // A link back up to an ancestor of the folder it sits in: if the
            // scan ever followed it, this test would hang rather than
            // finish, which is exactly the bug this case exists to catch.
            let link = root.join("sub/subsub/loop");
            std::os::unix::fs::symlink(&root, &link).unwrap();
            fs::symlink_metadata(&link).unwrap().len()
        };
        #[cfg(not(unix))]
        let link_len: u64 = 0;

        let cancel = AtomicBool::new(false);
        let node = scan(&root, &|_, _| 0u8, &cancel, &|_| {}).expect("scan should complete");

        let leaf_bytes = 5 + 7 + 3;
        assert_eq!(node.size, leaf_bytes + link_len);

        let expected_count = if cfg!(unix) { 7 } else { 6 };
        assert_eq!(node.count(), expected_count);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_returns_none_when_already_cancelled() {
        let root =
            std::env::temp_dir().join(format!("mpfiles-treemap-test-cancel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let cancel = AtomicBool::new(true);
        let result = scan(&root, &|_, _| 0u8, &cancel, &|_| {});
        assert!(result.is_none());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_reports_progress_at_a_bounded_rate_not_per_entry() {
        let root = std::env::temp_dir()
            .join(format!("mpfiles-treemap-test-progress-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for i in 0..600 {
            fs::write(root.join(format!("f{i}.bin")), b"x").unwrap();
        }

        let cancel = AtomicBool::new(false);
        let calls = std::sync::atomic::AtomicU32::new(0);
        let node = scan(&root, &|_, _| 0u8, &cancel, &|_| {
            calls.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();

        assert_eq!(node.count(), 601); // root + 600 files
        // 600 entries at a stride of 512 is at most two mid-walk reports
        // plus the guaranteed final one — nowhere near one call per file.
        assert!(calls.load(Ordering::Relaxed) <= 4, "too many progress calls: {}", calls.load(Ordering::Relaxed));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn formats_byte_counts() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1500), "1.5 KB");
        assert_eq!(format_bytes(15_000), "15 KB");
        assert_eq!(format_bytes(2_500_000_000), "2.5 GB");
    }
}
