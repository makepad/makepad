//! Resolve the platform's default fonts at runtime via CoreText.
//!
//! Shared by the macOS / iOS / tvOS backends. We never ask for a typeface by
//! name; instead we ask CoreText for the system font for a role (and, for CJK /
//! emoji, for a font that can render a representative character). This is how we
//! avoid bundling text fonts: glyph coverage is delegated to the OS.
//!
//! Getting the actual bytes to `ttf_parser` is now unified across macOS, iOS and
//! tvOS: we never read the on-disk file. Instead we copy every sfnt table
//! straight out of the `CTFont` with `CTFontCopyTable` and reassemble a
//! self-contained single-face `.ttf` in memory (`sfnt_bytes_from_ctfont`). Color
//! emoji (`sbix`) and most scripts work because we copy all outline/bitmap tables.
//!
//! Why not read the file on macOS (where `/System/Library/Fonts/...` is
//! world-readable)? Because CoreText resolves the system UI/CJK roles to Apple's
//! proprietary *variable* fonts (`SFNS.ttf` = SF Pro, `PingFangUI.ttc`). Reading
//! those verbatim gives `ttf_parser` a `gvar`-driven outline it can't resolve, so
//! every glyph comes back empty (blank / tofu) — the reason macOS used to
//! substitute Helvetica. The in-memory reassembly instead flattens the variable
//! tables (see below), so we render the *real* system font.
//!
//! Wrinkles handled by the reassembly path:
//!   - Apple's UI font (SF Pro) is a *variable* TrueType font. `ttf_parser`
//!     routes all outlining through `gvar` when present and can't resolve
//!     Apple's `gvar` layout, so every glyph came back empty. We only consume
//!     the default instance, so `sfnt_bytes_from_ctfont` drops the variation
//!     tables (`gvar`/`fvar`/`avar`/`HVAR`/`MVAR`/`STAT`) when a `glyf` table is
//!     present, flattening to the base outlines `ttf_parser` can read.
//!   - Some fonts (and `AppleColorEmoji`) use Apple's proprietary `hvgl` outline
//!     format with no `glyf` fallback; `ttf_parser` can't render those at all, so
//!     they go through Apple's `libhvf` (`hvgl_render`, compiled on all three).
//!   - On iOS/tvOS `AppleColorEmoji`'s `sbix` bitmaps use Apple's private `emjc`
//!     format, rendered via CoreText (`color_emoji_render`). macOS bundled emoji
//!     use `png ` bitmaps `ttf_parser` already decodes, so that path stays off.
//!
//! iOS/tvOS additionally can't use file paths at all (system fonts live outside
//! the app sandbox and `kCTFontURLAttribute` returns NULL / an unreadable path);
//! the shared in-memory path sidesteps that too.

use crate::cx_api::{SystemFontQuery, SystemFontRole};
use crate::os::apple::apple_sys::*;
use std::ffi::c_void;

/// Resolve `query` to font file bytes using CoreText, or `None` on failure.
pub fn load_system_font(query: &SystemFontQuery) -> Option<Vec<u8>> {
    unsafe { load_system_font_inner(query) }
}

