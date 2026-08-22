//! FLOW-WARP SOURCE MATERIAL: the one implementation of the `mkfl` motion
//! payload, of a model-free way to measure it, and of the all-intra re-encode
//! that turns an ordinary clip into one a player can scratch at any rate.
//!
//! Two producers share it, and that is the point:
//!
//! - the asset-ai `video-enhance` backend, whose field comes from RIFE and
//!   whose upscale/interpolate stages are none of this crate's business — it
//!   uses the payload half ([`payload`]) so there is exactly ONE definition
//!   of the box the players read;
//! - the VJ's import converter, which may not load a model at all and so
//!   measures motion with the classical estimator here ([`estimate`]) before
//!   re-encoding through [`convert`].
//!
//! Both write bytes the VJ's `crate::flow::parse_mkfl` accepts, in the same
//! units, with the same playback contract. If that contract ever changes it
//! changes here, once.
//!
//! Without the default `convert` feature this crate is std-only: the format
//! and the estimator, no codec seam.

pub mod estimate;
pub mod payload;

#[cfg(feature = "convert")]
pub mod convert;

pub use estimate::{estimate_flow, flow_pair, FlowPair, FlowParams, FramePyramid};
pub use payload::{
    append_mkfl_box, encode_flow_payload, find_mkfl_box, mkfl_box_bytes, parse_flow_payload,
    quantize_flow_grid, quantize_flow_pair, PayloadHeader, HEADER_LEN, PAYLOAD_VERSION, PLANES,
};

#[cfg(feature = "convert")]
pub use convert::{convert_video, ConvertError, ConvertOptions, ConvertProgress, ConvertReport};
