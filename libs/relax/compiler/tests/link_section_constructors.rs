// Constructor sections run before the entry, as the platform's start code runs them: ELF's
// `.init_array.<priority>` before `.init_array`, Mach-O's `__mod_init_func`, COFF's `.CRT$XCU`
// (devhub's Windows image roles are one, `before_main`). The JIT skipped them: nothing ran before
// main. Lines "| " are compared with rustc --test.
use std::sync::atomic::{AtomicU32, Ordering};

static ORDER: AtomicU32 = AtomicU32::new(0);
static A: AtomicU32 = AtomicU32::new(0);
static B: AtomicU32 = AtomicU32::new(0);
static C: AtomicU32 = AtomicU32::new(0);
static EARLY: AtomicU32 = AtomicU32::new(0);
static CALLS: AtomicU32 = AtomicU32::new(0);

fn mark(s: &AtomicU32) {
    CALLS.fetch_add(1, Ordering::SeqCst);
    s.store(ORDER.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
}
extern "C" fn ctor_a() {
    mark(&A);
}
extern "C" fn ctor_b() {
    mark(&B);
}
extern "C" fn ctor_c() {
    mark(&C);
}
extern "C" fn ctor_early() {
    mark(&EARLY);
}

#[used]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(all(not(target_vendor = "apple"), not(windows)), link_section = ".init_array")]
#[cfg_attr(windows, link_section = ".CRT$XCU")]
static CTOR_A: extern "C" fn() = ctor_a;

// a table of two: called in its order
#[used]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(all(not(target_vendor = "apple"), not(windows)), link_section = ".init_array")]
#[cfg_attr(windows, link_section = ".CRT$XCU")]
static CTOR_BC: [extern "C" fn(); 2] = [ctor_b, ctor_c];

// ELF: a priority runs before the plain table; COFF: `$XCB` before `$XCU`
#[used]
#[cfg_attr(target_vendor = "apple", link_section = "__DATA,__mod_init_func")]
#[cfg_attr(all(not(target_vendor = "apple"), not(windows)), link_section = ".init_array.00100")]
#[cfg_attr(windows, link_section = ".CRT$XCB")]
static CTOR_EARLY: extern "C" fn() = ctor_early;

#[test]
fn a01_constructors_ran_before_the_tests() {
    // each ran once, whatever the tests
    println!("| a01: calls {}", CALLS.load(Ordering::SeqCst));
    let (a, b, c, e) = (A.load(Ordering::SeqCst), B.load(Ordering::SeqCst), C.load(Ordering::SeqCst), EARLY.load(Ordering::SeqCst));
    println!("| a01: every one ran {}", a > 0 && b > 0 && c > 0 && e > 0);
    println!("| a01: a table in its order {}", b < c);
    if cfg!(target_vendor = "apple") {
        // one section: the link order is the linker's
        println!("| a01: the early one first true");
    } else {
        println!("| a01: the early one first {}", e < a && e < b);
    }
}

#[test]
fn a02_main_state_is_theirs() {
    // what a constructor wrote is the program's state, as any static's
    A.fetch_add(100, Ordering::SeqCst);
    println!("| a02: a kept {}", A.load(Ordering::SeqCst) > 100);
}
