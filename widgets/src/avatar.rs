//! Avatar and AvatarGroup — one person or thing in a circle or a rounded
//! square, and a row of them.
//!
//! A plate this small can carry exactly one thing, so it carries the best
//! one it has: a picture when there is one, the initials of a name when
//! there is not, and a person mark when there is neither. The three are a
//! ladder rather than three widgets, because the host almost never knows in
//! advance which rung it is on — a directory has photographs for a third of
//! its people and names for the rest.
//!
//! # Where the fallback colour comes from
//!
//! From the name, by a fixed function. The same name is the same colour on
//! every screen, in every session and on every machine, and nothing has to
//! be stored, migrated or agreed on to keep it that way — which is the
//! whole point, because a colour that is allocated instead of computed has
//! to be kept somewhere, and then two services disagree about it. The name
//! is folded first (case dropped, runs of spaces collapsed), so a record
//! that writes "ADA  SORENSEN" and one that writes "Ada Sorensen" are one
//! person.
//!
//! The colours are the theme's role containers, the same seven a badge or a
//! chip speaks, so an avatar never introduces a palette of its own. Neutral
//! is deliberately not among them: a grey plate is what a nameless thing
//! gets, and the point of the rest is that people differ.
//!
//! # What it deliberately does not do
//!
//! It does not fetch anything. The picture is an [`Image`] the host points
//! at a source; the avatar only decides what shape it is cut to and what
//! stands in while it loads.
//!
//! It does not answer a press. An avatar is a picture of someone, not a
//! control; a clickable one is an avatar inside a button, and that way the
//! press keeps a button's hover, focus ring and keyboard behaviour instead
//! of a second-rate copy of them.
//!
//! It does not name a presence in words. The dot reuses the badge's status
//! shapes, so a state is a shape as well as a colour and an operator who
//! cannot tell the amber from the green still tells away from online.
//!
//! # The group
//!
//! [`AvatarGroup`] overlaps its children and ends the row with a "+3" when
//! there are more people than plates. `max` counts PLATES, the cap
//! included, so a row is never wider than it was told it could be — a cap
//! whose whole job is to bound the width would be a poor cap if the width
//! grew by one every time it appeared. The cap sits under the last face,
//! which is what "there are more behind these" looks like.
use crate::{
    badge::{
        corner_offset, measure, sized, BadgeCorner, BadgeIntent, BadgePalette, StatusDot,
        StatusKind,
    },
    image::Image,
    makepad_derive_widget::*,
    makepad_draw::*,
    view::View,
    widget::*,
};

/// The plate's outline: a circle, a soft rectangle, or a hard one.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum AvatarShape {
    #[pick]
    Circle = 0,
    Rounded = 1,
    Square = 2,
}

/// Whether someone is about, and how much so. `Hidden` draws no dot at all,
/// which is what a plate standing for a thing rather than a person wants.
#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
#[repr(u32)]
pub enum AvatarPresence {
    #[pick]
    Hidden = 0,
    Online = 1,
    Away = 2,
    Busy = 3,
    Offline = 4,
    Unknown = 5,
}

impl AvatarPresence {
    /// The status the dot is drawn as. The shapes are the badge's, so
    /// online is a filled circle, away a triangle, busy a filled square,
    /// offline a hollow ring and unknown a broken one: the state survives a
    /// screenshot in grey.
    pub fn status(self) -> Option<StatusKind> {
        match self {
            AvatarPresence::Hidden => None,
            AvatarPresence::Online => Some(StatusKind::Success),
            AvatarPresence::Away => Some(StatusKind::Warning),
            AvatarPresence::Busy => Some(StatusKind::Error),
            AvatarPresence::Offline => Some(StatusKind::Pending),
            AvatarPresence::Unknown => Some(StatusKind::Unknown),
        }
    }

