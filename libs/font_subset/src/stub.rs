//! A stub for a face nothing draws from: one glyph (`.notdef`, its outline
//! at the default instance), no characters, no layout or variation tables;
//! the metrics tables (`head`, `hhea`, `OS/2`) are the original's, so a
//! family that falls back from it lays lines out with the same height.

use crate::{
    cmap, glyf, name,
    sfnt::{put_u16, u16_at, Tables},
    SubsetError, SubsetOptions,
};

/// Name ids a stub keeps: copyright, family, subfamily, licence and its URL
/// (the notices the font's licence asks every copy to carry).
const NOTICES: [u16; 5] = [0, 1, 2, 13, 14];

/// The stub of `font` (renamed when `opts.rename` says so; the other
/// options do not apply).
pub fn stub(font: &[u8], opts: &SubsetOptions) -> Result<Vec<u8>, SubsetError> {
    let tables = Tables::read(font)?;
    let need = |tag: &[u8; 4]| tables.get(tag).ok_or(SubsetError::Malformed("a required table is missing"));
    let (head, hhea, maxp, hmtx) = (need(b"head")?, need(b"hhea")?, need(b"maxp")?, need(b"hmtx")?);
    let num_glyphs = u16_at(maxp, 4)?;
    let mut notdef = Vec::new();
    if let (Some(glyf_table), Some(loca)) = (tables.get(b"glyf"), tables.get(b"loca")) {
        let glyphs = glyf::split(glyf_table, loca, u16_at(head, 50)? != 0, num_glyphs)?;
        let g = glyphs.first().copied().unwrap_or(&[]);
        // A composite `.notdef` would need its components: draw it empty.
        if g.len() >= 10 && i16::from_be_bytes([g[0], g[1]]) >= 0 {
            notdef = glyf::build(&[g], &[0].into(), true)?.0;
        }
    }
    let mut out: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    let mut head = head.to_vec();
    put_u16(&mut head, 50, 0);
    out.push((*b"head", head));
    let mut hhea = hhea.to_vec();
    put_u16(&mut hhea, 34, 1);
    out.push((*b"hhea", hhea));
    let mut maxp = maxp.to_vec();
    put_u16(&mut maxp, 4, 1);
    if maxp.len() >= 32 {
        put_u16(&mut maxp, 26, 0);
    }
    out.push((*b"maxp", maxp));
    out.push((*b"hmtx", hmtx.get(..4).ok_or(SubsetError::Malformed("hmtx truncated"))?.to_vec()));
    let mut loca = vec![0, 0];
    loca.extend_from_slice(&((notdef.len() / 2) as u16).to_be_bytes());
    out.push((*b"loca", loca));
    out.push((*b"glyf", notdef));
    out.push((*b"cmap", cmap::build(&Default::default())));
    if let Some(os2) = tables.get(b"OS/2") {
        let mut os2 = os2.to_vec();
        if os2.len() >= 68 {
            put_u16(&mut os2, 64, 0xFFFF);
            put_u16(&mut os2, 66, 0xFFFF);
        }
        out.push((*b"OS/2", os2));
    }
    if let Some(post) = tables.get(b"post") {
        let mut post = post.get(..32).ok_or(SubsetError::Malformed("post table truncated"))?.to_vec();
        post[..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
        out.push((*b"post", post));
    }
    if let Some(names) = tables.get(b"name") {
        let mut names = name::select(names, |platform, _, id| platform == 3 && NOTICES.contains(&id))?;
        if let Some(rename) = &opts.rename {
            names = name::rename(&names, rename)?;
        }
        out.push((*b"name", names));
    }
    Ok(crate::sfnt::write(tables.version, out))
}
