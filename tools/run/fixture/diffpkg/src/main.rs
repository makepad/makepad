// diff_fast package programs (tools/run/fixture/diff_fast.sh): `relax-diffpkg <program>` prints `D ...`
// lines; rustc --release and `makepad run` at each tier must print the same lines and exit alike.
// `relax-diffpkg --list` names the programs. Rounds as in the single-file programs: the work runs 4
// times with DIFF_SLEEP_MS between rounds so O3 code installs for the later rounds.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }

#[path = "../../../../../libs/relax/compiler/rwbench/src/box3d_scene.rs"]
mod box3d_scene;

const PROGRAMS: &[(&str, fn())] = &[("box3d", box3d), ("static_tables", static_tables), ("font", font), ("zone_swap", zone_swap), ("inflate_files", inflate_files)];

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if arg == "--list" {
        for (n, _) in PROGRAMS {
            println!("{}", n);
        }
        return;
    }
    let Some((_, f)) = PROGRAMS.iter().find(|(n, _)| *n == arg) else {
        eprintln!("relax-diffpkg: unknown program {:?} (--list)", arg);
        std::process::exit(2);
    };
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 {
        f();
        if r < 3 && ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}

/// The rwbench box3d scene at its small size: stacking, hinges with motor and limits, a rope, a spring
/// pendulum, falling mixed shapes. Per 20 steps the contact count and state hash, then every body.
fn box3d() {
    let mut s = box3d_scene::build(1);
    d!("box3d bodies={} contacts0={}", s.bodies.len(), box3d_scene::contact_count(&s));
    for k in 1..=120 {
        box3d_scene::step(&mut s);
        if k % 20 == 0 {
            d!("box3d step={} contacts={} state={:016x}", k, box3d_scene::contact_count(&s), box3d_scene::state_hash(&s));
        }
    }
    for i in 0..s.bodies.len() {
        d!("box3d body {} {:016x}", i, box3d_scene::body_hash(&s, i));
    }
}

/// Static tables of a dependency crate (relax-diffpkg-tables) read here: every slice's length and
/// contents, the include_bytes! slices' head bytes and sums.
fn static_tables() {
    use relax_diffpkg_tables as t;
    let k = &t::KIT;
    d!("kit {} {:x} mask {:#x} fields [{}]", k.text, k.id & 0xffff, k.mask, k.fields.iter().map(|f| format!("{}/{:?}", f.text, f.kind)).collect::<Vec<_>>().join(","));
    for e in k.events {
        d!("event {} {:x} {}:[{}]", e.text, e.id & 0xffff, e.payload.len(), e.payload.iter().map(|f| format!("{}/{:?}/{:x}", f.text, f.kind, f.id & 0xfff)).collect::<Vec<_>>().join(","));
    }
    d!("direct {} {:?}", t::DIRECT.payload.len(), t::DIRECT.payload.iter().map(|f| f.text).collect::<Vec<_>>());
    d!("nested {:?} bytes {:?}", t::NESTED, t::BYTES);
    let sum = |b: &[u8]| b.iter().map(|&x| x as u64).sum::<u64>();
    d!("inc len {} head {:?} sum {}", t::INC.len(), &t::INC[..8], sum(t::INC));
    let p = t::private_inc();
    d!("font {} {:?}", t::FONT.len(), &t::FONT[..8]);
    d!("private inc len {} head {:?} same {}", p.len(), &p[..8], p == t::INC);
}

/// The UI font (a static include_bytes! slice of a dependency crate) parsed with ttf-parser: its
/// metrics and a few glyphs (Sandbox's vector canvas failed here with UnknownMagic).
fn font() {
    match ttf_parser::Face::parse(relax_diffpkg_tables::FONT, 0) {
        Ok(f) => {
            d!("font glyphs {} units {} asc {} desc {}", f.number_of_glyphs(), f.units_per_em(), f.ascender(), f.descender());
            for c in ['A', 'g', '?'] {
                let g = f.glyph_index(c);
                let b = g.and_then(|g| f.glyph_bounding_box(g)).map(|b| (b.x_min, b.y_min, b.x_max, b.y_max));
                d!("font {:?} glyph {:?} adv {:?} bbox {:?}", c, g, g.and_then(|g| f.glyph_hor_advance(g)), b);
            }
        }
        Err(e) => d!("font parse error {:?}", e),
    }
}

/// include_bytes!/include_str! statics first used after the compiler-zone swap (2026-10-21: the
/// swap's restore had no payload table, so FONT's slice was the 36-byte payload marker and
/// Sandbox's vector font failed with UnknownMagic). Under Relax the program reports its first
/// present (loader symbol `relax_first_present`; the swap waits for it) and waits for the swap
/// (`relax_zone_swaps`) before anything reads the tables; natively both symbols are absent.
fn zone_swap() {
    static WAITED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !WAITED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        wait_for_swap();
    }
    after_swap();
}

