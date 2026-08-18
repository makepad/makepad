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

mod apc40;
mod beat_sync;
mod billboard;
mod catalog;
mod cue;
mod decks;
mod fx;
mod gen;
mod lanes;
mod local_lib;
mod loop_detect;
mod media;
mod mesh_view;
mod mixer;
mod pads;
mod service;
mod views;

use crate::apc40::{Apc40State, ApcAction, ApcSurface, LedDiff, LedFrame, PadLed, PAD_COUNT};
use crate::beat_sync::{
    fit_loop_to_grid, BeatFit, BeatLockState, BeatSnapshot, BeatSyncAnalyzer,
};
use crate::catalog::{BrowseModel, CatCmd, CatGen, TileMedia, TileThumb};
use crate::cue::{CueCmd, CueEngine, CueGen, CueItem, CueScheduleId, SlotId};
use crate::loop_detect::{
    analyze_video_loop, FrameSignature, LoopDetection, LoopKind, MotionSummary,
};
use crate::decks::{DeckCmd, DeckEngine, DeckId, DeckLoad, DeckTarget, FadeCurve, TrackItem};
use crate::fx::FxState;
use crate::local_lib::{LocalItem, LocalLibrary};
use crate::gen::{GenCmd, GenModel, GenTag, ProfilesState};
use crate::lanes::{LatestWins, AUDIO_LANE};
use crate::media::{DecodeDone, DecodeJob, DecodePool, SlotPlayer};
use crate::mixer::{
    Mixer, TrackPcm, VideoTransitionError, VideoTransitionId, VideoTransitionPhase,
};
use crate::pads::{PadCmd, PadEngine, PadItem};
use crate::views::{GridEntry, JobRowEntry, VjJobList, VjPadMatrix, VjTileGrid, GRID_SLOTS};
use makepad_widgets::splitter::{Splitter, SplitterAlign};
use makepad_asset_client::{
    select_file, CatalogSubscriptionEvent, ClientEvent, ClientOutput, ClientRequest, JobId,
    RequestId, SessionConnector, SessionHandles, SessionMsg, SessionStatus, TierPreference,
};
use makepad_asset_data::{
    AssetId, AssetKind, AssetManifest, AssetRevisionId, DeviceTier, FileRole, MediaType,
};
use makepad_show_control::{
    is_vj_reserved_midi, ArtNetConfig, ColorControl, HazardArms, HazardControl, LightSample,
    MoverControl, PerformanceConfig, PerformanceState, PowerCaps, PresetBank, RoomShow,
    SpatialLightSample, StrobeControl, VideoLightAnalyzer, ARTNET_BROADCAST_ADDR, SCENE_COUNT,
};
use std::collections::{HashMap, VecDeque};
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
        draw_text.color: #x7a8794
        draw_text.text_style.font_size: 9
    }

    let ValueLabel = Label{
        draw_text.color: #xe8eef4
        draw_text.text_style.font_size: 11
    }

    let ChromeButton = Button{
        draw_bg +: {
            color: #x171c23
            color_hover: #x222933
            color_down: #x0f1318
            border_color: #xffffff14
            border_radius: 6.0
            border_size: 1.0
        }
        draw_text +: {
            color: #xc5d0da
            color_hover: #xfffaf4
            text_style: theme.font_regular{font_size: 10}
        }
    }

    let PillButton = Button{
        draw_bg +: {
            color: #x12161c
            color_hover: #x1c232c
            color_down: #x0c1014
            border_color: #xffffff10
            border_radius: 12.0
            border_size: 1.0
        }
        draw_text +: {
            color: #x9aa6b2
            color_hover: #x3ee0b0
            text_style: theme.font_bold{font_size: 10}
        }
    }

    let DeckWell = RoundedView{
        width: Fill
        height: Fill
        padding: 1
        draw_bg +: {
            color: #x000000
            border_color: #xffffff10
            border_size: 1.0
            border_radius: 10.0
        }
    }

    let ApcKnob = Rotary{
        width: 40
        height: 40
        min: 0.0
        max: 1.0
        text: ""
        flow: Down
        text_input: TextInput{
            width: 0
            height: 0
        }
    }

    let ApcFader = Slider{
        axis: DragAxis.Vertical
        width: 40
        height: 168
        min: 0.0
        max: 1.0
        text: ""
        flow: Down
        text_input: TextInput{
            width: 0
            height: 0
        }
        draw_bg +: {
            body_color: uniform(#x080a0e)
            track_color: uniform(#x161a20)
            fill_color: uniform(#x3ee0b0)
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
        draw_text.color: #x7a8794
        draw_text.text_style: theme.font_bold{font_size: 7}
    }

    let FaderCol = View{
        width: 40
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
            body_color: uniform(#x080a0e)
            track_color: uniform(#x161a20)
            fill_color: uniform(#x3ee0b0)
            cap_color: uniform(#xe8eef4)
            cap_shadow: uniform(#x8d98a7)
            pixel: fn() {
                let sdf = Sdf2d.viewport(self.pos * self.rect_size)
                sdf.box(2., 6., self.rect_size.x - 4., self.rect_size.y - 12., 8.)
                sdf.fill(self.body_color)
                let left = 10.
                let right_pad = 10.
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
                body +: {
                    SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 6
                        padding: Inset{left: 10.0 right: 10.0 top: 8.0 bottom: 8.0}
                        draw_bg.color: #x07080b

                        // ---- header: connection + master ----
                        View{
                            width: Fill
                            height: 36
                            flow: Right
                            spacing: 12
                            align: Align{x: 0.0, y: 0.5}
                            Label{
                                text: "VJ"
                                draw_text.color: #x3ee0b0
                                draw_text.text_style: theme.font_bold{font_size: 20}
                            }
                            // Offscreen A/B mesh passes. Each slot has its
                            // own color+depth so two models never share a
                            // z-buffer; VideoProgram samples the textures.
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
                            status_label := Label{
                                width: Fill
                                text: "starting…"
                                draw_text.color: #x8b97a3
                                draw_text.text_style.font_size: 11
                            }
                            show_status_label := Label{
                                width: 280
                                text: "show control starting…"
                                draw_text.color: #x66727f
                                draw_text.text_style.font_size: 9
                            }
                            PanelLabel{text: "MASTER"}
                            master_slider := Slider{
                                width: 150
                                min: 0.0
                                max: 1.2
                                default: 0.9
                            }
                            open_output := ChromeButton{text: "OUTPUT"}
                            output_window_status := PanelLabel{text: "output open"}
                        }

                        // ---- surface tabs ----
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 6
                            align: Align{x: 0.0, y: 0.5}
                            tab_video := PillButton{text: "VIDEO"}
                            tab_music := PillButton{text: "MUSIC"}
                            tab_sfx := PillButton{text: "SFX"}
                            tab_mesh := PillButton{text: "MESH"}
                            tab_gen := PillButton{text: "GENERATE"}
                            View{width: Fill height: 1}
                            apc_map_label := PanelLabel{text: ""}
                        }

                        // Compact fixed-geometry sync strip. Changing values
                        // stay in fixed-width slots so the beat pump cannot
                        // reflow the stage.
                        external_sync_panel := RoundedView{
                            width: Fill
                            height: 40
                            flow: Right
                            spacing: 10
                            padding: Inset{left: 10.0 right: 10.0 top: 4.0 bottom: 4.0}
                            align: Align{x: 0.0 y: 0.5}
                            draw_bg +: {
                                color: #x0d1116
                                border_color: #xffffff0c
                                border_size: 1.0
                                border_radius: 8.0
                            }
                            Label{
                                width: 42
                                text: "SYNC"
                                draw_text.color: #x3ee0b0
                                draw_text.text_style: theme.font_bold{font_size: 10}
                            }
                            external_bpm := Label{
                                width: 128
                                text: "  ---.- BPM"
                                draw_text.color: #xf4f7fa
                                draw_text.text_style: theme.font_bold{font_size: 18}
                            }
                            external_lock := Label{
                                width: 108
                                text: "LOCK: STARTING"
                                draw_text.color: #x3ee0b0
                                draw_text.text_style: theme.font_bold{font_size: 9}
                            }
                            external_confidence := Label{
                                width: 88
                                text: "CONF:   0%"
                                draw_text.color: #x8b97a3
                                draw_text.text_style.font_size: 9
                            }
                            external_phase := Label{
                                width: 210
                                text: "BEAT -/4 [........] PHASE   0%"
                                draw_text.color: #x7fdabb
                                draw_text.text_style: theme.font_bold{font_size: 9}
                            }
                            external_capture := Label{
                                width: 220
                                text: "SYSTEM AUDIO: STARTING"
                                draw_text.color: #x8b97a3
                                draw_text.text_style.font_size: 9
                            }
                            external_video_state := Label{
                                width: Fill
                                text: "IDLE: SELECT A VIDEO"
                                draw_text.color: #xc5d0da
                                draw_text.text_style.font_size: 9
                            }
                            external_loop_state := Label{
                                visible: false
                                width: 1
                                text: "LOOP: NO VIDEO | RATE 1.000"
                                draw_text.color: #x8b97a3
                                draw_text.text_style.font_size: 9
                            }
                            external_sync_enable := Toggle{
                                text: "Lock audio"
                                active: true
                            }
                            external_sync_now := ChromeButton{
                                width: 92
                                text: "FORCE"
                            }
                        }

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
                                color: #x0d1116
                                border_color: #xffffff0c
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
                                    draw_text.color: #x3ee0b0
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

                        gen_split := Splitter{
                            width: Fill
                            height: Fill
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.FromA(300.0)
                            min_vertical: 0.0
                            max_vertical: 360.0
                            size: 6.0
                            draw_bg +: {
                                color: #x1a2028
                                color_hover: #x3ee0b0
                                color_drag: #x3ee0b0
                                splitter_pad: 2.0
                                bar_size: 72.0
                            }
                            b: View{
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
                                spacing: 6
                                View{
                                    width: Fill
                                    height: Fill
                                    flow: Right
                                    spacing: 8
                                    // Deck A — live / slot A
                                    View{
                                        width: 248
                                        height: Fill
                                        flow: Down
                                        spacing: 6
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 8
                                            align: Align{x: 0.0, y: 0.5}
                                            slot_a_role := Label{
                                                text: "A"
                                                draw_text.color: #x3ee0b0
                                                draw_text.text_style: theme.font_bold{font_size: 11}
                                            }
                                            now_label := ValueLabel{text: "—"}
                                            View{width: Fill height: 1}
                                            video_pos_label := PanelLabel{text: "0:00 / 0:00"}
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            align: Align{x: 0.0, y: 0.5}
                                            Tick{width: 28 text: "POS"}
                                            video_pos := Slider{
                                                width: Fill
                                                text: ""
                                                min: 0.0
                                                max: 1.0
                                                text_input: TextInput{width: 0 height: 0}
                                            }
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            align: Align{x: 0.0, y: 0.5}
                                            Tick{width: 28 text: "FADE"}
                                            video_fade := Slider{
                                                width: Fill
                                                text: ""
                                                min: 0.05
                                                max: 5.0
                                                default: 1.0
                                                text_input: TextInput{width: 0 height: 0}
                                            }
                                        }
                                        DeckWell{
                                            View{
                                                width: Fill
                                                height: Fill
                                                flow: Overlay
                                                preview_a := VideoProgram{}
                                                deck_a_empty := View{
                                                    width: Fill
                                                    height: Fill
                                                    align: Align{x: 0.5, y: 0.5}
                                                    Label{
                                                        text: "cue"
                                                        draw_text.color: #x4d5863
                                                        draw_text.text_style.font_size: 13
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    // APC40 body — pads = VJ clips, everything else = lights
                                    RoundedView{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        spacing: 6
                                        padding: 8
                                        draw_bg +: {
                                            color: #x0a0c10
                                            border_color: #xffffff10
                                            border_size: 1.0
                                            border_radius: 12.0
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 8
                                            View{
                                                width: Fill
                                                height: Fit
                                                flow: Down
                                                spacing: 2
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Right
                                                    spacing: 4
                                                    Tick{text: "—"}
                                                    Tick{text: "WASH"}
                                                    Tick{text: "COL"}
                                                    Tick{text: "HUE"}
                                                    Tick{text: "—"}
                                                    Tick{text: "BEAM"}
                                                    Tick{text: "—"}
                                                    Tick{text: "—"}
                                                }
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Right
                                                    spacing: 4
                                                    light_knob_0 := ApcKnob{}
                                                    light_knob_1 := ApcKnob{}
                                                    light_knob_2 := ApcKnob{}
                                                    light_knob_3 := ApcKnob{}
                                                    light_knob_4 := ApcKnob{}
                                                    light_knob_5 := ApcKnob{}
                                                    light_knob_6 := ApcKnob{}
                                                    light_knob_7 := ApcKnob{}
                                                }
                                            }
                                            View{width: 40 height: 1}
                                            View{
                                                width: 88
                                                height: Fit
                                                flow: Down
                                                spacing: 2
                                                dev_knob_legend := Tick{text: "SMK"}
                                                View{
                                                width: 88
                                                height: Fit
                                                flow: Right
                                                spacing: 4
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Down
                                                    spacing: 4
                                                    light_dev_0 := ApcKnob{}
                                                    light_dev_4 := ApcKnob{}
                                                }
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Down
                                                    spacing: 4
                                                    light_dev_1 := ApcKnob{}
                                                    light_dev_5 := ApcKnob{}
                                                }
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Down
                                                    spacing: 4
                                                    light_dev_2 := ApcKnob{}
                                                    light_dev_6 := ApcKnob{}
                                                }
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Down
                                                    spacing: 4
                                                    light_dev_3 := ApcKnob{}
                                                    light_dev_7 := ApcKnob{}
                                                }
                                            }
                                            }
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            video_grid := VjPadMatrix{}
                                            View{
                                                width: 36
                                                height: Fill
                                                flow: Down
                                                spacing: 4
                                                light_scene_8 := ApcPad{text: "P9"}
                                                light_scene_9 := ApcPad{text: "P10"}
                                                light_scene_10 := ApcPad{text: "P11"}
                                                light_scene_11 := ApcPad{text: "P12"}
                                                light_scene_12 := ApcPad{text: "P13"}
                                            }
                                            View{width: 88 height: 1}
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 4
                                            light_scene_0 := ApcPad{text: "P1"}
                                            light_scene_1 := ApcPad{text: "P2"}
                                            light_scene_2 := ApcPad{text: "P3"}
                                            light_scene_3 := ApcPad{text: "P4"}
                                            light_scene_4 := ApcPad{text: "P5"}
                                            light_scene_5 := ApcPad{text: "P6"}
                                            light_scene_6 := ApcPad{text: "P7"}
                                            light_scene_7 := ApcPad{text: "P8"}
                                            View{width: 36 height: 1}
                                            View{width: 88 height: 1}
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            align: Align{x: 0.0, y: 1.0}
                                            FaderCol{
                                                Tick{text: "—"}
                                                light_fader_0 := ApcFader{}
                                            }
                                            FaderCol{
                                                Tick{text: "WASH"}
                                                light_fader_1 := ApcFader{}
                                            }
                                            FaderCol{
                                                Tick{text: "GOBO"}
                                                light_fader_2 := ApcFader{}
                                            }
                                            FaderCol{
                                                Tick{text: "RGB"}
                                                light_fader_3 := ApcFader{}
                                            }
                                            FaderCol{
                                                Tick{text: "STRB"}
                                                light_fader_4 := ApcFader{}
                                            }
                                            FaderCol{
                                                Tick{text: "BEAM"}
                                                light_fader_5 := ApcFader{}
                                            }
                                            FaderCol{
                                                Tick{text: "UV"}
                                                light_fader_6 := ApcFader{}
                                            }
                                            FaderCol{
                                                Tick{text: "UV+"}
                                                light_fader_7 := ApcFader{}
                                            }
                                            FaderCol{
                                                Tick{text: "M"}
                                                light_fader_8 := ApcFader{}
                                            }
                                            View{
                                                width: Fill
                                                height: Fill
                                                flow: Down
                                                spacing: 6
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Right
                                                    spacing: 4
                                                    light_track_0 := ApcPad{text: "1"}
                                                    light_track_1 := ApcPad{text: "2"}
                                                    light_track_2 := ApcPad{text: "3"}
                                                    light_track_3 := ApcPad{text: "4"}
                                                    light_track_4 := ApcPad{text: "5"}
                                                    light_track_5 := ApcPad{text: "6"}
                                                    light_track_6 := ApcPad{text: "7"}
                                                    light_track_7 := ApcPad{text: "8"}
                                                }
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Right
                                                    spacing: 6
                                                    align: Align{x: 0.0, y: 0.5}
                                                    video_play := ChromeButton{text: "▶"}
                                                    video_loop := Toggle{text: "loop"}
                                                    video_mute := Toggle{text: "mute"}
                                                    light_power := Toggle{text: "pwr"}
                                                    light_write := Toggle{text: "wrt"}
                                                    light_blackout := Button{text: "BLK"}
                                                }
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Right
                                                    spacing: 6
                                                    align: Align{x: 0.0, y: 0.5}
                                                    mix_mode := ChromeButton{text: "MIX"}
                                                    fx_prev := ChromeButton{text: "<"}
                                                    fx_name := Label{
                                                        width: 72
                                                        text: "OFF"
                                                        draw_text.color: #x3ee0b0
                                                        draw_text.text_style: theme.font_bold{font_size: 9}
                                                    }
                                                    fx_next := ChromeButton{text: ">"}
                                                    View{width: Fill height: 1}
                                                    fx_p1_lab := Tick{width: 36 text: "—"}
                                                    fx_p1 := Slider{
                                                        width: 72
                                                        min: 0.0
                                                        max: 1.0
                                                        default: 0.45
                                                        text: ""
                                                        text_input: TextInput{width: 0 height: 0}
                                                    }
                                                    fx_beat1 := Toggle{text: "♪"}
                                                    fx_p2_lab := Tick{width: 36 text: "—"}
                                                    fx_p2 := Slider{
                                                        width: 72
                                                        min: 0.0
                                                        max: 1.0
                                                        default: 0.35
                                                        text: ""
                                                        text_input: TextInput{width: 0 height: 0}
                                                    }
                                                    fx_beat2 := Toggle{text: "♪"}
                                                }
                                                View{
                                                    width: Fill
                                                    height: Fit
                                                    flow: Right
                                                    spacing: 6
                                                    align: Align{x: 0.0, y: 0.5}
                                                    Tick{width: 14 text: "A"}
                                                    apc_xfader := ApcXfader{}
                                                    Tick{width: 14 text: "B"}
                                                }
                                            }
                                        }
                                        Tick{text: "PGM"}
                                        DeckWell{
                                            width: Fill
                                            height: Fill
                                            preview := VideoProgram{}
                                        }
                                        bb_states := View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 4
                                            bb_s0 := ChromeButton{text: ""}
                                            bb_s1 := ChromeButton{text: ""}
                                            bb_s2 := ChromeButton{text: ""}
                                            bb_s3 := ChromeButton{text: ""}
                                            bb_s4 := ChromeButton{text: ""}
                                            bb_s5 := ChromeButton{text: ""}
                                            bb_s6 := ChromeButton{text: ""}
                                            bb_s7 := ChromeButton{text: ""}
                                        }
                                        next_label := Label{
                                            width: Fill
                                            text: ""
                                            draw_text.color: #x6aa8ff
                                            draw_text.text_style.font_size: 9
                                        }
                                        video_error := Label{
                                            width: Fill
                                            text: ""
                                            draw_text.color: #xe08a7a
                                            draw_text.text_style.font_size: 8
                                        }
                                        light_desk_status := Label{
                                            width: Fill
                                            text: ""
                                            draw_text.color: #x66727f
                                            draw_text.text_style.font_size: 8
                                        }
                                    }
                                    // Deck B — standby / slot B
                                    View{
                                        width: 248
                                        height: Fill
                                        flow: Down
                                        spacing: 6
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 8
                                            align: Align{x: 0.0, y: 0.5}
                                            slot_b_role := Label{
                                                text: "B"
                                                draw_text.color: #x6aa8ff
                                                draw_text.text_style: theme.font_bold{font_size: 11}
                                            }
                                            View{width: Fill height: 1}
                                            slot_b_mode := Label{
                                                text: "STANDBY"
                                                draw_text.color: #x6aa8ff
                                                draw_text.text_style: theme.font_bold{font_size: 9}
                                            }
                                        }
                                        DeckWell{
                                            View{
                                                width: Fill
                                                height: Fill
                                                flow: Overlay
                                                preview_b := VideoProgram{}
                                                deck_b_empty := View{
                                                    width: Fill
                                                    height: Fill
                                                    align: Align{x: 0.5, y: 0.5}
                                                    Label{
                                                        text: "over / standby"
                                                        draw_text.color: #x4d5863
                                                        draw_text.text_style.font_size: 13
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                SearchRow{
                                    video_search := TextInput{
                                        width: Fill
                                        empty_text: "search clips / 3d / tex"
                                    }
                                    video_category := TextInput{
                                        width: 100
                                        empty_text: "cat"
                                    }
                                    video_go := ChromeButton{text: "Go"}
                                    video_more := ChromeButton{text: "+"}
                                    video_count := PanelLabel{text: ""}
                                }
                            }

                            // ============ MUSIC ============
                            music_page := View{
                                width: Fill
                                height: Fill
                                flow: Down
                                spacing: 8
                                View{
                                    width: Fill
                                    height: Fill
                                    flow: Right
                                    spacing: 10
                                    deck_a_panel := View{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        spacing: 6
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 8
                                            align: Align{x: 0.0, y: 0.5}
                                            Label{
                                                text: "A"
                                                draw_text.color: #x3ee0b0
                                                draw_text.text_style: theme.font_bold{font_size: 11}
                                            }
                                            deck_a_title := ValueLabel{text: "empty"}
                                            View{width: Fill height: 1}
                                            deck_a_time := PanelLabel{text: "0:00 / 0:00"}
                                        }
                                        deck_a_zone := DeckWell{
                                            height: Fill
                                            cursor: MouseCursor.Hand
                                            deck_a_wave := Image{
                                                width: Fill
                                                height: Fill
                                            }
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            align: Align{x: 0.0, y: 0.5}
                                            deck_a_play := ChromeButton{text: "Play"}
                                            deck_a_loop := Toggle{text: "loop"}
                                            deck_a_mute := Toggle{text: "mute"}
                                        }
                                        deck_a_gain := Slider{
                                            width: Fill
                                            text: "gain"
                                            min: 0.0
                                            max: 1.5
                                            default: 1.0
                                        }
                                    }
                                    View{
                                        width: 200
                                        height: Fill
                                        flow: Down
                                        spacing: 8
                                        align: Align{x: 0.5, y: 0.0}
                                        Label{
                                            text: "MIX"
                                            draw_text.color: #x7a8794
                                            draw_text.text_style: theme.font_bold{font_size: 9}
                                        }
                                        PanelLabel{text: "A  ↔  B"}
                                        xfader := Slider{
                                            width: Fill
                                            min: 0.0
                                            max: 1.0
                                            default: 0.0
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            fade_to_a := ChromeButton{text: "◀ A"}
                                            fade_to_b := ChromeButton{text: "B ▶"}
                                        }
                                        xfade_secs := Slider{
                                            width: Fill
                                            text: "fade"
                                            min: 0.05
                                            max: 20.0
                                            default: 4.0
                                        }
                                        xcurve := DropDown{labels: ["Equal power" "Linear"]}
                                        decks_swap := ChromeButton{text: "Swap"}
                                    }
                                    deck_b_panel := View{
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        spacing: 6
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 8
                                            align: Align{x: 0.0, y: 0.5}
                                            Label{
                                                text: "B"
                                                draw_text.color: #x6aa8ff
                                                draw_text.text_style: theme.font_bold{font_size: 11}
                                            }
                                            deck_b_title := ValueLabel{text: "empty"}
                                            View{width: Fill height: 1}
                                            deck_b_time := PanelLabel{text: "0:00 / 0:00"}
                                        }
                                        deck_b_zone := DeckWell{
                                            height: Fill
                                            cursor: MouseCursor.Hand
                                            deck_b_wave := Image{
                                                width: Fill
                                                height: Fill
                                            }
                                        }
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            align: Align{x: 0.0, y: 0.5}
                                            deck_b_play := ChromeButton{text: "Play"}
                                            deck_b_loop := Toggle{text: "loop"}
                                            deck_b_mute := Toggle{text: "mute"}
                                        }
                                        deck_b_gain := Slider{
                                            width: Fill
                                            text: "gain"
                                            min: 0.0
                                            max: 1.5
                                            default: 1.0
                                        }
                                    }
                                }
                                SearchRow{
                                    music_search := TextInput{
                                        width: Fill
                                        empty_text: "search music…"
                                    }
                                    music_category := TextInput{
                                        width: 120
                                        text: "music"
                                    }
                                    music_go := ChromeButton{text: "Search"}
                                    music_more := ChromeButton{text: "More"}
                                    music_count := PanelLabel{text: ""}
                                    PanelLabel{text: "load"}
                                    deck_target := DropDown{labels: ["Auto" "Deck A" "Deck B"]}
                                }
                                music_grid := VjTileGrid{
                                    height: 240
                                }
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
                                    color: #x0c1014
                                    border_color: #xffffff10
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
                                        draw_text.color: #x3ee0b0
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
                                    gen_go := ChromeButton{text: "Queue"}
                                }
                                gen_status := PanelLabel{text: ""}
                                gen_jobs := VjJobList{}
                            }
                        }
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
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 8
                            padding: Inset{left: 8.0 right: 8.0 top: 4.0 bottom: 4.0}
                            align: Align{x: 0.0, y: 0.5}
                            out_now := Label{
                                width: Fill
                                text: ""
                                draw_text.color: #x66727f
                                draw_text.text_style.font_size: 9
                            }
                        }
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
    Billboard,
}

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

/// What a catalog-runtime request was for.
#[derive(Clone, Debug)]
enum CatPurpose {
    Page { surface: Surface, gen: CatGen, slot: usize, first: bool },
    Detail { surface: Surface, gen: CatGen, asset: AssetId },
    Manifest { surface: Surface, gen: CatGen, asset: AssetId, revision: AssetRevisionId },
    Thumb { revision: AssetRevisionId },
    JobProfiles,
    JobEnqueue { tag: GenTag },
    JobStatus { job: JobId },
    JobCancel { job: JobId },
}

/// What a media-lane request was for (keyed by `(lane, request)`).
#[derive(Clone, Debug)]
enum MediaPurpose {
    Cue { gen: CueGen },
    Deck { deck: DeckId, gen: u64, media: MediaType },
    Pad { pad: AssetId, gen: u64, revision: AssetRevisionId, media: MediaType },
    Mesh { gen: u64 },
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

/// Published by the sync worker; read by the UI thread each pump.
#[derive(Clone, Default)]
pub struct SyncSnapshot {
    pub sample_rate: u32,
    pub frames: u64,
    pub dropped: u64,
    pub peak: f32,
    pub lock_state: BeatLockState,
    pub beat: Option<BeatInfo>,
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
}

impl SyncWorker {
    pub fn start(feed: Arc<CaptureFeed>) -> SyncWorker {
        let shared = Arc::new(SyncShared { snap: Mutex::new(SyncSnapshot::default()) });
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_shared = shared.clone();
        let thread_stop = stop.clone();
        let _ = std::thread::Builder::new().name("vj-beat-sync".into()).spawn(move || {
            let mut scratch: Vec<f32> = Vec::with_capacity(CAPTURE_RING);
            let mut analyzer: Option<BeatSyncAnalyzer> = None;
            let mut lock_started_beat: Option<i64> = None;
            let mut seen_dropped: u64 = 0;
            while !thread_stop.load(Ordering::Acquire) {
                scratch.clear();
                let rate = feed.drain_into(&mut scratch);
                if rate >= 8_000 && !scratch.is_empty() {
                    // A device-rate change restarts the analyzer: its whole
                    // grid is in samples of one rate.
                    let stale = analyzer
                        .as_ref()
                        .is_some_and(|a| (a.sample_rate() - rate as f64).abs() > 0.5);
                    if stale {
                        analyzer = None;
                        lock_started_beat = None;
                    }
                    analyzer
                        .get_or_insert_with(|| BeatSyncAnalyzer::new(rate as f64))
                        .push_mono(&scratch);
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
                let analyzer_snapshot = analyzer.as_ref().map(BeatSyncAnalyzer::snapshot);
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
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        SyncWorker { shared, stop }
    }

    pub fn snapshot(&self) -> SyncSnapshot {
        self.shared.snap.lock().unwrap().clone()
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
const MAX_THUMB_TEXTURES: usize = 512;

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
    #[rust]
    connector: Option<SessionConnector>,
    #[rust]
    up: Option<SessionHandles>,
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
    #[rust(true)]
    gen_panel_open: bool,
    #[rust(300.0)]
    gen_panel_width: f64,

    // Pure engines.
    #[rust]
    local_lib: LocalLibrary,
    #[rust]
    local_thumbs_queued: Vec<AssetRevisionId>,
    #[rust]
    thumb_anims: HashMap<AssetRevisionId, (Vec<Texture>, f32)>,
    #[rust(BrowseModel::visual())]
    video_model: BrowseModel,
    #[rust(BrowseModel::new(AssetKind::Audio, "music"))]
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

    // Media lane plans (latest-click-wins with cancel-on-supersede).
    #[rust(LatestWins::video())]
    video_plan: LatestWins,
    #[rust(LatestWins::mesh())]
    mesh_plan: LatestWins,

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
    #[rust]
    thumb_order: VecDeque<AssetRevisionId>,
    #[rust]
    pcm_store: HashMap<AssetRevisionId, Arc<TrackPcm>>,

    // Video slots.
    #[rust]
    players: [Option<SlotPlayer>; 2],
    #[rust]
    slot_textures: [Option<Texture>; 2],
    #[rust]
    awaiting_preroll: [Option<CueGen>; 2],
    /// Settled program mix (0 = slot A on screen, 1 = slot B).
    #[rust]
    program_mix: f32,
    /// B composites over A instead of an A↔B crossfade.
    #[rust]
    overlay_mode: bool,
    #[rust]
    fx: FxState,
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
    #[rust(1.0f32)]
    fade_secs: f32,

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
    deck_wave_tex: [Option<Texture>; 2],
    #[rust([-1.0f64, -1.0f64])]
    deck_wave_drawn: [f64; 2],
    #[rust]
    deck_target: DeckTarget,
    #[rust(4.0f32)]
    xfade_secs: f32,

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

    // Grid rebuild flag + timers.
    #[rust]
    grids_dirty: bool,
    #[rust]
    poll_timer: Timer,
    #[rust]
    refresh_timer: Timer,
    #[rust]
    video_pump: NextFrame,
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

    fn clear_slot_mesh(&mut self, cx: &mut Cx, slot: SlotId) {
        let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
        if let Some(mut view) = widget.borrow_mut::<mesh_view::VjMeshView>() {
            view.clear(cx);
        };
    }

    fn apply_slot_mesh(&mut self, cx: &mut Cx, slot: SlotId, prepared: Box<media::PreparedMesh>) {
        let widget = self.ui.widget(cx, Self::slot_mesh_path(slot));
        if let Some(mut view) = widget.borrow_mut::<mesh_view::VjMeshView>() {
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

    fn sync_mix_mode_ui(&mut self, cx: &mut Cx) {
        let (mode, role) = if self.overlay_mode {
            ("OVER", "OVER")
        } else {
            ("MIX", "STANDBY")
        };
        self.ui.button(cx, ids!(mix_mode)).set_text(cx, mode);
        self.ui.label(cx, ids!(slot_b_mode)).set_text(cx, role);
    }

    fn sync_fx_ui(&mut self, cx: &mut Cx) {
        let info = self.fx.kind.info();
        self.ui.label(cx, ids!(fx_name)).set_text(cx, info.name);
        self.ui.label(cx, ids!(fx_p1_lab)).set_text(cx, info.p1);
        self.ui.label(cx, ids!(fx_p2_lab)).set_text(cx, info.p2);
        self.ui.slider(cx, ids!(fx_p1)).set_value(cx, self.fx.p1 as f64);
        self.ui.slider(cx, ids!(fx_p2)).set_value(cx, self.fx.p2 as f64);
        self.ui
            .check_box(cx, ids!(fx_beat1))
            .set_active(cx, self.fx.beat1, Animate::No);
        self.ui
            .check_box(cx, ids!(fx_beat2))
            .set_active(cx, self.fx.beat2, Animate::No);
        self.video_pump = cx.new_next_frame();
    }

    fn beat_phase_01(&self) -> f32 {
        let Some(beat) = self.current_beat() else {
            return 0.0;
        };
        if beat.period.is_zero() {
            return 0.0;
        }
        let now = Instant::now();
        let beat = extrapolate_beat(&beat, now);
        let until = beat.next_beat.saturating_duration_since(now).as_secs_f64();
        let period = beat.period.as_secs_f64().max(0.001);
        ((1.0 - until / period) as f32).clamp(0.0, 1.0)
    }

    fn set_visual_mix(&mut self, cx: &mut Cx, value: f32) {
        self.program_mix = value.clamp(0.0, 1.0);
        self.sync_xfader_ui(cx, self.program_mix);
        self.video_pump = cx.new_next_frame();
    }

    fn pump_billboards(&mut self, cx: &mut Cx) {
        let now = cx.seconds_since_app_start();
        for index in 0..2 {
            let Some(bb) = self.billboards[index].as_mut() else { continue };
            let Some(state) = bb.states.get(bb.state_i) else { continue };
            let last = bb.last.replace(now).unwrap_or(now);
            bb.accum += (now - last).min(0.25);
            let step = 1.0 / f64::from(state.fps.max(1.0));
            let looping = state.r#loop;
            let n = state.frames.len().max(1);
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

    fn live_billboard_slot(&self) -> Option<usize> {
        if self.program_mix > 0.5 && self.billboards[1].is_some() {
            Some(1)
        } else if self.billboards[0].is_some() {
            Some(0)
        } else if self.billboards[1].is_some() {
            Some(1)
        } else {
            None
        }
    }

    fn set_billboard_state(&mut self, cx: &mut Cx, name: &str) {
        let Some(index) = self.live_billboard_slot() else { return };
        let Some(bb) = self.billboards[index].as_mut() else { return };
        let Some(state_i) = bb.states.iter().position(|s| s.name == name) else {
            return;
        };
        bb.state_i = state_i;
        bb.frame_i = 0;
        bb.accum = 0.0;
        self.sync_bb_states_ui(cx);
        self.video_pump = cx.new_next_frame();
    }

    fn sync_bb_states_ui(&mut self, cx: &mut Cx) {
        let ids = [
            ids!(bb_s0),
            ids!(bb_s1),
            ids!(bb_s2),
            ids!(bb_s3),
            ids!(bb_s4),
            ids!(bb_s5),
            ids!(bb_s6),
            ids!(bb_s7),
        ];
        let names: Vec<String> = self
            .live_billboard_slot()
            .and_then(|i| self.billboards[i].as_ref())
            .map(|bb| {
                bb.states
                    .iter()
                    .map(|s| s.name.clone())
                    .take(ids.len())
                    .collect()
            })
            .unwrap_or_default();
        let active = self
            .live_billboard_slot()
            .and_then(|i| self.billboards[i].as_ref())
            .map(|bb| bb.state_i);
        for (i, id) in ids.iter().enumerate() {
            let button = self.ui.button(cx, *id);
            if let Some(name) = names.get(i) {
                button.set_visible(cx, true);
                button.set_text(cx, name);
                let color = if Some(i) == active {
                    vec4(0.24, 0.88, 0.69, 1.0)
                } else {
                    vec4(0.60, 0.65, 0.70, 1.0)
                };
                let mut widget = self.ui.widget(cx, *id);
                script_apply_eval!(cx, widget, {
                    draw_text +: { color: #(color) }
                });
            } else {
                button.set_text(cx, "");
                button.set_visible(cx, false);
            }
        }
    }

    fn sync_xfader_ui(&mut self, cx: &mut Cx, mix: f32) {
        self.ui
            .slider(cx, ids!(apc_xfader))
            .set_value(cx, mix.clamp(0.0, 1.0) as f64);
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
            if matches!(
                transition.phase,
                VideoTransitionPhase::Started | VideoTransitionPhase::Completed
            ) {
                let target = if transition.to == SlotId::B { 1.0 } else { 0.0 };
                let origin = match transition.from {
                    Some(SlotId::B) => 1.0,
                    Some(SlotId::A) => 0.0,
                    None => 1.0 - target,
                };
                mix = origin + (target - origin) * transition.progress;
                if transition.phase == VideoTransitionPhase::Completed {
                    self.program_mix = target;
                }
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
            // The video surface pages the pad-matrix window (filtered,
            // local-first), not the raw local+catalog lists.
            ApcSurface::Video => self.video_pad_total,
            ApcSurface::Music => {
                self.local_items(Surface::Music).len() + self.music_model.tiles().len()
            }
            ApcSurface::Sfx => {
                self.local_items(Surface::Sfx).len() + self.sfx_model.tiles().len()
            }
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
            ApcSurface::Music => self
                .local_items(Surface::Music)
                .get(index)
                .map(|item| item.asset)
                .or_else(|| {
                    let local = self.local_items(Surface::Music).len();
                    self.music_model.tiles().get(index.saturating_sub(local)).map(|t| t.asset)
                }),
            ApcSurface::Sfx => self
                .local_items(Surface::Sfx)
                .get(index)
                .map(|item| item.asset)
                .or_else(|| {
                    let local = self.local_items(Surface::Sfx).len();
                    self.sfx_model.tiles().get(index.saturating_sub(local)).map(|t| t.asset)
                }),
        }
    }

    fn show_apc_surface(&mut self, cx: &mut Cx) {
        let page = match self.apc.surface {
            ApcSurface::Video => id!(video_page),
            ApcSurface::Music => id!(music_page),
            ApcSurface::Sfx => id!(sfx_page),
        };
        self.ui.page_flip(cx, ids!(pages)).set_active_page(cx, page.into());
        self.paint_tabs(cx, page);
        self.ui.redraw(cx);
    }

    fn paint_tabs(&mut self, cx: &mut Cx, active: LiveId) {
        let on = vec4(0.24, 0.88, 0.69, 1.0);
        let off = vec4(0.60, 0.65, 0.70, 1.0);
        for (button, page) in [
            (ids!(tab_video), id!(video_page)),
            (ids!(tab_music), id!(music_page)),
            (ids!(tab_sfx), id!(sfx_page)),
            (ids!(tab_mesh), id!(mesh_page)),
        ] {
            let color = if page == active { on } else { off };
            let mut widget = self.ui.widget(cx, button);
            script_apply_eval!(cx, widget, {
                draw_text +: { color: #(color) }
            });
        }
        self.paint_gen_tab(cx);
    }

    fn paint_gen_tab(&mut self, cx: &mut Cx) {
        let color = if self.gen_panel_open {
            vec4(0.24, 0.88, 0.69, 1.0)
        } else {
            vec4(0.60, 0.65, 0.70, 1.0)
        };
        let mut widget = self.ui.widget(cx, ids!(tab_gen));
        script_apply_eval!(cx, widget, {
            draw_text +: { color: #(color) }
        });
        self.ui
            .button(cx, ids!(gen_fold))
            .set_text(cx, if self.gen_panel_open { "⟨" } else { "⟩" });
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

    /// The current beat estimate, if the sync worker holds one.
    fn current_beat(&self) -> Option<BeatInfo> {
        self.sync_worker.as_ref().and_then(|worker| worker.snapshot().beat)
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
                SlotMedia::Still | SlotMedia::Mesh | SlotMedia::Billboard
            ) {
                let cmds = self.cue.start_armed(gen, schedule);
                self.program_mix = if to == SlotId::B { 1.0 } else { 0.0 };
                self.run_cue_cmds(cx, cmds);
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
                self.program_mix = if snapshot.to == SlotId::B { 1.0 } else { 0.0 };
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
    fn apply_loop_fit(&mut self, slot: SlotId) {
        let index = slot.index();
        let beat = self.current_beat();
        let media_secs = self.players[index]
            .as_ref()
            .map(|player| player.duration_secs)
            .unwrap_or(0.0);
        let fit = self.slot_scan[index].and_then(|revision| {
            if !self.external_sync_enabled {
                return None;
            }
            let beat = beat.as_ref().filter(|b| b.locked && b.confidence >= CONF_MUSICAL)?;
            let report = self.loop_reports.get(&revision)?;
            let usable = report.detection.is_usable()
                && matches!(report.detection.kind, LoopKind::Wrap | LoopKind::PingPong)
                && report.detection.confidence >= LOOP_FIT_MIN_CONFIDENCE
                && loop_report_matches_media(*report, media_secs);
            if !usable {
                return None;
            }
            fit_loop_to_grid(report.period_secs, beat.bpm as f64, MAX_LOOP_RATE_DEVIATION)
                .filter(|fit| fit.within_rate_limit)
        });
        let target = fit.map(|fit| fit.playback_rate).unwrap_or(1.0);
        let Some(player) = self.players[index].as_mut() else { return };
        if (player.playback_rate() - target).abs() > 0.0015 {
            player.set_playback_rate(target);
        }
        self.applied_fit[index] = fit;
    }

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
                    ApcSurface::Video => self.video_tile_clicked(cx, asset),
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
                self.ui.slider(cx, ids!(master_slider)).set_value(cx, value as f64);
            }
            ApcAction::Crossfader(value) => {
                self.set_visual_mix(cx, value);
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
            // Local library items have no catalog tile and are always ready.
            let mut state = tile
                .map(|tile| match tile.state {
                    catalog::TileState::Ready => PadLed::Ready,
                    catalog::TileState::Failed(_) => PadLed::Failed,
                    catalog::TileState::Listed | catalog::TileState::Resolving => PadLed::Queued,
                })
                .unwrap_or(PadLed::Ready);
            match self.apc.surface {
                ApcSurface::Video => {
                    if self.cue.live().is_some_and(|item| item.asset == asset) {
                        state = PadLed::Live;
                    } else if self.cue.next().is_some_and(|item| item.asset == asset) {
                        state = PadLed::Queued;
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
        for message in self.apc_leds.update(frame) {
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
                    if let Ok(id) = up
                        .catalog
                        .submit(ClientRequest::CatalogSearch { query, cursor })
                    {
                        self.cat_reqs
                            .insert(id, CatPurpose::Page { surface, gen, slot, first });
                    }
                }
                CatCmd::FetchDetail { gen, asset } => {
                    if let Ok(id) = up.catalog.submit(ClientRequest::AssetDetail { id: asset }) {
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
                        self.grids_dirty = true;
                        continue;
                    }
                    // Publication cap: refuse to even download an oversized
                    // thumbnail; the tile keeps its placeholder.
                    if len > media::MAX_THUMB_BYTES {
                        continue;
                    }
                    if let Ok(id) = up.catalog.submit(ClientRequest::FetchBlob {
                        blob,
                        expected_len: Some(len),
                        pin: false,
                    }) {
                        self.cat_reqs.insert(id, CatPurpose::Thumb { revision });
                    }
                }
            }
        }
        self.grids_dirty = true;
    }

    fn run_gen_cmds(&mut self, cmds: Vec<GenCmd>) {
        let Some(up) = self.up.as_mut() else { return };
        for cmd in cmds {
            match cmd {
                GenCmd::FetchProfiles => {
                    if let Ok(id) = up.catalog.submit(ClientRequest::FetchJobProfiles {
                        domain: Some("video".to_string()),
                    }) {
                        self.cat_reqs.insert(id, CatPurpose::JobProfiles);
                    }
                }
                GenCmd::Enqueue { tag, namespace, kind, body } => {
                    if let Ok(id) =
                        up.catalog.submit(ClientRequest::EnqueueJob { namespace, kind, body })
                    {
                        self.cat_reqs.insert(id, CatPurpose::JobEnqueue { tag });
                    }
                }
                GenCmd::PollStatus { job } => {
                    if let Ok(id) = up.catalog.submit(ClientRequest::FetchJobStatus { job }) {
                        self.cat_reqs.insert(id, CatPurpose::JobStatus { job });
                    }
                }
                GenCmd::Cancel { job } => {
                    if let Ok(id) = up.catalog.submit(ClientRequest::CancelJob { job }) {
                        self.cat_reqs.insert(id, CatPurpose::JobCancel { job });
                    }
                }
            }
        }
    }

    fn run_cue_cmds(&mut self, cx: &mut Cx, cmds: Vec<CueCmd>) {
        for cmd in cmds {
            match cmd {
                CueCmd::FetchMedia { gen, item } => {
                    let (lane, cancel) = self.video_plan.begin();
                    if let (Some(up), Some(stale)) = (self.up.as_ref(), cancel) {
                        // Supersede: abort the older transfer ON ITS LANE.
                        if let Some(runtime) = up.media.get(stale.lane) {
                            runtime.cancel(stale.request);
                        }
                    }
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
                }
                CueCmd::OpenSlot { slot, gen, item, path } => {
                    // A fresh slot must never fade in showing the previous
                    // clip's last frame.
                    self.players[slot.index()] = None;
                    self.slot_textures[slot.index()] = None;
                    self.light_samples[slot.index()] = None;
                    self.light_analyzers[slot.index()].reset();
                    self.clear_slot_mesh(cx, slot);
                    self.slot_media[slot.index()] = SlotMedia::Empty;
                    self.billboards[slot.index()] = None;
                    self.awaiting_preroll[slot.index()] = None;
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
                        MediaType::Glb => {
                            // Silent mixer bus so the existing fade arm
                            // accepts a picture-only destination.
                            self.mixer.open_slot(slot);
                            self.mixer.set_slot_paused(slot, true);
                            self.slot_media[slot.index()] = SlotMedia::Mesh;
                            self.slot_aspect[slot.index()] = 16.0 / 9.0;
                            self.decode.submit(DecodeJob::SlotMesh {
                                gen,
                                slot: slot.index(),
                                path,
                            });
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
                                Ok(player) => {
                                    self.slot_media[slot.index()] = SlotMedia::Video;
                                    self.players[slot.index()] = Some(player);
                                    self.awaiting_preroll[slot.index()] = Some(gen);
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
                    // and surface the program.
                    if let Some(player) = self.players[to.index()].as_mut() {
                        player.set_paused(false);
                    }
                    if self.armed_fade.is_some_and(|armed| armed.schedule == schedule) {
                        self.armed_fade = None;
                    }
                    self.refresh_program_lighting();
                    self.video_pump = cx.new_next_frame();
                    self.show_output_page(cx, id!(video_out_page));
                }
                CueCmd::CloseSlot { slot } => {
                    // An armed fade whose slot goes away must never fire.
                    if self.armed_fade.is_some_and(|armed| armed.to == slot) {
                        self.armed_fade = None;
                    }
                    self.players[slot.index()] = None; // detached teardown
                    self.awaiting_preroll[slot.index()] = None;
                    self.slot_textures[slot.index()] = None;
                    self.slot_media[slot.index()] = SlotMedia::Empty;
                    self.billboards[slot.index()] = None;
                    self.clear_slot_mesh(cx, slot);
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
                    if let Some(local) = self.local_lib.get(&item.asset) {
                        self.decode.submit(DecodeJob::Deck {
                            deck,
                            gen,
                            path: local.path.clone(),
                            media: item.media,
                        });
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
                            self.deck_tracks[deck.index()] = Some((pcm, peaks));
                            self.deck_wave_drawn[deck.index()] = -1.0;
                        }
                    }
                }
                DeckCmd::SetPlaying { deck, playing } => self.mixer.set_deck_playing(deck, playing),
                DeckCmd::SeekFraction { deck, fraction } => {
                    self.mixer.seek_deck_fraction(deck, fraction);
                    self.deck_wave_drawn[deck.index()] = -1.0;
                }
                DeckCmd::SetLoop { deck, loop_on } => self.mixer.set_deck_loop(deck, loop_on),
                DeckCmd::SetMute { deck, muted } => self.mixer.set_deck_mute(deck, muted),
                DeckCmd::SetGain { deck, gain } => self.mixer.set_deck_gain(deck, gain),
                DeckCmd::SetCrossfader { position } => self.mixer.set_crossfader(position),
                DeckCmd::FadeCrossfader { position, secs } => {
                    self.mixer.fade_crossfader(position, secs)
                }
                DeckCmd::SetCurve { curve } => self.mixer.set_curve(curve),
                DeckCmd::SwapVoices => {
                    self.mixer.swap_decks();
                    self.deck_tracks.swap(0, 1);
                    self.deck_wave_tex.swap(0, 1);
                    self.deck_wave_drawn = [-1.0, -1.0];
                    // Re-bind the swapped wave textures to the panels.
                    for (deck, image) in
                        [(0usize, ids!(deck_a_wave)), (1usize, ids!(deck_b_wave))]
                    {
                        self.ui
                            .image(cx, image)
                            .set_texture(cx, self.deck_wave_tex[deck].clone());
                    }
                    self.sync_deck_controls(cx);
                }
            }
        }
    }

    /// Mirror engine deck state into the toggle/slider widgets (after swap,
    /// and at install) so the controls always show the deck they control.
    fn sync_deck_controls(&mut self, cx: &mut Cx) {
        for (deck, loop_id, mute_id, gain_id) in [
            (DeckId::A, ids!(deck_a_loop), ids!(deck_a_mute), ids!(deck_a_gain)),
            (DeckId::B, ids!(deck_b_loop), ids!(deck_b_mute), ids!(deck_b_gain)),
        ] {
            let state = self.decks.deck(deck);
            self.ui.check_box(cx, loop_id).set_active(cx, state.loop_on, Animate::No);
            self.ui.check_box(cx, mute_id).set_active(cx, state.muted, Animate::No);
            self.ui.slider(cx, gain_id).set_value(cx, state.gain as f64);
        }
        let pos = self.decks.crossfader as f64;
        self.ui.slider(cx, ids!(xfader)).set_value(cx, pos);
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
                    if let Some(local) = self.local_lib.get(&item.asset) {
                        self.decode.submit(DecodeJob::Pad {
                            pad,
                            gen,
                            revision: item.revision,
                            path: local.path.clone(),
                            media: item.media,
                        });
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
        self.retry_lighting_if_due();
        // Refresh the worker watchdog and, only while the physical button is
        // held, its shorter hazardous-output heartbeat.
        self.publish_lighting_controls();
        self.pump_apc40(cx);
        self.pump_session(cx);
        self.pump_subscriber();
        self.pump_catalog_runtime(cx);
        self.pump_media_lanes(cx);
        self.pump_decodes(cx);
        for deck in self.mixer.drain_ended_decks() {
            self.decks.track_ended(deck);
        }
        for voice in self.mixer.drain_ended_voices() {
            self.pads.voice_ended(voice);
        }
        // Slot pre-rolls (video only — stills/meshes complete from decode).
        for slot in [SlotId::A, SlotId::B] {
            let Some(gen) = self.awaiting_preroll[slot.index()] else { continue };
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
        if self.grids_dirty {
            self.grids_dirty = false;
            self.rebuild_grids(cx);
        }
        self.queue_visible_thumbs(cx);
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

    fn pump_subscriber(&mut self) {
        let Some(up) = self.up.as_mut() else { return };
        let events = up.subscriber.poll();
        for event in events {
            match event {
                CatalogSubscriptionEvent::Ready { .. } => {}
                CatalogSubscriptionEvent::Events { events, .. } => {
                    for ev in events {
                        self.video_model.event_touch(ev.content_kind);
                        self.music_model.event_touch(ev.content_kind);
                        self.sfx_model.event_touch(ev.content_kind);
                        self.mesh_model.event_touch(ev.content_kind);
                        // Publication marks matching generation rows —
                        // event-driven, never a whole-catalog poll.
                        if let Some(asset) = ev.asset_id {
                            self.gen.catalog_published(asset);
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
                    let Some(purpose) = self.cat_reqs.remove(&id) else { continue };
                    self.catalog_done(cx, purpose, output);
                }
                ClientEvent::Failed { error, .. } => {
                    let Some(purpose) = self.cat_reqs.remove(&id) else { continue };
                    match purpose {
                        CatPurpose::Page { surface, gen, .. } => {
                            self.model(surface).page_failed(gen, error.to_string());
                            self.grids_dirty = true;
                        }
                        CatPurpose::Detail { surface, gen, asset }
                        | CatPurpose::Manifest { surface, gen, asset, .. } => {
                            let cmds =
                                self.model(surface).resolve_failed(gen, asset, error.to_string());
                            self.run_cat_cmds(surface, cmds);
                        }
                        CatPurpose::Thumb { .. } => {}
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
                        asset: h.asset_id,
                        title: if h.title.is_empty() {
                            h.asset_id.to_string()
                        } else {
                            h.title
                        },
                        alias: h.alias.map(|a| a.as_str().to_string()),
                        live: h.live,
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
                    Surface::Video => select_visual_file(&manifest),
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
                let thumb = manifest
                    .thumbnail
                    .as_ref()
                    .map(|t| TileThumb { blob: t.blob, len: t.byte_len });
                let cmds =
                    self.model(surface).manifest_arrived(gen, asset, revision, media, thumb);
                self.run_cat_cmds(surface, cmds);
                if surface == Surface::Sfx {
                    self.sync_pads();
                }
            }
            (CatPurpose::Thumb { revision }, ClientOutput::Blob { path, .. }) => {
                // Decode on the worker pool; only the finished BGRA pixels
                // come back to this thread.
                self.decode.submit(DecodeJob::Thumb { revision, path });
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
                self.gen.status_arrived_at(&status, now_ms());
            }
            (CatPurpose::JobCancel { job }, ClientOutput::JobCancelled(count)) => {
                self.gen.cancel_confirmed_at(job, count, Some(now_ms()));
            }
            _ => {}
        }
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
                                    let cmds = self.cue.media_ready(gen, path);
                                    self.run_cue_cmds(cx, cmds);
                                }
                            }
                            MediaPurpose::Deck { deck, gen, media } => {
                                self.decode.submit(DecodeJob::Deck { deck, gen, path, media });
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
                        match purpose {
                            MediaPurpose::Cue { gen } => {
                                if self.video_plan.finished(lane, id) {
                                    let cmds = self.cue.media_failed(gen, error.to_string());
                                    self.run_cue_cmds(cx, cmds);
                                }
                            }
                            MediaPurpose::Deck { deck, gen, .. } => {
                                let cmds = self.decks.track_failed(deck, gen, error.to_string());
                                self.run_deck_cmds(cx, cmds);
                            }
                            MediaPurpose::Pad { pad, gen, .. } => {
                                let cmds = self.pads.load_failed(pad, gen, error.to_string());
                                self.run_pad_cmds(cmds);
                            }
                            MediaPurpose::Mesh { gen } => {
                                if self.mesh_plan.finished(lane, id) && gen == self.mesh_gen {
                                    self.set_mesh_status(
                                        cx,
                                        &format!("mesh fetch failed: {error}"),
                                    );
                                }
                            }
                        }
                        self.grids_dirty = true;
                    }
                }
            }
        }
    }

    fn pump_decodes(&mut self, cx: &mut Cx) {
        for done in self.decode.poll() {
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
                DecodeDone::SlotMesh { gen, slot, result } => {
                    let slot = if slot == 0 { SlotId::A } else { SlotId::B };
                    if !self.cue.preroll_current(slot, gen) {
                        continue; // superseded click owns this slot now
                    }
                    match result {
                        Ok(prepared) => {
                            self.apply_slot_mesh(cx, slot, prepared);
                            self.slot_media[slot.index()] = SlotMedia::Mesh;
                            self.slot_aspect[slot.index()] = 16.0 / 9.0;
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
                DecodeDone::Billboard { gen, slot, result } => {
                    let slot = if slot == 0 { SlotId::A } else { SlotId::B };
                    if !self.cue.preroll_current(slot, gen) {
                        continue; // superseded click owns this slot now
                    }
                    match result {
                        Ok(prepared) => {
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
                            self.sync_bb_states_ui(cx);
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
                            self.slot_textures[slot.index()] = Some(Texture::new_with_format(
                                cx,
                                TextureFormat::VecBGRAu8_32 {
                                    width: w,
                                    height: h,
                                    data: Some(bgra),
                                    updated: TextureUpdated::Full,
                                },
                            ));
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
                        if self.thumbs.insert(revision, texture.clone()).is_none() {
                            self.thumb_order.push_back(revision);
                        }
                        if frames.len() > 1 {
                            self.thumb_anims
                                .insert(revision, (frames.clone(), thumb.fps));
                        }
                        self.apply_thumb(cx, revision, texture, frames, thumb.fps);
                        while self.thumb_order.len() > MAX_THUMB_TEXTURES {
                            if let Some(old) = self.thumb_order.pop_front() {
                                if self.thumbs.contains_key(&old) && old != revision {
                                    self.thumbs.remove(&old);
                                    self.thumb_anims.remove(&old);
                                    self.local_thumbs_queued.retain(|queued| *queued != old);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- UI sync ------------------------------------------------------------

    fn set_output_window_status(&self, cx: &mut Cx, text: &str) {
        self.ui
            .label(cx, ids!(output_window_status))
            .set_text(cx, text);
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
                self.set_output_window_status(cx, "output closing…");
            }
            Event::WindowClosed(ev) if ev.window_id == output_id => {
                self.output_window_lifecycle = OutputWindowLifecycle::Closed;
                self.set_output_window_status(cx, "output closed — click OPEN OUTPUT");
            }
            Event::WindowGeomChange(ev) if ev.window_id == output_id => {
                self.remember_output_window_geometry(cx, output_id, &ev.new_geom);
                if self.output_window_lifecycle == OutputWindowLifecycle::Opening {
                    self.output_window_lifecycle = OutputWindowLifecycle::Open;
                    self.set_output_window_status(cx, "output open");
                }
            }
            Event::WindowGotFocus(window_id) if *window_id == output_id => {
                self.output_window_lifecycle = OutputWindowLifecycle::Open;
                self.set_output_window_status(cx, "output open");
            }
            _ => {}
        }
    }

    fn open_output_window(&mut self, cx: &mut Cx) {
        let output = self.ui.window(cx, ids!(output_window));
        let Some(window_id) = output.window_id() else {
            self.set_output_window_status(cx, "output unavailable");
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
                    self.set_output_window_status(cx, "output opening…");
                    output.redraw(cx);
                }
            }
            Some(OutputWindowCommand::Deminiaturize) => {
                cx.push_unique_platform_op(
                    makepad_widgets::makepad_platform::CxOsOp::Deminiaturize(window_id),
                );
                self.set_output_window_status(cx, "output open");
            }
            Some(OutputWindowCommand::Restore) => {
                cx.push_unique_platform_op(
                    makepad_widgets::makepad_platform::CxOsOp::RestoreWindow(window_id),
                );
                self.set_output_window_status(cx, "output open");
            }
            None => self.set_output_window_status(cx, "output opening…"),
        }
    }

    fn show_output_page(&mut self, cx: &mut Cx, page: LiveId) {
        self.ui.page_flip(cx, ids!(out_pages)).set_active_page(cx, page);
        self.ui.redraw(cx);
    }

    fn set_mesh_status(&mut self, cx: &mut Cx, text: &str) {
        self.ui.label(cx, ids!(mesh_status)).set_text(cx, text);
    }

    fn sync_gen_profiles(&mut self, cx: &mut Cx) {
        let labels = crate::gen::GenModel::pipe_labels();
        if labels != self.gen_profile_labels {
            self.gen_profile_labels = labels.clone();
            self.ui.drop_down(cx, ids!(gen_profile)).set_labels(cx, labels);
        }
    }

    fn sync_video_pad_window(&mut self, cx: &mut Cx) {
        let widget = self.ui.widget(cx, ids!(video_grid));
        let Some(pads) = widget.borrow::<VjPadMatrix>() else { return };
        self.apc.bank = pads.bank;
        self.video_pad_total = pads.len();
        self.video_pad_assets.clear();
        self.video_pad_assets
            .extend((0..40).map(|pad| pads.visible_at(pad).map(|entry| entry.asset)));
    }

    /// Mirror the sfx surface's ready tiles into the pad engine.
    fn sync_pads(&mut self) {
        let mut keep = Vec::new();
        let mut items: Vec<PadItem> = self
            .local_lib
            .filtered(|i| i.is_sfx(), self.sfx_model.text.as_str())
            .into_iter()
            .map(|local| {
                keep.push(local.asset);
                PadItem {
                    asset: local.asset,
                    revision: local.revision,
                    title: local.title.clone(),
                    media_blob: local.blob,
                    media_len: local.len,
                    media: local.media,
                }
            })
            .collect();
        items.extend(self.sfx_model.tiles().iter().filter_map(|t| {
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
        }));
        for item in items {
            self.pads.upsert_pad(item);
        }
        let cmds = self.pads.retain_pads(&keep);
        self.run_pad_cmds(cmds);
    }

    fn local_query(&self, surface: Surface) -> &str {
        match surface {
            Surface::Video => self.video_model.text.as_str(),
            Surface::Music => self.music_model.text.as_str(),
            Surface::Sfx => self.sfx_model.text.as_str(),
            Surface::Mesh => self.mesh_model.text.as_str(),
        }
    }

    fn local_items(&self, surface: Surface) -> Vec<&LocalItem> {
        let q = self.local_query(surface);
        match surface {
            Surface::Video => self.local_lib.filtered(|i| i.is_visual(), q),
            Surface::Music => self.local_lib.filtered(|i| i.is_music(), q),
            Surface::Sfx => self.local_lib.filtered(|i| i.is_sfx(), q),
            Surface::Mesh => self.local_lib.filtered(|i| i.is_mesh(), q),
        }
    }

    fn queue_visible_thumbs(&mut self, cx: &mut Cx) {
        let mut want: Vec<AssetRevisionId> = Vec::new();
        let video = self.ui.widget(cx, ids!(video_grid));
        if let Some(pads) = video.borrow::<VjPadMatrix>() {
            for asset in pads.visible_assets() {
                if let Some(item) = self.local_lib.get(&asset) {
                    want.push(item.revision);
                }
            }
        }
        for surface in [Surface::Music, Surface::Sfx, Surface::Mesh] {
            for item in self.local_items(surface).into_iter().take(48) {
                want.push(item.revision);
            }
        }
        for revision in want {
            if self.thumbs.contains_key(&revision) {
                continue;
            }
            if self.local_thumbs_queued.contains(&revision) {
                continue;
            }
            let Some(item) = self
                .local_lib
                .items
                .iter()
                .find(|item| item.revision == revision)
            else {
                continue;
            };
            let Some(path) = item.thumb.clone() else { continue };
            self.local_thumbs_queued.push(revision);
            self.decode.submit(DecodeJob::Thumb { revision, path });
        }
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
            .local_lib
            .items
            .iter()
            .find(|item| item.revision == revision)
            .map(|item| item.asset)
            .or_else(|| {
                self.video_model
                    .tiles()
                    .iter()
                    .chain(self.music_model.tiles())
                    .chain(self.sfx_model.tiles())
                    .chain(self.mesh_model.tiles())
                    .find(|tile| tile.revision == Some(revision))
                    .map(|tile| tile.asset)
            })
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
        for (index, item) in self.local_items(surface).into_iter().enumerate() {
            let texture = self.thumbs.get(&item.revision).cloned();
            let state = match surface {
                Surface::Sfx => match self.pads.pad(&item.asset).map(|p| p.load.clone()) {
                    Some(pads::PadLoad::Ready) => "ready".to_string(),
                    Some(pads::PadLoad::Loading { .. }) => "loading".to_string(),
                    Some(pads::PadLoad::Failed { .. }) => "failed".to_string(),
                    _ => "SFX".to_string(),
                },
                _ => match item.media {
                    MediaType::Glb => "3D".to_string(),
                    MediaType::Png | MediaType::Jpeg => "TEX".to_string(),
                    MediaType::Mp4 => "VID".to_string(),
                    MediaType::Wav | MediaType::Ogg => "SFX".to_string(),
                    MediaType::Text => "BB".to_string(),
                    _ => "ready".to_string(),
                },
            };
            let active = match surface {
                Surface::Video => {
                    self.cue.live().map(|i| i.asset) == Some(item.asset)
                        || self.cue.next().map(|i| i.asset) == Some(item.asset)
                }
                Surface::Sfx => self.pads.playing_voices(&item.asset) > 0,
                _ => false,
            };
            let (frames, fps) = self
                .thumb_anims
                .get(&item.revision)
                .cloned()
                .unwrap_or((Vec::new(), 0.0));
            entries.push(GridEntry {
                asset: item.asset,
                title: item.title.clone(),
                sub: item.domain.clone(),
                state,
                pad: format!("{:02}", index + 1),
                texture,
                frames,
                fps,
                active,
            });
        }
        let model = match surface {
            Surface::Video => &self.video_model,
            Surface::Music => &self.music_model,
            Surface::Sfx => &self.sfx_model,
            Surface::Mesh => &self.mesh_model,
        };
        let local_len = entries.len();
        entries.extend(model.tiles().iter().enumerate().map(|(index, tile)| {
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
                        Some(MediaType::Png) | Some(MediaType::Jpeg) => "TEX".to_string(),
                        Some(MediaType::Mp4) => "VID".to_string(),
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
                    Surface::Video => {
                        self.cue.live().map(|i| i.asset) == Some(tile.asset)
                            || self.cue.next().map(|i| i.asset) == Some(tile.asset)
                    }
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
                GridEntry {
                    asset: tile.asset,
                    title: tile.title.clone(),
                    sub,
                    state,
                    pad: format!("{:02}", local_len + index + 1),
                    texture,
                    frames,
                    fps,
                    active,
                }
            }));
        entries
    }

    fn rebuild_grids(&mut self, cx: &mut Cx) {
        let video_entries = self.grid_entries(Surface::Video);
        let video = self.ui.widget(cx, ids!(video_grid));
        if let Some(mut pads) = video.borrow_mut::<VjPadMatrix>() {
            pads.set_entries(cx, video_entries);
            pads.set_offset(cx, self.apc.bank);
            self.apc.bank = pads.bank;
        }
        self.sync_video_pad_window(cx);
        self.queue_visible_thumbs(cx);
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
            (Surface::Video, ids!(video_count)),
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
            let local_n = self.local_items(surface).len();
            let mut text = if local_n > 0 {
                format!("{} local", local_n)
            } else {
                format!("{} / {}", model.tiles().len(), model.total)
            };
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
        let rows = self
            .gen
            .jobs()
            .map(|job| JobRowEntry::from_job(job, now))
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
        self.ui.label(cx, ids!(status_label)).set_text(cx, &self.status_text);
        self.ui.label(cx, ids!(show_status_label)).set_text(
            cx,
            &format!("{} · {}", self.lighting_status, self.midi_status),
        );
        // Video program labels + position mirror.
        let now = self
            .cue
            .live()
            .map(|i| i.title.clone())
            .unwrap_or_else(|| "—".to_string());
        let next = self
            .cue
            .next()
            .map(|i| i.title.clone())
            .unwrap_or_else(|| "—".to_string());
        self.ui.label(cx, ids!(now_label)).set_text(cx, &now);
        self.ui.label(cx, ids!(next_label)).set_text(
            cx,
            &if next == "—" {
                "standby —".to_string()
            } else {
                format!("standby  {next}")
            },
        );
        let live_slot = self.cue.live_slot();
        let (a_role, b_role) = match live_slot {
            Some(SlotId::A) => ("A  LIVE", "B  NEXT"),
            Some(SlotId::B) => ("A  NEXT", "B  LIVE"),
            None => ("A", "B"),
        };
        self.ui.label(cx, ids!(slot_a_role)).set_text(cx, a_role);
        self.ui.label(cx, ids!(slot_b_role)).set_text(cx, b_role);
        self.ui.label(cx, ids!(apc_map_label)).set_text(cx, "");
        self.ui
            .label(cx, ids!(video_error))
            .set_text(cx, self.cue.last_error().unwrap_or(""));
        let out_line = match self.cue.live() {
            Some(item) => format!("NOW {}   NEXT {}", item.title, next),
            None => String::new(),
        };
        self.ui.label(cx, ids!(out_now)).set_text(cx, &out_line);
        if let Some(slot) = self.cue.live_slot() {
            if let Some(player) = self.players[slot.index()].as_ref() {
                let (pos, dur) = (player.position_secs(), player.duration_secs);
                let text = format!("{} / {}", format_time(pos), format_time(dur));
                self.ui.label(cx, ids!(video_pos_label)).set_text(cx, &text);
                // Keep the slider mirroring playback (not label-only).
                let fraction = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 };
                self.ui.slider(cx, ids!(video_pos)).set_value(cx, fraction);
                let paused = player.is_paused();
                self.ui
                    .button(cx, ids!(video_play))
                    .set_text(cx, if paused { "Play" } else { "Pause" });
            }
        }
        self.ui.label(cx, ids!(sfx_voices)).set_text(
            cx,
            &format!("voices {}", self.pads.voice_count()),
        );

        // Deck panels.
        for (deck, title_id, time_id, play_id) in [
            (DeckId::A, ids!(deck_a_title), ids!(deck_a_time), ids!(deck_a_play)),
            (DeckId::B, ids!(deck_b_title), ids!(deck_b_time), ids!(deck_b_play)),
        ] {
            let state = self.decks.deck(deck);
            let name = match deck {
                DeckId::A => "Deck A",
                DeckId::B => "Deck B",
            };
            let title = match &state.load {
                DeckLoad::Empty => format!("{name} — empty"),
                DeckLoad::Loading { item, .. } => format!("{name} — loading {}", item.title),
                DeckLoad::Loaded { item } => format!("{name} — {}", item.title),
                DeckLoad::Failed { item, error } => {
                    format!("{name} — {} failed: {error}", item.title)
                }
            };
            self.ui.label(cx, title_id).set_text(cx, &title);
            let (pos, dur, playing) = self.mixer.deck_position(deck);
            self.ui
                .label(cx, time_id)
                .set_text(cx, &format!("{} / {}", format_time(pos), format_time(dur)));
            self.ui
                .button(cx, play_id)
                .set_text(cx, if playing { "Pause" } else { "Play" });
            self.update_deck_wave(cx, deck, pos, dur);
        }

        // Selected pad strip.
        let sel = match self.selected_pad.and_then(|k| self.pads.pad(&k)) {
            Some(pad) => format!("pad: {}", pad.item.title),
            None => "pad: —".to_string(),
        };
        self.ui.label(cx, ids!(sfx_sel)).set_text(cx, &sel);

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
        let lock_text = match snap.lock_state {
            BeatLockState::Unlocked => "LOCK: UNLOCKED",
            BeatLockState::Acquiring => "LOCK: ACQUIRING",
            BeatLockState::Locked => "LOCK: LOCKED",
            BeatLockState::Holdover => "LOCK: HOLDOVER",
            BeatLockState::Lost => "LOCK: LOST",
        };
        let bpm_text = snap
            .beat
            .as_ref()
            .map(|beat| format!("{:>7.1} BPM", beat.bpm))
            .unwrap_or_else(|| "  ---.- BPM".to_string());
        let confidence_text = format!(
            "CONF: {:>3.0}%",
            snap.beat
                .as_ref()
                .map(|beat| beat.confidence * 100.0)
                .unwrap_or(0.0)
                .clamp(0.0, 100.0)
        );
        let phase_text = match snap.beat.as_ref() {
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

        self.ui.label(cx, ids!(external_capture)).set_text(cx, &capture_text);
        self.ui.label(cx, ids!(external_lock)).set_text(cx, lock_text);
        self.ui.label(cx, ids!(external_bpm)).set_text(cx, &bpm_text);
        self.ui
            .label(cx, ids!(external_confidence))
            .set_text(cx, &confidence_text);
        self.ui.label(cx, ids!(external_phase)).set_text(cx, &phase_text);

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
        self.ui
            .label(cx, ids!(external_video_state))
            .set_text(cx, &video_state);
        self.ui
            .label(cx, ids!(external_loop_state))
            .set_text(cx, &loop_text);
    }

    fn update_deck_wave(&mut self, cx: &mut Cx, deck: DeckId, pos: f64, dur: f64) {
        let index = deck.index();
        let Some((_pcm, peaks)) = self.deck_tracks[index].as_ref() else {
            return;
        };
        let fraction = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 };
        if (fraction - self.deck_wave_drawn[index]).abs() < 0.002 {
            return;
        }
        self.deck_wave_drawn[index] = fraction;
        const W: usize = 560;
        const H: usize = 84;
        let bgra = media::waveform_bgra(peaks, W, H, fraction);
        match &self.deck_wave_tex[index] {
            Some(tex) => tex.set_data_u32(cx, W, H, bgra),
            None => {
                let tex = Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 {
                        width: W,
                        height: H,
                        data: Some(bgra),
                        updated: TextureUpdated::Full,
                    },
                );
                self.deck_wave_tex[index] = Some(tex.clone());
                let image = match deck {
                    DeckId::A => ids!(deck_a_wave),
                    DeckId::B => ids!(deck_b_wave),
                };
                self.ui.image(cx, image).set_texture(cx, Some(tex));
            }
        }
        let image = match deck {
            DeckId::A => ids!(deck_a_wave),
            DeckId::B => ids!(deck_b_wave),
        };
        self.ui.image(cx, image).redraw(cx);
    }

    // ---- video frame pump ---------------------------------------------------

    fn pump_video(&mut self, cx: &mut Cx) {
        // Device-clock transition confirmations run at display-frame rate
        // for tight picture starts (the 20 Hz poll is only a fallback).
        self.pump_transitions(cx);
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
                match &self.slot_textures[index] {
                    Some(tex) => tex.set_data_u32(cx, w, h, frame),
                    None => {
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
            }
        }
        self.pump_billboards(cx);
        // The program mix mirrors the device-clock transition exactly: the
        // audio gains and this visual mix advance from one sample counter,
        // so what you see crossfades in lockstep with what you hear.
        let mix = self.live_program_mix();
        self.sync_xfader_ui(cx, mix);
        self.publish_program_lighting(mix);
        let mesh_a = self.slot_mesh_source(cx, SlotId::A);
        let mesh_b = self.slot_mesh_source(cx, SlotId::B);
        let source = |index: usize,
                      kind: SlotMedia,
                      mesh: Option<(Texture, f32)>,
                      players: &[Option<SlotPlayer>; 2],
                      textures: &[Option<Texture>; 2],
                      aspects: &[f32; 2]| {
            if kind == SlotMedia::Mesh {
                return mesh;
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
            &self.players,
            &self.slot_textures,
            &self.slot_aspect,
        );
        let b = source(
            1,
            self.slot_media[1],
            mesh_b,
            &self.players,
            &self.slot_textures,
            &self.slot_aspect,
        );
        let overlay = self.overlay_mode;
        let fx = self.fx;
        let beat = self.beat_phase_01();
        let time = cx.seconds_since_app_start() as f32;
        for target in [ids!(program), ids!(preview)] {
            let widget = self.ui.widget(cx, target);
            let borrow = widget.borrow_mut::<views::VideoProgram>();
            if let Some(mut program) = borrow {
                program.set_sources(cx, a.clone(), b.clone(), mix, overlay);
                program.set_fx(cx, fx, beat, time);
            }
        }
        if let Some(mut deck) = self
            .ui
            .widget(cx, ids!(preview_a))
            .borrow_mut::<views::VideoProgram>()
        {
            deck.set_sources(cx, a.clone(), None, 0.0, false);
        }
        if let Some(mut deck) = self
            .ui
            .widget(cx, ids!(preview_b))
            .borrow_mut::<views::VideoProgram>()
        {
            deck.set_sources(cx, b.clone(), None, 0.0, false);
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
            || self.fx.kind.0 != 0
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
    ) -> (Vec<AssetId>, Vec<AssetId>) {
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
                if cell.finger_down(actions).is_some() {
                    if let Some(entry) =
                        widget.borrow::<VjPadMatrix>().and_then(|g| g.visible_at(pad).cloned())
                    {
                        down.push(entry.asset);
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

    fn video_tile_clicked(&mut self, cx: &mut Cx, asset: AssetId) {
        if let Some(local) = self.local_lib.get(&asset).cloned() {
            self.play_local_visual(cx, local);
            return;
        }
        let Some(tile) = self.video_model.tile(&asset) else { return };
        let (Some(revision), Some(media)) = (tile.revision, tile.media.clone()) else {
            return;
        };
        let item = CueItem {
            asset,
            revision,
            title: tile.title.clone(),
            media_blob: media.blob,
            media_len: media.len,
            media: media.media,
        };
        let cmds = self.cue.click(item);
        self.run_cue_cmds(cx, cmds);
        self.grids_dirty = true;
    }

    fn play_local_visual(&mut self, cx: &mut Cx, local: LocalItem) {
        let item = CueItem {
            asset: local.asset,
            revision: local.revision,
            title: local.title.clone(),
            media_blob: local.blob,
            media_len: local.len,
            media: local.media,
        };
        let cmds = self.cue.click(item);
        let mut follow = Vec::new();
        for cmd in cmds {
            match cmd {
                CueCmd::FetchMedia { gen, .. } => {
                    follow.extend(self.cue.media_ready(gen, local.path.clone()));
                }
                other => follow.push(other),
            }
        }
        self.run_cue_cmds(cx, follow);
        self.grids_dirty = true;
    }

    fn music_tile_clicked(&mut self, cx: &mut Cx, asset: AssetId) {
        if let Some(local) = self.local_lib.get(&asset).cloned() {
            let item = TrackItem {
                asset: local.asset,
                revision: local.revision,
                title: local.title,
                media_blob: local.blob,
                media_len: local.len,
                media: local.media,
            };
            let cmds = self.decks.click(item, self.deck_target);
            self.run_deck_cmds(cx, cmds);
            self.grids_dirty = true;
            return;
        }
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
        };
        let cmds = self.decks.click(item, self.deck_target);
        self.run_deck_cmds(cx, cmds);
        self.grids_dirty = true;
    }

    fn mesh_tile_clicked(&mut self, cx: &mut Cx, asset: AssetId) {
        if let Some(local) = self.local_lib.get(&asset).cloned() {
            self.mesh_gen += 1;
            self.mesh_now = local.title.clone();
            let gen = self.mesh_gen;
            self.decode.submit(DecodeJob::MeshPrep {
                gen,
                path: local.path,
            });
            self.set_mesh_status(cx, &format!("loading {}…", local.title));
            return;
        }
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

    fn wave_seek(&mut self, cx: &mut Cx, actions: &Actions, deck: DeckId, zone: &[LiveId]) {
        let view = self.ui.view(cx, zone);
        let mut fraction = None;
        if let Some(fe) = view.finger_down(actions) {
            fraction = Some(((fe.abs.x - fe.rect.pos.x) / fe.rect.size.x.max(1.0)).clamp(0.0, 1.0));
        }
        if let Some(fe) = view.finger_move(actions) {
            fraction = Some(((fe.abs.x - fe.rect.pos.x) / fe.rect.size.x.max(1.0)).clamp(0.0, 1.0));
        }
        if let Some(fraction) = fraction {
            let cmds = self.decks.seek(deck, fraction);
            self.run_deck_cmds(cx, cmds);
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
        match SessionConnector::start(service::session_config_from_env()) {
            Ok(connector) => self.connector = Some(connector),
            Err(error) => self.status_text = format!("session config invalid: {error}"),
        }
        self.poll_timer = cx.start_interval(0.05);
        self.refresh_timer = cx.start_interval(1.0);
        self.video_loop = true;
        self.ui
            .check_box(cx, ids!(video_loop))
            .set_active(cx, true, Animate::No);
        self.external_sync_enabled = true;
        self.ui
            .check_box(cx, ids!(external_sync_enable))
            .set_active(cx, true, Animate::No);
        self.paint_tabs(cx, id!(video_page));
        self.paint_gen_tab(cx);
        self.sync_mix_mode_ui(cx);
        self.sync_fx_ui(cx);
        self.sync_bb_states_ui(cx);
        self.local_lib = LocalLibrary::load();
        self.queue_visible_thumbs(cx);
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
        if self.capture.is_none() {
            // System-audio capture: ONE bounded realtime callback. Input 0
            // is the explicitly selected loopback device — never a
            // microphone (see handle_audio_devices).
            let feed = Arc::new(CaptureFeed::new());
            let callback_feed = feed.clone();
            cx.audio_input(0, move |info, buffer| {
                callback_feed.push(info.sample_rate, buffer);
            });
            self.sync_worker = Some(SyncWorker::start(feed.clone()));
            self.capture = Some(feed);
        }
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
        cx.use_audio_inputs(&loopback);
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
        for desc in &ports.descs {
            if !apc40::is_apc40_port(&desc.name) {
                continue;
            }
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
        self.apc_leds.invalidate();
        self.midi_status = if self.apc_input_ports.is_empty() {
            "APC40: not connected".to_string()
        } else {
            format!(
                "APC40: {} ({} LED out)",
                names.join(", "),
                self.apc_output_ports.len()
            )
        };
        self.sync_apc_leds();
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // ---- tabs ----
        for (button, page, apc_surface) in [
            (ids!(tab_video), id!(video_page), Some(ApcSurface::Video)),
            (ids!(tab_music), id!(music_page), Some(ApcSurface::Music)),
            (ids!(tab_sfx), id!(sfx_page), Some(ApcSurface::Sfx)),
            (ids!(tab_mesh), id!(mesh_page), None),
        ] {
            if self.ui.button(cx, button).clicked(actions) {
                self.ui.page_flip(cx, ids!(pages)).set_active_page(cx, page.into());
                if let Some(surface) = apc_surface {
                    self.apc.surface = surface;
                    self.apc.bank = 0;
                }
                self.paint_tabs(cx, page);
                self.ui.redraw(cx);
            }
        }
        if self.ui.button(cx, ids!(tab_gen)).clicked(actions)
            || self.ui.button(cx, ids!(gen_fold)).clicked(actions)
        {
            self.set_gen_panel_open(cx, !self.gen_panel_open);
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
        if let Some(v) = self.ui.slider(cx, ids!(master_slider)).slided(actions) {
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

        // ---- search rows ----
        let rows: [(Surface, &[LiveId], &[LiveId], &[LiveId], &[LiveId]); 4] = [
            (
                Surface::Video,
                ids!(video_search),
                ids!(video_category),
                ids!(video_go),
                ids!(video_more),
            ),
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
            if let Some((text, _)) = self.ui.text_input(cx, search).returned(actions) {
                cmds.extend(self.model(surface).set_text(text.trim().to_string()));
            }
            if let Some((text, _)) = self.ui.text_input(cx, category).returned(actions) {
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
        for id in [ids!(pad_filter), ids!(video_search)] {
            if let Some(text) = self.ui.text_input(cx, id).changed(actions) {
                let widget = self.ui.widget(cx, ids!(video_grid));
                if let Some(mut pads) = widget.borrow_mut::<VjPadMatrix>() {
                    pads.set_filter(cx, text);
                    self.apc.bank = pads.bank;
                }
                self.sync_video_pad_window(cx);
            }
        }
        let (video_down, _) = self.pad_matrix_hits(cx, actions, ids!(video_grid));
        for asset in video_down {
            self.video_tile_clicked(cx, asset);
        }
        let (music_down, _) = self.grid_hits(cx, actions, ids!(music_grid));
        for asset in music_down {
            self.music_tile_clicked(cx, asset);
        }
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
        }
        if let Some(text) = self.ui.text_input(cx, ids!(gen_prompt)).changed(actions) {
            self.gen.set_prompt(text);
        }
        let submit_prompt = self
            .ui
            .text_input(cx, ids!(gen_prompt))
            .returned(actions)
            .map(|(text, _)| text);
        if let Some(text) = submit_prompt {
            self.gen.set_prompt(text);
            let cmds = self.gen.generate(now_ms());
            self.run_gen_cmds(cmds);
            self.grids_dirty = true;
        }
        if self.ui.button(cx, ids!(gen_go)).clicked(actions) {
            let text = self.ui.text_input(cx, ids!(gen_prompt)).text();
            self.gen.set_prompt(text);
            let cmds = self.gen.generate(now_ms());
            self.run_gen_cmds(cmds);
            self.grids_dirty = true;
        }
        if self.ui.button(cx, ids!(gen_clear)).clicked(actions) {
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
        if let Some(on) = self
            .ui
            .check_box(cx, ids!(external_sync_enable))
            .changed(actions)
        {
            self.external_sync_enabled = on;
            if !on {
                // Do not leave an already-armed cue waiting on a grid the
                // performer just bypassed.
                self.force_armed_fade_now(cx);
            }
            for slot in [SlotId::A, SlotId::B] {
                self.apply_loop_fit(slot);
            }
        }
        if self.ui.button(cx, ids!(external_sync_now)).clicked(actions) {
            self.force_armed_fade_now(cx);
        }
        if self.ui.button(cx, ids!(video_play)).clicked(actions) {
            self.toggle_video_playback(cx);
        }
        if let Some(v) = self.ui.slider(cx, ids!(video_pos)).end_slide(actions) {
            if let Some(slot) = self.cue.live_slot() {
                self.mixer.flush_slot_audio(slot);
                if let Some(player) = self.players[slot.index()].as_mut() {
                    player.seek_fraction(v);
                    if !player.is_paused() {
                        self.video_pump = cx.new_next_frame();
                    }
                }
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(video_loop)).changed(actions) {
            self.video_loop = on;
            for player in self.players.iter_mut().flatten() {
                player.set_loop(on);
            }
            if on {
                self.video_pump = cx.new_next_frame();
            }
        }
        if let Some(on) = self.ui.check_box(cx, ids!(video_mute)).changed(actions) {
            self.video_muted = on;
            self.mixer.set_video_muted(on);
            if on {
                self.disarm_hazards(Some(cx));
            }
            self.refresh_program_lighting();
        }
        if let Some(v) = self.ui.slider(cx, ids!(video_fade)).slided(actions) {
            self.fade_secs = v as f32;
        }

        // ---- deck transport ----
        if let Some(index) = self.ui.drop_down(cx, ids!(deck_target)).selected(actions) {
            self.deck_target = match index {
                1 => DeckTarget::A,
                2 => DeckTarget::B,
                _ => DeckTarget::Auto,
            };
        }
        for (deck, play, loop_id, mute, gain, zone) in [
            (
                DeckId::A,
                ids!(deck_a_play),
                ids!(deck_a_loop),
                ids!(deck_a_mute),
                ids!(deck_a_gain),
                ids!(deck_a_zone),
            ),
            (
                DeckId::B,
                ids!(deck_b_play),
                ids!(deck_b_loop),
                ids!(deck_b_mute),
                ids!(deck_b_gain),
                ids!(deck_b_zone),
            ),
        ] {
            if self.ui.button(cx, play).clicked(actions) {
                let cmds = self.decks.play_pause(deck);
                self.run_deck_cmds(cx, cmds);
            }
            if self.ui.check_box(cx, loop_id).changed(actions).is_some() {
                let cmds = self.decks.toggle_loop(deck);
                self.run_deck_cmds(cx, cmds);
            }
            if self.ui.check_box(cx, mute).changed(actions).is_some() {
                let cmds = self.decks.toggle_mute(deck);
                self.run_deck_cmds(cx, cmds);
            }
            if let Some(v) = self.ui.slider(cx, gain).slided(actions) {
                let cmds = self.decks.set_gain(deck, v as f32);
                self.run_deck_cmds(cx, cmds);
            }
            self.wave_seek(cx, actions, deck, zone);
        }
        if let Some(v) = self.ui.slider(cx, ids!(xfader)).slided(actions) {
            let cmds = self.decks.set_crossfader(v as f32);
            self.run_deck_cmds(cx, cmds);
        }
        if let Some(v) = self.ui.slider(cx, ids!(apc_xfader)).slided(actions) {
            self.set_visual_mix(cx, v as f32);
        }
        for (i, id) in [
            ids!(bb_s0),
            ids!(bb_s1),
            ids!(bb_s2),
            ids!(bb_s3),
            ids!(bb_s4),
            ids!(bb_s5),
            ids!(bb_s6),
            ids!(bb_s7),
        ]
        .iter()
        .enumerate()
        {
            if self.ui.button(cx, *id).clicked(actions) {
                let name = self
                    .live_billboard_slot()
                    .and_then(|slot| self.billboards[slot].as_ref())
                    .and_then(|bb| bb.states.get(i))
                    .map(|s| s.name.clone());
                if let Some(name) = name {
                    self.set_billboard_state(cx, &name);
                }
            }
        }
        if self.ui.button(cx, ids!(mix_mode)).clicked(actions) {
            self.overlay_mode = !self.overlay_mode;
            self.cue.set_overlay(self.overlay_mode);
            self.sync_mix_mode_ui(cx);
            self.video_pump = cx.new_next_frame();
        }
        if self.ui.button(cx, ids!(fx_prev)).clicked(actions) {
            self.fx.kind = self.fx.kind.wrap(-1);
            self.sync_fx_ui(cx);
        }
        if self.ui.button(cx, ids!(fx_next)).clicked(actions) {
            self.fx.kind = self.fx.kind.wrap(1);
            self.sync_fx_ui(cx);
        }
        if let Some(v) = self.ui.slider(cx, ids!(fx_p1)).slided(actions) {
            self.fx.p1 = v as f32;
            self.video_pump = cx.new_next_frame();
        }
        if let Some(v) = self.ui.slider(cx, ids!(fx_p2)).slided(actions) {
            self.fx.p2 = v as f32;
            self.video_pump = cx.new_next_frame();
        }
        if let Some(on) = self.ui.check_box(cx, ids!(fx_beat1)).changed(actions) {
            self.fx.beat1 = on;
            self.video_pump = cx.new_next_frame();
        }
        if let Some(on) = self.ui.check_box(cx, ids!(fx_beat2)).changed(actions) {
            self.fx.beat2 = on;
            self.video_pump = cx.new_next_frame();
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

        // ---- output window ----
        if self.ui.button(cx, ids!(open_output)).clicked(actions) {
            self.open_output_window(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_render::script_mod(vm);
        crate::views::script_mod(vm);
        crate::mesh_view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.handle_output_window_event(cx, event);
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
        if self.refresh_timer.is_event(event).is_some() {
            for surface in SURFACES {
                let model = self.model(surface);
                if model.refresh_wanted && !model.is_loading() {
                    let cmds = model.refresh();
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
        if self.video_pump.is_event(event).is_some() {
            self.pump_video(cx);
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
mod sync_tests {
    use super::*;
    use makepad_widgets::makepad_platform::audio::AudioBuffer;

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
}
