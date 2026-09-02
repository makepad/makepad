//! Derived variants: canonical recipes, the single-flight derivation cache,
//! immutable variant records, frozen variant sets, and deterministic
//! per-client resolution.
//!
//! Laws enforced here, all fail-closed:
//! - The cache key is the content contract's [`derivation_key`] over base
//!   revision, recipe digest, and exact inputs. One key names at most one
//!   ready variant, ever: the winner row is immutable and a late duplicate
//!   completion with different bytes refuses.
//! - The server never executes processing kernels. `begin_derivation` arms a
//!   typed worker job; `complete_derivation` validates the returned facts
//!   against the recipe (dims, roles, budgets, blob existence/sizes) and only
//!   then publishes — a worker is compute, not content authority.
//! - Job identity is deterministic: `sha256(dkey ‖ round)[..16]`, so retries
//!   and crash-repair re-arm the SAME job instead of minting strays, and the
//!   transport's meta-then-enqueue ordering can recover from any crash point.
//! - The canonical `DerivedVariantManifest` carries only deterministic
//!   content facts; job/worker/attempt/timestamps stay in these control-plane
//!   rows so two clean servers produce byte-identical variant IDs.
//! - Freezing a set is idempotent by digest and never edits the base
//!   revision or an older set.

use crate::budget::Budgets;
use crate::catalog::{fixed16, fixed32, CandidateState, Catalog};
use crate::error::{ServerError, ServerResult};
// The queue is gone (aicore P7): derivation is client-driven now. The
// deterministic 16-byte job identity SURVIVES — it is what makes a retry
// re-arm the same derivation instead of minting strays — as a local type
// with the same wire and row shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobId(pub [u8; 16]);

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "job_")?;
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}
use crate::sqlite::Db;
use makepad_asset_data::sha256::Sha256;
use makepad_asset_data::{
    derivation_key, resolve_variants, AssetFile, AssetManifest, AssetRevisionRef, ClientProfile,
    DerivationKey, DerivedInput, DerivedVariantId, DerivedVariantManifest, Metrics,
    ProcessingRecipe, RecipeDigest, ResolvedVariantMap, ThumbnailMeta, VariantSetId,
    VariantSetManifest,
};

pub const VARIANT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS recipes(
    recipe_digest BLOB PRIMARY KEY,
    recipe BLOB NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS derivations(
    dkey BLOB PRIMARY KEY,
    base_asset BLOB NOT NULL,
    base_revision BLOB NOT NULL,
    recipe_digest BLOB NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','ready','failed')),
    round INTEGER NOT NULL DEFAULT 0,
    job_id BLOB NOT NULL,
    variant BLOB,
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS derivations_by_job ON derivations(job_id);
CREATE TABLE IF NOT EXISTS derived_variants(
    variant BLOB PRIMARY KEY,
    dkey BLOB NOT NULL,
    base_asset BLOB NOT NULL,
    base_revision BLOB NOT NULL,
    manifest BLOB NOT NULL,
    created_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS variant_sets(
    set_digest BLOB PRIMARY KEY,
    base_asset BLOB NOT NULL,
    base_revision BLOB NOT NULL,
    manifest BLOB NOT NULL,
    created_ms INTEGER NOT NULL
);
";

/// How many rounds of terminal job failure one derivation key may re-arm
/// through before the server refuses further automatic retries.
pub const MAX_DERIVATION_ROUNDS: u32 = 8;

/// Outcome of a derivation request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DerivationOutcome {
    /// The cache already holds the validated result.
    Ready {
        dkey: DerivationKey,
        variant: DerivedVariantId,
    },
    /// A live job is already producing this key; the caller joins it.
    InFlight { dkey: DerivationKey, job_id: JobId },
    /// A job row was armed (fresh, re-armed after terminal failure, or
    /// crash-repair where the job vanished). The caller must now enqueue
    /// exactly this job through the jobs system with `kind`.
    NeedsJob {
        dkey: DerivationKey,
        job_id: JobId,
        kind: &'static str,
        recipe_digest: RecipeDigest,
    },
}

/// Live status of one derivation key, joined with job state at read time so
/// crash windows report truthfully.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivationStatus {
    pub dkey: DerivationKey,
    pub base: AssetRevisionRef,
    pub recipe_digest: RecipeDigest,
    /// "ready" | "pending" | "failed"
    pub state: &'static str,
    pub round: u32,
    pub job_id: JobId,

    pub variant: Option<DerivedVariantId>,
}

/// The worker-reported facts of one completed derivation. The server rebuilds
/// the canonical manifest itself; a worker cannot author identity fields.
#[derive(Clone, Debug, PartialEq)]
pub struct DerivedResult {
    pub outputs: Vec<AssetFile>,
    pub thumbnail: Option<ThumbnailMeta>,
    pub metrics: Metrics,
}

pub struct Variants<'a> {
    pub(crate) db: &'a Db,
    pub(crate) budgets: &'a Budgets,
}

