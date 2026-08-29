//! The treemap: a spatial map of where a folder's bytes actually are.
//!
//! Every file is a rectangle whose area is its size, so a 4 GB video is
//! visibly four thousand times the block a 1 MB photo gets. Folders are
//! bordered groups two levels deep, then flat — deeper than that the
//! rectangles are smaller than their own borders and the picture stops saying
//! anything.
//!
//! The scan is the expensive part and it never runs on the UI thread: a worker
//! walks the tree, reports its progress as a real fraction of the top-level
//! children it has finished, and can be cancelled the instant the user
//! navigates away. The layout itself ([`crate::treemap`]) is pure arithmetic
//! and runs inline on resize.

use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;

use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    thread,
};

use crate::{
    model::{kind_for, FileKind},
    theme::Palette,
    treemap::{self, Cell, Node, Rect as MapRect, ScanProgress},
};

/// How deep folders keep their own bordered group. Past this a folder is one
/// flat rectangle: the picture is about *where the bytes are*, and a third
/// level of 2px borders is all border and no bytes.
const GROUP_DEPTH: usize = 2;
/// Border thickness of a group, and the strip its name gets.
const GROUP_INSET: f64 = 2.0;
const GROUP_HEADER: f64 = 13.0;
/// Rectangles thinner than this on either edge are dropped from the layout.
const MIN_SIDE: f64 = 3.0;
/// A rectangle needs at least this much room before its name is worth drawing.
const LABEL_MIN: DVec2 = DVec2 { x: 46.0, y: 14.0 };

script_mod! {
    use mod.prelude.widgets.*

    mod.widgets.MpfTreemapBase = #(TreemapView::register_widget(vm))
    mod.widgets.MpfTreemap = set_type_default() do mod.widgets.MpfTreemapBase{
        width: Fill
        height: Fill
        draw_bg +: {color: mod.mpf.bg}
        draw_text +: {
            color: mod.mpf.bg_dark
            text_style: theme.font_regular{font_size: 8.0}
        }
    }
}

/// What a click on the map means to the folder view around it.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum TreemapAction {
    /// A rectangle was clicked; reveal it in the breadcrumb and select it.
    Selected(PathBuf),
    /// A folder was double-clicked; the map should re-scan scoped to it.
    Drill(PathBuf),
    #[default]
    None,
}

/// The kind class a file's color comes from: the index into
/// [`Palette::kinds`]. Kept here rather than on `FileKind` because it is a
/// property of *this picture*, not of the file.
pub fn kind_class(kind: FileKind) -> u8 {
    match kind {
        FileKind::Video => 0,
        FileKind::Image => 1,
        FileKind::Audio => 2,
        FileKind::Code => 3,
        // A PDF reads as a document, which is what the text hue means here.
        FileKind::Text | FileKind::Pdf => 4,
        FileKind::Archive => 5,
        FileKind::Folder | FileKind::Generic => 6,
    }
}

/// One message from the scan worker. `generation` is the request it answers,
/// so a scan the user already navigated away from is dropped rather than
/// painted over the folder they are looking at now.
struct ScanUpdate {
    generation: u64,
    fraction: f64,
    progress: ScanProgress,
    /// `Some(None)` = the scan was cancelled or the folder is unreadable.
    done: Option<Option<Box<Node>>>,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TreemapView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawColor,
    #[live]
    draw_text: DrawText,
    #[rust]
    area: Area,

    /// The folder the map is of — the browser's folder, or a folder the user
    /// drilled into.
    #[rust]
    root: PathBuf,
    #[rust]
    node: Option<Node>,
    #[rust]
    cells: Vec<Cell>,
    /// The rect `cells` was laid out for; a different one means re-layout.
    #[rust]
    laid_out: Rect,

    #[rust]
    generation: u64,
    #[rust]
    cancel: Option<Arc<AtomicBool>>,
    #[rust]
    scanning: bool,
    #[rust]
    fraction: f64,
    #[rust]
    progress: ScanProgress,
    #[rust]
    error: Option<String>,

    #[rust]
    sender: Option<Sender<ScanUpdate>>,
    #[rust]
    receiver: Option<Receiver<ScanUpdate>>,

