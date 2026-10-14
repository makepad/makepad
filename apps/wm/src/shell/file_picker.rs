//! The file and folder picker the WM draws for its clients.
//!
//! A hosted app has no display of its own to open a system dialog on, so a
//! client the WM launched relays its `FileDialog` (`ChildRelay::FileDialog`,
//! platform hosted_relay.rs) and the WM answers it from here: a card over
//! the asking window with the places, the folder's listing, a path field,
//! a name field when saving, and Cancel / Open. The answer goes back as
//! `HostRelay::FileDialog` and reaches the app as the `FileDialogAction` a
//! system dialog would have posted.
//!
//! The model (`PickerModel`) is plain data over the file system and is
//! tested on its own; the widget draws it and maps the pointer and keys.
//! While a picker is up it is modal: every pointer event, key and typed
//! text is its own.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};

use makepad_widgets::makepad_platform::studio::{RelayFileDialog, RelayFileDialogResult};
use makepad_widgets::*;

use super::ui::{contains, inset, rect, DrawShellFill, HAlign, Ico, ShellDraw};
use super::{alpha, darker, CtrlState, ShellTokens};

const CARD_W: f64 = 760.0;
const CARD_H: f64 = 480.0;
const PAD: f64 = 14.0;
const GAP: f64 = 8.0;
const ROW: f64 = 24.0;
const PLACE_ROW: f64 = 26.0;
const SIDEBAR: f64 = 168.0;
const BUTTON_W: f64 = 96.0;
const SIZE_COL: f64 = 76.0;
/// Two presses on one row within this many seconds open it.
const DOUBLE_CLICK: f64 = 0.45;
/// Typed letters in the listing find a name; a pause starts a new search.
const TYPE_AHEAD: f64 = 1.0;

/// What the app asked for (`RelayFileDialog::kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    OpenFile,
    SaveFile,
    /// `select_folder` and `save_folder`.
    Folder,
}

impl PickKind {
    pub fn from_relay(kind: &str) -> Self {
        match kind {
            "save_file" => PickKind::SaveFile,
            "select_folder" | "save_folder" => PickKind::Folder,
            _ => PickKind::OpenFile,
        }
    }

    fn default_title(self) -> &'static str {
        match self {
            PickKind::OpenFile => "Open",
            PickKind::SaveFile => "Save",
            PickKind::Folder => "Choose a Folder",
        }
    }

    fn accept_label(self) -> &'static str {
        match self {
            PickKind::OpenFile => "Open",
            PickKind::SaveFile => "Save",
            PickKind::Folder => "Choose",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    /// Bytes, for files.
    pub size: u64,
}

/// Where the keyboard goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    List,
    Name,
    Path,
}

/// What a step of the picker came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Open,
    /// The paths chosen (empty: cancelled).
    Done(Vec<PathBuf>),
}

/// The picker's state over the file system: no drawing, no Cx.
#[derive(Clone, Debug)]
pub struct PickerModel {
    pub kind: PickKind,
    pub multiple: bool,
    pub title: String,
    /// (description, extensions) as the app gave them.
    pub filters: Vec<(String, Vec<String>)>,
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    /// Why the folder could not be listed.
    pub error: Option<String>,
    /// A typed path that led nowhere (shown under the listing).
    pub notice: Option<String>,
    /// The path field was just entered: what is typed replaces it (as if all selected).
    pub path_fresh: bool,
    /// Indices into `entries`, in the order chosen.
    pub selected: Vec<usize>,
    /// The keyboard's row.
    pub cursor: usize,
    /// The first row shown.
    pub scroll: usize,
    pub name: String,
    pub path_text: String,
    pub focus: Focus,
    pub show_hidden: bool,
    /// A save onto an existing file waits for a second Save.
    pub confirm_replace: bool,
}

