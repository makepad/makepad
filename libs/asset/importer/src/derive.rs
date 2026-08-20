//! The derivation seam: claimed `mesh.derive` jobs turn one source GLB blob
//! into a published multi-file asset revision (render mesh + PBR texture
//! roles + mandatory thumbnail) through the bounded bundle publication API.
//!
//! Structure mirrors the video coordinator, with the compute behind a
//! [`Deriver`] trait instead of a fleet transport:
//!
//! - claim under a lease (kind-filtered, so this worker never touches
//!   foreign job kinds),
//! - fetch the exact source blob from the Asset Server (digest-verified by
//!   the client cache),
//! - run the deriver, which reports stages and polls cancellation,
//! - publish everything as ONE deterministic bundle revision,
//! - report the produced identities via `worker_succeed`.
//!
//! The claim lease heartbeat and upstream-cancellation checks stay active
//! through EVERY phase — source fetch, derivation, each blob upload,
//! staging, publication, and the alias commit — via a [`JobControl`] seam
//! that works while the client handle is mutably busy.
//!
//! Identity is deterministic: the derived asset id and alias derive from the
//! source blob digest plus the canonical parameter digest, so a retried or
//! re-enqueued derivation converges on the same asset and (for identical
//! outputs) the same immutable revision instead of duplicating catalog rows.
//!
//! There is deliberately NO production PBR generator here yet: the trait is
//! the seam, the scripted implementation in the tests is the contract
//! executor. Wiring a real generator is a later, separate step.

use makepad_asset_importer::coordinator::JobOutcome;
use makepad_asset_client::json::Value;
use makepad_asset_client::util::{from_hex_exact, sanitize_text, to_hex};
use makepad_asset_client::{
    AssetClient, ClaimedJobDto, ClientError, JobControl, JobId, JobStateDto, PublishBundle,
    PublishBundleFile, PublishProvenance, PublishRights, PublishStage, PublishStats,
    PublishThumbnail,
};
use makepad_asset_data::limits::MAX_FILE_BYTES;
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, AssetRevisionId, BlobId, DerivativePolicy, DeviceTier,
    FileRole, MediaType, Redistribution, Sha256,
};
use std::cell::{Cell, RefCell};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The job kind this coordinator claims. Kind filtering happens server-side
/// at claim time, so generation workers and derivation workers share one
/// queue without stealing each other's work.
pub const DERIVE_KIND: &str = "mesh.derive";

/// Lease + heartbeat cadence against the Asset Server.
const LEASE_MS: u64 = 60_000;
const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);
/// Upstream-cancellation poll cadence (cheap control-plane read).
const CANCEL_CHECK_EVERY: Duration = Duration::from_secs(1);
/// Idle sleep between claim attempts when the queue is empty (production
/// claim loop only; the scripted tests drive `run_one` directly).
#[allow(dead_code)]
const IDLE_SLEEP: Duration = Duration::from_secs(2);
/// Hard wall-clock ceiling for one derivation (fetch + derive + publish).
const DERIVE_DEADLINE: Duration = Duration::from_secs(45 * 60);

// ---------------------------------------------------------------------------
// job contract
// ---------------------------------------------------------------------------

/// The typed `mesh.derive` job body. Only the source blob is mandatory;
/// `params` is the deriver's bounded recipe document and participates in the
/// deterministic output identity.
///
/// Rights are NEVER invented. A derive job must either name the exact
/// published `source_revision` its blob comes from (the derived revision
/// INHERITS that manifest's complete rights record), or declare the full
/// terms explicitly (`license` + `redistribution` + `derivatives`, plus
/// optional pins). Declaring neither fails the job; declaring both that
/// disagree fails the job (a caller cannot downgrade inherited terms).
#[derive(Clone, Debug, PartialEq)]
pub struct DeriveRequest {
    pub source_blob: BlobId,
    /// Expected source size when the enqueuer knows it (enforced against
    /// the server's declared size before bytes stream).
    pub source_len: Option<u64>,
    pub title: String,
    pub prompt: String,
    /// The published revision the source blob belongs to; the derived
    /// output inherits its rights and records it as provenance parent.
    pub source_revision: Option<AssetRevisionId>,
    /// Explicitly declared complete terms (import-style declaration).
    pub rights: Option<PublishRights>,
    /// The raw bounded recipe parameters object (empty object when absent).
    pub params: Value,
}

impl DeriveRequest {
    pub fn from_body(body: &Value) -> Result<DeriveRequest, String> {
        let source = body
            .get("source_blob")
            .and_then(Value::as_str)
            .ok_or("job body has no source_blob")?;
        let source_blob =
            BlobId::from_str(source).map_err(|_| "malformed source_blob".to_string())?;
        let source_len = match body.get("source_len") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let len = v.as_u64().ok_or("malformed source_len")?;
                if len == 0 || len > MAX_FILE_BYTES {
                    return Err("source_len out of bounds".to_string());
                }
                Some(len)
            }
        };
        let title = body
            .get("title")
            .and_then(Value::as_str)
            .map(|t| sanitize_text(t.trim(), 120))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Derived asset".to_string());
        let prompt = body
            .get("prompt")
            .and_then(Value::as_str)
            .map(|p| sanitize_text(p, 4_000))
            .unwrap_or_default();
        let source_revision = match body.get("source_revision") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let text = v.as_str().ok_or("malformed source_revision")?;
                Some(
                    AssetRevisionId::from_str(text)
                        .map_err(|_| "malformed source_revision".to_string())?,
                )
            }
        };
        let rights = declared_rights(body)?;
        if source_revision.is_none() && rights.is_none() {
            return Err(
                "derive job declares no rights (source_revision or license + policies required)"
                    .to_string(),
            );
        }
        let params = match body.get("params") {
            None | Some(Value::Null) => Value::Obj(Vec::new()),
            Some(p @ Value::Obj(_)) => p.clone(),
            Some(_) => return Err("params must be an object".to_string()),
        };
        Ok(DeriveRequest {
            source_blob,
            source_len,
            title,
            prompt,
            source_revision,
            rights,
            params,
        })
    }

    /// A seed pinned by the enqueuer, when one was.
    pub fn seed(&self) -> Option<u64> {
        self.params.get("seed").and_then(Value::as_u64)
    }
}

