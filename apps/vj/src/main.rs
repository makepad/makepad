#![allow(dead_code)]
//! Makepad VJ — live performance console over the Asset Server.
//!
//! Console window: five surfaces — VIDEO program tiles, MUSIC DJ decks, SFX
//! one-shot pads, MESH dancers (Mesh + Character kinds), and GENERATE
//! (prompt → Asset Server job → fleet) — all fed by the shared asset client
//! (session lifecycle, search, verified blob cache, committed-catalog event
//! subscription, jobs). Output window: the crossfaded video program or the
//! 3D dancer, maximizable to a projector.
//!
//! Architecture: every performance behavior lives in a pure engine module
//! (`cue`, `decks`, `pads`, `catalog`, `gen`, `lanes`) that turns clicks +
//! stamped completions into commands; this file only maps commands onto the
//! session runtimes, the decode workers, the audio mixer and the widgets.
//! Nothing blocks the UI thread: blobs, decodes, thumbnail decodes and mesh
//! preparation complete on workers; stale completions die on generation
//! stamps; superseded media fetches are CANCELLED on their lane; decoder
//! teardown is detached.

pub use makepad_widgets;
use makepad_widgets::*;
use makepad_widgets::value_input::{ValueInput, ValueInputAction};
use makepad_widgets::makepad_platform::file_dialogs::{FileDialog, FileDialogAction};
use crate::import_ui::ImportPanel;
use crate::local_store::LocalStore;

mod apc40;
mod beat_sync;
mod billboard;
mod catalog;
mod chat;
mod cue;
mod decks;
// VJ effect renderstack: mesh-generating engines configured by splash
// documents (see effects/mod.rs for the document contract). Also compiled
// standalone by the effect_gallery example.
mod effects;
mod flow;
// FLOW-WARP PLAYBACK: GPU warp pre-pass over the mkfl motion fields — also
// compiled standalone by the flow_warp_lab example.
mod flow_warp;
// EFFECT SLOTS: the vjeffect content category's home in the mixer — three
// slots (EFFECT A | TRANSITION | EFFECT B) above the crossfader, loaded by
// clicking FX tiles in the browse grid (see fx_slot.rs).
mod fx_slot;
// Lazy ANIMATED thumbnails for vjeffect tiles: a hidden slot-mode VjFxView
// renders each effect offscreen into a declared-cells sheet the grid
// already knows how to animate (digest-keyed disk cache; see fx_thumbs.rs).
mod fx_thumbs;
mod import_ui;
mod gen;
mod lanes;
mod loop_detect;
// Karaoke: whisper over the separated vocals stem, cached beside the stems.
mod lyrics;
// Word-level karaoke timing: cross-attention DTW + teacher forcing + onset
// snap. Also compiled standalone by the audit harness (bin karaoke_align).
pub use makepad_audio_lyrics::align as lyrics_align;
mod media;
mod mesh_view;
// MIDI LEARN: alt-click (or LEARN+click) any wrapped dial/slider, wiggle a
// CC, and that CC drives it from then on — persistent (midi-map.txt).
mod midi_learn;
mod mix;
mod mixer;
// Two-deck music mode: deck DSP, off-thread track analysis, deck surface.
mod music_dsp;
mod music_view;
mod stems;
mod wave_analysis;
mod pads;
mod local_store;
mod media_scan;
mod service;
// Stems/lyrics the STORE already holds: fetch instead of separate, and give
// back what this machine had to compute.
mod side_channels;
mod views;

use crate::apc40::{
    palette_velocity, thumb_color, Apc40State, ApcAction, ApcSurface, LedDiff, LedFrame, PadLed,
    PAD_COUNT,
};
use crate::beat_sync::{
    fit_loop_to_grid, BeatClock, BeatFit, BeatLockState, BeatSnapshot, BeatSyncAnalyzer,
    BeatTarget, ClockSource, TapTempo,
};
use crate::catalog::{BrowseModel, CatCmd, CatGen, TileMedia, TileThumb};
use crate::cue::{CueCmd, CueEngine, CueGen, CueItem, CueScheduleId, SlotId};
use crate::loop_detect::{
    analyze_video_loop, FrameSignature, LoopDetection, LoopKind, MotionSummary,
};
use crate::decks::{
    DeckCmd, DeckEngine, DeckId, DeckLoad, DeckTarget, FadeCurve, ScratchMotion, SyncMode,
    SyncView, TrackItem, TrackSideChannels,
};
use crate::side_channels::{
    FetchedJob, SideChannelMsg, SideChannelPool, WriteBackJob, WriteBackMsg, WriteBackPool,
};
use crate::music_view::{
    format_bpm, format_duration, format_pitch, track_list_hits, OverviewEvent, TrackKey,
    TrackListHit, TrackRowEntry, VjTrackList, VjWaveOverview, VjWaveScroll, WaveEvent, WaveLane,
};
use crate::lyrics::{
    KaraokeSchedule, KaraokeTiming, LyricsDispatch, LyricsJob, LyricsMsg, LyricsPool, TrackLyrics,
};
use crate::stems::{StemsJob, StemsMsg, StemsPool};
use crate::wave_analysis::{AnalysisJob, AnalysisKey, AnalysisPool, TrackAnalysis, TrackGrid};
use crate::fx_slot::{
    FxSlotKind, FxSlotTileAction, FxSlotTileState, FxSlots, PremixJob, VjFxSlotHost, VjFxSlotTile,
};
use crate::midi_learn::{LearnEvent, LearnWrapAction, MidiLearn, VjLearnWrap};
use makepad_asset_widgets::{VideoAction, VideoView};
use crate::gen::{GenCmd, GenModel, GenTag, ProfilesState};
use crate::lanes::{LatestWins, AUDIO_LANE};
use crate::media::{DecodeDone, DecodeJob, DecodePool, SlotPlayer};
use crate::mixer::{
    TrackStems,
    Mixer, TrackPcm, VideoTransitionError, VideoTransitionId, VideoTransitionPhase,
    MAX_VIDEO_PLAYBACK_RATE, MIN_VIDEO_PLAYBACK_RATE,
};
use crate::pads::{PadCmd, PadEngine, PadItem};
use crate::chat::{ChatBridge, ChatData};
use crate::views::{GridEntry, JobRowEntry, VjJobList, VjPadMatrix, VjTileGrid, GRID_SLOTS};
use makepad_widgets::splitter::{Splitter, SplitterAlign};
use makepad_widgets::widget_tree::WidgetTreeStats;
use crate::mix::MixState;
use makepad_asset_client::side_channels::SideChannelOutcome;
use makepad_asset_client::{
    select_file, CatalogSubscriptionEvent, ClientError, ClientEvent, ClientOutput, ClientRequest,
    JobId, RequestId, SessionConnector, SessionHandles, SessionMsg, SessionStatus, TierPreference,
};
use makepad_asset_data::{
    Anchor, AssetId, AssetKind, AssetManifest, AssetRevisionId, BlobId, DeviceTier, FileRole,
    MediaType,
};
use makepad_show_control::{
    is_vj_reserved_midi, ColorControl, HazardArms, HazardControl,
    MoverControl, PerformanceConfig, PerformanceState, PowerCaps, PresetBank, RoomShow,
    SpatialLightSample, StrobeControl, VideoLightAnalyzer, ARTNET_BROADCAST_ADDR,
};
// Only exercised by `program_light_mix_uses_the_exact_picture_fraction_and_blackout_gate`.
#[cfg(test)]
use makepad_show_control::LightSample;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let SearchRow = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: 8
        align: Align{x: 0.0, y: 0.5}
    }

    let PanelLabel = Label{
        draw_text.color: #xa6b1bd
        draw_text.text_style.font_size: 10
    }

    let ValueLabel = Label{
        draw_text.color: #xe8eef4
        draw_text.text_style.font_size: 11
    }

    let ChromeButton = Button{
        draw_bg +: {
            color: #x272e38
            color_focus: #x272e38
            color_hover: #x2b3440
            color_down: #x1e232b
            border_color: #xffffff2e
            border_radius: 6.0
            border_size: 1.0
        }
        draw_text +: {
            color: #xd6dee6
            color_focus: #xd6dee6
            color_hover: #xfffaf4
            text_style: theme.font_regular{font_size: 10}
        }
    }

    let PillButton = Button{
        draw_bg +: {
            color: #x222831
            color_hover: #x2f3842
            color_down: #x1c2129
            border_color: #xffffff26
            border_radius: 12.0
            border_size: 1.0
        }
        draw_text +: {
            color: #xb4bfca
            color_hover: #xff5c39
            text_style: theme.font_bold{font_size: 10}
        }
    }

    // Horizontal VJ parameter slider in the fader/xfader family (not the
    // theme's thin grey slider).
    let ApcHSlider = Slider{
        width: Fill
        height: 22
        min: 0.0
        max: 1.0
        text: ""
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            body_color: uniform(#x1d222a)
            track_color: uniform(#x2b343f)
            fill_color: uniform(#xff5c39)
            cap_color: uniform(#xe8eef4)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let track_h = 6.
                let track_y = (self.rect_size.y - track_h) * 0.5
                sdf.box(1., track_y, self.rect_size.x - 2., track_h, 3.)
                sdf.fill(self.track_color)
                // Inset travel: the cap never clips at 0% / 100%.
                let cap_w = 10.
                let left = 1. + cap_w * 0.5
                let w = self.rect_size.x - 2. - cap_w
                let fill_w = max(1., w * self.slide_pos + cap_w * 0.5)
                sdf.box(1., track_y, fill_w, track_h, 3.)
                sdf.fill(self.fill_color)
                let cap_x = left + w * self.slide_pos - cap_w * 0.5
                sdf.box(cap_x, 3., cap_w, self.rect_size.y - 6., 3.)
                sdf.fill(self.cap_color)
                return sdf.result
            }
        }
    }

    let DeckWell = RoundedView{
        width: Fill
        height: Fill
        padding: 1
        draw_bg +: {
            color: #x000000
            border_color: #xffffff26
            border_size: 1.0
            border_radius: 10.0
        }
    }

    // APC40 knob mirror: dark body, 270-degree LED ring (gap at the bottom,
    // like the hardware), white pointer. The stock Rotary is drawn for a
    // 65x95 well with a label gutter and vanishes at pad size.
    let ApcKnob = Rotary{
        width: 44
        height: 44
        min: 0.0
        max: 1.0
        text: ""
        flow: Down
        text_input: TextInput{
            width: 0
            height: 0
        }
        draw_bg +: {
            body_color: uniform(#x1c222b)
            body_color_hover: uniform(#x2a323d)
            rim_color: uniform(#xffffff40)
            ring_color: uniform(#x2f3842)
            val_color: uniform(#xff5c39)
            pointer_color: uniform(#xf2f6fa)
            // Slot dials the loaded effect does not declare dim to inert
            // (fixed dial count keeps MIDI maps stable; the dimming keeps
            // the strip honest about which ones do anything).
            inert: instance(0.0)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                let c = self.rect_size * 0.5
                let r = min(self.rect_size.x, self.rect_size.y) * 0.5
                // Sdf2d arcs: angle 0 points DOWN, increasing = clockwise.
                let start = PI * 0.25
                let sweep = PI * 1.5
                sdf.arc_round_caps(c.x, c.y, r - 2.5, start, start + sweep, 2.5)
                sdf.fill(self.ring_color)
                let lit = max(self.slide_pos, 0.01)
                sdf.arc_round_caps(c.x, c.y, r - 2.5, start, start + sweep * lit, 2.5)
                sdf.fill(self.val_color)
                sdf.circle(c.x, c.y, r - 7.5)
                sdf.fill_keep(self.body_color.mix(self.body_color_hover, max(self.hover, self.drag)))
                sdf.stroke(self.rim_color, 1.0)
                let a = start + sweep * self.slide_pos
                let d = vec2(-sin(a), cos(a))
                let p0 = c + d * (r - 13.5)
                let p1 = c + d * (r - 8.5)
                sdf.move_to(p0.x, p0.y)
                sdf.line_to(p1.x, p1.y)
                sdf.stroke(self.pointer_color, 2.0)
                return sdf.result * (1.0 - self.inert * 0.72)
            }
        }
    }

    // 22px icon button for the cue strips / console. The host paints `lit`
    // state (playing, loop on, spin on) through draw_bg.color like FxButton.
    let IconButton = ButtonIcon{
        width: 24
        height: 22
        padding: 0
        // ONE BUTTON FAMILY app-wide (the pager's well + glyph ratio):
        // 24x22 wells, 9-wide glyphs, comfortable padding everywhere.
        icon_walk: Walk{width: 9 height: Fit}
        draw_bg +: {
            color: #x272e38
            color_focus: #x272e38
            color_hover: #x2b3440
            color_down: #x1e232b
            border_color: #xffffff26
            border_radius: 5.0
            border_size: 1.0
        }
        draw_icon +: {
            color: #xd6dee6
        }
    }

    // Knob with its legend directly above, so legend and knob always line up.
    let KnobCol = View{
        width: 44
        height: Fit
        flow: Down
        spacing: 2
        align: Align{x: 0.5, y: 0.0}
    }

    let ApcFader = Slider{
        axis: DragAxis.Vertical
        width: 44
        height: 108
        min: 0.0
        max: 1.0
        text: ""
        flow: Down
        text_input: TextInput{
            width: 0
            height: 0
        }
        draw_bg +: {
            body_color: uniform(#x1d222a)
            track_color: uniform(#x2b343f)
            fill_color: uniform(#xff5c39)
            cap_color: uniform(#xe8eef4)
            cap_shadow: uniform(#x8d98a7)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(3., 2., self.rect_size.x - 6., self.rect_size.y - 4., 6.)
                sdf.fill(self.body_color)
                let top = 10.
                let bottom_pad = 10.
                let h = self.rect_size.y - top - bottom_pad
                let track_w = 8.
                let track_x = (self.rect_size.x - track_w) * 0.5
                sdf.box(track_x, top, track_w, h, 3.)
                sdf.fill(self.track_color)
                let fill_h = max(1., h * self.slide_pos)
                sdf.box(track_x + 1.5, top + (h - fill_h) + 1.5, track_w - 3., max(1., fill_h - 3.), 2.)
                sdf.fill(self.fill_color)
                let cap_h = 14.
                let cap_y = top + (h - fill_h) - cap_h * 0.5
                sdf.box(6., cap_y + 1.5, self.rect_size.x - 12., cap_h, 4.)
                sdf.fill(self.cap_shadow)
                sdf.box(5., cap_y, self.rect_size.x - 10., cap_h, 4.)
                sdf.fill(self.cap_color)
                return sdf.result
            }
        }
    }

    let ApcPad = ChromeButton{
        width: Fill
        height: 22
        draw_text +: {
            text_style: theme.font_bold{font_size: 8}
        }
    }

    let Tick = Label{
        width: Fill
        draw_text.color: #xa6b1bd
        draw_text.text_style: theme.font_bold{font_size: 8}
    }

    let FaderCol = View{
        width: 44
        height: Fit
        flow: Down
        spacing: 3
        align: Align{x: 0.5, y: 0.0}
    }

    let ApcXfader = Slider{
        width: Fill
        height: 44
        min: 0.0
        max: 1.0
        text: ""
        text_input: TextInput{width: 0 height: 0}
        draw_bg +: {
            body_color: uniform(#x1d222a)
            track_color: uniform(#x2b343f)
            fill_color: uniform(#xff5c39)
            cap_color: uniform(#xe8eef4)
            cap_shadow: uniform(#x8d98a7)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(2., 6., self.rect_size.x - 4., self.rect_size.y - 12., 8.)
                sdf.fill(self.body_color)
                // The cap's CENTER travels an inset span, so the cap body
                // never clips at the extremes (cap 22 wide → 15px inset
                // each end keeps it fully inside the chrome).
                let left = 15.
                let right_pad = 15.
                let w = self.rect_size.x - left - right_pad
                let track_h = 10.
                let track_y = (self.rect_size.y - track_h) * 0.5
                sdf.box(left, track_y, w, track_h, 4.)
                sdf.fill(self.track_color)
                let fill_w = max(1., w * self.slide_pos)
                sdf.box(left + 1.5, track_y + 1.5, max(1., fill_w - 3.), track_h - 3., 3.)
                sdf.fill(self.fill_color)
                let cap_w = 22.
                let cap_x = left + fill_w - cap_w * 0.5
                sdf.box(cap_x + 1.5, 8., cap_w, self.rect_size.y - 16., 6.)
                sdf.fill(self.cap_shadow)
                sdf.box(cap_x, 6., cap_w, self.rect_size.y - 14., 6.)
                sdf.fill(self.cap_color)
                return sdf.result
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Makepad VJ"
                window.inner_size: vec2(1680, 1040)
                window.position: vec2(40, 24)
                // No caption bar: the picture owns the top edge. The dot
                // gripper in the top bar answers WindowDragQuery so the
                // window can still be moved.
                show_caption_bar: false
                body +: {
                    View{
                    width: Fill
                    height: Fill
                    flow: Overlay
                    SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 4
                        padding: Inset{left: 8.0 right: 8.0 top: 4.0 bottom: 8.0}
                        draw_bg.color: #x14171c

                        // ---- status / navigation bar (top). Left padding leaves
                        // room for the macOS traffic lights; the window drags by
                        // the dot gripper only, never by a control.
                        status_bar := View{
                            width: Fill
                            height: 28
                            flow: Right
                            spacing: 10
                            // Its own draw list. The four OFFSCREEN 3D views
                            // below live in this bar, and each one re-renders
                            // a full pass every time the bar is walked — so
                            // without this every scratch frame on the deck
                            // surface re-rendered two levels and two splat
                            // scenes. They now redraw when THEY change.
                            new_batch: true
                            // +1: the bar's contents sit 1px low of the
                            // window edge optics (user-calibrated).
                            padding: Inset{left: 74.0 right: 8.0 top: 1.0 bottom: 0.0}
                            align: Align{x: 0.0, y: 0.5}
                            // THE window grip: as the bar filled with
                            // controls, "drag anywhere that isn't a button"
                            // became a guessing game — so one VISIBLE dot-grid
                            // glyph at the far left (right of the traffic
                            // lights on macOS) is the always-draggable spot,
                            // and the drag-query answers over it alone.
                            // THE MAKEPAD LOGO — the brand lockup from
                            // makepad.nl (orange mark + cream wordmark),
                            // flattened into two single-color icon svgs.
                            // It IS the window's drag handle: the
                            // WindowDragQuery answers Caption over this
                            // rect, and icons consume no clicks.
                            Tip{ text: "Drag to move the window"
                                win_grip := View{
                                    width: Fit
                                    height: 22
                                    flow: Right
                                    spacing: 7
                                    align: Align{x: 0.0, y: 0.5}
                                    Icon{
                                        icon_walk: Walk{width: 24 height: Fit}
                                        draw_icon +: {
                                            svg: crate_resource("self:resources/icons/logo_mark.svg")
                                            color: #xff5c39
                                        }
                                    }
                                    Icon{
                                        icon_walk: Walk{width: 96 height: Fit}
                                        draw_icon +: {
                                            svg: crate_resource("self:resources/icons/logo_word.svg")
                                            color: #xfae1bc
                                        }
                                    }
                                }
                            }
                            // The three MODES sit far left, where the
                            // wordmark used to be — the lit mode button IS
                            // the label. VJ = the visual surface, DJ the
                            // two-deck music mode, SFX the pad sampler.
                            mode_vj := PillButton{text: "VJ"}
                            mode_dj := PillButton{text: "DJ"}
                            mode_sfx := PillButton{text: "SFX"}
                            // OFFSCREEN RENDER HOSTS — every 4x4 heartbeat
                            // widget stacked in ONE overlay slot with a
                            // bar-colored cover on top: the sample draws
                            // (the frame dependencies that make the
                            // offscreen chains render) are still issued,
                            // but no speck of sampled video reaches the
                            // bar. (The cleaner idiom — no parked widgets,
                            // passes driven by repaint_pass from the pump —
                            // needs the hosts' pass orchestration lifted
                            // out of draw_walk first; see CONTRACT.md.)
                            View{
                            width: 4
                            height: 4
                            flow: Overlay
                            // Offscreen A/B mesh passes. Each slot has its own
                            // color+depth so two models never share a z-buffer;
                            // VideoProgram samples the textures.
                            slot_mesh_a := VjMeshView{
                                width: 4
                                height: 4
                                composite: false
                                stage: false
                            }
                            slot_mesh_b := VjMeshView{
                                width: 4
                                height: 4
                                composite: false
                                stage: false
                            }
                            // Offscreen A/B Gaussian-splat scenes, rendered at
                            // program resolution behind 4x4 placeholders.
                            slot_splat_a := XrSceneView{
                                width: 4
                                height: 4
                                render_size: vec2(1280.0, 720.0)
                                clear_color: vec4(0.0, 0.0, 0.0, 1.0)
                                camera.distance: 3.2
                                camera.distance_min: 0.5
                                splat := ViewSplat{}
                            }
                            slot_splat_b := XrSceneView{
                                width: 4
                                height: 4
                                render_size: vec2(1280.0, 720.0)
                                clear_color: vec4(0.0, 0.0, 0.0, 1.0)
                                camera.distance: 3.2
                                camera.distance_min: 0.5
                                splat := ViewSplat{}
                            }
                            // Offscreen A/B flow-warp passes (see flow_warp.rs):
                            // when a cued clip carries an mkfl motion payload,
                            // this pass synthesizes the picture at any t and
                            // its texture replaces the slot's decoder texture.
                            slot_flow_a := FlowWarpView{}
                            slot_flow_b := FlowWarpView{}
                            // Offscreen vjeffect thumbnail renderer: one
                            // hidden slot-mode effect pass at a time, its
                            // sheets fed back through the thumb decode lane.
                            fx_thumbs := VjFxThumbs{}
                            // Offscreen EFFECT-SLOT passes (fx_slot.rs): the
                            // per-deck effect passes and the transition
                            // effect, each a program-sized slot-mode
                            // VjFxView behind a 4x4 placeholder. They live
                            // in the always-drawn bar so the program keeps
                            // its effects even when the mixer column
                            // scrolls out of view.
                            fx_host_a := VjFxSlotHost{}
                            fx_host_t := VjFxSlotHost{}
                            fx_host_b := VjFxSlotHost{}
                            // Offscreen CONTENT-mode effect hosts: an FX
                            // tile cued onto a deck (no slot armed) renders
                            // here and feeds the slot like any clip.
                            fx_content_a := VjFxSlotHost{}
                            fx_content_b := VjFxSlotHost{}
                            SolidView{
                                width: 4
                                height: 4
                                draw_bg.color: #x14171c
                            }
                            }
                            apc_map_label := PanelLabel{width: 0 text: ""}
                            status_label := Label{
                                width: Fill
                                flow: Flow.Right{wrap: false}
                                max_lines: 1
                                text: "starting…"
                                draw_text.color: #xa9b4bf
                                draw_text.text_style.font_size: 10
                            }
                            // ONE beat block: a live wave of the captured
                            // audio with the beat grid ruled over it (so the
                            // rulings visibly sit on the transients), the LED
                            // that flashes the beat, the BPM, and the lock as
                            // a word. Its own draw batch: the wave and the LED
                            // animate every frame and must not drag the rest
                            // of the bar's geometry along with them. Fixed
                            // widths so the beat pump cannot reflow the bar.
                            beat_cluster := View{
                                width: Fit
                                height: Fit
                                flow: Right
                                spacing: 6
                                align: Align{x: 0.0, y: 0.5}
                                new_batch: true
                                beat_wave := VjBeatWave{width: 120 height: 22}
                                beat_led := VjBeatLed{width: 18 height: 22}
                            }
                            // BPM as a Blender-style value field: drag to
                            // bend (±0.1/px), click to type, hover chevrons
                            // step ±0.1. Manual set = TAP authority, phase
                            // untouched. RESYNC sits with the value it
                            // acts on.
                            Tip{ text: "Tempo: drag to bend, click to type"
                                bpm_field := ValueInput{
                                    width: 64
                                    min: 40.0
                                    max: 300.0
                                    step: 0.1
                                    precision: 1.0
                                }
                            }
                            Tip{ text: "Resync the beat from the audio"
                                external_sync_now := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/resync.svg") } }
                            }
                            external_lock := Label{
                                visible: false
                                flow: Flow.Right{wrap: false}
                                max_lines: 1
                                width: 0
                                text: ""
                            }
                            external_confidence := Label{
                                visible: false
                                flow: Flow.Right{wrap: false}
                                max_lines: 1
                                width: 0
                                text: "CONF:   0%"
                                draw_text.color: #xa9b4bf
                                draw_text.text_style.font_size: 9
                            }
                            external_phase := Label{
                                visible: false
                                flow: Flow.Right{wrap: false}
                                max_lines: 1
                                width: 0
                                text: "BEAT -/4 [........] PHASE   0%"
                                draw_text.color: #xff8f70
                                draw_text.text_style: theme.font_bold{font_size: 9}
                            }
                            external_capture := Label{
                                width: 0
                                visible: false
                                text: ""
                            }
                            external_video_state := Label{
                                width: 0
                                visible: false
                                text: ""
                            }
                            external_loop_state := Label{
                                visible: false
                                width: 0
                                text: ""
                            }
                            // MONITOR AUDIO: the system-loopback capture
                            // behind the beat clock, OFF until the operator
                            // flips it — no screen-recording TCC prompt at
                            // launch, and the clock falls back to TAP/FREE.
                            // The glyph says the whole story: speaker →
                            // wave → beat ticks (system audio locks the
                            // beat). Lit green while capturing.
                            monitor_audio_tip := Tip{ text: "Monitor system audio (beat lock)"
                                monitor_audio_btn := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/beat_lock.svg") } }
                            }
                            // TAP is the operator's clock, and it says two
                            // things with one gesture: ONE press moves the
                            // downbeat to the press and leaves the tempo
                            // alone ("the one is HERE"), four rhythmic
                            // presses hand over the tempo as well.
                            Tip{ text: "Tap tempo: one press moves the downbeat"
                                beat_tap := ChromeButton{
                                    width: 46
                                    text: "TAP"
                                }
                            }
                            // Global MIDI-learn: press, then click a dial
                            // to map; wiggle a CC to bind it. Top bar —
                            // where a DAW keeps it.
                            // MIDI learn behind the universal mark: the
                            // 5-pin DIN, lit while learn mode is on.
                            Tip{ text: "MIDI learn: click a control, wiggle a CC"
                                midi_learn_btn := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/midi.svg") } }
                            }
                            // The RIG GROUP: karaoke overlay pair, master
                            // fadeout, and the output window — the things
                            // that shape what the ROOM sees.
                            Tip{ text: "Karaoke overlay"
                                karaoke_enable := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/karaoke.svg") } }
                            }
                            Tip{ text: "Word-hop timing"
                                karaoke_word_hops := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/wordhop.svg") } }
                            }
                            // FADEOUT (output-window dim) as a DROPDOWN
                            // SLIDER — same idiom as volume, no bar drags.
                            Tip{ text: "Fade the output to black"
                                fadeout_learn := Learn{
                                    fadeout_knob := DropSlider{
                                        min: 0.0
                                        max: 1.0
                                        default: 0.0
                                        display_scale: 100.0
                                        suffix: "%"
                                        draw_icon +: { svg: crate_resource("self:resources/icons/fade.svg") }
                                    }
                                }
                            }
                            Tip{ text: "Output window"
                                open_output := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/monitor.svg") } }
                            }
                            // MASTER VOLUME as a DROPDOWN SLIDER: the chip
                            // is a plain click target (no drag in the bar —
                            // it can never fight window-dragging); the
                            // slider lives in a popover below.
                            Tip{ text: "Master volume"
                                master_learn := Learn{
                                    master_slider := DropSlider{
                                        min: 0.0
                                        max: 1.2
                                        default: 0.9
                                        display_scale: 100.0
                                        suffix: "%"
                                        draw_icon +: { svg: crate_resource("self:resources/icons/volume.svg") }
                                    }
                                }
                            }
                            // The stock Slider is drawn for a tall well with a
                            // label gutter and is INVISIBLE at 28px of chrome —
                            // this is the same fader the FX/light strips use.
                            // The master program-audio mute lives with the
                            // MASTER it silences — nothing floats loose in
                            // panel space.
                            video_mute := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/mute.svg") } }
                            // Windows-only chrome: the borderless window's
                            // min/max/close cluster floats at the bar's right
                            // edge (made visible at startup on that OS).
                            win_buttons := View{
                                visible: false
                                width: Fit height: Fit
                                flow: Right
                                win_min := DesktopButton{
                                    draw_bg.button_type: DesktopButtonType.WindowsMin
                                    width: 40 height: 26
                                    draw_bg +: {
                                        color: #xd6dee6, color_hover: #xffffff, color_down: #xffffff
                                        bg_color_hover: #x2b3440, bg_color_down: #x1e232b
                                    }
                                }
                                win_max := DesktopButton{
                                    draw_bg.button_type: DesktopButtonType.WindowsMax
                                    width: 40 height: 26
                                    draw_bg +: {
                                        color: #xd6dee6, color_hover: #xffffff, color_down: #xffffff
                                        bg_color_hover: #x2b3440, bg_color_down: #x1e232b
                                    }
                                }
                                win_close := DesktopButton{
                                    draw_bg.button_type: DesktopButtonType.WindowsClose
                                    width: 40 height: 26
                                    draw_bg +: {
                                        color: #xd6dee6, color_hover: #xffffff, color_down: #xffffff
                                        bg_color_hover: #xe81123, bg_color_down: #xf1707a
                                    }
                                }
                            }
                        }
                        gen_split := Splitter{
                            width: Fill
                            height: Fill
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.FromA(300.0)
                            min_vertical: 0.0
                            max_vertical: 360.0
                            size: 6.0
                            draw_bg +: {
                                // App-dark ground, near-invisible bar at
                                // rest, accent only under the pointer.
                                color_bg: #x14171c
                                color: #x222830
                                color_hover: #x46312b
                                color_drag: #xff5c39
                                splitter_pad: 2.0
                                bar_size: 72.0
                            }
                            b: View{
                            // Slimmer inset against the splitter: with the
                            // GEN panel closed this is the console's LEFT
                            // margin, and the filter column must read
                            // centered between window edge and grid.
                            padding: Inset{left: 4.0, right: 4.0}
                            width: Fill
                            height: Fill
                            pages := PageFlip{
                            width: Fill
                            height: Fill
                            active_page: @video_page

                            // ============ VIDEO ============
                            video_page := View{
                                width: Fill
                                height: Fill
                                flow: Down
                                spacing: 4
                                // Top pane: the three windows — cue A, program, cue B
                                // (oOo). Drag the splitter down for a bigger picture;
                                // everything else scrolls below it.
                                deck_split := Splitter{
                                    width: Fill
                                    height: Fill
                                    axis: SplitterAxis.Vertical
                                    align: SplitterAlign.FromA(290.0)
                                    size: 6.0
                                    draw_bg +: {
                                        color_bg: #x14171c
                                        color: #x222830
                                        color_hover: #x46312b
                                        color_drag: #xff5c39
                                        splitter_pad: 2.0
                                        bar_size: 72.0
                                    }
                                    a: View{
                                        width: Fill
                                        height: Fill
                                        flow: Right
                                        spacing: 8
                                        // ---- cue A ----
                                        View{
                                            width: 300
                                            height: Fill
                                            flow: Down
                                            spacing: 4
                                            // NO header/controls strip: the
                                            // corner marker overlays the
                                            // picture; every control lives
                                            // in the source transport below.
                                            DeckWell{
                                                width: Fill
                                                height: Fill
                                                View{
                                                    width: Fill
                                                    height: Fill
                                                    flow: Overlay
                                                    preview_a := VideoProgram{}
                                                    // CUE ACK: the moment a
                                                    // click lands, this deck
                                                    // says "working on it" —
                                                    // spinner over the held
                                                    // frame or the thumb.
                                                    deck_a_busy := View{
                                                        visible: false
                                                        width: Fill
                                                        height: Fill
                                                        align: Align{x: 0.5, y: 0.5}
                                                        LoadingSpinner{
                                                            width: 44
                                                            height: 44
                                                            draw_bg +: {
                                                                color: #xff5c39
                                                                stroke_width: 3.0
                                                            }
                                                        }
                                                    }
                                                    deck_a_empty := View{
                                                        width: Fill
                                                        height: Fill
                                                        align: Align{x: 0.5, y: 0.5}
                                                        Label{
                                                            text: "cue"
                                                            draw_text.color: #x6b7783
                                                            draw_text.text_style.font_size: 12
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        // ---- program ----
                                        View{
                                            width: Fill
                                            height: Fill
                                            flow: Down
                                            spacing: 4
                                            // No header row: the picture
                                            // says what it is. (Cue/error
                                            // text lives on the tiles + log.)
                                            DeckWell{
                                                width: Fill
                                                height: Fill
                                                preview := VideoProgram{}
                                            }
                                        }
                                        // ---- cue B ----
                                        View{
                                            width: 300
                                            height: Fill
                                            flow: Down
                                            spacing: 4
                                            DeckWell{
                                                width: Fill
                                                height: Fill
                                                View{
                                                    width: Fill
                                                    height: Fill
                                                    flow: Overlay
                                                    preview_b := VideoProgram{}
                                                    // CUE ACK: the moment a
                                                    // click lands, this deck
                                                    // says "working on it" —
                                                    // spinner over the held
                                                    // frame or the thumb.
                                                    deck_b_busy := View{
                                                        visible: false
                                                        width: Fill
                                                        height: Fill
                                                        align: Align{x: 0.5, y: 0.5}
                                                        LoadingSpinner{
                                                            width: 44
                                                            height: 44
                                                            draw_bg +: {
                                                                color: #xff5c39
                                                                stroke_width: 3.0
                                                            }
                                                        }
                                                    }
                                                    deck_b_empty := View{
                                                        width: Fill
                                                        height: Fill
                                                        align: Align{x: 0.5, y: 0.5}
                                                        Label{
                                                            text: "over / standby"
                                                            draw_text.color: #x6b7783
                                                            draw_text.text_style.font_size: 12
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    b: ScrollYView{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        spacing: 4
                                        // Breathing room under the splitter
                                        // bar — the strip used to sit flush
                                        // against the video pane.
                                        padding: Inset{top: 10.0}
                                        // A drag on the console background must
                                        // never pan it: on a VJ surface every drag
                                        // belongs to a control (fader, knob, scratch,
                                        // tile). Wheel/trackpad and the scrollbar
                                        // itself keep working.
                                        scroll_bars.scroll_bar_y.drag_scrolling: false
                                        // ---- console: mix controls + EFFECT SLOTS on ONE row.
                                        // The old hardwired FX bank (13 buttons + its knob strip)
                                        // is GONE — effects and transitions are catalog content
                                        // now, loaded into the three slots. The surviving mix
                                        // controls (autofade time, downstream mix mode + its two
                                        // knobs, audio mute) stack compactly LEFT of the slots;
                                        // the vertical space they used to take belongs to the
                                        // content grid below.
                                        //
                                        // Slots: EFFECT A | TRANSITION | EFFECT B, each with a
                                        // few controls — ON, SPD (effect clock), P1/P2 (the
                                        // doc's p0/p1 levers), CLEAR. Click a slot to ARM it,
                                        // then click an FX tile in the grid to load it; an
                                        // unarmed FX-tile click cues the effect AS CONTENT onto
                                        // a deck, autofade included, like any clip.
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 14
                                            align: Align{x: 0.5, y: 0.5}
                                            // The VJ / LIGHTS page tabs live UP
                                            // HERE beside the video players, not
                                            // beside the content grid — they flip
                                            // the whole lower region, so they sit
                                            // above it, at the console's left edge.
                                            View{
                                                width: Fit
                                                height: Fit
                                                flow: Down
                                                spacing: 4
                                                Tip{ text: "Video console"
                                                    lower_tab_vj := IconButton{
                                                        width: 28 height: 46
                                                        icon_walk: Walk{width: 12 height: Fit}
                                                        draw_icon +: { svg: crate_resource("self:resources/icons/vj.svg") }
                                                    }
                                                }
                                                Tip{ text: "Lighting desk"
                                                    lower_tab_lights := IconButton{
                                                        width: 28 height: 46
                                                        icon_walk: Walk{width: 12 height: Fit}
                                                        draw_icon +: { svg: crate_resource("self:resources/icons/lights.svg") }
                                                    }
                                                }
                                            }
                                            View{width: Fill height: 1}
                                            // DECK A SOURCE: the raw clip
                                            // (pre-effect) with its own
                                            // transport — play/pause, loop,
                                            // restart, scrub. The deck
                                            // monitor above stays the
                                            // COMPOSITE (video × effect).
                                            // Card: one tidy module per
                                            // cluster (the app's well idiom).
                                            RoundedView{
                                                // THE BAND LAW: every card in
                                                // the center strip is the SAME
                                                // fixed height — nothing in
                                                // this row ever moves.
                                                width: Fit height: 200 flow: Down spacing: 4
                                                padding: 6
                                                draw_bg +: {
                                                    color: #x181c23
                                                    border_color: #xffffff12
                                                    border_size: 1.0
                                                    border_radius: 9.0
                                                }
                                                // A NORMAL VIDEO PLAYER: 16:9
                                                // black canvas (clips letterbox
                                                // inside whatever their aspect),
                                                // one long scrub bar, controls
                                                // tidily on one line below.
                                                deck_a_source := VideoView{
                                                    width: 252
                                                    height: 156
                                                    bar_below: true
                                                    show_controls: false
                                                    trim_handles: true
                                                    lane +: {
                                                        show_bg: true
                                                        draw_bg +: { color: #x000000 }
                                                    }
                                                }
                                                deck_a_controls := View{
                                                    width: Fill height: 26 flow: Right spacing: 3
                                                    align: Align{x: 0.0, y: 0.5}
                                                    deck_a_play := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/play.svg") } }
                                                    deck_a_rw := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/rewind.svg") } }
                                                    deck_a_loop2 := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/loop.svg") } }
                                                    deck_a_bounce := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/bounce.svg") } }
                                                    deck_a_rate := ChromeButton{width: 30 text: "1"}
                                                    deck_a_mute := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/mute.svg") } }
                                                    // SCRATCH / SHUTTLE:
                                                    // centre neutral, right
                                                    // forward, left reverse,
                                                    // sprung home on release
                                                    // — a jog for testing
                                                    // the frame tweening.
                                                    Tip{ text: "Scratch / shuttle"
                                                        deck_a_scratch := VjShuttle{width: 72}
                                                    }
                                                    Tip{ text: "Auto-spin 3D content"
                                                        slot_a_spin := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/spin.svg") } }
                                                    }
                                                    View{width: Fill height: 1}
                                                    // UNSLOT: eject the clip,
                                                    // deck back to its empty
                                                    // cue state (the effect
                                                    // slots' × idiom).
                                                    deck_a_eject := ChromeButton{width: 22 text: "×"}
                                                }
                                                slot_a_anim_box := View{
                                                    width: Fill
                                                    height: Fit
                                                    visible: false
                                                    slot_a_anim := DropDown{width: Fill labels: ["—"]}
                                                }
                                            }
                                            // Tile over dials: the effect's face on
                                            // top, its 2x2 dial grid below — the
                                            // width this frees belongs to the deck
                                            // source monitors' transport rows.
                                            RoundedView{
                                                // THE BAND LAW: every card in
                                                // the center strip is the SAME
                                                // fixed height — nothing in
                                                // this row ever moves.
                                                width: Fit height: 200 flow: Down spacing: 4
                                                padding: 6
                                                draw_bg +: {
                                                    color: #x181c23
                                                    border_color: #xffffff12
                                                    border_size: 1.0
                                                    border_radius: 9.0
                                                }
                                                align: Align{x: 0.0, y: 0.0}
                                                // The deck-window corner
                                                // idiom: face on top, dials
                                                // tucked tight, ON bottom-
                                                // left / × bottom-right —
                                                // the card takes no more
                                                // width than its tile.
                                                fx_slot_a_tile := VjFxSlotTile{width: 142 height: 80}
                                                // (FIXED widths inside the
                                                // Fit card: a Fill child
                                                // resolves against the OUTER
                                                // context and inflates the
                                                // card past the tile.)
                                                View{
                                                    width: Fit height: Fit flow: Down spacing: 2
                                                    View{
                                                        width: Fit height: Fit flow: Right spacing: 1
                                                        align: Align{x: 0.0, y: 0.5}
                                                        fx_slot_a_spd_learn := Learn{ fx_slot_a_spd := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        Tick{width: 36 margin: Inset{left: -3.0} text: "SPD"}
                                                        fx_slot_a_d0_learn := Learn{ fx_slot_a_d0 := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        fx_slot_a_d0_lab := Tick{width: 36 margin: Inset{left: -3.0} flow: Flow.Right{wrap: false} max_lines: 1 text: "—"}
                                                    }
                                                    View{
                                                        width: Fit height: Fit flow: Right spacing: 1
                                                        align: Align{x: 0.0, y: 0.5}
                                                        fx_slot_a_d1_learn := Learn{ fx_slot_a_d1 := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        fx_slot_a_d1_lab := Tick{width: 36 margin: Inset{left: -3.0} flow: Flow.Right{wrap: false} max_lines: 1 text: "—"}
                                                        fx_slot_a_d2_learn := Learn{ fx_slot_a_d2 := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        fx_slot_a_d2_lab := Tick{width: 36 margin: Inset{left: -3.0} flow: Flow.Right{wrap: false} max_lines: 1 text: "—"}
                                                    }
                                                }
                                                View{width: 142 height: Fill}
                                                // ONE BASELINE across the
                                                // band: this row's buttons
                                                // sit at exactly the deck
                                                // transports' y (margin
                                                // solved from the measured
                                                // rects, verified by snap).
                                                View{
                                                    width: 142 height: Fit flow: Right
                                                    margin: Inset{top: -4.0}
                                                    align: Align{x: 0.0, y: 0.5}
                                                    fx_slot_a_on := ChromeButton{width: 30 text: "ON"}
                                                    View{width: Fill height: 1}
                                                    fx_slot_a_clear := ChromeButton{width: 22 text: "×"}
                                                }
                                            }
                                            // Tile over dials: the effect's face on
                                            // top, its 2x2 dial grid below — the
                                            // width this frees belongs to the deck
                                            // source monitors' transport rows.
                                            RoundedView{
                                                // THE BAND LAW: every card in
                                                // the center strip is the SAME
                                                // fixed height — nothing in
                                                // this row ever moves.
                                                width: Fit height: 200 flow: Down spacing: 4
                                                padding: 6
                                                draw_bg +: {
                                                    color: #x181c23
                                                    border_color: #xffffff12
                                                    border_size: 1.0
                                                    border_radius: 9.0
                                                }
                                                align: Align{x: 0.0, y: 0.0}
                                                // The deck-window corner
                                                // idiom: face on top, dials
                                                // tucked tight, ON bottom-
                                                // left / × bottom-right —
                                                // the card takes no more
                                                // width than its tile.
                                                fx_slot_t_tile := VjFxSlotTile{width: 142 height: 80}
                                                // (FIXED widths inside the
                                                // Fit card: a Fill child
                                                // resolves against the OUTER
                                                // context and inflates the
                                                // card past the tile.)
                                                View{
                                                    width: Fit height: Fit flow: Down spacing: 2
                                                    View{
                                                        width: Fit height: Fit flow: Right spacing: 1
                                                        align: Align{x: 0.0, y: 0.5}
                                                        fx_slot_t_spd_learn := Learn{ fx_slot_t_spd := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        Tick{width: 36 margin: Inset{left: -3.0} text: "SPD"}
                                                        fx_slot_t_d0_learn := Learn{ fx_slot_t_d0 := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        fx_slot_t_d0_lab := Tick{width: 36 margin: Inset{left: -3.0} flow: Flow.Right{wrap: false} max_lines: 1 text: "—"}
                                                    }
                                                    View{
                                                        width: Fit height: Fit flow: Right spacing: 1
                                                        align: Align{x: 0.0, y: 0.5}
                                                        fx_slot_t_d1_learn := Learn{ fx_slot_t_d1 := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        fx_slot_t_d1_lab := Tick{width: 36 margin: Inset{left: -3.0} flow: Flow.Right{wrap: false} max_lines: 1 text: "—"}
                                                        fx_slot_t_d2_learn := Learn{ fx_slot_t_d2 := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        fx_slot_t_d2_lab := Tick{width: 36 margin: Inset{left: -3.0} flow: Flow.Right{wrap: false} max_lines: 1 text: "—"}
                                                    }
                                                }
                                                View{width: 142 height: Fill}
                                                // ONE BASELINE across the
                                                // band: this row's buttons
                                                // sit at exactly the deck
                                                // transports' y (margin
                                                // solved from the measured
                                                // rects, verified by snap).
                                                View{
                                                    width: 142 height: Fit flow: Right
                                                    margin: Inset{top: -4.0}
                                                    align: Align{x: 0.0, y: 0.5}
                                                    fx_slot_t_on := ChromeButton{width: 30 text: "ON"}
                                                    View{width: Fill height: 1}
                                                    fx_slot_t_clear := ChromeButton{width: 22 text: "×"}
                                                }
                                            }
                                            // Tile over dials: the effect's face on
                                            // top, its 2x2 dial grid below — the
                                            // width this frees belongs to the deck
                                            // source monitors' transport rows.
                                            RoundedView{
                                                // THE BAND LAW: every card in
                                                // the center strip is the SAME
                                                // fixed height — nothing in
                                                // this row ever moves.
                                                width: Fit height: 200 flow: Down spacing: 4
                                                padding: 6
                                                draw_bg +: {
                                                    color: #x181c23
                                                    border_color: #xffffff12
                                                    border_size: 1.0
                                                    border_radius: 9.0
                                                }
                                                align: Align{x: 0.0, y: 0.0}
                                                // The deck-window corner
                                                // idiom: face on top, dials
                                                // tucked tight, ON bottom-
                                                // left / × bottom-right —
                                                // the card takes no more
                                                // width than its tile.
                                                fx_slot_b_tile := VjFxSlotTile{width: 142 height: 80}
                                                // (FIXED widths inside the
                                                // Fit card: a Fill child
                                                // resolves against the OUTER
                                                // context and inflates the
                                                // card past the tile.)
                                                View{
                                                    width: Fit height: Fit flow: Down spacing: 2
                                                    View{
                                                        width: Fit height: Fit flow: Right spacing: 1
                                                        align: Align{x: 0.0, y: 0.5}
                                                        fx_slot_b_spd_learn := Learn{ fx_slot_b_spd := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        Tick{width: 36 margin: Inset{left: -3.0} text: "SPD"}
                                                        fx_slot_b_d0_learn := Learn{ fx_slot_b_d0 := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        fx_slot_b_d0_lab := Tick{width: 36 margin: Inset{left: -3.0} flow: Flow.Right{wrap: false} max_lines: 1 text: "—"}
                                                    }
                                                    View{
                                                        width: Fit height: Fit flow: Right spacing: 1
                                                        align: Align{x: 0.0, y: 0.5}
                                                        fx_slot_b_d1_learn := Learn{ fx_slot_b_d1 := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        fx_slot_b_d1_lab := Tick{width: 36 margin: Inset{left: -3.0} flow: Flow.Right{wrap: false} max_lines: 1 text: "—"}
                                                        fx_slot_b_d2_learn := Learn{ fx_slot_b_d2 := ApcKnob{width: 26 height: 26 default: 0.5} }
                                                        fx_slot_b_d2_lab := Tick{width: 36 margin: Inset{left: -3.0} flow: Flow.Right{wrap: false} max_lines: 1 text: "—"}
                                                    }
                                                }
                                                View{width: 142 height: Fill}
                                                // ONE BASELINE across the
                                                // band: this row's buttons
                                                // sit at exactly the deck
                                                // transports' y (margin
                                                // solved from the measured
                                                // rects, verified by snap).
                                                View{
                                                    width: 142 height: Fit flow: Right
                                                    margin: Inset{top: -4.0}
                                                    align: Align{x: 0.0, y: 0.5}
                                                    fx_slot_b_on := ChromeButton{width: 30 text: "ON"}
                                                    View{width: Fill height: 1}
                                                    fx_slot_b_clear := ChromeButton{width: 22 text: "×"}
                                                }
                                            }
                                            // DECK B SOURCE, mirroring A.
                                            // Card: one tidy module per
                                            // cluster (the app's well idiom).
                                            RoundedView{
                                                // THE BAND LAW: every card in
                                                // the center strip is the SAME
                                                // fixed height — nothing in
                                                // this row ever moves.
                                                width: Fit height: 200 flow: Down spacing: 4
                                                padding: 6
                                                draw_bg +: {
                                                    color: #x181c23
                                                    border_color: #xffffff12
                                                    border_size: 1.0
                                                    border_radius: 9.0
                                                }
                                                // A NORMAL VIDEO PLAYER: 16:9
                                                // black canvas (clips letterbox
                                                // inside whatever their aspect),
                                                // one long scrub bar, controls
                                                // tidily on one line below.
                                                deck_b_source := VideoView{
                                                    width: 252
                                                    height: 156
                                                    bar_below: true
                                                    show_controls: false
                                                    trim_handles: true
                                                    lane +: {
                                                        show_bg: true
                                                        draw_bg +: { color: #x000000 }
                                                    }
                                                }
                                                deck_b_controls := View{
                                                    width: Fill height: 26 flow: Right spacing: 3
                                                    align: Align{x: 0.0, y: 0.5}
                                                    deck_b_play := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/play.svg") } }
                                                    deck_b_rw := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/rewind.svg") } }
                                                    deck_b_loop2 := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/loop.svg") } }
                                                    deck_b_bounce := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/bounce.svg") } }
                                                    deck_b_rate := ChromeButton{width: 30 text: "1"}
                                                    deck_b_mute := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/mute.svg") } }
                                                    // SCRATCH / SHUTTLE:
                                                    // centre neutral, right
                                                    // forward, left reverse,
                                                    // sprung home on release
                                                    // — a jog for testing
                                                    // the frame tweening.
                                                    Tip{ text: "Scratch / shuttle"
                                                        deck_b_scratch := VjShuttle{width: 72}
                                                    }
                                                    Tip{ text: "Auto-spin 3D content"
                                                        slot_b_spin := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/spin.svg") } }
                                                    }
                                                    View{width: Fill height: 1}
                                                    // UNSLOT: eject the clip,
                                                    // deck back to its empty
                                                    // cue state (the effect
                                                    // slots' × idiom).
                                                    deck_b_eject := ChromeButton{width: 22 text: "×"}
                                                }
                                                slot_b_anim_box := View{
                                                    width: Fill
                                                    height: Fit
                                                    visible: false
                                                    slot_b_anim := DropDown{width: Fill labels: ["—"]}
                                                }
                                            }
                                            View{width: Fill height: 1}
                                        }
                                        // The crossfader gets its own row and
                                        // sits in the MIDDLE of it: equal Fill
                                        // spacers either side, so its centre
                                        // detent is the centre of the strip and
                                        // its travel is symmetric about it.
                                        // Sharing a row with the FX knobs pinned
                                        // it to the left, which is exactly where
                                        // a crossfader must not be.
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 8
                                            align: Align{x: 0.5, y: 0.5}
                                            // EQUAL side plates + Fill gaps:
                                            // whatever either side holds, the
                                            // fader's centre stays the console
                                            // centre. NOTHING else lives on
                                            // this line any more — output/
                                            // fadeout/karaoke moved to the
                                            // top bar's rig group. AUTOWIPE +
                                            // its speed dial flank the left
                                            // as SWAP (+ balance) flanks the
                                            // right.
                                            // ONE fixed-width cluster, dead-
                                            // centered and alone on the line:
                                            // [autowipe + speed] [fader]
                                            // [swap + autofade]. Every child
                                            // has a FIXED width, so nothing
                                            // can ever push it or make it
                                            // reflow.
                                            View{
                                                width: Fit height: Fit
                                                flow: Right spacing: 8
                                                align: Align{x: 0.0, y: 0.5}
                                                // Roster (user law): SPEED,
                                                // AUTOWIPE, MIXER, SWAP,
                                                // AUTOFADE.
                                                Tip{ text: "Autofade / take speed"
                                                    video_fade_learn := Learn{
                                                        video_fade := ApcKnob{width: 32 height: 32 min: 0.05 max: 5.0 default: 1.0}
                                                    }
                                                }
                                                Tip{ text: "Auto-run the transition"
                                                    autowipe := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/autowipe.svg") } }
                                                }
                                                Tick{width: 14 text: "A"}
                                                xfader_learn := Learn{
                                                    apc_xfader := ApcXfader{width: 360}
                                                }
                                                Tick{width: 14 text: "B"}
                                                // SWAP ⇄: the decks trade
                                                // their complete personalities
                                                // under a STATIONARY fader —
                                                // a cut move.
                                                Tip{ text: "Swap decks (clips + effects + settings)"
                                                    deck_swap := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/swap.svg") } }
                                                }
                                                // AUTOFADE latch rides with
                                                // the fader it automates.
                                                Tip{ text: "Autofade: cue clicks sweep the fader"
                                                    autofade := IconButton{ draw_icon +: { svg: crate_resource("self:resources/icons/xfade.svg") } }
                                                }
                                            }
                                        }
                                        // ---- the LOWER region: content grid OR the lights desk,
                                        // toggled by the VJ/LIGHTS tab pair up in the console
                                        // strip — the lights rows no longer stack below the
                                        // grid, so the page stops scrolling.
                                        lower_pages := PageFlip{
                                                width: Fill
                                                height: Fill
                                                active_page: @grid_lower_page
                                                grid_lower_page := View{
                                                    width: Fill
                                                    height: Fill
                                                    flow: Down
                                                    spacing: 4
                                        // ---- clips: a VERTICAL filter column
                                        // on the grid's LEFT — search, the
                                        // VJ-relevant lane chips (one radio
                                        // group; the selected chip clicked again
                                        // returns to ALL), paging, and IMPORT
                                        // where content enters the library. The
                                        // exotic authoring kinds live behind
                                        // the search box, not a chip wall.
                                        View{
                                            width: Fill
                                            height: Fill
                                            flow: Right
                                            spacing: 14
                                            View{
                                                width: 104
                                                height: Fill
                                                flow: Down
                                                spacing: 4
                                                pad_filter := TextInput{
                                                    width: Fill
                                                    empty_text: "filter"
                                                }
                                                preset_transition := PillButton{width: Fill text: "TRANSITION"}
                                                preset_effect := PillButton{width: Fill text: "EFFECT"}
                                                preset_video := PillButton{width: Fill text: "VIDEO"}
                                                chip_image := PillButton{width: Fill text: "IMAGE"}
                                                chip_mesh := PillButton{width: Fill text: "MESH"}
                                                chip_map := PillButton{width: Fill text: "MAP"}
                                                import_toggle := ChromeButton{width: Fill text: "IMPORT"}
                                                // (No count readout: the
                                                // scrollbar + pager already
                                                // say where you are.)
                                                View{width: Fill height: Fill}
                                                // Bank paging at the column's
                                                // FOOT — one line with the
                                                // grid's scrollbar beside it.
                                                View{
                                                    width: Fill height: Fit flow: Right spacing: 2
                                                    grid_prev_page := IconButton{width: 24 icon_walk: Walk{width: 9 height: Fit} draw_icon +: { svg: crate_resource("self:resources/icons/page_first.svg") }}
                                                    grid_prev_row := IconButton{width: 24 icon_walk: Walk{width: 9 height: Fit} draw_icon +: { svg: crate_resource("self:resources/icons/page_prev.svg") }}
                                                    grid_next_row := IconButton{width: 24 icon_walk: Walk{width: 9 height: Fit} draw_icon +: { svg: crate_resource("self:resources/icons/page_next.svg") }}
                                                    grid_next_page := IconButton{width: 24 icon_walk: Walk{width: 9 height: Fit} draw_icon +: { svg: crate_resource("self:resources/icons/page_last.svg") }}
                                                }
                                            }
                                            video_grid := VjPadMatrix{}
                                        }
                                                }
                                                lights_lower_page := ScrollYView{
                                                    width: Fill
                                                    height: Fill
                                                    flow: Down
                                                    spacing: 4
                                                    scroll_bars.scroll_bar_y.drag_scrolling: false
                                        // ---- lighting desk (APC40 knobs / faders / scenes) ----
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 8
                                            align: Align{x: 0.0, y: 0.5}
                                            Tick{width: Fit text: "LIGHTS"}
                                            show_status_label := Label{
                                                width: Fill
                                                text: "show control starting…"
                                                draw_text.color: #x8e9aa7
                                                draw_text.text_style.font_size: 9
                                            }
                                            light_desk_status := Label{
                                                width: Fit
                                                text: ""
                                                draw_text.color: #x8e9aa7
                                                draw_text.text_style.font_size: 8
                                            }
                                            light_power := Toggle{text: "pwr"}
                                            light_write := Toggle{text: "wrt"}
                                            light_blackout := Button{text: "BLK"}
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            align: Align{x: 0.0, y: 0.0}
                                            KnobCol{ Tick{text: "—"} light_knob_0 := ApcKnob{} }
                                            KnobCol{ Tick{text: "WASH"} light_knob_1 := ApcKnob{} }
                                            KnobCol{ Tick{text: "COL"} light_knob_2 := ApcKnob{} }
                                            KnobCol{ Tick{text: "HUE"} light_knob_3 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_knob_4 := ApcKnob{} }
                                            KnobCol{ Tick{text: "BEAM"} light_knob_5 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_knob_6 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_knob_7 := ApcKnob{} }
                                            View{width: 14 height: 1}
                                            KnobCol{ dev_knob_legend := Tick{text: "SMK"} light_dev_0 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_dev_1 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_dev_2 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_dev_3 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_dev_4 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_dev_5 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_dev_6 := ApcKnob{} }
                                            KnobCol{ Tick{text: "—"} light_dev_7 := ApcKnob{} }
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            align: Align{x: 0.0, y: 1.0}
                                            FaderCol{ Tick{text: "—"} light_fader_0 := ApcFader{} }
                                            FaderCol{ Tick{text: "WASH"} light_fader_1 := ApcFader{} }
                                            FaderCol{ Tick{text: "GOBO"} light_fader_2 := ApcFader{} }
                                            FaderCol{ Tick{text: "RGB"} light_fader_3 := ApcFader{} }
                                            FaderCol{ Tick{text: "STRB"} light_fader_4 := ApcFader{} }
                                            FaderCol{ Tick{text: "BEAM"} light_fader_5 := ApcFader{} }
                                            FaderCol{ Tick{text: "UV"} light_fader_6 := ApcFader{} }
                                            FaderCol{ Tick{text: "UV+"} light_fader_7 := ApcFader{} }
                                            FaderCol{ Tick{text: "M"} light_fader_8 := ApcFader{} }
                                            View{
                                                width: Fill
                                                height: Fit
                                                flow: Down
                                                spacing: 4
                                                Tick{width: Fit text: "TRACKS"}
                                                View{
                                                    width: Fill height: Fit flow: Right spacing: 4
                                                    light_track_0 := ApcPad{text: "1"}
                                                    light_track_1 := ApcPad{text: "2"}
                                                    light_track_2 := ApcPad{text: "3"}
                                                    light_track_3 := ApcPad{text: "4"}
                                                    light_track_4 := ApcPad{text: "5"}
                                                    light_track_5 := ApcPad{text: "6"}
                                                    light_track_6 := ApcPad{text: "7"}
                                                    light_track_7 := ApcPad{text: "8"}
                                                }
                                                Tick{width: Fit text: "SCENES"}
                                                View{
                                                    width: Fill height: Fit flow: Right spacing: 4
                                                    light_scene_0 := ApcPad{text: "P1"}
                                                    light_scene_1 := ApcPad{text: "P2"}
                                                    light_scene_2 := ApcPad{text: "P3"}
                                                    light_scene_3 := ApcPad{text: "P4"}
                                                    light_scene_4 := ApcPad{text: "P5"}
                                                    light_scene_5 := ApcPad{text: "P6"}
                                                    light_scene_6 := ApcPad{text: "P7"}
                                                }
                                                View{
                                                    width: Fill height: Fit flow: Right spacing: 4
                                                    light_scene_7 := ApcPad{text: "P8"}
                                                    light_scene_8 := ApcPad{text: "P9"}
                                                    light_scene_9 := ApcPad{text: "P10"}
                                                    light_scene_10 := ApcPad{text: "P11"}
                                                    light_scene_11 := ApcPad{text: "P12"}
                                                    light_scene_12 := ApcPad{text: "P13"}
                                                    View{width: Fill height: 1}
                                                }
                                            }
                                        }
                                                }
                                            }
                                        }
                                    }
                                }
                            // ============ MUSIC ============
                            music_page := View{
                                width: Fill
                                height: Fill
                                flow: Down
                                music_surface := MusicDeckPage{}
                            }

                            // ============ SFX ============
                            sfx_page := View{
                                width: Fill
                                height: Fill
                                flow: Down
                                spacing: 8
                                SearchRow{
                                    sfx_search := TextInput{
                                        width: Fill
                                        empty_text: "search sfx…"
                                    }
                                    sfx_category := TextInput{
                                        width: 120
                                        text: "sfx"
                                    }
                                    sfx_go := ChromeButton{text: "Search"}
                                    sfx_more := ChromeButton{text: "More"}
                                    sfx_count := PanelLabel{text: ""}
                                    sfx_voices := PanelLabel{text: "voices 0"}
                                }
                                sfx_grid := VjTileGrid{}
                                // Selected-pad strip: pads themselves stay
                                // pure triggers (no per-pad transport).
                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 8
                                    align: Align{x: 0.0, y: 0.5}
                                    sfx_sel := ValueLabel{text: "pad: —"}
                                    sfx_gain := Slider{
                                        width: 170
                                        text: "gain"
                                        min: 0.0
                                        max: 1.5
                                        default: 1.0
                                    }
                                    PanelLabel{text: "choke"}
                                    sfx_choke := DropDown{labels: ["off" "1" "2" "3" "4"]}
                                    sfx_hold := Toggle{text: "hold"}
                                    sfx_loop := Toggle{text: "loop"}
                                    sfx_stop := ChromeButton{text: "stop pad"}
                                    sfx_stop_all := ChromeButton{text: "stop all"}
                                }
                            }

                            // ============ MESH ============
                            mesh_page := View{
                                width: Fill
                                height: Fill
                                flow: Down
                                spacing: 8
                                SearchRow{
                                    mesh_search := TextInput{
                                        width: Fill
                                        empty_text: "search dancers (mesh + character)…"
                                    }
                                    mesh_category := TextInput{
                                        width: 120
                                        empty_text: "category"
                                    }
                                    mesh_go := ChromeButton{text: "Search"}
                                    mesh_more := ChromeButton{text: "More"}
                                    mesh_count := PanelLabel{text: ""}
                                }
                                mesh_grid := VjTileGrid{}
                                mesh_status := PanelLabel{text: "click a mesh to send it to the output window"}
                            }
                            }
                            }
                            a: RoundedView{
                                width: Fill
                                height: Fill
                                flow: Down
                                spacing: 8
                                padding: 8
                                draw_bg +: {
                                    color: #x1c2129
                                    border_color: #xffffff26
                                    border_size: 1.0
                                    border_radius: 10.0
                                }
                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 6
                                    align: Align{x: 0.0, y: 0.5}
                                    Label{
                                        text: "GEN"
                                        draw_text.color: #xff5c39
                                        draw_text.text_style: theme.font_bold{font_size: 11}
                                    }
                                    View{width: Fill height: 1}
                                    gen_clear := ChromeButton{text: "Clear"}
                                    gen_fold := ChromeButton{text: "⟨"}
                                }
                                gen_prompt := TextInput{
                                    width: Fill
                                    empty_text: "prompt"
                                }
                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 6
                                    align: Align{x: 0.0, y: 0.5}
                                    gen_profile := DropDown{labels: ["…"]}
                                    gen_len := DropDown{labels: ["…"]}
                                    gen_go := ChromeButton{text: "Queue"}
                                }
                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 6
                                    align: Align{x: 0.0, y: 0.5}
                                    gen_blast := ChromeButton{text: "BLAST"}
                                    // Keep the queue topped up from the same
                                    // prompt for as long as it is checked.
                                    gen_loop := CheckBox{text: "CONT"}
                                }
                                gen_status := PanelLabel{text: ""}
                                gen_jobs := VjJobList{}
                                // Say it in words instead: the same broker
                                // chat the asset UI runs (session on the
                                // server, tool chips, rate meter), opened
                                // with the VJ profile. It shares the drawer
                                // with the queue above.
                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 6
                                    align: Align{x: 0.0, y: 0.5}
                                    Label{
                                        text: "CHAT"
                                        draw_text.color: #xff5c39
                                        draw_text.text_style: theme.font_bold{font_size: 11}
                                    }
                                    View{width: Fill height: 1}
                                    chat_cancel_btn := ChromeButton{text: "Stop" visible: false}
                                    chat_clear_btn := ChromeButton{text: "Clear"}
                                }
                                chat_status := PanelLabel{
                                    width: Fill
                                    text: "Waiting for the asset server…"
                                }
                                chat_list := AssetChatList{}
                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 6
                                    align: Align{y: 1.0}
                                    chat_input := TextInput{
                                        width: Fill
                                        empty_text: "find me 20s of dark techno…"
                                    }
                                    chat_send_btn := ChromeButton{text: "Send"}
                                }
                            }
                        }
                        // IMPORT CONTENT. Folded away until asked for, because
                        // importing is a thing you do between sets, not during
                        // one — but the handle is always on screen so it is
                        // discoverable without a manual.
                        // Live show control stays visible on every surface. The
                        // hazardous groups are deliberately separate from the
                        // video-reactive controls and require both an arm toggle
                        // and the momentary deadman below.
                        show_panel := RoundedView{
                            visible: false
                            width: Fill
                            height: Fit
                            flow: Down
                            spacing: 4
                            padding: Inset{left: 10.0 right: 10.0 top: 4.0 bottom: 4.0}
                            draw_bg +: {
                                color: #x1c2129
                                border_color: #xffffff22
                                border_size: 1.0
                                border_radius: 8.0
                            }
                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 6
                                align: Align{x: 0.0 y: 0.5}
                                Label{
                                    text: "LIGHT"
                                    draw_text.color: #xff5c39
                                    draw_text.text_style: theme.font_bold{font_size: 10}
                                }
                                PanelLabel{text: "auto spatial"}
                                light_master := Slider{
                                    width: 150
                                    min: 0.0
                                    max: 0.65
                                    default: 0.26
                                }
                                lighting_values := PanelLabel{
                                    width: Fill
                                    text: "spatial colour · safe output"
                                }
                                light_advanced := Toggle{text: "advanced"}
                                light_reset := Button{text: "RESET"}
                                light_restore := Button{text: "RESTORE"}
                            }
                            light_advanced_row := View{
                                visible: false
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 6
                                align: Align{x: 0.0 y: 0.5}
                                Label{
                                    text: "TUNING"
                                    draw_text.color: #x58c4a0
                                    draw_text.text_style: theme.font_bold{font_size: 10}
                                }
                                PanelLabel{text: "black floor"}
                                light_black_floor := Slider{
                                    width: 82
                                    min: 0.0
                                    max: 0.18
                                    default: 0.02
                                }
                                PanelLabel{text: "colour"}
                                light_colorfulness := Slider{
                                    width: 82
                                    min: 0.5
                                    max: 1.5
                                    default: 1.05
                                }
                                PanelLabel{text: "response"}
                                light_response := Slider{
                                    width: 82
                                    min: 0.02
                                    max: 0.75
                                    default: 0.28
                                }
                                PanelLabel{text: "movers"}
                                show_movers := Slider{
                                    width: 70
                                    min: 0.0
                                    max: 0.6
                                    default: 0.0
                                }
                                PanelLabel{text: "RGB"}
                                show_rgb := Slider{
                                    width: 70
                                    min: 0.0
                                    max: 0.5
                                    default: 0.0
                                }
                                PanelLabel{text: "strobe"}
                                show_strobe := Slider{
                                    width: 70
                                    min: 0.0
                                    max: 0.25
                                    default: 0.0
                                }
                            }
                            light_hazard_row := View{
                                visible: false
                                width: Fill
                                height: Fit
                                flow: Right
                                spacing: 6
                                align: Align{x: 0.0 y: 0.5}
                                PanelLabel{text: "HAZARDS"}
                                laser_arm := Toggle{text: "laser arm"}
                                laser_level := Slider{
                                    width: 70
                                    min: 0.0
                                    max: 1.0
                                    default: 0.0
                                }
                                smoke_arm := Toggle{text: "smoke arm"}
                                smoke_level := Slider{
                                    width: 70
                                    min: 0.0
                                    max: 1.0
                                    default: 0.0
                                }
                                uv_arm := Toggle{text: "UV arm"}
                                uv_level := Slider{
                                    width: 70
                                    min: 0.0
                                    max: 1.0
                                    default: 0.0
                                }
                                hazard_deadman := Button{text: "HOLD HAZARDS"}
                                hazard_status := PanelLabel{
                                    width: Fill
                                    text: "disarmed"
                                }
                            }
                        }

                    }
                    // F3: frame-time overlay (Cx perf monitor).
                    perf_box := View{
                        visible: false
                        width: Fill
                        height: Fill
                        perf_graph := PerfGraph{}
                    }
                    // The system tooltip host: LAST in the overlay stack,
                    // draws on the overlay layer over every panel.
                    tip_layer := TipLayer{}
                    }
                }
            }

            output_window := Window{
                window.title: "VJ Output"
                window.inner_size: vec2(1280, 720)
                window.position: vec2(720, 220)
                body +: {
                    SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        draw_bg.color: #x000000
                        out_pages := PageFlip{
                            width: Fill
                            height: Fill
                            active_page: @video_out_page
                            video_out_page := View{
                                width: Fill
                                height: Fill
                                program := VideoProgram{}
                            }
                            mesh_out_page := View{
                                width: Fill
                                height: Fill
                                mesh_program := VjMeshView{}
                            }
                        }
                    }
                }
            }
        }
    }
}


/// Every widget on the deck surface, resolved ONCE.
///
/// The per-frame sync used to re-find each of these by id path, which walks
/// and hashes the whole widget tree a hundred times a frame — by far the
/// biggest cost in the DJ view. Resolved refs make the per-frame path a
/// handful of pointer derefs.
#[derive(Default)]
struct DeckRefs {
    title: LabelRef,
    artist: LabelRef,
    bpm: LabelRef,
    pitch_text: LabelRef,
    time: LabelRef,
    grid_state: LabelRef,
    stem_state: LabelRef,
    loop_len: LabelRef,
    range: ButtonRef,
    play: ButtonRef,
    cue: ButtonRef,
    loop_button: ButtonRef,
    loop_halve: ButtonRef,
    loop_double: ButtonRef,
    mute: ButtonRef,
    sync: ButtonRef,
    keylock: ButtonRef,
    pitch_reset: ButtonRef,
    pitch: SliderRef,
    gain: SliderRef,
    filter: SliderRef,
    vu: ViewRef,
    eq_knobs: Vec<SliderRef>,
    eq_kills: Vec<ButtonRef>,
    stem_knobs: Vec<SliderRef>,
    stem_kills: Vec<ButtonRef>,
    stem_labels: Vec<LabelRef>,
    /// The transcript panel filling the bottom of the deck column.
    lyrics: WidgetRef,
}

impl DeckRefs {
    fn resolve(ui: &WidgetRef, cx: &mut Cx, deck: DeckId) -> DeckRefs {
        let ids = MusicDeckIds::for_deck(deck);
        DeckRefs {
            title: ui.label(cx, ids.title),
            artist: ui.label(cx, ids.artist),
            bpm: ui.label(cx, ids.bpm),
            pitch_text: ui.label(cx, ids.pitch_text),
            time: ui.label(cx, ids.time),
            grid_state: ui.label(cx, ids.grid_state),
            stem_state: ui.label(cx, ids.stem_state),
            loop_len: ui.label(cx, ids.loop_len),
            range: ui.button(cx, ids.range),
            play: ui.button(cx, ids.play),
            cue: ui.button(cx, ids.cue),
            loop_button: ui.button(cx, ids.loop_button),
            loop_halve: ui.button(cx, ids.loop_halve),
            loop_double: ui.button(cx, ids.loop_double),
            mute: ui.button(cx, ids.mute),
            sync: ui.button(cx, ids.sync),
            keylock: ui.button(cx, ids.keylock),
            pitch_reset: ui.button(cx, ids.pitch_reset),
            pitch: ui.slider(cx, ids.pitch),
            gain: ui.slider(cx, ids.gain),
            filter: ui.slider(cx, ids.filter),
            vu: ui.view(cx, ids.vu),
            eq_knobs: ids.eq_knobs.iter().map(|p| ui.slider(cx, p)).collect(),
            eq_kills: ids.eq_kills.iter().map(|p| ui.button(cx, p)).collect(),
            stem_knobs: ids.stem_knobs.iter().map(|p| ui.slider(cx, p)).collect(),
            stem_kills: ids.stem_kills.iter().map(|p| ui.button(cx, p)).collect(),
            stem_labels: ids.stem_labels.iter().map(|p| ui.label(cx, p)).collect(),
            lyrics: ui.widget(cx, ids.lyrics),
        }
    }
}

#[derive(Default)]
struct MusicRefs {
    decks: [DeckRefs; 2],
    waves: WidgetRef,
    overviews: [WidgetRef; 2],
    tracks: WidgetRef,
    queue: WidgetRef,
    auto_sync: ButtonRef,
    music_local: ButtonRef,
    queue_clear: ButtonRef,
    queue_count: LabelRef,
    xfader: SliderRef,
}

impl MusicRefs {
    fn resolve(ui: &WidgetRef, cx: &mut Cx) -> MusicRefs {
        MusicRefs {
            decks: [
                DeckRefs::resolve(ui, cx, DeckId::A),
                DeckRefs::resolve(ui, cx, DeckId::B),
            ],
            waves: ui.widget(cx, ids!(music_waves)),
            overviews: [
                ui.widget(cx, ids!(deck_a_overview)),
                ui.widget(cx, ids!(deck_b_overview)),
            ],
            tracks: ui.widget(cx, ids!(music_tracks)),
            queue: ui.widget(cx, ids!(music_queue)),
            auto_sync: ui.button(cx, ids!(auto_sync)),
            music_local: ui.button(cx, ids!(music_local)),
            queue_clear: ui.button(cx, ids!(queue_clear)),
            queue_count: ui.label(cx, ids!(queue_count)),
            xfader: ui.slider(cx, ids!(xfader)),
        }
    }

    /// The surface exists once the wave view resolves.
    fn is_live(&self) -> bool {
        self.waves.borrow::<VjWaveScroll>().is_some()
    }
}

/// Every widget id one deck strip owns, so the surface refresh and the
/// action handling both walk the same table instead of two long tuples.
struct MusicDeckIds {
    title: &'static [LiveId],
    artist: &'static [LiveId],
    bpm: &'static [LiveId],
    pitch_text: &'static [LiveId],
    time: &'static [LiveId],
    grid_state: &'static [LiveId],
    stem_state: &'static [LiveId],
    range: &'static [LiveId],
    loop_len: &'static [LiveId],
    play: &'static [LiveId],
    cue: &'static [LiveId],
    loop_button: &'static [LiveId],
    loop_halve: &'static [LiveId],
    loop_double: &'static [LiveId],
    mute: &'static [LiveId],
    sync: &'static [LiveId],
    keylock: &'static [LiveId],
    pitch: &'static [LiveId],
    pitch_reset: &'static [LiveId],
    gain: &'static [LiveId],
    vu: &'static [LiveId],
    filter: &'static [LiveId],
    /// Low, mid, high — engine band order.
    eq_knobs: [&'static [LiveId]; 3],
    eq_kills: [&'static [LiveId]; 3],
    /// Vocals, drums, bass, other — engine stem order.
    stem_knobs: [&'static [LiveId]; 4],
    stem_kills: [&'static [LiveId]; 4],
    lyrics: &'static [LiveId],
    /// The legends over those knobs, tinted to match the waveform.
    stem_labels: [&'static [LiveId]; 4],
}

impl MusicDeckIds {
    fn for_deck(deck: DeckId) -> MusicDeckIds {
        match deck {
            DeckId::A => MusicDeckIds {
                title: ids!(deck_a_title),
                artist: ids!(deck_a_artist),
                bpm: ids!(deck_a_bpm),
                pitch_text: ids!(deck_a_pitch_text),
                time: ids!(deck_a_time),
                grid_state: ids!(deck_a_grid_state),
                stem_state: ids!(deck_a_stem_state),
                range: ids!(deck_a_range),
                loop_len: ids!(deck_a_loop_len),
                play: ids!(deck_a_play),
                cue: ids!(deck_a_cue),
                loop_button: ids!(deck_a_loop),
                loop_halve: ids!(deck_a_loop_halve),
                loop_double: ids!(deck_a_loop_double),
                mute: ids!(deck_a_mute),
                sync: ids!(deck_a_sync),
                keylock: ids!(deck_a_keylock),
                pitch: ids!(deck_a_pitch),
                pitch_reset: ids!(deck_a_pitch_reset),
                gain: ids!(deck_a_gain),
                vu: ids!(deck_a_vu),
                filter: ids!(deck_a_filter),
                eq_knobs: [
                    ids!(deck_a_eq_low),
                    ids!(deck_a_eq_mid),
                    ids!(deck_a_eq_high),
                ],
                eq_kills: [
                    ids!(deck_a_kill_low),
                    ids!(deck_a_kill_mid),
                    ids!(deck_a_kill_high),
                ],
                stem_knobs: [
                    ids!(deck_a_stem_vocals),
                    ids!(deck_a_stem_drums),
                    ids!(deck_a_stem_bass),
                    ids!(deck_a_stem_other),
                ],
                stem_kills: [
                    ids!(deck_a_kill_vocals),
                    ids!(deck_a_kill_drums),
                    ids!(deck_a_kill_bass),
                    ids!(deck_a_kill_other),
                ],
                stem_labels: [
                    ids!(deck_a_label_vocals),
                    ids!(deck_a_label_drums),
                    ids!(deck_a_label_bass),
                    ids!(deck_a_label_other),
                ],
                lyrics: ids!(deck_a_lyrics),
            },
            DeckId::B => MusicDeckIds {
                title: ids!(deck_b_title),
                artist: ids!(deck_b_artist),
                bpm: ids!(deck_b_bpm),
                pitch_text: ids!(deck_b_pitch_text),
                time: ids!(deck_b_time),
                grid_state: ids!(deck_b_grid_state),
                stem_state: ids!(deck_b_stem_state),
                range: ids!(deck_b_range),
                loop_len: ids!(deck_b_loop_len),
                play: ids!(deck_b_play),
                cue: ids!(deck_b_cue),
                loop_button: ids!(deck_b_loop),
                loop_halve: ids!(deck_b_loop_halve),
                loop_double: ids!(deck_b_loop_double),
                mute: ids!(deck_b_mute),
                sync: ids!(deck_b_sync),
                keylock: ids!(deck_b_keylock),
                pitch: ids!(deck_b_pitch),
                pitch_reset: ids!(deck_b_pitch_reset),
                gain: ids!(deck_b_gain),
                vu: ids!(deck_b_vu),
                filter: ids!(deck_b_filter),
                eq_knobs: [
                    ids!(deck_b_eq_low),
                    ids!(deck_b_eq_mid),
                    ids!(deck_b_eq_high),
                ],
                eq_kills: [
                    ids!(deck_b_kill_low),
                    ids!(deck_b_kill_mid),
                    ids!(deck_b_kill_high),
                ],
                stem_knobs: [
                    ids!(deck_b_stem_vocals),
                    ids!(deck_b_stem_drums),
                    ids!(deck_b_stem_bass),
                    ids!(deck_b_stem_other),
                ],
                stem_kills: [
                    ids!(deck_b_kill_vocals),
                    ids!(deck_b_kill_drums),
                    ids!(deck_b_kill_bass),
                    ids!(deck_b_kill_other),
                ],
                stem_labels: [
                    ids!(deck_b_label_vocals),
                    ids!(deck_b_label_drums),
                    ids!(deck_b_label_bass),
                    ids!(deck_b_label_other),
                ],
                lyrics: ids!(deck_b_lyrics),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// request routing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    Video,
    Music,
    Sfx,
    Mesh,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SlotMedia {
    #[default]
    Empty,
    Video,
    Still,
    Mesh,
    /// Gaussian splat scene in the slot's offscreen XrSceneView.
    Splat,
    Billboard,
    /// A vjeffect running AS CONTENT in the slot's offscreen VjFxSlotHost
    /// (fx_slot.rs) — an FX tile clicked with no effect slot armed cues
    /// like any clip, autofade included.
    Effect,
}

/// Slow orbit for a parked splat scene (radians per second).
const SPLAT_ORBIT_RATE: f32 = 0.22;

struct BillboardSlot {
    states: Vec<crate::billboard::PreparedState>,
    textures: Vec<Vec<Texture>>,
    state_i: usize,
    frame_i: usize,
    accum: f64,
    last: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OutputWindowLifecycle {
    #[default]
    Open,
    Closed,
    Opening,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputWindowCommand {
    Recreate,
    Restore,
    Deminiaturize,
}

fn output_window_command(
    lifecycle: OutputWindowLifecycle,
    is_macos: bool,
) -> Option<OutputWindowCommand> {
    match lifecycle {
        OutputWindowLifecycle::Closed => Some(OutputWindowCommand::Recreate),
        OutputWindowLifecycle::Opening => None,
        OutputWindowLifecycle::Open if is_macos => Some(OutputWindowCommand::Deminiaturize),
        OutputWindowLifecycle::Open => Some(OutputWindowCommand::Restore),
    }
}

const SURFACES: [Surface; 4] = [Surface::Video, Surface::Music, Surface::Sfx, Surface::Mesh];

/// Everything a cue strip's appearance depends on (see sync_slot_controls_ui).
#[derive(Clone, Debug, PartialEq)]
struct StripShape {
    /// Anything cued at all (empty deck = no transport row).
    present: bool,
    video: bool,
    spin: bool,
    tracks: Vec<String>,
    selected: Option<usize>,
    playing: bool,
    looping: bool,
    spinning: bool,
    /// The ♪ chip's two facts: synced at all, and at what rate.
    beat_sync: bool,
    beat_rate: f32,
}

/// One clip's sticky playback profile (see `save_clip_profile`).
#[derive(Clone, Copy)]
struct ClipProfile {
    loop_on: bool,
    pingpong: bool,
    trim: (f64, f64),
    rate: f32,
    muted: bool,
    sync: bool,
}

impl Default for ClipProfile {
    fn default() -> Self {
        Self {
            loop_on: true,
            pingpong: false,
            trim: (0.0, 1.0),
            // Loops beat-sync at 4 beats a sweep by default — a bar of
            // motion at 4/4, the natural read for a typical clip at 120.
            rate: 4.0,
            muted: true,
            sync: true,
        }
    }
}

/// Hands-free crossfade at the deck's set fade time.
///
/// The operator arms a clip on the far side and presses AUTOFADE instead of
/// riding the fader — the same move a T-bar's auto-take button makes. Press
/// again mid-fade and it turns round; touch the fader and it lets go at
/// once, because the fader IS the operator's hand and must always win.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct AutoFade {
    /// Where the fade is heading (0 = A, 1 = B).
    target: f32,
    /// Units of mix per second; 0 while idle.
    rate: f32,
    active: bool,
}

impl AutoFade {
    /// Start, or reverse. `mix` is the fader now, `secs` the fade time.
    /// Returns the target end so the caller can report it.
    fn press(&mut self, mix: f32, secs: f32) -> f32 {
        // Heading for whichever end the fader is NOT at. Mid-fade that is
        // the end it came from, which is what "press again to turn round"
        // has to mean.
        let target = match self.active {
            true => 1.0 - self.target,
            false if mix >= 0.5 => 0.0,
            false => 1.0,
        };
        self.target = target;
        // The knob sets how long a FULL crossfade takes, so the fader moves
        // at a constant speed and a part-way start simply finishes sooner.
        self.rate = 1.0 / secs.max(0.05);
        self.active = true;
        target
    }

    /// One frame. `None` when nothing is running; `Some(mix)` otherwise,
    /// and the fade ends exactly on the target.
    fn tick(&mut self, dt: f32, mix: f32) -> Option<f32> {
        if !self.active {
            return None;
        }
        let step = self.rate * dt;
        let next = match self.target > mix {
            true => (mix + step).min(self.target),
            false => (mix - step).max(self.target),
        };
        if (next - self.target).abs() <= 1e-4 {
            self.active = false;
            return Some(self.target);
        }
        Some(next)
    }

    /// The operator grabbed the fader (or a cue landed): let go silently.
    fn cancel(&mut self) {
        self.active = false;
        self.rate = 0.0;
    }

    fn active(&self) -> bool {
        self.active
    }
}

/// The fade the cue engine still holds slots for, when the mixer has moved
/// on to a different transition (or never took this one at all) and can
/// therefore never report it `Completed`.
///
/// The engine reserves BOTH slots for a running fade — the outgoing one is
/// "still fading", the incoming one is live — so a fade that can never land
/// wedges every later click into `WaitingSlot` and the program freezes.
/// The mixer's phase mailbox is a single slot, so this is the host's only
/// way back: as soon as the published identity is not the engine's, that
/// fade is over as far as the device clock is concerned.
fn stale_fade_to_land(
    active: Option<CueScheduleId>,
    published: mixer::VideoTransitionId,
) -> Option<CueScheduleId> {
    active.filter(|schedule| *schedule != published)
}

/// The three top-level modes. Everything else is a filter or a drawer.
const MODE_BUTTONS: [(&[LiveId], ApcSurface); 3] = [
    (ids!(mode_vj), ApcSurface::Video),
    (ids!(mode_dj), ApcSurface::Music),
    (ids!(mode_sfx), ApcSurface::Sfx),
];

/// The explorer's lane: exactly one chip at a time (radio; the selected
/// chip clicked again returns to ALL). Audio has no chip here — that is
/// the DJ surface's lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GridLane {
    /// (No chip any more — kept for the set_lanes match.)
    All,
    Kind(AssetKind),
    /// vjeffects narrowed to the `transition` tag.
    Transition,
}

impl Default for GridLane {
    /// The natural landing place for a VJ set: VIDEO. One category is
    /// always selected — the unfiltered mixed view is gone.
    fn default() -> Self {
        GridLane::Kind(AssetKind::Video)
    }
}

/// The VJ-relevant content lanes, one chip each, in column order. The
/// exotic authoring kinds (char/prop/material/…) live behind the search
/// box — a VJ set never browses them by chip.
const LANE_CHIPS: [(&[LiveId], GridLane, &str); 6] = [
    // Performance lanes first (user order law), content lanes below;
    // the BOOT default stays VIDEO.
    (ids!(preset_transition), GridLane::Transition, "TRANSITION"),
    (ids!(preset_effect), GridLane::Kind(AssetKind::VjEffect), "EFFECT"),
    (ids!(preset_video), GridLane::Kind(AssetKind::Video), "VIDEO"),
    (ids!(chip_image), GridLane::Kind(AssetKind::Texture), "IMAGE"),
    (ids!(chip_mesh), GridLane::Kind(AssetKind::Mesh), "MESH"),
    // MAP = the walkable worlds (the doom/quake imports) as deck content.
    (ids!(chip_map), GridLane::Kind(AssetKind::World), "MAP"),
];

/// Every MIDI-learnable control: wrapper widget path + stable persisted id.
/// Making another control learnable = wrap it in `Learn{...}` in the DSL
/// and add one row here (plus its value arm in `apply_learned`).
const LEARNABLES: [(&[LiveId], &str); 16] = [
    (ids!(video_fade_learn), "video_fade"),
    (ids!(xfader_learn), "xfader"),
    (ids!(master_learn), "master"),
    (ids!(fx_slot_a_spd_learn), "fx_a_spd"),
    (ids!(fx_slot_a_d0_learn), "fx_a_d0"),
    (ids!(fx_slot_a_d1_learn), "fx_a_d1"),
    (ids!(fx_slot_a_d2_learn), "fx_a_d2"),
    (ids!(fx_slot_t_spd_learn), "fx_t_spd"),
    (ids!(fx_slot_t_d0_learn), "fx_t_d0"),
    (ids!(fx_slot_t_d1_learn), "fx_t_d1"),
    (ids!(fx_slot_t_d2_learn), "fx_t_d2"),
    (ids!(fx_slot_b_spd_learn), "fx_b_spd"),
    (ids!(fx_slot_b_d0_learn), "fx_b_d0"),
    (ids!(fx_slot_b_d1_learn), "fx_b_d1"),
    (ids!(fx_slot_b_d2_learn), "fx_b_d2"),
    (ids!(fadeout_learn), "fadeout"),
];

/// The four-state colour set of a LATCHING toggle (ROTATE, PLAY, LOOP,
/// MUTE, the kind/preset chips).
///
/// A latch has to read lit in EVERY interaction state. Painting only `color`
/// (and `color_focus`) leaves `color_hover` / `color_down` at their theme
/// values — which are the rest colours of an UNLIT button. The result is the
/// bug the operator sees: the toggle applies, but the moment they click it
/// (and their pointer is still on it) it paints unlit, so it "never lights
/// up". Every latch paints all four.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatchPaint {
    /// RRGGBBAA, rest / hover / down background and the foreground.
    pub bg: u32,
    pub bg_hover: u32,
    pub bg_down: u32,
    pub fg: u32,
    pub fg_hover: u32,
}

impl LatchPaint {
    /// Icon buttons and the FX bank: dark chrome at rest, accent when lit.
    pub fn icon(lit: bool) -> LatchPaint {
        if lit {
            LatchPaint {
                // MAKEPAD ORANGE — the brand accent (#FF5C39, the bright
                // orange of the makepad.nl logo) is the app-wide ON color.
                bg: 0xff5c39ff,
                bg_hover: 0xff7a5cff,
                bg_down: 0xd94a2cff,
                fg: 0x1c0b06ff,
                fg_hover: 0x1c0b06ff,
            }
        } else {
            LatchPaint {
                bg: 0x272e38ff,
                bg_hover: 0x2b3440ff,
                bg_down: 0x1e232bff,
                fg: 0xd6dee6ff,
                fg_hover: 0xfffaf4ff,
            }
        }
    }

    /// The RESERVED transport row of an EMPTY deck (the no-push law):
    /// chrome present, faces nearly off — geometry never changes.
    pub fn ghost() -> LatchPaint {
        LatchPaint {
            bg: 0x1a1e2455,
            bg_hover: 0x1a1e2455,
            bg_down: 0x1a1e2455,
            fg: 0x39404a66,
            fg_hover: 0x39404a66,
        }
    }

    /// Kind / preset chips: a softer unlit fill than the icon buttons.
    pub fn chip(lit: bool) -> LatchPaint {
        if lit {
            LatchPaint::icon(true)
        } else {
            LatchPaint {
                bg: 0x222831ff,
                bg_hover: 0x2f3842ff,
                bg_down: 0x1c2129ff,
                fg: 0xb4bfcaff,
                fg_hover: 0xffffffff,
            }
        }
    }

    /// True when this latch reads as LIT in every interaction state — the
    /// invariant the operator's eye depends on.
    pub fn reads_lit(&self) -> bool {
        let unlit = LatchPaint::icon(false);
        let unlit_chip = LatchPaint::chip(false);
        [self.bg, self.bg_hover, self.bg_down].iter().all(|c| {
            ![unlit.bg, unlit.bg_hover, unlit.bg_down, unlit_chip.bg, unlit_chip.bg_hover, unlit_chip.bg_down]
                .contains(c)
        })
    }

    fn bg(&self) -> Vec4f {
        Vec4f::from_u32(self.bg)
    }
    fn bg_hover(&self) -> Vec4f {
        Vec4f::from_u32(self.bg_hover)
    }
    fn bg_down(&self) -> Vec4f {
        Vec4f::from_u32(self.bg_down)
    }
    fn fg(&self) -> Vec4f {
        Vec4f::from_u32(self.fg)
    }
    fn fg_hover(&self) -> Vec4f {
        Vec4f::from_u32(self.fg_hover)
    }
}

/// What a catalog-runtime request was for.
#[derive(Clone, Debug)]
enum CatPurpose {
    Page { surface: Surface, gen: CatGen, slot: usize, first: bool },
    Detail { surface: Surface, gen: CatGen, asset: AssetId },
    Manifest { surface: Surface, gen: CatGen, asset: AssetId, revision: AssetRevisionId },
    Thumb { revision: AssetRevisionId },
    /// A vjeffect's splash source, fetched to render its ANIMATED thumbnail
    /// offscreen (see fx_thumbs.rs).
    FxSource { asset: AssetId, revision: AssetRevisionId },
    /// A vjeffect's splash source, fetched to LOAD into an effect slot
    /// (fx_slot.rs). `title` rides along so the tile can wear the catalog
    /// name even when the document's own name differs.
    FxSlotSource { slot: FxSlotKind, revision: AssetRevisionId, title: String },
    JobProfiles,
    JobEnqueue { tag: GenTag },
    JobStatus { job: JobId },
    JobCancel { job: JobId },
    /// Offering this machine's locally computed stems/lyrics back to the
    /// store. Fire and forget: one line either way, never a dialog.
    SideChannelPublish { asset: AssetId },
}

/// What a media-lane request was for (keyed by `(lane, request)`).
#[derive(Clone, Debug)]
enum MediaPurpose {
    Cue { gen: CueGen },
    /// The cue's companion file (grouped sprite actor manifest text).
    CueSource { gen: CueGen },
    Deck { deck: DeckId, gen: u64, media: MediaType },
    /// One of the four precomputed stem oggs for a deck's track; `index` is
    /// its place in `FileRole::STEMS`.
    DeckStem { deck: DeckId, gen: u64, index: usize },
    /// The deck track's precomputed lyrics document.
    DeckLyrics { deck: DeckId, gen: u64 },
    Pad { pad: AssetId, gen: u64, revision: AssetRevisionId, media: MediaType },
    Mesh { gen: u64 },
}

/// The side-channel blobs a deck load is waiting on. Complete means the
/// whole set landed and the decode job can go out — but only once the track
/// itself is installed, because the track's rate and length decide the
/// chunk geometry.
#[derive(Clone, Debug)]
struct PendingSideChannels {
    gen: u64,
    /// In `FileRole::STEMS` order.
    stems: [Option<PathBuf>; 4],
    /// False once the lyrics landed, or once their fetch failed — a missing
    /// transcript is not a reason to hold the stems back.
    want_lyrics: bool,
    lyrics: Option<PathBuf>,
}

impl PendingSideChannels {
    fn new(gen: u64, want_lyrics: bool) -> PendingSideChannels {
        PendingSideChannels { gen, stems: Default::default(), want_lyrics, lyrics: None }
    }

    fn complete(&self) -> bool {
        self.stems.iter().all(Option::is_some) && !self.want_lyrics
    }

    /// The decode job, once every file is on disk.
    fn into_job(self, deck: DeckId, gen: u64, pcm: Arc<TrackPcm>) -> Option<FetchedJob> {
        let [a, b, c, d] = self.stems;
        Some(FetchedJob {
            deck,
            gen,
            pcm,
            stem_files: [a?, b?, c?, d?],
            lyrics_file: self.lyrics,
        })
    }
}

/// Two-file cue pairing. The engine is a single-media state machine, but a
/// grouped sprite actor is only playable as sheet + manifest, so the host
/// holds both transfers here and reports `media_ready` once — when the pair
/// is complete. Latest-click-wins: a newer generation replaces the pair
/// wholesale, so a straggling old transfer can never complete a new cue.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CuePair {
    gen: CueGen,
    sheet: Option<PathBuf>,
    manifest: Option<PathBuf>,
}

impl CuePair {
    fn begin(gen: CueGen) -> CuePair {
        CuePair { gen, sheet: None, manifest: None }
    }

    /// Record the primary (sheet) file; `Some(path)` once both landed.
    fn sheet_landed(&mut self, gen: CueGen, path: PathBuf) -> Option<PathBuf> {
        if self.gen != gen {
            return None;
        }
        self.sheet = Some(path);
        self.complete()
    }

    /// Record the companion manifest; `Some(sheet)` once both landed.
    fn manifest_landed(&mut self, gen: CueGen, path: PathBuf) -> Option<PathBuf> {
        if self.gen != gen {
            return None;
        }
        self.manifest = Some(path);
        self.complete()
    }

    fn complete(&self) -> Option<PathBuf> {
        self.manifest.as_ref()?;
        self.sheet.clone()
    }

    /// The manifest path for `gen`, for the slot that is opening now.
    fn manifest_for(&self, gen: CueGen) -> Option<PathBuf> {
        (self.gen == gen).then(|| self.manifest.clone()).flatten()
    }
}

/// How long connection-class failures must persist before the session is
/// declared lost (two subscriber retries; a single slow poll never trips it).
const SESSION_LOSS_GRACE_S: f64 = 5.0;

/// Failures that mean "nobody is listening at this address any more" — as
/// opposed to a request the server answered badly. Only these may trigger
/// re-discovery; a 404 or a digest mismatch must not tear a session down.
fn is_session_loss(error: &ClientError) -> bool {
    use std::io::ErrorKind as K;
    match error {
        ClientError::Io { kind, .. } => matches!(
            kind,
            K::ConnectionRefused
                | K::ConnectionReset
                | K::ConnectionAborted
                | K::NotConnected
                | K::BrokenPipe
                | K::TimedOut
                | K::HostUnreachable
                | K::NetworkUnreachable
                | K::AddrNotAvailable
        ),
        // A different server now answers at the address: the old one is gone.
        ClientError::ServerIdentityMismatch { .. } => true,
        ClientError::Timeout { .. } => true,
        _ => false,
    }
}

// player_nav: catalog anchor → the render-side plain struct. The importer
// only writes yaw-about-+Y rotations, but the general quat→yaw form keeps a
// hand-authored anchor from decoding to garbage.
fn nav_anchor(a: &Anchor) -> makepad_render::player_nav::NavAnchor {
    let r = &a.transform.rot;
    let yaw = (2.0 * (r.w * r.y + r.x * r.z)).atan2(1.0 - 2.0 * (r.y * r.y + r.z * r.z));
    makepad_render::player_nav::NavAnchor {
        name: a.name.clone(),
        pos: vec3f(a.transform.pos.x, a.transform.pos.y, a.transform.pos.z),
        yaw,
        scale: vec3f(a.transform.scale.x, a.transform.scale.y, a.transform.scale.z),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn select_visual_file(manifest: &AssetManifest) -> Option<TileMedia> {
    const ROLES: [FileRole; 5] = [
        FileRole::Video,
        FileRole::RenderGlb,
        FileRole::Texture,
        FileRole::Albedo,
        FileRole::PreviewFront,
    ];
    for role in ROLES {
        if let Ok(file) = select_file(
            manifest,
            role,
            TierPreference::PreferWithAnyFallback(DeviceTier::High),
            7,
        ) {
            return Some(TileMedia {
                blob: file.blob,
                len: file.byte_len,
                media: file.media,
            });
        }
    }
    None
}

/// A vjeffect's one real file: the splash document (role `Source`, media
/// `Text`). It is the tile's "playable" file — the thumbnail renderer
/// fetches it to run the effect offscreen — so an effect tile resolves
/// Ready instead of wearing "no playable file" across the whole library.
fn select_vjfx_source(manifest: &AssetManifest) -> Option<TileMedia> {
    if manifest.kind != AssetKind::VjEffect {
        return None;
    }
    let file = select_file(
        manifest,
        FileRole::Source,
        TierPreference::PreferWithAnyFallback(DeviceTier::High),
        7,
    )
    .ok()?;
    (file.media == MediaType::Text).then_some(TileMedia {
        blob: file.blob,
        len: file.byte_len,
        media: file.media,
    })
}

/// The `stateful-billboard` manifest text a grouped sprite actor publishes
/// beside its packed sheet (role `Source`, media `Text`). Its presence is
/// also how the VJ tells a grouped actor from a legacy per-lump sprite.
fn select_billboard_source(manifest: &AssetManifest) -> Option<TileMedia> {
    if manifest.kind != AssetKind::Billboard {
        return None;
    }
    let file = select_file(
        manifest,
        FileRole::Source,
        TierPreference::PreferWithAnyFallback(DeviceTier::High),
        7,
    )
    .ok()?;
    (file.media == MediaType::Text).then_some(TileMedia {
        blob: file.blob,
        len: file.byte_len,
        media: file.media,
    })
}

/// The precomputed analysis a music revision carries: four stem oggs and/or
/// a lyrics document, published as side-channel files on the audio asset.
///
/// Read straight off the manifest rather than through `select_file`: a
/// side-channel has exactly one slot (tier `Any`, lod 0) by contract, so
/// there is nothing to select between, and the stem set is all-four-or-none.
fn side_channel_refs(manifest: &AssetManifest) -> TrackSideChannels {
    side_channel_refs_of(&manifest.files)
}

fn side_channel_refs_of(files: &[makepad_asset_data::AssetFile]) -> TrackSideChannels {
    let file = |role: FileRole| {
        files
            .iter()
            .find(|f| f.role == role)
            .map(|f| (f.blob, f.byte_len))
    };
    let stems = FileRole::STEMS.map(file);
    TrackSideChannels {
        stems: stems
            .iter()
            .all(Option::is_some)
            .then(|| stems.map(|slot| slot.expect("checked above"))),
        lyrics: file(FileRole::Lyrics),
    }
}

fn format_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    let minutes = (secs / 60.0).floor() as u64;
    format!("{minutes}:{:04.1}", secs - minutes as f64 * 60.0)
}

// ---------------------------------------------------------------------------
// system-audio capture + beat-quantized scheduling
// ---------------------------------------------------------------------------

/// Mono capacity of the capture ring, samples (~0.7 s at 48 kHz). Power of
/// two so index wrapping is a mask.
const CAPTURE_RING: usize = 32_768;

/// Bounded lock-free single-producer/single-consumer feed from the
/// system-audio (loopback) input callback to the beat-analysis worker.
///
/// Realtime contract (producer = the device callback): bounded downmix +
/// copy into this preallocated ring using atomics only — no locks, no
/// allocation, no filesystem, no blocking. On overflow the NEWEST samples
/// are dropped and counted; the callback never waits for the consumer.
pub struct CaptureFeed {
    ring: Vec<AtomicU32>,
    /// Monotonic sample indices (masked into the power-of-two ring).
    head: AtomicUsize,
    tail: AtomicUsize,
    sample_rate: AtomicU32,
    frames_written: AtomicU64,
    dropped_samples: AtomicU64,
    peak: AtomicU32,
}

/// Capture health for honest UI status.
#[derive(Clone, Copy, Debug, Default)]
pub struct CaptureStats {
    pub sample_rate: u32,
    pub frames_written: u64,
    pub dropped_samples: u64,
    pub peak: f32,
}

impl CaptureFeed {
    pub fn new() -> CaptureFeed {
        CaptureFeed {
            ring: (0..CAPTURE_RING).map(|_| AtomicU32::new(0)).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            sample_rate: AtomicU32::new(0),
            frames_written: AtomicU64::new(0),
            dropped_samples: AtomicU64::new(0),
            peak: AtomicU32::new(0),
        }
    }

    /// Producer side (realtime callback): average the planar channels to
    /// mono and copy in. Bounded work, atomics only.
    pub fn push(&self, sample_rate: f64, buffer: &makepad_platform::audio::AudioBuffer) {
        let frames = buffer.frame_count();
        let channels = buffer.channel_count().max(1);
        if frames == 0 {
            return;
        }
        self.sample_rate.store(sample_rate.max(0.0) as u32, Ordering::Relaxed);
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);
        let free = CAPTURE_RING - head.wrapping_sub(tail);
        let take = frames.min(free);
        let scale = 1.0 / channels as f32;
        let mut peak = 0.0f32;
        for frame in 0..frames {
            let mut mono = 0.0f32;
            for channel in 0..channels {
                mono += buffer.channel(channel)[frame];
            }
            mono *= scale;
            peak = peak.max(mono.abs());
            // The block peak covers even dropped samples so the status
            // meter stays honest under overrun.
            if frame < take {
                self.ring[head.wrapping_add(frame) & (CAPTURE_RING - 1)]
                    .store(mono.to_bits(), Ordering::Relaxed);
            }
        }
        self.head.store(head.wrapping_add(take), Ordering::Release);
        self.frames_written.fetch_add(frames as u64, Ordering::Relaxed);
        if take < frames {
            self.dropped_samples.fetch_add((frames - take) as u64, Ordering::Relaxed);
        }
        self.peak.store(peak.to_bits(), Ordering::Relaxed);
    }

    /// Consumer side (analysis worker): append everything available to
    /// `out` and return the producer's sample rate.
    pub fn drain_into(&self, out: &mut Vec<f32>) -> u32 {
        let head = self.head.load(Ordering::Acquire);
        let mut tail = self.tail.load(Ordering::Relaxed);
        while tail != head {
            out.push(f32::from_bits(
                self.ring[tail & (CAPTURE_RING - 1)].load(Ordering::Relaxed),
            ));
            tail = tail.wrapping_add(1);
        }
        self.tail.store(tail, Ordering::Release);
        self.sample_rate.load(Ordering::Relaxed)
    }

    pub fn stats(&self) -> CaptureStats {
        CaptureStats {
            sample_rate: self.sample_rate.load(Ordering::Relaxed),
            frames_written: self.frames_written.load(Ordering::Relaxed),
            dropped_samples: self.dropped_samples.load(Ordering::Relaxed),
            peak: f32::from_bits(self.peak.load(Ordering::Relaxed)),
        }
    }
}

impl Default for CaptureFeed {
    fn default() -> Self {
        Self::new()
    }
}

/// One coherent beat estimate. `next_beat` is a host-clock deadline that
/// extrapolates forward whole periods, so a slightly stale snapshot still
/// quantizes correctly.
#[derive(Clone, Debug)]
pub struct BeatInfo {
    pub bpm: f32,
    /// 0..1 detector confidence; the fade policy tiers on this.
    pub confidence: f32,
    /// True only while the detector holds a stable lock (silence unlocks).
    pub locked: bool,
    pub period: Duration,
    pub next_beat: Instant,
    /// Bar position of `next_beat` (bar = 4 beats): 0 = downbeat.
    pub beat_index: u64,
    /// Confident beats observed since the current lock began.
    pub beats_observed: u64,
}

/// Columns of capture envelope the chrome bar's wave keeps.
pub const WAVE_COLS: usize = 256;
/// One envelope column per 10 ms of AUDIO — the ring advances on the sample
/// clock, not on the UI pump, so its time axis is exactly uniform however
/// late a frame runs. 256 columns is 2.56 s of history.
pub const WAVE_HZ: f64 = 100.0;

/// The capture envelope of the last [`WAVE_COLS`] columns: peak in the high
/// byte, RMS in the low one, so one `u16` is a whole column.
#[derive(Clone, Copy)]
pub struct WaveRing {
    cols: [u16; WAVE_COLS],
    /// Where the next column goes; the oldest kept column is here too.
    head: usize,
}

impl Default for WaveRing {
    fn default() -> Self {
        WaveRing { cols: [0; WAVE_COLS], head: 0 }
    }
}

impl WaveRing {
    fn push(&mut self, peak: f32, rms: f32) {
        // The same perceptual curve the deck waveforms use, so a quiet room
        // still draws a legible wave and the two surfaces agree about how
        // loud "loud" looks.
        let curve = |value: f32| {
            (value.clamp(0.0, 1.0).powf(crate::wave_analysis::WAVE_CURVE) * 255.0).round() as u16
        };
        let peak = curve(peak);
        let rms = curve(rms);
        self.cols[self.head] = (peak << 8) | rms;
        self.head = (self.head + 1) % WAVE_COLS;
    }

    /// Unroll into `out`, oldest column first.
    pub fn unroll(&self, out: &mut Vec<u16>) {
        out.clear();
        out.extend_from_slice(&self.cols[self.head..]);
        out.extend_from_slice(&self.cols[..self.head]);
    }
}

/// Published by the sync worker; read by the UI thread each pump.
#[derive(Clone, Default)]
pub struct SyncSnapshot {
    pub sample_rate: u32,
    pub frames: u64,
    pub dropped: u64,
    pub peak: f32,
    pub lock_state: BeatLockState,
    pub beat: Option<BeatInfo>,
    /// Capture envelope for the bar's wave, and when its newest column was
    /// published — the wave's right edge in host time.
    pub wave: WaveRing,
    pub wave_stamp: Option<Instant>,
    /// True while the detector is parked because a deck owns the clock.
    pub suppressed: bool,
}

struct SyncShared {
    snap: Mutex<SyncSnapshot>,
}

/// The analysis worker: drains the capture ring OFF the realtime callback
/// and publishes a coherent snapshot for the UI thread. The pure beat
/// detector (`beat_sync`, separate module lane) plugs into the marked spot;
/// until it lands the snapshot carries capture health and `beat` stays
/// honestly `None`, so every consumer falls back to immediate fades.
pub struct SyncWorker {
    shared: Arc<SyncShared>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    resync: Arc<std::sync::atomic::AtomicBool>,
    suppress: Arc<std::sync::atomic::AtomicBool>,
}

impl SyncWorker {
    pub fn start(feed: Arc<CaptureFeed>) -> SyncWorker {
        let shared = Arc::new(SyncShared { snap: Mutex::new(SyncSnapshot::default()) });
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let resync = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let suppress = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_shared = shared.clone();
        let thread_stop = stop.clone();
        let thread_resync = resync.clone();
        let thread_suppress = suppress.clone();
        let _ = std::thread::Builder::new().name("vj-beat-sync".into()).spawn(move || {
            let mut scratch: Vec<f32> = Vec::with_capacity(CAPTURE_RING);
            let mut analyzer: Option<BeatSyncAnalyzer> = None;
            let mut lock_started_beat: Option<i64> = None;
            let mut seen_dropped: u64 = 0;
            let mut wave = WaveRing::default();
            // Partial envelope column: peak, sum of squares, samples.
            let mut column = (0.0f32, 0.0f64, 0usize);
            let mut wave_stamp: Option<Instant> = None;
            let mut was_suppressed = false;
            while !thread_stop.load(Ordering::Acquire) {
                scratch.clear();
                // The ring is ALWAYS drained, suppressed or not: leaving it
                // to overflow would count as dropped samples and reset the
                // analyzer under us the moment it is wanted again.
                let rate = feed.drain_into(&mut scratch);
                // While a deck is playing, the loopback detector would be
                // re-detecting OUR OWN output — a laggier, wobblier copy of
                // a grid the deck already knows sample-exactly. Park it.
                let suppressed = thread_suppress.load(Ordering::Acquire);
                if suppressed != was_suppressed {
                    was_suppressed = suppressed;
                    // Coming back, its sample clock has a hole in it: the
                    // only honest thing to do is start looking again.
                    if let Some(analyzer) = analyzer.as_mut() {
                        analyzer.reset();
                    }
                    lock_started_beat = None;
                }
                // RESYNC: throw the grid away and re-derive tempo and phase
                // from the audio that is playing now.
                if thread_resync.swap(false, Ordering::AcqRel) {
                    if let Some(analyzer) = analyzer.as_mut() {
                        analyzer.reset();
                    }
                    lock_started_beat = None;
                }
                if rate >= 8_000 && !scratch.is_empty() {
                    // The envelope the chrome bar draws, on the SAMPLE clock:
                    // one column per `1/WAVE_HZ` seconds of audio however
                    // unevenly this thread happens to be scheduled.
                    let step = ((rate as f64 / WAVE_HZ).round() as usize).max(1);
                    for sample in &scratch {
                        let sample = if sample.is_finite() { *sample } else { 0.0 };
                        column.0 = column.0.max(sample.abs());
                        column.1 += (sample as f64) * (sample as f64);
                        column.2 += 1;
                        if column.2 >= step {
                            wave.push(column.0, (column.1 / column.2 as f64).sqrt() as f32);
                            column = (0.0, 0.0, 0);
                        }
                    }
                    wave_stamp = Some(Instant::now());

                    // A device-rate change restarts the analyzer: its whole
                    // grid is in samples of one rate.
                    let stale = analyzer
                        .as_ref()
                        .is_some_and(|a| (a.sample_rate() - rate as f64).abs() > 0.5);
                    if stale {
                        analyzer = None;
                        lock_started_beat = None;
                    }
                    // The wave above is drawn whatever happens — it proves
                    // the right audio is arriving — but the ANALYSIS is what
                    // costs, and it is pointless while a deck owns the clock.
                    if !suppressed {
                        analyzer
                            .get_or_insert_with(|| BeatSyncAnalyzer::new(rate as f64))
                            .push_mono(&scratch);
                    }
                }
                let stats = feed.stats();
                if stats.dropped_samples > seen_dropped {
                    // Lost capture samples silently shift the analyzer's
                    // sample clock against real time — its phase would be
                    // confidently wrong. Start clean instead.
                    seen_dropped = stats.dropped_samples;
                    if let Some(analyzer) = analyzer.as_mut() {
                        analyzer.reset();
                    }
                    lock_started_beat = None;
                }
                let analyzer_snapshot =
                    analyzer.as_ref().filter(|_| !suppressed).map(BeatSyncAnalyzer::snapshot);
                let lock_state = analyzer_snapshot
                    .as_ref()
                    .map(|snapshot| snapshot.state)
                    .unwrap_or_default();
                let beat = analyzer_snapshot.as_ref().and_then(|snapshot| {
                    beat_info_from_snapshot(snapshot, &mut lock_started_beat, Instant::now())
                });
                {
                    let mut snap = thread_shared.snap.lock().unwrap();
                    snap.sample_rate = rate;
                    snap.frames = stats.frames_written;
                    snap.dropped = stats.dropped_samples;
                    snap.peak = stats.peak;
                    snap.lock_state = lock_state;
                    snap.beat = beat;
                    snap.wave = wave;
                    snap.wave_stamp = wave_stamp;
                    snap.suppressed = suppressed;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        SyncWorker { shared, stop, resync, suppress }
    }

    /// Park the detector: a deck is playing, so the room is only ever going
    /// to tell us what that deck already knows exactly. The capture ring is
    /// still drained and the wave still drawn; only the analysis stops.
    pub fn set_suppressed(&self, on: bool) {
        self.suppress.store(on, Ordering::Release);
    }

    pub fn snapshot(&self) -> SyncSnapshot {
        self.shared.snap.lock().unwrap().clone()
    }

    /// Drop the tracked grid and re-derive tempo and phase from the audio
    /// that is playing now. This is what RESYNC means: the MACHINE decides
    /// where the beat is, from scratch.
    pub fn resync(&self) {
        self.resync.store(true, Ordering::Release);
    }
}

/// Project the analyzer's sample-clock grid onto the host clock at the
/// moment the samples were drained. The residual error is the capture ring
/// latency (≤ ~10 ms), which is far inside the fade slew.
///
/// `lock_started` tracks the analyzer beat index at which the current lock
/// began, so bar preference can require a tracked history.
fn beat_info_from_snapshot(
    snapshot: &BeatSnapshot,
    lock_started: &mut Option<i64>,
    now: Instant,
) -> Option<BeatInfo> {
    if !snapshot.has_grid() || snapshot.sample_rate <= 0.0 {
        *lock_started = None;
        return None;
    }
    let locked = snapshot.is_locked();
    match (locked, lock_started.is_some()) {
        (true, false) => *lock_started = Some(snapshot.beat_index),
        (false, _) => *lock_started = None,
        _ => {}
    }
    let period = Duration::from_secs_f64(snapshot.seconds_per_beat()?);
    let boundary = snapshot.next_boundary(snapshot.sample_position, 1)?;
    let delta_secs =
        boundary.saturating_sub(snapshot.sample_position) as f64 / snapshot.sample_rate;
    let next_beat = now + Duration::from_secs_f64(delta_secs);
    let beats_observed = lock_started
        .map(|start| (snapshot.beat_index - start).max(0) as u64)
        .unwrap_or(0);
    Some(BeatInfo {
        bpm: snapshot.bpm as f32,
        confidence: snapshot.confidence,
        locked,
        period,
        next_beat,
        // The boundary after `sample_position` is beat_index + 1 on the
        // analyzer grid; bars are a stable 4-grouping of it (the detector
        // has no downbeat estimate — the grouping is consistent, not
        // musically anchored).
        beat_index: (snapshot.beat_index + 1).rem_euclid(BAR_BEATS as i64) as u64,
        beats_observed,
    })
}

impl Drop for SyncWorker {
    fn drop(&mut self) {
        // Detached teardown: flag it and walk away (never a UI-thread join).
        self.stop.store(true, Ordering::Release);
    }
}

/// Confidence tiers: below `CONF_QUANTIZE` the program stays immediate and
/// honest; between the tiers, starts quantize to the beat but transitions
/// are hard cuts; at/above `CONF_MUSICAL`, fades run for a whole number of
/// beats and prefer landing on a bar.
const CONF_QUANTIZE: f32 = 0.5;
const CONF_MUSICAL: f32 = 0.75;
const BAR_BEATS: u64 = 4;
/// How long the TAP button stays lit after a press.
const TAP_FLASH_SECS: f64 = 0.14;
/// Bar preference needs this many tracked beats — a fresh lock has no
/// trustworthy downbeat yet.
const MIN_BEATS_FOR_BAR: u64 = 8;
/// A bar wait longer than this falls back to the next beat (the performer
/// clicked for a reason).
const BAR_WAIT_MAX: Duration = Duration::from_millis(2600);
/// The "quantized cut" length: click-free but visually square.
const CUT_SECS: f32 = 0.08;

/// Where a requested program fade should start and how long it should run.
#[derive(Clone, Debug, PartialEq)]
enum FadePlan {
    Immediate { secs: f32 },
    Quantized { fire_at: Instant, secs: f32, kind: &'static str },
}

/// Advance a beat estimate so `next_beat` lies in the future of `now`.
fn extrapolate_beat(beat: &BeatInfo, now: Instant) -> BeatInfo {
    let mut beat = beat.clone();
    if beat.period.is_zero() {
        return beat;
    }
    if beat.next_beat < now {
        let behind = now.duration_since(beat.next_beat);
        let periods = ((behind.as_secs_f64() / beat.period.as_secs_f64()).floor() as u64 + 1)
            .min(1_000_000);
        beat.next_beat += beat.period * periods as u32;
        beat.beat_index = (beat.beat_index + periods) % BAR_BEATS;
    }
    beat
}

/// An operator-set beat clock, from the TAP button.
///
/// Two strengths, and the difference is the whole point of the button:
///
/// * `bpm: None` — **the one is HERE**. Tempo keeps coming from whatever
///   clock is underneath (deck grid or detector); only the downbeat moves to
///   the moment of the press. This is the fix for a grid that is right about
///   the tempo and wrong about which beat is beat one.
/// * `bpm: Some(..)` — tempo AND phase come from the taps: the operator's
///   ear replaces the machine's, for as long as the override stands.
///
/// It outranks every other clock while it stands, because the operator asked
/// for it. See `App::pump_beat_override` for how the machine gets it back.
#[derive(Clone, Copy, Debug)]
struct BeatOverride {
    /// The tap that defined beat one.
    anchor: Instant,
    bpm: Option<f64>,
    /// Which deck (if any) was leading when the override was set.
    leader: Option<DeckId>,
    /// Set once the detector has actually LOST its grid since the tap: only
    /// then may a fresh confident lock take the clock back.
    saw_unlock: bool,
}

impl BeatOverride {
    /// Project the override onto `now`. `base` supplies the tempo for a
    /// phase-only anchor, so an override with no tapped tempo dies quietly
    /// (returns `None`) when there is no clock under it to anchor.
    fn beat(&self, base: Option<&BeatInfo>, now: Instant) -> Option<BeatInfo> {
        let period = match self.bpm {
            Some(bpm) if bpm.is_finite() && bpm > 0.0 => Duration::from_secs_f64(60.0 / bpm),
            _ => base.map(|beat| beat.period).filter(|period| !period.is_zero())?,
        };
        let secs = period.as_secs_f64();
        let elapsed = now.saturating_duration_since(self.anchor).as_secs_f64();
        let beats = (elapsed / secs).floor().max(0.0);
        Some(BeatInfo {
            bpm: (60.0 / secs) as f32,
            // The operator is the authority here: a tapped clock quantizes
            // like a hard lock, which is why one taps in the first place.
            confidence: 1.0,
            locked: true,
            period,
            next_beat: self.anchor + Duration::from_secs_f64((beats + 1.0) * secs),
            beat_index: (beats as u64 + 1) % BAR_BEATS,
            // Bar preference still has to earn itself after the tap.
            beats_observed: beats as u64,
        })
    }
}

/// How many beats one cycle of an `frames`-frame sprite state spans when
/// the slot is beat-synced.
///
/// The rule is FOUR FRAMES TO THE BEAT, rounded to a power-of-two number of
/// beats and held between one and eight. A four-frame walk is one frame per
/// quarter beat and marches one cycle per beat; an eight-frame cycle takes
/// two beats at the same stride; a sixteen-frame one takes a bar. Powers of
/// two are the whole point — any other rounding puts the loop point
/// somewhere that is not a musical boundary, and the eye sees that.
fn sprite_cycle_beats(frames: usize) -> u32 {
    let want = (frames.max(1) as f64 / 4.0).max(1.0);
    let below = 1u32 << want.log2().floor().clamp(0.0, 3.0) as u32;
    let above = (below * 2).min(8);
    // Nearest in RATIO, which is what "nearest power of two" has to mean.
    match want / below as f64 <= above as f64 / want {
        true => below,
        false => above,
    }
}

/// The source ladder, as a decision.
///
/// Operator first, always. Then: a deck FOLLOWING the room means the room
/// is in charge and that deck's grid must never be read back (it would be
/// chasing itself). Otherwise a playing deck is master — the normal show,
/// and while it is, the detector has nothing to add but latency, because
/// all it can hear is that deck's own output. The detector is the source
/// only when the VJ is standing alone against somebody else's music.
fn resolve_clock_source(
    operator: bool,
    external_follow: bool,
    deck: bool,
    detector: bool,
) -> ClockSource {
    if operator {
        return ClockSource::Operator;
    }
    match (external_follow, deck, detector) {
        (true, _, true) => ClockSource::External,
        (_, true, _) => ClockSource::Deck,
        (_, false, true) => ClockSource::Detector,
        _ => ClockSource::None,
    }
}

/// Where a raw estimate says the beat is, in the clock's own terms.
///
/// `bar_aware` sources (a deck's analysed grid, an operator tap) can vouch
/// for the downbeat, so their corrections may move the bar; a detector
/// listening to a room cannot, so it only ever corrects the phase.
fn beat_target(beat: &BeatInfo, now: Instant, bar_aware: bool) -> Option<BeatTarget> {
    let period = beat.period.as_secs_f64();
    if !(period > 0.0) || !period.is_finite() {
        return None;
    }
    let beat = extrapolate_beat(beat, now);
    let until = beat.next_beat.saturating_duration_since(now).as_secs_f64();
    let phase = (1.0 - until / period).clamp(0.0, 1.0);
    Some(match bar_aware {
        true => BeatTarget::in_bar((beat.beat_index + BAR_BEATS - 1) % BAR_BEATS, phase, period),
        false => BeatTarget::phase_only(phase, period),
    })
}

/// The detector has genuinely dropped its grid — not merely re-settled on
/// one. Only this arms a hand-back of the clock from a tap override.
fn detector_lost_grid(lock_state: BeatLockState) -> bool {
    matches!(lock_state, BeatLockState::Unlocked | BeatLockState::Lost)
}

/// Whether a fresh detector lock may take the clock back from a tap
/// override: it must have lost the grid first (a new track), and the lock it
/// came back with must be one the fade policy would trust on its own.
fn detector_reclaims(lock_state: BeatLockState, confidence: f32, saw_unlock: bool) -> bool {
    matches!(lock_state, BeatLockState::Locked) && saw_unlock && confidence >= CONF_MUSICAL
}

/// The quantize policy. No lock (or low confidence) = immediate with the
/// authored duration — never a fake sync. Medium confidence = a hard cut on
/// the next beat. High confidence = an integer-beat fade near the authored
/// length, starting on the next bar when the lock has tracked enough beats
/// and the wait stays performable, else the next beat.
fn plan_fade(beat: Option<&BeatInfo>, now: Instant, authored_secs: f32) -> FadePlan {
    let Some(beat) = beat else {
        return FadePlan::Immediate { secs: authored_secs };
    };
    if !beat.locked || beat.confidence < CONF_QUANTIZE || beat.period.is_zero() {
        return FadePlan::Immediate { secs: authored_secs };
    }
    let beat = extrapolate_beat(beat, now);
    if beat.confidence < CONF_MUSICAL {
        return FadePlan::Quantized {
            fire_at: beat.next_beat,
            secs: CUT_SECS,
            kind: "cut on beat",
        };
    }
    let period_secs = beat.period.as_secs_f32();
    let beats = (authored_secs / period_secs).round().clamp(1.0, 8.0);
    let secs = beats * period_secs;
    let to_bar = (BAR_BEATS - (beat.beat_index % BAR_BEATS)) % BAR_BEATS;
    let bar_at = beat.next_beat + beat.period * to_bar as u32;
    if beat.beats_observed >= MIN_BEATS_FOR_BAR && bar_at.duration_since(now) <= BAR_WAIT_MAX {
        return FadePlan::Quantized { fire_at: bar_at, secs, kind: "fade on bar" };
    }
    FadePlan::Quantized { fire_at: beat.next_beat, secs, kind: "fade on beat" }
}

/// Master policy switch for external sync. Analysis keeps running while the
/// switch is off, but cues use their authored immediate fade and never arm to
/// a stale or unwanted grid.
fn plan_external_fade(
    enabled: bool,
    beat: Option<&BeatInfo>,
    now: Instant,
    authored_secs: f32,
) -> FadePlan {
    if enabled {
        plan_fade(beat, now, authored_secs)
    } else {
        FadePlan::Immediate { secs: authored_secs }
    }
}

/// UI/reschedule mirror of a device-clock-armed transition. The mixer owns
/// the actual deadline; this remembers the plan so a `Missed` transition can
/// be re-quantized and the header can show an honest countdown.
#[derive(Clone, Copy)]
struct ArmedFadeUi {
    gen: CueGen,
    schedule: CueScheduleId,
    from: Option<SlotId>,
    to: SlotId,
    fire_at: Instant,
    secs: f32,
    kind: &'static str,
    retries: u32,
}

// ---------------------------------------------------------------------------
// live loop analysis (frame signatures → loop_detect, off the UI thread)
// ---------------------------------------------------------------------------

/// Signature raster and block-grid bounds. The raster is point-sampled from
/// the frame (≤ 2.3k texels), so signature building costs microseconds on
/// the UI thread; everything heavier runs on the loop worker.
const SIG_RASTER_W: usize = 64;
const SIG_RASTER_H: usize = 36;
const SIG_LUMA_W: usize = 16;
const SIG_LUMA_H: usize = 9;
const SIG_CHROMA_W: usize = 8;
const SIG_CHROMA_H: usize = 5;
/// Block-motion search radius on the raster, cells.
const SIG_MOTION_SEARCH: isize = 3;
/// Motion block edge, raster cells.
const SIG_MOTION_BLOCK: usize = 8;
/// Safe playback-rate deviation for loop fitting (mirrors the mixer's
/// clamp range).
const MAX_LOOP_RATE_DEVIATION: f64 = 0.08;
/// Loop reports below this confidence never drive rate sync.
const LOOP_FIT_MIN_CONFIDENCE: f32 = 0.60;

/// Per-slot scratch for incremental signature building.
#[derive(Default)]
struct SigState {
    prev_raster: Vec<f32>,
}

/// Sample a bounded luma raster + block signature from one BGRA frame and
/// estimate block motion against the previous raster of the same slot.
fn build_frame_signature(
    bgra: &[u32],
    width: usize,
    height: usize,
    state: &mut SigState,
) -> FrameSignature {
    let mut raster = vec![0.0f32; SIG_RASTER_W * SIG_RASTER_H];
    let mut chroma = vec![[0.0f32; 2]; SIG_CHROMA_W * SIG_CHROMA_H];
    let mut chroma_counts = vec![0u32; SIG_CHROMA_W * SIG_CHROMA_H];
    if width == 0 || height == 0 || bgra.len() < width * height {
        state.prev_raster.clear();
        return FrameSignature::from_luma(vec![0.0; SIG_LUMA_W * SIG_LUMA_H], MotionSummary::default());
    }
    for ry in 0..SIG_RASTER_H {
        let y = (ry * height + height / 2) / SIG_RASTER_H;
        for rx in 0..SIG_RASTER_W {
            let x = (rx * width + width / 2) / SIG_RASTER_W;
            let px = bgra[y.min(height - 1) * width + x.min(width - 1)];
            let r = ((px >> 16) & 0xff) as f32 / 255.0;
            let g = ((px >> 8) & 0xff) as f32 / 255.0;
            let b = (px & 0xff) as f32 / 255.0;
            raster[ry * SIG_RASTER_W + rx] = 0.299 * r + 0.587 * g + 0.114 * b;
            let cx = rx * SIG_CHROMA_W / SIG_RASTER_W;
            let cy = ry * SIG_CHROMA_H / SIG_RASTER_H;
            let cell = cy * SIG_CHROMA_W + cx;
            chroma[cell][0] += r;
            chroma[cell][1] += b;
            chroma_counts[cell] += 1;
        }
    }
    for (cell, count) in chroma.iter_mut().zip(&chroma_counts) {
        let n = (*count).max(1) as f32;
        cell[0] /= n;
        cell[1] /= n;
    }
    // Luma + edge blocks summarize raster cells.
    let mut luma = vec![0.0f32; SIG_LUMA_W * SIG_LUMA_H];
    let mut edge = vec![0.0f32; SIG_LUMA_W * SIG_LUMA_H];
    let cells_x = SIG_RASTER_W / SIG_LUMA_W;
    let cells_y = SIG_RASTER_H / SIG_LUMA_H;
    for by in 0..SIG_LUMA_H {
        for bx in 0..SIG_LUMA_W {
            let mut sum = 0.0;
            let mut gradient = 0.0;
            for cy in 0..cells_y {
                for cx in 0..cells_x {
                    let x = bx * cells_x + cx;
                    let y = by * cells_y + cy;
                    let value = raster[y * SIG_RASTER_W + x];
                    sum += value;
                    let right = raster[y * SIG_RASTER_W + (x + 1).min(SIG_RASTER_W - 1)];
                    let down = raster[(y + 1).min(SIG_RASTER_H - 1) * SIG_RASTER_W + x];
                    gradient += (right - value).abs() + (down - value).abs();
                }
            }
            let n = (cells_x * cells_y) as f32;
            luma[by * SIG_LUMA_W + bx] = sum / n;
            edge[by * SIG_LUMA_W + bx] = (gradient / n * 4.0).min(1.0);
        }
    }
    let motion = estimate_block_motion(&state.prev_raster, &raster);
    state.prev_raster = raster;
    FrameSignature::new(luma, chroma, edge, motion)
}

/// Coarse block-matching motion between two rasters: mean translation,
/// radial divergence and the fraction of blocks with a reliable match.
fn estimate_block_motion(previous: &[f32], current: &[f32]) -> MotionSummary {
    if previous.len() != SIG_RASTER_W * SIG_RASTER_H
        || current.len() != SIG_RASTER_W * SIG_RASTER_H
    {
        return MotionSummary::default();
    }
    let blocks_x = SIG_RASTER_W / SIG_MOTION_BLOCK;
    let blocks_y = SIG_RASTER_H / SIG_MOTION_BLOCK;
    let mut sum_x = 0.0f32;
    let mut sum_y = 0.0f32;
    let mut sum_divergence = 0.0f32;
    let mut reliable = 0usize;
    let mut counted = 0usize;
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let x0 = bx * SIG_MOTION_BLOCK;
            let y0 = by * SIG_MOTION_BLOCK;
            let sad_at = |dx: isize, dy: isize| -> Option<f32> {
                let mut sum = 0.0;
                for y in 0..SIG_MOTION_BLOCK {
                    for x in 0..SIG_MOTION_BLOCK {
                        let cx = x0 as isize + x as isize + dx;
                        let cy = y0 as isize + y as isize + dy;
                        if cx < 0
                            || cy < 0
                            || cx >= SIG_RASTER_W as isize
                            || cy >= SIG_RASTER_H as isize
                        {
                            return None;
                        }
                        let a = current[(y0 + y) * SIG_RASTER_W + x0 + x];
                        let b = previous[cy as usize * SIG_RASTER_W + cx as usize];
                        sum += (a - b).abs();
                    }
                }
                Some(sum / (SIG_MOTION_BLOCK * SIG_MOTION_BLOCK) as f32)
            };
            let Some(still) = sad_at(0, 0) else { continue };
            let mut best = (0isize, 0isize, still);
            for dy in -SIG_MOTION_SEARCH..=SIG_MOTION_SEARCH {
                for dx in -SIG_MOTION_SEARCH..=SIG_MOTION_SEARCH {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    if let Some(sad) = sad_at(dx, dy) {
                        if sad < best.2 {
                            best = (dx, dy, sad);
                        }
                    }
                }
            }
            counted += 1;
            // A match is reliable when moving clearly beats standing still
            // on a textured block. The best offset points INTO the previous
            // frame, so content velocity is its negation.
            if still > 0.004 && best.2 < still * 0.85 {
                reliable += 1;
                let dx = -best.0 as f32 / SIG_RASTER_W as f32;
                let dy = -best.1 as f32 / SIG_RASTER_H as f32;
                sum_x += dx;
                sum_y += dy;
                let nx = (x0 as f32 + SIG_MOTION_BLOCK as f32 * 0.5) / SIG_RASTER_W as f32
                    - 0.5;
                let ny = (y0 as f32 + SIG_MOTION_BLOCK as f32 * 0.5) / SIG_RASTER_H as f32
                    - 0.5;
                let radius = (nx * nx + ny * ny).sqrt().max(0.15);
                sum_divergence += (dx * nx + dy * ny) / radius;
            }
        }
    }
    if counted == 0 {
        return MotionSummary::default();
    }
    let n = reliable.max(1) as f32;
    MotionSummary::new(
        sum_x / n,
        sum_y / n,
        sum_divergence / n,
        reliable as f32 / counted as f32,
    )
}

/// UI → loop-worker feed.
enum LoopScanCtl {
    /// A slot (re)opened on a revision, or closed (`None`): drop its
    /// accumulated signatures.
    Reset { slot: usize, revision: Option<AssetRevisionId> },
    /// One presented frame's signature.
    Sig {
        slot: usize,
        revision: AssetRevisionId,
        position_secs: f64,
        sig: FrameSignature,
    },
}

/// A finished analysis, keyed by immutable revision upstream.
#[derive(Clone, Copy, Debug)]
struct LoopReport {
    detection: LoopDetection,
    /// Source-time seconds of one detected visual cycle.
    period_secs: f64,
}

fn loop_report_matches_media(report: LoopReport, media_secs: f64) -> bool {
    report.period_secs.is_finite()
        && report.period_secs > 0.2
        && media_secs.is_finite()
        && media_secs > 0.2
        // A recurrence detector observing an already-looping decoder can see
        // a 2x/3x harmonic across wraps. Never present or tempo-warp from a
        // cycle longer than the source itself.
        && report.period_secs <= media_secs * 1.05
}

/// Per-slot accumulation on the loop worker.
#[derive(Default)]
struct LoopAccum {
    revision: Option<AssetRevisionId>,
    sigs: Vec<FrameSignature>,
    arrivals: u64,
    accept_mod: u64,
    last_pos: Option<f64>,
    delta_sum: f64,
    delta_count: u64,
    accepted_since_analysis: usize,
}

impl LoopAccum {
    fn reset(&mut self, revision: Option<AssetRevisionId>) {
        *self = LoopAccum { revision, accept_mod: 1, ..LoopAccum::default() };
    }
}

/// Spawn the loop-analysis worker: it accumulates bounded signatures per
/// slot (decimating to stay under `loop_detect`'s frame cap) and publishes
/// a `LoopReport` per revision every couple dozen accepted frames.
fn start_loop_worker() -> (
    std::sync::mpsc::Sender<LoopScanCtl>,
    Arc<Mutex<Vec<(AssetRevisionId, LoopReport)>>>,
) {
    let (tx, rx) = std::sync::mpsc::channel::<LoopScanCtl>();
    let results: Arc<Mutex<Vec<(AssetRevisionId, LoopReport)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let worker_results = results.clone();
    let _ = std::thread::Builder::new().name("vj-loop-detect".into()).spawn(move || {
        let mut slots: [LoopAccum; 2] = Default::default();
        for accum in &mut slots {
            accum.accept_mod = 1;
        }
        while let Ok(msg) = rx.recv() {
            match msg {
                LoopScanCtl::Reset { slot, revision } => {
                    slots[slot.min(1)].reset(revision);
                }
                LoopScanCtl::Sig { slot, revision, position_secs, sig } => {
                    let accum = &mut slots[slot.min(1)];
                    if accum.revision != Some(revision) {
                        accum.reset(Some(revision));
                    }
                    accum.arrivals += 1;
                    if accum.arrivals % accum.accept_mod.max(1) != 0 {
                        continue;
                    }
                    if let Some(last) = accum.last_pos {
                        let delta = position_secs - last;
                        // Wrap/seek jumps are excluded from the cadence
                        // estimate; the cyclic signatures themselves are
                        // exactly what recurrence analysis wants.
                        if delta > 0.0 && delta < 2.0 {
                            accum.delta_sum += delta;
                            accum.delta_count += 1;
                        }
                    }
                    accum.last_pos = Some(position_secs);
                    accum.sigs.push(sig);
                    accum.accepted_since_analysis += 1;
                    if accum.sigs.len() >= crate::loop_detect::MAX_ANALYSIS_FRAMES {
                        // Halve temporal resolution: retained frames are
                        // now spaced twice as far apart, matching the new
                        // acceptance stride, so the cadence stats double.
                        let mut keep = 0;
                        accum.sigs.retain(|_| {
                            keep += 1;
                            keep % 2 == 1
                        });
                        accum.accept_mod = accum.accept_mod.saturating_mul(2);
                        accum.delta_sum *= 2.0;
                    }
                    if accum.accepted_since_analysis >= 24 && accum.sigs.len() >= 48 {
                        accum.accepted_since_analysis = 0;
                        let seconds_per_frame = if accum.delta_count > 0 {
                            accum.delta_sum / accum.delta_count as f64
                        } else {
                            continue;
                        };
                        let detection = analyze_video_loop(&accum.sigs);
                        let report = LoopReport {
                            detection,
                            period_secs: detection.period_frames as f64 * seconds_per_frame,
                        };
                        worker_results.lock().unwrap().push((revision, report));
                    }
                }
            }
        }
    });
    (tx, results)
}

/// Most cached thumbnail textures (revision-keyed, FIFO-evicted).
/// Byte budget for the in-RAM thumbnail cache. Sized so ALL the lanes'
/// thumbs stay warm together — filter flips must be RAM hits, never a
/// redecode (sheet math: a 30-frame animated thumb at 128x80 BGRA is
/// ~1.2 MB; ~120 effects+transitions is ~150 MB, videos on top). Eviction
/// is purely LRU by this budget; nothing about a filter change drops
/// textures by itself.
const THUMB_CACHE_BYTES: usize = 512 * 1024 * 1024;
/// One thumb texture's resident bytes (128x80 BGRA).
const THUMB_TEX_BYTES: usize = 128 * 80 * 4;
/// Thumbnails re-requested per grid rebuild after an eviction. A bank of
/// three thousand tiles must not queue three thousand blob fetches at once.
const MAX_THUMB_REFETCH: usize = 48;

const DEFAULT_LIGHT_MASTER: f32 = 0.26;

/// UI-owned show state. Keeping this as a small Copy value lets pointer
/// actions publish a complete latest-state snapshot to the Art-Net worker;
/// the UI thread never queues scene updates or allocates per video frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LightingControls {
    master: f32,
    black_floor: f32,
    colorfulness: f32,
    response: f32,
    movers: f32,
    rgb: f32,
    strobe: f32,
    laser_level: f32,
    smoke_level: f32,
    uv_level: f32,
    laser_armed: bool,
    smoke_armed: bool,
    uv_armed: bool,
    deadman_held: bool,
    blackout_latched: bool,
}

impl Default for LightingControls {
    fn default() -> Self {
        Self {
            master: DEFAULT_LIGHT_MASTER,
            black_floor: 0.02,
            colorfulness: 1.05,
            response: 0.28,
            movers: 0.0,
            rgb: 0.0,
            strobe: 0.0,
            laser_level: 0.0,
            smoke_level: 0.0,
            uv_level: 0.0,
            laser_armed: false,
            smoke_armed: false,
            uv_armed: false,
            deadman_held: false,
            blackout_latched: false,
        }
    }
}

impl LightingControls {
    fn from_env() -> Self {
        let mut state = Self::default();
        state.master = env_level("VJ_LIGHT_MASTER", state.master, 0.0, 1.0);
        state.black_floor =
            env_level("VJ_LIGHT_BLACK_FLOOR", state.black_floor, 0.0, 0.25);
        state.colorfulness =
            env_level("VJ_LIGHT_COLORFULNESS", state.colorfulness, 0.0, 2.0);
        state.response = env_level("VJ_LIGHT_RESPONSE", state.response, 0.02, 1.0);
        state.movers = env_level("VJ_SHOW_MOVERS", state.movers, 0.0, 1.0);
        state.rgb = env_level("VJ_SHOW_RGB", state.rgb, 0.0, 1.0);
        state.strobe = env_level("VJ_SHOW_STROBE", state.strobe, 0.0, 1.0);
        state.laser_level = env_level("VJ_SHOW_LASER", state.laser_level, 0.0, 1.0);
        state.smoke_level = env_level("VJ_SHOW_SMOKE", state.smoke_level, 0.0, 1.0);
        state.uv_level = env_level("VJ_SHOW_UV", state.uv_level, 0.0, 1.0);
        // Environment variables may preset hazardous levels, but never arm a
        // group or satisfy the live deadman.
        state
    }

    fn disarm_hazards(&mut self) {
        self.laser_armed = false;
        self.smoke_armed = false;
        self.uv_armed = false;
        self.deadman_held = false;
    }

    fn latch_blackout(&mut self) {
        self.blackout_latched = true;
        self.disarm_hazards();
    }

    fn any_hazard_armed(self) -> bool {
        self.laser_armed || self.smoke_armed || self.uv_armed
    }

    fn hazards_live(self) -> bool {
        self.any_hazard_armed() && self.deadman_held && !self.blackout_latched
    }
}

fn parse_level(raw: Option<&str>, fallback: f32, min: f32, max: f32) -> f32 {
    raw.and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(min, max))
        .unwrap_or(fallback)
}

fn env_level(name: &str, fallback: f32, min: f32, max: f32) -> f32 {
    let raw = std::env::var(name).ok();
    parse_level(raw.as_deref(), fallback, min, max)
}

fn tune_light_analyzer(analyzer: &mut VideoLightAnalyzer, controls: LightingControls) {
    let mut config = analyzer.config();
    // Generated VJ clips often contain a bright subject on a dark field.
    // Preserve that contrast instead of pumping every scene toward the same
    // half-bright room exposure; the visible master remains the intensity
    // authority.
    config.exposure_target = 0.34;
    config.max_exposure = 1.9;
    config.max_intensity = 0.50;
    config.black_floor = controls.black_floor;
    config.saturation_boost = controls.colorfulness;
    analyzer.set_config(config);
    analyzer.set_response(controls.response);
}

fn mix_program_lights(
    a: Option<SpatialLightSample>,
    b: Option<SpatialLightSample>,
    mix: f32,
    output_enabled: bool,
) -> SpatialLightSample {
    if output_enabled {
        SpatialLightSample::blend(a, b, mix)
    } else {
        SpatialLightSample::default()
    }
}

fn performance_state_for(
    controls: LightingControls,
    program_running: bool,
) -> PerformanceState {
    let live = program_running && !controls.blackout_latched;
    let level = |value| if live { value } else { 0.0 };
    PerformanceState {
        master: controls.master,
        // The explicit VideoArtNet::blackout/restore latch owns blackout.
        // Transport safety is reversible and is expressed by zeroing every
        // output layer in this latest-state snapshot.
        blackout: false,
        ambilight_level: if live { 1.0 } else { 0.0 },
        rgb: ColorControl { rgb: [1.0; 3], level: level(controls.rgb) },
        strobe: StrobeControl {
            level: level(controls.strobe),
            rate: controls.strobe,
        },
        movers: [
            MoverControl {
                position: [0.5, 0.5],
                rgb: [1.0; 3],
                level: level(controls.movers),
            },
            MoverControl {
                position: [0.5, 0.5],
                rgb: [1.0; 3],
                level: level(controls.movers),
            },
        ],
        hazards: HazardControl {
            rgb_laser: ColorControl {
                rgb: [1.0; 3],
                level: level(controls.laser_level),
            },
            beam_lasers: ColorControl {
                rgb: [1.0; 3],
                level: level(controls.laser_level),
            },
            beam_pattern: 0.5,
            smoke: [level(controls.smoke_level); 2],
            uv: level(controls.uv_level),
        },
    }
}

fn hazard_arms_for(controls: LightingControls, program_running: bool) -> HazardArms {
    if controls.blackout_latched || !program_running {
        return HazardArms::default();
    }
    HazardArms {
        lasers: controls.laser_armed,
        smoke: controls.smoke_armed,
        uv: controls.uv_armed,
    }
}

fn parse_millis(raw: Option<&str>, fallback: Duration, min: u64, max: u64) -> Duration {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .map(|value| Duration::from_millis(value.clamp(min, max)))
        .unwrap_or(fallback)
}

fn env_millis(name: &str, fallback: Duration, min: u64, max: u64) -> Duration {
    let raw = std::env::var(name).ok();
    parse_millis(raw.as_deref(), fallback, min, max)
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    started: bool,
    /// The IMPORT CONTENT panel: its path, its worker and its progress.
    #[rust]
    import: ImportPanel,
    /// Set while a native folder picker is open, so a second click on the
    /// browse button cannot stack two modal dialogs.
    #[rust]
    import_picking: bool,
    #[rust]
    connector: Option<SessionConnector>,
    /// First connection-class failure of the current session, if failures
    /// are ongoing; cleared by any successful round trip.
    #[rust]
    session_loss_since: Option<Instant>,
    #[rust]
    up: Option<SessionHandles>,
    /// One-shot per process: the bundled vjeffect preset library has been
    /// handed to the seeding worker (publish-if-absent into the local
    /// store — see effects/seed.rs). A fresh/empty store gets the full
    /// library as real catalog rows on first connect.
    #[rust]
    fx_presets_seeded: bool,
    /// The GEN drawer's broker chat (shared component; VJ profile).
    #[rust]
    chat: ChatBridge,
    #[rust]
    status_text: String,
    #[rust]
    lighting_status: String,
    #[rust]
    midi_status: String,

    // Shared show-control adapters. MIDI messages are decoded into the same
    // pure cue/deck/pad engines as pointer input; Art-Net only sees a sampled
    // copy of the current program color.
    #[rust]
    midi_input: MidiInput,
    #[rust]
    midi_output: MidiOutput,
    #[rust]
    apc_input_ports: Vec<MidiPortId>,
    #[rust]
    apc_output_ports: Vec<MidiPortId>,
    #[rust]
    apc: Apc40State,
    #[rust]
    apc_leds: LedDiff,
    #[rust]
    apc_sfx_holds: HashMap<usize, AssetId>,
    #[rust]
    lighting: Option<RoomShow>,
    #[rust]
    lighting_retry_at: Option<Instant>,
    #[rust]
    light_analyzers: [VideoLightAnalyzer; 2],
    #[rust]
    light_samples: [Option<SpatialLightSample>; 2],
    #[rust(LightingControls::default())]
    lighting_controls: LightingControls,
    #[rust]
    lighting_controls_loaded: bool,
    #[rust]
    light_track: usize,
    #[rust]
    video_pad_assets: Vec<Option<AssetId>>,
    #[rust]
    video_pad_total: usize,
    /// GEN drawer visibility — HIDDEN by default (the grid earns the
    /// space); the GEN tab reveals it, and the choice persists.
    #[rust]
    gen_panel_open: bool,
    #[rust(300.0)]
    gen_panel_width: f64,

    // Pure engines.
    #[rust]
    thumb_anims: HashMap<AssetRevisionId, (Vec<Texture>, f32)>,
    /// APC40 pad colour (palette velocity) ≈ the thumbnail's colour.
    #[rust]
    thumb_leds: HashMap<AssetRevisionId, u8>,
    #[rust(BrowseModel::visual())]
    video_model: BrowseModel,
    // The deck explorer lists EVERY audio asset in the store, whatever its
    // namespace or category: a generated song is as loadable as an imported
    // one. Only the intermediate-artifact exclusion applies (that is in the
    // query itself). The sfx surface keeps its own narrower model.
    #[rust(BrowseModel::new(AssetKind::Audio, ""))]
    music_model: BrowseModel,
    #[rust(BrowseModel::new(AssetKind::Audio, "sfx"))]
    sfx_model: BrowseModel,
    #[rust(BrowseModel::dance())]
    mesh_model: BrowseModel,
    #[rust(CueEngine::new())]
    cue: CueEngine,
    #[rust(DeckEngine::new())]
    decks: DeckEngine,
    #[rust(PadEngine::new())]
    pads: PadEngine,
    #[rust(GenModel::new())]
    gen: GenModel,
    #[rust]
    gen_panel_loaded: bool,

    // Media lane plans (latest-click-wins with cancel-on-supersede).
    #[rust(LatestWins::video())]
    video_plan: LatestWins,
    #[rust(LatestWins::mesh())]
    mesh_plan: LatestWins,
    /// Sheet + manifest pairing for the cue in flight (sprite actors only).
    #[rust]
    cue_pair: Option<CuePair>,

    // Realtime plumbing.
    #[rust(Mixer::new())]
    mixer: Mixer,
    #[rust(DecodePool::new())]
    decode: DecodePool,
    #[rust]
    audio_installed: bool,

    // Request routing.
    #[rust]
    cat_reqs: HashMap<RequestId, CatPurpose>,
    #[rust]
    media_reqs: HashMap<(usize, RequestId), MediaPurpose>,

    // Caches (keyed by immutable revision).
    #[rust]
    thumbs: HashMap<AssetRevisionId, Texture>,
    /// When each cached thumbnail was last WANTED by a grid rebuild. The
    /// cache used to evict in decode order, which throws out the tiles that
    /// have been on screen longest — the first columns — the moment the
    /// bank scrolls far enough to decode 512 new ones. Least-recently-WANTED
    /// keeps what the operator is looking at.
    #[rust]
    thumb_used: HashMap<AssetRevisionId, u64>,
    #[rust]
    thumb_clock: u64,
    /// Thumbnail fetches in flight, so a re-request cannot pile up.
    #[rust]
    thumb_inflight: HashSet<AssetRevisionId>,
    /// Effect-source fetches in flight for the vjeffect thumbnail renderer
    /// (at most one — the renderer is strictly serial).
    #[rust]
    fx_source_inflight: HashSet<AssetRevisionId>,
    /// Sheet decodes handed to the thumb lane for rendered/cached effect
    /// thumbnails, by submit time — a decode the epoch guard dropped is
    /// simply asked again a few seconds later (the cache file is idempotent).
    #[rust]
    fx_decode_pending: HashMap<AssetRevisionId, f64>,
    /// EFFECT SLOTS (fx_slot.rs): assignment/arm/knob state for the three
    /// mixer slots.
    #[rust]
    fx_slots: FxSlots,
    /// Source fetches in flight for slot loads, per slot (latest wins).
    #[rust]
    fx_slot_inflight: [Option<AssetRevisionId>; 3],
    /// This frame's transition engagement (envelope × triangle) and the
    /// quantized value last pushed to the tile UI.
    #[rust]
    fx_engage_now: f32,
    #[rust]
    fx_engage_synced: f32,
    /// `VJ_TRACE_THUMBS=1` — log every tile texture transition.
    #[rust(std::env::var_os("VJ_TRACE_THUMBS").is_some())]
    trace_thumbs: bool,
    /// `VJ_TRACE_CUE=1` — log the click → cue → fade chain.
    #[rust(std::env::var_os("VJ_TRACE_CUE").is_some())]
    trace_cue: bool,
    /// Visible-range generation. Bumped whenever the bank window moves, and
    /// stamped on every thumbnail decode submitted for that window, so a
    /// fast scroll drops the work it flew past instead of decoding all of
    /// it behind the tiles the operator is actually looking at.
    #[rust]
    view_epoch: u64,
    /// The bank offset the last epoch was taken at.
    #[rust(usize::MAX)]
    view_epoch_bank: usize,
    #[rust]
    pcm_store: HashMap<AssetRevisionId, Arc<TrackPcm>>,

    // Video slots.
    #[rust]
    players: [Option<SlotPlayer>; 2],
    #[rust]
    slot_textures: [Option<Texture>; 2],
    /// The slot texture is a BORROWED thumbnail (cue ack): the first real
    /// frame must REPLACE it with a fresh texture — never set_data into a
    /// texture the grid tiles are drawing.
    #[rust]
    slot_tex_borrowed: [bool; 2],
    /// Parked after a fade: picture kept, clocks stopped (see HoldSlot).
    #[rust]
    slot_held: [bool; 2],
    /// Host clock of the last splat orbit step (seconds since app start).
    #[rust]
    last_splat_pump: Option<f64>,
    /// Operator spin (turntable/orbit) per slot; HOLD overrides it.
    #[rust([true, true])]
    slot_spin: [bool; 2],
    /// What each cue strip currently shows (diffed so the 20 Hz mirror only
    /// evaluates scripts when something changed).
    #[rust]
    strip_shape: [Option<StripShape>; 2],
    #[rust]
    mute_painted: Option<bool>,
    /// Per-slot loop flag (video), and beat-sync length in beats (0 = free).
    #[rust([true, true])]
    slot_loop: [bool; 2],
    /// Ping-pong (forward-backward bounce) per video slot; combined with
    /// slot_loop into the player's PlayMode (ping-pong wins). With flow
    /// warp active the same switch means BOUNCE (triangle-wave position).
    #[rust]
    slot_pingpong: [bool; 2],
    /// FLOW WARP per slot: the load generation the pending/adopted flow
    /// cache belongs to (0 = none), whether a clip is resident in the warp
    /// view, the operator's FLOW toggle (default on — a flow clip warps
    /// unless switched off), and the unquantized bars-fit rate pushed onto
    /// the warp clock every pump (see apply_slot_beat_sync).
    #[rust]
    slot_flow_gen: [u64; 2],
    #[rust]
    slot_flow_avail: [bool; 2],
    #[rust([true, true])]
    slot_flow_on: [bool; 2],
    #[rust([1.0f64, 1.0f64])]
    slot_flow_rate: [f64; 2],
    /// Per-slot video audio mute (a pad loop is a visual). Every cue
    /// RESETS it to muted; the button is the per-clip unmute.
    #[rust([true, true])]
    slot_video_muted: [bool; 2],
    /// A live SCRATCH on a deck's shuttle: the manual transport override
    /// (None = the beat machinery owns the transport).
    #[rust]
    slot_scratch: [Option<f32>; 2],
    /// Per-deck IN/OUT trim (fractions) from the source monitor's range
    /// handles. Session state: reset to (0, 1) on every cue.
    #[rust([(0.0f64, 1.0f64); 2])]
    slot_trim: [(f64, f64); 2],
    /// Bars-fit base for the ♪ chip (1 = one bar; the chip's rate halves
    /// or doubles on top).
    #[rust([1u32, 1u32])]
    slot_sync_beats: [u32; 2],
    /// The beat chip, literally BEATS PER SWEEP: 8 / 4 / 2 / 1.
    #[rust([4.0f32, 4.0f32])]
    slot_beat_rate: [f32; 2],

    /// Cue-well drag in progress: (slot, last pointer position).
    #[rust]
    well_drag: Option<(SlotId, DVec2)>,
    /// THE published beat clock — the disciplined oscillator every consumer
    /// reads, its monotonic time base, and the confidence it inherited from
    /// whatever source it last followed (kept through a coast).
    #[rust]
    beat_clock: BeatClock,
    #[rust]
    clock_epoch: Option<Instant>,
    /// The next beat boundary the pump will pulse the deck transports at.
    #[rust]
    beat_edge: Option<Instant>,
    /// The FREE-RUNNING FLOOR of the clock ladder: there is NO clockless
    /// state. From process start this ticks at 120 BPM; every real source
    /// (tap, typed BPM, deck, detector) steers the ladder above it, and
    /// when the last one drops, the floor coasts at the last known tempo,
    /// phase-continuous. Downstream code may assume a clock exists,
    /// unconditionally.
    #[rust(120.0f64)]
    free_bpm: f64,
    #[rust(Instant::now())]
    free_anchor: Instant,
    #[rust]
    clock_confidence: f32,
    /// Which rung of the source ladder the clock is on.
    #[rust]
    clock_source: ClockSource,
    /// Operator tap tempo, and the clock override its taps produce.
    #[rust]
    tap_tempo: TapTempo,
    #[rust]
    beat_override: Option<BeatOverride>,
    /// App-clock time of the last TAP press, so the button can flash it.
    #[rust]
    tap_flash_secs: Option<f64>,
    /// Scratch for the chrome wave upload, so a pump allocates nothing.
    #[rust]
    wave_cols: Vec<u16>,
    /// Per-slot SYNC: the slot's content is held to the beat grid — video
    /// loops rate-fitted, sprite states stepped on beat subdivisions. ON by
    /// default, because a VJ surface that ignores the music is the odd one.
    #[rust([true, true])]
    slot_beat_sync: [bool; 2],
    /// Hands-free crossfade (the AUTOFADE button).
    #[rust]
    auto_fade: AutoFade,
    /// The completed fade already landed on the mixer.
    #[rust]
    consumed_transition: Option<mixer::VideoTransitionId>,
    /// Last event-driven refresh per surface (see EVENT_REFRESH_COOLDOWN_S).
    #[rust]
    last_event_refresh: [Option<Instant>; 4],
    /// The lane chip the explorer is on (one at a time; ALL default).
    #[rust]
    grid_lane: GridLane,
    /// MIDI-learn state machine + its persisted CC map (midi_learn.rs).
    #[rust]
    midi_learn: MidiLearn,
    /// A tile clicked before its manifest resolved; fires on arrival.
    /// The flag is the click's SHIFT state (explicit effect-as-content).
    #[rust]
    pending_click: Option<(AssetId, bool)>,
    /// The one tile the clip grid marks: the last one clicked (pointer or
    /// APC pad). The grid used to paint LIVE / CUE / held markers on top of
    /// each other, which read as noise; a single green ring on the last
    /// click is the whole of the grid's state now.
    #[rust]
    last_clicked: Option<AssetId>,
    // player_nav: manifest anchors of World assets seen this session
    // (player_start, key_*, exit, door_N…), kept so the walker slot can be
    // handed them when that world is cued. Classic maps published before
    // the anchor lane carry none; bounded, newest wins.
    #[rust]
    world_anchors: HashMap<AssetId, Vec<Anchor>>,
    /// `VJ_TRACE_WTREE=1` — log widget-tree lookup counters every 2 s.
    #[rust(std::env::var_os("VJ_TRACE_WTREE").is_some())]
    trace_wtree: bool,
    #[rust]
    tstats_last: f64,
    #[rust]
    tstats_prev: WidgetTreeStats,
    #[rust]
    awaiting_preroll: [Option<CueGen>; 2],
    /// Settled program mix (0 = slot A on screen, 1 = slot B).
    #[rust]
    program_mix: f32,
    #[rust]
    slot_media: [SlotMedia; 2],
    #[rust]
    billboards: [Option<BillboardSlot>; 2],
    #[rust([16.0 / 9.0, 16.0 / 9.0])]
    slot_aspect: [f32; 2],
    #[rust]
    video_loop: bool,
    #[rust]
    video_muted: bool,
    /// The lower region's tab: false = content grid, true = lights desk.
    #[rust]
    lights_tab: bool,
    /// Master video FADEOUT 0..1 (post-everything dim to black).
    #[rust]
    fadeout: f32,
    #[rust(1.0f32)]
    fade_secs: f32,

    /// MONITOR AUDIO switch: the system-loopback capture runs only while
    /// this is on (persisted; fresh installs OFF — no TCC prompt at boot).
    #[rust]
    monitor_audio: bool,
    /// The loopback devices from the last AudioDevicesEvent — applied or
    /// withheld as the switch flips.
    #[rust]
    loopback_ids: Vec<AudioDeviceId>,
    // Beat-sync: system-audio loopback capture + quantized program fades.
    #[rust(true)]
    external_sync_enabled: bool,
    #[rust]
    capture: Option<Arc<CaptureFeed>>,
    #[rust]
    sync_worker: Option<SyncWorker>,
    #[rust]
    loopback_selected: bool,
    #[rust]
    loopback_failed: bool,
    #[rust]
    capture_frames_seen: u64,
    #[rust]
    capture_progress_at: Option<Instant>,
    #[rust]
    armed_fade: Option<ArmedFadeUi>,

    // Live loop analysis: UI builds bounded frame signatures; a worker
    // accumulates + analyzes; reports are keyed by immutable revision.
    #[rust]
    loop_tx: Option<Sender<LoopScanCtl>>,
    #[rust]
    loop_results: Option<Arc<Mutex<Vec<(AssetRevisionId, LoopReport)>>>>,
    #[rust]
    loop_reports: HashMap<AssetRevisionId, LoopReport>,
    #[rust]
    sig_states: [SigState; 2],
    #[rust]
    slot_scan: [Option<AssetRevisionId>; 2],
    #[rust]
    applied_fit: [Option<BeatFit>; 2],

    // Decks.
    #[rust]
    deck_incoming: HashMap<(usize, u64), (Arc<TrackPcm>, Vec<(f32, f32)>)>,
    #[rust]
    deck_tracks: [Option<(Arc<TrackPcm>, Vec<(f32, f32)>)>; 2],
    #[rust]
    deck_target: DeckTarget,
    #[rust(4.0f32)]
    xfade_secs: f32,

    // Music mode: whole-track analysis off-thread, and the deck surface it
    // feeds (waveform tiles, beat grids, explorer rows, queue).
    #[rust(AnalysisPool::new())]
    analysis: AnalysisPool,
    #[rust]
    deck_analysis: [Option<Arc<TrackAnalysis>>; 2],
    /// The waveform pyramid per deck: one texture holding every zoom level.
    #[rust]
    deck_zoom_tex: [Option<crate::music_view::WavePyramid>; 2],
    /// Source separation, off-thread, per deck.
    #[rust(StemsPool::new())]
    stems: StemsPool,
    /// The other way to get stems: precomputed ones off the store, decoded
    /// on their own worker so a deck load never waits behind a model.
    #[rust(SideChannelPool::new())]
    sidechan: SideChannelPool,
    /// Side-channel blobs in flight per deck. Armed for a generation means
    /// this load fetches its stems and does NOT separate.
    #[rust]
    deck_side_channels: [Option<PendingSideChannels>; 2],
    /// The precomputed files a music revision carries, learned from its
    /// manifest while browsing. Bounded like the anchor map.
    #[rust]
    track_side_channels: HashMap<AssetRevisionId, TrackSideChannels>,
    /// Giving locally computed analysis back to the store, lowest priority
    /// in the app, at most one offer per asset per session.
    #[rust(WriteBackPool::new())]
    writeback: WriteBackPool,
    #[rust]
    writeback_stems: HashSet<AssetId>,
    #[rust]
    writeback_lyrics: HashSet<AssetId>,
    /// One side-channel publication per asset at a time; the next one waits
    /// here so it can build on the revision the first leaves behind.
    #[rust]
    publish_inflight: HashSet<AssetId>,
    #[rust]
    publish_deferred: Vec<(AssetId, Vec<makepad_asset_client::side_channels::SideChannelFile>)>,
    /// Separated audio as it streams in, per deck.
    #[rust]
    deck_stems: [Option<Arc<TrackStems>>; 2],
    /// Per-zoom-column stem energy, and the pyramid built from it.
    #[rust]
    deck_stem_tiles: [Vec<[u8; 4]>; 2],
    #[rust]
    deck_stem_tex: [Option<crate::music_view::WavePyramid>; 2],
    /// What the deck should say about its separation.
    #[rust]
    deck_stem_status: [String; 2],
    /// Whether the separator is still WORKING on this deck (`Some(true)`), has
    /// FAILED on it (`Some(false)`), or has settled (`None`).
    ///
    /// It decides who owns the deck's one status line. A separation that is
    /// running, or one that broke, is a fact with nowhere else to appear, so
    /// it outranks the karaoke bake — which arrives from its own cache within
    /// a second of loading a track that has been transcribed before, and used
    /// to take that line for the rest of the session. Separation going quiet
    /// with the line reading "lyrics: 32 lines in 17s" is exactly what "the
    /// splitting stopped working" looks like from the operator's chair.
    #[rust]
    deck_stem_busy: [Option<bool>; 2],
    /// Karaoke: whisper over the separated vocals, off-thread, once per track.
    #[rust(LyricsPool::new())]
    lyrics: LyricsPool,
    /// One read probe and one bake per track digest, however many times a
    /// deck is loaded or a separation run reports its coverage.
    #[rust]
    lyrics_dispatch: LyricsDispatch,
    /// Transcripts already in hand this session, by track digest. The
    /// dispatch gate above refuses a second job for the same track — so a
    /// reload, or the other deck playing the same track, re-hangs the words
    /// from HERE instead of sitting on "waiting for separation" forever.
    #[rust]
    lyrics_by_digest: HashMap<String, Arc<TrackLyrics>>,
    /// The digest under each deck's current track, learned from the
    /// separation worker's coverage report (the only place it exists).
    #[rust]
    deck_track_digest: [Option<String>; 2],
    /// The transcript per deck, and the beat-quantized display schedule built
    /// from it plus that deck's grid. The schedule is rebuilt whenever either
    /// half arrives, because they land in either order.
    #[rust]
    deck_lyrics: [Option<Arc<TrackLyrics>>; 2],
    #[rust]
    deck_karaoke: [Option<Arc<KaraokeSchedule>>; 2],
    #[rust]
    deck_lyrics_status: [String; 2],
    /// Karaoke subtitles on the program output.
    #[rust]
    karaoke_on: bool,
    /// Rows on screen, so a row click maps back to a track.
    #[rust]
    music_rows: Vec<TrackRowEntry>,
    #[rust]
    queue_rows: Vec<TrackRowEntry>,
    /// Browsing local audio files instead of the store catalog.
    #[rust]
    music_local: bool,
    #[rust]
    local_tracks: Vec<PathBuf>,
    /// Local files a deck was pointed at, keyed by their synthetic asset id.
    #[rust]
    local_by_asset: HashMap<AssetId, PathBuf>,
    /// Loop length in beats, per deck, for the halve/double buttons.
    #[rust([4u32, 4u32])]
    deck_loop_beats: [u32; 2],
    /// Last lit/unlit state pushed into each chrome button.
    #[rust]
    lit_state: HashMap<u64, bool>,
    /// Last string pushed into each status label. The status panel is
    /// mirrored on a 20 Hz pump and most of it never changes between
    /// ticks; `set_text` on an unchanged string still costs a widget
    /// lookup, a redraw flag and a text relayout.
    #[rust]
    label_state: HashMap<u64, String>,
    /// Last live/killed state painted onto each stem knob.
    #[rust]
    stem_paint: HashMap<u64, bool>,
    /// The deck surface's widgets, resolved once.
    #[rust]
    music_refs: MusicRefs,
    #[rust]
    music_refs_ready: bool,
    /// Display-cadence pump for the deck surface. The wave view's own
    /// `NextFrame` never comes back (measured: zero ticks a second), so the
    /// app drives it the same way it drives video frames.
    #[rust]
    music_pump: NextFrame,
    /// Last text pushed into each label, so an unchanged readout costs
    /// nothing: a `set_text` re-formats and re-lays-out every call.
    #[rust]
    label_cache: HashMap<u64, String>,

    // SFX.
    #[rust]
    selected_pad: Option<AssetId>,

    // Mesh program.
    #[rust]
    mesh_gen: u64,
    #[rust]
    mesh_now: String,

    // Generate surface UI mirrors.
    #[rust]
    gen_profile_labels: Vec<String>,

    // The native output surface can be destroyed independently of the main
    // console. Keep the widget/pass alive and recreate that native surface on
    // demand instead of leaving its WindowHandle permanently closed.
    #[rust(OutputWindowLifecycle::default())]
    output_window_lifecycle: OutputWindowLifecycle,
    /// One-shot: close the output window on the first pump tick unless
    /// `VJ_OUTPUT=1` (default-closed is cleaner while testing).
    #[rust(true)]
    output_close_on_start: bool,
    /// Which console page and which output page are up. A walked level
    /// raycasts its collision mesh sixty times a second and re-renders a
    /// full pass; `sync_mesh_liveness` uses these to stop that for a picture
    /// no surface is showing.
    #[rust(live_id!(video_page))]
    console_page: LiveId,
    #[rust(live_id!(video_out_page))]
    out_page: LiveId,

    // Grid rebuild flag + timers.
    #[rust]
    grids_dirty: bool,
    #[rust]
    poll_timer: Timer,
    #[rust]
    refresh_timer: Timer,
    /// Pad-filter text waiting for its idle debounce before it becomes a
    /// catalog query.
    #[rust]
    pending_filter: Option<String>,
    #[rust]
    filter_timer: Timer,
    /// Search-row input (music / sfx / mesh, text or category box) waiting
    /// for the same idle debounce — typing queries the catalog, no button
    /// press needed.
    #[rust]
    pending_search: Option<(Surface, SearchBox, String)>,
    #[rust]
    search_timer: Timer,
    #[rust]
    video_pump: NextFrame,
    /// Finished decodes the operator is WAITING for (the clicked cue, a
    /// deck, a pad): always handled in the tick they arrive.
    #[rust]
    decode_ready: VecDeque<DecodeDone>,
    /// Finished THUMBNAILS: decoration, handled only while the frame budget
    /// lasts and continued next frame. A screenful arriving at once used to
    /// spend 20ms of every 50ms tick uploading them.
    #[rust]
    decode_backlog: VecDeque<DecodeDone>,
    /// Armed while `decode_backlog` still holds anything, so the rest of a
    /// thumbnail burst lands at frame rate instead of waiting for the 20Hz
    /// poll timer.
    #[rust]
    decode_pump: NextFrame,

    /// The Asset Server this process is HOSTING, when it is hosting one.
    ///
    /// LAST FIELD ON PURPOSE. Rust drops struct fields in declaration order,
    /// so everything that might still be talking to the store — the session
    /// handles, the decode pool, the import worker — is torn down before the
    /// server it was talking to. `None` when attached to somebody else's
    /// store, which is the same code path either way.
    #[rust]
    local_store: Option<LocalStore>,
}

/// Minimum spacing between catalog refreshes triggered by publish events.
const EVENT_REFRESH_COOLDOWN_S: f64 = 3.0;

/// Idle time after the last keystroke before the pad filter re-queries the
/// server. Short enough to feel live, long enough not to search per key.
const FILTER_DEBOUNCE_S: f64 = 0.3;

/// Which box of a search row a debounced keystroke belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchBox {
    Text,
    Category,
}

impl App {
    fn model(&mut self, surface: Surface) -> &mut BrowseModel {
        match surface {
            Surface::Video => &mut self.video_model,
            Surface::Music => &mut self.music_model,
            Surface::Sfx => &mut self.sfx_model,
            Surface::Mesh => &mut self.mesh_model,
        }
    }

    fn slot_mesh_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(slot_mesh_a),
            SlotId::B => ids!(slot_mesh_b),
        }
    }

    fn slot_splat_scene_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(slot_splat_a),
            SlotId::B => ids!(slot_splat_b),
        }
    }

    fn slot_splat_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(slot_splat_a.splat),
            SlotId::B => ids!(slot_splat_b.splat),
        }
    }

    /// Point the slot's splat view at a verified-cache PLY; the view decodes
    /// it on its next draw. `ViewSplat::is_scene_ready` flips when the GPU
    /// scene exists — pump_video turns that into the preroll confirmation.
    fn open_slot_splat(&mut self, cx: &mut Cx, slot: SlotId, path: &std::path::Path) {
        let abs = path.to_string_lossy().to_string();
        let mut splat = self.ui.widget(cx, Self::slot_splat_path(slot));
        script_apply_eval!(cx, splat, {
            src: mod.res.file_resource(#(abs))
        });
        let scene = self.ui.widget(cx, Self::slot_splat_scene_path(slot));
        if let Some(mut view) = scene.borrow_mut::<makepad_xr::scene::XrSceneView>() {
            let camera = view.camera_mut();
            camera.distance = 3.2;
            camera.distance_min = 0.5;
            camera.orbit_yaw = 0.4;
            camera.orbit_pitch = -0.15;
        }
        splat.redraw(cx);
        scene.redraw(cx);
    }

    fn slot_splat_ready(&self, cx: &mut Cx, slot: SlotId) -> bool {
        let widget = self.ui.widget(cx, Self::slot_splat_path(slot));
        widget
            .borrow::<makepad_xr::obj::ViewSplat>()
            .is_some_and(|view| view.is_scene_ready())
    }

    fn slot_splat_source(&self, cx: &mut Cx, slot: SlotId) -> Option<(Texture, f32)> {
        if self.slot_media[slot.index()] != SlotMedia::Splat || !self.slot_splat_ready(cx, slot) {
            return None;
        }
        let widget = self.ui.widget(cx, Self::slot_splat_scene_path(slot));
        let view = widget.borrow::<makepad_xr::scene::XrSceneView>()?;
        Some((view.color_texture().clone(), 16.0 / 9.0))
    }

    /// Advance a live splat slot's orbit and re-render its scene.
    fn pump_splat(&mut self, cx: &mut Cx, slot: SlotId, dt: f32) {
        if self.slot_media[slot.index()] != SlotMedia::Splat {
            return;
        }
        let scene = self.ui.widget(cx, Self::slot_splat_scene_path(slot));
        if let Some(mut view) = scene.borrow_mut::<makepad_xr::scene::XrSceneView>() {
            // ROTATE is the operator's switch and the only thing that gates
            // the orbit — a parked (HOLD) splat keeps turning if they left
            // it on, exactly like a parked mesh does. Anything else makes
            // the lit ROTATE latch lie about what the well is doing.
            if self.slot_spin[slot.index()] {
                view.camera_mut().orbit_yaw += SPLAT_ORBIT_RATE * dt;
            }
        }
        scene.redraw(cx);
    }

    fn clear_slot_mesh(&mut self, cx: &mut Cx, slot: SlotId) {
        let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
        if let Some(mut view) = widget.borrow_mut::<mesh_view::VjMeshView>() {
            view.clear(cx);
        };
    }

    fn set_slot_mesh_paused(&mut self, cx: &mut Cx, slot: SlotId, paused: bool) {
        let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
        if let Some(mut view) = widget.borrow_mut::<mesh_view::VjMeshView>() {
            view.set_paused(paused);
        };
    }

    /// Which 3D views are worth simulating. A walked level raycasts its
    /// collision mesh sixty times a second and re-renders a full offscreen
    /// pass per frame; none of that may run for a picture no surface is
    /// showing — while the DJ page is up with the fader parked on the other
    /// slot, that is pure heat under the deck.
    ///
    /// A slot's mesh reaches the eye through the program (output window) or
    /// through the cue wells and preview on the video page, so it is live
    /// while either shows it. `mesh_program` is the output window's own mesh
    /// page. Nothing is forgotten when a view goes dormant: the tour keeps
    /// its position and its map memory and resumes on the next frame.
    fn sync_mesh_liveness(&mut self, cx: &mut Cx) {
        let output_up = self.output_window_lifecycle == OutputWindowLifecycle::Open;
        let video_front = self.console_page == live_id!(video_page);
        let program_up = output_up && self.out_page == live_id!(video_out_page);
        let mix = self.live_program_mix();
        for slot in [SlotId::A, SlotId::B] {
            // A plain crossfade: each side carries its fader weight. (Doc
            // transitions replace the old resident-B mix modes.)
            let weight = match slot {
                SlotId::A => 1.0 - mix,
                SlotId::B => mix,
            };
            let live = self.slot_media[slot.index()] == SlotMedia::Mesh
                && (video_front || (program_up && weight > 0.002));
            let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
            let view = widget.borrow_mut::<mesh_view::VjMeshView>();
            if let Some(mut view) = view {
                view.set_live(cx, live);
            }
        }
        let live = output_up && self.out_page == live_id!(mesh_out_page);
        let widget = self.ui.widget(cx, ids!(mesh_program));
        let view = widget.borrow_mut::<mesh_view::VjMeshView>();
        if let Some(mut view) = view {
            view.set_live(cx, live);
        }
    }

    /// `world` = a walkable level (catalog kind `World` with a GLB): shown
    /// at authored scale and toured by the NPC walker, not shrunk onto a
    /// turntable. Splat worlds never reach here (they are PLY).
    /// The body a world asset gets walked with: the engine family from its
    /// alias, put into that map's own units by the anchors the importer
    /// published (`step_height` / `eye_height`).
    ///
    /// One place, called BEFORE the prep job, because the nav grid and the
    /// walker must be the same body — a graph probed with a Doom-scale body
    /// and walked by a Quake 1 one offers steps the legs then refuse.
    fn world_walker_config(&self, asset: AssetId) -> makepad_render::level::WalkerConfig {
        let source = self
            .video_model
            .tile(&asset)
            .and_then(|tile| tile.alias.clone())
            .unwrap_or_default();
        let anchors: Vec<makepad_render::player_nav::NavAnchor> = self
            .world_anchors
            .get(&asset)
            .map(|list| list.iter().map(nav_anchor).collect())
            .unwrap_or_default();
        makepad_render::player_nav::config_for_world(
            makepad_render::level::BobStyle::from_source(&source),
            &anchors,
        )
    }

    fn apply_slot_mesh(
        &mut self,
        cx: &mut Cx,
        slot: SlotId,
        prepared: Box<media::PreparedMesh>,
        world: bool,
        source: &str,
        anchors: Vec<makepad_render::player_nav::NavAnchor>,
    ) {
        let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
        if let Some(mut view) = widget.borrow_mut::<mesh_view::VjMeshView>() {
            view.set_world_mode(world, source);
            // player_nav: manifest anchors for the player planner.
            view.set_world_anchors(anchors);
            view.set_prepared(cx, prepared);
        };
    }

    fn slot_mesh_source(&self, cx: &mut Cx, slot: SlotId) -> Option<(Texture, f32)> {
        let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
        let view = widget.borrow::<mesh_view::VjMeshView>()?;
        if !view.has_mesh() {
            return None;
        }
        Some((view.color_texture(), 16.0 / 9.0))
    }

    /// The video master dim. The readout burns amber the moment it pulls
    /// the picture down — a half-faded program must never be a mystery.
    fn set_fadeout(&mut self, cx: &mut Cx, value: f32) {
        self.fadeout = value.clamp(0.0, 1.0);
        // The chip carries its own live readout.
        self.set_drop_slider(cx, ids!(fadeout_knob), self.fadeout as f64);
        self.video_pump = cx.new_next_frame();
    }

    /// The lower region: content grid or the lights desk — one visible at
    /// a time, chosen by the vertical tab pair (persisted).
    fn set_lower_tab(&mut self, cx: &mut Cx, lights: bool) {
        self.lights_tab = lights;
        let page = if lights {
            id!(lights_lower_page)
        } else {
            id!(grid_lower_page)
        };
        self.ui
            .page_flip(cx, ids!(lower_pages))
            .set_active_page(cx, page.into());
        self.paint_icon_button(cx, ids!(lower_tab_vj), !lights);
        self.paint_icon_button(cx, ids!(lower_tab_lights), lights);
        self.ui.redraw(cx);
    }

    // ---- DECK SOURCE MONITORS ----------------------------------------------

    fn deck_source_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(deck_a_source),
            SlotId::B => ids!(deck_b_source),
        }
    }

    fn deck_mute_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(deck_a_mute),
            SlotId::B => ids!(deck_b_mute),
        }
    }

    fn deck_play_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(deck_a_play),
            SlotId::B => ids!(deck_b_play),
        }
    }

    fn deck_rw_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(deck_a_rw),
            SlotId::B => ids!(deck_b_rw),
        }
    }

    fn deck_loop2_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(deck_a_loop2),
            SlotId::B => ids!(deck_b_loop2),
        }
    }

    fn deck_rate_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(deck_a_rate),
            SlotId::B => ids!(deck_b_rate),
        }
    }

    fn deck_bounce_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(deck_a_bounce),
            SlotId::B => ids!(deck_b_bounce),
        }
    }

    fn deck_controls_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(deck_a_controls),
            SlotId::B => ids!(deck_b_controls),
        }
    }

    fn deck_eject_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(deck_a_eject),
            SlotId::B => ids!(deck_b_eject),
        }
    }

    /// Feed each deck's SOURCE mini-monitor: the raw pre-effect picture and
    /// the honest transport (flow-warp's clock wins while it drives the
    /// picture — the decoder underneath is parked then).
    fn pump_source_monitors(
        &mut self,
        cx: &mut Cx,
        a: &Option<(Texture, f32)>,
        b: &Option<(Texture, f32)>,
    ) {
        for (slot, tex) in [(SlotId::A, a), (SlotId::B, b)] {
            let i = slot.index();
            let (playing, pos, dur) = match self.players[i].as_ref() {
                Some(p) => (!p.is_paused(), p.position_secs(), p.duration_secs),
                None => (false, 0.0, 0.0),
            };
            let (playing, pos) = if self.flow_active(i) {
                self.flow_view(cx, slot, |_cx, view| {
                    (view.is_playing(), view.position_secs())
                })
                .unwrap_or((playing, pos))
            } else {
                (playing, pos)
            };
            let fraction = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 };
            let position = if dur > 0.0 {
                format!("{}:{:04.1}", (pos / 60.0) as u32, pos % 60.0)
            } else {
                "—".to_string()
            };
            let looping = self.slot_loop[i];
            let trim = self.slot_trim[i];
            let widget = self.ui.widget(cx, Self::deck_source_path(slot));
            if let Some(mut mini) = widget.borrow_mut::<VideoView>() {
                mini.set_frame(cx, tex.as_ref().map(|(t, _)| t.clone()));
                mini.set_transport(cx, fraction, playing, &position);
                if mini.looping() != looping {
                    mini.set_looping(cx, looping);
                }
                // Push the HOST's trim only while no finger holds a notch
                // (a cue reset lands; a drag in flight is never fought).
                let mt = mini.trim();
                if !mini.is_trim_dragging()
                    && ((mt.0 - trim.0).abs() > 1e-4 || (mt.1 - trim.1).abs() > 1e-4)
                {
                    mini.set_trim(cx, trim.0, trim.1);
                }
            };
        }
    }

    // ---- MIDI LEARN (midi_learn.rs) -----------------------------------------

    fn midi_map_path() -> PathBuf {
        service::session_config_from_env().cache_parent.join("midi-map.txt")
    }

    fn save_midi_map(&self) {
        let path = Self::midi_map_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.midi_learn.encode());
    }

    fn load_midi_map(&mut self) {
        if let Ok(body) = std::fs::read_to_string(Self::midi_map_path()) {
            self.midi_learn = MidiLearn::decode(&body);
        }
    }

    /// Mirror the learn state onto every wrapper: pick-mode hint outlines,
    /// the armed pulse, the mapped ticks, and the LEARN button's latch.
    fn sync_midi_learn_ui(&mut self, cx: &mut Cx) {
        for (path, control) in LEARNABLES {
            let mode = if self.midi_learn.is_armed(control) {
                2
            } else if self.midi_learn.picking {
                1
            } else {
                0
            };
            let mapped = self.midi_learn.is_bound(control);
            let widget = self.ui.widget(cx, path);
            if let Some(mut wrap) = widget.borrow_mut::<VjLearnWrap>() {
                wrap.set_learn_state(cx, mode, mapped);
            };
        }
        self.paint_icon_button(cx, ids!(midi_learn_btn), self.midi_learn.active());
    }

    /// A learned CC's value lands on its control — the same state changes
    /// the pointer path makes, plus the widget mirror.
    fn apply_learned(&mut self, cx: &mut Cx, control: &str, v: f32) {
        match control {
            "video_fade" => {
                let secs = 0.05 + v * (5.0 - 0.05);
                self.fade_secs = secs;
                self.ui.slider(cx, ids!(video_fade)).set_value(cx, secs as f64);
                self.ui
                    .label(cx, ids!(fade_value))
                    .set_text(cx, &format!("{secs:.1}s"));
            }
            "xfader" => {
                // The hand always wins — a mapped fader IS the hand.
                self.auto_fade.cancel();
                self.sync_autofade_ui(cx);
                self.set_visual_mix(cx, v);
            }
            "fadeout" => {
                self.set_fadeout(cx, v);
            }
            "master" => {
                let value = v * 1.2;
                self.mixer.set_master(value);
                self.set_drop_slider(cx, ids!(master_slider), value as f64);
            }
            _ => {
                let Some(rest) = control.strip_prefix("fx_") else { return };
                let (kind, knob) = match rest.split_once('_') {
                    Some(("a", knob)) => (FxSlotKind::EffectA, knob),
                    Some(("t", knob)) => (FxSlotKind::Transition, knob),
                    Some(("b", knob)) => (FxSlotKind::EffectB, knob),
                    _ => return,
                };
                let index = match knob {
                    "spd" => 0,
                    "d0" => 1,
                    "d1" => 2,
                    "d2" => 3,
                    _ => return,
                };
                match index {
                    0 => self.fx_slots.slot_mut(kind).speed = v,
                    i => self.fx_slots.slot_mut(kind).p[i - 1] = Some(v),
                }
                self.save_fx_slots();
                self.ui
                    .slider(cx, Self::fx_slot_knob_path(kind, index))
                    .set_value(cx, v as f64);
            }
        }
        self.video_pump = cx.new_next_frame();
    }

    // ---- EFFECT SLOTS (fx_slot.rs) ------------------------------------------
    //
    // The vjeffect category's runtime: three offscreen hosts in the status
    // bar, three tiles + control strips in the mixer column, and the
    // program seam in `apply_fx_slots`.

    fn fx_slot_host_path(kind: FxSlotKind) -> &'static [LiveId] {
        match kind {
            FxSlotKind::EffectA => ids!(fx_host_a),
            FxSlotKind::Transition => ids!(fx_host_t),
            FxSlotKind::EffectB => ids!(fx_host_b),
        }
    }

    /// The CONTENT-mode host: an FX tile cued onto the deck (no slot armed)
    /// renders here and feeds `slot_textures`-level compositing like a clip.
    fn fx_content_host_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(fx_content_a),
            SlotId::B => ids!(fx_content_b),
        }
    }

    fn clear_slot_fx_content(&mut self, cx: &mut Cx, slot: SlotId) {
        let widget = self.ui.widget(cx, Self::fx_content_host_path(slot));
        if let Some(mut host) = widget.borrow_mut::<VjFxSlotHost>() {
            host.set_enabled(cx, false);
            host.clear(cx);
        };
    }

    /// The deck's picture when its content IS an effect. Ungated by run
    /// state so a HELD slot keeps its frozen last frame, like a parked clip.
    fn slot_fx_content_source(&self, cx: &mut Cx, slot: SlotId) -> Option<(Texture, f32)> {
        if self.slot_media[slot.index()] != SlotMedia::Effect {
            return None;
        }
        let widget = self.ui.widget(cx, Self::fx_content_host_path(slot));
        let host = widget.borrow::<VjFxSlotHost>()?;
        host.preview_output().map(|tex| (tex, 16.0 / 9.0))
    }

    /// Clock + run-state for the content-mode hosts: live while their slot's
    /// content is an effect and the slot is not parked.
    fn pump_fx_content(&mut self, cx: &mut Cx) {
        let beat = self
            .clock_secs(Instant::now())
            .filter(|_| self.beat_clock.running())
            .map(|secs| (self.beat_clock.position_at(secs), self.beat_clock.bpm()))
            .filter(|(_, bpm)| *bpm > 0.0);
        for slot in [SlotId::A, SlotId::B] {
            // Standby decks stay HOT (the standby law): held only means
            // off-program, never paused.
            let active = self.slot_media[slot.index()] == SlotMedia::Effect;
            let widget = self.ui.widget(cx, Self::fx_content_host_path(slot));
            let Some(mut host) = widget.borrow_mut::<VjFxSlotHost>() else { continue };
            let run = active && host.has_effect();
            host.set_enabled(cx, run);
            if active {
                if let Some((pos, bpm)) = beat {
                    host.set_beat(pos, bpm);
                }
            }
        }
    }

    fn fx_slot_tile_path(kind: FxSlotKind) -> &'static [LiveId] {
        match kind {
            FxSlotKind::EffectA => ids!(fx_slot_a_tile),
            FxSlotKind::Transition => ids!(fx_slot_t_tile),
            FxSlotKind::EffectB => ids!(fx_slot_b_tile),
        }
    }

    fn fx_slot_on_path(kind: FxSlotKind) -> &'static [LiveId] {
        match kind {
            FxSlotKind::EffectA => ids!(fx_slot_a_on),
            FxSlotKind::Transition => ids!(fx_slot_t_on),
            FxSlotKind::EffectB => ids!(fx_slot_b_on),
        }
    }

    fn fx_slot_clear_path(kind: FxSlotKind) -> &'static [LiveId] {
        match kind {
            FxSlotKind::EffectA => ids!(fx_slot_a_clear),
            FxSlotKind::Transition => ids!(fx_slot_t_clear),
            FxSlotKind::EffectB => ids!(fx_slot_b_clear),
        }
    }

    fn fx_slot_dial_lab_path(kind: FxSlotKind, dial: usize) -> &'static [LiveId] {
        match (kind, dial) {
            (FxSlotKind::EffectA, 0) => ids!(fx_slot_a_d0_lab),
            (FxSlotKind::EffectA, 1) => ids!(fx_slot_a_d1_lab),
            (FxSlotKind::EffectA, _) => ids!(fx_slot_a_d2_lab),
            (FxSlotKind::Transition, 0) => ids!(fx_slot_t_d0_lab),
            (FxSlotKind::Transition, 1) => ids!(fx_slot_t_d1_lab),
            (FxSlotKind::Transition, _) => ids!(fx_slot_t_d2_lab),
            (FxSlotKind::EffectB, 0) => ids!(fx_slot_b_d0_lab),
            (FxSlotKind::EffectB, 1) => ids!(fx_slot_b_d1_lab),
            (FxSlotKind::EffectB, _) => ids!(fx_slot_b_d2_lab),
        }
    }

    /// Knob 0 = SPD, 1..=3 = the THREE fixed dials (user params p0..p2).
    fn fx_slot_knob_path(kind: FxSlotKind, knob: usize) -> &'static [LiveId] {
        match (kind, knob) {
            (FxSlotKind::EffectA, 0) => ids!(fx_slot_a_spd),
            (FxSlotKind::EffectA, 1) => ids!(fx_slot_a_d0),
            (FxSlotKind::EffectA, 2) => ids!(fx_slot_a_d1),
            (FxSlotKind::EffectA, _) => ids!(fx_slot_a_d2),
            (FxSlotKind::Transition, 0) => ids!(fx_slot_t_spd),
            (FxSlotKind::Transition, 1) => ids!(fx_slot_t_d0),
            (FxSlotKind::Transition, 2) => ids!(fx_slot_t_d1),
            (FxSlotKind::Transition, _) => ids!(fx_slot_t_d2),
            (FxSlotKind::EffectB, 0) => ids!(fx_slot_b_spd),
            (FxSlotKind::EffectB, 1) => ids!(fx_slot_b_d0),
            (FxSlotKind::EffectB, 2) => ids!(fx_slot_b_d1),
            (FxSlotKind::EffectB, _) => ids!(fx_slot_b_d2),
        }
    }

    /// An FX tile was clicked with slot `kind` armed: fetch its splash
    /// source and load it into that slot.
    fn fx_effect_tile_clicked(&mut self, cx: &mut Cx, kind: FxSlotKind, asset: AssetId) {
        let Some(tile) = self.video_model.tile(&asset) else { return };
        let (Some(revision), Some(media)) = (tile.revision, tile.media.clone()) else {
            // Manifest still resolving: the click fires the moment it lands
            // (the same latch the cue path uses).
            self.pending_click = Some((asset, false));
            let cmds = self.video_model.resolve_first(asset);
            self.run_cat_cmds(Surface::Video, cmds);
            self.grids_dirty = true;
            return;
        };
        self.pending_click = None;
        if media.media != MediaType::Text || media.len > media::MAX_THUMB_BYTES {
            log!("fx slot: {} has no loadable effect document", tile.title);
            self.refuse_fx_slot(cx, kind, "no FX doc");
            return;
        }
        let title = tile.title.clone();
        let Some(up) = self.up.as_mut() else { return };
        if let Ok(id) = up.catalog.submit_with(
            ClientRequest::FetchBlob {
                blob: media.blob,
                expected_len: Some(media.len),
                pin: false,
            },
            makepad_asset_client::SubmitOptions::newest_first(),
        ) {
            self.cat_reqs
                .insert(id, CatPurpose::FxSlotSource { slot: kind, revision, title });
            self.fx_slot_inflight[kind.index()] = Some(revision);
            self.fx_slots.slot_mut(kind).note = Some("loading…".to_string());
            // ONE-SHOT ARM: the accepted click consumes it. A latched arm
            // silently owning every later effect click was the "auto-drop
            // into the channel stopped working" wedge — armed once, loaded
            // once, and the arm kept eating (or type-refusing) clicks.
            self.fx_slots.consume_armed(kind);
            self.sync_fx_slots_ui(cx);
            self.grids_dirty = true;
        }
    }

    /// Load splash text into a slot's offscreen host and take the name onto
    /// the tile. `persist` is off during the startup restore. `revision`
    /// keys the tile's identity thumbnail.
    fn load_fx_slot(
        &mut self,
        cx: &mut Cx,
        kind: FxSlotKind,
        title: &str,
        revision: Option<AssetRevisionId>,
        source: &str,
        persist: bool,
    ) {
        let widget = self.ui.widget(cx, Self::fx_slot_host_path(kind));
        let result = widget
            .borrow_mut::<VjFxSlotHost>()
            .map(|mut host| host.load(cx, &format!("vjfx_slot_{}", kind.key()), source));
        match result {
            Some(Ok(name)) => {
                let title = if title.is_empty() { name } else { title.to_string() };
                self.fx_slots
                    .loaded(kind, title, revision.map(|r| r.to_string()));
                // `loaded` reset every dial to UNTOUCHED; now layer THIS
                // effect's own sticky profile (if the operator ever moved
                // its dials) — never the previous occupant's values.
                self.apply_fx_profile(kind);
                // The knob strip re-describes itself from the new doc's
                // dial declarations.
                self.sync_fx_slot_knobs(cx, kind);
                if persist {
                    self.save_fx_slot_source(kind, source);
                    self.save_fx_slots();
                    // AUTOFADE: the effect landed on the most-faded-out
                    // side — sweep the program over to it, exactly the
                    // fade a content cue gets. (Restores never sweep; the
                    // toggle off means the operator rides the fader.)
                    let target_is_b = kind == FxSlotKind::EffectB;
                    let heading_to_b = self.program_mix < 0.5;
                    if kind != FxSlotKind::Transition
                        && self.fx_slots.click_autofade
                        && heading_to_b == target_is_b
                    {
                        let secs = self
                            .ui
                            .slider(cx, ids!(video_fade))
                            .value()
                            .unwrap_or(1.0) as f32;
                        self.auto_fade.press(self.program_mix, secs);
                        self.sync_autofade_ui(cx);
                    }
                }
            }
            Some(Err(error)) => {
                log!("fx slot {kind:?}: load failed — {error}");
                self.fx_slots.slot_mut(kind).note = Some("load failed".to_string());
            }
            None => {}
        }
        self.sync_fx_slots_ui(cx);
        self.video_pump = cx.new_next_frame();
    }

    fn clear_fx_slot(&mut self, cx: &mut Cx, kind: FxSlotKind) {
        let widget = self.ui.widget(cx, Self::fx_slot_host_path(kind));
        if let Some(mut host) = widget.borrow_mut::<VjFxSlotHost>() {
            host.set_enabled(cx, false);
            host.clear(cx);
        }
        self.fx_slots.clear(kind);
        let _ = std::fs::remove_file(Self::fx_slot_source_path(kind));
        self.save_fx_slots();
        self.sync_fx_slot_knobs(cx, kind);
        self.sync_fx_slots_ui(cx);
        self.video_pump = cx.new_next_frame();
    }

    /// Mirror the model onto the three tiles + ON buttons. Equality-gated
    /// inside the widgets, so calling this on every state change is cheap.
    fn sync_fx_slots_ui(&mut self, cx: &mut Cx) {
        for kind in FxSlotKind::ALL {
            let slot = self.fx_slots.slot(kind).clone();
            let armed = self.fx_slots.armed == Some(kind);
            let engage = if kind == FxSlotKind::Transition {
                self.fx_engage_synced
            } else {
                0.0
            };
            let flash = slot.flash.as_ref().map(|(msg, _)| msg.clone());
            let note = if let Some(flash) = flash.clone() {
                flash
            } else if let Some(note) = &slot.note {
                note.clone()
            } else if slot.title.is_none() {
                if armed { "pick an FX tile".to_string() } else { String::new() }
            } else if slot.bypass {
                "BYP".to_string()
            } else if kind == FxSlotKind::Transition {
                if engage > 0.05 { "FADE".to_string() } else { "on fade".to_string() }
            } else {
                String::new()
            };
            let widget = self.ui.widget(cx, Self::fx_slot_tile_path(kind));
            if let Some(mut tile) = widget.borrow_mut::<VjFxSlotTile>() {
                tile.set_labels(kind.tag(), kind.hint());
                tile.set_state(
                    cx,
                    FxSlotTileState {
                        title: slot.title.clone(),
                        note,
                        armed,
                        bypass: slot.bypass,
                        engage,
                        flash: flash.is_some(),
                    },
                );
            }
            self.paint_lit(cx, Self::fx_slot_on_path(kind), slot.running());
        }
    }

    /// Mirror the LOADED DOC's dial declarations onto the FIXED knob strip:
    /// three dials, one per user param p0..p2, always present so MIDI maps
    /// stay stable across effect swaps. A declared dial gets its real
    /// legend and doc default; an undeclared one dims to inert with a blank
    /// legend. Runs on load, restore and clear — never while a hand is on a
    /// knob.
    fn sync_fx_slot_knobs(&mut self, cx: &mut Cx, kind: FxSlotKind) {
        let slot = self.fx_slots.slot(kind).clone();
        let dials = self
            .ui
            .widget(cx, Self::fx_slot_host_path(kind))
            .borrow::<VjFxSlotHost>()
            .map(|host| host.dials())
            .unwrap_or_default();
        let loaded = slot.title.is_some();
        self.ui
            .slider(cx, Self::fx_slot_knob_path(kind, 0))
            .set_value(cx, slot.speed as f64);
        for i in 0..3 {
            // The dial for p_i, whatever position the doc declared it in.
            let dial = dials.iter().find(|d| d.index == i).filter(|_| loaded);
            let label = match dial {
                Some(dial) => {
                    let mut label = dial.label.clone();
                    label.truncate(6);
                    label
                }
                None => "—".to_string(),
            };
            self.ui
                .label(cx, Self::fx_slot_dial_lab_path(kind, i))
                .set_text(cx, &label);
            let knob = self.ui.slider(cx, Self::fx_slot_knob_path(kind, i + 1));
            knob.set_value(
                cx,
                slot.p[i]
                    .unwrap_or_else(|| dial.map(|d| d.default).unwrap_or(0.0)) as f64,
            );
            let inert = if dial.is_some() { 0.0f64 } else { 1.0 };
            let mut knob = knob;
            script_apply_eval!(cx, knob, {
                draw_bg +: {
                    inert: #(inert)
                }
            });
        }
    }

    /// THE PROGRAM SEAM. Channel slots: the deck's source becomes the
    /// effect's `input0` and the effect's output replaces the deck's
    /// contribution (an empty deck runs the effect standalone — generator
    /// engines become playable content). Transition slot: while the
    /// crossfader travels, the A/B mix is premixed offscreen, fed to the
    /// effect, and the effect's output is dissolved over the program by
    /// `triangle(mix)` — zero at the ends, full mid-fade, so engagement
    /// never pops. Empty/bypassed slots leave everything byte-identical.
    /// Returns (deck-level sources, program sources): the deck monitors
    /// show content × that deck's OWN effect only; the transition splice
    /// exists solely in the program tuple.
    #[allow(clippy::type_complexity)]
    fn apply_fx_slots(
        &mut self,
        cx: &mut Cx,
        a: Option<(Texture, f32)>,
        b: Option<(Texture, f32)>,
        mix: f32,
        mix_state: MixState,
    ) -> (
        (Option<(Texture, f32)>, Option<(Texture, f32)>),
        (Option<(Texture, f32)>, Option<(Texture, f32)>, f32, MixState),
    ) {
        let beat = self
            .clock_secs(Instant::now())
            .filter(|_| self.beat_clock.running())
            .map(|secs| (self.beat_clock.position_at(secs), self.beat_clock.bpm()))
            .filter(|(_, bpm)| *bpm > 0.0);
        let mut a = a;
        let mut b = b;
        for (kind, chan) in [(FxSlotKind::EffectA, &mut a), (FxSlotKind::EffectB, &mut b)] {
            let slot = self.fx_slots.slot(kind).clone();
            let widget = self.ui.widget(cx, Self::fx_slot_host_path(kind));
            let Some(mut host) = widget.borrow_mut::<VjFxSlotHost>() else { continue };
            let want = slot.running() && host.has_effect();
            host.set_enabled(cx, want);
            if !want {
                continue;
            }
            if let Some((pos, bpm)) = beat {
                host.set_beat(pos, bpm);
            }
            host.set_speed(FxSlots::speed_scale(slot.speed));
            // The three fixed dials drive p0..p2 directly; an untouched
            // knob leaves the doc's binding alone. (A touched knob on an
            // undeclared param writes a value nothing reads — honest and
            // dimmed in the UI.)
            let over = [slot.p[0], slot.p[1], slot.p[2], None];
            host.set_user(over);
            host.set_channel_input(chan.as_ref().map(|(tex, _)| tex.clone()));
            // First frame after a load has no output yet: pass the deck
            // through rather than a black frame.
            if let Some(tex) = host.output() {
                *chan = Some((tex, 16.0 / 9.0));
            }
        }
        // Transition engagement is a PURE FUNCTION OF POSITION — parked
        // mid-fader HOLDS the transition (that is how an MX50 key is
        // ridden). The CURVE is the doc's to declare (`engage:`):
        //   triangle — zero at both ends, full at mid; timed moves whose
        //              t endpoints ARE the plain decks (wipe/dissolve).
        //   ramp     — min(2·mix, 1): zero only at A's end, FULL at B's —
        //              an overlay/key ridden to the end STAYS applied (the
        //              keyed composite is the destination; snapping to
        //              plain B was the "luma key flicks A off" bug).
        let tri = 1.0 - (2.0 * mix - 1.0).abs();
        let slot = self.fx_slots.slot(FxSlotKind::Transition).clone();
        let widget = self.ui.widget(cx, Self::fx_slot_host_path(FxSlotKind::Transition));
        // The DECK monitors stop here: their own effect applied, no
        // transition — that belongs to the program alone.
        let decks = (a.clone(), b.clone());
        let mut out = (a, b, mix, mix_state);
        if let Some(mut host) = widget.borrow_mut::<VjFxSlotHost>() {
            let engage = if host.engage_ramp() {
                (2.0 * mix).clamp(0.0, 1.0)
            } else {
                tri
            };
            self.fx_engage_now = if slot.running() && host.has_effect() {
                engage
            } else {
                0.0
            };
            let run = slot.running() && host.has_effect() && engage > 0.002;
            host.set_enabled(cx, run);
            if run {
                if let Some((pos, bpm)) = beat {
                    host.set_beat(pos, bpm);
                }
                host.set_speed(FxSlots::speed_scale(slot.speed));
                if host.wants_deck_inputs() {
                    // TWO-DECK transition (`engine: "transition"`): the doc
                    // sees both decks and p3 IS the crossfader, so its
                    // output equals deck A at 0 and deck B at 1 — the
                    // engaged picture fully REPLACES the program with no
                    // pops at the ends, and a mid-fader park simply HOLDS
                    // (keys are ridden, not passed through).
                    host.set_user([slot.p[0], slot.p[1], slot.p[2], Some(mix)]);
                    host.set_deck_inputs(
                        out.0.as_ref().map(|(tex, _)| tex.clone()),
                        out.1.as_ref().map(|(tex, _)| tex.clone()),
                    );
                    if let Some(fx) = host.output() {
                        out = (
                            Some((fx, 16.0 / 9.0)),
                            None,
                            0.0,
                            MixState::default(),
                        );
                    }
                } else {
                    // PREMIX transition (input0-only docs): the effect
                    // shapes the composited program and dissolves over it
                    // by the fade triangle — zero at the ends, full
                    // mid-fade. p3 carries the triangle for docs that bind
                    // intensity to it.
                    host.set_user([slot.p[0], slot.p[1], slot.p[2], Some(tri)]);
                    host.set_premix(PremixJob {
                        a: out.0.clone(),
                        b: out.1.clone(),
                        mix: out.2,
                        state: out.3,
                    });
                    // Only splice into the program once both textures exist
                    // (the first engaged frame renders them).
                    if engage > 0.004 {
                        if let (Some(premix), Some(fx)) =
                            (host.premix_output(), host.output())
                        {
                            out = (
                                Some((premix, 16.0 / 9.0)),
                                Some((fx, 16.0 / 9.0)),
                                engage,
                                MixState::default(),
                            );
                        }
                    }
                }
            }
        }
        (decks, out)
    }

    /// Per-pump tile upkeep: the loaded effect's catalog THUMBNAIL on the
    /// tile (identity over monitoring — the deck monitors show the live
    /// result; the tile says WHICH effect is loaded), and the transition
    /// engage meter (quantized so idle frames cost nothing).
    fn pump_fx_slot_tiles(&mut self, cx: &mut Cx) {
        let now = cx.seconds_since_app_start();
        for kind in FxSlotKind::ALL {
            let thumb = self
                .fx_slots
                .slot(kind)
                .rev
                .as_deref()
                .and_then(|r| r.parse::<AssetRevisionId>().ok())
                .and_then(|rev| {
                    // The animated sheet PLAYS on the tile (identity with
                    // motion — the same frames the grid tile animates);
                    // stills fall back to the plain thumb.
                    self.thumb_anims
                        .get(&rev)
                        .filter(|(frames, _)| !frames.is_empty())
                        .map(|(frames, fps)| {
                            let fps = if *fps > 0.0 { *fps as f64 } else { 12.0 };
                            let at = (now * fps) as usize % frames.len();
                            frames[at].clone()
                        })
                        .or_else(|| self.thumbs.get(&rev).cloned())
                });
            let widget = self.ui.widget(cx, Self::fx_slot_tile_path(kind));
            if let Some(mut tile) = widget.borrow_mut::<VjFxSlotTile>() {
                tile.set_preview(cx, thumb);
            };
        }
        let quantized = (self.fx_engage_now * 24.0).round() / 24.0;
        let now = cx.seconds_since_app_start();
        let flashes_done = self.fx_slots.tick_flashes(now);
        if flashes_done || (quantized - self.fx_engage_synced).abs() > 1e-3 {
            self.fx_engage_synced = quantized;
            self.sync_fx_slots_ui(cx);
        }
    }

    // ---- STICKY PER-CONTENT PROFILES ------------------------------------
    // Digest-keyed local cache files (thin-client law: durable content
    // stays in the store; these are the operator's per-clip/per-effect
    // hand positions). Written at SETTLE moments — mouse-up, a toggle —
    // never per frame.

    fn clip_settings_path(rev: &AssetRevisionId) -> PathBuf {
        service::session_config_from_env()
            .cache_parent
            .join("clip-settings")
            .join(format!("{rev}.txt"))
    }

    fn fx_settings_path(rev: &str) -> PathBuf {
        service::session_config_from_env()
            .cache_parent
            .join("fx-settings")
            .join(format!("{rev}.txt"))
    }

    /// Persist THIS deck's playback profile under the cued clip's revision:
    /// cueing the same asset anywhere restores loop/bounce/trim/rate/mute.
    fn save_clip_profile(&self, slot: SlotId) {
        let i = slot.index();
        let Some(rev) = self.slot_scan[i] else { return };
        let path = Self::clip_settings_path(&rev);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let trim = self.slot_trim[i];
        let body = format!(
            "loop {}
bounce {}
trim {:.6} {:.6}
rate {}
mute {}
sync {}
",
            u8::from(self.slot_loop[i]),
            u8::from(self.slot_pingpong[i]),
            trim.0,
            trim.1,
            self.slot_beat_rate[i],
            u8::from(self.slot_video_muted[i]),
            u8::from(self.slot_beat_sync[i]),
        );
        let _ = std::fs::write(path, body);
    }

    fn load_clip_profile(rev: &AssetRevisionId) -> Option<ClipProfile> {
        let body = std::fs::read_to_string(Self::clip_settings_path(rev)).ok()?;
        let mut profile = ClipProfile::default();
        for line in body.lines() {
            let mut it = line.split_whitespace();
            match (it.next(), it.next(), it.next()) {
                (Some("loop"), Some(v), _) => profile.loop_on = v == "1",
                (Some("bounce"), Some(v), _) => profile.pingpong = v == "1",
                (Some("trim"), Some(a), Some(b)) => {
                    let t0: f64 = a.parse().unwrap_or(0.0);
                    let t1: f64 = b.parse().unwrap_or(1.0);
                    profile.trim = (t0.clamp(0.0, 1.0), t1.clamp(0.0, 1.0));
                }
                (Some("rate"), Some(v), _) => {
                    profile.rate = v.parse().unwrap_or(1.0);
                }
                (Some("mute"), Some(v), _) => profile.muted = v == "1",
                (Some("sync"), Some(v), _) => profile.sync = v == "1",
                _ => {}
            }
        }
        Some(profile)
    }

    /// Persist the slot's dial state under the LOADED EFFECT's revision —
    /// only dials the operator actually touched (None entries stay the
    /// doc's own defaults forever).
    fn save_fx_profile(&self, kind: FxSlotKind) {
        let slot = self.fx_slots.slot(kind);
        let Some(rev) = slot.rev.as_deref() else { return };
        let path = Self::fx_settings_path(rev);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let p = |v: Option<f32>| match v {
            Some(v) => format!("{v}"),
            None => "-".to_string(),
        };
        let body = format!(
            "spd {}
on {}
p0 {}
p1 {}
p2 {}
",
            slot.speed,
            u8::from(!slot.bypass),
            p(slot.p[0]),
            p(slot.p[1]),
            p(slot.p[2]),
        );
        let _ = std::fs::write(path, body);
    }

    /// Layer the effect's own sticky dial profile onto a freshly loaded
    /// (untouched) slot. Called right after `loaded()` reset the dials.
    fn apply_fx_profile(&mut self, kind: FxSlotKind) {
        let Some(rev) = self.fx_slots.slot(kind).rev.clone() else { return };
        let Ok(body) = std::fs::read_to_string(Self::fx_settings_path(&rev)) else {
            return;
        };
        let slot = self.fx_slots.slot_mut(kind);
        for line in body.lines() {
            let mut it = line.split_whitespace();
            match (it.next(), it.next()) {
                (Some("spd"), Some(v)) => {
                    if let Ok(v) = v.parse::<f32>() {
                        slot.speed = v.clamp(0.0, 1.0);
                    }
                }
                (Some("on"), Some(v)) => slot.bypass = v == "0",
                (Some(k), Some(v)) if k.starts_with('p') => {
                    let index: usize = k[1..].parse().unwrap_or(9);
                    if index < 3 && v != "-" {
                        if let Ok(v) = v.parse::<f32>() {
                            slot.p[index] = Some(v.clamp(0.0, 1.0));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Persistence: gen-panel.txt style — the knob/bypass lines plus the
    // loaded documents' splash text, so slots restore with no store round
    // trip (and before the store is even connected).

    fn fx_slots_state_path() -> PathBuf {
        service::session_config_from_env().cache_parent.join("fx-slots.txt")
    }

    fn fx_slot_source_path(kind: FxSlotKind) -> PathBuf {
        service::session_config_from_env()
            .cache_parent
            .join(format!("fx-slot-{}.splash", kind.key()))
    }

    fn save_fx_slots(&self) {
        let path = Self::fx_slots_state_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.fx_slots.encode());
    }

    fn save_fx_slot_source(&self, kind: FxSlotKind, source: &str) {
        let path = Self::fx_slot_source_path(kind);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, source);
    }

    fn load_fx_slots_panel(&mut self, cx: &mut Cx) {
        let Ok(body) = std::fs::read_to_string(Self::fx_slots_state_path()) else {
            self.sync_fx_slots_ui(cx);
            return;
        };
        let decoded = FxSlots::decode(&body);
        for kind in FxSlotKind::ALL {
            let slot = decoded.slot(kind).clone();
            let bypass = slot.bypass;
            let has_doc = slot.title.is_some();
            self.fx_slots.slots[kind.index()] = slot;
            if has_doc {
                match std::fs::read_to_string(Self::fx_slot_source_path(kind)) {
                    Ok(source) => {
                        let title = self
                            .fx_slots
                            .slot(kind)
                            .title
                            .clone()
                            .unwrap_or_default();
                        let revision = self
                            .fx_slots
                            .slot(kind)
                            .rev
                            .as_deref()
                            .and_then(|r| r.parse().ok());
                        self.load_fx_slot(cx, kind, &title, revision, &source, false);
                        // `loaded()` switches a slot on; the operator's
                        // saved bypass wins over that.
                        self.fx_slots.slot_mut(kind).bypass = bypass;
                    }
                    Err(_) => {
                        self.fx_slots.clear(kind);
                    }
                }
            }
            self.sync_fx_slot_knobs(cx, kind);
        }
        self.sync_fx_slots_ui(cx);
        self.sync_autofade_ui(cx);
        self.video_pump = cx.new_next_frame();
    }

    /// Toggle one kind chip; the grid re-queries the server for the new lane
    /// set (no chip selected = every visual kind).

    /// A chip's ON state is FILLED plus a check — never colour alone, which
    /// is unreadable on a stage in the dark and invisible to a colour-blind
    /// operator. `label` re-labels the chip (the check is prepended here).
    fn paint_chip(&mut self, cx: &mut Cx, chip: &[LiveId], on: bool, label: Option<&str>) {
        let mut button = self.ui.button(cx, chip);
        let p = LatchPaint::chip(on);
        let (bg, bg_hover, bg_down, fg, fg_hover) =
            (p.bg(), p.bg_hover(), p.bg_down(), p.fg(), p.fg_hover());
        script_apply_eval!(cx, button, {
            draw_bg +: {
                color: #(bg)
                color_focus: #(bg)
                color_hover: #(bg_hover)
                color_down: #(bg_down)
            }
            draw_text +: {
                color: #(fg)
                color_focus: #(fg)
                color_hover: #(fg_hover)
                color_down: #(fg)
            }
        });
        if let Some(text) = label {
            // No ✓ prefix: the green lit state alone marks selection.
            button.set_text(cx, text);
        }
    }

    /// AUTOFADE wears its state: lit while it is walking the fader.
    fn sync_autofade_ui(&mut self, cx: &mut Cx) {
        // The latch state, not the transient sweep: the icon answers
        // "will a click fade in?".
        let on = self.fx_slots.click_autofade;
        self.paint_lit(cx, ids!(autofade), on);
    }

    /// Paint the ONE radio group of lane chips (and the mode buttons).
    fn sync_lane_chips_ui(&mut self, cx: &mut Cx) {
        for (chip, lane, label) in LANE_CHIPS {
            self.paint_chip(cx, chip, self.grid_lane == lane, Some(label));
        }
        for (button, surface) in MODE_BUTTONS {
            self.paint_chip(cx, button, self.apc.surface == surface, None);
        }
    }

    /// Point the explorer at one lane. SET semantics — used by the slot arm
    /// gesture; chip clicks go through [`Self::lane_chip_clicked`], which
    /// adds the click-again-for-ALL radio behavior.
    fn set_lane(&mut self, cx: &mut Cx, lane: GridLane) {
        self.grid_lane = lane;
        let (kinds, tag, exclude) = match lane {
            GridLane::All => (
                catalog::BrowseModel::<makepad_asset_client::PageCursor>::visual_kinds(),
                String::new(),
                String::new(),
            ),
            // EFFECT and TRANSITION are DISJOINT lanes: a doc lives in
            // exactly one of the two chips.
            GridLane::Kind(AssetKind::VjEffect) => (
                vec![AssetKind::VjEffect],
                String::new(),
                crate::effects::seed::TRANSITION_TAG.to_string(),
            ),
            GridLane::Kind(kind) => (vec![kind], String::new(), String::new()),
            GridLane::Transition => (
                vec![AssetKind::VjEffect],
                crate::effects::seed::TRANSITION_TAG.to_string(),
                String::new(),
            ),
        };
        self.video_model.rank_aliases = self.grid_lane == GridLane::Transition;
        let cmds = self.video_model.set_lanes(kinds, tag, exclude);
        self.run_cat_cmds(Surface::Video, cmds);
        self.sync_lane_chips_ui(cx);
    }

    /// Radio semantics: a chip click selects that lane alone. One category
    /// is ALWAYS selected — clicking the selected chip is a no-op.
    fn lane_chip_clicked(&mut self, cx: &mut Cx, lane: GridLane) {
        self.set_lane(cx, lane);
    }

    /// Switch top-level mode. The APC surface and the page are the same
    /// choice seen from two sides, so they move together.
    fn select_mode(&mut self, cx: &mut Cx, surface: ApcSurface) {
        self.apc.surface = surface;
        self.apc.bank = 0;
        self.show_apc_surface(cx);
    }

    fn set_visual_mix(&mut self, cx: &mut Cx, value: f32) {
        self.program_mix = value.clamp(0.0, 1.0);
        // Picture AND sound follow the hand; the cue engine preloads the
        // far side next.
        self.mixer.set_video_mix(self.program_mix);
        self.cue.set_fader(self.program_mix);
        self.sync_xfader_ui(cx, self.program_mix);
        self.video_pump = cx.new_next_frame();
    }

    fn pump_billboards(&mut self, cx: &mut Cx) {
        let now = cx.seconds_since_app_start();
        // The published beat position, if the slot's SYNC is going to want
        // it. Read ONCE: both slots must step on the same clock.
        let beat_position = self
            .clock_secs(Instant::now())
            .filter(|_| self.beat_clock.running())
            .map(|secs| self.beat_clock.position_at(secs));
        // The skeletal dancers ride the same clock the sprites do: a synced
        // slot's mesh view gets (position-in-beats, bpm) every pump and
        // fits one clip cycle to a musical unit; unsynced slots free-run.
        let bpm = self.beat_clock.bpm();
        for index in 0..2 {
            let slot = if index == 0 { SlotId::A } else { SlotId::B };
            let synced_slot = self.slot_beat_sync[index] && self.external_sync_enabled;
            let clock = match (synced_slot, beat_position) {
                (true, Some(position)) if bpm > 0.0 => Some((position, bpm)),
                _ => None,
            };
            if let Some(mut view) = self
                .ui
                .widget(cx, Self::slot_mesh_path(slot))
                .borrow_mut::<mesh_view::VjMeshView>()
            {
                view.set_beat_clock(clock);
            }
        }
        for index in 0..2 {
            let synced = self.slot_beat_sync[index] && self.external_sync_enabled;
            let Some(bb) = self.billboards[index].as_mut() else { continue };
            let Some(state) = bb.states.get(bb.state_i) else { continue };
            let last = bb.last.replace(now).unwrap_or(now);
            bb.accum += (now - last).min(0.25);
            let looping = state.r#loop;
            let n = state.frames.len().max(1);
            // SYNC: a looping state's whole cycle spans a musical unit, so
            // a walk marches in time and a flame flickers on the beat. The
            // clock is continuous, so this never judders.
            if let (true, true, Some(position)) = (synced, looping, beat_position) {
                let beats = sprite_cycle_beats(n);
                let cycle = (position / beats as f64).rem_euclid(1.0);
                bb.frame_i = ((cycle * n as f64).floor() as usize).min(n - 1);
                bb.accum = 0.0;
                if let Some(tex) = bb
                    .textures
                    .get(bb.state_i)
                    .and_then(|frames| frames.get(bb.frame_i))
                {
                    self.slot_textures[index] = Some(tex.clone());
                    if let Some((_, w, h)) = state.frames.get(bb.frame_i) {
                        self.slot_aspect[index] = *w as f32 / (*h).max(1) as f32;
                    }
                }
                continue;
            }
            let step = 1.0 / f64::from(state.fps.max(1.0));
            while bb.accum >= step {
                bb.accum -= step;
                if bb.frame_i + 1 < n {
                    bb.frame_i += 1;
                } else if looping {
                    bb.frame_i = 0;
                }
            }
            if let Some(tex) = bb
                .textures
                .get(bb.state_i)
                .and_then(|frames| frames.get(bb.frame_i))
            {
                self.slot_textures[index] = Some(tex.clone());
                if let Some((_, w, h)) = state.frames.get(bb.frame_i) {
                    self.slot_aspect[index] = *w as f32 / (*h).max(1) as f32;
                }
            }
        }
    }

    // ---- flow warp (see flow_warp.rs) ---------------------------------------

    /// The offscreen warp pass widget of a slot.
    fn slot_flow_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(slot_flow_a),
            SlotId::B => ids!(slot_flow_b),
        }
    }

    /// Flow warp is this slot's picture transport right now.
    fn flow_active(&self, i: usize) -> bool {
        self.slot_flow_avail[i] && self.slot_flow_on[i] && self.slot_media[i] == SlotMedia::Video
    }

    /// Run `f` on a slot's warp view (no-op without the widget).
    fn flow_view<R>(
        &self,
        cx: &mut Cx,
        slot: SlotId,
        f: impl FnOnce(&mut Cx, &mut flow_warp::FlowWarpView) -> R,
    ) -> Option<R> {
        let widget = self.ui.widget(cx, Self::slot_flow_path(slot));
        let mut view = widget.borrow_mut::<flow_warp::FlowWarpView>()?;
        Some(f(cx, &mut view))
    }

    /// Drop a slot's flow cache and transport (load superseded / slot closed).
    fn clear_slot_flow(&mut self, cx: &mut Cx, slot: SlotId) {
        let i = slot.index();
        self.slot_flow_gen[i] = 0;
        self.slot_flow_avail[i] = false;
        self.slot_flow_rate[i] = 1.0;
        self.flow_view(cx, slot, |cx, view| view.clear(cx));
    }

    /// Make the warp pass this slot's transport: the decoder is parked (flow
    /// playback is silent, like ping-pong — reversed/warped audio is not a
    /// thing a pad wants) and the warp clock takes over at the same position.
    fn engage_flow(&mut self, cx: &mut Cx, slot: SlotId) {
        let i = slot.index();
        let (was_playing, pos) = match self.players[i].as_mut() {
            Some(player) => {
                let was = !player.is_paused();
                let pos = player.position_secs();
                player.set_paused(true);
                player.set_muted(true);
                (was, pos)
            }
            None => (false, 0.0),
        };
        self.flow_view(cx, slot, |cx, view| {
            view.set_position_secs(cx, pos);
            view.set_playing(was_playing);
        });
        self.apply_slot_beat_sync(slot);
        self.refresh_program_lighting();
        self.video_pump = cx.new_next_frame();
    }

    /// Hand the picture back to the decoder at the warp clock's position.
    fn disengage_flow(&mut self, cx: &mut Cx, slot: SlotId) {
        let i = slot.index();
        let state = self.flow_view(cx, slot, |_cx, view| {
            let state = (view.is_playing(), view.position_secs());
            view.set_playing(false);
            state
        });
        let (playing, pos) = state.unwrap_or((false, 0.0));
        let muted = self.slot_video_muted[i];
        if let Some(player) = self.players[i].as_mut() {
            player.set_muted(muted);
            if player.duration_secs > 0.0 {
                player.seek_fraction((pos / player.duration_secs).clamp(0.0, 1.0));
            }
            player.set_paused(!playing);
        }
        self.apply_slot_beat_sync(slot);
        self.refresh_program_lighting();
        self.video_pump = cx.new_next_frame();
    }

    /// The warp output as a program source, once a pass has rendered.
    fn slot_flow_source(&self, cx: &mut Cx, slot: SlotId) -> Option<(Texture, f32)> {
        if !self.flow_active(slot.index()) {
            return None;
        }
        self.flow_view(cx, slot, |_cx, view| view.output()).flatten()
    }

    /// One display frame of flow transport: push the live rate/bounce state
    /// onto the warp clock and advance it (the advance redraws the pass).
    fn pump_flow(&mut self, cx: &mut Cx, slot: SlotId, dt: f64) {
        let i = slot.index();
        if !self.flow_active(i) {
            return;
        }
        // THE SWEEP LAW reaches the warp clock too: the trim window is
        // the range, one direction sweep spans the chip's beats, and a
        // scratch hand overrides everything. (Before this the warp free-
        // ran the whole clip — the brackets did NOTHING on an enhanced
        // deck, which is exactly what the operator reported.)
        let (t_in, t_out) = self.slot_trim[i];
        let bounce = self.slot_pingpong[i];
        let scratching = self.slot_scratch[i].is_some();
        let synced = self.slot_beat_sync[i] && self.external_sync_enabled;
        let beats = self.slot_beat_rate[i].round().clamp(1.0, 8.0) as f64;
        let beat_secs = self
            .current_beat()
            .map(|beat| beat.period.as_secs_f64())
            .unwrap_or(0.5);
        let hand_rate = self.slot_flow_rate[i];
        self.flow_view(cx, slot, |cx, view| {
            view.set_window(t_in, t_out);
            view.set_bounce(bounce);
            let rate = if scratching {
                hand_rate
            } else if synced {
                view.law_rate(beats * beat_secs)
            } else {
                1.0
            };
            view.set_rate(rate);
            view.advance(cx, dt);
        });
    }

    /// The player mode a slot's toggles add up to (ping-pong wins).
    fn slot_play_mode(&self, i: usize) -> crate::media::PlayMode {
        if self.slot_pingpong[i] {
            crate::media::PlayMode::PingPong
        } else if self.slot_loop[i] {
            crate::media::PlayMode::Loop
        } else {
            crate::media::PlayMode::Once
        }
    }

    /// The ♪ chip face for a (synced, rate) pair.
    fn rate_chip_label(sync: bool, rate: f32) -> &'static str {
        if !sync {
            return "—";
        }
        match rate {
            r if r > 6.0 => "8",
            r if r > 3.0 => "4",
            r if r > 1.5 => "2",
            _ => "1",
        }
    }

    /// The per-slot SYNC control: is this slot's content held to the beat,
    /// and — the honest half — what that is actually doing right now.
    /// Flip the MONITOR AUDIO capture: install the callback + worker on
    /// first enable, open/close the loopback device, persist the choice.
    fn set_monitor_audio(&mut self, cx: &mut Cx, on: bool) {
        self.monitor_audio = on;
        if on {
            if self.capture.is_none() {
                // ONE bounded realtime callback. Input 0 is the explicitly
                // selected loopback device — never a microphone.
                let feed = Arc::new(CaptureFeed::new());
                let callback_feed = feed.clone();
                cx.audio_input(0, move |info, buffer| {
                    callback_feed.push(info.sample_rate, buffer);
                });
                self.sync_worker = Some(SyncWorker::start(feed.clone()));
                self.capture = Some(feed);
            }
            cx.use_audio_inputs(&self.loopback_ids.clone());
        } else {
            cx.use_audio_inputs(&[]);
        }
        log!(
            "monitor audio: {} — {} loopback device(s), failed={}",
            if on { "ON" } else { "OFF" },
            self.loopback_ids.len(),
            self.loopback_failed
        );
        self.sync_monitor_audio_ui(cx);
        self.save_gen_panel();
    }

    /// The ear/beat-lock button wears the CAPTURE state — dark off, accent
    /// capturing, RED when the OS blocks the capture (a TCC denial gives a
    /// silent no-op otherwise, and silence reads as breakage).
    fn sync_monitor_audio_ui(&mut self, cx: &mut Cx) {
        let blocked = self.monitor_audio && self.loopback_failed;
        let waiting = self.monitor_audio && self.loopback_ids.is_empty();
        if blocked || waiting {
            let mut button = self.ui.button(cx, ids!(monitor_audio_btn));
            script_apply_eval!(cx, button, {
                draw_bg +: {
                    color: #xd92c2c
                    color_focus: #xd92c2c
                    color_hover: #xe54848
                    color_down: #xb02020
                }
                draw_icon +: { color: #x2b0808 }
            });
        } else {
            self.paint_icon_button(cx, ids!(monitor_audio_btn), self.monitor_audio);
        }
        let tip = if blocked {
            "Audio capture BLOCKED: System Settings → Privacy → Screen & System Audio Recording"
        } else if waiting {
            "No system-audio loopback device found"
        } else if self.monitor_audio {
            "Monitoring system audio (beat lock)"
        } else {
            "Monitor system audio (beat lock)"
        };
        if let Some(mut wrap) = self
            .ui
            .widget(cx, ids!(monitor_audio_tip))
            .borrow_mut::<makepad_widgets::tip::Tip>()
        {
            wrap.text = tip.to_string();
        };
    }

    /// SWAP (⇄): the decks trade their COMPLETE personalities instantly —
    /// clips (players mid-play, positions intact), transport state
    /// (mode/rate/mute/trim/sync), and the A/B effect slots (docs + dial
    /// values + bypass + SPD). The FADER STAYS where the hand left it, so
    /// this is a CUT move: whatever was standby slams live. With a PiP or
    /// key transition ridden mid-fader, the inset/base ROLES flip because
    /// the deck inputs did — the transition slot itself never moves.
    fn swap_decks(&mut self, cx: &mut Cx) {
        // 3D and content-effect decks render through per-side offscreen
        // widgets (mesh/splat/fx-content passes) that cannot trade yet.
        if self
            .slot_media
            .iter()
            .any(|m| matches!(m, SlotMedia::Mesh | SlotMedia::Splat | SlotMedia::Effect))
        {
            log!("swap: 3D/effect-content decks are not swappable yet");
            return;
        }
        // Flow warp is a per-side pass too: disengage, the probe re-arms.
        for slot in [SlotId::A, SlotId::B] {
            if self.slot_flow_on[slot.index()] {
                self.disengage_flow(cx, slot);
            }
        }
        self.slot_flow_on = [false, false];
        self.players.swap(0, 1);
        self.slot_textures.swap(0, 1);
        self.slot_media.swap(0, 1);
        self.slot_trim.swap(0, 1);
        self.slot_loop.swap(0, 1);
        self.slot_pingpong.swap(0, 1);
        self.slot_video_muted.swap(0, 1);
        self.slot_beat_rate.swap(0, 1);
        self.slot_beat_sync.swap(0, 1);
        self.slot_sync_beats.swap(0, 1);
        self.slot_scan.swap(0, 1);
        self.applied_fit.swap(0, 1);
        self.sig_states.swap(0, 1);
        self.slot_held.swap(0, 1);
        self.slot_aspect.swap(0, 1);
        self.billboards.swap(0, 1);
        self.awaiting_preroll.swap(0, 1);
        self.light_samples.swap(0, 1);
        self.slot_flow_avail.swap(0, 1);
        self.slot_flow_gen.swap(0, 1);
        self.strip_shape = [None, None];
        // KNOWN LIMIT: a swapped player's AUDIO keeps feeding its original
        // mixer bus (the decode thread owns its bus id), so an UNMUTED
        // clip follows the wrong fader side until re-cued. Clips default
        // muted; the picture — the point of the move — is exact.
        //
        // Effect slots trade wholesale: dial states swap in the model and
        // each host cross-loads the other's persisted doc source. The
        // content-addressed shader cache makes both loads cache hits, so
        // the exchange costs no compile.
        let state_a = self.fx_slots.slot(FxSlotKind::EffectA).clone();
        let state_b = self.fx_slots.slot(FxSlotKind::EffectB).clone();
        *self.fx_slots.slot_mut(FxSlotKind::EffectA) = state_b;
        *self.fx_slots.slot_mut(FxSlotKind::EffectB) = state_a;
        let src_a =
            std::fs::read_to_string(Self::fx_slot_source_path(FxSlotKind::EffectA)).ok();
        let src_b =
            std::fs::read_to_string(Self::fx_slot_source_path(FxSlotKind::EffectB)).ok();
        for (kind, source) in
            [(FxSlotKind::EffectA, &src_b), (FxSlotKind::EffectB, &src_a)]
        {
            let widget = self.ui.widget(cx, Self::fx_slot_host_path(kind));
            if let Some(mut host) = widget.borrow_mut::<VjFxSlotHost>() {
                match source {
                    Some(src) => {
                        let _ = host.load(cx, &format!("vjfx_slot_{}", kind.key()), src);
                    }
                    None => host.clear(cx),
                }
            };
            match source {
                Some(src) => self.save_fx_slot_source(kind, src),
                None => {
                    let _ = std::fs::remove_file(Self::fx_slot_source_path(kind));
                }
            }
            self.sync_fx_slot_knobs(cx, kind);
        }
        self.save_fx_slots();
        self.sync_fx_slots_ui(cx);
        self.sync_slot_controls_ui(cx);
        self.video_pump = cx.new_next_frame();
    }

    /// The deck's cue-ack spinner (immediate visible response to a click).
    fn set_deck_busy(&mut self, cx: &mut Cx, slot: SlotId, on: bool) {
        let path: &[LiveId] = match slot {
            SlotId::A => ids!(deck_a_busy),
            SlotId::B => ids!(deck_b_busy),
        };
        let view = self.ui.view(cx, path);
        if view.visible() != on {
            view.set_visible(cx, on);
            self.ui.redraw(cx);
        }
    }

    /// A shuttle move. `pos` -1..1; 0 = the spring landed home and the
    /// beat machinery re-owns the transport. Exponential speed with
    /// distance (4^|pos|). FLOW-WARP clips scratch bidirectionally through
    /// the warp clock — smooth frame tweening is the whole point; a plain
    /// decoder clip has no reverse (yet — the sync lane's override API
    /// lands here when it exists), so its left side is slow-motion.
    fn apply_scratch(&mut self, cx: &mut Cx, slot: SlotId, pos: f32) {
        let i = slot.index();
        if pos == 0.0 {
            if self.slot_scratch[i].take().is_some() {
                // Release: the sweep re-engages from wherever the hand
                // left the picture, then re-locks to the grid.
                if let Some(player) = self.players[i].as_mut() {
                    player.clear_scratch();
                }
                self.apply_slot_beat_sync(slot);
                self.strip_shape[i] = None;
                self.sync_slot_controls_ui(cx);
            }
            self.video_pump = cx.new_next_frame();
            return;
        }
        if self.slot_scratch[i].is_none() {
            // Entering the scratch: the chip says MANUAL.
            self.ui
                .button(cx, Self::deck_rate_path(slot))
                .set_text(cx, "—");
        }
        self.slot_scratch[i] = Some(pos);
        let mag = 4.0f64.powf(pos.abs() as f64);
        if self.flow_active(i) {
            // The warp clock runs any rate, both directions.
            self.slot_flow_rate[i] = mag * if pos < 0.0 { -1.0 } else { 1.0 };
        } else if let Some(player) = self.players[i].as_mut() {
            // Signed shuttle straight into the transport: negative IS
            // reverse (the old set_playback_rate path could only slow a
            // forward pacer — a backward scratch played slow-forward).
            player.set_scratch(mag * if pos < 0.0 { -1.0 } else { 1.0 });
        }
        self.video_pump = cx.new_next_frame();
    }

    /// × on the source cluster: EJECT the deck's clip — stop playback,
    /// release the player, deck back to its empty cue state. The same
    /// teardown a fresh cue runs, minus the new clip.
    fn unslot_deck(&mut self, cx: &mut Cx, slot: SlotId) {
        let i = slot.index();
        self.slot_held[i] = false;
        self.players[i] = None;
        self.slot_textures[i] = None;
        self.clear_slot_flow(cx, slot);
        self.light_samples[i] = None;
        self.light_analyzers[i].reset();
        self.clear_slot_mesh(cx, slot);
        self.clear_slot_fx_content(cx, slot);
        self.slot_media[i] = SlotMedia::Empty;
        self.billboards[i] = None;
        self.awaiting_preroll[i] = None;
        self.slot_scan[i] = None;
        self.applied_fit[i] = None;
        self.mixer.flush_slot_audio(slot);
        self.mixer.set_slot_paused(slot, true);
        self.strip_shape[i] = None;
        self.sync_slot_controls_ui(cx);
        self.video_pump = cx.new_next_frame();
    }

    /// Pause/resume one slot's video (picture clock + its mixer bus). With
    /// flow warp active, the warp clock is the transport: the decoder stays
    /// parked and play/pause drives the warp position instead.
    fn set_slot_paused(&mut self, cx: &mut Cx, slot: SlotId, paused: bool) {
        if self.flow_active(slot.index()) {
            self.flow_view(cx, slot, |_cx, view| view.set_playing(!paused));
            if paused {
                self.disarm_hazards(Some(cx));
            }
            self.refresh_program_lighting();
            self.video_pump = cx.new_next_frame();
            return;
        }
        let Some(player) = self.players[slot.index()].as_mut() else { return };
        player.set_paused(paused);
        self.mixer.set_slot_paused(slot, paused);
        if paused {
            self.disarm_hazards(Some(cx));
        }
        self.refresh_program_lighting();
        self.video_pump = cx.new_next_frame();
    }

    /// Beat-sync a video loop: N beats per loop → playback rate so the clip
    /// length lands exactly on N beats (0 = free-running). Re-applied as the
    /// tempo drifts; silently free when there is no lock.
    /// THE VIDEOLOOP SYNC LAW ("sync is 1 beat one play direction"):
    /// playback RATE is fixed — natural speed × the .5/1/2/4 chip, NEVER
    /// derived from the range length (stretch-to-fit made a shorter trim
    /// play faster, which is not what a videoloop does). SYNC means the
    /// BEAT drives direction/phase: every beat boundary flips a bounce /
    /// restarts a wrap (`SlotPlayer::beat_pulse`, fed from the pump), so
    /// the clip sweeps at constant speed and TURNS ON THE BEAT. A range
    /// edge reached early holds until the next beat. The chip multiplies
    /// the RATE — a 2× loop sweeps twice as far per beat, not twice as
    /// fast through a fixed range.
    fn apply_slot_beat_sync(&mut self, slot: SlotId) {
        let i = slot.index();
        // A hand on the SHUTTLE owns the transport outright: nothing
        // (refresh tick, trim release, chip write) may reassert the beat
        // rate until the spring lands the knob home.
        if self.slot_scratch[i].is_some() {
            return;
        }
        let synced = self.slot_beat_sync[i] && self.external_sync_enabled;
        // THE CHIP IS BEATS PER SWEEP now (8/4/2/1) — never a playback
        // rate. The player's pacer runs at 1.0 and the sweep's own step
        // math sets the on-screen speed from the range and the grid.
        let beats = self.slot_beat_rate[i].round().clamp(1.0, 8.0) as u8;
        let hint = self
            .current_beat()
            .map(|beat| (beat.period.as_secs_f64() * 1e7) as i64)
            .unwrap_or(0);
        self.applied_fit[i] = None;
        if let Some(player) = self.players[i].as_mut() {
            if (player.playback_rate() - 1.0).abs() > 0.0015 {
                player.set_playback_rate(1.0);
            }
            player.set_beats_per_sweep(beats);
            player.set_beat_hint(hint);
            player.set_beat_transport(synced);
        }
        // FLOW WARP: the law rate is derived per-pump (pump_flow); the
        // stored rate only carries the scratch hand.
        if self.slot_scratch[i].is_none() {
            self.slot_flow_rate[i] = 1.0;
        }
    }

    /// Alias kept for its many call sites; the ONE authority lives above.
    fn apply_loop_fit(&mut self, slot: SlotId) {
        self.apply_slot_beat_sync(slot);
    }


    /// Paint an icon button lit (accent) or at rest.
    fn paint_icon_button(&mut self, cx: &mut Cx, id: &[LiveId], lit: bool) {
        self.paint_icon_face(cx, id, LatchPaint::icon(lit));
    }

    fn paint_icon_face(&mut self, cx: &mut Cx, id: &[LiveId], p: LatchPaint) {
        let (bg, bg_hover, bg_down, fg) = (p.bg(), p.bg_hover(), p.bg_down(), p.fg());
        let mut button = self.ui.widget(cx, id);
        script_apply_eval!(cx, button, {
            draw_bg +: {
                color: #(bg)
                color_focus: #(bg)
                color_hover: #(bg_hover)
                color_down: #(bg_down)
            }
            draw_icon +: { color: #(fg) }
        });
    }

    /// Same face law for TEXT buttons (the rate chip, ×) — and it drops
    /// the paint_lit cache entry so the next real latch paint lands.
    fn paint_text_face(&mut self, cx: &mut Cx, id: &[LiveId], p: LatchPaint) {
        let key = id.iter().fold(0u64, |acc, live| acc ^ live.0.rotate_left(7));
        self.lit_state.remove(&key);
        let (bg, bg_hover, bg_down, fg, fg_hover) =
            (p.bg(), p.bg_hover(), p.bg_down(), p.fg(), p.fg_hover());
        let mut button = self.ui.button(cx, id);
        script_apply_eval!(cx, button, {
            draw_bg +: {
                color: #(bg)
                color_focus: #(bg)
                color_hover: #(bg_hover)
                color_down: #(bg_down)
            }
            draw_text +: {
                color: #(fg)
                color_focus: #(fg)
                color_hover: #(fg_hover)
                color_down: #(fg)
            }
        });
    }

    fn slot_spin_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(slot_a_spin),
            SlotId::B => ids!(slot_b_spin),
        }
    }

    fn slot_anim_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(slot_a_anim),
            SlotId::B => ids!(slot_b_anim),
        }
    }

    fn slot_anim_box_path(slot: SlotId) -> &'static [LiveId] {
        match slot {
            SlotId::A => ids!(slot_a_anim_box),
            SlotId::B => ids!(slot_b_anim_box),
        }
    }

    /// Animation tracks a slot can switch between: billboard states or the
    /// dancer's clips. Empty for video/stills/statues/splats.
    fn slot_anim_tracks(&self, cx: &mut Cx, slot: SlotId) -> (Vec<String>, Option<usize>) {
        match self.slot_media[slot.index()] {
            SlotMedia::Billboard => match self.billboards[slot.index()].as_ref() {
                Some(bb) => (bb.states.iter().map(|s| s.name.clone()).collect(), Some(bb.state_i)),
                None => (Vec::new(), None),
            },
            SlotMedia::Mesh => {
                let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
                let view = widget.borrow::<mesh_view::VjMeshView>();
                let tracks = match view.as_ref() {
                    Some(view) => (view.clip_names(), view.clip_index()),
                    None => (Vec::new(), None),
                };
                tracks
            }
            _ => (Vec::new(), None),
        }
    }

    /// Mirror each cue strip to its content: video transport, 3D/splat spin +
    /// track, sprite track. Hidden controls keep the strip the same height.
    /// Runs at the status rate, so everything that costs a script evaluation
    /// (button paint, dropdown labels, visibility) is applied only on change.
    fn sync_slot_controls_ui(&mut self, cx: &mut Cx) {
        for slot in [SlotId::A, SlotId::B] {
            let i = slot.index();
            let media = self.slot_media[i];
            let (tracks, selected) = self.slot_anim_tracks(cx, slot);
            let is_video = media == SlotMedia::Video;
            let is_3d = matches!(media, SlotMedia::Mesh | SlotMedia::Splat);
            let (playing, pos, dur) = match self.players[i].as_ref() {
                Some(p) => (!p.is_paused(), p.position_secs(), p.duration_secs),
                None => (false, 0.0, 0.0),
            };
            // With flow warp active the warp clock is the transport the
            // strip must mirror — the decoder underneath is parked.
            let (playing, pos) = if self.flow_active(i) {
                self.flow_view(cx, slot, |_cx, view| (view.is_playing(), view.position_secs()))
                    .unwrap_or((playing, pos))
            } else {
                (playing, pos)
            };
            // ROTATE latches on the operator's switch, never on a derived
            // motion state: a toggle that "applies" (the model reacts) but
            // stays dark is the bug this pins down.
            let spinning = self.slot_spin[i];
            let shape = StripShape {
                present: media != SlotMedia::Empty,
                video: is_video,
                spin: is_3d,
                tracks: tracks.clone(),
                selected,
                playing,
                looping: self.slot_loop[i],
                spinning,
                beat_sync: self.slot_beat_sync[i],
                beat_rate: self.slot_beat_rate[i],
            };
            if self.strip_shape[i].as_ref() != Some(&shape) {
                self.strip_shape[i] = Some(shape.clone());
                // THE NO-PUSH LAW: the transport row's SPACE is always
                // reserved — hiding it made the first cue shove the whole
                // console down. An empty deck GHOSTS the controls instead.
                if !shape.present {
                    for path in [
                        Self::deck_play_path(slot),
                        Self::deck_rw_path(slot),
                        Self::deck_loop2_path(slot),
                        Self::deck_bounce_path(slot),
                        Self::deck_mute_path(slot),
                    ] {
                        self.paint_icon_face(cx, path, LatchPaint::ghost());
                    }
                    self.paint_text_face(cx, Self::deck_rate_path(slot), LatchPaint::ghost());
                    self.paint_text_face(cx, Self::deck_eject_path(slot), LatchPaint::ghost());
                }
                self.ui.button(cx, Self::slot_spin_path(slot)).set_visible(cx, is_3d);
                self.ui
                    .view(cx, Self::slot_anim_box_path(slot))
                    .set_visible(cx, !tracks.is_empty());
                if is_video {
                    // This runs at cue time too, so a fresh clip shows its
                    // real latches at once: LOOP lit (loops default on),
                    // MUTE lit (cues default muted) — never an innocent
                    // dark icon over an active state.
                    self.paint_icon_button(cx, Self::deck_play_path(slot), playing);
                    self.paint_icon_button(cx, Self::deck_loop2_path(slot), shape.looping);
                    self.paint_icon_button(cx, Self::deck_bounce_path(slot), self.slot_pingpong[i]);
                    self.paint_icon_button(cx, Self::deck_mute_path(slot), self.slot_video_muted[i]);
                    // ONE compact chip = sync mode + rate + status: ♪N
                    // lit while beat-synced, ♪— dim while free-running.
                    self.ui.button(cx, Self::deck_rate_path(slot)).set_text(
                        cx,
                        Self::rate_chip_label(shape.beat_sync, shape.beat_rate),
                    );
                    self.paint_lit(cx, Self::deck_rate_path(slot), shape.beat_sync);
                }
                if is_3d {
                    self.paint_icon_button(cx, Self::slot_spin_path(slot), spinning);
                }
                if !tracks.is_empty() {
                    let anim = self.ui.drop_down(cx, Self::slot_anim_path(slot));
                    anim.set_labels(cx, tracks);
                    if let Some(index) = selected {
                        anim.set_selected_item(cx, index);
                    }
                }
            }
            // Position/time mirroring lives in pump_source_monitors — the
            // mini player's scrub bar and clock are fed every video frame.
            let _ = (pos, dur);
        }
        if self.mute_painted != Some(self.video_muted) {
            self.mute_painted = Some(self.video_muted);
            self.paint_icon_button(cx, ids!(video_mute), self.video_muted);
        }
    }

    /// Spin (turntable / orbit) on or off for a 3D or splat slot.
    fn set_slot_spin(&mut self, cx: &mut Cx, slot: SlotId, on: bool) {
        self.slot_spin[slot.index()] = on;
        self.set_slot_mesh_paused(cx, slot, !on);
        self.video_pump = cx.new_next_frame();
    }

    /// Switch a slot's animation track (billboard state / dancer clip).
    fn set_slot_anim(&mut self, cx: &mut Cx, slot: SlotId, index: usize) {
        match self.slot_media[slot.index()] {
            SlotMedia::Billboard => {
                if let Some(bb) = self.billboards[slot.index()].as_mut() {
                    if index < bb.states.len() {
                        bb.state_i = index;
                        bb.frame_i = 0;
                        bb.accum = 0.0;
                    }
                }
            }
            SlotMedia::Mesh => {
                let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
                let view = widget.borrow_mut::<mesh_view::VjMeshView>();
                if let Some(mut view) = view {
                    view.set_clip(cx, index);
                }
            }
            _ => {}
        }
        self.video_pump = cx.new_next_frame();
    }

    /// Wheel on a cue well zooms that slot's 3D model / splat camera.
    fn zoom_slot_by(&mut self, cx: &mut Cx, slot: SlotId, axis: f64) {
        match self.slot_media[slot.index()] {
            SlotMedia::Mesh => {
                let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
                let view = widget.borrow_mut::<mesh_view::VjMeshView>();
                if let Some(mut view) = view {
                    view.zoom_by(cx, axis);
                }
            }
            SlotMedia::Splat => {
                let scene = self.ui.widget(cx, Self::slot_splat_scene_path(slot));
                if let Some(mut view) = scene.borrow_mut::<makepad_xr::scene::XrSceneView>() {
                    let camera = view.camera_mut();
                    let factor = if axis > 0.0 { 1.0 / 0.92 } else { 0.92 };
                    camera.distance = (camera.distance * factor).clamp(camera.distance_min.max(0.05), 40.0);
                }
                scene.redraw(cx);
            }
            _ => return,
        }
        self.video_pump = cx.new_next_frame();
    }

    /// Drag on a cue well orbits that slot's 3D model / splat camera.
    fn orbit_slot_by(&mut self, cx: &mut Cx, slot: SlotId, dx: f32, dy: f32) {
        match self.slot_media[slot.index()] {
            SlotMedia::Mesh => {
                let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
                let view = widget.borrow_mut::<mesh_view::VjMeshView>();
                if let Some(mut view) = view {
                    view.orbit_by(cx, dx, dy);
                }
            }
            SlotMedia::Splat => {
                let scene = self.ui.widget(cx, Self::slot_splat_scene_path(slot));
                if let Some(mut view) = scene.borrow_mut::<makepad_xr::scene::XrSceneView>() {
                    let camera = view.camera_mut();
                    camera.orbit_yaw -= dx * 0.01;
                    camera.orbit_pitch = (camera.orbit_pitch + dy * 0.01).clamp(-1.45, 1.45);
                }
                scene.redraw(cx);
            }
            _ => {}
        }
        self.video_pump = cx.new_next_frame();
    }

    fn sync_xfader_ui(&mut self, cx: &mut Cx, mix: f32) {
        let slider = self.ui.slider(cx, ids!(apc_xfader));
        // The mirror never fights the hand on the fader.
        let dragging = slider.borrow().is_some_and(|s| s.dragging.is_some());
        if !dragging {
            slider.set_value(cx, mix.clamp(0.0, 1.0) as f64);
        }
    }

    // ---- show control ------------------------------------------------------

    fn sync_lighting_controls_ui(&mut self, cx: &mut Cx) {
        let state = self.lighting_controls;
        for (id, value) in [
            (ids!(light_master), state.master),
            (ids!(light_black_floor), state.black_floor),
            (ids!(light_colorfulness), state.colorfulness),
            (ids!(light_response), state.response),
            (ids!(show_movers), state.movers),
            (ids!(show_rgb), state.rgb),
            (ids!(show_strobe), state.strobe),
            (ids!(laser_level), state.laser_level),
            (ids!(smoke_level), state.smoke_level),
            (ids!(uv_level), state.uv_level),
        ] {
            self.ui.slider(cx, id).set_value(cx, value as f64);
        }
        for (id, active) in [
            (ids!(laser_arm), state.laser_armed),
            (ids!(smoke_arm), state.smoke_armed),
            (ids!(uv_arm), state.uv_armed),
        ] {
            self.ui.check_box(cx, id).set_active(cx, active, Animate::No);
        }
        let video_status = if state.blackout_latched {
            "BLACKOUT LATCHED".to_string()
        } else {
            format!(
                "master {:.0}% · floor {:.1}% · color {:.0}% · response {:.0}%",
                state.master * 100.0,
                state.black_floor * 100.0,
                state.colorfulness * 100.0,
                state.response * 100.0,
            )
        };
        self.ui
            .label(cx, ids!(lighting_values))
            .set_text(cx, &video_status);

        let hazard_status = if state.hazards_live() {
            "DEADMAN LIVE"
        } else if state.any_hazard_armed() {
            "armed · hold deadman"
        } else {
            "disarmed"
        };
        self.ui
            .label(cx, ids!(hazard_status))
            .set_text(cx, hazard_status);
        self.refresh_room_desk_ui(cx);
    }

    fn refresh_room_desk_ui(&mut self, cx: &mut Cx) {
        let Some(desk) = self.lighting.as_ref() else { return };
        let snap = desk.snapshot();
        let fade_ids = [
            ids!(light_fader_0),
            ids!(light_fader_1),
            ids!(light_fader_2),
            ids!(light_fader_3),
            ids!(light_fader_4),
            ids!(light_fader_5),
            ids!(light_fader_6),
            ids!(light_fader_7),
            ids!(light_fader_8),
        ];
        for (index, id) in fade_ids.iter().enumerate() {
            self.ui
                .slider(cx, *id)
                .set_value(cx, snap.state.fade[index] as f64);
        }
        let top_ids = [
            ids!(light_knob_0),
            ids!(light_knob_1),
            ids!(light_knob_2),
            ids!(light_knob_3),
            ids!(light_knob_4),
            ids!(light_knob_5),
            ids!(light_knob_6),
            ids!(light_knob_7),
        ];
        for (index, id) in top_ids.iter().enumerate() {
            self.ui
                .slider(cx, *id)
                .set_value(cx, snap.state.dial_top[index] as f64);
        }
        let bank = match self.light_track {
            1 => snap.state.dial_1,
            2 => snap.state.dial_2,
            3 => snap.state.dial_3,
            4 => snap.state.dial_4,
            5 => snap.state.dial_5,
            6 => snap.state.dial_6,
            7 => snap.state.dial_7,
            _ => snap.state.dial_0,
        };
        let dev_ids = [
            ids!(light_dev_0),
            ids!(light_dev_1),
            ids!(light_dev_2),
            ids!(light_dev_3),
            ids!(light_dev_4),
            ids!(light_dev_5),
            ids!(light_dev_6),
            ids!(light_dev_7),
        ];
        for (index, id) in dev_ids.iter().enumerate() {
            self.ui.slider(cx, *id).set_value(cx, bank[index] as f64);
        }
        self.ui
            .check_box(cx, ids!(light_power))
            .set_active(cx, snap.buttons.power, Animate::No);
        self.ui
            .check_box(cx, ids!(light_write))
            .set_active(cx, snap.buttons.write_preset, Animate::No);
        let scene = snap
            .last_scene
            .map(|s| format!("P{s:02}"))
            .unwrap_or_default();
        self.ui.label(cx, ids!(light_desk_status)).set_text(cx, &scene);
        let legend = match self.light_track {
            1 => "PAN TILT",
            3 => "RGB FX",
            4 => "STRB",
            5 => "PAT XY",
            6 => "UV",
            _ => "SMK",
        };
        self.ui.label(cx, ids!(dev_knob_legend)).set_text(cx, legend);
    }

    /// Fail-safe local state transition used by focus/lifecycle events as well
    /// as explicit blackout. The shared driver independently expires its
    /// heartbeat, so losing this UI thread cannot leave a hazard running.
    fn disarm_hazards(&mut self, cx: Option<&mut Cx>) {
        self.lighting_controls.disarm_hazards();
        if let Some(lighting) = self.lighting.as_ref() {
            lighting.set_power(false);
        }
        if let Some(cx) = cx {
            self.sync_lighting_controls_ui(cx);
        }
    }

    fn lighting_program_running(&self) -> bool {
        !self.video_muted
            && self
                .cue
                .live_slot()
                .and_then(|slot| self.players[slot.index()].as_ref())
                .is_some_and(|player| !player.is_paused())
    }

    fn live_program_mix(&mut self) -> f32 {
        let mut mix = self.program_mix;
        if let Some(transition) = self.mixer.video_transition_snapshot() {
            // Only a RUNNING fade drives the picture mix; a completed one was
            // landed once by pump_transitions and the fader is the operator's.
            if transition.phase == VideoTransitionPhase::Started {
                let target = if transition.to == SlotId::B { 1.0 } else { 0.0 };
                let origin = match transition.from {
                    Some(SlotId::B) => 1.0,
                    Some(SlotId::A) => 0.0,
                    None => 1.0 - target,
                };
                mix = origin + (target - origin) * transition.progress;
            }
        }
        mix
    }

    fn program_light_sample(&self, mix: f32) -> SpatialLightSample {
        let output_enabled =
            self.lighting_program_running() && !self.lighting_controls.blackout_latched;
        let active_sample = |index: usize| {
            self.players[index]
                .as_ref()
                .filter(|player| !player.is_paused())
                .and(self.light_samples[index])
        };
        mix_program_lights(active_sample(0), active_sample(1), mix, output_enabled)
    }

    fn publish_program_lighting(&self, _mix: f32) {
        // Room lighting is the automate APC40 desk (`apply_dmx_mapping`),
        // not the video-pixel ambilight path.
    }

    fn publish_lighting_controls(&self) {}

    fn latch_lighting_blackout(&mut self) {
        self.lighting_controls.latch_blackout();
        if let Some(lighting) = self.lighting.as_ref() {
            lighting.set_power(false);
        }
    }

    fn restore_lighting(&mut self) {
        self.lighting_controls.blackout_latched = false;
        self.lighting_controls.disarm_hazards();
        if let Some(lighting) = self.lighting.as_ref() {
            lighting.set_power(true);
        }
    }

    fn refresh_program_lighting(&mut self) {
        let mix = self.live_program_mix();
        self.publish_program_lighting(mix);
        self.publish_lighting_controls();
    }

    fn start_lighting(&mut self) {
        if !self.lighting_controls_loaded {
            self.lighting_controls = LightingControls::from_env();
            self.lighting_controls_loaded = true;
        }
        for analyzer in &mut self.light_analyzers {
            tune_light_analyzer(analyzer, self.lighting_controls);
        }
        let disabled = std::env::var("VJ_ARTNET_DISABLE")
            .ok()
            .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"));
        if disabled {
            self.lighting_status = "lights off (VJ_ARTNET_DISABLE)".to_string();
            self.lighting_retry_at = None;
            return;
        }
        let mut config = PerformanceConfig::default();
        if let Ok(target) = std::env::var("VJ_ARTNET_TARGET") {
            if !target.trim().is_empty() {
                config.artnet.target_addr = target;
            }
        }
        // The live master belongs to PerformanceState. Keep the Art-Net
        // adapter's outer multiplier as a startup-only safety ceiling.
        config.artnet.master = env_level("VJ_LIGHT_OUTPUT_CAP", 1.0, 0.0, 1.0);
        // VJ starts below the physical fixture ceilings. Operators can raise
        // an individual group explicitly via environment policy, but ordinary
        // video playback should never flood the room or produce a harsh cut.
        let vj_caps = PowerCaps {
            ambilight: 0.72,
            rgb: 0.40,
            movers: 0.45,
            strobe: 0.08,
            lasers: 0.15,
            smoke: 0.30,
            uv: 0.30,
        };
        config.power_caps = PowerCaps {
            ambilight: env_level(
                "VJ_SHOW_CAP_AMBILIGHT",
                vj_caps.ambilight,
                0.0,
                1.0,
            ),
            rgb: env_level("VJ_SHOW_CAP_RGB", vj_caps.rgb, 0.0, 1.0),
            movers: env_level(
                "VJ_SHOW_CAP_MOVERS",
                vj_caps.movers,
                0.0,
                1.0,
            ),
            strobe: env_level(
                "VJ_SHOW_CAP_STROBE",
                vj_caps.strobe,
                0.0,
                1.0,
            ),
            lasers: env_level(
                "VJ_SHOW_CAP_LASERS",
                vj_caps.lasers,
                0.0,
                1.0,
            ),
            smoke: env_level(
                "VJ_SHOW_CAP_SMOKE",
                vj_caps.smoke,
                0.0,
                1.0,
            ),
            uv: env_level("VJ_SHOW_CAP_UV", vj_caps.uv, 0.0, 1.0),
        };
        config.hazard_heartbeat_timeout = env_millis(
            "VJ_HAZARD_HEARTBEAT_MS",
            config.hazard_heartbeat_timeout,
            100,
            1_000,
        );
        config.control_timeout = env_millis(
            "VJ_SHOW_CONTROL_TIMEOUT_MS",
            config.control_timeout,
            250,
            10_000,
        );
        let target = std::env::var("VJ_ARTNET_TARGET")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| ARTNET_BROADCAST_ADDR.to_string());
        match RoomShow::start(PresetBank::default_dir(), target.clone()) {
            Ok(lighting) => {
                self.lighting = Some(lighting);
                self.lighting_retry_at = None;
                if self.lighting_controls.blackout_latched {
                    if let Some(lighting) = self.lighting.as_ref() {
                        lighting.set_power(false);
                    }
                }
                self.lighting_status = format!("desk→Art-Net {target}");
            }
            Err(error) => {
                self.lighting_retry_at = Some(Instant::now() + Duration::from_secs(2));
                self.lighting_status = format!("desk lock: {error}");
            }
        }
    }

    fn retry_lighting_if_due(&mut self) {
        if self.lighting.is_none()
            && self
                .lighting_retry_at
                .is_some_and(|retry_at| Instant::now() >= retry_at)
        {
            self.start_lighting();
        }
    }

    fn apc_item_count(&self) -> usize {
        match self.apc.surface {
            // The video surface pages the pad-matrix window, not the raw
            // catalog list.
            ApcSurface::Video => self.video_pad_total,
            ApcSurface::Music => self.music_model.tiles().len(),
            ApcSurface::Sfx => self.sfx_model.tiles().len(),
        }
    }

    fn apc_asset_at(&self, surface: ApcSurface, index: usize) -> Option<AssetId> {
        match surface {
            // `video_pad_assets` is the visible 40-pad window (already offset
            // by the bank); `index` arrives as bank + physical pad, so strip
            // the bank back off or banked pages trigger the wrong clip.
            ApcSurface::Video => {
                self.video_pad_assets.get(index.saturating_sub(self.apc.bank)).copied().flatten()
            }
            ApcSurface::Music => self.music_model.tiles().get(index).map(|t| t.asset),
            ApcSurface::Sfx => self.sfx_model.tiles().get(index).map(|t| t.asset),
        }
    }

    fn show_apc_surface(&mut self, cx: &mut Cx) {
        let page = match self.apc.surface {
            ApcSurface::Video => id!(video_page),
            ApcSurface::Music => id!(music_page),
            ApcSurface::Sfx => id!(sfx_page),
        };
        self.ui.page_flip(cx, ids!(pages)).set_active_page(cx, page.into());
        self.console_page = page.into();
        self.sync_mesh_liveness(cx);
        self.paint_tabs(cx, page);
        self.ui.redraw(cx);
    }

    /// The bar's chips wear which surface is up. The lane TABS are gone —
    /// the hot presets are the navigation now — so this only has to keep
    /// the chips and the GENERATE pill honest.
    fn paint_tabs(&mut self, cx: &mut Cx, _active: LiveId) {
        self.sync_lane_chips_ui(cx);
        self.paint_gen_tab(cx);
    }

    /// GENERATE is a DRAWER, not a mode: its only handle is the fold button
    /// on the panel edge, which stays visible when the panel is folded away
    /// so the drawer is still discoverable. Nothing of it sits in the bar.
    fn paint_gen_tab(&mut self, cx: &mut Cx) {
        self.ui
            .button(cx, ids!(gen_fold))
            .set_text(cx, if self.gen_panel_open { "⟨ GEN" } else { "GEN ⟩" });
    }

    fn set_gen_panel_open(&mut self, cx: &mut Cx, open: bool) {
        let split = self.ui.widget(cx, ids!(gen_split));
        if let Some(current) = split.borrow::<Splitter>().map(|s| s.align()) {
            if let SplitterAlign::FromA(width) = current {
                if width > 40.0 {
                    self.gen_panel_width = width;
                }
            }
        }
        self.gen_panel_open = open;
        let width = if open { self.gen_panel_width.max(240.0) } else { 0.0 };
        if let Some(mut splitter) = split.borrow_mut::<Splitter>() {
            splitter.set_align(SplitterAlign::FromA(width));
        }
        self.paint_gen_tab(cx);
        self.ui.redraw(cx);
    }

    // ---- beat-quantized program fades (armed on the device clock) ----------

    /// The current beat estimate. A playing music deck wins: its grid comes
    /// from a whole-file analysis and its playhead from the device clock, so
    /// it is strictly better than listening to the room. The capture-based
    /// detector stays the source for everything else.
    fn current_beat(&self) -> Option<BeatInfo> {
        let now = Instant::now();
        let Some(secs) = self.clock_secs(now) else {
            return Some(self.free_beat(now));
        };
        if !self.beat_clock.running() {
            return Some(self.free_beat(now));
        }
        let position = self.beat_clock.position_at(secs);
        let period = self.beat_clock.period_secs();
        if !(period > 0.0) || !period.is_finite() {
            return Some(self.free_beat(now));
        }
        // The next whole beat of the published position — which is the same
        // number every consumer would compute for itself, because the clock
        // is continuous and everybody reads it at the instant they ask.
        let until = ((position.ceil() - position) * period).max(0.0);
        Some(BeatInfo {
            bpm: self.beat_clock.bpm() as f32,
            // A coast keeps the confidence it earned: through a breakdown
            // the held grid is exactly what a fade should land on.
            confidence: self.clock_confidence,
            locked: true,
            period: Duration::from_secs_f64(period),
            next_beat: now + Duration::from_secs_f64(until),
            beat_index: (position.ceil() as i64).rem_euclid(BAR_BEATS as i64) as u64,
            beats_observed: position.max(0.0) as u64,
        })
    }

    /// The free-running clock floor (see `free_bpm`): a steady grid at the
    /// last known tempo, anchored so a source drop coasts without a phase
    /// pop. Full confidence — the house tempo is an operator statement,
    /// and beat-fitted loops must run from the very first cue.
    fn free_beat(&self, now: Instant) -> BeatInfo {
        let bpm = self.free_bpm.clamp(40.0, 300.0);
        let period = 60.0 / bpm;
        let elapsed = now.saturating_duration_since(self.free_anchor).as_secs_f64();
        let beats = (elapsed / period).floor().max(0.0);
        BeatInfo {
            bpm: bpm as f32,
            confidence: 1.0,
            locked: true,
            period: Duration::from_secs_f64(period),
            next_beat: self.free_anchor
                + Duration::from_secs_f64((beats + 1.0) * period),
            beat_index: (beats as u64 + 1) % BAR_BEATS,
            beats_observed: beats as u64,
        }
    }

    /// The clock's own time base: monotonic seconds since the first pump.
    fn clock_secs(&self, at: Instant) -> Option<f64> {
        Some(at.saturating_duration_since(self.clock_epoch?).as_secs_f64())
    }

    /// The raw source the clock should be following right now.
    ///
    /// Priority: the operator's tap, then a playing deck's whole-file grid,
    /// then the room. The flag says whether the source knows where the ONE
    /// is — a detector listening to a room does not, so its corrections are
    /// phase-only and the bar grouping stays where it was.
    fn beat_source(&self, snap: &SyncSnapshot) -> Option<(BeatInfo, bool, ClockSource)> {
        let detector = snap
            .beat
            .clone()
            .filter(|beat| beat.locked && beat.confidence >= CONF_QUANTIZE);
        let deck = self.deck_beat();
        let machine = match resolve_clock_source(
            false,
            self.decks.any_external_sync(),
            deck.is_some(),
            detector.is_some(),
        ) {
            ClockSource::External => detector.map(|beat| (beat, false, ClockSource::External)),
            ClockSource::Deck => deck.map(|beat| (beat, true, ClockSource::Deck)),
            ClockSource::Detector => detector.map(|beat| (beat, false, ClockSource::Detector)),
            _ => None,
        };
        if let Some(over) = self.beat_override.as_ref() {
            let base = machine.as_ref().map(|(beat, _, _)| beat);
            if let Some(beat) = over.beat(base, Instant::now()) {
                return Some((beat, true, ClockSource::Operator));
            }
        }
        machine
    }

    /// The published clock as a deck-sync leader, so an EXT deck can be
    /// tempo-matched to the room with the same arithmetic that matches it
    /// to the other deck.
    fn external_sync_view(&self) -> Option<SyncView> {
        if !self.beat_clock.running() {
            return None;
        }
        let secs = self.clock_secs(Instant::now())?;
        let bpm = self.beat_clock.bpm();
        if !(bpm > 1.0) || !bpm.is_finite() {
            return None;
        }
        let beat_secs = 60.0 / bpm;
        Some(SyncView {
            grid: TrackGrid {
                bpm,
                beat_secs,
                first_beat_secs: 0.0,
                downbeat_phase: 0,
                confidence: self.clock_confidence,
            },
            position_secs: self.beat_clock.position_at(secs) * beat_secs,
            rate: 1.0,
        })
    }

    /// Hold every EXT deck against the published clock.
    fn pump_external_sync(&mut self, cx: &mut Cx) {
        if !self.decks.any_external_sync() {
            return;
        }
        let Some(external) = self.external_sync_view() else { return };
        let cmds = self.decks.follow_external(&external);
        self.run_deck_cmds(cx, cmds);
    }

    /// Drive the published clock from whatever source is current.
    ///
    /// Nothing confident to follow is NOT a reason to stop: the clock coasts
    /// on the tempo and phase it last believed, which is what carries the
    /// visuals through a breakdown and puts them back on the grid the drop
    /// lands on.
    fn pump_beat_clock(&mut self, snap: &SyncSnapshot) {
        let now = Instant::now();
        let epoch = *self.clock_epoch.get_or_insert(now);
        let secs = now.saturating_duration_since(epoch).as_secs_f64();
        // While a deck owns the clock the detector has nothing to add: it
        // would only be listening to that deck's own output. Park it — but
        // NOT when a deck is following external audio, because then the
        // room is the one thing that knows where the beat is.
        if let Some(worker) = self.sync_worker.as_ref() {
            worker.set_suppressed(self.deck_beat().is_some() && !self.decks.any_external_sync());
        }
        match self.beat_source(snap) {
            Some((beat, bar_aware, source)) => {
                self.clock_confidence = beat.confidence;
                if let Some(target) = beat_target(&beat, now, bar_aware) {
                    self.clock_source = source;
                    self.beat_clock.discipline(secs, target);
                    return;
                }
                self.beat_clock.coast(secs);
            }
            // Nothing to follow: the clock keeps the grid it had, and only
            // says so — the source it was following is still the source it
            // is coasting on.
            None => self.beat_clock.coast(secs),
        }
    }

    /// Register a TAP press and turn the run into a clock override.
    ///
    /// ONE press is "the one is here": phase-only, the tempo underneath is
    /// left alone. FOUR rhythmic presses escalate to a full tap tempo. That
    /// overloading is deliberate — the first tap of a tempo run is on a beat
    /// the operator already means as beat one, so the same gesture serves
    /// both and there is no second button to aim at in the dark.
    fn tap_beat(&mut self, cx: &mut Cx) {
        let now_secs = cx.seconds_since_app_start();
        // Timestamp the press, not the frame it is handled in: the anchor is
        // what the whole room's downbeat will be built from.
        let anchor = Instant::now();
        let Some(clock) = self.tap_tempo.tap(now_secs) else { return };
        let over = BeatOverride {
            anchor,
            bpm: clock.bpm,
            leader: self.decks.sync_leader(),
            saw_unlock: false,
        };
        // Re-anchor the published clock NOW, at operator speed — a fast
        // slew, not a teleport, so nothing driven by the phase glitches.
        let base = self.current_beat();
        if let Some(beat) = over.beat(base.as_ref(), anchor) {
            if let Some((secs, target)) =
                self.clock_secs(anchor).zip(beat_target(&beat, anchor, true))
            {
                self.beat_clock.anchor(secs, target);
            }
        }
        self.beat_override = Some(over);
        self.tap_flash_secs = Some(now_secs);
        self.paint_chip(cx, ids!(beat_tap), true, None);
        // Every consumer reads the clock through `current_beat`, so the
        // fades, the loop rate-fit and the visual PLL all move with this.
        for slot in [SlotId::A, SlotId::B] {
            self.apply_slot_beat_sync(slot);
        }
        self.video_pump = cx.new_next_frame();
    }

    /// Manual tempo from the BPM value field: TAP authority, TEMPO ONLY —
    /// the downbeat stays where it is. The override's anchor is
    /// back-computed from the current clock's next beat, so the beat
    /// boundary is continuous and only the period changes (this is how a
    /// drifting DJ gets ridden into lock, 0.1 BPM at a time).
    fn set_manual_bpm(&mut self, cx: &mut Cx, bpm: f64) {
        let bpm = bpm.clamp(40.0, 300.0);
        let now = Instant::now();
        let period = Duration::from_secs_f64(60.0 / bpm);
        let anchor = self
            .current_beat()
            .as_ref()
            .and_then(|beat| {
                let beat = extrapolate_beat(beat, now);
                beat.next_beat.checked_sub(period)
            })
            .unwrap_or(now);
        self.beat_override = Some(BeatOverride {
            anchor,
            bpm: Some(bpm),
            leader: self.decks.sync_leader(),
            saw_unlock: false,
        });
        for slot in [SlotId::A, SlotId::B] {
            self.apply_slot_beat_sync(slot);
        }
        self.video_pump = cx.new_next_frame();
    }

    /// Hand the clock back to the machine. RESYNC is the operator saying
    /// "you work it out": the detector drops its grid and re-derives tempo
    /// and phase from the audio, and any tap override is dropped with it.
    fn resync_beat(&mut self, cx: &mut Cx) {
        self.beat_override = None;
        self.tap_tempo.clear();
        if let Some(worker) = self.sync_worker.as_ref() {
            worker.resync();
        }
        // There is nothing to anchor to at the moment of the press — the
        // detector is only just starting to look — so the operator's intent
        // is carried forward: the next confident grid is taken at operator
        // speed instead of eased in over bars.
        self.beat_clock.arm_operator();
        for slot in [SlotId::A, SlotId::B] {
            self.apply_slot_beat_sync(slot);
        }
        self.video_pump = cx.new_next_frame();
    }

    /// When the machine may take the clock back from a tap override.
    ///
    /// Only two things end an override on their own, and both mean "the
    /// music the operator tapped is gone":
    ///
    /// * the detector LOST its grid and has since re-locked confidently — a
    ///   new track, not the one that was tapped. A detector that merely
    ///   re-settles on the same track never qualifies, so the override
    ///   cannot be yanked out from under a running set;
    /// * a different deck took over the sync lead, bringing its own
    ///   whole-file grid with it.
    ///
    /// Anything else is the operator's job: RESYNC hands it back in a click.
    fn pump_beat_override(&mut self, lock_state: BeatLockState, confidence: f32) {
        let leader = self.decks.sync_leader();
        let Some(over) = self.beat_override.as_mut() else { return };
        if over.leader != leader && leader.is_some() {
            self.beat_override = None;
            self.tap_tempo.clear();
            return;
        }
        if detector_lost_grid(lock_state) {
            over.saw_unlock = true;
        }
        if detector_reclaims(lock_state, confidence, over.saw_unlock) {
            self.beat_override = None;
            self.tap_tempo.clear();
        }
    }

    /// Feed the chrome bar's beat cluster: the wave, the grid ruled over it,
    /// and the LED.
    ///
    /// Both widgets resolve their own phase at DRAW time, so all they are
    /// given here is a fixed reference converted once into the app clock —
    /// nothing is animated from this 20 Hz pump.
    fn refresh_beat_cluster(
        &mut self,
        cx: &mut Cx,
        snap: &SyncSnapshot,
        clock: Option<&BeatInfo>,
        flowing: bool,
    ) {
        let now = Instant::now();
        let now_secs = cx.seconds_since_app_start();
        let beat_ref = clock.filter(|beat| !beat.period.is_zero()).map(|beat| {
            let beat = extrapolate_beat(beat, now);
            let until = beat.next_beat.saturating_duration_since(now).as_secs_f64();
            views::BeatRef {
                next_beat_secs: now_secs + until,
                period_secs: beat.period.as_secs_f64(),
                next_index: beat.beat_index as u32,
                bar_beats: BAR_BEATS as u32,
                coasting: self.beat_clock.coasting(),
            }
        });
        snap.wave.unroll(&mut self.wave_cols);
        // The wave's right edge is the moment the newest column was
        // published, not this frame: the grid rides the same axis, so a
        // ruling on a transient means the clock really is on that transient.
        let stamp_secs = snap
            .wave_stamp
            .map(|at| now_secs - now.saturating_duration_since(at).as_secs_f64())
            .unwrap_or(now_secs);
        let live = flowing && self.loopback_selected;
        let cols = std::mem::take(&mut self.wave_cols);
        let wave = self.ui.widget(cx, ids!(beat_wave));
        if let Some(mut wave) = wave.borrow_mut::<views::VjBeatWave>() {
            wave.set_wave(cx, &cols, WAVE_HZ, stamp_secs, live);
            wave.set_beat(cx, beat_ref);
        }
        self.wave_cols = cols;
        let led = self.ui.widget(cx, ids!(beat_led));
        if let Some(mut led) = led.borrow_mut::<views::VjBeatLed>() {
            led.set_beat(cx, beat_ref);
        }
        // The TAP button lights on the press and falls back to chrome a
        // moment later — two paints per press, none in between.
        if let Some(at) = self.tap_flash_secs {
            if now_secs - at > TAP_FLASH_SECS {
                self.tap_flash_secs = None;
                self.paint_chip(cx, ids!(beat_tap), false, None);
            }
        }
    }

    /// Project the leading deck's analysed grid onto the host clock.
    fn deck_beat(&self) -> Option<BeatInfo> {
        let deck = self.decks.sync_leader()?;
        let state = self.decks.deck(deck);
        let grid = state.grid.filter(|grid| grid.has_grid())?;
        let (position, _duration, playing) = self.mixer.deck_position(deck);
        if !playing {
            return None;
        }
        let rate = state.rate.max(1e-6);
        // The audible beat period: source beats, compressed by the rate.
        let period_secs = grid.beat_secs / rate;
        if !period_secs.is_finite() || period_secs <= 0.01 {
            return None;
        }
        let beat = grid.beat_at(position);
        let next_beat = beat.floor() + 1.0;
        let until = ((grid.secs_at_beat(next_beat) - position) / rate).max(0.0);
        // A whole-file grid the analyser was sure about is worth more than a
        // live estimate; a shaky one reports what it actually scored.
        let confidence = if grid.confidence > 0.45 { 0.9 } else { grid.confidence };
        Some(BeatInfo {
            bpm: grid.effective_bpm(rate) as f32,
            confidence,
            locked: true,
            period: Duration::from_secs_f64(period_secs),
            next_beat: Instant::now() + Duration::from_secs_f64(until),
            beat_index: (next_beat + grid.downbeat_phase as f64).rem_euclid(BAR_BEATS as f64)
                as u64,
            beats_observed: beat.max(0.0) as u64,
        })
    }

    /// Plan for a fade requested right now: start instant, duration, label.
    fn fade_plan_now(&self) -> (Instant, f32, &'static str) {
        let beat = self.current_beat();
        match plan_external_fade(
            self.external_sync_enabled,
            beat.as_ref(),
            Instant::now(),
            self.fade_secs,
        ) {
            FadePlan::Immediate { secs } => (Instant::now(), secs, "now"),
            FadePlan::Quantized { fire_at, secs, kind } => (fire_at, secs, kind),
        }
    }

    /// Arm a ready cue's transition on the audio-device clock at the
    /// quantized start. With no beat lock the delay is zero — the first
    /// sample of the next rendered buffer — which is also the honest
    /// immediate path. The mixer releases audio sample-exactly; picture
    /// starts on the `BeginFade` command that `Started` confirms.
    fn arm_fade(
        &mut self,
        cx: &mut Cx,
        gen: CueGen,
        schedule: CueScheduleId,
        from: Option<SlotId>,
        to: SlotId,
    ) {
        // Rate-fit the destination BEFORE its first sample plays.
        self.apply_loop_fit(to);
        let (fire_at, secs, kind) = self.fade_plan_now();
        if let Err(error) =
            self.schedule_transition(gen, schedule, from, to, fire_at, secs, kind, 0)
        {
            // Stills/meshes have no decode bus until OpenSlot; never throw
            // the picture away because the audio mixer refused the fade.
            if matches!(
                self.slot_media[to.index()],
                SlotMedia::Still | SlotMedia::Mesh | SlotMedia::Billboard | SlotMedia::Effect
            ) {
                if self.trace_cue {
                    log!("cue: mixer refused schedule {schedule} ({error:?}); host cut to {to:?}");
                }
                let cmds = self.cue.start_armed(gen, schedule);
                self.program_mix = if to == SlotId::B { 1.0 } else { 0.0 };
                self.run_cue_cmds(cx, cmds);
                // The mixer never took this schedule, so no device clock
                // will ever report it Completed. Land it here or the cue
                // engine keeps both slots reserved for a fade that cannot
                // finish, and every later click parks in `WaitingSlot` —
                // clicking a tile silently stops cueing anything.
                let cmds = self.cue.fade_complete_for(schedule);
                self.run_cue_cmds(cx, cmds);
                self.sync_xfader_ui(cx, self.program_mix);
                self.video_pump = cx.new_next_frame();
                return;
            }
            let cmds = self.cue.cancel_armed(gen, schedule);
            self.run_cue_cmds(cx, cmds);
            log!("vj: failed to arm video transition: {error:?}");
            return;
        }
        self.video_pump = cx.new_next_frame();
    }

    /// Convert a host-clock plan into device output frames and arm it.
    #[allow(clippy::too_many_arguments)]
    fn schedule_transition(
        &mut self,
        gen: CueGen,
        schedule: CueScheduleId,
        from: Option<SlotId>,
        to: SlotId,
        fire_at: Instant,
        secs: f32,
        kind: &'static str,
        retries: u32,
    ) -> Result<(), VideoTransitionError> {
        let rate = self.mixer.output_sample_rate().unwrap_or(48_000.0);
        let fade_frames = (secs.max(0.01) as f64 * rate).round() as u64;
        let delay = fire_at.saturating_duration_since(Instant::now());
        let delay_frames = (delay.as_secs_f64() * rate).round() as u64;
        let target_frame = self.mixer.rendered_output_frames().saturating_add(delay_frames);
        self.mixer
            .schedule_video_transition_at(schedule, from, to, target_frame, fade_frames)?;
        self.armed_fade =
            Some(ArmedFadeUi { gen, schedule, from, to, fire_at, secs, kind, retries });
        Ok(())
    }

    /// Drive cue confirmation, program-mix settling and slot cleanup from
    /// the device-clock transition snapshot. All engine calls here are
    /// identity-checked, so repeated polling is idempotent.
    fn pump_transitions(&mut self, cx: &mut Cx) {
        let Some(snapshot) = self.mixer.video_transition_snapshot() else { return };
        // The mixer publishes phases into ONE slot, so a `Completed` the UI
        // thread never polled is overwritten by the next arm — and a fade
        // the mixer refused outright is never published at all. Either way
        // the engine would hold both slots for a transition that can no
        // longer finish. Once the mixer has moved on, land it here.
        if let Some(stale) = stale_fade_to_land(self.cue.active_fade(), snapshot.id) {
            let cmds = self.cue.fade_complete_for(stale);
            self.run_cue_cmds(cx, cmds);
        }
        match snapshot.phase {
            VideoTransitionPhase::Started => {
                if let Some((gen, schedule, _slot)) = self.cue.armed() {
                    if schedule == snapshot.id {
                        let cmds = self.cue.start_armed(gen, schedule);
                        self.run_cue_cmds(cx, cmds);
                    }
                }
            }
            VideoTransitionPhase::Completed => {
                // The mixer keeps reporting the finished fade; land it on the
                // fader exactly once, then the fader belongs to the operator
                // (re-landing every pump made the mixer un-draggable).
                if self.consumed_transition != Some(snapshot.id) {
                    self.consumed_transition = Some(snapshot.id);
                    self.program_mix = if snapshot.to == SlotId::B { 1.0 } else { 0.0 };
                    self.cue.set_fader(self.program_mix);
                    self.sync_xfader_ui(cx, self.program_mix);
                }
                let cmds = self.cue.fade_complete_for(snapshot.id);
                if !cmds.is_empty() {
                    self.run_cue_cmds(cx, cmds);
                }
                if self.armed_fade.is_some_and(|armed| armed.schedule == snapshot.id) {
                    self.armed_fade = None;
                }
            }
            VideoTransitionPhase::Missed => self.reschedule_missed(cx, snapshot.id),
            VideoTransitionPhase::Armed
            | VideoTransitionPhase::Cancelled
            | VideoTransitionPhase::Idle => {}
        }
    }

    /// The mixer never starts late — a `Missed` transition left the
    /// destination paused. Re-quantize onto the next boundary from the
    /// CURRENT grid; a lost lock (or repeated misses) degrades to an
    /// immediate start.
    fn reschedule_missed(&mut self, cx: &mut Cx, id: VideoTransitionId) {
        let Some(armed) = self.armed_fade else { return };
        if armed.schedule != id {
            return;
        }
        let Some((gen, schedule, _slot)) = self.cue.armed() else {
            self.armed_fade = None;
            return;
        };
        if schedule != id || gen != armed.gen {
            self.armed_fade = None;
            return;
        }
        let (fire_at, secs, kind) = if armed.retries >= 4 {
            (Instant::now(), armed.secs, "now")
        } else {
            self.fade_plan_now()
        };
        if let Err(error) = self.schedule_transition(
            gen,
            schedule,
            armed.from,
            armed.to,
            fire_at,
            secs,
            kind,
            armed.retries + 1,
        ) {
            let cmds = self.cue.cancel_armed(gen, schedule);
            self.run_cue_cmds(cx, cmds);
            log!("vj: could not reschedule missed transition: {error:?}");
            return;
        }
        self.video_pump = cx.new_next_frame();
    }

    /// Where a slot's playback rate should sit right now: a confident loop
    /// report fitted to a confident beat grid inside the safe range —
    /// else exactly 1.0. Low confidence on either side never warps.

    /// Drain finished loop analyses and re-evaluate both slots (also
    /// releases the rate back to 1.0 when the beat lock decays).
    fn pump_loop_reports(&mut self) {
        if let Some(results) = self.loop_results.as_ref() {
            let drained: Vec<(AssetRevisionId, LoopReport)> =
                results.lock().unwrap().drain(..).collect();
            for (revision, report) in drained {
                self.loop_reports.insert(revision, report);
            }
            // Bounded memory: keep only the slots' current revisions once
            // the map grows past a browse session's worth.
            if self.loop_reports.len() > 256 {
                let keep: Vec<AssetRevisionId> =
                    self.slot_scan.iter().flatten().copied().collect();
                self.loop_reports.retain(|revision, _| keep.contains(revision));
            }
        }
        for slot in [SlotId::A, SlotId::B] {
            self.apply_loop_fit(slot);
        }
    }

    fn set_video_paused(&mut self, cx: &mut Cx, paused: bool) {
        let Some(slot) = self.cue.live_slot() else { return };
        let Some(player) = self.players[slot.index()].as_mut() else { return };
        player.set_paused(paused);
        self.mixer.set_slot_paused(slot, paused);
        if paused {
            self.disarm_hazards(Some(cx));
        }
        self.refresh_program_lighting();
        // One pump updates both the demand-driven decoder and the lighting
        // state. A paused/stopped program must emit black instead of leaving
        // its last sampled color latched on the fixtures.
        self.video_pump = cx.new_next_frame();
    }

    fn toggle_video_playback(&mut self, cx: &mut Cx) {
        // Performer override: PLAY on an armed quantized fade re-arms it at
        // zero delay — it starts on the next rendered device buffer.
        if self.force_armed_fade_now(cx) {
            return;
        }
        let paused = self
            .cue
            .live_slot()
            .and_then(|slot| self.players[slot.index()].as_ref())
            .is_some_and(|player| !player.is_paused());
        self.set_video_paused(cx, paused);
    }

    /// Fire an already-armed program transition on the next audio callback.
    /// This is the explicit performer escape hatch shown in EXTERNAL SYNC.
    fn force_armed_fade_now(&mut self, cx: &mut Cx) -> bool {
        if let Some((gen, schedule, _slot)) = self.cue.armed() {
            if let Some(armed) = self.armed_fade {
                if armed.schedule == schedule && armed.gen == gen {
                    let _ = self.schedule_transition(
                        gen,
                        schedule,
                        armed.from,
                        armed.to,
                        Instant::now(),
                        armed.secs,
                        "now",
                        armed.retries,
                    );
                    self.video_pump = cx.new_next_frame();
                    return true;
                }
            }
        }
        false
    }

    fn stop_video_playback(&mut self, cx: &mut Cx) {
        let Some(slot) = self.cue.live_slot() else { return };
        self.mixer.flush_slot_audio(slot);
        if let Some(player) = self.players[slot.index()].as_mut() {
            player.set_paused(true);
            player.seek_fraction(0.0);
        }
        self.mixer.set_slot_paused(slot, true);
        self.disarm_hazards(Some(cx));
        self.refresh_program_lighting();
        self.video_pump = cx.new_next_frame();
    }

    fn dispatch_apc_action(&mut self, cx: &mut Cx, action: ApcAction) {
        match action {
            ApcAction::Pad { surface, pad, index, pressed } => {
                if !pressed {
                    self.release_apc_sfx_pad(pad);
                    return;
                }
                if surface == ApcSurface::Sfx {
                    // Recover safely if a previous release was lost before
                    // this physical pad was reused.
                    self.release_apc_sfx_pad(pad);
                }
                let Some(asset) = self.apc_asset_at(surface, index) else { return };
                match surface {
                    ApcSurface::Video => self.video_tile_clicked(cx, asset, false),
                    ApcSurface::Music => self.music_tile_clicked(cx, asset),
                    ApcSurface::Sfx => {
                        self.apc_sfx_holds.insert(pad, asset);
                        self.selected_pad = Some(asset);
                        let cmds = self.pads.press(asset, now_ms());
                        self.run_pad_cmds(cmds);
                        self.grids_dirty = true;
                    }
                }
            }
            ApcAction::Surface(_) => self.show_apc_surface(cx),
            ApcAction::VideoPlayPause => self.toggle_video_playback(cx),
            ApcAction::VideoStop => self.stop_video_playback(cx),
            ApcAction::Master(value) => {
                self.mixer.set_master(value);
                self.set_drop_slider(cx, ids!(master_slider), value as f64);
            }
            ApcAction::Crossfader(value) => {
                // On the music surface the hardware crossfader IS the deck
                // crossfader; everywhere else it stays the visual mix.
                if self.apc.surface == ApcSurface::Music {
                    let cmds = self.decks.set_crossfader(value);
                    self.run_deck_cmds(cx, cmds);
                    self.ui.slider(cx, ids!(xfader)).set_value(cx, value as f64);
                } else {
                    self.set_visual_mix(cx, value);
                }
            }
            // The first two channel strips are the two decks.
            ApcAction::ChannelFader { channel, value } => {
                let deck = match channel {
                    0 => Some(DeckId::A),
                    1 => Some(DeckId::B),
                    _ => None,
                };
                if let Some(deck) = deck {
                    let cmds = self.decks.set_gain(deck, value * 1.5);
                    self.run_deck_cmds(cx, cmds);
                    self.sync_deck_controls(cx);
                }
            }
            // Top knob row: each deck's three tone bands plus its filter.
            ApcAction::TrackKnob { index, value } => {
                let deck = if index < 4 { DeckId::A } else { DeckId::B };
                let cmds = match index % 4 {
                    3 => self.decks.set_filter(deck, value),
                    band => self.decks.set_eq(deck, band, value * 2.0),
                };
                self.run_deck_cmds(cx, cmds);
                self.sync_deck_knobs(cx, deck);
            }
            // Bottom knob row: each deck's four stem lanes.
            ApcAction::DeviceKnob { index, value } => {
                let deck = if index < 4 { DeckId::A } else { DeckId::B };
                // Hardware order matches the panel: drums, bass, vocals,
                // other; the engine's order is vocals-first.
                let stem = match index % 4 {
                    0 => 1,
                    1 => 2,
                    2 => 0,
                    _ => 3,
                };
                let cmds = self.decks.set_stem(deck, stem, value * 2.0);
                self.run_deck_cmds(cx, cmds);
                self.sync_deck_knobs(cx, deck);
            }
            ApcAction::BankChanged => {
                if self.apc.surface == ApcSurface::Video {
                    let widget = self.ui.widget(cx, ids!(video_grid));
                    if let Some(mut pads) = widget.borrow_mut::<VjPadMatrix>() {
                        pads.set_offset(cx, self.apc.bank);
                        self.apc.bank = pads.bank;
                    }
                    self.sync_video_pad_window(cx);
                } else {
                    let count = self.apc_item_count();
                    self.apc.clamp_bank(count);
                }
            }
        }
    }

    fn release_apc_sfx_pad(&mut self, pad: usize) {
        let Some(held) = self.apc_sfx_holds.remove(&pad) else {
            return;
        };
        let cmds = self.pads.release(held);
        self.run_pad_cmds(cmds);
    }

    fn pump_apc40(&mut self, cx: &mut Cx) {
        let mut actions = Vec::new();
        let mut pad_touched = false;
        for _ in 0..256 {
            let Some((port, data)) = self.midi_input.receive() else { break };
            // LEARN layer first, on EVERY port: an armed control binds to
            // the next CC that moves; a learned CC then drives its control
            // INSTEAD of whatever the hardwired surface meant by it.
            match self.midi_learn.midi(data.data) {
                Some(LearnEvent::Bound { control, channel, cc: number }) => {
                    log!("midi learn: {control} ← ch{channel} cc{number}");
                    self.save_midi_map();
                    self.sync_midi_learn_ui(cx);
                    continue;
                }
                Some(LearnEvent::Value { control, value }) => {
                    self.apply_learned(cx, &control, value);
                    continue;
                }
                None => {}
            }
            if !self.apc_input_ports.contains(&port) {
                continue;
            }
            if let Some(action) = self.apc.decode(data.data) {
                pad_touched |= matches!(action, ApcAction::Pad { .. });
                actions.push(action);
            }
            if !is_vj_reserved_midi(data.data) {
                if let Some(desk) = self.lighting.as_ref() {
                    let _ = desk.handle_midi(data.data);
                }
            }
        }
        for action in actions {
            self.dispatch_apc_action(cx, action);
        }
        // Generic mode momentarily owns pad LEDs while a finger is down.
        // Restore the authoritative VJ colors after both press and release.
        if pad_touched {
            self.apc_leds.invalidate();
        }
        self.sync_apc_leds();
    }

    fn sync_apc_leds(&mut self) {
        let count = self.apc_item_count();
        self.apc.clamp_bank(count);
        let mut frame = LedFrame { surface: self.apc.surface, ..Default::default() };
        for pad in 0..PAD_COUNT {
            let index = self.apc.bank + pad;
            // Resolve the same asset a press on this pad would trigger
            // (local-first mixed lists / the banked video window) so LEDs
            // never point at a different clip than the pad plays.
            let Some(asset) = self.apc_asset_at(self.apc.surface, index) else { continue };
            let tile = match self.apc.surface {
                ApcSurface::Video => self.video_model.tiles().iter().find(|t| t.asset == asset),
                ApcSurface::Music => self.music_model.tiles().iter().find(|t| t.asset == asset),
                ApcSurface::Sfx => self.sfx_model.tiles().iter().find(|t| t.asset == asset),
            };
            // The pad wears the clip's thumbnail colour once that is known.
            let color = tile
                .and_then(|tile| tile.revision)
                .and_then(|rev| self.thumb_leds.get(&rev).copied());
            let mut state = tile
                .map(|tile| match tile.state {
                    catalog::TileState::Ready => color.map_or(PadLed::Ready, PadLed::Color),
                    catalog::TileState::Failed(_) => PadLed::Failed,
                    catalog::TileState::Listed | catalog::TileState::Resolving => PadLed::Queued,
                })
                .unwrap_or(PadLed::Ready);
            match self.apc.surface {
                ApcSurface::Video => {
                    if self.cue.live().is_some_and(|item| item.asset == asset) {
                        state = color.map_or(PadLed::Live, PadLed::LiveColor);
                    } else if self.cue.next().is_some_and(|item| item.asset == asset) {
                        state = color.map_or(PadLed::Queued, PadLed::NextColor);
                    }
                }
                ApcSurface::Music => {
                    for deck in [DeckId::A, DeckId::B] {
                        let deck = self.decks.deck(deck);
                        let loaded = match &deck.load {
                            DeckLoad::Loading { item, .. }
                            | DeckLoad::Loaded { item }
                            | DeckLoad::Failed { item, .. } => Some(item.asset),
                            DeckLoad::Empty => None,
                        };
                        if loaded == Some(asset) {
                            state = if deck.playing { PadLed::Live } else { PadLed::Queued };
                        }
                    }
                }
                ApcSurface::Sfx => {
                    if self.pads.playing_voices(&asset) > 0 {
                        state = PadLed::Live;
                    }
                }
            }
            frame.pads[pad] = state;
        }
        frame.video_playing = self
            .cue
            .live_slot()
            .and_then(|slot| self.players[slot.index()].as_ref())
            .is_some_and(|player| !player.is_paused());
        let trace = std::env::var_os("VJ_TRACE_LED").is_some();
        for message in self.apc_leds.update(frame) {
            if trace {
                // ch = LED behaviour (solid/pulse/blink per the device's
                // protocol), velocity = palette index.
                log!(
                    "led: {:?} ch{} note {} vel {} pad #{:06X}",
                    self.apc_leds.model,
                    message[0] & 0x0f,
                    message[1],
                    message[2],
                    apc40::PAD_PALETTE[(message[2] & 0x7f) as usize]
                );
            }
            for port in &self.apc_output_ports {
                self.midi_output.send(Some(*port), MidiData { data: message });
            }
        }
    }

    // ---- command execution -------------------------------------------------

    fn run_cat_cmds(&mut self, surface: Surface, cmds: Vec<CatCmd>) {
        let Some(up) = self.up.as_mut() else { return };
        for cmd in cmds {
            match cmd {
                CatCmd::SearchPage { gen, slot, query, cursor, first } => {
                    // The page the operator is scrolling INTO ranks above the
                    // pages behind them, and a page outranks a thumbnail
                    // blob: an empty tile with no title is worse than a tile
                    // waiting for its picture.
                    if let Ok(id) = up.catalog.submit_with(
                        ClientRequest::CatalogSearch { query, cursor },
                        makepad_asset_client::SubmitOptions::newest_first(),
                    ) {
                        self.cat_reqs
                            .insert(id, CatPurpose::Page { surface, gen, slot, first });
                    }
                }
                CatCmd::FetchDetail { gen, asset } => {
                    if let Ok(id) = up.catalog.submit_with(
                        ClientRequest::AssetDetail { id: asset },
                        makepad_asset_client::SubmitOptions::newest_first(),
                    ) {
                        self.cat_reqs.insert(id, CatPurpose::Detail { surface, gen, asset });
                    }
                }
                CatCmd::FetchManifest { gen, asset, revision } => {
                    if let Ok(id) = up
                        .catalog
                        .submit(ClientRequest::FetchAssetManifest { rev: revision })
                    {
                        self.cat_reqs
                            .insert(id, CatPurpose::Manifest { surface, gen, asset, revision });
                    }
                }
                CatCmd::FetchThumb { revision, blob, len, .. } => {
                    if self.thumbs.contains_key(&revision) {
                        self.thumb_used.insert(revision, self.thumb_clock);
                        self.grids_dirty = true;
                        continue;
                    }
                    if self.thumb_inflight.contains(&revision) {
                        continue;
                    }
                    // Publication cap: refuse to even download an oversized
                    // thumbnail; the tile keeps its placeholder.
                    if len > media::MAX_THUMB_BYTES {
                        continue;
                    }
                    // Newest-first: the row under the operator's thumb is
                    // worth more than the twenty rows they have scrolled
                    // past, and the runtime will not guess that for us.
                    if let Ok(id) = up.catalog.submit_with(
                        ClientRequest::FetchBlob {
                            blob,
                            expected_len: Some(len),
                            pin: false,
                        },
                        makepad_asset_client::SubmitOptions::newest_first(),
                    ) {
                        self.cat_reqs.insert(id, CatPurpose::Thumb { revision });
                        self.thumb_inflight.insert(revision);
                        if self.trace_thumbs {
                            log!("thumb: fetching {revision}");
                        }
                    }
                }
            }
        }
        self.grids_dirty = true;
    }

    fn run_gen_cmds(&mut self, cmds: Vec<GenCmd>) {
        // A dead runtime must never eat commands in silence: the rows say
        // so, and the session-loss clock starts so the reconnect machinery
        // brings the connection back without anyone noticing twice.
        let now = now_ms();
        let mut runtime_down = false;
        {
            let Some(up) = self.up.as_mut() else { return };
            for cmd in cmds {
                match cmd {
                    GenCmd::FetchProfiles => {
                        if let Ok(id) = up.catalog.submit(ClientRequest::FetchJobProfiles {
                            domain: Some("video".to_string()),
                        }) {
                            self.cat_reqs.insert(id, CatPurpose::JobProfiles);
                        } else {
                            runtime_down = true;
                        }
                    }
                    GenCmd::Enqueue { tag, namespace, kind, body } => {
                        match up.catalog.submit(ClientRequest::EnqueueJob { namespace, kind, body }) {
                            Ok(id) => {
                                self.cat_reqs.insert(id, CatPurpose::JobEnqueue { tag });
                            }
                            Err(_) => {
                                runtime_down = true;
                                self.gen.enqueue_failed_at(
                                    tag,
                                    "connection lost — reconnecting, press Queue again".to_string(),
                                    Some(now),
                                );
                            }
                        }
                    }
                    GenCmd::PollStatus { job } => {
                        if let Ok(id) = up.catalog.submit(ClientRequest::FetchJobStatus { job }) {
                            self.cat_reqs.insert(id, CatPurpose::JobStatus { job });
                        } else {
                            runtime_down = true;
                        }
                    }
                    GenCmd::Cancel { job } => {
                        if let Ok(id) = up.catalog.submit(ClientRequest::CancelJob { job }) {
                            self.cat_reqs.insert(id, CatPurpose::JobCancel { job });
                        } else {
                            runtime_down = true;
                        }
                    }
                }
            }
        }
        if runtime_down && self.session_loss_since.is_none() {
            self.session_loss_since =
                Some(Instant::now() - Duration::from_secs_f64(SESSION_LOSS_GRACE_S));
        }
    }

    fn run_cue_cmds(&mut self, cx: &mut Cx, cmds: Vec<CueCmd>) {
        for cmd in cmds {
            if self.trace_cue {
                log!("cue: {cmd:?}");
            }
            match cmd {
                CueCmd::FetchMedia { gen, item } => {
                    let (lane, cancel) = self.video_plan.begin();
                    if let (Some(up), Some(stale)) = (self.up.as_ref(), cancel) {
                        // Supersede: abort the older transfer ON ITS LANE.
                        if let Some(runtime) = up.media.get(stale.lane) {
                            runtime.cancel(stale.request);
                        }
                    }
                    // A superseded pair (and its half-landed paths) dies with
                    // the click that started it.
                    self.cue_pair = item.sidecar.as_ref().map(|_| CuePair::begin(gen));
                    let Some(up) = self.up.as_mut() else { continue };
                    let Some(runtime) = up.media.get_mut(lane) else { continue };
                    if let Ok(id) = runtime.submit(ClientRequest::FetchBlob {
                        blob: item.media_blob,
                        expected_len: Some(item.media_len),
                        pin: false,
                    }) {
                        self.video_plan.submitted(lane, id, gen);
                        self.media_reqs.insert((lane, id), MediaPurpose::Cue { gen });
                    }
                    // Grouped sprite actor: the manifest text rides the same
                    // lane, tracked separately so the small text transfer
                    // never displaces the sheet in the latest-wins plan.
                    if let Some(sidecar) = item.sidecar.as_ref() {
                        if let Ok(id) = runtime.submit(ClientRequest::FetchBlob {
                            blob: sidecar.blob,
                            expected_len: Some(sidecar.len),
                            pin: false,
                        }) {
                            self.media_reqs
                                .insert((lane, id), MediaPurpose::CueSource { gen });
                        }
                    }
                }
                CueCmd::OpenSlot { slot, gen, item, path } => {
                    self.slot_held[slot.index()] = false;
                    self.players[slot.index()] = None;
                    // HOLD, don't flash: a clip→clip cue keeps the outgoing
                    // PICTURE on the deck until the new decoder's first
                    // frame lands (which overwrites this texture in place —
                    // a cut, never a black gap; the fade itself only starts
                    // after preroll, so it can't fade in the stale frame).
                    // Only decks whose picture actually lives in this
                    // texture may hold; a 3D/effect/empty deck clears, or a
                    // frame from two cues ago would resurrect.
                    let texture_backed = matches!(
                        self.slot_media[slot.index()],
                        SlotMedia::Video | SlotMedia::Still | SlotMedia::Billboard
                    );
                    if !texture_backed {
                        // No held picture to keep: the clip's cached
                        // THUMBNAIL stands in until the first decoded
                        // frame (borrowed — the frame writer replaces it,
                        // never writes into the tiles' texture).
                        self.slot_textures[slot.index()] =
                            self.thumbs.get(&item.revision).cloned();
                        self.slot_tex_borrowed[slot.index()] =
                            self.slot_textures[slot.index()].is_some();
                    }
                    // …and the deck answers the click THIS frame.
                    self.set_deck_busy(cx, slot, true);
                    self.clear_slot_flow(cx, slot);
                    self.light_samples[slot.index()] = None;
                    self.light_analyzers[slot.index()].reset();
                    self.clear_slot_mesh(cx, slot);
                    self.clear_slot_fx_content(cx, slot);
                    self.slot_media[slot.index()] = SlotMedia::Empty;
                    self.billboards[slot.index()] = None;
                    self.awaiting_preroll[slot.index()] = None;
                    // Grouped catalog sprite actor: packed sheet + manifest.
                    if let Some(manifest) =
                        item.sidecar.as_ref().and_then(|_| {
                            self.cue_pair.as_ref().and_then(|pair| pair.manifest_for(gen))
                        })
                    {
                        self.mixer.open_slot(slot);
                        self.mixer.set_slot_paused(slot, true);
                        self.slot_media[slot.index()] = SlotMedia::Billboard;
                        self.decode.submit(DecodeJob::BillboardSheet {
                            gen,
                            slot: slot.index(),
                            sheet: path,
                            manifest,
                        });
                        continue;
                    }
                    if path.extension().and_then(|e| e.to_str()) == Some("billboard") {
                        self.mixer.open_slot(slot);
                        self.mixer.set_slot_paused(slot, true);
                        self.slot_media[slot.index()] = SlotMedia::Billboard;
                        self.decode.submit(DecodeJob::Billboard {
                            gen,
                            slot: slot.index(),
                            path,
                        });
                        continue;
                    }
                    match item.media {
                        // A vjeffect cued AS CONTENT: evaluate the splash
                        // document into the slot's offscreen effect host —
                        // ready immediately, so the same arm/fade the clips
                        // ride starts at once (silent bus, like meshes).
                        MediaType::Text if item.kind == Some(AssetKind::VjEffect) => {
                            let source = match std::fs::read_to_string(&path) {
                                Ok(source) => source,
                                Err(error) => {
                                    let follow = self.cue.preroll_failed(
                                        slot,
                                        gen,
                                        format!("effect source unreadable: {error}"),
                                    );
                                    self.run_cue_cmds(cx, follow);
                                    continue;
                                }
                            };
                            let widget =
                                self.ui.widget(cx, Self::fx_content_host_path(slot));
                            let result = widget.borrow_mut::<VjFxSlotHost>().map(
                                |mut host| {
                                    let key = format!("vjfx_deck_{}", slot.index());
                                    let loaded = host.load(cx, &key, &source);
                                    host.set_enabled(cx, loaded.is_ok());
                                    loaded
                                },
                            );
                            match result {
                                Some(Ok(_)) => {
                                    self.mixer.open_slot(slot);
                                    self.mixer.set_slot_paused(slot, true);
                                    self.slot_media[slot.index()] = SlotMedia::Effect;
                                    self.slot_aspect[slot.index()] = 16.0 / 9.0;
                                    let cmds = self.cue.preroll_ready(slot, gen);
                                    self.run_cue_cmds(cx, cmds);
                                    self.video_pump = cx.new_next_frame();
                                }
                                Some(Err(error)) => {
                                    let follow =
                                        self.cue.preroll_failed(slot, gen, error);
                                    self.run_cue_cmds(cx, follow);
                                }
                                None => {}
                            }
                        }
                        MediaType::Glb => {
                            // Silent mixer bus so the existing fade arm
                            // accepts a picture-only destination.
                            self.mixer.open_slot(slot);
                            self.mixer.set_slot_paused(slot, true);
                            self.slot_media[slot.index()] = SlotMedia::Mesh;
                            self.slot_aspect[slot.index()] = 16.0 / 9.0;
                            // The body that will walk it, decided HERE: the
                            // alias names the engine family and the manifest
                            // anchors say what this map's metres are worth.
                            // The nav grid is probed with it on the worker
                            // and the walker is handed the same one back.
                            let cfg = self.world_walker_config(item.asset);
                            self.decode.submit(DecodeJob::SlotMesh {
                                gen,
                                slot: slot.index(),
                                path,
                                // A classic level publishes as kind World
                                // with a RenderGlb; a prop never does.
                                world: item.kind == Some(AssetKind::World),
                                cfg: Some(cfg),
                            });
                        }
                        MediaType::Ply => {
                            // Silent bus (picture-only source), same as meshes.
                            self.mixer.open_slot(slot);
                            self.mixer.set_slot_paused(slot, true);
                            self.slot_media[slot.index()] = SlotMedia::Splat;
                            self.slot_aspect[slot.index()] = 16.0 / 9.0;
                            self.awaiting_preroll[slot.index()] = Some(gen);
                            self.open_slot_splat(cx, slot, &path);
                            self.video_pump = cx.new_next_frame();
                        }
                        MediaType::Png | MediaType::Jpeg => {
                            self.mixer.open_slot(slot);
                            self.mixer.set_slot_paused(slot, true);
                            self.slot_media[slot.index()] = SlotMedia::Still;
                            self.decode.submit(DecodeJob::Still {
                                gen,
                                slot: slot.index(),
                                path,
                            });
                        }
                        _ => {
                            self.mixer.open_slot(slot);
                            // True preroll: player AND bus stay paused until
                            // the fade starts, so audio/video begin on one clock.
                            self.mixer.set_slot_paused(slot, true);
                            let path_text = path.to_string_lossy().to_string();
                            match SlotPlayer::open(
                                slot,
                                &path_text,
                                item.media,
                                self.mixer.clone(),
                                self.video_loop,
                                true,
                            ) {
                                Ok(mut player) => {
                                    // STICKY per-clip profile: the same
                                    // asset cues back with the loop/bounce/
                                    // trim/rate/mute hands it was left with;
                                    // a first-time clip gets the defaults
                                    // (muted, loop on, whole clip, ♪1).
                                    let index = slot.index();
                                    let profile = Self::load_clip_profile(&item.revision)
                                        .unwrap_or_default();
                                    self.slot_loop[index] = profile.loop_on;
                                    self.slot_pingpong[index] = profile.pingpong;
                                    // The chip ladder is 8/4/2/1 now; a
                                    // stale profile (old .5/1/2/4 scale)
                                    // just falls to the default — no
                                    // migration by design.
                                    self.slot_beat_rate[index] =
                                        if [8.0f32, 4.0, 2.0, 1.0]
                                            .contains(&profile.rate)
                                        {
                                            profile.rate
                                        } else {
                                            4.0
                                        };
                                    self.slot_beat_sync[index] = profile.sync;
                                    self.slot_sync_beats[index] = 1;
                                    self.slot_video_muted[index] = profile.muted;
                                    self.slot_trim[index] = profile.trim;
                                    player.set_mode(self.slot_play_mode(index));
                                    player.set_muted(profile.muted);
                                    player.set_trim(profile.trim.0, profile.trim.1);
                                    // Everything above repaints from shape.
                                    self.strip_shape[index] = None;
                                    self.slot_media[slot.index()] = SlotMedia::Video;
                                    self.players[slot.index()] = Some(player);
                                    self.apply_slot_beat_sync(slot);
                                    self.awaiting_preroll[slot.index()] = Some(gen);
                                    // FLOW WARP probe: parse + full decode on
                                    // a worker; the clip plays normally until
                                    // (and unless) the cache lands.
                                    self.slot_flow_gen[slot.index()] = gen;
                                    self.decode.submit(DecodeJob::FlowClip {
                                        gen,
                                        slot: slot.index(),
                                        path: path.clone(),
                                    });
                                    // Fresh loop-analysis lane for this revision.
                                    self.slot_scan[slot.index()] = Some(item.revision);
                                    self.applied_fit[slot.index()] = None;
                                    self.sig_states[slot.index()] = SigState::default();
                                    if let Some(tx) = self.loop_tx.as_ref() {
                                        let _ = tx.send(LoopScanCtl::Reset {
                                            slot: slot.index(),
                                            revision: Some(item.revision),
                                        });
                                    }
                                }
                                Err(error) => {
                                    let follow = self.cue.preroll_failed(slot, gen, error);
                                    self.run_cue_cmds(cx, follow);
                                }
                            }
                        }
                    }
                }
                CueCmd::ArmFade { gen, schedule, from, to } => {
                    self.arm_fade(cx, gen, schedule, from, to);
                }
                CueCmd::CancelArm { schedule } => {
                    self.mixer.cancel_video_transition(schedule);
                    if self.armed_fade.is_some_and(|armed| armed.schedule == schedule) {
                        self.armed_fade = None;
                    }
                }
                CueCmd::BeginFade { schedule, from: _, to } => {
                    // The device clock already released this slot's audio at
                    // the exact scheduled sample; start the picture clock
                    // and surface the program. With flow warp engaged the
                    // warp clock IS the picture clock: the decoder stays
                    // parked (flow playback is silent).
                    if self.flow_active(to.index()) {
                        self.flow_view(cx, to, |_cx, view| view.set_playing(true));
                    } else if let Some(player) = self.players[to.index()].as_mut() {
                        player.set_paused(false);
                    }
                    if self.armed_fade.is_some_and(|armed| armed.schedule == schedule) {
                        self.armed_fade = None;
                    }
                    self.refresh_program_lighting();
                    self.video_pump = cx.new_next_frame();
                    self.show_output_page(cx, id!(video_out_page));
                }
                CueCmd::HoldSlot { slot } => {
                    // THE STANDBY LAW ("both should keep looping and
                    // playing"): a crossfade parks NOTHING. The faded-out
                    // deck keeps decoding, looping, beat-fitting and
                    // holding phase off-program — its bus gain is already
                    // zero on that side of the fader, and a VJ fades
                    // BACK. (This used to pause player+bus+warp and drop
                    // the fit/scan identity: riding the fader back showed
                    // a frozen deck — "videos stop when we crossfade".)
                    self.slot_held[slot.index()] = true;
                    self.awaiting_preroll[slot.index()] = None;
                }
                CueCmd::CloseSlot { slot } => {
                    // An armed fade whose slot goes away must never fire.
                    if self.armed_fade.is_some_and(|armed| armed.to == slot) {
                        self.armed_fade = None;
                    }
                    self.slot_held[slot.index()] = false;
                    self.players[slot.index()] = None; // detached teardown
                    self.awaiting_preroll[slot.index()] = None;
                    self.slot_textures[slot.index()] = None;
                    self.clear_slot_flow(cx, slot);
                    self.slot_media[slot.index()] = SlotMedia::Empty;
                    self.billboards[slot.index()] = None;
                    self.clear_slot_mesh(cx, slot);
                    self.clear_slot_fx_content(cx, slot);
                    self.light_samples[slot.index()] = None;
                    self.slot_scan[slot.index()] = None;
                    self.applied_fit[slot.index()] = None;
                    self.sig_states[slot.index()] = SigState::default();
                    if let Some(tx) = self.loop_tx.as_ref() {
                        let _ = tx.send(LoopScanCtl::Reset {
                            slot: slot.index(),
                            revision: None,
                        });
                    }
                    self.mixer.close_slot(slot);
                    self.refresh_program_lighting();
                }
            }
        }
    }

    fn run_deck_cmds(&mut self, cx: &mut Cx, cmds: Vec<DeckCmd>) {
        for cmd in cmds {
            match cmd {
                DeckCmd::LoadTrack { deck, gen, item } => {
                    // A new load supersedes whatever the last one was still
                    // fetching: stale files landing later find no pending set
                    // and are dropped.
                    self.deck_side_channels[deck.index()] = None;
                    // A local file never goes near the store: it decodes
                    // straight off disk on the same worker pool.
                    if let Some(path) = self.local_by_asset.get(&item.asset).cloned() {
                        let media = match path
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|e| e.to_ascii_lowercase())
                            .as_deref()
                        {
                            Some("wav") => MediaType::Wav,
                            Some("ogg") | Some("oga") => MediaType::Ogg,
                            Some("mp3") => MediaType::Mp3,
                            _ => MediaType::Mp4,
                        };
                        self.decode.submit(DecodeJob::Deck { deck, gen, path, media });
                        continue;
                    }
                    let Some(up) = self.up.as_mut() else { continue };
                    let Some(runtime) = up.media.get_mut(AUDIO_LANE) else { continue };
                    if let Ok(id) = runtime.submit(ClientRequest::FetchBlob {
                        blob: item.media_blob,
                        expected_len: Some(item.media_len),
                        pin: false,
                    }) {
                        self.media_reqs.insert(
                            (AUDIO_LANE, id),
                            MediaPurpose::Deck { deck, gen, media: item.media },
                        );
                    }
                    // Whatever the store already knows about this track rides
                    // along with the audio: a few hundred kilobytes of stems
                    // instead of a third of the track's duration on the GPU.
                    self.begin_side_channel_fetch(deck, gen, &item);
                }
                DeckCmd::InstallTrack { deck } => {
                    let key = self
                        .deck_incoming
                        .keys()
                        .find(|(d, _)| *d == deck.index())
                        .copied();
                    if let Some(key) = key {
                        if let Some((pcm, peaks)) = self.deck_incoming.remove(&key) {
                            self.mixer.install_deck(deck, pcm.clone());
                            self.deck_tracks[deck.index()] = Some((pcm.clone(), peaks));
                            // The waveform and the beat grid come from a
                            // worker: never the UI thread, never the audio
                            // callback. Until it answers the deck shows the
                            // track with no grid, which is honest.
                            self.deck_analysis[deck.index()] = None;
                            self.deck_zoom_tex[deck.index()] = None;
                                            self.deck_stems[deck.index()] = None;
                            self.deck_stem_tex[deck.index()] = None;
                            self.deck_stem_tiles[deck.index()] = Vec::new();
                            // The old track's words must not sit over the new
                            // one; the separation worker re-reports coverage
                            // for this deck and the cache hit puts them back
                            // in a file read if this track has any.
                            self.deck_lyrics[deck.index()] = None;
                            self.deck_karaoke[deck.index()] = None;
                            self.deck_lyrics_status[deck.index()] = String::new();
                            // Until the new track's coverage report names its
                            // digest, this deck matches no cached transcript.
                            self.deck_track_digest[deck.index()] = None;
                            self.mixer.clear_deck_stems(deck);
                            self.submit_analysis(deck, pcm.clone());
                            // Fetch or compute, decided when the track was
                            // clicked: a deck whose side-channel fetch is
                            // armed for this generation never loads the
                            // separation model at all. The fetch's own
                            // failure paths fall back here.
                            if self.side_channels_armed(deck, key.1) {
                                self.deck_stem_status[deck.index()] =
                                    "stems: fetching…".to_string();
                                self.deck_stem_busy[deck.index()] = Some(true);
                                self.try_start_side_channels(deck, key.1);
                            } else {
                                self.submit_separation(deck, pcm);
                            }
                        }
                    }
                }
                DeckCmd::SetPlaying { deck, playing } => self.mixer.set_deck_playing(deck, playing),
                DeckCmd::SeekFraction { deck, fraction } => {
                    self.mixer.seek_deck_fraction(deck, fraction)
                }
                DeckCmd::SetLoop { deck, loop_on } => self.mixer.set_deck_loop(deck, loop_on),
                DeckCmd::SetMute { deck, muted } => self.mixer.set_deck_mute(deck, muted),
                DeckCmd::SetGain { deck, gain } => self.mixer.set_deck_gain(deck, gain),
                DeckCmd::SetCrossfader { position } => self.mixer.set_crossfader(position),
                DeckCmd::FadeCrossfader { position, secs } => {
                    self.mixer.fade_crossfader(position, secs)
                }
                DeckCmd::SetCurve { curve } => self.mixer.set_curve(curve),
                // ---- music mode: tempo, scratch, tone, stems ----
                DeckCmd::SetRate { deck, rate } => self.mixer.set_deck_rate(deck, rate),
                DeckCmd::SeekSeconds { deck, secs } => {
                    self.mixer.seek_deck_seconds(deck, secs)
                }
                DeckCmd::Scratch { deck, motion } => self.mixer.scratch_deck(deck, motion),
                DeckCmd::SetKeylock { deck, on } => self.mixer.set_deck_keylock(deck, on),
                DeckCmd::SetEqBand { deck, band, gain } => {
                    self.mixer.set_deck_eq_band(deck, band, gain)
                }
                DeckCmd::SetFilter { deck, position } => {
                    self.mixer.set_deck_filter(deck, position)
                }
                DeckCmd::SetStemGain { deck, stem, gain } => {
                    self.mixer.set_deck_stem_gain(deck, stem, gain)
                }
                DeckCmd::SwapVoices => {
                    self.mixer.swap_decks();
                    self.deck_tracks.swap(0, 1);
                    self.deck_analysis.swap(0, 1);
                    self.deck_zoom_tex.swap(0, 1);
                            self.deck_loop_beats.swap(0, 1);
                    self.sync_deck_controls(cx);
                }
            }
        }
    }

    /// Mirror engine deck state into the toggle/slider widgets (after swap,
    /// and at install) so the controls always show the deck they control.
    fn sync_deck_controls(&mut self, cx: &mut Cx) {
        for (deck, gain_id, pitch_id) in [
            (DeckId::A, ids!(deck_a_gain), ids!(deck_a_pitch)),
            (DeckId::B, ids!(deck_b_gain), ids!(deck_b_pitch)),
        ] {
            let state = self.decks.deck(deck);
            let range = state.pitch_range.fraction();
            self.ui.slider(cx, gain_id).set_value(cx, state.gain as f64);
            self.ui
                .slider(cx, pitch_id)
                .set_value(cx, (state.pitch / range).clamp(-1.0, 1.0));
        }
        let pos = self.decks.crossfader as f64;
        self.ui.slider(cx, ids!(xfader)).set_value(cx, pos);
    }

    /// Push a deck's tone/stem knob positions back onto the surface, so the
    /// hardware and the screen never disagree.
    fn sync_deck_knobs(&mut self, cx: &mut Cx, deck: DeckId) {
        let ids = MusicDeckIds::for_deck(deck);
        let state = self.decks.deck(deck);
        let eq = state.eq;
        let filter = state.filter;
        let stems = state.stem_gain;
        for (band, knob) in ids.eq_knobs.iter().enumerate() {
            self.ui.slider(cx, knob).set_value(cx, eq[band] as f64);
        }
        self.ui.slider(cx, ids.filter).set_value(cx, filter as f64);
        for (stem, knob) in ids.stem_knobs.iter().enumerate() {
            self.ui.slider(cx, knob).set_value(cx, stems[stem] as f64);
        }
    }

    /// Hand a freshly decoded track to the analysis worker. The key is the
    /// content digest, so a track that has been on a deck before comes back
    /// from its sidecar instead of being analysed again.
    fn submit_analysis(&mut self, deck: DeckId, pcm: Arc<TrackPcm>) {
        let state = self.decks.deck(deck);
        let Some(item) = state.item() else { return };
        let key = match self.local_by_asset.get(&item.asset) {
            Some(path) => AnalysisKey::from_path(path),
            None => AnalysisKey::from_blob(item.media_blob),
        };
        self.analysis.submit(AnalysisJob {
            deck,
            gen: state.load_gen,
            key,
            pcm,
        });
    }

    fn run_pad_cmds(&mut self, cmds: Vec<PadCmd>) {
        for cmd in cmds {
            match cmd {
                PadCmd::LoadPad { pad, gen, item } => {
                    if self.pcm_store.contains_key(&item.revision) {
                        let follow = self.pads.load_ready(pad, gen, now_ms());
                        self.run_pad_cmds(follow);
                        continue;
                    }
                    let Some(up) = self.up.as_mut() else { continue };
                    let Some(runtime) = up.media.get_mut(AUDIO_LANE) else { continue };
                    if let Ok(id) = runtime.submit(ClientRequest::FetchBlob {
                        blob: item.media_blob,
                        expected_len: Some(item.media_len),
                        pin: false,
                    }) {
                        self.media_reqs.insert(
                            (AUDIO_LANE, id),
                            MediaPurpose::Pad {
                                pad,
                                gen,
                                revision: item.revision,
                                media: item.media,
                            },
                        );
                    }
                }
                PadCmd::StartVoice { voice, revision } => {
                    if let Some(pcm) = self.pcm_store.get(&revision) {
                        self.mixer.start_voice(voice, pcm.clone());
                    }
                }
                PadCmd::StopVoice { id } => self.mixer.stop_voice(id),
                PadCmd::SetPadVoicesGain { pad, gain } => {
                    self.mixer.set_pad_voices_gain(pad, gain)
                }
            }
        }
    }

    // ---- polling ------------------------------------------------------------

    fn pump(&mut self, cx: &mut Cx) {
        // The import worker reports here: cheap when idle, and it must be
        // drained on the UI tick rather than blocking anything.
        self.pump_import(cx);
        // Lazy vjeffect thumbnails: feed the one-at-a-time offscreen
        // renderer and land its finished sheets in the thumb decode lane.
        self.pump_fx_thumbs(cx);
        // The output window starts CLOSED (cleaner for testing; the OUTPUT
        // button reopens it). Done here, not in Startup, because the native
        // window may not exist yet at Startup; `VJ_OUTPUT=1` keeps the old
        // open-at-launch behaviour. One-shot.
        if self.output_close_on_start {
            let wanted_open = std::env::var("VJ_OUTPUT").is_ok_and(|v| v == "1");
            let output = self.ui.window(cx, ids!(output_window));
            if wanted_open {
                self.output_close_on_start = false;
            } else if output.window_id().is_some() {
                self.close_output_window(cx);
                self.output_close_on_start = false;
            }
        }
        self.retry_lighting_if_due();
        // Refresh the worker watchdog and, only while the physical button is
        // held, its shorter hazardous-output heartbeat.
        self.publish_lighting_controls();
        self.pump_apc40(cx);
        self.pump_session(cx);
        self.pump_chat(cx);
        self.pump_subscriber(cx);
        self.pump_catalog_runtime(cx);
        self.pump_media_lanes(cx);
        self.pump_decodes(cx);
        for deck in self.mixer.drain_ended_decks() {
            let cmds = self.decks.track_ended(deck);
            self.run_deck_cmds(cx, cmds);
        }
        self.pump_analysis(cx);
        self.pump_stems(cx);
        self.pump_lyrics(cx);
        self.pump_side_channel_writeback();
        self.observe_decks();
        self.sync_mesh_liveness(cx);
        self.schedule_music_frame(cx);
        for voice in self.mixer.drain_ended_voices() {
            self.pads.voice_ended(voice);
        }
        // Slot pre-rolls: video players report readiness; splat scenes are
        // ready once ViewSplat built the GPU scene (stills/meshes complete
        // from their decode jobs).
        for slot in [SlotId::A, SlotId::B] {
            let Some(gen) = self.awaiting_preroll[slot.index()] else { continue };
            if self.slot_media[slot.index()] == SlotMedia::Splat {
                if self.slot_splat_ready(cx, slot) {
                    self.awaiting_preroll[slot.index()] = None;
                    self.show_output_page(cx, id!(video_out_page));
                    let cmds = self.cue.preroll_ready(slot, gen);
                    self.run_cue_cmds(cx, cmds);
                    self.video_pump = cx.new_next_frame();
                }
                continue;
            }
            if self.slot_media[slot.index()] != SlotMedia::Video {
                continue;
            }
            let (ready, failure) = match self.players[slot.index()].as_ref() {
                Some(p) => (p.preroll_ready(), p.failure()),
                None => (false, Some("player vanished".to_string())),
            };
            if let Some(error) = failure {
                self.awaiting_preroll[slot.index()] = None;
                let cmds = self.cue.preroll_failed(slot, gen, error);
                self.run_cue_cmds(cx, cmds);
            } else if ready {
                self.awaiting_preroll[slot.index()] = None;
                let cmds = self.cue.preroll_ready(slot, gen);
                self.run_cue_cmds(cx, cmds);
            }
        }
        self.pump_transitions(cx);
        self.pump_loop_reports();
        self.update_status_ui(cx);
        // The published clock has just been advanced; hold every EXT deck
        // against it before anything else reads a deck position.
        self.pump_external_sync(cx);
        if self.grids_dirty {
            self.grids_dirty = false;
            self.rebuild_grids(cx);
            // The pads follow the grid window (delta-compressed, so a
            // quiet grid sends nothing).
            self.sync_apc_leds();
        }
    }

    fn pump_session(&mut self, cx: &mut Cx) {
        let Some(connector) = self.connector.as_mut() else { return };
        for msg in connector.poll() {
            match msg {
                SessionMsg::Status(status) => {
                    self.status_text = match status {
                        SessionStatus::Discovering => "discovering asset server…".to_string(),
                        SessionStatus::Connecting { server } => {
                            format!("connecting {server}…")
                        }
                        SessionStatus::Connected { server } => format!("connected {server}"),
                        SessionStatus::Retrying { error, in_secs } => {
                            format!("connection failed: {error} — retrying in {in_secs}s")
                        }
                    };
                }
                SessionMsg::Up(up) => {
                    self.status_text = format!("connected {}", up.server_label);
                    self.up = Some(*up);
                    // Seed the bundled vjeffect preset library into the local
                    // store, publish-if-absent (idempotent; a user-edited
                    // revision under a seeded alias is never touched). Runs
                    // detached — the UI never waits on it.
                    if !self.fx_presets_seeded {
                        self.fx_presets_seeded = true;
                        let endpoints = self.up.as_ref().unwrap().endpoints;
                        let token = self.up.as_ref().unwrap().token.clone();
                        let cache = service::session_config_from_env()
                            .cache_parent
                            .join("cache-vjfx-seed");
                        std::thread::spawn(move || {
                            let _ = std::fs::create_dir_all(&cache);
                            let mut cfg = makepad_asset_client::ClientConfig::new(cache);
                            cfg.token = token;
                            match makepad_asset_client::AssetClient::connect(cfg, endpoints, None)
                            {
                                Ok(mut client) => {
                                    let report =
                                        crate::effects::seed::seed_presets(&mut client);
                                    log!(
                                        "vjfx preset seeding: {} present, {} published, {} updated, {} retagged, {} failed{}",
                                        report.present,
                                        report.published,
                                        report.updated,
                                        report.retagged,
                                        report.failed.len(),
                                        report
                                            .failed
                                            .first()
                                            .map(|(a, e)| format!(" (first: {a}: {e})"))
                                            .unwrap_or_default()
                                    );
                                    // ACCEPTANCE, every boot: the store lists
                                    // the whole shipped library or this line
                                    // says so, loudly.
                                    match crate::effects::seed::library_check(&mut client) {
                                        Ok((listed, bundled)) if (listed as usize) < bundled => {
                                            log!(
                                                "vjfx library check FAILED: store lists {listed} vjeffect rows of {bundled} bundled — seeding is not reaching the store"
                                            );
                                        }
                                        Ok((listed, bundled)) => {
                                            log!("vjfx library check: {listed} listed / {bundled} bundled");
                                        }
                                        Err(error) => {
                                            log!("vjfx library check unavailable: {error}");
                                        }
                                    }
                                }
                                Err(error) => {
                                    log!("vjfx preset seeding skipped: {error}");
                                }
                            }
                        });
                    }
                    if !self.gen_panel_loaded {
                        self.gen_panel_loaded = true;
                        self.load_gen_panel(cx);
                        // Dev/automation hook: VJ_IMPORT_PATH=<dir|file>
                        // imports on first connect — headless rigs have no
                        // native picker to click.
                        if let Ok(path) = std::env::var("VJ_IMPORT_PATH") {
                            if !path.trim().is_empty() {
                                self.import.set_path(path.trim().to_string());
                                self.start_import(cx);
                            }
                        }
                    }
                    // The grid BOOTS on its default lane (VIDEO) — the
                    // chip is lit, so the query must match it; without
                    // this the model's everything-default listed the whole
                    // store under a lit VIDEO chip.
                    self.set_lane(cx, self.grid_lane);
                    for surface in SURFACES {
                        let cmds = self.model(surface).refresh();
                        self.run_cat_cmds(surface, cmds);
                    }
                    let cmds = self.gen.ensure_profiles();
                    self.run_gen_cmds(cmds);
                }
            }
        }
        let _ = cx;
    }

    /// The chat lives on the STORE's chat broker — the server picks the
    /// serving box and executes the catalog/operation tools itself — so all
    /// this app has to do is hand over the session once it is up.
    fn pump_chat(&mut self, cx: &mut Cx) {
        if !self.chat.is_linked() {
            if let Some(up) = &self.up {
                let cache = service::session_config_from_env()
                    .cache_parent
                    .join("cache-chat");
                self.chat.connect(up.endpoints, up.token.clone(), cache);
                // The pane says "waiting for the asset server" until
                // something redraws it, and the feed only marks itself
                // dirty once a turn runs — so the line would sit there
                // lying until the operator typed.
                self.refresh_chat_ui(cx);
            }
        }
        if self.chat.take_dirty() {
            self.refresh_chat_ui(cx);
            self.ui.widget(cx, ids!(chat_list)).redraw(cx);
        }
    }

    fn refresh_chat_ui(&mut self, cx: &mut Cx) {
        let mut status = ChatData::status();
        let activity = ChatData::activity();
        if !activity.is_empty() && !status.contains(&activity) {
            if status.is_empty() {
                status = activity;
            } else {
                status = format!("{status} · {activity}");
            }
        }
        // The live number has ONE home while a reply streams: this strip.
        // (The landed message keeps its own average as a footnote.) It is
        // the serving box's real generation rate, `· thinking` included.
        if let Some(rate) = ChatData::live_rate_label() {
            status = if status.is_empty() { rate } else { format!("{status} · {rate}") };
        }
        self.ui.label(cx, ids!(chat_status)).set_text(cx, &status);
        let streaming = ChatData::is_streaming();
        self.ui
            .button(cx, ids!(chat_cancel_btn))
            .set_visible(cx, streaming);
        self.ui.widget(cx, ids!(chat_list)).redraw(cx);
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
        self.chat.send(text, Vec::new());
        self.refresh_chat_ui(cx);
        self.ui.redraw(cx);
    }

    fn pump_subscriber(&mut self, cx: &mut Cx) {
        let Some(up) = self.up.as_mut() else { return };
        let events = up.subscriber.poll();
        for event in events {
            match event {
                CatalogSubscriptionEvent::Ready { .. } => self.note_session_ok(),
                CatalogSubscriptionEvent::Events { events, .. } => {
                    self.note_session_ok();
                    for ev in events {
                        self.video_model.event_touch(ev.content_kind);
                        self.music_model.event_touch(ev.content_kind);
                        self.sfx_model.event_touch(ev.content_kind);
                        self.mesh_model.event_touch(ev.content_kind);
                        // Publication marks matching generation rows —
                        // event-driven, never a whole-catalog poll.
                        if let Some(asset) = ev.asset_id {
                            if ev.kind.removes_content() {
                                // A retired asset's tile leaves the grid
                                // immediately and the grid compacts — no
                                // dead holes where garbage got pulled.
                                self.video_model.event_remove(asset);
                                self.music_model.event_remove(asset);
                                self.sfx_model.event_remove(asset);
                                self.mesh_model.event_remove(asset);
                                self.grids_dirty = true;
                            } else {
                                self.gen.catalog_published(asset);
                            }
                        }
                    }
                }
                CatalogSubscriptionEvent::ResyncRequired { .. } => {
                    self.video_model.event_touch(None);
                    self.music_model.event_touch(None);
                    self.sfx_model.event_touch(None);
                    self.mesh_model.event_touch(None);
                }
                CatalogSubscriptionEvent::Retry { error, retry_in_ms } => {
                    self.status_text =
                        format!("event feed retry in {}ms: {error}", retry_in_ms);
                    if is_session_loss(&error) {
                        self.note_session_failure(cx);
                    }
                }
            }
        }
    }

    fn pump_catalog_runtime(&mut self, cx: &mut Cx) {
        let events = match self.up.as_mut() {
            Some(up) => up.catalog.poll(),
            None => return,
        };
        for event in events {
            let id = event.id();
            match event {
                ClientEvent::Started { .. } | ClientEvent::Progress { .. } => {}
                ClientEvent::Done { output, .. } => {
                    self.note_session_ok();
                    let Some(purpose) = self.cat_reqs.remove(&id) else { continue };
                    self.catalog_done(cx, purpose, output);
                }
                ClientEvent::Failed { error, .. } => {
                    if is_session_loss(&error) {
                        self.note_session_failure(cx);
                    }
                    let Some(purpose) = self.cat_reqs.remove(&id) else { continue };
                    match purpose {
                        CatPurpose::Page { surface, gen, slot, .. } => {
                            let cmds = self.model(surface).page_failed(gen, slot, error.to_string());
                            self.run_cat_cmds(surface, cmds);
                            self.grids_dirty = true;
                        }
                        CatPurpose::Detail { surface, gen, asset }
                        | CatPurpose::Manifest { surface, gen, asset, .. } => {
                            let cmds =
                                self.model(surface).resolve_failed(gen, asset, error.to_string());
                            self.run_cat_cmds(surface, cmds);
                        }
                        CatPurpose::Thumb { revision } => {
                            // A failed blob is NOT cached as "no thumbnail":
                            // clearing the in-flight mark lets the next grid
                            // rebuild ask again, so a 404 straight after a
                            // republish heals itself.
                            self.thumb_inflight.remove(&revision);
                            if self.trace_thumbs {
                                log!("thumb: fetch FAILED {revision}: {error}");
                            }
                        }
                        CatPurpose::FxSource { revision, .. } => {
                            // Transient: the pump asks again on a later tick.
                            self.fx_source_inflight.remove(&revision);
                            log!("fx thumb: source fetch FAILED {revision}: {error}");
                        }
                        CatPurpose::FxSlotSource { slot, revision, .. } => {
                            if self.fx_slot_inflight[slot.index()] == Some(revision) {
                                self.fx_slot_inflight[slot.index()] = None;
                            }
                            self.fx_slots.slot_mut(slot).note =
                                Some("fetch failed".to_string());
                            self.sync_fx_slots_ui(cx);
                            log!("fx slot {:?}: source fetch FAILED {revision}: {error}", slot);
                        }
                        CatPurpose::JobProfiles => {
                            self.gen.profiles_failed(error.to_string());
                        }
                        CatPurpose::JobEnqueue { tag } => {
                            self.gen.enqueue_failed_at(tag, error.to_string(), Some(now_ms()));
                        }
                        CatPurpose::JobStatus { job } => {
                            self.gen.status_failed_at(
                                job,
                                error.to_string(),
                                Some(now_ms()),
                            );
                        }
                        CatPurpose::JobCancel { .. } => {}
                        CatPurpose::SideChannelPublish { asset } => {
                            // A store that will not take them (no write
                            // capability, an older server) is not an error
                            // the operator can act on mid-set: one line, and
                            // the asset stays marked so nothing retries.
                            log!("side-channels: {asset} refused: {error}");
                            self.side_channel_publish_settled(asset);
                        }
                    }
                }
            }
        }
    }

    fn catalog_done(&mut self, cx: &mut Cx, purpose: CatPurpose, output: ClientOutput) {
        match (purpose, output) {
            (
                CatPurpose::Page { surface, gen, slot, first },
                ClientOutput::CatalogPage(page),
            ) => {
                let hits = page
                    .hits
                    .into_iter()
                    .map(|h| catalog::HitRow {
                        updated_ms: h.updated_ms,
                        asset: h.asset_id,
                        title: if h.title.is_empty() {
                            h.asset_id.to_string()
                        } else {
                            h.title
                        },
                        alias: h.alias.map(|a| a.as_str().to_string()),
                        live: h.live,
                        kind: h.kind,
                    })
                    .collect();
                let cmds =
                    self.model(surface).page_arrived(gen, slot, first, hits, page.total, page.next);
                self.run_cat_cmds(surface, cmds);
            }
            (CatPurpose::Detail { surface, gen, asset }, ClientOutput::AssetDetail(detail)) => {
                let latest = detail.latest_published().map(|c| c.revision);
                let cmds = self.model(surface).detail_arrived(gen, asset, latest);
                self.run_cat_cmds(surface, cmds);
            }
            (
                CatPurpose::Manifest { surface, gen, asset, revision },
                ClientOutput::AssetManifest(manifest),
            ) => {
                let media = match surface {
                    Surface::Video => select_visual_file(&manifest)
                        .or_else(|| select_vjfx_source(&manifest)),
                    Surface::Music | Surface::Sfx => select_file(
                        &manifest,
                        FileRole::Audio,
                        TierPreference::PreferWithAnyFallback(DeviceTier::High),
                        7,
                    )
                    .ok()
                    .map(|f| TileMedia { blob: f.blob, len: f.byte_len, media: f.media }),
                    Surface::Mesh => select_file(
                        &manifest,
                        FileRole::RenderGlb,
                        TierPreference::PreferWithAnyFallback(DeviceTier::High),
                        7,
                    )
                    .ok()
                    .map(|f| TileMedia { blob: f.blob, len: f.byte_len, media: f.media }),
                };
                // A grouped sprite actor is only playable with its manifest
                // text: carry it so the cue can fetch both files.
                let source = (surface == Surface::Video)
                    .then(|| select_billboard_source(&manifest))
                    .flatten();
                let thumb = manifest
                    .thumbnail
                    .as_ref()
                    .map(|t| TileThumb {
                        blob: t.blob,
                        len: t.byte_len,
                        // Straight off the manifest: whether this picture is
                        // a packed sheet, which of its cells are frames, and
                        // how fast they run. Nothing measured, nothing
                        // inferred from the kind.
                        anim: t.animation(),
                    })
                    .or_else(|| {
                        // No preview published (classic sprites/flats are
                        // bare PNGs): a small still IS its own thumbnail.
                        // A grouped actor's packed sheet is NOT — its cells
                        // are authored sizes, not the 128² tiles the strip
                        // splitter expects.
                        if source.is_some() {
                            return None;
                        }
                        media.as_ref().and_then(|m| {
                            (matches!(m.media, MediaType::Png | MediaType::Jpeg)
                                && m.len <= media::MAX_THUMB_BYTES)
                                .then_some(TileThumb { blob: m.blob, len: m.len, anim: None })
                        })
                    });
                // Precomputed stems/lyrics decide, at load time, whether this
                // track is FETCHED or separated on this machine — so they are
                // learned here, where the manifest is, and remembered against
                // the revision that carries them.
                if surface == Surface::Music {
                    self.remember_side_channels(revision, &manifest);
                }
                // player_nav: a World's anchors (player_start, keys, exit)
                // feed the walker slot's player planner when it is cued.
                // Bounded: a long browse session must not grow without end.
                if manifest.kind == AssetKind::World && !manifest.anchors.is_empty() {
                    if self.world_anchors.len() >= 64 && !self.world_anchors.contains_key(&asset)
                    {
                        self.world_anchors.clear();
                    }
                    self.world_anchors.insert(asset, manifest.anchors.clone());
                }
                let cmds = self
                    .model(surface)
                    .manifest_arrived(gen, asset, revision, media, source, thumb);
                self.run_cat_cmds(surface, cmds);
                if surface == Surface::Sfx {
                    self.sync_pads();
                }
                if surface == Surface::Video
                    && self.pending_click.map(|(pending, _)| pending) == Some(asset)
                {
                    let as_content =
                        self.pending_click.take().map(|(_, sh)| sh).unwrap_or(false);
                    self.video_tile_clicked(cx, asset, as_content);
                }
            }
            (CatPurpose::FxSource { asset, revision }, ClientOutput::Blob { path, .. }) => {
                // The splash text is here: hand the render job to the hidden
                // offscreen effect host. Small file, read in place.
                self.fx_source_inflight.remove(&revision);
                let (title, transition) = self
                    .video_model
                    .tile(&asset)
                    .map(|t| {
                        (t.title.clone(), Self::alias_is_transition(t.alias.as_deref()))
                    })
                    .unwrap_or_else(|| (revision.to_string(), false));
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        let widget = self.ui.widget(cx, ids!(fx_thumbs));
                        if let Some(mut thumbs) =
                            widget.borrow_mut::<fx_thumbs::VjFxThumbs>()
                        {
                            thumbs.enqueue(
                                cx,
                                fx_thumbs::FxThumbJob {
                                    asset,
                                    revision,
                                    title,
                                    source,
                                    transition,
                                },
                            );
                        };
                    }
                    Err(error) => {
                        log!("fx thumb: {title} source unreadable: {error}");
                    }
                }
            }
            (
                CatPurpose::FxSlotSource { slot, revision, title },
                ClientOutput::Blob { path, .. },
            ) => {
                // The splash text is here: load it into the slot's offscreen
                // host. A newer click on the same slot supersedes this one.
                if self.fx_slot_inflight[slot.index()] != Some(revision) {
                    return;
                }
                self.fx_slot_inflight[slot.index()] = None;
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        self.load_fx_slot(cx, slot, &title, Some(revision), &source, true)
                    }
                    Err(error) => {
                        self.fx_slots.slot_mut(slot).note = Some("unreadable".to_string());
                        self.sync_fx_slots_ui(cx);
                        log!("fx slot {slot:?}: {title} source unreadable: {error}");
                    }
                }
            }
            (CatPurpose::Thumb { revision }, ClientOutput::Blob { path, .. }) => {
                // Decode on the worker pool; only the finished BGRA pixels
                // come back to this thread.
                // What the MANIFEST said this picture is; the kind gate is
                // only consulted when it said nothing.
                let (sheet, legacy_may_be_sheet) = self.thumb_layout(revision);
                // Stamped with the visible-range generation: a decode for
                // tiles the operator has already scrolled past is dropped
                // unstarted rather than holding up the ones under their
                // thumb (the thumb lane serves newest-first).
                self.decode.submit(DecodeJob::Thumb {
                    revision,
                    path,
                    sheet,
                    legacy_may_be_sheet,
                    epoch: self.view_epoch,
                });
            }
            (CatPurpose::JobProfiles, ClientOutput::JobProfiles(profiles)) => {
                self.gen.profiles_arrived(profiles);
                self.sync_gen_profiles(cx);
            }
            (CatPurpose::JobEnqueue { tag }, ClientOutput::JobQueued(job)) => {
                let cmds = self.gen.queued_at(tag, job, Some(now_ms()));
                self.run_gen_cmds(cmds);
            }
            (CatPurpose::JobStatus { .. }, ClientOutput::JobStatus(status)) => {
                let chain_cmds = self.gen.status_arrived_at(&status, now_ms());
                if !chain_cmds.is_empty() {
                    self.run_gen_cmds(chain_cmds);
                    self.grids_dirty = true;
                }
            }
            (CatPurpose::JobCancel { job }, ClientOutput::JobCancelled(count)) => {
                self.gen.cancel_confirmed_at(job, count, Some(now_ms()));
            }
            (CatPurpose::SideChannelPublish { asset }, ClientOutput::SideChannels(outcome)) => {
                match outcome {
                    SideChannelOutcome::Published { revision } => {
                        log!("side-channels published for {asset}: revision {revision}");
                    }
                    SideChannelOutcome::AlreadyPresent { .. } => {
                        log!("side-channels already published by another client");
                    }
                }
                self.side_channel_publish_settled(asset);
            }
            _ => {}
        }
    }

    /// Remember what a music revision's manifest says it already carries.
    /// Bounded like the anchor map: a long browse must not grow without end,
    /// and the only entries worth keeping are the ones that HAVE something.
    fn remember_side_channels(&mut self, revision: AssetRevisionId, manifest: &AssetManifest) {
        let refs = side_channel_refs(manifest);
        if refs.stems.is_none() && refs.lyrics.is_none() {
            // A republish can also take side-channels AWAY; a stale entry
            // would send this deck fetching blobs the store no longer has.
            self.track_side_channels.remove(&revision);
            return;
        }
        if self.track_side_channels.len() >= 256
            && !self.track_side_channels.contains_key(&revision)
        {
            self.track_side_channels.clear();
        }
        self.track_side_channels.insert(revision, refs);
    }

    fn pump_media_lanes(&mut self, cx: &mut Cx) {
        let lane_count = match self.up.as_ref() {
            Some(up) => up.media.len(),
            None => return,
        };
        for lane in 0..lane_count {
            let events = match self.up.as_mut() {
                Some(up) => match up.media.get_mut(lane) {
                    Some(runtime) => runtime.poll(),
                    None => continue,
                },
                None => return,
            };
            for event in events {
                let id = event.id();
                match event {
                    ClientEvent::Started { .. } | ClientEvent::Progress { .. } => {}
                    ClientEvent::Done { output, .. } => {
                        let Some(purpose) = self.media_reqs.remove(&(lane, id)) else {
                            continue;
                        };
                        let ClientOutput::Blob { path, .. } = output else { continue };
                        match purpose {
                            MediaPurpose::Cue { gen } => {
                                // Only the CURRENT plan entry advances the
                                // cue; superseded completions are stale.
                                if self.video_plan.finished(lane, id) {
                                    // A paired cue waits for its manifest.
                                    let ready = match self.cue_pair.as_mut() {
                                        Some(pair) if pair.gen == gen => {
                                            pair.sheet_landed(gen, path)
                                        }
                                        _ => Some(path),
                                    };
                                    if let Some(path) = ready {
                                        let cmds = self.cue.media_ready(gen, path);
                                        self.run_cue_cmds(cx, cmds);
                                    }
                                }
                            }
                            MediaPurpose::CueSource { gen } => {
                                let ready = self
                                    .cue_pair
                                    .as_mut()
                                    .and_then(|pair| pair.manifest_landed(gen, path));
                                if let Some(path) = ready {
                                    let cmds = self.cue.media_ready(gen, path);
                                    self.run_cue_cmds(cx, cmds);
                                }
                            }
                            MediaPurpose::Deck { deck, gen, media } => {
                                self.decode.submit(DecodeJob::Deck { deck, gen, path, media });
                            }
                            MediaPurpose::DeckStem { deck, gen, index } => {
                                self.side_channel_landed(deck, gen, Some(index), path);
                            }
                            MediaPurpose::DeckLyrics { deck, gen } => {
                                self.side_channel_landed(deck, gen, None, path);
                            }
                            MediaPurpose::Pad { pad, gen, revision, media } => {
                                self.decode.submit(DecodeJob::Pad {
                                    pad,
                                    gen,
                                    revision,
                                    path,
                                    media,
                                });
                            }
                            MediaPurpose::Mesh { gen } => {
                                if self.mesh_plan.finished(lane, id) && gen == self.mesh_gen {
                                    self.decode.submit(DecodeJob::MeshPrep { gen, path });
                                }
                            }
                        }
                    }
                    ClientEvent::Failed { error, .. } => {
                        let Some(purpose) = self.media_reqs.remove(&(lane, id)) else {
                            continue;
                        };
                        if is_session_loss(&error) {
                            self.note_session_failure(cx);
                        }
                        self.media_request_failed(cx, lane, id, purpose, error.to_string());
                    }
                }
            }
        }
    }

    /// One media-lane request is not coming back: tell the engine that
    /// owns it (cue/deck/pad/mesh) so nothing waits on it forever.
    fn media_request_failed(
        &mut self,
        cx: &mut Cx,
        lane: usize,
        id: RequestId,
        purpose: MediaPurpose,
        error: String,
    ) {
        match purpose {
            MediaPurpose::Cue { gen } => {
                if self.video_plan.finished(lane, id) {
                    let cmds = self.cue.media_failed(gen, error);
                    self.run_cue_cmds(cx, cmds);
                }
            }
            MediaPurpose::CueSource { gen } => {
                // Without the manifest a packed sheet is a contact sheet,
                // not an actor: fail the cue honestly instead of showing one.
                if self.cue_pair.as_ref().is_some_and(|pair| pair.gen == gen) {
                    self.cue_pair = None;
                    let cmds = self
                        .cue
                        .media_failed(gen, format!("sprite manifest fetch failed: {error}"));
                    self.run_cue_cmds(cx, cmds);
                }
            }
            MediaPurpose::Deck { deck, gen, .. } => {
                let cmds = self.decks.track_failed(deck, gen, error);
                self.run_deck_cmds(cx, cmds);
            }
            MediaPurpose::DeckStem { deck, gen, .. } => {
                self.side_channel_failed(deck, gen, true, &error);
            }
            MediaPurpose::DeckLyrics { deck, gen } => {
                self.side_channel_failed(deck, gen, false, &error);
            }
            MediaPurpose::Pad { pad, gen, .. } => {
                let cmds = self.pads.load_failed(pad, gen, error);
                self.run_pad_cmds(cmds);
            }
            MediaPurpose::Mesh { gen } => {
                if self.mesh_plan.finished(lane, id) && gen == self.mesh_gen {
                    self.set_mesh_status(cx, &format!("mesh fetch failed: {error}"));
                }
            }
        }
        self.grids_dirty = true;
    }

    /// A connection-class failure was seen on the live session. One failure
    /// is noise (a slow poll); failures that keep coming for the grace
    /// period mean the server is gone — asset servers bind ephemeral ports,
    /// so a restarted asset-ui is unreachable at the old address forever.
    /// Then the only correct move is to drop the dead handles and discover
    /// the live pair again.
    fn note_session_failure(&mut self, cx: &mut Cx) {
        let since = *self.session_loss_since.get_or_insert_with(Instant::now);
        if since.elapsed().as_secs_f64() >= SESSION_LOSS_GRACE_S {
            self.reconnect_session(cx);
        }
    }

    fn note_session_ok(&mut self) {
        self.session_loss_since = None;
    }

    /// Tear down the dead session (off the UI thread), fail what was in
    /// flight on it, and start a fresh connector. The `Up` handler
    /// re-queries every surface when the new session lands.
    fn reconnect_session(&mut self, cx: &mut Cx) {
        self.session_loss_since = None;
        if let Some(up) = self.up.take() {
            // Runtime joins wait out in-flight transfers: never on the UI thread.
            let _ = std::thread::Builder::new()
                .name("vj-session-teardown".into())
                .spawn(move || up.shutdown());
        }
        self.cat_reqs.clear();
        // Side-channel offers die with the session that was carrying them.
        // Nothing retries a write-back: the assets stay marked, and the next
        // machine to separate this track makes the offer instead.
        self.publish_inflight.clear();
        self.publish_deferred.clear();
        let orphaned: Vec<((usize, RequestId), MediaPurpose)> = self.media_reqs.drain().collect();
        for ((lane, id), purpose) in orphaned {
            self.media_request_failed(cx, lane, id, purpose, "asset server lost".to_string());
        }
        self.video_plan = LatestWins::video();
        self.mesh_plan = LatestWins::mesh();
        // The chat's session died with the server; a fresh one opens when
        // the reconnect lands (the transcript on screen survives).
        self.chat.disconnect();
        self.status_text = "asset server lost — rediscovering…".to_string();
        // If THIS process is the host, the store did not go anywhere — only
        // the client session did. Re-point at the server we are still
        // running rather than re-resolving, which would try to take a lock
        // we already hold and fail every time.
        let config = match &self.local_store {
            Some(local) => {
                let mut config = service::session_config_from_env();
                config.endpoints = Some(local.endpoints());
                self.status_text = "reconnecting to the local store…".to_string();
                config
            }
            None => {
                let resolved = local_store::resolve(service::session_config_from_env());
                self.status_text = resolved.note.clone();
                self.local_store = resolved.local;
                resolved.config
            }
        };
        match SessionConnector::start(config) {
            Ok(connector) => self.connector = Some(connector),
            Err(error) => self.status_text = format!("session config invalid: {error}"),
        }
        self.grids_dirty = true;
    }

    // ---- IMPORT CONTENT ---------------------------------------------------

    /// Open the platform's native folder picker.
    ///
    /// The answer arrives later as a `FileDialogAction` in an actions pass —
    /// there is no return value to wait for, and waiting would mean holding
    /// the `Cx` borrow across a modal run loop.
    fn open_import_picker(&mut self, cx: &mut Cx) {
        if self.import_picking {
            return;
        }
        self.import_picking = true;
        let mut dialog = FileDialog::new().set_title("Choose media to import".into());
        let current = self.import.path.trim();
        if !current.is_empty() {
            dialog = dialog.set_location(PathBuf::from(current));
        }
        cx.open_select_folder_dialog(dialog);
    }

    /// Hand the panel a live session and let it start its worker.
    fn start_import(&mut self, cx: &mut Cx) {
        let Some(up) = self.up.as_ref() else {
            self.import.status = "no asset server session yet".to_string();
            self.sync_import_ui(cx);
            return;
        };
        let endpoints = up.endpoints;
        let server_id = up.server_id;
        let token = up.token.clone();
        let cache = service::session_config_from_env().cache_parent;
        if let Err(error) = self.import.start(endpoints, server_id, token, cache) {
            crate::log!("import refused: {error}");
        }
        self.sync_import_ui(cx);
    }

    /// Drain the import worker and repaint the panel. Cheap when idle.
    fn pump_import(&mut self, cx: &mut Cx) {
        if self.import.poll() {
            self.sync_import_ui(cx);
            // Newly published rows arrive through the normal catalog-event
            // path; nudging the grids just makes them appear a beat sooner.
            self.grids_dirty = true;
        }
    }

    /// The IMPORT button wears the whole import lifecycle: quiet label at
    /// rest, live "done/total" while working (lit, click = cancel), back
    /// to quiet when finished — summaries go to the log, the new tiles to
    /// the grid.
    fn sync_import_ui(&mut self, cx: &mut Cx) {
        use crate::import_ui::ImportPhase;
        let (label, lit) = match &self.import.phase {
            ImportPhase::Scanning { found } => (format!("SCAN {found}"), true),
            ImportPhase::Importing { done, total, .. } => {
                (format!("{done}/{total}"), true)
            }
            _ => ("IMPORT".to_string(), false),
        };
        self.ui.button(cx, ids!(import_toggle)).set_text(cx, &label);
        self.paint_lit(cx, ids!(import_toggle), lit);
        match &self.import.phase {
            ImportPhase::Done(summary) | ImportPhase::Cancelled(summary) => {
                if !summary.notes.is_empty() {
                    crate::log!("import: {}", summary.notes.join("  ·  "));
                }
            }
            ImportPhase::Failed(error) => crate::log!("import failed: {error}"),
            _ => {}
        }
        self.ui.redraw(cx);
    }

    /// The next finished decode to spend UI-thread time on.
    ///
    /// What the operator clicked comes first and always: a cue must never
    /// queue behind a screenful of thumbnails. Thumbnails then fill what is
    /// left of the frame budget and the rest waits for the next frame — the
    /// grid fills a beat later instead of the whole app stuttering.
    fn next_decode_result(&mut self, deadline: std::time::Instant) -> Option<DecodeDone> {
        if let Some(done) = self.decode_ready.pop_front() {
            return Some(done);
        }
        if std::time::Instant::now() < deadline {
            return self.decode_backlog.pop_front();
        }
        None
    }

    fn pump_decodes(&mut self, cx: &mut Cx) {
        // The whole batch, not each result: a hundred small thumbnail
        // uploads in one frame hitch exactly as hard as one big mesh.
        let batch = media::UiStep::new("decode results (whole batch)");
        for done in self.decode.poll() {
            match done {
                DecodeDone::Thumb { .. } => self.decode_backlog.push_back(done),
                done => self.decode_ready.push_back(done),
            }
        }
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_micros((media::UI_STEP_BUDGET_MS * 1000.0) as u64);
        while let Some(done) = self.next_decode_result(deadline) {
            match done {
                DecodeDone::Deck { deck, gen, result } => match result {
                    Ok((pcm, peaks)) => {
                        let seconds = pcm.seconds();
                        self.deck_incoming.insert((deck.index(), gen), (pcm, peaks));
                        let cmds = self.decks.track_ready(deck, gen, seconds);
                        if cmds.is_empty() {
                            self.deck_incoming.remove(&(deck.index(), gen));
                        }
                        self.run_deck_cmds(cx, cmds);
                        self.sync_deck_controls(cx);
                    }
                    Err(error) => {
                        let cmds = self.decks.track_failed(deck, gen, error);
                        self.run_deck_cmds(cx, cmds);
                    }
                },
                DecodeDone::Pad { pad, gen, revision, result } => match result {
                    Ok(pcm) => {
                        self.pcm_store.insert(revision, pcm);
                        let cmds = self.pads.load_ready(pad, gen, now_ms());
                        self.run_pad_cmds(cmds);
                    }
                    Err(error) => {
                        let cmds = self.pads.load_failed(pad, gen, error);
                        self.run_pad_cmds(cmds);
                    }
                },
                DecodeDone::MeshPrep { gen, result } => {
                    if gen != self.mesh_gen {
                        continue; // superseded click
                    }
                    match result {
                        Ok(prepared) => {
                            let mesh = self.ui.widget(cx, ids!(mesh_program));
                            let borrow = mesh.borrow_mut::<mesh_view::VjMeshView>();
                            if let Some(mut view) = borrow {
                                view.set_prepared(cx, prepared);
                            }
                            self.show_output_page(cx, id!(mesh_out_page));
                            let status = format!("output: {}", self.mesh_now);
                            self.set_mesh_status(cx, &status);
                        }
                        Err(error) => {
                            self.set_mesh_status(cx, &format!("mesh prep failed: {error}"));
                        }
                    }
                }
                DecodeDone::SlotMesh { gen, slot, world, result } => {
                    let slot = if slot == 0 { SlotId::A } else { SlotId::B };
                    if !self.cue.preroll_current(slot, gen) {
                        continue; // superseded click owns this slot now
                    }
                    match result {
                        Ok(prepared) => {
                            // The alias names the game family, which picks
                            // the engine's head-bob feel.
                            let asset =
                                self.cue.next().or_else(|| self.cue.live()).map(|item| item.asset);
                            let source = asset
                                .and_then(|asset| self.video_model.tile(&asset))
                                .and_then(|tile| tile.alias.clone())
                                .unwrap_or_default();
                            // player_nav: the world's manifest anchors ride
                            // along to the walker (empty on classic maps
                            // published before the anchor lane).
                            let anchors = asset
                                .and_then(|a| self.world_anchors.get(&a))
                                .map(|list| list.iter().map(nav_anchor).collect())
                                .unwrap_or_default();
                            self.apply_slot_mesh(cx, slot, prepared, world, &source, anchors);
                            self.slot_media[slot.index()] = SlotMedia::Mesh;
                            self.slot_aspect[slot.index()] = 16.0 / 9.0;
                            self.show_output_page(cx, id!(video_out_page));
                            let cmds = self.cue.preroll_ready(slot, gen);
                            self.run_cue_cmds(cx, cmds);
                            self.video_pump = cx.new_next_frame();
                        }
                        Err(error) => {
                            log!("slot mesh failed: {error}");
                            self.slot_media[slot.index()] = SlotMedia::Empty;
                            let cmds = self.cue.preroll_failed(slot, gen, error);
                            self.run_cue_cmds(cx, cmds);
                        }
                    }
                }
                DecodeDone::Billboard { gen, slot, result } => {
                    let slot = if slot == 0 { SlotId::A } else { SlotId::B };
                    if !self.cue.preroll_current(slot, gen) {
                        continue; // superseded click owns this slot now
                    }
                    match result {
                        Ok(prepared) => {
                            let step = media::UiStep::new("billboard frame textures");
                            let mut textures = Vec::new();
                            for state in &prepared.states {
                                let mut frames = Vec::new();
                                for (bgra, w, h) in &state.frames {
                                    frames.push(Texture::new_with_format(
                                        cx,
                                        TextureFormat::VecBGRAu8_32 {
                                            width: *w,
                                            height: *h,
                                            data: Some(bgra.clone()),
                                            updated: TextureUpdated::Full,
                                        },
                                    ));
                                }
                                textures.push(frames);
                            }
                            step.done(cx);
                            let start = prepared
                                .states
                                .iter()
                                .position(|s| s.name == prepared.preview)
                                .unwrap_or(0);
                            let aspect = prepared
                                .states
                                .get(start)
                                .and_then(|s| s.frames.first())
                                .map(|(_, w, h)| *w as f32 / (*h).max(1) as f32)
                                .unwrap_or(1.0);
                            self.slot_aspect[slot.index()] = aspect;
                            self.slot_media[slot.index()] = SlotMedia::Billboard;
                            self.billboards[slot.index()] = Some(BillboardSlot {
                                states: prepared.states,
                                textures,
                                state_i: start,
                                frame_i: 0,
                                accum: 0.0,
                                last: None,
                            });
                            self.sync_slot_controls_ui(cx);
                            self.show_output_page(cx, id!(video_out_page));
                            let cmds = self.cue.preroll_ready(slot, gen);
                            self.run_cue_cmds(cx, cmds);
                            self.video_pump = cx.new_next_frame();
                        }
                        Err(error) => {
                            self.slot_media[slot.index()] = SlotMedia::Empty;
                            let cmds = self.cue.preroll_failed(slot, gen, error);
                            self.run_cue_cmds(cx, cmds);
                        }
                    }
                }
                DecodeDone::Still { gen, slot, result } => {
                    let slot = if slot == 0 { SlotId::A } else { SlotId::B };
                    if !self.cue.preroll_current(slot, gen) {
                        continue; // superseded click owns this slot now
                    }
                    match result {
                        Ok((bgra, w, h)) => {
                            self.slot_aspect[slot.index()] =
                                w.max(1) as f32 / h.max(1) as f32;
                            let step = media::UiStep::new("still texture upload");
                            self.slot_textures[slot.index()] = Some(Texture::new_with_format(
                                cx,
                                TextureFormat::VecBGRAu8_32 {
                                    width: w,
                                    height: h,
                                    data: Some(bgra),
                                    updated: TextureUpdated::Full,
                                },
                            ));
                            step.done(cx);
                            self.slot_media[slot.index()] = SlotMedia::Still;
                            self.show_output_page(cx, id!(video_out_page));
                            let cmds = self.cue.preroll_ready(slot, gen);
                            self.run_cue_cmds(cx, cmds);
                            self.video_pump = cx.new_next_frame();
                        }
                        Err(error) => {
                            self.slot_media[slot.index()] = SlotMedia::Empty;
                            let cmds = self.cue.preroll_failed(slot, gen, error);
                            self.run_cue_cmds(cx, cmds);
                        }
                    }
                }
                DecodeDone::FlowClip { gen, slot, result } => {
                    let slot = if slot == 0 { SlotId::A } else { SlotId::B };
                    let i = slot.index();
                    // Staleness gate: the cache must belong to the load that
                    // asked for it, and the slot must still be a video.
                    if self.slot_flow_gen[i] != gen || self.slot_media[i] != SlotMedia::Video {
                        continue;
                    }
                    match result {
                        Ok(Some(data)) => {
                            log!(
                                "slot {slot:?}: FLOW map adopted — {} pairs, grid {}x{}, {}x{} @ stride {} ({} endpoint frames, {:.1} MB)",
                                data.pairs,
                                data.map.grid_w,
                                data.map.grid_h,
                                data.width,
                                data.height,
                                data.stride,
                                data.frames.len(),
                                (data.frames.len() * data.width * data.height * 4) as f64
                                    / (1024.0 * 1024.0)
                            );
                            self.flow_view(cx, slot, |cx, view| view.set_clip(cx, data));
                            self.slot_flow_avail[i] = true;
                            // FLOW is not a control: a clip that carries a
                            // motion payload always plays through the warp.
                            self.slot_flow_on[i] = true;
                            self.engage_flow(cx, slot);
                            self.strip_shape[i] = None;
                            self.sync_slot_controls_ui(cx);
                            self.video_pump = cx.new_next_frame();
                        }
                        // No mkfl / over budget / unmappable: plays as today
                        // (the worker already logged the WHY for the fallbacks).
                        Ok(None) => {}
                        Err(error) => {
                            log!("slot {slot:?}: flow probe failed: {error} — playing as plain video");
                        }
                    }
                }
                DecodeDone::Thumb { revision, result } => {
                    if let Ok(thumb) = result {
                        let mut make = |bgra: Vec<u32>, w: usize, h: usize| {
                            Texture::new_with_format(
                                cx,
                                TextureFormat::VecBGRAu8_32 {
                                    width: w,
                                    height: h,
                                    data: Some(bgra),
                                    updated: TextureUpdated::Full,
                                },
                            )
                        };
                        if let Some((r, g, b)) = thumb_color(&thumb.bgra) {
                            self.thumb_leds.insert(revision, palette_velocity(r, g, b));
                        }
                        let texture = make(thumb.bgra, thumb.width, thumb.height);
                        let frames: Vec<Texture> = if thumb.frames.len() > 1 {
                            thumb
                                .frames
                                .into_iter()
                                .filter(|(px, w, h)| *w > 0 && *h > 0 && px.len() == *w * *h)
                                .map(|(px, w, h)| make(px, w, h))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        self.thumbs.insert(revision, texture.clone());
                        self.thumb_clock += 1;
                        self.thumb_used.insert(revision, self.thumb_clock);
                        self.thumb_inflight.remove(&revision);
                        self.fx_decode_pending.remove(&revision);
                        if frames.len() > 1 {
                            self.thumb_anims
                                .insert(revision, (frames.clone(), thumb.fps));
                        }
                        if self.trace_thumbs {
                            log!("thumb: decoded {revision} ({} cached)", self.thumbs.len());
                        }
                        self.apply_thumb(cx, revision, texture, frames, thumb.fps);
                        self.evict_thumbs();
                    } else if let Err(e) = result {
                        // A failed decode must NOT be remembered as "no
                        // thumbnail": the tile asks again next time it is
                        // wanted, which is what makes a transient 404 after a
                        // republish heal itself.
                        self.thumb_inflight.remove(&revision);
                        // A rendered effect sheet that will not decode is a
                        // bad cache file: drop it so the renderer writes a
                        // fresh one instead of resubmitting it forever.
                        if self.fx_decode_pending.remove(&revision).is_some() {
                            let cache = service::session_config_from_env()
                                .cache_parent
                                .join("cache-vjfx-thumbs-30");
                            let _ = std::fs::remove_file(fx_thumbs::cache_path(
                                &cache, &revision, false,
                            ));
                            let _ = std::fs::remove_file(fx_thumbs::cache_path(
                                &cache, &revision, true,
                            ));
                            log!("fx thumb: sheet decode FAILED {revision}: {e}");
                        }
                        if self.trace_thumbs {
                            log!("thumb: decode FAILED {revision}: {e}");
                        }
                    }
                }
            }
        }
        if !self.decode_backlog.is_empty() {
            self.decode_pump = cx.new_next_frame();
        }
        batch.done(cx);
    }

    /// Trim the thumbnail cache to its budget, dropping the LEAST RECENTLY
    /// WANTED first and never the ones this rebuild just asked for.
    fn evict_thumbs(&mut self) {
        let bytes = |anim_frames: usize| THUMB_TEX_BYTES * (1 + anim_frames);
        let mut total: usize = self
            .thumbs
            .keys()
            .map(|r| bytes(self.thumb_anims.get(r).map(|(f, _)| f.len()).unwrap_or(0)))
            .sum();
        if total <= THUMB_CACHE_BYTES {
            return;
        }
        let mut by_age: Vec<(u64, AssetRevisionId)> = self
            .thumbs
            .keys()
            .map(|r| (self.thumb_used.get(r).copied().unwrap_or(0), *r))
            .collect();
        by_age.sort_unstable();
        // Anything wanted by the current rebuild is off limits, or a bank
        // wider than the budget would evict the very tiles being drawn.
        let keep_after = self.thumb_clock.saturating_sub(1);
        for (used, revision) in by_age {
            if total <= THUMB_CACHE_BYTES || used >= keep_after {
                break;
            }
            total = total.saturating_sub(bytes(
                self.thumb_anims.get(&revision).map(|(f, _)| f.len()).unwrap_or(0),
            ));
            self.thumbs.remove(&revision);
            self.thumb_anims.remove(&revision);
            self.thumb_leds.remove(&revision);
            self.thumb_used.remove(&revision);
            if self.trace_thumbs {
                log!("thumb: evicted {revision} (last wanted {used}, clock {})", self.thumb_clock);
            }
        }
    }

    /// Stamp every thumbnail the grids are about to draw as wanted, and
    /// re-request any whose texture is gone.
    ///
    /// The cache used to evict without anyone ever asking again, so a tile
    /// that lost its texture stayed blank for the rest of the session —
    /// "tiles continuously lose their thumbnails". A published thumbnail is
    /// immutable per revision, so re-fetching is always safe.
    fn refresh_thumbs(&mut self, cx: &mut Cx) {
        self.thumb_clock += 1;
        let now = self.thumb_clock;
        // What is actually on screen. Stamping EVERY tile would give every
        // revision the same age and turn the cache back into a coin toss;
        // the drawn window is what must never be evicted.
        let mut hot: HashSet<AssetId> = HashSet::new();
        let video = self.ui.widget(cx, ids!(video_grid));
        let mut bank = self.view_epoch_bank;
        if let Some(pads) = video.borrow::<VjPadMatrix>() {
            hot.extend(pads.visible_assets());
            bank = pads.bank;
        }
        drop(video);
        // The window moved: everything queued for the old one is stale.
        if bank != self.view_epoch_bank {
            self.view_epoch_bank = bank;
            self.view_epoch += 1;
        }
        hot.extend(self.cue.live().map(|i| i.asset));
        hot.extend(self.cue.next().map(|i| i.asset));
        let mut wanted: Vec<(AssetRevisionId, catalog::TileThumb)> = Vec::new();
        for surface in [Surface::Video, Surface::Music, Surface::Sfx, Surface::Mesh] {
            let model = match surface {
                Surface::Video => &self.video_model,
                Surface::Music => &self.music_model,
                Surface::Sfx => &self.sfx_model,
                Surface::Mesh => &self.mesh_model,
            };
            for tile in model.tiles() {
                let (Some(revision), Some(thumb)) = (tile.revision, tile.thumb.clone()) else {
                    continue;
                };
                // The small grids draw everything they hold; the video bank
                // is a window onto thousands.
                if surface == Surface::Video && !hot.contains(&tile.asset) {
                    continue;
                }
                if self.thumbs.contains_key(&revision) {
                    self.thumb_used.insert(revision, now);
                    continue;
                }
                if self.thumb_inflight.contains(&revision) {
                    continue;
                }
                wanted.push((revision, thumb));
            }
        }
        if wanted.is_empty() {
            return;
        }
        // Bounded per rebuild: a 3000-tile bank must not queue 3000 blobs.
        wanted.truncate(MAX_THUMB_REFETCH);
        let Some(up) = self.up.as_mut() else { return };
        for (revision, thumb) in wanted {
            if thumb.len > media::MAX_THUMB_BYTES {
                continue;
            }
            if let Ok(id) = up.catalog.submit_with(
                ClientRequest::FetchBlob {
                    blob: thumb.blob,
                    expected_len: Some(thumb.len),
                    pin: false,
                },
                makepad_asset_client::SubmitOptions::newest_first(),
            ) {
                self.cat_reqs.insert(id, CatPurpose::Thumb { revision });
                self.thumb_inflight.insert(revision);
                if self.trace_thumbs {
                    log!("thumb: re-requesting {revision} (texture gone)");
                }
            }
        }
    }

    /// Lazy ANIMATED thumbnails for vjeffect tiles (see fx_thumbs.rs).
    ///
    /// Each pump tick: land any freshly rendered sheet in the thumb decode
    /// lane (the same lane store thumbnails ride, so the grid needs no new
    /// path), then — only while the renderer is idle — pick the next effect
    /// tile still wearing its seeded placeholder, VISIBLE PADS FIRST. A
    /// cached sheet decodes straight from disk; only a cache miss costs a
    /// source fetch and an offscreen render, and never more than one at a
    /// time.
    fn pump_fx_thumbs(&mut self, cx: &mut Cx) {
        if self.up.is_none() {
            return;
        }
        let now = cx.seconds_since_app_start();
        let widget = self.ui.widget(cx, ids!(fx_thumbs));
        let (results, idle, render_disabled, cache_dir) = {
            let Some(mut thumbs) = widget.borrow_mut::<fx_thumbs::VjFxThumbs>() else {
                return;
            };
            if thumbs.cache_dir().is_none() {
                thumbs.set_cache_dir(
                    service::session_config_from_env()
                        .cache_parent
                        .join("cache-vjfx-thumbs-30"),
                );
            }
            let Some(cache_dir) = thumbs.cache_dir().map(Path::to_path_buf) else { return };
            (
                thumbs.take_results(),
                thumbs.is_idle(),
                thumbs.disabled_reason().is_some(),
                cache_dir,
            )
        };
        for sheet in results {
            self.fx_decode_pending.insert(sheet.revision, now);
            self.decode.submit(DecodeJob::Thumb {
                revision: sheet.revision,
                path: sheet.path,
                sheet: Some((sheet.cells, sheet.fps)),
                legacy_may_be_sheet: false,
                epoch: self.view_epoch,
            });
            self.grids_dirty = true;
        }
        // Candidates: the pads on screen lead, the rest of the loaded
        // catalog window follows.
        let mut order: Vec<AssetId> = self.video_pad_assets.iter().flatten().copied().collect();
        order.extend(self.video_model.tiles().iter().map(|t| t.asset));
        let mut seen: HashSet<AssetId> = HashSet::new();
        let mut cache_decodes = 0usize;
        for asset in order {
            if !seen.insert(asset) {
                continue;
            }
            let Some(tile) = self.video_model.tile(&asset) else { continue };
            if tile.kind != Some(AssetKind::VjEffect) {
                continue;
            }
            let Some(revision) = tile.revision else { continue };
            // A revision whose picture already declares cells has a REAL
            // animated thumbnail (store-side or ours) — nothing to do.
            if tile.thumb.as_ref().is_some_and(|t| t.anim.is_some()) {
                continue;
            }
            if self.thumb_anims.contains_key(&revision) {
                continue;
            }
            if self
                .fx_decode_pending
                .get(&revision)
                .is_some_and(|at| now - at < 3.0)
            {
                continue;
            }
            let failed = widget
                .borrow::<fx_thumbs::VjFxThumbs>()
                .is_some_and(|t| t.is_failed(&revision));
            if failed {
                continue;
            }
            let transition = Self::alias_is_transition(tile.alias.as_deref());
            let cache = fx_thumbs::cache_path(&cache_dir, &revision, transition);
            if cache.exists() {
                // A relaunch must not re-render: decode the digest-keyed
                // sheet straight off disk, bounded per tick.
                let layout = std::fs::read(&cache)
                    .ok()
                    .and_then(|png| makepad_asset_importer::anim_icon::read_layout(&png));
                match layout {
                    Some((cells, fps)) => {
                        self.fx_decode_pending.insert(revision, now);
                        self.decode.submit(DecodeJob::Thumb {
                            revision,
                            path: cache,
                            sheet: Some((cells, fps)),
                            legacy_may_be_sheet: false,
                            epoch: self.view_epoch,
                        });
                        cache_decodes += 1;
                        if cache_decodes >= 6 {
                            break;
                        }
                    }
                    None => {
                        // Unreadable/unstamped cache file: drop it and let
                        // the render path write a fresh one next tick.
                        let _ = std::fs::remove_file(&cache);
                    }
                }
                continue;
            }
            // Cache miss: one render pipeline at a time, and only while the
            // renderer is idle and able.
            if render_disabled || !idle || !self.fx_source_inflight.is_empty() {
                continue;
            }
            if self.fx_source_inflight.contains(&revision) {
                continue;
            }
            let Some(media) = tile.media.clone() else { continue };
            if media.media != MediaType::Text || media.len > media::MAX_THUMB_BYTES {
                continue;
            }
            let Some(up) = self.up.as_mut() else { return };
            if let Ok(id) = up.catalog.submit_with(
                ClientRequest::FetchBlob {
                    blob: media.blob,
                    expected_len: Some(media.len),
                    pin: false,
                },
                makepad_asset_client::SubmitOptions::newest_first(),
            ) {
                self.cat_reqs
                    .insert(id, CatPurpose::FxSource { asset, revision });
                self.fx_source_inflight.insert(revision);
            }
            break;
        }
    }

    // ---- UI sync ------------------------------------------------------------

    /// MASTER's number next to its fader. Percent of unity, so 0.90 reads
    /// as "90%" and the 1.2 headroom reads as "120%" — a dB scale would be
    /// honest too, but the fader is linear and pretending otherwise lies.
    /// The chip carries its own readout now.
    fn set_master_readout(&self, _cx: &mut Cx, _value: f32) {}

    /// A DropSlider's change in this frame's actions, by widget path.
    fn drop_slider_changed(&mut self, cx: &mut Cx, path: &[LiveId], actions: &Actions) -> Option<f64> {
        let uid = self.ui.widget(cx, path).widget_uid();
        let mut changed = None;
        for action in actions.iter() {
            if let Some(wa) = action.as_widget_action() {
                if wa.widget_uid == uid {
                    if let makepad_widgets::drop_slider::DropSliderAction::Changed(v) = wa.cast() {
                        changed = Some(v);
                    }
                }
            }
        }
        changed
    }

    fn set_drop_slider(&mut self, cx: &mut Cx, path: &[LiveId], value: f64) {
        if let Some(mut chip) = self
            .ui
            .widget(cx, path)
            .borrow_mut::<makepad_widgets::drop_slider::DropSlider>()
        {
            chip.set_value(cx, value);
        };
    }

    fn remember_output_window_geometry(
        &self,
        cx: &mut Cx,
        window_id: WindowId,
        geom: &WindowGeom,
    ) {
        if geom.is_fullscreen
            || geom.inner_size.x <= 0.0
            || geom.inner_size.y <= 0.0
            || !cx.windows.is_valid(window_id)
        {
            return;
        }
        let window = &mut cx.windows[window_id];
        window.create_inner_size = Some(geom.inner_size);
        window.create_position = Some(geom.position);
    }

    fn handle_output_window_event(&mut self, cx: &mut Cx, event: &Event) {
        let Some(output_id) = self.ui.window(cx, ids!(output_window)).window_id() else {
            return;
        };
        match event {
            Event::WindowCloseRequested(ev) if ev.window_id == output_id => {
                let geom = cx.windows[output_id].window_geom.clone();
                self.remember_output_window_geometry(cx, output_id, &geom);
            }
            Event::WindowClosed(ev) if ev.window_id == output_id => {
                self.output_window_lifecycle = OutputWindowLifecycle::Closed;
                // Closed with the window's own close box: the button has to
                // follow, or OUTPUT stays lit over a window that is gone.
                self.sync_output_button(cx);
            }
            Event::WindowGeomChange(ev) if ev.window_id == output_id => {
                self.remember_output_window_geometry(cx, output_id, &ev.new_geom);
                if self.output_window_lifecycle == OutputWindowLifecycle::Opening {
                    self.output_window_lifecycle = OutputWindowLifecycle::Open;
                    self.sync_output_button(cx);
                }
            }
            Event::WindowGotFocus(window_id) if *window_id == output_id => {
                self.output_window_lifecycle = OutputWindowLifecycle::Open;
                self.sync_output_button(cx);
            }
            _ => {}
        }
    }

    fn open_output_window(&mut self, cx: &mut Cx) {
        let output = self.ui.window(cx, ids!(output_window));
        let Some(window_id) = output.window_id() else {
            return;
        };
        let command = output_window_command(
            self.output_window_lifecycle,
            matches!(cx.os_type(), OsType::Macos),
        );
        match command {
            Some(OutputWindowCommand::Recreate) => {
                // A closed native surface leaves its stable WindowId, widget
                // tree and render pass in place. Recreate only that surface;
                // rebuilding the Root would also destroy console state.
                if cx.windows.is_valid(window_id) {
                    cx.windows[window_id].is_fullscreen = false;
                    cx.push_unique_platform_op(
                        makepad_widgets::makepad_platform::CxOsOp::CreateWindow(window_id),
                    );
                    self.output_window_lifecycle = OutputWindowLifecycle::Opening;
                    output.redraw(cx);
                }
            }
            Some(OutputWindowCommand::Deminiaturize) => {
                cx.push_unique_platform_op(
                    makepad_widgets::makepad_platform::CxOsOp::Deminiaturize(window_id),
                );
            }
            Some(OutputWindowCommand::Restore) => {
                cx.push_unique_platform_op(
                    makepad_widgets::makepad_platform::CxOsOp::RestoreWindow(window_id),
                );
            }
            None => {}
        }
    }

    /// Put the output window away. The widget tree, its render pass and the
    /// stable `WindowId` all survive a closed native surface — that is what
    /// `OutputWindowCommand::Recreate` reopens — so this is symmetric with
    /// `open_output_window` and never rebuilds the Root.
    fn close_output_window(&mut self, cx: &mut Cx) {
        let output = self.ui.window(cx, ids!(output_window));
        let Some(window_id) = output.window_id() else {
            return;
        };
        if cx.windows.is_valid(window_id) {
            cx.push_unique_platform_op(
                makepad_widgets::makepad_platform::CxOsOp::CloseWindow(window_id),
            );
        }
        self.output_window_lifecycle = OutputWindowLifecycle::Closed;
        self.sync_output_button(cx);
    }

    /// The OUTPUT button wears its state: lit while the window is up, plain
    /// when it is not — including when the operator closed it with the
    /// window's own close box, which is why this is driven from the
    /// lifecycle rather than from the click.
    fn sync_output_button(&mut self, cx: &mut Cx) {
        let open = matches!(self.output_window_lifecycle, OutputWindowLifecycle::Open);
        // The button's COLOR is the whole story: orange = window up,
        // black = off — the app's standard on/off language.
        self.paint_icon_button(cx, ids!(open_output), open);
    }

    fn show_output_page(&mut self, cx: &mut Cx, page: LiveId) {
        self.ui.page_flip(cx, ids!(out_pages)).set_active_page(cx, page);
        self.out_page = page;
        self.sync_mesh_liveness(cx);
        self.ui.redraw(cx);
    }

    fn set_mesh_status(&mut self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(mesh_status)).set_text(cx, text);
    }

    fn gen_panel_path() -> std::path::PathBuf {
        service::session_config_from_env().cache_parent.join("gen-panel.txt")
    }

    /// The gen panel survives restarts: pipe, length, CONT arm and the
    /// prompt re-apply at launch, so an endless stream stays endless
    /// through every reboot instead of dying with the window.
    fn save_gen_panel(&self) {
        let prompt = self.gen.prompt.replace('\n', " ");
        let body = format!(
            "{}\n{}\n{}\n{}\n{}\n",
            self.gen.selected,
            self.gen.video_length(),
            u8::from(self.gen.continuous()),
            prompt,
            u8::from(self.gen_panel_open),
        ) + &format!(
            "{}\n{}\n",
            u8::from(self.lights_tab),
            u8::from(self.monitor_audio),
        );
        let path = Self::gen_panel_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, body);
    }

    fn load_gen_panel(&mut self, cx: &mut Cx) {
        let Ok(body) = std::fs::read_to_string(Self::gen_panel_path()) else { return };
        let mut lines = body.lines();
        let selected: usize = lines.next().and_then(|l| l.parse().ok()).unwrap_or(0);
        let length: usize = lines.next().and_then(|l| l.parse().ok()).unwrap_or(0);
        let cont = lines.next().map(|l| l == "1").unwrap_or(false);
        let prompt = lines.next().unwrap_or("").to_string();
        let open = lines.next().map(|l| l == "1").unwrap_or(false);
        if open != self.gen_panel_open {
            self.set_gen_panel_open(cx, open);
        }
        let lights = lines.next().map(|l| l == "1").unwrap_or(false);
        if lights != self.lights_tab {
            self.set_lower_tab(cx, lights);
        }
        // MONITOR AUDIO comes back on for the operator who left it on
        // (the TCC prompt was answered on the deliberate first flip).
        if lines.next().map(|l| l == "1").unwrap_or(false) {
            self.set_monitor_audio(cx, true);
        }
        self.gen.select_profile(selected);
        self.gen.set_video_length(length);
        self.ui
            .drop_down(cx, ids!(gen_profile))
            .set_selected_item(cx, self.gen.selected);
        self.ui
            .drop_down(cx, ids!(gen_len))
            .set_selected_item(cx, self.gen.video_length());
        if !prompt.is_empty() {
            self.gen.set_prompt(prompt.clone());
            self.ui.text_input(cx, ids!(gen_prompt)).set_text(cx, &prompt);
        }
        if cont {
            let cmds = self.gen.set_continuous(true, now_ms());
            self.run_gen_cmds(cmds);
            self.ui.check_box(cx, ids!(gen_loop)).set_active(cx, true, Animate::No);
        }
    }

    fn sync_gen_profiles(&mut self, cx: &mut Cx) {
        let labels = crate::gen::GenModel::pipe_labels();
        if labels != self.gen_profile_labels {
            self.gen_profile_labels = labels.clone();
            self.ui.drop_down(cx, ids!(gen_profile)).set_labels(cx, labels);
            // The widget's own selection starts at row 0 no matter what the
            // model defaults to; without this the box SHOWS "expand → image"
            // while the model would run video, and the first click makes the
            // lie true ("gimme a jumping rabbit" came back as a picture).
            self.ui
                .drop_down(cx, ids!(gen_profile))
                .set_selected_item(cx, self.gen.selected);
            let lengths = crate::gen::GenModel::video_length_labels();
            self.ui.drop_down(cx, ids!(gen_len)).set_labels(cx, lengths);
            self.ui
                .drop_down(cx, ids!(gen_len))
                .set_selected_item(cx, self.gen.video_length());
        }
    }

    /// Whether a catalog thumbnail for `revision` may be a packed animation
    /// sheet. Decided by the tile's KIND (sprite actors and meshes publish
    /// 128²-tile strips), never by the thumbnail's dimensions: a 1024² PBR
    /// map or Flux still is dimensionally a 64-tile sheet and must stay one
    /// still picture.
    /// What a revision's thumbnail says about itself, and — only when it
    /// says nothing — whether its catalog kind allows the legacy guess.
    ///
    /// A declared layout is the answer; the kind gate is what is left for
    /// revisions published before thumbnails could declare anything.
    fn thumb_layout(
        &self,
        revision: AssetRevisionId,
    ) -> (Option<(makepad_asset_data::ThumbnailCells, f32)>, bool) {
        let tile = SURFACES
            .iter()
            .flat_map(|surface| match surface {
                Surface::Video => self.video_model.tiles(),
                Surface::Music => self.music_model.tiles(),
                Surface::Sfx => self.sfx_model.tiles(),
                Surface::Mesh => self.mesh_model.tiles(),
            })
            .find(|tile| tile.revision == Some(revision));
        let Some(tile) = tile else { return (None, false) };
        match tile.thumb.as_ref().and_then(|t| t.anim.clone()) {
            Some(anim) => (Some(anim), false),
            None => (None, catalog::kind_may_be_sheet(tile.kind)),
        }
    }

    fn sync_video_pad_window(&mut self, cx: &mut Cx) {
        let widget = self.ui.widget(cx, ids!(video_grid));
        let at_tail = {
            let Some(pads) = widget.borrow::<VjPadMatrix>() else { return };
            self.apc.bank = pads.bank;
            self.video_pad_total = pads.len();
            self.video_pad_assets.clear();
            self.video_pad_assets
                .extend((0..40).map(|pad| pads.visible_at(pad).map(|entry| entry.asset)));
            pads.at_tail()
        };
        // Page on demand: the window reached the loaded tail, so ask the
        // server for the next page instead of having fetched everything.
        if at_tail && self.video_model.has_more() && !self.video_model.is_loading() {
            let cmds = self.video_model.load_more();
            self.run_cat_cmds(Surface::Video, cmds);
        }
    }

    /// Mirror the sfx surface's ready tiles into the pad engine.
    fn sync_pads(&mut self) {
        let mut keep = Vec::new();
        let items: Vec<PadItem> = self.sfx_model.tiles().iter().filter_map(|t| {
            let media = t.media.clone()?;
            let revision = t.revision?;
            keep.push(t.asset);
            Some(PadItem {
                asset: t.asset,
                revision,
                title: t.title.clone(),
                media_blob: media.blob,
                media_len: media.len,
                media: media.media,
            })
        }).collect();
        for item in items {
            self.pads.upsert_pad(item);
        }
        let cmds = self.pads.retain_pads(&keep);
        self.run_pad_cmds(cmds);
    }

    fn apply_thumb(
        &mut self,
        cx: &mut Cx,
        revision: AssetRevisionId,
        texture: Texture,
        frames: Vec<Texture>,
        fps: f32,
    ) {
        let Some(asset) = self
            .video_model
            .tiles()
            .iter()
            .chain(self.music_model.tiles())
            .chain(self.sfx_model.tiles())
            .chain(self.mesh_model.tiles())
            .find(|tile| tile.revision == Some(revision))
            .map(|tile| tile.asset)
        else {
            return;
        };
        let widget = self.ui.widget(cx, ids!(video_grid));
        if let Some(mut pads) = widget.borrow_mut::<VjPadMatrix>() {
            if frames.len() > 1 {
                pads.set_thumb_anim(cx, asset, frames.clone(), fps);
            } else {
                pads.set_thumb(cx, asset, texture.clone());
            }
        };
        for grid in [ids!(music_grid), ids!(sfx_grid), ids!(mesh_grid)] {
            let widget = self.ui.widget(cx, grid);
            if let Some(mut grid) = widget.borrow_mut::<VjTileGrid>() {
                if frames.len() > 1 {
                    grid.set_thumb_anim(cx, asset, frames.clone(), fps);
                } else {
                    grid.set_thumb(cx, asset, texture.clone());
                }
            };
        }
    }

    fn grid_entries(&self, surface: Surface) -> Vec<GridEntry> {
        let mut entries = Vec::new();
        let model = match surface {
            Surface::Video => &self.video_model,
            Surface::Music => &self.music_model,
            Surface::Sfx => &self.sfx_model,
            Surface::Mesh => &self.mesh_model,
        };
        // The program strip is drawn in DISPLAY order — the PENDING head
        // column (reserved to a full column height, so filling it never
        // moves the body) and then the settled body. Every other surface is
        // a plain list.
        let cells: Vec<Option<&catalog::Tile>> = match surface {
            Surface::Video => model
                .display_order()
                .into_iter()
                .map(|slot| slot.and_then(|asset| model.tile(&asset)))
                .collect(),
            _ => model.tiles().iter().map(Some).collect(),
        };
        entries.extend(cells
            .into_iter()
            // One-line legacy screen: the 483 pre-grouping per-lump Doom
            // sprites stay out of the grid until the server retires them.
            // They are stable rows, so dropping them never shifts anything
            // under the operator's hand between refreshes.
            .filter(|cell| {
                cell.is_none_or(|tile| {
                    !catalog::HIDE_LEGACY_LUMP_SPRITES
                        || !catalog::is_legacy_lump_sprite(
                            tile.kind,
                            tile.alias.as_deref(),
                            tile.source.is_some(),
                        )
                })
            })
            .enumerate()
            .map(|(index, cell)| {
                let Some(tile) = cell else {
                    // A reserved pending cell: nothing to draw, nothing to
                    // click, and it holds the body still while the column
                    // fills around it.
                    return GridEntry {
                        asset: AssetId::from_bytes([0; 16]),
                        title: String::new(),
                        sub: String::new(),
                        state: String::new(),
                        pad: String::new(),
                        texture: None,
                        frames: Vec::new(),
                        fps: 0.0,
                        cells: false,
                        loading: false,
                        failed: false,
                        active: false,
                        placeholder: true,
                    };
                };
                let texture = tile.revision.and_then(|rev| self.thumbs.get(&rev).cloned());
                let state = match (&tile.state, surface) {
                    (catalog::TileState::Ready, Surface::Sfx) => {
                        match self.pads.pad(&tile.asset).map(|p| p.load.clone()) {
                            Some(pads::PadLoad::Ready) => "ready".to_string(),
                            Some(pads::PadLoad::Loading { .. }) => "loading".to_string(),
                            Some(pads::PadLoad::Failed { .. }) => "failed".to_string(),
                            _ => "…".to_string(),
                        }
                    }
                    (catalog::TileState::Ready, _) => match tile.media.as_ref().map(|m| m.media) {
                        Some(MediaType::Glb) => "3D".to_string(),
                        Some(MediaType::Ply) => "SPLAT".to_string(),
                        Some(MediaType::Png) | Some(MediaType::Jpeg) => "TEX".to_string(),
                        Some(MediaType::Mp4) => "VID".to_string(),
                        // A vjeffect's playable file is its splash text.
                        Some(MediaType::Text) => "FX".to_string(),
                        _ => "ready".to_string(),
                    },
                    (catalog::TileState::Listed, _) | (catalog::TileState::Resolving, _) => {
                        "…".to_string()
                    }
                    (catalog::TileState::Failed(e), _) => {
                        let mut e = e.clone();
                        e.truncate(18);
                        e
                    }
                };
                let active = match surface {
                    // ONE mark on the clip grid: the tile last clicked. The
                    // live / next / held distinctions still drive the cue
                    // engine and the APC LEDs — they just are not painted
                    // on top of each other on screen any more.
                    Surface::Video => self.last_clicked == Some(tile.asset),
                    Surface::Sfx => self.pads.playing_voices(&tile.asset) > 0,
                    _ => false,
                };
                let sub = match (&tile.alias, tile.live) {
                    (Some(alias), _) => alias.clone(),
                    (None, true) => String::new(),
                    (None, false) => "draft".to_string(),
                };
                let (frames, fps) = tile
                    .revision
                    .and_then(|rev| self.thumb_anims.get(&rev).cloned())
                    .unwrap_or((Vec::new(), 0.0));

                // Straight off the manifest: this picture declared a cell
                // layout, so the tile draws one of its cells whole. True for
                // a single-cell sprite strip too, which has no second frame
                // to reveal it — see `GridEntry::cells`.
                let cells = tile.thumb.as_ref().is_some_and(|t| t.anim.is_some());
                // Loading feedback, straight off the engines that know: the
                // cue being prepared for the program grid, the pad loader
                // for the SFX bank. An ARMED cue is ready — it is only
                // waiting for its beat — so it stops spinning.
                let (loading, failed) = match surface {
                    Surface::Sfx => match self.pads.pad(&tile.asset).map(|p| p.load.clone()) {
                        Some(pads::PadLoad::Loading { .. }) => (true, false),
                        Some(pads::PadLoad::Failed { .. }) => (false, true),
                        _ => (false, false),
                    },
                    _ => (
                        self.cue.loading_asset() == Some(tile.asset),
                        self.cue.failed_asset() == Some(tile.asset),
                    ),
                };
                // The tile says it in words too — a spinner over a dark
                // thumbnail is easy to miss on a 56px pad.
                let state = if loading {
                    "LOADING".to_string()
                } else if failed {
                    "FAILED".to_string()
                } else {
                    state
                };
                GridEntry {
                    asset: tile.asset,
                    title: tile.title.clone(),
                    sub,
                    state,
                    pad: format!("{:02}", index + 1),
                    texture,
                    frames,
                    fps,
                    cells,
                    loading,
                    failed,
                    active,
                    placeholder: false,
                }
            }));
        entries
    }

    fn rebuild_grids(&mut self, cx: &mut Cx) {
        self.refresh_thumbs(cx);
        let video_entries = self.grid_entries(Surface::Video);
        let video = self.ui.widget(cx, ids!(video_grid));
        if let Some(mut pads) = video.borrow_mut::<VjPadMatrix>() {
            // The scrollbar sizes off the catalog's reported TOTAL, not how
            // many pages have streamed in — otherwise the thumb shrinks as
            // `load_more()` pages arrive. But the total counts rows this
            // client HIDES (the legacy per-lump Doom sprites), and a scroll
            // range wider than the content is a run of black tiles with no
            // title in the middle of the list. Subtract what has actually
            // been dropped so far: exact over the loaded prefix, and it can
            // only get more accurate as more pages land.
            let hidden = self.video_model.tiles().len().saturating_sub(video_entries.len());
            pads.set_total(cx, (self.video_model.total as usize).saturating_sub(hidden));
            pads.set_entries(cx, video_entries);
            pads.set_offset(cx, self.apc.bank);
            self.apc.bank = pads.bank;
        }
        self.sync_video_pad_window(cx);
        self.sync_pads();
        for (surface, grid) in [
            (Surface::Music, ids!(music_grid)),
            (Surface::Sfx, ids!(sfx_grid)),
            (Surface::Mesh, ids!(mesh_grid)),
        ] {
            let entries = self.grid_entries(surface);
            let widget = self.ui.widget(cx, grid);
            if let Some(mut grid) = widget.borrow_mut::<VjTileGrid>() {
                grid.set_entries(cx, entries);
            };
        }
        for (surface, label) in [
            // (The VJ grid's count label was removed with the sidebar
            // trim; the music/sfx/mesh surfaces keep theirs.)
            (Surface::Music, ids!(music_count)),
            (Surface::Sfx, ids!(sfx_count)),
            (Surface::Mesh, ids!(mesh_count)),
        ] {
            let model = match surface {
                Surface::Video => &self.video_model,
                Surface::Music => &self.music_model,
                Surface::Sfx => &self.sfx_model,
                Surface::Mesh => &self.mesh_model,
            };
            let mut text = format!("{} / {}", model.tiles().len(), model.total);
            if model.has_more() {
                text.push_str(" +");
            }
            if model.is_loading() {
                text.push_str(" …");
            }
            if let Some(error) = &model.error {
                text = format!("error: {error}");
            }
            self.ui.label(cx, label).set_text(cx, &text);
        }
        self.rebuild_gen_rows(cx);
    }

    fn rebuild_gen_rows(&mut self, cx: &mut Cx) {
        let now = now_ms();
        // Queue position: pending rows count everyone pending who was
        // submitted before them (jobs() iterates newest first).
        let jobs: Vec<&crate::gen::GenJob> = self.gen.jobs().collect();
        let rows = jobs
            .iter()
            .map(|job| {
                let ahead = matches!(job.state, crate::gen::GenJobState::Pending).then(|| {
                    jobs.iter()
                        .filter(|other| {
                            matches!(other.state, crate::gen::GenJobState::Pending)
                                && other.tag < job.tag
                        })
                        .count()
                });
                JobRowEntry::from_job(job, now, ahead)
            })
            .collect();
        let widget = self.ui.widget(cx, ids!(gen_jobs));
        let borrow = widget.borrow_mut::<VjJobList>();
        if let Some(mut list) = borrow {
            list.set_entries(cx, rows);
        }
        let status = match &self.gen.profiles_state {
            ProfilesState::Idle => "".to_string(),
            ProfilesState::Loading => "loading profiles…".to_string(),
            ProfilesState::Ready => match &self.gen.last_error {
                Some(error) => error.clone(),
                None => {
                    let n = self.gen.active_jobs();
                    if n == 0 {
                        String::new()
                    } else {
                        format!("{n}")
                    }
                }
            },
            ProfilesState::Failed(error) => format!("profiles failed: {error}"),
        };
        self.ui.label(cx, ids!(gen_status)).set_text(cx, &status);
    }

    fn update_status_ui(&mut self, cx: &mut Cx) {
        // `VJ_TRACE_WTREE=1` — widget-tree lookup cost per 2 s. `walk_nodes`
        // climbing means the per-frame UI sync is re-deriving lookups the
        // path cache should be answering.
        if self.trace_wtree {
            let now = cx.seconds_since_app_start();
            if now - self.tstats_last > 2.0 {
                let s = cx.widget_tree().stats();
                if self.tstats_last > 0.0 {
                    log!(
                        "wtree/2s: lookups={} misses={} walk_nodes={} inval={}",
                        s.lookups - self.tstats_prev.lookups,
                        s.cache_misses - self.tstats_prev.cache_misses,
                        s.walk_nodes - self.tstats_prev.walk_nodes,
                        s.invalidations - self.tstats_prev.invalidations,
                    );
                }
                self.tstats_last = now;
                self.tstats_prev = s;
            }
        }
        let status_text = std::mem::take(&mut self.status_text);
        // The asset-server status TEXT stays out of the bar (dev noise —
        // "connected 12"); the label is air now. The state lives on in
        // `status_text` for logs and diagnostics.
        let _ = &status_text;
        self.set_status_label(cx, ids!(status_label), "");
        self.status_text = status_text;
        let show = format!("{} · {}", self.lighting_status, self.midi_status);
        self.set_status_label(cx, ids!(show_status_label), &show);
        // Video program labels + position mirror. Each slot header says
        // what its well is showing: LIVE (on program), NEXT (the cue being
        // prepared), HOLD (the previous program, parked until replaced).
        let next = self
            .cue
            .next()
            .map(|i| i.title.clone())
            .unwrap_or_else(|| "—".to_string());
        // A cue that is still fetching/decoding says so, in the bar as well
        // as on its tile: "standby" for something ready and waiting for its
        // beat, "loading" for something still being made ready.
        let loading_now = self.cue.loading_asset().is_some();
        // Push the busy marks straight at the grids: a click has to answer
        // in the next frame, and a grid rebuild only happens when the
        // catalog changes.
        let cue_loading = self.cue.loading_asset();
        let cue_failed = self.cue.failed_asset();
        let video = self.ui.widget(cx, ids!(video_grid));
        if let Some(mut pads) = video.borrow_mut::<VjPadMatrix>() {
            pads.set_busy(cx, cue_loading, cue_failed);
        }
        for path in [ids!(music_grid), ids!(sfx_grid), ids!(mesh_grid)] {
            let widget = self.ui.widget(cx, path);
            if let Some(mut grid) = widget.borrow_mut::<VjTileGrid>() {
                grid.set_busy(cx, cue_loading, cue_failed);
            };
        }
        // ONE standby marker on screen: the deck corner. This row carries
        // only the cued clip's NAME (or the loading note) — no echo.
        let next_text = if next == "—" {
            String::new()
        } else if loading_now {
            format!("loading  {next}")
        } else {
            next.clone()
        };
        self.set_status_label(cx, ids!(next_label), &next_text);
        let live_slot = self.cue.live_slot();
        let held = self.cue.held().map(|(slot, item)| (slot, item.title.clone()));
        let live_title = self.cue.live().map(|i| i.title.clone());
        let next_title = self.cue.next().map(|i| i.title.clone());
        let slot_line = |slot: SlotId| -> (String, String) {
            let name = match slot {
                SlotId::A => "A",
                SlotId::B => "B",
            };
            if live_slot == Some(slot) {
                return (format!("{name}  LIVE"), live_title.clone().unwrap_or_default());
            }
            if let Some((held_slot, title)) = &held {
                if *held_slot == slot && next_title.is_none() {
                    return (format!("{name}  HOLD"), title.clone());
                }
            }
            if live_slot.is_some() || next_title.is_some() {
                let title = next_title.clone().unwrap_or_else(|| "—".to_string());
                let role = if loading_now { "LOAD" } else { "NEXT" };
                return (format!("{name}  {role}"), title);
            }
            (name.to_string(), "—".to_string())
        };
        let (a_role, a_title) = slot_line(SlotId::A);
        let (b_role, b_title) = slot_line(SlotId::B);
        self.sync_slot_controls_ui(cx);
        self.set_status_label(cx, ids!(slot_a_role), &a_role);
        self.set_status_label(cx, ids!(now_label), &a_title);
        self.set_status_label(cx, ids!(slot_b_role), &b_role);
        self.set_status_label(cx, ids!(slot_b_title), &b_title);
        self.set_status_label(cx, ids!(apc_map_label), "");
        let video_error = self.cue.last_error().unwrap_or("").to_string();
        self.set_status_label(cx, ids!(video_error), &video_error);
        let voices = format!("voices {}", self.pads.voice_count());
        self.set_status_label(cx, ids!(sfx_voices), &voices);

        self.refresh_music_surface(cx);

        // Selected pad strip.
        let sel = match self.selected_pad.and_then(|k| self.pads.pad(&k)) {
            Some(pad) => format!("pad: {}", pad.item.title),
            None => "pad: —".to_string(),
        };
        self.set_status_label(cx, ids!(sfx_sel), &sel);

        // ---- stable EXTERNAL SYNC panel ---------------------------------
        // All widgets have fixed geometry; these fixed-width strings may
        // redraw on the existing pump timer without causing layout churn.
        let snap = self
            .sync_worker
            .as_ref()
            .map(|worker| worker.snapshot())
            .unwrap_or_default();
        if snap.frames > self.capture_frames_seen {
            self.capture_frames_seen = snap.frames;
            self.capture_progress_at = Some(Instant::now());
        }
        let flowing = self
            .capture_progress_at
            .is_some_and(|at| at.elapsed() < Duration::from_secs(2));
        let level_steps = ((snap.peak.clamp(0.0, 1.0) * 8.0).round() as usize).min(8);
        let level = format!("{}{}", "#".repeat(level_steps), ".".repeat(8 - level_steps));
        let capture_text = if !self.loopback_selected {
            "SYSTEM AUDIO: NOT AVAILABLE".to_string()
        } else if flowing {
            let mut text = format!(
                "SYSTEM AUDIO: LIVE {:>4.1} kHz [{}]",
                snap.sample_rate as f64 / 1000.0,
                level
            );
            if snap.dropped > 0 {
                text.push_str(&format!(" DROP {}", snap.dropped));
            }
            text
        } else if self.loopback_failed {
            "SYSTEM AUDIO: CAPTURE ERROR".to_string()
        } else {
            "SYSTEM AUDIO: WAITING FOR CAPTURE".to_string()
        };
        let now = Instant::now();
        // Does the machine still own the clock? Ask before drawing anything,
        // so one pump cannot show a lock word from a clock that just changed.
        self.pump_beat_override(
            snap.lock_state,
            snap.beat.as_ref().map(|beat| beat.confidence).unwrap_or(0.0),
        );
        self.pump_beat_clock(&snap);
        // One word, not a sentence: the bar has to fit MASTER and OUTPUT too.
        // A playing deck owns the beat clock: its grid came from a whole
        // file and its playhead from the device clock, which beats
        // listening to the room. The detector is the fallback — and a tap
        // override outranks both while it stands.
        let deck_beat = self.deck_beat();
        let deck_source = deck_beat.as_ref().and(self.decks.sync_leader());
        // The word names the RUNG OF THE LADDER the clock is on — never
        // something that implies detection when a deck is master.
        let lock_text = match self.beat_override.as_ref() {
            // TAP = tempo and phase are the operator's; ONE = only the
            // downbeat was moved and the tempo underneath still rules.
            Some(over) if over.bpm.is_some() => "● TAP",
            Some(_) => "● ONE",
            // A coasting clock says so: it is still running the grid it last
            // believed, and that is the honest word for it.
            None if self.beat_clock.coasting() => "HOLD",
            None if self.clock_source == ClockSource::External => "● EXT",
            None => match deck_source {
                Some(DeckId::A) => "● DECK A",
                Some(DeckId::B) => "● DECK B",
                None => match snap.lock_state {
                    BeatLockState::Unlocked => "FREE",
                    BeatLockState::Acquiring => "SEEK",
                    BeatLockState::Locked => "● LOCK",
                    BeatLockState::Holdover => "HOLD",
                    BeatLockState::Lost => "LOST",
                },
            },
        };
        // Everything below reads the SAME resolved clock the fades, the
        // visual PLL and the loop rate-fit run on.
        let clock = self.current_beat();
        // The NOMINAL tempo, not the effective one: a correction in flight
        // is a transient, and a BPM readout that swings to 280 while the
        // clock catches half a beat is a lie about the music.
        // The clock repaints the BPM field (nominal while the PLL runs, the
        // free floor otherwise — 0.0 is impossible, there is no clockless
        // state); the field yields to the operator's finger or keyboard.
        let shown_bpm = if self.beat_clock.running() {
            self.beat_clock.nominal_bpm() as f64
        } else {
            self.free_bpm
        };
        if let Some(mut field) =
            self.ui.widget(cx, ids!(bpm_field)).borrow_mut::<ValueInput>()
        {
            field.set_value(cx, shown_bpm.clamp(40.0, 300.0));
        };
        // …and the FLOOR follows the live clock while one exists, so
        // losing every source coasts at the last known tempo with the
        // grid lines where they were.
        if self.beat_clock.running() {
            if let Some(beat) = clock.as_ref() {
                let bpm = self.beat_clock.nominal_bpm() as f64;
                if bpm.is_finite() && bpm > 0.0 {
                    self.free_bpm = bpm.clamp(40.0, 300.0);
                    self.free_anchor = beat.next_beat;
                }
            }
        }
        let confidence_text = format!(
            "CONF: {:>3.0}%",
            clock
                .as_ref()
                .map(|beat| beat.confidence * 100.0)
                .unwrap_or(0.0)
                .clamp(0.0, 100.0)
        );
        let phase_text = match clock.as_ref() {
            Some(beat) if !beat.period.is_zero() => {
                let beat = extrapolate_beat(beat, now);
                let until = beat.next_beat.saturating_duration_since(now).as_secs_f64();
                let period = beat.period.as_secs_f64().max(0.001);
                let phase = (1.0 - until / period).clamp(0.0, 1.0);
                let current_beat = (beat.beat_index + BAR_BEATS - 1) % BAR_BEATS;
                let marker = ((phase * 8.0).floor() as usize).min(7);
                let phase_bar: String =
                    (0..8).map(|index| if index == marker { '|' } else { '.' }).collect();
                format!(
                    "BEAT {}/4 [{}] PHASE {:>3.0}%",
                    current_beat + 1,
                    phase_bar,
                    phase * 100.0
                )
            }
            Some(_) | None => "BEAT -/4 [........] PHASE   0%".to_string(),
        };

        self.set_status_label(cx, ids!(external_capture), &capture_text);
        self.set_status_label(cx, ids!(external_lock), lock_text);
        self.refresh_beat_cluster(cx, &snap, clock.as_ref(), flowing);
        self.set_status_label(cx, ids!(external_confidence), &confidence_text);
        self.set_status_label(cx, ids!(external_phase), &phase_text);

        let transition = self.mixer.video_transition_snapshot();
        let video_state = match transition {
            Some(transition) if transition.phase == VideoTransitionPhase::Started => format!(
                "FIRING: FADE {:>3.0}% | DEVICE CLOCK",
                transition.progress * 100.0
            ),
            Some(transition) if transition.phase == VideoTransitionPhase::Missed => {
                "MISSED: RE-ARMING NEXT BOUNDARY".to_string()
            }
            _ => match self.armed_fade.as_ref() {
                Some(armed) => format!(
                    "ARMED: {:<12} IN {:>5.2}s | FADE {:>4.2}s",
                    armed.kind.to_ascii_uppercase(),
                    armed.fire_at.saturating_duration_since(now).as_secs_f32(),
                    armed.secs
                ),
                None if !self.external_sync_enabled => {
                    "BYPASS: IMMEDIATE AUTHORED FADES".to_string()
                }
                None if self.cue.live().is_some() => {
                    "LIVE: NEXT VIDEO LOCKS TO SYSTEM AUDIO".to_string()
                }
                None => "IDLE: SELECT A VIDEO".to_string(),
            },
        };

        // Loop analysis + applied rate for the program slot on screen (or
        // the armed one about to take over).
        let loop_slot = self
            .cue
            .live_slot()
            .or_else(|| self.cue.armed().map(|(_, _, slot)| slot));
        let loop_text = match loop_slot {
            None => "LOOP: NO VIDEO | RATE 1.000".to_string(),
            Some(slot) => {
                let index = slot.index();
                let media_secs = self.players[index]
                    .as_ref()
                    .map(|player| player.duration_secs)
                    .unwrap_or(0.0);
                let raw_report = self
                    .slot_scan[index]
                    .and_then(|revision| self.loop_reports.get(&revision));
                let report = raw_report
                    .filter(|report| loop_report_matches_media(**report, media_secs));
                match (report, self.applied_fit[index]) {
                    // Rate shown is the mixer's ACTUAL resample rate for
                    // this slot, not just the plan.
                    (Some(report), Some(fit)) => format!(
                        "LOOP: {:?} {:.2}s -> {} BEATS | RATE {:.3}",
                        report.detection.kind,
                        report.period_secs,
                        fit.beats,
                        self.mixer.slot_playback_rate(slot)
                    ),
                    (Some(report), None) => match report.detection.kind {
                        LoopKind::Wrap | LoopKind::PingPong => format!(
                            "LOOP: {:?} {:.2}s | RATE 1.000{}",
                            report.detection.kind,
                            report.period_secs,
                            if self.external_sync_enabled { " (NO FIT)" } else { " (BYPASS)" }
                        ),
                        LoopKind::Static => "LOOP: STATIC | RATE 1.000".to_string(),
                        LoopKind::None => "LOOP: NONE DETECTED | RATE 1.000".to_string(),
                    },
                    (None, _) if raw_report.is_some() => {
                        "LOOP: NO RELIABLE CYCLE | RATE 1.000".to_string()
                    }
                    (None, _) => "LOOP: ANALYZING | RATE 1.000".to_string(),
                }
            }
        };
        self.set_status_label(cx, ids!(external_video_state), &video_state);
        self.set_status_label(cx, ids!(external_loop_state), &loop_text);
    }

    // ---- music mode: analysis results and the deck surface -----------------

    /// Take finished whole-track analyses: publish the grid to the engine,
    /// and upload the waveform tiles as textures for the deck surface.
    fn pump_analysis(&mut self, cx: &mut Cx) {
        for done in self.analysis.poll() {
            let index = done.deck.index();
            // The grid goes to the engine first: it decides whether this
            // arrival should engage a sync.
            let cmds = self.decks.grid_ready(done.deck, done.gen, done.analysis.grid);
            if self.decks.deck(done.deck).load_gen != done.gen {
                continue;
            }
            self.run_deck_cmds(cx, cmds);
            self.deck_zoom_tex[index] =
                crate::music_view::zoom_texture(cx, &done.analysis.tiles);
            self.deck_analysis[index] = Some(done.analysis);
            // Separation may have finished before the analysis that defines
            // the column grid: colour whatever is already separated.
            if self.rebuild_stem_colour(done.deck) {
                let tiles = std::mem::take(&mut self.deck_stem_tiles[index]);
                self.deck_stem_tex[index] = crate::music_view::stem_texture(cx, &tiles);
                self.deck_stem_tiles[index] = tiles;
            }
            self.push_deck_wave(cx, done.deck);
            // The grid is what quantizes the karaoke display to the music;
            // a transcript that landed before it must be re-scheduled now.
            self.rebuild_karaoke(cx, done.deck);
            self.music_rows.clear();
        }
    }

    /// What a deck's one status line reads, given where separation stands.
    ///
    /// `busy` is `Some(true)` while the separator is working, `Some(false)`
    /// once it has failed and `None` once it has settled. Working and failed
    /// both KEEP the line: the separator's own words are the only report the
    /// operator gets, and a track whose lyrics were baked in an earlier
    /// session answers from its cache within a second of loading — early
    /// enough to bury "stems: separating…", "stems: model not installed" and
    /// every device error under a line about words.
    fn deck_stem_line(
        busy: Option<bool>,
        stems: &str,
        lyrics: &str,
        stems_ready: bool,
    ) -> String {
        if busy.is_some() && !stems.is_empty() {
            return stems.to_string();
        }
        if !lyrics.is_empty() {
            return lyrics.to_string();
        }
        if !stems.is_empty() {
            return stems.to_string();
        }
        if stems_ready {
            return "stems: live".to_string();
        }
        "stems: full mix".to_string()
    }

    // ---- music mode: stems and lyrics the store already has ----------------

    /// Start the download of a track's precomputed side-channels beside its
    /// audio.
    ///
    /// Arming is all-or-nothing: unless every stem blob is on its way this
    /// deck stays on the local path, because three stems out of four is not
    /// a stem mix. The lyrics document is optional in both directions.
    fn begin_side_channel_fetch(&mut self, deck: DeckId, gen: u64, item: &TrackItem) {
        let index = deck.index();
        let Some(stems) = item.side.stems else { return };
        let lyrics = item.side.lyrics;
        let mut pending = PendingSideChannels::new(gen, false);
        let Some(up) = self.up.as_mut() else { return };
        let Some(runtime) = up.media.get_mut(AUDIO_LANE) else { return };
        for (slot, (blob, len)) in stems.iter().enumerate() {
            let request = ClientRequest::FetchBlob {
                blob: *blob,
                expected_len: Some(*len),
                pin: false,
            };
            let Ok(id) = runtime.submit(request) else {
                // The session is going away; the completions of whatever did
                // go out find no pending set and are dropped.
                return;
            };
            self.media_reqs
                .insert((AUDIO_LANE, id), MediaPurpose::DeckStem { deck, gen, index: slot });
        }
        if let Some((blob, len)) = lyrics {
            let request = ClientRequest::FetchBlob {
                blob,
                expected_len: Some(len),
                pin: false,
            };
            if let Ok(id) = runtime.submit(request) {
                self.media_reqs
                    .insert((AUDIO_LANE, id), MediaPurpose::DeckLyrics { deck, gen });
                pending.want_lyrics = true;
            }
        }
        self.deck_side_channels[index] = Some(pending);
    }

    /// True when this deck's load is being served from the store's own
    /// analysis — the one condition under which separation is skipped.
    fn side_channels_armed(&self, deck: DeckId, gen: u64) -> bool {
        self.deck_side_channels[deck.index()]
            .as_ref()
            .is_some_and(|pending| pending.gen == gen)
    }

    /// One downloaded side-channel file landed.
    fn side_channel_landed(&mut self, deck: DeckId, gen: u64, slot: Option<usize>, path: PathBuf) {
        let index = deck.index();
        {
            let Some(pending) = self.deck_side_channels[index].as_mut() else { return };
            if pending.gen != gen {
                return;
            }
            match slot {
                Some(slot) => {
                    if let Some(entry) = pending.stems.get_mut(slot) {
                        *entry = Some(path);
                    }
                }
                None => {
                    pending.lyrics = Some(path);
                    pending.want_lyrics = false;
                }
            }
        }
        self.try_start_side_channels(deck, gen);
    }

    /// A side-channel file is not coming. Missing lyrics only cost the words;
    /// a missing stem costs the whole set, and the deck separates locally
    /// after all.
    fn side_channel_failed(&mut self, deck: DeckId, gen: u64, stem: bool, error: &str) {
        if !self.side_channels_armed(deck, gen) {
            return;
        }
        let index = deck.index();
        if !stem {
            if let Some(pending) = self.deck_side_channels[index].as_mut() {
                pending.want_lyrics = false;
                pending.lyrics = None;
            }
            self.try_start_side_channels(deck, gen);
            return;
        }
        self.deck_side_channels[index] = None;
        log!("deck {deck:?}: side-channel stem fetch failed ({error}); separating locally");
        self.fall_back_to_separation(deck, gen);
    }

    /// Hand the fetched files to the decode worker, once they are ALL here
    /// and the track itself is installed — the track's rate and length are
    /// what the stems are resampled to and cut by, so neither half is any
    /// use without the other. Called from both sides; whichever completes
    /// last is the one that starts the job.
    fn try_start_side_channels(&mut self, deck: DeckId, gen: u64) {
        let index = deck.index();
        if !self.deck_side_channels[index]
            .as_ref()
            .is_some_and(|pending| pending.gen == gen && pending.complete())
        {
            return;
        }
        if !self.deck_track_is(deck, gen) {
            return;
        }
        let Some((pcm, _)) = self.deck_tracks[index].as_ref() else { return };
        let pcm = pcm.clone();
        let Some(pending) = self.deck_side_channels[index].take() else { return };
        let Some(job) = pending.into_job(deck, gen, pcm) else { return };
        self.deck_stem_status[index] = "stems: side-channel".to_string();
        self.deck_stem_busy[index] = Some(true);
        self.sidechan.submit(job);
    }

    /// Whether `deck_tracks` holds the audio of THIS load generation — the
    /// deck holds the previous track's PCM until the new one installs.
    fn deck_track_is(&self, deck: DeckId, gen: u64) -> bool {
        let state = self.decks.deck(deck);
        state.load_gen == gen && state.is_loaded() && self.deck_tracks[deck.index()].is_some()
    }

    /// Separate locally after all: the fetched side-channel never arrived or
    /// would not decode. When the track has not installed yet there is
    /// nothing to do here — the arming is gone, so `InstallTrack` separates.
    fn fall_back_to_separation(&mut self, deck: DeckId, gen: u64) {
        if !self.deck_track_is(deck, gen) {
            return;
        }
        let Some((pcm, _)) = self.deck_tracks[deck.index()].as_ref() else { return };
        let pcm = pcm.clone();
        self.submit_separation(deck, pcm);
    }

    /// Ask the separator for this deck's stems, starting where the
    /// playhead is so the knobs go live where they are needed first.
    fn submit_separation(&mut self, deck: DeckId, pcm: Arc<TrackPcm>) {
        let state = self.decks.deck(deck);
        let Some(item) = state.item() else { return };
        let source = self.local_by_asset.get(&item.asset).cloned();
        let (position, _duration, _playing) = self.mixer.deck_position(deck);
        self.deck_stem_status[deck.index()] = "stems: queued".to_string();
        self.deck_stem_busy[deck.index()] = Some(true);
        self.stems.submit(StemsJob {
            deck,
            gen: state.load_gen,
            pcm,
            source,
            start_secs: position,
        });
    }

    /// Take separated chunks: install them for playback, and fold their
    /// energy into the waveform's colour so the operator can SEE the
    /// separation arrive.
    fn pump_stems(&mut self, cx: &mut Cx) {
        let mut touched = [false; 2];
        // Two sources, one vocabulary: the local separator and the fetched
        // side-channel publish the same chunks and the same status lines, so
        // everything below this point is blind to which one served the deck.
        let mut messages = self.stems.poll();
        for message in self.sidechan.poll() {
            match message {
                SideChannelMsg::Stems(message) => messages.push(message),
                SideChannelMsg::Fallback { deck, gen, reason } => {
                    log!("deck {deck:?}: side-channel unusable ({reason}); separating locally");
                    self.deck_side_channels[deck.index()] = None;
                    self.fall_back_to_separation(deck, gen);
                }
            }
        }
        for message in messages {
            match message {
                StemsMsg::Status { deck, gen, text, working } => {
                    if self.decks.deck(deck).load_gen != gen {
                        continue;
                    }
                    self.deck_stem_status[deck.index()] = text;
                    // `working: false` is the worker's word for "this is as
                    // far as it got" — the checkpoint is missing, the device
                    // had no room. Latching it here is what keeps the reason
                    // on screen instead of behind the next karaoke line.
                    self.deck_stem_busy[deck.index()] = Some(working);
                }
                StemsMsg::Done { deck, gen } => {
                    if self.decks.deck(deck).load_gen != gen {
                        continue;
                    }
                    self.deck_stem_status[deck.index()] = "stems: live".to_string();
                    self.deck_stem_busy[deck.index()] = None;
                }
                StemsMsg::Coverage { deck, gen, digest, model_frames, complete } => {
                    // The separation worker is the only place the track's
                    // digest exists (hashing decoded PCM is not UI-thread
                    // work), so it is also where the karaoke bake is armed.
                    // A read probe goes out immediately — a track transcribed
                    // in an earlier session shows its words the moment it
                    // loads — and the bake only once the whole vocals stem is
                    // actually on disk.
                    if self.decks.deck(deck).load_gen != gen {
                        continue;
                    }
                    self.deck_track_digest[deck.index()] = Some(digest.clone());
                    // A track this machine separated end to end is worth
                    // giving back — before the dispatch gate below, which
                    // stops at the SECOND report of the same coverage.
                    if complete {
                        self.arm_stems_write_back(deck, &digest, model_frames);
                    }
                    // Words already in hand for this digest — the other deck
                    // played it, or an earlier load did. Hang them now: the
                    // gate below is for JOBS and would refuse the re-ask,
                    // which used to leave a reloaded track on "waiting for
                    // separation" with its stems audibly live.
                    if let Some(lyrics) = self.lyrics_by_digest.get(&digest).cloned() {
                        if self.deck_lyrics[deck.index()].is_none() {
                            self.deck_lyrics_status[deck.index()] =
                                format!("lyrics: {} lines (cached)", lyrics.lines.len());
                            self.deck_lyrics[deck.index()] = Some(lyrics);
                            self.rebuild_karaoke(cx, deck);
                            if self.karaoke_on {
                                self.video_pump = cx.new_next_frame();
                            }
                        }
                        continue;
                    }
                    if !self.lyrics_dispatch.should_dispatch(&digest, complete) {
                        continue;
                    }
                    let (_, duration, _) = self.mixer.deck_position(deck);
                    self.lyrics.submit(LyricsJob {
                        deck,
                        gen,
                        digest,
                        model_frames,
                        duration_secs: duration,
                        bake: complete,
                    });
                }
                StemsMsg::Chunk(chunk) => {
                    let deck = chunk.deck;
                    let index = deck.index();
                    if self.decks.deck(deck).load_gen != chunk.gen {
                        continue;
                    }
                    // Rebuild the published table with the new chunk in it:
                    // the buffers themselves are shared, never copied.
                    let mut table = match self.deck_stems[index].as_ref() {
                        Some(existing) => TrackStems {
                            chunk_frames: existing.chunk_frames,
                            lanes: existing.lanes.clone(),
                        },
                        None => TrackStems::new(chunk.chunk_frames, chunk.chunk_count),
                    };
                    for (lane, block) in table.lanes.iter_mut().zip(chunk.lanes.iter()) {
                        if let Some(slot) = lane.get_mut(chunk.index) {
                            *slot = Some(block.clone());
                        }
                    }
                    let table = Arc::new(table);
                    self.mixer.install_deck_stems(deck, table.clone());
                    self.deck_stems[index] = Some(table);
                    touched[index] = true;
                }
            }
        }
        for deck in [DeckId::A, DeckId::B] {
            let index = deck.index();
            if !touched[index] {
                continue;
            }
            // Rebuild the colour pyramid and re-bind the lane.
            if self.rebuild_stem_colour(deck) {
                let tiles = std::mem::take(&mut self.deck_stem_tiles[index]);
                self.deck_stem_tex[index] = crate::music_view::stem_texture(cx, &tiles);
                self.deck_stem_tiles[index] = tiles;
            }
            self.push_deck_wave(cx, deck);
            // The knobs go live once the stretch under the playhead exists.
            // The chunk length IS the track's rate: one second of it.
            let (position, _duration, _playing) = self.mixer.deck_position(deck);
            let covers = self.deck_stems[index]
                .as_ref()
                .map(|stems| {
                    let rate = stems.chunk_frames as f64 / crate::stems::STEM_CHUNK_SECS;
                    stems.covers((position * rate) as usize)
                })
                .unwrap_or(false);
            if covers && !self.decks.deck(deck).stems_ready {
                let gen = self.decks.deck(deck).load_gen;
                let cmds = self.decks.stems_ready(deck, gen);
                self.run_deck_cmds(cx, cmds);
            }
        }
    }

    /// Take finished transcriptions and hang them on the deck that asked.
    fn pump_lyrics(&mut self, cx: &mut Cx) {
        for message in self.lyrics.poll() {
            match message {
                LyricsMsg::Status { deck, gen, text } => {
                    if self.decks.deck(deck).load_gen != gen {
                        continue;
                    }
                    self.deck_lyrics_status[deck.index()] = text;
                }
                LyricsMsg::Ready { deck, gen, digest, lyrics } => {
                    // The transcript is the TRACK's, not the load's: it is
                    // kept by digest so a reload or the other deck re-hangs
                    // it without a new job — and so a Ready that raced a
                    // re-cue is not lost with its generation, which used to
                    // cost the words for the rest of the session.
                    self.lyrics_by_digest.insert(digest.clone(), lyrics.clone());
                    if self.decks.deck(deck).load_gen == gen {
                        // Words this machine has and the store does not: the
                        // same offer the stems make, and free — the document
                        // is already in hand. Only from the asking deck's
                        // still-current load, so the offer stays attached to
                        // the asset the operator actually cued.
                        self.arm_lyrics_write_back(deck, &digest, &lyrics);
                    }
                    // Hang the words on every deck currently showing this
                    // digest: the asking deck usually, both when both play
                    // the same track, the re-cued deck when the gen went
                    // stale mid-transcription.
                    let mut hung = false;
                    for d in [DeckId::A, DeckId::B] {
                        let i = d.index();
                        if self.deck_track_digest[i].as_deref() != Some(digest.as_str()) {
                            continue;
                        }
                        if self.deck_lyrics[i].is_some() {
                            continue;
                        }
                        self.deck_lyrics[i] = Some(lyrics.clone());
                        self.rebuild_karaoke(cx, d);
                        hung = true;
                    }
                    if hung && self.karaoke_on {
                        self.video_pump = cx.new_next_frame();
                    }
                }
            }
        }
    }

    // ---- music mode: giving the analysis back ------------------------------

    /// Offer a locally separated store track's stems back to the store.
    ///
    /// Everything about this is deliberately timid. It runs ONLY for a track
    /// that came from the store (a local file has no asset to attach to),
    /// ONLY when that track's manifest carried no stems (otherwise there is
    /// nothing to add), and ONLY once per asset per session — a refusal is a
    /// refusal, not something to try again in a loop. The reading and the
    /// encoding happen on the write-back worker, which stands aside before it
    /// starts: the deck it just separated is probably playing.
    fn arm_stems_write_back(&mut self, deck: DeckId, digest: &str, model_frames: u64) {
        let Some(item) = self.decks.deck(deck).item() else { return };
        if item.side.stems.is_some() {
            return;
        }
        let asset = item.asset;
        if self.local_by_asset.contains_key(&asset) || self.up.is_none() {
            return;
        }
        if !self.writeback_stems.insert(asset) {
            return;
        }
        self.writeback.submit(WriteBackJob {
            asset,
            digest: digest.to_string(),
            model_frames,
        });
    }

    /// The same offer for a baked transcript. No encode stands between the
    /// document and the store, so this one goes straight out.
    fn arm_lyrics_write_back(&mut self, deck: DeckId, digest: &str, lyrics: &TrackLyrics) {
        let Some(item) = self.decks.deck(deck).item() else { return };
        if item.side.lyrics.is_some() {
            return;
        }
        let asset = item.asset;
        if self.local_by_asset.contains_key(&asset) || self.up.is_none() {
            return;
        }
        if !self.writeback_lyrics.insert(asset) {
            return;
        }
        let files =
            makepad_audio_sidechannels::side_channel_files(None, Some(lyrics.to_json(digest)));
        self.submit_side_channel_publish(asset, files);
    }

    /// Take finished write-back encodes and offer them to the store.
    fn pump_side_channel_writeback(&mut self) {
        for message in self.writeback.poll() {
            match message {
                WriteBackMsg::Encoded { asset, oggs } => {
                    let files =
                        makepad_audio_sidechannels::side_channel_files(Some(*oggs), None);
                    self.submit_side_channel_publish(asset, files);
                }
                WriteBackMsg::Skipped { asset, reason } => {
                    log!("side-channels: nothing to publish for {asset}: {reason}");
                }
            }
        }
    }

    /// Publish attached side-channel files on the catalog runtime's bulk
    /// lane — the deck's own audio lane stays clear for the next load.
    ///
    /// ONE publication per asset at a time. Each attach reads the head
    /// revision and stages a successor, so two of them in flight together
    /// (stems finishing while the transcript lands) would both build on the
    /// same head and the loser would drop the winner's files. The second
    /// waits for the first to settle and then builds on what it left.
    fn submit_side_channel_publish(
        &mut self,
        asset: AssetId,
        files: Vec<makepad_asset_client::side_channels::SideChannelFile>,
    ) {
        if files.is_empty() {
            return;
        }
        if self.publish_inflight.contains(&asset) {
            if self.publish_deferred.len() < 8 {
                self.publish_deferred.push((asset, files));
            }
            return;
        }
        let bytes: usize = files.iter().map(|file| file.bytes.len()).sum();
        let roles: Vec<FileRole> = files.iter().map(|file| file.role).collect();
        let Some(up) = self.up.as_mut() else { return };
        match up.catalog.submit(ClientRequest::PublishSideChannels {
            asset,
            files: Arc::new(files),
        }) {
            Ok(id) => {
                self.cat_reqs.insert(id, CatPurpose::SideChannelPublish { asset });
                self.publish_inflight.insert(asset);
                log!("side-channels: offering {roles:?} ({bytes} bytes) for {asset}");
            }
            Err(error) => log!("side-channels: publish not submitted: {error}"),
        }
    }

    /// A publication finished, one way or the other: let whatever was waiting
    /// on this asset go, now that it can see the revision this one left.
    fn side_channel_publish_settled(&mut self, asset: AssetId) {
        self.publish_inflight.remove(&asset);
        if let Some(at) = self.publish_deferred.iter().position(|(a, _)| *a == asset) {
            let (asset, files) = self.publish_deferred.remove(at);
            self.submit_side_channel_publish(asset, files);
        }
    }

    /// Rebuild a deck's display schedule. The transcript is in track time and
    /// the grid decides where the appear/leave moments land, so this runs
    /// whenever EITHER arrives — they land in either order, and a track whose
    /// analysis is still running would otherwise keep an ungridded schedule.
    fn rebuild_karaoke(&mut self, cx: &mut Cx, deck: DeckId) {
        let index = deck.index();
        match self.deck_lyrics[index].as_ref() {
            Some(lyrics) => {
                let grid = self.deck_analysis[index].as_ref().map(|a| &a.grid);
                let timing = KaraokeTiming::from_grid(grid);
                self.deck_karaoke[index] = Some(Arc::new(KaraokeSchedule::build(
                    lyrics.lines.clone(),
                    timing,
                )));
            }
            None => self.deck_karaoke[index] = None,
        }
        self.push_deck_lyrics(cx, deck);
    }

    /// Hand the deck's transcript to its reader panel. Only when it changes:
    /// the playhead goes in every frame, the lines a few times a session.
    fn push_deck_lyrics(&mut self, cx: &mut Cx, deck: DeckId) {
        let index = deck.index();
        let rows: Vec<crate::music_view::LyricRow> = self.deck_lyrics[index]
            .as_ref()
            .map(|lyrics| {
                lyrics
                    .lines
                    .iter()
                    .map(|line| crate::music_view::LyricRow {
                        start_secs: line.start_secs,
                        end_secs: line.end_secs,
                        text: line.text.clone(),
                        stamp: crate::music_view::lyric_stamp(line.start_secs),
                        words: line.words.clone(),
                        confident: line.confident,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let hint = if !self.deck_lyrics_status[index].is_empty() {
            self.deck_lyrics_status[index].clone()
        } else if self.decks.deck(deck).is_loaded() {
            "lyrics: waiting for separation".to_string()
        } else {
            String::new()
        };
        let widget = self.music_refs.decks[index].lyrics.clone();
        {
            if let Some(mut reader) = widget.borrow_mut::<crate::music_view::VjLyricReader>() {
                reader.set_lines(cx, rows);
                reader.set_placeholder(cx, &hint);
            }
        }
        drop(widget);
    }

    /// What the program should be showing right now: the live deck's current
    /// line, the next one, and how far the sweep has crossed the current one.
    fn karaoke_overlay(&self) -> Option<crate::views::KaraokeOverlay> {
        if !self.karaoke_on {
            return None;
        }
        let (_, _, playing_a) = self.mixer.deck_position(DeckId::A);
        let (_, _, playing_b) = self.mixer.deck_position(DeckId::B);
        let deck = crate::lyrics::live_deck(self.decks.crossfader, playing_a, playing_b);
        let schedule = self.deck_karaoke[deck.index()].as_ref()?;
        let (position, _, _) = self.mixer.deck_position(deck);
        let frame = schedule.at(position + crate::lyrics::display_offset_secs());
        if frame.current.is_none() && frame.next.is_none() {
            return None;
        }
        Some(crate::views::KaraokeOverlay {
            current: frame.current.and_then(|i| schedule.text(i)).map(str::to_string),
            next: frame.next.and_then(|i| schedule.text(i)).map(str::to_string),
            progress: frame.progress,
        })
    }

    /// Per-zoom-column RMS of every separated stem lane, for the waveform's
    /// colour. Recomputed from the published table rather than folded per
    /// chunk, so it does not matter whether separation or analysis lands
    /// first — the colour is always whatever has actually been separated.
    ///
    /// Colour, and only colour: what goes in the texture is each lane's
    /// SHARE of the column, never its level. A column's height comes from
    /// the track-wide level channel of the analysis tiles, so a span that
    /// has just been separated is exactly as tall as it was a moment
    /// earlier in grey, and no span is ever measured against itself.
    fn rebuild_stem_colour(&mut self, deck: DeckId) -> bool {
        let index = deck.index();
        let Some(analysis) = self.deck_analysis[index].as_ref() else { return false };
        let Some(stems) = self.deck_stems[index].as_ref() else { return false };
        let total_cols = analysis.tiles.zoom.len();
        if total_cols == 0 || stems.chunk_frames == 0 {
            return false;
        }
        let rate = analysis.sample_rate.max(1) as f64;
        let frames_per_col = rate / crate::wave_analysis::ZOOM_COLS_PER_SEC;
        if frames_per_col < 1.0 {
            return false;
        }
        let mut tiles = vec![[0u8; 4]; total_cols];
        for column in 0..total_cols {
            let from = (column as f64 * frames_per_col) as usize;
            let to = ((column + 1) as f64 * frames_per_col) as usize;
            let chunk = from / stems.chunk_frames;
            // A column that straddles two chunks is measured in the first;
            // at a hundred columns a second the difference is invisible.
            let offset = from - chunk * stems.chunk_frames;
            let span = to - from;
            let mut any = false;
            let mut rms = [0.0f64; 4];
            for (lane, blocks) in stems.lanes.iter().enumerate() {
                let Some(Some(block)) = blocks.get(chunk) else { continue };
                let end = (offset + span).min(block.len());
                if offset >= end {
                    continue;
                }
                any = true;
                let mut sum = 0.0f64;
                for frame in &block[offset..end] {
                    let mono = (frame[0] as f64 + frame[1] as f64) * 0.5 / 32768.0;
                    sum += mono * mono;
                }
                rms[lane] = (sum / (end - offset) as f64).sqrt();
            }
            if any {
                tiles[column] = crate::music_view::stem_column_shares(rms);
            }
        }
        let changed = self.deck_stem_tiles[index] != tiles;
        self.deck_stem_tiles[index] = tiles;
        changed
    }

    /// Mirror the mixer's playheads into the engine, which is where every
    /// sync decision is made.
    fn observe_decks(&mut self) {
        for deck in [DeckId::A, DeckId::B] {
            let (position, _duration, playing) = self.mixer.deck_position(deck);
            self.decks.observe(deck, position, playing);
        }
    }

    /// Bind a deck's tiles + grid into the scrolling lane and its overview.
    fn push_deck_wave(&mut self, cx: &mut Cx, deck: DeckId) {
        let index = deck.index();
        let pyramid = self.deck_zoom_tex[index].clone();
        let stem_pyramid = self.deck_stem_tex[index].clone();
        let cols = self
            .deck_analysis[index]
            .as_ref()
            .map(|analysis| analysis.tiles.zoom.len())
            .unwrap_or(0);
        let state = self.decks.deck(deck);
        let lane = WaveLane {
            pyramid,
            stem_pyramid,
            cols,
            position_secs: state.position_secs,
            grid: state.grid,
            rate: state.rate,
            playing: state.playing,
            loaded: state.is_loaded(),
            scratching: self.mixer.deck_scratching(deck),
            stem_gain: [
                state.stem_effective(0),
                state.stem_effective(1),
                state.stem_effective(2),
                state.stem_effective(3),
            ],
            // Stamped by the widget when it takes the lane.
            stamp: 0.0,
        };
        let waves = self.ui.widget(cx, ids!(music_waves));
        if let Some(mut scroll) = waves.borrow_mut::<VjWaveScroll>() {
            scroll.set_lane(cx, deck, lane);
        };
        // The strip is the same store at its deepest levels: one pyramid,
        // both views.
        let strip_widget = self.ui.widget(cx, Self::overview_path(deck));
        if let Some(mut strip) = strip_widget.borrow_mut::<VjWaveOverview>() {
            strip.set_track(
                cx,
                self.deck_zoom_tex[index].clone(),
                self.deck_stem_tex[index].clone(),
                cols,
            );
        };
    }

    fn overview_path(deck: DeckId) -> &'static [LiveId] {
        match deck {
            DeckId::A => ids!(deck_a_overview),
            DeckId::B => ids!(deck_b_overview),
        }
    }

    /// Resolve the deck surface's widgets once, when they first exist.
    fn ensure_music_refs(&mut self, cx: &mut Cx) -> bool {
        if self.music_refs_ready {
            return true;
        }
        let ui = self.ui.clone();
        let refs = MusicRefs::resolve(&ui, cx);
        if !refs.is_live() {
            return false;
        }
        self.music_refs = refs;
        self.music_refs_ready = true;
        true
    }

    /// Set a label only when its text actually changed: `set_text`
    /// re-formats and re-lays-out, and most readouts are the same string
    /// frame after frame.
    fn set_label(&mut self, cx: &mut Cx, key: u64, label: &LabelRef, text: &str) {
        match self.label_cache.get(&key) {
            Some(previous) if previous == text => return,
            _ => {}
        }
        self.label_cache.insert(key, text.to_string());
        label.set_text(cx, text);
    }

    /// One pass over everything the deck surface shows.
    fn refresh_music_surface(&mut self, cx: &mut Cx) {
        if !self.ensure_music_refs(cx) {
            return;
        }
        let levels = self.mixer.deck_levels();
        for deck in [DeckId::A, DeckId::B] {
            let index = deck.index();
            // One mixer lock per deck per frame: the audio callback
            // `try_lock`s and goes silent on contention, so the UI must not
            // grab it three times for three fields.
            let (position, duration, playing, scratching) = self.mixer.deck_snapshot(deck);
            let state = self.decks.deck(deck);
            let (title, artist) = match &state.load {
                DeckLoad::Empty => ("empty".to_string(), String::new()),
                DeckLoad::Loading { item, .. } => {
                    (item.title.clone(), "loading…".to_string())
                }
                DeckLoad::Loaded { item } => (item.title.clone(), String::new()),
                DeckLoad::Failed { item, error } => (item.title.clone(), error.clone()),
            };
            // Copy what the paint pass needs: `paint_lit` takes &mut self.
            let grid = state.grid;
            let rate = state.rate;
            let pitch = state.pitch;
            let synced = state.synced;
            let loaded = state.is_loaded();
            let loop_on = state.loop_on;
            let muted = state.muted;
            let keylock = state.keylock;
            let stems_ready = state.stems_ready;
            let eq_kill = state.eq_kill;
            let stem_kill = state.stem_kill;
            let stem_gains: [f32; 4] = [
                state.stem_effective(0),
                state.stem_effective(1),
                state.stem_effective(2),
                state.stem_effective(3),
            ];
            let range_label = state.pitch_range.label();
            let ids = MusicDeckIds::for_deck(deck);
            let base = (index as u64) << 8;
            let refs = std::mem::take(&mut self.music_refs.decks[index]);
            self.set_label(cx, base, &refs.title, &title);
            self.set_label(cx, base + 1, &refs.artist, &artist);
            self.set_label(cx, base + 2, &refs.bpm, &format_bpm(grid, rate));
            self.set_label(
                cx,
                base + 3,
                &refs.pitch_text,
                &format!("{}{}", format_pitch(pitch), if synced { " SYNC" } else { "" }),
            );
            self.set_label(
                cx,
                base + 4,
                &refs.time,
                &format!("{} / {}", format_time(position), format_time(duration)),
            );
            self.set_label(
                cx,
                base + 5,
                &refs.loop_len,
                &format!("{}", self.deck_loop_beats[index]),
            );
            let grid_text = match grid {
                Some(grid) if grid.has_grid() => {
                    format!("grid {:.1} BPM · {:.0}%", grid.bpm, grid.confidence * 100.0)
                }
                _ if loaded => "analysing…".to_string(),
                _ => String::new(),
            };
            self.set_label(cx, base + 6, &refs.grid_state, &grid_text);
            // One line, and an order of precedence rather than a race. The
            // separation owns it while it is running or while it is broken —
            // those are the two things an operator has to be able to see, and
            // this is the only place either appears. Once separation settles,
            // the karaoke bake takes over, because "stems: live" is old news
            // by the time whisper has anything to say.
            let stem_text = Self::deck_stem_line(
                self.deck_stem_busy[index],
                &self.deck_stem_status[index],
                &self.deck_lyrics_status[index],
                stems_ready,
            );
            self.set_label(cx, base + 7, &refs.stem_state, &stem_text);
            if self.label_cache.get(&(base + 8)).map(String::as_str) != Some(range_label) {
                self.label_cache.insert(base + 8, range_label.to_string());
                refs.range.set_text(cx, range_label);
            }

            // Lit chrome for the toggles the host owns.
            self.paint_lit(cx, ids.play, playing);
            self.paint_lit(cx, ids.loop_button, loop_on);
            self.paint_lit(cx, ids.mute, muted);
            // The SYNC control wears its mode: chrome when free, lit when
            // held against the other deck, and lit reading EXT + the room's
            // tempo when it is following the room instead.
            let mode = self.decks.sync_mode(deck);
            let sync_text = match mode {
                SyncMode::External => match self.beat_clock.running() {
                    true => format!("EXT {:.0}", self.beat_clock.nominal_bpm()),
                    false => "EXT —".to_string(),
                },
                _ => "SYNC".to_string(),
            };
            if self.label_cache.get(&(base + 9)).map(String::as_str) != Some(sync_text.as_str()) {
                self.label_cache.insert(base + 9, sync_text.clone());
                refs.sync.set_text(cx, &sync_text);
            }
            self.paint_lit(cx, ids.sync, mode != SyncMode::Off || synced);
            self.paint_lit(cx, ids.keylock, keylock);
            for (band, kill) in ids.eq_kills.iter().enumerate() {
                self.paint_lit(cx, kill, eq_kill[band]);
            }
            for (stem, kill) in ids.stem_kills.iter().enumerate() {
                self.paint_lit(cx, kill, stem_kill[stem]);
            }
            // The knobs ARE the waveform's legend: each one wears the exact
            // colour of the layer it controls, drained when that layer is
            // killed. One palette feeds both (`music_view::STEM_COLORS`).
            for (stem, knob) in ids.stem_knobs.iter().enumerate() {
                let live = stems_ready && !stem_kill[stem];
                self.paint_stem_knob(cx, deck, stem, knob, ids.stem_labels[stem], live);
            }

            // The channel meter, with a little ballistic decay so it reads.
            let level = levels[index].clamp(0.0, 1.0).sqrt();
            refs.vu.set_uniform(cx, live_id!(level), &[level]);

            // Waveforms.
            if let Some(mut scroll) = self.music_refs.waves.borrow_mut::<VjWaveScroll>() {
                scroll.set_position(cx, deck, position, playing, scratching);
                scroll.set_grid(cx, deck, grid, rate);
                scroll.set_stem_gain(cx, deck, stem_gains);
            };
            if let Some(mut strip) =
                self.music_refs.overviews[index].borrow_mut::<VjWaveOverview>()
            {
                let fraction = if duration > 0.0 { position / duration } else { 0.0 };
                strip.set_head(cx, fraction, playing);
            };
            self.music_refs.decks[index] = refs;
        }
        self.paint_lit(cx, ids!(auto_sync), self.decks.auto_sync);
        self.paint_lit(cx, ids!(music_local), self.music_local);
        self.refresh_music_rows(cx);
    }

    /// Colour one stem knob (and its legend) with its layer's colour.
    fn paint_stem_knob(
        &mut self,
        cx: &mut Cx,
        deck: DeckId,
        stem: usize,
        knob: &[LiveId],
        label: &[LiveId],
        live: bool,
    ) {
        let key = (deck.index() * 8 + stem) as u64;
        if self.stem_paint.get(&key) == Some(&live) {
            return;
        }
        self.stem_paint.insert(key, live);
        let color = if live {
            crate::music_view::stem_color(stem)
        } else {
            crate::music_view::stem_color_killed()
        };
        let body: Vec4f = vec4(
            color.x * 0.22 + 0.04,
            color.y * 0.22 + 0.05,
            color.z * 0.22 + 0.07,
            1.0,
        );
        let mut widget = self.ui.widget(cx, knob);
        script_apply_eval!(cx, widget, {
            draw_bg +: {
                val_color: #(color)
                body_color: #(body)
                pointer_color: #(color)
            }
        });
        let mut legend = self.ui.widget(cx, label);
        script_apply_eval!(cx, legend, {
            draw_text +: { color: #(color) }
        });
    }

    /// Write a status label only when its text actually changed. Both the
    /// widget lookup and `set_text` (redraw + relayout) are skipped when it
    /// did not — which, on the status pump, is nearly every tick.
    fn set_status_label(&mut self, cx: &mut Cx, path: &[LiveId], text: &str) {
        let key = path.iter().fold(0u64, |acc, id| acc ^ id.0.rotate_left(7));
        match self.label_state.get(&key) {
            Some(last) if last == text => return,
            _ => {}
        }
        self.label_state.insert(key, text.to_string());
        self.ui.label(cx, path).set_text(cx, text);
    }

    /// Paint a chrome button as engaged / at rest. Re-applying script
    /// properties to a button every frame is both the app's most expensive
    /// per-frame habit and a way to disturb its own input state, so this
    /// only fires when the state actually changes.
    fn paint_lit(&mut self, cx: &mut Cx, path: &[LiveId], lit: bool) {
        let key = path.iter().fold(0u64, |acc, id| acc ^ id.0.rotate_left(7));
        if self.lit_state.get(&key) == Some(&lit) {
            return;
        }
        self.lit_state.insert(key, lit);
        let mut button = self.ui.button(cx, path);
        // ALL FOUR states, per the latch law at `LatchPaint`: painting only
        // rest+focus leaves hover/down at the theme's UNLIT colours — which
        // is exactly the "hover off goes black on a lit button" bug.
        let p = LatchPaint::icon(lit);
        let (bg, bg_hover, bg_down, fg, fg_hover) =
            (p.bg(), p.bg_hover(), p.bg_down(), p.fg(), p.fg_hover());
        script_apply_eval!(cx, button, {
            draw_bg +: {
                color: #(bg)
                color_focus: #(bg)
                color_hover: #(bg_hover)
                color_down: #(bg_down)
            }
            draw_text +: {
                color: #(fg)
                color_focus: #(fg)
                color_hover: #(fg_hover)
                color_down: #(fg)
            }
        });
    }

    /// Explorer + queue rows. Both are plain views over engine state.
    fn refresh_music_rows(&mut self, cx: &mut Cx) {
        let rows = self.music_row_entries();
        if rows != self.music_rows {
            self.music_rows = rows.clone();
            if let Some(mut list) = self.music_refs.tracks.borrow_mut::<VjTrackList>() {
                list.set_entries(cx, rows);
            };
        }
        let queue: Vec<TrackRowEntry> = self
            .decks
            .queue()
            .iter()
            .enumerate()
            .map(|(index, item)| TrackRowEntry {
                key: TrackKey::Asset(item.asset),
                title: item.title.clone(),
                artist: String::new(),
                bpm: String::new(),
                musical_key: String::new(),
                duration: String::new(),
                tags: String::new(),
                badge: format!("{}", index + 1),
                live: false,
            })
            .collect();
        if queue != self.queue_rows {
            self.queue_rows = queue.clone();
            let count = queue.len();
            if let Some(mut list) = self.music_refs.queue.borrow_mut::<VjTrackList>() {
                list.set_entries(cx, queue);
            };
            let label = self.music_refs.queue_count.clone();
            self.set_label(cx, 0xffff, &label, &format!("{count} waiting"));
        }
    }

    /// Where a track already is, for the row badge.
    fn deck_badge(&self, key: &TrackKey) -> (String, bool) {
        for deck in [DeckId::A, DeckId::B] {
            let state = self.decks.deck(deck);
            let Some(item) = state.item() else { continue };
            let same = match key {
                TrackKey::Asset(asset) => item.asset == *asset,
                TrackKey::Local(path) => self
                    .local_by_asset
                    .get(&item.asset)
                    .is_some_and(|known| known == path),
            };
            if same {
                let label = match deck {
                    DeckId::A => "A",
                    DeckId::B => "B",
                };
                return (label.to_string(), state.playing);
            }
        }
        if let TrackKey::Asset(asset) = key {
            if let Some(index) = self.decks.queue().iter().position(|q| q.asset == *asset) {
                return (format!("Q{}", index + 1), false);
            }
        }
        (String::new(), false)
    }

    /// The explorer's rows: the store's music catalog, or local files.
    fn music_row_entries(&self) -> Vec<TrackRowEntry> {
        if self.music_local {
            return self
                .local_tracks
                .iter()
                .map(|path| {
                    let key = TrackKey::Local(path.clone());
                    let (badge, live) = self.deck_badge(&key);
                    let title = path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default();
                    TrackRowEntry {
                        key,
                        title,
                        artist: "local file".to_string(),
                        bpm: String::new(),
                        musical_key: String::new(),
                        duration: String::new(),
                        tags: path
                            .parent()
                            .map(|dir| dir.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        badge,
                        live,
                    }
                })
                .collect();
        }
        self.music_model
            .tiles()
            .iter()
            .map(|tile| {
                let key = TrackKey::Asset(tile.asset);
                let (badge, live) = self.deck_badge(&key);
                // BPM and duration come from whichever deck holds it; a
                // track that has never been on a deck has not been analysed.
                let mut bpm = String::new();
                let mut duration = String::new();
                for deck in [DeckId::A, DeckId::B] {
                    let state = self.decks.deck(deck);
                    if state.item().map(|item| item.asset) != Some(tile.asset) {
                        continue;
                    }
                    if let Some(grid) = state.grid.filter(|grid| grid.has_grid()) {
                        bpm = format!("{:.1}", grid.bpm);
                    }
                    duration = format_duration(state.duration_secs);
                }
                TrackRowEntry {
                    key,
                    title: tile.title.clone(),
                    artist: String::new(),
                    bpm,
                    musical_key: String::new(),
                    duration,
                    tags: tile.alias.clone().unwrap_or_default(),
                    badge,
                    live,
                }
            })
            .collect()
    }


    /// The program-side deck's loaded clip for the enhance pipe: the slot
    /// the crossfader currently shows (ties break to A), falling back to
    /// the other deck so "the playing clip" never needs pixel-perfect fader
    /// position. `None` when neither deck holds a catalog clip.
    fn program_clip_source(&self) -> Option<(String, String)> {
        let program = if self.program_mix <= 0.5 { 0 } else { 1 };
        [program, 1 - program]
            .into_iter()
            .find_map(|slot| self.slot_scan[slot])
            .map(|revision| {
                let id = revision.to_string();
                let short: String = id.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
                (id, format!("clip …{short}"))
            })
    }

    // ---- video frame pump ---------------------------------------------------

    fn pump_video(&mut self, cx: &mut Cx) {
        // Device-clock transition confirmations run at display-frame rate
        // for tight picture starts (the 20 Hz poll is only a fallback).
        self.pump_transitions(cx);
        // AUTOFADE walks the fader itself, at the same speed the FADE knob
        // gives an armed fade.
        if let Some(mix) = self.auto_fade.tick(makepad_render::TICK_DT, self.program_mix) {
            self.set_visual_mix(cx, mix);
            if !self.auto_fade.active() {
                self.sync_autofade_ui(cx);
            }
        }
        for index in 0..2 {
            let Some(player) = self.players[index].as_mut() else { continue };
            if let Some(frame) = player.take_due_frame() {
                let (w, h) = (player.width as usize, player.height as usize);
                self.light_samples[index] = Some(
                    self.light_analyzers[index].push_bgra_frame(&frame, w, h),
                );
                // Feed the loop analyzer: a bounded point-sampled signature
                // per presented frame; everything heavier is off-thread.
                if let (Some(revision), Some(tx)) =
                    (self.slot_scan[index], self.loop_tx.as_ref())
                {
                    let sig =
                        build_frame_signature(&frame, w, h, &mut self.sig_states[index]);
                    let _ = tx.send(LoopScanCtl::Sig {
                        slot: index,
                        revision,
                        position_secs: player.position_secs(),
                        sig,
                    });
                }
                match (&self.slot_textures[index], self.slot_tex_borrowed[index]) {
                    (Some(tex), false) => tex.set_data_u32(cx, w, h, frame),
                    _ => {
                        // First frame over a borrowed thumbnail (or an
                        // empty slot): a FRESH texture — the thumb handle
                        // belongs to the grid tiles.
                        self.slot_tex_borrowed[index] = false;
                        self.slot_textures[index] = Some(Texture::new_with_format(
                            cx,
                            TextureFormat::VecBGRAu8_32 {
                                width: w,
                                height: h,
                                data: Some(frame),
                                updated: TextureUpdated::Full,
                            },
                        ));
                    }
                }
                // The real picture is here: the cue-ack spinner goes.
                self.set_deck_busy(cx, SlotId::from_index(index), false);
            }
        }
        self.pump_billboards(cx);
        // Live splat scenes orbit slowly and re-render every frame.
        let now = cx.seconds_since_app_start();
        let dt = (now - self.last_splat_pump.replace(now).unwrap_or(now)).clamp(0.0, 0.25) as f32;
        for slot in [SlotId::A, SlotId::B] {
            self.pump_splat(cx, slot, dt);
            // Flow-warp transport: the free-running pair-space clock, one
            // step per display frame (parked players cost nothing above).
            self.pump_flow(cx, slot, dt as f64);
        }
        // THE BEAT EDGE: one pulse per beat boundary to every synced video
        // deck — the transport turn/wrap authority of the sync law.
        {
            let beat = self.current_beat();
            if let Some(beat) = beat.as_ref() {
                let now_i = Instant::now();
                let fire = match self.beat_edge {
                    Some(edge) => now_i >= edge,
                    None => false,
                };
                if fire {
                    for i in 0..2 {
                        if self.slot_beat_sync[i] && self.external_sync_enabled {
                            if let Some(player) = self.players[i].as_mut() {
                                player.beat_pulse();
                            }
                        }
                    }
                }
                if fire || self.beat_edge.is_none() {
                    let mut next = beat.next_beat;
                    if next <= now_i {
                        next = now_i + beat.period;
                    }
                    self.beat_edge = Some(next);
                }
            }
        }
        // The program mix mirrors the device-clock transition exactly: the
        // audio gains and this visual mix advance from one sample counter,
        // so what you see crossfades in lockstep with what you hear.
        let mix = self.live_program_mix();
        self.sync_xfader_ui(cx, mix);
        self.publish_program_lighting(mix);
        let mesh_a = self.slot_mesh_source(cx, SlotId::A).or_else(|| self.slot_splat_source(cx, SlotId::A));
        let mesh_b = self.slot_mesh_source(cx, SlotId::B).or_else(|| self.slot_splat_source(cx, SlotId::B));
        // Flow warp is a pre-pass: its output texture REPLACES the decoder
        // texture for that slot; everything downstream is untouched.
        let flow_a = self.slot_flow_source(cx, SlotId::A);
        let flow_b = self.slot_flow_source(cx, SlotId::B);
        // Content-mode effects: run their clocks, then read their pictures
        // exactly like the mesh/splat offscreen slots.
        self.pump_fx_content(cx);
        let fxc_a = self.slot_fx_content_source(cx, SlotId::A);
        let fxc_b = self.slot_fx_content_source(cx, SlotId::B);
        let source = |index: usize,
                      kind: SlotMedia,
                      mesh: Option<(Texture, f32)>,
                      flow: Option<(Texture, f32)>,
                      fx_content: Option<(Texture, f32)>,
                      players: &[Option<SlotPlayer>; 2],
                      textures: &[Option<Texture>; 2],
                      aspects: &[f32; 2]| {
            if kind == SlotMedia::Mesh || kind == SlotMedia::Splat {
                return mesh;
            }
            if kind == SlotMedia::Effect {
                return fx_content;
            }
            if kind == SlotMedia::Video {
                if let Some(flow) = flow {
                    return Some(flow);
                }
            }
            let tex = textures[index].clone()?;
            let aspect = players[index]
                .as_ref()
                .map(|p| p.width.max(1) as f32 / p.height.max(1) as f32)
                .unwrap_or(aspects[index]);
            Some((tex, aspect))
        };
        let a = source(
            0,
            self.slot_media[0],
            mesh_a,
            flow_a,
            fxc_a,
            &self.players,
            &self.slot_textures,
            &self.slot_aspect,
        );
        let b = source(
            1,
            self.slot_media[1],
            mesh_b,
            flow_b,
            fxc_b,
            &self.players,
            &self.slot_textures,
            &self.slot_aspect,
        );
        // The SOURCE minis show the raw decks before any effect touches
        // them; the deck monitors above show the composite.
        self.pump_source_monitors(cx, &a, &b);
        // The downstream stage is a plain crossfade now — transition STYLE
        // comes from the TRANSITION slot's document (empty slot = dissolve).
        let mix_state = MixState::default();
        // EFFECT SLOTS: per-deck effect passes over the deck sources, and
        // the transition effect while the crossfader travels. Empty or
        // bypassed slots leave a/b/mix untouched.
        let ((a, b), (pa, pb, pmix, pstate)) =
            self.apply_fx_slots(cx, a, b, mix, mix_state);
        self.pump_fx_slot_tiles(cx);
        let karaoke = self.karaoke_overlay();
        for (is_output, target) in [(true, ids!(program)), (false, ids!(preview))] {
            let widget = self.ui.widget(cx, target);
            let borrow = widget.borrow_mut::<views::VideoProgram>();
            if let Some(mut program) = borrow {
                program.set_sources(cx, pa.clone(), pb.clone(), pmix, pstate);
                // FADEOUT dims ONLY the OUTPUT window — the room goes
                // dark while the operator's in-app monitor keeps full
                // brightness (the console never goes blind).
                program.set_fadeout(cx, if is_output { self.fadeout } else { 0.0 });
                program.set_karaoke(cx, karaoke.clone());
            }
        }
        if let Some(mut deck) = self
            .ui
            .widget(cx, ids!(preview_a))
            .borrow_mut::<views::VideoProgram>()
        {
            deck.set_sources(cx, a.clone(), None, 0.0, MixState::default());
        }
        if let Some(mut deck) = self
            .ui
            .widget(cx, ids!(preview_b))
            .borrow_mut::<views::VideoProgram>()
        {
            deck.set_sources(cx, b.clone(), None, 0.0, MixState::default());
        }
        self.ui.view(cx, ids!(deck_a_empty)).set_visible(cx, a.is_none());
        self.ui.view(cx, ids!(deck_b_empty)).set_visible(cx, b.is_none());
        let transition_live = self.mixer.video_transition_snapshot().is_some_and(|t| {
            matches!(
                t.phase,
                VideoTransitionPhase::Armed | VideoTransitionPhase::Started
            )
        });
        if transition_live
            || self
                .players
                .iter()
                .flatten()
                .any(SlotPlayer::needs_frame_pump)
            || self.slot_media.iter().any(|kind| *kind != SlotMedia::Empty)
            // A loaded, switched-on effect slot keeps the pump alive: a
            // standalone generator effect on an empty deck IS the program.
            || self.fx_slots.any_running()
            // A refusal flash needs frames to expire on.
            || self.fx_slots.any_flash()
            || self.fadeout > 0.001
            // The sweep across the current line moves every frame, so
            // karaoke keeps the pump alive on its own — a black program
            // with subtitles is a legitimate output. The condition is the
            // TOGGLE, not the overlay: during an instrumental there is
            // nothing to draw, and a pump that stopped there would never
            // wake up for the verse.
            || (self.karaoke_on
                && (self.mixer.deck_position(DeckId::A).2
                    || self.mixer.deck_position(DeckId::B).2))
        {
            self.video_pump = cx.new_next_frame();
        }
    }

    // ---- clicks -------------------------------------------------------------

    fn grid_hits(
        &mut self,
        cx: &mut Cx,
        actions: &Actions,
        grid: &[LiveId],
    ) -> (Vec<AssetId>, Vec<AssetId>) {
        let widget = self.ui.widget(cx, grid);
        let (cols, len) = match widget.borrow::<VjTileGrid>() {
            Some(grid) => (grid.last_cols.max(1), grid.len()),
            None => return (Vec::new(), Vec::new()),
        };
        let list = widget.portal_list(cx, ids!(list));
        let slots = [
            ids!(c1),
            ids!(c2),
            ids!(c3),
            ids!(c4),
            ids!(c5),
            ids!(c6),
            ids!(c7),
            ids!(c8),
        ];
        let mut down = Vec::new();
        let mut up = Vec::new();
        for (row_id, item) in list.items_with_actions(actions) {
            for (slot, path) in slots.iter().enumerate().take(GRID_SLOTS) {
                let index = row_id * cols + slot;
                if index >= len {
                    continue;
                }
                let cell = item.view(cx, *path);
                if cell.finger_down(actions).is_some() {
                    if let Some(entry) =
                        widget.borrow::<VjTileGrid>().and_then(|g| g.entry_at(index).cloned())
                    {
                        down.push(entry.asset);
                    }
                }
                if cell.finger_up(actions).is_some() {
                    if let Some(entry) =
                        widget.borrow::<VjTileGrid>().and_then(|g| g.entry_at(index).cloned())
                    {
                        up.push(entry.asset);
                    }
                }
            }
        }
        (down, up)
    }

    fn pad_matrix_hits(
        &mut self,
        cx: &mut Cx,
        actions: &Actions,
        grid: &[LiveId],
    ) -> (Vec<(AssetId, KeyModifiers)>, Vec<AssetId>) {
        let widget = self.ui.widget(cx, grid);
        if widget.borrow::<VjPadMatrix>().is_none() {
            return (Vec::new(), Vec::new());
        }
        let rows = [ids!(r0), ids!(r1), ids!(r2), ids!(r3), ids!(r4)];
        let slots = [
            ids!(c1),
            ids!(c2),
            ids!(c3),
            ids!(c4),
            ids!(c5),
            ids!(c6),
            ids!(c7),
            ids!(c8),
        ];
        let mut down = Vec::new();
        let mut up = Vec::new();
        for (row, row_id) in rows.iter().enumerate() {
            let row_view = widget.view(cx, *row_id);
            for (slot, path) in slots.iter().enumerate() {
                let pad = row * 8 + slot;
                let cell = row_view.view(cx, *path);
                if let Some(fe) = cell.finger_down(actions) {
                    let entry =
                        widget.borrow::<VjPadMatrix>().and_then(|g| g.visible_at(pad).cloned());
                    if self.trace_cue {
                        // The press landed on a cell. Whether it becomes a
                        // cue depends on the bank window still pointing at a
                        // loaded entry — the one way a click can vanish.
                        log!(
                            "click: pad {pad} down, entry {:?}",
                            entry.as_ref().map(|e| e.title.clone())
                        );
                    }
                    if let Some(entry) = entry {
                        down.push((entry.asset, fe.modifiers));
                    }
                }
                if cell.finger_up(actions).is_some() {
                    if let Some(entry) =
                        widget.borrow::<VjPadMatrix>().and_then(|g| g.visible_at(pad).cloned())
                    {
                        up.push(entry.asset);
                    }
                }
            }
        }
        (down, up)
    }

    /// THE ROUTING LAWS (user-ratified):
    ///  1. CONTENT (video/image/mesh/splat/sprite) auto-cues into the real
    ///     deck A/B source, autofade included — the classic flow.
    ///  2. An FX tile routes to the EFFECT LAYER: the armed slot, else the
    ///     STANDBY side's effect slot — a plain effect click never
    ///     displaces the video playing on a deck. SHIFT-click keeps the
    ///     explicit effect-AS-CONTENT cue (standalone generator on a deck).
    ///  3. Slot type law: A/B slots accept only vjeffects, the TRANSITION
    ///     slot only transition-tagged ones. A wrong-type click while a
    ///     slot is armed is a VISIBLE refusal on that slot — never a
    ///     silent accept, never a surprise cue.
    fn video_tile_clicked(&mut self, cx: &mut Cx, asset: AssetId, as_content: bool) {
        let Some(tile) = self.video_model.tile(&asset) else { return };
        // The ring follows the hand, not the cue: it marks the tile the
        // operator last touched even while its manifest is still resolving.
        self.last_clicked = Some(asset);
        let is_effect = tile.kind == Some(AssetKind::VjEffect);
        let transition_ok = Self::tile_is_transition_fx(&self.video_model, tile);
        if is_effect && !as_content {
            // Transitions auto-route to their one home; a plain effect
            // lands on the MOST-FADED-OUT side's slot (and, with AUTOFADE
            // on, sweeps in once loaded). An armed slot is the precision
            // path and wins over both.
            let kind = if let Some(kind) = self.fx_slots.armed {
                kind
            } else if transition_ok {
                FxSlotKind::Transition
            } else {
                self.standby_fx_slot()
            };
            match FxSlots::accepts(kind, true, transition_ok) {
                Ok(()) => self.fx_effect_tile_clicked(cx, kind, asset),
                Err(msg) => self.refuse_fx_slot(cx, kind, msg),
            }
            // (The accepted load consumes the arm inside
            // `fx_effect_tile_clicked` — one-shot, see `consume_armed`.)
            return;
        }
        if !is_effect {
            if let Some(kind) = self.fx_slots.armed {
                // Assignment mode: content can never land in an effect
                // slot. Refuse loudly; disarm to cue clips again.
                self.refuse_fx_slot(cx, kind, "FX docs only");
                return;
            }
        }
        let Some(item) = CueItem::from_tile(tile) else {
            // Manifest not resolved yet: the click is not lost — it fires
            // the moment the manifest lands (otherwise a fresh tile needs a
            // second click). Resolve it ahead of the queue.
            self.pending_click = Some((asset, as_content));
            let cmds = self.video_model.resolve_first(asset);
            self.run_cat_cmds(Surface::Video, cmds);
            self.grids_dirty = true;
            return;
        };
        self.pending_click = None;
        let cmds = self.cue.click(item);
        self.run_cue_cmds(cx, cmds);
        self.grids_dirty = true;
    }

    /// The effect slot of the deck the crossfader is NOT on — where an
    /// unarmed FX-tile click lands, mirroring how content cues to standby.
    fn standby_fx_slot(&self) -> FxSlotKind {
        if self.program_mix < 0.5 {
            FxSlotKind::EffectB
        } else {
            FxSlotKind::EffectA
        }
    }

    /// Whether a catalog tile is a transition-suited effect, as far as this
    /// client can KNOW: it came through the transition-tagged lane, or its
    /// alias names a bundled transition preset. (Search hits do not carry
    /// tags; a user-authored transition doc is loadable from the TRANSITION
    /// chip's lane, where the server has already vouched for the tag.)
    fn tile_is_transition_fx(model: &BrowseModel, tile: &catalog::Tile) -> bool {
        if model.tag == crate::effects::seed::TRANSITION_TAG {
            return true;
        }
        Self::alias_is_transition(tile.alias.as_deref())
    }

    /// A bundled transition preset, by alias — the client-side certainty
    /// that needs no server round trip (thumbnails key on it too).
    fn alias_is_transition(alias: Option<&str>) -> bool {
        alias
            .and_then(|alias| alias.strip_prefix("vjfx/"))
            .is_some_and(crate::effects::seed::is_transition_preset)
    }

    /// A wrong-type click while a slot is armed: flash the slot with the
    /// reason. Nothing loads, nothing cues.
    fn refuse_fx_slot(&mut self, cx: &mut Cx, kind: FxSlotKind, msg: &str) {
        let now = cx.seconds_since_app_start();
        self.fx_slots.refuse(kind, msg, now);
        self.sync_fx_slots_ui(cx);
        self.video_pump = cx.new_next_frame();
        log!("fx slot {kind:?}: refused — {msg}");
    }

    fn music_tile_clicked(&mut self, cx: &mut Cx, asset: AssetId) {
        let Some(tile) = self.music_model.tile(&asset) else { return };
        let (Some(revision), Some(media)) = (tile.revision, tile.media.clone()) else {
            return;
        };
        let item = TrackItem {
            asset,
            revision,
            title: tile.title.clone(),
            media_blob: media.blob,
            media_len: media.len,
            media: media.media,
            side: self.track_side_channels.get(&revision).cloned().unwrap_or_default(),
        };
        let cmds = self.decks.click(item, self.deck_target);
        self.run_deck_cmds(cx, cmds);
        self.grids_dirty = true;
    }

    fn mesh_tile_clicked(&mut self, cx: &mut Cx, asset: AssetId) {
        let Some(tile) = self.mesh_model.tile(&asset) else { return };
        let (Some(_revision), Some(media)) = (tile.revision, tile.media.clone()) else {
            return;
        };
        self.mesh_gen += 1;
        self.mesh_now = tile.title.clone();
        let title = tile.title.clone();
        let gen = self.mesh_gen;
        let (lane, cancel) = self.mesh_plan.begin();
        if let (Some(up), Some(stale)) = (self.up.as_ref(), cancel) {
            if let Some(runtime) = up.media.get(stale.lane) {
                runtime.cancel(stale.request);
            }
        }
        let Some(up) = self.up.as_mut() else { return };
        let Some(runtime) = up.media.get_mut(lane) else { return };
        if let Ok(id) = runtime.submit(ClientRequest::FetchBlob {
            blob: media.blob,
            expected_len: Some(media.len),
            pin: false,
        }) {
            self.mesh_plan.submitted(lane, id, gen);
            self.media_reqs.insert((lane, id), MediaPurpose::Mesh { gen });
            self.set_mesh_status(cx, &format!("fetching {title}…"));
        }
    }

    // ---- music mode: the deck surface's controls ---------------------------

    /// Every per-deck control on the music surface, plus the shared
    /// crossfader row.
    fn handle_deck_controls(&mut self, cx: &mut Cx, actions: &Actions) {
        if !self.ensure_music_refs(cx) {
            return;
        }
        for deck in [DeckId::A, DeckId::B] {
            // Take the resolved refs out for the duration: every check below
            // is a pointer deref, not a walk of the widget tree.
            let refs = std::mem::take(&mut self.music_refs.decks[deck.index()]);
            if refs.play.clicked(actions) {
                let cmds = self.decks.play_pause(deck);
                self.run_deck_cmds(cx, cmds);
            }
            if refs.cue.clicked(actions) {
                // Cue: stop and return to the start of the track.
                let mut cmds = Vec::new();
                if self.decks.deck(deck).playing {
                    cmds.extend(self.decks.play_pause(deck));
                }
                cmds.extend(self.decks.seek_secs(deck, 0.0));
                self.run_deck_cmds(cx, cmds);
            }
            if refs.loop_button.clicked(actions) {
                let cmds = self.decks.toggle_loop(deck);
                self.run_deck_cmds(cx, cmds);
            }
            if refs.loop_halve.clicked(actions) {
                let beats = &mut self.deck_loop_beats[deck.index()];
                *beats = (*beats / 2).max(1);
            }
            if refs.loop_double.clicked(actions) {
                let beats = &mut self.deck_loop_beats[deck.index()];
                *beats = (*beats * 2).min(64);
            }
            if refs.mute.clicked(actions) {
                let cmds = self.decks.toggle_mute(deck);
                self.run_deck_cmds(cx, cmds);
            }
            if refs.sync.clicked(actions) {
                // OFF → SYNC (the other deck) → EXT (the room) → OFF.
                let cmds = self.decks.cycle_sync(deck);
                self.run_deck_cmds(cx, cmds);
                self.sync_deck_controls(cx);
            }
            if refs.keylock.clicked(actions) {
                let cmds = self.decks.toggle_keylock(deck);
                self.run_deck_cmds(cx, cmds);
            }
            if refs.range.clicked(actions) {
                let cmds = self.decks.toggle_pitch_range(deck);
                self.run_deck_cmds(cx, cmds);
                self.sync_deck_controls(cx);
            }
            if refs.pitch_reset.clicked(actions) {
                let cmds = self.decks.reset_pitch(deck);
                self.run_deck_cmds(cx, cmds);
                self.sync_deck_controls(cx);
            }
            if let Some(value) = refs.pitch.slided(actions) {
                let cmds = self.decks.set_pitch(deck, value);
                self.run_deck_cmds(cx, cmds);
            }
            if let Some(value) = refs.gain.slided(actions) {
                let cmds = self.decks.set_gain(deck, value as f32);
                self.run_deck_cmds(cx, cmds);
            }
            if let Some(value) = refs.filter.slided(actions) {
                let cmds = self.decks.set_filter(deck, value as f32);
                self.run_deck_cmds(cx, cmds);
            }
            for (band, knob) in refs.eq_knobs.iter().enumerate() {
                if let Some(value) = knob.slided(actions) {
                    let cmds = self.decks.set_eq(deck, band, value as f32);
                    self.run_deck_cmds(cx, cmds);
                }
            }
            for (band, kill) in refs.eq_kills.iter().enumerate() {
                if kill.clicked(actions) {
                    let cmds = self.decks.toggle_eq_kill(deck, band);
                    self.run_deck_cmds(cx, cmds);
                }
            }
            for (stem, knob) in refs.stem_knobs.iter().enumerate() {
                if let Some(value) = knob.slided(actions) {
                    let cmds = self.decks.set_stem(deck, stem, value as f32);
                    self.run_deck_cmds(cx, cmds);
                }
            }
            for (stem, kill) in refs.stem_kills.iter().enumerate() {
                if kill.clicked(actions) {
                    let cmds = self.decks.toggle_stem_kill(deck, stem);
                    self.run_deck_cmds(cx, cmds);
                }
            }
            self.music_refs.decks[deck.index()] = refs;
        }
        if self.music_refs.auto_sync.clicked(actions) {
            let on = !self.decks.auto_sync;
            let cmds = self.decks.set_auto_sync(on);
            self.run_deck_cmds(cx, cmds);
        }
        self.handle_wave_input(cx);
    }

    /// One display frame of the deck surface: fresh playheads into the
    /// lanes and nothing else. Scheduled while a deck is playing or a hand
    /// is on a record, so a scratch tracks at the display's rate rather
    /// than the console's poll rate.
    fn pump_music_frame(&mut self, cx: &mut Cx) {
        self.push_wave_positions(cx);
        self.schedule_music_frame(cx);
    }

    /// Ask for another frame while anything on the surface is moving.
    fn schedule_music_frame(&mut self, cx: &mut Cx) {
        let moving = [DeckId::A, DeckId::B].iter().any(|deck| {
            let (_, _, playing, scratching) = self.mixer.deck_snapshot(*deck);
            playing || scratching
        });
        if moving {
            self.music_pump = cx.new_next_frame();
            // Karaoke lives on the PROGRAM, which normally only redraws when
            // there is video to redraw. A DJ set over a black program is the
            // usual case, so a playing deck has to keep the program's pump
            // alive too or the words would never advance.
            if self.karaoke_on {
                self.video_pump = cx.new_next_frame();
            }
        }
    }

    /// Fresh playheads into the lanes, and nothing else: this runs at
    /// display cadence during a scratch, so it stays uniform-only work.
    fn push_wave_positions(&mut self, cx: &mut Cx) {
        for deck in [DeckId::A, DeckId::B] {
            let (position, _, _, _) = self.mixer.deck_snapshot(deck);
            let position = position + crate::lyrics::display_offset_secs();
            let widget = self.music_refs.decks[deck.index()].lyrics.clone();
            {
                if let Some(mut reader) =
                    widget.borrow_mut::<crate::music_view::VjLyricReader>()
                {
                    reader.set_position(cx, position);
                }
            }
            drop(widget);
        }
        let Some(mut scroll) = self.music_refs.waves.borrow_mut::<VjWaveScroll>() else {
            return;
        };
        for deck in [DeckId::A, DeckId::B] {
            let (position, _duration, playing, scratching) = self.mixer.deck_snapshot(deck);
            scroll.set_position(cx, deck, position, playing, scratching);
        }
    }

    /// Pointer work on the waveforms: scratching the zoomed lanes, seeking
    /// on the overview strips.
    fn handle_wave_input(&mut self, cx: &mut Cx) {
        let events = match self.music_refs.waves.borrow_mut::<VjWaveScroll>() {
            Some(mut scroll) => scroll.take_events(),
            None => Vec::new(),
        };
        for event in events {
            match event {
                WaveEvent::ScratchStart { deck } => {
                    let cmds = self.decks.scratch(deck, ScratchMotion::Grab);
                    self.run_deck_cmds(cx, cmds);
                }
                WaveEvent::ScratchRate { deck, rate } => {
                    let cmds = self.decks.scratch(deck, ScratchMotion::Move { rate });
                    self.run_deck_cmds(cx, cmds);
                }
                WaveEvent::ScratchEnd { deck } => {
                    let cmds = self.decks.scratch(deck, ScratchMotion::Release);
                    self.run_deck_cmds(cx, cmds);
                }
                WaveEvent::Zoom { .. } => {}
                // Every frame of a drag: hand the lanes the playhead the
                // mixer is actually at, so a scratch tracks at display rate.
                WaveEvent::Tick => self.push_wave_positions(cx),
            }
        }
        for deck in [DeckId::A, DeckId::B] {
            let events = match self.music_refs.overviews[deck.index()]
                .borrow_mut::<VjWaveOverview>()
            {
                Some(mut strip) => strip.take_events(),
                None => Vec::new(),
            };
            for OverviewEvent::Seek { fraction } in events {
                let duration = self.decks.deck(deck).duration_secs;
                let cmds = self.decks.seek_secs(deck, fraction * duration);
                self.run_deck_cmds(cx, cmds);
            }
            // Click a lyric line to put the needle on it — the shortest way
            // there is to check whether a line's timing is right.
            let widget = self.music_refs.decks[deck.index()].lyrics.clone();
            let events = {
                match widget.borrow_mut::<crate::music_view::VjLyricReader>() {
                    Some(mut reader) => reader.take_events(),
                    None => Vec::new(),
                }
            };
            drop(widget);
            for crate::music_view::LyricEvent::Seek { secs } in events {
                let cmds = self.decks.seek_secs(deck, secs);
                self.run_deck_cmds(cx, cmds);
            }
        }
    }

    /// The explorer + queue rows, and the local-files switch.
    fn handle_music_rows(&mut self, cx: &mut Cx, actions: &Actions) {
        if !self.ensure_music_refs(cx) {
            return;
        }
        if self.music_refs.music_local.clicked(actions) {
            self.music_local = !self.music_local;
            if self.music_local && self.local_tracks.is_empty() {
                self.local_tracks = wave_analysis::list_local_audio(&Self::local_music_dir());
            }
            self.music_rows.clear();
        }
        if self.music_refs.queue_clear.clicked(actions) {
            self.decks.clear_queue();
            self.queue_rows.clear();
        }
        for hit in track_list_hits(&self.ui, cx, ids!(music_tracks), actions) {
            let index = match hit {
                TrackListHit::Load(index) => index,
                TrackListHit::Queue(index) => {
                    if let Some(item) = self.track_item_at(index) {
                        let cmds = self.decks.enqueue(item);
                        self.run_deck_cmds(cx, cmds);
                        self.queue_rows.clear();
                    }
                    continue;
                }
            };
            if let Some(item) = self.track_item_at(index) {
                let cmds = self.decks.click(item, self.deck_target);
                self.run_deck_cmds(cx, cmds);
                self.music_rows.clear();
            }
        }
        for hit in track_list_hits(&self.ui, cx, ids!(music_queue), actions) {
            if let TrackListHit::Load(index) = hit {
                let cmds = self.decks.load_queued(index, self.deck_target);
                self.run_deck_cmds(cx, cmds);
                self.queue_rows.clear();
            }
        }
    }

    /// Directory the local-files lane lists. Defaults to the checkout root,
    /// which is where the test renders land.
    fn local_music_dir() -> PathBuf {
        std::env::var("VJ_MUSIC_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
            })
    }

    /// Audio files named on the command line: `makepad-vj track-a.wav
    /// track-b.wav` opens the deck surface with them cued up.
    fn startup_audio_files() -> Vec<PathBuf> {
        std::env::args()
            .skip(1)
            .map(PathBuf::from)
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|e| e.to_ascii_lowercase())
                        .is_some_and(|e| {
                            wave_analysis::LOCAL_AUDIO_EXTENSIONS.contains(&e.as_str())
                        })
            })
            .collect()
    }

    /// A deck-loadable item for a file on this machine. Its identity is
    /// derived from the path, so latest-wins loading and the analysis cache
    /// work exactly as they do for a catalog track.
    fn local_track_item(&mut self, path: &Path) -> Option<TrackItem> {
        let digest = BlobId::hash_of(path.to_string_lossy().as_bytes());
        let mut asset_bytes = [0u8; 16];
        asset_bytes.copy_from_slice(&digest.as_bytes()[..16]);
        let asset = AssetId::from_bytes(asset_bytes);
        self.local_by_asset.insert(asset, path.to_path_buf());
        Some(TrackItem {
            asset,
            revision: AssetRevisionId::from_bytes(*digest.as_bytes()),
            title: path.file_name()?.to_string_lossy().to_string(),
            media_blob: digest,
            media_len: 0,
            media: MediaType::Wav,
            // A file on this machine has no revision on any store: it
            // separates locally, exactly as it always has.
            side: TrackSideChannels::default(),
        })
    }

    /// Turn an explorer row into something a deck can load.
    fn track_item_at(&mut self, index: usize) -> Option<TrackItem> {
        let entry = self.music_rows.get(index)?.clone();
        match entry.key {
            TrackKey::Asset(asset) => {
                let tile = self.music_model.tile(&asset)?;
                let (revision, media) = (tile.revision?, tile.media.clone()?);
                Some(TrackItem {
                    asset,
                    revision,
                    title: tile.title.clone(),
                    media_blob: media.blob,
                    media_len: media.len,
                    media: media.media,
                    side: self
                        .track_side_channels
                        .get(&revision)
                        .cloned()
                        .unwrap_or_default(),
                })
            }
            TrackKey::Local(path) => self.local_track_item(&path),
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.status_text = "starting…".to_string();
        self.midi_status = "APC40: scanning…".to_string();
        self.midi_input = cx.midi_input();
        self.midi_output = cx.midi_output();
        self.start_lighting();
        self.sync_lighting_controls_ui(cx);
        // STANDALONE BY DEFAULT. The VJ hosts its own Asset Server on
        // loopback unless an external one is reachable (or pinned) — see
        // `local_store::resolve`. Either way everything above this line is a
        // thin client over HTTP and cannot tell the difference.
        let resolved = local_store::resolve(service::session_config_from_env());
        self.status_text = resolved.note.clone();
        self.local_store = resolved.local;
        match SessionConnector::start(resolved.config) {
            Ok(connector) => self.connector = Some(connector),
            Err(error) => self.status_text = format!("session config invalid: {error}"),
        }
        self.poll_timer = cx.start_interval(0.05);
        self.refresh_timer = cx.start_interval(1.0);
        self.video_loop = true;
        // Beat-lock has NO off switch: the ear (capture) button is the one
        // audio control — with no captured audio the clock already falls
        // back to TAP/FREE, which covers everything the old "Lock audio"
        // bypass did.
        self.external_sync_enabled = true;
        // `VJ_SURFACE=music` (or a file on the command line) opens straight
        // on the deck surface — the everyday "open the DJ set" start.
        let files = Self::startup_audio_files();
        let want_music = !files.is_empty()
            || std::env::var("VJ_SURFACE").is_ok_and(|value| value.eq_ignore_ascii_case("music"));
        if want_music {
            self.apc.surface = ApcSurface::Music;
            self.ui
                .page_flip(cx, ids!(pages))
                .set_active_page(cx, id!(music_page).into());
            self.console_page = id!(music_page).into();
            self.paint_tabs(cx, id!(music_page));
            // Files on the command line open the local lane; a bare
            // `VJ_SURFACE=music` opens the store, like clicking the tab.
            self.music_local = !files.is_empty();
            if self.music_local {
                self.local_tracks = wave_analysis::list_local_audio(&Self::local_music_dir());
            }
            for (index, path) in files.iter().take(2).enumerate() {
                let target = if index == 0 { DeckTarget::A } else { DeckTarget::B };
                if let Some(item) = self.local_track_item(path) {
                    let cmds = self.decks.click(item, target);
                    self.run_deck_cmds(cx, cmds);
                }
            }
        } else {
            self.paint_tabs(cx, id!(video_page));
        }
        self.set_lower_tab(cx, self.lights_tab);
        // GEN starts put away unless the operator had it open last time
        // (load_gen_panel may reopen it after the session connects).
        self.set_gen_panel_open(cx, self.gen_panel_open);
        self.paint_gen_tab(cx);
        // The borderless window carries its own min/max/close on Windows.
        #[cfg(target_os = "windows")]
        self.ui.view(cx, ids!(win_buttons)).set_visible(cx, true);
        // Effect slots restore from their local splash files — before (and
        // independent of) the store connection; the MIDI map rides the same
        // boot.
        self.load_fx_slots_panel(cx);
        self.load_midi_map();
        self.sync_midi_learn_ui(cx);
        self.sync_slot_controls_ui(cx);
        self.sync_pads();
        self.grids_dirty = true;
        self.sync_gen_profiles(cx);
        self.ui.drop_down(cx, ids!(gen_profile)).set_selected_item(cx, 2);
        if !self.audio_installed {
            self.audio_installed = true;
            let mixer = self.mixer.clone();
            cx.audio_output(0, move |info, output| {
                output.zero();
                mixer.render(info.sample_rate, output);
            });
        }
        // The system-audio capture is NOT installed here: MONITOR AUDIO
        // (top bar) starts it on demand — see `set_monitor_audio`.
        if self.loop_tx.is_none() {
            let (tx, results) = start_loop_worker();
            self.loop_tx = Some(tx);
            self.loop_results = Some(results);
        }
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        cx.use_audio_outputs(&devices.default_output());
        // PRIVACY RULE: capture ONLY the explicit system-audio loopback
        // device. A microphone is never an implicit fallback — with no
        // loopback device the input list stays empty and the UI says so.
        let loopback: Vec<_> = devices
            .descs
            .iter()
            .filter(|desc| desc.device_type.is_loopback())
            .map(|desc| desc.device_id)
            .collect();
        self.loopback_selected = !loopback.is_empty();
        self.loopback_failed = devices
            .descs
            .iter()
            .any(|desc| desc.device_type.is_loopback() && desc.has_failed);
        self.loopback_ids = loopback;
        log!(
            "audio devices: {} loopback device(s), failed={}, monitor={}",
            self.loopback_ids.len(),
            self.loopback_failed,
            self.monitor_audio
        );
        // MONITOR AUDIO gates the actual device open (that open is what
        // fires the OS screen-recording prompt).
        if self.monitor_audio {
            cx.use_audio_inputs(&self.loopback_ids.clone());
        } else {
            cx.use_audio_inputs(&[]);
        }
        self.sync_monitor_audio_ui(cx);
    }


    fn handle_midi_ports(&mut self, cx: &mut Cx, ports: &MidiPortsEvent) {
        // A hot-unplug cannot deliver Note Off. Clear physical-pad ownership
        // before adopting the new port set so hold/loop SFX never stick.
        for pad in 0..PAD_COUNT {
            self.release_apc_sfx_pad(pad);
        }
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut names = Vec::new();
        // The two mk2 surfaces share a palette but not their LED channel
        // meanings (see `ApcModel`): light them in their own dialect.
        let mut model = None;
        for desc in &ports.descs {
            let Some(found) = apc40::apc_model_for_port(&desc.name) else {
                continue;
            };
            model = model.or(Some(found));
            if desc.port_type.is_input() {
                inputs.push(desc.port_id);
                names.push(desc.name.clone());
            }
            if desc.port_type.is_output() {
                outputs.push(desc.port_id);
            }
        }
        cx.use_midi_inputs(&inputs);
        cx.use_midi_outputs(&outputs);
        self.apc_input_ports = inputs;
        self.apc_output_ports = outputs;
        self.apc_leds.set_model(model.unwrap_or_default());
        // The press decoder translates grid notes with the same per-model
        // mapping the LEDs use — one truth for both directions.
        self.apc.model = model.unwrap_or_default();
        self.apc_leds.invalidate();
        self.midi_status = if self.apc_input_ports.is_empty() {
            "APC: not connected".to_string()
        } else {
            format!(
                "{}: {} ({} LED out)",
                match self.apc_leds.model {
                    apc40::ApcModel::Apc40Mk2 => "APC40 mkII",
                    apc40::ApcModel::ApcMiniMk2 => "APC mini mk2",
                },
                names.join(", "),
                self.apc_output_ports.len()
            )
        };
        self.sync_apc_leds();
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // The lane tabs are gone: VJ/DJ/SFX are the modes (see
        // `select_mode`) and the presets filter the VJ explorer.
        for (button, surface) in MODE_BUTTONS {
            if self.ui.button(cx, button).clicked(actions) {
                self.select_mode(cx, surface);
            }
        }
        if self.ui.button(cx, ids!(gen_fold)).clicked(actions) {
            self.set_gen_panel_open(cx, !self.gen_panel_open);
            self.save_gen_panel();
        }
        if let Some((_, align)) = self
            .ui
            .widget(cx, ids!(gen_split))
            .borrow::<Splitter>()
            .and_then(|s| s.changed(actions))
        {
            if let SplitterAlign::FromA(width) = align {
                self.gen_panel_open = width > 24.0;
                if width > 40.0 {
                    self.gen_panel_width = width;
                }
                self.paint_gen_tab(cx);
            }
        }

        // ---- header ----
        if let Some(v) = self.drop_slider_changed(cx, ids!(master_slider), actions) {
            self.mixer.set_master(v as f32);
        }

        let mut lighting_changed = false;
        macro_rules! lighting_slider {
            ($id:ident, $field:ident) => {
                if let Some(value) = self.ui.slider(cx, ids!($id)).slided(actions) {
                    self.lighting_controls.$field = value as f32;
                    lighting_changed = true;
                }
            };
        }
        lighting_slider!(light_master, master);
        lighting_slider!(light_black_floor, black_floor);
        lighting_slider!(light_colorfulness, colorfulness);
        lighting_slider!(light_response, response);
        lighting_slider!(show_movers, movers);
        lighting_slider!(show_rgb, rgb);
        lighting_slider!(show_strobe, strobe);
        lighting_slider!(laser_level, laser_level);
        lighting_slider!(smoke_level, smoke_level);
        lighting_slider!(uv_level, uv_level);

        for (id, group) in [
            (ids!(laser_arm), 0usize),
            (ids!(smoke_arm), 1usize),
            (ids!(uv_arm), 2usize),
        ] {
            if let Some(armed) = self.ui.check_box(cx, id).changed(actions) {
                let armed = armed && !self.lighting_controls.blackout_latched;
                match group {
                    0 => self.lighting_controls.laser_armed = armed,
                    1 => self.lighting_controls.smoke_armed = armed,
                    _ => self.lighting_controls.uv_armed = armed,
                }
                // Changing an arm state always requires a fresh press.
                self.lighting_controls.deadman_held = false;
                lighting_changed = true;
            }
        }

        let deadman = self.ui.button(cx, ids!(hazard_deadman));
        if deadman.pressed(actions) {
            self.lighting_controls.deadman_held = self.lighting_controls.any_hazard_armed()
                && !self.lighting_controls.blackout_latched;
            lighting_changed = true;
        }
        if deadman.clicked(actions) || deadman.released(actions) {
            self.lighting_controls.deadman_held = false;
            lighting_changed = true;
        }
        if self.ui.button(cx, ids!(light_blackout)).clicked(actions) {
            self.latch_lighting_blackout();
            lighting_changed = true;
        }
        if let Some(advanced) = self.ui.check_box(cx, ids!(light_advanced)).changed(actions) {
            self.ui
                .view(cx, ids!(light_advanced_row))
                .set_visible(cx, advanced);
            self.ui
                .view(cx, ids!(light_hazard_row))
                .set_visible(cx, advanced);
        }
        if self.ui.button(cx, ids!(light_reset)).clicked(actions) {
            self.lighting_controls = LightingControls::default();
            for analyzer in &mut self.light_analyzers {
                analyzer.reset();
            }
            if let Some(lighting) = self.lighting.as_ref() {
                lighting.set_power(true);
            }
            lighting_changed = true;
        }
        if self.ui.button(cx, ids!(light_restore)).clicked(actions) {
            // Restore only ordinary video/show output. A blackout always
            // leaves every hazardous group explicitly disarmed.
            self.restore_lighting();
            lighting_changed = true;
        }
        if lighting_changed {
            for analyzer in &mut self.light_analyzers {
                tune_light_analyzer(analyzer, self.lighting_controls);
            }
            self.sync_lighting_controls_ui(cx);
            self.refresh_program_lighting();
        }

        // ---- video filter: ONE box, and it is a server search ----
        // Typing re-queries the catalog after a short idle (the server does
        // the narrowing; the frontend never sifts a full dump). Enter fires
        // at once.
        if let Some(text) = self.ui.text_input(cx, ids!(pad_filter)).changed(actions) {
            self.pending_filter = Some(text);
            cx.stop_timer(self.filter_timer);
            self.filter_timer = cx.start_timeout(FILTER_DEBOUNCE_S);
        }
        if let Some((text, _)) = self.ui.text_input(cx, ids!(pad_filter)).returned(actions) {
            self.pending_filter = None;
            let cmds = self.model(Surface::Video).set_text(text.trim().to_string());
            self.run_cat_cmds(Surface::Video, cmds);
        }

        // ---- search rows ----
        let rows: [(Surface, &[LiveId], &[LiveId], &[LiveId], &[LiveId]); 3] = [
            (
                Surface::Music,
                ids!(music_search),
                ids!(music_category),
                ids!(music_go),
                ids!(music_more),
            ),
            (Surface::Sfx, ids!(sfx_search), ids!(sfx_category), ids!(sfx_go), ids!(sfx_more)),
            (
                Surface::Mesh,
                ids!(mesh_search),
                ids!(mesh_category),
                ids!(mesh_go),
                ids!(mesh_more),
            ),
        ];
        for (surface, search, category, go, more) in rows {
            let mut cmds = Vec::new();
            // As-you-type, like the video filter above: the query fires
            // after a short idle; Enter (and the button) fire at once.
            if let Some(text) = self.ui.text_input(cx, search).changed(actions) {
                self.pending_search = Some((surface, SearchBox::Text, text));
                cx.stop_timer(self.search_timer);
                self.search_timer = cx.start_timeout(FILTER_DEBOUNCE_S);
            }
            if let Some(text) = self.ui.text_input(cx, category).changed(actions) {
                self.pending_search = Some((surface, SearchBox::Category, text));
                cx.stop_timer(self.search_timer);
                self.search_timer = cx.start_timeout(FILTER_DEBOUNCE_S);
            }
            if let Some((text, _)) = self.ui.text_input(cx, search).returned(actions) {
                self.pending_search = None;
                cmds.extend(self.model(surface).set_text(text.trim().to_string()));
            }
            if let Some((text, _)) = self.ui.text_input(cx, category).returned(actions) {
                self.pending_search = None;
                cmds.extend(self.model(surface).set_category(text.trim().to_string()));
            }
            if self.ui.button(cx, go).clicked(actions) {
                let text = self.ui.text_input(cx, search).text();
                let cat = self.ui.text_input(cx, category).text();
                let model = self.model(surface);
                model.text = text.trim().to_string();
                model.category = cat.trim().to_string();
                cmds.extend(model.refresh());
            }
            if self.ui.button(cx, more).clicked(actions) {
                cmds.extend(self.model(surface).load_more());
            }
            if !cmds.is_empty() {
                self.run_cat_cmds(surface, cmds);
            }
        }

        // ---- grids ----
        let (video_down, _) = self.pad_matrix_hits(cx, actions, ids!(video_grid));
        for (asset, modifiers) in video_down {
            self.video_tile_clicked(cx, asset, modifiers.shift);
        }
        self.handle_music_rows(cx, actions);
        let (sfx_down, sfx_up) = self.grid_hits(cx, actions, ids!(sfx_grid));
        for asset in sfx_down {
            self.selected_pad = Some(asset);
            if let Some(pad) = self.pads.pad(&asset) {
                self.ui.slider(cx, ids!(sfx_gain)).set_value(cx, pad.gain as f64);
                self.ui
                    .drop_down(cx, ids!(sfx_choke))
                    .set_selected_item(cx, pad.choke_group as usize);
                self.ui.check_box(cx, ids!(sfx_hold)).set_active(cx, pad.hold, Animate::No);
                self.ui.check_box(cx, ids!(sfx_loop)).set_active(cx, pad.loop_on, Animate::No);
            }
            let cmds = self.pads.press(asset, now_ms());
            self.run_pad_cmds(cmds);
        }
        for asset in sfx_up {
            if self.pads.pad(&asset).is_some_and(|pad| pad.hold) {
                let cmds = self.pads.release(asset);
                self.run_pad_cmds(cmds);
            }
        }
        let (mesh_down, _) = self.grid_hits(cx, actions, ids!(mesh_grid));
        for asset in mesh_down {
            self.mesh_tile_clicked(cx, asset);
        }

        // ---- generate surface ----
        if let Some(index) = self.ui.drop_down(cx, ids!(gen_profile)).selected(actions) {
            self.gen.select_profile(index);
            self.save_gen_panel();
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(gen_len)).selected(actions) {
            self.gen.set_video_length(index);
            self.save_gen_panel();
        }
        if let Some(text) = self.ui.text_input(cx, ids!(gen_prompt)).changed(actions) {
            self.gen.set_prompt(text);
            self.save_gen_panel();
        }
        let submit_prompt = self
            .ui
            .text_input(cx, ids!(gen_prompt))
            .returned(actions)
            .map(|(text, _)| text);
        if let Some(text) = submit_prompt {
            self.gen.set_prompt(text);
            self.gen.enhance_source = self.program_clip_source();
            let cmds = self.gen.generate(now_ms());
            self.run_gen_cmds(cmds);
            self.grids_dirty = true;
        }
        if self.ui.button(cx, ids!(gen_blast)).clicked(actions) {
            let cmds = self.gen.blast(now_ms());
            self.run_gen_cmds(cmds);
            self.grids_dirty = true;
        }
        if self.ui.button(cx, ids!(gen_go)).clicked(actions) {
            let text = self.ui.text_input(cx, ids!(gen_prompt)).text();
            self.gen.set_prompt(text);
            self.gen.enhance_source = self.program_clip_source();
            let cmds = self.gen.generate(now_ms());
            self.run_gen_cmds(cmds);
            self.grids_dirty = true;
        }
        if let Some(on) = self.ui.check_box(cx, ids!(gen_loop)).changed(actions) {
            // Arming reads whatever is in the prompt box right now, so the
            // operator never gets a loop of a stale prompt.
            let text = self.ui.text_input(cx, ids!(gen_prompt)).text();
            self.gen.set_prompt(text);
            let cmds = self.gen.set_continuous(on, now_ms());
            self.run_gen_cmds(cmds);
            self.grids_dirty = true;
            self.save_gen_panel();
        }
        // ---- chat (same flow as the asset UI: shared feed + broker) ----
        if self.ui.button(cx, ids!(chat_send_btn)).clicked(actions) {
            self.send_chat(cx);
        }
        if self.ui.text_input(cx, ids!(chat_input)).returned(actions).is_some() {
            self.send_chat(cx);
        }
        if self.ui.button(cx, ids!(chat_cancel_btn)).clicked(actions) {
            self.chat.cancel();
            self.refresh_chat_ui(cx);
        }
        if self.ui.button(cx, ids!(chat_clear_btn)).clicked(actions) {
            // Wipe the transcript AND retire the session: the next message
            // starts a conversation the model has no memory of.
            self.chat.clear();
            self.refresh_chat_ui(cx);
        }
        // Tool chips in the chat expand/collapse on click.
        self.ui
            .widget(cx, ids!(chat_list))
            .borrow_mut::<makepad_asset_chat_ui::AssetChatList>()
            .map(|mut list| list.handle_actions(cx, actions));
        if self.ui.button(cx, ids!(gen_clear)).clicked(actions) {
            // Clearing the queue also disarms the loop: otherwise the next
            // tick refills what the operator just emptied.
            let cmds = self.gen.set_continuous(false, now_ms());
            self.run_gen_cmds(cmds);
            self.ui.check_box(cx, ids!(gen_loop)).set_active(cx, false, Animate::No);
            let cmds = self.gen.clear_queue();
            self.run_gen_cmds(cmds);
            self.grids_dirty = true;
        }
        // Per-row cancel buttons.
        {
            let widget = self.ui.widget(cx, ids!(gen_jobs));
            let list = widget.portal_list(cx, ids!(list));
            let mut cancel_tags = Vec::new();
            for (row_id, item) in list.items_with_actions(actions) {
                if item.button(cx, ids!(job_cancel)).clicked(actions) {
                    let tag = widget
                        .borrow::<VjJobList>()
                        .and_then(|l| l.entry_at(row_id).map(|e| e.tag));
                    if let Some(tag) = tag {
                        cancel_tags.push(tag);
                    }
                }
            }
            for tag in cancel_tags {
                let cmds = self.gen.cancel(tag);
                self.run_gen_cmds(cmds);
                self.grids_dirty = true;
            }
        }

        // ---- video transport ----
        if self.ui.button(cx, ids!(beat_tap)).clicked(actions) {
            self.tap_beat(cx);
        }
        if self.ui.button(cx, ids!(external_sync_now)).clicked(actions) {
            self.resync_beat(cx);
        }
        if self.ui.button(cx, ids!(karaoke_word_hops)).clicked(actions) {
            // Live switch between the line sweep and the aligned word
            // mapper: re-time both decks' schedules from their caches.
            let on = !crate::lyrics::word_hops_enabled();
            crate::lyrics::set_word_hops(on);
            self.paint_icon_button(cx, ids!(karaoke_word_hops), on);
            self.rebuild_karaoke(cx, DeckId::A);
            self.rebuild_karaoke(cx, DeckId::B);
        }
        if self.ui.button(cx, ids!(karaoke_enable)).clicked(actions) {
            let on = !self.karaoke_on;
            self.paint_icon_button(cx, ids!(karaoke_enable), on);
            self.karaoke_on = on;
            // Turning it off has to clear the words that are already on the
            // program, not merely stop updating them.
            if !on {
                for target in [ids!(program), ids!(preview)] {
                    if let Some(mut program) = self
                        .ui
                        .widget(cx, target)
                        .borrow_mut::<views::VideoProgram>()
                    {
                        program.set_karaoke(cx, None);
                    }
                }
            }
            // Turning it ON has to start the frame pump: with no video
            // playing nothing else would ever ask for another frame.
            self.video_pump = cx.new_next_frame();
        }
        // Per-slot transport (the strip above each cue well).
        for slot in [SlotId::A, SlotId::B] {
            let i = slot.index();
            // The SOURCE mini's own transport: same verbs, deck-local.
            let action = actions
                .find_widget_action(
                    self.ui.widget(cx, Self::deck_source_path(slot)).widget_uid(),
                )
                .map(|a| a.cast())
                .unwrap_or(VideoAction::None);
            match action {
                VideoAction::TogglePlay => {
                    let paused = self.players[i].as_ref().is_some_and(|p| !p.is_paused());
                    self.set_slot_paused(cx, slot, paused);
                }
                VideoAction::Restart => {
                    if self.flow_active(i) {
                        self.flow_view(cx, slot, |cx, view| view.seek_fraction(cx, 0.0));
                    } else if let Some(player) = self.players[i].as_mut() {
                        player.seek_fraction(0.0);
                    }
                    self.video_pump = cx.new_next_frame();
                }
                VideoAction::Seek(fraction) => {
                    // SCRUB: flow-warped clips seek on the warp clock
                    // (cache-indexed, free); plain clips ride the player's
                    // platform-seek tier. Mid-drag seeks are SILENT; the
                    // release seek (scrubbing already false) re-primes
                    // audio on an unmuted clip.
                    let scrubbing = self
                        .ui
                        .widget(cx, Self::deck_source_path(slot))
                        .borrow::<VideoView>()
                        .map(|mini| mini.is_scrubbing())
                        .unwrap_or(false);
                    if self.flow_active(i) {
                        self.flow_view(cx, slot, |cx, view| view.seek_fraction(cx, fraction));
                    } else if let Some(player) = self.players[i].as_mut() {
                        player.set_scrub(scrubbing);
                        player.seek_fraction(fraction);
                    }
                    self.video_pump = cx.new_next_frame();
                }
                VideoAction::ToggleLoop => {
                    let looping = self
                        .ui
                        .widget(cx, Self::deck_source_path(slot))
                        .borrow::<VideoView>()
                        .map(|mini| mini.looping())
                        .unwrap_or(true);
                    self.slot_loop[i] = looping;
                    let mode = self.slot_play_mode(i);
                    if let Some(player) = self.players[i].as_mut() {
                        player.set_mode(mode);
                    }
                    self.sync_slot_controls_ui(cx);
                    self.save_clip_profile(slot);
                    self.video_pump = cx.new_next_frame();
                }
                VideoAction::TrimChanged(t_in, t_out) => {
                    // Handle RELEASE: tighten the bounds and NOTHING else —
                    // the clip keeps playing exactly where it is ("just
                    // make the space it bounces in smaller"). Playback
                    // meets the new bounds at the natural moment: the next
                    // wrap/reflect at OUT, or, if the playhead is already
                    // outside, the wrap the decode tiers take on their own
                    // — never an instant seek from here. The beat re-fit
                    // only adjusts RATE, so it re-anchors nothing.
                    self.slot_trim[i] = (t_in, t_out);
                    if let Some(player) = self.players[i].as_mut() {
                        player.set_trim(t_in, t_out);
                    }
                    self.apply_loop_fit(slot);
                    self.save_clip_profile(slot);
                    self.video_pump = cx.new_next_frame();
                }
                VideoAction::None => {}
            }
            // GRAYED = disabled: an empty deck's transports are inert (the
            // ghost faces already say so).
            if self.slot_media[i] == SlotMedia::Empty {
                continue;
            }
            {
                let path: &[LiveId] = match slot {
                    SlotId::A => ids!(deck_a_scratch),
                    SlotId::B => ids!(deck_b_scratch),
                };
                let uid = self.ui.widget(cx, path).widget_uid();
                let mut scratch = None;
                for action in actions.iter() {
                    if let Some(wa) = action.as_widget_action() {
                        if wa.widget_uid == uid {
                            if let views::VjShuttleAction::Scratch(v) = wa.cast() {
                                scratch = Some(v);
                            }
                        }
                    }
                }
                if let Some(v) = scratch {
                    self.apply_scratch(cx, slot, v);
                }
            }
            if self.ui.button(cx, Self::deck_eject_path(slot)).clicked(actions) {
                self.unslot_deck(cx, slot);
            }
            if self.ui.button(cx, Self::deck_play_path(slot)).clicked(actions) {
                let paused = self.players[i].as_ref().is_some_and(|p| !p.is_paused());
                self.set_slot_paused(cx, slot, paused);
            }
            if self.ui.button(cx, Self::deck_rw_path(slot)).clicked(actions) {
                // Rewind: the IN point (frame 0 untrimmed) — for a
                // beat-synced loop that IS "start your cycle now".
                let t_in = self.slot_trim[i].0;
                if self.flow_active(i) {
                    self.flow_view(cx, slot, |cx, view| view.seek_fraction(cx, 0.0));
                } else if let Some(player) = self.players[i].as_mut() {
                    player.set_scrub(false);
                    player.seek_fraction(t_in);
                }
                self.video_pump = cx.new_next_frame();
            }
            if self.ui.button(cx, Self::deck_loop2_path(slot)).clicked(actions) {
                self.slot_loop[i] = !self.slot_loop[i];
                let mode = self.slot_play_mode(i);
                if let Some(player) = self.players[i].as_mut() {
                    player.set_mode(mode);
                }
                self.paint_icon_button(cx, Self::deck_loop2_path(slot), self.slot_loop[i]);
                self.sync_slot_controls_ui(cx);
                self.save_clip_profile(slot);
                self.video_pump = cx.new_next_frame();
            }
            // THE BEAT CHIP — the number IS the beats one sweep spans:
            // click cycles 8 → 4 → 2 → 1 (beat-synced, lit) → — (free,
            // dim) → 8 … (8 = slowest, a sweep stretched over 8 beats;
            // 1 = a sweep per beat.)
            if self.ui.button(cx, Self::deck_rate_path(slot)).clicked(actions) {
                let (sync, rate) = if !self.slot_beat_sync[i] {
                    (true, 8.0)
                } else {
                    match self.slot_beat_rate[i] {
                        r if r > 6.0 => (true, 4.0),
                        r if r > 3.0 => (true, 2.0),
                        r if r > 1.5 => (true, 1.0),
                        _ => (false, 4.0),
                    }
                };
                self.slot_beat_sync[i] = sync;
                self.slot_beat_rate[i] = rate;
                self.slot_sync_beats[i] = 1;
                self.ui
                    .button(cx, Self::deck_rate_path(slot))
                    .set_text(cx, Self::rate_chip_label(sync, rate));
                self.paint_lit(cx, Self::deck_rate_path(slot), sync);
                self.apply_slot_beat_sync(slot);
                self.apply_loop_fit(slot);
                self.save_clip_profile(slot);
                self.video_pump = cx.new_next_frame();
            }
            // BOUNCE (ping-pong) — the same latch the strip's pp button flips.
            if self.ui.button(cx, Self::deck_bounce_path(slot)).clicked(actions) {
                self.slot_pingpong[i] = !self.slot_pingpong[i];
                let mode = self.slot_play_mode(i);
                if let Some(player) = self.players[i].as_mut() {
                    player.set_mode(mode);
                }
                self.paint_icon_button(cx, Self::deck_bounce_path(slot), self.slot_pingpong[i]);
                self.save_clip_profile(slot);
                self.video_pump = cx.new_next_frame();
            }

            if self.ui.button(cx, Self::deck_mute_path(slot)).clicked(actions) {
                self.slot_video_muted[i] = !self.slot_video_muted[i];
                if let Some(player) = self.players[i].as_mut() {
                    player.set_muted(self.slot_video_muted[i]);
                }
                self.paint_icon_button(cx, Self::deck_mute_path(slot), self.slot_video_muted[i]);
                self.save_clip_profile(slot);
            }
        }
        if self.ui.button(cx, ids!(video_mute)).clicked(actions) {
            let on = !self.video_muted;
            self.video_muted = on;
            self.mixer.set_video_muted(on);
            if on {
                self.disarm_hazards(Some(cx));
            }
            self.refresh_program_lighting();
            self.sync_slot_controls_ui(cx);
        }
        if let Some(v) = self.drop_slider_changed(cx, ids!(fadeout_knob), actions) {
            self.set_fadeout(cx, v as f32);
        }
        if let Some(v) = self.ui.slider(cx, ids!(video_fade)).slided(actions) {
            self.fade_secs = v as f32;
            self.ui
                .label(cx, ids!(fade_value))
                .set_text(cx, &format!("{:.1}s", v));
        }

        // ---- deck transport ----
        if let Some(index) = self.ui.drop_down(cx, ids!(deck_target)).selected(actions) {
            self.deck_target = match index {
                1 => DeckTarget::A,
                2 => DeckTarget::B,
                _ => DeckTarget::Auto,
            };
        }
        self.handle_deck_controls(cx, actions);
        if let Some(v) = self.ui.slider(cx, ids!(xfader)).slided(actions) {
            let cmds = self.decks.set_crossfader(v as f32);
            self.run_deck_cmds(cx, cmds);
        }
        if let Some(v) = self.ui.slider(cx, ids!(apc_xfader)).slided(actions) {
            // The hand always wins.
            self.auto_fade.cancel();
            self.sync_autofade_ui(cx);
            self.set_visual_mix(cx, v as f32);
        }
        {
            let field = self.ui.widget(cx, ids!(bpm_field));
            let mut changed = None;
            for action in actions.iter() {
                if let Some(wa) = action.as_widget_action() {
                    if wa.widget_uid == field.widget_uid() {
                        if let ValueInputAction::Changed(v) = wa.cast() {
                            changed = Some(v);
                        }
                    }
                }
            }
            if let Some(v) = changed {
                self.set_manual_bpm(cx, v);
            }
        }
        if self.ui.button(cx, ids!(deck_swap)).clicked(actions) {
            self.swap_decks(cx);
        }
        // AUTOWIPE — the MX50 AUTO take: the machine sweeps the fader to
        // the far side over the fade time, driving the loaded transition
        // exactly as a hand-drag would. Mid-travel presses retarget; a
        // hand on the fader always cancels and takes over.
        if self.ui.button(cx, ids!(autowipe)).clicked(actions) {
            let secs = self.ui.slider(cx, ids!(video_fade)).value().unwrap_or(1.0) as f32;
            self.auto_fade.press(self.program_mix, secs);
            self.sync_autofade_ui(cx);
            self.video_pump = cx.new_next_frame();
        }
        if self.ui.button(cx, ids!(monitor_audio_btn)).clicked(actions) {
            let on = !self.monitor_audio;
            self.set_monitor_audio(cx, on);
        }
        if self.ui.button(cx, ids!(autofade)).clicked(actions) {
            // LATCHING toggle: clicks (effects now, content always did)
            // sweep the program to the side they land on while this is on.
            self.fx_slots.click_autofade = !self.fx_slots.click_autofade;
            self.save_fx_slots();
            self.sync_autofade_ui(cx);
            self.video_pump = cx.new_next_frame();
        }
        for slot in [SlotId::A, SlotId::B] {
            if self.ui.button(cx, Self::slot_spin_path(slot)).clicked(actions) {
                let on = !self.slot_spin[slot.index()];
                self.set_slot_spin(cx, slot, on);
            }
            if let Some(index) = self.ui.drop_down(cx, Self::slot_anim_path(slot)).selected(actions) {
                self.set_slot_anim(cx, slot, index);
            }
        }
        // Grid strip paging: a column at a time or a whole page of columns.
        {
            let step = if self.ui.button(cx, ids!(grid_prev_row)).clicked(actions) {
                Some(-1i32)
            } else if self.ui.button(cx, ids!(grid_next_row)).clicked(actions) {
                Some(1)
            } else if self.ui.button(cx, ids!(grid_prev_page)).clicked(actions) {
                Some(-(views::PAD_COLS as i32))
            } else if self.ui.button(cx, ids!(grid_next_page)).clicked(actions) {
                Some(views::PAD_COLS as i32)
            } else {
                None
            };
            if let Some(cols) = step {
                let widget = self.ui.widget(cx, ids!(video_grid));
                if let Some(mut pads) = widget.borrow_mut::<VjPadMatrix>() {
                    pads.nudge_cols(cx, cols);
                }
                self.sync_video_pad_window(cx);
            }
        }
        // Hot presets in the bar: they SET what the explorer shows.
        for (chip, lane, _) in LANE_CHIPS {
            if self.ui.button(cx, chip).clicked(actions) {
                self.lane_chip_clicked(cx, lane);
            }
        }
        // ---- MIDI learn ----
        if self.ui.button(cx, ids!(midi_learn_btn)).clicked(actions) {
            self.midi_learn.toggle_pick();
            self.sync_midi_learn_ui(cx);
        }
        for (path, control) in LEARNABLES {
            let wrap = self.ui.widget(cx, path);
            if let Some(item) = actions.find_widget_action(wrap.widget_uid()) {
                if let LearnWrapAction::Clicked { alt } = item.cast() {
                    if self.midi_learn.control_clicked(control, alt) {
                        // The clear gesture may have dropped a binding.
                        self.save_midi_map();
                        self.sync_midi_learn_ui(cx);
                    }
                }
            }
        }

        // ---- lower-region tabs ----
        if self.ui.button(cx, ids!(lower_tab_vj)).clicked(actions) {
            self.set_lower_tab(cx, false);
            self.save_gen_panel();
        }
        if self.ui.button(cx, ids!(lower_tab_lights)).clicked(actions) {
            self.set_lower_tab(cx, true);
            self.save_gen_panel();
        }

        // ---- effect slots (fx_slot.rs) ----
        for kind in FxSlotKind::ALL {
            let tile = self.ui.widget(cx, Self::fx_slot_tile_path(kind));
            if let Some(item) = actions.find_widget_action(tile.widget_uid()) {
                if let FxSlotTileAction::Pressed = item.cast() {
                    // Click a slot to arm it (the next FX-tile click loads
                    // here); click it again to disarm. Arming also points
                    // the explorer at the matching lane: the TRANSITION
                    // slot prefers the transition-tagged effects.
                    if self.fx_slots.toggle_arm(kind) {
                        let lane = match kind {
                            FxSlotKind::Transition => GridLane::Transition,
                            _ => GridLane::Kind(AssetKind::VjEffect),
                        };
                        self.set_lane(cx, lane);
                    }
                    self.sync_fx_slots_ui(cx);
                }
            }
            if self.ui.button(cx, Self::fx_slot_on_path(kind)).clicked(actions)
                && self.fx_slots.slot(kind).title.is_some()
            {
                let bypass = !self.fx_slots.slot(kind).bypass;
                self.fx_slots.slot_mut(kind).bypass = bypass;
                self.save_fx_slots();
                self.save_fx_profile(kind);
                self.sync_fx_slots_ui(cx);
                self.video_pump = cx.new_next_frame();
            }
            if self.ui.button(cx, Self::fx_slot_clear_path(kind)).clicked(actions) {
                self.clear_fx_slot(cx, kind);
            }
            if let Some(v) = self
                .ui
                .slider(cx, Self::fx_slot_knob_path(kind, 0))
                .slided(actions)
            {
                self.fx_slots.slot_mut(kind).speed = (v as f32).clamp(0.0, 1.0);
                self.save_fx_slots();
                self.video_pump = cx.new_next_frame();
            }
            for p in 0..3 {
                if let Some(v) = self
                    .ui
                    .slider(cx, Self::fx_slot_knob_path(kind, p + 1))
                    .slided(actions)
                {
                    self.fx_slots.slot_mut(kind).p[p] = Some((v as f32).clamp(0.0, 1.0));
                    self.save_fx_slots();
                    self.video_pump = cx.new_next_frame();
                }
            }
            // Sticky per-EFFECT dials: written at knob RELEASE (settle),
            // not per drag frame.
            for knob in 0..4 {
                if self
                    .ui
                    .slider(cx, Self::fx_slot_knob_path(kind, knob))
                    .end_slide(actions)
                    .is_some()
                {
                    self.save_fx_profile(kind);
                }
            }
        }
        let fade_ids = [
            ids!(light_fader_0),
            ids!(light_fader_1),
            ids!(light_fader_2),
            ids!(light_fader_3),
            ids!(light_fader_4),
            ids!(light_fader_5),
            ids!(light_fader_6),
            ids!(light_fader_7),
            ids!(light_fader_8),
        ];
        for (index, id) in fade_ids.iter().enumerate() {
            if let Some(v) = self.ui.slider(cx, *id).slided(actions) {
                if let Some(desk) = self.lighting.as_ref() {
                    desk.set_fader(index, v as f32);
                }
            }
        }
        let top_ids = [
            ids!(light_knob_0),
            ids!(light_knob_1),
            ids!(light_knob_2),
            ids!(light_knob_3),
            ids!(light_knob_4),
            ids!(light_knob_5),
            ids!(light_knob_6),
            ids!(light_knob_7),
        ];
        for (index, id) in top_ids.iter().enumerate() {
            if let Some(v) = self.ui.slider(cx, *id).slided(actions) {
                if let Some(desk) = self.lighting.as_ref() {
                    desk.set_top_knob(index, v as f32);
                }
            }
        }
        let dev_ids = [
            ids!(light_dev_0),
            ids!(light_dev_1),
            ids!(light_dev_2),
            ids!(light_dev_3),
            ids!(light_dev_4),
            ids!(light_dev_5),
            ids!(light_dev_6),
            ids!(light_dev_7),
        ];
        for (index, id) in dev_ids.iter().enumerate() {
            if let Some(v) = self.ui.slider(cx, *id).slided(actions) {
                if let Some(desk) = self.lighting.as_ref() {
                    desk.set_device_knob(self.light_track, index, v as f32);
                }
            }
        }
        let scene_ids = [
            ids!(light_scene_0),
            ids!(light_scene_1),
            ids!(light_scene_2),
            ids!(light_scene_3),
            ids!(light_scene_4),
            ids!(light_scene_5),
            ids!(light_scene_6),
            ids!(light_scene_7),
            ids!(light_scene_8),
            ids!(light_scene_9),
            ids!(light_scene_10),
            ids!(light_scene_11),
            ids!(light_scene_12),
        ];
        for (index, id) in scene_ids.iter().enumerate() {
            if self.ui.button(cx, *id).clicked(actions) {
                if let Some(desk) = self.lighting.as_ref() {
                    let _ = desk.trigger_scene(index);
                }
            }
        }
        let track_ids = [
            ids!(light_track_0),
            ids!(light_track_1),
            ids!(light_track_2),
            ids!(light_track_3),
            ids!(light_track_4),
            ids!(light_track_5),
            ids!(light_track_6),
            ids!(light_track_7),
        ];
        for (index, id) in track_ids.iter().enumerate() {
            if self.ui.button(cx, *id).clicked(actions) {
                self.light_track = index;
                self.refresh_room_desk_ui(cx);
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(light_power)).changed(actions) {
            if let Some(desk) = self.lighting.as_ref() {
                desk.set_power(on);
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(light_write)).changed(actions) {
            if let Some(desk) = self.lighting.as_ref() {
                desk.set_write(on);
            }
        }
        if let Some(v) = self.ui.slider(cx, ids!(xfade_secs)).slided(actions) {
            self.xfade_secs = v as f32;
        }
        if self.ui.button(cx, ids!(fade_to_a)).clicked(actions) {
            let secs = self.xfade_secs;
            let cmds = self.decks.fade_to(DeckId::A, secs);
            self.run_deck_cmds(cx, cmds);
            self.ui.slider(cx, ids!(xfader)).set_value(cx, 0.0);
        }
        if self.ui.button(cx, ids!(fade_to_b)).clicked(actions) {
            let secs = self.xfade_secs;
            let cmds = self.decks.fade_to(DeckId::B, secs);
            self.run_deck_cmds(cx, cmds);
            self.ui.slider(cx, ids!(xfader)).set_value(cx, 1.0);
        }
        if let Some(index) = self.ui.drop_down(cx, ids!(xcurve)).selected(actions) {
            let curve = if index == 1 { FadeCurve::Linear } else { FadeCurve::EqualPower };
            let cmds = self.decks.set_curve(curve);
            self.run_deck_cmds(cx, cmds);
        }
        if self.ui.button(cx, ids!(decks_swap)).clicked(actions) {
            let cmds = self.decks.swap();
            self.run_deck_cmds(cx, cmds);
            self.sync_deck_controls(cx);
        }

        // ---- sfx settings strip ----
        if let Some(pad) = self.selected_pad {
            if let Some(v) = self.ui.slider(cx, ids!(sfx_gain)).slided(actions) {
                let cmds = self.pads.set_gain(pad, v as f32);
                self.run_pad_cmds(cmds);
            }
            if let Some(index) = self.ui.drop_down(cx, ids!(sfx_choke)).selected(actions) {
                self.pads.set_choke_group(pad, index as u8);
            }
            if let Some(on) = self.ui.check_box(cx, ids!(sfx_hold)).changed(actions) {
                self.pads.set_hold(pad, on);
            }
            if let Some(on) = self.ui.check_box(cx, ids!(sfx_loop)).changed(actions) {
                self.pads.set_loop_on(pad, on);
            }
            if self.ui.button(cx, ids!(sfx_stop)).clicked(actions) {
                let cmds = self.pads.stop_pad(pad);
                self.run_pad_cmds(cmds);
            }
        }
        if self.ui.button(cx, ids!(sfx_stop_all)).clicked(actions) {
            for key in self.pads.pad_keys() {
                let cmds = self.pads.stop_pad(key);
                self.run_pad_cmds(cmds);
            }
        }

        // ---- IMPORT: one quiet button ----
        // Idle: click opens the native picker (folder — a whole shoot
        // imports in one go; single files ride the same reference-import
        // walk). Busy: the button IS the progress readout and clicking it
        // CANCELS, keeping everything already landed.
        if self.ui.button(cx, ids!(import_toggle)).clicked(actions) {
            if self.import.busy() {
                self.import.cancel();
                self.sync_import_ui(cx);
            } else {
                self.open_import_picker(cx);
            }
        }
        // The native picker answers in a LATER actions pass; the pick
        // starts the import on the spot — no panel, no second click.
        for action in actions {
            let Some(picked) = action.downcast_ref::<FileDialogAction>() else { continue };
            match picked {
                FileDialogAction::FolderSelected(path) => {
                    self.import_picking = false;
                    self.import.set_path(path.to_string_lossy().into_owned());
                    self.start_import(cx);
                }
                FileDialogAction::FolderCancelled => {
                    self.import_picking = false;
                }
                FileDialogAction::None => {}
            }
        }

        // ---- Windows chrome cluster ----
        if self.ui.desktop_button(cx, ids!(win_min)).clicked(actions) {
            if let Some(window_id) = self.ui.window(cx, ids!(main_window)).window_id() {
                cx.push_unique_platform_op(
                    makepad_widgets::makepad_platform::CxOsOp::MinimizeWindow(window_id),
                );
            }
        }
        if self.ui.desktop_button(cx, ids!(win_max)).clicked(actions) {
            if let Some(window_id) = self.ui.window(cx, ids!(main_window)).window_id() {
                let op = if cx.windows[window_id].window_geom.is_fullscreen {
                    makepad_widgets::makepad_platform::CxOsOp::RestoreWindow(window_id)
                } else {
                    makepad_widgets::makepad_platform::CxOsOp::MaximizeWindow(window_id)
                };
                cx.push_unique_platform_op(op);
            }
        }
        if self.ui.desktop_button(cx, ids!(win_close)).clicked(actions) {
            if let Some(window_id) = self.ui.window(cx, ids!(main_window)).window_id() {
                cx.push_unique_platform_op(
                    makepad_widgets::makepad_platform::CxOsOp::CloseWindow(window_id),
                );
            }
        }

        // ---- output window ----
        if self.ui.button(cx, ids!(open_output)).clicked(actions) {
            // A toggle, not a one-way door: the second press puts the
            // output window away again.
            match self.output_window_lifecycle {
                OutputWindowLifecycle::Open => self.close_output_window(cx),
                _ => self.open_output_window(cx),
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_render::script_mod(vm);
        makepad_xr::script_mod(vm);
        makepad_asset_widgets::script_mod(vm);
        makepad_asset_chat_ui::script_mod(vm);
        crate::views::script_mod(vm);
        crate::mesh_view::script_mod(vm);
        crate::flow_warp::script_mod(vm);
        crate::music_view::script_mod(vm);
        crate::effects::script_mod(vm);
        crate::fx_thumbs::script_mod(vm);
        crate::fx_slot::script_mod(vm);
        crate::midi_learn::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.handle_output_window_event(cx, event);
        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::Escape && self.midi_learn.active() {
                self.midi_learn.escape();
                self.sync_midi_learn_ui(cx);
            }
            if ke.key_code == KeyCode::F3 {
                let graph = self.ui.view(cx, ids!(perf_box));
                let on = !graph.visible();
                graph.set_visible(cx, on);
                cx.perf_monitor.set_enabled(on);
                self.ui.redraw(cx);
            }
        }
        // Caption-less main window: the visible dot-grid GRIPPER at the
        // bar's far left is the one drag handle. The old answer was "bar
        // background minus an enumerated control list", and every control
        // added since silently turned its clicks into window drags (the
        // list went stale on macOS) — a fixed glyph can't rot, and the
        // user never hunts for where the window grabs.
        if let Event::WindowDragQuery(dq) = event {
            let main_id = self.ui.window(cx, ids!(main_window)).window_id();
            if Some(dq.window_id) == main_id {
                // The LOGO is the one Caption spot; EVERY other point of
                // the top strip answers Client EXPLICITLY — macOS treats
                // any UNANSWERED titlebar-strip point as a native drag
                // region (the BPM-field-drags-the-window bug, then the
                // fadeout dial: its top pixels poked above the bar's rect
                // into the root padding). Answering the full strip height,
                // edge to edge, is the systematic fix — no per-widget
                // allowlist to rot.
                let grip = self.ui.view(cx, ids!(win_grip)).area();
                if grip.is_valid(cx) && grip.rect(cx).contains(dq.abs) {
                    dq.response.set(WindowDragQueryResponse::Caption);
                } else {
                    let bar = self.ui.view(cx, ids!(status_bar)).area();
                    let strip_bottom = if bar.is_valid(cx) {
                        let rect = bar.rect(cx);
                        rect.pos.y + rect.size.y + 2.0
                    } else {
                        34.0
                    };
                    if dq.abs.y <= strip_bottom {
                        dq.response.set(WindowDragQueryResponse::Client);
                    }
                }
            }
        }
        // Drag inside a cue well orbits that slot's model / splat camera.
        match event {
            Event::MouseDown(me) => {
                for (slot, well) in [(SlotId::A, ids!(preview_a)), (SlotId::B, ids!(preview_b))] {
                    let area = self.ui.widget(cx, well).area();
                    if area.is_valid(cx) && area.rect(cx).contains(me.abs) {
                        self.well_drag = Some((slot, me.abs));
                    }
                }
            }
            Event::MouseMove(me) => {
                if let Some((slot, last)) = self.well_drag {
                    let delta = me.abs - last;
                    self.well_drag = Some((slot, me.abs));
                    self.orbit_slot_by(cx, slot, delta.x as f32, delta.y as f32);
                }
            }
            Event::MouseUp(_) => {
                self.well_drag = None;
            }
            Event::Scroll(se) => {
                for (slot, well) in [(SlotId::A, ids!(preview_a)), (SlotId::B, ids!(preview_b))] {
                    let area = self.ui.widget(cx, well).area();
                    if area.is_valid(cx) && area.rect(cx).contains(se.abs) {
                        let axis = if se.scroll.y.abs() > f64::EPSILON { se.scroll.y } else { se.scroll.x };
                        self.zoom_slot_by(cx, slot, axis);
                    }
                }
            }
            _ => {}
        }
        match event {
            Event::WindowLostFocus(_) => {
                if self.lighting_controls.any_hazard_armed()
                    || self.lighting_controls.deadman_held
                {
                    self.disarm_hazards(Some(cx));
                }
            }
            Event::Pause | Event::Background | Event::QuitRequested(_) | Event::Shutdown => {
                self.latch_lighting_blackout();
                self.sync_lighting_controls_ui(cx);
                self.publish_program_lighting(self.program_mix);
            }
            _ => {}
        }
        if let Event::Startup = event {
            if !self.started {
                self.started = true;
            }
        }
        if self.poll_timer.is_event(event).is_some() {
            self.pump(cx);
        }
        if self.filter_timer.is_event(event).is_some() {
            if let Some(text) = self.pending_filter.take() {
                let cmds = self.model(Surface::Video).set_text(text.trim().to_string());
                self.run_cat_cmds(Surface::Video, cmds);
            }
        }
        if self.search_timer.is_event(event).is_some() {
            if let Some((surface, field, text)) = self.pending_search.take() {
                let text = text.trim().to_string();
                let cmds = match field {
                    SearchBox::Text => self.model(surface).set_text(text),
                    SearchBox::Category => self.model(surface).set_category(text),
                };
                self.run_cat_cmds(surface, cmds);
            }
        }
        if self.refresh_timer.is_event(event).is_some() {
            for slot in [SlotId::A, SlotId::B] {
                if self.slot_beat_sync[slot.index()] {
                    self.apply_slot_beat_sync(slot);
                }
            }
            for surface in SURFACES {
                // Event-driven refreshes are rate-limited: an import streams
                // hundreds of publish events, and re-listing every second
                // would keep restarting resolves (resolved tiles are carried
                // across a refresh, but the listing round trip still costs).
                let since = self.last_event_refresh[surface as usize]
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(f64::MAX);
                let model = self.model(surface);
                if model.refresh_wanted && !model.is_loading() && since >= EVENT_REFRESH_COOLDOWN_S {
                    // A publish, not a query change: whatever is new lands
                    // in the PENDING head column and nothing on screen
                    // moves (see `BrowseModel::refresh_event`).
                    let cmds = model.refresh_event();
                    self.last_event_refresh[surface as usize] = Some(Instant::now());
                    self.run_cat_cmds(surface, cmds);
                }
            }
            // Bounded generation-status polling.
            let cmds = self.gen.tick(now_ms());
            self.run_gen_cmds(cmds);
            let cmds = self.gen.ensure_profiles();
            self.run_gen_cmds(cmds);
            self.grids_dirty = true;
        }
        if self.decode_pump.is_event(event).is_some() {
            self.pump_decodes(cx);
        }
        if self.video_pump.is_event(event).is_some() {
            self.pump_video(cx);
        }
        if self.music_pump.is_event(event).is_some() {
            self.pump_music_frame(cx);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        self.sync_video_pad_window(cx);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.lighting_controls.latch_blackout();
        if let Some(lighting) = self.lighting.take() {
            lighting.set_power(false);
            drop(lighting);
        }
    }
}

#[cfg(test)]
mod autofade_tests {
    use super::*;

    /// Run the fade to its end, returning how long it took.
    fn run(fade: &mut AutoFade, mut mix: f32) -> (f32, f32) {
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        for _ in 0..6000 {
            match fade.tick(dt, mix) {
                Some(next) => {
                    mix = next;
                    t += dt;
                }
                None => break,
            }
        }
        (mix, t)
    }

    #[test]
    fn a_press_crosses_to_the_other_side_at_the_set_fade_time() {
        let mut fade = AutoFade::default();
        assert!(!fade.active(), "idle until pressed");
        assert_eq!(fade.tick(0.016, 0.0), None, "an idle autofade drives nothing");
        assert_eq!(fade.press(0.0, 2.0), 1.0, "from A it heads for B");
        assert!(fade.active());
        let (mix, secs) = run(&mut fade, 0.0);
        assert!((mix - 1.0).abs() < 1e-3, "landed on B: {mix}");
        assert!((secs - 2.0).abs() < 0.1, "a 2 s fade took {secs} s");
        assert!(!fade.active(), "and it stops when it arrives");
    }

    #[test]
    fn the_next_press_goes_back_the_other_way() {
        let mut fade = AutoFade::default();
        fade.press(0.0, 1.0);
        let (mix, _) = run(&mut fade, 0.0);
        assert!((mix - 1.0).abs() < 1e-3);
        // Sitting on B, the next press returns to A.
        assert_eq!(fade.press(mix, 1.0), 0.0);
        let (mix, _) = run(&mut fade, mix);
        assert!(mix.abs() < 1e-3, "back on A: {mix}");
        assert_eq!(fade.press(mix, 1.0), 1.0, "and away again");
    }

    #[test]
    fn a_press_mid_fade_turns_it_round() {
        let mut fade = AutoFade::default();
        fade.press(0.0, 2.0);
        let mut mix = 0.0;
        for _ in 0..30 {
            mix = fade.tick(1.0 / 60.0, mix).expect("running");
        }
        assert!(mix > 0.2 && mix < 0.4, "half a second in: {mix}");
        assert_eq!(fade.press(mix, 2.0), 0.0, "pressing again heads back");
        let (mix, _) = run(&mut fade, mix);
        assert!(mix.abs() < 1e-3, "returned to A: {mix}");
    }

    #[test]
    fn grabbing_the_fader_cancels_it_on_the_spot() {
        let mut fade = AutoFade::default();
        fade.press(0.0, 3.0);
        let mut mix = 0.0;
        for _ in 0..30 {
            mix = fade.tick(1.0 / 60.0, mix).expect("running");
        }
        fade.cancel();
        assert!(!fade.active(), "the hand always wins");
        assert_eq!(fade.tick(1.0 / 60.0, mix), None, "and it drives nothing after");
        // A cancelled fade starts fresh from wherever the operator left it.
        assert_eq!(fade.press(mix, 1.0), 1.0, "still short of the middle, so on to B");
    }

    #[test]
    fn a_part_way_start_keeps_the_same_speed() {
        // The knob is how long a FULL crossfade takes, so starting near the
        // end must not crawl — it simply finishes sooner.
        let mut fade = AutoFade::default();
        // Past the middle, the far end is the one it came from.
        assert_eq!(fade.press(0.75, 2.0), 0.0);
        let (mix, secs) = run(&mut fade, 0.75);
        assert!(mix.abs() < 1e-3, "landed on A: {mix}");
        assert!((secs - 1.5).abs() < 0.1, "three quarters of the way took {secs} s");
    }

    /// The recovery half of the wedge pinned by
    /// `cue::tests::a_started_fade_that_is_never_landed_wedges_every_later_click`:
    /// the mixer publishes transition phases into ONE mailbox, so a
    /// `Completed` the UI thread did not poll before the next arm is gone
    /// for good, and a fade the mixer refused is never published at all.
    /// The host has to notice that the device clock has moved on and land
    /// the engine's fade itself, or every later click parks forever.
    #[test]
    fn a_fade_the_mixer_no_longer_owns_is_landed_by_the_host() {
        // Nothing running: nothing to land.
        assert_eq!(stale_fade_to_land(None, 0), None);
        assert_eq!(stale_fade_to_land(None, 7), None);
        // The engine's fade IS the published one: the device clock owns it
        // and the host must keep its hands off.
        assert_eq!(stale_fade_to_land(Some(7), 7), None);
        // The mixer has moved on to a newer transition (or never took this
        // one, so the mailbox still names an older one): schedule 7 can no
        // longer complete, so the host lands it.
        assert_eq!(stale_fade_to_land(Some(7), 8), Some(7));
        assert_eq!(stale_fade_to_land(Some(7), 6), Some(7));
    }
}

#[cfg(test)]
mod latch_tests {
    use super::*;

    /// A latching toggle (ROTATE, PLAY, LOOP, MUTE, the FX bank, the chips)
    /// must read LIT in every interaction state. The regression this pins:
    /// only `color` was painted, so a lit toggle under the operator's
    /// pointer fell back to the theme's UNLIT hover colour and looked off.
    #[test]
    fn a_lit_latch_stays_lit_under_the_pointer_and_under_the_press() {
        for lit in [LatchPaint::icon(true), LatchPaint::chip(true)] {
            assert!(lit.reads_lit(), "{lit:?} falls back to an unlit colour");
            assert_ne!(lit.bg, lit.bg_hover, "hover must still read as a hover");
            assert_ne!(lit.bg, lit.bg_down, "down must still read as a press");
            // The accent's foreground is dark on all three: an accent fill
            // with the resting light-grey glyph is unreadable.
            assert_eq!(lit.fg, lit.fg_hover);
        }
        for unlit in [LatchPaint::icon(false), LatchPaint::chip(false)] {
            assert!(!unlit.reads_lit());
            assert_ne!(unlit.bg, unlit.bg_hover, "an unlit toggle still hovers");
        }
        // The two families share one accent so LIVE reads the same
        // everywhere on the console.
        assert_eq!(LatchPaint::icon(true), LatchPaint::chip(true));
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;

    /// The regression that made separation LOOK dead: the karaoke bake
    /// answers from its own cache within a second of loading a track that was
    /// transcribed before, and it used to take the deck's only status line
    /// with it — so "stems: separating…" was never seen again, and neither
    /// was "stems: model not installed" or any device error. Whoever is still
    /// working, or has failed, keeps the line.
    #[test]
    fn a_working_or_broken_separation_keeps_the_deck_line() {
        let line = App::deck_stem_line;
        // Working, with a cached transcript already sitting there: the
        // separator's progress is what the operator needs to see.
        assert_eq!(
            line(Some(true), "stems: separating…", "lyrics: 32 lines in 17s", false),
            "stems: separating…"
        );
        // Failed is the case that matters most — it is the only report there
        // is that this track will not be split.
        assert_eq!(
            line(Some(false), "stems: model not installed", "lyrics: 32 lines in 17s", false),
            "stems: model not installed"
        );
        // Settled: the bake is the newer news and takes over.
        assert_eq!(
            line(None, "stems: live", "lyrics: transcribing…", true),
            "lyrics: transcribing…"
        );
        // Settled with nothing from the bake yet.
        assert_eq!(line(None, "stems: live", "", true), "stems: live");
        assert_eq!(line(None, "", "", true), "stems: live");
        assert_eq!(line(None, "", "", false), "stems: full mix");
    }

    #[test]
    fn session_loss_is_only_connection_class() {
        use std::io::ErrorKind as K;
        assert!(is_session_loss(&ClientError::Io { op: "x", kind: K::ConnectionRefused }));
        assert!(is_session_loss(&ClientError::Io { op: "x", kind: K::TimedOut }));
        assert!(is_session_loss(&ClientError::Timeout { op: "x" }));
        assert!(is_session_loss(&ClientError::ServerIdentityMismatch {
            expected: [1; 16],
            found: [2; 16],
        }));
        // The server ANSWERED these: never a reason to re-discover.
        assert!(!is_session_loss(&ClientError::NotFound { what: "asset" }));
        assert!(!is_session_loss(&ClientError::Server { status: 500, detail: None }));
        assert!(!is_session_loss(&ClientError::Io { op: "x", kind: K::PermissionDenied }));
    }
    use makepad_widgets::makepad_platform::audio::AudioBuffer;

    #[test]
    fn paired_cue_opens_only_after_both_files_land() {
        // Sheet first, manifest second.
        let mut pair = CuePair::begin(7);
        assert_eq!(pair.sheet_landed(7, "/m/sheet".into()), None, "half a cue");
        assert_eq!(pair.manifest_for(7), None);
        assert_eq!(
            pair.manifest_landed(7, "/m/manifest".into()),
            Some(PathBuf::from("/m/sheet")),
            "the engine is handed the PRIMARY path once the pair is complete"
        );
        assert_eq!(pair.manifest_for(7), Some(PathBuf::from("/m/manifest")));
        // The opposite arrival order completes the same way.
        let mut pair = CuePair::begin(8);
        assert_eq!(pair.manifest_landed(8, "/m/manifest".into()), None);
        assert_eq!(
            pair.sheet_landed(8, "/m/sheet".into()),
            Some(PathBuf::from("/m/sheet"))
        );
        // Latest-click-wins: a straggling old transfer never completes a
        // newer cue, and never leaks its manifest into one.
        let mut pair = CuePair::begin(9);
        assert_eq!(pair.sheet_landed(8, "/m/old-sheet".into()), None);
        assert_eq!(pair.manifest_landed(8, "/m/old-manifest".into()), None);
        assert_eq!(pair.manifest_for(8), None);
        assert_eq!(pair.manifest_for(9), None);
        assert_eq!(pair.sheet_landed(9, "/m/sheet".into()), None);
        assert_eq!(
            pair.manifest_landed(9, "/m/manifest".into()),
            Some(PathBuf::from("/m/sheet"))
        );
    }

    /// What a manifest is read for on the music surface: the fetch-or-compute
    /// switch. Reading it wrong in either direction is expensive — a missed
    /// set costs a GPU separation that did not need to happen, and a
    /// half-read one would put three stems on four knobs.
    #[test]
    fn a_manifest_offers_its_stems_only_as_a_complete_set() {
        use makepad_asset_data::AssetFile;
        let file = |role: FileRole, seed: u8, media: MediaType| AssetFile {
            role,
            tier: DeviceTier::Any,
            lod: 0,
            media,
            blob: BlobId::from_bytes([seed; 32]),
            byte_len: 1000 + seed as u64,
            dims: None,
        };
        let audio = file(FileRole::Audio, 1, MediaType::Mp3);

        // Nothing but the audio: the deck separates, as it always has.
        let bare = side_channel_refs_of(&[audio]);
        assert_eq!(bare, TrackSideChannels::default());

        // Three stems out of four is not a stem mix.
        let partial: Vec<AssetFile> = FileRole::STEMS[..3]
            .iter()
            .enumerate()
            .map(|(i, role)| file(*role, 10 + i as u8, MediaType::Ogg))
            .collect();
        assert!(side_channel_refs_of(&partial).stems.is_none());

        // The whole set, in the contract's order — and the refs come back in
        // that same order, which is what the lane mapping indexes.
        let mut files: Vec<AssetFile> = FileRole::STEMS
            .iter()
            .enumerate()
            .map(|(i, role)| file(*role, 20 + i as u8, MediaType::Ogg))
            .collect();
        files.push(file(FileRole::Lyrics, 40, MediaType::Json));
        files.push(audio);
        let refs = side_channel_refs_of(&files);
        let stems = refs.stems.expect("a complete set");
        for (slot, (blob, len)) in stems.iter().enumerate() {
            assert_eq!(*blob, BlobId::from_bytes([20 + slot as u8; 32]));
            assert_eq!(*len, 1000 + 20 + slot as u64);
        }
        assert_eq!(refs.lyrics, Some((BlobId::from_bytes([40; 32]), 1040)));

        // Lyrics alone are worth having: the words show, the knobs do not.
        let words = side_channel_refs_of(&[file(FileRole::Lyrics, 40, MediaType::Json)]);
        assert!(words.stems.is_none());
        assert!(words.lyrics.is_some());
    }

    /// A deck starts its decode only when every file it asked for is on
    /// disk — and a lyrics fetch that failed must not hold the stems back.
    #[test]
    fn a_side_channel_set_is_complete_only_when_nothing_is_outstanding() {
        let mut pending = PendingSideChannels::new(4, true);
        for slot in 0..4 {
            assert!(!pending.complete(), "slot {slot} is still missing");
            pending.stems[slot] = Some(PathBuf::from(format!("/tmp/{slot}.ogg")));
        }
        assert!(!pending.complete(), "the lyrics were asked for and are not here");
        pending.want_lyrics = false;
        assert!(pending.complete());
        assert!(pending
            .clone()
            .into_job(DeckId::A, 4, Arc::new(TrackPcm { frames: vec![], sample_rate: 44_100 }))
            .is_some());
        // A set that never wanted lyrics is complete without them.
        let mut bare = PendingSideChannels::new(4, false);
        bare.stems = std::array::from_fn(|slot| Some(PathBuf::from(format!("/tmp/{slot}.ogg"))));
        assert!(bare.complete());
    }

    #[test]
    fn output_window_recreates_after_close_and_restores_when_still_alive() {
        assert_eq!(
            output_window_command(OutputWindowLifecycle::Closed, false),
            Some(OutputWindowCommand::Recreate)
        );
        assert_eq!(
            output_window_command(OutputWindowLifecycle::Open, false),
            Some(OutputWindowCommand::Restore)
        );
        assert_eq!(
            output_window_command(OutputWindowLifecycle::Open, true),
            Some(OutputWindowCommand::Deminiaturize)
        );
        assert_eq!(
            output_window_command(OutputWindowLifecycle::Opening, false),
            None
        );
    }

    #[test]
    fn lighting_defaults_are_dim_and_hazards_fail_closed() {
        let state = LightingControls::default();
        assert!((state.master - 0.26).abs() < f32::EPSILON);
        assert!(state.black_floor <= 0.02);
        assert!(!state.any_hazard_armed());
        assert!(!state.hazards_live());
        assert_eq!(state.strobe, 0.0);
        assert_eq!(state.laser_level, 0.0);
        assert_eq!(state.smoke_level, 0.0);
        assert_eq!(state.uv_level, 0.0);
    }

    #[test]
    fn lighting_env_values_are_finite_and_bounded() {
        assert_eq!(parse_level(Some("0.42"), 0.1, 0.0, 1.0), 0.42);
        assert_eq!(parse_level(Some(" 9 "), 0.1, 0.0, 1.0), 1.0);
        assert_eq!(parse_level(Some("-2"), 0.1, 0.0, 1.0), 0.0);
        assert_eq!(parse_level(Some("NaN"), 0.1, 0.0, 1.0), 0.1);
        assert_eq!(parse_level(Some("bad"), 0.1, 0.0, 1.0), 0.1);
    }

    #[test]
    fn loop_report_cannot_claim_a_cycle_longer_than_the_media() {
        let report = LoopReport {
            detection: LoopDetection {
                kind: LoopKind::Wrap,
                confidence: 0.9,
                ..LoopDetection::default()
            },
            period_secs: 5.8,
        };
        assert!(!loop_report_matches_media(report, 1.6));
        assert!(loop_report_matches_media(
            LoopReport { period_secs: 1.55, ..report },
            1.6
        ));
    }

    #[test]
    fn blackout_clears_hazard_arm_and_deadman() {
        let mut state = LightingControls {
            laser_level: 0.7,
            laser_armed: true,
            deadman_held: true,
            ..LightingControls::default()
        };
        assert!(state.hazards_live());
        state.latch_blackout();
        assert!(state.blackout_latched);
        assert!(!state.any_hazard_armed());
        assert!(!state.deadman_held);
        assert!(!state.hazards_live());
        // Preset levels may survive for the next explicitly armed show; the
        // arm and deadman are the safety boundary.
        assert_eq!(state.laser_level, 0.7);
    }

    #[test]
    fn program_light_mix_uses_the_exact_picture_fraction_and_blackout_gate() {
        let a = SpatialLightSample::uniform(LightSample {
            rgb: [1.0, 0.0, 0.0],
            intensity: 0.2,
        });
        let b = SpatialLightSample::uniform(LightSample {
            rgb: [0.0, 0.0, 1.0],
            intensity: 1.0,
        });
        let mixed = mix_program_lights(Some(a), Some(b), 0.25, true);
        assert_eq!(mixed.overall.rgb, [0.75, 0.0, 0.25]);
        assert!((mixed.overall.intensity - 0.4).abs() < 1e-6);
        for zone in mixed.zones {
            assert_eq!(zone, mixed.overall);
        }
        assert_eq!(
            mix_program_lights(Some(a), Some(b), 0.25, false),
            SpatialLightSample::default()
        );
    }

    /// A beat estimate whose `next_beat` sits `in_ms` after `now`.
    fn beat_at(
        now: Instant,
        confidence: f32,
        locked: bool,
        bpm: f64,
        in_ms: u64,
        beat_index: u64,
        beats_observed: u64,
    ) -> BeatInfo {
        BeatInfo {
            bpm: bpm as f32,
            confidence,
            locked,
            period: Duration::from_secs_f64(60.0 / bpm),
            next_beat: now + Duration::from_millis(in_ms),
            beat_index,
            beats_observed,
        }
    }

    #[test]
    fn no_lock_or_low_confidence_stays_immediate_and_honest() {
        let now = Instant::now();
        assert_eq!(plan_fade(None, now, 1.5), FadePlan::Immediate { secs: 1.5 });
        let unlocked = beat_at(now, 0.9, false, 120.0, 100, 0, 64);
        assert_eq!(
            plan_fade(Some(&unlocked), now, 1.5),
            FadePlan::Immediate { secs: 1.5 }
        );
        let weak = beat_at(now, CONF_QUANTIZE - 0.01, true, 120.0, 100, 0, 64);
        assert_eq!(plan_fade(Some(&weak), now, 1.5), FadePlan::Immediate { secs: 1.5 });
    }

    #[test]
    fn external_sync_bypass_ignores_a_strong_grid() {
        let now = Instant::now();
        let strong = beat_at(now, 0.95, true, 120.0, 200, 0, 64);
        assert!(matches!(
            plan_external_fade(true, Some(&strong), now, 1.25),
            FadePlan::Quantized { .. }
        ));
        assert_eq!(
            plan_external_fade(false, Some(&strong), now, 1.25),
            FadePlan::Immediate { secs: 1.25 }
        );
    }

    #[test]
    fn medium_confidence_cuts_exactly_on_the_next_beat() {
        let now = Instant::now();
        let beat = beat_at(now, 0.6, true, 120.0, 200, 1, 64);
        let FadePlan::Quantized { fire_at, secs, kind } = plan_fade(Some(&beat), now, 3.0)
        else {
            panic!("medium confidence must quantize");
        };
        assert_eq!(fire_at, now + Duration::from_millis(200));
        assert_eq!(secs, CUT_SECS);
        assert_eq!(kind, "cut on beat");
    }

    #[test]
    fn high_confidence_prefers_the_bar_with_integer_beat_fades() {
        let now = Instant::now();
        // 120 BPM, next beat in 100ms at bar position 2 → bar lands 2 beats
        // later; authored 1.0s fade fits 2 beats = 1.0s exactly.
        let beat = beat_at(now, 0.9, true, 120.0, 100, 2, 32);
        let FadePlan::Quantized { fire_at, secs, kind } = plan_fade(Some(&beat), now, 1.0)
        else {
            panic!("high confidence must quantize");
        };
        assert_eq!(kind, "fade on bar");
        assert_eq!(fire_at, now + Duration::from_millis(100) + Duration::from_secs(1));
        assert!((secs - 1.0).abs() < 1e-6, "2 beats at 120 BPM = 1.0s, got {secs}");
        // A very short authored fade still runs at least one whole beat.
        let FadePlan::Quantized { secs, .. } = plan_fade(Some(&beat), now, 0.1) else {
            panic!()
        };
        assert!((secs - 0.5).abs() < 1e-6, "minimum one beat, got {secs}");
    }

    #[test]
    fn bar_preference_needs_history_and_a_bounded_wait() {
        let now = Instant::now();
        // Too few tracked beats: quantize to the next beat instead.
        let fresh = beat_at(now, 0.9, true, 120.0, 100, 2, MIN_BEATS_FOR_BAR - 1);
        let FadePlan::Quantized { kind, .. } = plan_fade(Some(&fresh), now, 1.0) else {
            panic!()
        };
        assert_eq!(kind, "fade on beat");
        // Bar too far away (70 BPM, 3 beats to the bar ≈ 2.67s): next beat.
        let far = beat_at(now, 0.9, true, 70.0, 100, 1, 64);
        let FadePlan::Quantized { kind, fire_at, .. } = plan_fade(Some(&far), now, 1.0)
        else {
            panic!()
        };
        assert_eq!(kind, "fade on beat");
        assert_eq!(fire_at, now + Duration::from_millis(100));
    }

    #[test]
    fn extrapolate_advances_whole_periods_and_the_bar_index() {
        let now = Instant::now();
        let mut beat = beat_at(now, 0.9, true, 120.0, 0, 0, 64);
        beat.period = Duration::from_millis(500);
        beat.next_beat = now - Duration::from_millis(625);
        let advanced = extrapolate_beat(&beat, now);
        assert_eq!(advanced.next_beat, now + Duration::from_millis(375));
        assert_eq!(advanced.beat_index, 2);
        // Already in the future: untouched.
        beat.next_beat = now + Duration::from_millis(80);
        let advanced = extrapolate_beat(&beat, now);
        assert_eq!(advanced.next_beat, beat.next_beat);
        assert_eq!(advanced.beat_index, 0);
    }

    #[test]
    fn beat_snapshot_projects_onto_the_host_clock() {
        use crate::beat_sync::BeatLockState;
        let now = Instant::now();
        let snapshot = BeatSnapshot {
            sample_rate: 48_000.0,
            sample_position: 96_000,
            bpm: 120.0,
            beat_period_samples: 24_000.0,
            phase_sample: 0.0,
            beat_index: 4,
            confidence: 0.9,
            state: BeatLockState::Locked,
            last_onset_sample: Some(95_000),
        };
        let mut lock_started = None;
        let beat = beat_info_from_snapshot(&snapshot, &mut lock_started, now).unwrap();
        assert!(beat.locked);
        assert_eq!(lock_started, Some(4));
        // Next strict boundary after sample 96 000 on a 24 000 grid is
        // 120 000 → exactly half a second out on the host clock.
        assert_eq!(beat.next_beat, now + Duration::from_millis(500));
        assert!((beat.period.as_secs_f64() - 0.5).abs() < 1e-9);
        assert_eq!(beat.beat_index, (4 + 1) % 4);
        assert_eq!(beat.beats_observed, 0);
        // Twelve beats later the observed count grows from the lock start.
        let later = BeatSnapshot {
            sample_position: 96_000 + 12 * 24_000,
            beat_index: 16,
            ..snapshot
        };
        let beat = beat_info_from_snapshot(&later, &mut lock_started, now).unwrap();
        assert_eq!(beat.beats_observed, 12);
        // Losing the lock clears the tracked history.
        let lost = BeatSnapshot { state: BeatLockState::Lost, ..snapshot };
        let beat = beat_info_from_snapshot(&lost, &mut lock_started, now).unwrap();
        assert!(!beat.locked);
        assert_eq!(beat.beats_observed, 0);
    }

    #[test]
    fn frame_signature_downsamples_and_detects_translation() {
        // A bright vertical band that shifts right between frames must
        // produce positive x motion with meaningful activity — and the
        // block channels must respect the loop_detect size caps.
        let (w, h) = (256usize, 144usize);
        let frame_at = |shift: usize| -> Vec<u32> {
            let mut out = vec![0xff10_1010u32; w * h];
            for y in 0..h {
                for x in 0..w {
                    if ((x + w - shift) % w) < w / 8 {
                        out[y * w + x] = 0xffe0_e0e0;
                    }
                }
            }
            out
        };
        let mut state = SigState::default();
        let first = build_frame_signature(&frame_at(0), w, h, &mut state);
        assert_eq!(first.motion.activity, 0.0, "no previous frame, no motion claim");
        assert_eq!(first.luma_blocks.len(), SIG_LUMA_W * SIG_LUMA_H);
        assert_eq!(first.chroma_blocks.len(), SIG_CHROMA_W * SIG_CHROMA_H);
        assert_eq!(first.edge_blocks.len(), SIG_LUMA_W * SIG_LUMA_H);
        assert!(first.luma_blocks.len() <= crate::loop_detect::MAX_CHANNEL_VALUES);
        // Bright band cells read bright; background cells read dark.
        let max = first.luma_blocks.iter().cloned().fold(0.0f32, f32::max);
        let min = first.luma_blocks.iter().cloned().fold(1.0f32, f32::min);
        assert!(max > 0.6 && min < 0.2, "max {max} min {min}");
        // 8 px right on a 256-wide frame = 2 raster cells: inside the
        // block-match search radius.
        let second = build_frame_signature(&frame_at(8), w, h, &mut state);
        assert!(second.motion.activity > 0.05, "{:?}", second.motion);
        assert!(second.motion.x > 0.005, "rightward motion, got {:?}", second.motion);
    }

    #[test]
    fn capture_feed_downmixes_planar_channels_and_roundtrips() {
        let feed = CaptureFeed::new();
        let mut buffer = AudioBuffer::new_with_size(4, 2);
        buffer.channel_mut(0).copy_from_slice(&[0.2, 0.4, -0.6, 0.8]);
        buffer.channel_mut(1).copy_from_slice(&[0.0, 0.0, -0.2, 0.4]);
        feed.push(48_000.0, &buffer);
        let mut out = Vec::new();
        let rate = feed.drain_into(&mut out);
        assert_eq!(rate, 48_000);
        assert_eq!(out.len(), 4);
        for (got, want) in out.iter().zip([0.1f32, 0.2, -0.4, 0.6]) {
            assert!((got - want).abs() < 1e-6, "downmix avg: got {got}, want {want}");
        }
        let stats = feed.stats();
        assert_eq!(stats.frames_written, 4);
        assert_eq!(stats.dropped_samples, 0);
        assert!((stats.peak - 0.6).abs() < 1e-6, "block peak, got {}", stats.peak);
    }

    #[test]
    fn capture_feed_overflow_drops_newest_counts_and_recovers() {
        let feed = CaptureFeed::new();
        let mut buffer = AudioBuffer::new_with_size(1024, 1);
        let blocks = CAPTURE_RING / 1024 + 3;
        for block in 0..blocks {
            for (i, sample) in buffer.channel_mut(0).iter_mut().enumerate() {
                *sample = (block * 1024 + i) as f32;
            }
            feed.push(44_100.0, &buffer);
        }
        let stats = feed.stats();
        assert_eq!(stats.frames_written, (blocks * 1024) as u64);
        assert_eq!(stats.dropped_samples, (blocks * 1024 - CAPTURE_RING) as u64);
        let mut out = Vec::new();
        feed.drain_into(&mut out);
        assert_eq!(out.len(), CAPTURE_RING, "ring holds exactly its capacity");
        // Drop-NEWEST policy: the head of the stream is intact.
        assert_eq!(out[0], 0.0);
        assert_eq!(out[CAPTURE_RING - 1], (CAPTURE_RING - 1) as f32);
        // After a drain the producer has room again and nothing new drops.
        feed.push(44_100.0, &buffer);
        let stats = feed.stats();
        assert_eq!(stats.dropped_samples, (blocks * 1024 - CAPTURE_RING) as u64);
        out.clear();
        feed.drain_into(&mut out);
        assert_eq!(out.len(), 1024);
    }

    #[test]
    fn loop_worker_accumulates_and_reports_the_visual_period() {
        let (tx, results) = start_loop_worker();
        let revision = AssetRevisionId::from_bytes([7; 32]);
        tx.send(LoopScanCtl::Reset { slot: 0, revision: Some(revision) }).unwrap();
        // Five clean 24-frame cycles at 30 fps → a 0.8 s Wrap loop.
        let period = 24usize;
        for index in 0..(period * 5) {
            let angle = (index % period) as f32 * std::f32::consts::TAU / period as f32;
            let value = 0.5 + 0.42 * angle.sin();
            let sig = FrameSignature::new(
                vec![value, value * 0.73 + 0.11, 1.0 - value * 0.6],
                vec![[value * 0.4, 1.0 - value * 0.3]; 2],
                vec![value, 1.0 - value],
                MotionSummary::new(0.05, 0.0, 0.0, 0.8),
            );
            tx.send(LoopScanCtl::Sig {
                slot: 0,
                revision,
                position_secs: index as f64 / 30.0,
                sig,
            })
            .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut best: Option<LoopReport> = None;
        while Instant::now() < deadline {
            for (got_revision, report) in results.lock().unwrap().drain(..) {
                assert_eq!(got_revision, revision);
                best = Some(report);
            }
            if best.is_some_and(|report| report.detection.kind == LoopKind::Wrap) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let report = best.expect("loop worker must publish a report");
        assert_eq!(report.detection.kind, LoopKind::Wrap, "{report:?}");
        assert!(
            (report.period_secs - 0.8).abs() < 0.06,
            "24 frames at 30 fps ≈ 0.8 s, got {}",
            report.period_secs
        );
        assert!(report.detection.confidence > 0.6, "{report:?}");
        drop(tx); // channel close ends the worker cleanly
    }

    #[test]
    fn sync_worker_publishes_capture_health_and_no_fake_beat() {
        let feed = Arc::new(CaptureFeed::new());
        let worker = SyncWorker::start(feed.clone());
        let mut buffer = AudioBuffer::new_with_size(512, 2);
        for sample in buffer.channel_mut(0).iter_mut() {
            *sample = 0.5;
        }
        feed.push(48_000.0, &buffer);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snap = worker.snapshot();
            if snap.frames >= 512 {
                assert_eq!(snap.sample_rate, 48_000);
                // Honesty invariant: no detector, no beat claim.
                assert!(snap.beat.is_none());
                break;
            }
            assert!(Instant::now() < deadline, "worker never published");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(worker); // detached stop — must not hang the test thread
    }

    #[test]
    fn sync_worker_publishes_a_stamped_capture_envelope() {
        let feed = Arc::new(CaptureFeed::new());
        let worker = SyncWorker::start(feed.clone());
        let mut buffer = AudioBuffer::new_with_size(512, 2);
        for channel in 0..2 {
            for (index, sample) in buffer.channel_mut(channel).iter_mut().enumerate() {
                *sample = if index % 64 == 0 { 0.8 } else { 0.05 };
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut cols = Vec::new();
        loop {
            // 10 buffers = 5120 samples = ten 10 ms columns at 48 kHz.
            for _ in 0..10 {
                feed.push(48_000.0, &buffer);
            }
            let snap = worker.snapshot();
            snap.wave.unroll(&mut cols);
            assert_eq!(cols.len(), WAVE_COLS);
            if cols.iter().filter(|col| **col > 0).count() >= 8 {
                // The newest columns are at the END: the wave draws left to
                // right, oldest to now.
                assert!(cols[WAVE_COLS - 1] > 0, "newest column must be last");
                let peak = (cols[WAVE_COLS - 1] >> 8) as f32 / 255.0;
                let rms = (cols[WAVE_COLS - 1] & 0xff) as f32 / 255.0;
                let drawn = 0.8f32.powf(crate::wave_analysis::WAVE_CURVE);
                assert!((peak - drawn).abs() < 0.02, "{peak} vs {drawn}");
                assert!(rms > 0.0 && rms < peak, "rms {rms} peak {peak}");
                let stamp = snap.wave_stamp.expect("a filled ring is stamped");
                assert!(stamp.elapsed() < Duration::from_secs(1));
                break;
            }
            assert!(Instant::now() < deadline, "worker never filled the wave");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(worker);
    }

    #[test]
    fn the_wave_ring_unrolls_oldest_first() {
        let level = |index: usize| (index % 200) as f32 / 200.0;
        let drawn = |index: usize| {
            (level(index).powf(crate::wave_analysis::WAVE_CURVE) * 255.0).round() as u16
        };
        let mut ring = WaveRing::default();
        for index in 0..WAVE_COLS + 3 {
            ring.push(level(index), 0.0);
        }
        let mut cols = Vec::new();
        ring.unroll(&mut cols);
        assert_eq!(cols.len(), WAVE_COLS);
        // The last three pushes are the last three columns, in order.
        for (back, index) in (WAVE_COLS..WAVE_COLS + 3).rev().enumerate() {
            assert_eq!(cols[WAVE_COLS - 1 - back] >> 8, drawn(index), "back {back}");
        }
    }

    // ---- tap tempo as a clock override ------------------------------------

    fn detector_beat(bpm: f32, next_in: Duration, index: u64) -> BeatInfo {
        BeatInfo {
            bpm,
            confidence: 0.9,
            locked: true,
            period: Duration::from_secs_f64(60.0 / bpm as f64),
            next_beat: Instant::now() + next_in,
            beat_index: index,
            beats_observed: 32,
        }
    }

    #[test]
    fn one_tap_moves_the_downbeat_and_keeps_the_tempo() {
        let base = detector_beat(120.0, Duration::from_millis(370), 2);
        let anchor = Instant::now();
        let over = BeatOverride { anchor, bpm: None, leader: None, saw_unlock: false };
        let beat = over.beat(Some(&base), anchor).expect("a clock underneath supplies the tempo");
        // Tempo untouched...
        assert_eq!(beat.period, base.period);
        assert!((beat.bpm - 120.0).abs() < 1e-3);
        // ...phase re-anchored: the tap IS beat one, so the next beat is the
        // second of the bar, one period away.
        assert_eq!(beat.beat_index, 1);
        assert_eq!(beat.next_beat, anchor + base.period);
        assert!(beat.locked && beat.confidence >= CONF_MUSICAL);
        // Bar preference has to earn itself again from the tap.
        assert_eq!(beat.beats_observed, 0);
    }

    #[test]
    fn four_taps_replace_tempo_and_phase() {
        let base = detector_beat(120.0, Duration::from_millis(370), 2);
        let anchor = Instant::now();
        let over = BeatOverride { anchor, bpm: Some(128.0), leader: None, saw_unlock: false };
        let period = Duration::from_secs_f64(60.0 / 128.0);
        let beat = over.beat(Some(&base), anchor + period * 5).expect("a tapped tempo stands alone");
        assert!((beat.bpm - 128.0).abs() < 1e-3, "{}", beat.bpm);
        assert_eq!(beat.period, period);
        // Five whole beats after the one: the sixth is bar position 1.
        assert_eq!(beat.beat_index, (5 + 1) % BAR_BEATS);
        assert_eq!(beat.beats_observed, 5);
        // And it still stands with no clock underneath at all.
        assert!(over.beat(None, anchor).is_some());
    }

    #[test]
    fn a_phase_only_override_needs_a_clock_underneath() {
        let over = BeatOverride {
            anchor: Instant::now(),
            bpm: None,
            leader: None,
            saw_unlock: false,
        };
        // Nothing to anchor: "the one is here" says nothing about tempo, so
        // it must not invent one.
        assert!(over.beat(None, Instant::now()).is_none());
    }

    #[test]
    fn the_detector_takes_the_clock_back_only_after_losing_it() {
        // A confident lock that never dropped is the SAME music the operator
        // tapped: it may not yank the clock back mid-set.
        assert!(!detector_reclaims(BeatLockState::Locked, 0.95, false));
        // Nor may a shaky one, even after a dropout.
        assert!(!detector_reclaims(BeatLockState::Locked, 0.4, true));
        // A new track: grid lost, then a lock the fade policy would trust.
        assert!(detector_reclaims(BeatLockState::Locked, 0.95, true));
        // Only a real dropout arms it — re-settling does not.
        assert!(detector_lost_grid(BeatLockState::Unlocked));
        assert!(detector_lost_grid(BeatLockState::Lost));
        assert!(!detector_lost_grid(BeatLockState::Acquiring));
        assert!(!detector_lost_grid(BeatLockState::Holdover));
    }

    #[test]
    fn four_taps_at_a_tempo_produce_that_clock() {
        // End to end over the two halves: the tap rule makes a BPM, the
        // override turns it into the grid every consumer reads.
        let mut tap = TapTempo::new();
        let period = 60.0 / 124.0;
        let mut clock = None;
        for index in 0..4 {
            clock = tap.tap(index as f64 * period);
        }
        let clock = clock.expect("rhythmic taps stand");
        let bpm = clock.bpm.expect("four taps make a tempo");
        let anchor = Instant::now();
        let over = BeatOverride { anchor, bpm: Some(bpm), leader: None, saw_unlock: false };
        let beat = over.beat(None, anchor).unwrap();
        assert!((beat.bpm - 124.0).abs() < 0.01, "{}", beat.bpm);
        assert_eq!(beat.beat_index, 1);
        // The bar rules from the tap: the LED reads the same reference.
        let led = views::BeatRef {
            next_beat_secs: 10.0 + beat.period.as_secs_f64(),
            period_secs: beat.period.as_secs_f64(),
            next_index: beat.beat_index as u32,
            bar_beats: BAR_BEATS as u32,
            coasting: false,
        };
        let (since, index) = led.at(10.0);
        assert!(since < 1e-9, "the tap itself is a beat: {since}");
        assert_eq!(index, 0, "and it is the ONE");
        let (since, index) = led.at(10.0 + beat.period.as_secs_f64() * 2.0);
        assert!(since < 1e-9, "{since}");
        assert_eq!(index, 2);
    }

    #[test]
    fn the_source_ladder_puts_the_dj_above_the_room_and_the_operator_above_both() {
        use ClockSource::*;
        // The normal show: a deck is playing, so the deck IS the clock and
        // the detector — which can only hear that deck — is not consulted.
        assert_eq!(resolve_clock_source(false, false, true, true), Deck);
        assert_eq!(resolve_clock_source(false, false, true, false), Deck);
        // VJ standalone against somebody else's music.
        assert_eq!(resolve_clock_source(false, false, false, true), Detector);
        // A deck following the room: the room leads, the deck's own grid is
        // never read back or it would be chasing itself.
        assert_eq!(resolve_clock_source(false, true, true, true), External);
        // ...but EXT with nothing detected falls back to the deck rather
        // than to nothing at all.
        assert_eq!(resolve_clock_source(false, true, true, false), Deck);
        // The operator outranks every one of them.
        assert_eq!(resolve_clock_source(true, true, true, true), Operator);
        assert_eq!(resolve_clock_source(false, false, false, false), None);
    }

    #[test]
    fn a_sprite_cycle_spans_a_power_of_two_number_of_beats() {
        // Four frames to the beat: a four-frame walk marches one cycle per
        // beat, eight frames take two, a sixteen-frame cycle takes a bar.
        assert_eq!(sprite_cycle_beats(4), 1);
        assert_eq!(sprite_cycle_beats(8), 2);
        assert_eq!(sprite_cycle_beats(16), 4);
        assert_eq!(sprite_cycle_beats(32), 8);
        // Short states never go below one beat...
        assert_eq!(sprite_cycle_beats(1), 1);
        assert_eq!(sprite_cycle_beats(2), 1);
        assert_eq!(sprite_cycle_beats(3), 1);
        // ...odd counts land on the nearer power of two...
        assert_eq!(sprite_cycle_beats(6), 2);
        assert_eq!(sprite_cycle_beats(5), 1);
        assert_eq!(sprite_cycle_beats(12), 4);
        // ...and long ones stop at a bar-and-a-bar rather than running away.
        assert_eq!(sprite_cycle_beats(64), 8);
        assert_eq!(sprite_cycle_beats(300), 8);
        // Every answer is a power of two.
        for frames in 1..64 {
            let beats = sprite_cycle_beats(frames);
            assert!(beats.is_power_of_two() && (1..=8).contains(&beats), "{frames} -> {beats}");
        }
    }

    #[test]
    fn a_synced_sprite_steps_on_the_grid() {
        // The stepper is `floor(fract(position / cycle_beats) * frames)`;
        // walk a synthetic clock through a bar and check the frames land on
        // the subdivisions rather than on wall-clock fps.
        let frames = 8usize;
        let beats = sprite_cycle_beats(frames) as f64;
        assert_eq!(beats, 2.0);
        let frame_at = |position: f64| {
            let cycle = (position / beats).rem_euclid(1.0);
            ((cycle * frames as f64).floor() as usize).min(frames - 1)
        };
        // One frame per quarter beat, and the cycle restarts on the beat
        // that starts the two-beat unit.
        assert_eq!(frame_at(0.0), 0);
        assert_eq!(frame_at(0.25), 1);
        assert_eq!(frame_at(0.5), 2);
        assert_eq!(frame_at(1.0), 4);
        assert_eq!(frame_at(1.75), 7);
        assert_eq!(frame_at(2.0), 0);
        assert_eq!(frame_at(4.0), 0);
        // A frame drawn a hair before the boundary is still the last one:
        // the step happens ON the beat, never before it.
        assert_eq!(frame_at(2.0 - 1e-9), 7);
    }

    #[test]
    fn a_beat_reference_resolves_the_phase_between_beats() {
        let led = views::BeatRef {
            next_beat_secs: 4.0,
            period_secs: 0.5,
            next_index: 0,
            bar_beats: BAR_BEATS as u32,
            coasting: false,
        };
        // Half a beat before the one: the LED is dark and counting beat four.
        let (since, index) = led.at(3.75);
        assert!((since - 0.25).abs() < 1e-9, "{since}");
        assert_eq!(index, 3);
        // The grid coordinate the wave rules with lands on whole numbers
        // exactly on the beat, and on a multiple of the bar on the one.
        let on_the_one = led.coordinate(4.0);
        assert!((on_the_one.fract()).abs() < 1e-9, "{on_the_one}");
        assert!((on_the_one % BAR_BEATS as f64).abs() < 1e-9, "{on_the_one}");
        let third = led.coordinate(5.0);
        assert!((third % BAR_BEATS as f64 - 2.0).abs() < 1e-9, "{third}");
    }
}
