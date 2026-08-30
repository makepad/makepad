//! Page engraving: turns the semantic score into one retained paint page.
//!
//! Everything here is in staff spaces, page-local, y down. The vertical
//! placement of a note is diatonic — a staff step is half a staff space — and
//! all glyph metrics (notehead width, stem attachment, ledger extension, beam
//! thickness) come from the loaded SMuFL font rather than from constants.

use crate::document::{
    pitch_to_midi, semantic_for_measure, semantic_for_note, DocumentError, SemanticElement,
    SemanticKind, DECORATION_TAG, PAGE_HEIGHT_SP, PAGE_WIDTH_SP,
};
use crate::font::{music_font, Engraving, MusicFont};
use crate::spacing::{MeasurePlacement, PagePlacement};
use makepad_score::{
    model::{
        EventKind, KeySignature, Measure, Meter, Notehead, Pitch, Rational, Score,
        ScoreTime, StaffId, TimedEvent, VoiceId,
    },
    symbol::{
        Accidental, Direction, FlagDuration, NoteheadDuration, NoteheadShape,
        Placement, Symbol,
    },
};
use makepad_score_layout::LayoutStyle;
use makepad_score_render::{
    Beam, GlyphItem, Ink, InkRole, LinearRgba, MusicFontRef, PageId, PaintItem, PaintKind,
    PaintList, Point, Primitive, Rect, RuleKind, SemanticId, SmuflGlyph, TextDirection,
    TextFontRef, TextRun,
};
use std::sync::Arc;

/// One em is four staff spaces in every SMuFL font.
const EM_SIZE: f64 = 4.0;
/// Distance from the top staff line of the upper staff to that of the lower.
const STAFF_GAP: f64 = 14.0;
/// Top staff line of the upper staff to bottom staff line of the lower.
pub(crate) const STAFF_SPAN: f64 = STAFF_GAP + 4.0;
pub(crate) const MARGIN_LEFT: f64 = 17.0;
pub(crate) const MARGIN_RIGHT: f64 = 14.0;
/// Shortest stem, in staff spaces, measured from the notehead centre.
const STEM_LENGTH: f64 = 3.5;
const BEAM_MIN_STEM: f64 = 3.0;

/// Which staff of the grand staff a note is being drawn on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StaffRole {
    Upper,
    Lower,
}

/// A five-line staff placed on the page, with its clef.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StaffFrame {
    /// Page y of the top staff line.
    pub(crate) top: f64,
    clef: &'static str,
    /// Page y of the line the clef's origin sits on.
    clef_line: f64,
    /// Diatonic index (octave * 7 + step) of the pitch on the middle line.
    pub(crate) middle_diatonic: i32,
    /// Staff steps to shift a key signature by, relative to a treble staff.
    key_shift: i8,
}

/// The grand staff of one system, given the page y of its top staff line.
pub(crate) fn staff_frames(top: f64) -> [StaffFrame; 2] {
    [StaffFrame::treble(top), StaffFrame::bass(top + STAFF_GAP)]
}

impl StaffFrame {
    fn treble(top: f64) -> Self {
        Self {
            top,
            clef: "gClef",
            clef_line: top + 3.0,
            // B4 sits on the middle line of a treble staff.
            middle_diatonic: 4 * 7 + 6,
            key_shift: 0,
        }
    }

    fn bass(top: f64) -> Self {
        Self {
            top,
            clef: "fClef",
            clef_line: top + 1.0,
            // D3 sits on the middle line of a bass staff.
            middle_diatonic: 3 * 7 + 1,
            key_shift: -2,
        }
    }

    fn middle(self) -> f64 {
        self.top + 2.0
    }

    fn bottom(self) -> f64 {
        self.top + 4.0
    }

    /// Page y of a diatonic pitch position on this staff.
    pub(crate) fn y_of(self, diatonic: i32) -> f64 {
        self.middle() - f64::from(diatonic - self.middle_diatonic) * 0.5
    }
}

/// The written form of one duration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NoteValue {
    /// 0 = whole, 1 = half, 2 = quarter, 3 = eighth, ...
    power: u8,
    pub(crate) dots: u8,
}

impl NoteValue {
    fn flags(self) -> u8 {
        self.power.saturating_sub(2)
    }

    fn notehead(self, notehead: &Notehead) -> Symbol {
        let duration = match self.power {
            0 => NoteheadDuration::Whole,
            1 => NoteheadDuration::Half,
            _ => NoteheadDuration::Black,
        };
        let shape = match notehead {
            Notehead::X => NoteheadShape::X,
            Notehead::Diamond => NoteheadShape::Diamond,
            Notehead::Triangle => NoteheadShape::TriangleUp,
            Notehead::Slash => NoteheadShape::Slash,
            _ => NoteheadShape::Normal,
        };
        Symbol::Notehead { duration, shape }
    }

    fn has_stem(self) -> bool {
        self.power >= 1
    }
}

/// One notehead inside a chord column.
#[derive(Clone, Debug)]
pub(crate) struct HeadLayout {
    note: makepad_score::model::NoteId,
    midi: u8,
    diatonic: i32,
    y: f64,
    pub(crate) glyph: String,
    pub(crate) accidental: Option<String>,
    /// Seconds are offset to the far side of the stem.
    pub(crate) shifted: bool,
}

/// One rhythmic column: a chord (or single note) of one voice.
#[derive(Clone, Debug)]
pub(crate) struct Column {
    event: makepad_score::model::EventId,
    measure: makepad_score::model::MeasureId,
    voice: VoiceId,
    staff: StaffId,
    /// Onset in whole notes from the start of the score.
    onset: f64,
    /// The same onset, exactly: the key columns are merged on.
    pub(crate) time: ScoreTime,
    /// Page x of the unshifted notehead's left edge, filled in from the
    /// spacing plan once the system's springs are solved.
    x: f64,
    pub(crate) heads: Vec<HeadLayout>,
    pub(crate) value: NoteValue,
    pub(crate) stem_up: bool,
    articulations: Vec<String>,
}

impl Column {
    pub(crate) fn top_y(&self) -> f64 {
        self.heads
            .iter()
            .map(|head| head.y)
            .fold(f64::INFINITY, f64::min)
    }

    pub(crate) fn bottom_y(&self) -> f64 {
        self.heads
            .iter()
            .map(|head| head.y)
            .fold(f64::NEG_INFINITY, f64::max)
    }

    /// The notehead the stem starts from.
    fn stem_origin_y(&self) -> f64 {
        if self.stem_up {
            self.bottom_y()
        } else {
            self.top_y()
        }
    }

    /// The notehead the stem has to clear.
    fn stem_far_y(&self) -> f64 {
        if self.stem_up {
            self.top_y()
        } else {
            self.bottom_y()
        }
    }
}

/// Accumulates paint items and hands out decoration IDs.
struct PageBuilder<'a> {
    font: &'static MusicFont,
    engraving: Engraving,
    items: Vec<PaintItem>,
    elements: Vec<SemanticElement>,
    decoration: u64,
    page_index: usize,
    score: &'a Score,
}

