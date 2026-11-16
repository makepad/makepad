//! `Reach`: ancestry against a moving tip, built with write_commit only (no git binary).
use makepad_git::test_support::tempdir;
use makepad_git::*;
use std::fs;
use std::path::Path;

fn init_repo(dir: &Path) -> Repository {
    let git = dir.join(".git");
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/work\n").unwrap();
    Repository::open(dir).unwrap()
}

fn commit(repo: &Repository, parents: &[ObjectId], message: &str, at: i64) -> ObjectId {
    let tree = repo.write_tree(&Tree { entries: Vec::new() }).unwrap();
    let sig = Signature { name: "t".into(), email: "t@t".into(), timestamp: at, tz_offset: "+0000".into() };
    repo.write_commit(&Commit { tree, parents: parents.to_vec(), author: sig.clone(), committer: sig, message: message.into() }).unwrap()
}

#[test]
fn ancestry_and_ranges_follow_the_tip() {
    let dir = tempdir().unwrap();
    let mut repo = init_repo(dir.path());
    // work: a - b - c ; lane: b - x - y, then the bot merges c into it (m)
    let a = commit(&repo, &[], "a", 1);
    let b = commit(&repo, &[a], "b", 2);
    let c = commit(&repo, &[b], "c", 3);
    let x = commit(&repo, &[b], "x", 4);
    let y = commit(&repo, &[x], "y", 5);
    let m = commit(&repo, &[y, c], "Merge work (c) into rc-l", 6);
    let mut reach = Reach::new();
    reach.advance(&mut repo, c).unwrap();
    assert_eq!(reach.len(), 3);
    assert!(reach.contains(&b) && !reach.contains(&x));
    let r = reach.range(&mut repo, m).unwrap();
    assert_eq!(r.commits.iter().map(|(o, _)| *o).collect::<Vec<_>>(), vec![m, y, x]);
    assert!(r.boundary.contains(&c), "m holds the tip");
    let r = reach.range(&mut repo, y).unwrap();
    assert_eq!(r.boundary, vec![b], "y does not hold the tip");
    // work moves on by landing the lane: only the new commits are read
    let land = commit(&repo, &[c, m], "land", 7);
    reach.advance(&mut repo, land).unwrap();
    assert_eq!(reach.len(), 7);
    assert!(reach.contains(&y) && reach.range(&mut repo, m).unwrap().commits.is_empty());
    // beyond two tips: what neither reaches
    let mut lane = Reach::new();
    lane.advance(&mut repo, y).unwrap();
    let r = Reach::range_beyond(&[&reach, &lane], &mut repo, m).unwrap();
    assert!(r.commits.is_empty() && r.boundary.contains(&m));
    // a tip that is no descendant (a retired chain) is read again from scratch
    let squashed = commit(&repo, &[], "squashed", 8);
    reach.advance(&mut repo, squashed).unwrap();
    assert_eq!(reach.len(), 1);
    assert!(!reach.contains(&a));
    // moving back to an ancestor drops what the old tip held
    reach.advance(&mut repo, land).unwrap();
    reach.advance(&mut repo, b).unwrap();
    assert_eq!(reach.len(), 2);
    assert!(!reach.contains(&c));
}

/// `is_ancestor` agrees with a full `Reach` for every pair of a history with merges, side
/// chains and a clock that runs backwards on one chain.
#[test]
fn is_ancestor_is_exact_for_every_pair() {
    let dir = tempdir().unwrap();
    let mut repo = init_repo(dir.path());
    let a = commit(&repo, &[], "a", 10);
    let b = commit(&repo, &[a], "b", 20);
    let c = commit(&repo, &[b], "c", 30);
    // a side chain whose clock is behind (made "before" its own parent)
    let x = commit(&repo, &[b], "x", 5);
    let y = commit(&repo, &[x], "y", 6);
    let m = commit(&repo, &[c, y], "m", 40);
    let z = commit(&repo, &[a], "z", 50);
    let n = commit(&repo, &[z, x], "n", 60);
    let root2 = commit(&repo, &[], "other root", 70);
    let all = [a, b, c, x, y, m, z, n, root2];
    for &to in &all {
        let mut reach = Reach::new();
        reach.advance(&mut repo, to).unwrap();
        for &from in &all {
            assert_eq!(is_ancestor(&mut repo, from, to).unwrap(), reach.contains(&from), "{from} in {to}");
        }
    }
}

