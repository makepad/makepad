use crate::{Beam, Point, Rect, Ribbon, SpatialIndex, Sp};
use std::{collections::BTreeSet, fmt, sync::Arc};

/// Stable score-model identity. IDs must survive re-layout and re-pagination.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageId(pub u32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MusicFontRef(pub u32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextFontRef(pub u32);

/// A canonical SMuFL glyph name, for example `noteheadBlack`.
///
/// Code points are intentionally not public identity: optional glyph mappings
/// and alternates belong to a specific font profile.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SmuflGlyph(pub Arc<str>);

impl SmuflGlyph {
    pub fn new(name: impl Into<Arc<str>>) -> Self {
        Self(name.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InkRole {
    Primary,
    Staff,
    Secondary,
    Playback,
    Selection,
    Annotation,
    Hover,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinearRgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl LinearRgba {
    pub const TRANSPARENT: Self = Self {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_srgb8(r: u8, g: u8, b: u8, a: u8) -> Self {
        fn linear(channel: u8) -> f32 {
            let value = channel as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        Self::new(linear(r), linear(g), linear(b), a as f32 / 255.0)
    }

    pub fn with_alpha(self, alpha: f32) -> Self {
        Self { a: alpha, ..self }
    }

    pub fn is_finite(self) -> bool {
        self.r.is_finite() && self.g.is_finite() && self.b.is_finite() && self.a.is_finite()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    pub role: InkRole,
    pub override_color: Option<LinearRgba>,
}

impl Ink {
    pub const fn role(role: InkRole) -> Self {
        Self {
            role,
            override_color: None,
        }
    }

    pub const fn color(role: InkRole, color: LinearRgba) -> Self {
        Self {
            role,
            override_color: Some(color),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuleKind {
    Staff,
    Stem,
    Ledger,
    BarlineThin,
    BarlineThick,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineKind {
    Hairpin,
    Bracket,
    Octave,
    Pedal,
    Tuplet,
    RepeatEnding,
    LyricExtender,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DashPattern {
    pub on: Sp,
    pub off: Sp,
    /// Phase is anchored in page/staff-space coordinates, so it does not crawl.
    pub phase: Sp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HairpinDirection {
    Crescendo,
    Diminuendo,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Rule {
        rect: Rect,
        kind: RuleKind,
        /// Five staff lines share a nonzero group to receive one snap phase.
        staff_group: Option<u32>,
    },
    Beam(Beam),
    Ribbon(Ribbon),
    Hairpin {
        start: Point,
        end: Point,
        opening: Sp,
        thickness: Sp,
        direction: HairpinDirection,
    },
    Bracket {
        x: Sp,
        top: Sp,
        bottom: Sp,
        thickness: Sp,
        hook: Sp,
    },
    Line {
        start: Point,
        end: Point,
        thickness: Sp,
        dash: Option<DashPattern>,
        kind: LineKind,
    },
    TupletBracket {
        start: Point,
        end: Point,
        thickness: Sp,
        hook: Sp,
        number_gap: Rect,
    },
}

impl Primitive {
    pub fn conservative_bounds(&self) -> Rect {
        match self {
            Self::Rule { rect, .. } => *rect,
            Self::Beam(beam) => beam.bounds(),
            Self::Ribbon(ribbon) => ribbon.bounds(),
            Self::Hairpin {
                start,
                end,
                opening,
                thickness,
                ..
            } => Rect::from_points([*start, *end]).expanded(opening * 0.5 + thickness * 0.5),
            // The vertical spine is stroked, so its bounds have to include
            // the half-thickness the stroke puts past each end as well.
            Self::Bracket {
                x,
                top,
                bottom,
                thickness,
                hook,
            } => Rect::from_xywh(
                *x - *thickness * 0.5,
                *top - *thickness * 0.5,
                *hook + *thickness,
                *bottom - *top + *thickness,
            ),
            Self::Line {
                start,
                end,
                thickness,
                ..
            } => Rect::from_points([*start, *end]).expanded(*thickness * 0.5),
            Self::TupletBracket {
                start,
                end,
                thickness,
                hook,
                ..
            } => Rect::from_points([*start, *end]).expanded(*hook + *thickness * 0.5),
        }
    }

    fn is_finite(&self) -> bool {
        match self {
            Self::Rule { rect, .. } => rect.is_finite(),
            Self::Beam(beam) => {
                beam.start.is_finite()
                    && beam.end.is_finite()
                    && beam.thickness.is_finite()
                    && beam.thickness > 0.0
            }
            Self::Ribbon(ribbon) => {
                ribbon.curve.is_finite()
                    && ribbon.endpoint_thickness.is_finite()
                    && ribbon.midpoint_thickness.is_finite()
                    && ribbon.endpoint_thickness > 0.0
                    && ribbon.midpoint_thickness > 0.0
            }
            Self::Hairpin {
                start,
                end,
                opening,
                thickness,
                ..
            } => {
                start.is_finite()
                    && end.is_finite()
                    && opening.is_finite()
                    && thickness.is_finite()
                    && *opening >= 0.0
                    && *thickness > 0.0
            }
            Self::Bracket {
                x,
                top,
                bottom,
                thickness,
                hook,
            } => [*x, *top, *bottom, *thickness, *hook]
                .into_iter()
                .all(f64::is_finite),
            Self::Line {
                start,
                end,
                thickness,
                dash,
                ..
            } => {
                start.is_finite()
                    && end.is_finite()
                    && thickness.is_finite()
                    && *thickness > 0.0
                    && dash.is_none_or(|dash| {
                        dash.on.is_finite()
                            && dash.off.is_finite()
                            && dash.phase.is_finite()
                            && dash.on > 0.0
                            && dash.off >= 0.0
                    })
            }
            Self::TupletBracket {
                start,
                end,
                thickness,
                hook,
                number_gap,
            } => {
                start.is_finite()
                    && end.is_finite()
                    && thickness.is_finite()
                    && hook.is_finite()
                    && *thickness > 0.0
                    && *hook >= 0.0
                    && number_gap.is_finite()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphItem {
    pub font: MusicFontRef,
    pub glyph: SmuflGlyph,
    /// SMuFL registration origin in page coordinates.
    pub origin: Point,
    /// Em height in staff spaces. Standard music size is normally `4.0`.
    pub em_size: Sp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextDirection {
    Auto,
    LeftToRight,
    RightToLeft,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub font: TextFontRef,
    pub text: Arc<str>,
    pub origin: Point,
    pub size: Sp,
    pub letter_spacing: Sp,
    pub direction: TextDirection,
    pub language: Option<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaintKind {
    Glyph(GlyphItem),
    Primitive(Primitive),
    Text(TextRun),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaintItem {
    pub id: SemanticId,
    pub bounds: Rect,
    pub z: i16,
    pub ink: Ink,
    pub kind: PaintKind,
}

impl PaintItem {
    pub fn primitive(id: SemanticId, z: i16, ink: Ink, primitive: Primitive) -> Self {
        Self {
            id,
            bounds: primitive.conservative_bounds(),
            z,
            ink,
            kind: PaintKind::Primitive(primitive),
        }
    }

    fn is_valid(&self) -> bool {
        self.id.0 != 0
            && self.bounds.is_finite()
            && self.ink.override_color.is_none_or(LinearRgba::is_finite)
            && match &self.kind {
                PaintKind::Glyph(glyph) => {
                    glyph.origin.is_finite()
                        && glyph.em_size.is_finite()
                        && glyph.em_size > 0.0
                        && !glyph.glyph.0.is_empty()
                }
                PaintKind::Primitive(primitive) => primitive.is_finite(),
                PaintKind::Text(run) => {
                    run.origin.is_finite()
                        && run.size.is_finite()
                        && run.size > 0.0
                        && run.letter_spacing.is_finite()
                }
            }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaintListError {
    InvalidPageSize,
    InvalidItem(SemanticId),
    DuplicateSemanticId(SemanticId),
    MissingPage(PageId),
}

impl fmt::Display for PaintListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPageSize => f.write_str("page size must be finite and positive"),
            Self::InvalidItem(id) => write!(f, "paint item {} is invalid", id.0),
            Self::DuplicateSemanticId(id) => write!(f, "duplicate semantic id {}", id.0),
            Self::MissingPage(page) => write!(f, "page {} is not cached", page.0),
        }
    }
}

impl std::error::Error for PaintListError {}

/// Immutable display list for one score page.
///
/// # Coordinate convention
///
/// Every coordinate is an `f64` staff-space value in page-local coordinates:
/// `(0, 0)` is the physical page's top-left, +x points right, +y points down,
/// and `1.0` is exactly the distance between adjacent staff lines. Geometry is
/// never pixel-snapped in this list. A backend applies page translation, zoom,
/// and device scale once; SMuFL y-up font outlines are flipped only while the
/// glyph backend registers an outline. Bounds use the same convention and are
/// conservative ink bounds, not hit slop.
#[derive(Clone, Debug)]
pub struct PaintList {
    page_id: PageId,
    revision: u64,
    page_size: Point,
    items: Arc<[PaintItem]>,
    semantic_lookup: Arc<[(SemanticId, u32)]>,
    spatial: SpatialIndex,
    fingerprint: u64,
}

impl PaintList {
    pub fn new(
        page_id: PageId,
        revision: u64,
        page_size: Point,
        mut items: Vec<PaintItem>,
    ) -> Result<Self, PaintListError> {
        if !page_size.is_finite() || page_size.x <= 0.0 || page_size.y <= 0.0 {
            return Err(PaintListError::InvalidPageSize);
        }
        if let Some(item) = items.iter().find(|item| !item.is_valid()) {
            return Err(PaintListError::InvalidItem(item.id));
        }
        items.sort_by_key(|item| (item.z, item.id));
        let mut ids = BTreeSet::new();
        for item in &items {
            if !ids.insert(item.id) {
                return Err(PaintListError::DuplicateSemanticId(item.id));
            }
        }
        let semantic_lookup: Arc<[(SemanticId, u32)]> = {
            let mut lookup: Vec<_> = items
                .iter()
                .enumerate()
                .map(|(index, item)| (item.id, index as u32))
                .collect();
            lookup.sort_unstable_by_key(|entry| entry.0);
            lookup.into()
        };
        let spatial = SpatialIndex::build(
            &items
                .iter()
                .map(|item| item.bounds)
                .collect::<Vec<_>>(),
        );
        let fingerprint = fingerprint_items(page_id, revision, page_size, &items);
        Ok(Self {
            page_id,
            revision,
            page_size,
            items: items.into(),
            semantic_lookup,
            spatial,
            fingerprint,
        })
    }

    pub fn page_id(&self) -> PageId {
        self.page_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn page_size(&self) -> Point {
        self.page_size
    }

    pub fn items(&self) -> &[PaintItem] {
        &self.items
    }

    pub fn item(&self, id: SemanticId) -> Option<&PaintItem> {
        self.semantic_lookup
            .binary_search_by_key(&id, |entry| entry.0)
            .ok()
            .map(|lookup_index| self.semantic_lookup[lookup_index].1 as usize)
            .map(|paint_index| &self.items[paint_index])
    }

    pub fn visible_indices(&self, viewport: Rect) -> Vec<usize> {
        self.spatial.query(viewport)
    }

    pub fn hit_test(&self, point: Point, tolerance: Sp) -> Vec<SemanticId> {
        let query = Rect::from_xywh(
            point.x - tolerance,
            point.y - tolerance,
            tolerance * 2.0,
            tolerance * 2.0,
        );
        let mut hits: Vec<_> = self
            .visible_indices(query)
            .into_iter()
            .filter(|index| self.items[*index].bounds.expanded(tolerance).contains(point))
            .map(|index| self.items[index].id)
            .collect();
        hits.sort_unstable();
        hits
    }

    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    pub fn memory_bytes(&self) -> usize {
        self.items.len() * std::mem::size_of::<PaintItem>()
            + self.semantic_lookup.len() * std::mem::size_of::<(SemanticId, u32)>()
            + self.spatial.memory_bytes()
            + self
                .items
                .iter()
                .map(|item| match &item.kind {
                    PaintKind::Glyph(glyph) => glyph.glyph.0.len(),
                    PaintKind::Text(run) => {
                        run.text.len() + run.language.as_ref().map_or(0, |text| text.len())
                    }
                    PaintKind::Primitive(_) => 0,
                })
                .sum::<usize>()
    }

    pub fn diff(&self, newer: &Self) -> PaintDiff {
        let mut old = self.semantic_lookup.iter().copied().peekable();
        let mut new = newer.semantic_lookup.iter().copied().peekable();
        let mut diff = PaintDiff::default();
        loop {
            match (old.peek().copied(), new.peek().copied()) {
                (Some((old_id, old_index)), Some((new_id, new_index))) if old_id == new_id => {
                    if self.items[old_index as usize] != newer.items[new_index as usize] {
                        diff.changed.push(old_id);
                    }
                    old.next();
                    new.next();
                }
                (Some((old_id, _)), Some((new_id, _))) if old_id < new_id => {
                    diff.removed.push(old_id);
                    old.next();
                }
                (Some(_), Some((new_id, _))) => {
                    diff.added.push(new_id);
                    new.next();
                }
                (Some((old_id, _)), None) => {
                    diff.removed.push(old_id);
                    old.next();
                }
                (None, Some((new_id, _))) => {
                    diff.added.push(new_id);
                    new.next();
                }
                (None, None) => break,
            }
        }
        diff
    }

    pub fn patched(
        &self,
        revision: u64,
        remove: &[SemanticId],
        replacements: Vec<PaintItem>,
    ) -> Result<Self, PaintListError> {
        let remove: BTreeSet<_> = remove
            .iter()
            .copied()
            .chain(replacements.iter().map(|item| item.id))
            .collect();
        let mut items: Vec<_> = self
            .items
            .iter()
            .filter(|item| !remove.contains(&item.id))
            .cloned()
            .collect();
        items.extend(replacements);
        Self::new(self.page_id, revision, self.page_size, items)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaintDiff {
    pub added: Vec<SemanticId>,
    pub changed: Vec<SemanticId>,
    pub removed: Vec<SemanticId>,
}

impl PaintDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }

    pub fn upload_count(&self) -> usize {
        self.added.len() + self.changed.len()
    }
}

fn fingerprint_items(page_id: PageId, revision: u64, page_size: Point, items: &[PaintItem]) -> u64 {
    let mut hash = Fnv64::new();
    hash.u64(page_id.0 as u64);
    hash.u64(revision);
    hash.f64(page_size.x);
    hash.f64(page_size.y);
    for item in items {
        hash.u64(item.id.0);
        hash.u64(item.z as u16 as u64);
        hash.f64(item.bounds.min.x);
        hash.f64(item.bounds.min.y);
        hash.f64(item.bounds.max.x);
        hash.f64(item.bounds.max.y);
        hash.u64(item.kind.discriminant());
        match &item.kind {
            PaintKind::Glyph(glyph) => {
                hash.bytes(glyph.glyph.0.as_bytes());
                hash.u64(glyph.font.0 as u64);
                hash.f64(glyph.origin.x);
                hash.f64(glyph.origin.y);
                hash.f64(glyph.em_size);
            }
            PaintKind::Primitive(primitive) => hash.primitive(primitive),
            PaintKind::Text(text) => {
                hash.bytes(text.text.as_bytes());
                hash.u64(text.font.0 as u64);
                hash.f64(text.origin.x);
                hash.f64(text.origin.y);
                hash.f64(text.size);
            }
        }
    }
    hash.finish()
}

trait KindDiscriminant {
    fn discriminant(&self) -> u64;
}

impl KindDiscriminant for PaintKind {
    fn discriminant(&self) -> u64 {
        match self {
            Self::Glyph(_) => 1,
            Self::Primitive(_) => 2,
            Self::Text(_) => 3,
        }
    }
}

struct Fnv64(u64);

impl Fnv64 {
    fn new() -> Self {
        Self(0xcbf29ce484222325)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn f64(&mut self, value: f64) {
        self.u64(value.to_bits());
    }

    fn finish(self) -> u64 {
        self.0
    }

    fn point(&mut self, point: Point) {
        self.f64(point.x);
        self.f64(point.y);
    }

    fn rect(&mut self, rect: Rect) {
        self.point(rect.min);
        self.point(rect.max);
    }

    fn primitive(&mut self, primitive: &Primitive) {
        match primitive {
            Primitive::Rule {
                rect,
                kind,
                staff_group,
            } => {
                self.u64(1);
                self.rect(*rect);
                self.u64(*kind as u64);
                self.u64(staff_group.map_or(0, |group| group as u64 + 1));
            }
            Primitive::Beam(beam) => {
                self.u64(2);
                self.point(beam.start);
                self.point(beam.end);
                self.f64(beam.thickness);
            }
            Primitive::Ribbon(ribbon) => {
                self.u64(3);
                self.point(ribbon.curve.p0);
                self.point(ribbon.curve.p1);
                self.point(ribbon.curve.p2);
                self.point(ribbon.curve.p3);
                self.f64(ribbon.endpoint_thickness);
                self.f64(ribbon.midpoint_thickness);
            }
            Primitive::Hairpin {
                start,
                end,
                opening,
                thickness,
                direction,
            } => {
                self.u64(4);
                self.point(*start);
                self.point(*end);
                self.f64(*opening);
                self.f64(*thickness);
                self.u64(*direction as u64);
            }
            Primitive::Bracket {
                x,
                top,
                bottom,
                thickness,
                hook,
            } => {
                self.u64(5);
                for value in [x, top, bottom, thickness, hook] {
                    self.f64(*value);
                }
            }
            Primitive::Line {
                start,
                end,
                thickness,
                dash,
                kind,
            } => {
                self.u64(6);
                self.point(*start);
                self.point(*end);
                self.f64(*thickness);
                self.u64(*kind as u64);
                if let Some(dash) = dash {
                    self.u64(1);
                    self.f64(dash.on);
                    self.f64(dash.off);
                    self.f64(dash.phase);
                } else {
                    self.u64(0);
                }
            }
            Primitive::TupletBracket {
                start,
                end,
                thickness,
                hook,
                number_gap,
            } => {
                self.u64(7);
                self.point(*start);
                self.point(*end);
                self.f64(*thickness);
                self.f64(*hook);
                self.rect(*number_gap);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cubic;

    fn rule(id: u64, x: f64) -> PaintItem {
        PaintItem::primitive(
            SemanticId(id),
            0,
            Ink::role(InkRole::Staff),
            Primitive::Rule {
                rect: Rect::from_xywh(x, 2.0, 8.0, 0.13),
                kind: RuleKind::Staff,
                staff_group: Some(1),
            },
        )
    }

    #[test]
    fn list_is_deterministic_and_patchable_by_semantic_id() {
        let page = PaintList::new(
            PageId(3),
            7,
            Point::new(100.0, 140.0),
            vec![rule(2, 4.0), rule(1, 1.0)],
        )
        .unwrap();
        assert_eq!(page.items()[0].id, SemanticId(1));
        assert_eq!(page.item(SemanticId(2)).unwrap().bounds.min.x, 4.0);
        let patched = page.patched(8, &[], vec![rule(2, 9.0)]).unwrap();
        assert_eq!(patched.item(SemanticId(2)).unwrap().bounds.min.x, 9.0);
        assert_eq!(
            page.diff(&patched),
            PaintDiff {
                changed: vec![SemanticId(2)],
                ..PaintDiff::default()
            }
        );
        assert_ne!(page.fingerprint(), patched.fingerprint());
    }

    /// A cull uses these bounds, so they have to hold the ink the backend
    /// actually strokes — half a thickness past each end of a bracket spine
    /// included, or a bracket blinks out at the viewport edge.
    #[test]
    fn conservative_bounds_hold_the_stroked_ink() {
        let bracket = Primitive::Bracket {
            x: 10.0,
            top: 4.0,
            bottom: 20.0,
            thickness: 0.5,
            hook: 1.0,
        };
        let bounds = bracket.conservative_bounds();
        assert_eq!(bounds.min, Point::new(9.75, 3.75));
        assert_eq!(bounds.max, Point::new(11.25, 20.25));
    }

    #[test]
    fn duplicate_ids_are_rejected() {
        let result = PaintList::new(
            PageId(0),
            0,
            Point::new(100.0, 100.0),
            vec![rule(1, 0.0), rule(1, 4.0)],
        );
        assert_eq!(
            result.unwrap_err(),
            PaintListError::DuplicateSemanticId(SemanticId(1))
        );
    }

    #[test]
    fn canonical_coordinates_cull_and_hit() {
        let page = PaintList::new(
            PageId(0),
            0,
            Point::new(100.0, 100.0),
            vec![rule(1, 0.0), rule(2, 30.0)],
        )
        .unwrap();
        assert_eq!(
            page.visible_indices(Rect::from_xywh(0.0, 0.0, 10.0, 10.0)),
            vec![0]
        );
        assert_eq!(page.hit_test(Point::new(3.0, 2.05), 0.0), vec![SemanticId(1)]);
    }

    #[test]
    fn cubic_is_accepted_as_ribbon_geometry() {
        let item = PaintItem::primitive(
            SemanticId(5),
            2,
            Ink::role(InkRole::Primary),
            Primitive::Ribbon(Ribbon {
                curve: Cubic {
                    p0: Point::new(1.0, 3.0),
                    p1: Point::new(2.0, 1.0),
                    p2: Point::new(4.0, 1.0),
                    p3: Point::new(5.0, 3.0),
                },
                endpoint_thickness: 0.10,
                midpoint_thickness: 0.22,
            }),
        );
        assert!(PaintList::new(PageId(0), 1, Point::new(20.0, 20.0), vec![item]).is_ok());
    }
}
