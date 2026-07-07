//! Resolve Android's default fonts at runtime by parsing the system font
//! config (`/system/etc/fonts.xml`, with a fallback to the legacy
//! `/system/etc/system_fonts.xml`) and reading the matched file from
//! `/system/fonts/`.
//!
//! These files are world-readable on Android, so plain `std::fs` works — no
//! AssetManager / JNI needed. We never bundle text fonts: glyph coverage is
//! delegated to whatever the device ships.

use crate::cx_api::{SystemFontQuery, SystemFontResult, SystemFontRole};

const FONTS_DIR: &str = "/system/fonts";
const FONTS_XML: &str = "/system/etc/fonts.xml";
const LEGACY_FONTS_XML: &str = "/system/etc/system_fonts.xml";

/// Resolve `query` to font bytes + face index by reading the Android font config.
pub fn load_system_font(query: &SystemFontQuery) -> Option<SystemFontResult> {
    let xml = std::fs::read_to_string(FONTS_XML)
        .or_else(|_| std::fs::read_to_string(LEGACY_FONTS_XML))
        .ok()?;
    let families = parse_families(&xml);

    let bold = query.weight >= 600;
    let want_weight = if bold { 700 } else { 400 };

    // Per-glyph fallback: if the caller passed the actual uncovered characters,
    // pick the first font file (across every family in fonts.xml) whose cmap
    // covers them all. Android tags families with `lang`, but a charset check
    // is authoritative and matches what the browser/Flutter do. We parse each
    // candidate file's cmap directly (no extra dependency); the caller caches
    // the result and guards with `attempted_scripts`, so each script triggers
    // at most one sweep.
    if !query.sample.is_empty() {
        let wanted: Vec<u32> = query.sample.chars().map(|c| c as u32).collect();
        let lang_hint = query.lang.to_ascii_lowercase();
        let mut entries: Vec<(&FontEntry, &Option<String>)> = families
            .iter()
            .flat_map(|f| f.fonts.iter().map(move |e| (e, &f.lang)))
            .collect();
        entries.sort_by_key(|(e, lang)| {
            let lang_lc = lang.as_deref().map(|l| l.to_ascii_lowercase());
            let lang_rank = match &lang_lc {
                Some(l) if !lang_hint.is_empty() && l.contains(&lang_hint) => 0,
                Some(_) => 1,
                None => 2,
            };
            let style_penalty = if e.italic == query.italic { 0 } else { 10_000 };
            let weight_penalty = (e.weight as i64 - want_weight as i64).unsigned_abs() as u64;
            (lang_rank, style_penalty as u64 + weight_penalty)
        });
        let mut tried: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (entry, _) in entries {
            if !tried.insert(entry.file.as_str()) {
                continue; // several xml entries share one file; read it once.
            }
            let Some(bytes) = read_font_file(&entry.file) else {
                continue;
            };
            // A `.ttc` bundles several faces covering different scripts; pick the
            // exact sub-face that covers the sample so the loader parses the right
            // one (index 0 is frequently the wrong script).
            if let Some(index) = ttc_covering_index(bytes.as_slice(), &wanted) {
                return Some(SystemFontResult { bytes, index });
            }
        }
        return None;
    }

    // Pick candidate families in priority order for the requested role.
    let candidates = role_family_candidates(query.role, &query.lang);

    // First try named-family matches in priority order; then, for CJK / emoji,
    // fall back to any family whose lang attribute matches.
    for name in &candidates {
        if let Some((file, index)) = pick_font_in_family_named(&families, name, want_weight, query.italic) {
            if let Some(bytes) = read_font_file(&file) {
                // Honor the fonts.xml `index=` for `.ttc` collections; it's the
                // sub-face Android assigns to this entry (0 for a bare font).
                return Some(SystemFontResult { bytes, index });
            }
        }
    }

    if let Some(langs) = role_lang_candidates(query.role, &query.lang) {
        for lang in langs {
            if let Some((file, index)) = pick_font_in_family_by_lang(&families, lang, want_weight, query.italic)
            {
                if let Some(bytes) = read_font_file(&file) {
                    return Some(SystemFontResult { bytes, index });
                }
            }
        }
    }

    None
}

