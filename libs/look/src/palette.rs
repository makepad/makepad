//! Every colour Stage paints with, in one table.
//!
//! Stage has a look of its own -- slate panels, pale ink, Makepad orange for
//! everything that is ON -- and it used to write that look out as some six
//! hundred colour literals, so a style sheet worn over the app changed the
//! library's widgets and nothing of Stage's. This module is the one place
//! those colours are now decided. The markup says `stage.panel`,
//! `stage.accent`; Rust asks [`get`] for the same table.
//!
//! # Two ways the table is filled
//!
//! While the library is in the state it starts in -- the dark base theme and
//! no style sheet -- the table is [`Palette::HOUSE`]: the colours Stage has
//! always had, to the digit, with the accent read from `theme.color_makepad`
//! so the brand colour has one owner.
//!
//! Under anything else (a style sheet from the Tweaker or the window manager,
//! the light or the skeleton base theme) every role is worked out again from
//! the theme in force, by what KIND of colour it is ([`Kind`]): a surface is
//! the same step of lightness off the theme's page that it was off Stage's
//! own, an ink reads as strongly against the new page as it did against the
//! old one, a hairline is the new ink at the old alpha, the accent family is
//! the theme's `color_primary`, a colour that MEANS something (live green,
//! found yellow, the red of a fault) keeps its hue and is pulled to where it
//! reads on the new page. The roles are the library's newer ones
//! (`color_surface`, `color_on_surface`, `color_primary`, ...): every sheet has
//! them, because `theme_tokens::sheet_roles_script` derives them from what
//! the sheet did set.
//!
//! Video wells, the EQ graph and the score keep their dark glass. Audio
//! waveforms have separate roles: a softly grey ground with dark traces in
//! light themes, and the original instrument colours in dark themes.
//!
//! # When it is rebuilt
//!
//! [`script_mod`] runs from `App::script_mod`, straight after the widget
//! library's own, so it runs again on every style reload -- which is what the
//! Tweaker's theme picker asks for. Each run bumps [`generation`]; the app
//! watches that number to put its Rust-side paints on again, since a style
//! reload re-applies the markup over them. A re-apply that does not evaluate
//! anything ([`reapplied`]) moves the same number for the same reason.

use makepad_widgets::makepad_platform::NoTrap;
use makepad_widgets::theme_tokens::{contrast, hsl_to_rgb, mix_rgb, over, readable_on, rgb_to_hsl};
use makepad_widgets::*;

const WHITE: u32 = 0xffff_ffff;
const BLACK: u32 = 0x0000_00ff;

/// What a role is, which decides how it follows a theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// The page itself: the theme's `color_surface`.
    Page,
    /// A neutral fill: the same step of lightness off the page, toward the
    /// ink where Stage's own is lighter than its page and away from it where
    /// it is darker.
    Surface,
    /// A fill with a hue of its own (the picked row, the live row, the
    /// danger button): the same step of lightness, keeping the hue.
    Wash,
    /// A library token laid over the page, for the things the library has a
    /// word for: button faces, field wells, the ink on a button.
    Token(&'static str),
    /// Neutral ink: reads against the page as strongly as Stage's own does
    /// against its own page.
    Ink,
    /// A translucent stroke of the ink colour: hairlines, borders, veils.
    Line,
    /// The accent, at the alpha the house value carries.
    Accent,
    /// The accent pulled toward white by this much: hover and soft states.
    AccentLift(f64),
    /// The accent pulled toward black by this much: the pressed state.
    AccentSink(f64),
    /// What is drawn on the accent.
    OnAccent,
    /// A surface with this much of the accent mixed in: a selected row.
    AccentWash(f64),
    /// The accent pulled most of the way to the strong ink: a pale ring.
    AccentTint,
    /// A hue that means something, drawn as ink or a mark on the page: kept
    /// where it reads there, and walked along its own lightness where not.
    Signal,
    /// The accent as it is drawn on glass, at the house alpha: lifted toward
    /// white until it shows on a dark ground, which a light theme's accent
    /// (chosen to read on a pale page) otherwise does not.
    GlassAccent,
    /// A pale ring of that accent, on glass.
    GlassAccentTint,
    /// Audio display surfaces preserve the dark instrument look and become
    /// a soft grey under a light theme.
    WaveSurface,
    WaveInk,
    WaveLine,
    WaveSignal,
    WaveAccent,
    /// The same under every theme: scrims and shadows, fills that carry
    /// their own ink, the operating system's close-button red, and the
    /// instrument glass with what is drawn on it.
    Fixed,
}

