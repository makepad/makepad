//! RecentList — the folders and files an app opened, most recent first, one
//! click to open one again.
//!
//! Each row shows the place's name, its path (the home folder as `~`, cut
//! from the front when it does not fit, so the end that tells two places
//! apart stays) and when it was last opened. A click on a row reports its
//! path as [`RecentListAction::Open`]; the app opens it and remembers it
//! again, which puts it first.
//!
//! The widget is a view of the app's list. It does not store anything: an
//! app with its own settings file keeps the entries there (Scope), and an app
//! without one can keep them in a [`RecentFile`] — one line per place, the
//! time it was opened and its path, written on every open.

use crate::{
    family_api::{measure, sized},
    makepad_derive_widget::*,
    makepad_draw::*,
    widget::*,
};
use std::path::{Path, PathBuf};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawRecentRow::script_shader(vm)){
        ..mod.draw.DrawQuad
        color: #0000
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 5.0)
            sdf.fill(self.color)
            return sdf.result
        }
    }

    mod.widgets.RecentListBase = #(RecentList::register_widget(vm))

    /** The places an app opened, most recent first; a click opens one. */
    mod.widgets.RecentList = set_type_default() do mod.widgets.RecentListBase{
        width: Fill
        height: Fit
        /** row height in pixels 24..80 step 1 */
        row_height: 40.
        /** the most rows shown 1..50 step 1 */
        max_rows: 12
        /** shown when nothing was opened yet; empty takes no room */
        empty_text: "Nothing opened yet"
        hover_color: theme.color_outset_hover
        draw_row +: {}
        draw_name +: {
            color: theme.color_text
            text_style: theme.font_bold{font_size: theme.font_size_p}
        }
        draw_path +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p * 0.9}
        }
        draw_when +: {
            color: theme.color_text_meta
            text_style: theme.font_regular{font_size: theme.font_size_p * 0.9}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawRecentRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    pub color: Vec4f,
}

/// One place an app opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecentEntry {
    pub path: String,
    /// When it was last opened, in seconds since the Unix epoch (`None`:
    /// not known, the row shows no time).
    pub opened: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum RecentListAction {
    /// The row with this path was clicked.
    Open(String),
    #[default]
    None,
}

#[derive(Script, ScriptHook, Widget)]
pub struct RecentList {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_row: DrawRecentRow,
    #[live]
    draw_name: DrawText,
    #[live]
    draw_path: DrawText,
    #[live]
    draw_when: DrawText,
    #[live(40.0)]
    row_height: f64,
    #[live(12)]
    max_rows: usize,
    #[live]
    empty_text: String,
    #[live]
    hover_color: Vec4f,
    #[rust]
    entries: Vec<RecentEntry>,
    #[rust]
    rows: Vec<Rect>,
    #[rust]
    hover: Option<usize>,
    #[rust]
    pressed: Option<usize>,
}

impl RecentList {
    /// The places to show, most recent first.
    pub fn set_entries(&mut self, cx: &mut Cx, entries: Vec<RecentEntry>) {
        if self.entries != entries {
            self.entries = entries;
            self.hover = None;
            self.pressed = None;
            self.draw_row.redraw(cx);
        }
    }

    fn shown(&self) -> usize {
        self.entries.len().min(self.max_rows.max(1))
    }

    fn row_at(&self, p: Vec2d) -> Option<usize> {
        self.rows.iter().position(|rect| rect.contains(p))
    }
}

