//! Portable configuration and the deliberately unavailable blocking facade.

use crate::error::{ClientError, ClientResult};
use crate::location::{
    ApiEndpoints, BaseUrl, ClientLocation, ClientMode, CAPABILITY_BLOCKING_API,
    CAPABILITY_STATIC_SITE_SESSION,
};
use std::path::PathBuf;
use crate::api::{AnnotationUpload, CatalogQuery};
use crate::dto::{AliasStatusDto, CatalogFacet, CatalogHit};
use makepad_asset_data::{AssetAlias, AssetId, AssetManifest, AssetRevisionId, BlobId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageCursor {
    server_id: [u8; 16],
    token: String,
}

impl PageCursor {
    pub fn server_id(&self) -> &[u8; 16] {
        &self.server_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPage {
    pub hits: Vec<CatalogHit>,
    pub total: u64,
    pub next: Option<PageCursor>,
    pub facets: Vec<CatalogFacet>,
}

#[derive(Clone, Copy, Debug)]
pub struct CacheBudgets {
    pub max_total_bytes: u64,
    pub max_object_bytes: u64,
    pub max_partial_bytes: u64,
    pub stale_partial_ms: u64,
    pub max_ram_bytes: u64,
}

impl CacheBudgets {
    pub fn default_v1() -> Self {
        Self {
            max_total_bytes: 4 * 1024 * 1024 * 1024,
            max_object_bytes: 256 * 1024 * 1024,
            max_partial_bytes: 512 * 1024 * 1024,
            stale_partial_ms: 7 * 24 * 60 * 60 * 1000,
            max_ram_bytes: 512 * 1024 * 1024,
        }
    }

    pub fn validate(&self) -> ClientResult<()> {
        if self.max_total_bytes == 0
            || self.max_object_bytes == 0
            || self.max_partial_bytes == 0
            || self.stale_partial_ms == 0
            || self.max_ram_bytes == 0
            || self.max_object_bytes > self.max_total_bytes
        {
            return Err(ClientError::InvalidInput { what: "cache budgets" });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub object_count: u64,
    pub total_bytes: u64,
    pub pinned_bytes: u64,
    pub partial_bytes: u64,
    pub evictions: u64,
    pub corruption_evictions: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct HttpLimits {
    pub connect_timeout_ms: u64,
    pub read_timeout_ms: u64,
    pub write_timeout_ms: u64,
    pub head_deadline_ms: u64,
    pub body_deadline_ms: u64,
}

impl HttpLimits {
    pub fn default_v1() -> Self {
        Self {
            connect_timeout_ms: 5_000,
            read_timeout_ms: 10_000,
            write_timeout_ms: 10_000,
            head_deadline_ms: 10_000,
            body_deadline_ms: 60_000,
        }
    }

    pub fn validate(&self) -> ClientResult<()> {
        if self.connect_timeout_ms == 0
            || self.read_timeout_ms == 0
            || self.write_timeout_ms == 0
            || self.head_deadline_ms == 0
            || self.body_deadline_ms == 0
        {
            return Err(ClientError::InvalidInput { what: "http limits" });
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub location: Option<ClientLocation>,
    pub cache_root: PathBuf,
    pub cache: CacheBudgets,
    pub http: HttpLimits,
    pub token: Option<String>,
    pub http_keep_alive: bool,
    pub max_transfer_attempts: u32,
    pub blob_body_deadline_ms: u64,
}

impl ClientConfig {
    pub fn new(cache_root: impl Into<PathBuf>) -> Self {
        Self {
            location: None,
            cache_root: cache_root.into(),
            cache: CacheBudgets::default_v1(),
            http: HttpLimits::default_v1(),
            token: None,
            http_keep_alive: true,
            max_transfer_attempts: 4,
            blob_body_deadline_ms: 600_000,
        }
    }

    pub fn static_site(base_url: BaseUrl) -> Self {
        let mut config = Self::new(PathBuf::new());
        config.location = Some(ClientLocation::StaticSite(base_url));
        config
    }

    fn validate(&self) -> ClientResult<()> {
        self.cache.validate()?;
        self.http.validate()?;
        if self.max_transfer_attempts == 0 || self.blob_body_deadline_ms == 0 {
            return Err(ClientError::InvalidInput { what: "transfer attempts/deadline" });
        }
        if matches!(self.location, Some(ClientLocation::StaticSite(_))) && self.token.is_some() {
            return Err(ClientError::InvalidInput { what: "static site bearer token" });
        }
        Ok(())
    }
}

/// Blocking client calls are intentionally absent on a single-threaded web
/// target. This type keeps construction source-compatible and fails before
/// performing I/O; the static runtime arrives in the next feature lane.
pub struct AssetClient;

impl AssetClient {
    pub fn connect(
        config: ClientConfig,
        _endpoints: ApiEndpoints,
        _expected_server: Option<[u8; 16]>,
    ) -> ClientResult<Self> {
        config.validate()?;
        let mode = config
            .location
            .as_ref()
            .map(ClientLocation::mode)
            .unwrap_or(ClientMode::Native);
        let capability = match mode {
            ClientMode::Native => CAPABILITY_BLOCKING_API,
            ClientMode::StaticWeb => CAPABILITY_STATIC_SITE_SESSION,
        };
        Err(ClientError::Unavailable { capability, mode })
    }

    fn unavailable<T>(&self, capability: &'static str) -> ClientResult<T> {
        Err(ClientError::Unavailable {
            capability,
            mode: ClientMode::StaticWeb,
        })
    }

    pub fn resolve_alias(&self, _alias: &AssetAlias) -> ClientResult<crate::dto::AliasDto> {
        self.unavailable("resolve_alias")
    }

    pub fn alias_status(
        &self,
        _entries: &[(AssetAlias, Option<BlobId>)],
        _tags: &[String],
    ) -> ClientResult<Vec<AliasStatusDto>> {
        self.unavailable("alias_status")
    }

    pub fn catalog_search(
        &self,
        _query: &CatalogQuery,
        _cursor: Option<&PageCursor>,
    ) -> ClientResult<CatalogPage> {
        self.unavailable("catalog_search")
    }

    pub fn put_annotation(
        &self,
        _asset: &AssetId,
        _annotation: &AnnotationUpload,
    ) -> ClientResult<()> {
        self.unavailable("put_annotation")
    }

    pub fn fetch_asset_manifest(
        &mut self,
        _revision: &AssetRevisionId,
    ) -> ClientResult<AssetManifest> {
        self.unavailable("fetch_asset_manifest")
    }

    pub fn fetch_blob_bytes(
        &mut self,
        _blob: &BlobId,
        _expected_len: Option<u64>,
    ) -> ClientResult<Vec<u8>> {
        self.unavailable("fetch_blob_bytes")
    }
}
