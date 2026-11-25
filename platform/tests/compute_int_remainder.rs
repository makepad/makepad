//! Integer remainders in compute kernels on the native GPU backend, against the CPU.
//!
//! Metal's shader compiler took the remainder of a word's high half by a constant
//! (`(x >> 16u) % 10u`) with the quotient of the word's low half: on an M-series GPU it answered
//! `hi - 10 * (lo / 10)`, so packed cell coordinates (`gx | gy << 16`) went to the wrong row
//! of the wrong cell. It did so for signed and vector remainders too, and for a divisor that is
//! a constant only once `let`s are folded. Vulkan on an NVIDIA GPU took a signed remainder of a
//! negative operand as the unsigned remainder of its bits. Every form here must give the CPU's
//! bits on every backend.
#![cfg(not(target_arch = "wasm32"))]
use makepad_platform::{*, makepad_hal as hal, thread::Class};
use std::{sync::mpsc, time::{Duration, Instant}};

/// A form: its name, the kernel's expression of it, and the CPU's answer from the input word.
/// The kernel has `x`, `hi` = `x >> 16u`, `lo` = `x & 65535u`, `d` = `10u`, `r` = `hi %= 7u`,
/// `v` = `vec2u(hi, lo) %= 10u` and the uniform `self.divisor` = 10.
type Form = (&'static str, &'static str, fn(u32) -> u32);

const FORMS: &[Form] = &[
    ("hi % 10", "hi % 10u", |x| (x >> 16) % 10),
    ("hi % 7", "hi % 7u", |x| (x >> 16) % 7),
    ("hi % 1000", "hi % 1000u", |x| (x >> 16) % 1000),
    ("hi % 16", "hi % 16u", |x| (x >> 16) % 16),
    ("(x / 65536) % 10", "(x / 65536u) % 10u", |x| (x / 65536) % 10),
    ("hi %= 7", "r", |x| (x >> 16) % 7),
    ("let d = 10, hi % d", "hi % d", |x| (x >> 16) % 10),
    ("hi % divisor", "hi % self.divisor", |x| (x >> 16) % 10),
    ("lo % 10", "lo % 10u", |x| (x & 65535) % 10),
    ("x % 10", "x % 10u", |x| x % 10),
    ("x % 7", "x % 7u", |x| x % 7),
    ("signed hi % 10", "u32(i32(hi) % i32(10u))", |x| (x >> 16) % 10),
    ("signed (hi - 32768) % 10", "u32((i32(hi) - i32(32768u)) % i32(10u))", |x| (((x >> 16) as i32 - 32768) % 10) as u32),
    ("signed (hi - 32768) % -7", "u32((i32(hi) - i32(32768u)) % (i32(0u) - i32(7u)))", |x| (((x >> 16) as i32 - 32768) % -7) as u32),
    ("vec2(hi, hi) % 10", "(vec2u(hi, hi) % vec2u(10u, 10u)).y", |x| (x >> 16) % 10),
    ("vec2(hi, lo) % vec2(10, 6)", "(vec2u(hi, lo) % vec2u(10u, 6u)).x", |x| (x >> 16) % 10),
    ("vec3(hi, lo, x) % 10", "(vec3u(hi, lo, x) % 10u).x", |x| (x >> 16) % 10),
    ("vec4(x, x, hi, hi) % 7", "(vec4u(x, x, hi, hi) % vec4u(7u, 7u, 7u, 7u)).w", |x| (x >> 16) % 7),
    ("vec2(hi, lo) %= 10", "v.x", |x| (x >> 16) % 10),
    ("hi / 10", "hi / 10u", |x| (x >> 16) / 10),
    ("hi / 7", "hi / 7u", |x| (x >> 16) / 7),
    ("vec2(hi, lo) / vec2(10, 6)", "(vec2u(hi, lo) / vec2u(10u, 6u)).x", |x| (x >> 16) / 10),
    ("signed (hi - 32768) / 10", "u32((i32(hi) - i32(32768u)) / i32(10u))", |x| (((x >> 16) as i32 - 32768) / 10) as u32),
    ("signed (hi - 32768) / -7", "u32((i32(hi) - i32(32768u)) / (i32(0u) - i32(7u)))", |x| (((x >> 16) as i32 - 32768) / -7) as u32),
    ("ivec2(hi - 32768, lo) % 10", "u32((vec2i(i32(hi) - i32(32768u), i32(lo)) % vec2i(10, 10)).x)", |x| (((x >> 16) as i32 - 32768) % 10) as u32),
];

