//! The windows of the last run, restored at launch: `~/.makepad/desklink/windows`, one line a
//! window, `x y width height host:port` (position and inner size in points, the machine it
//! showed). Written a second after a change and at quit.

use crate::App;
use makepad_widgets::*;

/// One window to restore.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedWindow {
    pub pos: Vec2d,
    pub size: Vec2d,
    pub host: String,
    pub port: u16,
}

fn path() -> std::path::PathBuf {
    makepad_network::fleet::makepad_home().join("desklink/windows")
}

pub fn parse(text: &str) -> Vec<SavedWindow> {
    text.lines()
        .filter_map(|line| {
            let mut w = line.split_whitespace();
            let mut num = || w.next().and_then(|n| n.parse::<f64>().ok());
            let (x, y, width, height) = (num()?, num()?, num()?, num()?);
            let (host, port) = crate::port_of(w.next()?);
            (width >= 100.0 && height >= 100.0 && !host.is_empty()).then(|| SavedWindow { pos: dvec2(x, y), size: dvec2(width, height), host, port })
        })
        .collect()
}

pub fn format(windows: &[SavedWindow]) -> String {
    windows.iter().map(|w| format!("{:.0} {:.0} {:.0} {:.0} {}:{}\n", w.pos.x, w.pos.y, w.size.x, w.size.y, w.host, w.port)).collect()
}

pub fn load() -> Vec<SavedWindow> {
    std::fs::read_to_string(path()).map(|t| parse(&t)).unwrap_or_default()
}

impl App {
    /// The windows on screen now (the first window when none is: closed into the Dock).
    pub fn save_windows(&mut self, cx: &mut Cx) {
        self.restore_dirty = false;
        let shown: Vec<usize> = (0..self.viewers.len()).filter(|vi| self.viewers[*vi].visible).collect();
        let list = if shown.is_empty() { vec![0] } else { shown };
        let mut out = Vec::new();
        for vi in list {
            let v = &self.viewers[vi];
            let Some(c) = v.selected.and_then(|i| self.conns.get(i)) else { continue };
            if c.port == 0 {
                continue; // a dump played as a desktop
            }
            let w = v.window.as_window();
            out.push(SavedWindow { pos: w.get_position(cx), size: w.get_inner_size(cx), host: c.host.clone(), port: c.port });
        }
        let text = format(&out);
        let file = path();
        if std::fs::read_to_string(&file).ok().as_deref() == Some(text.as_str()) {
            return;
        }
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&file, text) {
            eprintln!("desklink: {}: {e}", file.display());
        }
    }

    /// At launch: the saved windows again, each on the machine it showed (one no longer known
    /// shows the first machine); none saved: the first window on `first`.
    pub fn restore_windows(&mut self, cx: &mut Cx, first: Option<usize>) {
        let saved = if std::env::args().any(|a| a == "--no-restore") { Vec::new() } else { load() };
        let conn_of = |app: &App, w: &SavedWindow| app.conns.iter().position(|c| c.host == w.host && c.port == w.port);
        let Some((head, rest)) = saved.split_first() else {
            if let Some(i) = first.or((!self.conns.is_empty()).then_some(0)) {
                self.select(cx, 0, i);
            }
            return;
        };
        if let Some(id) = self.viewers[0].window_id() {
            if cx.windows.is_valid(id) {
                cx.windows[id].create_position = Some(head.pos);
                cx.windows[id].create_inner_size = Some(head.size);
            }
        }
        if let Some(i) = first.or_else(|| conn_of(self, head)).or((!self.conns.is_empty()).then_some(0)) {
            self.select(cx, 0, i);
        }
        for w in rest {
            let conn = conn_of(self, w);
            self.new_window(cx, conn.or(Some(0)).filter(|_| !self.conns.is_empty()), Some((w.pos, w.size)));
        }
        self.focused = 0;
        crate::viewer::selftest(&format!("restored {} windows", saved.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_round_trip() {
        let w = vec![SavedWindow { pos: dvec2(10.0, 20.0), size: dvec2(1400.0, 860.0), host: "10.0.0.217".into(), port: 8790 }, SavedWindow { pos: dvec2(-1500.0, 0.0), size: dvec2(700.0, 860.0), host: "10.0.0.100".into(), port: 18801 }];
        assert_eq!(parse(&format(&w)), w);
        // junk and too-small windows are skipped
        assert_eq!(parse("1 2 3\n0 0 50 50 10.0.0.1:1\nx y 300 300 a:1\n").len(), 0);
    }
}
