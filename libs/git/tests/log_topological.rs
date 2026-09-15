//! Topological guarantees of `Repository::log` after the priority-walk rewrite.
//! Repositories are built with write_object / write_tree / write_commit only.
//! No git binary.
use makepad_git::test_support::tempdir;
use makepad_git::*;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn sig_at(timestamp: i64) -> Signature {
    Signature {
        name: "Test".into(),
        email: "test@example.org".into(),
        timestamp,
        tz_offset: "+0000".into(),
    }
}

fn init_repo(dir: &Path) -> Repository {
    let git = dir.join(".git");
    fs::create_dir_all(git.join("objects")).unwrap();
    fs::create_dir_all(git.join("refs/heads")).unwrap();
    fs::create_dir_all(git.join("refs/tags")).unwrap();
    fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    Repository::open(dir).unwrap()
}

fn shared_tree(repo: &Repository) -> ObjectId {
    let blob = repo.write_object(ObjectKind::Blob, b"x\n").unwrap();
    repo.write_tree(&Tree {
        entries: vec![TreeEntry {
            mode: 0o100644,
            name: "f".into(),
            oid: blob,
        }],
    })
    .unwrap()
}

fn write(
    repo: &Repository,
    tree: ObjectId,
    parents: Vec<ObjectId>,
    message: &str,
    timestamp: i64,
) -> ObjectId {
    let sig = sig_at(timestamp);
    repo.write_commit(&Commit {
        tree,
        parents,
        author: sig.clone(),
        committer: sig,
        message: message.to_string(),
    })
    .unwrap()
}

fn positions(log: &[(ObjectId, Commit)]) -> HashMap<ObjectId, usize> {
    log.iter()
        .enumerate()
        .map(|(i, (oid, _))| (*oid, i))
        .collect()
}

fn oids(log: &[(ObjectId, Commit)]) -> Vec<ObjectId> {
    log.iter().map(|(oid, _)| *oid).collect()
}

fn messages(log: &[(ObjectId, Commit)]) -> Vec<&str> {
    log.iter().map(|(_, c)| c.message.as_str()).collect()
}

/// Every parent that appears in `log` is strictly after its child.
fn assert_child_before_every_parent(log: &[(ObjectId, Commit)]) {
    let pos = positions(log);
    assert_eq!(pos.len(), log.len(), "duplicate commits in log: {:?}", messages(log));
    for (oid, commit) in log {
        for parent in &commit.parents {
            if let Some(&parent_pos) = pos.get(parent) {
                let child_pos = pos[oid];
                assert!(
                    child_pos < parent_pos,
                    "child {} (pos {child_pos}, {:?}) must come before parent {} (pos {parent_pos}); order {:?}",
                    oid.to_hex(),
                    commit.message.trim(),
                    parent.to_hex(),
                    messages(log)
                );
            }
        }
    }
}

fn assert_unique_and_complete(log: &[(ObjectId, Commit)], expected: &[ObjectId]) {
    assert_eq!(
        log.len(),
        expected.len(),
        "log length {} != expected {}; order {:?}",
        log.len(),
        expected.len(),
        messages(log)
    );
    let mut seen = HashSet::new();
    for (oid, _) in log {
        assert!(seen.insert(*oid), "duplicate commit {}", oid.to_hex());
    }
    for id in expected {
        assert!(seen.contains(id), "missing commit {}", id.to_hex());
    }
}

/// Merge diamond `c3(c1, c2) -> c0`: c3 first, c0 last, each of c1/c2 before c0.
#[test]
fn merge_diamond_c3_first_c0_last() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);

    let c0 = write(&repo, tree, vec![], "c0\n", 0);
    let c1 = write(&repo, tree, vec![c0], "c1\n", 1);
    let c2 = write(&repo, tree, vec![c0], "c2\n", 1);
    let c3 = write(&repo, tree, vec![c1, c2], "c3\n", 2);

    let log = repo.log(&c3, 10).unwrap();
    assert_unique_and_complete(&log, &[c0, c1, c2, c3]);
    assert_child_before_every_parent(&log);
    assert_eq!(log[0].0, c3, "c3 must be first; order {:?}", messages(&log));
    assert_eq!(log[log.len() - 1].0, c0, "c0 must be last; order {:?}", messages(&log));
    let pos = positions(&log);
    assert!(pos[&c1] < pos[&c0], "c1 before c0; order {:?}", messages(&log));
    assert!(pos[&c2] < pos[&c0], "c2 before c0; order {:?}", messages(&log));
}