    /// The name a test waits on, and the name [`AvatarPresence::from_name`]
    /// accepts.
    pub fn name(self) -> &'static str {
        match self {
            AvatarPresence::Hidden => "hidden",
            AvatarPresence::Online => "online",
            AvatarPresence::Away => "away",
            AvatarPresence::Busy => "busy",
            AvatarPresence::Offline => "offline",
            AvatarPresence::Unknown => "unknown",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        [
            AvatarPresence::Hidden,
            AvatarPresence::Online,
            AvatarPresence::Away,
            AvatarPresence::Busy,
            AvatarPresence::Offline,
            AvatarPresence::Unknown,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*

    // Registered before the `use` below, because a block's `use` is a
    // snapshot taken when it runs. A bare variant name at a call site
    // resolves against the property's own type, so `Rounded` and `Square`
    // live beside the badge's shapes without changing what they mean there.
    mod.widgets.AvatarShape = set_type_default() do #(AvatarShape::script_api(vm))
    mod.widgets.splat(mod.widgets.AvatarShape)
    mod.widgets.AvatarPresence = set_type_default() do #(AvatarPresence::script_api(vm))
    mod.widgets.splat(mod.widgets.AvatarPresence)

    use mod.widgets.*

    // The whole face lives in the type default rather than in the widget's
    // preset, so the plate and the disc behind the presence dot are one
    // shader written once.
    set_type_default() do #(DrawAvatar::script_shader(vm)){
        ..mod.draw.DrawQuad
        pixel: fn() {
            let sdf = Sdf2d.viewport(self.pos * self.rect_size)
            // The ring is stroked on the path, so the path is pulled in by
            // half of it and the whole ring stays inside the plate's box.
            let g = self.ring_size * 0.5
            sdf.box(g, g, self.rect_size.x - g * 2.0, self.rect_size.y - g * 2.0, self.radius)
            sdf.fill_keep(self.color)
            // The stroke runs whatever the ring is, because it is also what
            // clears the shape for the mark below; a ring of no width
            // strokes a colour with no alpha and shows nothing.
            sdf.stroke(self.ring_color * step(0.001, self.ring_size), self.ring_size)
            if self.mark > 0.0 {
                let s = min(self.rect_size.x, self.rect_size.y)
                let mx = self.rect_size.x * 0.5
                let my = self.rect_size.y * 0.5
                // Head and shoulders as a circle and a rounded box: a small
                // mark traced as a path does not paint reliably here. Both
                // stay well inside the plate, so a round one does not cut
                // the shoulders off at the sides.
                sdf.circle(mx, my - s * 0.135, s * 0.135)
                sdf.fill(self.mark_color * self.mark)
                sdf.box(mx - s * 0.21, my + s * 0.045, s * 0.42, s * 0.25, s * 0.06)
                sdf.fill(self.mark_color * self.mark)
            }
            return sdf.result
        }
    }

    mod.widgets.AvatarBase = #(Avatar::register_widget(vm))
    /** One person or thing: a picture, else initials, else a mark. */
    mod.widgets.Avatar = set_type_default() do mod.widgets.AvatarBase{
        width: Fit
        height: Fit
        /** who this is; both the initials and the colour come from it */
        name: ""
        /** letters drawn instead of the ones the name yields */
        initials: ""
        /** how many letters a name yields 1..2 step 1 */
        initials_max: 2
        /** Circle Rounded Square */
        shape: Circle
        /** the plate across, in pixels, when the walk does not say 16..160 step 1 */
        plate: 36.
        /** corner radius of the Rounded shape 0..40 step 0.5 */
        radius: theme.radius_m
        /** a ring round the plate, so an overlapping row reads 0..8 step 0.5 */
        ring: 0.
        /** the ring's colour: whatever the plate is sitting on */
        ring_color: theme.color_surface
        /** initials height as a share of the plate 0.2..0.7 step 0.01 */
        text_scale: 0.4
        /** Hidden Online Away Busy Offline Unknown */
        presence: Hidden
        /** which corner the presence dot sits in */
        presence_corner: BadgeCorner.BottomRight
        /** presence dot across as a share of the plate 0.15..0.5 step 0.01 */
        presence_scale: 0.3
        /** take the plate's colour from the name rather than from intent */
        tint_from_name: true
        /** the colour when tint_from_name is off: Neutral Primary Secondary Tertiary Error Warning Success Info */
        intent: Neutral
        /** drawn at all */
        visible: true
        /** dimmed */
        disabled: false
        /** ink and fill alpha while disabled 0..1 step 0.05 */
        disabled_opacity: theme.state_disabled_content_opacity
        /** The colour of every role, shared with the badges and the chips,
         * so one meaning is one colour across the library. */
        palette: mod.widgets.BadgePalette{}

        /** the face; `picture +: {src: crate_resource("...")}` at a call site */
        picture: mod.widgets.Image{
            // Cropped to the plate rather than squashed into it: a face
            // stretched to a square stops being that face. Nothing here
            // says what shape it is cut to: the plate's corner depends on
            // a size, so the widget writes it on every draw.
            fit: ImageFit.CropToFill
        }

        /** the presence dot: the badge's status shapes in the presence colours */
        presence_dot: mod.widgets.StatusDot{
            draw_bg +: {
                color_success: theme.color_presence_online
                color_warning: theme.color_presence_away
                color_error: theme.color_presence_busy
                color_pending: theme.color_presence_offline
            }
        }

        draw_text +: {
            // line_spacing 1.0, so the ink sits where the arithmetic in
            // draw_walk says it does.
            text_style: theme.font_bold{font_size: 14 line_spacing: 1.0}
            color: theme.color_text
        }
    }

    /** A small plate, for a row of a list or a line of running text. */
    mod.widgets.AvatarSmall = mod.widgets.Avatar{
        plate: 24.
        radius: theme.radius_s
    }

    /** A large plate, for the head of a record. */
    mod.widgets.AvatarLarge = mod.widgets.Avatar{
        plate: 56.
    }

    /** The plate at the top of a profile. */
    mod.widgets.AvatarXl = mod.widgets.Avatar{
        plate: 96.
    }

    /** A soft rectangle rather than a circle: a team, a project, a file. */
    mod.widgets.AvatarSquare = mod.widgets.Avatar{
        shape: Rounded
    }

    mod.widgets.AvatarGroupBase = #(AvatarGroup::register_widget(vm))
    /** Several avatars overlapping, ending in a count of the ones not
     * shown. Its children are avatars; it owns their size, their ring and
     * where they sit. */
    mod.widgets.AvatarGroup = set_type_default() do mod.widgets.AvatarGroupBase{
        width: Fit
        height: Fit
        // Overlay, so every plate starts at the same origin and its own
        // margin is what carries it along the row.
        flow: Overlay
        /** how many plates the row may show, the cap counted 1..12 step 1 */
        max: 5
        /** how much of a plate the next one covers 0..0.9 step 0.05 */
        overlap: 0.32
        /** every plate across, in pixels 16..160 step 1 */
        plate: 36.
        /** the ring each plate gets, so the overlap reads 0..8 step 0.5 */
        ring: 2.
        /** the plate at the end, saying how many are not shown */
        more: mod.widgets.Avatar{}
    }
}

/// The plate, the ring round it and the person mark. Every value is written
/// by the widget each draw, so a name, a shape or a size can change without
/// a shader recompile.
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawAvatar {
    #[deref]
    draw_super: DrawQuad,
    /// HALF the visual corner radius: `sdf.box` draws twice what it is
    /// given.
    #[live]
    radius: f32,
    #[live]
    color: Vec4f,
    #[live]
    ring_color: Vec4f,
    #[live]
    ring_size: f32,
    /// 1 draws the person mark, 0 leaves the plate bare.
    #[live]
    mark: f32,
    #[live]
    mark_color: Vec4f,
}

/// The seven colours a name may land on. Neutral is not among them: it is
/// what a plate with no name gets, and a row of people is meant to look
/// like a row of different people.
const TINTS: [BadgeIntent; 7] = [
    BadgeIntent::Primary,
    BadgeIntent::Secondary,
    BadgeIntent::Tertiary,
    BadgeIntent::Info,
    BadgeIntent::Success,
    BadgeIntent::Warning,
    BadgeIntent::Error,
];

/// The name with its case dropped and its spacing collapsed. Two records of
/// one person rarely agree on either, and a colour that changed with the
/// spelling would defeat the point of computing it.
fn fold(name: &str) -> String {
    name.split_whitespace()
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The colour a name always gets.
///
/// A fixed hash rather than an allocated index, so the same person is the
/// same colour in two processes that have never spoken to each other and
/// nothing has to be stored to keep it so. It is FNV-1a written out here
/// rather than the standard library's hasher, whose output is deliberately
/// not stable between builds — which would change the colour under a person
/// for no reason they could see.
pub fn tint_of(name: &str) -> BadgeIntent {
    let key = fold(name);
    if key.is_empty() {
        return BadgeIntent::Neutral;
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    TINTS[(hash % TINTS.len() as u64) as usize]
}

/// The first letter of a word, upper-cased; nothing for a word that is all
/// punctuation.
fn first_letter(word: &str) -> Option<char> {
    word.chars()
        .find(|c| c.is_alphanumeric())
        .and_then(|c| c.to_uppercase().next())
}

/// The letters a name is drawn as: the first word's, and the last word's
/// when there is more than one and `max` allows two.
///
/// The MIDDLE names are skipped rather than crowded in, because a middle
/// name is not what anybody is called, and three letters in a 36 point
/// circle are three grey smudges. A word with no letter or digit in it is
/// not a word — a lone hyphen between two names must not become an initial.
pub fn initials_of(name: &str, max: usize) -> String {
    let max = max.clamp(1, 2);
    let words: Vec<&str> = name
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .collect();
    let Some(first) = words.first().and_then(|word| first_letter(word)) else {
        return String::new();
    };
    let mut out = String::new();
    out.push(first);
    if max >= 2 && words.len() > 1 {
        if let Some(last) = first_letter(words[words.len() - 1]) {
            out.push(last);
        }
    }
    out
}

/// Where the presence dot's top-left goes, relative to the plate's
/// top-left.
///
/// On a circle the dot rides the rim at forty-five degrees, because the
/// corner of a circle's bounding box is empty air and a dot hung there
/// floats away from the plate. On a rectangle it hangs off the corner by
/// the badge's own rule, which is where the eye already looks for a mark.
pub fn presence_offset(
    corner: BadgeCorner,
    shape: AvatarShape,
    host: DVec2,
    dot: f64,
) -> DVec2 {
    if shape != AvatarShape::Circle {
        return corner_offset(corner, host, dvec2(dot, dot), 0.25);
    }
    let diag = std::f64::consts::FRAC_1_SQRT_2;
    let r = host * 0.5;
    let centre = match corner {
        BadgeCorner::TopRight => dvec2(r.x + r.x * diag, r.y - r.y * diag),
        BadgeCorner::TopLeft => dvec2(r.x - r.x * diag, r.y - r.y * diag),
        BadgeCorner::BottomRight => dvec2(r.x + r.x * diag, r.y + r.y * diag),
        BadgeCorner::BottomLeft => dvec2(r.x - r.x * diag, r.y + r.y * diag),
    };
    centre - dvec2(dot * 0.5, dot * 0.5)
}

/// How far apart two plates in a row stand. The overlap is a share of the
/// plate rather than a number of points, so a row of small plates and a row
/// of large ones look like the same row.
pub fn stride_of(plate: f64, overlap: f64) -> f64 {
    plate * (1.0 - overlap.clamp(0.0, 0.9))
}

/// How wide a row of `plates` is.
pub fn row_width(plates: usize, plate: f64, overlap: f64) -> f64 {
    if plates == 0 {
        return 0.0;
    }
    (plates - 1) as f64 * stride_of(plate, overlap) + plate
}

/// How many faces a row shows and how many it counts, for `total` people in
/// a row of at most `max` PLATES.
///
/// The cap is one of the plates, not an extra one: the whole job of a cap
/// is to bound the row's width, and a cap that made the row one plate wider
/// every time it appeared would not be doing it. A `max` of nothing is read
/// as one, because a row of no plates cannot say anything at all.
pub fn group_split(total: usize, max: usize) -> (usize, usize) {
    let max = max.max(1);
    if total <= max {
        return (total, 0);
    }
    (max - 1, total - (max - 1))
}

/// How far below the top of its line box a glyph's ink starts, as a share
/// of the font size. `draw_abs` takes the line box, so centring the box
/// alone leaves the letters riding high.
const INK_DROP: f64 = 0.30;

fn dimmed(color: Vec4f, opacity: f32) -> Vec4f {
    Vec4f { w: color.w * opacity, ..color }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Avatar {
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
    draw_bg: DrawAvatar,
    /// The disc behind the presence dot. The same shader as the plate,
    /// drawn a second time; a separate layer because `draw_bg`'s area is
    /// what a redraw dirties and that has to stay the whole plate.
    #[live]
    draw_dot_ring: DrawAvatar,
    #[live]
    draw_text: DrawText,
    /// The face. The avatar never loads it; it only says what shape it is
    /// cut to.
    #[live]
    pub picture: Image,
    #[live]
    pub presence_dot: StatusDot,

    /// Who this is. The initials and the colour both come from it.
    #[live]
    pub name: String,
    /// Letters drawn instead of the ones the name yields, verbatim: this is
    /// how a group's cap says "+3".
    #[live]
    pub initials: String,
    #[live(2usize)]
    pub initials_max: usize,
    #[live]
    pub shape: AvatarShape,
    /// The plate across, when the walk does not say.
    #[live(36.0)]
    pub plate: f64,
    /// Corner radius of the Rounded shape.
    #[live(6.0)]
    pub radius: f64,
    #[live]
    pub ring: f64,
    #[live]
    pub ring_color: Vec4f,
    /// Initials height as a share of the plate, so an avatar at any size
    /// has letters of the right weight for it.
    #[live(0.4)]
    pub text_scale: f64,
    #[live]
    pub presence: AvatarPresence,
    #[live]
    pub presence_corner: BadgeCorner,
    #[live(0.3)]
    pub presence_scale: f64,
    #[live(true)]
    pub tint_from_name: bool,
    /// The colour when `tint_from_name` is off: a team's own, or a state.
    #[live]
    pub intent: BadgeIntent,
    #[live]
    pub palette: BadgePalette,
    #[live(0.38)]
    pub disabled_opacity: f32,
    #[live(true)]
    #[visible]
    visible: bool,
    /// Dimmed. A story or a form can set it in the DSL, and `set_disabled`
    /// moves the same flag.
    #[live]
    pub disabled: bool,
}

impl Avatar {
    /// What the plate shows instead of a picture: the letters it was given,
    /// else the ones the name yields, else nothing — and nothing is what
    /// puts the person mark on the plate.
    pub fn face_text(&self) -> String {
        if !self.initials.is_empty() {
            return self.initials.clone();
        }
        initials_of(&self.name, self.initials_max)
    }

    /// The role whose container the plate is filled with.
    pub fn tint(&self) -> BadgeIntent {
        if self.tint_from_name {
            tint_of(&self.name)
        } else {
            self.intent
        }
    }

    /// The visual corner radius for a plate `size` points across.
    fn visual_radius(&self, size: f64) -> f64 {
        match self.shape {
            AvatarShape::Circle => size * 0.5,
            AvatarShape::Rounded => self.radius.clamp(0.0, size * 0.5),
            AvatarShape::Square => 0.0,
        }
    }

    pub fn set_name(&mut self, cx: &mut Cx, name: &str) {
        if self.name != name {
            self.name = name.to_string();
            self.draw_bg.redraw(cx);
        }
    }

    pub fn set_presence(&mut self, cx: &mut Cx, presence: AvatarPresence) {
        if self.presence != presence {
            self.presence = presence;
            self.draw_bg.redraw(cx);
        }
    }

    /// Put this plate `left` points along its group's row, `plate` points
    /// across, wearing a ring `ring` points wide. A group owns these for
    /// every child it has, because a row of five sizes is not a row.
    pub fn place(&mut self, left: f64, plate: f64, ring: f64) {
        self.walk.margin.left = left;
        self.walk.width = Size::Fixed(plate);
        self.walk.height = Size::Fixed(plate);
        self.ring = ring;
    }
}

impl Widget for Avatar {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        let walk = sized(walk, self.plate, self.plate);
        let has_picture = self.picture.has_content();
        let letters = self.face_text();
        let family = self.palette.family(self.tint());
        let opacity = if self.disabled { self.disabled_opacity } else { 1.0 };

        // The corner radius has to be on the quad before it is drawn and the
        // drawn rect only exists afterwards, so it is worked out from the
        // walk. A walk that is neither fixed nor fit peeks as NaN; `plate`
        // is the honest answer there.
        let peek = cx.peek_walk_turtle(walk).size;
        let guess = if peek.x.is_nan() || peek.y.is_nan() {
            self.plate
        } else {
            peek.x.min(peek.y)
        };
        self.draw_bg.color = dimmed(family.container, opacity);
        self.draw_bg.ring_color = dimmed(self.ring_color, opacity);
        self.draw_bg.ring_size = self.ring.max(0.0) as f32;
        self.draw_bg.mark = if has_picture || !letters.is_empty() { 0.0 } else { 1.0 };
        self.draw_bg.mark_color = dimmed(family.on_container, opacity);
        self.draw_bg.radius = (self.visual_radius(guess) * 0.5) as f32;
        // The plate is a box of its own, and the picture and the presence dot
        // are placed inside it. Each of them is an absolute walk, and an
        // absolute walk drawn straight into the parent joins the parent's
        // row: a row that centres its children vertically centred the dot
        // too, which put a top corner's dot at the plate's middle and a
        // bottom corner's below the plate, out of the row's clip, leaving
        // only the disc behind it. Inside a box that aligns nothing they stay
        // where they were put, and the row moves the whole avatar as one.
        // No clip, so a dot hanging off a corner is not cut off.
        cx.begin_turtle(
            walk,
            Layout {
                flow: Flow::Overlay,
                clip_x: false,
                clip_y: false,
                ..Default::default()
            },
        );
        let rect = self.draw_bg.draw_walk(cx, Walk::fill());
        let size = rect.size.x.min(rect.size.y);

        // The picture sits inside the ring, so the ring is not painted over
        // by the very face it is there to separate.
        let ring = self.ring.max(0.0).min(size * 0.25);
        let inner = Rect {
            pos: rect.pos + dvec2(ring, ring),
            size: rect.size - dvec2(ring * 2.0, ring * 2.0),
        };
        let inner_size = inner.size.x.min(inner.size.y);
        let inner_radius = match self.shape {
            AvatarShape::Circle => inner_size * 0.5,
            AvatarShape::Rounded => (self.radius - ring).clamp(0.0, inner_size * 0.5),
            AvatarShape::Square => 0.0,
        };
        // Halved, because `sdf.box` draws twice the radius it is handed —
        // the same number a view takes for its own `border_radius`.
        self.picture.draw_bg.border_radius = (inner_radius * 0.5) as f32;
        // The picture is drawn on every frame even when it holds nothing,
        // because that draw is what starts the load; until there is
        // something in it, it is drawn at no opacity and the initials stand
        // in. Set, draw, RESTORE: the opacity is the call site's.
        let rest_opacity = self.picture.draw_bg.opacity;
        self.picture.draw_bg.opacity = if has_picture { rest_opacity * opacity } else { 0.0 };
        cx.begin_turtle(
            Walk {
                abs_pos: Some(inner.pos),
                margin: Inset::default(),
                width: Size::Fixed(inner.size.x),
                height: Size::Fixed(inner.size.y),
                ..Default::default()
            },
            Layout::flow_down(),
        );
        self.picture
            .draw_walk(cx, scope, Walk::fixed(inner.size.x, inner.size.y))?;
        cx.end_turtle();
        self.picture.draw_bg.opacity = rest_opacity;

        if !has_picture && !letters.is_empty() {
            let font = (size * self.text_scale).max(1.0);
            let rest_color = self.draw_text.color;
            let rest_font = self.draw_text.text_style.font_size;
            self.draw_text.color = dimmed(family.on_container, opacity);
            self.draw_text.text_style.font_size = font as f32;
            let width = measure(&self.draw_text, cx, &letters);
            let x = rect.pos.x + (rect.size.x - width) * 0.5;
            let y = rect.pos.y + (rect.size.y - font) * 0.5 - font * INK_DROP;
            self.draw_text.draw_abs(cx, dvec2(x, y), &letters);
            self.draw_text.color = rest_color;
            self.draw_text.text_style.font_size = rest_font;
        }

        if let Some(status) = self.presence.status() {
            let dot = (size * self.presence_scale).max(6.0);
            let pos = rect.pos + presence_offset(self.presence_corner, self.shape, rect.size, dot);
            // A disc of the surface behind the dot, so a green dot on a
            // green plate is still a dot.
            let pad = (dot * 0.16).max(1.5);
            let backing = dot + pad * 2.0;
            self.draw_dot_ring.color = dimmed(self.ring_color, opacity);
            self.draw_dot_ring.ring_color = Vec4f::default();
            self.draw_dot_ring.ring_size = 0.0;
            self.draw_dot_ring.mark = 0.0;
            self.draw_dot_ring.mark_color = Vec4f::default();
            self.draw_dot_ring.radius = (backing * 0.25) as f32;
            self.draw_dot_ring.draw_abs(
                cx,
                Rect { pos: pos - dvec2(pad, pad), size: dvec2(backing, backing) },
            );
            // The dot keeps its full strength on a dimmed plate: whether
            // somebody is at their desk is a fact about them, not a thing
            // this control is offering to do.
            self.presence_dot.status = status;
            self.presence_dot.draw_walk(
                cx,
                scope,
                Walk {
                    abs_pos: Some(pos),
                    margin: Inset::default(),
                    width: Size::Fixed(dot),
                    height: Size::Fixed(dot),
                    ..Default::default()
                },
            )?;
        }
        cx.end_turtle();
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        // The picture decodes on the event pass. Without this an async
        // source never finishes and the initials stand in forever.
        self.picture.handle_event(cx, event, scope);
    }

    /// The name, which is what the plate is standing for.
    fn text(&self) -> String {
        self.name.clone()
    }

    fn set_text(&mut self, cx: &mut Cx, v: &str) {
        self.set_name(cx, v);
    }

    fn set_disabled(&mut self, cx: &mut Cx, disabled: bool) {
        if self.disabled != disabled {
            self.disabled = disabled;
            self.draw_bg.redraw(cx);
        }
    }

    fn disabled(&self, _cx: &Cx) -> bool {
        self.disabled
    }

    /// What is actually on the plate, so a test can tell the three rungs
    /// apart without reading pixels.
    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        if self.picture.has_content() {
            return Some("picture".to_string());
        }
        let letters = self.face_text();
        Some(if letters.is_empty() { "person".to_string() } else { letters })
    }
}

impl AvatarRef {
    pub fn set_name(&self, cx: &mut Cx, name: &str) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_name(cx, name);
        }
    }

