use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

fn fp(value: &u32) -> Fingerprint {
    Fingerprint::from_bytes(&value.to_le_bytes())
}

fn value_fp(value: &u64) -> Fingerprint {
    Fingerprint::from_bytes(&value.to_le_bytes())
}

fn no_force(query: QueryId) -> Result<(), QueryError> {
    Err(QueryError::MissingQuery(query))
}

struct Cx {
    engine: QueryEngine,
    values: [u64; 16],
    leaf: QueryTable<u32, u64>,
    sum: QueryTable<u32, u64>,
    leaf_runs: [AtomicUsize; 16],
    sum_runs: [AtomicUsize; 6],
}

impl Cx {
    fn new() -> Self {
        let mut cx = Self {
            engine: QueryEngine::new(),
            values: [10; 16],
            leaf: QueryTable::new(1, fp, value_fp),
            sum: QueryTable::new(2, fp, value_fp),
            leaf_runs: std::array::from_fn(|_| AtomicUsize::new(0)),
            sum_runs: std::array::from_fn(|_| AtomicUsize::new(0)),
        };
        for key in 0..16 {
            cx.engine.register_input(0, key, fp(&key)).unwrap();
            cx.engine.set_input(0, key, value_fp(&10)).unwrap();
        }
        cx
    }

    fn edit(&mut self, key: u32, value: u64) {
        self.values[key as usize] = value;
        self.engine.set_input(0, key, value_fp(&value)).unwrap();
    }

    fn input(&self, key: u32) -> Result<u64, QueryError> {
        self.engine.input(0, key)?;
        Ok(self.values[key as usize])
    }

    fn leaf(&self, key: u32) -> Result<u64, QueryError> {
        self.leaf.get(
            &self.engine,
            key,
            &mut |key| {
                self.leaf_runs[key as usize].fetch_add(1, Ordering::Relaxed);
                Ok(self.input(key)? / 10)
            },
            &|query| self.force(query),
        )
    }

    fn sum(&self, key: u32) -> Result<u64, QueryError> {
        self.sum.get(
            &self.engine,
            key,
            &mut |key| {
                self.sum_runs[key as usize].fetch_add(1, Ordering::Relaxed);
                match key {
                    0 => Ok(self.leaf(0)? * 2),
                    1 => {
                        let mut value = 0;
                        self.engine.with_anon_task(&mut || {
                            value += self.leaf(0)?;
                            self.engine.with_anon_task(&mut || {
                                value += self.leaf(1)?;
                                value += self.leaf(0)?;
                                Ok(())
                            }).0
                        }).0?;
                        Ok(value)
                    }
                    2 => Ok(self.sum(0)? + self.sum(1)?),
                    3 => {
                        if self.input(2)? % 2 == 0 {
                            self.leaf(0)
                        } else {
                            self.leaf(1)
                        }
                    }
                    4 => {
                        let mut value = 0;
                        for i in 0..10 {
                            value += self.leaf(i)?;
                        }
                        // Repeat reads to exercise deduplication.
                        for i in 0..10 {
                            value += self.leaf(i)?;
                        }
                        Ok(value)
                    }
                    5 => Ok(self.input(2)? + self.sum(0)?),
                    _ => unreachable!(),
                }
            },
            &|query| self.force(query),
        )
    }

    fn force(&self, query: QueryId) -> Result<(), QueryError> {
        match query.kind {
            1 => {
                self.leaf(self.leaf.key(query.key).unwrap())?;
            }
            2 => {
                self.sum(self.sum.key(query.key).unwrap())?;
            }
            _ => return Err(QueryError::MissingQuery(query)),
        }
        Ok(())
    }

    fn count(&self, kind: u16, key: usize) -> usize {
        match kind {
            1 => self.leaf_runs[key].load(Ordering::Relaxed),
            2 => self.sum_runs[key].load(Ordering::Relaxed),
            _ => unreachable!(),
        }
    }
}

// Intended concrete-query shape: plain functions plus one static table per kind.
static TOY_DOUBLE: QueryTable<u32, u64> = QueryTable::new(20, fp, value_fp);
static TOY_PLUS_ONE: QueryTable<u32, u64> = QueryTable::new(21, fp, value_fp);

fn toy_double(engine: &QueryEngine, key: u32) -> Result<u64, QueryError> {
    TOY_DOUBLE.get(engine, key, &mut |key| Ok(key as u64 * 2), &|query| toy_force(engine, query))
}

fn toy_plus_one(engine: &QueryEngine, key: u32) -> Result<u64, QueryError> {
    TOY_PLUS_ONE.get(engine, key, &mut |key| Ok(toy_double(engine, key)? + 1), &|query| toy_force(engine, query))
}

