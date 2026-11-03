//! Small native Vulkan dispatch with an exact-sized input and excess invocations.
#![cfg(target_os = "linux")]
use makepad_hal::*;
use makepad_hal::codec::ArenaSink;
use makepad_script::{shader_ir_compute::*, *};
use std::time::{Duration, Instant};

fn until(hal: &Hal, session: u32, mut done: impl FnMut(HalEvent<'_>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = EventStore::new(session, 32, 4096);
    loop {
        assert!(Instant::now() < deadline, "compute completion deadline");
        let mut buf = events.buf();
        let count = hal.pump(1_000_000, &mut buf);
        for (_, event) in events.reader(count, buf.arena_len) {
            match &event {
                HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                HalEvent::DeviceLost { .. } => panic!("device lost"),
                _ => {}
            }
            if done(event) { return; }
        }
    }
}

#[test]
fn tail_threads_skip_reads_and_effects() {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::VULKAN, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend_id::VULKAN);
    assert_ne!(info.flags & info_flags::COMPUTE, 0);
    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    let value = vm.eval(ScriptMod::new("", "", "short_circuit", 0, 0,
        include_str!("../script/tests/support/short_circuit.splash"), &[]));
    assert!(vm.take_errors().is_empty());
    let kernel = compile_compute_kernel_result(&mut vm, value.as_object().unwrap(), ComputeTarget::Spirv).view().to_kernel().unwrap();
    assert!(vm.take_errors().is_empty());
    let layout = &kernel.layout;
    let src = Id::new(1, 1);
    let dst = Id::new(2, 1);
    let calls = Id::new(3, 1);
    let pipeline = Id::new(1, 1);
    let code: Vec<_> = kernel.code.as_spirv().unwrap().iter().flat_map(|w| w.to_le_bytes()).collect();
    let bindings: Vec<_> = layout.buffers.iter().map(|b| StorageBinding { index: b.index, offset: b.offset, writable: b.writable as u32 }).collect();
    let blob = CmdWriter::compute_pipeline_blob(PipelineDesc::default(), &code, ComputeDesc {
        push_size: layout.push_size, param_binding: layout.param_binding.unwrap_or(u32::MAX),
        param_binding_size: layout.param_binding_size,
        group_count_offset: layout.group_count_offset.unwrap_or(u32::MAX),
        workgroup: layout.workgroup, precise_math: layout.precise_math as u32, ..Default::default()
    }, &bindings, &[]);
    let mut w = CmdWriter::new(info.session);
    for (id, size) in [(src, 20), (dst, 128), (calls, 8)] {
        w.buffer_create(id, buffer_kind::STORAGE | buffer_kind::READBACK, size, Attr::default());
    }
    let input: Vec<_> = [-1.0f32, 0.0, 1.0, 2.0, -2.0].into_iter().flat_map(f32::to_le_bytes).collect();
    w.buffer_update(src, 0, &input);
    w.buffer_update(dst, 0, &[255; 128]);
    w.buffer_update(calls, 0, &[0; 8]);
    w.pipeline_create(pipeline, dialect::SPIRV, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, info.session, |e| matches!(e, HalEvent::PipelineReady { pipeline: p } if p == pipeline));
    let mut w = CmdWriter::new(info.session);
    let ids: Vec<_> = layout.buffers.iter().map(|b| match b.name.to_string().as_str() {
        "src" => src, "dst" => dst, "calls" => calls, _ => panic!("unexpected binding"),
    }).collect();
    let buffers = w.put_ids(&ids);
    let params = w.put_bytes(&vec![0; layout.push_size as usize]);
    w.frame_begin(1, FrameMeta::default());
    w.compute_pass(0, 0, &[Dispatch { pipeline, groups: [1, 1, 1], params,
        buffers: Range { off: buffers.off * 8, len: buffers.len * 8 }, ..Default::default() }]);
    w.frame_end(Id::NONE, target::COMPUTE);
    w.buffer_readback(dst, 1, 0, 128);
    w.buffer_readback(calls, 2, 0, 8);
    hal.submit(&w.buf());
    let mut results = std::collections::HashMap::new();
    until(&hal, info.session, |e| {
        if let HalEvent::Readback { req, status, pixels, .. } = e {
            assert_eq!(status, readback::OK);
            results.insert(req, pixels.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect::<Vec<_>>());
        }
        results.len() == 2
    });
    hal.close(info.session);
    let mut expected = vec![5; 32];
    expected[..5].copy_from_slice(&[0, 0, 7, 7, 0]);
    assert_eq!(results[&1], expected, "valid values and all 27 tail invocations");
    assert_eq!(results[&2], [5, 5], "each logical RHS runs only for the five valid invocations");
}