    pub fn name(&self) -> String {
        self.borrow().map(|inner| inner.name.clone()).unwrap_or_default()
    }

    /// The letters on the plate, whether they were given or worked out.
    pub fn initials(&self) -> String {
        self.borrow().map(|inner| inner.face_text()).unwrap_or_default()
    }

    pub fn set_presence(&self, cx: &mut Cx, presence: AvatarPresence) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_presence(cx, presence);
        }
    }

    pub fn presence(&self) -> AvatarPresence {
        self.borrow()
            .map(|inner| inner.presence)
            .unwrap_or(AvatarPresence::Hidden)
    }
}

/// A row of avatars, overlapping, ending in a count of the ones it did not
/// draw.
#[derive(Script, ScriptHook, Widget)]
pub struct AvatarGroup {
    #[deref]
    view: View,
    /// The plate at the end. A property rather than a child so it is always
    /// there to be written to, and so it is drawn last and sits where the
    /// arithmetic put it rather than where the flow would.
    #[live]
    pub more: Avatar,
    /// How many plates the row may show, the cap counted.
    #[live(5usize)]
    pub max: usize,
    /// How much of a plate the next one covers, 0..0.9.
    #[live(0.32)]
    pub overlap: f64,
    #[live(36.0)]
    pub plate: f64,
    #[live(2.0)]
    pub ring: f64,
    #[rust]
    hidden: usize,
}

