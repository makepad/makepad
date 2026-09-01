//! Product shell. Pianist mode leaves only the score surface in the normal
//! layout; its controls are transient overlays. Editor mode reveals the same
//! document through a compact menu/toolbar/panel arrangement.

use crate::{
    action::{
        room_summary, AnnotationTool, BrowseTarget, DialogKind,
        InspectorTab, PageLayout, PaletteCommand, PrefToggle, ScoreAction, ScoreTool,
    },
    keymap::KEYMAP_ROWS,
    playback::REVERB_PRESETS,
    sound::{self, SoundParam},
    state::{transport_label, ScoreAppState},
    ProductMode,
};
use makepad_widgets::*;
use std::path::PathBuf;

script_mod! {
    use mod.prelude.score.*
    use mod.widgets.*

    mod.widgets.ScoreContextMenuBase = #(ScoreContextMenu::register_widget(vm))
    // The rows live inside a ScorePopup, not straight on the widget: a bare
    // `show_bg` on the registered base drew no panel at all, so the menu was
    // white-on-paper text floating over the music.
    mod.widgets.ScoreContextMenu = set_type_default() do mod.widgets.ScoreContextMenuBase{
        width: Fit
        height: Fit
        context_panel := ScorePopup{
            width: 218
            context_play := ScoreMenuRow{text: "Play from here"}
            context_loop := ScoreMenuRow{text: "Loop this bar"}
            context_select_more := ScoreMenuRow{text: "Select more                 R"}
            ScoreDivider{}
            context_fingering := ScoreMenuRow{text: "Add fingering…"}
            context_circle := ScoreMenuRow{text: "Circle note"}
            ScoreDivider{}
            context_properties := ScoreMenuRow{text: "Show properties"}
        }
    }

    mod.widgets.ScoreOverlayBase = #(ScoreOverlay::register_widget(vm))
    // No width/height: an overlay claims no slot in the shell's layout.
    // `ScoreOverlay::on_after_apply` pins the walk it reports upward to
    // `Walk::empty()`, and `draw_walk` sizes it from the pass instead.
    mod.widgets.ScoreOverlay = set_type_default() do mod.widgets.ScoreOverlayBase{
        flow: Overlay
    }

    mod.widgets.ScoreShellBase = #(ScoreShell::register_widget(vm))
    mod.widgets.ScoreShell = set_type_default() do mod.widgets.ScoreShellBase{
        width: Fill
        height: Fill
        flow: Overlay
        show_bg: true
        draw_bg +: {color: score.color_surround}

        main := View{
            width: Fill
            height: Fill
            flow: Down
            spacing: 0

            editor_top := View{
                visible: false
                width: Fill
                height: Fit
                flow: Down
                menu_bar := SolidView{
                    width: Fill
                    height: score.menu_height
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 6 right: 8 top: 0 bottom: 0}
                    spacing: 0
                    draw_bg +: {color: score.color_chrome}
                    menu_file_button := ScoreMenuButton{text: "File"}
                    menu_edit_button := ScoreMenuButton{text: "Edit"}
                    menu_view_button := ScoreMenuButton{text: "View"}
                    menu_play_button := ScoreMenuButton{text: "Playback"}
                    menu_notation_button := ScoreMenuButton{text: "Notation"}
                    menu_help_button := ScoreMenuButton{text: "Help"}
                    Filler{}
                    score_name := ScoreLabelDim{text: "Score"}
                    ScoreLabelMuted{text: "  ·  EDITOR"}
                }
                toolbar := SolidView{
                    width: Fill
                    height: score.toolbar_height
                    flow: Right
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{left: 8 right: 8 top: 5 bottom: 5}
                    spacing: 5
                    draw_bg +: {color: score.color_chrome_raised}
                    mode_pianist := ScoreButton{text: "Pianist"}
                    ScoreDivider{width: 1 height: 22 margin: Inset{left: 3 right: 3}}
                    // What a drag MEANS, chosen rather than inferred. The
                    // first is the safe one, and it is where the app rests.
                    tool_navigate := ScoreButtonFlat{text: "Navigate"}
                    tool_choose := ScoreButtonFlat{text: "Select"}
                    tool_edit := ScoreButtonFlat{text: "Edit"}
                    ScoreDivider{width: 1 height: 22 margin: Inset{left: 3 right: 3}}
                    tool_transpose_up := ScoreToolButton{text: "▲"}
                    tool_transpose_down := ScoreToolButton{text: "▼"}
                    tool_delete := ScoreToolButton{text: "⌫"}
                    ScoreDivider{width: 1 height: 22 margin: Inset{left: 3 right: 3}}
                    tool_select := ScoreToolButton{text: "↖"}
                    duration_4 := ScoreToolButton{width: 38 text: "1/8"}
                    duration_5 := ScoreToolButton{width: 38 text: "1/4"}
                    duration_6 := ScoreToolButton{width: 38 text: "1/2"}
                    duration_7 := ScoreToolButton{width: 38 text: "1/1"}
                    palette_flat := ScoreToolButton{text: "♭"}
                    palette_natural := ScoreToolButton{text: "♮"}
                    palette_sharp := ScoreToolButton{text: "♯"}
                    palette_staccato := ScoreToolButton{text: "·"}
                    palette_accent := ScoreToolButton{text: ">"}
                    palette_tenuto := ScoreToolButton{text: "—"}
                    Filler{}
                    layout_single := ScoreButtonFlat{text: "Pages"}
                    layout_two := ScoreButtonFlat{text: "Two-up"}
                    layout_continuous := ScoreButtonFlat{text: "Continuous"}
                    layout_overview := ScoreButtonFlat{text: "All pages"}
                    zoom_out := ScoreToolButton{text: "−"}
                    fit_page := ScoreButton{text: "Fit"}
                    zoom_in := ScoreToolButton{text: "+"}
                }
            }

            workspace := View{
                width: Fill
                height: Fill
                flow: Right
                spacing: 0
                left_panel := ScorePanel{
                    visible: false
                    width: score.panel_width
                    ScorePanelHeader{
                        ScoreHeader{text: "MUSIC"}
                        Filler{}
                        music_folder := ScoreButtonFlat{text: "Folder…"}
                    }
                    music_list := ScrollYView{
                        width: Fill height: 244 flow: Down
                        music_0 := ScoreMenuRow{text: ""}
                        music_1 := ScoreMenuRow{text: ""}
                        music_2 := ScoreMenuRow{text: ""}
                        music_3 := ScoreMenuRow{text: ""}
                        music_4 := ScoreMenuRow{text: ""}
                        music_5 := ScoreMenuRow{text: ""}
                        music_6 := ScoreMenuRow{text: ""}
                        music_7 := ScoreMenuRow{text: ""}
                        music_8 := ScoreMenuRow{text: ""}
                        music_9 := ScoreMenuRow{text: ""}
                        music_10 := ScoreMenuRow{text: ""}
                        music_11 := ScoreMenuRow{text: ""}
                        music_12 := ScoreMenuRow{text: ""}
                        music_13 := ScoreMenuRow{text: ""}
                        music_14 := ScoreMenuRow{text: ""}
                        music_15 := ScoreMenuRow{text: ""}
                    }
                    music_empty := ScoreLabelWrap{visible: false text: ""}
                    music_credit := ScoreLabelMuted{text: ""}
                    ScoreDivider{}
                    panel_parts_header := ScorePanelHeader{
                        ScoreHeader{text: "PARTS & INSTRUMENTS"}
                        Filler{}
                        parts_count := ScoreLabelMuted{text: ""}
                    }
                    part_0 := ScoreRow{
                        part_0_name := ScoreLabel{text: "Piano"}
                        Filler{}
                        part_0_mute := ScoreButtonFlat{text: "M"}
                        part_0_solo := ScoreButtonFlat{text: "S"}
                    }
                    part_1 := ScoreRow{visible: false part_1_name := ScoreLabel{text: "Part 2"} Filler{} part_1_mute := ScoreButtonFlat{text: "M"} part_1_solo := ScoreButtonFlat{text: "S"}}
                    part_2 := ScoreRow{visible: false part_2_name := ScoreLabel{text: "Part 3"} Filler{} part_2_mute := ScoreButtonFlat{text: "M"} part_2_solo := ScoreButtonFlat{text: "S"}}
                    part_3 := ScoreRow{visible: false part_3_name := ScoreLabel{text: "Part 4"} Filler{} part_3_mute := ScoreButtonFlat{text: "M"} part_3_solo := ScoreButtonFlat{text: "S"}}
                    ScoreDivider{}
                    ScorePanelHeader{ScoreHeader{text: "NOTATION PALETTE"}}
                    ScoreSection{ScoreLabelDim{text: "Common"}}
                    View{
                        width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 5
                        padding: Inset{left: 9 right: 9 top: 8 bottom: 8}
                        p_staccato := ScoreToolButton{text: "·"}
                        p_accent := ScoreToolButton{text: ">"}
                        p_tenuto := ScoreToolButton{text: "—"}
                        p_flat := ScoreToolButton{text: "♭"}
                        p_natural := ScoreToolButton{text: "♮"}
                        p_sharp := ScoreToolButton{text: "♯"}
                    }
                    ScoreDivider{}
                    ScorePanelHeader{ScoreHeader{text: "ANNOTATE"}}
                    View{
                        width: Fill height: Fit flow: Flow.Right{wrap: true} spacing: 5
                        padding: Inset{left: 9 right: 9 top: 8 bottom: 8}
                        annotate_highlight := ScoreButton{text: "Highlight"}
                        annotate_circle := ScoreButton{text: "Circle"}
                        annotate_text := ScoreButton{text: "Text"}
                        annotate_fingering := ScoreButton{text: "Fingering"}
                        annotate_ink := ScoreButton{text: "Ink"}
                    }
                    Filler{}
                    View{
                        width: Fill height: Fit flow: Down spacing: 3
                        padding: Inset{left: 10 right: 10 top: 8 bottom: 10}
                        ScoreLabelMuted{text: "NOTE ENTRY"}
                        ScoreLabelDim{text: "C D E F G A B · pitches"}
                        ScoreLabelDim{text: "1…7 · durations   R · select more"}
                    }
                }

                canvas := ScoreCanvas{}

                right_panel := ScorePanel{
                    visible: false
                    width: score.inspector_width
                    ScorePanelHeader{
                        // No title: four tabs already fill the strip, and the
                        // panel they open says what it is.
                        inspector_properties := ScoreButtonFlat{text: "Properties"}
                        inspector_mixer := ScoreButtonFlat{text: "Mixer"}
                        inspector_sound := ScoreButtonFlat{text: "Sound"}
                        inspector_history := ScoreButtonFlat{text: "History"}
                    }
                    properties_page := View{
                        width: Fill height: Fill flow: Down
                        ScoreSection{ScoreLabelDim{text: "Selection"}}
                        ScoreRow{ScoreLabelDim{text: "Object"} Filler{} selection_kind := ScoreLabel{text: "No selection"}}
                        ScoreRow{ScoreLabelDim{text: "Pitch"} Filler{} selection_pitch := ScoreLabel{text: "—"}}
                        ScoreRow{ScoreLabelDim{text: "Position"} Filler{} selection_position := ScoreLabel{text: "—"}}
                        ScoreRow{ScoreLabelDim{text: "Duration"} Filler{} selection_duration := ScoreLabel{text: "—"}}
                        ScoreRow{ScoreLabelDim{text: "Articulation"} Filler{} selection_articulation := ScoreLabel{text: "—"}}
                        ScoreDivider{}
                        ScoreSection{ScoreLabelDim{text: "Score"}}
                        ScoreRow{ScoreLabelDim{text: "Pages"} Filler{} score_pages := ScoreLabel{text: "—"}}
                        ScoreRow{ScoreLabelDim{text: "Bars"} Filler{} score_bars := ScoreLabel{text: "—"}}
                        ScoreRow{ScoreLabelDim{text: "Tempo"} Filler{} score_tempo := ScoreLabel{text: "—"}}
                        ScoreDivider{}
                        ScoreSection{ScoreLabelDim{text: "Playback"}}
                        ScoreRow{ScoreLabelDim{text: "Sound"} Filler{} sound_status := ScoreLabel{text: "Built-in piano"}}
                        ScoreRow{ScoreLabelDim{text: "Engine"} Filler{} engine_status := ScoreLabel{text: "—"}}
                        Filler{}
                    }
                    mixer_page := View{
                        visible: false width: Fill height: Fill flow: Down
                        ScoreSection{ScoreLabelDim{text: "Part mix"}}
                        mix_0 := View{
                            visible: false width: Fill height: Fit flow: Down spacing: 4
                            padding: Inset{left: 10 right: 10 top: 9 bottom: 9}
                            mix_0_name := ScoreHeader{text: ""}
                            ScoreRow{padding: Inset{left: 0 right: 0} ScoreLabelDim{text: "Gain"} Filler{} mix_0_gain := ScoreLabel{text: "0 dB"} mix_0_down := ScoreButtonFlat{text: "−"} mix_0_up := ScoreButtonFlat{text: "+"}}
                            ScoreRow{padding: Inset{left: 0 right: 0} ScoreLabelDim{text: "Pan"} Filler{} mix_0_pan := ScoreLabel{text: "C"} mix_0_left := ScoreButtonFlat{text: "L"} mix_0_right := ScoreButtonFlat{text: "R"}}
                            View{width: Fill height: Fit flow: Right spacing: 5 mix_0_mute := ScoreButton{text: "Mute"} mix_0_solo := ScoreButton{text: "Solo"}}
                        }
                        mix_1 := View{
                            visible: false width: Fill height: Fit flow: Down spacing: 4
                            padding: Inset{left: 10 right: 10 top: 9 bottom: 9}
                            mix_1_name := ScoreHeader{text: ""}
                            ScoreRow{padding: Inset{left: 0 right: 0} ScoreLabelDim{text: "Gain"} Filler{} mix_1_gain := ScoreLabel{text: "0 dB"} mix_1_down := ScoreButtonFlat{text: "−"} mix_1_up := ScoreButtonFlat{text: "+"}}
                            ScoreRow{padding: Inset{left: 0 right: 0} ScoreLabelDim{text: "Pan"} Filler{} mix_1_pan := ScoreLabel{text: "C"} mix_1_left := ScoreButtonFlat{text: "L"} mix_1_right := ScoreButtonFlat{text: "R"}}
                            View{width: Fill height: Fit flow: Right spacing: 5 mix_1_mute := ScoreButton{text: "Mute"} mix_1_solo := ScoreButton{text: "Solo"}}
                        }
                        mix_2 := View{
                            visible: false width: Fill height: Fit flow: Down spacing: 4
                            padding: Inset{left: 10 right: 10 top: 9 bottom: 9}
                            mix_2_name := ScoreHeader{text: ""}
                            ScoreRow{padding: Inset{left: 0 right: 0} ScoreLabelDim{text: "Gain"} Filler{} mix_2_gain := ScoreLabel{text: "0 dB"} mix_2_down := ScoreButtonFlat{text: "−"} mix_2_up := ScoreButtonFlat{text: "+"}}
                            ScoreRow{padding: Inset{left: 0 right: 0} ScoreLabelDim{text: "Pan"} Filler{} mix_2_pan := ScoreLabel{text: "C"} mix_2_left := ScoreButtonFlat{text: "L"} mix_2_right := ScoreButtonFlat{text: "R"}}
                            View{width: Fill height: Fit flow: Right spacing: 5 mix_2_mute := ScoreButton{text: "Mute"} mix_2_solo := ScoreButton{text: "Solo"}}
                        }
                        mix_3 := View{
                            visible: false width: Fill height: Fit flow: Down spacing: 4
                            padding: Inset{left: 10 right: 10 top: 9 bottom: 9}
                            mix_3_name := ScoreHeader{text: ""}
                            ScoreRow{padding: Inset{left: 0 right: 0} ScoreLabelDim{text: "Gain"} Filler{} mix_3_gain := ScoreLabel{text: "0 dB"} mix_3_down := ScoreButtonFlat{text: "−"} mix_3_up := ScoreButtonFlat{text: "+"}}
                            ScoreRow{padding: Inset{left: 0 right: 0} ScoreLabelDim{text: "Pan"} Filler{} mix_3_pan := ScoreLabel{text: "C"} mix_3_left := ScoreButtonFlat{text: "L"} mix_3_right := ScoreButtonFlat{text: "R"}}
                            View{width: Fill height: Fit flow: Right spacing: 5 mix_3_mute := ScoreButton{text: "Mute"} mix_3_solo := ScoreButton{text: "Solo"}}
                        }
                        ScoreDivider{}
                        // The room moved into the Sound panel: one place for
                        // everything that shapes what the piano sounds like.
                        ScoreRow{ScoreLabelMuted{text: "Instrument, brightness and room · Sound tab"}}
                        Filler{}
                    }
                    sound_page := ScrollYView{
                        visible: false width: Fill height: Fill flow: Down

                        // Two instruments. The engine is a property of the
                        // instrument, not a mode chosen first, so picking a
                        // row is all there is to it.
                        ScoreSection{ScoreLabelDim{text: "Instrument"}}
                        inst_0 := ScoreMenuRow{text: ""}
                        inst_0_desc := ScoreLabelWrap{text: "" margin: Inset{left: 18 right: 10 top: 0 bottom: 7}}
                        inst_1 := ScoreMenuRow{text: ""}
                        inst_1_desc := ScoreLabelWrap{text: "" margin: Inset{left: 18 right: 10 top: 0 bottom: 7}}
                        ScoreDivider{}

                        // Brightness: one treble shelf over whichever
                        // instrument is playing. See sound::BRIGHTNESS_HZ.
                        ScoreSection{ScoreLabelDim{text: "Tone"}}
                        View{
                            width: Fill height: Fit flow: Down spacing: 3
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 8}
                            View{
                                width: Fill height: Fit flow: Right align: Align{x: 0.0 y: 0.5} spacing: 5
                                sl_brightness_name := ScoreLabelDim{text: ""}
                                Filler{}
                                sl_brightness_value := ScoreLabel{text: ""}
                            }
                            sl_brightness := ScoreSlider{}
                        }
                        ScoreDivider{}

                        ScoreSection{ScoreLabelDim{text: "Room"} Filler{} room_name := ScoreLabelMuted{text: ""}}
                        View{
                            width: Fill height: Fit flow: Down spacing: 4
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 3}
                            View{
                                width: Fill height: Fit flow: Right spacing: 4
                                room_practice := ScoreButtonFlat{text: "Practice"}
                                room_studio := ScoreButtonFlat{text: "Studio"}
                            }
                            View{
                                width: Fill height: Fit flow: Right spacing: 4
                                room_small_hall := ScoreButtonFlat{text: "Small hall"}
                                room_concert_hall := ScoreButtonFlat{text: "Concert hall"}
                            }
                            View{
                                width: Fill height: Fit flow: Right spacing: 4
                                room_cathedral := ScoreButtonFlat{text: "Cathedral"}
                                Filler{}
                            }
                        }
                        View{
                            width: Fill height: Fit flow: Down spacing: 3
                            padding: Inset{left: 10 right: 10 top: 7 bottom: 4}
                            View{
                                width: Fill height: Fit flow: Right align: Align{x: 0.0 y: 0.5} spacing: 5
                                sl_reverb_name := ScoreLabelDim{text: ""}
                                Filler{}
                                sl_reverb_value := ScoreLabel{text: ""}
                            }
                            sl_reverb := ScoreSlider{}
                        }
                        View{
                            width: Fill height: Fit flow: Down
                            padding: Inset{left: 10 right: 10 top: 4 bottom: 14}
                            sound_hint := ScoreLabelWrap{text: ""}
                        }
                    }
                    history_page := View{
                        visible: false width: Fill height: Fill flow: Down
                        ScoreSection{ScoreLabelDim{text: "EDIT JOURNAL"}}
                        history_0 := ScoreRow{history_0_text := ScoreLabelDim{text: "No edits yet"}}
                        history_1 := ScoreRow{visible: false history_1_text := ScoreLabelDim{text: ""}}
                        history_2 := ScoreRow{visible: false history_2_text := ScoreLabelDim{text: ""}}
                        history_3 := ScoreRow{visible: false history_3_text := ScoreLabelDim{text: ""}}
                        history_4 := ScoreRow{visible: false history_4_text := ScoreLabelDim{text: ""}}
                        Filler{}
                        View{width: Fill height: Fit flow: Right spacing: 5 padding: Inset{left: 10 right: 10 top: 8 bottom: 10} history_undo := ScoreButton{text: "Undo"} history_redo := ScoreButton{text: "Redo"}}
                    }
                }
            }

            editor_transport := SolidView{
                visible: false
                width: Fill
                height: score.transport_height
                flow: Right
                align: Align{x: 0.0 y: 0.5}
                padding: Inset{left: 8 right: 8 top: 4 bottom: 4}
                spacing: 5
                draw_bg +: {color: score.color_chrome}
                transport_stop := ScoreToolButton{text: "■"}
                transport_play := ScoreButtonAccent{text: "Play"}
                transport_position := ScoreLabelDim{text: "001 · 1 · 000"}
                transport_scrub := ScoreScrub{}
                tempo_down := ScoreButtonFlat{text: "−"}
                transport_tempo := ScoreButton{text: "108 BPM"}
                tempo_up := ScoreButtonFlat{text: "+"}
                transport_metronome := ScoreButton{text: "Metronome"}
                transport_count_in := ScoreButton{text: "Count-in"}
                transport_loop := ScoreButton{text: "Loop"}
                transport_follow := ScoreButton{text: "Follow"}
            }
            status_bar := SolidView{
                visible: false
                width: Fill height: 22 flow: Right align: Align{x: 0.0 y: 0.5}
                padding: Inset{left: 9 right: 9 top: 0 bottom: 0}
                spacing: 6 draw_bg +: {color: score.color_panel_alt}
                status_text := ScoreLabelMuted{text: "Ready"}
                Filler{}
                page_status := ScoreLabelMuted{text: "Page 1 / 24"}
                zoom_status := ScoreLabelMuted{text: "100%"}
            }
        }

        pianist_layer := View{
            visible: false
            width: Fill height: Fill flow: Down
            pianist_top := View{
                width: Fill height: 52 flow: Right align: Align{x: 0.0 y: 0.5}
                padding: Inset{left: 14 right: 14 top: 9 bottom: 1}
                pianist_edit := ScoreButton{text: "Edit"}
                Filler{}
                pianist_title := ScoreFloatingBar{
                    pianist_title_text := ScoreLabel{text: "Makepad Etude"}
                    pianist_page_text := ScoreLabelMuted{text: "1 / 24"}
                }
                Filler{}
                pianist_annotate := ScoreButton{text: "Annotate"}
            }
            Filler{}
            pianist_annotation := View{
                visible: false width: Fill height: Fit flow: Right align: Align{x: 0.5 y: 0.5}
                margin: Inset{bottom: 7}
                ScoreFloatingBar{
                    p_highlight := ScoreButtonFlat{text: "Highlight"}
                    p_circle := ScoreButtonFlat{text: "Circle"}
                    p_text := ScoreButtonFlat{text: "Text"}
                    p_fingering := ScoreButtonFlat{text: "Fingering"}
                    p_ink := ScoreButtonFlat{text: "Ink"}
                    p_annotation_close := ScoreButtonFlat{text: "Done"}
                }
            }
            pianist_bottom := View{
                width: Fill height: Fit flow: Right align: Align{x: 0.5 y: 0.5}
                padding: Inset{left: 10 right: 10 top: 2 bottom: 13}
                ScoreFloatingBar{
                    pianist_prev := ScoreToolButton{text: "‹"}
                    pianist_play := ScoreButtonAccent{text: "Play"}
                    pianist_next := ScoreToolButton{text: "›"}
                    pianist_metronome := ScoreButton{text: "Metro"}
                    pianist_loop := ScoreButton{text: "Loop"}
                    pianist_follow := ScoreButton{text: "Follow"}
                    pianist_fit := ScoreButtonFlat{text: "Fit"}
                    pianist_overview := ScoreButtonFlat{text: "All pages"}
                    pianist_music := ScoreButtonFlat{text: "Music"}
                    pianist_sound := ScoreButtonFlat{text: "Sound"}
                }
            }
        }

        // Menus, the modal dialog and the context menu live in their own
        // overlay draw list, which composites above the score canvas however
        // the canvas ordered its own ink with `draw_depth`. See ScoreOverlay.
        overlay := mod.widgets.ScoreOverlay{
            menu_layer := View{
                width: Fill height: Fill
                file_menu := ScorePopup{
                    visible: false abs_pos: vec2(6.0, 27.0)
                    file_new := ScoreMenuRow{text: "New score                         ⌘N"}
                    file_open := ScoreMenuRow{text: "Open…                               ⌘O"}
                    file_save := ScoreMenuRow{text: "Save                                  ⌘S"}
                    file_save_as := ScoreMenuRow{text: "Save As…                         ⇧⌘S"}
                    ScoreDivider{}
                    file_library := ScoreMenuRow{text: "Music library…                 ⌘L"}
                    ScoreDivider{}
                    file_setup := ScoreMenuRow{text: "Score setup…"}
                    file_page_setup := ScoreMenuRow{text: "Page and staff size…"}
                    ScoreDivider{}
                    file_prefs := ScoreMenuRow{text: "Preferences…"}
                    file_quit := ScoreMenuRow{text: "Quit                                   ⌘Q"}
                }
                edit_menu := ScorePopup{
                    visible: false abs_pos: vec2(51.0, 27.0)
                    edit_undo := ScoreMenuRow{text: "Undo                                  ⌘Z"}
                    edit_redo := ScoreMenuRow{text: "Redo                              ⇧⌘Z"}
                    ScoreDivider{}
                    edit_select_all := ScoreMenuRow{text: "Select all                           ⌘A"}
                    edit_select_more := ScoreMenuRow{text: "Select more                           R"}
                    edit_clear := ScoreMenuRow{text: "Clear selection                 Escape"}
                    ScoreDivider{}
                    edit_prefs := ScoreMenuRow{text: "Preferences…"}
                }
                view_menu := ScorePopup{
                    visible: false abs_pos: vec2(95.0, 27.0)
                    view_pianist := ScoreMenuRow{text: "Pianist mode                     ⌘E"}
                    view_single := ScoreMenuRow{text: "Pages, left to right"}
                    view_two := ScoreMenuRow{text: "Two-up"}
                    view_continuous := ScoreMenuRow{text: "Continuous scroll"}
                    view_overview := ScoreMenuRow{text: "Zoom out to all pages"}
                    ScoreDivider{}
                    view_fit := ScoreMenuRow{text: "Fit page                             ⌘0"}
                    view_zoom_in := ScoreMenuRow{text: "Zoom in                              ⌘+"}
                    view_zoom_out := ScoreMenuRow{text: "Zoom out                            ⌘−"}
                }
                play_menu := ScorePopup{
                    visible: false abs_pos: vec2(145.0, 27.0)
                    play_toggle := ScoreMenuRow{text: "Play / pause                    Space"}
                    play_stop := ScoreMenuRow{text: "Stop"}
                    ScoreDivider{}
                    play_sound := ScoreMenuRow{text: "Piano sound…"}
                    ScoreDivider{}
                    play_metro := ScoreMenuRow{text: "Metronome                         M"}
                    play_count := ScoreMenuRow{text: "Count-in"}
                    play_loop := ScoreMenuRow{text: "Practice loop                     L"}
                    play_follow := ScoreMenuRow{text: "Follow cursor                      F"}
                }
                notation_menu := ScorePopup{
                    visible: false abs_pos: vec2(228.0, 27.0)
                    notation_staccato := ScoreMenuRow{text: "Staccato"}
                    notation_accent := ScoreMenuRow{text: "Accent"}
                    notation_tenuto := ScoreMenuRow{text: "Tenuto"}
                    ScoreDivider{}
                    notation_sharp := ScoreMenuRow{text: "Sharpen"}
                    notation_flat := ScoreMenuRow{text: "Flatten"}
                    notation_natural := ScoreMenuRow{text: "Natural"}
                    ScoreDivider{}
                    notation_text := ScoreMenuRow{text: "Add text…"}
                    notation_fingering := ScoreMenuRow{text: "Add fingering…"}
                }
                help_menu := ScorePopup{
                    visible: false abs_pos: vec2(309.0, 27.0)
                    help_keymap := ScoreMenuRow{text: "Keyboard map                         F1"}
                    help_about := ScoreMenuRow{text: "About Score"}
                }
            }

            dialog_layer := SolidView{
                visible: false width: Fill height: Fill flow: Down align: Align{x: 0.5 y: 0.5}
                draw_bg +: {color: #x000000b4}
                dialog_box := ScorePopup{
                    width: 560
                    // A dialog is modal; reading the score through it made the
                    // panel look like a rendering artefact rather than a panel.
                    draw_bg +: {color: #x1d1e1bff}
                    padding: Inset{left: 18 right: 18 top: 13 bottom: 15}
                    spacing: 8
                    View{
                        width: Fill height: Fit flow: Right align: Align{x: 0.0 y: 0.5} spacing: 8
                        dialog_title := ScoreTitle{text: "Open score"}
                        Filler{}
                        dialog_close := ScoreButtonFlat{text: "×  Close"}
                    }
                    dialog_body := ScoreLabelDim{text: ""}

                    dialog_file := View{
                        visible: false width: Fill height: Fit flow: Down spacing: 7
                        View{
                            width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                            dialog_path := ScoreTextInput{empty_text: "Path to a score file"}
                            dialog_browse := ScoreButton{text: "Browse…"}
                        }
                        dialog_recent_header := ScoreLabelMuted{text: "RECENT"}
                            recent_0 := ScoreMenuRow{text: ""}
                            recent_1 := ScoreMenuRow{text: ""}
                            recent_2 := ScoreMenuRow{text: ""}
                            recent_3 := ScoreMenuRow{text: ""}
                            recent_4 := ScoreMenuRow{text: ""}
                            recent_5 := ScoreMenuRow{text: ""}
                        dialog_formats := ScoreLabelMuted{text: ""}
                    }

                    dialog_library := View{
                        visible: false width: Fill height: Fit flow: Down spacing: 7
                        View{
                            width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                            library_dir_input := ScoreTextInput{empty_text: "Folder of .mid, .musicxml or .mpscore scores"}
                            library_browse := ScoreButton{text: "Browse…"}
                            library_rescan := ScoreButton{text: "Rescan"}
                        }
                        library_summary := ScoreLabelMuted{text: ""}
                    }

                    dialog_text := View{
                        visible: false width: Fill height: Fit flow: Down spacing: 6
                        dialog_annotation := ScoreTextInput{empty_text: "Annotation text"}
                    }

                    dialog_prefs := View{
                        visible: false width: Fill height: Fit flow: Down
                        ScoreRow{ScoreLabelDim{text: "Start in"} Filler{} pref_start := ScoreButton{text: "Pianist"}}
                        ScoreRow{ScoreLabelDim{text: "Sound notes under the pointer"} Filler{} pref_audition := ScoreButton{text: "On"}}
                        ScoreRow{ScoreLabelDim{text: "Follow the playback cursor"} Filler{} pref_follow := ScoreButton{text: "On"}}
                        ScoreRow{ScoreLabelDim{text: "Metronome armed at launch"} Filler{} pref_metronome := ScoreButton{text: "Off"}}
                        ScoreRow{ScoreLabelDim{text: "Count-in armed at launch"} Filler{} pref_count_in := ScoreButton{text: "Off"}}
                        ScoreRow{ScoreLabelDim{text: "Engraving"} Filler{} pref_paper := ScoreButton{text: "Light paper"}}
                        pref_path := ScoreLabelMuted{text: ""}
                    }

                    dialog_score_setup := View{
                        visible: false width: Fill height: Fit flow: Down
                        ScoreRow{ScoreLabelDim{text: "Title"} Filler{} setup_title := ScoreLabel{text: ""}}
                        ScoreRow{ScoreLabelDim{text: "Parts"} Filler{} setup_parts := ScoreLabel{text: ""}}
                        ScoreRow{ScoreLabelDim{text: "Key and meter"} Filler{} setup_key := ScoreLabel{text: ""}}
                        ScoreRow{ScoreLabelDim{text: "Extent"} Filler{} setup_extent := ScoreLabel{text: ""}}
                        ScoreRow{
                            ScoreLabelDim{text: "Playback tempo"} Filler{}
                            setup_tempo_down := ScoreButtonFlat{text: "−"}
                            setup_tempo := ScoreLabel{text: ""}
                            setup_tempo_up := ScoreButtonFlat{text: "+"}
                        }
                        setup_note := ScoreLabelMuted{text: ""}
                    }

                    dialog_page_setup := View{
                        visible: false width: Fill height: Fit flow: Down
                        ScoreRow{
                            ScoreLabelDim{text: "Page layout"} Filler{}
                            page_single := ScoreButtonFlat{text: "Pages"}
                            page_two := ScoreButtonFlat{text: "Two-up"}
                            page_continuous := ScoreButtonFlat{text: "Continuous"}
                        }
                        ScoreRow{
                            ScoreLabelDim{text: "Staff size"} Filler{}
                            staff_down := ScoreButtonFlat{text: "−"}
                            staff_value := ScoreLabel{text: ""}
                            staff_up := ScoreButtonFlat{text: "+"}
                        }
                        ScoreRow{ScoreLabelDim{text: "Page"} Filler{} page_size := ScoreLabel{text: ""}}
                        page_note := ScoreLabelMuted{text: ""}
                    }

                    dialog_keymap := View{
                        visible: false width: Fill height: Fit flow: Down
                        key_row_0 := ScoreRow{height: 21 key_name_0 := ScoreLabel{text: ""} Filler{} key_action_0 := ScoreLabelDim{text: ""}}
                        key_row_1 := ScoreRow{height: 21 key_name_1 := ScoreLabel{text: ""} Filler{} key_action_1 := ScoreLabelDim{text: ""}}
                        key_row_2 := ScoreRow{height: 21 key_name_2 := ScoreLabel{text: ""} Filler{} key_action_2 := ScoreLabelDim{text: ""}}
                        key_row_3 := ScoreRow{height: 21 key_name_3 := ScoreLabel{text: ""} Filler{} key_action_3 := ScoreLabelDim{text: ""}}
                        key_row_4 := ScoreRow{height: 21 key_name_4 := ScoreLabel{text: ""} Filler{} key_action_4 := ScoreLabelDim{text: ""}}
                        key_row_5 := ScoreRow{height: 21 key_name_5 := ScoreLabel{text: ""} Filler{} key_action_5 := ScoreLabelDim{text: ""}}
                        key_row_6 := ScoreRow{height: 21 key_name_6 := ScoreLabel{text: ""} Filler{} key_action_6 := ScoreLabelDim{text: ""}}
                        key_row_7 := ScoreRow{height: 21 key_name_7 := ScoreLabel{text: ""} Filler{} key_action_7 := ScoreLabelDim{text: ""}}
                        key_row_8 := ScoreRow{height: 21 key_name_8 := ScoreLabel{text: ""} Filler{} key_action_8 := ScoreLabelDim{text: ""}}
                        key_row_9 := ScoreRow{height: 21 key_name_9 := ScoreLabel{text: ""} Filler{} key_action_9 := ScoreLabelDim{text: ""}}
                        key_row_10 := ScoreRow{height: 21 key_name_10 := ScoreLabel{text: ""} Filler{} key_action_10 := ScoreLabelDim{text: ""}}
                        key_row_11 := ScoreRow{height: 21 key_name_11 := ScoreLabel{text: ""} Filler{} key_action_11 := ScoreLabelDim{text: ""}}
                        key_row_12 := ScoreRow{height: 21 key_name_12 := ScoreLabel{text: ""} Filler{} key_action_12 := ScoreLabelDim{text: ""}}
                        key_row_13 := ScoreRow{height: 21 key_name_13 := ScoreLabel{text: ""} Filler{} key_action_13 := ScoreLabelDim{text: ""}}
                        key_row_14 := ScoreRow{height: 21 key_name_14 := ScoreLabel{text: ""} Filler{} key_action_14 := ScoreLabelDim{text: ""}}
                        key_row_15 := ScoreRow{height: 21 key_name_15 := ScoreLabel{text: ""} Filler{} key_action_15 := ScoreLabelDim{text: ""}}
                        key_row_16 := ScoreRow{height: 21 key_name_16 := ScoreLabel{text: ""} Filler{} key_action_16 := ScoreLabelDim{text: ""}}
                        key_row_17 := ScoreRow{height: 21 key_name_17 := ScoreLabel{text: ""} Filler{} key_action_17 := ScoreLabelDim{text: ""}}
                        key_row_18 := ScoreRow{height: 21 key_name_18 := ScoreLabel{text: ""} Filler{} key_action_18 := ScoreLabelDim{text: ""}}
                        key_row_19 := ScoreRow{height: 21 key_name_19 := ScoreLabel{text: ""} Filler{} key_action_19 := ScoreLabelDim{text: ""}}
                        key_row_20 := ScoreRow{height: 21 key_name_20 := ScoreLabel{text: ""} Filler{} key_action_20 := ScoreLabelDim{text: ""}}
                        key_row_21 := ScoreRow{height: 21 key_name_21 := ScoreLabel{text: ""} Filler{} key_action_21 := ScoreLabelDim{text: ""}}
                        key_row_22 := ScoreRow{height: 21 key_name_22 := ScoreLabel{text: ""} Filler{} key_action_22 := ScoreLabelDim{text: ""}}
                        key_row_23 := ScoreRow{height: 21 key_name_23 := ScoreLabel{text: ""} Filler{} key_action_23 := ScoreLabelDim{text: ""}}
                    }

                    dialog_about := View{
                        visible: false width: Fill height: Fit flow: Down spacing: 4
                        about_0 := ScoreLabelDim{text: ""}
                        about_1 := ScoreLabelDim{text: ""}
                        about_2 := ScoreLabelDim{text: ""}
                        about_3 := ScoreLabelDim{text: ""}
                        about_4 := ScoreLabelDim{text: ""}
                        about_5 := ScoreLabelDim{text: ""}
                        about_6 := ScoreLabelDim{text: ""}
                    }

                    dialog_error := ScoreLabel{
                        visible: false width: Fill
                        draw_text +: {color: score.color_error}
                        text: ""
                    }

                    View{
                        width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                        dialog_hint := ScoreLabelMuted{text: "Escape closes this dialog"}
                        Filler{}
                        dialog_cancel := ScoreButton{text: "Cancel"}
                        dialog_confirm := ScoreButtonAccent{text: "Open"}
                    }
                }
            }

            context_menu := mod.widgets.ScoreContextMenu{}
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ScoreContextMenu {
    #[deref]
    view: View,
}

impl Widget for ScoreContextMenu {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            if self.view.button(cx, ids!(context_play)).clicked(actions) {
                cx.action(ScoreAction::PlayPause);
                cx.action(ScoreAction::CloseContextMenu);
            }
            if self.view.button(cx, ids!(context_loop)).clicked(actions) {
                cx.action(ScoreAction::ToggleLoop);
                cx.action(ScoreAction::CloseContextMenu);
            }
            if self.view.button(cx, ids!(context_select_more)).clicked(actions) {
                cx.action(ScoreAction::SelectMore);
                cx.action(ScoreAction::CloseContextMenu);
            }
            if self.view.button(cx, ids!(context_fingering)).clicked(actions) {
                cx.action(ScoreAction::SetAnnotationTool(AnnotationTool::Fingering));
                cx.action(ScoreAction::CloseContextMenu);
            }
            if self.view.button(cx, ids!(context_circle)).clicked(actions) {
                cx.action(ScoreAction::SetAnnotationTool(AnnotationTool::Circle));
                cx.action(ScoreAction::CloseContextMenu);
            }
            if self.view.button(cx, ids!(context_properties)).clicked(actions) {
                cx.action(ScoreAction::SetMode(ProductMode::Editor));
                cx.action(ScoreAction::SetInspectorTab(InspectorTab::Properties));
                cx.action(ScoreAction::CloseContextMenu);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let position = scope
            .data
            .get_mut::<ScoreAppState>()
            .and_then(|state| state.ui.context_menu_at);
        let Some(position) = position else {
            return DrawStep::done();
        };
        self.view.draw_walk(cx, scope, walk.with_abs_pos(position))
    }
}

/// The shell's chrome layer: menus, the modal dialog and the context menu.
///
/// It exists for one reason — compositing. Chrome has to cover the score, and
/// the score is not ordinary flat content: the canvas spends a real band of
/// `draw_depth` (0..4, and 8 while a note is dragged) to order its own ink —
/// see `ui/canvas.rs`.
///
/// The whole answer is [`DrawList2d::begin_overlay_reuse`]: an overlay draw
/// list composites after the body **and** above it in depth, because
/// `begin_overlay_inner` gives every overlay a depth floor well clear of any
/// `draw_depth` an app spends. Nothing about the score is special, and this
/// widget does no matrix work — read the docs on `begin_overlay_inner` for the
/// guarantee and its budget.
///
/// What is left here is layout: this layer claims no slot in the shell,
/// because `on_after_apply` pins the walk it reports upward to `Walk::empty()`
/// and `draw_walk` opens a root turtle sized by the pass instead.
#[derive(Script, Widget)]
pub struct ScoreOverlay {
    #[deref]
    view: View,
    #[rust]
    draw_list: Option<DrawList2d>,
}

impl ScriptHook for ScoreOverlay {
    fn on_after_new(&mut self, vm: &mut ScriptVm) {
        self.draw_list = Some(DrawList2d::script_new(vm));
    }

    fn on_after_apply(
        &mut self,
        vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        self.view.walk = Walk::empty();
        vm.with_cx_mut(|cx| {
            if let Some(draw_list) = &self.draw_list {
                draw_list.redraw(cx);
            }
        });
    }
}

impl Widget for ScoreOverlay {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    /// The incoming `walk` is ignored on purpose: this layer is not laid out
    /// by its parent, it covers the pass.
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, _walk: Walk) -> DrawStep {
        let draw_list = self.draw_list.as_mut().unwrap();
        draw_list.begin_overlay_reuse(cx);
        cx.begin_root_turtle_for_pass(self.view.layout);
        self.view
            .draw_walk_all(cx, scope, Walk::fill().with_abs_pos(Vec2d { x: 0.0, y: 0.0 }));
        cx.end_pass_sized_turtle();
        self.draw_list.as_mut().unwrap().end(cx);
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct ScoreShell {
    #[deref]
    view: View,
    #[rust]
    open_menu: u8,
    /// Drives the transient pianist controls' dwell. It only runs while they
    /// are up, so an idle reader costs nothing.
    #[rust]
    controls_timer: Timer,
    #[rust]
    annotation_bar_open: bool,
    #[rust]
    last_scrub: Option<(f64, f64)>,
    #[rust]
    synced_dialog: DialogKind,
    /// Bottom-left corner of the menu button that opened the current popup.
    #[rust]
    menu_anchor: Option<DVec2>,
}

impl ScoreShell {
    fn emit_button(&self, cx: &mut Cx, actions: &Actions, path: &[LiveId], action: ScoreAction) {
        if self.view.button(cx, path).clicked(actions) {
            cx.action(action);
        }
    }

    /// True while the pointer is over one of the transient control strips.
    /// Their own rects are the authority — a hard-coded band would drift the
    /// first time the layout moves.
    fn pointer_on_controls(&self, cx: &mut Cx, abs: DVec2) -> bool {
        [ids!(pianist_top), ids!(pianist_bottom), ids!(pianist_annotation)]
            .into_iter()
            .any(|path| {
                let rect = self.view.view(cx, path).area().rect(cx);
                rect.size.x > 0.0 && rect.size.y > 0.0 && rect.contains(abs)
            })
    }

    fn arm_controls_timer(&mut self, cx: &mut Cx) {
        if self.controls_timer.is_empty() {
            // Coarse: the dwell is measured against a wall clock, this only
            // has to wake up often enough to end it promptly.
            self.controls_timer = cx.start_interval(0.2);
        }
    }

    fn disarm_controls_timer(&mut self, cx: &mut Cx) {
        if !self.controls_timer.is_empty() {
            cx.stop_timer(self.controls_timer);
            self.controls_timer = Timer::empty();
        }
    }

    /// Pointer movement anywhere in the window reveals the controls and
    /// restarts their dwell; resting on the strip itself pins them open.
    fn handle_controls_reveal(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let pinned = match event {
            Event::MouseMove(moved) => Some(self.pointer_on_controls(cx, moved.abs)),
            Event::MouseDown(down) => Some(self.pointer_on_controls(cx, down.abs)),
            _ => None,
        };
        let now = Cx::time_now();
        let Some(state) = scope.data.get_mut::<ScoreAppState>() else {
            return;
        };
        if let Some(pinned) = pinned {
            state.ui.controls_pinned = pinned;
            if state.ui.reveal_controls(now) {
                cx.redraw_all();
            }
            self.arm_controls_timer(cx);
            return;
        }
        if self.controls_timer.is_event(event).is_some() {
            if state.ui.tick_controls(now) {
                cx.redraw_all();
            }
            if !state.ui.controls_visible {
                self.disarm_controls_timer(cx);
            }
        }
    }

    /// The popup the open menu draws into, if any.
    fn open_menu_path(&self) -> Option<&'static [LiveId]> {
        Some(match self.open_menu {
            1 => ids!(file_menu),
            2 => ids!(edit_menu),
            3 => ids!(view_menu),
            4 => ids!(play_menu),
            5 => ids!(notation_menu),
            6 => ids!(help_menu),
            _ => return None,
        })
    }

    /// A press anywhere outside the open menu closes it, the way a menu bar
    /// has always worked. Without this the popup stayed up over the score
    /// while the click went to whatever was under it.
    fn close_menu_on_outside_press(&mut self, cx: &mut Cx, event: &Event) {
        let Event::MouseDown(mouse) = event else {
            return;
        };
        let Some(popup) = self.open_menu_path() else {
            return;
        };
        let inside_popup = self.view.widget(cx, popup).area().rect(cx).contains(mouse.abs);
        let inside_bar = self
            .view
            .view(cx, ids!(menu_bar))
            .area()
            .rect(cx)
            .contains(mouse.abs);
        if !inside_popup && !inside_bar {
            self.close_menu(cx);
        }
    }

    fn close_menu(&mut self, cx: &mut Cx) {
        self.open_menu = 0;
        cx.redraw_all();
        cx.clear_all_hovers();
    }

    /// Open (or close) one menu, remembering where its button sits.
    ///
    /// The anchor is taken HERE, in the actions pass, because a widget's area
    /// is stale during the redraw it belongs to — reading it from `draw_walk`
    /// answers an empty rect every time.
    fn toggle_menu(&mut self, cx: &mut Cx, menu: u8, button: &[LiveId]) {
        self.open_menu = if self.open_menu == menu { 0 } else { menu };
        let rect = self.view.widget(cx, button).area().rect(cx);
        if rect.size.y > 0.0 {
            self.menu_anchor = Some(dvec2(rect.pos.x, rect.pos.y + rect.size.y));
        }
        cx.redraw_all();
        // A popup that opens under the pointer keeps the ordinary hover-outs
        // from arriving, so the menu-bar button that opened it stayed lit
        // forever — the "sticky grey" the reader saw. One clear puts every
        // widget's hover visual back where the pointer actually is; the row or
        // button under it lights again on the next movement.
        cx.clear_all_hovers();
    }

    fn dialog_kind(&self, scope: &mut Scope) -> DialogKind {
        scope
            .data
            .get_mut::<ScoreAppState>()
            .map(|state| state.ui.dialog)
            .unwrap_or(DialogKind::None)
    }

    /// Confirm means something different in every dialog, and in none of them
    /// does it mean "close and forget".
    fn handle_dialog_confirm(&mut self, cx: &mut Cx, scope: &mut Scope) {
        let path_text = self.view.text_input(cx, ids!(dialog_path)).text();
        let path_text = path_text.trim().to_string();
        let annotation = self.view.text_input(cx, ids!(dialog_annotation)).text();
        let annotation = annotation.trim().to_string();
        let Some(state) = scope.data.get_mut::<ScoreAppState>() else {
            return;
        };
        let draft = state.ui.draft;
        match state.ui.dialog {
            DialogKind::Open => {
                if path_text.is_empty() {
                    state.ui.dialog_error = Some("Type a path or choose Browse…".into());
                } else {
                    cx.action(ScoreAction::OpenPath(PathBuf::from(path_text)));
                }
            }
            DialogKind::SaveAs => {
                if path_text.is_empty() {
                    state.ui.dialog_error = Some("Type a path or choose Browse…".into());
                } else {
                    cx.action(ScoreAction::SavePath(PathBuf::from(path_text)));
                }
            }
            DialogKind::AnnotationText => {
                if annotation.is_empty() {
                    state.ui.dialog_error = Some("Type the text to place on the note".into());
                } else {
                    cx.action(ScoreAction::ApplyAnnotationText(annotation));
                }
            }
            DialogKind::Preferences => cx.action(ScoreAction::ApplyPreferences),
            DialogKind::ScoreSetup => cx.action(ScoreAction::ApplyScoreSetup { tempo: draft.tempo }),
            DialogKind::PageSetup => cx.action(ScoreAction::ApplyPageSetup {
                layout: draft.layout,
                zoom: draft.zoom,
            }),
            DialogKind::Keymap | DialogKind::About | DialogKind::Library | DialogKind::None => {
                cx.action(ScoreAction::CloseDialog)
            }
        }
    }

    /// A popup hangs off the bottom edge of the button that opened it. The
    /// DSL's hard-coded offsets put every menu five points ABOVE the menu bar,
    /// covering the very row it belonged to, and at x values that no longer
    /// matched the buttons.
    fn anchor_menus(&mut self, cx: &mut Cx2d) {
        let Some(anchor) = self.menu_anchor else {
            return;
        };
        let Some(popup) = self.open_menu_path() else {
            return;
        };
        if let Some(mut view) = self.view.view(cx, popup).borrow_mut() {
            view.walk.abs_pos = Some(anchor);
        }
    }

    fn sync_dialog(&mut self, cx: &mut Cx2d, state: &ScoreAppState) {
        let dialog = state.ui.dialog;
        let (title, body, confirm) = match dialog {
            DialogKind::Open => (
                "Open score",
                "Open a native workspace, a MusicXML file, or a MIDI file.",
                "Open",
            ),
            DialogKind::SaveAs => (
                "Save score as",
                "Writes the semantic model, annotations, and the edit journal.",
                "Save",
            ),
            DialogKind::ScoreSetup => (
                "Score setup",
                "What this score is, and the tempo it plays back at.",
                "Apply",
            ),
            DialogKind::PageSetup => (
                "Page and staff size",
                "How pages are laid out, and how large the staff is drawn.",
                "Apply",
            ),
            DialogKind::Preferences => (
                "Preferences",
                "Kept between launches in your user configuration directory.",
                "Save",
            ),
            DialogKind::Keymap => ("Keyboard map", "Every binding this build listens for.", "Done"),
            DialogKind::About => ("About Score", "", "Done"),
            DialogKind::AnnotationText => (
                "Add annotation",
                "The text stays anchored to the note you tapped.",
                "Add",
            ),
            DialogKind::Library => (
                "Music library",
                "Every score in your music folder. Pick one to open and play it.",
                "Done",
            ),
            DialogKind::None => ("", "", ""),
        };
        self.view.label(cx, ids!(dialog_title)).set_text(cx, title);
        self.view.label(cx, ids!(dialog_body)).set_text(cx, body);
        self.view.label(cx, ids!(dialog_body)).set_visible(cx, !body.is_empty());
        self.view.button(cx, ids!(dialog_confirm)).set_text(cx, confirm);
        // Keymap and About have one outcome; a Cancel next to Done means nothing.
        self.view.button(cx, ids!(dialog_cancel)).set_visible(
            cx,
            !matches!(
                dialog,
                DialogKind::Keymap | DialogKind::About | DialogKind::Library
            ),
        );

        let file = matches!(dialog, DialogKind::Open | DialogKind::SaveAs);
        self.view.view(cx, ids!(dialog_file)).set_visible(cx, file);
        self.view
            .view(cx, ids!(dialog_text))
            .set_visible(cx, dialog == DialogKind::AnnotationText);
        self.view
            .view(cx, ids!(dialog_prefs))
            .set_visible(cx, dialog == DialogKind::Preferences);
        self.view
            .view(cx, ids!(dialog_score_setup))
            .set_visible(cx, dialog == DialogKind::ScoreSetup);
        self.view
            .view(cx, ids!(dialog_page_setup))
            .set_visible(cx, dialog == DialogKind::PageSetup);
        self.view
            .view(cx, ids!(dialog_keymap))
            .set_visible(cx, dialog == DialogKind::Keymap);
        self.view
            .view(cx, ids!(dialog_about))
            .set_visible(cx, dialog == DialogKind::About);
        self.view
            .view(cx, ids!(dialog_library))
            .set_visible(cx, dialog == DialogKind::Library);

        match &state.ui.dialog_error {
            Some(error) => {
                self.view.label(cx, ids!(dialog_error)).set_visible(cx, true);
                self.view.label(cx, ids!(dialog_error)).set_text(cx, error);
            }
            None => self.view.label(cx, ids!(dialog_error)).set_visible(cx, false),
        }

        if file {
            self.sync_file_dialog(cx, state);
        }
        if dialog == DialogKind::Preferences {
            self.sync_prefs_dialog(cx, state);
        }
        if dialog == DialogKind::ScoreSetup {
            self.sync_score_setup(cx, state);
        }
        if dialog == DialogKind::PageSetup {
            self.sync_page_setup(cx, state);
        }
        if dialog == DialogKind::Keymap {
            self.sync_keymap(cx);
        }
        if dialog == DialogKind::About {
            self.sync_about(cx, state);
        }
        if dialog == DialogKind::Library {
            self.sync_library(cx, state);
        }
    }

    /// The music library browser. Rows the folder does not fill are hidden
    /// rather than left blank, and a folder with nothing in it says why in a
    /// sentence instead of showing an empty box.
    fn sync_library(&mut self, cx: &mut Cx2d, state: &ScoreAppState) {
        self.view
            .label(cx, ids!(library_summary))
            .set_text(cx, &state.library.summary());
        // An empty shelf gets a sentence, not a tall empty box.
        let listed = !state.library.entries().is_empty();
        self.view.view(cx, ids!(music_list)).set_visible(cx, listed);
        match state.library.empty_state() {
            Some(explanation) => {
                self.view.label(cx, ids!(music_empty)).set_visible(cx, true);
                self.view.label(cx, ids!(music_empty)).set_text(cx, explanation);
            }
            None => self.view.label(cx, ids!(music_empty)).set_visible(cx, false),
        }
        self.view.label(cx, ids!(music_credit)).set_text(
            cx,
            state.performance_credit.unwrap_or_default(),
        );
        let open_path = state.document.path();
        let visible = state.library_visible();
        for (row, path) in LIBRARY_ROWS.iter().enumerate() {
            match visible.get(row) {
                Some(entry) => {
                    let playing = open_path
                        .is_some_and(|open| crate::library::same_file(open, &entry.path));
                    let recent = state
                        .prefs
                        .recent
                        .first()
                        .is_some_and(|last| crate::library::same_file(last, &entry.path));
                    // The piece on screen, then the one opened last: the
                    // browser sits next to the existing recents list rather
                    // than keeping a second one of its own.
                    let mark = if playing {
                        "✓ "
                    } else if recent {
                        "· "
                    } else {
                        "   "
                    };
                    self.view.button(cx, path).set_visible(cx, true);
                    self.view
                        .button(cx, path)
                        .set_text(cx, &format!("{mark}{}", entry.line()));
                    self.view.button(cx, path).set_enabled(cx, !playing);
                }
                None => self.view.button(cx, path).set_visible(cx, false),
            }
        }
    }

    /// The whole sound panel: two instruments, brightness, and the room.
    /// Everything on it reaches the sounding instrument — there is nothing
    /// here that only some of the instruments answer to.
    fn sync_sound_panel(&mut self, cx: &mut Cx2d, state: &ScoreAppState) {
        let sound = &state.sound;

        // The list. Two instruments, each saying what it is; the selected one
        // is ticked. More rows than instruments simply stay hidden, so adding
        // an instrument back is a row in a table plus a slot in the script.
        let entries = sound::instrument_list();
        let selected = state.selected_instrument();
        for (slot, row) in INSTRUMENT_ROWS.iter().enumerate() {
            let entry = entries.get(slot);
            let shown = entry.is_some();
            self.view.button(cx, row).set_visible(cx, shown);
            self.view
                .label(cx, INSTRUMENT_DESCRIPTIONS[slot])
                .set_visible(cx, shown);
            let Some(entry) = entry else { continue };
            let is_selected = entry.id == selected;
            self.view.button(cx, row).set_text(
                cx,
                &format!(
                    "{}{}",
                    if is_selected { "\u{2713} " } else { "    " },
                    entry.name
                ),
            );
            self.view
                .label(cx, INSTRUMENT_DESCRIPTIONS[slot])
                .set_text(cx, entry.description);
        }

        for (index, param) in SoundParam::ALL.into_iter().enumerate() {
            let value = param.get(sound);
            self.view
                .slider(cx, SOUND_SLIDERS[index])
                .set_value(cx, param.to_position(value));
            self.view
                .label(cx, SOUND_SLIDER_NAMES[index])
                .set_text(cx, param.label());
            self.view
                .label(cx, SOUND_SLIDER_VALUES[index])
                .set_text(cx, &param.format(value));
        }

        // The room's own label and buttons are synced with the rest of the
        // shell in draw_walk; this panel adds the hint under them.
        self.view.label(cx, ids!(sound_hint)).set_text(
            cx,
            match state.ui.sound_focus {
                Some(param) => param.hint(),
                None => "Both controls reach the sounding instrument straight away, and \
                         switching instrument dissolves rather than cuts — so either is \
                         safe mid-phrase.",
            },
        );
    }

    fn sync_file_dialog(&mut self, cx: &mut Cx2d, state: &ScoreAppState) {
        let recent = &state.prefs.recent;
        self.view
            .label(cx, ids!(dialog_recent_header))
            .set_visible(cx, !recent.is_empty());
        for index in 0..RECENT_SLOTS {
            let row = recent_row(index);
            match recent.get(index) {
                Some(path) => {
                    self.view.button(cx, row).set_visible(cx, true);
                    self.view.button(cx, row).set_text(cx, &path.display().to_string());
                }
                None => self.view.button(cx, row).set_visible(cx, false),
            }
        }
        let formats = if state.ui.dialog == DialogKind::Open {
            format!(
                "Opens .{} workspaces, .musicxml / .mxl / .xml, and .mid / .midi.",
                makepad_score_ui_native_extension()
            )
        } else {
            format!(
                "Saved as .{} — the native workspace, not the file it was imported from.",
                makepad_score_ui_native_extension()
            )
        };
        self.view.label(cx, ids!(dialog_formats)).set_text(cx, &formats);
    }

    fn sync_prefs_dialog(&mut self, cx: &mut Cx2d, state: &ScoreAppState) {
        let prefs = &state.prefs;
        self.view.button(cx, ids!(pref_start)).set_text(
            cx,
            if prefs.start_in_editor { "Editor" } else { "Pianist" },
        );
        for (path, value) in [
            (ids!(pref_audition), prefs.audition_on_hover),
            (ids!(pref_follow), prefs.follow_cursor),
            (ids!(pref_metronome), prefs.metronome),
            (ids!(pref_count_in), prefs.count_in),
        ] {
            self.view
                .button(cx, path)
                .set_text(cx, if value { "On" } else { "Off" });
        }
        self.view.button(cx, ids!(pref_paper)).set_text(
            cx,
            if prefs.dark_paper { "Dark paper" } else { "Light paper" },
        );
        let where_ = match crate::prefs::ScorePrefs::path() {
            Some(path) => format!("Stored in {}", path.display()),
            None => "No writable configuration directory".to_string(),
        };
        self.view.label(cx, ids!(pref_path)).set_text(cx, &where_);
    }

    fn sync_score_setup(&mut self, cx: &mut Cx2d, state: &ScoreAppState) {
        self.view
            .label(cx, ids!(setup_title))
            .set_text(cx, state.document.title());
        let parts: Vec<&str> = state.parts.iter().map(|part| part.name.as_str()).collect();
        self.view.label(cx, ids!(setup_parts)).set_text(
            cx,
            &if parts.is_empty() { "—".to_string() } else { parts.join(", ") },
        );
        self.view
            .label(cx, ids!(setup_key))
            .set_text(cx, &state.key_and_meter());
        self.view.label(cx, ids!(setup_extent)).set_text(
            cx,
            &format!(
                "{} bars · {} pages",
                state.document.score().measures.len(),
                state.document.page_count()
            ),
        );
        self.view
            .label(cx, ids!(setup_tempo))
            .set_text(cx, &format!("{:.0} BPM", state.ui.draft.tempo));
        self.view.label(cx, ids!(setup_note)).set_text(
            cx,
            "Title, parts, key and meter come from the file; this build reads them, it does not rewrite them.",
        );
    }

    fn sync_page_setup(&mut self, cx: &mut Cx2d, state: &ScoreAppState) {
        let draft = state.ui.draft;
        for (path, layout) in [
            (ids!(page_single), PageLayout::Single),
            (ids!(page_two), PageLayout::TwoUp),
            (ids!(page_continuous), PageLayout::Continuous),
        ] {
            let selected = draft.layout == layout;
            self.view.button(cx, path).set_text(
                cx,
                &if selected {
                    format!("✓ {}", layout.label())
                } else {
                    layout.label().to_string()
                },
            );
        }
        self.view
            .label(cx, ids!(staff_value))
            .set_text(cx, &format!("{}%", (draft.zoom * 100.0).round()));
        self.view.label(cx, ids!(page_size)).set_text(
            cx,
            &format!(
                "{:.0} × {:.0} staff spaces · {} page{}",
                crate::document::PAGE_WIDTH_SP,
                crate::document::PAGE_HEIGHT_SP,
                state.document.page_count(),
                if state.document.page_count() == 1 { "" } else { "s" }
            ),
        );
        self.view.label(cx, ids!(page_note)).set_text(
            cx,
            "Staff size scales the engraved page on screen; the page proportions are fixed in this build.",
        );
    }

    fn sync_keymap(&mut self, cx: &mut Cx2d) {
        for index in 0..KEYMAP_SLOTS {
            let (row, name, action) = keymap_row(index);
            match KEYMAP_ROWS.get(index) {
                Some((key, description)) => {
                    self.view.view(cx, row).set_visible(cx, true);
                    self.view.label(cx, name).set_text(cx, key);
                    self.view.label(cx, action).set_text(cx, description);
                }
                None => self.view.view(cx, row).set_visible(cx, false),
            }
        }
    }

    fn sync_about(&mut self, cx: &mut Cx2d, state: &ScoreAppState) {
        let lines: [String; ABOUT_SLOTS] = [
            format!("Score {} · Makepad", env!("CARGO_PKG_VERSION")),
            "Semantic music notation: one model, engraved incrementally, played from the audio clock.".to_string(),
            format!("Music font · {}", crate::font::music_font_summary()),
            format!("Open score · {}", state.document.title()),
            format!(
                "{} bars · {} pages · {} part{}",
                state.document.score().measures.len(),
                state.document.page_count(),
                state.parts.len(),
                if state.parts.len() == 1 { "" } else { "s" }
            ),
            format!("Audio · {}", state.midi_status()),
            state.performance_credit.map_or_else(
                || "F1 shows the keyboard map. Escape closes any dialog.".to_string(),
                |credit| credit.to_string(),
            ),
        ];
        for (index, line) in lines.iter().enumerate() {
            self.view.label(cx, about_row(index)).set_text(cx, line);
        }
    }
}

/// Named-row helpers. The DSL declares a fixed number of slots because the
/// script has no loops; these keep the Rust side from repeating the ids.
const RECENT_SLOTS: usize = 6;
const KEYMAP_SLOTS: usize = 24;
const ABOUT_SLOTS: usize = 7;

/// One fixed slot per instrument row: the row itself and its description.
/// The script has no loops, so the slots are declared and `instrument_list`
/// fills as many as it needs. Two instruments ship; kept honest by
/// `every_declared_row_matches_the_list_it_shows`.
const INSTRUMENT_ROWS: &[&[LiveId]] = ids_array!(inst_0, inst_1,);
const INSTRUMENT_DESCRIPTIONS: &[&[LiveId]] = ids_array!(inst_0_desc, inst_1_desc,);

/// One slider per [`SoundParam`], in `SoundParam::ALL` order.
const SOUND_SLIDERS: &[&[LiveId]] = ids_array!(sl_brightness, sl_reverb,);
const SOUND_SLIDER_NAMES: &[&[LiveId]] = ids_array!(sl_brightness_name, sl_reverb_name,);
const SOUND_SLIDER_VALUES: &[&[LiveId]] = ids_array!(sl_brightness_value, sl_reverb_value,);

/// One row per listed piece, in the sidebar. The list pages when a chosen
/// folder holds more than this; the shipped shelf never does.
const LIBRARY_ROWS: &[&[LiveId]] = ids_array!(
    music_0, music_1, music_2, music_3, music_4, music_5, music_6, music_7,
    music_8, music_9, music_10, music_11, music_12, music_13, music_14, music_15,
);

fn recent_row(index: usize) -> &'static [LiveId] {
    match index {
        0 => ids!(recent_0),
        1 => ids!(recent_1),
        2 => ids!(recent_2),
        3 => ids!(recent_3),
        4 => ids!(recent_4),
        _ => ids!(recent_5),
    }
}

fn keymap_row(index: usize) -> (&'static [LiveId], &'static [LiveId], &'static [LiveId]) {
    match index {
        0 => (ids!(key_row_0), ids!(key_name_0), ids!(key_action_0)),
        1 => (ids!(key_row_1), ids!(key_name_1), ids!(key_action_1)),
        2 => (ids!(key_row_2), ids!(key_name_2), ids!(key_action_2)),
        3 => (ids!(key_row_3), ids!(key_name_3), ids!(key_action_3)),
        4 => (ids!(key_row_4), ids!(key_name_4), ids!(key_action_4)),
        5 => (ids!(key_row_5), ids!(key_name_5), ids!(key_action_5)),
        6 => (ids!(key_row_6), ids!(key_name_6), ids!(key_action_6)),
        7 => (ids!(key_row_7), ids!(key_name_7), ids!(key_action_7)),
        8 => (ids!(key_row_8), ids!(key_name_8), ids!(key_action_8)),
        9 => (ids!(key_row_9), ids!(key_name_9), ids!(key_action_9)),
        10 => (ids!(key_row_10), ids!(key_name_10), ids!(key_action_10)),
        11 => (ids!(key_row_11), ids!(key_name_11), ids!(key_action_11)),
        12 => (ids!(key_row_12), ids!(key_name_12), ids!(key_action_12)),
        13 => (ids!(key_row_13), ids!(key_name_13), ids!(key_action_13)),
        14 => (ids!(key_row_14), ids!(key_name_14), ids!(key_action_14)),
        15 => (ids!(key_row_15), ids!(key_name_15), ids!(key_action_15)),
        16 => (ids!(key_row_16), ids!(key_name_16), ids!(key_action_16)),
        17 => (ids!(key_row_17), ids!(key_name_17), ids!(key_action_17)),
        18 => (ids!(key_row_18), ids!(key_name_18), ids!(key_action_18)),
        19 => (ids!(key_row_19), ids!(key_name_19), ids!(key_action_19)),
        20 => (ids!(key_row_20), ids!(key_name_20), ids!(key_action_20)),
        21 => (ids!(key_row_21), ids!(key_name_21), ids!(key_action_21)),
        22 => (ids!(key_row_22), ids!(key_name_22), ids!(key_action_22)),
        _ => (ids!(key_row_23), ids!(key_name_23), ids!(key_action_23)),
    }
}

fn about_row(index: usize) -> &'static [LiveId] {
    match index {
        0 => ids!(about_0),
        1 => ids!(about_1),
        2 => ids!(about_2),
        3 => ids!(about_3),
        4 => ids!(about_4),
        5 => ids!(about_5),
        _ => ids!(about_6),
    }
}

fn part_rows(part: usize) -> (&'static [LiveId], &'static [LiveId]) {
    match part {
        0 => (ids!(part_0), ids!(part_0_name)),
        1 => (ids!(part_1), ids!(part_1_name)),
        2 => (ids!(part_2), ids!(part_2_name)),
        _ => (ids!(part_3), ids!(part_3_name)),
    }
}

fn mix_ids(part: usize) -> MixIds {
    match part {
        0 => MixIds {
            strip: ids!(mix_0),
            name: ids!(mix_0_name),
            gain: ids!(mix_0_gain),
            pan: ids!(mix_0_pan),
            mute: ids!(mix_0_mute),
            solo: ids!(mix_0_solo),
        },
        1 => MixIds {
            strip: ids!(mix_1),
            name: ids!(mix_1_name),
            gain: ids!(mix_1_gain),
            pan: ids!(mix_1_pan),
            mute: ids!(mix_1_mute),
            solo: ids!(mix_1_solo),
        },
        2 => MixIds {
            strip: ids!(mix_2),
            name: ids!(mix_2_name),
            gain: ids!(mix_2_gain),
            pan: ids!(mix_2_pan),
            mute: ids!(mix_2_mute),
            solo: ids!(mix_2_solo),
        },
        _ => MixIds {
            strip: ids!(mix_3),
            name: ids!(mix_3_name),
            gain: ids!(mix_3_gain),
            pan: ids!(mix_3_pan),
            mute: ids!(mix_3_mute),
            solo: ids!(mix_3_solo),
        },
    }
}

struct MixIds {
    strip: &'static [LiveId],
    name: &'static [LiveId],
    gain: &'static [LiveId],
    pan: &'static [LiveId],
    mute: &'static [LiveId],
    solo: &'static [LiveId],
}

fn mix_buttons(part: usize) -> (&'static [LiveId], &'static [LiveId], &'static [LiveId], &'static [LiveId]) {
    match part {
        0 => (ids!(mix_0_down), ids!(mix_0_up), ids!(mix_0_left), ids!(mix_0_right)),
        1 => (ids!(mix_1_down), ids!(mix_1_up), ids!(mix_1_left), ids!(mix_1_right)),
        2 => (ids!(mix_2_down), ids!(mix_2_up), ids!(mix_2_left), ids!(mix_2_right)),
        _ => (ids!(mix_3_down), ids!(mix_3_up), ids!(mix_3_left), ids!(mix_3_right)),
    }
}

fn part_mute_solo(part: usize) -> (&'static [LiveId], &'static [LiveId]) {
    match part {
        0 => (ids!(part_0_mute), ids!(part_0_solo)),
        1 => (ids!(part_1_mute), ids!(part_1_solo)),
        2 => (ids!(part_2_mute), ids!(part_2_solo)),
        _ => (ids!(part_3_mute), ids!(part_3_solo)),
    }
}

fn history_rows(index: usize) -> (&'static [LiveId], &'static [LiveId]) {
    match index {
        0 => (ids!(history_0), ids!(history_0_text)),
        1 => (ids!(history_1), ids!(history_1_text)),
        2 => (ids!(history_2), ids!(history_2_text)),
        3 => (ids!(history_3), ids!(history_3_text)),
        _ => (ids!(history_4), ids!(history_4_text)),
    }
}

fn makepad_score_ui_native_extension() -> &'static str {
    crate::document::NATIVE_EXTENSION
}

