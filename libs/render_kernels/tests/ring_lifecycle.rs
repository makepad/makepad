//! The kernel output lifecycle under the fence-delay test flag, and a hot
//! reload under load (KERNELS.md P5 gate).
//!
//! A simulated renderer draws one frame per iteration and its fences
//! complete two frames late (the GPU running behind), while kernel jobs on
//! the engine's pool keep writing the next output. Halfway, the draw
//! shader changes (a field is added: a new layout id and stride); the new
//! kernel compiles on another thread while the old one keeps producing.
//!
//! Checked every frame: a writer never gets a slot a frame in flight reads;
//! every drawn buffer is whole records of one layout (never mixed); after
//! the first publish in the new layout the old one never draws again; old
//! slots retire only after their fences; a job begun under the old layout
//! cannot publish.

use makepad_render_kernels::compute::kernel::{FieldTy, Layout, LayoutField};
use makepad_render_kernels::compute::sched::{Job, JobHandle, Priority};
use makepad_render_kernels::compute::admission::{JobBudget, Origin};
use makepad_render_kernels::{engine, FrameFences, ManualFences, OutputRing, PublishError, SlotState, Topology, WriteLease};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

fn inst_a() -> Layout {
    Layout { name: "Inst".into(), stride: 4, fields: vec![LayoutField { name: "pos".into(), ty: FieldTy::Vec3, offset: 0 }, LayoutField { name: "tag".into(), ty: FieldTy::I32, offset: 3 }] }
}

fn inst_b() -> Layout {
    let mut l = inst_a();
    l.stride = 6;
    l.fields.push(LayoutField { name: "birth".into(), ty: FieldTy::Vec2, offset: 4 });
    l
}

const SRC_A: &str = "let out = output(Inst)\nlet frame = param(0)\nfn instance(i) { out[i].pos = vec3(float(i), frame, 0.5)\n out[i].tag = 1 }";
const SRC_B: &str = "let out = output(Inst)\nlet frame = param(0)\nfn instance(i) { out[i].pos = vec3(float(i), frame, 0.5)\n out[i].tag = 2\n out[i].birth = vec2(frame, 1.0) }";

fn count_of(frame: usize) -> usize {
    3000 + (frame * 977) % 9000
}

#[test]
fn slots_under_delayed_fences_and_a_hot_reload_under_load() {
    let fences = ManualFences::with_delay(2);
    let (layout_a, layout_b) = (0xA, 0xB);
    let mut ring = OutputRing::new(2, Topology::Instances, layout_a, 4);
    let kernel_a = makepad_render_kernels::compile(SRC_A, &[inst_a()], &[]).unwrap();
    let mut kernel = kernel_a.clone();
    let mut layout = layout_a;
    // The new shader compiles on its own thread while frames go on.
    let mut compiling: Option<std::thread::JoinHandle<_>> = None;
    let mut running: Option<(JobHandle, WriteLease, usize, u64)> = None;
    // Frame serial -> the buffer it reads (its allocation), until its fence.
    let mut reading: HashMap<u64, usize> = HashMap::new();
    let mut new_layout_seen = false;
    let mut stale_refused = 0;
    let mut drawn = 0;
    for frame in 0..400usize {
        if frame == 150 {
            compiling = Some(std::thread::spawn(|| makepad_render_kernels::compile(SRC_B, &[inst_b()], &[]).unwrap()));
        }
        // A finished job publishes.
        if let Some((h, _, _, _)) = &running {
            if h.is_done() {
                let (h, mut lease, count, _) = running.take().unwrap();
                let mut job = h.try_take().unwrap().unwrap_or_else(|(_, e)| panic!("{}", e));
                lease.set_data(job.take_output_u32("out").unwrap());
                match ring.publish(lease, count as u32, &fences) {
                    Ok(_) => {}
                    Err(PublishError::StaleLayout { .. }) => stale_refused += 1,
                    Err(e) => panic!("{}", e),
                }
            }
        }
        // Start the next output when a slot is free.
        if running.is_none() {
            if let Ok(mut lease) = ring.begin_write(&fences) {
                // Never a buffer a frame in flight still reads.
                let done = fences.completed();
                let ptr = lease.data_mut().as_ptr() as usize;
                if lease.data_mut().capacity() > 0 {
                    for (s, p) in &reading {
                        assert!(!(*s > done && *p == ptr), "frame {} (in flight, completed {}) reads the buffer handed to a writer", s, done);
                    }
                }
                let count = count_of(frame);
                let stride = lease.stride() as usize;
                assert_eq!(stride, if lease.layout() == layout_a { 4 } else { 6 });
                let mut data = lease.take_data();
                data.clear();
                data.resize(count * stride, 0);
                let mut job = Job::new(if lease.layout() == layout { kernel.clone() } else { kernel_a.clone() }, count);
                job.set_param("frame", frame as f32);
                job.output_u32("out", data).unwrap();
                let h = engine().scheduler().submit(job, Priority::Near, Origin::Host, JobBudget { wall: Duration::from_secs(10) }).unwrap_or_else(|(_, e)| panic!("{}", e));
                running = Some((h, lease, count, layout));
            }
        }
        // The reload lands while an old-layout job runs: that job's output
        // must be refused.
        if running.is_some() && compiling.as_ref().is_some_and(|h| h.is_finished()) {
            kernel = compiling.take().unwrap().join().unwrap();
            layout = layout_b;
            ring.set_layout(layout_b, 6);
        }
        // Draw this frame, then submit it.
        let serial = fences.submitted() + 1;
        if let Some(v) = ring.draw(&fences) {
            drawn += 1;
            let stride = v.stride as usize;
            assert_eq!(v.data.len(), v.count as usize * stride, "whole records");
            let tag = if v.layout == layout_a { 1 } else { 2 };
            assert_eq!(stride, if tag == 1 { 4 } else { 6 });
            for r in v.data.chunks_exact(stride) {
                assert_eq!(r[3], tag, "one layout per draw");
            }
            if v.layout == layout_b {
                new_layout_seen = true;
            } else {
                assert!(!new_layout_seen, "the old layout drew again after the new one published");
            }
            reading.insert(serial, v.data.as_ptr() as usize);
        }
        assert_eq!(fences.submit(), serial);
        fences.complete(serial);
        let done = fences.completed();
        reading.retain(|s, _| *s > done);
        std::thread::sleep(Duration::from_micros(300));
    }
    if let Some((h, lease, _, _)) = running.take() {
        let mut job = h.wait().unwrap_or_else(|(_, e)| panic!("{}", e));
        let _ = job.take_output_u32("out");
        ring.abandon(lease);
    }
    assert!(new_layout_seen, "the new layout published");
    let stats = ring.stats();
    assert!(stats.published > 20, "{:?}", stats);
    assert!(stats.busy > 0, "with fences two frames late some frames find no free slot: {:?}", stats);
    assert_eq!(stats.layout_swaps, 1);
    assert_eq!(stale_refused, 1, "the job begun under the old layout could not publish");
    assert!(stats.max_in_flight >= 1);
    assert!(drawn > 300);
    // Old slots retire once their fences signal.
    fences.complete(u64::MAX);
    for _ in 0..3 {
        fences.submit();
    }
    ring.collect(&fences);
    assert!(ring.slot_states().iter().all(|s| matches!(s, SlotState::Free | SlotState::Published)), "{:?}", ring.slot_states());
    println!("stats {:?}, stale refused {}", stats, stale_refused);
}

