//! Pipelines: passes chained by buffer name, ping-pong repeats, and
//! emitted records compacted into a draw shader's reflected layout, the same
//! on one thread and on the pool.

use makepad_platform::draw_shader_layout::{Layout as ShaderLayout, LayoutField, LayoutKind, LayoutPacking, PodType};
use makepad_platform::makepad_live_id::LiveId;
use makepad_render_kernels::compute::sched::{InlineExecutor, Priority};
use makepad_render_kernels::pipeline::Count;
use makepad_render_kernels::{engine, kernel_layout, ManualFences, OutputRing, Pipeline, PipelineError, Topology};

const HEIGHTS: &str = "let h = output(f32)\nlet n = param(64)\nfn vertex(i) { let x = float(i % int(n))\n let z = float(i / int(n))\n h[i] = sin(x * 0.3) * cos(z * 0.2) * 4.0 }";
const NORMALS: &str = "let h = input(f32)\nlet nrm = output(vec3)\nlet n = param(64)\nfn vertex(i) { let w = int(n)\n let x = i % w\n let z = i / w\n let l = h[z * w + max(x - 1, 0)]\n let r = h[z * w + min(x + 1, w - 1)]\n let d = h[max(z - 1, 0) * w + x]\n let u = h[min(z + 1, w - 1) * w + x]\n nrm[i] = normalize(vec3(l - r, 2.0, d - u)) }";

fn words(v: &[u32]) -> Vec<f32> {
    v.iter().map(|w| f32::from_bits(*w)).collect()
}

#[test]
fn passes_chain_by_buffer_name_the_same_on_any_thread_count() {
    let n = 300usize;
    let run = |threads: usize| {
        let mut p = Pipeline::new();
        p.pass(makepad_render_kernels::compile(HEIGHTS, &[], &[]).unwrap(), Count::Fixed(n * n)).set_param("n", n as f32);
        p.pass(makepad_render_kernels::compile(NORMALS, &[], &[]).unwrap(), Count::Records { buffer: "h".into(), stride: 1 }).set_param("n", n as f32);
        if threads == 1 {
            p.run(&InlineExecutor, 1).unwrap();
        } else {
            p.run_on(engine(), Priority::Near).unwrap();
        }
        (p.take_buffer("h").unwrap(), p.take_buffer("nrm").unwrap())
    };
    let (h1, n1) = run(1);
    let (h8, n8) = run(8);
    assert_eq!(h1, h8);
    assert_eq!(n1, n8);
    assert_eq!(n1.len(), n * n * 3);
    // The second pass read the first one's neighbours.
    let (h, nrm) = (words(&h1), words(&n1));
    let k = 5 * n + 7;
    let want = [h[k - 1] - h[k + 1], 2.0, h[k - n] - h[k + n]];
    let len = (want[0] * want[0] + want[1] * want[1] + want[2] * want[2]).sqrt();
    for c in 0..3 {
        assert!((nrm[k * 3 + c] - want[c] / len).abs() < 1e-5);
    }
}

#[test]
fn ping_pong_repeats_smooth_a_state_buffer() {
    let src = "let a = input(f32)\nlet b = output(f32)\nfn element(i) { let l = a[max(i - 1, 0)]\n let r = a[min(i + 1, count - 1)]\n b[i] = (l + a[i] + r) * (1.0 / 3.0) }";
    let n = 10_000usize;
    let init: Vec<f32> = (0..n).map(|i| if i % 97 == 0 { 9.0 } else { 0.0 }).collect();
    let mut p = Pipeline::new();
    p.pass(makepad_render_kernels::compile(src, &[], &[]).unwrap(), Count::Records { buffer: "a".into(), stride: 1 });
    p.repeat(5).ping_pong("a", "b");
    p.set_buffer("a", init.iter().map(|x| x.to_bits()).collect());
    p.run_on(engine(), Priority::Far).unwrap();
    let got = words(p.buffer("a").unwrap());
    let mut want = init.clone();
    for _ in 0..5 {
        let prev = want.clone();
        for i in 0..n {
            let (l, r) = (prev[i.saturating_sub(1)], prev[(i + 1).min(n - 1)]);
            want[i] = (l + prev[i] + r) * (1.0 / 3.0);
        }
    }
    assert_eq!(got.iter().map(|x| x.to_bits()).collect::<Vec<_>>(), want.iter().map(|x| x.to_bits()).collect::<Vec<_>>());
}

#[test]
fn emitted_records_land_in_the_draw_shaders_reflected_layout() {
    // A reflected instance record: a (vec3) at 0, width (f32) at 3, birth
    // (f32) at 4, seed (u32) at 5, padded to 8 words.
    let field = |n: &str, ty, offset_words| LayoutField { name: LiveId::from_str_with_lut(n).unwrap(), ty, offset_words };
    let shader = ShaderLayout {
        kind: LayoutKind::Instance,
        packing: LayoutPacking::VertexFetch,
        stride_words: 8,
        fields: vec![field("a", PodType::Vec3, 0), field("width", PodType::F32, 3), field("birth", PodType::F32, 4), field("seed", PodType::U32, 5)],
        id: 0x5e6,
    };
    let seg = kernel_layout("Seg", &shader).unwrap();
    let src = "let s = emit_buffer(Seg, 3)\nfn primitive(i) { for k in 0..3 { if k < i % 4 { let r = Seg{}\n r.a = vec3(float(i), float(k), 0.0)\n r.width = 0.25\n r.birth = float(i) * 0.5\n r.seed = i * 16 + k\n emit(s, r) } } }";
    let kernel = makepad_render_kernels::compile(src, &[seg], &[]).unwrap();
    let n = 20_000usize;
    let mut p = Pipeline::new();
    p.pass(kernel.clone(), Count::Fixed(n));
    // i % 4 == 3 wants three records: exactly the three slots.
    p.run_on(engine(), Priority::Near).unwrap();
    let total = p.emitted("s").unwrap();
    assert_eq!(total, (0..n).map(|i| i % 4).sum::<usize>());
    // Into a ring slot: the draw reads the records in element order.
    let fences = ManualFences::new();
    let mut ring = OutputRing::new(2, Topology::Instances, shader.id, shader.stride_words);
    let mut lease = ring.begin_write(&fences).unwrap();
    lease.set_data(p.take_buffer("s").unwrap());
    ring.publish(lease, total as u32, &fences).unwrap();
    let v = ring.draw(&fences).unwrap();
    let mut at = 0;
    for i in 0..n {
        for k in 0..i % 4 {
            let r = &v.data[at * 8..at * 8 + 8];
            assert_eq!([f32::from_bits(r[0]), f32::from_bits(r[1]), f32::from_bits(r[3]), f32::from_bits(r[4])], [i as f32, k as f32, 0.25, i as f32 * 0.5]);
            assert_eq!(r[5], (i * 16 + k) as u32, "integers stay integer words");
            assert_eq!([r[6], r[7]], [0, 0], "padding is zero");
            at += 1;
        }
    }
    // One record too many per element is an error, not a shorter output.
    let over = "let s = emit_buffer(Seg, 2)\nfn primitive(i) { for k in 0..3 { let r = Seg{}\n r.width = 1.0\n emit(s, r) } }";
    let shader_seg = kernel_layout("Seg", &shader).unwrap();
    let mut p = Pipeline::new();
    p.pass(makepad_render_kernels::compile(over, &[shader_seg], &[]).unwrap(), Count::Fixed(10));
    assert!(matches!(p.run(&InlineExecutor, 1), Err(PipelineError::Overflow { .. })));
}
