use super::publish::{AssetWorkerHandle, PublishExecutor};
use super::{Executor, Poll};
use crate::{Literal, Node, Value};

#[derive(Default)]
pub struct OutputExecutor {
    result: Option<Poll>,
    value: Option<Value>,
    publisher: Option<PublishExecutor>,
}

impl OutputExecutor {
    pub fn start_with_asset_publish(
        &mut self,
        node: &Node,
        inputs: &[(String, Value)],
        worker: AssetWorkerHandle,
        flow: String,
        instance: String,
    ) -> Result<(), String> {
        let value = inputs
            .iter()
            .find_map(|(port, value)| (port == "value").then_some(value.clone()))
            .ok_or_else(|| format!("Output node `{}` has no value", node.id))?;
        let mut publish = node.clone();
        publish.kind = "publish".to_string();
        publish.params.retain(|(key, _)| key != "namespace" && key != "title" && key != "tags");
        publish.params.push(("namespace".to_string(), Literal::Str("flows".to_string())));
        publish.params.push((
            "title".to_string(),
            Literal::Str(format!("{flow} · {}", node.id)),
        ));
        publish.params.push((
            "tags".to_string(),
            Literal::Arr(vec![Literal::Str("flow".to_string()), Literal::Str(flow.clone())]),
        ));
        let mut publisher = PublishExecutor::new(Some(worker), flow, instance, String::new());
        publisher.start(&publish, inputs)?;
        self.value = Some(value);
        self.publisher = Some(publisher);
        Ok(())
    }
}

impl Executor for OutputExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String> {
        let value = inputs
            .iter()
            .find_map(|(port, value)| (port == "value").then_some(value.clone()))
            .ok_or_else(|| format!("Output node `{}` has no value", node.id))?;
        self.value = Some(value.clone());
        self.result = Some(Poll::Done(vec![("value".to_string(), value)]));
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        if let Some(publisher) = &mut self.publisher {
            return match publisher.poll() {
                Poll::Pending => Poll::Pending,
                Poll::Progress { permille, stage } => Poll::Progress { permille, stage },
                Poll::Done(_) => {
                    self.publisher = None;
                    Poll::Done(vec![("value".to_string(), self.value.take().unwrap())])
                }
                Poll::Failed(error) => {
                    self.publisher = None;
                    self.value = None;
                    Poll::Failed(format!("Output publish failed: {error}"))
                }
                Poll::Delta { port, text } => Poll::Delta { port, text },
            };
        }
        self.result.take().unwrap_or(Poll::Pending)
    }

    fn cancel(&mut self) {
        if let Some(publisher) = &mut self.publisher {
            publisher.cancel();
        }
        self.publisher = None;
        self.result = None;
        self.value = None;
    }
}
