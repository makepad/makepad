//! The theme token registry: what every design token is called, what kind of
//! value it holds, which of the three themes carry it, the range a control
//! may drive it over, and the rule that generates the accent palette.
//!
//! Why this exists. The three theme files (`theme_desktop_dark.rs`,
//! `theme_desktop_light.rs`, `theme_desktop_skeleton.rs`) are script objects
//! that only the runtime reads. The Shift+F10 tweaker's theme tab and the catalogue
//! app need a typed list to build their controls from, with a doc line and a
//! range per token, and they need to know which theme lacks what (the
//! skeleton has no contrast, tint or font-contrast knob). `THEME_TOKENS` is
//! that list. Two drift tests at the bottom keep it and the three files in
//! step in both directions: every registered token is defined exactly once in
//! every theme it claims, and every key one of the new prefixes introduces is
//! registered. A third test regenerates the accent palette and compares it to
//! the literals in the files, so the palette is provably the repo's own.
//!
//! The colour rule. Accent roles are not transcribed from any published
//! palette; they are computed here in HSL from the hue of the house seed,
//! `color_makepad` #FF5C39 (hue 10.6 degrees). Seven families share one shape
//! (a base, the colour drawn on it, a container tint and the colour drawn on
//! that): primary keeps the seed hue at saturation 0.62; secondary rotates it
//! by +30 degrees and keeps 55 percent of the saturation; tertiary rotates it
//! by -150 degrees at 80 percent; the four intents use fixed hues (error 0,
//! warning 42, success 140, info 212) and their own saturations. Per scheme
//! the lightness targets are fixed: light puts the base at 0.40 under white
//! text with a 0.90 container and 0.12 text on it; dark lifts the base to 0.74
//! over 0.18 text with a 0.30 container (saturation times 0.7) and 0.90 text on
//! it; the skeleton desaturates the three brand families entirely (0.35 base,
//! 0.80 container) and runs the intents through the light rule at saturation
//! 0.45, so a warning still reads as a warning on a grey page.
//! `color_inverse_primary` is the other scheme's primary base. The existing
//! `color_error` and `color_warning` keep their historic values in dark and
//! light and are not generated into the files; the generator still produces
//! those two bases so an override path can use them.
//!
//! The generator is a trait (`RoleGenerator`) so a colour-science generator
//! can replace `BuiltinRoles` later without any caller changing.
//!
//! Overriding a theme has exactly two sanctioned shapes, both emitted here:
//! `theme_module_script` derives a new theme object from an existing one and
//! points `mod.theme` at it; `theme_source_with_globals` and
//! `export_theme_source` produce whole theme files. Never assign into
//! `mod.theme.x` directly: `mod.theme` is the shared base object, and such a
//! write mutates it for every widget that was built from it.

use crate::desktop_style::DesktopStyle;
use crate::makepad_platform::{LiveId, NoTrap, ScriptMod, ScriptVm};
use crate::script_eval;
use std::collections::{BTreeMap, BTreeSet};

/// One of the three themes shipped with the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Dark,
    Light,
    Skeleton,
}

impl Scheme {
    pub const ALL: [Scheme; 3] = [Scheme::Dark, Scheme::Light, Scheme::Skeleton];

    /// The key under `mod.themes`.
    pub fn theme_name(self) -> &'static str {
        match self {
            Scheme::Dark => "dark",
            Scheme::Light => "light",
            Scheme::Skeleton => "skeleton",
        }
    }

    /// The theme file's source text, as compiled into this library.
    pub fn source(self) -> &'static str {
        match self {
            Scheme::Dark => include_str!("theme_desktop_dark.rs"),
            Scheme::Light => include_str!("theme_desktop_light.rs"),
            Scheme::Skeleton => include_str!("theme_desktop_skeleton.rs"),
        }
    }
}

/// The tab a token belongs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenGroup {
    /// The knobs everything else derives from.
    Global,
    Space,
    Size,
    Radius,
    Elevation,
    Motion,
    State,
    Type,
    ColorAccent,
    ColorSurface,
    ColorStatus,
}

/// The kind of value a token holds; decides which control edits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Color,
    /// Layout points.
    Length,
    Seconds,
    /// 0..1.
    Opacity,
    /// A unitless multiplier or exponent.
    Factor,
    FontSize,
    /// An `Ease` value from the animator module.
    Ease,
    /// A `TextStyle` object.
    TextStyle,
}

/// One registered token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenSpec {
    pub name: &'static str,
    pub group: TokenGroup,
    pub kind: TokenKind,
    /// One plain sentence for the control's tooltip.
    pub doc: &'static str,
    /// The range a control may drive the token over; all zero for kinds a
    /// slider cannot edit.
    pub min: f64,
    pub max: f64,
    pub step: f64,
    /// The themes that define this token.
    pub themes: &'static [Scheme],
}

const ALL: &[Scheme] = &[Scheme::Dark, Scheme::Light, Scheme::Skeleton];
const DARK_LIGHT: &[Scheme] = &[Scheme::Dark, Scheme::Light];

const fn spec(
    name: &'static str,
    group: TokenGroup,
    kind: TokenKind,
    doc: &'static str,
    min: f64,
    max: f64,
    step: f64,
    themes: &'static [Scheme],
) -> TokenSpec {
    TokenSpec { name, group, kind, doc, min, max, step, themes }
}

const fn color(name: &'static str, group: TokenGroup, doc: &'static str, themes: &'static [Scheme]) -> TokenSpec {
    spec(name, group, TokenKind::Color, doc, 0.0, 0.0, 0.0, themes)
}

const fn length(name: &'static str, group: TokenGroup, doc: &'static str, min: f64, max: f64, step: f64) -> TokenSpec {
    spec(name, group, TokenKind::Length, doc, min, max, step, ALL)
}

const fn seconds(name: &'static str, doc: &'static str) -> TokenSpec {
    spec(name, TokenGroup::Motion, TokenKind::Seconds, doc, 0.0, 2.0, 0.05, ALL)
}

const fn ease(name: &'static str, doc: &'static str) -> TokenSpec {
    spec(name, TokenGroup::Motion, TokenKind::Ease, doc, 0.0, 0.0, 0.0, ALL)
}

const fn opacity(name: &'static str, doc: &'static str) -> TokenSpec {
    spec(name, TokenGroup::State, TokenKind::Opacity, doc, 0.0, 1.0, 0.01, ALL)
}

const fn font_size(name: &'static str, doc: &'static str) -> TokenSpec {
    spec(name, TokenGroup::Type, TokenKind::FontSize, doc, 4.0, 64.0, 0.25, ALL)
}

const fn text_style(name: &'static str, doc: &'static str) -> TokenSpec {
    spec(name, TokenGroup::Type, TokenKind::TextStyle, doc, 0.0, 0.0, 0.0, ALL)
}

const fn accent(name: &'static str, doc: &'static str) -> TokenSpec {
    color(name, TokenGroup::ColorAccent, doc, ALL)
}

const fn surface(name: &'static str, doc: &'static str) -> TokenSpec {
    color(name, TokenGroup::ColorSurface, doc, ALL)
}

const fn status(name: &'static str, doc: &'static str) -> TokenSpec {
    color(name, TokenGroup::ColorStatus, doc, ALL)
}

/// Every token the themes define for the widget programme, plus the globals
/// and the existing ladders new widgets are built on.
pub static THEME_TOKENS: &[TokenSpec] = &[
    // The globals.
    spec("color_contrast", TokenGroup::Global, TokenKind::Factor, "Exponent on every tint and shade of the ladder; above one spreads them, below one flattens them.", 0.2, 3.0, 0.05, DARK_LIGHT),
    color("color_tint", TokenGroup::Global, "Hue mixed into the app background and foreground by color_tint_amount.", DARK_LIGHT),
    spec("color_tint_amount", TokenGroup::Global, TokenKind::Factor, "How much of color_tint reaches the app background and foreground.", 0.0, 1.0, 0.01, DARK_LIGHT),
    spec("space_factor", TokenGroup::Global, TokenKind::Length, "The unit every space rung and control height is a multiple of.", 2.0, 20.0, 0.5, ALL),
    spec("corner_radius", TokenGroup::Global, TokenKind::Length, "The corner radius of the bevelled controls.", 0.0, 20.0, 0.5, ALL),
    spec("beveling", TokenGroup::Global, TokenKind::Length, "Width of the light and shadow edge on bevelled controls.", 0.0, 4.0, 0.05, ALL),
    spec("font_size_base", TokenGroup::Global, TokenKind::FontSize, "The paragraph size every other font size is offset from.", 6.0, 30.0, 0.5, ALL),
    spec("font_size_contrast", TokenGroup::Global, TokenKind::Factor, "The step between neighbouring sizes of the type ladder.", 0.0, 8.0, 0.1, DARK_LIGHT),
    // Space.
    length("space_1", TokenGroup::Space, "Half a space unit; the tightest gap.", 0.0, 80.0, 0.5),
    length("space_2", TokenGroup::Space, "One space unit; the default gap between siblings.", 0.0, 80.0, 0.5),
    length("space_3", TokenGroup::Space, "One and a half units; a group gap.", 0.0, 80.0, 0.5),
    length("space_4", TokenGroup::Space, "Two units; a section gap.", 0.0, 80.0, 0.5),
    length("space_5", TokenGroup::Space, "Three units; a panel inset.", 0.0, 80.0, 0.5),
    length("space_6", TokenGroup::Space, "Four units; a page margin.", 0.0, 80.0, 0.5),
    // Sizes.
    length("size_control_s", TokenGroup::Size, "Height of a small control.", 8.0, 120.0, 1.0),
    length("size_control_m", TokenGroup::Size, "Height of a default control.", 8.0, 120.0, 1.0),
    length("size_control_l", TokenGroup::Size, "Height of a large control; a tab is this tall.", 8.0, 120.0, 1.0),
    length("size_icon_s", TokenGroup::Size, "Side of a small icon.", 4.0, 96.0, 1.0),
    length("size_icon_m", TokenGroup::Size, "Side of a default icon.", 4.0, 96.0, 1.0),
    length("size_icon_l", TokenGroup::Size, "Side of a large icon.", 4.0, 96.0, 1.0),
    length("size_touch_target", TokenGroup::Size, "Minimum side of anything a finger has to hit.", 16.0, 96.0, 1.0),
    length("size_border", TokenGroup::Size, "Width of a standard border.", 0.0, 8.0, 0.5),
    length("size_focus_ring", TokenGroup::Size, "Width of the keyboard focus ring.", 0.0, 8.0, 0.5),
    length("size_divider", TokenGroup::Size, "Thickness of a divider line.", 0.0, 8.0, 0.5),
    // Shape.
    length("radius_none", TokenGroup::Radius, "Square corners.", 0.0, 40.0, 0.5),
    length("radius_xs", TokenGroup::Radius, "Barely rounded; chips and tags.", 0.0, 40.0, 0.5),
    length("radius_s", TokenGroup::Radius, "Small controls.", 0.0, 40.0, 0.5),
    length("radius_m", TokenGroup::Radius, "Buttons and fields.", 0.0, 40.0, 0.5),
    length("radius_l", TokenGroup::Radius, "Cards and menus.", 0.0, 40.0, 0.5),
    length("radius_xl", TokenGroup::Radius, "Dialogs and sheets.", 0.0, 40.0, 0.5),
    length("radius_full", TokenGroup::Radius, "A pill; larger than any control is tall.", 0.0, 1000.0, 1.0),
    // Elevation.
    length("elevation_1_radius", TokenGroup::Elevation, "Shadow blur of a resting card.", 0.0, 64.0, 1.0),
    length("elevation_1_offset_y", TokenGroup::Elevation, "Shadow drop of a resting card.", -32.0, 32.0, 1.0),
    length("elevation_2_radius", TokenGroup::Elevation, "Shadow blur of a raised control.", 0.0, 64.0, 1.0),
    length("elevation_2_offset_y", TokenGroup::Elevation, "Shadow drop of a raised control.", -32.0, 32.0, 1.0),
    length("elevation_3_radius", TokenGroup::Elevation, "Shadow blur of a menu or popover.", 0.0, 64.0, 1.0),
    length("elevation_3_offset_y", TokenGroup::Elevation, "Shadow drop of a menu or popover.", -32.0, 32.0, 1.0),
    length("elevation_4_radius", TokenGroup::Elevation, "Shadow blur of a dragged item.", 0.0, 64.0, 1.0),
    length("elevation_4_offset_y", TokenGroup::Elevation, "Shadow drop of a dragged item.", -32.0, 32.0, 1.0),
    length("elevation_5_radius", TokenGroup::Elevation, "Shadow blur of a dialog.", 0.0, 64.0, 1.0),
    length("elevation_5_offset_y", TokenGroup::Elevation, "Shadow drop of a dialog.", -32.0, 32.0, 1.0),
    color("color_elevation_1", TokenGroup::Elevation, "Shadow colour of a resting card.", ALL),
    color("color_elevation_2", TokenGroup::Elevation, "Shadow colour of a raised control.", ALL),
    color("color_elevation_3", TokenGroup::Elevation, "Shadow colour of a menu or popover.", ALL),
    color("color_elevation_4", TokenGroup::Elevation, "Shadow colour of a dragged item.", ALL),
    color("color_elevation_5", TokenGroup::Elevation, "Shadow colour of a dialog.", ALL),
    color("color_elevation_shadow", TokenGroup::Elevation, "The opaque colour the elevation shadows are tints of.", ALL),
    // Motion.
    seconds("motion_short_1", "Fifty milliseconds; a state layer appearing."),
    seconds("motion_short_2", "A tenth of a second; a hover or press."),
    seconds("motion_short_3", "150 milliseconds; a toggle or a selection."),
    seconds("motion_short_4", "A fifth of a second; a small element entering."),
    seconds("motion_medium_1", "A quarter second; a menu opening."),
    seconds("motion_medium_2", "300 milliseconds; a panel expanding."),
    seconds("motion_medium_3", "350 milliseconds; a sheet sliding."),
    seconds("motion_medium_4", "Two fifths of a second; a dialog entering."),
    seconds("motion_long_1", "450 milliseconds; a page transition."),
    seconds("motion_long_2", "Half a second; a large surface moving."),
    seconds("motion_long_3", "550 milliseconds; a hero element."),
    seconds("motion_long_4", "Six tenths of a second; a full-screen change."),
    seconds("motion_extra_long_1", "700 milliseconds; a staggered reveal."),
    seconds("motion_extra_long_2", "800 milliseconds; a long reveal."),
    seconds("motion_extra_long_3", "900 milliseconds; a decorative sweep."),
    seconds("motion_extra_long_4", "One second; the longest a widget may take."),
    ease("motion_ease_standard", "Ease in and out; anything that moves on screen."),
    ease("motion_ease_standard_decelerate", "Ease out only; things entering the screen."),
    ease("motion_ease_standard_accelerate", "Ease in only; things leaving the screen."),
    ease("motion_ease_emphasized_decelerate", "A strong ease out; a surface arriving with weight."),
    ease("motion_ease_emphasized_accelerate", "A strong ease in; a surface leaving with weight."),
    ease("motion_ease_linear", "No easing; progress and colour fades."),
    ease("motion_ease_spring", "Overshoots and settles; drags and snaps."),
    ease("motion_ease_bounce", "Rebounds off the value without passing it; something landing."),
    // State layers.
    opacity("state_hover_opacity", "Opacity of the state layer under a hovered control."),
    opacity("state_focus_opacity", "Opacity of the state layer under a focused control."),
    opacity("state_press_opacity", "Opacity of the state layer under a pressed control."),
    opacity("state_drag_opacity", "Opacity of the state layer under a dragged item."),
    opacity("state_disabled_content_opacity", "Opacity of the text and icon of a disabled control."),
    opacity("state_disabled_container_opacity", "Opacity of the fill of a disabled control."),
    opacity("state_scrim_opacity", "Opacity of the scrim behind a modal."),
    // Type.
    font_size("type_title_l_size", "Large title; a dialog heading."),
    font_size("type_title_m_size", "Medium title; a card heading."),
    font_size("type_title_s_size", "Small title; a list heading."),
    font_size("type_body_l_size", "Large body; long reading."),
    font_size("type_body_m_size", "Body; the paragraph size."),
    font_size("type_body_s_size", "Small body; secondary text."),
    font_size("type_label_l_size", "Large label; buttons and tabs."),
    font_size("type_label_m_size", "Medium label; chips and fields."),
    font_size("type_label_s_size", "Small label; captions and badges."),
    font_size("font_size_5", "Between H4 and the paragraph size."),
    font_size("font_size_6", "Just above the paragraph size."),
    text_style("font_title_l", "Bold at type_title_l_size."),
    text_style("font_title_m", "Bold at type_title_m_size."),
    text_style("font_title_s", "Bold at type_title_s_size."),
    text_style("font_body_l", "Regular at type_body_l_size."),
    text_style("font_body_m", "Regular at type_body_m_size."),
    text_style("font_body_s", "Regular at type_body_s_size."),
    text_style("font_label_l", "Bold at type_label_l_size."),
    text_style("font_label_m", "Bold at type_label_m_size."),
    text_style("font_label_s", "Bold at type_label_s_size."),
    // Accent roles.
    accent("color_primary", "The brand colour; filled buttons and active states."),
    accent("color_on_primary", "Text and icons on color_primary."),
    accent("color_primary_container", "A soft tint of the brand colour for prominent containers."),
    accent("color_on_primary_container", "Text and icons on color_primary_container."),
    accent("color_secondary", "A quieter accent for less prominent controls."),
    accent("color_on_secondary", "Text and icons on color_secondary."),
    accent("color_secondary_container", "A soft tint of the secondary accent."),
    accent("color_on_secondary_container", "Text and icons on color_secondary_container."),
    accent("color_tertiary", "A contrasting accent for highlights and balance."),
    accent("color_on_tertiary", "Text and icons on color_tertiary."),
    accent("color_tertiary_container", "A soft tint of the tertiary accent."),
    accent("color_on_tertiary_container", "Text and icons on color_tertiary_container."),
    accent("color_error", "Errors and destructive actions."),
    accent("color_on_error", "Text and icons on color_error."),
    accent("color_error_container", "A soft tint for error messages and fields."),
    accent("color_on_error_container", "Text and icons on color_error_container."),
    accent("color_warning", "Warnings and things that need attention."),
    accent("color_on_warning", "Text and icons on color_warning."),
    accent("color_warning_container", "A soft tint for warning messages."),
    accent("color_on_warning_container", "Text and icons on color_warning_container."),
    accent("color_success", "Success and completion."),
    accent("color_on_success", "Text and icons on color_success."),
    accent("color_success_container", "A soft tint for success messages."),
    accent("color_on_success_container", "Text and icons on color_success_container."),
    accent("color_info", "Neutral information."),
    accent("color_on_info", "Text and icons on color_info."),
    accent("color_info_container", "A soft tint for informational messages."),
    accent("color_on_info_container", "Text and icons on color_info_container."),
    accent("color_inverse_primary", "The primary of the opposite scheme, for accents on an inverse surface."),
    accent("color_scrim", "The opaque colour a modal scrim is a tint of."),
    // Surfaces and outlines.
    surface("color_surface", "The page background."),
    surface("color_surface_container", "The default container fill."),
    surface("color_surface_container_low", "A container between the page and the default fill."),
    surface("color_surface_container_high", "A container lifted above the default fill."),
    surface("color_surface_container_highest", "The most lifted container."),
    surface("color_surface_container_lowest", "The most recessed container."),
    surface("color_surface_dim", "A dimmed page, behind a scrim."),
    surface("color_surface_bright", "The brightest surface."),
    surface("color_on_surface", "Text and icons on any surface."),
    surface("color_on_surface_variant", "Secondary text and icons on any surface."),
    surface("color_outline", "Borders that must be seen."),
    surface("color_outline_variant", "Decorative borders and dividers."),
    surface("color_inverse_surface", "A surface of the opposite scheme, for snackbars and tooltips."),
    surface("color_inverse_on_surface", "Text and icons on color_inverse_surface."),
    surface("color_placeholder", "The block a loading placeholder is drawn in."),
    surface("color_placeholder_hl", "The sheen that sweeps a loading placeholder."),
    surface("color_opaque_u_1", "First opaque step up from the foreground."),
    surface("color_opaque_u_2", "Second opaque step up from the foreground."),
    surface("color_opaque_u_3", "Third opaque step up from the foreground."),
    surface("color_opaque_u_4", "Fourth opaque step up from the foreground."),
    surface("color_opaque_u_5", "Fifth opaque step up from the foreground."),
    surface("color_opaque_u_6", "Sixth opaque step up from the foreground."),
    surface("color_opaque_d_1", "First opaque step down from the foreground."),
    surface("color_opaque_d_2", "Second opaque step down from the foreground."),
    surface("color_opaque_d_3", "Third opaque step down from the foreground."),
    surface("color_opaque_d_4", "Fourth opaque step down from the foreground."),
    surface("color_opaque_d_5", "Fifth opaque step down from the foreground."),
    // Status.
    status("color_presence_online", "A presence dot for someone available."),
    status("color_presence_away", "A presence dot for someone away."),
    status("color_presence_busy", "A presence dot for someone busy."),
    status("color_presence_offline", "A presence dot for someone offline."),
    status("color_high", "The historic high-severity colour."),
    status("color_mid", "The historic mid-severity colour."),
    status("color_low", "The historic low-severity colour."),
    status("color_panic", "The historic panic colour."),
    status("color_icon_wait", "The icon colour while waiting."),
    status("color_icon_panic", "The icon colour of a panic."),
];

