//! `/v1/events` end-to-end: publish notification, kind filtering, long-poll
//! wakeup/timeout, cursor resume without duplicates, retention gaps, auth,
//! shutdown behavior, and slow/parked clients — all over real sockets
//! against a real server instance.

mod common;

use common::*;
use makepad_asset_store::json::Value;
use makepad_asset_data::AssetId;
use std::time::{Duration, Instant};

fn events_of(resp: &Response) -> Vec<Value> {
    resp.json().get("events").and_then(Value::as_arr).map(<[Value]>::to_vec).expect("events array")
}

fn cursor_of(resp: &Response) -> String {
    resp.str_field("cursor")
}

fn gap_of(resp: &Response) -> bool {
    resp.json().get("gap").and_then(Value::as_bool).expect("gap flag")
}

fn ev_str<'v>(ev: &'v Value, key: &str) -> Option<&'v str> {
    ev.get(key).and_then(Value::as_str)
}

/// Mint a principal allowed to upload/register/publish/alias in `ns`, and
/// hand back control+data clients bearing its token.
fn content_clients(ts: &TestServer, admin: &str, ns: &str) -> (Client, Client) {
    let mut admin_control = ts.control(Some(admin));
    let token = principal_with(
        &mut admin_control,
        &[
            ("blob_write", ns),
            ("asset_register", ns),
            ("asset_publish", ns),
            ("alias_write", ns),
        ],
    );
    (ts.control(Some(&token)), ts.data(Some(&token)))
}

/// Publish a prop whose annotation declares `kind`, and return its ids.
fn publish_annotated(
    control: &mut Client,
    data: &mut Client,
    ns: &str,
    alias: &str,
    seed: u8,
    kind: &str,
) -> (String, String) {
    let glb = vec![seed; 600];
    let thumb = vec![seed ^ 0xff; 300];
    let (asset_id, revision) = publish_prop_http(control, data, ns, alias, &glb, &thumb);
    let r = control.put_json(
        &format!("/v1/assets/{asset_id}/annotation"),
        &jobj(vec![
            ("title", jstr(format!("clip {seed}"))),
            ("kind", jstr(kind)),
        ]),
    );
    assert_eq!(r.status, 204, "{}", String::from_utf8_lossy(&r.body));
    (asset_id, revision)
}

#[test]
fn publish_and_alias_emit_events_with_cursor_resume_and_no_duplicates() {
    let ts = start_server("events_publish");
    let admin = ts.admin_token();
    let (mut control, mut data) = content_clients(&ts, &admin, "stock");

    // Tail resume point before any activity.
    let r = control.get("/v1/events");
    assert_eq!(r.status, 200);
    assert!(events_of(&r).is_empty());
    assert!(!gap_of(&r));
    let start_cursor = cursor_of(&r);

    let (asset_id, revision) =
        publish_annotated(&mut control, &mut data, "stock", "stock/clip-a", 1, "video");

    // Everything since the resume point: publish, annotation, alias.
    let r = control.get(&format!("/v1/events?cursor={start_cursor}"));
    assert_eq!(r.status, 200);
    let events = events_of(&r);
    let kinds: Vec<&str> = events.iter().filter_map(|e| ev_str(e, "kind")).collect();
    assert_eq!(kinds, vec!["asset_published", "alias_set", "annotation_set"]);
    assert!(!gap_of(&r));
    // The publish event names the exact asset/revision identity.
    assert_eq!(ev_str(&events[0], "asset_id"), Some(asset_id.as_str()));
    assert_eq!(ev_str(&events[0], "revision"), Some(revision.as_str()));
    assert_eq!(ev_str(&events[0], "ns"), Some("stock"));
    assert!(events[0].get("ts_ms").and_then(Value::as_u64).is_some());
    // The alias event carries the alias and (annotation not yet set at that
    // point) no content kind; the annotation event carries the kind.
    assert_eq!(ev_str(&events[1], "alias"), Some("stock/clip-a"));
    assert_eq!(ev_str(&events[2], "content_kind"), Some("video"));
    // Sequences strictly increase.
    let seqs: Vec<u64> = events.iter().filter_map(|e| e.get("seq").and_then(Value::as_u64)).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");

    // Resuming from the returned cursor replays nothing.
    let done_cursor = cursor_of(&r);
    let r = control.get(&format!("/v1/events?cursor={done_cursor}"));
    assert!(events_of(&r).is_empty());
    assert!(!gap_of(&r));
    assert_eq!(cursor_of(&r), done_cursor);

    // A malformed cursor is refused, not misread.
    let r = control.get("/v1/events?cursor=garbage");
    assert_eq!(r.status, 400);
}

