//! Plain full-screen views in the terminal's own colours: a header, sections
//! of rows, one status line (questions and progress live there too) and a
//! key-hint footer. Details go to the log file, never onto the screen.
use super::{clean, console, line, Key};
use crate::progress;
use std::{
    cell::{Cell as StdCell, RefCell},
    env, fs,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

pub(super) const PLAIN: &str = "0";
pub(super) const DIM: &str = "2";
pub(super) const BOLD: &str = "1";
pub(super) const OK: &str = "32";
pub(super) const WARN: &str = "33";
/// The accent: Makepad orange, for markers, spinners and progress bars.
pub(super) const ACC: &str = "38;2;255;92;57";
const INV: &str = "7";
/// A key letter in the footer: bold Makepad orange.
const KEY: &str = "1;38;2;255;92;57";
/// Makepad orange: the ▌ on the selected row and the filled part of
/// progress bars; never a background or an action.
const MARK: &str = "38;2;255;92;57";
/// The Makepad mark drawn faintly behind the rows (braille dots).
const LOGO: &str = include_str!("../logo.txt");
/// Faint colours for the selection band and the logo, for a dark or a
/// light terminal background (see `set_light_background`).
const BAND_DARK: &str = "48;2;42;44;48";
const BAND_LIGHT: &str = "48;2;228;229;231";
const LOGO_DARK: &str = "38;2;44;47;52";
const LOGO_LIGHT: &str = "38;2;226;227;229";

#[derive(Clone)]
pub(super) struct Span(pub String, pub &'static str);
pub(super) type Text = Vec<Span>;
pub(super) fn text(value: impl Into<String>, style: &'static str) -> Text {
    vec![Span(value.into(), style)]
}
/// A green check mark followed by plain text.
pub(super) fn done(value: impl Into<String>) -> Text {
    vec![Span("✓".into(), OK), Span(format!(" {}", value.into()), PLAIN)]
}
pub(super) fn plain_text(value: &Text) -> String {
    value.iter().map(|s| s.0.as_str()).collect()
}

#[derive(Clone)]
pub(super) struct Item {
    pub id: String,
    pub name: String,
    /// "commercial", "beta", "free" or empty (no license column).
    pub license: &'static str,
    pub status: Text,
    /// Shown on the selected row only, as `<action> ⏎`.
    pub action: String,
    /// A row inside an open tree node (the experiments): indented.
    pub child: bool,
    /// Said on the line under the rule while the row is selected.
    pub info: String,
    /// An app whose source is here: e sends the person's changes.
    pub send: bool,
}
#[derive(Clone)]
pub(super) enum Row {
    Head(String),
    Note(Text),
    Item(Item),
}
#[derive(Clone, Default)]
pub(super) struct View {
    pub crumb: String,
    pub subtitle: String,
    pub email: String,
    pub rows: Vec<Row>,
    /// Sub-screens: Escape goes back.
    pub back: bool,
    /// Name column width (15 unless a screen needs more).
    pub name_width: usize,
    /// Replaces the key-hint footer (a consent screen cancels, not quits).
    pub footer: Option<&'static str>,
}
pub(super) enum Nav {
    Select(String),
    Back,
    Quit,
    /// Something changed in the background (an app exited, disk measured):
    /// rebuild the view and call `menu` again.
    Refresh,
    /// A letter key on the main menu (c cancel, s start when done, e send
    /// my changes) and the selected row's id.
    Key(char, String),
}

/// What runs in the background, as the menu shows it: set by the session
/// whenever it changes, the progress on every tick.
#[derive(Clone, Default)]
pub(super) struct Background {
    /// The app compiling now.
    pub building: Option<Building>,
    /// Waiting their turn, in order: (row id, opens when done).
    pub queue: Vec<(String, bool)>,
    /// Experiments Compile all queued that still wait to compile only.
    pub batch: usize,
    /// The update check and its progress.
    pub update: Option<Doing>,
    /// Local AI's components installing, on its SETUP row.
    pub setup: Option<Doing>,
}
#[derive(Clone)]
pub(super) struct Building {
    pub id: String,
    pub title: String,
    /// Opens by itself when it is done (Return on its row; s turns it off).
    pub open: bool,
    /// Cancelled; its compiler is being stopped.
    pub stopping: bool,
    pub doing: Doing,
}
/// A step on a row: what it does, how far when known, and the amount.
#[derive(Clone, Default)]
pub(super) struct Doing {
    pub step: String,
    pub fraction: Option<f64>,
    pub amount: String,
}
pub(super) fn set_background(background: Background) {
    BACKGROUND.with(|b| *b.borrow_mut() = background);
}

thread_local! {
    static VIEW: RefCell<View> = RefCell::new(View::default());
    static SELECTED: StdCell<Option<usize>> = const { StdCell::new(None) };
    static SCROLL: StdCell<usize> = const { StdCell::new(0) };
    static MESSAGE: RefCell<Text> = const { RefCell::new(Vec::new()) };
    static CHOICE: RefCell<Text> = const { RefCell::new(Vec::new()) };
    static FOOTER: StdCell<Option<&'static str>> = const { StdCell::new(None) };
    static COLOR: StdCell<bool> = const { StdCell::new(false) };
    static LIGHT: StdCell<bool> = const { StdCell::new(false) };
    /// The row last worked on; `menu` starts its selection there once.
    static WORKED_ON: RefCell<Option<String>> = const { RefCell::new(None) };
    static BACKGROUND: RefCell<Background> = RefCell::new(Background::default());
    /// The question being answered, if any (see `Prompt`).
    static PROMPT: RefCell<Option<Prompt>> = const { RefCell::new(None) };
}
/// A question being answered (an email, a folder, a choice): every prompt
/// is shown the same way, as rows at the end of the content above the
/// rule, its active row selected like a menu row. The line under the rule
/// stays for messages.
struct Prompt {
    lines: Vec<Text>,
    active: usize,
}
fn set_prompt(prompt: Option<Prompt>) {
    PROMPT.with(|p| *p.borrow_mut() = prompt);
}
/// The question's own lines: a blank one, then the question wrapped.
fn prompt_head(question: &str, note: &str) -> Vec<Text> {
    let mut lines = vec![Vec::new()];
    for line in wrap_text(&text(clean(question), PLAIN), 72, 4) {
        let mut spans = vec![Span("    ".into(), PLAIN)];
        spans.extend(line);
        lines.push(spans);
    }
    if !note.is_empty() {
        lines.push(vec![Span("    ".into(), PLAIN), Span(clean(note), DIM)]);
    }
    lines
}

/// Setup detail (stages, compiler output summaries, errors) goes to this file.
pub(super) fn set_log(path: PathBuf) {
    // Start the log over (truncate, never delete) once it grew large.
    if fs::metadata(&path).is_ok_and(|m| m.len() > 4 * 1024 * 1024) {
        let _ = fs::File::create(&path);
    }
    let _ = LOG_PATH.set(path);
}
/// One log for the session, written from the terminal's thread and from the
/// threads that build and update in the background.
static LOG_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
pub(super) fn log_path() -> Option<PathBuf> {
    LOG_PATH.get().cloned()
}
pub(super) fn activity(value: &str) {
    let lines: String = value.lines().map(clean).filter(|s| !s.is_empty()).map(|s| s + "\n").collect();
    if lines.is_empty() {
        return;
    }
    if let Some(path) = log_path() {
        // Seconds since the Builder started, so the log shows where time goes.
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let at = START.get_or_init(Instant::now).elapsed().as_secs_f64();
        let stamped: String = lines.lines().map(|line| format!("{at:8.2} {line}\n")).collect();
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(stamped.as_bytes());
        }
    }
    if !COLOR.with(StdCell::get) && !progress::active() {
        print!("{lines}");
    }
}
/// The status line shown until the next key press.
pub(super) fn message(value: Text) {
    if !COLOR.with(StdCell::get) {
        println!("{}", plain_text(&value));
    }
    MESSAGE.with(|m| *m.borrow_mut() = value);
}
pub(super) fn warn(value: &str) {
    activity(value);
    message(text(clean(value.lines().next().unwrap_or_default()), WARN));
}
/// Show short blocking work on the status line.
pub(super) fn busy(label: &str) {
    MESSAGE.with(|m| *m.borrow_mut() = vec![Span("⠋".into(), ACC), Span(format!(" {label}"), PLAIN)]);
    if COLOR.with(StdCell::get) {
        draw();
    } else {
        println!("{label}");
    }
}
/// Work started from a row: show it on that row of the current screen
/// ("installing…", "compiling…") and switch the footer until the work ends.
pub(super) fn working(id: &str, label: &str) {
    // The selection moves to the row being worked on and stays there
    // afterwards (the next `menu` call starts on it).
    VIEW.with(|v| {
        let mut index = 0;
        for row in v.borrow_mut().rows.iter_mut() {
            if let Row::Item(item) = row {
                if item.id == id {
                    // An orange spinner (animated as the screen redraws)
                    // and what is going on.
                    item.status = vec![Span(SPINNER[0].to_string(), ACC), Span(format!(" {label}"), PLAIN)];
                    item.action.clear();
                    SELECTED.with(|s| s.set(Some(index)));
                }
                index += 1;
            }
        }
    });
    WORKED_ON.with(|w| *w.borrow_mut() = Some(id.to_owned()));
    MESSAGE.with(|m| m.borrow_mut().clear());
    FOOTER.with(|f| f.set(Some("working · ctrl+c stops")));
    draw();
}
/// The full-screen view is on (a terminal); otherwise plain lines.
pub(super) fn full_screen() -> bool {
    COLOR.with(StdCell::get)
}
pub(super) fn set_view(view: View) {
    VIEW.with(|v| *v.borrow_mut() = view);
    SELECTED.with(|s| s.set(None));
}

/// The terminal's background is light: the band and the logo use light
/// greys. Dark (the default) when the terminal does not say.
pub(super) fn set_light_background(light: bool) {
    LIGHT.with(|l| l.set(light));
}
fn ansi(style: &str, band: bool) -> String {
    if band {
        let back = if LIGHT.with(StdCell::get) { BAND_LIGHT } else { BAND_DARK };
        format!("\x1b[0;{style};{back}m")
    } else {
        format!("\x1b[0;{style}m")
    }
}
#[derive(Clone, Copy, PartialEq)]
struct Cell { ch: char, style: &'static str, band: bool }
#[derive(Default)]
struct Canvas { width: usize, cells: Vec<Cell>, shown: Vec<Cell> }
thread_local! {
    static CANVAS: RefCell<Canvas> = RefCell::new(Canvas::default());
    static SCREEN_DEPTH: StdCell<usize> = const { StdCell::new(0) };
}
fn invalidate() { CANVAS.with(|c| c.borrow_mut().shown.clear()); }
fn begin_frame(cols: usize, rows: usize) {
    CANVAS.with(|c| {
        let mut c = c.borrow_mut();
        if c.width != cols || c.cells.len() != cols * rows { c.shown.clear(); }
        c.width = cols;
        c.cells.clear();
        c.cells.resize(cols * rows, Cell { ch: ' ', style: PLAIN, band: false });
    });
}
fn present() -> io::Result<()> {
    use std::fmt::Write as _;
    CANVAS.with(|canvas| {
        let mut c = canvas.borrow_mut();
        let mut out = String::from("\x1b[?2026h\x1b[?25l\x1b[?7l");
        let width = c.width;
        if width == 0 { return Ok(()); }
        for (y, cells) in c.cells.chunks(width).enumerate() {
            let start = y * width;
            if c.shown.get(start..start + width) == Some(cells) { continue; }
            let _ = write!(out, "\x1b[{};1H", y + 1);
            let mut style = None;
            for cell in cells {
                if style != Some((cell.style, cell.band)) { out.push_str(&ansi(cell.style, cell.band)); style = Some((cell.style, cell.band)); }
                out.push(cell.ch);
            }
        }
        out.push_str("\x1b[0m\x1b[?7h\x1b[?2026l");
        io::stdout().write_all(out.as_bytes())?;
        io::stdout().flush()?;
        c.shown = c.cells.clone();
        Ok(())
    })
}
/// Write `value` at 1-based row `y`, 0-based column `x`; returns the column after it.
fn put(y: usize, x: usize, value: &str, style: &'static str) -> usize {
    // No verbatim `\\?\` path prefix ever reaches the screen.
    let plain;
    let value = if value.contains(r"\\?\") {
        plain = crate::plain_paths(value);
        plain.as_str()
    } else {
        value
    };
    CANVAS.with(|canvas| {
        let mut c = canvas.borrow_mut();
        let width = c.width;
        let mut x = x;
        for ch in value.chars() {
            if ch.is_control() { continue; }
            if x < width {
                if let Some(cell) = c.cells.get_mut((y - 1) * width + x) { *cell = Cell { ch, style, band: cell.band }; }
            }
            x += 1;
        }
        x
    })
}
/// The selected row's band: a faint background from column `x0` to `x1`.
fn band(y: usize, x0: usize, x1: usize) {
    CANVAS.with(|canvas| {
        let mut c = canvas.borrow_mut();
        let width = c.width;
        for x in x0..x1.min(width) {
            if let Some(cell) = c.cells.get_mut((y - 1) * width + x) { cell.band = true; }
        }
    });
}
/// The Makepad mark, faint, right-aligned to `right` and centred between
/// rows `top` and `bottom` (inclusive, 1-based), or as near the middle as
/// it fits beside every row's text with two columns of air: the large one
/// (44 x 13) when it fits somewhere, else the small one (30 x 9), else none,
/// so it is never cut into. The selected
/// row's `<action> ⏎` does not count (it moves with the selection); where it
/// reaches into the mark, the text wins there and the mark goes on around it.
///
/// `sizes`: which of the two to try, in order (0 large, 1 small). With
/// `paint` false nothing is drawn; the answer says whether it would fit.
fn logo(top: usize, bottom: usize, right: usize, sizes: &[usize], paint: bool) -> bool {
    let height = (bottom + 1).saturating_sub(top);
    let style = if LIGHT.with(StdCell::get) { LOGO_LIGHT } else { LOGO_DARK };
    CANVAS.with(|canvas| {
        let mut c = canvas.borrow_mut();
        let width = c.width;
        if top == 0 || width == 0 {
            return false;
        }
        let row_cells = |c: &Canvas, y: usize| -> Vec<Cell> { c.cells.get((y - 1) * width..y * width).map(<[Cell]>::to_vec).unwrap_or_default() };
        // Where each row's own text ends (plus two columns of air).
        let clear = |cells: &[Cell]| -> usize {
            let band = cells.iter().any(|cell| cell.band);
            let mut end = cells.len();
            if band {
                // Skip the trailing action hint, drawn in OK after a gap.
                while end > 0 && (cells[end - 1].ch == ' ' || cells[end - 1].style == OK) { end -= 1; }
            } else {
                while end > 0 && cells[end - 1].ch == ' ' { end -= 1; }
            }
            if end == 0 { 0 } else { end + 2 }
        };
        let arts: Vec<Vec<&str>> = LOGO.split("\n\n").map(|art| art.lines().collect()).collect();
        for art in sizes.iter().filter_map(|&size| arts.get(size)) {
            let art_width = art.iter().map(|l| l.chars().count()).max().unwrap_or(0);
            if art.len() > height || right < art_width + 30 || right > width {
                continue;
            }
            let x0 = right - art_width;
            let middle = top + (height - art.len()) / 2;
            let mut starts: Vec<usize> = (top..=top + height - art.len()).collect();
            starts.sort_by_key(|&y| y.abs_diff(middle));
            let Some(y0) = starts.into_iter().find(|&y0| {
                art.iter().enumerate().all(|(i, line)| {
                    let ink = line.chars().position(|ch| ch != ' ').unwrap_or(art_width);
                    clear(&row_cells(&c, y0 + i)) <= x0 + ink
                })
            }) else {
                continue;
            };
            if !paint {
                return true;
            }
            for (i, line) in art.iter().enumerate() {
                let y = y0 + i;
                let before = row_cells(&c, y);
                let row = (y - 1) * width;
                for (dx, ch) in line.chars().enumerate() {
                    let x = x0 + dx;
                    let free = |x: usize| before.get(x).is_none_or(|cell| cell.ch == ' ');
                    if ch != ' ' && x < width && free(x) && free(x + 1) && (x == 0 || free(x - 1)) {
                        let band = before[x].band;
                        c.cells[row + x] = Cell { ch, style, band };
                    }
                }
            }
            return true;
        }
        false
    })
}
/// `value` in at most `max` lines of `width` columns, broken at spaces and
/// keeping each span's style; the last line ends in "…" when text is left.
fn wrap_text(value: &Text, width: usize, max: usize) -> Vec<Text> {
    let total: usize = value.iter().map(|s| crate::plain_paths(&s.0).chars().count()).sum();
    if total <= width || width < 8 || max == 0 {
        return vec![value.clone()];
    }
    // Words with their style; a word never spans two styles' boundary.
    // `glued`: no space before it (it continues the previous span's word).
    let mut words: Vec<(String, &'static str, bool)> = Vec::new();
    let mut after_space = true;
    for span in value {
        let text = crate::plain_paths(&span.0);
        for (i, part) in text.split(' ').enumerate() {
            if i > 0 {
                after_space = true;
            }
            if part.is_empty() {
                continue;
            }
            words.push((part.to_string(), span.1, !after_space));
            after_space = false;
        }
    }
    // A leading mark ("✓", "✗") keeps the text beside it: later lines
    // start under the text, not under the mark.
    let hang = match words.first() {
        Some((mark, _, _)) if mark.chars().count() <= 2 && !mark.chars().any(char::is_alphanumeric) => mark.chars().count() + 1,
        _ => 0,
    };
    let mut lines: Vec<Text> = vec![Vec::new()];
    let mut used = 0;
    for (word, style, glued) in words {
        let n = word.chars().count();
        let gap = usize::from(!glued && used > 0);
        if used > 0 && used + gap + n > width {
            if lines.len() == max {
                let last = lines.last_mut().unwrap();
                if used + 1 <= width {
                    last.push(Span("…".into(), style));
                } else if let Some(span) = last.last_mut() {
                    span.0.pop();
                    span.0.push('…');
                }
                return lines;
            }
            lines.push(vec![Span(" ".repeat(hang), PLAIN)]);
            used = hang;
            let line = lines.last_mut().unwrap();
            line.push(Span(word, style));
            used += n;
            continue;
        }
        let line = lines.last_mut().unwrap();
        let text = if used > 0 && !glued { format!(" {word}") } else { word };
        used += text.chars().count();
        match line.last_mut() {
            Some(span) if span.1 == style => span.0.push_str(&text),
            _ => line.push(Span(text, style)),
        }
    }
    lines
}
fn put_text(y: usize, x: usize, value: &Text) -> usize {
    value.iter().fold(x, |x, span| put(y, x, &span.0, span.1))
}
fn padded(value: &str, width: usize) -> String {
    let n = value.chars().count();
    format!("{value}{}", " ".repeat(width.saturating_sub(n)))
}

/// One body line: its spans, and the item index it shows (if any).
fn body_lines(view: &View, selected: Option<usize>) -> (Vec<(Text, Option<usize>)>, Option<usize>) {
    let mut lines = Vec::new();
    let mut item = 0;
    let mut selected_line = None;
    let name_width = view.name_width.max(15);
    // A screen with a license column gives every row one, so the status
    // column lines up (the main menu: status at column 30).
    let licenses = view.rows.iter().any(|r| matches!(r, Row::Item(i) if !i.license.is_empty()));
    for (i, row) in view.rows.iter().enumerate() {
        match row {
            Row::Head(title) => {
                if i > 0 || !view.back { lines.push((Vec::new(), None)); }
                lines.push((vec![Span("    ".into(), PLAIN), Span(title.clone(), DIM)], None));
            }
            Row::Note(note) => {
                let mut spans = vec![Span("    ".into(), PLAIN)];
                spans.extend(note.iter().cloned());
                lines.push((spans, None));
            }
            Row::Item(entry) => {
                if item == 0 && view.back {
                    lines.push((Vec::new(), None));
                }
                let chosen = selected == Some(item);
                // Rows of an open node sit two columns in, the name column
                // two shorter, so the columns after it stay where they are.
                let (indent, width) = if entry.child { ("  ", name_width - 2) } else { ("", name_width) };
                let mut spans = if chosen {
                    vec![Span("  ".into(), PLAIN), Span("▌".into(), MARK), Span(format!(" {indent}"), PLAIN), Span(padded(&entry.name, width), BOLD), Span(" ".into(), PLAIN)]
                } else {
                    vec![Span(format!("    {indent}{} ", padded(&entry.name, width)), PLAIN)]
                };
                let license = match entry.license {
                    "commercial" => Some(("licensed", PLAIN)),
                    "beta" => Some(("beta", WARN)),
                    "free" => Some(("free", DIM)),
                    _ if licenses => Some(("", PLAIN)),
                    _ => None,
                };
                if let Some((label, style)) = license {
                    spans.push(Span(padded(label, 10), style));
                }
                let live = live_status(&entry.id);
                let mut status = live.clone().unwrap_or_else(|| entry.status.clone());
                if let Some(first) = status.first_mut() {
                    if SPINNER.iter().any(|c| first.0 == c.to_string()) {
                        first.0 = spinner_frame().to_string();
                    }
                }
                spans.extend(status);
                // Work in progress on the row: no action; its keys are in the footer.
                let chosen_idle = chosen && live.is_none();
                if chosen_idle && entry.action == "⏎" {
                    spans.push(Span(" ⏎".into(), OK));
                } else if chosen_idle && !entry.action.is_empty() {
                    // Right after the name or the license column, else two
                    // columns after the status.
                    let gap = if entry.status.is_empty() { "" } else { "  " };
                    spans.push(Span(format!("{gap}{} ⏎", entry.action), OK));
                }
                if chosen { selected_line = Some(lines.len()); }
                lines.push((spans, Some(item)));
                item += 1;
            }
        }
    }
    (lines, selected_line)
}

/// The spinner, the step, a short bar when the share is known and the amount.
/// The row never wraps: the status starts at column 30 of a view at most
/// 80 wide, so the bar shrinks (to 4, then none) and the step is cut to fit.
fn doing_text(doing: &Doing) -> Text {
    let room = console::size().0.min(80).saturating_sub(32);
    let amount = if doing.amount.is_empty() { String::new() } else { format!(" {}", doing.amount) };
    let fixed = 2 + amount.chars().count();
    let mut step = doing.step.clone();
    let mut bar = if doing.fraction.is_some() { room.saturating_sub(fixed + step.chars().count() + 1).min(12) } else { 0 };
    if bar < 4 {
        bar = 0;
    }
    let used = fixed + step.chars().count() + if bar > 0 { bar + 1 } else { 0 };
    if used > room {
        let keep = step.chars().count().saturating_sub(used - room + 1);
        step = step.chars().take(keep).collect::<String>() + "…";
    }
    let mut spans = vec![Span(spinner_frame().to_string(), ACC), Span(format!(" {step}"), PLAIN)];
    if let (Some(fraction), true) = (doing.fraction, bar > 0) {
        let filled = ((fraction.clamp(0.0, 1.0) * bar as f64).round() as usize).min(bar);
        spans.push(Span(" ".into(), PLAIN));
        spans.push(Span("━".repeat(filled), MARK));
        spans.push(Span("─".repeat(bar - filled), DIM));
    }
    if !amount.is_empty() {
        spans.push(Span(amount, DIM));
    }
    spans
}
/// A row's status while background work involves it: compiling (or
/// downloading) with its progress, its place in the queue, or the update
/// check. None: the row's own status.
fn live_status(id: &str) -> Option<Text> {
    BACKGROUND.with(|b| {
        let b = b.borrow();
        if id == "updates" {
            return b.update.as_ref().map(doing_text);
        }
        if id == "localai" {
            return b.setup.as_ref().map(doing_text);
        }
        if let Some(building) = b.building.as_ref().filter(|x| x.id == id) {
            if building.stopping {
                return Some(vec![Span(spinner_frame().to_string(), ACC), Span(" stopping…".into(), PLAIN)]);
            }
            return Some(doing_text(&building.doing));
        }
        let place = b.queue.iter().position(|(queued, _)| queued == id)?;
        let after = if place == 0 { " · next".to_owned() } else { format!(" · {} to go", place + 1) };
        Some(vec![Span("◌".into(), ACC), Span(" queued".into(), PLAIN), Span(after, DIM)])
    })
}
/// The line under the rule when no message is showing: what the selected
/// row does, or, while an app compiles, what happens with it.
fn info_line(item: &Item) -> Text {
    BACKGROUND.with(|b| {
        let b = b.borrow();
        if let Some(building) = &b.building {
            let queued = b.queue.iter().find(|(id, _)| *id == item.id);
            if building.id == item.id {
                return if building.stopping {
                    text("Stopping the compiler; what was downloaded and compiled is kept.", PLAIN)
                } else if building.open {
                    text("Compiling from source. It opens by itself when it is done.", PLAIN)
                } else {
                    text("Compiling from source. It only compiles; press s or ⏎ to open it when done.", PLAIN)
                };
            }
            if let Some((_, open)) = queued {
                return if *open {
                    text(format!("Queued behind {}; it compiles and opens after it.", building.title), PLAIN)
                } else {
                    text(format!("Queued behind {}; it only compiles. Press s or ⏎ to open it when done.", building.title), PLAIN)
                };
            }
            if building.stopping {
                return text(format!("{} is stopping.", building.title), DIM);
            }
            return if building.open {
                text(format!("{} opens by itself when it is done compiling.", building.title), DIM)
            } else {
                text(format!("{} is compiling; it only compiles and does not open.", building.title), DIM)
            };
        }
        text(item.info.clone(), PLAIN)
    })
}
/// The main menu's keys: s and c only where the selected row has work to
/// change, their letters in orange.
fn menu_keys(item: Option<&Item>) -> Text {
    let (open, cancel) = BACKGROUND.with(|b| {
        let b = b.borrow();
        let Some(item) = item else { return (None, false) };
        if let Some(building) = b.building.as_ref().filter(|x| x.id == item.id) {
            if building.stopping { (None, false) } else { (Some(building.open), true) }
        } else if let Some((_, open)) = b.queue.iter().find(|(id, _)| *id == item.id) {
            (Some(*open), true)
        } else {
            (None, item.id == "all" && b.batch > 0)
        }
    });
    let mut spans = vec![Span("↑↓ move   ⏎ select   ".into(), DIM)];
    if let Some(open) = open {
        spans.push(Span("s".into(), KEY));
        spans.push(Span(if open { " don't start   ".into() } else { " start when done   ".into() }, DIM));
    }
    if cancel {
        spans.push(Span("c".into(), KEY));
        spans.push(Span(" cancel   ".into(), DIM));
    }
    // Not while the app compiles or waits in the queue.
    if item.is_some_and(|item| item.send) && open.is_none() {
        spans.push(Span("e".into(), KEY));
        spans.push(Span(" send my changes   ".into(), DIM));
    }
    spans.push(Span("q quit".into(), DIM));
    spans
}

fn draw() {
    if !COLOR.with(StdCell::get) { return; }
    let (cols, rows) = console::size();
    if cols < 40 || rows < 14 {
        print!("\x1b[0m\x1b[2J\x1b[HResize the terminal to at least 40 × 14.");
        let _ = io::stdout().flush();
        invalidate();
        return;
    }
    begin_frame(cols, rows);
    let width = cols.min(80);
    VIEW.with(|view| {
        let view = view.borrow();
        let selected = SELECTED.with(StdCell::get);
        let prompt = PROMPT.with(|p| p.borrow().as_ref().map(|p| (p.lines.clone(), p.active)));
        let (mut body, mut selected_line) = match &prompt {
            Some(_) => body_lines(&view, None),
            None => body_lines(&view, selected),
        };
        // A question's rows follow the content; its active row is the one
        // selected.
        if let Some((prompt_lines, active)) = prompt.clone() {
            selected_line = Some(body.len() + active);
            body.extend(prompt_lines.into_iter().map(|line| (line, None)));
        }
        // Everything above the footer scrolls as one: the header, then the
        // rows. The email shows on the Account row, not in the header.
        let _ = &view.email;
        let mut lines: Vec<(Text, Option<usize>)> = vec![
            (Vec::new(), None),
            (vec![Span("  ".into(), PLAIN), Span("Makepad".into(), BOLD), Span(" Apps".into(), PLAIN), Span(view.crumb.clone(), PLAIN)], None),
            (vec![Span("  ".into(), PLAIN), Span(view.subtitle.clone(), DIM)], None),
        ];
        let head = lines.len();
        lines.extend(body);
        let selected_line = selected_line.map(|line| line + head);
        // The footer (the rule, two status lines, the keys) takes the last
        // four lines at most; one line above it stays empty or says what is
        // scrolled out of view.
        let room = rows - 5;
        let mut offset = SCROLL.with(StdCell::get);
        if lines.len() <= room {
            offset = 0;
        } else {
            if let Some(line) = selected_line {
                // The first row brings the header back into view.
                let first_item = lines.iter().position(|l| l.1 == Some(0)).unwrap_or(0);
                if line <= first_item { offset = 0; }
                if line < offset { offset = line; }
                if line >= offset + room { offset = line + 1 - room; }
            }
            offset = offset.min(lines.len() - room);
        }
        SCROLL.with(|s| s.set(offset));
        let shown = lines.len().min(room);
        for (index, (spans, _)) in lines.iter().enumerate().skip(offset).take(room) {
            let y = index - offset + 1;
            if selected_line == Some(index) {
                band(y, 2, width.saturating_sub(2));
            }
            put_text(y, 0, spans);
        }
        let mut y = shown + 2;
        if lines.len() > room {
            let below = lines[offset + room..].iter().filter(|l| l.1.is_some()).count();
            let above = lines[..offset].iter().filter(|l| l.1.is_some()).count();
            let more = match (above, below) {
                (0, 0) => String::new(),
                (0, below) => format!("↓ {below} more"),
                (above, 0) => format!("↑ {above} above"),
                (above, below) => format!("↑ {above} above · ↓ {below} more"),
            };
            put(y - 1, 4, &more, DIM);
        }
        // The mark sits at the right of the window (up to 100 columns)
        // beside the rows: the large one or the small one where either
        // fits, else the rule moves down just far enough for the small one
        // (a short page like Log in gets it under its text), else none.
        let right = cols.min(100).saturating_sub(2);
        let top = if offset == 0 { head + 1 } else { 1 };
        if logo(top, y - 2, right, &[0, 1], false) {
            logo(top, y - 2, right, &[0, 1], true);
        } else if let Some(rule) = (y + 1..=rows - 3).find(|&rule| logo(top, rule - 2, right, &[1], false)) {
            y = rule;
            logo(top, y - 2, right, &[1], true);
        }
        put(y, 2, &"─".repeat(width.saturating_sub(4)), DIM);
        // A status message longer than the line wraps onto the choice line
        // when that is free; what still does not fit ends in "…". With no
        // message, the main menu says what the selected row does.
        let room = width.saturating_sub(4);
        let choice = CHOICE.with(|c| c.borrow().clone());
        let mut message = MESSAGE.with(|m| m.borrow().clone());
        let item = selected.and_then(|n| view.rows.iter().filter_map(|r| if let Row::Item(i) = r { Some(i) } else { None }).nth(n));
        let main = !view.back && prompt.is_none();
        if message.is_empty() && choice.is_empty() && main {
            if let Some(item) = item {
                message = info_line(item);
            }
        }
        let lines = wrap_text(&message, room, if choice.is_empty() { 2 } else { 1 });
        for (i, line) in lines.iter().enumerate() {
            put_text(y + 1 + i, 2, line);
        }
        put_text(y + 2, 2, &choice);
        match FOOTER.with(StdCell::get).or(view.footer) {
            Some(footer) => {
                put(y + 3, 2, footer, DIM);
            }
            None if view.back => {
                put(y + 3, 2, "↑↓ move   ⏎ select   esc back   q quit", DIM);
            }
            None => {
                put_text(y + 3, 2, &menu_keys(item));
            }
        }
    });
    let _ = present();
}

/// Run one screen until a row is chosen or the person leaves it.
/// `selected` is the item index, kept by the caller across calls.
pub(super) fn menu(view: View, selected: &mut usize, changed: &dyn Fn() -> bool) -> Result<Nav, String> {
    let ids: Vec<String> = view.rows.iter().filter_map(|r| if let Row::Item(i) = r { Some(i.id.clone()) } else { None }).collect();
    if !COLOR.with(StdCell::get) {
        println!();
        for row in &view.rows {
            match row {
                Row::Head(title) => println!("\n{title}"),
                Row::Note(note) => println!("  {}", plain_text(note)),
                Row::Item(item) => {
                    let number = ids.iter().position(|id| *id == item.id).unwrap_or(0) + 1;
                    println!("{number:>3}. {}  {}  [{}]", item.name, plain_text(&item.status), item.action);
                }
            }
        }
        let Ok(answer) = line(if view.back { "Choose a number, Enter to go back, q to quit: " } else { "Choose a number, or q to quit: " }) else {
            return Ok(Nav::Quit);
        };
        return Ok(match answer.to_lowercase().as_str() {
            "q" => Nav::Quit,
            "" if view.back => Nav::Back,
            choice => match choice.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|n| ids.get(n)) {
                Some(id) => { *selected = ids.iter().position(|i| i == id).unwrap_or(0); Nav::Select(id.clone()) }
                None => Nav::Refresh,
            },
        });
    }
    if ids.is_empty() {
        return Ok(Nav::Back);
    }
    let main = !view.back;
    VIEW.with(|v| *v.borrow_mut() = view);
    FOOTER.with(|f| f.set(None));
    // Only the screen that has the worked-on row takes it over.
    let worked = WORKED_ON.with(|w| w.borrow().as_ref().and_then(|id| ids.iter().position(|i| i == id)));
    if let Some(index) = worked {
        WORKED_ON.with(|w| w.borrow_mut().take());
        *selected = index;
    }
    let mut sel = (*selected).min(ids.len() - 1);
    let _input = console::Input::enter()?;
    loop {
        SELECTED.with(|s| s.set(Some(sel)));
        *selected = sel;
        draw();
        let key = console::key()?;
        if !matches!(key, Key::Other) {
            MESSAGE.with(|m| m.borrow_mut().clear());
        }
        match key {
            // The list wraps around: Up on the first row goes to the last.
            Key::Up | Key::Char('k') => sel = (sel + ids.len() - 1) % ids.len(),
            Key::Down | Key::Char('j') => sel = (sel + 1) % ids.len(),
            Key::PageUp => sel = sel.saturating_sub(10),
            Key::PageDown => sel = (sel + 10).min(ids.len() - 1),
            Key::Home => sel = 0,
            Key::End => sel = ids.len() - 1,
            #[cfg(not(windows))]
            Key::WheelUp => sel = sel.saturating_sub(3),
            #[cfg(not(windows))]
            Key::WheelDown => sel = (sel + 3).min(ids.len() - 1),
            Key::Enter => return Ok(Nav::Select(ids[sel].clone())),
            Key::Back => return Ok(Nav::Back),
            Key::Quit | Key::Char('q') | Key::Char('Q') => return Ok(Nav::Quit),
            // Background work: s (start when done) and c (cancel); e sends
            // the changes made to an app.
            Key::Char(c @ ('c' | 's' | 'e')) if main => return Ok(Nav::Key(c, ids[sel].clone())),
            Key::Other => {
                // Background builds report on every tick, each tick redraws.
                let exited = super::reap_apps();
                let finished = super::poll_background();
                if exited || finished || changed() {
                    return Ok(Nav::Refresh);
                }
            }
            _ => {}
        }
    }
}

/// A question with its options as rows in the content (see `Prompt`),
/// the default one selected: ↑↓ or a first letter picks, ⏎ accepts, Escape
/// backs out (None). Closed input and Ctrl-C also return None.
pub(super) fn choose(question: &str, note: &str, options: &[&str], default: usize) -> Result<Option<String>, String> {
    if options.is_empty() {
        return Ok(None);
    }
    if !COLOR.with(StdCell::get) {
        println!("\n{}", clean(question));
        if !note.is_empty() { println!("({note})"); }
        loop {
            let Ok(answer) = line(&format!("[{}] ", options.join("/"))) else { return Ok(None) };
            if answer.contains('\u{1b}') { return Ok(None); }
            let answer = answer.to_lowercase();
            if answer.is_empty() { return Ok(Some(options[default.min(options.len() - 1)].to_owned())); }
            if let Some(option) = options.iter().find(|o| o.to_lowercase().starts_with(&answer)) { return Ok(Some((*option).to_owned())); }
            println!("Please answer {}.", options.join(" or "));
        }
    }
    let mut pick = default.min(options.len() - 1);
    let _input = console::Input::enter()?;
    MESSAGE.with(|m| m.borrow_mut().clear());
    let result = loop {
        let mut lines = prompt_head(question, note);
        let first = lines.len();
        for (i, option) in options.iter().enumerate() {
            lines.push(if i == pick {
                vec![Span("  ".into(), PLAIN), Span("▌".into(), MARK), Span(" ".into(), PLAIN), Span((*option).into(), BOLD), Span(" ⏎".into(), OK)]
            } else {
                vec![Span(format!("    {option}"), PLAIN)]
            });
        }
        set_prompt(Some(Prompt { lines, active: first + pick }));
        FOOTER.with(|f| f.set(Some("↑↓ choose   ⏎ accept   esc back")));
        draw();
        match console::key()? {
            Key::Left | Key::Up => pick = pick.saturating_sub(1),
            Key::Right | Key::Down => pick = (pick + 1).min(options.len() - 1),
            Key::Enter => break Some(options[pick].to_owned()),
            Key::Back | Key::Quit => break None,
            Key::Char(c) => {
                let c = c.to_ascii_lowercase();
                if let Some(option) = options.iter().find(|o| o.to_lowercase().starts_with(c)) {
                    break Some((*option).to_owned());
                }
            }
            Key::Other => { super::reap_apps(); super::poll_background(); }
            _ => {}
        }
    };
    set_prompt(None);
    MESSAGE.with(|m| m.borrow_mut().clear());
    FOOTER.with(|f| f.set(None));
    Ok(result)
}

/// A one-line editor as a row in the content (see `Prompt`), under its
/// question; `hint` is the line under it. Escape or closed input: None.
/// `initial` pre-fills the text with the cursor at its end (a rejected email
/// to correct).
pub(super) fn edit(prompt: &str, initial: &str, hint: Text, footer: &'static str) -> Result<Option<String>, String> {
    if !COLOR.with(StdCell::get) {
        if !hint.is_empty() { println!("{}", plain_text(&hint)); }
        return Ok(line(&format!("{prompt} ")).ok().map(|v| if v.is_empty() { initial.to_owned() } else { v }));
    }
    let mut value = initial.to_owned();
    let _input = console::Input::enter()?;
    let result = loop {
        let mut lines = prompt_head(prompt, "");
        let active = lines.len();
        lines.push(vec![Span("  ".into(), PLAIN), Span("▌".into(), MARK), Span(" ".into(), PLAIN), Span(value.clone(), BOLD), Span(" ".into(), INV)]);
        if !hint.is_empty() {
            let mut spans = vec![Span("    ".into(), PLAIN)];
            spans.extend(hint.iter().cloned());
            lines.push(spans);
        }
        set_prompt(Some(Prompt { lines, active }));
        MESSAGE.with(|m| m.borrow_mut().clear());
        FOOTER.with(|f| f.set(Some(footer)));
        draw();
        match console::key()? {
            Key::Enter => break Some(value.trim().to_owned()),
            Key::Back | Key::Quit => break None,
            Key::Backspace => { value.pop(); }
            Key::Char(c) if !c.is_control() && value.chars().count() < 200 => value.push(c),
            Key::Other => { super::poll_background(); }
            _ => {}
        }
    };
    set_prompt(None);
    MESSAGE.with(|m| m.borrow_mut().clear());
    FOOTER.with(|f| f.set(None));
    Ok(result)
}

/// The next menu keeps its own selection instead of the last work's row.
pub(super) fn forget_worked_on() {
    WORKED_ON.with(|w| w.borrow_mut().take());
}

pub(super) struct Screen {
    color: bool,
}
impl Screen {
    pub(super) fn enter() -> Self {
        let color = io::stdout().is_terminal()
            && io::stdin().is_terminal()
            && env::var_os("NO_COLOR").is_none();
        if color {
            console::enable();
            SCREEN_DEPTH.with(|depth| {
                if depth.get() == 0 { print!("\x1b[?1049h\x1b[?25l"); invalidate(); COLOR.with(|c| c.set(true)); }
                depth.set(depth.get() + 1);
            });
        }
        Self { color }
    }
    pub(super) fn pause() -> Pause {
        let active = SCREEN_DEPTH.with(|d| d.get() > 0);
        if active {
            print!("\x1b[0m\x1b[?25h\x1b[?1049l");
            let _ = io::stdout().flush();
            COLOR.with(|c| c.set(false));
        }
        Pause(active)
    }
}
pub(super) struct Pause(bool);
impl Drop for Pause {
    fn drop(&mut self) {
        if self.0 {
            print!("\x1b[?1049h\x1b[?25l");
            invalidate();
            COLOR.with(|c| c.set(true));
            let _ = io::stdout().flush();
        }
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        if self.color {
            SCREEN_DEPTH.with(|depth| {
                depth.set(depth.get().saturating_sub(1));
                if depth.get() == 0 {
                    print!("\x1b[0m\x1b[?25h\x1b[?1049l");
                    invalidate();
                    COLOR.with(|c| c.set(false));
                }
            });
            let _ = io::stdout().flush();
        }
    }
}

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// The spinner's frame for now: one step every 100 ms.
fn spinner_frame() -> char {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    SPINNER[(START.get_or_init(Instant::now).elapsed().as_millis() / 100) as usize % SPINNER.len()]
}

/// A component's short name on the row: Cuda, Tools, SDK, Rust.
pub(super) fn short_name(label: &str) -> String {
    match label {
        "CUDA" => "Cuda".into(),
        "Build tools" | "Build Tools" => "Tools".into(),
        "Windows SDK" => "SDK".into(),
        other => other.into(),
    }
}
/// A component installing beside others (Build tools, Windows SDK, Rust,
/// CUDA), as the app's row counts it.
pub(super) struct Part {
    pub label: String,
    pub done: bool,
    pub running: bool,
    pub fraction: Option<f64>,
    /// Its payload size; 0 until its manifest is read.
    pub bytes: u64,
}
/// The row's one bar over components installing side by side: the work
/// done over the work expected, each weighted by its payload bytes. It never
/// goes back (a component that starts or learns its size adds to the
/// whole; `shown` is what the row showed) and reaches 100% only when every
/// one is done. The step names what still runs, short and in one order:
/// "Cuda, Tools, SDK, Rust".
pub(super) fn combined_install(parts: &[Part], shown: f64) -> (String, f64) {
    let total: f64 = parts.iter().map(|p| p.bytes.max(1) as f64).sum();
    let done: f64 = parts.iter().map(|p| p.bytes.max(1) as f64 * if p.done { 1.0 } else { p.fraction.unwrap_or(0.0).clamp(0.0, 1.0) }).sum();
    let all_done = !parts.is_empty() && parts.iter().all(|p| p.done);
    let mut fraction = if total > 0.0 { done / total } else { 0.0 };
    if !all_done {
        fraction = fraction.min(0.99);
    }
    let fraction = fraction.max(shown.min(if all_done { 1.0 } else { 0.99 }));
    // Short names in one order: Cuda, Tools, SDK, Rust; a finished one drops.
    let order = ["Cuda", "Tools", "SDK", "Rust"];
    let mut running: Vec<String> = parts.iter().filter(|p| p.running).map(|p| short_name(&p.label)).collect();
    running.sort_by_key(|name| order.iter().position(|o| o == name).unwrap_or(order.len()));
    let step = if running.is_empty() { "installing".to_owned() } else { running.join(", ") };
    (step, fraction)
}

#[cfg(test)]
mod combined_install_tests {
    use super::{combined_install, Part};
    fn part(label: &str, done: bool, fraction: Option<f64>, bytes: u64) -> Part {
        Part { label: label.into(), done, running: !done, fraction, bytes }
    }
    #[test]
    fn weighted_by_bytes_forward_only_and_full_only_when_all_done() {
        // Build tools 1000 MB at 50 %, CUDA 3000 MB at 10 %: (500 + 300) / 4000.
        let (step, f) = combined_install(&[part("Build tools", false, Some(0.5), 1000), part("CUDA", false, Some(0.1), 3000)], 0.0);
        assert_eq!(step, "Cuda, Tools");
        assert!((f - 0.2).abs() < 1e-9);
        // Weighted, not averaged: Rust (300 MB) done and CUDA at 10 % is
        // (300 + 300) / 3300 = 18 %, not 55 %; below what was shown, the bar holds.
        let (_, g) = combined_install(&[part("Rust", true, Some(1.0), 300), part("CUDA", false, Some(0.1), 3000)], 0.0);
        assert!((g - 600.0 / 3300.0).abs() < 1e-9);
        let (_, g) = combined_install(&[part("Rust", true, Some(1.0), 300), part("CUDA", false, Some(0.1), 3000)], f);
        assert_eq!(g, f);
        // A new component with a large size lowers the share: the bar holds.
        let (step, h) = combined_install(
            &[part("Build tools", false, Some(0.5), 1000), part("Windows SDK", false, Some(0.0), 2000), part("Rust", false, None, 0), part("CUDA", false, Some(0.1), 3000)],
            0.3,
        );
        assert_eq!(step, "Cuda, Tools, SDK, Rust");
        assert!((h - 0.3).abs() < 1e-9);
        // Nearly done is not done; all done is 100 %.
        let (_, i) = combined_install(&[part("Rust", true, Some(1.0), 300), part("CUDA", false, Some(1.0), 3000)], 0.0);
        assert!(i < 1.0);
        let (step, j) = combined_install(&[part("Rust", true, Some(1.0), 300), part("CUDA", true, Some(1.0), 3000)], 0.99);
        assert_eq!((step.as_str(), j), ("installing", 1.0));
    }
}

/// Background work's progress events, read on the terminal's thread: the
/// log gets the phases and Cargo's lines (as `with_progress` logs them), the
/// row gets what it is doing now.
pub(super) struct Follow {
    identity: String,
    logged_line: String,
    /// Components installing side by side (Rust, the build tools, CUDA).
    rows: progress::Rows,
    /// The row shows their combined bar (it only moves forward).
    installing: bool,
    pub doing: Doing,
}
impl Follow {
    pub(super) fn new(starting: &str) -> Self {
        Follow { identity: String::new(), logged_line: String::new(), rows: progress::Rows::default(), installing: false, doing: Doing { step: starting.into(), ..Doing::default() } }
    }
    pub(super) fn event(&mut self, p: &progress::Progress) {
        if !p.row.is_empty() {
            if p.stage == "Working" {
                activity(&format!("{}: {}", p.row, p.detail.trim()));
            }
            if self.rows.update(p) {
                if let Some(row) = self.rows.rows.iter().find(|r| r.label == p.row && r.state != progress::RowState::Running) {
                    activity(&format!("{}: {}", row.label, progress::Rows::end_line(row)));
                }
            }
            let parts: Vec<Part> = self.rows.rows.iter().map(|r| Part {
                label: r.label.clone(),
                done: r.state == progress::RowState::Done,
                running: r.state == progress::RowState::Running,
                fraction: r.fraction,
                bytes: r.bytes,
            }).collect();
            let shown = if self.installing { self.doing.fraction.unwrap_or(0.0) } else { 0.0 };
            self.installing = true;
            let (step, fraction) = combined_install(&parts, shown);
            self.doing = Doing { step, fraction: Some(fraction), amount: format!("{:.0}%", fraction * 100.0) };
            return;
        }
        let phase = format!("{}:{}:{}", p.package.group, p.package.index, p.stage);
        if self.identity != phase {
            self.identity = phase;
            let group = if p.package.group.is_empty() { String::new() } else { format!("{} {}: ", p.package.group, p.package.name) };
            activity(&format!("{group}{}: {}", p.stage, p.detail));
        } else if p.stage == "Compiling Rust" && !p.detail.trim().is_empty() && p.detail != self.logged_line {
            activity(&p.detail);
        }
        if p.stage == "Compiling Rust" {
            self.logged_line = p.detail.clone();
        }
        let mb = |bytes: f64| bytes / 1048576.;
        match (p.stage.as_str(), &p.overall) {
            ("Working", _) => {}
            (stage, Some(overall)) => {
                // A component with its one forward-only bar (the source).
                let step = if stage == "Download" { "downloading" } else { "unpacking" };
                self.doing = Doing {
                    step: step.into(),
                    fraction: Some(overall.fraction),
                    amount: format!("{:.0} / {:.0} MB", mb(overall.fraction * overall.bytes as f64), mb(overall.bytes as f64)),
                };
            }
            ("Compiling Rust", None) => {
                let (fraction, amount) = match p.unit {
                    progress::Unit::Crates if p.total > 0 => (Some((p.loaded as f64 / p.total as f64).clamp(0.0, 1.0)), format!("{} / {} crates", p.loaded, p.total)),
                    progress::Unit::Crates if p.loaded > 0 => (None, format!("{} crates", p.loaded)),
                    _ => (self.doing.fraction.filter(|_| self.doing.step == "compiling"), if self.doing.step == "compiling" { self.doing.amount.clone() } else { String::new() }),
                };
                self.doing = Doing { step: "compiling".into(), fraction, amount };
            }
            ("Finishing", None) => self.doing = Doing { step: "finishing".into(), ..Doing::default() },
            _ => {}
        }
    }
}

/// Run blocking setup work with its progress as one self-rewriting bar on the
/// status line: a short label, the bar, a percentage and a dim amount. Every
/// phase change is written to the log file; per-file events never are.
pub fn with_progress<R>(work: impl FnOnce() -> R) -> R {
    let _screen = Screen::enter();
    let color = COLOR.with(StdCell::get);
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut identity = String::new();
    // Cargo's last line comes again with every crate counted; log it once.
    let mut logged_line = String::new();
    let mut started = Instant::now();
    let mut baseline = 0;
    let begun = Instant::now();
    // Components installing side by side: one row each on the work page,
    // redrawn at most every 66 ms (and at once when one starts or ends).
    let mut rows = progress::Rows::default();
    let mut printer = (!color).then(progress::printer);
    let mut logged: Vec<(String, String)> = Vec::new();
    let mut drawn = Instant::now() - Duration::from_secs(1);
    let result = progress::scope(
        move |p| {
            if !p.row.is_empty() {
                // The log gets each component's notes and its one-line end
                // (the timing lines follow the install); stages flip with
                // every payload on many threads and are not logged.
                if p.stage == "Working" && !logged.iter().any(|(row, note)| *row == p.row && *note == p.detail) {
                    activity(&format!("{}: {}", p.row, p.detail.trim()));
                    logged.push((p.row.clone(), p.detail.clone()));
                }
                let changed = rows.update(&p);
                if let Some(row) = rows.rows.iter().find(|r| r.label == p.row && r.state != progress::RowState::Running) {
                    if changed {
                        activity(&format!("{}: {}", row.label, progress::Rows::end_line(row)));
                    }
                }
                if let Some(print) = printer.as_mut() {
                    print(p);
                    return;
                }
                if !changed && drawn.elapsed() < Duration::from_millis(66) {
                    return;
                }
                drawn = Instant::now();
                let width = console::size().0.min(80);
                for row in &rows.rows {
                    let line = row_line(row, &begun, width);
                    if row.state == progress::RowState::Running {
                        let mut spans = vec![Span(format!("{} ", padded(&row.label, 13)), PLAIN)];
                        spans.extend(line);
                        MESSAGE.with(|m| *m.borrow_mut() = spans);
                        CHOICE.with(|c| c.borrow_mut().clear());
                    }
                }
                FOOTER.with(|f| f.set(Some("working · ctrl+c stops")));
                draw();
                return;
            }
            let phase = format!("{}:{}:{}", p.package.group, p.package.index, p.stage);
            let changed = identity != phase;
            if changed {
                identity = phase;
                started = Instant::now();
                baseline = p.loaded;
                let group = if p.package.group.is_empty() { String::new() } else { format!("{} {}: ", p.package.group, p.package.name) };
                activity(&format!("{group}{}: {}", p.stage, p.detail));
            } else if p.stage == "Compiling Rust" && !p.detail.trim().is_empty() && p.detail != logged_line {
                activity(&p.detail);
            }
            if p.stage == "Compiling Rust" {
                logged_line = p.detail.clone();
            }
            if !changed && last.elapsed() < Duration::from_millis(100) && !(p.total > 0 && p.loaded >= p.total) {
                return;
            }
            last = Instant::now();
            let label = if p.package.group.is_empty() { p.stage.clone() } else { p.package.group.clone() };
            let fraction = if p.total > 0 {
                Some((p.loaded as f64 / p.total as f64).clamp(0.0, 1.0))
            } else if p.stage == "Ready" {
                Some(1.0)
            } else if p.package.count > 0 {
                Some(p.package.index.saturating_sub(1) as f64 / p.package.count as f64)
            } else {
                None
            };
            let mut amount = match p.unit {
                progress::Unit::Bytes if p.total > 0 => format!("{:.1} / {:.1} MB", p.loaded as f64 / 1048576., p.total as f64 / 1048576.),
                progress::Unit::Bytes if p.loaded > 0 => format!("{:.1} MB", p.loaded as f64 / 1048576.),
                progress::Unit::Files | progress::Unit::Blocks | progress::Unit::Objects if p.loaded > 0 => format!(
                    "{}{} {}",
                    p.loaded,
                    if p.total > 0 { format!(" / {}", p.total) } else { String::new() },
                    match p.unit { progress::Unit::Files => "files", progress::Unit::Objects => "objects", _ => "blocks" }
                ),
                progress::Unit::Crates if p.total > 0 => format!("{} / {} crates", p.loaded, p.total),
                progress::Unit::Crates => format!("{} crates compiled", p.loaded),
                _ => String::new(),
            };
            if p.stage == "Download" && p.loaded > baseline {
                let elapsed = started.elapsed().as_secs_f64();
                if elapsed > 0.25 {
                    amount.push_str(&format!(" · {:.1} MB/s", (p.loaded - baseline) as f64 / elapsed / 1048576.));
                }
            }
            // One short word or two about what is happening, never a path.
            let what = if p.package.count > 0 {
                format!("{} · {} of {}", p.stage.to_lowercase(), p.package.index.max(1), p.package.count)
            } else if p.stage == "Compiling Rust" {
                // The count says it all; crate names flickering past do not.
                String::new()
            } else if matches!(p.unit, progress::Unit::None) {
                p.detail.clone()
            } else {
                p.stage.to_lowercase()
            };
            let what = [amount, clean(&what)].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
            // A component with a known total shows one forward-only bar for all of
            // it, the MB done of its payload and what it is doing now.
            let (label, fraction, what) = match &p.overall {
                Some(overall) => {
                    let mb = |bytes: f64| bytes / 1048576.;
                    let doing = match p.stage.as_str() {
                        "Download" => "downloading",
                        "Verify cache" => "checking downloads",
                        _ => "unpacking",
                    };
                    let amount = format!("{:.0} / {:.0} MB", mb(overall.fraction * overall.bytes as f64), mb(overall.bytes as f64));
                    (overall.label.clone(), Some(overall.fraction), format!("{amount} · {doing}"))
                }
                None => (label, fraction, what),
            };
            if !color {
                if changed { println!("{label}: {what}"); }
                return;
            }
            let label: String = label.chars().take(22).collect();
            let mut spans = Vec::new();
            spans.push(Span(format!("{} ", padded(&label, 13)), PLAIN));
            let width = 30;
            match fraction {
                Some(f) => {
                    let filled = (f * width as f64).round() as usize;
                    spans.push(Span("━".repeat(filled), MARK));
                    spans.push(Span("─".repeat(width - filled), DIM));
                    spans.push(Span(format!(" {:3.0}%", f * 100.), PLAIN));
                }
                None => {
                    let frame = (begun.elapsed().as_millis() / 100) as usize % SPINNER.len();
                    spans.insert(0, Span(format!("{} ", SPINNER[frame]), ACC));
                }
            }
            if !what.is_empty() {
                spans.push(Span(format!("  {what}"), DIM));
            }
            MESSAGE.with(|m| *m.borrow_mut() = spans);
            CHOICE.with(|c| c.borrow_mut().clear());
            FOOTER.with(|f| f.set(Some("working · ctrl+c stops")));
            draw();
        },
        work,
    );
    FOOTER.with(|f| f.set(None));
    MESSAGE.with(|m| m.borrow_mut().clear());
    CHOICE.with(|c| c.borrow_mut().clear());
    result
}

/// A side-by-side component's line after its name: its bar, percentage
/// and figures, cut to the width.
fn row_line(row: &progress::Row, begun: &Instant, width: usize) -> Text {
    const BAR: usize = 16;
    let mut spans = Vec::new();
    match (&row.state, row.fraction) {
        (progress::RowState::Running, Some(f)) => {
            let filled = (f * BAR as f64).round() as usize;
            spans.push(Span("━".repeat(filled), MARK));
            spans.push(Span("─".repeat(BAR - filled), DIM));
            spans.push(Span(format!(" {:3.0}%", f * 100.), PLAIN));
        }
        (progress::RowState::Running, None) => {
            let frame = (begun.elapsed().as_millis() / 100) as usize % SPINNER.len();
            spans.push(Span(SPINNER[frame].to_string(), ACC));
        }
        _ => {}
    }
    // Rows are indented 4, then the mark and the 16-wide name.
    let room = width.saturating_sub(4 + 2 + 16 + BAR + 5 + 2 + 1);
    // Whole figures only: drop the last ones that do not fit.
    let mut detail = String::new();
    for part in progress::Rows::detail(row).split(" · ") {
        let next = if detail.is_empty() { part.to_string() } else { format!("{detail} · {part}") };
        if next.chars().count() > room {
            if detail.is_empty() {
                detail = part.chars().take(room).collect();
            }
            break;
        }
        detail = next;
    }
    if !detail.is_empty() {
        spans.push(Span(format!("  {detail}"), DIM));
    }
    spans
}
