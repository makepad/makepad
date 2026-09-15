//! Portable side-channel vocabulary and typed publishing refusal.

use crate::client::AssetClient;
use crate::error::{ClientError, ClientResult};
use crate::location::ClientMode;
use makepad_asset_data::{AssetId, AssetRevisionId, FileRole, MediaType};

#[derive(Clone, Debug)]
pub struct SideChannelFile {
    pub role: FileRole,
    pub media: MediaType,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SideChannelOutcome {
    Published { revision: AssetRevisionId },
    AlreadyPresent { revision: AssetRevisionId },
}

impl AssetClient {
    pub fn publish_side_channel_files(
        &mut self,
        _asset: &AssetId,
        _files: Vec<SideChannelFile>,
    ) -> ClientResult<SideChannelOutcome> {
        Err(ClientError::Unavailable {
            capability: "side_channel_publish",
            mode: ClientMode::StaticWeb,
        })
    }
}
