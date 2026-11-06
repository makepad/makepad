//! The proof hash format, shared with devhub. No Cargo dependencies.
use std::collections::{BTreeMap, BTreeSet};

/// Sorted blob paths/object IDs and external package checksums; proofs exclude themselves.
pub fn digest_input(lines: &str, external: &[String]) -> String {
    let mut rows: BTreeSet<String> = lines.lines().filter_map(|line| {
        let (head, path) = line.split_once('\t')?;
        if path.rsplit('/').next() == Some("ci.toml") || head.split_whitespace().nth(1) != Some("blob") {
            return None;
        }
        Some(format!("{path} {}", head.split_whitespace().nth(2)?))
    }).collect();
    rows.extend(external.iter().cloned());
    rows.into_iter().map(|line| format!("{line}\n")).collect()
}

/// Documents: licences, readmes, credits, notes (`*.md`, `*.rst`, `LICENSE*` and the like),
/// outside a `resources/` folder (an app loads its resources at run time).
pub fn doc_file(path: &str) -> bool {
    const DOCS: [&str; 13] = ["license", "licence", "copying", "unlicense", "notice", "notices", "authors", "credits", "changelog", "readme", "contributing", "attribution", "third_party_notices"];
    if path.split('/').any(|c| c == "resources") {
        return false;
    }
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if [".md", ".markdown", ".rst"].iter().any(|e| name.ends_with(e)) {
        return true;
    }
    let stem = name.split('.').next().unwrap_or("");
    DOCS.iter().any(|d| stem.strip_prefix(d).is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '_'])))
}

/// A file of a crate its proofs leave out unless a Rust source of that crate names it
/// ([`names`]; `include_str!("../README.md")` names `README.md`): a document ([`doc_file`]), and
/// for the crate's tests its `ci.splash`, the app's smoke script, which no test runs.
pub fn inert(path: &str, tests: bool) -> bool {
    doc_file(path) || (tests && path.rsplit('/').next() == Some("ci.splash"))
}

/// Whether a Rust source names a file: its text holds the file's name.
pub fn names(source: &str, path: &str) -> bool {
    source.contains(path.rsplit('/').next().unwrap_or(path))
}

/// Of a unit's [`inert`] files, the crate each is in: the deepest of the unit's crate dirs
/// (repo paths, no trailing `/`) that holds it.
pub fn inert_by_crate<'a>(paths: impl Iterator<Item = &'a str>, crates: &BTreeSet<String>, tests: bool) -> BTreeMap<String, Vec<&'a str>> {
    let mut out: BTreeMap<String, Vec<&'a str>> = BTreeMap::new();
    for path in paths.filter(|p| inert(p, tests)) {
        let holds = |d: &&String| d.is_empty() || (path.starts_with(d.as_str()) && path.as_bytes().get(d.len()) == Some(&b'/'));
        if let Some(dir) = crates.iter().filter(holds).max_by_key(|d| d.len()) {
            out.entry(dir.clone()).or_default().push(path);
        }
    }
    out
}

pub fn digest_platform(input: &str, os: &str) -> String {
    finish(midstate(input.as_bytes()), platform(os).as_bytes())
}

/// [`digest_platform`] for each of `oses`, the input hashed once: a unit's input holds every file
/// of its crates (the JIT's for an app), and hashing it again for each OS was a third of the
/// master's gate-proof time after each landing (11-06).
pub fn digest_platforms<const N: usize>(input: &str, oses: [&str; N]) -> [String; N] {
    let at = midstate(input.as_bytes());
    oses.map(|os| finish(at, platform(os).as_bytes()))
}

fn platform(os: &str) -> String {
    format!("platform={os}\n")
}

/// SHA-256, FIPS 180-4.
pub fn sha256(bytes: &[u8]) -> String {
    finish(midstate(bytes), &[])
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// A hash over a message's whole 64-byte blocks: its state, the message's length and the bytes
/// past the last whole block, which [`finish`] hashes with any suffix.
#[derive(Clone, Copy)]
struct Midstate<'a> {
    h: [u32; 8],
    len: usize,
    tail: &'a [u8],
}

fn midstate(bytes: &[u8]) -> Midstate<'_> {
    let mut h = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let whole = bytes.len() / 64 * 64;
    for block in bytes[..whole].chunks_exact(64) {
        compress(&mut h, block);
    }
    Midstate { h, len: bytes.len(), tail: &bytes[whole..] }
}

/// The hash of the message `at` was taken over followed by `suffix`, as hex.
fn finish(at: Midstate, suffix: &[u8]) -> String {
    let mut h = at.h;
    let mut data = Vec::with_capacity(at.tail.len() + suffix.len() + 72);
    data.extend_from_slice(at.tail);
    data.extend_from_slice(suffix);
    data.push(0x80);
    while data.len() % 64 != 56 { data.push(0); }
    data.extend_from_slice(&((at.len + suffix.len()) as u64).wrapping_mul(8).to_be_bytes());
    for block in data.chunks_exact(64) {
        compress(&mut h, block);
    }
    h.iter().map(|v| format!("{v:08x}")).collect()
}

