//! `MilkdropBrowser`: the preset browser every host shares — a search
//! field that filters as you type, favourites and rated switches, and one
//! list: a tree of folders (the pack's categories and subcategories, and
//! the person's own presets) with the presets under them. While no pack is
//! on disk it offers to download projectM's pack instead.
//!
//! The list is the shared DataGrid (one column, virtualised, keyboard
//! selection), drawn as a tree: a folder row folds with a press, Return or
//! the Left and Right keys; a preset row loads with a press or Return. The
//! grid does its own hit testing against where its rows really are, which
//! is what the old TreeView could not do inside an aligned parent (see the
//! report in `local/agent_state/skeuomorph/music/browser/`).
//!
//! The widget owns its library: a worker thread scans the pack (with the
//! index cache) and the person's own presets
//! ([`crate::user_presets::user_dir`], watched), builds the
//! search index, downloads the pack on request and saves the person's
//! favourites, ratings and recently played list. The UI thread reads and
//! writes no file, and a keystroke only runs the filter over the index.
//! Everything lives in one folder, [`data_dir`], so every app on the
//! machine downloads the pack once and shares the favourites.
//!
//! The list's source is pluggable: a host can show its own items (an
//! effect list, say) with [`MilkdropBrowserRef::set_external`], grouped by
//! their folders and filtered the same way; a pick arrives as
//! [`MilkdropBrowserAction::PickItem`]. [`MilkdropBrowserRef::clear_external`]
//! brings the presets back.
//!
//! A host places a `MilkdropBrowser` beside a
//! [`MilkdropView`](crate::MilkdropView): a picked preset arrives as
//! [`MilkdropBrowserAction::Play`], and the host's own next, previous and
//! random buttons go through [`MilkdropBrowserRef::step`] and
//! [`MilkdropBrowserRef::random`], which return what to load. When the
//! library first arrives the browser picks up the preset played last (or a
//! random one) and says so with a `Play` action.

use crate::library::{download_pack, scan_cached, step, DownloadEvent, PresetEntry, PresetLibrary, Shuffle, UserPresets, CREAM_OF_THE_CROP_DIR, CREAM_OF_THE_CROP_ZIP};
use crate::preset_tree::{BrowserItem, Folds, RowKey, TreeIndex, TreeRow};
use makepad_widgets::makepad_platform::makepad_network::blocking_http::CancelToken;
use makepad_widgets::makepad_platform::thread::{SignalToUI, ThreadOptions};
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // One column, no headings: the rows are drawn by the browser.
    let PresetList = DataGrid{
        width: Fill
        height: Fill
        rows: 0
        cols: 1
        show_row_headers: false
        show_col_headers: false
        selection: GridSelectMode.Rows
        cell_cursor: MouseCursor.Hand
        zebra_stripes: false
        allow_col_resize: false
        allow_row_resize: false
        allow_col_reorder: false
        default_col_width: 200.0
        default_row_height: 24.0
        cell_pad_x: 0.0
        color_bg: #x1b1c1f
        color_cell: #x1b1c1f
        color_cell_alt: #x1b1c1f
        color_text: #xe3e4e6
        color_selection: #x5cf2d42e
        color_selection_border: #x5cf2d400
        draw_cell +: {border_size: 0.0}
    }

    mod.widgets.MilkdropBrowserBase = #(MilkdropBrowser::register_widget(vm))
    mod.widgets.MilkdropBrowser = set_type_default() do mod.widgets.MilkdropBrowserBase{
        width: Fill
        height: Fill
        flow: Down
        spacing: 6
        // The list's inks: names, counts and authors, the playing preset.
        color_text: #xe3e4e6
        color_muted: #x8a8d94
        color_accent: #x5cf2d4
        color_hover: #x5cf2d41a
        row_indent: 14.0
        draw_name +: {text_style: theme.font_regular{font_size: 10.5}}
        draw_detail +: {text_style: theme.font_regular{font_size: 9.0}}
        draw_icon +: {text_style: theme.font_icons{font_size: 7.5}}
        filters := View{
            width: Fill
            height: Fit
            flow: Right
            spacing: 10
            align: Align{y: 0.5}
            search := TextInput{width: Fill empty_text: "Search presets"}
            favourites := CheckBox{text: "Favourites"}
            rated := CheckBox{text: "Rated"}
        }
        // The list, in a view: a DataGrid has no visibility of its own.
        grid_box := View{width: Fill height: Fill grid := PresetList{}}
        // No pack on disk: one centred group, the title, what the pack is,
        // the keys, and what the download is doing under them. The texts
        // wrap and the keys wrap under each other, so a narrow or short
        // host never cuts a key off.
        missing := View{
            visible: false
            width: Fill
            height: Fill
            flow: Down
            align: Align{x: 0.5 y: 0.5}
            padding: Inset{left: 12 right: 12 top: 8 bottom: 8}
            missing_group := View{
                width: Fill
                height: Fit
                flow: Down
                spacing: 10
                align: Align{x: 0.5}
                missing_title := Label{width: Fill align: Align{x: 0.5} text: "No presets yet." draw_text +: {text_style: theme.font_regular{font_size: 13}}}
                missing_about := Label{width: Fill align: Align{x: 0.5} text: "The \"cream of the crop\" pack from projectM: 9,795 presets, about 160 MB." draw_text +: {text_style: theme.font_regular{font_size: 10}}}
                missing_keys := View{
                    width: Fill
                    height: Fit
                    flow: Flow.Right{wrap: true}
                    spacing: 8
                    align: Align{x: 0.5}
                    download_key := View{width: Fit height: Fit download := Button{text: "Download the preset pack"}}
                    cancel_key := View{visible: false width: Fit height: Fit cancel := Button{text: "Cancel"}}
                }
                progress := Label{width: Fill align: Align{x: 0.5} text: ""}
            }
        }
        status := Label{width: Fill text: "Reading presets…" draw_text +: {text_style: theme.font_regular{font_size: 9.5}}}
    }
}

/// Where every host keeps the MilkDrop pack, its index cache, and the
/// person's favourites, ratings and recently played list:
/// `MILKDROP_DATA_DIR` when set (an absolute path), else the platform's
/// application-data folder for "milkdrop". One folder for every app, so the
/// pack is downloaded once.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MILKDROP_DATA_DIR").map(PathBuf::from).filter(|p| p.is_absolute()) {
        return Some(dir);
    }
    makepad_widgets::makepad_platform::home::app_data_dir("milkdrop")
}

/// The folder the person's own presets are listed under.
pub const OWN_FOLDER: &str = "My presets";
/// What an own preset's name starts with, so its favourites and ratings
/// never collide with a pack preset of the same path.
const OWN_PREFIX: &str = "~own/";

