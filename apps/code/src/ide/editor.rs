//! One editor tab: a file, editable, in the code view.
//!
//! CONTRACT (the shell calls these):
//! - `open(cx, root, rel)` loads the file (highlighted by its extension);
//!   the text is read and tokenized on a worker, the tab shows it when ready;
//! - `goto(cx, line, col)` puts the caret there and centres it (1-based),
//!   also right after `open`, before the text has arrived;
//! - `save(cx)` writes it (temp file + rename in the same folder, line ends
//!   and BOM as on disk); Cmd/Ctrl+S does the same while it has focus;
//! - `path()`;
//! - `check_disk(cx)` reloads a clean file that changed on disk, and shows
//!   a bar (Reload / Keep mine) over a dirty one; it also runs on its own
//!   when the code gets the keyboard and every 1.5 s while the window is
//!   active (a stat; the file is read only when its stamp moved);
//! - `agent_edited(cx, lines)` reloads after an agent edit and lights the
//!   changed 1-based line ranges, fading out over 1.5 s;
//! - it emits `FileEditorAction`s the shell uses for the tab title and the
//!   status bar.
//!
//! Keys in the code: Cmd/Ctrl+S save, Cmd/Ctrl+F find, Cmd/Ctrl+G (and
//! Ctrl+G) go to line, Cmd/Ctrl+Z undo, Shift+Cmd/Ctrl+Z or Ctrl+Y redo.

use makepad_code_editor::code_view::{CodeView, CodeViewAction};
use makepad_code_editor::document::PreparedDocument;
use makepad_code_editor::history::NewGroup;
use makepad_code_editor::selection::Affinity;
use makepad_code_editor::session::{PreparedView, SelectionMode};
use makepad_code_editor::decoration::{Decoration, DecorationType};
use makepad_code_editor::inlays::InlineInlay;
use makepad_code_editor::text::Position;
use makepad_code_editor::{CodeDocument, CodeSession};
use makepad_code_agent::runs::{Diagnostic, Level, RunManager};
use makepad_widgets::*;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Instant, SystemTime};

