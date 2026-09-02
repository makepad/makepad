//! Portable background-runtime facade. Blocking worker execution is unavailable.

use crate::api::CatalogQuery;
use crate::client::{CatalogPage, PageCursor};
use crate::dto::AssetDetailDto;
use crate::error::{ClientError, ClientResult};
use crate::side_channels::{SideChannelFile, SideChannelOutcome};
use makepad_asset_data::{AssetId, AssetManifest, AssetRevisionId, BlobId};
use std::path::PathBuf;
use std::sync::Arc;

pub type RequestId = u64;

#[derive(Clone, Debug)]
pub enum ClientRequest {
    CatalogSearch { query: CatalogQuery, cursor: Option<PageCursor> },
    AssetDetail { id: AssetId },
    FetchAssetManifest { rev: AssetRevisionId },
    FetchBlob { blob: BlobId, expected_len: Option<u64>, pin: bool },
    PublishSideChannels { asset: AssetId, files: Arc<Vec<SideChannelFile>> },
    RetireAsset { id: AssetId },
}

#[derive(Clone, Debug)]
pub enum ClientOutput {
    CatalogPage(CatalogPage),
    AssetDetail(AssetDetailDto),
    AssetManifest(Box<AssetManifest>),
    Blob { blob: BlobId, path: PathBuf },
    SideChannels(SideChannelOutcome),
}

#[derive(Clone, Debug)]
pub enum ClientEvent {
    Started { id: RequestId },
    Progress { id: RequestId, bytes: u64, total: u64 },
    Done { id: RequestId, output: ClientOutput },
    Failed { id: RequestId, error: ClientError },
}

impl ClientEvent {
    pub fn id(&self) -> RequestId {
        match self {
            Self::Started { id }
            | Self::Progress { id, .. }
            | Self::Done { id, .. }
            | Self::Failed { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageEvent {
    pub id: RequestId,
    pub stage: crate::PublishStage,
}

#[derive(Clone, Debug, Default)]
pub enum ResourceState<T> {
    #[default]
    Idle,
    Loading { progress: Option<(u64, u64)> },
    Ready(T),
    Failed(ClientError),
}

#[derive(Clone, Debug, Default)]
pub struct ResourceSlot<T> {
    request: Option<RequestId>,
    pub state: ResourceState<T>,
}

impl<T> ResourceSlot<T> {
    pub fn begin(&mut self, id: RequestId) {
        self.request = Some(id);
        self.state = ResourceState::Loading { progress: None };
    }

    pub fn request(&self) -> Option<RequestId> {
        self.request
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Fast,
    Bulk,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubmitOptions {
    pub lane: Option<Lane>,
    pub lifo: bool,
}

impl SubmitOptions {
    pub fn fast() -> Self {
        Self { lane: Some(Lane::Fast), lifo: false }
    }

    pub fn bulk() -> Self {
        Self { lane: Some(Lane::Bulk), lifo: false }
    }

    pub fn newest_first() -> Self {
        Self { lane: None, lifo: true }
    }

    pub fn with_lane(mut self, lane: Lane) -> Self {
        self.lane = Some(lane);
        self
    }

    pub fn with_lifo(mut self, lifo: bool) -> Self {
        self.lifo = lifo;
        self
    }
}

pub struct ClientRuntime;

impl ClientRuntime {
    pub fn submit(&mut self, _request: ClientRequest) -> ClientResult<RequestId> {
        Err(ClientError::RuntimeDown)
    }

    pub fn submit_with(
        &mut self,
        _request: ClientRequest,
        _options: SubmitOptions,
    ) -> ClientResult<RequestId> {
        Err(ClientError::RuntimeDown)
    }

    pub fn cancel(&self, _id: RequestId) {}

    pub fn poll(&mut self) -> Vec<ClientEvent> {
        Vec::new()
    }

    pub fn poll_stages(&mut self) -> Vec<StageEvent> {
        Vec::new()
    }

    pub fn shutdown(self) {}
}