impl Widget for RecentList {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event.hits(cx, self.draw_row.area()) {
            Hit::FingerHoverIn(e) | Hit::FingerHoverOver(e) => {
                let hover = self.row_at(e.abs);
                if hover != self.hover {
                    self.hover = hover;
                    self.draw_row.redraw(cx);
                }
                cx.set_cursor(if hover.is_some() { MouseCursor::Hand } else { MouseCursor::Default });
            }
            Hit::FingerHoverOut(_) => {
                if self.hover.take().is_some() {
                    self.draw_row.redraw(cx);
                }
            }
            Hit::FingerDown(e) if e.device.is_primary_hit() => {
                self.pressed = self.row_at(e.abs);
            }
            Hit::FingerUp(e) => {
                let pressed = self.pressed.take();
                if e.is_over && pressed.is_some() && pressed == self.row_at(e.abs) {
                    if let Some(entry) = pressed.and_then(|row| self.entries.get(row)) {
                        cx.widget_action(self.uid, RecentListAction::Open(entry.path.clone()));
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let shown = self.shown();
        let height = if shown > 0 {
            shown as f64 * self.row_height
        } else if self.empty_text.is_empty() {
            0.0
        } else {
            self.row_height * 0.6
        };
        let rect = cx.walk_turtle(sized(walk, 0.0, height));
        self.rows.clear();
        // One quad over the whole list carries the hits; each row's own
        // quad is its hover.
        let rest = self.draw_row.color;
        self.draw_row.draw_abs(cx, rect);
        if shown == 0 {
            let empty = self.empty_text.clone();
            let font = self.draw_path.text_style.font_size as f64;
            self.draw_path.draw_abs(cx, dvec2(rect.pos.x + 8.0, rect.pos.y + (height - font) * 0.5 - 2.0), &empty);
            return DrawStep::done();
        }
        let now = unix_now();
        let home = home_dir();
        for index in 0..shown {
            let entry = self.entries[index].clone();
            let row = Rect {
                pos: dvec2(rect.pos.x, rect.pos.y + index as f64 * self.row_height),
                size: dvec2(rect.size.x, self.row_height),
            };
            self.rows.push(row);
            if self.hover == Some(index) {
                self.draw_row.color = self.hover_color;
                self.draw_row.draw_abs(cx, Rect { pos: row.pos + dvec2(0.0, 1.0), size: dvec2(row.size.x, row.size.y - 2.0) });
                self.draw_row.color = rest;
            }
            let pad = 8.0;
            let when = entry.opened.map(|opened| opened_ago(opened, now)).unwrap_or_default();
            let when_w = if when.is_empty() { 0.0 } else { measure(&self.draw_when, cx, &when) };
            let name_size = self.draw_name.text_style.font_size as f64;
            let path_size = self.draw_path.text_style.font_size as f64;
            let gap = 3.0;
            let top = row.pos.y + (self.row_height - name_size - path_size - gap) * 0.5 - 2.0;
            let text_w = (row.size.x - 2.0 * pad).max(1.0);
            let name = fit_tail(&self.draw_name, cx, &display_name(&entry.path), (text_w - when_w - 12.0).max(1.0));
            self.draw_name.draw_abs(cx, dvec2(row.pos.x + pad, top), &name);
            if !when.is_empty() {
                self.draw_when.draw_abs(cx, dvec2(row.pos.x + row.size.x - pad - when_w, top + 1.0), &when);
            }
            let path = fit_tail(&self.draw_path, cx, &display_path(&entry.path, home.as_deref()), text_w);
            self.draw_path.draw_abs(cx, dvec2(row.pos.x + pad, top + name_size + gap + 2.0), &path);
        }
        DrawStep::done()
    }
}

impl RecentListRef {
    pub fn set_entries(&self, cx: &mut Cx, entries: Vec<RecentEntry>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_entries(cx, entries);
        }
    }

    /// The path of the row clicked in these actions.
    pub fn opened(&self, actions: &Actions) -> Option<String> {
        match actions.find_widget_action(self.widget_uid())?.cast::<RecentListAction>() {
            RecentListAction::Open(path) => Some(path),
            RecentListAction::None => None,
        }
    }
}

/// `text` as it fits in `width`: whole, or its end after an ellipsis.
fn fit_tail(draw: &DrawText, cx: &mut Cx2d, text: &str, width: f64) -> String {
    if measure(draw, cx, text) <= width {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut keep_lo, mut keep_hi) = (0usize, chars.len());
    while keep_lo < keep_hi {
        let keep = (keep_lo + keep_hi + 1) / 2;
        let candidate: String = std::iter::once('…').chain(chars[chars.len() - keep..].iter().copied()).collect();
        if measure(draw, cx, &candidate) <= width {
            keep_lo = keep;
        } else {
            keep_hi = keep - 1;
        }
    }
    std::iter::once('…').chain(chars[chars.len() - keep_lo..].iter().copied()).collect()
}

/// Seconds since the Unix epoch now (0 where the platform has no clock).
pub fn unix_now() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// When `opened` was, in words from `now`: "just now", "5 min ago",
/// "3 h ago", "yesterday", "4 days ago", "3 weeks ago", "5 months ago".
pub fn opened_ago(opened: u64, now: u64) -> String {
    let seconds = now.saturating_sub(opened);
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    match () {
        _ if minutes < 1 => "just now".into(),
        _ if hours < 1 => format!("{minutes} min ago"),
        _ if days < 1 => format!("{hours} h ago"),
        _ if days == 1 => "yesterday".into(),
        _ if days < 14 => format!("{days} days ago"),
        _ if days < 60 => format!("{} weeks ago", days / 7),
        _ if days < 730 => format!("{} months ago", days / 30),
        _ => format!("{} years ago", days / 365),
    }
}

/// The last part of `path` (the path itself for a root).
pub fn display_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// `path` with the home folder written `~`.
pub fn display_path(path: &str, home: Option<&Path>) -> String {
    if let Some(rest) = home.and_then(|home| Path::new(path).strip_prefix(home).ok()) {
        if rest.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~/{}", rest.to_string_lossy());
    }
    path.to_string()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

/// A recent list kept in a file of its own: one line per place, the time it
/// was last opened (seconds since the Unix epoch), a tab and its path, most
/// recent first. Places that are gone are dropped when it is read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecentFile {
    file: PathBuf,
    limit: usize,
    entries: Vec<RecentEntry>,
}

impl RecentFile {
    /// The list in `file` (empty when there is none), at most `limit` places.
    pub fn load(file: PathBuf, limit: usize) -> Self {
        let entries = std::fs::read_to_string(&file)
            .map(|text| parse_recent(&text))
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| Path::new(&entry.path).exists())
            .take(limit)
            .collect();
        Self { file, limit, entries }
    }