fn toy_force(engine: &QueryEngine, query: QueryId) -> Result<(), QueryError> {
    match query.kind {
        20 => {
            toy_double(engine, TOY_DOUBLE.key(query.key).unwrap())?;
        }
        21 => {
            toy_plus_one(engine, TOY_PLUS_ONE.key(query.key).unwrap())?;
        }
        _ => return Err(QueryError::MissingQuery(query)),
    }
    Ok(())
}

#[test]
fn static_toy_queries() {
    let engine = QueryEngine::new();
    assert_eq!(toy_plus_one(&engine, 10), Ok(21));
    assert_eq!(engine.dependencies(TOY_PLUS_ONE.query_id(&10).unwrap()), vec![TOY_DOUBLE.query_id(&10).unwrap()]);
}

#[test]
fn memoization_and_stable_identity() {
    let cx = Cx::new();
    for _ in 0..20 {
        assert_eq!(cx.sum(0), Ok(2));
    }
    assert_eq!(cx.count(1, 0), 1);
    assert_eq!(cx.count(2, 0), 1);
    let query = cx.leaf.query_id(&0).unwrap();
    assert_eq!(cx.engine.dep_node(query), Some(DepNode { kind: DepKind(1), key_fingerprint: fp(&0) }));
    assert_eq!(fp(&123).0, crate::sip::hash_bytes(&123u32.to_le_bytes()));
    // Interning order and engine-local indices do not enter the stable identity.
    let other = Cx::new();
    other.leaf(1).unwrap();
    other.leaf(0).unwrap();
    assert_eq!(cx.engine.dep_node(query), other.engine.dep_node(other.leaf.query_id(&0).unwrap()));
}

fn assert_anonymous_sum_edges(cx: &Cx) {
    let state = cx.engine.state.lock().unwrap();
    let graph = &state.graph;
    let root = graph.find(cx.sum.query_id(&1).unwrap()).unwrap();
    let edges = graph.reads(root);
    assert_eq!(edges.len(), 1);
    let outer = DepNodeIndex(edges[0]);
    assert_eq!(graph.query(outer), None);
    let reads = graph.reads(outer);
    assert_eq!(reads.len(), 2);
    assert_eq!(graph.query(DepNodeIndex(reads[0])), cx.leaf.query_id(&0));
    let inner = DepNodeIndex(reads[1]);
    assert_eq!(graph.query(inner), None);
    let reads: Vec<_> = graph.reads(inner).iter().map(|&i| graph.query(DepNodeIndex(i)).unwrap()).collect();
    assert_eq!(reads, vec![cx.leaf.query_id(&1).unwrap(), cx.leaf.query_id(&0).unwrap()]);
}

#[test]
fn anonymous_cached_read_invalidates_and_preserves_early_cutoff() {
    let mut cx = Cx::new();
    let (value, node) = cx.engine.with_anon_task(&mut || Ok::<_, QueryError>(cx.leaf(0)? + cx.leaf(1)?));
    assert_eq!(value, Ok(2));
    let table = QueryTable::new(3, fp, value_fp);
    let runs = AtomicUsize::new(0);
    let get = |cx: &Cx| table.get(&cx.engine, 0, &mut |_| {
        runs.fetch_add(1, Ordering::Relaxed);
        cx.engine.read_index(node);
        Ok(cx.leaf(0)? + cx.leaf(1)?)
    }, &|id| cx.force(id));
    assert_eq!(get(&cx), Ok(2));
    cx.edit(0, 11);
    assert_eq!(get(&cx), Ok(2));
    assert_eq!(runs.load(Ordering::Relaxed), 1);
    cx.edit(1, 20);
    assert_eq!(get(&cx), Ok(3));
    assert_eq!(runs.load(Ordering::Relaxed), 2);
    assert_eq!(cx.sum(1), Ok(4));
    cx.edit(0, 30);
    assert_eq!(cx.sum(1), Ok(8));
}

#[test]
fn cached_reads_and_anonymous_tasks_record_ordered_edges() {
    let cx = Cx::new();
    cx.leaf(0).unwrap();
    cx.leaf(1).unwrap();
    assert_eq!(cx.sum(1), Ok(3));
    assert_anonymous_sum_edges(&cx);
    assert_eq!(cx.engine.dependencies(cx.leaf.query_id(&0).unwrap()), vec![QueryId { kind: 0, key: 0 }]);
    cx.sum(4).unwrap();
    let mut expected = Vec::new();
    for key in 0..10 {
        expected.push(cx.leaf.query_id(&key).unwrap());
    }
    assert_eq!(cx.engine.dependencies(cx.sum.query_id(&4).unwrap()), expected);
}

