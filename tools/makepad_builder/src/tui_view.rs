//! Character-cell dialogs shared by setup, licensing and compiler output.
use super::{clean, console, line, Key};
use crate::progress;
use std::{
    cell::RefCell,
    collections::VecDeque,
    env,
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};
thread_local! { static LOG: RefCell<VecDeque<String>> = const { RefCell::new(VecDeque::new()) }; }
pub(super) fn activity(text: &str) {
    LOG.with(|log| {
        let mut log = log.borrow_mut();
        for line in text.lines().map(clean).filter(|s| !s.is_empty()) {
            if log.back() != Some(&line) {
                log.push_back(line);
            }
        }
        while log.len() > 160 {
            log.pop_front();
        }
    });
}
fn ansi(style: &str) -> String {
    let mut codes = vec!["0".to_owned()];
    for code in style.split(';') {
        codes.push(
            match code {
                "30" => "38;2;0;0;0",
                "32" => "38;2;0;100;0",
                "37" => "38;2;192;192;192",
                "90" => "38;2;112;112;112",
                "93" => "38;2;255;255;85",
                "97" => "38;2;255;255;255",
                "40" => "48;2;0;0;0",
                "shadow" => "48;2;0;0;92",
                "44" => "48;2;0;0;128",
                "47" => "48;2;192;192;192",
                _ => code,
            }
            .to_owned(),
        );
    }
    format!("\x1b[{}m", codes.join(";"))
}
fn pad(text: &str, width: usize) -> String {
    let mut chars: Vec<_> = clean(text).chars().collect();
    if chars.len() > width {
        chars.truncate(width.saturating_sub(1));
        chars.push('…');
    }
    let length = chars.len();
    chars.into_iter().collect::<String>() + &" ".repeat(width.saturating_sub(length))
}
#[derive(Clone, Copy, PartialEq)]
struct Cell { ch: char, style: &'static str }
#[derive(Default)]
struct Canvas { width: usize, cells: Vec<Cell>, shown: Vec<Cell> }
thread_local! {
    static CANVAS: RefCell<Canvas> = RefCell::new(Canvas::default());
    static SCREEN_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
fn invalidate() { CANVAS.with(|c| c.borrow_mut().shown.clear()); }
fn begin_frame(cols: usize, rows: usize) {
    CANVAS.with(|c| {
        let mut c=c.borrow_mut();
        if c.width != cols || c.cells.len() != cols*rows { c.shown.clear(); }
        c.width=cols;
        c.cells.clear();
        c.cells.resize(cols*rows,Cell{ch:' ',style:"37;44"});
    });
}
fn present() -> io::Result<()> {
    use std::fmt::Write as _;
    CANVAS.with(|canvas| {
        let mut c=canvas.borrow_mut();
        let mut out=String::from("\x1b[?2026h\x1b[?25l\x1b[?7l");
        let width=c.width;
        for (y,cells) in c.cells.chunks(width).enumerate() {
            let start=y*width;
            if c.shown.get(start..start+width)==Some(cells) {continue;}
            let _=write!(out,"\x1b[{};1H",y+1);
            let mut style="";
            for cell in cells {
                if style!=cell.style {out.push_str(&ansi(cell.style));style=cell.style;}
                out.push(cell.ch);
            }
        }
        out.push_str("\x1b[?7h\x1b[?2026l");
        io::stdout().write_all(out.as_bytes())?;
        io::stdout().flush()?;
        c.shown=c.cells.clone();
        Ok(())
    })
}
fn row(y: usize, x: usize, text: &str, style: &'static str, width: usize) {
    CANVAS.with(|canvas| {
        let mut c=canvas.borrow_mut();
        if y==0 || x==0 || c.width==0 {return;}
        let start=(y-1)*c.width+x-1;
        let available=width.min(c.width.saturating_sub(x-1));
        for (i,ch) in pad(text,width).chars().take(available).enumerate() {
            if let Some(cell)=c.cells.get_mut(start+i) {*cell=Cell{ch,style};}
        }
    });
}
fn panel(x: usize, y: usize, width: usize, height: usize, blue: bool, shadow: bool) {
    let style = if blue { "37;44" } else { "30;47" };
    if shadow {
        for offset in 1..=height {
            row(y + offset, x + 1, "", "shadow", width);
        }
    }
    row(y, x, &format!("┌{}┐", "─".repeat(width - 2)), style, width);
    for offset in 1..height - 1 {
        row(
            y + offset,
            x,
            &format!("│{}│", " ".repeat(width - 2)),
            style,
            width,
        );
    }
    row(
        y + height - 1,
        x,
        &format!("└{}┘", "─".repeat(width - 2)),
        style,
        width,
    );
}
fn wrapped(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in text.lines() {
        let mut current = String::new();
        for word in clean(line).split_whitespace() {
            if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
                out.push(std::mem::take(&mut current));
            }
            if !current.is_empty() { current.push(' '); }
            for ch in word.chars() {
                if current.chars().count() == width { out.push(std::mem::take(&mut current)); }
                current.push(ch);
            }
        }
        out.push(current);
    }
    out
}
fn backdrop(title: &str, footer: &str, dialog_height: usize) -> (usize, usize, usize) {
    let (cols, rows) = console::size();
    begin_frame(cols, rows);
    let width = cols.saturating_sub(8).clamp(24, 86);
    let x = cols.saturating_sub(width) / 2 + 1;
    let title_width = (title.chars().count() + 6).min(width);
    let title_x = cols.saturating_sub(title_width) / 2 + 1;
    panel(title_x, 1, title_width, 3, false, true);
    row(2, title_x + 2, title, "1;30;47", title_width - 4);
    panel(x, 5, width, dialog_height, false, true);
    let log_y = 5 + dialog_height + 1;
    if rows > log_y + 4 {
        let log_height = rows - log_y - 1;
        panel(3, log_y, cols.saturating_sub(6).max(4), log_height, true, true);
        row(log_y, 5, " Activity ", "1;97;44", 10);
        let count = log_height.saturating_sub(2);
        LOG.with(|log| {
            let log = log.borrow();
            for (i, text) in log.iter().skip(log.len().saturating_sub(count)).enumerate() {
                row(log_y + 1 + i, 5, text, "37;44", cols.saturating_sub(10));
            }
        });
    }
    row(rows, 1, footer, "30;47", cols);
    (x, 5, width)
}
fn too_small() -> bool {
    let (cols, rows) = console::size();
    if cols >= 48 && rows >= 24 {
        return false;
    }
    print!(
        "{}\x1b[2J\x1b[HResize the terminal to at least 48 × 24.\r\nEsc closes setup.",
        ansi("37;44")
    );
    let _ = io::stdout().flush();
    true
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
                if depth.get()==0 {print!("\x1b[?1049h\x1b[?25l");invalidate();}
                depth.set(depth.get()+1);
            });
        }
        Self { color }
    }
    pub(super) fn pause() -> Pause {
        let active=SCREEN_DEPTH.with(|d|d.get()>0);
        if active {print!("\x1b[0m\x1b[?25h\x1b[?1049l");let _=io::stdout().flush();}
        Pause(active)
    }
    pub(super) fn choose(
        &self,
        title: &str,
        info: &[String],
        options: &[(String, String, String)],
    ) -> Result<String, String> {
        self.choose_state(title, info, options, None, &[])
    }
    pub(super) fn choose_state(
        &self,
        title: &str,
        info: &[String],
        options: &[(String, String, String)],
        ready: Option<[bool; 2]>,
        disabled: &[&str],
    ) -> Result<String, String> {
        let selected = ready.map_or(0, |r| r.iter().position(|v| !v).unwrap_or(r.len() - 1));
        self.choose_state_from(title, info, options, ready, disabled, selected)
    }
    pub(super) fn choose_from(
        &self,
        title: &str,
        info: &[String],
        options: &[(String, String, String)],
        selected: usize,
    ) -> Result<String, String> {
        self.choose_state_from(title, info, options, None, &[], selected)
    }
    fn choose_state_from(
        &self,
        title: &str,
        info: &[String],
        options: &[(String, String, String)],
        ready: Option<[bool; 2]>,
        disabled: &[&str],
        selected: usize,
    ) -> Result<String, String> {
        super::reap_apps();
        if options.is_empty() { return Ok("q".into()); }
        let enabled = |i: usize| !disabled.contains(&options[i].0.as_str());
        let mut selected = selected.min(options.len() - 1);
        if !self.color {
            println!("\n{title}");
            for line in info {
                println!("{}", clean(line));
            }
            for (i, (key, label, _)) in options.iter().enumerate() {
                println!(
                    "{key}. {label}{}",
                    if enabled(i) { "" } else { " (not ready)" }
                );
            }
            let choice = line("Choose: ")?.to_lowercase();
            return Ok(
                if options
                    .iter()
                    .position(|o| o.0 == choice)
                    .is_some_and(|i| !enabled(i))
                {
                    String::new()
                } else {
                    choice
                },
            );
        }
        let _input = console::Input::enter()?;
        let mut last_frame = None;
        let mut offset = 0;
        let mut max_offset = 0;
        let mut option_offset = 0;
        let mut visible_options = options.len();
        let mut shortcut = String::new();
        let numbered = options.iter().any(|o| o.0.len() > 1 && o.0.bytes().all(|c| c.is_ascii_digit()));
        loop {
            if super::reap_apps() { last_frame = None; }
            let size = console::size();
            let frame = (size, selected, offset);
            if last_frame != Some(frame) {
                last_frame = Some(frame);
                if !too_small() {
                    let inset = if ready.is_some() { 4 } else { 0 };
                    let detail_width = size.0.saturating_sub(12).clamp(20, 82) - inset;
                    let info_lines: Vec<_> = info
                        .iter()
                        .flat_map(|line| wrapped(line, detail_width))
                        .collect();
                    let intro_rows = if ready.is_some() { 2 } else { 0 };
                    let max_info = size.1.saturating_sub(options.len() + 13 + intro_rows).max(1);
                    max_offset = info_lines.len().saturating_sub(max_info);
                    offset = offset.min(max_offset);
                    let info_lines: Vec<_> = info_lines.iter().skip(offset).take(max_info).collect();
                    visible_options = size.1.saturating_sub(info_lines.len() + 13 + intro_rows).max(1).min(options.len());
                    option_offset = option_offset.min(options.len() - visible_options);
                    if selected < option_offset { option_offset = selected; }
                    if selected >= option_offset + visible_options { option_offset = selected + 1 - visible_options; }
                    let scrolling = visible_options < options.len();
                    let height = (info_lines.len() + visible_options + 5 + intro_rows).max(11);
                    let footer = if scrolling {
                        " ↑↓ Scroll   PgUp/PgDn   Enter Open   Esc Back"
                    } else if max_offset > 0 {
                        " PgUp/PgDn Read   ↑↓ Select   Enter Confirm   Esc Back"
                    } else if ready.is_some() {
                        " ↑↓ Select   Enter Continue   Esc Quit"
                    } else {
                        " ↑↓ Select   Enter Confirm   Esc Back"
                    };
                    let (x, y, width) = backdrop(title, footer, height);
                    if ready.is_some() {
                        panel(x + 2, y + 1, width - 4, info_lines.len() + 2, false, false);
                    }
                    for (i, text) in info_lines.iter().enumerate() {
                        row(y + 1 + i + intro_rows / 2, x + 2 + intro_rows, text, "30;47", width - 4 - inset);
                    }
                    let first = y + info_lines.len() + 2 + intro_rows;
                    if scrolling {
                        let range = format!("{}–{} / {}", option_offset + 1, option_offset + visible_options, options.len());
                        row(first - 1, x + width - 2 - range.chars().count(), &range, "90;47", range.chars().count());
                        let thumb = (visible_options * visible_options / options.len()).max(1);
                        let top = option_offset * (visible_options - thumb) / (options.len() - visible_options);
                        for i in 0..visible_options {
                            row(first + i, x + width - 2, if i >= top && i < top + thumb { "█" } else { "│" }, "90;47", 1);
                        }
                    }
                    for (i, (key, label, _)) in options.iter().enumerate().skip(option_offset).take(visible_options) {
                        let style = if i == selected {
                            if enabled(i) { "1;97;40" } else { "90;40" }
                        } else if !enabled(i) {
                            "90;47"
                        } else {
                            "30;47"
                        };
                        let mark = ready.map(|r| {
                            if i < r.len() && r[i] {
                                "[✓]"
                            } else if i < r.len() {
                                "[ ]"
                            } else {
                                "   "
                            }
                        });
                        row(
                            first + i - option_offset,
                            x + 2,
                            &format!("{} {key}. {label}", mark.unwrap_or("")),
                            style,
                            width - 4 - usize::from(scrolling),
                        );
                        if ready.is_some_and(|r| i < r.len() && r[i]) {
                            row(
                                first + i - option_offset,
                                x + 2,
                                "[✓]",
                                if i == selected { "32;40" } else { "32;47" },
                                3,
                            );
                        }
                    }
                    row(
                        y + height - 2,
                        x + 2,
                        &options[selected].2,
                        "90;47",
                        width - 4,
                    );
                    present().map_err(|e| e.to_string())?;
                }
            }
            match console::key()? {
                Key::PageUp if visible_options < options.len() => { selected = selected.saturating_sub(visible_options); shortcut.clear(); }
                Key::PageDown if visible_options < options.len() => { selected = (selected + visible_options).min(options.len() - 1); shortcut.clear(); }
                Key::PageUp => offset = offset.saturating_sub(3),
                Key::PageDown => offset = (offset + 3).min(max_offset),
                Key::Home => { selected = 0; shortcut.clear(); }
                Key::End => { selected = options.len() - 1; shortcut.clear(); }
                #[cfg(not(windows))]
                Key::WheelUp => {
                    if max_offset > 0 && visible_options == options.len() { offset = offset.saturating_sub(3); }
                    else { selected = selected.saturating_sub(3); }
                    shortcut.clear();
                }
                #[cfg(not(windows))]
                Key::WheelDown => {
                    if max_offset > 0 && visible_options == options.len() { offset = (offset + 3).min(max_offset); }
                    else { selected = (selected + 3).min(options.len() - 1); }
                    shortcut.clear();
                }
                Key::Up | Key::Left => { selected = if visible_options < options.len() { selected.saturating_sub(1) } else { (selected + options.len() - 1) % options.len() }; shortcut.clear(); }
                Key::Down | Key::Right => { selected = if visible_options < options.len() { (selected + 1).min(options.len() - 1) } else { (selected + 1) % options.len() }; shortcut.clear(); }
                Key::Enter if enabled(selected) => return Ok(options[selected].0.clone()),
                Key::Quit => return Ok("q".into()),
                Key::Char(c) => {
                    if numbered && c.is_ascii_digit() {
                        shortcut.push(c);
                        if !options.iter().any(|o| o.0.starts_with(&shortcut)) { shortcut = c.to_string(); }
                        if let Some(i) = options.iter().position(|o| o.0 == shortcut) {
                            selected = i;
                            if enabled(i) && !options.iter().any(|o| o.0.len() > shortcut.len() && o.0.starts_with(&shortcut)) {
                                return Ok(options[i].0.clone());
                            }
                        }
                        continue;
                    }
                    shortcut.clear();
                    let key = c.to_ascii_lowercase().to_string();
                    if let Some(i) = options.iter().position(|o| o.0 == key) {
                        selected = i;
                        if enabled(i) {
                            return Ok(key);
                        }
                    } else if key == "q" {
                        return Ok(key);
                    }
                }
                _ => (),
            }
        }
    }
    pub(super) fn confirm(&self, title: &str, info: &[String]) -> Result<bool, String> {
        if !self.color {
            return Ok(matches!(
                line(&format!("{}\n{} [Y/n] ", title, info.join("\n")))?
                    .to_lowercase()
                    .as_str(),
                "" | "y" | "yes"
            ));
        }
        self.choose(
            title,
            info,
            &[
                (
                    "y".into(),
                    "Yes, accept and continue".into(),
                    "Enter accepts the terms and starts this step".into(),
                ),
                (
                    "n".into(),
                    "No, return to setup".into(),
                    "Nothing will be installed".into(),
                ),
            ],
        )
        .map(|v| v == "y")
    }
    /// A general notice with an explicit continue/quit answer, separate from
    /// vendor license consent. `info` holds the notice only; the question
    /// "Do you want to continue?" is added here once. Yes is selected; Enter
    /// continues. No, Escape, Ctrl-C and closed input quit. Plain terminals
    /// reprompt on other input and treat closed input or an Escape character
    /// as quitting, never as the default.
    pub(super) fn acknowledge(&self, title: &str, info: &[String]) -> Result<bool, String> {
        const QUESTION: &str = "Do you want to continue?";
        if !self.color {
            println!("\n{title}");
            for line in info {
                println!("{}", clean(line));
            }
            loop {
                let Ok(answer) = line(&format!("{QUESTION} [Y/n] ")) else { return Ok(false) };
                if answer.contains('\u{1b}') {
                    return Ok(false);
                }
                match answer.to_lowercase().as_str() {
                    "" | "y" | "yes" => return Ok(true),
                    "n" | "no" => return Ok(false),
                    _ => println!("Please answer y or n."),
                }
            }
        }
        let mut lines = info.to_vec();
        lines.push(QUESTION.to_owned());
        let choice = self.choose_from(
            title,
            &lines,
            &[
                ("y".into(), "Yes, continue".into(), "Enter continues with setup".into()),
                ("n".into(), "No, quit setup".into(), "Nothing is downloaded or installed".into()),
            ],
            0,
        )?;
        Ok(choice == "y")
    }
    pub(super) fn message(&self, title: &str, text: &str) -> Result<(), String> {
        self.choose(
            title,
            &[text.into()],
            &[("\u{21b5}".into(), "Continue".into(), String::new())],
        )
        .map(|_| ())
    }
}
pub(super) struct Pause(bool);
impl Drop for Pause {
    fn drop(&mut self) {
        if self.0 {print!("\x1b[?1049h\x1b[?25l");invalidate();let _=io::stdout().flush();}
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        if self.color {
            SCREEN_DEPTH.with(|depth| {
                depth.set(depth.get().saturating_sub(1));
                if depth.get()==0 {print!("\x1b[0m\x1b[?25h\x1b[?1049l");invalidate();}
            });
            let _ = io::stdout().flush();
        }
    }
}
fn meter(x: usize, y: usize, width: usize, fraction: Option<f64>, tick: usize) {
    row(
        y,
        x,
        &format!("[{}]", "·".repeat(width - 2)),
        "90;47",
        width,
    );
    let inner = width - 2;
    if let Some(f) = fraction {
        let filled = (f.clamp(0.0, 1.0) * inner as f64).round() as usize;
        row(y, x + 1, &"█".repeat(filled), "32;47", filled);
    } else {
        let position = tick % inner.saturating_sub(3).max(1);
        row(y, x + 1 + position, "███", "90;47", inner.min(3));
    }
}
pub fn with_progress<R>(work: impl FnOnce() -> R) -> R {
    let _screen=Screen::enter();
    let color = io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let mut last = Instant::now() - Duration::from_secs(1);
    let mut identity = String::new();
    let mut detail = String::new();
    let mut started = Instant::now();
    let mut baseline = 0;
    progress::scope(
        move |p| {
            let phase = format!("{}:{}:{}", p.package.group, p.package.index, p.stage);
            let changed = identity != phase;
            if changed {
                identity = phase;
                started = Instant::now();
                baseline = p.loaded;
            }
            // File/object callbacks can arrive once per item. Keep their
            // current detail in the progress dialog, but avoid turning the
            // activity pane into a log of every path in a large repository.
            let item_detail = matches!(p.unit, progress::Unit::Files | progress::Unit::Blocks | progress::Unit::Objects);
            if changed || (detail != p.detail && !item_detail) {
                activity(&format!("{}: {}", p.stage, p.detail));
                detail = p.detail.clone();
            }
            if !changed
                && last.elapsed() < Duration::from_millis(100)
                && !(p.total > 0 && p.loaded >= p.total)
            {
                return;
            }
            last = Instant::now();
            let fraction = if p.total > 0 {
                Some(p.loaded as f64 / p.total as f64)
            } else if p.stage == "Ready" {
                Some(1.0)
            } else {
                None
            };
            let amount = match p.unit {
                progress::Unit::Bytes if p.total > 0 => format!(
                    "{:.1} / {:.1} MiB",
                    p.loaded as f64 / 1048576.,
                    p.total as f64 / 1048576.
                ),
                progress::Unit::Bytes => format!("{:.1} MiB", p.loaded as f64 / 1048576.),
                progress::Unit::Files | progress::Unit::Blocks | progress::Unit::Objects => format!(
                    "{}{} {}",
                    p.loaded,
                    if p.total > 0 {
                        format!(" / {}", p.total)
                    } else {
                        String::new()
                    },
                    if p.unit == progress::Unit::Files {
                        "files"
                    } else if p.unit == progress::Unit::Objects {
                        "objects"
                    } else {
                        "blocks"
                    }
                ),
                _ => {
                    if p.stage == "Ready" {
                        "Complete".into()
                    } else {
                        "Working…".into()
                    }
                }
            };
            let elapsed = started.elapsed().as_secs_f64();
            let rate = if p.stage == "Download" && elapsed > 0.25 && p.loaded > baseline {
                let speed = (p.loaded - baseline) as f64 / elapsed;
                format!(
                    "  {:.1} MiB/s{}",
                    speed / 1048576.,
                    if p.total > p.loaded {
                        format!(
                            "  ~{}s left",
                            ((p.total - p.loaded) as f64 / speed).ceil() as u64
                        )
                    } else {
                        String::new()
                    }
                )
            } else {
                String::new()
            };
            if !color {
                println!("{}: {}  {amount}{rate}", p.stage, clean(&p.detail));
                return;
            }
            console::enable();
            if too_small() {
                return;
            }
            let (x, y, width) = backdrop(
                "Makepad Builder",
                " Setup is running. This window will return to the checklist when finished.",
                11,
            );
            row(
                y + 1,
                x + 2,
                if p.package.group.is_empty() {
                    &p.stage
                } else {
                    &p.package.group
                },
                "1;30;47",
                width - 4,
            );
            row(y + 2, x + 2, &p.package.name, "30;47", width - 4);
            if p.package.count > 0 {
                let done = if p.stage == "Ready" {
                    p.package.count
                } else {
                    p.package.index.saturating_sub(1)
                };
                row(
                    y + 3,
                    x + 2,
                    &format!("Package {} of {}", p.package.index, p.package.count),
                    "30;47",
                    width - 4,
                );
                meter(
                    x + 2,
                    y + 4,
                    width - 4,
                    Some(done as f64 / p.package.count as f64),
                    0,
                );
            }
            row(y + 5, x + 2, &p.stage, "1;30;47", width - 4);
            row(y + 6, x + 2, &p.detail, "30;47", width - 4);
            meter(x + 2, y + 7, width - 4, fraction, (elapsed * 8.) as usize);
            row(
                y + 8,
                x + 2,
                &format!(
                    "{}{amount}{rate}",
                    fraction.map_or(String::new(), |f| format!("{:3.0}%  ", f * 100.))
                ),
                "30;47",
                width - 4,
            );
            let _ = present();
        },
        work,
    )
}
