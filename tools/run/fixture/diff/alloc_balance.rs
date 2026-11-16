// diff_fast corpus: rustc --edition 2021 -O --test and makepad test must agree.
// Each round warms each case once, then measures 200 calls without printing.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hint::black_box;
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, Ordering::SeqCst};

struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;
static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);
static LIVE_ALLOCS: AtomicIsize = AtomicIsize::new(0);
const N: usize = 200;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as isize, SeqCst);
            LIVE_ALLOCS.fetch_add(1, SeqCst);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as isize, SeqCst);
            LIVE_ALLOCS.fetch_add(1, SeqCst);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE_BYTES.fetch_sub(layout.size() as isize, SeqCst);
        LIVE_ALLOCS.fetch_sub(1, SeqCst);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, size);
        if !new_ptr.is_null() {
            // A failed realloc leaves the old allocation and both counters intact.
            LIVE_BYTES.fetch_add(size as isize - layout.size() as isize, SeqCst);
        }
        new_ptr
    }
}

fn snapshot() -> (isize, isize) {
    (LIVE_BYTES.load(SeqCst), LIVE_ALLOCS.load(SeqCst))
}

// Exercise dispatch through the global hooks so an unused allocator cannot pass silently.
fn check_allocator() {
    unsafe {
        let before = snapshot();
        let layout = Layout::from_size_align(32, 8).unwrap();
        let ptr = black_box(std::alloc::alloc(layout));
        assert!(!ptr.is_null());
        assert_eq!(snapshot(), (before.0 + 32, before.1 + 1));
        let grown = black_box(std::alloc::realloc(ptr, layout, 96));
        assert!(!grown.is_null());
        assert_eq!(snapshot(), (before.0 + 96, before.1 + 1));
        let shrunk = black_box(std::alloc::realloc(
            grown,
            Layout::from_size_align(96, 8).unwrap(),
            16,
        ));
        assert!(!shrunk.is_null());
        assert_eq!(snapshot(), (before.0 + 16, before.1 + 1));
        std::alloc::dealloc(shrunk, Layout::from_size_align(16, 8).unwrap());
        assert_eq!(snapshot(), before);
        let zeroed = black_box(std::alloc::alloc_zeroed(layout));
        assert!(!zeroed.is_null());
        assert_eq!(snapshot(), (before.0 + 32, before.1 + 1));
        assert_eq!(*zeroed, 0);
        std::alloc::dealloc(zeroed, layout);
        assert_eq!(snapshot(), before);
    }
}

#[inline(never)]
fn make_vec() -> Vec<String> {
    black_box(vec![
        "alpha".to_string(),
        "beta".to_string(),
        "gamma".to_string(),
        "delta".to_string(),
    ])
}

#[inline(never)]
fn make_map() -> HashMap<String, Vec<u8>> {
    let mut map = HashMap::new();
    for key in ["alpha", "beta", "gamma", "delta"] {
        map.insert(key.to_string(), vec![7u8; 32]);
    }
    black_box(map)
}

#[inline(never)]
fn make_option() -> Option<Vec<String>> {
    black_box(Some(make_vec()))
}

// Return with the implicit Vec iterator and three unconsumed strings live.
#[inline(never)]
fn vec_for<const ANSWER: bool>() -> bool {
    for value in make_vec() {
        black_box(&value);
        return ANSWER;
    }
    !ANSWER
}

// Return with an explicit owning Vec iterator and its current element live.
#[inline(never)]
fn vec_into_iter<const ANSWER: bool>() -> bool {
    for value in make_vec().into_iter() {
        black_box(&value);
        return ANSWER;
    }
    !ANSWER
}

// Return while a HashMap iterator still owns its table, keys, and value buffers.
#[inline(never)]
fn hashmap_into_iter<const ANSWER: bool>() -> bool {
    for entry in make_map().into_iter() {
        black_box(&entry);
        return ANSWER;
    }
    !ANSWER
}

// Return while a BTreeMap iterator still owns nodes and unvisited entries.
#[inline(never)]
fn btreemap_into_iter<const ANSWER: bool>() -> bool {
    let map: BTreeMap<String, Vec<u8>> = make_map().into_iter().collect();
    for entry in map.into_iter() {
        black_box(&entry);
        return ANSWER;
    }
    !ANSWER
}

// Drop a partial Drain, its unyielded element, and the Vec's prefix and suffix.
#[inline(never)]
fn vec_drain<const ANSWER: bool>() -> bool {
    let mut values = make_vec();
    for value in values.drain(1..3) {
        black_box(&value);
        return ANSWER;
    }
    !ANSWER
}

