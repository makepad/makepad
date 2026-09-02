//! Makepad Asset Server client + verified local cache (work package D).
//!
//! One reusable layer under both AI Content and the Sandbox: it discovers
//! servers on the LAN, verifies which server it is talking to, browses and
//! searches the catalog with server-bound pagination cursors, and turns exact
//! content identities into verified local bytes — resumably, atomically, and
//! within explicit budgets. UI hosts consume typed states and typed refusals;
//! nothing in this crate guesses, substitutes, or falls back.
//!
//! Trust model, end to end:
//!
//! - **Discovery is a hint** ([`discovery`]): a 36-byte fixed beacon that can
//!   only name ports/flags/capability bits; endpoints derive from the UDP
//!   sender address. Nothing is believed until HTTP health verifies the
//!   server identity and a credentialed probe succeeds
//!   ([`AssetClient::connect`]).
//! - **The network is hostile** ([`http`], [`json`], [`dto`]): bounded
//!   response heads, `Content-Length`-only framing, refused redirects and
//!   transfer-encodings, strict bounded JSON, fail-closed DTO parsing through
//!   the content contract's strict ID spellings.
//! - **Bytes are their digest** ([`cache`], [`client`], [`resolver`]):
//!   manifests must hash to the requested revision and decode canonically
//!   before they are cached; blobs stream hash-while-write into resumable
//!   partials (`Range`/`If-Range`) and commit atomically only on an exact
//!   digest match; resolves re-hash on read, so a returned path IS the
//!   content. Pinning and eviction budgets keep the cache bounded without
//!   ever evicting pinned content.
//! - **States are explicit** ([`runtime`]): background execution with typed
//!   `Idle / Loading / Ready / Failed` resource states and typed errors,
//!   across two worker lanes ([`Lane`]) so a multi-megabyte transfer can
//!   never park the thumbnail fetches queued behind it.
//!
//! The wire contract (route paths, beacon layout, token shape, budgets)
//! lives in [`wire`] — the single coordination surface with the server
//! process (`libs/asset/store`).

#[cfg(not(target_arch = "wasm32"))]
pub mod api;
#[cfg(not(target_arch = "wasm32"))]
pub mod cache;
#[cfg(not(target_arch = "wasm32"))]
pub mod client;
#[cfg(target_arch = "wasm32")]
#[path = "portable/client.rs"]
pub mod client;
#[cfg(not(target_arch = "wasm32"))]
pub mod discovery;
#[cfg(target_arch = "wasm32")]
#[path = "portable/discovery.rs"]
pub mod discovery;
pub mod dto;
pub mod error;
#[cfg(not(target_arch = "wasm32"))]
pub mod http;
// The module moved; this re-export keeps every dependent's `makepad_asset_client::json::Value` path compiling.
pub mod json { pub use makepad_strict_json::*; }
pub mod location;
pub mod transport;
pub mod cache_store;
#[cfg(not(target_arch = "wasm32"))]
pub mod side_channels;
#[cfg(not(target_arch = "wasm32"))]
pub mod publish;
#[cfg(not(target_arch = "wasm32"))]
pub mod resolver;
#[cfg(not(target_arch = "wasm32"))]
pub mod runtime;
#[cfg(not(target_arch = "wasm32"))]
pub mod session;
#[cfg(target_arch = "wasm32")]
#[path = "portable/session.rs"]
pub mod session;
#[cfg(not(target_arch = "wasm32"))]
pub mod subscriber;
pub mod util;
pub mod wire;

