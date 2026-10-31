//! A session's reusable semantic workers. A wait must leave another worker able to
//! perform the action that satisfies it. Workers grow to observed concurrency and
//! remain alive until the authenticated session closes.
use super::{proto, Out, ServerMsg};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{channel, Sender},
    Arc,
};

struct Worker {
    send: Sender<(u64, String)>,
    pending: Arc<AtomicUsize>,
}
pub(super) struct Session {
    id: u32,
    out: Sender<Out>,
    alive: Arc<AtomicBool>,
    workers: Vec<Worker>,
}
impl Drop for Session {
    fn drop(&mut self) { self.alive.store(false, Ordering::Release); }
}
impl Session {
    pub fn new(id: u32, out: Sender<Out>) -> Self {
        Self { id, out, alive: Arc::new(AtomicBool::new(true)), workers: Vec::new() }
    }
    pub fn submit(&mut self, req: u64, line: String) {
        let idle = self.workers.iter().position(|w| w.pending.load(Ordering::Acquire) == 0);
        let index = match idle {
            Some(index) => index,
            None => match self.spawn() {
                Ok(worker) => { self.workers.push(worker); self.workers.len() - 1 }
                Err(error) => {
                    eprintln!("[desk-link] semantic worker unavailable: {error}; queuing on existing worker");
                    let Some((index, _)) = self.workers.iter().enumerate().min_by_key(|(_, w)| w.pending.load(Ordering::Acquire)) else {
                        self.error(req, &format!("semantic worker unavailable: {error}"));
                        return;
                    };
                    index
                }
            },
        };
        let worker = &self.workers[index];
        worker.pending.fetch_add(1, Ordering::AcqRel);
        if worker.send.send((req, line)).is_err() {
            worker.pending.fetch_sub(1, Ordering::AcqRel);
            self.error(req, "semantic worker stopped; inspect before retrying");
        }
    }
    fn error(&self, req: u64, error: &str) {
        let text = format!("{{\"err\":{}}}\n", makepad_remote::json_str(error));
        let _ = self.out.send(Out::Msg(ServerMsg::A11yReply { req, text }.encode()));
    }
    fn spawn(&self) -> std::io::Result<Worker> {
        let (send, recv) = channel::<(u64, String)>();
        let alive = self.alive.clone();
        let out = self.out.clone();
        let pending = Arc::new(AtomicUsize::new(0));
        let count = pending.clone();
        std::thread::Builder::new().name(format!("desk-link-ui-{}-{}", self.id, self.workers.len())).spawn(move || {
            while let Ok((req, line)) = recv.recv() {
                if !alive.load(Ordering::Acquire) { break; }
                let mut bytes = Vec::new();
                let cancelled = alive.clone();
                makepad_remote::cmd::answer_to_cancellable(dispatch, &line, &mut bytes,
                    Some(Arc::new(move || !cancelled.load(Ordering::Acquire))));
                if !alive.load(Ordering::Acquire) { break; }
                let text = if bytes.len() + 12 <= proto::MAX_MESSAGE {
                    String::from_utf8_lossy(&bytes).into_owned()
                } else {
                    "{\"err\":\"semantic reply exceeds Desk Link framing; narrow q= or depth=\"}\n".into()
                };
                if out.send(Out::Msg(ServerMsg::A11yReply { req, text }.encode())).is_err() { break; }
                count.fetch_sub(1, Ordering::AcqRel);
            }
        })?;
        Ok(Worker { send, pending })
    }
}
fn dispatch(req: &makepad_remote::Req, out: &mut makepad_remote::Out) {
    if !makepad_hal::a11y::dispatch(req, out) { out.err("Desk Link accepts ui commands only"); }
}