#[test]
fn kind_filter_selects_video_and_advances_past_foreign_events() {
    let ts = start_server("events_filter");
    let admin = ts.admin_token();
    let (mut control, mut data) = content_clients(&ts, &admin, "stock");

    let start = cursor_of(&control.get("/v1/events"));
    let (video_asset, _) =
        publish_annotated(&mut control, &mut data, "stock", "stock/vid", 11, "video");
    let (_mesh_asset, _) =
        publish_annotated(&mut control, &mut data, "stock", "stock/mesh", 12, "mesh");

    let r = control.get(&format!("/v1/events?cursor={start}&kind=video"));
    let events = events_of(&r);
    // Publish/alias events precede their annotation, so their content kind is
    // unknown (conservatively included); the mesh ANNOTATION event, whose
    // kind is known, is filtered out. Every surviving known kind is video.
    assert!(!events.is_empty());
    for ev in &events {
        if let Some(ck) = ev_str(ev, "content_kind") {
            assert_eq!(ck, "video");
        }
    }
    assert!(events.iter().any(|e| {
        ev_str(e, "kind") == Some("annotation_set")
            && ev_str(e, "asset_id") == Some(video_asset.as_str())
    }));
    assert!(!events.iter().any(|e| ev_str(e, "content_kind") == Some("mesh")));
    // The cursor advanced past the filtered-out mesh events: replaying from
    // it returns nothing new.
    let cursor = cursor_of(&r);
    let r = control.get(&format!("/v1/events?cursor={cursor}&kind=video"));
    assert!(events_of(&r).is_empty());

    // After annotation, a new publish of the SAME asset carries its kind, so
    // an exact-kind subscriber sees it even under the filter.
    let glb = vec![21u8; 700];
    let thumb = vec![22u8; 300];
    let ast: AssetId = video_asset.parse().unwrap();
    let bytes = prop_manifest(ast, &glb, &thumb).to_canonical_bytes().unwrap();
    for b in [&glb, &thumb] {
        let r = data.post_bytes("/v1/blobs?ns=stock", b);
        assert_eq!(r.status, 201);
    }
    let r = control.post_bytes(&format!("/v1/assets/{video_asset}/revisions"), &bytes);
    assert_eq!(r.status, 201);
    let rev2 = r.str_field("revision");
    let r = control.post_json(
        &format!("/v1/assets/{video_asset}/revisions/{rev2}/publish"),
        &jobj(vec![]),
    );
    assert_eq!(r.status, 200);
    let r = control.get(&format!("/v1/events?cursor={cursor}&kind=video"));
    let events = events_of(&r);
    assert_eq!(events.len(), 1);
    assert_eq!(ev_str(&events[0], "kind"), Some("asset_published"));
    assert_eq!(ev_str(&events[0], "content_kind"), Some("video"));
    assert_eq!(ev_str(&events[0], "revision"), Some(rev2.as_str()));

    // Unknown kind value is refused.
    let r = control.get(&format!("/v1/events?cursor={cursor}&kind=blob"));
    assert_eq!(r.status, 400);
}