const ICON_CLOSED: &str = "\u{f054}";
const ICON_OPEN: &str = "\u{f078}";
const ICON_PLAYING: &str = "\u{f04b}";
const ICON_HEART: &str = "\u{f004}";
const ICON_OWN: &str = "\u{f007}";

// ---- the model ----

/// One list: its index, what is folded, the rows the filter leaves.
#[derive(Default)]
struct Pane {
    index: Arc<TreeIndex>,
    folds: Folds,
    rows: Vec<TreeRow>,
    /// Kept items in list order: what next and previous walk.
    shown: Vec<usize>,
    /// The playing (or picked) item.
    current: Option<usize>,
    /// Where the selection and the keys stand.
    cursor: Option<RowKey>,
}

impl Pane {
    fn row_of(&self, key: RowKey) -> Option<usize> {
        self.rows.iter().position(|r| r.key == key)
    }
}

/// A host's own list, shown instead of the presets.
struct External {
    label: String,
    items: Vec<BrowserItem>,
    pane: Pane,
}

#[derive(Default)]
struct Browser {
    /// The pack, then the person's own presets from `own_from` on.
    library: Arc<PresetLibrary>,
    own_from: usize,
    user: UserPresets,
    presets: Pane,
    external: Option<External>,
    query: String,
    favourites_only: bool,
    min_rating: u8,
    /// Bumped whenever the rows or what a row says change.
    version: u64,
    /// The next draw brings the cursor's row into view.
    reveal: bool,
}

impl Browser {
    fn pane(&self) -> &Pane {
        self.external.as_ref().map(|e| &e.pane).unwrap_or(&self.presets)
    }

    fn pane_mut(&mut self) -> &mut Pane {
        match &mut self.external {
            Some(external) => &mut external.pane,
            None => &mut self.presets,
        }
    }

    /// Any filter is on: every folder with a match unfolds.
    fn filtering(&self) -> bool {
        !self.query.trim().is_empty() || (self.external.is_none() && (self.favourites_only || self.min_rating > 0))
    }

    /// The rows for the query and switches, in list order. A few hundred
    /// microseconds for the whole pack.
    fn refilter(&mut self) {
        let folded = makepad_search::fold_words(&self.query);
        let words: Vec<&str> = folded.split_whitespace().collect();
        let filtering = self.filtering();
        let rows = match &self.external {
            Some(external) => {
                let pane = &external.pane;
                pane.index.rows(|i| pane.index.matches(i, &words), |f| pane.folds.is_open(f, filtering))
            }
            None => {
                let (pane, lib, user) = (&self.presets, &self.library, &self.user);
                let (favourites, min_rating) = (self.favourites_only, self.min_rating);
                pane.index.rows(
                    |i| {
                        pane.index.matches(i, &words)
                            && (!favourites || user.is_favourite(&lib.entries[i].rel))
                            && (min_rating == 0 || user.rating(&lib.entries[i].rel) >= min_rating)
                    },
                    |f| pane.folds.is_open(f, filtering),
                )
            }
        };
        let pane = self.pane_mut();
        pane.rows = rows.rows;
        pane.shown = rows.shown;
        self.version += 1;
    }

    fn pick(&self, index: usize) -> Option<PresetPick> {
        let e = self.library.entries.get(index)?;
        Some(PresetPick { path: e.path.clone(), rel: e.rel.clone(), title: e.title.clone(), authors: e.authors_text() })
    }
}

/// A preset to load: its file and what to call it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresetPick {
    pub path: PathBuf,
    /// The stable name under the library root (`~own/…` for the person's
    /// own presets).
    pub rel: String,
    pub title: String,
    /// The authors as one line, or empty.
    pub authors: String,
}