impl AvatarGroup {
    fn stride(&self) -> f64 {
        stride_of(self.plate, self.overlap)
    }

    /// Size every face, place it along the row, and hide the ones the cap
    /// speaks for. Run on every draw rather than once: `max`, `plate` and
    /// `overlap` are live properties and the tweaker may have moved any of
    /// them since the last frame.
    ///
    /// A child that is not an avatar keeps whatever size and place its own
    /// DSL gave it, but is still counted, because the row is a count of
    /// what is in it.
    fn arrange(&mut self, cx: &mut Cx) -> (usize, usize) {
        let mut kids = Vec::new();
        self.view.children(&mut |_id, child| kids.push(child));
        let (shown, hidden) = group_split(kids.len(), self.max);
        let step_x = self.stride();
        for (index, child) in kids.iter().enumerate() {
            child.set_visible(cx, index < shown);
            if index < shown {
                // The ref and its guard are both named locals, and in that
                // order: an `if let` holds its temporary to the end of the
                // block, which is after the ref it borrowed from would have
                // been dropped. Naming the guard makes it drop first.
                let child_avatar = child.as_avatar();
                let mut guard = child_avatar.borrow_mut();
                if let Some(face) = guard.as_mut() {
                    face.place(index as f64 * step_x, self.plate, self.ring);
                }
            }
        }
        self.more.place(0.0, self.plate, self.ring);
        // The cap's letters are given, not worked out: "+3" run through the
        // initials rule would come out as "3".
        self.more.initials = format!("+{hidden}");
        (shown, hidden)
    }

