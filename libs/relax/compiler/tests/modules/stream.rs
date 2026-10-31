use std::relax::{Edit, Module, Stream};

fn pause() { std::thread::sleep(std::time::Duration::from_millis(20)); }
fn value(m: Module) -> i32 { m.get::<fn() -> i32>("value").unwrap()() }
fn label(m: Module) -> usize { m.get::<fn() -> usize>("label").unwrap()() }

extern "C" {
    fn __relax_module_load_stream(n: *const u8, nl: usize, p: *const u8, pl: usize, s: *const u8, sl: usize, d: *const u8, dl: usize) -> u64;
    fn __relax_module_stream(h: u64, s: *const u8, sl: usize, kept: *mut usize) -> u32;
    fn __relax_module_get(h: u64, n: *const u8, nl: usize, t: *const u8, tl: usize) -> u64;
    fn __relax_module_unload(h: u64);
    fn __relax_module_error(buf: *mut u8, cap: usize) -> usize;
}

fn byte_label(h: u64) -> usize {
    let ty = std::any::type_name::<fn() -> usize>();
    let addr = unsafe { __relax_module_get(h, "label".as_ptr(), 5, ty.as_ptr(), ty.len()) } as usize;
    assert!(addr != 0);
    let f: fn() -> usize = unsafe { std::mem::transmute_copy(&addr) };
    f()
}

#[test]
fn streaming() {
    let old = "pub fn value() -> i32 { 1 }\npub fn label() -> usize { 3 }\n";
    let m = Module::load_source("streamed", "memory/stream.rs", old, &[]).unwrap();
    let f = m.get::<fn() -> i32>("value").unwrap();
    let text = "pub fn value() -> i32 { 2 }\npub fn label() -> usize { \"é🙂 new\".len() }\n";
    // Mid-identifier, return type, body, between items, and mid-string. &str cannot
    // contain a partial UTF-8 character: a byte-stream caller must buffer it first.
    let cuts = [9, 24, 28, 34, 57, 61, text.len()];
    let mut applied = 0;
    for (i, n) in cuts.iter().enumerate() {
        pause();
        assert!(text.is_char_boundary(*n));
        let r = m.stream(&text[..*n]).unwrap();
        if let Stream::Applied { kept } = r {
            applied += 1;
            assert_eq!(kept, if i == 2 { 1 } else { 0 });
        }
        assert_eq!(value(m), if i < 2 { 1 } else { 2 });
        assert_eq!(f(), value(m));
        assert_eq!(label(m), if i == cuts.len() - 1 { 10 } else { 3 });
    }
    assert_eq!(applied, 2);
    assert_eq!(m.finish_stream(text).unwrap(), Edit::Patched);
    // The extern accepts bytes: cut each multibyte character before its final byte.
    let h = unsafe { __relax_module_load_stream("byte_stream".as_ptr(), 11, "bytes.rs".as_ptr(), 8, old.as_ptr(), old.len(), "".as_ptr(), 0) };
    if h == 0 {
        let mut error = [0u8; 4096];
        let n = unsafe { __relax_module_error(error.as_mut_ptr(), error.len()) };
        panic!("byte stream load: {}", String::from_utf8_lossy(&error[..n.min(error.len())]));
    }
    for end in [56, 58, 59, 60] {
        pause();
        let mut kept = 0;
        let status = unsafe { __relax_module_stream(h, text.as_ptr(), end, &mut kept) };
        assert!(status <= 1);
        assert_eq!(byte_label(h), 3);
    }
    pause();
    let mut kept = 0;
    assert_eq!(unsafe { __relax_module_stream(h, text.as_ptr(), text.len(), &mut kept) }, 0);
    assert_eq!(byte_label(h), 10);
    unsafe { __relax_module_unload(h) };
    println!("out awkward chunks and finish: true");

    // A complete item that fails checking must not prevent an independent item applying.
    pause();
    let bad = "pub fn value() -> i32 { 9 }\npub fn label() -> usize { missing() }\n";
    assert_eq!(m.stream(bad).unwrap(), Stream::Applied { kept: 1 });
    assert_eq!(value(m), 9);
    assert_eq!(label(m), 10);
    assert!(m.finish_stream(bad).unwrap_err().contains("memory/stream.rs:2:"));
    assert_eq!(value(m), 9);
    assert!(m.finish_stream("pub fn value() -> i32 { 77 }\npub fn lab").is_err());
    assert_eq!(value(m), 9);
    println!("out independent items and final errors: true");

    // The newest prefix wins; finish discards a queued prefix even on error.
    pause();
    m.stream(text).unwrap();
    assert_eq!(m.stream(bad).unwrap(), Stream::Pending);
    assert_eq!(value(m), 2);
    assert!(m.finish_stream("pub fn value() -> i32 { nope }\npub fn label() -> usize { 3 }\n").is_err());
    pause();
    assert_eq!(value(m), 2);
    assert_eq!(m.stream(old).unwrap(), Stream::Applied { kept: 0 });
    assert_eq!(value(m), 1);
    pause();
    assert_eq!(m.stream("pub fn value() -> i64 { 7 }\n").unwrap(), Stream::NeedsReload);
    assert_eq!(value(m), 1);
    println!("out coalescing and needs reload: true");

    // Empty and unfinished initial prefixes, new exports, then another body edit.
    let fresh = Module::load_stream("fresh_stream", "memory/fresh.rs", "", &[]).unwrap();
    assert!(fresh.get::<fn() -> i32>("value").is_err());
    assert_eq!(fresh.stream("pub fn val").unwrap(), Stream::Incomplete);
    pause();
    assert_eq!(fresh.stream(old).unwrap(), Stream::Applied { kept: 0 });
    assert_eq!(value(fresh), 1);
    assert_eq!(label(fresh), 3);
    pause();
    assert_eq!(fresh.stream(text).unwrap(), Stream::Applied { kept: 0 });
    assert_eq!(value(fresh), 2);
    assert_eq!(fresh.finish_stream(text).unwrap(), Edit::Patched);
    let prefix = Module::load_stream("prefix_stream", "memory/prefix.rs", "pub fn value() -> i32 { 4 }\npub fn lab", &[]).unwrap();
    assert_eq!(value(prefix), 4);
    let rr = Module::load_stream("rr_stream", "memory/stream.rx", "pub fn val", &[]).unwrap();
    pause();
    assert_eq!(rr.stream(old).unwrap(), Stream::Applied { kept: 0 });
    assert_eq!(value(rr), 1);
    pause();
    rr.stream(&text[..28]).unwrap();
    assert_eq!(value(rr), 1);
    assert_eq!(label(rr), 3);
    assert_eq!(rr.finish_stream(text).unwrap(), Edit::Patched);
    assert_eq!(value(rr), 2);
    assert_eq!(label(rr), 10);
    rr.unload();
    println!("out new modules from prefixes: true");

    // Identical display paths are not module identity.
    let other = Module::load_stream("other_stream", "memory/stream.rs", old, &[]).unwrap();
    pause();
    m.stream(text).unwrap();
    assert_eq!(value(m), 2);
    assert_eq!(value(other), 1);
    // Streaming cannot introduce sandbox escapes or grant an unlisted host dependency.
    pause();
    let escape = "pub fn value() -> i32 { unsafe { 88 } }\npub fn label() -> usize { 3 }\n";
    m.stream(escape).unwrap();
    assert_eq!(value(m), 2);
    assert!(m.finish_stream(escape).unwrap_err().contains("sandbox"));
    let orphan = format!("{}impl core::fmt::Display for Vec<u8> {{ fn fmt(&self, _f: &mut core::fmt::Formatter) -> core::fmt::Result {{ Ok(()) }} }}\n", text);
    pause();
    m.stream(&orphan).unwrap();
    assert_eq!(value(m), 2);
    assert!(m.finish_stream(&orphan).unwrap_err().contains("orphan rule"));
    let bad_added = format!("{}pub fn bad() {{ unsafe {{ }} }}\n", text);
    pause();
    m.stream(&bad_added).unwrap();
    assert!(m.finish_stream(&bad_added).unwrap_err().contains("sandbox"));
    pause();
    assert_eq!(m.stream(old).unwrap(), Stream::Applied { kept: 0 });
    assert_eq!(value(m), 1);
    assert_eq!(m.finish_stream(text).unwrap(), Edit::Patched);
    let denied = "pub fn value() -> i32 { test_crate::host_value() }\npub fn label() -> usize { 3 }\n";
    assert!(m.finish_stream(denied).is_err());
    assert_eq!(value(m), 2);
    let allowed = Module::load_stream("allowed_stream", "memory/allowed.rs", denied, &["test_crate"]).unwrap();
    assert_eq!(value(allowed), 42);
    m.unload();
    assert!(m.stream(text).unwrap_err().contains("unloaded"));
    assert!(m.finish_stream(text).is_err());
    fresh.unload(); prefix.unload(); other.unload(); allowed.unload();
    println!("out handle identity sandbox and deps: true");
}