/// The registered token of that name, if any.
pub fn token_spec(name: &str) -> Option<&'static TokenSpec> {
    THEME_TOKENS.iter().find(|t| t.name == name)
}

/// The colours a palette is grown from. Only `primary` is required; every
/// other seed defaults to a rotation of it or a fixed intent hue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeedColors {
    pub primary: u32,
    pub secondary: Option<u32>,
    pub tertiary: Option<u32>,
    pub neutral: Option<u32>,
    pub error: Option<u32>,
}

impl SeedColors {
    /// The library's own seed: `color_makepad`.
    pub const HOUSE: SeedColors = SeedColors {
        primary: 0xFF5C39FF,
        secondary: None,
        tertiary: None,
        neutral: None,
        error: None,
    };
}

/// One accent family: the colour, what is drawn on it, its container tint
/// and what is drawn on that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleFamily {
    pub base: u32,
    pub on_base: u32,
    pub container: u32,
    pub on_container: u32,
}

/// The accent roles of one scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorRoles {
    pub primary: RoleFamily,
    pub secondary: RoleFamily,
    pub tertiary: RoleFamily,
    pub error: RoleFamily,
    pub warning: RoleFamily,
    pub success: RoleFamily,
    pub info: RoleFamily,
    pub inverse_primary: u32,
}

const FAMILY_NAMES: [&str; 7] = ["primary", "secondary", "tertiary", "error", "warning", "success", "info"];

impl ColorRoles {
    fn families(&self) -> [&RoleFamily; 7] {
        [&self.primary, &self.secondary, &self.tertiary, &self.error, &self.warning, &self.success, &self.info]
    }

    /// Every role as `(theme key, rgba)`, in the order the theme files list
    /// them: the four members of each family, then `color_inverse_primary`.
    pub fn entries(&self) -> Vec<(&'static str, u32)> {
        let mut out = Vec::with_capacity(29);
        for (name, family) in FAMILY_NAMES.iter().zip(self.families()) {
            out.push((role_key(name, ""), family.base));
            out.push((role_key(name, "on_"), family.on_base));
            out.push((role_key(name, "container"), family.container));
            out.push((role_key(name, "on_container"), family.on_container));
        }
        out.push(("color_inverse_primary", self.inverse_primary));
        out
    }
}

/// The theme key of one role member, as a static string so `entries` can
/// hand out `&'static str`.
fn role_key(family: &str, member: &str) -> &'static str {
    const KEYS: [[&str; 4]; 7] = [
        ["color_primary", "color_on_primary", "color_primary_container", "color_on_primary_container"],
        ["color_secondary", "color_on_secondary", "color_secondary_container", "color_on_secondary_container"],
        ["color_tertiary", "color_on_tertiary", "color_tertiary_container", "color_on_tertiary_container"],
        ["color_error", "color_on_error", "color_error_container", "color_on_error_container"],
        ["color_warning", "color_on_warning", "color_warning_container", "color_on_warning_container"],
        ["color_success", "color_on_success", "color_success_container", "color_on_success_container"],
        ["color_info", "color_on_info", "color_info_container", "color_on_info_container"],
    ];
    let f = FAMILY_NAMES.iter().position(|n| *n == family).expect("a known family");
    let m = match member {
        "" => 0,
        "on_" => 1,
        "container" => 2,
        _ => 3,
    };
    KEYS[f][m]
}

