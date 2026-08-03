//! Extraction is a security boundary: these archives are built to hurt us.
//!
//! The contract under test is threefold — never panic, never write outside the
//! destination, always terminate. A refusal is a pass; a successful extraction
//! of something benign is a pass; a panic or an escape is not.

use makepad_game_pkg::pack::{
    read_package, unpack, MAX_ENTRIES, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES,
};
use makepad_zip_file::{ZipMethod, ZipWriter};
use std::path::{Path, PathBuf};

/// Build an archive whose member names bypass ZipWriter's own validation, by
/// patching the names in after the fact. A real attacker writes the bytes
/// directly, so our writer's refusal to emit them proves nothing about the
/// reader — this is how we get a genuinely hostile input.
fn archive_with_raw_name(name: &str, data: &[u8]) -> Vec<u8> {
    let placeholder = "X".repeat(name.len());
    let mut w = ZipWriter::new();
    w.add(&placeholder, data, ZipMethod::Store).unwrap();
    w.add("manifest.toml", b"name = \"evil\"\n", ZipMethod::Store)
        .unwrap();
    w.add("game.splash", b"", ZipMethod::Store).unwrap();
    let mut bytes = w.finish().unwrap();
    // Overwrite every occurrence of the placeholder (local header + central
    // directory) with the hostile name — same length, so all offsets hold.
    let pat = placeholder.as_bytes();
    let mut i = 0;
    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] == pat {
            bytes[i..i + pat.len()].copy_from_slice(name.as_bytes());
            i += pat.len();
        } else {
            i += 1;
        }
    }
    bytes
}

fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "makepad-pkg-hostile-{tag}-{}-{:p}",
        std::process::id(),
        &tag
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                n += count_files(&p);
            } else {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn traversal_names_never_escape_the_destination() {
    let sandbox = tmp_dir("traversal");
    let dest = sandbox.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    // A canary next to the destination: if extraction escapes, it dies here.
    let canary = sandbox.join("canary.txt");
    std::fs::write(&canary, b"original").unwrap();

    for name in [
        "../canary.txt",
        "../../canary.txt",
        "a/../../canary.txt",
        "/etc/hosts",
        "..\\canary.txt",
        "C:/canary.txt",
        "./../canary.txt",
    ] {
        let archive = archive_with_raw_name(name, b"overwritten");
        // Refusal is the expected outcome; success would mean it wrote
        // something, and the canary check below proves where.
        let _ = unpack(&archive, &dest);
        assert_eq!(
            std::fs::read(&canary).unwrap(),
            b"original",
            "archive with name {name:?} escaped the destination"
        );
    }
    let _ = std::fs::remove_dir_all(&sandbox);
}

#[test]
fn a_symlink_member_is_refused() {
    // Craft a member whose external attributes mark it S_IFLNK. Writing the
    // link target as content would be an escape on extraction.
    let mut w = ZipWriter::new();
    w.add("manifest.toml", b"name = \"evil\"\n", ZipMethod::Store)
        .unwrap();
    w.add("game.splash", b"", ZipMethod::Store).unwrap();
    w.add("assets/link", b"/etc/passwd", ZipMethod::Store).unwrap();
    let mut bytes = w.finish().unwrap();

    // Patch the central directory entry for assets/link: external attributes
    // sit 38 bytes into the 46-byte header.
    let needle = b"assets/link";
    let mut positions = Vec::new();
    for i in 0..bytes.len().saturating_sub(needle.len()) {
        if &bytes[i..i + needle.len()] == needle {
            positions.push(i);
        }
    }
    // The last occurrence is the central directory copy.
    let name_at = *positions.last().unwrap();
    let header_at = name_at - 46;
    let attrs = (0xA1FFu32) << 16;
    bytes[header_at + 38..header_at + 42].copy_from_slice(&attrs.to_le_bytes());

    let err = read_package(&bytes).unwrap_err();
    assert!(
        format!("{err}").contains("symlink"),
        "expected a symlink refusal, got {err}"
    );
}

#[test]
fn declared_size_bombs_are_refused_before_decompression() {
    // A member that DECLARES more than the per-entry cap, without ever
    // containing it: the check must read the header, not the payload.
    let mut w = ZipWriter::new();
    w.add("manifest.toml", b"name = \"bomb\"\n", ZipMethod::Store)
        .unwrap();
    w.add("game.splash", b"", ZipMethod::Store).unwrap();
    w.add("assets/bomb.bin", b"small", ZipMethod::Store).unwrap();
    let mut bytes = w.finish().unwrap();

    let needle = b"assets/bomb.bin";
    let mut positions = Vec::new();
    for i in 0..bytes.len().saturating_sub(needle.len()) {
        if &bytes[i..i + needle.len()] == needle {
            positions.push(i);
        }
    }
    let header_at = positions.last().unwrap() - 46;
    // uncompressed_size is 24 bytes into the central directory header.
    let huge = (MAX_ENTRY_BYTES + 1).min(u32::MAX as u64) as u32;
    bytes[header_at + 24..header_at + 28].copy_from_slice(&huge.to_le_bytes());

    let err = read_package(&bytes).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("declares"), "expected a size refusal, got {msg}");
}