impl PickerModel {
    pub fn new(request: &RelayFileDialog, home: &Path) -> Self {
        let kind = PickKind::from_relay(&request.kind);
        let location = request.location.as_deref().filter(|l| !l.is_empty()).map(|l| expand_home(l, home));
        let mut name = request.filename.clone().unwrap_or_default();
        let dir = match location {
            Some(l) if l.is_dir() => l,
            // A file named as the location: its folder, and its name to save under.
            Some(l) => match l.parent().filter(|p| p.is_dir()) {
                Some(parent) => {
                    if name.is_empty() && kind == PickKind::SaveFile {
                        name = l.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    }
                    parent.to_path_buf()
                }
                None => home.to_path_buf(),
            },
            None => home.to_path_buf(),
        };
        let mut model = PickerModel {
            kind,
            multiple: request.multiple && kind == PickKind::OpenFile,
            title: request.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| kind.default_title().to_string()),
            filters: request.filters.clone(),
            dir: PathBuf::new(),
            entries: Vec::new(),
            error: None,
            notice: None,
            path_fresh: false,
            selected: Vec::new(),
            cursor: 0,
            scroll: 0,
            name,
            path_text: String::new(),
            focus: if kind == PickKind::SaveFile { Focus::Name } else { Focus::List },
            show_hidden: false,
            confirm_replace: false,
        };
        model.go(dir);
        model
    }

    /// Whether a file is listed under the app's filters ("*" or none: all).
    pub fn passes_filters(&self, name: &str) -> bool {
        if self.filters.is_empty() {
            return true;
        }
        let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase());
        self.filters.iter().flat_map(|(_, exts)| exts).any(|want| {
            let want = want.trim_start_matches("*.").trim_start_matches('.').to_lowercase();
            want.is_empty() || want == "*" || ext.as_deref() == Some(want.as_str())
        })
    }

    /// Show `dir`: its folders, then its files under the filters (none when
    /// choosing a folder), each by name; dot-files only when asked.
    pub fn go(&mut self, dir: PathBuf) {
        self.selected.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.confirm_replace = false;
        self.entries.clear();
        self.error = None;
        self.notice = None;
        match std::fs::read_dir(&dir) {
            Ok(read) => {
                for item in read.flatten() {
                    let name = item.file_name().to_string_lossy().into_owned();
                    if !self.show_hidden && name.starts_with('.') {
                        continue;
                    }
                    // A link is what it points at.
                    let meta = std::fs::metadata(item.path()).ok();
                    let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
                    if !is_dir && (self.kind == PickKind::Folder || !self.passes_filters(&name)) {
                        continue;
                    }
                    let size = meta.as_ref().filter(|_| !is_dir).map_or(0, |m| m.len());
                    self.entries.push(Entry { name, dir: is_dir, size });
                }
                self.entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
            }
            Err(e) => self.error = Some(format!("{}: {}", dir.display(), e)),
        }
        self.path_text = dir.to_string_lossy().into_owned();
        self.dir = dir;
        if self.focus == Focus::Path {
            self.focus = if self.kind == PickKind::SaveFile { Focus::Name } else { Focus::List };
        }
    }

    pub fn reload(&mut self) {
        let dir = self.dir.clone();
        self.go(dir);
    }

    /// Up one folder; the folder left is the row the cursor is on.
    pub fn up(&mut self) {
        let Some(parent) = self.dir.parent().map(Path::to_path_buf) else { return };
        let left = self.dir.file_name().map(|n| n.to_string_lossy().into_owned());
        self.go(parent);
        if let Some(i) = left.and_then(|left| self.entries.iter().position(|e| e.name == left)) {
            self.cursor = i;
            self.selected = vec![i];
        }
    }

    /// A click on row `i` (ctrl: add or drop it, shift: the run from the cursor).
    pub fn click(&mut self, i: usize, ctrl: bool, shift: bool) {
        if i >= self.entries.len() {
            return;
        }
        self.confirm_replace = false;
        if self.multiple && ctrl {
            if let Some(at) = self.selected.iter().position(|s| *s == i) {
                self.selected.remove(at);
            } else {
                self.selected.push(i);
            }
        } else if self.multiple && shift {
            let (a, b) = (self.cursor.min(i), self.cursor.max(i));
            self.selected = (a..=b).collect();
        } else {
            self.selected = vec![i];
        }
        self.cursor = i;
        if self.kind == PickKind::SaveFile && !self.entries[i].dir {
            self.name = self.entries[i].name.clone();
        }
    }

    pub fn move_cursor(&mut self, by: isize) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as isize - 1;
        let at = if self.selected.is_empty() && by > 0 { 0 } else { (self.cursor as isize + by).clamp(0, last) };
        self.click(at as usize, false, false);
    }

    /// Keep the cursor's row among the `rows` shown.
    pub fn reveal(&mut self, rows: usize) {
        let rows = rows.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + rows {
            self.scroll = self.cursor + 1 - rows;
        }
        self.scroll = self.scroll.min(self.entries.len().saturating_sub(rows));
    }

    /// The first name from the cursor on that starts with `prefix`.
    pub fn find(&mut self, prefix: &str) {
        let prefix = prefix.to_lowercase();
        let n = self.entries.len();
        let found = (0..n).map(|k| (self.cursor + k) % n).find(|&i| self.entries[i].name.to_lowercase().starts_with(&prefix));
        if let Some(i) = found {
            self.click(i, false, false);
        }
    }

    fn chosen(&self) -> Vec<usize> {
        let mut chosen = self.selected.clone();
        if chosen.is_empty() && self.cursor < self.entries.len() && self.focus == Focus::List {
            chosen.push(self.cursor);
        }
        chosen
    }

    /// What Open / Save / Choose would do now.
    pub fn can_accept(&self) -> bool {
        match self.kind {
            PickKind::OpenFile => !self.chosen().is_empty(),
            PickKind::SaveFile => !self.name.trim().is_empty(),
            PickKind::Folder => self.error.is_none(),
        }
    }

    /// The save target the name field names (a typed path, or a name in this folder).
    fn save_target(&self) -> PathBuf {
        let name = self.name.trim();
        let mut path = if Path::new(name).is_absolute() { PathBuf::from(name) } else { self.dir.join(name) };
        // No extension typed: the first one the app filters for, as a system dialog adds it.
        if path.extension().is_none() {
            let first = self.filters.iter().flat_map(|(_, exts)| exts).map(|e| e.trim_start_matches("*.").trim_start_matches('.')).find(|e| !e.is_empty() && *e != "*");
            if let Some(ext) = first {
                path.set_extension(ext);
            }
        }
        path
    }

    /// Open / Save / Choose (and Enter): into a chosen folder, or the answer.
    pub fn accept(&mut self) -> Outcome {
        match self.kind {
            PickKind::OpenFile => {
                let chosen = self.chosen();
                if let [one] = chosen[..] {
                    if self.entries[one].dir {
                        self.go(self.dir.join(&self.entries[one].name));
                        return Outcome::Open;
                    }
                }
                let files: Vec<PathBuf> = chosen.iter().filter(|i| !self.entries[**i].dir).map(|i| self.dir.join(&self.entries[*i].name)).collect();
                if files.is_empty() { Outcome::Open } else { Outcome::Done(files) }
            }
            PickKind::SaveFile => {
                if self.name.trim().is_empty() {
                    return Outcome::Open;
                }
                let target = self.save_target();
                if target.is_dir() {
                    self.name.clear();
                    self.go(target);
                    return Outcome::Open;
                }
                if target.exists() && !self.confirm_replace {
                    self.confirm_replace = true;
                    return Outcome::Open;
                }
                Outcome::Done(vec![target])
            }
            // The folder chosen in the listing, else the folder shown.
            PickKind::Folder => match self.selected[..] {
                [one] if self.entries.get(one).is_some_and(|e| e.dir) => Outcome::Done(vec![self.dir.join(&self.entries[one].name)]),
                _ if self.error.is_none() => Outcome::Done(vec![self.dir.clone()]),
                _ => Outcome::Open,
            },
        }
    }

    /// Double-click or Enter on row `i`: into a folder (choosing a folder:
    /// also into it), or that file as the answer.
    pub fn activate(&mut self, i: usize) -> Outcome {
        let Some(entry) = self.entries.get(i).cloned() else { return Outcome::Open };
        if entry.dir {
            self.go(self.dir.join(&entry.name));
            return Outcome::Open;
        }
        self.selected = vec![i];
        self.cursor = i;
        if self.kind == PickKind::SaveFile {
            self.name = entry.name;
        }
        self.accept()
    }

    /// Enter in the path field: that folder (a file's: its folder, and the file chosen).
    pub fn go_typed(&mut self, home: &Path) {
        let path = expand_home(self.path_text.trim(), home);
        let path = if path.is_absolute() { path } else { self.dir.join(path) };
        self.show_path(path);
    }

    /// A folder: into it. A file: its folder, with the file chosen (saving: its name).
    pub fn show_path(&mut self, path: PathBuf) {
        if path.is_dir() {
            self.go(path);
        } else if let Some(parent) = path.parent().filter(|p| p.is_dir()) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.go(parent.to_path_buf());
            if let Some(i) = self.entries.iter().position(|e| e.name == name) {
                self.click(i, false, false);
            } else if self.kind == PickKind::SaveFile {
                self.name = name;
            }
        } else {
            self.notice = Some(format!("{}: no such folder", path.display()));
        }
    }

    /// Into the path field, showing the folder; typing replaces it.
    pub fn edit_path(&mut self) {
        self.path_text = self.dir.to_string_lossy().into_owned();
        self.focus = Focus::Path;
        self.path_fresh = true;
    }

    /// Text typed into the field that has the keyboard.
    pub fn type_text(&mut self, input: &str) {
        match self.focus {
            Focus::Name => {
                self.name.push_str(input);
                self.confirm_replace = false;
            }
            Focus::Path => {
                if std::mem::take(&mut self.path_fresh) {
                    self.path_text.clear();
                }
                self.path_text.push_str(input);
            }
            Focus::List => {}
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.reload();
    }
}