impl Widget for ScoreShell {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.close_menu_on_outside_press(cx, event);
        self.view.handle_event(cx, event, scope);
        self.handle_controls_reveal(cx, event, scope);
        let Event::Actions(actions) = event else {
            return;
        };

        if self.view.button(cx, ids!(menu_file_button)).clicked(actions) { self.toggle_menu(cx, 1, ids!(menu_file_button)); }
        if self.view.button(cx, ids!(menu_edit_button)).clicked(actions) { self.toggle_menu(cx, 2, ids!(menu_edit_button)); }
        if self.view.button(cx, ids!(menu_view_button)).clicked(actions) { self.toggle_menu(cx, 3, ids!(menu_view_button)); }
        if self.view.button(cx, ids!(menu_play_button)).clicked(actions) { self.toggle_menu(cx, 4, ids!(menu_play_button)); }
        if self.view.button(cx, ids!(menu_notation_button)).clicked(actions) { self.toggle_menu(cx, 5, ids!(menu_notation_button)); }
        if self.view.button(cx, ids!(menu_help_button)).clicked(actions) { self.toggle_menu(cx, 6, ids!(menu_help_button)); }

        self.emit_button(cx, actions, ids!(mode_pianist), ScoreAction::SetMode(ProductMode::Pianist));
        self.emit_button(cx, actions, ids!(pianist_edit), ScoreAction::SetMode(ProductMode::Editor));
        self.emit_button(cx, actions, ids!(layout_single), ScoreAction::SetPageLayout(PageLayout::Single));
        self.emit_button(cx, actions, ids!(layout_two), ScoreAction::SetPageLayout(PageLayout::TwoUp));
        self.emit_button(cx, actions, ids!(layout_continuous), ScoreAction::SetPageLayout(PageLayout::Continuous));
        self.emit_button(cx, actions, ids!(layout_overview), ScoreAction::FitAllPages);
        self.emit_button(cx, actions, ids!(zoom_out), ScoreAction::ZoomBy(1.0 / 1.12));
        self.emit_button(cx, actions, ids!(zoom_in), ScoreAction::ZoomBy(1.12));
        self.emit_button(cx, actions, ids!(fit_page), ScoreAction::FitPage);
        for (path, tool) in [
            (ids!(tool_navigate), ScoreTool::Navigate),
            (ids!(tool_choose), ScoreTool::Select),
            (ids!(tool_edit), ScoreTool::Edit),
        ] {
            self.emit_button(cx, actions, path, ScoreAction::SetTool(tool));
        }
        self.emit_button(cx, actions, ids!(tool_transpose_up), ScoreAction::Transpose(1));
        self.emit_button(cx, actions, ids!(tool_transpose_down), ScoreAction::Transpose(-1));
        self.emit_button(cx, actions, ids!(tool_delete), ScoreAction::DeleteSelection);
        // The select tool is "no annotation tool": it puts the pointer back to
        // selecting notes, which is what the arrow promises.
        self.emit_button(cx, actions, ids!(tool_select), ScoreAction::SetAnnotationTool(AnnotationTool::None));
        self.emit_button(cx, actions, ids!(duration_4), ScoreAction::SetDuration(4));
        self.emit_button(cx, actions, ids!(duration_5), ScoreAction::SetDuration(5));
        self.emit_button(cx, actions, ids!(duration_6), ScoreAction::SetDuration(6));
        self.emit_button(cx, actions, ids!(duration_7), ScoreAction::SetDuration(7));

