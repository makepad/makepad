//! CAS behavior: streaming admission, dedup, atomicity, restart recovery,
//! and corruption refusal.

mod common;
use common::*;
use makepad_asset_store::{AssetServerCore, Budgets, ServerError};
use makepad_asset_data::BlobId;
use std::fs;

#[test]
fn put_read_roundtrip_and_digest() {
    let (_root, core) = open_core("roundtrip");
    let bytes = b"hello content addressed world".to_vec();
    let commit = core.put_blob(&bytes, NOW).unwrap();
    assert_eq!(commit.blob_id, BlobId::hash_of(&bytes));
    assert_eq!(commit.size, bytes.len() as u64);
    assert!(!commit.deduped);
    assert_eq!(core.read_blob(&commit.blob_id).unwrap(), bytes);
}

#[test]
fn streaming_write_with_predeclared_digest() {
    let (_root, core) = open_core("stream");
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let expected = BlobId::hash_of(&data);
    let mut w = core.begin_blob().unwrap();
    for chunk in data.chunks(7919) {
        w.write(chunk).unwrap();
    }
    let commit = core.commit_blob(w, Some(expected), NOW).unwrap();
    assert_eq!(commit.blob_id, expected);
    assert_eq!(core.read_blob(&expected).unwrap(), data);
}

#[test]
fn predeclared_digest_mismatch_refused_and_nothing_lands() {
    let (root, core) = open_core("mismatch");
    let wrong = BlobId::hash_of(b"some other bytes");
    let mut w = core.begin_blob().unwrap();
    w.write(b"actual bytes").unwrap();
    let err = core.commit_blob(w, Some(wrong), NOW).unwrap_err();
    assert!(matches!(err, ServerError::DigestMismatch { .. }), "{err}");
    // Nothing at the final path, nothing recorded, temp cleaned up.
    assert!(!core.cas().contains(&BlobId::hash_of(b"actual bytes")));
    assert!(matches!(
        core.read_blob(&BlobId::hash_of(b"actual bytes")).unwrap_err(),
        ServerError::NotFound { .. }
    ));
    let tmp_entries = fs::read_dir(root.join("cas/tmp")).unwrap().count();
    assert_eq!(tmp_entries, 0);
}

#[test]
fn dedup_second_identical_write() {
    let (_root, core) = open_core("dedup");
    let bytes = b"identical payload".to_vec();
    let first = core.put_blob(&bytes, NOW).unwrap();
    assert!(!first.deduped);
    let second = core.put_blob(&bytes, NOW + 1).unwrap();
    assert!(second.deduped);
    assert_eq!(first.blob_id, second.blob_id);
    assert_eq!(core.read_blob(&first.blob_id).unwrap(), bytes);
}

#[test]
fn oversize_blob_refused_mid_stream() {
    let root = test_root("oversize");
    let budgets = Budgets {
        max_blob_bytes: 8,
        ..Budgets::default_v1()
    };
    let core = AssetServerCore::open(&root, budgets).unwrap();
    let mut w = core.begin_blob().unwrap();
    let err = w.write(b"nine bytes").unwrap_err();
    assert!(matches!(err, ServerError::OverBudget { what: "blob bytes", .. }), "{err}");
}

#[test]
fn corrupt_object_refused_on_read() {
    let (root, core) = open_core("corrupt");
    let bytes = b"soon to be corrupted".to_vec();
    let commit = core.put_blob(&bytes, NOW).unwrap();
    // Flip one byte of the committed object on disk.
    let mut object_path = None;
    for fan in fs::read_dir(root.join("cas/objects")).unwrap() {
        for obj in fs::read_dir(fan.unwrap().path()).unwrap() {
            object_path = Some(obj.unwrap().path());
        }
    }
    let object_path = object_path.expect("committed object on disk");
    let mut on_disk = fs::read(&object_path).unwrap();
    on_disk[0] ^= 0xff;
    fs::write(&object_path, &on_disk).unwrap();

    let err = core.read_blob(&commit.blob_id).unwrap_err();
    assert!(matches!(err, ServerError::DigestMismatch { what: "cas object", .. }), "{err}");
}