/// Same 30-commit, two-merge graph as `log_thirty_commits_two_merges`.
fn thirty_commits_two_merges(repo: &Repository, tree: ObjectId, timestamp: impl Fn(usize) -> i64) -> Vec<ObjectId> {
    let mut ids = Vec::with_capacity(30);
    ids.push(write(repo, tree, vec![], "c0\n", timestamp(0)));
    for i in 1..20 {
        ids.push(write(
            repo,
            tree,
            vec![ids[i - 1]],
            &format!("c{i}\n"),
            timestamp(i),
        ));
    }
    ids.push(write(repo, tree, vec![ids[5]], "c20\n", timestamp(20)));
    ids.push(write(repo, tree, vec![ids[20]], "c21\n", timestamp(21)));
    ids.push(write(repo, tree, vec![ids[21]], "c22\n", timestamp(22)));
    ids.push(write(
        repo,
        tree,
        vec![ids[19], ids[22]],
        "merge1\n",
        timestamp(23),
    ));
    ids.push(write(repo, tree, vec![ids[10]], "c24\n", timestamp(24)));
    ids.push(write(repo, tree, vec![ids[24]], "c25\n", timestamp(25)));
    ids.push(write(
        repo,
        tree,
        vec![ids[23], ids[25]],
        "merge2\n",
        timestamp(26),
    ));
    ids.push(write(repo, tree, vec![ids[26]], "c27\n", timestamp(27)));
    ids.push(write(repo, tree, vec![ids[27]], "c28\n", timestamp(28)));
    ids.push(write(repo, tree, vec![ids[28]], "c29\n", timestamp(29)));
    assert_eq!(ids.len(), 30);
    ids
}

#[test]
fn thirty_commits_two_merges_child_before_parent_and_max_count() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);
    let ids = thirty_commits_two_merges(&repo, tree, |i| i as i64);
    let head = *ids.last().unwrap();

    let full = repo.log(&head, 100).unwrap();
    assert_unique_and_complete(&full, &ids);
    assert_child_before_every_parent(&full);
    for (oid, commit) in &full {
        for parent in &commit.parents {
            assert!(
                ids.contains(parent),
                "parent {} of {} not in the 30-commit set",
                parent.to_hex(),
                oid.to_hex()
            );
        }
    }

    let clipped = repo.log(&head, 5).unwrap();
    assert_eq!(clipped.len(), 5, "max_count=5 must return exactly 5");
    assert_eq!(
        oids(&clipped),
        oids(&full)[..5],
        "max_count=5 must be the first 5 of the full order; full {:?} clipped {:?}",
        messages(&full),
        messages(&clipped)
    );
    assert_child_before_every_parent(&clipped);
}

/// Two merges, each listing the other side's parent: `m1(c1,c2)` and `m2(c2,c1)`.
/// `tip` parents both merges so one walk reaches the whole criss-cross.
#[test]
fn criss_cross_merge_child_before_parent() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);

    let c0 = write(&repo, tree, vec![], "c0\n", 0);
    let c1 = write(&repo, tree, vec![c0], "c1\n", 1);
    let c2 = write(&repo, tree, vec![c0], "c2\n", 2);
    let m1 = write(&repo, tree, vec![c1, c2], "m1\n", 3);
    let m2 = write(&repo, tree, vec![c2, c1], "m2\n", 4);
    let tip = write(&repo, tree, vec![m1, m2], "tip\n", 5);
    let expected = [c0, c1, c2, m1, m2, tip];

    let log = repo.log(&tip, 10).unwrap();
    assert_unique_and_complete(&log, &expected);
    assert_child_before_every_parent(&log);
    assert_eq!(log[0].0, tip);
    let pos = positions(&log);
    assert!(pos[&m1] < pos[&c1] && pos[&m1] < pos[&c2]);
    assert!(pos[&m2] < pos[&c1] && pos[&m2] < pos[&c2]);
    assert!(pos[&c1] < pos[&c0] && pos[&c2] < pos[&c0]);

    let from_m1 = repo.log(&m1, 10).unwrap();
    assert_child_before_every_parent(&from_m1);
    let from_m2 = repo.log(&m2, 10).unwrap();
    assert_child_before_every_parent(&from_m2);
}

