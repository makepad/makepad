//! Resolve HarmonyOS / OpenHarmony (OHOS) system fonts at runtime, via ArkTS.
//!
//! Like every other platform, makepad no longer bundles text fonts — glyph
//! coverage is delegated to whatever the device ships (HarmonyOS Sans and its
//! SC / TC / CJK / emoji variants). We resolve those fonts through HarmonyOS's
//! own font API rather than assuming a `/system/fonts` layout: the ArkTS glue
//! (`ArkGlue.getSystemFontPaths` in `makepad.ets`) enumerates the system fonts
//! with `@ohos.font` — `getSystemFontList()` + `getFontByName()`, both
//! synchronous so they can run inside the blocking uv_loop callback — ranks them
//! for the requested role/weight/style, and returns candidate file paths.
//!
//! `@ohos.font`'s `FontInfo` exposes a font-file `path` but no charset, so the
//! per-glyph fallback (`query.sample`) still has to be decided here by reading
//! each candidate's `cmap` table — the same dependency-free coverage check
//! Android uses. In short: ArkTS does font *discovery* (names → paths), Rust
//! does *coverage* and byte loading.
//!
//! Fallback: `@ohos.font`'s `getSystemFontList` / `getFontByName` are deprecated
//! since API 18, so a future OHOS release could remove them (the ArkTS glue then
//! returns no paths). If the ArkTS route yields nothing, we fall back to scanning
//! the system font directories directly and deciding coverage / role by `cmap`
//! and filename — the same self-contained strategy Android uses. Only when *both*
//! routes come up empty do we return `None`.

use super::arkts_obj_ref::ArkTsObjRef;
use super::oh_util;
use crate::cx_api::{SystemFontQuery, SystemFontResult, SystemFontRole};

/// Resolve `query` to font file bytes + face index. Primary route is the ArkTS
/// font API; if that returns nothing (e.g. the deprecated `@ohos.font` API was
/// removed in a future OHOS release), fall back to a direct scan of the system
/// font dirs. Returns `None` only when both routes fail.
pub fn load_system_font(
    arkts: &mut ArkTsObjRef,
    query: &SystemFontQuery,
) -> Option<SystemFontResult> {
    if let Some(result) = load_via_arkts(arkts, query) {
        return Some(result);
    }
    // ArkTS gave us nothing usable — scan the font dirs ourselves.
    load_via_dir_scan(query)
}

/// Primary route: ask the ArkTS layer for candidate font paths, then read (and,
/// for the sample case, coverage-check) them.
fn load_via_arkts(arkts: &mut ArkTsObjRef, query: &SystemFontQuery) -> Option<SystemFontResult> {
    let paths = query_font_paths(arkts, query)?;
    if paths.is_empty() {
        return None;
    }
    load_from_paths(&paths, query)
}

/// Given candidate paths ordered best-first, read the bytes to return. For the
/// per-glyph fallback (`query.sample`), pick the first candidate whose cmap
/// covers every requested char — a charset check is authoritative and matches
/// what the browser / Flutter do. `@ohos.font` exposes no TTC face index, so we
/// derive it from the bytes: `ttc_covering_index` picks the sub-face that
/// actually covers the sample. Otherwise take the best-ranked path (face 0).
/// mmap the font file (falling back to a read) so unused pages of a large font
/// — e.g. the sbix table of a color emoji font, or uncovered CJK regions —
/// never become resident. Coverage probes below read through `as_slice()`.
fn map_font_file(path: &str) -> Option<crate::shared_bytes::SharedBytes> {
    crate::shared_bytes::SharedBytes::from_file_mmap_or_read(path).ok()
}

fn load_from_paths(paths: &[String], query: &SystemFontQuery) -> Option<SystemFontResult> {
    if !query.sample.is_empty() {
        let wanted: Vec<u32> = query.sample.chars().map(|c| c as u32).collect();
        for path in paths {
            let Some(bytes) = map_font_file(path) else {
                continue;
            };
            if let Some(index) = ttc_covering_index(bytes.as_slice(), &wanted) {
                return Some(SystemFontResult { bytes, index });
            }
        }
        return None;
    }
    // Role case: the list is ordered best-first — read the first readable path.
    for path in paths {
        if let Some(bytes) = map_font_file(path) {
            return Some(SystemFontResult { bytes, index: 0 });
        }
    }
    None
}

