//! The pdf viewer: the document, the thumbnail strip, and everything that
//! moves them.
//!
//! The pages themselves are drawn by the stock `PdfView` from
//! widgets/src/pdf_view.rs — a `PortalList` of `PdfPageView`s, each of which
//! replays its page's content-stream ops into `DrawVector` geometry and
//! `DrawText` runs. That gives continuous vertical scrolling, the gap
//! between pages, drag and wheel scrolling, and (because `PortalList` is
//! virtual) drawing work proportional to what is on screen rather than to
//! the document.
//!
//! What this widget adds: an off-thread loader so a big document does not
//! freeze the UI while it parses (see `loader.rs`), the zoom / fit-width /
//! fit-page maths, zoom around the cursor, page-at-a-time navigation,
//! horizontal panning once the zoom is wider than the window, the thumbnail
//! strip, and the status the chrome reads.

use crate::loader::{file_name, spawn_load, PdfLoadEvent, PdfLoadMsg};
use crate::thumbs::{
    MpPdfThumbsAction, MpPdfThumbsRef, MpPdfThumbsWidgetExt, STRIP_WIDTH,
};
use makepad_widgets::makepad_platform::thread::{CancellationToken, TaskHandle, ToUIReceiver};
use makepad_widgets::*;
use std::path::{Path, PathBuf};

/// The dark margin left of, right of and around the paper.
const GUTTER: f64 = 12.0;
/// The gap between pages, matching the `Page` template's bottom margin in
/// widgets/src/pdf_view.rs.
const PAGE_GAP: f64 = 8.0;
/// One press of Cmd+ or Cmd-.
const ZOOM_STEP: f64 = 1.25;
const MIN_ZOOM: f64 = 0.1;
const MAX_ZOOM: f64 = 8.0;
/// How much of the previous screen stays visible when paging within a page
/// that is taller than the window.
const PAGE_OVERLAP: f64 = 40.0;
/// One arrow-key press.
const LINE_STEP: f64 = 60.0;
const PAN_STEP: f64 = 60.0;
/// The page size assumed for a page that has not been parsed yet, so the
/// scroll extent is right from the first frame.
const NOMINAL_PAGE: (f64, f64) = (612.0, 792.0);

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MpPdfViewBase = #(MpPdfView::register_widget(vm))

    mod.widgets.MpPdfView = set_type_default() do mod.widgets.MpPdfViewBase{
        width: Fill
        height: Fill
        flow: Right

        thumbs := mod.widgets.MpPdfThumbs{
            visible: false
        }

        doc := SolidView{
            width: Fill
            height: Fill
            flow: Down
            // The gutter the paper floats on.
            draw_bg +: { color: mod.mpp.bg_dark }

            pdf := PdfView{
                width: Fill
                height: Fill
            }
        }
    }
}

/// Which zoom rule is in charge.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum FitMode {
    /// The page is as wide as the window allows (Cmd+0). The default.
    #[default]
    Width,
    /// A whole page fits in the window.
    Page,
    /// An explicit zoom the user chose.
    Free,
}

/// Everything the chrome shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdfStatus {
    pub name: String,
    /// 1-based, 0 while there is no document.
    pub page: usize,
    pub page_count: usize,
    /// Pages parsed so far; equals `page_count` once loading finishes.
    pub loaded: usize,
    pub zoom_pct: i32,
    pub fit_width: bool,
    pub fit_page: bool,
    pub thumbs: bool,
    pub error: Option<String>,
}

/// What the viewer tells the app shell.
#[derive(Clone, Debug, Default)]
pub enum MpPdfAction {
    Status(PdfStatus),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct MpPdfView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,

    /// Parsed pages arriving from the loader thread. `ToUISender::send`
    /// pokes the UI signal, so they land on `Event::Signal` with no polling.
    #[rust]
    loader: ToUIReceiver<PdfLoadMsg>,
    #[rust]
    loader_task: Option<TaskHandle<()>>,
    #[rust]
    loader_cancel: CancellationToken,
    #[rust]
    loader_generation: u64,
    #[rust]
    path: Option<PathBuf>,
    /// Page boxes in PDF points, one per parsed page: the layout maths runs
    /// off these rather than walking back into the widget tree.
    #[rust]
    sizes: Vec<Vec2d>,
    #[rust]
    page_count: usize,
    #[rust]
    loaded: usize,

