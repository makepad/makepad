//! Desk Link's panels, drawn with the shell kit so the WM's bar flyout and the tray dropdowns of
//! Desk Host and the Desk Link viewer look the same as the AI Hub's:
//!
//! - [`draw_viewers`]: who views this desktop (the WM's Desk Link flyout, Desk Host's dropdown):
//!   address and name, watching or not and since when, the stream (codec, chroma, pictures a
//!   second, rate), Disconnect;
//! - [`draw_desktops`]: the desktops a viewer knows (the viewer's menu-bar dropdown), each with
//!   whether it is reachable;
//! - [`tray_glyph`]: the tray glyphs (a screen: off, serving, watched; the viewer's).

use makepad_shell_kit::ui::{cut_top, rect, HAlign, ShellDraw};
use makepad_shell_kit::{alpha, darker, CtrlState, ShellTokens};
use makepad_widgets::*;

pub mod tray_glyph;

/// Someone viewing this desktop.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewerRow {
    pub session: u32,
    /// What the viewer calls itself (`desklink <user>`).
    pub name: String,
    /// Its address (IP only).
    pub peer: String,
    /// Not watching right now (the viewer shows another desktop); still connected.
    pub paused: bool,
    /// Seconds since it connected.
    pub secs: u64,
    /// The stream it gets ([`stream_line`]); empty while nothing is sent.
    pub stream: String,
}

/// A stream in words: `H.264 · 4:2:0 · 60 fps · 250 Mbit/s` (`kbps` 0: no rate said; `streams` >
/// 1: tiled).
pub fn stream_line(hevc: bool, full_chroma: bool, fps: u32, kbps: u32, streams: u32) -> String {
    let mut line = format!("{} \u{b7} {} \u{b7} {fps} fps", if hevc { "H.265" } else { "H.264" }, if full_chroma { "4:4:4" } else { "4:2:0" });
    if kbps > 0 {
        line.push_str(&format!(" \u{b7} {} Mbit/s", (kbps + 500) / 1000));
    }
    if streams > 1 {
        line.push_str(&format!(" \u{b7} {streams} tiles"));
    }
    line
}

/// The height of one viewer row.
pub const VIEWER_ROW: f64 = 62.0;

fn since(secs: u64) -> String {
    let mins = secs / 60;
    if mins == 0 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins} min")
    } else {
        format!("{} h {} min", mins / 60, mins % 60)
    }
}

/// The viewers in `area`, one [`VIEWER_ROW`] each, a Disconnect button on each (its session and
/// rect pushed to `hits`; `hot` is the session under the pointer). Returns what is left below.
pub fn draw_viewers(cx: &mut Cx2d, d: &mut ShellDraw, tok: &ShellTokens, area: Rect, viewers: &[ViewerRow], hot: Option<u32>, hits: &mut Vec<(u32, Rect)>) -> Rect {
    let fg = tok.popups.text;
    let mut rest = area;
    for v in viewers {
        let (row, r2) = cut_top(rest, VIEWER_ROW);
        rest = r2;
        let w = d.button_width(cx, tok, false, "Disconnect", tok.font.body_small);
        let button = rect(row.pos.x + row.size.x - w, row.pos.y + 12.0, w, 30.0);
        let state = if hot == Some(v.session) { CtrlState::Hover } else { CtrlState::Normal };
        d.button(cx, button, tok, state, None, "Disconnect", tok.font.body_small, fg, true);
        hits.push((v.session, button));
        let text_w = row.size.x - w - 8.0;
        d.label_elided(cx, rect(row.pos.x, row.pos.y + 4.0, text_w, 18.0), false, tok.font.body, fg, HAlign::Left, &format!("{} \u{b7} {}", v.peer, v.name));
        let detail = format!("{} \u{b7} {}", if v.paused { "connected, not watching" } else { "watching" }, since(v.secs));
        d.label_elided(cx, rect(row.pos.x, row.pos.y + 23.0, text_w, 16.0), false, tok.font.caption, darker(fg, 1.2), HAlign::Left, &detail);
        if !v.stream.is_empty() && !v.paused {
            d.label_elided(cx, rect(row.pos.x, row.pos.y + 40.0, text_w, 16.0), false, tok.font.caption, darker(fg, 1.4), HAlign::Left, &v.stream);
        }
    }
    rest
}

/// Whether a desktop the viewer knows answers.
#[derive(Clone, Debug, PartialEq)]
pub enum Reach {
    /// Connected, with the last round trip (ms).
    Online(Option<f64>),
    Connecting,
    Offline(String),
}

/// A desktop the viewer knows.
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopRow {
    pub name: String,
    /// `host:port`.
    pub addr: String,
    pub reach: Reach,
    /// It is the one the viewer's window shows.
    pub shown: bool,
}

/// The height of one desktop row.
pub const DESKTOP_ROW: f64 = 44.0;

/// The desktops in `area`, one [`DESKTOP_ROW`] each, each row a hit (its index and rect pushed
/// to `hits`; `hot` is the row under the pointer). Returns what is left below.
pub fn draw_desktops(cx: &mut Cx2d, d: &mut ShellDraw, tok: &ShellTokens, area: Rect, desktops: &[DesktopRow], hot: Option<usize>, hits: &mut Vec<(usize, Rect)>) -> Rect {
    let fg = tok.popups.text;
    let mut rest = area;
    for (i, desk) in desktops.iter().enumerate() {
        let (row, r2) = cut_top(rest, DESKTOP_ROW);
        rest = r2;
        let state = if desk.shown { CtrlState::Selected } else if hot == Some(i) { CtrlState::Hover } else { CtrlState::Normal };
        if state != CtrlState::Normal {
            d.button(cx, rect(row.pos.x - 6.0, row.pos.y + 1.0, row.size.x + 12.0, row.size.y - 2.0), tok, state, None, "", tok.font.body, fg, false);
        }
        hits.push((i, row));
        // the reachability dot
        let (dot, line) = match &desk.reach {
            Reach::Online(rtt) => (vec4(0.35, 0.85, 0.45, 1.0), format!("{} \u{b7} {}", desk.addr, rtt.map(|r| format!("{r:.1} ms")).unwrap_or_else(|| "reachable".into()))),
            Reach::Connecting => (alpha(fg, 0.5), format!("{} \u{b7} connecting", desk.addr)),
            Reach::Offline(why) => (vec4(0.9, 0.45, 0.4, 1.0), format!("{} \u{b7} {why}", desk.addr)),
        };
        d.label(cx, rect(row.pos.x, row.pos.y + 4.0, 14.0, 18.0), false, tok.font.body, dot, HAlign::Left, "\u{25cf}");
        d.label_elided(cx, rect(row.pos.x + 18.0, row.pos.y + 4.0, row.size.x - 18.0, 18.0), false, tok.font.body, fg, HAlign::Left, &desk.name);
        d.label_elided(cx, rect(row.pos.x + 18.0, row.pos.y + 23.0, row.size.x - 18.0, 16.0), false, tok.font.caption, darker(fg, 1.3), HAlign::Left, &line);
    }
    rest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_lines_say_codec_chroma_rate() {
        assert_eq!(stream_line(false, false, 60, 250_000, 1), "H.264 \u{b7} 4:2:0 \u{b7} 60 fps \u{b7} 250 Mbit/s");
        assert_eq!(stream_line(true, true, 120, 0, 4), "H.265 \u{b7} 4:4:4 \u{b7} 120 fps \u{b7} 4 tiles");
    }
}
