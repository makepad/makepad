//! Minimal WOFF2 → sfnt decompressor.
//!
//! WOFF2 (W3C, <https://www.w3.org/TR/WOFF2/>) is a web font container: the sfnt
//! table data is concatenated and compressed as a single brotli stream, and the
//! `glyf`/`loca` tables are stored in a reversible "transformed" form to shrink
//! them further. `ttf_parser` only understands sfnt, so before parsing a font we
//! detect the `wOF2` signature and reconstruct the original sfnt bytes here.
//!
//! Scope: we support the null transform (all tables except glyf/loca) and the
//! glyf/loca transform version 0 — the two forms every real-world WOFF2 file
//! produced by `woff2_compress` / fontTools uses. WOFF2 collections (`ttcf`
//! flavor) and the (rare) hmtx transform are not reconstructed; those return
//! `None` and the caller skips the font member, matching the existing
//! "unparseable member is silently skipped" behaviour.
//!
//! Every read is bounds-checked; any malformed input yields `None` rather than
//! panicking.

use std::io::Read;

/// The 255UInt16 base value used by the glyf transform's nContour/nPoints
/// streams (WOFF2 §5.1).
const LOWEST_U_CODE: u16 = 253;
const ONE_MORE_BYTE_CODE1: u8 = 255;
const ONE_MORE_BYTE_CODE2: u8 = 254;
const WORD_CODE: u8 = 253;

/// Decompress a WOFF2 file into the equivalent sfnt (TTF/OTF) bytes.
///
/// Returns `None` if `data` is not a WOFF2 file, uses an unsupported feature
/// (collections, unknown transform), or is malformed.
pub fn decompress(data: &[u8]) -> Option<Vec<u8>> {
    let mut r = Reader::new(data);

    // ── Header (WOFF2 §5) ──
    if r.read_u32()? != 0x774F_4632 {
        return None; // not 'wOF2'
    }
    let flavor = r.read_u32()?;
    if flavor == 0x7474_6366 {
        return None; // 'ttcf' collections not supported
    }
    let _length = r.read_u32()?;
    let num_tables = r.read_u16()? as usize;
    let _reserved = r.read_u16()?;
    let total_sfnt_size = r.read_u32()? as usize;
    let total_compressed_size = r.read_u32()? as usize;
    let _major = r.read_u16()?;
    let _minor = r.read_u16()?;
    let _meta_offset = r.read_u32()?;
    let _meta_length = r.read_u32()?;
    let _meta_orig_length = r.read_u32()?;
    let _priv_offset = r.read_u32()?;
    let _priv_length = r.read_u32()?;

    if num_tables == 0 {
        return None;
    }

    // ── Table directory (WOFF2 §5.2) ──
    let mut entries = Vec::with_capacity(num_tables);
    for _ in 0..num_tables {
        entries.push(read_table_entry(&mut r)?);
    }

    // ── Compressed font data: one brotli stream over all table data ──
    let compressed = r.read_bytes(total_compressed_size)?;
    let font_data = brotli_decompress(compressed)?;

    // Slice the decompressed stream into per-table byte ranges, in directory
    // order (the stream stores tables back-to-back with no padding).
    let mut cursor = 0usize;
    for e in &mut entries {
        let len = e.transform_length.unwrap_or(e.orig_length) as usize;
        let end = cursor.checked_add(len)?;
        if end > font_data.len() {
            return None;
        }
        e.data_range = (cursor, end);
        cursor = end;
    }

    reconstruct_sfnt(flavor, &entries, &font_data, total_sfnt_size)
}

/// One entry of the WOFF2 table directory, plus (after slicing) the byte range
/// of its data within the decompressed stream.
struct TableEntry {
    tag: u32,
    orig_length: u32,
    /// Present iff the table was transformed (glyf/loca): the length of the
    /// *transformed* bytes stored in the stream.
    transform_length: Option<u32>,
    /// Transform version from the entry flags (0..=3).
    transform_version: u8,
    /// (start, end) into the decompressed font data. Filled in after slicing.
    data_range: (usize, usize),
}

