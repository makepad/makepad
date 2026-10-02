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
//! ac:   --data DIR... --mix lyric=..,speech=..,sung=.. --holdout P --augment P
//!       --dropout P --init W.mksing --keep --patience N
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
        Some("small") => (AcousticConfig::small(), VocoderConfig::base()),
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
/// Add the discriminators (fresh) to a checkpoint that has none.
fn add_disc(opt: &mut Optimizer) {
    if opt.params.has("disc.r0.post.w") {
        return;
    }
    let mut rng = Rng::new(11);
    let mut dp = Params::new();
    makepad_ai_sing::disc::declare(&mut dp, &mut rng);
    for (n, t) in dp.names.iter().zip(&dp.vals) {
        opt.params.insert(n, (**t).clone());
        opt.m.push(vec![0.0; t.len()]);
        opt.v.push(vec![0.0; t.len()]);
        opt.ema.push(t.data.clone());
    }
}

fn fresh_schedule(opt: &mut Optimizer) {
    opt.cfg.start = opt.step;
}

/// A held-out loss (evaluated at log steps) and how many log steps without
/// improvement end the round early; the best weights are saved as `<name>-best`.
type Val<'a> = Option<(&'a dyn Fn(&Optimizer) -> f32, usize)>;

#[allow(clippy::too_many_arguments)]
fn run<B>(
    name: &str, opt: &mut Optimizer, use_gpu: bool, steps: usize, log_every: usize, save_every: usize, out: &Path,
    config: &[(String, String)], rx: &Receiver<B>, loss: &dyn Fn(&mut Graph, &B, &mut Rng) -> Vec<(&'static str, usize)>,
    on_save: &dyn Fn(&Optimizer, usize), val: Val,
) {
    let mut best = f32::INFINITY;
    let mut since_best = 0usize;
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
            if let Some((vf, patience)) = val {
                let v = vf(opt);
                if v < best {
                    best = v;
                    since_best = 0;
                    #[cfg(any(target_os = "linux", target_os = "windows"))]
                    opt.sync_host();
                    opt.save(&out.join(format!("{name}-best")), config).expect("best checkpoint");
                } else {
                    since_best += 1;
                }
                eprintln!("[{name}] step {} | held-out {v:.4} (best {best:.4}, {since_best} logs without improvement)", opt.step);
                if since_best >= patience {
                    eprintln!("[{name}] early stop at step {}: held-out loss has not improved for {patience} logs", opt.step);
                    #[cfg(any(target_os = "linux", target_os = "windows"))]
                    opt.sync_host();
                    opt.save(&out.join(name), config).expect("checkpoint");
                    on_save(opt, opt.step);
                    return;
                }
            }
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
fn run_gan<B>(
    opt: &mut Optimizer, use_gpu: bool, steps: usize, log_every: usize, save_every: usize, out: &Path,
    config: &[(String, String)], rx: &Receiver<B>, g_loss: &dyn Fn(&mut Graph, &B, &mut Rng) -> train::GanLosses,
    on_save: &dyn Fn(&Optimizer, usize),
) {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    if use_gpu {
        opt.to_device();
        gpu::staging_init(512 << 20);
    }
    let _ = use_gpu;
    fresh_schedule(opt);
    let is_d: Vec<bool> = opt.params.names.iter().map(|n| n.starts_with("disc.")).collect();
    // A frozen acoustic model (V2) is in the graph but never updated.
    let frozen: Vec<bool> = opt.params.names.iter().map(|n| n.starts_with("ac.")).collect();
    let mut rng = Rng::new(opt.step as u64 + 29);
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
            let l = g_loss(&mut g, &batch, &mut rng);
            let mut grads = g.backward(l.total);
            for (i, d) in is_d.iter().enumerate() {
                if *d || frozen[i] {
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
        let (cents, gross, frames) = makepad_ai_sing::cantor::f0_error_stats(&audio, &r.f0);
        // Mel L1 of the copy against the reference.
        let mut g2 = Graph::new(&c.params, false);
        let a = train::target_mel(&mut g2, &audio, 1, t);
        let b = train::target_mel(&mut g2, &r.audio[..t * HOP], 1, t);
        let l = g2.l1_loss(a, b, None, None);
        let mel_l1 = g2.host(l)[0] * acoustic::MEL_STD;
        write_wav(&dir.join(format!("copy-{step:07}.wav")), &audio);
        write_wav(&dir.join("reference.wav"), &r.audio);
        eprintln!("[sample] step {step}: vocoder copy-synthesis f0 error {cents:.2} cents ({:.1}% gross) over {frames} frames, log-mel L1 {mel_l1:.3}", gross * 100.0);
    }
    if ac_p.is_some() {
        c.durations_trained = true;
        let line = makepad_ai_sing::score::simple_line(
            &[(60.0, 1.0, "lɑ"), (62.0, 1.0, "mi"), (64.0, 2.0, "soʊ"), (62.0, 1.0, "_"), (60.0, 2.0, "dus")],
            96.0,
        );
        let (audio, phrases) = c.render(&line, &makepad_ai_sing::RenderOpts::default());
        for p in &phrases {
            let (cents, gross, n) = makepad_ai_sing::cantor::f0_error_stats(&p.audio, &p.frames.f0);
            eprintln!("[sample] step {step}: sung line f0 error {cents:.2} cents ({:.1}% gross) over {n} frames", gross * 100.0);
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
                // Each file's own section of the config (a small acoustic model with
                // a base vocoder); the other section only where nothing set it yet.
                let own = if key == "--ac" { "ac." } else { "voc." };
                for (k, v) in &w.config {
                    let set = cfg.iter().position(|(k2, _)| k2 == k);
                    match set {
                        Some(i) if k.starts_with(own) => cfg[i].1 = v.clone(),
                        None => cfg.push((k.clone(), v.clone())),
                        _ => {}
                    }
                }
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
            }, &|o, step| sample_render(&outc, step, None, Some(&o.ema_params()), &acc, &vc, Some(&refc)), None);
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
            }, &|o, step| sample_render(&outc, step, Some(&o.ema_params()), Some(&voc_params), &acc, &vc, None), None);
        }
        "voc" => {
            let store = Arc::new(data::Store::open(&a.all("--data")).expect("data"));
            eprintln!("{} items, {:.1} h", store.items.len(), store.hours());
            let reference = store.items.iter().find(|r| r.kind == Kind::Sung && r.frames > 400).or(store.items.first()).map(|r| {
                let t = (r.frames as usize).min(600);
                Aligned { tokens: vec![], dur: vec![], notes: vec![], f0: store.f0(r, 0, t), vel: vec![], audio: store.audio(r, 0, t), singer: 0, band: r.band_hz }
            });
            let long: Vec<usize> = (0..store.items.len()).filter(|i| store.items[*i].frames as usize >= frames + 2).collect();
            let st2 = store.clone();
            let rx = prefetch(workers, 8, 1, Arc::new(move |rng: &mut Rng| {
                let crops: Vec<_> = (0..batch).map(|_| loop {
                    let r = &st2.items[long[rng.below(long.len())]];
                    let s = rng.below(r.frames as usize - frames);
                    let audio = st2.audio(r, s, frames);
                    let rms = (audio.iter().map(|v| v * v).sum::<f32>() / audio.len().max(1) as f32).sqrt();
                    if rms < 1e-3 || audio.len() < frames * HOP {
                        continue;
                    }
                    break (audio, st2.f0(r, s, frames), r.band_hz);
                }).collect();
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
                add_disc(&mut opt);
                opt.cfg.b1 = 0.8;
                opt.cfg.b2 = 0.99;
                opt.cfg.lr = a.num("--lr", 2e-4);
                opt.cfg.start = opt.step;
                run_gan(&mut opt, use_gpu, steps, log_every, save_every, &out, &cfg_kv, &rx, &|g, b: &VocBatch, _r| train::vocoder_gan_loss(g, &voc_c, b), &save);
            } else {
                run("voc", &mut opt, use_gpu, steps, log_every, save_every, &out, &cfg_kv, &rx, &|g, b: &VocBatch, _r| {
                    let l = train::vocoder_loss(g, &voc_c, b);
                    vec![("total", l.total), ("stft", l.stft), ("mel", l.mel)]
                }, &save, None);
            }
        }
        "gta" => {
            // V2: sing_train gta --data DIR --acoustic A.ema.mksing --resume-from VOC_STEM [--holdout P]
            let store = Arc::new(data::Store::open(&a.all("--data")).expect("data"));
            let max_frames = a.num("--ac-frames", 400usize);
            // Sung vowels and score-aligned sung lyrics (--holdout P keeps the held-out songs out).
            let holdout: u64 = a.num("--holdout", 0u64);
            let sung: Vec<Aligned> = store
                .items
                .iter()
                .filter(|r| !(r.kind == Kind::SungAligned && store.held_out(r, holdout)))
                .filter_map(|r| match r.kind {
                    Kind::Sung => Aligned::from_vowel_item(&store.item(r)),
                    Kind::SungAligned => Aligned::from_aligned_item(&store.item(r), 100_000),
                    _ => None,
                })
                .collect();
            eprintln!("{} sung items for GTA", sung.len());
            let sung = Arc::new(sung);
            let rx = prefetch(workers, 8, 3, Arc::new(move |rng: &mut Rng| {
                let v: Vec<Aligned> = (0..batch).map(|_| sung[rng.below(sung.len())].crop(max_frames, rng)).collect();
                train::ac_batch(&v)
            }));
            let from = PathBuf::from(a.get("--resume-from").expect("--resume-from <vocoder checkpoint stem>"));
            let mut opt = Optimizer::load(&from, oc.clone()).expect("vocoder checkpoint");
            add_disc(&mut opt);
            let acw = makepad_ai_sing::weights::read(Path::new(&a.get("--acoustic").expect("--acoustic"))).unwrap();
            for (n, t) in acw.params.names.iter().zip(&acw.params.vals) {
                if n.starts_with("ac.") && !opt.params.has(n) {
                    opt.params.insert(n, (**t).clone());
                    opt.m.push(vec![0.0; t.len()]);
                    opt.v.push(vec![0.0; t.len()]);
                    opt.ema.push(t.data.clone());
                }
            }
            opt.cfg.b1 = 0.8;
            opt.cfg.b2 = 0.99;
            opt.cfg.lr = a.num("--lr", 1e-4);
            let (acc, vc) = (ac.clone(), voc.clone());
            let outc = out.clone();
            let save = |o: &Optimizer, step: usize| {
                let p = o.ema_params();
                let mut q = Params::new();
                for (n, t) in p.names.iter().zip(&p.vals) {
                    if n.starts_with("voc.") {
                        q.insert(n, (**t).clone());
                    }
                }
                let mut ap = Params::new();
                for (n, t) in p.names.iter().zip(&p.vals) {
                    if n.starts_with("ac.") {
                        ap.insert(n, (**t).clone());
                    }
                }
                sample_render(&outc, step, Some(&ap), Some(&q), &acc, &vc, None)
            };
            run_gan(&mut opt, use_gpu, steps, log_every, save_every, &out, &cfg_kv, &rx, &|g, b: &AcBatch, r| train::vocoder_gta_loss(g, &ac, &voc, b, r), &save);
        }
        "ac" => {
            let store = Arc::new(data::Store::open(&a.all("--data")).expect("data"));
            let max_frames = a.num("--ac-frames", 600usize);
            let speech_frames = a.num("--speech-frames", 1000usize);
            let sung: Vec<Aligned> = store.items.iter().filter(|r| r.kind == Kind::Sung).filter_map(|r| Aligned::from_vowel_item(&store.item(r))).collect();
            let pick = |kind: Kind| -> Vec<usize> {
                (0..store.items.len())
                    .filter(|i| {
                        let r = &store.items[*i];
                        r.kind == kind && r.frames as usize <= speech_frames && r.frames >= 2 * r.tokens
                    })
                    .collect()
            };
            // Sung lyrics: score-aligned items, and text-only ones aligned in training (a
            // batch holding any of those aligns all of it by alignment search).
            let (speech, mut lyric) = (pick(Kind::Speech), [pick(Kind::SungAligned), pick(Kind::SungText)].concat());
            // --holdout P: whole songs (shards) held out of training, P% by hash; their
            // segments score each log step (--patience logs without improvement stop the round).
            let holdout: u64 = a.num("--holdout", 0u64);
            let held: Vec<usize> = lyric.iter().copied().filter(|i| store.held_out(&store.items[*i], holdout)).collect();
            lyric.retain(|i| !held.contains(i));
            if holdout > 0 {
                eprintln!("held out {} sung-lyric segments from {}% of the songs", held.len(), holdout);
            }
            // --mix lyric=0.5,speech=0.25,sung=0.25 (shares of batches; empty pools drop out).
            let mut mix = [("lyric", 0.0f32), ("speech", 0.0), ("sung", 0.0)];
            match a.get("--mix") {
                Some(m) => {
                    for part in m.split(',') {
                        if let Some((k, v)) = part.split_once('=') {
                            if let Some(e) = mix.iter_mut().find(|e| e.0 == k) {
                                e.1 = v.parse().unwrap_or(0.0);
                            }
                        }
                    }
                }
                None => {
                    let sf: f32 = a.num("--speech-frac", if sung.is_empty() { 1.0 } else if speech.is_empty() { 0.0 } else { 0.7 });
                    mix = [("lyric", 0.0), ("speech", sf), ("sung", 1.0 - sf)];
                }
            }
            for (e, n) in mix.iter_mut().zip([lyric.len(), speech.len(), sung.len()]) {
                if n == 0 {
                    e.1 = 0.0;
                }
            }
            let total: f32 = mix.iter().map(|e| e.1).sum::<f32>().max(1e-6);
            eprintln!(
                "{} sung-lyric, {} speech, {} sung-vowel items ({:.1} h in the store); batch shares lyric {:.0}% speech {:.0}% sung {:.0}%",
                lyric.len(), speech.len(), sung.len(), store.hours(), mix[0].1 / total * 100.0, mix[1].1 / total * 100.0, mix[2].1 / total * 100.0
            );
            let sung = Arc::new(sung);
            let augment: f32 = a.num("--augment", 0.0f32);
            let st2 = store.clone();
            let rx = prefetch(workers, 8, 2, Arc::new(move |rng: &mut Rng| {
                // Each batch is one kind: unaligned (lyric, speech; aligned on the fly) or sung vowels.
                let mut x = rng.unit() * total;
                let mut kind = 2;
                for (k, e) in mix.iter().enumerate() {
                    if x < e.1 {
                        kind = k;
                        break;
                    }
                    x -= e.1;
                }
                let v: Vec<Aligned> = match kind {
                    0 => (0..batch)
                        .map(|_| {
                            let a = Aligned::from_lyric_item(&st2.item(&st2.items[lyric[rng.below(lyric.len())]]), speech_frames).unwrap();
                            // --augment P: speed perturbation on P of the segments, at one of
                            // nine speeds k/40 (0.9..1.1): 48 kHz·k/40 shares a large factor
                            // with 48 kHz, so the resampler runs its 40-phase table (~3 ms
                            // per 3 s segment, against ~125 ms at an arbitrary ratio).
                            if rng.unit() < augment { a.speed((36 + rng.below(9)) as f32 / 40.0) } else { a }
                        })
                        .collect(),
                    1 => (0..batch).map(|_| Aligned::from_speech_item(&st2.item(&st2.items[speech[rng.below(speech.len())]]), speech_frames).unwrap()).collect(),
                    _ => (0..batch).map(|_| sung[rng.below(sung.len())].crop(max_frames, rng)).collect(),
                };
                train::ac_batch(&v)
            }));
            let mut opt = if a.flag("--resume") && out.join("ac.mksing").exists() {
                Optimizer::load(&out.join("ac"), oc.clone()).unwrap()
            } else {
                let mut p = declare(Some(&ac), None, 3);
                // --init W.mksing: start from another run's acoustic weights (same
                // config), with a fresh optimiser and schedule.
                if let Some(init) = a.get("--init") {
                    let w = makepad_ai_sing::weights::read(Path::new(&init)).unwrap_or_else(|e| panic!("{init}: {e}"));
                    let mut n = 0;
                    for (name, t) in w.params.names.iter().zip(&w.params.vals) {
                        if name.starts_with("ac.") && p.has(name) && (p.get(name).rows, p.get(name).cols) == (t.rows, t.cols) {
                            p.insert(name, (**t).clone());
                            n += 1;
                        }
                    }
                    eprintln!("{n} of {} acoustic tensors from {init}", p.names.len());
                }
                Optimizer::new(p, oc.clone())
            };
            let voc_params = a.get("--vocoder").map(|p| makepad_ai_sing::weights::read(Path::new(&p)).unwrap().params);
            let val_batch = (!held.is_empty()).then(|| {
                let v: Vec<Aligned> = held.iter().take(64).filter_map(|i| Aligned::from_lyric_item(&store.item(&store.items[*i]), speech_frames)).collect();
                train::ac_batch(&v)
            });
            let ac_v = ac.clone();
            let val_fn = move |o: &Optimizer| -> f32 {
                let Some(b) = &val_batch else { return 0.0 };
                #[cfg(any(target_os = "linux", target_os = "windows"))]
                let mut g = match &o.dev {
                    Some(d) => Graph::new_gpu(&o.params, &d.params, false),
                    None => Graph::new(&o.params, false),
                };
                #[cfg(not(any(target_os = "linux", target_os = "windows")))]
                let mut g = Graph::new(&o.params, false);
                let l = train::acoustic_loss(&mut g, &ac_v, b, &mut Rng::new(1));
                g.host(l.mel)[0]
            };
            let patience = a.num("--patience", 4usize);
            let val: Val = if held.is_empty() { None } else { Some((&val_fn, patience)) };
            let keep = a.flag("--keep");
            let ac_c = ac.clone();
            let (acc, vc) = (ac.clone(), voc.clone());
            let outc = out.clone();
            // --dropout P: Gaussian dropout on every residual branch of the encoder and decoder.
            let dropout: f32 = a.num("--dropout", 0.0f32);
            run("ac", &mut opt, use_gpu, steps, log_every, save_every, &out, &cfg_kv, &rx, &|g, b: &AcBatch, r| {
                g.dropout = dropout;
                g.noise_seed = r.next_u64();
                let l = train::acoustic_loss(g, &ac_c, b, r);
                vec![("total", l.total), ("mel", l.mel), ("dur", l.dur), ("f0", l.f0), ("voicing", l.voicing), ("flow", l.flow), ("prior", l.prior)]
            }, &|o, step| {
                // --keep: every save also leaves the EMA weights as ac-<step>.ema.mksing.
                if keep {
                    let mut kv = cfg_kv.clone();
                    kv.push(("step".into(), step.to_string()));
                    makepad_ai_sing::weights::write(&outc.join(format!("ac-{step:07}.ema.mksing")), &kv, &o.ema_params(), makepad_ai_sing::weights::Dtype::F32).expect("kept checkpoint");
                }
                sample_render(&outc, step, Some(&o.ema_params()), voc_params.as_ref(), &acc, &vc, None)
            }, val);
        }
        _ => {
            eprintln!("usage: sing_train check | t0 | voc | ac  (see the file header)");
            std::process::exit(2);
        }
    }
    gpu::release_pool();
}