    /// How many people the row is not showing.
    pub fn hidden(&self) -> usize {
        self.hidden
    }

    pub fn set_max(&mut self, cx: &mut Cx, max: usize) {
        if self.max != max {
            self.max = max;
            self.view.redraw(cx);
        }
    }
}

impl Widget for AvatarGroup {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let (shown, hidden) = self.arrange(cx.cx.cx);
        self.hidden = hidden;
        let plates = shown + usize::from(hidden > 0);
        let width = row_width(plates, self.plate, self.overlap);
        // The row is given the width the cap needs as well, even though the
        // cap is drawn outside the children's turtle: a group that reported
        // itself one plate narrower than it draws would be walked over by
        // whatever comes after it.
        let step = self
            .view
            .draw_walk(cx, scope, sized(walk, width, self.plate));
        if !step.is_done() {
            return step;
        }
        if hidden > 0 {
            let rect = self.view.area().rect(cx);
            let x = rect.pos.x + shown as f64 * self.stride();
            cx.begin_turtle(
                Walk {
                    abs_pos: Some(dvec2(x, rect.pos.y)),
                    margin: Inset::default(),
                    width: Size::Fixed(self.plate),
                    height: Size::Fixed(self.plate),
                    ..Default::default()
                },
                Layout::flow_down(),
            );
            self.more
                .draw_walk(cx, scope, Walk::fixed(self.plate, self.plate))?;
            cx.end_turtle();
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        self.more.handle_event(cx, event, scope);
    }