unsafe fn load_system_font_inner(query: &SystemFontQuery) -> Option<Vec<u8>> {
    let bold = query.weight >= 600;

    // A representative sample string used to coax CoreText into picking the
    // right covering font via its cascade list. An explicit `query.sample`
    // (per-glyph fallback: the actual uncovered characters) takes precedence;
    // otherwise we fall back to a per-role representative character for CJK /
    // emoji. Roles without a sample resolve the plain UI font.
    let sample: Option<&str> = if !query.sample.is_empty() {
        Some(query.sample.as_str())
    } else {
        match query.role {
            SystemFontRole::Cjk => Some(match query.lang.as_str() {
                "ja" => "\u{3042}", // あ
                "ko" => "\u{ac00}", // 가
                _ => "\u{4e2d}",    // 中 (default zh)
            }),
            SystemFontRole::Emoji => Some("\u{1f600}"), // 😀
            _ => None,
        }
    };

    // Base UI font. CoreText's UI font types give us the system default for the
    // primary script; for serif/mono we still start from the system font and
    // rely on the role mapping below.
    let ui_type = match query.role {
        SystemFontRole::Mono => kCTFontUIFontUserFixedPitch,
        _ if bold => kCTFontUIFontEmphasizedSystem,
        _ => kCTFontUIFontSystem,
    };

    let base_font = CTFontCreateUIFontForLanguage(ui_type, 0.0, std::ptr::null());
    if base_font.is_null() {
        return None;
    }

    // For CJK / emoji, ask CoreText for a font that can render the sample char,
    // starting from the base UI font (this triggers the cascade list).
    let font = if let Some(sample) = sample {
        let cf_sample = cfstring_from_str(sample);
        if cf_sample.is_null() {
            CFRelease(base_font);
            return None;
        }
        let len = CFStringGetLength(cf_sample);
        let range = CFRange {
            location: 0,
            length: len as u64,
        };
        let f = CTFontCreateForString(base_font, cf_sample, range);
        CFRelease(cf_sample as *const c_void);
        CFRelease(base_font);
        if f.is_null() {
            return None;
        }
        f
    } else {
        base_font
    };

    // macOS / iOS / tvOS all reassemble the font's sfnt tables from the CTFont
    // directly, in memory. We never read the on-disk file: on iOS the path is
    // sandboxed away, and on macOS the system UI/CJK fonts (SFNS.ttf,
    // PingFangUI.ttc) are Apple's proprietary *variable* fonts whose `gvar`-driven
    // outlines `ttf_parser` can't resolve — the in-memory path flattens them to
    // their base outlines (see `sfnt_bytes_from_ctfont`), so we get the real
    // system font instead of a substitute.
    let bytes = {
        // CoreText's `CTFontCreateForString` never fails: when no installed font
        // covers the requested characters it returns Apple's "LastResort" font, a
        // tiny CFF stub that maps *every* character to a single tofu-box glyph
        // (glyph id 4 — a real, non-`.notdef` outline). Reassembling that into a
        // family member is doubly harmful: it renders tofu boxes, and because the
        // final glyph id is non-zero the shaper's `collect_missing_scripts` never
        // flags the run as missing, so `ensure_fallback_for_scripts` never runs and
        // no proper per-script system font is ever resolved. Reject it here so the
        // uncovered characters stay at glyph id 0 and the fallback path can fire.
        //
        // LastResort can't be detected by name — CoreText preserves the *requested*
        // PostScript/family name even when it falls back. But LastResort has only a
        // handful of glyphs (7), whereas any real covering system font has hundreds
        // to thousands, so a tiny glyph count is an unambiguous signature.
        if is_last_resort_glyph_count(CTFontGetGlyphCount(font)) {
            CFRelease(font);
            return None;
        }
        let out = sfnt_bytes_from_ctfont(font);
        CFRelease(font);
        out
    };

    bytes
}

/// Build a CFString from a Rust &str (caller must CFRelease).
unsafe fn cfstring_from_str(s: &str) -> CFStringRef {
    CFStringCreateWithBytes(
        std::ptr::null(),
        s.as_ptr(),
        s.len() as CFIndex,
        kCFStringEncodingUTF8,
        0,
    )
}

