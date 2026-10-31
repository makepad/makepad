//! Window skins: novel chrome that layers on whichever desktop style is
//! running. A skin does not add a `DesktopStyle`; it takes over the window
//! frame (inset and title height) and paints it with one quad drawn after the
//! window's content (`DrawSkin`), plus the title text.
//!
//! * Holo: neon edge lines with a glow, a thin scanline title strip, the
//!   controls as outlines. Focus is brighter and thicker.
//! * Float: no title bar. A small handle floats over the window's top edge
//!   and, under the pointer, grows to the title and the controls. A long soft
//!   shadow. The content has the whole rect.
//! * Facet: a bevelled crystal frame whose highlights follow the background's
//!   light direction, with a softly lit title.
//!
//! Choice: the theme token `mod.wm_theme.skin` (`"none"`, `"holo"`, `"float"`,
//! `"facet"`), the `MAKEPAD_WM_SKIN` variable, and SUPER+CTRL+S, which cycles.
//! `None` leaves every code path of the desk as it was.
use crate::desk::ChromeHit;
use makepad_app_host::ClientId;
use crate::shell::ui::{rect, HAlign, ShellDraw};
use makepad_widgets::*;

mod shader;
pub use shader::DrawSkin;

pub fn script_mod(vm: &mut ScriptVm) {
    shader::script_mod(vm);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Skin {
    #[default]
    None,
    Holo,
    Float,
    Facet,
}

impl Skin {
    pub const ALL: [Skin; 4] = [Skin::None, Skin::Holo, Skin::Float, Skin::Facet];

    pub fn name(self) -> &'static str {
        match self {
            Skin::None => "none",
            Skin::Holo => "holo",
            Skin::Float => "float",
            Skin::Facet => "facet",
        }
    }

    pub fn from_name(name: &str) -> Option<Skin> {
        Skin::ALL.into_iter().find(|s| s.name() == name)
    }

    /// The next skin in the cycle none, holo, float, facet.
    pub fn next(self) -> Skin {
        Skin::ALL[(Skin::ALL.iter().position(|s| *s == self).unwrap_or(0) + 1) % Skin::ALL.len()]
    }

    pub fn active(self) -> bool {
        self != Skin::None
    }

    /// The skin `MAKEPAD_WM_SKIN` names.
    pub fn from_env() -> Option<Skin> {
        std::env::var("MAKEPAD_WM_SKIN").ok().and_then(|n| Skin::from_name(n.trim()))
    }

    /// The theme's `skin: "name"` token, when the theme has one.
    pub fn from_theme_source(source: &str) -> Option<Skin> {
        source.lines().find_map(|line| {
            let (key, value) = line.trim().split_once(':')?;
            (key.trim() == "skin").then(|| Skin::from_name(value.trim().trim_matches(|c| c == '"' || c == ',')))?
        })
    }

    /// The skin paints its own light or shadow around the window, so the
    /// style's shadow stays off.
    pub fn own_shadow(self) -> bool {
        matches!(self, Skin::Holo | Skin::Float)
    }

    /// The frame inset around the content, given the style's own.
    pub fn inset(self, base: f64) -> f64 {
        match self {
            Skin::None => base,
            Skin::Holo => 2.0,
            Skin::Float => 0.0,
            Skin::Facet => 4.0,
        }
    }

    /// The title bar's height, given the style's own.
    pub fn title_height(self, base: f64) -> f64 {
        match self {
            Skin::None => base,
            Skin::Holo => 22.0,
            Skin::Float => 0.0,
            Skin::Facet => 26.0,
        }
    }

    /// How far the skin's quad reaches beyond the window; the desk widens
    /// the window's layer rect to match.
    pub fn margin(self) -> f64 {
        match self {
            Skin::None => 0.0,
            Skin::Holo => 24.0,
            Skin::Float => 36.0,
            Skin::Facet => 2.0,
        }
    }
}

/// Where a skin puts its parts, in screen coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The drag area: the title strip, or the Float handle.
    pub grab: Rect,
    /// Minimize, maximize and close, in that order.
    pub controls: [Rect; 3],
    /// Where the title text goes (empty when it is not shown).
    pub label: Rect,
    /// The first control's centre x, the step between centres and their
    /// centre y, relative to the window; and the Float handle's rect.
    pub btn: [f32; 3],
    pub pill: Rect,
}

