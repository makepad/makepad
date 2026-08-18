//! Snapshot framing, scene tags, readiness policy, and structured refusals:
//! golden digests, roundtrips, truncation totality, budgets, and the
//! wrong-ticket/stale-acknowledgement rules the protocol depends on.

mod common;

use common::*;
use makepad_asset_data::*;

// Frozen goldens for CONTENT_SCHEMA_VERSION 3. The stream digest covers
// section payload bytes, which the version bump does not touch; the begin
// document embeds the version header and therefore moved.
const GOLDEN_SNAPSHOT_STREAM: &str =
    "snap_1d694e336100a9ba95a2aae56d681b0403f1915ccaadb9bd557a4c210afd25cd";
const GOLDEN_SNAPSHOT_BEGIN_DOC: &str =
    "sha256:f4ecf4eda60aebcca63668a87d17e83767c11089be09584ee3e1956179faef49";

fn scene_tag() -> SceneTag {
    SceneTag {
        realm_epoch: RealmEpoch(3),
        scene_sequence: SceneSequence(8),
    }
}

fn begin() -> SnapshotBegin {
    SnapshotBegin {
        snapshot_id: SnapshotId(11),
        ticket: ticket(1),
        scene: scene_tag(),
        snapshot_tick: Tick(90_000),
        counts: SnapshotCounts {
            descriptor: 3,
            kit_state: 2,
            entity_state: 3,
            player_body_mount: 1,
            terrain_structure: 0,
            terrain_voxel: 2,
        },
    }
}

/// Deterministic fixture chunk stream matching `begin()`'s counts.
fn chunks() -> Vec<SnapshotChunk> {
    let mk = |section, first, count, fill: u8, len: usize| SnapshotChunk {
        snapshot_id: SnapshotId(11),
        scene: scene_tag(),
        section,
        first_record: first,
        record_count: count,
        payload: vec![fill; len],
    };
    vec![
        mk(SnapshotSection::Descriptor, 0, 2, 0xd0, 64),
        mk(SnapshotSection::Descriptor, 2, 1, 0xd1, 32),
        mk(SnapshotSection::KitState, 0, 2, 0x4b, 48),
        mk(SnapshotSection::EntityState, 0, 3, 0xe0, 96),
        mk(SnapshotSection::PlayerBodyMount, 0, 1, 0xb0, 16),
        // terrain_structure declares zero records: no chunks at all.
        mk(SnapshotSection::TerrainVoxel, 0, 2, 0x70, 128),
    ]
}

fn assembled_digest() -> SnapshotDigest {
    let mut b = SnapshotDigestBuilder::new();
    for c in chunks() {
        b.add_chunk(&c);
    }
    b.finalize()
}

#[test]
fn snapshot_stream_golden_digest_and_full_assembly() {
    let digest = assembled_digest();
    assert_eq!(digest.to_string(), GOLDEN_SNAPSHOT_STREAM);

    let mut asm = SnapshotAssembler::new(begin()).unwrap();
    for c in chunks() {
        asm.accept(&c).unwrap();
    }
    asm.finish(&SnapshotEnd {
        snapshot_id: SnapshotId(11),
        scene: scene_tag(),
        digest,
    })
    .unwrap();

    // The begin document itself is canonical: golden blob identity.
    let bytes = begin().to_canonical_bytes().unwrap();
    assert_eq!(
        BlobId::hash_of(&bytes).to_string(),
        GOLDEN_SNAPSHOT_BEGIN_DOC
    );
}

#[test]
fn snapshot_docs_roundtrip_and_are_total() {
    let begin_bytes = begin().to_canonical_bytes().unwrap();
    assert_eq!(
        SnapshotBegin::from_canonical_bytes(&begin_bytes).unwrap(),
        begin()
    );
    let chunk = &chunks()[0];
    let chunk_bytes = chunk.to_canonical_bytes().unwrap();
    assert_eq!(
        &SnapshotChunk::from_canonical_bytes(&chunk_bytes).unwrap(),
        chunk
    );
    let end = SnapshotEnd {
        snapshot_id: SnapshotId(11),
        scene: scene_tag(),
        digest: assembled_digest(),
    };
    let end_bytes = end.to_canonical_bytes().unwrap();
    assert_eq!(SnapshotEnd::from_canonical_bytes(&end_bytes).unwrap(), end);
    let ready = SnapshotReady {
        snapshot_id: SnapshotId(11),
        ticket: ticket(1),
        resolution: rres(5),
    };
    let ready_bytes = ready.to_canonical_bytes().unwrap();
    assert_eq!(
        SnapshotReady::from_canonical_bytes(&ready_bytes).unwrap(),
        ready
    );

    for (bytes, name) in [
        (begin_bytes, "begin"),
        (chunk_bytes, "chunk"),
        (end_bytes, "end"),
        (ready_bytes, "ready"),
    ] {
        for len in 0..bytes.len() {
            let r = match name {
                "begin" => SnapshotBegin::from_canonical_bytes(&bytes[..len]).map(|_| ()),
                "chunk" => SnapshotChunk::from_canonical_bytes(&bytes[..len]).map(|_| ()),
                "end" => SnapshotEnd::from_canonical_bytes(&bytes[..len]).map(|_| ()),
                _ => SnapshotReady::from_canonical_bytes(&bytes[..len]).map(|_| ()),
            };
            assert!(r.is_err(), "{name} prefix {len} decoded");
        }
    }
}

