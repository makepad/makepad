//! The folder body: four views over one model.
//!
//! Icons, List, Compact and the Treemap all read the same `rows` + `selected`,
//! so switching a view never loses the selection, the filter or the sort. Only
//! the visible page draws, which is why the drawing code can tell which list it
//! was handed from `mode` alone.
//!
//! Selection is held as a set of **paths**, not indices. That is what lets a
//! re-sort, a re-listing, a rename or a folder expanding under the cursor
//! leave the selection exactly where the user put it — an index-based
//! selection silently moves to whatever file slid into that slot.

use makepad_widgets::*;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crate::{
    model::{format_size, FileEntry, SortKey, SortSpec},
    theme::Palette,
    thumbs::{clear_thumb, fill_thumb, Thumbs},
    treemap_view::{TreemapAction, TreemapViewRef, TreemapViewWidgetExt},
};

/// Cells in a grid row. The row template carries this many; the ones past the
/// column count for the current width are hidden, and hidden views take no
/// layout, so the visible cells always split the width evenly.
pub const GRID_MAX_COLUMNS: usize = 12;
/// Width the auto-fitted Name column leaves for the vertical scroll bar, so
/// the last column's text never ends up underneath it.
const SCROLL_BAR_ALLOWANCE: f64 = 16.0;
/// One indent step of the List view's folder tree, in points.
const INDENT_STEP: f64 = 15.0;

/// The four icon sizes Cmd+plus and Cmd+minus cycle: (tile width, row height,
/// thumbnail height). The name always gets its two lines under the picture, so
/// only the picture grows.
pub const ZOOM_LEVELS: [(f64, f64, f64); 4] = [
    (102.0, 100.0, 38.0),
    (132.0, 128.0, 56.0),
    (176.0, 168.0, 90.0),
    (236.0, 220.0, 140.0),
];
/// The size a window opens at — the one round two shipped.
pub const DEFAULT_ZOOM: usize = 1;

/// The columns a fresh window shows. Created and Permissions are off until
/// the user picks them out of the column menu.
pub const DEFAULT_COLUMNS: [SortKey; 4] = [
    SortKey::Name,
    SortKey::Size,
    SortKey::Kind,
    SortKey::Modified,
];

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let FileTile = View{
        width: Fill
        height: Fill
        flow: Overlay
        cursor: MouseCursor.Hand
        tile_sel := SolidView{
            visible: false
            width: Fill
            height: Fill
            draw_bg +: {color: mod.mpf.sel}
        }
        tile_body := View{
            width: Fill
            height: Fill
            flow: Down
            spacing: 6
            padding: Inset{left: 4 right: 4 top: 10 bottom: 6}
            align: Align{x: 0.5 y: 0.0}
            tile_thumb := MpfThumb{
                width: Fill
                height: 56
            }
            tile_name_slot := View{
                width: Fill
                // Two lines at this size need 40pt; 34 clipped the descenders
                // of the second one.
                height: 40
                flow: Overlay
                tile_name := Label{
                    width: Fill
                    height: Fill
                    align: Align{x: 0.5 y: 0.0}
                    max_lines: 2
                    text_overflow: TextOverflow.Ellipsis
                    draw_text +: {
                        color: mod.mpf.fg
                        text_style: theme.font_regular{font_size: 9.5}
                    }
                }
                // The inline editor sits in a View of its own because only
                // View, Label, Image and Button carry `visible`.
                tile_edit_box := View{
                    visible: false
                    width: Fill
                    height: 22
                    tile_edit := MpfInput{}
                }
            }
        }
    }

    let CompactRow = View{
        width: Fill
        height: 26
        flow: Overlay
        cursor: MouseCursor.Hand
        row_sel := SolidView{
            visible: false
            width: Fill
            height: Fill
            draw_bg +: {color: mod.mpf.sel}
        }
        row_body := View{
            width: Fill
            height: Fill
            flow: Right
            spacing: 9
            // The right padding clears the scroll bar so the size column's
            // last glyph is not clipped against it.
            padding: Inset{left: 16 right: 24}
            align: Align{y: 0.5}
            row_thumb := MpfThumb{
                width: 18
                height: 18
                img +: {width: 18 height: 18}
            }
            row_name_slot := View{
                width: Fill
                height: Fill
                flow: Overlay
                align: Align{y: 0.5}
                row_name := Label{
                    width: Fill
                    max_lines: 1
                    text_overflow: TextOverflow.Ellipsis
                    draw_text +: {
                        color: mod.mpf.fg
                        text_style: theme.font_regular{font_size: 9.5}
                    }
                }
                row_edit_box := View{
                    visible: false
                    width: Fill
                    height: 22
                    row_edit := MpfInput{}
                }
            }
            // The size right-aligns inside its own box: a Label's own align
            // does not push its ink to the box edge, and text that runs on
            // ends up under the scroll bar.
            row_size_box := View{
                width: 96
                height: Fill
                align: Align{x: 1.0 y: 0.5}
                row_size := Label{
                    draw_text +: {
                        color: mod.mpf.fg_dim
                        text_style: theme.font_regular{font_size: 9.0}
                    }
                }
            }
        }
    }

    mod.widgets.FileContentsBase = #(FileContents::register_widget(vm))
    mod.widgets.FileContents = set_type_default() do mod.widgets.FileContentsBase{
        width: Fill
        height: Fill

        icons_page := View{
            width: Fill
            height: Fill
            icons_list := PortalList{
                width: Fill
                height: Fill
                scroll_bar: ScrollBar{}
                GridRow := View{
                    width: Fill
                    height: 128
                    flow: Right
                    padding: Inset{left: 12 right: 12}
                    c0 := FileTile{}
                    c1 := FileTile{}
                    c2 := FileTile{}
                    c3 := FileTile{}
                    c4 := FileTile{}
                    c5 := FileTile{}
                    c6 := FileTile{}
                    c7 := FileTile{}
                    c8 := FileTile{}
                    c9 := FileTile{}
                    c10 := FileTile{}
                    c11 := FileTile{}
                }
            }
        }

        list_page := View{
            visible: false
            width: Fill
            height: Fill
            list_grid := DataGrid{
                width: Fill
                height: Fill
                rows: 0
                cols: 4
                show_row_headers: false
                zebra_stripes: true
                allow_col_resize: true
                allow_col_reorder: false
                default_row_height: 28.0
                col_header_height: 30.0
                cell_pad_x: 12.0
                color_bg: mod.mpf.bg
                color_cell: mod.mpf.bg
                color_cell_alt: mod.mpf.stripe
                color_text: mod.mpf.fg
                color_header: mod.mpf.bg_light
                color_header_active: mod.mpf.hover
                color_header_text: mod.mpf.fg_dim
                color_selection: mod.mpf.sel_soft
                color_selection_border: mod.mpf.accent
                color_drag_marker: mod.mpf.accent
                color_resize_guide: mod.mpf.muted
                draw_cell +: {
                    border_color: uniform(mod.mpf.bg)
                }
                draw_text +: {
                    color: mod.mpf.fg
                    text_style: theme.font_regular{font_size: 9.5}
                }
                draw_text_bold +: {
                    color: mod.mpf.fg_dim
                    text_style: theme.font_bold{font_size: 9.0}
                }
                NameCell := View{
                    width: Fill
                    height: Fill
                    flow: Right
                    spacing: 7
                    padding: Inset{left: 6 right: 6}
                    align: Align{y: 0.5}
                    // The tree indent: a spacer whose width is the row's depth.
                    cell_indent := View{
                        width: 0
                        height: 1
                    }
                    // The disclosure triangle. A folder that can be opened
                    // shows one; everything else shows an empty box of the
                    // same width so the names still line up.
                    // The triangle is an icon, not a character: ▸ and ▾ are
                    // not in the UI font and came out as empty .notdef boxes.
                    cell_twist := View{
                        width: 13
                        height: Fill
                        flow: Overlay
                        align: Align{x: 0.5 y: 0.5}
                        cursor: MouseCursor.Hand
                        twist_closed := View{
                            visible: false
                            width: Fill
                            height: Fill
                            align: Align{x: 0.5 y: 0.5}
                            Icon{
                                icon_walk: Walk{width: 7 height: 7}
                                draw_icon +: {
                                    svg: crate_resource("self://resources/icons/twist-right.svg")
                                    color: mod.mpf.fg_dim
                                }
                            }
                        }
                        twist_open := View{
                            visible: false
                            width: Fill
                            height: Fill
                            align: Align{x: 0.5 y: 0.5}
                            Icon{
                                icon_walk: Walk{width: 7 height: 7}
                                draw_icon +: {
                                    svg: crate_resource("self://resources/icons/twist-down.svg")
                                    color: mod.mpf.fg
                                }
                            }
                        }
                    }
                    cell_thumb := MpfThumb{
                        width: 18
                        height: 18
                        img +: {width: 18 height: 18}
                    }
                    cell_name_slot := View{
                        width: Fill
                        height: Fill
                        flow: Overlay
                        align: Align{y: 0.5}
                        cell_name := Label{
                            width: Fill
                            max_lines: 1
                            text_overflow: TextOverflow.Ellipsis
                            draw_text +: {
                                color: mod.mpf.fg
                                text_style: theme.font_regular{font_size: 9.5}
                            }
                        }
                        cell_edit_box := View{
                            visible: false
                            width: Fill
                            height: 22
                            cell_edit := MpfInput{}
                        }
                    }
                }
            }
        }

        compact_page := View{
            visible: false
            width: Fill
            height: Fill
            compact_list := PortalList{
                width: Fill
                height: Fill
                scroll_bar: ScrollBar{}
                CompactRow := CompactRow{}
            }
        }

        treemap_page := View{
            visible: false
            width: Fill
            height: Fill
            treemap := MpfTreemap{}
        }
    }
}