#[test]
fn edits_reexecute_dependents_and_same_input_is_noop() {
    let mut cx = Cx::new();
    assert_eq!(cx.sum(0), Ok(2));
    let revision = cx.engine.revision();
    cx.edit(0, 10);
    assert_eq!(cx.engine.revision(), revision);
    cx.edit(0, 20);
    assert_eq!(cx.engine.color(cx.sum.query_id(&0).unwrap()), DepNodeColor::Unknown);
    assert_eq!(cx.sum(0), Ok(4));
    assert_eq!(cx.count(1, 0), 2);
    assert_eq!(cx.count(2, 0), 2);
    assert_eq!(cx.engine.color(cx.sum.query_id(&0).unwrap()), DepNodeColor::Red);
    assert_eq!(cx.sum(0), Ok(4));
    assert_eq!(cx.count(2, 0), 2);
}

#[test]
fn early_cutoff_and_validation_do_not_add_transitive_edges() {
    let mut cx = Cx::new();
    assert_eq!(cx.sum(0), Ok(2));
    cx.edit(0, 11);
    assert_eq!(cx.sum(0), Ok(2));
    assert_eq!(cx.count(1, 0), 2);
    assert_eq!(cx.count(2, 0), 1);
    assert_eq!(cx.engine.color(cx.leaf.query_id(&0).unwrap()), DepNodeColor::Green);
    assert_eq!(cx.engine.color(cx.sum.query_id(&0).unwrap()), DepNodeColor::Green);
    assert_eq!(cx.engine.dependencies(cx.sum.query_id(&0).unwrap()), vec![cx.leaf.query_id(&0).unwrap()]);
    cx.edit(0, 12);
    assert_eq!(cx.sum(5), Ok(12));
    assert_eq!(cx.engine.dependencies(cx.sum.query_id(&5).unwrap()), vec![QueryId { kind: 0, key: 2 }, cx.sum.query_id(&0).unwrap()]);
}

#[test]
fn diamond_computes_shared_dependency_once_per_revision() {
    let mut cx = Cx::new();
    assert_eq!(cx.sum(2), Ok(5));
    cx.edit(0, 20);
    assert_eq!(cx.sum(2), Ok(9));
    assert_eq!(cx.count(1, 0), 2);
    assert_eq!(cx.count(1, 1), 1);
    for key in 0..3 {
        assert_eq!(cx.count(2, key), 2);
    }
    cx.edit(0, 21);
    assert_eq!(cx.sum(2), Ok(9));
    assert_eq!(cx.count(1, 0), 3);
    for key in 0..3 {
        assert_eq!(cx.count(2, key), 2);
    }
}

#[test]
fn skipped_revisions_still_notice_earlier_changes() {
    let mut cx = Cx::new();
    assert_eq!(cx.sum(0), Ok(2));
    cx.edit(0, 20);
    assert_eq!(cx.leaf(0), Ok(2));
    cx.edit(15, 30);
    assert_eq!(cx.sum(0), Ok(4));
    assert_eq!(cx.count(2, 0), 2);
    cx.edit(0, 30);
    cx.edit(15, 40);
    assert_eq!(cx.sum(0), Ok(6));
    assert_eq!(cx.count(2, 0), 3);
}

#[test]
fn validation_stops_before_obsolete_branch_and_replaces_edges() {
    let mut cx = Cx::new();
    assert_eq!(cx.sum(3), Ok(1));
    cx.edit(0, 20);
    cx.edit(2, 11);
    assert_eq!(cx.sum(3), Ok(1));
    // The old branch must not be forced after the controlling input turned red.
    assert_eq!(cx.count(1, 0), 1);
    assert_eq!(cx.count(1, 1), 1);
    assert_eq!(cx.engine.dependencies(cx.sum.query_id(&3).unwrap()), vec![QueryId { kind: 0, key: 2 }, cx.leaf.query_id(&1).unwrap()]);
    cx.edit(0, 30);
    assert_eq!(cx.sum(3), Ok(1));
    assert_eq!(cx.count(2, 3), 2);
}

#[test]
fn concurrency_same_and_different_keys_compute_once() {
    let mut cx = Cx::new();
    for revision in 0..2 {
        if revision == 1 {
            cx.edit(0, 20);
        }
        let start = Barrier::new(8);
        thread::scope(|scope| {
            let mut threads = Vec::new();
            for worker in 0..8 {
                let cx = &cx;
                let start = &start;
                threads.push(scope.spawn(move || {
                    start.wait();
                    for key in 0..8 {
                        let key = (key + worker) % 8;
                        assert_eq!(cx.leaf(key), Ok(if revision == 1 && key == 0 { 2 } else { 1 }));
                    }
                    assert_eq!(cx.sum(2), Ok(if revision == 0 { 5 } else { 9 }));
                }));
            }
            for thread in threads {
                thread.join().unwrap();
            }
        });
    }
    for key in 0..8 {
        assert_eq!(cx.count(1, key), if key == 0 { 2 } else { 1 });
    }
    for key in 0..3 {
        assert_eq!(cx.count(2, key), 2);
    }
}

