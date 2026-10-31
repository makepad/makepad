//! Formula tests: the compiled programs (interpreter and native) against
//! the reference tree-walker, on focused scripts and on every script of
//! the patch library (patch content, not in the repo: the folder named by
//! `FORMULA_LIBRARY`, default `local/agent_state/synth/formula/lib`).

use super::compile::{f, BOOL_FIELDS, NUM_FIELDS};
use super::reference::RefRunner;
use super::*;

fn inputs(block: usize, voice: Option<usize>) -> FormulaInputs {
    let t = block as f64;
    // A phase running at an uneven rate, with its cycle count.
    let total = t * 0.0137 + voice.unwrap_or(7) as f64 * 0.31;
    let mut i = FormulaInputs {
        intphase: total.floor(),
        phase: (total - total.floor()) as f32,
        deform: ((t * 0.021).sin() * 0.9) as f32,
        amplitude: (0.5 + 0.5 * (t * 0.013).cos()) as f32,
        eg: [0.1, (t * 0.003).fract() as f32, 0.3, 0.4, ((t * 0.017).sin() * 0.5 + 0.5) as f32, ((t * 0.011).cos() * 0.5 + 0.5) as f32],
        songpos: t * 32.0 / 48000.0 * 2.0,
        macros: [0.1, 0.25, 0.5, 0.75, (t * 0.01).fract() as f32, 0.6, 0.7, 0.9],
        ..FormulaInputs::default()
    };
    if let Some(v) = voice {
        i.voice = Some(VoiceInputs { key: 36.0 + 9.0 * v as f64, velocity: 100.0, channel: 0.0, voice_id: v as f64, tuned_key: 36.0 + 9.0 * v as f64, released: block > 150 + 20 * v, ..Default::default() });
    }
    i
}

/// Runs `src` through both and compares every block; Ok(Some(error)) when
/// both stopped with the same run-time error.
fn compare(src: &str, backend: Backend, blocks: usize, with_scene: bool) -> Result<(Option<String>, bool), String> {
    let formula = Formula::compile_with(src, backend).map_err(|e| e.to_string())?;
    if backend == Backend::Native && cfg!(any(target_arch = "aarch64", target_arch = "x86_64")) && formula.backend() != Backend::Native {
        return Err("the native back end did not compile it".into());
    }
    let mut nonzero = false;
    let chunk = parse::parse(src).unwrap();
    let mut a = Runner::new(formula, 8);
    let mut b = RefRunner::new(&chunk, 8);
    // Three voices starting at different blocks, and one scene instance.
    let starts = [(0usize, Some(0usize)), (5, Some(1)), (40, if with_scene { None } else { Some(3) }), (90, Some(2))];
    let mut slots: Vec<Option<(usize, usize)>> = vec![None; starts.len()];
    for blk in 0..blocks {
        for (n, (at, voice)) in starts.iter().enumerate() {
            if blk == *at {
                let ka = a.acquire().unwrap();
                let kb = b.acquire().unwrap();
                let inp = inputs(blk, *voice);
                a.start(ka, &inp);
                b.start(kb, &inp);
                slots[n] = Some((ka, kb));
            }
            if let Some((ka, kb)) = slots[n] {
                let inp = inputs(blk, *voice);
                let x = a.process(ka, &inp);
                let y = b.process(kb, &inp);
                let ea = a.error().map(|e| e.message);
                let eb = b.error.map(|c| compile::err::message(c).to_string());
                if ea != eb {
                    return Err(format!("block {} instance {}: error {:?} vs reference {:?}", blk, n, ea, eb));
                }
                nonzero |= x.outputs.iter().any(|v| *v != 0.0);
                if !same_io(&x, &y) {
                    return Err(format!("block {} instance {}: {:?}\n  reference {:?}", blk, n, x, y));
                }
            }
        }
    }
    Ok((a.error().map(|e| e.to_string()), nonzero))
}

