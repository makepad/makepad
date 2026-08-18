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
//!   `Idle / Loading / Ready / Failed` resource states and typed errors.
//!
//! The wire contract (route paths, beacon layout, token shape, budgets)
//! lives in [`wire`] — the single coordination surface with the server
//! process (`libs/asset/store`).

pub mod api;
pub mod cache;
pub mod client;
pub mod discovery;
pub mod dto;
pub mod error;
pub mod http;
pub mod json;
pub mod publish;
pub mod resolver;
pub mod runtime;
pub mod session;
pub mod subscriber;
pub mod util;
pub mod wire;

pub use api::{
    AnnotationUpload, Api, ApiEndpoints, BlobHead, CatalogQuery, ChatAttachment, ChatCreateRequest,
    ChatSendRequest, OperationAliasExpect, OperationCreateRequest, OperationFinalizeRequest,
    OperationInputRef, OperationOutputFile, OperationPublicationRef, SourceCollectionRegistered,
    MAX_LIST_LIMIT, MAX_SEARCH_LIMIT,
};
pub use cache::{CacheBudgets, CacheStats, ContentCache, PartialWriter};
pub use client::{
    AssetClient, AssetsPage, CatalogEventCursor, CatalogEventsPage, CatalogPage, ClientConfig,
    JobControl, PageCursor, SourceCollectionsCursor, SourceCollectionsPage,
};
pub use discovery::{
    content_client_caps, Beacon, DiscoveredServer, DiscoveryListener, MAX_ENTRIES,
};
pub use dto::{
    AliasDto, AssetDetailDto, AssetRow, CandidateDto, CandidateStateDto, CatalogEventDto,
    CatalogEventKind, CatalogHit, ClaimedJobDto, EventsPageDto, GameAliasDto, HealthDto, ImportEntryDto,
    ImportReportDto, ImportStatusDto, JobAttemptDto, JobDetailDto, JobId, JobProfileDto,
    ChatEventBodyDto, ChatEventDto, ChatEventsPageDto, ChatProviderDto, ChatProviderKind,
    JobProgressDto, JobResultDto, JobRowDto, JobStateDto, JobStatusDto, OperationEventDto,
    ChatProviderStateDto, ChatSessionDto, ChatSessionId, ChatSessionStateDto, ChatToolOutcomeDto,
    OperationEventsPageDto, OperationId, OperationInputDto, OperationProgressDto, OperationStateDto,
    OperationStatusDto, OperationTypeDto, PrincipalDto, ResolvedVariantMapDto,
    SourceCollectionRowDto, SourceCollectionsPageDto,
};
pub use error::{ClientError, ClientResult};
pub use http::HttpLimits;
pub use publish::{
    PublishBundle, PublishBundleFile, PublishFile, PublishProvenance, PublishRequest,
    PublishRights, PublishStage, PublishStats, PublishThumbnail, Published, PublishedBundle,
    PublishedFile,
};
pub use session::{
    SessionConfig, SessionConnector, SessionHandles, SessionMsg, SessionStatus,
};
pub use resolver::{
    select_file, ClosureBudget, ResolvedFile, ResolvedThumbnail, TierPreference,
};
pub use runtime::{
    ClientEvent, ClientOutput, ClientRequest, ClientRuntime, RequestId, ResourceSlot,
    ResourceState, StageEvent,
};
pub use subscriber::{
    CatalogSubscriber, CatalogSubscriberConfig, CatalogSubscriptionEvent,
};