struct RecursiveCx {
    engine: QueryEngine,
    table: QueryTable<u32, u64>,
    rendezvous: Option<Barrier>,
    nested: bool,
}

impl RecursiveCx {
    fn query(&self, key: u32) -> Result<u64, QueryError> {
        self.table.get(
            &self.engine,
            key,
            &mut |key| {
                if key < 2 {
                    if let Some(barrier) = &self.rendezvous {
                        barrier.wait();
                    }
                }
                let next = match (self.nested, key) {
                    (true, 0) => 2,
                    (_, 0) => 1,
                    (_, 1) => 0,
                    (_, 2) => 1,
                    _ => unreachable!(),
                };
                self.query(next)
            },
            &no_force,
        )
    }
}

fn assert_cycle(result: Result<u64, QueryError>, length: usize) {
    match result {
        Err(QueryError::Cycle(cycle)) => {
            assert_eq!(cycle.stack.len(), length);
            assert_eq!(cycle.stack.first(), cycle.stack.last());
        }
        other => panic!("expected cycle, got {other:?}"),
    }
}

#[test]
fn cycle_within_one_thread() {
    let cx = RecursiveCx { engine: QueryEngine::new(), table: QueryTable::new(1, fp, value_fp), rendezvous: None, nested: false };
    assert_cycle(cx.query(0), 3);
    assert_cycle(cx.query(1), 3);
}

#[test]
fn cycle_across_two_threads_includes_synchronous_children() {
    for nested in [false, true] {
        let cx = Arc::new(RecursiveCx {
            engine: QueryEngine::new(),
            table: QueryTable::new(1, fp, value_fp),
            rendezvous: Some(Barrier::new(2)),
            nested,
        });
        let (send, recv) = mpsc::channel();
        let mut threads = Vec::new();
        for key in 0..2 {
            let cx = Arc::clone(&cx);
            let send = send.clone();
            threads.push(thread::spawn(move || send.send(cx.query(key)).unwrap()));
        }
        for _ in 0..2 {
            assert_cycle(recv.recv_timeout(Duration::from_secs(5)).expect("cycle deadlocked"), if nested { 4 } else { 3 });
        }
        for thread in threads {
            thread.join().unwrap();
        }
    }
}

#[test]
fn panic_poison_wakes_blocked_waiters_and_restores_task_stack() {
    let engine = Arc::new(QueryEngine::new());
    let table = Arc::new(QueryTable::new(1, fp, value_fp));
    let (started_send, started_recv) = mpsc::channel();
    let (release_send, release_recv) = mpsc::channel();
    let owner = {
        let engine = Arc::clone(&engine);
        let table = Arc::clone(&table);
        thread::spawn(move || {
            table.get(
                &engine,
                0,
                &mut |_| {
                    started_send.send(()).unwrap();
                    release_recv.recv().unwrap();
                    panic!("provider panic");
                },
                &no_force,
            )
        })
    };
    started_recv.recv_timeout(Duration::from_secs(5)).unwrap();
    let (send, recv) = mpsc::channel();
    let mut waiters = Vec::new();
    for _ in 0..7 {
        let engine = Arc::clone(&engine);
        let table = Arc::clone(&table);
        let send = send.clone();
        waiters.push(thread::spawn(move || send.send(table.get(&engine, 0, &mut |_| panic!("second executor"), &no_force)).unwrap()));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = engine.state.lock().unwrap();
        let waiting = match &state.jobs[&DepNodeIndex(0)].latch {
            Some(latch) => Arc::strong_count(latch) == 8,
            None => false,
        };
        drop(state);
        if waiting {
            break;
        }
        assert!(Instant::now() < deadline, "waiter never blocked");
        thread::yield_now();
    }
    release_send.send(()).unwrap();
    assert!(owner.join().is_err());
    let query = table.query_id(&0).unwrap();
    for _ in 0..7 {
        assert_eq!(recv.recv_timeout(Duration::from_secs(5)).unwrap(), Err(QueryError::Poisoned(query)));
    }
    for waiter in waiters {
        waiter.join().unwrap();
    }
    assert_eq!(table.get(&engine, 0, &mut |_| Ok(7), &no_force), Err(QueryError::Poisoned(query)));
    assert_eq!(table.get(&engine, 1, &mut |_| Ok(8), &no_force), Ok(8));

    // Catch an unwind on this thread, then verify TLS has no abandoned task/job.
    let local_engine = QueryEngine::new();
    let local_table = QueryTable::new(2, fp, value_fp);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        local_table.get(
            &local_engine,
            0,
            &mut |_| {
                local_engine.with_anon_task(&mut || -> Result<(), QueryError> { panic!("anonymous panic") }).0?;
                Ok(0)
            },
            &no_force,
        )
    }));
    assert!(panic.is_err());
    assert_eq!(local_table.get(&local_engine, 1, &mut |_| Ok(9), &no_force), Ok(9));
    assert!(local_engine.dependencies(local_table.query_id(&1).unwrap()).is_empty());
}

