//! Batch creation and retirement. Each slice is still an ordinary instance
//! plus run; the batch only owns their queue-view lifetime.

use super::state::{CreateInstanceOutcome, SetInputsOutcome, StartRunOutcome};
use super::FlowState;
use crate::{BatchRunDto, CreateBatchRequest, CreateBatchResponse, InstanceId, RunId};

pub(crate) const MAX_BATCH_PARALLEL: u64 = 256;

#[derive(Clone, Debug)]
pub(crate) struct BatchRecord {
    pub runs: Vec<BatchRunDto>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CreateBatchOutcome {
    Created(CreateBatchResponse),
    FlowNotFound,
    FlowInvalid,
    InvalidParallel,
    Error(String),
}

impl FlowState {
    pub(crate) fn create_batch(
        &mut self,
        flow: &str,
        request: CreateBatchRequest,
    ) -> CreateBatchOutcome {
        if request.parallel == 0 || request.parallel > MAX_BATCH_PARALLEL {
            return CreateBatchOutcome::InvalidParallel;
        }
        if !self.definitions.contains_key(flow) {
            return CreateBatchOutcome::FlowNotFound;
        }
        if self
            .definitions
            .get(flow)
            .and_then(|definition| definition.graph.as_ref())
            .is_none()
        {
            return CreateBatchOutcome::FlowInvalid;
        }

        let mut batch = fresh_batch_id();
        while self.batches.contains_key(&batch) {
            batch = fresh_batch_id();
        }
        let mut runs = Vec::with_capacity(request.parallel as usize);
        for index in 1..=request.parallel {
            let label = Some(format!("batch-{batch}#{index}"));
            let instance = match self.create_instance(flow, label, true, Default::default()) {
                CreateInstanceOutcome::Created(instance) => instance,
                CreateInstanceOutcome::FlowNotFound => {
                    rollback(self, &runs);
                    return CreateBatchOutcome::FlowNotFound;
                }
                CreateInstanceOutcome::FlowInvalid => {
                    rollback(self, &runs);
                    return CreateBatchOutcome::FlowInvalid;
                }
                CreateInstanceOutcome::Error(error) => {
                    rollback(self, &runs);
                    return CreateBatchOutcome::Error(error);
                }
            };
            if let Some(inputs) = request.inputs.clone() {
                match self.set_instance_inputs(&instance, inputs, "tab") {
                    SetInputsOutcome::Ok(_) => {}
                    SetInputsOutcome::Error(error) => {
                        self.delete_instance(&instance);
                        rollback(self, &runs);
                        return CreateBatchOutcome::Error(error);
                    }
                    SetInputsOutcome::AskNotWaiting => {
                        self.delete_instance(&instance);
                        rollback(self, &runs);
                        return CreateBatchOutcome::Error(
                            "batch inputs cannot answer a waiting Ask".to_string(),
                        );
                    }
                    SetInputsOutcome::InstanceNotFound => {
                        rollback(self, &runs);
                        return CreateBatchOutcome::Error(
                            "batch instance disappeared before input setup".to_string(),
                        );
                    }
                }
            }
            let run_id = match self.start_run(&instance, None) {
                StartRunOutcome::Started { run_id, .. } => run_id,
                StartRunOutcome::InstanceNotFound => {
                    rollback(self, &runs);
                    return CreateBatchOutcome::Error(
                        "batch instance disappeared before run start".to_string(),
                    );
                }
                StartRunOutcome::FlowInvalid => {
                    self.delete_instance(&instance);
                    rollback(self, &runs);
                    return CreateBatchOutcome::FlowInvalid;
                }
                StartRunOutcome::Busy => {
                    self.delete_instance(&instance);
                    rollback(self, &runs);
                    return CreateBatchOutcome::Error("flow concurrency is zero".to_string());
                }
            };
            if let Some(row) = self.runs.get_mut(&RunId(run_id.clone())) {
                row.batch = Some(batch.clone());
                row.batch_index = Some(index);
            }
            runs.push(BatchRunDto {
                run_id,
                instance: instance.0,
            });
        }
        self.batches.insert(
            batch.clone(),
            BatchRecord {
                runs: runs.clone(),
            },
        );
        CreateBatchOutcome::Created(CreateBatchResponse { batch, runs })
    }

    /// Cancel every slice and immediately retire its volatile instance. Run
    /// rows stay until the queue explicitly clears the batch.
    pub(crate) fn cancel_batch(&mut self, batch: &str) -> Option<u64> {
        let record = self.batches.get(batch)?.clone();
        for run in &record.runs {
            self.cancel_run(&RunId(run.run_id.clone()));
            self.delete_instance(&InstanceId(run.instance.clone()));
        }
        Some(record.runs.len() as u64)
    }

    /// Row-level cancellation has the same immediate instance retirement as
    /// batch cancellation, while never touching a sibling slice.
    pub(crate) fn cancel_run_and_retire_batch_instance(&mut self, run_id: &RunId) -> bool {
        let instance = self
            .runs
            .get(run_id)
            .and_then(|row| row.batch.as_ref().map(|_| row.instance.clone()));
        if !self.cancel_run(run_id) {
            return false;
        }
        if let Some(instance) = instance {
            self.delete_instance(&instance);
        }
        true
    }

    /// Clear is terminal queue acknowledgement: cancel anything still live,
    /// retire all instances and forget the retained run rows.
    pub(crate) fn clear_batch(&mut self, batch: &str) -> Option<u64> {
        let count = self.cancel_batch(batch)?;
        let record = self.batches.remove(batch)?;
        for run in record.runs {
            self.runs.remove(&RunId(run.run_id));
        }
        Some(count)
    }
}

fn rollback(state: &mut FlowState, runs: &[BatchRunDto]) {
    for run in runs {
        state.delete_instance(&InstanceId(run.instance.clone()));
        state.runs.remove(&RunId(run.run_id.clone()));
    }
}

/// Eight hex digits from the OS entropy behind `RandomState`; the caller
/// redraws on the (vanishingly rare) repeat.
fn fresh_batch_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    format!("{:08x}", RandomState::new().build_hasher().finish() as u32)
}
