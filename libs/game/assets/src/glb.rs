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
    /// Animation clip names in file order. This is the fact the AI cannot
    /// guess: it must know a character HAS `Jump_Land` before writing code
    /// that plays it, and every pack names its states differently.
    pub clips: Vec<String>,
    /// Joint count of the first skin. Equal counts usually mean the same rig,
    /// which is what lets one animation vocabulary drive many characters.
    pub joints: u32,
}

/// "glTF" little-endian. The previous value (0x4655_4C67) spells "gLUF", so
/// the magic check rejected every real GLB and `probe` returned defaults for
/// the whole library — no model was ever detected as rigged, animated or
/// sized. Kept as a named constant with this note because the failure was
/// invisible: the index still built, it just believed nothing.
const GLB_MAGIC: u32 = 0x4654_6C67;
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
/// The JSON chunk of a Kenney-scale model is a few KB; cap the read so a
/// hostile or corrupt file cannot make us allocate wildly.
const MAX_JSON: usize = 4 * 1024 * 1024;

pub fn probe(path: &Path) -> Probe {
    // Read only the JSON chunk, not the mesh data behind it. A GLB states its
    // JSON length in the header, so indexing 5000 models touches a few KB
    // each instead of the whole file — the difference between a 7 s startup
    // and a fraction of a second.
    let Ok(bytes) = read_head(path) else {
        return Probe::default();
    };
    // Two containers reach us: .glb wraps the JSON in a binary chunk (Kenney,
    // KayKit), while .gltf IS the JSON (Quaternius ships self-contained .gltf
    // with the buffer base64'd inline). Only the header differs.
    let Some(json) = json_chunk(&bytes).or_else(|| gltf_json(&bytes)) else {
        return Probe::default();
    };
    let clips = clip_names(&json);
    Probe {
        // Presence of a skin means a skeleton; `"skins"` only appears as a
        // top-level array key in glTF.
        rigged: json.contains("\"skins\""),
        animated: !clips.is_empty() || json.contains("\"animations\""),
        size: bounds(&json),
        joints: joint_count(&json),
        clips,
    }
}

/// Read just enough of the file to answer the probe.
///
/// For a GLB that is the 20-byte header plus the JSON chunk it declares. For
/// a plain `.gltf` there is no such framing, so the document is read whole but
/// capped — those embed their buffer as base64 and run to megabytes.
fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut head = [0u8; 20];
    let n = f.read(&mut head)?;
    if n == 20 && u32(&head, 0) == Some(GLB_MAGIC) {
        let len = u32(&head, 12).unwrap_or(0) as usize;
        if u32(&head, 16) == Some(CHUNK_JSON) && len <= MAX_JSON {
            let mut out = head.to_vec();
            out.resize(20 + len, 0);
            // A truncated file yields a short read; the caller's bounds check
            // rejects it rather than reading past what arrived.
            let got = f.read(&mut out[20..])?;
            out.truncate(20 + got);
            return Ok(out);
        }
        return Ok(head.to_vec());
    }
    // Not a GLB: fall back to the whole document, capped.
    let mut out = head[..n].to_vec();
    f.take(MAX_JSON as u64).read_to_end(&mut out)?;
    Ok(out)
}

/// A plain `.gltf` file is the JSON document itself. Cap it like the GLB path:
/// Quaternius embeds its buffer as base64, so these run to a few MB.
fn gltf_json(bytes: &[u8]) -> Option<String> {
    if bytes.len() > 64 * 1024 * 1024 {
        return None;
    }
    let s = String::from_utf8_lossy(bytes);
    let head = &s[..s.len().min(512)];
    if !head.trim_start().starts_with('{') || !head.contains("\"asset\"") {
        return None;
    }
    Some(s.into_owned())
}

/// Clip names, read from the `"animations"` array only.
///
/// Scoped by bracket-matching rather than a global scan for `"name"`, because
/// meshes, nodes and materials all carry names too — a global scan would
/// report "Cube.003" as an animation state.
fn clip_names(json: &str) -> Vec<String> {
    let Some(arr) = array_after(json, "\"animations\"") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut rest = arr;
    // Names at object-depth 1 are the animations themselves; deeper ones
    // belong to channels and samplers. `]` at depth 0 ends the array — without
    // that stop the scan runs on into `materials` and reports texture names as
    // animation states.
    while let Some(i) = rest.find(|c| c == '{' || c == '}' || c == '[' || c == ']' || c == '"') {
        let ch = rest.as_bytes()[i];
        match ch {
            b'{' | b'[' => {
                depth += 1;
                rest = &rest[i + 1..];
            }
            b']' if depth == 0 => break,
            b'}' | b']' => {
                depth -= 1;
                rest = &rest[i + 1..];
            }
            _ => {
                let after = &rest[i..];
                if depth == 1 && after.starts_with("\"name\"") {
                    if let Some(v) = string_value(&after[6..]) {
                        out.push(v);
                    }
                }
                // Skip the whole string so its contents can't be re-scanned.
                rest = match string_end(after) {
                    Some(e) => &after[e..],
                    None => break,
                };
            }
        }
        if out.len() > 512 {
            break; // absurd clip count: stop rather than grow unbounded
        }
    }
    out
}