#[test]
fn table_ownership_and_missing_inputs_are_checked() {
    let mut engine = QueryEngine::new();
    let table = QueryTable::new(1, fp, value_fp);
    assert_eq!(table.get(&engine, 0, &mut |_| Ok(1), &no_force), Ok(1));
    let duplicate = QueryTable::new(1, fp, value_fp);
    assert_eq!(duplicate.get(&engine, 0, &mut |_| Ok(2), &no_force), Err(QueryError::KindConflict(1)));
    assert_eq!(table.get(&QueryEngine::new(), 0, &mut |_| Ok(2), &no_force), Err(QueryError::WrongEngine));
    assert_eq!(engine.register_input(1, 0, fp(&0)), Err(QueryError::KindConflict(1)));
    let input = QueryId { kind: 0, key: 0 };
    assert_eq!(engine.set_input(0, 0, value_fp(&0)), Err(QueryError::MissingInput(input)));
    engine.register_input(0, 0, fp(&0)).unwrap();
    assert_eq!(engine.input(0, 0), Err(QueryError::MissingInput(input)));
    assert_eq!(engine.register_input(0, 1, fp(&0)), Err(QueryError::IdentityConflict(QueryId { kind: 0, key: 1 })));
}

#[test]
fn edge_storage_is_bounded_across_edits() {
    let mut cx = Cx::new();
    for value in 10..500 {
        cx.edit(0, value * 10);
        cx.sum(4).unwrap();
    }
    let state = cx.engine.state.lock().unwrap();
    assert_eq!(state.graph.nodes.len(), 27);
    assert!(state.graph.edges.len() < 1100, "{} stored edges", state.graph.edges.len());
}

#[test]
fn incomplete_force_is_an_error_and_next_revision_can_retry() {
    let mut cx = Cx::new();
    assert_eq!(cx.sum(0), Ok(2));
    cx.edit(15, 20);
    let dependency = cx.leaf.query_id(&0).unwrap();
    assert_eq!(
        cx.sum.get(&cx.engine, 0, &mut |_| panic!("must not compute after force failure"), &|_| Ok(())),
        Err(QueryError::ForceDidNotComplete(dependency))
    );
    assert_eq!(cx.sum(0), Err(QueryError::ForceDidNotComplete(dependency)));
    cx.edit(15, 30);
    assert_eq!(cx.sum(0), Ok(2));
    assert_eq!(cx.count(1, 0), 1);
    assert_eq!(cx.count(2, 0), 1);
}

// Initialize cleanup before both the query context and a destructor-owning TLS
// marker. Its callback must run after the marker's storage is unavailable.
fn in_late_destructor(check: fn(&Cx)) {
    struct Marker(Arc<AtomicUsize>);
    impl Drop for Marker {
        fn drop(&mut self) { self.0.store(1, Ordering::SeqCst); }
    }
    struct Cleanup { cx: Cx, marker: Arc<AtomicUsize>, check: fn(&Cx), send: mpsc::Sender<bool> }
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_eq!(self.marker.load(Ordering::SeqCst), 1);
                assert!(MARKER.try_with(|_| ()).is_err());
                (self.check)(&self.cx);
                assert_eq!(dep_graph::current_job(self.cx.engine.id), None);
                assert_eq!(self.cx.leaf(15), Ok(1));
            }));
            self.send.send(result.is_ok()).unwrap();
        }
    }
    thread_local! {
        static CLEANUP: std::cell::RefCell<Option<Cleanup>> = const { std::cell::RefCell::new(None) };
        static MARKER: std::cell::RefCell<Option<Marker>> = const { std::cell::RefCell::new(None) };
    }
    let (send, recv) = mpsc::channel();
    thread::spawn(move || {
        let marker = Arc::new(AtomicUsize::new(0));
        CLEANUP.with(|slot| {
            *slot.borrow_mut() = Some(Cleanup { cx: Cx::new(), marker: marker.clone(), check, send });
            assert_eq!(slot.borrow().as_ref().unwrap().cx.leaf(0), Ok(1));
        });
        MARKER.with(|slot| *slot.borrow_mut() = Some(Marker(marker)));
    }).join().unwrap();
    assert!(recv.recv_timeout(Duration::from_secs(5)).unwrap(), "late TLS query failed");
}

#[test]
fn late_tls_cached_hit() {
    in_late_destructor(|cx| {
        assert_eq!(cx.leaf(0), Ok(1));
        assert_eq!(cx.count(1, 0), 1);
    });
}