        for (path, command) in [
            (ids!(palette_staccato), PaletteCommand::Staccato),
            (ids!(p_staccato), PaletteCommand::Staccato),
            (ids!(palette_accent), PaletteCommand::Accent),
            (ids!(p_accent), PaletteCommand::Accent),
            (ids!(palette_tenuto), PaletteCommand::Tenuto),
            (ids!(p_tenuto), PaletteCommand::Tenuto),
            (ids!(palette_flat), PaletteCommand::Flat),
            (ids!(p_flat), PaletteCommand::Flat),
            (ids!(palette_natural), PaletteCommand::Natural),
            (ids!(p_natural), PaletteCommand::Natural),
            (ids!(palette_sharp), PaletteCommand::Sharp),
            (ids!(p_sharp), PaletteCommand::Sharp),
        ] {
            self.emit_button(cx, actions, path, ScoreAction::ApplyPalette(command));
        }

        for (path, tool) in [
            (ids!(annotate_highlight), AnnotationTool::Highlight),
            (ids!(p_highlight), AnnotationTool::Highlight),
            (ids!(annotate_circle), AnnotationTool::Circle),
            (ids!(p_circle), AnnotationTool::Circle),
            (ids!(annotate_text), AnnotationTool::Text),
            (ids!(p_text), AnnotationTool::Text),
            (ids!(annotate_fingering), AnnotationTool::Fingering),
            (ids!(p_fingering), AnnotationTool::Fingering),
            (ids!(annotate_ink), AnnotationTool::Ink),
            (ids!(p_ink), AnnotationTool::Ink),
        ] {
            self.emit_button(cx, actions, path, ScoreAction::SetAnnotationTool(tool));
        }
        if self.view.button(cx, ids!(pianist_annotate)).clicked(actions) {
            self.annotation_bar_open = !self.annotation_bar_open;
            cx.redraw_all();
        }
        if self.view.button(cx, ids!(p_annotation_close)).clicked(actions) {
            self.annotation_bar_open = false;
            cx.action(ScoreAction::SetAnnotationTool(AnnotationTool::None));
        }