/// Equal outputs (bit for bit, NaN included) and flags.
fn same_io(a: &FormulaIo, b: &FormulaIo) -> bool {
    a.outputs.iter().zip(&b.outputs).all(|(x, y)| x.to_bits() == y.to_bits()) && FormulaIo { outputs: [0.0; 16], ..*a } == FormulaIo { outputs: [0.0; 16], ..*b }
}

fn both(src: &str) {
    for be in [Backend::Interp, Backend::Native] {
        if let Err(e) = compare(src, be, 300, true) {
            panic!("{:?}: {}\n--- script ---\n{}", be, e, src);
        }
    }
}

fn first_out(src: &str, blocks: usize) -> Vec<f32> {
    let formula = Formula::compile(src).unwrap_or_else(|e| panic!("{}", e));
    let mut r = Runner::new(formula, 2);
    let k = r.acquire().unwrap();
    r.start(k, &inputs(0, Some(0)));
    (0..blocks).map(|b| r.process(k, &inputs(b, Some(0))).outputs[0]).collect()
}

#[test]
fn field_indices_match_names() {
    assert_eq!(NUM_FIELDS[f::SAMPLERATE], "samplerate");
    assert_eq!(NUM_FIELDS[f::EG], "delay");
    assert_eq!(NUM_FIELDS[f::EG + 5], "release");
    assert_eq!(NUM_FIELDS[f::LFO_ID], "lfo_id");
    assert_eq!(NUM_FIELDS[f::TUNED_KEY], "tuned_key");
    assert_eq!(NUM_FIELDS[f::CYCLE], "cycle");
    assert_eq!(NUM_FIELDS[f::CC_MW + 3], "cc_sus");
    assert_eq!(NUM_FIELDS[f::LATEST_KEY], "latest_key");
    assert_eq!(NUM_FIELDS[f::MPE_PRESSURE], "mpe_pressure");
    assert_eq!(NUM_FIELDS.len(), f::MPE_PRESSURE + 1);
    assert_eq!(BOOL_FIELDS[f::RELEASED], "released");
    assert_eq!(BOOL_FIELDS[f::USE_RATE], "use_rate");
    assert_eq!(BOOL_FIELDS[f::RETRIGGER_FEG], "retrigger_FEG");
    assert_eq!(BOOL_FIELDS.len(), f::RETRIGGER_FEG + 1);
}

#[test]
fn default_script_is_a_saw() {
    let o = first_out(DEFAULT_SCRIPT, 50);
    for (b, x) in o.iter().enumerate() {
        let p = inputs(b, Some(0)).phase as f64;
        assert_eq!(*x, (p * 2.0 - 1.0) as f32);
    }
    both(DEFAULT_SCRIPT);
}

#[test]
fn language_basics() {
    both(
        "local k = 3\n\
         local function sq(x) return x * x end\n\
         function init(state) state.n = 0; state.t = {1, 2, 3, x = 5}; return state end\n\
         function process(state)\n\
           state.n = state.n + 1\n\
           local acc = 0\n\
           for i = 1, #state.t do acc = acc + state.t[i] * k end\n\
           local j = 10\n\
           while j > 0 do j = j - 3; if j < 2 then break end end\n\
           repeat local z = j; j = j + 1 until z > 4\n\
           local s = (state.phase < 0.5 and -1 or 1) * sq(state.deform)\n\
           if state.n % 3 == 0 then s = s + 0.25 elseif state.n % 3 == 1 then s = s - 0.25 else s = -s end\n\
           state.output = { s, acc / 100, j / 10, state.t.x / 10, not state.released and 0.5 or -0.5, math.floor(state.cycle) % 4 / 4 }\n\
           return state\n\
         end",
    );
}