// Return before a VecDeque owning iterator reaches its remaining strings.
#[inline(never)]
fn deque_into_iter<const ANSWER: bool>() -> bool {
    let values: VecDeque<String> = make_vec().into_iter().collect();
    for value in values.into_iter() {
        black_box(&value);
        return ANSWER;
    }
    !ANSWER
}

// all(false) short-circuits; all(true) exhausts the same owning iterator.
#[inline(never)]
fn iter_all<const ANSWER: bool>() -> bool {
    make_vec().into_iter().all(|value| {
        black_box(&value);
        black_box(ANSWER)
    })
}

// any(true) short-circuits; any(false) exhausts the same owning iterator.
#[inline(never)]
fn iter_any<const ANSWER: bool>() -> bool {
    make_vec().into_iter().any(|value| {
        black_box(&value);
        black_box(ANSWER)
    })
}

// Drop a find result and a partially consumed temporary iterator, or exhaust it.
#[inline(never)]
fn iter_find<const ANSWER: bool>() -> bool {
    let found = make_vec().into_iter().find(|value| {
        black_box(value);
        black_box(ANSWER)
    });
    black_box(&found);
    found.is_some()
}

// position must drop both the tested element and any unconsumed elements.
#[inline(never)]
fn iter_position<const ANSWER: bool>() -> bool {
    make_vec()
        .into_iter()
        .position(|value| {
            black_box(&value);
            black_box(ANSWER)
        })
        .is_some()
}

// A failing try_fold owns an accumulator and leaves elements in its iterator.
#[inline(never)]
fn iter_try_fold<const ANSWER: bool>() -> bool {
    let result: Result<String, String> =
        make_vec()
            .into_iter()
            .try_fold("seed".to_string(), |mut acc, value| {
                acc.push_str(&value);
                Err(acc)
            });
    black_box(&result);
    assert!(result.is_err());
    return ANSWER;
}

// take_while rejects its first owned element, leaving the underlying tail live.
#[inline(never)]
fn iter_take_while<const ANSWER: bool>() -> bool {
    let mut iter = make_vec().into_iter().take_while(|value| {
        black_box(value);
        false
    });
    assert!(iter.next().is_none());
    return ANSWER;
}

// Exhaust zip's left side first; the right side still owns multiple strings.
#[inline(never)]
fn iter_zip_left<const ANSWER: bool>() -> bool {
    let left = vec!["short".to_string()];
    let mut iter = left.into_iter().zip(make_vec());
    while let Some(pair) = iter.next() {
        black_box(&pair);
    }
    return ANSWER;
}

// Exhaust zip's right side first, including the unmatched item fetched on the left.
#[inline(never)]
fn iter_zip_right<const ANSWER: bool>() -> bool {
    let right = vec!["short".to_string()];
    let mut iter = make_vec().into_iter().zip(right);
    while let Some(pair) = iter.next() {
        black_box(&pair);
    }
    return ANSWER;
}

// Return in chain's first iterator, leaving its second iterator untouched.
#[inline(never)]
fn iter_chain<const ANSWER: bool>() -> bool {
    let mut iter = make_vec().into_iter().chain(make_vec());
    let first = iter.next().unwrap();
    black_box(&first);
    return ANSWER;
}

// flat_map must drop its active owned inner iterator and its unvisited outer tail.
#[inline(never)]
fn iter_flat_map<const ANSWER: bool>() -> bool {
    let mut iter = make_vec()
        .into_iter()
        .flat_map(|value| vec![value, "inner".to_string()]);
    let first = iter.next().unwrap();
    black_box(&first);
    return ANSWER;
}

// flatten owns a partially consumed inner Vec iterator plus untouched inner Vecs.
#[inline(never)]
fn iter_flatten<const ANSWER: bool>() -> bool {
    let mut iter = vec![make_vec(), make_vec()].into_iter().flatten();
    let first = iter.next().unwrap();
    black_box(&first);
    return ANSWER;
}

// peekable holds one cached owned string in addition to its underlying iterator.
#[inline(never)]
fn iter_peekable<const ANSWER: bool>() -> bool {
    let mut iter = make_vec().into_iter().peekable();
    black_box(iter.peek().unwrap());
    return ANSWER;
}

// filter_map rejects one string, yields the next, and retains an owning tail.
#[inline(never)]
fn iter_filter_map<const ANSWER: bool>() -> bool {
    let mut iter =
        make_vec().into_iter().filter_map(
            |value| {
                if value == "alpha" {
                    None
                } else {
                    Some(value)
                }
            },
        );
    let first = iter.next().unwrap();
    black_box(&first);
    return ANSWER;
}

