//! `Repository::object_size` (header-only sizes) and `Repository::first_parents`.
//! No git binary.
use makepad_git::test_support::{
    delta_copy, delta_header, delta_insert, tempdir, write_pack_entries, PackEntry,
};
use makepad_git::*;
use std::fs;
use std::path::Path;

fn init_repo(dir: &Path) -> Repository {
    let git = dir.join(".git");
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    Repository::open(dir).unwrap()
}

#[test]
fn object_size_reads_loose_packed_and_delta_headers() {
    let dir = tempdir().unwrap();
    let mut repo = init_repo(dir.path());
    let loose = repo.write_blob(b"loose object\n").unwrap();

    let base: Vec<u8> = (0..300u32).map(|i| (i % 251) as u8).collect();
    let mut ofs_result = base[..200].to_vec();
    ofs_result.extend_from_slice(b"tail");
    let mut ofs_delta = delta_header(base.len(), ofs_result.len());
    ofs_delta.extend(delta_copy(0, 200));
    ofs_delta.extend(delta_insert(b"tail"));
    let mut ref_result = b"head".to_vec();
    ref_result.extend_from_slice(&base[100..300]);
    let mut ref_delta = delta_header(base.len(), ref_result.len());
    ref_delta.extend(delta_insert(b"head"));
    ref_delta.extend(delta_copy(100, 200));
    let base_oid = makepad_git::oid::hash_object("blob", &base);
    let ids = write_pack_entries(
        &dir.path().join(".git"),
        &[
            PackEntry::Full(ObjectKind::Blob, base.clone()),
            PackEntry::OfsDelta { kind: ObjectKind::Blob, base_index: 0, delta: ofs_delta, result: ofs_result.clone() },
            PackEntry::RefDelta { kind: ObjectKind::Blob, base: base_oid, delta: ref_delta, result: ref_result.clone() },
        ],
    )
    .unwrap();

    assert_eq!(repo.object_size(&loose).unwrap(), 13);
    assert_eq!(repo.object_size(&ids[0]).unwrap(), base.len() as u64);
    assert_eq!(repo.object_size(&ids[1]).unwrap(), ofs_result.len() as u64);
    assert_eq!(repo.object_size(&ids[2]).unwrap(), ref_result.len() as u64);
    // Sizes agree with a full read of the same objects.
    for oid in [&ids[0], &ids[1], &ids[2], &loose] {
        assert_eq!(repo.object_size(oid).unwrap(), repo.read_object(oid).unwrap().data.len() as u64);
    }
    let missing = ObjectId::from_hex("0123456789012345678901234567890123456789").unwrap();
    assert!(matches!(repo.object_size(&missing), Err(GitError::ObjectNotFound(_))));
}

#[test]
fn first_parents_walks_the_first_parent_chain_only() {
    let dir = tempdir().unwrap();
    let repo = init_repo(dir.path());
    let tree = repo.write_tree(&Tree { entries: vec![] }).unwrap();
    let commit = |parents: Vec<ObjectId>, message: &str, timestamp: i64| {
        let sig = Signature { name: "T".into(), email: "t@example.org".into(), timestamp, tz_offset: "+0000".into() };
        repo.write_commit(&Commit { tree, parents, author: sig.clone(), committer: sig, message: message.into() }).unwrap()
    };
    let root = commit(vec![], "root\n", 1);
    let side = commit(vec![root], "side\n", 2);
    let main = commit(vec![root], "main\n", 3);
    let merge = commit(vec![main, side], "merge\n", 4);
    let mut repo = Repository::open(dir.path()).unwrap();
    let chain: Vec<ObjectId> = repo.first_parents(merge).map(|item| item.unwrap().0).collect();
    assert_eq!(chain, vec![merge, main, root]);
}