        for path in [ids!(transport_play), ids!(pianist_play)] {
            self.emit_button(cx, actions, path, ScoreAction::PlayPause);
        }
        self.emit_button(cx, actions, ids!(transport_stop), ScoreAction::Stop);
        self.emit_button(cx, actions, ids!(pianist_prev), ScoreAction::PageDelta(-1));
        self.emit_button(cx, actions, ids!(pianist_next), ScoreAction::PageDelta(1));
        for path in [ids!(transport_metronome), ids!(pianist_metronome)] {
            self.emit_button(cx, actions, path, ScoreAction::ToggleMetronome);
        }
        for path in [ids!(transport_loop), ids!(pianist_loop)] {
            self.emit_button(cx, actions, path, ScoreAction::ToggleLoop);
        }
        for path in [ids!(transport_follow), ids!(pianist_follow)] {
            self.emit_button(cx, actions, path, ScoreAction::ToggleFollow);
        }
        self.emit_button(cx, actions, ids!(transport_count_in), ScoreAction::ToggleCountIn);
        for (path, preset) in [
            (ids!(room_practice), REVERB_PRESETS[0].0),
            (ids!(room_studio), REVERB_PRESETS[1].0),
            (ids!(room_small_hall), REVERB_PRESETS[2].0),
            (ids!(room_concert_hall), REVERB_PRESETS[3].0),
            (ids!(room_cathedral), REVERB_PRESETS[4].0),
        ] {
            self.emit_button(cx, actions, path, ScoreAction::SetReverbPreset(preset));
        }

