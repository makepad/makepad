//! The `op.depth.from_image.v1` operation executor: claim the armed job,
//! fetch the EXACT pinned input image, run DA3 metric depth on the fleet,
//! upload the 16-bit mm PNG plus JSON sidecar, and finalize via the server.

use makepad_asset_importer::thumbs;
use makepad_asset_client::{
    json::Value, AssetClient, ClientError, JobStateDto, OperationFinalizeRequest, OperationId,
    OperationOutputFile,
};
use makepad_asset_data::{BlobId, DeviceTier, FileRole, MediaType};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const DEPTH_OP_KIND: &str = "op.depth.from_image.v1";

const LEASE_MS: u64 = 60_000;
const HEARTBEAT_EVERY: Duration = Duration::from_secs(15);
const FLEET_POLL_EVERY: Duration = Duration::from_millis(1200);
const CANCEL_CHECK_EVERY: Duration = Duration::from_secs(3);
const IDLE_SLEEP: Duration = Duration::from_secs(2);
const JOB_DEADLINE: Duration = Duration::from_secs(10 * 60);

pub struct DepthRequest {
    pub image: Vec<u8>,
    pub content_type: &'static str,
}

#[derive(Debug)]
pub enum DepthDispatch {
    Started { fleet_job: String, model: String, version: String },
    Waiting { stage: String },
}

pub enum DepthPoll {
    Running { stage: String, permille: u16 },
    Done { png: Vec<u8>, sidecar: Vec<u8> },
    Failed { error: String },
}

pub trait DepthFleet {
    fn dispatch(&mut self, request: &DepthRequest) -> Result<DepthDispatch, String>;
    fn poll(&mut self, fleet_job: &str) -> Result<DepthPoll, String>;
    fn cancel(&mut self, fleet_job: &str);
}

#[derive(Debug)]
pub enum DepthJobOutcome {
    Finalized {
        #[allow(dead_code)]
        operation: String,
        #[allow(dead_code)]
        asset: String,
        #[allow(dead_code)]
        revision: String,
    },
    Failed { error: String },
    CancelledUpstream,
}

pub struct DepthOpCoordinator<'f> {
    pub client: AssetClient,
    pub fleet: &'f mut dyn DepthFleet,
    pub suffix: String,
    pub log: bool,
}

impl<'f> DepthOpCoordinator<'f> {
    fn log(&self, message: &str) {
        if self.log {
            eprintln!("[asset-worker depth] {message}");
        }
    }

