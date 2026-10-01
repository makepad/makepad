//! `makepad-font-subset`: TrueType subsetting for the runtime text stack
//! (draw's `text`: ttf-parser outlines, rustybuzz shaping, glyph atlases
//! keyed by font and glyph id).
//!
//! Glyph ids are retained: a glyph outside the keep set keeps its id with an
//! empty outline and no variation data, so shaping and the atlas keys are
//! unchanged and the subset draws the kept text exactly as the original at
//! every axis value. [`closure`] finds the glyphs a set of shaped runs needs;
//! [`subset`] writes the font: `glyf`/`loca` and `gvar` rebuilt, `cmap` cut
//! to the kept characters, GSUB, GPOS, GDEF, `kern` and `HVAR` rewritten to
//! what the kept glyphs use, the glyph count cut after the last kept glyph,
//! `post` without glyph names, the tables the runtime never reads dropped,
//! axes the text leaves at their default pinned there, the family renamed
//! when its licence reserves the name ([`reserved_font_names`]). [`stub`]
//! writes a one-glyph font for a face nothing draws from, and [`verify`]
//! checks a subset against the original for the text it was made for.

mod closure;
mod cmap;
mod glyf;
mod graph;
mod gvar;
mod layout;
mod name;
mod ot_layout;
mod sfnt;
mod stub;
mod var_store;

pub use {
    closure::{closure, gsub_closure, unvaried_axes, verify, ClosureInputs, ShapeRun},
    name::reserved_font_names,
    stub::stub,
};

use {
    sfnt::{put_u16, u16_at, Tables, Tag},
    std::collections::{BTreeMap, BTreeSet},
};

/// Why a font cannot be subset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubsetError {
    /// A collection, CFF outlines or a symbol cmap: not handled.
    Unsupported(&'static str),
    /// The font's tables contradict themselves or are cut short.
    Malformed(&'static str),
    /// A rewritten table's offsets do not fit (internal: the table is
    /// written another way).
    Overflow,
}

impl std::fmt::Display for SubsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(what) => write!(f, "font subset: unsupported: {what}"),
            Self::Malformed(what) => write!(f, "font subset: malformed font: {what}"),
            Self::Overflow => write!(f, "font subset: a rewritten table's offsets overflow"),
        }
    }
}

impl std::error::Error for SubsetError {}

/// A new family name for a font whose licence reserves its name.
#[derive(Clone, Debug, Default)]
pub struct Rename {
    /// The reserved names, as [`reserved_font_names`] reads them.
    pub reserved: Vec<String>,
    /// What replaces them in every naming record (copyright, licence and
    /// trademark records keep them); PostScript names take it without spaces.
    pub family: String,
}

#[derive(Clone, Debug)]
pub struct SubsetOptions {
    /// Drop TrueType hinting (glyph instructions, `fpgm`, `prep`, `cvt `,
    /// `cvar`, `gasp`): the runtime draws outlines unhinted.
    pub strip_hinting: bool,
    /// Rewrite GSUB, GPOS, GDEF (with its variation store), `kern` and
    /// `HVAR` to what the kept glyphs use. When a table cannot be rewritten
    /// (an offset overflows, a format is unknown) the layout tables are
    /// kept, GPOS pair kerning cleared in place.
    pub prune_layout: bool,
    /// Cut the glyph count to the highest kept glyph (and what a kept
    /// substitution can output). Only when the layout tables were rewritten
    /// and every other table is one that is safe to cut.
    pub trim_glyphs: bool,
    /// Drop Apple's layout tables (`morx`, `kerx` and their companions).
    /// The shaper prefers `morx` over GSUB for horizontal text, so this
    /// changes which table shapes: check the result with [`verify`].
    pub drop_aat: bool,
    /// Axes (tags as `u32`) pinned at their default: their glyph variation
    /// tuples are dropped. The text must be laid out with these axes at
    /// their defaults ([`unvaried_axes`] finds them).
    pub pin_axes: Vec<u32>,
    /// Keep `STAT` (axis naming for font menus; the runtime never reads it).
    pub keep_stat: bool,
    pub rename: Option<Rename>,
}

impl Default for SubsetOptions {
    fn default() -> Self {
        Self {
            strip_hinting: true,
            prune_layout: true,
            trim_glyphs: true,
            drop_aat: false,
            pin_axes: Vec::new(),
            keep_stat: false,
            rename: None,
        }
    }
}

