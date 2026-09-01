//! Catalog READS of the content tool allowlist against a real Asset Server,
//! through the hardened `makepad-asset-client` API: asset.search,
//! asset.inspect, and honest, typed refusals for everything the store no
//! longer executes (aicore §9 — generation and transforms run in the
//! creating app; `makepad-asset-creator`'s CreatorTools wraps this executor
//! and owns them). Error text is redacted before it can reach a model or a
//! transcript; the bearer token never appears on the chat wire.

use crate::session::{CancelFlag, ExecCtx, ToolExecutor};
use crate::tools::{ContentToolCall, InspectTarget};
use crate::wire::{sanitize_public_error, ToolOutcome};
use makepad_asset_client::error::{ClientError, ClientResult};
use makepad_asset_client::json::{self, Value};
use makepad_asset_client::{Api, ApiEndpoints, CatalogQuery, HttpLimits};
use makepad_asset_data::{AssetKind, AssetManifest, AssetRevisionId};

fn call_prompt(call: &ContentToolCall) -> Option<&str> {
    match call {
        ContentToolCall::ImageGenerate { prompt, .. }
        | ContentToolCall::VideoGenerate { prompt, .. }
        | ContentToolCall::AudioGenerate { prompt, .. }
        | ContentToolCall::SpeechGenerate { prompt, .. }
        | ContentToolCall::MusicGenerate { prompt, .. }
        | ContentToolCall::MeshGenerate { prompt, .. }
        | ContentToolCall::WorldGenerate { prompt, .. }
        | ContentToolCall::CharacterGenerate { prompt, .. }
        | ContentToolCall::ContentGenerate { prompt, .. } => Some(prompt.as_str()),
        _ => None,
    }
}

pub struct AssetServerTools {
    api: Api,
    /// Namespace this gateway reads as its working project.
    namespace: String,
}

impl AssetServerTools {
    /// Connect with the SERVER-side bearer token (broker or service
    /// deployment) and the namespace operations are created in. UI clients
    /// never construct this.
    pub fn connect(
        endpoints: ApiEndpoints,
        token: Option<String>,
        namespace: impl Into<String>,
    ) -> ClientResult<AssetServerTools> {
        let api = Api::new(endpoints, HttpLimits::default_v1(), token)?;
        Ok(AssetServerTools { api, namespace: namespace.into() })
    }

    // ------------------------------------------------------------ executors

    fn asset_search(&self, query: &str, limit: u32) -> ToolOutcome {
        let mut q = CatalogQuery::browse(limit);
        q.text = query.to_string();
        match self.api.catalog_search(&q, None) {
            Err(e) => err_outcome(e),
            Ok(page) => ToolOutcome::Ok {
                value: json::obj(vec![(
                    "hits",
                    Value::Arr(
                        page.hits
                            .iter()
                            .map(|h| {
                                let mut pairs = vec![
                                    ("asset", json::s(h.asset_id.to_string())),
                                    ("title", json::s(h.title.clone())),
                                    ("namespace", json::s(h.namespace.clone())),
                                ];
                                if let Some(k) = h.kind {
                                    pairs.push(("kind", json::s(kind_label(k))));
                                }
                                if let Some(a) = &h.alias {
                                    pairs.push(("alias", json::s(a.to_string())));
                                }
                                json::obj(pairs)
                            })
                            .collect(),
                    ),
                )]),
            },
        }
    }

    fn inspect(&self, target: &InspectTarget) -> ToolOutcome {
        match target {
            InspectTarget::Revision(rev) => self.revision_summary(rev),
            InspectTarget::Alias(alias) => match self.api.resolve_alias(alias) {
                Err(e) => err_outcome(e),
                Ok(dto) => ToolOutcome::Ok {
                    value: json::obj(vec![
                        ("alias", json::s(dto.alias.to_string())),
                        ("asset", json::s(dto.asset_id.to_string())),
                        ("revision", json::s(dto.head_revision.to_string())),
                    ]),
                },
            },
            InspectTarget::Asset(id) => match self.api.asset_detail(id) {
                Err(e) => err_outcome(e),
                Ok(detail) => ToolOutcome::Ok {
                    value: json::obj(vec![
                        ("asset", json::s(detail.asset_id.to_string())),
                        ("namespace", json::s(detail.namespace.clone())),
                        (
                            "candidates",
                            Value::Arr(
                                detail
                                    .candidates
                                    .iter()
                                    .map(|c| {
                                        json::obj(vec![
                                            ("revision", json::s(c.revision.to_string())),
                                            ("state", json::s(c.state.as_str())),
                                        ])
                                    })
                                    .collect(),
                            ),
                        ),
                    ]),
                },
            },
        }
    }