        // The sound panel. Clicking a row picks that instrument; the engine
        // follows it. The list is rebuilt here from the same source the panel
        // drew from, so a row can never mean a different instrument than the
        // one it shows.
        {
            let entries = sound::instrument_list();
            let mut picked = None;
            for (slot, row) in INSTRUMENT_ROWS.iter().enumerate() {
                let Some(entry) = entries.get(slot) else { break };
                if self.view.button(cx, row).clicked(actions) {
                    picked = Some(entry.id);
                }
            }
            if let Some(id) = picked {
                cx.action(ScoreAction::SelectInstrument(id));
            }
        }
        for (index, param) in SoundParam::ALL.into_iter().enumerate() {
            if let Some(position) = self.view.slider(cx, SOUND_SLIDERS[index]).slided(actions) {
                cx.action(ScoreAction::SetSoundParam {
                    param,
                    value: param.from_position(position),
                });
            }
        }
        for path in [ids!(pianist_sound), ids!(play_sound)] {
            if self.view.button(cx, path).clicked(actions) {
                cx.action(ScoreAction::SetMode(ProductMode::Editor));
                cx.action(ScoreAction::SetInspectorTab(InspectorTab::Sound));
                self.close_menu(cx);
            }
        }

        // The music library browser.
        for path in [ids!(pianist_music), ids!(file_library)] {
            if self.view.button(cx, path).clicked(actions) {
                cx.action(ScoreAction::OpenDialog(DialogKind::Library));
                self.close_menu(cx);
            }
        }
        for (row, path) in LIBRARY_ROWS.iter().enumerate() {
            self.emit_button(cx, actions, path, ScoreAction::OpenLibraryEntry(row));
        }
        self.emit_button(cx, actions, ids!(library_rescan), ScoreAction::RescanLibrary);
        for path in [ids!(library_browse), ids!(music_folder)] {
            if self.view.button(cx, path).clicked(actions) {
                cx.action(ScoreAction::Browse(BrowseTarget::LibraryDirectory));
            }
        }
        if self
            .view
            .text_input(cx, ids!(library_dir_input))
            .returned(actions)
            .is_some()
        {
            let typed = self.view.text_input(cx, ids!(library_dir_input)).text();
            let typed = typed.trim().to_string();
            if !typed.is_empty() {
                cx.action(ScoreAction::SetLibraryDir(PathBuf::from(typed)));
            }
        }
        self.emit_button(cx, actions, ids!(pianist_fit), ScoreAction::FitPage);
        self.emit_button(cx, actions, ids!(pianist_overview), ScoreAction::FitAllPages);