#[test]
fn equal_committer_timestamps_are_deterministic() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);
    let ids = thirty_commits_two_merges(&repo, tree, |_| 0);
    let head = *ids.last().unwrap();

    let first = repo.log(&head, 100).unwrap();
    let second = repo.log(&head, 100).unwrap();
    assert_unique_and_complete(&first, &ids);
    assert_child_before_every_parent(&first);
    assert_eq!(
        oids(&first),
        oids(&second),
        "equal timestamps must yield the same order on two runs; first {:?} second {:?}",
        messages(&first),
        messages(&second)
    );
}

#[test]
fn linear_chain_200_head_first() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);

    let mut ids = Vec::with_capacity(200);
    ids.push(write(&repo, tree, vec![], "c0\n", 0));
    for i in 1..200 {
        ids.push(write(
            &repo,
            tree,
            vec![ids[i - 1]],
            &format!("c{i}\n"),
            i as i64,
        ));
    }
    let head = *ids.last().unwrap();
    let log = repo.log(&head, 200).unwrap();
    assert_eq!(log.len(), 200);
    assert_eq!(log[0].0, head, "HEAD must be first");
    for i in 0..200 {
        assert_eq!(
            log[i].0, ids[199 - i],
            "linear log position {i} expected c{}",
            199 - i
        );
    }
    assert_child_before_every_parent(&log);

    // Repository::log reads through private `read_commit` / `read_object`;
    // there is no overridable object-store or read-count hook, so this test
    // cannot wrap the walk to count commit reads.
}

#[test]
fn max_count_zero_is_empty() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);
    let head = write(&repo, tree, vec![], "c0\n", 0);

    let log = repo.log(&head, 0).unwrap();
    assert!(log.is_empty(), "max_count=0 must return an empty list, got {:?}", messages(&log));
}

fn commit_loose_path(repo: &Repository, oid: &ObjectId) -> PathBuf {
    let (dir, file) = oid.loose_path_components();
    repo.common_dir.join("objects").join(dir).join(file)
}

/// Move commit objects aside so a subsequent `log` errors if it reads them.
/// `read_commit` is not overridable; hiding the loose files is the wrapper.
fn hide_commits(repo: &Repository, oids: &[ObjectId]) -> Vec<(PathBuf, PathBuf)> {
    let mut hidden = Vec::with_capacity(oids.len());
    for oid in oids {
        let path = commit_loose_path(repo, oid);
        let aside = path.with_extension("hidden");
        fs::rename(&path, &aside).unwrap_or_else(|e| {
            panic!("hide {} at {}: {e}", oid.to_hex(), path.display())
        });
        hidden.push((path, aside));
    }
    hidden
}

fn restore_commits(hidden: Vec<(PathBuf, PathBuf)>) {
    for (path, aside) in hidden {
        fs::rename(&aside, &path).unwrap_or_else(|e| {
            panic!("restore {}: {e}", path.display())
        });
    }
}

fn linear_chain(repo: &Repository, tree: ObjectId, n: usize, timestamp: impl Fn(usize) -> i64) -> Vec<ObjectId> {
    let mut ids = Vec::with_capacity(n);
    ids.push(write(repo, tree, vec![], "c0\n", timestamp(0)));
    for i in 1..n {
        ids.push(write(
            repo,
            tree,
            vec![ids[i - 1]],
            &format!("c{i}\n"),
            timestamp(i),
        ));
    }
    ids
}

/// One commit with three parents (octopus). Octopus first, root last, every edge child-before-parent.
#[test]
fn octopus_merge_three_parents() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);

    let r = write(&repo, tree, vec![], "r\n", 0);
    let p1 = write(&repo, tree, vec![r], "p1\n", 1);
    let p2 = write(&repo, tree, vec![r], "p2\n", 2);
    let p3 = write(&repo, tree, vec![r], "p3\n", 3);
    let octopus = write(&repo, tree, vec![p1, p2, p3], "octopus\n", 4);
    let expected = [r, p1, p2, p3, octopus];

    let log = repo.log(&octopus, 10).unwrap();
    assert_unique_and_complete(&log, &expected);
    assert_child_before_every_parent(&log);
    assert_eq!(log[0].0, octopus, "octopus must be first; order {:?}", messages(&log));
    assert_eq!(log[log.len() - 1].0, r, "root must be last; order {:?}", messages(&log));
    let pos = positions(&log);
    assert!(pos[&p1] < pos[&r] && pos[&p2] < pos[&r] && pos[&p3] < pos[&r]);
    assert_eq!(log[0].1.parents.len(), 3, "octopus must list 3 parents");
}