/// `~` and `~/x` under the user's home.
fn expand_home(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir()).unwrap_or_else(|| PathBuf::from("/"))
}

/// How many folders and files picked through the picker it remembers.
const RECENTS: usize = 8;

/// Where the picker keeps what was picked through it, newest first, one path a line.
fn recents_file() -> PathBuf {
    crate::theme::makepad_home().join("wm/picker-recents")
}

/// The remembered picks that still exist.
pub fn load_recents(file: &Path) -> Vec<PathBuf> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    text.lines().filter(|l| !l.is_empty()).map(PathBuf::from).filter(|p| p.exists()).take(RECENTS).collect()
}

/// Put `picked` first in the remembered picks (each once, the oldest dropped).
pub fn remember_recents(file: &Path, picked: &[PathBuf]) {
    let mut list: Vec<PathBuf> = picked.to_vec();
    for old in load_recents(file) {
        if !list.contains(&old) {
            list.push(old);
        }
    }
    list.truncate(RECENTS);
    let text: String = list.iter().map(|p| format!("{}\n", p.to_string_lossy())).collect();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(file, text) {
        log!("wm: file picker recents not saved: {e}");
    }
}

/// The sidebar: home, the usual folders that exist, mounted media, the root.
fn places(home: &Path) -> Vec<(String, PathBuf)> {
    let mut places = vec![("Home".to_string(), home.to_path_buf())];
    for name in ["Desktop", "Documents", "Downloads", "Pictures", "Music", "Videos", "Projects"] {
        let path = home.join(name);
        if path.is_dir() {
            places.push((name.to_string(), path));
        }
    }
    let user = home.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for media in [PathBuf::from("/run/media").join(&user), PathBuf::from("/media").join(&user), PathBuf::from("/mnt")] {
        let Ok(read) = std::fs::read_dir(&media) else { continue };
        let mut mounts: Vec<PathBuf> = read.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        mounts.sort();
        for mount in mounts {
            places.push((mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), mount));
        }
    }
    places.push(("Computer".to_string(), PathBuf::from("/")));
    places
}

fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit + 1 < UNITS.len() {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else if v < 10.0 { format!("{v:.1} {}", UNITS[unit]) } else { format!("{v:.0} {}", UNITS[unit]) }
}

/// Who asked: a hosted client (answered with `HostRelay::FileDialog`), or a portal call (a
/// browser through xdg-desktop-portal, answered as the call's reply; portal_file_chooser.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requester {
    Client(u64),
    Portal(u64),
}

/// One request the WM holds: who it is for, and what they asked.
pub struct PickRequest {
    pub from: Requester,
    pub relay: RelayFileDialog,
    /// The asking window's rect, the card is centred over it.
    pub anchor: Option<Rect>,
}

/// What the WM sends back: to whom, and the answer.
pub type PickAnswer = (Requester, RelayFileDialogResult);