/// Known table tags in flag-index order (WOFF2 §5.2, Table 6). Index 63 means
/// "arbitrary tag follows as 4 bytes".
const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

fn read_table_entry(r: &mut Reader) -> Option<TableEntry> {
    let flags = r.read_u8()?;
    let tag_index = (flags & 0x3F) as usize;
    let transform_version = (flags >> 6) & 0x03;

    let tag = if tag_index == 0x3F {
        r.read_u32()? // arbitrary tag as 4 raw bytes (already big-endian u32)
    } else {
        let t = KNOWN_TAGS.get(tag_index)?;
        u32::from_be_bytes(**t)
    };

    let orig_length = r.read_base128()?;

    // Per §5.2: a transform is applied when the version is "non-null" for that
    // table. For glyf/loca the null (untransformed) transform is version 3;
    // for all other tables the null transform is version 0. Only when a table
    // is actually transformed does a transformLength follow.
    let is_glyf = tag == u32::from_be_bytes(*b"glyf");
    let is_loca = tag == u32::from_be_bytes(*b"loca");
    let transformed = if is_glyf || is_loca {
        transform_version != 3
    } else {
        transform_version != 0
    };
    let transform_length = if transformed { Some(r.read_base128()?) } else { None };

    Some(TableEntry {
        tag,
        orig_length,
        transform_length,
        transform_version,
        data_range: (0, 0),
    })
}

/// Rebuild the sfnt: offset table + table records (4-byte aligned, with real
/// checksums left as-is / zeroed) + table data. glyf/loca get reconstructed
/// from their transformed form first.
fn reconstruct_sfnt(
    flavor: u32,
    entries: &[TableEntry],
    font_data: &[u8],
    total_sfnt_size: usize,
) -> Option<Vec<u8>> {
    // First materialise each table's final (untransformed) bytes.
    struct OutTable {
        tag: u32,
        bytes: Vec<u8>,
    }
    let mut tables: Vec<OutTable> = Vec::with_capacity(entries.len());

    // glyf and loca are reconstructed together; remember the loca format the
    // glyf transform decides so we can emit the matching loca table.
    let glyf_idx = entries
        .iter()
        .position(|e| e.tag == u32::from_be_bytes(*b"glyf"));

    for (i, e) in entries.iter().enumerate() {
        let (start, end) = e.data_range;
        let raw = font_data.get(start..end)?;
        let is_glyf = e.tag == u32::from_be_bytes(*b"glyf");
        let is_loca = e.tag == u32::from_be_bytes(*b"loca");

        let bytes = if is_glyf && e.transform_length.is_some() {
            // Reconstruct glyf (and stash loca for the loca entry below).
            let (glyf, _loca) = reconstruct_glyf(raw)?;
            glyf
        } else if is_loca && e.transform_length.is_some() {
            // loca is emitted from the glyf reconstruction; find glyf's raw and
            // regenerate. (loca's own transformed length is 0.)
            let gi = glyf_idx?;
            let ge = &entries[gi];
            let (gs, gend) = ge.data_range;
            let graw = font_data.get(gs..gend)?;
            let (_glyf, loca) = reconstruct_glyf(graw)?;
            loca
        } else {
            // Untransformed table: bytes are already the original.
            if e.transform_version != 0 && !is_glyf && !is_loca {
                // Unknown transform on a non-glyf/loca table — unsupported.
                return None;
            }
            raw.to_vec()
        };
        let _ = i;
        tables.push(OutTable { tag: e.tag, bytes });
    }

    // Build the offset table. Table records must be sorted by tag.
    let mut order: Vec<usize> = (0..tables.len()).collect();
    order.sort_by_key(|&i| tables[i].tag);

    let num_tables = tables.len() as u16;
    let mut search_range = 1u16;
    let mut entry_selector = 0u16;
    while (search_range as u32) * 2 <= num_tables as u32 {
        search_range *= 2;
        entry_selector += 1;
    }
    search_range *= 16;
    let range_shift = num_tables.wrapping_mul(16).wrapping_sub(search_range);

    let header_len = 12 + tables.len() * 16;
    let mut out = Vec::with_capacity(total_sfnt_size.max(header_len));

    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&num_tables.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    // Compute each table's aligned offset.
    let mut offset = header_len;
    let mut offsets = vec![0usize; tables.len()];
    for &i in &order {
        offsets[i] = offset;
        let padded = (tables[i].bytes.len() + 3) & !3;
        offset += padded;
    }

    // Emit table records in sorted order.
    for &i in &order {
        let t = &tables[i];
        out.extend_from_slice(&t.tag.to_be_bytes());
        out.extend_from_slice(&table_checksum(&t.bytes).to_be_bytes());
        out.extend_from_slice(&(offsets[i] as u32).to_be_bytes());
        out.extend_from_slice(&(t.bytes.len() as u32).to_be_bytes());
    }

    // Emit table data in the same offset order, 4-byte padded.
    for &i in &order {
        debug_assert_eq!(out.len(), offsets[i]);
        out.extend_from_slice(&tables[i].bytes);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }

    Some(out)
}