    /// What the cap says, or nothing when everybody fits.
    fn text(&self) -> String {
        if self.hidden > 0 {
            format!("+{}", self.hidden)
        } else {
            String::new()
        }
    }

    fn snapshot_value(&self, _cx: &Cx) -> Option<String> {
        Some(self.hidden.to_string())
    }
}

impl AvatarGroupRef {
    /// How many people the row is not showing.
    pub fn hidden(&self) -> usize {
        self.borrow().map(|inner| inner.hidden()).unwrap_or(0)
    }

    /// How many plates the row may show, the cap counted.
    pub fn max(&self) -> usize {
        self.borrow().map(|inner| inner.max).unwrap_or(0)
    }

    pub fn set_max(&self, cx: &mut Cx, max: usize) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.set_max(cx, max);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn one_word_is_one_letter() {
        // Two letters of one word would be a syllable, not an initial.
        assert_eq!(initials_of("Ada", 2), "A");
        assert_eq!(initials_of("okafor", 2), "O");
    }

    #[test]
    fn two_words_are_the_first_of_each() {
        assert_eq!(initials_of("Mira Okafor", 2), "MO");
        assert_eq!(initials_of("  mira   okafor  ", 2), "MO");
    }

    #[test]
    fn three_words_skip_the_middle() {
        // Nobody is called by their middle name, and three letters in a
        // small circle are three smudges.
        assert_eq!(initials_of("Mira Adaeze Okafor", 2), "MO");
        assert_eq!(initials_of("a b c d e", 2), "AE");
    }

