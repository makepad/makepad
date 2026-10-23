// Escaped module code (MEMRECLAIM.REVIEW F3): a module must not hand its own code to the host as
// a value (a fn pointer, a `dyn` vtable, a closure) through a host API call. If the host kept it
// and called it later -- from another thread, outside any wrapper -- twin code would run with no
// arena entered and no reclaim record: masked accesses relative to arena 0 (host memory) and
// frames the code reclaimer cannot see. Such calls are refused when the module is loaded.
// Run as the repro with SAFE2_ESCAPE_CALL=1: it calls the escaped pointer from a host thread.
use std::sync::atomic::{AtomicUsize, Ordering};

pub static KEPT: AtomicUsize = AtomicUsize::new(0);

pub struct Hooks { pub on_tick: fn(u64) -> u64, pub n: u64 }
pub struct Plain { pub a: u64, pub b: [u32; 4] }

/// Host API: keeps a callback (the shape that leaks module code).
pub fn host_register(cb: fn(u64) -> u64) -> u64 {
    KEPT.store(cb as usize, Ordering::SeqCst);
    1
}
pub fn host_register_dyn(cb: &dyn Fn(u64) -> u64) -> u64 { cb(1) }
pub fn host_register_box(cb: Box<dyn FnMut()>) -> u64 { drop(cb); 1 }
pub fn host_register_hooks(h: &Hooks) -> u64 { h.n }
pub fn host_register_opt(h: Option<fn() -> u64>) -> u64 { h.is_some() as u64 }
/// Host API taking plain data only: allowed.
pub fn host_plain(p: &Plain, s: &[u64], t: &str) -> u64 { p.a + p.b[0] as u64 + s.len() as u64 + t.len() as u64 }

fn load(name: &str, src: &str) -> Result<std::relax::Module, String> {
    std::relax::Module::load_source(name, &format!("{}.rs", name), src, &["test_crate"]).map_err(|e| format!("{:?}", e))
}

#[test]
fn escape_fn() {
    let cases: [(&str, &str); 5] = [
        ("esc_fnptr", "static S: [u64; 4] = [5, 6, 7, 8];\npub fn cb(x: u64) -> u64 { S[(x & 3) as usize] + x }\npub fn reg() -> u64 { test_crate::host_register(cb) }\n"),
        ("esc_dyn", "pub fn reg() -> u64 { let k = 3u64; test_crate::host_register_dyn(&move |x| x + k) }\n"),
        ("esc_box", "pub fn reg() -> u64 { let mut n = 0u64; test_crate::host_register_box(Box::new(move || n += 1)) }\n"),
        ("esc_hooks", "fn t(x: u64) -> u64 { x }\npub fn reg() -> u64 { test_crate::host_register_hooks(&test_crate::Hooks { on_tick: t, n: 2 }) }\n"),
        ("esc_opt", "fn t() -> u64 { 1 }\npub fn reg() -> u64 { test_crate::host_register_opt(Some(t)) }\n"),
    ];
    if std::env::var("SAFE2_ESCAPE_CALL").is_ok_and(|v| v == "1") {
        // the repro: before the fix this loads, and the host calls module code from a bare thread
        let m = match load(cases[0].0, cases[0].1) {
            Ok(m) => m,
            Err(e) => {
                println!("out escaping fn value refused at load: {}", e.contains("host API"));
                return;
            }
        };
        let reg: fn() -> u64 = m.get("reg").unwrap();
        reg();
        let p = KEPT.load(Ordering::SeqCst);
        let r = std::thread::spawn(move || {
            let f: fn(u64) -> u64 = unsafe { std::mem::transmute(p) };
            f(1)
        }).join();
        println!("out escaped call from a host thread: {:?}", r.is_ok());
        return;
    }
    let mut refused = 0;
    for (name, src) in cases.iter() {
        match load(name, src) {
            Err(e) if e.contains("host API") => refused += 1,
            Err(e) => eprintln!("escape_fn: {} refused for another reason: {}", name, e),
            Ok(m) => {
                eprintln!("escape_fn: {} loaded (module code can escape to the host)", name);
                m.unload();
            }
        }
    }
    println!("out escaping fn values refused: {}", refused == cases.len());
    let plain = load("esc_plain", "pub fn go() -> u64 { let p = test_crate::Plain { a: 1, b: [2; 4] }; test_crate::host_plain(&p, &[1, 2, 3], \"ab\") }\n");
    let ok = match plain {
        Ok(m) => {
            let go: fn() -> u64 = m.get("go").unwrap();
            let v = go();
            m.unload();
            v == 8
        }
        Err(e) => {
            eprintln!("escape_fn: plain data refused: {}", e);
            false
        }
    };
    println!("out plain data to host API: {}", ok);
}