/// Larger files open with a notice instead of the text.
const MAX_FILE: u64 = 16 * 1024 * 1024;
/// How often the file's stamp is looked at while the window is active.
const POLL_SECONDS: f64 = 1.5;
/// An agent edit's wash: full strength this long, then fading out.
const WASH_HOLD: f64 = 0.3;
const WASH_SECONDS: f64 = 1.5;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.sdf.Finish

    mod.widgets.FileEditorBase = #(FileEditor::register_widget(vm))

    let BarText = Label{
        width: Fit
        height: Fit
        padding: 0
        text: ""
        draw_text +: {
            color: mod.look.text
            text_style: mod.look.mono{font_size: 8.5}
        }
    }

    /// A key in a bar: a chamfered hairline frame, the label in mono.
    let BarKey = ButtonFlat{
        height: 18
        padding: Inset{left: 8 right: 8 top: 0 bottom: 0}
        margin: 0
        draw_text +: {
            color: mod.look.text
            color_hover: mod.look.text_hi
            color_down: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            text_style: mod.look.mono{font_size: 8.0}
        }
        draw_bg +: {
            ink: uniform(mod.look.meta)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let d = Finish.sd_chamfer(p, c, c - vec2(0.5, 0.5), 3.0)
                let edge = Finish.band(d, 0.0, px, px)
                let fill = Finish.cover(d, px)
                let wash = 0.05 + 0.12 * self.hover + 0.22 * self.down
                let a = clamp(fill * wash + edge * (0.55 + 0.45 * max(self.hover, self.focus)), 0.0, 1.0)
                return vec4(self.ink.rgb * a, a)
            }
        }
    }

    /// A small key in the find bar: just the glyph, lit on hover.
    let BarGlyph = ButtonFlat{
        width: 20
        height: 18
        padding: 0
        margin: 0
        align: Align{x: 0.5 y: 0.5}
        draw_text +: {
            color: mod.look.meta
            color_hover: mod.look.text_hi
            color_down: uniform(mod.look.agent)
            color_focus: uniform(mod.look.text_hi)
            text_style: mod.look.mono{font_size: 8.5}
        }
        draw_bg +: {
            pixel: fn() {
                let a = 0.08 * self.hover + 0.16 * self.down
                return vec4(vec3(1.0) * a, a)
            }
        }
    }

    let BarField = TextInput{
        width: Fill
        height: Fit
        padding: Inset{left: 4 right: 4 top: 2 bottom: 2}
        draw_text +: {
            color: mod.look.text_hi
            color_hover: uniform(mod.look.text_hi)
            color_focus: uniform(mod.look.text_hi)
            color_down: uniform(mod.look.text_hi)
            color_empty: uniform(mod.look.dim)
            color_empty_hover: uniform(mod.look.dim)
            color_empty_focus: uniform(mod.look.dim)
            text_style: mod.look.mono{font_size: 8.5}
        }
        draw_cursor +: { color: mod.look.agent }
        draw_selection +: {
            color: mod.look.agent_select
            color_hover: mod.look.agent_select
            color_focus: mod.look.agent_select
        }
        draw_bg +: {
            color: #0000
            color_hover: #0000
            color_focus: #0000
            color_empty: #0000
            color_empty_hover: #0000
            color_empty_focus: #0000
            border_size: 0.0
        }
    }

    /// The floating bars over the code (find, go to line): a raised plate
    /// in a chamfered hairline frame.
    let FloatBar = View{
        height: Fit
        flow: Right
        spacing: 2
        align: Align{y: 0.5}
        padding: Inset{left: 6 right: 3 top: 3 bottom: 3}
        show_bg: true
        draw_bg +: {
            color: mod.look.raised
            ink: uniform(mod.look.hair_hi)
            pixel: fn() {
                let p = self.pos * self.rect_size
                let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                let c = self.rect_size * 0.5
                let d = Finish.sd_chamfer(p, c, c - vec2(0.5, 0.5), 4.0)
                let edge = Finish.band(d, 0.0, px, px)
                let fill = Finish.cover(d, px)
                let col = mix(self.color, self.ink, edge)
                let a = max(fill * 0.96, edge)
                return vec4(col.rgb * a, a)
            }
        }
    }

    let Count = BarText{
        padding: Inset{left: 4 right: 4}
        draw_text +: {
            color: mod.look.meta
            text_style: mod.look.mono{font_size: 8.0}
        }
    }

    mod.widgets.FileEditor = set_type_default() do mod.widgets.FileEditorBase{
        width: Fill
        height: Fill
        flow: Down

        // "<file> changed on disk   [Reload] [Keep mine]": over a dirty
        // file whose disk copy moved. Click it, then r / k.
        disk_bar := View{
            visible: false
            width: Fill
            height: 24
            flow: Right
            spacing: 6
            align: Align{y: 0.5}
            padding: Inset{left: 10 right: 6}
            show_bg: true
            draw_bg +: {
                color: mod.look.change
                hair: uniform(mod.look.hair_hi)
                pixel: fn() {
                    let p = self.pos * self.rect_size
                    let px = 1.0 / max(self.draw_pass.dpi_factor, 0.5)
                    let bar = 1.0 - step(2.0, p.x)
                    let line = 1.0 - step(px, self.rect_size.y - p.y)
                    let wash = vec4(self.color.rgb, 0.07)
                    let c = mix(mix(wash, vec4(self.color.rgb, 0.9), bar), self.hair, line)
                    return vec4(c.rgb * c.a, c.a)
                }
            }
            disk_text := BarText{
                draw_text +: { color: mod.look.change }
            }
            disk_keys := BarText{
                draw_text +: {
                    color: mod.look.meta
                    text_style: mod.look.mono{font_size: 7.5}
                }
            }
            View{ width: Fill height: 1 }
            reload := BarKey{ text: "Reload  r" }
            keep := BarKey{ text: "Keep mine  k" }
        }

        body := View{
            width: Fill
            height: Fill
            flow: Overlay

            code_box := View{
                width: Fill
                height: Fill
                code := CodeView{
                    editor +: {
                        height: Fill
                        read_only: false
                        word_wrap: false
                        show_gutter: true
                        empty_page_at_end: true
                        pad_left_top: vec2(6.0, 6.0)
                        draw_bg +: { color: #0000 }
                        draw_text +: { text_style: mod.look.mono{font_size: 8.5} }
                        draw_gutter +: {
                            text_style: mod.look.mono{font_size: 8.0}
                            color: mod.look.dim
                        }
                        draw_indent_guide +: { color: #x18202a }
                        draw_find_match +: { color: mod.look.agent }
                        draw_find_current +: { color: mod.look.agent }
                        draw_line_wash +: { color: #xffbe4d1c }
                        draw_cursor +: { color: mod.look.agent }
                        draw_cursor_bg +: {
                            pixel: fn() {
                                return vec4(0.0)
                            }
                        }
                        draw_selection +: {
                            sel: uniform(mod.look.agent_select)
                            pixel: fn() {
                                let sdf = Sdf2d.viewport(self.rect_pos + self.pos * self.rect_size)
                                sdf.box(self.rect_pos.x, self.rect_pos.y, self.rect_size.x, self.rect_size.y, 1.0)
                                if self.prev_w > 0.0 {
                                    sdf.box(self.prev_x, self.rect_pos.y - self.rect_size.y, self.prev_w, self.rect_size.y, 1.0)
                                    sdf.gloop(2.0)
                                }
                                if self.next_w > 0.0 {
                                    sdf.box(self.next_x, self.rect_pos.y + self.rect_size.y, self.next_w, self.rect_size.y, 1.0)
                                    sdf.gloop(2.0)
                                }
                                let c = vec4(self.sel.rgb, self.sel.a * (0.55 + 0.45 * self.focus))
                                return sdf.fill(c) * self.content_opacity
                            }
                        }
                        token_colors +: {
                            whitespace: #x2a3440
                            delimiter: #x8a97a8
                            delimiter_highlight: #xeaf2f8
                            unknown: #xc3cdd8
                            branch_keyword: #xff7ab4
                            constant: #xc6ef8e
                            identifier: #xc3cdd8
                            macro_identifier: #xb39dff
                            attribute: #xb39dff
                            loop_keyword: #xff7ab4
                            number: #xc6ef8e
                            other_keyword: #x5fdcea
                            punctuator: #x7d8a9a
                            string: #xffcf73
                            function: #xeaf2f8
                            typename: #x7fe6c8
                            comment: #x506273
                        }
                    }
                    find_bar: FloatBar{
                        width: 340
                        query := BarField{ empty_text: "find" }
                        count := Count{}
                        prev := BarGlyph{ text: "↑" }
                        next := BarGlyph{ text: "↓" }
                        close := BarGlyph{ text: "×" }
                    }
                    goto_bar: FloatBar{
                        width: 200
                        BarText{
                            text: "go to"
                            draw_text +: { color: mod.look.meta text_style: mod.look.mono{font_size: 8.0} }
                        }
                        line := BarField{ empty_text: "line[:col]" }
                        hint := Count{ draw_text +: { color: mod.look.bad } }
                    }
                    find_open +: {
                        open +: { draw_icon +: { color: mod.look.meta } }
                    }
                }
            }

            // Binary, too large, unreadable: a line instead of the text.
            notice := View{
                visible: false
                width: Fill
                height: Fill
                align: Align{x: 0.5 y: 0.4}
                notice_text := BarText{
                    draw_text +: { color: mod.look.meta }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum FileEditorAction {
    /// The dirty mark changed.
    Dirty(bool),
    /// The caret moved (1-based line and column), for the status bar.
    Caret { line: u32, col: u32 },
    Saved,
    #[default]
    None,
}

/// What a file looked like on disk when it was last looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    mtime: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Stamp { mtime: meta.modified().ok(), len: meta.len() }
    }
}

/// How the file's bytes map to the editor's text: line ends and a BOM come
/// back on save as they were.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextFormat {
    pub crlf: bool,
    pub bom: bool,
}

/// Bytes on disk to the editor's text (LF line ends, no BOM) and the format
/// to write it back in; `Err` with the notice to show for a file that is
/// not text.
pub fn decode_text(bytes: &[u8]) -> Result<(String, TextFormat), String> {
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return Err("binary file: not shown".to_string());
    }
    let (bytes, bom) = match bytes.strip_prefix(b"\xEF\xBB\xBF") {
        Some(rest) => (rest, true),
        None => (bytes, false),
    };
    let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8 text: not shown".to_string())?;
    let lf = text.matches('\n').count();
    let crlf_count = text.matches("\r\n").count();
    let crlf = lf > 0 && crlf_count * 2 >= lf;
    let text = if crlf_count > 0 { text.replace("\r\n", "\n") } else { text.to_string() };
    Ok((text, TextFormat { crlf, bom }))
}

/// The editor's text back to the bytes for disk, in the file's format.
pub fn encode_text(text: &str, format: TextFormat) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if format.bom {
        out.extend_from_slice(b"\xEF\xBB\xBF");
    }
    if format.crlf {
        out.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
    } else {
        out.extend_from_slice(text.as_bytes());
    }
    out
}

/// The extension of a `/`-separated path's file name, if any.
fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next()?;
    name.rsplit_once('.').map(|(_, ext)| ext).filter(|_| !name.starts_with('.') || name.matches('.').count() > 1)
}

/// FNV-1a over the bytes: tells a touched file from a changed one.
fn bytes_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &b in bytes {
        hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
    }
    hash
}