/// One table's size before and after (0 after: dropped).
#[derive(Clone, Debug)]
pub struct TableSize {
    pub tag: String,
    pub before: usize,
    pub after: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SubsetReport {
    pub tables: Vec<TableSize>,
    pub font_before: usize,
    pub font_after: usize,
    /// The glyphs that keep their outlines (the keep set, the glyphs of the
    /// kept characters, `.notdef` and composite components).
    pub kept: BTreeSet<u16>,
    pub glyphs_total: usize,
    /// The glyph count of the subset (lower than the total when trimmed).
    pub glyphs_out: usize,
}

impl std::fmt::Display for SubsetReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{} -> {} bytes, {} of {} glyphs kept, {} glyph ids",
            self.font_before, self.font_after, self.kept.len(), self.glyphs_total, self.glyphs_out
        )?;
        for table in &self.tables {
            writeln!(f, "  {:4} {:>9} -> {:>9}", table.tag, table.before, table.after)?;
        }
        Ok(())
    }
}

/// Tables nothing in the runtime reads.
const DROPPED: [&Tag; 6] = [b"DSIG", b"hdmx", b"LTSH", b"VDMX", b"PCLT", b"meta"];
const HINTING: [&Tag; 5] = [b"fpgm", b"prep", b"cvt ", b"cvar", b"gasp"];
/// Apple layout tables.
const AAT: [&Tag; 18] = [
    b"morx", b"mort", b"kerx", b"ankr", b"trak", b"prop", b"bsln", b"feat", b"lcar", b"opbd", b"just", b"Zapf",
    b"acnt", b"fdsc", b"fmtx", b"gcid", b"ltag", b"xref",
];
/// Tables that hold no glyph-indexed data, or whose glyph-indexed data the
/// subsetter rewrites for a cut glyph count.
const TRIM_SAFE: [&Tag; 27] = [
    b"head", b"hhea", b"maxp", b"OS/2", b"name", b"post", b"cmap", b"glyf", b"loca", b"hmtx", b"vhea", b"vmtx",
    b"gvar", b"HVAR", b"MVAR", b"fvar", b"avar", b"STAT", b"GSUB", b"GPOS", b"GDEF", b"kern", b"BASE", b"gasp",
    b"FFTM", b"fpgm", b"prep",
];

/// Subsets `font` to `keep_glyphs` (normally [`closure`]'s result) and
/// `keep_chars` (normally [`ClosureInputs::chars`]), keeping glyph ids.
///
/// `.notdef`, the glyphs of `keep_chars` and the components of kept
/// composites are always kept, and every character whose glyph is kept stays
/// in `cmap` (so the shaper's composition and fallback lookups answer as in
/// the original).
pub fn subset(
    font: &[u8],
    keep_glyphs: &BTreeSet<u16>,
    keep_chars: &BTreeSet<char>,
    opts: &SubsetOptions,
) -> Result<Vec<u8>, SubsetError> {
    subset_with_report(font, keep_glyphs, keep_chars, opts).map(|(font, _)| font)
}

/// Unicode mappings of every Unicode cmap subtable (the shaper reads one,
/// the Windows full-repertoire or BMP one; the union covers both).
fn unicode_mapping(face: &ttf_parser::Face) -> Result<BTreeMap<u32, u16>, SubsetError> {
    let cmap_table = face.tables().cmap.ok_or(SubsetError::Malformed("no cmap table"))?;
    let mut mapping = BTreeMap::new();
    for subtable in cmap_table.subtables {
        if subtable.platform_id == ttf_parser::PlatformId::Windows && subtable.encoding_id == 0 {
            return Err(SubsetError::Unsupported("symbol-encoded cmap"));
        }
        if !subtable.is_unicode() {
            continue;
        }
        subtable.codepoints(|c| {
            if let Some(g) = subtable.glyph_index(c) {
                mapping.entry(c).or_insert(g.0);
            }
        });
    }
    Ok(mapping)
}

