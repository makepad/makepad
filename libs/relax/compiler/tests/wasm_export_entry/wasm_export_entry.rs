// A bin whose static the compiler initializes before `main` runs (its const fn compiles first):
// the wasm export of a run of it (RELAX_WASM_EXPORT) must export `main` as `t0`
// (tools/run/fixture/modules_harness.rs, a_bins_wasm_export_exports_its_main).
static TABLE: [u32; 4] = make(3);

const fn make(n: u32) -> [u32; 4] {
    let mut t = [0; 4];
    let mut i = 0;
    while i < 4 {
        t[i] = n + i as u32;
        i += 1;
    }
    t
}

#[cfg(target_family = "wasm")]
extern "C" {
    fn js_console_log(ptr: *const u8, len: usize);
}

fn main() {
    let s: u32 = TABLE.iter().sum();
    let t = format!("sum {s}");
    #[cfg(target_family = "wasm")]
    unsafe {
        js_console_log(t.as_ptr(), t.len())
    }
    #[cfg(not(target_family = "wasm"))]
    println!("{t}");
}