/// What the pointer last pressed or is over.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Row(usize),
    Place(usize),
    Up,
    Path,
    Name,
    Hidden,
    Cancel,
    Accept,
    Card,
    Outside,
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.ShellFilePickerBase = #(ShellFilePicker::register_widget(vm))
    mod.widgets.ShellFilePicker = set_type_default() do mod.widgets.ShellFilePickerBase {
        width: Fill
        height: Fill
        draw_bg +: {}
        d +: {}
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ShellFilePicker {
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
    draw_bg: DrawShellFill,
    #[live]
    d: ShellDraw,
    #[live]
    tokens: ShellTokens,
    #[redraw]
    #[area]
    #[rust]
    area: Area,
    /// The one showing, and its model; the rest wait their turn.
    #[rust]
    current: Option<(PickRequest, PickerModel)>,
    #[rust]
    queue: VecDeque<PickRequest>,
    #[rust]
    home: PathBuf,
    /// The sidebar: the recent picks (the first `recent_count`), then the places.
    #[rust]
    places: Vec<(String, PathBuf)>,
    #[rust]
    recent_count: usize,
    // Laid out by the last draw.
    #[rust]
    card: Rect,
    #[rust]
    list: Rect,
    #[rust]
    rows: usize,
    #[rust]
    place_rects: Vec<Rect>,
    #[rust]
    up_r: Rect,
    #[rust]
    path_r: Rect,
    #[rust]
    name_r: Rect,
    #[rust]
    hidden_r: Rect,
    #[rust]
    cancel_r: Rect,
    #[rust]
    accept_r: Rect,
    #[rust]
    hover: Option<Hit>,
    /// The last press on a row, for a double click.
    #[rust]
    last_press: Option<(usize, f64)>,
    #[rust]
    typed: String,
    #[rust]
    typed_at: f64,
    #[rust]
    scroll_rest: f64,
}

impl ShellFilePicker {
    pub fn is_open(&self) -> bool {
        self.current.is_some()
    }

    /// A client's request: shown now, or after the ones before it.
    pub fn request(&mut self, cx: &mut Cx, request: PickRequest) {
        log!("wm: file picker {} for {:?} ({:?})", request.relay.kind, request.from, request.relay.title);
        self.queue.push_back(request);
        if self.current.is_none() {
            self.next(cx);
        }
    }

    fn next(&mut self, cx: &mut Cx) {
        if let Some(request) = self.queue.pop_front() {
            if self.home.as_os_str().is_empty() {
                self.home = home_dir();
            }
            let recents = load_recents(&recents_file());
            self.recent_count = recents.len();
            self.places = recents.into_iter().map(|p| (p.file_name().map_or_else(|| p.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned()), p)).collect();
            self.places.extend(places(&self.home));
            let mut model = PickerModel::new(&request.relay, &self.home);
            // Nowhere asked for: where the last pick was, with it chosen.
            let unplaced = request.relay.location.as_deref().is_none_or(str::is_empty);
            if let Some(last) = self.places.first().filter(|_| unplaced && self.recent_count > 0) {
                let last = last.1.clone();
                let name = model.name.clone();
                model.show_path(last);
                if model.kind == PickKind::SaveFile {
                    model.name = name;
                }
            }
            self.current = Some((request, model));
            self.hover = None;
            self.last_press = None;
            self.typed.clear();
        }
        self.redraw(cx);
    }

    /// A client went away: its requests go with it (it hears nothing more).
    pub fn forget_client(&mut self, cx: &mut Cx, client: u64) {
        self.queue.retain(|r| r.from != Requester::Client(client));
        if self.current.as_ref().is_some_and(|(r, _)| r.from == Requester::Client(client)) {
            self.current = None;
            self.next(cx);
        }
    }

    fn finish(&mut self, cx: &mut Cx, paths: Vec<PathBuf>) -> Option<PickAnswer> {
        let (request, model) = self.current.take()?;
        let answer = RelayFileDialogResult {
            id: request.relay.id,
            save: model.kind == PickKind::SaveFile,
            folder: model.kind == PickKind::Folder,
            paths: paths.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
        };
        log!("wm: file picker for {:?} answered {:?}", request.from, answer.paths);
        if !paths.is_empty() {
            remember_recents(&recents_file(), &paths);
        }
        self.next(cx);
        Some((request.from, answer))
    }

    fn outcome(&mut self, cx: &mut Cx, outcome: Outcome) -> Option<PickAnswer> {
        match outcome {
            Outcome::Open => None,
            Outcome::Done(paths) => self.finish(cx, paths),
        }
    }

    fn hit(&self, p: Vec2d) -> Hit {
        if !contains(self.card, p) {
            return Hit::Outside;
        }
        let Some((_, model)) = &self.current else { return Hit::Outside };
        if contains(self.list, p) {
            let i = model.scroll + ((p.y - self.list.pos.y - 1.0) / ROW).floor().max(0.0) as usize;
            return if i < model.entries.len() { Hit::Row(i) } else { Hit::Card };
        }
        if let Some(i) = self.place_rects.iter().position(|r| contains(*r, p)) {
            return Hit::Place(i);
        }
        for (r, hit) in [
            (self.up_r, Hit::Up),
            (self.path_r, Hit::Path),
            (self.name_r, Hit::Name),
            (self.hidden_r, Hit::Hidden),
            (self.cancel_r, Hit::Cancel),
            (self.accept_r, Hit::Accept),
        ] {
            if contains(r, p) {
                return hit;
            }
        }
        Hit::Card
    }

    /// Every pointer event while the picker is up (all of them are its own).
    /// Returns the answer when one was chosen.
    pub fn pointer(&mut self, cx: &mut Cx, event: &Event) -> Option<PickAnswer> {
        self.current.as_ref()?;
        match event {
            Event::MouseMove(e) => {
                let hover = Some(self.hit(e.abs));
                if hover != self.hover {
                    self.hover = hover;
                    self.redraw(cx);
                }
                cx.set_cursor(MouseCursor::Default);
                None
            }
            Event::MouseDown(e) => {
                let hit = self.hit(e.abs);
                let (ctrl, shift) = (e.modifiers.control || e.modifiers.logo, e.modifiers.shift);
                let home = self.home.clone();
                let rows = self.rows;
                let place = match hit {
                    Hit::Place(i) => self.places.get(i).map(|(_, p)| p.clone()),
                    _ => None,
                };
                let double = matches!(hit, Hit::Row(i) if self.last_press.is_some_and(|(j, t)| j == i && e.time - t < DOUBLE_CLICK));
                self.last_press = match hit { Hit::Row(i) if !double => Some((i, e.time)), _ => None };
                let (_, model) = self.current.as_mut()?;
                let outcome = match hit {
                    Hit::Row(i) => {
                        if model.focus == Focus::Path {
                            model.focus = if model.kind == PickKind::SaveFile { Focus::Name } else { Focus::List };
                        }
                        if model.kind != PickKind::SaveFile {
                            model.focus = Focus::List;
                        }
                        if double {
                            model.activate(i)
                        } else {
                            model.click(i, ctrl, shift);
                            model.reveal(rows);
                            Outcome::Open
                        }
                    }
                    Hit::Place(_) => {
                        if let Some(place) = place {
                            model.show_path(place);
                            model.reveal(rows);
                        }
                        Outcome::Open
                    }
                    Hit::Up => {
                        model.up();
                        model.reveal(rows);
                        Outcome::Open
                    }
                    Hit::Path => {
                        if model.focus != Focus::Path {
                            model.edit_path();
                        }
                        Outcome::Open
                    }
                    Hit::Name => {
                        model.focus = Focus::Name;
                        Outcome::Open
                    }
                    Hit::Hidden => {
                        model.toggle_hidden();
                        Outcome::Open
                    }
                    Hit::Cancel => Outcome::Done(Vec::new()),
                    Hit::Accept => {
                        if model.focus == Focus::Path {
                            model.go_typed(&home);
                            Outcome::Open
                        } else {
                            model.accept()
                        }
                    }
                    Hit::Card | Hit::Outside => Outcome::Open,
                };
                self.redraw(cx);
                self.outcome(cx, outcome)
            }
            Event::Scroll(e) => {
                if contains(self.list, e.abs) {
                    let rows = self.rows.max(1);
                    let (_, model) = self.current.as_mut()?;
                    self.scroll_rest += e.scroll.y / ROW;
                    let steps = self.scroll_rest.trunc();
                    self.scroll_rest -= steps;
                    let max = model.entries.len().saturating_sub(rows) as f64;
                    model.scroll = (model.scroll as f64 + steps).clamp(0.0, max) as usize;
                    self.redraw(cx);
                }
                None
            }
            _ => None,
        }
    }

    /// Keys while the picker is up (all of them are its own). Returns the
    /// answer when one was chosen.
    pub fn key(&mut self, cx: &mut Cx, e: &KeyEvent) -> Option<PickAnswer> {
        // A key ends a name search: the next letters start a new one.
        self.typed.clear();
        let home = self.home.clone();
        let rows = self.rows.max(1);
        let (_, model) = self.current.as_mut()?;
        let ctrl = e.modifiers.control || e.modifiers.logo;
        let outcome = match e.key_code {
            KeyCode::Escape if model.focus == Focus::Path => {
                model.path_text = model.dir.to_string_lossy().into_owned();
                model.focus = if model.kind == PickKind::SaveFile { Focus::Name } else { Focus::List };
                Outcome::Open
            }
            KeyCode::Escape => Outcome::Done(Vec::new()),
            KeyCode::ReturnKey | KeyCode::NumpadEnter => match model.focus {
                Focus::Path => {
                    model.go_typed(&home);
                    Outcome::Open
                }
                Focus::Name => model.accept(),
                Focus::List if model.selected.len() > 1 => model.accept(),
                Focus::List if model.kind == PickKind::Folder && model.selected.is_empty() => model.accept(),
                Focus::List => {
                    let at = model.cursor;
                    model.activate(at)
                }
            },
            KeyCode::Backspace => {
                let fresh = model.focus == Focus::Path && std::mem::take(&mut model.path_fresh);
                let field = match model.focus {
                    Focus::Name => Some(&mut model.name),
                    Focus::Path => Some(&mut model.path_text),
                    Focus::List => None,
                };
                if fresh {
                    model.path_text.clear();
                } else if let Some(text) = field {
                    if ctrl {
                        let trimmed = text.trim_end_matches(|c| c == '/' || c == ' ');
                        let cut = trimmed.rfind(|c| c == '/' || c == ' ').map_or(0, |i| i + 1);
                        text.truncate(cut);
                    } else {
                        text.pop();
                    }
                    model.confirm_replace = false;
                } else {
                    model.up();
                }
                Outcome::Open
            }
            KeyCode::Tab => {
                model.focus = match (model.kind, model.focus) {
                    (PickKind::SaveFile, Focus::Name) => Focus::List,
                    (PickKind::SaveFile, _) => Focus::Name,
                    (_, Focus::List) => Focus::Path,
                    _ => Focus::List,
                };
                if model.focus == Focus::Path {
                    model.edit_path();
                }
                Outcome::Open
            }
            KeyCode::KeyL if ctrl => {
                model.edit_path();
                Outcome::Open
            }
            KeyCode::KeyH if ctrl => {
                model.toggle_hidden();
                Outcome::Open
            }
            KeyCode::KeyA if ctrl && model.focus == Focus::List && model.multiple => {
                model.selected = (0..model.entries.len()).filter(|i| !model.entries[*i].dir).collect();
                Outcome::Open
            }
            KeyCode::ArrowUp if e.modifiers.alt => {
                model.up();
                Outcome::Open
            }
            KeyCode::ArrowDown if e.modifiers.alt => {
                let at = model.cursor;
                if model.entries.get(at).is_some_and(|e| e.dir) {
                    model.activate(at)
                } else {
                    Outcome::Open
                }
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End
                if model.focus != Focus::Path =>
            {
                let n = model.entries.len() as isize;
                let by = match e.key_code {
                    KeyCode::ArrowUp => -1,
                    KeyCode::ArrowDown => 1,
                    KeyCode::PageUp => -(rows as isize),
                    KeyCode::PageDown => rows as isize,
                    KeyCode::Home => -n,
                    _ => n,
                };
                if model.kind != PickKind::SaveFile {
                    model.focus = Focus::List;
                }
                model.move_cursor(by);
                Outcome::Open
            }
            KeyCode::ArrowLeft if model.focus == Focus::List => {
                model.up();
                Outcome::Open
            }
            KeyCode::ArrowRight if model.focus == Focus::List => {
                let at = model.cursor;
                if model.entries.get(at).is_some_and(|e| e.dir) {
                    model.activate(at)
                } else {
                    Outcome::Open
                }
            }
            _ => Outcome::Open,
        };
        model.reveal(rows);
        self.redraw(cx);
        self.outcome(cx, outcome)
    }

    /// Typed text: into the name or path field, or a name search in the listing
    /// (a `/` or `~` there starts typing a path).
    pub fn typed(&mut self, cx: &mut Cx, input: &str) {
        let rows = self.rows.max(1);
        let Some((_, model)) = self.current.as_mut() else { return };
        let input: String = input.chars().filter(|c| !c.is_control()).collect();
        if input.is_empty() {
            return;
        }
        match model.focus {
            Focus::Name | Focus::Path => model.type_text(&input),
            Focus::List if input.starts_with('/') || input.starts_with('~') => {
                model.edit_path();
                model.type_text(&input);
            }
            Focus::List => {
                let now = Cx::monotonic_now();
                if now - self.typed_at > TYPE_AHEAD {
                    self.typed.clear();
                }
                self.typed_at = now;
                self.typed.push_str(&input);
                model.find(&self.typed);
                model.reveal(rows);
            }
        }
        self.redraw(cx);
    }

    fn draw_picker(&mut self, cx: &mut Cx2d, screen: Rect) {
        let Some((request, model)) = self.current.as_ref() else { return };
        let model = model.clone();
        let anchor = request.anchor.filter(|a| a.size.x > 0.0 && a.size.y > 0.0).unwrap_or(screen);
        let tok = self.tokens;
        let fg = tok.popups.text;
        let dim = darker(fg, 1.6);
        let c = &tok.controls;

        // The desk dims; the card sits over the window that asked, inside the screen.
        self.d.solid(cx, screen, alpha(tok.menu.scrim, 0.35));
        let w = CARD_W.min(screen.size.x - 2.0 * GAP);
        let h = CARD_H.min(screen.size.y - 2.0 * GAP);
        let x = (anchor.pos.x + (anchor.size.x - w) * 0.5).clamp(screen.pos.x + GAP, screen.pos.x + screen.size.x - w - GAP).floor();
        let y = (anchor.pos.y + (anchor.size.y - h) * 0.5).clamp(screen.pos.y + GAP, screen.pos.y + screen.size.y - h - GAP).floor();
        let card = rect(x, y, w, h);
        self.card = card;
        let mut surface = tok.popups;
        surface.background_alpha = 1.0;
        self.d.card(cx, card, &surface);
        let inner = inset(card, tok.popups.border_width + PAD);
        let ctrl_h = tok.spacing.control_height;
        let right = inner.pos.x + inner.size.x;
        let state = |hit: Hit, hover: Option<Hit>| if hover == Some(hit) { CtrlState::Hover } else { CtrlState::Normal };

        // Title, and the hidden-files switch.
        let mut top = inner.pos.y;
        let hidden_w = 124.0;
        self.d.label_elided(cx, rect(inner.pos.x, top, inner.size.x - hidden_w - GAP, ctrl_h), true, tok.font.title, fg, HAlign::Left, &model.title);
        self.hidden_r = rect(right - hidden_w, top, hidden_w, ctrl_h);
        let hidden_state = if model.show_hidden { CtrlState::Selected } else { state(Hit::Hidden, self.hover) };
        self.d.button(cx, self.hidden_r, &tok, hidden_state, None, "Hidden files", tok.font.body_small, fg, false);
        top += ctrl_h + GAP;

        // Up, and the path (typed into after a click or Ctrl+L).
        self.up_r = rect(inner.pos.x, top, ctrl_h, ctrl_h);
        let up_state = if model.dir.parent().is_none() { CtrlState::Disabled } else { state(Hit::Up, self.hover) };
        self.d.button(cx, self.up_r, &tok, up_state, Some(Ico::ChevronLeft), "", tok.font.body, fg, true);
        self.path_r = rect(inner.pos.x + ctrl_h + GAP, top, inner.size.x - ctrl_h - GAP, ctrl_h);
        self.d.text_field(cx, self.path_r, &tok, &model.path_text, "Folder", model.focus == Focus::Path, self.hover == Some(Hit::Path), fg);
        if model.focus == Focus::Path && model.path_fresh {
            // Typing replaces the whole path: it reads as selected.
            let pad = tok.spacing.control_padding_x;
            let w = self.d.measure(cx, false, tok.font.body, &model.path_text).min(self.path_r.size.x - 2.0 * pad);
            self.d.solid(cx, rect(self.path_r.pos.x + pad - 2.0, self.path_r.pos.y + 5.0, w + 4.0, ctrl_h - 10.0), alpha(fg, c.selection_fill_alpha * 0.6));
        }
        top += ctrl_h + GAP;

        // The footer: the name (saving) or the filters, Cancel and the accept button.
        let foot = inner.pos.y + inner.size.y - ctrl_h;
        self.accept_r = rect(right - BUTTON_W, foot, BUTTON_W, ctrl_h);
        self.cancel_r = rect(right - 2.0 * BUTTON_W - GAP, foot, BUTTON_W, ctrl_h);
        let accept_label = if model.confirm_replace { "Replace" } else { model.kind.accept_label() };
        let accept_state = if !model.can_accept() { CtrlState::Disabled } else if self.hover == Some(Hit::Accept) { CtrlState::Pressed } else { CtrlState::Selected };
        self.d.button(cx, self.cancel_r, &tok, state(Hit::Cancel, self.hover), None, "Cancel", tok.font.body, fg, true);
        self.d.button(cx, self.accept_r, &tok, accept_state, None, accept_label, tok.font.body, fg, true);
        let left_w = self.cancel_r.pos.x - GAP - inner.pos.x;
        self.name_r = Rect::default();
        if model.kind == PickKind::SaveFile {
            let label_w = 48.0;
            self.d.label(cx, rect(inner.pos.x, foot, label_w, ctrl_h), false, tok.font.body, dim, HAlign::Left, "Name");
            self.name_r = rect(inner.pos.x + label_w, foot, (left_w - label_w).max(0.0), ctrl_h);
            self.d.text_field(cx, self.name_r, &tok, &model.name, "File name", model.focus == Focus::Name, self.hover == Some(Hit::Name), fg);
        }
        let note = if let Some(notice) = &model.notice {
            notice.clone()
        } else if model.confirm_replace {
            format!("\u{201c}{}\u{201d} exists: Replace overwrites it", model.name.trim())
        } else if model.kind != PickKind::SaveFile && !model.filters.is_empty() {
            model.filters.iter().map(|(d, exts)| if d.is_empty() { exts.join(", ") } else { format!("{d} ({})", exts.join(", ")) }).collect::<Vec<_>>().join("; ")
        } else {
            String::new()
        };
        if model.kind == PickKind::SaveFile {
            if !note.is_empty() {
                self.d.label_elided(cx, rect(inner.pos.x, foot - ctrl_h * 0.8, left_w, ctrl_h * 0.8), false, tok.font.body_small, fg, HAlign::Left, &note);
            }
        } else {
            self.d.label_elided(cx, rect(inner.pos.x, foot, left_w, ctrl_h), false, tok.font.body_small, dim, HAlign::Left, &note);
        }
        let body_bottom = foot - GAP - if model.kind == PickKind::SaveFile && !note.is_empty() { ctrl_h * 0.8 } else { 0.0 };
        let body = rect(inner.pos.x, top, inner.size.x, body_bottom - top);

        // The places.
        self.place_rects.clear();
        let mut py = body.pos.y;
        let header_h = 20.0;
        for (i, (name, path)) in self.places.clone().iter().enumerate() {
            // Section names over the recent picks and the places.
            let header = if i == 0 && self.recent_count > 0 { Some("Recent") } else if i == self.recent_count { Some("Places") } else { None };
            if let Some(header) = header {
                if py + header_h + PLACE_ROW > body.pos.y + body.size.y {
                    break;
                }
                let top_gap = if i == 0 { 0.0 } else { 6.0 };
                self.d.label(cx, rect(body.pos.x + 6.0, py + top_gap, SIDEBAR - 12.0, header_h - top_gap), true, tok.font.caption, dim, HAlign::Left, header);
                py += header_h;
            }
            if py + PLACE_ROW > body.pos.y + body.size.y {
                break;
            }
            let r = rect(body.pos.x, py, SIDEBAR, PLACE_ROW);
            let is_dir = i >= self.recent_count || path.is_dir();
            self.d.cursor_surface(cx, r, c, self.hover == Some(Hit::Place(i)), *path == model.dir);
            self.d.icon_centered(cx, if is_dir { Ico::Folder } else { Ico::File }, rect(r.pos.x + 6.0, r.pos.y, 16.0, PLACE_ROW), 14.0, fg);
            self.d.label_elided(cx, rect(r.pos.x + 28.0, r.pos.y, SIDEBAR - 34.0, PLACE_ROW), false, tok.font.body, fg, HAlign::Left, name);
            self.place_rects.push(r);
            py += PLACE_ROW;
        }

        // The listing.
        let list = rect(body.pos.x + SIDEBAR + GAP, body.pos.y, body.size.x - SIDEBAR - GAP, body.size.y);
        self.list = list;
        let list_state = if model.focus == Focus::List { CtrlState::Focus } else { CtrlState::Normal };
        self.d.control(cx, list, c, list_state);
        let rows = ((list.size.y - 2.0) / ROW).floor().max(1.0) as usize;
        self.rows = rows;
        if let Some(error) = &model.error {
            self.d.label_elided(cx, inset(list, 10.0), false, tok.font.body, dim, HAlign::Center, error);
        } else if model.entries.is_empty() {
            let empty = if model.kind == PickKind::Folder { "No folders here" } else { "Nothing here" };
            self.d.label(cx, list, false, tok.font.body, dim, HAlign::Center, empty);
        }
        let scroll = model.scroll.min(model.entries.len().saturating_sub(rows));
        let bar = model.entries.len() > rows;
        let row_w = list.size.x - 2.0 - if bar { 6.0 } else { 0.0 };
        for (k, entry) in model.entries.iter().enumerate().skip(scroll).take(rows) {
            let r = rect(list.pos.x + 1.0, list.pos.y + 1.0 + (k - scroll) as f64 * ROW, row_w, ROW);
            let selected = model.selected.contains(&k);
            // The keyboard's row shows once the listing is navigated (nothing chosen yet
            // means the folder shown, when choosing a folder).
            let cursor = model.focus == Focus::List && k == model.cursor && !selected && !model.selected.is_empty();
            self.d.cursor_surface(cx, r, c, self.hover == Some(Hit::Row(k)) || cursor, selected);
            self.d.icon_centered(cx, if entry.dir { Ico::Folder } else { Ico::File }, rect(r.pos.x + 6.0, r.pos.y, 16.0, ROW), 14.0, if entry.dir { fg } else { dim });
            let name_w = r.size.x - 34.0 - if entry.dir { 4.0 } else { SIZE_COL };
            self.d.label_elided(cx, rect(r.pos.x + 28.0, r.pos.y, name_w, ROW), false, tok.font.body, fg, HAlign::Left, &entry.name);
            if !entry.dir {
                self.d.label(cx, rect(r.pos.x + r.size.x - SIZE_COL - 6.0, r.pos.y, SIZE_COL, ROW), false, tok.font.body_small, dim, HAlign::Right, &size_text(entry.size));
            }
        }
        if bar {
            let track = rect(list.pos.x + list.size.x - 6.0, list.pos.y + 2.0, 4.0, list.size.y - 4.0);
            let n = model.entries.len() as f64;
            let thumb_h = (track.size.y * rows as f64 / n).max(16.0);
            let thumb_y = track.pos.y + (track.size.y - thumb_h) * scroll as f64 / (n - rows as f64).max(1.0);
            self.d.solid(cx, rect(track.pos.x, thumb_y, 4.0, thumb_h), alpha(fg, 0.35));
        }
    }
}

impl Widget for ShellFilePicker {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let screen = cx.turtle().rect();
        self.draw_picker(cx, screen);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    /// Input reaches it through the WM (`pointer`, `key`, `text`), before any client.
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder of the given files and folders under the test temp dir.
    fn fixture(tag: &str, files: &[&str], dirs: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wm-file-picker-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for d in dirs {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in files {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        root
    }

    fn request(kind: &str, location: &Path) -> RelayFileDialog {
        RelayFileDialog { id: 7, kind: kind.into(), location: Some(location.to_string_lossy().into_owned()), ..Default::default() }
    }

    #[test]
    fn folders_come_first_and_files_follow_the_filters() {
        let root = fixture("list", &["b.png", "A.jpg", "notes.txt", ".hidden.png"], &["zeta", "Alpha"]);
        let mut req = request("select_file", &root);
        req.filters = vec![("Images".into(), vec!["png".into(), "jpg".into()])];
        let mut m = PickerModel::new(&req, &root);
        let names: Vec<&str> = m.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "zeta", "A.jpg", "b.png"]);
        m.toggle_hidden();
        assert!(m.entries.iter().any(|e| e.name == ".hidden.png"));
        // Choosing a folder lists folders only.
        let f = PickerModel::new(&request("select_folder", &root), &root);
        assert!(f.entries.iter().all(|e| e.dir));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn open_goes_into_a_folder_and_answers_with_the_chosen_files() {
        let root = fixture("open", &["one.txt", "sub/two.txt", "sub/three.txt"], &["sub"]);
        let mut req = request("select_file", &root);
        req.multiple = true;
        let mut m = PickerModel::new(&req, &root);
        assert_eq!(m.entries[0].name, "sub");
        // Nothing chosen yet in an empty selection outside the list: nothing to open.
        m.focus = Focus::Name;
        assert!(!m.can_accept());
        m.focus = Focus::List;
        assert_eq!(m.activate(0), Outcome::Open);
        assert_eq!(m.dir, root.join("sub"));
        m.click(0, false, false);
        m.click(1, true, false);
        assert_eq!(m.accept(), Outcome::Done(vec![root.join("sub/three.txt"), root.join("sub/two.txt")]));
        m.up();
        assert_eq!(m.dir, root);
        assert_eq!(m.entries[m.cursor].name, "sub", "the folder left is the cursor's row");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn choose_folder_answers_the_selected_folder_or_the_one_shown() {
        let root = fixture("folder", &[], &["proj"]);
        let mut m = PickerModel::new(&request("select_folder", &root), &root);
        assert_eq!(m.accept(), Outcome::Done(vec![root.clone()]));
        m.click(0, false, false);
        assert_eq!(m.accept(), Outcome::Done(vec![root.join("proj")]));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_adds_the_filters_extension_and_asks_before_replacing() {
        let root = fixture("save", &["old.json"], &["dir"]);
        let mut req = request("save_file", &root);
        req.filters = vec![("JSON".into(), vec!["json".into()])];
        req.filename = Some("scene".into());
        let mut m = PickerModel::new(&req, &root);
        assert_eq!(m.focus, Focus::Name);
        assert_eq!(m.accept(), Outcome::Done(vec![root.join("scene.json")]));
        m.name = "old".into();
        assert_eq!(m.accept(), Outcome::Open);
        assert!(m.confirm_replace);
        assert_eq!(m.accept(), Outcome::Done(vec![root.join("old.json")]));
        // A folder's name goes into it.
        m.name = "dir".into();
        m.filters.clear();
        m.confirm_replace = false;
        assert_eq!(m.accept(), Outcome::Open);
        assert_eq!(m.dir, root.join("dir"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_location_naming_a_file_opens_its_folder_with_the_name() {
        let root = fixture("loc", &["keep.txt"], &[]);
        let req = request("save_file", &root.join("new.txt"));
        let m = PickerModel::new(&req, &root);
        assert_eq!(m.dir, root);
        assert_eq!(m.name, "new.txt");
        // A typed path goes to its folder; `~` is home.
        let mut m = PickerModel::new(&request("select_file", Path::new("/nonexistent/x")), &root);
        assert_eq!(m.dir, root, "an unknown location falls back to home");
        m.path_text = "~/keep.txt".into();
        m.go_typed(&root);
        assert_eq!(m.entries[m.cursor].name, "keep.txt");
        assert_eq!(m.selected, vec![m.cursor]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Entering the path field shows the folder; typing replaces it; a path that leads
    /// nowhere says so without hiding the listing or blocking Choose.
    #[test]
    fn a_typed_path_replaces_the_shown_one() {
        let root = fixture("path", &[], &["a", "b"]);
        let mut m = PickerModel::new(&request("select_folder", &root), &root);
        m.edit_path();
        assert_eq!(m.path_text, root.to_string_lossy());
        m.type_text(&root.join("b").to_string_lossy());
        m.go_typed(&root);
        assert_eq!(m.dir, root.join("b"));
        m.edit_path();
        m.type_text("/no/such/place");
        m.go_typed(&root);
        assert_eq!(m.dir, root.join("b"));
        assert!(m.notice.is_some() && m.error.is_none() && m.can_accept());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Picks are remembered newest first, once each, and only while they exist; a recent
    /// file opens its folder with the file chosen.
    #[test]
    fn recent_picks_are_remembered_newest_first() {
        let root = fixture("recents", &["f.txt"], &["a", "b"]);
        let file = root.join("state/recents");
        remember_recents(&file, &[root.join("a")]);
        remember_recents(&file, &[root.join("b"), root.join("f.txt")]);
        remember_recents(&file, &[root.join("a")]);
        assert_eq!(load_recents(&file), [root.join("a"), root.join("b"), root.join("f.txt")]);
        std::fs::remove_dir(root.join("b")).unwrap();
        assert_eq!(load_recents(&file), [root.join("a"), root.join("f.txt")]);
        let mut m = PickerModel::new(&request("select_file", &root.join("a")), &root);
        m.show_path(root.join("f.txt"));
        assert_eq!(m.dir, root);
        assert_eq!(m.entries[m.selected[0]].name, "f.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_cursor_stays_in_view_and_type_ahead_finds_names() {
        let names: Vec<String> = (0..40).map(|i| format!("f{i:02}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let root = fixture("scroll", &refs, &[]);
        let mut m = PickerModel::new(&request("select_file", &root), &root);
        m.move_cursor(1);
        m.move_cursor(25);
        m.reveal(10);
        assert_eq!((m.cursor, m.scroll), (25, 16));
        m.find("f03");
        m.reveal(10);
        assert_eq!((m.cursor, m.scroll), (3, 3));
        let _ = std::fs::remove_dir_all(&root);
    }
}
