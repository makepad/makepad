//! The tray icon: the hub glyph of the app icon (a centre with six spokes to its nodes)
//! as a monochrome template, so it follows a light or dark menu bar, drawn here from
//! geometry. Its state reads from the glyph alone:
//!
//! * serving: solid centre and nodes;
//! * off: thin spokes, hollow centre, faint nodes;
//! * a problem (a backend on that does not serve, a failed start): the centre is a ring
//!   with a bar through it;
//! * a request at work: the nodes grow, alternating with the serving glyph (pulse).

use crate::settings::{Backend, Settings};
use makepad_widgets::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Serving,
    Working,
    Off,
    Problem,
}

pub const SIZE: u32 = 36;

/// The alpha mask of `glyph`, `SIZE` x `SIZE`.
pub fn image(glyph: Glyph) -> TrayImage {
    let n = SIZE as usize;
    let c = (n as f32 - 1.0) * 0.5;
    let spoke_r = 13.0;
    let (node_r, centre_r, spoke_w, node_a) = match glyph {
        Glyph::Serving => (3.4, 5.6, 1.7, 1.0),
        Glyph::Working => (4.4, 5.6, 1.9, 1.0),
        Glyph::Off => (2.8, 5.0, 1.0, 0.45),
        Glyph::Problem => (3.0, 6.0, 1.3, 0.7),
    };
    let nodes: Vec<(f32, f32)> = (0..6)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / 6.0;
            (c + spoke_r * a.cos(), c - spoke_r * a.sin())
        })
        .collect();
    let mut alpha = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32, y as f32);
            let mut v: f32 = 0.0;
            // Anti-aliased coverage: 1 inside, falling to 0 over one pixel.
            let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
            for &(nx, ny) in &nodes {
                v = v.max(cover(((px - nx).powi(2) + (py - ny).powi(2)).sqrt() - node_r) * node_a);
                // The spoke from the centre's edge to the node.
                let (dx, dy) = (nx - c, ny - c);
                let len2 = dx * dx + dy * dy;
                let t = (((px - c) * dx + (py - c) * dy) / len2).clamp(0.0, 1.0);
                let (qx, qy) = (c + t * dx, c + t * dy);
                let d = ((px - qx).powi(2) + (py - qy).powi(2)).sqrt() - spoke_w * 0.5;
                if t * len2.sqrt() > centre_r {
                    v = v.max(cover(d) * node_a.max(0.6));
                }
            }
            let r = ((px - c).powi(2) + (py - c).powi(2)).sqrt();
            match glyph {
                Glyph::Serving | Glyph::Working => v = v.max(cover(r - centre_r)),
                Glyph::Off => v = v.max(cover((r - (centre_r - 0.9)).abs() - 0.9)),
                Glyph::Problem => {
                    v = v.max(cover((r - (centre_r - 1.0)).abs() - 1.0));
                    // A bar through the ring.
                    let along = (px - c + py - c) / std::f32::consts::SQRT_2;
                    let across = ((px - c) - (py - c)).abs() / std::f32::consts::SQRT_2;
                    if along.abs() <= centre_r {
                        v = v.max(cover(across - 1.1));
                    }
                }
            }
            alpha[y * n + x] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    TrayImage { width: SIZE, height: SIZE, alpha }
}

pub const TRAY: LiveId = live_id!(aihub_tray);
pub const OPEN: LiveId = live_id!(open);
pub const ENABLED: LiveId = live_id!(enabled);
pub const CLAUDE: LiveId = live_id!(claude);
pub const LOCAL: LiveId = live_id!(local);
pub const QUIT: LiveId = live_id!(quit_hub);

/// The tray icon for the hub as it stands. `quit`: the menu offers Quit (not on a WM node,
/// whose WM keeps its hub running; Enabled turns it off there).
pub fn icon(settings: &Settings, glyph: Glyph, status: &str, quit: bool) -> TrayIcon {
    let svc = |b: Backend, id: LiveId| {
        let s = settings.service(b);
        let label = match (s.on, s.auto_off_at) {
            (false, Some(_)) => format!("{} (turned off when idle)", b.label()),
            _ => b.label().to_string(),
        };
        TrayMenuItem::Item { id, label, enabled: settings.enabled, checked: Some(s.on) }
    };
    let mut menu = vec![
        TrayMenuItem::Item { id: live_id!(status), label: status.to_string(), enabled: false, checked: None },
        TrayMenuItem::Line,
        TrayMenuItem::check(ENABLED, "Enabled", settings.enabled),
        svc(Backend::Claude, CLAUDE),
        svc(Backend::Local, LOCAL),
        TrayMenuItem::Line,
        TrayMenuItem::item(OPEN, "Open in window"),
    ];
    if quit {
        menu.push(TrayMenuItem::item(QUIT, "Quit AI Hub (stops serving)"));
    }
    // a click opens the dropdown (the panel) where the desktop gives the click to the app; the
    // menu is the secondary click's
    TrayIcon { id: TRAY, image: image(glyph), tooltip: format!("AI Hub \u{b7} {status}"), menu, dropdown: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wm_nodes_menu_has_no_quit() {
        let s = Settings::default();
        let has_quit = |t: TrayIcon| t.menu.iter().any(|i| matches!(i, TrayMenuItem::Item { id, .. } if *id == QUIT));
        assert!(has_quit(icon(&s, Glyph::Serving, "Serving", true)));
        assert!(!has_quit(icon(&s, Glyph::Serving, "Serving", false)));
    }

    #[test]
    fn every_glyph_is_a_distinct_full_size_mask() {
        let all = [Glyph::Serving, Glyph::Working, Glyph::Off, Glyph::Problem].map(image);
        for img in &all {
            assert_eq!(img.alpha.len(), (SIZE * SIZE) as usize);
            assert!(img.alpha.iter().any(|a| *a == 255) || img.alpha.iter().any(|a| *a > 100));
            assert_eq!(img.alpha[0], 0, "the corners stay clear");
        }
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i].alpha, all[j].alpha);
            }
        }
    }
}
