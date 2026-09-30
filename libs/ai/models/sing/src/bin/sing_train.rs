//! Cantor's trainer (runs on the GPU box).
//!
//!   sing_train check                      CPU vs GPU: losses and every parameter
//!                                         gradient of both models on one batch
//!   sing_train t0  --out DIR [opts]       the synthetic singer, both models
//!   sing_train voc --data DIR... --out DIR [opts]
//!   sing_train ac  --data DIR... --out DIR [opts]
//!
//! opts: --steps N --batch B --frames T --lr X --config tiny|base --cpu
//!       --resume --log N --save N --workers N
//!
//! Every op runs on the device (nn_gpu); CPU worker threads build batches
//! ahead into a bounded queue; per step the host only enqueues work, and reads
//! scalars back at log steps.

use makepad_ai_sing::acoustic::{self, AcousticConfig};
use makepad_ai_sing::data::{self, Kind};
use makepad_ai_sing::dsp::{self, Rng, HOP, SR};
use makepad_ai_sing::nn::{Graph, Params};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use makepad_ai_sing::nn::Tensor;
use makepad_ai_sing::train::{self, AcBatch, Aligned, OptConfig, Optimizer, SynthSinger, VocBatch};
use makepad_ai_sing::vocoder::{self, VocoderConfig};
use makepad_ai_sing::Cantor;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

struct Args(Vec<String>);

impl Args {
    fn get(&self, k: &str) -> Option<String> {
        self.0.iter().position(|a| a == k).and_then(|i| self.0.get(i + 1)).cloned()
    }
    fn all(&self, k: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.0.len() {
            if self.0[i] == k {
                if let Some(v) = self.0.get(i + 1) {
                    out.push(v.clone());
                }
                i += 1;
            }
            i += 1;
        }
        out
    }
    fn num<T: std::str::FromStr>(&self, k: &str, d: T) -> T {
        self.get(k).and_then(|v| v.parse().ok()).unwrap_or(d)
    }
    fn flag(&self, k: &str) -> bool {
        self.0.iter().any(|a| a == k)
    }
}

/// Worker threads that each run `make(rng)` forever into a bounded queue.
fn prefetch<T: Send + 'static>(workers: usize, depth: usize, seed: u64, make: Arc<dyn Fn(&mut Rng) -> T + Send + Sync>) -> Receiver<T> {
    let (tx, rx) = sync_channel(depth);
    for w in 0..workers {
        let tx = tx.clone();
        let make = make.clone();
        std::thread::spawn(move || {
            let mut rng = Rng::new(seed.wrapping_add(w as u64 * 1_000_003));
            loop {
                if tx.send(make(&mut rng)).is_err() {
                    return;
                }
            }
        });
    }
    rx
}

fn configs(a: &Args) -> (AcousticConfig, VocoderConfig) {
    match a.get("--config").as_deref() {
        Some("base") => (AcousticConfig::base(), VocoderConfig::base()),
        _ => (AcousticConfig::tiny(), VocoderConfig::tiny()),
    }
}