    #[rust]
    hover: Option<usize>,
    #[rust]
    hover_at: DVec2,
    #[rust]
    selected: Option<PathBuf>,
}

impl TreemapView {
    /// The folder the map is currently of.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Scan `path` and map it. A scan already running for another folder is
    /// cancelled first — the user asked for this folder, not that one.
    pub fn set_root(&mut self, cx: &mut Cx, path: &Path) {
        if self.sender.is_none() {
            let (sender, receiver) = channel();
            self.sender = Some(sender);
            self.receiver = Some(receiver);
        }
        self.stop(cx);
        self.root = path.to_path_buf();
        self.node = None;
        self.cells.clear();
        self.laid_out = Rect::default();
        self.hover = None;
        self.error = None;
        self.fraction = 0.0;
        self.progress = ScanProgress::default();
        self.scanning = true;

        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let Some(sender) = self.sender.clone() else {
            return;
        };
        let root = self.root.clone();
        thread::spawn(move || {
            let report = |fraction: f64, progress: ScanProgress| {
                let _ = sender.send(ScanUpdate {
                    generation,
                    fraction,
                    progress,
                    done: None,
                });
                SignalToUI::set_ui_signal();
            };
            let node = scan_by_child(&root, &cancel, &report);
            let _ = sender.send(ScanUpdate {
                generation,
                fraction: 1.0,
                progress: ScanProgress::default(),
                done: Some(node.map(Box::new)),
            });
            SignalToUI::set_ui_signal();
        });
        self.redraw(cx);
    }

    /// Stop whatever scan is running. Called when the view is left, when the
    /// folder changes, and when the window goes away.
    pub fn stop(&mut self, cx: &mut Cx) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        if self.scanning {
            self.scanning = false;
            self.redraw(cx);
        }
    }

    /// The status line for the map: what it is showing, or how far the scan
    /// has got.
    pub fn status(&self) -> String {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if self.scanning {
            return format!(
                "Scanning {} — {:.0}% · {} files · {}",
                crate::model::display_name(&self.root),
                self.fraction * 100.0,
                self.progress.files,
                treemap::format_bytes(self.progress.bytes)
            );
        }
        match &self.node {
            Some(node) => format!(
                "{} — {} in {} files · double-click a folder to drill in, Backspace to go up",
                crate::model::display_name(&self.root),
                treemap::format_bytes(node.size),
                node.count().saturating_sub(1),
            ),
            None => "Nothing to map".to_string(),
        }
    }

    /// Take everything the worker sent. True when the view needs a redraw.
    pub fn drain(&mut self, cx: &mut Cx) -> bool {
        let updates: Vec<ScanUpdate> = self
            .receiver
            .as_ref()
            .map(|r| r.try_iter().collect())
            .unwrap_or_default();
        let mut dirty = false;
        for update in updates {
            if update.generation != self.generation {
                continue;
            }
            self.fraction = update.fraction;
            if update.done.is_none() {
                self.progress = update.progress;
                dirty = true;
                continue;
            }
            self.scanning = false;
            self.cancel = None;
            match update.done.flatten() {
                Some(node) => {
                    self.progress = ScanProgress {
                        files: node.count().saturating_sub(1) as u64,
                        bytes: node.size,
                    };
                    self.node = Some(*node);
                    // A finished scan always re-lays out, whatever the size.
                    self.laid_out = Rect::default();
                }
                None => {
                    self.error = Some(format!(
                        "Could not map {}",
                        crate::model::display_name(&self.root)
                    ));
                }
            }
            dirty = true;
        }
        if dirty {
            self.redraw(cx);
        }
        dirty
    }

    /// Which path is highlighted on the map.
    pub fn set_selected(&mut self, cx: &mut Cx, path: Option<PathBuf>) {
        if self.selected != path {
            self.selected = path;
            self.redraw(cx);
        }
    }

    fn relayout(&mut self, rect: Rect) {
        let Some(node) = &self.node else {
            self.cells.clear();
            return;
        };
        self.laid_out = rect;
        self.cells = treemap::layout(
            node,
            MapRect {
                x: rect.pos.x,
                y: rect.pos.y,
                w: rect.size.x,
                h: rect.size.y,
            },
            GROUP_DEPTH,
            GROUP_INSET,
            GROUP_HEADER,
            MIN_SIDE,
        );
        self.hover = None;
    }

    /// The cell under a window point, if any.
    fn hit_cell(&self, pos: DVec2) -> Option<usize> {
        treemap::hit(&self.cells, pos.x, pos.y)
    }

    /// The file or folder under a window point — what a right-click there is
    /// about.
    pub fn path_at(&self, pos: DVec2) -> Option<PathBuf> {
        self.hit_cell(pos).map(|i| self.cells[i].path.clone())
    }
}