/// Reconstruct the `glyf` and `loca` tables from the WOFF2 transformed glyf
/// table (transform version 0, WOFF2 §5.1).
///
/// Returns `(glyf_bytes, loca_bytes)`. The loca is emitted in the format
/// (short/long) the transform header specifies.
fn reconstruct_glyf(data: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut r = Reader::new(data);
    let _reserved = r.read_u16()?;
    let option_flags = r.read_u16()?;
    let num_glyphs = r.read_u16()? as usize;
    let index_format = r.read_u16()?; // 0 = short loca, 1 = long loca

    let n_contour_stream_size = r.read_u32()? as usize;
    let n_points_stream_size = r.read_u32()? as usize;
    let flag_stream_size = r.read_u32()? as usize;
    let glyph_stream_size = r.read_u32()? as usize;
    let composite_stream_size = r.read_u32()? as usize;
    let bbox_stream_size = r.read_u32()? as usize;
    let instruction_stream_size = r.read_u32()? as usize;

    // The bbox stream begins with a bitmap (one bit per glyph) selecting which
    // glyphs carry an explicit bbox; the rest of the stream is the bbox values.
    let bbox_bitmap_bytes = (num_glyphs + 7) / 8;

    let n_contour = r.read_bytes(n_contour_stream_size)?;
    let n_points = r.read_bytes(n_points_stream_size)?;
    let flags = r.read_bytes(flag_stream_size)?;
    let glyphs = r.read_bytes(glyph_stream_size)?;
    let composites = r.read_bytes(composite_stream_size)?;
    let bbox = r.read_bytes(bbox_stream_size)?;
    let instructions = r.read_bytes(instruction_stream_size)?;

    if bbox.len() < bbox_bitmap_bytes {
        return None;
    }
    let bbox_bitmap = &bbox[..bbox_bitmap_bytes];
    let mut bbox_data = SubReader::new(&bbox[bbox_bitmap_bytes..]);

    let mut nc = SubReader::new(n_contour);
    let mut np = SubReader::new(n_points);
    let mut fl = SubReader::new(flags);
    let mut gl = SubReader::new(glyphs);
    let mut comp = SubReader::new(composites);
    let mut instr = SubReader::new(instructions);

    let _ = option_flags;

    let mut glyf = Vec::new();
    let mut loca_offsets = Vec::with_capacity(num_glyphs + 1);

    for gid in 0..num_glyphs {
        loca_offsets.push(glyf.len() as u32);
        let number_of_contours = nc.read_i16()?;

        if number_of_contours == 0 {
            // Empty glyph: no data.
            continue;
        }

        if number_of_contours < 0 {
            // Composite glyph: copy from the composite stream verbatim until the
            // MORE_COMPONENTS flag clears, then optional instructions.
            let start = glyf.len();
            glyf.extend_from_slice(&(number_of_contours as i16).to_be_bytes());
            // bbox
            let has_bbox = bbox_bit(bbox_bitmap, gid);
            if !has_bbox {
                return None; // composite glyphs must have an explicit bbox
            }
            write_bbox(&mut glyf, &mut bbox_data)?;

            let mut have_instructions = false;
            loop {
                let flags_word = comp.read_u16()?;
                glyf.extend_from_slice(&flags_word.to_be_bytes());
                const ARG_1_AND_2_ARE_WORDS: u16 = 0x0001;
                const WE_HAVE_A_SCALE: u16 = 0x0008;
                const MORE_COMPONENTS: u16 = 0x0020;
                const WE_HAVE_AN_X_AND_Y_SCALE: u16 = 0x0040;
                const WE_HAVE_A_TWO_BY_TWO: u16 = 0x0080;
                const WE_HAVE_INSTRUCTIONS: u16 = 0x0100;

                let glyph_index = comp.read_u16()?;
                glyf.extend_from_slice(&glyph_index.to_be_bytes());

                let arg_bytes = if flags_word & ARG_1_AND_2_ARE_WORDS != 0 { 4 } else { 2 };
                let args = comp.read_bytes(arg_bytes)?;
                glyf.extend_from_slice(args);

                let scale_bytes = if flags_word & WE_HAVE_A_SCALE != 0 {
                    2
                } else if flags_word & WE_HAVE_AN_X_AND_Y_SCALE != 0 {
                    4
                } else if flags_word & WE_HAVE_A_TWO_BY_TWO != 0 {
                    8
                } else {
                    0
                };
                if scale_bytes > 0 {
                    let s = comp.read_bytes(scale_bytes)?;
                    glyf.extend_from_slice(s);
                }

                if flags_word & WE_HAVE_INSTRUCTIONS != 0 {
                    have_instructions = true;
                }
                if flags_word & MORE_COMPONENTS == 0 {
                    break;
                }
            }
            if have_instructions {
                let instr_len = read_255u16(&mut gl)? as usize;
                let instr_bytes = instr.read_bytes(instr_len)?;
                glyf.extend_from_slice(&(instr_len as u16).to_be_bytes());
                glyf.extend_from_slice(instr_bytes);
            }
            pad_glyph(&mut glyf, start);
            continue;
        }

        // Simple glyph. WOFF2 stream read order (§5.2): nPoints per contour
        // (nPointStream) → point flags (flagStream) → coordinate triplets
        // (glyphStream) → instructionLength (glyphStream) → instructions
        // (instructionStream). Only after reading everything do we assemble the
        // TrueType glyf entry, whose on-disk order is different (endPts,
        // instructionLength, instructions, flags, xCoords, yCoords).
        let start = glyf.len();
        let n_contours = number_of_contours as usize;

        // Per-contour endpoints: read nPoints for each contour.
        let mut end_pts = Vec::with_capacity(n_contours);
        let mut total_points = 0usize;
        for _ in 0..n_contours {
            let p = read_255u16(&mut np)? as usize;
            total_points += p;
            end_pts.push(total_points as u16 - 1);
        }

        // Decode flags + coordinates (flagStream + glyphStream) into a standard
        // TrueType flags/xCoords/yCoords blob, plus the computed bbox.
        let mut coord_blob = Vec::new();
        let (min_x, min_y, max_x, max_y) =
            decode_simple_glyph(&mut fl, &mut gl, total_points, &mut coord_blob)?;

        // instructionLength + instructions come AFTER the coordinates in the
        // glyph stream.
        let instr_len = read_255u16(&mut gl)? as usize;
        let instr_bytes = instr.read_bytes(instr_len)?;

        // Assemble the TrueType simple glyph.
        glyf.extend_from_slice(&(number_of_contours as i16).to_be_bytes());
        let bbox_pos = glyf.len();
        glyf.extend_from_slice(&[0u8; 8]); // bbox filled below
        for ep in &end_pts {
            glyf.extend_from_slice(&ep.to_be_bytes());
        }
        glyf.extend_from_slice(&(instr_len as u16).to_be_bytes());
        glyf.extend_from_slice(instr_bytes);
        glyf.extend_from_slice(&coord_blob);

        // Fill bbox: explicit from the bbox stream if present, else computed.
        let has_bbox = bbox_bit(bbox_bitmap, gid);
        if has_bbox {
            write_bbox_at(&mut glyf, bbox_pos, &mut bbox_data)?;
        } else {
            glyf[bbox_pos..bbox_pos + 2].copy_from_slice(&min_x.to_be_bytes());
            glyf[bbox_pos + 2..bbox_pos + 4].copy_from_slice(&min_y.to_be_bytes());
            glyf[bbox_pos + 4..bbox_pos + 6].copy_from_slice(&max_x.to_be_bytes());
            glyf[bbox_pos + 6..bbox_pos + 8].copy_from_slice(&max_y.to_be_bytes());
        }

        pad_glyph(&mut glyf, start);
    }
    loca_offsets.push(glyf.len() as u32);

    // Emit loca in the requested index format.
    let mut loca = Vec::new();
    if index_format == 0 {
        for off in &loca_offsets {
            // short loca stores offset/2.
            loca.extend_from_slice(&((off / 2) as u16).to_be_bytes());
        }
    } else {
        for off in &loca_offsets {
            loca.extend_from_slice(&off.to_be_bytes());
        }
    }

    Some((glyf, loca))
}