/// `merge_bases` gives every pair of a history with a criss-cross merge, side chains and a clock
/// that runs backwards what the full histories say: the common commits no other common commit
/// descends from.
#[test]
fn merge_bases_are_exact_for_every_pair() {
    let dir = tempdir().unwrap();
    let mut repo = init_repo(dir.path());
    let a = commit(&repo, &[], "a", 10);
    let b = commit(&repo, &[a], "b", 20);
    let c = commit(&repo, &[b], "c", 30);
    let x = commit(&repo, &[b], "x", 5);
    let y = commit(&repo, &[x], "y", 6);
    // criss-cross: each side merges the other's older commit
    let m1 = commit(&repo, &[c, y], "m1", 40);
    let m2 = commit(&repo, &[y, c], "m2", 41);
    let c2 = commit(&repo, &[m1], "c2", 42);
    let y2 = commit(&repo, &[m2], "y2", 3);
    let z = commit(&repo, &[a], "z", 50);
    let n = commit(&repo, &[z, x], "n", 60);
    let root2 = commit(&repo, &[], "other root", 70);
    let all = [a, b, c, x, y, m1, m2, c2, y2, z, n, root2];
    let reach_of = |repo: &mut Repository, tip: ObjectId| {
        let mut r = Reach::new();
        r.advance(repo, tip).unwrap();
        all.iter().copied().filter(|o| r.contains(o)).collect::<Vec<_>>()
    };
    let reaches: Vec<Vec<ObjectId>> = all.iter().map(|&t| reach_of(&mut repo, t)).collect();
    let of = |o: ObjectId| &reaches[all.iter().position(|&t| t == o).unwrap()];
    for &p in &all {
        for &q in &all {
            let common: Vec<ObjectId> = of(p).iter().copied().filter(|o| of(q).contains(o)).collect();
            let mut best: Vec<ObjectId> = common.iter().copied().filter(|&o| !common.iter().any(|&d| d != o && of(d).contains(&o))).collect();
            let mut got = merge_bases(&mut repo, p, q).unwrap();
            best.sort();
            got.sort();
            assert_eq!(got, best, "merge bases of {p} and {q}");
        }
    }
    let mut cross = merge_bases(&mut repo, c2, y2).unwrap();
    cross.sort();
    let mut both = vec![c, y];
    both.sort();
    assert_eq!(cross, both, "a criss-cross merge has two bases");
}

#[test]
fn entries_at_paths() {
    let dir = tempdir().unwrap();
    let mut repo = init_repo(dir.path());
    let blob = repo.write_blob(b"x").unwrap();
    let inner = repo.write_tree(&Tree { entries: vec![TreeEntry { mode: 0o100644, name: "f.rs".into(), oid: blob }] }).unwrap();
    let top = repo.write_tree(&Tree { entries: vec![TreeEntry { mode: 0o40000, name: "src".into(), oid: inner }] }).unwrap();
    assert_eq!(repo.entry_at(&top, "").unwrap(), Some(top));
    assert_eq!(repo.entry_at(&top, "src").unwrap(), Some(inner));
    assert_eq!(repo.entry_at(&top, "src/f.rs").unwrap(), Some(blob));
    assert_eq!(repo.entry_at(&top, "src/f.rs/more").unwrap(), None);
    assert_eq!(repo.entry_at(&top, "lib").unwrap(), None);
}