fn declare(ac: Option<&AcousticConfig>, voc: Option<&VocoderConfig>, seed: u64) -> Params {
    let mut rng = Rng::new(seed);
    let mut p = Params::new();
    if let Some(c) = ac {
        c.declare(&mut p, &mut rng);
    }
    if let Some(c) = voc {
        c.declare(&mut p, &mut rng);
    }
    p
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod gpu {
    pub use makepad_ai_cuda::train::{mem, release_pool, staging_begin_step, staging_end_step, staging_init, sync};
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
#[allow(dead_code)]
mod gpu {
    pub fn staging_init(_: usize) {}
    pub fn staging_begin_step() {}
    pub fn staging_end_step() {}
    pub fn sync() {}
    pub fn mem() -> (usize, usize) {
        (0, 0)
    }
    pub fn release_pool() {}
}

/// One training loop over batches from `next`, with `loss` building the graph.
#[allow(clippy::too_many_arguments)]
fn fresh_schedule(opt: &mut Optimizer) {
    opt.cfg.start = opt.step;
}

fn run<B>(
    name: &str, opt: &mut Optimizer, use_gpu: bool, steps: usize, log_every: usize, save_every: usize, out: &Path,
    config: &[(String, String)], rx: &Receiver<B>, loss: &dyn Fn(&mut Graph, &B, &mut Rng) -> Vec<(&'static str, usize)>,
    on_save: &dyn Fn(&Optimizer, usize),
) {
    fresh_schedule(opt);
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    if use_gpu {
        opt.to_device();
        gpu::staging_init(512 << 20);
    }
    let _ = use_gpu;
    let mut rng = Rng::new(opt.step as u64 + 17);
    let mut t_log = Instant::now();
    let mut wait = 0.0f64;
    let start_step = opt.step;
    // SING_PROFILE=1: per phase, host enqueue time and time to device idle.
    let profile = std::env::var("SING_PROFILE").map(|v| v == "1").unwrap_or(false);
    let light = std::env::var("SING_PROFILE").map(|v| v == "2").unwrap_or(false);
    let mut prof = [0.0f64; 6];
    let mut lp = [0.0f64; 5];
    while opt.step < start_step + steps {
        let tw = Instant::now();
        let batch = rx.recv().expect("data workers died");
        wait += tw.elapsed().as_secs_f64();
        let tl = Instant::now();
        gpu::staging_begin_step();
        lp[0] += tl.elapsed().as_secs_f64();
        let tl = Instant::now();
        let grads;
        let mut readouts: Vec<(&'static str, usize)>;
        let losses: Option<Vec<(&'static str, f32)>>;
        {
            let params = &opt.params;
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            let mut g = match &opt.dev {
                Some(d) => Graph::new_gpu(params, &d.params, true),
                None => Graph::new(params, true),
            };
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            let mut g = Graph::new(params, true);
            let tp = Instant::now();
            readouts = loss(&mut g, &batch, &mut rng);
            if profile {
                prof[0] += tp.elapsed().as_secs_f64();
                gpu::sync();
                prof[1] += tp.elapsed().as_secs_f64();
            }
            let total = readouts[0].1;
            let tp = Instant::now();
            grads = g.backward(total);
            if profile {
                prof[2] += tp.elapsed().as_secs_f64();
                gpu::sync();
                prof[3] += tp.elapsed().as_secs_f64();
            }
            let log_now = (opt.step + 1) % log_every == 0;
            losses = log_now.then(|| readouts.drain(..).map(|(n, id)| (n, g.host(id)[0])).collect());
        }
        lp[1] += tl.elapsed().as_secs_f64();
        let tp = Instant::now();
        let lr = opt.apply(&grads);
        drop(grads);
        if profile {
            prof[4] += tp.elapsed().as_secs_f64();
            gpu::sync();
            prof[5] += tp.elapsed().as_secs_f64();
        }
        gpu::staging_end_step();
        if let Some(l) = losses {
            gpu::sync();
            let dt = t_log.elapsed().as_secs_f64() / log_every as f64;
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            let gn = opt.grad_norm();
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            let gn = 0.0;
            let (free, total) = gpu::mem();
            let parts: Vec<String> = l.iter().map(|(n, v)| format!("{n} {v:.4}")).collect();
            eprintln!(
                "[{name}] step {} | {} | gnorm {gn:.3} lr {lr:.2e} | {:.1} ms/step (data wait {:.1} ms) | gpu mem {:.1}/{:.1} GB",
                opt.step,
                parts.join(" "),
                dt * 1000.0,
                wait * 1000.0 / log_every as f64,
                (total - free) as f64 / 1e9,
                total as f64 / 1e9
            );
            if light {
                let k = 1000.0 / log_every as f64;
                eprintln!("  light ms/step: staging wait {:.1} | graph build+backward+drop {:.1}", lp[0] * k, lp[1] * k);
                lp = [0.0; 5];
            }
            if profile {
                let k = 1000.0 / log_every as f64;
                eprintln!(
                    "  profile ms/step: forward enqueue {:.1} done {:.1} | backward enqueue {:.1} done {:.1} | optimiser enqueue {:.1} done {:.1}",
                    prof[0] * k, prof[1] * k, prof[2] * k, prof[3] * k, prof[4] * k, prof[5] * k
                );
                prof = [0.0; 6];
            }
            t_log = Instant::now();
            wait = 0.0;
        }
        if opt.step % save_every == 0 || opt.step == start_step + steps {
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            opt.sync_host();
            opt.save(&out.join(name), config).expect("checkpoint");
            on_save(opt, opt.step);
        }
    }
}

/// The vocoder's GAN loop: per step a generator update (reconstruction +
/// adversarial + feature matching; discriminator gradients dropped) and a
/// discriminator update on the detached output.
#[allow(clippy::too_many_arguments)]
fn run_gan(
    opt: &mut Optimizer, use_gpu: bool, steps: usize, log_every: usize, save_every: usize, out: &Path,
    config: &[(String, String)], rx: &Receiver<VocBatch>, voc: &VocoderConfig, on_save: &dyn Fn(&Optimizer, usize),
) {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    if use_gpu {
        opt.to_device();
        gpu::staging_init(512 << 20);
    }
    let _ = use_gpu;
    fresh_schedule(opt);
    let is_d: Vec<bool> = opt.params.names.iter().map(|n| n.starts_with("disc.")).collect();
    let mut t_log = Instant::now();
    let start_step = opt.step;
    while opt.step < start_step + steps {
        let batch = rx.recv().expect("data workers died");
        gpu::staging_begin_step();
        let log_now = (opt.step + 1) % log_every == 0;
        let mut logged: Vec<(&str, f32)> = Vec::new();
        let (fake, real);
        let lr;
        {
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            let mut g = match &opt.dev {
                Some(d) => Graph::new_gpu(&opt.params, &d.params, true),
                None => Graph::new(&opt.params, true),
            };
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            let mut g = Graph::new(&opt.params, true);
            let l = train::vocoder_gan_loss(&mut g, voc, &batch);
            let mut grads = g.backward(l.total);
            for (i, d) in is_d.iter().enumerate() {
                if *d {
                    grads[i] = None;
                }
            }
            if log_now {
                for (n, id) in [("stft", l.stft), ("mel", l.mel), ("adv", l.adv), ("fm", l.fm)] {
                    logged.push((n, g.host(id)[0]));
                }
            }
            fake = (*g.val(l.wave)).clone();
            real = (*g.val(l.real)).clone();
            drop(g);
            lr = opt.apply_part(&grads, false);
        }
        {
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            let mut g = match &opt.dev {
                Some(d) => Graph::new_gpu(&opt.params, &d.params, true),
                None => Graph::new(&opt.params, true),
            };
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            let mut g = Graph::new(&opt.params, true);
            let (fi, ri) = (g.input(fake), g.input(real));
            let dl = makepad_ai_sing::disc::d_loss(&mut g, ri, fi);
            let mut grads = g.backward(dl);
            for (i, d) in is_d.iter().enumerate() {
                if !*d {
                    grads[i] = None;
                }
            }
            if log_now {
                logged.push(("disc", g.host(dl)[0]));
            }
            drop(g);
            opt.apply_part(&grads, true);
        }
        gpu::staging_end_step();
        if log_now {
            gpu::sync();
            let dt = t_log.elapsed().as_secs_f64() / log_every as f64;
            let parts: Vec<String> = logged.iter().map(|(n, v)| format!("{n} {v:.4}")).collect();
            eprintln!("[voc-gan] step {} | {} | lr {lr:.2e} | {:.1} ms/step", opt.step, parts.join(" "), dt * 1000.0);
            t_log = Instant::now();
        }
        if opt.step % save_every == 0 || opt.step == start_step + steps {
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            opt.sync_host();
            opt.save(&out.join("voc"), config).expect("checkpoint");
            on_save(opt, opt.step);
        }
    }
}

fn load_items(dirs: &[String]) -> Vec<data::Item> {
    let mut shards = Vec::new();
    for d in dirs {
        for e in std::fs::read_dir(d).unwrap_or_else(|e| panic!("{d}: {e}")).flatten() {
            if e.path().extension().map(|x| x == "mksdat").unwrap_or(false) {
                shards.push(e.path());
            }
        }
    }
    shards.sort();
    let items = std::sync::Mutex::new(Vec::new());
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..16 {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= shards.len() {
                    return;
                }
                let v = data::read_shard(&shards[i]).unwrap();
                items.lock().unwrap().extend(v);
            });
        }
    });
    let mut v = items.into_inner().unwrap();
    v.sort_by_key(|it| (it.speaker, it.audio.len()));
    v
}

/// A vocoder crop: `t` frames from a random item (skipping mostly silent crops).
fn voc_crop(items: &[data::Item], t: usize, rng: &mut Rng) -> (Vec<f32>, Vec<f32>, f32) {
    loop {
        let it = &items[rng.below(items.len())];
        if it.frames() < t + 2 {
            continue;
        }
        let s = rng.below(it.frames() - t);
        let audio: Vec<f32> = it.audio[s * HOP..(s + t) * HOP].iter().map(|v| *v as f32 / 32768.0).collect();
        let rms = (audio.iter().map(|v| v * v).sum::<f32>() / audio.len() as f32).sqrt();
        if rms < 1e-3 {
            continue;
        }
        return (audio, it.f0[s..s + t].to_vec(), it.band_hz);
    }
}

fn write_wav(path: &Path, x: &[f32]) {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    let y: Vec<f32> = x.iter().map(|v| v * (0.9 / peak).min(1.0)).collect();
    std::fs::write(path, dsp::wav_bytes(SR, &y)).unwrap();
}

/// Differential check: the same loss and gradients on the CPU and the GPU.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn check() {
    // Exact f32 GEMMs first (the kernels must match the CPU reference), then TF32.
    makepad_ai_cuda::train::set_tf32(false);
    // The device harmonic source against the host one.
    {
        let f0: Vec<f32> = (0..200).map(|i| if i % 50 < 40 { 110.0 * (1.0 + i as f32 / 100.0) } else { 0.0 }).collect();
        let src = makepad_ai_sing::vocoder::SourceCtl::new(&f0, 100, 1);
        let params = Params::new();
        let g = Graph::new_gpu(&params, &[], false);
        let (h_dev, _) = g.source_waves(&src);
        let (h_host, _) = src.host_waves();
        let d = h_dev.host();
        let err = d.iter().zip(&h_host.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        let peak = h_host.data.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        eprintln!("harmonic source: max |device - host| {err:.2e} (peak {peak:.2})");
        if err > 1e-3 * peak {
            std::process::exit(1);
        }
    }
    let exact = check_once(1e-3);
    makepad_ai_cuda::train::set_tf32(true);
    let tf32 = check_once(3e-2);
    if !(exact && tf32) {
        std::process::exit(1);
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn check_once(tol: f64) -> bool {
    makepad_ai_sing::nn::force_cpu(true);
    makepad_ai_sing::nn::host_noise(true);
    let ac = AcousticConfig::tiny();
    let voc = VocoderConfig::tiny();
    let params = declare(Some(&ac), Some(&voc), 5);
    let dev: Vec<Arc<Tensor>> = params.vals.iter().map(|t| Arc::new(makepad_ai_sing::nn_gpu::upload((**t).clone()))).collect();
    let synth = SynthSinger::new(3);
    let mut rng = Rng::new(9);
    let items: Vec<Aligned> = (0..3).map(|_| synth.example(&mut rng).crop(160, &mut rng)).collect();
    let ab = train::ac_batch(&items);
    let unaligned: Vec<Aligned> = items.iter().map(|a| Aligned { dur: Vec::new(), ..a.clone() }).collect();
    let abm = train::ac_batch(&unaligned);
    let crops: Vec<_> = items.iter().map(|a| {
        let t = 40.min(a.frames());
        (a.audio[..t * HOP].to_vec(), a.f0[..t].to_vec(), if a.frames() % 2 == 0 { 24_000.0 } else { 12_000.0 })
    }).collect();
    let t = crops.iter().map(|c| c.1.len()).min().unwrap();
    let crops: Vec<_> = crops.into_iter().map(|(a, f, b)| (a[..t * HOP].to_vec(), f[..t].to_vec(), b)).collect();
    let mut vb = train::voc_batch(&crops, t, 4);
    vb.src.host = true;
    let mut worst = 0.0f64;
    for (what, run_ac, mas) in [("acoustic", true, false), ("acoustic, aligned by MAS", true, true), ("vocoder", false, false)] {
        let eval = |gpu: bool| {
            let mut g = if gpu { Graph::new_gpu(&params, &dev, true) } else { Graph::new(&params, true) };
            let mut r = Rng::new(77);
            let (loss, parts) = if run_ac {
                let l = train::acoustic_loss(&mut g, &ac, if mas { &abm } else { &ab }, &mut r);
                (l.total, vec![l.mel, l.dur, l.f0, l.voicing, l.flow, l.prior])
            } else {
                let l = train::vocoder_loss(&mut g, &voc, &vb);
                (l.total, vec![l.stft, l.mel])
            };
            let vals: Vec<f32> = parts.iter().map(|p| g.host(*p)[0]).collect();
            let grads: Vec<Option<Vec<f32>>> = g.backward(loss).iter().map(|x| x.as_ref().map(|b| b.to_host())).collect();
            (vals, grads)
        };
        let t0 = Instant::now();
        let (cv, cg) = eval(false);
        let tc = t0.elapsed();
        let t0 = Instant::now();
        let (gv, gg) = eval(true);
        let tg = t0.elapsed();
        eprintln!("{what}: losses cpu {cv:?}\n{what}: losses gpu {gv:?}  (cpu {:.0} ms, gpu {:.0} ms)", tc.as_secs_f64() * 1e3, tg.as_secs_f64() * 1e3);
        for (a, b) in cv.iter().zip(&gv) {
            let r = ((a - b).abs() / a.abs().max(1e-3)) as f64;
            worst = worst.max(r);
        }
        if mas && tol > 1e-3 {
            // A discrete alignment follows TF32 rounding upstream of it; its
            // gradients are compared in the exact pass only.
            eprintln!("  {what}: gradients compared in the exact-f32 pass only");
            continue;
        }
        let mut shown = 0;
        for (i, (a, b)) in cg.iter().zip(&gg).enumerate() {
            match (a, b) {
                (Some(a), Some(b)) => {
                    let na: f64 = a.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
                    let nd: f64 = a.iter().zip(b).map(|(x, y)| ((x - y) as f64).powi(2)).sum::<f64>().sqrt();
                    let rel = nd / na.max(1e-8);
                    worst = worst.max(if na < 1e-6 { 0.0 } else { rel });
                    if rel > tol && na > 1e-6 && shown < 12 {
                        eprintln!("  grad {} rel err {rel:.3e} (|g| {na:.3e})", params.names[i]);
                        shown += 1;
                    }
                }
                (None, None) => {}
                _ => eprintln!("  grad {}: present on one side only", params.names[i]),
            }
        }
    }
    eprintln!("worst relative error {worst:.3e} (tolerance {tol:.0e})");
    worst <= tol
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn check() {
    eprintln!("check needs the CUDA backend (Linux/Windows)");
}

fn sample_render(dir: &Path, step: usize, ac_p: Option<&Params>, voc_p: Option<&Params>, ac: &AcousticConfig, voc: &VocoderConfig, reference: Option<&Aligned>) {
    // Assemble a Cantor from whatever is trained, random for the rest.
    let mut c = Cantor::random(ac.clone(), voc.clone(), 1);
    for p in [ac_p, voc_p].into_iter().flatten() {
        for (n, t) in p.names.iter().zip(&p.vals) {
            c.params.insert(n, (**t).clone());
        }
    }
    makepad_ai_sing::nn::force_cpu(false);
    if let Some(r) = reference {
        // Copy synthesis of the reference: its mel and f0 through the vocoder.
        let t = r.frames();
        let mut g = Graph::new(&c.params, false);
        let m = train::target_mel(&mut g, &r.audio[..t * HOP].iter().copied().chain(std::iter::repeat(0.0)).take(t * HOP).collect::<Vec<_>>(), 1, t);
        let src = vocoder::SourceCtl::new(&r.f0, t, 3);
        let w = vocoder::forward(&mut g, &c.voc, m, &r.f0, t, &src);
        let audio = g.host(w);
        let (cents, frames) = makepad_ai_sing::cantor::f0_error_cents(&audio, &r.f0);
        // Mel L1 of the copy against the reference.
        let mut g2 = Graph::new(&c.params, false);
        let a = train::target_mel(&mut g2, &audio, 1, t);
        let b = train::target_mel(&mut g2, &r.audio[..t * HOP], 1, t);
        let l = g2.l1_loss(a, b, None, None);
        let mel_l1 = g2.host(l)[0] * acoustic::MEL_STD;
        write_wav(&dir.join(format!("copy-{step:07}.wav")), &audio);
        write_wav(&dir.join("reference.wav"), &r.audio);
        eprintln!("[sample] step {step}: vocoder copy-synthesis f0 error {cents:.2} cents over {frames} frames, log-mel L1 {mel_l1:.3}");
    }
    if ac_p.is_some() {
        c.durations_trained = true;
        let line = makepad_ai_sing::score::simple_line(
            &[(60.0, 1.0, "lɑ"), (62.0, 1.0, "mi"), (64.0, 2.0, "soʊ"), (62.0, 1.0, "_"), (60.0, 2.0, "dus")],
            96.0,
        );
        let (audio, phrases) = c.render(&line, &makepad_ai_sing::RenderOpts::default());
        for p in &phrases {
            let (cents, n) = makepad_ai_sing::cantor::f0_error_cents(&p.audio, &p.frames.f0);
            eprintln!("[sample] step {step}: sung line f0 error {cents:.2} cents over {n} frames");
        }
        write_wav(&dir.join(format!("line-{step:07}.wav")), &audio);
    }
}

fn main() {
    let a = Args(std::env::args().collect());
    let mode = a.0.get(1).cloned().unwrap_or_default();
    if mode == "check" {
        check();
        return;
    }
    if mode == "export" {
        // sing_train export --ac A.ema.mksing --voc V.ema.mksing --out cantor.mksing
        let mut c: Option<Cantor> = None;
        let mut params = Params::new();
        let mut cfg: Vec<(String, String)> = Vec::new();
        for key in ["--ac", "--voc"] {
            if let Some(p) = a.get(key) {
                let w = makepad_ai_sing::weights::read(Path::new(&p)).unwrap_or_else(|e| panic!("{p}: {e}"));
                for (n, t) in w.params.names.iter().zip(&w.params.vals) {
                    if !n.starts_with("disc.") && n.starts_with(if key == "--ac" { "ac." } else { "voc." }) {
                        params.insert(n, (**t).clone());
                    }
                }
                cfg = w.config.clone();
            }
        }
        let ac = AcousticConfig::from_kv(&makepad_ai_sing::weights::section(&cfg, "ac.")).expect("acoustic config");
        let voc = VocoderConfig::from_kv(&makepad_ai_sing::weights::section(&cfg, "voc.")).expect("vocoder config");
        let mut cantor = Cantor::random(ac, voc, 1);
        for (n, t) in params.names.iter().zip(&params.vals) {
            cantor.params.insert(n, (**t).clone());
        }
        cantor.durations_trained = a.get("--ac").is_some();
        cantor.f0_trained = a.flag("--f0-trained");
        let out = PathBuf::from(a.get("--out").unwrap_or_else(|| "cantor.mksing".into()));
        let mut kv = cantor.config();
        kv.push(("credits".into(), makepad_ai_sing::CREDITS.into()));
        makepad_ai_sing::weights::write(&out, &kv, &cantor.params, makepad_ai_sing::weights::Dtype::F16).unwrap();
        c.replace(cantor);
        eprintln!("wrote {} ({} params)", out.display(), c.unwrap().params.count());
        return;
    }
    let out = PathBuf::from(a.get("--out").unwrap_or_else(|| "cantor-run".into()));
    std::fs::create_dir_all(&out).unwrap();
    let (ac, voc) = configs(&a);
    let steps = a.num("--steps", 1000usize);
    let batch = a.num("--batch", 16usize);
    let frames = a.num("--frames", 100usize);
    let log_every = a.num("--log", 50usize);
    let save_every = a.num("--save", 5000usize);
    let workers = a.num("--workers", 12usize);
    let use_gpu = !a.flag("--cpu");
    if !use_gpu {
        makepad_ai_sing::nn::force_cpu(false);
    }
    let mut oc = OptConfig { total: steps, ..OptConfig::default() };
    // A resumed run starts its own warmup and cosine (set from the checkpoint's step below).
    oc.lr = a.num("--lr", oc.lr);
    oc.warmup = a.num("--warmup", (steps / 20).clamp(10, 2000));
    let mut cfg_kv: Vec<(String, String)> = Vec::new();
    cfg_kv.extend(ac.to_kv().into_iter().map(|(k, v)| (format!("ac.{k}"), v)));
    cfg_kv.extend(voc.to_kv().into_iter().map(|(k, v)| (format!("voc.{k}"), v)));

    match mode.as_str() {
        "t0" => {
            // Synthetic singer: the vocoder, then the acoustic model.
            let synth = Arc::new(SynthSinger::new(3));
            let mut rr = Rng::new(123);
            let reference = synth.example(&mut rr);
            let s2 = synth.clone();
            let rx = prefetch(workers, 8, 1, Arc::new(move |rng: &mut Rng| {
                let crops: Vec<_> = (0..batch).map(|_| loop {
                    let e = s2.example(rng);
                    if e.frames() >= frames {
                        let s = rng.below(e.frames() - frames + 1);
                        break (e.audio[s * HOP..(s + frames) * HOP].to_vec(), e.f0[s..s + frames].to_vec(), 24_000.0);
                    }
                }).collect();
                train::voc_batch(&crops, frames, rng.next_u64())
            }));
            let voc_c = voc.clone();
            let mut opt = if a.flag("--resume") && out.join("voc.mksing").exists() {
                Optimizer::load(&out.join("voc"), oc.clone()).unwrap()
            } else {
                Optimizer::new(declare(None, Some(&voc), 2), oc.clone())
            };
            let (acc, vc) = (ac.clone(), voc.clone());
            let refc = reference.clone();
            let outc = out.clone();
            run("voc", &mut opt, use_gpu, steps, log_every, save_every, &out, &cfg_kv, &rx, &|g, b: &VocBatch, _r| {
                let l = train::vocoder_loss(g, &voc_c, b);
                vec![("total", l.total), ("stft", l.stft), ("mel", l.mel)]
            }, &|o, step| sample_render(&outc, step, None, Some(&o.ema_params()), &acc, &vc, Some(&refc)));
            drop(rx);
            let voc_params = opt.ema_params();
            let s3 = synth.clone();
            let max_frames = a.num("--ac-frames", 600usize);
            let rx = prefetch(workers, 8, 2, Arc::new(move |rng: &mut Rng| {
                let items: Vec<Aligned> = (0..batch).map(|_| s3.example(rng).crop(max_frames, rng)).collect();
                train::ac_batch(&items)
            }));
            let mut opt = Optimizer::new(declare(Some(&ac), None, 3), oc.clone());
            let ac_c = ac.clone();
            let (acc, vc) = (ac.clone(), voc.clone());
            let outc = out.clone();
            run("ac", &mut opt, use_gpu, steps, log_every, save_every, &out, &cfg_kv, &rx, &|g, b: &AcBatch, r| {
                let l = train::acoustic_loss(g, &ac_c, b, r);
                vec![("total", l.total), ("mel", l.mel), ("dur", l.dur), ("f0", l.f0), ("voicing", l.voicing), ("flow", l.flow), ("prior", l.prior)]
            }, &|o, step| sample_render(&outc, step, Some(&o.ema_params()), Some(&voc_params), &acc, &vc, None));
        }
        "voc" => {
            let items = Arc::new(load_items(&a.all("--data")));
            let hours: f64 = items.iter().map(|i| i.audio.len() as f64).sum::<f64>() / SR as f64 / 3600.0;
            eprintln!("{} items, {hours:.1} h", items.len());
            let reference = items.iter().find(|i| i.kind == Kind::Sung && i.frames() > 400).or(items.first()).map(|i| {
                let t = i.frames().min(600);
                Aligned { tokens: vec![], dur: vec![], notes: vec![], f0: i.f0[..t].to_vec(), vel: vec![], audio: i.audio_f32()[..t * HOP].to_vec(), singer: 0, band: i.band_hz }
            });
            let it2 = items.clone();
            let rx = prefetch(workers, 8, 1, Arc::new(move |rng: &mut Rng| {
                let crops: Vec<_> = (0..batch).map(|_| voc_crop(&it2, frames, rng)).collect();
                train::voc_batch(&crops, frames, rng.next_u64())
            }));
            let mut opt = if a.flag("--resume") && out.join("voc.mksing").exists() {
                Optimizer::load(&out.join("voc"), oc.clone()).unwrap()
            } else {
                Optimizer::new(declare(None, Some(&voc), 2), oc.clone())
            };
            let voc_c = voc.clone();
            let (acc, vc) = (ac.clone(), voc.clone());
            let outc = out.clone();
            let save = |o: &Optimizer, step: usize| {
                // Samples from the generator's EMA only.
                let mut p = o.ema_params();
                let keep: Vec<usize> = (0..p.names.len()).filter(|i| !p.names[*i].starts_with("disc.")).collect();
                let mut q = Params::new();
                for i in keep {
                    q.insert(&p.names[i].clone(), (*p.vals[i]).clone());
                }
                p = q;
                sample_render(&outc, step, None, Some(&p), &acc, &vc, reference.as_ref())
            };
            if a.flag("--gan") {
                // Add the discriminators to a reconstruction-only checkpoint.
                if !opt.params.has("disc.r0.post.w") {
                    let mut rng = Rng::new(11);
                    let mut dp = Params::new();
                    makepad_ai_sing::disc::declare(&mut dp, &mut rng);
                    let mut o2 = Optimizer::new(opt.params.clone(), opt.cfg.clone());
                    o2.step = opt.step;
                    o2.m = opt.m.clone();
                    o2.v = opt.v.clone();
                    o2.ema = opt.ema.clone();
                    for (n, t) in dp.names.iter().zip(&dp.vals) {
                        o2.params.insert(n, (**t).clone());
                        o2.m.push(vec![0.0; t.len()]);
                        o2.v.push(vec![0.0; t.len()]);
                        o2.ema.push(t.data.clone());
                    }
                    opt = o2;
                }
                opt.cfg.b1 = 0.8;
                opt.cfg.b2 = 0.99;
                opt.cfg.lr = a.num("--lr", 2e-4);
                opt.cfg.start = opt.step;
                run_gan(&mut opt, use_gpu, steps, log_every, save_every, &out, &cfg_kv, &rx, &voc_c, &save);
            } else {
                run("voc", &mut opt, use_gpu, steps, log_every, save_every, &out, &cfg_kv, &rx, &|g, b: &VocBatch, _r| {
                    let l = train::vocoder_loss(g, &voc_c, b);
                    vec![("total", l.total), ("stft", l.stft), ("mel", l.mel)]
                }, &save);
            }
        }
        "ac" => {
            let items = load_items(&a.all("--data"));
            let max_frames = a.num("--ac-frames", 600usize);
            let sung: Vec<Aligned> = items.iter().filter_map(Aligned::from_vowel_item).collect();
            let speech: Vec<Aligned> = items.iter().filter_map(|i| Aligned::from_speech_item(i, a.num("--speech-frames", 1000usize))).collect();
            drop(items);
            let speech_frac: f32 = a.num("--speech-frac", if sung.is_empty() { 1.0 } else if speech.is_empty() { 0.0 } else { 0.7 });
            eprintln!("{} sung items, {} speech items; speech batches {:.0}%", sung.len(), speech.len(), speech_frac * 100.0);
            let (sung, speech) = (Arc::new(sung), Arc::new(speech));
            let rx = prefetch(workers, 8, 2, Arc::new(move |rng: &mut Rng| {
                // Batches are all speech (aligned on the fly) or all sung.
                let pool = if !speech.is_empty() && (sung.is_empty() || rng.unit() < speech_frac) { &speech } else { &sung };
                let v: Vec<Aligned> = (0..batch).map(|_| pool[rng.below(pool.len())].crop(max_frames, rng)).collect();
                train::ac_batch(&v)
            }));
            let mut opt = if a.flag("--resume") && out.join("ac.mksing").exists() {
                Optimizer::load(&out.join("ac"), oc.clone()).unwrap()
            } else {
                Optimizer::new(declare(Some(&ac), None, 3), oc.clone())
            };
            let voc_params = a.get("--vocoder").map(|p| makepad_ai_sing::weights::read(Path::new(&p)).unwrap().params);
            let ac_c = ac.clone();
            let (acc, vc) = (ac.clone(), voc.clone());
            let outc = out.clone();
            run("ac", &mut opt, use_gpu, steps, log_every, save_every, &out, &cfg_kv, &rx, &|g, b: &AcBatch, r| {
                let l = train::acoustic_loss(g, &ac_c, b, r);
                vec![("total", l.total), ("mel", l.mel), ("dur", l.dur), ("f0", l.f0), ("voicing", l.voicing), ("flow", l.flow), ("prior", l.prior)]
            }, &|o, step| sample_render(&outc, step, Some(&o.ema_params()), voc_params.as_ref(), &acc, &vc, None));
        }
        _ => {
            eprintln!("usage: sing_train check | t0 | voc | ac  (see the file header)");
            std::process::exit(2);
        }
    }
    gpu::release_pool();
}
