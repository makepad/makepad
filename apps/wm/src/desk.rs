//! The desk: shared WM state, the animated tile container and its menu
//! overlay. Split from main.rs so the desk's script_mod! block can live in
//! its own module (one script_mod! per module).
//!
//! The look is Omarchy's, read from its source: gaps 5/10, a 2px square
//! border (focused = the theme's gradient along its angle, unfocused = a
//! grey wash), and hyprland's animation curves — windows easeOutQuint over
//! 379ms, border color over 539ms, a new window popping in from 87% over
//! 410ms, a closing one popping back out over 149ms while it fades.

use makepad_widgets::app_icon::AppIconDraw;
pub(crate) mod phone;
use phone::{DrawPhoneApp, PhoneFrame};
use crate::mobile_surface::PhoneSurface;
use crate::dock_warp::{DockWarp, DrawDockWarp, WindowFrame};
use std::collections::{HashMap, HashSet};

use makepad_widgets::*;
use makepad_widgets::{backdrop::BackdropCompositor, gauss_view::GaussRoundedView};

use crate::clients::ClientSlot;
use crate::hub::ClientId;
use crate::layout::{LRect, WmLayout};
use crate::module_view::MpModuleView;
use crate::run_view::{MpRunView, MpRunViewAction};
use crate::tile::TileHost;
use crate::theme;
use crate::desktop::{DrawDesktopChrome, StyleTween};
use crate::shell::ui::ShellDraw;

#[allow(unused_imports)]
use crate::layout;

// ======================================================================
// Geometry (omarchy default/hypr/looknfeel.lua)
// ======================================================================

/// Gap around each window's own box. Two neighbours therefore sit
/// `2 * GAPS_IN` apart — 10px, the same as the outer gap.
pub const GAPS_IN: f64 = 5.0;
/// Gap from the desk edge (screen / bar) to a window box.
pub const GAPS_OUT: f64 = 10.0;
/// The gap the layout tree splits with: the visible distance between two
/// tiles, both halves of GAPS_IN.
pub const TILE_GAP: f64 = GAPS_IN * 2.0;
/// Border thickness, drawn just inside the window box (hard corners).
pub const BORDER_SIZE: f64 = 2.0;
/// The tab strip a grouped tile wears, just inside its border — hyprland's
/// `group:groupbar:height`. It comes off the CHILD's rect, not the tile's,
/// so the strip never covers the window it is labelling.
pub const GROUPBAR_H: f64 = 20.0;
/// Side padding inside one tab, before its (middle-elided) title.
const TAB_PAD: f64 = 6.0;
/// The gutter between two tabs: one column of the strip's own base color.
const TAB_GUTTER: f64 = 1.0;

// ======================================================================
// Animation (omarchy default/hypr/looknfeel.lua, speeds are ds = 100ms)
// ======================================================================

/// windows 3.79 easeOutQuint — move + resize.
const DUR_WINDOWS: f64 = 0.379;
/// windowsIn 4.1 easeOutQuint, style popin 87%.
const DUR_WINDOWS_IN: f64 = 0.41;
/// windowsOut 1.49 linear, style popin 87%.
const DUR_WINDOWS_OUT: f64 = 0.149;
/// fadeIn 1.73 almostLinear.
const DUR_FADE_IN: f64 = 0.173;
/// fadeOut 1.46 almostLinear.
const DUR_FADE_OUT: f64 = 0.146;
/// popin 87%: the scale a window opens from and closes to.
const POPIN_SCALE: f64 = 0.87;

/// A cubic bezier from (0,0) to (1,1) with hyprland's two control points,
/// evaluated as y(x): x is solved for the curve parameter first, exactly
/// like a CSS timing function.
pub fn bezier(p1x: f64, p1y: f64, p2x: f64, p2y: f64, x: f64) -> f64 {
    // The ends are exact: bisection would leave a rounding crumb there and
    // an animation must land ON its target, not a hair short of it.
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let curve = |a: f64, b: f64, u: f64| {
        let v = 1.0 - u;
        3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
    };
    // Bisection: monotone in u for the curves we use, and 24 halvings put
    // the solution well inside a pixel of any animation we drive with it.
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if curve(p1x, p2x, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    curve(p1y, p2y, 0.5 * (lo + hi))
}

/// hyprland's easeOutQuint bezier (0.23, 1, 0.32, 1).
pub fn ease_out_quint(t: f64) -> f64 {
    bezier(0.23, 1.0, 0.32, 1.0, t)
}

/// hyprland's almostLinear bezier (0.5, 0.5, 0.75, 1.0).
pub fn almost_linear(t: f64) -> f64 {
    bezier(0.5, 0.5, 0.75, 1.0, t)
}

fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn lerp_rect(a: LRect, b: LRect, t: f64) -> LRect {
    LRect::new(
        lerp(a.x, b.x, t),
        lerp(a.y, b.y, t),
        lerp(a.w, b.w, t),
        lerp(a.h, b.h, t),
    )
}

fn lerp_color(a: Vec4f, b: Vec4f, t: f64) -> Vec4f {
    let t = t as f32;
    Vec4f {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        z: a.z + (b.z - a.z) * t,
        w: a.w + (b.w - a.w) * t,
    }
}

/// Snap position and size independently to physical pixels. Translating a
/// window must never change its framebuffer dimensions.
fn snap_to_device(r: Rect, dpi: f64) -> Rect {
    let dpi = dpi.max(1.0);
    let snap = |v: f64| (v * dpi).round() / dpi;
    Rect {
        pos: dvec2(snap(r.pos.x), snap(r.pos.y)),
        size: dvec2(snap(r.size.x).max(1.0 / dpi), snap(r.size.y).max(1.0 / dpi)),
    }
}

/// The chrome and app use the same physical-pixel grid.
fn snap_child_rect(r: Rect, dpi: f64) -> Rect {
    snap_to_device(r, dpi)
}

fn fade_color(c: Vec4f, alpha: f64) -> Vec4f {
    Vec4f {
        w: c.w * alpha as f32,
        ..c
    }
}

/// The starting wash's alpha while the arriving content fades in over it.
///
/// The naive complement (`glass * (1 - arrival)`) FLICKERS: the two layers
/// stack, so the wallpaper still visible through the pair is
/// `(1 - wash) * (1 - arrival)`, which at `arrival = 0.5` is 0.28 against
/// 0.12 at either end — the tile blooms BRIGHTER halfway through the
/// crossfade than it is before or after, and over a bright wallpaper that
/// reads as a flash (measured: a terminal opening went 33 → 42 → 35 mean
/// luma, a browser 33 → 37 → 17).
///
/// Coverage must stay put instead. The content lands over the wash, so the
/// pair covers `arrival + wash * (1 - arrival)`; holding that at `glass`
/// gives `wash = glass * (1 - arrival) / (1 - glass * arrival)` — the wash
/// retreats exactly as fast as the content fills in, never faster. It still
/// starts at `glass` and still reaches 0 at `arrival = 1`, so the ends are
/// unchanged; only the middle stops leaking.
fn wash_alpha(glass: f32, arrival: f32) -> f32 {
    let arrival = arrival.clamp(0.0, 1.0);
    let glass = glass.clamp(0.0, 1.0);
    let denom = 1.0 - glass * arrival;
    if denom <= 1.0e-4 {
        return 0.0;
    }
    (glass * (1.0 - arrival) / denom).clamp(0.0, 1.0)
}

/// Split a tile's interior into (tab strip, child). A grouped leaf gives
/// `GROUPBAR_H` at the top to the strip and the rest to the child; anything
/// else hands the whole interior back. Capped at a third of the tile so a
/// very short tile still shows some of its window.
fn split_groupbar(inner: Rect, grouped: bool) -> (Option<Rect>, Rect) {
    let h = if grouped {
        GROUPBAR_H.min((inner.size.y / 3.0).floor())
    } else {
        0.0
    };
    if h < 1.0 {
        return (None, inner);
    }
    (
        Some(Rect {
            pos: inner.pos,
            size: dvec2(inner.size.x, h),
        }),
        Rect {
            pos: dvec2(inner.pos.x, inner.pos.y + h),
            size: dvec2(inner.size.x, inner.size.y - h),
        },
    )
}

/// Tab `i` of `n`, laid out in equal widths across a strip `w` wide.
/// Computed from the two edges rather than from a width so rounding can
/// never open a seam: tab i ends exactly where tab i+1 starts.
fn tab_span(w: f64, i: usize, n: usize) -> (f64, f64) {
    let n = n.max(1) as f64;
    let x0 = (w * i as f64 / n).floor();
    let x1 = (w * (i + 1) as f64 / n).floor();
    (x0, x1.max(x0))
}

/// `keep` characters of `chars`, head and tail, around one ellipsis. The
/// head gets the odd character — a title reads from the left.
fn join_middle(chars: &[char], keep: usize) -> String {
    let keep = keep.min(chars.len());
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('\u{2026}');
    out.extend(chars[chars.len() - tail..].iter());
    out
}

/// Has a press that started on a tab travelled far enough to stop being a
/// tab click and become a tear-out drag?
fn tab_press_escaped(start: Vec2d, abs: Vec2d) -> bool {
    (abs.x - start.x).abs() >= TAB_TEAR_THRESHOLD || (abs.y - start.y).abs() >= TAB_TEAR_THRESHOLD
}

/// Middle ellipsis: "terminal — ~/makepad/apps" keeps both the app and where
/// it is, which a right-elide would throw away. `width` measures a
/// candidate, so the choice of cut is testable without a font.
fn elide_middle(s: &str, max_w: f64, mut width: impl FnMut(&str) -> f64) -> String {
    if s.is_empty() || width(s) <= max_w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    // Binary search the largest surviving character count that still fits.
    let (mut lo, mut hi, mut best) = (0usize, chars.len().saturating_sub(1), None);
    while lo <= hi {
        let mid = (lo + hi) / 2;
        let candidate = join_middle(&chars, mid);
        if width(&candidate) <= max_w {
            best = Some(candidate);
            lo = mid + 1;
        } else if mid == 0 {
            break;
        } else {
            hi = mid - 1;
        }
    }
    best.unwrap_or_default()
}

/// The back-to-front draw order `WmDesk::draw_walk`'s three loops compose:
/// closing tiles, then everything else `rects()` returned that is not a
/// preview, then previews on top. Pure (no `Cx`) so the composition itself
/// is unit-testable; `zorder` stores the result and `handle_event`
/// dispatches its reverse. `previews` stays a whole client list rather
/// than a predicate so a client that is both closing AND (implausibly) a
/// preview still ends up counted once, in the preview group.
fn compose_zorder(closing: &[ClientId], rest: &[ClientId], previews: &[ClientId]) -> Vec<ClientId> {
    closing
        .iter()
        .copied()
        .chain(rest.iter().copied().filter(|c| !previews.contains(c)))
        .chain(previews.iter().copied())
        .collect()
}

// ======================================================================
// Shared WM state (App owns it; WmDesk reads it through Scope)
// ======================================================================

/// The window border colors, resolved from the theme.splash source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BorderTheme {
    /// Focused: the gradient's first stop.
    pub active: Vec4f,
    /// Focused: the second stop (equal to `active` for a solid border).
    pub active_end: Vec4f,
    /// Gradient angle in degrees, hyprland's screen-space convention:
    /// 0 = left→right, 90 = top→bottom (45 therefore runs down-right,
    /// which is what the reference desktop shows).
    pub angle: f32,
    /// Unfocused: one grey wash over the wallpaper.
    pub inactive: Vec4f,
}

