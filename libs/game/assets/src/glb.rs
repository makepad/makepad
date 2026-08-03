//! Minimal GLB probe: rigged/animated flags and an approximate bounding size.
//!
//! Deliberately not a glTF parser — `libs/game/render` owns the real one. This
//! reads the container header and scans the JSON chunk textually for the three
//! facts the index needs. Everything is optional: a file we cannot understand
//! yields `Probe::default()` and the entry is still indexed and searchable.

use std::path::Path;

#[derive(Default, Debug)]
pub struct Probe {
    pub rigged: bool,
    pub animated: bool,
    pub size: Option<[f32; 3]>,
}

const GLB_MAGIC: u32 = 0x4655_4C67; // "glTF"
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
/// The JSON chunk of a Kenney-scale model is a few KB; cap the read so a
/// hostile or corrupt file cannot make us allocate wildly.
const MAX_JSON: usize = 4 * 1024 * 1024;

pub fn probe(path: &Path) -> Probe {
    let Ok(bytes) = std::fs::read(path) else {
        return Probe::default();
    };
    let Some(json) = json_chunk(&bytes) else {
        return Probe::default();
    };
    Probe {
        // Presence of a skin means a skeleton; `"skins"` only appears as a
        // top-level array key in glTF.
        rigged: json.contains("\"skins\""),
        animated: json.contains("\"animations\""),
        size: bounds(&json),
    }
}

fn json_chunk(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 20 || u32(bytes, 0)? != GLB_MAGIC {
        return None;
    }
    let len = u32(bytes, 12)? as usize;
    if u32(bytes, 16)? != CHUNK_JSON || len > MAX_JSON {
        return None;
    }
    let start = 20usize;
    let end = start.checked_add(len)?;
    if end > bytes.len() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes[start..end]).into_owned())
}

fn u32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at + 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Approximate bounds from the first 3-component accessor min/max pair, which
/// by glTF convention is a POSITION accessor (they are the only accessors
/// required to carry min/max). Approximate on purpose: it drives "big vs
/// small" filters, not collision.
fn bounds(json: &str) -> Option<[f32; 3]> {
    let mut min: Option<[f32; 3]> = None;
    let mut max: Option<[f32; 3]> = None;
    let mut rest = json;
    while let Some(pos) = rest.find("\"min\"") {
        let after = &rest[pos + 5..];
        if let Some(v) = triple(after) {
            // The matching "max" follows within the same accessor object.
            let m = after.find("\"max\"").and_then(|p| triple(&after[p + 5..]));
            if let Some(m) = m {
                min = Some(v);
                max = Some(m);
                break;
            }
        }
        rest = &rest[pos + 5..];
    }
    let (lo, hi) = (min?, max?);
    Some([
        (hi[0] - lo[0]).abs(),
        (hi[1] - lo[1]).abs(),
        (hi[2] - lo[2]).abs(),
    ])
}

/// Read `[a,b,c]` (exactly three numbers) from the head of `s`.
fn triple(s: &str) -> Option<[f32; 3]> {
    let open = s.find('[')?;
    let close = s[open..].find(']')? + open;
    let mut it = s[open + 1..close].split(',');
    let a = it.next()?.trim().parse::<f32>().ok()?;
    let b = it.next()?.trim().parse::<f32>().ok()?;
    let c = it.next()?.trim().parse::<f32>().ok()?;
    if it.next().is_some() {
        return None; // 4+ components: not a POSITION accessor
    }
    Some([a, b, c])
}