/// Skew graph: merge M of (A, S) over parent P. S is older than P by `skew_secs`.
fn skew_graph(repo: &Repository, tree: ObjectId, skew_secs: i64) -> (ObjectId, ObjectId, ObjectId, ObjectId) {
    let p_ts = 1_000_000i64;
    let p = write(repo, tree, vec![], "P\n", p_ts);
    let a = write(repo, tree, vec![p], "A\n", p_ts + 1);
    let s = write(repo, tree, vec![p], "S\n", p_ts - skew_secs);
    let m = write(repo, tree, vec![a, s], "M\n", p_ts + 2);
    (p, a, s, m)
}

/// Child 1 hour older than its parent (within the 24 h allowance): child still before parent.
#[test]
fn clock_skew_one_hour_child_before_parent() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);

    let (p, a, s, m) = skew_graph(&repo, tree, 3600);
    let expected = [p, a, s, m];

    let log = repo.log(&m, 10).unwrap();
    assert_unique_and_complete(&log, &expected);
    assert_child_before_every_parent(&log);
    let pos = positions(&log);
    assert!(
        pos[&s] < pos[&p],
        "1h-older child S must still come before parent P; order {:?}",
        messages(&log)
    );
    assert_eq!(log[0].0, m);
}

/// Child 48 hours older than its parent: still child-before-parent (no skew allowance).
#[test]
fn clock_skew_48_hours_child_before_parent() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);

    let (p, a, s, m) = skew_graph(&repo, tree, 48 * 3600);
    let expected = [p, a, s, m];

    let log = repo.log(&m, 10).unwrap();
    assert_unique_and_complete(&log, &expected);
    assert_child_before_every_parent(&log);
    let pos = positions(&log);
    assert!(
        pos[&s] < pos[&p],
        "48h-older child S must still come before parent P; order {:?}",
        messages(&log)
    );
    assert_eq!(log[0].0, m);
}

/// `<common>/shallow` lists a mid-chain commit: walk stops there, no error, no reads beyond.
#[test]
fn shallow_mid_chain_stops_without_reading_beyond() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);

    let ids = linear_chain(&repo, tree, 10, |i| i as i64);
    let mid = ids[4];
    let head = *ids.last().unwrap();
    fs::write(
        repo.git_dir.join("shallow"),
        format!("{}\n", mid.to_hex()),
    )
    .unwrap();

    // Hide every commit older than the shallow boundary. If the walk followed
    // mid's parent it would hit ObjectNotFound.
    let hidden = hide_commits(&repo, &ids[..4]);
    let log = repo
        .log(&head, 100)
        .expect("shallow walk must not error at the boundary");
    restore_commits(hidden);

    let expected = &ids[4..];
    assert_eq!(
        log.len(),
        expected.len(),
        "shallow log length {} != {}; order {:?}",
        log.len(),
        expected.len(),
        messages(&log)
    );
    assert_eq!(oids(&log), expected.iter().copied().rev().collect::<Vec<_>>());
    assert_eq!(log[0].0, head);
    assert_eq!(
        log.last().unwrap().0,
        mid,
        "walk must stop on the shallow commit; order {:?}",
        messages(&log)
    );
    assert_child_before_every_parent(&log);
}

/// 3,000-commit linear history: `log` walks every reachable commit, then Kahn
/// release. `max_count=10` still emits 10; hiding the oldest object must error.
#[test]
fn cost_linear_3000() {
    const N: usize = 3000;
    const HOUR: i64 = 3600;

    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);
    let ids = linear_chain(&repo, tree, N, |i| i as i64 * HOUR);
    let head = *ids.last().unwrap();

    let t0 = Instant::now();
    let clipped = repo.log(&head, 10).unwrap();
    let clipped_ms = t0.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(clipped.len(), 10);
    assert_eq!(clipped[0].0, head);
    assert_child_before_every_parent(&clipped);
    for i in 0..10 {
        assert_eq!(clipped[i].0, ids[N - 1 - i]);
    }

    // Full walk: a missing parent is an error, never a silent gap.
    let hidden = hide_commits(&repo, &ids[..1]);
    let missing = repo.log(&head, 10);
    restore_commits(hidden);
    assert!(
        missing.is_err(),
        "hiding the oldest commit must make log(head, 10) return Err, got {:?}",
        missing.as_ref().map(|l| messages(l))
    );

    println!("log(head, 10) on {N} commits: {clipped_ms:.3} ms");

    let t1 = Instant::now();
    let full = repo.log(&head, N).unwrap();
    let full_ms = t1.elapsed().as_secs_f64() * 1000.0;
    println!("log(head, {N}) on {N} commits: {full_ms:.3} ms, len = {}", full.len());
    assert_unique_and_complete(&full, &ids);
    assert_child_before_every_parent(&full);
    assert_eq!(full[0].0, head);
    assert_eq!(full[N - 1].0, ids[0]);
}