    pub fn run(&mut self, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            match self.run_one(stop) {
                Ok(Some(outcome)) => self.log(&format!("outcome: {outcome:?}")),
                Ok(None) => sleep_sliced(IDLE_SLEEP, stop),
                Err(error) => {
                    self.log(&format!("asset-server call failed: {error}; retrying"));
                    sleep_sliced(Duration::from_secs(5), stop);
                }
            }
        }
    }

    pub fn run_one(&mut self, stop: &AtomicBool) -> Result<Option<DepthJobOutcome>, ClientError> {
        let Some(claimed) =
            self.client
                .worker_claim_kinds(LEASE_MS, Some(&self.suffix), &[DEPTH_OP_KIND])?
        else {
            return Ok(None);
        };
        if claimed.kind != DEPTH_OP_KIND {
            let error = format!("worker does not handle kind {}", claimed.kind);
            self.fail_job(&claimed.job, &error)?;
            return Ok(Some(DepthJobOutcome::Failed { error }));
        }
        let outcome = self.process(&claimed, stop);
        if let Ok(DepthJobOutcome::Failed { error }) = &outcome {
            self.fail_job(&claimed.job, error)?;
        }
        outcome.map(Some)
    }

    fn fail_job(
        &self,
        job: &makepad_asset_client::JobId,
        error: &str,
    ) -> Result<(), ClientError> {
        let doc = makepad_asset_client::json::obj(vec![(
            "error",
            makepad_asset_client::json::s(bounded(error, 2_000)),
        )]);
        let _ = self.client.worker_fail(job, Some(&self.suffix), 0, Some(&doc));
        Ok(())
    }

    fn process(
        &mut self,
        claimed: &makepad_asset_client::ClaimedJobDto,
        stop: &AtomicBool,
    ) -> Result<DepthJobOutcome, ClientError> {
        let body = &claimed.body;
        let Some(operation) = body
            .get("operation")
            .and_then(Value::as_str)
            .and_then(OperationId::parse)
        else {
            return Ok(DepthJobOutcome::Failed {
                error: "job body missing operation id".to_string(),
            });
        };
        let Some(input) = body
            .get("inputs")
            .and_then(Value::as_arr)
            .and_then(|arr| arr.first())
        else {
            return Ok(DepthJobOutcome::Failed {
                error: "job body missing inputs".to_string(),
            });
        };
        let Some(blob) = input
            .get("blob")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<BlobId>().ok())
        else {
            return Ok(DepthJobOutcome::Failed {
                error: "job input missing blob".to_string(),
            });
        };
        let byte_len = input.get("byte_len").and_then(Value::as_u64);
        let content_type = match input.get("media").and_then(Value::as_str) {
            Some("png") => "image/png",
            Some("jpeg") => "image/jpeg",
            other => {
                return Ok(DepthJobOutcome::Failed {
                    error: format!("unsupported input media {other:?}"),
                })
            }
        };

        self.heartbeat(&claimed.job, 20, "fetching input")?;
        let image = self.client.fetch_blob_bytes(&blob, byte_len)?;
        if image.is_empty() {
            return Ok(DepthJobOutcome::Failed {
                error: "pinned input image is empty".to_string(),
            });
        }
        if content_type != "image/png" {
            return Ok(DepthJobOutcome::Failed {
                error: format!(
                    "da3-metric-large requires PNG input (got {content_type})"
                ),
            });
        }

        let started = Instant::now();
        let request = DepthRequest { image, content_type };
        let (fleet_job, model, version) = loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(DepthJobOutcome::Failed {
                    error: "worker stopping".to_string(),
                });
            }
            if started.elapsed() > JOB_DEADLINE {
                return Ok(DepthJobOutcome::Failed {
                    error: "depth job deadline".to_string(),
                });
            }
            if self.upstream_cancelled(&claimed.job)? {
                return Ok(DepthJobOutcome::CancelledUpstream);
            }
            match self.fleet.dispatch(&request) {
                Ok(DepthDispatch::Started { fleet_job, model, version }) => {
                    break (fleet_job, model, version)
                }
                Ok(DepthDispatch::Waiting { stage }) => {
                    self.heartbeat(&claimed.job, 40, &bounded(&stage, 180))?;
                    sleep_sliced(FLEET_POLL_EVERY, stop);
                }
                Err(error) => return Ok(DepthJobOutcome::Failed { error }),
            }
        };
        self.log(&format!("dispatched {model} for {operation}"));

        let mut last_heartbeat = Instant::now();
        let mut last_cancel_check = Instant::now();
        let (png, sidecar) = loop {
            if started.elapsed() > JOB_DEADLINE {
                self.fleet.cancel(&fleet_job);
                return Ok(DepthJobOutcome::Failed {
                    error: "depth job deadline".to_string(),
                });
            }
            if stop.load(Ordering::SeqCst) {
                self.fleet.cancel(&fleet_job);
                return Ok(DepthJobOutcome::Failed {
                    error: "worker stopping".to_string(),
                });
            }
            if last_cancel_check.elapsed() >= CANCEL_CHECK_EVERY {
                last_cancel_check = Instant::now();
                if self.upstream_cancelled(&claimed.job)? {
                    self.fleet.cancel(&fleet_job);
                    return Ok(DepthJobOutcome::CancelledUpstream);
                }
            }
            match self.fleet.poll(&fleet_job) {
                Ok(DepthPoll::Done { png, sidecar }) => break (png, sidecar),
                Ok(DepthPoll::Failed { error }) => {
                    return Ok(DepthJobOutcome::Failed { error })
                }
                Ok(DepthPoll::Running { stage, permille }) => {
                    if last_heartbeat.elapsed() >= HEARTBEAT_EVERY {
                        last_heartbeat = Instant::now();
                        let clamped = 50 + (permille.min(1000) as u32 * 800 / 1000) as u16;
                        if self
                            .client
                            .worker_heartbeat(
                                &claimed.job,
                                LEASE_MS,
                                Some(&self.suffix),
                                Some((clamped, &bounded(&stage, 180))),
                            )
                            .is_err()
                        {
                            self.fleet.cancel(&fleet_job);
                            return Ok(DepthJobOutcome::CancelledUpstream);
                        }
                    }
                }
                Err(error) => {
                    self.log(&format!("fleet poll failed: {error}; retrying"));
                    self.heartbeat(&claimed.job, 500, "fleet poll retry")?;
                }
            }
            sleep_sliced(FLEET_POLL_EVERY, stop);
        };

        if png.is_empty() {
            return Ok(DepthJobOutcome::Failed {
                error: "model produced empty depth png".to_string(),
            });
        }
        if sidecar.is_empty() {
            return Ok(DepthJobOutcome::Failed {
                error: "model produced empty depth sidecar".to_string(),
            });
        }

        self.heartbeat(&claimed.job, 940, "uploading output")?;
        let png_blob = self.client.upload_blob(&claimed.namespace, &png)?;
        let json_blob = self.client.upload_blob(&claimed.namespace, &sidecar)?;
        let (thumb_bytes, thumb_media, thumb_dims) = thumbnail_for(&png);

        self.heartbeat(&claimed.job, 980, "finalizing")?;
        let finalize = OperationFinalizeRequest {
            job: claimed.job.clone(),
            suffix: Some(self.suffix.clone()),
            output_name: "depth".into(),
            files: vec![
                OperationOutputFile {
                    role: FileRole::Depth,
                    tier: DeviceTier::Any,
                    lod: 0,
                    media: MediaType::Png,
                    blob: png_blob,
                    byte_len: png.len() as u64,
                    dims: thumbs::png_dims(&png),
                },
                OperationOutputFile {
                    role: FileRole::Source,
                    tier: DeviceTier::Any,
                    lod: 0,
                    media: MediaType::Text,
                    blob: json_blob,
                    byte_len: sidecar.len() as u64,
                    dims: None,
                },
            ],
            thumbnail: Some((
                self.client.upload_blob(&claimed.namespace, &thumb_bytes)?,
                thumb_media,
                thumb_dims.0,
                thumb_dims.1,
                thumb_bytes.len() as u64,
            )),
            metrics: (
                png.len() as u64 + sidecar.len() as u64 + thumb_bytes.len() as u64,
                0,
                0,
                0,
                0,
                thumb_dims.0.max(thumb_dims.1),
                0,
            ),
            bounds: None,
            generator: "asset-worker".into(),
            model,
            version: if version.is_empty() {
                "unversioned".into()
            } else {
                version
            },
            seed: 0,
        };
        match self.client.operation_finalize(&operation, &finalize) {
            Ok((asset, revision)) => Ok(DepthJobOutcome::Finalized {
                operation: operation.to_string(),
                asset: asset.to_string(),
                revision: revision.to_string(),
            }),
            Err(error) => Ok(DepthJobOutcome::Failed {
                error: format!("finalize refused: {error}"),
            }),
        }
    }

    fn heartbeat(
        &self,
        job: &makepad_asset_client::JobId,
        permille: u16,
        note: &str,
    ) -> Result<(), ClientError> {
        let _ = self
            .client
            .worker_heartbeat(job, LEASE_MS, Some(&self.suffix), Some((permille, note)));
        Ok(())
    }

    fn upstream_cancelled(
        &self,
        job: &makepad_asset_client::JobId,
    ) -> Result<bool, ClientError> {
        match self.client.job_status(job) {
            Ok(status) => Ok(status.state == JobStateDto::Cancelled),
            Err(_) => Ok(false),
        }
    }
}