/// `hmtx`/`vmtx` for `count` glyphs: the long metrics cut to the count, the
/// metrics of glyphs that are gone zeroed (the last long metric's advance
/// also serves the glyphs after it, so it stays). Returns the table and its
/// long-metric count.
fn metrics(data: &[u8], long: usize, keep: &BTreeSet<u16>, count: usize) -> Result<(Vec<u8>, usize), SubsetError> {
    let new_long = long.min(count).max(1);
    let mut out = data.get(..4 * new_long).ok_or(SubsetError::Malformed("metrics truncated"))?.to_vec();
    for g in new_long..count {
        let at = if g < long { 4 * g + 2 } else { 4 * long + 2 * (g - long) };
        out.extend_from_slice(data.get(at..at + 2).ok_or(SubsetError::Malformed("metrics truncated"))?);
    }
    for g in (0..count).filter(|&g| !keep.contains(&(g as u16))) {
        let range = if g + 1 < new_long {
            4 * g..4 * g + 4
        } else if g >= new_long {
            4 * new_long + 2 * (g - new_long)..4 * new_long + 2 * (g - new_long) + 2
        } else {
            4 * g + 2..4 * g + 4
        };
        out[range].fill(0);
    }
    Ok((out, new_long))
}

/// The index of each pinned axis in `fvar`.
fn axis_indices(fvar: Option<&[u8]>, tags: &[u32]) -> Result<Vec<usize>, SubsetError> {
    let Some(fvar) = fvar else { return Ok(Vec::new()) };
    let axes_at = u16_at(fvar, 4)? as usize;
    let count = u16_at(fvar, 8)? as usize;
    let size = u16_at(fvar, 10)? as usize;
    let mut out = Vec::new();
    for i in 0..count {
        if tags.contains(&sfnt::u32_at(fvar, axes_at + size * i)?) {
            out.push(i);
        }
    }
    Ok(out)
}