impl PageBuilder<'_> {
    fn next_decor(&mut self) -> SemanticId {
        self.decoration = self.decoration.saturating_add(1);
        SemanticId(self.decoration)
    }

    fn glyph(&mut self, id: SemanticId, name: &str, origin: Point, ink: Ink, z: i16) -> bool {
        if !self.font.has(name) {
            return false;
        }
        let bbox = self.font.bbox(name);
        // The font box is y-up around the origin; the page is y-down.
        let bounds = Rect::new(
            Point::new(origin.x + bbox.min_x, origin.y - bbox.max_y),
            Point::new(origin.x + bbox.max_x, origin.y - bbox.min_y),
        );
        self.items.push(PaintItem {
            id,
            bounds,
            z,
            ink,
            kind: PaintKind::Glyph(GlyphItem {
                font: MusicFontRef(0),
                glyph: SmuflGlyph::new(name.to_string()),
                origin,
                em_size: EM_SIZE,
            }),
        });
        true
    }

    fn decor_glyph(&mut self, name: &str, origin: Point) {
        let id = self.next_decor();
        self.glyph(id, name, origin, Ink::role(InkRole::Primary), 2);
    }

    fn rule(&mut self, rect: Rect, kind: RuleKind, ink: Ink, z: i16) {
        let id = self.next_decor();
        self.items
            .push(PaintItem::primitive(id, z, ink, Primitive::Rule {
                rect,
                kind,
                staff_group: None,
            }));
    }

    fn beam(&mut self, start: Point, end: Point, thickness: f64) {
        let id = self.next_decor();
        self.items.push(PaintItem::primitive(
            id,
            2,
            Ink::role(InkRole::Primary),
            Primitive::Beam(Beam {
                start,
                end,
                thickness,
            }),
        ));
    }

    /// One centred run. The width is *measured*, not estimated: it decides
    /// both the item's bounds and where the run starts, so a guess is a run
    /// drawn off centre — and, for the title, off the page.
    fn text(&mut self, text: impl Into<Arc<str>>, origin: Point, size: f64, z: i16) {
        let id = self.next_decor();
        let text = text.into();
        let width = crate::title::text_width_sp(&text, size).max(size * 0.5);
        self.items.push(PaintItem {
            id,
            bounds: Rect::from_xywh(
                origin.x - width * 0.5,
                origin.y,
                width,
                crate::title::line_box_sp(size),
            ),
            z,
            ink: Ink::role(InkRole::Secondary),
            kind: PaintKind::Text(TextRun {
                font: TextFontRef(0),
                text,
                origin: Point::new(origin.x - width * 0.5, origin.y),
                size,
                letter_spacing: 0.0,
                direction: TextDirection::Auto,
                language: None,
            }),
        });
    }
}

/// Engraves one page of the score from its solved placement.
///
/// Every x here comes from the spacing plan: the page tells this function
/// where each system, measure and onset column landed once the spring-and-rod
/// chain was solved to the system width. Nothing is divided up locally.
pub fn make_page(
    score: &Score,
    page: &PagePlacement,
    page_index: usize,
    revision: u64,
) -> Result<(PaintList, Vec<SemanticElement>), DocumentError> {
    let font = music_font();
    let style = LayoutStyle::default();
    let mut builder = PageBuilder {
        font,
        engraving: font.engraving(),
        items: Vec::new(),
        elements: Vec::new(),
        decoration: DECORATION_TAG | ((page_index as u64 + 1) << 40),
        page_index,
        score,
    };

    if page_index == 0 {
        // The instrumentation is the score's own part list, not a guess.
        let parts: Vec<&str> = score
            .parts
            .values()
            .map(|part| part.name.as_str())
            .filter(|name| !name.trim().is_empty())
            .collect();
        let subtitle = if parts.is_empty() {
            String::new()
        } else {
            format!("for {}", parts.join(", "))
        };
        // Fitted, not fixed: a long title shrinks and then wraps rather than
        // running off both edges of the page.
        let block = crate::title::title_block(&score.title, &subtitle);
        for line in block.lines() {
            builder.text(
                line.text.clone(),
                Point::new(PAGE_WIDTH_SP * 0.5, line.top),
                line.size,
                1,
            );
        }
    }

    let measures = crate::spacing::ordered_measures(score);
    for (index, system) in page.systems.iter().enumerate() {
        let Some(first) = system.measures.first() else {
            continue;
        };
        let Some(&measure) = measures.get(first.index) else {
            continue;
        };
        let staves = staff_frames(system.top);
        let right = system
            .measures
            .last()
            .map(|last| last.right)
            .unwrap_or(system.right);
        draw_system_frame(&mut builder, &staves, index, right);

        let key = score
            .maps
            .key_at(measure.start, None, None)
            .cloned()
            .unwrap_or(KeySignature::C_MAJOR);
        let meter = score.maps.meter_at(measure.start, None, None).cloned();
        draw_system_prefix(
            &mut builder,
            &staves,
            &key,
            meter.as_ref().filter(|_| system.show_meter),
            &style,
        );

        for placement in &system.measures {
            let Some(&measure) = measures.get(placement.index) else {
                continue;
            };
            draw_measure(
                &mut builder,
                &staves,
                measure,
                &key,
                placement,
                placement.index == first.index,
                placement.index + 1 == measures.len(),
            );
        }
    }

    builder.text(
        (page_index + 1).to_string(),
        Point::new(PAGE_WIDTH_SP * 0.5, PAGE_HEIGHT_SP - 7.0),
        1.8,
        1,
    );

    let list = PaintList::new(
        PageId(page_index as u32),
        revision,
        Point::new(PAGE_WIDTH_SP, PAGE_HEIGHT_SP),
        builder.items,
    )
    .map_err(|error| DocumentError::Native(error.to_string()))?;
    Ok((list, builder.elements))
}

fn draw_system_frame(
    builder: &mut PageBuilder<'_>,
    staves: &[StaffFrame; 2],
    system: usize,
    right: f64,
) {
    let staff_ink = Ink::role(InkRole::Staff);
    let thickness = builder.engraving.staff_line_thickness;
    let left = MARGIN_LEFT;
    for (index, staff) in staves.iter().enumerate() {
        let group = system as u32 * 2 + index as u32 + 1;
        for line in 0..5 {
            let rect = Rect::from_xywh(left, staff.top + line as f64, right - left, thickness);
            let id = builder.next_decor();
            builder.items.push(PaintItem::primitive(
                id,
                0,
                staff_ink,
                Primitive::Rule {
                    rect,
                    kind: RuleKind::Staff,
                    staff_group: Some(group),
                },
            ));
        }
    }
    // The brace-substitute bracket plus the left-hand system barline.
    let id = builder.next_decor();
    builder.items.push(PaintItem::primitive(
        id,
        1,
        Ink::role(InkRole::Primary),
        Primitive::Bracket {
            x: left - 1.3,
            top: staves[0].top,
            bottom: staves[1].bottom(),
            thickness: builder.engraving.bracket_thickness * 0.5,
            hook: 1.0,
        },
    ));
    let thin = builder.engraving.thin_barline_thickness;
    builder.rule(
        Rect::from_xywh(left, staves[0].top, thin, staves[1].bottom() - staves[0].top),
        RuleKind::BarlineThin,
        Ink::role(InkRole::Primary),
        1,
    );
}