#[cfg(not(target_arch = "wasm32"))]
pub use api::{
    AnnotationUpload, Api, BatchFlow, BatchFrame, BatchItem, BlobHead, BlobRefAdmission, BlobRefRow, BlobRefsPage, CatalogQuery,
    ChatAttachment, ChatCreateRequest,
    ChatSendRequest, GcRequest, OperationAliasExpect, OperationCreateRequest,
    OperationFinalizeRequest, OperationInputRef, OperationOutputFile, OperationPublicationRef,
    PipelineStageSpec, SourceCollectionRegistered, default_stage_weight, stage_ref,
    DEFAULT_STAGE_WEIGHTS, MAX_LIST_LIMIT, MAX_SEARCH_LIMIT, NEUTRAL_STAGE_WEIGHT,
};
#[cfg(not(target_arch = "wasm32"))]
pub use cache::{CacheBudgets, CacheStats, ContentCache, PartialWriter};
#[cfg(not(target_arch = "wasm32"))]
pub use client::{
    AssetClient, AssetsPage, CatalogEventCursor, CatalogEventsPage, CatalogPage, ClientConfig,
    PageCursor, SourceCollectionsCursor, SourceCollectionsPage,
};
#[cfg(target_arch = "wasm32")]
pub use client::{AssetClient, CacheBudgets, CacheStats, ClientConfig, HttpLimits};
#[cfg(not(target_arch = "wasm32"))]
pub use discovery::{
    bind_reuse_udp, content_client_caps, Beacon, DiscoveredServer, DiscoveryListener, MAX_ENTRIES,
};
#[cfg(target_arch = "wasm32")]
pub use discovery::{content_client_caps, Beacon};
pub use dto::{
    ChatProviderLocality,
    AliasDto, AliasStatusDto, AnnotateBacklogDto, AnnotateSummaryDto, AnnotationDto,
    AssetDetailDto, AssetRow, CandidateDto, CandidateStateDto,
    CatalogEventDto,
    CatalogEventKind, CatalogFacet, CatalogHit, ClaimedJobDto, EventsPageDto, FacetKind,
    ModelPreviewDto, ModelPreviewPartDto, ModelPreviewRenameDto,
    GameAliasDto, GcPhaseDto,
    GcStatusDto, HealthDto, ImportEntryDto, RetireDto,
    ImportReportDto, ImportStatusDto, JobAttemptDto, JobDetailDto, JobId, JobProfileDto,
    ChatEventBodyDto, ChatEventDto, ChatEventsPageDto, ChatProviderDto, ChatProviderKind,
    JobProgressDto, JobResultDto, JobRowDto, JobStageDto, JobStageInput, JobStateDto,
    JobStatusDto, OperationEventDto,
    ChatProviderStateDto, ChatSessionDto, ChatSessionId, ChatSessionStateDto, ChatToolOutcomeDto,
    ChatTranscriptDto, ChatTranscriptRole, ChatTranscriptRowDto,
    OperationEventsPageDto, OperationId, OperationInputDto, OperationProgressDto, OperationStateDto,
    OperationStatusDto, OperationTypeDto, PipelineCancelDto, PipelineCreatedDto, PipelineDetailDto,
    PipelineId, PipelineRowDto, PipelineStageDto, PipelineStageJobDto, PipelineStateDto,
    PrincipalDto, ResolvedVariantMapDto,
    RoomClaimDto, RoomDto,
    SourceCollectionRowDto, SourceCollectionsPageDto, StageOnFailDto, aggregate_permille,
};
pub use error::{ClientError, ClientResult};
pub use location::{
    ApiEndpoints, BaseUrl, ClientLocation, ClientMode, CAPABILITY_BLOCKING_API,
    CAPABILITY_STATIC_SITE_SESSION,
};
pub use transport::{
    OwnedRequest, OwnedResponse, Transport, TransportCompletion, TransportError, TransportId,
    TransportMethod,
};
#[cfg(not(target_arch = "wasm32"))]
pub use transport::TcpHttpTransport;
#[cfg(any(target_arch = "wasm32", feature = "web"))]
pub use transport::PlatformHttpTransport;
pub use cache_store::{BlobContent, CacheStore, CacheStoreStats, MemoryCacheStore};
#[cfg(not(target_arch = "wasm32"))]
pub use cache_store::FsCacheStore;
#[cfg(not(target_arch = "wasm32"))]
pub use http::HttpLimits;
#[cfg(not(target_arch = "wasm32"))]
pub use publish::{
    PublishBundle, PublishBundleFile, PublishFile, PublishProvenance, PublishRequest,
    PublishRights, PublishStage, PublishStats, PublishThumbnail, Published, PublishedBundle,
    PublishedFile,
};
#[cfg(not(target_arch = "wasm32"))]
pub use session::{
    SessionConfig, SessionConnector, SessionHandles, SessionMsg, SessionStatus,
};
#[cfg(target_arch = "wasm32")]
pub use session::{
    CatalogSubscriberConfig, RuntimeConfig, SessionConfig, SessionConnector, SessionMsg,
    SessionStatus,
};
#[cfg(not(target_arch = "wasm32"))]
pub use resolver::{
    select_file, ClosureBudget, ResolvedFile, ResolvedThumbnail, TierPreference,
};
#[cfg(not(target_arch = "wasm32"))]
pub use runtime::{
    ClientEvent, ClientOutput, ClientRequest, ClientRuntime, Lane, RequestId, ResourceSlot,
    ResourceState, RuntimeConfig, StageEvent, SubmitOptions,
};
#[cfg(not(target_arch = "wasm32"))]
pub use subscriber::{
    CatalogSubscriber, CatalogSubscriberConfig, CatalogSubscriptionEvent,
};