#[test]
fn assembler_refuses_wrong_identity_and_order() {
    // Wrong snapshot id.
    let mut asm = SnapshotAssembler::new(begin()).unwrap();
    let mut c = chunks()[0].clone();
    c.snapshot_id = SnapshotId(12);
    assert!(matches!(asm.accept(&c), Err(AssetDataError::Mismatch { .. })));

    // Wrong scene tag (older sequence trying to feed the new snapshot).
    let mut c = chunks()[0].clone();
    c.scene.scene_sequence = SceneSequence(7);
    assert!(matches!(asm.accept(&c), Err(AssetDataError::Mismatch { .. })));

    // Section order violation: kit before descriptors complete is fine only
    // in fixed order — going BACK to an earlier section refuses.
    let mut asm = SnapshotAssembler::new(begin()).unwrap();
    let all = chunks();
    asm.accept(&all[0]).unwrap();
    asm.accept(&all[1]).unwrap();
    asm.accept(&all[2]).unwrap(); // KitState begins
    assert!(matches!(
        asm.accept(&all[0]),
        Err(AssetDataError::NotSorted { .. })
    ));

    // Non-contiguous records: skipping ahead refuses.
    let mut asm = SnapshotAssembler::new(begin()).unwrap();
    assert!(matches!(
        asm.accept(&all[1]), // first_record 2 while 0 expected
        Err(AssetDataError::Mismatch { .. })
    ));

    // Duplicate chunk replay refuses (contiguity again).
    let mut asm = SnapshotAssembler::new(begin()).unwrap();
    asm.accept(&all[0]).unwrap();
    assert!(matches!(
        asm.accept(&all[0]),
        Err(AssetDataError::Mismatch { .. })
    ));

    // More records than declared refuses.
    let mut asm = SnapshotAssembler::new(begin()).unwrap();
    let mut c = all[0].clone();
    c.record_count = 99;
    assert!(matches!(asm.accept(&c), Err(AssetDataError::OverBudget { .. })));
}

#[test]
fn assembler_refuses_incomplete_or_forged_end() {
    let all = chunks();

    // Missing a section's records at end.
    let mut asm = SnapshotAssembler::new(begin()).unwrap();
    for c in &all[..all.len() - 1] {
        asm.accept(c).unwrap();
    }
    assert!(matches!(
        asm.finish(&SnapshotEnd {
            snapshot_id: SnapshotId(11),
            scene: scene_tag(),
            digest: assembled_digest(),
        }),
        Err(AssetDataError::Missing { .. })
    ));

    // Forged digest.
    let mut asm = SnapshotAssembler::new(begin()).unwrap();
    for c in &all {
        asm.accept(c).unwrap();
    }
    assert!(matches!(
        asm.finish(&SnapshotEnd {
            snapshot_id: SnapshotId(11),
            scene: scene_tag(),
            digest: SnapshotDigest::hash_of(b"forged"),
        }),
        Err(AssetDataError::Mismatch { .. })
    ));

    // Altered payload bytes change the stream digest.
    let mut asm = SnapshotAssembler::new(begin()).unwrap();
    for (i, c) in all.iter().enumerate() {
        if i == 3 {
            let mut altered = c.clone();
            altered.payload[0] ^= 1;
            asm.accept(&altered).unwrap();
        } else {
            asm.accept(c).unwrap();
        }
    }
    assert!(matches!(
        asm.finish(&SnapshotEnd {
            snapshot_id: SnapshotId(11),
            scene: scene_tag(),
            digest: assembled_digest(),
        }),
        Err(AssetDataError::Mismatch { .. })
    ));

    // Begin whose scene epoch contradicts its own ticket refuses up front.
    let mut bad = begin();
    bad.scene.realm_epoch = RealmEpoch(9);
    assert!(matches!(
        SnapshotAssembler::new(bad),
        Err(AssetDataError::Mismatch { .. })
    ));
}

#[test]
fn snapshot_budgets_fail_closed() {
    // Oversized chunk payload.
    let mut c = chunks()[0].clone();
    c.payload = vec![0; 300 * 1024];
    assert!(matches!(
        c.to_canonical_bytes(),
        Err(AssetDataError::OverBudget { .. })
    ));

    // Zero-record or empty-payload chunks are framing garbage.
    let mut c = chunks()[0].clone();
    c.record_count = 0;
    assert!(c.to_canonical_bytes().is_err());
    let mut c = chunks()[0].clone();
    c.payload.clear();
    assert!(c.to_canonical_bytes().is_err());

    // Section count over the declared per-section ceiling.
    let mut b = begin();
    b.counts.entity_state = u32::MAX;
    assert!(matches!(
        b.to_canonical_bytes(),
        Err(AssetDataError::OverBudget { .. })
    ));
}