/// The geometry of a system prefix: clef, key signature and — on the score's
/// first system only — the time signature.
///
/// The planner and the engraver share this so that "where music starts" is
/// one number computed once. All distances are style constants, not
/// hand-tuned literals.
struct Prefix {
    clef_x: f64,
    accidental: Option<String>,
    accidental_x: Vec<f64>,
    meter: Option<MeterPrefix>,
    /// Distance from the left margin to the end of the prefix.
    width: f64,
}

struct MeterPrefix {
    numerator: Vec<String>,
    denominator: Vec<String>,
    x: f64,
    span: f64,
}

fn prefix_layout(
    font: &MusicFont,
    key: &KeySignature,
    meter: Option<&Meter>,
    style: &LayoutStyle,
) -> Prefix {
    let distance = &style.distance;
    let mut cursor = distance.clef_left_margin.0;
    let clef_x = cursor;
    let clef = font.advance("gClef").max(font.advance("fClef"));
    cursor += clef + distance.clef_to_key.0;

    let steps = key_signature_steps(key.fifths);
    let mut accidental = None;
    let mut accidental_x = Vec::new();
    if !steps.is_empty() {
        let glyph = if key.fifths > 0 {
            Symbol::Accidental(Accidental::Sharp)
        } else {
            Symbol::Accidental(Accidental::Flat)
        }
        .canonical_name()
        .to_string();
        let advance = font.advance(&glyph).max(0.7) + distance.accidental_column.0;
        for index in 0..steps.len() {
            accidental_x.push(cursor + index as f64 * advance);
        }
        cursor += advance * steps.len() as f64;
        accidental = Some(glyph);
    }

    let meter = match meter {
        Some(Meter::Measured { groups, unit }) => {
            cursor += distance.key_to_time.0;
            let beats: u32 = groups.iter().map(|group| u32::from(*group)).sum();
            let numerator: Vec<String> = digits_of(beats)
                .iter()
                .map(|digit| digit.canonical_name().to_string())
                .collect();
            let denominator: Vec<String> = digits_of(u32::from(*unit))
                .iter()
                .map(|digit| digit.canonical_name().to_string())
                .collect();
            let run = |names: &[String]| -> f64 { names.iter().map(|name| font.advance(name)).sum() };
            let span = run(&numerator).max(run(&denominator));
            let at = cursor;
            cursor += span;
            Some(MeterPrefix {
                numerator,
                denominator,
                x: at,
                span,
            })
        }
        _ => None,
    };

    Prefix {
        clef_x,
        accidental,
        accidental_x,
        meter,
        width: cursor,
    }
}

/// Distance from the left margin to where a system's music may start.
pub(crate) fn prefix_width(
    font: &MusicFont,
    key: &KeySignature,
    meter: Option<&Meter>,
    style: &LayoutStyle,
) -> f64 {
    prefix_layout(font, key, meter, style).width
}

/// Clef, key signature and (only where it belongs) time signature.
fn draw_system_prefix(
    builder: &mut PageBuilder<'_>,
    staves: &[StaffFrame; 2],
    key: &KeySignature,
    meter: Option<&Meter>,
    style: &LayoutStyle,
) {
    let prefix = prefix_layout(builder.font, key, meter, style);
    let steps = key_signature_steps(key.fifths);
    for staff in staves {
        builder.decor_glyph(staff.clef, Point::new(MARGIN_LEFT + prefix.clef_x, staff.clef_line));
        if let Some(glyph) = &prefix.accidental {
            let glyph = glyph.clone();
            for (step, x) in steps.iter().zip(&prefix.accidental_x) {
                let y = staff.middle() - f64::from(*step + staff.key_shift) * 0.5;
                builder.decor_glyph(&glyph, Point::new(MARGIN_LEFT + x, y));
            }
        }
        if let Some(meter) = &prefix.meter {
            for (digits, line) in [(&meter.numerator, 1.0), (&meter.denominator, 3.0)] {
                let run: f64 = digits.iter().map(|name| builder.font.advance(name)).sum();
                let mut x = MARGIN_LEFT + meter.x + (meter.span - run) * 0.5;
                for name in digits {
                    let name = name.clone();
                    builder.decor_glyph(&name, Point::new(x, staff.top + line));
                    x += builder.font.advance(&name);
                }
            }
        }
    }
}

fn digits_of(value: u32) -> Vec<Symbol> {
    use makepad_score::symbol::Digit;
    let digit = |value: u32| match value {
        0 => Digit::Zero,
        1 => Digit::One,
        2 => Digit::Two,
        3 => Digit::Three,
        4 => Digit::Four,
        5 => Digit::Five,
        6 => Digit::Six,
        7 => Digit::Seven,
        8 => Digit::Eight,
        _ => Digit::Nine,
    };
    value
        .to_string()
        .chars()
        .filter_map(|character| character.to_digit(10))
        .map(|value| Symbol::TimeSignatureDigit(digit(value)))
        .collect()
}

/// Diatonic offsets from the middle line, in staff steps, for the accidentals
/// of a key signature on a treble staff. A bass staff is two steps lower.
fn key_signature_steps(fifths: i8) -> Vec<i8> {
    const SHARPS: [i8; 7] = [4, 1, 5, 2, -1, 3, 0];
    const FLATS: [i8; 7] = [0, 3, -1, 2, -2, 1, -3];
    let count = fifths.unsigned_abs().min(7) as usize;
    if fifths > 0 {
        SHARPS[..count].to_vec()
    } else {
        FLATS[..count].to_vec()
    }
}

fn draw_measure(
    builder: &mut PageBuilder<'_>,
    staves: &[StaffFrame; 2],
    measure: &Measure,
    key: &KeySignature,
    placement: &MeasurePlacement,
    first_in_system: bool,
    last_of_score: bool,
) {
    let (x0, x1) = (placement.left, placement.right);
    let measure_semantic = semantic_for_measure(measure.id);
    let bounds = Rect::from_xywh(
        x0,
        staves[0].top - 3.0,
        x1 - x0,
        staves[1].bottom() - staves[0].top + 6.0,
    );
    builder.items.push(PaintItem::primitive(
        measure_semantic,
        -2,
        Ink::color(InkRole::Selection, LinearRgba::new(0.0, 0.0, 0.0, 0.0)),
        Primitive::Rule {
            rect: bounds,
            kind: RuleKind::Staff,
            staff_group: None,
        },
    ));
    let score = builder.score;
    if let (Some(voice), Some(staff)) = (
        score.voices.values().next().map(|voice| voice.id),
        score.staves.values().next().map(|staff| staff.id),
    ) {
        builder.elements.push(SemanticElement {
            semantic: measure_semantic,
            kind: SemanticKind::Measure,
            note: None,
            event: None,
            measure: measure.id,
            staff,
            voice,
            page: builder.page_index,
            bounds,
            midi: None,
        });
    }
    if first_in_system {
        builder.text(
            measure.label.clone(),
            Point::new(x0 + 0.9, staves[0].top - 1.4),
            1.5,
            1,
        );
    }

    // One barline through the whole grand staff reads as one system.
    let thin = builder.engraving.thin_barline_thickness;
    let height = staves[1].bottom() - staves[0].top;
    if last_of_score {
        let thick = builder.engraving.thick_barline_thickness;
        let separation = 0.4;
        builder.rule(
            Rect::from_xywh(x1 - thick - separation - thin, staves[0].top, thin, height),
            RuleKind::BarlineThin,
            Ink::role(InkRole::Primary),
            1,
        );
        builder.rule(
            Rect::from_xywh(x1 - thick, staves[0].top, thick, height),
            RuleKind::BarlineThick,
            Ink::role(InkRole::Primary),
            1,
        );
    } else {
        builder.rule(
            Rect::from_xywh(x1 - thin, staves[0].top, thin, height),
            RuleKind::BarlineThin,
            Ink::role(InkRole::Primary),
            1,
        );
    }

    let staves_columns = measure_staff_columns(builder.font, builder.score, measure, key, staves);
    for (staff_frame, voices) in staves.iter().zip(&staves_columns) {
        if voices.is_empty() {
            draw_measure_rest(builder, *staff_frame, (x0 + x1) * 0.5);
            continue;
        }
        for columns in voices {
            // Every column takes its x from the solved system chain; nothing
            // is spread out locally.
            let mut columns = columns.clone();
            for column in &mut columns {
                column.x = placement.x_of(column.time);
            }
            draw_columns(builder, *staff_frame, measure, &columns);
        }
    }
}