/// `0xRRGGBBAA` to (hue in degrees, saturation 0..1, lightness 0..1).
pub fn rgb_to_hsl(rgba: u32) -> (f64, f64, f64) {
    let r = ((rgba >> 24) & 0xFF) as f64 / 255.0;
    let g = ((rgba >> 16) & 0xFF) as f64 / 255.0;
    let b = ((rgba >> 8) & 0xFF) as f64 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

/// (hue in degrees, saturation 0..1, lightness 0..1) to opaque `0xRRGGBBFF`,
/// each channel rounded half away from zero.
pub fn hsl_to_rgb(h: f64, s: f64, l: f64) -> u32 {
    let h = h.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let channel = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    (channel(r1) << 24) | (channel(g1) << 16) | (channel(b1) << 8) | 0xFF
}

/// Mix two opaque colours by `t` (0 gives `a`, 1 gives `b`), per channel,
/// rounded half away from zero; the script's `mix()` on two opaque colours.
pub fn mix_rgb(a: u32, b: u32, t: f64) -> u32 {
    let ch = |shift: u32| {
        let av = ((a >> shift) & 0xFF) as f64;
        let bv = ((b >> shift) & 0xFF) as f64;
        (av + (bv - av) * t).round().clamp(0.0, 255.0) as u32
    };
    (ch(24) << 24) | (ch(16) << 16) | (ch(8) << 8) | 0xFF
}

/// The hue of the house seed, about 10.6 degrees.
pub fn seed_hue() -> f64 {
    rgb_to_hsl(SeedColors::HOUSE.primary).0
}

const WHITE: u32 = 0xFFFFFFFF;

/// A family's hue and saturation before the scheme rule is applied.
struct FamilyInput {
    hue: f64,
    sat: f64,
    /// The four intents keep a little colour in the skeleton; the three brand
    /// families lose it all.
    intent: bool,
}

fn family_inputs(seed: &SeedColors) -> [FamilyInput; 7] {
    let hue_of = |rgba: u32| rgb_to_hsl(rgba).0;
    let primary = hue_of(seed.primary);
    // A seed with no colour in it has no hue either, and the number it
    // reports for one is nought, which is red: a style whose accent is black
    // got a family of reds. The rule takes only the hue from a seed and
    // brings its own saturation, so it has to be told when there is no hue
    // to take, and then the three brand families are greys.
    let brand = if rgb_to_hsl(seed.primary).1 < 0.08 { 0.0 } else { 0.62 };
    [
        FamilyInput { hue: primary, sat: brand, intent: false },
        FamilyInput { hue: seed.secondary.map(hue_of).unwrap_or(primary + 30.0), sat: brand * 0.55, intent: false },
        FamilyInput { hue: seed.tertiary.map(hue_of).unwrap_or(primary - 150.0), sat: brand * 0.8, intent: false },
        FamilyInput { hue: seed.error.map(hue_of).unwrap_or(0.0), sat: 0.72, intent: true },
        FamilyInput { hue: 42.0, sat: 0.85, intent: true },
        FamilyInput { hue: 140.0, sat: 0.55, intent: true },
        FamilyInput { hue: 212.0, sat: 0.70, intent: true },
    ]
}

/// The least a colour drawn on another has to stand off it to be read as
/// text, as a ratio of their luminances.
pub const READABLE: f64 = 4.5;

/// What a graphic or a second voice needs, where body text needs
/// `READABLE`. The variant ink carries menu labels, list item detail and
/// icons; held to the body rule it would have to be as strong as the body
/// ink and would stop being a second voice at all.
pub const LEGIBLE: f64 = 3.0;

const BLACK: u32 = 0x000000FF;

/// How far two colours stand apart, 1 to 21, by the luminance each has once
/// its channels are taken out of their display curve. Alpha is ignored: a
/// role is drawn solid.
pub fn contrast(a: u32, b: u32) -> f64 {
    fn luminance(rgba: u32) -> f64 {
        let channel = |shift: u32| {
            let v = ((rgba >> shift) & 0xFF) as f64 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(24) + 0.7152 * channel(16) + 0.0722 * channel(8)
    }
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// An ink laid over its ground. A role's ink often carries an alpha -- the
/// dark theme's text was white at 65% -- and what a reader sees is the two
/// combined, never the ink alone. Measuring the ink by itself flatters every
/// pair it appears in.
pub fn over(ground: u32, ink: u32) -> u32 {
    let a = (ink & 0xFF) as f64 / 255.0;
    let ch = |v: u32, sh: u32| ((v >> sh) & 0xFF) as f64;
    let mixed = |sh: u32| (ch(ink, sh) * a + ch(ground, sh) * (1.0 - a)).round() as u32;
    (mixed(24) << 24) | (mixed(16) << 16) | (mixed(8) << 8) | 0xFF
}

/// How an ink really reads on a ground, its alpha included.
pub fn reads_on(ground: u32, ink: u32) -> f64 {
    contrast(ground | 0xFF, over(ground | 0xFF, ink))
}

/// The ink to draw on a ground: the one asked for where it reaches `need`,
/// and the plainer end where it does not.
pub fn ink_for(ground: u32, ink: u32, need: f64) -> u32 {
    if reads_on(ground, ink) >= need {
        ink
    } else if contrast(WHITE, ground | 0xFF) >= contrast(BLACK, ground | 0xFF) {
        WHITE
    } else {
        BLACK
    }
}

/// What to draw ON a ground, given the two ends the rule would reach for:
/// the first if it reads, the other if only that does, and failing both the
/// plainer of black and white.
///
/// The rule used to name one end and stop. That is right for a red or a
/// blue and wrong for an amber or a green, which are far brighter than a red
/// of the same lightness in the space the rule counts in: white on the
/// warning colour stood at 1.9:1. Asking the ground is the only version of
/// the rule that holds for a hue nobody has tried yet, which is what a style
/// sheet's accent is.
pub fn readable_on(ground: u32, first: u32, other: u32) -> u32 {
    if contrast(first, ground) >= READABLE {
        first
    } else if contrast(other, ground) >= READABLE {
        other
    } else if contrast(BLACK, ground) > contrast(WHITE, ground) {
        BLACK
    } else {
        WHITE
    }
}

fn light_family(hue: f64, sat: f64) -> RoleFamily {
    let base = hsl_to_rgb(hue, sat, 0.40);
    let container = hsl_to_rgb(hue, (sat * 1.1).min(1.0), 0.90);
    let ink = hsl_to_rgb(hue, sat, 0.12);
    RoleFamily {
        base,
        on_base: readable_on(base, WHITE, ink),
        container,
        on_container: readable_on(container, ink, WHITE),
    }
}

fn dark_family(hue: f64, sat: f64) -> RoleFamily {
    let base = hsl_to_rgb(hue, sat, 0.74);
    let container = hsl_to_rgb(hue, sat * 0.7, 0.30);
    let ink = hsl_to_rgb(hue, sat, 0.18);
    let pale = hsl_to_rgb(hue, (sat * 1.1).min(1.0), 0.90);
    RoleFamily {
        base,
        on_base: readable_on(base, ink, pale),
        container,
        on_container: readable_on(container, pale, ink),
    }
}

fn skeleton_family(hue: f64, intent: bool) -> RoleFamily {
    if intent {
        light_family(hue, 0.45)
    } else {
        RoleFamily {
            base: hsl_to_rgb(0.0, 0.0, 0.35),
            on_base: WHITE,
            container: hsl_to_rgb(0.0, 0.0, 0.80),
            on_container: hsl_to_rgb(0.0, 0.0, 0.10),
        }
    }
}

fn family_for(scheme: Scheme, input: &FamilyInput) -> RoleFamily {
    match scheme {
        Scheme::Light => light_family(input.hue, input.sat),
        Scheme::Dark => dark_family(input.hue, input.sat),
        Scheme::Skeleton => skeleton_family(input.hue, input.intent),
    }
}

/// The accent roles of a scheme grown from a seed, by the rule in the module
/// doc. `roles_for` is this with the house seed.
pub fn roles_from_seed(seed: &SeedColors, scheme: Scheme) -> ColorRoles {
    let inputs = family_inputs(seed);
    let f = |i: usize| family_for(scheme, &inputs[i]);
    let inverse_primary = match scheme {
        Scheme::Light => dark_family(inputs[0].hue, inputs[0].sat).base,
        Scheme::Dark => light_family(inputs[0].hue, inputs[0].sat).base,
        Scheme::Skeleton => 0xBBBBBBFF,
    };
    // Error and warning are not drawn on the colours this rule makes for
    // them. Every theme keeps `color_error` and `color_warning` as the older
    // `color_high` and `color_mid`, so what is drawn on them has to be chosen
    // against THOSE: the dark theme's ink stood at 2.6:1 on the red that is
    // actually there, having been picked for a pink that is not.
    let on_kept = |family: RoleFamily, ground: u32| RoleFamily {
        on_base: readable_on(ground, family.on_base, if family.on_base == WHITE { BLACK } else { WHITE }),
        ..family
    };
    ColorRoles {
        primary: f(0),
        secondary: f(1),
        tertiary: f(2),
        error: on_kept(f(3), KEPT_ERROR),
        warning: on_kept(f(4), KEPT_WARNING),
        success: f(5),
        info: f(6),
        inverse_primary,
    }
}

/// `color_high` and `color_mid` as every theme file declares them, which is
/// what `color_error` and `color_warning` are in all three.
/// `error_and_warning_are_the_older_colours` holds these to the files.
pub const KEPT_ERROR: u32 = 0xCC0000FF;
pub const KEPT_WARNING: u32 = 0xFFAA00FF;

/// The accent roles the theme files carry for a scheme: the house seed
/// through the built-in rule.
pub fn roles_for(scheme: Scheme) -> ColorRoles {
    roles_from_seed(&SeedColors::HOUSE, scheme)
}

/// The skeleton theme's opaque ladder as `(theme key, rgba)`: its foreground
/// #EEEEEE mixed toward white by 0.15 / 0.25 / 0.35 / 0.5 / 0.7 / 0.8 for the
/// six steps up and toward black by 0.15 / 0.25 / 0.45 / 0.6 / 0.75 for the
/// five steps down, the same amounts dark and light use in their `mix()`
/// expressions.
pub fn skeleton_ladder() -> Vec<(&'static str, u32)> {
    const FG: u32 = 0xEEEEEEFF;
    const BLACK: u32 = 0x000000FF;
    vec![
        ("color_opaque_u_6", mix_rgb(FG, WHITE, 0.8)),
        ("color_opaque_u_5", mix_rgb(FG, WHITE, 0.7)),
        ("color_opaque_u_4", mix_rgb(FG, WHITE, 0.5)),
        ("color_opaque_u_3", mix_rgb(FG, WHITE, 0.35)),
        ("color_opaque_u_2", mix_rgb(FG, WHITE, 0.25)),
        ("color_opaque_u_1", mix_rgb(FG, WHITE, 0.15)),
        ("color_opaque_d_1", mix_rgb(FG, BLACK, 0.15)),
        ("color_opaque_d_2", mix_rgb(FG, BLACK, 0.25)),
        ("color_opaque_d_3", mix_rgb(FG, BLACK, 0.45)),
        ("color_opaque_d_4", mix_rgb(FG, BLACK, 0.6)),
        ("color_opaque_d_5", mix_rgb(FG, BLACK, 0.75)),
    ]
}

/// Where one of the library's own roles comes from in a base theme: another
/// token, or two of them mixed half and half.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RoleSource {
    Token(&'static str),
    HalfMix(&'static str, &'static str),
    /// A token mixed toward white or black by an amount: the surface ladder,
    /// written against `color_fg_app` rather than against the opaque ladder
    /// it used to alias. A sheet sets `color_fg_app` and does not set the
    /// opaque ladder, so the alias left every rung above a sheet's own
    /// surfaces holding the base theme's greys.
    MixTo(&'static str, u32, f64),
}

impl RoleSource {
    /// The expression as the theme file writes it.
    pub fn expression(self) -> String {
        match self {
            RoleSource::Token(t) => format!("theme.{t}"),
            RoleSource::HalfMix(a, b) => format!("mix(theme.{a}, theme.{b}, 0.5)"),
            RoleSource::MixTo(t, end, amount) => {
                format!("mix(theme.{t}, {}, {amount})", if end == WHITE { "#F" } else { "#0" })
            }
        }
    }
}

/// The roles a base theme derives from older tokens, as `(role, where the
/// light theme gets it, where the dark theme gets it)`.
///
/// Written down a second time, beside the theme files that are the first,
/// because a style sheet needs it and cannot get it from them: a theme
/// derives these ONCE, when it is built, so a sheet that then changes
/// `color_bg_app` leaves `color_surface` holding the colour it replaced.
/// `the_derived_role_table_is_the_theme_files` holds the two copies together.
pub const DERIVED_ROLES: &[(&str, RoleSource, RoleSource)] = {
    use RoleSource::{HalfMix as M, MixTo as X, Token as T};
    &[
        ("color_surface", T("color_bg_app"), T("color_bg_app")),
        ("color_surface_container", T("color_fg_app"), T("color_fg_app")),
        ("color_surface_container_low", M("color_bg_app", "color_fg_app"), M("color_bg_app", "color_fg_app")),
        ("color_surface_container_high", X("color_fg_app", BLACK, 0.15), X("color_fg_app", WHITE, 0.08)),
        ("color_surface_container_highest", X("color_fg_app", BLACK, 0.25), X("color_fg_app", WHITE, 0.15)),
        ("color_surface_dim", X("color_fg_app", BLACK, 0.15), X("color_fg_app", BLACK, 0.25)),
        ("color_surface_bright", X("color_fg_app", WHITE, 0.35), X("color_fg_app", WHITE, 0.17)),
        ("color_outline", T("color_d_2"), T("color_u_3")),
        ("color_outline_variant", T("color_d_1"), T("color_u_15")),
        ("color_inverse_surface", T("color_opaque_d_5"), T("color_opaque_u_6")),
        ("color_inverse_on_surface", T("color_opaque_u_6"), T("color_opaque_d_5")),
        ("color_placeholder", T("color_opaque_d_1"), T("color_opaque_u_1")),
        ("color_placeholder_hl", T("color_opaque_d_2"), T("color_opaque_u_2")),
    ]
};

/// The token a style sheet's accent is read from. Every sheet the library
/// ships sets it, and it is the saturated one: the highlight behind a
/// selection is a pale tint in some of them, which is no seed for an accent.
pub const SHEET_ACCENT: &str = "color_ctrl_selected";

/// What the role derivation is allowed to regrow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoleGrowth {
    /// Surfaces, the inks that go on them, and the three brand families grown
    /// from `SHEET_ACCENT`. What a style sheet needs: it sets a few hundred of
    /// the older tokens and none of the roles at all.
    FromAccent,
    /// Surfaces and inks only, leaving the brand families as they stand, and
    /// putting the mix's OWN body ink to the ground rather than `color_text`.
    /// What a mix needs: every theme in it already carries a considered brand
    /// family and a considered body ink, where a sheet carries neither. The
    /// base themes' `color_ctrl_selected` is a focus BLUE that has never been
    /// their `color_primary`, so growing from it would turn a mix of two
    /// orange themes blue; and their `color_on_surface` is a decided white or
    /// black that their `color_text` is a translucent version of, so reading
    /// the ink off `color_text` would rewrite a lone theme into something
    /// that is not it.
    KeepAccent,
}

/// The roles a theme derives from its older tokens, as `(key, rgba)` in the
/// order a theme file lists them.
///
/// `sheet_roles_script` writes these out as a script of assignments, which is
/// what a sheet needs; the equalizer folds the same answers into the overrides
/// of a derived theme object instead. One rule, two ways of spending it.
pub fn derived_roles(
    dark: bool,
    growth: RoleGrowth,
    named: &mut dyn FnMut(&str) -> bool,
    read: &mut dyn FnMut(&str) -> Option<u32>,
) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    let mut ladder_top = None;
    for (role, light, dark_source) in DERIVED_ROLES {
        if named(role) {
            continue;
        }
        let value = match if dark { *dark_source } else { *light } {
            RoleSource::Token(t) => read(t),
            RoleSource::HalfMix(a, b) => match (read(a), read(b)) {
                (Some(a), Some(b)) => Some(mix_rgb(a, b, 0.5)),
                _ => None,
            },
            RoleSource::MixTo(t, end, amount) => read(t).map(|c| mix_rgb(c | 0xFF, end, amount)),
        };
        if let Some(rgba) = value {
            if *role == "color_surface_container_high" {
                ladder_top = Some(rgba);
            }
            out.push((role.to_string(), rgba));
        }
    }
    // What is drawn ON those surfaces. A sheet chose its text colour against
    // its own flat background and never saw the ladder derived above, so the
    // choice is put to the brightest rung a widget lays text on: kept where
    // it still reads there, replaced by the plain end where it does not. The
    // second voice has no token of its own in a sheet, so what carries over
    // is the base theme's own considered value, put to the same test.
    if let Some(ground) = ladder_top.or_else(|| read("color_surface_container_high")) {
        let body = match growth {
            RoleGrowth::FromAccent => "color_text",
            RoleGrowth::KeepAccent => "color_on_surface",
        };
        for (role, source, need) in [
            ("color_on_surface", body, READABLE),
            ("color_on_surface_variant", "color_on_surface_variant", LEGIBLE),
        ] {
            if named(role) {
                continue;
            }
            if let Some(ink) = read(source) {
                out.push((role.to_string(), ink_for(ground, ink, need)));
            }
        }
    }
    if growth == RoleGrowth::FromAccent {
        if let Some(accent) = read(SHEET_ACCENT) {
            let seed = SeedColors { primary: accent | 0xFF, ..SeedColors::HOUSE };
            let scheme = if dark { Scheme::Dark } else { Scheme::Light };
            for (key, rgba) in roles_from_seed(&seed, scheme).entries() {
                let follows = ["primary", "secondary", "tertiary"].iter().any(|family| key.contains(family));
                if follows && !named(key) {
                    out.push((key.to_string(), rgba));
                }
            }
        }
    }
    // The four families that mean something keep their colours, but a sheet
    // may have set those colours itself, and what is drawn on them was chosen
    // for the base theme's: black picked for the light theme's green stood at
    // 4.2:1 on the darker green six sheets use. So the ink is asked of the
    // ground that is there now, and left alone wherever it already reads.
    for family in ["error", "warning", "success", "info"] {
        let on_key = format!("color_on_{family}");
        if named(&on_key) {
            continue;
        }
        if let (Some(ground), Some(ink)) = (read(&format!("color_{family}")), read(&on_key)) {
            let (ground, ink) = (ground | 0xFF, ink | 0xFF);
            let reads = readable_on(ground, ink, if contrast(WHITE, ground) > contrast(BLACK, ground) { WHITE } else { BLACK });
            if reads != ink {
                out.push((on_key, reads));
            }
        }
    }
    out
}

/// The script that brings the library's own roles into line with a style
/// sheet, to be run straight after the sheet's tokens.
///
/// A sheet sets a few hundred of the older tokens and none of the roles,
/// because the roles are younger than the sheets. Left alone, a widget built
/// on roles keeps the base theme's surfaces under a sheet that has changed
/// them, and lights its selection in the house accent beside buttons lit in
/// the sheet's own. So the surfaces are read again from the tokens the sheet
/// DID set, and the accent families are grown from the sheet's own accent by
/// the rule that grew the house ones — which also settles what is drawn ON
/// the accent, a colour no sheet names and one that cannot be guessed: white
/// on one sheet's navy and near black on another's lavender.
///
/// The families that mean something — error, warning, success, info — stay
/// as they are: red is an error under every style. A role the sheet names
/// for itself is the sheet's to decide and is left alone.
pub fn sheet_roles_script(sheet_theme: &str, read: &mut dyn FnMut(&str) -> Option<u32>) -> String {
    let dark = sheet_theme.contains("mod.themes.dark");
    let mut named = |key: &str| {
        sheet_theme.lines().any(|line| {
            line.trim_start()
                .strip_prefix("mod.theme.")
                .and_then(|rest| rest.strip_prefix(key))
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
    };
    let mut out = String::new();
    for (key, rgba) in derived_roles(dark, RoleGrowth::FromAccent, &mut named, read) {
        out.push_str(&format!("mod.theme.{key} = #x{rgba:08X}\n"));
    }
    // The last statement of an evaluated script is swallowed, so a script
    // that ends on an assignment loses it. Every sheet the library ships
    // ends with this same bare `true` for the same reason. Without it the
    // final role derived here was silently the one before the sheet.
    out.push_str("true\n");
    out
}

/// Grows a scheme's accent roles from seed colours. The built-in generator is
/// the HSL rule above; a colour-science generator implements the same trait
/// and slots in without any caller changing.
pub trait RoleGenerator {
    /// `contrast` is the theme's `color_contrast` (1.0 is neutral); a
    /// generator may use it to pull the roles apart or ignore it.
    fn generate(&self, seed: &SeedColors, scheme: Scheme, contrast: f32) -> ColorRoles;
}

/// The HSL rule the theme files were generated with. Honours the seed hues
/// and ignores `contrast`, since the rule's lightness targets are fixed.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuiltinRoles;

impl RoleGenerator for BuiltinRoles {
    fn generate(&self, seed: &SeedColors, scheme: Scheme, _contrast: f32) -> ColorRoles {
        roles_from_seed(seed, scheme)
    }
}

/// A token value on its way into a script: a colour, a number, or any other
/// expression written out as the script would read it.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenValue {
    Color(u32),
    Num(f64),
    Raw(String),
}

impl TokenValue {
    /// The script literal: colours as `#xRRGGBBAA` (the `x` keeps a hex run
    /// that happens to contain an `e` from reading as a number), numbers with
    /// a decimal point so they stay floats, raw text verbatim.
    pub fn render(&self) -> String {
        match self {
            TokenValue::Color(c) => format!("#x{:08X}", c),
            TokenValue::Num(f) => format!("{:?}", f),
            TokenValue::Raw(s) => s.clone(),
        }
    }
}

/// A script that derives a theme from an existing one and makes it current:
/// `mod.themes.<name> = mod.themes.<base>{ k: v ... }` followed by
/// `mod.theme = mod.themes.<name>`. Run it between `theme_mod` and
/// `widgets_mod`, or through a live edit, the way the catalogue switches
/// themes. This is the only sanctioned override path: assigning into
/// `mod.theme.k` mutates the shared base object for every widget built from
/// it. Overriding a derived token pins it; the tokens derived from it keep
/// their old values, since derivation happens once when the base is built.
pub fn theme_module_script(name: &str, base: &str, overrides: &[(String, TokenValue)]) -> String {
    let mut out = format!("mod.themes.{name} = mod.themes.{base}{{");
    for (key, value) in overrides {
        out.push(' ');
        out.push_str(key);
        out.push_str(": ");
        out.push_str(&value.render());
    }
    out.push_str(&format!(" }}\nmod.theme = mod.themes.{name}\n"));
    out
}

/// A copy of a theme file with some of its top-level keys replaced by
/// literals and the theme renamed. Meant for the globals (`space_factor`,
/// `color_contrast`, ...): every expression that derives from them stays an
/// expression, so the edit genuinely re-derives the ladder, which a
/// `theme_module_script` override cannot do. A key whose value spans several
/// lines (a `mix(` or a `TextStyle{`) is replaced whole.
pub fn theme_source_with_globals(name: &str, base_file_body: &str, globals: &[(String, TokenValue)]) -> String {
    let mut out = String::with_capacity(base_file_body.len() + 64);
    let mut skipping = false;
    for line in base_file_body.lines() {
        if skipping {
            // Continuation lines of a replaced multi-line value sit deeper
            // than the key, and the value's closing brace sits level with it.
            let deeper = line.starts_with("         ");
            let closing = line.starts_with("        }");
            if deeper || closing {
                continue;
            }
            skipping = false;
        }
        let indent = line.len() - line.trim_start().len();
        if indent == 4 && line.trim_start().starts_with("mod.themes.") && line.trim_end().ends_with("= {") {
            out.push_str(&format!("    mod.themes.{name} = {{\n"));
            continue;
        }
        if let Some(rest) = line.strip_prefix("        ") {
            if !rest.starts_with(' ') {
                if let Some(colon) = rest.find(':') {
                    let key = &rest[..colon];
                    if let Some((_, value)) = globals.iter().find(|(k, _)| k == key) {
                        out.push_str(&format!("        {key}: {}\n", value.render()));
                        skipping = !rest.trim_end().ends_with('}') && (rest.ends_with('(') || rest.ends_with('{'));
                        continue;
                    }
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The eight keys everything else derives from.
const GLOBAL_KEYS: [&str; 8] = [
    "color_contrast",
    "color_tint",
    "color_tint_amount",
    "space_factor",
    "corner_radius",
    "beveling",
    "font_size_base",
    "font_size_contrast",
];

/// Which of the four sections of a theme file a key is listed under.
fn section_of(name: &str) -> usize {
    if GLOBAL_KEYS.contains(&name) {
        0
    } else if name.starts_with("color_") {
        2
    } else if name.starts_with("font_") {
        3
    } else {
        1
    }
}

/// A complete theme file body from a flat list of values: the header the
/// three in-tree files share, then every value as a literal under the section
/// its name belongs to, in the order given. Every value is a literal, so the
/// result is a snapshot: nothing in it re-derives. Inside this library the
/// first line reads `use crate::makepad_platform::*;`; an app that adopts the
/// file replaces it with `use makepad_widgets::*;`.
pub fn export_theme_source(name: &str, values: &[(String, TokenValue)]) -> String {
    const SECTIONS: [&str; 4] = ["GLOBAL PARAMETERS", "DIMENSIONS", "COLORS", "TYPOGRAPHY"];
    let mut out = String::new();
    out.push_str("use crate::makepad_platform::*;\n\nscript_mod! {\n");
    for module in ["math", "pod", "text", "turtle", "res", "animator"] {
        out.push_str(&format!("    use mod.{module}.*\n"));
    }
    out.push_str(&format!("\n    mod.themes.{name} = {{\n        let theme = me\n"));
    for (index, section) in SECTIONS.iter().enumerate() {
        out.push_str(&format!("        // {section}\n"));
        for (key, value) in values.iter().filter(|(k, _)| section_of(k) == index) {
            out.push_str(&format!("        {key}: {}\n", value.render()));
        }
        if index + 1 < SECTIONS.len() {
            out.push('\n');
        }
    }
    out.push_str("    }\n}\n");
    out
}

// ---------------------------------------------------------------------------
// The theme equalizer: several themes mixed together by weight.
// ---------------------------------------------------------------------------

/// Which appearance group a theme sits in.
///
/// A mix never crosses the two. Half way between a dark theme and a light one
/// is a mid grey page with mid grey text on it: both grounds meet in the
/// middle, and so does the ink each theme chose to stand off its own ground,
/// so the pair arrives at the middle together and nothing reads. There is no
/// weighting of a dark theme and a light one that comes out legible, which is
/// why the equalizer offers one group at a time and `BlendCache::blend`
/// refuses a mix that spans both rather than quietly handing back that grey.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    pub const ALL: [Appearance; 2] = [Appearance::Dark, Appearance::Light];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::Dark => "Dark",
            Appearance::Light => "Light",
        }
    }
}

/// Which base theme each style sheet is written against. A sheet says so in
/// its own first line -- `mod.theme = mod.themes.dark` -- and that line is
/// also the only honest source for its appearance: two of the sheets are dark
/// and carry no `-dark` in their name. `the_sheet_table_is_what_the_sheets_say`
/// holds this copy to the sheets themselves.
const SHEET_BASE: &[(DesktopStyle, bool, Scheme)] = &[
    (DesktopStyle::Omarchy, false, Scheme::Dark),
    (DesktopStyle::BlackOrange, false, Scheme::Dark),
    (DesktopStyle::Macos, false, Scheme::Light),
    (DesktopStyle::Macos, true, Scheme::Dark),
    (DesktopStyle::Windows, false, Scheme::Light),
    (DesktopStyle::Windows, true, Scheme::Dark),
    (DesktopStyle::Windows2000, false, Scheme::Light),
    (DesktopStyle::NextStep, false, Scheme::Light),
    (DesktopStyle::Ios, false, Scheme::Light),
    (DesktopStyle::Ios, true, Scheme::Dark),
    (DesktopStyle::Android, false, Scheme::Light),
    (DesktopStyle::Android, true, Scheme::Dark),
];

/// One ingredient of a mix: a base theme, or a style sheet in one of the
/// appearances it offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendTheme {
    Base(Scheme),
    Sheet(DesktopStyle, bool),
}

impl BlendTheme {
    /// Every theme the equalizer can mix, the three base themes first.
    pub fn all() -> Vec<BlendTheme> {
        let mut out: Vec<BlendTheme> = Scheme::ALL.iter().map(|s| BlendTheme::Base(*s)).collect();
        out.extend(SHEET_BASE.iter().map(|(style, dark, _)| BlendTheme::Sheet(*style, *dark)));
        out
    }

    /// The themes of one appearance group, which is the list a mix is drawn
    /// from: a slider per entry, and nothing from the other group.
    pub fn group(appearance: Appearance) -> Vec<BlendTheme> {
        BlendTheme::all().into_iter().filter(|t| t.appearance() == appearance).collect()
    }

    /// The `mod.themes` object this theme's tokens live in: itself for a base
    /// theme, and for a sheet the theme the sheet assigns into.
    pub fn base(self) -> Scheme {
        match self {
            BlendTheme::Base(scheme) => scheme,
            BlendTheme::Sheet(style, dark) => SHEET_BASE
                .iter()
                .find(|(s, d, _)| *s == style && *d == dark)
                .map(|(_, _, base)| *base)
                .unwrap_or(Scheme::Light),
        }
    }

    /// The group this theme mixes in.
    pub fn appearance(self) -> Appearance {
        match self {
            BlendTheme::Base(Scheme::Dark) => Appearance::Dark,
            // The skeleton is a pale placeholder page, so it belongs with the
            // light group; `the_groups_match_the_grounds` measures that off
            // the resolved themes rather than taking it on trust.
            BlendTheme::Base(_) => Appearance::Light,
            BlendTheme::Sheet(..) => {
                if self.base() == Scheme::Dark {
                    Appearance::Dark
                } else {
                    Appearance::Light
                }
            }
        }
    }

    /// The key under `mod.themes` for a base theme, and the name a sheet is
    /// loaded and installed under.
    pub fn name(self) -> String {
        match self {
            BlendTheme::Base(scheme) => scheme.theme_name().to_string(),
            BlendTheme::Sheet(style, dark) => {
                if dark && style.supports_dark() {
                    format!("{}-dark", style.id())
                } else {
                    style.id().to_string()
                }
            }
        }
    }

    /// A name for the control that drives this theme's weight.
    pub fn label(self) -> String {
        match self {
            BlendTheme::Base(Scheme::Dark) => "Dark".to_string(),
            BlendTheme::Base(Scheme::Light) => "Light".to_string(),
            BlendTheme::Base(Scheme::Skeleton) => "Skeleton".to_string(),
            BlendTheme::Sheet(style, dark) => {
                if dark && style.supports_dark() {
                    format!("{} dark", style.label())
                } else {
                    style.label().to_string()
                }
            }
        }
    }
}

/// Whether averaging this token with another theme's would mean anything.
///
/// The fifteen `color_map_N` series with their eight shades each, and the
/// fourteen `color_syntax_*` keys, are categorical palettes: their members are
/// told apart BY hue, and which hue is which carries the meaning -- the third
/// series of a chart, a string literal, a comment. A palette half way between
/// two palettes is not a palette: every member has drifted toward every other
/// member and toward the mean of the lot, which is the one property a
/// categorical palette exists to avoid. So these are not blended at all. They
/// are taken whole from the argmax theme and read as that theme's, which is
/// 133 of the 557 tokens a theme carries.
pub fn is_categorical(key: &str) -> bool {
    key.starts_with("color_map_") || key.starts_with("color_syntax_")
}

/// How the weights behave when one of them moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeightMode {
    /// Every weight is its own, and the mix is `sum(w_i * T_i) / sum(w_i)`.
    /// Raising one theme leaves the others exactly where they were; what
    /// changes is their share of the total. This is adding an ingredient:
    /// turn a theme up and more of it goes in.
    Absolute,
    /// The weights always add up to `RELATIVE_TOTAL`. Raising one takes from
    /// the anchor -- the theme the mix started from -- and once the anchor is
    /// spent, from all the others in proportion to what each still holds.
    /// Lowering one hands it back to the anchor. This is a budget of a hundred
    /// parts shared out, where a theme's number IS its share of the result.
    Relative,
}

/// What a relative mix always adds up to.
pub const RELATIVE_TOTAL: f64 = 100.0;

/// Move one weight and settle the rest by the rule of the mode.
///
/// `anchor` is the theme the mix started from, which relative mode takes from
/// first; absolute mode ignores it. An index past the end is ignored.
pub fn set_weight(mode: WeightMode, weights: &mut [f64], anchor: usize, index: usize, value: f64) {
    if index >= weights.len() {
        return;
    }
    match mode {
        WeightMode::Absolute => weights[index] = value.max(0.0),
        WeightMode::Relative => relative_set(weights, anchor, index, value),
    }
}

/// Set one weight of a relative mix, keeping the total at `RELATIVE_TOTAL`.
/// The weights handed in are expected to add up to it already; `to_relative`
/// is how a mix gets there in the first place.
pub fn relative_set(weights: &mut [f64], anchor: usize, index: usize, value: f64) {
    if index >= weights.len() {
        return;
    }
    let value = value.clamp(0.0, RELATIVE_TOTAL);
    let mut owed = value - weights[index];
    weights[index] = value;
    let anchor = if anchor < weights.len() && anchor != index { Some(anchor) } else { None };
    let others: Vec<usize> = (0..weights.len()).filter(|i| *i != index && Some(*i) != anchor).collect();
    if owed > 0.0 {
        // Taking: the anchor first, and only then everything else, each in
        // proportion to what it holds.
        let anchor_first = anchor.map(|a| vec![a]).unwrap_or_default();
        owed -= take_proportionally(weights, &anchor_first, owed);
        owed -= take_proportionally(weights, &others, owed);
        // Nothing left anywhere to take: the slider cannot go that high.
        weights[index] -= owed.max(0.0);
    } else if owed < 0.0 {
        // Giving back: to the anchor, which is what a raise took from, and to
        // everything else when this slider IS the anchor.
        let group = match anchor {
            Some(a) => vec![a],
            None => others,
        };
        give_proportionally(weights, &group, -owed);
    }
    // Floating point is the enemy of a conserved total, so the sum gets the
    // last word: whatever it is out by lands on the largest weight, which is
    // always big enough to absorb it.
    let drift = RELATIVE_TOTAL - weights.iter().sum::<f64>();
    if drift != 0.0 && drift.abs() < 1e-6 {
        if let Some(at) = (0..weights.len()).max_by(|a, b| weights[*a].total_cmp(&weights[*b])) {
            weights[at] += drift;
        }
    }
}

/// A set of weights scaled to `RELATIVE_TOTAL`, which is how a relative mix
/// starts: an absolute mix carried over, or everything on the anchor when
/// nothing carries weight at all.
pub fn to_relative(weights: &[f64], anchor: usize) -> Vec<f64> {
    let total: f64 = weights.iter().map(|w| w.max(0.0)).sum();
    if total <= 0.0 {
        let mut out = vec![0.0; weights.len()];
        if let Some(w) = out.get_mut(anchor) {
            *w = RELATIVE_TOTAL;
        }
        return out;
    }
    weights.iter().map(|w| w.max(0.0) / total * RELATIVE_TOTAL).collect()
}

/// Take up to `amount` from `group` in proportion to what each member holds,
/// and report what was actually taken.
fn take_proportionally(weights: &mut [f64], group: &[usize], amount: f64) -> f64 {
    let held: f64 = group.iter().map(|i| weights[*i]).sum();
    if amount <= 0.0 || held <= 0.0 {
        return 0.0;
    }
    let take = amount.min(held);
    for i in group {
        weights[*i] -= weights[*i] / held * take;
    }
    take
}

/// Hand `amount` to `group` in proportion to what each member holds, or
/// evenly when the group holds nothing at all.
fn give_proportionally(weights: &mut [f64], group: &[usize], amount: f64) {
    if group.is_empty() || amount <= 0.0 {
        return;
    }
    let held: f64 = group.iter().map(|i| weights[*i]).sum();
    for i in group {
        weights[*i] += if held > 0.0 {
            weights[*i] / held * amount
        } else {
            amount / group.len() as f64
        };
    }
}

/// The weights as fractions of their total, which is what both modes actually
/// blend by: `sum(w_i * T_i) / sum(w_i)`. All zero when nothing carries
/// weight, which `BlendCache::blend` reports rather than divides by.
pub fn normalized(weights: &[f64]) -> Vec<f64> {
    let total: f64 = weights.iter().map(|w| w.max(0.0)).sum();
    if total <= 0.0 {
        return vec![0.0; weights.len()];
    }
    weights.iter().map(|w| w.max(0.0) / total).collect()
}

/// The narrowest and the widest the random curve may be.
pub const SIGMA_MIN: f64 = 0.6;
pub const SIGMA_MAX: f64 = 4.0;

/// A random mix of `n` themes: pure, seeded, and summing to one.
///
/// The weights fall off a bell from whichever theme the shuffle put first:
/// `w_k = exp(-(k / sigma)^2)`, with `k` a theme's place in a random order.
/// Sigma is itself random, log-uniform between `SIGMA_MIN` and `SIGMA_MAX`, so
/// that the CURVE is what is being randomised: a sharp sigma gives one
/// dominant theme with an influence or two behind it, a wide one gives a
/// spread of five or six. Drawing each weight independently instead gives the
/// same flat mush every time -- the mean of n independent weights concentrates
/// as n grows, so nothing ever dominates and nothing is ever nearly absent.
///
/// Pure and seeded so the same seed is the same mix, here and in a test.
pub fn random_weights(seed: u64, n: usize) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let mut state = seed;
    let sigma = SIGMA_MIN * (SIGMA_MAX / SIGMA_MIN).powf(next_unit(&mut state));
    let mut order: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = (next_u64(&mut state) % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    let mut out = vec![0.0; n];
    for (rank, theme) in order.iter().enumerate() {
        let k = rank as f64 / sigma;
        out[*theme] = (-(k * k)).exp();
    }
    let total: f64 = out.iter().sum();
    for w in out.iter_mut() {
        *w /= total;
    }
    out
}

/// SplitMix64, so the randomiser owes nothing to a crate and nothing to the
/// clock: the same seed is the same sequence on every machine and every run.
fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The next draw as a number in 0..1.
fn next_unit(state: &mut u64) -> f64 {
    (next_u64(state) >> 11) as f64 / (1u64 << 53) as f64
}

/// `mix_rgb` with the alpha channel carried along.
///
/// Theme tokens are not all opaque -- the dark theme's text is white at 65
/// percent, and the whole hidden ladder is a tint -- and `mix_rgb` returns an
/// opaque colour, because the script's own `mix()` on two opaque colours is
/// what it was written for. Blending with it alone turned every translucent
/// token solid, which reads as a different theme rather than a mix of two.
pub fn mix_rgba(a: u32, b: u32, t: f64) -> u32 {
    let alpha = |v: u32| (v & 0xFF) as f64;
    let out = (alpha(a) + (alpha(b) - alpha(a)) * t).round().clamp(0.0, 255.0) as u32;
    (mix_rgb(a, b, t) & 0xFFFF_FF00) | out
}

/// A resolved token value, in the only two kinds a mix can carry.
///
/// Everything else a theme holds -- a `TextStyle`, an `Ease`, the `Inset` of
/// `mspace_1` -- has no midpoint, so it is not cached and not written: it
/// comes from the object the blend script derives from, which is the argmax
/// theme's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendValue {
    Color(u32),
    Num(f64),
}

/// One theme's resolved tokens, by name.
///
/// A theme's values are not in its file: a token is an expression over other
/// tokens, a style sheet then assigns over the lot, and the answer only exists
/// once the module has been evaluated. `resolve_theme` does that, at the cost
/// of a full module reload, which is why a `BlendCache` keeps the answers.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeValues {
    pub theme: BlendTheme,
    values: BTreeMap<String, BlendValue>,
}

impl ThemeValues {
    pub fn new(theme: BlendTheme, values: BTreeMap<String, BlendValue>) -> Self {
        Self { theme, values }
    }

    pub fn get(&self, key: &str) -> Option<BlendValue> {
        self.values.get(key).copied()
    }

    pub fn color(&self, key: &str) -> Option<u32> {
        match self.get(key) {
            Some(BlendValue::Color(c)) => Some(c),
            _ => None,
        }
    }

    pub fn num(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(BlendValue::Num(n)) => Some(n),
            _ => None,
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(|k| k.as_str())
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Why a mix could not be made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlendError {
    /// Every slider is at nought; there is nothing to divide by.
    NoWeight,
    /// A dark theme and a light one in the same mix. The two never blend.
    MixedAppearance(BlendTheme, BlendTheme),
    /// A theme in the mix has not been resolved into the cache yet.
    NotResolved(BlendTheme),
}

impl std::fmt::Display for BlendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlendError::NoWeight => write!(f, "no theme in the mix carries any weight"),
            BlendError::MixedAppearance(a, b) => write!(
                f,
                "{} is {:?} and {} is {:?}; a mix never crosses the two",
                a.name(),
                a.appearance(),
                b.name(),
                b.appearance()
            ),
            BlendError::NotResolved(t) => write!(f, "{} has not been resolved into the cache", t.name()),
        }
    }
}