// skip_while drops skipped strings and retains the tail after its first yield.
#[inline(never)]
fn iter_skip_while<const ANSWER: bool>() -> bool {
    let mut iter = make_vec().into_iter().skip_while(|value| value == "alpha");
    let first = iter.next().unwrap();
    black_box(&first);
    return ANSWER;
}

trait Payload {
    fn len(&self) -> usize;
}
impl Payload for Vec<String> {
    fn len(&self) -> usize {
        self.len()
    }
}

#[inline(never)]
fn make_box() -> Box<dyn Payload> {
    black_box(Box::new(make_vec()))
}

#[inline(never)]
fn missing() -> Option<String> {
    black_box(None)
}

#[inline(never)]
fn failure() -> Result<String, String> {
    black_box(Err("failure".to_string()))
}

#[inline(never)]
fn option_body() -> Option<bool> {
    let text = "local".to_string();
    let values = make_vec();
    let object = make_box();
    let shared = Rc::new(RefCell::new(make_vec()));
    let alias = Rc::clone(&shared);
    let map = make_map();
    black_box((&text, &values, object.len(), &shared, &alias, &map));
    let value = missing()?;
    black_box(value);
    Some(true)
}

// None propagates through ? while five kinds of owned local and an Rc alias live.
fn option_locals() {
    assert!(option_body().is_none());
}

#[inline(never)]
fn result_body() -> Result<bool, String> {
    let text = "local".to_string();
    let values = make_vec();
    let object = make_box();
    let shared = Rc::new(RefCell::new(make_vec()));
    let alias = Rc::clone(&shared);
    let map = make_map();
    black_box((&text, &values, object.len(), &shared, &alias, &map));
    let value = failure()?;
    black_box(value);
    Ok(true)
}

// Err propagates through ?; the caller also drops the owned error payload.
fn result_locals() {
    assert!(result_body().is_err());
}

#[inline(never)]
fn option_temporary_body() -> Option<bool> {
    let outer = make_vec();
    black_box(&outer);
    let pending = (
        "temporary".to_string(),
        make_vec(),
        make_box(),
        Rc::new(RefCell::new(make_vec())),
        make_map(),
        missing()?,
    );
    black_box(pending);
    Some(true)
}

// ? in a tuple's last operand drops all five already evaluated owning temporaries.
fn option_temporaries() {
    assert!(option_temporary_body().is_none());
}

#[inline(never)]
fn result_temporary_body() -> Result<bool, String> {
    let outer = make_vec();
    black_box(&outer);
    let pending = (
        "temporary".to_string(),
        make_vec(),
        make_box(),
        Rc::new(RefCell::new(make_vec())),
        make_map(),
        failure()?,
    );
    black_box(pending);
    Ok(true)
}

// Err in a tuple's last operand drops pending temporaries and returns the error.
fn result_temporaries() {
    assert!(result_temporary_body().is_err());
}

#[inline(never)]
fn option_match_body() -> Option<bool> {
    let outer = make_map();
    black_box(&outer);
    match make_option() {
        Some(values) => {
            let arm = make_box();
            black_box((&values, arm.len()));
            let value = missing()?;
            black_box(value);
            Some(true)
        }
        None => Some(false),
    }
}

// ? leaves a match arm with owned bindings, arm locals, and enclosing locals.
fn option_match() {
    assert!(option_match_body().is_none());
}

#[inline(never)]
fn result_match_body() -> Result<bool, String> {
    let outer = make_map();
    black_box(&outer);
    match make_option() {
        Some(values) => {
            let arm = make_box();
            black_box((&values, arm.len()));
            let value = failure()?;
            black_box(value);
            Ok(true)
        }
        None => Ok(false),
    }
}

// Err leaves a match arm and its surrounding scope, carrying a heap error outward.
fn result_match() {
    assert!(result_match_body().is_err());
}

#[inline(never)]
fn option_closure_body() -> Option<bool> {
    let outer = make_map();
    let capture = make_vec();
    let call = move || -> Option<bool> {
        let captured = capture;
        let inner = make_box();
        black_box((&captured, inner.len()));
        let value = missing()?;
        black_box(value);
        Some(true)
    };
    black_box(&outer);
    call()?;
    Some(true)
}

// ? exits an owning FnOnce closure, then ? exits its caller with outer locals live.
fn option_closure() {
    assert!(option_closure_body().is_none());
}

#[inline(never)]
fn result_closure_body() -> Result<bool, String> {
    let outer = make_map();
    let capture = make_vec();
    let call = move || -> Result<bool, String> {
        let captured = capture;
        let inner = make_box();
        black_box((&captured, inner.len()));
        let value = failure()?;
        black_box(value);
        Ok(true)
    };
    black_box(&outer);
    call()?;
    Ok(true)
}