/// One measure's columns, per staff of the grand staff and then per active
/// voice. An empty voice list means that staff rests for the whole measure.
///
/// Both the spacing pass (which measures the ink) and the engraving pass
/// (which draws it) go through here, so the rods the solver sees describe the
/// glyphs that actually get drawn.
pub(crate) fn measure_staff_columns(
    font: &'static MusicFont,
    score: &Score,
    measure: &Measure,
    key: &KeySignature,
    staves: &[StaffFrame; 2],
) -> [Vec<Vec<Column>>; 2] {
    let mut out = [Vec::new(), Vec::new()];
    let Some(part) = score.parts.values().next() else {
        return out;
    };
    let measure_end = measure
        .start
        .checked_add(measure.extent)
        .unwrap_or(measure.start);
    let upper = part.staves.first().copied();
    for (slot, (role, staff_frame)) in
        [(StaffRole::Upper, staves[0]), (StaffRole::Lower, staves[1])]
            .into_iter()
            .enumerate()
    {
        let staff_ids: Vec<StaffId> = part
            .staves
            .iter()
            .copied()
            .filter(|id| {
                let is_upper = Some(*id) == upper;
                (role == StaffRole::Upper) == is_upper
            })
            .collect();
        let active: Vec<&makepad_score::model::Voice> = score
            .voices
            .values()
            .filter(|voice| staff_ids.contains(&voice.staff))
            .filter(|voice| {
                voice
                    .events
                    .iter()
                    .any(|event| in_measure(event, measure.start, measure_end))
            })
            .collect();
        let multi_voice = active.len() > 1;
        for (voice_index, voice) in active.iter().enumerate() {
            out[slot].push(build_columns(
                font,
                staff_frame,
                voice,
                measure,
                measure_end,
                key,
                if multi_voice {
                    Some(voice_index == 0)
                } else {
                    None
                },
            ));
        }
    }
    out
}

fn in_measure(event: &TimedEvent, start: ScoreTime, end: ScoreTime) -> bool {
    matches!(event.kind, EventKind::Chord(_))
        && !event.chord_notes().is_empty()
        && event.onset >= start
        && event.onset < end
}

fn draw_measure_rest(builder: &mut PageBuilder<'_>, staff: StaffFrame, center_x: f64) {
    let name = Symbol::Rest(makepad_score::symbol::RestDuration::Whole)
        .canonical_name()
        .to_string();
    let width = builder.font.bbox(&name).width();
    // A whole-measure rest hangs from the second line from the top.
    builder.decor_glyph(&name, Point::new(center_x - width * 0.5, staff.top + 1.0));
}

/// One voice's columns for one measure, everything but their x: pitches,
/// written note values, stem directions, accidentals and second-interval
/// head shifts. The x arrives later, from the solved spacing chain.
fn build_columns(
    font: &'static MusicFont,
    staff: StaffFrame,
    voice: &makepad_score::model::Voice,
    measure: &Measure,
    measure_end: ScoreTime,
    key: &KeySignature,
    forced_stem_up: Option<bool>,
) -> Vec<Column> {
    let mut state = KeyState::new(key);
    let mut columns = Vec::new();

    let events: Vec<&TimedEvent> = voice
        .events
        .iter()
        .filter(|event| in_measure(event, measure.start, measure_end))
        .collect();
    for (index, event) in events.iter().enumerate() {
        // A performance import carries sounding lengths, not written ones: a
        // staccato sixteenth is stored as a thirty-second. Writing each note up
        // to the next onset recovers the notated rhythm, and cannot overstate a
        // note, because this engraver has no rests to put in the gap.
        let remaining = measure_end
            .checked_sub(event.onset)
            .map(|time| rational_f64(time.0))
            .unwrap_or(0.0);
        let sounding = event
            .duration
            .map(|duration| rational_f64(duration.0))
            .unwrap_or(0.0);
        let written = match events.get(index + 1) {
            Some(next) => next
                .onset
                .checked_sub(event.onset)
                .map(|time| rational_f64(time.0))
                .unwrap_or(sounding),
            // The last note of the measure keeps its own length: there is no
            // following onset to measure against.
            None => sounding.min(remaining),
        };
        let value = note_value_of(if written > 0.0 { written } else { sounding });
        let mut heads: Vec<HeadLayout> = Vec::new();
        for note in event.chord_notes() {
            let Some(pitch) = note.written_pitch else {
                continue;
            };
            let diatonic = diatonic_index(pitch);
            let accidental = state.accidental_for(pitch, diatonic);
            heads.push(HeadLayout {
                note: note.id,
                midi: pitch_to_midi(pitch),
                diatonic,
                y: staff.y_of(diatonic),
                glyph: value.notehead(&note.notehead).canonical_name().to_string(),
                accidental,
                shifted: false,
            });
        }
        if heads.is_empty() {
            continue;
        }
        heads.sort_by(|a, b| a.diatonic.cmp(&b.diatonic));
        heads.dedup_by(|a, b| a.diatonic == b.diatonic);

        let average = heads.iter().map(|head| head.diatonic).sum::<i32>() as f64
            / heads.len() as f64;
        let stem_up = forced_stem_up
            .unwrap_or_else(|| average < f64::from(staff.middle_diatonic) + 0.01);
        // Seconds cannot share a side of the stem.
        let mut previous: Option<i32> = None;
        let mut previous_shifted = false;
        let order: Vec<usize> = if stem_up {
            (0..heads.len()).collect()
        } else {
            (0..heads.len()).rev().collect()
        };
        for index in order {
            let diatonic = heads[index].diatonic;
            let shifted = previous
                .map(|previous_diatonic| (diatonic - previous_diatonic).abs() == 1)
                .unwrap_or(false)
                && !previous_shifted;
            heads[index].shifted = shifted;
            previous = Some(diatonic);
            previous_shifted = shifted;
        }

        let articulations = event
            .articulations
            .iter()
            .filter_map(|placed| {
                let placement = if stem_up {
                    Placement::Below
                } else {
                    Placement::Above
                };
                let symbol = Symbol::Articulation {
                    articulation: placed.kind,
                    placement,
                };
                let name = symbol.canonical_name().to_string();
                font.has(&name).then_some(name)
            })
            .collect();

        columns.push(Column {
            event: event.id,
            measure: measure.id,
            voice: voice.id,
            onset: rational_f64(event.onset.0),
            time: event.onset,
            staff: voice.staff,
            x: 0.0,
            heads,
            value,
            stem_up,
            articulations,
        });
    }
    columns.sort_by(|a, b| a.onset.total_cmp(&b.onset));
    columns
}