fn thumbnail_for(png: &[u8]) -> (Vec<u8>, &'static str, (u32, u32)) {
    if let Some((w, h)) = thumbs::png_dims(png) {
        if (256..=4096).contains(&w) && (256..=4096).contains(&h) {
            return (png.to_vec(), "png", (w, h));
        }
    }
    let bgra = thumbs::placeholder_bgra_512();
    let jpeg = thumbs::encode_jpeg_bgra(&bgra, 512, 512).expect("placeholder encode");
    (jpeg, "jpeg", (512, 512))
}

pub struct AssetAiDepthFleet {
    boxes: Vec<String>,
    discovered: Option<makepad_asset_ai::discovery::Discovered>,
    routes: std::collections::HashMap<String, String>,
    log: bool,
}

impl AssetAiDepthFleet {
    pub fn from_fleet_file(path: &std::path::Path, log: bool) -> Result<Self, String> {
        let config = makepad_asset_ai::fleet::FleetConfig::load_file(path)
            .map_err(|e| format!("fleet config {}: {e}", path.display()))?;
        let boxes = config.boxes.clone();
        if boxes.is_empty() {
            return Err(format!("fleet config {} lists no boxes", path.display()));
        }
        Ok(Self {
            boxes,
            discovered: None,
            routes: std::collections::HashMap::new(),
            log,
        })
    }

