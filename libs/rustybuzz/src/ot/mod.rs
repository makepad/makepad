pub mod feature;
mod apply;
mod contextual;
mod kerning;
mod layout;
mod map;
pub mod matching;
mod position;
mod substitute;

pub use apply::*;
pub use kerning::*;
pub use layout::*;
pub use map::*;
pub use position::*;
pub use substitute::*;

use alloc::vec::Vec;

use ttf_parser::opentype_layout::{Coverage, Lookup};
use ttf_parser::gpos::PositioningSubtable;
use ttf_parser::gsub::SubstitutionSubtable;

use crate::glyph_set::{GlyphSet, GlyphSetBuilder};

#[allow(dead_code)]
pub mod lookup_flags {
    pub const RIGHT_TO_LEFT: u16 = 0x0001;
    pub const IGNORE_BASE_GLYPHS: u16 = 0x0002;
    pub const IGNORE_LIGATURES: u16 = 0x0004;
    pub const IGNORE_MARKS: u16 = 0x0008;
    pub const IGNORE_FLAGS: u16 = 0x000E;
    pub const USE_MARK_FILTERING_SET: u16 = 0x0010;
    pub const MARK_ATTACHMENT_TYPE_MASK: u16 = 0xFF00;

}

#[derive(Clone)]
pub struct PositioningTable<'a> {
    pub inner: ttf_parser::opentype_layout::LayoutTable<'a>,
    pub lookups: Vec<PositioningLookup<'a>>,
}

impl<'a> PositioningTable<'a> {
    pub fn new(inner: ttf_parser::opentype_layout::LayoutTable<'a>) -> Self {
        let lookups = inner.lookups.into_iter()
            .map(PositioningLookup::parse)
            .collect();

        Self { inner, lookups}
    }
}

pub trait CoverageExt {
    fn collect(&self, set: &mut GlyphSetBuilder);
}

impl CoverageExt for Coverage<'_> {
    /// Collect this coverage table into a glyph set.
    fn collect(&self, set: &mut GlyphSetBuilder) {
        match *self {
            Self::Format1 { glyphs } => {
                for glyph in glyphs {
                    set.insert(glyph);
                }
            }
            Self::Format2 { records } => {
                for record in records {
                    set.insert_range(record.start..=record.end);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct PositioningLookup<'a> {
    pub subtables: Vec<PositioningSubtable<'a>>,
    pub coverage: GlyphSet,
    pub props: u32,
}

impl<'a> PositioningLookup<'a> {
    pub fn parse(lookup: Lookup<'a>) -> Self {
        let subtables: Vec<_> = lookup
            .subtables
            .into_iter::<PositioningSubtable>()
            .collect();

        let mut coverage = GlyphSet::builder();
        for subtable in &subtables {
            subtable.coverage().collect(&mut coverage);
        }

        Self {
            subtables,
            coverage: coverage.finish(),
            props: lookup_props(lookup),
        }
    }
}


#[derive(Clone)]
pub struct SubstitutionTable<'a> {
    pub inner: ttf_parser::opentype_layout::LayoutTable<'a>,
    pub lookups: Vec<SubstLookup<'a>>,
}

impl<'a> SubstitutionTable<'a> {
    pub fn new(inner: ttf_parser::opentype_layout::LayoutTable<'a>) -> Self {
        let lookups = inner.lookups.into_iter()
            .map(SubstLookup::parse)
            .collect();

        Self { inner, lookups}
    }
}

#[derive(Clone)]
pub struct SubstLookup<'a> {
    pub subtables: Vec<SubstitutionSubtable<'a>>,
    pub coverage: GlyphSet,
    pub reverse: bool,
    pub props: u32,
}

impl<'a> SubstLookup<'a> {
    pub fn parse(lookup: Lookup<'a>) -> Self {
        let subtables: Vec<_> = lookup
            .subtables
            .into_iter::<SubstitutionSubtable>()
            .collect();

        let mut coverage = GlyphSet::builder();
        let mut reverse = !subtables.is_empty();

        for subtable in &subtables {
            subtable.coverage().collect(&mut coverage);
            reverse &= subtable.is_reverse();
        }

        Self {
            subtables,
            coverage: coverage.finish(),
            reverse,
            props: lookup_props(lookup),
        }
    }
}

// lookup_props is a 32-bit integer where the lower 16-bit is LookupFlag and
// higher 16-bit is mark-filtering-set if the lookup uses one.
// Not to be confused with glyph_props which is very similar. */
fn lookup_props(lookup: Lookup) -> u32 {
    let mut props = u32::from(lookup.flags.0);
    if let Some(set) = lookup.mark_filtering_set {
        props |= u32::from(set) << 16;
    }
    props
}