/// macOS / iOS / tvOS: copy every sfnt table out of a `CTFont` and reassemble a
/// self-contained single-face `.ttf` in memory that `ttf_parser` can parse
/// (see module docs). Returns `None` if the font exposes no tables.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos"))]
unsafe fn sfnt_bytes_from_ctfont(font: CTFontRef) -> Option<Vec<u8>> {
    let tag_array = CTFontCopyAvailableTables(font, kCTFontTableOptionNoOptions);
    if tag_array.is_null() {
        return None;
    }
    let count = CFArrayGetCount(tag_array);
    let mut tables: Vec<(u32, Vec<u8>)> = Vec::with_capacity(count.max(0) as usize);
    for i in 0..count {
        // NOTE: `CTFontCopyAvailableTables` does NOT box the tags as CFNumber
        // objects. It stuffs each tag's raw integer value directly into the
        // CFArray's `const void *` element slot (a documented CoreText quirk).
        // So the "pointer" IS the tag — passing it to CFNumberGetValue would
        // dereference e.g. 0x47444546 ("GDEF") as an object and segfault.
        let tag = CFArrayGetValueAtIndex(tag_array, i) as usize as u32;
        if tag == 0 {
            continue;
        }

        // Skip color-bitmap tables entirely — never even copy them out of
        // CoreText. On `AppleColorEmoji` the `sbix` table is ~179MB (99.5% of
        // the font); materializing it here would make that 179MB resident in
        // `ttf_parser`'s parsed face for the whole process lifetime. We don't
        // need it: color-emoji glyphs are drawn on demand via CoreText's
        // `CTFontDrawGlyphs` (see draw crate's `color_emoji_render`), which
        // opens its own lazily-mmapped CTFont (~8MB) with the bitmaps intact.
        // `ttf_parser` still gets `glyf`/`cmap`/`name`/metrics from the ~0.75MB
        // of remaining tables, so shaping and glyph ids are unchanged. Filtered
        // here (not in the `retain` below) so the 179MB is never allocated at
        // all — not even transiently — and independent of `has_glyf` so a
        // hypothetical `sbix`-only emoji font is also stripped.
        const SKIP_TAGS: [&[u8; 4]; 5] = [b"sbix", b"CBDT", b"CBLC", b"COLR", b"CPAL"];
        if SKIP_TAGS
            .iter()
            .any(|t| tag == u32::from_be_bytes(**t))
        {
            continue;
        }

        let data = CTFontCopyTable(font, tag, kCTFontTableOptionNoOptions);
        if data.is_null() {
            continue;
        }
        let ptr = CFDataGetBytePtr(data);
        let len = CFDataGetLength(data);
        if !ptr.is_null() && len > 0 {
            let bytes = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            tables.push((tag, bytes));
        }
        CFRelease(data);
    }
    CFRelease(tag_array as *const c_void);

    if tables.is_empty() {
        return None;
    }

    // Flatten variable TrueType fonts to their default instance.
    //
    // Apple's UI fonts (SF Pro) are variable `glyf` fonts carrying `gvar` deltas.
    // `ttf_parser` routes ALL outlining through `gvar` when it is present, and it
    // cannot resolve Apple's `gvar`/`avar` layout for these fonts, so every glyph
    // comes back empty (blank / tofu). We only ever consume the default instance
    // anyway (consumers load with `weight: None`, `variations: []`), so drop the
    // variation tables and let `ttf_parser` read the base `glyf` outlines directly.
    //
    // Guarded on `glyf` being present: CFF fonts and Apple's proprietary `hvgl`
    // fonts have no `glyf` fallback, so stripping would not help and could remove
    // tables the CFF path relies on — leave those untouched.
    const GLYF_TAG: u32 = u32::from_be_bytes(*b"glyf");
    let has_glyf = tables.iter().any(|(tag, _)| *tag == GLYF_TAG);
    if has_glyf {
        const VAR_TAGS: [&[u8; 4]; 6] = [b"gvar", b"fvar", b"avar", b"HVAR", b"MVAR", b"STAT"];
        let drop: [u32; 6] = std::array::from_fn(|i| u32::from_be_bytes(*VAR_TAGS[i]));
        tables.retain(|(tag, _)| !drop.contains(tag));
    }

    Some(assemble_sfnt(tables))
}

/// Whether a resolved font's glyph count marks it as Apple's "LastResort" tofu
/// font — the font CoreText hands back when nothing covers the requested
/// characters. LastResort has only a handful of glyphs (7); any real covering
/// system font has hundreds to thousands. The threshold is well clear of both.
/// See the call site for why names can't be used for this.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos", test))]
fn is_last_resort_glyph_count(glyph_count: isize) -> bool {
    glyph_count <= 16
}