        if self.view.button(cx, ids!(tempo_down)).clicked(actions) {
            if let Some(state) = scope.data.get_mut::<ScoreAppState>() {
                cx.action(ScoreAction::SetTempo(state.practice.tempo - 2.0));
            }
        }
        if self.view.button(cx, ids!(tempo_up)).clicked(actions) {
            if let Some(state) = scope.data.get_mut::<ScoreAppState>() {
                cx.action(ScoreAction::SetTempo(state.practice.tempo + 2.0));
            }
        }
        if let Some(value) = self.view.slider(cx, ids!(transport_scrub)).slided(actions) {
            let now = Cx::time_now();
            let speed = self.last_scrub.map_or(1.0, |(last, time)| {
                ((value - last).abs() / (now - time).max(1.0 / 240.0) / 16.0).clamp(0.2, 8.0)
            }) as f32;
            self.last_scrub = Some((value, now));
            if let Some(state) = scope.data.get_mut::<ScoreAppState>() {
                state.scrub_quarter(value, speed);
            }
            cx.action(ScoreAction::SeekQuarter(value));
        }

        self.emit_button(cx, actions, ids!(inspector_properties), ScoreAction::SetInspectorTab(InspectorTab::Properties));
        self.emit_button(cx, actions, ids!(inspector_mixer), ScoreAction::SetInspectorTab(InspectorTab::Mixer));
        self.emit_button(cx, actions, ids!(inspector_sound), ScoreAction::SetInspectorTab(InspectorTab::Sound));
        self.emit_button(cx, actions, ids!(inspector_history), ScoreAction::SetInspectorTab(InspectorTab::History));
        self.emit_button(cx, actions, ids!(history_undo), ScoreAction::Undo);
        self.emit_button(cx, actions, ids!(history_redo), ScoreAction::Redo);
        for part in 0..4 {
            let (mute, solo) = part_mute_solo(part);
            self.emit_button(cx, actions, mute, ScoreAction::TogglePartMute(part));
            self.emit_button(cx, actions, solo, ScoreAction::TogglePartSolo(part));
            let ids = mix_ids(part);
            let (down, up, left, right) = mix_buttons(part);
            self.emit_button(cx, actions, down, ScoreAction::SetPartGain { part, delta: -0.05 });
            self.emit_button(cx, actions, up, ScoreAction::SetPartGain { part, delta: 0.05 });
            self.emit_button(cx, actions, left, ScoreAction::SetPartPan { part, delta: -0.1 });
            self.emit_button(cx, actions, right, ScoreAction::SetPartPan { part, delta: 0.1 });
            self.emit_button(cx, actions, ids.mute, ScoreAction::TogglePartMute(part));
            self.emit_button(cx, actions, ids.solo, ScoreAction::TogglePartSolo(part));
        }

