//! Once the first frame is up, a driver that links one program at a time compiles on this worker, so
//! frames don't wait behind a link. Its context shares nothing with ours, so no driver locks the two
//! together: a program comes back as its driver binary, which the render thread loads and the cache keeps.

use super::cache::{self, Key};
use super::egl::{Private, Share};
use super::render::{Queue as Jobs, Renderer};
use super::sys::GLenum;
use super::PipeState;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};

pub(super) struct Job {
    pub state: Weak<PipeState>,
    pub key: Key,
}

pub(super) enum Reply {
    /// the program's driver binary, `cached` when an earlier launch compiled it
    Binary { format: GLenum, binary: Vec<u8>, cached: bool },
    /// it didn't compile or link: the log
    Failed(String),
    /// the worker didn't compile it: it was stopped, or (the reason) it can't compile at all
    Declined(Option<String>),
}

pub(super) enum WorkerState {
    Unstarted,
    On(Worker),
    Off,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    compiling: bool,
    /// the render thread is loading a binary, which some drivers (Adreno) make wait for any compile
    loading: bool,
    /// the render thread hasn't loaded what we handed over yet, so no compile starts under that load
    handing_over: bool,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
}

fn lock(shared: &Shared) -> MutexGuard<'_, Queue> {
    shared.queue.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) struct Worker(Arc<Shared>);

/// Keeps the worker from starting a compile while the render thread loads a binary.
struct Hold(Arc<Shared>);

impl Drop for Hold {
    fn drop(&mut self) {
        lock(&self.0).loading = false;
        self.0.changed.notify_all();
    }
}

