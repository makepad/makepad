use super::publish::{AssetWorkerHandle, PublishExecutor};
use super::{Executor, Poll};
use crate::{Literal, Node, PortType, Value};
use std::collections::HashSet;

pub struct ArchiveExecutor {
    original: Vec<(String, Value)>,
    jobs: Vec<(String, PublishExecutor)>,
    index: usize,
    node: String,
    keys: Vec<String>,
}

impl ArchiveExecutor {
    pub fn new(
        node: &Node,
        outputs: Vec<(String, Value)>,
        worker: AssetWorkerHandle,
        flow: String,
        instance: String,
        already_queued: &HashSet<String>,
    ) -> Result<Self, String> {
        let mut jobs = Vec::new();
        let mut keys = Vec::new();
        let mut seen = already_queued.clone();
        for (port, value) in &outputs {
            if !matches!(value.ty, PortType::Image | PortType::Audio | PortType::Video | PortType::Mesh) {
                continue;
            }
            let key = format!("{}:{}:{}", value.digest_hex(), value.ty.as_str(), value.content_type);
            if !seen.insert(key.clone()) { continue; }
            let mut publish = node.clone();
            publish.kind = "publish".to_string();
            publish.params.retain(|(key, _)| key != "namespace" && key != "title" && key != "tags");
            publish.params.push(("namespace".into(), Literal::Str("flows".into())));
            publish.params.push(("title".into(), Literal::Str(format!("{flow} · {}.{port}", node.id))));
            publish.params.push((
                "tags".into(),
                Literal::Arr(vec![Literal::Str("flow".into()), Literal::Str(flow.clone())]),
            ));
            let mut executor = PublishExecutor::new(Some(worker.clone()), flow.clone(), instance.clone(), String::new());
            executor.start(&publish, &[("value".into(), value.clone())])?;
            jobs.push((port.clone(), executor));
            keys.push(key);
        }
        Ok(Self { original: outputs, jobs, index: 0, node: node.id.clone(), keys })
    }

    pub fn keys(&self) -> impl Iterator<Item = String> + '_ {
        self.keys.iter().cloned()
    }
}

impl Executor for ArchiveExecutor {
    fn start(&mut self, _node: &Node, _inputs: &[(String, Value)]) -> Result<(), String> { Ok(()) }

    fn poll(&mut self) -> Poll {
        let Some((_, job)) = self.jobs.get_mut(self.index) else {
            return Poll::Done(self.original.clone());
        };
        match job.poll() {
            Poll::Pending => Poll::Pending,
            Poll::Progress { permille, stage } => Poll::Progress { permille, stage },
            Poll::Done(_) => { self.index += 1; Poll::Pending }
            Poll::Failed(error) => Poll::Failed(format!("Gen output archive failed for {}: {error}", self.node)),
            Poll::Delta { port, text } => Poll::Delta { port, text },
        }
    }

    fn cancel(&mut self) {
        let start = self.index.min(self.jobs.len());
        for (_, job) in &mut self.jobs[start..] {
            job.cancel();
        }
        self.index = self.jobs.len();
    }
}