        let mut picked_menu = false;
        macro_rules! menu_action {
            ($id:ident, $action:expr) => {
                if self.view.button(cx, ids!($id)).clicked(actions) {
                    cx.action($action);
                    picked_menu = true;
                }
            };
        }
        menu_action!(file_new, ScoreAction::NewDemo);
        menu_action!(file_open, ScoreAction::OpenDialog(DialogKind::Open));
        menu_action!(file_save, ScoreAction::Save);
        menu_action!(file_save_as, ScoreAction::OpenDialog(DialogKind::SaveAs));
        menu_action!(file_setup, ScoreAction::OpenDialog(DialogKind::ScoreSetup));
        menu_action!(file_page_setup, ScoreAction::OpenDialog(DialogKind::PageSetup));
        menu_action!(file_prefs, ScoreAction::OpenDialog(DialogKind::Preferences));
        menu_action!(edit_prefs, ScoreAction::OpenDialog(DialogKind::Preferences));
        menu_action!(file_quit, ScoreAction::Quit);
        menu_action!(edit_undo, ScoreAction::Undo);
        menu_action!(edit_redo, ScoreAction::Redo);
        menu_action!(edit_select_all, ScoreAction::SelectAll);
        menu_action!(edit_select_more, ScoreAction::SelectMore);
        menu_action!(edit_clear, ScoreAction::ClearSelection);
        menu_action!(view_pianist, ScoreAction::SetMode(ProductMode::Pianist));
        menu_action!(view_single, ScoreAction::SetPageLayout(PageLayout::Single));
        menu_action!(view_two, ScoreAction::SetPageLayout(PageLayout::TwoUp));
        menu_action!(view_continuous, ScoreAction::SetPageLayout(PageLayout::Continuous));
        menu_action!(view_overview, ScoreAction::FitAllPages);
        menu_action!(view_fit, ScoreAction::FitPage);
        menu_action!(view_zoom_in, ScoreAction::ZoomBy(1.12));
        menu_action!(view_zoom_out, ScoreAction::ZoomBy(1.0 / 1.12));
        menu_action!(play_toggle, ScoreAction::PlayPause);
        menu_action!(play_stop, ScoreAction::Stop);
        menu_action!(play_metro, ScoreAction::ToggleMetronome);
        menu_action!(play_count, ScoreAction::ToggleCountIn);
        menu_action!(play_loop, ScoreAction::ToggleLoop);
        menu_action!(play_follow, ScoreAction::ToggleFollow);
        menu_action!(notation_staccato, ScoreAction::ApplyPalette(PaletteCommand::Staccato));
        menu_action!(notation_accent, ScoreAction::ApplyPalette(PaletteCommand::Accent));
        menu_action!(notation_tenuto, ScoreAction::ApplyPalette(PaletteCommand::Tenuto));
        menu_action!(notation_sharp, ScoreAction::ApplyPalette(PaletteCommand::Sharp));
        menu_action!(notation_flat, ScoreAction::ApplyPalette(PaletteCommand::Flat));
        menu_action!(notation_natural, ScoreAction::ApplyPalette(PaletteCommand::Natural));
        menu_action!(notation_text, ScoreAction::SetAnnotationTool(AnnotationTool::Text));
        menu_action!(notation_fingering, ScoreAction::SetAnnotationTool(AnnotationTool::Fingering));
        menu_action!(help_keymap, ScoreAction::OpenDialog(DialogKind::Keymap));
        menu_action!(help_about, ScoreAction::OpenDialog(DialogKind::About));
        if picked_menu {
            self.close_menu(cx);
        }