#[test]
fn restart_recovers_orphan_temps_and_keeps_objects() {
    let root = test_root("restart");
    let committed = b"survives restart".to_vec();
    let blob_id;
    {
        let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
        blob_id = core.put_blob(&committed, NOW).unwrap().blob_id;
        // Simulate a crash mid-upload: an in-flight writer that never commits
        // and never runs its Drop cleanup.
        let mut w = core.begin_blob().unwrap();
        w.write(b"partial upload lost in a crash").unwrap();
        std::mem::forget(w);
    }
    assert_eq!(fs::read_dir(root.join("cas/tmp")).unwrap().count(), 1);

    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    let report = core.recover(NOW + 10).unwrap();
    assert_eq!(report.cas_temps_removed, 1);
    assert_eq!(fs::read_dir(root.join("cas/tmp")).unwrap().count(), 0);
    // The committed object is intact and still verifies.
    assert_eq!(core.read_blob(&blob_id).unwrap(), committed);
}

#[test]
fn stream_verified_emits_nothing_unless_whole_digest_verifies() {
    // Tiny chunk budget: the blob spans many read chunks, so a streaming
    // implementation that emitted per-chunk would have leaked most of the
    // blob before discovering the corrupt final byte.
    let root = test_root("stream_fail_closed");
    let budgets = Budgets {
        io_chunk_bytes: 8,
        ..Budgets::default_v1()
    };
    let core = AssetServerCore::open(&root, budgets).unwrap();
    let bytes: Vec<u8> = (0..100u8).collect();
    let commit = core.put_blob(&bytes, NOW).unwrap();

    // Happy path: whole-digest verification first, then the sink gets all.
    let mut sink = Vec::new();
    let n = core.cas().stream_verified(&commit.blob_id, &mut sink).unwrap();
    assert_eq!(n, bytes.len() as u64);
    assert_eq!(sink, bytes);

    // Missing object refuses with an untouched sink.
    let mut sink = Vec::new();
    let err = core
        .cas()
        .stream_verified(&BlobId::hash_of(b"absent"), &mut sink)
        .unwrap_err();
    assert!(matches!(err, ServerError::NotFound { what: "cas object" }), "{err}");
    assert!(sink.is_empty(), "refused stream leaked bytes into the sink");

    // Corrupt the LAST byte on disk: the mismatch is only detectable after
    // every chunk hashed, so fail-closed means zero bytes may have escaped.
    let mut object_path = None;
    for fan in fs::read_dir(root.join("cas/objects")).unwrap() {
        for obj in fs::read_dir(fan.unwrap().path()).unwrap() {
            object_path = Some(obj.unwrap().path());
        }
    }
    let object_path = object_path.expect("committed object on disk");
    let mut on_disk = fs::read(&object_path).unwrap();
    let last = on_disk.len() - 1;
    on_disk[last] ^= 0xff;
    fs::write(&object_path, &on_disk).unwrap();

    let mut sink = Vec::new();
    let err = core
        .cas()
        .stream_verified(&commit.blob_id, &mut sink)
        .unwrap_err();
    assert!(matches!(err, ServerError::DigestMismatch { what: "cas object", .. }), "{err}");
    assert!(sink.is_empty(), "corrupt stream leaked bytes into the sink");
}

#[test]
fn unrecorded_cas_object_is_invisible() {
    let (_root, core) = open_core("unrecorded");
    // Committed to the CAS directly, but never recorded in the catalog —
    // the crash window between CAS commit and catalog record. Reads fail
    // closed on the catalog; a later proper admission dedups and records.
    let bytes = b"orphan object".to_vec();
    let mut w = core.cas().begin().unwrap();
    w.write(&bytes).unwrap();
    let commit = core.cas().commit(w, None).unwrap();
    assert!(core.cas().contains(&commit.blob_id));
    assert!(matches!(
        core.read_blob(&commit.blob_id).unwrap_err(),
        ServerError::NotFound { what: "blob record" }
    ));
    let readmitted = core.put_blob(&bytes, NOW).unwrap();
    assert!(readmitted.deduped);
    assert_eq!(core.read_blob(&commit.blob_id).unwrap(), bytes);
}