/// Decode a WOFF2 simple-glyph flag + coordinate run into standard TrueType
/// flags/xCoordinates/yCoordinates appended to `out`. Returns the glyph bbox.
fn decode_simple_glyph(
    fl: &mut SubReader,
    gl: &mut SubReader,
    total_points: usize,
    out: &mut Vec<u8>,
) -> Option<(i16, i16, i16, i16)> {
    // WOFF2 triplet encoding (§5.2 "the glyf table transformation"): each point
    // has a flag byte in the flag stream (bit 7 = on-curve inverse) and a
    // variable-length (x,y) delta in the glyph stream selected by the low 7 bits.
    let mut xs = Vec::with_capacity(total_points);
    let mut ys = Vec::with_capacity(total_points);
    let mut on_curve = Vec::with_capacity(total_points);

    let mut x = 0i32;
    let mut y = 0i32;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);

    for _ in 0..total_points {
        let flag = fl.read_u8()?;
        let on = (flag & 0x80) == 0;
        let triplet = flag & 0x7F;
        let (dx, dy) = read_triplet(gl, triplet)?;
        x += dx;
        y += dy;
        on_curve.push(on);
        xs.push(dx);
        ys.push(dy);
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    if total_points == 0 {
        min_x = 0;
        min_y = 0;
        max_x = 0;
        max_y = 0;
    }

    // Encode standard glyph flags with no compression (each point one flag byte,
    // x/y as SHORT with sign bit). This is valid TrueType and keeps the encoder
    // simple; ttf_parser reads it fine.
    // Flags: bit0 ON_CURVE, bit1 X_SHORT, bit2 Y_SHORT, bit4 X_SAME/pos,
    // bit5 Y_SAME/pos.
    for i in 0..total_points {
        let mut f = 0u8;
        if on_curve[i] {
            f |= 0x01;
        }
        let dx = xs[i];
        let dy = ys[i];
        // X
        if dx == 0 {
            f |= 0x10; // X_SAME (no delta)
        } else if (-255..=255).contains(&dx) {
            f |= 0x02; // X_SHORT
            if dx > 0 {
                f |= 0x10; // positive sign
            }
        }
        // Y
        if dy == 0 {
            f |= 0x20; // Y_SAME
        } else if (-255..=255).contains(&dy) {
            f |= 0x04; // Y_SHORT
            if dy > 0 {
                f |= 0x20;
            }
        }
        out.push(f);
    }
    // X coordinates.
    for i in 0..total_points {
        let dx = xs[i];
        if dx == 0 {
            // nothing (X_SAME)
        } else if (-255..=255).contains(&dx) {
            out.push(dx.unsigned_abs() as u8);
        } else {
            out.extend_from_slice(&(dx as i16).to_be_bytes());
        }
    }
    // Y coordinates.
    for i in 0..total_points {
        let dy = ys[i];
        if dy == 0 {
        } else if (-255..=255).contains(&dy) {
            out.push(dy.unsigned_abs() as u8);
        } else {
            out.extend_from_slice(&(dy as i16).to_be_bytes());
        }
    }

    Some((
        clamp_i16(min_x),
        clamp_i16(min_y),
        clamp_i16(max_x),
        clamp_i16(max_y),
    ))
}

fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// WOFF2 triplet coordinate decoding (§5.2, Table "triplet encoding").
///
/// Transcribed from the reference decoder (Google woff2 `TripletDecode`):
///
/// * The **magnitude** high bits come from the *flag byte itself* (bits 4-5 for
///   dx, bits 2-3 for dy in the 1-byte group), not from a group-relative index.
/// * The **sign** for dx is `flag` bit 0, for dy is `flag` bit 1, where a set
///   bit selects a *positive* delta (`with_sign(flag, mag) = (flag&1) ? mag :
///   -mag`; dy uses `flag >> 1`).
fn read_triplet(gl: &mut SubReader, flag: u8) -> Option<(i32, i32)> {
    let f = flag as i32;
    // `with_sign(flag, mag)`: low bit of `flag` set => positive, else negative.
    fn with_sign(flag: i32, mag: i32) -> i32 {
        if flag & 1 != 0 { mag } else { -mag }
    }
    let (dx, dy) = if f < 10 {
        let b0 = gl.read_u8()? as i32;
        (0, with_sign(f, ((f & 14) << 7) + b0))
    } else if f < 20 {
        let b0 = gl.read_u8()? as i32;
        (with_sign(f, (((f - 10) & 14) << 7) + b0), 0)
    } else if f < 84 {
        // 1 data byte. dx high bits = flag bits 4-5; dy high bits = flag bits
        // 2-3. `b0` here is the flag byte; `b1` is the single data byte.
        let b0 = f;
        let b1 = gl.read_u8()? as i32;
        (
            with_sign(f, 1 + (b0 & 0x30) + (b1 >> 4)),
            with_sign(f >> 1, 1 + ((b0 & 0x0c) << 2) + (b1 & 0x0f)),
        )
    } else if f < 120 {
        let b0 = gl.read_u8()? as i32;
        let b1 = gl.read_u8()? as i32;
        let g = f - 84;
        (
            with_sign(f, 1 + ((g / 12) << 8) + b0),
            with_sign(f >> 1, 1 + (((g % 12) >> 2) << 8) + b1),
        )
    } else if f < 124 {
        let b0 = gl.read_u8()? as i32;
        let b1 = gl.read_u8()? as i32;
        let b2 = gl.read_u8()? as i32;
        (
            with_sign(f, (b0 << 4) + (b1 >> 4)),
            with_sign(f >> 1, ((b1 & 0x0f) << 8) + b2),
        )
    } else {
        let b0 = gl.read_u8()? as i32;
        let b1 = gl.read_u8()? as i32;
        let b2 = gl.read_u8()? as i32;
        let b3 = gl.read_u8()? as i32;
        (
            with_sign(f, (b0 << 8) + b1),
            with_sign(f >> 1, (b2 << 8) + b3),
        )
    };
    Some((dx, dy))
}

