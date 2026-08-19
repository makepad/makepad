#![allow(dead_code)]
//! makepad-app-asset-ui — catalog, licensed pack import, viewers, and
//! fleet generation over the Asset Server. The GPU fleet still lives in
//! `makepad-asset-ai`; this binary is the Asset UI:
//!
//! - FLEET: GPU boxes announce themselves on the LAN UDP beacon; the app
//!   joins whatever is live. Capabilities come from /health + /models, jobs
//!   are routed by the model-affinity scheduler in `makepad_asset_ai::fleet`
//!   — observable (per-stage "affinity: loaded") and overridable (pin a
//!   model and/or a box from the dropdowns).
//! - PIPELINE: one-click preset chains (prompt → expand → image → mesh,
//!   image → video, ...) with live per-stage status (/job stage + percent +
//!   elapsed), an app-side run QUEUE (cancel / move-up / retry; service-side
//!   queued jobs cancel via POST /job/<id>/cancel), and cross-box artifact
//!   relay as `input_b64`.
//! - VIEWERS: one per content type — text, image (png), audio (wav play +
//!   waveform), video (mp4, hardware decode + soundtrack), mesh (GLB orbit
//!   viewer), splat (ViewSplat, .ply). The viewer is selection-driven: it
//!   always shows the selected History item (a fresh artifact selects
//!   itself), with a kind badge + prompt caption up top.
//! - SURFACES: the right pane flips between CREATE (viewer + History strip),
//!   CHAT (local Fleet Qwen — tool calls image/video/audio/speech/music/mesh/world/character.generate, defaults, fleet
//!   introspection, plus catalog ops when the Asset Server is up),
//!   LIBRARY (searchable Local/Server asset browser with kind/category/tag
//!   filters, thumbnail grid and a revision/provenance/publish detail rail),
//!   IMPORT (hardcoded OSS pack modules — Kenney first, local-folder only),
//!   RUNS + WORKERS (local pipeline + LAN fleet, cancellable),
//!   and ADMIN + AUDIT. The left generator column
//!   stays on all of them. Server
//!   data is never fabricated: every server view renders the honest
//!   disconnected/loading/empty state from the shared Asset Server session;
//!   no server rows are synthesized locally.
//!
//! Headless drive for tests/evidence:
//!   AI_CONTENT_AUTO="audio sfx" AI_CONTENT_PROMPT="sword clash" \
//!   AI_CONTENT_QUEUE="speech;image" AI_CONTENT_CAPTURE=/tmp/shot.png \
//!   AI_CONTENT_CAPTURE_AT_S=5 AI_CONTENT_SURFACE=import AI_CONTENT_EXIT=1 \
//!   AI_CONTENT_SAMPLE=mesh AI_CONTENT_SAMPLE_MESH=x.glb AI_CONTENT_DARK=1 \
//!   cargo run -p makepad-app-asset-ui --release

pub use makepad_widgets;

mod artifact_io;
mod asset_store_state;
mod audio;
mod billboard_view;
mod chat;
mod enhance_meta;
mod fast_presets;
mod fleet_poll;
mod http;
mod import;
mod import_classic;
mod library;
mod mesh_view;
mod pipeline;
mod scheduler;
mod store_views;
mod thumbnail_renderer;
mod video_player;

use crate::artifact_io::{
    ArtifactIo, IoDone, IoPurpose, IoRequest, PendingOpen, PreviewPixels, ViewerContent,
    ViewerOpenGate,
};
use crate::fleet_poll::FleetPoll;
use crate::import::{ImportJob, ImportPage, ImportQueue};
use crate::import_classic::ClassicImportPage;
use crate::library::{collect_tag_stats, Library, TagStat, ThumbnailBackfillJob};
use crate::billboard_view::BillboardView;
use crate::mesh_view::MeshView;
use crate::thumbnail_renderer::ThumbnailRenderer;
use crate::asset_store_state::{
    server_kind_label, session_config_from_env, AssetStoreState, LocalLibraryFilters,
    Remote, SERVER_KINDS,
};
use crate::chat::{ChatBridge, ChatData, ChatJob, ChatRole, FleetView};
use crate::fast_presets::{SavedFastPreset, MAX_FAST_PRESETS};
use crate::pipeline::{
    consumer_only_domain, format_clock, format_music_duration, seed_replaces_prefix, stage_display_name, CandidateSetState, GenParams,
    Pipeline, PipelineEvent, StageState,
    IMAGE_SIZES, IMAGE_STEPS, MESH_FACE_COUNTS, MESH_TEXTURE_SIZES, MUSIC_DEFAULT_SECONDS, MUSIC_LENGTHS, PRESETS,
    VIDEO_LENGTHS, VIDEO_SIZES,
};
use crate::scheduler::{plan_run, DispatchPlan, EndpointLoad, MAX_ACTIVE_RUNS};
use crate::store_views::{
    admin_rows, candidate_cards, catalog_rows, format_bytes, runs_rows, short_digest, truncate,
    should_start_file_drag, upstream_preview_allowed, CandidateSheet, GalleryEntry, InputAsset,
    InputTray, LibraryGallery, LibraryGrid, PreviewWork, RowAction, StoreListPanel, StoreRow,
    TileDelete,
};
use crate::video_player::VideoPlayer;