#[test]
fn long_poll_wakes_on_publish_and_times_out_empty() {
    let ts = start_server("events_longpoll");
    let admin = ts.admin_token();
    let (mut control, mut data) = content_clients(&ts, &admin, "stock");
    let start = cursor_of(&control.get("/v1/events"));

    // Timeout path: nothing happens, bounded wait, empty response.
    let t0 = Instant::now();
    let r = control.get(&format!("/v1/events?cursor={start}&wait=300"));
    assert_eq!(r.status, 200);
    assert!(events_of(&r).is_empty());
    assert!(!gap_of(&r));
    let waited = t0.elapsed();
    assert!(waited >= Duration::from_millis(250), "waited {waited:?}");
    assert!(waited < Duration::from_secs(5), "waited {waited:?}");

    // Wakeup path: a parked poll returns as soon as a publish lands.
    let addr = ts.server.control_addr();
    let admin_t = admin.clone();
    let start_t = start.clone();
    let waiter = std::thread::spawn(move || {
        let mut c = Client::new(addr, Some(&admin_t));
        let t0 = Instant::now();
        let r = c.get(&format!("/v1/events?cursor={start_t}&wait=10000"));
        (r, t0.elapsed())
    });
    // Give the waiter time to park, then publish.
    std::thread::sleep(Duration::from_millis(200));
    publish_annotated(&mut control, &mut data, "stock", "stock/wake", 31, "video");
    let (r, waited) = waiter.join().unwrap();
    assert_eq!(r.status, 200);
    assert!(!events_of(&r).is_empty());
    assert!(
        waited < Duration::from_secs(5),
        "long poll should wake on publish, waited {waited:?}"
    );
}

#[test]
fn retention_gap_forces_resync() {
    // Tiny journal: publishing more events than it retains invalidates old
    // cursors explicitly.
    let ts = start_server_with("events_gap", |cfg| {
        cfg.event_journal_cap = 4;
    });
    let admin = ts.admin_token();
    let (mut control, mut data) = content_clients(&ts, &admin, "stock");

    let start = cursor_of(&control.get("/v1/events"));
    // Each publish emits 3 events (publish, alias, annotation); 3 rounds
    // overflow a cap of 4.
    for seed in 41..44u8 {
        publish_annotated(
            &mut control,
            &mut data,
            "stock",
            &format!("stock/gap-{seed}"),
            seed,
            "video",
        );
    }
    let r = control.get(&format!("/v1/events?cursor={start}"));
    assert_eq!(r.status, 200);
    assert!(gap_of(&r), "evicted cursor must report a gap");
    assert!(events_of(&r).is_empty());
    // The gap cursor is a clean tail resume point.
    let cursor = cursor_of(&r);
    let r = control.get(&format!("/v1/events?cursor={cursor}"));
    assert!(!gap_of(&r));
    assert!(events_of(&r).is_empty());

    // A cursor from another journal life (foreign epoch) is a gap too.
    let foreign = format!("{}-1", "ab".repeat(8));
    let r = control.get(&format!("/v1/events?cursor={foreign}"));
    assert_eq!(r.status, 200);
    assert!(gap_of(&r));
}

#[test]
fn batch_limit_pages_without_loss() {
    let ts = start_server("events_batch");
    let admin = ts.admin_token();
    let (mut control, mut data) = content_clients(&ts, &admin, "stock");
    let start = cursor_of(&control.get("/v1/events"));
    for seed in 51..54u8 {
        publish_annotated(
            &mut control,
            &mut data,
            "stock",
            &format!("stock/batch-{seed}"),
            seed,
            "audio",
        );
    }
    // 9 events total; page through 2 at a time and prove exact-once order.
    let mut cursor = start;
    let mut seqs: Vec<u64> = Vec::new();
    loop {
        let r = control.get(&format!("/v1/events?cursor={cursor}&limit=2"));
        assert_eq!(r.status, 200);
        let events = events_of(&r);
        assert!(events.len() <= 2);
        cursor = cursor_of(&r);
        if events.is_empty() {
            break;
        }
        seqs.extend(events.iter().filter_map(|e| e.get("seq").and_then(Value::as_u64)));
    }
    assert_eq!(seqs.len(), 9);
    assert!(seqs.windows(2).all(|w| w[1] == w[0] + 1), "{seqs:?}");
}

