//! Shared worker-owned pipeline lifecycle for Flux and inpaint.

/// Evict a different model before loading, or carry the component owners across
/// a size rebuild. Drop the host arenas before the replacement allocates.
pub(crate) fn ensure<P, O, R, E>(
    slot: &mut Option<P>,
    request: R,
    serves: impl FnOnce(&P, &R) -> bool,
    same_model: impl FnOnce(&P, &R) -> bool,
    owners: impl FnOnce(&P, &R) -> O,
    evict: impl FnOnce(&P, &R),
    load: impl FnOnce(Option<&mut P>, Option<O>, R) -> Result<Option<P>, E>,
    cancelled: impl FnOnce(&E) -> bool,
) -> Result<(), E> {
    let mut carried = None;
    if !slot.as_ref().is_some_and(|pipeline| serves(pipeline, &request)) {
        if let Some(old) = slot.take() {
            if same_model(&old, &request) {
                carried = Some(owners(&old, &request));
            } else {
                evict(&old, &request);
            }
            drop(old);
        }
    }
    let result = load(slot.as_mut(), carried, request);
    if let Some(pipeline) = finish(slot, result, cancelled)? {
        *slot = Some(pipeline);
    }
    Ok(())
}

/// Cancellation keeps a valid warm pipeline; any other failure retires it.
pub(crate) fn finish<P, T, E>(
    slot: &mut Option<P>,
    result: Result<T, E>,
    cancelled: impl FnOnce(&E) -> bool,
) -> Result<T, E> {
    if result.as_ref().is_err_and(|err| !cancelled(err)) {
        *slot = None;
    }
    result
}

#[cfg(all(test, any(target_os = "macos", all(target_os = "linux", feature = "cuda"))))]
mod tests {
    use super::*;
    use makepad_ai_common::backend::{
        gpu_weight_cache_evict_prefix, gpu_weight_cache_resident,
        try_matmul_nt_ggml_bytes_cached, WeightOwner,
    };
    use std::cell::Cell;
    use std::sync::Arc;

    struct Pipeline {
        model: [u16; 2],
        size: usize,
        owners: Vec<Arc<WeightOwner>>,
    }