/// Tracks which accidentals are already sounding in the current measure.
struct KeyState {
    signature: [i32; 7],
    current: std::collections::BTreeMap<i32, i32>,
}

impl KeyState {
    fn new(key: &KeySignature) -> Self {
        let mut signature = [0_i32; 7];
        let sharp_order = [3_usize, 0, 4, 1, 5, 2, 6];
        let flat_order = [6_usize, 2, 5, 1, 4, 0, 3];
        let count = key.fifths.unsigned_abs().min(7) as usize;
        if key.fifths > 0 {
            for step in &sharp_order[..count] {
                signature[*step] = 1;
            }
        } else {
            for step in &flat_order[..count] {
                signature[*step] = -1;
            }
        }
        Self {
            signature,
            current: std::collections::BTreeMap::new(),
        }
    }

    fn accidental_for(&mut self, pitch: Pitch, diatonic: i32) -> Option<String> {
        let alter = (rational_f64(pitch.alter.0)).round() as i32;
        let step = diatonic.rem_euclid(7) as usize;
        let sounding = self
            .current
            .get(&diatonic)
            .copied()
            .unwrap_or(self.signature[step]);
        if alter == sounding {
            return None;
        }
        self.current.insert(diatonic, alter);
        let accidental = match alter {
            -3 => Accidental::TripleFlat,
            -2 => Accidental::DoubleFlat,
            -1 => Accidental::Flat,
            0 => Accidental::Natural,
            1 => Accidental::Sharp,
            2 => Accidental::DoubleSharp,
            _ => Accidental::TripleSharp,
        };
        Some(Symbol::Accidental(accidental).canonical_name().to_string())
    }
}

fn draw_columns(
    builder: &mut PageBuilder<'_>,
    staff: StaffFrame,
    measure: &Measure,
    columns: &[Column],
) {
    let groups = beam_groups(builder.score, measure, columns);
    let mut beamed = vec![false; columns.len()];
    for group in &groups {
        for index in group {
            beamed[*index] = true;
        }
    }
    for (index, column) in columns.iter().enumerate() {
        draw_column_heads(builder, staff, column);
        if !column.value.has_stem() {
            continue;
        }
        if beamed[index] {
            continue;
        }
        let tip = unbeamed_stem_tip(staff, column);
        draw_stem(builder, column, tip);
        let flags = column.value.flags();
        if flags > 0 {
            draw_flag(builder, column, tip, flags);
        }
    }
    for group in &groups {
        draw_beam_group(builder, staff, columns, group);
    }
}