#[test]
fn integer_remainders_match_the_cpu() {
    let backend = if cfg!(target_vendor = "apple") { hal::backend_id::METAL }
        else if cfg!(windows) { hal::backend_id::D3D11 } else { hal::backend_id::VULKAN };
    let info = hal::hal_open(&hal::OpenReq { backend, flags: hal::open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend, "requires the native GPU backend");
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.hal_attach_session(info);
    // low halves that do and do not share their tens with the high half, high halves past i16,
    // whole words at the ends of the range
    let mut input = vec![u32::MAX, u32::MAX - 1, 0x8000_0000, 0x7fff_ffff];
    for hi in [0u32, 5, 15, 25, 35, 55, 1945, 40000, 65535] {
        for lo in [0u32, 3, 16, 20, 42, 59, 60, 65535] { input.push(lo | hi << 16); }
    }
    let (n, words) = (input.len() as u32, FORMS.len() as u32);
    // each form in a kernel of its own: beside the others the compiler shares their work, and a
    // vector remainder that is wrong alone came out right
    let kernels: Vec<ComputeKernel> = FORMS.iter().enumerate().map(|(k, (_, expr, _))| cx.with_vm(|vm| {
        vm.capture_errors();
        let value = vm.eval(makepad_script::ScriptMod::new("", "", "int_remainder", 0, 0, &format!(
            "use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{{workgroup_size: 64\ndivisor: shader.uniform(u32)\nsrc: shader.buffer(u32)\ndst: shader.buffer_rw(u32)\n\
             main: fn() {{ let i = global_id().x\nif i >= {n}u {{ return }}\nlet x = self.src[i]\nlet hi = x >> 16u\nlet lo = x & 65535u\nlet d = 10u\n\
             var r = hi\nr %= 7u\nvar v = vec2u(hi, lo)\nv %= 10u\nself.dst[i * {words}u + {k}u] = {expr} }}}}"), &[]));
        let kernel = ComputeKernel::new(vm, value.as_object().unwrap(), "int_remainder").unwrap();
        assert!(vm.take_errors().is_empty());
        kernel
    })).collect();
    let src = GpuBuffer::new(&mut cx, n as u64 * 4, Default::default()).unwrap();
    src.update(&mut cx, 0, &input.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>()).unwrap();
    let dst = GpuBuffer::new(&mut cx, (n * words) as u64 * 4, Default::default()).unwrap();
    let mut pass = ComputePass::new("integer remainders", None);
    for kernel in &kernels {
        let mut params = kernel.params();
        // a kernel that does not read the divisor has no such parameter
        let _ = params.set_u32(id!(divisor), 10);
        let buffers: Vec<GpuBuffer> = kernel.layout().buffers.iter().map(|b| if b.name == id!(src) { src.clone() } else { dst.clone() }).collect();
        pass.dispatch(kernel, [n.div_ceil(64), 1, 1], &params, &buffers, &[]).unwrap();
    }
    pass.submit(&mut cx);
    let (tx, rx) = mpsc::channel();
    cx.read_to_pool(&[(&dst, 0..dst.size())], Class::Interactive, move |result| { tx.send(result).unwrap(); }).unwrap();
    let start = Instant::now();
    let got: Vec<u32> = loop {
        assert!(start.elapsed() < Duration::from_secs(10), "readback deadline");
        cx.hal_session_turn(1_000_000, true);
        if let Ok(result) = rx.try_recv() { break result.unwrap()[0].chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect(); }
    };
    cx.hal_close_session();
    let broken: Vec<String> = FORMS.iter().enumerate().filter_map(|(k, (name, _, cpu))| {
        let wrong: Vec<String> = input.iter().enumerate().filter_map(|(i, &x)| {
            let (at, want) = (got[i * FORMS.len() + k], cpu(x));
            (at != want).then(|| format!("(hi {}, lo {}): {at}, want {want}", x >> 16, x & 65535))
        }).collect();
        (!wrong.is_empty()).then(|| format!("{name}: {} of {n} wrong, first {:?}", wrong.len(), &wrong[..wrong.len().min(3)]))
    }).collect();
    assert!(broken.is_empty(), "integer forms that differ from the CPU:\n{}", broken.join("\n"));
}