/// The ways to look at a folder. The last three are one treemap under three
/// projections — flat, extruded, perspective — sharing scan, camera and pick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    Icons,
    List,
    Compact,
    Treemap,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            ViewMode::Icons => "Icons",
            ViewMode::List => "List",
            ViewMode::Compact => "Compact",
            ViewMode::Treemap => "Treemap",
        }
    }

    /// Whether this mode shows the treemap page, whatever the projection.
    pub fn is_treemap(self) -> bool {
        matches!(self, ViewMode::Treemap)
    }
}

/// One line of the display list: an entry, and where it sits in the List
/// view's folder tree.
#[derive(Clone, Debug)]
pub struct Row {
    pub entry: FileEntry,
    /// 0 for the folder's own entries, 1 for the children of an expanded
    /// folder, and so on. Only the List view draws it.
    pub depth: usize,
    /// True when this folder can be opened in place — a folder, in the List
    /// view, that we have not found to be empty.
    pub expandable: bool,
    pub expanded: bool,
}

/// What a click in the body means to the app.
#[derive(Clone, Debug)]
pub enum FileContentsAction {
    Open(FileEntry),
    Selected(FileEntry),
    /// A column header was clicked; the listing is re-ordered.
    Sorted,
    /// The view changed what it is saying about itself and the status line
    /// should ask it again — the treemap picking something the listing does
    /// not hold, which is most of the map.
    Restated,
    /// The inline editor was confirmed: `path` should become `name`.
    Renamed(PathBuf, String),
    /// The inline editor was dismissed with nothing changed.
    RenameCancelled,
    /// A drag of these paths ended at this window point. Only the shell knows
    /// what is under it.
    Dropped(Vec<PathBuf>, DVec2),
    /// A folder in the List tree was opened and its children are not loaded.
    NeedChildren(PathBuf),
    /// The map's filter chip was clicked away; the filter controls should
    /// show themselves cleared.
    MapFilterCleared,
    /// A secondary press: open the context menu at `at`, for `entry` when the
    /// press landed on one and for the folder itself when it landed on the
    /// empty space.
    Context {
        at: DVec2,
        entry: Option<FileEntry>,
    },
}

/// Where one row was drawn this frame.
#[derive(Clone, Copy, Debug)]
pub struct HitRect {
    pub position: usize,
    pub rect: Rect,
}

/// A secondary press, resolved against what was drawn.
#[derive(Clone, Debug)]
pub struct ContextHit {
    pub at: DVec2,
    pub position: Option<usize>,
    /// The target when it is not in the current listing at all — a file the
    /// treemap found several folders down. The menu acts on it exactly as it
    /// acts on a row, because it is exactly as real a file.
    pub off_list: Option<FileEntry>,
}

/// The colors the body sets from Rust. Everything else comes straight from
/// `mod.mpf`; only the fast text-cell path needs a `Vec4f` in hand.
#[derive(Clone, Copy)]
pub struct Colors {
    pub dim: Vec4f,
    pub selection: Vec4f,
}

impl Default for Colors {
    fn default() -> Self {
        let palette = Palette::tokyo_night();
        Self {
            dim: Palette::vec4(&palette.fg_dim),
            selection: Palette::vec4(&palette.sel),
        }
    }
}

/// A press the body wants the shell to look at. It carries nothing: its only
/// job is to make sure an `Actions` event happens at all, because a secondary
/// press that no widget claims produces no actions of its own and the shell's
/// action handler would never run.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ContentsPing {
    #[default]
    Ping,
}