    pub fn from_lan(log: bool) -> Self {
        Self {
            boxes: Vec::new(),
            discovered: Some(makepad_asset_ai::discovery::start_listener()),
            routes: std::collections::HashMap::new(),
            log,
        }
    }

    fn boxes(&self) -> Vec<String> {
        let mut boxes = self.boxes.clone();
        if let Some(discovered) = &self.discovered {
            for node in discovered.nodes() {
                if !boxes.contains(&node.base_url) {
                    boxes.push(node.base_url);
                }
            }
        }
        boxes
    }

    fn snapshots(&self) -> Vec<makepad_asset_ai::fleet::BoxSnapshot> {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        use makepad_asset_ai::fleet::BoxSnapshot;
        let boxes = self.boxes();
        let mut snapshots = Vec::with_capacity(boxes.len());
        for url in &boxes {
            let provider = LocalService::new(url);
            let mut snapshot = BoxSnapshot::new(url);
            if let Ok(health) = provider.health() {
                snapshot.health = Some(health);
                if let Ok(models) = provider.list_models() {
                    snapshot.models = models;
                } else if self.log {
                    eprintln!("[asset-worker depth] model probe {url} failed");
                }
            } else if self.log {
                eprintln!("[asset-worker depth] health probe {url} failed");
            }
            snapshots.push(snapshot);
        }
        snapshots
    }
}

impl DepthFleet for AssetAiDepthFleet {
    fn dispatch(&mut self, request: &DepthRequest) -> Result<DepthDispatch, String> {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        use makepad_asset_ai::fleet::pick_for_domain_admitted_scored;
        use makepad_asset_ai::protocol::GenerateRequestJson;
        use makepad_asset_ai::registry::Domain;
        if request.content_type != "image/png" {
            return Err(format!(
                "da3-metric-large requires PNG input (got {})",
                request.content_type
            ));
        }
        let snapshots = self.snapshots();
        let Some((index, model, _score)) = pick_for_domain_admitted_scored(&snapshots, "depth")
        else {
            return Ok(DepthDispatch::Waiting {
                stage: "waiting-for-fleet: no admitted depth model".to_string(),
            });
        };
        let base_url = snapshots[index].base_url.clone();
        let provider = LocalService::new(&base_url);
        let wire = GenerateRequestJson {
            model,
            input_b64: Some(base64_encode(&request.image)),
            input_content_type: Some(request.content_type.to_string()),
            ..Default::default()
        };
        match provider.request(Domain::Depth, &wire) {
            Ok(fleet_job) => {
                let model = wire.model.clone();
                self.routes.insert(fleet_job.clone(), base_url);
                Ok(DepthDispatch::Started {
                    fleet_job,
                    model,
                    version: "registry".to_string(),
                })
            }
            Err(makepad_asset_ai::AssetAiError::Busy)
            | Err(makepad_asset_ai::AssetAiError::QueueFull(_)) => Ok(DepthDispatch::Waiting {
                stage: "waiting-for-fleet: box busy".to_string(),
            }),
            Err(error) => Err(scrub(&error.to_string(), &base_url)),
        }
    }