#[inline(never)]
fn after_swap() {
    use relax_diffpkg_tables as t;
    let sum = |b: &[u8]| b.iter().map(|&x| x as u64).sum::<u64>();
    d!("zone font {} {:?} sum {}", t::FONT.len(), &t::FONT[..8], sum(t::FONT));
    d!("zone inc {} {:?} sum {}", t::INC.len(), &t::INC[..8], sum(t::INC));
    d!("zone text {} {:?} same {}", t::TEXT.len(), &t::TEXT[..12], t::TEXT.as_bytes() == t::INC);
    d!("zone const {} arr {} same {}", t::CINC.len(), t::ARR.len(), &t::ARR[..] == t::CINC && t::CINC == t::INC);
    match ttf_parser::Face::parse(t::FONT, 0) {
        Ok(f) => d!("zone font glyphs {} units {}", f.number_of_glyphs(), f.units_per_em()),
        Err(e) => d!("zone font parse error {:?}", e),
    }
}

fn host_symbol(name: &std::ffi::CStr) -> *mut std::ffi::c_void {
    extern "C" {
        fn dlsym(handle: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    }
    #[cfg(target_os = "macos")]
    let default = -2isize as *mut std::ffi::c_void;
    #[cfg(not(target_os = "macos"))]
    let default = std::ptr::null_mut();
    unsafe { dlsym(default, name.as_ptr()) }
}

fn wait_for_swap() {
    let present = host_symbol(c"relax_first_present");
    if present.is_null() {
        return;
    }
    unsafe { std::mem::transmute::<*mut std::ffi::c_void, extern "C" fn()>(present)() };
    let swaps = host_symbol(c"relax_zone_swaps");
    if swaps.is_null() {
        d!("zone: the loader has no relax_zone_swaps");
        return;
    }
    let swaps = unsafe { std::mem::transmute::<*mut std::ffi::c_void, extern "C" fn() -> u32>(swaps) };
    for _ in 0..1800 {
        if swaps() > 0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    d!("zone: no swap within 180 s");
}

/// zlib streams of real PNG files (their IDAT chunks) through makepad-fast-inflate with the size hint
/// the asset importer passes: per file the result length and a hash of the bytes. The files: every
/// .png under DIFF_INFLATE_DIR (default the repo's apps/), sorted, at most DIFF_INFLATE_MAX (64).
/// (PGO3 10-22: the Sandbox race's texture decode faulted in deflate_decompress_with's counted code.)
fn inflate_files() {
    let root = std::env::var("DIFF_INFLATE_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../apps").to_string());
    let max: usize = std::env::var("DIFF_INFLATE_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(64);
    let mut files = Vec::new();
    let mut todo = vec![std::path::PathBuf::from(&root)];
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                todo.push(p);
            } else if p.extension().is_some_and(|x| x == "png") {
                files.push(p);
            }
        }
    }
    files.sort();
    files.truncate(max);
    for f in &files {
        let Ok(b) = std::fs::read(f) else { continue };
        if b.len() < 33 || &b[1..4] != b"PNG" {
            continue;
        }
        let be = |o: usize| u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize;
        let (w, h, depth, color, interlace) = (be(16), be(20), b[24] as usize, b[25], b[28]);
        let ch = match color { 0 | 3 => 1, 2 => 3, 4 => 2, 6 => 4, _ => continue };
        let mut z = Vec::new();
        let mut o = 8;
        while o + 12 <= b.len() {
            let n = be(o);
            if o + 12 + n > b.len() {
                break;
            }
            if &b[o + 4..o + 8] == b"IDAT" {
                z.extend_from_slice(&b[o + 8..o + 8 + n]);
            }
            o += 12 + n;
        }
        let expect = if interlace == 0 { ((w * ch * depth + 7) / 8 + 1) * h } else { 0 };
        let name = f.strip_prefix(&root).unwrap_or(f).display().to_string();
        match makepad_fast_inflate::zlib_decompress_vec_with_hint(&z, expect) {
            Ok(v) => {
                let mut x = 0xcbf29ce484222325u64;
                for &c in &v {
                    x = (x ^ c as u64).wrapping_mul(0x100000001b3);
                }
                d!("inflate {} in={} out={} {:016x}", name, z.len(), v.len(), x);
            }
            Err(e) => d!("inflate {} in={} error {:?}", name, z.len(), e),
        }
    }
}
