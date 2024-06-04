//! An [SVG Table](https://docs.microsoft.com/en-us/typography/opentype/spec/svg) implementation.

use crate::parser::{FromData, LazyArray16, NumFrom, Offset, Offset32, Stream};
use crate::GlyphId;

#[derive(Clone, Copy)]
struct SvgDocumentRecord {
    start_glyph_id: GlyphId,
    end_glyph_id: GlyphId,
    svg_doc_offset: Option<Offset32>,
    svg_doc_length: u32,
}

impl FromData for SvgDocumentRecord {
    const SIZE: usize = 12;

    #[inline]
    fn parse(data: &[u8]) -> Option<Self> {
        let mut s = Stream::new(data);
        Some(SvgDocumentRecord {
            start_glyph_id: s.read::<GlyphId>()?,
            end_glyph_id: s.read::<GlyphId>()?,
            svg_doc_offset: s.read::<Option<Offset32>>()?,
            svg_doc_length: s.read::<u32>()?,
        })
    }
}

/// A list of [SVG documents](
/// https://docs.microsoft.com/en-us/typography/opentype/spec/svg#svg-document-list).
#[derive(Clone, Copy)]
pub struct SvgDocumentsList<'a> {
    data: &'a [u8],
    records: LazyArray16<'a, SvgDocumentRecord>,
}

impl<'a> SvgDocumentsList<'a> {
    /// Returns SVG document data at index.
    ///
    /// `index` is not a GlyphId. You should use [`find()`](SvgDocumentsList::find) instead.
    #[inline]
    pub fn get(&self, index: u16) -> Option<&'a [u8]> {
        let record = self.records.get(index)?;
        let offset = record.svg_doc_offset?.to_usize();
        self.data
            .get(offset..offset + usize::num_from(record.svg_doc_length))
    }

    /// Returns a SVG document data by glyph ID.
    #[inline]
    pub fn find(&self, glyph_id: GlyphId) -> Option<&'a [u8]> {
        let index = self
            .records
            .into_iter()
            .position(|v| (v.start_glyph_id..=v.end_glyph_id).contains(&glyph_id))?;
        self.get(index as u16)
    }

    /// Returns the number of SVG documents in the list.
    pub fn len(&self) -> u16 {
        self.records.len()
    }

    /// Checks if the list is empty.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl core::fmt::Debug for SvgDocumentsList<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "SvgDocumentsList {{ ... }}")
    }
}

impl<'a> IntoIterator for SvgDocumentsList<'a> {
    type Item = &'a [u8];
    type IntoIter = SvgDocumentsListIter<'a>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        SvgDocumentsListIter {
            list: self,
            index: 0,
        }
    }
}

/// An iterator over [`SvgDocumentsList`] values.
#[derive(Clone, Copy)]
#[allow(missing_debug_implementations)]
pub struct SvgDocumentsListIter<'a> {
    list: SvgDocumentsList<'a>,
    index: u16,
}

impl<'a> Iterator for SvgDocumentsListIter<'a> {
    type Item = &'a [u8];

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.list.len() {
            self.index += 1;
            self.list.get(self.index - 1)
        } else {
            None
        }
    }

    #[inline]
    fn count(self) -> usize {
        usize::from(self.list.len().saturating_sub(self.index))
    }
}

/// An [SVG Table](https://docs.microsoft.com/en-us/typography/opentype/spec/svg).
#[derive(Clone, Copy, Debug)]
pub struct Table<'a> {
    /// A list of SVG documents.
    pub documents: SvgDocumentsList<'a>,
}

impl<'a> Table<'a> {
    /// Parses a table from raw data.
    pub fn parse(data: &'a [u8]) -> Option<Self> {
        let mut s = Stream::new(data);
        s.skip::<u16>(); // version
        let doc_list_offset = s.read::<Option<Offset32>>()??;

        let mut s = Stream::new_at(data, doc_list_offset.to_usize())?;
        let count = s.read::<u16>()?;
        let records = s.read_array16::<SvgDocumentRecord>(count)?;

        Some(Table {
            documents: SvgDocumentsList {
                data: &data[doc_list_offset.0 as usize..],
                records,
            },
        })
    }
}
