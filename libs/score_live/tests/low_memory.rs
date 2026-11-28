use makepad_alloc_guard::{armed, low_memory_epoch, Guarded};
use makepad_score_live::vm::{evaluate_begin, evaluate_controlled, EvalStep};
use makepad_score_live::{LoadError, PrepareControl};
use makepad_script::ScriptRunBudget;
use std::alloc::{alloc, dealloc, GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::ptr::null_mut;
use std::time::Duration;

thread_local! { static REFUSE: Cell<u32> = const { Cell::new(0) }; }

struct Gate;

fn refused() -> bool {
    REFUSE.try_with(|count| {
        let left = count.get();
        count.set(left.saturating_sub(1));
        left != 0
    }).unwrap_or(false)
}

// SAFETY: a refusal leaves the old allocation intact. All other operations go to System.
unsafe impl GlobalAlloc for Gate {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refused() { null_mut() } else { unsafe { System.alloc(layout) } }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if refused() { null_mut() } else { unsafe { System.alloc_zeroed(layout) } }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if refused() { null_mut() } else { unsafe { System.realloc(ptr, layout, size) } }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Guarded<Gate> = Guarded(Gate);

fn spend_reserve() {
    assert!(armed(), "a reserve must be held before the test allocation");
    let before = low_memory_epoch();
    let layout = Layout::new::<usize>();
    let allocate = black_box(alloc as unsafe fn(Layout) -> *mut u8);
    REFUSE.set(1);
    let block = unsafe { allocate(layout) };
    assert!(!block.is_null(), "the test allocation must succeed on retry");
    unsafe { dealloc(block, layout) };
    assert!(!armed(), "the test allocation must release the reserve");
    assert_eq!(low_memory_epoch(), before.wrapping_add(1), "the refusal must advance the epoch once");
}

#[test]
fn the_memory_watch_lasts_for_one_document_across_its_slices() {
    const GOOD: &str = "use mod.score.*\nlet pulse = Loop{beats: 4 code: fn(i) { return i + 1 }}\nScore{pulse}";
    const LONG: &str = "use mod.score.*\nlet n = 0\nfor i in 0..2048 { n += i }\nScore{}";
    let control = PrepareControl::default();
    let good = evaluate_controlled(GOOD, &control).expect("a short document must evaluate");
    assert!(good.errors.is_empty(), "the good score must have no diagnostics");
    assert!(armed(), "document evaluation must arm the reserve");
    drop(good);

    let EvalStep::Pending(pending) = evaluate_begin(LONG, &control, Duration::ZERO) else {
        panic!("the finite loop must suspend before its document finishes");
    };
    spend_reserve();
    assert!(matches!(pending.step(Duration::ZERO), EvalStep::Done(Err(LoadError::Memory { line: Some(3) }))),
        "the resumed VM must report the sampled loop's line on its first slice after a refusal");
    assert!(!armed(), "resuming a spent evaluation must leave the reserve released");

    let mut next = evaluate_controlled(GOOD, &control).expect("a fresh document must recover");
    assert!(armed(), "the fresh evaluation must re-arm the reserve");
    spend_reserve();
    // Sample every instruction in a cycle on the completed document's own VM.
    next.with_vm(|vm| { vm.set_run_budget(Some(ScriptRunBudget::slice(Duration::from_secs(60), 1))); });
    let answer = next.call(0, &[41.0]).expect("the old document's epoch must leave a later cycle alone");
    assert_eq!(answer.as_number(), Some(42.0), "the budgeted cycle must return its script result");

    REFUSE.set(2);
    assert!(matches!(evaluate_controlled(GOOD, &control), Err(LoadError::Memory { line: None })),
        "a refused reserve and retry must end evaluation before there is a document line");
    assert_eq!(REFUSE.get(), 0, "arming must have consumed both refusals");
    assert!(!armed(), "a refused reserve must leave the allocator disarmed");
    let recovered = evaluate_controlled(GOOD, &control).expect("evaluation must recover after arming failed");
    assert!(recovered.errors.is_empty() && armed(), "recovery must produce a clean score with a reserve");
}