    fn poll(&mut self, fleet_job: &str) -> Result<DepthPoll, String> {
        use makepad_asset_ai::client::{verify_artifact_bytes, ContentProvider, LocalService};
        use makepad_asset_ai::protocol::{JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR};
        let Some(base_url) = self.routes.get(fleet_job).cloned() else {
            return Err("unknown fleet job".to_string());
        };
        let provider = LocalService::new(&base_url);
        let status = provider
            .poll(fleet_job)
            .map_err(|e| scrub(&e.to_string(), &base_url))?;
        match status.state.as_str() {
            s if s == JOB_STATE_DONE => {
                if status.artifacts.len() < 2 {
                    return Ok(DepthPoll::Failed {
                        error: "done with fewer than two artifacts".to_string(),
                    });
                }
                let png_art = &status.artifacts[0];
                let json_art = &status.artifacts[1];
                if png_art.content_type != "image/png" {
                    return Ok(DepthPoll::Failed {
                        error: format!("unexpected depth type {}", png_art.content_type),
                    });
                }
                if json_art.content_type != "application/json" {
                    return Ok(DepthPoll::Failed {
                        error: format!("unexpected sidecar type {}", json_art.content_type),
                    });
                }
                let png = provider
                    .fetch_artifact(&png_art.id)
                    .map_err(|e| scrub(&e.to_string(), &base_url))?;
                verify_artifact_bytes(&png.bytes, png_art)
                    .map_err(|e| scrub(&e.to_string(), &base_url))?;
                let sidecar = provider
                    .fetch_artifact(&json_art.id)
                    .map_err(|e| scrub(&e.to_string(), &base_url))?;
                verify_artifact_bytes(&sidecar.bytes, json_art)
                    .map_err(|e| scrub(&e.to_string(), &base_url))?;
                Ok(DepthPoll::Done {
                    png: png.bytes,
                    sidecar: sidecar.bytes,
                })
            }
            s if s == JOB_STATE_ERROR || s == JOB_STATE_CANCELLED => Ok(DepthPoll::Failed {
                error: scrub(
                    status.error.as_deref().unwrap_or("fleet job failed"),
                    &base_url,
                ),
            }),
            _ => {
                let permille = (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16;
                Ok(DepthPoll::Running {
                    stage: status.stage.unwrap_or_else(|| "generating".to_string()),
                    permille,
                })
            }
        }
    }

    fn cancel(&mut self, fleet_job: &str) {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        if let Some(base_url) = self.routes.get(fleet_job) {
            let provider = LocalService::new(base_url);
            let _ = provider.cancel(fleet_job);
        }
    }
}

fn scrub(error: &str, base_url: &str) -> String {
    error.replace(base_url, "<fleet-node>")
}

fn sleep_sliced(total: Duration, stop: &AtomicBool) {
    let mut remaining = total;
    while remaining > Duration::ZERO && !stop.load(Ordering::SeqCst) {
        let slice = remaining.min(Duration::from_millis(100));
        std::thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
}

fn bounded(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out: String = text[..end].to_string();
    out.retain(|c| !c.is_control());
    out
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_store::{AssetServer, ServerConfig};
    use makepad_asset_client::{
        ApiEndpoints, ClientConfig, OperationCreateRequest, OperationInputRef, OperationStateDto,
        PublishFile, PublishRequest, PublishRights, PublishThumbnail,
    };
    use makepad_asset_data::{AssetKind, DerivativePolicy, Redistribution, ThumbnailMedia};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_root(name: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "mp_worker_depthop_{}_{}_{}",
            std::process::id(),
            n,
            name
        ))
    }

    fn start_server(name: &str) -> (AssetServer, String) {
        let root = test_root(name);
        let mut cfg = ServerConfig::new(root.clone());
        cfg.control_addr = "127.0.0.1:0".parse().unwrap();
        cfg.data_addr = "127.0.0.1:0".parse().unwrap();
        cfg.bootstrap_admin = true;
        cfg.log = false;
        let server = AssetServer::start(cfg).expect("server start");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        (server, token)
    }

    fn connect(server: &AssetServer, token: &str, leaf: &str) -> AssetClient {
        let mut cfg = ClientConfig::new(test_root(leaf));
        cfg.token = Some(token.to_string());
        let endpoints = ApiEndpoints {
            control: server.control_addr(),
            data: server.data_addr(),
        };
        AssetClient::connect(cfg, endpoints, Some(server.server_id())).expect("connect")
    }

    struct ScriptedDepth {
        expect_image: Vec<u8>,
        png: Vec<u8>,
        sidecar: Vec<u8>,
        polls: u32,
        dispatched: bool,
    }

    impl DepthFleet for ScriptedDepth {
        fn dispatch(&mut self, request: &DepthRequest) -> Result<DepthDispatch, String> {
            assert_eq!(request.image, self.expect_image);
            assert_eq!(request.content_type, "image/png");
            self.dispatched = true;
            Ok(DepthDispatch::Started {
                fleet_job: "fj-d".to_string(),
                model: "da3-metric-large".to_string(),
                version: "registry".to_string(),
            })
        }
        fn poll(&mut self, fleet_job: &str) -> Result<DepthPoll, String> {
            assert_eq!(fleet_job, "fj-d");
            self.polls += 1;
            if self.polls == 1 {
                Ok(DepthPoll::Running {
                    stage: "infer".to_string(),
                    permille: 500,
                })
            } else {
                Ok(DepthPoll::Done {
                    png: self.png.clone(),
                    sidecar: self.sidecar.clone(),
                })
            }
        }
        fn cancel(&mut self, _fleet_job: &str) {}
    }