/// Parse an explicit rights declaration out of a derive job body. Touching
/// ANY rights key commits the enqueuer to a complete declaration: license
/// plus BOTH policies are then mandatory, so a half-stated grant cannot
/// slip through as "whatever the worker assumes".
fn declared_rights(body: &Value) -> Result<Option<PublishRights>, String> {
    const KEYS: [&str; 9] = [
        "license",
        "license_revision",
        "terms_digest",
        "terms_url",
        "credits",
        "source",
        "source_archive",
        "redistribution",
        "derivatives",
    ];
    let declared = KEYS
        .iter()
        .any(|key| body.get(key).is_some_and(|v| !matches!(v, Value::Null)));
    if !declared {
        return Ok(None);
    }
    let text = |key: &'static str, max: usize| -> Result<String, String> {
        match body.get(key) {
            None | Some(Value::Null) => Ok(String::new()),
            Some(v) => {
                let raw = v.as_str().ok_or(format!("malformed {key}"))?;
                Ok(sanitize_text(raw.trim(), max))
            }
        }
    };
    let digest = |key: &'static str| -> Result<Option<[u8; 32]>, String> {
        match body.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => {
                let raw = v.as_str().ok_or(format!("malformed {key}"))?;
                Ok(Some(
                    from_hex_exact::<32>(raw).ok_or(format!("malformed {key}"))?,
                ))
            }
        }
    };
    let license = text("license", 120)?;
    if license.is_empty() {
        return Err("rights declaration needs a license".to_string());
    }
    let redistribution = policy_of(body, "redistribution")?
        .ok_or("rights declaration needs a redistribution policy")?;
    let derivatives = policy_of(body, "derivatives")?
        .ok_or("rights declaration needs a derivatives policy")?;
    Ok(Some(PublishRights {
        license,
        license_revision: text("license_revision", 64)?,
        terms_digest: digest("terms_digest")?,
        terms_url: text("terms_url", 500)?,
        credits: text("credits", 500)?,
        source: text("source", 500)?,
        source_archive: digest("source_archive")?,
        redistribution: match redistribution {
            PolicyWord::Allowed => Redistribution::Allowed,
            PolicyWord::AttributionRequired => Redistribution::AttributionRequired,
            PolicyWord::Forbidden => Redistribution::Forbidden,
            PolicyWord::LocalOnly => Redistribution::LanLocal,
        },
        derivatives: match derivatives {
            PolicyWord::Allowed => DerivativePolicy::Allowed,
            PolicyWord::AttributionRequired => DerivativePolicy::AttributionRequired,
            PolicyWord::Forbidden => DerivativePolicy::Forbidden,
            PolicyWord::LocalOnly => DerivativePolicy::LocalPreview,
        },
    }))
}

/// The closed policy vocabulary shared by both policy fields on the wire.
enum PolicyWord {
    Allowed,
    AttributionRequired,
    Forbidden,
    /// LAN-only (redistribution) / local-preview-only (derivatives).
    LocalOnly,
}

fn policy_of(body: &Value, key: &'static str) -> Result<Option<PolicyWord>, String> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let raw = v.as_str().ok_or(format!("malformed {key}"))?;
            Ok(Some(match raw {
                "allowed" => PolicyWord::Allowed,
                "attribution-required" => PolicyWord::AttributionRequired,
                "forbidden" => PolicyWord::Forbidden,
                "lan-local" | "user-owned-local" | "local-preview-only" | "local-preview" => {
                    PolicyWord::LocalOnly
                }
                _ => return Err(format!("unknown {key} policy")),
            }))
        }
    }
}

/// Digest of the canonical (sorted-key) JSON encoding of `params`, so two
/// enqueuers writing the same recipe in different field order share one
/// derived identity.
pub fn params_digest(params: &Value) -> [u8; 32] {
    let mut text = String::new();
    write_canonical_json(params, &mut text);
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher.finalize()
}

fn write_canonical_json(value: &Value, out: &mut String) {
    match value {
        Value::Obj(pairs) => {
            let mut sorted: Vec<&(String, Value)> = pairs.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            out.push('{');
            for (index, (key, v)) in sorted.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::Str((*key).clone()).to_json());
                out.push(':');
                write_canonical_json(v, out);
            }
            out.push('}');
        }
        Value::Arr(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical_json(item, out);
            }
            out.push(']');
        }
        leaf => out.push_str(&leaf.to_json()),
    }
}

/// The deterministic derived identity: asset id and stable two-segment alias
/// from the source digest plus the canonical recipe digest. A retry, a new
/// attempt, or an identical re-enqueue all converge here.
pub fn derived_identity(
    namespace: &str,
    source: &BlobId,
    params_digest: &[u8; 32],
) -> Result<(AssetId, AssetAlias), String> {
    let mut hasher = Sha256::new();
    hasher.update(b"mesh.derive/v1");
    hasher.update(source.as_bytes());
    hasher.update(params_digest);
    let digest = hasher.finalize();
    let asset = AssetId::from_bytes(digest[..16].try_into().expect("16 bytes"));
    let alias_text = format!("{namespace}/derived-{}", &to_hex(&digest)[..32]);
    let alias = AssetAlias::from_str(&alias_text)
        .map_err(|_| "namespace cannot form a catalog alias".to_string())?;
    Ok((asset, alias))
}

// ---------------------------------------------------------------------------
// the deriver seam
// ---------------------------------------------------------------------------

/// Progress + cancellation the coordinator hands into a deriver. `progress`
/// feeds the job heartbeat note (0..=1000 within the derive phase);
/// `cancelled` must be polled between expensive steps — when it reports
/// true, return promptly with any error, partial output is discarded.
pub struct DeriveCtl<'a> {
    pub progress: &'a mut dyn FnMut(&str, u16),
    pub cancelled: &'a dyn Fn() -> bool,
}

/// One derived output file (role/tier/LOD slot + bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    pub lod: u8,
    pub media: MediaType,
    pub bytes: Vec<u8>,
    /// Required for PNG/JPEG outputs, refused otherwise.
    pub dims: Option<(u32, u32)>,
}

/// Everything a deriver produced for one source: the typed files, the
/// mandatory thumbnail, MEASURED stats (zeros = unmeasured, never guessed),
/// and the deriver's own identity for provenance.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedBundle {
    pub kind: AssetKind,
    pub files: Vec<DerivedFile>,
    pub thumbnail: PublishThumbnail,
    pub stats: PublishStats,
    pub model: String,
    pub version: String,
}

/// The compute seam. Implementations run CPU/GPU work; they never talk to
/// the Asset Server, never see credentials, and never choose identities —
/// the coordinator owns fetch, identity, publication, and job reporting.
pub trait Deriver {
    fn derive(
        &mut self,
        request: &DeriveRequest,
        source_glb: &[u8],
        ctl: &mut DeriveCtl<'_>,
    ) -> Result<DerivedBundle, String>;
}

// ---------------------------------------------------------------------------
// lease pulse
// ---------------------------------------------------------------------------

/// Keeps one claimed job alive and honest while the coordinator is deep
/// inside a fetch, a derivation, or a publication: renews the lease with the
/// current stage note on a fixed cadence, polls for upstream cancellation,
/// and classifies a refused heartbeat (cancelled vs lease lost). Interior
/// mutability so `Fn` closures can drive it from within blocking client
/// calls.
struct Pulse<'a> {
    control: &'a JobControl,
    job: &'a JobId,
    suffix: &'a str,
    started: Instant,
    last_heartbeat: Cell<Instant>,
    last_cancel_check: Cell<Instant>,
    note: RefCell<String>,
    permille: Cell<u16>,
    cancelled: Cell<bool>,
    lease_lost: Cell<bool>,
}