#[test]
fn closures_returns_and_multiple_values() {
    both(
        "function process(state)\n\
           local count = 0\n\
           local function bump(d) count = count + d; if count > 3 then return count, true end; return count, false end\n\
           local a, b = bump(2)\n\
           local c, d = bump(2)\n\
           local ip, fp = math.modf(state.phase * 7)\n\
           local mn = math.min(state.phase, state.deform, 0.3)\n\
           return (b and 1 or 0) + (d and 0.5 or 0) + fp * 0.1 + mn * 0.1 + a * 0 + c * 0 + ip * 0\n\
         end",
    );
}

#[test]
fn tables_alias_and_survive_collection() {
    // A table kept from earlier blocks (through a global and through
    // state) while new ones are made every block: aliasing must hold and
    // the collector must keep only the reachable ones.
    both(
        "function init(state) state.prior = {0, 0}; state.next = {0.5, 0.5}; state.counter = -1; return state end\n\
         function process(state)\n\
           local tmp = { state.phase, state.deform }\n\
           if state.cycle ~= state.counter then\n\
             state.priorval = state.next\n\
             state.next = {}\n\
             for i = 1, 2 do state.next[i] = math.random() end\n\
             keep = state.next\n\
             state.counter = state.cycle\n\
           end\n\
           local o = {}\n\
           for i = 1, 2 do o[i] = keep[i] * 2 - 1 + tmp[i] * 0 end\n\
           o[3] = (state.priorval or state.next)[1]\n\
           state.output = o\n\
           return state\n\
         end",
    );
    // Thousands of blocks of garbage: the heap must not fill up.
    let src = "function process(state) local t = {1, 2, 3}; local u = {t, {4}}; state.output = u[1][3] / 3; return state end";
    let formula = Formula::compile(src).unwrap();
    let mut r = Runner::new(formula, 4);
    let k = r.acquire().unwrap();
    r.start(k, &inputs(0, None));
    for b in 0..5000 {
        let o = r.process(k, &inputs(b, None));
        assert_eq!(o.outputs[0], 1.0);
    }
    assert!(r.error().is_none());
    assert!(r.live_tables() < N_OBJ);
}

#[test]
fn process_globals_are_shared_by_instances() {
    // Addendum 5: a global assigned in process is one variable for every
    // instance of the text; init's globals are separate.
    let src = "function init(state) g = 100; return state end\n\
               function process(state) total = (total or 0) + 1; state.output = { total / 100, (g or -1) / 100 }; return state end";
    both(src);
    let formula = Formula::compile_with(src, Backend::Interp).unwrap();
    let mut r = Runner::new(formula, 4);
    let (a, b) = (r.acquire().unwrap(), r.acquire().unwrap());
    r.start(a, &inputs(0, Some(0)));
    r.start(b, &inputs(0, Some(1)));
    assert_eq!(r.process(a, &inputs(0, Some(0))).outputs[..2], [0.01, -0.01]);
    assert_eq!(r.process(b, &inputs(0, Some(1))).outputs[..2], [0.02, -0.01]);
}

#[test]
fn strings_sub_find_len() {
    both(
        "function init(m) m.grid = '123A456B789C*0#D'; m.number = '08755501942'; m.i = 0; return m end\n\
         function process(m)\n\
           m.i = m.i % #m.number + 1\n\
           local digit = m.number:sub(m.i, m.i)\n\
           local idx = m.grid:find(digit)\n\
           local star = m.grid:find('*')\n\
           local same = (digit == '5') and 1 or 0\n\
           local tail = string.sub(m.grid, -2)\n\
           m.output = { (idx or 0) / 16, star / 16, same, #tail / 4, m.grid:byte(1) / 100, type(m.grid) == 'string' and 1 or 0 }\n\
           return m\n\
         end",
    );
}