fn bbox_bit(bitmap: &[u8], gid: usize) -> bool {
    let byte = gid / 8;
    let bit = 7 - (gid % 8);
    bitmap.get(byte).map(|b| (b >> bit) & 1 == 1).unwrap_or(false)
}

fn write_bbox(out: &mut Vec<u8>, bbox: &mut SubReader) -> Option<()> {
    let b = bbox.read_bytes(8)?;
    out.extend_from_slice(b);
    Some(())
}

fn write_bbox_at(out: &mut [u8], pos: usize, bbox: &mut SubReader) -> Option<()> {
    let b = bbox.read_bytes(8)?;
    out.get_mut(pos..pos + 8)?.copy_from_slice(b);
    Some(())
}

fn pad_glyph(glyf: &mut Vec<u8>, start: usize) {
    // glyf entries are padded to a 2-byte boundary within the table.
    let len = glyf.len() - start;
    if len % 2 != 0 {
        glyf.push(0);
    }
}

/// Read a 255UInt16 (WOFF2 §5.1).
fn read_255u16(r: &mut SubReader) -> Option<u16> {
    let code = r.read_u8()?;
    if code == WORD_CODE {
        let hi = r.read_u8()? as u16;
        let lo = r.read_u8()? as u16;
        Some((hi << 8) | lo)
    } else if code == ONE_MORE_BYTE_CODE1 {
        Some(r.read_u8()? as u16 + (LOWEST_U_CODE * 2))
    } else if code == ONE_MORE_BYTE_CODE2 {
        Some(r.read_u8()? as u16 + LOWEST_U_CODE)
    } else {
        Some(code as u16)
    }
}