/// Assemble a spec-compliant sfnt (`.ttf`) byte buffer from `(tag, table_bytes)`
/// pairs. Pure (no CoreText) so it is unit-testable. All multi-byte fields are
/// written big-endian; table bytes copied from CoreText are already in sfnt
/// (big-endian) order and are written verbatim.
///
/// Layout per the OpenType spec: 12-byte header, then a table directory of
/// 16-byte records sorted by tag ascending, then the table data with each table
/// aligned to a 4-byte boundary. Checksums (including `head.checkSumAdjustment`)
/// are computed correctly, though `ttf_parser` itself does not verify them.
// Called from the shared reassembly path (macOS/iOS/tvOS) and tests.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos", test))]
fn assemble_sfnt(mut tables: Vec<(u32, Vec<u8>)>) -> Vec<u8> {
    // Table directory must be sorted by tag ascending.
    tables.sort_by_key(|(tag, _)| *tag);

    let num_tables = tables.len() as u16;

    // searchRange / entrySelector / rangeShift, per spec.
    let mut entry_selector: u16 = 0;
    while (1u32 << (entry_selector + 1)) <= num_tables as u32 {
        entry_selector += 1;
    }
    let search_range: u16 = (1u16 << entry_selector) * 16;
    let range_shift: u16 = num_tables * 16 - search_range;

    let header_len = 12usize;
    let dir_len = tables.len() * 16;
    let mut data_offset = header_len + dir_len;

    // Compute the absolute (4-byte-aligned) offset of each table.
    let mut offsets: Vec<u32> = Vec::with_capacity(tables.len());
    for (_, bytes) in &tables {
        offsets.push(data_offset as u32);
        data_offset += bytes.len();
        data_offset = (data_offset + 3) & !3; // pad to 4-byte boundary
    }
    let total_len = data_offset;

    let mut buf = vec![0u8; total_len];

    // Pick the sfnt version by outline flavor. Apple's UI fonts (SF Pro etc.)
    // use CFF/CFF2 (PostScript) outlines, not TrueType `glyf`. ttf_parser keys
    // off this version word to decide whether to read `glyf`/`loca` or `CFF `,
    // so hardcoding TrueType makes it look for a `glyf` table that CFF fonts
    // don't have — every glyph then comes back empty (blank / tofu). Emit
    // 'OTTO' when a CFF/CFF2 table is present, else the TrueType version.
    const CFF_TAG: u32 = u32::from_be_bytes(*b"CFF ");
    const CFF2_TAG: u32 = u32::from_be_bytes(*b"CFF2");
    let is_cff = tables
        .iter()
        .any(|(tag, _)| *tag == CFF_TAG || *tag == CFF2_TAG);
    let sfnt_version: u32 = if is_cff { 0x4F54_544F } else { 0x0001_0000 };

    // sfnt header.
    buf[0..4].copy_from_slice(&sfnt_version.to_be_bytes());
    buf[4..6].copy_from_slice(&num_tables.to_be_bytes());
    buf[6..8].copy_from_slice(&search_range.to_be_bytes());
    buf[8..10].copy_from_slice(&entry_selector.to_be_bytes());
    buf[10..12].copy_from_slice(&range_shift.to_be_bytes());

    const HEAD_TAG: u32 = u32::from_be_bytes(*b"head");

    // Copy table data first (so we can checksum it) and remember where `head` lives.
    let mut head_offset: Option<usize> = None;
    for (i, (tag, bytes)) in tables.iter().enumerate() {
        let off = offsets[i] as usize;
        buf[off..off + bytes.len()].copy_from_slice(bytes);
        if *tag == HEAD_TAG {
            head_offset = Some(off);
        }
    }

    // Zero head.checkSumAdjustment (offset 8 within the head table) before
    // computing checksums, per spec.
    if let Some(off) = head_offset {
        if off + 12 <= buf.len() {
            buf[off + 8..off + 12].copy_from_slice(&0u32.to_be_bytes());
        }
    }

    // Write the table directory records (tag / checkSum / offset / length).
    for (i, (tag, bytes)) in tables.iter().enumerate() {
        let rec = header_len + i * 16;
        let off = offsets[i] as usize;
        // Checksum over the padded table bytes as they sit in the buffer.
        let padded_end = if i + 1 < tables.len() {
            offsets[i + 1] as usize
        } else {
            total_len
        };
        let checksum = sfnt_checksum(&buf[off..padded_end]);
        buf[rec..rec + 4].copy_from_slice(&tag.to_be_bytes());
        buf[rec + 4..rec + 8].copy_from_slice(&checksum.to_be_bytes());
        buf[rec + 8..rec + 12].copy_from_slice(&offsets[i].to_be_bytes());
        buf[rec + 12..rec + 16].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
    }

    // head.checkSumAdjustment = 0xB1B0AFBA - checksum(entire file).
    if let Some(off) = head_offset {
        if off + 12 <= buf.len() {
            let file_checksum = sfnt_checksum(&buf);
            let adjustment = 0xB1B0_AFBAu32.wrapping_sub(file_checksum);
            buf[off + 8..off + 12].copy_from_slice(&adjustment.to_be_bytes());
        }
    }

    buf
}

