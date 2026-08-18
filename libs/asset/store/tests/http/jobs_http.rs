//! Job and worker protocol over HTTP: enqueue, claim, heartbeat/progress,
//! success with results, retry/failure, hierarchical + dependency
//! cancellation, listing, and lease expiry via the janitor.

mod common;

use common::*;
use makepad_asset_store::json::Value;

fn setup() -> (TestServer, Client, Client, Client) {
    let ts = start_server("jobs");
    let token = ts.admin_token();
    let mut admin = ts.control(Some(&token));
    let enq = principal_with(&mut admin, &[("job_enqueue", "demo"), ("job_cancel", "demo")]);
    let wrk = principal_with(&mut admin, &[("job_worker", "demo")]);
    let enqueuer = ts.control(Some(&enq));
    let worker = ts.control(Some(&wrk));
    (ts, admin, enqueuer, worker)
}

fn enqueue(client: &mut Client, kind: &str, extra: Vec<(&str, Value)>) -> String {
    let mut pairs = vec![
        ("namespace", jstr("demo")),
        ("kind", jstr(kind)),
        ("body", jobj(vec![("step", jstr(kind))])),
    ];
    pairs.extend(extra);
    let r = client.post_json("/v1/jobs", &jobj(pairs));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    r.str_field("job")
}

fn claim(worker: &mut Client) -> Value {
    let r = worker.post_json("/v1/worker/claim", &jobj(vec![("lease_ms", Value::Int(60_000))]));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
}