/// TrueType table checksum: sum of big-endian u32 words (zero-padded).
fn table_checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    let mut i = 0;
    while i < data.len() {
        let mut word = [0u8; 4];
        let n = (data.len() - i).min(4);
        word[..n].copy_from_slice(&data[i..i + n]);
        sum = sum.wrapping_add(u32::from_be_bytes(word));
        i += 4;
    }
    sum
}

/// Decompress a raw brotli stream to a `Vec<u8>`.
fn brotli_decompress(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut dec = brotli_decompressor::Decompressor::new(input, 4096);
    dec.read_to_end(&mut out).ok()?;
    Some(out)
}

/// Big-endian cursor over a byte slice with bounds checking.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn read_u8(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn read_u16(&mut self) -> Option<u16> {
        let b = self.data.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    fn read_u32(&mut self) -> Option<u32> {
        let b = self.data.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn read_bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let b = self.data.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(b)
    }
    /// UIntBase128 (WOFF2 §4.2): up to 5 bytes, 7 bits each, big-endian.
    fn read_base128(&mut self) -> Option<u32> {
        let mut result: u32 = 0;
        for i in 0..5 {
            let b = self.read_u8()?;
            // No leading zeros allowed and no overflow.
            if i == 0 && b == 0x80 {
                return None;
            }
            if result & 0xFE00_0000 != 0 {
                return None; // would overflow on shift
            }
            result = (result << 7) | (b & 0x7F) as u32;
            if b & 0x80 == 0 {
                return Some(result);
            }
        }
        None
    }
}