fn read_font_file(file: &str) -> Option<crate::shared_bytes::SharedBytes> {
    // fonts.xml entries are bare filenames relative to /system/fonts.
    let path = if file.starts_with('/') {
        file.to_string()
    } else {
        format!("{}/{}", FONTS_DIR, file)
    };
    // mmap (falling back to a read) so unused pages of a large font — e.g. the
    // sbix table of a color emoji font or uncovered CJK regions — never become
    // resident. The `.ttc` coverage probe below reads through `as_slice()`.
    crate::shared_bytes::SharedBytes::from_file_mmap_or_read(&path).ok()
}

// ── Minimal cmap parser (dependency-free) ──────────────────────────────────
//
// Just enough of the OpenType spec to answer "does this font cover these code
// points?". We parse the table directory to locate `cmap`, pick the best
// Unicode subtable, and decode format 4 (BMP) and format 12 (full Unicode) —
// the two formats every real-world system font uses for Unicode coverage.
// All multi-byte integers are big-endian. Every read is bounds-checked; any
// malformed offset just yields "not covered" rather than panicking.

fn be_u16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}
fn be_u32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Returns *which* face of a `.ttc` to parse: the
/// index of the first sub-font whose `cmap` covers every code point in `wanted`.
/// A bare (non-collection) font is face 0. Returns `None` if nothing covers the
/// sample, so the caller keeps searching other files. This is what makes the
/// loader hand `ttf_parser` the correct sub-face instead of a hardcoded 0 (the
/// first face of a CJK `.ttc` is frequently the wrong script).
fn ttc_covering_index(font: &[u8], wanted: &[u32]) -> Option<u32> {
    if wanted.is_empty() {
        return None;
    }
    let tag = be_u32(font, 0)?;
    if tag == 0x7474_6366 {
        // 'ttcf' — walk each sub-font's offset table in order.
        let num_fonts = be_u32(font, 8).unwrap_or(0) as usize;
        for i in 0..num_fonts {
            let sfnt_offset = be_u32(font, 12 + i * 4)? as usize;
            if let Some(cmap) = find_cmap_in_offset_table(font, sfnt_offset) {
                if wanted.iter().all(|&cp| cmap_covers(font, cmap, cp)) {
                    return Some(i as u32);
                }
            }
        }
        None
    } else {
        let cmap = find_cmap_in_offset_table(font, 0)?;
        wanted
            .iter()
            .all(|&cp| cmap_covers(font, cmap, cp))
            .then_some(0)
    }
}

/// Find the `cmap` table offset within a single sfnt offset table at
/// `sfnt_offset` (0 for a bare font, or a sub-font offset within a `.ttc`).
fn find_cmap_in_offset_table(font: &[u8], sfnt_offset: usize) -> Option<usize> {
    let num_tables = be_u16(font, sfnt_offset + 4)? as usize;
    let dir = sfnt_offset + 12;
    for i in 0..num_tables {
        let rec = dir + i * 16;
        if be_u32(font, rec)? == 0x636d_6170 {
            // 'cmap'
            return Some(be_u32(font, rec + 8)? as usize);
        }
    }
    None
}

/// Pick the best Unicode subtable in the cmap and test coverage of `cp`.
fn cmap_covers(font: &[u8], cmap: usize, cp: u32) -> bool {
    let Some(num_sub) = be_u16(font, cmap + 2) else {
        return false;
    };
    // Prefer a full-Unicode subtable (platform 3 / encoding 10, or platform 0)
    // so we can resolve code points above the BMP; fall back to a BMP subtable.
    let mut best: Option<(usize, u8)> = None; // (subtable offset, rank)
    for i in 0..num_sub as usize {
        let rec = cmap + 4 + i * 8;
        let (Some(platform), Some(encoding), Some(off)) = (
            be_u16(font, rec),
            be_u16(font, rec + 2),
            be_u32(font, rec + 4),
        ) else {
            continue;
        };
        let rank = match (platform, encoding) {
            (3, 10) => 4, // Windows UCS-4
            (0, 4) | (0, 6) => 4, // Unicode full
            (3, 1) => 3, // Windows BMP
            (0, _) => 2, // Unicode BMP
            _ => 0,
        };
        if rank == 0 {
            continue;
        }
        let sub = cmap + off as usize;
        if best.map_or(true, |(_, r)| rank > r) {
            best = Some((sub, rank));
        }
    }
    let Some((sub, _)) = best else {
        return false;
    };
    match be_u16(font, sub) {
        Some(4) => cmap_format4_covers(font, sub, cp),
        Some(12) => cmap_format12_covers(font, sub, cp),
        _ => false,
    }
}