    #[test]
    fn punctuation_is_not_a_letter() {
        assert_eq!(initials_of("Jean-Luc Vermeer", 2), "JV");
        assert_eq!(initials_of("O'Neill", 2), "O");
        // A lone hyphen between two names is not a word and must not become
        // an initial.
        assert_eq!(initials_of("Mira - Okafor", 2), "MO");
        assert_eq!(initials_of("(Mira) [Okafor]", 2), "MO");
    }

    #[test]
    fn a_name_with_no_letters_in_it_yields_none() {
        // Nothing is what puts the person mark on the plate, so all three
        // of these have to reach it rather than draw a stray glyph.
        assert_eq!(initials_of("", 2), "");
        assert_eq!(initials_of("     ", 2), "");
        assert_eq!(initials_of("!!! ???", 2), "");
    }

    #[test]
    fn a_one_letter_plate_takes_only_the_first() {
        assert_eq!(initials_of("Mira Okafor", 1), "M");
        // Out of range clamps rather than panicking: the count is a live
        // property and the tweaker can put anything in it.
        assert_eq!(initials_of("Mira Okafor", 0), "M");
        assert_eq!(initials_of("Mira Okafor", 9), "MO");
    }

    #[test]
    fn letters_outside_ascii_are_upper_cased_too() {
        assert_eq!(initials_of("\u{e5}sa \u{f6}berg", 2), "\u{c5}\u{d6}");
        assert_eq!(initials_of("\u{440}\u{43e}\u{43c}\u{430}", 2), "\u{420}");
    }

    #[test]
    fn a_digit_counts_as_a_letter() {
        // Plates stand for things as well as people: a room, a build, a
        // release.
        assert_eq!(initials_of("4th Floor", 2), "4F");
    }

    #[test]
    fn the_same_person_is_the_same_colour() {
        // Case and spacing are what two records of one person disagree
        // about most, so neither may reach the hash.
        assert_eq!(tint_of("Mira Okafor"), tint_of("mira okafor"));
        assert_eq!(tint_of("Mira Okafor"), tint_of("  MIRA   Okafor "));
        assert_eq!(tint_of("Mira Okafor"), tint_of("Mira\tOkafor"));
    }

    #[test]
    fn a_different_person_is_a_different_colour_often_enough() {
        let names = [
            "Mira Okafor",
            "Jun Park",
            "Ada Sorensen",
            "Tomas Ruiz",
            "Leah Bright",
            "Noor Haddad",
            "Kai",
            "Jean-Luc Vermeer",
        ];
        let spread: HashSet<u32> = names.iter().map(|n| tint_of(n) as u32).collect();
        assert!(spread.len() >= 4, "eight names landed on {} colours", spread.len());
        assert_ne!(tint_of("Mira Okafor"), tint_of("Jun Park"));
    }

    #[test]
    fn a_nameless_plate_is_the_quiet_one() {
        // Neutral is reserved for this: a plate standing for nobody must
        // not borrow a person's colour.
        assert_eq!(tint_of(""), BadgeIntent::Neutral);
        assert_eq!(tint_of("   "), BadgeIntent::Neutral);
        assert!(!TINTS.contains(&BadgeIntent::Neutral));
    }

    #[test]
    fn a_row_that_fits_shows_everybody() {
        assert_eq!(group_split(3, 5), (3, 0));
        assert_eq!(group_split(5, 5), (5, 0));
    }

    #[test]
    fn the_cap_is_one_of_the_plates_not_an_extra_one() {
        // Five plates asked for, five plates drawn: four faces and a cap
        // that speaks for the other two.
        assert_eq!(group_split(6, 5), (4, 2));
        assert_eq!(group_split(100, 3), (2, 98));
    }

    #[test]
    fn a_row_of_one_plate_is_the_cap_alone() {
        assert_eq!(group_split(1, 1), (1, 0));
        assert_eq!(group_split(2, 1), (0, 2));
        // A row of no plates cannot say anything, so nothing is read as one.
        assert_eq!(group_split(3, 0), (0, 3));
    }

    #[test]
    fn an_empty_row_has_no_cap() {
        assert_eq!(group_split(0, 5), (0, 0));
        assert_eq!(group_split(0, 0), (0, 0));
    }

    #[test]
    fn the_row_is_its_plates_less_what_they_cover() {
        // Four plates 40 across overlapping by a quarter: three strides of
        // 30 and one whole plate.
        assert_eq!(row_width(4, 40.0, 0.25), 130.0);
        assert_eq!(row_width(1, 40.0, 0.25), 40.0, "one plate has nothing to overlap");
        assert_eq!(row_width(0, 40.0, 0.25), 0.0);
        // An overlap of everything would stack the row into a single plate,
        // so it stops short of that.
        assert!((row_width(2, 40.0, 5.0) - 44.0).abs() < 1e-9);
    }

