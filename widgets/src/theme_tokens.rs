//! The theme token registry: what every design token is called, what kind of
//! value it holds, which of the three themes carry it, the range a control
//! may drive it over, and the rule that generates the accent palette.
//!
//! Why this exists. The three theme files (`theme_desktop_dark.rs`,
//! `theme_desktop_light.rs`, `theme_desktop_skeleton.rs`) are script objects
//! that only the runtime reads. The F12 tweaker's theme tab and the catalogue
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
    ease("motion_ease_spring", "An exponential settle; drags and snaps."),
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
    [
        FamilyInput { hue: primary, sat: 0.62, intent: false },
        FamilyInput { hue: seed.secondary.map(hue_of).unwrap_or(primary + 30.0), sat: 0.62 * 0.55, intent: false },
        FamilyInput { hue: seed.tertiary.map(hue_of).unwrap_or(primary - 150.0), sat: 0.62 * 0.8, intent: false },
        FamilyInput { hue: seed.error.map(hue_of).unwrap_or(0.0), sat: 0.72, intent: true },
        FamilyInput { hue: 42.0, sat: 0.85, intent: true },
        FamilyInput { hue: 140.0, sat: 0.55, intent: true },
        FamilyInput { hue: 212.0, sat: 0.70, intent: true },
    ]
}

fn light_family(hue: f64, sat: f64) -> RoleFamily {
    RoleFamily {
        base: hsl_to_rgb(hue, sat, 0.40),
        on_base: WHITE,
        container: hsl_to_rgb(hue, (sat * 1.1).min(1.0), 0.90),
        on_container: hsl_to_rgb(hue, sat, 0.12),
    }
}

fn dark_family(hue: f64, sat: f64) -> RoleFamily {
    RoleFamily {
        base: hsl_to_rgb(hue, sat, 0.74),
        on_base: hsl_to_rgb(hue, sat, 0.18),
        container: hsl_to_rgb(hue, sat * 0.7, 0.30),
        on_container: hsl_to_rgb(hue, (sat * 1.1).min(1.0), 0.90),
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
    ColorRoles {
        primary: f(0),
        secondary: f(1),
        tertiary: f(2),
        error: f(3),
        warning: f(4),
        success: f(5),
        info: f(6),
        inverse_primary,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The keys a theme file defines at its top level: lines at exactly eight
    /// spaces of indent that open with `name:`.
    fn own_keys(source: &str) -> Vec<&str> {
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
