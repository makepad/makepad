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

pub use makepad_ai_hub_panel::tray_glyph::{image, Glyph};

pub const TRAY: LiveId = live_id!(aihub_tray);
pub const OPEN: LiveId = live_id!(open);
pub const ENABLED: LiveId = live_id!(enabled);
pub const CLAUDE: LiveId = live_id!(claude);
pub const LOCAL: LiveId = live_id!(local);
pub const QUIT: LiveId = live_id!(quit_hub);

/// The tray icon for the hub as it stands. `window`: the menu offers "Open in window" (not on a
/// makepad WM node, whose only UI is the tray's dropdown). Quit is on every desktop; on a WM node
/// it is deliberate and the hub stays off until the person starts it from the launcher.
pub fn icon(settings: &Settings, glyph: Glyph, status: &str, window: bool) -> TrayIcon {
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
    ];
    if window {
        menu.push(TrayMenuItem::item(OPEN, "Open in window"));
    }
    menu.push(TrayMenuItem::item(QUIT, "Quit AI Hub (stops serving)"));
    // a click opens the dropdown (the panel) where the desktop gives the click to the app; the
    // menu is the secondary click's
    TrayIcon { id: TRAY, image: image(glyph), tooltip: format!("AI Hub \u{b7} {status}"), menu, dropdown: true }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_hub_panel::tray_glyph::SIZE;

    #[test]
    fn a_wm_nodes_menu_has_quit_but_no_window() {
        let s = Settings::default();
        let has = |t: &TrayIcon, wanted: LiveId| t.menu.iter().any(|i| matches!(i, TrayMenuItem::Item { id, .. } if *id == wanted));
        let desktop = icon(&s, Glyph::Serving, "Serving", true);
        let wm = icon(&s, Glyph::Serving, "Serving", false);
        assert!(has(&desktop, QUIT) && has(&desktop, OPEN));
        assert!(has(&wm, QUIT) && !has(&wm, OPEN));
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