pub fn host_value() -> i32 { 42 }

#[test]
fn cost() {
    // 100 three-line functions: measure feeds separately from the 20 ms pacing.
    let mut old = String::new();
    let mut new = String::new();
    for i in 0..100 {
        old.push_str(&format!("pub fn f{}() -> i32 {{\n    1\n}}\n", i));
        new.push_str(&format!("pub fn f{}() -> i32 {{\n    2\n}}\n", i));
    }
    let m = Module::load_source("stream_cost", "memory/cost.rs", &old, &[]).unwrap();
    let mut times: Vec<f64> = Vec::new();
    let mut patches = 0;
    for end in (17..new.len()).step_by(17) {
        pause();
        let t = std::time::Instant::now();
        let r = m.stream(&new[..end]).unwrap();
        times.push(t.elapsed().as_secs_f64() * 1e3);
        if let Stream::Applied { .. } = r { patches += 1; }
    }
    assert_eq!(m.finish_stream(&new).unwrap(), Edit::Patched);
    assert_eq!(m.get::<fn() -> i32>("f99").unwrap()(), 2);
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    eprintln!("stream cost: 300 lines, {} chunks (17 bytes), {} patches; mean {:.3} ms, median {:.3} ms, p95 {:.3} ms, max {:.3} ms", times.len(), patches, mean, times[times.len()/2], times[times.len()*95/100], times[times.len()-1]);
    m.unload();
    println!("out stream cost final value: true");
}
