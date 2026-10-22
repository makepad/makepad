//! The text answers rendered from recorded JSON answers (relax-inspect-eval, 2026-10-22).

use super::*;

/// `cmd word k=v` (the inspect client parses it with makepad-remote's `Req`).
fn render(line: &str, lines: &[String]) -> String {
    let mut q = crate::ask::Q::parse(line);
    let cmd = if q.pos.is_empty() { String::new() } else { q.pos.remove(0) };
    super::render(&Request { cmd, pos: q.pos, kv: q.kv }, lines)
}

fn lines(s: &str) -> Vec<String> {
    s.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()
}

#[test]
fn numbers_keep_two_or_three_significant_digits() {
    assert_eq!(sig(1234.4), "1234");
    assert_eq!(sig(12.34), "12.3");
    assert_eq!(sig(3.14159), "3.14");
    assert_eq!(sig(0.3141), "0.31");
    assert_eq!(sig(0.0042), "0.0042");
    assert_eq!(sig(20.0), "20");
    assert_eq!(ms(0.05), "50 µs");
    assert_eq!(mb(1709.0), "1.67 GB");
    assert_eq!(mb(0.5), "512 KB");
}

#[test]
fn fn_names_keep_their_last_two_segments() {
    assert_eq!(short_fn("core::slice::iter::<Iter as Iterator>::next"), "<Iter as Iterator>::next");
    assert_eq!(short_fn("relax_inspect_eval::score_items"), "relax_inspect_eval::score_items");
    assert_eq!(short_fn("std::sys::time::sleep"), "time::sleep");
    assert_eq!(short_fn("{closure} in relax_inspect_eval::main"), "{closure} in relax_inspect_eval::main");
    assert_eq!(short_fn("core::vec::Vec::with_capacity<u8>"), "Vec::with_capacity<u8>");
}

