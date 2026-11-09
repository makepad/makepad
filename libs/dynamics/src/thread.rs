//! Serial collision task dispatch.
//!
//! Translated from `src/engine/engine_thread.cc` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2026 DeepMind Technologies Limited);
//! changed: translated to Rust; thread pools remain untranslated.

use crate::data::Data;
use crate::model::Model;

// CpuPause, ThreadPoolContext and mju_threadpool are omitted with pool support.
pub fn dispatch<A>(m: &Model, d: &mut Data, func: &mut impl FnMut(&Model, &mut Data, &mut A, usize, usize), arg: &mut A, ntask: usize) {
    if d.threadpool == 0 || ntask < 2 {
        for i in 0..ntask { func(m, d, arg, 0, i); }
        return;
    }
    crate::unported!("thread pool");
}

pub fn num_thread(d: &Data) -> usize {
    if d.threadpool != 0 { crate::unported!("thread pool"); }
    1
}