impl Widget for TreemapView {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle(walk);
        self.draw_bg.color = Palette::vec4(&Palette::shared().bg);
        self.draw_bg.draw_abs(cx, rect);
        if rect.size.x < 8.0 || rect.size.y < 8.0 {
            cx.add_aligned_rect_area(&mut self.area, rect);
            return DrawStep::done();
        }

        let palette = Palette::shared();
        self.draw_text.text_style.font_size = 8.0;

        if self.scanning || self.node.is_none() {
            // Nothing to paint yet: say what is happening where the map will
            // be, rather than leaving a blank panel.
            self.draw_text.color = Palette::vec4(&palette.fg_dim);
            let text = if self.scanning {
                format!(
                    "Scanning… {:.0}%   {} files   {}",
                    self.fraction * 100.0,
                    self.progress.files,
                    treemap::format_bytes(self.progress.bytes)
                )
            } else {
                self.error.clone().unwrap_or_else(|| "Nothing to map".into())
            };
            self.draw_text.draw_abs(
                cx,
                dvec2(rect.pos.x + 20.0, rect.pos.y + 20.0),
                &text,
            );
            cx.add_aligned_rect_area(&mut self.area, rect);
            return DrawStep::done();
        }

        let map_rect = Rect {
            pos: rect.pos + dvec2(1.0, 1.0),
            size: rect.size - dvec2(2.0, 2.0),
        };
        if self.laid_out != map_rect {
            self.relayout(map_rect);
        }

        let border = Palette::vec4(&palette.bg_dark);
        let accent = Palette::vec4(&palette.accent);
        let bright = Palette::vec4(&palette.fg_bright);
        cx.push_clip_rect(rect);
        for (index, cell) in self.cells.iter().enumerate() {
            let r = Rect {
                pos: dvec2(cell.rect.x, cell.rect.y),
                size: dvec2(cell.rect.w, cell.rect.h),
            };
            // Every rectangle is drawn as a border-colored plate with its fill
            // inset by one point: that single point of background *is* the
            // grid line between neighbours, for free.
            let selected = self.selected.as_deref() == Some(cell.path.as_path());
            self.draw_bg.color = if selected {
                accent
            } else if Some(index) == self.hover {
                bright
            } else {
                border
            };
            self.draw_bg.draw_abs(cx, r);
            if r.size.x <= 2.0 || r.size.y <= 2.0 {
                continue;
            }
            let inner = Rect {
                pos: r.pos + dvec2(1.0, 1.0),
                size: r.size - dvec2(2.0, 2.0),
            };
            let mut fill = palette.kind_color(kind_class(cell_kind(cell)) as usize);
            if cell.is_group {
                // A group is the plate its children sit on: darker, so the
                // files inside it read as the bright things.
                fill = Vec4f {
                    x: fill.x * 0.32,
                    y: fill.y * 0.32,
                    z: fill.z * 0.34,
                    w: 1.0,
                };
            }
            self.draw_bg.color = fill;
            self.draw_bg.draw_abs(cx, inner);

            // The name goes on when there is room for it — on a group that is
            // its header strip, on a leaf the middle of the block.
            if inner.size.x >= LABEL_MIN.x
                && inner.size.y >= LABEL_MIN.y
                && (cell.depth == 0 || cell.is_group)
            {
                self.draw_text.color = if cell.is_group {
                    bright
                } else {
                    Palette::vec4(&palette.bg_dark)
                };
                cx.push_clip_rect(Rect {
                    pos: inner.pos,
                    size: inner.size - dvec2(3.0, 0.0),
                });
                self.draw_text.draw_abs(
                    cx,
                    inner.pos + dvec2(4.0, if cell.is_group { 2.0 } else { 3.0 }),
                    &cell.name,
                );
                cx.pop_clip_rect();
            }
        }
        cx.pop_clip_rect();

