//! The JSON event journal's rendering.
//!
//! Flow state lives in the engine (`runtime`); the HTTP host keeps no store.
//! What is left here is the wire shape of the journal: each typed
//! [`StoreEvent`] the engine's ordered journal lane delivers becomes the
//! topic, kind and JSON payload the event plane has always published.

use super::config::FlowServerConfig;
use super::events::EventHub;
use super::util::log;
use crate::engine::RunId;
use crate::instance::InstanceId;
use crate::runtime::store::StoreEvent;
use crate::RunEventPayload;
use makepad_micro_serde::{DeJson, JsonValue, SerJson};
use std::collections::HashMap;

pub use crate::runtime::store::{Definition, NodeRow, RunRow};

/// Publish one store event, in the order the engine produced it.
pub(crate) fn publish_store_event(events: &EventHub, config: &FlowServerConfig, event: StoreEvent) {
    match event {
        StoreEvent::FlowChanged { name, revision, canonical } => events.publish(
            "flows",
            "flow.changed",
            JsonValue::Object(HashMap::from([
                ("name".to_string(), JsonValue::String(name)),
                ("revision".to_string(), JsonValue::U64(revision)),
                ("canonical".to_string(), JsonValue::Bool(canonical)),
            ])),
        ),
        StoreEvent::FlowError { name, error } => events.publish(
            "flows",
            "flow.error",
            JsonValue::Object(HashMap::from([
                ("name".to_string(), JsonValue::String(name)),
                ("error".to_string(), json_value(&error)),
            ])),
        ),
        StoreEvent::FlowRemoved { name } => events.publish(
            "flows",
            "flow.removed",
            JsonValue::Object(HashMap::from([("name".to_string(), JsonValue::String(name))])),
        ),
        StoreEvent::InstanceCreated { instance, flow } => {
            publish_instance_lifecycle(events, "flows", "instance.created", &instance, &flow)
        }
        StoreEvent::InstanceRemoved { instance, flow } => {
            publish_instance_lifecycle(events, "flows", "instance.removed", &instance, &flow)
        }
        StoreEvent::InstanceInputs { instance, flow } => {
            publish_instance_lifecycle(events, "flows", "instance.inputs", &instance, &flow)
        }
        StoreEvent::InstanceCleared { instance, flow } => {
            publish_instance_lifecycle(events, "instance", "instance.cleared", &instance, &flow)
        }
        StoreEvent::Run { run_id, instance, flow, event } => {
            let payload = event.to_wire();
            let kind = run_event_kind(&payload);
            let json = run_event_json(&run_id, &instance, &flow, &payload);
            events.publish("run", kind, json.clone());
            // Published to `instance` too, at the granularity §4.3 names.
            if matches!(
                payload,
                RunEventPayload::NodeDone { .. }
                    | RunEventPayload::NodeWaiting { .. }
                    | RunEventPayload::NodeAnswered { .. }
                    | RunEventPayload::RunStarted { .. }
                    | RunEventPayload::RunFinished { .. }
            ) {
                events.publish("instance", kind, json);
            }
        }
        StoreEvent::Log(message) => log(config, &message),
    }
}

fn publish_instance_lifecycle(events: &EventHub, topic: &str, kind: &str, id: &InstanceId, flow: &str) {
    events.publish(
        topic,
        kind,
        JsonValue::Object(HashMap::from([
            ("instance".to_string(), JsonValue::String(id.0.clone())),
            ("flow".to_string(), JsonValue::String(flow.to_string())),
        ])),
    );
}

fn run_event_kind(payload: &RunEventPayload) -> &'static str {
    match payload {
        RunEventPayload::RunStarted { .. } => "run.started",
        RunEventPayload::NodeStarted { .. } => "node.started",
        RunEventPayload::NodeProgress { .. } => "node.progress",
        RunEventPayload::NodeDelta { .. } => "node.delta",
        RunEventPayload::NodeWaiting { .. } => "node.waiting",
        RunEventPayload::NodeAnswered { .. } => "node.answered",
        RunEventPayload::NodeDone { .. } => "node.done",
        RunEventPayload::NodeFailed { .. } => "node.failed",
        RunEventPayload::NodeSkipped { .. } => "node.skipped",
        RunEventPayload::RunFinished { .. } => "run.finished",
    }
}

/// Flatten `{"<Variant>": {fields...}}` (the derive's enum shape) into a
/// bare fields object stamped with `run_id`/`instance`/`flow`, matching
/// DESIGN.md §5.4's event payload shape.
fn run_event_json(run_id: &RunId, instance_id: &InstanceId, flow: &str, payload: &RunEventPayload) -> JsonValue {
    let wrapped = json_value(payload);
    let mut fields = match wrapped {
        JsonValue::Object(mut outer) => outer
            .drain()
            .next()
            .map(|(_, inner)| match inner {
                JsonValue::Object(fields) => fields,
                _ => HashMap::new(),
            })
            .unwrap_or_default(),
        _ => HashMap::new(),
    };
    fields.insert("run_id".to_string(), JsonValue::String(run_id.0.clone()));
    fields.insert("instance".to_string(), JsonValue::String(instance_id.0.clone()));
    fields.insert("flow".to_string(), JsonValue::String(flow.to_string()));
    JsonValue::Object(fields)
}

pub(crate) fn json_value<T: SerJson>(value: &T) -> JsonValue {
    JsonValue::deserialize_json(&value.serialize_json()).unwrap_or(JsonValue::Null)
}