macro_rules! roles {
    ($($(#[$doc:meta])* $name:ident: $house:expr => $kind:expr,)*) => {
        /// One colour per role, `0xRRGGBBAA`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub struct Palette {
            $($(#[$doc])* pub $name: u32,)*
        }
        impl Palette {
            /// Stage's own look: what the app has always been drawn in.
            pub const HOUSE: Palette = Palette { $($name: $house,)* };
            const KINDS: &'static [(&'static str, Kind)] = &[$((stringify!($name), $kind),)*];
            /// `(role, colour)` in table order.
            pub fn entries(&self) -> Vec<(&'static str, u32)> {
                vec![$((stringify!($name), self.$name),)*]
            }
            fn from_fn(mut f: impl FnMut(&'static str, u32, Kind) -> u32) -> Palette {
                let mut kinds = Self::KINDS.iter();
                Palette {
                    $($name: {
                        let (name, kind) = *kinds.next().expect("one kind per role");
                        f(name, $house, kind)
                    },)*
                }
            }
        }
    };
}

roles! {
    // ---- surfaces, darkest to lightest in the house look ----
    /// A field's well: text inputs, value boxes, the wells of a meter.
    well: 0x10141aff => Kind::Token("color_inset"),
    /// Every other row of a list.
    row_alt: 0x11161cff => Kind::Surface,
    /// The page.
    bg: 0x14171cff => Kind::Page,
    /// A menu, a popover, a card laid over the page.
    menu: 0x16161bff => Kind::Surface,
    /// The same, let through a little.
    menu_glass: 0x16161bf5 => Kind::Surface,
    /// A panel: the rounded containers a page is made of.
    panel: 0x181c23ff => Kind::Surface,
    /// The panels of the synth and mix pages, a shade bluer in Stage's own
    /// look.
    panel_score: 0x181e25ff => Kind::Surface,
    /// A list row, a grid cell.
    row: 0x1c2129ff => Kind::Surface,
    /// The body a slider or a meter runs in.
    groove: 0x1d222aff => Kind::Surface,
    /// A tile or pad cell waiting for its picture. Glass, like the picture
    /// that lands in it: a caption drawn over a thumbnail has to read on the
    /// empty cell too, and cannot be pale for one and dark for the other.
    cell: 0x1e232bff => Kind::Fixed,
    /// A raised strip: a chip at rest, a header that is active, an unlit step.
    raised: 0x222831ff => Kind::Surface,
    /// A button face, and the face under the pointer and under the press.
    control: 0x272e38ff => Kind::Token("color_outset"),
    control_hover: 0x2b3440ff => Kind::Token("color_outset_hover"),
    control_down: 0x1e232bff => Kind::Token("color_outset_down"),
    /// A button face over moving content, let through a little.
    control_glass: 0x2b3440f0 => Kind::Surface,
    /// The unfilled run of a slider, the beat column of a step grid.
    track: 0x2b343fff => Kind::Surface,
    /// The lightest neutral fill: a chip under the pointer, a knob's ring.
    control_top: 0x2f3842ff => Kind::Surface,
    /// Opaque edges, for where a translucent hairline would show what is
    /// under it.
    edge_lo: 0x38424dff => Kind::Surface,
    edge: 0x46505bff => Kind::Surface,

    // ---- ink, strongest to faintest ----
    text_max: 0xffffffff => Kind::Ink,
    /// Text under the pointer.
    text_hi: 0xfffaf4ff => Kind::Ink,
    text_bright: 0xf4f7faff => Kind::Ink,
    /// Body text and values.
    text: 0xe8eef4ff => Kind::Ink,
    /// The words on a control.
    label: 0xd6dee6ff => Kind::Token("color_label_inner"),
    text_soft_hi: 0xc7d0daff => Kind::Ink,
    text_soft: 0xb4bfcaff => Kind::Ink,
    /// Panel headings and captions.
    secondary: 0xa6b1bdff => Kind::Ink,
    secondary_lo: 0x9fabb7ff => Kind::Ink,
    text_cool: 0x94a8b8ff => Kind::Ink,
    /// Units, hints, column heads.
    muted: 0x8e9aa7ff => Kind::Ink,
    dim_hi: 0x7a8592ff => Kind::Ink,
    dim: 0x6f7b87ff => Kind::Ink,
    dim_lo: 0x657383ff => Kind::Ink,
    /// The wordmark beside the logo: warm in Stage's own look, plain ink
    /// under a theme.
    brand_word: 0xfae1bcff => Kind::Ink,
    /// Barely there: an idle icon.
    faint: 0x5f6a76ff => Kind::Ink,
    /// A control that is present and off: the transport of an empty deck.
    ghost: 0x39404a66 => Kind::Line,
    ghost_hi: 0x39404a88 => Kind::Line,
    ghost_mid: 0x39404a55 => Kind::Line,
    ghost_lo: 0x39404a33 => Kind::Line,
    ghost_faint: 0x39404a22 => Kind::Line,
    /// The face of such a control.
    ghost_face: 0x1a1e2455 => Kind::Surface,

    // ---- hairlines and veils: the ink, let through ----
    line_faint: 0xffffff0d => Kind::Line,
    separator: 0xffffff12 => Kind::Line,
    line_soft: 0xffffff18 => Kind::Line,
    line: 0xffffff1f => Kind::Line,
    border: 0xffffff26 => Kind::Line,
    border_hi: 0xffffff2e => Kind::Line,
    border_mid: 0xffffff33 => Kind::Line,
    border_strong: 0xffffff40 => Kind::Line,
    veil: 0xffffff5c => Kind::Line,
    veil_hi: 0xffffff70 => Kind::Line,
    mark: 0xffffffcc => Kind::Line,

    // ---- the accent: everything that is ON ----
    accent: 0xff5c39ff => Kind::Accent,
    accent_hover: 0xff7a5cff => Kind::AccentLift(0.17),
    accent_soft: 0xff8f70ff => Kind::AccentLift(0.28),
    accent_down: 0xd94a2cff => Kind::AccentSink(0.15),
    accent_a80: 0xff5c3980 => Kind::Accent,
    accent_a66: 0xff5c3966 => Kind::Accent,
    accent_a38: 0xff5c3938 => Kind::Accent,
    on_accent: 0x1c0b06ff => Kind::OnAccent,
    /// A row or a chip the accent has been mixed into.
    accent_wash: 0x46312bff => Kind::AccentWash(0.22),
    /// A pale ring round something lit.
    accent_tint: 0xffd9ccff => Kind::AccentTint,
    /// The beat lamp on the downbeat.
    led_down: 0xfff1c8ff => Kind::AccentTint,
    /// A scroll thumb under the pointer.
    thumb_hover: 0xd6fff0ff => Kind::AccentTint,

    // ---- colours that mean something ----
    /// Live, processed, monitoring.
    live: 0x35c05fff => Kind::Signal,
    live_a80: 0x35c05f80 => Kind::Signal,
    live_a55: 0x35c05f55 => Kind::Signal,
    live_a33: 0x35c05f33 => Kind::Signal,
    /// The monitoring latch: a fill that carries its own ink.
    live_fill: 0x35c05fff => Kind::Fixed,
    live_fill_hover: 0x4fd47aff => Kind::Fixed,
    live_fill_down: 0x2a9e4cff => Kind::Fixed,
    on_live: 0x07180dff => Kind::Fixed,
    /// The row that is playing.
    row_live: 0x1d2a2aff => Kind::Wash,
    teal: 0x58c4a0ff => Kind::Signal,
    /// A control armed for MIDI learn.
    learn: 0x3ee0b0ff => Kind::Signal,
    /// Found, cached, held.
    found: 0xf5c542ff => Kind::Signal,
    cached: 0xe0b341ff => Kind::Signal,
    amber: 0xf0b34dff => Kind::Signal,
    /// Status text that wants to be noticed without being an alarm.
    warm: 0xffe0a3ff => Kind::Signal,
    /// The overload lamp.
    lamp: 0xffe14dff => Kind::Fixed,
    lamp_off: 0x2a3038ff => Kind::Surface,
    /// The top of a meter.
    hot: 0xff5a4eff => Kind::Signal,
    hot_a99: 0xff5a4d99 => Kind::Signal,
    /// Faults, removal, off.
    danger: 0xe5484dff => Kind::Signal,
    danger_a55: 0xe5484d55 => Kind::Signal,
    fail: 0xff5c5cff => Kind::Signal,
    /// A generation job that failed, and one still waiting its turn.
    job_failed: 0xe0574fff => Kind::Signal,
    job_wait: 0xc2943dff => Kind::Signal,
    /// The panic button: a fill that carries its own ink.
    danger_fill: 0xd92c2cff => Kind::Fixed,
    danger_fill_hover: 0xe54848ff => Kind::Fixed,
    danger_fill_down: 0xb02020ff => Kind::Fixed,
    on_danger: 0x2b0808ff => Kind::Fixed,
    /// A destructive button at rest: a red cast on a surface.
    danger_wash: 0x3a1f1fff => Kind::Wash,
    danger_wash_hover: 0x4a2626ff => Kind::Wash,
    danger_wash_down: 0x2c1717ff => Kind::Wash,
    /// A chip that takes something away, under the pointer.
    remove_wash: 0x3a2b2fff => Kind::Wash,
    /// Saved, linked, informational.
    saved: 0x3d8bffff => Kind::Signal,
    link: 0x6aa8ffff => Kind::Signal,
    info: 0x5aa9ffff => Kind::Signal,
    info_soft: 0x73b9efff => Kind::Signal,
    info_fill: 0x4f9ee8ff => Kind::Signal,
    /// The keyboard cursor in a list.
    cursor: 0x8fb4ffff => Kind::Signal,
    /// The row the pointer picked.
    row_picked: 0x2c3a4eff => Kind::Wash,
    /// The musical key readout.
    key: 0xc6a0f0ff => Kind::Signal,
    key_hover: 0xe2ccffff => Kind::Signal,
    key_down: 0xa27fc9ff => Kind::Signal,
    /// A key that sits with the room, and one that argues with it.
    key_good: 0x9be8b0ff => Kind::Signal,
    key_clash: 0xd8776bff => Kind::Signal,

    // ---- the SCORE code view: its lenses and the code over them ----
    /// A lens: a tint over the code's well.
    lens: 0x151a21ff => Kind::Surface,
    /// A lens knob's body and its cap.
    lens_knob: 0x1f262fff => Kind::Surface,
    lens_knob_cap: 0x262e38ff => Kind::Surface,
    lens_ink: 0xd4d4d4ff => Kind::Ink,
    lens_muted: 0x7d8590ff => Kind::Ink,
    /// The value under the hand.
    lens_hand: 0xff6a3dff => Kind::Accent,
    /// The node categories, from the syntax colours: a loop, the clock, a
    /// shader, an effect, an instrument.
    lens_loop: 0xce9178ff => Kind::Signal,
    lens_clock: 0xdcdcaaff => Kind::Signal,
    lens_shader: 0xc586c0ff => Kind::Signal,
    lens_fx: 0x569cd6ff => Kind::Signal,
    lens_voice: 0x4ec9b0ff => Kind::Signal,
    /// The code's syntax colours: the code editor's own, read on the page.
    code_ident: 0xd4d4d4ff => Kind::Ink,
    code_unknown: 0xc0c0c0ff => Kind::Ink,
    code_space: 0x6e6e6eff => Kind::Ink,
    code_delimiter: 0xaaaaaaff => Kind::Ink,
    code_delimiter_hi: 0xffffffff => Kind::Ink,
    code_keyword: 0xc485beff => Kind::Signal,
    code_other_keyword: 0x5b9bd3ff => Kind::Signal,
    code_loop_keyword: 0xff8c00ff => Kind::Signal,
    code_string: 0xcc917bff => Kind::Signal,
    code_number: 0xb6ceaaff => Kind::Signal,
    code_function: 0xfffcc9ff => Kind::Signal,
    code_type: 0x56c9b1ff => Kind::Signal,
    code_comment: 0x638d54ff => Kind::Signal,

    // ---- the same under every theme ----
    /// The operating system's close button.
    os_close_hover: 0xe81123ff => Kind::Fixed,
    os_close_down: 0xf1707aff => Kind::Fixed,
    /// Video ground and letterbox.
    video: 0x000000ff => Kind::Fixed,
    scrim: 0x000000b8 => Kind::Fixed,
    shadow: 0x00000066 => Kind::Fixed,
    shadow_soft: 0x00000033 => Kind::Fixed,
    /// Instrument glass -- waveforms, the beat scope, the EQ graph, the
    /// phones strip, the score -- and what is drawn on it. A display is
    /// dark under every theme, so its inks are their own and do not follow
    /// the page's.
    glass: 0x0a0d12ff => Kind::Fixed,
    /// The beat scope in the top bar.
    glass_scope: 0x0b1016ff => Kind::Fixed,
    glass_deep: 0x05070aff => Kind::Fixed,
    glass_graph: 0x0e1319ff => Kind::Fixed,
    glass_paper: 0x20211fff => Kind::Fixed,
    glass_dead: 0x243039ff => Kind::Fixed,
    glass_rim: 0xffffff1a => Kind::Fixed,
    glass_grid_faint: 0xffffff12 => Kind::Fixed,
    glass_grid: 0xffffff1e => Kind::Fixed,
    glass_grid_hi: 0xffffff2b => Kind::Fixed,
    glass_bar: 0xffffff6e => Kind::Fixed,
    glass_bar_hi: 0xffffffa0 => Kind::Fixed,
    glass_ink: 0xf4f7faff => Kind::Fixed,
    glass_label: 0xd6dee6ff => Kind::Fixed,
    glass_ink_soft: 0xc2ccd8ff => Kind::Fixed,
    glass_ink_edge: 0xb4c0cdff => Kind::Fixed,
    glass_ink_muted: 0x8e9aa7ff => Kind::Fixed,
    glass_ink_dim: 0x66707cff => Kind::Fixed,
    glass_white: 0xffffffff => Kind::Fixed,
    glass_wave: 0x2fb894ff => Kind::Fixed,
    glass_core: 0x9df3d8ff => Kind::Fixed,
    glass_glow: 0x46e8a8ff => Kind::Fixed,
    glass_curve: 0x49d8e6ff => Kind::Fixed,
    glass_bin: 0xe8a33dff => Kind::Fixed,
    glass_live: 0x35c05fff => Kind::Fixed,
    glass_found: 0xf5c542ff => Kind::Fixed,
    glass_saved: 0x3d8bffff => Kind::Fixed,
    glass_danger: 0xe5484dff => Kind::Fixed,
    glass_danger_a55: 0xe5484d55 => Kind::Fixed,
    glass_warn: 0xff3b30ff => Kind::Fixed,
    glass_fail: 0xff5c5cff => Kind::Fixed,
    /// The accent where it is drawn on glass: the theme's, lifted until it
    /// shows on a dark ground.
    glass_accent: 0xff5c39ff => Kind::GlassAccent,
    glass_accent_wash: 0xff5c392e => Kind::GlassAccent,
    glass_accent_tint: 0xffd9ccff => Kind::GlassAccentTint,

    // ---- audio displays: independent of the video glass ----
    wave_bg: 0x0a0d12ff => Kind::WaveSurface,
    wave_ruler_bg: 0x0b1016ff => Kind::WaveSurface,
    wave_ink: 0xf4f7faff => Kind::WaveInk,
    wave_label: 0xd6dee6ff => Kind::WaveInk,
    wave_muted: 0x8e9aa7ff => Kind::WaveInk,
    wave_rim: 0xffffff1a => Kind::WaveLine,
    wave_grid: 0xffffff1e => Kind::WaveLine,
    wave_bar: 0xffffff6e => Kind::WaveLine,
    wave_head: 0xffffffff => Kind::WaveInk,
    wave_glow: 0x46e8a8ff => Kind::WaveSignal,
    wave_warn: 0xff3b30ff => Kind::WaveSignal,
    wave_accent: 0xff5c39ff => Kind::WaveAccent,
    wave_accent_wash: 0xff5c392e => Kind::WaveAccent,
    wave_vocals: 0x22d3eeff => Kind::WaveSignal,
    wave_drums: 0xff9f1cff => Kind::WaveSignal,
    wave_bass: 0xc026d3ff => Kind::WaveSignal,
    wave_other: 0x5b8defff => Kind::WaveSignal,
}

/// The theme a palette is worked out against.
#[derive(Clone, Copy, Debug)]
struct Ground {
    /// The page, opaque.
    bg: u32,
    /// Body ink as it lands on the page, opaque.
    ink: u32,
    /// The plain end the ink leans to: white on a dark page, black on a light.
    strong: u32,
    dark: bool,
    accent: u32,
    on_accent: u32,
}

/// CIE lightness, 0 to 100: the scale on which equal steps look equal, which
/// the luminance `contrast` counts in does not.
fn lstar(rgba: u32) -> f64 {
    let channel = |shift: u32| {
        let v = ((rgba >> shift) & 0xff) as f64 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    let y = 0.2126 * channel(24) + 0.7152 * channel(16) + 0.0722 * channel(8);
    if y <= 216.0 / 24389.0 { y * 24389.0 / 27.0 } else { 116.0 * y.cbrt() - 16.0 }
}

fn alpha(rgba: u32) -> u32 {
    rgba & 0xff
}

fn with_alpha(rgb: u32, a: u32) -> u32 {
    (rgb & 0xffff_ff00) | (a & 0xff)
}

/// The point between `from` and `to` where `measure` crosses `target`.
/// `measure` has to run one way along the mix, which lightness and contrast
/// against a fixed ground both do.
fn seek(from: u32, to: u32, target: f64, measure: impl Fn(u32) -> f64) -> u32 {
    let rising = measure(to) >= measure(from);
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        let below = measure(mix_rgb(from, to, mid)) < target;
        if below == rising { lo = mid } else { hi = mid }
    }
    mix_rgb(from, to, (lo + hi) * 0.5)
}

impl Ground {
    /// A light page takes a smaller step than a dark one for the same
    /// separation: a grey two shades under white reads as a panel, where
    /// the same distance over black is barely a change.
    const LIGHT_STEP: f64 = 0.7;

    fn target_lstar(&self, house: u32) -> f64 {
        let step = lstar(house) - lstar(Palette::HOUSE.bg);
        let page = lstar(self.bg);
        (if self.dark { page + step } else { page - step * Self::LIGHT_STEP }).clamp(0.0, 100.0)
    }

    fn surface(&self, house: u32) -> u32 {
        let target = self.target_lstar(house);
        let end = if target >= lstar(self.bg) { WHITE } else { BLACK };
        with_alpha(seek(self.bg, end, target, lstar), alpha(house))
    }

    fn wash(&self, house: u32) -> u32 {
        let (hue, sat, _) = rgb_to_hsl(house);
        let target = self.target_lstar(house);
        let at = |l: f64| hsl_to_rgb(hue, sat, l);
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..24 {
            let mid = (lo + hi) * 0.5;
            if lstar(at(mid)) < target { lo = mid } else { hi = mid }
        }
        with_alpha(at((lo + hi) * 0.5), alpha(house))
    }

    fn ink(&self, house: u32) -> u32 {
        let house_bg = Palette::HOUSE.bg;
        let asked = contrast(house, house_bg);
        let body = contrast(Palette::HOUSE.text, house_bg);
        if asked >= body {
            // Stronger than body text: part of the way to the plain end.
            let top = contrast(WHITE, house_bg);
            return mix_rgb(self.ink, self.strong, ((asked - body) / (top - body)).clamp(0.0, 1.0));
        }
        // Weaker: the same share of the contrast the theme's own body ink has.
        let have = contrast(self.ink, self.bg);
        let target = 1.0 + (asked - 1.0) * (have - 1.0) / (body - 1.0);
        let bg = self.bg;
        seek(self.bg, self.ink, target, |c| contrast(c, bg))
    }

    fn line(&self, house: u32) -> u32 {
        with_alpha(self.ink, alpha(house))
    }

    /// Enough to read a thin mark or a short word by.
    const SIGNAL_NEED: f64 = 3.2;

    fn signal(&self, house: u32) -> u32 {
        let opaque = house | 0xff;
        if contrast(opaque, self.bg) >= Self::SIGNAL_NEED {
            return house;
        }
        let (hue, sat, _) = rgb_to_hsl(opaque);
        let end = hsl_to_rgb(hue, sat, if self.dark { 0.92 } else { 0.12 });
        let bg = self.bg;
        with_alpha(seek(opaque, end, Self::SIGNAL_NEED, |c| contrast(c, bg)), alpha(house))
    }

    fn wave_ground(&self) -> Self {
        Self { bg: mix_rgb(self.bg, self.ink, 0.055), ..*self }
    }

    fn role(&self, house: u32, kind: Kind, token: &mut dyn FnMut(&str) -> Option<u32>) -> u32 {
        match kind {
            Kind::Page => self.bg,
            Kind::Surface => self.surface(house),
            Kind::Wash => self.wash(house),
            Kind::Token(name) => match token(name) {
                Some(value) => over(self.bg, value),
                None => self.surface(house),
            },
            Kind::Ink => self.ink(house),
            Kind::Line => self.line(house),
            Kind::Accent => with_alpha(self.accent, alpha(house)),
            Kind::AccentLift(t) => mix_rgb(self.accent, WHITE, t),
            Kind::AccentSink(t) => mix_rgb(self.accent, BLACK, t),
            Kind::OnAccent => self.on_accent,
            Kind::AccentWash(t) => mix_rgb(self.surface(Palette::HOUSE.raised), self.accent, t),
            Kind::AccentTint => mix_rgb(self.accent, self.strong, 0.7),
            Kind::Signal => self.signal(house),
            Kind::WaveSurface => if self.dark { house } else { with_alpha(self.wave_ground().bg, alpha(house)) },
            Kind::WaveInk => if self.dark { house } else { self.wave_ground().ink(house) },
            Kind::WaveLine => if self.dark { house } else { self.line(house) },
            Kind::WaveSignal => if self.dark { house } else { self.wave_ground().signal(house) },
            Kind::WaveAccent => if self.dark {
                self.role(house, Kind::GlassAccent, token)
            } else {
                self.wave_ground().signal(with_alpha(self.accent, alpha(house)))
            },
            Kind::GlassAccentTint => mix_rgb(self.role(Palette::HOUSE.glass_accent, Kind::GlassAccent, token), WHITE, 0.7),
            Kind::GlassAccent => {
                let glass = Palette::HOUSE.glass;
                let lifted = if contrast(self.accent, glass) >= Self::SIGNAL_NEED {
                    self.accent
                } else {
                    seek(self.accent, WHITE, Self::SIGNAL_NEED, |c| contrast(c, glass))
                };
                with_alpha(lifted, alpha(house))
            }
            Kind::Fixed => house,
        }
    }
}

impl Palette {
    /// The house look, with the accent taken from the theme's own
    /// `color_makepad` so the brand colour is decided in one place.
    pub fn house(token: &mut dyn FnMut(&str) -> Option<u32>) -> Palette {
        let brand = token("color_makepad").map(|c| c | 0xff).unwrap_or(Self::HOUSE.accent);
        if brand == Self::HOUSE.accent {
            return Self::HOUSE;
        }
        // Somebody moved the brand colour: the accent family follows it and
        // everything else stays as it is.
        let ground = Ground {
            bg: Self::HOUSE.bg,
            ink: Self::HOUSE.text,
            strong: WHITE,
            dark: true,
            accent: brand,
            on_accent: readable_on(brand, Self::HOUSE.on_accent, WHITE),
        };
        Palette::from_fn(|_, house, kind| match kind {
            Kind::Accent | Kind::AccentLift(_) | Kind::AccentSink(_) | Kind::OnAccent | Kind::AccentWash(_)
            | Kind::AccentTint | Kind::GlassAccent | Kind::GlassAccentTint | Kind::WaveAccent => ground.role(house, kind, token),
            _ => house,
        })
    }

    /// Every role worked out against the theme in force, read through
    /// `token` (a theme key to its colour).
    pub fn themed(token: &mut dyn FnMut(&str) -> Option<u32>) -> Palette {
        let bg = token("color_surface").or_else(|| token("color_bg_app")).unwrap_or(Self::HOUSE.bg) | 0xff;
        let dark = contrast(WHITE, bg) >= contrast(BLACK, bg);
        let strong = if dark { WHITE } else { BLACK };
        let ink = token("color_on_surface").or_else(|| token("color_text")).map(|c| over(bg, c)).unwrap_or(strong);
        let accent = token("color_primary").map(|c| c | 0xff).unwrap_or(Self::HOUSE.accent);
        let on_accent = readable_on(accent, token("color_on_primary").unwrap_or(strong) | 0xff, if dark { BLACK } else { WHITE });
        let ground = Ground { bg, ink, strong, dark, accent, on_accent };
        Palette::from_fn(|_, house, kind| ground.role(house, kind, token))
    }

    /// `mod.stage = { role: #xRRGGBBAA ... }`.
    pub fn script(&self) -> String {
        let mut out = String::from("mod.stage = {\n");
        for (name, rgba) in self.entries() {
            out.push_str(&format!("    {name}: #x{rgba:08x}\n"));
        }
        // The last statement of an evaluated script is swallowed, so it must
        // not be the assignment.
        out.push_str("}\ntrue\n");
        out
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::HOUSE
    }
}

impl Palette {
    /// Identity colours in the audio engine's vocals/drums/bass/other order.
    /// Waveforms, captions and their controls share this one palette.
    pub const fn stem_colors(&self) -> [u32; 4] {
        [self.wave_vocals, self.wave_drums, self.wave_bass, self.wave_other]
    }

    pub fn wave_is_light(&self) -> bool {
        contrast(BLACK, self.wave_bg) > contrast(WHITE, self.wave_bg)
    }

    /// The one look of a button, tab or segment that can be picked, across
    /// Stage: picked is the accent's wash inside an accent edge, bright
    /// ink; under the pointer it lifts; otherwise it keeps `rest`, its own
    /// face, with a faint edge.
    pub fn pick(&self, on: bool, hot: bool, rest: u32) -> Pick {
        if on {
            Pick { fill: self.accent_wash, edge: self.accent, ink: self.text_hi }
        } else {
            Pick { fill: if hot { self.control_hover } else { rest }, edge: self.line_faint, ink: self.text }
        }
    }
}

/// A picked or unpicked control's colours (`Palette::pick`).
#[derive(Clone, Copy, Debug)]
pub struct Pick {
    pub fill: u32,
    pub edge: u32,
    pub ink: u32,
}

#[derive(Default)]
struct PaletteState {
    palette: Palette,
    generation: u64,
    /// Whether `palette` was worked out from a theme rather than being
    /// Stage's own.
    themed: bool,
}

/// The palette in force.
pub fn get(cx: &mut Cx) -> Palette {
    cx.global::<PaletteState>().palette
}

/// A colour that is decided outside the table -- a stem's identity colour, a
/// deck's -- as it should be drawn ON THE PAGE: itself in Stage's own look,
/// and under a theme walked along its own lightness until it reads there, the
/// way the table's own signal colours are. For ink and thin marks; a colour
/// drawn on glass or on a dark knob body stays as it is.
pub fn on_page(cx: &mut Cx, rgba: u32) -> u32 {
    let state = cx.global::<PaletteState>();
    if !state.themed {
        return rgba;
    }
    let bg = state.palette.bg;
    let dark = contrast(WHITE, bg) >= contrast(BLACK, bg);
    // Only the page and its polarity matter to `signal`.
    Ground { bg, ink: state.palette.text, strong: if dark { WHITE } else { BLACK }, dark, accent: 0, on_accent: 0 }.signal(rgba)
}

/// Read an externally supplied audio identity colour against the waveform
/// ground. Dark instruments retain their exact original colours.
pub fn on_wave(cx: &mut Cx, rgba: u32) -> u32 {
    let palette = get(cx);
    if !palette.wave_is_light() { return rgba; }
    Ground { bg: palette.wave_bg, ink: palette.wave_ink, strong: BLACK, dark: false, accent: 0, on_accent: 0 }.signal(rgba)
}

/// Goes up by one every time the palette is rebuilt, which is every time the
/// markup is evaluated: what the app watches to know its Rust-side paints
/// have been re-applied over and must go on again.
pub fn generation(cx: &mut Cx) -> u64 {
    cx.global::<PaletteState>().generation
}

/// The markup was re-applied over the tree WITHOUT being evaluated again
/// (`Event::ScriptReapply`): the palette is what it was, and everything Rust
/// had painted is gone all the same. Moves [`generation`] so whoever watches
/// it puts its paints back.
pub fn reapplied(cx: &mut Cx) {
    let state = cx.global::<PaletteState>();
    state.generation = state.generation.wrapping_add(1);
}

/// What Stage holds still about the library's `Button` whatever sheet is on.
///
/// The fill: a sheet that gives `Button` a second fill stop (`color_2`) sets
/// it to its own face colour, which is a flat face for the library's buttons
/// and a gradient from Stage's colour to the sheet's for every button Stage
/// paints -- a lit latch fading to white. Stage's faces are flat in its own
/// look and stay flat under a sheet.
///
/// The padding: the console is laid out to the point around the base theme's
/// button (3 above and below the word, 6 either side). Every sheet doubles
/// that, and a column of lane buttons that fitted its panel runs off the
/// bottom of the window. The numbers are the base theme's own, so in Stage's
/// own look this line changes nothing.
const CONSOLE_BUTTONS: &str = "use mod.prelude.widgets_internal.*\nmod.widgets.Button.draw_bg.color_2 = vec4(-1.0, -1.0, -1.0, -1.0)\nmod.widgets.Button.padding = Inset{left: 6.0 right: 6.0 top: 3.0 bottom: 3.0}\n";

/// Build the palette for the theme in force and publish it as `mod.stage`.
/// Call after the widget library's `script_mod` and before any of Stage's.
pub fn script_mod(vm: &mut ScriptVm) {
    let sheet = desktop_style::current_name(vm).is_some();
    let house = !sheet && base_theme(vm.cx_mut()) == BaseTheme::Dark;
    let theme = vm.module(id!(theme));
    let palette = {
        let mut token = |key: &str| vm.bx.heap_ref().value(theme, LiveId::from_str(key).into(), NoTrap).as_color();
        if house { Palette::house(&mut token) } else { Palette::themed(&mut token) }
    };
    vm.eval(ScriptMod::new(&(env!("CARGO_MANIFEST_DIR")), "stage_palette", "stage_palette.splash", 0, 0, &(format!("{CONSOLE_BUTTONS}{}", palette.script())), &[]));
    let state = vm.cx_mut().global::<PaletteState>();
    state.palette = palette;
    state.themed = !house;
    state.generation = state.generation.wrapping_add(1);
}

/// A palette under a light style sheet, for tests elsewhere in the app that
/// have to hold in both looks.
#[cfg(test)]
pub fn test_light() -> Palette {
    Palette::themed(&mut tests::macos)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the macOS sheet puts in the tokens the palette reads.
    pub fn macos(key: &str) -> Option<u32> {
        Some(match key {
            "color_surface" => 0xecececff,
            "color_on_surface" => 0x242426ff,
            "color_primary" => 0x276ba5ff,
            "color_on_primary" => 0xffffffff,
            "color_inset" | "color_outset" | "color_outset_hover" => 0xffffffff,
            "color_outset_down" => 0xecececff,
            "color_label_inner" => 0x242426ff,
            _ => return None,
        })
    }

    #[test]
    fn the_house_look_is_the_literals_it_replaced() {
        let mut stock = |key: &str| (key == "color_makepad").then_some(0xff5c39ff);
        let p = Palette::house(&mut stock);
        assert_eq!(p, Palette::HOUSE);
        assert_eq!(p.bg, 0x14171cff);
        assert_eq!(p.accent, 0xff5c39ff);
        assert_eq!(p.border, 0xffffff26);
    }

    #[test]
    fn a_moved_brand_colour_moves_the_accent_family_and_nothing_else() {
        let mut moved = |key: &str| (key == "color_makepad").then_some(0x3d8bffff);
        let p = Palette::house(&mut moved);
        assert_eq!(p.accent, 0x3d8bffff);
        assert_eq!(p.accent_a80, 0x3d8bff80);
        assert_ne!(p.accent_wash, Palette::HOUSE.accent_wash);
        assert_eq!(p.panel, Palette::HOUSE.panel);
        assert_eq!(p.live, Palette::HOUSE.live);
        assert!(contrast(p.on_accent, p.accent) >= 4.5);
    }

    #[test]
    fn a_light_sheet_turns_the_page_and_keeps_everything_legible() {
        let p = Palette::themed(&mut macos);
        assert_eq!(p.bg, 0xecececff);
        // Panels step toward the ink, wells are the sheet's own white.
        assert!(lstar(p.panel) < lstar(p.bg));
        assert!(lstar(p.raised) < lstar(p.panel));
        assert_eq!(p.well, 0xffffffff);
        assert_eq!(p.control, 0xffffffff);
        // The ink ladder keeps its order and body text reads.
        assert!(contrast(p.text, p.bg) >= 7.0);
        assert!(contrast(p.muted, p.bg) >= 3.0);
        let ladder = [p.text, p.secondary, p.muted, p.dim, p.faint];
        assert!(ladder.windows(2).all(|w| contrast(w[0], p.bg) > contrast(w[1], p.bg)));
        // Hairlines are the dark ink, not white.
        assert_eq!(p.border, 0x24242626);
        // Signals are pulled to where they read on a pale page.
        for signal in [p.live, p.found, p.warm, p.danger, p.link, p.key] {
            assert!(contrast(signal | 0xff, p.bg) >= 3.0, "{signal:08x}");
        }
        // The accent is the sheet's, with ink that reads on it.
        assert_eq!(p.accent, 0x276ba5ff);
        assert!(contrast(p.on_accent, p.accent) >= 4.5);
        // Glass and video do not move.
        assert_eq!(p.glass, Palette::HOUSE.glass);
        assert_eq!(p.video, 0x000000ff);
    }

    #[test]
    fn a_dark_sheet_keeps_the_direction_of_every_step() {
        let mut omarchy = |key: &str| {
            Some(match key {
                "color_surface" => 0x101315ff,
                "color_on_surface" => 0xcaccceff,
                "color_primary" => 0x9db9f0ff,
                "color_on_primary" => 0x14233fff,
                _ => return None,
            })
        };
        let p = Palette::themed(&mut omarchy);
        assert!(lstar(p.panel) > lstar(p.bg));
        assert!(lstar(p.row_alt) < lstar(p.bg));
        assert_eq!(p.border, 0xcaccce26);
        assert_eq!(p.live, Palette::HOUSE.live);
    }

    #[test]
    fn the_script_names_every_role_once() {
        let script = Palette::HOUSE.script();
        for (name, _) in Palette::HOUSE.entries() {
            assert_eq!(script.matches(&format!("    {name}: ")).count(), 1, "{name}");
        }
        assert!(script.ends_with("true\n"));
    }
}
