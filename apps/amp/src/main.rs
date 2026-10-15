//! music — one app for the music systems installed on this machine.
//!
//! The library of the first system found (Spotify, through the CLI inside
//! Spotify.app) is read into a local index kept on disk, so the playlists
//! show and search answers the moment the app opens, on every keystroke,
//! without asking the system, and still do when the system is closed,
//! signed out or gone. Folders of music files the person owns join it
//! (`local.rs`): a file that is a copy of a playlist track is linked to it.
//! The system's tracks play in the system's own player, this app being
//! its library browser and remote; local files play in this app
//! (`player.rs`), and while the system is away so do the local copies of
//! its tracks.
//!
//! The app owns the play order (`queue.rs`): a row plays the list as shown
//! from that row on, Next plays our next row, and the system only ever
//! holds our next few tracks in its own queue.
//!
//! Layout: the sidebar (search, the equalizer, the playlist tree), the main
//! pane (the open playlist or search results) beside the Up Next panel,
//! and the now-playing bar along the bottom.

pub use makepad_widgets;

use makepad_widgets::*;
use makepad_widgets_feedback::FeedbackPanelWidgetRefExt;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

mod audio;
mod cache;
mod columns;
mod connection;
mod eq;
mod geq;
mod hw;
mod layout;
mod index;
mod local;
mod micro;
mod panels;
mod milk;
mod popout;
mod torch;
mod model;
mod player;
mod queue;
mod source;
mod style_store;
mod visual;
mod widgets;
mod worker;
// The AI stylist and the style manager.
mod ai;
mod ai_schema;
mod theme_manager;

use audio::{AudioState, EqSettings};
use local::LocalCommand;
use milk::Milk;
use popout::Popout;
use model::{duration, Activate, Engine, Filter, Model, Page, PlayerRequest, RowCommand};
use player::{Output, PlayerCommand};
use source::{Repeat, Transport};
use queue::{PlayQueue, QueueItem};
use hw::HwKnobWidgetRefExt;
use layout::HwStageWidgetRefExt;
use micro::{HwClockWidgetRefExt, HwMarqueeWidgetRefExt};
use widgets::{LibraryListWidgetRefExt, MusicBarWidgetRefExt, TrackTableWidgetRefExt, UpNextAction, UpNextWidgetRefExt, ICON_NEXT, ICON_PAUSE, ICON_PLAY, ICON_PREVIOUS, ICON_QUEUE, ICON_RADIO, ICON_REPEAT, ICON_SHUFFLE, ICON_VOLUME};
use worker::{Command, DiskCommand, Message, Setting, Workers};

app_main!(App);