impl Worker {
    fn start(share: Arc<Share>, driver: String, dir: Option<PathBuf>, jobs: Jobs) -> Option<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let thread = std::thread::Builder::new().name("makepad-gl-compile".into())
            .spawn(move || run(&worker, &share, &driver, dir.as_deref(), &jobs));
        if let Err(e) = thread { eprintln!("[hal-gl] compile worker: {e}"); return None; }
        Some(Self(shared))
    }

    fn push(&self, job: Job) {
        lock(&self.0).jobs.push_back(job);
        self.0.changed.notify_all();
    }

    /// None while the worker compiles.
    fn hold(&self) -> Option<Hold> {
        let mut queue = lock(&self.0);
        if queue.compiling { return None; }
        queue.loading = true;
        Some(Hold(self.0.clone()))
    }

    fn end_handover(&self) {
        lock(&self.0).handing_over = false;
        self.0.changed.notify_all();
    }

    /// Stops the worker after the job it's on, and hands back the ones it hasn't started.
    fn stop(&self) -> Vec<Job> {
        let jobs = {
            let mut queue = lock(&self.0);
            queue.stop = true;
            queue.jobs.drain(..).collect()
        };
        self.0.changed.notify_all();
        jobs
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(shared: &Shared, share: &Arc<Share>, driver: &str, dir: Option<&Path>, jobs: &Jobs) {
    let mut gl: Option<Private> = None;
    loop {
        let next = {
            let mut queue = lock(shared);
            loop {
                if queue.stop { return; }
                if !queue.loading && !queue.handing_over {
                    if let Some(job) = queue.jobs.pop_front() { break Some(job); }
                }
                // we keep no context while there's nothing to compile
                if queue.jobs.is_empty() && gl.is_some() { break None; }
                queue = shared.changed.wait(queue).unwrap_or_else(|e| e.into_inner());
            }
        };
        let Some(job) = next else { gl = None; continue };
        let Some(state) = job.state.upgrade() else { continue };
        let reply = 'reply: {
            // an earlier launch compiled it, so the render thread only has to load it
            if let Some((format, binary)) = dir.and_then(|dir| cache::read(dir, job.key)) {
                break 'reply Reply::Binary { format, binary, cached: true };
            }
            if gl.is_none() {
                match Private::new(share) {
                    Ok(private) if private.gl.driver() == driver => gl = Some(private),
                    Ok(_) => break 'reply Reply::Declined(Some("its GL driver differs from the render thread's".into())),
                    Err(e) => break 'reply Reply::Declined(Some(e)),
                }
            }
            let gl = &gl.as_ref().unwrap().gl;
            {
                let mut queue = lock(shared);
                while queue.loading && !queue.stop { queue = shared.changed.wait(queue).unwrap_or_else(|e| e.into_inner()); }
                if queue.stop { break 'reply Reply::Declined(None); }
                queue.compiling = true;
            }
            let (program, shaders) = super::program::start_link(gl, &state.code.vertex, &state.code.fragment, true);
            let mut ok = 0;
            unsafe { (gl.glGetProgramiv)(program, super::sys::LINK_STATUS, &mut ok) };
            let reply = if ok == 0 {
                Reply::Failed(super::program::link_error(gl, program, shaders))
            } else {
                match cache::binary(gl, program) {
                    Some((format, binary)) => {
                        if let Some(dir) = dir { cache::write(dir, job.key, format, &binary); }
                        Reply::Binary { format, binary, cached: false }
                    }
                    None => Reply::Declined(Some("the driver hands back no program binaries".into())),
                }
            };
            unsafe {
                for s in shaders { (gl.glDeleteShader)(s) }
                (gl.glDeleteProgram)(program);
            }
            lock(shared).compiling = false;
            reply
        };
        drop(state);
        let give_up = matches!(reply, Reply::Declined(Some(_)));
        lock(shared).handing_over = true;
        jobs.push_urgent(move |r| r.worker_reply(job, reply));
        if give_up { return; }
    }
}

impl Renderer {
    /// After the first frame, a driver that links one program at a time compiles on the worker.
    /// True when `state` went there, or loaded from the cache here.
    pub(super) fn compile_on_worker(&mut self, state: &Arc<PipeState>, key: Key) -> bool {
        if !self.presented || self.caps.parallel_compile { return false; }
        if matches!(self.worker, WorkerState::Unstarted) {
            let cache = self.cache.as_ref().filter(|c| c.binaries);
            self.worker = cache.and_then(|c| Worker::start(self.ctx.share.clone(), self.ctx.gl.driver(), c.dir(), self.queue.clone()))
                .map_or(WorkerState::Off, WorkerState::On);
        }
        let WorkerState::On(worker) = &self.worker else { return false };
        // a cached program loads here, unless the worker is compiling
        if let Some(hold) = worker.hold() {
            if let Some(program) = self.cache.as_mut().and_then(|c| c.load(&self.ctx.gl, key)) {
                self.loaded(state, program);
                return true;
            }
            drop(hold);
        }
        if let WorkerState::On(worker) = &self.worker { worker.push(Job { state: Arc::downgrade(state), key }); }
        true
    }

    /// What the worker handed over for `job`; it starts no compile before this has run.
    pub(super) fn worker_reply(&mut self, job: Job, reply: Reply) {
        if let Some(state) = job.state.upgrade() {
            match reply {
                Reply::Binary { format, binary, cached } => match cache::load(&self.ctx.gl, format, &binary) {
                    Some(program) => {
                        if let Some(c) = &mut self.cache { c.loaded(); }
                        self.loaded(&state, program);
                    }
                    // that cache file is no good, so the program compiles afresh
                    None if cached => {
                        if let Some(c) = &self.cache { c.forget(job.key); }
                        let key = job.key;
                        match &self.worker {
                            WorkerState::On(worker) => worker.push(job),
                            _ => self.compile_here(&state, key),
                        }
                    }
                    // a binary made in this process didn't load back, so none will
                    None => {
                        if let Some(c) = &mut self.cache { c.reject(); }
                        self.stop_worker();
                        self.compile_here(&state, job.key);
                    }
                },
                Reply::Failed(log) => self.linked(&state, Err(log)),
                Reply::Declined(reason) => {
                    if let Some(reason) = reason {
                        eprintln!("[hal-gl] programs compile on the render thread, since the compile worker can't: {reason}");
                        self.stop_worker();
                    }
                    self.compile_here(&state, job.key);
                }
            }
        }
        if let WorkerState::On(worker) = &self.worker { worker.end_handover(); }
    }

    /// The render thread compiles from now on, starting with what the worker had queued.
    fn stop_worker(&mut self) {
        if let WorkerState::On(worker) = std::mem::replace(&mut self.worker, WorkerState::Off) {
            for job in worker.stop() {
                if let Some(state) = job.state.upgrade() { self.compile_here(&state, job.key); }
            }
        }
    }
}
