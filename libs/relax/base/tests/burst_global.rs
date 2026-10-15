//! Exercise std's allocator entrypoints and memstat together. Also run this test binary with
//! MAKEPAD_RUN_BURST_ARENA=0 and MAKEPAD_RUN_MEM_REPORT=1 to cover the process-wide environment knobs.
use relax_base::{memstat, zone_alloc};

#[global_allocator]
static ALLOC: memstat::Counting = memstat::Counting;

#[test]
fn std_allocations_escape_and_free_on_another_thread() {
    let runtime = relax_base::options::RuntimeOptions::from_env();
    let before = zone_alloc::burst_stats();
    let mut heap = vec![37_u8; 8192];
    let (escaped, committed) = {
        let _scope = zone_alloc::scratch(&runtime);
        heap.reserve_exact(1 << 20); // heap -> arena
        let mut values = vec![0_u64; 4096];
        for (i, v) in values.iter_mut().enumerate() {
            *v = i as u64;
        }
        (values, zone_alloc::burst_stats().0)
    };
    let disabled = !runtime.burst_arena
        || !cfg!(any(target_os = "macos", target_os = "linux", windows));
    if disabled {
        assert_eq!(committed, 0);
    } else {
        assert!(committed > before.0);
    }
    std::thread::spawn(move || {
        let mut escaped = escaped;
        escaped.reserve_exact(1 << 20); // arena -> heap outside the scope
        for (i, v) in escaped.iter().enumerate() {
            assert_eq!(*v, i as u64);
        }
        assert!(heap.iter().all(|&b| b == 37));
    })
    .join()
    .unwrap();
    zone_alloc::burst_trim(); // emptied chunks wait warm until a trim
    assert_eq!(zone_alloc::burst_stats(), before);
    assert!(memstat::report("test").contains("burst arena: committed 0 MB, live 0 MB, chunks 0"));
}