fn joint_count(json: &str) -> u32 {
    let Some(arr) = array_after(json, "\"joints\"") else {
        return 0;
    };
    let Some(end) = arr.find(']') else { return 0 };
    arr[..end]
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .count() as u32
}

/// Slice starting just after the `[` that follows `key`.
fn array_after<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let at = json.find(key)?;
    let open = json[at..].find('[')? + at;
    Some(&json[open + 1..])
}

/// Read `"..."` from the head of `s` (after a `:`), honouring backslash escapes.
fn string_value(s: &str) -> Option<String> {
    let colon = s.find(':')?;
    let rest = &s[colon + 1..];
    let open = rest.find('"')?;
    let body = &rest[open + 1..];
    let end = unescaped_quote(body)?;
    Some(body[..end].to_string())
}

/// Byte offset just past the closing quote of the string starting at `s[0]`.
fn string_end(s: &str) -> Option<usize> {
    let body = &s[1..];
    Some(1 + unescaped_quote(body)? + 1)
}

fn unescaped_quote(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
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
    // Search each key independently rather than assuming an order. Kenney's
    // exporter writes "max" BEFORE "min", so looking for "max" only after
    // "min" found nothing and every Kenney model came back sizeless — which
    // silently disabled the big/small filters across the whole library.
    let min = first_triple(json, "\"min\"")?;
    let max = first_triple(json, "\"max\"")?;
    let (lo, hi) = (min, max);
    Some([
        (hi[0] - lo[0]).abs(),
        (hi[1] - lo[1]).abs(),
        (hi[2] - lo[2]).abs(),
    ])
}

/// First `[a,b,c]` following `key` anywhere in the document. The first
/// accessor carrying bounds is POSITION by glTF convention (only it is
/// required to have them), so the first hit is the mesh's own extent.
fn first_triple(json: &str, key: &str) -> Option<[f32; 3]> {
    let mut rest = json;
    while let Some(pos) = rest.find(key) {
        let after = &rest[pos + key.len()..];
        if let Some(v) = triple(after) {
            return Some(v);
        }
        rest = &rest[pos + key.len()..];
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Clip extraction must read the `animations` array and nothing else:
    /// meshes, nodes and materials all carry `"name"` too, and a global scan
    /// would report mesh names as playable animation states.
    #[test]
    fn clip_names_come_only_from_the_animations_array() {
        let json = r#"{"asset":{},"meshes":[{"name":"Cube.003"}],
            "nodes":[{"name":"Armature"}],
            "animations":[{"name":"Idle","channels":[{"target":{"path":"translation"}}]},
                          {"name":"Walk_A","samplers":[{"input":0}]}],
            "materials":[{"name":"Atlas"}]}"#;
        assert_eq!(clip_names(json), vec!["Idle", "Walk_A"]);
    }

    #[test]
    fn clip_names_survive_escapes_and_missing_animations() {
        let json = r#"{"animations":[{"name":"Say \"Hi\""},{"name":"Wave"}]}"#;
        assert_eq!(clip_names(json), vec!["Say \\\"Hi\\\"", "Wave"]);
        assert!(clip_names(r#"{"meshes":[{"name":"X"}]}"#).is_empty());
    }

    #[test]
    fn joint_count_reads_the_first_skin() {
        assert_eq!(joint_count(r#"{"skins":[{"joints":[1,2,3,4]}]}"#), 4);
        assert_eq!(joint_count(r#"{"meshes":[]}"#), 0);
    }

    /// A plain .gltf is the JSON itself — Quaternius ships those with the
    /// buffer base64'd inline, so there is no binary chunk to unwrap.
    #[test]
    fn plain_gltf_is_recognised_and_junk_is_not() {
        let ok = br#"{"asset":{"version":"2.0"},"animations":[{"name":"Idle"}]}"#;
        assert!(gltf_json(ok).is_some());
        assert!(gltf_json(b"not json at all").is_none());
        assert!(gltf_json(br#"{"nope":1}"#).is_none());
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::*;

    /// Kenney's exporter writes "max" BEFORE "min". Searching for "max" only
    /// after "min" found nothing, so every Kenney model indexed with no size
    /// and the big/small filters silently matched nothing.
    #[test]
    fn bounds_do_not_depend_on_key_order() {
        let kenney = r#"{"accessors":[{"type":"VEC3","max":[0.3,1.02,0.125],"min":[-0.3,0.0,-0.1]}]}"#;
        let other = r#"{"accessors":[{"type":"VEC3","min":[-0.3,0.0,-0.1],"max":[0.3,1.02,0.125]}]}"#;
        let a = bounds(kenney).expect("max-first must parse");
        let b = bounds(other).expect("min-first must parse");
        assert!((a[0] - 0.6).abs() < 1e-5 && (a[1] - 1.02).abs() < 1e-5);
        assert_eq!(a, b);
    }
}