struct DerivationRow {
    base: AssetRevisionRef,
    recipe_digest: RecipeDigest,
    state: String,
    round: u32,
    job_id: JobId,
    variant: Option<DerivedVariantId>,
}

impl<'a> Variants<'a> {
    fn catalog(&self) -> Catalog<'a> {
        Catalog {
            db: self.db,
            budgets: self.budgets,
        }
    }

    // ---- request / single flight -------------------------------------------

    /// Request one derivation. Validates the recipe and base, derives the
    /// exact inputs, and returns the single-flight outcome. Never enqueues
    /// the job itself: the caller (transport) enqueues `NeedsJob` results so
    /// its job-metadata invariants hold; a crash between the two steps is
    /// repaired here by re-offering the same deterministic job id.
    pub fn begin_derivation(
        &self,
        base: &AssetRevisionRef,
        recipe_bytes: &[u8],
        now_ms: u64,
    ) -> ServerResult<DerivationOutcome> {
        if recipe_bytes.len() as u64 > self.budgets.max_manifest_bytes {
            return Err(ServerError::OverBudget {
                what: "recipe bytes",
                limit: self.budgets.max_manifest_bytes,
                found: recipe_bytes.len() as u64,
            });
        }
        let recipe = ProcessingRecipe::from_canonical_bytes(recipe_bytes)?;
        let recipe_digest = RecipeDigest::hash_of(recipe_bytes);
        let (_, inputs) = self.base_inputs(base, &recipe)?;
        let dkey = derivation_key(base, &recipe_digest, &inputs)?;

        self.db.tx(|db| {
            if let Some(row) = self.derivation_row(&dkey)? {
                match row.state.as_str() {
                    "ready" => {
                        let variant = row.variant.ok_or(ServerError::InvalidState {
                            what: "derivation row",
                            state: "ready without variant",
                        })?;
                        return Ok(DerivationOutcome::Ready { dkey, variant });
                    }
                    // Client-driven now: a pending row IS the in-flight
                    // marker — its deriver holds the deterministic id and
                    // completes or abandons it.
                    "pending" => {
                        return Ok(DerivationOutcome::InFlight {
                            dkey,
                            job_id: row.job_id,
                        })
                    }
                    "failed" => {
                        {
                            {
                                // Re-arm the next round with a fresh
                                // deterministic job id.
                                let round = row.round.checked_add(1).ok_or(
                                    ServerError::InvalidState {
                                        what: "derivation round",
                                        state: "overflow",
                                    },
                                )?;
                                if round >= MAX_DERIVATION_ROUNDS {
                                    return Err(ServerError::InvalidState {
                                        what: "derivation",
                                        state: "retry rounds exhausted",
                                    });
                                }
                                let job_id = derivation_job_id(&dkey, round);
                                let mut s = db.prepare(
                                    "re-arm derivation",
                                    "UPDATE derivations SET state='pending', round=?2,
                                        job_id=?3, updated_ms=?4 WHERE dkey=?1",
                                )?;
                                s.bind_blob(1, dkey.as_bytes())?;
                                s.bind_i64(2, round as i64)?;
                                s.bind_blob(3, &job_id.0)?;
                                s.bind_u64(4, now_ms)?;
                                s.run()?;
                                return Ok(DerivationOutcome::NeedsJob {
                                    dkey,
                                    job_id,
                                    kind: recipe.kind().job_kind(),
                                    recipe_digest,
                                });
                            }
                        }
                    }
                    other => {
                        let _ = other;
                        return Err(ServerError::InvalidState {
                            what: "derivation row",
                            state: "unknown",
                        });
                    }
                }
            }
            // Fresh key: record the recipe and arm round 0.
            let mut s = db.prepare(
                "insert recipe",
                "INSERT OR IGNORE INTO recipes(recipe_digest, recipe, created_ms)
                 VALUES(?1, ?2, ?3)",
            )?;
            s.bind_blob(1, recipe_digest.as_bytes())?;
            s.bind_blob(2, recipe_bytes)?;
            s.bind_u64(3, now_ms)?;
            s.run()?;
            drop(s);
            let job_id = derivation_job_id(&dkey, 0);
            let mut s = db.prepare(
                "insert derivation",
                "INSERT INTO derivations(dkey, base_asset, base_revision, recipe_digest,
                                         state, round, job_id, created_ms, updated_ms)
                 VALUES(?1, ?2, ?3, ?4, 'pending', 0, ?5, ?6, ?6)",
            )?;
            s.bind_blob(1, dkey.as_bytes())?;
            s.bind_blob(2, base.asset_id.as_bytes())?;
            s.bind_blob(3, base.revision.as_bytes())?;
            s.bind_blob(4, recipe_digest.as_bytes())?;
            s.bind_blob(5, &job_id.0)?;
            s.bind_u64(6, now_ms)?;
            s.run()?;
            Ok(DerivationOutcome::NeedsJob {
                dkey,
                job_id,
                kind: recipe.kind().job_kind(),
                recipe_digest,
            })
        })
    }

    /// Decode the base manifest and derive the exact recipe inputs from it.
    /// The base must exist, belong to the named asset, and not be
    /// quarantined; the recipe's input role must be present.
    fn base_inputs(
        &self,
        base: &AssetRevisionRef,
        recipe: &ProcessingRecipe,
    ) -> ServerResult<(AssetManifest, Vec<DerivedInput>)> {
        let catalog = self.catalog();
        let manifest_bytes = catalog
            .asset_revision_manifest(&base.revision)?
            .ok_or(ServerError::NotFound { what: "derivation base revision" })?;
        let manifest = AssetManifest::from_canonical_bytes(&manifest_bytes)?;
        if manifest.asset_id != base.asset_id {
            return Err(ServerError::Conflict { what: "derivation base asset" });
        }
        match catalog.asset_candidate_state(&base.asset_id, &base.revision)? {
            Some(CandidateState::Quarantined) => {
                return Err(ServerError::InvalidState {
                    what: "derivation base",
                    state: "quarantined",
                })
            }
            Some(_) => {}
            None => return Err(ServerError::NotFound { what: "derivation base candidate" }),
        }
        // Rights gate: content whose registered terms forbid derivatives can
        // never be processed, whatever capability the caller holds. Checked
        // here so BOTH arming and completion enforce it.
        if manifest.rights.derivatives == makepad_asset_data::DerivativePolicy::Forbidden {
            return Err(ServerError::InvalidState {
                what: "derivation rights",
                state: "derivatives forbidden",
            });
        }
        let role = recipe.settings.input_role();
        // The exact input: the base's file(s) of the recipe's input role. The
        // canonical (role, tier, lod) order of the manifest makes the input
        // list deterministic.
        let inputs: Vec<DerivedInput> = manifest
            .files
            .iter()
            .filter(|f| f.role == role)
            .map(|f| DerivedInput {
                role: f.role,
                blob: f.blob,
            })
            .collect();
        if inputs.is_empty() {
            return Err(ServerError::NotFound { what: "recipe input role in base" });
        }
        Ok((manifest, inputs))
    }

    fn derivation_row(&self, dkey: &DerivationKey) -> ServerResult<Option<DerivationRow>> {
        let mut s = self.db.prepare(
            "derivation row",
            "SELECT base_asset, base_revision, recipe_digest, state, round, job_id, variant
             FROM derivations WHERE dkey = ?1",
        )?;
        s.bind_blob(1, dkey.as_bytes())?;
        if !s.step()? {
            return Ok(None);
        }
        Ok(Some(DerivationRow {
            base: AssetRevisionRef {
                asset_id: makepad_asset_data::AssetId::from_bytes(fixed16(
                    &s.column_blob(0),
                    "derivation row",
                )?),
                revision: makepad_asset_data::AssetRevisionId::from_bytes(fixed32(
                    &s.column_blob(1),
                    "derivation row",
                )?),
            },
            recipe_digest: RecipeDigest::from_bytes(fixed32(&s.column_blob(2), "derivation row")?),
            state: s.column_text(3),
            round: u32::try_from(s.column_u64(4)).map_err(|_| ServerError::InvalidState {
                what: "derivation row",
                state: "round corrupt",
            })?,
            job_id: JobId(fixed16(&s.column_blob(5), "derivation row")?),
            variant: if s.column_is_null(6) {
                None
            } else {
                Some(DerivedVariantId::from_bytes(fixed32(
                    &s.column_blob(6),
                    "derivation row",
                )?))
            },
        }))
    }

    /// Find the derivation a worker's claimed job belongs to.
    pub fn derivation_of_job(&self, job_id: &JobId) -> ServerResult<Option<DerivationKey>> {
        let mut s = self.db.prepare(
            "derivation of job",
            "SELECT dkey FROM derivations WHERE job_id = ?1",
        )?;
        s.bind_blob(1, &job_id.0)?;
        if !s.step()? {
            return Ok(None);
        }
        Ok(Some(DerivationKey::from_bytes(fixed32(
            &s.column_blob(0),
            "derivation row",
        )?)))
    }

    /// Live status joined with job state, or None for an unknown key.
    pub fn derivation_status(&self, dkey: &DerivationKey) -> ServerResult<Option<DerivationStatus>> {
        let Some(row) = self.derivation_row(dkey)? else {
            return Ok(None);
        };
        let state = match row.state.as_str() {
            "ready" => "ready",
            // Client-driven now: a pending row stays pending until its
            // deriver completes it (or a fresh begin re-offers the same
            // deterministic id — the crash-repair path unchanged).
            "pending" => "pending",
            _ => "failed",
        };
        Ok(Some(DerivationStatus {
            dkey: *dkey,
            base: row.base,
            recipe_digest: row.recipe_digest,
            state,
            round: row.round,
            job_id: row.job_id,
            variant: row.variant,
        }))
    }

    /// Canonical bytes of one recorded recipe.
    pub fn recipe_bytes(&self, digest: &RecipeDigest) -> ServerResult<Option<Vec<u8>>> {
        let mut s = self.db.prepare(
            "recipe bytes",
            "SELECT recipe FROM recipes WHERE recipe_digest = ?1",
        )?;
        s.bind_blob(1, digest.as_bytes())?;
        if s.step()? {
            Ok(Some(s.column_blob(0)))
        } else {
            Ok(None)
        }
    }

    // ---- completion --------------------------------------------------------

    /// Validate and publish a worker's completed derivation, atomically with
    /// its job success. The worker names the derivation key it claims to
    /// have produced (the transport route carries it in the path) plus its
    /// job; a job superseded by a newer round refuses as lease loss. The
    /// server rebuilds the canonical manifest from the recorded base/recipe
    /// plus the worker's claimed facts, validates it against the recipe
    /// contract and the blob store, and installs the single-flight winner.
    /// An identical late duplicate is idempotent; a divergent one refuses
    /// without touching the winner.
    pub fn complete_derivation(
        &self,
        dkey: &DerivationKey,
        job_id: &JobId,
        worker: &str,
        result: &DerivedResult,
        now_ms: u64,
    ) -> ServerResult<DerivedVariantId> {
        let catalog = self.catalog();
        self.db.tx(|db| {
            let dkey = *dkey;
            let row = self
                .derivation_row(&dkey)?
                .ok_or(ServerError::NotFound { what: "derivation row" })?;
            // A newer round superseded this job while it was still running:
            // its lease is gone and its report can never publish. (A READY
            // row still compares results below so an identical late
            // duplicate stays idempotent.)
            if row.state != "ready" && row.job_id != *job_id {
                return Err(ServerError::LeaseLost { what: "superseded derivation job" });
            }

            // Rebuild the deterministic manifest the server is willing to
            // publish for this key.
            let recipe_bytes = self
                .recipe_bytes(&row.recipe_digest)?
                .ok_or(ServerError::NotFound { what: "derivation recipe" })?;
            let recipe = ProcessingRecipe::from_canonical_bytes(&recipe_bytes)?;
            let (base_manifest, inputs) = self.base_inputs(&row.base, &recipe)?;
            // Derived variants inherit the base's EXACT rights record —
            // license identity/revision, terms digest, attribution, upstream
            // provenance, and policy — nothing weakened, nothing dropped.
            let mut manifest = DerivedVariantManifest {
                base: row.base,
                kind: recipe.kind(),
                recipe: row.recipe_digest,
                inputs,
                outputs: result.outputs.clone(),
                thumbnail: result.thumbnail.clone(),
                metrics: result.metrics,
                rights: base_manifest.rights.clone(),
            };
            manifest.canonicalize();
            recipe.validate_result(&manifest)?;
            // Every output blob must already be in CAS at the declared size.
            for f in &manifest.outputs {
                catalog.require_blob_sized(
                    &f.blob,
                    f.byte_len,
                    "variant output blob",
                    "variant output blob size",
                )?;
            }
            if let Some(t) = &manifest.thumbnail {
                catalog.require_blob_sized(
                    &t.blob,
                    t.byte_len,
                    "variant thumbnail blob",
                    "variant thumbnail blob size",
                )?;
            }
            let manifest_bytes = manifest.to_canonical_bytes()?;
            let variant = DerivedVariantId::hash_of(&manifest_bytes);

            if row.state == "ready" {
                // Late duplicate: identical result is an idempotent success;
                // different bytes can never replace the winner.
                return match row.variant {
                    Some(winner) if winner == variant => Ok(winner),
                    _ => Err(ServerError::Conflict { what: "late duplicate derivation" }),
                };
            }
            // Client-driven now: identity replaces the lease — the report
            // must carry the exact armed job id (checked above) and names
            // its worker for the record.
            let _ = worker;

            let mut s = db.prepare(
                "insert derived variant",
                "INSERT OR IGNORE INTO derived_variants(variant, dkey, base_asset, base_revision,
                                                        manifest, created_ms)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            s.bind_blob(1, variant.as_bytes())?;
            s.bind_blob(2, dkey.as_bytes())?;
            s.bind_blob(3, row.base.asset_id.as_bytes())?;
            s.bind_blob(4, row.base.revision.as_bytes())?;
            s.bind_blob(5, &manifest_bytes)?;
            s.bind_u64(6, now_ms)?;
            s.run()?;
            drop(s);
            // A variant makes its output bytes reachable; an in-flight blob
            // GC run learns about them in THIS transaction (see crate::gc).
            let mut pinned: Vec<makepad_asset_data::BlobId> =
                manifest.outputs.iter().map(|f| f.blob).collect();
            if let Some(t) = &manifest.thumbnail {
                pinned.push(t.blob);
            }
            crate::gc::pin_blobs_in_tx(db, &pinned)?;
            let mut s = db.prepare(
                "ready derivation",
                "UPDATE derivations SET state='ready', variant=?2, updated_ms=?3 WHERE dkey=?1",
            )?;
            s.bind_blob(1, dkey.as_bytes())?;
            s.bind_blob(2, variant.as_bytes())?;
            s.bind_u64(3, now_ms)?;
            s.run()?;
            drop(s);
            Ok(variant)
        })
    }

    /// Canonical bytes of one derived variant.
    pub fn variant_manifest(&self, variant: &DerivedVariantId) -> ServerResult<Option<Vec<u8>>> {
        let mut s = self.db.prepare(
            "variant manifest",
            "SELECT manifest FROM derived_variants WHERE variant = ?1",
        )?;
        s.bind_blob(1, variant.as_bytes())?;
        if s.step()? {
            Ok(Some(s.column_blob(0)))
        } else {
            Ok(None)
        }
    }

    // ---- variant sets ------------------------------------------------------

    /// Freeze an immutable variant set over ready variants of one base.
    /// Idempotent by digest; refuses unknown variants, foreign bases, and
    /// variants whose derivation is not the recorded ready winner.
    pub fn freeze_variant_set(
        &self,
        base: &AssetRevisionRef,
        variants: &[DerivedVariantId],
        now_ms: u64,
    ) -> ServerResult<VariantSetId> {
        let mut set = VariantSetManifest {
            base: *base,
            variants: variants.to_vec(),
            policy_version: makepad_asset_data::RESOLUTION_POLICY_V1,
        };
        set.canonicalize();
        let set_bytes = set.to_canonical_bytes()?;
        let set_id = VariantSetId::hash_of(&set_bytes);
        self.db.tx(|db| {
            for v in &set.variants {
                let manifest_bytes = self
                    .variant_manifest(v)?
                    .ok_or(ServerError::NotFound { what: "variant for set" })?;
                let manifest = DerivedVariantManifest::from_canonical_bytes(&manifest_bytes)?;
                if manifest.base != *base {
                    return Err(ServerError::Conflict { what: "variant set base" });
                }
            }
            let mut s = db.prepare(
                "freeze variant set",
                "INSERT OR IGNORE INTO variant_sets(set_digest, base_asset, base_revision,
                                                    manifest, created_ms)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
            )?;
            s.bind_blob(1, set_id.as_bytes())?;
            s.bind_blob(2, base.asset_id.as_bytes())?;
            s.bind_blob(3, base.revision.as_bytes())?;
            s.bind_blob(4, &set_bytes)?;
            s.bind_u64(5, now_ms)?;
            s.run()?;
            Ok(set_id)
        })
    }

    /// Canonical bytes of one frozen set.
    pub fn variant_set_manifest(&self, set: &VariantSetId) -> ServerResult<Option<Vec<u8>>> {
        let mut s = self.db.prepare(
            "variant set manifest",
            "SELECT manifest FROM variant_sets WHERE set_digest = ?1",
        )?;
        s.bind_blob(1, set.as_bytes())?;
        if s.step()? {
            Ok(Some(s.column_blob(0)))
        } else {
            Ok(None)
        }
    }

    /// Frozen variant sets for one exact base revision, in digest order.
    /// This bounded public read lets snapshot exporters discover the
    /// immutable derived documents reachable from selected content without
    /// exposing the variants database or any derivation/job history.
    pub fn variant_sets_for_base(
        &self,
        base: &AssetRevisionRef,
        after: Option<&VariantSetId>,
        limit: u32,
    ) -> ServerResult<Vec<VariantSetId>> {
        if limit == 0 || limit > self.budgets.max_search_results {
            return Err(ServerError::OverBudget {
                what: "variant set export page size",
                limit: self.budgets.max_search_results as u64,
                found: limit as u64,
            });
        }
        let mut s = self.db.prepare(
            "variant sets for base",
            "SELECT set_digest FROM variant_sets
             WHERE base_asset=?1 AND base_revision=?2
               AND (?3 IS NULL OR set_digest>?3)
             ORDER BY set_digest LIMIT ?4",
        )?;
        s.bind_blob(1, base.asset_id.as_bytes())?;
        s.bind_blob(2, base.revision.as_bytes())?;
        match after {
            Some(after) => s.bind_blob(3, after.as_bytes())?,
            None => s.bind_null(3)?,
        }
        s.bind_u64(4, limit as u64)?;
        let mut out = Vec::new();
        while s.step()? {
            out.push(VariantSetId::from_bytes(fixed32(
                &s.column_blob(0),
                "variant set export row",
            )?));
        }
        Ok(out)
    }

    // ---- resolution --------------------------------------------------------

    /// Deterministic read-only resolution of one frozen set against a bounded
    /// client profile. Pure over recorded state: never schedules work, never
    /// mutates a set, never consults anything but the named manifests.
    pub fn resolve(
        &self,
        set_id: &VariantSetId,
        profile: &ClientProfile,
    ) -> ServerResult<ResolvedVariantMap> {
        let set_bytes = self
            .variant_set_manifest(set_id)?
            .ok_or(ServerError::NotFound { what: "variant set" })?;
        let set = VariantSetManifest::from_canonical_bytes(&set_bytes)?;
        let mut manifests = Vec::with_capacity(set.variants.len());
        for v in &set.variants {
            let bytes = self
                .variant_manifest(v)?
                .ok_or(ServerError::NotFound { what: "variant in set" })?;
            manifests.push(DerivedVariantManifest::from_canonical_bytes(&bytes)?);
        }
        Ok(resolve_variants(&set, &manifests, profile)?)
    }
}

/// Deterministic job identity for one derivation round.
fn derivation_job_id(dkey: &DerivationKey, round: u32) -> JobId {
    let mut h = Sha256::new();
    h.update(b"mp-derivation-job:v1\0");
    h.update(dkey.as_bytes());
    h.update(&round.to_be_bytes());
    let digest = h.finalize();
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest[..16]);
    JobId(id)
}
