//! `program-gpu-checks [dispatches]`: what the GPU device's design rests on, measured on this
//! machine's GPU. The device runs a program's op as one dispatch per range of about 1 ms, each
//! with its own params (its range), in passes outside any window, and wakes the program on a
//! readback. So, with an element kernel (`y[i] = y[i] + x[i]`) through the route the device
//! uses (`ElementRoute`, `ComputeKernel::from_compiled`):
//! - record: `dispatches` (default 10000) one-workgroup dispatches, each its own range, into one
//!   unattached pass: the CPU's time a dispatch;
//! - the pass: from its submit to the readback after it, the session's turns (encode, submit)
//!   and the GPU's time (its pass timer), against one dispatch of the same elements (and that
//!   dispatch's own timer);
//! - readback: 4 bytes read with no compute queued, from the ask to the bytes (median of 20);
//! - the answer: every element added to once per pass.

use makepad_compute_program_gpu::GpuSession;
use makepad_platform::makepad_script::shader_ir_compute::CompiledKernel;
use makepad_platform::makepad_script::*;
use makepad_platform::{gpu_frame_timer, ComputeKernel, ComputeParams, ComputePass, GpuBuffer, GpuBufferFlags};
use makepad_script_compute::element_route::{ElementRoute, ELEMENT_ARGS, ELEMENT_WORKGROUP};
use makepad_script_compute::lower::kernel::K_COUNT;
use makepad_script_compute::{vm_kernel, Backend};
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn die(m: &str) -> ! {
    eprintln!("program-gpu-checks: {m}");
    std::process::exit(2)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// The element kernel, compiled for `session`'s GPU, its context words and its host buffers'
/// names (by the kernel's index: the control word's too).
fn kernel(session: &mut GpuSession) -> (ComputeKernel, usize, Vec<String>) {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    vm_kernel::script_mod(&mut vm, &[]);
    let source = "use mod.pod.*\nuse mod.kernel.*\n{label: \"add\" x: input(f32) y: output(f32) element: fn(i) { y[i] = y[i] + x[i] }}";
    let v = vm.eval(ScriptMod::new("", "", "checks.splash", 0, 0, source, &[]));
    let errors = vm.take_errors();
    if !errors.is_empty() {
        die(&format!("the kernel: {errors:?}"));
    }
    let obj = v.as_object().unwrap_or_else(|| die("the kernel is not an object"));
    let _keep = vm.new_object_ref(obj);
    let k = vm_kernel::from_object(&vm, obj).unwrap_or_else(|e| die(&format!("{e:?}")));
    let compiled = vm_kernel::compile(&vm, &k, &[], Backend::Interp, &[]).unwrap_or_else(|e| die(&format!("{e:?}")));
    let target = session.cx.compute_target().unwrap_or_else(|e| die(&e));
    let gpu: CompiledKernel = ElementRoute::new(&compiled, 64).compile(target).view().to_kernel().unwrap_or_else(|e| die(&e));
    let kernel = ComputeKernel::from_compiled(&mut session.cx, gpu, "add").unwrap_or_else(|e| die(&e));
    (kernel, compiled.ctx_words(), compiled.buffer_views().map(|b| b.name.to_string()).collect())
}

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// `buffer`'s bytes `offset..offset + len` once the queued compute that binds it ran, turning
/// the session until they come: the bytes, and ms from the ask.
fn read(session: &mut GpuSession, buffer: &GpuBuffer, offset: u64, len: u64) -> (Vec<u8>, f64) {
    let got: Arc<Mutex<Option<(Instant, Vec<u8>)>>> = Arc::default();
    let into = got.clone();
    let t = Instant::now();
    session.cx.read(&[(buffer, offset..offset + len)], move |_, r| {
        let bytes = r.map(|mut ranges| ranges.remove(0).to_vec()).unwrap_or_default();
        *into.lock().unwrap() = Some((Instant::now(), bytes));
    }).unwrap_or_else(|e| die(&format!("read: {e:?}")));
    loop {
        if let Some((at, bytes)) = got.lock().unwrap().take() {
            return (bytes, (at - t).as_secs_f64() * 1e3);
        }
        if !session.turn() {
            die("the GPU session ended");
        }
    }
}

fn f32_at(bytes: &[u8], k: usize) -> f32 {
    f32::from_le_bytes(bytes[4 * k..4 * k + 4].try_into().unwrap())
}

fn median(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(f64::total_cmp);
    xs[xs.len() / 2]
}

fn main() {
    let dispatches: u32 = std::env::args().nth(1).map_or(10_000, |a| a.parse().unwrap_or_else(|_| die("dispatches: a count")));
    let mut session = GpuSession::open().unwrap_or_else(|e| die(&e));
    let (kernel, ctx_words, names) = kernel(&mut session);
    let n = dispatches * ELEMENT_WORKGROUP;
    let cx = &mut session.cx;
    let readable = GpuBufferFlags { indirect: false, readback: true };
    let x = GpuBuffer::new(cx, n as u64 * 4, GpuBufferFlags::default()).unwrap_or_else(|e| die(&e));
    let y = GpuBuffer::new(cx, n as u64 * 4, readable).unwrap_or_else(|e| die(&e));
    x.update(cx, 0, &bytes(&vec![1.0f32.to_bits(); n as usize])).unwrap_or_else(|e| die(&e));
    y.update(cx, 0, &bytes(&vec![0; n as usize])).unwrap_or_else(|e| die(&e));
    // the run's context words (its count), then each host buffer's length in words by the
    // kernel's index (the control word's reads 0)
    let mut words = vec![0u32; ctx_words];
    words[K_COUNT as usize] = n;
    words.extend(names.iter().map(|name| if name == "x" || name == "y" { n } else { 0 }));
    let args = GpuBuffer::new(cx, words.len() as u64 * 4, GpuBufferFlags::default()).unwrap_or_else(|e| die(&e));
    args.update(cx, 0, &bytes(&words)).unwrap_or_else(|e| die(&e));
    let buffers: Vec<GpuBuffer> = kernel
        .layout()
        .buffers
        .iter()
        .map(|b| match b.name {
            n if n == id!(x) => x.clone(),
            n if n == id!(y) => y.clone(),
            n if n == LiveId::from_str(ELEMENT_ARGS) => args.clone(),
            other => die(&format!("an unknown buffer {other}")),
        })
        .collect();
    let range = |params: &mut ComputeParams, first: u32, count: u32| {
        // a uvec4: its first element and count, two words unused
        params.set(id!(dispatch_range), &bytes(&[first, count, 0, 0])).unwrap_or_else(|e| die(&e));
    };
    println!("program-gpu-checks: {} {}, {dispatches} dispatches of {ELEMENT_WORKGROUP} elements", std::env::consts::OS, std::env::consts::ARCH);
    // the kernel's pipeline is built on the first pass: one small pass first, so the passes
    // timed below run on a built pipeline
    let mut params = kernel.params();
    range(&mut params, 0, 0);
    let mut warm = ComputePass::new("checks warm", None);
    warm.dispatch(&kernel, [1, 1, 1], &params, &buffers, &[]).unwrap_or_else(|e| die(&e));
    warm.submit(&mut session.cx);
    read(&mut session, &y, 0, 4);
    for (name, many) in [("checks many", true), ("checks one", false)] {
        gpu_frame_timer::watch(name);
        let mut pass = ComputePass::new(name, None);
        let t = Instant::now();
        if many {
            for k in 0..dispatches {
                range(&mut params, k * ELEMENT_WORKGROUP, ELEMENT_WORKGROUP);
                pass.dispatch(&kernel, [1, 1, 1], &params, &buffers, &[]).unwrap_or_else(|e| die(&e));
            }
        } else {
            range(&mut params, 0, n);
            pass.dispatch_timed("checks one dispatch", &kernel, [dispatches, 1, 1], &params, &buffers, &[]).unwrap_or_else(|e| die(&e));
        }
        let record = ms(t);
        let t = Instant::now();
        pass.submit(&mut session.cx);
        let (_, after) = read(&mut session, &y, 0, 4);
        let wall = ms(t);
        // the timers come with the frame's events: a few more turns
        let mut timed = Vec::new();
        for _ in 0..50 {
            timed = gpu_frame_timer::take_watched(name);
            if !timed.is_empty() {
                break;
            }
            session.turn();
        }
        let gpu = timed.first().map_or("no pass timer".to_string(), |ms| format!("GPU {ms:.3} ms (pass timer)"));
        let own = match many {
            true => String::new(),
            false => gpu_frame_timer::take_watched("checks one dispatch").first().map_or(", no dispatch timer".to_string(), |ms| format!(", the dispatch's own timer {ms:.3} ms")),
        };
        println!(
            "{:<12} record {record:>8.3} ms ({:.2} us a dispatch)  submit to its readback {wall:>8.3} ms (the read {after:.3} ms of it)  {gpu}{own}",
            if many { "many" } else { "one" },
            record * 1e3 / if many { dispatches as f64 } else { 1.0 }
        );
    }
    let reads: Vec<f64> = (0..20).map(|_| read(&mut session, &y, 0, 4).1).collect();
    println!("readback with no compute queued: median {:.3} ms (min {:.3}, max {:.3})", median(reads.clone()), reads.iter().cloned().fold(f64::MAX, f64::min), reads.iter().cloned().fold(0.0, f64::max));
    let (all, _) = read(&mut session, &y, 0, n as u64 * 4);
    let wrong = (0..n as usize).filter(|&k| f32_at(&all, k) != 2.0).count();
    match wrong {
        0 => println!("answer: every element added to once a pass"),
        w => println!("answer: {w} of {n} elements are not 2 (first {})", f32_at(&all, 0)),
    }
}