#[test]
fn late_tls_provider_execution() {
    in_late_destructor(|cx| {
        assert_eq!(cx.leaf(1), Ok(1));
        assert_eq!(cx.count(1, 1), 1);
        assert_eq!(cx.engine.dependencies(cx.leaf.query_id(&1).unwrap()), vec![QueryId { kind: 0, key: 1 }]);
    });
}

#[test]
fn late_tls_nested_reads() {
    in_late_destructor(|cx| {
        assert_eq!(cx.sum(1), Ok(3));
        assert_anonymous_sum_edges(cx);
        let table = QueryTable::new(3, fp, value_fp);
        assert_eq!(table.get(&cx.engine, 0, &mut |_| {
            for key in 0..16 { cx.input(key)?; cx.leaf(key)?; }
            Ok(0)
        }, &no_force), Ok(0));
        assert_eq!(cx.engine.dependencies(table.query_id(&0).unwrap()).len(), 32);
    });
}

#[test]
fn late_tls_cycle() {
    in_late_destructor(|_| {
        let cx = RecursiveCx { engine: QueryEngine::new(), table: QueryTable::new(1, fp, value_fp), rendezvous: None, nested: true };
        assert_cycle(cx.query(0), 4);
    });
}

#[test]
fn late_tls_provider_panic() {
    in_late_destructor(|cx| {
        let table = QueryTable::new(3, fp, value_fp);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            table.get(&cx.engine, 0, &mut |_| {
                cx.engine.with_anon_task(&mut || -> Result<(), QueryError> { cx.leaf(1)?; panic!("provider panic in TLS cleanup"); }).0?;
                Ok(0)
            }, &no_force)
        }));
        assert!(result.is_err());
        assert_eq!(table.get(&cx.engine, 0, &mut |_| Ok(0), &no_force), Err(QueryError::Poisoned(table.query_id(&0).unwrap())));
        assert_eq!(table.get(&cx.engine, 1, &mut |_| Ok(9), &no_force), Ok(9));
        assert!(cx.engine.dependencies(table.query_id(&1).unwrap()).is_empty());
    });
}

#[test]
fn spilled_task_reads_preserve_order_and_deduplicate() {
    let mut engine = QueryEngine::new();
    for key in 0..40 {
        engine.register_input(0, key, fp(&key)).unwrap();
        engine.set_input(0, key, value_fp(&0)).unwrap();
    }
    let table = QueryTable::new(1, fp, value_fp);
    table.get(&engine, 0, &mut |_| {
        engine.with_anon_task(&mut || {
            for _ in 0..2 { for key in 0..40 { engine.input(0, key)?; } }
            Ok::<_, QueryError>(())
        }).0?;
        Ok(0)
    }, &no_force).unwrap();
    let mut expected = Vec::new();
    for key in 0..40 { expected.push(QueryId { kind: 0, key }); }
    {
        let state = engine.state.lock().unwrap();
        let root = state.graph.find(table.query_id(&0).unwrap()).unwrap();
        let edges = state.graph.reads(root);
        assert_eq!(edges.len(), 1);
        let anon = DepNodeIndex(edges[0]);
        assert_eq!(state.graph.query(anon), None);
        let reads: Vec<_> = state.graph.reads(anon).iter().map(|&i| state.graph.query(DepNodeIndex(i)).unwrap()).collect();
        assert_eq!(reads, expected);
    }
    table.get(&engine, 1, &mut |_| {
        for key in 20..40 { engine.input(0, key)?; }
        Ok(0)
    }, &no_force).unwrap();
    assert_eq!(engine.dependencies(table.query_id(&1).unwrap()), expected[20..]);
}

#[test]
#[ignore = "micro-benchmark; run once with --ignored --nocapture"]
fn micro_benchmark() {
    use std::hint::black_box;
    let mut cx = Cx::new();
    cx.leaf(0).unwrap();
    let n = 100_000;
    let start = Instant::now();
    for _ in 0..n {
        black_box(cx.leaf(black_box(0)).unwrap());
    }
    let hit = start.elapsed().as_nanos() as f64 / n as f64;

    let mut execute_ns = 0;
    for value in 100..10_100 {
        cx.edit(0, value);
        let start = Instant::now();
        black_box(cx.leaf(0).unwrap());
        execute_ns += start.elapsed().as_nanos();
    }
    cx.sum(4).unwrap();
    let mut green_ns = 0;
    for value in 100..10_100 {
        cx.edit(15, value);
        // Time just the ten-dependency validation, with its dependencies current.
        for key in 0..10 {
            cx.leaf(key).unwrap();
        }
        let start = Instant::now();
        black_box(cx.sum(4).unwrap());
        green_ns += start.elapsed().as_nanos();
    }
    println!(
        "query benchmark: hit {hit:.1} ns; tracked trivial execution {:.1} ns; try_mark_green 10 deps {:.1} ns",
        execute_ns as f64 / 10_000.0,
        green_ns as f64 / 10_000.0
    );
}