#[derive(Script, ScriptHook, Widget)]
pub struct FileContents {
    #[deref]
    view: View,
    /// The current folder's own entries, as the worker read them.
    #[rust]
    entries: Vec<FileEntry>,
    /// Children of folders the List view has expanded, keyed by folder.
    #[rust]
    children: HashMap<PathBuf, Vec<FileEntry>>,
    #[rust]
    expanded: HashSet<PathBuf>,
    /// Folders whose children have been asked for but not yet delivered.
    #[rust]
    pending: HashSet<PathBuf>,
    /// The display list every view walks: filtered, sorted, and — in the List
    /// view — flattened out of the expanded tree.
    #[rust]
    rows: Vec<Row>,
    #[rust]
    filter: String,
    #[rust]
    sort: SortSpec,
    /// The selection, by path so it survives everything that renumbers rows.
    #[rust]
    selected: HashSet<PathBuf>,
    /// The last path the user landed on: the one Space previews, Cmd+I
    /// describes, and Shift+click extends from.
    #[rust]
    anchor: Option<PathBuf>,
    #[rust]
    mode: ViewMode,
    /// Tiles per row in the icons view, from the current width.
    #[rust]
    grid_columns: usize,
    #[rust]
    last_width: f64,
    #[rust]
    colors: Colors,
    #[rust]
    thumbs: Thumbs,
    #[rust]
    grid_ready: bool,
    /// Set once the user drags a column divider: from then on the widths are
    /// theirs for the rest of the session and the Name column stops
    /// auto-fitting to the window.
    #[rust]
    columns_user_sized: bool,
    /// The columns the list view shows, in order. Name always leads; the
    /// rest are the user's to pick.
    #[rust]
    columns: Vec<SortKey>,
    /// The window width the Name column was last fitted to. Re-fitting only
    /// when this changes is what lets a column drag stick: fitting every
    /// frame would pull the width back under the user's hand.
    #[rust]
    fitted_width: f64,
    /// The icon size, an index into [`ZOOM_LEVELS`].
    #[rust]
    zoom: usize,
    /// The file being renamed in place, if any.
    #[rust]
    renaming: Option<PathBuf>,
    /// The editor cannot take key focus until it has been drawn once and has
    /// an area to focus; this is the frame after.
    #[rust]
    rename_focus: NextFrame,
    #[rust]
    rename_tries: usize,
    /// The file whose editor has already been filled with its current name.
    /// Filling it every frame would erase what the user is typing.
    #[rust]
    rename_seeded: Option<PathBuf>,
    /// Where a press started, so a release somewhere else reads as a drag.
    #[rust]
    press_at: Option<DVec2>,
    /// The screen rectangle of every row drawn this frame, by display
    /// position. A context menu has to know what is under the pointer even
    /// though the widget under it swallowed the press, so the answer comes
    /// from geometry the drawing already knows.
    #[rust]
    hit_rects: Vec<HitRect>,
    /// The whole body, for telling "inside the folder view" from "somewhere
    /// else in the window".
    #[rust]
    body_rect: Rect,
    /// A secondary press waiting to be reported to the shell.
    #[rust]
    pending_context: Option<ContextHit>,
}