        // Dialogs.
        for path in [ids!(dialog_cancel), ids!(dialog_close)] {
            self.emit_button(cx, actions, path, ScoreAction::CloseDialog);
        }
        for (path, toggle) in [
            (ids!(pref_start), PrefToggle::StartInEditor),
            (ids!(pref_audition), PrefToggle::AuditionOnHover),
            (ids!(pref_follow), PrefToggle::FollowCursor),
            (ids!(pref_metronome), PrefToggle::Metronome),
            (ids!(pref_count_in), PrefToggle::CountIn),
            (ids!(pref_paper), PrefToggle::DarkPaper),
        ] {
            self.emit_button(cx, actions, path, ScoreAction::TogglePref(toggle));
        }
        for (path, layout) in [
            (ids!(page_single), PageLayout::Single),
            (ids!(page_two), PageLayout::TwoUp),
            (ids!(page_continuous), PageLayout::Continuous),
        ] {
            self.emit_button(cx, actions, path, ScoreAction::SetDialogLayout(layout));
        }
        let draft = scope
            .data
            .get_mut::<ScoreAppState>()
            .map(|state| state.ui.draft);
        if let Some(draft) = draft {
            if self.view.button(cx, ids!(staff_down)).clicked(actions) {
                cx.action(ScoreAction::SetDialogZoom(draft.zoom / 1.12));
            }
            if self.view.button(cx, ids!(staff_up)).clicked(actions) {
                cx.action(ScoreAction::SetDialogZoom(draft.zoom * 1.12));
            }
            if self.view.button(cx, ids!(setup_tempo_down)).clicked(actions) {
                cx.action(ScoreAction::SetDialogTempo(draft.tempo - 2.0));
            }
            if self.view.button(cx, ids!(setup_tempo_up)).clicked(actions) {
                cx.action(ScoreAction::SetDialogTempo(draft.tempo + 2.0));
            }
        }
        if self.view.button(cx, ids!(dialog_browse)).clicked(actions) {
            let target = if self.dialog_kind(scope) == DialogKind::SaveAs {
                BrowseTarget::SaveDirectory
            } else {
                BrowseTarget::Open
            };
            cx.action(ScoreAction::Browse(target));
        }
        for index in 0..RECENT_SLOTS {
            if self.view.button(cx, recent_row(index)).clicked(actions) {
                cx.action(ScoreAction::OpenRecent(index));
            }
        }
        if self.view.button(cx, ids!(dialog_confirm)).clicked(actions)
            || self.view.text_input(cx, ids!(dialog_path)).returned(actions).is_some()
            || self.view.text_input(cx, ids!(dialog_annotation)).returned(actions).is_some()
        {
            self.handle_dialog_confirm(cx, scope);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.anchor_menus(cx);
        if let Some(state) = scope.data.get_mut::<ScoreAppState>() {
            let state: &mut ScoreAppState = state;
            if self.synced_dialog != state.ui.dialog {
                self.synced_dialog = state.ui.dialog;
                // Save As offers a target; Open starts empty, because the
                // path already loaded is the one thing you do not want.
                let seed = match state.ui.dialog {
                    DialogKind::SaveAs => state.document.suggested_save_path().display().to_string(),
                    _ => String::new(),
                };
                self.view.text_input(cx, ids!(dialog_path)).set_text(cx, &seed);
                self.view.text_input(cx, ids!(dialog_annotation)).set_text(cx, "");
                let folder = state.library.dir_text();
                self.view
                    .text_input(cx, ids!(library_dir_input))
                    .set_text(cx, &folder);
            }
            let editor = state.ui.mode == ProductMode::Editor && state.ui.chrome_visible;
            self.view.view(cx, ids!(editor_top)).set_visible(cx, editor);
            self.view.view(cx, ids!(left_panel)).set_visible(cx, editor);
            // The music shelf lives in the sidebar, so it is drawn every frame
            // rather than only while a dialog is open.
            self.sync_library(cx, state);
            self.view.view(cx, ids!(right_panel)).set_visible(cx, editor);
            self.view.view(cx, ids!(editor_transport)).set_visible(cx, editor);
            self.view.view(cx, ids!(status_bar)).set_visible(cx, editor);
            self.view
                .view(cx, ids!(pianist_layer))
                .set_visible(cx, !editor && state.ui.controls_visible);
            self.view
                .view(cx, ids!(pianist_annotation))
                .set_visible(cx, self.annotation_bar_open);

            self.view.view(cx, ids!(file_menu)).set_visible(cx, self.open_menu == 1);
            self.view.view(cx, ids!(edit_menu)).set_visible(cx, self.open_menu == 2);
            self.view.view(cx, ids!(view_menu)).set_visible(cx, self.open_menu == 3);
            self.view.view(cx, ids!(play_menu)).set_visible(cx, self.open_menu == 4);
            self.view.view(cx, ids!(notation_menu)).set_visible(cx, self.open_menu == 5);
            self.view.view(cx, ids!(help_menu)).set_visible(cx, self.open_menu == 6);
            self.view
                .view(cx, ids!(dialog_layer))
                .set_visible(cx, state.ui.dialog != DialogKind::None);

            self.view.label(cx, ids!(score_name)).set_text(cx, state.document.title());
            self.view.label(cx, ids!(pianist_title_text)).set_text(cx, state.document.title());
            self.view.label(cx, ids!(status_text)).set_text(cx, &state.ui.status);
            let page = state.ui.current_page + 1;
            let page_count = state.document.page_count();
            let page_text = format!("{} / {}", page, page_count);
            self.view.label(cx, ids!(pianist_page_text)).set_text(cx, &page_text);
            self.view.label(cx, ids!(page_status)).set_text(cx, &format!("Page {page} / {page_count}"));
            self.view.label(cx, ids!(zoom_status)).set_text(cx, &format!("{}%", (state.ui.zoom * 100.0).round()));
            let play_label = transport_label(state);
            self.view.button(cx, ids!(transport_play)).set_text(cx, play_label);
            self.view.button(cx, ids!(pianist_play)).set_text(cx, play_label);
            self.view.button(cx, ids!(transport_tempo)).set_text(cx, &format!("{:.0} BPM", state.practice.tempo));
            self.view.button(cx, ids!(transport_metronome)).set_text(cx, if state.practice.metronome { "✓ Metronome" } else { "Metronome" });
            self.view.button(cx, ids!(pianist_metronome)).set_text(cx, if state.practice.metronome { "✓ Metro" } else { "Metro" });
            self.view.button(cx, ids!(transport_loop)).set_text(cx, if state.practice.loop_enabled { "✓ Loop" } else { "Loop" });
            self.view.button(cx, ids!(pianist_loop)).set_text(cx, if state.practice.loop_enabled { "✓ Loop" } else { "Loop" });
            self.view.button(cx, ids!(transport_follow)).set_text(cx, if state.practice.follow_cursor { "✓ Follow" } else { "Follow" });
            self.view.button(cx, ids!(pianist_follow)).set_text(cx, if state.practice.follow_cursor { "✓ Follow" } else { "Follow" });
            self.view.button(cx, ids!(transport_count_in)).set_text(cx, if state.practice.count_in { "✓ Count-in" } else { "Count-in" });
            for (path, layout) in [
                (ids!(layout_single), PageLayout::Single),
                (ids!(layout_two), PageLayout::TwoUp),
                (ids!(layout_continuous), PageLayout::Continuous),
            ] {
                let selected = state.ui.page_layout == layout;
                self.view.button(cx, path).set_text(
                    cx,
                    &if selected {
                        format!("✓ {}", layout.label())
                    } else {
                        layout.label().to_string()
                    },
                );
            }
            let (_, _, quarter) = state.playback_overlay();
            // The bar spans THIS piece. A fixed range put the marker at a
            // fraction of the wrong whole: bar 60 of a 60-bar prelude sat a
            // fifth of the way along, disagreeing with the page.
            let end = state.playback.end_quarter();
            let mut scrub = self.view.slider(cx, ids!(transport_scrub));
            script_apply_eval!(cx, scrub, { max: #(end) });
            self.view.slider(cx, ids!(transport_scrub)).set_value(cx, quarter);
            let bar = (quarter / 4.0).floor() as usize + 1;
            let beat = (quarter.rem_euclid(4.0)).floor() as usize + 1;
            self.view.label(cx, ids!(transport_position)).set_text(cx, &format!("{bar:03} · {beat} · {:03}", ((quarter.fract()) * 1000.0) as usize));

            // Nothing to undo must look like nothing to undo.
            let (can_undo, can_redo) = state.document.undo_redo_available();
            for path in [ids!(edit_undo), ids!(history_undo)] {
                self.view.button(cx, path).set_enabled(cx, can_undo);
            }
            for path in [ids!(edit_redo), ids!(history_redo)] {
                self.view.button(cx, path).set_enabled(cx, can_redo);
            }
            let has_selection = state.ui.selection.active.is_some();
            for path in [
                ids!(edit_select_more), ids!(edit_clear),
                ids!(notation_staccato), ids!(notation_accent), ids!(notation_tenuto),
                ids!(notation_sharp), ids!(notation_flat), ids!(notation_natural),
                ids!(palette_staccato), ids!(palette_accent), ids!(palette_tenuto),
                ids!(palette_flat), ids!(palette_natural), ids!(palette_sharp),
                ids!(p_staccato), ids!(p_accent), ids!(p_tenuto),
                ids!(p_flat), ids!(p_natural), ids!(p_sharp),
            ] {
                self.view.button(cx, path).set_enabled(cx, has_selection);
            }
            for path in [ids!(duration_4), ids!(duration_5), ids!(duration_6), ids!(duration_7)] {
                self.view.button(cx, path).set_enabled(cx, has_selection || state.ui.caret.is_some());
            }
            self.view
                .button(cx, ids!(file_save))
                .set_enabled(cx, state.document.is_dirty() || state.document.native_path().is_none());

            // The armed pointer tool is the one you cannot pick again — the
            // same convention the inspector tabs use, so the toolbar always
            // says which of the three the pointer is obeying.
            for (path, tool) in [
                (ids!(tool_navigate), ScoreTool::Navigate),
                (ids!(tool_choose), ScoreTool::Select),
                (ids!(tool_edit), ScoreTool::Edit),
            ] {
                self.view.button(cx, path).set_enabled(cx, state.ui.tool != tool);
            }
            // Transpose and delete are selection operations, and they are
            // edits: the safe tool does not offer them.
            let may_edit = state.ui.tool != ScoreTool::Navigate && has_selection;
            for path in [
                ids!(tool_transpose_up),
                ids!(tool_transpose_down),
                ids!(tool_delete),
            ] {
                self.view.button(cx, path).set_enabled(cx, may_edit);
            }
            for (path, tool) in [
                (ids!(tool_select), AnnotationTool::None),
                (ids!(annotate_highlight), AnnotationTool::Highlight),
                (ids!(p_highlight), AnnotationTool::Highlight),
                (ids!(annotate_circle), AnnotationTool::Circle),
                (ids!(p_circle), AnnotationTool::Circle),
                (ids!(annotate_text), AnnotationTool::Text),
                (ids!(p_text), AnnotationTool::Text),
                (ids!(annotate_fingering), AnnotationTool::Fingering),
                (ids!(p_fingering), AnnotationTool::Fingering),
                (ids!(annotate_ink), AnnotationTool::Ink),
                (ids!(p_ink), AnnotationTool::Ink),
            ] {
                self.view
                    .button(cx, path)
                    .set_enabled(cx, state.ui.annotation_tool != tool);
            }

            let properties = state.ui.inspector_tab == InspectorTab::Properties;
            self.view.view(cx, ids!(properties_page)).set_visible(cx, properties);
            self.view.view(cx, ids!(mixer_page)).set_visible(cx, state.ui.inspector_tab == InspectorTab::Mixer);
            let sound_tab = state.ui.inspector_tab == InspectorTab::Sound;
            self.view.view(cx, ids!(sound_page)).set_visible(cx, sound_tab);
            self.view.view(cx, ids!(history_page)).set_visible(cx, state.ui.inspector_tab == InspectorTab::History);
            for (path, tab) in [
                (ids!(inspector_properties), InspectorTab::Properties),
                (ids!(inspector_mixer), InspectorTab::Mixer),
                (ids!(inspector_sound), InspectorTab::Sound),
                (ids!(inspector_history), InspectorTab::History),
            ] {
                let selected = state.ui.inspector_tab == tab;
                self.view.button(cx, path).set_enabled(cx, !selected);
            }
            if sound_tab {
                self.sync_sound_panel(cx, state);
            }
            self.view.label(cx, ids!(selection_kind)).set_text(cx, &state.selection_description());
            if let Some(element) = state.ui.selection.active.and_then(|id| state.document.element(id)) {
                self.view.label(cx, ids!(selection_pitch)).set_text(cx, &element.midi.map_or("—".into(), |midi| format!("MIDI {midi}")));
                self.view.label(cx, ids!(selection_position)).set_text(cx, &format!("Page {} · bar {}", element.page + 1, state.document.score().measures[&element.measure].label));
            } else {
                self.view.label(cx, ids!(selection_pitch)).set_text(cx, "—");
                self.view.label(cx, ids!(selection_position)).set_text(cx, "—");
            }
            let (duration, articulation) = state
                .ui
                .selection
                .active
                .and_then(|id| state.document.selection_facts(id))
                .unwrap_or_else(|| ("—".to_string(), "—".to_string()));
            self.view.label(cx, ids!(selection_duration)).set_text(cx, &duration);
            self.view.label(cx, ids!(selection_articulation)).set_text(cx, &articulation);
            self.view
                .label(cx, ids!(score_pages))
                .set_text(cx, &format!("{}", state.document.page_count()));
            self.view
                .label(cx, ids!(score_bars))
                .set_text(cx, &format!("{}", state.document.score().measures.len()));
            self.view
                .label(cx, ids!(score_tempo))
                .set_text(cx, &format!("{:.0} BPM", state.practice.tempo));
            self.view.label(cx, ids!(sound_status)).set_text(cx, &state.midi_status());
            self.view
                .label(cx, ids!(engine_status))
                .set_text(cx, state.engine_status());

            self.view.label(cx, ids!(parts_count)).set_text(
                cx,
                &format!("{} part{}", state.parts.len(), if state.parts.len() == 1 { "" } else { "s" }),
            );
            for part in 0..4 {
                let (row, name) = part_rows(part);
                self.view.view(cx, row).set_visible(cx, part < state.parts.len());
                let ids = mix_ids(part);
                self.view.view(cx, ids.strip).set_visible(cx, part < state.parts.len());
                if let Some(channel) = state.parts.get(part) {
                    self.view.label(cx, name).set_text(cx, &channel.name);
                    self.view.label(cx, ids.name).set_text(cx, &channel.name);
                    self.view.label(cx, ids.gain).set_text(cx, &format!("{:+.1} dB", 20.0 * channel.gain.max(0.001).log10()));
                    self.view.label(cx, ids.pan).set_text(cx, if channel.pan.abs() < 0.01 { "C" } else if channel.pan < 0.0 { "L" } else { "R" });
                    self.view.button(cx, ids.mute).set_text(cx, if channel.mute { "✓ Mute" } else { "Mute" });
                    self.view.button(cx, ids.solo).set_text(cx, if channel.solo { "✓ Solo" } else { "Solo" });
                    let (mute, solo) = part_mute_solo(part);
                    self.view.button(cx, mute).set_text(cx, if channel.mute { "✓M" } else { "M" });
                    self.view.button(cx, solo).set_text(cx, if channel.solo { "✓S" } else { "S" });
                }
            }
            let room = state.sound.room;
            self.view.label(cx, ids!(room_name)).set_text(cx, &room_summary(room));
            for (path, preset) in [
                (ids!(room_practice), REVERB_PRESETS[0]),
                (ids!(room_studio), REVERB_PRESETS[1]),
                (ids!(room_small_hall), REVERB_PRESETS[2]),
                (ids!(room_concert_hall), REVERB_PRESETS[3]),
                (ids!(room_cathedral), REVERB_PRESETS[4]),
            ] {
                let label = if room.preset == preset.0 {
                    format!("✓ {}", preset.1)
                } else {
                    preset.1.to_string()
                };
                self.view.button(cx, path).set_text(cx, &label);
            }

            let history = state.history_lines();
            for index in 0..5 {
                let (row, label) = history_rows(index);
                self.view.view(cx, row).set_visible(cx, index < history.len() || index == 0);
                self.view.label(cx, label).set_text(cx, history.get(index).map(String::as_str).unwrap_or("No edits yet"));
            }
            let state_snapshot = &*state;
            self.sync_dialog(cx, state_snapshot);
        }
        self.view.draw_walk(cx, scope, walk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The DSL declares one fixed slot per row because the script has no
    /// loops. A slot that goes missing, or an id typed twice, would silently
    /// drive the wrong widget — so the tables are checked against the lists
    /// they exist to display.
    #[test]
    fn every_declared_row_matches_the_list_it_shows() {
        assert_eq!(SOUND_SLIDERS.len(), SoundParam::ALL.len());
        assert_eq!(SOUND_SLIDER_NAMES.len(), SoundParam::ALL.len());
        assert_eq!(SOUND_SLIDER_VALUES.len(), SoundParam::ALL.len());
        // Every shipped instrument must have a slot: the panel has no loop
        // to grow one, so an instrument added to the table without a row in
        // the script would simply not be listed.
        assert_eq!(INSTRUMENT_ROWS.len(), crate::sound::instrument_list().len());
        assert_eq!(INSTRUMENT_ROWS.len(), INSTRUMENT_DESCRIPTIONS.len());
        assert_eq!(LIBRARY_ROWS.len(), crate::state::LIBRARY_PAGE);
        assert_eq!(KEYMAP_SLOTS, KEYMAP_ROWS.len());

        let mut every: Vec<LiveId> = Vec::new();
        for table in [
            INSTRUMENT_ROWS,
            INSTRUMENT_DESCRIPTIONS,
            SOUND_SLIDERS,
            SOUND_SLIDER_NAMES,
            SOUND_SLIDER_VALUES,
            LIBRARY_ROWS,
        ] {
            for path in table {
                assert_eq!(path.len(), 1, "row ids are single names");
                every.push(path[0]);
            }
        }
        let unique: std::collections::BTreeSet<LiveId> = every.iter().copied().collect();
        assert_eq!(unique.len(), every.len(), "a row id is used twice");
    }

    /// The fixed slot helpers must cover their whole range, not silently fold
    /// the tail onto the last row.
    #[test]
    fn the_indexed_row_helpers_cover_every_slot() {
        let recent: std::collections::BTreeSet<LiveId> =
            (0..RECENT_SLOTS).map(|index| recent_row(index)[0]).collect();
        assert_eq!(recent.len(), RECENT_SLOTS);
        let keymap: std::collections::BTreeSet<LiveId> =
            (0..KEYMAP_SLOTS).map(|index| keymap_row(index).0[0]).collect();
        assert_eq!(keymap.len(), KEYMAP_SLOTS);
        let about: std::collections::BTreeSet<LiveId> =
            (0..ABOUT_SLOTS).map(|index| about_row(index)[0]).collect();
        assert_eq!(about.len(), ABOUT_SLOTS);
    }
}