    fn revision_summary(&self, rev: &AssetRevisionId) -> ToolOutcome {
        let bytes = match self.api.fetch_revision_bytes(rev) {
            Ok(b) => b,
            Err(e) => return err_outcome(e),
        };
        let manifest = match AssetManifest::from_canonical_bytes(&bytes) {
            Ok(m) => m,
            Err(e) => {
                return ToolOutcome::Failed { message: public_error(&format!("manifest decode: {e}")) }
            }
        };
        let mut pairs = vec![
            ("revision", json::s(rev.to_string())),
            ("asset", json::s(manifest.asset_id.to_string())),
            ("kind", json::s(kind_label(manifest.kind))),
            ("files", Value::Int(manifest.files.len() as i64)),
        ];
        if let Some(p) = &manifest.provenance {
            pairs.push((
                "parents",
                Value::Arr(p.parents.iter().map(|r| json::s(r.to_string())).collect()),
            ));
        }
        ToolOutcome::Ok { value: json::obj(pairs) }
    }
}

impl ToolExecutor for AssetServerTools {
    fn capability_doc(&mut self) -> String {
        format!(
            "Catalog tools over namespace '{}': asset.search, asset.inspect, assets.query. \
             Generation and asset transforms run in the creator apps, not through this \
             dispatcher.",
            self.namespace
        )
    }

    fn execute(
        &mut self,
        call: &ContentToolCall,
        _ctx: &ExecCtx,
        _progress: &mut dyn FnMut(u16, &str),
        _cancel: &CancelFlag,
    ) -> ToolOutcome {
        match call {
            ContentToolCall::ImageGenerate { .. }
            | ContentToolCall::VideoGenerate { .. }
            | ContentToolCall::AudioGenerate { .. }
            | ContentToolCall::SpeechGenerate { .. }
            | ContentToolCall::MusicGenerate { .. }
            | ContentToolCall::MeshGenerate { .. }
            | ContentToolCall::WorldGenerate { .. }
            | ContentToolCall::CharacterGenerate { .. } => ToolOutcome::Ok {
                value: json::obj(vec![
                    ("queued", Value::Bool(true)),
                    ("tool", json::s(call.name())),
                    ("prompt", json::s(call_prompt(call).unwrap_or("").to_string())),
                    (
                        "note",
                        json::s(
                            "fleet generate tools are executed by the AI Content app; \
                             the Asset Server dispatcher only acknowledges the request",
                        ),
                    ),
                ]),
            },
            // Generation and transforms run in the creating app (aicore §9);
            // CreatorTools intercepts these before this executor ever sees
            // them, and a direct caller gets the same honest answer.
            ContentToolCall::ContentGenerate { .. } => ToolOutcome::Unavailable {
                reason: "content generation runs in the creator apps (makepad-asset-creator); \
                         the store only stores"
                    .to_string(),
            },
            ContentToolCall::DefaultsGet
            | ContentToolCall::DefaultsSet { .. }
            | ContentToolCall::FleetIntrospect { .. } => ToolOutcome::Unavailable {
                reason: "defaults and fleet introspection are owned by the AI Content chat session"
                    .into(),
            },
            ContentToolCall::AssetSearch { query, limit } => self.asset_search(query, *limit),
            ContentToolCall::AssetInspect { target } => self.inspect(target),
            ContentToolCall::OperationCapabilities
            | ContentToolCall::OperationCreate { .. }
            | ContentToolCall::OperationGet { .. }
            | ContentToolCall::OperationWait { .. }
            | ContentToolCall::OperationCancel { .. }
            | ContentToolCall::OperationRetry { .. } => ToolOutcome::Unavailable {
                reason: "asset transforms run in the creator apps now (the store only \
                         stores); ask the person to run the enhancement from their app"
                    .to_string(),
            },
            ContentToolCall::LlmConsult { .. } => ToolOutcome::Unavailable {
                reason: "llm.consult is executed by the chat broker".to_string(),
            },
            // Game-client tools: the broker's dispatcher never advertises
            // them (see `ToolExecutor::tool_definitions`); a model that
            // calls one anyway gets the honest answer, not an execution.
            ContentToolCall::AssetsQuery { .. }
            | ContentToolCall::AssetsSchema
            | ContentToolCall::ModelBuild { .. }
            | ContentToolCall::ModelFetch { .. }
            | ContentToolCall::WorldPlace { .. }
            | ContentToolCall::WorldRemove { .. }
            | ContentToolCall::WorldMove { .. }
            | ContentToolCall::WorldList
            | ContentToolCall::WorldGetSource
            | ContentToolCall::WorldSetSource { .. }
            | ContentToolCall::WorldNewLevel { .. }
            | ContentToolCall::WorldSetPlayerModel { .. }
            | ContentToolCall::WorldSpawn { .. }
            | ContentToolCall::WorldTune { .. }
            | ContentToolCall::WorldAddAddon { .. }
            | ContentToolCall::WorldInSub { .. } => ToolOutcome::Unavailable {
                reason: "catalog SQL, local model, and world tools run in a game chat session"
                    .to_string(),
            },
        }
    }
}