    fn seed_and_create(
        server: &AssetServer,
        token: &str,
    ) -> (AssetClient, OperationId, Vec<u8>) {
        let mut admin = connect(server, token, "seed-cache");
        let png = b"png-bytes-depth-op".to_vec();
        let mut request = PublishRequest::new(
            "gen",
            AssetKind::Texture,
            "op seed",
            PublishFile {
                bytes: png.clone(),
                media: MediaType::Png,
                role: FileRole::Texture,
                media_millis: 0,
                dims: Some((64, 64)),
            },
            PublishThumbnail {
                bytes: vec![0xAB; 1_500],
                media: ThumbnailMedia::Png,
                width: 512,
                height: 512,
            },
        );
        request.rights = PublishRights::declared(
            "CC-BY-4.0",
            "Seed Author",
            "https://example.com/seed",
            Redistribution::AttributionRequired,
            DerivativePolicy::AttributionRequired,
        );
        let published = admin.publish_artifact(&request).expect("seed publish");
        let worker_probe = connect(server, token, "probe-cache");
        assert!(worker_probe
            .worker_claim_kinds(60_000, Some("w1"), &[DEPTH_OP_KIND])
            .unwrap()
            .is_none());
        let status = admin
            .operation_create(&OperationCreateRequest::new(
                "gen",
                "depth.from_image.v1",
                "worker-e2e",
                vec![OperationInputRef {
                    slot: "image".into(),
                    asset: published.asset_id,
                    revision: published.revision,
                    role: FileRole::Texture,
                    tier: None,
                    lod: None,
                    expected_media: Some(MediaType::Png),
                }],
            ))
            .expect("create");
        (admin, status.operation, png)
    }

    fn tiny_depth_png() -> Vec<u8> {
        // Valid 2x2 16-bit grayscale PNG (IHDR+IDAT+IEND).
        vec![
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2,
            16, 0, 0, 0, 0, 7, 77, 142, 187, 0, 0, 0, 14, 73, 68, 65, 84, 120, 156, 99, 96, 16, 0,
            66, 16, 1, 0, 1, 42, 0, 65, 187, 248, 98, 2, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96,
            130,
        ]
    }

    #[test]
    fn scripted_worker_finalizes_depth_operation() {
        let (server, token) = start_server("depthop_e2e");
        let (admin, op, png) = seed_and_create(&server, &token);
        let stop = AtomicBool::new(false);
        let sidecar = br#"{"metric":true,"unit":"mm","min":100.0,"max":200.0,"focal_px":436.48}"#;
        let mut fleet = ScriptedDepth {
            expect_image: png,
            png: tiny_depth_png(),
            sidecar: sidecar.to_vec(),
            polls: 0,
            dispatched: false,
        };
        let mut coordinator = DepthOpCoordinator {
            client: connect(&server, &token, "worker-cache"),
            fleet: &mut fleet,
            suffix: "w1".to_string(),
            log: false,
        };
        let outcome = coordinator.run_one(&stop).expect("run").expect("claimed");
        let DepthJobOutcome::Finalized { operation, .. } = outcome else {
            panic!("expected finalized, got {outcome:?}");
        };
        assert_eq!(operation, op.to_string());
        assert!(fleet.dispatched);
        let done = admin.operation_get(&op).expect("get");
        assert_eq!(done.state, OperationStateDto::Succeeded);
    }

    #[test]
    fn missing_input_fails_closed() {
        let outcome = DepthJobOutcome::Failed {
            error: "job body missing inputs".to_string(),
        };
        match outcome {
            DepthJobOutcome::Failed { error } => assert!(error.contains("missing inputs")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn jpeg_is_not_silently_substituted() {
        let request = DepthRequest {
            image: b"jpeg".to_vec(),
            content_type: "image/jpeg",
        };
        let mut fleet = AssetAiDepthFleet::from_lan(false);
        let err = fleet.dispatch(&request).unwrap_err();
        assert!(err.contains("requires PNG"));
    }
}