#[test]
fn an_emitted_index_buffer_is_validated_before_publish() {
    // A kernel emits triangles whose indices point at its own vertices;
    // a corrupt one (an index past the count) keeps the previous output.
    let fences = ManualFences::new();
    let mut ring = OutputRing::new(2, Topology::Triangles { indexed: true }, 1, 3);
    let k = makepad_render_kernels::compile("let v = output(vec3)\nlet ix = output(i32, 3)\nlet bad = param(0)\nfn element(i) { v[i] = vec3(float(i), 0.0, 0.0)\n let o = if bad > 0.5 && i == 2 { 100 } else { 0 }\n ix[i] = [i, (i + 1) % count, (i + 2) % count + o] }", &[], &[]);
    let k = match k {
        Ok(k) => k,
        // Arrays as outputs are optional syntax; the scalar form below.
        Err(_) => makepad_render_kernels::compile("let v = output(vec3)\nlet a = output(i32, 3, 0, ix)\nlet b = output(i32, 3, 1, ix)\nlet c = output(i32, 3, 2, ix)\nlet bad = param(0)\nfn element(i) { v[i] = vec3(float(i), 0.0, 0.0)\n a[i] = i\n b[i] = (i + 1) % count\n c[i] = if bad > 0.5 && i == 2 { 100 } else { (i + 2) % count } }", &[], &[]).unwrap(),
    };
    for (bad, ok) in [(0.0, true), (1.0, false)] {
        let mut lease = ring.begin_write(&fences).unwrap();
        let mut job = Job::new(k.clone(), 5);
        job.set_param("bad", bad);
        job.output_u32("v", vec![0; 15]).unwrap();
        job.output_u32("ix", vec![0; 15]).unwrap();
        let mut job = engine().run(job, Priority::MustComplete, Duration::from_secs(5)).unwrap_or_else(|(_, e)| panic!("{}", e));
        lease.set_data(job.take_output_u32("v").unwrap());
        *lease.indices_mut() = job.take_output_u32("ix").unwrap();
        assert_eq!(ring.publish(lease, 5, &fences).is_ok(), ok);
    }
    assert_eq!(ring.generation(), 1, "the corrupt output did not replace the good one");
    assert_eq!(ring.draw(&fences).unwrap().indices[6..9], [2, 3, 4]);
    let _ = Arc::new(());
}
