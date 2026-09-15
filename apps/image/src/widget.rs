//! The image viewer widget: fit-to-window / zoom / pan over a single
//! `Image`, plus Left/Right navigation through the sorted sibling images in
//! the same folder. Fully self-positions the inner `Image` every frame via
//! an absolute `Walk` (`Walk::abs_rect`), so it owns zoom/pan math directly
//! instead of fighting the stock `ImageFit` layout modes.

use makepad_widgets::image::Image;
use makepad_widgets::*;
use std::path::{Path, PathBuf};

/// How much a single `+`/`-` key press or wheel notch scales the zoom.
const ZOOM_STEP: f64 = 1.25;
const MIN_ZOOM: f64 = 0.05;
const MAX_ZOOM: f64 = 32.0;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.MpImageViewBase = #(MpImageView::register_widget(vm))

    mod.widgets.MpImageView = set_type_default() do mod.widgets.MpImageViewBase{
        width: Fill
        height: Fill
        img: Image{}
    }
}

#[derive(Clone, Debug, Default)]
pub enum MpImageAction {
    /// filename, pixel width, pixel height (0 while still decoding), zoom %
    Status { name: String, width: u32, height: u32, zoom_pct: i32 },
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct MpImageView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    img: WidgetRef,

    #[rust]
    area: Area,
    #[rust]
    rect: Rect,

    #[rust]
    dir_paths: Vec<PathBuf>,
    #[rust]
    index: usize,
    /// Every path this viewer has actually had decoded. The image cache is
    /// process-wide and sheds entries only past 512 of them, counted
    /// without regard to their size — dialing through a folder of
    /// 24-megapixel photographs is 96 MB a picture and the cap never
    /// bites. So the viewer remembers what it asked for and hands it back
    /// (`evict_cached`) the moment it stops showing it.
    #[rust]
    cached: Vec<PathBuf>,
    #[rust]
    natural_size: Option<(f64, f64)>,

    #[rust(true)]
    fit_mode: bool,
    #[rust(1.0)]
    zoom: f64,
    #[rust]
    pan: Vec2d,

    #[rust]
    dragging: bool,
    #[rust]
    drag_start_mouse: Vec2d,
    #[rust]
    drag_start_pan: Vec2d,
    #[rust]
    last_mouse: Vec2d,

    /// Set when a status update was requested before the first real layout
    /// pass gave us a non-zero `rect` (e.g. the very first frame): the fit
    /// percentage needs an actual container size, so `draw_walk` re-fires
    /// the status once `rect` is valid.
    #[rust]
    status_pending: bool,

    /// This process is a warm Quick Look viewer (`--preview`): Escape/Q
    /// hides the panel instead of ending the process. See `preview.rs`.
    #[rust]
    preview: bool,
}