/// cmap format 4 (segment mapping to delta values), BMP only.
fn cmap_format4_covers(font: &[u8], sub: usize, cp: u32) -> bool {
    if cp > 0xFFFF {
        return false;
    }
    let cp = cp as u16;
    let Some(seg_x2) = be_u16(font, sub + 6) else {
        return false;
    };
    let segs = (seg_x2 / 2) as usize;
    let end_codes = sub + 14;
    let start_codes = end_codes + seg_x2 as usize + 2; // +2 reservedPad
    let id_deltas = start_codes + seg_x2 as usize;
    let id_range_offsets = id_deltas + seg_x2 as usize;
    for s in 0..segs {
        let Some(end) = be_u16(font, end_codes + s * 2) else {
            return false;
        };
        if cp > end {
            continue;
        }
        let Some(start) = be_u16(font, start_codes + s * 2) else {
            return false;
        };
        if cp < start {
            return false; // segments are sorted; cp falls in a gap
        }
        let Some(range_off) = be_u16(font, id_range_offsets + s * 2) else {
            return false;
        };
        if range_off == 0 {
            // Mapped via idDelta; a non-zero resulting glyph means covered.
            let Some(delta) = be_u16(font, id_deltas + s * 2) else {
                return false;
            };
            let glyph = cp.wrapping_add(delta);
            return glyph != 0;
        }
        // Mapped via glyphIdArray: index from the idRangeOffset slot.
        let gid_addr = id_range_offsets
            + s * 2
            + range_off as usize
            + (cp - start) as usize * 2;
        return match be_u16(font, gid_addr) {
            Some(0) | None => false,
            Some(_) => true,
        };
    }
    false
}

/// cmap format 12 (segmented coverage), full Unicode range.
fn cmap_format12_covers(font: &[u8], sub: usize, cp: u32) -> bool {
    let Some(n_groups) = be_u32(font, sub + 12) else {
        return false;
    };
    let groups = sub + 16;
    // Groups are sorted by startCharCode; binary-search them.
    let (mut lo, mut hi) = (0u32, n_groups);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let g = groups + mid as usize * 12;
        let (Some(start), Some(end)) = (be_u32(font, g), be_u32(font, g + 4)) else {
            return false;
        };
        if cp < start {
            hi = mid;
        } else if cp > end {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

fn role_family_candidates(role: SystemFontRole, _lang: &str) -> Vec<&'static str> {
    match role {
        SystemFontRole::Ui => vec!["sans-serif", "roboto"],
        SystemFontRole::Serif => vec!["serif", "noto-serif"],
        SystemFontRole::Mono => vec!["monospace", "sans-serif-monospace"],
        // CJK / emoji are resolved via lang attributes below; named families
        // here are best-effort common names.
        SystemFontRole::Cjk => vec![],
        SystemFontRole::Emoji => vec![],
    }
}

fn role_lang_candidates(role: SystemFontRole, lang: &str) -> Option<Vec<&'static str>> {
    match role {
        SystemFontRole::Cjk => Some(match lang {
            "ja" => vec!["ja"],
            "ko" => vec!["ko"],
            // zh-Hans first, then generic CJK fallbacks.
            _ => vec!["zh-Hans", "zh-Hant", "zh"],
        }),
        // Android tags color emoji with the und-Zsye (Symbols, Emoji) script.
        SystemFontRole::Emoji => Some(vec!["und-Zsye", "und-Zsym"]),
        _ => None,
    }
}

#[derive(Default)]
struct FontEntry {
    file: String,
    weight: u32,
    italic: bool,
    /// `.ttc` face index from the fonts.xml `index=` attribute (0 for a bare
    /// font or when the attribute is absent). This is the sub-face Android
    /// itself selects for the entry, so a role match must honor it rather than
    /// assuming face 0 — a CJK `.ttc` collection often puts the wanted script
    /// at a non-zero index.
    index: u32,
}

#[derive(Default)]
struct Family {
    name: Option<String>,
    lang: Option<String>,
    fonts: Vec<FontEntry>,
}

/// A deliberately tiny, dependency-free parser for the subset of the Android
/// fonts.xml schema we need: `<family [name=..] [lang=..]> <font weight=..
/// style=..>file</font>* </family>`.
fn parse_families(xml: &str) -> Vec<Family> {
    let mut families = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find("<family") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else { break };
        let open_tag = &after[..open_end];
        let name = attr_value(open_tag, "name");
        let lang = attr_value(open_tag, "lang");

        // Body up to the closing </family>.
        let body_start = start + open_end + 1;
        let body_and_rest = &rest[body_start..];
        let (body, next) = match body_and_rest.find("</family>") {
            Some(close) => (
                &body_and_rest[..close],
                &body_and_rest[close + "</family>".len()..],
            ),
            None => (body_and_rest, ""),
        };

        let mut family = Family {
            name,
            lang,
            fonts: Vec::new(),
        };
        parse_fonts_into(body, &mut family.fonts);
        families.push(family);
        rest = next;
    }

    families
}