    pub fn entries(&self) -> &[RecentEntry] {
        &self.entries
    }

    /// Put `path` first, opened now, and write the file.
    pub fn remember(&mut self, path: &Path) -> std::io::Result<()> {
        self.remember_at(path, unix_now())
    }

    pub fn remember_at(&mut self, path: &Path, opened: u64) -> std::io::Result<()> {
        let path = path.to_string_lossy().into_owned();
        self.entries.retain(|entry| entry.path != path);
        self.entries.insert(0, RecentEntry { path, opened: Some(opened) });
        self.entries.truncate(self.limit.max(1));
        self.save()
    }

    /// Drop places that are gone; writes the file when one was.
    pub fn forget_missing(&mut self) -> std::io::Result<()> {
        let before = self.entries.len();
        self.entries.retain(|entry| Path::new(&entry.path).exists());
        if self.entries.len() != before {
            self.save()?;
        }
        Ok(())
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut text = String::new();
        for entry in &self.entries {
            text.push_str(&format!("{}\t{}\n", entry.opened.unwrap_or(0), entry.path));
        }
        let temporary = self.file.with_extension("tmp");
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, &self.file)
    }
}

fn parse_recent(text: &str) -> Vec<RecentEntry> {
    text.lines()
        .filter_map(|line| {
            let (opened, path) = line.split_once('\t')?;
            (!path.is_empty()).then(|| RecentEntry {
                path: path.to_string(),
                opened: opened.parse::<u64>().ok().filter(|opened| *opened > 0),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opened_times_read_as_words() {
        let now = 1_000_000_000;
        assert_eq!(opened_ago(now - 20, now), "just now");
        assert_eq!(opened_ago(now - 5 * 60, now), "5 min ago");
        assert_eq!(opened_ago(now - 3 * 3600, now), "3 h ago");
        assert_eq!(opened_ago(now - 30 * 3600, now), "yesterday");
        assert_eq!(opened_ago(now - 4 * 86400, now), "4 days ago");
        assert_eq!(opened_ago(now - 21 * 86400, now), "3 weeks ago");
        assert_eq!(opened_ago(now + 50, now), "just now");
    }

    #[test]
    fn paths_show_home_as_tilde() {
        let home = Path::new("/home/arch");
        assert_eq!(display_path("/home/arch/demo-repos/linux", Some(home)), "~/demo-repos/linux");
        assert_eq!(display_path("/home/arch", Some(home)), "~");
        assert_eq!(display_path("/home/archive/x", Some(home)), "/home/archive/x");
        assert_eq!(display_name("/home/arch/demo-repos/linux"), "linux");
    }

    #[test]
    fn a_recent_file_keeps_the_latest_first_and_drops_what_is_gone() {
        let dir = std::env::temp_dir().join(format!("recent-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = dir.join("a");
        let b = dir.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let file = dir.join("state").join("recent.txt");
        let mut recent = RecentFile::load(file.clone(), 2);
        assert!(recent.entries().is_empty());
        recent.remember_at(&a, 100).unwrap();
        recent.remember_at(&b, 200).unwrap();
        recent.remember_at(&a, 300).unwrap();
        let paths: Vec<_> = recent.entries().iter().map(|e| (e.path.clone(), e.opened)).collect();
        assert_eq!(paths, vec![(a.to_string_lossy().into_owned(), Some(300)), (b.to_string_lossy().into_owned(), Some(200))]);
        assert_eq!(RecentFile::load(file.clone(), 2), recent);
        std::fs::remove_dir_all(&b).unwrap();
        let reread = RecentFile::load(file.clone(), 2);
        assert_eq!(reread.entries().len(), 1);
        assert_eq!(reread.entries()[0].path, a.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