#[test]
fn a_real_deflate_bomb_is_refused() {
    // 40 MB of zeros compresses to a few KB. Declared honestly, so this tests
    // the total cap rather than a header lie.
    let big = vec![0u8; 40 * 1024 * 1024];
    let mut w = ZipWriter::new();
    w.add("manifest.toml", b"name = \"bomb\"\n", ZipMethod::Store)
        .unwrap();
    w.add("game.splash", b"", ZipMethod::Store).unwrap();
    for i in 0..8 {
        w.add(&format!("assets/b{i}.bin"), &big, ZipMethod::Deflate)
            .unwrap();
    }
    let bytes = w.finish().unwrap();
    assert!(
        bytes.len() < 1024 * 1024,
        "the bomb should be small on the wire ({} bytes)",
        bytes.len()
    );
    let err = read_package(&bytes).unwrap_err();
    assert!(
        format!("{err}").contains("total size cap"),
        "expected the total cap to fire, got {err}"
    );
    assert!(8u64 * big.len() as u64 > MAX_TOTAL_BYTES);
}

#[test]
fn entry_count_is_capped() {
    let mut w = ZipWriter::new();
    w.add("manifest.toml", b"name = \"many\"\n", ZipMethod::Store)
        .unwrap();
    w.add("game.splash", b"", ZipMethod::Store).unwrap();
    for i in 0..MAX_ENTRIES + 10 {
        w.add(&format!("assets/f{i}.bin"), b"x", ZipMethod::Store)
            .unwrap();
    }
    let bytes = w.finish().unwrap();
    let err = read_package(&bytes).unwrap_err();
    assert!(format!("{err}").contains("too many entries"), "got {err}");
}

#[test]
fn duplicate_members_are_refused() {
    // Two members with the same name: which one wins is ambiguous, and the
    // ambiguity is the attack (validate one, extract the other).
    let mut w = ZipWriter::new();
    w.add("manifest.toml", b"name = \"dup\"\n", ZipMethod::Store)
        .unwrap();
    w.add("game.splash", b"first", ZipMethod::Store).unwrap();
    w.add("game.splashX", b"second", ZipMethod::Store).unwrap();
    let mut bytes = w.finish().unwrap();
    // Rename the second member to collide with the first.
    let pat = b"game.splashX";
    for i in 0..bytes.len().saturating_sub(pat.len()) {
        if &bytes[i..i + pat.len()] == pat {
            bytes[i..i + pat.len()].copy_from_slice(b"game.splash\0");
        }
    }
    // The NUL makes it a distinct-but-hostile name; either refusal is correct.
    let err = read_package(&bytes).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("duplicate") || msg.contains("NUL"),
        "expected duplicate or NUL refusal, got {msg}"
    );
}