impl Widget for MpImageView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.img.handle_event(cx, event, scope);

        // The image decodes asynchronously; its completion arrives as an
        // Event::Actions entry the line above already forwarded. Pick the
        // resulting pixel size up here (this runs on every event, not just
        // draws, so the status line updates the moment it's ready).
        let mut just_loaded = false;
        if self.natural_size.is_none() {
            if let Some(image) = self.img.borrow_mut::<Image>() {
                if let Some((w, h)) = image.size_in_pixels(cx) {
                    self.natural_size = Some((w as f64, h as f64));
                    just_loaded = true;
                }
            }
        }
        if just_loaded {
            self.emit_status(cx);
            cx.redraw_all();
        }

        match event {
            Event::KeyDown(ke) => self.handle_key(cx, ke),
            Event::Scroll(se) if self.rect.contains(se.abs) && se.scroll.y != 0.0 => {
                let factor = (1.0 - se.scroll.y * 0.01).clamp(0.5, 1.5);
                let anchor = se.abs;
                self.apply_zoom(cx, self.current_display_scale() * factor, anchor);
                se.handled_x.set(true);
                se.handled_y.set(true);
            }
            Event::MouseDown(me) if me.button.is_primary() && self.rect.contains(me.abs) => {
                self.dragging = true;
                self.drag_start_mouse = me.abs;
                self.drag_start_pan = self.pan;
            }
            Event::MouseMove(me) => {
                self.last_mouse = me.abs;
                if self.dragging {
                    self.pan = self.drag_start_pan + (me.abs - self.drag_start_mouse);
                    cx.redraw_all();
                }
            }
            Event::MouseUp(_) => {
                self.dragging = false;
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, Layout::default());
        let rect = cx.turtle().rect();
        self.rect = rect;

        if self.status_pending && rect.size.x > 0.0 && rect.size.y > 0.0 {
            // `cx: &mut Cx2d` derefs through `CxDraw` down to `Cx`.
            self.emit_status(&mut **cx);
        }

        // An unloaded warm viewer draws nothing at all: the window's clear
        // color is the blank themed panel, and a textureless `Image` has no
        // business painting a quad over it.
        if self.is_loaded() && rect.size.x > 0.0 && rect.size.y > 0.0 {
            let natural = match self.natural_size {
                Some((w, h)) if w > 0.0 && h > 0.0 => (w, h),
                _ => (rect.size.x, rect.size.y),
            };
            let center = rect.center();

            let (size, pos) = if self.fit_mode || self.natural_size.is_none() {
                let scale = (rect.size.x / natural.0).min(rect.size.y / natural.1);
                let size = dvec2(natural.0 * scale, natural.1 * scale);
                (size, center - size * 0.5)
            } else {
                let size = dvec2(natural.0 * self.zoom, natural.1 * self.zoom);
                (size, center + self.pan - size * 0.5)
            };

            self.img
                .draw_walk_all(cx, scope, Walk::abs_rect(Rect { pos, size }));
        }

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

impl MpImageView {
    /// Opens `path`: scans its folder for sibling images (sorted), and loads it.
    ///
    /// A Quick-Look retarget lands here, once per arrow key: everything the
    /// last file cost goes back to the allocator before the new one is
    /// decoded, so dialing a folder holds one picture at a time instead of
    /// every picture the panel has ever shown.
    pub fn open(&mut self, cx: &mut Cx, path: &Path) {
        self.evict_cached(cx, Some(path));
        self.scan_dir(path);
        self.load_current(cx);
    }

    /// Quick Look v2: the panel hid, so drop the picture entirely — the
    /// decoded texture, any decode still in flight, and the folder listing —
    /// and idle blank until the next `open`. The process stays alive; that
    /// is what "warm" means.
    pub fn unload(&mut self, cx: &mut Cx) {
        if let Some(mut image) = self.img.borrow_mut::<Image>() {
            // Clearing the texture is also how a pending async decode is
            // cancelled: it drops `async_image_path`, so a late result for
            // the old file cannot land on a viewer showing nothing.
            ImageCacheImpl::set_texture(&mut *image, None, 0);
        }
        // Nothing is on screen now, so nothing is worth keeping decoded:
        // the widget just dropped its own handle, and this drops the
        // process-wide cache's. "Idle at near-zero cost" is not true of a
        // viewer still holding a folder of decoded photographs.
        self.evict_cached(cx, None);
        self.dir_paths.clear();
        self.index = 0;
        self.natural_size = None;
        self.fit_mode = true;
        self.zoom = 1.0;
        self.pan = dvec2(0.0, 0.0);
        self.dragging = false;
        self.emit_status(cx);
        cx.redraw_all();
    }

    /// True while a picture (or a decode of one) is on screen.
    pub fn is_loaded(&self) -> bool {
        !self.dir_paths.is_empty()
    }

    /// Take every image this viewer had decoded back out of the process-
    /// wide cache, except `keep` (the file it is about to show, which would
    /// only have to be decoded again). Paths nobody cached are ignored.
    fn evict_cached(&mut self, cx: &mut Cx, keep: Option<&Path>) {
        for path in evictable(&self.cached, keep) {
            evict_image_from_cache(cx, &path);
        }
        self.cached.clear();
        if let Some(keep) = keep {
            self.cached.push(keep.to_path_buf());
        }
    }

    /// Tell the view it belongs to a warm Quick Look panel, where Escape/Q
    /// hides the panel rather than ending the process.
    pub fn set_preview(&mut self, preview: bool) {
        self.preview = preview;
    }

    /// Escape / Q / Space: hide the panel when hosted as a Quick Look,
    /// quit otherwise.
    fn close(&mut self, cx: &mut Cx) {
        if crate::preview::hide_panel(cx, self.preview) {
            return;
        }
        cx.quit();
    }

    fn scan_dir(&mut self, path: &Path) {
        let dir = path.parent().filter(|p| !p.as_os_str().is_empty());
        let dir = dir.unwrap_or_else(|| Path::new("."));
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| is_image_path(p))
                    .collect()
            })
            .unwrap_or_default();
        entries.sort();

        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let index = entries
            .iter()
            .position(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == canon)
            .unwrap_or(0);

        if entries.is_empty() {
            entries.push(path.to_path_buf());
        }
        self.dir_paths = entries;
        self.index = index;
    }

    fn navigate(&mut self, cx: &mut Cx, delta: i32) {
        if self.dir_paths.len() < 2 {
            return;
        }
        let len = self.dir_paths.len() as i32;
        let idx = ((self.index as i32 + delta) % len + len) % len;
        self.index = idx as usize;
        self.load_current(cx);
    }

    fn load_current(&mut self, cx: &mut Cx) {
        let Some(path) = self.dir_paths.get(self.index).cloned() else {
            return;
        };
        self.natural_size = None;
        self.fit_mode = true;
        self.zoom = 1.0;
        self.pan = dvec2(0.0, 0.0);
        if let Some(mut image) = self.img.borrow_mut::<Image>() {
            let _ = image.load_image_file_by_path_async(cx, &path);
        }
        if !self.cached.contains(&path) {
            self.cached.push(path);
        }
        self.emit_status(cx);
        cx.redraw_all();
    }

    fn handle_key(&mut self, cx: &mut Cx, ke: &KeyEvent) {
        match ke.key_code {
            KeyCode::Escape | KeyCode::KeyQ | KeyCode::Space => self.close(cx),
            KeyCode::ArrowLeft => self.navigate(cx, -1),
            KeyCode::ArrowRight => self.navigate(cx, 1),
            KeyCode::Equals | KeyCode::NumpadAdd => {
                let anchor = self.zoom_anchor();
                self.apply_zoom(cx, self.current_display_scale() * ZOOM_STEP, anchor);
            }
            KeyCode::Minus | KeyCode::NumpadSubtract => {
                let anchor = self.zoom_anchor();
                self.apply_zoom(cx, self.current_display_scale() / ZOOM_STEP, anchor);
            }
            KeyCode::Key0 => {
                self.fit_mode = true;
                self.pan = dvec2(0.0, 0.0);
                self.emit_status(cx);
                cx.redraw_all();
            }
            KeyCode::Key1 => {
                self.fit_mode = false;
                self.zoom = 1.0;
                self.pan = dvec2(0.0, 0.0);
                self.emit_status(cx);
                cx.redraw_all();
            }
            _ => {}
        }
    }

    fn zoom_anchor(&self) -> Vec2d {
        if self.rect.contains(self.last_mouse) {
            self.last_mouse
        } else {
            self.rect.center()
        }
    }

    /// The scale currently on screen, whether it comes from the live fit
    /// computation or an explicit zoom factor.
    fn current_display_scale(&self) -> f64 {
        if self.fit_mode {
            if let Some((nw, nh)) = self.natural_size {
                if nw > 0.0 && nh > 0.0 && self.rect.size.x > 0.0 && self.rect.size.y > 0.0 {
                    return (self.rect.size.x / nw).min(self.rect.size.y / nh);
                }
            }
            1.0
        } else {
            self.zoom
        }
    }

    /// Zooms to `new_zoom`, keeping the image point under `anchor` fixed on
    /// screen (the standard "zoom to cursor" formula).
    fn apply_zoom(&mut self, cx: &mut Cx, new_zoom: f64, anchor: Vec2d) {
        let Some((nw, nh)) = self.natural_size else {
            return;
        };
        if nw <= 0.0 || nh <= 0.0 {
            return;
        }
        let old_zoom = self.current_display_scale();
        let new_zoom = new_zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let old_pan = if self.fit_mode { dvec2(0.0, 0.0) } else { self.pan };
        let old_size = dvec2(nw, nh) * old_zoom;
        let new_size = dvec2(nw, nh) * new_zoom;
        let center = self.rect.center();

        let frac_x = (anchor.x - center.x - old_pan.x + old_size.x * 0.5) / old_size.x.max(0.0001);
        let frac_y = (anchor.y - center.y - old_pan.y + old_size.y * 0.5) / old_size.y.max(0.0001);

        self.pan = dvec2(
            anchor.x - center.x + new_size.x * 0.5 - frac_x * new_size.x,
            anchor.y - center.y + new_size.y * 0.5 - frac_y * new_size.y,
        );
        self.zoom = new_zoom;
        self.fit_mode = false;
        self.emit_status(cx);
        cx.redraw_all();
    }

    fn emit_status(&mut self, cx: &mut Cx) {
        // The fit percentage needs a real container size; before the first
        // layout pass, `rect` is still zero-sized, so remember to re-emit
        // once `draw_walk` gives us one instead of reporting a bogus 100%.
        if self.rect.size.x <= 0.0 || self.rect.size.y <= 0.0 {
            self.status_pending = true;
            return;
        }
        self.status_pending = false;
        let name = self
            .dir_paths
            .get(self.index)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let zoom_pct = (self.current_display_scale() * 100.0).round() as i32;
        let (width, height) = self
            .natural_size
            .map(|(w, h)| (w as u32, h as u32))
            .unwrap_or((0, 0));
        cx.widget_action(
            self.uid,
            MpImageAction::Status { name, width, height, zoom_pct },
        );
    }
}