#[test]
fn independent_context_restores_enclosing_query_after_unwind() {
    let outer = Cx::new();
    let inner = Cx::new();
    let table = QueryTable::new(3, fp, value_fp);
    let value = table.get(&outer.engine, 0, &mut |_| {
        outer.input(0)?;
        assert_eq!(inner.engine.enter_context(|| inner.leaf(1)), Ok(1));
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            inner.engine.enter_context(|| {
                inner.input(2).unwrap();
                panic!("nested compilation failed");
            });
        }));
        assert!(panic.is_err());
        outer.engine.enter_context(|| outer.input(3))
    }, &no_force).unwrap();
    assert_eq!(value, 10);
    assert_eq!(outer.engine.dependencies(table.query_id(&0).unwrap()),
        vec![QueryId { kind: 0, key: 0 }, QueryId { kind: 0, key: 3 }]);
    assert_eq!(inner.engine.dependencies(inner.leaf.query_id(&1).unwrap()),
        vec![QueryId { kind: 0, key: 1 }]);
}

#[test]
fn round42_nested_cancellation_discards_reads_and_retries_after_edit() {
    let mut engine = QueryEngine::new();
    for key in 0..32 {
        engine.register_input(0, key, fp(&key)).unwrap();
        engine.set_input(0, key, value_fp(&10)).unwrap();
    }
    let parent = QueryTable::new(1, fp, value_fp);
    let child = QueryTable::new(2, fp, value_fp);
    let cancelled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        parent.get(&engine, 0, &mut |_| {
            engine.input(0, 0)?;
            child.get(&engine, 0, &mut |_| {
                // Exercise both inline and pooled read frames, including an anonymous task.
                engine.with_anon_task(&mut || {
                    for key in 0..32 { engine.input(0, key).unwrap(); }
                    std::panic::resume_unwind(Box::new(crate::preempt::Preempted));
                });
                Ok(0)
            }, &no_force)
        }, &no_force)
    }));
    assert!(cancelled.is_err_and(|p| crate::preempt::is_preempted(&*p)));
    for query in [parent.query_id(&0).unwrap(), child.query_id(&0).unwrap()] {
        assert_eq!(engine.color(query), DepNodeColor::Unknown);
        assert!(engine.dependencies(query).is_empty());
        let state = engine.state.lock().unwrap();
        let index = state.graph.find(query).unwrap().0 as usize;
        assert!(state.graph.fingerprint(DepNodeIndex(index as u32)).is_none());
        assert!(!state.graph.errors.contains_key(&DepNodeIndex(index as u32)));
        assert!(!state.jobs.contains_key(&DepNodeIndex(index as u32)));
        assert!(state.jobs.get(&DepNodeIndex(index as u32)).and_then(|job| job.waiting_on).is_none());
    }
    let mut published = 0;
    parent.for_each(&mut |_, _, _| published += 1);
    child.for_each(&mut |_, _, _| published += 1);
    assert_eq!(published, 0);
    assert!(dep_graph::current_job(engine.id).is_none());
    engine.set_input(0, 31, value_fp(&20)).unwrap();
    assert_eq!(parent.get(&engine, 0, &mut |_| child.get(&engine, 0, &mut |_| {
        engine.input(0, 31)?;
        Ok(20)
    }, &no_force), &no_force), Ok(20));
    assert_eq!(engine.dependencies(child.query_id(&0).unwrap()), vec![QueryId { kind: 0, key: 31 }]);
    assert_eq!(engine.dependencies(parent.query_id(&0).unwrap()), vec![child.query_id(&0).unwrap()]);
}

/// The post-boot loader's NeedsLoad (a constant waiting for a skipped module) crosses
/// nested queries like a preemption: neither caches a value or an error (not Poisoned), and
/// the same key computes normally on the next request, in the same revision.
#[test]
fn needs_load_cancels_nested_queries_without_caching() {
    let mut engine = QueryEngine::new();
    engine.register_input(0, 0, fp(&0)).unwrap();
    engine.set_input(0, 0, value_fp(&1)).unwrap();
    let parent = QueryTable::new(1, fp, value_fp);
    let child = QueryTable::new(2, fp, value_fp);
    let cancelled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        parent.get(&engine, 0, &mut |_| {
            engine.input(0, 0)?;
            child.get(&engine, 0, &mut |_| -> Result<u64, QueryError> {
                std::panic::resume_unwind(Box::new(crate::preempt::NeedsLoad));
            }, &no_force)
        }, &no_force)
    }));
    assert!(cancelled.is_err_and(|p| p.is::<crate::preempt::NeedsLoad>()));
    for query in [parent.query_id(&0).unwrap(), child.query_id(&0).unwrap()] {
        let state = engine.state.lock().unwrap();
        let index = state.graph.find(query).unwrap().0 as usize;
        assert!(state.graph.fingerprint(DepNodeIndex(index as u32)).is_none());
        assert!(!state.graph.errors.contains_key(&DepNodeIndex(index as u32)));
        assert!(!state.jobs.contains_key(&DepNodeIndex(index as u32)));
    }
    let mut published = 0;
    parent.for_each(&mut |_, _, _| published += 1);
    child.for_each(&mut |_, _, _| published += 1);
    assert_eq!(published, 0);
    assert_eq!(parent.get(&engine, 0, &mut |_| child.get(&engine, 0, &mut |_| Ok(7), &no_force), &no_force), Ok(7));
}