#[test]
fn helpers_follow_the_addendum() {
    // Slew: first output equals the input; rate 0 jumps; the default rate
    // limits a unit step to 1 / (1 + 10000 * 0.2^3) per block at 48 kHz.
    let src = "function init(s) s.a = surge.mod.Slew:new{ samplerate = s.samplerate, block_size = s.block_size }; s.b = surge.mod.Slew:new{}; s.n = 0; return s end\n\
               function process(s) s.n = s.n + 1; local x = s.n > 1 and 1 or 0; s.output = { s.a:run(x), s.b:run(x), s.a.block_factor }; s.clamp_output = false; return s end";
    both(src);
    let formula = Formula::compile(src).unwrap();
    let mut r = Runner::new(formula, 1);
    let k = r.acquire().unwrap();
    r.start(k, &inputs(0, None));
    let o1 = r.process(k, &inputs(0, None));
    assert_eq!(o1.outputs[0], 0.0);
    assert_eq!(o1.outputs[1], 0.0);
    let o2 = r.process(k, &inputs(1, None));
    let lim = 1.0 / (1.0 + 10000.0 * 0.2f64.powf(3.0));
    assert!((o2.outputs[0] as f64 - lim).abs() < 1e-6, "{} vs {}", o2.outputs[0], lim);
    assert_eq!(o2.outputs[1], 0.0, "no samplerate: calculate is false, output 0");
    assert_eq!(o2.outputs[2], 1.0);

    // ClockDivider: ibeat wraps with the numerator; the typo keeps numerator 1.
    both(
        "function init(s) s.c = surge.mod.ClockDivider:new{ numerator = 3, denominator = 2 }; s.t = surge.mod.ClockDivider:new{ enumerator = 5, denominator = 4 }; return s end\n\
         function process(s) s.c:tick(s.intphase, s.phase); s.t:tick(s.intphase, s.phase); s.output = { s.c.ibeat / 3, s.c.phase, s.c.newbeat and 1 or 0, s.t.ibeat, s.t.intphase / 10 }; return s end",
    );
    // AHD: piecewise linear, defaults 0.1 0.1 0.7, and prototype defaults
    // a script changes apply to later instances.
    both(
        "function init(s) surge.mod.AHDEnvelope.d = 0.5; s.e = surge.mod.AHDEnvelope:new{ a = 0.2 }; return s end\n\
         function process(s) s.output = { s.e:at(s.phase), s.e:at(s.phase * 2 - 0.5), s.e.d }; return s end",
    );
}

#[test]
fn math_library() {
    both(
        "function process(s)\n\
           local p = s.phase + 0.01\n\
           s.clamp_output = false\n\
           s.output = { sin(p * 7), math.cos(p), math.tan(p), math.atan(p), math.atan2(p, -0.3), math.asin(p * 0.9), math.acos(p * 0.9), math.exp(p), math.log(p), math.log(p, 2), math.log10(p), p ^ 2.5, math.sqrt(p), math.fmod(p * 9, 2), math.tanh(p * 3), clamp(p * 3 - 1, -0.5, 0.5) }\n\
           return s\n\
         end",
    );
    both(
        "function process(s)\n\
           local p = s.phase\n\
           s.clamp_output = false\n\
           s.output = { math.note_to_freq(60 + p) / 1000, math.freq_to_note(300 + p) / 100, math.lin_to_dB(p + 0.1) / 10, math.dB_to_lin(-6 * p), math.gcd(12, 18), math.lcm(4, 6), math.sgn(p - 0.5), math.sign(0), math.parity(s.cycle), math.rescale(p, 0, 1, -2, 2), math.norm(p, 0.5), math.range(p, 0.2), math.random(3, 5), math.random(4), math.random_normal(0, 0.1), math.random_exponential(2) }\n\
           return s\n\
         end",
    );
    both(
        "function init(s) math.randomseed(s.songpos); s.a = math.random(); return s end\n\
         function process(s) math.randomseed(s.intphase); local p = math.random_poisson(3); local q = math.random_poisson(40); s.clamp_output = false; s.output = { s.a, p / 10, q / 100, math.random_bernoulli(0.3) and 1 or 0 }; return s end",
    );
}

