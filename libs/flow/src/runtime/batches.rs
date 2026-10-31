//! Batch creation and retirement. Each slice is still an ordinary instance
//! plus run; the batch only owns their queue-view lifetime.
//!
//! This is store state: a batch is created, rolled back, cancelled and
//! cleared in one store call each, so whoever owns the store (the engine
//! worker) keeps the all-or-nothing behaviour the HTTP plane documents.

use super::store::{
    CreateInstanceOutcome, FlowStore, RunDriver, SetInputsOutcome, StartRunOutcome,
};
use crate::instance::Owner;
use crate::{BatchRunDto, CreateBatchRequest, CreateBatchResponse, InstanceId, RunId};

pub const MAX_BATCH_PARALLEL: u64 = 256;

#[derive(Clone, Debug)]
pub struct BatchRecord {
    pub runs: Vec<BatchRunDto>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CreateBatchOutcome {
    Created(CreateBatchResponse),
    FlowNotFound,
    FlowInvalid,
    InvalidParallel,
    /// The retained-batch budget has no room for this many slices. Nothing
    /// was started; clearing a finished batch frees its rows.
    Busy { retained: usize, limit: usize },
    Error(String),
}

impl FlowStore {
    pub fn create_batch(
        &mut self,
        flow: &str,
        request: CreateBatchRequest,
        driver: &mut dyn RunDriver,
    ) -> CreateBatchOutcome {
        if request.parallel == 0 || request.parallel > MAX_BATCH_PARALLEL {
            return CreateBatchOutcome::InvalidParallel;
        }
        // A batch keeps its rows until it is cleared, so admission is against
        // what every batch still holds, before anything starts.
        if let Some(limit) = self.max_batch_runs {
            let retained = self.batch_runs();
            if retained + request.parallel as usize > limit {
                return CreateBatchOutcome::Busy { retained, limit };
            }
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
            let instance = match self.create_instance(flow, label, true, Owner::Tab, Default::default()) {
                CreateInstanceOutcome::Created(instance) => instance,
                CreateInstanceOutcome::FlowNotFound => {
                    self.rollback_batch(&runs);
                    return CreateBatchOutcome::FlowNotFound;
                }
                CreateInstanceOutcome::FlowInvalid => {
                    self.rollback_batch(&runs);
                    return CreateBatchOutcome::FlowInvalid;
                }
                CreateInstanceOutcome::Error(error) => {
                    self.rollback_batch(&runs);
                    return CreateBatchOutcome::Error(error);
                }
            };
            if let Some(inputs) = request.inputs.clone() {
                match self.set_instance_inputs(&instance, inputs, "tab") {
                    SetInputsOutcome::Ok(_) => {}
                    SetInputsOutcome::Error(error) => {
                        self.delete_instance(&instance);
                        self.rollback_batch(&runs);
                        return CreateBatchOutcome::Error(error);
                    }
                    SetInputsOutcome::AskNotWaiting => {
                        self.delete_instance(&instance);
                        self.rollback_batch(&runs);
                        return CreateBatchOutcome::Error(
                            "batch inputs cannot answer a waiting Ask".to_string(),
                        );
                    }
                    SetInputsOutcome::InstanceNotFound => {
                        self.rollback_batch(&runs);
                        return CreateBatchOutcome::Error(
                            "batch instance disappeared before input setup".to_string(),
                        );
                    }
                }
            }
            let run_id = match self.start_run(&instance, None, driver) {
                StartRunOutcome::Started { run_id, .. } => run_id,
                StartRunOutcome::InstanceNotFound => {
                    self.rollback_batch(&runs);
                    return CreateBatchOutcome::Error(
                        "batch instance disappeared before run start".to_string(),
                    );
                }
                StartRunOutcome::FlowInvalid => {
                    self.delete_instance(&instance);
                    self.rollback_batch(&runs);
                    return CreateBatchOutcome::FlowInvalid;
                }
                StartRunOutcome::Busy => {
                    self.delete_instance(&instance);
                    self.rollback_batch(&runs);
                    return CreateBatchOutcome::Error("flow concurrency is zero".to_string());
                }
                StartRunOutcome::Refused => {
                    self.delete_instance(&instance);
                    self.rollback_batch(&runs);
                    return CreateBatchOutcome::Error("run capacity is exhausted".to_string());
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
    pub fn cancel_batch(&mut self, batch: &str) -> Option<u64> {
        let record = self.batches.get(batch)?.clone();
        for run in &record.runs {
            self.cancel_run(&RunId(run.run_id.clone()));
            self.delete_instance(&InstanceId(run.instance.clone()));
        }
        Some(record.runs.len() as u64)
    }

    /// Row-level cancellation has the same immediate instance retirement as
    /// batch cancellation, while never touching a sibling slice. A run that
    /// belongs to no batch is simply cancelled.
    pub fn cancel_run_and_retire_batch_instance(&mut self, run_id: &RunId) -> bool {
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
    pub fn clear_batch(&mut self, batch: &str) -> Option<u64> {
        let count = self.cancel_batch(batch)?;
        let record = self.batches.remove(batch)?;
        for run in record.runs {
            self.runs.remove(&RunId(run.run_id));
        }
        Some(count)
    }

    fn rollback_batch(&mut self, runs: &[BatchRunDto]) {
        for run in runs {
            self.delete_instance(&InstanceId(run.instance.clone()));
            self.runs.remove(&RunId(run.run_id.clone()));
        }
    }
}

/// Eight hex digits from the OS entropy behind `RandomState`; the caller
/// redraws on the (vanishingly rare) repeat.
fn fresh_batch_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    format!("{:08x}", RandomState::new().build_hasher().finish() as u32)
}
