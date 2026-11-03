//! Packed reads must scale with objects in flight, never the pack's size.
use makepad_git::{test_support::*, *};
use std::{alloc::{GlobalAlloc, Layout, System}, cell::Cell, fs};

struct Meter;
thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}
fn allocated(delta: isize) {
    let _ = LIVE.try_with(|live| {
        let next = live.get() + delta;
        live.set(next);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(next)));
    });
}
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() { allocated(layout.size() as isize); }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        allocated(-(layout.size() as isize));
        System.dealloc(ptr, layout);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let ptr = System.realloc(ptr, layout, size);
        if !ptr.is_null() { allocated(size as isize - layout.size() as isize); }
        ptr
    }
}
#[global_allocator]
static METER: Meter = Meter;

#[test]
fn packed_walk_blobs_and_mixed_delta_chain_have_bounded_allocations() {
    let dir = tempdir().unwrap();
    let git = dir.path().join(".git");
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let mut entries = vec![PackEntry::Full(ObjectKind::Tree, Vec::new())];
    let tree = oid::hash_object("tree", &[]);
    let mut parent = None;
    for i in 0..200 {
        let sig = Signature { name: "test".into(), email: String::new(), timestamp: i, tz_offset: "+0000".into() };
        let c = Commit { tree, parents: parent.into_iter().collect(), author: sig.clone(), committer: sig, message: format!("commit {i}\n") };
        let bytes = commit::serialize_commit(&c);
        parent = Some(oid::hash_object("commit", &bytes));
        entries.push(PackEntry::Full(ObjectKind::Commit, bytes));
    }
    // Incompressible filler makes a whole-pack read exceed the allocation bound.
    let mut random = 1u64;
    for _ in 0..32 {
        let bytes = (0..32768).map(|_| {
            random ^= random << 13; random ^= random >> 7; random ^= random << 17;
            random as u8
        }).collect();
        entries.push(PackEntry::Full(ObjectKind::Blob, bytes));
    }
    let base_index = entries.len();
    let mut result = vec![b'x'; 4096];
    entries.push(PackEntry::Full(ObjectKind::Blob, result.clone()));
    let mut previous = oid::hash_object("blob", &result);
    for i in 0..160 {
        let mut delta = delta_header(result.len(), result.len() + 1);
        result.push(i as u8);
        // Large literal programs expose retaining every inflated delta in a
        // chain, even though each individual result is only a few KiB.
        for chunk in result.chunks(127) { delta.extend(delta_insert(chunk)); }
        let entry = if i % 2 == 0 {
            PackEntry::OfsDelta { kind: ObjectKind::Blob, base_index: base_index + i, delta, result: result.clone() }
        } else {
            PackEntry::RefDelta { kind: ObjectKind::Blob, base: previous, delta, result: result.clone() }
        };
        previous = oid::hash_object("blob", &result);
        entries.push(entry);
    }
    let ids = write_pack_entries(&git, &entries).unwrap();
    drop(entries);
    fs::write(git.join("refs/heads/main"), parent.unwrap().to_hex()).unwrap();
    let baseline = LIVE.with(Cell::get);
    PEAK.with(|p| p.set(baseline));
    // First read uses the default path: no opt-in is required to avoid loading packs.
    let mut repo = Repository::open(dir.path()).unwrap();
    assert_eq!(repo.read_commit(&parent.unwrap()).unwrap().message, "commit 199\n");
    let account = memory::MemoryAccount::new(128 << 10);
    repo.set_read_cache_budget(account.clone(), 16 << 10).unwrap();
    assert_eq!(repo.first_parents(parent.unwrap()).map(|c| c.unwrap()).count(), 200);
    for oid in &ids[201..233] {
        let data = repo.read_blob(oid).unwrap();
        assert_eq!(oid::hash_object("blob", &data), *oid, "oversized objects must bypass cache admission");
    }
    assert_eq!(repo.read_blob(&previous).unwrap(), result, "mixed OFS/REF chain longer than 128 resolves");
    assert_eq!(repo.read_blob(&previous).unwrap(), result, "cached result agrees");
    let peak = PEAK.with(Cell::get) - baseline;
    assert!(peak < 512 << 10, "packed read peak allocation delta {peak} exceeds 512 KiB");
    assert!(repo.read_stats().cache_bytes <= 16 << 10);
    assert!(account.reserved() <= account.capacity(), "retained objects respect the account");
    eprintln!("packed reads: peak allocated={peak} bytes, retained={} bytes", account.reserved());
}