    #[rust]
    fit: FitMode,
    #[rust(1.0)]
    zoom: f64,
    #[rust]
    scroll_x: f64,

    #[rust]
    last_mouse: Vec2d,
    #[rust(true)]
    keys_enabled: bool,
    #[rust]
    thumbs_open: bool,
    #[rust]
    error: Option<String>,
    /// This process is a warm Quick Look viewer (`--preview`): Escape/Q
    /// hides the panel instead of ending the process, and an empty viewer
    /// says nothing rather than advertising the command line. See
    /// `preview.rs`.
    #[rust]
    preview: bool,
    /// The last status handed to the shell; the next one is only sent when
    /// it differs, so drawing does not loop on its own status updates.
    #[rust]
    status: PdfStatus,
}

impl Widget for MpPdfView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // Cmd+wheel is a zoom, not a scroll, and Shift+wheel pans a page
        // that is wider than the window: claim both before the list sees
        // them.
        if let Event::Scroll(se) = event {
            let inside = self.viewport(cx).contains(se.abs);
            if inside && se.modifiers.is_primary() && se.scroll.y != 0.0 {
                let factor = (1.0 - se.scroll.y * 0.01).clamp(0.5, 1.5);
                self.zoom_around(cx, self.zoom * factor, se.abs);
                se.handled_x.set(true);
                se.handled_y.set(true);
            } else if inside && se.modifiers.shift && se.scroll.y != 0.0 {
                self.pan_by(cx, se.scroll.y);
                se.handled_x.set(true);
                se.handled_y.set(true);
            }
        }

        self.view.handle_event(cx, event, scope);

        match event {
            Event::Signal => self.drain_loader(cx),
            Event::KeyDown(ke) if self.keys_enabled => self.handle_key(cx, ke),
            Event::MouseMove(me) => self.last_mouse = me.abs,
            Event::Actions(actions) => {
                let thumbs = self.view.mp_pdf_thumbs(cx, ids!(thumbs));
                for action in actions {
                    let Some(widget_action) = action.as_widget_action() else {
                        continue;
                    };
                    if widget_action.widget_uid != thumbs.widget_uid() {
                        continue;
                    }
                    if let MpPdfThumbsAction::Goto(page) = widget_action.cast() {
                        self.go_to_page(cx, page);
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        // Both of these need the drawn viewport, so they run after it: the
        // fit zoom is a function of the window, and scrolling moves the
        // current page without any of our code running. Changing the zoom
        // asks for one more frame, which then settles.
        // `cx: &mut Cx2d` derefs through `CxDraw` down to `Cx`.
        self.apply_fit(&mut **cx);
        self.emit_status(&mut **cx);
        step
    }
}

impl MpPdfView {
    /// Open `path`. Reading and parsing happen on a worker thread; pages
    /// appear as they arrive.
    pub fn open(&mut self, cx: &mut Cx, path: &Path) {
        self.cancel_load();
        self.loader_generation = self.loader_generation.wrapping_add(1);
        self.path = Some(path.to_path_buf());
        self.sizes.clear();
        self.page_count = 0;
        self.loaded = 0;
        self.error = None;
        self.scroll_x = 0.0;
        self.document(cx).begin_load(cx, 0);
        self.thumb_strip(cx).set_pages(cx, Vec::new());
        self.loader = ToUIReceiver::default();
        self.loader_cancel = CancellationToken::new();
        match spawn_load(
            &cx.task_pool(),
            path.to_path_buf(),
            self.loader.sender(),
            self.loader_generation,
            self.loader_cancel.clone(),
        ) {
            Ok(task) => self.loader_task = Some(task),
            Err(error) => self.error = Some(format!("could not queue PDF load: {error}")),
        }
        self.list(cx).set_first_id_and_scroll(0, 0.0);
        cx.redraw_all();
    }

    /// Quick Look v2: the panel hid, so drop the document — the parsed
    /// pages, the page cache the list draws from, the thumbnail strip — and
    /// idle blank until the next `open`. A fresh receiver also drops the
    /// loader's sender, which is how a parse still running for the old file
    /// learns to stop.
    pub fn unload(&mut self, cx: &mut Cx) {
        self.cancel_load();
        self.loader_generation = self.loader_generation.wrapping_add(1);
        self.loader = ToUIReceiver::default();
        self.path = None;
        self.sizes.clear();
        self.page_count = 0;
        self.loaded = 0;
        self.error = None;
        self.scroll_x = 0.0;
        self.zoom = 1.0;
        self.fit = FitMode::default();
        self.document(cx).begin_load(cx, 0);
        self.document(cx).set_scroll_x(cx, 0.0);
        self.thumb_strip(cx).set_pages(cx, Vec::new());
        self.thumbs_open = false;
        self.view.widget(cx, ids!(thumbs)).set_visible(cx, false);
        self.list(cx).set_first_id_and_scroll(0, 0.0);
        cx.redraw_all();
    }

    /// Tell the view it belongs to a warm Quick Look panel, where Escape/Q
    /// hides the panel rather than ending the process.
    pub fn set_preview(&mut self, preview: bool) {
        self.preview = preview;
    }

    /// Escape / Q: hide the panel when hosted as a Quick Look, quit
    /// otherwise.
    fn close(&mut self, cx: &mut Cx) {
        if crate::preview::hide_panel(cx, self.preview) {
            return;
        }
        cx.quit();
    }

    /// The shell turns key handling off while the page field has focus:
    /// typing "12" there must not also be two zoom commands.
    pub fn set_keys_enabled(&mut self, enabled: bool) {
        self.keys_enabled = enabled;
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }

    // ---- commands the chrome drives ----

    pub fn zoom_in(&mut self, cx: &mut Cx) {
        let anchor = self.zoom_anchor(cx);
        self.zoom_around(cx, self.zoom * ZOOM_STEP, anchor);
    }

    pub fn zoom_out(&mut self, cx: &mut Cx) {
        let anchor = self.zoom_anchor(cx);
        self.zoom_around(cx, self.zoom / ZOOM_STEP, anchor);
    }

    pub fn set_fit(&mut self, cx: &mut Cx, fit: FitMode) {
        self.fit = fit;
        self.scroll_x = 0.0;
        self.document(cx).set_scroll_x(cx, 0.0);
        cx.redraw_all();
    }

    /// 100%: one PDF point per pixel.
    pub fn actual_size(&mut self, cx: &mut Cx) {
        self.fit = FitMode::Free;
        self.set_zoom(cx, 1.0);
    }

    pub fn toggle_thumbs(&mut self, cx: &mut Cx) {
        self.thumbs_open = !self.thumbs_open;
        self.view
            .widget(cx, ids!(thumbs))
            .set_visible(cx, self.thumbs_open);
        if self.thumbs_open {
            let pages = self.document(cx).borrow().map(|d| d.pages().to_vec());
            if let Some(pages) = pages {
                self.thumb_strip(cx).set_pages(cx, pages);
            }
        }
        cx.redraw_all();
    }

    pub fn thumbs_open(&self) -> bool {
        self.thumbs_open
    }

    /// Jump so that page `page` (0-based) starts at the top of the view.
    pub fn go_to_page(&mut self, cx: &mut Cx, page: usize) {
        if self.page_count == 0 {
            return;
        }
        let page = page.min(self.page_count - 1);
        self.list(cx).set_first_id_and_scroll(page, 0.0);
        cx.redraw_all();
    }

    /// PageDown / Space: down one screen inside a tall page, else to the
    /// top of the next one.
    pub fn page_forward(&mut self, cx: &mut Cx) {
        if self.page_count == 0 {
            return;
        }
        let list = self.list(cx);
        let first = list.first_id();
        let scroll = list.scroll_position();
        let view_h = self.viewport(cx).size.y;
        let item_h = self.item_height(first);
        if scroll + item_h > view_h + 1.0 {
            list.set_first_id_and_scroll(first, scroll - (view_h - PAGE_OVERLAP).max(1.0));
        } else if first + 1 < self.page_count {
            list.set_first_id_and_scroll(first + 1, 0.0);
        }
        cx.redraw_all();
    }

    /// PageUp / Shift+Space: the mirror image, landing on the *bottom* of
    /// the previous page rather than its top.
    pub fn page_back(&mut self, cx: &mut Cx) {
        if self.page_count == 0 {
            return;
        }
        let list = self.list(cx);
        let first = list.first_id();
        let scroll = list.scroll_position();
        let view_h = self.viewport(cx).size.y;
        if scroll < -1.0 {
            let target = (scroll + (view_h - PAGE_OVERLAP).max(1.0)).min(0.0);
            list.set_first_id_and_scroll(first, target);
        } else if first > 0 {
            let item_h = self.item_height(first - 1);
            list.set_first_id_and_scroll(first - 1, (view_h - item_h).min(0.0));
        }
        cx.redraw_all();
    }

    /// The page the reader is looking at, 0-based: the one covering a point
    /// a little below the top of the view.
    pub fn current_page(&self, cx: &Cx) -> usize {
        if self.page_count == 0 {
            return 0;
        }
        let list = self.list(cx);
        let mut index = list.first_id().min(self.page_count - 1);
        let mut top = list.scroll_position();
        let probe = self.viewport(cx).size.y * 0.4;
        while index + 1 < self.page_count {
            let height = self.item_height(index);
            if top + height > probe {
                break;
            }
            top += height;
            index += 1;
        }
        index
    }

    // ---- loading ----

    fn cancel_load(&mut self) {
        self.loader_cancel.cancel();
        if let Some(task) = self.loader_task.take() {
            task.cancel();
        }
    }

    fn drain_loader(&mut self, cx: &mut Cx) {
        loop {
            let Ok(message) = self.loader.try_recv() else {
                break;
            };
            if message.generation != self.loader_generation {
                continue;
            }
            match message.event {
                PdfLoadEvent::Opened {
                    page_count,
                    open_ms,
                } => {
                    self.page_count = page_count;
                    self.document(cx).begin_load(cx, page_count);
                    log!(
                        "pdf: {} opened, {} pages, {} ms",
                        self.name(),
                        page_count,
                        open_ms
                    );
                }
                PdfLoadEvent::Pages { pages } => {
                    for page in &pages {
                        self.sizes.push(page.size());
                    }
                    self.loaded = self.sizes.len();
                    self.document(cx).append_pages(cx, pages);
                    if self.thumbs_open {
                        let shared = self.document(cx).borrow().map(|d| d.pages().to_vec());
                        if let Some(shared) = shared {
                            self.thumb_strip(cx).set_pages(cx, shared);
                        }
                    }
                }
                PdfLoadEvent::Done { total_ms } => {
                    log!(
                        "pdf: {} parsed, {} pages in {} ms",
                        self.name(),
                        self.loaded,
                        total_ms
                    );
                }
                PdfLoadEvent::Failed { message } => {
                    log!("pdf: {}", message);
                    self.error = Some(message);
                }
            }
            cx.redraw_all();
        }
        if self.loader_task.as_ref().is_some_and(TaskHandle::is_finished) {
            let mut task = self.loader_task.take().unwrap();
            if let Some(Err(error)) = task.try_take() {
                if !matches!(error, makepad_widgets::makepad_platform::thread::TaskError::Cancelled)
                {
                    self.error = Some(format!("PDF load failed: {error}"));
                    cx.redraw_all();
                }
            }
        }
    }

    // ---- zoom and fit ----

    /// Re-derive the zoom from the window when a fit mode is in charge. Runs
    /// every draw, so a resize follows the window with no resize handler.
    fn apply_fit(&mut self, cx: &mut Cx) {
        let viewport = self.viewport(cx);
        if viewport.size.x <= 0.0 || viewport.size.y <= 0.0 {
            return;
        }
        let zoom = match self.fit {
            FitMode::Free => self.zoom,
            FitMode::Width | FitMode::Page => {
                let page = self.page_size(self.current_page(cx));
                let by_width = (viewport.size.x - 2.0 * GUTTER).max(1.0) / page.x;
                if self.fit == FitMode::Width {
                    by_width
                } else {
                    by_width.min((viewport.size.y - 2.0 * GUTTER).max(1.0) / page.y)
                }
            }
        }
        .clamp(MIN_ZOOM, MAX_ZOOM);

        if (zoom - self.zoom).abs() > 1e-9 {
            self.zoom = zoom;
        }
        // Always assert it: the document view starts at its script default.
        self.document(cx).set_zoom(cx, self.zoom);
        let clamped = self.clamp_scroll_x(cx, self.scroll_x);
        if (clamped - self.scroll_x).abs() > 1e-9 {
            self.scroll_x = clamped;
        }
        self.document(cx).set_scroll_x(cx, self.scroll_x);
    }

    fn set_zoom(&mut self, cx: &mut Cx, zoom: f64) {
        let anchor = self.zoom_anchor(cx);
        self.zoom_around(cx, zoom, anchor);
    }

    /// Zoom to `zoom`, keeping the document point under `anchor` where it
    /// is — the standard zoom-to-cursor, done in the list's own coordinates
    /// (which item, how far into it) instead of a single scroll offset.
    fn zoom_around(&mut self, cx: &mut Cx, zoom: f64, anchor: Vec2d) {
        let new_zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let old_zoom = self.zoom;
        if (new_zoom - old_zoom).abs() < 1e-9 {
            return;
        }
        let ratio = new_zoom / old_zoom;
        let viewport = self.viewport(cx);
        let list = self.list(cx);

        // Vertical: find the page under the cursor and how far into it.
        let cursor_y = anchor.y - viewport.pos.y;
        let mut index = list.first_id().min(self.page_count.saturating_sub(1));
        let mut top = list.scroll_position();
        while index + 1 < self.page_count {
            let height = self.item_height(index);
            if top + height > cursor_y {
                break;
            }
            top += height;
            index += 1;
        }
        let into_page = cursor_y - top;

        // Horizontal: the page is centered, shifted by scroll_x.
        let page_w_old = self.page_size(index).x * old_zoom;
        let page_w_new = page_w_old * ratio;
        let cursor_x = anchor.x - viewport.pos.x;
        let page_left = (viewport.size.x - page_w_old) * 0.5 - self.scroll_x;
        let into_page_x = cursor_x - page_left;

        self.zoom = new_zoom;
        self.fit = FitMode::Free;
        let wanted = (viewport.size.x - page_w_new) * 0.5 + into_page_x * ratio - cursor_x;
        self.scroll_x = self.clamp_scroll_x_for(viewport.size.x, page_w_new, wanted);

        self.document(cx).set_zoom(cx, new_zoom);
        self.document(cx).set_scroll_x(cx, self.scroll_x);
        list.set_first_id_and_scroll(index, cursor_y - into_page * ratio);
        cx.redraw_all();
    }

    fn pan_by(&mut self, cx: &mut Cx, delta: f64) {
        let wanted = self.scroll_x + delta;
        let clamped = self.clamp_scroll_x(cx, wanted);
        if (clamped - self.scroll_x).abs() < 1e-9 {
            return;
        }
        self.scroll_x = clamped;
        self.document(cx).set_scroll_x(cx, clamped);
        cx.redraw_all();
    }

    fn scroll_by(&mut self, cx: &mut Cx, delta: f64) {
        let list = self.list(cx);
        list.set_first_id_and_scroll(list.first_id(), list.scroll_position() + delta);
        cx.redraw_all();
    }

    fn clamp_scroll_x(&self, cx: &Cx, wanted: f64) -> f64 {
        let viewport = self.viewport(cx);
        let page_w = self.page_size(self.current_page(cx)).x * self.zoom;
        self.clamp_scroll_x_for(viewport.size.x, page_w, wanted)
    }

    /// A page narrower than the window cannot pan; a wider one pans until
    /// each edge reaches the window edge.
    fn clamp_scroll_x_for(&self, view_w: f64, page_w: f64, wanted: f64) -> f64 {
        let limit = ((page_w - view_w) * 0.5 + GUTTER).max(0.0);
        wanted.clamp(-limit, limit)
    }

    fn zoom_anchor(&self, cx: &Cx) -> Vec2d {
        let viewport = self.viewport(cx);
        if viewport.contains(self.last_mouse) {
            self.last_mouse
        } else {
            viewport.center()
        }
    }

    // ---- keys ----

    fn handle_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        let command = ke.modifiers.is_primary();
        match ke.key_code {
            KeyCode::Escape | KeyCode::KeyQ => self.close(cx),
            KeyCode::PageDown => self.page_forward(cx),
            KeyCode::PageUp => self.page_back(cx),
            KeyCode::Space => {
                if ke.modifiers.shift {
                    self.page_back(cx)
                } else {
                    self.page_forward(cx)
                }
            }
            KeyCode::Home => self.go_to_page(cx, 0),
            KeyCode::End => self.go_to_page(cx, self.page_count.saturating_sub(1)),
            KeyCode::ArrowDown => self.scroll_by(cx, -LINE_STEP),
            KeyCode::ArrowUp => self.scroll_by(cx, LINE_STEP),
            KeyCode::ArrowRight => self.pan_by(cx, PAN_STEP),
            KeyCode::ArrowLeft => self.pan_by(cx, -PAN_STEP),
            KeyCode::Equals | KeyCode::NumpadAdd if command => self.zoom_in(cx),
            KeyCode::Minus | KeyCode::NumpadSubtract if command => self.zoom_out(cx),
            KeyCode::Key0 if command => self.set_fit(cx, FitMode::Width),
            KeyCode::Key1 if command => self.actual_size(cx),
            KeyCode::Key2 if command => self.set_fit(cx, FitMode::Page),
            KeyCode::KeyT if command && ke.modifiers.shift => self.toggle_thumbs(cx),
            _ => {}
        }
    }

    // ---- geometry helpers ----

    /// The page box of page `index` in points. Pages the loader has not
    /// reached yet stand in as the last known size, so the scroll extent of
    /// a document being read is stable rather than guessed per page.
    fn page_size(&self, index: usize) -> Vec2d {
        self.sizes
            .get(index)
            .copied()
            .or_else(|| self.sizes.last().copied())
            .unwrap_or(dvec2(NOMINAL_PAGE.0, NOMINAL_PAGE.1))
    }

    /// One list item: the paper plus the gap under it.
    fn item_height(&self, index: usize) -> f64 {
        self.page_size(index).y * self.zoom + PAGE_GAP
    }

    fn viewport(&self, cx: &Cx) -> Rect {
        self.view.widget(cx, ids!(doc)).area().rect(cx)
    }

    fn document(&self, cx: &Cx) -> PdfViewRef {
        self.view.pdf_view(cx, ids!(doc.pdf))
    }

    fn thumb_strip(&self, cx: &Cx) -> MpPdfThumbsRef {
        self.view.mp_pdf_thumbs(cx, ids!(thumbs))
    }

    fn list(&self, cx: &Cx) -> PortalListRef {
        self.view.portal_list(cx, ids!(doc.pdf.list))
    }

    fn name(&self) -> String {
        match self.path.as_deref() {
            Some(path) => file_name(path),
            // A warm preview viewer with nothing loaded has no name to
            // give; standalone, the empty state names itself.
            None if self.preview => String::new(),
            None => "no document".to_string(),
        }
    }

    fn emit_status(&mut self, cx: &mut Cx) {
        let page = self.current_page(cx);
        let status = PdfStatus {
            name: self.name(),
            page: if self.page_count == 0 { 0 } else { page + 1 },
            page_count: self.page_count,
            loaded: self.loaded,
            zoom_pct: (self.zoom * 100.0).round() as i32,
            fit_width: self.fit == FitMode::Width,
            fit_page: self.fit == FitMode::Page,
            thumbs: self.thumbs_open,
            error: self.error.clone(),
        };
        if status == self.status {
            return;
        }
        if self.thumbs_open && status.page > 0 {
            self.thumb_strip(cx).set_current(cx, page);
        }
        self.status = status.clone();
        cx.widget_action(self.widget_uid(), MpPdfAction::Status(status));
    }
}

/// Width of the thumbnail strip, re-exported so the shell can reason about
/// the layout without reaching into the strip's module.
pub const THUMB_STRIP_WIDTH: f64 = STRIP_WIDTH;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_modes_are_exclusive() {
        assert_ne!(FitMode::Width, FitMode::Page);
        assert_eq!(FitMode::default(), FitMode::Width);
    }
}
