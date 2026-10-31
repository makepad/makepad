//! Where a score's samples and singers come from: the library in Stage,
//! a game's catalog, a test.

use crate::model::AssetRef;
use makepad_score_player::Sample;
use std::sync::Arc;

pub trait Samples {
    fn sample(&self, asset: &AssetRef) -> Option<Arc<Sample>>;

    /// A library singer (a voice profile asset); none by default.
    fn singer(&self, _asset: &AssetRef) -> Option<Arc<makepad_score_player::sung::VoiceProfile>> {
        None
    }

    /// A library file a native instrument loads (a SoundFont's bytes);
    /// none by default.
    fn file(&self, _asset: &AssetRef) -> Option<Arc<[u8]>> {
        None
    }
}

/// No samples: samplers are silent.
pub struct NoSamples;

impl Samples for NoSamples {
    fn sample(&self, _asset: &AssetRef) -> Option<Arc<Sample>> {
        None
    }
}