        // The tooltip is the map's only text that has to be readable at any
        // size, so it paints last, over everything.
        if let Some(cell) = self.hover.and_then(|i| self.cells.get(i)) {
            let total = self.node.as_ref().map(|n| n.size).unwrap_or(0).max(1);
            // The path is shown relative to the folder being mapped: the map is
            // already *of* that folder, and its absolute path would be most of
            // the tooltip and none of the information.
            let relative = cell
                .path
                .strip_prefix(&self.root)
                .unwrap_or(&cell.path)
                .display()
                .to_string();
            let lines = [
                relative,
                format!(
                    "{} · {:.1}% of {}",
                    treemap::format_bytes(cell.size),
                    cell.size as f64 * 100.0 / total as f64,
                    treemap::format_bytes(total)
                ),
            ];
            let width = lines
                .iter()
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0) as f64
                * 4.9
                + 16.0;
            let size = dvec2(width.min(rect.size.x - 8.0), 32.0);
            let pos = dvec2(
                (self.hover_at.x + 14.0).min(rect.pos.x + rect.size.x - size.x - 4.0),
                (self.hover_at.y + 16.0).min(rect.pos.y + rect.size.y - size.y - 4.0),
            );
            self.draw_bg.color = Palette::vec4(&palette.fg_bright);
            self.draw_bg.draw_abs(cx, Rect { pos, size });
            self.draw_bg.color = Palette::vec4(&palette.bg_dark);
            self.draw_bg.draw_abs(cx, Rect {
                pos: pos + dvec2(1.0, 1.0),
                size: size - dvec2(2.0, 2.0),
            });
            // Clip to the box: a long name must not run out of the tooltip
            // and across the map behind it.
            cx.push_clip_rect(Rect {
                pos,
                size: size - dvec2(4.0, 0.0),
            });
            self.draw_text.color = bright;
            self.draw_text.draw_abs(cx, pos + dvec2(7.0, 5.0), &lines[0]);
            self.draw_text.color = Palette::vec4(&palette.fg_dim);
            self.draw_text.draw_abs(cx, pos + dvec2(7.0, 17.0), &lines[1]);
            cx.pop_clip_rect();
        }

        cx.add_aligned_rect_area(&mut self.area, rect);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.area) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                cx.set_cursor(MouseCursor::Arrow);
                let hover = self.hit_cell(e.abs);
                if hover != self.hover || hover.is_some() {
                    self.hover = hover;
                    self.hover_at = e.abs;
                    self.redraw(cx);
                }
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.redraw(cx);
                }
            }
            Hit::FingerDown(e) => {
                let Some(index) = self.hit_cell(e.abs) else {
                    return;
                };
                let cell = self.cells[index].clone();
                self.selected = Some(cell.path.clone());
                self.redraw(cx);
                if e.tap_count >= 2 && cell.is_dir {
                    cx.widget_action(self.uid, TreemapAction::Drill(cell.path));
                } else {
                    cx.widget_action(self.uid, TreemapAction::Selected(cell.path));
                }
            }
            _ => {}
        }
    }
}