impl PresetPick {
    /// "Title — Authors", or the title.
    pub fn line(&self) -> String {
        if self.authors.is_empty() {
            self.title.clone()
        } else {
            format!("{} — {}", self.title, self.authors)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum MilkdropBrowserAction {
    #[default]
    None,
    /// Load this preset: a row was picked, or the library arrived and the
    /// preset played last comes back.
    Play(PresetPick),
    /// A host's own item (see `set_external`) was picked: its key.
    PickItem(String),
    /// There is no pack on disk; the browser offers the download.
    NeedsPack,
}

// ---- worker ----

enum Command {
    /// Index the pack in the data folder (or say it is missing).
    Scan,
    /// Download the pack; the token stops this download (a cancelled
    /// token stays cancelled, so each download brings its own).
    Download(CancelToken),
    /// Write a file of the data folder (atomically).
    Save(&'static str, String),
    /// List the `.milk` files under this folder as the person's own, and
    /// list them again whenever the folder changes.
    OwnDir(PathBuf),
}

enum Message {
    /// The pack and the person's own presets, indexed. `user` comes with
    /// the first reading only: after that the UI's copy is the one kept.
    Library { library: Arc<PresetLibrary>, own_from: usize, index: Arc<TreeIndex>, user: Option<UserPresets>, note: String },
    Missing,
    Progress(String),
    Failed(String),
}

const USER_FILE: &str = "user.txt";
/// How often the own-presets folder is looked at for changes.
const OWN_POLL: Duration = Duration::from_secs(2);

fn send(tx: &SyncSender<Message>, message: Message) {
    if tx.try_send(message).is_ok() {
        SignalToUI::set_ui_signal();
    }
}

/// The folders a preset is listed under.
fn folder_of(entry: &PresetEntry, own: bool) -> String {
    let mut folder = if own { OWN_FOLDER.to_string() } else { String::new() };
    for part in [entry.category.as_str(), entry.subcategory.as_str()] {
        if !part.is_empty() {
            if !folder.is_empty() {
                folder.push('/');
            }
            folder.push_str(part);
        }
    }
    folder
}

/// The pack and the own presets as one library, and its index.
fn merge(pack: &PresetLibrary, own: Option<&PresetLibrary>) -> (Arc<PresetLibrary>, usize, Arc<TreeIndex>) {
    let mut entries = pack.entries.clone();
    let own_from = entries.len();
    if let Some(own) = own {
        entries.extend(own.entries.iter().cloned().map(|mut e| {
            e.rel = format!("{OWN_PREFIX}{}", e.rel);
            e
        }));
    }
    let index = TreeIndex::build(entries.iter().enumerate().map(|(i, e)| (folder_of(e, i >= own_from), e.title.clone(), e.authors_text())));
    (Arc::new(PresetLibrary { root: pack.root.clone(), entries }), own_from, Arc::new(index))
}

/// What changes when a file in `dir` is added, removed or rewritten.
fn folder_signature(dir: &Path) -> (usize, u128) {
    let (mut count, mut newest) = (0usize, 0u128);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&at) else { continue };
        for entry in read.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            count += 1;
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
            newest = newest.max(modified);
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    (count, newest)
}

fn run_worker(commands: Receiver<Command>, tx: SyncSender<Message>) {
    let Some(data) = data_dir() else {
        send(&tx, Message::Failed("No data folder for the presets: set MILKDROP_DATA_DIR.".into()));
        return;
    };
    let presets = data.join("presets");
    let pack_dir = presets.join(CREAM_OF_THE_CROP_DIR);
    let mut pack: Option<PresetLibrary> = None;
    // The person's own presets: the folder `user_presets` writes to,
    // unless the host names another.
    let mut own_dir: Option<(PathBuf, (usize, u128))> = crate::user_presets::user_dir().map(|dir| {
        let signature = folder_signature(&dir);
        (dir, signature)
    });
    let mut sent_user = false;
    let mut note = String::new();

    let read_own = |dir: &Path| -> Option<PresetLibrary> { scan_cached(dir, None).ok().map(|(library, _)| library).filter(|l| !l.entries.is_empty()) };
    let mut own: Option<PresetLibrary> = own_dir.as_ref().and_then(|(dir, _)| read_own(dir));
    let publish = |pack: &Option<PresetLibrary>, own: &Option<PresetLibrary>, note: &str, sent_user: &mut bool| {
        let Some(pack) = pack else { return };
        let (library, own_from, index) = merge(pack, own.as_ref());
        let user = if *sent_user { None } else { Some(std::fs::read_to_string(data.join(USER_FILE)).map(|t| UserPresets::from_text(&t)).unwrap_or_default()) };
        *sent_user = true;
        send(&tx, Message::Library { library, own_from, index, user, note: note.to_string() });
    };
    let scan_pack = |pack: &mut Option<PresetLibrary>, note: &mut String| -> bool {
        if !pack_dir.is_dir() {
            send(&tx, Message::Missing);
            return false;
        }
        match scan_cached(&pack_dir, Some(&data.join("index.txt"))) {
            Ok((library, stats)) => {
                *note = format!("{} presets indexed in {} ms", stats.files, stats.millis);
                *pack = Some(library);
                true
            }
            Err(error) => {
                send(&tx, Message::Failed(format!("Cannot read {}: {error}", pack_dir.display())));
                false
            }
        }
    };
    loop {
        let command = match commands.recv_timeout(OWN_POLL) {
            Ok(command) => Some(command),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        match command {
            None => {
                // The own-presets folder, looked at while nothing else is asked.
                let Some((dir, signature)) = &mut own_dir else { continue };
                let now = folder_signature(dir);
                if now != *signature {
                    *signature = now;
                    own = read_own(dir);
                    publish(&pack, &own, &note, &mut sent_user);
                }
            }
            Some(Command::Scan) => {
                if scan_pack(&mut pack, &mut note) {
                    publish(&pack, &own, &note, &mut sent_user);
                }
            }
            Some(Command::OwnDir(dir)) => {
                let signature = folder_signature(&dir);
                own = read_own(&dir);
                own_dir = Some((dir, signature));
                publish(&pack, &own, &note, &mut sent_user);
            }
            Some(Command::Download(cancel)) => {
                log!("milkdrop: downloading {CREAM_OF_THE_CROP_ZIP} into {}", presets.display());
                let mut logged_mb = 0;
                let cancelled = cancel.clone();
                let result = download_pack(CREAM_OF_THE_CROP_ZIP, &presets, CREAM_OF_THE_CROP_DIR, cancel, |event| {
                    match &event {
                        DownloadEvent::Downloading { received, .. } if received >> 20 >= logged_mb + 20 => {
                            logged_mb = received >> 20;
                            log!("milkdrop: {logged_mb} MB received");
                        }
                        DownloadEvent::Unpacking { done: 0, total } => log!("milkdrop: unpacking {total} files"),
                        DownloadEvent::Done { presets, dir } => log!("milkdrop: {presets} presets unpacked into {}", dir.display()),
                        _ => {}
                    }
                    let text = match event {
                        DownloadEvent::Downloading { received, total: Some(total) } => format!("Downloading {} of {} MB", received >> 20, total >> 20),
                        DownloadEvent::Downloading { received, total: None } => format!("Downloading {} MB", received >> 20),
                        DownloadEvent::Unpacking { done, total } => format!("Unpacking {done} of {total}"),
                        DownloadEvent::Done { presets, .. } => format!("{presets} presets downloaded"),
                    };
                    send(&tx, Message::Progress(text));
                });
                match result {
                    Ok(_) => {
                        if scan_pack(&mut pack, &mut note) {
                            publish(&pack, &own, &note, &mut sent_user);
                        }
                    }
                    Err(_) if cancelled.is_cancelled() => {
                        log!("milkdrop: the pack download was cancelled");
                        send(&tx, Message::Failed("Download cancelled.".into()));
                    }
                    Err(error) => {
                        error!("milkdrop: the pack download stopped: {error}");
                        send(&tx, Message::Failed(error));
                    }
                }
            }
            Some(Command::Save(file, text)) => {
                let tmp = data.join(format!("{file}.tmp"));
                if std::fs::create_dir_all(&data).is_ok() && std::fs::write(&tmp, text).is_ok() {
                    let _ = std::fs::rename(&tmp, data.join(file));
                }
            }
        }
    }
}

// ---- drawing helpers ----

fn text_width(draw: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text).size_in_lpxs.width as f64
}

fn text_height(draw: &DrawText, cx: &mut Cx2d, text: &str) -> f64 {
    draw.layout(cx, 0.0, 0.0, None, false, Align::default(), text).size_in_lpxs.height as f64
}

/// Text centred on `mid`, cut to `max_width` with an ellipsis; the width
/// it took.
fn text_fit(draw: &mut DrawText, cx: &mut Cx2d, x: f64, mid: f64, max_width: f64, text: &str) -> f64 {
    if max_width <= 4.0 || text.is_empty() {
        return 0.0;
    }
    let y = mid - text_height(draw, cx, "Ag") * 0.5;
    let width = text_width(draw, cx, text);
    if width <= max_width {
        draw.draw_abs(cx, dvec2(x, y), text);
        return width;
    }
    // Binary search on the character count: measured O(log n) times.
    let bounds: Vec<usize> = text.char_indices().map(|(at, _)| at).chain(std::iter::once(text.len())).collect();
    let (mut low, mut high) = (0usize, bounds.len() - 1);
    while low < high {
        let middle = (low + high).div_ceil(2);
        if text_width(draw, cx, &format!("{}…", &text[..bounds[middle]])) <= max_width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    if low == 0 {
        return 0.0;
    }
    let cut = format!("{}…", &text[..bounds[low]]);
    draw.draw_abs(cx, dvec2(x, y), &cut);
    text_width(draw, cx, &cut)
}

/// Text ending at `right`, centred on `mid`; its width.
fn text_right(draw: &mut DrawText, cx: &mut Cx2d, right: f64, mid: f64, text: &str) -> f64 {
    let width = text_width(draw, cx, text);
    let y = mid - text_height(draw, cx, text) * 0.5;
    draw.draw_abs(cx, dvec2(right - width, y), text);
    width
}

fn with_alpha(color: Vec4f, alpha: f32) -> Vec4f {
    vec4(color.x, color.y, color.z, color.w * alpha)
}

// ---- the widget ----

#[derive(Script, ScriptHook, Widget)]
pub struct MilkdropBrowser {
    #[deref]
    view: View,
    #[live]
    draw_name: DrawText,
    #[live]
    draw_detail: DrawText,
    #[live]
    draw_icon: DrawText,
    #[live]
    color_text: Vec4f,
    #[live]
    color_muted: Vec4f,
    #[live]
    color_accent: Vec4f,
    /// The row under the pointer's tint (under the selection's).
    #[live]
    color_hover: Vec4f,
    #[live]
    draw_hover: DrawColor,
    #[live(14.0)]
    row_indent: f64,
    /// The row under the pointer, and where each drawn row is.
    #[rust]
    hover_row: Option<usize>,
    #[rust]
    row_rects: Vec<(usize, Rect)>,
    #[rust]
    browser: Browser,
    #[rust]
    drawn_version: Option<u64>,
    #[rust]
    commands: Option<SyncSender<Command>>,
    #[rust]
    messages: Option<Receiver<Message>>,
    /// Commands that found the worker's queue full, sent again on the
    /// next event.
    #[rust]
    pending: Vec<Command>,
    #[rust]
    cancel: Option<CancelToken>,
    #[rust(Shuffle::new(0x5eed))]
    shuffle: Shuffle,
    /// A folder row folded by a press, and when: the second press of a
    /// double-click does not fold it back.
    #[rust]
    last_fold: Option<(usize, Instant)>,
    /// The library has arrived at least once.
    #[rust]
    ready: bool,
    /// No pack on disk: the download is on offer.
    #[rust]
    missing: bool,
    /// The keys moved the cursor onto an item: it loads when the cursor
    /// rests there this long (holding an arrow does not load every row).
    #[rust]
    preview: Option<usize>,
    #[rust]
    preview_timer: Timer,
}

/// How long the cursor rests on a row before the keys load it (s).
const PREVIEW_DELAY: f64 = 0.15;

impl MilkdropBrowser {
    fn start_worker(&mut self, cx: &mut Cx) {
        let (command_tx, command_rx) = mpsc::sync_channel(64);
        let (message_tx, message_rx) = mpsc::sync_channel(256);
        match cx.thread_spawner().spawn_worker(ThreadOptions { name: Some("milkdrop-library".into()), ..Default::default() }, move || run_worker(command_rx, message_tx)) {
            Ok(handle) => handle.detach(),
            Err(error) => error!("milkdrop: cannot start the library worker: {error}"),
        }
        let _ = command_tx.try_send(Command::Scan);
        self.commands = Some(command_tx);
        self.messages = Some(message_rx);
    }

    fn command(&mut self, command: Command) {
        self.pending.push(command);
        self.flush_commands();
    }

    /// Send what is waiting; whatever still finds the queue full waits for
    /// the next event.
    fn flush_commands(&mut self) {
        let Some(tx) = &self.commands else { return };
        while !self.pending.is_empty() {
            let command = self.pending.remove(0);
            if let Err(mpsc::TrySendError::Full(command)) = tx.try_send(command) {
                self.pending.insert(0, command);
                break;
            }
        }
    }

    fn set_status(&mut self, cx: &mut Cx, text: &str) {
        self.view.label(cx, ids!(status)).set_text(cx, text);
    }

    /// What the status line says: how many rows the filter leaves.
    fn update_status(&mut self, cx: &mut Cx) {
        let b = &self.browser;
        let text = match &b.external {
            Some(external) => {
                let total = external.items.len();
                let shown = external.pane.shown.len();
                if b.filtering() {
                    format!("{shown} of {total} {}", external.label)
                } else {
                    format!("{total} {}", external.label)
                }
            }
            None if !self.ready => return,
            None => {
                let total = b.library.entries.len();
                let own = total - b.own_from;
                let shown = b.presets.shown.len();
                let mut text = if b.filtering() { format!("{shown} of {total} presets") } else { format!("{total} presets") };
                if own > 0 {
                    text.push_str(&format!(" · {own} of them yours"));
                }
                if b.filtering() && shown == 0 {
                    text = format!("Nothing matches · {total} presets");
                }
                text
            }
        };
        self.set_status(cx, &text);
    }

    /// Show the download offer, or the list.
    fn show_parts(&mut self, cx: &mut Cx) {
        let missing = self.missing && self.browser.external.is_none();
        let external = self.browser.external.is_some();
        self.view.view(cx, ids!(missing)).set_visible(cx, missing);
        self.view.widget(cx, ids!(grid_box)).set_visible(cx, !missing);
        self.view.view(cx, ids!(filters)).set_visible(cx, !missing);
        // The download says where it is under its keys.
        self.view.widget(cx, ids!(status)).set_visible(cx, !missing);
        // The switches keep presets; a host's list has none.
        self.view.widget(cx, ids!(favourites)).set_visible(cx, !external);
        self.view.widget(cx, ids!(rated)).set_visible(cx, !external);
    }

    fn downloading(&mut self, cx: &mut Cx, on: bool) {
        self.view.button(cx, ids!(download)).set_enabled(cx, !on);
        self.view.widget(cx, ids!(cancel_key)).set_visible(cx, on);
    }

    fn poll_worker(&mut self, cx: &mut Cx) {
        let Some(messages) = self.messages.take() else { return };
        loop {
            match messages.try_recv() {
                Ok(Message::Library { library, own_from, index, user, note }) => {
                    log!("milkdrop: {note}; {} own presets", library.entries.len() - own_from);
                    self.missing = false;
                    let current_rel = self.browser.presets.current.and_then(|i| self.browser.library.entries.get(i)).map(|e| e.rel.clone());
                    let cursor_rel = match self.browser.presets.cursor {
                        Some(RowKey::Item(i)) => self.browser.library.entries.get(i).map(|e| e.rel.clone()),
                        _ => None,
                    };
                    self.browser.library = library;
                    self.browser.own_from = own_from;
                    self.browser.presets.index = index;
                    // Folder numbers may have moved: the folds are kept by
                    // path, the cursor on a folder is dropped.
                    let find = |rel: Option<String>| rel.and_then(|rel| self.browser.library.entries.iter().position(|e| e.rel == rel));
                    self.browser.presets.current = find(current_rel);
                    self.browser.presets.cursor = find(cursor_rel).map(RowKey::Item);
                    if let Some(user) = user {
                        self.browser.user = user;
                    }
                    if self.browser.external.is_none() {
                        self.browser.refilter();
                    } else {
                        // Filter the presets once without the host's query.
                        let external = self.browser.external.take();
                        let query = std::mem::take(&mut self.browser.query);
                        self.browser.refilter();
                        self.browser.query = query;
                        self.browser.external = external;
                    }
                    self.show_parts(cx);
                    self.downloading(cx, false);
                    if !self.ready {
                        self.ready = true;
                        // Pick up where the person left off, else somewhere random
                        // (the pack's first rows are transitions, which fade to black).
                        let lib = &self.browser.library;
                        let last = self.browser.user.recent().next().and_then(|rel| lib.entries.iter().position(|e| e.rel == rel)).filter(|&i| !lib.entries[i].is_transition());
                        let first = last.or_else(|| self.shuffle.pick(&self.browser.presets.shown, None, |i| lib.entries[i].is_transition()));
                        if let Some(pick) = first.and_then(|index| self.play(cx, index)) {
                            cx.widget_action(self.widget_uid(), MilkdropBrowserAction::Play(pick));
                        }
                    }
                    self.update_status(cx);
                    self.view.redraw(cx);
                }
                Ok(Message::Missing) => {
                    self.missing = true;
                    self.show_parts(cx);
                    cx.widget_action(self.widget_uid(), MilkdropBrowserAction::NeedsPack);
                    self.set_status(cx, "No preset pack downloaded yet.");
                    self.view.redraw(cx);
                }
                Ok(Message::Progress(text)) => self.view.label(cx, ids!(progress)).set_text(cx, &text),
                Ok(Message::Failed(error)) => {
                    self.downloading(cx, false);
                    self.view.label(cx, ids!(progress)).set_text(cx, &error);
                    self.set_status(cx, &error);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        self.messages = Some(messages);
    }

    /// Make `index` the playing preset, note it as played, and say what to
    /// load.
    fn play(&mut self, cx: &mut Cx, index: usize) -> Option<PresetPick> {
        let pick = self.browser.pick(index)?;
        self.browser.presets.current = Some(index);
        self.browser.version += 1;
        self.browser.user.played(&pick.rel);
        self.save_user();
        self.view.redraw(cx);
        Some(pick)
    }

    fn save_user(&mut self) {
        let text = self.browser.user.to_text();
        self.command(Command::Save(USER_FILE, text));
    }

    fn after_user_change(&mut self, cx: &mut Cx) {
        if self.browser.favourites_only || self.browser.min_rating > 0 {
            self.refilter(cx);
        } else {
            self.browser.version += 1;
        }
        self.save_user();
        self.view.redraw(cx);
    }

    fn refilter(&mut self, cx: &mut Cx) {
        self.browser.refilter();
        self.update_status(cx);
        self.view.redraw(cx);
    }

    /// The query changed: the folds made under the old one are forgotten,
    /// so every match shows.
    fn set_query(&mut self, cx: &mut Cx, query: String) {
        if query == self.browser.query {
            return;
        }
        self.browser.query = query;
        self.browser.pane_mut().folds.shut_while_filtering.clear();
        self.refilter(cx);
        // The first match is where Return goes and the keys start.
        let pane = self.browser.pane_mut();
        if !pane.cursor.is_some_and(|c| pane.rows.iter().any(|r| r.key == c)) {
            pane.cursor = pane.rows.iter().find(|r| matches!(r.key, RowKey::Item(_))).map(|r| r.key);
        }
        self.browser.reveal = true;
    }

    fn set_search_hint(&mut self, cx: &mut Cx, hint: &str) {
        if let Some(mut search) = self.view.text_input(cx, ids!(search)).borrow_mut() {
            search.set_empty_text(cx, hint.to_string());
        }
    }

    fn clear_query(&mut self, cx: &mut Cx) {
        self.view.text_input(cx, ids!(search)).set_text(cx, "");
        self.set_query(cx, String::new());
    }

    /// Fold or unfold a folder (`open` None flips it).
    fn fold(&mut self, cx: &mut Cx, folder: usize, open: Option<bool>) {
        let filtering = self.browser.filtering();
        let pane = self.browser.pane_mut();
        let index = pane.index.clone();
        let Some(f) = index.folder(folder) else { return };
        let now = open.unwrap_or(!pane.folds.is_open(f, filtering));
        pane.folds.set_open(f, filtering, now);
        pane.cursor = Some(RowKey::Folder(folder));
        self.browser.refilter();
        self.browser.reveal = true;
        self.view.redraw(cx);
    }

    /// Load what a row holds: a preset plays, a host's item is picked.
    fn activate_item(&mut self, cx: &mut Cx, item: usize) {
        let uid = self.widget_uid();
        self.browser.pane_mut().cursor = Some(RowKey::Item(item));
        if let Some(external) = &mut self.browser.external {
            external.pane.current = Some(item);
            let key = external.items.get(item).map(|i| i.key.clone());
            self.browser.version += 1;
            self.view.redraw(cx);
            if let Some(key) = key {
                cx.widget_action(uid, MilkdropBrowserAction::PickItem(key));
            }
            return;
        }
        if let Some(pick) = self.play(cx, item) {
            cx.widget_action(uid, MilkdropBrowserAction::Play(pick));
        }
    }

    fn activate(&mut self, cx: &mut Cx, key: RowKey) {
        match key {
            RowKey::Folder(folder) => self.fold(cx, folder, None),
            RowKey::Item(item) => self.activate_item(cx, item),
        }
    }

    /// Move the cursor to a row and show it.
    fn set_cursor_row(&mut self, cx: &mut Cx, row: usize) {
        let pane = self.browser.pane_mut();
        if let Some(r) = pane.rows.get(row) {
            let key = r.key;
            pane.cursor = Some(key);
            self.browser.version += 1;
            self.browser.reveal = true;
            self.view.redraw(cx);
            self.preview_later(cx, key);
        }
    }

    /// The keys put the cursor on `key`: an item loads once the cursor
    /// rests on it for `PREVIEW_DELAY`.
    fn preview_later(&mut self, cx: &mut Cx, key: RowKey) {
        match key {
            RowKey::Item(item) if self.browser.pane().current != Some(item) => {
                self.preview = Some(item);
                cx.stop_timer(self.preview_timer);
                self.preview_timer = cx.start_timeout(PREVIEW_DELAY);
            }
            _ => self.preview = None,
        }
    }

    /// Right: unfold a folded folder, else step into it.
    fn key_right(&mut self, cx: &mut Cx) {
        let pane = self.browser.pane();
        let Some(cursor) = pane.cursor else { return self.set_cursor_row(cx, 0) };
        let (Some(row), RowKey::Folder(folder)) = (pane.row_of(cursor), cursor) else { return };
        if pane.rows[row].open {
            self.set_cursor_row(cx, row + 1);
        } else {
            self.fold(cx, folder, Some(true));
        }
    }

    /// Left: fold an unfolded folder, else climb to the folder above.
    fn key_left(&mut self, cx: &mut Cx) {
        let pane = self.browser.pane();
        let Some(cursor) = pane.cursor else { return };
        let open = pane.row_of(cursor).is_some_and(|row| pane.rows[row].open);
        let parent = match cursor {
            RowKey::Folder(folder) => pane.index.folder(folder).map(|f| f.parent).unwrap_or(0),
            RowKey::Item(item) => pane.index.item_folder(item),
        };
        let parent_row = pane.row_of(RowKey::Folder(parent)).filter(|_| parent != 0);
        match cursor {
            RowKey::Folder(folder) if open => self.fold(cx, folder, Some(false)),
            _ => {
                if let Some(row) = parent_row {
                    self.set_cursor_row(cx, row);
                }
            }
        }
    }

    fn focus_list(&mut self, cx: &mut Cx) {
        let area = self.view.widget(cx, ids!(grid)).area();
        cx.set_key_focus(area);
    }

    /// The next (`delta` 1) or previous (-1) preset in the shown list.
    pub fn step(&mut self, cx: &mut Cx, delta: isize) -> Option<PresetPick> {
        let index = step(&self.browser.presets.shown, self.browser.presets.current, delta)?;
        self.play(cx, index)
    }

    /// A random preset of the shown list, not one played recently.
    pub fn random(&mut self, cx: &mut Cx) -> Option<PresetPick> {
        let (lib, user) = (&self.browser.library, &self.browser.user);
        // Transitions are for fading between presets, not for landing on.
        let index = self.shuffle.pick(&self.browser.presets.shown, self.browser.presets.current, |i| lib.entries[i].is_transition() || user.recency(&lib.entries[i].rel).is_some_and(|r| r < 50))?;
        self.play(cx, index)
    }

    /// The playing preset.
    pub fn current(&self) -> Option<PresetPick> {
        self.browser.presets.current.and_then(|index| self.browser.pick(index))
    }

    /// Favourite the playing preset, or take it back; its new state.
    pub fn toggle_favourite(&mut self, cx: &mut Cx) -> bool {
        let Some(rel) = self.current().map(|pick| pick.rel) else { return false };
        let on = self.browser.user.toggle_favourite(&rel);
        self.after_user_change(cx);
        on
    }

    /// Rate the playing preset 0 (none) to 5 stars.
    pub fn rate(&mut self, cx: &mut Cx, stars: u8) {
        let Some(rel) = self.current().map(|pick| pick.rel) else { return };
        self.browser.user.set_rating(&rel, stars);
        self.after_user_change(cx);
    }

    /// Whether the playing preset is a favourite, and its stars.
    pub fn current_marks(&self) -> (bool, u8) {
        match self.current() {
            Some(pick) => (self.browser.user.is_favourite(&pick.rel), self.browser.user.rating(&pick.rel)),
            None => (false, 0),
        }
    }

    /// Whether a library is loaded (the pack is there).
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// No pack is on disk: the browser shows the download instead of the
    /// list (a host that keeps the browser folded should show it now).
    pub fn needs_pack(&self) -> bool {
        self.missing
    }

    /// List the `.milk` files under `dir` as the person's own presets,
    /// under "My presets", and keep listing what appears there, instead of
    /// the default [`crate::user_presets::user_dir`]. A folder that does
    /// not exist yet is fine.
    pub fn set_own_dir(&mut self, dir: PathBuf) {
        self.command(Command::OwnDir(dir));
    }

    /// Show a host's own list instead of the presets (grouped by each
    /// item's folder, filtered the same way). `label` names the items in
    /// the count ("effects"). Calling it again with the same items keeps
    /// the folds and the cursor.
    pub fn set_external(&mut self, cx: &mut Cx, label: &str, items: Vec<BrowserItem>) {
        let switching = self.browser.external.is_none();
        match &mut self.browser.external {
            Some(external) if external.items == items => {
                external.label = label.to_string();
            }
            Some(external) => {
                let current = external.pane.current.and_then(|i| external.items.get(i)).map(|i| i.key.clone());
                external.pane.index = Arc::new(TreeIndex::of_items(&items));
                external.pane.current = current.and_then(|key| items.iter().position(|i| i.key == key));
                external.pane.cursor = None;
                external.items = items;
                external.label = label.to_string();
            }
            None => {
                let index = Arc::new(TreeIndex::of_items(&items));
                // A host's list is short: its top folders start unfolded.
                let mut folds = Folds::default();
                folds.open.extend(items.iter().map(|i| i.folder.split('/').next().unwrap_or("").to_string()).filter(|f| !f.is_empty()));
                self.browser.external = Some(External { label: label.to_string(), items, pane: Pane { index, folds, ..Default::default() } });
            }
        }
        if switching {
            self.clear_query(cx);
            self.set_search_hint(cx, &format!("Search {label}"));
        }
        self.browser.refilter();
        self.show_parts(cx);
        self.update_status(cx);
        self.view.redraw(cx);
    }

    /// Back to the presets.
    pub fn clear_external(&mut self, cx: &mut Cx) {
        if self.browser.external.take().is_some() {
            self.clear_query(cx);
            self.set_search_hint(cx, "Search presets");
            self.browser.refilter();
            self.show_parts(cx);
            self.update_status(cx);
            self.view.redraw(cx);
        }
    }

    /// Mark a host's item as the one playing (by key), or none.
    pub fn set_external_current(&mut self, cx: &mut Cx, key: Option<&str>) {
        if let Some(external) = &mut self.browser.external {
            let at = key.and_then(|key| external.items.iter().position(|i| i.key == key));
            if at != external.pane.current {
                external.pane.current = at;
                self.browser.version += 1;
                self.view.redraw(cx);
            }
        }
    }

    /// Unfold the folders of the playing row and bring it into view.
    pub fn reveal_current(&mut self, cx: &mut Cx) {
        let filtering = self.browser.filtering();
        let pane = self.browser.pane_mut();
        let Some(current) = pane.current else { return };
        let index = pane.index.clone();
        for folder in index.ancestors(current) {
            if let Some(f) = index.folder(folder) {
                pane.folds.set_open(f, filtering, true);
            }
        }
        pane.cursor = Some(RowKey::Item(current));
        self.browser.refilter();
        self.browser.reveal = true;
        self.view.redraw(cx);
    }

    /// Draw one row in its cell.
    fn draw_row(&mut self, cx: &mut Cx2d, rect: Rect, row: &TreeRow, current_folders: &[usize]) {
        let b = &self.browser;
        let pane = b.pane();
        let mid = rect.pos.y + rect.size.y * 0.5;
        let left = rect.pos.x + 6.0 + row.depth as f64 * self.row_indent;
        // Room for the vertical scroll bar, drawn over the column.
        let right = rect.pos.x + rect.size.x - 12.0;
        let (text, muted, accent) = (self.color_text, self.color_muted, self.color_accent);
        match row.key {
            RowKey::Folder(folder) => {
                let Some(f) = pane.index.folder(folder) else { return };
                let holds_current = current_folders.contains(&folder);
                self.draw_icon.color = if holds_current { accent } else { muted };
                text_fit(&mut self.draw_icon, cx, left + 1.0, mid, 12.0, if row.open { ICON_OPEN } else { ICON_CLOSED });
                let count = row.count.to_string();
                self.draw_detail.color = muted;
                let count_w = text_right(&mut self.draw_detail, cx, right, mid, &count);
                self.draw_name.color = if holds_current && !row.open { accent } else { text };
                let name = f.name.clone();
                text_fit(&mut self.draw_name, cx, left + 16.0, mid, right - count_w - 8.0 - (left + 16.0), &name);
            }
            RowKey::Item(item) => {
                let playing = pane.current == Some(item);
                let (title, detail, favourite, own, stars) = match &b.external {
                    Some(external) => {
                        let Some(i) = external.items.get(item) else { return };
                        (i.title.clone(), i.detail.clone(), false, i.own, 0)
                    }
                    None => {
                        let Some(e) = b.library.entries.get(item) else { return };
                        (e.title.clone(), e.authors_text(), b.user.is_favourite(&e.rel), item >= b.own_from, b.user.rating(&e.rel))
                    }
                };
                if playing {
                    self.draw_icon.color = accent;
                    text_fit(&mut self.draw_icon, cx, left + 1.0, mid, 12.0, ICON_PLAYING);
                }
                // The marks at the right end: stars, own, favourite.
                let mut end = right;
                if favourite {
                    self.draw_icon.color = accent;
                    end -= text_right(&mut self.draw_icon, cx, end, mid, ICON_HEART) + 6.0;
                }
                if own {
                    self.draw_icon.color = muted;
                    end -= text_right(&mut self.draw_icon, cx, end, mid, ICON_OWN) + 6.0;
                }
                if stars > 0 {
                    self.draw_detail.color = muted;
                    end -= text_right(&mut self.draw_detail, cx, end, mid, &"★".repeat(stars as usize)) + 6.0;
                }
                let x = left + 16.0;
                self.draw_name.color = if playing { accent } else { text };
                let used = text_fit(&mut self.draw_name, cx, x, mid, end - x, &title);
                let rest = end - (x + used + 8.0);
                if !detail.is_empty() && rest > 36.0 {
                    self.draw_detail.color = with_alpha(muted, 0.9);
                    text_fit(&mut self.draw_detail, cx, x + used + 8.0, mid + 0.5, rest, &detail);
                }
            }
        }
    }
}

impl Widget for MilkdropBrowser {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let fresh = self.drawn_version != Some(self.browser.version);
        self.drawn_version = Some(self.browser.version);
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            let grid_ref = step.as_data_grid();
            let Some(mut grid) = grid_ref.borrow_mut() else { continue };
            let rows = self.browser.pane().rows.len();
            grid.set_grid_size(rows, 1);
            let width = grid.data_width();
            if width > 0.0 && (grid.col_width(0) - width).abs() > 0.5 {
                grid.set_col_widths(cx, &[width]);
            }
            let reveal = std::mem::take(&mut self.browser.reveal);
            let pane = self.browser.pane();
            let cursor_row = pane.cursor.and_then(|c| pane.row_of(c));
            if fresh {
                grid.set_selection(cx, cursor_row.map(|row| GridSelection { kind: GridSelectKind::Rows, anchor: (row, 0), head: (row, 0) }));
            }
            if reveal {
                if let Some(row) = cursor_row {
                    grid.scroll_cell_into_view(cx, row, 0);
                }
            }
            let current_folders = pane.current.map(|c| pane.index.ancestors(c)).unwrap_or_default();
            self.row_rects.clear();
            while let Some(cell) = grid.next_cell(cx) {
                let Some(row) = self.browser.pane().rows.get(cell.row).copied() else { continue };
                self.row_rects.push((cell.row, cell.rect));
                // Every row draws its tint (clear unless hovered) before its
                // text, so the tint's batch lies under the text's.
                self.draw_hover.color = if self.hover_row == Some(cell.row) { self.color_hover } else { vec4(0.0, 0.0, 0.0, 0.0) };
                self.draw_hover.draw_abs(cx, cell.rect);
                self.draw_row(cx, cell.rect, &row, &current_folders);
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The library loads whether or not the browser is shown: a host may
        // keep it folded and still step through it. Hidden widgets get every
        // event but pointer ones, so the first event starts it.
        if self.commands.is_none() {
            self.start_worker(cx);
        }
        if !self.pending.is_empty() {
            self.flush_commands();
        }
        if let Event::Signal = event {
            self.poll_worker(cx);
        }
        // The row under the pointer: a faint tint.
        if let Event::MouseMove(e) = event {
            let grid_area = self.view.widget(cx, ids!(grid)).area();
            let inside = grid_area.is_valid(cx) && grid_area.rect(cx).contains(e.abs);
            let over = if inside { self.row_rects.iter().find(|(_, r)| r.contains(e.abs)).map(|(row, _)| *row) } else { None };
            if over != self.hover_row {
                self.hover_row = over;
                self.view.widget(cx, ids!(grid)).redraw(cx);
            }
        }
        if self.preview_timer.is_event(event).is_some() {
            let item = self.preview.take();
            let pane = self.browser.pane();
            if let Some(item) = item.filter(|&i| pane.cursor == Some(RowKey::Item(i)) && pane.current != Some(i)) {
                self.activate_item(cx, item);
            }
        }
        // The keys the list answers itself (the grid would edit a cell on
        // Return and move between columns on Left and Right), and the keys
        // that take the search field down into the list.
        if let Event::KeyDown(ke) = event {
            let grid_area = self.view.widget(cx, ids!(grid)).area();
            let search_area = self.view.widget(cx, ids!(search)).area();
            let plain = !ke.modifiers.shift && !ke.modifiers.logo && !ke.modifiers.control && !ke.modifiers.alt;
            if plain && cx.has_key_focus(grid_area) {
                match ke.key_code {
                    KeyCode::ArrowLeft => return self.key_left(cx),
                    KeyCode::ArrowRight => return self.key_right(cx),
                    KeyCode::ReturnKey | KeyCode::NumpadEnter => {
                        if let Some(cursor) = self.browser.pane().cursor {
                            self.activate(cx, cursor);
                        }
                        return;
                    }
                    KeyCode::Escape => return self.clear_query(cx),
                    _ => {}
                }
            }
            if plain && cx.has_key_focus(search_area) && ke.key_code == KeyCode::ArrowDown {
                let pane = self.browser.pane();
                let row = pane.cursor.and_then(|c| pane.row_of(c)).unwrap_or(0);
                self.focus_list(cx);
                self.set_cursor_row(cx, row);
                return;
            }
        }
        self.view.handle_event(cx, event, scope);
        let Event::Actions(actions) = event else { return };
        if self.view.button(cx, ids!(download)).clicked(actions) {
            log!("milkdrop: download asked");
            self.downloading(cx, true);
            self.view.label(cx, ids!(progress)).set_text(cx, "Starting the download…");
            let cancel = CancelToken::new();
            self.cancel = Some(cancel.clone());
            self.command(Command::Download(cancel));
        }
        if self.view.button(cx, ids!(cancel)).clicked(actions) {
            if let Some(cancel) = self.cancel.take() {
                cancel.cancel();
            }
            self.downloading(cx, false);
            self.view.label(cx, ids!(progress)).set_text(cx, "Cancelling…");
        }
        let search = self.view.text_input(cx, ids!(search));
        if let Some(query) = search.changed(actions) {
            self.set_query(cx, query);
        }
        if search.escaped(actions) {
            self.clear_query(cx);
        }
        if search.returned(actions).is_some() {
            // Return in the field loads where the cursor stands: the first
            // match after typing.
            if let Some(cursor) = self.browser.pane().cursor {
                self.activate(cx, cursor);
            }
        }
        if let Some(on) = self.view.check_box(cx, ids!(favourites)).changed(actions) {
            self.browser.favourites_only = on;
            self.browser.presets.folds.shut_while_filtering.clear();
            self.refilter(cx);
        }
        if let Some(on) = self.view.check_box(cx, ids!(rated)).changed(actions) {
            self.browser.min_rating = if on { 1 } else { 0 };
            self.browser.presets.folds.shut_while_filtering.clear();
            self.refilter(cx);
        }
        let grid = self.view.data_grid(cx, ids!(grid));
        for action in grid.actions(actions) {
            match action {
                DataGridAction::CellClicked { row, .. } => {
                    let Some(key) = self.browser.pane().rows.get(row).map(|r| r.key) else { continue };
                    match key {
                        RowKey::Folder(folder) => {
                            // A double-click is two presses: fold once.
                            let repeat = self.last_fold.is_some_and(|(f, at)| f == folder && at.elapsed() < Duration::from_millis(350));
                            if !repeat {
                                self.last_fold = Some((folder, Instant::now()));
                                self.fold(cx, folder, None);
                            }
                        }
                        RowKey::Item(item) => {
                            let already = self.browser.pane().current == Some(item);
                            self.browser.pane_mut().cursor = Some(key);
                            if !already {
                                self.activate_item(cx, item);
                            }
                        }
                    }
                }
                DataGridAction::SelectionChanged { selection: Some(selection) } if selection.kind == GridSelectKind::Rows => {
                    let pane = self.browser.pane_mut();
                    if let Some(key) = pane.rows.get(selection.head.0).map(|r| r.key) {
                        let moved = pane.cursor != Some(key);
                        pane.cursor = Some(key);
                        // Moving through the list plays what it rests on.
                        if moved {
                            self.preview_later(cx, key);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

impl MilkdropBrowserRef {
    /// The next (`delta` 1) or previous (-1) preset in the shown list.
    pub fn step(&self, cx: &mut Cx, delta: isize) -> Option<PresetPick> {
        self.borrow_mut().and_then(|mut inner| inner.step(cx, delta))
    }

    /// A random preset of the shown list.
    pub fn random(&self, cx: &mut Cx) -> Option<PresetPick> {
        self.borrow_mut().and_then(|mut inner| inner.random(cx))
    }

    pub fn current(&self) -> Option<PresetPick> {
        self.borrow().and_then(|inner| inner.current())
    }

    pub fn toggle_favourite(&self, cx: &mut Cx) -> bool {
        self.borrow_mut().is_some_and(|mut inner| inner.toggle_favourite(cx))
    }

    pub fn rate(&self, cx: &mut Cx, stars: u8) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.rate(cx, stars);
        }
    }

    pub fn current_marks(&self) -> (bool, u8) {
        self.borrow().map(|inner| inner.current_marks()).unwrap_or_default()
    }

    pub fn is_ready(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.is_ready())
    }

    pub fn needs_pack(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.needs_pack())
    }

    /// See [`MilkdropBrowser::set_own_dir`].
    pub fn set_own_dir(&self, dir: PathBuf) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_own_dir(dir);
        }
    }

    /// See [`MilkdropBrowser::set_external`].
    pub fn set_external(&self, cx: &mut Cx, label: &str, items: Vec<BrowserItem>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_external(cx, label, items);
        }
    }

    /// A host's list is showing (not the presets).
    pub fn is_external(&self) -> bool {
        self.borrow().is_some_and(|inner| inner.browser.external.is_some())
    }

    pub fn clear_external(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.clear_external(cx);
        }
    }

    pub fn set_external_current(&self, cx: &mut Cx, key: Option<&str>) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_external_current(cx, key);
        }
    }

    pub fn reveal_current(&self, cx: &mut Cx) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.reveal_current(cx);
        }
    }

    /// The preset this browser asked to play in `actions`, if any.
    pub fn played(&self, actions: &Actions) -> Option<PresetPick> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            MilkdropBrowserAction::Play(pick) => Some(pick),
            _ => None,
        }
    }

    /// The host item (see `set_external`) picked in `actions`, by key.
    pub fn picked_item(&self, actions: &Actions) -> Option<String> {
        match actions.find_widget_action(self.widget_uid())?.cast() {
            MilkdropBrowserAction::PickItem(key) => Some(key),
            _ => None,
        }
    }
}