    impl Pipeline {
        fn load(
            model: [u16; 2],
            size: usize,
            owners: Option<Vec<Option<Arc<WeightOwner>>>>,
            calls: &Cell<usize>,
            fail: bool,
            seen: &mut Vec<String>,
            retired: &[String],
        ) -> Result<Self, &'static str> {
            let carried: Vec<bool> = (0..2)
                .map(|i| owners.as_ref().is_some_and(|owners| owners[i].is_some()))
                .collect();
            let pipeline = Self {
                model,
                size,
                owners: owners.unwrap_or_else(|| vec![None, None]).into_iter()
                    .map(|owner| owner.unwrap_or_else(|| Arc::new(WeightOwner::new("hub-warm-slot"))))
                    .collect(),
            };
            *seen = pipeline.owners.iter().map(|o| o.namespace().to_owned()).collect();
            // Two 8x16 F16 matrices per component. Both backends use their
            // actual keyed entry; the loader count measures device uploads.
            for (component, owner) in pipeline.owners.iter().enumerate() {
                let calls_before = calls.get();
                let bytes: Vec<u8> = (0..8 * 16).flat_map(|_| model[component].to_le_bytes()).collect();
                for key in ["first", "second"] {
                    let out = try_matmul_nt_ggml_bytes_cached(
                        &vec![1.0; size * 16], 1, size, 16, 8,
                        owner.namespace(), key,
                        || { calls.set(calls.get() + 1); Ok(std::borrow::Cow::Borrowed(&bytes)) },
                    ).expect("test requires a real GPU backend").expect("keyed matmul succeeds");
                    let expected = if model[component] == 0x3c00 { 16.0 } else { 32.0 };
                    assert_eq!(out, vec![expected; size * 8], "model identity selects the correct bytes");
                    if component == 0 && key == "first" {
                        assert_eq!(resident(retired), (0, 0),
                            "first matmul drains all replaced component owners");
                    }
                    if fail { return Err("load failed after uploading weights"); }
                }
                if carried[component] {
                    assert_eq!(calls.get(), calls_before, "carried component invokes no loader");
                }
            }
            Ok(pipeline)
        }
    }

    fn rebuild(
        slot: &mut Option<Pipeline>, model: [u16; 2], size: usize, calls: &Cell<usize>,
        fail: bool, seen: &mut Vec<String>, evictions: &Cell<usize>,
    ) -> Result<(), &'static str> {
        let retired: Vec<String> = slot.as_ref().map(|p| p.owners.iter().enumerate()
            .filter(|(i, _)| p.model[0] != model[0] || p.model[*i] != model[*i])
            .map(|(_, owner)| owner.namespace().to_owned()).collect()).unwrap_or_default();
        ensure(
            slot, (model, size),
            |p, &(model, size)| p.model == model && p.size == size,
            |p, &(model, _)| p.model[0] == model[0],
            |p, &(model, _)| p.owners.iter().enumerate()
                .map(|(i, owner)| (p.model[i] == model[i]).then(|| owner.clone())).collect(),
            |p, _| {
                for owner in &p.owners {
                    gpu_weight_cache_evict_prefix(owner.namespace()).unwrap();
                    assert_eq!(gpu_weight_cache_resident(owner.namespace()).unwrap(), (0, 0),
                        "model switch frees the old stack before the next load");
                }
                evictions.set(evictions.get() + 1);
            },
            |warm, owners, (model, size)| {
                if warm.is_some() { return Ok(None); }
                Pipeline::load(model, size, owners, calls, fail, seen, &retired).map(Some)
            },
            |err| *err == "cancelled",
        )
    }

    fn resident(names: &[String]) -> (usize, usize) {
        names.iter().map(|ns| gpu_weight_cache_resident(ns).unwrap())
            .fold((0, 0), |(ac, ab), (c, b)| (ac + c, ab + b))
    }

    #[test]
    fn size_rebuild_keeps_one_weight_stack() {
        std::thread::spawn(|| {
            let (calls, evictions) = (Cell::new(0), Cell::new(0));
            let mut slot = None;
            let mut names = Vec::new();
            rebuild(&mut slot, [0x3c00; 2], 1, &calls, false, &mut names, &evictions).unwrap();
            let first = names.clone();
            assert_eq!(calls.get(), 4, "cold load uploads each weight once");
            assert_eq!(resident(&first), (4, 1024), "one complete stack is resident");
            rebuild(&mut slot, [0x3c00; 2], 2, &calls, false, &mut names, &evictions).unwrap();
            assert_eq!(names, first, "size rebuild carries the exact component owners");
            assert_eq!(calls.get(), 4, "size rebuild invokes no weight loader");
            assert_eq!(resident(&names), (4, 1024), "size rebuild keeps one stack");
            assert_eq!(evictions.get(), 0, "size rebuild does not evict");
            rebuild(&mut slot, [0x3c00, 0x4000], 2, &calls, false, &mut names, &evictions).unwrap();
            let split = names.clone();
            assert_eq!(names[0], first[0], "unchanged component A retains its owner");
            assert_ne!(names[1], first[1], "changed component B gets a fresh owner");
            assert_eq!(calls.get(), 6, "only component B uploads new weights");
            assert_eq!(gpu_weight_cache_resident(&first[0]).unwrap(), (2, 512),
                "component A keeps its original residency");
            assert_eq!(gpu_weight_cache_resident(&first[1]).unwrap(), (0, 0),
                "component B's old entries are gone");
            assert_eq!(resident(&names), (4, 1024), "component replacement keeps one stack");
            assert_eq!(evictions.get(), 0, "component replacement skips full eviction");
            rebuild(&mut slot, [0x4000; 2], 2, &calls, false, &mut names, &evictions).unwrap();
            assert_eq!(evictions.get(), 1, "model change explicitly evicts");
            assert_eq!(resident(&split), (0, 0), "outgoing model is gone");
            assert_eq!(calls.get(), 10, "new model uploads its own stack");
            assert_eq!(resident(&names), (4, 1024), "only the new stack is resident");
            let cancelled: Result<(), &str> = Err("cancelled");
            assert!(finish(&mut slot, cancelled, |err| *err == "cancelled").is_err());
            assert!(slot.is_some(), "cancel retains a valid warm pipeline");
            drop(slot);
            assert_eq!(resident(&names), (0, 0), "final pipeline drop retires its stack");
        }).join().unwrap();
    }

    #[test]
    fn failed_load_then_cold_load_retires_dead_namespace() {
        std::thread::spawn(|| {
            let (calls, evictions) = (Cell::new(0), Cell::new(0));
            let mut slot = None;
            let mut names = Vec::new();
            assert!(rebuild(&mut slot, [0x3c00; 2], 1, &calls, true, &mut names, &evictions).is_err());
            let failed = names.clone();
            assert!(slot.is_none(), "failed partial load never fills the warm slot");
            assert_eq!(calls.get(), 1, "failed load did upload a weight");
            // Stop the next load after its first real matmul too. Inspect on
            // this same worker while the replacement owner is still alive.
            ensure(&mut slot, (), |_, _| false, |_, _| false, |p, _| p.owners.clone(), |_, _| {},
                |_, owners, ()| {
                    let owner = Arc::new(WeightOwner::new("hub-warm-slot-cold"));
                    assert!(owners.is_none(), "failed load carries no owners");
                    let bytes = vec![0u8; 8 * 16 * 2];
                    try_matmul_nt_ggml_bytes_cached(&[1.0; 16], 1, 1, 16, 8,
                        owner.namespace(), "w", || Ok(std::borrow::Cow::Borrowed(&bytes)))
                        .expect("real GPU backend").unwrap();
                    assert_eq!(resident(&failed), (0, 0), "first cold matmul drains the failed owner's namespace");
                    assert_eq!(gpu_weight_cache_resident(owner.namespace()).unwrap(), (1, 256),
                        "only the new owner's weight remains");
                    names = vec![owner.namespace().to_owned()];
                    Ok(Some(Pipeline { model: [0; 2], size: 1, owners: vec![owner] }))
                }, |_: &&str| false,
            ).unwrap();
            let failed_run: Result<(), &str> = Err("run failed");
            assert!(finish(&mut slot, failed_run, |_| false).is_err());
            assert!(slot.is_none(), "non-cancel run failure drops the pipeline");
            assert_eq!(resident(&names), (0, 0), "run failure retires the new stack too");
        }).join().unwrap();
    }
}