// An owned error passes through both closure and caller cleanup edges.
fn result_closure() {
    assert!(result_closure_body().is_err());
}

// The temporary Vec in a match scrutinee survives until this arm's early return.
#[inline(never)]
fn match_vec_len<const ANSWER: bool>() -> bool {
    match make_vec().len() {
        4 => return ANSWER,
        _ => !ANSWER,
    }
}

// An early arm return drops the temporary lookup key and the owned map.
#[inline(never)]
fn match_map_get<const ANSWER: bool>() -> bool {
    let map = make_map();
    let key = black_box("alpha");
    match map.get(&key.to_string()) {
        Some(value) => {
            black_box(value);
            return ANSWER;
        }
        None => !ANSWER,
    }
}

// Return from if-let after moving an owning payload out of a temporary Option.
#[inline(never)]
fn if_let_owned<const ANSWER: bool>() -> bool {
    if let Some(values) = make_option() {
        black_box(&values);
        return ANSWER;
    }
    !ANSWER
}

// while-let yields from a temporary owning iterator before returning.
#[inline(never)]
fn while_let_temporary<const ANSWER: bool>() -> bool {
    while let Some(value) = make_vec().into_iter().next() {
        black_box(&value);
        return ANSWER;
    }
    !ANSWER
}

// break transfers one owned value out of a loop and drops its other live locals.
#[inline(never)]
fn loop_break_value<const ANSWER: bool>() -> bool {
    let result = loop {
        let values = make_vec();
        let map = make_map();
        black_box(&map);
        break (ANSWER, values);
    };
    black_box(&result.1);
    return result.0;
}

// A direct return bypasses a loop's owned break value while loop locals are live.
#[inline(never)]
fn loop_return<const ANSWER: bool>() -> bool {
    let result = loop {
        let values = make_vec();
        if black_box(true) {
            return ANSWER;
        }
        break values;
    };
    black_box(&result);
    !ANSWER
}

// A labeled block break carries an owned result across the block's cleanup edge.
#[inline(never)]
fn labeled_break<const ANSWER: bool>() -> bool {
    let outer = make_map();
    let result = 'block: {
        let values = make_vec();
        let scratch = make_box();
        black_box(scratch.len());
        break 'block (ANSWER, values);
    };
    black_box((&outer, &result.1));
    return result.0;
}

// Return directly from inside a labeled block with enclosing and nested owners.
#[inline(never)]
fn labeled_return<const ANSWER: bool>() -> bool {
    let outer = make_map();
    let result = 'block: {
        let values = make_vec();
        black_box((&outer, &values));
        if black_box(true) {
            return ANSWER;
        }
        break 'block values;
    };
    black_box(&result);
    !ANSWER
}

// A failing let-else drops a heap-bearing Err scrutinee and the else block's locals.
#[inline(never)]
fn let_else_return<const ANSWER: bool>() -> bool {
    let values = make_vec();
    let Ok(value) = failure() else {
        let inner = make_box();
        black_box((&values, inner.len()));
        return ANSWER;
    };
    black_box(&value);
    !ANSWER
}

struct Guard {
    field: String,
    rest: Vec<String>,
}
impl Drop for Guard {
    fn drop(&mut self) {
        black_box((&self.field, &self.rest));
    }
}

struct Parts {
    field: String,
    guard: Guard,
}

// Move one field out of a non-Drop wrapper; its remaining Drop field must still drop.
#[inline(never)]
fn partial_move<const ANSWER: bool>() -> bool {
    let parts = Parts {
        field: "moved".to_string(),
        guard: Guard {
            field: "guard".to_string(),
            rest: make_vec(),
        },
    };
    let moved = parts.field;
    black_box((&moved, &parts.guard));
    return ANSWER;
}

// Direct partial moves from Drop types are illegal; take leaves a valid replacement.
#[inline(never)]
fn drop_struct_take<const ANSWER: bool>() -> bool {
    let mut guard = Guard {
        field: "taken".to_string(),
        rest: make_vec(),
    };
    let moved = std::mem::take(&mut guard.field);
    black_box((&moved, &guard));
    return ANSWER;
}

enum Boxed {
    Payload(Box<Guard>),
    Empty,
}