/// A mix, ready to apply.
///
/// One script, derived from the argmax theme's base object, pinning every
/// token the blend moved. Never a run of `mod.theme.k = v` assignments: those
/// mutate the shared base object for every widget already built from it.
///
/// The values a blend cannot carry -- the fonts, the eases, `mspace_1` -- come
/// from that base object, and only the argmax theme's sheet puts ITS versions
/// of them there, so `argmax_sheet` names the sheet to have in force when the
/// script runs. The caller's three steps:
///
/// ```ignore
/// let blend = cache.blend(&mix)?;                  // one per slider move
/// match blend.argmax_sheet() {                     // whose fonts the mix wears
///     Some((style, dark)) => install(vm, StyleSheet::load_with_appearance(style, dark)),
///     None => uninstall(vm),
/// }
/// vm.eval(/* a ScriptMod carrying */ blend.script("equalized"));
/// cx.request_script_reapply();
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeBlend {
    /// The theme with the largest weight, which lends the mix everything a
    /// blend means nothing for.
    pub argmax: BlendTheme,
    /// The `mod.themes` object the script derives from: the argmax theme's.
    pub base: Scheme,
    /// The group the whole mix came from.
    pub appearance: Appearance,
    /// Every token the script pins, in key order.
    pub overrides: Vec<(String, TokenValue)>,
}