impl FileContents {
    // -------------------------------------------------------------- model

    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<FileEntry>) {
        self.entries = entries;
        // A new listing is a new folder: nothing it expanded still applies.
        self.children.clear();
        self.expanded.clear();
        self.pending.clear();
        self.selected.clear();
        self.anchor = None;
        self.renaming = None;
        self.reorder(cx);
    }

    /// Deliver the children of a folder the List tree expanded.
    pub fn set_children(&mut self, cx: &mut Cx, folder: &Path, entries: Vec<FileEntry>) {
        self.pending.remove(folder);
        self.children.insert(folder.to_path_buf(), entries);
        self.reorder(cx);
    }

    pub fn set_filter(&mut self, cx: &mut Cx, filter: String) {
        self.filter = filter;
        self.reorder(cx);
    }

    pub fn sort(&self) -> SortSpec {
        self.sort
    }

    pub fn set_sort(&mut self, cx: &mut Cx, sort: SortSpec) {
        self.sort = sort;
        self.reorder(cx);
    }

    pub fn zoom(&self) -> usize {
        self.zoom
    }

    /// Step the icon size. Returns the tile width now in force, for the
    /// status line.
    pub fn set_zoom(&mut self, cx: &mut Cx, zoom: usize) -> f64 {
        self.zoom = zoom.min(ZOOM_LEVELS.len() - 1);
        self.view.redraw(cx);
        ZOOM_LEVELS[self.zoom].0
    }

    /// Rebuild the display order from the filter, the sort and the expansions.
    fn reorder(&mut self, cx: &mut Cx) {
        let needle = self.filter.to_lowercase();
        let mut rows = Vec::with_capacity(self.entries.len());
        let tree = self.mode == ViewMode::List;
        self.push_rows(&self.entries.clone(), 0, &needle, tree, &mut rows);
        self.rows = rows;
        self.sync_grid_selection(cx);
        self.view.redraw(cx);
    }

    /// Append one level of the tree, sorted, then recurse into whatever of it
    /// is expanded.
    fn push_rows(
        &self,
        entries: &[FileEntry],
        depth: usize,
        needle: &str,
        tree: bool,
        out: &mut Vec<Row>,
    ) {
        let mut order: Vec<usize> = (0..entries.len())
            .filter(|i| {
                // A filter only ever hides entries at the top level: hiding a
                // child would leave its parent claiming to be open onto
                // nothing.
                depth > 0 || needle.is_empty() || entries[*i].name.to_lowercase().contains(needle)
            })
            .collect();
        crate::model::sort_indices(entries, &mut order, self.sort);
        for index in order {
            let entry = entries[index].clone();
            let expanded = self.expanded.contains(&entry.path);
            let expandable = tree && entry.is_dir && entry.child_count.unwrap_or(1) > 0;
            out.push(Row {
                depth,
                expandable,
                expanded: expanded && expandable,
                entry: entry.clone(),
            });
            if !expanded || !expandable {
                continue;
            }
            if let Some(children) = self.children.get(&entry.path) {
                self.push_rows(children, depth + 1, needle, tree, out);
            }
        }
    }

    pub fn set_mode(&mut self, cx: &mut Cx, mode: ViewMode) {
        let was_tree = self.mode == ViewMode::List;
        self.mode = mode;
        self.cancel_rename(cx);
        self.view
            .view(cx, ids!(icons_page))
            .set_visible(cx, mode == ViewMode::Icons);
        self.view
            .view(cx, ids!(list_page))
            .set_visible(cx, mode == ViewMode::List);
        self.view
            .view(cx, ids!(compact_page))
            .set_visible(cx, mode == ViewMode::Compact);
        self.view
            .view(cx, ids!(treemap_page))
            .set_visible(cx, mode.is_treemap());
        // The tree only exists in the List view, so leaving it flattens the
        // rows and entering it can bring them back.
        if was_tree != (mode == ViewMode::List) {
            self.reorder(cx);
        }
        self.view.redraw(cx);
    }

    /// The treemap, for the shell to point at a folder and drain.
    pub fn treemap(&self, cx: &mut Cx) -> TreemapViewRef {
        self.view.treemap_view(cx, ids!(treemap))
    }

    pub fn set_colors(&mut self, cx: &mut Cx, colors: Colors) {
        self.colors = colors;
        self.view.redraw(cx);
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn total(&self) -> usize {
        self.entries.len()
    }

    /// Thumbnails decoded and resident, for the status line.
    pub fn thumbs_resident(&self) -> usize {
        self.thumbs.resident()
    }

    /// The columns the list view shows, in order.
    pub fn columns(&self) -> Vec<SortKey> {
        if self.columns.is_empty() {
            DEFAULT_COLUMNS.to_vec()
        } else {
            self.columns.clone()
        }
    }

    /// Show or hide one column. Name is the row's identity and always stays.
    pub fn toggle_column(&mut self, cx: &mut Cx, column: SortKey) {
        if column == SortKey::Name {
            return;
        }
        let mut columns = self.columns();
        match columns.iter().position(|c| *c == column) {
            Some(at) => {
                columns.remove(at);
                // Sorting by a column nobody can see is a sort with no
                // indicator: fall back to the name order.
                if self.sort.key == column {
                    self.sort = SortSpec::default();
                    self.reorder(cx);
                }
            }
            // Keep the natural order rather than appending: the columns read
            // the same however they were switched on.
            None => {
                columns.push(column);
                columns.sort_by_key(|c| {
                    SortKey::ALL.iter().position(|a| a == c).unwrap_or(usize::MAX)
                });
            }
        }
        self.columns = columns;
        // The widths belong to the column set that asked for them.
        self.grid_ready = false;
        self.columns_user_sized = false;
        self.fitted_width = 0.0;
        let grid = self.view.data_grid(cx, ids!(list_grid));
        grid.redraw(cx);
        self.view.redraw(cx);
    }

    /// Push the current column set's labels and widths into the grid.
    fn apply_columns(&self, grid: &mut DataGrid) {
        let columns = self.columns();
        grid.set_col_labels(columns.iter().map(|c| c.label().to_string()).collect());
        for (index, column) in columns.iter().enumerate() {
            grid.set_col_width(index, column.default_width());
        }
    }

    // ---------------------------------------------------------- selection

    /// The entry the shell acts on: the one the user last landed on.
    pub fn selected_entry(&self) -> Option<FileEntry> {
        let anchor = self.anchor.as_ref()?;
        self.rows
            .iter()
            .find(|r| &r.entry.path == anchor)
            .map(|r| r.entry.clone())
    }

    /// Everything selected, in display order — what copy, trash and batch
    /// rename operate on.
    pub fn selected_entries(&self) -> Vec<FileEntry> {
        self.rows
            .iter()
            .filter(|r| self.selected.contains(&r.entry.path))
            .map(|r| r.entry.clone())
            .collect()
    }

    pub fn selection_count(&self) -> usize {
        self.selected.len()
    }

    /// Put the selection on exactly these paths (as far as they are on
    /// screen), with the first as the anchor. Used after an operation lands.
    pub fn select_paths(&mut self, cx: &mut Cx, paths: &[PathBuf]) {
        self.selected = paths.iter().cloned().collect();
        self.anchor = paths.first().cloned();
        let anchor = self.anchor.clone();
        self.treemap(cx).set_selected(cx, anchor);
        if let Some(anchor) = self.anchor.clone() {
            if let Some(position) = self.rows.iter().position(|r| r.entry.path == anchor) {
                self.scroll_into_view(cx, position);
            }
        }
        self.sync_grid_selection(cx);
        self.view.redraw(cx);
    }

    pub fn select_all(&mut self, cx: &mut Cx) {
        self.selected = self.rows.iter().map(|r| r.entry.path.clone()).collect();
        if self.anchor.is_none() {
            self.anchor = self.rows.first().map(|r| r.entry.path.clone());
        }
        self.sync_grid_selection(cx);
        self.view.redraw(cx);
    }

    pub fn clear_selection(&mut self, cx: &mut Cx) {
        self.selected.clear();
        self.anchor = None;
        self.sync_grid_selection(cx);
        self.view.redraw(cx);
    }

    /// A click at `position` in the display list, with the modifiers that
    /// decide what it means.
    fn click(&mut self, cx: &mut Cx, position: usize, modifiers: KeyModifiers) {
        let Some(row) = self.rows.get(position) else {
            return;
        };
        let path = row.entry.path.clone();
        if modifiers.shift {
            // Extend from the anchor: the run between the two, inclusive.
            let from = self
                .anchor
                .as_ref()
                .and_then(|a| self.rows.iter().position(|r| &r.entry.path == a))
                .unwrap_or(position);
            let (lo, hi) = (from.min(position), from.max(position));
            self.selected = self.rows[lo..=hi]
                .iter()
                .map(|r| r.entry.path.clone())
                .collect();
        } else if modifiers.logo || modifiers.control {
            // Toggle one out of the set without disturbing the rest.
            if !self.selected.remove(&path) {
                self.selected.insert(path.clone());
            }
        } else {
            self.selected.clear();
            self.selected.insert(path.clone());
        }
        self.anchor = Some(path);
        self.sync_grid_selection(cx);
        self.view.redraw(cx);
    }

    /// Keep the grid's own scroll on the anchor. The row highlight is painted
    /// by the cell drawing instead of by `GridSelection`, because a selection
    /// made of scattered Cmd-clicks is not a rectangle and the grid's own
    /// overlay can only draw rectangles.
    fn sync_grid_selection(&mut self, cx: &mut Cx) {
        let grid = self.view.data_grid(cx, ids!(list_grid));
        grid.set_selection(cx, None);
        let row = self
            .anchor
            .as_ref()
            .and_then(|a| self.rows.iter().position(|r| &r.entry.path == a));
        if let Some(row) = row {
            grid.scroll_cell_into_view(cx, row, 0);
        }
    }

    /// Move the selection by `amount` display positions and return what is
    /// now selected. `extend` grows the selection instead of replacing it.
    pub fn move_selection(
        &mut self,
        cx: &mut Cx,
        amount: isize,
        extend: bool,
    ) -> Option<FileEntry> {
        if self.rows.is_empty() {
            self.selected.clear();
            self.anchor = None;
            return None;
        }
        let current = self
            .anchor
            .as_ref()
            .and_then(|a| self.rows.iter().position(|r| &r.entry.path == a));
        let next = match current {
            Some(current) => (current as isize + amount).clamp(0, self.rows.len() as isize - 1),
            // Nothing selected yet: the first step lands on an end, not on
            // whatever index the offset happens to hit.
            None if amount < 0 => self.rows.len() as isize - 1,
            None => 0,
        } as usize;
        let path = self.rows[next].entry.path.clone();
        if extend {
            self.selected.insert(path.clone());
        } else {
            self.selected.clear();
            self.selected.insert(path.clone());
        }
        self.anchor = Some(path);
        self.sync_grid_selection(cx);
        self.scroll_into_view(cx, next);
        self.view.redraw(cx);
        self.selected_entry()
    }

    /// How many display positions one arrow key covers in this view.
    pub fn row_stride(&self) -> isize {
        match self.mode {
            ViewMode::Icons => self.grid_columns.max(1) as isize,
            _ => 1,
        }
    }

    fn scroll_into_view(&mut self, cx: &mut Cx, position: usize) {
        match self.mode {
            ViewMode::Icons => {
                let row = position / self.grid_columns.max(1);
                self.view
                    .portal_list(cx, ids!(icons_list))
                    .smooth_scroll_to(cx, row, 90.0, None, 0.0);
            }
            ViewMode::Compact => {
                self.view
                    .portal_list(cx, ids!(compact_list))
                    .smooth_scroll_to(cx, position, 90.0, None, 0.0);
            }
            // The grid scrolls itself from `sync_grid_selection`; the map has
            // no scroll at all.
            ViewMode::List | ViewMode::Treemap => {}
        }
    }

    // ----------------------------------------------------------- renaming

    /// True while an inline editor is open — the shell must then leave the
    /// keyboard alone, because a hidden text field keeps key focus.
    pub fn is_renaming(&self) -> bool {
        self.renaming.is_some()
    }

    /// Open the inline editor over `path`'s name.
    pub fn begin_rename(&mut self, cx: &mut Cx, path: &Path) -> bool {
        if !self.rows.iter().any(|r| r.entry.path == path) {
            return false;
        }
        if self.mode.is_treemap() {
            return false;
        }
        self.renaming = Some(path.to_path_buf());
        self.selected.clear();
        self.selected.insert(path.to_path_buf());
        self.anchor = Some(path.to_path_buf());
        // The field has no area until it has been drawn, so focus waits a
        // frame.
        self.rename_tries = 0;
        self.rename_seeded = None;
        self.rename_focus = cx.new_next_frame();
        self.view.redraw(cx);
        true
    }

    /// Close the editor without renaming anything.
    pub fn cancel_rename(&mut self, cx: &mut Cx) {
        if self.renaming.take().is_some() {
            // A hidden text field would otherwise keep key focus and swallow
            // every navigation key from here on.
            cx.set_key_focus(Area::Empty);
            self.view.redraw(cx);
        }
    }

    /// The widget hosting the inline editor for the row at `position`, if it
    /// is on screen.
    fn rename_editor(&self, cx: &mut Cx, position: usize) -> Option<TextInputRef> {
        match self.mode {
            ViewMode::Icons => {
                let columns = self.grid_columns.max(1);
                let list = self.view.portal_list(cx, ids!(icons_list));
                let (row, column) = (position / columns, position % columns);
                let (_, item) = list.get_item(row)?;
                Some(
                    item.widget(cx, CELL_IDS[column])
                        .text_input(cx, ids!(tile_edit)),
                )
            }
            ViewMode::Compact => {
                let list = self.view.portal_list(cx, ids!(compact_list));
                let (_, item) = list.get_item(position)?;
                Some(item.text_input(cx, ids!(row_edit)))
            }
            ViewMode::List => {
                let grid = self.view.data_grid(cx, ids!(list_grid));
                let (_, item) = grid.get_item(position, 0)?;
                Some(item.text_input(cx, ids!(cell_edit)))
            }
            ViewMode::Treemap => None,
        }
    }

    /// The display position of the row being renamed.
    fn rename_position(&self) -> Option<usize> {
        let path = self.renaming.as_ref()?;
        self.rows.iter().position(|r| &r.entry.path == path)
    }

    // ---------------------------------------------------------- thumbnails

    /// Turn finished decodes into textures; true when a redraw is owed.
    pub fn drain_thumbs(&mut self, cx: &mut Cx) -> bool {
        if self.thumbs.drain(cx) {
            self.view.redraw(cx);
            return true;
        }
        false
    }

    /// Columns that fit in `width` at `tile_width`, at least one and never
    /// more than the row template carries.
    fn columns_for(width: f64, tile_width: f64) -> usize {
        (((width - 24.0) / tile_width).floor() as isize).clamp(1, GRID_MAX_COLUMNS as isize)
            as usize
    }

    // ---------------------------------------------------------------- draw

    fn draw_icons(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        let columns = self.grid_columns.max(1);
        let rows = self.rows.len().div_ceil(columns);
        let (tile_width, row_height, thumb_height) =
            ZOOM_LEVELS[self.zoom.min(ZOOM_LEVELS.len() - 1)];
        // The picture never touches the tile's edges: the name below it needs
        // the same optical margin the small size already had.
        let thumb_width = (tile_width - 24.0).max(24.0);
        let renaming = self.rename_position();
        list.set_item_range(cx, 0, rows);
        while let Some(row) = list.next_visible_item(cx) {
            let mut item = list.item(cx, row, id!(GridRow));
            script_apply_eval!(cx, item, {
                height: #(row_height)
            });
            if row >= rows {
                // Past the last row: blank every cell rather than leave a
                // recycled one showing the previous folder.
                for column in 0..GRID_MAX_COLUMNS {
                    item.widget(cx, CELL_IDS[column]).set_visible(cx, false);
                }
                item.draw_all(cx, &mut Scope::empty());
                continue;
            }
            for column in 0..GRID_MAX_COLUMNS {
                let cell = item.widget(cx, CELL_IDS[column]);
                // Every column the width affords stays in the layout even when
                // the folder runs out, so a short last row keeps the same tile
                // size as a full one instead of stretching to fill it.
                cell.set_visible(cx, column < columns);
                if column >= columns {
                    continue;
                }
                let position = row * columns + column;
                // Both the slot and the picture inside it grow with the zoom:
                // sizing only the picture would draw it inside a box that is
                // still the small size, and clip it.
                let mut thumb = cell.widget(cx, ids!(tile_thumb));
                script_apply_eval!(cx, thumb, {
                    height: #(thumb_height)
                });
                let mut img = thumb.widget(cx, ids!(img));
                script_apply_eval!(cx, img, {
                    width: #(thumb_width)
                    height: #(thumb_height)
                });
                if position >= self.rows.len() {
                    cell.widget(cx, ids!(tile_sel)).set_visible(cx, false);
                    cell.widget(cx, ids!(tile_edit_box)).set_visible(cx, false);
                    cell.label(cx, ids!(tile_name)).set_text(cx, "");
                    clear_thumb(cx, &thumb);
                    continue;
                }
                let entry = self.rows[position].entry.clone();
                let editing = renaming == Some(position);
                cell.widget(cx, ids!(tile_sel))
                    .set_visible(cx, self.selected.contains(&entry.path));
                cell.widget(cx, ids!(tile_name))
                    .set_visible(cx, !editing);
                cell.widget(cx, ids!(tile_edit_box)).set_visible(cx, editing);
                cell.label(cx, ids!(tile_name)).set_text(cx, &entry.name);
                if editing && self.rename_seeded.as_deref() != Some(entry.path.as_path()) {
                    self.rename_seeded = Some(entry.path.clone());
                    cell.text_input(cx, ids!(tile_edit)).set_text(cx, &entry.name);
                }
                fill_thumb(cx, &thumb, &entry, &mut self.thumbs);
            }
            item.draw_all(cx, &mut Scope::empty());
            // The tile rectangles are only final once the row has been drawn.
            for column in 0..columns.min(GRID_MAX_COLUMNS) {
                let position = row * columns + column;
                if position >= self.rows.len() {
                    break;
                }
                let rect = item.widget(cx, CELL_IDS[column]).area().rect(cx);
                if rect.size.x > 0.0 {
                    self.hit_rects.push(HitRect { position, rect });
                }
            }
        }
    }

    fn draw_compact(&mut self, cx: &mut Cx2d, list: &mut PortalList) {
        let renaming = self.rename_position();
        list.set_item_range(cx, 0, self.rows.len());
        while let Some(position) = list.next_visible_item(cx) {
            let item = list.item(cx, position, id!(CompactRow));
            if position >= self.rows.len() {
                // A row past the end still has to be cleared: a recycled item
                // that is never repopulated keeps the last row's highlight.
                item.widget(cx, ids!(row_sel)).set_visible(cx, false);
                item.widget(cx, ids!(row_edit_box)).set_visible(cx, false);
                item.widget(cx, ids!(row_name)).set_visible(cx, true);
                item.label(cx, ids!(row_name)).set_text(cx, "");
                item.label(cx, ids!(row_size)).set_text(cx, "");
                let thumb = item.widget(cx, ids!(row_thumb));
                clear_thumb(cx, &thumb);
                item.draw_all(cx, &mut Scope::empty());
                continue;
            }
            let entry = self.rows[position].entry.clone();
            let editing = renaming == Some(position);
            item.widget(cx, ids!(row_sel))
                .set_visible(cx, self.selected.contains(&entry.path));
            item.widget(cx, ids!(row_name)).set_visible(cx, !editing);
            item.widget(cx, ids!(row_edit_box)).set_visible(cx, editing);
            item.label(cx, ids!(row_name)).set_text(cx, &entry.name);
            if editing && self.rename_seeded.as_deref() != Some(entry.path.as_path()) {
                self.rename_seeded = Some(entry.path.clone());
                item.text_input(cx, ids!(row_edit)).set_text(cx, &entry.name);
            }
            item.label(cx, ids!(row_size))
                .set_text(cx, &format_size(entry.size, entry.is_dir));
            let thumb = item.widget(cx, ids!(row_thumb));
            fill_thumb(cx, &thumb, &entry, &mut self.thumbs);
            item.draw_all(cx, &mut Scope::empty());
            let rect = item.area().rect(cx);
            if rect.size.x > 0.0 {
                self.hit_rects.push(HitRect { position, rect });
            }
        }
    }

    fn draw_list(&mut self, cx: &mut Cx2d, grid: &mut DataGrid) {
        if self.columns.is_empty() {
            self.columns = DEFAULT_COLUMNS.to_vec();
        }
        let columns = self.columns.clone();
        if !self.grid_ready {
            self.grid_ready = true;
            self.apply_columns(grid);
        }
        // Name takes whatever the other columns leave, so the table fills the
        // window instead of ending in a band of empty background. This runs
        // when the width it was fitted to changes — never every frame, which
        // would undo a column drag while it is happening — and not at all
        // once the user has sized a column themselves.
        if !self.columns_user_sized && (self.last_width - self.fitted_width).abs() > 0.5 {
            self.fitted_width = self.last_width;
            let fixed: f64 = (1..columns.len()).map(|i| grid.col_width(i)).sum();
            let name_width = (self.last_width - fixed - SCROLL_BAR_ALLOWANCE).max(160.0);
            grid.set_col_width(0, name_width);
        }
        grid.set_grid_size(self.rows.len(), columns.len());
        grid.set_sort_indicator(
            columns
                .iter()
                .position(|c| *c == self.sort.key)
                .map(|col| (col, self.sort.ascending)),
        );
        let renaming = self.rename_position();
        while let Some(cell) = grid.next_cell(cx) {
            if cell.row >= self.rows.len() {
                continue;
            }
            let Some(column) = columns.get(cell.col).copied() else {
                continue;
            };
            let row = self.rows[cell.row].clone();
            let entry = &row.entry;
            let picked = self.selected.contains(&entry.path);
            let bg = picked.then_some(self.colors.selection);
            if column == SortKey::Name {
                // The name column carries the icon, the tree's indent and its
                // disclosure triangle.
                let Some(item) = grid.item(cx, cell.row, cell.col, id!(NameCell)) else {
                    continue;
                };
                let indent = row.depth as f64 * INDENT_STEP;
                let mut spacer = item.widget(cx, ids!(cell_indent));
                script_apply_eval!(cx, spacer, {
                    width: #(indent)
                });
                item.widget(cx, ids!(twist_closed))
                    .set_visible(cx, row.expandable && !row.expanded);
                item.widget(cx, ids!(twist_open))
                    .set_visible(cx, row.expandable && row.expanded);
                let editing = renaming == Some(cell.row);
                item.widget(cx, ids!(cell_name)).set_visible(cx, !editing);
                item.widget(cx, ids!(cell_edit_box)).set_visible(cx, editing);
                item.label(cx, ids!(cell_name)).set_text(cx, &entry.name);
                if editing && self.rename_seeded.as_deref() != Some(entry.path.as_path()) {
                    self.rename_seeded = Some(entry.path.clone());
                    item.text_input(cx, ids!(cell_edit)).set_text(cx, &entry.name);
                }
                let thumb = item.widget(cx, ids!(cell_thumb));
                fill_thumb(cx, &thumb, entry, &mut self.thumbs);
                grid.draw_item(cx, &cell, &item, bg);
                // The name cell starts at the row's left edge, so widening it
                // to the table's width is the row.
                self.hit_rects.push(HitRect {
                    position: cell.row,
                    rect: Rect {
                        pos: cell.rect.pos,
                        size: dvec2(self.last_width, cell.rect.size.y),
                    },
                });
                continue;
            }
            let (text, align) = (column.text(entry), column.align());
            grid.cell_text_styled(
                cx,
                &cell,
                &text,
                CellStyle {
                    align,
                    color: Some(self.colors.dim),
                    bg,
                    ..CellStyle::default()
                },
            );
        }
    }

    // -------------------------------------------------------------- events

    pub fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) -> Vec<FileContentsAction> {
        let mut out = Vec::new();
        if let Some(hit) = self.pending_context.take() {
            let entry = hit.off_list.clone().or_else(|| {
                hit.position
                    .and_then(|p| self.rows.get(p))
                    .map(|r| r.entry.clone())
            });
            out.push(FileContentsAction::Context { at: hit.at, entry });
            return out;
        }
        // The inline editor is read before anything else: Enter and Escape in
        // a field are never navigation.
        if let Some(position) = self.rename_position() {
            let path = self.renaming.clone().unwrap_or_default();
            if let Some(field) = self.rename_editor(cx, position) {
                if let Some((text, _)) = field.returned(actions) {
                    self.cancel_rename(cx);
                    out.push(FileContentsAction::Renamed(path, text));
                    return out;
                }
                if field.escaped(actions) {
                    self.cancel_rename(cx);
                    out.push(FileContentsAction::RenameCancelled);
                    return out;
                }
            }
        }

        match self.mode {
            ViewMode::Icons => {
                let list = self.view.portal_list(cx, ids!(icons_list));
                let columns = self.grid_columns.max(1);
                for (row, item) in list.items_with_actions(actions) {
                    for column in 0..GRID_MAX_COLUMNS.min(columns) {
                        let position = row * columns + column;
                        if position >= self.rows.len() {
                            continue;
                        }
                        let tile = item.view(cx, CELL_IDS[column]);
                        if let Some(event) = tile.finger_down(actions) {
                            self.press_at = Some(event.abs);
                            out.push(self.hit(cx, position, event.tap_count >= 2, event.modifiers));
                        }
                        if let Some(event) = tile.finger_up(actions) {
                            self.drop(&mut out, event.abs);
                        }
                    }
                }
            }
            ViewMode::Compact => {
                let list = self.view.portal_list(cx, ids!(compact_list));
                for (position, item) in list.items_with_actions(actions) {
                    if position >= self.rows.len() {
                        continue;
                    }
                    let row = item.as_view();
                    if let Some(event) = row.finger_down(actions) {
                        self.press_at = Some(event.abs);
                        out.push(self.hit(cx, position, event.tap_count >= 2, event.modifiers));
                    }
                    if let Some(event) = row.finger_up(actions) {
                        self.drop(&mut out, event.abs);
                    }
                }
            }
            ViewMode::List => {
                // A click on the disclosure triangle opens the folder in
                // place; it must be read before the grid's own cell click so
                // it does not also count as "select this row".
                let grid = self.view.data_grid(cx, ids!(list_grid));
                let mut twisted = None;
                for (row, col, item) in grid.cell_widgets_with_actions(actions) {
                    if col != 0 || row >= self.rows.len() {
                        continue;
                    }
                    if item.view(cx, ids!(cell_twist)).finger_down(actions).is_some() {
                        twisted = Some(row);
                    }
                }
                if let Some(row) = twisted {
                    if let Some(action) = self.toggle_expand(cx, row) {
                        out.push(action);
                    }
                    return out;
                }
                // The grid emits CellClicked *and then* CellDoubleClicked for
                // the second tap, so the whole batch has to be read before
                // deciding: acting on the first one would turn every
                // double-click into a plain select.
                let mut hit: Option<(usize, bool, KeyModifiers)> = None;
                let mut sorted = false;
                for action in grid.actions(actions) {
                    match action {
                        DataGridAction::CellClicked { row, modifiers, .. }
                            if row < self.rows.len() =>
                        {
                            hit = Some((row, false, modifiers));
                        }
                        DataGridAction::CellDoubleClicked { row, .. }
                            if row < self.rows.len() =>
                        {
                            let modifiers = hit.map(|h| h.2).unwrap_or_default();
                            hit = Some((row, true, modifiers));
                        }
                        DataGridAction::ColumnResized { .. } => {
                            self.columns_user_sized = true;
                        }
                        DataGridAction::HeaderClicked { col, .. } => {
                            let Some(key) = self.columns.get(col).copied() else {
                                continue;
                            };
                            let sort = if self.sort.key == key {
                                SortSpec {
                                    key,
                                    ascending: !self.sort.ascending,
                                }
                            } else {
                                SortSpec {
                                    key,
                                    ascending: true,
                                }
                            };
                            self.set_sort(cx, sort);
                            sorted = true;
                        }
                        _ => {}
                    }
                }
                if let Some((row, open, modifiers)) = hit {
                    out.push(self.hit(cx, row, open, modifiers));
                }
                if sorted {
                    out.push(FileContentsAction::Sorted);
                }
            }
            ViewMode::Treemap => {
                // Every action from the map, not just the first: a secondary
                // click emits its pick *and* its context request in one
                // batch, and dropping either would lose the menu or the
                // selection.
                let map_uid = self.treemap(cx).widget_uid();
                let map_actions: Vec<TreemapAction> = actions
                    .iter()
                    .filter_map(|a| a.as_widget_action().filter(|wa| wa.widget_uid == map_uid))
                    .map(|wa| wa.cast::<TreemapAction>())
                    .collect();
                for action in map_actions {
                    match action {
                    // Picking is picking. The old rule — anything not in the
                    // current listing means "go there" — made a single click
                    // on any rectangle below the top level throw the whole
                    // browser somewhere else, which is the opposite of what a
                    // map is for. Deeper picks live on the map's own readout;
                    // only the ones the listing also holds reach the shell.
                    TreemapAction::Selected(path) => {
                        self.selected.clear();
                        self.selected.insert(path.clone());
                        self.anchor = Some(path.clone());
                        if let Some(entry) = self
                            .rows
                            .iter()
                            .find(|r| r.entry.path == path)
                            .map(|r| r.entry.clone())
                        {
                            out.push(FileContentsAction::Selected(entry));
                        } else {
                            // Below the listing, so there is no row to
                            // describe — but the status line still has to
                            // stop saying what the *last* pick was.
                            out.push(FileContentsAction::Restated);
                        }
                    }
                    // The map was showing something that is not there any
                    // more — deleted by something other than this app since
                    // the folder was measured. It has already dropped it; the
                    // listing should hear about it too.
                    TreemapAction::Vanished(path) => {
                        self.selected.remove(&path);
                        out.push(FileContentsAction::Restated);
                    }
                    TreemapAction::FilterCleared => {
                        out.push(FileContentsAction::MapFilterCleared);
                    }
                    // A secondary click that stayed a click: the menu opens
                    // exactly as it would have on the press, only now it is
                    // certain no pan was meant.
                    TreemapAction::Context(at) => {
                        self.open_context(cx, at);
                    }
                    TreemapAction::None => {}
                    }
                }
            }
        }
        out
    }

    /// A press landed on the row at `position`.
    fn hit(
        &mut self,
        cx: &mut Cx,
        position: usize,
        open: bool,
        modifiers: KeyModifiers,
    ) -> FileContentsAction {
        // A press anywhere else ends an inline rename, exactly as clicking
        // away from a field does in every file manager.
        if self.renaming.is_some() && self.rename_position() != Some(position) {
            self.cancel_rename(cx);
        }
        self.click(cx, position, modifiers);
        let entry = self.rows[position].entry.clone();
        if open {
            FileContentsAction::Open(entry)
        } else {
            FileContentsAction::Selected(entry)
        }
    }

    /// A release; when it happened far enough from the press it is a drag,
    /// and the shell gets to decide what is under it.
    fn drop(&mut self, out: &mut Vec<FileContentsAction>, at: DVec2) {
        let Some(from) = self.press_at.take() else {
            return;
        };
        if (at - from).length() < 12.0 {
            return;
        }
        let paths: Vec<PathBuf> = self
            .selected_entries()
            .into_iter()
            .map(|e| e.path)
            .collect();
        if !paths.is_empty() {
            out.push(FileContentsAction::Dropped(paths, at));
        }
    }

    /// Resolve a secondary press into a menu target, selecting what it landed
    /// on when that is not already part of the selection — which is what every
    /// file manager does, and what keeps a right-click on one of five selected
    /// files from throwing the other four away.
    fn open_context(&mut self, cx: &mut Cx, at: DVec2) {
        let mut off_list = None;
        let position = match self.mode {
            ViewMode::Treemap => {
                let path = self.treemap(cx).path_at(at);
                let position = path
                    .as_ref()
                    .and_then(|p| self.rows.iter().position(|r| r.entry.path == *p));
                // Most of the map is below the folder being listed, so most
                // right-clicks land on something the rows do not know about.
                // The entry is read straight off the disk instead — a menu
                // that refuses to act on what the map is showing would be
                // useless for the one job the map exists for.
                if position.is_none() {
                    off_list = path.as_deref().and_then(crate::model::entry_at);
                }
                position
            }
            _ => self
                .hit_rects
                .iter()
                .find(|hit| hit.rect.contains(at))
                .map(|hit| hit.position),
        };
        if let Some(position) = position {
            let path = self.rows[position].entry.path.clone();
            if !self.selected.contains(&path) {
                self.selected.clear();
                self.selected.insert(path.clone());
                self.anchor = Some(path);
                self.sync_grid_selection(cx);
                self.view.redraw(cx);
            }
        }
        self.pending_context = Some(ContextHit {
            at,
            position,
            off_list,
        });
        let uid = self.widget_uid();
        cx.widget_action(uid, ContentsPing::Ping);
    }

    /// Open or close the folder on `row` in the List tree.
    fn toggle_expand(&mut self, cx: &mut Cx, row: usize) -> Option<FileContentsAction> {
        let path = self.rows.get(row)?.entry.path.clone();
        if !self.rows[row].expandable {
            return None;
        }
        if self.expanded.remove(&path) {
            self.reorder(cx);
            return None;
        }
        self.expanded.insert(path.clone());
        self.reorder(cx);
        if self.children.contains_key(&path) || !self.pending.insert(path.clone()) {
            return None;
        }
        Some(FileContentsAction::NeedChildren(path))
    }
}