impl<'a> Pulse<'a> {
    /// Starts with one forced beat, so every claimed job records progress
    /// ("derive:starting") even when the work finishes inside the first
    /// heartbeat window.
    fn start(control: &'a JobControl, job: &'a JobId, suffix: &'a str) -> Pulse<'a> {
        let now = Instant::now();
        let pulse = Pulse {
            control,
            job,
            suffix,
            started: now,
            last_heartbeat: Cell::new(now),
            last_cancel_check: Cell::new(now),
            note: RefCell::new(String::new()),
            permille: Cell::new(0),
            cancelled: Cell::new(false),
            lease_lost: Cell::new(false),
        };
        pulse.report("derive:starting", 0, true);
        pulse
    }

    fn report(&self, note: &str, permille: u16, force: bool) {
        *self.note.borrow_mut() = sanitize_text(note, 180);
        self.permille.set(permille.min(1000));
        self.tick(force);
    }

    fn tick(&self, force: bool) {
        if self.cancelled.get() || self.lease_lost.get() {
            return;
        }
        // Cancellation first: a cancelled job also refuses heartbeats, and
        // "cancelled" must never be misreported as "lease lost".
        if force || self.last_cancel_check.get().elapsed() >= CANCEL_CHECK_EVERY {
            self.last_cancel_check.set(Instant::now());
            if let Ok(state) = self.control.job_state(self.job) {
                if state == JobStateDto::Cancelled {
                    self.cancelled.set(true);
                    return;
                }
            }
        }
        if force || self.last_heartbeat.get().elapsed() >= HEARTBEAT_EVERY {
            self.last_heartbeat.set(Instant::now());
            let note = self.note.borrow().clone();
            let beat = self.control.heartbeat(
                self.job,
                LEASE_MS,
                Some(self.suffix),
                Some((self.permille.get(), &note)),
            );
            if beat.is_err() {
                match self.control.job_state(self.job) {
                    Ok(JobStateDto::Cancelled) => self.cancelled.set(true),
                    _ => self.lease_lost.set(true),
                }
            }
        }
    }

    /// The abort probe wired into every blocking phase: keeps the pulse
    /// alive as a side effect and reports whether work must stop.
    fn aborted(&self) -> bool {
        self.tick(false);
        self.cancelled.get()
            || self.lease_lost.get()
            || self.started.elapsed() > DERIVE_DEADLINE
    }
}

// ---------------------------------------------------------------------------
// the coordinator
// ---------------------------------------------------------------------------

pub struct DeriveCoordinator<'d> {
    pub client: AssetClient,
    pub deriver: &'d mut dyn Deriver,
    pub suffix: String,
    pub log: bool,
}

impl<'d> DeriveCoordinator<'d> {
    /// Claim and fully process at most one derive job. `Ok(None)` = queue
    /// empty.
    pub fn run_one(&mut self, stop: &AtomicBool) -> Result<Option<JobOutcome>, ClientError> {
        let Some(claimed) =
            self.client
                .worker_claim_kinds(LEASE_MS, Some(&self.suffix), &[DERIVE_KIND])?
        else {
            return Ok(None);
        };
        if claimed.kind != DERIVE_KIND {
            // The server-side kind filter makes this unreachable; if it ever
            // fires the job fails honestly instead of running foreign work.
            let error = Value::Obj(vec![(
                "error".to_string(),
                Value::Str(format!("derive worker cannot run kind {}", claimed.kind)),
            )]);
            self.client
                .worker_fail(&claimed.job, Some(&self.suffix), 0, Some(&error))?;
            return Ok(Some(JobOutcome::Failed {
                error: format!("unsupported kind {}", claimed.kind),
            }));
        }
        let outcome = self.process(&claimed, stop);
        match &outcome {
            Ok(JobOutcome::Published { asset, revision }) => {
                let result = Value::Obj(vec![
                    ("asset_id".to_string(), Value::Str(asset.clone())),
                    ("revision".to_string(), Value::Str(revision.clone())),
                ]);
                self.client
                    .worker_succeed(&claimed.job, Some(&self.suffix), Some(&result))?;
            }
            Ok(JobOutcome::Failed { error }) => {
                let doc = Value::Obj(vec![(
                    "error".to_string(),
                    Value::Str(sanitize_text(error, 2_000)),
                )]);
                self.client
                    .worker_fail(&claimed.job, Some(&self.suffix), 0, Some(&doc))?;
            }
            // Cancelled upstream: the server already terminated the job.
            Ok(JobOutcome::CancelledUpstream) => {}
            Err(_) => {}
        }
        outcome.map(Some)
    }

    fn process(
        &mut self,
        claimed: &ClaimedJobDto,
        stop: &AtomicBool,
    ) -> Result<JobOutcome, ClientError> {
        let request = match DeriveRequest::from_body(&claimed.body) {
            Ok(request) => request,
            Err(error) => return Ok(JobOutcome::Failed { error }),
        };
        self.log(&format!(
            "job {}: deriving from {} (\"{}\")",
            claimed.job, request.source_blob, request.title
        ));
        let control = self.client.job_control();
        let pulse = Pulse::start(&control, &claimed.job, &self.suffix);
        let abort = || stop.load(Ordering::SeqCst) || pulse.aborted();

        // Classify an aborted phase into the honest job outcome.
        let aborted_outcome = |pulse: &Pulse| {
            if pulse.cancelled.get() {
                JobOutcome::CancelledUpstream
            } else if pulse.lease_lost.get() {
                JobOutcome::Failed { error: "lease lost".to_string() }
            } else if stop.load(Ordering::SeqCst) {
                JobOutcome::Failed { error: "worker shutdown".to_string() }
            } else {
                JobOutcome::Failed { error: "derive deadline".to_string() }
            }
        };

        // ---- rights resolution: inherit, declare, or refuse ----
        // Runs BEFORE any compute is spent: a source whose terms forbid
        // derivatives never reaches the deriver.
        let inherited = match &request.source_revision {
            None => None,
            Some(rev) => match self.client.fetch_asset_manifest(rev) {
                Ok(manifest) => {
                    // Inheritance must come from the blob's OWN revision:
                    // naming an unrelated permissive manifest cannot launder
                    // a stricter source's terms.
                    if !manifest.files.iter().any(|f| f.blob == request.source_blob) {
                        return Ok(JobOutcome::Failed {
                            error: "source_revision does not reference the source blob"
                                .to_string(),
                        });
                    }
                    Some(PublishRights::from_manifest(&manifest.rights))
                }
                Err(ClientError::NotFound { .. }) => {
                    return Ok(JobOutcome::Failed {
                        error: "source_revision not on server".to_string(),
                    })
                }
                Err(error) => {
                    return Ok(JobOutcome::Failed {
                        error: format!("source manifest: {error}"),
                    })
                }
            },
        };
        let rights = match (&inherited, &request.rights) {
            (Some(inherited_rights), Some(declared)) => {
                // Restating inherited terms is fine; changing them is not —
                // a caller cannot downgrade the source's rights.
                if declared != inherited_rights {
                    return Ok(JobOutcome::Failed {
                        error: "declared rights conflict with source revision terms".to_string(),
                    });
                }
                inherited_rights.clone()
            }
            (Some(inherited_rights), None) => inherited_rights.clone(),
            (None, Some(declared)) => declared.clone(),
            // from_body refuses this shape; keep the honest refusal anyway.
            (None, None) => {
                return Ok(JobOutcome::Failed {
                    error: "derive job declares no rights".to_string(),
                })
            }
        };
        if rights.derivatives == DerivativePolicy::Forbidden {
            return Ok(JobOutcome::Failed {
                error: "source rights forbid derivatives".to_string(),
            });
        }

        // ---- fetch the exact source (digest-verified by the cache) ----
        let mut fetch_note = |done: u64, total: u64| {
            let permille = if total > 0 { ((done * 100) / total) as u16 } else { 0 };
            pulse.report("derive:fetch-source", permille, false);
        };
        let source_path = match self.client.fetch_blob_with_abort(
            &request.source_blob,
            request.source_len,
            Some(&mut fetch_note),
            &abort,
        ) {
            Ok(path) => path,
            Err(ClientError::Cancelled) => return Ok(aborted_outcome(&pulse)),
            Err(ClientError::NotFound { .. }) => {
                return Ok(JobOutcome::Failed { error: "source blob not on server".to_string() })
            }
            Err(error) => {
                return Ok(JobOutcome::Failed { error: format!("source fetch: {error}") })
            }
        };
        let source = match std::fs::read(&source_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Ok(JobOutcome::Failed { error: format!("source read: {error}") })
            }
        };