/// A window too small for a skin's parts gets none of them.
/// `expanded`: the pointer is on the window's handle (Float: the controls are
/// live); `grow`: 0..1, how far the handle's grow tween has come.
pub fn layout(skin: Skin, r: Rect, inset: f64, title_h: f64, expanded: bool, grow: f64) -> Option<Layout> {
    if r.size.x < 160.0 || r.size.y < 80.0 {
        return None;
    }
    let none = rect(0.0, 0.0, 0.0, 0.0);
    // `row`: the strip the controls sit in; `step`: their spacing.
    let (row, step, grab, pill) = match skin {
        Skin::None => return None,
        Skin::Holo | Skin::Facet => {
            let t = rect(r.pos.x + inset, r.pos.y + inset, r.size.x - inset * 2.0, title_h);
            (t, 30.0, t, none)
        }
        Skin::Float => {
            // The handle floats ABOVE the window's top edge, in the gap.
            let w = (r.size.x - 24.0).min(320.0);
            let full = rect(r.pos.x + (r.size.x - w) * 0.5, r.pos.y - 30.0, w, 26.0);
            let small = rect(r.pos.x + (r.size.x - 56.0) * 0.5, r.pos.y - 14.0, 56.0, 10.0);
            let target = if expanded { full } else { small };
            let grab = rect(target.pos.x - 6.0, target.pos.y - 2.0, target.size.x + 12.0, target.size.y + 4.0);
            let (pw, ph) = (56.0 + (w - 56.0) * grow, 10.0 + 16.0 * grow);
            let shown = rect(r.pos.x + (r.size.x - pw) * 0.5, r.pos.y - 4.0 - ph, pw, ph);
            (full, 24.0, grab, shown)
        }
    };
    let pad = if skin == Skin::Float { 16.0 } else { 15.0 };
    let controls_on = skin != Skin::Float || expanded;
    let close_cx = row.pos.x + row.size.x - pad;
    let mut controls = [none; 3];
    if controls_on {
        for (i, c) in controls.iter_mut().enumerate() {
            let cx = close_cx - step * (2 - i) as f64;
            *c = rect(cx - step * 0.5, row.pos.y, step, row.size.y);
        }
    }
    let label = if controls_on {
        let lead = if skin == Skin::Float { 14.0 } else { 10.0 };
        rect(row.pos.x + lead, row.pos.y, (row.size.x - lead - step * 3.0 - 6.0).max(0.0), row.size.y)
    } else {
        none
    };
    Some(Layout {
        grab,
        controls,
        label,
        btn: [(close_cx - step * 2.0 - r.pos.x) as f32, step as f32, (row.pos.y + row.size.y * 0.5 - r.pos.y) as f32],
        pill: if pill.size.x > 0.0 { rect(pill.pos.x - r.pos.x, pill.pos.y - r.pos.y, pill.size.x, pill.size.y) } else { none },
    })
}

/// A direction toward a light: x right, y up, z toward the viewer.
pub type Light = [f32; 3];

/// What `DrawSkin::draw` needs to know about one window.
pub struct Draw<'a> {
    pub skin: Skin,
    /// The window's rect as drawn.
    pub rect: Rect,
    pub inset: f64,
    pub title_h: f64,
    pub focus: f64,
    pub fade: f64,
    /// The control or title under the pointer, for this window.
    pub hover: Option<ChromeHit>,
    /// The Float handle's grow tween, 0..1 (eased).
    pub grow: f64,
    pub accent: Vec4f,
    /// The background's light direction (x right, y up, z toward the viewer).
    pub light: Option<Light>,
    pub title: &'a str,
}

/// The light a skin uses when the background has none to offer.
const FALLBACK_LIGHT: Light = [-0.4, 0.6, 0.7];

impl DrawSkin {
    /// Draws the skin of one window and records its hit regions after
    /// `hits` (title first, then the controls; the caller adds the resize
    /// edges so they stay on top).
    pub fn draw(&mut self, cx: &mut Cx2d, shell: &mut ShellDraw, d: &Draw, client: ClientId, hits: &mut Vec<(ClientId, Rect, ChromeHit)>) {
        let expanded = d.hover.is_some();
        let Some(l) = layout(d.skin, d.rect, d.inset, d.title_h, expanded, d.grow) else {
            return;
        };
        let m = d.skin.margin();
        let r = d.rect;
        self.win = Vec4f { x: m as f32, y: 0.0, z: d.inset as f32, w: d.title_h as f32 };
        let hov = match d.hover {
            Some(ChromeHit::Minimize) => 1.0,
            Some(ChromeHit::Maximize) => 2.0,
            Some(ChromeHit::Close) => 3.0,
            Some(_) => 4.0,
            None => 0.0,
        };
        let kind = match d.skin {
            Skin::None => 0.0,
            Skin::Holo => 1.0,
            Skin::Float => 2.0,
            Skin::Facet => 3.0,
        };
        self.style = Vec4f { x: kind, y: d.focus as f32, z: hov, w: d.grow as f32 };
        self.btn = Vec4f { x: l.btn[0], y: l.btn[1], z: l.btn[2], w: 9.0 };
        self.pill = Vec4f { x: l.pill.pos.x as f32, y: l.pill.pos.y as f32, z: l.pill.size.x as f32, w: l.pill.size.y as f32 };
        let v = d.light.unwrap_or(FALLBACK_LIGHT);
        self.light = Vec4f { x: v[0], y: v[1], z: v[2], w: d.fade as f32 };
        self.accent = d.accent;
        self.draw_abs(cx, rect(r.pos.x - m, r.pos.y - m, r.size.x + m * 2.0, r.size.y + m * 2.0));
        if l.label.size.x > 8.0 {
            let mut ink = self.fg;
            ink.w *= d.fade as f32 * if d.skin == Skin::Float { d.grow as f32 } else { 1.0 };
            shell.label_elided(cx, l.label, false, 12.0, ink, HAlign::Left, d.title);
        }
        hits.push((client, l.grab, ChromeHit::Title));
        for (rect, hit) in l.controls.into_iter().zip([ChromeHit::Minimize, ChromeHit::Maximize, ChromeHit::Close]) {
            if rect.size.x > 0.0 {
                hits.push((client, rect, hit));
            }
        }
    }
}