impl ThemeBlend {
    /// The one script to evaluate. `name` is the key the derived theme is
    /// filed under in `mod.themes`; re-running with the same name replaces it.
    pub fn script(&self, name: &str) -> String {
        theme_module_script(name, self.base.theme_name(), &self.overrides)
    }

    /// The sheet to have installed when the script runs, if the argmax theme
    /// is a sheet at all.
    pub fn argmax_sheet(&self) -> Option<(DesktopStyle, bool)> {
        match self.argmax {
            BlendTheme::Sheet(style, dark) => Some((style, dark)),
            BlendTheme::Base(_) => None,
        }
    }

    fn value(&self, key: &str) -> Option<&TokenValue> {
        self.overrides
            .binary_search_by(|(k, _)| k.as_str().cmp(key))
            .ok()
            .map(|at| &self.overrides[at].1)
    }

    /// A blended colour, for a preview or a contrast check.
    pub fn color(&self, key: &str) -> Option<u32> {
        match self.value(key) {
            Some(TokenValue::Color(c)) => Some(*c),
            _ => None,
        }
    }

    /// A blended number.
    pub fn num(&self, key: &str) -> Option<f64> {
        match self.value(key) {
            Some(TokenValue::Num(n)) => Some(*n),
            _ => None,
        }
    }
}

/// The resolved themes a mix is made from.
///
/// Resolving costs a module reload per theme, so it happens once per theme and
/// the answers are kept here; a slider move then blends from the cache and
/// touches no VM at all.
#[derive(Clone, Debug, Default)]
pub struct BlendCache {
    resolved: Vec<ThemeValues>,
}

impl BlendCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, theme: BlendTheme) -> Option<&ThemeValues> {
        self.resolved.iter().find(|v| v.theme == theme)
    }

    pub fn insert(&mut self, values: ThemeValues) {
        self.resolved.retain(|v| v.theme != values.theme);
        self.resolved.push(values);
    }

    pub fn themes(&self) -> impl Iterator<Item = BlendTheme> + '_ {
        self.resolved.iter().map(|v| v.theme)
    }

    pub fn len(&self) -> usize {
        self.resolved.len()
    }

    pub fn is_empty(&self) -> bool {
        self.resolved.is_empty()
    }

    /// Resolve every theme of `themes` that is not in the cache yet, and put
    /// the VM back the way it was found: whatever sheet was installed goes
    /// back on, and the module is reloaded, so the caller can apply a blend on
    /// top of a VM that is none the wiser.
    pub fn fill(&mut self, vm: &mut ScriptVm, themes: &[BlendTheme]) {
        let restore = crate::desktop_style::current(vm);
        let mut resolved_any = false;
        for theme in themes {
            if self.get(*theme).is_some() {
                continue;
            }
            let values = resolve_theme(vm, *theme);
            self.insert(values);
            resolved_any = true;
        }
        if resolved_any {
            match restore {
                Some(sheet) => crate::desktop_style::install(vm, sheet),
                None => crate::desktop_style::uninstall(vm),
            }
            vm.with_reload(crate::script_mod);
        }
    }

    /// Mix the themes of `mix` by their weights.
    ///
    /// The weights need not be normalised and need not be in any mode: what is
    /// blended is `sum(w_i * T_i) / sum(w_i)` either way, and the two modes
    /// differ only in what happens to the OTHER sliders when one of them
    /// moves (see `WeightMode`). A weight of nought drops its theme out
    /// entirely, so a mix of one theme at any weight is that theme exactly,
    /// byte for byte, with no rounding anywhere.
    ///
    /// Colours mix through `mix_rgba`, numbers linearly. The categorical
    /// palettes (`is_categorical`) are not mixed at all but taken from the
    /// argmax theme. Then the library's own roles are derived again from the
    /// blended tokens, because the choices in them -- which ink goes on which
    /// ground -- are thresholds, and a threshold does not survive an average:
    /// two themes whose text reads on their own page can average into one
    /// whose does not.
    pub fn blend(&self, mix: &[(BlendTheme, f64)]) -> Result<ThemeBlend, BlendError> {
        let mut parts: Vec<(&ThemeValues, f64)> = Vec::new();
        for (theme, weight) in mix {
            if !(*weight > 0.0) {
                continue;
            }
            let values = self.get(*theme).ok_or(BlendError::NotResolved(*theme))?;
            parts.push((values, *weight));
        }
        let Some((first, _)) = parts.first().copied() else {
            return Err(BlendError::NoWeight);
        };
        let appearance = first.theme.appearance();
        for (values, _) in &parts {
            if values.theme.appearance() != appearance {
                return Err(BlendError::MixedAppearance(first.theme, values.theme));
            }
        }
        // The heaviest theme, and the FIRST of them on a tie, so that an even
        // mix does not change its mind about whose fonts it wears when the
        // list is rebuilt.
        let mut argmax = first;
        let mut best = parts[0].1;
        for (values, weight) in &parts[1..] {
            if *weight > best {
                best = *weight;
                argmax = values;
            }
        }
        let mut keys: BTreeSet<&str> = BTreeSet::new();
        for (values, _) in &parts {
            keys.extend(values.keys());
        }
        let mut blended: BTreeMap<String, BlendValue> = BTreeMap::new();
        for key in keys {
            if is_categorical(key) {
                if let Some(value) = argmax.get(key) {
                    blended.insert(key.to_string(), value);
                }
                continue;
            }
            // A running weighted mean: each theme in turn, mixed in at its
            // share of the weight so far. The first one lands whole, so one
            // ingredient is itself and nothing is rounded on the way.
            let mut acc: Option<BlendValue> = None;
            let mut total = 0.0;
            for (values, weight) in &parts {
                let Some(value) = values.get(key) else {
                    continue;
                };
                total += weight;
                let t = weight / total;
                acc = Some(match acc {
                    None => value,
                    Some(sofar) => blend_value(sofar, value, t),
                });
            }
            if let Some(value) = acc {
                blended.insert(key.to_string(), value);
            }
        }
        // The roles, off the blended tokens. The brand families are left as
        // they were blended: every ingredient already carries a considered
        // one, and the base themes' accent token is a focus blue that has
        // never been their `color_primary`. A surface rung the mix already
        // carries is left alone too: the rule that derives it is linear, so
        // blending the rung and deriving it from the blended ground are the
        // same answer -- all but the rounding, and the script's own `mix()`
        // does not round a half the way `mix_rgb` does, which put a lone dark
        // theme one step off itself on three rungs. What is always asked
        // again is the ink: which colour goes on which ground is a threshold,
        // and a threshold is exactly what does not survive an average.
        let roles = {
            let mut named = |key: &str| {
                DERIVED_ROLES.iter().any(|(role, _, _)| *role == key) && blended.contains_key(key)
            };
            let mut read = |key: &str| match blended.get(key) {
                Some(BlendValue::Color(c)) => Some(*c),
                _ => None,
            };
            derived_roles(
                appearance == Appearance::Dark,
                RoleGrowth::KeepAccent,
                &mut named,
                &mut read,
            )
        };
        for (key, rgba) in roles {
            blended.insert(key, BlendValue::Color(rgba));
        }
        let overrides = blended
            .into_iter()
            .map(|(key, value)| {
                let value = match value {
                    BlendValue::Color(c) => TokenValue::Color(c),
                    BlendValue::Num(n) => TokenValue::Num(n),
                };
                (key, value)
            })
            .collect();
        Ok(ThemeBlend {
            argmax: argmax.theme,
            base: argmax.theme.base(),
            appearance,
            overrides,
        })
    }
}

/// Two values of one token, mixed by `t`.
fn blend_value(a: BlendValue, b: BlendValue, t: f64) -> BlendValue {
    match (a, b) {
        (BlendValue::Color(a), BlendValue::Color(b)) => BlendValue::Color(mix_rgba(a, b, t)),
        (BlendValue::Num(a), BlendValue::Num(b)) => BlendValue::Num(a + (b - a) * t),
        // A key that is a colour in one theme and a number in another is not a
        // token the two share; the heavier side keeps it rather than the two
        // being forced into one kind.
        (a, b) => {
            if t >= 0.5 {
                b
            } else {
                a
            }
        }
    }
}

/// Every top-level key of a theme file: the lines at exactly eight spaces of
/// indent that open with `name:`.
pub fn theme_keys(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("        ")?;
            let first = rest.chars().next()?;
            if !first.is_ascii_lowercase() {
                return None;
            }
            let key = &rest[..rest.find(':')?];
            if key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                Some(key)
            } else {
                None
            }
        })
        .collect()
}

/// Every key the three theme files define between them, in file order and
/// without repeats: the tokens a mix can carry.
pub fn base_theme_keys() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for scheme in Scheme::ALL {
        for key in theme_keys(scheme.source()) {
            if !out.contains(&key) {
                out.push(key);
            }
        }
    }
    out
}