#[test]
fn enqueue_claim_progress_succeed() {
    let (_ts, _admin, mut enqueuer, mut worker) = setup();
    let job = enqueue(&mut enqueuer, "bake.thumbnail", vec![("priority", Value::Int(5))]);

    // Status for the enqueuer: pending, envelope metadata surfaced.
    let r = enqueuer.get(&format!("/v1/jobs/{job}"));
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("state"), "pending");
    assert_eq!(r.str_field("kind"), "bake.thumbnail");
    assert_eq!(r.str_field("namespace"), "demo");

    // Claim: the worker receives the payload body and the namespace.
    let claimed = claim(&mut worker);
    assert_eq!(claimed.get("job").unwrap().as_str(), Some(job.as_str()));
    assert_eq!(claimed.get("namespace").unwrap().as_str(), Some("demo"));
    assert_eq!(claimed.get("attempt").unwrap().as_i64(), Some(1));
    assert_eq!(
        claimed.get("body").unwrap().get("step").unwrap().as_str(),
        Some("bake.thumbnail")
    );
    let r = enqueuer.get(&format!("/v1/jobs/{job}"));
    assert_eq!(r.str_field("state"), "running");

    // Heartbeat extends the lease and records progress.
    let r = worker.post_json(
        "/v1/worker/heartbeat",
        &jobj(vec![
            ("job", jstr(job.clone())),
            ("extend_ms", Value::Int(60_000)),
            ("progress", jobj(vec![
                ("permille", Value::Int(500)),
                ("note", jstr("halfway")),
            ])),
        ]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert!(r.json().get("lease_expires_ms").unwrap().as_i64().unwrap() > 0);
    let r = enqueuer.get(&format!("/v1/jobs/{job}"));
    let progress = r.json().get("progress").unwrap().clone();
    assert_eq!(progress.get("permille").unwrap().as_i64(), Some(500));
    assert_eq!(progress.get("note").unwrap().as_str(), Some("halfway"));

    // Success with a result document.
    let r = worker.post_json(
        "/v1/worker/succeed",
        &jobj(vec![
            ("job", jstr(job.clone())),
            ("result", jobj(vec![("blob", jstr("sha256:cafe"))])),
        ]),
    );
    assert_eq!(r.status, 200);
    let r = enqueuer.get(&format!("/v1/jobs/{job}"));
    assert_eq!(r.str_field("state"), "succeeded");
    let result = r.json().get("result").unwrap().clone();
    assert_eq!(result.get("outcome").unwrap().as_str(), Some("succeeded"));
    assert_eq!(result.get("attempt").unwrap().as_i64(), Some(1));
    assert_eq!(result.get("body").unwrap().get("blob").unwrap().as_str(), Some("sha256:cafe"));
    // One closed attempt on record.
    let attempts = r.json().get("attempts").unwrap().as_arr().unwrap().to_vec();
    assert_eq!(attempts.len(), 1);
    assert!(attempts[0].get("ended_ms").unwrap().as_i64().is_some());

    // The queue is empty again.
    assert!(claim(&mut worker).get("job").unwrap().as_str().is_none());
}

#[test]
fn worker_kind_filter_is_atomic_and_strict() {
    let (_ts, _admin, mut enqueuer, mut worker) = setup();
    let music = enqueue(
        &mut enqueuer,
        "music.generate",
        vec![("priority", Value::Int(50))],
    );
    let video = enqueue(&mut enqueuer, "video.generate", vec![]);

    let r = worker.post_json(
        "/v1/worker/claim",
        &jobj(vec![
            ("lease_ms", Value::Int(60_000)),
            ("kinds", Value::Arr(vec![jstr("video.generate")])),
        ]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json().get("job").and_then(Value::as_str), Some(video.as_str()));
    assert_eq!(
        enqueuer.get(&format!("/v1/jobs/{music}")).str_field("state"),
        "pending",
        "foreign worker kind must remain untouched"
    );

    // Legacy/unrestricted workers can still claim the remaining job.
    let unrestricted = claim(&mut worker);
    assert_eq!(unrestricted.get("job").and_then(Value::as_str), Some(music.as_str()));

    for kinds in [
        Value::Arr(vec![]),
        Value::Arr(vec![jstr("video.generate"), jstr("video.generate")]),
        Value::Arr(vec![jstr("Bad Kind")]),
    ] {
        let r = worker.post_json(
            "/v1/worker/claim",
            &jobj(vec![("lease_ms", Value::Int(60_000)), ("kinds", kinds)]),
        );
        assert_eq!(r.status, 400);
    }
}

#[test]
fn retry_then_terminal_failure_with_error() {
    let (_ts, _admin, mut enqueuer, mut worker) = setup();
    let job = enqueue(&mut enqueuer, "retry.me", vec![("max_attempts", Value::Int(2))]);

    let c = claim(&mut worker);
    assert_eq!(c.get("attempt").unwrap().as_i64(), Some(1));
    // First failure re-queues immediately (no backoff requested).
    let r = worker.post_json(
        "/v1/worker/fail",
        &jobj(vec![("job", jstr(job.clone())), ("retry_delay_ms", Value::Int(0))]),
    );
    assert_eq!(r.status, 200);
    assert_eq!(r.str_field("state"), "pending");

    let c = claim(&mut worker);
    assert_eq!(c.get("attempt").unwrap().as_i64(), Some(2));
    // Second failure exhausts attempts: terminal, error document recorded.
    let r = worker.post_json(
        "/v1/worker/fail",
        &jobj(vec![
            ("job", jstr(job.clone())),
            ("error", jobj(vec![("reason", jstr("model oom"))])),
        ]),
    );
    assert_eq!(r.str_field("state"), "failed");
    let r = enqueuer.get(&format!("/v1/jobs/{job}"));
    assert_eq!(r.str_field("state"), "failed");
    let result = r.json().get("result").unwrap().clone();
    assert_eq!(result.get("outcome").unwrap().as_str(), Some("failed"));
    assert_eq!(result.get("body").unwrap().get("reason").unwrap().as_str(), Some("model oom"));
}

#[test]
fn hierarchical_cancel_and_doomed_dependents() {
    let (_ts, _admin, mut enqueuer, mut worker) = setup();

    // Parent tree: cancelling the root cancels the child.
    let parent = enqueue(&mut enqueuer, "tree.parent", vec![]);
    let child = enqueue(&mut enqueuer, "tree.child", vec![("parent", jstr(parent.clone()))]);
    let r = enqueuer.post_json(&format!("/v1/jobs/{parent}/cancel"), &jobj(vec![]));
    assert_eq!(r.status, 200);
    assert_eq!(r.json().get("cancelled").unwrap().as_i64(), Some(2));
    for j in [&parent, &child] {
        assert_eq!(enqueuer.get(&format!("/v1/jobs/{j}")).str_field("state"), "cancelled");
    }

    // Dependency doom: B depends on A; cancelling A dooms B at claim time.
    let a = enqueue(&mut enqueuer, "dep.a", vec![]);
    let b = enqueue(&mut enqueuer, "dep.b", vec![("deps", Value::Arr(vec![jstr(a.clone())]))]);
    let r = enqueuer.post_json(&format!("/v1/jobs/{a}/cancel"), &jobj(vec![]));
    assert_eq!(r.status, 200);
    // Nothing claimable: B is doomed by its dependency, and the claim pass
    // is what propagates that.
    assert!(claim(&mut worker).get("job").unwrap().as_str().is_none());
    assert_eq!(enqueuer.get(&format!("/v1/jobs/{b}")).str_field("state"), "cancelled");

    // Cancelled jobs refuse worker reports.
    let c = enqueue(&mut enqueuer, "cancel.mid", vec![]);
    let claimed = claim(&mut worker);
    assert_eq!(claimed.get("job").unwrap().as_str(), Some(c.as_str()));
    let r = enqueuer.post_json(&format!("/v1/jobs/{c}/cancel"), &jobj(vec![]));
    assert_eq!(r.status, 200);
    let r = worker.post_json("/v1/worker/succeed", &jobj(vec![("job", jstr(c.clone()))]));
    assert_eq!(r.status, 409, "report after cancel must lose the lease");
}

#[test]
fn lease_expiry_requeues_via_janitor() {
    let (_ts, _admin, mut enqueuer, mut worker) = setup();
    let job = enqueue(&mut enqueuer, "expire.me", vec![("max_attempts", Value::Int(2))]);
    // Claim with the shortest possible lease and let the 50ms janitor reap it.
    let r = worker.post_json("/v1/worker/claim", &jobj(vec![("lease_ms", Value::Int(1))]));
    assert_eq!(r.status, 200);
    assert_eq!(r.json().get("job").unwrap().as_str(), Some(job.as_str()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let state = enqueuer.get(&format!("/v1/jobs/{job}")).str_field("state");
        if state == "pending" {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "janitor never expired the lease");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The stale holder's late report is refused; the retry can be claimed.
    let r = worker.post_json("/v1/worker/succeed", &jobj(vec![("job", jstr(job.clone()))]));
    assert_eq!(r.status, 409);
    let c = claim(&mut worker);
    assert_eq!(c.get("attempt").unwrap().as_i64(), Some(2));
}

#[test]
fn job_listing_scopes() {
    let (ts, mut admin, mut enqueuer, _worker) = setup();
    let j1 = enqueue(&mut enqueuer, "list.one", vec![]);
    let j2 = enqueue(&mut enqueuer, "list.two", vec![]);

    // Without a namespace filter: the caller's own jobs, newest first.
    let r = enqueuer.get("/v1/jobs");
    let jobs = r.json().get("jobs").unwrap().as_arr().unwrap().to_vec();
    assert_eq!(jobs.len(), 2);
    let ids: Vec<String> = jobs
        .iter()
        .map(|j| j.get("job").unwrap().as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&j1) && ids.contains(&j2));

    // Namespace listing requires a job capability on that namespace.
    let r = enqueuer.get("/v1/jobs?ns=demo&limit=1");
    assert_eq!(r.json().get("jobs").unwrap().as_arr().unwrap().len(), 1);
    let outsider = principal_with(&mut admin, &[]);
    let mut outsider = ts.control(Some(&outsider));
    assert_eq!(outsider.get("/v1/jobs?ns=demo").status, 403);
    // And an uninvolved principal's own-jobs listing is empty.
    let r = outsider.get("/v1/jobs");
    assert_eq!(r.json().get("jobs").unwrap().as_arr().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// advertised job profiles
// ---------------------------------------------------------------------------

#[test]
fn job_profiles_advertise_filter_and_require_auth() {
    let ts = start_server("job_profiles");
    let admin = ts.admin_token();
    let mut control = ts.control(Some(&admin));

    // Unauthenticated: uniform 401. Wrong method: 405.
    let mut anon = ts.control(None);
    assert_eq!(anon.get("/v1/job-profiles").status, 401);
    let r = control.post_json("/v1/job-profiles", &jobj(vec![]));
    assert_eq!(r.status, 405);

    // Default advertisement: the stock video profiles, complete shape.
    let r = control.get("/v1/job-profiles?domain=video");
    assert_eq!(r.status, 200);
    let profiles = r.json().get("profiles").and_then(Value::as_arr).map(<[Value]>::to_vec).unwrap();
    assert!(!profiles.is_empty());
    for p in &profiles {
        assert_eq!(p.get("domain").and_then(Value::as_str), Some("video"));
        assert_eq!(p.get("kind").and_then(Value::as_str), Some("video.generate"));
        assert_eq!(p.get("namespace").and_then(Value::as_str), Some("gen"));
        assert!(p.get("id").and_then(Value::as_str).is_some());
        assert!(p.get("label").and_then(Value::as_str).is_some());
        let defaults = p.get("defaults").expect("defaults object");
        assert!(defaults.get("model").and_then(Value::as_str).is_some());
        assert!(defaults.get("width").and_then(Value::as_u64).is_some());
    }

    // A domain nothing advertises filters to empty (not an error).
    let r = control.get("/v1/job-profiles?domain=hologram");
    assert_eq!(r.status, 200);
    assert_eq!(
        r.json().get("profiles").and_then(Value::as_arr).map(|a| a.len()),
        Some(0)
    );

    // No filter returns every profile.
    let r = control.get("/v1/job-profiles");
    let all = r.json().get("profiles").and_then(Value::as_arr).map(|a| a.len()).unwrap();
    assert!(all >= profiles.len());
}