/// The eight resize edges of a window rect, as `(rect, x, y)`.
pub fn resize_edges(r: Rect) -> [(Rect, i8, i8); 8] {
    [
        (rect(r.pos.x, r.pos.y + 12.0, 5.0, (r.size.y - 24.0).max(0.0)), -1, 0),
        (rect(r.pos.x + r.size.x - 5.0, r.pos.y + 12.0, 5.0, (r.size.y - 24.0).max(0.0)), 1, 0),
        (rect(r.pos.x + 12.0, r.pos.y, (r.size.x - 24.0).max(0.0), 4.0), 0, -1),
        (rect(r.pos.x + 12.0, r.pos.y + r.size.y - 5.0, (r.size.x - 24.0).max(0.0), 5.0), 0, 1),
        (rect(r.pos.x, r.pos.y, 8.0, 8.0), -1, -1),
        (rect(r.pos.x + r.size.x - 8.0, r.pos.y, 8.0, 8.0), 1, -1),
        (rect(r.pos.x, r.pos.y + r.size.y - 12.0, 12.0, 12.0), -1, 1),
        (rect(r.pos.x + r.size.x - 12.0, r.pos.y + r.size.y - 12.0, 12.0, 12.0), 1, 1),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cycle_visits_every_skin_and_names_round_trip() {
        let mut s = Skin::None;
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(s);
            s = s.next();
        }
        assert_eq!(seen, Skin::ALL);
        assert_eq!(s, Skin::None);
        for s in Skin::ALL {
            assert_eq!(Skin::from_name(s.name()), Some(s));
        }
    }

    #[test]
    fn the_theme_token_is_read_with_or_without_quotes() {
        assert_eq!(Skin::from_theme_source("mod.wm_theme = {\n  skin: \"holo\"\n}"), Some(Skin::Holo));
        assert_eq!(Skin::from_theme_source("a: 1\n  skin: float,\n"), Some(Skin::Float));
        assert_eq!(Skin::from_theme_source("skin: \"nope\""), None);
        assert_eq!(Skin::from_theme_source("accent: #fff"), None);
    }

    #[test]
    fn none_leaves_the_styles_metrics_alone() {
        assert_eq!(Skin::None.inset(2.0), 2.0);
        assert_eq!(Skin::None.title_height(34.0), 34.0);
        assert_eq!(Skin::Float.title_height(34.0), 0.0);
    }

    #[test]
    fn the_controls_sit_in_the_title_strip_in_order() {
        let r = rect(100.0, 50.0, 600.0, 400.0);
        let l = layout(Skin::Holo, r, 2.0, 22.0, false, 0.0).unwrap();
        assert_eq!(l.grab, rect(102.0, 52.0, 596.0, 22.0));
        assert!(l.controls[0].pos.x < l.controls[1].pos.x && l.controls[1].pos.x < l.controls[2].pos.x);
        assert_eq!(l.controls[2].pos.x + l.controls[2].size.x, 698.0 - 15.0 + 15.0);
        for c in l.controls {
            assert!(c.pos.y == 52.0 && c.size.y == 22.0);
        }
        // The glyph centres agree with the hit rects.
        let first = l.controls[0].pos.x + l.controls[0].size.x * 0.5 - r.pos.x;
        assert_eq!(l.btn[0] as f64, first);
    }

    #[test]
    fn the_float_handle_grows_under_the_pointer() {
        let r = rect(0.0, 100.0, 800.0, 500.0);
        let small = layout(Skin::Float, r, 0.0, 0.0, false, 0.0).unwrap();
        let big = layout(Skin::Float, r, 0.0, 0.0, true, 1.0).unwrap();
        assert!(big.pill.size.x > small.pill.size.x && big.grab.contains(small.grab.pos));
        assert_eq!(small.controls[0].size.x, 0.0);
        assert!(big.controls[2].size.x > 0.0 && big.label.size.x > 0.0);
        assert!(big.pill.pos.y + big.pill.size.y <= 0.0 && big.pill.pos.x + big.pill.size.x <= r.size.x);
        assert!(layout(Skin::Float, rect(0.0, 0.0, 100.0, 50.0), 0.0, 0.0, true, 1.0).is_none());
    }
}