    #[test]
    fn the_presence_dot_rides_the_rim_of_a_circle() {
        // The corner of a circle's bounding box is empty air; a dot hung
        // there floats off the plate.
        let host = dvec2(40.0, 40.0);
        let at = presence_offset(BadgeCorner::BottomRight, AvatarShape::Circle, host, 12.0);
        let from_middle = at + dvec2(6.0, 6.0) - dvec2(20.0, 20.0);
        let reach = (from_middle.x * from_middle.x + from_middle.y * from_middle.y).sqrt();
        assert!((reach - 20.0).abs() < 0.001, "the dot's centre sits on the rim");
        assert!(from_middle.x > 0.0 && from_middle.y > 0.0, "in the bottom right");
    }

    #[test]
    fn a_rectangle_hangs_its_dot_off_the_corner() {
        let host = dvec2(40.0, 40.0);
        let at = presence_offset(BadgeCorner::TopLeft, AvatarShape::Square, host, 12.0);
        assert_eq!(at, corner_offset(BadgeCorner::TopLeft, host, dvec2(12.0, 12.0), 0.25));
    }

    /// The face is cut to the plate, and to the plate INSIDE the ring: a
    /// picture cut to the outer edge would be painted over the very band
    /// the ring is there to draw. The picture cuts itself now, so what is
    /// worth pinning is the number the widget hands it — halved, because
    /// `sdf.box` draws twice what it is given.
    #[test]
    fn the_face_is_cut_to_the_plate_inside_the_ring() {
        use crate::makepad_draw::cx_draw::CxDraw;
        use crate::makepad_script::script;

        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut row = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 400 height: 120
                    flow: Right
                    Avatar{plate: 40. name: "Mira Okafor"}
                    Avatar{plate: 40. name: "Jun Park" ring: 4. ring_color: #fff}
                    Avatar{plate: 40. name: "Ada Sorensen" shape: Rounded radius: 8.}
                    Avatar{plate: 40. name: "Tomas Ruiz" shape: Square}
                }
            });
            View::script_from_value(vm, value)
        });

        let size = dvec2(400.0, 120.0);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, size);
        let mut draw_list = DrawList2d::new(&mut cx);
        {
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_overlay());
            let walk = row.walk;
            row.draw_walk_all(&mut cx2d, &mut Scope::empty(), walk);
            cx2d.end_pass_sized_turtle();
            draw_list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }

        // A disc 40 across is cut at 20, a ring 4 wide leaves 32 and cuts
        // at 16, a rounded plate is cut at the radius it was written with
        // and a square one is not cut at all.
        for (index, want) in [(0, 10.0), (1, 8.0), (2, 4.0), (3, 0.0)] {
            let child = row.children[index].1.clone();
            let avatar = child.borrow::<Avatar>().expect("an avatar");
            assert_eq!(
                avatar.picture.draw_bg.border_radius, want,
                "plate {index} cut its face at the wrong radius"
            );
        }
    }

    /// The dot is placed against the plate wherever the row puts the plate.
    /// Drawn straight into a row that centres its children, the dot's
    /// absolute walk was centred with them: a bottom corner's dot fell below
    /// the plate and out of the row's clip, and a top corner's sat at the
    /// plate's middle. The row here is taller than the plates and centres
    /// them, which is the row that did it.
    #[test]
    fn the_presence_dot_keeps_its_corner_in_a_row_that_centres() {
        use crate::makepad_draw::cx_draw::CxDraw;
        use crate::makepad_script::script;

        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.with_vm(crate::script_mod);
        let mut row = cx.with_vm(|vm| {
            let value = vm.eval(script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{
                    width: 300 height: 120
                    flow: Right
                    align: Align{x: 0. y: 0.5}
                    Avatar{plate: 40. name: "Mira Okafor" presence: Online presence_corner: BadgeCorner.BottomRight}
                    Avatar{plate: 40. name: "Jun Park" shape: Square presence: Busy presence_corner: BadgeCorner.TopLeft}
                }
            });
            View::script_from_value(vm, value)
        });

        let size = dvec2(300.0, 120.0);
        let pass = DrawPass::new(&mut cx);
        pass.set_size(&mut cx, size);
        let mut draw_list = DrawList2d::new(&mut cx);
        {
            let event = DrawEvent::default();
            let mut draw = CxDraw::new(&mut cx, &event);
            let mut cx2d = Cx2d::new(&mut draw);
            cx2d.begin_pass(&pass, None);
            draw_list.begin_always(&mut cx2d);
            cx2d.begin_root_turtle(size, Layout::flow_overlay());
            let walk = row.walk;
            row.draw_walk_all(&mut cx2d, &mut Scope::empty(), walk);
            cx2d.end_pass_sized_turtle();
            draw_list.end(&mut cx2d);
            cx2d.end_pass(&pass);
        }

        for (index, corner) in [(0, BadgeCorner::BottomRight), (1, BadgeCorner::TopLeft)] {
            let child = row.children[index].1.clone();
            let avatar = child.borrow::<Avatar>().expect("an avatar");
            let plate = child.area().rect(&cx);
            let dot = avatar.presence_dot.area().rect(&cx);
            assert_eq!(plate.size, dvec2(40.0, 40.0), "plate {index}");
            assert!((plate.pos.y - 40.0).abs() < 1e-3, "the row centres plate {index}");
            let want = plate.pos + presence_offset(corner, avatar.shape, plate.size, dot.size.x);
            assert!(
                (dot.pos - want).length() < 1e-3,
                "the dot of plate {index} is at {:?}, not {:?} on its {:?} corner",
                dot.pos,
                want,
                corner
            );
        }
    }
}
