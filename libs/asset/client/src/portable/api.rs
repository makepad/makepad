//! Portable request vocabulary shared with UI code. HTTP execution lives in
//! the polled static runtime, not in a blocking API facade.

use crate::dto::StageOnFailDto;
use crate::dto::{AliasDto, AssetDetailDto, AssetsQueryDto, CatalogPageDto};
use crate::error::{ClientError, ClientResult};
use crate::json::{self, Value};
use crate::{ApiEndpoints, HttpLimits};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, AssetRevisionId, SourceCollectionId,
};

pub const MAX_SEARCH_LIMIT: u32 = 100;
pub const MAX_LIST_LIMIT: u64 = 500;

pub struct Api;

impl Api {
    pub fn new(
        _endpoints: ApiEndpoints,
        _limits: HttpLimits,
        _token: Option<String>,
    ) -> ClientResult<Self> {
        Err(ClientError::Unavailable {
            capability: "blocking_api",
            mode: crate::ClientMode::StaticWeb,
        })
    }

    pub fn catalog_search(
        &self,
        _query: &CatalogQuery,
        _cursor: Option<&str>,
    ) -> ClientResult<CatalogPageDto> {
        unavailable()
    }

    pub fn assets_query(&self, _sql: &str) -> ClientResult<AssetsQueryDto> {
        unavailable()
    }

    pub fn asset_detail(&self, _id: &AssetId) -> ClientResult<AssetDetailDto> {
        unavailable()
    }

    pub fn resolve_alias(&self, _alias: &AssetAlias) -> ClientResult<AliasDto> {
        unavailable()
    }

    pub fn fetch_revision_bytes(&self, _revision: &AssetRevisionId) -> ClientResult<Vec<u8>> {
        unavailable()
    }
}

fn unavailable<T>() -> ClientResult<T> {
    Err(ClientError::Unavailable {
        capability: "blocking_api",
        mode: crate::ClientMode::StaticWeb,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogQuery {
    pub text: String,
    pub namespace: Option<String>,
    pub kind: Option<AssetKind>,
    pub category: Option<String>,
    pub tag: Option<String>,
    pub exclude_tag: Option<String>,
    pub creator: Option<String>,
    pub live_only: bool,
    pub page_size: u32,
    pub facets: u32,
}

impl CatalogQuery {
    pub fn browse(page_size: u32) -> Self {
        Self { page_size, ..Self::default() }
    }

    pub fn text(text: impl Into<String>, page_size: u32) -> Self {
        Self { text: text.into(), page_size, ..Self::default() }
    }

    pub(crate) fn validate(&self) -> ClientResult<()> {
        if self.page_size == 0 || self.page_size > MAX_SEARCH_LIMIT {
            return Err(ClientError::InvalidInput { what: "search page_size" });
        }
        if self.text.len() > crate::wire::MAX_QUERY_TEXT_BYTES
            || self.text.chars().any(char::is_control)
        {
            return Err(ClientError::InvalidInput { what: "search text" });
        }
        for value in [
            &self.namespace,
            &self.category,
            &self.tag,
            &self.exclude_tag,
            &self.creator,
        ]
        .into_iter()
        .flatten()
        {
            if value.is_empty()
                || value.len() > crate::wire::MAX_FILTER_VALUE_BYTES
                || value.chars().any(char::is_control)
            {
                return Err(ClientError::InvalidInput { what: "search filter value" });
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobHead {
    pub size: u64,
    pub etag_matches: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcRequest {
    pub dry_run: bool,
    pub grace_ms: Option<u64>,
    pub retain_per_asset: Option<u32>,
    pub max_steps: Option<u32>,
}

impl GcRequest {
    pub fn dry_run() -> Self { Self { dry_run: true, ..Self::default() } }
    pub fn collect() -> Self { Self::default() }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnnotationUpload {
    pub title: String,
    pub description: String,
    pub kind: Option<AssetKind>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    pub prompt: String,
    pub provenance: String,
    pub private: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatAttachment {
    pub revision: AssetRevisionId,
    pub role: String,
}

pub const DEFAULT_STAGE_WEIGHTS: &[(&str, u16)] = &[
    ("text.expand", 5),
    ("image.generate", 15),
    ("image.upscale", 25),
    ("video.generate", 70),
    ("video.enhance", 25),
    ("music.generate", 60),
    ("mesh.generate", 40),
];
pub const NEUTRAL_STAGE_WEIGHT: u16 = 10;

pub fn default_stage_weight(kind: &str) -> u16 {
    DEFAULT_STAGE_WEIGHTS
        .iter()
        .find(|(candidate, _)| *candidate == kind)
        .map(|(_, weight)| *weight)
        .unwrap_or(NEUTRAL_STAGE_WEIGHT)
}

pub fn stage_ref(stage: &str, field: &str) -> Value {
    json::obj(vec![("$from_stage", json::s(stage)), ("field", json::s(field))])
}

#[derive(Clone, Debug, PartialEq)]
pub struct PipelineStageSpec {
    pub name: String,
    pub kind: String,
    pub body: Value,
    pub weight: Option<u16>,
    pub on_fail: StageOnFailDto,
    pub deps: Option<Vec<String>>,
    pub priority: i64,
    pub max_attempts: Option<u32>,
}

impl PipelineStageSpec {
    pub fn new(name: impl Into<String>, kind: impl Into<String>, body: Value) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            body,
            weight: None,
            on_fail: StageOnFailDto::Fail,
            deps: None,
            priority: 0,
            max_attempts: None,
        }
    }

    pub fn on_fail_skip(mut self) -> Self {
        self.on_fail = StageOnFailDto::Skip;
        self
    }

    pub fn with_weight(mut self, weight: u16) -> Self {
        self.weight = Some(weight);
        self
    }

    pub fn with_deps(mut self, deps: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.deps = Some(deps.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = Some(attempts);
        self
    }

    pub fn weight(&self) -> u16 {
        self.weight.unwrap_or_else(|| default_stage_weight(&self.kind))
    }

    pub fn to_value(&self) -> ClientResult<Value> {
        if !stage_token_ok(&self.name) || !stage_token_ok(&self.kind) {
            return Err(ClientError::InvalidInput { what: "pipeline stage token" });
        }
        if !matches!(self.body, Value::Obj(_)) {
            return Err(ClientError::InvalidInput { what: "stage body must be an object" });
        }
        let weight = self.weight();
        if weight == 0 || weight > crate::wire::MAX_STAGE_WEIGHT {
            return Err(ClientError::InvalidInput { what: "stage weight" });
        }
        let mut pairs = vec![
            ("name", json::s(&self.name)),
            ("kind", json::s(&self.kind)),
            ("body", self.body.clone()),
            ("weight", Value::Int(weight as i64)),
            ("on_fail", json::s(self.on_fail.as_str())),
        ];
        if let Some(deps) = &self.deps {
            pairs.push((
                "deps",
                Value::Arr(deps.iter().map(|dependency| json::s(dependency)).collect()),
            ));
        }
        if self.priority != 0 {
            pairs.push(("priority", Value::Int(self.priority)));
        }
        if let Some(attempts) = self.max_attempts {
            if attempts == 0 {
                return Err(ClientError::InvalidInput { what: "stage max_attempts" });
            }
            pairs.push(("max_attempts", Value::Int(attempts as i64)));
        }
        Ok(json::obj(pairs))
    }
}

fn stage_token_ok(token: &str) -> bool {
    (1..=64).contains(&token.len())
        && token.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'-' | b'_')
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCollectionRegistered {
    pub source_id: String,
    pub digest: SourceCollectionId,
}