/// `ids!` paths for the row template's cells, indexed by column.
const CELL_IDS: [&[LiveId]; GRID_MAX_COLUMNS] = [
    ids!(c0),
    ids!(c1),
    ids!(c2),
    ids!(c3),
    ids!(c4),
    ids!(c5),
    ids!(c6),
    ids!(c7),
    ids!(c8),
    ids!(c9),
    ids!(c10),
    ids!(c11),
];

impl Widget for FileContents {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // The row template has a fixed cell count; how many of them are used
        // follows the width we are actually given and the icon size.
        let width = cx.turtle().rect().size.x;
        if width > 1.0 {
            self.last_width = width;
        }
        let tile_width = ZOOM_LEVELS[self.zoom.min(ZOOM_LEVELS.len() - 1)].0;
        self.grid_columns = Self::columns_for(self.last_width.max(tile_width), tile_width);
        self.body_rect = cx.turtle().rect();
        self.hit_rects.clear();
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            // Only the page for `mode` is visible, so whatever list this is,
            // `mode` says which one.
            match self.mode {
                ViewMode::Icons => {
                    if let Some(mut list) = step.borrow_mut::<PortalList>() {
                        self.draw_icons(cx, &mut list);
                    }
                }
                ViewMode::Compact => {
                    if let Some(mut list) = step.borrow_mut::<PortalList>() {
                        self.draw_compact(cx, &mut list);
                    }
                }
                ViewMode::List => {
                    if let Some(mut grid) = step.borrow_mut::<DataGrid>() {
                        self.draw_list(cx, &mut grid);
                    }
                }
                ViewMode::Treemap => {}
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // A context menu has to know what is under the pointer even when the
        // tile, row or grid under it swallows the press — and each of the four
        // views swallows it differently. The raw button event is the one place
        // where the answer is the same for all of them, so it is read here and
        // resolved against the rectangles the last draw recorded.
        if let Event::MouseDown(press) = event {
            let secondary = press.button.is_secondary()
                || (press.button.is_primary() && press.modifiers.control);
            // Not on the treemap: there a secondary press may be the start of
            // a right-drag pan, so the map itself decides on release and
            // reports a clean click as `TreemapAction::Context`.
            if secondary && self.mode != ViewMode::Treemap && self.body_rect.contains(press.abs) {
                self.open_context(cx, press.abs);
            }
        }
        self.view.handle_event(cx, event, scope);
        // An editor that has not been drawn since it was revealed has no area,
        // and focusing an empty area focuses nothing — so this waits for the
        // frame that gives it one, rather than assuming the next frame does.
        if self.rename_focus.is_event(event).is_some() {
            let field = self
                .rename_position()
                .and_then(|position| self.rename_editor(cx, position));
            let drawn = field
                .as_ref()
                .map(|f| f.area().rect(cx).size.x >= 1.0)
                .unwrap_or(false);
            match field.filter(|_| drawn) {
                Some(field) => {
                    self.rename_tries = 0;
                    field.take_key_focus(cx);
                    if let Some(mut inner) = field.borrow_mut() {
                        inner.select_all(cx);
                    }
                }
                None if self.renaming.is_some() && self.rename_tries < 8 => {
                    self.rename_tries += 1;
                    self.rename_focus = cx.new_next_frame();
                }
                None => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_follow_the_width_and_the_icon_size() {
        let medium = ZOOM_LEVELS[DEFAULT_ZOOM].0;
        assert_eq!(FileContents::columns_for(0.0, medium), 1);
        assert_eq!(FileContents::columns_for(200.0, medium), 1);
        assert_eq!(FileContents::columns_for(700.0, medium), 5);
        // Never more cells than the row template carries.
        assert_eq!(
            FileContents::columns_for(9000.0, medium),
            GRID_MAX_COLUMNS
        );
        // A bigger icon means fewer of them across the same window.
        let biggest = ZOOM_LEVELS[ZOOM_LEVELS.len() - 1].0;
        assert!(
            FileContents::columns_for(1200.0, biggest)
                < FileContents::columns_for(1200.0, medium)
        );
    }

    #[test]
    fn the_zoom_levels_only_ever_grow() {
        for pair in ZOOM_LEVELS.windows(2) {
            assert!(pair[1].0 > pair[0].0, "tile width");
            assert!(pair[1].1 > pair[0].1, "row height");
            assert!(pair[1].2 > pair[0].2, "thumbnail height");
        }
        assert!(DEFAULT_ZOOM < ZOOM_LEVELS.len());
    }
}