/// Write `bytes` to `path` atomically: a temp file in the same folder
/// (keeping the old file's permissions), then a rename over it. A symlink is
/// followed, so the link stays a link.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let temp = dir.join(format!(".{name}.save-{}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&temp, meta.permissions())?;
        }
        std::fs::rename(&temp, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// The bar over the code when the disk copy moved under edits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum DiskConflict {
    #[default]
    None,
    Changed,
    Deleted,
}

/// What a worker read and prepared.
enum LoadBody {
    Text { prepared: PreparedDocument, view: PreparedView, format: TextFormat },
    Notice(String),
}

struct Load {
    generation: u64,
    stamp: Option<Stamp>,
    hash: u64,
    body: LoadBody,
}

fn load_file(abs: &Path, rel: &str) -> (Option<Stamp>, u64, LoadBody) {
    let meta = match std::fs::metadata(abs) {
        Ok(meta) => meta,
        Err(e) => return (None, 0, LoadBody::Notice(format!("cannot read {rel}: {e}"))),
    };
    let stamp = Some(Stamp::of(&meta));
    if meta.len() > MAX_FILE {
        let note = format!("{rel} is {} MB: too large to edit here", meta.len() / (1024 * 1024));
        return (stamp, 0, LoadBody::Notice(note));
    }
    let bytes = match std::fs::read(abs) {
        Ok(bytes) => bytes,
        Err(e) => return (stamp, 0, LoadBody::Notice(format!("cannot read {rel}: {e}"))),
    };
    let hash = bytes_hash(&bytes);
    match decode_text(&bytes) {
        Ok((text, format)) => {
            let prepared = CodeDocument::prepare_for_path(rel, text.as_str().into());
            let view = CodeSession::prepare_view(&prepared, None);
            (stamp, hash, LoadBody::Text { prepared, view, format })
        }
        Err(note) => (stamp, hash, LoadBody::Notice(note)),
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct FileEditor {
    #[deref]
    view: View,
    #[rust]
    root: PathBuf,
    #[rust]
    path: Option<String>,
    #[rust]
    dirty: bool,
    /// The document's digest when it last matched the disk (None: never, a
    /// file kept after it was deleted).
    #[rust]
    saved_digest: Option<u64>,
    /// The document version `dirty` was computed for.
    #[rust]
    checked_version: u64,
    #[rust]
    format: TextFormat,
    /// The bytes the buffer's baseline came from (loaded or saved).
    #[rust]
    disk_hash: u64,
    /// The file's stamp when it was last looked at.
    #[rust]
    seen_stamp: Option<Stamp>,
    #[rust]
    conflict: DiskConflict,
    /// The text is shown (not a notice, not loading).
    #[rust]
    ready: bool,
    #[rust]
    loading: Option<Receiver<Load>>,
    #[rust]
    generation: u64,
    /// The load in flight is a reload (keep caret and scroll).
    #[rust]
    reloading: bool,
    #[rust]
    pending_goto: Option<(u32, Option<u32>)>,
    /// Agent lines (0-based ranges) to light once the reload lands.
    #[rust]
    pending_washes: Vec<std::ops::Range<usize>>,
    #[rust]
    washes: Vec<(std::ops::Range<usize>, Instant)>,
    #[rust]
    wash_frame: NextFrame,
    #[rust]
    focus_pending: bool,
    #[rust]
    had_focus: bool,
    #[rust]
    caret: (u32, u32),
    #[rust]
    poll: Timer,
    #[rust(true)]
    window_active: bool,
    /// The build and run errors marked in the text (underlined, the message
    /// after the line), and the run manager's generation they were read at.
    #[rust]
    marked: Vec<Diagnostic>,
    #[rust]
    marks_seen: u64,
}

impl FileEditor {
    pub fn open(&mut self, cx: &mut Cx, root: &Path, rel: &str) {
        self.root = root.to_path_buf();
        self.path = Some(rel.to_string());
        self.ready = false;
        self.saved_digest = None;
        self.conflict = DiskConflict::None;
        self.set_dirty(cx, false);
        self.start_load(cx, false);
        if self.poll.is_empty() {
            self.poll = cx.start_interval(POLL_SECONDS);
        }
    }

    /// The file (or a folder above it) was renamed: follow it, keeping the
    /// buffer, caret, scroll, dirty state and undo; the renamed file's stamp
    /// is the new baseline (no reload), and a new extension re-highlights.
    pub fn moved_to(&mut self, cx: &mut Cx, root: &Path, rel: &str) {
        let old_ext = self.path.as_deref().and_then(extension).map(str::to_string);
        self.root = root.to_path_buf();
        self.path = Some(rel.to_string());
        if let Some(abs) = self.abs() {
            self.seen_stamp = std::fs::metadata(&abs).ok().map(|m| Stamp::of(&m));
        }
        if self.conflict == DiskConflict::Deleted {
            self.set_conflict(cx, DiskConflict::None);
        }
        if old_ext.as_deref() != extension(rel) {
            if let Some(view) = self.code(cx).borrow::<CodeView>() {
                if let Some(session) = view.session.as_ref() {
                    session.document().set_language_for_path(rel);
                }
            }
            self.view.redraw(cx);
        }
    }

    pub fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    fn abs(&self) -> Option<PathBuf> {
        Some(self.root.join(self.path.as_ref()?))
    }

    fn code(&self, cx: &mut Cx) -> WidgetRef {
        self.view.widget(cx, ids!(code))
    }

    pub fn save(&mut self, cx: &mut Cx) -> std::io::Result<()> {
        let Some(abs) = self.abs() else { return Ok(()) };
        if !self.ready {
            return Ok(());
        }
        let code = self.code(cx);
        let Some(view) = code.borrow::<CodeView>() else { return Ok(()) };
        let Some(session) = view.session.as_ref() else { return Ok(()) };
        let text = session.document().as_text().to_string();
        let digest = session.document().digest();
        drop(view);
        let bytes = encode_text(&text, self.format);
        if let Err(e) = write_atomic(&abs, &bytes) {
            log!("code: could not save {}: {e}", abs.display());
            return Err(e);
        }
        self.disk_hash = bytes_hash(&bytes);
        self.seen_stamp = std::fs::metadata(&abs).ok().map(|m| Stamp::of(&m));
        self.saved_digest = Some(digest);
        self.set_conflict(cx, DiskConflict::None);
        self.set_dirty(cx, false);
        cx.widget_action(self.widget_uid(), FileEditorAction::Saved);
        Ok(())
    }

    pub fn goto(&mut self, cx: &mut Cx, line: u32, col: Option<u32>) {
        if !self.ready {
            self.pending_goto = Some((line, col));
            return;
        }
        if let Some(mut view) = self.code(cx).borrow_mut::<CodeView>() {
            view.goto_line(cx, line as usize, col.map(|c| c as usize));
        }
        self.emit_caret(cx);
    }

    /// Look at the file on disk: a clean buffer follows it, a dirty one gets
    /// the bar. Only a stat unless the file's stamp moved.
    pub fn check_disk(&mut self, cx: &mut Cx) {
        let Some(abs) = self.abs() else { return };
        if self.loading.is_some() {
            return;
        }
        let meta = match std::fs::metadata(&abs) {
            Ok(meta) => meta,
            Err(_) => {
                if self.seen_stamp.is_some() || self.conflict == DiskConflict::Changed {
                    self.seen_stamp = None;
                    self.set_conflict(cx, DiskConflict::Deleted);
                }
                return;
            }
        };
        let stamp = Some(Stamp::of(&meta));
        if stamp == self.seen_stamp {
            return;
        }
        self.seen_stamp = stamp;
        let Ok(bytes) = std::fs::read(&abs) else { return };
        if bytes_hash(&bytes) == self.disk_hash {
            // Touched, or back as it was.
            if self.conflict != DiskConflict::None {
                self.set_conflict(cx, DiskConflict::None);
            }
            return;
        }
        if self.dirty {
            self.set_conflict(cx, DiskConflict::Changed);
        } else {
            self.start_load(cx, true);
        }
    }

    /// The agent changed the file: reload it (or show the bar over edits)
    /// and light these 1-based line ranges (inclusive) for a moment.
    pub fn agent_edited(&mut self, cx: &mut Cx, lines: &[(u32, u32)]) {
        self.pending_washes = lines
            .iter()
            .map(|&(a, b)| (a.max(1) as usize - 1)..(b.max(a).max(1) as usize))
            .collect();
        if self.dirty {
            self.set_conflict(cx, DiskConflict::Changed);
        } else {
            self.start_load(cx, true);
        }
    }

    /// Give the keyboard to the code (now, or on the first draw).
    pub fn focus(&mut self, cx: &mut Cx) {
        self.focus_pending = true;
        self.view.redraw(cx);
    }

    fn start_load(&mut self, cx: &mut Cx, reload: bool) {
        let Some(abs) = self.abs() else { return };
        let rel = self.path.clone().unwrap_or_default();
        self.generation += 1;
        self.reloading = reload && self.ready;
        let generation = self.generation;
        let (tx, rx) = channel();
        self.loading = Some(rx);
        std::thread::spawn(move || {
            let (stamp, hash, body) = load_file(&abs, &rel);
            let _ = tx.send(Load { generation, stamp, hash, body });
            SignalToUI::set_ui_signal();
        });
        self.view.redraw(cx);
    }

    fn poll_load(&mut self, cx: &mut Cx) {
        let Some(rx) = self.loading.as_ref() else { return };
        let Ok(load) = rx.try_recv() else { return };
        self.loading = None;
        if load.generation != self.generation {
            return;
        }
        self.seen_stamp = load.stamp;
        match load.body {
            LoadBody::Notice(note) => {
                self.ready = false;
                self.disk_hash = load.hash;
                self.view.label(cx, ids!(notice_text)).set_text(cx, &note);
                self.view.view(cx, ids!(notice)).set_visible(cx, true);
                self.view.view(cx, ids!(code_box)).set_visible(cx, false);
                if load.stamp.is_none() {
                    self.set_conflict(cx, DiskConflict::Deleted);
                }
            }
            LoadBody::Text { prepared, view, format } => {
                // Typed into while the reload was read: the edits win, the bar asks.
                if self.reloading && self.dirty {
                    self.set_conflict(cx, DiskConflict::Changed);
                    return;
                }
                let document = CodeDocument::from_prepared(prepared);
                let digest = document.digest();
                let version = document.version();
                let session = match CodeSession::from_prepared(document.clone(), view) {
                    Ok(session) => session,
                    Err(_) => CodeSession::new(document),
                };
                let code = self.code(cx);
                if let Some(mut code_view) = code.borrow_mut::<CodeView>() {
                    let old_caret = code_view.session.as_ref().map(|s| {
                        let sel = s.selections()[s.last_added_selection_index().unwrap_or(0)];
                        sel.cursor.position
                    });
                    if let (true, Some(caret)) = (self.reloading, old_caret) {
                        let caret = session.clamp_position(caret);
                        session.set_selection(caret, Affinity::Before, SelectionMode::Simple, NewGroup::Yes);
                    }
                    code_view.set_session(cx, session);
                    code_view.editor.line_washes.clear();
                }
                self.view.view(cx, ids!(notice)).set_visible(cx, false);
                self.view.view(cx, ids!(code_box)).set_visible(cx, true);
                self.format = format;
                self.disk_hash = load.hash;
                self.saved_digest = Some(digest);
                self.checked_version = version;
                self.ready = true;
                self.set_conflict(cx, DiskConflict::None);
                self.set_dirty(cx, false);
                if let Some((line, col)) = self.pending_goto.take() {
                    self.goto(cx, line, col);
                }
                self.marked.clear();
                self.mark_diagnostics(cx, true);
                let washes = std::mem::take(&mut self.pending_washes);
                if !washes.is_empty() {
                    self.light(cx, washes);
                }
                self.emit_caret(cx);
            }
        }
        self.reloading = false;
        self.view.redraw(cx);
    }

    /// Mark the errors the run manager has for this file (the newest build of
    /// each app, the code of each `makepad run` as last saved): each place
    /// underlined, its message written after its line. `force`: the document
    /// is new (a load), mark even when the errors did not change.
    fn mark_diagnostics(&mut self, cx: &mut Cx, force: bool) {
        static HOOKED: std::sync::Once = std::sync::Once::new();
        HOOKED.call_once(|| RunManager::global().on_change(std::sync::Arc::new(|| SignalToUI::set_ui_signal())));
        let m = RunManager::global();
        let generation = m.generation();
        if !self.ready || (!force && generation == self.marks_seen) {
            return;
        }
        self.marks_seen = generation;
        let Some(path) = self.path.clone() else { return };
        let diags = m.diagnostics_for(&path);
        if !force && diags == self.marked {
            return;
        }
        let code = self.code(cx);
        let Some(mut code_view) = code.borrow_mut::<CodeView>() else { return };
        let Some(session) = code_view.session.as_ref() else { return };
        let lines: Vec<String> = session.document().as_text().as_lines().to_vec();
        let (decorations, inlays) = diagnostic_marks(&lines, &diags);
        let mut document = session.document().clone();
        if document.replace_prepared_decorations(decorations).is_err() {
            return;
        }
        document.set_inline_inlays(inlays);
        session.relayout_inline();
        code_view.redraw(cx);
        self.marked = diags;
    }

    /// Wash these lines, fading; bring the first into view if it is off.
    fn light(&mut self, cx: &mut Cx, lines: Vec<std::ops::Range<usize>>) {
        let now = Instant::now();
        let first = lines.iter().map(|r| r.start).min();
        self.washes = lines.into_iter().map(|r| (r, now)).collect();
        if let Some(mut code_view) = self.code(cx).borrow_mut::<CodeView>() {
            if let Some(first) = first {
                let visible = code_view.editor.visible_lines();
                if !visible.contains(&first) || first + 1 >= visible.end {
                    code_view.editor.center_line(cx, first);
                }
            }
        }
        self.step_washes(cx);
    }

    fn step_washes(&mut self, cx: &mut Cx) {
        let now = Instant::now();
        self.washes.retain(|(_, at)| now.duration_since(*at).as_secs_f64() < WASH_HOLD + WASH_SECONDS);
        let washes: Vec<_> = self
            .washes
            .iter()
            .map(|(range, at)| {
                let t = (now.duration_since(*at).as_secs_f64() - WASH_HOLD).max(0.0) / WASH_SECONDS;
                let fade = 1.0 - t.min(1.0);
                (range.clone(), (fade * fade) as f32)
            })
            .collect();
        if let Some(mut code_view) = self.code(cx).borrow_mut::<CodeView>() {
            code_view.editor.line_washes = washes;
            code_view.redraw(cx);
        }
        if !self.washes.is_empty() {
            self.wash_frame = cx.new_next_frame();
        }
    }

    fn set_dirty(&mut self, cx: &mut Cx, dirty: bool) {
        if self.dirty != dirty {
            self.dirty = dirty;
            cx.widget_action(self.widget_uid(), FileEditorAction::Dirty(dirty));
        }
    }

    /// Recompute dirty when the document moved: it differs from what was
    /// last loaded or saved (an undo back to it is clean again).
    fn update_dirty(&mut self, cx: &mut Cx) {
        let code = self.code(cx);
        let Some(view) = code.borrow::<CodeView>() else { return };
        let Some(session) = view.session.as_ref() else { return };
        let version = session.document().version();
        if version == self.checked_version {
            return;
        }
        self.checked_version = version;
        let dirty = Some(session.document().digest()) != self.saved_digest;
        drop(view);
        self.set_dirty(cx, dirty);
    }

    fn emit_caret(&mut self, cx: &mut Cx) {
        let code = self.code(cx);
        let Some(view) = code.borrow::<CodeView>() else { return };
        let Some(session) = view.session.as_ref() else { return };
        let position = session.selections()[session.last_added_selection_index().unwrap_or(0)].cursor.position;
        let col = session
            .document()
            .as_text()
            .as_lines()
            .get(position.line_index)
            .map(|line| line.get(..position.byte_index).map(|s| s.chars().count()).unwrap_or(0))
            .unwrap_or(0);
        let caret = (position.line_index as u32 + 1, col as u32 + 1);
        drop(view);
        if caret != self.caret {
            self.caret = caret;
            cx.widget_action(self.widget_uid(), FileEditorAction::Caret { line: caret.0, col: caret.1 });
        }
    }

    fn set_conflict(&mut self, cx: &mut Cx, conflict: DiskConflict) {
        if self.conflict == conflict {
            return;
        }
        self.conflict = conflict;
        let name = self.path.as_deref().map(|p| p.rsplit('/').next().unwrap_or(p)).unwrap_or("");
        let (text, keys) = match conflict {
            DiskConflict::None => (String::new(), ""),
            DiskConflict::Changed => (format!("{name} changed on disk"), "click, then r / k"),
            DiskConflict::Deleted => (format!("{name} deleted on disk"), "save writes it back"),
        };
        self.view.label(cx, ids!(disk_text)).set_text(cx, &text);
        self.view.label(cx, ids!(disk_keys)).set_text(cx, keys);
        self.view.button(cx, ids!(reload)).set_visible(cx, conflict == DiskConflict::Changed);
        self.view.view(cx, ids!(disk_bar)).set_visible(cx, conflict != DiskConflict::None);
        self.view.redraw(cx);
    }

    fn reload_from_disk(&mut self, cx: &mut Cx) {
        self.set_conflict(cx, DiskConflict::None);
        // The edits are dropped: the reload is not blocked by them.
        self.dirty = false;
        cx.widget_action(self.widget_uid(), FileEditorAction::Dirty(false));
        self.start_load(cx, true);
        self.focus(cx);
    }

    fn keep_mine(&mut self, cx: &mut Cx) {
        if self.conflict == DiskConflict::Deleted {
            // Nothing on disk matches the buffer any more.
            self.saved_digest = None;
            self.set_dirty(cx, true);
        }
        self.pending_washes.clear();
        self.set_conflict(cx, DiskConflict::None);
        self.focus(cx);
    }

    fn handle_bar(&mut self, cx: &mut Cx, event: &Event, actions: &Actions) {
        if self.conflict == DiskConflict::None {
            return;
        }
        if self.view.button(cx, ids!(reload)).clicked(actions) && self.conflict == DiskConflict::Changed {
            self.reload_from_disk(cx);
            return;
        }
        if self.view.button(cx, ids!(keep)).clicked(actions) {
            self.keep_mine(cx);
            return;
        }
        let area = self.view.view(cx, ids!(disk_bar)).area();
        match event.hits(cx, area) {
            Hit::FingerDown(_) => cx.set_key_focus(area),
            Hit::KeyDown(KeyEvent { key_code: KeyCode::KeyR, .. }) if self.conflict == DiskConflict::Changed => {
                self.reload_from_disk(cx)
            }
            Hit::KeyDown(KeyEvent { key_code: KeyCode::KeyK, .. }) => self.keep_mine(cx),
            _ => {}
        }
    }
}

impl FileEditor {
    /// Files dropped on the editor: their paths are typed at the caret (a
    /// path with spaces in double quotes), as the terminal does.
    fn drop_paths(&mut self, cx: &mut Cx, scope: &mut Scope, drop: &DropEvent) {
        if !self.ready || *drop.handled.lock().unwrap_or_else(|e| e.into_inner()) {
            return;
        }
        let code = self.code(cx);
        if !code.area().clipped_rect(cx).contains(drop.abs) {
            return;
        }
        let text = drop_text(&drop.items);
        if text.is_empty() {
            return;
        }
        *drop.handled.lock().unwrap_or_else(|e| e.into_inner()) = true;
        if let Some(mut code_view) = code.borrow_mut::<CodeView>() {
            code_view.editor.set_key_focus(cx);
        }
        let mut input = TextInputEvent::default();
        input.input = text;
        input.was_paste = true;
        self.view.handle_event(cx, &Event::TextInput(input), scope);
    }
}

/// The paths a drop carries, one per line's worth of text: separated by
/// spaces, a path with a space quoted.
fn drop_text(items: &[DragItem]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            DragItem::FilePath { path, internal_id: None } => {
                Some(if path.contains(' ') { format!("\"{path}\"") } else { path.clone() })
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Widget for FileEditor {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        match event {
            Event::Signal => {
                self.poll_load(cx);
                self.mark_diagnostics(cx, false);
            }
            Event::WindowLostFocus(_) => self.window_active = false,
            Event::WindowGotFocus(_) => {
                self.window_active = true;
                self.check_disk(cx);
            }
            _ => {}
        }
        if let Event::Drop(drop) = event {
            self.drop_paths(cx, scope, drop);
        }
        if self.poll.is_event(event).is_some() && self.window_active {
            self.check_disk(cx);
        }
        if self.wash_frame.is_event(event).is_some() {
            self.step_washes(cx);
        }
        let actions = cx.capture_actions(|cx| self.view.handle_event(cx, event, scope));
        self.handle_bar(cx, event, &actions);
        if !self.ready {
            return;
        }
        let code = self.code(cx);
        let uid = code.widget_uid();
        let mut changed = false;
        for action in actions.filter_widget_actions(uid) {
            match action.cast::<CodeViewAction>() {
                CodeViewAction::Changed => changed = true,
                CodeViewAction::UnhandledKey(KeyEvent { key_code: KeyCode::KeyS, modifiers, .. })
                    if modifiers.is_primary() && !modifiers.shift && !modifiers.alt =>
                {
                    let _ = self.save(cx);
                }
                _ => {}
            }
        }
        if changed {
            self.update_dirty(cx);
        }
        if event.requires_visibility() || changed || matches!(event, Event::KeyDown(_) | Event::Signal) {
            self.emit_caret(cx);
        }
        let focus = cx.has_key_focus(code.area());
        if focus && !self.had_focus {
            self.check_disk(cx);
        }
        self.had_focus = focus;
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let step = self.view.draw_walk(cx, scope, walk);
        if self.focus_pending && self.ready {
            self.focus_pending = false;
            if let Some(mut code_view) = self.code(cx).borrow_mut::<CodeView>() {
                code_view.editor.set_key_focus(cx);
            }
        }
        step
    }
}

/// The editor marks of `diags` in a text of `lines`: an underline from each
/// place over the identifier there (one character otherwise; the whole line
/// when the column is past its end), sorted and never overlapping, and one
/// text after each line naming its errors ("✗ message", warnings "⚠").
/// Places outside the text are left out.
pub fn diagnostic_marks(lines: &[String], diags: &[Diagnostic]) -> (Vec<Decoration>, Vec<(usize, usize, InlineInlay)>) {
    let mut spans: Vec<(Position, Position, DecorationType)> = Vec::new();
    let mut notes: Vec<(usize, String)> = Vec::new();
    for d in diags {
        let Some(index) = (d.line as usize).checked_sub(1) else { continue };
        let Some(text) = lines.get(index) else { continue };
        let ty = if d.level == Level::Error { DecorationType::Error } else { DecorationType::Warning };
        let col = (d.col as usize).saturating_sub(1);
        let (start, end) = match text.char_indices().nth(col) {
            Some((start, c)) => {
                let word = |c: char| c.is_alphanumeric() || c == '_';
                let end = if word(c) {
                    text[start..].char_indices().find(|&(_, c)| !word(c)).map_or(text.len(), |(i, _)| start + i)
                } else {
                    start + c.len_utf8()
                };
                (start, end)
            }
            None => (text.len() - text.trim_start().len(), text.len()),
        };
        if end > start {
            spans.push((Position { line_index: index, byte_index: start }, Position { line_index: index, byte_index: end }, ty));
        }
        let mark = if d.level == Level::Error { "✗" } else { "⚠" };
        match notes.iter_mut().find(|(line, _)| *line == index) {
            Some((_, note)) => note.push_str(&format!("  ·  {mark} {}", d.message)),
            None => notes.push((index, format!("    {mark} {}", d.message))),
        }
    }
    spans.sort_by_key(|(start, _, _)| *start);
    let mut decorations: Vec<Decoration> = Vec::new();
    for (start, end, ty) in spans {
        if decorations.last().is_some_and(|last| last.end() > start) {
            continue;
        }
        decorations.push(Decoration::new(decorations.len(), start, end, ty));
    }
    let inlays = notes.into_iter().map(|(line, note)| (line, lines[line].len(), InlineInlay::Text(note))).collect();
    (decorations, inlays)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run's error marks its place in the editor: the identifier at the
    /// column underlined, the message after the line; a column past the end
    /// underlines the line; places outside the file are left out.
    #[test]
    fn a_run_error_is_marked_at_its_line() {
        let lines: Vec<String> = ["fn main() {", "    let bad: u32 = \"a\";", "}"].iter().map(|l| l.to_string()).collect();
        let at = |line, col, level, message: &str| Diagnostic { level, message: message.into(), path: Some("src/main.rs".into()), line, col };
        let diags = [
            at(2, 20, Level::Error, "E0308: mismatched types"),
            at(2, 9, Level::Warning, "unused variable"),
            at(3, 9, Level::Error, "past the end"),
            at(9, 1, Level::Error, "outside"),
        ];
        let (decorations, inlays) = diagnostic_marks(&lines, &diags);
        let spans: Vec<_> = decorations.iter().map(|d| (d.start().line_index, d.start().byte_index, d.end().byte_index, d.ty)).collect();
        assert_eq!(spans, [(1, 8, 11, DecorationType::Warning), (1, 19, 20, DecorationType::Error), (2, 0, 1, DecorationType::Error)]);
        assert_eq!(inlays.len(), 2);
        assert_eq!(inlays[0], (1, lines[1].len(), InlineInlay::Text("    ✗ E0308: mismatched types  ·  ⚠ unused variable".into())));
        assert_eq!(inlays[1].0, 2);
    }

    #[test]
    fn crlf_and_bom_round_trip() {
        let disk = b"\xEF\xBB\xBFa\r\nb\r\n";
        let (text, format) = decode_text(disk).unwrap();
        assert_eq!(text, "a\nb\n");
        assert_eq!(format, TextFormat { crlf: true, bom: true });
        assert_eq!(encode_text(&text, format), disk.to_vec());
    }

    #[test]
    fn lf_without_trailing_newline_stays() {
        let (text, format) = decode_text(b"x\ny").unwrap();
        assert_eq!(text, "x\ny");
        assert_eq!(format, TextFormat::default());
        assert_eq!(encode_text(&text, format), b"x\ny".to_vec());
    }

    #[test]
    fn dropped_paths_are_typed_as_text_and_spaces_are_quoted() {
        let items = vec![
            DragItem::FilePath { path: "/h/Screenshots/a.png".into(), internal_id: None },
            DragItem::FilePath { path: "/h/my shots/b.png".into(), internal_id: None },
            DragItem::String { value: "x".into(), internal_id: None },
            DragItem::FilePath { path: "/own".into(), internal_id: Some(LiveId(1)) },
        ];
        assert_eq!(drop_text(&items), "/h/Screenshots/a.png \"/h/my shots/b.png\"");
        assert_eq!(drop_text(&[]), "");
    }

    #[test]
    fn extension_of_a_path() {
        assert_eq!(extension("src/a.rs"), Some("rs"));
        assert_eq!(extension("a.d/Makefile"), None);
        assert_eq!(extension(".gitignore"), None);
        assert_eq!(extension("x/.env.local"), Some("local"));
    }

    #[test]
    fn binary_and_non_utf8_are_notices() {
        assert!(decode_text(b"ab\0cd").is_err());
        assert!(decode_text(&[0x66, 0xff, 0xfe]).is_err());
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("code-editor-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f.txt");
        std::fs::write(&file, "old").unwrap();
        write_atomic(&file, b"new").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1);
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