fn compress(h: &mut [u32; 8], block: &[u8]) {
    let mut w = [0u32; 64];
    for (i, word) in block.chunks_exact(4).enumerate() {
        w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
    }
    for i in 16..64 {
        let a = w[i - 15];
        let b = w[i - 2];
        w[i] = w[i - 16].wrapping_add(a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3))
            .wrapping_add(w[i - 7]).wrapping_add(b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10));
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..64 {
        let t1 = hh.wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
            .wrapping_add((e & f) ^ (!e & g)).wrapping_add(K[i]).wrapping_add(w[i]);
        let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
            .wrapping_add((a & b) ^ (a & c) ^ (b & c));
        hh = g; g = f; f = e; e = d.wrapping_add(t1);
        d = c; c = b; b = a; a = t1.wrapping_add(t2);
    }
    for (v, add) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) { *v = v.wrapping_add(add); }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hash before [`midstate`]: the whole message copied and hashed per call.
    fn old_sha256(bytes: &[u8]) -> String {
        let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
        let mut data = bytes.to_vec();
        data.push(0x80);
        while data.len() % 64 != 56 { data.push(0); }
        data.extend_from_slice(&(bytes.len() as u64).wrapping_mul(8).to_be_bytes());
        for block in data.chunks_exact(64) {
            let mut w = [0u32; 64];
            for (i, word) in block.chunks_exact(4).enumerate() {
                w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
            }
            for i in 16..64 {
                let a = w[i - 15];
                let b = w[i - 2];
                w[i] = w[i - 16].wrapping_add(a.rotate_right(7) ^ a.rotate_right(18) ^ (a >> 3))
                    .wrapping_add(w[i - 7]).wrapping_add(b.rotate_right(17) ^ b.rotate_right(19) ^ (b >> 10));
            }
            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for i in 0..64 {
                let t1 = hh.wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                    .wrapping_add((e & f) ^ (!e & g)).wrapping_add(K[i]).wrapping_add(w[i]);
                let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                    .wrapping_add((a & b) ^ (a & c) ^ (b & c));
                hh = g; g = f; f = e; e = d.wrapping_add(t1);
                d = c; c = b; b = a; a = t1.wrapping_add(t2);
            }
            for (v, add) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) { *v = v.wrapping_add(add); }
        }
        h.iter().map(|v| format!("{v:08x}")).collect()
    }

    /// Every OS's digest of an input hashed once is the digest of that input and its OS line
    /// hashed whole, at every length about a block's edges and past a megabyte: a proof digest
    /// that changed would prove a unit on sources it never ran.
    #[test]
    fn an_input_hashed_once_gives_every_platforms_digest_as_before() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut text = |len: usize| -> String {
            (0..len).map(|_| {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (b' ' + (seed >> 33) as u8 % 95) as char
            }).collect()
        };
        let oses = ["linux", "macos", "windows"];
        for len in (0..300).chain([1000, 4095, 4096, 4097, 1 << 20, (1 << 20) + 37]) {
            let input = text(len);
            let all = digest_platforms(&input, oses);
            for (os, digest) in oses.iter().zip(&all) {
                let whole = old_sha256(format!("{input}platform={os}\n").as_bytes());
                assert_eq!(digest, &whole, "{len} bytes, {os}");
                assert_eq!(digest_platform(&input, os), whole, "{len} bytes, {os}");
            }
            assert_eq!(sha256(input.as_bytes()), old_sha256(input.as_bytes()), "{len} bytes");
        }
    }

    #[test]
    fn documents_and_a_tests_ci_splash_are_left_to_their_deepest_crate() {
        for doc in ["README.md", "libs/a/LICENSE-MIT", "libs/a/LICENSE", "libs/a/COPYING", "libs/a/docs/notes.MD", "libs/a/CREDITS.md", "libs/a/LICENSE.txt", "libs/a/THIRD_PARTY_NOTICES.md", "libs/a/README.rst", "libs/a/LICENSE_APACHE"] {
            assert!(doc_file(doc), "{doc}");
        }
        // an app's resources load at run time; data, sources and manifests are code
        for code in ["apps/x/resources/help.md", "libs/a/src/lib.rs", "libs/a/data.txt", "libs/a/src/licenses.rs", "libs/a/LICENSEE", "libs/a/ci.splash", "libs/a/Cargo.toml"] {
            assert!(!doc_file(code), "{code}");
        }
        assert!(inert("apps/x/ci.splash", true) && !inert("apps/x/ci.splash", false) && !inert("apps/x/main.splash", true));
        let crates: BTreeSet<String> = ["libs/a", "libs/a/b", "apps/x"].map(String::from).into();
        let by = inert_by_crate(["libs/a/README.md", "libs/a/b/README.md", "libs/ab/README.md", "libs/a/src/lib.rs", "apps/x/ci.splash"].into_iter(), &crates, true);
        assert_eq!(by.into_iter().collect::<Vec<_>>(), vec![
            ("apps/x".to_string(), vec!["apps/x/ci.splash"]),
            ("libs/a".to_string(), vec!["libs/a/README.md"]),
            ("libs/a/b".to_string(), vec!["libs/a/b/README.md"]),
        ]);
        assert!(names("#![doc = include_str!(\"../README.md\")]", "libs/a/README.md"));
        assert!(!names("// see the docs", "libs/a/README.md"));
    }
}