        // ---- derive (compute stays behind the seam) ----
        let mut derive_note = |stage: &str, permille: u16| {
            // The derive phase owns 100..=800 of the job's progress range.
            let mapped = 100 + (permille.min(1000) as u32 * 700 / 1000) as u16;
            pulse.report(&format!("derive:{stage}"), mapped, false);
        };
        let derive_cancelled = || stop.load(Ordering::SeqCst) || pulse.aborted();
        let derived = {
            let mut ctl = DeriveCtl {
                progress: &mut derive_note,
                cancelled: &derive_cancelled,
            };
            self.deriver.derive(&request, &source, &mut ctl)
        };
        let derived = match derived {
            Ok(bundle) => bundle,
            Err(error) => {
                if pulse.cancelled.get() {
                    self.log(&format!("job {}: cancelled upstream mid-derive", claimed.job));
                    return Ok(JobOutcome::CancelledUpstream);
                }
                return Ok(if pulse.lease_lost.get() {
                    JobOutcome::Failed { error: "lease lost".to_string() }
                } else {
                    JobOutcome::Failed { error: format!("derive: {error}") }
                });
            }
        };
        if abort() {
            return Ok(aborted_outcome(&pulse));
        }

        // ---- deterministic identity + bundle publication ----
        let digest = params_digest(&request.params);
        let (asset_id, alias) =
            match derived_identity(&claimed.namespace, &request.source_blob, &digest) {
                Ok(identity) => identity,
                Err(error) => return Ok(JobOutcome::Failed { error }),
            };
        let mut publish = PublishBundle::new(
            &claimed.namespace,
            derived.kind,
            request.title.clone(),
            derived
                .files
                .into_iter()
                .map(|f| PublishBundleFile {
                    role: f.role,
                    tier: f.tier,
                    lod: f.lod,
                    media: f.media,
                    bytes: f.bytes,
                    dims: f.dims,
                })
                .collect(),
            derived.thumbnail,
            // The resolved (inherited or declared) source terms — never a
            // worker-invented default.
            rights.clone(),
        );
        publish.asset_id = Some(asset_id);
        publish.alias = Some(alias);
        publish.stats = derived.stats;
        publish.categories = vec!["derived".to_string()];
        publish.prompt = request.prompt.clone();
        publish.generator = "asset-worker".to_string();
        publish.backend = derived.model.clone();
        publish.model = derived.model.clone();
        // Attribution is also catalog-searchable via the annotation.
        publish.creator = rights.credits.clone();
        // Typed provenance only from REAL knowledge: the seed must have been
        // pinned by the enqueuer and the deriver must report its version.
        // The derivation lineage records the exact source revision when the
        // job named one.
        if let Some(seed) = request.seed().filter(|_| !derived.version.is_empty()) {
            publish.manifest_provenance = Some(PublishProvenance {
                generator: "makepad-asset-importer".to_string(),
                model: derived.model.clone(),
                version: derived.version.clone(),
                seed,
                parents: request.source_revision.iter().copied().collect(),
                params_digest: Some(digest),
            });
        }
        let mut stage_note = |stage: &PublishStage| {
            pulse.report(&format!("publish:{stage}"), publish_permille(stage), false);
        };
        match self
            .client
            .publish_bundle_with(&publish, Some(&mut stage_note), &abort)
        {
            Ok(published) => {
                self.log(&format!(
                    "job {}: published {} rev {}",
                    claimed.job, published.asset_id, published.revision
                ));
                Ok(JobOutcome::Published {
                    asset: published.asset_id.to_string(),
                    revision: published.revision.to_string(),
                })
            }
            Err(ClientError::Cancelled) => {
                self.log(&format!("job {}: cancelled mid-publication", claimed.job));
                Ok(aborted_outcome(&pulse))
            }
            Err(error) => Ok(JobOutcome::Failed { error: format!("publish: {error}") }),
        }
    }

    /// Claim/process until stopped. Transport errors back off and retry.
    /// (Production claim loop; the scripted tests drive `run_one`.)
    #[allow(dead_code)]
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

    fn log(&self, message: &str) {
        if self.log {
            eprintln!("[asset-worker] {message}");
        }
    }
}

/// The publish phase owns 800..=1000 of the job progress range; uploads
/// spread across their own window so a many-blob bundle visibly advances.
fn publish_permille(stage: &PublishStage) -> u16 {
    match stage {
        PublishStage::Validating => 805,
        PublishStage::RegisteringAsset => 815,
        PublishStage::UploadingBlob { index, of, .. } => {
            820 + ((*index).min(*of) as u32 * 120 / (*of).max(1) as u32) as u16
        }
        PublishStage::Annotating => 950,
        PublishStage::Staging => 965,
        PublishStage::Publishing => 980,
        PublishStage::SettingAlias => 990,
        PublishStage::Complete => 1000,
    }
}