/// Call `ArkGlue.getSystemFontPaths(queryJson)` and split its newline-joined
/// result into paths. Returns `None` if the ArkTS call fails.
fn query_font_paths(arkts: &mut ArkTsObjRef, query: &SystemFontQuery) -> Option<Vec<String>> {
    let json = query_to_json(query);
    let arg = oh_util::create_string(arkts.raw(), &json)?;
    let ret = arkts
        .call_js_function("getSystemFontPaths", 1, &arg)
        .ok()?;
    let joined = oh_util::get_value_string(arkts.raw(), ret)?;
    Some(
        joined
            .split('\n')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
    )
}

/// Serialize a `SystemFontQuery` into the small JSON object the ArkTS side
/// parses. Hand-rolled (no serde dependency): only `lang` and `sample` are
/// free-form strings that need escaping.
fn query_to_json(query: &SystemFontQuery) -> String {
    let role = match query.role {
        SystemFontRole::Ui => "ui",
        SystemFontRole::Serif => "serif",
        SystemFontRole::Mono => "mono",
        SystemFontRole::Cjk => "cjk",
        SystemFontRole::Emoji => "emoji",
    };
    format!(
        "{{\"role\":\"{}\",\"weight\":{},\"italic\":{},\"lang\":\"{}\",\"sample\":\"{}\"}}",
        role,
        query.weight,
        query.italic,
        json_escape(&query.lang),
        json_escape(&query.sample),
    )
}

/// Escape a string for embedding in a JSON string literal.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ── Directory-scan fallback ─────────────────────────────────────────────────
//
// Used only when the ArkTS route yields nothing (e.g. a future OHOS release
// removed the now-deprecated `@ohos.font` `getSystemFontList`/`getFontByName`).
// We enumerate the system font dirs directly and pick by `cmap` coverage (sample
// case) or filename heuristics + weight/style (role case) — no OHOS API needed,
// mirroring the self-contained strategy `android_system_fonts.rs` uses.

/// System font locations to scan. `/system/fonts` is the standard OHOS layout;
/// the others are checked defensively in case a future release relocates them.
const FONT_DIRS: &[&str] = &["/system/fonts", "/system/etc/fonts", "/data/themes/a/app/fonts"];

/// Fallback route: scan the font dirs and resolve `query` without any OHOS API.
fn load_via_dir_scan(query: &SystemFontQuery) -> Option<SystemFontResult> {
    let files = enumerate_font_files();
    if files.is_empty() {
        return None;
    }

    // Per-glyph fallback: return the first font (by role/weight ranking) whose
    // cmap actually covers every requested char. Coverage is authoritative, so
    // role hints here only decide the *order* we test candidates in.
    if !query.sample.is_empty() {
        let wanted: Vec<u32> = query.sample.chars().map(|c| c as u32).collect();
        let mut ranked = files.clone();
        rank_by_role(&mut ranked, query);
        for path in &ranked {
            let Some(bytes) = map_font_file(path) else {
                continue;
            };
            if let Some(index) = ttc_covering_index(bytes.as_slice(), &wanted) {
                return Some(SystemFontResult { bytes, index });
            }
        }
        return None;
    }

    // Role case: pick the best filename match for the role, then read it.
    let best = pick_best_matching(&files, query)?;
    let bytes = map_font_file(&best)?;
    Some(SystemFontResult { bytes, index: 0 })
}

/// Recursively (one level) collect `.ttf`/`.otf`/`.ttc` paths from `FONT_DIRS`.
fn enumerate_font_files() -> Vec<String> {
    let mut out = Vec::new();
    for dir in FONT_DIRS {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_font = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    let e = e.to_ascii_lowercase();
                    e == "ttf" || e == "otf" || e == "ttc"
                })
                .unwrap_or(false);
            if is_font {
                if let Some(s) = path.to_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
    out
}

/// Normalize a font id (filename stem or family) for substring matching:
/// lowercase, strip `_`, `-`, and spaces. Mirrors `normalizeFontId` in the ArkTS side.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Normalized file stem (basename without extension) for `path`.
fn file_stem_normalized(path: &str) -> String {
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    normalize(stem)
}