#[test]
fn round42_cancelled_owner_wakes_foreground_and_preempts_background_waiters() {
    let engine = Arc::new(QueryEngine::new());
    let table = Arc::new(QueryTable::new(1, fp, value_fp));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let owner = {
        let engine = engine.clone(); let table = table.clone();
        thread::spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| table.get(&engine, 0, &mut |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                std::panic::resume_unwind(Box::new(crate::preempt::Preempted));
            }, &no_force)))
        })
    };
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (background_tx, background_rx) = mpsc::channel();
    let background = {
        let engine = engine.clone(); let table = table.clone();
        thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _p = crate::preempt::Preemptible::on();
                table.get(&engine, 0, &mut |_| panic!("background must not compute"), &no_force)
            }));
            background_tx.send(r.is_err_and(|p| crate::preempt::is_preempted(&*p))).unwrap();
        })
    };
    let runs = Arc::new(AtomicUsize::new(0));
    let mut foreground = Vec::new();
    for _ in 0..3 {
        let engine = engine.clone(); let table = table.clone(); let runs = runs.clone();
        foreground.push(thread::spawn(move || table.get(&engine, 0, &mut |_| {
            runs.fetch_add(1, Ordering::Relaxed); Ok(42)
        }, &no_force)));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = engine.state.lock().unwrap();
        let waiting = state.jobs[&DepNodeIndex(0)].latch.as_ref().is_some_and(|l| Arc::strong_count(l) == 5);
        drop(state);
        if waiting { break; }
        assert!(Instant::now() < deadline);
        thread::yield_now();
    }
    let writer = crate::preempt::WriterWaiting::new();
    // The background waiter must unwind on a timed wait, before the owner finishes.
    assert!(background_rx.recv_timeout(Duration::from_secs(5)).unwrap());
    background.join().unwrap();
    release_tx.send(()).unwrap();
    assert!(owner.join().unwrap().is_err_and(|p| crate::preempt::is_preempted(&*p)));
    for waiter in foreground { assert_eq!(waiter.join().unwrap(), Ok(42)); }
    assert_eq!(runs.load(Ordering::Relaxed), 1);
    drop(writer);
    assert_eq!(table.get(&engine, 0, &mut |_| panic!("must reuse completed retry"), &no_force), Ok(42));
}

#[test]
fn round42_cancellation_during_green_validation_preserves_previous_revision() {
    let mut cx = Cx::new();
    assert_eq!(cx.sum(0), Ok(2));
    let query = cx.sum.query_id(&0).unwrap();
    let deps = cx.engine.dependencies(query);
    cx.edit(0, 20);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cx.sum.get(&cx.engine, 0, &mut |_| panic!("validation must run first"), &|_| {
            std::panic::resume_unwind(Box::new(crate::preempt::Preempted));
        })
    }));
    assert!(result.is_err_and(|p| crate::preempt::is_preempted(&*p)));
    assert_eq!(cx.engine.color(query), DepNodeColor::Unknown);
    assert_eq!(cx.engine.dependencies(query), deps);
    assert_eq!(cx.sum(0), Ok(4));
    assert_eq!(cx.sum_runs[0].load(Ordering::Relaxed), 2);
}

/// Revisions past the inline encoding (>= 32767) go to the overflow map: a node created before the
/// boundary and validated after it, and nodes created after it, must keep red/green answers.
#[test]
fn revisions_past_the_inline_encoding_keep_red_green_answers() {
    let mut cx = Cx::new();
    let before = cx.sum(0);
    assert!(before.is_ok());
    let runs_before = cx.count(2, 0);
    for i in 0..33_000u64 {
        cx.edit(15, 100 + (i & 1));
    }
    cx.edit(15, 10);
    assert!(cx.engine.revision() > 32_767);
    assert_eq!(cx.sum(0), before);
    cx.edit(0, 20);
    let changed = cx.sum(0);
    assert!(changed.is_ok());
    assert!(cx.count(2, 0) > runs_before);
    let runs = cx.count(2, 0);
    assert_eq!(cx.sum(0), changed);
    assert_eq!(cx.count(2, 0), runs);
}