/// [`subset`], with the bytes per table before and after.
pub fn subset_with_report(
    font: &[u8],
    keep_glyphs: &BTreeSet<u16>,
    keep_chars: &BTreeSet<char>,
    opts: &SubsetOptions,
) -> Result<(Vec<u8>, SubsetReport), SubsetError> {
    let tables = Tables::read(font)?;
    let face = ttf_parser::Face::parse(font, 0).map_err(|_| SubsetError::Malformed("the parser cannot read the font"))?;
    let need = |tag: &Tag| tables.get(tag).ok_or(SubsetError::Malformed("a required table is missing"));
    let head = need(b"head")?;
    let maxp = need(b"maxp")?;
    let (Some(glyf_table), Some(loca)) = (tables.get(b"glyf"), tables.get(b"loca")) else {
        return Err(SubsetError::Unsupported("fonts without glyf outlines"));
    };
    let num_glyphs = u16_at(maxp, 4)?;
    let long_loca = u16_at(head, 50)? != 0;
    let glyphs = glyf::split(glyf_table, loca, long_loca, num_glyphs)?;
    let mut mapping = unicode_mapping(&face)?;

    let mut keep: BTreeSet<u16> = keep_glyphs.iter().copied().filter(|&g| g < num_glyphs).collect();
    keep.insert(0);
    keep.extend(keep_chars.iter().filter_map(|&c| mapping.get(&(c as u32)).copied()));
    glyf::component_closure(&glyphs, &mut keep)?;
    mapping.retain(|_, g| *g != 0 && keep.contains(g));

    let dropped = |tag: &Tag| {
        DROPPED.contains(&tag)
            || (opts.strip_hinting && HINTING.contains(&tag))
            || (tag == b"STAT" && !opts.keep_stat)
            || (opts.drop_aat && AAT.contains(&tag))
    };
    let layout = if opts.prune_layout {
        ot_layout::prune(tables.get(b"GSUB"), tables.get(b"GPOS"), tables.get(b"GDEF"), &keep).ok()
    } else {
        None
    };
    let has_layout = ["GSUB", "GPOS", "GDEF"].iter().any(|t| tables.get(t.as_bytes().try_into().unwrap()).is_some());
    let count = match &layout {
        Some(layout)
            if opts.trim_glyphs && tables.tables.iter().all(|(t, _)| dropped(t) || TRIM_SAFE.contains(&t)) =>
        {
            keep.last().copied().unwrap_or(0).max(layout.max_output) + 1
        }
        None if opts.trim_glyphs && opts.prune_layout && !has_layout && tables.tables.iter().all(|(t, _)| dropped(t) || TRIM_SAFE.contains(&t)) => {
            keep.last().copied().unwrap_or(0) + 1
        }
        _ => num_glyphs,
    };
    let pinned = axis_indices(tables.get(b"fvar"), &opts.pin_axes)?;

    let (new_glyf, new_loca, long) = glyf::build(&glyphs[..count as usize], &keep, opts.strip_hinting)?;
    let mut long_metrics = BTreeMap::new();
    let mut out: Vec<(Tag, Vec<u8>)> = Vec::new();
    for &(tag, data) in &tables.tables {
        let rebuilt = match &tag {
            t if dropped(t) => None,
            b"glyf" => Some(new_glyf.clone()),
            b"loca" => Some(new_loca.clone()),
            b"gvar" => Some(gvar::build(data, &keep, num_glyphs, count, &pinned)?),
            b"cmap" => Some(cmap::build(&mapping)),
            b"GSUB" => Some(layout.as_ref().and_then(|l| l.gsub.clone()).unwrap_or_else(|| data.to_vec())),
            b"GPOS" => match layout.as_ref().and_then(|l| l.gpos.clone()) {
                Some(gpos) => Some(gpos),
                None if opts.prune_layout => Some(layout::prune_gpos(data, &keep)?),
                None => Some(data.to_vec()),
            },
            b"GDEF" => Some(layout.as_ref().and_then(|l| l.gdef.clone()).unwrap_or_else(|| data.to_vec())),
            b"kern" if opts.prune_layout => Some(ot_layout::prune_kern(data, &keep)?),
            // The original stays valid for a cut glyph count (its maps and
            // rows past the count are never read): keep whichever is smaller.
            b"HVAR" if opts.prune_layout => Some(match var_store::hvar(data, &keep, count) {
                Ok(hvar) if hvar.len() < data.len() => hvar,
                _ => data.to_vec(),
            }),
            b"head" => {
                let mut head = data.to_vec();
                put_u16(&mut head, 50, long as u16);
                Some(head)
            }
            b"maxp" => {
                let mut maxp = data.to_vec();
                put_u16(&mut maxp, 4, count);
                // Version 1.0 counts instruction bytes; none are left.
                if opts.strip_hinting && maxp.len() >= 32 && u16_at(&maxp, 0)? == 1 {
                    put_u16(&mut maxp, 26, 0);
                }
                Some(maxp)
            }
            b"hmtx" | b"vmtx" => {
                let header = need(if &tag == b"hmtx" { b"hhea" } else { b"vhea" })?;
                let (table, long) = metrics(data, u16_at(header, 34)? as usize, &keep, count as usize)?;
                long_metrics.insert(if &tag == b"hmtx" { *b"hhea" } else { *b"vhea" }, long);
                Some(table)
            }
            b"post" => {
                // Version 3: no glyph names.
                let mut post = data
                    .get(..32)
                    .ok_or(SubsetError::Malformed("post table truncated"))?
                    .to_vec();
                post[..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
                Some(post)
            }
            b"OS/2" => {
                let mut os2 = data.to_vec();
                let bmp = mapping.keys().map(|&c| c.min(0xFFFF) as u16);
                if let (Some(first), Some(last)) = (bmp.clone().min(), bmp.max()) {
                    if os2.len() >= 68 {
                        put_u16(&mut os2, 64, first);
                        put_u16(&mut os2, 66, last);
                    }
                }
                Some(os2)
            }
            b"name" => match &opts.rename {
                Some(rename) => Some(name::rename(data, rename)?),
                None => Some(data.to_vec()),
            },
            _ => Some(data.to_vec()),
        };
        if let Some(rebuilt) = rebuilt {
            out.push((tag, rebuilt));
        }
    }
    for (tag, data) in &mut out {
        if let Some(&long) = long_metrics.get(tag) {
            put_u16(data, 34, long as u16);
        }
    }

    let mut report = SubsetReport {
        font_before: font.len(),
        kept: keep.clone(),
        glyphs_total: num_glyphs as usize,
        glyphs_out: count as usize,
        ..Default::default()
    };
    for &(tag, data) in &tables.tables {
        report.tables.push(TableSize {
            tag: String::from_utf8_lossy(&tag).into_owned(),
            before: data.len(),
            after: out.iter().find(|(t, _)| *t == tag).map_or(0, |(_, d)| d.len()),
        });
    }
    report.tables.sort_by(|a, b| b.before.cmp(&a.before));
    let font = sfnt::write(tables.version, out);
    report.font_after = font.len();
    Ok((font, report))
}

#[cfg(test)]
mod tests;