/// OpenType table checksum: sum of the data as big-endian u32 words, with the
/// final partial word zero-padded. Wraps on overflow.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "tvos", test))]
fn sfnt_checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
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

#[cfg(test)]
mod tests {
    use super::{assemble_sfnt, is_last_resort_glyph_count, sfnt_checksum};

    #[test]
    fn detects_last_resort_by_glyph_count() {
        // LastResort has 7 glyphs; reject anything at or below the threshold.
        assert!(is_last_resort_glyph_count(7));
        assert!(is_last_resort_glyph_count(0));
        assert!(is_last_resort_glyph_count(16));
        // Real system fonts have hundreds to thousands of glyphs.
        assert!(!is_last_resort_glyph_count(17));
        assert!(!is_last_resort_glyph_count(2938)); // SF UI
        assert!(!is_last_resort_glyph_count(47098)); // PingFang super-font
    }

    #[test]
    fn checksum_zero_pads_partial_word() {
        // 3 bytes -> padded to [0x01,0x02,0x03,0x00].
        assert_eq!(sfnt_checksum(&[0x01, 0x02, 0x03]), 0x0102_0300);
        // exact word.
        assert_eq!(sfnt_checksum(&[0x00, 0x00, 0x00, 0x05]), 0x0000_0005);
    }

    #[test]
    fn assembles_valid_sorted_aligned_directory() {
        // Tags deliberately out of order; `head` present with a bogus adjustment.
        let head_tag = u32::from_be_bytes(*b"head");
        let tables = vec![
            (u32::from_be_bytes(*b"glyf"), vec![0xAA; 7]),
            (head_tag, {
                let mut h = vec![0u8; 54];
                // put junk in checkSumAdjustment to prove it gets rewritten
                h[8..12].copy_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
                h
            }),
            (u32::from_be_bytes(*b"cmap"), vec![0xBB; 4]),
        ];
        let buf = assemble_sfnt(tables);

        // Header.
        assert_eq!(&buf[0..4], &0x0001_0000u32.to_be_bytes());
        let num_tables = u16::from_be_bytes([buf[4], buf[5]]);
        assert_eq!(num_tables, 3);

        // Directory tags must be ascending, offsets 4-byte aligned, and within bounds.
        let mut prev_tag = 0u32;
        for i in 0..num_tables as usize {
            let rec = 12 + i * 16;
            let tag = u32::from_be_bytes([buf[rec], buf[rec + 1], buf[rec + 2], buf[rec + 3]]);
            let off = u32::from_be_bytes([buf[rec + 8], buf[rec + 9], buf[rec + 10], buf[rec + 11]]);
            let len = u32::from_be_bytes([buf[rec + 12], buf[rec + 13], buf[rec + 14], buf[rec + 15]]);
            assert!(tag > prev_tag, "tags must be strictly ascending");
            prev_tag = tag;
            assert_eq!(off % 4, 0, "table offset must be 4-byte aligned");
            assert!(off as usize + len as usize <= buf.len());
        }

        // Whole-file checksum must satisfy the OpenType invariant once
        // checkSumAdjustment is written.
        assert_eq!(sfnt_checksum(&buf), 0xB1B0_AFBA);
    }
}