/// Typed error mapping: authorization failures are `Denied` (the ACL
/// answer), everything else is an operational failure with a bounded
/// description. Never a panic, never a silent Ok.
fn err_outcome(e: ClientError) -> ToolOutcome {
    match e {
        ClientError::Unauthenticated => {
            ToolOutcome::Denied { what: "not authenticated to the asset server".to_string() }
        }
        ClientError::Denied => {
            ToolOutcome::Denied { what: "the asset server denied this operation".to_string() }
        }
        ClientError::NotFound { what } => {
            ToolOutcome::Failed { message: public_error(&format!("not found: {what}")) }
        }
        ClientError::Server { status: 409, detail } => ToolOutcome::Refused {
            what: public_error(&format!(
                "the asset server refused this state transition: {}",
                detail.as_deref().unwrap_or("conflict")
            )),
        },
        other => ToolOutcome::Failed { message: public_error(&other.to_string()) },
    }
}

fn public_error(message: &str) -> String {
    sanitize_public_error(message)
}

pub(crate) fn kind_label(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Mesh => "mesh",
        AssetKind::Character => "character",
        AssetKind::Weapon => "weapon",
        AssetKind::Vehicle => "vehicle",
        AssetKind::Prop => "prop",
        AssetKind::Texture => "texture",
        AssetKind::Material => "material",
        AssetKind::Audio => "audio",
        AssetKind::Video => "video",
        AssetKind::Skybox => "skybox",
        AssetKind::World => "world",
        AssetKind::Prefab => "prefab",
        AssetKind::Billboard => "billboard",
        AssetKind::Game => "game",
        AssetKind::VjEffect => "vjeffect",
        AssetKind::Data => "data",
        AssetKind::ModelProgram => "model-program",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_no_credential(s: &str) {
        assert!(!s.to_ascii_lowercase().contains("bearer"), "{s}");
        assert!(!s.contains("mpat_"), "{s}");
    }

    /// Error text that could carry a bearer header is redacted before it
    /// reaches a model or a transcript.
    #[test]
    fn tool_errors_redact_bearer_mpat() {
        match err_outcome(ClientError::Server {
            status: 500,
            detail: Some("Authorization: Bearer mpat_LEAK123".into()),
        }) {
            ToolOutcome::Failed { message } => {
                assert_eq!(message, "provider error");
                assert_no_credential(&message);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        match err_outcome(ClientError::Server {
            status: 409,
            detail: Some("Authorization: Bearer mpat_LEAK123".into()),
        }) {
            ToolOutcome::Refused { what } => assert_no_credential(&what),
            other => panic!("expected Refused, got {other:?}"),
        }
    }
}