impl BorderTheme {
    fn stop(s: theme::Stop) -> Vec4f {
        Vec4f {
            x: s.rgb.r as f32 / 255.0,
            y: s.rgb.g as f32 / 255.0,
            z: s.rgb.b as f32 / 255.0,
            w: s.alpha as f32,
        }
    }

    /// Read the border keys out of a theme.splash source.
    pub fn from_theme_source(source: &str) -> Self {
        let (active, inactive) = theme::scan_borders(source);
        Self {
            active: Self::stop(active.start),
            active_end: Self::stop(active.end),
            angle: active.angle as f32,
            inactive: Self::stop(inactive),
        }
    }
}

impl Default for BorderTheme {
    fn default() -> Self {
        Self::from_theme_source("")
    }
}

pub struct WmState {
    pub snap: crate::snap::SnapState,
    pub phone: crate::mobile::PhoneState,
    pub style: StyleTween,
    pub dock_backdrop: Option<gauss_view::GaussBlurSnapshot>,
    pub layout: WmLayout,
    pub clients: HashMap<ClientId, ClientSlot>,
    pub hub_port: u16,
    pub theme_name: String,
    pub term_env: String,
    /// The theme accent — menu highlights, not the window border.
    pub accent: Vec4f,
    pub borders: BorderTheme,
    /// The gap the layout splits with: tile to tile (2 * gaps_in).
    pub gap: f64,
    /// The gap from the desk edge to the outermost tiles (gaps_out).
    pub gaps_out: f64,
    /// The clients a drag is currently moving/resizing — one for a
    /// SUPER+drag, both sides of the split for a divider drag in the gap.
    /// `WmDesk` reads this to skip the tile's normal 379ms layout tween
    /// for exactly those clients: Hyprland moves a drag 1:1 with the pointer —
    /// the tween is for LAYOUT changes (a new window, a workspace switch),
    /// not for a drag, which already delivers a new rect every frame and
    /// would otherwise restart the tween's `move_t` each time, so the
    /// drawn quad chases eased-and-lagging positions it never quite
    /// reaches — a size that "wobbles" by a device pixel as the two ends
    /// of that lag round independently, and a child texture sampled at a
    /// quad size that keeps missing the swapchain's true resolution
    /// (blur/shimmer). See `WmDesk::draw_walk`'s sync loop.
    pub dragging: Vec<ClientId>,
    /// The tile a SHIFT-drag is hovering, so the desk can paint its ring in
    /// the accent: dropping here makes the dragged window a TAB of this
    /// tile instead of swapping the two.
    pub drop_hint: Option<ClientId>,
    /// The AI pane is mid-slide: the desk's rect narrows or widens every
    /// frame, and every tile snaps to the layout like a dragged one — a
    /// tween on top of the slide would lag it (see `dragging`).
    pub pane_sliding: bool,
}

impl WmState {
    pub fn focused_terminal_cwd(&self) -> Option<std::path::PathBuf> {
        let focus = self.layout.focused_client()?;
        let slot = self.clients.get(&focus)?;
        if slot.app == "terminal" {
            slot.pwd.clone()
        } else {
            None
        }
    }
}