#[test]
fn runtime_errors_stop_the_formula() {
    let src = "function process(s) if s.cycle > 2 then return s.missing + 1 end; return 0.5 end";
    both(src);
    let formula = Formula::compile(src).unwrap();
    let mut r = Runner::new(formula, 2);
    let k = r.acquire().unwrap();
    r.start(k, &inputs(0, None));
    let mut stopped = false;
    for b in 0..400 {
        let o = r.process(k, &inputs(b, None));
        if r.error().is_some() {
            stopped = true;
            assert_eq!(o.outputs[0], 0.0);
        }
    }
    assert!(stopped);
    let e = r.error().unwrap();
    assert!(e.runtime && e.line == 1, "{}", e);
}

#[test]
fn load_errors_are_reported() {
    for (src, what) in [
        ("function process(s) return s.a .. 'x' end", "concatenation"),
        ("function process(s) for k, v in pairs(s) do end return 0 end", "for"),
        ("local function f(n) return f(n) end\nfunction process(s) return f(1) end", "recursive"),
        ("function helper() return 1 end\nfunction process(s) return helper() end", "top level"),
        ("function process(s) s.f = function() end return 0 end", "function"),
        ("function init(s) return s end", "process"),
        ("function process(s) return shared.x end", "shared"),
        ("function process(s return s end", "expected"),
    ] {
        match Formula::compile(src) {
            Ok(_) => panic!("{} should not compile", src),
            Err(e) => assert!(e.message.contains(what) && !e.runtime, "{}: {}", src, e),
        }
    }
}

fn library_dir() -> std::path::PathBuf {
    match std::env::var("FORMULA_LIBRARY") {
        Ok(p) => p.into(),
        Err(_) => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../local/agent_state/synth/formula/lib"),
    }
}

/// Every script of the patch library compiles and runs like the reference.
#[test]
fn library_scripts_compile_and_match() {
    let dir = library_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        eprintln!("formula library not present at {} (patch content; fetch it with the library download): skipped", dir.display());
        return;
    };
    let mut files: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "lua")).collect();
    files.sort();
    let mut fails = Vec::new();
    for p in &files {
        let src = std::fs::read_to_string(p).unwrap();
        for be in [Backend::Interp, Backend::Native] {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            // As voice LFOs every script must run; with a scene instance
            // too (no key, velocity, ...) a script may stop, but only the
            // way the reference does.
            match compare(&src, be, 600, false) {
                Ok((None, true)) => {}
                Ok((None, false)) => fails.push(format!("{} ({:?}): every output was 0", name, be)),
                Ok((Some(e), _)) => fails.push(format!("{} ({:?}): stopped: {}", name, be, e)),
                Err(e) => fails.push(format!("{} ({:?}): {}", name, be, e)),
            }
            if let Err(e) = compare(&src, be, 600, true) {
                fails.push(format!("{} ({:?}, with a scene LFO): {}", name, be, e));
            }
        }
    }
    eprintln!("formula library: {} scripts, {} failures", files.len(), fails.len());
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

/// Compile cost of the library (load time matters): printed with --nocapture.
#[test]
fn library_compile_cost() {
    let Ok(rd) = std::fs::read_dir(library_dir()) else { return };
    let mut total = std::time::Duration::ZERO;
    let mut worst = (std::time::Duration::ZERO, String::new());
    let mut n = 0;
    for p in rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "lua")) {
        let src = std::fs::read_to_string(&p).unwrap();
        let t = std::time::Instant::now();
        let f = Formula::compile(&src).unwrap();
        let d = t.elapsed();
        total += d;
        n += 1;
        if d > worst.0 {
            worst = (d, format!("{} ({} AIR values)", p.file_name().unwrap().to_string_lossy(), f.process_program().vals.len()));
        }
    }
    eprintln!("formula compile: {} scripts in {:?}, worst {:?} {}", n, total, worst.0, worst.1);
}