/// How long typing must pause before the catalog is searched: local
/// results come on every key, the system's catalog once the words settle.
const CATALOG_DELAY: f64 = 0.3;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // A key with its indicator dash under it.
    let LitKey = View{
        width: Fit
        height: Fit
        flow: Down
        spacing: 4
        align: Align{x: 0.5}
    }

    // A caption over a group of controls.
    let Group = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: 8
        align: Align{x: 0.5}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Makepad Amp"
                // Every skin draws its own chrome: the rail is the caption, with the
                // skin's own window buttons (`set_drag_region`).
                chrome: WindowChrome.Custom
                window.inner_size: vec2(1360 900)
                // The macOS View menu reaches the same controls (the keys'
                // shortcuts stay the app's own: these items have none).
                window_menu +: {
                    view_menu := MenuItem.Sub{name: "View" items: [@music_style, @music_layout, @music_ai, @music_popout, @music_torch, @music_eq, @line9, @zoom_in, @zoom_out, @fullscreen]}
                    music_style := MenuItem.Item{name: "Style…  (⌘T)" enabled: true}
                    music_layout := MenuItem.Item{name: "Layout…  (⌘L)" enabled: true}
                    music_ai := MenuItem.Item{name: "AI Stylist  (⌘I)" enabled: true}
                    music_popout := MenuItem.Item{name: "Party  (⇧⌘P)" enabled: true}
                    music_torch := MenuItem.Item{name: "Torch  (⇧⌘L)" enabled: true}
                    music_eq := MenuItem.Item{name: "Equalizer  (⌘E)" enabled: true}
                }
                // Overlay: the drawer and the menu layer cover the device.
                body +: {
                    flow: Overlay
                    app_bg := mod.widgets.HwChassis{
                        draw_bg +: {plate_id: 12.0}
                        flow: Down
                        padding: Inset{left: 18 right: 18 top: 0 bottom: 16}
                        // The chassis's top rail is the window's caption: the
                        // window buttons sit on it at the left, the device's own
                        // controls at the right (the style, the layout, the AI
                        // stylist, the party picture; one menu key when narrow),
                        // and its empty part drags the window (Window's drag
                        // region). A right-click on the chassis opens the menu.
                        rail := View{
                            width: Fill
                            height: 32
                            flow: Right
                            align: Align{y: 0.5}
                            padding: Inset{left: 64}
                            rail_title := mod.widgets.HwCaption{width: Fill align: Align{x: 0.5} text: "Makepad Amp"}
                            controls := View{
                                width: Fit
                                height: Fill
                                flow: Right
                                spacing: 6
                                align: Align{y: 0.5}
                                padding: Inset{left: 8 right: 10}
                                // The Spotify audio connection's status (connection.rs).
                                connect_cap := mod.widgets.HwKey{visible: false width: Fit height: 24 inset: 3 draw_bg +: {has_led: 1.0} connect_key := mod.widgets.HwKeyText{text: "SPOTIFY" width: Fit padding: Inset{left: 13 right: 11} draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                style_cap := mod.widgets.HwKey{width: Fit height: 24 inset: 3 draw_bg +: {role: 27.0} style_next := mod.widgets.HwKeyText{text: "STYLE" width: Fit padding: Inset{left: 13 right: 11} draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                layout_cap := mod.widgets.HwKey{width: Fit height: 24 inset: 3 draw_bg +: {role: 29.0} layout_next := mod.widgets.HwKeyText{text: "LAYOUT" width: Fit padding: Inset{left: 13 right: 11} draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                ai_cap := mod.widgets.HwKey{width: Fit height: 24 inset: 3 ai_key := mod.widgets.HwKeyText{text: "AI" width: Fit padding: Inset{left: 13 right: 11} draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                popout_cap := mod.widgets.HwKey{width: Fit height: 24 inset: 3 draw_bg +: {has_led: 1.0 role: 40.0} popout_key := mod.widgets.HwKeyText{text: "PARTY" width: Fit padding: Inset{left: 13 right: 11} draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                controls_menu_cap := mod.widgets.HwKey{visible: false width: 32 height: 24 inset: 3 controls_menu := mod.widgets.HwKeyButton{text: "\u{f0c9}" draw_text +: {text_style: theme.font_icons{font_size: 10.0}}}}
                                // Send feedback (makepad-widgets-feedback): the corner of the rail.
                                feedback_cap := mod.widgets.HwKey{width: 32 height: 24 inset: 3 feedback_key := mod.widgets.HwKeyIcon{draw_icon +: {svg: crate_resource("makepad_widgets_feedback:resources/feedback.svg")}}}
                            }
                            // The window's own buttons where the OS draws none on a
                            // hidden caption (Windows, Linux): minimise, maximise,
                            // close, at the rail's right (macOS keeps its traffic
                            // lights at the left).
                            winbtns := View{
                                visible: false
                                width: Fit
                                height: Fill
                                flow: Right
                                spacing: 4
                                align: Align{y: 0.5}
                                padding: Inset{right: 6}
                                win_min_cap := mod.widgets.HwKey{width: 34 height: 24 inset: 3 win_min := mod.widgets.HwKeyButton{text: "\u{f2d1}" draw_text +: {text_style: theme.font_icons{font_size: 9.0}}}}
                                win_max_cap := mod.widgets.HwKey{width: 34 height: 24 inset: 3 win_max := mod.widgets.HwKeyButton{text: "\u{f2d0}" draw_text +: {text_style: theme.font_icons{font_size: 9.0}}}}
                                win_close_cap := mod.widgets.HwKey{width: 34 height: 24 inset: 3 draw_bg +: {critical: 1.0} win_close := mod.widgets.HwKeyButton{text: "\u{f00d}" draw_text +: {text_style: theme.font_icons{font_size: 10.0}}}}
                            }
                        }

                        // The layout (a file of its own: slots and splitters) places the
                        // components; the components are the same in every layout, so
                        // switching layout keeps everything playing.
                        stage := mod.widgets.HwStage{
                            width: Fill
                            height: Fill
                            slots := mod.widgets.HwLayout{}
                            components := View{
                                width: Fill
                                height: Fill
                                flow: Overlay
                                c_volume := View{
                                    width: Fill
                                    height: Fill
                                    flow: Overlay
                                    // Pocket size: a slider in place of the wing.
                                    volume_slim := View{
                                        visible: false
                                        width: Fill
                                        height: Fill
                                        flow: Right
                                        spacing: 5
                                        align: Align{y: 0.5}
                                        padding: Inset{left: 4 right: 4}
                                        mod.widgets.HwCaption{
                                            text: "\u{f028}"
                                            draw_text +: {text_style: theme.font_icons{font_size: 8.0}}
                                        }
                                        View{
                                            width: Fill
                                            height: Fill
                                            flow: Down
                                            spacing: 2
                                            align: Align{y: 0.5}
                                            // The slider in a screen well, as the
                                            // position line: its track reads on
                                            // every style's front. Drag-only, and
                                            // the whole slot takes the finger.
                                            volume_well := mod.widgets.HwDisplay{
                                                draw_bg +: {plate_id: 9.0}
                                                width: Fill
                                                height: 22
                                                align: Align{y: 0.5}
                                                padding: Inset{left: 10 right: 10}
                                                volume_bar := MusicBar{width: Fill height: 14 relative: true}
                                            }
                                            // The level under it, where there is room.
                                            meter_slim := mod.widgets.HwMeter{height: 18 face +: {draw_bg +: {inner: vec4(0.72, 4.0, 0.0, 0.0)}}}
                                        }
                                    }
                                    left_wing := mod.widgets.HwWing{
                                        draw_bg +: {plate_id: 1.0}
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        spacing: 10
                                        align: Align{x: 0.5}
                                        brand := mod.widgets.HwTitle{text: "MAKEPAD"}
                                        brand_sub := mod.widgets.HwCaption{text: "AMP"}
                                        Group{
                                            mod.widgets.HwCaption{text: "VOLUME"}
                                            volume := mod.widgets.HwKnob{width: 120 height: 120}
                                        }
                                        meter := mod.widgets.HwMeter{height: 56}
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 8
                                            align: Align{y: 0.5}
                                            volume_icon := mod.widgets.HwCaption{
                                                text: "\u{f028}"
                                                draw_text +: {text_style: theme.font_icons{font_size: 9.0}}
                                            }
                                            devices := mod.widgets.HwDropDown{
                                                width: Fill
                                                height: 26
                                                labels: ["No device"]
                                                draw_text +: {text_style: theme.font_regular{font_size: 9.5}}
                                            }
                                        }
                                    }}
                                c_screen := View{
                                    width: Fill
                                    height: Fill
                                    screen := mod.widgets.HwBezel{
                                        draw_bg +: {plate_id: 7.0}
                                        width: Fill
                                        height: Fill
                                        flow: Overlay
                                        padding: Inset{left: 14 right: 14 top: 14 bottom: 14}
                                        // The picture of the sound: the spectrum or MilkDrop.
                                        visual := mod.widgets.HwSpectrum{}
                                        milk_stage := View{
                                            visible: false
                                            width: Fill
                                            height: Fill
                                            milk_frame := RectView{
                                                width: Fill
                                                height: Fill
                                                draw_bg +: {color: #x000000}
                                                milk := mod.widgets.HwMilk{}
                                            }
                                        }
                                        // Faint, low: the sound is missing (connection.rs).
                                        // The music plays on another device: a calm card
                                        // saying so, with the way back.
                                        remote_card_row := View{
                                            visible: false
                                            width: Fill
                                            height: Fill
                                            align: Align{x: 0.5 y: 0.5}
                                            padding: Inset{left: 20 right: 20}
                                            remote_card := mod.widgets.HwDisplay{
                                                width: Fill
                                                height: Fit
                                                flow: Down
                                                spacing: 8
                                                align: Align{x: 0.5}
                                                padding: Inset{left: 16 right: 16 top: 12 bottom: 14}
                                                remote_title := mod.widgets.HwDisplayText{width: Fill align: Align{x: 0.5} text: "" draw_text +: {text_style: theme.font_bold{font_size: 11.0}}}
                                                remote_note := mod.widgets.HwDisplayText{width: Fill align: Align{x: 0.5} text: "The visuals need the music playing on this Mac." draw_text +: {text_style: theme.font_regular{font_size: 9.0}}}
                                                remote_here_cap := mod.widgets.HwKey{width: Fit height: 30 inset: 3 draw_bg +: {role: 52.0} remote_here := mod.widgets.HwKeyText{text: "PLAY HERE" width: Fit padding: Inset{left: 14 right: 14}}}
                                            }
                                        }
                                        screen_hint_row := View{visible: false width: Fill height: Fill align: Align{x: 0.5 y: 1.0} padding: Inset{bottom: 8} screen_hint := mod.widgets.HwConnectHint{text: "Visuals need the Spotify audio connection"}}
                                        screen_glass := mod.widgets.HwGlass{}
                                    }}
                                c_transport := View{
                                    width: Fill
                                    height: Fill
                                    keys := View{
                                        width: Fill
                                        height: Fill
                                        flow: Right
                                        spacing: 12
                                        align: Align{x: 0.5 y: 0.5}
                                        LitKey{
                                            shuffle_cap := mod.widgets.HwTransportKey{draw_bg +: {has_led: 1.0 role: 4.0} shuffle := mod.widgets.HwTransportLegend{text: "\u{f074}"}}
                                            shuffle_led := mod.widgets.HwDash{}
                                        }
                                        LitKey{
                                            previous_cap := mod.widgets.HwTransportKey{width: 70 draw_bg +: {role: 2.0} previous := mod.widgets.HwTransportLegend{text: "\u{f048}"}}
                                            previous_pad := View{width: 1 height: 20}
                                        }
                                        LitKey{
                                            play_cap := mod.widgets.HwPlayKey{
                                                draw_bg +: {critical: 1.0 role: 1.0}
                                                play_pause := mod.widgets.HwPlayLegend{text: "\u{f04b}"}
                                            }
                                            play_led := mod.widgets.HwDash{width: 60}
                                        }
                                        LitKey{
                                            next_cap := mod.widgets.HwTransportKey{width: 70 draw_bg +: {role: 3.0} next := mod.widgets.HwTransportLegend{text: "\u{f051}"}}
                                            next_pad := View{width: 1 height: 20}
                                        }
                                        LitKey{
                                            repeat_cap := mod.widgets.HwTransportKey{draw_bg +: {has_led: 1.0 role: 5.0} repeat := mod.widgets.HwTransportLegend{text: "\u{f01e}"}}
                                            repeat_led := mod.widgets.HwDash{}
                                        }
                                        LitKey{
                                            radio_cap := mod.widgets.HwTransportKey{draw_bg +: {has_led: 1.0 role: 6.0} radio := mod.widgets.HwTransportLegend{text: "\u{f519}"}}
                                            radio_led := mod.widgets.HwDash{}
                                        }
                                    }}
                                c_display := View{
                                    width: Fill
                                    height: Fill
                                    right_wing := mod.widgets.HwWing{
                                        draw_bg +: {plate_id: 2.0}
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        spacing: 10
                                        align: Align{x: 0.5}
                                        mod.widgets.HwCaption{text: "DISPLAY"}
                                        // The screen's picture: the spectrum or a MilkDrop preset.
                                        eq_view := mod.widgets.HwSegmented{
                                            segment_height: 26
                                            // Three words across the wing: tighter caps, smaller type.
                                            segment_padding: 5
                                            draw_text +: {text_style +: {font_size: 8.5}}
                                            draw_text_selected +: {text_style +: {font_size: 8.5}}
                                            options: ["Spectrum" "Visualizer"]
                                        }
                                        milk_bar := View{
                                            width: Fill
                                            height: Fit
                                            flow: Down
                                            spacing: 8
                                            align: Align{x: 0.5}
                                            View{
                                                width: Fit
                                                height: Fit
                                                flow: Right
                                                spacing: 6
                                                milk_prev_cap := mod.widgets.HwKey{width: 46 height: 40 draw_bg +: {role: 10.0} milk_prev := mod.widgets.HwKeyButton{text: "\u{f048}"}}
                                                milk_random_cap := mod.widgets.HwKey{width: 46 height: 40 draw_bg +: {role: 11.0} milk_random := mod.widgets.HwKeyButton{text: "\u{f074}"}}
                                                milk_next_cap := mod.widgets.HwKey{width: 46 height: 40 draw_bg +: {role: 12.0} milk_next := mod.widgets.HwKeyButton{text: "\u{f051}"}}
                                                milk_full_cap := mod.widgets.HwKey{width: 46 height: 40 draw_bg +: {role: 13.0} milk_full := mod.widgets.HwKeyButton{text: "\u{f065}"}}
                                            }
                                            View{
                                                width: Fit
                                                height: Fit
                                                flow: Right
                                                spacing: 6
                                                milk_fav_cap := mod.widgets.HwKey{width: 46 height: 40 draw_bg +: {has_led: 1.0 role: 14.0} milk_fav := mod.widgets.HwKeyButton{text: "\u{f004}"}}
                                                milk_lock_cap := mod.widgets.HwKey{width: 46 height: 40 draw_bg +: {has_led: 1.0 role: 15.0} milk_lock := mod.widgets.HwKeyButton{text: "\u{f023}"}}
                                                milk_list_cap := mod.widgets.HwKey{width: 96 height: 40 draw_bg +: {has_led: 1.0 critical: 1.0 role: 16.0} milk_list := mod.widgets.HwKeyText{text: "PRESETS"}}
                                            }
                                        }
                                        // The wing's middle: the style's ornament (`HwOrnament`),
                                        // or room, so the settings below sit at the wing's foot.
                                        wing_fill := View{
                                            width: Fill
                                            height: Fill
                                            flow: Down
                                            spacing: 10
                                            align: Align{y: 0.5}
                                            // The picture's name, whole, in a window of its own.
                                            milk_now_window := mod.widgets.HwLabelWindow{
                                                height: Fit
                                                flow: Down
                                                align: Align{x: 0.5 y: 0.5}
                                                padding: Inset{left: 10 right: 10 top: 9 bottom: 9}
                                                milk_now := mod.widgets.HwDisplayText{width: Fill align: Align{x: 0.5} max_lines: 3 text_overflow: TextOverflow.Ellipsis text: "Visualizer" draw_text +: {text_style: theme.font_regular{font_size: 10.5}}}
                                            }
                                            ornament := mod.widgets.HwOrnament{}
                                        }
                                        // The choosers fold away in a short wing (a drop
                                        // down cannot hide itself; its View can).
                                        milk_choosers := View{
                                            width: Fill
                                            height: Fit
                                            flow: Down
                                            spacing: 8
                                            milk_auto := mod.widgets.HwDropDown{
                                                width: Fill
                                                height: 26
                                                labels: ["Change: never" "Change: 15 s" "Change: 30 s" "Change: 1 min" "Change: 2 min"]
                                            }
                                        }
                                        // The equalizer page, and the picture in the style's colours.
                                        View{
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 6
                                            eq_page_cap := mod.widgets.HwKey{width: Fill height: 34 draw_bg +: {has_led: 1.0 role: 18.0} eq_page_key := mod.widgets.HwKeyText{text: "EQ"}}
                                            map_cap := mod.widgets.HwKey{width: Fill height: 34 draw_bg +: {has_led: 1.0 role: 17.0} map_key := mod.widgets.HwKeyText{text: "THEME COLOURS"}}
                                        }
                                    }}
                                c_now := View{
                                    width: Fill
                                    height: Fill
                                    now_left := View{
                                        width: Fill
                                        height: Fill
                                        flow: Right
                                        spacing: 8
                                        align: Align{y: 0.5}
                                        art_well := mod.widgets.HwPanel{
                                            draw_bg +: {plate_id: 8.0}
                                            width: 58
                                            height: 58
                                            radius: 6
                                            flow: Overlay
                                            padding: Inset{left: 4 right: 4 top: 4 bottom: 4}
                                            art_blank := mod.widgets.HwArtBlank{}
                                            art := Image{visible: false width: 50 height: 50 fit: ImageFit.Smallest}
                                        }
                                        now_text := View{
                                            width: Fill
                                            height: Fit
                                            flow: Down
                                            spacing: 3
                                            now_title := mod.widgets.HwTitle{width: Fill max_lines: 1 text_overflow: TextOverflow.Ellipsis text: "Nothing playing" draw_text +: {text_style: theme.font_bold{font_size: 10.5}}}
                                            now_artist := mod.widgets.HwText{width: Fill max_lines: 1 text_overflow: TextOverflow.Ellipsis text: ""}
                                            now_context := mod.widgets.HwCaption{width: Fill max_lines: 1 text_overflow: TextOverflow.Ellipsis text: ""}
                                            // Cast to another device: where it plays.
                                            now_cast := View{
                                                visible: false
                                                width: Fill
                                                height: Fit
                                                flow: Right
                                                spacing: 5
                                                align: Align{y: 0.5}
                                                now_cast_icon := mod.widgets.HwCaption{text: "\u{f028}" draw_text +: {text_style: theme.font_icons{font_size: 8.5}}}
                                                now_cast_name := mod.widgets.HwCaption{width: Fill max_lines: 1 text_overflow: TextOverflow.Ellipsis text: ""}
                                            }
                                        }
                                    }}
                                c_progress := View{
                                    width: Fill
                                    height: Fill
                                    align: Align{y: 0.5}
                                    progress_well := mod.widgets.HwDisplay{
                                        draw_bg +: {plate_id: 9.0}
                                        width: Fill
                                        height: 38
                                        flow: Right
                                        spacing: 10
                                        align: Align{y: 0.5}
                                        padding: Inset{left: 12 right: 12}
                                        position := mod.widgets.HwReadout{width: 44 align: Align{x: 1.0} text: ""}
                                        progress := MusicBar{}
                                        length := mod.widgets.HwReadout{width: 44 text: ""}
                                    }}
                                // The time, big (seven segments unless the style
                                // draws it otherwise), and the playing title
                                // scrolling: the pocket layouts' readouts.
                                c_clock := View{
                                    width: Fill
                                    height: Fill
                                    clock := mod.widgets.HwClock{}
                                }
                                c_marquee := View{
                                    width: Fill
                                    height: Fill
                                    marquee_well := mod.widgets.HwDisplay{
                                        draw_bg +: {plate_id: 9.0}
                                        width: Fill
                                        height: Fill
                                        padding: Inset{left: 8 right: 8 top: 2 bottom: 2}
                                        marquee := mod.widgets.HwMarquee{}
                                    }
                                }
                                c_queue := View{
                                    width: Fill
                                    height: Fill
                                    align: Align{x: 1.0 y: 0.5}
                                    up_next_cap := mod.widgets.HwKey{width: 52 height: 48 draw_bg +: {role: 26.0} up_next_toggle := mod.widgets.HwKeyButton{text: "\u{f0cb}"}}}
                                c_library := View{
                                    width: Fill
                                    height: Fill
                                    sidebar := mod.widgets.HwPanel{
                                        draw_bg +: {plate_id: 4.0}
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        spacing: 4
                                        padding: Inset{left: 10 right: 8 top: 12 bottom: 10}
                                        search := mod.widgets.HwTextInput{
                                            width: Fill
                                            height: 30
                                            empty_text: "Search your music"
                                        }
                                        // The playlists, and a line only when something is wrong.
                                        library := LibraryList{margin: Inset{top: 6}}
                                        status := mod.widgets.HwPanelText{visible: false width: Fill margin: Inset{left: 10 top: 4} max_lines: 2 text_overflow: TextOverflow.Ellipsis text: ""}
                                    }}
                                c_tracks := View{
                                    width: Fill
                                    height: Fill
                                    tracks_panel := mod.widgets.HwPanel{
                                        draw_bg +: {plate_id: 5.0}
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        spacing: 6
                                        // The list fills the panel: the column header is
                                        // its heading (the list's name is on its tab key).
                                        padding: Inset{left: 3 right: 3 top: 3 bottom: 3}
                                        // Only the folders page has a header (its ADD FOLDER).
                                        header := View{
                                            visible: false
                                            width: Fill
                                            height: Fit
                                            flow: Right
                                            spacing: 12
                                            align: Align{y: 0.5}
                                            View{
                                                width: Fill
                                                height: Fit
                                                flow: Down
                                                spacing: 3
                                                // One slim line: the list's name.
                                                title := mod.widgets.HwPanelTitle{
                                                    visible: false
                                                    width: Fill
                                                    max_lines: 1
                                                    text_overflow: TextOverflow.Ellipsis
                                                    text: ""
                                                    draw_text +: {text_style: theme.font_bold{font_size: 12.0}}
                                                }
                                            }
                                            add_folder_cap := mod.widgets.HwKey{
                                                visible: false
                                                width: 120
                                                height: 38
                                                draw_bg +: {role: 22.0}
                                                add_folder := mod.widgets.HwKeyText{text: "ADD FOLDER"}
                                            }
                                        }
                                        // Said while the music system is away.
                                        away_note := mod.widgets.HwPanelText{visible: false width: Fill text: ""}
                                        tracks := TrackTable{}
                                    }}
                                c_upnext := View{
                                    width: Fill
                                    height: Fill
                                    up_next_panel := mod.widgets.HwPanel{
                                        draw_bg +: {plate_id: 6.0}
                                        width: Fill
                                        height: Fill
                                        flow: Down
                                        spacing: 6
                                        padding: Inset{left: 6 right: 8 top: 14 bottom: 8}
                                        up_next := UpNext{}
                                    }}
                                // The graphic EQ (`eq.rs`): in a layout's `eq` slot, or a
                                // drawer over the main list the EQ key opens.
                                c_eq := View{
                                    width: Fill
                                    height: Fill
                                    eq_plate := mod.widgets.HwPlate{
                                        draw_bg +: {plate_id: 14.0}
                                        width: Fill
                                        height: Fill
                                        depth: 7.0
                                        inset: 8
                                        padding: Inset{left: 16 right: 16 top: 14 bottom: 14}
                                        flow: Overlay
                                        eq_panel := mod.widgets.HwEq{}
                                        eq_remote_row := View{visible: false width: Fill height: Fill align: Align{x: 0.5 y: 0.0} padding: Inset{top: 34} eq_remote := mod.widgets.HwCaption{text: ""}}
                                        eq_hint_row := View{visible: false width: Fill height: Fill align: Align{x: 0.5 y: 0.0} padding: Inset{top: 50} eq_hint := mod.widgets.HwConnectHint{text: "The equalizer needs the Spotify audio connection"}}
                                    }}
                                // The small window's parts (panels.rs): a mini spectrum,
                                // the playlist band's header (the list chooser and an
                                // inline search) and its numbered list.
                                c_spectrum := View{
                                    width: Fill
                                    height: Fill
                                    spectrum_well := mod.widgets.HwDisplay{
                                        draw_bg +: {plate_id: 9.0}
                                        width: Fill
                                        height: Fill
                                        radius: 4
                                        padding: Inset{left: 3 right: 3 top: 3 bottom: 3}
                                        mini_spectrum := mod.widgets.HwSpectrum{compact: true}
                                    }
                                }
                                // A desktop player's file readouts: kbps, kHz, stereo.
                                c_info := View{
                                    width: Fill
                                    height: Fill
                                    flow: Right
                                    spacing: 4
                                    align: Align{y: 0.5}
                                    info_kbps_well := mod.widgets.HwLabelWindow{width: Fit height: 18 padding: Inset{left: 6 right: 6} info_kbps := mod.widgets.HwReadout{text: "" draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}}
                                    info_khz_well := mod.widgets.HwLabelWindow{width: Fit height: 18 padding: Inset{left: 6 right: 6} info_khz := mod.widgets.HwReadout{text: "" draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}}
                                    info_ch_well := mod.widgets.HwLabelWindow{width: Fit height: 18 padding: Inset{left: 6 right: 6} info_ch := mod.widgets.HwReadout{text: "" draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}}
                                }
                                c_plhead := View{
                                    width: Fill
                                    height: Fill
                                    flow: Right
                                    spacing: 6
                                    align: Align{y: 0.5}
                                    pl_search := mod.widgets.HwTextInput{width: Fill height: 26 empty_text: "Search"}
                                    pl_lists_cap := mod.widgets.HwKey{width: Fit height: 26 inset: 3 draw_bg +: {has_led: 1.0 role: 49.0} pl_lists := mod.widgets.HwKeyText{text: "PLAYLISTS" width: Fit padding: Inset{left: 20 right: 12} draw_text +: {text_style: theme.font_bold{font_size: 9.0}}}}
                                }
                                c_plist := View{
                                    width: Fill
                                    height: Fill
                                    pl_well := mod.widgets.HwDisplay{
                                        draw_bg +: {plate_id: 9.0}
                                        width: Fill
                                        height: Fill
                                        radius: 4
                                        flow: Down
                                        padding: Inset{left: 4 right: 4 top: 4 bottom: 2}
                                        pl_lib := LibraryList{
                                            visible: false
                                            flat: true
                                            draw_name +: {text_style: theme.font_regular{font_size: 10.5}}
                                            draw_count +: {text_style: theme.font_regular{font_size: 9.5}}
                                            grid +: {default_row_height: 21.0}
                                        }
                                        plist := TrackTable{
                                            numbered: true
                                            draw_title +: {text_style: theme.font_regular{font_size: 10.5}}
                                            draw_small +: {text_style: theme.font_regular{font_size: 9.5}}
                                            grid +: {show_col_headers: false default_row_height: 21.0 allow_col_resize: false}
                                        }
                                        View{
                                            width: Fill
                                            height: 15
                                            align: Align{x: 1.0 y: 0.5}
                                            padding: Inset{right: 8}
                                            pl_total := mod.widgets.HwReadout{text: "" draw_text +: {text_style: theme.font_regular{font_size: 8.5}}}
                                        }
                                    }
                                }
                                // The small window's section keys (panels.rs): the
                                // equalizer band and the playlist.
                                c_toggles := View{
                                    width: Fill
                                    height: Fill
                                    flow: Right
                                    spacing: 6
                                    align: Align{y: 0.5}
                                    tog_eq_cap := mod.widgets.HwKey{width: Fill height: 30 inset: 3 draw_bg +: {has_led: 1.0 role: 18.0} tog_eq := mod.widgets.HwKeyText{text: "EQ" draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                    tog_pl_cap := mod.widgets.HwKey{width: Fill height: 30 inset: 3 draw_bg +: {has_led: 1.0 role: 26.0} tog_pl := mod.widgets.HwKeyText{text: "PL" draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                    tog_vis_cap := mod.widgets.HwKey{width: Fill height: 30 inset: 3 draw_bg +: {has_led: 1.0 role: 13.0} tog_vis := mod.widgets.HwKeyText{text: "VIS" draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                }
                                // The theatre's deck (milk.rs): the picture's mode, its
                                // name in a label window, and the keys that open the
                                // presets and the equalizer.
                                c_deck := View{
                                    width: Fill
                                    height: Fill
                                    flow: Right
                                    spacing: 8
                                    align: Align{y: 0.5}
                                    deck_mode := mod.widgets.HwSegmented{
                                        segment_height: 28
                                        segment_padding: 5
                                        draw_text +: {text_style +: {font_size: 9.0}}
                                        draw_text_selected +: {text_style +: {font_size: 9.0}}
                                        options: ["Spectrum" "Visualizer"]
                                    }
                                    // The previous, a random and the next preset (or effect).
                                    deck_prev_cap := mod.widgets.HwKey{width: 42 height: 34 inset: 3 draw_bg +: {role: 10.0} deck_prev := mod.widgets.HwKeyButton{text: "\u{f048}"}}
                                    deck_random_cap := mod.widgets.HwKey{width: 42 height: 34 inset: 3 draw_bg +: {role: 11.0} deck_random := mod.widgets.HwKeyButton{text: "\u{f074}"}}
                                    deck_next_cap := mod.widgets.HwKey{width: 42 height: 34 inset: 3 draw_bg +: {role: 12.0} deck_next := mod.widgets.HwKeyButton{text: "\u{f051}"}}
                                    // The picture in the style's colours, and the automatic change.
                                    deck_map_cap := mod.widgets.HwKey{width: Fit height: 34 inset: 3 draw_bg +: {has_led: 1.0 role: 17.0} deck_map := mod.widgets.HwKeyText{text: "THEME COLOURS" width: Fit padding: Inset{left: 16 right: 11} draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                    deck_map_icon_cap := mod.widgets.HwKey{visible: false width: 42 height: 34 inset: 3 draw_bg +: {has_led: 1.0 role: 17.0} deck_map_icon := mod.widgets.HwKeyButton{text: "\u{f53f}"}}
                                    deck_auto_cap := mod.widgets.HwKey{width: Fit height: 34 inset: 3 draw_bg +: {role: 50.0} deck_auto := mod.widgets.HwKeyText{text: "CHANGE: 30 S" width: Fit padding: Inset{left: 12 right: 12} draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                    deck_auto_icon_cap := mod.widgets.HwKey{visible: false width: 42 height: 34 inset: 3 draw_bg +: {role: 50.0} deck_auto_icon := mod.widgets.HwKeyButton{text: "\u{f017}"}}
                                    // A narrow deck: the rest of its keys in one menu.
                                    deck_more_cap := mod.widgets.HwKey{visible: false width: 42 height: 34 inset: 3 draw_bg +: {role: 51.0} deck_more := mod.widgets.HwKeyButton{text: "\u{f141}"}}
                                    deck_name_window := mod.widgets.HwLabelWindow{
                                        width: Fill
                                        height: 30
                                        deck_name := mod.widgets.HwMarquee{speed: 18.0}
                                    }
                                    deck_presets_cap := mod.widgets.HwKey{width: Fit height: 34 inset: 3 draw_bg +: {has_led: 1.0 critical: 1.0 role: 16.0} deck_presets := mod.widgets.HwKeyText{text: "PRESETS" width: Fit padding: Inset{left: 16 right: 11} draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                    deck_eq_cap := mod.widgets.HwKey{width: Fit height: 34 inset: 3 draw_bg +: {has_led: 1.0 role: 18.0} deck_eq := mod.widgets.HwKeyText{text: "EQ" width: Fit padding: Inset{left: 16 right: 11} draw_text +: {text_style: theme.font_bold{font_size: 9.5}}}}
                                    // Narrow decks: the same keys as glyphs.
                                    deck_presets_icon_cap := mod.widgets.HwKey{visible: false width: 42 height: 34 inset: 3 draw_bg +: {has_led: 1.0 critical: 1.0 role: 16.0} deck_presets_icon := mod.widgets.HwKeyButton{text: "\u{f0ca}"}}
                                    deck_eq_icon_cap := mod.widgets.HwKey{visible: false width: 42 height: 34 inset: 3 draw_bg +: {has_led: 1.0 role: 18.0} deck_eq_icon := mod.widgets.HwKeyButton{text: "\u{f1de}"}}
                                }
                                // The panels host's tab keys (panels.rs): the pages the
                                // list area swaps between.
                                c_tabs := View{
                                    width: Fill
                                    height: Fill
                                    flow: Right
                                    spacing: 6
                                    align: Align{y: 0.5}
                                    tab_tracks_cap := mod.widgets.HwKey{width: Fit height: 32 inset: 3 flow: Right spacing: 7 align: Align{x: 0.5 y: 0.5} padding: Inset{left: 14 right: 14} draw_bg +: {has_led: 1.0 role: 46.0}
                                        tab_tracks_icon := mod.widgets.HwKeyButton{text: "\u{f001}" width: Fit draw_text +: {text_style: theme.font_icons{font_size: 10.5}}}
                                        tab_tracks := mod.widgets.HwKeyText{text: "Tracks" width: Fit draw_text +: {text_style: theme.font_bold{font_size: 11.5}}}
                                    }
                                    tab_eq_cap := mod.widgets.HwKey{width: Fit height: 32 inset: 3 flow: Right spacing: 7 align: Align{x: 0.5 y: 0.5} padding: Inset{left: 14 right: 14} draw_bg +: {has_led: 1.0 role: 47.0}
                                        tab_eq_icon := mod.widgets.HwKeyButton{text: "\u{f1de}" width: Fit draw_text +: {text_style: theme.font_icons{font_size: 10.5}}}
                                        tab_eq := mod.widgets.HwKeyText{text: "Equalizer" width: Fit draw_text +: {text_style: theme.font_bold{font_size: 11.5}}}
                                    }
                                }
                                // The AI stylist's chat (theme_manager.rs).
                                c_chat := View{
                                    width: Fill
                                    height: Fill
                                    mod.widgets.HwChat{}
                                }
                            }
                        }
                    }
                    // The MilkDrop presets: a drawer that rises over the lower
                    // right of the device, under the screen's wing, never
                    // covering or moving the screen.
                    drawer_layer := View{
                        width: Fill
                        height: Fill
                        align: Align{x: 1.0 y: 1.0}
                        milk_drawer := mod.widgets.HwPlate{
                            draw_bg +: {plate_id: 3.0}
                            visible: false
                            width: 470
                            height: Fill
                            depth: 7.0
                            inset: 10
                            margin: Inset{top: 422 bottom: 10 right: 10}
                            flow: Down
                            spacing: 8
                            padding: Inset{left: 22 right: 22 top: 20 bottom: 20}
                            View{
                                width: Fill
                                height: Fit
                                flow: Right
                                align: Align{y: 0.5}
                                mod.widgets.HwTitle{width: Fill text: "PRESETS"}
                                drawer_close_cap := mod.widgets.HwKey{width: 44 height: 36 draw_bg +: {role: 28.0} milk_drawer_close := mod.widgets.HwKeyButton{text: "\u{f00d}"}}
                            }
                            milk_well := mod.widgets.HwDisplay{
                                draw_bg +: {plate_id: 11.0}
                                width: Fill
                                height: Fill
                                padding: Inset{left: 8 right: 8 top: 8 bottom: 8}
                                milk_browser := mod.widgets.HwBrowser{height: Fill}
                            }
                        }
                    }
                    // The style manager ("Edit styles…"), over the device.
                    style_manager := mod.widgets.HwStyleManager{}
                    // The Spotify audio connection's explainer (connection.rs).
                    connection_sheet := mod.widgets.HwConnectionSheet{}
                    conn_tip := CalloutTooltip{}
                    // The torch (torch.rs): carried over the device, it lights it.
                    torch := mod.widgets.HwTorch{visible: false}
                    // Send feedback: the message, the screenshot and the system
                    // information, each removable before it goes.
                    feedback := mod.widgets.FeedbackPanel{app_id: "amp" app_name: "Makepad Amp" app_version: #(env!("CARGO_PKG_VERSION"))}
                    // The row menus draw here, over everything.
                    menus := mod.widgets.HwMenuLayer{}
                }
            }
        }
    }
}

// ---- theme ----

/// The palette the lists, the spectrum and the bars paint with: the
/// style's `hw_palette` (`hw::palette`).
/// A change the window crossfades to (`App::transition`).
enum Change {
    Style(String),
    ShowStyle(String),
    Layout(String, bool),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub accent: Vec4f,
    pub background: Vec4f,
    pub foreground: Vec4f,
    /// Secondary text.
    pub muted: Vec4f,
    /// A key's legend while its switch is on, and while off.
    pub legend_on: Vec4f,
    pub legend_off: Vec4f,
    /// Titles and headings the app sets are shown in capitals.
    pub caps: bool,
    /// Key legends sized to the key's face: glyphs at `legend_scale` of its
    /// short side, words at `legend_text_scale` (and to fit its width),
    /// moved down by `legend_offset` of the short side; `legend_fit` off
    /// leaves every legend as the style set it, `legend_transport` off the
    /// transport keys' (a mechanical key's label window).
    pub legend_scale: f32,
    pub legend_text_scale: f32,
    pub legend_offset: f32,
    pub legend_fit: bool,
    pub legend_transport: bool,
    /// The selected row of a list is printed inverted (the text colour as
    /// its floor, the floor's colour as its text), like an LCD's cursor.
    pub row_invert: bool,
    /// The device's floor is dark (the style's `dark_floor`, else its
    /// panel type is light): display type reads on it without a window.
    pub dark_floor: bool,
}

impl Default for Theme {
    fn default() -> Self {
        let accent = color("#1ed760");
        let muted = color("#9b9ea6");
        Self {
            accent,
            background: color("#18191b"),
            foreground: color("#e8e9eb"),
            muted,
            legend_on: accent,
            legend_off: muted,
            caps: false,
            legend_scale: 0.34,
            legend_text_scale: 0.36,
            legend_offset: 0.0,
            legend_fit: true,
            legend_transport: true,
            row_invert: false,
            dark_floor: false,
        }
    }
}

impl Theme {
    /// The sidebar and the now-playing bar: a step off the background.
    pub fn raised(&self) -> Vec4f {
        mix(self.background, self.foreground, 0.04)
    }

    /// The colours of an inverted row: text in the floor's colour.
    pub fn inverted(&self) -> Theme {
        Theme {
            background: self.foreground,
            foreground: self.background,
            muted: mix(self.background, self.foreground, 0.3),
            accent: self.background,
            ..*self
        }
    }
}

fn color(value: &str) -> Vec4f {
    parse_color(value).unwrap_or(vec4(1.0, 0.0, 1.0, 1.0))
}

/// `#rrggbb` or `#rrggbbaa`.
fn parse_color(value: &str) -> Option<Vec4f> {
    let digits = value.trim().trim_start_matches('#');
    let rgba = match digits.len() {
        6 => u32::from_str_radix(digits, 16).ok()?.checked_shl(8)? | 0xff,
        8 => u32::from_str_radix(digits, 16).ok()?,
        _ => return None,
    };
    Some(Vec4f::from_u32(rgba))
}

pub fn with_alpha(mut value: Vec4f, alpha: f32) -> Vec4f {
    value.w = alpha;
    value
}

pub fn mix(a: Vec4f, b: Vec4f, t: f32) -> Vec4f {
    vec4(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w + (b.w - a.w) * t)
}

// ---- album art ----

/// Album art for the now-playing bar: fetched once and kept on disk by the
/// disk worker, which does every file read and write; the UI only asks and
/// shows.
#[derive(Default)]
struct Art {
    /// The picture the bar wants.
    wanted: String,
    /// Requests in flight: id -> URL.
    pending: HashMap<LiveId, String>,
}

impl Art {
    /// The system's art URL at a size that suits a 60 pt square on a
    /// retina screen. Spotify's image ids carry the size: `…4851` is 64 px,
    /// `…1e02` is 300 px.
    fn sized(url: &str) -> String {
        url.replace("ab67616d00004851", "ab67616d00001e02")
    }

    /// Show the picture at `url`: asked from the disk cache first.
    fn show(&mut self, cx: &mut Cx, image: &ImageRef, url: &str, workers: Option<&Workers>) {
        let url = Self::sized(url);
        if url == self.wanted {
            return;
        }
        self.wanted = url.clone();
        if url.is_empty() {
            image.set_texture(cx, None);
            return;
        }
        if let Some(workers) = workers {
            if !workers.disk(DiskCommand::Find(url.clone())) {
                // The cache is behind: fetch instead of waiting for it.
                self.fetch(cx, url);
            }
        }
    }

    /// The disk cache answered for `url`.
    fn found(&mut self, cx: &mut Cx, image: &ImageRef, url: String, bytes: Option<Arc<[u8]>>) {
        if url != self.wanted {
            return;
        }
        match bytes {
            Some(bytes) => {
                let _ = image.load_image_from_data_async(cx, Path::new(&url), bytes);
            }
            None => self.fetch(cx, url),
        }
    }

    fn fetch(&mut self, cx: &mut Cx, url: String) {
        // An offline song's artwork is only on disk.
        if url.starts_with(cache::ART_URI) {
            return;
        }
        if !self.pending.values().any(|pending| *pending == url) {
            let id = LiveId::unique();
            cx.http_request(id, HttpRequest::new(url.clone(), HttpMethod::GET));
            self.pending.insert(id, url);
        }
    }

    fn arrived(&mut self, cx: &mut Cx, image: &ImageRef, id: LiveId, response: &HttpResponse, workers: Option<&Workers>) {
        let Some(url) = self.pending.remove(&id) else { return };
        let Some(body) = response.body.as_ref().filter(|_| response.status_code == 200) else { return };
        let bytes = body.clone();
        if url == self.wanted {
            let _ = image.load_image_from_data_async(cx, Path::new(&url), bytes.clone());
        }
        if let Some(workers) = workers {
            // A full queue only means this picture is fetched again later.
            workers.disk(DiskCommand::Keep { url, bytes });
        }
    }

    /// A request that failed: the URL may be asked for again.
    fn failed(&mut self, id: LiveId) {
        self.pending.remove(&id);
    }
}

// ---- app ----

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    /// Space was taken for play/pause on its way down: its release is the
    /// player's too, not a click on whatever control holds the focus.
    #[rust]
    space_for_play: bool,
    #[rust(Model::new(Theme::default()))]
    model: Model,
    #[rust]
    workers: Option<Workers>,
    #[rust]
    art: Art,
    /// Waits for typing to settle before the catalog is searched.
    #[rust]
    catalog_timer: Timer,
    /// Moves the position bar once a second while playing.
    #[rust]
    ticker: Timer,
    /// Watches the style's file while the style is pinned (`--style`).
    #[rust]
    style_watch: Timer,
    /// `AMP_TAP_TRACE=1`: once a second, how many frames the sound's tap
    /// wrote and how loud the newest were (does a silent source still
    /// call back?).
    #[rust]
    tap_trace: Timer,
    #[rust]
    tap_trace_last: u64,
    /// When the last playback poll came back, in app seconds.
    #[rust]
    last_poll: f64,
    /// What the chrome last showed, so labels are set only on change.
    #[rust]
    shown: HashMap<LiveId, String>,
    #[rust]
    device_labels: Vec<String>,
    /// The device just picked in the output list, and when: shown as the
    /// selection until Spotify reports it active.
    #[rust]
    device_pending: Option<(String, f64)>,
    /// What the open row menu does, resolved when it opened: the rows can
    /// be rebuilt under it (a new queue, a library snapshot).
    #[rust]
    menu_commands: Vec<(RowCommand, Activate)>,
    /// The upcoming track the Up Next menu is open on.
    #[rust]
    up_next_menu: Option<usize>,
    /// Whether the Up Next panel is shown.
    #[rust(true)]
    up_next_open: bool,
    /// What the system's own queue had next at the last poll: a track
    /// change to it is the system going on by itself.
    #[rust]
    player_next: Option<String>,
    /// Transport commands sent to the control worker, counted.
    #[rust]
    sent: u64,
    /// The shuffle and repeat colours last applied.
    #[rust]
    switch_colors: Option<(Vec4f, Vec4f)>,
    /// The equalizer power colour last applied.
    #[rust]
    power_color: Option<Vec4f>,
    /// Playback polls in a row that found the route passing only silence
    /// while the player plays: the sign that macOS withholds the sound.
    #[rust]
    silent_polls: u32,
    /// The chassis's margin is thin: a pocket-sized window.
    #[rust]
    tight: Option<bool>,
    /// The chassis's margins as last set: (tiny, the caption's height).
    #[rust]
    tight_key: Option<(bool, i64)>,
    /// A size-class switch under way: when it began and the skipped-draw
    /// count then (logged once the new layout draws whole).
    #[rust]
    switch_started: Option<(f64, u64)>,
    /// The chassis rail is registered as the window's caption.
    #[rust]
    rail_registered: bool,
    /// The rail's left room and whether it draws its own window buttons.
    #[rust]
    rail_sides: Option<(i64, bool)>,
    /// The rects the pocket sizes were last fitted to.
    #[rust]
    fit_rects: Option<[(i64, i64); 4]>,
    /// The rects moved while fitting: fit again after the next draw.
    #[rust]
    fit_again: NextFrame,
    /// The window size before the mini player or widget was chosen.
    #[rust]
    size_before_small: Option<Vec2d>,
    /// The draw after a layout switch's first (its fits applied).
    #[rust]
    refit_frame: NextFrame,
    /// Style and layout changes waiting for the window's picture to freeze.
    #[rust]
    pending_changes: Vec<Change>,
    /// The play queue as last kept on disk (or read back from it).
    #[rust]
    saved_queue: String,
    /// The number of the last play or seek handed to the local player.
    #[rust]
    player_seq: u64,
    /// The output device local files play to, and whether it is open.
    #[rust]
    output_devices: Vec<AudioDeviceId>,
    #[rust]
    output_open: bool,
    /// MilkDrop on the equalizer page (`milk.rs`).
    #[rust]
    milk: Milk,
    /// The visuals in a window of their own (`popout.rs`).
    #[rust]
    popout: Popout,
    /// The torch's settings (`torch.rs`), read on first use.
    #[rust]
    torch: Option<torch::TorchSettings>,
    /// The MilkDrop buttons' colours last applied.
    #[rust]
    milk_colors: Option<[Vec4f; 3]>,
    /// The showcase's timer (layouts, and styles, cycling).
    #[rust]
    showcase_timer: Timer,
    /// The fits last shown, and the window size the layout was fitted to.
    #[rust]
    fits_applied: Option<std::collections::HashMap<String, layout::Fit>>,
    #[rust]
    fitted_size: Option<(i64, i64)>,
    /// What the legends were last fitted for: style reload, window size.
    #[rust]
    legend_sig: Option<(u64, i64, i64)>,
    /// The page the EQ key left, to go back to.
    #[rust]
    before_eq: Option<Page>,
    /// The style on the device and its printed name.
    #[rust]
    style_title: (String, String),
    /// Whether the deck was last laid out for fullscreen.
    #[rust]
    milk_full_applied: Option<bool>,
    /// Whether the equalizer page was open at the last event.
    #[rust]
    on_eq_page: bool,
}

impl App {
    /// 1 when the output list leads with "Choose a device" (devices known,
    /// none active, nothing picked), else 0: the list row minus this is the
    /// device's index.
    fn device_row_offset(&self) -> usize {
        let none_active = !self.model.devices.iter().any(|device| device.active);
        (none_active && self.device_pending.is_none() && !self.model.devices.is_empty()) as usize
    }

    fn now(cx: &Cx) -> f64 {
        cx.seconds_since_app_start()
    }

    fn set_label(&mut self, cx: &mut Cx, id: LiveId, path: &[LiveId], text: &str) {
        let upper;
        let text = if self.model.theme.caps && CAPS.contains(&id) {
            upper = text.to_uppercase();
            upper.as_str()
        } else {
            text
        };
        if self.shown.get(&id).map(String::as_str) != Some(text) {
            self.shown.insert(id, text.to_string());
            self.ui.widget(cx, path).set_text(cx, text);
        }
    }

    /// The style picker: every style by name with a chip of its finish and
    /// light, the current one marked, hung under the style key (or at the
    /// top left when the key is folded away).
    /// The window's room for the device (the front, less nothing).
    fn device_size(&self, cx: &mut Cx) -> Vec2d {
        self.ui.widget(cx, ids!(app_bg)).area().rect_or_zero(cx).size
    }

    /// The window's inner size (caption included): what the size classes
    /// and a layout's `min:` measure, as `--size` and the person see it.
    fn window_size(&self, cx: &mut Cx) -> Vec2d {
        match self.ui.window(cx, ids!(main_window)).window_id() {
            Some(id) => cx.windows[id].window_geom.inner_size,
            None => self.device_size(cx),
        }
    }

    /// The layout picker: every layout, its minimum size, those that do not
    /// fit the window greyed; the showcase switches under them.
    fn open_layout_picker(&mut self, cx: &mut Cx) {
        let chosen = layout::chosen(cx);
        let mut rows = Vec::new();
        // Only a large window has a choice; smaller ones have one layout
        // each, picked by the window's size.
        if layout::active_class(cx) != layout::SizeClass::Large {
            rows.push(MenuRow::new(live_id!(layout_hint), "Make the window larger to choose a layout").enabled(false));
            rows.extend(self.size_rows());
            let anchor = self.controls_anchor(cx);
            self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(layout_menu), rows, anchor, MenuPlace::Below);
            return;
        }
        for entry in layout::layout_list().into_iter().filter(|e| e.class == layout::SizeClass::Large) {
            rows.push(MenuRow::new(LiveId::from_str(&entry.name), &entry.title).radio(entry.name == chosen));
        }
        rows.extend(self.size_rows());
        let (showcase, styles) = layout::showcase(cx);
        rows.push(MenuRow::separator());
        rows.push(MenuRow::new(live_id!(showcase), "Showcase: cycle layouts").checked(showcase).key("⇧⌘L"));
        rows.push(MenuRow::new(live_id!(showcase_styles), "Showcase: cycle styles too").checked(showcase && styles).key("⇧⌘Y"));
        let anchor = self.controls_anchor(cx);
        self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(layout_menu), rows, anchor, MenuPlace::Below);
    }

    /// The window sizes the small layouts are made for, as menu rows: the
    /// mini player and the widget (they resize the window), and the way back.
    fn size_rows(&self) -> Vec<MenuRow> {
        let mut rows = vec![MenuRow::separator(), MenuRow::new(live_id!(size_mini), "Mini player").key("⇧⌘M"), MenuRow::new(live_id!(size_widget), "Widget")];
        if self.size_before_small.is_some() {
            rows.push(MenuRow::new(live_id!(size_back), "Back to the full player").key("⇧⌘M"));
        }
        rows
    }

    /// Resize the window to the size `layout` is made for (its `pref:`),
    /// remembering the size to go back to.
    pub(crate) fn go_to_layout_size(&mut self, cx: &mut Cx, layout_name: &str) {
        let Some(pref) = layout::layout_list().into_iter().find(|e| e.name == layout_name).and_then(|e| e.pref) else { return };
        if matches!(layout::active_class(cx), layout::SizeClass::Medium | layout::SizeClass::Large) {
            self.size_before_small = Some(self.window_size(cx));
        }
        self.ui.window(cx, ids!(main_window)).resize(cx, pref);
    }

    /// Cmd+Shift+M: the mini player, or back to the size before it.
    fn toggle_mini_player(&mut self, cx: &mut Cx) {
        let small = matches!(layout::active_class(cx), layout::SizeClass::Small | layout::SizeClass::Tiny);
        match (small, self.size_before_small.take()) {
            (true, Some(size)) => self.ui.window(cx, ids!(main_window)).resize(cx, size),
            (true, None) => self.ui.window(cx, ids!(main_window)).resize(cx, dvec2(1360.0, 900.0)),
            _ => self.go_to_layout_size(cx, "mini"),
        }
    }

    /// Where the controls' menus open: under the controls in the caption.
    fn controls_anchor(&mut self, cx: &mut Cx) -> Rect {
        for id in [ids!(layout_cap), ids!(style_cap), ids!(controls_menu_cap)] {
            let key = self.ui.widget(cx, id);
            let rect = key.area().rect_or_zero(cx);
            if key.visible() && rect.size.x > 0.0 {
                return rect;
            }
        }
        Rect { pos: dvec2(24.0, 24.0), size: dvec2(160.0, 0.0) }
    }

    /// The controls fit the size class: the layout key only where there is
    /// a choice (large), one menu key in a tiny window.
    /// The caption is narrow: the style and the menu key only.
    pub(crate) fn caption_narrow(&self, cx: &mut Cx) -> bool {
        self.window_size(cx).x < 700.0
    }

    fn apply_controls(&mut self, cx: &mut Cx) {
        let class = layout::active_class(cx);
        // A tiny window, or one too narrow for the keys beside the window
        // buttons: one menu key.
        let narrow = self.caption_narrow(cx);
        let tiny = class == layout::SizeClass::Tiny || self.window_size(cx).x < 430.0;
        // The style is a direct control at every size; in a narrow window
        // everything else is in the menu key.
        self.set_visible(cx, ids!(style_cap), true);
        for id in [ids!(ai_cap), ids!(popout_cap)] {
            self.set_visible(cx, id, !narrow);
        }
        self.set_visible(cx, ids!(layout_cap), class == layout::SizeClass::Large && !narrow);
        self.set_visible(cx, ids!(controls_menu_cap), narrow);
        // Where the OS's window buttons are: macOS keeps its traffic lights
        // at the rail's left; elsewhere the rail draws its own at the right.
        let (left, own) = rail_sides(cx.os_type().clone());
        if self.rail_sides != Some((left as i64, own)) {
            self.rail_sides = Some((left as i64, own));
            let mut rail = self.ui.widget(cx, ids!(rail));
            script_apply_eval!(cx, rail, {padding: mod.turtle.Inset{left: #(left)}});
        }
        // Every pass: a restyle re-applies the rail's template (the keys hidden).
        self.set_visible(cx, ids!(winbtns), own);
        // The rail's title only where there is room for it (it still fills
        // the rail, so the keys stay at the right).
        self.set_label(cx, live_id!(rail_title), ids!(rail_title), if narrow { "" } else { "Makepad Amp" });
        let _ = tiny;
        // The rail is the caption: it drags the window, its keys are clicks.
        if !self.rail_registered {
            self.rail_registered = true;
            let rail = self.ui.widget(cx, ids!(rail));
            let clients: Vec<WidgetRef> = [ids!(connect_cap), ids!(style_cap), ids!(layout_cap), ids!(ai_cap), ids!(popout_cap), ids!(controls_menu_cap), ids!(feedback_cap), ids!(win_min_cap), ids!(win_max_cap), ids!(win_close_cap), ids!(torch)]
                .into_iter()
                .map(|id| self.ui.widget(cx, id))
                .collect();
            self.ui.window(cx, ids!(main_window)).set_drag_region(cx, Some((rail, clients)));
        }
    }

    /// Every control in one menu: the ≡ key in a tiny window, and a
    /// right-click on the chassis at any size.
    fn open_controls_menu(&mut self, cx: &mut Cx, anchor: Rect) {
        let current = hw::current(cx);
        let styles: Vec<MenuRow> = hw::style_list()
            .iter()
            .map(|entry| {
                let (fill, dot) = hw::swatch(&entry.name);
                MenuRow::new(LiveId::from_str(&format!("style:{}", entry.name)), &entry.title).radio(entry.name == current).swatch(fill, dot)
            })
            .collect();
        let mut rows = vec![MenuRow::new(live_id!(controls_style), "Style").submenu(styles).key("⌘T")];
        if layout::active_class(cx) == layout::SizeClass::Large {
            let chosen = layout::chosen(cx);
            let layouts: Vec<MenuRow> = layout::layout_list()
                .into_iter()
                .filter(|e| e.class == layout::SizeClass::Large)
                .map(|e| MenuRow::new(LiveId::from_str(&format!("layout:{}", e.name)), &e.title).radio(e.name == chosen))
                .collect();
            rows.push(MenuRow::new(live_id!(controls_layout), "Layout").submenu(layouts).key("⌘L"));
        }
        rows.push(MenuRow::separator());
        if self.caption_narrow(cx) {
            if let Some(row) = self.connection_menu_row(cx) {
                rows.push(row);
            }
        }
        rows.push(MenuRow::new(live_id!(controls_ai), "AI stylist").key("⌘I"));
        rows.push(MenuRow::new(live_id!(controls_popout), "Party").key("⇧⌘P"));
        let torch = self.torch_on();
        rows.push(MenuRow::new(live_id!(controls_torch), "Torch").checked(torch).key("⇧⌘L"));
        rows.push(MenuRow::new(live_id!(controls_eq), "Equalizer").key("⌘E"));
        rows.extend(self.size_rows());
        self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(controls_menu), rows, anchor, MenuPlace::Below);
    }

    fn controls_menu_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(controls_menu)).clicked(actions) {
            let anchor = self.controls_anchor(cx);
            self.open_controls_menu(cx, anchor);
        }
        let Some(id) = menu_picked(actions, live_id!(controls_menu)) else { return };
        if self.connection_menu_pick(cx, id) {
            return;
        }
        if id == live_id!(controls_ai) {
            crate::theme_manager::toggle_chat(cx, &self.ui);
        } else if id == live_id!(controls_popout) {
            self.toggle_popout(cx);
        } else if id == live_id!(controls_torch) {
            self.toggle_torch(cx);
        } else if id == live_id!(controls_eq) {
            self.toggle_eq();
        } else if let Some(entry) = hw::style_list().into_iter().find(|e| LiveId::from_str(&format!("style:{}", e.name)) == id) {
            self.transition(cx, Change::Style(entry.name.clone()));
        } else if let Some(entry) = layout::layout_list().into_iter().find(|e| LiveId::from_str(&format!("layout:{}", e.name)) == id) {
            self.switch_layout(cx, &entry.name, true);
        }
    }

    /// Show `name` (the person's pick or the showcase's next), moving the
    /// components there; what appears or goes fades.
    fn switch_layout(&mut self, cx: &mut Cx, name: &str, save: bool) {
        self.transition(cx, Change::Layout(name.to_string(), save));
    }

    /// A change of style or layout, made once the window's picture is
    /// frozen so the new one fades in over it.
    fn transition(&mut self, cx: &mut Cx, change: Change) {
        let first = self.pending_changes.is_empty();
        self.pending_changes.push(change);
        if first {
            self.ui.window(cx, ids!(main_window)).begin_crossfade(cx, 0.25, true);
        }
    }

    fn apply_changes(&mut self, cx: &mut Cx) {
        for change in std::mem::take(&mut self.pending_changes) {
            match change {
                Change::Style(name) => {
                    self.switch_started = Some((cx.seconds_since_app_start(), cx.pipeline_skips()));
                    hw::set_style(cx, &name)
                }
                Change::ShowStyle(name) => hw::show_style(cx, &name),
                Change::Layout(name, save) => self.apply_layout_now(cx, &name, save),
            }
        }
    }

    fn apply_layout_now(&mut self, cx: &mut Cx, name: &str, save: bool) {
        let size = self.window_size(cx);
        self.ui.hw_stage(cx, ids!(stage)).begin_tween(cx);
        if save {
            layout::choose(cx, name, size);
        } else {
            layout::show(cx, name);
        }
        self.apply_layout_in_place(cx);
        // The picker's crossfade reveals it once it is drawn.
        self.ui.window(cx, ids!(main_window)).start_crossfade(cx);
    }

    /// The window manager parked the window (`scale`, with the room `max` its
    /// slot has) or returned it to full size: show the largest compact form
    /// that fits (its layout: see `strip.splash`, `tile.splash`) and answer
    /// with it, or the full layout again. The size is the host's: nothing
    /// here asks for one.
    fn on_parked(&mut self, cx: &mut Cx, parked: makepad_widgets::makepad_platform::ime::HostedParked) {
        let forms = layout::compact_forms();
        let form = parked
            .scale
            .filter(|_| !layout::pinned(cx))
            .and_then(|_| makepad_widgets::makepad_platform::ime::pick_compact_form(&forms, parked.max));
        if layout::set_compact(cx, form.map(|i| forms[i].name.as_str())) {
            // Applied before this frame draws, and no frame shown until every
            // part of the layout can draw (the last whole frame stays).
            cx.present_whole_frames_for(1.0);
            self.apply_layout_in_place(cx);
            self.fits_applied = None;
            self.tight_key = None;
        }
        cx.answer_compact(form);
    }

    /// A layout change waiting to be made in place: the slots take the new
    /// arrangement, then what Rust sets on the layout's own widgets (the
    /// splitters) is set again. The components are untouched.
    fn apply_layout_in_place(&mut self, cx: &mut Cx) {
        let mut slots = self.ui.widget(cx, ids!(stage.slots));
        if !layout::apply_in_place(cx, &mut slots) {
            return;
        }
        self.restore_layout_splits(cx);
        self.apply_up_next(cx);
        self.fits_applied = None;
        self.tight = None;
        self.legend_sig = None;
        self.ui.redraw(cx);
        layout::switch_mark(cx, "in place: splitters restored");
    }

    fn toggle_showcase(&mut self, cx: &mut Cx, styles: bool) {
        let (on, with_styles) = layout::showcase(cx);
        let next = if styles { !(on && with_styles) } else { !on };
        layout::set_showcase(cx, next, if styles { next } else { with_styles && next });
        if next {
            self.showcase_timer = cx.start_interval(6.0);
        } else {
            cx.stop_timer(self.showcase_timer);
            self.showcase_timer = Timer::empty();
            // Back to the person's pick.
            let chosen = layout::chosen(cx);
            self.switch_layout(cx, &chosen, false);
        }
    }

    fn showcase_step(&mut self, cx: &mut Cx) {
        let size = self.device_size(cx);
        let list: Vec<String> = layout::layout_list()
            .into_iter()
            .filter(|e| e.class == layout::SizeClass::Large && size.x + 1.0 >= e.min.x && size.y + 1.0 >= e.min.y)
            .map(|e| e.name)
            .collect();
        if list.is_empty() {
            return;
        }
        let active = layout::active(cx);
        let at = list.iter().position(|n| *n == active).map(|i| (i + 1) % list.len()).unwrap_or(0);
        let next = list[at].clone();
        let (_, styles) = layout::showcase(cx);
        if styles {
            let all = hw::style_list();
            let cur = hw::current(cx);
            let i = all.iter().position(|e| e.name == cur).map(|i| (i + 1) % all.len()).unwrap_or(0);
            if let Some(entry) = all.get(i) {
                self.transition(cx, Change::ShowStyle(entry.name.clone()));
            }
        }
        self.switch_layout(cx, &next, false);
    }

    /// Keep the splitters of the layout on screen where the person left
    /// them (per layout).
    fn save_layout_splits(&mut self, cx: &mut Cx) {
        let name = layout::active(cx);
        let mut all: Vec<(String, String, String, f64)> = layout::saved_splits().into_iter().filter(|(l, _, _, _)| *l != name).collect();
        for id in layout::splitter_ids(&name) {
            let (kind, value) = match self.ui.splitter(cx, &[id]).align() {
                Some(SplitterAlign::FromA(v)) => ("a", v),
                Some(SplitterAlign::FromB(v)) => ("b", v),
                Some(SplitterAlign::Weighted(v)) => ("w", v),
                None => continue,
            };
            all.push((name.clone(), id.to_string(), kind.to_string(), (value * 1000.0).round() / 1000.0));
        }
        layout::save_splits(&all);
    }

    fn restore_layout_splits(&mut self, cx: &mut Cx) {
        let name = layout::active(cx);
        let saved = layout::saved_splits();
        // The file's own positions first (a splitter squeezed while the
        // window was smaller keeps no clamp), then where the person left them.
        let defaults = layout::splitter_defaults(&name).into_iter().map(|(id, kind, value)| (name.clone(), id, kind, value));
        for (l, id, kind, value) in defaults.chain(saved.into_iter()) {
            if l != name {
                continue;
            }
            let align = match kind.as_str() {
                "a" => SplitterAlign::FromA(value),
                "b" => SplitterAlign::FromB(value),
                _ => SplitterAlign::Weighted(value),
            };
            self.ui.splitter(cx, &[LiveId::from_str(&id)]).set_align(cx, align);
        }
    }

    /// Components show themselves to fit the room their layout gives them.
    fn apply_fits(&mut self, cx: &mut Cx) {
        // Only the tiny widget's chassis is slimmer (top and bottom); every
        // class keeps the style's frame at the sides.
        let class = layout::active_class(cx);
        let tight = class == layout::SizeClass::Tiny;
        let pocket = class == layout::SizeClass::Pocket;
        // A compact form has no rail (the host's window has its own handle).
        self.set_visible(cx, ids!(rail), !pocket);
        let key = (tight, pocket as i64);
        if self.tight_key != Some(key) {
            self.tight_key = Some(key);
            // The tiny widget keeps the style's frame at the sides (rack ears,
            // cheeks) and is only slimmer at the bottom and in its rail. A
            // compact form is a little frame all round.
            let (x, top, bottom, rail_h) = if pocket {
                (8.0, 6.0, 6.0, 0.0)
            } else if tight {
                (18.0, 0.0, 10.0, 28.0)
            } else {
                (18.0, 0.0, 16.0, 34.0)
            };
            let mut chassis = self.ui.widget(cx, ids!(app_bg));
            script_apply_eval!(cx, chassis, {padding: mod.turtle.Inset{left: #(x) right: #(x) top: #(top) bottom: #(bottom)}});
            let mut rail = self.ui.widget(cx, ids!(rail));
            script_apply_eval!(cx, rail, {height: #(rail_h)});
        }
        self.apply_controls(cx);
        self.apply_panels(cx);
        let fits = self.ui.hw_stage(cx, ids!(stage)).fits();
        let fit = |name: &str| fits.get(name).copied().unwrap_or(layout::Fit::Full);
        // The pocket sizes follow the rects themselves, not only the fits: a
        // row that narrows within one fit still resizes its keys.
        let stage = self.ui.hw_stage(cx, ids!(stage));
        let round = |r: Option<Rect>| r.map(|r| (r.size.x.round() as i64, r.size.y.round() as i64)).unwrap_or_default();
        let rects = [round(stage.shown("transport")), round(stage.shown("progress")), round(stage.shown("volume")), round(stage.shown("deck"))];
        let rects_same = self.fit_rects == Some(rects);
        self.fit_rects = Some(rects);
        // What this pass sets moves the rects the next draw lays out (the
        // deck's width decides its keys): look again once it has drawn.
        if !rects_same {
            self.fit_again = cx.new_next_frame();
        }
        if Some(fits.clone()) == self.fits_applied && rects_same {
            return;
        }
        self.fits_applied = Some(fits.clone());
        use layout::Fit::*;
        let transport = fit("transport");
        for id in [ids!(shuffle_led), ids!(repeat_led), ids!(radio_led), ids!(play_led)] {
            self.set_visible(cx, id, transport == Full);
        }
        for id in [ids!(shuffle_cap), ids!(repeat_cap), ids!(radio_cap)] {
            self.set_visible(cx, id, matches!(transport, Full | Compact));
        }
        // Pocket size: all six keys small, filling the row's height (the
        // shuffle, repeat and radio too where the row is wide enough); back
        // to the templates' sizes above it.
        let micro = transport == Micro;
        // The pads under previous and next stand in for the dashes under the
        // others: shown with them, so every key's column is as tall and the
        // keys share their top edge.
        for id in [ids!(previous_pad), ids!(next_pad)] {
            self.set_visible(cx, id, transport == Full);
        }
        let row_rect = self.ui.hw_stage(cx, ids!(stage)).shown("transport");
        let row = row_rect.map(|r| r.size.y).unwrap_or(40.0);
        let row_w = row_rect.map(|r| r.size.x).unwrap_or(0.0);
        // One row, one height: the play key is only wider. Keys fill the
        // row's height, and never more than its width holds:
        // three keys (1.3 + 1.7 + 1.3 heights wide) and two gaps: 12, or
        // closer in a narrow slot (a compact form at the host's width), where
        // the keys also give up their 18 pt floor so all three show whole.
        let gap = if micro { (row_w * 0.05).clamp(3.0, 12.0) } else { 12.0 };
        let key_h = if micro { (row - 4.0).min((row_w - 2.0 * gap - 6.0) / 4.3).clamp(12.0, 40.0) } else { 0.0 };
        let mut keys = self.ui.widget(cx, ids!(keys));
        script_apply_eval!(cx, keys, {spacing: #(gap)});
        if micro {
            let all_six = row_w >= key_h * 9.0 + 60.0;
            for id in [ids!(shuffle_cap), ids!(repeat_cap), ids!(radio_cap)] {
                self.set_visible(cx, id, all_six);
            }
        }
        for (id, full_w, full_h, ratio) in [(ids!(previous_cap), 70.0, 52.0, 1.3), (ids!(next_cap), 70.0, 52.0, 1.3), (ids!(play_cap), 96.0, 52.0, 1.7), (ids!(shuffle_cap), 60.0, 52.0, 1.3), (ids!(repeat_cap), 60.0, 52.0, 1.3), (ids!(radio_cap), 60.0, 52.0, 1.3)] {
            let (w, h) = if micro { ((key_h * ratio).round(), key_h) } else { (full_w, full_h) };
            // A small key's bevel is thinner, so its glyph stays legible.
            let inset = if micro { 3.0 } else { 7.0 };
            let mut key = self.ui.widget(cx, id);
            script_apply_eval!(cx, key, {width: #(w) height: #(h) inset: #(inset)});
        }
        let progress = fit("progress");
        let well_h = if progress == Micro { self.ui.hw_stage(cx, ids!(stage)).shown("progress").map(|r| (r.size.y - 2.0).max(12.0)).unwrap_or(24.0) } else { 38.0 };
        let mut well = self.ui.widget(cx, ids!(progress_well));
        script_apply_eval!(cx, well, {height: #(well_h)});
        // The theatre deck: the richest arrangement its width holds, from
        // every key in words with the preset's name, down to glyphs and short
        // mode names. EQ goes where the list area's tabs open them.
        let deck_w = self.ui.hw_stage(cx, ids!(stage)).shown("deck").map(|r| r.size.x).unwrap_or(0.0);
        let tabbed = self.ui.hw_stage(cx, ids!(stage)).panels().0.len() > 1;
        let tier = deck_tier(deck_w, tabbed);
        self.set_visible(cx, ids!(deck_name_window), tier.name);
        self.set_visible(cx, ids!(deck_random_cap), tier.random && !tier.more);
        self.set_visible(cx, ids!(deck_more_cap), tier.more);
        for (words, glyph, on, in_words) in [
            (ids!(deck_presets_cap), ids!(deck_presets_icon_cap), true, tier.words),
            (ids!(deck_eq_cap), ids!(deck_eq_icon_cap), !tabbed, tier.words),
            (ids!(deck_map_cap), ids!(deck_map_icon_cap), true, tier.long_words),
            (ids!(deck_auto_cap), ids!(deck_auto_icon_cap), true, tier.long_words),
        ] {
            self.set_visible(cx, words, on && in_words && !tier.more);
            self.set_visible(cx, glyph, on && !in_words && !tier.more);
        }
        let mut mode = self.ui.widget(cx, ids!(deck_mode));
        if tier.mode_words {
            script_apply_eval!(cx, mode, {options: ["Spectrum" "Visualizer"]});
        } else {
            script_apply_eval!(cx, mode, {options: ["Spec" "Vis"]});
        }
        let screen_pad = if fit("screen") == Micro { 4.0 } else { 14.0 };
        let mut screen = self.ui.widget(cx, ids!(screen));
        script_apply_eval!(cx, screen, {padding: mod.turtle.Inset{left: #(screen_pad) right: #(screen_pad) top: #(screen_pad) bottom: #(screen_pad)}});
        let now = fit("now");
        // A narrow slot (a compact form at the host's width): a small record
        // and the title on two lines instead of one cut to a few letters.
        let now_w = self.ui.hw_stage(cx, ids!(stage)).shown("now").map(|r| r.size.x).unwrap_or(f64::MAX);
        let narrow = now_w < 200.0;
        let art = if narrow { 36.0 } else { 58.0 };
        let title_lines = if narrow { 2.0 } else { 1.0 };
        let art_img = art - 8.0;
        let mut art_image = self.ui.widget(cx, ids!(art));
        script_apply_eval!(cx, art_image, {width: #(art_img) height: #(art_img)});
        let mut art_well = self.ui.widget(cx, ids!(art_well));
        script_apply_eval!(cx, art_well, {width: #(art) height: #(art)});
        let mut now_title = self.ui.widget(cx, ids!(now_title));
        script_apply_eval!(cx, now_title, {max_lines: #(title_lines)});
        self.set_visible(cx, ids!(now_context), now == Full && !narrow);
        self.set_visible(cx, ids!(now_artist), now != Tiny);
        self.set_visible(cx, ids!(art_well), now != Tiny);
        let volume = fit("volume");
        self.set_visible(cx, ids!(left_wing), volume != Micro);
        self.set_visible(cx, ids!(volume_slim), volume == Micro);
        let slim_h = self.ui.hw_stage(cx, ids!(stage)).shown("volume").map(|r| r.size.y).unwrap_or(0.0);
        // The well (22) and the meter (18) under it.
        self.set_visible(cx, ids!(meter_slim), slim_h >= 44.0);
        for id in [ids!(meter), ids!(brand), ids!(brand_sub)] {
            self.set_visible(cx, id, volume == Full);
        }
        let display = fit("display");
        for id in [ids!(milk_choosers), ids!(milk_now_window)] {
            self.set_visible(cx, id, display == Full);
        }
        self.set_visible(cx, ids!(milk_bar), display != Tiny);
        let progress = fit("progress");
        // A pocket line is the bar alone (its layout shows the clock).
        self.set_visible(cx, ids!(position), !matches!(progress, Tiny | Micro));
        self.set_visible(cx, ids!(length), !matches!(progress, Tiny | Micro));
        // The legends follow the new faces.
        self.legend_sig = None;
    }

    fn open_style_picker(&mut self, cx: &mut Cx) {
        let current = hw::current(cx);
        let list = hw::style_list();
        let mut rows = Vec::new();
        for entry in list.iter() {
            let (fill, dot) = hw::swatch(&entry.name);
            rows.push(MenuRow::new(LiveId::from_str(&entry.name), &entry.title).radio(entry.name == current).swatch(fill, dot));
        }
        crate::theme_manager::extend_style_menu(&mut rows);
        let key = self.ui.widget(cx, ids!(style_cap));
        let mut anchor = key.area().rect_or_zero(cx);
        if !key.visible() || anchor.size.x <= 0.0 {
            anchor = Rect { pos: dvec2(24.0, 24.0), size: dvec2(160.0, 0.0) };
        }
        self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(style_menu), rows, anchor, MenuPlace::Below);
    }

    fn apply_theme(&mut self, cx: &mut Cx) {
        // The chassis, its labels and keys take their look from the style's
        // templates; only what paints from the palette is set here.
        let accent = self.model.theme.accent;
        let _ = accent;
        // The presets list paints from the palette like the other lists.
        let theme = self.model.theme;
        let mut grid = self.ui.widget(cx, &[live_id!(milk_browser), live_id!(grid)]);
        widgets::style_grid(cx, &mut grid, &theme, theme.background);
        self.shown.clear();
        // The headings the layout prints, through the style's capitals.
        for (id, text) in [
            (live_id!(brand), "Makepad"),
            (live_id!(brand_sub), "Amp"),
        ] {
            let text = if id == live_id!(brand) || id == live_id!(brand_sub) { text.to_uppercase() } else { text.to_string() };
            self.set_label(cx, id, &[id], &text);
        }
        self.switch_colors = None;
        self.power_color = None;
        self.milk_colors = None;
    }

    /// Everything outside the two lists, from the model. Cheap: labels are
    /// only set when their text changed.
    fn update_chrome(&mut self, cx: &mut Cx) {
        let (title, _) = self.model.heading();
        self.set_label(cx, live_id!(title), ids!(title), &title);
        // The list's name rides on its tab key, cut short.
        let short: String = if title.chars().count() > 22 { format!("{}…", title.chars().take(21).collect::<String>().trim_end()) } else { title.clone() };
        let tab = if short.is_empty() { "Tracks".to_string() } else { short };
        self.set_label(cx, live_id!(tab_tracks), ids!(tab_tracks), &tab);

        // The sidebar's one line: only when something is wrong or under way
        // (the music system away included), never counts.
        let status = if let Some(error) = &self.model.error {
            error.clone()
        } else if !self.model.progress.is_empty() {
            self.model.progress.clone()
        } else if !self.model.local_progress.is_empty() {
            self.model.local_progress.clone()
        } else {
            self.model.away_note().unwrap_or_default()
        };
        self.set_visible(cx, ids!(status), !status.is_empty());
        self.set_label(cx, live_id!(status), ids!(status), &status);

        let playback = &self.model.playback;
        let (now_title, now_artist, now_context) = match &playback.now {
            Some(now) => (now.title.clone(), now.artist.clone(), if now.context.is_empty() { String::new() } else { format!("from {}", now.context) }),
            None => ("Nothing playing".to_string(), String::new(), String::new()),
        };
        let cached = playback.uri().is_some_and(|uri| self.model.index.is_cached(uri));
        let now_context = if cached && self.model.engine == Engine::Local { "Cached · offline playback".to_string() } else if cached { "Cached".to_string() } else { now_context };
        let art_url = playback.art_url.clone();
        let playing = playback.is_playing();
        let radio_on = playback.now.as_ref().is_some_and(|now| now.context.contains("Radio"));
        self.set_label(cx, live_id!(now_title), ids!(now_title), &now_title);
        self.set_label(cx, live_id!(now_artist), ids!(now_artist), &now_artist);
        self.set_label(cx, live_id!(now_context), ids!(now_context), &now_context);
        // Cast to another device: where it plays, and the parts that need
        // the sound here say why they rest.
        let remote = self.model.remote_device().map(|d| d.name.clone());
        let back = self.model.this_machine_device().is_some();
        self.set_visible(cx, ids!(now_cast), remote.is_some());
        self.set_visible(cx, ids!(remote_card_row), remote.is_some());
        self.set_visible(cx, ids!(remote_here_cap), back);
        self.set_visible(cx, ids!(eq_remote_row), remote.is_some());
        if let Some(name) = &remote {
            let on = format!("Playing on {name}");
            self.set_label(cx, live_id!(now_cast_name), ids!(now_cast_name), &on);
            self.set_label(cx, live_id!(remote_title), ids!(remote_title), &on);
            self.set_label(cx, live_id!(eq_remote), ids!(eq_remote), &format!("{on} · the equalizer applies to playback on this Mac"));
        }
        let marquee = if now_artist.is_empty() { now_title.clone() } else { format!("{now_artist} – {now_title}") };
        let marquee = match &remote {
            Some(name) => format!("{marquee} · on {name}"),
            None => marquee,
        };
        self.ui.hw_marquee(cx, ids!(marquee)).set(cx, &marquee, playing, None);
        self.set_label(cx, live_id!(play_pause), ids!(play_pause), if playing { ICON_PAUSE } else { ICON_PLAY });
        self.set_label(cx, live_id!(previous), ids!(previous), ICON_PREVIOUS);
        self.set_label(cx, live_id!(next), ids!(next), ICON_NEXT);
        self.set_label(cx, live_id!(radio), ids!(radio), ICON_RADIO);
        self.set_label(cx, live_id!(up_next_toggle), ids!(up_next_toggle), ICON_QUEUE);
        self.set_label(cx, live_id!(volume_icon), ids!(volume_icon), ICON_VOLUME);
        // The record stands in until there is art to show.
        self.set_visible(cx, ids!(art), !art_url.is_empty());
        let image = self.ui.image(cx, ids!(art));
        self.art.show(cx, &image, &art_url, self.workers.as_ref());

        // Shuffle, repeat, radio and play light their dashes when on.
        let leds = (self.model.shuffle, self.model.repeat != Repeat::Off);
        let _ = self.switch_colors;
        for (path, on) in [
            (ids!(shuffle_led), leds.0),
            (ids!(repeat_led), leds.1),
            (ids!(radio_led), radio_on),
            (ids!(play_led), playing),
            (ids!(shuffle_cap), leds.0),
            (ids!(repeat_cap), leds.1),
            (ids!(radio_cap), radio_on),
        ] {
            self.ui.relief_view(cx, path).set_lit(cx, if on { 1.0 } else { 0.0 });
        }
        // Mechanical keys stay down while their function is on.
        for (path, on) in [(ids!(play_cap), playing), (ids!(shuffle_cap), leds.0), (ids!(repeat_cap), leds.1), (ids!(radio_cap), radio_on)] {
            self.ui.relief_view(cx, path).set_latched(cx, if on { 1.0 } else { 0.0 });
        }
        // The style's printed name reads its file: only when the style changed.
        let style = hw::current(cx);
        if self.style_title.0 != style {
            self.style_title = (style.clone(), hw::label(&style).to_uppercase());
        }
        let title = self.style_title.1.clone();
        self.set_label(cx, live_id!(style_next), ids!(style_next), &title);
        self.set_label(cx, live_id!(shuffle), ids!(shuffle), ICON_SHUFFLE);
        self.set_label(cx, live_id!(repeat), ids!(repeat), if self.model.repeat == Repeat::Track { "\u{f01e}1" } else { ICON_REPEAT });

        // A picked device stays shown until the device list reports it
        // active (or 8 s pass): the poll lags the transfer.
        if let Some((id, at)) = &self.device_pending {
            let landed = self.model.devices.iter().any(|device| device.active && &device.id == id);
            if landed || cx.seconds_since_app_start() - *at > 8.0 {
                self.device_pending = None;
            }
        }
        let offset = self.device_row_offset();
        let mut devices: Vec<String> = self.model.devices.iter().map(|device| device.name.clone()).collect();
        if devices.is_empty() {
            devices.push("No device".to_string());
        } else if offset == 1 {
            // Nothing plays anywhere: every real device must be a change.
            devices.insert(0, "Choose a device".to_string());
        }
        let drop_down = self.ui.drop_down(cx, ids!(devices));
        if devices != self.device_labels {
            drop_down.set_labels(cx, devices.clone());
            self.device_labels = devices;
        }
        let shown = match &self.device_pending {
            Some((id, _)) => self.model.devices.iter().position(|device| &device.id == id),
            None => self.model.devices.iter().position(|device| device.active),
        };
        let row = shown.map(|at| at + offset).unwrap_or(0);
        if drop_down.selected_item() != row {
            drop_down.set_selected_item(cx, row);
        }
        let volume = self.model.volume_shown();
        self.ui.hw_knob(cx, ids!(volume)).set(cx, volume);
        let theme = self.model.theme;
        self.ui.music_bar(cx, ids!(volume_bar)).set(cx, volume, theme);

        self.update_equalizer(cx);
        self.update_progress(cx);
        self.update_ticker(cx);
    }

    fn update_progress(&mut self, cx: &mut Cx) {
        let now = Self::now(cx);
        // A local file's position is the player's own count, not an estimate
        // — once the decoder has taken the last play or seek; until then the
        // count is still the previous one's, and the start asked for shows.
        if self.model.engine == Engine::Local && self.model.playback.is_playing() {
            if let Some(position) = self.workers.as_ref().and_then(|workers| workers.shared.position_for(self.player_seq)) {
                self.model.playback.seek(position as f64, now);
            }
        }
        let playback = &self.model.playback;
        let length = playback.duration_ms as f64;
        let position = playback.position_ms(now);
        let fraction = match position {
            Some(position) if length > 0.0 => Some(position / length),
            // The length is known but not where in it playback is: the bar
            // can still seek, it just cannot show.
            None if length > 0.0 => Some(0.0),
            _ => None,
        };
        let position_text = position.map(|p| duration(p as u64)).unwrap_or_else(|| if length > 0.0 { "–:––".into() } else { String::new() });
        let length_text = if length > 0.0 { duration(length as u64) } else { String::new() };
        self.set_label(cx, live_id!(position), ids!(position), &position_text);
        self.set_label(cx, live_id!(length), ids!(length), &length_text);
        let theme = self.model.theme;
        self.ui.music_bar(cx, ids!(progress)).set(cx, fraction, theme);
        let playing = self.model.playback.is_playing();
        self.ui.hw_clock(cx, ids!(clock)).set(cx, position.map(|p| p / 1000.0), length / 1000.0, playing);
    }

    /// The position only moves while playing; at rest nothing redraws.
    fn update_ticker(&mut self, cx: &mut Cx) {
        let wanted = self.model.playback.is_playing() && self.model.playback.position_ms(Self::now(cx)).is_some();
        if wanted && self.ticker.is_empty() {
            self.ticker = cx.start_interval(1.0);
        } else if !wanted && !self.ticker.is_empty() {
            cx.stop_timer(self.ticker);
            self.ticker = Timer::empty();
        }
    }

    /// `false` when the command was not taken. With no music system there
    /// is nobody to take it, and the startup message already says why.
    fn send(&mut self, command: Command) -> bool {
        let Some(workers) = &self.workers else { return false };
        if workers.source.is_none() {
            return false;
        }
        let taken = workers.send(command);
        if !taken {
            self.model.error = Some("The music system is busy; try again".into());
        }
        taken
    }

    fn knobs(&self) -> Option<&audio::Knobs> {
        Some(&self.workers.as_ref()?.knobs)
    }

    fn save_eq(&self) {
        self.save(Setting::Equalizer, self.model.eq.serialize());
    }

    fn set_visible(&mut self, cx: &mut Cx, path: &[LiveId], visible: bool) {
        let widget = self.ui.widget(cx, path);
        if widget.visible() != visible {
            widget.set_visible(cx, visible);
        }
    }

    /// The equalizer page and the small spectrum, from the model.
    fn update_equalizer(&mut self, cx: &mut Cx) {
        self.update_eq(cx);
        self.update_connection(cx);
        let folders = self.model.page == Page::Folders;
        let _ = folders;
        let tracks_page = !matches!(self.model.page, Page::Equalizer | Page::Folders);
        let _ = tracks_page;
        self.set_visible(cx, ids!(add_folder_cap), self.model.page == Page::Folders);
        self.set_visible(cx, ids!(tracks_panel.header), self.model.page == Page::Folders);
        let theme = self.model.theme;
        let eq = self.model.eq;
        self.set_label(cx, live_id!(eq_power), ids!(eq_power), if eq.on { "EQ ON" } else { "EQ OFF" });
        if self.power_color != Some(if eq.on { theme.accent } else { theme.muted }) {
            self.power_color = Some(if eq.on { theme.accent } else { theme.muted });
            self.ui.relief_view(cx, ids!(eq_led)).set_lit(cx, if eq.on { 1.0 } else { 0.0 });
            self.ui.relief_view(cx, ids!(eq_power_key)).set_lit(cx, if eq.on { 1.0 } else { 0.0 });
        }
    }

    /// A route that opened but hears only zeros while the player plays is
    /// one macOS will not let hear the player: say where to allow it.
    fn check_silence(&mut self) {
        // Playing on another device, silence here is expected: nothing to
        // allow, and a note already shown goes.
        if self.model.remote_device().is_some() {
            self.silent_polls = 0;
            if self.model.error.as_deref() == Some(SILENCE_NOTE) {
                self.model.error = None;
                self.model.requests.redraw = true;
            }
            return;
        }
        let routed = matches!(self.model.audio, AudioState::Routed { .. });
        let Some(analyzer) = self.knobs().map(|knobs| knobs.route_sound.clone()).filter(|_| routed && self.model.engine == Engine::System && self.model.playback.is_playing()) else {
            self.silent_polls = 0;
            return;
        };
        let mut recent = [0.0f32; 4096];
        analyzer.latest(&mut recent);
        if recent.iter().any(|sample| *sample != 0.0) {
            self.silent_polls = 0;
            return;
        }
        self.silent_polls += 1;
        // Only macOS withholds the sound; elsewhere silence is Spotify's own.
        if self.silent_polls == 4 && cfg!(target_os = "macos") {
            // The status key says it (NEEDS PERMISSION); its sheet says how.
            self.model.error = Some(SILENCE_NOTE.into());
            self.model.requests.redraw = true;
        }
    }

    /// Act on what the widgets asked for.
    fn after_event(&mut self, cx: &mut Cx) {
        let requests = std::mem::take(&mut self.model.requests);
        let mut redraw = requests.redraw;
        for command in requests.transport {
            let now = Self::now(cx);
            // Shown at once; the next poll confirms it.
            match &command {
                Transport::Pause => self.model.playback.set_playing(false, now),
                Transport::Resume => self.model.playback.set_playing(true, now),
                Transport::Seek(ms) => self.model.playback.seek(*ms as f64, now),
                Transport::Previous => self.model.playback.restart(now),
                Transport::Play(uri) if self.model.playback.uri() == Some(uri.as_str()) => {
                    self.model.playback.restart(now);
                    // Shown already (optimistic): it counts from here.
                    self.model.pending_sent(uri, now);
                }
                _ => {}
            }
            self.model.error = None;
            let seq = self.sent + 1;
            if self.send(Command::Transport { seq, transport: command }) {
                self.sent = seq;
            }
            redraw = true;
        }
        for request in requests.player {
            self.run_player(cx, request);
            redraw = true;
        }
        if requests.knobs {
            if let Some(knobs) = self.knobs() {
                knobs.apply(&self.model.eq);
            }
        }
        if requests.save_eq {
            self.save_eq();
        }
        for folder in requests.remove_folders {
            if let Some(workers) = &self.workers {
                if !workers.local(LocalCommand::RemoveFolder(folder)) {
                    self.model.error = Some("The folder reader is busy; try again".into());
                }
            }
        }
        if requests.queue {
            self.save_queue();
        }
        if requests.queue && self.model.system_ready() {
            // The next tracks of ours the system should hold, while ours plays.
            let wanted = self.model.handed_over();
            let _ = self.send(Command::SyncQueue(wanted));
        }
        if let Some((seed, title)) = requests.radio {
            let seq = self.model.radio_seq;
            let _ = self.send(Command::Radio { seq, seed, title });
        }
        if requests.columns {
            self.save(Setting::Columns, self.model.columns.serialize());
        }
        if requests.catalog && self.model.system_ready() {
            if !self.catalog_timer.is_empty() {
                cx.stop_timer(self.catalog_timer);
            }
            self.catalog_timer = cx.start_timeout(CATALOG_DELAY);
        }
        let on_eq_page = self.model.page == Page::Equalizer;
        if on_eq_page != self.on_eq_page {
            self.on_eq_page = on_eq_page;
            self.milk_page_changed(cx);
            redraw = true;
        }
        if redraw {
            self.update_chrome(cx);
            self.ui.redraw(cx);
            self.drive_milk(cx);
        }
    }

    /// Do what local playback was asked: the decoder worker plays and seeks,
    /// the shared state pauses, the output device opens while a local file
    /// plays and closes when it pauses, stops or the system's player takes
    /// over.
    fn run_player(&mut self, cx: &mut Cx, request: PlayerRequest) {
        let now = Self::now(cx);
        let Some(workers) = &self.workers else { return };
        match request {
            PlayerRequest::Play { path, generation, now: playing, duration_ms, art_url } => {
                workers.shared.set_paused(false);
                let seq = self.player_seq + 1;
                if workers.player(PlayerCommand::Play { path, start_ms: 0, generation, seq }) {
                    self.player_seq = seq;
                } else {
                    self.model.error = Some("The local player is busy; try again".into());
                }
                self.model.playback.start(playing, duration_ms, art_url, 0.0, now);
                self.set_output(cx, true);
            }
            PlayerRequest::Pause => {
                workers.shared.set_paused(true);
                self.model.playback.set_playing(false, now);
                // Paused, nothing plays: an open output would run its
                // callback for silence and keep the sound hardware awake.
                self.set_output(cx, false);
            }
            PlayerRequest::Resume => {
                workers.shared.set_paused(false);
                self.model.playback.set_playing(true, now);
                self.set_output(cx, true);
            }
            PlayerRequest::Seek(ms) => {
                if let Some(path) = self.model.local_path.clone() {
                    let generation = self.model.local_generation;
                    let seq = self.player_seq + 1;
                    if workers.player(PlayerCommand::Play { path, start_ms: ms, generation, seq }) {
                        self.player_seq = seq;
                    }
                }
                self.model.playback.seek(ms as f64, now);
            }
            PlayerRequest::Stop => {
                workers.player(PlayerCommand::Stop);
                self.model.playback.set_playing(false, now);
                self.set_output(cx, false);
            }
        }
    }

    /// Open the output device local files play to, or close it: closed, the
    /// sound card's callback does not run at all.
    fn set_output(&mut self, cx: &mut Cx, open: bool) {
        if open != self.output_open {
            self.output_open = open;
            cx.use_audio_outputs(if open { &self.output_devices } else { &[] });
        }
    }

    /// Hand a setting to the disk worker; a full queue drops this save and
    /// the next change saves it all.
    fn save(&self, setting: Setting, text: String) {
        if let Some(workers) = &self.workers {
            workers.disk(DiskCommand::Save { setting, text });
        }
    }

    /// Keep the play queue for the next run, when it changed since it was
    /// last kept (or read back).
    fn save_queue(&mut self) {
        let text = self.model.queue.serialize();
        if text != self.saved_queue {
            self.save(Setting::Queue, text.clone());
            self.saved_queue = text;
        }
    }

    /// The Up Next panel as saved: `open 0|1`. Where the splitters sit is
    /// the layout's (`layout_splits.txt`); older files' `width` and `deck`
    /// lines are ignored.
    fn restore_up_next(&mut self, cx: &mut Cx, text: &str) {
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            if let (Some("open"), Some(open)) = (parts.next(), parts.next()) {
                self.up_next_open = open == "1";
            }
        }
        self.apply_up_next(cx);
    }

    fn apply_up_next(&mut self, cx: &mut Cx) {
        let collapse = if self.up_next_open { SplitterCollapse::None } else { SplitterCollapse::B };
        let split = self.ui.splitter(cx, ids!(split));
        if split.collapse() != collapse {
            split.set_collapse(cx, collapse);
        }
        let color = if self.up_next_open { self.model.theme.accent } else { self.model.theme.muted };
        let mut toggle = self.ui.widget(cx, ids!(up_next_toggle));
        script_apply_eval!(cx, toggle, {draw_text +: {color: #(color)}});
    }

    fn save_up_next(&mut self, _cx: &mut Cx) {
        self.save(Setting::UpNext, format!("open {}\n", self.up_next_open as u8));
    }

    /// Take what the workers published. Only a change redraws: polls that
    /// find everything as it was cost nothing on screen.
    fn poll_workers(&mut self, cx: &mut Cx) {
        let Some(workers) = &self.workers else { return };
        let messages = workers.poll();
        if messages.is_empty() {
            return;
        }
        let now = Self::now(cx);
        let mut index = None;
        let mut art = Vec::new();
        for message in messages {
            match message {
                // Snapshots replace each other: only the newest is shown.
                Message::Index(snapshot) => index = Some(snapshot),
                Message::Progress(text) => {
                    if text != self.model.progress {
                        self.model.progress = text;
                        self.model.requests.redraw = true;
                    }
                }
                Message::Playback { after, now: playing, player_next, detail } => {
                    let gap = if self.last_poll > 0.0 { now - self.last_poll } else { 1.0 };
                    self.last_poll = now;
                    // While a local file plays the system's player is paused
                    // and its state is not what the bar shows, unless the
                    // person started it again there: then it takes over.
                    if self.model.engine == Engine::Local {
                        if playing.as_ref().is_some_and(|p| p.is_playing) && after >= self.sent {
                            if playing.as_ref().is_some_and(|p| self.model.playback.uri() == Some(p.uri.as_str()) && self.model.index.local_file(&p.uri).is_some()) {
                                // The system's pause can lag its command
                                // while our file of the same track plays:
                                // the file keeps the control.
                                self.model.requests.transport.push(Transport::Pause);
                                self.player_next = player_next;
                                continue;
                            }
                            self.model.system_took_over();
                        } else {
                            self.player_next = player_next;
                            continue;
                        }
                    }
                    // Taken before a command the UI sent since: it shows the
                    // state that command changed. The poll after it follows.
                    if after < self.sent {
                        continue;
                    }
                    // A track the index knows needs no lookup for its art.
                    let known = playing.as_ref().and_then(|p| self.model.index.track(&p.uri)).map(|track| source::TrackDetail {
                        uri: track.uri.clone(),
                        duration_ms: track.duration_ms,
                        art_url: track.art_url.clone(),
                        album: track.album.clone(),
                        album_uri: track.album_uri.clone(),
                        release: track.release,
                        artist_uri: track.artist_uri.clone(),
                    });
                    let detail = detail.or(known.filter(|d| d.duration_ms > 0));
                    // A play shown before the system confirmed it: held
                    // while the system switches, rolled back when it did not.
                    if self.model.reconcile(playing.as_ref().map(|p| p.uri.as_str()), now, gap, false) {
                        self.player_next = player_next;
                        continue;
                    }
                    let playback = &self.model.playback;
                    let before = (playback.now.clone(), playback.duration_ms, playback.art_url.clone(), playback.position_ms(now).is_some());
                    self.model.playback.update(playing, detail.as_ref(), now, gap);
                    let playback = &self.model.playback;
                    let after_update = (playback.now.clone(), playback.duration_ms, playback.art_url.clone(), playback.position_ms(now).is_some());
                    // A new track: our queue follows it. It ran on by itself
                    // when it is what the system's own queue had next.
                    if let Some(uri) = self.model.playback.uri().filter(|uri| before.0.as_ref().map(|n| n.uri.as_str()) != Some(*uri)).map(str::to_string) {
                        let ran_on = self.player_next.as_deref() == Some(uri.as_str());
                        self.model.follow_player(&uri, ran_on);
                    }
                    self.player_next = player_next;
                    self.check_silence();
                    self.connection_tick(cx);
                    if before.0 != after_update.0 {
                        self.model.changed();
                    } else if before != after_update {
                        self.model.requests.redraw = true;
                    }
                }
                Message::Devices { after, devices } => {
                    if after >= self.sent && devices != self.model.devices {
                        self.model.devices = devices;
                        self.model.requests.redraw = true;
                    }
                }
                Message::Catalog { seq, results } => self.model.set_catalog(seq, results),
                Message::Radio { seq, title, tracks } => match tracks {
                    Ok(tracks) => {
                        let items = tracks.into_iter().map(|track| QueueItem { uri: track.uri, name: track.name, artist: track.artists.join(", ") }).collect();
                        self.model.set_radio(seq, &title, items);
                    }
                    Err(error) => {
                        self.model.error = Some(format!("No radio for {title}: {error}"));
                        self.model.requests.redraw = true;
                    }
                },
                Message::Art { url, bytes } => art.push((url, bytes)),
                Message::Saved { setting: Setting::Equalizer, text } => {
                    let settings = EqSettings::parse(&text);
                    self.model.eq = settings;
                    if let Some(knobs) = self.knobs() {
                        knobs.apply(&settings);
                    }
                    // The audio connection is its own switch: back on only
                    // when it was connected and macOS already allows it.
                    self.connection_restore(cx);
                    self.model.changed();
                }
                Message::Audio(state) => {
                    if let AudioState::Failed(error) = &state {
                        log!("amp: {error}");
                    }
                    self.model.audio = state;
                    self.silent_polls = 0;
                    self.model.changed();
                }
                Message::Saved { setting: Setting::Columns, text } => {
                    if self.model.columns.parse(&text) {
                        // Read back, not changed: nothing to save.
                        self.model.columns_changed(false);
                    }
                }
                Message::Saved { setting: Setting::UpNext, text } => self.restore_up_next(cx, &text),
                Message::Saved { setting: Setting::Visualizer, text } => self.restore_milk(cx, &text),
                // The last run's queue, parked: shown, steering nothing,
                // unless something was played before it was read.
                Message::Saved { setting: Setting::Queue, text } => {
                    let untouched = self.model.queue.current.is_none() && self.model.queue.upcoming_len() == 0;
                    if let Some(queue) = PlayQueue::parse(&text).filter(|_| untouched) {
                        self.model.queue = queue;
                        self.saved_queue = text;
                        self.model.changed();
                    }
                }
                Message::Availability(availability) => self.model.set_availability(availability),
                Message::LocalProgress(text) => {
                    if text != self.model.local_progress {
                        self.model.local_progress = text;
                        self.model.requests.redraw = true;
                    }
                }
                Message::Folders(folders) => self.model.set_folders(folders),
                Message::LocalEnded { generation } => self.model.local_ended(generation),
                Message::LocalError { generation, error } => {
                    log!("amp: {error}");
                    self.model.error = Some(error);
                    self.model.local_failed(generation);
                }
                Message::Error(error) => {
                    // A command failed while a play shows ahead of the
                    // system: the system's own track comes back.
                    if self.model.pending.is_some() {
                        let polled = self.model.pending.as_ref().and_then(|p| p.previous.uri().map(str::to_string));
                        self.model.reconcile(polled.as_deref(), Self::now(cx), 0.0, true);
                    }
                    if self.model.error.as_ref() != Some(&error) {
                        log!("amp: {error}");
                        self.model.error = Some(error);
                        self.model.requests.redraw = true;
                    }
                }
            }
        }
        if let Some(index) = index {
            self.model.set_index(index);
        }
        if !art.is_empty() {
            let image = self.ui.image(cx, ids!(art));
            for (url, bytes) in art {
                self.art.found(cx, &image, url, bytes);
            }
        }
    }

    /// Raise a menu of commands at `abs`; what they do is kept until one is
    /// picked, since the rows may change under an open menu.
    fn open_menu(&mut self, cx: &mut Cx, commands: Vec<(RowCommand, Activate)>, abs: DVec2) {
        if commands.is_empty() {
            return;
        }
        let rows: Vec<MenuRow> = commands.iter().map(|(command, _)| MenuRow::new(LiveId::from_str(command.label()), command.label())).collect();
        self.menu_commands = commands;
        let anchor = Rect { pos: abs, size: dvec2(0.0, 0.0) };
        self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(row_menu), rows, anchor, MenuPlace::At);
    }

    /// Size and centre every key's legend on the key's face (its quad less
    /// the margin): glyphs to a share of the face's short side, words to
    /// fit its width too.
    fn fit_legends(&mut self, cx: &mut Cx) -> bool {
        let theme = self.model.theme;
        if !theme.legend_fit {
            return true;
        }
        let mut measured = true;
        for &(cap, button, word, transport) in LEGENDS.iter() {
            // A mechanical key prints its legend in the label window on its
            // top: centred in that window at any key size (and moving with
            // the key, as its children do), sized to it, and in an ink that
            // reads on the window in every state.
            if transport && self.fit_mechanical_legend(cx, cap, button) {
                continue;
            }
            if transport && !theme.legend_transport {
                continue;
            }
            let key = self.ui.relief_view(cx, &[cap]);
            let Some((rect, inset, visible)) = key.borrow().map(|k| (k.view.area().rect_or_zero(cx), k.inset, k.view.visible)) else { continue };
            if !visible {
                continue;
            }
            if rect.size.x < 4.0 || rect.size.y < 4.0 {
                measured = false;
                continue;
            }
            let face_w = (rect.size.x - 2.0 * inset).max(4.0);
            let face_h = (rect.size.y - 2.0 * inset).max(4.0);
            let short = face_w.min(face_h) as f32;
            let mut widget = self.ui.widget(cx, &[button]);
            let size = if word {
                let chars = widget.text().chars().count().max(1) as f32;
                (short * theme.legend_text_scale).min(face_w as f32 * 0.86 / (chars * 0.62))
            } else {
                short * theme.legend_scale
            }
            .clamp(5.0, 24.0) as f64;
            let shift = (theme.legend_offset * short * 2.0) as f64;
            let (top, bottom) = if shift >= 0.0 { (shift, 0.0) } else { (0.0, -shift) };
            // A word on a key with an LED at its top-left keeps clear of it.
            let led = word && LED_KEYS.contains(&cap);
            let left = if led { 12.0 } else { 0.0 };
            script_apply_eval!(cx, widget, {
                draw_text +: {text_style +: {font_size: #(size)}}
                padding: mod.turtle.Inset{left: #(left) right: 0.0 top: #(top) bottom: #(bottom)}
            });
        }
        measured
    }

    /// A mechanical key's legend in its label window; `false` for a key
    /// that has no window (not mechanical in this style).
    fn fit_mechanical_legend(&mut self, cx: &mut Cx, cap: LiveId, button: LiveId) -> bool {
        let key = self.ui.relief_view(cx, &[cap]);
        let Some((inset, mech, window, rect)) = key.borrow().map(|k| {
            let mut mech = [0.0f32; 4];
            k.view.draw_bg.draw_vars.get_uniform(cx, live_id!(mech), &mut mech);
            let mut window = [0.0f32; 4];
            k.view.draw_bg.draw_vars.get_uniform(cx, live_id!(window_ink), &mut window);
            (k.inset, mech, window, k.view.area().rect_or_zero(cx))
        }) else {
            return false;
        };
        if rect.size.y < 4.0 {
            return false;
        }
        if mech[0] < 0.5 || mech[3] < 0.5 {
            return false;
        }
        // The window (hw.rs `mech_key`): from inset + 2.5, `mech.w` high.
        // The button's content box is made the window itself, and the
        // glyph's ink is centred in it (the key's children move with its
        // travel, and so does the window).
        let wh = mech[3] as f64;
        let size = (wh * 0.62).clamp(5.0, 14.0);
        // (The icon font's ink sits low in its box: a sixth of the window
        // up puts it in the middle.)
        let lift = wh / 6.0;
        let top = (inset + 2.5 - lift).max(0.0);
        let bottom = (rect.size.y - (inset + 2.5) - wh + lift).max(0.0);
        let ink = if hw::contrast(vec4(0.85, 0.86, 0.88, 1.0), vec4(window[0], window[1], window[2], 1.0)) >= 4.5 {
            vec4(0.85, 0.86, 0.88, 1.0)
        } else {
            vec4(0.08, 0.08, 0.09, 1.0)
        };
        let bright = vec4((ink.x + 0.15).min(1.0), (ink.y + 0.15).min(1.0), (ink.z + 0.15).min(1.0), 1.0);
        let mut widget = self.ui.widget(cx, &[button]);
        script_apply_eval!(cx, widget, {
            align: mod.turtle.Align{x: 0.5 y: 0.5}
            padding: mod.turtle.Inset{left: 0.0 right: 0.0 top: #(top) bottom: #(bottom)}
            draw_text +: {
                text_style +: {font_size: #(size)}
                color: #(ink) color_hover: #(bright) color_down: #(bright) color_focus: #(ink)
            }
        });
        true
    }

    /// The picture in the style's colours, or its own: the map, the key's
    /// light.
    fn apply_color_map(&mut self, cx: &mut Cx) {
        let (map, amount, filter, light_tint) = hw::color_map(cx);
        let (m0, m1, m2, m3) = (map[0], map[1], map[2], map[3]);
        let amount = amount as f64;
        let mut milk = self.ui.widget(cx, ids!(milk));
        script_apply_eval!(cx, milk, {map0: #(m0) map1: #(m1) map2: #(m2) map3: #(m3) map_amount: #(amount) filter: #(filter) light_tint: #(light_tint)});
        let on = amount > 0.0 || filter.y > 0.0;
        for cap in [ids!(map_cap), ids!(deck_map_cap), ids!(deck_map_icon_cap)] {
            self.ui.relief_view(cx, cap).set_lit(cx, if on { 1.0 } else { 0.0 });
        }
        // What a style's glass draws over the picture comes with the filter.
        let filter_on = if on { 1.0 } else { 0.0 };
        let mut glass = self.ui.widget(cx, ids!(screen_glass));
        script_apply_eval!(cx, glass, {draw_bg +: {filter_on: #(filter_on)}});
    }

    /// The row menu with the list's "available locally" switch under it.
    fn open_menu_with_filter(&mut self, cx: &mut Cx, commands: Vec<(RowCommand, Activate)>, abs: DVec2) {
        let mut rows: Vec<MenuRow> = commands.iter().map(|(command, _)| MenuRow::new(LiveId::from_str(command.label()), command.label())).collect();
        rows.push(MenuRow::separator());
        rows.push(MenuRow::new(live_id!(filter_local), "Available Locally Only").checked(self.model.filter == Filter::Local).key("⌘U"));
        self.menu_commands = commands;
        let anchor = Rect { pos: abs, size: dvec2(0.0, 0.0) };
        self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(row_menu), rows, anchor, MenuPlace::At);
    }

    fn toggle_filter(&mut self) {
        let next = if self.model.filter == Filter::Local { Filter::All } else { Filter::Local };
        self.model.set_filter(next);
    }

    /// The radio of the picked row of the open page, or of what plays.
    fn radio(&mut self, row: Option<usize>) {
        match self.model.radio_for(row) {
            Some(radio) => self.model.run(radio),
            None => {
                self.model.error = Some("Pick a track, or play one, for its radio".into());
                self.model.requests.redraw = true;
            }
        }
    }

    fn toggle_play(&mut self) {
        if !self.model.playback.is_playing() && (self.model.resume_parked() || self.model.play_offline()) {
            return;
        }
        let command = if self.model.playback.is_playing() { Transport::Pause } else { Transport::Resume };
        self.model.transport(command);
    }

    /// Whether the visualisation fills the screen: the visualiser's own
    /// fullscreen, or the main window made fullscreen.
    fn fullscreen_view(&self, cx: &Cx) -> bool {
        self.milk.fullscreen
            || self.ui.window(cx, ids!(main_window)).window_id().is_some_and(|id| cx.windows[id].window_geom.is_fullscreen)
    }

    /// Any text field with the keys (the search fields, the AI stylist's
    /// composer, a name field): bare keys and the text-editing chords are
    /// its, not the player's.
    fn typing(&self, cx: &mut Cx) -> bool {
        self.ui.text_input(cx, ids!(search)).key_focus(cx) || cx.text_input_has_focus()
    }

    fn handle_key(&mut self, cx: &mut Cx, key: &KeyEvent) {
        let search = self.ui.text_input(cx, ids!(search));
        let typing = self.typing(cx);
        let now = Self::now(cx);
        match key.key_code {
            KeyCode::KeyF if key.modifiers.is_primary() => {
                self.model.open(Page::Search);
                search.set_key_focus(cx);
            }
            KeyCode::KeyR if key.modifiers.is_primary() => {
                let picked = self.ui.track_table(cx, ids!(tracks)).picked();
                self.radio(picked);
            }
            KeyCode::Space if !typing && !key.modifiers.is_primary() => self.toggle_play(),
            KeyCode::Escape if self.milk.fullscreen => self.set_milk_fullscreen(cx, false),
            // In a fullscreen view (the visualiser's own, or the whole window
            // made fullscreen) the arrows step the visualisation. The
            // visualiser's fullscreen hides every text field, so one that
            // kept the focus does not take the arrows there.
            KeyCode::ArrowRight | KeyCode::ArrowLeft
                if !key.modifiers.is_primary() && self.fullscreen_view(cx) && (self.milk.fullscreen || !typing) =>
            {
                self.visual_step(cx, if key.key_code == KeyCode::ArrowRight { 1 } else { -1 });
            }
            KeyCode::Escape if typing => {
                search.set_text(cx, "");
                self.model.set_query("");
            }
            KeyCode::ArrowRight if key.modifiers.is_primary() && !typing => self.model.next(),
            KeyCode::ArrowLeft if key.modifiers.is_primary() && !typing => self.model.previous(now),
            // Restyle from the style's file as it is now.
            KeyCode::F5 => cx.request_style_reload(),
            // The style picker, from the keyboard (arrows and Return pick).
            KeyCode::KeyT if key.modifiers.is_primary() => self.open_style_picker(cx),
            // The list's commands and pages, without their buttons.
            KeyCode::ReturnKey if key.modifiers.is_primary() && !typing => {
                if let Some((_, play)) = self.model.page_commands().into_iter().next() {
                    self.model.run(play);
                }
            }
            KeyCode::KeyU if key.modifiers.is_primary() => self.toggle_filter(),
            KeyCode::KeyP if key.modifiers.is_primary() && key.modifiers.shift => self.toggle_popout(cx),
            KeyCode::KeyL if key.modifiers.is_primary() && key.modifiers.shift => self.toggle_torch(cx),
            // The layout picker; with Shift, the showcase (layouts cycling).
            KeyCode::KeyL if key.modifiers.is_primary() && key.modifiers.shift => self.toggle_showcase(cx, false),
            KeyCode::KeyM if key.modifiers.is_primary() && key.modifiers.shift => self.toggle_mini_player(cx),
            KeyCode::KeyL if key.modifiers.is_primary() => self.open_layout_picker(cx),
            KeyCode::KeyY if key.modifiers.is_primary() && key.modifiers.shift => self.toggle_showcase(cx, true),
            KeyCode::KeyE if key.modifiers.is_primary() => self.toggle_eq(),
            KeyCode::KeyO if key.modifiers.is_primary() => self.model.open(Page::Folders),
            KeyCode::Backspace if key.modifiers.is_primary() && key.modifiers.shift => self.model.edit_queue(|queue| queue.clear()),
            _ => {}
        }
    }
}

/// Where a window's buttons go on the chassis rail: the room to leave at
/// its left (macOS draws its traffic lights there) and whether the rail
/// draws its own minimise / maximise / close at its right (the OS draws none
/// on a hidden caption elsewhere).
fn rail_sides(os: OsType) -> (f64, bool) {
    match os {
        OsType::Macos => (64.0, false),
        _ => (12.0, true),
    }
}

#[cfg(test)]
mod rail_tests {
    use super::*;

    #[test]
    fn window_buttons_go_on_the_platform_side() {
        assert_eq!(rail_sides(OsType::Macos), (64.0, false));
        assert_eq!(rail_sides(OsType::Windows), (12.0, true));
        assert_eq!(rail_sides(OsType::LinuxWindow(Default::default())), (12.0, true));
    }
}

/// How the theatre deck is set out at a width.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DeckTier {
    /// The mode names in full.
    mode_words: bool,
    /// PRESETS and EQ in words.
    words: bool,
    /// THEME COLOURS and CHANGE in words.
    long_words: bool,
    random: bool,
    name: bool,
    /// The rest (random, PRESETS, THEME COLOURS, CHANGE, EQ) in one
    /// menu key.
    more: bool,
}

/// The richest deck that fits `width` (EQ left out when the
/// list area's tabs open them).
fn deck_tier(width: f64, tabbed: bool) -> DeckTier {
    const GAP: f64 = 8.0;
    const KEY: f64 = 42.0;
    let need = |t: &DeckTier| {
        let mut parts = vec![if t.mode_words { 200.0 } else { 125.0 }, KEY, KEY];
        if t.random {
            parts.push(KEY);
        }
        parts.push(if t.words { 92.0 } else { KEY });
        if !tabbed {
            parts.push(if t.words { 58.0 } else { KEY });
            parts.push(if t.words { 98.0 } else { KEY });
        }
        parts.push(if t.long_words { 128.0 } else { KEY });
        parts.push(if t.long_words { 118.0 } else { KEY });
        if t.name {
            parts.push(160.0);
        }
        if t.more {
            parts = vec![125.0, KEY, KEY, KEY];
        }
        parts.iter().sum::<f64>() + GAP * (parts.len() - 1) as f64
    };
    let tiers = [
        DeckTier { mode_words: true, words: true, long_words: true, random: true, name: true, more: false },
        DeckTier { mode_words: true, words: true, long_words: true, random: true, name: false, more: false },
        DeckTier { mode_words: true, words: true, long_words: false, random: true, name: false, more: false },
        DeckTier { mode_words: true, words: false, long_words: false, random: true, name: false, more: false },
        DeckTier { mode_words: false, words: false, long_words: false, random: true, name: false, more: false },
    ];
    tiers.iter().copied().find(|t| need(t) <= width).unwrap_or(DeckTier { mode_words: false, words: false, long_words: false, random: false, name: false, more: true })
}

#[cfg(test)]
mod deck_tests {
    use super::*;

    #[test]
    fn the_deck_gives_way_from_words_to_glyphs() {
        assert!(deck_tier(1400.0, false).name);
        let mid = deck_tier(650.0, false);
        assert!(!mid.name && !mid.long_words);
        let narrow = deck_tier(330.0, true);
        assert!(!narrow.mode_words && !narrow.words);
        assert!(deck_tier(280.0, true).more);
        // Tabs free the room EQ took.
        assert!(deck_tier(700.0, true).words || !deck_tier(700.0, false).words);
    }
}

/// What the sidebar says when the route hears only zeros while Spotify plays.
const SILENCE_NOTE: &str = "Amp hears only silence from Spotify: press SPOTIFY in the top bar to allow it";

/// Keys with an LED at the face's top-left (their word legends keep clear).
const LED_KEYS: [LiveId; 8] = [
    live_id!(map_cap),
    live_id!(eq_page_cap),
    live_id!(milk_list_cap),
    live_id!(eq_power_key),
    live_id!(eq_auto_cap),
    live_id!(tog_eq_cap),
    live_id!(tog_pl_cap),
    live_id!(tog_vis_cap),
];

/// Labels a style's `caps` shows in capitals: titles and headings.
const CAPS: [LiveId; 4] = [
    live_id!(title),
    live_id!(now_title),
    live_id!(brand),
    live_id!(brand_sub),
];

/// The keys whose legends follow their face: (cap, button, legend is a
/// word, transport key).
const LEGENDS: [(LiveId, LiveId, bool, bool); 27] = [
    (live_id!(shuffle_cap), live_id!(shuffle), false, true),
    (live_id!(previous_cap), live_id!(previous), false, true),
    (live_id!(play_cap), live_id!(play_pause), false, true),
    (live_id!(next_cap), live_id!(next), false, true),
    (live_id!(repeat_cap), live_id!(repeat), false, true),
    (live_id!(radio_cap), live_id!(radio), false, true),
    (live_id!(milk_prev_cap), live_id!(milk_prev), false, false),
    (live_id!(milk_random_cap), live_id!(milk_random), false, false),
    (live_id!(milk_next_cap), live_id!(milk_next), false, false),
    (live_id!(milk_full_cap), live_id!(milk_full), false, false),
    (live_id!(milk_fav_cap), live_id!(milk_fav), false, false),
    (live_id!(milk_lock_cap), live_id!(milk_lock), false, false),
    (live_id!(milk_list_cap), live_id!(milk_list), true, false),
    (live_id!(up_next_cap), live_id!(up_next_toggle), false, false),
        (live_id!(tog_eq_cap), live_id!(tog_eq), true, false),
    (live_id!(tog_pl_cap), live_id!(tog_pl), true, false),
    (live_id!(tog_vis_cap), live_id!(tog_vis), true, false),
    (live_id!(eq_page_cap), live_id!(eq_page_key), true, false),
    (live_id!(map_cap), live_id!(map_key), true, false),
    (live_id!(eq_power_key), live_id!(eq_power), true, false),
    (live_id!(eq_flat_cap), live_id!(eq_flat), true, false),
    (live_id!(eq_auto_cap), live_id!(eq_auto), true, false),
    (live_id!(eq_save_cap), live_id!(eq_save), true, false),
    (live_id!(eq_close_cap), live_id!(eq_close), false, false),
    (live_id!(add_folder_cap), live_id!(add_folder), true, false),
    (live_id!(drawer_close_cap), live_id!(milk_drawer_close), false, false),
    (live_id!(drawer_close_cap), live_id!(milk_drawer_close), false, false),
];

/// A rect that is empty for an area that drew nothing (a hidden widget),
/// instead of an error in the log.
trait AreaRectOrZero {
    fn rect_or_zero(&self, cx: &Cx) -> Rect;
}

impl AreaRectOrZero for Area {
    fn rect_or_zero(&self, cx: &Cx) -> Rect {
        if self.is_valid(cx) {
            self.rect(cx)
        } else {
            Rect::default()
        }
    }
}

/// `--size WxH` or `--size=WxH`.
fn size_from_args() -> Option<Vec2d> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        let value = match arg.strip_prefix("--size=") {
            Some(value) => value.to_string(),
            None if arg == "--size" => args.next()?,
            None => continue,
        };
        let (width, height) = value.split_once(['x', 'X'])?;
        return Some(dvec2(width.trim().parse().ok()?, height.trim().parse().ok()?));
    }
    None
}

/// The commands of the Up Next panel's row menu.
fn up_next_commands() -> [(&'static str, LiveId); 4] {
    [("Play", live_id!(up_play)), ("Play Next", live_id!(up_first)), ("Remove", live_id!(up_remove)), ("Clear Up Next", live_id!(up_clear))]
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.window(cx, ids!(main_window)).crossfade_ready(actions) {
            self.apply_changes(cx);
        }
        crate::theme_manager::handle_actions(cx, actions, &self.ui);
        self.milk_actions(cx, actions);
        self.popout_actions(cx, actions);
        self.torch_actions(cx, actions);
        self.connection_actions(cx, actions);
        let search = self.ui.text_input(cx, ids!(search));
        if let Some(query) = search.changed(actions) {
            self.model.set_query(&query);
        }

        if self.ui.button(cx, ids!(add_folder)).clicked(actions) {
            cx.open_select_folder_dialog(FileDialog::new().set_title("Add a Folder of Music".into()));
        }
        for action in actions {
            if let Some(FileDialogAction::FolderSelected(path)) = action.downcast_ref::<FileDialogAction>() {
                let folder = path.to_string_lossy().to_string();
                if let Some(workers) = &self.workers {
                    if !workers.local(LocalCommand::AddFolder(folder)) {
                        self.model.error = Some("The folder reader is busy; try again".into());
                    }
                }
            }
        }

        if self.ui.button(cx, ids!(eq_power)).clicked(actions) {
            let on = !self.model.eq.on;
            self.model.eq.on = on;
            if let Some(knobs) = self.knobs() {
                knobs.apply(&self.model.eq);
            }
            // Only the bands and the preamp: off bypasses them sample for
            // sample, and the audio connection stays as it is.
            self.save_eq();
            self.model.changed();
        }
        self.eq_actions(cx, actions);
        let tracks = self.ui.track_table(cx, ids!(tracks));
        if let Some(row) = tracks.activated(actions) {
            if let Some(chosen) = self.model.activate(row) {
                self.model.run(chosen);
            }
        }
        // The small window's numbered list does what the track list does.
        let plist = self.ui.track_table(cx, ids!(plist));
        if let Some(row) = plist.activated(actions) {
            if let Some(chosen) = self.model.activate(row) {
                self.model.run(chosen);
            }
        }
        if let Some((row, abs)) = plist.menu(actions) {
            let mut commands = self.model.row_commands(row);
            commands.extend(self.model.page_commands());
            self.open_menu_with_filter(cx, commands, abs);
        }
        if let Some((row, abs)) = tracks.menu(actions) {
            // The row's commands, then the list's (play all, shuffle, queue
            // all) and the "available locally" switch.
            let mut commands = self.model.row_commands(row);
            commands.extend(self.model.page_commands());
            self.open_menu_with_filter(cx, commands, abs);
        }
        if let Some(row) = tracks.radio(actions) {
            self.radio(Some(row));
        }
        if let Some((at, abs)) = self.ui.library_list(cx, ids!(library)).menu(actions) {
            let commands = self.model.playlist_commands(at);
            self.open_menu(cx, commands, abs);
        }
        if menu_picked(actions, live_id!(row_menu)) == Some(live_id!(filter_local)) {
            self.toggle_filter();
        }
        if let Some(picked) = menu_picked(actions, live_id!(row_menu)) {
            let commands = std::mem::take(&mut self.menu_commands);
            if let Some((_, chosen)) = commands.into_iter().find(|(command, _)| LiveId::from_str(command.label()) == picked) {
                self.model.run(chosen);
            }
        }
        match self.ui.up_next(cx, ids!(up_next)).action(actions) {
            Some(UpNextAction::Jump(at)) => self.model.jump(at),
            Some(UpNextAction::Remove(at)) => self.model.edit_queue(|queue| drop(queue.remove(at))),
            Some(UpNextAction::Move { from, to }) => self.model.edit_queue(|queue| queue.move_to(from, to)),
            Some(UpNextAction::Menu(at, abs)) => {
                self.up_next_menu = Some(at);
                let rows = up_next_commands().iter().map(|(label, id)| MenuRow::new(*id, label)).collect();
                self.ui.menu_layer(cx, ids!(menus)).open(cx, live_id!(up_menu), rows, Rect { pos: abs, size: dvec2(0.0, 0.0) }, MenuPlace::At);
            }
            Some(UpNextAction::None) | None => {}
        }
        if let Some(picked) = menu_picked(actions, live_id!(up_menu)) {
            if let Some(at) = self.up_next_menu.take() {
                match picked {
                    id if id == live_id!(up_play) => self.model.jump(at),
                    id if id == live_id!(up_first) => self.model.edit_queue(|queue| queue.move_to(at, 0)),
                    id if id == live_id!(up_remove) => self.model.edit_queue(|queue| drop(queue.remove(at))),
                    id if id == live_id!(up_clear) => self.model.edit_queue(|queue| queue.clear()),
                    _ => {}
                }
            }
        }

        if self.ui.button(cx, ids!(up_next_toggle)).clicked(actions) {
            self.up_next_open = !self.up_next_open;
            self.apply_up_next(cx);
            self.save_up_next(cx);
        }
        if let Some(collapse) = self.ui.splitter(cx, ids!(split)).collapsed(actions) {
            let open = collapse != SplitterCollapse::B;
            if open != self.up_next_open {
                self.up_next_open = open;
                self.apply_up_next(cx);
                self.save_up_next(cx);
            }
        }
        // Any of the layout's splitters let go: keep them for this layout.
        if actions.iter().any(|a| a.as_widget_action().is_some_and(|wa| matches!(wa.cast::<SplitterAction>(), SplitterAction::Settled { .. }))) {
            self.save_layout_splits(cx);
            // Up Next dragged open (or shut) by its rail: its key follows.
            let open = self.ui.splitter(cx, ids!(split)).collapse() != SplitterCollapse::B;
            if open != self.up_next_open {
                self.up_next_open = open;
                self.apply_up_next(cx);
                self.save_up_next(cx);
            }
        }
        if self.ui.button(cx, ids!(play_pause)).clicked(actions) {
            self.toggle_play();
        }
        if self.ui.button(cx, ids!(next)).clicked(actions) {
            self.model.next();
        }
        if self.ui.button(cx, ids!(previous)).clicked(actions) {
            let now = Self::now(cx);
            self.model.previous(now);
        }
        if self.ui.button(cx, ids!(radio)).clicked(actions) {
            self.radio(None);
        }
        if self.ui.button(cx, ids!(shuffle)).clicked(actions) {
            self.model.shuffle = !self.model.shuffle;
            let on = self.model.shuffle;
            self.model.transport(Transport::Shuffle(on));
        }
        if self.ui.button(cx, ids!(repeat)).clicked(actions) {
            self.model.repeat = match self.model.repeat {
                Repeat::Off => Repeat::Context,
                Repeat::Context => Repeat::Track,
                Repeat::Track => Repeat::Off,
            };
            let mode = self.model.repeat;
            self.model.transport(Transport::Repeat(mode));
        }
        if let Some(fraction) = self.ui.music_bar(cx, ids!(progress)).released(actions) {
            let length = self.model.playback.duration_ms as f64;
            if length > 0.0 {
                self.model.transport(Transport::Seek((fraction * length) as u32));
            }
        }
        // The volume follows the knob and slider as they move; letting go
        // also saves it (see `Model::set_volume`).
        let volume = self.ui.hw_knob(cx, ids!(volume));
        if let Some(level) = volume.moved(actions) {
            self.model.set_volume(level as f32, false);
        }
        if let Some(level) = volume.released(actions) {
            self.model.set_volume(level as f32, true);
        }
        let volume_bar = self.ui.music_bar(cx, ids!(volume_bar));
        if let Some(level) = volume_bar.moved(actions) {
            self.model.set_volume(level as f32, false);
        }
        if let Some(level) = volume_bar.released(actions) {
            self.model.set_volume(level as f32, true);
        }
        if self.ui.button(cx, ids!(eq_page_key)).clicked(actions) {
            self.toggle_eq();
        }
        if self.ui.button(cx, ids!(map_key)).clicked(actions) || self.ui.button(cx, ids!(deck_map)).clicked(actions) || self.ui.button(cx, ids!(deck_map_icon)).clicked(actions) {
            let on = !hw::color_map_on(cx);
            hw::set_color_map(cx, on);
            self.apply_color_map(cx);
        }
        // The style key opens the style picker; a pick restyles live.
        if self.ui.button(cx, ids!(style_next)).clicked(actions) {
            self.open_style_picker(cx);
        }
        self.controls_menu_actions(cx, actions);
        if self.ui.button(cx, ids!(feedback_key)).clicked(actions) {
            self.ui.feedback_panel(cx, ids!(feedback)).open(cx);
        }
        // The rail's own window buttons (Windows, Linux).
        let window = self.ui.window(cx, ids!(main_window));
        if self.ui.button(cx, ids!(win_min)).clicked(actions) {
            window.minimize(cx);
        }
        if self.ui.button(cx, ids!(win_max)).clicked(actions) {
            // Maximised is the window's full-screen geometry (window_geom).
            let full = window.window_id().is_some_and(|id| cx.windows[id].window_geom.is_fullscreen);
            if full { window.restore(cx) } else { window.maximize(cx) }
        }
        // Playing on another device: bring it back to this machine.
        if self.ui.button(cx, ids!(remote_here)).clicked(actions) {
            if let Some(id) = self.model.this_machine_device().map(|d| d.id.clone()) {
                self.model.transport(Transport::Transfer(id));
            }
        }
        if self.ui.button(cx, ids!(win_close)).clicked(actions) {
            window.close(cx);
        }
        self.panels_actions(cx, actions);
        if let Some(id) = menu_picked(actions, live_id!(layout_menu)).or_else(|| menu_picked(actions, live_id!(controls_menu))) {
            if id == live_id!(size_mini) {
                self.go_to_layout_size(cx, "mini");
            } else if id == live_id!(size_widget) {
                self.go_to_layout_size(cx, "widget");
            } else if id == live_id!(size_back) {
                self.toggle_mini_player(cx);
            }
        }
        if self.ui.hw_clock(cx, ids!(clock)).double_clicked(actions) && layout::active_class(cx) == layout::SizeClass::Small {
            self.go_to_layout_size(cx, "widget");
        }
        if let Some(id) = menu_picked(actions, live_id!(layout_menu)) {
            if id == live_id!(showcase) {
                self.toggle_showcase(cx, false);
            } else if id == live_id!(showcase_styles) {
                self.toggle_showcase(cx, true);
            } else if let Some(entry) = layout::layout_list().into_iter().find(|e| LiveId::from_str(&e.name) == id) {
                self.switch_layout(cx, &entry.name, true);
            }
        }
        if self.ui.button(cx, ids!(layout_next)).clicked(actions) {
            self.open_layout_picker(cx);
        }
        if let Some(id) = menu_picked(actions, live_id!(style_menu)) {
            if let Some(entry) = hw::style_list().into_iter().find(|entry| LiveId::from_str(&entry.name) == id) {
                self.transition(cx, Change::Style(entry.name.clone()));
            }
        }
        if let Some(at) = self.ui.drop_down(cx, ids!(devices)).changed(actions) {
            let offset = self.device_row_offset();
            let picked = at.checked_sub(offset).and_then(|at| self.model.devices.get(at));
            if let Some(device) = picked.filter(|device| !device.active) {
                let id = device.id.clone();
                self.device_pending = Some((id.clone(), cx.seconds_since_app_start()));
                self.model.transport(Transport::Transfer(id));
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        layout::switch_mark(vm.cx_mut(), "script_mod: begin (every module re-evaluated)");
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);
        layout::switch_mark(vm.cx_mut(), "script_mod: widgets done");
        crate::widgets::script_mod(vm);
        crate::visual::script_mod(vm);
        makepad_milkdrop::script_mod(vm);
        crate::popout::script_mod(vm);
        crate::torch::script_mod(vm);
        crate::hw::script_mod(vm);
        crate::layout::script_mod(vm);
        crate::eq::script_mod(vm);
        crate::micro::script_mod(vm);
        crate::connection::script_mod(vm);
        makepad_app_aichat::script_mod(vm);
        makepad_widgets_feedback::script_mod(vm);
        crate::theme_manager::script_mod(vm);
        layout::switch_mark(vm.cx_mut(), "script_mod: app modules done");
        crate::hw::apply_style(vm);
        layout::switch_mark(vm.cx_mut(), "script_mod: style applied (2 evals)");
        crate::layout::apply_layout(vm);
        layout::switch_mark(vm.cx_mut(), "script_mod: layout evaluated");
        let value = self::script_mod(vm);
        layout::switch_mark(vm.cx_mut(), "script_mod: app tree evaluated");
        value
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        // A window manager parks the window small (or returns it to full
        // size): the compact-forms contract, platform/src/ime.rs.
        if let Event::Custom(json) = event {
            if let Some(parked) = makepad_widgets::makepad_platform::ime::HostedParked::parse(json) {
                self.on_parked(cx, parked);
                return;
            }
        }
        self.ai_event(cx, event);
        crate::theme_manager::handle_event(cx, event, &self.ui);
        if let Event::Startup = event {
            cx.declare_compact_forms(&layout::compact_forms());
            layout::set_in_place(cx, true);
            // Said over the old picture while a new style's shaders compile.
            self.ui.window(cx, ids!(main_window)).set_crossfade_note("Compiling shaders\u{2026}");
            self.model.theme = cx.with_vm(hw::palette);
            self.apply_theme(cx);
            self.apply_color_map(cx);
            self.apply_torch(cx);
            // Every splitter where the person left it in this layout.
            self.restore_layout_splits(cx);
            if hw::pinned(cx) || layout::pinned(cx) {
                // A style or layout author's run: the file is watched.
                self.style_watch = cx.start_interval(0.4);
            }
            if std::env::var_os("AMP_TAP_TRACE").is_some() {
                self.tap_trace = cx.start_interval(1.0);
            }
            // `--size WxH`: start at that window size (checks small layouts).
            if let Some(size) = size_from_args() {
                self.ui.window(cx, ids!(main_window)).resize(cx, size);
            }
            let workers = Workers::start(&cx.thread_spawner());
            self.model.route_sound = Some(workers.knobs.route_sound.clone());
            self.model.local_sound = Some(workers.knobs.local_sound.clone());
            self.model.has_route = workers.has_route();
            // One callback for the life of the app; the device it feeds
            // opens only while a local file plays.
            let mut output = Output::new(workers.shared.clone(), workers.knobs.local_chain());
            cx.audio_output(0, move |info, buffer| output.fill(info.sample_rate, buffer));
            self.workers = Some(workers);
            self.ui.text_input(cx, ids!(search)).set_key_focus(cx);
            self.model.rebuild();
        }
        if self.tap_trace.is_event(event).is_some() {
            match self.model.sound() {
                Some(tap) => {
                    let written = tap.written();
                    let mut last = vec![0.0f32; 1024];
                    tap.latest(&mut last);
                    let peak = last.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                    log!("amp: tap +{} frames in 1 s, newest peak {:.5}", written.saturating_sub(self.tap_trace_last), peak);
                    self.tap_trace_last = written;
                }
                None => log!("amp: tap: no sound source"),
            }
        }
        if self.style_watch.is_event(event).is_some() {
            if hw::pinned(cx) {
                hw::check_file(cx);
            }
            if layout::pinned(cx) {
                layout::check_file(cx);
            }
        }
        if self.showcase_timer.is_event(event).is_some() {
            self.showcase_step(cx);
        }
        if matches!(event, Event::LiveEdit) {
            layout::switch_mark(cx, "LiveEdit at the app (script_apply over the tree done)");
            self.model.theme = cx.with_vm(hw::palette);
            // The restyle put every widget back to its template: what Rust
            // set on them (labels, lights, selections, the grids' colours)
            // is set again.
            self.model.style_gen += 1;
            // Its printed name may have changed with it (`// title:`).
            self.style_title = Default::default();
            self.apply_theme(cx);
            self.milk_full_applied = None;
            self.update_milk(cx);
            self.apply_color_map(cx);
            self.restore_layout_splits(cx);
            self.fits_applied = None;
            self.tight = None;
            self.tight_key = None;
            // A restyle puts the Window's stock caption back (a script
            // property): the rail takes its place again.
            self.rail_registered = false;
            self.rail_sides = None;
            // The torch's template came back hidden, in the old accent.
            self.apply_torch(cx);
            // The new style and layout are in: fade the frozen frame out.
            self.ui.window(cx, ids!(main_window)).start_crossfade(cx);
            self.model.changed();
            layout::switch_mark(cx, "app LiveEdit handling done");
        }
        if self.fit_again.is_event(event).is_some() {
            self.ui.redraw(cx);
        }
        if let Event::Draw(_) = event {
            hw::log_relief_stats(cx);
            self.place_torch(cx);
            if let Some((t0, skips0)) = self.switch_started {
                let skips = cx.pipeline_skips();
                let now = cx.seconds_since_app_start();
                if std::env::var_os("MUSIC_SWITCH_TRACE").is_some() {
                    let r = cx.draw_skip_reasons();
                    log!("amp: switch +{:.0} ms: {} draws not ready so far (all-time by reason: presentable {} no-shader {} pipeline {})", (now - t0) * 1000.0, skips - skips0, r[0], r[1], r[2]);
                }
                if now - t0 > 2.0 {
                    self.switch_started = None;
                }
            }
            // Legends follow the keys' faces: again when the style or the
            // window's size changed (once the keys have been laid out).
            let size = self.window_size(cx);
            // The window changed size: the layout that fits it (the pick,
            // or the largest that fits, with some hysteresis).
            let now_size = (size.x as i64, size.y as i64);
            if size.x > 0.0 && self.fitted_size != Some(now_size) {
                self.fitted_size = Some(now_size);
                // The rects of this size are known once it has drawn.
                self.fit_again = cx.new_next_frame();
                if !layout::showcase(cx).0 && layout::refit(cx, size) {
                    // A new size class appears in one step: applied in place
                    // before this frame draws, no tween through in-between
                    // rects, and no frame shown until every part of it can
                    // draw (the last whole frame stays).
                    cx.present_whole_frames_for(1.0);
                    self.switch_started = Some((cx.seconds_since_app_start(), cx.pipeline_skips()));
                }
                self.apply_layout_in_place(cx);
            }
            self.apply_fits(cx);
            let sig = (self.model.style_gen, (size.x * 4.0) as i64, (size.y * 4.0) as i64);
            if self.legend_sig != Some(sig) && size.x > 0.0 {
                if self.fit_legends(cx) {
                    self.legend_sig = Some(sig);
                }
            }
        }
        if let Event::Signal = event {
            self.poll_workers(cx);
            // A poll or a local file starting may bring sound to a frozen
            // MilkDrop picture.
            self.drive_milk(cx);
        }
        self.milk_event(cx, event);
        if let Event::AudioDevices(devices) = event {
            self.output_devices = devices.default_output();
            // An open output follows the default device when it changes.
            if self.output_open {
                cx.use_audio_outputs(&self.output_devices);
            }
        }
        match event {
            Event::WindowGotFocus(_) => {
                self.send(Command::Active(true));
                if let Some(workers) = &self.workers {
                    workers.local(LocalCommand::Rescan);
                }
            }
            Event::WindowLostFocus(_) => {
                self.send(Command::Active(false));
            }
            _ => {}
        }
        if self.catalog_timer.is_event(event).is_some() {
            self.catalog_timer = Timer::empty();
            let (seq, query) = (self.model.catalog_seq, self.model.query.trim().to_string());
            if !query.is_empty() {
                let _ = self.send(Command::Search { seq, query });
            }
        }
        if self.ticker.is_event(event).is_some() {
            self.update_progress(cx);
            self.update_ticker(cx);
            // The ticker runs while anything plays: sound may be flowing again
            // into a frozen MilkDrop picture (a local file resumed).
            self.drive_milk(cx);
        }
        if let Event::NetworkResponses(responses) = event {
            let image = self.ui.image(cx, ids!(art));
            for response in responses {
                match response {
                    NetworkResponse::HttpResponse { request_id, response } => self.art.arrived(cx, &image, *request_id, response, self.workers.as_ref()),
                    NetworkResponse::HttpError { request_id, .. } => self.art.failed(*request_id),
                    _ => {}
                }
            }
        }
        if let Event::MacosMenuCommand(item) = event {
            match *item {
                id if id == live_id!(music_style) => self.open_style_picker(cx),
                id if id == live_id!(music_layout) => self.open_layout_picker(cx),
                id if id == live_id!(music_ai) => crate::theme_manager::toggle_chat(cx, &self.ui),
                id if id == live_id!(music_popout) => self.toggle_popout(cx),
                id if id == live_id!(music_torch) => self.toggle_torch(cx),
                id if id == live_id!(music_eq) => self.show_page(cx, if self.model.eq.open { "tracks" } else { "eq" }),
                _ => {}
            }
        }
        // While Send feedback is open its keys are its own: Escape closes it
        // (not the fullscreen view or the search), Space presses its buttons
        // (not play).
        // Space is the player's: play/pause, whatever control holds the key
        // focus (a focused button would otherwise press on it as well). The
        // widgets never see that press or its release.
        let mut player_key = false;
        if let Event::KeyDown(key) = event {
            if !self.ui.feedback_panel(cx, ids!(feedback)).is_open() && !self.popout_key(cx, event) {
                if key.key_code == KeyCode::Space && !key.modifiers.is_primary() && !self.typing(cx) {
                    player_key = true;
                    self.space_for_play = true;
                }
                self.handle_key(cx, key);
            }
        }
        if let Event::KeyUp(key) = event {
            if key.key_code == KeyCode::Space && std::mem::take(&mut self.space_for_play) {
                player_key = true;
            }
        }
        self.match_event(cx, event);
        if self.refit_frame.is_event(event).is_some() {
            self.ui.redraw(cx);
        }
        let draw = matches!(event, Event::Draw(_));
        if draw {
            layout::switch_mark(cx, "draw: begin");
        }
        if !player_key {
            self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.model));
        }
        if draw {
            // The pocket volume slider takes the finger anywhere in its slot
            // (the bar alone is too thin a target).
            let slot = self.ui.view(cx, ids!(volume_slim)).area();
            self.ui.music_bar(cx, ids!(volume_bar)).set_touch_area(slot);
        }
        if draw {
            // The components found new fits in this draw (a layout switch):
            // this frame is not the finished one; the next, with the fits
            // applied, is (the platform holds this one back).
            if self.switch_started.is_some() && Some(self.ui.hw_stage(cx, ids!(stage)).fits()) != self.fits_applied {
                cx.hold_this_frame();
                // A redraw asked for inside a draw is dropped: ask on the
                // next frame's event.
                self.refit_frame = cx.new_next_frame();
            }
            layout::switch_mark(cx, "draw: end");
            if cx.seconds_since_app_start() - self.switch_started.map(|s| s.0).unwrap_or(f64::MAX) > 0.8 {
                layout::switch_done(cx);
            }
        }
        // A right-click nothing else took (the chassis, a plate): every
        // control in one menu.
        if let Event::MouseDown(down) = event {
            if down.button.is_secondary() && down.handled.get().is_empty() && !self.ui.menu_layer(cx, ids!(menus)).is_open() {
                let anchor = Rect { pos: down.abs, size: dvec2(1.0, 0.0) };
                self.open_controls_menu(cx, anchor);
            }
        }
        // After the device: the pop-out shows what it drew this frame.
        self.popout_event(cx, event);
        self.connection_event(cx, event);
        self.after_event(cx);
    }
}

#[cfg(test)]
makepad_script_test::shader_inventory_test!("amp", <App as AppMain>::script_mod);
