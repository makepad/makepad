#![allow(dead_code)]
//! Library surface for the Asset Worker: the licensed pack compiler and the
//! media-inspection helpers the binary modes share.
//!
//! The networked coordinators stay in the binary. This crate exists so the
//! Asset UI Import page can compile a local Kenney (or later OSS) pack
//! through the same fail-closed path as `--import-pack`.

pub mod anim_icon;
pub mod ao_bake;
pub mod billboard_sheet;
pub mod vertex_skin;
pub mod classic_fetch;
pub mod classic_import;
pub mod iso9660;
pub mod tdm_zipsync;
pub mod doom3_import;
pub mod duke_import;
pub mod quake2_import;
pub mod quake3_import;
pub mod stateful_billboard;
pub mod world_preview;
pub mod world_place;
pub mod world_nav;
pub mod glb;
pub mod glb_nodes;
// Idempotent ai-content-library publication (one-shot + continuous). Public
// because the Asset UI runs the same watcher in-process against its own
// embedded Asset Server.
pub mod import;
pub mod watch;
pub mod coordinator;
// The wired generation kinds (job kind <-> fleet domain <-> catalog shape)
// and the profile advertisement built from a live fleet snapshot.
pub mod gen_kinds;
pub mod gen_profiles;
// The per-box claim fan-out + profile announcer both hosts run.
pub mod gen_service;
// Splash game folders -> `AssetKind::Game` assets (the sandbox's Games list).
pub mod games_import;
pub mod pack_import;
pub mod thumbs;
pub mod videothumb;