#[test]
fn paths_are_relative_to_the_answers_root_and_std_is_short() {
    let r = render(
        "versions S35",
        &lines(r#"{"measured":"code versions","at_ms":1,"source":"x","partial":[]}
{"slot":35,"fn":"relax_inspect_eval::score_items","kind":"fn","compiled":true,"version":3}
{"version":{"slot":35,"fn":"relax_inspect_eval::score_items","tier":"O3","origin":"fresh","code":"0x1+0x94","state":"installed","inlined":["core::iter::range::<Range as Iterator>::next"],"threads":["main (T1)"]}}
{"version":{"slot":35,"fn":"relax_inspect_eval::score_items","tier":"baseline","origin":"fresh","code":"0x2+0x1a8","state":"replaced","threads":[]}}
{"version":{"slot":35,"fn":"relax_inspect_eval::score_items","tier":"O1","origin":"fresh","code":"0x3+0x184","state":"retired","threads":[]}}
{"inliners":0,"more":["tier x o1"]}"#),
    );
    let first = r.lines().next().unwrap();
    assert!(first.starts_with("relax_inspect_eval::score_items (S35) runs O3 fresh code on main (T1); 3 code versions: O3 installed, baseline replaced, O1 retired; inlined into its installed code: <Range as Iterator>::next"), "{}", r);
    assert!(r.contains("more: count S35"), "{}", r);
}

#[test]
fn why_hung_leads_with_the_deadlock_and_keeps_the_parked_panic() {
    let r = render(
        "why-hung",
        &lines(r#"{"measured":"24 threads","at_ms":1,"source":"x","partial":[]}
{"verdict":"deadlock","why":"wait cycle: ledger waits: mutex 0x480 held by audit (T21) -> audit waits: mutex 0x470 held by ledger (T20)","findings":[{"kind":"deadlock","why":"wait cycle: ledger waits: mutex 0x480 held by audit (T21) -> audit waits: mutex 0x470 held by ledger (T20)"},{"kind":"parked panic","why":"P1 parked on thread importer (T22): index out of bounds"},{"kind":"idle","why":"main is not stuck: 28% CPU"}],"owner":{"serving":"idle","queued":{"total":0}}}
{"thread":"ledger","id":"T20","state":"waiting","cpu_pct":0,"waits":"mutex 0x480 held by audit (T21) for 45 s","frames":[{"fn":"__ulock_wait2","in":"libsystem_kernel.dylib"},{"fn":"std::sync::raw::RawMutex::lock","at":"/w/libs/relax/compiler/std/std/sync/raw.rs:89:18","in":"jit/baseline fresh","code":{"slot":832,"tier":"baseline","origin":"fresh","state":"installed"}},{"fn":"relax_inspect_eval::post_ledger","at":"/w/tools/run/fixture/src/inspect_eval.rs:52:21","in":"jit/baseline fresh","code":{"slot":30,"tier":"baseline","origin":"fresh","state":"installed"}},{"fn":"relax_compiler::jit::call_jit_in","in":"makepad"}]}
{"other_threads":["run-owner T9 waiting 0%: __ulock_wait2","run-pool-0 T3 waiting 0%: __ulock_wait2","run-live T12 waiting 0%: __semwait_signal"],"more":"stacks thread=<id>"}"#),
    );
    assert!(r.starts_with("Deadlock: wait cycle: ledger waits"), "{}", r);
    assert!(r.contains("- parked panic: P1 parked on thread importer"), "{}", r);
    assert!(r.contains("relax_inspect_eval::post_ledger @ tools/run/fixture/src/inspect_eval.rs:52 [baseline S30]"), "{}", r);
    assert!(r.contains("other threads (3): 2 futex wait, 1 sleeping, none busy"), "{}", r);
    assert!(r.contains("paths: under /w/"), "{}", r);
}

#[test]
fn heap_sites_merge_their_stacks_and_say_growth_or_churn() {
    let r = render(
        "heap top by=alloc",
        &lines(r#"{"measured":"program heap by allocation stack","at_ms":1,"source":"heap hooks","partial":[]}
{"id":"H2","live_mb":0.00,"live_samples":0,"alloc_mb":9.38,"alloc_mb_per_s":0.04,"freed_mb":9.38,"site":"relax_inspect_eval::render_label","site_at":"/w/tools/run/fixture/src/inspect_eval.rs:35:17","via":"core::fmt::rt::Argument::fmt","frames":[]}
{"id":"H3","live_mb":0.00,"live_samples":0,"alloc_mb":5.62,"alloc_mb_per_s":0.02,"freed_mb":5.62,"site":"relax_inspect_eval::render_label","site_at":"/w/tools/run/fixture/src/inspect_eval.rs:35:17","frames":[]}
{"id":"H1","live_mb":27.31,"live_samples":420,"alloc_mb":27.31,"alloc_mb_per_s":0.13,"freed_mb":0.00,"site":"relax_inspect_eval::remember_batch","site_at":"/w/tools/run/fixture/src/inspect_eval.rs:43:17","frames":[]}
{"sites":3,"total_live_mb":27.31,"total_alloc_mb":42.31,"window_s":211.2,"not_shown":0,"not_shown_live_mb":0.00,"not_shown_alloc_mb":0.00}"#),
    );
    assert!(r.starts_with("Most allocated: relax_inspect_eval::render_label @ inspect_eval.rs:35 (H2,H3) 15 MB allocated in 211 s, 0 MB live -- churn: freed again."), "{}", r);
    assert!(r.contains("| relax_inspect_eval::remember_batch @ inspect_eval.rs:43 | H1 | 27.3 | 27.3 | 0 | grows ~0.13 MB/s, never freed |"), "{}", r);
}

#[test]
fn an_error_answer_is_one_line() {
    let r = render("locals panic=P9", &lines(r#"{"err":"no parked panic P9 (parked: P1); try: panics"}"#));
    assert_eq!(r, "error: no parked panic P9 (parked: P1); try: panics\n");
}

#[test]
fn unknown_commands_fall_back_to_tables() {
    let r = render(
        "hal pump",
        &lines(r#"{"measured":"HAL pump","at_ms":1,"source":"x","partial":[]}
{"session":1,"turns":12,"wake_ms":3.5}
{"session":2,"turns":4,"wake_ms":0.25}"#),
    );
    assert!(r.starts_with("HAL pump\n| session | turns | wake ms |\n|---|---|---|\n| 1 | 12 | 3.5 |\n| 2 | 4 | 0.25 |"), "{}", r);
}

#[test]
fn thread_pools_fold_into_one_row() {
    let mut l = String::from("{\"measured\":\"threads\",\"at_ms\":1,\"source\":\"os\",\"partial\":[]}\n");
    l.push_str("{\"id\":\"T1\",\"name\":\"main\",\"role\":\"main\",\"state\":\"running\",\"cpu_ms\":900}\n");
    for i in 0..4 {
        l.push_str(&format!("{{\"id\":\"T{}\",\"name\":\"makepad-pool-{}\",\"role\":\"program\",\"state\":\"waiting\",\"cpu_ms\":10}}\n", i + 2, i));
    }
    l.push_str("{\"id\":\"T9\",\"name\":\"run-owner\",\"role\":\"owner\",\"state\":\"waiting\",\"cpu_ms\":5}\n");
    let r = render("threads", &lines(&l));
    assert!(r.contains("| T2..T5 | makepad-pool* (4 threads) | program | 0 running | 0.04 |"), "{}", r);
    assert!(r.starts_with("6 threads: 5 of the program, 1 of makepad run."), "{}", r);
}