/// Child C (ts 100) whose parent P is newer (ts 101). Emission re-checks that
/// the candidate is still the heap top, so the walk returns both [C, P] rather
/// than dropping P. The 3-deep chain C→P→G with increasing parent times is
/// the same rule applied twice.
#[test]
fn newer_parent_than_child_both_emitted() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let mut repo = init_repo(&root);
    let tree = shared_tree(&repo);

    let p = write(&repo, tree, vec![], "P\n", 101);
    let c = write(&repo, tree, vec![p], "C\n", 100);
    let log = repo.log(&c, 10).unwrap();
    assert_eq!(
        oids(&log),
        vec![c, p],
        "C (ts 100) then P (ts 101); got {:?}",
        messages(&log)
    );
    assert_child_before_every_parent(&log);

    let g = write(&repo, tree, vec![], "G\n", 102);
    let p3 = write(&repo, tree, vec![g], "P3\n", 101);
    let c3 = write(&repo, tree, vec![p3], "C3\n", 100);
    let log3 = repo.log(&c3, 10).unwrap();
    assert_eq!(
        oids(&log3),
        vec![c3, p3, g],
        "C (ts 100) then P (ts 101) then G (ts 102); got {:?}",
        messages(&log3)
    );
    assert_child_before_every_parent(&log3);
}

/// Numerical Recipes 64-bit LCG: `state = state * 6364136223846793005 + 1`.
struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        self.state
    }

    fn next_usize(&mut self, min: usize, max_inclusive: usize) -> usize {
        let span = max_inclusive - min + 1;
        min + (self.next_u64() as usize % span)
    }

    fn next_i64(&mut self, min: i64, max_inclusive: i64) -> i64 {
        let span = (max_inclusive as i128 - min as i128 + 1) as u64;
        min + (self.next_u64() % span) as i64
    }
}

const BASE_TS: i64 = 1_700_000_000;
const HOUR: i64 = 3600;
const SKEW_23H: i64 = 23 * HOUR;
const SKEW_72H: i64 = 72 * HOUR;
const RANDOM_DAG_COUNT: u64 = 200;
const LOG_TIMEOUT: Duration = Duration::from_millis(1500);

/// `Repository::log` can fail to return on some skewed DAGs (promotion with
/// `i64::MAX` leaves the newest pending commit at the heap top). Bound the wait
/// so the suite reports the seed instead of hanging.
fn log_or_timeout(
    workdir: &Path,
    tip: ObjectId,
    max_count: usize,
) -> Result<Vec<(ObjectId, Commit)>, String> {
    let workdir = workdir.to_path_buf();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut repo = Repository::open(&workdir).expect("reopen repo for log");
        let _ = tx.send(repo.log(&tip, max_count).map_err(|e| e.to_string()));
    });
    match rx.recv_timeout(LOG_TIMEOUT) {
        Ok(Ok(log)) => Ok(log),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("timed out (walk did not return)".into()),
    }
}

/// One random connected DAG: 5–60 commits, each later commit has 1–3 parents
/// among earlier commits (always including the previous commit so the tip
/// reaches everyone). Committer times are the newest parent's time plus a
/// uniform offset in `[-max_skew_secs, +max_skew_secs]`.
struct RandomDag {
    seed: u64,
    ids: Vec<ObjectId>,
    timestamps: Vec<i64>,
    /// `parent_indices[i]` lists the indices of commit `i`'s parents.
    parent_indices: Vec<Vec<usize>>,
}