/// Like `Reader` but returns owned copies-free sub-slices; used for the many
/// parallel glyf sub-streams.
struct SubReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SubReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn read_u8(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn read_u16(&mut self) -> Option<u16> {
        let b = self.data.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    fn read_i16(&mut self) -> Option<i16> {
        self.read_u16().map(|v| v as i16)
    }
    fn read_bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let b = self.data.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real Noto Sans (latin subset) WOFF2, TrueType flavor with a
    /// transformed glyf/loca. Decompressing it must yield sfnt bytes that
    /// `ttf_parser` accepts, with a plausible glyph count and a usable cmap.
    #[test]
    fn decompress_real_woff2_parses_as_sfnt() {
        let woff2 = include_bytes!("test_data/noto-sans-latin-400.woff2");
        assert_eq!(&woff2[0..4], b"wOF2", "fixture must be a WOFF2 file");

        let sfnt = decompress(woff2).expect("woff2 decompresses to sfnt");

        // sfnt magic: TrueType outlines => 0x00010000.
        assert_eq!(&sfnt[0..4], &[0x00, 0x01, 0x00, 0x00]);

        use rustybuzz::ttf_parser;
        let face = ttf_parser::Face::parse(&sfnt, 0).expect("decompressed sfnt parses");
        assert!(face.number_of_glyphs() > 100, "latin subset has many glyphs");

        // The 'A' glyph must resolve and have a non-empty outline bbox —
        // exercises the glyf/loca transform reconstruction.
        let gid = face.glyph_index('A').expect("cmap maps 'A'");
        let bbox = face.glyph_bounding_box(gid).expect("'A' has an outline");
        assert!(bbox.width() > 0 && bbox.height() > 0);

        // Walk the actual outline segments — a nonzero bbox with an empty
        // segment list would still rasterise as tofu.
        struct Counter(usize);
        impl ttf_parser::OutlineBuilder for Counter {
            fn move_to(&mut self, _: f32, _: f32) { self.0 += 1; }
            fn line_to(&mut self, _: f32, _: f32) { self.0 += 1; }
            fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) { self.0 += 1; }
            fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) { self.0 += 1; }
            fn close(&mut self) {}
        }
        let mut c = Counter(0);
        face.outline_glyph(gid, &mut c);
        assert!(c.0 > 2, "'A' outline must have real segments, got {}", c.0);

        // Shape a latin word through rustybuzz — the *exact* path the text
        // engine uses. Every glyph must resolve to a non-.notdef id.
        let rb = rustybuzz::Face::from_face(face.clone());
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str("Hello");
        let out = rustybuzz::shape(&rb, &[], buf);
        assert_eq!(out.len(), 5, "5 latin glyphs shaped");
        for info in out.glyph_infos() {
            assert_ne!(info.glyph_id, 0, "no glyph should shape to .notdef");
        }
    }

    /// The exact font bytes the `web_font` example fetches from the CDN
    /// (a newer @fontsource build than the fixture above). Reproduces the
    /// runtime path: decompress → shape "Hello" → every glyph must be real.
    #[test]
    fn decompress_cdn_woff2_shapes_latin() {
        let woff2 = include_bytes!("test_data/cdn-latin-400.woff2");
        let sfnt = decompress(woff2).expect("cdn woff2 decompresses to sfnt");
        use rustybuzz::ttf_parser;
        let face = ttf_parser::Face::parse(&sfnt, 0).expect("cdn sfnt parses");
        let gid = face.glyph_index('H').expect("cmap maps 'H'");
        let bbox = face.glyph_bounding_box(gid).expect("'H' has an outline");
        // 'H' must sit in the normal upper-right em quadrant, not mirrored into
        // negative space — the symptom of inverted triplet-delta signs, which
        // rasterises as tofu even though the glyph shapes to a real (non-.notdef)
        // id. The older fixture only checked width/height, which is
        // sign-invariant, so it masked this bug.
        assert!(
            bbox.x_min >= 0 && bbox.y_min >= 0,
            "'H' must be in positive space, got {:?}",
            bbox
        );
        assert!(bbox.width() > 0 && bbox.height() > 0);

        // Shape "Hello" through rustybuzz — the exact path the text engine uses.
        let rb = rustybuzz::Face::from_face(face.clone());
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str("Hello");
        let out = rustybuzz::shape(&rb, &[], buf);
        for info in out.glyph_infos() {
            assert_ne!(info.glyph_id, 0, "'Hello' shaped a .notdef glyph");
        }

        // Every letter of "Hello" (not just 'H') must reconstruct into a sane
        // em-box. The larger triplet groups (2-4 byte deltas) went through a
        // different code path than 'H'/'l', so 'e' caught a bug the single-glyph
        // check missed: verify each glyph's outline stays within plausible font
        // units (roughly [-em, 2*em]) instead of shooting off to -1932 etc.
        let em = face.units_per_em() as i16;
        for ch in "Hello".chars() {
            let gid = face.glyph_index(ch).expect("cmap maps letter");
            let bb = face.glyph_bounding_box(gid).expect("letter has an outline");
            assert!(
                bb.x_min >= -em && bb.y_min >= -em && bb.x_max <= 2 * em && bb.y_max <= 2 * em,
                "'{}' reconstructed to an implausible bbox {:?} (em={})",
                ch, bb, em
            );
        }
    }

    #[test]
    fn non_woff2_input_is_rejected() {
        assert!(decompress(b"\x00\x01\x00\x00rest-is-not-woff2").is_none());
        assert!(decompress(b"OTTO").is_none());
        assert!(decompress(&[]).is_none());
    }
}
