#![allow(dead_code)]
//! Library surface for the Asset Worker: the licensed pack compiler and the
//! media-inspection helpers the binary modes share.
//!
//! The networked coordinators stay in the binary. This crate exists so the
//! Asset UI Import page can compile a local Kenney (or later OSS) pack
//! through the same fail-closed path as `--import-pack`.

pub mod anim_icon;
pub mod ao_bake;
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
pub mod glb;
pub mod pack_import;
pub mod thumbs;
pub mod videothumb;