/// Role → filename substring hints (best first). Same heuristics as the ArkTS
/// `roleHints`, kept here so the fallback needs no OHOS API.
fn role_name_candidates(role: SystemFontRole, lang: &str) -> Vec<&'static str> {
    let lang = lang.to_ascii_lowercase();
    match role {
        SystemFontRole::Serif => vec!["harmonyossansserif", "notoserif", "serif"],
        SystemFontRole::Mono => vec!["harmonyossansmono", "monospace", "mono"],
        SystemFontRole::Emoji => vec!["hmoscoloremoji", "coloremoji", "emoji", "notocoloremoji"],
        SystemFontRole::Cjk => {
            if lang.starts_with("ja") {
                vec!["notosansjp", "harmonyossansjp", "jp", "cjk", "notosanscjk"]
            } else if lang.starts_with("ko") {
                vec!["notosanskr", "harmonyossanskr", "kr", "cjk", "notosanscjk"]
            } else if lang.starts_with("zh-hant")
                || lang.starts_with("zh-tw")
                || lang.starts_with("zh-hk")
                || lang.contains("hant")
            {
                vec!["harmonyossanstc", "notosanstc", "tc", "cjk", "notosanscjk"]
            } else {
                vec!["harmonyossanssc", "notosanssc", "sc", "cjk", "notosanscjk"]
            }
        }
        SystemFontRole::Ui => vec!["harmonyossans", "roboto", "sans"],
    }
}

/// Order `files` in place by how well their filename matches the role hints,
/// tie-broken by weight/style closeness (parsed from the filename).
fn rank_by_role(files: &mut Vec<String>, query: &SystemFontQuery) {
    let hints = role_name_candidates(query.role, &query.lang);
    let score = |path: &str| -> (usize, u32) {
        let stem = file_stem_normalized(path);
        let rank = hints
            .iter()
            .position(|h| stem.contains(h))
            .unwrap_or(hints.len());
        let style_pen = if name_is_italic(&stem) == query.italic { 0 } else { 10_000 };
        let weight_pen = (weight_from_name(&stem) as i32 - query.weight as i32).unsigned_abs();
        (rank, style_pen + weight_pen)
    };
    files.sort_by(|a, b| score(a).cmp(&score(b)));
}

/// Best role match, or `None` if nothing even loosely matches the role hints.
fn pick_best_matching(files: &[String], query: &SystemFontQuery) -> Option<String> {
    let hints = role_name_candidates(query.role, &query.lang);
    let mut ranked = files.to_vec();
    rank_by_role(&mut ranked, query);
    // Require the top candidate to actually match a hint; otherwise a fallback
    // to an unrelated font would be worse than degrading gracefully.
    let best = ranked.into_iter().next()?;
    let stem = file_stem_normalized(&best);
    if hints.iter().any(|h| stem.contains(h)) {
        Some(best)
    } else {
        None
    }
}

/// Guess a numeric weight from a normalized filename stem.
fn weight_from_name(stem: &str) -> u32 {
    if stem.contains("thin") {
        100
    } else if stem.contains("extralight") || stem.contains("ultralight") {
        200
    } else if stem.contains("light") {
        300
    } else if stem.contains("medium") {
        500
    } else if stem.contains("semibold") || stem.contains("demibold") {
        600
    } else if stem.contains("extrabold") || stem.contains("ultrabold") {
        800
    } else if stem.contains("black") || stem.contains("heavy") {
        900
    } else if stem.contains("bold") {
        700
    } else {
        400
    }
}

/// Whether a normalized filename stem denotes an italic/oblique face.
fn name_is_italic(stem: &str) -> bool {
    stem.contains("italic") || stem.contains("oblique")
}

// ── Minimal cmap parser (dependency-free) ──────────────────────────────────
//
// Just enough of the OpenType spec to answer "does this font cover these code
// points?". We parse the table directory to locate `cmap`, pick the best
// Unicode subtable, and decode format 4 (BMP) and format 12 (full Unicode) —
// the two formats every real-world system font uses for Unicode coverage.
// All multi-byte integers are big-endian. Every read is bounds-checked; any
// malformed offset just yields "not covered" rather than panicking.
//
// This mirrors the parser in `android_system_fonts.rs`; each per-platform font
// module is self-contained.

fn be_u16(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_be_bytes([b[0], b[1]]))
}
fn be_u32(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// Returns *which* face of a `.ttc` to parse: the index of the first sub-font
/// whose `cmap` covers every code point in `wanted`. A bare (non-collection)
/// font is face 0. Returns `None` if nothing covers the sample, so the caller
/// keeps searching other files. HarmonyOS/Noto CJK fonts ship as `.ttc` whose
/// first face is frequently the wrong script, so handing `ttf_parser` this
/// index (rather than a hardcoded 0) is what makes CJK/emoji render correctly.
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
            (3, 10) => 4,         // Windows UCS-4
            (0, 4) | (0, 6) => 4, // Unicode full
            (3, 1) => 3,          // Windows BMP
            (0, _) => 2,          // Unicode BMP
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
        let gid_addr = id_range_offsets + s * 2 + range_off as usize + (cp - start) as usize * 2;
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