#[test]
fn truncated_and_garbage_archives_are_refused_not_panics() {
    let mut w = ZipWriter::new();
    w.add("manifest.toml", b"name = \"ok\"\n", ZipMethod::Deflate)
        .unwrap();
    w.add("game.splash", b"game.sky({})", ZipMethod::Deflate)
        .unwrap();
    let good = w.finish().unwrap();

    for cut in [0, 1, 5, 21, 22, 30, good.len() / 2, good.len() - 1] {
        let _ = read_package(&good[..cut.min(good.len())]);
    }
    for garbage in [
        vec![0u8; 64],
        vec![0xffu8; 512],
        b"PK\x03\x04 not really a zip".to_vec(),
    ] {
        let _ = read_package(&garbage);
    }
}

#[test]
fn mutation_fuzz_never_panics_or_escapes() {
    // Seeded LCG: deterministic, so a failure is reproducible.
    let mut seed: u64 = 0x5EED_1234_ABCD_0001;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };

    let mut w = ZipWriter::new();
    w.add(
        "manifest.toml",
        b"name = \"fuzz\"\ndescription = \"seed\"\n",
        ZipMethod::Deflate,
    )
    .unwrap();
    w.add("game.splash", b"game.box({pos: vec3(0,0,0)})\n", ZipMethod::Deflate)
        .unwrap();
    w.add("assets/a.bin", &vec![3u8; 4096], ZipMethod::Deflate)
        .unwrap();
    let base = w.finish().unwrap();

    let sandbox = tmp_dir("fuzz");
    let dest = sandbox.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let canary = sandbox.join("canary.txt");
    std::fs::write(&canary, b"original").unwrap();

    let mut accepted = 0usize;
    for round in 0..4000 {
        let mut m = base.clone();
        // 1-6 byte flips per round, biased toward the headers where the
        // interesting fields live.
        let flips = 1 + (next() % 6) as usize;
        for _ in 0..flips {
            let pos = if next() % 3 == 0 {
                (next() as usize) % m.len()
            } else {
                // Central directory / EOCD region.
                m.len() - 1 - ((next() as usize) % m.len().min(256))
            };
            m[pos] ^= 1 << (next() % 8);
        }

        match read_package(&m) {
            Ok(_) => {
                accepted += 1;
                // If it parsed, writing it must also stay inside dest.
                let _ = unpack(&m, &dest);
            }
            Err(_) => {}
        }
        assert_eq!(
            std::fs::read(&canary).unwrap(),
            b"original",
            "fuzz round {round} escaped the destination"
        );
    }
    // Sanity: the corpus must not be so broken that nothing ever parses, or
    // this test would be asserting nothing.
    assert!(
        accepted > 0,
        "no mutated archive ever parsed — the fuzz corpus is not exercising the reader"
    );
    let _ = std::fs::remove_dir_all(&sandbox);
}

#[test]
fn a_benign_package_still_round_trips() {
    // The counterweight to all of the above: hardening that rejects everything
    // would pass every test here and be useless.
    let mut w = ZipWriter::new();
    w.add(
        "manifest.toml",
        b"name = \"Speedway\"\ndescription = \"race\"\nplayers_max = 4\n",
        ZipMethod::Deflate,
    )
    .unwrap();
    w.add("game.splash", b"game.terrain({size: 100})\n", ZipMethod::Deflate)
        .unwrap();
    w.add("assets/car.bin", &vec![9u8; 1000], ZipMethod::Deflate)
        .unwrap();
    let bytes = w.finish().unwrap();

    let sandbox = tmp_dir("benign");
    let dest = sandbox.join("game");
    let manifest = unpack(&bytes, &dest).unwrap();
    assert_eq!(manifest.name, "Speedway");
    assert_eq!(manifest.players_max, 4);
    assert_eq!(
        std::fs::read_to_string(dest.join("game.splash")).unwrap(),
        "game.terrain({size: 100})\n"
    );
    assert_eq!(
        std::fs::read(dest.join("assets/car.bin")).unwrap(),
        vec![9u8; 1000]
    );
    assert_eq!(count_files(&dest), 3);
    let _ = std::fs::remove_dir_all(&sandbox);
}