// A reference pattern borrows a boxed payload, leaving ownership with the enum.
#[inline(never)]
fn enum_box_ref<const ANSWER: bool>() -> bool {
    let value = if black_box(true) {
        Boxed::Payload(Box::new(Guard {
            field: "boxed".to_string(),
            rest: make_vec(),
        }))
    } else {
        Boxed::Empty
    };
    match value {
        Boxed::Payload(ref payload) => {
            black_box(payload);
            return ANSWER;
        }
        Boxed::Empty => !ANSWER,
    }
}

enum Ty {
    Leaf(String),
    List(Vec<Ty>),
    Map(Box<BTreeMap<String, Ty>>),
}

impl Ty {
    // Each recursive frame owns scratch allocations when all short-circuits false.
    #[inline(never)]
    fn is_empty(&self) -> bool {
        let scratch = make_vec();
        let names: HashSet<String> = make_vec().into_iter().collect();
        let label = format!("visit {} {}", scratch.len(), names.len());
        black_box((&scratch, &names, &label));
        match self {
            Ty::Leaf(text) => {
                if !text.is_empty() {
                    return false;
                }
            }
            Ty::List(children) => {
                if !children.iter().all(|child| child.is_empty()) {
                    return false;
                }
            }
            Ty::Map(children) => {
                if !children.iter().all(|(_, child)| child.is_empty()) {
                    return false;
                }
            }
        }
        true
    }

    // Explicit recursive loops return false with the same three scratch owners live.
    #[inline(never)]
    fn is_empty_loop(&self) -> bool {
        let scratch = make_vec();
        let names: HashSet<String> = make_vec().into_iter().collect();
        let label = format!("visit {} {}", scratch.len(), names.len());
        black_box((&scratch, &names, &label));
        match self {
            Ty::Leaf(text) => {
                if !text.is_empty() {
                    return false;
                }
            }
            Ty::List(children) => {
                for child in children {
                    if !child.is_empty_loop() {
                        return false;
                    }
                }
            }
            Ty::Map(children) => {
                for (_, child) in children.iter() {
                    if !child.is_empty_loop() {
                        return false;
                    }
                }
            }
        }
        true
    }
}

#[inline(never)]
fn make_tree(empty: bool) -> Ty {
    let mut map = BTreeMap::new();
    map.insert(
        "first".to_string(),
        Ty::List(vec![
            Ty::Leaf(if empty {
                String::new()
            } else {
                "occupied".to_string()
            }),
            Ty::Leaf(String::new()),
        ]),
    );
    map.insert("unvisited".to_string(), Ty::Leaf(String::new()));
    black_box(Ty::List(vec![
        Ty::Map(Box::new(map)),
        Ty::Leaf(String::new()),
    ]))
}

// A false result exits recursive all calls; true visits every child before cleanup.
#[inline(never)]
fn tree_all<const ANSWER: bool>() -> bool {
    let tree = make_tree(ANSWER);
    tree.is_empty()
}

// Compare the same nonempty/empty trees using explicit recursive early-return loops.
#[inline(never)]
fn tree_loop<const ANSWER: bool>() -> bool {
    let tree = make_tree(ANSWER);
    tree.is_empty_loop()
}

macro_rules! cases {
    (paired: [$($paired:ident),* $(,)?], plain: [$($plain:ident),* $(,)?]) => {
        const CASES: &[(&str, fn())] = &[
            $((concat!(stringify!($paired), "_false"), || assert!(!black_box($paired::<false>()))),
              (concat!(stringify!($paired), "_true"), || assert!(black_box($paired::<true>()))),)*
            $((stringify!($plain), $plain),)*
        ];
    };
}

cases! {
    paired: [
        vec_for, vec_into_iter, hashmap_into_iter, btreemap_into_iter, vec_drain, deque_into_iter,
        iter_all, iter_any, iter_find, iter_position, iter_try_fold, iter_take_while,
        iter_zip_left, iter_zip_right, iter_chain, iter_flat_map, iter_flatten, iter_peekable,
        iter_filter_map, iter_skip_while, match_vec_len, match_map_get, if_let_owned,
        while_let_temporary, loop_break_value, loop_return, labeled_break, labeled_return,
        let_else_return, partial_move, drop_struct_take, enum_box_ref, tree_all, tree_loop,
    ],
    plain: [
        option_locals, result_locals, option_temporaries, result_temporaries,
        option_match, result_match, option_closure, result_closure,
    ]
}

fn work() {
    for &(name, case) in CASES {
        case();
        let before = snapshot();
        for _ in 0..N {
            case();
        }
        let after = snapshot();
        println!(
            "D {name} live_bytes_delta={} live_allocs_delta={}",
            after.0 - before.0,
            after.1 - before.1
        );
    }
}

#[test]
fn run() {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    check_allocator();
    for round in 0..4 {
        work();
        if round < 3 && ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}