fn parse_fonts_into(body: &str, out: &mut Vec<FontEntry>) {
    let mut rest = body;
    while let Some(start) = rest.find("<font") {
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else { break };
        let open_tag = &after[..open_end];

        let weight = attr_value(open_tag, "weight")
            .and_then(|w| w.trim().parse::<u32>().ok())
            .unwrap_or(400);
        let italic = attr_value(open_tag, "style")
            .map(|s| s.trim().eq_ignore_ascii_case("italic"))
            .unwrap_or(false);
        let index = attr_value(open_tag, "index")
            .and_then(|i| i.trim().parse::<u32>().ok())
            .unwrap_or(0);

        let content_start = start + open_end + 1;
        let content_and_rest = &rest[content_start..];
        let (content, next) = match content_and_rest.find("</font>") {
            Some(close) => (
                &content_and_rest[..close],
                &content_and_rest[close + "</font>".len()..],
            ),
            None => (content_and_rest, ""),
        };

        let file = content.trim();
        if !file.is_empty() {
            out.push(FontEntry {
                file: file.to_string(),
                weight,
                italic,
                index,
            });
        }
        rest = next;
    }
}

/// Read the value of `attr="..."` (or `attr='...'`) from an opening-tag slice.
fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let key = format!("{}=", attr);
    let mut search = tag;
    while let Some(pos) = search.find(&key) {
        // Ensure the char before the key is a boundary (space or tag-open), so
        // `lang=` doesn't match inside e.g. `xlang=`.
        let prev_ok = pos == 0
            || search.as_bytes()[pos - 1].is_ascii_whitespace()
            || search.as_bytes()[pos - 1] == b'<';
        let after = &search[pos + key.len()..];
        if !prev_ok {
            search = after;
            continue;
        }
        let bytes = after.as_bytes();
        if bytes.is_empty() {
            return None;
        }
        let quote = bytes[0];
        if quote == b'"' || quote == b'\'' {
            let inner = &after[1..];
            if let Some(end) = inner.find(quote as char) {
                return Some(inner[..end].to_string());
            }
        }
        return None;
    }
    None
}

fn pick_font_in_family_named(
    families: &[Family],
    name: &str,
    want_weight: u32,
    want_italic: bool,
) -> Option<(String, u32)> {
    families
        .iter()
        .filter(|f| {
            f.name
                .as_deref()
                .map(|n| n.eq_ignore_ascii_case(name))
                .unwrap_or(false)
        })
        .find_map(|f| pick_best(&f.fonts, want_weight, want_italic))
}

fn pick_font_in_family_by_lang(
    families: &[Family],
    lang: &str,
    want_weight: u32,
    want_italic: bool,
) -> Option<(String, u32)> {
    families
        .iter()
        .filter(|f| {
            f.lang
                .as_deref()
                .map(|l| {
                    // lang attr may list several tags space-separated, and may
                    // be more specific (zh-Hans-CN) than our query (zh-Hans).
                    l.split([',', ' ']).any(|t| {
                        let t = t.trim();
                        !t.is_empty()
                            && (t.eq_ignore_ascii_case(lang)
                                || t.to_ascii_lowercase()
                                    .starts_with(&lang.to_ascii_lowercase()))
                    })
                })
                .unwrap_or(false)
        })
        .find_map(|f| pick_best(&f.fonts, want_weight, want_italic))
}

/// Choose the font entry closest to the requested weight, preferring a style
/// (italic/roman) match. Returns the file plus its `.ttc` face index, so a role
/// match parses the sub-face fonts.xml assigned rather than a hardcoded 0.
fn pick_best(fonts: &[FontEntry], want_weight: u32, want_italic: bool) -> Option<(String, u32)> {
    fonts
        .iter()
        .min_by_key(|e| {
            let style_penalty = if e.italic == want_italic { 0 } else { 10_000 };
            let weight_penalty = (e.weight as i64 - want_weight as i64).unsigned_abs() as u64;
            style_penalty as u64 + weight_penalty
        })
        .map(|e| (e.file.clone(), e.index))
}