#[test]
fn stale_snapshot_ready_cannot_embody() {
    let current = ticket(1);
    let resolution = rres(5);
    let ready = SnapshotReady {
        snapshot_id: SnapshotId(11),
        ticket: current,
        resolution,
    };
    assert!(ready.matches(SnapshotId(11), &current, &resolution));

    // Same snapshot, older transaction: refused.
    assert!(!SnapshotReady {
        snapshot_id: SnapshotId(11),
        ticket: ticket(9),
        resolution,
    }
    .matches(SnapshotId(11), &current, &resolution));

    // Right ticket, superseded snapshot: refused.
    assert!(!ready.matches(SnapshotId(12), &current, &resolution));

    // Content set advanced mid-transfer: refused.
    let mut moved = current;
    moved.content_set = ContentSetId::hash_of(b"s2");
    assert!(!ready.matches(SnapshotId(11), &moved, &resolution));

    // Right snapshot and ticket, wrong acknowledged aggregate resolution: a
    // snapshot installed against an abandoned resolution cannot embody.
    assert!(!ready.matches(SnapshotId(11), &current, &rres(6)));
}

#[test]
fn scene_tag_wire_and_classification() {
    let tag = scene_tag();
    let wire = tag.to_wire_bytes();
    assert_eq!(SceneTag::from_wire_bytes(wire), tag);
    // Big-endian layout is part of the contract.
    assert_eq!(wire[7], 3);
    assert_eq!(wire[15], 8);

    let current = tag;
    let older_seq = SceneTag {
        realm_epoch: RealmEpoch(3),
        scene_sequence: SceneSequence(7),
    };
    let newer_seq = SceneTag {
        realm_epoch: RealmEpoch(3),
        scene_sequence: SceneSequence(9),
    };
    let older_epoch = SceneTag {
        realm_epoch: RealmEpoch(2),
        scene_sequence: SceneSequence(99),
    };
    assert_eq!(current.classify(current), SceneTagDisposition::Current);
    assert_eq!(older_seq.classify(current), SceneTagDisposition::Stale);
    assert_eq!(newer_seq.classify(current), SceneTagDisposition::Future);
    // A high sequence from a dead epoch is still stale.
    assert_eq!(older_epoch.classify(current), SceneTagDisposition::Stale);
}

#[test]
fn readiness_policy_bounds() {
    let mut p = PrepareRealm {
        transaction_id: txn(5),
        next_epoch: RealmEpoch(4),
        game_revision: GameRevisionId::hash_of(b"g"),
        content_set: ContentSetId::hash_of(b"s"),
        readiness: readiness(),
    };
    let bytes = p.to_canonical_bytes().unwrap();
    assert_eq!(PrepareRealm::from_canonical_bytes(&bytes).unwrap(), p);

    // Even Wait must declare a bounded, visible deadline.
    p.readiness.deadline_millis = 0;
    assert!(p.to_canonical_bytes().is_err());
    p.readiness.deadline_millis = u32::MAX;
    assert!(matches!(
        p.to_canonical_bytes(),
        Err(AssetDataError::OverBudget { .. })
    ));
}

#[test]
fn content_refusal_roundtrip_and_shape() {
    let refusal = ContentRefusal {
        code: ContentRefusalCode::MissingContent,
        ticket: ticket(1),
        missing: vec![aref(2, 0x21), aref(3, 0x31)],
        missing_truncated: false,
        detail: "2 blobs unreachable".into(),
    };
    let bytes = refusal.to_canonical_bytes().unwrap();
    assert_eq!(
        ContentRefusal::from_canonical_bytes(&bytes).unwrap(),
        refusal
    );
    for len in 0..bytes.len() {
        assert!(ContentRefusal::from_canonical_bytes(&bytes[..len]).is_err());
    }

    // A missing list on a non-content code is a lie about the failure.
    let mut bad = refusal.clone();
    bad.code = ContentRefusalCode::QuotaExceeded;
    assert!(matches!(
        bad.to_canonical_bytes(),
        Err(AssetDataError::Mismatch { .. })
    ));

    // Over-budget missing list refuses; truncation is the flagged path.
    let mut bad = refusal.clone();
    bad.missing = (0..100u8).map(|i| aref(i.wrapping_add(10), i)).collect();
    bad.missing.sort();
    assert!(matches!(
        bad.to_canonical_bytes(),
        Err(AssetDataError::OverBudget { .. })
    ));

    // Unsorted missing list refuses rather than reorders.
    let mut bad = refusal.clone();
    bad.missing.reverse();
    assert!(matches!(
        bad.to_canonical_bytes(),
        Err(AssetDataError::NotSorted { .. })
    ));

    // Oversized diagnostic refuses.
    let mut bad = refusal;
    bad.detail = "x".repeat(4096);
    assert!(matches!(
        bad.to_canonical_bytes(),
        Err(AssetDataError::OverBudget { .. })
    ));
}
