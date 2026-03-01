use makepad_live_id::LiveId;
use makepad_studio_backend::log_store::{
    AppendLogEntry, LogQuery, LogStore, ProfilerQuery, ProfilerStore, SAMPLE_TYPE_EVENT,
    SAMPLE_TYPE_GPU,
};
use makepad_studio_protocol::backend_protocol::{
    ClientId, EventSample, GPUSample, LogSource, QueryId,
};
use makepad_studio_protocol::LogLevel;

#[test]
fn log_store_filters_by_build_source_and_pattern() {
    let build_a = QueryId::new(ClientId(1), 1);
    let build_b = QueryId::new(ClientId(2), 1);
    let mut store = LogStore::default();

    store.append(AppendLogEntry {
        build_id: Some(build_a),
        level: LogLevel::Log,
        source: LogSource::Cargo,
        message: "compile ok".to_string(),
        file_name: None,
        line: None,
        column: None,
        timestamp: Some(1.0),
    });
    store.append(AppendLogEntry {
        build_id: Some(build_b),
        level: LogLevel::Warning,
        source: LogSource::Studio,
        message: "lint warning".to_string(),
        file_name: Some("repo/src/lib.rs".to_string()),
        line: Some(10),
        column: Some(3),
        timestamp: Some(2.0),
    });

    let query = LogQuery {
        build_id: Some(build_b),
        level: Some("warning".to_string()),
        source: Some(LogSource::Studio),
        file: Some("repo/src/lib.rs".to_string()),
        pattern: Some("lint".to_string()),
        since_index: Some(1),
    };
    let results = store.query(&query);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, 1);
    assert_eq!(results[0].1.source, LogSource::Studio);
}

#[test]
fn profiler_store_filters_and_downsamples() {
    let build = QueryId::new(ClientId(4), 9);
    let mut store = ProfilerStore::default();

    for i in 0..20 {
        store.append_event(
            Some(build),
            EventSample {
                at: i as f64,
                label: LiveId::from_str("event.tick"),
                ..Default::default()
            },
        );
    }
    for i in 0..10 {
        store.append_gpu(
            Some(build),
            GPUSample {
                at: i as f64,
                label: LiveId::from_str("gpu.frame"),
                ..Default::default()
            },
        );
    }

    let query = ProfilerQuery {
        build_id: Some(build),
        sample_type: Some(SAMPLE_TYPE_EVENT),
        time_start: Some(4.0),
        time_end: Some(15.0),
        max_samples: Some(5),
    };
    let (events, gpu, gc, total) = store.query(&query);
    assert!(!events.is_empty());
    assert!(events.len() <= 5);
    assert!(gpu.is_empty());
    assert!(gc.is_empty());
    assert_eq!(total, 12);

    let gpu_query = ProfilerQuery {
        build_id: Some(build),
        sample_type: Some(SAMPLE_TYPE_GPU),
        time_start: None,
        time_end: None,
        max_samples: Some(3),
    };
    let (_events, gpu, _gc, total) = store.query(&gpu_query);
    assert_eq!(gpu.len(), 3);
    assert_eq!(total, 10);
}