impl RandomDag {
    fn generate(repo: &Repository, tree: ObjectId, seed: u64, max_skew_secs: i64) -> Self {
        let mut rng = Lcg::new(seed);
        let n = rng.next_usize(5, 60);
        let mut ids = Vec::with_capacity(n);
        let mut timestamps = Vec::with_capacity(n);
        let mut parent_indices = Vec::with_capacity(n);

        let ts0 = BASE_TS + rng.next_i64(-max_skew_secs, max_skew_secs);
        ids.push(write(repo, tree, vec![], "c0\n", ts0));
        timestamps.push(ts0);
        parent_indices.push(Vec::new());

        for i in 1..n {
            // Spanning parent i-1 plus 0..=2 extra earlier parents, capped at 3.
            let mut chosen = vec![i - 1];
            if i > 1 {
                let extra = rng.next_usize(0, (i - 1).min(2));
                let mut pool: Vec<usize> = (0..i - 1).collect();
                for _ in 0..extra {
                    let k = rng.next_usize(0, pool.len() - 1);
                    chosen.push(pool.swap_remove(k));
                }
            }
            chosen.sort_unstable();
            let newest = chosen.iter().map(|&j| timestamps[j]).max().unwrap();
            let ts = newest + rng.next_i64(-max_skew_secs, max_skew_secs);
            let parents: Vec<ObjectId> = chosen.iter().map(|&j| ids[j]).collect();
            ids.push(write(repo, tree, parents, &format!("c{i}\n"), ts));
            timestamps.push(ts);
            parent_indices.push(chosen);
        }

        Self {
            seed,
            ids,
            timestamps,
            parent_indices,
        }
    }

    fn tip(&self) -> ObjectId {
        *self.ids.last().unwrap()
    }

    fn format_edges(&self) -> String {
        let mut parts = Vec::new();
        for (child, parents) in self.parent_indices.iter().enumerate() {
            for &parent in parents {
                parts.push(format!(
                    "c{child}(ts={})->c{parent}(ts={})",
                    self.timestamps[child], self.timestamps[parent]
                ));
            }
        }
        parts.join(", ")
    }
}

fn duplicate_in_log(log: &[(ObjectId, Commit)]) -> Option<ObjectId> {
    let mut seen = HashSet::new();
    for (oid, _) in log {
        if !seen.insert(*oid) {
            return Some(*oid);
        }
    }
    None
}

fn missing_from_log(log: &[(ObjectId, Commit)], expected: &[ObjectId]) -> Option<ObjectId> {
    let seen: HashSet<ObjectId> = log.iter().map(|(oid, _)| *oid).collect();
    expected.iter().copied().find(|id| !seen.contains(id))
}

fn extra_in_log(log: &[(ObjectId, Commit)], expected: &[ObjectId]) -> Option<ObjectId> {
    let expected: HashSet<ObjectId> = expected.iter().copied().collect();
    log.iter()
        .map(|(oid, _)| *oid)
        .find(|id| !expected.contains(id))
}

/// Child-before-parent on every edge whose parent also appears in `log`.
fn first_parent_before_child(
    log: &[(ObjectId, Commit)],
) -> Option<(ObjectId, ObjectId, usize, usize)> {
    let pos = positions(log);
    for (oid, commit) in log {
        for parent in &commit.parents {
            if let Some(&parent_pos) = pos.get(parent) {
                let child_pos = pos[oid];
                if child_pos >= parent_pos {
                    return Some((*oid, *parent, child_pos, parent_pos));
                }
            }
        }
    }
    None
}