/// The keys a style sheet assigns, as `mod.theme.<key> = ...`. A sheet may
/// introduce a token no base theme file defines -- seventeen of them do --
/// and a mix that read only the base files would drop those on the floor.
pub fn assigned_keys(sheet_theme: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for line in sheet_theme.lines() {
        let Some(rest) = line.trim_start().strip_prefix("mod.theme.") else {
            continue;
        };
        let end = rest
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len());
        let (key, tail) = rest.split_at(end);
        if !key.is_empty() && tail.trim_start().starts_with('=') && !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// Resolve one theme's tokens in a script VM.
///
/// Installs the sheet (or takes one off), reloads the widget module, and reads
/// every key back off `mod.theme`. The reload is not optional: a sheet assigns
/// into the base theme OBJECT, so a theme read after another one had its sheet
/// on would be wearing half of it.
pub fn resolve_theme(vm: &mut ScriptVm, theme: BlendTheme) -> ThemeValues {
    let sheet = match theme {
        BlendTheme::Base(_) => {
            crate::desktop_style::uninstall(vm);
            None
        }
        BlendTheme::Sheet(style, dark) => {
            let sheet = crate::desktop_style::StyleSheet::load_with_appearance(style, dark);
            crate::desktop_style::install(vm, sheet.clone());
            Some(sheet)
        }
    };
    vm.with_reload(crate::script_mod);
    // A reload leaves `mod.theme` on the dark theme; a sheet has already moved
    // it to its own base.
    match theme {
        BlendTheme::Base(Scheme::Light) => {
            script_eval!(vm, {
                mod.theme = mod.themes.light
            });
        }
        BlendTheme::Base(Scheme::Skeleton) => {
            script_eval!(vm, {
                mod.theme = mod.themes.skeleton
            });
        }
        _ => {}
    }
    let mut keys: Vec<String> = base_theme_keys().iter().map(|k| k.to_string()).collect();
    if let Some(sheet) = &sheet {
        for key in assigned_keys(&sheet.theme) {
            if !keys.iter().any(|k| k == key) {
                keys.push(key.to_string());
            }
        }
    }
    let object = vm.module(LiveId::from_str("theme"));
    let mut values = BTreeMap::new();
    for key in keys {
        let value = vm.bx.heap.value(object, LiveId::from_str(&key).into(), NoTrap);
        if let Some(rgba) = value.as_color() {
            values.insert(key, BlendValue::Color(rgba));
        } else if let Some(number) = value.as_number() {
            values.insert(key, BlendValue::Num(number));
        }
    }
    ThemeValues { theme, values }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table is a second copy of what the theme files say, and a second
    /// copy is only safe while something fails when the first one moves.
    /// The ground error and warning text is drawn on is not the rule's; it
    /// is whatever the theme files keep those two names pointing at.
    #[test]
    fn error_and_warning_are_the_older_colours() {
        for scheme in [Scheme::Dark, Scheme::Light, Scheme::Skeleton] {
            let src = scheme.source();
            let has = |line: &str| src.lines().any(|l| l.trim() == line);
            assert!(has("color_high: #C00") || has("color_high: #xCC0000FF"), "{}", scheme.theme_name());
            assert!(has("color_mid: #FA0") || has("color_mid: #xFFAA00FF"), "{}", scheme.theme_name());
            assert!(has("color_error: theme.color_high") || has("color_error: #xCC0000FF"), "{}", scheme.theme_name());
            assert!(has("color_warning: theme.color_mid") || has("color_warning: #xFFAA00FF"), "{}", scheme.theme_name());
        }
        assert_eq!((KEPT_ERROR, KEPT_WARNING), (0xCC0000FF, 0xFFAA00FF));
    }

    /// The ground a family's text really sits on, which for two of them is
    /// not the family's own base.
    fn ground_of(key: &str, base: u32) -> u32 {
        match key {
            "color_on_error" => KEPT_ERROR,
            "color_on_warning" => KEPT_WARNING,
            _ => base,
        }
    }

    fn unreadable(roles: &ColorRoles) -> Vec<String> {
        let entries = roles.entries();
        let mut out = Vec::new();
        for quad in entries.chunks(4).take(7) {
            let [(_, base), (on_key, on_base), (_, container), (on_c_key, on_container)] = [quad[0], quad[1], quad[2], quad[3]];
            for (key, ink, ground) in [(on_key, on_base, ground_of(on_key, base)), (on_c_key, on_container, container)] {
                let ratio = contrast(ink, ground);
                if ratio < READABLE {
                    out.push(format!("{key} {ink:08X} on {ground:08X} is {ratio:.2}:1"));
                }
            }
        }
        out
    }

    /// White on the warning colour stood at 1.9:1 in every light theme and
    /// the dark theme's error ink at 2.6:1, and the test that pinned those
    /// values compared them only to the rule that made them.
    #[test]
    fn everything_drawn_on_an_accent_reads() {
        for scheme in [Scheme::Dark, Scheme::Light, Scheme::Skeleton] {
            let bad = unreadable(&roles_for(scheme));
            assert!(bad.is_empty(), "{}: {bad:#?}", scheme.theme_name());
        }
    }

    /// A style sheet's accent is a hue nobody chose with this rule in mind,
    /// so the rule has to hold all the way round, not at the house seed.
    #[test]
    fn it_reads_whatever_hue_the_accent_is() {
        for scheme in [Scheme::Dark, Scheme::Light] {
            for step in 0..36 {
                let seed = SeedColors { primary: hsl_to_rgb(step as f64 * 10.0, 0.85, 0.5), ..SeedColors::HOUSE };
                let bad = unreadable(&roles_from_seed(&seed, scheme));
                assert!(bad.is_empty(), "{} at hue {}: {bad:#?}", scheme.theme_name(), step * 10);
            }
            // And a sheet whose accent is no hue at all.
            for grey in [0x000000FFu32, 0x808080FF, 0xFFFFFFFF] {
                let bad = unreadable(&roles_from_seed(&SeedColors { primary: grey, ..SeedColors::HOUSE }, scheme));
                assert!(bad.is_empty(), "{} at grey {grey:08X}: {bad:#?}", scheme.theme_name());
            }
        }
    }

    #[test]
    fn an_accent_with_no_colour_grows_a_family_with_none() {
        let grey = |rgba: u32| { let (r, g, b) = ((rgba >> 24) & 0xFF, (rgba >> 16) & 0xFF, (rgba >> 8) & 0xFF); r == g && g == b };
        for seed in [0x000000FFu32, 0x808080FF, 0xFFFFFFFF] {
            for scheme in [Scheme::Dark, Scheme::Light] {
                let roles = roles_from_seed(&SeedColors { primary: seed, ..SeedColors::HOUSE }, scheme);
                for (key, rgba) in roles.entries() {
                    if ["primary", "secondary", "tertiary"].iter().any(|f| key.contains(f)) {
                        assert!(grey(rgba), "{key} grown from {seed:08X} came out {rgba:08X}, which has a colour in it");
                    }
                }
                // Red still means an error, whatever the accent is.
                assert!(!grey(roles.entries()[14].1), "the error container keeps its red");
            }
        }
        // The house seed is unchanged by any of this.
        assert_eq!(roles_for(Scheme::Light).entries()[0].1, 0xA53D27FF);
    }

    #[test]
    fn the_rule_keeps_its_own_choice_where_that_reads() {
        // A red: white reads on it, so white stays.
        assert_eq!(readable_on(0xA53D27FF, WHITE, 0x32120CFF), WHITE);
        // An amber: white does not, and the ink does.
        assert_eq!(readable_on(0xFFAA00FF, WHITE, 0x392905FF), 0x392905FF);
        // A mid grey that neither end reads on falls to black or white.
        assert_eq!(readable_on(0x777777FF, 0x888888FF, 0x666666FF), BLACK);
    }

    #[test]
    fn the_derived_role_table_is_the_theme_files() {
        for (scheme, pick) in [(Scheme::Light, 0usize), (Scheme::Dark, 1usize)] {
            let source = scheme.source();
            for (role, light, dark) in DERIVED_ROLES {
                let want = if pick == 0 { light.expression() } else { dark.expression() };
                let line = format!("{role}: {want}");
                assert!(
                    source.lines().any(|l| l.trim() == line),
                    "{} theme: expected the line `{line}`",
                    scheme.theme_name()
                );
            }
        }
    }

    /// The value a generated script writes for a role.
    fn written(script: &str, key: &str) -> u32 {
        let line = script
            .lines()
            .find(|l| l.starts_with(&format!("mod.theme.{key} = ")))
            .unwrap_or_else(|| panic!("{key} not written:
{script}"));
        u32::from_str_radix(line.rsplit("#x").next().unwrap(), 16).unwrap()
    }

    fn a_sheet_in_beige_and_navy(key: &str) -> Option<u32> {
        match key {
            "color_bg_app" | "color_fg_app" => Some(0xD4D0C8FF),
            "color_text" => Some(0x000000FF),
            "color_ctrl_selected" => Some(0x000080FF),
            "color_opaque_d_1" => Some(0xB4B1AAFF),
            _ => None,
        }
    }

    #[test]
    fn a_sheet_moves_the_surfaces_and_grows_its_own_accent() {
        let sheet = "mod.theme = mod.themes.light
mod.theme.color_bg_app = #d4d0c8
";
        let script = sheet_roles_script(sheet, &mut a_sheet_in_beige_and_navy);
        // The surface is the ground the sheet set, not the one it replaced.
        assert!(script.contains("mod.theme.color_surface = #xD4D0C8FF
"), "{script}");
        assert!(script.contains("mod.theme.color_on_surface = #x000000FF
"), "{script}");
        // The whole ladder follows the sheet, because it is written against
        // `color_fg_app`, which a sheet sets: it used to alias the opaque
        // ladder, which no sheet sets, so every rung above a sheet's own
        // surfaces kept the base theme's greys.
        assert!(script.contains("mod.theme.color_surface_bright = "), "{script}");
        // A token the sheet left unanswerable is still left alone rather
        // than written as nothing.
        assert!(!script.contains("color_inverse_surface"), "{script}");
        // The accent is grown from the sheet's navy: blue, where the house
        // accent is an orange red.
        let value = |key: &str| written(&script, key);
        let (hue, _, _) = rgb_to_hsl(value("color_primary"));
        assert!((220.0..=260.0).contains(&hue), "primary hue {hue} should be the sheet's blue");
        let (house_hue, _, _) = rgb_to_hsl(roles_for(Scheme::Light).entries()[0].1);
        assert!((house_hue - hue).abs() > 100.0, "and nowhere near the house accent at {house_hue}");
        // What is drawn on it is settled by the same rule, and reads.
        let (_, _, on) = rgb_to_hsl(value("color_on_primary"));
        let (_, _, base) = rgb_to_hsl(value("color_primary"));
        assert!((on - base).abs() > 0.4, "text on the accent has to stand off it: {on} on {base}");
        // Red is an error under every style.
        assert!(!script.contains("color_error"), "{script}");
        assert!(!script.contains("color_success"), "{script}");
    }

    #[test]
    fn what_is_drawn_on_a_status_colour_is_asked_of_the_sheets_own() {
        // The light theme's black, on the darker green a sheet brought.
        let mut read = |key: &str| match key {
            "color_success" => Some(0x2F7D4BFF),
            "color_on_success" => Some(0x000000FF),
            // One that already reads is not rewritten.
            "color_error" => Some(0xCC0000FF),
            "color_on_error" => Some(0xFFFFFFFF),
            _ => None,
        };
        let script = sheet_roles_script("mod.theme = mod.themes.light
", &mut read);
        assert!(script.contains("mod.theme.color_on_success = #xFFFFFFFF
"), "{script}");
        assert!(!script.contains("color_on_error"), "{script}");
        assert!(contrast(0xFFFFFFFF, 0x2F7D4BFF) >= READABLE && contrast(0x000000FF, 0x2F7D4BFF) < READABLE);
    }

    #[test]
    fn a_role_the_sheet_names_is_the_sheets_to_decide() {
        let sheet = "mod.theme = mod.themes.light
mod.theme.color_primary = #ff0000
mod.theme.color_surface=#123456
";
        let script = sheet_roles_script(sheet, &mut a_sheet_in_beige_and_navy);
        assert!(!script.contains("mod.theme.color_primary ="), "{script}");
        assert!(!script.contains("mod.theme.color_surface ="), "{script}");
        // Its neighbours are still grown, and a longer name that merely
        // starts the same way is not mistaken for the one the sheet set.
        assert!(script.contains("mod.theme.color_on_primary ="), "{script}");
        assert!(script.contains("mod.theme.color_primary_container ="), "{script}");
        assert!(script.contains("mod.theme.color_surface_container ="), "{script}");
    }

    #[test]
    fn a_dark_sheet_reads_the_dark_themes_sources() {
        let mut read = |key: &str| match key {
            "color_fg_app" => Some(0x202020FF),
            _ => None,
        };
        // The dark rule lifts the sheet's own foreground toward white and
        // the light rule takes it down toward black. Reading the wrong one
        // is how a dark sheet ended up wearing the light theme's greys.
        let dark = sheet_roles_script("mod.theme = mod.themes.dark
", &mut read);
        let light = sheet_roles_script("mod.theme = mod.themes.light
", &mut read);
        let rung = |script: &str| written(script, "color_surface_container_high");
        assert!(rung(&dark) > 0x202020FF, "{dark}");
        assert!(rung(&light) < 0x202020FF, "{light}");
    }

    /// The keys a theme file defines at its top level, which the equalizer
    /// needs too and so keeps.
    fn own_keys(source: &str) -> Vec<&str> {
        theme_keys(source)
    }

    /// Whether a key falls under one of the prefixes the token programme
    /// introduced: `radius_ elevation_ motion_ state_ type_ size_ space_[4-9]`
    /// and the `color_` roles.
    fn is_new_prefix(key: &str) -> bool {
        const PLAIN: &[&str] = &[
            "radius_", "elevation_", "motion_", "state_", "type_", "size_", "font_title_", "font_body_",
            "font_label_",
        ];
        if PLAIN.iter().any(|p| key.starts_with(p)) {
            return true;
        }
        if let Some(rest) = key.strip_prefix("font_size_") {
            return rest == "5" || rest == "6";
        }
        if let Some(rest) = key.strip_prefix("space_") {
            return rest.starts_with(|c: char| ('4'..='9').contains(&c));
        }
        if let Some(rest) = key.strip_prefix("color_") {
            const ROLES: &[&str] = &[
                "on_", "primary", "secondary", "tertiary", "error_", "warning_", "success", "info", "surface",
                "outline", "inverse", "scrim", "elevation", "presence", "placeholder",
            ];
            return ROLES.iter().any(|p| rest.starts_with(p));
        }
        false
    }

    fn literal_after(source: &str, name: &str) -> Option<String> {
        let needle = format!("\n        {name}: #x");
        let at = source.find(&needle)? + needle.len();
        Some(source[at..at + 8].to_string())
    }

    #[test]
    fn registry_names_are_unique() {
        for (i, a) in THEME_TOKENS.iter().enumerate() {
            assert!(THEME_TOKENS[..i].iter().all(|b| b.name != a.name), "{} registered twice", a.name);
        }
    }

    #[test]
    fn every_registered_token_is_defined_once_in_its_themes() {
        for scheme in Scheme::ALL {
            let source = scheme.source();
            for token in THEME_TOKENS.iter().filter(|t| t.themes.contains(&scheme)) {
                let needle = format!("\n        {}:", token.name);
                let n = source.matches(&needle).count();
                assert_eq!(n, 1, "{} defined {} times in {}", token.name, n, scheme.theme_name());
            }
        }
    }

    #[test]
    fn every_new_prefix_key_is_registered() {
        for scheme in Scheme::ALL {
            for key in own_keys(scheme.source()) {
                if !is_new_prefix(key) {
                    continue;
                }
                let spec = token_spec(key)
                    .unwrap_or_else(|| panic!("{key} in {} is not registered", scheme.theme_name()));
                assert!(
                    spec.themes.contains(&scheme),
                    "{key} is defined in {} but not registered for it",
                    scheme.theme_name()
                );
            }
        }
    }

    #[test]
    fn accent_role_literals_match_the_generator() {
        for scheme in Scheme::ALL {
            let source = scheme.source();
            for (name, rgba) in roles_for(scheme).entries() {
                if name == "color_error" || name == "color_warning" {
                    continue;
                }
                let found = literal_after(source, name)
                    .unwrap_or_else(|| panic!("{name} has no #x literal in {}", scheme.theme_name()));
                assert_eq!(found, format!("{rgba:08X}"), "{name} in {}", scheme.theme_name());
            }
        }
    }

    #[test]
    fn skeleton_ladder_literals_match_the_generator() {
        let source = Scheme::Skeleton.source();
        for (name, rgba) in skeleton_ladder() {
            let found = literal_after(source, name).unwrap_or_else(|| panic!("{name} has no #x literal"));
            assert_eq!(found, format!("{rgba:08X}"), "{name}");
        }
    }

    #[test]
    fn seed_hue_is_the_house_orange() {
        assert!((seed_hue() - 10.606).abs() < 0.01, "{}", seed_hue());
        assert_eq!(hsl_to_rgb(0.0, 0.0, 0.35), 0x595959FF);
        assert_eq!(rgb_to_hsl(0x0000FFFF).0, 240.0);
    }

    #[test]
    fn theme_module_script_renders_a_known_sample() {
        let overrides = [
            ("color_primary".to_string(), TokenValue::Color(0xFF5C39FF)),
            ("radius_m".to_string(), TokenValue::Num(6.0)),
            ("motion_ease_standard".to_string(), TokenValue::Raw("Ease.Linear".to_string())),
        ];
        assert_eq!(
            theme_module_script("mine", "dark", &overrides),
            "mod.themes.mine = mod.themes.dark{ color_primary: #xFF5C39FF radius_m: 6.0 motion_ease_standard: Ease.Linear }\nmod.theme = mod.themes.mine\n"
        );
    }

    #[test]
    fn theme_source_with_globals_substitutes_only_the_globals() {
        let globals = [
            ("space_factor".to_string(), TokenValue::Num(8.0)),
            ("color_tint".to_string(), TokenValue::Color(0x00FF00FF)),
            ("color_bg_app".to_string(), TokenValue::Color(0x101010FF)),
        ];
        let out = theme_source_with_globals("mine", Scheme::Dark.source(), &globals);
        assert!(out.contains("    mod.themes.mine = {\n"));
        assert!(!out.contains("mod.themes.dark"));
        assert!(out.contains("\n        space_factor: 8.0\n"));
        assert!(out.contains("\n        color_tint: #x00FF00FF\n"));
        assert!(out.contains("\n        color_bg_app: #x101010FF\n        color_fg_app: mix(\n"));
        assert!(out.contains("\n        space_2: 1.0 * theme.space_factor\n"));
        assert!(out.contains("\n        font_label: TextStyle{\n"));
        assert_eq!(out.lines().count(), Scheme::Dark.source().lines().count() - 3);
    }

    #[test]
    fn export_theme_source_is_a_complete_theme_body() {
        let values = [
            ("font_size_base".to_string(), TokenValue::Num(11.0)),
            ("radius_m".to_string(), TokenValue::Num(6.0)),
            ("color_primary".to_string(), TokenValue::Color(0xA53D27FF)),
            ("font_body_m".to_string(), TokenValue::Raw("theme.font_regular{font_size: 11.0}".to_string())),
        ];
        let out = export_theme_source("mine", &values);
        for module in ["math", "pod", "text", "turtle", "res", "animator"] {
            assert!(out.contains(&format!("    use mod.{module}.*\n")));
        }
        assert!(out.contains("    mod.themes.mine = {\n        let theme = me\n"));
        let global = out.find("// GLOBAL PARAMETERS").unwrap();
        let dims = out.find("// DIMENSIONS").unwrap();
        let colors = out.find("// COLORS").unwrap();
        let fonts = out.find("// TYPOGRAPHY").unwrap();
        assert!(global < dims && dims < colors && colors < fonts);
        let base = out.find("        font_size_base: 11.0\n").unwrap();
        let radius = out.find("        radius_m: 6.0\n").unwrap();
        let primary = out.find("        color_primary: #xA53D27FF\n").unwrap();
        let body = out.find("        font_body_m: theme.font_regular{font_size: 11.0}\n").unwrap();
        assert!(global < base && base < dims);
        assert!(dims < radius && radius < colors);
        assert!(colors < primary && primary < fonts);
        assert!(fonts < body);
        assert!(out.ends_with("    }\n}\n"));
    }
}

#[cfg(test)]
mod sheet_contrast_tests {
    use super::*;
    use crate::desktop_style::{install, uninstall, DesktopStyle, StyleSheet};
    use crate::makepad_platform::*;
    use crate::script_eval;

    /// Every sheet the library ships, in both appearances it offers.
    pub(super) const SHEETS: &[(DesktopStyle, bool)] = &[
        (DesktopStyle::Omarchy, false),
        (DesktopStyle::BlackOrange, false),
        (DesktopStyle::Macos, false),
        (DesktopStyle::Macos, true),
        (DesktopStyle::Windows, false),
        (DesktopStyle::Windows, true),
        (DesktopStyle::Windows2000, false),
        (DesktopStyle::NextStep, false),
        (DesktopStyle::Ios, false),
        (DesktopStyle::Ios, true),
        (DesktopStyle::Android, false),
        (DesktopStyle::Android, true),
    ];

    /// The four families that mean something, and the accent, each with the
    /// ink meant to be drawn on it.
    pub(super) const MEANING: &[(&str, &str)] = &[
        ("color_success", "color_on_success"),
        ("color_warning", "color_on_warning"),
        ("color_error", "color_on_error"),
        ("color_info", "color_on_info"),
        ("color_primary", "color_on_primary"),
    ];

    /// Every rung of the surface ladder, against the body ink.
    pub(super) const SURFACES: &[(&str, &str)] = &[
        ("color_surface", "color_on_surface"),
        ("color_surface_container", "color_on_surface"),
        ("color_surface_container_low", "color_on_surface"),
        ("color_surface_container_high", "color_on_surface"),
        ("color_surface_container_highest", "color_on_surface"),
        ("color_surface_dim", "color_on_surface"),
        ("color_surface_bright", "color_on_surface"),
    ];

    /// The same rungs against the second voice, which is held to `LEGIBLE`.
    pub(super) const VARIANTS: &[(&str, &str)] = &[
        ("color_surface", "color_on_surface_variant"),
        ("color_surface_container", "color_on_surface_variant"),
        ("color_surface_container_high", "color_on_surface_variant"),
        ("color_surface_container_highest", "color_on_surface_variant"),
    ];

    fn val(vm: &mut ScriptVm, key: &str) -> Option<u32> {
        let theme = vm.module(id!(theme));
        vm.bx.heap.value(theme, LiveId::from_str(key).into(), NoTrap).as_color()
    }

    /// How the pair really reads. An ink is often the theme's text colour,
    /// which carries an alpha, so it is laid over its ground before being
    /// measured: white at 65% on a mid grey is not white.
    fn reads(ground: u32, ink: u32) -> f64 {
        reads_on(ground, ink)
    }

    /// The pairs that fail, as readable lines.
    fn failures(vm: &mut ScriptVm, label: &str, pairs: &[(&str, &str)], need: f64) -> Vec<String> {
        let mut out = Vec::new();
        for (ground, ink) in pairs {
            if let (Some(g), Some(i)) = (val(vm, ground), val(vm, ink)) {
                let c = reads(g | 0xFF, i);
                if c < need {
                    out.push(format!("{label}: {ink} on {ground} = {c:.2}, wanted {need}"));
                }
            }
        }
        out
    }

    /// Walks the base themes and then every sheet, handing each to `check`.
    fn walk(check: &mut dyn FnMut(&mut ScriptVm, &str)) {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(|vm| {
            crate::script_mod(vm);
            check(vm, "dark");
            script_eval!(vm, {
                mod.theme = mod.themes.light
            });
            check(vm, "light");
            script_eval!(vm, {
                mod.theme = mod.themes.skeleton
            });
            check(vm, "skeleton");
            for (style, dark) in SHEETS {
                install(vm, StyleSheet::load_with_appearance(*style, *dark));
                vm.bx.captured_errors = Some(Vec::new());
                vm.with_reload(crate::script_mod);
                let errors = vm.take_errors();
                let label = StyleSheet::load_with_appearance(*style, *dark).name;
                assert!(errors.is_empty(), "{label}: {errors:?}");
                check(vm, &label);
            }
            uninstall(vm);
        });
    }

    /// Red is an error under every style, but the ground a sheet gives that
    /// name is its own, and what reads on ours may not read on theirs: black
    /// chosen for our green stood at 4.2:1 on the darker green six sheets
    /// use. The derivation asks the ground that is actually there, and this
    /// holds it to the answer.
    #[test]
    fn a_meaning_family_reads_on_its_own_ground_under_every_sheet() {
        let mut bad: Vec<String> = Vec::new();
        walk(&mut |vm, label| bad.extend(failures(vm, label, MEANING, READABLE)));
        assert!(bad.is_empty(), "text below {READABLE}:1 on its own ground:
{}", bad.join("
"));
    }

    /// The last statement of an evaluated script is swallowed, so a derived
    /// script that ends on an assignment loses it, silently and without an
    /// error: the role stayed at whatever it was before the sheet. Every
    /// sheet the library ships ends with the same bare `true` for the same
    /// reason. Read the sheets rather than trusting the memory of it.
    #[test]
    fn a_derived_script_ends_in_a_statement_it_can_afford_to_lose() {
        let script = sheet_roles_script("mod.theme = mod.themes.light
", &mut |_| Some(0x808080FF));
        assert_eq!(script.lines().last(), Some("true"), "{script}");
        for (style, dark) in SHEETS {
            let sheet = StyleSheet::load_with_appearance(*style, *dark);
            assert_eq!(sheet.theme.lines().last().map(str::trim), Some("true"), "{}", sheet.name);
        }
    }

    /// A raised surface is still a surface somebody reads off. Every rung of
    /// the ladder is a ground in the library -- the high one alone is the
    /// ground of thirty-one widget files -- and the ink on it has to hold up
    /// on all of them, in every theme and under every sheet.
    #[test]
    fn the_surface_ladder_carries_its_ink_on_every_rung() {
        let mut bad: Vec<String> = Vec::new();
        walk(&mut |vm, label| {
            bad.extend(failures(vm, label, SURFACES, READABLE));
            bad.extend(failures(vm, label, VARIANTS, LEGIBLE));
        });
        assert!(bad.is_empty(), "ink that does not hold on its rung:
{}", bad.join("
"));
    }

    /// Every ground a widget draws text on, in every theme and every sheet,
    /// with the ink that goes on it. Not an assertion: the surface ladder
    /// does not pass yet, and the numbers are the input to fixing it.
    /// `cargo test -p makepad-widgets contrast_audit -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn contrast_audit() {
        let mut lines: Vec<String> = Vec::new();
        walk(&mut |vm, label| {
            let bar = |pairs: &[(&str, &str)]| {
                if std::ptr::eq(pairs.as_ptr(), VARIANTS.as_ptr()) { LEGIBLE } else { READABLE }
            };
            for pairs in [MEANING, SURFACES, VARIANTS] {
                let need = bar(pairs);
                for (ground, ink) in pairs {
                    if let (Some(g), Some(i)) = (val(vm, ground), val(vm, ink)) {
                        let c = reads(g | 0xFF, i);
                        lines.push(format!(
                            "{label:<14} {ground:<32} {ink:<24} #{:06X} on #{:06X} = {c:5.2} (needs {need}){}",
                            i >> 8,
                            g >> 8,
                            if c < need { "  FAIL" } else { "" }
                        ));
                    }
                }
            }
        });
        let failed = lines.iter().filter(|l| l.ends_with("FAIL")).count();
        println!("{}", lines.join("
"));
        println!("{failed} of {} pairs below the bar for their kind", lines.len());
    }
}

#[cfg(test)]
mod equalizer_tests {
    use super::sheet_contrast_tests::{MEANING, SURFACES, VARIANTS};
    use super::*;
    use crate::desktop_style::StyleSheet;
    use crate::makepad_platform::Cx;
    use std::collections::BTreeMap;
    use std::sync::OnceLock;

    /// A theme invented for the maths: the tokens the role derivation reads,
    /// two numbers, and one member of each categorical palette. No VM, so the
    /// blending itself can be checked at speed and by hand.
    fn made_up(theme: BlendTheme, colors: &[(&str, u32)], numbers: &[(&str, f64)]) -> ThemeValues {
        let mut values = BTreeMap::new();
        for (key, rgba) in colors {
            values.insert(key.to_string(), BlendValue::Color(*rgba));
        }
        for (key, number) in numbers {
            values.insert(key.to_string(), BlendValue::Num(*number));
        }
        ThemeValues::new(theme, values)
    }

    const NEAR_BLACK: BlendTheme = BlendTheme::Base(Scheme::Dark);
    const CHARCOAL: BlendTheme = BlendTheme::Sheet(DesktopStyle::Omarchy, false);

    /// Two dark themes far enough apart that a midpoint is obvious, and one
    /// light theme to be refused.
    fn bench() -> BlendCache {
        let mut cache = BlendCache::new();
        cache.insert(made_up(
            NEAR_BLACK,
            &[
                ("color_bg_app", 0x101010FF),
                ("color_fg_app", 0x202020FF),
                ("color_text", 0xEEEEEEFF),
                ("color_on_surface", 0xEEEEEEFF),
                ("color_on_surface_variant", 0xAAAAAAFF),
                ("color_u_3", 0x404040FF),
                ("color_u_15", 0x303030FF),
                ("color_opaque_u_6", 0xF0F0F0FF),
                ("color_opaque_d_5", 0x0A0A0AFF),
                ("color_opaque_u_1", 0x181818FF),
                ("color_opaque_u_2", 0x282828FF),
                ("color_primary", 0xFF5C39FF),
                ("color_map_1", 0x286CABFF),
                ("color_syntax_string", 0x00FF00FF),
            ],
            &[("space_factor", 8.0), ("radius_m", 6.0)],
        ));
        cache.insert(made_up(
            CHARCOAL,
            &[
                ("color_bg_app", 0x303030FF),
                ("color_fg_app", 0x404040FF),
                ("color_text", 0xCCCCCCFF),
                ("color_on_surface", 0xCCCCCCFF),
                ("color_on_surface_variant", 0xB0B0B0FF),
                ("color_u_3", 0x606060FF),
                ("color_u_15", 0x505050FF),
                ("color_opaque_u_6", 0xE0E0E0FF),
                ("color_opaque_d_5", 0x1A1A1AFF),
                ("color_opaque_u_1", 0x383838FF),
                ("color_opaque_u_2", 0x484848FF),
                ("color_primary", 0x39A5FFFF),
                ("color_map_1", 0x900000FF),
                ("color_syntax_string", 0x0000FFFF),
            ],
            &[("space_factor", 10.0), ("radius_m", 4.0)],
        ));
        cache.insert(made_up(
            BlendTheme::Base(Scheme::Light),
            &[("color_bg_app", 0xFFFFFFFF), ("color_fg_app", 0xF0F0F0FF)],
            &[("space_factor", 9.0)],
        ));
        cache
    }

    /// A slider at the top with every other at nought is not a blend of
    /// anything, and has to come back as the theme itself -- not the theme
    /// plus a rounding error, and not the theme with its inks second-guessed.
    #[test]
    fn a_mix_at_full_weight_is_exactly_that_theme() {
        let cache = bench();
        let theme = cache.get(NEAR_BLACK).unwrap();
        for weight in [1.0, 100.0, 0.001] {
            let blend = cache.blend(&[(NEAR_BLACK, weight), (CHARCOAL, 0.0)]).unwrap();
            assert_eq!(blend.argmax, NEAR_BLACK);
            for key in theme.keys() {
                let want = theme.get(key).unwrap();
                let got = match (blend.color(key), blend.num(key)) {
                    (Some(c), _) => BlendValue::Color(c),
                    (_, Some(n)) => BlendValue::Num(n),
                    _ => panic!("{key} is missing from the blend"),
                };
                assert_eq!(got, want, "{key} at weight {weight}");
            }
        }
    }

    /// Half of one and half of the other is the midpoint of every channel and
    /// of every number, with nothing else moved.
    #[test]
    fn half_and_half_is_the_midpoint_of_every_channel() {
        let cache = bench();
        let blend = cache.blend(&[(NEAR_BLACK, 50.0), (CHARCOAL, 50.0)]).unwrap();
        assert_eq!(blend.color("color_bg_app"), Some(0x202020FF));
        assert_eq!(blend.color("color_fg_app"), Some(0x303030FF));
        // (0xEE + 0xCC) / 2 and (0xFF5C39 + 0x39A5FF) / 2, per channel.
        assert_eq!(blend.color("color_text"), Some(0xDDDDDDFF));
        assert_eq!(blend.color("color_primary"), Some(0x9C819CFF));
        assert_eq!(blend.num("space_factor"), Some(9.0));
        assert_eq!(blend.num("radius_m"), Some(5.0));
        // And it is the same answer as the library's own two-colour mix.
        assert_eq!(blend.color("color_bg_app"), Some(mix_rgb(0x101010FF, 0x303030FF, 0.5)));
        // A weighting works out the same way: three parts to one.
        let quarter = cache.blend(&[(NEAR_BLACK, 75.0), (CHARCOAL, 25.0)]).unwrap();
        assert_eq!(quarter.color("color_bg_app"), Some(mix_rgb(0x101010FF, 0x303030FF, 0.25)));
    }

    /// An ink that is not opaque is most of the theme's text, and a mix that
    /// forgets its alpha comes back as a solid version of itself.
    #[test]
    fn a_mix_keeps_the_alpha_a_token_carries() {
        assert_eq!(mix_rgba(0xFFFFFF00, 0xFFFFFFFF, 0.5), 0xFFFFFF80);
        assert_eq!(mix_rgba(0x000000AA, 0x000000AA, 0.5), 0x000000AA);
        assert_eq!(mix_rgba(0x101010FF, 0x303030FF, 0.5), 0x202020FF);
    }

    /// The heaviest theme lends the mix everything an average means nothing
    /// for: the chart and syntax palettes, and -- through the object the
    /// script derives from -- the fonts, the eases and `mspace_1`.
    #[test]
    fn the_heaviest_theme_lends_its_palettes_and_its_fonts() {
        let cache = bench();
        let mine = cache.blend(&[(NEAR_BLACK, 70.0), (CHARCOAL, 30.0)]).unwrap();
        assert_eq!(mine.argmax, NEAR_BLACK);
        assert_eq!(mine.color("color_map_1"), Some(0x286CABFF));
        assert_eq!(mine.color("color_syntax_string"), Some(0x00FF00FF));
        assert_eq!(mine.base, Scheme::Dark);
        assert_eq!(mine.argmax_sheet(), None);
        // The palettes never meet in the middle, however close the weights.
        let theirs = cache.blend(&[(NEAR_BLACK, 49.0), (CHARCOAL, 51.0)]).unwrap();
        assert_eq!(theirs.argmax, CHARCOAL);
        assert_eq!(theirs.color("color_map_1"), Some(0x900000FF));
        assert_eq!(theirs.color("color_syntax_string"), Some(0x0000FFFF));
        assert_eq!(theirs.argmax_sheet(), Some((DesktopStyle::Omarchy, false)));
        // The rest of the mix is still a mix, and barely moved by the swap.
        assert_eq!(mine.color("color_bg_app"), Some(mix_rgb(0x101010FF, 0x303030FF, 0.3)));
        assert_eq!(theirs.color("color_bg_app"), Some(mix_rgb(0x101010FF, 0x303030FF, 0.51)));
        // A tie keeps the first, so an even mix does not change its mind
        // about whose fonts it wears when the list is rebuilt.
        let even = cache.blend(&[(NEAR_BLACK, 50.0), (CHARCOAL, 50.0)]).unwrap();
        assert_eq!(even.argmax, NEAR_BLACK);
    }

    /// One script, deriving a new theme object. Never a run of assignments
    /// into `mod.theme`, which is shared with every widget already built.
    #[test]
    fn a_mix_is_one_script_that_derives_a_theme() {
        let cache = bench();
        let blend = cache.blend(&[(NEAR_BLACK, 50.0), (CHARCOAL, 50.0)]).unwrap();
        let script = blend.script("equalized");
        assert!(script.starts_with("mod.themes.equalized = mod.themes.dark{ "), "{script}");
        assert!(script.ends_with(" }\nmod.theme = mod.themes.equalized\n"), "{script}");
        assert!(!script.contains("mod.theme."), "a mix must not assign into the shared theme: {script}");
        assert!(script.contains("color_bg_app: #x202020FF"), "{script}");
        assert!(script.contains("space_factor: 9.0"), "{script}");
        assert_eq!(script.lines().count(), 2, "{script}");
    }

    /// Nothing to divide by, and a theme nobody resolved, are both said out
    /// loud rather than guessed at.
    #[test]
    fn a_mix_of_nothing_is_an_error_not_a_guess() {
        let cache = bench();
        assert_eq!(cache.blend(&[]), Err(BlendError::NoWeight));
        assert_eq!(cache.blend(&[(NEAR_BLACK, 0.0)]), Err(BlendError::NoWeight));
        let absent = BlendTheme::Sheet(DesktopStyle::NextStep, false);
        assert_eq!(cache.blend(&[(absent, 1.0)]), Err(BlendError::NotResolved(absent)));
    }

    /// A dark theme and a light one meet in a mid grey with mid grey text on
    /// it. The engine refuses rather than producing that.
    #[test]
    fn a_mix_never_crosses_the_appearance() {
        let cache = bench();
        let light = BlendTheme::Base(Scheme::Light);
        assert_eq!(
            cache.blend(&[(NEAR_BLACK, 50.0), (light, 50.0)]),
            Err(BlendError::MixedAppearance(NEAR_BLACK, light))
        );
        // A weight of nought is not in the mix at all, so it cannot spoil one.
        assert!(cache.blend(&[(NEAR_BLACK, 50.0), (light, 0.0)]).is_ok());
        // And the two groups are the two lists a panel offers.
        let dark = BlendTheme::group(Appearance::Dark);
        let pale = BlendTheme::group(Appearance::Light);
        assert_eq!(dark.len() + pale.len(), BlendTheme::all().len());
        assert!(dark.iter().all(|t| !pale.contains(t)));
        assert_eq!(dark.len(), 7, "{dark:?}");
        assert_eq!(pale.len(), 8, "{pale:?}");
    }

    /// The weights of a relative mix are a hundred parts shared out, so
    /// however they are dragged about they still add up to a hundred and none
    /// of them goes negative.
    #[test]
    fn a_relative_mix_always_adds_up_to_a_hundred() {
        let mut weights = to_relative(&[1.0, 0.0, 0.0, 0.0], 0);
        assert_eq!(weights, vec![100.0, 0.0, 0.0, 0.0]);
        // Raising the second takes from the theme the mix started from.
        relative_set(&mut weights, 0, 1, 30.0);
        assert_eq!(weights, vec![70.0, 30.0, 0.0, 0.0]);
        // Raising the third takes the anchor's last seventy and then ten more
        // from the second, which is all there is left to scale down.
        relative_set(&mut weights, 0, 2, 80.0);
        assert_eq!(weights, vec![0.0, 20.0, 80.0, 0.0]);
        // Lowering hands it back to the anchor.
        relative_set(&mut weights, 0, 2, 40.0);
        assert_eq!(weights, vec![40.0, 20.0, 40.0, 0.0]);
        // A slider cannot go past the whole budget.
        relative_set(&mut weights, 0, 3, 150.0);
        assert_eq!(weights, vec![0.0, 0.0, 0.0, 100.0]);
        // And it holds through any sequence of drags, including on the anchor
        // itself and in absolute mode, which conserves nothing on purpose.
        let mut weights = to_relative(&[3.0, 1.0, 1.0, 5.0, 0.0], 3);
        let mut state = 99u64;
        for step in 0..500 {
            let index = (next_u64(&mut state) % 5) as usize;
            let value = next_unit(&mut state) * RELATIVE_TOTAL;
            set_weight(WeightMode::Relative, &mut weights, 3, index, value);
            let total: f64 = weights.iter().sum();
            assert!((total - RELATIVE_TOTAL).abs() < 1e-9, "step {step}: {weights:?} adds up to {total}");
            assert!(weights.iter().all(|w| *w >= -1e-9), "step {step}: {weights:?}");
        }
        let mut absolute = vec![1.0, 1.0];
        set_weight(WeightMode::Absolute, &mut absolute, 0, 1, 7.0);
        assert_eq!(absolute, vec![1.0, 7.0]);
        assert_eq!(normalized(&absolute), vec![0.125, 0.875]);
        assert_eq!(normalized(&[0.0, 0.0]), vec![0.0, 0.0]);
    }

    /// The randomiser randomises the CURVE, not just the numbers: a sharp
    /// sigma is one theme with an influence behind it, a wide one is a spread.
    /// Same seed, same mix, so a mix somebody liked can be got back.
    #[test]
    fn random_weights_is_seeded_sums_to_one_and_reaches_both_ends() {
        assert_eq!(random_weights(7, 6), random_weights(7, 6));
        assert_ne!(random_weights(7, 6), random_weights(8, 6));
        assert!(random_weights(1, 0).is_empty());
        assert_eq!(random_weights(1, 1), vec![1.0]);
        for seed in 0..200u64 {
            for n in 1..10usize {
                let weights = random_weights(seed, n);
                assert_eq!(weights.len(), n);
                let total: f64 = weights.iter().sum();
                assert!((total - 1.0).abs() < 1e-12, "seed {seed} of {n}: {total}");
                assert!(weights.iter().all(|w| *w > 0.0 && w.is_finite()), "seed {seed}: {weights:?}");
            }
        }
        // Both ends of the sharpness range are actually reached.
        let mut sharpest: f64 = 0.0;
        let mut widest: f64 = 1.0;
        let mut firsts = std::collections::BTreeSet::new();
        for seed in 0..500u64 {
            let weights = random_weights(seed, 7);
            let top = weights.iter().copied().fold(0.0f64, f64::max);
            sharpest = sharpest.max(top);
            widest = widest.min(top);
            firsts.insert(weights.iter().position(|w| *w == top).unwrap());
        }
        assert!(sharpest > 0.85, "no seed gave one dominant theme: the best was {sharpest}");
        assert!(widest < 0.30, "no seed gave a spread: the flattest was {widest}");
        // The order is random too, so it is not always the first theme that
        // wins.
        assert_eq!(firsts.len(), 7, "the shuffle never reached {:?}", firsts);
    }

    /// The table of which base theme each sheet is written against is a second
    /// copy of the sheets' own first line, and a second copy is only safe
    /// while something fails when the first one moves.
    #[test]
    fn the_sheet_table_is_what_the_sheets_say() {
        let mut expected: Vec<(DesktopStyle, bool)> = Vec::new();
        for style in DesktopStyle::ALL {
            expected.push((style, false));
            if style.supports_dark() {
                expected.push((style, true));
            }
        }
        let listed: Vec<(DesktopStyle, bool)> = SHEET_BASE.iter().map(|(s, d, _)| (*s, *d)).collect();
        assert_eq!(listed.len(), expected.len(), "{listed:?}");
        for entry in &expected {
            assert!(listed.contains(entry), "{entry:?} is not in the table");
        }
        for (style, dark, base) in SHEET_BASE {
            let theme = BlendTheme::Sheet(*style, *dark);
            let sheet = StyleSheet::load_with_appearance(*style, *dark);
            assert_eq!(sheet.name, theme.name(), "the name a sheet loads under");
            let line = format!("mod.theme = mod.themes.{}", base.theme_name());
            assert!(
                sheet.theme.lines().any(|l| l.trim() == line),
                "{}: expected the line `{line}`",
                sheet.name
            );
            assert_eq!(theme.base(), *base);
            let wanted = if *base == Scheme::Dark { Appearance::Dark } else { Appearance::Light };
            assert_eq!(theme.appearance(), wanted, "{}", sheet.name);
        }
    }

    /// The map and syntax palettes, and nothing else. Half the colours in a
    /// theme file are `color_map_*`, which is why the exclusion is worth
    /// stating: the other four hundred tokens do blend.
    #[test]
    fn only_the_categorical_palettes_are_left_out() {
        assert!(is_categorical("color_map_1"));
        assert!(is_categorical("color_map_12_h"));
        assert!(is_categorical("color_syntax_string"));
        assert!(!is_categorical("color_mark_active"));
        assert!(!is_categorical("color_primary"));
        assert!(!is_categorical("color_surface"));
        let keys = base_theme_keys();
        let out = keys.iter().filter(|k| is_categorical(k)).count();
        assert_eq!(keys.len(), 557, "the theme files have grown or shrunk");
        assert_eq!(out, 133, "the categorical palettes are {out} of {} tokens", keys.len());
    }

    /// The keys a sheet introduces that no base theme file has, which a mix
    /// that read only the files would drop.
    #[test]
    fn a_sheets_own_keys_are_read_as_well() {
        let sheet = "mod.theme = mod.themes.light\nmod.theme.color_label = #f00\n  mod.theme.color_terminal_bg=#0f0\nmod.theme.color_x.y = #00f\nlet color_no = 1\n";
        assert_eq!(assigned_keys(sheet), vec!["color_label", "color_terminal_bg"]);
        let real = StyleSheet::load_with_appearance(DesktopStyle::Windows2000, false);
        let keys = assigned_keys(&real.theme);
        assert!(keys.contains(&"color_bg_app"), "{keys:?}");
        assert!(keys.iter().any(|k| !base_theme_keys().contains(k)), "windows-2000 sets a key of its own");
    }

    /// Every theme the equalizer offers, resolved once. The reload per theme
    /// is the expensive part, so the two tests that need real values share
    /// one cache.
    fn resolved() -> &'static BlendCache {
        static CACHE: OnceLock<BlendCache> = OnceLock::new();
        CACHE.get_or_init(|| {
            let mut cache = BlendCache::new();
            let mut cx = Cx::new(Box::new(|_, _| {}));
            cx.with_vm(|vm| cache.fill(vm, &BlendTheme::all()));
            cache
        })
    }

    /// The same promise as `a_mix_at_full_weight_is_exactly_that_theme`, but
    /// against the values a real theme actually resolves to, where the role
    /// derivation gets a chance to second-guess a theme's own inks.
    #[test]
    fn a_lone_resolved_theme_blends_into_itself() {
        let cache = resolved();
        assert_eq!(cache.len(), BlendTheme::all().len());
        for theme in BlendTheme::all() {
            let values = cache.get(theme).unwrap();
            // The skeleton is a shorter file than the other two and carries
            // 350 of the 557 tokens; the rest carry over 500.
            let least = if theme == BlendTheme::Base(Scheme::Skeleton) { 340 } else { 500 };
            assert!(values.len() > least, "{} resolved only {} tokens", theme.name(), values.len());
            let blend = cache.blend(&[(theme, 100.0)]).unwrap();
            let mut moved: Vec<String> = Vec::new();
            for key in values.keys() {
                let want = values.get(key).unwrap();
                let got = match (blend.color(key), blend.num(key)) {
                    (Some(c), _) => BlendValue::Color(c),
                    (_, Some(n)) => BlendValue::Num(n),
                    _ => {
                        moved.push(format!("{key} is missing"));
                        continue;
                    }
                };
                if got != want {
                    moved.push(format!("{key}: {want:?} became {got:?}"));
                }
            }
            assert!(moved.is_empty(), "{} did not survive its own blend:\n{}", theme.name(), moved.join("\n"));
        }
    }

    /// Whether the skeleton really belongs with the light group, and the dark
    /// group with the dark: measured off the resolved page rather than taken
    /// on trust from a name.
    #[test]
    fn the_groups_match_the_grounds() {
        let cache = resolved();
        for theme in BlendTheme::all() {
            let ground = cache.get(theme).unwrap().color("color_surface").unwrap();
            let dark = contrast(ground | 0xFF, 0xFFFFFFFF) > contrast(ground | 0xFF, 0x000000FF);
            let wanted = if dark { Appearance::Dark } else { Appearance::Light };
            assert_eq!(theme.appearance(), wanted, "{} has a {ground:08X} page", theme.name());
        }
    }

    /// The case the whole re-derivation is there for. The light theme draws
    /// BLACK on its green and macOS draws WHITE on its own, so half of one
    /// and half of the other meets at a mid grey, and a mid grey stands at
    /// 1.04:1 on the green underneath it: two themes whose success message
    /// reads perfectly well, blending into one whose does not. Thirty-six of
    /// the mixes tried in `every_blended_pair_still_reaches_its_bar` have that
    /// shape. Nothing is clamped to hide it -- the ink is simply asked again
    /// of the ground it will actually sit on, and comes back black at 5.10:1.
    /// The surface ladder, by contrast, blends safely on its own: not one of
    /// the 5184 pairs measured there needed its ink changed.
    #[test]
    fn two_readable_themes_can_blend_into_an_unreadable_one() {
        let cache = resolved();
        let light = BlendTheme::Base(Scheme::Light);
        let macos = BlendTheme::Sheet(DesktopStyle::Macos, false);
        for theme in [light, macos] {
            let values = cache.get(theme).unwrap();
            let (ground, ink) = (values.color("color_success").unwrap(), values.color("color_on_success").unwrap());
            assert!(reads_on(ground | 0xFF, ink) >= READABLE, "{} starts out readable", theme.name());
        }
        let blend = cache.blend(&[(light, 50.0), (macos, 50.0)]).unwrap();
        let ground = blend.color("color_success").unwrap() | 0xFF;
        let naive = mix_rgba(
            cache.get(light).unwrap().color("color_on_success").unwrap(),
            cache.get(macos).unwrap().color("color_on_success").unwrap(),
            0.5,
        );
        assert_eq!(naive, 0x808080FF, "black and white average to a mid grey");
        let flat = reads_on(ground, naive);
        assert!(flat < 1.1, "the averaged ink stands at {flat:.2} on the blended green");
        let chosen = blend.color("color_on_success").unwrap();
        assert_eq!(chosen, 0x000000FF);
        assert!(reads_on(ground, chosen) >= READABLE, "and what the blend ships does read");
    }

    /// Two readable themes CAN blend into an unreadable one: the ink each of
    /// them chose was chosen against its own ground, and both the ink and the
    /// ground move when they are averaged, with no promise that they move
    /// apart. So every mix is put to the same bars the library holds itself
    /// to -- `READABLE` for body text, `LEGIBLE` for the second voice -- over
    /// every pair in a group at several weightings, and over a few dozen
    /// random mixes of the whole group.
    #[test]
    fn every_blended_pair_still_reaches_its_bar() {
        let cache = resolved();
        let mut bad: Vec<String> = Vec::new();
        let mut checked = 0usize;
        let mut measured = 0usize;
        let mut tightest = (f64::MAX, String::new());
        let mut check = |blend: &ThemeBlend, label: &str, bad: &mut Vec<String>| {
            for (pairs, need) in [(SURFACES, READABLE), (VARIANTS, LEGIBLE), (MEANING, READABLE)] {
                for (ground, ink) in pairs {
                    if let (Some(g), Some(i)) = (blend.color(ground), blend.color(ink)) {
                        let c = reads_on(g | 0xFF, i);
                        measured += 1;
                        if c - need < tightest.0 {
                            tightest = (c - need, format!("{label}: {ink} on {ground} = {c:.2}, needs {need}"));
                        }
                        if c < need {
                            bad.push(format!("{label}: {ink} on {ground} = {c:.2}, wanted {need}"));
                        }
                    }
                }
            }
        };
        for appearance in Appearance::ALL {
            let group = BlendTheme::group(appearance);
            for (a, first) in group.iter().enumerate() {
                for second in &group[a + 1..] {
                    for (wa, wb) in [(50.0, 50.0), (25.0, 75.0), (75.0, 25.0), (90.0, 10.0)] {
                        let mix = [(*first, wa), (*second, wb)];
                        let blend = cache.blend(&mix).unwrap();
                        let label = format!("{} {wa} / {} {wb}", first.name(), second.name());
                        check(&blend, &label, &mut bad);
                        checked += 1;
                    }
                }
            }
            for seed in 0..64u64 {
                let weights = random_weights(seed, group.len());
                let mix: Vec<(BlendTheme, f64)> = group.iter().copied().zip(weights.iter().copied()).collect();
                let blend = cache.blend(&mix).unwrap();
                let label = format!(
                    "{} seed {seed}: {}",
                    appearance.label(),
                    mix.iter().map(|(t, w)| format!("{} {:.2}", t.name(), w)).collect::<Vec<_>>().join(" ")
                );
                check(&blend, &label, &mut bad);
                checked += 1;
            }
        }
        assert!(checked > 200, "only {checked} mixes were tried");
        // A pair that is never found is a pair that is never checked, and a
        // test that checks nothing passes beautifully.
        assert!(measured > 3000, "only {measured} pairs were actually measured");
        assert!(bad.is_empty(), "{} of {checked} mixes carry ink that does not hold (the tightest pair overall was {}):\n{}", bad.len(), tightest.1, bad.join("\n"));
    }
}