#[test]
fn events_require_auth_and_refuse_bad_params() {
    let ts = start_server("events_auth");
    let admin = ts.admin_token();
    let mut control = ts.control(Some(&admin));

    // No token, garbage token: the uniform 401.
    let mut anon = ts.control(None);
    assert_eq!(anon.get("/v1/events").status, 401);
    let bogus = format!("mpat_{}", "ab".repeat(32));
    let mut bad = ts.control(Some(&bogus));
    assert_eq!(bad.get("/v1/events").status, 401);

    // Authenticated: parameter refusals are explicit 400s.
    assert_eq!(control.get("/v1/events?wait=abc").status, 400);
    assert_eq!(control.get("/v1/events?limit=0").status, 400);
    assert_eq!(control.get("/v1/events?kind=nope").status, 400);
    assert_eq!(control.get("/v1/events?cursor=zz-1").status, 400);
    // Method discipline: POST is not a read.
    let r = control.post_json("/v1/events", &jobj(vec![]));
    assert_eq!(r.status, 405);
}

#[test]
fn shutdown_releases_parked_long_polls_promptly() {
    let ts = start_server("events_shutdown");
    let admin = ts.admin_token();
    let mut control = ts.control(Some(&admin));
    let start = cursor_of(&control.get("/v1/events"));

    // Park three long-polls (the "slow clients"), then shut the server down.
    let mut waiters = Vec::new();
    for _ in 0..3 {
        let addr = ts.server.control_addr();
        let token = admin.clone();
        let cursor = start.clone();
        waiters.push(std::thread::spawn(move || {
            let mut c = Client::new(addr, Some(&token));
            let t0 = Instant::now();
            let r = c.get(&format!("/v1/events?cursor={cursor}&wait=30000"));
            (r.status, t0.elapsed())
        }));
    }
    std::thread::sleep(Duration::from_millis(300));
    let mut server = ts;
    let t0 = Instant::now();
    server.server.shutdown();
    let shutdown_took = t0.elapsed();
    assert!(
        shutdown_took < Duration::from_secs(10),
        "shutdown must not wait out 30s long-polls, took {shutdown_took:?}"
    );
    for w in waiters {
        let (status, waited) = w.join().unwrap();
        // Parked polls answer with an empty batch before the socket closes.
        assert_eq!(status, 200);
        assert!(waited < Duration::from_secs(10), "waited {waited:?}");
    }
}

#[test]
fn parked_waiters_beyond_cap_degrade_to_immediate_polls() {
    let ts = start_server_with("events_waiter_cap", |cfg| {
        cfg.event_max_waiters = 1;
    });
    let admin = ts.admin_token();
    let mut control = ts.control(Some(&admin));
    let start = cursor_of(&control.get("/v1/events"));

    // First waiter parks and holds the only slot.
    let addr = ts.server.control_addr();
    let token = admin.clone();
    let cursor = start.clone();
    let parked = std::thread::spawn(move || {
        let mut c = Client::new(addr, Some(&token));
        c.get(&format!("/v1/events?cursor={cursor}&wait=2000")).status
    });
    std::thread::sleep(Duration::from_millis(200));
    // Over the cap: this poll answers immediately instead of parking.
    let t0 = Instant::now();
    let r = control.get(&format!("/v1/events?cursor={start}&wait=2000"));
    assert_eq!(r.status, 200);
    assert!(events_of(&r).is_empty());
    assert!(
        t0.elapsed() < Duration::from_millis(1500),
        "over-cap poll must not park, took {:?}",
        t0.elapsed()
    );
    assert_eq!(parked.join().unwrap(), 200);
}