fn log_order_labels(dag: &RandomDag, log: &[(ObjectId, Commit)]) -> String {
    let index_of: HashMap<ObjectId, usize> = dag
        .ids
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, i))
        .collect();
    log.iter()
        .map(|(oid, _)| match index_of.get(oid) {
            Some(i) => format!("c{i}"),
            None => oid.to_hex(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn assert_random_dag_complete_unique_topo(dag: &RandomDag, log: &[(ObjectId, Commit)]) {
    if let Some(oid) = duplicate_in_log(log) {
        println!(
            "first violating dag seed={} edges=[{}] log=[{}]",
            dag.seed,
            dag.format_edges(),
            log_order_labels(dag, log)
        );
        panic!(
            "seed {}: duplicate commit {} in log [{}]",
            dag.seed,
            oid.to_hex(),
            log_order_labels(dag, log)
        );
    }
    if log.len() != dag.ids.len() {
        println!(
            "first violating dag seed={} edges=[{}] log=[{}]",
            dag.seed,
            dag.format_edges(),
            log_order_labels(dag, log)
        );
        panic!(
            "seed {}: log length {} != {} commits; log [{}]",
            dag.seed,
            log.len(),
            dag.ids.len(),
            log_order_labels(dag, log)
        );
    }
    if let Some(oid) = missing_from_log(log, &dag.ids) {
        println!(
            "first violating dag seed={} edges=[{}] log=[{}]",
            dag.seed,
            dag.format_edges(),
            log_order_labels(dag, log)
        );
        panic!(
            "seed {}: missing commit {} from log [{}]",
            dag.seed,
            oid.to_hex(),
            log_order_labels(dag, log)
        );
    }
    if let Some(oid) = extra_in_log(log, &dag.ids) {
        println!(
            "first violating dag seed={} edges=[{}] log=[{}]",
            dag.seed,
            dag.format_edges(),
            log_order_labels(dag, log)
        );
        panic!(
            "seed {}: unexpected commit {} in log [{}]",
            dag.seed,
            oid.to_hex(),
            log_order_labels(dag, log)
        );
    }
    if let Some((child, parent, child_pos, parent_pos)) = first_parent_before_child(log) {
        println!(
            "first violating dag seed={} edges=[{}] log=[{}]",
            dag.seed,
            dag.format_edges(),
            log_order_labels(dag, log)
        );
        panic!(
            "seed {}: child {} (pos {child_pos}) must come before parent {} (pos {parent_pos}); log [{}]",
            dag.seed,
            child.to_hex(),
            parent.to_hex(),
            log_order_labels(dag, log)
        );
    }
}

#[test]
fn random_dags_are_topological() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let repo = init_repo(&root);
    let tree = shared_tree(&repo);

    for seed in 0..RANDOM_DAG_COUNT {
        let dag = RandomDag::generate(&repo, tree, seed, SKEW_72H);
        let tip = dag.tip();
        let full = match log_or_timeout(&root, tip, usize::MAX) {
            Ok(log) => log,
            Err(e) => {
                println!(
                    "first violating dag seed={} edges=[{}] error={e}",
                    dag.seed,
                    dag.format_edges()
                );
                panic!("seed {}: log(tip, MAX) failed: {e}", dag.seed);
            }
        };
        assert_random_dag_complete_unique_topo(&dag, &full);

        for max_count in 1..=full.len() {
            let clipped = match log_or_timeout(&root, tip, max_count) {
                Ok(log) => log,
                Err(e) => {
                    println!(
                        "first violating dag seed={} edges=[{}] error={e} max_count={max_count}",
                        dag.seed,
                        dag.format_edges()
                    );
                    panic!(
                        "seed {}: log(tip, {max_count}) failed: {e}",
                        dag.seed
                    );
                }
            };
            assert_eq!(
                oids(&clipped),
                oids(&full)[..max_count],
                "seed {}: max_count={max_count} must be the full order prefix; full [{}] clipped [{}]",
                dag.seed,
                log_order_labels(&dag, &full),
                log_order_labels(&dag, &clipped)
            );
        }
    }
}

fn assert_log_complete_and_unique(dag: &RandomDag, log: &[(ObjectId, Commit)], label: &str) {
    if let Some(oid) = duplicate_in_log(log) {
        panic!(
            "seed {}: {label} log duplicated {}; log [{}]",
            dag.seed,
            oid.to_hex(),
            log_order_labels(dag, log)
        );
    }
    if let Some(oid) = missing_from_log(log, &dag.ids) {
        panic!(
            "seed {}: {label} log missing {}; log [{}]",
            dag.seed,
            oid.to_hex(),
            log_order_labels(dag, log)
        );
    }
    if log.len() != dag.ids.len() {
        panic!(
            "seed {}: {label} log length {} != {}",
            dag.seed,
            log.len(),
            dag.ids.len()
        );
    }
}

/// Any committer-clock skew: complete, unique, child-before-parent. Timeout
/// wrapper reports the seed instead of hanging on a regression.
#[test]
fn random_dags_any_skew_topological() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    let repo = init_repo(&root);
    let tree = shared_tree(&repo);

    let mut skew23_violated = 0usize;
    let mut skew23_hung = 0usize;
    let mut skew23_completed = 0usize;
    let mut skew23_first: Option<(u64, String, String)> = None;
    let mut skew23_first_hang: Option<u64> = None;
    for seed in 0..RANDOM_DAG_COUNT {
        let dag = RandomDag::generate(&repo, tree, seed, SKEW_23H);
        let full = match log_or_timeout(&root, dag.tip(), usize::MAX) {
            Ok(log) => log,
            Err(e) => {
                skew23_hung += 1;
                skew23_first_hang = Some(dag.seed);
                println!(
                    "first ±23h hang seed={} edges=[{}] error={e} (aborting remaining ±23h seeds; a spinning walk cannot be killed)",
                    dag.seed,
                    dag.format_edges()
                );
                break;
            }
        };
        skew23_completed += 1;
        assert_log_complete_and_unique(&dag, &full, "±23h");
        if first_parent_before_child(&full).is_some() {
            if skew23_first.is_none() {
                skew23_first = Some((
                    dag.seed,
                    dag.format_edges(),
                    log_order_labels(&dag, &full),
                ));
                println!(
                    "first ±23h topological violation seed={} edges=[{}] log=[{}]",
                    dag.seed,
                    dag.format_edges(),
                    log_order_labels(&dag, &full)
                );
            }
            skew23_violated += 1;
        }
    }
    println!(
        "±23h completed={skew23_completed}/{} violated={skew23_violated} hung={skew23_hung} first_hang={skew23_first_hang:?}",
        RANDOM_DAG_COUNT
    );

    let mut skew72_violated = 0usize;
    let mut skew72_hung = 0usize;
    let mut skew72_completed = 0usize;
    let mut skew72_first: Option<(u64, String, String)> = None;
    let mut skew72_first_hang: Option<u64> = None;
    for seed in 0..RANDOM_DAG_COUNT {
        let dag = RandomDag::generate(&repo, tree, seed, SKEW_72H);
        let full = match log_or_timeout(&root, dag.tip(), usize::MAX) {
            Ok(log) => log,
            Err(e) => {
                skew72_hung += 1;
                skew72_first_hang = Some(dag.seed);
                println!(
                    "first ±72h hang seed={} edges=[{}] error={e} (aborting remaining ±72h seeds; a spinning walk cannot be killed)",
                    dag.seed,
                    dag.format_edges()
                );
                break;
            }
        };
        skew72_completed += 1;
        assert_log_complete_and_unique(&dag, &full, "±72h");
        if first_parent_before_child(&full).is_some() {
            if skew72_first.is_none() {
                skew72_first = Some((
                    dag.seed,
                    dag.format_edges(),
                    log_order_labels(&dag, &full),
                ));
                println!(
                    "first ±72h topological violation seed={} edges=[{}] log=[{}]",
                    dag.seed,
                    dag.format_edges(),
                    log_order_labels(&dag, &full)
                );
            }
            skew72_violated += 1;
        }
    }

    println!(
        "±72h completed={skew72_completed}/{} violated={skew72_violated} hung={skew72_hung} first_hang={skew72_first_hang:?}",
        RANDOM_DAG_COUNT
    );

    if skew23_hung > 0 || skew72_hung > 0 {
        panic!(
            "walk hung before finishing {RANDOM_DAG_COUNT} dags: \
             ±23h completed {skew23_completed} violated {skew23_violated} hung {skew23_hung} (first {:?}); \
             ±72h completed {skew72_completed} violated {skew72_violated} hung {skew72_hung} (first {:?})",
            skew23_first_hang, skew72_first_hang
        );
    }
    if let Some((seed, edges, order)) = skew23_first {
        panic!(
            "±23h must be topological; \
             {skew23_violated}/{skew23_completed} completed dags violated. first seed={seed} edges=[{edges}] log=[{order}]"
        );
    }
    if let Some((seed, edges, order)) = skew72_first {
        panic!(
            "±72h must be topological; \
             {skew72_violated}/{skew72_completed} completed dags violated. first seed={seed} edges=[{edges}] log=[{order}]"
        );
    }
    assert_eq!(
        (skew23_completed, skew23_violated, skew23_hung),
        (RANDOM_DAG_COUNT as usize, 0, 0),
        "±23h expected completed=200 violated=0 hung=0, first_hang={skew23_first_hang:?}"
    );
    assert_eq!(
        (skew72_completed, skew72_violated, skew72_hung),
        (RANDOM_DAG_COUNT as usize, 0, 0),
        "±72h expected completed=200 violated=0 hung=0, first_hang={skew72_first_hang:?}"
    );
}
