//! REALTIME FRAME TWEENING as a library: two pictures and a fraction go
//! in, the in-between comes out of a texture.
//!
//! Extracted from the VJ, which remains the reference user — every pass,
//! every constant and every persisted mode code here is the one that
//! shipped on its decks. Two producers sit behind one seam:
//!
//! - **classical** ([`Mode::Flow`]): pyramidal block-matching optical flow
//!   expressed as ~41 chained fragment passes, then a cycle-consistency
//!   weighted two-sided gather. A few milliseconds of GPU per PAIR; every
//!   display frame inside that pair costs one textured quad.
//! - **neural** ([`Mode::Ai1`]/[`Mode::Ai2`]/[`Mode::Ai3`]): the in-house
//!   Practical-RIFE v4.26 ([`makepad_ai_rife`]) on a background worker,
//!   feeding the same warp pass. Intermediate-defined fields make the
//!   backward gather exact.
//!
//! Plus the two honest tiers with no fields at all: [`Mode::None`] holds
//! and swaps, [`Mode::Crossfade`] dissolves.
//!
//! # Shape
//!
//! [`FlowTweenView`] is a widget that renders NOTHING on screen — it owns
//! the offscreen pass chain and hands back [`FlowTweenView::output_texture`].
//! Feed it a pair ([`FlowTweenView::set_pair`] for NV12,
//! [`FlowTweenView::set_pair_rgb8`] for packed RGB) and a fraction
//! ([`FlowTweenView::set_t`]); draw it; sample the output.
//!
//! A host that just has a feed and wants fluid motion out of it can skip
//! all of that and drive [`player::FeedTweener`], which owns the clock, the
//! neural worker and the per-mode wiring.
//!
//! # Renderer laws this stack is built on
//!
//! 1. A draw shader's pipeline BAKES its colour-attachment format — the
//!    default BGRA8 into a float target silently draws NOTHING on Metal.
//!    Every float data pass declares `color_format: @Rgba16F`.
//! 2. An offscreen pass quad recorded inside a widget's turtle inherits the
//!    widget's on-screen clip and silently loses rows. Each stage opens its
//!    own `begin_root_turtle(pass_size)`.
//! 3. Sibling child passes do NOT run in creation order — each stage is
//!    parented to the next so the chain resolves back to front.

use makepad_widgets::*;

pub mod flow_tween;
pub mod frame;
pub mod mode;
pub mod pair_cache;
pub mod player;
pub mod selftest;

pub use flow_tween::{
    ai2_frame_plan, ai3_budget_depth, ai3_complete_depth, ai3_frame_plan, ai3_neural_frames,
    build_derive_ops, default_model_path, field_prefetch_budgets, rife_enabled, rife_proxy_dims,
    Ai2FramePlan, Ai2Pair, Ai3DepthChooser, Ai3FramePlan, DeriveOp, FieldPrefetch, FlowTweenView,
    FlowTweenViewRef, RifeField, RifeJob, RifeMidpoint, RifeProduct, RifeProductKind, RifeService,
    RifeSource, RifeSubdivision, AI3_BOOTSTRAP_SYNTH_SECS, AI3_MAX_DEPTH, AI3_MIN_DEPTH,
    FIELD_PREFETCH_OPS_PER_FRAME, LEVELS, SWEEPS,
};
pub use frame::{
    nv12_cut_score, nv12_proxy_rgb8, rgb8_cut_score, rgb8_proxy, tl_on, Frame, Pixels,
};
pub use mode::{ai_ceiling, modes, short_modes, tip, AiRateGate, Mode, RIFE_CAPACITY_FPS};
pub use pair_cache::{ClipGeneration, PairCache, PairKey};
pub use player::FeedTweener;

/// Register the tween widget and its seven draw shaders. Call this after
/// `makepad_widgets::script_mod` and before any UI that names
/// `FlowTweenView`.
pub fn script_mod(vm: &mut ScriptVm) {
    crate::flow_tween::script_mod(vm);
}