fn draw_column_heads(builder: &mut PageBuilder<'_>, staff: StaffFrame, column: &Column) {
    let head_width = builder.font.bbox("noteheadBlack").width();
    let extension = builder.engraving.leger_line_extension;
    let ledger_thickness = builder.engraving.leger_line_thickness;
    let mut ledgers: Vec<(f64, f64, f64)> = Vec::new();

    for head in &column.heads {
        let width = builder.font.bbox(&head.glyph).width().max(0.1);
        let shift = if head.shifted {
            if column.stem_up {
                head_width
            } else {
                -head_width
            }
        } else {
            0.0
        };
        let x = column.x + shift;
        let semantic = semantic_for_note(head.note);
        let bounds = Rect::new(
            Point::new(x, head.y - 0.5),
            Point::new(x + width, head.y + 0.5),
        );
        let drawn = builder.glyph(
            semantic,
            &head.glyph,
            Point::new(x, head.y),
            Ink::role(InkRole::Primary),
            2,
        );
        if drawn {
            builder.elements.push(SemanticElement {
                semantic,
                kind: SemanticKind::Note,
                note: Some(head.note),
                event: Some(column.event),
                measure: column.measure,
                staff: column.staff,
                voice: column.voice,
                page: builder.page_index,
                bounds,
                midi: Some(head.midi),
            });
        }
        if let Some(accidental) = &head.accidental {
            let accidental_width = builder.font.advance(accidental).max(0.6);
            builder.decor_glyph(
                accidental,
                Point::new(x - accidental_width - 0.22, head.y),
            );
        }
        // Ledger lines: one for every staff position outside the five lines.
        let mut line = staff.top - 1.0;
        while line >= head.y - 0.01 {
            ledgers.push((line, x, width));
            line -= 1.0;
        }
        let mut line = staff.bottom() + 1.0;
        while line <= head.y + 0.01 {
            ledgers.push((line, x, width));
            line += 1.0;
        }
        if column.value.dots > 0 {
            let mut dot_y = head.y;
            if (head.y - staff.top).rem_euclid(1.0).abs() < 0.01 {
                dot_y -= 0.5;
            }
            let dot_width = builder.font.advance("augmentationDot").max(0.3);
            for dot in 0..column.value.dots {
                builder.decor_glyph(
                    "augmentationDot",
                    Point::new(
                        x + width + 0.32 + f64::from(dot) * dot_width * 1.1,
                        dot_y,
                    ),
                );
            }
        }
    }

    ledgers.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    ledgers.dedup_by(|a, b| (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01);
    for (y, x, width) in ledgers {
        builder.rule(
            Rect::from_xywh(
                x - extension,
                y - ledger_thickness * 0.5,
                width + extension * 2.0,
                ledger_thickness,
            ),
            RuleKind::Ledger,
            Ink::role(InkRole::Primary),
            1,
        );
    }

    for (index, articulation) in column.articulations.iter().enumerate() {
        let bbox = builder.font.bbox(articulation);
        let width = bbox.width().max(0.2);
        let x = column.x + head_width * 0.5 - width * 0.5;
        let y = if column.stem_up {
            column.bottom_y() + 0.9 + index as f64 * 0.6
        } else {
            column.top_y() - 0.9 - index as f64 * 0.6
        };
        builder.decor_glyph(articulation, Point::new(x, y));
    }
}

fn unbeamed_stem_tip(staff: StaffFrame, column: &Column) -> f64 {
    let extra = f64::from(column.value.flags().saturating_sub(1)) * 0.5;
    if column.stem_up {
        (column.stem_far_y() - STEM_LENGTH - extra).min(staff.middle())
    } else {
        (column.stem_far_y() + STEM_LENGTH + extra).max(staff.middle())
    }
}

/// Page x of the centre of a column's stem, from the font's own anchor.
fn stem_x(builder: &PageBuilder<'_>, column: &Column) -> f64 {
    let thickness = builder.engraving.stem_thickness;
    let glyph = column
        .heads
        .first()
        .map(|head| head.glyph.clone())
        .unwrap_or_else(|| "noteheadBlack".to_string());
    if column.stem_up {
        column.x + builder.font.stem_up_se(&glyph).0 - thickness * 0.5
    } else {
        column.x + builder.font.stem_down_nw(&glyph).0 + thickness * 0.5
    }
}

fn draw_stem(builder: &mut PageBuilder<'_>, column: &Column, tip: f64) {
    let thickness = builder.engraving.stem_thickness;
    let glyph = column
        .heads
        .first()
        .map(|head| head.glyph.clone())
        .unwrap_or_else(|| "noteheadBlack".to_string());
    let center = stem_x(builder, column);
    let attach = if column.stem_up {
        column.stem_origin_y() - builder.font.stem_up_se(&glyph).1
    } else {
        column.stem_origin_y() - builder.font.stem_down_nw(&glyph).1
    };
    let (top, bottom) = if tip < attach { (tip, attach) } else { (attach, tip) };
    builder.rule(
        Rect::from_xywh(center - thickness * 0.5, top, thickness, bottom - top),
        RuleKind::Stem,
        Ink::role(InkRole::Primary),
        2,
    );
}

fn draw_flag(builder: &mut PageBuilder<'_>, column: &Column, tip: f64, flags: u8) {
    let duration = match flags {
        1 => FlagDuration::Eighth,
        2 => FlagDuration::Sixteenth,
        3 => FlagDuration::ThirtySecond,
        4 => FlagDuration::SixtyFourth,
        _ => FlagDuration::OneTwentyEighth,
    };
    let direction = if column.stem_up {
        Direction::Up
    } else {
        Direction::Down
    };
    let name = Symbol::Flag {
        duration,
        direction,
    }
    .canonical_name()
    .to_string();
    let thickness = builder.engraving.stem_thickness;
    let x = stem_x(builder, column)
        + if column.stem_up {
            -thickness * 0.5
        } else {
            -thickness * 0.5
        };
    builder.decor_glyph(&name, Point::new(x, tip));
}

/// Splits a measure's columns into beam groups: runs of two or more flagged
/// notes inside one metrical beat.
fn beam_groups(score: &Score, measure: &Measure, columns: &[Column]) -> Vec<Vec<usize>> {
    let beat = beat_length(score, measure);
    let measure_start = rational_f64(measure.start.0);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut current_beat: Option<i64> = None;
    for (index, column) in columns.iter().enumerate() {
        let beat_index = (((column.onset - measure_start) / beat) + 1e-6).floor() as i64;
        let beamable = column.value.flags() > 0;
        // Note: a played rest inside a beat does not break the group, because
        // this engraver does not yet write rests between notes; grouping on the
        // beat alone is what a performance import can honestly support.
        if !beamable || current_beat != Some(beat_index) {
            if current.len() > 1 {
                groups.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
        if beamable {
            current_beat = Some(beat_index);
            current.push(index);
        } else {
            current_beat = None;
        }
    }
    if current.len() > 1 {
        groups.push(current);
    }
    groups
}

/// The beaming unit: a beat, or a dotted beat in a compound meter.
fn beat_length(score: &Score, measure: &Measure) -> f64 {
    let meter = score.maps.meter_at(measure.start, None, None);
    match meter {
        Some(Meter::Measured { groups, unit }) if *unit > 0 => {
            let beats: u32 = groups.iter().map(|group| u32::from(*group)).sum();
            let unit_length = 1.0 / f64::from(*unit);
            if *unit >= 8 && beats % 3 == 0 && beats > 3 {
                unit_length * 3.0
            } else {
                unit_length
            }
        }
        _ => 0.25,
    }
}

fn draw_beam_group(
    builder: &mut PageBuilder<'_>,
    staff: StaffFrame,
    columns: &[Column],
    group: &[usize],
) {
    if group.len() < 2 {
        return;
    }
    // One direction for the whole group. Where a voice has already been given
    // a direction (two voices sharing a staff), the beam must not fight it.
    let forced = columns[group[0]].stem_up;
    let agreed = group
        .iter()
        .all(|index| columns[*index].stem_up == forced);
    let average = group
        .iter()
        .flat_map(|index| columns[*index].heads.iter().map(|head| head.diatonic))
        .sum::<i32>() as f64
        / group
            .iter()
            .map(|index| columns[*index].heads.len())
            .sum::<usize>()
            .max(1) as f64;
    let stem_up = if agreed {
        forced
    } else {
        average < f64::from(staff.middle_diatonic) + 0.01
    };

    let members: Vec<Column> = group
        .iter()
        .map(|index| Column {
            stem_up,
            ..columns[*index].clone()
        })
        .collect();
    let xs: Vec<f64> = members
        .iter()
        .map(|column| stem_x(builder, column))
        .collect();
    let first_x = xs[0];
    let last_x = *xs.last().unwrap();
    let run = (last_x - first_x).max(0.001);

    let ideal = |column: &Column| -> f64 {
        if stem_up {
            column.stem_far_y() - STEM_LENGTH
        } else {
            column.stem_far_y() + STEM_LENGTH
        }
    };
    let first_ideal = ideal(&members[0]);
    let last_ideal = ideal(members.last().unwrap());
    // A gentle, capped slope reads better than following the outer notes.
    let slope = ((last_ideal - first_ideal) / run).clamp(-0.25, 0.25);
    let cap = 1.5 / run;
    let slope = slope.clamp(-cap, cap);

    let mut offset = if stem_up { f64::INFINITY } else { f64::NEG_INFINITY };
    for (column, x) in members.iter().zip(&xs) {
        let limit = if stem_up {
            column.stem_far_y() - BEAM_MIN_STEM - slope * (x - first_x)
        } else {
            column.stem_far_y() + BEAM_MIN_STEM - slope * (x - first_x)
        };
        offset = if stem_up {
            offset.min(limit)
        } else {
            offset.max(limit)
        };
    }
    // Keep the beam from crowding the staff on the far side.
    let line_y = |x: f64| offset + slope * (x - first_x);

    let thickness = builder.engraving.beam_thickness;
    let spacing = builder.engraving.beam_spacing;
    let stem_thickness = builder.engraving.stem_thickness;
    let inward = if stem_up { 1.0 } else { -1.0 };

    for (column, x) in members.iter().zip(&xs) {
        draw_stem(builder, column, line_y(*x));
    }

    let max_level = members
        .iter()
        .map(|column| column.value.flags())
        .max()
        .unwrap_or(1)
        .max(1);
    for level in 0..max_level {
        let dy = inward * (f64::from(level) * (thickness + spacing) + thickness * 0.5);
        let mut index = 0;
        while index < members.len() {
            if members[index].value.flags() <= level {
                index += 1;
                continue;
            }
            let start = index;
            while index < members.len() && members[index].value.flags() > level {
                index += 1;
            }
            let end = index - 1;
            if start == end {
                if level == 0 {
                    continue;
                }
                // A lone short note inside the group takes a hook.
                let x = xs[start];
                let hook = 1.0;
                let toward_previous = start > 0;
                let (from, to) = if toward_previous {
                    (x - hook, x + stem_thickness * 0.5)
                } else {
                    (x - stem_thickness * 0.5, x + hook)
                };
                builder.beam(
                    Point::new(from, line_y(from) + dy),
                    Point::new(to, line_y(to) + dy),
                    thickness,
                );
                continue;
            }
            let from = xs[start] - stem_thickness * 0.5;
            let to = xs[end] + stem_thickness * 0.5;
            builder.beam(
                Point::new(from, line_y(from) + dy),
                Point::new(to, line_y(to) + dy),
                thickness,
            );
        }
    }
}

/// Reads a length in whole notes as a written note value: a power of two plus
/// augmentation dots.
fn note_value_of(value: f64) -> NoteValue {
    if !(value > 0.0) {
        return NoteValue { power: 2, dots: 0 };
    }
    for power in 0..=7_u8 {
        let base = 0.5_f64.powi(i32::from(power));
        for dots in 0..=2_u8 {
            let scale = 2.0 - 0.5_f64.powi(i32::from(dots));
            if (value - base * scale).abs() < 1e-6 {
                return NoteValue { power, dots };
            }
        }
    }
    // Not a written value (a quantized import can produce these): take the
    // largest note that fits, so the notehead and beaming stay sane.
    let mut power = 0_u8;
    while power < 7 && 0.5_f64.powi(i32::from(power)) > value + 1e-9 {
        power += 1;
    }
    NoteValue { power, dots: 0 }
}

fn diatonic_index(pitch: Pitch) -> i32 {
    i32::from(pitch.octave) * 7 + i32::from(pitch.step.index())
}

fn rational_f64(value: Rational) -> f64 {
    value.numerator() as f64 / value.denominator() as f64
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use makepad_score::model::{
        Alter, Change, Duration, EventTag, FlowNode, IdGenerator, LayerTag, MapScope, MeasureTag, Note,
        NoteTag, Part, PartTag, Staff, StaffKind, StaffTag, Step, Transposition, VoiceTag,
    };
    use makepad_score_render::PaintKind;

    fn duration(numerator: i64, denominator: u64) -> f64 {
        numerator as f64 / denominator as f64
    }

    /// A one-measure grand-staff score whose upper voice is `pitches`, each of
    /// `note_denominator` length, starting on the downbeat.
    pub(crate) fn fixture(pitches: &[(Step, i8)], note_denominator: u64) -> Score {
        let events: Vec<Placed> = pitches
            .iter()
            .enumerate()
            .map(|(index, &(step, octave))| Placed {
                onset: (index as i64, note_denominator),
                duration: (1, note_denominator),
                step,
                octave,
            })
            .collect();
        fixture_events(&events)
    }

    /// One note of a test fixture: where it starts, how long it lasts, what
    /// pitch it is.
    #[derive(Clone, Copy, Debug)]
    pub(crate) struct Placed {
        pub onset: (i64, u64),
        pub duration: (i64, u64),
        pub step: Step,
        pub octave: i8,
    }

    /// A one-measure, one-voice grand-staff score holding exactly `events`.
    pub(crate) fn fixture_events(events: &[Placed]) -> Score {
        let mut ids = IdGenerator::new(0x7e57);
        let piano = ids.next::<PartTag>().unwrap();
        let treble = ids.next::<StaffTag>().unwrap();
        let bass = ids.next::<StaffTag>().unwrap();
        let right = ids.next::<VoiceTag>().unwrap();
        let left = ids.next::<VoiceTag>().unwrap();
        let _ = ids.next::<LayerTag>().unwrap();
        let mut score = Score::new(*b"MAKEPADSCORETEST");
        score.title = "Fixture".into();
        score.parts.insert(
            piano,
            Part {
                id: piano,
                name: "Piano".into(),
                staves: vec![treble, bass],
                transposition: Transposition::NONE,
            },
        );
        for (id, parent) in [(treble, None), (bass, Some(treble))] {
            score.staves.insert(
                id,
                Staff {
                    id,
                    part: piano,
                    parent,
                    kind: StaffKind::Standard,
                    voices: vec![if id == treble { right } else { left }],
                },
            );
        }
        let measure = ids.next::<MeasureTag>().unwrap();
        score.measures.insert(
            measure,
            Measure {
                id: measure,
                ordinal: 0,
                label: "1".into(),
                start: ScoreTime::ZERO,
                extent: Duration::new(1, 1).unwrap(),
            },
        );
        score.flow.nodes.push(FlowNode {
            measure,
            ordinal: 0,
        });
        let mut placed = Vec::new();
        for note in events {
            let event = ids.next::<EventTag>().unwrap();
            let id = ids.next::<NoteTag>().unwrap();
            placed.push(TimedEvent {
                id: event,
                onset: ScoreTime::new(note.onset.0, note.onset.1).unwrap(),
                duration: Some(Duration::new(note.duration.0, note.duration.1).unwrap()),
                grace: None,
                kind: EventKind::Chord(vec![Note {
                    id,
                    written_pitch: Some(Pitch::new(note.step, Alter::NATURAL, note.octave)),
                    unpitched_sound: None,
                    display_staff: treble,
                    tie_from: None,
                    tie_to: None,
                    tab: None,
                    notehead: Notehead::Normal,
                }]),
                beams: Vec::new(),
                tuplets: Vec::new(),
                articulations: Vec::new(),
                ornaments: Vec::new(),
            });
        }
        score.voices.insert(
            right,
            makepad_score::model::Voice {
                id: right,
                staff: treble,
                number: 1,
                events: placed,
            },
        );
        score.voices.insert(
            left,
            makepad_score::model::Voice {
                id: left,
                staff: bass,
                number: 2,
                events: Vec::new(),
            },
        );
        score.maps.time_signature.push(Change {
            at: ScoreTime::ZERO,
            scope: MapScope::Global,
            value: Meter::Measured {
                groups: vec![4],
                unit: 4,
            },
        });
        score
    }

    pub(crate) struct Drawn {
        pub noteheads: Vec<(f64, f64, f64)>,
        pub beams: Vec<Beam>,
        pub stems: Vec<Rect>,
        pub ledgers: Vec<Rect>,
        pub glyphs: Vec<String>,
        /// Page y of the upper staff's top line on the first system.
        pub staff_top: f64,
        /// Page x of the first system's closing barline.
        pub system_right: f64,
    }

    pub(crate) fn engrave(score: &Score) -> Drawn {
        let mut spacing = crate::spacing::ScoreSpacing::new();
        spacing.rebuild(score);
        let placement = spacing.pages()[0].clone();
        let system = &placement.systems[0];
        let (page, _elements) = make_page(score, &placement, 0, 1).unwrap();
        let mut drawn = Drawn {
            noteheads: Vec::new(),
            beams: Vec::new(),
            stems: Vec::new(),
            ledgers: Vec::new(),
            glyphs: Vec::new(),
            staff_top: system.top,
            system_right: system.measures.last().map(|m| m.right).unwrap_or(system.right),
        };
        for item in page.items() {
            match &item.kind {
                PaintKind::Glyph(glyph) => {
                    drawn.glyphs.push(glyph.glyph.0.to_string());
                    if glyph.glyph.0.starts_with("notehead") {
                        drawn.noteheads.push((
                            glyph.origin.x,
                            glyph.origin.y,
                            item.bounds.width(),
                        ));
                    }
                }
                PaintKind::Primitive(Primitive::Beam(beam)) => drawn.beams.push(*beam),
                PaintKind::Primitive(Primitive::Rule {
                    rect,
                    kind: RuleKind::Stem,
                    ..
                }) => drawn.stems.push(*rect),
                PaintKind::Primitive(Primitive::Rule {
                    rect,
                    kind: RuleKind::Ledger,
                    ..
                }) => drawn.ledgers.push(*rect),
                _ => {}
            }
        }
        drawn
    }

    #[test]
    fn eighth_notes_are_beamed_by_the_beat_and_the_beam_clears_every_head() {
        let pitches: Vec<(Step, i8)> = [
            (Step::C, 4),
            (Step::D, 4),
            (Step::E, 4),
            (Step::F, 4),
            (Step::G, 4),
            (Step::A, 4),
            (Step::B, 4),
            (Step::C, 5),
        ]
        .into();
        let drawn = engrave(&fixture(&pitches, 8));
        assert_eq!(drawn.noteheads.len(), 8);
        // Four beats of two eighths each.
        assert_eq!(drawn.beams.len(), 4);
        assert_eq!(drawn.stems.len(), 8);
        for beam in &drawn.beams {
            let heads: Vec<_> = drawn
                .noteheads
                .iter()
                .filter(|(x, _, width)| {
                    *x + *width >= beam.start.x - 0.3 && *x <= beam.end.x + 0.3
                })
                .collect();
            assert_eq!(heads.len(), 2, "each beam spans exactly its two heads");
            // A beam sits wholly above its heads (stems up) or wholly below.
            let above = beam.start.y < heads[0].1;
            for (x, y, width) in heads {
                // The beam is slanted: measure it directly over the notehead.
                let t = ((x + width * 0.5 - beam.start.x) / (beam.end.x - beam.start.x))
                    .clamp(0.0, 1.0);
                let center = beam.start.y + (beam.end.y - beam.start.y) * t;
                let near_edge = center + beam.thickness * 0.5 * if above { 1.0 } else { -1.0 };
                let clearance = if above { y - near_edge } else { near_edge - y };
                assert!(
                    clearance >= 2.0,
                    "beam edge {near_edge} crowds a notehead centred on {y}"
                );
            }
        }
        // Every stem ends exactly on the outer edge of its beam.
        for stem in &drawn.stems {
            let x = stem.center().x;
            assert!(
                drawn
                    .beams
                    .iter()
                    .filter(|beam| x >= beam.start.x - 0.2 && x <= beam.end.x + 0.2)
                    .any(|beam| {
                        let t = ((x - beam.start.x) / (beam.end.x - beam.start.x)).clamp(0.0, 1.0);
                        let center = beam.start.y + (beam.end.y - beam.start.y) * t;
                        (center - beam.thickness * 0.5 - stem.min.y).abs() < 0.02
                            || (center + beam.thickness * 0.5 - stem.max.y).abs() < 0.02
                    }),
                "a stem at {stem:?} does not meet a beam"
            );
        }
    }

    #[test]
    fn high_notes_take_ledger_lines_at_whole_staff_positions() {
        // A5 and C6 are the first two ledger positions above a treble staff.
        let drawn = engrave(&fixture(&[(Step::C, 6)], 4));
        assert_eq!(drawn.noteheads.len(), 1);
        let mut lines: Vec<f64> = drawn
            .ledgers
            .iter()
            .map(|rect| rect.center().y - drawn.staff_top)
            .collect();
        lines.sort_by(f64::total_cmp);
        assert_eq!(lines, vec![-2.0, -1.0]);
        let head_width = drawn.noteheads[0].2;
        for ledger in &drawn.ledgers {
            assert!(
                ledger.width() > head_width,
                "a ledger line must extend past the notehead"
            );
        }
    }

    #[test]
    fn notes_inside_the_staff_take_no_ledger_lines() {
        let drawn = engrave(&fixture(&[(Step::B, 4), (Step::E, 4), (Step::F, 5)], 4));
        assert!(drawn.ledgers.is_empty(), "{:?}", drawn.ledgers);
    }

    #[test]
    fn an_empty_staff_gets_a_measure_rest_and_the_page_gets_its_furniture() {
        let drawn = engrave(&fixture(&[(Step::G, 4)], 4));
        assert!(drawn.glyphs.iter().any(|name| name == "restWhole"));
        assert!(drawn.glyphs.iter().any(|name| name == "gClef"));
        assert!(drawn.glyphs.iter().any(|name| name == "fClef"));
        assert!(drawn.glyphs.iter().any(|name| name == "timeSig4"));
    }

    #[test]
    fn durations_read_as_written_values() {
        assert_eq!(note_value_of(duration(1, 1)), NoteValue { power: 0, dots: 0 });
        assert_eq!(note_value_of(duration(1, 2)), NoteValue { power: 1, dots: 0 });
        assert_eq!(note_value_of(duration(1, 4)), NoteValue { power: 2, dots: 0 });
        assert_eq!(note_value_of(duration(3, 8)), NoteValue { power: 2, dots: 1 });
        assert_eq!(note_value_of(duration(1, 8)), NoteValue { power: 3, dots: 0 });
        assert_eq!(note_value_of(duration(1, 16)).flags(), 2);
        // 5/16 is not a written value; it degrades to the largest that fits.
        assert_eq!(note_value_of(duration(5, 16)), NoteValue { power: 2, dots: 0 });
    }

    #[test]
    fn diatonic_positions_follow_the_clef() {
        let treble = StaffFrame::treble(10.0);
        // B4 is the middle line; C4 is one ledger line below the staff.
        let b4 = diatonic_index(Pitch::new(Step::B, Alter::NATURAL, 4));
        let c4 = diatonic_index(Pitch::new(Step::C, Alter::NATURAL, 4));
        let f5 = diatonic_index(Pitch::new(Step::F, Alter::NATURAL, 5));
        assert_eq!(treble.y_of(b4), 12.0);
        assert_eq!(treble.y_of(c4), 15.0);
        assert_eq!(treble.y_of(f5), 10.0);

        let bass = StaffFrame::bass(30.0);
        let d3 = diatonic_index(Pitch::new(Step::D, Alter::NATURAL, 3));
        let c4 = diatonic_index(Pitch::new(Step::C, Alter::NATURAL, 4));
        assert_eq!(bass.y_of(d3), 32.0);
        // Middle C is one ledger line above a bass staff.
        assert_eq!(bass.y_of(c4), 29.0);
    }

    #[test]
    fn key_signature_accidentals_land_on_the_right_lines() {
        assert_eq!(key_signature_steps(0), Vec::<i8>::new());
        // F sharp sits on the top line of a treble staff.
        assert_eq!(key_signature_steps(1), vec![4]);
        assert_eq!(key_signature_steps(-1), vec![0]);
        assert_eq!(key_signature_steps(5).len(), 5);
    }
}
