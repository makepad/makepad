//! Reference blobs: content the store catalogues without copying.
//!
//! The properties proven here are the ones that make "we did not copy your
//! file" a safe promise rather than a hopeful one:
//! - admitting by reference writes NOTHING into the CAS and leaves the
//!   original file untouched, byte for byte,
//! - a reference serves exactly the bytes it names, through the same
//!   `read_blob` chokepoint every HTTP route uses,
//! - a file that vanished, changed length, or changed CONTENT at the same
//!   length is refused with a distinct, honest reason — never served, never
//!   silently substituted,
//! - a manifest may reference such a blob and publish normally: the content
//!   contract cannot tell the difference, which is the point,
//! - blob GC forgets an unreferenced reference row WITHOUT EVER DELETING THE
//!   USER'S FILE,
//! - and re-scanning reports each reference's live state so a UI can show
//!   what went stale.

mod common;

use common::*;
use makepad_asset_data::*;
use makepad_asset_store::*;
use std::path::{Path, PathBuf};

/// An external directory, outside any store root: this is the "somewhere
/// else" the user's video library lives.
fn external_dir(name: &str) -> PathBuf {
    let dir = test_root(&format!("external_{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Every regular file under `root`, relative and sorted — used to assert the
/// CAS gained nothing.
fn tree(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.display().to_string());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn reference_admission_copies_nothing_and_serves_the_bytes() {
    let (root, core) = open_core("blobref_serve");
    let ext = external_dir("blobref_serve");
    let clip = b"MP4-BYTES-THAT-ARE-NOT-THE-STORES-TO-OWN".to_vec();
    let path = write(&ext, "opener.mp4", &clip);

    let before = tree(&root.join("cas"));
    let commit = core.put_blob_ref(&path, NOW).unwrap();
    assert_eq!(commit.blob_id, BlobId::hash_of(&clip));
    assert_eq!(commit.size, clip.len() as u64);
    assert!(!commit.deduped);
    assert!(!commit.owned, "the store must not claim to own a referenced file");

    // The CAS gained no object, and the original is untouched.
    assert_eq!(tree(&root.join("cas")), before, "reference import wrote into the CAS");
    assert_eq!(std::fs::read(&path).unwrap(), clip);
    assert!(!core.cas().contains(&commit.blob_id));

    // …and the blob nonetheless serves, through the one path every HTTP
    // route uses.
    assert_eq!(core.read_blob(&commit.blob_id).unwrap(), clip);
    assert_eq!(core.catalog().blob_size(&commit.blob_id).unwrap(), Some(clip.len() as u64));
    assert_eq!(core.verify_blob_ref(&commit.blob_id).unwrap(), Some(RefState::Present));

    std::fs::remove_dir_all(&ext).ok();
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn re_admitting_the_same_file_is_idempotent() {
    let (root, core) = open_core("blobref_idempotent");
    let ext = external_dir("blobref_idempotent");
    let path = write(&ext, "loop.mp4", b"SAME-BYTES");

    let first = core.put_blob_ref(&path, NOW).unwrap();
    assert!(!first.deduped);
    let second = core.put_blob_ref(&path, NOW + 1).unwrap();
    assert_eq!(second.blob_id, first.blob_id);
    assert!(second.deduped, "a second scan of unchanged bytes is a dedupe hit");
    assert_eq!(core.blob_refs().count().unwrap(), 1);

    // A file that MOVED re-points the same digest: the digest is the
    // identity, the path is only where the bytes happen to live today.
    let moved = ext.join("moved.mp4");
    std::fs::rename(&path, &moved).unwrap();
    let third = core.put_blob_ref(&moved, NOW + 2).unwrap();
    assert_eq!(third.blob_id, first.blob_id);
    assert_eq!(core.blob_refs().count().unwrap(), 1);
    assert_eq!(core.blob_refs().lookup(&first.blob_id).unwrap().unwrap().path, moved);
    assert_eq!(core.read_blob(&first.blob_id).unwrap(), b"SAME-BYTES");

    std::fs::remove_dir_all(&ext).ok();
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn bytes_the_store_already_owns_are_never_downgraded_to_a_reference() {
    let (root, core) = open_core("blobref_owned");
    let ext = external_dir("blobref_owned");
    let bytes = b"OWNED-ALREADY".to_vec();
    core.put_blob(&bytes, NOW).unwrap();
    let path = write(&ext, "dup.mp4", &bytes);

    let commit = core.put_blob_ref(&path, NOW + 1).unwrap();
    assert!(commit.owned, "the CAS already holds these bytes");
    assert_eq!(core.blob_refs().count().unwrap(), 0, "no reference should be recorded");

    // Deleting the external file cannot hurt a blob the store owns.
    std::fs::remove_file(&path).unwrap();
    assert_eq!(core.read_blob(&commit.blob_id).unwrap(), bytes);

    std::fs::remove_dir_all(&ext).ok();
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_changed_or_missing_file_refuses_loudly_and_never_serves() {
    let (root, core) = open_core("blobref_drift");
    let ext = external_dir("blobref_drift");
    let path = write(&ext, "clip.mp4", b"ELEVEN-CHAR");
    let blob = core.put_blob_ref(&path, NOW).unwrap().blob_id;
    assert_eq!(core.read_blob(&blob).unwrap(), b"ELEVEN-CHAR");

    // Same length, different bytes: the digest is what catches this, and it
    // is reported as content drift rather than absence.
    std::fs::write(&path, b"ELEVEN-XXXX").unwrap();
    assert_eq!(core.verify_blob_ref(&blob).unwrap(), Some(RefState::ContentChanged));
    match core.read_blob(&blob) {
        Err(ServerError::DigestMismatch { what, .. }) => assert_eq!(what, "blob ref file"),
        other => panic!("content drift must refuse with a digest mismatch, got {other:?}"),
    }

    // Different length: caught before a byte is hashed.
    std::fs::write(&path, b"MUCH-LONGER-NOW").unwrap();
    assert_eq!(
        core.verify_blob_ref(&blob).unwrap(),
        Some(RefState::SizeChanged { expected: 11, found: 15 })
    );
    assert!(matches!(core.read_blob(&blob), Err(ServerError::SizeMismatch { .. })));

    // Gone entirely.
    std::fs::remove_file(&path).unwrap();
    assert_eq!(core.verify_blob_ref(&blob).unwrap(), Some(RefState::Missing));
    assert!(matches!(
        core.read_blob(&blob),
        Err(ServerError::NotFound { what: "blob ref file" })
    ));

    // Restoring the exact bytes restores the blob: the reference was never
    // wrong, only unavailable.
    std::fs::write(&path, b"ELEVEN-CHAR").unwrap();
    assert_eq!(core.verify_blob_ref(&blob).unwrap(), Some(RefState::Present));
    assert_eq!(core.read_blob(&blob).unwrap(), b"ELEVEN-CHAR");

    std::fs::remove_dir_all(&ext).ok();
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_manifest_cannot_tell_a_reference_from_an_owned_blob() {
    let (root, core) = open_core("blobref_publish");
    let ext = external_dir("blobref_publish");
    let glb = b"GLB-BYTES-LIVING-ELSEWHERE".to_vec();
    let thumb = vec![7u8; 900];
    let glb_path = write(&ext, "prop.glb", &glb);

    // The heavy payload is referenced; the derived thumbnail is owned, which
    // is exactly the split the VJ import uses.
    core.put_blob_ref(&glb_path, NOW).unwrap();
    core.put_blob(&thumb, NOW).unwrap();

    let id = asset_id_n(3);
    let manifest = prop_manifest(id, &glb, &thumb);
    let canonical = manifest.to_canonical_bytes().unwrap();
    let revision = manifest.revision().unwrap();
    core.catalog().register_asset(&id, "gen", NOW).unwrap();
    core.catalog().stage_asset_revision(&canonical, NOW).unwrap();
    core.catalog().publish_asset(&id, &revision, NOW).unwrap();

    // The revision is live and its referenced payload reads back.
    assert_eq!(
        core.catalog().asset_candidate_state(&id, &revision).unwrap(),
        Some(CandidateState::Published)
    );
    assert_eq!(core.read_blob(&BlobId::hash_of(&glb)).unwrap(), glb);

    std::fs::remove_dir_all(&ext).ok();
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn gc_forgets_an_unreferenced_reference_and_never_deletes_the_file() {
    let (root, core) = open_core("blobref_gc");
    let ext = external_dir("blobref_gc");
    let orphan = write(&ext, "unused.mp4", b"NOBODY-POINTS-AT-ME");
    let blob = core.put_blob_ref(&orphan, NOW).unwrap().blob_id;
    assert_eq!(core.blob_refs().count().unwrap(), 1);

    let status = core
        .gc_run(GcConfig { dry_run: false, grace_ms: 0, ..GcConfig::default_v1() }, 10_000, NOW)
        .unwrap();
    assert!(status.finished());
    assert_eq!(status.deleted_blobs, 1, "the catalog row is reclaimed");
    assert_eq!(
        status.deleted_bytes, 0,
        "no bytes were freed on this machine — the file is not the store's"
    );

    // THE POINT: the user's file is still exactly where they put it.
    assert!(orphan.is_file(), "GC deleted a file the store never owned");
    assert_eq!(std::fs::read(&orphan).unwrap(), b"NOBODY-POINTS-AT-ME");

    // And the store has honestly forgotten it.
    assert_eq!(core.blob_refs().count().unwrap(), 0);
    assert!(core.blob_refs().lookup(&blob).unwrap().is_none());
    assert!(matches!(core.read_blob(&blob), Err(ServerError::NotFound { .. })));

    std::fs::remove_dir_all(&ext).ok();
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn rescan_pages_the_library_and_names_each_state() {
    let (root, core) = open_core("blobref_rescan");
    let ext = external_dir("blobref_rescan");
    let mut paths = Vec::new();
    for i in 0..5u8 {
        paths.push(write(&ext, &format!("clip{i}.mp4"), &[i; 64]));
    }
    for path in &paths {
        core.put_blob_ref(path, NOW).unwrap();
    }
    assert_eq!(core.blob_refs().count().unwrap(), 5);

    // Break two of them in the two distinct ways.
    std::fs::remove_file(&paths[1]).unwrap();
    std::fs::write(&paths[3], [99u8; 64]).unwrap();

    // Walk the whole library two at a time, exactly as a UI would.
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = core.rescan_blob_refs(after.as_ref(), 2).unwrap();
        if page.entries.is_empty() {
            break;
        }
        for (entry, state) in &page.entries {
            seen.push((entry.path.clone(), *state));
        }
        after = page.next;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(seen.len(), 5, "every reference is visited exactly once");
    let state_of = |p: &PathBuf| seen.iter().find(|(path, _)| path == p).unwrap().1;
    assert_eq!(state_of(&paths[0]), RefState::Present);
    assert_eq!(state_of(&paths[1]), RefState::Missing);
    assert_eq!(state_of(&paths[2]), RefState::Present);
    assert_eq!(state_of(&paths[3]), RefState::ContentChanged);
    assert_eq!(state_of(&paths[4]), RefState::Present);

    std::fs::remove_dir_all(&ext).ok();
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn references_survive_a_reopen() {
    let root = test_root("blobref_reopen");
    let ext = external_dir("blobref_reopen");
    let path = write(&ext, "persist.mp4", b"ACROSS-RESTARTS");
    let blob = {
        let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
        core.put_blob_ref(&path, NOW).unwrap().blob_id
    };
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    assert_eq!(core.read_blob(&blob).unwrap(), b"ACROSS-RESTARTS");
    assert_eq!(core.verify_blob_ref(&blob).unwrap(), Some(RefState::Present));

    std::fs::remove_dir_all(&ext).ok();
    std::fs::remove_dir_all(&root).ok();
}