impl TreemapViewRef {
    pub fn set_root(&self, cx: &mut Cx, path: &Path) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_root(cx, path);
        }
    }

    pub fn stop(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.stop(cx);
        }
    }

    pub fn drain(&self, cx: &mut Cx) -> bool {
        self.borrow_mut().map(|mut i| i.drain(cx)).unwrap_or(false)
    }

    pub fn status(&self) -> String {
        self.borrow().map(|i| i.status()).unwrap_or_default()
    }

    pub fn root(&self) -> PathBuf {
        self.borrow().map(|i| i.root().to_path_buf()).unwrap_or_default()
    }

    pub fn set_selected(&self, cx: &mut Cx, path: Option<PathBuf>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_selected(cx, path);
        }
    }

    pub fn path_at(&self, pos: DVec2) -> Option<PathBuf> {
        self.borrow().and_then(|i| i.path_at(pos))
    }

    /// The map's action out of an event batch.
    pub fn action(&self, actions: &Actions) -> TreemapAction {
        let uid = self.widget_uid();
        actions
            .iter()
            .filter_map(|a| a.as_widget_action().filter(|wa| wa.widget_uid == uid))
            .map(|wa| wa.cast::<TreemapAction>())
            .find(|a| *a != TreemapAction::None)
            .unwrap_or(TreemapAction::None)
    }
}

/// Walk `root` one top-level child at a time so the progress the user sees is
/// a real fraction — "62% of this folder's 34 entries", not a number of files
/// with no denominator. The per-child scans are the module's own.
fn scan_by_child(
    root: &Path,
    cancel: &AtomicBool,
    report: &dyn Fn(f64, ScanProgress),
) -> Option<Node> {
    let classify = |path: &Path, is_dir: bool| kind_for(path, is_dir) as u8;
    let entries = crate::vfs::vfs().read_dir(root, true).ok()?;
    let total = entries.len().max(1);
    let mut children = Vec::with_capacity(entries.len());
    let mut running = ScanProgress::default();
    for (index, entry) in entries.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let path = entry.path.clone();
        let child = if entry.is_dir {
            let before = running;
            crate::vfs::vfs().scan(&path, cancel, &|p: ScanProgress| {
                report(
                    (index as f64 + 0.5) / total as f64,
                    ScanProgress {
                        files: before.files + p.files,
                        bytes: before.bytes + p.bytes,
                    },
                );
            })?
        } else {
            Node {
                name: entry.name.clone(),
                is_dir: false,
                size: entry.size,
                kind: classify(&path, false),
                children: Vec::new(),
                path,
            }
        };
        running.files += child.count() as u64;
        running.bytes += child.size;
        children.push(child);
        report((index as f64 + 1.0) / total as f64, running);
    }
    Some(Node {
        name: crate::model::display_name(root),
        path: root.to_path_buf(),
        is_dir: true,
        size: children.iter().map(|c| c.size).sum(),
        kind: classify(root, true),
        children,
    })
}

/// [`FileKind`] in discriminant order, so the opaque `u8` the scan carried —
/// which for this app is a `FileKind` discriminant — can be read back. A
/// change to the enum's order breaks this, which is what the test below is
/// for.
const FILE_KINDS: [FileKind; 9] = [
    FileKind::Folder,
    FileKind::Image,
    FileKind::Text,
    FileKind::Code,
    FileKind::Audio,
    FileKind::Video,
    FileKind::Archive,
    FileKind::Pdf,
    FileKind::Generic,
];

/// The kind a cell paints as.
fn cell_kind(cell: &Cell) -> FileKind {
    FILE_KINDS
        .get(cell.kind as usize)
        .copied()
        .unwrap_or(FileKind::Generic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kind_tag_survives_the_round_trip_through_a_byte() {
        // The scan stores a FileKind as its discriminant; FILE_KINDS turns it
        // back. If the enum ever gains a variant in the middle, this fails
        // before the map starts painting videos as archives.
        for (index, kind) in FILE_KINDS.iter().enumerate() {
            assert_eq!(*kind as usize, index, "{kind:?}");
        }
    }

    #[test]
    fn every_kind_lands_on_a_palette_class() {
        let palette = Palette::tokyo_night();
        for kind in FILE_KINDS {
            let class = kind_class(kind) as usize;
            assert!(class < palette.kinds.len(), "{kind:?} -> {class}");
        }
        // The classes that carry the picture are all different colors.
        let video = kind_class(FileKind::Video);
        let image = kind_class(FileKind::Image);
        let archive = kind_class(FileKind::Archive);
        assert_ne!(video, image);
        assert_ne!(image, archive);
    }
}