// ======================================================================
// The desk: animated tiles hosting run views
// ======================================================================

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    // The stops and the angle come from the theme through BorderTheme and
    // are set per tile in Rust (they animate with focus), so the shader's
    // own defaults stay plain literals — a theme writing `45` instead of
    // `45.0` would otherwise type the instance as an int and the shader
    // would not compile.
    /** The tile focus border: a hard square ring carrying the two-stop
     * gradient (Hyprland's axis convention), fed per tile from BorderTheme
     * and animated with focus. */
    set_type_default() do #(DrawTileBorder::script_shader(vm)) {
        ..mod.draw.DrawQuad
        /** gradient start color */
        color: #7aa2f7
        /** gradient end color */
        color_end: #7aa2f7
        /** gradient axis in degrees clockwise 0..360 step 15 */
        angle: 0.0
        /** ring thickness in pixels 1..8 step 0.5 */
        border_size: 2.0
        // A hard square ring, measured straight off the quad edges. NOT an
        // Sdf2d box + stroke: with radius 0 that box has no interior
        // distance (it saturates at 0), so the stroke floods the whole
        // tile — which only showed wherever the child did not cover it.
        pixel: fn() {
            let p = self.pos * self.rect_size
            let bs = self.border_size
            let d = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
            let cov = clamp((bs - d) * /**edge sharpness 1..8 step 0.5*/ 3.0 + 0.5, 0.0, 1.0)
            // Hyprland's gradient axis: degrees clockwise in screen space,
            // 0 = left→right, 90 = top→bottom (45 runs down-right).
            let rad = self.angle * 0.017453292
            let dir = vec2(cos(rad), sin(rad))
            let half = self.rect_size * 0.5
            let extent = max(abs(dir.x) * half.x + abs(dir.y) * half.y, 0.001)
            let t = clamp(0.5 + dot(p - half, dir) / (2.0 * extent), 0.0, 1.0)
            let c = mix(self.color, self.color_end, t)
            let a = c.w * cov
            return vec4(c.rgb * a, a)
        }
    }

    // The panel a tile shows before its child has a frame: translucent,
    // so the wallpaper reads through exactly like the running window.
    set_type_default() do #(DrawTilePanel::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: mod.wm_theme.background
        alpha: 0.88
        pixel: fn() {
            return vec4(self.color.rgb * self.alpha, self.alpha)
        }
    }

    set_type_default() do #(DrawMenuBg::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: mod.wm_theme.background
        border_color: mod.wm_theme.active_border
        pixel: fn() {
            let p = self.pos * self.rect_size
            let d = min(min(p.x, p.y), min(self.rect_size.x - p.x, self.rect_size.y - p.y))
            let line = clamp((1.0 - d) * 3.0 + 0.5, 0.0, 1.0)
            let c = mix(vec4(self.color.rgb, 0.98), vec4(self.border_color.rgb, 1.0), line)
            return vec4(c.rgb * c.w, c.w)
        }
    }

    // A selected menu row: foreground at 0.08, hard corners.
    set_type_default() do #(DrawMenuRow::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: mod.wm_theme.foreground
        pixel: fn() {
            return vec4(self.color.rgb * self.color.w, self.color.w)
        }
    }

    // One tab of a grouped tile's strip: a flat premultiplied fill. The
    // color is set per tab in Rust (active = the theme's active border,
    // inactive = the grey wash at a lower alpha), so this default is just
    // a placeholder.
    set_type_default() do #(DrawGroupTab::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: mod.wm_theme.darker_background
        pixel: fn() {
            return vec4(self.color.rgb * self.color.w, self.color.w)
        }
    }

    // A hairline under the search field.
    set_type_default() do #(DrawMenuLine::script_shader(vm)) {
        ..mod.draw.DrawQuad
        color: mod.wm_theme.lighter_background
        pixel: fn() {
            return vec4(self.color.rgb * self.color.w, self.color.w)
        }
    }

    mod.widgets.WmDeskBase = #(WmDesk::register_widget(vm))
    mod.widgets.WmDesk = set_type_default() do mod.widgets.WmDeskBase {
        width: Fill
        height: Fill
        draw_shadow +: {
            opacity: uniform(0.25)
            pixel: fn() {
                let p = self.pos*self.rect_size
                let sdf = Sdf2d.viewport(p)
                sdf.box(24.0,24.0,self.rect_size.x-48.0,self.rect_size.y-48.0,5.0)
                let outside = smoothstep(-0.5,0.5,sdf.shape)
                let shadow = GaussShadow.rounded_box_shadow(vec2(24.0,30.0),self.rect_size-vec2(24.0,18.0),p,8.0,10.0)
                return vec4(0.0,0.0,0.0,shadow*outside*self.opacity)
            }
        }
        terminal_glass: GaussRoundedView {
            width: Fill height: Fill show_bg: true
            draw_bg +: {
                blur_level: 3.0
                corner_radius: 0.0
                tint_alpha: 0.0 surface_alpha: 1.0
                lensing_strength: 0.0 specular_strength: 0.0 noise_strength: 0.0
                border_width: 0.0 border_alpha: 0.0 shadow_radius: 0.0
                shadow_color: #0000
            }
        }
        menu_text_color: mod.wm_theme.foreground
        menu_dim_color: mod.wm_theme.dark_foreground
        // The active tab is filled with the active border color, so its
        // label is written in the background color to stay readable.
        tab_fg_active: mod.wm_theme.background
        tab_fg_inactive: mod.wm_theme.foreground
        tab_strip_color: mod.wm_theme.darker_background
        chrome +: {}
        draw_warp +: {}
        phone_ui +: {}
        draw_phone +: {}
        shell_draw +: {
            text.text_style: theme.font_regular
            text_bold.text_style: theme.font_bold
        }
        draw_border +: {}
        draw_panel +: {}
        draw_tab +: {}
        draw_tab_text +: {
            text_style: theme.font_code
            text_style.font_size: 8.5
            color: mod.wm_theme.foreground
        }
        draw_menu_bg +: {}
        draw_menu_row +: {}
        draw_menu_line +: {}
        draw_menu_text +: {
            text_style: theme.font_code
            text_style.font_size: 9.5
            color: mod.wm_theme.foreground
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawTileBorder {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    color_end: Vec4f,
    #[live(45.0)]
    angle: f32,
    #[live(2.0)]
    border_size: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawTilePanel {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live(0.55)]
    alpha: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawGroupTab {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawMenuBg {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
    #[live]
    border_color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawMenuRow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
struct DrawMenuLine {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    color: Vec4f,
}

/// Per-tile animation state. The quad tweens toward the layout target on
/// hyprland's curves; the child is always configured at the target
/// (resize-sync design), so only the presented quad moves.
struct TileAnim {
    /// Where the current tween started and where it lands.
    from: LRect,
    target: LRect,
    /// Progress 0..1 of the move/resize tween.
    move_t: f64,
    /// The drawn rect (from/target at ease_out_quint(move_t)).
    cur: LRect,
    /// Opening: 0..1 popin from 87% + fade in.
    open_t: f64,
    /// Closing: 0..1 popin back to 87% + fade out, then the tile goes.
    close_t: Option<f64>,
    /// Border color: 0 = inactive, 1 = focused.
    focus: f64,
    focus_target: f64,
}

impl TileAnim {
    fn new(rect: LRect, focused: bool) -> Self {
        Self {
            from: rect,
            target: rect,
            move_t: 1.0,
            cur: rect,
            open_t: 0.0,
            close_t: None,
            focus: if focused { 1.0 } else { 0.0 },
            focus_target: if focused { 1.0 } else { 0.0 },
        }
    }

    /// A tile that must appear ALREADY THERE. A group tab switch reveals a
    /// window that has been running behind the strip all along, so it gets
    /// none of the new-window popin: full size, full opacity, first frame.
    /// (`new` leaves `move_t` finished already; only the open channel has
    /// to be wound forward.)
    fn settled(rect: LRect, focused: bool) -> Self {
        Self {
            open_t: 1.0,
            ..Self::new(rect, focused)
        }
    }

    fn retarget(&mut self, target: LRect) {
        if self.target == target {
            return;
        }
        self.from = self.cur;
        self.target = target;
        self.move_t = 0.0;
    }

    /// Move straight to `target`, no tween — a SUPER+drag's own per-frame
    /// rect update. `retarget` restarted every drag frame would otherwise
    /// leave `cur` perpetually chasing a moving target through
    /// `ease_out_quint`, never quite arriving; this is Hyprland's 1:1 drag
    /// instead of the layout tween.
    fn snap_to(&mut self, target: LRect) {
        self.from = target;
        self.target = target;
        self.cur = target;
        self.move_t = 1.0;
    }

    /// Advance every channel; true while anything is still moving.
    fn step(&mut self, dt: f64) -> bool {
        let mut busy = false;
        if self.move_t < 1.0 {
            self.move_t = (self.move_t + dt / DUR_WINDOWS).min(1.0);
            self.cur = lerp_rect(self.from, self.target, ease_out_quint(self.move_t));
            busy = true;
        } else {
            self.cur = self.target;
        }
        if self.open_t < 1.0 && self.close_t.is_none() {
            self.open_t = (self.open_t + dt / DUR_WINDOWS_IN).min(1.0);
            busy = true;
        }
        if let Some(close_t) = self.close_t.as_mut() {
            *close_t = (*close_t + dt / DUR_WINDOWS_OUT).min(1.0);
            busy = true;
        }
        // The focus ring SNAPS. Omarchy crossfades it over 539ms
        // (looknfeel.lua:75 `border speed 5.39`), but a focus rect that
        // takes half a second to catch up reads as lag, so this one is a
        // deliberate deviation: instant, like the menu selection.
        self.focus = self.focus_target;
        busy
    }

    /// Scale + alpha of the popin, opening or closing.
    fn popin(&self) -> (f64, f64) {
        if let Some(close_t) = self.close_t {
            // Hyprland's popin close, verbatim (WindowAnimationController
            // applyPopin + renderSnapshot): the FROZEN snapshot stretched
            // into a box shrinking to 87% centered, 149ms LINEAR, while
            // fadeOut runs 146ms almostLinear — the fade dominates, the
            // pop stays subtle. The freeze is what makes it read clean.
            let scale = lerp(1.0, POPIN_SCALE, close_t);
            let fade = 1.0 - almost_linear((close_t * DUR_WINDOWS_OUT / DUR_FADE_OUT).min(1.0));
            (scale, fade)
        } else if self.open_t < 1.0 {
            let scale = lerp(POPIN_SCALE, 1.0, ease_out_quint(self.open_t));
            let fade = almost_linear((self.open_t * DUR_WINDOWS_IN / DUR_FADE_IN).min(1.0));
            (scale, fade)
        } else {
            (1.0, 1.0)
        }
    }

    fn done_closing(&self) -> bool {
        self.close_t.map(|t| t >= 1.0).unwrap_or(false)
    }
}

/// The tab strip one grouped tile wears: every member's title in group
/// order, and which of them the tile is currently showing. Rebuilt from
/// `WmLayout::groups` every frame — the desk never owns group state.
struct GroupTabs {
    members: Vec<(ClientId, String)>,
    active: usize,
}

/// How far a press on a tab has to travel before it stops being a tab
/// click and becomes a tear-out drag. Generous next to the 3px window
/// drag threshold: a click on a tab must never accidentally rip it out.
const TAB_TEAR_THRESHOLD: f64 = 10.0;

/// What the desk asks the WM to do with a group tab.
#[derive(Clone, Debug, Default)]
pub enum WmDeskAction {
    /// A press on a tab dragged off the strip: tear that member out of its
    /// group and carry on as an ordinary tiled drag from `abs`.
    TearOutTab { client: ClientId, abs: Vec2d },
    #[default]
    None,
}

/// A press that landed on a tab and has not yet moved far enough to be a
/// tear-out. Released in place, it was only a tab click.
struct PendingTabDrag {
    member: ClientId,
    start: Vec2d,
}

/// One tab as it was last drawn, kept for the next `handle_event`. The
/// strip is the desk's own chrome, so it hit-tests itself instead of
/// letting the hosted child claim the press.
struct TabHit {
    rect: Rect,
    /// The member whose window the tile is showing — the group's identity
    /// as far as the layout is concerned.
    visible: ClientId,
    /// The member this tab selects, and its index inside the group.
    member: ClientId,
    index: usize,
}

#[derive(Script, Widget)]
pub struct WmDesk {
    #[live] phone_ui: PhoneSurface,
    #[live] draw_phone: DrawPhoneApp,
    #[rust] phone_frames: HashMap<ClientId, PhoneFrame>,
    #[rust] desktop_frames: HashMap<ClientId, WindowFrame>,
    #[rust] pub wallpaper: WidgetRef,
    #[rust] compositor: Option<BackdropCompositor>,
    #[rust] composing: bool,
    #[rust] terminal_clients: HashSet<ClientId>,
    #[rust] blur_counts: (usize, usize),
    #[live] terminal_glass: GaussRoundedView,
    #[live] chrome: DrawDesktopChrome,
    #[live] draw_shadow: DrawQuad,
    #[live] shell_draw: ShellDraw,
    #[rust] style: StyleTween,
    #[rust] titles: HashMap<ClientId, String>,
    #[rust] window_apps: HashMap<ClientId, String>,
    #[rust] startup_sheet: Option<desktop_style::StyleSheet>,
    #[rust] app_icons: AppIconDraw,
    #[rust] title_hits: Vec<(ClientId, Rect, ChromeHit)>,
    #[rust] chrome_hover: Option<(ClientId, ChromeHit)>,
    #[rust] chrome_pressed: Option<(ClientId, ChromeHit)>,
    #[rust] minimized: HashSet<ClientId>,
    #[rust] dock_warps: HashMap<ClientId, DockWarp>,
    #[live] draw_warp: DrawDockWarp,
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
    draw_border: DrawTileBorder,
    #[live]
    draw_panel: DrawTilePanel,
    #[live]
    draw_tab: DrawGroupTab,
    #[live]
    draw_tab_text: DrawText,
    #[live]
    draw_menu_bg: DrawMenuBg,
    #[live]
    draw_menu_row: DrawMenuRow,
    #[live]
    draw_menu_line: DrawMenuLine,
    #[live]
    draw_menu_text: DrawText,
    #[live]
    menu_text_color: Vec4f,
    #[live]
    menu_dim_color: Vec4f,
    #[live]
    tab_fg_active: Vec4f,
    #[live]
    tab_fg_inactive: Vec4f,
    #[live]
    tab_strip_color: Vec4f,
    #[rust]
    templates: HashMap<LiveId, ScriptObjectRef>,
    #[rust]
    items: HashMap<ClientId, WidgetRef>,
    /// Clients hosted IN-PROCESS: their tile is the `ModuleTile` template
    /// (module_view.rs), not a process run view. Marked by the WM before
    /// the tile is first asked for.
    #[rust]
    module_clients: HashSet<ClientId>,
    #[rust]
    anims: HashMap<ClientId, TileAnim>,
    /// The last frame's draw order, back to front (closing tiles, then the
    /// live tiles/floats/scratchpad, then preview floats on top). Reused
    /// by `handle_event` so hit dispatch mirrors the visual stack — see
    /// the note there.
    #[rust]
    zorder: Vec<ClientId>,
    /// Grouped tiles this frame, keyed by the member the tile is showing.
    #[rust]
    group_tabs: HashMap<ClientId, GroupTabs>,
    /// Every tab `draw_tile` drew, in screen coordinates.
    #[rust]
    tab_hits: Vec<TabHit>,
    /// Last frame's group memberships. A tile that appears this frame and
    /// was a member of one of them was REVEALED by a tab switch, not
    /// opened — see `TileAnim::settled`.
    #[rust]
    prev_group_members: Vec<Vec<ClientId>>,
    /// A press on a tab, still undecided between a click and a tear-out.
    #[rust]
    pending_tab_drag: Option<PendingTabDrag>,
    /// The tile a SHIFT-drag is hovering: its border is drawn in the accent
    /// as the "drop me in here as a tab" hint.
    #[rust]
    hint: Option<ClientId>,
    /// The theme accent, for that hint.
    #[rust]
    accent: Vec4f,
    #[rust]
    area: Area,
    #[rust]
    pub desk_rect: Rect,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_anim_time: f64,
    #[rust]
    animating: bool,
}

impl ScriptHook for WmDesk {
    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        apply: &Apply,
        _scope: &mut Scope,
        value: ScriptValue,
    ) {
        if !apply.is_eval() {
            if let Some(obj) = value.as_object() {
                vm.vec_with(obj, |vm, vec| {
                    for kv in vec {
                        if let Some(id) = kv.key.as_id() {
                            if let Some(template_obj) = kv.value.as_object() {
                                self.templates
                                    .insert(id, vm.bx.heap.new_object_ref(template_obj));
                            }
                        }
                    }
                });
            }
        }
    }
}

impl WmDesk {
    pub fn set_startup_style(&mut self, cx: &mut Cx, sheet: &desktop_style::StyleSheet) {
        self.startup_sheet = Some(sheet.clone());
        for item in self.items.values() {
            if let Some(mut view) = item.borrow_mut::<MpRunView>() {view.set_startup_style(cx, sheet);}
        }
    }
    pub fn chrome_pointer(&mut self, cx: &mut Cx, p: Option<Vec2d>) -> Option<(ClientId, ChromeHit)> {
        let hit = p.and_then(|p| self.chrome_hit(p));
        let hover = hit.filter(|(_, h)| matches!(h, ChromeHit::Close | ChromeHit::Minimize | ChromeHit::Maximize));
        if self.chrome_hover != hover {
            self.chrome_hover = hover;
            cx.redraw_area(self.area);
        }
        hit
    }
    pub fn press_chrome(&mut self, cx: &mut Cx, hit: (ClientId, ChromeHit)) {
        self.chrome_pressed = Some(hit);
        cx.redraw_area(self.area);
    }
    pub fn release_chrome(&mut self, cx: &mut Cx) -> Option<(ClientId, ChromeHit)> {
        let pressed = self.chrome_pressed.take();
        if pressed.is_some() { cx.redraw_area(self.area); }
        pressed
    }
    fn item(&mut self, cx: &mut Cx, client: ClientId) -> Option<WidgetRef> {
        if let Some(item) = self.items.get(&client) {
            if let Some(app) = self.window_apps.get(&client) {
                if let Some(mut view) = item.borrow_mut::<MpRunView>() {
                    view.set_startup_app(app);
                }
            }
            return Some(item.clone());
        }
        let template_id = if self.module_clients.contains(&client) {
            live_id!(ModuleTile)
        } else {
            live_id!(Tile)
        };
        let template_ref = self.templates.get(&template_id)?;
        let template_value: ScriptValue = template_ref.as_object().into();
        let vm_id = cx.script_ref_vm_id(template_ref)?;
        let widget_ref =
            cx.with_script_vm_id(vm_id, |vm| WidgetRef::script_from_value(vm, template_value));
        cx.widget_tree_insert_child(self.uid, LiveId(client), widget_ref.clone());
        if let Some(mut view) = widget_ref.borrow_mut::<MpRunView>() {
            if let Some(sheet) = &self.startup_sheet {view.set_startup_style(cx, sheet);}
            if let Some(app) = self.window_apps.get(&client) {view.set_startup_app(app);}
        }
        self.items.insert(client, widget_ref.clone());
        Some(widget_ref)
    }

    pub fn with_run_view<R>(
        &mut self,
        cx: &mut Cx,
        client: ClientId,
        f: impl FnOnce(&mut Cx, &mut MpRunView) -> R,
    ) -> Option<R> {
        let item = self.item(cx, client)?;
        let mut view = item.borrow_mut::<MpRunView>()?;
        Some(f(cx, &mut view))
    }

    /// The next tile asked for `client` is a module tile.
    pub fn mark_module(&mut self, client: ClientId) {
        self.module_clients.insert(client);
    }

    /// The tile as its host trait, whichever kind it is (tile.rs).
    pub fn with_tile<R>(
        &mut self,
        cx: &mut Cx,
        client: ClientId,
        f: impl FnOnce(&mut Cx, &mut dyn TileHost) -> R,
    ) -> Option<R> {
        let item = self.item(cx, client)?;
        if let Some(mut view) = item.borrow_mut::<MpRunView>() {
            return Some(f(cx, &mut *view as &mut dyn TileHost));
        }
        if let Some(mut view) = item.borrow_mut::<MpModuleView>() {
            return Some(f(cx, &mut *view as &mut dyn TileHost));
        }
        None
    }

    pub fn with_module_view<R>(
        &mut self,
        cx: &mut Cx,
        client: ClientId,
        f: impl FnOnce(&mut Cx, &mut MpModuleView) -> R,
    ) -> Option<R> {
        let item = self.item(cx, client)?;
        let mut view = item.borrow_mut::<MpModuleView>()?;
        Some(f(cx, &mut view))
    }

    /// Start the close animation: the tile keeps its last frame while it
    /// pops back to 87% and fades, and is dropped when that finishes.
    pub fn remove_client(&mut self, client: ClientId) {
        match self.anims.get_mut(&client) {
            Some(anim) if anim.close_t.is_none() => {
                anim.close_t = Some(0.0);
                anim.focus_target = 0.0;
            }
            Some(_) => {}
            None => {
                self.items.remove(&client);
                self.module_clients.remove(&client);
            }
        }
    }

    /// Drop whatever finished its close animation.
    fn reap_closed(&mut self) {
        let done: Vec<ClientId> = self
            .anims
            .iter()
            .filter(|(_, a)| a.done_closing())
            .map(|(c, _)| *c)
            .collect();
        for client in done {
            self.anims.remove(&client);
            self.items.remove(&client);
            self.module_clients.remove(&client);
        }
    }

    fn lrect_to_rect(r: LRect) -> Rect {
        Rect {
            pos: dvec2(r.x, r.y),
            size: dvec2(r.w, r.h),
        }
    }

    /// Step every tile animation; true while anything is still moving.
    fn step_anims(&mut self, dt: f64) -> bool {
        let mut moving = false;
        for anim in self.anims.values_mut() {
            moving |= anim.step(dt);
        }
        moving
    }

    /// Draw one tile: the border ring (nothing else — the wallpaper stays
    /// visible behind the child, which composites itself at the window
    /// opacity), plus the child at the ring's inset.
    fn draw_tile(
        &mut self,
        cx: &mut Cx2d,
        scope: &mut Scope,
        client: ClientId,
        borders: &BorderTheme,
    ) {
        let Some((cur, target, focus, (scale, fade), closing)) = self
            .anims
            .get(&client)
            .map(|a| (a.cur, a.target, a.focus, a.popin(), a.close_t.is_some()))
        else {
            return;
        };
        if let Some(warp) = self.dock_warps.get_mut(&client) {
            if warp.minimized && !warp.active() { return; }
            if let Some(frame) = warp.frame.as_mut().filter(|_| !warp.refresh) {
                frame.freeze(cx);
                if !frame.frozen() && self.composing { self.compositor.as_mut().unwrap().content_pass(frame.pass_id()); }
                warp.draw(cx, &mut self.draw_warp);
                if self.composing { self.compositor.as_mut().unwrap().content(warp.bounds()); }
                return;
            }
        }
        let fade = if self.dock_warps.contains_key(&client) { 1.0 } else if self.minimized.contains(&client) {
            let t = self.anims.get(&client).map(|a| a.move_t).unwrap_or(1.0);
            if t >= 1.0 { return; }
            fade * (1.0 - ease_out_quint(t))
        } else { fade };
        let mut draw_rect = self.dock_warps.get(&client).map(|w|w.source).unwrap_or(Self::lrect_to_rect(cur));
        // The rect BEFORE the popin scale: while closing, the frozen frame
        // stays pinned to this and the shrinking quad only CROPS it.
        let unscaled_rect = draw_rect;
        if scale < 1.0 && !self.dock_warps.contains_key(&client) {
            let center = draw_rect.pos + draw_rect.size * 0.5;
            draw_rect.size *= scale;
            draw_rect.pos = center - draw_rect.size * 0.5;
        }
        // The ring's edges land on device pixels, so a 2px border is
        // exactly 2px on every side whatever the tween left behind.
        let dpi = cx.current_dpi_factor().max(1.0);
        draw_rect = snap_to_device(draw_rect, dpi);

        let inset = BORDER_SIZE * (self.style.weights[0] + self.style.weights[3] + self.style.weights[4]);
        let mut inner = snap_child_rect(
            Rect {
                pos: draw_rect.pos + dvec2(inset, inset),
                size: dvec2(
                    (draw_rect.size.x - inset * 2.0).max(1.0),
                    (draw_rect.size.y - inset * 2.0).max(1.0),
                ),
            },
            dpi,
        );

        let (has_frame, arrival) = self
            .items
            .get(&client)
            .and_then(|item| with_tile_host(item, |v| (v.has_frame(), v.arrival_fade())))
            .unwrap_or((false, 1.0));
        let startup_glass = self.style.target == crate::desktop::DesktopStyle::Macos && (!has_frame || arrival < 1.0);
        let backdrop = if self.composing && (self.terminal_clients.contains(&client) || startup_glass) {
            Some(self.compositor.as_mut().unwrap().backdrop(cx, inner, 3.0))
        } else {None};
        let rounding = self.style.value([0.0, 14.0, 8.0, 0.0, 0.0]);
        self.draw_window_shadow(cx, draw_rect, focus, fade);
        let mut capture = self.dock_warps.get_mut(&client)
            .filter(|w| w.frame.is_none() || w.refresh)
            .map(|w| {
                w.refresh = false;
                w.frame.take().unwrap_or_else(|| WindowFrame::new(cx))
            }).or_else(|| (rounding > 0.01).then(|| self.desktop_frames.remove(&client)
                .unwrap_or_else(|| WindowFrame::new_with_name(cx, "wm_window_surface"))));
        if let Some(frame) = &mut capture { frame.begin(cx, draw_rect); }
        if let Some(snapshot) = backdrop {
            self.terminal_glass.draw_surface_with_backdrop(cx, inner, Some(snapshot), fade as f32);
        }
        let title_h = self.style.title_height().min((inner.size.y * 0.3).max(0.0));
        if title_h > 0.1 {
            self.draw_window_chrome(cx, client, draw_rect, title_h, focus, fade);
            inner.pos.y += title_h;
            inner.size.y = (inner.size.y-title_h).max(1.0);
        }

        // A translucent panel only while the child has nothing to show —
        // once it has a frame the desk paints nothing behind it.
        // The dark starting wash CROSSFADES with the arriving content:
        // full while no frame exists, then fading out exactly as the
        // first frames fade in — the tile's darkness stays continuous,
        // never a bright wallpaper flash between the two.
        // The wash IS the terminal glass (same color, same focus
        // opacity), so a terminal's first frame changes nothing but
        // the prompt appearing. (Omarchy shows no placeholder at all
        // — windows map only with their first buffer — this is our
        // equivalent for cargo-launched children.)
        let glass = if focus > 0.5 { 0.88 } else { 0.84 };
        let wash = wash_alpha(glass, if has_frame { arrival } else { 0.0 });
        if wash > 0.004 && !startup_glass {
            self.draw_panel.alpha = wash * fade as f32;
            self.draw_panel.draw_abs(cx, inner);
        }

        // A SHIFT-drag hovering this tile paints its ring in the accent
        // instead of the focus gradient: the drop will make the dragged
        // window a tab HERE, and the ring is where that will show.
        let (ring_start, ring_end) = if self.hint == Some(client) {
            (self.accent, self.accent)
        } else {
            (
                lerp_color(borders.inactive, borders.active, focus),
                lerp_color(borders.inactive, borders.active_end, focus),
            )
        };
        self.draw_border.color = fade_color(ring_start, fade);
        self.draw_border.color_end = fade_color(ring_end, fade);
        self.draw_border.angle = borders.angle;
        self.draw_border.border_size = BORDER_SIZE as f32;
        if self.style.weights[0] > 0.001 {
            self.draw_border.color.w *= self.style.weights[0] as f32;
            self.draw_border.color_end.w *= self.style.weights[0] as f32;
            self.draw_border.draw_abs(cx, draw_rect);
        }

        // A grouped leaf gives the top of its interior to the tab strip;
        // the child gets what is left, at both the drawn and the settled
        // size, so its swapchain is never sized as if the strip were not
        // there.
        let grouped = self.group_tabs.contains_key(&client);
        let (strip, child_rect) = split_groupbar(inner, grouped);
        let child_rect = if strip.is_some() {
            snap_child_rect(child_rect, dpi)
        } else {
            child_rect
        };
        if let Some(strip) = strip {
            self.draw_group_tabs(cx, client, strip, borders, fade);
        }

        // The child is configured at the SETTLED size (resize-sync), snapped
        // the same way so its swapchain matches the rect it will be drawn at.
        let settled = snap_to_device(Self::lrect_to_rect(target), dpi);
        let mut settled_inner = snap_child_rect(
            Rect {
                pos: settled.pos + dvec2(inset, inset),
                size: dvec2(
                    (settled.size.x - inset * 2.0).max(1.0),
                    (settled.size.y - inset * 2.0).max(1.0),
                ),
            },
            dpi,
        );
        let title_h = self.style.target.title_height().min(settled_inner.size.y * 0.3);
        settled_inner.pos.y += title_h;
        settled_inner.size.y = (settled_inner.size.y-title_h).max(1.0);
        let (_, settled_child) = split_groupbar(settled_inner, grouped);
        let settled_child = if grouped {
            snap_child_rect(settled_child, dpi)
        } else {
            settled_child
        };
        // Hyprland stretches the frozen snapshot into the shrinking box —
        // no crop (that experiment read odd; git has it).
        let _ = (closing, unscaled_rect);
        if let Some(item) = self.item(cx, client) {
            with_tile_host(&item, |view| {
                // Minimize only changes the compositor quad. Reconfiguring the
                // app to the taskbar target would destroy its responsive layout.
                if !self.minimized.contains(&client) {
                    view.set_target_size(Some(settled_child.size));
                }
                view.set_close_crop(None);
                view.set_fade(fade as f32);
            });
            item.draw_walk_all(cx, scope, Walk::abs_rect(child_rect));
        }
        if let Some(mut frame) = capture {
            frame.end(cx);
            if self.composing {self.compositor.as_mut().unwrap().content_pass(frame.pass_id());}
            if let Some(warp) = self.dock_warps.get_mut(&client) {
                warp.frame=Some(frame);
                warp.draw(cx, &mut self.draw_warp);
                if self.composing {self.compositor.as_mut().unwrap().content(warp.bounds());}
            } else {
                self.draw_window_surface(cx, &frame, draw_rect, rounding as f32);
                self.desktop_frames.insert(client, frame);
                if self.composing { self.compositor.as_mut().unwrap().content(draw_rect); }
            }
            return;
        }
        if self.composing { self.compositor.as_mut().unwrap().content(Rect {pos: draw_rect.pos-dvec2(24.0,24.0),size: draw_rect.size+dvec2(48.0,48.0)}); }
    }

    /// The group's tab strip: one equal-width tab per member, the active
    /// one filled with the theme's active border color and labelled in the
    /// background color, the rest a dimmed wash of the inactive border.
    /// Every tab is recorded in `tab_hits` so the next click can find it.
    fn draw_group_tabs(
        &mut self,
        cx: &mut Cx2d,
        client: ClientId,
        strip: Rect,
        borders: &BorderTheme,
        fade: f64,
    ) {
        let Some(tabs) = self.group_tabs.get(&client) else {
            return;
        };
        let members = tabs.members.clone();
        let active = tabs.active;
        if members.is_empty() {
            return;
        }

        // A base under the tabs: the gutters between them, and whatever a
        // low-alpha inactive tab lets through, read as chrome instead of
        // as wallpaper.
        self.draw_tab.color = fade_color(
            Vec4f {
                w: 0.92,
                ..self.tab_strip_color
            },
            fade,
        );
        self.draw_tab.draw_abs(cx, strip);

        let active_fill = fade_color(borders.active, fade);
        let inactive_fill = fade_color(
            Vec4f {
                w: borders.inactive.w * 0.45,
                ..borders.inactive
            },
            fade,
        );
        let fg_active = fade_color(self.tab_fg_active, fade);
        let fg_inactive = fade_color(self.tab_fg_inactive, fade);

        for (i, (member, title)) in members.iter().enumerate() {
            let (x0, x1) = tab_span(strip.size.x, i, members.len());
            let hit = Rect {
                pos: dvec2(strip.pos.x + x0, strip.pos.y),
                size: dvec2(x1 - x0, strip.size.y),
            };
            let is_active = i == active;
            self.draw_tab.color = if is_active {
                active_fill
            } else {
                inactive_fill
            };
            self.draw_tab.draw_abs(
                cx,
                Rect {
                    pos: hit.pos,
                    size: dvec2((hit.size.x - TAB_GUTTER).max(1.0), hit.size.y),
                },
            );

            let text_w = hit.size.x - TAB_GUTTER - TAB_PAD * 2.0;
            if text_w > 1.0 {
                let (label, label_w) = self.tab_label(cx, title, text_w);
                let px = self.draw_tab_text.text_style.font_size as f64 / 0.75;
                let pos = dvec2(
                    (hit.pos.x + TAB_PAD + (text_w - label_w) * 0.5).floor(),
                    (hit.pos.y + (hit.size.y - px * 1.2) * 0.5).floor(),
                );
                self.draw_tab_text.color = if is_active { fg_active } else { fg_inactive };
                self.draw_tab_text.draw_abs(cx, pos, &label);
            }

            self.tab_hits.push(TabHit {
                rect: hit,
                visible: client,
                member: *member,
                index: i,
            });
        }
    }

    /// A title cut to fit one tab, with the width it actually measured.
    fn tab_label(&mut self, cx: &mut Cx2d, title: &str, max_w: f64) -> (String, f64) {
        let face = &mut self.draw_tab_text;
        let mut measure = |s: &str| {
            face.prepare_single_line_run(cx, s)
                .map(|r| r.width_in_lpxs as f64)
                .unwrap_or(0.0)
        };
        let label = elide_middle(title, max_w, &mut measure);
        let width = measure(&label);
        (label, width)
    }

    /// A press inside a group tab belongs to the strip, never to the child
    /// below it: it makes that member the group's active one and hands the
    /// focus move to the WM over the same action a tile-body click raises.
    fn hit_group_tab(&mut self, cx: &mut Cx, scope: &mut Scope, abs: Vec2d) -> bool {
        let Some((visible, member, index)) = self
            .tab_hits
            .iter()
            .find(|h| h.rect.contains(abs))
            .map(|h| (h.visible, h.member, h.index))
        else {
            return false;
        };
        if member != visible {
            if let Some(state) = scope.data.get_mut::<WmState>() {
                // `group_set_active` acts on the FOCUSED leaf, so focus the
                // group's visible member first — the click may well have
                // landed on a strip belonging to some other tile.
                state.layout.set_focus(visible);
                state.layout.group_set_active(index + 1);
            }
        }
        // The same press may still turn into a tear-out; until it travels
        // TAB_TEAR_THRESHOLD it was only a tab click, which has already
        // happened above.
        self.pending_tab_drag = Some(PendingTabDrag { member, start: abs });
        cx.widget_action(self.uid, MpRunViewAction::Clicked { client: member });
        self.redraw(cx);
        true
    }

    /// A pending tab press that has travelled far enough: the member is
    /// torn out of its group and the WM turns the press into an ordinary
    /// tiled drag. Below the threshold nothing happens, so a click that
    /// wobbles a pixel or two still just switches tabs.
    fn tab_drag_escaped(&mut self, cx: &mut Cx, abs: Vec2d) -> bool {
        let Some(pending) = self.pending_tab_drag.as_ref() else {
            return false;
        };
        if !tab_press_escaped(pending.start, abs) {
            return false;
        }
        let client = pending.member;
        self.pending_tab_drag = None;
        cx.widget_action(self.uid, WmDeskAction::TearOutTab { client, abs });
        self.redraw(cx);
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChromeHit { Title, Close, Minimize, Maximize, Resize(i8,i8) }
impl WmDesk {
    pub fn window_at(&self, p: Vec2d) -> Option<ClientId> {
        self.zorder.iter().rev().find(|c| !self.minimized.contains(c) && !self.dock_warps.get(c).is_some_and(DockWarp::active) && self.anims.get(c).is_some_and(|a| Self::lrect_to_rect(a.cur).contains(p))).copied()
    }
    pub fn chrome_hit(&self, p: Vec2d) -> Option<(ClientId, ChromeHit)> {
        let top = self.window_at(p)?;
        self.title_hits.iter().rev().find(|(c,r,_)| *c==top && r.contains(p)).map(|(c,_,h)| (*c,*h))
    }
    pub fn chrome_button_rect(&self, client: ClientId, hit: ChromeHit) -> Option<Rect> {
        self.title_hits.iter().rev().find(|(c,_,h)|*c==client && *h==hit).map(|(_,r,_)|*r)
    }
    fn draw_window_shadow(&mut self, cx: &mut Cx2d, r: Rect, focus: f64, fade: f64) {
        use crate::shell::ui::rect;
        let t=&self.style;
        if t.weights[1] + t.weights[2] > 0.001 {
            self.draw_shadow.draw_vars.set_uniform(cx, live_id!(opacity), &[((t.weights[1]+t.weights[2])*fade*if focus>0.5 {0.28}else{0.16}) as f32]);
            self.draw_shadow.draw_abs(cx,rect(r.pos.x-24.0,r.pos.y-24.0,r.size.x+48.0,r.size.y+48.0));
        }
    }
    fn draw_window_chrome(&mut self, cx: &mut Cx2d, client: ClientId, r: Rect, h: f64, focus: f64, fade: f64) {
        use crate::desktop::DesktopStyle;
        use crate::shell::{rgb, alpha, ui::{rect,Ico,HAlign}};
        let t=&self.style;
        let opacity = ((1.0-t.weights[0])*fade) as f32;
        let classic = t.weights[3] as f32;
        let next = t.weights[4] as f32;
        let retro = classic + next;
        self.chrome.top_only=0.0;self.chrome.title_gradient=0.0;
        self.chrome.radius=t.value([0.0,10.0,8.0,0.0,0.0]) as f32;
        self.chrome.bevel=retro;
        self.chrome.color=alpha(if t.dark && t.target.supports_dark() {rgb(40,40,42)}else{rgb(212,208,200)},opacity);
        self.chrome.frame_width=2.0;
        self.chrome.color.w *= retro;
        self.chrome.draw_abs(cx,r);
        self.chrome.frame_width=0.0;
        let edge=2.0*(t.weights[3]+t.weights[4]);
        let title=rect(r.pos.x+edge,r.pos.y+edge,r.size.x-edge*2.0,h);
        self.chrome.top_only=1.0;
        self.chrome.title_gradient=classic * focus as f32;
        // The compositor masks the complete window, including its content.
        // The title fills that surface without an independently inset edge.
        self.chrome.radius=0.0;
        self.chrome.bevel=0.0;
        self.chrome.color=lerp_color(alpha(if t.dark && t.target.supports_dark() {rgb(48,48,51)}else{rgb(237,237,240)},opacity),alpha(if focus>0.5 {rgb(0,0,128)}else{rgb(128,128,128)},opacity),classic as f64);
        self.chrome.color=lerp_color(self.chrome.color,alpha(if focus>0.5 {rgb(0,0,0)}else{rgb(85,85,85)},opacity),next as f64);
        self.chrome.draw_abs(cx,title);
        self.title_hits.push((client,title,ChromeHit::Title));
        let ink=alpha(if retro>0.5 || (t.dark && t.target.supports_dark()) {rgb(255,255,255)}else{rgb(30,30,34)},opacity);
        let text=self.titles.get(&client).cloned().unwrap_or_default();
        let inset=if t.target==DesktopStyle::Macos {86.0}else if t.target==DesktopStyle::Windows {38.0}else{28.0};
        if t.target != DesktopStyle::Macos && t.target != DesktopStyle::NextStep {
            let app = self.window_apps.get(&client).map(String::as_str).unwrap_or("app");
            let size = if t.target == DesktopStyle::Windows { 24.0 } else { 16.0 };
            self.app_icons.draw(cx, app, t.target, rect(title.pos.x+7.0,title.pos.y+(h-size)*0.5,size,size),opacity,ink);
        }
        let text_rect=rect(title.pos.x+inset,title.pos.y,(title.size.x-inset-if matches!(t.target,DesktopStyle::Macos|DesktopStyle::NextStep) {inset}else{142.0}).max(1.0),h);
        self.shell_draw.label_elided(cx,text_rect,retro>0.5,12.0,ink,if matches!(t.target,DesktopStyle::Macos|DesktopStyle::NextStep) {HAlign::Center}else{HAlign::Left},&text);
        self.chrome.top_only=0.0;self.chrome.title_gradient=0.0;
        let mac=t.weights[1];
        for (i,hit,ico,color) in [(0,ChromeHit::Close,Ico::Close,rgb(255,95,86)),(1,ChromeHit::Minimize,Ico::WindowMin,rgb(255,189,46)),(2,ChromeHit::Maximize,Ico::WindowMax,rgb(39,201,63))] {
            if t.target == DesktopStyle::NextStep && hit == ChromeHit::Maximize { continue; }
            let slot=if i==0 {0}else if i==1 {2}else{1};
            let width=t.value([30.0,30.0,46.0,18.0,22.0]);
            let right=title.pos.x+title.size.x-width-2.0-(slot as f64)*(width+2.0);
            let left=title.pos.x+10.0+(i as f64)*22.0;
            let bw=width+(18.0-width)*mac;
            let modern = t.target == DesktopStyle::Windows;
            let button=if t.target==DesktopStyle::NextStep {rect(if hit==ChromeHit::Minimize {title.pos.x+2.0}else{title.pos.x+title.size.x-24.0},title.pos.y+2.0,22.0,(h-4.0).max(1.0))}else if modern {rect(title.pos.x+title.size.x-(slot as f64+1.0)*width,title.pos.y,width,h)}else{rect(right+(left-right)*mac,title.pos.y+3.0,bw,(h-6.0).max(1.0))};
            let hovered = self.chrome_hover == Some((client, hit));
            let pressed = hovered && self.chrome_pressed == Some((client, hit));
            self.chrome.radius=(mac*12.0) as f32;
            self.chrome.bevel=retro;
            self.chrome.color=lerp_color(alpha(rgb(212,208,200),opacity*classic),alpha(color,opacity),mac);
            if next>0.01 {self.chrome.color=alpha(if pressed {rgb(128,128,128)}else{rgb(170,170,170)},opacity);}
            if modern && hovered {
                self.chrome.color = alpha(if hit == ChromeHit::Close {
                    if pressed {rgb(196,43,28)}else{rgb(232,17,35)}
                } else if t.dark {
                    if pressed {rgb(82,82,86)}else{rgb(64,64,68)}
                } else if pressed {rgb(196,196,200)}else{rgb(218,218,222)},opacity);
            }
            let face=if mac>0.99 {rect(button.pos.x+2.0,button.pos.y+(button.size.y-12.0)*0.5,12.0,12.0)}else{button};
            self.chrome.draw_abs(cx,face);
            if mac<0.99 { self.shell_draw.icon_centered(cx,ico,button,11.0,alpha(if (modern && hovered && hit == ChromeHit::Close) || (t.dark && t.target.supports_dark()) {rgb(255,255,255)}else{rgb(20,20,20)},opacity*(1.0-mac) as f32)); }
            if mac>0.99 && self.chrome_hover.is_some_and(|(c,_)| c==client) {
                self.shell_draw.icon_centered(cx,ico,face,6.0,alpha(rgb(64,44,32),opacity*0.8));
            }
            self.title_hits.push((client,button,hit));
        }
        for (edge,x,y) in [
            (rect(r.pos.x,r.pos.y+12.0,5.0,(r.size.y-24.0).max(0.0)),-1,0),
            (rect(r.pos.x+r.size.x-5.0,r.pos.y+12.0,5.0,(r.size.y-24.0).max(0.0)),1,0),
            (rect(r.pos.x+12.0,r.pos.y,(r.size.x-24.0).max(0.0),4.0),0,-1),
            (rect(r.pos.x+12.0,r.pos.y+r.size.y-5.0,(r.size.x-24.0).max(0.0),5.0),0,1),
            (rect(r.pos.x,r.pos.y,8.0,8.0),-1,-1),
            (rect(r.pos.x+r.size.x-8.0,r.pos.y,8.0,8.0),1,-1),
            (rect(r.pos.x,r.pos.y+r.size.y-12.0,12.0,12.0),-1,1),
            (rect(r.pos.x+r.size.x-12.0,r.pos.y+r.size.y-12.0,12.0,12.0),1,1),
        ] {self.title_hits.push((client,edge,ChromeHit::Resize(x,y)));}
    }
}

impl Widget for WmDesk {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let mut rect = cx.turtle().rect();
        self.desk_rect = rect;

        // A run view can be created by a client message before its first
        // layout. Keep its launch identity current in both desktop and phone
        // modes so the compiling surface uses the application's real icon.
        if let Some(state) = scope.data.get_mut::<WmState>() {
            self.window_apps.retain(|client, _| state.clients.contains_key(client));
            for (client, slot) in &state.clients {
                if self.window_apps.get(client) != Some(&slot.app) {
                    self.window_apps.insert(*client, slot.app.clone());
                }
            }
        }

        if scope.data.get_mut::<WmState>().is_some_and(|s|s.style.target.mobile()) {
            self.draw_phone_scene(cx,scope,rect);
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        }

        let Some(state) = scope.data.get_mut::<WmState>() else {
            cx.end_turtle_with_area(&mut self.area);
            return DrawStep::done();
        };

        state.dock_backdrop = None;
        self.terminal_clients = state.clients.iter().filter(|(_,s)| s.app == "terminal" && !matches!(state.style.target, crate::desktop::DesktopStyle::Windows2000 | crate::desktop::DesktopStyle::NextStep)).map(|(c,_)| *c).collect();
        if state.style.active() || self.style.active()
            || self.style.target != state.style.target || self.style.dark != state.style.dark {
            // Child process frames arrive asynchronously during the crossfade.
            // Refresh through its final frame instead of freezing the first,
            // potentially old appearance. Hidden windows defer this until restore.
            for warp in self.dock_warps.values_mut() { warp.refresh = true; }
        }
        self.style = state.style.clone();
        self.titles = state.clients.iter().map(|(c,s)| (*c,s.display_title().to_string())).collect();
        self.title_hits.clear();
        rect.size.y = (rect.size.y - self.style.reserved_height()).max(1.0);
        let gap = state.gap;
        let gaps_out = state.gaps_out;
        let area = LRect::new(
            rect.pos.x + gaps_out,
            rect.pos.y + gaps_out,
            (rect.size.x - gaps_out * 2.0).max(1.0),
            (rect.size.y - gaps_out * 2.0).max(1.0),
        );
        // `rects` already hands floats (and the scratchpad) back after the
        // tiled windows, so drawing in order puts them on top.
        let mut targets = state.layout.rects(area, gap);
        let was_minimized = self.minimized.clone();
        self.minimized = state.layout.clients_on(state.layout.active).into_iter().filter(|c| state.layout.desktop.minimized(*c)).collect();
        if self.style.target == crate::desktop::DesktopStyle::Macos {
            let size=cx.owning_window_or_root_pass_size();
            for client in state.layout.clients_on(state.layout.active) {
                let hidden=self.minimized.contains(&client);
                let restored=was_minimized.contains(&client) && !hidden;
                if (hidden && !was_minimized.contains(&client)) || restored {
                    if !self.dock_warps.contains_key(&client) {
                        let source=targets.iter().find(|(c,_)|*c==client).map(|(_,r)|Self::lrect_to_rect(*r))
                            .or_else(||self.anims.get(&client).map(|a|Self::lrect_to_rect(a.cur)));
                        if let (Some(source),Some(slot))=(source,state.clients.get(&client)) {
                            self.dock_warps.insert(client,DockWarp::new(source,crate::desktop::dock_icon_bounds(state,size,&slot.app),restored));
                        }
                    }
                    if let Some(warp)=self.dock_warps.get_mut(&client) {warp.minimized=hidden;}
                }
                if let Some(warp)=self.dock_warps.get_mut(&client) {
                    if let Some(slot)=state.clients.get(&client) {warp.dock=crate::desktop::dock_icon_bounds(state,size,&slot.app);}
                }
            }
            self.dock_warps.retain(|client,warp|state.clients.contains_key(client) && (warp.minimized || warp.active()));
        } else { self.dock_warps.clear(); }
        for client in &self.minimized {
            if let Some(warp)=self.dock_warps.get(client) {
                let r=warp.source;
                targets.push((*client,LRect::new(r.pos.x,r.pos.y,r.size.x,r.size.y)));
            } else if self.anims.contains_key(client) {
                targets.push((*client,LRect::new(rect.pos.x+rect.size.x*0.5,rect.pos.y+rect.size.y+12.0,48.0,34.0)));
            }
        }
        let focused = state.layout.focused_client();
        self.accent = state.accent;
        self.hint = state.drop_hint;
        let borders = state.borders;
        let dragging = state.dragging.clone();
        let pane_sliding = state.pane_sliding;
        // A preview float dims everything behind it (Quick Look).
        let previews: Vec<ClientId> = targets
            .iter()
            .map(|(c, _)| *c)
            .filter(|c| {
                state
                    .clients
                    .get(c)
                    .map(|slot| slot.is_preview)
                    .unwrap_or(false)
            })
            .collect();

        // Which tiles wear a tab strip this frame, and what the tabs say.
        // Read straight out of the layout every frame — the desk keeps no
        // group state of its own — and keyed by the member the tile shows,
        // which is the client `rects` handed back for that slot.
        self.group_tabs.clear();
        self.tab_hits.clear();
        let prev_groups = std::mem::take(&mut self.prev_group_members);
        for group in state.layout.groups(area, gap).into_iter().filter(|_| !state.layout.desktop.enabled) {
            let Some(visible) = group.clients.get(group.active).copied() else {
                continue;
            };
            // A fullscreen window is exactly the one place a strip must not
            // steal a row: it is showing one member, full bleed.
            let drawn = targets.iter().any(|(c, _)| *c == visible);
            if !drawn || state.layout.is_client_fullscreen(visible) {
                continue;
            }
            let members = group
                .clients
                .iter()
                .map(|c| {
                    let title = state
                        .clients
                        .get(c)
                        .map(|slot| slot.display_title().to_string())
                        .unwrap_or_default();
                    (*c, title)
                })
                .collect();
            self.prev_group_members.push(group.clients.clone());
            self.group_tabs.insert(
                visible,
                GroupTabs {
                    members,
                    active: group.active,
                },
            );
        }

        // Sync animation targets. Tiles that vanished from the layout
        // without a close animation (workspace switch) go immediately.
        let live: Vec<ClientId> = targets.iter().map(|(c, _)| *c).collect();
        for (client, target) in &targets {
            match self.anims.get_mut(client) {
                Some(anim) => {
                    anim.close_t = None;
                    if was_minimized.contains(client) && !self.minimized.contains(client) && !self.dock_warps.contains_key(client) { anim.open_t = 0.0; }
                    if dragging.contains(client) || pane_sliding {
                        anim.snap_to(*target);
                    } else {
                        anim.retarget(*target);
                    }
                    anim.focus_target = if Some(*client) == focused { 1.0 } else { 0.0 };
                }
                None => {
                    // A tile the layout only now hands back either OPENED
                    // (popin from 87%, fade in) or was REVEALED by a group
                    // tab switch — the same window that was behind the
                    // strip a frame ago. Revealing must be instant: a
                    // pulse there reads as the window reloading. A member
                    // of one of last frame's groups is a reveal; anything
                    // else is genuinely new.
                    let focus = Some(*client) == focused;
                    let revealed = prev_groups.iter().any(|m| m.contains(client));
                    let anim = if revealed {
                        TileAnim::settled(*target, focus)
                    } else {
                        TileAnim::new(*target, focus)
                    };
                    self.anims.insert(*client, anim);
                }
            }
        }
        // Tiles on other workspaces keep their widget (and their child's
        // swapchain) but lose their animation state — they pop back in
        // when the workspace returns.
        self.anims
            .retain(|client, anim| live.contains(client) || anim.close_t.is_some());
        self.desktop_frames.retain(|client, _| self.anims.contains_key(client));

        // Closing tiles paint under the live ones.
        let closing: Vec<ClientId> = self
            .anims
            .iter()
            .filter(|(c, a)| a.close_t.is_some() && !live.contains(c))
            .map(|(c, _)| *c)
            .collect();
        // The exact back-to-front order the three loops below draw in,
        // kept for `handle_event`: `Event::hits` claims a positional event
        // FIRST-WINS (see finger.rs), so dispatch has to offer it to
        // widgets topmost-first or a float sitting over a tile loses the
        // click to whichever the HashMap iterates first instead of
        // winning it.
        self.zorder = compose_zorder(
            &closing,
            &targets.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
            &previews,
        );
        self.composing = self.style.weights[1] > 0.001 || self.zorder.iter().any(|c| self.terminal_clients.contains(c));
        if self.composing {
            self.compositor.get_or_insert_with(|| backdrop::BackdropCompositor::new(cx)).begin(cx);
            let bounds = self.wallpaper.area().rect(cx);
            if let Some(mut wallpaper) = self.wallpaper.borrow_mut::<View>() { wallpaper.draw_cached_texture(cx, bounds); }
        }
        for client in closing {
            self.draw_tile(cx, scope, client, &borders);
        }
        for (client, _) in &targets {
            if previews.contains(client) {
                continue;
            }
            self.draw_tile(cx, scope, *client, &borders);
        }
        // Then the scrim and the previews floating over it.
        if !previews.is_empty() {
            self.draw_panel.alpha = 0.5;
            self.draw_panel.draw_abs(cx, rect);
            if self.composing { self.compositor.as_mut().unwrap().content(rect); }
            for client in &previews {
                self.draw_tile(cx, scope, *client, &borders);
            }
        }

        if self.composing {
            // The dock samples the final window stack. When no window reaches
            // its blur footprint it can share an earlier terminal checkpoint.
            let size = cx.owning_window_or_root_pass_size();
            let dock = if self.style.weights[1] > 0.001 {
                scope.data.get_mut::<WmState>().map(|state| (crate::desktop::dock_bounds(state, size), 4.5))
            } else { None };
            let (snapshot, stacks, passes) = self.compositor.as_mut().unwrap().finish(cx, self.desk_rect, dock);
            if self.blur_counts != (stacks, passes) {
                self.blur_counts = (stacks, passes);
                if std::env::var_os("MAKEPAD_WM_TRACE_BLUR").is_some() { log!("wm: backdrop stacks={} passes={}", stacks, passes); }
            }
            if let Some(state) = scope.data.get_mut::<WmState>() { state.dock_backdrop = snapshot; }
        }

        let animating = self.dock_warps.values().any(DockWarp::active) || self.anims.values().any(|a| {
            a.move_t < 1.0
                || a.open_t < 1.0
                || a.close_t.is_some()
                || (a.focus - a.focus_target).abs() > 0.001
        });
        if animating {
            if !self.animating {
                self.last_anim_time = 0.0;
            }
            self.animating = true;
            self.next_frame = cx.new_next_frame();
        } else {
            self.animating = false;
        }

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        if scope.data.get_mut::<WmState>().is_some_and(|s|s.style.target.mobile()) {
            self.handle_phone_event(cx,event,scope);
            return;
        }
        if let Some(ne) = self.next_frame.is_event(event) {
            if self.animating {
                // A first frame after idle has no meaningful delta.
                let dt = if self.last_anim_time <= 0.0 {
                    1.0 / 60.0
                } else {
                    (ne.time - self.last_anim_time).clamp(0.001, 0.05)
                };
                self.last_anim_time = ne.time;
                self.step_anims(dt);
                for warp in self.dock_warps.values_mut() {warp.step(dt);}
                self.reap_closed();
                self.next_frame = cx.new_next_frame();
                cx.redraw_area(self.area);
            }
        }
        // The group strips are the desk's own chrome, drawn above every
        // tile: a press on one is taken here, before any tile is offered
        // the event. The child underneath is drawn BELOW the strip, so it
        // would never have claimed the press anyway — this only makes the
        // ordering explicit and stops the tab click doubling as a click
        // into whatever window happens to sit there.
        match event {
            Event::MouseDown(e) => {
                if self.hit_group_tab(cx, scope, e.abs) {
                    return;
                }
            }
            // A tab press that walks off the strip tears its window out of
            // the group; the WM takes the press over from there.
            Event::MouseMove(e) if self.pending_tab_drag.is_some() => {
                if self.tab_drag_escaped(cx, e.abs) {
                    return;
                }
            }
            Event::MouseUp(_) => self.pending_tab_drag = None,
            _ => {}
        }
        // Forward to tiles, TOPMOST FIRST. `Event::hits` (finger.rs) is a
        // first-wins claim: the first widget whose hit test succeeds marks
        // the event handled and every later `.hits()` call on it returns
        // `Hit::Nothing`. Offering the event in HashMap order (arbitrary,
        // unrelated to the visual stack) let a background tile claim a
        // click that landed on a float drawn over it — the float lost
        // input to whatever `self.items` happened to iterate first. Going
        // in reverse `zorder` (the frame's own back-to-front draw order)
        // makes the actually-topmost widget claim it instead; anything not
        // in `zorder` (a hidden workspace's tiles) is offered last, same
        // as before.
        let input=matches!(event,Event::MouseDown(_) | Event::MouseUp(_) | Event::MouseMove(_) | Event::Scroll(_) | Event::KeyDown(_) | Event::KeyUp(_) | Event::TextInput(_));
        let mut items: Vec<WidgetRef> = self
            .zorder
            .iter()
            .rev()
            .filter(|c| !input || (!self.minimized.contains(c) && !self.dock_warps.get(c).is_some_and(DockWarp::active)))
            .filter_map(|c| self.items.get(c).cloned())
            .collect();
        for (client, item) in &self.items {
            if !input && !self.zorder.contains(client) {
                items.push(item.clone());
            }
        }
        // Scroll is NOT a captured hit: every area containing the point
        // receives it, so a wheel over a float would also scroll the tile
        // underneath. Deliver a scroll only to the TOPMOST window under
        // the pointer.
        if let Event::Scroll(e) = event {
            let top = self
                .zorder
                .iter()
                .rev()
                .find(|c| {
                    !self.minimized.contains(c) && self.anims
                        .get(c)
                        .map(|a| Self::lrect_to_rect(a.cur).contains(e.abs))
                        .unwrap_or(false)
                })
                .copied();
            if let Some(client) = top {
                if let Some(item) = self.items.get(&client).cloned() {
                    item.handle_event(cx, event, scope);
                }
            }
            return;
        }
        for item in items {
            item.handle_event(cx, event, scope);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A monospace stand-in for a real font: every character is one unit
    /// wide, so a width is just a character count.
    fn mono(s: &str) -> f64 {
        s.chars().count() as f64
    }

    #[test]
    fn the_wash_starts_at_the_glass_and_ends_gone() {
        for glass in [0.84f32, 0.88] {
            assert!((wash_alpha(glass, 0.0) - glass).abs() < 1.0e-6);
            assert_eq!(wash_alpha(glass, 1.0), 0.0);
        }
    }

    #[test]
    fn the_wash_never_lets_more_wallpaper_through_than_the_glass_does() {
        // The flicker this replaces: with `glass * (1 - arrival)` the pair
        // uncovers 0.28 of the wallpaper mid-crossfade against 0.12 at the
        // ends. Coverage must never dip below the glass while the content
        // is opaque, at any point of the fade.
        for glass in [0.84f32, 0.88] {
            let mut prev = f32::INFINITY;
            for step in 0..=200 {
                let arrival = step as f32 / 200.0;
                let covered = arrival + wash_alpha(glass, arrival) * (1.0 - arrival);
                assert!(
                    covered >= glass - 1.0e-4,
                    "glass {glass} arrival {arrival}: covered {covered}"
                );
                // ...and it only ever grows, so nothing brightens on the way.
                if prev.is_finite() {
                    assert!(covered >= prev - 1.0e-4, "arrival {arrival} went backwards");
                }
                prev = covered;
            }
            assert!((prev - 1.0).abs() < 1.0e-4);
        }
    }

    #[test]
    fn the_wash_retreats_monotonically() {
        let mut prev = 1.0f32;
        for step in 0..=100 {
            let wash = wash_alpha(0.88, step as f32 / 100.0);
            assert!(wash <= prev + 1.0e-6, "step {step}: {wash} > {prev}");
            prev = wash;
        }
    }

    #[test]
    fn the_groupbar_comes_off_the_child_not_the_tile() {
        let inner = Rect {
            pos: dvec2(10.0, 20.0),
            size: dvec2(400.0, 300.0),
        };
        // Not a group: the child keeps the whole interior.
        let (strip, child) = split_groupbar(inner, false);
        assert!(strip.is_none());
        assert_eq!(child, inner);

        let (strip, child) = split_groupbar(inner, true);
        let strip = strip.expect("a grouped leaf wears a strip");
        assert_eq!(strip.pos, inner.pos);
        assert_eq!(strip.size.x, inner.size.x);
        assert_eq!(strip.size.y, GROUPBAR_H);
        // The child starts under the strip and the two exactly tile the
        // interior — no row is drawn twice and none is lost.
        assert_eq!(child.pos.x, inner.pos.x);
        assert_eq!(child.pos.y, inner.pos.y + GROUPBAR_H);
        assert_eq!(child.size.x, inner.size.x);
        assert_eq!(strip.size.y + child.size.y, inner.size.y);
    }

    #[test]
    fn a_short_tile_keeps_most_of_its_window() {
        // A third of the tile is the cap; below 3px there is no strip at
        // all rather than a one-pixel smear.
        let short = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(200.0, 30.0),
        };
        let (strip, child) = split_groupbar(short, true);
        assert_eq!(strip.expect("still a strip").size.y, 10.0);
        assert_eq!(child.size.y, 20.0);

        let tiny = Rect {
            pos: dvec2(0.0, 0.0),
            size: dvec2(200.0, 2.0),
        };
        let (strip, child) = split_groupbar(tiny, true);
        assert!(strip.is_none());
        assert_eq!(child, tiny);
    }

    #[test]
    fn tabs_are_equal_width_and_leave_no_seam() {
        // 101 is deliberately indivisible by 3: the spans must still meet.
        for (w, n) in [(300.0, 3), (101.0, 3), (7.0, 5), (400.0, 1)] {
            let mut prev_end = 0.0;
            let mut widths = Vec::new();
            for i in 0..n {
                let (x0, x1) = tab_span(w, i, n);
                assert_eq!(x0, prev_end, "seam between tab {} and {}", i - 1, i);
                assert!(x1 >= x0);
                widths.push(x1 - x0);
                prev_end = x1;
            }
            assert_eq!(prev_end, w.floor(), "the tabs must cover the strip");
            // "Equal widths": within the one pixel rounding can cost.
            let min = widths.iter().cloned().fold(f64::MAX, f64::min);
            let max = widths.iter().cloned().fold(f64::MIN, f64::max);
            assert!(max - min <= 1.0, "w={} n={} widths={:?}", w, n, widths);
        }
    }

    #[test]
    fn a_title_is_cut_in_the_middle_not_at_the_end() {
        // Fits: untouched.
        assert_eq!(elide_middle("terminal", 20.0, mono), "terminal");
        // Does not fit: the head AND the tail survive, and the result is
        // inside the budget.
        let cut = elide_middle("makepad-example-splash", 11.0, mono);
        assert!(mono(&cut) <= 11.0, "{:?}", cut);
        assert_eq!(cut, "makep\u{2026}plash");
        // The head takes the odd character when the budget is uneven.
        assert_eq!(elide_middle("abcdefghij", 6.0, mono), "abc\u{2026}ij");
        assert_eq!(elide_middle("abcdefghij", 5.0, mono), "ab\u{2026}ij");
        // One more character would not have fit — the cut is maximal.
        assert_eq!(mono(&cut), 11.0);
        // Nothing fits: the ellipsis alone, never a panic.
        assert_eq!(elide_middle("terminal", 1.0, mono), "\u{2026}");
        assert_eq!(elide_middle("", 0.0, mono), "");
    }

    #[test]
    fn a_tab_click_survives_a_shaky_hand() {
        let start = dvec2(400.0, 53.0);
        // A click that wobbles a few pixels is still a click.
        assert!(!tab_press_escaped(start, start));
        assert!(!tab_press_escaped(start, dvec2(405.0, 56.0)));
        assert!(!tab_press_escaped(start, dvec2(391.0, 44.0)));
        // Past the threshold in either axis, either direction, it tears.
        assert!(tab_press_escaped(start, dvec2(410.0, 53.0)));
        assert!(tab_press_escaped(start, dvec2(390.0, 53.0)));
        assert!(tab_press_escaped(start, dvec2(400.0, 90.0)));
        assert!(tab_press_escaped(start, dvec2(400.0, 20.0)));
    }

    #[test]
    fn a_revealed_group_member_never_pops_in() {
        let rect = LRect::new(0.0, 0.0, 400.0, 300.0);
        // A genuinely new window opens from 87% and fades up.
        let opened = TileAnim::new(rect, true);
        let (scale, fade) = opened.popin();
        assert_eq!(scale, POPIN_SCALE);
        assert_eq!(fade, 0.0);

        // A tab switch reveals a window that was already running: full
        // size and fully opaque on its very first frame, and its drawn
        // rect is the target, not a tween start.
        let revealed = TileAnim::settled(rect, true);
        assert_eq!(revealed.popin(), (1.0, 1.0));
        assert_eq!(revealed.cur, rect);
        assert_eq!(revealed.target, rect);
        assert!(revealed.close_t.is_none());
        // And it stays there — nothing left to animate.
        let mut revealed = revealed;
        assert!(!revealed.step(1.0 / 60.0));
        assert_eq!(revealed.popin(), (1.0, 1.0));
        assert_eq!(revealed.cur, rect);
    }

    #[test]
    fn curves_are_eased_and_bounded() {
        assert_eq!(ease_out_quint(0.0), 0.0);
        assert!((ease_out_quint(1.0) - 1.0).abs() < 1e-6);
        // easeOutQuint is far past halfway at the halfway point.
        assert!(ease_out_quint(0.5) > 0.85, "{}", ease_out_quint(0.5));
        // Monotone, always inside [0,1].
        let mut prev = 0.0;
        for i in 0..=100 {
            let v = ease_out_quint(i as f64 / 100.0);
            assert!(v >= prev - 1e-9 && (0.0..=1.0).contains(&v));
            prev = v;
        }
        // almostLinear stays close to the diagonal.
        for i in 0..=10 {
            let t = i as f64 / 10.0;
            assert!((almost_linear(t) - t).abs() < 0.16, "t={}", t);
        }
        // Out of range input is clamped, not extrapolated.
        assert_eq!(ease_out_quint(-1.0), 0.0);
        assert!((ease_out_quint(2.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_linear_bezier_is_the_identity() {
        for i in 0..=10 {
            let t = i as f64 / 10.0;
            assert!((bezier(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, t) - t).abs() < 1e-3);
        }
    }

    #[test]
    fn tiles_settle_on_target_and_close_after_the_out_animation() {
        let a = LRect::new(0.0, 0.0, 100.0, 100.0);
        let b = LRect::new(200.0, 0.0, 100.0, 100.0);
        let mut anim = TileAnim::new(a, false);
        anim.retarget(b);
        assert_eq!(anim.cur, a);
        // 379ms of 60fps steps lands exactly on the target.
        for _ in 0..30 {
            anim.step(1.0 / 60.0);
        }
        assert_eq!(anim.cur, b);
        assert!(!anim.step(1.0 / 60.0) || anim.open_t < 1.0);

        // Opening: starts at 87% and fully faded out.
        let mut opening = TileAnim::new(a, true);
        let (scale, fade) = opening.popin();
        assert!((scale - POPIN_SCALE).abs() < 1e-9 && fade == 0.0);
        for _ in 0..30 {
            opening.step(1.0 / 60.0);
        }
        assert_eq!(opening.popin(), (1.0, 1.0));

        // Closing: the 220ms frozen-frame zoom-out, then reapable.
        let mut closing = TileAnim::new(a, true);
        closing.open_t = 1.0;
        closing.close_t = Some(0.0);
        assert!(!closing.done_closing());
        for _ in 0..((DUR_WINDOWS_OUT * 60.0) as usize + 2) {
            closing.step(1.0 / 60.0);
        }
        assert!(closing.done_closing());
        let (scale, fade) = closing.popin();
        assert!((scale - POPIN_SCALE).abs() < 1e-9 && fade == 0.0);
    }

    /// The drag-jumble fix: `snap_to` (what `WmDesk` calls every frame for
    /// the client `WmState::dragging` names) must land EXACTLY on target
    /// with zero tween left running — unlike `retarget`, which restarted
    /// every frame would leave `cur` chasing a moving target forever, at a
    /// wobbling size once its independently-rounded edges disagree.
    #[test]
    fn drag_snap_lands_exactly_with_no_tween_left_running() {
        let a = LRect::new(0.0, 0.0, 900.0, 700.0);
        let mut anim = TileAnim::new(a, true);
        // A `retarget` mid-drag would leave this at 0.0 — chasing.
        let b = LRect::new(12.0, 5.0, 900.0, 700.0);
        anim.snap_to(b);
        assert_eq!(anim.cur, b);
        assert_eq!(anim.from, b);
        assert_eq!(anim.target, b);
        assert_eq!(anim.move_t, 1.0);
        // A further `step` never moves `cur`: the position tween has
        // nothing left to animate (open_t may still be settling).
        anim.step(1.0 / 60.0);
        assert_eq!(anim.cur, b);

        // Many snaps in a row (one pointer-move per frame, as a real drag
        // sends) never restart a tween: size stays byte-identical, so its
        // device-pixel rounding can never wobble between two values.
        let mut pos = b;
        for i in 1..=20 {
            pos = LRect::new(12.0 + i as f64, 5.0 + i as f64, 900.0, 700.0);
            anim.snap_to(pos);
            assert_eq!(anim.cur.w, 900.0);
            assert_eq!(anim.cur.h, 700.0);
            assert_eq!(anim.move_t, 1.0, "a drag frame must never leave a tween running");
        }
        assert_eq!(anim.cur, pos);
    }

    #[test]
    fn translation_keeps_framebuffer_size_on_every_pixel_grid() {
        for dpi in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let size = dvec2(684.5, 853.1);
            let initial = snap_to_device(Rect { pos: dvec2(0.0, 0.0), size }, dpi);
            for step in 0..100 {
                let r = snap_to_device(Rect { pos: dvec2(step as f64 * 0.13, step as f64 * 0.17), size }, dpi);
                assert_eq!(r.size, initial.size);
                assert!((r.pos.x * dpi - (r.pos.x * dpi).round()).abs() < 1e-9);
                assert_eq!(snap_child_rect(r, dpi), r);
            }
        }
        let r = snap_to_device(Rect { pos: dvec2(10.25, 36.4), size: dvec2(684.5, 853.1) }, 2.0);
        assert_eq!(r.pos, dvec2(10.5, 36.5));
        assert_eq!(r.size, dvec2(684.5, 853.0));
    }

    #[test]
    fn gaps_match_omarchy() {
        // gaps_in 5 on each side of a window: 10 between two tiles, and
        // gaps_out 10 to the desk edge — uniform, like the reference.
        assert_eq!(TILE_GAP, 10.0);
        assert_eq!(GAPS_OUT, 10.0);
        assert_eq!(BORDER_SIZE, 2.0);
    }

    #[test]
    fn border_theme_reads_the_theme_source() {
        // Tokyo night names no gradient of its own, so it wears
        // hyprland's default: cyan → green at 45°.
        let t = BorderTheme::from_theme_source(theme::BUNDLED_TOKYO_NIGHT_SPLASH);
        assert!((t.active.x - 0x33 as f32 / 255.0).abs() < 0.01);
        assert!((t.active_end.y - 1.0).abs() < 0.01);
        assert_ne!(t.active, t.active_end);
        assert_eq!(t.angle, 45.0);
        assert!((t.inactive.w - 0.85).abs() < 0.01);
        // A theme with one stop draws solid.
        let solid = BorderTheme::from_theme_source("    active_border: #ff0000\n");
        assert_eq!(solid.active, solid.active_end);
        // Two stops and an angle survive to the shader.
        let grad = BorderTheme::from_theme_source(
            "active_border: #26a269\nactive_border_end: #2ec27e\nactive_border_angle: 45.0\n",
        );
        assert_ne!(grad.active, grad.active_end);
        assert_eq!(grad.angle, 45.0);
    }

    /// `handle_event` dispatches `zorder` in reverse — the bug this fixes:
    /// a float drawn over a tile lost its click to whichever the desk's
    /// `HashMap<ClientId, WidgetRef>` happened to iterate first, because
    /// `Event::hits` claims a positional event FIRST-WINS regardless of
    /// the visual stack. Composing the SAME order the draw loops use, and
    /// reversing it for dispatch, makes the actually-topmost widget the
    /// one whose `hits()` call sees the point first.
    #[test]
    fn zorder_is_back_to_front_previews_last() {
        let closing: Vec<ClientId> = vec![9];
        let rest: Vec<ClientId> = vec![1, 2, 3];
        let previews: Vec<ClientId> = vec![3];
        let order = compose_zorder(&closing, &rest, &previews);
        // Closing first (bottom), then the non-preview rest in their given
        // order, the preview pulled out of the middle and moved to the
        // very top.
        assert_eq!(order, vec![9, 1, 2, 3]);
        // Reversed for dispatch: the float (3) is offered the event
        // before either tile, so it wins `Event::hits`'s first-wins claim.
        let dispatch: Vec<ClientId> = order.into_iter().rev().collect();
        assert_eq!(dispatch, vec![3, 2, 1, 9]);
        assert_eq!(dispatch[0], 3, "the preview must be tried first");

        // No previews: the rest keeps its own order, nothing moves.
        assert_eq!(compose_zorder(&[], &[1, 2], &[]), vec![1, 2]);
        // Everything closing and nothing else: unchanged.
        assert_eq!(compose_zorder(&[5, 6], &[], &[]), vec![5, 6]);
    }
}

/// Run `f` on a tile's host trait, whichever tile kind the widget is.
pub fn with_tile_host<R>(item: &WidgetRef, f: impl FnOnce(&mut dyn TileHost) -> R) -> Option<R> {
    if let Some(mut view) = item.borrow_mut::<MpRunView>() {
        return Some(f(&mut *view as &mut dyn TileHost));
    }
    if let Some(mut view) = item.borrow_mut::<MpModuleView>() {
        return Some(f(&mut *view as &mut dyn TileHost));
    }
    None
}