use makepad_micro_serde::SerJson;
use makepad_widgets::*;
use makepad_xr::obj::ViewSplat;
use std::collections::VecDeque;
use std::path::PathBuf;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.ContentChat
    use mod.widgets.*

    // ---- design language ----------------------------------------------------
    // Near-black canvas, elevated neutral surfaces, hairline (1px, ~8% white)
    // borders, 6px visual rounding (sdf radius 3), a 4/8px spacing rhythm.
    // ONE accent (#x3d9bf0) reserved for: the primary action, progress fills
    // and the selected history item. Reds appear only on destructive hover.

    let PanelHeading = Label{
        margin: Inset{top: 10}
        draw_text +: {
            color: #x8a939d
            text_style: theme.font_bold{font_size: 8}
        }
    }
    let HintLabel = Label{
        draw_text +: {
            color: #x555b62
            text_style: theme.font_regular{font_size: 7.5}
        }
    }
    let MonoLabel = Label{
        width: Fill
        draw_text +: {
            color: #x99a2ac
            text_style: theme.font_regular{font_size: 8.5}
        }
    }
    let DimLabel = Label{
        width: Fill
        draw_text +: {
            color: #x6a7178
            text_style: theme.font_regular{font_size: 8}
        }
    }
    let BrightLabel = Label{
        width: Fill
        draw_text +: {
            color: #xdfe6ec
            text_style: theme.font_regular{font_size: 9}
        }
    }

    // The one filled-accent button in the app: Generate.
    let PrimaryButton = ButtonFlat{
        margin: 0
        padding: Inset{left: 12 right: 12 top: 7 bottom: 7}
        draw_text +: {
            color: #xffffff
            color_hover: #xffffff
            color_down: #xd5e6f7
            color_focus: #xffffff
            text_style: theme.font_bold{font_size: 9.5}
        }
        draw_bg +: {
            border_radius: 3.0
            border_size: 1.0
            color: #x2f7fc9
            color_hover: #x3d9bf0
            color_down: #x2569aa
            color_focus: #x2f7fc9
            border_color: #x5fb1ff38
            border_color_hover: #x5fb1ff60
            border_color_down: #x5fb1ff38
            border_color_focus: #x5fb1ff60
        }
    }
    // Secondary chip: one-click chains, in-viewer utilities.
    let ChipButton = ButtonFlat{
        margin: 0
        padding: Inset{left: 8 right: 8 top: 4 bottom: 4}
        draw_text +: {
            color: #xaab3bd
            color_hover: #xe6ebf0
            color_down: #xffffff
            color_focus: #xbac3cd
            text_style: theme.font_regular{font_size: 8.5}
        }
        draw_bg +: {
            border_radius: 3.0
            border_size: 1.0
            color: #x1b1b1f
            color_hover: #x25252b
            color_down: #x2b2b32
            color_focus: #x1e1e23
            border_color: #xffffff14
            border_color_hover: #xffffff26
            border_color_down: #xffffff30
            border_color_focus: #xffffff1e
        }
    }
    // Quiet control: queue reorder, transport, sample loaders.
    let GhostButton = ChipButton{
        padding: Inset{left: 7 right: 7 top: 3 bottom: 3}
        draw_text +: {
            color: #x828a93
        }
        draw_bg +: {
            color: #x00000000
            color_hover: #xffffff10
            color_down: #xffffff1a
            color_focus: #x00000000
            border_color: #x00000000
            border_color_hover: #xffffff1e
            border_color_down: #xffffff28
            border_color_focus: #x00000000
        }
    }
    // Destructive: quiet until hovered, then a muted red tint.
    let DangerButton = GhostButton{
        draw_text +: {
            color_hover: #xff9d94
            color_down: #xffb4ab
        }
        draw_bg +: {
            color_hover: #x391d1d
            color_down: #x452020
            border_color_hover: #xff8a8030
            border_color_down: #xff8a8042
        }
    }

    let Card = RoundedView{
        width: Fill height: Fit
        flow: Down
        draw_bg +: {
            color: #x18181c
            border_color: #xffffff10
            border_size: 1.0
            border_radius: 3.0
        }
    }
    // progress/color_fill are uniforms (driven via set_uniform from Rust —
    // per-widget values split draw calls, so each bar keeps its own).
    let ProgressBar = SolidView{
        width: Fill height: 6
        draw_bg +: {
            progress: uniform(0.0)
            color_track: uniform(#x26262b)
            color_fill: uniform(#x3d9bf0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.0, 0.0, self.rect_size.x, self.rect_size.y, 1.5)
                sdf.fill(self.color_track)
                // Branchless fill: zero progress fades the fill out instead
                // of an `if` (branches on uniforms mis-evaluate here).
                let w = max(clamp(self.progress, 0.0, 1.0) * self.rect_size.x, 4.0)
                let vis = clamp(self.progress * 1000.0, 0.0, 1.0)
                sdf.box(0.0, 0.0, w, self.rect_size.y, 1.5)
                sdf.fill(vec4(self.color_fill.rgb, self.color_fill.a * vis))
                return sdf.result
            }
        }
    }
    let Divider = SolidView{
        width: Fill height: 1
        margin: Inset{top: 8 bottom: 2}
        draw_bg +: { color: #xffffff0d }
    }

    // One-click chains: a tiny group tag column + wrapping chips.
    let GroupTag = Label{
        width: 40
        draw_text +: {
            color: #x555b62
            text_style: theme.font_bold{font_size: 7}
        }
    }
    let ChainChips = View{
        width: Fill height: Fit
        flow: Flow.Right{wrap: true}
        spacing: 4
    }
    let ChainRow = View{
        width: Fill height: Fit
        flow: Right
        spacing: 4
        align: Align{y: 0.5}
    }

    // History cards: selection is per recycled card, so it is an animator-
    // applied instance value rather than a shader uniform shared by a draw
    // batch. Every draw explicitly toggles select.on/off.
    let GalleryCard = RoundedView{
        width: 102 height: 92
        flow: Down spacing: 3
        padding: 4
        // A cursor makes the whole card hit-testable (Views only emit
        // finger actions with a cursor or animator) — clicking anywhere on
        // the card opens the item, not just its label button.
        cursor: MouseCursor.Hand
        draw_bg +: {
            color: #x161619
            border_color: #xffffff10
            border_color_selected: #x3d9bf0
            selected: instance(0.0)
            border_size: 1.0
            border_radius: 3.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.0,
                    self.rect_size.y - self.border_size * 2.0,
                    self.border_radius
                )
                sdf.fill_keep(self.color)
                sdf.stroke(
                    self.border_color.mix(self.border_color_selected, self.selected),
                    self.border_size
                )
                return sdf.result
            }
        }
        animator: Animator{
            select: {
                default: @off
                off: AnimatorState{
                    from: {all: Play.Snap}
                    apply: {draw_bg: {selected: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Play.Snap}
                    apply: {draw_bg: {selected: 1.0}}
                }
            }
        }
    }
    // ImageFit.Smallest shrinks the Image's own walk to the aspect-fitted
    // size — it does NOT center the result in the slot. Each card wraps one
    // of these in a fixed-size aligning box; the box is what visibly
    // centers portrait/square/strip textures, with no stretch and no crop.
    let ThumbFitImage = Image{
        width: Fill
        height: Fill
        fit: ImageFit.Smallest
    }
    // One artifact of the selected run in the run tray: thumb + kind. Click
    // = open in the viewer AND pin as the next transform run's input.
    let RunChip = RoundedView{
        width: 54 height: 58
        flow: Down spacing: 2
        padding: 3
        align: Align{x: 0.5}
        cursor: MouseCursor.Hand
        draw_bg +: {
            color: #x161619
            border_color: #xffffff10
            border_color_selected: #x3d9bf0
            selected: instance(0.0)
            border_size: 1.0
            border_radius: 3.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.0,
                    self.rect_size.y - self.border_size * 2.0,
                    self.border_radius
                )
                sdf.fill_keep(self.color)
                sdf.stroke(
                    self.border_color.mix(self.border_color_selected, self.selected),
                    self.border_size
                )
                return sdf.result
            }
        }
        animator: Animator{
            select: {
                default: @off
                off: AnimatorState{
                    from: {all: Play.Snap}
                    apply: {draw_bg: {selected: 0.0}}
                }
                on: AnimatorState{
                    from: {all: Play.Snap}
                    apply: {draw_bg: {selected: 1.0}}
                }
            }
        }
        View{
            width: 46 height: 36
            align: Align{x: 0.5 y: 0.5}
            thumb := ThumbFitImage{}
        }
        kind := HintLabel{
            text: ""
            draw_text +: { text_style: theme.font_regular{font_size: 6.5} }
        }
    }
    // Doom/Quake sprites: point sample so authored texels stay crisp when
    // the card scales them up. Linear filtering turns them to mush.
    let SpriteFitImage = Image{
        width: Fill
        height: Fill
        fit: ImageFit.Smallest
        draw_bg +: {
            get_color_scale_pan: fn(scale: vec2, pan: vec2) {
                if self.image_dim_w > 0.0 {
                    let angle = self.rotation * 3.141592653589793 / 180.0
                    let cos_a = cos(-angle)
                    let sin_a = sin(-angle)
                    let c = (self.pos - vec2(0.5, 0.5)) * self.rect_size
                    let cr = vec2(c.x * cos_a - c.y * sin_a, c.x * sin_a + c.y * cos_a)
                    let iuv = cr / vec2(self.image_dim_w, self.image_dim_h) + vec2(0.5, 0.5)
                    let uv = iuv * scale + pan
                    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 {
                        return vec4(0.0, 0.0, 0.0, 0.0)
                    }
                    return self.image_texture.sample_nearest(uv)
                }
                let uv = self.pos * scale + pan
                return self.image_texture.sample_nearest(uv)
            }
        }
    }
    let ThumbButton = GhostButton{
        width: Fill
        padding: Inset{left: 2 right: 2 top: 2 bottom: 2}
        draw_text +: {
            text_style: theme.font_regular{font_size: 7.5}
        }
    }
    // Always-visible deletion affordance. Destructive red is still reserved
    // for hover/down, but the normal-state × is deliberately high contrast.
    let LibraryDeleteButton = DangerButton{
        width: 22 height: 22
        padding: 0
        draw_text +: {
            color: #xc6cfd8
            color_hover: #xffffff
            color_down: #xffffff
            text_style: theme.font_bold{font_size: 11}
        }
        draw_bg +: {
            color: #xffffff0b
            border_color: #xffffff14
            border_size: 1.0
        }
    }
    // Compact grab affordance. Claims and sweep-locks the pointer on
    // FingerDown so PortalList scroll cannot steal the outbound file drag.
    // A six-dot grip, not the word "DRAG" — the title keeps the row.
    mod.widgets.FileDragHandleBase = #(FileDragHandle::register_widget(vm))
    let FileDragHandle = mod.widgets.FileDragHandleBase{
        width: 18 height: 18
        cursor: MouseCursor.Grab
        draw_bg +: {
            color: #x20252a
            border_color: #x3d9bf055
            border_size: 1.0
            border_radius: 3.0
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(0.5, 0.5, self.rect_size.x - 1.0, self.rect_size.y - 1.0, 3.0)
                sdf.fill(#x20252a)
                let dot = #x8fcdf0
                sdf.circle(6.0, 5.0, 1.25)
                sdf.fill(dot)
                sdf.circle(12.0, 5.0, 1.25)
                sdf.fill(dot)
                sdf.circle(6.0, 9.0, 1.25)
                sdf.fill(dot)
                sdf.circle(12.0, 9.0, 1.25)
                sdf.fill(dot)
                sdf.circle(6.0, 13.0, 1.25)
                sdf.fill(dot)
                sdf.circle(12.0, 13.0, 1.25)
                sdf.fill(dot)
                return sdf.result
            }
        }
    }

    // History is a real horizontal PortalList rather than twelve hard-coded
    // cards. One compact tile per pipeline-run group, fronted by the run's
    // final artifact; the library cap is 64 records, and every reachable
    // tile scrolls by wheel/trackpad drag or the visible scrollbar.
    mod.widgets.LibraryGalleryBase = #(LibraryGallery::register_widget(vm))
    mod.widgets.LibraryGallery = set_type_default() do mod.widgets.LibraryGalleryBase{
        width: Fill
        height: 104
        list := PortalList{
            width: Fill
            height: Fill
            flow: Right
            spacing: 6
            scroll_bar: ScrollBar{}

            Item := CachedView{
                width: 102 height: 92
                card := GalleryCard{
                    View{
                        width: Fill height: 22 flow: Right spacing: 2
                        align: Align{y: 0.5}
                        title := ThumbButton{ text: "" }
                        file_drag := FileDragHandle{}
                        // The tile's ONE ×: removes the whole run group
                        // (every intermediate payload/thumbnail), or just
                        // this record when the tile is an ungrouped import.
                        delete := LibraryDeleteButton{ text: "×" }
                    }
                    // Aligning box centers the fitted image; ids!(card.thumb)
                    // resolves through the anonymous wrappers.
                    View{
                        width: 94 height: 58
                        flow: Overlay
                        View{
                            width: Fill height: Fill
                            align: Align{x: 0.5 y: 0.5}
                            thumb := ThumbFitImage{}
                        }
                        // Member-count badge over the thumbnail: the cue
                        // that this tile IS a multi-artifact run and its ×
                        // removes all of it. Hidden on single-record tiles.
                        View{
                            width: Fill height: Fill
                            align: Align{x: 0.03 y: 0.97}
                            run_count := RoundedView{
                                width: Fit height: Fit
                                padding: Inset{left: 4 right: 4 top: 1 bottom: 1}
                                draw_bg +: {
                                    color: #x000000b4
                                    border_radius: 2.0
                                }
                                run_count_label := Label{
                                    draw_text +: {
                                        color: #xaab3bd
                                        text_style: theme.font_bold{font_size: 6.5}
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Empty := CachedView{
                width: 220 height: 72
                align: Align{x: 0.5 y: 0.5}
                HintLabel{ text: "No resources yet — generate or import one." }
            }
        }
    }

    let CandidateCell = GalleryCard{
        width: Fill height: 144
        padding: 6 spacing: 4
        candidate_title := Label{
            draw_text +: {
                color: #xe6ebf0
                text_style: theme.font_bold{font_size: 8.5}
            }
        }
        View{
            width: Fill height: 78
            align: Align{x: 0.5 y: 0.5}
            candidate_thumb := ThumbFitImage{}
        }
        candidate_progress := ProgressBar{ height: 4 }
        candidate_status := Label{
            width: Fill
            draw_text +: {
                color: #xaab3bd
                text_style: theme.font_regular{font_size: 7}
            }
        }
        candidate_meta := Label{
            width: Fill
            draw_text +: {
                color: #x626a73
                text_style: theme.font_regular{font_size: 6.5}
            }
        }
    }

    mod.widgets.CandidateSheetBase = #(CandidateSheet::register_widget(vm))
    mod.widgets.CandidateSheet = set_type_default() do mod.widgets.CandidateSheetBase{
        width: Fill height: Fill
        list := PortalList{
            width: Fill height: Fill
            flow: Down
            spacing: 8
            scroll_bar: ScrollBar{}
            Row := View{
                width: Fill height: Fit
                flow: Right spacing: 8
                c1 := CandidateCell{} c2 := CandidateCell{}
                c3 := CandidateCell{} c4 := CandidateCell{}
            }
            Empty := CachedView{
                width: Fill height: 140
                align: Align{x: 0.5 y: 0.5}
                HintLabel{ text: "Waiting for admitted image GPUs…" }
            }
        }
    }

    // Library-surface grid card: title uses the full caption row; a
    // 18px grip is the only chrome (no kind badge — that ate the name).
    let GridCell = GalleryCard{
        width: 150 height: 126
        flow: Down spacing: 4
        padding: 5
        View{
            width: 140 height: 88
            align: Align{x: 0.5 y: 0.5}
            grid_thumb := ThumbFitImage{}
            grid_sprite := SpriteFitImage{ visible: false }
        }
        View{
            width: Fill height: Fit flow: Right spacing: 3
            align: Align{y: 0.0}
            grid_title := Label{
                width: Fill
                draw_text +: {
                    color: #xc6cfd8
                    text_style: theme.font_regular{font_size: 8}
                }
            }
            file_drag := FileDragHandle{}
        }
    }

    // Wrapping, virtualized thumbnail grid: PortalList rows of up to eight
    // card slots; the Rust side shows `columns(width)` of them per row.
    mod.widgets.LibraryGridBase = #(LibraryGrid::register_widget(vm))
    mod.widgets.LibraryGrid = set_type_default() do mod.widgets.LibraryGridBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill height: Fill
            flow: Down
            spacing: 8
            scroll_bar: ScrollBar{}
            Row := View{
                width: Fill height: Fit
                flow: Right spacing: 8
                c1 := GridCell{} c2 := GridCell{} c3 := GridCell{} c4 := GridCell{}
                c5 := GridCell{} c6 := GridCell{} c7 := GridCell{} c8 := GridCell{}
            }
            Empty := View{
                width: Fill height: 140
                flow: Down spacing: 4
                align: Align{x: 0.5 y: 0.5}
                HintLabel{ text: "Nothing here matches." }
                HintLabel{ text: "Clear the filters, or generate something from Create." }
            }
        }
    }

    // One active Library filter chip. Instances are shown/hidden from Rust.
    let FilterTagChip = RoundedView{
        visible: false
        width: Fit height: 22
        flow: Right
        spacing: 3
        padding: Inset{left: 8 right: 3 top: 0 bottom: 0}
        align: Align{y: 0.5}
        draw_bg +: {
            color: #x14283c
            border_color: #x3d9bf066
            border_size: 1.0
            border_radius: 11.0
        }
        chip_name := Label{
            width: Fit
            height: Fill
            align: Align{y: 0.5}
            draw_text +: {
                color: #x9ec4ea
                text_style: theme.font_bold{font_size: 8}
            }
        }
        chip_x := GhostButton{
            width: 16 height: Fill
            margin: 0
            padding: 0
            align: Align{x: 0.5 y: 0.5}
            text: "×"
            draw_text +: {
                color: #x8a939d
                color_hover: #xe6ebf0
                text_style: theme.font_bold{font_size: 10}
            }
        }
    }

    // Shared row renderer behind Runs+Workers, Admin and the server catalog.
    // Template ids match store_views.rs StoreRow variants one-to-one.
    mod.widgets.StoreListPanelBase = #(StoreListPanel::register_widget(vm))
    mod.widgets.StoreListPanel = set_type_default() do mod.widgets.StoreListPanelBase{
        width: Fill
        height: Fill
        list := PortalList{
            width: Fill height: Fill
            flow: Down
            spacing: 4
            scroll_bar: ScrollBar{}
            SectionR := View{
                width: Fill height: Fit
                padding: Inset{top: 10 bottom: 2}
                section_label := Label{
                    draw_text +: {
                        color: #x8a939d
                        text_style: theme.font_bold{font_size: 8}
                    }
                }
            }
            NoteR := View{
                width: Fill height: Fit
                padding: Inset{left: 2 top: 1 bottom: 1}
                note_label := Label{
                    width: Fill
                    draw_text +: {
                        color: #x6a7178
                        text_style: theme.font_regular{font_size: 8.5}
                    }
                }
            }
            StageR := Card{
                flow: Right spacing: 8
                padding: Inset{left: 10 right: 6 top: 6 bottom: 6}
                align: Align{y: 0.5}
                stage_title := Label{
                    width: 190
                    draw_text +: {
                        color: #xdfe6ec
                        text_style: theme.font_regular{font_size: 8.5}
                    }
                }
                stage_meta := Label{
                    width: Fill
                    draw_text +: {
                        color: #x8a939d
                        text_style: theme.font_regular{font_size: 8}
                    }
                }
                stage_bar := ProgressBar{ width: 140 height: 4 }
                stage_cancel := DangerButton{ text: "Stop" visible: false }
            }
            QueuedR := Card{
                flow: Right spacing: 4
                padding: Inset{left: 10 right: 4 top: 4 bottom: 4}
                align: Align{y: 0.5}
                queued_title := MonoLabel{}
                queued_cancel := DangerButton{ text: "×" }
            }
            WorkerR := Card{
                padding: 10 spacing: 3
                View{
                    width: Fill height: Fit flow: Right spacing: 6
                    align: Align{y: 0.5}
                    worker_dot := SolidView{
                        width: 8 height: 8
                        draw_bg +: {
                            online: uniform(0.0)
                            pixel: fn() {
                                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                sdf.circle(self.rect_size.x * 0.5, self.rect_size.y * 0.5, self.rect_size.x * 0.35)
                                sdf.fill(mix(#x3d4750, #x58c4a0, self.online))
                                return sdf.result
                            }
                        }
                    }
                    worker_title := Label{
                        width: Fill
                        draw_text +: {
                            color: #xdfe6ec
                            text_style: theme.font_bold{font_size: 9}
                        }
                    }
                    worker_state := Label{
                        draw_text +: {
                            color: #x8a939d
                            text_style: theme.font_regular{font_size: 8}
                        }
                    }
                }
                worker_meta := DimLabel{}
                worker_models := MonoLabel{}
            }
            RecordR := Card{
                flow: Right spacing: 8
                padding: Inset{left: 10 right: 10 top: 6 bottom: 6}
                align: Align{y: 0.5}
                record_title := BrightLabel{}
                record_meta := Label{
                    draw_text +: {
                        color: #x6a7178
                        text_style: theme.font_regular{font_size: 8}
                    }
                }
            }
            AssetR := View{
                width: Fill height: Fit
                asset_card := GalleryCard{
                    width: Fill height: Fit
                    flow: Down spacing: 3
                    padding: Inset{left: 10 right: 10 top: 7 bottom: 7}
                    asset_title := BrightLabel{}
                    asset_meta := DimLabel{}
                }
            }
            DiscR := RoundedView{
                width: Fill height: 110
                flow: Down spacing: 5
                align: Align{x: 0.5 y: 0.5}
                margin: Inset{top: 8}
                draw_bg +: {
                    color: #x141418
                    border_color: #xffffff10
                    border_size: 1.0
                    border_radius: 3.0
                }
                disc_title := Label{
                    draw_text +: {
                        color: #x8a939d
                        text_style: theme.font_bold{font_size: 10}
                    }
                }
                disc_detail := Label{
                    draw_text +: {
                        color: #x555b62
                        text_style: theme.font_regular{font_size: 8}
                    }
                }
            }
        }
    }

    // Scrolling DropDown2 popup: selected row stays under the field, list
    // clamps to the window, ▲/▼ arrows scroll. Used for every select in
    // this app (preset lists are long enough that DropDownFlat clips).
    let FieldCaption = HintLabel{
        width: 92
        height: Fit
        align: Align{y: 0.5}
    }
    let DropField = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: 6
        align: Align{y: 0.5}
    }

    let FieldDrop = DropDown2Flat{
        width: Fill
        margin: 0
        padding: Inset{left: 8 right: 24 top: 5 bottom: 5}
        item_height: 22.0
        arrow_height: 16.0
        popup_margin: 8.0
        draw_text +: {
            color: #xaab3bd
            color_hover: #xe6ebf0
            color_focus: #xc6cfd8
            color_down: #xe6ebf0
            text_style: theme.font_regular{font_size: 8.5}
        }
        draw_item_text +: {
            hover: instance(0.0)
            active: instance(0.0)
            color: #xc6cfd8
            color_hover: #xe6ebf0
            color_active: #xe6ebf0
            text_style: theme.font_regular{font_size: 8.5}
        }
        draw_bg +: {
            border_radius: 3.0
            border_size: 1.0
            color: #x1a1a1e
            color_hover: #x202025
            color_down: #x232328
            color_focus: #x1c1c20
            border_color: #xffffff14
            border_color_hover: #xffffff22
            border_color_down: #xffffff22
            border_color_focus: #x3d9bf055
            arrow_color: #x828a93
            arrow_color_hover: #xc6cfd8
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(
                    self.border_size,
                    self.border_size,
                    self.rect_size.x - self.border_size * 2.0,
                    self.rect_size.y - self.border_size * 2.0,
                    self.border_radius
                )
                let fill = self.color
                    .mix(self.color_focus, self.focus)
                    .mix(self.color_hover, self.hover)
                    .mix(self.color_down, self.down * self.hover)
                sdf.fill_keep(fill)
                let stroke = self.border_color
                    .mix(self.border_color_focus, self.focus)
                    .mix(self.border_color_hover, self.hover)
                    .mix(self.border_color_down, self.down * self.hover)
                sdf.stroke(stroke, self.border_size)
                let c = vec2(self.rect_size.x - 12.0, self.rect_size.y * 0.5)
                let sz = 2.5
                sdf.move_to(c.x - sz, c.y - sz * 0.5)
                sdf.line_to(c.x + sz, c.y - sz * 0.5)
                sdf.line_to(c.x, c.y + sz)
                sdf.close_path()
                sdf.fill(self.arrow_color.mix(self.arrow_color_hover, self.hover))
                return sdf.result
            }
        }
        draw_popup_bg +: {
            border_radius: 3.0
            border_size: 1.0
            color: #x1a1a1e
            border_color: #xffffff22
        }
        draw_item +: {
            hover: instance(0.0)
            active: instance(0.0)
            color: #x00000000
            color_hover: #x2a2a32
            color_active: #x243044
        }
        draw_scroll_arrow +: {
            up: instance(0.0)
            enabled: instance(1.0)
            color: #xc6cfd8
            color_disabled: #x4a5158
        }
    }
    let FieldDrop2 = FieldDrop{}

    let FilterInput = TextInputFlat{
        width: Fill height: 28
        margin: 0
        padding: Inset{left: 8 right: 8 top: 5 bottom: 5}
        draw_text +: {
            color: #xc6cfd8
            color_hover: #xe6ebf0
            color_focus: #xe6ebf0
            color_down: #xc6cfd8
            color_empty: #x5a616a
            color_empty_hover: #x6a7178
            color_empty_focus: #x6a7178
            text_style: theme.font_regular{font_size: 8}
        }
        draw_bg +: {
            border_radius: 3.0
            border_size: 1.0
            color: #x161619
            color_hover: #x18181c
            color_focus: #x1a1a1e
            color_down: #x1a1a1e
            color_empty: #x161619
            border_color: #xffffff14
            border_color_hover: #xffffff20
            border_color_focus: #x3d9bf066
            border_color_down: #xffffff20
            border_color_empty: #xffffff14
        }
    }

    // Scroll areas on dark surfaces: hairline-quiet scrollbar handle.
    let QuietScrollY = ScrollYView{
        scroll_bars +: {
            scroll_bar_y +: {
                draw_bg +: {
                    color: #xffffff1e
                    color_hover: #xffffff38
                    color_drag: #xffffff50
                }
            }
        }
    }

    // Secondary Asset Store surfaces share the generator's dark visual
    // language, but stay one quiet navigation tier below Create.
    let SurfaceTab = GhostButton{
        padding: Inset{left: 10 right: 10 top: 5 bottom: 5}
        draw_text +: { text_style: theme.font_bold{font_size: 8} }
    }
    let SurfaceTitle = Label{
        width: Fill
        draw_text +: {
            color: #xe6ebf0
            text_style: theme.font_bold{font_size: 13}
        }
    }
    let StorePanel = RoundedView{
        width: Fill height: Fill
        flow: Down spacing: 8
        padding: 12
        draw_bg +: {
            color: #x141418
            border_color: #xffffff10
            border_size: 1.0
            border_radius: 3.0
        }
    }
    let StoreEmpty = View{
        width: Fill height: Fill
        flow: Down spacing: 8
        align: Align{x: 0.5 y: 0.5}
        Label{
            text: "No server data"
            draw_text +: {
                color: #x8a939d
                text_style: theme.font_bold{font_size: 11}
            }
        }
        Label{
            text: "Disconnected — connect a real Asset Store transport to load this panel."
            draw_text +: {
                color: #x555b62
                text_style: theme.font_regular{font_size: 8.5}
            }
        }
    }
    let StoreSection = RoundedView{
        width: Fill height: Fit
        flow: Down spacing: 6
        padding: 10
        draw_bg +: {
            color: #x18181c
            border_color: #xffffff10
            border_size: 1.0
            border_radius: 3.0
        }
    }
    let ImportRow = RoundedView{
        width: Fill height: Fit
        flow: Right spacing: 8
        padding: Inset{left: 8 right: 8 top: 5 bottom: 5}
        align: Align{y: 0.5}
        draw_bg +: {
            color: #x18181c
            border_color: #xffffff18
            border_size: 1.0
            border_radius: 4.0
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1560, 980)
                window.title: "Asset UI"
                body +: {
                    flow: Right
                    show_bg: true
                    // Plain-View DrawQuad ignores `color` (its pixel returns
                    // transparent) — declare the instance + pixel to paint.
                    draw_bg +: {
                        color: instance(#x0a0a0b)
                        pixel: fn() {
                            return Pal.premul(self.color)
                        }
                    }

                    left_panel := SolidView{
                        width: 430
                        height: Fill
                        flow: Down
                        draw_bg +: { color: #x121215 }

                        // The whole panel scrolls: on a short window nothing
                        // (Fleet included) falls off the bottom — it scrolls
                        // into reach instead.
                        left_scroll := QuietScrollY{
                        width: Fill
                        height: Fill
                        flow: Down
                        padding: Inset{left: 14 right: 14 top: 12 bottom: 10}
                        spacing: 6

                        title_row := View{
                            width: Fill height: Fit flow: Right
                            align: Align{y: 0.5}
                            spacing: 10
                            H2{ text: "Asset UI" draw_text +: { color: #xe6ebf0 } }
                            View{ width: Fill height: Fit }
                            spinner := LoadingSpinner{ width: 22 height: 22 visible: false }
                        }

                        PanelHeading{ text: "Saved" margin: Inset{top: 4} }
                        saved_presets := View{
                            width: Fill height: Fit
                            flow: Flow.Right{wrap: true}
                            spacing: 4
                            fp0 := View{ width: Fit height: Fit flow: Right spacing: 2 visible: false
                                fp0_go := ChipButton{ text: "" }
                                fp0_del := GhostButton{ text: "×" }
                            }
                            fp1 := View{ width: Fit height: Fit flow: Right spacing: 2 visible: false
                                fp1_go := ChipButton{ text: "" }
                                fp1_del := GhostButton{ text: "×" }
                            }
                            fp2 := View{ width: Fit height: Fit flow: Right spacing: 2 visible: false
                                fp2_go := ChipButton{ text: "" }
                                fp2_del := GhostButton{ text: "×" }
                            }
                            fp3 := View{ width: Fit height: Fit flow: Right spacing: 2 visible: false
                                fp3_go := ChipButton{ text: "" }
                                fp3_del := GhostButton{ text: "×" }
                            }
                            fp4 := View{ width: Fit height: Fit flow: Right spacing: 2 visible: false
                                fp4_go := ChipButton{ text: "" }
                                fp4_del := GhostButton{ text: "×" }
                            }
                            fp5 := View{ width: Fit height: Fit flow: Right spacing: 2 visible: false
                                fp5_go := ChipButton{ text: "" }
                                fp5_del := GhostButton{ text: "×" }
                            }
                            fp6 := View{ width: Fit height: Fit flow: Right spacing: 2 visible: false
                                fp6_go := ChipButton{ text: "" }
                                fp6_del := GhostButton{ text: "×" }
                            }
                            fp7 := View{ width: Fit height: Fit flow: Right spacing: 2 visible: false
                                fp7_go := ChipButton{ text: "" }
                                fp7_del := GhostButton{ text: "×" }
                            }
                        }
                        save_preset_row := View{
                            width: Fill height: Fit flow: Right spacing: 6
                            align: Align{y: 0.5}
                            preset_name_input := TextInputFlat{
                                width: Fill
                                height: 28
                                empty_text: "preset name"
                            }
                            save_preset_btn := GhostButton{ text: "Save preset" }
                        }

                        View{
                            width: Fill height: Fit flow: Right
                            align: Align{y: 1.0}
                            PanelHeading{ text: "Prompt" }
                            View{ width: Fill height: Fit }
                            HintLabel{ text: "Enter runs · Shift+Enter = new line" }
                        }
                        prompt_input := TextInputFlat{
                            width: Fill
                            height: 84
                            margin: 0
                            padding: 8
                            is_multiline: true
                            submit_on_enter: true
                            empty_text: "Describe what to make — e.g. a weathered fishing trawler at dawn, misty harbor"
                            draw_text +: {
                                text_style: theme.font_regular{font_size: 9}
                                color: #xdfe6ec
                                color_hover: #xe6ebf0
                                color_focus: #xe6ebf0
                                color_down: #xdfe6ec
                                color_empty: #x5a616a
                                color_empty_hover: #x6a7178
                                color_empty_focus: #x6a7178
                            }
                            draw_bg +: {
                                border_radius: 3.0
                                border_size: 1.0
                                color: #x161619
                                color_hover: #x18181c
                                color_focus: #x1a1a1e
                                color_down: #x1a1a1e
                                color_empty: #x161619
                                border_color: #xffffff14
                                border_color_hover: #xffffff20
                                border_color_focus: #x3d9bf066
                                border_color_down: #xffffff20
                                border_color_empty: #xffffff14
                            }
                        }

                        PanelHeading{ text: "Pipeline & routing" }
                        DropField{
                            FieldCaption{ text: "Type" }
                            preset_drop := FieldDrop{}
                        }
                        md_text_row := DropField{ visible: false FieldCaption{ text: "Text model" } md_text := FieldDrop{} }
                        md_image_row := DropField{ visible: false FieldCaption{ text: "Image model" } md_image := FieldDrop{} }
                        md_audio_row := DropField{ visible: false FieldCaption{ text: "Audio model" } md_audio := FieldDrop{} }
                        md_speech_row := DropField{ visible: false FieldCaption{ text: "Speech model" } md_speech := FieldDrop{} }
                        md_music_row := DropField{ visible: false FieldCaption{ text: "Music model" } md_music := FieldDrop{} }
                        md_video_row := DropField{ visible: false FieldCaption{ text: "Video model" } md_video := FieldDrop{} }
                        md_mesh_row := DropField{ visible: false FieldCaption{ text: "Mesh model" } md_mesh := FieldDrop{} }
                        md_matte_row := DropField{ visible: false FieldCaption{ text: "Matte model" } md_matte := FieldDrop{} }
                        md_depth_row := DropField{ visible: false FieldCaption{ text: "Depth model" } md_depth := FieldDrop{} }
                        md_segment_row := DropField{ visible: false FieldCaption{ text: "Segment model" } md_segment := FieldDrop{} }
                        md_paint_row := DropField{ visible: false FieldCaption{ text: "Paint model" } md_paint := FieldDrop{} }
                        md_world_row := DropField{ visible: false FieldCaption{ text: "World model" } md_world := FieldDrop{} }
                        md_rig_row := DropField{ visible: false FieldCaption{ text: "Rig model" } md_rig := FieldDrop{} }
                        md_motion_row := DropField{ visible: false FieldCaption{ text: "Motion model" } md_motion := FieldDrop{} }
                        DropField{
                            FieldCaption{ text: "Box" }
                            box_drop := FieldDrop{}
                        }
                        speech_params_row := DropField{
                            visible: false
                            FieldCaption{ text: "Speech voice" }
                            voice_drop := FieldDrop{}
                        }
                        image_size_row := DropField{
                            visible: false
                            FieldCaption{ text: "Image size" }
                            size_drop := FieldDrop{}
                        }
                        image_steps_row := DropField{
                            visible: false
                            FieldCaption{ text: "Image steps" }
                            steps_drop := FieldDrop{}
                        }
                        mesh_params_row := DropField{
                            visible: false
                            FieldCaption{ text: "Mesh texture" }
                            texture_size_drop := FieldDrop{}
                        }
                        mesh_faces_row := DropField{
                            visible: false
                            FieldCaption{ text: "Mesh faces" }
                            mesh_faces_drop := FieldDrop{}
                        }
                        // TRELLIS bakes its own colors; mesh-only chains keep
                        // them, PBR-paint chains skip the tex flow unless asked.
                        // Rig/motion chains: leave empty for the playable
                        // idle/walk/jump/run/dance set (the backend's own body-
                        // action prompts); type a motion to get ONE prompted
                        // take instead ("A person dances the robot").
                        motion_prompt_row := DropField{
                            visible: false
                            FieldCaption{ text: "Motion prompt" }
                            motion_prompt_input := TextInputFlat{
                                width: Fill
                                height: 28
                                margin: 0
                                padding: Inset{left: 8 right: 8 top: 5 bottom: 5}
                                empty_text: "empty = playable set · e.g. A person dances the robot"
                                draw_text +: {
                                    text_style: theme.font_regular{font_size: 8.5}
                                    color: #xdfe6ec
                                    color_hover: #xe6ebf0
                                    color_focus: #xe6ebf0
                                    color_down: #xdfe6ec
                                    color_empty: #x5a616a
                                    color_empty_hover: #x6a7178
                                    color_empty_focus: #x6a7178
                                }
                            }
                        }
                        mesh_colors_row := DropField{
                            visible: false
                            FieldCaption{ text: "TRELLIS colors" }
                            trellis_colors_toggle := CheckBox{
                                text: "keep on mesh stage before PBR paint"
                                active: false
                                padding: Inset{left: 4 right: 4 top: 1 bottom: 1}
                                draw_text +: {
                                    color: #x828a93
                                    text_style: theme.font_regular{font_size: 8.5}
                                }
                            }
                        }
                        vid_size_row := DropField{
                            visible: false
                            FieldCaption{ text: "Video size" }
                            vid_size_drop := FieldDrop{}
                        }
                        vid_len_row := DropField{
                            visible: false
                            FieldCaption{ text: "Video length" }
                            vid_len_drop := FieldDrop{}
                        }
                        music_params_row := DropField{
                            visible: false
                            FieldCaption{ text: "Music length" }
                            music_len_drop := FieldDrop{}
                        }

                        // Persistent selected-input chip: the managed asset
                        // the next transform run consumes. Populated by an
                        // explicit click on a History tile / Library card;
                        // its × unpins without deleting anything.
                        input_tray := Card{
                            visible: false
                            width: Fill height: Fit
                            flow: Right spacing: 8
                            padding: 6
                            align: Align{y: 0.5}
                            View{
                                width: 44 height: 33
                                align: Align{x: 0.5 y: 0.5}
                                input_chip_thumb := ThumbFitImage{}
                            }
                            View{
                                width: Fill height: Fit flow: Down spacing: 2
                                input_chip_kind := HintLabel{ text: "" }
                                input_chip_title := BrightLabel{ text: "" }
                            }
                            input_clear := LibraryDeleteButton{ text: "×" }
                        }

                        // The selected run, spread out: every artifact the
                        // pipeline produced (source image, matte, mesh, paint,
                        // rig, …) as clickable chips. History keeps ONE tile
                        // per run; this is where its members are reachable —
                        // click a chip to view it, and it becomes the pinned
                        // input, so a "mesh only" preset re-meshes exactly
                        // that image.
                        run_tray := Card{
                            visible: false
                            width: Fill height: Fit
                            flow: Down spacing: 4
                            padding: 6
                            run_tray_title := HintLabel{ text: "" }
                            View{
                                width: Fill height: Fit
                                flow: Flow.Right{wrap: true} spacing: 4 wrap_spacing: 4
                                run_chip0 := RunChip{ visible: false }
                                run_chip1 := RunChip{ visible: false }
                                run_chip2 := RunChip{ visible: false }
                                run_chip3 := RunChip{ visible: false }
                                run_chip4 := RunChip{ visible: false }
                                run_chip5 := RunChip{ visible: false }
                                run_chip6 := RunChip{ visible: false }
                                run_chip7 := RunChip{ visible: false }
                            }
                        }


                        action_row := View{
                            width: Fill height: Fit flow: Right spacing: 6
                            margin: Inset{top: 4}
                            generate_btn := PrimaryButton{ text: "Generate" width: Fill }
                            pull_btn := GhostButton{ text: "Pull model" }
                            retry_btn := ChipButton{ text: "Retry last" visible: false }
                        }
                        // One run per IDLE capable box (not up, not busy with
                        // another job, not already holding one of our stages),
                        // each its own History item — a quick spread of
                        // variations across the fleet. Off = the normal single
                        // run placed by affinity.
                        parallel_toggle := CheckBox{
                            text: "all free boxes in parallel"
                            active: false
                            padding: Inset{left: 4 right: 4 top: 1 bottom: 1}
                            draw_text +: {
                                color: #x828a93
                                text_style: theme.font_regular{font_size: 8.5}
                            }
                        }

                        PanelHeading{ text: "Now running" }
                        now_card := Card{
                            padding: 8
                            spacing: 6
                            now_top := View{
                                width: Fill height: Fit flow: Right spacing: 4
                                align: Align{y: 0.5}
                                now_head := BrightLabel{ text: "Idle — nothing running" }
                                cancel_btn := DangerButton{ text: "Stop" visible: false }
                            }
                            now_bar := ProgressBar{}
                            now_detail := DimLabel{ text: "" }
                        }

                        PanelHeading{ text: "Up next" }
                        queue_panel := View{
                            width: Fill height: Fit flow: Down spacing: 3
                            q1_row := Card{ flow: Right padding: Inset{left: 8 right: 4 top: 3 bottom: 3} spacing: 4 align: Align{y: 0.5} visible: false
                                q1_label := MonoLabel{} q1_up := GhostButton{ text: "↑" } q1_cancel := DangerButton{ text: "×" } }
                            q2_row := Card{ flow: Right padding: Inset{left: 8 right: 4 top: 3 bottom: 3} spacing: 4 align: Align{y: 0.5} visible: false
                                q2_label := MonoLabel{} q2_up := GhostButton{ text: "↑" } q2_cancel := DangerButton{ text: "×" } }
                            q3_row := Card{ flow: Right padding: Inset{left: 8 right: 4 top: 3 bottom: 3} spacing: 4 align: Align{y: 0.5} visible: false
                                q3_label := MonoLabel{} q3_up := GhostButton{ text: "↑" } q3_cancel := DangerButton{ text: "×" } }
                            q4_row := Card{ flow: Right padding: Inset{left: 8 right: 4 top: 3 bottom: 3} spacing: 4 align: Align{y: 0.5} visible: false
                                q4_label := MonoLabel{} q4_up := GhostButton{ text: "↑" } q4_cancel := DangerButton{ text: "×" } }
                            q5_row := Card{ flow: Right padding: Inset{left: 8 right: 4 top: 3 bottom: 3} spacing: 4 align: Align{y: 0.5} visible: false
                                q5_label := MonoLabel{} q5_up := GhostButton{ text: "↑" } q5_cancel := DangerButton{ text: "×" } }
                            q6_row := Card{ flow: Right padding: Inset{left: 8 right: 4 top: 3 bottom: 3} spacing: 4 align: Align{y: 0.5} visible: false
                                q6_label := MonoLabel{} q6_up := GhostButton{ text: "↑" } q6_cancel := DangerButton{ text: "×" } }
                        }

                        PanelHeading{ text: "Stage details" }
                        stage_bars := View{
                            width: Fill height: Fit flow: Down spacing: 4
                            s1_row := View{ width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5} visible: false
                                s1_name := DimLabel{ width: 220 } s1_bar := ProgressBar{ height: 4 } }
                            s2_row := View{ width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5} visible: false
                                s2_name := DimLabel{ width: 220 } s2_bar := ProgressBar{ height: 4 } }
                            s3_row := View{ width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5} visible: false
                                s3_name := DimLabel{ width: 220 } s3_bar := ProgressBar{ height: 4 } }
                            s4_row := View{ width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5} visible: false
                                s4_name := DimLabel{ width: 220 } s4_bar := ProgressBar{ height: 4 } }
                            s5_row := View{ width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5} visible: false
                                s5_name := DimLabel{ width: 220 } s5_bar := ProgressBar{ height: 4 } }
                            s6_row := View{ width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5} visible: false
                                s6_name := DimLabel{ width: 220 } s6_bar := ProgressBar{ height: 4 } }
                        }
                        stages_scroll := QuietScrollY{
                            width: Fill
                            height: 170
                            stages_label := DimLabel{ text: "No pipeline yet — pick a chain above and Generate." }
                        }

                        Divider{}
                        PanelHeading{ text: "Fleet" margin: Inset{top: 0} }
                        fleet_scroll := QuietScrollY{
                            width: Fill
                            height: 110
                            fleet_label := DimLabel{ text: "Discovering fleet…" }
                        }
                        }
                    }

                    right_panel := View{
                        width: Fill
                        height: Fill
                        flow: Down

                        // Headless History-preview renderer. Hosted HERE —
                        // outside both PageFlips — so its offscreen queue
                        // progresses on every frame regardless of which
                        // viewer or surface is active. One on-screen pixel
                        // keeps draw_walk alive; the child pass is 512².
                        thumbnail_renderer := ThumbnailRenderer{
                            abs_pos: vec2(0.0, 0.0)
                        }

                        surface_nav := SolidView{
                            width: Fill height: Fit flow: Right spacing: 2
                            align: Align{y: 0.5}
                            padding: Inset{left: 10 right: 10 top: 5 bottom: 5}
                            draw_bg +: { color: #x0f0f12 }
                            nav_create := SurfaceTab{ text: "● CREATE" }
                            nav_chat := SurfaceTab{ text: "CHAT" }
                            nav_library := SurfaceTab{ text: "LIBRARY" }
                            nav_import := SurfaceTab{ text: "LOAD" }
                            nav_runs := SurfaceTab{ text: "RUNS + WORKERS" }
                            nav_admin := SurfaceTab{ text: "ADMIN + AUDIT" }
                            View{ width: Fill height: Fit }
                            remote_connection := Label{
                                text: "SERVER · DISCONNECTED"
                                draw_text +: {
                                    color: #xc47d74
                                    text_style: theme.font_bold{font_size: 7}
                                }
                            }
                        }

                        // Everything below the nav flips between the surfaces.
                        // Create keeps the viewer + History strip; Import is
                        // the OSS pack catalog; Library/Runs/Admin are the
                        // Asset Store views. The left generator panel stays
                        // visible on all of them, so pipeline activity and
                        // Stop are never hidden.
                        surfaces := PageFlip{
                            width: Fill
                            height: Fill
                            active_page: @create_surface

                            create_surface := View{
                                width: Fill
                                height: Fill
                                flow: Down

                                // The viewer always shows the selected History item (a
                                // freshly generated artifact selects itself). This
                                // header says what that is.
                                viewer_head := SolidView{
                                    width: Fill height: Fit flow: Right spacing: 8
                                    align: Align{y: 0.5}
                                    padding: Inset{left: 14 right: 14 top: 8 bottom: 8}
                                    draw_bg +: { color: #x121215 }
                                    viewer_badge := RoundedView{
                                        visible: false
                                        width: Fit height: Fit
                                        padding: Inset{left: 7 right: 7 top: 2 bottom: 2}
                                        draw_bg +: {
                                            color: #x14283c
                                            border_color: #x3d9bf04d
                                            border_size: 1.0
                                            border_radius: 2.5
                                        }
                                        viewer_badge_label := Label{
                                            draw_text +: {
                                                color: #x7db8f0
                                                text_style: theme.font_bold{font_size: 7.5}
                                            }
                                        }
                                    }
                                    viewer_caption := Label{
                                        width: Fill
                                        text: "Nothing selected — run a chain, or pick something from History below."
                                        draw_text +: {
                                            color: #xb4bdc7
                                            text_style: theme.font_regular{font_size: 9}
                                        }
                                    }
                                }

                                pages := PageFlip{
                                    width: Fill
                                    height: Fill
                                    active_page: @text_page

                                    choice_page := SolidView{
                                        width: Fill height: Fill
                                        flow: Down spacing: 8
                                        padding: Inset{left: 12 right: 12 top: 10 bottom: 10}
                                        draw_bg +: { color: #x0d0d10 }
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            choice_head := BrightLabel{
                                                text: "Fleet image candidates are starting…"
                                            }
                                            retry_candidates_btn := GhostButton{
                                                text: "Retry failed slots"
                                                visible: false
                                            }
                                            continue_early_btn := GhostButton{
                                                text: "Continue Early · Cancel Remaining"
                                                visible: false
                                            }
                                            continue_choice_btn := PrimaryButton{
                                                text: "Continue After All 8 · Create Video"
                                            }
                                        }
                                        choice_hint := DimLabel{
                                            text: "Eight stable slots run in balanced GPU waves. Select one landed image; wait for all eight, or continue early and cancel the rest."
                                        }
                                        View{
                                            width: Fill height: 180
                                            align: Align{x: 0.5 y: 0.5}
                                            candidate_preview := Image{
                                                width: Fill height: Fill
                                                fit: ImageFit.Smallest
                                            }
                                        }
                                        candidate_sheet := mod.widgets.CandidateSheet{}
                                    }

                                    text_page := QuietScrollY{
                                        width: Fill
                                        height: Fill
                                        padding: 16
                                        show_bg: true
                                        draw_bg +: {
                                            color: instance(#x0d0d10)
                                            pixel: fn() {
                                                return Pal.premul(self.color)
                                            }
                                        }
                                        text_out := Label{
                                            width: Fill
                                            text: "Text results (prompt expansions, variants) appear here."
                                            draw_text +: {
                                                color: #xc6cfd8
                                                text_style: theme.font_regular{font_size: 9.5}
                                            }
                                        }
                                    }

                                    image_page := SolidView{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        draw_bg +: { color: #x0d0d10 }
                                        image_tools := View{
                                            width: Fill height: Fit flow: Right
                                            padding: Inset{left: 10 right: 10 top: 6}
                                            View{ width: Fill height: Fit }
                                            alpha_btn := ChipButton{ text: "Alpha matte" }
                                        }
                                        image_body := View{
                                            width: Fill height: Fill
                                            align: Align{x: 0.5 y: 0.5}
                                            image_view := Image{
                                                width: Fill
                                                height: Fill
                                                fit: ImageFit.Smallest
                                                draw_bg +: {
                                                    // Alpha evaluation: a checkerboard
                                                    // shows through wherever alpha < 1
                                                    // (invisible on opaque images), and
                                                    // alpha_view flips to the matte —
                                                    // alpha as grayscale. Branchless:
                                                    // shader `if` on a uniform
                                                    // mis-evaluates headless.
                                                    alpha_view: uniform(0.0)
                                                    pixel: fn() {
                                                        let color = self.get_color()
                                                        let uv = self.pos * (self.fit_scale * self.image_scale) + (self.fit_pan * self.image_scale + self.image_pan)
                                                        let inside = step(0.0, uv.x) * step(uv.x, 1.0) * step(0.0, uv.y) * step(uv.y, 1.0)
                                                        let p = floor(self.pos * self.rect_size / 8.0)
                                                        let check = modf(p.x + p.y, 2.0)
                                                        let board = mix(#x26262b, #x3c3c42, check)
                                                        let normal = vec4(mix(board.xyz, color.xyz, color.w * self.opacity), 1.0)
                                                        let matte = vec4(color.w, color.w, color.w, 1.0)
                                                        return Pal.premul(mix(normal, matte, self.alpha_view) * inside)
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    audio_page := SolidView{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        padding: 16
                                        spacing: 10
                                        draw_bg +: { color: #x0d0d10 }
                                        audio_info := BrightLabel{
                                            text: "Audio results (wav) appear here."
                                        }
                                        wave_scrub := View{
                                            width: Fill
                                            height: 140
                                            flow: Overlay
                                            wave_img := Image{
                                                width: Fill
                                                height: Fill
                                                fit: ImageFit.Stretch
                                            }
                                            // Drawn playhead: played-region tint + a 2px
                                            // position line, driven by the `progress`
                                            // uniform (device-clocked playback position).
                                            // `active` hides both while no clip is loaded.
                                            wave_playhead := SolidView{
                                                width: Fill
                                                height: Fill
                                                draw_bg +: {
                                                    progress: uniform(0.0)
                                                    active: uniform(0.0)
                                                    pixel: fn() {
                                                        let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                                                        let p = clamp(self.progress, 0.0, 1.0)
                                                        let x = p * self.rect_size.x
                                                        sdf.rect(0.0, 0.0, x, self.rect_size.y)
                                                        sdf.fill(vec4(0.24, 0.61, 0.94, 0.10 * self.active))
                                                        sdf.rect(max(x - 1.0, 0.0), 0.0, 2.0, self.rect_size.y)
                                                        sdf.fill(vec4(0.36, 0.72, 1.0, 0.92 * self.active))
                                                        return sdf.result
                                                    }
                                                }
                                            }
                                        }
                                        audio_buttons := View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            play_btn := ChipButton{ text: "Play" }
                                            stop_btn := GhostButton{ text: "Restart" }
                                        }
                                    }

                                    video_page := SolidView{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        draw_bg +: { color: #x0d0d10 }
                                        video_info := DimLabel{
                                            margin: Inset{left: 16 top: 10}
                                            text: "Video results (mp4) appear here."
                                        }
                                        video_img := Image{
                                            width: Fill
                                            height: Fill
                                            fit: ImageFit.Smallest
                                        }
                                    }

                                    mesh_page := SolidView{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        draw_bg +: { color: #x0d0d10 }
                                        mesh_view := MeshView{}
                                        // (MeshView draws its own orbit/zoom hint.)
                                        View{
                                            width: Fill height: Fit flow: Right
                                            align: Align{y: 0.5}
                                            padding: Inset{left: 10 right: 10 top: 4 bottom: 6}
                                            spacing: 8
                                            HintLabel{ text: "Scene" }
                                            View{ width: Fill height: Fit }
                                            shadows_toggle := CheckBox{
                                                text: "Shadows"
                                                active: true
                                                padding: Inset{left: 4 right: 4 top: 1 bottom: 1}
                                                draw_text +: {
                                                    color: #x828a93
                                                    text_style: theme.font_regular{font_size: 8.5}
                                                }
                                            }
                                            dark_toggle := CheckBox{
                                                text: "Dark"
                                                active: false
                                                padding: Inset{left: 4 right: 4 top: 1 bottom: 1}
                                                draw_text +: {
                                                    color: #x828a93
                                                    text_style: theme.font_regular{font_size: 8.5}
                                                }
                                            }
                                            // PBR lane only: softbox environment + strong key so
                                            // metallic/roughness maps actually read.
                                            studio_toggle := CheckBox{
                                                text: "Studio light"
                                                active: true
                                                padding: Inset{left: 4 right: 4 top: 1 bottom: 1}
                                                draw_text +: {
                                                    color: #x828a93
                                                    text_style: theme.font_regular{font_size: 8.5}
                                                }
                                            }
                                            speculars_toggle := CheckBox{
                                                text: "Speculars"
                                                active: true
                                                padding: Inset{left: 4 right: 4 top: 1 bottom: 1}
                                                draw_text +: {
                                                    color: #x828a93
                                                    text_style: theme.font_regular{font_size: 8.5}
                                                }
                                            }
                                            HintLabel{ text: "View" }
                                            pbr_view_drop := FieldDrop{ width: 110 }
                                        }
                                    }

                                    billboard_page := SolidView{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        draw_bg +: { color: #x0d0d10 }
                                        billboard_view := BillboardView{}
                                    }

                                    splat_page := SolidView{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        draw_bg +: { color: #x0d0d10 }
                                        splat_scene := XrSceneView{
                                            width: Fill
                                            height: Fill
                                            // Frame the normalized splat scene (radius
                                            // ~1.1) to fill the view, and let the wheel
                                            // zoom INSIDE generated worlds.
                                            camera.distance: 1.5
                                            camera.distance_min: 0.03
                                            splat := ViewSplat{
                                                // Orientation is set per-load in
                                                // set_splat_file (worlds y-up, scans
                                                // y-down).
                                                scale: vec3(1.0, 1.0, 1.0)
                                            }
                                        }
                                        View{
                                            width: Fill height: Fit flow: Right
                                            align: Align{y: 0.5}
                                            padding: Inset{left: 10 right: 10 top: 4 bottom: 6}
                                            HintLabel{ text: "drag to look · wheel to move" }
                                            View{ width: Fill height: Fit }
                                            sample_splat_btn := GhostButton{ text: "load sample splat" }
                                        }
                                    }
                                }

                                // History: disk-backed library of everything generated.
                                // The thumbnails are the selection UI — clicking one
                                // shows it in the viewer (accent border = selected).
                                library_panel := SolidView{
                                    width: Fill
                                    height: Fit
                                    flow: Down
                                    padding: Inset{left: 10 right: 10 top: 6 bottom: 10}
                                    spacing: 6
                                    draw_bg +: { color: #x121215 }
                                    View{
                                        width: Fill height: Fit flow: Right
                                        align: Align{y: 0.5}
                                        PanelHeading{ text: "History" margin: Inset{top: 0} }
                                        View{ width: Fill height: Fit }
                                        library_hint := HintLabel{ text: "one tile per run · click to view · grab-icon to drag out · × removes the whole run" }
                                        open_library_btn := GhostButton{ text: "Library ›" }
                                    }
                                    library_gallery := mod.widgets.LibraryGallery{}
                                }
                            }

                            // ---- Chat: local Fleet Qwen + generation tools ----
                            chat_surface := SolidView{
                                width: Fill
                                height: Fill
                                flow: Down
                                padding: 12
                                spacing: 8
                                draw_bg +: { color: #x0d0d10 }
                                View{
                                    width: Fill height: Fit flow: Down spacing: 3
                                    PanelHeading{ text: "Qwen · fleet" margin: Inset{top: 0} }
                                    chat_status := DimLabel{
                                        text: "Waiting for Qwen fleet…"
                                    }
                                    HintLabel{
                                        text: "Ask for an image, video, sfx, speech, music, mesh, world, or character. Qwen calls the matching *.generate tool."
                                    }
                                }
                                chat_list := ContentChat{}
                                View{
                                    width: Fill height: Fit flow: Right spacing: 6
                                    align: Align{y: 1.0}
                                    chat_input := TextInputFlat{
                                        width: Fill
                                        height: 56
                                        is_multiline: true
                                        submit_on_enter: true
                                        empty_text: "hey make me an image of a rusty trawler at dawn"
                                        draw_text +: {
                                            text_style: theme.font_regular{font_size: 9}
                                            color: #xdfe6ec
                                            color_empty: #x5a616a
                                        }
                                        draw_bg +: {
                                            border_radius: 3.0
                                            border_size: 1.0
                                            color: #x161619
                                            border_color: #xffffff14
                                            border_color_focus: #x3d9bf066
                                        }
                                    }
                                    chat_send_btn := PrimaryButton{ text: "Send" }
                                    chat_cancel_btn := DangerButton{ text: "Stop" visible: false }
                                }
                            }

                            // ---- Library: the Local/Server asset browser ----
                            library_surface := SolidView{
                                width: Fill
                                height: Fill
                                flow: Down
                                padding: 12
                                spacing: 8
                                draw_bg +: { color: #x0d0d10 }

                                View{
                                    width: Fill height: Fit flow: Right spacing: 8
                                    align: Align{y: 0.5}
                                    SurfaceTitle{ text: "Library" width: Fit }
                                    lib_local_tab := ChipButton{ text: "● Local" }
                                    lib_server_tab := ChipButton{ text: "Server" }
                                    View{ width: Fill height: Fit }
                                    lib_count := HintLabel{ text: "" }
                                }
                                View{
                                    width: Fill height: Fit flow: Down spacing: 6
                                    View{
                                        width: Fill height: Fit flow: Right spacing: 6
                                        align: Align{y: 0.5}
                                        lib_search := FilterInput{ width: 250 empty_text: "Search label, prompt, id…" }
                                        FieldCaption{ text: "Tags" }
                                        lib_tag_drop := FieldDrop{ width: 220 }
                                        lib_clear_btn := GhostButton{ text: "Clear" }
                                        lib_enhance_btn := GhostButton{ text: "Enhance metadata" }
                                    }
                                    lib_tag_chips := View{
                                        visible: false
                                        width: Fill height: 22
                                        flow: Right
                                        spacing: 6
                                        align: Align{y: 0.5}
                                        lib_tag_chips_label := HintLabel{
                                            text: "filters"
                                            width: Fit
                                            height: Fill
                                            align: Align{y: 0.5}
                                        }
                                        ft0 := FilterTagChip{}
                                        ft1 := FilterTagChip{}
                                        ft2 := FilterTagChip{}
                                        ft3 := FilterTagChip{}
                                        ft4 := FilterTagChip{}
                                        ft5 := FilterTagChip{}
                                        ft6 := FilterTagChip{}
                                        ft7 := FilterTagChip{}
                                    }
                                }
                                View{
                                    width: Fill height: Fill flow: Right spacing: 8
                                    lib_pages := PageFlip{
                                        width: Fill
                                        height: Fill
                                        active_page: @lib_local_page
                                        lib_local_page := View{
                                            width: Fill height: Fill
                                            lib_grid := mod.widgets.LibraryGrid{}
                                        }
                                        lib_server_page := View{
                                            width: Fill height: Fill flow: Down spacing: 6
                                            lib_server_note := HintLabel{ width: Fill text: "" }
                                            lib_server_list := mod.widgets.StoreListPanel{}
                                        }
                                    }
                                    // Selected-item rail: prompt + provenance +
                                    // revision/publish detail and the actions.
                                    detail_panel := StorePanel{
                                        width: 330
                                        height: Fill
                                        detail_scroll := QuietScrollY{
                                            width: Fill height: Fill
                                            flow: Down
                                            spacing: 4
                                            View{
                                                width: Fill height: Fit flow: Right spacing: 6
                                                align: Align{y: 0.5}
                                                detail_badge := RoundedView{
                                                    visible: false
                                                    width: Fit height: Fit
                                                    padding: Inset{left: 7 right: 7 top: 2 bottom: 2}
                                                    draw_bg +: {
                                                        color: #x14283c
                                                        border_color: #x3d9bf04d
                                                        border_size: 1.0
                                                        border_radius: 2.5
                                                    }
                                                    detail_badge_label := Label{
                                                        draw_text +: {
                                                            color: #x7db8f0
                                                            text_style: theme.font_bold{font_size: 7.5}
                                                        }
                                                    }
                                                }
                                                detail_title := Label{
                                                    width: Fill
                                                    text: "Nothing selected"
                                                    draw_text +: {
                                                        color: #xdfe6ec
                                                        text_style: theme.font_bold{font_size: 10}
                                                    }
                                                }
                                            }
                                            detail_meta := DimLabel{ text: "Click a thumbnail to see its details." }
                                            Divider{}
                                            detail_head_a := PanelHeading{ text: "Prompt" }
                                            detail_prompt := MonoLabel{ text: "—" }
                                            PanelHeading{ text: "Provenance" }
                                            detail_prov := MonoLabel{ text: "—" }
                                            PanelHeading{ text: "Revisions" }
                                            detail_rev := MonoLabel{ text: "—" }
                                            PanelHeading{ text: "Publish" }
                                            detail_publish := MonoLabel{ text: "—" }
                                            detail_actions := View{
                                                width: Fill height: Fit flow: Right spacing: 6
                                                margin: Inset{top: 10}
                                                visible: false
                                                detail_open_btn := ChipButton{ text: "Open in viewer" }
                                                detail_reuse_btn := GhostButton{ text: "Reuse prompt" }
                                                detail_delete_btn := DangerButton{ text: "Delete" }
                                            }
                                        }
                                    }
                                }
                            }

                            // ---- Runs + Workers: compact activity view ----
                            runs_surface := SolidView{
                                width: Fill
                                height: Fill
                                flow: Down
                                padding: 12
                                spacing: 6
                                draw_bg +: { color: #x0d0d10 }
                                View{
                                    width: Fill height: Fit flow: Right spacing: 8
                                    align: Align{y: 0.5}
                                    SurfaceTitle{ text: "Runs & Workers" width: Fit }
                                    View{ width: Fill height: Fit }
                                    HintLabel{ text: "local pipeline + LAN fleet are live · server queue needs a connected Asset Store" }
                                }
                                runs_list := mod.widgets.StoreListPanel{}
                            }

                            // ---- Import: hardcoded OSS pack modules ----
                            import_surface := SolidView{
                                width: Fill
                                height: Fill
                                flow: Down
                                padding: 12
                                spacing: 8
                                draw_bg +: { color: #x0d0d10 }
                                View{
                                    width: Fill height: Fit flow: Right spacing: 8
                                    align: Align{y: 0.5}
                                    SurfaceTitle{ text: "Load" width: Fit }
                                    View{ width: Fill height: Fit }
                                    HintLabel{ text: "one load at a time · license on every card · shareware is local preview only" }
                                }
                                import_queue_card := StoreSection{
                                    spacing: 6
                                    padding: 8
                                    View{
                                        width: Fill height: Fit flow: Right spacing: 8
                                        align: Align{y: 0.5}
                                        BrightLabel{ text: "Loading" }
                                        View{ width: Fill height: Fit }
                                        queue_clear_btn := GhostButton{ text: "Clear" visible: false }
                                    }
                                    import_queue_list := mod.widgets.StoreListPanel{
                                        width: Fill
                                        height: 40
                                    }
                                    import_preview := View{
                                        width: Fill height: 72
                                        visible: false
                                        flow: Right spacing: 4
                                        it0 := View{
                                            width: 72 height: 72
                                            align: Align{x: 0.5 y: 0.5}
                                            import_t0 := ThumbFitImage{}
                                        }
                                        it1 := View{
                                            width: 72 height: 72
                                            align: Align{x: 0.5 y: 0.5}
                                            import_t1 := ThumbFitImage{}
                                        }
                                        it2 := View{
                                            width: 72 height: 72
                                            align: Align{x: 0.5 y: 0.5}
                                            import_t2 := ThumbFitImage{}
                                        }
                                        it3 := View{
                                            width: 72 height: 72
                                            align: Align{x: 0.5 y: 0.5}
                                            import_t3 := ThumbFitImage{}
                                        }
                                        it4 := View{
                                            width: 72 height: 72
                                            align: Align{x: 0.5 y: 0.5}
                                            import_t4 := ThumbFitImage{}
                                        }
                                        it5 := View{
                                            width: 72 height: 72
                                            align: Align{x: 0.5 y: 0.5}
                                            import_t5 := ThumbFitImage{}
                                        }
                                        it6 := View{
                                            width: 72 height: 72
                                            align: Align{x: 0.5 y: 0.5}
                                            import_t6 := ThumbFitImage{}
                                        }
                                        it7 := View{
                                            width: 72 height: 72
                                            align: Align{x: 0.5 y: 0.5}
                                            import_t7 := ThumbFitImage{}
                                        }
                                    }
                                }
                                import_scroll := QuietScrollY{
                                    width: Fill
                                    height: Fill
                                    flow: Down
                                    spacing: 3

                                    kenney_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            kenney_import_btn := PrimaryButton{ text: "Load" }
                                            kenney_import_all_btn := GhostButton{ text: "Load all" }
                                            BrightLabel{ text: "Kenney" width: 88 }
                                            HintLabel{ text: "CC BY 4.0 · attribution required" }
                                            LinkLabel{ text: "Terms" url: "https://creativecommons.org/licenses/by/4.0/" }
                                            kenney_pack_drop := FieldDrop2{ width: 180 }
                                        }
                                        HintLabel{ text: "© Kenney (kenney.nl). Attribution required on every copy and derivative. Not CC0. Local kits only — this card does not download Kenney." }
                                    }

                                    freedoom_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            freedoom_import_btn := PrimaryButton{ text: "Load" }
                                            BrightLabel{ text: "Freedoom" width: 88 }
                                            HintLabel{ text: "BSD-3-Clause · attribution required" }
                                            LinkLabel{ text: "Terms" url: "https://github.com/freedoom/freedoom/blob/master/COPYING.adoc" }
                                        }
                                        HintLabel{ text: "© Freedoom contributors. BSD-3-Clause — credit Freedoom. Original Freedoom art, not id Software shareware, not retail Doom, not CC0." }
                                    }

                                    doom_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            doom_import_btn := PrimaryButton{ text: "Load" }
                                            BrightLabel{ text: "Doom shareware" width: 130 }
                                            HintLabel{ text: "© id Software · shareware · local preview only" }
                                            LinkLabel{ text: "Terms" url: "https://doomwiki.org/wiki/DOOM1.WAD" }
                                        }
                                        HintLabel{ text: "Official Doom shareware (episode 1 / DOOM1.WAD). Local preview in this app only. Not a redistributable grant. Not Freedoom. Not retail Doom / Doom II." }
                                    }

                                    librequake_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            librequake_import_btn := PrimaryButton{ text: "Load" }
                                            BrightLabel{ text: "LibreQuake" width: 88 }
                                            HintLabel{ text: "Modified BSD · attribution required" }
                                            LinkLabel{ text: "Terms" url: "https://github.com/MissLav/LibreQuake/blob/master/LICENSE" }
                                        }
                                        HintLabel{ text: "© LibreQuake contributors. Modified BSD — credit LibreQuake. Original art, not id Software shareware, not retail Quake, not CC0." }
                                    }

                                    quake_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            quake_import_btn := PrimaryButton{ text: "Load" }
                                            BrightLabel{ text: "Quake shareware" width: 130 }
                                            HintLabel{ text: "© id Software · shareware · local preview only" }
                                            LinkLabel{ text: "Terms" url: "https://quakewiki.org/wiki/Getting_Started" }
                                        }
                                        HintLabel{ text: "Official Quake shareware (id1/pak0.pak). Local preview in this app only. Not a redistributable grant. Not LibreQuake. Not retail pak1." }
                                    }

                                    duke3d_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            duke3d_import_btn := PrimaryButton{ text: "Load" }
                                            BrightLabel{ text: "Duke3D shareware" width: 130 }
                                            HintLabel{ text: "© 3D Realms · shareware · local preview only" }
                                            LinkLabel{ text: "Terms" url: "https://wiki.eduke32.com/wiki/Frequently_Asked_Questions" }
                                        }
                                        HintLabel{ text: "Official Duke Nukem 3D shareware. Local preview in this app only. Not a redistributable grant. Not retail Atomic Edition. Optional HRP stays under the Duke4 HRP license." }
                                    }

                                    quake2_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            quake2_import_btn := PrimaryButton{ text: "Load" }
                                            BrightLabel{ text: "Quake II shareware" width: 130 }
                                            HintLabel{ text: "© id Software · demo · local preview only" }
                                            LinkLabel{ text: "Terms" url: "https://www.idsoftware.com/" }
                                        }
                                        HintLabel{ text: "Official Quake II Test demo. Local preview in this app only. Not a redistributable grant. Not retail Quake II." }
                                    }

                                    quake3_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            quake3_import_btn := PrimaryButton{ text: "Load" }
                                            BrightLabel{ text: "Quake III demo" width: 130 }
                                            HintLabel{ text: "© id Software · demo · local preview only" }
                                            LinkLabel{ text: "Terms" url: "https://ioquake3.org/help/players-guide/" }
                                        }
                                        HintLabel{ text: "Official Quake III Arena demo. Local preview in this app only. Not a redistributable grant. Not OpenArena. Not retail Quake III." }
                                    }

                                    darkmod_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            darkmod_import_btn := PrimaryButton{ text: "Load" }
                                            BrightLabel{ text: "The Dark Mod" width: 130 }
                                            HintLabel{ text: "CC BY-NC-SA 3.0 · non-commercial" }
                                            LinkLabel{ text: "Terms" url: "https://creativecommons.org/licenses/by-nc-sa/3.0/" }
                                        }
                                        HintLabel{ text: "© The Dark Mod team (thedarkmod.com). Credit required, non-commercial, share-alike. Fan missions stay with their authors. Not CC0, not BSD. Load fetches official tdm_installer.ini and reconstructs tdm_*.pk4 from the HTTP zipsync mirrors." }
                                    }

                                    kaykit_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            kaykit_import_btn := PrimaryButton{ text: "Load" }
                                            BrightLabel{ text: "KayKit" width: 88 }
                                            HintLabel{ text: "CC0 1.0 · public domain dedication" }
                                            LinkLabel{ text: "Terms" url: "https://creativecommons.org/publicdomain/zero/1.0/" }
                                        }
                                        HintLabel{ text: "© Kay Lousberg / KayKit. CC0 1.0 — attribution not required; we still credit Kay Lousberg. In-repo characters, no download." }
                                    }

                                    nasa_card := ImportRow{
                                        flow: Down spacing: 2
                                        View{
                                            width: Fill height: Fit flow: Right spacing: 8
                                            align: Align{y: 0.5}
                                            BrightLabel{ text: "NASA sky" width: 88 }
                                            HintLabel{ text: "U.S. government public domain" }
                                            LinkLabel{ text: "Terms" url: "https://svs.gsfc.nasa.gov/4851" }
                                        }
                                        HintLabel{ text: "NASA/GSFC SVS Deep Star Maps 2020. Public domain. Credit NASA/GSFC SVS. Not a pack_import target." }
                                    }
                                }
                            }

                            // ---- Admin + Audit: server records only ----
                            admin_surface := SolidView{
                                width: Fill
                                height: Fill
                                flow: Down
                                padding: 12
                                spacing: 6
                                draw_bg +: { color: #x0d0d10 }
                                View{
                                    width: Fill height: Fit flow: Right spacing: 8
                                    align: Align{y: 0.5}
                                    SurfaceTitle{ text: "Admin & Audit" width: Fit }
                                    View{ width: Fill height: Fit }
                                    HintLabel{ text: "games · rooms · namespaces · audit — all server records" }
                                }
                                admin_list := mod.widgets.StoreListPanel{}
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
enum FileDragHandleAction {
    FingerDown(FingerDownEvent),
    FingerMove(FingerMoveEvent),
    FingerUp(FingerUpEvent),
    #[default]
    None,
}

/// An exclusive pointer target for outbound file dragging inside a
/// `PortalList`. The list deliberately uses capture-overload for ordinary
/// card scrolling, so a normal child `View` cannot outlive the list's 5px
/// scroll threshold. Sweep-locking on the raw down keeps this handle as the
/// only hit route for the gesture while leaving the rest of every card's
/// scroll behavior untouched.
#[derive(Script, ScriptHook, Widget)]
pub struct FileDragHandle {
    #[deref]
    view: View,
    #[rust(false)]
    pointer_owned: bool,
}

impl Widget for FileDragHandle {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let area = self.view.area();
        if matches!(event, Event::DragEnd) {
            if self.pointer_owned {
                cx.sweep_unlock(area);
                self.pointer_owned = false;
            }
            return;
        }
        if !self.view.visible() && event.requires_visibility() {
            if self.pointer_owned {
                cx.sweep_unlock(area);
                self.pointer_owned = false;
            }
            return;
        }

        // A non-empty sweep area lets the handle exclude the enclosing
        // PortalList. Once captured, keep accepting moves outside the small
        // visible chip so a drag that starts near an edge still reaches the
        // file-drag threshold.
        let pointer_owned = self.pointer_owned;
        let hit = event.hits_with_options_and_test(
            cx,
            area,
            HitOptions::new().with_sweep_area(area),
            move |abs, rect, _margin| pointer_owned || rect.contains(abs),
        );
        let uid = self.widget_uid();
        match hit {
            Hit::FingerDown(event) if event.is_primary_hit() => {
                self.pointer_owned = true;
                cx.sweep_lock(area);
                cx.set_cursor(MouseCursor::Grabbing);
                cx.widget_action(uid, FileDragHandleAction::FingerDown(event));
            }
            Hit::FingerMove(event) if self.pointer_owned => {
                cx.set_cursor(MouseCursor::Grabbing);
                cx.widget_action(uid, FileDragHandleAction::FingerMove(event));
            }
            Hit::FingerUp(event) if self.pointer_owned => {
                cx.widget_action(uid, FileDragHandleAction::FingerUp(event));
                cx.sweep_unlock(area);
                self.pointer_owned = false;
                cx.set_cursor(MouseCursor::Grab);
            }
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => {
                cx.set_cursor(MouseCursor::Grab);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }
}

impl FileDragHandleRef {
    fn finger_down(&self, actions: &Actions) -> Option<FingerDownEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FileDragHandleAction::FingerDown(event) = item.cast() {
                return Some(event);
            }
        }
        None
    }

    fn finger_move(&self, actions: &Actions) -> Option<FingerMoveEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FileDragHandleAction::FingerMove(event) = item.cast() {
                return Some(event);
            }
        }
        None
    }

    fn finger_up(&self, actions: &Actions) -> Option<FingerUpEvent> {
        if let Some(item) = actions.find_widget_action(self.widget_uid()) {
            if let FileDragHandleAction::FingerUp(event) = item.cast() {
                return Some(event);
            }
        }
        None
    }
}

/// Repo-local artifact spill (mp4 for the decoder, ply for file_resource;
/// also handy for post-run inspection). local/ is git-ignored.
fn artifacts_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../local/ai_content_app"))
}

fn repo_path(rel: &str) -> String {
    format!("{}/../../{}", env!("CARGO_MANIFEST_DIR"), rel)
}

const QUEUE_ROWS: usize = 6;
/// Chip slots in the run tray (character runs have 7 artifacts).
const RUN_TRAY_SLOTS: usize = 8;

/// The immutable input attachment of a seeded (transform) run: the exact
/// managed payload bytes snapshotted at ENQUEUE, so deleting or evicting
/// the library record afterwards can never mutate a queued run (and a
/// transform never mutates its source — it only reads this snapshot into a
/// brand-new group). `source_file` holds the managed id today and is the
/// slot for the typed asset_client `AssetRevision` id later; this struct
/// is that attachment seam — swap the bytes for a revision handle here
/// without touching dispatch call sites.
#[derive(Clone)]
struct RunSeed {
    source_file: String,
    source_label: String,
    /// Exact stored content type, sent verbatim with the exact bytes.
    content_type: String,
    bytes: std::sync::Arc<Vec<u8>>,
    /// Producer-prefix stages of the preset this input replaces — derived
    /// and validated by [`pipeline::seed_replaces_prefix`] at spec time,
    /// so it is always a valid in-bounds stage index.
    skip: usize,
}

/// A pipeline waiting in the app-side run queue.
#[derive(Clone)]
struct PendingRun {
    prompt: String,
    preset: usize,
    model_overrides: Vec<(String, String)>,
    box_override: Option<String>,
    /// Voice pack for speech stages (None = backend default).
    voice: Option<String>,
    /// Generation knobs from the params dropdowns.
    gen: GenParams,
    /// Persisted History group. Allocated when the run spec is CREATED, so
    /// every distinct queued run carries a distinct id while a Retry of the
    /// same spec keeps its group.
    group_id: String,
    group_label: String,
    /// Seeded transform input; None = the preset generates its own inputs.
    input: Option<RunSeed>,
}

impl PendingRun {
    /// The chain actually dispatched: the preset's domains minus the
    /// producer prefix a seeded input replaces.
    fn domains(&self) -> &'static [&'static str] {
        let all = PRESETS[self.preset].domains;
        match &self.input {
            Some(seed) => &all[seed.skip.min(all.len().saturating_sub(1))..],
            None => all,
        }
    }
}

/// One dispatched pipeline. Several may run concurrently on distinct fleet
/// endpoints; artifact routing reads THIS run's prompt/group so completions
/// arriving out of order can never cross-attribute.
struct ActiveRun {
    id: u64,
    group_id: String,
    group_label: String,
    prompt: String,
    pipeline: Pipeline,
}

/// Converted .mkvoice packs in the registry; first entry is the default.
/// All 28 English Kokoro v1.0 voices (af_/am_ American, bf_/bm_ British).
const VOICES: &[&str] = &[
    "bm_daniel", "bm_fable", "bm_george", "bm_lewis",
    "bf_alice", "bf_emma", "bf_isabella", "bf_lily",
    "af_alloy", "af_aoede", "af_bella", "af_heart", "af_jessica", "af_kore",
    "af_nicole", "af_nova", "af_river", "af_sarah", "af_sky",
    "am_adam", "am_echo", "am_eric", "am_fenrir", "am_liam", "am_michael",
    "am_onyx", "am_puck", "am_santa",
];

/// Right-pane surface behind the nav tabs. Create keeps the viewer +
/// History strip; Import is the OSS pack catalog; the others are Asset
/// Store views.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Surface {
    #[default]
    Create,
    Chat,
    Library,
    Import,
    Runs,
    Admin,
}

/// Which side of the Library the browser shows: the disk-backed local
/// History, or the session-backed server catalog.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum LibSource {
    #[default]
    Local,
    Server,
}

#[derive(Default)]
struct AutoRun {
    /// Preset-name substring from AI_CONTENT_AUTO; consumed once fired.
    preset: Option<String>,
    prompt: Option<String>,
    /// AI_CONTENT_QUEUE: extra preset substrings queued at fire time
    /// (';'-separated) — exercises the run queue.
    queue: Vec<String>,
    /// AI_CONTENT_SAMPLE="mesh"|"splat": load the sample asset instead of
    /// running a pipeline (viewer smoke without live mesh/world backends).
    sample: Option<String>,
    /// AI_CONTENT_SURFACE="library"|"import"|"runs"|"admin": start on that
    /// surface (headless captures of the Asset Store / Import views).
    surface: Option<String>,
    /// ASSET_UI_IMPORT="duke3d": queue that classic Import card once the UI is up.
    import: Option<String>,
    capture: Option<PathBuf>,
    /// AI_CONTENT_CAPTURE_AT_S: capture at a fixed time after startup
    /// (mid-pipeline shots) instead of on completion.
    capture_at_s: Option<f64>,
    exit: bool,
    fired: bool,
    captured: bool,
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    fleet: Option<FleetPoll>,
    /// Concurrently active (and a few recently finished) pipeline runs.
    /// Each owns unique request ids, so they share one response stream.
    #[rust]
    runs: Vec<ActiveRun>,
    #[rust(0u64)]
    next_run_id: u64,
    /// App-side run queue: runs wait HERE (not on a box) whenever every
    /// compatible endpoint slot is occupied; the fleet-aware planner drains
    /// it as capacity frees. Reorder is reorder-before-submit.
    #[rust]
    run_queue: Vec<PendingRun>,
    /// The last run spec, for Retry after a failure.
    #[rust]
    last_run: Option<PendingRun>,
    #[rust]
    fleet_timer: Timer,
    /// LAN beacon listener; polled on the fleet timer.
    #[rust]
    discovered: Option<makepad_asset_ai::discovery::Discovered>,
    #[rust]
    job_timer: Timer,
    #[rust]
    capture_timer: Timer,
    /// One-shot: applies the startup surface (env or default) after the
    /// initial script apply has settled.
    #[rust]
    surface_timer: Timer,
    #[rust]
    sample_timer: Timer,
    #[rust]
    exit_timer: Timer,
    /// Per-domain model ids behind each stage-model dropdown (minus "auto").
    #[rust]
    model_choices: Vec<(String, Vec<String>)>,
    /// User-saved one-click pipeline snapshots.
    #[rust]
    saved_presets: Vec<SavedFastPreset>,
    /// Box urls behind the box selector (minus "auto").
    #[rust]
    box_choices: Vec<String>,
    #[rust]
    library: Option<Library>,
    #[rust]
    video: Option<VideoPlayer>,
    #[rust]
    video_texture: Option<Texture>,
    #[rust]
    video_pump: NextFrame,
    #[rust]
    audio_clip: Option<audio::WavPcm>,
    /// UI refresh only; the device callback owns playback time.
    #[rust]
    audio_timer: Timer,
    /// Per-frame waveform-playhead updates, alive ONLY while playback is
    /// audible and the audio viewer is visible (Create surface + audio
    /// page); parks otherwise. The 10Hz audio_timer keeps the text readout.
    #[rust]
    audio_pump: NextFrame,
    /// The viewer PageFlip currently shows the audio page (set by
    /// show_page) — the pump's visibility gate.
    #[rust(false)]
    audio_page_active: bool,
    /// Missing-preview regeneration work (badges show meanwhile). Built once
    /// from metadata at startup; drained a bounded slice per timer tick.
    #[rust]
    thumbnail_backfill: VecDeque<ThumbnailBackfillJob>,
    #[rust]
    thumbnail_timer: Timer,
    /// The one background payload-IO worker (viewer opens + preview reads).
    #[rust]
    artifact_io: Option<ArtifactIo>,
    /// Latest-selection-wins gate over async viewer opens.
    #[rust]
    viewer_gate: ViewerOpenGate,
    /// At most one thumbnail-source read in flight at the worker.
    #[rust(false)]
    thumb_read_in_flight: bool,
    #[rust]
    artifact_count: u64,
    /// Stable managed file id currently SELECTED (blue ring) — unlike a
    /// newest-first slot, this remains correct when another item is deleted.
    #[rust]
    selected_file: Option<String>,
    /// Persistent selected-input tray for seeded transform runs. Explicit
    /// user picks only: artifacts landing from finished runs and async
    /// viewer commits never touch it (they only move `selected_file`).
    #[rust]
    input_tray: InputTray,
    /// File whose chip the tray currently renders — gates redundant text/
    /// thumbnail refreshes and drops stale async chip previews.
    #[rust]
    input_chip_file: Option<String>,
    /// Members of the selected run shown in the run tray, pipeline order
    /// (oldest first), one per chip slot.
    #[rust]
    run_tray_files: Vec<String>,
    /// Preview decodes wanted by non-gallery widgets (run tray chips) —
    /// drained by `pump_gallery_previews` alongside the gallery caches.
    #[rust]
    extra_preview_work: Vec<(String, PreviewWork)>,
    /// Native outbound file drag is one OS session per pointer gesture. It is
    /// cleared only by DragEnd so repeated move actions cannot start twice.
    #[rust(false)]
    file_drag_active: bool,
    /// Candidate whose bytes are currently bound to the human-gate preview.
    /// Avoids decoding the same landed PNG on every status tick.
    #[rust]
    candidate_preview_id: Option<String>,
    /// What the central viewer is COMMITTED to (selection moves instantly;
    /// content transitions Loading → Showing/Failed only via the async
    /// commit — see artifact_io::ViewerContent).
    #[rust]
    viewer: ViewerContent,
    /// Files whose gallery PNG inflate is currently running on the decode
    /// pool. The worker stack is last-requested-first; this list only
    /// suppresses a same-file resubmit while that inflate is live.
    #[rust]
    preview_in_flight: Vec<String>,
    /// Image viewer in alpha-matte mode (alpha as grayscale).
    #[rust]
    alpha_view: bool,
    /// Voice dropdown currently lists Kokoro packs (vs the honest n/a state
    /// shown when the effective speech backend is not Kokoro).
    #[rust(true)]
    voice_drop_is_kokoro: bool,
    /// Active right-pane surface (nav tabs).
    #[rust]
    surface: Surface,
    /// Library browser source: local History vs server catalog.
    #[rust]
    lib_source: LibSource,
    /// Live filters over the local History (query/kind/category/tag).
    #[rust]
    lib_filters: LocalLibraryFilters,
    /// Server-side state backed by one Asset Server session:
    /// discovery/auth/retry and all catalog work live on its worker threads;
    /// `poll` only drains typed results on the UI thread.
    #[rust]
    store: AssetStoreState,
    /// Local Fleet Qwen chat (never blocks the UI).
    #[rust]
    chat: ChatBridge,
    /// Last fleet URL set sent to the chat worker (retry / membership).
    #[rust]
    chat_fleet_bases: Vec<String>,
    #[rust]
    chat_qwen_retry_at: Option<std::time::Instant>,
    /// Asset Server catalog tools have been handed to the chat worker.
    #[rust]
    chat_asset_linked: bool,
    /// Hardcoded OSS pack catalog (Kenney compile + honest empty states).
    #[rust]
    import_page: ImportPage,
    /// Freedoom / LibreQuake / Duke3D / Quake II / Quake III Import cards.
    #[rust]
    classic_import_page: ClassicImportPage,
    /// One running import plus a user-editable wait list.
    #[rust]
    import_queue: ImportQueue,
    /// Landings waiting to be written a few at a time so the UI stays live.
    #[rust]
    import_landings: Vec<crate::import::LibraryLanding>,
    /// Keeps discovery, catalog responses and the committed event feed
    /// responsive without ever blocking the UI thread.
    #[rust]
    asset_store_timer: Timer,
    /// Kind strings behind the Library kind filter dropdown (minus "all").
    #[rust]
    lib_kind_options: Vec<String>,
    /// Category (domain) strings behind the category dropdown (minus "all").
    #[rust]
    lib_cat_options: Vec<String>,
    /// Tag catalog behind the add-tag dropdown (minus the "add tag…" row).
    /// Sorted by how many items carry the tag, most-used first.
    #[rust]
    lib_tag_options: Vec<TagStat>,
    #[rust(AutoRun::default())]
    auto: AutoRun,
}

impl App {
    // -- setup ---------------------------------------------------------------

    fn setup(&mut self, cx: &mut Cx) {
        let _ = std::fs::create_dir_all(artifacts_dir());
        self.artifact_io = Some(ArtifactIo::start());
        self.library = Some(Library::open(repo_path("local/ai_content_library")));
        self.saved_presets = fast_presets::load(&fast_presets::store_path());
        if let Some(library) = &mut self.library {
            crate::enhance_meta::apply_catalog_names(library);
        }
        // One shared, real Asset Server session. Discovery/auth/retry happen
        // off-thread; this call only starts the lifecycle.
        self.store.start();
        self.asset_store_timer = cx.start_interval(0.2);
        ChatData::push(
            ChatRole::System,
            "Qwen on the GPU fleet. Ask for an image, an image turned into a \
             3D mesh, what models/sizes exist, or to change the default \
             model / resolution / steps. It works through tool calls — it \
             will not invent a finished image.",
        );
        // Opening stays metadata-only; every missing preview is queued here
        // and regenerated a bounded slice at a time once frames are flowing.
        self.thumbnail_backfill = self
            .library
            .as_ref()
            .map(|library| library.thumbnail_backfill_queue().into())
            .unwrap_or_default();
        // Base filter labels before the first refresh: the refresh only
        // re-labels these dropdowns when the option SET changes, and an
        // empty library changes nothing.
        self.refresh_gallery(cx, true);

        // GPU boxes join via the LAN beacon. Asset-ui stays on the `gen`
        // fleet so the sandbox `game` box (.123) never lands in this UI.
        if std::env::var_os("MAKEPAD_AI_FLEET").is_none() {
            std::env::set_var("MAKEPAD_AI_FLEET", "gen");
        }
        self.discovered = Some(makepad_asset_ai::discovery::start_listener());
        self.fleet = Some(FleetPoll::new());
        self.maybe_connect_chat();
        self.fleet_timer = cx.start_interval(3.0);
        self.job_timer = cx.start_interval(0.25);
        self.audio_timer = cx.start_interval(0.1);
        self.thumbnail_timer = cx.start_interval(0.5);

        // Type dropdown (pipeline). Field captions sit beside the control;
        // items are the choices only.
        let labels: Vec<String> = crate::pipeline::presets_sorted_order()
            .iter()
            .map(|&i| PRESETS[i].name.to_string())
            .collect();
        self.ui
            .drop_down2(cx, ids!(preset_drop))
            .set_labels(cx, labels);
        self.ui
            .drop_down2(cx, ids!(box_drop))
            .set_labels(cx, vec!["auto (affinity)".to_string()]);
        self.ui.drop_down2(cx, ids!(voice_drop)).set_labels(
            cx,
            std::iter::once(format!("default ({})", VOICES[0]))
                .chain(VOICES[1..].iter().map(|v| (*v).to_string()))
                .collect(),
        );
        self.ui.drop_down2(cx, ids!(size_drop)).set_labels(
            cx,
            IMAGE_SIZES
                .iter()
                .enumerate()
                .map(|(i, (w, h))| {
                    if i == 0 {
                        format!("{w}×{h} (default)")
                    } else {
                        format!("{w}×{h}")
                    }
                })
                .collect(),
        );
        self.ui.drop_down2(cx, ids!(steps_drop)).set_labels(
            cx,
            std::iter::once("model default".to_string())
                .chain(IMAGE_STEPS.iter().map(|s| s.to_string()))
                .collect(),
        );
        self.ui
            .drop_down2(cx, ids!(texture_size_drop))
            .set_labels(
                cx,
                MESH_TEXTURE_SIZES
                    .iter()
                    .enumerate()
                    .map(|(index, size)| match index {
                        0 => format!("{size} (fast default)"),
                        1 => format!("{size} (high)"),
                        _ => format!("{size} (ultra)"),
                    })
                    .collect(),
            );
        self.ui.drop_down2(cx, ids!(pbr_view_drop)).set_labels(
            cx,
            crate::mesh_view::pbr_preview::PbrViewMode::ALL
                .iter()
                .map(|mode| mode.label().to_string())
                .collect(),
        );
        self.ui.drop_down2(cx, ids!(mesh_faces_drop)).set_labels(
            cx,
            MESH_FACE_COUNTS
                .iter()
                .map(|count| match *count {
                    0 => "auto (12–20k)".to_string(),
                    n => format!("{}k", n / 1000),
                })
                .collect(),
        );
        self.ui.drop_down2(cx, ids!(vid_size_drop)).set_labels(
            cx,
            VIDEO_SIZES
                .iter()
                .enumerate()
                .map(|(i, (w, h))| {
                    if i == 0 {
                        format!("{w}×{h} (default)")
                    } else {
                        format!("{w}×{h}")
                    }
                })
                .collect(),
        );
        self.ui.drop_down2(cx, ids!(vid_len_drop)).set_labels(
            cx,
            VIDEO_LENGTHS
                .iter()
                .enumerate()
                .map(|(i, (frames, steps))| {
                    let seconds = *frames as f64 / 16.0;
                    if i == 0 {
                        format!("{seconds:.1}s · {steps} steps (default)")
                    } else {
                        format!("{seconds:.1}s · {steps} steps")
                    }
                })
                .collect(),
        );
        let music_default = MUSIC_LENGTHS
            .iter()
            .position(|seconds| *seconds == MUSIC_DEFAULT_SECONDS)
            .expect("default music duration must be a UI preset");
        let music_len_drop = self.ui.drop_down2(cx, ids!(music_len_drop));
        music_len_drop.set_labels(
            cx,
            MUSIC_LENGTHS
                .iter()
                .map(|seconds| {
                    let clock = format_music_duration(*seconds);
                    if *seconds == MUSIC_DEFAULT_SECONDS {
                        format!("{clock} (default; may end early)")
                    } else {
                        format!("{clock} (may end early)")
                    }
                })
                .collect(),
        );
        music_len_drop.set_selected_item(cx, music_default);
        self.refresh_saved_presets_ui(cx);
        self.refresh_model_ui(cx, true);
        self.refresh_voice_ui(cx);
        self.sync_preset_name_box(cx);

        // Speakers: wav artifacts + video soundtrack.
        cx.audio_output(0, move |info, output| {
            output.zero();
            crate::audio::mix_into(output, info.sample_rate);
            crate::video_player::mix_into(output, info.sample_rate);
        });

        // Headless drive.
        self.auto = AutoRun {
            preset: crate::asset_store_state::env_alias(&["ASSET_UI_AUTO", "AI_CONTENT_AUTO"]),
            prompt: crate::asset_store_state::env_alias(&["ASSET_UI_PROMPT", "AI_CONTENT_PROMPT"]),
            queue: crate::asset_store_state::env_alias(&["ASSET_UI_QUEUE", "AI_CONTENT_QUEUE"])
                .map(|s| s.split(';').map(|p| p.trim().to_string()).collect())
                .unwrap_or_default(),
            sample: crate::asset_store_state::env_alias(&["ASSET_UI_SAMPLE", "AI_CONTENT_SAMPLE"]),
            surface: crate::asset_store_state::env_alias(&["ASSET_UI_SURFACE", "AI_CONTENT_SURFACE"]),
            import: crate::asset_store_state::env_alias(&["ASSET_UI_IMPORT", "AI_CONTENT_IMPORT"]),
            capture: crate::asset_store_state::env_alias(&["ASSET_UI_CAPTURE", "AI_CONTENT_CAPTURE"])
                .map(PathBuf::from),
            capture_at_s: crate::asset_store_state::env_alias(&[
                "ASSET_UI_CAPTURE_AT_S",
                "AI_CONTENT_CAPTURE_AT_S",
            ])
            .and_then(|s| s.parse().ok()),
            exit: crate::asset_store_state::env_alias(&["ASSET_UI_EXIT", "AI_CONTENT_EXIT"]).is_some(),
            fired: false,
            captured: false,
        };
        // Applied via a short one-shot timer: the surface PageFlip's
        // active_page is a script property, and the post-startup resource
        // apply would clobber a flip done directly here.
        self.surface_timer = cx.start_timeout(0.3);
        if let Some(at) = self.auto.capture_at_s {
            self.capture_timer = cx.start_timeout(at);
        }
        if self.auto.sample.is_some() {
            self.sample_timer = cx.start_timeout(1.0);
        } else if self.auto.preset.is_none()
            && self.auto.capture.is_some()
            && self.auto.capture_at_s.is_none()
        {
            // Capture-only run: give the fleet panel time to populate.
            self.capture_timer = cx.start_timeout(6.0);
        }
    }

    // -- fleet ----------------------------------------------------------------

    fn refresh_fleet_ui(&mut self, cx: &mut Cx) {
        let Some(fleet) = &self.fleet else { return };
        self.ui
            .label(cx, ids!(fleet_label))
            .set_text(cx, &fleet.panel_text());

        let mut box_labels = vec!["auto (affinity)".to_string()];
        let mut boxes = Vec::new();
        for snap in &fleet.snapshots {
            if !snap.is_up() {
                continue;
            }
            box_labels.push(snap.base_url.trim_start_matches("http://").to_string());
            boxes.push(snap.base_url.clone());
        }
        if boxes != self.box_choices {
            self.ui.drop_down2(cx, ids!(box_drop)).set_labels(cx, box_labels);
        }
        self.box_choices = boxes;
        self.refresh_model_ui(cx, false);
        self.refresh_voice_ui(cx);
    }

    fn current_preset_index(&self, cx: &mut Cx) -> usize {
        let order = crate::pipeline::presets_sorted_order();
        let row = self
            .ui
            .drop_down2(cx, ids!(preset_drop))
            .selected_item()
            .min(order.len() - 1);
        order[row]
    }

    fn fleet_models_for_domain(&self, domain: &str) -> Vec<String> {
        let Some(fleet) = &self.fleet else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        for snap in &fleet.snapshots {
            if !snap.is_up() {
                continue;
            }
            for model in &snap.models {
                if model.available && model.domain == domain && !ids.contains(&model.id) {
                    ids.push(model.id.clone());
                }
            }
        }
        ids
    }

    fn selected_stage_model(&self, cx: &mut Cx, domain: &str) -> Option<String> {
        let drop = match domain {
            "text" => self.ui.drop_down2(cx, ids!(md_text)),
            "image" => self.ui.drop_down2(cx, ids!(md_image)),
            "audio" => self.ui.drop_down2(cx, ids!(md_audio)),
            "speech" => self.ui.drop_down2(cx, ids!(md_speech)),
            "music" => self.ui.drop_down2(cx, ids!(md_music)),
            "video" => self.ui.drop_down2(cx, ids!(md_video)),
            "mesh" => self.ui.drop_down2(cx, ids!(md_mesh)),
            "matte" => self.ui.drop_down2(cx, ids!(md_matte)),
            "depth" => self.ui.drop_down2(cx, ids!(md_depth)),
            "segment" => self.ui.drop_down2(cx, ids!(md_segment)),
            "paint" => self.ui.drop_down2(cx, ids!(md_paint)),
            "world" => self.ui.drop_down2(cx, ids!(md_world)),
            "rig" => self.ui.drop_down2(cx, ids!(md_rig)),
            "motion" => self.ui.drop_down2(cx, ids!(md_motion)),
            _ => return None,
        };
        let index = drop.selected_item().checked_sub(1)?;
        self.model_choices
            .iter()
            .find(|(name, _)| name == domain)
            .and_then(|(_, ids)| ids.get(index))
            .cloned()
    }

    fn collected_stage_models(&self, cx: &mut Cx, domains: &[&str]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for domain in domains {
            if let Some(model) = self.selected_stage_model(cx, domain) {
                out.push(((*domain).to_string(), model));
            }
        }
        out
    }

    fn refresh_one_stage_model(
        &mut self,
        cx: &mut Cx,
        domain: &str,
        row: &[LiveId],
        drop: &[LiveId],
        active: bool,
        apply_preset_pin: bool,
        pin: Option<&str>,
    ) {
        self.ui.widget(cx, row).set_visible(cx, active);
        if !active {
            return;
        }
        let previous = self.selected_stage_model(cx, domain);
        let ids = self.fleet_models_for_domain(domain);
        let labels: Vec<String> = std::iter::once("auto (affinity)".to_string())
            .chain(ids.iter().cloned())
            .collect();
        let mut select = 0usize;
        if apply_preset_pin {
            if let Some(pin) = pin {
                if let Some(index) = ids.iter().position(|id| id == pin) {
                    select = index + 1;
                }
            }
        } else if let Some(previous) = previous {
            if let Some(index) = ids.iter().position(|id| id == &previous) {
                select = index + 1;
            }
        }
        let slot = self
            .model_choices
            .iter()
            .position(|(name, _)| name == domain);
        let changed = match slot {
            Some(index) => self.model_choices[index].1 != ids,
            None => true,
        };
        if changed {
            self.ui.drop_down2(cx, drop).set_labels(cx, labels);
            match slot {
                Some(index) => self.model_choices[index].1 = ids,
                None => self.model_choices.push((domain.to_string(), ids)),
            }
        }
        self.ui.drop_down2(cx, drop).set_selected_item(cx, select);
    }

    /// Show a model dropdown for each step in the selected pipeline, the
    /// same way image size / music length already splat out.
    fn refresh_model_ui(&mut self, cx: &mut Cx, apply_preset_pin: bool) {
        let preset = self.current_preset_index(cx);
        let domains = PRESETS[preset].domains;
        let pin_for = |domain: &str| {
            PRESETS[preset]
                .pins
                .iter()
                .find(|(pin_domain, _)| *pin_domain == domain)
                .map(|(_, model)| *model)
        };
        let active = |domain: &str| domains.iter().any(|want| *want == domain);
        self.refresh_one_stage_model(
            cx, "text", ids!(md_text_row), ids!(md_text), active("text"), apply_preset_pin, pin_for("text"),
        );
        self.refresh_one_stage_model(
            cx, "image", ids!(md_image_row), ids!(md_image), active("image"), apply_preset_pin, pin_for("image"),
        );
        self.refresh_one_stage_model(
            cx, "audio", ids!(md_audio_row), ids!(md_audio), active("audio"), apply_preset_pin, pin_for("audio"),
        );
        self.refresh_one_stage_model(
            cx, "speech", ids!(md_speech_row), ids!(md_speech), active("speech"), apply_preset_pin, pin_for("speech"),
        );
        self.refresh_one_stage_model(
            cx, "music", ids!(md_music_row), ids!(md_music), active("music"), apply_preset_pin, pin_for("music"),
        );
        self.refresh_one_stage_model(
            cx, "video", ids!(md_video_row), ids!(md_video), active("video"), apply_preset_pin, pin_for("video"),
        );
        self.refresh_one_stage_model(
            cx, "mesh", ids!(md_mesh_row), ids!(md_mesh), active("mesh"), apply_preset_pin, pin_for("mesh"),
        );
        self.refresh_one_stage_model(
            cx, "matte", ids!(md_matte_row), ids!(md_matte), active("matte"), apply_preset_pin, pin_for("matte"),
        );
        self.refresh_one_stage_model(
            cx, "depth", ids!(md_depth_row), ids!(md_depth), active("depth"), apply_preset_pin, pin_for("depth"),
        );
        self.refresh_one_stage_model(
            cx, "segment", ids!(md_segment_row), ids!(md_segment), active("segment"), apply_preset_pin, pin_for("segment"),
        );
        self.refresh_one_stage_model(
            cx, "paint", ids!(md_paint_row), ids!(md_paint), active("paint"), apply_preset_pin, pin_for("paint"),
        );
        self.refresh_one_stage_model(
            cx, "world", ids!(md_world_row), ids!(md_world), active("world"), apply_preset_pin, pin_for("world"),
        );
        self.refresh_one_stage_model(
            cx, "rig", ids!(md_rig_row), ids!(md_rig), active("rig"), apply_preset_pin, pin_for("rig"),
        );
        self.refresh_one_stage_model(
            cx, "motion", ids!(md_motion_row), ids!(md_motion), active("motion"), apply_preset_pin, pin_for("motion"),
        );
        self.ui
            .widget(cx, ids!(speech_params_row))
            .set_visible(cx, active("speech"));
        self.ui
            .widget(cx, ids!(image_size_row))
            .set_visible(cx, active("image"));
        self.ui
            .widget(cx, ids!(image_steps_row))
            .set_visible(cx, active("image"));
        self.ui
            .widget(cx, ids!(mesh_params_row))
            .set_visible(cx, active("mesh") || active("paint"));
        self.ui
            .widget(cx, ids!(mesh_faces_row))
            .set_visible(cx, active("mesh"));
        self.ui
            .widget(cx, ids!(mesh_colors_row))
            .set_visible(cx, active("mesh") && active("paint"));
        self.ui
            .widget(cx, ids!(motion_prompt_row))
            .set_visible(cx, active("motion"));
        self.ui
            .widget(cx, ids!(vid_size_row))
            .set_visible(cx, active("video"));
        self.ui
            .widget(cx, ids!(vid_len_row))
            .set_visible(cx, active("video"));
        self.ui
            .widget(cx, ids!(music_params_row))
            .set_visible(cx, active("music"));
        self.sync_preset_name_box(cx);
    }

    fn current_panel_gen(&self, cx: &mut Cx) -> GenParams {
        let size = IMAGE_SIZES[self
            .ui
            .drop_down2(cx, ids!(size_drop))
            .selected_item()
            .min(IMAGE_SIZES.len() - 1)];
        let steps_index = self.ui.drop_down2(cx, ids!(steps_drop)).selected_item();
        let image_steps = steps_index
            .checked_sub(1)
            .and_then(|i| IMAGE_STEPS.get(i).copied());
        let mesh_texture_size = MESH_TEXTURE_SIZES[self
            .ui
            .drop_down2(cx, ids!(texture_size_drop))
            .selected_item()
            .min(MESH_TEXTURE_SIZES.len() - 1)];
        let mesh_faces_n = MESH_FACE_COUNTS[self
            .ui
            .drop_down2(cx, ids!(mesh_faces_drop))
            .selected_item()
            .min(MESH_FACE_COUNTS.len() - 1)];
        let mesh_faces = (mesh_faces_n != 0).then_some(mesh_faces_n);
        let vid_size = VIDEO_SIZES[self
            .ui
            .drop_down2(cx, ids!(vid_size_drop))
            .selected_item()
            .min(VIDEO_SIZES.len() - 1)];
        let (video_frames, video_steps) = VIDEO_LENGTHS[self
            .ui
            .drop_down2(cx, ids!(vid_len_drop))
            .selected_item()
            .min(VIDEO_LENGTHS.len() - 1)];
        let music_seconds = MUSIC_LENGTHS[self
            .ui
            .drop_down2(cx, ids!(music_len_drop))
            .selected_item()
            .min(MUSIC_LENGTHS.len() - 1)];
        GenParams {
            image_size: size,
            image_steps,
            mesh_texture_size,
            mesh_faces,
            mesh_trellis_texture: self.ui.check_box(cx, ids!(trellis_colors_toggle)).active(cx),
            motion_prompt: self.ui.text_input(cx, ids!(motion_prompt_input)).text(),
            video_size: vid_size,
            video_frames,
            video_steps,
            music_seconds,
        }
    }

    fn sync_preset_name_box(&mut self, cx: &mut Cx) {
        let preset = self.current_preset_index(cx);
        let models = self.collected_stage_models(cx, PRESETS[preset].domains);
        let gen = self.current_panel_gen(cx);
        let name = fast_presets::auto_name(PRESETS[preset].name, &models, &gen);
        self.ui
            .text_input(cx, ids!(preset_name_input))
            .set_text(cx, &name);
    }

    fn persist_saved_presets(&self) {
        if let Err(error) = fast_presets::save(&fast_presets::store_path(), &self.saved_presets) {
            log!("fast preset save failed: {error}");
        }
    }

    fn refresh_saved_presets_ui(&mut self, cx: &mut Cx) {
        let slots = [
            (ids!(fp0), ids!(fp0_go), ids!(fp0_del)),
            (ids!(fp1), ids!(fp1_go), ids!(fp1_del)),
            (ids!(fp2), ids!(fp2_go), ids!(fp2_del)),
            (ids!(fp3), ids!(fp3_go), ids!(fp3_del)),
            (ids!(fp4), ids!(fp4_go), ids!(fp4_del)),
            (ids!(fp5), ids!(fp5_go), ids!(fp5_del)),
            (ids!(fp6), ids!(fp6_go), ids!(fp6_del)),
            (ids!(fp7), ids!(fp7_go), ids!(fp7_del)),
        ];
        for (i, (row, go, _)) in slots.iter().enumerate() {
            if let Some(saved) = self.saved_presets.get(i) {
                self.ui.widget(cx, *row).set_visible(cx, true);
                self.ui.button(cx, *go).set_text(cx, &saved.name);
            } else {
                self.ui.widget(cx, *row).set_visible(cx, false);
            }
        }
    }

    fn save_current_preset(&mut self, cx: &mut Cx) {
        if self.saved_presets.len() >= MAX_FAST_PRESETS {
            self.set_caption(cx, "PRESET", "delete one first (8 max)");
            return;
        }
        let preset = self.current_preset_index(cx);
        let models = self.collected_stage_models(cx, PRESETS[preset].domains);
        let gen = self.current_panel_gen(cx);
        let typed = self.ui.text_input(cx, ids!(preset_name_input)).text();
        let name = {
            let trimmed = typed.trim();
            if trimmed.is_empty() {
                fast_presets::auto_name(PRESETS[preset].name, &models, &gen)
            } else {
                trimmed.to_string()
            }
        };
        let voice_index = self.ui.drop_down2(cx, ids!(voice_drop)).selected_item();
        let voice = if voice_index == 0 || !self.voice_drop_is_kokoro {
            None
        } else {
            VOICES.get(voice_index).map(|v| v.to_string())
        };
        self.saved_presets.push(fast_presets::snapshot(
            PRESETS[preset].name,
            models,
            voice,
            &gen,
            name,
        ));
        self.persist_saved_presets();
        self.refresh_saved_presets_ui(cx);
        self.sync_preset_name_box(cx);
    }

    fn apply_saved_preset(&mut self, cx: &mut Cx, index: usize) {
        let Some(saved) = self.saved_presets.get(index).cloned() else {
            return;
        };
        let Some(preset) = fast_presets::pipeline_index(&saved.pipeline) else {
            self.set_caption(cx, "PRESET", &format!("missing pipeline {}", saved.pipeline));
            return;
        };
        self.ui
            .drop_down2(cx, ids!(preset_drop))
            .set_selected_item(cx, crate::pipeline::preset_row_for_index(preset));
        self.refresh_model_ui(cx, false);
        for pin in &saved.models {
            let ids = self.fleet_models_for_domain(&pin.domain);
            if let Some(pos) = ids.iter().position(|id| id == &pin.model) {
                if let Some(drop) = Self::stage_model_drop_id(&pin.domain) {
                    self.ui
                        .drop_down2(cx, drop)
                        .set_selected_item(cx, pos + 1);
                }
            }
        }
        self.ui
            .drop_down2(cx, ids!(size_drop))
            .set_selected_item(cx, fast_presets::nearest_image_size(saved.image_w, saved.image_h));
        self.ui.drop_down2(cx, ids!(steps_drop)).set_selected_item(
            cx,
            fast_presets::nearest_image_steps(saved.image_steps),
        );
        self.ui.drop_down2(cx, ids!(texture_size_drop)).set_selected_item(
            cx,
            fast_presets::nearest_mesh_texture(saved.mesh_texture),
        );
        self.ui.drop_down2(cx, ids!(mesh_faces_drop)).set_selected_item(
            cx,
            fast_presets::nearest_mesh_faces(saved.mesh_faces),
        );
        self.ui
            .check_box(cx, ids!(trellis_colors_toggle))
            .set_active(cx, saved.mesh_trellis_texture.unwrap_or(false), Animate::No);
        self.ui
            .text_input(cx, ids!(motion_prompt_input))
            .set_text(cx, saved.motion_prompt.as_deref().unwrap_or(""));
        self.ui.drop_down2(cx, ids!(vid_size_drop)).set_selected_item(
            cx,
            fast_presets::nearest_video_size(saved.video_w, saved.video_h),
        );
        self.ui.drop_down2(cx, ids!(vid_len_drop)).set_selected_item(
            cx,
            fast_presets::nearest_video_len(saved.video_frames, saved.video_steps),
        );
        self.ui.drop_down2(cx, ids!(music_len_drop)).set_selected_item(
            cx,
            fast_presets::nearest_music_len(saved.music_seconds),
        );
        self.refresh_voice_ui(cx);
        self.sync_preset_name_box(cx);
        self.start_generate(cx);
    }

    fn stage_model_drop_id(domain: &str) -> Option<&'static [LiveId]> {
        Some(match domain {
            "text" => ids!(md_text),
            "image" => ids!(md_image),
            "audio" => ids!(md_audio),
            "speech" => ids!(md_speech),
            "music" => ids!(md_music),
            "video" => ids!(md_video),
            "mesh" => ids!(md_mesh),
            "matte" => ids!(md_matte),
            "depth" => ids!(md_depth),
            "segment" => ids!(md_segment),
            "paint" => ids!(md_paint),
            "world" => ids!(md_world),
            "rig" => ids!(md_rig),
            "motion" => ids!(md_motion),
            _ => return None,
        })
    }

    /// The speech model the NEXT run would use: an explicit speech model
    /// override wins, else the selected preset's speech pin, else Kokoro by
    /// affinity convention (the only pack-based backend).
    fn effective_speech_model(&mut self, cx: &mut Cx) -> Option<String> {
        if let Some(model) = self.selected_stage_model(cx, "speech") {
            return Some(model);
        }
        let preset = self.current_preset_index(cx);
        PRESETS[preset]
            .pins
            .iter()
            .find(|(domain, _)| *domain == "speech")
            .map(|(_, model)| model.to_string())
    }

    /// Voice packs are Kokoro's. When the effective speech model is another
    /// backend (IndexTTS 2.5), the dropdown says so honestly instead of
    /// offering packs that backend cannot use; the run spec then carries no
    /// voice (see also the pipeline-side guard).
    fn refresh_voice_ui(&mut self, cx: &mut Cx) {
        let speech_model = self.effective_speech_model(cx);
        let kokoro = speech_model.as_deref().map_or(true, |model| model == "kokoro");
        if kokoro == self.voice_drop_is_kokoro {
            return;
        }
        self.voice_drop_is_kokoro = kokoro;
        let drop = self.ui.drop_down2(cx, ids!(voice_drop));
        if kokoro {
            drop.set_labels(
                cx,
                std::iter::once(format!("default ({})", VOICES[0]))
                    .chain(VOICES[1..].iter().map(|voice| (*voice).to_string()))
                    .collect(),
            );
        } else {
            let model = speech_model.unwrap_or_default();
            drop.set_labels(
                cx,
                vec![format!(
                    "n/a — {model} uses reference audio + emotion (not wired here yet)"
                )],
            );
        }
        drop.set_selected_item(cx, 0);
    }

    /// Headless drive: fire the AUTO pipeline once its first stage's domain
    /// has a live box.
    fn maybe_fire_auto(&mut self, cx: &mut Cx) {
        if self.auto.fired {
            return;
        }
        let Some(preset_sub) = self.auto.preset.clone() else { return };
        let Some(fleet) = &self.fleet else { return };
        let Some(preset_index) = PRESETS
            .iter()
            .position(|p| p.name.contains(preset_sub.as_str()))
        else {
            log!("auto: no preset matches {preset_sub:?}");
            self.auto.fired = true;
            return;
        };
        let first_domain = PRESETS[preset_index].domains[0];
        if makepad_asset_ai::fleet::pick_for_domain(&fleet.snapshots, first_domain).is_none() {
            return; // wait for discovery
        }
        self.auto.fired = true;
        if let Some(prompt) = &self.auto.prompt {
            self.ui
                .text_input(cx, ids!(prompt_input))
                .set_text(cx, prompt);
        }
        self.ui
            .drop_down2(cx, ids!(preset_drop))
            .set_selected_item(cx, crate::pipeline::preset_row_for_index(preset_index));
        self.refresh_model_ui(cx, true);
        // Extra queued runs, to exercise the run queue. Each is its own
        // History group, exactly like distinct Generate clicks.
        for sub in self.auto.queue.clone() {
            if let Some(preset) = PRESETS.iter().position(|p| p.name.contains(sub.as_str())) {
                let prompt = self
                    .auto
                    .prompt
                    .clone()
                    .unwrap_or_else(|| "queued demo run".to_string());
                let group_label = format!(
                    "{} — \"{}\"",
                    PRESETS[preset].name,
                    truncate(prompt.trim(), 24)
                );
                self.run_queue.push(PendingRun {
                    group_id: crate::library::new_group_id("run"),
                    group_label,
                    prompt,
                    preset,
                    model_overrides: Vec::new(),
                    box_override: None,
                    voice: None,
                    gen: GenParams::default(),
                    input: None,
                });
            }
        }
        log!("auto: firing preset {:?}", PRESETS[preset_index].name);
        self.start_generate(cx);
    }

    // -- run queue --------------------------------------------------------------

    /// Build the run the Generate/quick-action click means. Errors are
    /// honest refusals (an unreadable selected input never silently falls
    /// back to a generate-from-prompt chain) — the caller surfaces them
    /// without queueing anything.
    fn current_run_spec(&mut self, cx: &mut Cx) -> Result<PendingRun, String> {
        let mut prompt = self.ui.text_input(cx, ids!(prompt_input)).text();
        if prompt.trim().is_empty() {
            prompt = "a weathered fishing trawler at dawn, misty harbor".to_string();
        }
        let preset = self.current_preset_index(cx);
        let model_overrides =
            self.collected_stage_models(cx, PRESETS[preset].domains);
        let box_index = self.ui.drop_down2(cx, ids!(box_drop)).selected_item();
        let box_override = if box_index == 0 {
            None
        } else {
            self.box_choices.get(box_index - 1).cloned()
        };
        let voice_index = self.ui.drop_down2(cx, ids!(voice_drop)).selected_item();
        // No pack when the dropdown is in its non-Kokoro (n/a) state.
        let voice = if voice_index == 0 || !self.voice_drop_is_kokoro {
            None
        } else {
            VOICES.get(voice_index).map(|v| v.to_string())
        };
        let size = IMAGE_SIZES
            [self.ui.drop_down2(cx, ids!(size_drop)).selected_item().min(IMAGE_SIZES.len() - 1)];
        let steps_index = self.ui.drop_down2(cx, ids!(steps_drop)).selected_item();
        let image_steps = steps_index
            .checked_sub(1)
            .and_then(|i| IMAGE_STEPS.get(i).copied());
        let mesh_texture_size = MESH_TEXTURE_SIZES[self
            .ui
            .drop_down2(cx, ids!(texture_size_drop))
            .selected_item()
            .min(MESH_TEXTURE_SIZES.len() - 1)];
        let mesh_faces_n = MESH_FACE_COUNTS[self
            .ui
            .drop_down2(cx, ids!(mesh_faces_drop))
            .selected_item()
            .min(MESH_FACE_COUNTS.len() - 1)];
        let mesh_faces = (mesh_faces_n != 0).then_some(mesh_faces_n);
        let vid_size = VIDEO_SIZES[self
            .ui
            .drop_down2(cx, ids!(vid_size_drop))
            .selected_item()
            .min(VIDEO_SIZES.len() - 1)];
        let (video_frames, video_steps) = VIDEO_LENGTHS[self
            .ui
            .drop_down2(cx, ids!(vid_len_drop))
            .selected_item()
            .min(VIDEO_LENGTHS.len() - 1)];
        let music_seconds = MUSIC_LENGTHS[self
            .ui
            .drop_down2(cx, ids!(music_len_drop))
            .selected_item()
            .min(MUSIC_LENGTHS.len() - 1)];
        // Seeded transform: a compatible pinned input becomes the run's
        // immutable byte snapshot; an incompatible one is called out and
        // the preset generates as labeled. Read failures REFUSE the run.
        let input = match self.input_tray.current().cloned() {
            Some(asset) => {
                match seed_replaces_prefix(PRESETS[preset].domains, &asset.content_type) {
                    Some(skip) => {
                        let bytes = std::fs::read(&asset.path).map_err(|error| {
                            format!(
                                "selected input \u{201c}{}\u{201d} could not be read ({error}) — run not queued",
                                asset.label
                            )
                        })?;
                        Some(RunSeed {
                            source_file: asset.file.clone(),
                            source_label: asset.label.clone(),
                            content_type: asset.content_type.clone(),
                            bytes: std::sync::Arc::new(bytes),
                            skip,
                        })
                    }
                    None => {
                        // Concise requirement hint, never a silent
                        // substitution: this preset does not consume the
                        // pinned kind, so it generates from the prompt.
                        self.set_caption(
                            cx,
                            "INPUT",
                            &format!(
                                "selected {} isn't used by \u{201c}{}\u{201d} — generating from the prompt",
                                crate::asset_store_state::local_kind(
                                    &asset.domain,
                                    &asset.content_type
                                ),
                                PRESETS[preset].name
                            ),
                        );
                        None
                    }
                }
            }
            None => None,
        };
        // A chain whose first stage CONSUMES an input it cannot make itself
        // (mesh-first rig chains: TRELLIS needs an image, and the only thing
        // that can stand in is a selected GLB) is refused without a seed —
        // the stage would just fail on the box with "needs an input image".
        let first = PRESETS[preset].domains.first().copied().unwrap_or("");
        if input.is_none() && (first == "mesh" || consumer_only_domain(first)) {
            let needs = if first == "mesh" { "mesh" } else { "image" };
            return Err(format!(
                "\u{201c}{}\u{201d} needs a selected {needs} — click one in History or a run-tray chip first",
                PRESETS[preset].name
            ));
        }
        let group_label = match &input {
            // Provenance in the run's durable group identity: this chain
            // was seeded FROM that exact managed artifact.
            Some(seed) => format!(
                "{} — from \u{201c}{}\u{201d}",
                PRESETS[preset].name,
                truncate(&seed.source_label, 20)
            ),
            None => format!(
                "{} — \"{}\"",
                PRESETS[preset].name,
                truncate(prompt.trim(), 24)
            ),
        };
        Ok(PendingRun {
            group_id: crate::library::new_group_id("run"),
            group_label,
            prompt,
            preset,
            model_overrides,
            box_override,
            voice,
            gen: GenParams {
                image_size: size,
                image_steps,
                mesh_texture_size,
                mesh_faces,
                mesh_trellis_texture: self.ui.check_box(cx, ids!(trellis_colors_toggle)).active(cx),
                motion_prompt: self.ui.text_input(cx, ids!(motion_prompt_input)).text(),
                video_size: vid_size,
                video_frames,
                video_steps,
                music_seconds,
            },
            input,
        })
    }

    /// Pull the pinned model onto the pinned box (a `pull_only` job:
    /// download/verify, then done — no generation). Needs both pins: pull is
    /// about seeding a SPECIFIC box, exactly the case affinity won't route.
    /// Progress shows on the box's fleet entry (model state + queue).
    fn pull_model(&mut self, cx: &mut Cx) {
        let preset = self.current_preset_index(cx);
        let Some((_, model)) = self
            .collected_stage_models(cx, PRESETS[preset].domains)
            .into_iter()
            .next()
        else {
            self.set_caption(cx, "PULL", "pick a model in a stage dropdown first");
            return;
        };
        let box_index = self.ui.drop_down2(cx, ids!(box_drop)).selected_item();
        let Some(box_url) = box_index
            .checked_sub(1)
            .and_then(|i| self.box_choices.get(i))
        else {
            self.set_caption(cx, "PULL", "pick a box in the box pin first");
            return;
        };
        let request = makepad_asset_ai::protocol::GenerateRequestJson {
            model: model.clone(),
            pull_only: Some(true),
            queue_policy: Some("queue".to_string()),
            ..Default::default()
        };
        let mut http = crate::http::request(format!("{box_url}/generate"), HttpMethod::POST);
        http.set_header("Content-Type".to_string(), "application/json".to_string());
        http.set_body(request.serialize_json().into_bytes());
        cx.http_request(LiveId::unique(), http);
        log!("pull: {model} -> {box_url}");
        self.set_caption(
            cx,
            "PULL",
            &format!("{model} downloading to {box_url} — watch its fleet entry"),
        );
    }

    /// Generate: every click enqueues its own run (own group id); the
    /// fleet-aware planner starts as many as free compatible slots allow.
    fn start_generate(&mut self, cx: &mut Cx) {
        let parallel = self.ui.check_box(cx, ids!(parallel_toggle)).active(cx);
        match self.current_run_spec(cx) {
            Ok(run) if parallel => {
                let boxes = self.idle_capable_boxes(&run);
                if boxes.is_empty() {
                    self.set_caption(
                        cx,
                        "FLEET",
                        "no idle capable box right now — queued one run by affinity",
                    );
                    self.run_queue.push(run);
                } else {
                    let count = boxes.len();
                    for base_url in boxes {
                        // Own group id per box: each spread run is its own
                        // History item; the label says where it ran.
                        let host = base_url
                            .trim_start_matches("http://")
                            .trim_start_matches("https://")
                            .to_string();
                        self.run_queue.push(PendingRun {
                            box_override: Some(base_url),
                            group_id: crate::library::new_group_id("run"),
                            group_label: format!("{} @ {host}", run.group_label),
                            ..run.clone()
                        });
                    }
                    self.set_caption(cx, "FLEET", &format!("spread across {count} idle boxes"));
                }
                self.try_dispatch_pending(cx);
            }
            Ok(run) => {
                self.run_queue.push(run);
                self.try_dispatch_pending(cx);
            }
            Err(message) => {
                log!("input: {message}");
                self.set_caption(cx, "INPUT", &message);
            }
        }
    }

    /// Boxes that could take `run`'s first stage RIGHT NOW and are doing
    /// nothing else: up, capable (advertised model + VRAM fit), zero
    /// service-reported jobs (nobody's video in flight), none of our own
    /// stages committed there. One per physical GPU slot.
    fn idle_capable_boxes(&self, run: &PendingRun) -> Vec<String> {
        let (_, loads) = self.endpoint_loads(run);
        let mut seen_slots = Vec::new();
        let mut out = Vec::new();
        for load in loads {
            if !load.up || !load.capable || load.vram_waiting {
                continue;
            }
            if load.reported_pending > 0 || load.ours_active > 0 {
                continue;
            }
            if seen_slots.contains(&load.slot_key) {
                continue;
            }
            seen_slots.push(load.slot_key.clone());
            out.push(load.base_url.clone());
        }
        out
    }

    /// Runs currently occupying a GPU slot.
    fn active_run_count(&self) -> usize {
        self.runs
            .iter()
            .filter(|run| run.pipeline.is_running())
            .count()
    }

    fn any_run_running(&self) -> bool {
        self.runs.iter().any(|run| run.pipeline.is_running())
    }

    /// Endpoint occupancy from OUR runs: url → committed current stages.
    fn our_endpoint_use(&self) -> Vec<(String, u32)> {
        let mut use_counts: Vec<(String, u32)> = Vec::new();
        for run in &self.runs {
            for url in run.pipeline.active_boxes() {
                match use_counts.iter_mut().find(|(existing, _)| existing == url) {
                    Some((_, count)) => *count += 1,
                    None => use_counts.push((url.to_string(), 1)),
                }
            }
        }
        use_counts
    }

    /// Honest per-endpoint load picture for one prospective run's FIRST
    /// stage: capability via the same affinity scorer the router uses,
    /// busy from reported queue depth + our committed stages.
    fn endpoint_loads(&self, run: &PendingRun) -> (String, Vec<EndpointLoad>) {
        // Seeded runs plan for their ACTUAL first stage (e.g. mesh), not
        // the producer stage the pinned input replaced.
        let domain = run
            .domains()
            .first()
            .copied()
            .unwrap_or("image")
            .to_string();
        let pinned_model = run
            .model_overrides
            .iter()
            .find(|(override_domain, _)| override_domain == &domain)
            .map(|(_, model)| model.clone())
            .or_else(|| {
                PRESETS[run.preset]
                    .pins
                    .iter()
                    .find(|(pin_domain, _)| *pin_domain == domain)
                    .map(|(_, model)| (*model).to_string())
            });
        let ours = self.our_endpoint_use();
        let loads = self
            .fleet
            .as_ref()
            .map(|fleet| fleet.snapshots.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter(|snapshot| {
                run.box_override
                    .as_deref()
                    .map_or(true, |pin| pin == snapshot.base_url)
            })
            .map(|snapshot| {
                // Hardware fit and transient free-VRAM pressure come from
                // the exact same advertised model facts used by the router.
                // A big-enough occupied GPU remains a capable queue target;
                // a physically undersized GPU does not.
                let admission = match &pinned_model {
                    Some(model) => makepad_asset_ai::fleet::model_admission(snapshot, model),
                    None => makepad_asset_ai::fleet::domain_admission(snapshot, &domain),
                };
                let capable = admission.is_some_and(|state| state.is_hardware_compatible());
                let vram_waiting = admission.is_some_and(|state| state.is_waiting());
                EndpointLoad {
                    base_url: snapshot.base_url.clone(),
                    // Deployment invariant: ONE service process per PC;
                    // extra GPUs arrive as slots advertised by that one
                    // service, never as extra ports. Host bucketing defends
                    // against stale/duplicate entries anyway. node_key is
                    // per SERVICE INSTANCE and must never be the key.
                    slot_key: crate::scheduler::slot_key(&snapshot.base_url, None),
                    up: snapshot.is_up(),
                    capable,
                    vram_waiting,
                    reported_pending: snapshot.jobs_pending(),
                    ours_active: ours
                        .iter()
                        .find(|(url, _)| *url == snapshot.base_url)
                        .map_or(0, |(_, count)| *count),
                    // The service does not advertise a GPU count; never
                    // assume more than one per slot.
                    capacity: 1,
                }
            })
            .collect();
        (domain, loads)
    }

    /// Endpoints OTHER runs' current stages occupy — expanded to every
    /// sibling port on the same GPU slot, so a mid-chain stage can never
    /// double-book a busy card through its second service.
    fn avoid_for_run(&self, run_index: usize) -> Vec<String> {
        let occupied: Vec<String> = self
            .runs
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != run_index)
            .flat_map(|(_, run)| {
                run.pipeline
                    .active_boxes()
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect();
        if occupied.is_empty() {
            return occupied;
        }
        let pairs: Vec<(String, String)> = self
            .fleet
            .as_ref()
            .map(|fleet| fleet.snapshots.as_slice())
            .unwrap_or(&[])
            .iter()
            .map(|snapshot| {
                (
                    snapshot.base_url.clone(),
                    crate::scheduler::slot_key(&snapshot.base_url, None),
                )
            })
            .collect();
        crate::scheduler::expand_occupied(&pairs, &occupied)
    }

    /// Start every queued run that has a free compatible slot RIGHT NOW.
    /// FIFO with skip-ahead: a run waiting on busy video slots never blocks
    /// a mesh run behind it whose slot is free. Bounded by
    /// [`MAX_ACTIVE_RUNS`] on top of per-endpoint capacity.
    fn try_dispatch_pending(&mut self, cx: &mut Cx) {
        let mut index = 0;
        while index < self.run_queue.len() {
            if self.active_run_count() >= MAX_ACTIVE_RUNS {
                break;
            }
            let run = self.run_queue[index].clone();
            let (domain, loads) = self.endpoint_loads(&run);
            match plan_run(&domain, &loads) {
                DispatchPlan::Start { avoid } => {
                    self.run_queue.remove(index);
                    self.dispatch_run(cx, run, &avoid);
                }
                DispatchPlan::NoCapability => {
                    // Dispatch anyway: the stage fails visibly as a service
                    // gap instead of silently parking in the queue.
                    self.run_queue.remove(index);
                    self.dispatch_run(cx, run, &[]);
                }
                DispatchPlan::Wait { reason } => {
                    log!("scheduler: run held — {reason}");
                    index += 1;
                }
            }
        }
        self.refresh_run_ui(cx);
    }

    fn dispatch_run(&mut self, cx: &mut Cx, run: PendingRun, avoid: &[String]) {
        let Some(fleet) = &self.fleet else { return };
        let snapshots = fleet.snapshots.clone();
        let mut pipeline = Pipeline::new(
            &run.prompt,
            run.domains(),
            PRESETS[run.preset].pins,
            run.model_overrides.clone(),
            run.box_override.clone(),
            run.voice.clone(),
            run.gen.clone(),
        );
        let skip = run.input.as_ref().map_or(0, |seed| seed.skip);
        if let Some(seed) = &run.input {
            // Durable provenance line: which exact managed artifact seeded
            // this new chain/group.
            log!(
                "run: {} seeded from {} ({}, {} bytes)",
                run.group_id,
                seed.source_file,
                seed.content_type,
                seed.bytes.len()
            );
            // Kind compatibility was validated at spec time; a rejection
            // here would mean the seeded chain silently regenerating, so
            // fail the whole dispatch honestly instead.
            if let Err(error) =
                pipeline.set_seed_input(seed.content_type.clone(), seed.bytes.as_ref().clone())
            {
                log!("run: seed input rejected at dispatch: {error}");
                self.set_caption(cx, "INPUT", &format!("run not started: {error}"));
                return;
            }
        }
        if let Some(stage) = PRESETS[run.preset].fan_out_stage {
            if stage >= skip {
                pipeline
                    .enable_fan_out(stage - skip, format!("{}:candidates", run.group_id))
                    .expect("built-in fan-out preset must describe an image stage");
            }
            // else: the pinned input replaced the fan-out image stage — the
            // user already chose the exact image, so there is no candidate
            // gate to run.
        }
        let events = pipeline.start(cx, &snapshots, avoid);
        self.next_run_id += 1;
        let run_id = self.next_run_id;
        self.runs.push(ActiveRun {
            id: run_id,
            group_id: run.group_id.clone(),
            group_label: run.group_label.clone(),
            prompt: run.prompt.clone(),
            pipeline,
        });
        self.last_run = Some(run);
        self.prune_finished_runs();
        self.on_run_events(cx, run_id, events);
    }

    /// Keep every running run plus the newest few finished ones for the
    /// Runs surface; older finished runs drop (their artifacts live on in
    /// History).
    fn prune_finished_runs(&mut self) {
        let mut finished_seen = 0usize;
        let mut keep: Vec<bool> = self
            .runs
            .iter()
            .rev()
            .map(|run| {
                if run.pipeline.is_running() {
                    true
                } else {
                    finished_seen += 1;
                    finished_seen <= MAX_FINISHED_RUNS_SHOWN
                }
            })
            .collect();
        keep.reverse();
        let mut keep_iter = keep.into_iter();
        self.runs.retain(|_| keep_iter.next().unwrap_or(true));
    }

    /// The run the NOW card and the left stage strip mirror: the newest
    /// running run, else the newest finished one. Every OTHER active run is
    /// fully visible on the Runs surface with its own progress and Stop.
    fn display_run(&self) -> Option<&ActiveRun> {
        self.runs
            .iter()
            .rev()
            .find(|run| run.pipeline.is_running())
            .or_else(|| self.runs.last())
    }

    /// NOW card + queue rows + spinner + retry visibility.
    fn refresh_run_ui(&mut self, cx: &mut Cx) {
        let active_running = self.any_run_running();
        let concurrent = self.active_run_count();
        self.ui
            .widget(cx, ids!(spinner))
            .set_visible(cx, active_running);
        let display_pipeline = self.display_run().map(|run| &run.pipeline);
        let failed = display_pipeline.is_some_and(|p| {
            p.stages
                .iter()
                .any(|s| matches!(s.state, StageState::Failed(_)))
        });
        self.ui
            .widget(cx, ids!(retry_btn))
            .set_visible(cx, failed && !active_running && self.last_run.is_some());

        // NOW card: the newest active stage front and center — what's
        // running, where, its live detail, and a real bar. Concurrent runs
        // are counted here and itemized on the Runs surface.
        let (head, detail, fraction) = match display_pipeline {
            Some(p) if p.is_running() => {
                let s = &p.stages[p.current];
                let stage_name = if s.domain == "music" {
                    format!(
                        "{} ({} target)",
                        stage_display_name(&s.domain),
                        format_music_duration(p.gen.music_seconds),
                    )
                } else {
                    stage_display_name(&s.domain).to_string()
                };
                let where_ = if s.box_url.is_empty() {
                    String::new()
                } else {
                    format!(
                        " — {} @ {}",
                        s.model,
                        s.box_url.trim_start_matches("http://")
                    )
                };
                let elapsed_s = s.started.map(|t0| t0.elapsed().as_secs_f64()).unwrap_or(0.0);
                let frac = s.progress.clamp(0.0, 1.0);
                let eta = if frac > 0.02 && frac < 0.999 && elapsed_s > 1.0 {
                    let left = elapsed_s * (1.0 - frac) / frac;
                    format!(" · ~{} left", format_clock(left))
                } else {
                    String::new()
                };
                let elapsed = if elapsed_s > 0.0 {
                    format!(" · {}", format_clock(elapsed_s))
                } else {
                    String::new()
                };
                let state = match &s.state {
                    StageState::Waiting => "waiting".to_string(),
                    StageState::FanOut => s.detail.clone(),
                    StageState::AwaitingChoice => s.detail.clone(),
                    StageState::Submitting => "submitting…".to_string(),
                    StageState::Polling => s.detail.clone(),
                    StageState::Fetching => "fetching artifacts…".to_string(),
                    StageState::Done => "done".to_string(),
                    StageState::Failed(e) => format!("FAILED: {e}"),
                };
                let others = if concurrent > 1 {
                    format!(" · {concurrent} runs live", )
                } else {
                    String::new()
                };
                (
                    format!(
                        "Stage {}/{} · {} · {:>5.1}%{}{}{}{others}",
                        p.current + 1,
                        p.stages.len(),
                        stage_name,
                        frac * 100.0,
                        where_,
                        elapsed,
                        eta
                    ),
                    state,
                    frac,
                )
            }
            Some(p) if failed => {
                let e = p
                    .stages
                    .iter()
                    .find_map(|s| match &s.state {
                        StageState::Failed(e) => Some(e.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                ("Failed".to_string(), format!("FAILED: {e}"), 0.0)
            }
            Some(p) => {
                let total: f64 = p
                    .stages
                    .iter()
                    .filter_map(|s| match (s.started, s.finished) {
                        (Some(t0), Some(t1)) => Some((t1 - t0).as_secs_f64()),
                        _ => None,
                    })
                    .sum();
                (format!("Done in {total:.1}s"), String::new(), 1.0)
            }
            None => ("Idle — nothing running".to_string(), String::new(), 0.0),
        };
        self.ui.label(cx, ids!(now_head)).set_text(cx, &head);
        self.ui.label(cx, ids!(now_detail)).set_text(cx, &detail);
        self.ui
            .widget(cx, ids!(cancel_btn))
            .set_visible(cx, active_running);
        self.ui
            .view(cx, ids!(now_bar))
            .set_uniform(cx, live_id!(progress), &[fraction as f32]);

        // Queue rows: waiting runs only — the active run lives in the card.
        let rows = [
            (ids!(q1_row), ids!(q1_label)),
            (ids!(q2_row), ids!(q2_label)),
            (ids!(q3_row), ids!(q3_label)),
            (ids!(q4_row), ids!(q4_label)),
            (ids!(q5_row), ids!(q5_label)),
            (ids!(q6_row), ids!(q6_label)),
        ];
        let texts: Vec<String> = self
            .run_queue
            .iter()
            .map(|run| {
                format!(
                    "{} — \"{}\"",
                    PRESETS[run.preset].name,
                    truncate(&run.prompt, 28)
                )
            })
            .collect();
        for (k, (row, label)) in rows.iter().enumerate() {
            let visible = k < texts.len();
            self.ui.widget(cx, *row).set_visible(cx, visible);
            if visible {
                self.ui.label(cx, *label).set_text(cx, &texts[k]);
            }
        }
        if self.surface == Surface::Runs {
            self.refresh_runs_panel(cx);
        }
        self.ui.redraw(cx);
    }

    fn cancel_row(&mut self, cx: &mut Cx, row: usize) {
        if row < self.run_queue.len() {
            self.run_queue.remove(row);
        }
        self.refresh_run_ui(cx);
    }

    /// Stop button on the NOW card: cancels the run the card is showing
    /// (the newest running one). Queued service jobs drop immediately;
    /// running jobs raise the cancel flag and unwind within seconds.
    fn cancel_active(&mut self, cx: &mut Cx) {
        let newest_running = self
            .runs
            .iter()
            .rev()
            .find(|run| run.pipeline.is_running())
            .map(|run| run.id);
        if let Some(run_id) = newest_running {
            self.cancel_run(cx, run_id);
        } else {
            self.refresh_run_ui(cx);
        }
    }

    /// Per-run Stop from the Runs surface — each concurrent run cancels
    /// independently; the others keep their slots and progress.
    fn cancel_run(&mut self, cx: &mut Cx, run_id: u64) {
        if let Some(run) = self.runs.iter_mut().find(|run| run.id == run_id) {
            if run.pipeline.cancel_current(cx) {
                log!("cancel: requested for run {run_id} ({})", run.group_label);
            } else {
                log!("cancel: run {run_id} has no cancellable job");
            }
        }
        self.refresh_run_ui(cx);
    }

    fn move_row_up(&mut self, cx: &mut Cx, row: usize) {
        if row > 0 && row < self.run_queue.len() {
            self.run_queue.swap(row, row - 1);
        }
        self.refresh_run_ui(cx);
    }

    // -- pipeline --------------------------------------------------------------

    fn install_candidate_texture(&mut self, cx: &mut Cx, candidate_id: &str, bytes: &[u8]) {
        let Ok(image) = decode_image_from_data(bytes) else {
            return;
        };
        let texture = image.into_new_texture(cx);
        if let Some(mut sheet) = self
            .ui
            .widget(cx, ids!(candidate_sheet))
            .borrow_mut::<CandidateSheet>()
        {
            sheet.install_texture(cx, candidate_id.to_string(), texture);
        }
    }

    fn refresh_candidate_ui(&mut self, cx: &mut Cx, run_id: u64) {
        if self.display_run().map(|run| run.id) != Some(run_id) {
            return;
        }
        let Some((cards, state, selected, chosen, preview, ready, failed, total)) = self
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .and_then(|run| {
                let set = run.pipeline.candidate_sets.last()?;
                let ready = set
                    .candidates
                    .iter()
                    .filter(|candidate| candidate.state == StageState::Done)
                    .count();
                let failed = set
                    .candidates
                    .iter()
                    .filter(|candidate| matches!(candidate.state, StageState::Failed(_)))
                    .count();
                let preview_candidate = set
                    .selected
                    .as_deref()
                    .and_then(|selected| {
                        set.candidates
                            .iter()
                            .find(|candidate| candidate.id == selected)
                    })
                    .or_else(|| {
                        set.candidates
                            .iter()
                            .find(|candidate| candidate.state == StageState::Done)
                    });
                let preview = preview_candidate.and_then(|candidate| {
                    candidate
                        .image_output()
                        .map(|artifact| (candidate.id.clone(), artifact.bytes.clone()))
                });
                Some((
                    candidate_cards(set),
                    set.state,
                    set.selected.clone(),
                    set.chosen.clone(),
                    preview,
                    ready,
                    failed,
                    set.candidates.len(),
                ))
            })
        else {
            return;
        };

        if let Some(mut sheet) = self
            .ui
            .widget(cx, ids!(candidate_sheet))
            .borrow_mut::<CandidateSheet>()
        {
            sheet.set_cards(cx, cards);
        }
        let heading = match state {
            CandidateSetState::FanOut => format!(
                "Fleet fan-out · {ready}/{total} ready · {failed} failed"
            ),
            CandidateSetState::Cancelling => format!(
                "Stopping fleet fan-out · {} candidate requests settling",
                total.saturating_sub(ready + failed)
            ),
            CandidateSetState::EarlyChoiceCancelling => format!(
                "Choice locked · cancelling {} unfinished candidates",
                total.saturating_sub(ready + failed)
            ),
            CandidateSetState::AwaitingChoice => format!(
                "Choose one image · {ready} ready{}",
                if failed > 0 {
                    format!(" · {failed} failed")
                } else {
                    String::new()
                }
            ),
            CandidateSetState::ChoiceCommitted => format!(
                "Choice committed · video input locked to {}",
                chosen.as_deref().unwrap_or("selected candidate")
            ),
        };
        self.ui.label(cx, ids!(choice_head)).set_text(cx, &heading);
        self.ui
            .button(cx, ids!(continue_choice_btn))
            .set_enabled(
                cx,
                state == CandidateSetState::AwaitingChoice && selected.is_some(),
            );
        self.ui
            .button(cx, ids!(continue_early_btn))
            .set_visible(
                cx,
                state == CandidateSetState::FanOut && selected.is_some(),
            );
        self.ui
            .button(cx, ids!(continue_early_btn))
            .set_enabled(
                cx,
                state == CandidateSetState::FanOut && selected.is_some(),
            );
        self.ui
            .button(cx, ids!(retry_candidates_btn))
            .set_visible(
                cx,
                state == CandidateSetState::AwaitingChoice && failed > 0,
            );

        match preview {
            Some((candidate_id, bytes)) => {
                if self.candidate_preview_id.as_deref() != Some(candidate_id.as_str()) {
                    match self
                        .ui
                        .image(cx, ids!(candidate_preview))
                        .load_png_from_data(cx, &bytes)
                    {
                        Ok(()) => self.candidate_preview_id = Some(candidate_id),
                        Err(error) => log!("candidate preview decode failed: {error:?}"),
                    }
                }
            }
            None => {
                self.candidate_preview_id = None;
                self.ui
                    .image(cx, ids!(candidate_preview))
                    .set_texture(cx, None);
            }
        }
        if self.surface == Surface::Create {
            self.set_caption(cx, "CHOOSE", "Fleet candidates · select exactly one image");
            self.show_page(cx, id!(choice_page));
        }
    }

    fn continue_candidate_choice(&mut self, cx: &mut Cx, early: bool) {
        let Some(index) = self.runs.iter().rposition(|run| {
            run.pipeline
                .active_candidate_set()
                .is_some_and(|set| {
                    set.state
                        == if early {
                            CandidateSetState::FanOut
                        } else {
                            CandidateSetState::AwaitingChoice
                        }
                })
        }) else {
            return;
        };
        let set_id = self.runs[index]
            .pipeline
            .active_candidate_set()
            .map(|set| set.id.clone())
            .expect("candidate set was found above");
        let snapshots = self
            .fleet
            .as_ref()
            .map(|fleet| fleet.snapshots.clone())
            .unwrap_or_default();
        let avoid = self.avoid_for_run(index);
        let run_id = self.runs[index].id;
        let result = if early {
            self.runs[index]
                .pipeline
                .continue_after_choice_early(cx, &set_id, &snapshots, &avoid)
        } else {
            self.runs[index]
                .pipeline
                .continue_after_choice(cx, &set_id, &snapshots, &avoid)
        };
        match result {
            Ok(events) => self.on_run_events(cx, run_id, events),
            Err(error) => self.set_caption(cx, "CHOOSE", &error),
        }
    }

    fn retry_candidate_failures(&mut self, cx: &mut Cx) {
        let Some(index) = self.runs.iter().rposition(|run| {
            run.pipeline
                .active_candidate_set()
                .is_some_and(|set| set.state == CandidateSetState::AwaitingChoice)
        }) else {
            return;
        };
        let set_id = self.runs[index]
            .pipeline
            .active_candidate_set()
            .map(|set| set.id.clone())
            .expect("candidate set was found above");
        let snapshots = self
            .fleet
            .as_ref()
            .map(|fleet| fleet.snapshots.clone())
            .unwrap_or_default();
        let avoid = self.avoid_for_run(index);
        let run_id = self.runs[index].id;
        match self.runs[index]
            .pipeline
            .retry_failed_candidates(cx, &set_id, &snapshots, &avoid)
        {
            Ok(events) => self.on_run_events(cx, run_id, events),
            Err(error) => self.set_caption(cx, "RETRY", &error),
        }
    }

    fn refresh_stages(&mut self, cx: &mut Cx) {
        // Slim per-stage bars: one row per chain stage, accent fill (red on
        // failure). The text log below keeps the full routing detail.
        let rows = [
            (ids!(s1_row), ids!(s1_name), ids!(s1_bar)),
            (ids!(s2_row), ids!(s2_name), ids!(s2_bar)),
            (ids!(s3_row), ids!(s3_name), ids!(s3_bar)),
            (ids!(s4_row), ids!(s4_name), ids!(s4_bar)),
            (ids!(s5_row), ids!(s5_name), ids!(s5_bar)),
            (ids!(s6_row), ids!(s6_name), ids!(s6_bar)),
        ];
        let stages: Vec<(String, f64, bool)> = self
            .display_run()
            .map(|run| {
                run.pipeline
                    .stages
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let (fraction, is_failed) = match &s.state {
                            StageState::Done => (1.0, false),
                            StageState::AwaitingChoice => (1.0, false),
                            StageState::Failed(_) => (1.0, true),
                            _ => (s.progress, false),
                        };
                        let activity = match &s.state {
                            StageState::Waiting if !s.detail.is_empty() => s.detail.as_str(),
                            StageState::Waiting => "waiting",
                            StageState::FanOut => s.detail.as_str(),
                            StageState::AwaitingChoice => s.detail.as_str(),
                            StageState::Submitting => "submitting",
                            StageState::Polling if !s.detail.is_empty() => s.detail.as_str(),
                            StageState::Polling => s.service_state.as_str(),
                            StageState::Fetching => "fetching artifacts",
                            StageState::Done => "done",
                            StageState::Failed(_) => "FAILED",
                        };
                        let elapsed = match (s.started, s.finished) {
                            (Some(t0), Some(t1)) => {
                                format!("{:.1}s", (t1 - t0).as_secs_f64())
                            }
                            (Some(t0), None) => format!("{:.0}s", t0.elapsed().as_secs_f64()),
                            _ => String::new(),
                        };
                        let stage_name = if s.domain == "music" {
                            format!(
                                "{} ({} target)",
                                stage_display_name(&s.domain),
                                format_music_duration(run.pipeline.gen.music_seconds),
                            )
                        } else {
                            stage_display_name(&s.domain).to_string()
                        };
                        let mut label = format!(
                            "{} · {} · {:>5.1}% · {}",
                            i + 1,
                            stage_name,
                            fraction.clamp(0.0, 1.0) * 100.0,
                            truncate(activity, 22),
                        );
                        if !elapsed.is_empty() {
                            label.push_str(&format!(" · {elapsed}"));
                        }
                        (label, fraction, is_failed)
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (k, (row, name, bar)) in rows.iter().enumerate() {
            let visible = k < stages.len();
            self.ui.widget(cx, *row).set_visible(cx, visible);
            if !visible {
                continue;
            }
            let (label, fraction, is_failed) = &stages[k];
            let fill: [f32; 4] = if *is_failed {
                [0.85, 0.35, 0.32, 1.0]
            } else {
                [0.24, 0.61, 0.94, 1.0]
            };
            self.ui.label(cx, *name).set_text(cx, label);
            let bar = self.ui.view(cx, *bar);
            bar.set_uniform(cx, live_id!(progress), &[*fraction as f32]);
            bar.set_uniform(cx, live_id!(color_fill), &fill);
        }
        if let Some(run) = self.display_run() {
            let others = self.active_run_count().saturating_sub(
                usize::from(run.pipeline.is_running()),
            );
            let mut text = format!("run: {}\n{}", run.group_label, run.pipeline.status_text());
            if others > 0 {
                text.push_str(&format!(
                    "\n+{others} more running — see RUNS + WORKERS for each"
                ));
            }
            self.ui.label(cx, ids!(stages_label)).set_text(cx, &text);
        }
    }

    fn on_run_events(&mut self, cx: &mut Cx, run_id: u64, events: Vec<PipelineEvent>) {
        let mut done_or_failed = false;
        for event in events {
            match event {
                PipelineEvent::Changed => {}
                PipelineEvent::Artifact { stage, output } => {
                    // Everything comes from the EMITTING run — prompt, group
                    // and the upstream-preview walk — so completions landing
                    // out of order across concurrent runs can never
                    // cross-attribute an artifact or a preview.
                    let Some(run) = self.runs.iter().find(|run| run.id == run_id) else {
                        continue;
                    };
                    let pipeline = &run.pipeline;
                    let s = &pipeline.stages[stage];
                    let (ct, bytes) = &s.outputs[output];
                    // Keep the nearest image/matte from the same pipeline
                    // run as the durable visual preview for downstream
                    // mesh/rig/motion/world/video artifacts. Include the
                    // current stage because some backends return a preview
                    // image alongside their primary artifact. AUDIO is
                    // excluded by contract: a WAV's preview is always its
                    // own waveform, never a neighbouring image (the
                    // gorilla-thumbnail provenance bug).
                    let thumbnail = upstream_preview_allowed(ct)
                        .then(|| {
                            pipeline.stages[..=stage]
                                .iter()
                                .rev()
                                .flat_map(|stage| stage.outputs.iter().rev())
                                .find(|(content_type, _)| {
                                    content_type
                                        .to_ascii_lowercase()
                                        .starts_with("image/")
                                })
                                .map(|(_, bytes)| bytes.clone())
                        })
                        .flatten();
                    let (domain, content_type, bytes) =
                        (s.domain.clone(), ct.clone(), bytes.clone());
                    let (prompt, group_id, group_label) = (
                        run.prompt.clone(),
                        run.group_id.clone(),
                        run.group_label.clone(),
                    );
                    self.route_artifact(
                        cx,
                        &domain,
                        &content_type,
                        bytes,
                        thumbnail.as_deref(),
                        &prompt,
                        Some((&group_id, &group_label)),
                        None,
                        true,
                    );
                }
                PipelineEvent::CandidateSetStarted { stage, set_id } => {
                    log!("run {run_id}: fan-out set {set_id} started at stage {stage}");
                    if self.display_run().map(|run| run.id) == Some(run_id) {
                        self.candidate_preview_id = None;
                        self.ui
                            .image(cx, ids!(candidate_preview))
                            .set_texture(cx, None);
                        if let Some(mut sheet) = self
                            .ui
                            .widget(cx, ids!(candidate_sheet))
                            .borrow_mut::<CandidateSheet>()
                        {
                            sheet.clear_textures(cx);
                        }
                    }
                    self.refresh_candidate_ui(cx, run_id);
                }
                PipelineEvent::CandidateUpdated {
                    stage,
                    set_id,
                    candidate_id,
                } => {
                    let _identity = (stage, set_id, candidate_id);
                    self.refresh_candidate_ui(cx, run_id);
                }
                PipelineEvent::CandidateSelected {
                    stage,
                    set_id,
                    candidate_id,
                } => {
                    log!(
                        "run {run_id}: selected {candidate_id} from {set_id} at stage {stage}"
                    );
                    self.refresh_candidate_ui(cx, run_id);
                }
                PipelineEvent::CandidateArtifact {
                    stage,
                    set_id,
                    candidate_id,
                    output,
                } => {
                    let info = self
                        .runs
                        .iter()
                        .find(|run| run.id == run_id)
                        .and_then(|run| {
                            let set = run
                                .pipeline
                                .candidate_sets
                                .iter()
                                .find(|set| set.id == set_id && set.stage == stage)?;
                            let index = set
                                .candidates
                                .iter()
                                .position(|candidate| candidate.id == candidate_id)?;
                            let candidate = &set.candidates[index];
                            let artifact = candidate.outputs.get(output)?;
                            Some((
                                artifact.content_type.clone(),
                                artifact.bytes.clone(),
                                artifact.remote_id.clone(),
                                artifact.sha256.clone(),
                                artifact.byte_len,
                                candidate.endpoint.clone(),
                                candidate.model.clone(),
                                candidate.seed,
                                index,
                                set.candidates.len(),
                                run.prompt.clone(),
                                run.group_id.clone(),
                                run.group_label.clone(),
                            ))
                        });
                    if let Some((
                        content_type,
                        bytes,
                        remote_id,
                        sha256,
                        byte_len,
                        endpoint,
                        model,
                        seed,
                        index,
                        count,
                        prompt,
                        group_id,
                        group_label,
                    )) = info
                    {
                        self.install_candidate_texture(cx, &candidate_id, &bytes);
                        let label = format!(
                            "candidate {}/{} · {} @ {} · seed {} · {} · {} · {} bytes",
                            index + 1,
                            count,
                            model,
                            endpoint.trim_start_matches("http://"),
                            seed,
                            remote_id,
                            sha256
                                .as_deref()
                                .map(|digest| digest.chars().take(12).collect::<String>())
                                .unwrap_or_else(|| "unhashed".to_string()),
                            byte_len.unwrap_or(bytes.len() as u64),
                        );
                        let _ = self.route_artifact(
                            cx,
                            "image",
                            &content_type,
                            bytes,
                            None,
                            &prompt,
                            Some((&group_id, &group_label)),
                            Some(&label),
                            false,
                        );
                    }
                    self.refresh_candidate_ui(cx, run_id);
                }
                PipelineEvent::ChoiceCommitted {
                    stage,
                    set_id,
                    candidate_id,
                    output,
                } => {
                    // Candidate artifacts were persisted as they landed. Add
                    // one exact-byte chosen record in the SAME history group,
                    // so the durable history clearly identifies the artifact
                    // whose bytes feed video.
                    let info = self
                        .runs
                        .iter()
                        .find(|run| run.id == run_id)
                        .and_then(|run| {
                            let set = run
                                .pipeline
                                .candidate_sets
                                .iter()
                                .find(|set| set.id == set_id && set.stage == stage)?;
                            let candidate = set
                                .candidates
                                .iter()
                                .find(|candidate| candidate.id == candidate_id)?;
                            let (content_type, bytes) =
                                run.pipeline.stages.get(stage)?.outputs.get(output)?;
                            Some((
                                content_type.clone(),
                                bytes.clone(),
                                candidate.endpoint.clone(),
                                candidate.model.clone(),
                                candidate.seed,
                                run.prompt.clone(),
                                run.group_id.clone(),
                                run.group_label.clone(),
                            ))
                        });
                    if let Some((
                        content_type,
                        bytes,
                        endpoint,
                        model,
                        seed,
                        prompt,
                        group_id,
                        group_label,
                    )) = info
                    {
                        let label = format!(
                            "✓ CHOSEN · {model} @ {} · seed {seed}",
                            endpoint.trim_start_matches("http://")
                        );
                        let _ = self.route_artifact(
                            cx,
                            "image",
                            &content_type,
                            bytes,
                            None,
                            &prompt,
                            Some((&group_id, &group_label)),
                            Some(&label),
                            false,
                        );
                    }
                    self.refresh_candidate_ui(cx, run_id);
                }
                PipelineEvent::StageFailed { stage } => {
                    let status = self
                        .runs
                        .iter()
                        .find(|run| run.id == run_id)
                        .map(|run| run.pipeline.status_text());
                    log!("run {run_id}: stage {stage} failed: {status:?}");
                    done_or_failed = true;
                }
                PipelineEvent::Finished => {
                    log!("run {run_id}: finished");
                    done_or_failed = true;
                }
            }
        }
        self.refresh_stages(cx);
        self.refresh_run_ui(cx);
        if done_or_failed {
            if self.auto.capture.is_some()
                && !self.auto.captured
                && self.auto.capture_at_s.is_none()
                && !self.any_run_running()
            {
                // Let the viewer draw the artifact before grabbing the frame.
                self.capture_timer = cx.start_timeout(1.5);
            }
            // A slot freed — drain whatever the fleet can now take.
            self.try_dispatch_pending(cx);
        }
        if let Some(candidate_run_id) = self
            .display_run()
            .and_then(|run| run.pipeline.active_candidate_set().is_some().then_some(run.id))
        {
            self.refresh_candidate_ui(cx, candidate_run_id);
        }
        self.ui.redraw(cx);
    }

    // -- artifact routing -------------------------------------------------------

    fn route_artifact(
        &mut self,
        cx: &mut Cx,
        domain: &str,
        content_type: &str,
        bytes: Vec<u8>,
        thumbnail: Option<&[u8]>,
        prompt: &str,
        group: Option<(&str, &str)>,
        label_override: Option<&str>,
        show_in_viewer: bool,
    ) -> Option<String> {
        self.artifact_count += 1;
        let n = self.artifact_count;
        log!("artifact #{n}: {domain} {content_type} {} bytes", bytes.len());
        // Paint sidecars (albedo/normal/ORM/manifest) stay in History but
        // must not replace the textured GLB in the viewer or steal selection.
        let show_in_viewer = show_in_viewer && auto_show_artifact(domain, content_type);
        // Persist FIRST — History readiness never depends on what surface
        // is up. (Audio-thumbnail provenance is enforced INSIDE the library:
        // any caller thumbnail for audio is discarded there.)
        let mut managed_file = None;
        if let Some(library) = &mut self.library {
            let label = label_override.map(str::to_string).unwrap_or_else(|| {
                format!(
                    "{} {}",
                    kind_label(domain, content_type),
                    truncate(prompt, 14)
                )
            });
            match library.add_with_thumbnail(
                domain,
                content_type,
                prompt,
                &label,
                &bytes,
                thumbnail,
                group,
            ) {
                Ok(file) => {
                    if show_in_viewer {
                        self.selected_file = Some(file.clone());
                    }
                    managed_file = Some(file);
                }
                Err(error) => log!("library: could not persist artifact: {error}"),
            }
        }
        if let Some(file) = &managed_file {
            self.queue_glb_thumbnail(cx, file, &bytes);
        }
        // Display/play ONLY while the Create viewer is actually up. On other
        // surfaces the completion is persisted + selected (ready in the
        // strip/grid); returning to Create reopens it through the async
        // loading path (ViewerContent::needs_reopen).
        if show_in_viewer && self.surface == Surface::Create {
            if self.display_artifact(cx, domain, content_type, &bytes, n, None, true) {
                // An unpersisted display (library write failed) is
                // TRANSIENT: not library-bound, so it must never trip the
                // deleted-item viewer reset; the next open replaces it.
                self.viewer = match &managed_file {
                    Some(file) => ViewerContent::Showing(file.clone()),
                    None => ViewerContent::Empty,
                };
                self.set_caption(cx, kind_label(domain, content_type), &truncate(prompt, 90));
            } else {
                let message =
                    format!("{domain} artifact could not be displayed ({content_type}).");
                match &managed_file {
                    Some(file) => self.show_viewer_error(cx, &file.clone(), &message),
                    None => {
                        self.viewer = ViewerContent::Empty;
                        self.set_viewer_text(cx, &message);
                        self.set_caption(cx, "error", &truncate(&message, 90));
                        self.show_page(cx, id!(text_page));
                    }
                }
            }
        }
        self.refresh_gallery(cx, true);
        managed_file
    }

    /// Viewer header: what the right pane is currently showing.
    fn set_caption(&mut self, cx: &mut Cx, kind: &str, detail: &str) {
        self.ui
            .widget(cx, ids!(viewer_badge))
            .set_visible(cx, !kind.is_empty());
        self.ui
            .label(cx, ids!(viewer_badge_label))
            .set_text(cx, &kind.to_ascii_uppercase());
        let detail = if detail.trim().is_empty() {
            "generated artifact"
        } else {
            detail
        };
        self.ui.label(cx, ids!(viewer_caption)).set_text(cx, detail);
    }

    /// Put artifact bytes on the matching viewer page. Returns false when
    /// NOTHING meaningful could be shown (decode/open/write failure) — the
    /// caller then presents the honest error state; prior content was
    /// already cleared by the loading entry, so a failure never leaves old
    /// content under a new caption.
    fn display_artifact(
        &mut self,
        cx: &mut Cx,
        domain: &str,
        content_type: &str,
        bytes: &[u8],
        n: u64,
        prewritten: Option<&std::path::Path>,
        // True only for a freshly accepted generated clip. History / Library
        // reopen must stay paused: `audio::play()` restarts at end-of-clip,
        // so a second display of a short Quake/Doom shot is a speaker loop.
        audition: bool,
    ) -> bool {
        let ct = content_type.to_ascii_lowercase();
        let is_glb = bytes.starts_with(b"glTF") || ct.contains("gltf");
        let is_ply = bytes.starts_with(b"ply") || ct.contains("ply");
        // Domain `billboard` is also used for loose Duke TILE-N PNGs.
        // Only a stateful manifest goes to BillboardView.
        let is_billboard = ct.contains("billboard") || ct.contains("x-stateful-billboard");
        if is_billboard {
            let path = self.selected_file.as_ref().and_then(|file| {
                self.library
                    .as_ref()
                    .and_then(|library| library.payload_path(file).ok())
            });
            if let Some(path) = path {
                if let Some(mut view) = self
                    .ui
                    .widget(cx, ids!(billboard_view))
                    .borrow_mut::<BillboardView>()
                {
                    view.load_manifest(cx, &path);
                }
                self.show_page(cx, id!(billboard_page));
                return true;
            }
            self.set_viewer_text(cx, "billboard: missing library path");
            self.show_page(cx, id!(text_page));
            return true;
        }
        if ct.starts_with("text/") || ct.starts_with("application/json") {
            // The text viewer REPLACES: it always shows exactly the selected
            // artifact, never an append-log of past ones.
            let text = String::from_utf8_lossy(bytes);
            self.set_viewer_text(cx, text.trim());
            self.show_page(cx, id!(text_page));
            true
        } else if ct.starts_with("image/") {
            match self
                .ui
                .image(cx, ids!(image_view))
                .load_png_from_data(cx, bytes)
            {
                Ok(()) => {
                    self.show_page(cx, id!(image_page));
                    true
                }
                Err(e) => {
                    log!("image artifact failed to decode: {e:?}");
                    self.ui.image(cx, ids!(image_view)).set_texture(cx, None);
                    false
                }
            }
        } else if ct.starts_with("audio/") {
            match audio::parse_wav(bytes) {
                Ok(pcm) => {
                    let wave = audio::waveform_bgra(&pcm, 900, 140);
                    let texture = Texture::new_with_format(
                        cx,
                        TextureFormat::VecBGRAu8_32 {
                            width: 900,
                            height: 140,
                            data: Some(wave),
                            updated: TextureUpdated::Full,
                        },
                    );
                    self.ui
                        .image(cx, ids!(wave_img))
                        .set_texture(cx, Some(texture));
                    if audio::load(pcm.clone()) {
                        // Accept-path only. A gallery reopen of the same
                        // WAV must not call play() or a 200ms DS_* / Quake
                        // shot becomes a loop (play-at-end restarts).
                        if audition && audio::autoplay_one_shot(domain, pcm.seconds()) {
                            crate::video_player::stop_audio();
                            audio::play();
                            self.arm_audio_pump(cx);
                        }
                        self.audio_clip = Some(pcm);
                        self.sync_audio_ui(cx);
                    } else {
                        self.audio_clip = None;
                        self.set_audio_unavailable(cx, "empty WAV — playback unavailable");
                    }
                }
                Err(e) => {
                    // Honest empty state ON the audio page: cleared strip,
                    // disabled transport, the reason in the info line.
                    log!("audio artifact: {e}");
                    audio::clear();
                    self.audio_clip = None;
                    self.ui.image(cx, ids!(wave_img)).set_texture(cx, None);
                    self.set_audio_unavailable(cx, &e);
                }
            }
            self.show_page(cx, id!(audio_page));
            true
        } else if ct.starts_with("video/") {
            // Async gallery opens hand in a path the IO worker already wrote;
            // fresh accept-path artifacts (bytes only in memory) write here.
            let path = match prewritten {
                Some(path) => path.to_path_buf(),
                None => {
                    let path = artifacts_dir().join(format!("artifact-{n}.mp4"));
                    if let Err(e) = std::fs::write(&path, bytes) {
                        log!("video artifact write failed: {e}");
                        return false;
                    }
                    path
                }
            };
            // The WIDGET texture clears too — never a previous clip's frame
            // behind a new open or an error state.
            self.stop_video_playback();
            self.clear_video_frame(cx);
            match VideoPlayer::new(&path.to_string_lossy()) {
                Ok(player) => {
                    self.ui.label(cx, ids!(video_info)).set_text(
                        cx,
                        &format!("{}x{}  {}", player.width, player.height, path.display()),
                    );
                    self.video = Some(player);
                    self.video_pump = cx.new_next_frame();
                    self.show_page(cx, id!(video_page));
                    true
                }
                Err(e) => {
                    log!("video artifact failed to open: {e}");
                    false
                }
            }
        } else if is_glb {
            let spawn = if domain.eq_ignore_ascii_case("world")
                || domain.eq_ignore_ascii_case("map")
            {
                self.selected_file.as_ref().and_then(|file| {
                    self.library
                        .as_ref()
                        .and_then(|library| library.world_spawn(file))
                })
            } else {
                None
            };
            if let Some(mut mesh) = self
                .ui
                .widget(cx, ids!(mesh_view))
                .borrow_mut::<MeshView>()
            {
                let (aomesh, ao_png) = self
                    .selected_file
                    .as_ref()
                    .and_then(|file| self.library.as_ref().map(|lib| lib.ao_sidecar_bytes(file)))
                    .unwrap_or((None, None));
                mesh.set_model_bytes_ao(cx, bytes.to_vec(), None, aomesh, ao_png);
                if let Some(spawn) = spawn {
                    mesh.enable_walk(cx, spawn);
                }
                if let (Some(file), Some(library)) =
                    (self.selected_file.as_ref(), self.library.as_ref())
                {
                    if let Some(place) = library.world_place(file) {
                        let mut sprites = Vec::new();
                        let mut models = Vec::new();
                        for p in &place.places {
                            if p.asset.is_empty() {
                                continue;
                            }
                            let Some(path) = library.find_place_asset(&p.asset) else {
                                continue;
                            };
                            if p.align == "face" {
                                sprites.push((
                                    vec3f(p.pos[0], p.pos[1], p.pos[2]),
                                    p.width,
                                    p.height,
                                    path,
                                ));
                            } else {
                                models.push((
                                    vec3f(p.pos[0], p.pos[1], p.pos[2]),
                                    p.yaw,
                                    path,
                                ));
                            }
                        }
                        mesh.set_placed_sprites(cx, sprites);
                        mesh.set_placed_models(cx, models);
                    }
                }
            }
            self.show_page(cx, id!(mesh_page));
            true
        } else if is_ply {
            let path = match prewritten {
                Some(path) => path.to_path_buf(),
                None => {
                    let path = artifacts_dir().join(format!("artifact-{n}.ply"));
                    if let Err(e) = std::fs::write(&path, bytes) {
                        log!("splat artifact write failed: {e}");
                        return false;
                    }
                    path
                }
            };
            self.set_splat_file(cx, &path.to_string_lossy(), false);
            self.show_page(cx, id!(splat_page));
            true
        } else {
            self.set_viewer_text(
                cx,
                &format!(
                    "{domain}: no viewer for {content_type} ({} bytes).",
                    bytes.len()
                ),
            );
            self.show_page(cx, id!(text_page));
            true
        }
    }

    // -- gallery -----------------------------------------------------------------

    fn start_file_payload_drag(
        &mut self,
        cx: &mut Cx,
        window_id: WindowId,
        payload_path: PathBuf,
    ) {
        if self.file_drag_active {
            return;
        }
        let managed_root = match std::fs::canonicalize(repo_path("local/ai_content_library")) {
            Ok(path) => path,
            Err(error) => {
                log!("library: cannot resolve managed root for outbound drag: {error}");
                return;
            }
        };
        let canonical = match std::fs::canonicalize(&payload_path) {
            Ok(path)
                if path.is_file()
                    && path.is_absolute()
                    && path.starts_with(&managed_root)
                    && std::fs::symlink_metadata(&payload_path)
                        .is_ok_and(|metadata| metadata.file_type().is_file()) =>
            {
                path
            }
            Ok(path) => {
                log!(
                    "library: refusing outbound media drag outside managed files: {}",
                    path.display()
                );
                return;
            }
            Err(error) => {
                log!(
                    "library: cannot resolve outbound media drag {}: {error}",
                    payload_path.display()
                );
                return;
            }
        };
        self.file_drag_active = true;
        cx.start_external_dragging(
            window_id,
            vec![DragItem::FilePath {
                path: canonical.to_string_lossy().into_owned(),
                internal_id: None,
            }],
        );
    }

    fn refresh_gallery(&mut self, cx: &mut Cx, clear_thumbnails: bool) {
        let (entries, count) = match &self.library {
            Some(library) => {
                let entries = library
                    .newest_items()
                    .filter_map(|item| {
                        let path = library.payload_path(&item.file).ok()?;
                        let ct = item.content_type.to_ascii_lowercase();
                        let preview_path = if item.domain.eq_ignore_ascii_case("billboard")
                            || ct.contains("billboard")
                        {
                            Some(path.clone())
                        } else if ct.starts_with("image/") {
                            Some(path.clone())
                        } else {
                            library.thumbnail_path(&item.file).ok().flatten()
                        };
                        Some(GalleryEntry {
                            meta: item.clone(),
                            path,
                            preview_path,
                            selected: self.selected_file.as_deref() == Some(item.file.as_str()),
                        })
                    })
                    .collect::<Vec<_>>();
                (entries, library.len())
            }
            None => (Vec::new(), 0),
        };
        if let Some(mut gallery) = self
            .ui
            .widget(cx, ids!(library_gallery))
            .borrow_mut::<LibraryGallery>()
        {
            gallery.set_entries(cx, entries, clear_thumbnails);
        }
        self.ui.label(cx, ids!(library_hint)).set_text(
            cx,
            &format!(
                "{count} resources · one tile per run · click to view · double-click = use as input · drag audio/video by DRAG · × removes the whole run"
            ),
        );
        // The Library surface browses the same disk-backed store; keep its
        // grid, counts and detail rail in lockstep with every mutation.
        self.refresh_library_ui(cx, clear_thumbnails);
        // Every library mutation funnels through here — the input tray's
        // deletion sweep rides along so it can never advertise a payload
        // that no longer exists.
        self.sync_input_tray(cx);
        self.sync_run_tray(cx);
        self.ui.redraw(cx);
    }

    /// The run tray's chip slots, in order.
    fn run_chip_ids() -> [&'static [LiveId]; RUN_TRAY_SLOTS] {
        [
            ids!(run_chip0),
            ids!(run_chip1),
            ids!(run_chip2),
            ids!(run_chip3),
            ids!(run_chip4),
            ids!(run_chip5),
            ids!(run_chip6),
            ids!(run_chip7),
        ]
    }

    /// Spread the SELECTED run out into the run tray: one chip per member
    /// artifact in pipeline order (oldest first), the viewer's current file
    /// highlighted. Hidden for ungrouped records and single-artifact runs.
    /// Textures come from the gallery's decoded cache when it has them, else
    /// from the same async preview worker the tiles use.
    fn sync_run_tray(&mut self, cx: &mut Cx) {
        let members: Vec<crate::library::LibraryMeta> = match (&self.library, &self.selected_file) {
            (Some(library), Some(selected)) => library
                .get(selected)
                .and_then(|item| item.group_id.clone())
                .map(|group| {
                    let mut members: Vec<_> = library
                        .newest_items()
                        .filter(|item| item.group_id.as_deref() == Some(group.as_str()))
                        .cloned()
                        .collect();
                    members.reverse();
                    members
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let files: Vec<String> = members.iter().map(|m| m.file.clone()).collect();
        let changed = files != self.run_tray_files;
        self.run_tray_files = files;
        let show = members.len() > 1;
        self.ui.widget(cx, ids!(run_tray)).set_visible(cx, show);
        if !show {
            return;
        }
        let label = members
            .iter()
            .find_map(|m| m.group_label.clone())
            .unwrap_or_else(|| "run".to_string());
        self.ui.label(cx, ids!(run_tray_title)).set_text(
            cx,
            &format!("RUN · {} · {} artifacts — click to view · double-click = use as input", truncate(&label, 34), members.len()),
        );
        let library = self.library.as_ref().expect("members imply a library");
        let gallery_widget = self.ui.widget(cx, ids!(library_gallery));
        for (slot, chip) in Self::run_chip_ids().iter().enumerate() {
            let chip_view = self.ui.view(cx, chip);
            let Some(member) = members.get(slot) else {
                chip_view.set_visible(cx, false);
                continue;
            };
            chip_view.set_visible(cx, true);
            let selected = self.selected_file.as_deref() == Some(member.file.as_str());
            chip_view.toggle_state(cx, selected, Animate::No, ids!(select.on), ids!(select.off));
            // Stage-ish label: the domain (matte, mesh, paint, rig, …) says
            // more than the payload kind; plain images keep the kind.
            let kind = if member.domain.is_empty() || member.domain.eq_ignore_ascii_case("image") {
                crate::asset_store_state::local_kind(&member.domain, &member.content_type).to_string()
            } else {
                member.domain.to_ascii_lowercase()
            };
            let mut chip_kind = chip.to_vec();
            chip_kind.push(live_id!(kind));
            self.ui.label(cx, &chip_kind).set_text(cx, &kind);
            if !changed {
                continue;
            }
            // Texture: cached decode if the History strip has it, else badge
            // now + async preview.
            let mut chip_thumb = chip.to_vec();
            chip_thumb.push(live_id!(thumb));
            let cached = gallery_widget
                .borrow::<LibraryGallery>()
                .and_then(|gallery| gallery.cached_texture(&member.file));
            match cached {
                Some(texture) => {
                    self.ui.image(cx, &chip_thumb).set_texture(cx, Some(texture));
                }
                None => {
                    let badge = crate::store_views::badge_texture(cx, &member.domain);
                    self.ui.image(cx, &chip_thumb).set_texture(cx, Some(badge));
                    if let Ok(path) = library.payload_path(&member.file) {
                        let ct = member.content_type.to_ascii_lowercase();
                        let preview_path = if ct.starts_with("image/") {
                            Some(path.clone())
                        } else {
                            library.thumbnail_path(&member.file).ok().flatten()
                        };
                        let entry = GalleryEntry {
                            meta: member.clone(),
                            path,
                            preview_path,
                            selected: false,
                        };
                        if let Some(work) = crate::store_views::preview_work(&entry) {
                            self.extra_preview_work.retain(|(f, _)| f != &member.file);
                            self.extra_preview_work.push((member.file.clone(), work));
                        }
                    }
                }
            }
        }
        if changed {
            self.pump_gallery_previews(cx);
        }
    }

    /// Input tray chip + honest transform-action labels. Cheap when nothing
    /// changed: chip work is gated on the pinned file actually differing.
    fn sync_input_tray(&mut self, cx: &mut Cx) {
        // The pinned record may have been deleted (single ×, whole-run ×,
        // or cap eviction) — the tray clears; nothing else may stand in.
        let library = self.library.as_ref();
        self.input_tray
            .retain_existing(|file| library.is_some_and(|library| library.get(file).is_some()));
        let current = self.input_tray.current().cloned();
        if current.as_ref().map(|asset| asset.file.clone()) == self.input_chip_file {
            return;
        }
        self.input_chip_file = current.as_ref().map(|asset| asset.file.clone());
        match current {
            Some(asset) => {
                self.ui.widget(cx, ids!(input_tray)).set_visible(cx, true);
                self.ui.label(cx, ids!(input_chip_kind)).set_text(
                    cx,
                    &format!(
                        "INPUT · {}",
                        crate::asset_store_state::local_kind(&asset.domain, &asset.content_type)
                    ),
                );
                self.ui
                    .label(cx, ids!(input_chip_title))
                    .set_text(cx, &asset.label);
                // Typed badge NOW; the decoded preview (display-only, never
                // the run input) swaps in from the IO worker.
                let badge = crate::store_views::badge_texture(cx, &asset.domain);
                self.ui
                    .image(cx, ids!(input_chip_thumb))
                    .set_texture(cx, Some(badge));
                self.request_input_chip_preview(cx, &asset);
            }
            None => {
                self.ui.widget(cx, ids!(input_tray)).set_visible(cx, false);
            }
        }
        self.refresh_transform_labels(cx);
        self.ui.redraw(cx);
    }

    /// Route the chip's display thumbnail through the bounded preview
    /// worker (same dedup + one-in-flight rules as the gallery cards).
    fn request_input_chip_preview(&mut self, cx: &mut Cx, asset: &InputAsset) {
        let entry = GalleryEntry {
            meta: crate::library::LibraryMeta {
                file: asset.file.clone(),
                label: asset.label.clone(),
                domain: asset.domain.clone(),
                content_type: asset.content_type.clone(),
                prompt: String::new(),
                group_id: None,
                group_label: None,
                tags: None,
                enhanced_tags: None,
            },
            path: asset.path.clone(),
            preview_path: asset.preview_path.clone(),
            selected: false,
        };
        // The History strip usually already decoded this file — reuse it.
        // Otherwise queue the decode ourselves: the gallery only records a
        // miss while IT draws the tile, so a chip for an off-screen (or
        // already-cached) file would never get a texture.
        let cached = self
            .ui
            .widget(cx, ids!(library_gallery))
            .borrow::<LibraryGallery>()
            .and_then(|gallery| gallery.cached_texture(&asset.file));
        if let Some(texture) = cached {
            self.ui
                .image(cx, ids!(input_chip_thumb))
                .set_texture(cx, Some(texture));
            return;
        }
        if let Some(work) = crate::store_views::preview_work(&entry) {
            self.extra_preview_work.retain(|(f, _)| f != &asset.file);
            self.extra_preview_work.push((asset.file.clone(), work));
            self.pump_gallery_previews(cx);
        }
    }

    /// Honest labels for the input-consuming quick actions: a compatible
    /// pinned input relabels each button to the transform it will actually
    /// run ("Mesh from selected image"); otherwise the stock generate-chain
    /// label shows and the button generates exactly as written.
    fn refresh_transform_labels(&mut self, cx: &mut Cx) {
        let seed_ct = self
            .input_tray
            .current()
            .map(|asset| asset.content_type.clone());
        for (button, preset_name, base, target) in [
            (ids!(qp_mesh), "image → mesh", "Image → Mesh", "Mesh"),
            (
                ids!(qp_mesh_pbr),
                "image → mesh → hunyuan PBR",
                "Image → Mesh → PBR",
                "PBR",
            ),
            (ids!(qp_cutout), "image → cutout (alpha)", "Image → Cutout", "Cutout"),
            (ids!(qp_depth), "image → depthmap", "Image → Depth", "Depth"),
            (ids!(qp_world), "image → world", "Image → World", "World"),
            (
                ids!(qp_expworld),
                "expand → image → world",
                "Expand → Image → World",
                "World",
            ),
            (ids!(qp_i2v), "image → video", "Image → Video", "Video"),
            (
                ids!(qp_expi2v),
                "expand → image → video",
                "Expand → Img → Video",
                "Video",
            ),
            (
                ids!(qp_fleet_i2v),
                "fleet images → choose → video",
                "Fleet Images → Choose → Video",
                "Video",
            ),
            (
                ids!(qp_character),
                "character (playable)",
                "Character",
                "Character",
            ),
            (
                ids!(qp_character_pbr),
                "character (playable + hunyuan PBR)",
                "Character → PBR",
                "Character",
            ),
        ] {
            let seeded_kind = seed_ct.as_deref().and_then(|ct| {
                let preset = PRESETS
                    .iter()
                    .find(|preset| preset.name.starts_with(preset_name))?;
                seed_replaces_prefix(preset.domains, ct)?;
                Some(seed_kind_word(ct))
            });
            let text = match seeded_kind {
                Some(kind) => format!("{target} from selected {kind}"),
                None => base.to_string(),
            };
            self.ui.button(cx, button).set_text(cx, &text);
        }
    }

    fn queue_glb_thumbnail(&mut self, cx: &mut Cx, file: &str, bytes: &[u8]) {
        let (aomesh, ao_png) = self
            .library
            .as_ref()
            .map(|library| library.ao_sidecar_bytes(file))
            .unwrap_or((None, None));
        self.queue_glb_thumbnail_ao(cx, file, bytes, aomesh, ao_png);
    }

    fn queue_glb_thumbnail_ao(
        &mut self,
        cx: &mut Cx,
        file: &str,
        bytes: &[u8],
        aomesh: Option<Vec<u8>>,
        ao_png: Option<Vec<u8>>,
    ) {
        if !bytes.starts_with(b"glTF") {
            return;
        }
        if let Some(mut renderer) = self
            .ui
            .widget(cx, ids!(thumbnail_renderer))
            .borrow_mut::<ThumbnailRenderer>()
        {
            let spawn = self
                .library
                .as_ref()
                .and_then(|library| library.world_spawn(file));
            renderer.queue_library_thumbnail_ao_spawn(
                cx,
                file.to_string(),
                bytes.to_vec(),
                aomesh,
                ao_png,
                spawn,
            );
        }
    }

    /// Commit completed model-only renders after the headless renderer's
    /// previous event. Library revalidates each stable file id, so deleting a
    /// card while its GPU readback is pending cannot resurrect a sidecar or
    /// update another recycled PortalList item.
    fn drain_rendered_thumbnails(&mut self, cx: &mut Cx) {
        let completed = self
            .ui
            .widget(cx, ids!(thumbnail_renderer))
            .borrow_mut::<ThumbnailRenderer>()
            .map(|mut renderer| renderer.take_rendered_thumbnails())
            .unwrap_or_default();
        let rejected = self
            .ui
            .widget(cx, ids!(thumbnail_renderer))
            .borrow_mut::<ThumbnailRenderer>()
            .map(|mut renderer| renderer.take_rejected_thumbnails())
            .unwrap_or_default();
        if completed.is_empty() && rejected.is_empty() {
            return;
        }
        let mut changed = false;
        let mut import_preview_changed = false;
        for file in rejected {
            if self.import_page.note_failed_icon(&file) {
                import_preview_changed = true;
            }
            log!("library: thumbnail load failed for {file}");
        }
        for rendered in completed {
            let result = self
                .library
                .as_ref()
                .ok_or_else(|| "library unavailable".to_string())
                .and_then(|library| {
                    library
                        .replace_thumbnail_png(&rendered.file, &rendered.png)
                        .map_err(|error| error.to_string())
                });
            match result {
                Ok(()) => {
                    changed = true;
                    if self
                        .import_page
                        .note_rendered_icon(&rendered.file, &rendered.png)
                    {
                        import_preview_changed = true;
                    }
                    log!("library: rendered model thumbnail for {}", rendered.file);
                }
                Err(error) => log!(
                    "library: ignored late thumbnail for {}: {error}",
                    rendered.file
                ),
            }
        }
        if changed {
            // Keep already-decoded cards. A full cache wipe made every
            // Freedoom sprite flash while world GPU thumbs landed.
            self.refresh_gallery(cx, false);
        }
        if import_preview_changed && self.surface == Surface::Import {
            self.refresh_import_ui(cx);
        }
    }

    /// One bounded backfill step per timer tick. The worker holds at most
    /// ONE payload read; the offscreen renderer holds at most
    /// [`MODEL_THUMBNAIL_MAX_PENDING`] jobs. Cards keep their typed badge
    /// until each preview lands, so a growing library changes how long
    /// badges linger — never startup, click, or frame time.
    fn pump_thumbnail_backfill(&mut self, cx: &mut Cx) {
        if self.thumb_read_in_flight {
            return;
        }
        while let Some(job) = self.thumbnail_backfill.front().cloned() {
            match job {
                ThumbnailBackfillJob::ModelRender { file } => {
                    let pending = self
                        .ui
                        .widget(cx, ids!(thumbnail_renderer))
                        .borrow::<ThumbnailRenderer>()
                        .map_or(usize::MAX, |renderer| renderer.thumbnail_pending_len());
                    if pending >= MODEL_THUMBNAIL_MAX_PENDING {
                        return;
                    }
                    self.thumbnail_backfill.pop_front();
                    // Revalidate now: the item may have been deleted, or its
                    // preview may have landed via the accept path meanwhile.
                    if !self
                        .library
                        .as_ref()
                        .is_some_and(|library| library.needs_model_thumbnail(&file))
                    {
                        continue;
                    }
                    self.request_thumb_read(&file, IoPurpose::ThumbModel);
                    return;
                }
                ThumbnailBackfillJob::AudioWaveform { file } => {
                    self.thumbnail_backfill.pop_front();
                    let missing = self
                        .library
                        .as_ref()
                        .is_some_and(|library| matches!(library.thumbnail_path(&file), Ok(None)));
                    if !missing {
                        continue;
                    }
                    self.request_thumb_read(&file, IoPurpose::ThumbAudioWaveform);
                    return;
                }
            }
        }
    }

    /// Hand one preview-source read to the worker (bounded to one in flight).
    fn request_thumb_read(&mut self, file: &str, purpose: IoPurpose) {
        let path = self
            .library
            .as_ref()
            .and_then(|library| library.payload_path(file).ok());
        let (Some(path), Some(io)) = (path, &self.artifact_io) else {
            return;
        };
        io.request(IoRequest {
            file: file.to_string(),
            path,
            purpose,
        });
        self.thumb_read_in_flight = true;
    }

    /// Everything the IO worker finished: viewer opens (latest selection
    /// wins), thumbnail model reads, worker-encoded waveform previews.
    fn drain_artifact_io(&mut self, cx: &mut Cx) {
        let completed = self
            .artifact_io
            .as_ref()
            .map(|io| io.drain())
            .unwrap_or_default();
        for done in completed {
            match done {
                IoDone::ViewerOpen {
                    file,
                    generation,
                    copy_to,
                    bytes,
                } => {
                    let (display, next) = self.viewer_gate.complete(generation);
                    if let Some(open) = next {
                        self.submit_viewer_open(open);
                    }
                    if !display {
                        continue;
                    }
                    // Second belt: deletes or auto-flows may have moved the
                    // selection since this read was submitted.
                    if self.selected_file.as_deref() != Some(file.as_str()) {
                        continue;
                    }
                    let bytes = match bytes {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            log!("library: payload read for {file} failed: {error}");
                            self.show_viewer_error(
                                cx,
                                &file,
                                &format!("Could not read this artifact: {error}"),
                            );
                            continue;
                        }
                    };
                    let Some(item) = self
                        .library
                        .as_ref()
                        .and_then(|library| library.get(&file).cloned())
                    else {
                        self.reset_viewer_if_gone(cx);
                        continue;
                    };
                    // COMMIT: content + caption together, only for the
                    // still-current selection (both belts passed above).
                    if self.display_artifact(
                        cx,
                        &item.domain,
                        &item.content_type,
                        &bytes,
                        0,
                        copy_to.as_deref(),
                        false,
                    ) {
                        self.viewer = ViewerContent::Showing(file.clone());
                        self.set_caption(
                            cx,
                            kind_label(&item.domain, &item.content_type),
                            &truncate(&item.prompt, 90),
                        );
                    } else {
                        self.show_viewer_error(
                            cx,
                            &file,
                            &format!(
                                "{} artifact could not be displayed ({}).",
                                item.domain, item.content_type
                            ),
                        );
                    }
                    // A valid persisted preview is authoritative — a click
                    // re-renders only a missing/invalidated GLB sidecar.
                    if self
                        .library
                        .as_ref()
                        .is_some_and(|library| library.needs_model_thumbnail(&file))
                    {
                        self.queue_glb_thumbnail(cx, &file, &bytes);
                    }
                }
                IoDone::ThumbModel { file, bytes } => {
                    self.thumb_read_in_flight = false;
                    let wanted = self
                        .library
                        .as_ref()
                        .is_some_and(|library| library.needs_model_thumbnail(&file));
                    if let (true, Ok(bytes)) = (wanted, bytes) {
                        self.queue_glb_thumbnail(cx, &file, &bytes);
                    }
                    self.pump_thumbnail_backfill(cx);
                }
                IoDone::ThumbAudioWaveform { file, png } => {
                    self.thumb_read_in_flight = false;
                    if let Some(png) = png {
                        self.commit_audio_waveform(cx, &file, &png);
                    }
                    self.pump_thumbnail_backfill(cx);
                }
                IoDone::GalleryPreview {
                    file,
                    cache_source,
                    pixels,
                    sequence,
                    fps,
                } => {
                    self.preview_in_flight.retain(|f| f != &file);
                    let pixel_texture = |cx: &mut Cx, pixels: PreviewPixels| match pixels {
                        PreviewPixels::Encoded(image) => image.into_new_texture(cx),
                        PreviewPixels::Raw {
                            width,
                            height,
                            data,
                        } => Texture::new_with_format(
                            cx,
                            TextureFormat::VecBGRAu8_32 {
                                width,
                                height,
                                data: Some(data),
                                updated: TextureUpdated::Full,
                            },
                        ),
                    };
                    let frames: Vec<Texture> = if sequence.len() > 1 {
                        sequence
                            .into_iter()
                            .map(|p| pixel_texture(cx, p))
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let texture = if let Some(first) = frames.first() {
                        first.clone()
                    } else if let Some(pixels) = pixels {
                        pixel_texture(cx, pixels)
                    } else {
                        let domain = self
                            .library
                            .as_ref()
                            .and_then(|library| library.get(&file))
                            .map(|item| item.domain.clone())
                            .unwrap_or_default();
                        crate::store_views::badge_texture(cx, &domain)
                    };
                    if let Some(mut gallery) = self
                        .ui
                        .widget(cx, ids!(library_gallery))
                        .borrow_mut::<LibraryGallery>()
                    {
                        if frames.len() > 1 {
                            gallery.install_anim_preview(
                                cx,
                                file.clone(),
                                cache_source.clone(),
                                frames.clone(),
                                fps,
                            );
                        } else {
                            gallery.install_preview(
                                cx,
                                file.clone(),
                                cache_source.clone(),
                                texture.clone(),
                            );
                        }
                    }
                    if self.input_chip_file.as_deref() == Some(file.as_str()) {
                        self.ui
                            .image(cx, ids!(input_chip_thumb))
                            .set_texture(cx, Some(texture.clone()));
                    }
                    if let Some(slot) = self.run_tray_files.iter().position(|f| f == &file) {
                        if let Some(chip) = Self::run_chip_ids().get(slot) {
                            let mut chip_thumb = chip.to_vec();
                            chip_thumb.push(live_id!(thumb));
                            self.ui.image(cx, &chip_thumb).set_texture(cx, Some(texture.clone()));
                        }
                    }
                    if let Some(mut grid) = self
                        .ui
                        .widget(cx, ids!(lib_grid))
                        .borrow_mut::<LibraryGrid>()
                    {
                        if frames.len() > 1 {
                            grid.install_anim_preview(cx, file, cache_source, frames, fps);
                        } else {
                            grid.install_preview(cx, file, cache_source, texture);
                        }
                    }
                    self.pump_gallery_previews(cx);
                }
            }
        }
    }

    /// Route draw-recorded preview misses to the LIFO decode pool. Newly
    /// visible cards are requested last so they inflate first; several
    /// cores run at once.
    fn pump_gallery_previews(&mut self, cx: &mut Cx) {
        let mut wanted = Vec::new();
        if let Some(mut gallery) = self
            .ui
            .widget(cx, ids!(library_gallery))
            .borrow_mut::<LibraryGallery>()
        {
            wanted.extend(gallery.take_preview_work());
        }
        if let Some(mut grid) = self
            .ui
            .widget(cx, ids!(lib_grid))
            .borrow_mut::<LibraryGrid>()
        {
            wanted.extend(grid.take_preview_work());
        }
        wanted.append(&mut self.extra_preview_work);
        let Some(io) = &self.artifact_io else { return };
        for (file, work) in wanted {
            if self.preview_in_flight.iter().any(|f| f == &file) {
                continue;
            }
            let (path, purpose) = match work {
                PreviewWork::Encoded(path) => (path, IoPurpose::GalleryPreviewEncoded),
                PreviewWork::WavPayload(path) => (path, IoPurpose::GalleryPreviewWav),
                PreviewWork::StatefulBillboard(path) => {
                    (path, IoPurpose::GalleryPreviewBillboard)
                }
            };
            io.request(IoRequest {
                file: file.clone(),
                path,
                purpose,
            });
            self.preview_in_flight.push(file);
        }
    }

    /// Commit a worker-encoded waveform sidecar (never overwrites, payload
    /// identity revalidated by Library) and refresh with caches kept warm.
    fn commit_audio_waveform(&mut self, cx: &mut Cx, file: &str, png: &[u8]) {
        let Some(library) = &self.library else {
            return;
        };
        if !matches!(library.thumbnail_path(file), Ok(None)) {
            return;
        }
        match library.replace_thumbnail_png(file, png) {
            Ok(()) => {
                log!("library: persisted waveform preview for {file}");
                // Only the new sidecar path re-decodes; every other cached
                // thumbnail stays warm.
                self.refresh_gallery(cx, false);
            }
            Err(error) => log!("library: waveform preview for {file} failed: {error}"),
        }
    }

    fn submit_viewer_open(&self, open: PendingOpen) {
        if let Some(io) = &self.artifact_io {
            io.request(IoRequest {
                file: open.file,
                path: open.path,
                purpose: IoPurpose::ViewerOpen {
                    generation: open.generation,
                    copy_to: open.copy_to,
                },
            });
        }
    }

    /// Select a library card without opening the viewer.
    fn select_gallery(&mut self, cx: &mut Cx, file: &str) {
        let Some(item) = self
            .library
            .as_ref()
            .and_then(|library| library.get(file).cloned())
        else {
            return;
        };
        self.selected_file = Some(item.file.clone());
        self.refresh_gallery(cx, false);
    }

    /// EXPLICIT user pick of a managed asset (History tile, Library card,
    /// detail rail): opens it in the viewer AND pins it as the next
    /// transform run's input.
    fn open_gallery(&mut self, cx: &mut Cx, file: &str) {
        self.open_gallery_impl(cx, file, true);
    }

    /// Automatic reopen (surface return, demo auto-flows): viewer only —
    /// it must never pin the input tray, or "whatever showed up last"
    /// could silently become a transform's input.
    fn reopen_gallery(&mut self, cx: &mut Cx, file: &str) {
        self.open_gallery_impl(cx, file, false);
    }

    fn open_gallery_impl(&mut self, cx: &mut Cx, file: &str, pin_input: bool) {
        let Some(item) = self
            .library
            .as_ref()
            .and_then(|library| library.get(file).cloned())
        else {
            log!("library: {file} is not in the index");
            return;
        };
        // Selection + accent border update INSTANTLY; the viewer clears and
        // enters an explicit loading state; the payload read (and any
        // video/splat artifacts-dir copy) runs on the IO worker. Caption and
        // content COMMIT TOGETHER when the newest click's read lands.
        self.selected_file = Some(item.file.clone());
        // An EXPLICIT user pick also pins the asset as the next transform
        // run's input (exact payload path + stored content type; the chip
        // thumbnail is display-only).
        if pin_input {
            if let Some(path) = self
                .library
                .as_ref()
                .and_then(|library| library.payload_path(&item.file).ok())
            {
                let preview_path = if item.content_type.to_ascii_lowercase().starts_with("image/")
                {
                    Some(path.clone())
                } else {
                    self.library
                        .as_ref()
                        .and_then(|library| library.thumbnail_path(&item.file).ok().flatten())
                };
                self.input_tray.select(InputAsset {
                    file: item.file.clone(),
                    label: item.label.clone(),
                    domain: item.domain.clone(),
                    content_type: item.content_type.clone(),
                    path,
                    preview_path,
                });
                self.sync_input_tray(cx);
            }
        }
        self.enter_viewer_loading(cx, &item.file, &item.label);
        self.refresh_gallery(cx, false);
        let Some(path) = self
            .library
            .as_ref()
            .and_then(|library| library.payload_path(file).ok())
        else {
            return;
        };
        let ct = item.content_type.to_ascii_lowercase();
        let copy_to = if ct.starts_with("video/") {
            Some(artifacts_dir().join("viewer-open.mp4"))
        } else if ct.contains("ply") || item.file.ends_with(".ply") {
            Some(artifacts_dir().join("viewer-open.ply"))
        } else {
            None
        };
        if let Some(open) = self.viewer_gate.click(file, path, copy_to) {
            self.submit_viewer_open(open);
        }
    }

    /// Deleting a run's History group also kills the RUN: cancel the active
    /// stage service-side and drop the run and its queued twins, so a late
    /// artifact or straggler response can never recreate the group (dropped
    /// runs stop owning their network traffic; unclaimed responses are
    /// ignored). An explicit Retry stays possible — that is a deliberate
    /// user action, not a straggler.
    fn suppress_group_runs(&mut self, cx: &mut Cx, group: &str) {
        let mut cancelled = 0usize;
        for run in &mut self.runs {
            if run.group_id == group && run.pipeline.is_running() {
                run.pipeline.cancel_current(cx);
                cancelled += 1;
            }
        }
        let active_before = self.runs.len();
        self.runs.retain(|run| run.group_id != group);
        let queued_before = self.run_queue.len();
        self.run_queue.retain(|run| run.group_id != group);
        let dropped = (active_before - self.runs.len()) + (queued_before - self.run_queue.len());
        if dropped > 0 {
            log!(
                "library: deleted group suppressed {dropped} run(s) ({cancelled} cancelled service-side)"
            );
            self.refresh_run_ui(cx);
            // Freed GPU slots can take queued work immediately.
            self.try_dispatch_pending(cx);
        }
    }

    /// Delete an entire pipeline-run / import group — every payload and
    /// sidecar, one atomic index commit (Library rolls back on failure).
    /// Only a PERSISTED group id is addressable here: the ungrouped
    /// "Earlier imports" records tile and delete one by one, so no UI path
    /// can pass Library the sweep-them-all `None` bucket.
    fn delete_gallery_group(&mut self, cx: &mut Cx, group: &str) {
        self.suppress_group_runs(cx, group);
        let removed = self
            .library
            .as_mut()
            .map(|library| library.remove_group(Some(group)))
            .unwrap_or(Ok(0));
        match removed {
            Ok(0) => {}
            Ok(count) => {
                log!("library: removed run group ({count} items)");
                let selection_gone = self.selected_file.as_deref().is_some_and(|file| {
                    self.library
                        .as_ref()
                        .is_some_and(|library| library.get(file).is_none())
                });
                if selection_gone {
                    self.selected_file = None;
                }
                self.reset_viewer_if_gone(cx);
            }
            Err(error) => {
                log!("library: group delete failed: {error}");
                self.ui
                    .label(cx, ids!(library_hint))
                    .set_text(cx, &format!("group delete failed: {error}"));
                return;
            }
        }
        self.refresh_gallery(cx, true);
    }

    fn delete_gallery(&mut self, cx: &mut Cx, file: &str) {
        let result = self
            .library
            .as_mut()
            .map(|library| library.remove_by_file(file))
            .unwrap_or(Ok(false));
        match result {
            Ok(true) => {
                if self.selected_file.as_deref() == Some(file) {
                    self.selected_file = None;
                }
                // The viewer may be showing/loading the deleted item even
                // when the selection has already moved elsewhere.
                self.reset_viewer_if_gone(cx);
            }
            Ok(false) => log!("library: resource {file} no longer exists"),
            Err(error) => {
                log!("library: delete {file} failed: {error}");
                self.ui.label(cx, ids!(library_hint)).set_text(
                    cx,
                    &format!("delete failed: {error}"),
                );
                return;
            }
        }
        self.refresh_gallery(cx, true);
    }

    // -- surfaces + Library browser --------------------------------------------

    fn show_surface(&mut self, cx: &mut Cx, surface: Surface) {
        // The Create viewer hides behind every other surface: a playing
        // video must stop decoding there too, not only on viewer-page flips.
        if surface != Surface::Create {
            // Nothing plays behind another surface: video stops decoding,
            // WAV playback pauses (resume is one Play click away).
            self.stop_video_playback();
            audio::pause();
        }
        self.surface = surface;
        self.arm_audio_pump(cx);
        // A completion that landed while another surface was up persisted +
        // selected itself without touching the viewer; opening it now goes
        // through the same async loading path as a click.
        if surface == Surface::Create {
            let candidate_run = self
                .display_run()
                .and_then(|run| run.pipeline.active_candidate_set().is_some().then_some(run.id));
            if let Some(run_id) = candidate_run {
                self.refresh_candidate_ui(cx, run_id);
            } else if let Some(selected) = self.selected_file.clone() {
                if self.viewer.needs_reopen(Some(&selected)) {
                    self.reopen_gallery(cx, &selected);
                }
            }
        }
        let page = match surface {
            Surface::Create => id!(create_surface),
            Surface::Chat => id!(chat_surface),
            Surface::Library => id!(library_surface),
            Surface::Import => id!(import_surface),
            Surface::Runs => id!(runs_surface),
            Surface::Admin => id!(admin_surface),
        };
        let flip = self.ui.page_flip(cx, ids!(surfaces));
        log!("show_surface: flip found={} page={:?}", !flip.is_empty(), page);
        flip.set_active_page(cx, page.into());
        for (tab, label, target) in [
            (ids!(nav_create), "CREATE", Surface::Create),
            (ids!(nav_chat), "CHAT", Surface::Chat),
            (ids!(nav_library), "LIBRARY", Surface::Library),
            (ids!(nav_import), "LOAD", Surface::Import),
            (ids!(nav_runs), "RUNS + WORKERS", Surface::Runs),
            (ids!(nav_admin), "ADMIN + AUDIT", Surface::Admin),
        ] {
            let active = surface == target;
            self.ui.button(cx, tab).set_text(
                cx,
                &if active {
                    format!("● {label}")
                } else {
                    label.to_string()
                },
            );
            let color = if active {
                vec4(0.87, 0.92, 0.95, 1.0)
            } else {
                vec4(0.51, 0.54, 0.58, 1.0)
            };
            let mut widget = self.ui.widget(cx, tab);
            script_apply_eval!(cx, widget, {
                draw_text +: { color: #(color) }
            });
        }
        match surface {
            Surface::Create => {}
            Surface::Chat => self.refresh_chat_ui(cx),
            Surface::Library => self.refresh_library_ui(cx, false),
            Surface::Import => self.refresh_import_ui(cx),
            Surface::Runs => self.refresh_runs_panel(cx),
            Surface::Admin => self.refresh_admin_panel(cx),
        }
        self.ui.redraw(cx);
    }

    fn refresh_chat_ui(&mut self, cx: &mut Cx) {
        let mut status = ChatData::status();
        let defaults = self.chat.defaults_summary();
        if !defaults.is_empty() && !status.contains("defaults") {
            if status.is_empty() {
                status = defaults;
            } else {
                status = format!("{status} · {defaults}");
            }
        }
        if let Ok(data) = crate::chat::CHAT.read() {
            if !data.activity.is_empty() && !status.contains(&data.activity) {
                if status.is_empty() {
                    status = data.activity.clone();
                } else {
                    status = format!("{status} · {}", data.activity);
                }
            }
        }
        self.ui
            .label(cx, ids!(chat_status))
            .set_text(cx, &status);
        let streaming = crate::chat::CHAT
            .read()
            .map(|d| d.is_streaming)
            .unwrap_or(false);
        self.ui
            .button(cx, ids!(chat_cancel_btn))
            .set_visible(cx, streaming);
        self.ui.widget(cx, ids!(chat_list)).redraw(cx);
    }

    fn maybe_connect_chat(&mut self) {
        let bases: Vec<String> = self
            .fleet
            .as_ref()
            .map(|fleet| {
                fleet
                    .snapshots
                    .iter()
                    .map(|snap| snap.base_url.clone())
                    .collect()
            })
            .unwrap_or_default();
        let now = std::time::Instant::now();
        let bases_changed = self.chat_fleet_bases != bases;
        let due = self.chat_qwen_retry_at.map(|t| now >= t).unwrap_or(true);
        if bases_changed || (!self.chat.is_connected() && !bases.is_empty() && due) {
            self.chat_fleet_bases = bases.clone();
            self.chat_qwen_retry_at = Some(now + std::time::Duration::from_secs(8));
            self.chat.set_fleet(bases);
        }
        if !self.chat_asset_linked {
            if let (Some(endpoints), Some(server)) =
                (self.store.endpoints, self.store.server.as_ref())
            {
                let cache = session_config_from_env().cache_parent.join("cache-chat");
                self.chat.connect_asset(
                    endpoints,
                    self.store.token.clone(),
                    cache,
                    server.server_id,
                );
                self.chat_asset_linked = true;
            }
        }
        self.push_fleet_view();
    }

    fn push_fleet_view(&self) {
        let Some(fleet) = &self.fleet else {
            return;
        };
        self.chat
            .update_fleet(FleetView::from_snapshots(&fleet.snapshots));
    }

    fn drain_chat_jobs(&mut self, cx: &mut Cx) {
        let jobs = self.chat.take_jobs();
        if jobs.is_empty() {
            return;
        }
        for job in jobs {
            self.enqueue_chat_job(job);
        }
        self.try_dispatch_pending(cx);
        self.refresh_chat_ui(cx);
        self.ui.redraw(cx);
    }

    fn enqueue_chat_job(&mut self, job: ChatJob) {
        let preset_name = job.kind.preset_name(job.then, job.model.as_deref());
        let Some(preset) = PRESETS.iter().position(|p| p.name == preset_name) else {
            ChatData::push(
                ChatRole::System,
                format!("no preset named {preset_name}"),
            );
            return;
        };
        let model_overrides = job
            .model
            .as_ref()
            .map(|model| vec![(job.kind.model_domain().to_string(), model.clone())])
            .unwrap_or_default();
        let group_label = format!(
            "{} — \"{}\"",
            PRESETS[preset].name,
            truncate(job.prompt.trim(), 24)
        );
        let note = match job.kind {
            crate::chat::ChatJobKind::Video => format!(
                "queued {preset_name} · {}×{} · {}f · {}",
                job.width,
                job.height,
                job.frames,
                job.model.as_deref().unwrap_or("affinity")
            ),
            crate::chat::ChatJobKind::Music => format!(
                "queued {preset_name} · {}s · {}",
                job.seconds,
                job.model.as_deref().unwrap_or("affinity")
            ),
            crate::chat::ChatJobKind::Audio | crate::chat::ChatJobKind::Speech => format!(
                "queued {preset_name} · {}",
                job.model.as_deref().unwrap_or("affinity")
            ),
            _ => format!(
                "queued {preset_name} · {}×{} · {}",
                job.width,
                job.height,
                job.model.as_deref().unwrap_or("affinity")
            ),
        };
        let mut gen = GenParams::default();
        if job.width > 0 && job.height > 0 {
            match job.kind {
                crate::chat::ChatJobKind::Video => {
                    gen.video_size = (job.width, job.height);
                    gen.video_frames = job.frames;
                    gen.video_steps = job.video_steps;
                }
                _ => {
                    gen.image_size = (job.width, job.height);
                    gen.image_steps = job.steps;
                }
            }
        }
        if job.seconds > 0 {
            gen.music_seconds = job.seconds;
        }
        self.run_queue.push(PendingRun {
            group_id: crate::library::new_group_id("run"),
            group_label,
            prompt: job.prompt,
            preset,
            model_overrides,
            box_override: None,
            voice: job.voice,
            gen,
            input: None,
        });
        ChatData::push(ChatRole::System, note);
    }

    fn send_chat(&mut self, cx: &mut Cx) {
        let text = self
            .ui
            .text_input(cx, ids!(chat_input))
            .text()
            .trim()
            .to_string();
        if text.is_empty() {
            return;
        }
        self.ui.text_input(cx, ids!(chat_input)).set_text(cx, "");
        let mut attachments = Vec::new();
        if let Some(detail) = self.store.detail.ready() {
            if let Some(head) = detail.latest_published() {
                attachments.push(makepad_asset_client::ChatAttachment {
                    revision: head.revision,
                    role: "source".into(),
                });
            }
        }
        self.chat.send(text, attachments);
        self.refresh_chat_ui(cx);
        self.ui.redraw(cx);
    }

    fn set_lib_source(&mut self, cx: &mut Cx, source: LibSource) {
        self.lib_source = source;
        let page = match source {
            LibSource::Local => id!(lib_local_page),
            LibSource::Server => id!(lib_server_page),
        };
        self.ui
            .page_flip(cx, ids!(lib_pages))
            .set_active_page(cx, page.into());
        self.refresh_library_ui(cx, false);
    }

    /// Pull the four filter controls into the local filter state and mirror
    /// them onto the real server query. A changed server query cancels and
    /// replaces its in-flight search; presentation-only refreshes do not.
    fn read_filters_from_ui(&mut self, cx: &mut Cx) {
        let query = self.ui.text_input(cx, ids!(lib_search)).text();
        let kind = None;
        let category = self.lib_filters.tags.iter().find_map(|tag| {
            matches!(
                tag.as_str(),
                "maps"
                    | "characters"
                    | "props"
                    | "weapons"
                    | "billboards"
                    | "images"
                    | "video"
                    | "music"
                    | "speech"
                    | "sfx"
                    | "splats"
                    | "meshes"
                    | "other"
            )
            .then(|| tag.clone())
        });
        let server_kind = kind
            .as_deref()
            .and_then(|label| {
                SERVER_KINDS
                    .into_iter()
                    .find(|candidate| server_kind_label(*candidate) == label)
            });
        let server_category = category.clone();
        let server_tag = self.lib_filters.tags.first().cloned();
        let server_changed = self.store.filters.text != query
            || self.store.filters.category != server_category
            || self.store.filters.kind != server_kind
            || self.store.filters.tag != server_tag;
        self.store.filters.text = query.clone();
        self.store.filters.category = server_category;
        self.store.filters.kind = server_kind;
        self.store.filters.tag = server_tag;
        self.lib_filters.query = query;
        self.lib_filters.category = category;
        self.lib_filters.kind = kind;
        if server_changed {
            self.store.submit_search();
        }
    }

    /// Library surface: source tabs, filter options, grid, counts, server
    /// pane and the detail rail. `clear_thumbnails` forces thumbnail
    /// re-decode (library content changed); filter edits keep the cache.
    fn run_enhance_metadata(&mut self, cx: &mut Cx) {
        let Some(library) = &mut self.library else {
            return;
        };
        let named = crate::enhance_meta::apply_catalog_names(library);
        let models: Vec<String> = match &self.store.profiles {
            Remote::Ready(profiles) => profiles
                .iter()
                .flat_map(|p| [p.id.clone(), p.domain.clone(), p.kind.clone()])
                .collect(),
            _ => Vec::new(),
        };
        let vision = crate::enhance_meta::fleet_has_vision(&models);
        let dir = std::path::PathBuf::from(repo_path("local/ai_content_library"));
        let skipped = if vision {
            0
        } else {
            crate::enhance_meta::stamp_skipped_vision(
                &dir,
                &library.index.items,
                "Qwen-VL not provisioned on this fleet — name table applied, no vision blob",
            )
        };
        log!(
            "enhance-metadata: renamed {named} · vision {} · stamped {skipped}",
            if vision { "ready (captions not wired this pass)" } else { "skipped" }
        );
        self.ui.label(cx, ids!(lib_count)).set_text(
            cx,
            &if vision {
                format!("enhanced {named} names · vision model present (caption loop next)")
            } else {
                format!("enhanced {named} names · no Qwen-VL on fleet (skipped {skipped})")
            },
        );
        self.refresh_library_ui(cx, false);
    }

    fn refresh_library_ui(&mut self, cx: &mut Cx, clear_thumbnails: bool) {
        let local = self.lib_source == LibSource::Local;
        let total = self.library.as_ref().map_or(0, |library| library.len());
        self.ui.button(cx, ids!(lib_local_tab)).set_text(
            cx,
            &if local {
                format!("● Local ({total})")
            } else {
                format!("Local ({total})")
            },
        );
        self.ui
            .button(cx, ids!(lib_server_tab))
            .set_text(cx, if local { "Server" } else { "● Server" });

        let tag_stats = match &self.library {
            Some(library) => collect_tag_stats(library.newest_items()),
            None => Vec::new(),
        };
        let tag_labels: Vec<String> = if tag_stats.is_empty() {
            vec!["None yet".to_string()]
        } else {
            tag_stats
                .iter()
                .map(|stat| {
                    let on = self
                        .lib_filters
                        .tags
                        .iter()
                        .any(|selected| selected.eq_ignore_ascii_case(&stat.name));
                    let mark = if on { "● " } else { "" };
                    let star = if stat.enhanced { "✦ " } else { "" };
                    format!("{mark}{star}{}   {}", stat.name, stat.count)
                })
                .collect()
        };
        let drop = self.ui.drop_down2(cx, ids!(lib_tag_drop));
        drop.set_labels(cx, tag_labels);
        self.lib_tag_options = tag_stats;

        let chips = self.lib_filters.tags.clone();
        self.ui
            .widget(cx, ids!(lib_tag_chips))
            .set_visible(cx, !chips.is_empty());
        for i in 0..8 {
            set_lib_tag_chip(&self.ui, cx, i, chips.get(i).map(String::as_str));
        }

        self.read_filters_from_ui(cx);

        // Filtered local grid.
        let entries = match &self.library {
            Some(library) => library
                .newest_items()
                .filter(|item| {
                    self.lib_filters.matches(
                        &item.label,
                        &item.prompt,
                        &item.domain,
                        &item.content_type,
                        &item.filter_tags(),
                    )
                })
                .filter_map(|item| {
                    let path = library.payload_path(&item.file).ok()?;
                    let ct = item.content_type.to_ascii_lowercase();
                    let preview_path = if item.domain.eq_ignore_ascii_case("billboard")
                        || ct.contains("billboard")
                    {
                        Some(path.clone())
                    } else if ct.starts_with("image/") {
                        Some(path.clone())
                    } else {
                        library.thumbnail_path(&item.file).ok().flatten()
                    };
                    Some(GalleryEntry {
                        meta: item.clone(),
                        path,
                        preview_path,
                        selected: self.selected_file.as_deref() == Some(item.file.as_str()),
                    })
                })
                .collect::<Vec<_>>(),
            None => Vec::new(),
        };
        let shown = entries.len();
        if let Some(mut grid) = self
            .ui
            .widget(cx, ids!(lib_grid))
            .borrow_mut::<LibraryGrid>()
        {
            grid.set_entries(cx, entries, clear_thumbnails);
        }
        let count_text = match self.lib_source {
            LibSource::Local => format!("{shown} shown · {total} local"),
            LibSource::Server => match &self.store.search {
                Remote::Ready(results) => format!(
                    "{} shown · {} on server{}",
                    results.hits.len(),
                    results.total,
                    if results.more { " · more available" } else { "" }
                ),
                Remote::Loading => "searching server…".to_string(),
                Remote::Failed(error) => format!("server search failed · {error}"),
                Remote::Idle if self.store.connected() => "server catalog not loaded".to_string(),
                Remote::Idle => "server disconnected · 0 assets".to_string(),
            },
        };
        self.ui.label(cx, ids!(lib_count)).set_text(cx, &count_text);

        // Server pane: the actual discovery/auth/session state and typed
        // catalog rows. Empty/loading/failure are intentionally distinct.
        let server_note = self.store.status_label();
        self.ui
            .label(cx, ids!(lib_server_note))
            .set_text(cx, &server_note);
        if let Some(mut list) = self
            .ui
            .widget(cx, ids!(lib_server_list))
            .borrow_mut::<StoreListPanel>()
        {
            list.set_rows(cx, catalog_rows(&self.store));
        }
        self.ui
            .label(cx, ids!(remote_connection))
            .set_text(cx, &self.store.status_label());

        self.refresh_library_detail(cx);
        self.ui.redraw(cx);
    }

    /// Selected-item rail. Local items show exactly what the local index
    /// records (and say so); server items show revision/provenance/publish
    /// from the snapshot. Nothing is invented for either side.
    fn refresh_library_detail(&mut self, cx: &mut Cx) {
        struct Detail {
            badge: String,
            title: String,
            meta: String,
            head_a: &'static str,
            prompt: String,
            prov: String,
            rev: String,
            publish: String,
            local_actions: bool,
        }
        let empty = |title: &str, meta: &str| Detail {
            badge: String::new(),
            title: title.to_string(),
            meta: meta.to_string(),
            head_a: "Prompt",
            prompt: "—".into(),
            prov: "—".into(),
            rev: "—".into(),
            publish: "—".into(),
            local_actions: false,
        };
        let detail = match self.lib_source {
            LibSource::Local => {
                let selected = self.selected_file.clone().and_then(|file| {
                    self.library.as_ref().and_then(|library| {
                        let item = library.get(&file)?.clone();
                        let size = library
                            .payload_path(&file)
                            .ok()
                            .and_then(|path| std::fs::metadata(path).ok())
                            .map(|meta| meta.len());
                        Some((item, size))
                    })
                });
                match selected {
                    Some((item, size)) => Detail {
                        badge: kind_label(&item.domain, &item.content_type).to_ascii_uppercase(),
                        title: item.label.clone(),
                        meta: {
                            let tags = item.filter_tags();
                            let tag_bit = if tags.is_empty() {
                                String::new()
                            } else {
                                format!(" · tags {}", tags.join(", "))
                            };
                            format!(
                                "{} · {} · {} · {}{tag_bit}",
                                item.domain,
                                item.content_type,
                                size.map(format_bytes)
                                    .unwrap_or_else(|| "size unknown".into()),
                                item.file
                            )
                        },
                        head_a: "Prompt",
                        prompt: if item.prompt.trim().is_empty() {
                            "(no prompt recorded)".into()
                        } else {
                            item.prompt.clone()
                        },
                        prov: "Produced by this app (local pipeline run or explicit import). \
                               The local index records domain, content type and prompt; fleet \
                               candidate labels additionally preserve endpoint, model, seed, \
                               remote artifact id, digest prefix and byte length."
                            .into(),
                        rev: "Single local copy — History keeps the latest payload only.".into(),
                        publish: "Local only — not published to any Asset Store server.".into(),
                        local_actions: true,
                    },
                    None => empty("Nothing selected", "Click a thumbnail to see its details."),
                }
            }
            LibSource::Server => {
                if !self.store.connected() {
                    empty(
                        "Asset Server not connected",
                        &self.store.status_label(),
                    )
                } else {
                    match self.store.selected {
                        None => empty("Nothing selected", "Click a catalog row to inspect it."),
                        Some(selected) => {
                            let hit = self
                                .store
                                .search
                                .ready()
                                .and_then(|results| {
                                    results.hits.iter().find(|hit| hit.asset_id == selected)
                                });
                            let badge = hit
                                .and_then(|hit| hit.kind)
                                .map(server_kind_label)
                                .unwrap_or("asset")
                                .to_ascii_uppercase();
                            let title = hit
                                .map(|hit| hit.title.clone())
                                .unwrap_or_else(|| format!("Asset {selected}"));
                            let meta = hit.map_or_else(
                                || format!("asset {selected}"),
                                |hit| {
                                    let alias = hit
                                        .alias
                                        .as_ref()
                                        .map(ToString::to_string)
                                        .unwrap_or_else(|| "no alias".to_string());
                                    format!(
                                        "{alias} · namespace {} · {}",
                                        hit.namespace,
                                        if hit.live { "live" } else { "not live" }
                                    )
                                },
                            );
                            let snippet = hit
                                .map(|hit| hit.snippet.clone())
                                .filter(|text| !text.trim().is_empty())
                                .unwrap_or_else(|| "(no search snippet returned)".to_string());
                            let (provenance, revisions, publish) = match &self.store.detail {
                                Remote::Idle => (
                                    "Detail has not been requested.".to_string(),
                                    "—".to_string(),
                                    "—".to_string(),
                                ),
                                Remote::Loading => (
                                    "Loading authoritative server detail…".to_string(),
                                    "loading…".to_string(),
                                    "loading…".to_string(),
                                ),
                                Remote::Failed(error) => (
                                    format!("Server detail request failed: {error}"),
                                    "unavailable".to_string(),
                                    "unavailable".to_string(),
                                ),
                                Remote::Ready(detail) if detail.asset_id == selected => {
                                    let revisions = if detail.candidates.is_empty() {
                                        "no candidates returned".to_string()
                                    } else {
                                        detail
                                            .candidates
                                            .iter()
                                            .map(|candidate| {
                                                let revision = candidate.revision.to_string();
                                                let when = candidate
                                                    .published_ms
                                                    .or(candidate.quarantined_ms)
                                                    .unwrap_or(candidate.staged_ms);
                                                format!(
                                                    "{} · {} · {} ms",
                                                    short_digest(&revision),
                                                    candidate.state.as_str(),
                                                    when
                                                )
                                            })
                                            .collect::<Vec<_>>()
                                            .join("\n")
                                    };
                                    let publish = detail
                                        .latest_published()
                                        .map(|candidate| {
                                            let revision = candidate.revision.to_string();
                                            format!(
                                                "latest published: {} · {} ms",
                                                short_digest(&revision),
                                                candidate.published_ms.unwrap_or_default()
                                            )
                                        })
                                        .unwrap_or_else(|| "no published revision".to_string());
                                    (
                                        format!(
                                            "namespace {} · asset {}\nManifest files and generator provenance are not part of the detail response.",
                                            detail.namespace, detail.asset_id
                                        ),
                                        revisions,
                                        publish,
                                    )
                                }
                                Remote::Ready(_) => (
                                    "Waiting for the selected asset detail…".to_string(),
                                    "loading…".to_string(),
                                    "loading…".to_string(),
                                ),
                            };
                            Detail {
                                badge,
                                title,
                                meta,
                                head_a: "Search match",
                                prompt: snippet,
                                prov: provenance,
                                rev: revisions,
                                publish,
                                local_actions: false,
                            }
                        }
                    }
                }
            }
        };
        self.ui
            .widget(cx, ids!(detail_badge))
            .set_visible(cx, !detail.badge.is_empty());
        self.ui
            .label(cx, ids!(detail_badge_label))
            .set_text(cx, &detail.badge);
        self.ui
            .label(cx, ids!(detail_title))
            .set_text(cx, &detail.title);
        self.ui
            .label(cx, ids!(detail_meta))
            .set_text(cx, &detail.meta);
        self.ui
            .label(cx, ids!(detail_head_a))
            .set_text(cx, detail.head_a);
        self.ui
            .label(cx, ids!(detail_prompt))
            .set_text(cx, &detail.prompt);
        self.ui
            .label(cx, ids!(detail_prov))
            .set_text(cx, &detail.prov);
        self.ui.label(cx, ids!(detail_rev)).set_text(cx, &detail.rev);
        self.ui
            .label(cx, ids!(detail_publish))
            .set_text(cx, &detail.publish);
        self.ui
            .widget(cx, ids!(detail_actions))
            .set_visible(cx, detail.local_actions);
    }

    /// Runs+Workers surface rows: local pipeline + queue + LAN fleet (all
    /// real) and the honestly-disconnected server section.
    fn refresh_runs_panel(&mut self, cx: &mut Cx) {
        let queued: Vec<String> = self
            .run_queue
            .iter()
            .map(|run| {
                format!(
                    "{} — \u{201c}{}\u{201d}",
                    PRESETS[run.preset].name,
                    truncate(&run.prompt, 40)
                )
            })
            .collect();
        // Newest first; every concurrent run shows its own stages + Stop.
        let run_views: Vec<crate::store_views::RunView> = self
            .runs
            .iter()
            .rev()
            .map(|run| crate::store_views::RunView {
                id: run.id,
                label: run.group_label.as_str(),
                pipeline: &run.pipeline,
            })
            .collect();
        let rows = match &self.fleet {
            Some(fleet) => runs_rows(
                &run_views,
                &queued,
                &fleet.snapshots,
                &fleet.latency_ms,
                &self.store,
            ),
            None => runs_rows(&run_views, &queued, &[], &[], &self.store),
        };
        if let Some(mut list) = self
            .ui
            .widget(cx, ids!(runs_list))
            .borrow_mut::<StoreListPanel>()
        {
            list.set_rows(cx, rows);
        }
    }

    fn refresh_admin_panel(&mut self, cx: &mut Cx) {
        if let Some(mut list) = self
            .ui
            .widget(cx, ids!(admin_list))
            .borrow_mut::<StoreListPanel>()
        {
            list.set_rows(cx, admin_rows(&self.store));
        }
    }

    fn refresh_import_ui(&mut self, cx: &mut Cx) {
        let _modules = crate::import_classic::PACK_MODULES_WITH_CLASSIC;
        let labels = crate::import::kenney_pack_labels();
        let drop = self.ui.drop_down2(cx, ids!(kenney_pack_drop));
        drop.set_labels(cx, labels);
        drop.set_selected_item(cx, self.import_page.kenney_pack_index);
        self.refresh_import_preview_strip(cx);
        self.refresh_import_queue_list(cx);
        let show_preview = !self.import_page.preview_thumbs.is_empty()
            && (self.import_busy()
                || self.import_queue.active.is_some()
                || !self.import_landings.is_empty()
                || self.import_page.icons_busy());
        self.ui
            .view(cx, ids!(import_preview))
            .set_visible(cx, show_preview);
        self.ui
            .button(cx, ids!(queue_clear_btn))
            .set_visible(cx, !self.import_queue.pending.is_empty() || self.import_busy());
        let kenney_job = ImportJob::Kenney {
            pack: self.import_page.selected_pack_id().0,
            pack_index: self.import_page.kenney_pack_index,
            path: String::new(),
        };
        self.ui.button(cx, ids!(kenney_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&kenney_job) && self.import_page.compiling() {
                "Loading…"
            } else if self.import_queue.has_job(&kenney_job) {
                "Waiting"
            } else {
                "Load"
            },
        );
        self.ui.button(cx, ids!(kenney_import_all_btn)).set_text(
            cx,
            if self.import_queue.is_active(&ImportJob::KenneyAll) && self.import_page.compiling()
            {
                "Loading all…"
            } else if self.import_queue.has_job(&ImportJob::KenneyAll) {
                "Waiting"
            } else {
                "Load all"
            },
        );

        let fd_job = ImportJob::Freedoom { path: String::new() };
        self.ui.button(cx, ids!(freedoom_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&fd_job)
                && self.classic_import_page.freedoom.compiling()
            {
                "Loading…"
            } else if self.import_queue.has_job(&fd_job) {
                "Waiting"
            } else {
                "Load"
            },
        );
        let doom_job = ImportJob::Doom { path: String::new() };
        self.ui.button(cx, ids!(doom_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&doom_job)
                && self.classic_import_page.doom.compiling()
            {
                "Loading…"
            } else if self.import_queue.has_job(&doom_job) {
                "Waiting"
            } else {
                "Load"
            },
        );
        let lq_job = ImportJob::LibreQuake { path: String::new() };
        self.ui.button(cx, ids!(librequake_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&lq_job)
                && self.classic_import_page.librequake.compiling()
            {
                "Loading…"
            } else if self.import_queue.has_job(&lq_job) {
                "Waiting"
            } else {
                "Load"
            },
        );
        let quake_job = ImportJob::Quake { path: String::new() };
        self.ui.button(cx, ids!(quake_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&quake_job)
                && self.classic_import_page.quake.compiling()
            {
                "Loading…"
            } else if self.import_queue.has_job(&quake_job) {
                "Waiting"
            } else {
                "Load"
            },
        );
        let duke_job = ImportJob::Duke3d { path: String::new() };
        self.ui.button(cx, ids!(duke3d_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&duke_job)
                && self.classic_import_page.duke3d.compiling()
            {
                "Loading…"
            } else if self.import_queue.has_job(&duke_job) {
                "Waiting"
            } else {
                "Load"
            },
        );
        let q2_job = ImportJob::Quake2 { path: String::new() };
        self.ui.button(cx, ids!(quake2_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&q2_job)
                && self.classic_import_page.quake2.compiling()
            {
                "Loading…"
            } else if self.import_queue.has_job(&q2_job) {
                "Waiting"
            } else {
                "Load"
            },
        );
        let q3_job = ImportJob::Quake3 { path: String::new() };
        self.ui.button(cx, ids!(quake3_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&q3_job)
                && self.classic_import_page.quake3.compiling()
            {
                "Loading…"
            } else if self.import_queue.has_job(&q3_job) {
                "Waiting"
            } else {
                "Load"
            },
        );
        let dm_job = ImportJob::DarkMod { path: String::new() };
        self.ui.button(cx, ids!(darkmod_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&dm_job)
                && self.classic_import_page.darkmod.compiling()
            {
                "Loading…"
            } else if self.import_queue.has_job(&dm_job) {
                "Waiting"
            } else {
                "Load"
            },
        );

        self.ui.button(cx, ids!(kaykit_import_btn)).set_text(
            cx,
            if self.import_queue.is_active(&ImportJob::KayKit) && self.import_page.compiling() {
                "Loading…"
            } else if self.import_queue.has_job(&ImportJob::KayKit) {
                "Waiting"
            } else {
                "Load"
            },
        );

        self.ui
            .label(cx, ids!(remote_connection))
            .set_text(cx, &self.store.status_label());
        self.ui.redraw(cx);
    }

    fn import_busy(&self) -> bool {
        self.import_page.compiling() || self.classic_import_page.compiling()
    }

    fn refresh_import_queue_list(&mut self, cx: &mut Cx) {
        let mut rows = Vec::new();
        let landing_left = self.import_landings.len();
        if let Some(active) = &self.import_queue.active {
            let (title, mut meta, mut progress, failed) = match &active.job {
                ImportJob::Kenney { .. } | ImportJob::KenneyAll | ImportJob::KayKit => (
                    active.job.title(),
                    self.import_page.kenney_status_line(self.store.connected()),
                    self.import_page.progress_fraction(),
                    matches!(
                        self.import_page.kenney_phase,
                        crate::import::ImportPhase::Failed { .. }
                    ),
                ),
                ImportJob::Freedoom { .. } => (
                    active.job.title(),
                    self.classic_import_page
                        .freedoom
                        .status_line(self.store.connected()),
                    self.classic_import_page.freedoom.progress_fraction(),
                    matches!(
                        self.classic_import_page.freedoom.phase,
                        crate::import::ImportPhase::Failed { .. }
                    ),
                ),
                ImportJob::Doom { .. } => (
                    active.job.title(),
                    self.classic_import_page
                        .doom
                        .status_line(self.store.connected()),
                    self.classic_import_page.doom.progress_fraction(),
                    matches!(
                        self.classic_import_page.doom.phase,
                        crate::import::ImportPhase::Failed { .. }
                    ),
                ),
                ImportJob::LibreQuake { .. } => (
                    active.job.title(),
                    self.classic_import_page
                        .librequake
                        .status_line(self.store.connected()),
                    self.classic_import_page.librequake.progress_fraction(),
                    matches!(
                        self.classic_import_page.librequake.phase,
                        crate::import::ImportPhase::Failed { .. }
                    ),
                ),
                ImportJob::Quake { .. } => (
                    active.job.title(),
                    self.classic_import_page
                        .quake
                        .status_line(self.store.connected()),
                    self.classic_import_page.quake.progress_fraction(),
                    matches!(
                        self.classic_import_page.quake.phase,
                        crate::import::ImportPhase::Failed { .. }
                    ),
                ),
                ImportJob::Duke3d { .. } => (
                    active.job.title(),
                    self.classic_import_page
                        .duke3d
                        .status_line(self.store.connected()),
                    self.classic_import_page.duke3d.progress_fraction(),
                    matches!(
                        self.classic_import_page.duke3d.phase,
                        crate::import::ImportPhase::Failed { .. }
                    ),
                ),
                ImportJob::Quake2 { .. } => (
                    active.job.title(),
                    self.classic_import_page
                        .quake2
                        .status_line(self.store.connected()),
                    self.classic_import_page.quake2.progress_fraction(),
                    matches!(
                        self.classic_import_page.quake2.phase,
                        crate::import::ImportPhase::Failed { .. }
                    ),
                ),
                ImportJob::Quake3 { .. } => (
                    active.job.title(),
                    self.classic_import_page
                        .quake3
                        .status_line(self.store.connected()),
                    self.classic_import_page.quake3.progress_fraction(),
                    matches!(
                        self.classic_import_page.quake3.phase,
                        crate::import::ImportPhase::Failed { .. }
                    ),
                ),
                ImportJob::DarkMod { .. } => (
                    active.job.title(),
                    self.classic_import_page
                        .darkmod
                        .status_line(self.store.connected()),
                    self.classic_import_page.darkmod.progress_fraction(),
                    matches!(
                        self.classic_import_page.darkmod.phase,
                        crate::import::ImportPhase::Failed { .. }
                    ),
                ),
            };
            if landing_left > 0 && !self.import_busy() {
                meta = format!("adding to library · {landing_left} left");
                progress = 0.92;
            }
            rows.push(StoreRow::Stage {
                title,
                meta,
                progress,
                failed,
                cancel: Some(RowAction::StopImport),
            });
        }
        for item in &self.import_queue.pending {
            rows.push(StoreRow::Queued {
                title: format!("waiting · {}", item.job.title()),
                cancel: RowAction::RemoveQueuedImport(item.id),
            });
        }
        if rows.is_empty() {
            rows.push(StoreRow::Note(
                "Nothing importing. Add a pack below.".into(),
            ));
        }
        let row_count = rows.len().max(1);
        if let Some(mut list) = self
            .ui
            .widget(cx, ids!(import_queue_list))
            .borrow_mut::<StoreListPanel>()
        {
            list.set_rows(cx, rows);
        }
        let mut list = self.ui.widget(cx, ids!(import_queue_list));
        let height = 8.0 + row_count as f32 * 36.0;
        script_apply_eval!(cx, list, { height: #(height) });
    }

    fn refresh_import_preview_strip(&mut self, cx: &mut Cx) {
        if !self.import_page.preview_dirty {
            return;
        }
        self.import_page.preview_dirty = false;
        let slots = [
            ids!(import_t0),
            ids!(import_t1),
            ids!(import_t2),
            ids!(import_t3),
            ids!(import_t4),
            ids!(import_t5),
            ids!(import_t6),
            ids!(import_t7),
        ];
        for (i, slot) in slots.iter().enumerate() {
            match self.import_page.preview_thumbs.get(i) {
                Some((_, png)) => {
                    if let Err(error) = self.ui.image(cx, *slot).load_png_from_data(cx, png) {
                        log!("import preview decode failed: {error:?}");
                        self.ui.image(cx, *slot).set_texture(cx, None);
                    }
                }
                None => {
                    self.ui.image(cx, *slot).set_texture(cx, None);
                }
            }
        }
    }

    fn enqueue_import(&mut self, cx: &mut Cx, job: ImportJob) {
        match self.import_queue.enqueue(job) {
            Ok(_) => self.kick_import_queue(cx),
            Err(error) => {
                log!("import: {error}");
                if self.surface == Surface::Import {
                    self.refresh_import_ui(cx);
                }
            }
        }
    }

    fn on_classic_download_event(&mut self, cx: &mut Cx) {
        if self.surface == Surface::Import {
            self.refresh_import_ui(cx);
        }
        if !self.import_busy() && self.import_landings.is_empty() {
            self.kick_import_queue(cx);
        }
    }

    fn kick_import_queue(&mut self, cx: &mut Cx) {
        if self.import_busy() || !self.import_landings.is_empty() {
            if self.surface == Surface::Import {
                self.refresh_import_ui(cx);
            }
            return;
        }
        self.import_queue.finish_active();
        let Some(item) = self.import_queue.promote() else {
            if self.surface == Surface::Import {
                self.refresh_import_ui(cx);
            }
            return;
        };
        self.start_import_job(cx, item);
    }

    fn start_import_job(&mut self, cx: &mut Cx, item: crate::import::QueuedImport) {
        self.import_page.reset_session_ui();
        let server = self.import_server_session();
        let result = match &item.job {
            ImportJob::Kenney {
                pack_index, path, ..
            } => {
                self.import_page.set_pack_index(*pack_index);
                self.import_page.start_kenney_import(path.clone(), server)
            }
            ImportJob::KenneyAll => self.import_page.start_kenney_import_all(server),
            ImportJob::Freedoom { path } => self
                .classic_import_page
                .freedoom
                .start_import(cx, path.clone(), server),
            ImportJob::Doom { path } => self
                .classic_import_page
                .doom
                .start_import(cx, path.clone(), server),
            ImportJob::LibreQuake { path } => self
                .classic_import_page
                .librequake
                .start_import(cx, path.clone(), server),
            ImportJob::Quake { path } => self
                .classic_import_page
                .quake
                .start_import(cx, path.clone(), server),
            ImportJob::Duke3d { path } => self
                .classic_import_page
                .duke3d
                .start_import(cx, path.clone(), server),
            ImportJob::Quake2 { path } => self
                .classic_import_page
                .quake2
                .start_import(cx, path.clone(), server),
            ImportJob::Quake3 { path } => self
                .classic_import_page
                .quake3
                .start_import(cx, path.clone(), server),
            ImportJob::DarkMod { path } => self
                .classic_import_page
                .darkmod
                .start_import(cx, path.clone(), server),
            ImportJob::KayKit => self.import_page.start_kaykit_import(server),
        };
        if let Err(error) = result {
            log!("import: {} refused: {error}", item.job.title());
            self.import_queue.finish_active();
            self.kick_import_queue(cx);
            return;
        }
        if let Some(mut renderer) = self
            .ui
            .widget(cx, ids!(thumbnail_renderer))
            .borrow_mut::<ThumbnailRenderer>()
        {
            renderer.clear_thumbnail_queue(cx);
        }
        log!("import: started {}", item.job.title());
        if self.surface == Surface::Import {
            self.refresh_import_ui(cx);
        }
    }

    fn stop_active_import(&mut self, cx: &mut Cx) {
        match self.import_queue.active.as_ref().map(|item| &item.job) {
            Some(ImportJob::Kenney { .. } | ImportJob::KenneyAll | ImportJob::KayKit) => {
                self.import_page.request_stop();
            }
            Some(ImportJob::Freedoom { .. }) => {
                self.classic_import_page.freedoom.request_stop(cx);
            }
            Some(ImportJob::Doom { .. }) => {
                self.classic_import_page.doom.request_stop(cx);
            }
            Some(ImportJob::LibreQuake { .. }) => {
                self.classic_import_page.librequake.request_stop(cx);
            }
            Some(ImportJob::Quake { .. }) => {
                self.classic_import_page.quake.request_stop(cx);
            }
            Some(ImportJob::Duke3d { .. }) => {
                self.classic_import_page.duke3d.request_stop(cx);
            }
            Some(ImportJob::Quake2 { .. }) => {
                self.classic_import_page.quake2.request_stop(cx);
            }
            Some(ImportJob::Quake3 { .. }) => {
                self.classic_import_page.quake3.request_stop(cx);
            }
            Some(ImportJob::DarkMod { .. }) => {
                self.classic_import_page.darkmod.request_stop(cx);
            }
            None => {}
        }
        if self.surface == Surface::Import {
            self.refresh_import_ui(cx);
        }
    }

    fn import_server_session(&self) -> Option<crate::import::ServerSession> {
        let endpoints = self.store.endpoints?;
        let token = self.store.token.clone()?;
        let server_id = self.store.server.as_ref()?.server_id;
        Some(crate::import::ServerSession {
            endpoints,
            token,
            server_id,
        })
    }

    fn collect_import_landings(&mut self) {
        self.import_landings
            .extend(self.import_page.take_library_landings());
        self.import_landings
            .extend(self.classic_import_page.take_all_landings());
    }

    fn drain_import_previews(&mut self) {
        for (name, png) in self.classic_import_page.take_all_previews() {
            self.import_page.push_preview_thumb(name, png);
        }
    }

    /// Persist a bounded slice of imported payloads so convert/AO can keep
    /// painting the queue strip instead of freezing the UI on 800 files.
    fn land_imported_pack(&mut self, cx: &mut Cx) {
        self.collect_import_landings();
        const BATCH: usize = 32;
        if self.import_landings.is_empty() {
            return;
        }
        let n = BATCH.min(self.import_landings.len());
        let landings: Vec<_> = self.import_landings.drain(..n).collect();
        let mut queued = 0usize;
        let mut reused = 0usize;
        let mut thumbs: Vec<(String, Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>)> = Vec::new();
        let mut preview_images: Vec<(String, Vec<u8>)> = Vec::new();
        let mut icon_tracks: Vec<(String, Option<Vec<u8>>)> = Vec::new();
        if let Some(library) = &mut self.library {
            for landing in &landings {
                let Ok(mut bytes) = std::fs::read(&landing.path) else {
                    log!("import: cannot read {}", landing.path.display());
                    continue;
                };
                if landing.content_type.contains("gltf") {
                    if let Some(dir) = landing.path.parent() {
                        match crate::import::embed_glb_file_images(&bytes, dir) {
                            Ok(embedded) => bytes = embedded,
                            Err(error) => log!(
                                "import: embed textures {}: {error}",
                                landing.label
                            ),
                        }
                    }
                }
                let source_id = if landing.source_id.is_empty() {
                    "import".to_string()
                } else {
                    landing.source_id.clone()
                };
                let pack = if landing.pack.is_empty() {
                    match source_id.as_str() {
                        "kenney" => landing
                            .label
                            .split('/')
                            .nth(1)
                            .unwrap_or("kenney")
                            .to_string(),
                        other => other.to_string(),
                    }
                } else {
                    landing.pack.clone()
                };
                let group_id = format!("import:{source_id}:{pack}");
                let group_label = match source_id.as_str() {
                    "freedoom" => "Freedoom · BSD-3-Clause".to_string(),
                    "doom" => "Doom shareware · id-Software-shareware".to_string(),
                    "librequake" => "LibreQuake · Modified BSD".to_string(),
                    "quake" => "Quake shareware · id-Software-shareware".to_string(),
                    "duke3d" => "Duke3D shareware · 3D-Realms-shareware".to_string(),
                    "quake2" => "Quake II shareware · id-Software-shareware".to_string(),
                    "quake3" => "Quake III demo · id-Software-demo".to_string(),
                    "darkmod" => "The Dark Mod · CC-BY-NC-SA-3.0".to_string(),
                    "kaykit" => "KayKit · CC0-1.0".to_string(),
                    _ => format!("Kenney {pack} · CC-BY-4.0"),
                };
                let premade = landing
                    .thumbnail
                    .as_ref()
                    .and_then(|p| std::fs::read(p).ok())
                    .filter(|b| {
                        b.starts_with(b"\x89PNG") && !crate::import::is_blank_preview_png(b)
                    });
                match library.import_unique_with_thumbnail(
                    landing.domain,
                    landing.content_type,
                    &landing.prompt,
                    &landing.label,
                    &bytes,
                    premade.as_deref(),
                    Some((group_id.as_str(), group_label.as_str())),
                ) {
                    Ok((file, created)) => {
                        if created {
                            queued += 1;
                        } else {
                            reused += 1;
                        }
                        // AO sidecars live beside the staged GLB after bake
                        // (or seed-from-source). Fail-closed: absent means none.
                        if landing.content_type.contains("gltf") {
                            if let Err(error) = library.install_ao_sidecars(&file, &landing.path) {
                                log!(
                                    "import: AO sidecar copy for {} failed: {error}",
                                    landing.label
                                );
                            }
                        }
                        if landing.content_type.contains("billboard") {
                            if let Err(error) =
                                library.install_billboard_frames(&file, &landing.path)
                            {
                                log!(
                                    "import: billboard frames for {} failed: {error}",
                                    landing.label
                                );
                            }
                        }
                        if landing.content_type.starts_with("image/") {
                            let preview = premade.clone().unwrap_or(bytes.clone());
                            preview_images.push((file.clone(), preview));
                        }
                        // Reimport rebuilds GPU icons unless a convert-time
                        // anim sheet already is the thumbnail.
                        if landing.content_type.contains("gltf") && premade.is_none() {
                            if let Err(error) = library.discard_model_thumbnail(&file) {
                                log!("import: could not drop old icon for {file}: {error}");
                            }
                            let (aomesh, ao_png) = library.ao_sidecar_bytes(&file);
                            thumbs.push((file.clone(), bytes, aomesh, ao_png));
                            icon_tracks.push((file, None));
                        } else if premade.is_some() {
                            icon_tracks.push((file, premade));
                        }
                    }
                    Err(error) => log!("import: library persist {}: {error}", landing.label),
                }
            }
        }
        for (file, png) in preview_images {
            self.import_page.push_preview_thumb(file, png);
        }
        for (file, existing) in icon_tracks {
            self.import_page.track_import_icon(file, existing);
        }
        for (file, bytes, aomesh, ao_png) in thumbs {
            self.queue_glb_thumbnail_ao(cx, &file, &bytes, aomesh, ao_png);
        }
        log!(
            "import: library landed {queued} new / {reused} cached · icons {}/{}",
            self.import_page.icons_done,
            self.import_page.icons_total()
        );
        self.refresh_gallery(cx, false);
        if self.import_landings.is_empty()
            && matches!(
                self.import_page.kenney_phase,
                crate::import::ImportPhase::Published { .. }
                    | crate::import::ImportPhase::PackFinished { .. }
                    | crate::import::ImportPhase::AllDone { .. }
            )
        {
            self.store.submit_search();
        }
        if self.surface == Surface::Library {
            self.refresh_library_ui(cx, false);
        }
        if self.surface == Surface::Import {
            self.refresh_import_ui(cx);
        }
    }

    /// `flip_y`: generated worlds (FlashWorld ply) are y-up/-z-forward and load
    /// as-is; scan-class plys (the biker sample) are y-down and need the flip.
    fn set_splat_file(&mut self, cx: &mut Cx, abs_path: &str, flip_y: bool) {
        let abs_path = abs_path.to_string();
        let mut splat = self.ui.widget(cx, ids!(splat));
        // Bare prelude names are not in the eval fragment's scope (only the
        // widget source's locals + `mod`), so address res through `mod`.
        // (`vec3(..)` is not in scope either — the scale is set through the
        // typed borrow below instead.)
        script_apply_eval!(cx, splat, {
            src: mod.res.file_resource(#(abs_path))
        });
        if let Some(mut view) = splat.borrow_mut::<ViewSplat>() {
            let sy = if flip_y { -1.0 } else { 1.0 };
            view.set_scale(vec3f(1.0, sy, 1.0));
        }
        splat.redraw(cx);
    }

    fn show_page(&mut self, cx: &mut Cx, page: LiveId) {
        // Leaving the video page stops the WHOLE player — decode thread,
        // per-frame pump chain and soundtrack — not just the audio. A hidden
        // video must never keep decoding; re-selecting the artifact starts a
        // fresh player.
        if page != id!(video_page).into() {
            self.stop_video_playback();
        }
        // The app owns one output callback, and hidden media stays silent:
        // leaving the audio page pauses the WAV (decoded clip retained for
        // an explicit resume), whatever page comes next.
        if page != id!(audio_page).into() {
            audio::pause();
        }
        self.audio_page_active = page == id!(audio_page).into();
        // Arriving on the audio page mid-playback (fresh one-shot artifact,
        // or a return visit) resumes the smooth playhead.
        self.arm_audio_pump(cx);
        self.ui
            .page_flip(cx, ids!(pages))
            .set_active_page(cx, page);
        self.ui.redraw(cx);
    }

    /// Tear down video playback: dropping the player signals its DETACHED
    /// decode thread to exit (never a join on the UI thread — see
    /// video_player.rs), the orphaned pump NextFrame then no-ops without
    /// re-arming, and the soundtrack queue is cleared with ownership
    /// revoked. The last uploaded frame remains on the texture unless
    /// [`Self::clear_video_frame`] is also called.
    fn stop_video_playback(&mut self) {
        self.video = None;
        crate::video_player::stop_audio();
    }

    /// Blank the actual video WIDGET texture (not only the app-side handle),
    /// so no previous clip's frame can sit behind a new open, a loading
    /// state, or an error state.
    fn clear_video_frame(&mut self, cx: &mut Cx) {
        self.video_texture = None;
        self.ui.image(cx, ids!(video_img)).set_texture(cx, None);
        self.ui.label(cx, ids!(video_info)).set_text(cx, "");
    }

    /// The text viewer shows exactly one thing at a time (REPLACE, never
    /// append).
    fn set_viewer_text(&mut self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(text_out)).set_text(cx, text);
    }

    /// Explicit loading state for an async selection: the blue ring already
    /// moved; here every stale viewer content is CLEARED (audio silenced +
    /// strip blanked, video stopped + frame blanked, image blanked) and a
    /// loading caption shows. The real caption commits together with the
    /// content when the still-current read lands.
    fn enter_viewer_loading(&mut self, cx: &mut Cx, file: &str, label: &str) {
        self.viewer = ViewerContent::Loading(file.to_string());
        audio::clear();
        self.audio_clip = None;
        self.stop_video_playback();
        self.clear_video_frame(cx);
        self.ui.image(cx, ids!(wave_img)).set_texture(cx, None);
        self.ui.image(cx, ids!(image_view)).set_texture(cx, None);
        self.update_audio_playhead(cx);
        self.set_viewer_text(cx, &format!("Loading {label}…"));
        self.set_caption(cx, "loading", label);
        self.show_page(cx, id!(text_page));
    }

    /// Honest failure state: cleared viewers (loading entry did that), the
    /// error itself on the text page, and an ERROR caption — never prior
    /// content.
    fn show_viewer_error(&mut self, cx: &mut Cx, file: &str, message: &str) {
        self.viewer = ViewerContent::Failed(file.to_string());
        self.set_viewer_text(cx, message);
        self.set_caption(cx, "error", &truncate(message, 90));
        self.show_page(cx, id!(text_page));
    }

    /// The shown/loading item no longer exists (single or group delete):
    /// empty the viewer instead of pointing it at a ghost.
    fn reset_viewer_if_gone(&mut self, cx: &mut Cx) {
        let gone = self.viewer.file().is_some_and(|file| {
            self.library
                .as_ref()
                .is_none_or(|library| library.get(file).is_none())
        });
        if gone {
            self.viewer = ViewerContent::Empty;
            self.set_viewer_text(cx, "Deleted.");
            self.show_page(cx, id!(text_page));
            self.set_caption(
                cx,
                "",
                "Nothing selected — run a chain, or pick something from History below.",
            );
        }
    }

    /// Push the device-clocked playback position into the waveform overlay
    /// (played-region tint + 2px line). `active` gates the whole overlay
    /// off while no decodable clip is loaded.
    fn update_audio_playhead(&mut self, cx: &mut Cx) {
        let (fraction, active) = if audio::is_ready() {
            (audio::playhead_fraction() as f32, 1.0f32)
        } else {
            (0.0, 0.0)
        };
        let playhead = self.ui.view(cx, ids!(wave_playhead));
        playhead.set_uniform(cx, live_id!(progress), &[fraction]);
        playhead.set_uniform(cx, live_id!(active), &[active]);
        playhead.redraw(cx);
    }

    /// Start the per-frame playhead pump when it is worth running; its
    /// handler re-arms under the same gate, so calling this is always safe.
    fn arm_audio_pump(&mut self, cx: &mut Cx) {
        if audio::is_playing() && self.surface == Surface::Create && self.audio_page_active {
            self.audio_pump = cx.new_next_frame();
        }
    }

    fn set_audio_unavailable(&mut self, cx: &mut Cx, reason: &str) {
        self.ui
            .label(cx, ids!(audio_info))
            .set_text(cx, &format!("Audio unavailable — {reason}"));
        self.ui.button(cx, ids!(play_btn)).set_enabled(cx, false);
        self.ui.button(cx, ids!(stop_btn)).set_enabled(cx, false);
        self.ui.button(cx, ids!(play_btn)).set_text(cx, "Play unavailable");
        self.update_audio_playhead(cx);
    }

    fn sync_audio_ui(&mut self, cx: &mut Cx) {
        let Some(pcm) = self.audio_clip.as_ref() else {
            self.set_audio_unavailable(cx, "no decodable WAV selected");
            return;
        };
        if !audio::is_ready() {
            self.set_audio_unavailable(cx, "decoded clip is no longer available");
            return;
        }
        let playing = audio::is_playing();
        let ended = audio::at_end();
        let state = if playing {
            "playing"
        } else if ended {
            "ended · Play restarts"
        } else {
            "paused"
        };
        self.ui.label(cx, ids!(audio_info)).set_text(
            cx,
            &format!(
                "{} / {} · {} Hz · {} ch · {state}",
                audio::format_time(audio::playhead_secs()),
                audio::format_time(audio::duration_secs()),
                pcm.sample_rate,
                pcm.channels,
            ),
        );
        self.ui.button(cx, ids!(play_btn)).set_enabled(cx, true);
        self.ui.button(cx, ids!(stop_btn)).set_enabled(cx, true);
        self.ui.button(cx, ids!(play_btn)).set_text(
            cx,
            if playing {
                "Pause"
            } else if ended {
                "Restart"
            } else {
                "Play"
            },
        );
        // Every transport interaction and the 10Hz readout tick also lands
        // the drawn playhead on the exact device-clocked position.
        self.update_audio_playhead(cx);
    }

    fn scrub_audio(&mut self, cx: &mut Cx, event: &Event) {
        if !audio::is_ready() {
            return;
        }
        let area = self.ui.view(cx, ids!(wave_scrub)).area();
        let rect = area.rect(cx);
        if rect.size.x <= 0.0 {
            return;
        }
        let seek = |x: f64| audio::seek_fraction((x - rect.pos.x) / rect.size.x);
        match event.hits(cx, area) {
            Hit::FingerHoverIn(_) | Hit::FingerHoverOver(_) => cx.set_cursor(MouseCursor::Hand),
            Hit::FingerDown(fe) if fe.device.is_primary_hit() => {
                seek(fe.abs.x);
                cx.set_cursor(MouseCursor::Grabbing);
                self.sync_audio_ui(cx);
            }
            Hit::FingerMove(fe) => {
                seek(fe.abs.x);
                self.sync_audio_ui(cx);
            }
            _ => (),
        }
    }

    fn set_mesh_shadows(&mut self, cx: &mut Cx, on: bool) {
        if let Some(mut mesh) = self
            .ui
            .widget(cx, ids!(mesh_view))
            .borrow_mut::<MeshView>()
        {
            mesh.set_shadows_enabled(cx, on);
        }
        self.ui
            .check_box(cx, ids!(shadows_toggle))
            .set_active(cx, on, Animate::No);
    }

    fn set_mesh_dark(&mut self, cx: &mut Cx, on: bool) {
        if let Some(mut mesh) = self
            .ui
            .widget(cx, ids!(mesh_view))
            .borrow_mut::<MeshView>()
        {
            mesh.set_dark_enabled(cx, on);
        }
        self.ui
            .check_box(cx, ids!(dark_toggle))
            .set_active(cx, on, Animate::No);
    }

    // -- samples (viewer smoke tests without live mesh/world backends) ---------

    fn load_sample_mesh(&mut self, cx: &mut Cx) {
        // AI_CONTENT_SAMPLE_MESH overrides the bundled sample with any GLB
        // path (viewer verification of generated meshes without a live run).
        let (glb, png) = match std::env::var("AI_CONTENT_SAMPLE_MESH") {
            Ok(path) => (std::fs::read(&path), None),
            Err(_) => (
                std::fs::read(repo_path("apps/asset-ui/resources/test/character_retex.glb")),
                std::fs::read(repo_path(
                    "apps/asset-ui/resources/test/character_retex_basecolor.png",
                ))
                .ok(),
            ),
        };
        match glb {
            Ok(glb) => {
                if let Some(mut mesh) = self
                    .ui
                    .widget(cx, ids!(mesh_view))
                    .borrow_mut::<MeshView>()
                {
                    mesh.set_model_bytes(cx, glb, png);
                }
                self.selected_file = None;
                // Samples are transient viewer content, not library-bound.
                self.viewer = ViewerContent::Empty;
                self.set_caption(cx, "mesh", "sample — bundled rig character");
                self.refresh_gallery(cx, true);
                self.show_page(cx, id!(mesh_page));
            }
            Err(e) => log!("sample mesh missing: {e}"),
        }
    }

    fn load_sample_splat(&mut self, cx: &mut Cx) {
        let path = repo_path("local/biker.ply");
        if std::path::Path::new(&path).is_file() {
            self.set_splat_file(cx, &path, true);
            self.selected_file = None;
            self.viewer = ViewerContent::Empty;
            self.set_caption(cx, "splat", "sample — biker scan");
            self.refresh_gallery(cx, true);
            self.show_page(cx, id!(splat_page));
        } else {
            log!("sample splat missing: {path}");
        }
    }

    /// A direct AI_CONTENT_SAMPLE path is an explicit user-selected artifact,
    /// not a disposable bundled demo. Copy it into the managed library once,
    /// select that stable resource, and leave the original file untouched.
    fn import_playtest_artifact(&mut self, cx: &mut Cx, path: &str, bytes: Vec<u8>) {
        let name = std::path::Path::new(path)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or("playable character");
        let label = format!("mesh {}", truncate(name, 18));
        let prompt = format!("Imported playable resource: {name}");
        // An external beauty render is preferable to a UV atlas extracted
        // from the GLB. The library still has a generalized embedded-image
        // fallback when this optional override is absent.
        let thumbnail = std::env::var("AI_CONTENT_SAMPLE_THUMBNAIL")
            .ok()
            .and_then(|path| std::fs::read(path).ok());
        // Every explicit standalone import is its own History group.
        let import_group = crate::library::new_group_id("import");
        let import_label = format!("Import — {}", truncate(name, 24));
        let managed = self.library.as_mut().and_then(|library| {
            match library.import_unique_with_thumbnail(
                "motion",
                "model/gltf-binary",
                &prompt,
                &label,
                &bytes,
                thumbnail.as_deref(),
                Some((&import_group, &import_label)),
            ) {
                Ok((file, added)) => {
                    log!(
                        "library: {} playtest resource as {file}",
                        if added { "imported" } else { "reused" }
                    );
                    Some((file, added))
                }
                Err(error) => {
                    log!("library: playtest import failed: {error}");
                    None
                }
            }
        });
        self.selected_file = managed.as_ref().map(|(file, _)| file.clone());
        self.set_caption(cx, "mesh", name);
        if self.display_artifact(cx, "motion", "model/gltf-binary", &bytes, 0, None, false) {
            self.viewer = match &self.selected_file {
                Some(file) => ViewerContent::Showing(file.clone()),
                None => ViewerContent::Empty,
            };
        }
        if let Some((file, added)) = managed {
            // A first import upgrades its embedded-atlas fallback to a real
            // model render once; relaunching with a valid sidecar in place
            // (the AI_CONTENT_SAMPLE dedupe path) queues nothing.
            if added
                || self
                    .library
                    .as_ref()
                    .is_some_and(|library| library.needs_model_thumbnail(&file))
            {
                self.queue_glb_thumbnail(cx, &file, &bytes);
            }
        }
        self.refresh_gallery(cx, true);
    }

    // -- per-frame video pump ----------------------------------------------------

    fn pump_video(&mut self, cx: &mut Cx) {
        let Some(player) = &mut self.video else { return };
        if let Some(frame) = player.take_due_frame() {
            let (w, h) = (player.width as usize, player.height as usize);
            match &self.video_texture {
                Some(tex) => tex.set_data_u32(cx, w, h, frame),
                None => {
                    self.video_texture = Some(Texture::new_with_format(
                        cx,
                        TextureFormat::VecBGRAu8_32 {
                            width: w,
                            height: h,
                            data: Some(frame),
                            updated: TextureUpdated::Full,
                        },
                    ));
                    self.ui
                        .image(cx, ids!(video_img))
                        .set_texture(cx, self.video_texture.clone());
                }
            }
            self.ui.image(cx, ids!(video_img)).redraw(cx);
        }
        if let Some(player) = &self.video {
            if player.at_end() {
                // Real EOS: decode thread exited (end of stream or error)
                // and every due frame is shown. The pump chain ends here
                // instead of re-arming at frame rate forever; the soundtrack
                // tail drains from the audio callback on its own.
                self.ui
                    .label(cx, ids!(video_info))
                    .set_text(cx, "ended — click the history card to replay");
                return;
            }
        }
        self.video_pump = cx.new_next_frame();
    }
}

/// Offscreen model renders the backfill pump keeps queued in the headless
/// renderer at once (one renders while the next is staged).
const MODEL_THUMBNAIL_MAX_PENDING: usize = 2;
/// Finished runs kept visible on the Runs surface (running ones always are).
const MAX_FINISHED_RUNS_SHOWN: usize = 3;

fn kind_label(domain: &str, content_type: &str) -> &'static str {
    crate::asset_store_state::library_type(domain, content_type)
}

/// Which completions take over the Create viewer. Paint's channel maps and
/// provenance JSON are kept in History; auto-showing them is why a finished
/// Hunyuan job landed on a text dump after two atlas images flashed past.
fn auto_show_artifact(domain: &str, content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    if ct.starts_with("application/json") || ct.starts_with("text/") {
        return false;
    }
    if domain == "paint" && ct.starts_with("image/") {
        return false;
    }
    true
}

/// Named chip slots under the Library tag dropdown. Keep in sync with
/// `ft0`…`ft7` in the Library filter row.
fn set_lib_tag_chip(ui: &WidgetRef, cx: &mut Cx, index: usize, text: Option<&str>) {
    let on = text.is_some();
    match index {
        0 => {
            ui.widget(cx, ids!(ft0)).set_visible(cx, on);
            if let Some(text) = text {
                ui.label(cx, ids!(ft0.chip_name)).set_text(cx, text);
            }
        }
        1 => {
            ui.widget(cx, ids!(ft1)).set_visible(cx, on);
            if let Some(text) = text {
                ui.label(cx, ids!(ft1.chip_name)).set_text(cx, text);
            }
        }
        2 => {
            ui.widget(cx, ids!(ft2)).set_visible(cx, on);
            if let Some(text) = text {
                ui.label(cx, ids!(ft2.chip_name)).set_text(cx, text);
            }
        }
        3 => {
            ui.widget(cx, ids!(ft3)).set_visible(cx, on);
            if let Some(text) = text {
                ui.label(cx, ids!(ft3.chip_name)).set_text(cx, text);
            }
        }
        4 => {
            ui.widget(cx, ids!(ft4)).set_visible(cx, on);
            if let Some(text) = text {
                ui.label(cx, ids!(ft4.chip_name)).set_text(cx, text);
            }
        }
        5 => {
            ui.widget(cx, ids!(ft5)).set_visible(cx, on);
            if let Some(text) = text {
                ui.label(cx, ids!(ft5.chip_name)).set_text(cx, text);
            }
        }
        6 => {
            ui.widget(cx, ids!(ft6)).set_visible(cx, on);
            if let Some(text) = text {
                ui.label(cx, ids!(ft6.chip_name)).set_text(cx, text);
            }
        }
        7 => {
            ui.widget(cx, ids!(ft7)).set_visible(cx, on);
            if let Some(text) = text {
                ui.label(cx, ids!(ft7.chip_name)).set_text(cx, text);
            }
        }
        _ => {}
    }
}

fn lib_tag_chip_removed(ui: &WidgetRef, cx: &mut Cx, actions: &Actions) -> Option<usize> {
    if ui.button(cx, ids!(ft0.chip_x)).clicked(actions) {
        Some(0)
    } else if ui.button(cx, ids!(ft1.chip_x)).clicked(actions) {
        Some(1)
    } else if ui.button(cx, ids!(ft2.chip_x)).clicked(actions) {
        Some(2)
    } else if ui.button(cx, ids!(ft3.chip_x)).clicked(actions) {
        Some(3)
    } else if ui.button(cx, ids!(ft4.chip_x)).clicked(actions) {
        Some(4)
    } else if ui.button(cx, ids!(ft5.chip_x)).clicked(actions) {
        Some(5)
    } else if ui.button(cx, ids!(ft6.chip_x)).clicked(actions) {
        Some(6)
    } else if ui.button(cx, ids!(ft7.chip_x)).clicked(actions) {
        Some(7)
    } else {
        None
    }
}

/// The word a seeded transform label uses for the pinned input's class.
fn seed_kind_word(content_type: &str) -> &'static str {
    let ct = content_type.to_ascii_lowercase();
    if ct.starts_with("image/") {
        "image"
    } else if ct.starts_with("model/") {
        "mesh"
    } else {
        "input"
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.setup(cx);
    }

    fn handle_http_response(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        response: &HttpResponse,
    ) {
        if self
            .classic_import_page
            .handle_http_response(cx, request_id, response)
        {
            self.on_classic_download_event(cx);
        }
    }

    fn handle_http_request_error(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        err: &HttpError,
    ) {
        if self.classic_import_page.handle_http_error(cx, request_id, err) {
            self.on_classic_download_event(cx);
        }
    }

    fn handle_http_progress(
        &mut self,
        cx: &mut Cx,
        request_id: LiveId,
        progress: &HttpProgress,
    ) {
        if self
            .classic_import_page
            .handle_http_progress(request_id, progress)
            && self.surface == Surface::Import
        {
            self.refresh_import_ui(cx);
        }
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        cx.use_audio_outputs(&devices.default_output());
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Surface nav.
        for (tab, surface) in [
            (ids!(nav_create), Surface::Create),
            (ids!(nav_chat), Surface::Chat),
            (ids!(nav_library), Surface::Library),
            (ids!(nav_import), Surface::Import),
            (ids!(nav_runs), Surface::Runs),
            (ids!(nav_admin), Surface::Admin),
        ] {
            if self.ui.button(cx, tab).clicked(actions) {
                self.show_surface(cx, surface);
            }
        }
        if self.ui.button(cx, ids!(open_library_btn)).clicked(actions) {
            self.show_surface(cx, Surface::Library);
        }
        if self
            .ui
            .drop_down2(cx, ids!(kenney_pack_drop))
            .changed(actions)
            .is_some()
        {
            let index = self.ui.drop_down2(cx, ids!(kenney_pack_drop)).selected_item();
            self.import_page.set_pack_index(index);
            if self.surface == Surface::Import {
                self.refresh_import_ui(cx);
            }
        }
        if self.ui.button(cx, ids!(kenney_import_btn)).clicked(actions) {
            let (pack, _) = self.import_page.selected_pack_id();
            log!("import: queue Kenney {pack}");
            self.enqueue_import(
                cx,
                ImportJob::Kenney {
                    pack,
                    pack_index: self.import_page.kenney_pack_index,
                    path: String::new(),
                },
            );
        }
        if self.ui.button(cx, ids!(kenney_import_all_btn)).clicked(actions) {
            log!("import: queue Kenney all");
            self.enqueue_import(cx, ImportJob::KenneyAll);
        }
        if self.ui.button(cx, ids!(queue_clear_btn)).clicked(actions) {
            self.import_queue.clear_pending();
            self.stop_active_import(cx);
            log!("import: queue cleared");
            self.refresh_import_ui(cx);
        }
        if self.ui.button(cx, ids!(freedoom_import_btn)).clicked(actions) {
            log!("import: queue Freedoom");
            self.enqueue_import(
                cx,
                ImportJob::Freedoom {
                    path: String::new(),
                },
            );
        }
        if self.ui.button(cx, ids!(doom_import_btn)).clicked(actions) {
            log!("import: queue Doom shareware");
            self.enqueue_import(
                cx,
                ImportJob::Doom {
                    path: String::new(),
                },
            );
        }
        if self.ui.button(cx, ids!(librequake_import_btn)).clicked(actions) {
            log!("import: queue LibreQuake");
            self.enqueue_import(
                cx,
                ImportJob::LibreQuake {
                    path: String::new(),
                },
            );
        }
        if self.ui.button(cx, ids!(quake_import_btn)).clicked(actions) {
            log!("import: queue Quake shareware");
            self.enqueue_import(
                cx,
                ImportJob::Quake {
                    path: String::new(),
                },
            );
        }
        if self.ui.button(cx, ids!(duke3d_import_btn)).clicked(actions) {
            log!("import: queue Duke3D shareware");
            self.enqueue_import(
                cx,
                ImportJob::Duke3d {
                    path: String::new(),
                },
            );
        }
        if self.ui.button(cx, ids!(quake2_import_btn)).clicked(actions) {
            log!("import: queue Quake II shareware");
            self.enqueue_import(
                cx,
                ImportJob::Quake2 {
                    path: String::new(),
                },
            );
        }
        if self.ui.button(cx, ids!(quake3_import_btn)).clicked(actions) {
            log!("import: queue Quake III demo");
            self.enqueue_import(
                cx,
                ImportJob::Quake3 {
                    path: String::new(),
                },
            );
        }
        if self.ui.button(cx, ids!(darkmod_import_btn)).clicked(actions) {
            log!("import: queue The Dark Mod");
            self.enqueue_import(
                cx,
                ImportJob::DarkMod {
                    path: String::new(),
                },
            );
        }
        if self.ui.button(cx, ids!(kaykit_import_btn)).clicked(actions) {
            log!("import: queue KayKit");
            self.enqueue_import(cx, ImportJob::KayKit);
        }
        let queue_widget = self.ui.widget(cx, ids!(import_queue_list));
        let queue_portal = queue_widget.portal_list(cx, ids!(list));
        let mut queue_action = None;
        for (row_id, item) in queue_portal.items_with_actions(actions) {
            if item.button(cx, ids!(stage_cancel)).clicked(actions)
                || item.button(cx, ids!(queued_cancel)).clicked(actions)
            {
                queue_action = queue_widget
                    .borrow::<StoreListPanel>()
                    .and_then(|panel| panel.row_at(row_id));
                break;
            }
        }
        match queue_action {
            Some(StoreRow::Stage {
                cancel: Some(RowAction::StopImport),
                ..
            }) => {
                log!("import: stop current");
                self.stop_active_import(cx);
            }
            Some(StoreRow::Queued {
                cancel: RowAction::RemoveQueuedImport(id),
                ..
            }) => {
                self.import_queue.remove(id);
                log!("import: removed queued item {id}");
                self.refresh_import_ui(cx);
            }
            _ => {}
        }
        if self.ui.button(cx, ids!(chat_send_btn)).clicked(actions) {
            self.send_chat(cx);
        }
        if self.ui.button(cx, ids!(chat_cancel_btn)).clicked(actions) {
            self.chat.cancel();
            self.refresh_chat_ui(cx);
        }
        if self.ui.text_input(cx, ids!(chat_input)).returned(actions).is_some() {
            self.send_chat(cx);
        }
        if self.ui.button(cx, ids!(lib_local_tab)).clicked(actions) {
            self.set_lib_source(cx, LibSource::Local);
        }
        if self.ui.button(cx, ids!(lib_server_tab)).clicked(actions) {
            self.set_lib_source(cx, LibSource::Server);
        }
        // Library filters re-run on every keystroke / dropdown pick; the
        // thumbnail cache survives (only the visible set changes).
        let mut filters_changed = self
            .ui
            .text_input(cx, ids!(lib_search))
            .changed(actions)
            .is_some();
        if let Some(index) = self.ui.drop_down2(cx, ids!(lib_tag_drop)).changed(actions) {
            if let Some(stat) = self.lib_tag_options.get(index).cloned() {
                if let Some(pos) = self
                    .lib_filters
                    .tags
                    .iter()
                    .position(|have| have.eq_ignore_ascii_case(&stat.name))
                {
                    self.lib_filters.tags.remove(pos);
                } else if self.lib_filters.tags.len() < 8 {
                    self.lib_filters.tags.push(stat.name);
                }
                filters_changed = true;
            }
        }
        if let Some(index) = lib_tag_chip_removed(&self.ui, cx, actions) {
            if index < self.lib_filters.tags.len() {
                self.lib_filters.tags.remove(index);
                filters_changed = true;
            }
        }
        if filters_changed {
            self.refresh_library_ui(cx, false);
        }
        if self.ui.button(cx, ids!(lib_clear_btn)).clicked(actions) {
            self.ui.text_input(cx, ids!(lib_search)).set_text(cx, "");
            self.lib_filters.tags.clear();
            self.ui
                .drop_down2(cx, ids!(lib_tag_drop))
                .set_selected_item(cx, 0);
            self.refresh_library_ui(cx, false);
        }
        if self.ui.button(cx, ids!(lib_enhance_btn)).clicked(actions) {
            self.run_enhance_metadata(cx);
        }
        // Grid card clicks select + load into the viewer. Media file dragging
        // is confined to the explicit handle so card-surface vertical scroll
        // and ordinary selection keep their existing behavior.
        let grid_widget = self.ui.widget(cx, ids!(lib_grid));
        let grid_list = grid_widget.portal_list(cx, ids!(list));
        let grid_cols = grid_widget
            .borrow::<LibraryGrid>()
            .map_or(1, |grid| grid.last_cols.max(1));
        let mut grid_pick = None;
        let mut grid_open = None;
        let mut grid_drag = None;
        let slots = [
            ids!(c1), ids!(c2), ids!(c3), ids!(c4),
            ids!(c5), ids!(c6), ids!(c7), ids!(c8),
        ];
        let drag_handles = [
            ids!(c1.file_drag), ids!(c2.file_drag), ids!(c3.file_drag),
            ids!(c4.file_drag), ids!(c5.file_drag), ids!(c6.file_drag),
            ids!(c7.file_drag), ids!(c8.file_drag),
        ];
        'grid_rows: for (row_id, item) in grid_list.items_with_actions(actions) {
            for (slot, path) in slots.iter().enumerate() {
                let index = row_id * grid_cols + slot;
                let handle = item.file_drag_handle(cx, drag_handles[slot]);
                let drag_down = handle.finger_down(actions).is_some();
                let drag_move = handle.finger_move(actions);
                let drag_up = handle.finger_up(actions).is_some();
                if drag_down || drag_move.is_some() || drag_up {
                    // The handle owns the whole gesture, including a short
                    // click. Otherwise its FingerDown can bubble to the card
                    // and accidentally select/open before a drag begins.
                    if let Some(event) = drag_move.filter(|event| {
                        should_start_file_drag(event.move_distance(), self.file_drag_active)
                    }) {
                        if let Some(payload) = grid_widget
                            .borrow::<LibraryGrid>()
                            .and_then(|grid| grid.file_drag_payload_path_at(index))
                        {
                            grid_drag = Some((event.window_id, payload));
                        }
                    }
                    break 'grid_rows;
                }
                if let Some(fe) = item.view(cx, *path).finger_down(actions) {
                    if fe.tap_count >= 2 {
                        grid_open = Some(index);
                    } else {
                        grid_pick = Some(index);
                    }
                    break 'grid_rows;
                }
            }
        }
        if let Some((window_id, payload)) = grid_drag {
            self.start_file_payload_drag(cx, window_id, payload);
        } else if let Some(index) = grid_open.or(grid_pick) {
            let file = grid_widget
                .borrow::<LibraryGrid>()
                .and_then(|grid| grid.file_at(index));
            if let Some(file) = file {
                if grid_open.is_some() {
                    self.open_gallery(cx, &file);
                    self.show_surface(cx, Surface::Create);
                } else {
                    self.select_gallery(cx, &file);
                }
            }
        }
        // Detail rail actions (local selection only).
        if self.ui.button(cx, ids!(detail_open_btn)).clicked(actions) {
            if let Some(file) = self.selected_file.clone() {
                self.open_gallery(cx, &file);
            }
            self.show_surface(cx, Surface::Create);
        }
        if self.ui.button(cx, ids!(detail_reuse_btn)).clicked(actions) {
            let prompt = self.selected_file.clone().and_then(|file| {
                self.library
                    .as_ref()
                    .and_then(|library| library.get(&file))
                    .map(|item| item.prompt.clone())
            });
            if let Some(prompt) = prompt.filter(|prompt| !prompt.trim().is_empty()) {
                self.ui
                    .text_input(cx, ids!(prompt_input))
                    .set_text(cx, &prompt);
            }
        }
        if self.ui.button(cx, ids!(detail_delete_btn)).clicked(actions) {
            if let Some(file) = self.selected_file.clone() {
                self.delete_gallery(cx, &file);
            }
        }
        // Runs list: cancel the active stage / drop a queued run.
        let runs_widget = self.ui.widget(cx, ids!(runs_list));
        let runs_portal = runs_widget.portal_list(cx, ids!(list));
        let mut runs_action = None;
        for (row_id, item) in runs_portal.items_with_actions(actions) {
            if item.button(cx, ids!(stage_cancel)).clicked(actions)
                || item.button(cx, ids!(queued_cancel)).clicked(actions)
            {
                runs_action = runs_widget
                    .borrow::<StoreListPanel>()
                    .and_then(|panel| panel.row_at(row_id));
                break;
            }
        }
        match runs_action {
            Some(StoreRow::Stage {
                cancel: Some(RowAction::CancelRun(run_id)),
                ..
            }) => self.cancel_run(cx, run_id),
            Some(StoreRow::Queued {
                cancel: RowAction::CancelQueued(index),
                ..
            }) => self.cancel_row(cx, index),
            _ => {}
        }
        // Server catalog rows (only ever populated by a real transport).
        let server_widget = self.ui.widget(cx, ids!(lib_server_list));
        let server_portal = server_widget.portal_list(cx, ids!(list));
        let mut picked_asset = None;
        for (row_id, item) in server_portal.items_with_actions(actions) {
            if item.view(cx, ids!(asset_card)).finger_down(actions).is_some() {
                if let Some(StoreRow::Asset {
                    action: RowAction::SelectAsset(asset_id),
                    ..
                }) = server_widget
                    .borrow::<StoreListPanel>()
                    .and_then(|panel| panel.row_at(row_id))
                {
                    picked_asset = Some(asset_id);
                    break;
                }
            }
        }
        if let Some(asset_id) = picked_asset {
            match asset_id.parse::<makepad_asset_data::AssetId>() {
                Ok(asset_id) => {
                    self.store.select(asset_id);
                    self.refresh_library_ui(cx, false);
                }
                Err(error) => log!("asset store: invalid catalog id {asset_id}: {error}"),
            }
        }

        if self.ui.button(cx, ids!(generate_btn)).clicked(actions) {
            self.start_generate(cx);
        }
        // Enter in the prompt box runs the selected preset.
        if self
            .ui
            .text_input(cx, ids!(prompt_input))
            .returned(actions)
            .is_some()
        {
            self.start_generate(cx);
        }
        if self.ui.button(cx, ids!(cancel_btn)).clicked(actions) {
            self.cancel_active(cx);
        }
        if self.ui.button(cx, ids!(alpha_btn)).clicked(actions) {
            self.alpha_view = !self.alpha_view;
            let v = if self.alpha_view { 1.0 } else { 0.0 };
            let mut image = self.ui.image(cx, ids!(image_view));
            script_apply_eval!(cx, image, {
                draw_bg +: { alpha_view: #(v) }
            });
            self.ui.button(cx, ids!(alpha_btn)).set_text(
                cx,
                if self.alpha_view { "RGB view" } else { "Alpha matte" },
            );
            self.ui.redraw(cx);
        }
        if self.ui.button(cx, ids!(pull_btn)).clicked(actions) {
            self.pull_model(cx);
        }
        // Unpin the selected input (the asset itself is untouched).
        if self.ui.button(cx, ids!(input_clear)).clicked(actions) {
            self.input_tray.clear();
            self.sync_input_tray(cx);
        }
        if self.ui.button(cx, ids!(retry_btn)).clicked(actions) {
            // Retry re-dispatches the SAME spec — including its group id, so
            // retried artifacts join the original run's History group.
            if let Some(run) = self.last_run.clone() {
                self.run_queue.push(run);
                self.try_dispatch_pending(cx);
            }
        }
        if self
            .ui
            .drop_down2(cx, ids!(preset_drop))
            .changed(actions)
            .is_some()
        {
            self.refresh_model_ui(cx, true);
            self.refresh_voice_ui(cx);
            self.sync_preset_name_box(cx);
        }
        if self.ui.button(cx, ids!(save_preset_btn)).clicked(actions) {
            self.save_current_preset(cx);
        }
        let fast_slots = [
            (ids!(fp0_go), ids!(fp0_del), 0usize),
            (ids!(fp1_go), ids!(fp1_del), 1),
            (ids!(fp2_go), ids!(fp2_del), 2),
            (ids!(fp3_go), ids!(fp3_del), 3),
            (ids!(fp4_go), ids!(fp4_del), 4),
            (ids!(fp5_go), ids!(fp5_del), 5),
            (ids!(fp6_go), ids!(fp6_del), 6),
            (ids!(fp7_go), ids!(fp7_del), 7),
        ];
        for (go, del, index) in fast_slots {
            if self.ui.button(cx, go).clicked(actions) {
                self.apply_saved_preset(cx, index);
            }
            if self.ui.button(cx, del).clicked(actions) && index < self.saved_presets.len() {
                self.saved_presets.remove(index);
                self.persist_saved_presets();
                self.refresh_saved_presets_ui(cx);
            }
        }
        let stage_drops = [
            ids!(md_text),
            ids!(md_image),
            ids!(md_audio),
            ids!(md_speech),
            ids!(md_music),
            ids!(md_video),
            ids!(md_mesh),
            ids!(md_matte),
            ids!(md_depth),
            ids!(md_segment),
            ids!(md_paint),
            ids!(md_world),
            ids!(md_rig),
            ids!(md_motion),
            ids!(size_drop),
            ids!(steps_drop),
            ids!(texture_size_drop),
            ids!(mesh_faces_drop),
            ids!(vid_size_drop),
            ids!(vid_len_drop),
            ids!(music_len_drop),
        ];
        if stage_drops
            .iter()
            .any(|id| self.ui.drop_down2(cx, *id).changed(actions).is_some())
        {
            self.refresh_voice_ui(cx);
            self.sync_preset_name_box(cx);
        }
        if self
            .ui
            .button(cx, ids!(continue_choice_btn))
            .clicked(actions)
        {
            self.continue_candidate_choice(cx, false);
        }
        if self
            .ui
            .button(cx, ids!(continue_early_btn))
            .clicked(actions)
        {
            self.continue_candidate_choice(cx, true);
        }
        if self
            .ui
            .button(cx, ids!(retry_candidates_btn))
            .clicked(actions)
        {
            self.retry_candidate_failures(cx);
        }
        let candidate_widget = self.ui.widget(cx, ids!(candidate_sheet));
        let candidate_list = candidate_widget.portal_list(cx, ids!(list));
        let mut candidate_pick = None;
        let candidate_cols = candidate_widget
            .borrow::<CandidateSheet>()
            .map_or(4, |sheet| sheet.last_cols.max(1));
        let candidate_slots = [ids!(c1), ids!(c2), ids!(c3), ids!(c4)];
        for (row, item) in candidate_list.items_with_actions(actions) {
            for (slot, path) in candidate_slots.iter().enumerate() {
                if slot < candidate_cols
                    && item.view(cx, *path).finger_down(actions).is_some()
                {
                    candidate_pick = candidate_widget
                        .borrow::<CandidateSheet>()
                        .and_then(|sheet| sheet.candidate_at_cell(row, slot));
                    break;
                }
            }
            if candidate_pick.is_some() {
                break;
            }
        }
        if let Some(candidate_id) = candidate_pick {
            let target = self.runs.iter().enumerate().rev().find_map(|(index, run)| {
                let set = run.pipeline.active_candidate_set()?;
                set.candidates
                    .iter()
                    .any(|candidate| candidate.id == candidate_id)
                    .then(|| (index, set.id.clone(), run.id))
            });
            if let Some((index, set_id, run_id)) = target {
                match self.runs[index]
                    .pipeline
                    .select_candidate(&set_id, &candidate_id)
                {
                    Ok(events) => self.on_run_events(cx, run_id, events),
                    Err(error) => self.set_caption(cx, "CHOOSE", &error),
                }
            }
        }
        if self.ui.button(cx, ids!(play_btn)).clicked(actions) {
            if audio::is_ready() {
                if audio::is_playing() {
                    audio::pause();
                } else {
                    // A user-resumed WAV preview wins over a stale video
                    // soundtrack in the shared device callback.
                    crate::video_player::stop_audio();
                    audio::play();
                    self.arm_audio_pump(cx);
                }
                self.sync_audio_ui(cx);
            }
        }
        if self.ui.button(cx, ids!(stop_btn)).clicked(actions) {
            if audio::is_ready() {
                audio::stop();
                self.sync_audio_ui(cx);
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(shadows_toggle)).changed(actions) {
            self.set_mesh_shadows(cx, on);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(dark_toggle)).changed(actions) {
            self.set_mesh_dark(cx, on);
        }
        if let Some(on) = self.ui.check_box(cx, ids!(studio_toggle)).changed(actions) {
            if let Some(mut mesh) = self
                .ui
                .widget(cx, ids!(mesh_view))
                .borrow_mut::<MeshView>()
            {
                mesh.set_studio_enabled(cx, on);
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(speculars_toggle)).changed(actions) {
            if let Some(mut mesh) = self
                .ui
                .widget(cx, ids!(mesh_view))
                .borrow_mut::<MeshView>()
            {
                mesh.set_pbr_speculars(cx, on);
            }
        }
        if let Some(index) = self.ui.drop_down2(cx, ids!(pbr_view_drop)).changed(actions) {
            let mode = crate::mesh_view::pbr_preview::PbrViewMode::ALL
                .get(index)
                .copied()
                .unwrap_or_default();
            if let Some(mut mesh) = self
                .ui
                .widget(cx, ids!(mesh_view))
                .borrow_mut::<MeshView>()
            {
                mesh.set_pbr_view_mode(cx, mode);
            }
        }
        if self.ui.button(cx, ids!(sample_splat_btn)).clicked(actions) {
            self.load_sample_splat(cx);
        }
        // History actions carry their virtualized item id through PortalList;
        // resolve it to the gallery's stable managed file id before mutating.
        let gallery_widget = self.ui.widget(cx, ids!(library_gallery));
        let gallery_list = gallery_widget.portal_list(cx, ids!(list));
        enum GalleryAct {
            Delete(usize),
            DragFile {
                window_id: WindowId,
                payload: PathBuf,
            },
            SuppressOpen,
            /// Single click: view only.
            View(usize),
            /// Double click: view AND pin as the next transform's input.
            Open(usize),
        }
        let mut gallery_action = None;
        for (index, item) in gallery_list.items_with_actions(actions) {
            // Deletes win when a press also bubbles through the card view.
            if item.button(cx, ids!(card.delete)).clicked(actions) {
                gallery_action = Some(GalleryAct::Delete(index));
                break;
            }
            let drag_handle = item.file_drag_handle(cx, ids!(card.file_drag));
            let drag_down = drag_handle.finger_down(actions).is_some();
            let drag_move = drag_handle.finger_move(actions);
            let drag_up = drag_handle.finger_up(actions).is_some();
            if drag_down || drag_move.is_some() || drag_up {
                gallery_action = drag_move
                    .filter(|event| {
                        should_start_file_drag(event.move_distance(), self.file_drag_active)
                    })
                    .and_then(|event| {
                        gallery_widget
                            .borrow::<LibraryGallery>()
                            .and_then(|gallery| gallery.file_drag_payload_path_at(index))
                            .map(|payload| GalleryAct::DragFile {
                                window_id: event.window_id,
                                payload,
                            })
                    })
                    .or(Some(GalleryAct::SuppressOpen));
                break;
            }
            // Single click views; a double click also pins the artifact as
            // the next transform's input (pinning on every click meant
            // clearing the input tray before each ordinary generate).
            if let Some(fe) = item.view(cx, ids!(card)).finger_down(actions) {
                gallery_action = Some(if fe.tap_count >= 2 {
                    GalleryAct::Open(index)
                } else {
                    GalleryAct::View(index)
                });
                break;
            }
            if item.button(cx, ids!(card.title)).clicked(actions) {
                gallery_action = Some(GalleryAct::View(index));
                break;
            }
        }
        match gallery_action {
            Some(GalleryAct::Delete(index)) => {
                // The tile itself knows the exact scope of its one ×.
                let target = gallery_widget
                    .borrow::<LibraryGallery>()
                    .and_then(|gallery| gallery.delete_at(index));
                match target {
                    Some(TileDelete::Group(group)) => self.delete_gallery_group(cx, &group),
                    Some(TileDelete::Single(file)) => self.delete_gallery(cx, &file),
                    None => {}
                }
            }
            Some(GalleryAct::DragFile { window_id, payload }) => {
                self.start_file_payload_drag(cx, window_id, payload);
            }
            Some(GalleryAct::SuppressOpen) => {}
            Some(GalleryAct::View(index)) => {
                let file = gallery_widget
                    .borrow::<LibraryGallery>()
                    .and_then(|gallery| gallery.file_at(index));
                if let Some(file) = file {
                    self.reopen_gallery(cx, &file);
                }
            }
            Some(GalleryAct::Open(index)) => {
                let file = gallery_widget
                    .borrow::<LibraryGallery>()
                    .and_then(|gallery| gallery.file_at(index));
                if let Some(file) = file {
                    self.open_gallery(cx, &file);
                }
            }
            None => {}
        }
        // Run tray chips: single click views the member, double click also
        // pins it as the next transform's input.
        let mut chip_hit = None;
        for (slot, chip) in Self::run_chip_ids().iter().enumerate() {
            if let Some(fe) = self.ui.view(cx, chip).finger_down(actions) {
                chip_hit = self.run_tray_files.get(slot).cloned().map(|f| (f, fe.tap_count >= 2));
                break;
            }
        }
        if let Some((file, pin)) = chip_hit {
            if pin {
                self.open_gallery(cx, &file);
            } else {
                self.reopen_gallery(cx, &file);
            }
        }
        // Run-queue rows: cancel / move up.
        for (k, (cancel, up)) in [
            (ids!(q1_cancel), ids!(q1_up)),
            (ids!(q2_cancel), ids!(q2_up)),
            (ids!(q3_cancel), ids!(q3_up)),
            (ids!(q4_cancel), ids!(q4_up)),
            (ids!(q5_cancel), ids!(q5_up)),
            (ids!(q6_cancel), ids!(q6_up)),
        ]
        .iter()
        .enumerate()
        {
            if self.ui.button(cx, *cancel).clicked(actions) {
                self.cancel_row(cx, k);
            }
            if self.ui.button(cx, *up).clicked(actions) {
                self.move_row_up(cx, k);
            }
        }
        let _ = QUEUE_ROWS;
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        // Draw shaders must register before the widgets that declare them.
        makepad_render::script_mod(vm);
        makepad_xr::script_mod(vm);
        crate::mesh_view::script_mod(vm);
        crate::billboard_view::script_mod(vm);
        crate::thumbnail_renderer::script_mod(vm);
        crate::chat::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if matches!(event, Event::DragEnd) {
            self.file_drag_active = false;
        }
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::F8 && !ke.is_repeat {
                let on = self
                    .ui
                    .widget(cx, ids!(mesh_view))
                    .borrow::<MeshView>()
                    .map(|mesh| mesh.shadows_enabled())
                    .unwrap_or(true);
                self.set_mesh_shadows(cx, !on);
            }
        }
        self.match_event(cx, event);
        if self.asset_store_timer.is_event(event).is_some() {
            let store_changed = self.store.poll();
            let kenney_poll = self.import_page.poll();
            let classic_poll = self.classic_import_page.poll();
            if self.surface == Surface::Import && self.import_page.icons_busy() {
                let current = self
                    .ui
                    .widget(cx, ids!(thumbnail_renderer))
                    .borrow::<ThumbnailRenderer>()
                    .and_then(|renderer| renderer.thumbnail_active_file().map(str::to_string));
                self.import_page.set_icon_current(current.as_deref());
                self.refresh_import_ui(cx);
            }
            self.drain_import_previews();
            if kenney_poll || classic_poll || !self.import_landings.is_empty() {
                if kenney_poll {
                    log!(
                        "import: {}",
                        self.import_page.kenney_status_line(self.store.connected())
                    );
                }
                if classic_poll {
                    log!(
                        "import classic: freedoom={} librequake={} duke3d={} quake2={} quake3={}",
                        self.classic_import_page
                            .freedoom
                            .status_line(self.store.connected()),
                        self.classic_import_page
                            .librequake
                            .status_line(self.store.connected()),
                        self.classic_import_page
                            .duke3d
                            .status_line(self.store.connected()),
                        self.classic_import_page
                            .quake2
                            .status_line(self.store.connected()),
                        self.classic_import_page
                            .quake3
                            .status_line(self.store.connected())
                    );
                }
                self.land_imported_pack(cx);
                if !self.import_busy()
                    && self.import_landings.is_empty()
                    && self.import_queue.active.is_some()
                {
                    self.kick_import_queue(cx);
                } else if self.surface == Surface::Import {
                    self.refresh_import_ui(cx);
                }
            } else if self.import_page.preview_dirty && self.surface == Surface::Import {
                self.refresh_import_ui(cx);
            }
            self.maybe_connect_chat();
            self.drain_chat_jobs(cx);
            if self.chat.take_dirty() {
                self.refresh_chat_ui(cx);
                self.ui.redraw(cx);
            }
            if store_changed {
                // Only redraw the surface that consumes the changed remote DTOs;
                // the connection chip is shared across every surface.
                self.ui
                    .label(cx, ids!(remote_connection))
                    .set_text(cx, &self.store.status_label());
                match self.surface {
                    Surface::Create => self.ui.redraw(cx),
                    Surface::Chat => self.refresh_chat_ui(cx),
                    Surface::Library => self.refresh_library_ui(cx, false),
                    Surface::Import => self.refresh_import_ui(cx),
                    Surface::Runs => self.refresh_runs_panel(cx),
                    Surface::Admin => self.refresh_admin_panel(cx),
                }
            }
        }
        self.scrub_audio(cx, event);
        if self.audio_timer.is_event(event).is_some() && audio::is_ready() {
            self.sync_audio_ui(cx);
        }
        self.drain_rendered_thumbnails(cx);
        // The IO worker wakes the loop with SignalToUI; drain on Signal (and
        // it is cheap enough that a spurious shared signal costs nothing).
        if let Event::Signal = event {
            self.drain_artifact_io(cx);
        }
        if self.thumbnail_timer.is_event(event).is_some() {
            self.pump_thumbnail_backfill(cx);
        }

        if self.fleet_timer.is_event(event).is_some() {
            if let Some(fleet) = &mut self.fleet {
                if let Some(discovered) = &self.discovered {
                    if fleet.reconcile_discovered(&discovered.nodes()) {
                        log!("fleet: discovery set reconciled");
                    }
                }
                fleet.poll(cx);
            }
            self.maybe_connect_chat();
        }
        if self.job_timer.is_event(event).is_some() {
            // Admission is re-evaluated for every stage, not just the first:
            // e.g. expand → video can finish text while H3 is waiting for
            // enough free VRAM. Snapshot once, calculate each run's occupied
            // slot exclusions before its mutable borrow, then let Pipeline
            // resume the held stage when a compatible GPU becomes admitted.
            let snapshots = self
                .fleet
                .as_ref()
                .map(|fleet| fleet.snapshots.clone())
                .unwrap_or_default();
            let mut tick_events = Vec::new();
            for index in 0..self.runs.len() {
                let avoid = self.avoid_for_run(index);
                let run_id = self.runs[index].id;
                let events = self.runs[index]
                    .pipeline
                    .tick(cx, &snapshots, &avoid);
                if !events.is_empty() {
                    tick_events.push((run_id, events));
                }
            }
            for (run_id, events) in tick_events {
                self.on_run_events(cx, run_id, events);
            }
            // Live elapsed timers while any run is active.
            if self.any_run_running() {
                self.refresh_stages(cx);
                if self.surface == Surface::Runs {
                    self.refresh_runs_panel(cx);
                }
                self.ui.redraw(cx);
            }
        }
        if self.surface_timer.is_event(event).is_some() {
            let surface = match self.auto.surface.as_deref() {
                Some("library") | Some("library-server") => Surface::Library,
                Some("import") => Surface::Import,
                Some("runs") => Surface::Runs,
                Some("admin") => Surface::Admin,
                Some("chat") => Surface::Chat,
                _ => Surface::Create,
            };
            if self.auto.surface.as_deref() == Some("library-server") {
                self.set_lib_source(cx, LibSource::Server);
            }
            self.show_surface(cx, surface);
            if let Some(name) = self.auto.import.take() {
                match name.to_ascii_lowercase().as_str() {
                    "duke3d" | "duke" => {
                        log!("auto: queue Duke3D shareware");
                        self.enqueue_import(
                            cx,
                            ImportJob::Duke3d {
                                path: String::new(),
                            },
                        );
                    }
                    "quake3" | "quakeiii" | "q3" => {
                        log!("auto: queue Quake III demo");
                        self.enqueue_import(
                            cx,
                            ImportJob::Quake3 {
                                path: String::new(),
                            },
                        );
                    }
                    other => log!("auto: unknown ASSET_UI_IMPORT={other}"),
                }
            }
        }
        if self.sample_timer.is_event(event).is_some() {
            // ASSET_UI_DARK=1: night stage for headless viewer captures.
            if crate::asset_store_state::env_alias(&["ASSET_UI_DARK", "AI_CONTENT_DARK"]).is_some() {
                self.set_mesh_dark(cx, true);
            }
            // ASSET_UI_OPEN_VIEW_DROP=1: open the View popup (popup placement captures).
            if crate::asset_store_state::env_alias(&["ASSET_UI_OPEN_VIEW_DROP", "AI_CONTENT_OPEN_VIEW_DROP"]).is_some() {
                if let Some(mut drop) = self.ui.drop_down2(cx, ids!(pbr_view_drop)).borrow_mut() {
                    drop.set_active(cx);
                }
            }
            // ASSET_UI_PBR_VIEW=<index|label>: inspection view for captures.
            if let Some(view) =
                crate::asset_store_state::env_alias(&["ASSET_UI_PBR_VIEW", "AI_CONTENT_PBR_VIEW"])
            {
                use crate::mesh_view::pbr_preview::PbrViewMode;
                let index = PbrViewMode::ALL
                    .iter()
                    .position(|mode| mode.label().eq_ignore_ascii_case(&view))
                    .or_else(|| view.parse::<usize>().ok())
                    .unwrap_or(0);
                self.ui
                    .drop_down2(cx, ids!(pbr_view_drop))
                    .set_selected_item(cx, index);
                if let Some(mut mesh) = self
                    .ui
                    .widget(cx, ids!(mesh_view))
                    .borrow_mut::<MeshView>()
                {
                    mesh.set_pbr_view_mode(cx, PbrViewMode::ALL[index.min(PbrViewMode::ALL.len() - 1)]);
                }
            }
            let sample = self.auto.sample.clone();
            match sample.as_deref() {
                Some("mesh") => self.load_sample_mesh(cx),
                Some("splat") => self.load_sample_splat(cx),
                Some("latest") => {
                    let newest = self
                        .library
                        .as_ref()
                        .and_then(|library| library.newest_items().next())
                        .map(|item| item.file.clone());
                    if let Some(file) = newest {
                        self.reopen_gallery(cx, &file);
                    }
                }
                Some(path) if std::path::Path::new(path).is_file() => {
                    match std::fs::read(path) {
                        Ok(bytes) => self.import_playtest_artifact(cx, path, bytes),
                        Err(error) => log!("sample: cannot read {path}: {error}"),
                    }
                }
                _ => {}
            }
            if self.auto.capture.is_some() {
                // A few frames for the offscreen pass / splat sort to settle.
                self.capture_timer = cx.start_timeout(4.0);
            }
        }
        if self.capture_timer.is_event(event).is_some() && !self.auto.captured {
            if let Some(path) = self.auto.capture.clone() {
                self.auto.captured = true;
                log!("capture: {}", path.display());
                cx.capture_next_frame_to_file(path);
                if self.auto.exit {
                    self.exit_timer = cx.start_interval(0.5);
                }
            }
        }
        if self.exit_timer.is_event(event).is_some() {
            // Exit only once the readback really landed (rig capture lesson).
            let written = self
                .auto
                .capture
                .as_ref()
                .is_some_and(|path| path.is_file());
            if written {
                log!("capture written, exiting");
                std::process::exit(0);
            }
        }
        if self.video_pump.is_event(event).is_some() {
            self.pump_video(cx);
        }
        if self.audio_pump.is_event(event).is_some() {
            // Smooth playhead while audibly playing and visible; parks
            // otherwise (transport clicks, scrubs and the 10Hz timer keep
            // the line honest when parked).
            self.update_audio_playhead(cx);
            if audio::is_playing() && self.surface == Surface::Create && self.audio_page_active {
                self.audio_pump = cx.new_next_frame();
            }
        }

        if let Event::NetworkResponses(responses) = event {
            let mut fleet_changed = false;
            let mut run_events: Vec<(u64, Vec<PipelineEvent>)> = Vec::new();
            for item in responses.iter() {
                if let Some(fleet) = &mut self.fleet {
                    if fleet.handle_response(cx, item) {
                        fleet_changed = true;
                        continue;
                    }
                }
                // Globally-unique request ids make ownership unambiguous
                // across concurrent runs; each claims exactly its traffic.
                let Some(position) = self
                    .runs
                    .iter()
                    .position(|run| run.pipeline.owns_response(item))
                else {
                    continue;
                };
                let avoid = self.avoid_for_run(position);
                let snapshots = self
                    .fleet
                    .as_ref()
                    .map(|fleet| fleet.snapshots.clone())
                    .unwrap_or_default();
                let run_id = self.runs[position].id;
                let events = self.runs[position].pipeline.handle_response(
                    cx,
                    item,
                    &snapshots,
                    &avoid,
                );
                if !events.is_empty() {
                    run_events.push((run_id, events));
                }
            }
            if fleet_changed {
                self.push_fleet_view();
                self.refresh_fleet_ui(cx);
                if self.surface == Surface::Runs {
                    self.refresh_runs_panel(cx);
                }
                self.maybe_fire_auto(cx);
                // Capacity may have appeared (box back up, model loaded).
                if !self.run_queue.is_empty() {
                    self.try_dispatch_pending(cx);
                }
            }
            for (run_id, events) in run_events {
                self.on_run_events(cx, run_id, events);
            }
        }

        self.ui.handle_event(cx, event, &mut Scope::empty());
        // Draw passes above recorded any gallery-preview cache misses;
        // route them through the IO worker (bounded, deduplicated).
        self.pump_gallery_previews(cx);
    }
}