/// Which of the paths a viewer has had decoded are worth handing back:
/// all of them, except the one it is about to show (or is still showing).
fn evictable(cached: &[PathBuf], keep: Option<&Path>) -> Vec<PathBuf> {
    cached
        .iter()
        .filter(|p| Some(p.as_path()) != keep)
        .cloned()
        .collect()
}

fn is_image_path(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "qoi" | "ico"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_retarget_hands_back_everything_but_the_new_file() {
        let cached: Vec<PathBuf> = ["a.png", "b.png", "c.png"]
            .iter()
            .map(PathBuf::from)
            .collect();
        // Dialing on to a file never seen before: all three go back.
        assert_eq!(
            evictable(&cached, Some(Path::new("d.png"))),
            vec![
                PathBuf::from("a.png"),
                PathBuf::from("b.png"),
                PathBuf::from("c.png")
            ]
        );
        // Dialing back onto one still cached keeps exactly that one, so it
        // is not thrown away only to be decoded again a moment later.
        assert_eq!(
            evictable(&cached, Some(Path::new("b.png"))),
            vec![PathBuf::from("a.png"), PathBuf::from("c.png")]
        );
        // An unload keeps nothing: the panel shows nothing.
        assert_eq!(evictable(&cached, None), cached);
        // A viewer that has loaded nothing has nothing to hand back.
        assert!(evictable(&[], None).is_empty());
    }
}