#[allow(dead_code)]
fn sleep_sliced(total: Duration, stop: &AtomicBool) {
    let mut left = total;
    while !left.is_zero() && !stop.load(Ordering::SeqCst) {
        let slice = left.min(Duration::from_millis(100));
        std::thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
}

// ---------------------------------------------------------------------------
// real-server release tests: scripted deriver end to end
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_store::{AssetServer, ServerConfig};
    use makepad_asset_client::json::{obj, s};
    use makepad_asset_client::{
        ApiEndpoints, AssetClient, ClientConfig, PublishFile, PublishRequest, TierPreference,
    };
    use makepad_asset_data::{AssetRevisionId, ThumbnailMedia};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicU64;

    static TEST_DIR: AtomicU64 = AtomicU64::new(0);

    fn test_root(name: &str) -> PathBuf {
        let n = TEST_DIR.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "mp_asset_worker_derive_{}_{}_{}",
            std::process::id(),
            n,
            name
        ))
    }

    fn connect(server: &AssetServer, token: &str, cache: &Path) -> AssetClient {
        let mut config = ClientConfig::new(cache.to_path_buf());
        config.token = Some(token.to_string());
        AssetClient::connect(
            config,
            ApiEndpoints {
                control: server.control_addr(),
                data: server.data_addr(),
            },
            Some(server.server_id()),
        )
        .expect("connect to isolated test server")
    }

    fn start_server(root: &Path) -> (AssetServer, String) {
        let mut config = ServerConfig::new(root.to_path_buf());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        (server, token)
    }

    /// Deterministic scripted source bytes (stand-in GLB payload).
    fn scripted_source() -> Vec<u8> {
        (0..8_192u32).map(|i| (i * 31 % 251) as u8).collect()
    }

    /// The scripted deriver's exact outputs for a source — shared between
    /// the deriver and the assertions so every published blob is verified
    /// byte-for-byte.
    fn scripted_outputs(source: &[u8]) -> Vec<DerivedFile> {
        let digest = *BlobId::hash_of(source).as_bytes();
        let image = |role, seed: u8, len: usize| DerivedFile {
            role,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Png,
            bytes: vec![seed; len],
            dims: Some((512, 512)),
        };
        vec![
            DerivedFile {
                role: FileRole::RenderGlb,
                tier: DeviceTier::Any,
                lod: 0,
                media: MediaType::Glb,
                bytes: source.to_vec(),
                dims: None,
            },
            image(FileRole::Albedo, digest[0], 4_096),
            image(FileRole::Normal, digest[1], 4_096),
            image(FileRole::Orm, digest[2], 2_048),
        ]
    }

    fn scripted_thumbnail(source: &[u8]) -> PublishThumbnail {
        let digest = *BlobId::hash_of(source).as_bytes();
        PublishThumbnail {
            bytes: vec![digest[3]; 900],
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            views: Vec::new(),
        }
    }

    fn scripted_stats() -> PublishStats {
        PublishStats { triangles: 12, vertices: 8, joints: 0, clips: 0 }
    }

    /// The registered source pack's complete terms — the record that must
    /// survive import, derivation, publication, and cache materialization
    /// byte-for-byte.
    fn licensed_rights() -> PublishRights {
        PublishRights {
            license: "CC-BY-4.0".to_string(),
            license_revision: "2013-11-25".to_string(),
            terms_digest: Some([0xAD; 32]),
            terms_url: "https://creativecommons.org/licenses/by/4.0/legalcode".to_string(),
            credits: "Kenney (kenney.nl)".to_string(),
            source: "https://kenney.nl/assets/space-kit".to_string(),
            source_archive: Some([0xCE; 32]),
            redistribution: Redistribution::AttributionRequired,
            derivatives: DerivativePolicy::AttributionRequired,
        }
    }

    /// The scripted contract executor: deterministic multi-role PBR set with
    /// staged progress and cancellation polls between "bakes".
    struct ScriptedDeriver;

    impl Deriver for ScriptedDeriver {
        fn derive(
            &mut self,
            _request: &DeriveRequest,
            source_glb: &[u8],
            ctl: &mut DeriveCtl<'_>,
        ) -> Result<DerivedBundle, String> {
            for (stage, permille) in [
                ("probing", 100u16),
                ("bake-albedo", 400),
                ("bake-normal", 600),
                ("bake-orm", 800),
            ] {
                if (ctl.cancelled)() {
                    return Err("cancelled".to_string());
                }
                (ctl.progress)(stage, permille);
            }
            Ok(DerivedBundle {
                kind: AssetKind::Prop,
                files: scripted_outputs(source_glb),
                thumbnail: scripted_thumbnail(source_glb),
                stats: scripted_stats(),
                model: "scripted-pbr".to_string(),
                version: "0.1-test".to_string(),
            })
        }
    }

    /// Publishes the source bytes as a real catalog asset carrying `rights`
    /// and returns the blob id + revision — the realistic "derive from an
    /// already-imported licensed mesh" seed.
    fn seed_source(
        client: &mut AssetClient,
        source: &[u8],
        title: &str,
        rights: PublishRights,
    ) -> (BlobId, AssetRevisionId) {
        let mut request = PublishRequest::new(
            "gen",
            AssetKind::Mesh,
            title,
            PublishFile {
                bytes: source.to_vec(),
                media: MediaType::Glb,
                role: FileRole::RenderGlb,
                media_millis: 0,
                dims: None,
            },
            scripted_thumbnail(source),
        );
        request.stats = PublishStats { triangles: 6, vertices: 4, joints: 0, clips: 0 };
        request.rights = rights;
        let published = client.publish_artifact(&request).expect("seed source asset");
        assert_eq!(published.artifact_blob, BlobId::hash_of(source));
        (published.artifact_blob, published.revision)
    }

    /// Enqueue a derive job whose rights are dishonest/incomplete and prove
    /// it fails with the expected reason, publishing nothing.
    fn expect_derive_failure(
        submitter: &mut AssetClient,
        coordinator: &mut DeriveCoordinator<'_>,
        body: &Value,
        needle: &str,
    ) {
        let job = submitter.enqueue_job("gen", DERIVE_KIND, body).expect("enqueue");
        let outcome = coordinator
            .run_one(&AtomicBool::new(false))
            .expect("coordinator call")
            .expect("claimed refusal-path job");
        let JobOutcome::Failed { error } = outcome else {
            panic!("expected failure containing {needle:?}, got {outcome:?}")
        };
        assert!(error.contains(needle), "error {error:?} lacks {needle:?}");
        assert_eq!(
            submitter.job_status(&job).expect("failed job status").state,
            JobStateDto::Failed
        );
    }

    /// The canonical derive body: rights INHERITED from the exact published
    /// source revision, never restated by hand.
    fn derive_body(source_blob: &BlobId, source: &[u8], source_revision: &AssetRevisionId) -> Value {
        obj(vec![
            ("source_blob", s(source_blob.to_string())),
            ("source_len", Value::Int(source.len() as i64)),
            ("title", s("Derived crate")),
            ("source_revision", s(source_revision.to_string())),
            (
                "params",
                obj(vec![("profile", s("pbr-basic")), ("seed", Value::Int(42))]),
            ),
        ])
    }

    #[test]
    fn derive_job_end_to_end_multifile_publish_and_job_views() {
        let root = test_root("e2e");
        let (server, token) = start_server(&root);
        let mut submitter = connect(&server, &token, &root.join("submit-cache"));
        let source = scripted_source();
        let (source_blob, source_revision) =
            seed_source(&mut submitter, &source, "Source mesh", licensed_rights());

        let body = derive_body(&source_blob, &source, &source_revision);
        let job = submitter.enqueue_job("gen", DERIVE_KIND, &body).expect("enqueue");
        // A queued job the derive worker must NOT claim.
        let foreign = submitter
            .enqueue_job("gen", "video.generate", &obj(vec![("prompt", s("x"))]))
            .expect("enqueue foreign");

        let worker = connect(&server, &token, &root.join("worker-cache"));
        let mut deriver = ScriptedDeriver;
        let mut coordinator = DeriveCoordinator {
            client: worker,
            deriver: &mut deriver,
            suffix: "derive-w".to_string(),
            log: false,
        };
        let outcome = coordinator
            .run_one(&AtomicBool::new(false))
            .expect("coordinator call")
            .expect("claimed the derive job");
        let JobOutcome::Published { asset, revision } = outcome else {
            panic!("expected publication, got {outcome:?}")
        };

        // Deterministic identity: recomputable from source + params alone.
        let digest = params_digest(&body.get("params").unwrap().clone());
        let (want_asset, want_alias) =
            derived_identity("gen", &source_blob, &digest).expect("identity");
        assert_eq!(asset, want_asset.to_string());
        let revision = AssetRevisionId::from_str(&revision).expect("revision id");
        let alias = submitter.resolve_alias(&want_alias).expect("derived alias");
        assert_eq!(alias.head_revision, revision);
        assert_eq!(alias.asset_id, want_asset);

        // The complete job detail: enqueuer, attempts, progress freshness,
        // typed + raw result.
        let detail = submitter.job_detail(&job).expect("job detail");
        assert_eq!(detail.status.state, JobStateDto::Succeeded);
        assert_eq!(detail.job(), job);
        let enqueuer = detail.enqueued_by.expect("enqueuer principal");
        assert!(enqueuer.to_string().starts_with("prin_"));
        assert_eq!(detail.attempts.len(), 1);
        assert_eq!(detail.attempts[0].attempt, 1);
        assert!(detail.attempts[0].ended_ms.is_some(), "attempt closed");
        let progress = detail.progress.as_ref().expect("heartbeat progress recorded");
        assert!(progress.updated_ms.is_some(), "progress freshness preserved");
        assert!(!progress.note.is_empty());
        let result = detail.result.as_ref().expect("typed result");
        assert_eq!(result.outcome, "succeeded");
        assert_eq!(result.attempt, 1);
        assert!(result.recorded_ms > 0);
        assert_eq!(
            result.body.get("asset_id").and_then(Value::as_str),
            Some(asset.as_str()),
            "raw result body preserved"
        );
        assert_eq!(detail.status.result_asset, Some(want_asset));
        assert_eq!(detail.status.result_revision, Some(revision));

        // Scoped job listing: own jobs and namespace-scoped both see it.
        let own = submitter.list_jobs(None, 50).expect("own jobs");
        let row = own.iter().find(|r| r.job == job).expect("own listing has the job");
        assert_eq!(row.kind, DERIVE_KIND);
        assert_eq!(row.state, JobStateDto::Succeeded);
        assert_eq!(row.enqueued_by, Some(enqueuer));
        assert!(own.iter().any(|r| r.job == foreign), "foreign job listed too");
        let scoped = submitter.list_jobs(Some("gen"), 50).expect("namespace jobs");
        assert!(scoped.iter().any(|r| r.job == job));
        assert!(matches!(
            submitter.list_jobs(Some("gen"), 0),
            Err(ClientError::InvalidInput { .. })
        ));
        assert!(matches!(
            submitter.list_jobs(None, 501),
            Err(ClientError::InvalidInput { .. })
        ));

        // The manifest carries every role/tier/LOD exactly; every blob
        // round-trips byte-for-byte.
        let manifest = submitter.fetch_asset_manifest(&revision).expect("manifest");
        assert_eq!(manifest.asset_id, want_asset);
        assert_eq!(manifest.kind, AssetKind::Prop);
        let expected = scripted_outputs(&source);
        assert_eq!(manifest.files.len(), expected.len());
        for want in &expected {
            let file = manifest
                .files
                .iter()
                .find(|f| (f.role, f.tier, f.lod) == (want.role, want.tier, want.lod))
                .unwrap_or_else(|| panic!("manifest misses slot {:?}", want.role));
            assert_eq!(file.media, want.media);
            assert_eq!(file.byte_len, want.bytes.len() as u64);
            assert_eq!(file.blob, BlobId::hash_of(&want.bytes));
            assert_eq!(
                file.dims.as_ref().map(|d| (d.width, d.height)),
                want.dims,
                "dims for {:?}",
                want.role
            );
            let bytes = submitter
                .fetch_blob_bytes(&file.blob, Some(file.byte_len))
                .expect("blob bytes");
            assert_eq!(bytes, want.bytes, "blob bytes for {:?}", want.role);
        }
        let thumb = manifest.thumbnail.clone().expect("mandatory thumbnail");
        let want_thumb = scripted_thumbnail(&source);
        assert_eq!(thumb.blob, BlobId::hash_of(&want_thumb.bytes));
        assert_eq!((thumb.width, thumb.height), (512, 512));
        let thumb_bytes = submitter
            .fetch_blob_bytes(&thumb.blob, Some(thumb.byte_len))
            .expect("thumbnail bytes");
        assert_eq!(thumb_bytes, want_thumb.bytes);
        let provenance = manifest.provenance.as_ref().expect("typed provenance from pinned seed");
        assert_eq!(provenance.model, "scripted-pbr");
        assert_eq!(provenance.seed, 42);
        assert_eq!(provenance.params_digest, Some(digest));
        // The derivation lineage names the exact source revision.
        assert_eq!(provenance.parents, vec![source_revision]);
        // The derived revision INHERITED the source's complete terms:
        // license id + revision qualifier, pinned digests/URL, attribution,
        // upstream identity, and both policies — byte-for-byte.
        assert_eq!(PublishRights::from_manifest(&manifest.rights), licensed_rights());

        // Verified cache materialization: the resolver hands back local
        // paths whose bytes re-hash to the manifest identities.
        let resolved = submitter
            .resolve_file(
                &manifest,
                FileRole::RenderGlb,
                TierPreference::PreferWithAnyFallback(DeviceTier::High),
                0,
                None,
            )
            .expect("materialize render glb");
        assert_eq!(std::fs::read(&resolved.path).expect("resolved bytes"), source);
        assert_eq!(resolved.blob, source_blob);
        let orm = submitter
            .resolve_file(
                &manifest,
                FileRole::Orm,
                TierPreference::PreferWithAnyFallback(DeviceTier::Low),
                0,
                None,
            )
            .expect("materialize orm texture");
        let want_orm = expected.iter().find(|f| f.role == FileRole::Orm).unwrap();
        assert_eq!(std::fs::read(&orm.path).expect("orm bytes"), want_orm.bytes);
        let thumb_resolved = submitter
            .resolve_thumbnail(&manifest)
            .expect("thumbnail resolve")
            .expect("manifest has a thumbnail");
        assert_eq!(
            std::fs::read(&thumb_resolved.path).expect("thumb bytes"),
            want_thumb.bytes
        );

        // An identical re-enqueue converges on the same immutable revision
        // and never duplicates a committed candidate.
        let job2 = submitter.enqueue_job("gen", DERIVE_KIND, &body).expect("re-enqueue");
        let outcome2 = coordinator
            .run_one(&AtomicBool::new(false))
            .expect("second run")
            .expect("claimed the second derive job");
        let JobOutcome::Published { asset: asset2, revision: revision2 } = outcome2 else {
            panic!("expected idempotent publication, got {outcome2:?}")
        };
        assert_eq!(asset2, asset);
        assert_eq!(AssetRevisionId::from_str(&revision2).unwrap(), revision);
        assert_eq!(
            submitter.job_detail(&job2).expect("second detail").status.state,
            JobStateDto::Succeeded
        );
        let derived_detail = submitter.asset_detail(&want_asset).expect("derived asset");
        assert_eq!(derived_detail.candidates.len(), 1, "no duplicate committed revision");
        assert_eq!(derived_detail.candidates[0].revision, revision);
        assert!(derived_detail.latest_published().is_some());

        // The foreign-kind job was never claimed by this worker.
        assert_eq!(
            submitter.job_status(&foreign).expect("foreign status").state,
            JobStateDto::Pending
        );
        submitter.cancel_job(&foreign).expect("drain fixture job");

        // ---- rights refusal paths (all fail the job, publish nothing) ----
        // A caller cannot downgrade inherited terms.
        let mut downgrade = derive_body(&source_blob, &source, &source_revision);
        let Value::Obj(pairs) = &mut downgrade else { unreachable!() };
        pairs.push(("license".to_string(), s("CC0-1.0")));
        pairs.push(("redistribution".to_string(), s("allowed")));
        pairs.push(("derivatives".to_string(), s("allowed")));
        expect_derive_failure(
            &mut submitter,
            &mut coordinator,
            &downgrade,
            "conflict with source revision terms",
        );
        // Rights cannot be laundered from an unrelated permissive revision.
        let other_bytes: Vec<u8> = (0..4_096u32).map(|i| (i * 7 % 249) as u8).collect();
        let (_, permissive_revision) = seed_source(
            &mut submitter,
            &other_bytes,
            "Unrelated permissive mesh",
            PublishRights::generated_cc0(),
        );
        expect_derive_failure(
            &mut submitter,
            &mut coordinator,
            &derive_body(&source_blob, &source, &permissive_revision),
            "does not reference the source blob",
        );
        // Omitting rights entirely is an error, never CC0.
        expect_derive_failure(
            &mut submitter,
            &mut coordinator,
            &obj(vec![("source_blob", s(source_blob.to_string()))]),
            "declares no rights",
        );
        // A source whose terms forbid derivatives never reaches the deriver.
        let sealed_bytes: Vec<u8> = (0..4_096u32).map(|i| (i * 11 % 241) as u8).collect();
        let (sealed_blob, sealed_revision) = seed_source(
            &mut submitter,
            &sealed_bytes,
            "Sealed proprietary mesh",
            PublishRights::declared(
                "LicenseRef-Proprietary-EULA",
                "",
                "https://example.com/eula",
                Redistribution::Forbidden,
                DerivativePolicy::Forbidden,
            ),
        );
        expect_derive_failure(
            &mut submitter,
            &mut coordinator,
            &derive_body(&sealed_blob, &sealed_bytes, &sealed_revision),
            "forbid derivatives",
        );

        drop(submitter);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }

    /// A deriver that triggers an upstream cancel at derive start, then
    /// waits for the coordinator's cancellation signal to reach it.
    struct CancellingDeriver<F: FnMut()> {
        cancel: F,
    }

    impl<F: FnMut()> Deriver for CancellingDeriver<F> {
        fn derive(
            &mut self,
            _request: &DeriveRequest,
            _source_glb: &[u8],
            ctl: &mut DeriveCtl<'_>,
        ) -> Result<DerivedBundle, String> {
            (self.cancel)();
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(20) {
                if (ctl.cancelled)() {
                    return Err("cancelled".to_string());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err("cancellation was never observed".to_string())
        }
    }

    #[test]
    fn cancellation_mid_derive_publishes_nothing() {
        let root = test_root("cancel_derive");
        let (server, token) = start_server(&root);
        let mut submitter = connect(&server, &token, &root.join("submit-cache"));
        let source = scripted_source();
        let (source_blob, source_revision) =
            seed_source(&mut submitter, &source, "Source mesh", licensed_rights());
        let body = derive_body(&source_blob, &source, &source_revision);
        let job = submitter.enqueue_job("gen", DERIVE_KIND, &body).expect("enqueue");

        let worker = connect(&server, &token, &root.join("worker-cache"));
        let mut deriver = CancellingDeriver {
            cancel: || {
                submitter.cancel_job(&job).expect("upstream cancel");
            },
        };
        let outcome = {
            let mut coordinator = DeriveCoordinator {
                client: worker,
                deriver: &mut deriver,
                suffix: "derive-w".to_string(),
                log: false,
            };
            coordinator
                .run_one(&AtomicBool::new(false))
                .expect("coordinator call")
                .expect("claimed the derive job")
        };
        assert_eq!(outcome, JobOutcome::CancelledUpstream);

        // The job ended cancelled (not failed by the worker), one attempt
        // was recorded, and NOTHING reached the catalog.
        let verifier = connect(&server, &token, &root.join("verify-cache"));
        let detail = verifier.job_detail(&job).expect("job detail");
        assert_eq!(detail.status.state, JobStateDto::Cancelled);
        assert_eq!(detail.attempts.len(), 1);
        assert!(detail.result.is_none(), "no terminal result document");
        let digest = params_digest(&body.get("params").unwrap().clone());
        let (derived_asset, derived_alias) =
            derived_identity("gen", &source_blob, &digest).expect("identity");
        assert!(matches!(
            verifier.asset_detail(&derived_asset),
            Err(ClientError::NotFound { .. })
        ));
        assert!(matches!(
            verifier.resolve_alias(&derived_alias),
            Err(ClientError::NotFound { .. })
        ));

        drop(verifier);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn publication_abort_is_all_or_nothing_and_retry_is_idempotent() {
        let root = test_root("abort_publish");
        let (server, token) = start_server(&root);
        let mut publisher = connect(&server, &token, &root.join("pub-cache"));
        let source = scripted_source();

        let mut bundle = PublishBundle::new(
            "gen",
            AssetKind::Prop,
            "Abort retry crate",
            scripted_outputs(&source)
                .into_iter()
                .map(|f| PublishBundleFile {
                    role: f.role,
                    tier: f.tier,
                    lod: f.lod,
                    media: f.media,
                    bytes: f.bytes,
                    dims: f.dims,
                })
                .collect(),
            scripted_thumbnail(&source),
            licensed_rights(),
        );
        bundle.stats = scripted_stats();
        bundle.asset_id = Some(AssetId::from_bytes([0x42; 16]));
        bundle.alias = Some(AssetAlias::from_str("gen/abort-retry").unwrap());
        let asset_id = bundle.asset_id.unwrap();

        // Abort before ANY network step: nothing exists, not even the id.
        assert!(matches!(
            publisher.publish_bundle_with(&bundle, None, &|| true),
            Err(ClientError::Cancelled)
        ));
        assert!(matches!(
            publisher.asset_detail(&asset_id),
            Err(ClientError::NotFound { .. })
        ));

        // Abort right after staging: the candidate exists but is NOT
        // published — nothing catalog-visible committed.
        let staged = Cell::new(false);
        let stages = RefCell::new(Vec::new());
        let mut observe = |stage: &PublishStage| {
            stages.borrow_mut().push(stage.clone());
            if matches!(stage, PublishStage::Staging) {
                staged.set(true);
            }
        };
        assert!(matches!(
            publisher.publish_bundle_with(&bundle, Some(&mut observe), &|| staged.get()),
            Err(ClientError::Cancelled)
        ));
        {
            let seen = stages.borrow();
            assert_eq!(seen[0], PublishStage::Validating);
            assert_eq!(seen[1], PublishStage::RegisteringAsset);
            assert!(seen.iter().any(|s| matches!(s, PublishStage::UploadingBlob { .. })));
            assert!(seen.contains(&PublishStage::Staging));
            assert!(!seen.contains(&PublishStage::Publishing), "abort landed before publish");
        }
        let detail = publisher.asset_detail(&asset_id).expect("registered asset");
        assert_eq!(detail.candidates.len(), 1);
        assert!(detail.latest_published().is_none(), "staged but never published");

        // The retry resumes from the typed candidate state: it skips the
        // stage step, publishes the SAME revision, and cannot duplicate it.
        let stages = RefCell::new(Vec::new());
        let mut observe = |stage: &PublishStage| stages.borrow_mut().push(stage.clone());
        let published = publisher
            .publish_bundle_with(&bundle, Some(&mut observe), &|| false)
            .expect("retry publishes");
        {
            let seen = stages.borrow();
            assert!(!seen.contains(&PublishStage::Staging), "resume skipped re-staging");
            assert!(seen.contains(&PublishStage::Publishing));
            assert!(seen.contains(&PublishStage::SettingAlias));
            assert_eq!(seen.last(), Some(&PublishStage::Complete));
        }
        assert_eq!(published.asset_id, asset_id);
        assert_eq!(published.files.len(), 4);
        let detail = publisher.asset_detail(&asset_id).expect("published asset");
        assert_eq!(detail.candidates.len(), 1, "no duplicate committed revision");
        assert_eq!(detail.candidates[0].revision, published.revision);
        assert!(detail.latest_published().is_some());

        // Replaying the whole publication after success is a no-op that
        // returns the same refs (idempotent all the way through).
        let replay = publisher.publish_bundle(&bundle).expect("replay");
        assert_eq!(replay, published);
        let detail = publisher.asset_detail(&asset_id).expect("still one candidate");
        assert_eq!(detail.candidates.len(), 1);

        // A later publication of the SAME asset with weakened terms refuses
        // — rights are immutable per asset, and the original record stays.
        let mut downgraded = bundle.clone();
        downgraded.rights = PublishRights::generated_cc0();
        assert!(matches!(
            publisher.publish_bundle(&downgraded),
            Err(ClientError::RightsConflict { .. })
        ));
        let detail = publisher.asset_detail(&asset_id).expect("asset survives");
        assert_eq!(detail.candidates.len(), 1, "downgrade committed nothing");
        let manifest = publisher
            .fetch_asset_manifest(&published.revision)
            .expect("published manifest");
        assert_eq!(
            PublishRights::from_manifest(&manifest.rights),
            licensed_rights(),
            "original terms intact after the refused downgrade"
        );

        drop(publisher);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn derive_request_parses_strictly_and_identity_is_canonical() {
        let blob = BlobId::hash_of(b"source");
        let rev = AssetRevisionId::from_bytes([9; 32]);
        let body = obj(vec![
            ("source_blob", s(blob.to_string())),
            ("source_revision", s(rev.to_string())),
            ("source_len", Value::Int(6)),
            ("title", s("  Fancy crate  ")),
            ("params", obj(vec![("b", Value::Int(2)), ("a", Value::Int(1))])),
        ]);
        let request = DeriveRequest::from_body(&body).expect("parses");
        assert_eq!(request.source_blob, blob);
        assert_eq!(request.source_len, Some(6));
        assert_eq!(request.title, "Fancy crate");
        assert_eq!(request.seed(), None);
        // The fixture body names a source_revision, so rights are inherited
        // (no explicit declaration present).
        assert!(request.source_revision.is_some());
        assert!(request.rights.is_none());

        // A COMPLETE explicit declaration parses through, trimmed and
        // control-stripped, with pins and policies typed.
        let licensed = obj(vec![
            ("source_blob", s(blob.to_string())),
            ("license", s(" CC-BY-4.0 ")),
            ("license_revision", s("2013-11-25")),
            ("terms_digest", s("ad".repeat(32))),
            ("terms_url", s("https://creativecommons.org/licenses/by/4.0/")),
            ("credits", s("Kenney (kenney.nl)")),
            ("source", s("https://kenney.nl/assets/space-kit")),
            ("source_archive", s("ce".repeat(32))),
            ("redistribution", s("attribution-required")),
            ("derivatives", s("attribution-required")),
        ]);
        let request = DeriveRequest::from_body(&licensed).expect("licensed parses");
        let rights = request.rights.expect("declared rights");
        assert_eq!(rights.license, "CC-BY-4.0");
        assert_eq!(rights.license_revision, "2013-11-25");
        assert_eq!(rights.terms_digest, Some([0xAD; 32]));
        assert_eq!(rights.credits, "Kenney (kenney.nl)");
        assert_eq!(rights.source_archive, Some([0xCE; 32]));
        assert_eq!(rights.redistribution, Redistribution::AttributionRequired);
        assert_eq!(rights.derivatives, DerivativePolicy::AttributionRequired);

        for (mutation, why) in [
            (obj(vec![("source_len", Value::Int(6))]), "missing source_blob"),
            (obj(vec![("source_blob", s("sha256:short"))]), "malformed blob"),
            (
                obj(vec![
                    ("source_blob", s(blob.to_string())),
                    ("source_revision", s(rev.to_string())),
                    ("source_len", Value::Int(0)),
                ]),
                "zero source_len",
            ),
            (
                obj(vec![
                    ("source_blob", s(blob.to_string())),
                    ("source_revision", s(rev.to_string())),
                    ("params", Value::Int(7)),
                ]),
                "non-object params",
            ),
            // Omitting rights entirely is an error, never CC0.
            (obj(vec![("source_blob", s(blob.to_string()))]), "no rights at all"),
            // Touching any rights key demands the complete declaration.
            (
                obj(vec![("source_blob", s(blob.to_string())), ("license", s("  "))]),
                "blank license",
            ),
            (
                obj(vec![("source_blob", s(blob.to_string())), ("license", Value::Int(7))]),
                "non-string license",
            ),
            (
                obj(vec![("source_blob", s(blob.to_string())), ("license", s("CC0-1.0"))]),
                "license without policies",
            ),
            (
                obj(vec![
                    ("source_blob", s(blob.to_string())),
                    ("license", s("CC0-1.0")),
                    ("redistribution", s("allowed")),
                    ("derivatives", s("whenever")),
                ]),
                "unknown policy word",
            ),
            (
                obj(vec![
                    ("source_blob", s(blob.to_string())),
                    ("credits", s("someone")),
                ]),
                "credits without license",
            ),
            (
                obj(vec![
                    ("source_blob", s(blob.to_string())),
                    ("source_revision", s("arev_short")),
                ]),
                "malformed source_revision",
            ),
        ] {
            assert!(DeriveRequest::from_body(&mutation).is_err(), "{why}");
        }

        // Key order does not change the canonical params digest; content does.
        let ab = params_digest(&obj(vec![("a", Value::Int(1)), ("b", Value::Int(2))]));
        let ba = params_digest(&obj(vec![("b", Value::Int(2)), ("a", Value::Int(1))]));
        assert_eq!(ab, ba);
        let other = params_digest(&obj(vec![("a", Value::Int(1)), ("b", Value::Int(3))]));
        assert_ne!(ab, other);

        // Identity is stable and namespace-scoped.
        let (asset_a, alias_a) = derived_identity("gen", &blob, &ab).unwrap();
        let (asset_b, alias_b) = derived_identity("gen", &blob, &ab).unwrap();
        assert_eq!(asset_a, asset_b);
        assert_eq!(alias_a, alias_b);
        assert!(alias_a.as_str().starts_with("gen/derived-"));
        let (asset_c, _) = derived_identity("gen", &blob, &other).unwrap();
        assert_ne!(asset_a, asset_c);
    }
}
