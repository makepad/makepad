//! `TranslatorView`: the whole app as one widget. Two panes (source and
//! target) over the AI hub's chat model, a history with favourites on the
//! side, and one layout that turns itself into a stacked one when the room
//! is narrow (phones).

use crate::engine::{Engine, HubSettings, Job, Update};
#[path = "models_ui.rs"]
mod models_ui;
use models_ui::Models;
#[path = "talk.rs"]
mod talk;
use talk::Talk;
use crate::camera;
use crate::lang::{detect, Lang};
use crate::pair::{self, Invite};
use crate::prompt::{clean, pinyin_system, translate_system, Style};
use crate::reader::{self, Frame, TextBox};
use makepad_widgets::makepad_platform::permission::{Permission, PermissionStatus};
use makepad_widgets::makepad_platform::VirtualKeyboardEvent;
use makepad_widgets::makepad_platform::video::{VideoFormatId, VideoInputId, VideoInputsEvent, VideoPixelFormat};
use crate::store::{Entry, Saved};
use makepad_widgets::makepad_platform::storage::{StorageHandle, StorageRequestId, StorageResponse, StorageResult};
use makepad_widgets::makepad_platform::thread::SignalToUI;
use makepad_widgets::*;
use std::sync::Arc;

/// Below this width the panes stack and the history becomes its own page.
const COMPACT_WIDTH: f64 = 760.0;
/// A pause in typing this long starts the translation.
const DEBOUNCE_SECS: f64 = 0.55;
/// How often the hub status is refreshed.
const PROBE_SECS: f64 = 15.0;
/// How often a hub that is down is looked for again.
const PROBE_DOWN_SECS: f64 = 3.0;
const SAVE_KEY: &str = "translator.json";

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    // Colours come from `mod.translator` (palette.rs: one set for a light
    // system, one for a dark one). Vermilion is kept for the one main action
    // on each screen (the mic, the shutter) and for progress; everything else
    // is ink on paper, and the translation sits on a deep ink card.

    // Inter for Latin (and pinyin's tone marks) at every weight we use; the
    // theme's CJK fallback (LXGW WenKai) is kept behind it for Chinese.
    let Reg = theme.font_regular{
        font_family +: {latin := FontMember{res: crate_resource("makepad_widgets:resources/Inter.ttf") weight: 400.0 asc: 0.0 desc: 0.0}}
    }
    let Semi = theme.font_bold{
        font_family +: {latin := FontMember{res: crate_resource("makepad_widgets:resources/Inter.ttf") weight: 600.0 asc: 0.0 desc: 0.0}}
    }

    let Hit = ButtonFlat{
        width: Fill height: Fill text: "" margin: 0 padding: 0
        draw_bg +: {color: #0000 color_hover: #0000 color_down: #0000 color_focus: #0000 border_size: 0.0}
        draw_text +: {color: #0000}
    }

    // Every tappable thing is at least 44 pt; the face only shows when pressed.
    let IconBtn = ButtonFlat{
        width: 44 height: 44 margin: 0 padding: 0 text: ""
        icon_walk: Walk{width: 21 height: 21}
        draw_icon +: {color: mod.translator.ink_dim}
        draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000 color_down: mod.translator.surface_2 border_size: 0.0 border_radius: 22.0}
    }
    let IconBtnOnInk = IconBtn{
        draw_icon +: {color: mod.translator.on_ink_dim}
        draw_bg +: {color_down: #ffffff1a}
    }

    // A quiet text button (links, secondary actions).
    let TextBtn = ButtonFlat{
        width: Fit height: 44 margin: 0 padding: Inset{left: 12 right: 12}
        draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000 color_down: mod.translator.surface_2 border_size: 0.0 border_radius: 22.0}
        draw_text +: {color: mod.translator.ink_dim color_hover: mod.translator.ink_dim color_focus: mod.translator.ink_dim color_down: mod.translator.ink text_style: Reg{font_size: 14}}
    }

    // A filled secondary button: a soft pill.
    let SoftBtn = ButtonFlat{
        width: Fit height: 40 margin: 0 padding: Inset{left: 16 right: 16}
        draw_bg +: {color: mod.translator.surface_2 color_hover: mod.translator.surface_2 color_focus: mod.translator.surface_2 color_down: mod.translator.line border_size: 0.0 border_radius: 20.0}
        draw_text +: {color: mod.translator.ink color_hover: mod.translator.ink color_focus: mod.translator.ink color_down: mod.translator.ink text_style: Semi{font_size: 13}}
    }

    // The primary button: vermilion.
    let PrimaryBtn = ButtonFlat{
        width: Fit height: 44 margin: 0 padding: Inset{left: 18 right: 18}
        draw_bg +: {color: mod.translator.vermilion color_hover: mod.translator.vermilion color_focus: mod.translator.vermilion color_down: mod.translator.vermilion_down border_size: 0.0 border_radius: 22.0}
        draw_text +: {color: mod.translator.on_vermilion color_hover: mod.translator.on_vermilion color_focus: mod.translator.on_vermilion color_down: mod.translator.on_vermilion text_style: Semi{font_size: 14}}
    }

    // One segment of a two-way switch; the Rust side lights the chosen one.
    let Segment = ButtonFlat{
        width: Fit height: 38 margin: 0 padding: Inset{left: 16 right: 16}
        draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000 color_down: mod.translator.line border_size: 0.0 border_radius: 19.0}
        draw_text +: {color: mod.translator.ink color_hover: mod.translator.ink color_focus: mod.translator.ink color_down: mod.translator.ink text_style: Semi{font_size: 13}}
    }
    let SegmentTrack = RoundedView{
        width: Fit height: Fit flow: Right padding: 3 spacing: 2
        draw_bg +: {color: mod.translator.surface_2 border_radius: 22 border_size: 0.0}
    }

    // An on/off switch: a grey track off, vermilion on, a white knob.
    let Switch = Toggle{
        draw_text +: {text_style: Reg{font_size: 14}}
        draw_bg +: {
            color: mod.translator.ink_faint color_hover: mod.translator.ink_faint color_focus: mod.translator.ink_faint color_down: mod.translator.ink_dim
            color_active: mod.translator.vermilion
            border_size: 0.0
            mark_color: #ffffff mark_color_hover: #ffffff mark_color_down: #ffffff mark_color_active: #ffffff mark_color_active_hover: #ffffff
        }
    }

    let Caption = Label{
        padding: 0 width: Fill height: Fit flow: Right{wrap: true}
        draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 12 line_spacing: 1.3}}
    }
    let Hairline = View{width: Fill height: 1 show_bg: true draw_bg +: {color: mod.translator.line}}

    // A quiet text field: no frame, the card is the surface.
    let Field = TextInputFlat{
        margin: 0 padding: 0
        draw_bg +: {
            color: #0000 color_hover: #0000 color_focus: #0000 color_down: #0000 color_empty: #0000 color_disabled: #0000
            border_size: 0.0 border_radius: 0.0
        }
        draw_text +: {
            color: mod.translator.ink color_hover: mod.translator.ink color_focus: mod.translator.ink color_down: mod.translator.ink color_disabled: mod.translator.ink
            color_empty: mod.translator.ink_faint color_empty_hover: mod.translator.ink_faint color_empty_focus: mod.translator.ink_faint
        }
        // A vermilion caret and a soft vermilion selection.
        draw_cursor +: {color: mod.translator.vermilion border_radius: 1.0}
        draw_selection +: {
            color: mod.translator.vermilion_soft color_hover: mod.translator.vermilion_soft color_focus: mod.translator.vermilion_soft
            color_down: mod.translator.vermilion_soft color_empty: mod.translator.vermilion_soft
        }
    }
    // A field that stands on its own (settings): a soft filled box.
    let BoxField = Field{
        padding: Inset{left: 14 right: 14 top: 12 bottom: 12}
        draw_bg +: {
            color: mod.translator.surface_2 color_hover: mod.translator.surface_2 color_focus: mod.translator.surface_2 color_down: mod.translator.surface_2 color_empty: mod.translator.surface_2
            border_radius: 12.0
        }
        draw_text +: {text_style: Reg{font_size: 14}}
    }
    let FieldOnInk = Field{
        draw_text +: {
            color: mod.translator.on_ink color_hover: mod.translator.on_ink color_focus: mod.translator.on_ink color_down: mod.translator.on_ink color_disabled: mod.translator.on_ink
            color_empty: mod.translator.on_ink_dim color_empty_hover: mod.translator.on_ink_dim color_empty_focus: mod.translator.on_ink_dim
        }
        draw_cursor +: {color: mod.translator.accent_on_ink}
        draw_selection +: {color: #ffffff2e color_hover: #ffffff2e color_focus: #ffffff2e color_down: #ffffff2e color_empty: #ffffff2e}
    }

    // A language in the language bar.
    let LangBtn = ButtonFlat{
        width: Fill height: 48 margin: 0 padding: Inset{left: 12 right: 12}
        align: Align{x: 0.5 y: 0.5}
        draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000 color_down: mod.translator.surface_2 border_size: 0.0 border_radius: 24.0}
        draw_text +: {color: mod.translator.ink color_hover: mod.translator.ink color_focus: mod.translator.ink color_down: mod.translator.ink text_style: Semi{font_size: 16}}
    }

    let ExampleChip = ButtonFlat{
        width: Fill height: 44 margin: 0 padding: Inset{left: 14 right: 14}
        align: Align{x: 0.0 y: 0.5}
        draw_bg +: {color: mod.translator.bg color_hover: mod.translator.bg color_focus: mod.translator.bg color_down: mod.translator.surface_2 border_size: 0.0 border_radius: 14.0}
        draw_text +: {color: mod.translator.ink_dim color_hover: mod.translator.ink color_focus: mod.translator.ink_dim color_down: mod.translator.ink text_style: Reg{font_size: 14}}
    }

    // A slot in the thumb bar: an icon over a short word.
    let NavItem = ButtonFlat{
        width: Fill height: 60 margin: 0 padding: Inset{top: 6 bottom: 4} flow: Down spacing: 5
        align: Align{x: 0.5 y: 0.5}
        icon_walk: Walk{width: 24 height: 24}
        draw_icon +: {color: mod.translator.ink}
        draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000 color_down: mod.translator.surface_2 border_size: 0.0 border_radius: 18.0}
        draw_text +: {color: mod.translator.ink_dim color_hover: mod.translator.ink_dim color_focus: mod.translator.ink_dim color_down: mod.translator.ink text_style: Reg{font_size: 11}}
    }

    // Hold to speak: a pill with a mic and the language it expects. Gray while
    // idle; bright red while the microphone is really capturing (sync_mics).
    let MicPill = ButtonFlat{
        width: Fit height: 38 margin: 0 padding: Inset{left: 12 right: 14} spacing: 6 text: ""
        icon_walk: Walk{width: 17 height: 17}
        draw_icon +: {svg: crate_resource("self:resources/icons/mic.svg") color: mod.translator.ink_dim}
        draw_bg +: {color: mod.translator.surface_2 color_hover: mod.translator.surface_2 color_focus: mod.translator.surface_2 color_down: mod.translator.line border_size: 0.0 border_radius: 19.0}
        draw_text +: {color: mod.translator.ink_dim color_hover: mod.translator.ink_dim color_focus: mod.translator.ink_dim color_down: mod.translator.ink text_style: Semi{font_size: 13}}
    }

    // A main action: a big round button over a word.
    let BigRound = ButtonFlat{
        width: 54 height: 54 margin: 0 padding: 0 text: ""
        icon_walk: Walk{width: 24 height: 24}
        draw_icon +: {color: mod.translator.ink}
        draw_bg +: {color: mod.translator.surface_2 color_hover: mod.translator.surface_2 color_focus: mod.translator.surface_2 color_down: mod.translator.line border_size: 0.0 border_radius: 27.0}
    }
    let BigLabel = Label{padding: 0 draw_text +: {color: mod.translator.ink text_style: Reg{font_size: 12}}}
    let Action = View{width: 96 height: Fit flow: Down align: Align{x: 0.5} spacing: 3}

    let HistoryRow = View{
        width: Fill height: Fit flow: Down
        View{
            width: Fill height: 76 flow: Right align: Align{y: 0.5}
            open := View{
                width: Fill height: Fill flow: Overlay
                hit := Hit{}
                texts := View{
                    width: Fill height: Fill flow: Down padding: Inset{left: 10 right: 4 top: 12 bottom: 10} spacing: 4
                    src := Label{width: Fill padding: 0 max_lines: 1 text_overflow: Ellipsis draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 13}}}
                    dst := Label{flow: Right{wrap: true} width: Fill padding: 0 max_lines: 2 text_overflow: Ellipsis draw_text +: {color: mod.translator.ink text_style: Reg{font_size: 16 line_spacing: 1.2}}}
                }
            }
            fav := IconBtn{draw_icon +: {svg: crate_resource("self:resources/icons/star.svg") color: mod.translator.ink_faint}}
        }
        Hairline{margin: Inset{left: 10 right: 10}}
    }

    // One model in settings: what it is, how far along, and the one action.
    let ModelRow = View{
        width: Fill height: Fit flow: Down
        Hairline{}
        View{
            width: Fill height: Fit flow: Down spacing: 8 padding: Inset{top: 10 bottom: 10}
            mtop := View{
                width: Fill height: Fit flow: Right spacing: 12 align: Align{y: 0.5}
                View{
                    width: Fill height: Fit flow: Down spacing: 3
                    mname := Label{width: Fill padding: 0 draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 15}}}
                    mstate := Label{width: Fill height: Fit padding: 0 max_lines: 1 text_overflow: Ellipsis draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 12}}}
                }
                mbtn := SoftBtn{text: ""}
            }
            mtrack := RoundedView{
                visible: false width: Fill height: 4
                draw_bg +: {color: mod.translator.surface_2 border_radius: 2.0 border_size: 0.0}
                mbar := RoundedView{width: 0 height: Fill draw_bg +: {color: mod.translator.vermilion border_radius: 2.0 border_size: 0.0}}
            }
        }
    }

    let Progress = RoundedView{
        visible: false width: Fill height: 4 flow: Right
        draw_bg +: {color: mod.translator.line border_radius: 2.0 border_size: 0.0}
        fill := RoundedView{width: 0 height: Fill draw_bg +: {color: mod.translator.vermilion border_radius: 2.0 border_size: 0.0}}
    }

    // One live line of what the models are doing ("Translating line 3 of 8 · 4.1 s").
    let ActLine = Label{
        width: Fill height: Fit padding: 0 max_lines: 2 text_overflow: Ellipsis flow: Right{wrap: true}
        draw_text +: {text_style: Reg{font_size: 12 line_spacing: 1.25}}
    }

    // A mode button on the landscape camera rail.
    let RailBtn = ButtonFlat{
        width: 48 height: 48 margin: 0 padding: 0 text: ""
        icon_walk: Walk{width: 22 height: 22}
        draw_icon +: {color: #f6f1e8}
        draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000 color_down: #ffffff33 border_size: 0.0 border_radius: 24.0}
    }

    // One line of a photo's result: the translation, and the original under it.
    let LineRow = RoundedView{
        visible: false width: Fill height: Fit flow: Down spacing: 2 padding: Inset{left: 16 right: 16 top: 10 bottom: 10}
        draw_bg +: {color: mod.translator.surface border_radius: 14 border_size: 0.0}
        trans := Label{flow: Right{wrap: true} width: Fill height: Fit padding: 0 draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 16 line_spacing: 1.3}}}
        orig := Label{flow: Right{wrap: true} width: Fill height: Fit padding: 0 draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 13 line_spacing: 1.3}}}
    }

    // A translated line laid over the camera picture.
    let LineTag = RoundedView{
        visible: false width: 160 height: Fit flow: Down padding: Inset{left: 10 right: 10 top: 5 bottom: 6}
        draw_bg +: {color: #1f2b46ee border_radius: 8 border_size: 0.0}
        ov_text := Label{flow: Right{wrap: true} width: Fill height: Fit padding: 0 draw_text +: {color: #f6f1e8 text_style: Semi{font_size: 14 line_spacing: 1.15}}}
    }

    let ScreenBar = View{
        width: Fill height: 48 flow: Right spacing: 4 align: Align{y: 0.5} margin: Inset{top: mod.widgets.SAFE_INSET_PAD_TOP}
    }
    let ScreenTitle = Label{padding: 0 draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 20}}}

    mod.widgets.TranslatorViewBase = #(TranslatorView::register_widget(vm))
    mod.widgets.TranslatorView = set_type_default() do mod.widgets.TranslatorViewBase{
        width: Fill height: Fill flow: Down
        show_bg: true
        draw_bg +: {color: mod.translator.bg}
        padding: Inset{left: 12 right: 12 top: 0 bottom: mod.widgets.SAFE_INSET_PAD_BOTTOM} spacing: 8

        // The top row: the direction (two languages and the one flip between them)
        // with the settings corner; on the settings page its title instead.
        bar := View{
            width: Fill height: 48 flow: Right spacing: 4 align: Align{y: 0.5} margin: Inset{top: mod.widgets.SAFE_INSET_PAD_TOP}
            title := ScreenTitle{visible: false width: Fill text: "Settings" margin: Inset{left: 4}}
            langs := View{
                width: Fill height: Fill flow: Right spacing: 14 align: Align{y: 0.5}
                // Balances the gear on the right, so the pair sits centred.
                View{width: 44 height: 1}
                View{width: Fill height: Fit align: Align{x: 1.0 y: 0.5}
                    from_lang := Label{padding: 0 draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 18}}}
                }
                swap := ButtonFlat{
                    width: 46 height: 46 margin: 0 padding: 0 text: ""
                    icon_walk: Walk{width: 20 height: 20}
                    draw_icon +: {svg: crate_resource("self:resources/icons/swap.svg") color: mod.translator.ink}
                    draw_bg +: {color: mod.translator.surface color_hover: mod.translator.surface color_focus: mod.translator.surface color_down: mod.translator.surface_2 border_size: 0.0 border_radius: 23.0}
                }
                View{width: Fill height: Fit align: Align{x: 0.0 y: 0.5}
                    to_lang := Label{padding: 0 draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 18}}}
                }
            }
            settings_btn := IconBtn{icon_walk: Walk{width: 18 height: 18} draw_icon +: {svg: crate_resource("self:resources/icons/settings.svg") color: mod.translator.ink_faint}}
        }
        route := Label{flow: Right{wrap: true} visible: false width: Fill height: Fit padding: Inset{left: 4} max_lines: 2 text_overflow: Ellipsis draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 12}}}

        offer := RoundedView{
            visible: false
            width: Fill height: Fit flow: Down padding: Inset{left: 18 right: 12 top: 16 bottom: 12} spacing: 12
            draw_bg +: {color: mod.translator.surface border_radius: 20 border_size: 0.0}
            offer_text := Label{flow: Right{wrap: true} width: Fill height: Fit padding: Inset{right: 6} draw_text +: {color: mod.translator.ink text_style: Reg{font_size: 15 line_spacing: 1.3}}}
            offer_row := View{
                width: Fill height: Fit flow: Right spacing: 4 align: Align{y: 0.5}
                offer_dl := PrimaryBtn{text: "Download on Wi-Fi"}
                View{width: Fill}
                offer_hub := TextBtn{text: "My hub"}
                offer_later := TextBtn{text: "Later"}
            }
        }

        // Settings: the model downloads, one line to say where the models run,
        // and the hub pairing behind one small link. Scrolls only when the hub
        // part is open.
        settings := ScrollYView{
            visible: false
            width: Fill height: Fill flow: Down spacing: 12
            models_card := RoundedView{
                visible: false
                width: Fill height: Fit flow: Down padding: Inset{left: 18 right: 14 top: 6 bottom: 6}
                draw_bg +: {color: mod.translator.surface border_radius: 20 border_size: 0.0}
                View{
                    width: Fill height: 48 flow: Right align: Align{y: 0.5}
                    Label{padding: 0 text: "Offline models" draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 16}}}
                    View{width: Fill}
                    models_remove := TextBtn{height: 36 padding: Inset{left: 10 right: 10} text: "Remove all" draw_text +: {text_style: Reg{font_size: 13}}}
                }
                m0 := ModelRow{}
                m1 := ModelRow{}
                m2 := ModelRow{}
                m3 := ModelRow{}
            }
            // Where the models run, Hub or On-device, each side with its state:
            // switching is instant, and the hub is found again on its own.
            run_on := RoundedView{
                width: Fill height: Fit flow: Down padding: Inset{left: 18 right: 8 top: 8 bottom: 12} spacing: 6
                draw_bg +: {color: mod.translator.surface border_radius: 20 border_size: 0.0}
                View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5}
                    Label{padding: 0 text: "Runs on" draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 15}}}
                    View{width: Fill}
                    SegmentTrack{
                        run_hub := Segment{text: "Hub"}
                        run_phone := Segment{text: "On-device"}
                    }
                }
                hub_state := Label{flow: Right{wrap: true} width: Fill height: Fit padding: Inset{right: 10} draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 12}}}
                phone_state := Label{flow: Right{wrap: true} width: Fill height: Fit padding: Inset{right: 10} draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 12}}}
            }
            options := RoundedView{
                width: Fill height: Fit flow: Right spacing: 20 align: Align{y: 0.5} padding: Inset{left: 18 right: 18 top: 14 bottom: 14}
                draw_bg +: {color: mod.translator.surface border_radius: 20 border_size: 0.0}
                Label{padding: 0 text: "Chinese" draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 15}}}
                View{width: Fill}
                pinyin_on := Switch{text: "Pinyin"}
                traditional_on := Switch{text: "Traditional"}
            }
            View{
                width: Fill height: Fit flow: Right spacing: 4
                open_history := TextBtn{text: "History"}
                View{width: Fill}
                hub_link := TextBtn{text: "Hub…"}
            }
            hub_section := RoundedView{
                visible: false
                width: Fill height: Fit flow: Down padding: 18 spacing: 10
                draw_bg +: {color: mod.translator.surface border_radius: 20 border_size: 0.0}
                Caption{text: "Hub address (empty: find it on the network)"}
                hub := BoxField{width: Fill height: Fit empty_text: "10.0.0.149:8123"}
                Caption{text: "Model on the hub"}
                model := BoxField{width: Fill height: Fit empty_text: "qwen3.8-27b"}
                Caption{text: "Pair a phone: on the hub's machine press Invite a phone, then paste the invitation here and press Connect."}
                pair_row := View{
                    width: Fill height: Fit flow: Right spacing: 8
                    invite_btn := SoftBtn{text: "Invite a phone"}
                    connect_btn := SoftBtn{text: "Connect"}
                    copy_invite := SoftBtn{text: "Copy"}
                }
                invite := BoxField{width: Fill height: 76 is_multiline: true submit_on_enter: false empty_text: "makepad-hub://10.0.0.149:8123?cred=..."}
                qr := Image{visible: false width: 220 height: 220 fit: ImageFit.Stretch}
                pair_status := Caption{}
                Hairline{margin: Inset{top: 6 bottom: 6}}
                Caption{text: "Voices (tap to hear a sample)"}
                voice_en_row := View{
                    width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                    prev := SoftBtn{width: 44 padding: 0 text: "‹"}
                    name := SoftBtn{width: Fill text: "English: af_heart"}
                    next := SoftBtn{width: 44 padding: 0 text: "›"}
                }
                voice_zh_row := View{
                    width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5}
                    prev := SoftBtn{width: 44 padding: 0 text: "‹"}
                    name := SoftBtn{width: Fill text: "Chinese: zf_001"}
                    next := SoftBtn{width: 44 padding: 0 text: "›"}
                }
                Caption{text: "Glossary, one per line: term = translation"}
                glossary := BoxField{width: Fill height: 88 is_multiline: true submit_on_enter: false empty_text: "Makepad = Makepad"}
            }
        }

        main := View{
            width: Fill height: Fill flow: Right spacing: 8
            panes := View{
                width: Fill height: Fill flow: Right spacing: 8
                source_card := RoundedView{
                    width: Fill height: Fill flow: Down padding: Inset{left: 16 right: 6 top: 2 bottom: 10} spacing: 2
                    draw_bg +: {color: mod.translator.surface border_radius: 20 border_size: 0.0}
                    head := View{
                        width: Fill height: 44 flow: Right align: Align{y: 0.5}
                        // Hold to speak this side's language (detected from the speech).
                        dictate := MicPill{}
                        View{width: Fill}
                        clear := IconBtn{icon_walk: Walk{width: 16 height: 16} draw_icon +: {svg: crate_resource("self:resources/icons/close.svg") color: mod.translator.ink_faint}}
                        copy_src := IconBtn{icon_walk: Walk{width: 19 height: 19} draw_icon +: {svg: crate_resource("self:resources/icons/copy.svg") color: mod.translator.ink_faint}}
                    }
                    input := Field{
                        width: Fill height: Fill is_multiline: true submit_on_enter: false empty_text: ""
                        padding: Inset{right: 10}
                        draw_text +: {text_style: Reg{font_size: 21 line_spacing: 1.3}}
                    }
                }
                target_card := RoundedView{
                    width: Fill height: Fill flow: Down padding: Inset{left: 16 right: 6 top: 2 bottom: 10} spacing: 4
                    draw_bg +: {color: mod.translator.ink_card border_radius: 20 border_size: 0.0}
                    head := View{
                        width: Fill height: 44 flow: Right align: Align{y: 0.5}
                        dictate_other := MicPill{}
                        View{width: Fill}
                        copy_dst := IconBtnOnInk{icon_walk: Walk{width: 19 height: 19} draw_icon +: {svg: crate_resource("self:resources/icons/copy.svg")}}
                    }
                    output := FieldOnInk{
                        width: Fill height: Fill is_multiline: true is_read_only: true empty_text: ""
                        padding: Inset{right: 10}
                        draw_text +: {text_style: Reg{font_size: 24 line_spacing: 1.3}}
                    }
                    pinyin_text := Label{flow: Right{wrap: true} width: Fill height: Fit padding: Inset{right: 10} draw_text +: {color: mod.translator.accent_on_ink text_style: Reg{font_size: 15 line_spacing: 1.35}}}
                    status := Label{flow: Right{wrap: true} width: Fill height: Fit padding: 0 draw_text +: {color: mod.translator.on_ink_dim text_style: Reg{font_size: 12}}}
                    // What the models are doing for this card, in words.
                    card_act := ActLine{visible: false draw_text +: {color: mod.translator.on_ink_dim}}
                    // Speak the translation: a big play button that turns into stop,
                    // with a line beside it that shows the voice being made, then played.
                    play_row := View{
                        width: Fill height: Fit flow: Right spacing: 14 align: Align{y: 0.5} padding: Inset{right: 4}
                        // The card's progress: translating, then the voice being made and played.
                        play_track := RoundedView{
                            visible: false width: Fill height: 4 flow: Right
                            draw_bg +: {color: #ffffff1f border_radius: 2.0 border_size: 0.0}
                            play_bar := RoundedView{width: 0 height: Fill draw_bg +: {color: mod.translator.accent_on_ink border_radius: 2.0 border_size: 0.0}}
                        }
                        View{width: Fill height: 1}
                        play := BigRound{
                            width: 60 height: 60 icon_walk: Walk{width: 24 height: 24}
                            draw_icon +: {svg: crate_resource("self:resources/icons/play.svg") color: mod.translator.on_vermilion}
                            draw_bg +: {color: mod.translator.vermilion color_hover: mod.translator.vermilion color_focus: mod.translator.vermilion color_down: mod.translator.vermilion_down border_radius: 30.0}
                        }
                        stop_play := BigRound{
                            visible: false width: 60 height: 60 icon_walk: Walk{width: 22 height: 22}
                            draw_icon +: {svg: crate_resource("self:resources/icons/stop.svg") color: mod.translator.ink_card}
                            draw_bg +: {color: mod.translator.on_ink color_hover: mod.translator.on_ink color_focus: mod.translator.on_ink color_down: mod.translator.on_ink_dim border_radius: 30.0}
                        }
                    }
                }
            }
            history_panel := RoundedView{
                width: 330 height: Fill flow: Down padding: Inset{left: 8 right: 8 top: 6 bottom: 8} spacing: 2
                draw_bg +: {color: mod.translator.surface border_radius: 24 border_size: 0.0}
                history_head := View{
                    width: Fill height: 52 flow: Right align: Align{y: 0.5} padding: Inset{left: 10}
                    history_close := IconBtn{visible: false draw_icon +: {svg: crate_resource("self:resources/icons/close.svg") color: mod.translator.ink}}
                    Label{padding: 0 text: "History" draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 17}}}
                    View{width: Fill}
                    only_fav := IconBtn{draw_icon +: {svg: crate_resource("self:resources/icons/star.svg")}}
                    clear_history := IconBtn{draw_icon +: {svg: crate_resource("self:resources/icons/trash.svg")}}
                }
                rows := PortalList{
                    width: Fill height: Fill
                    Row := HistoryRow{}
                    Empty := Label{
                        width: Fill height: 120 align: Align{x: 0.5 y: 0.5}
                        draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 14}}
                    }
                }
            }
        }

        // Listen: a presentation heard continuously, its translation in large
        // lines (the newest biggest), nothing to type.
        talk := View{
            visible: false
            width: Fill height: Fill flow: Down spacing: 10
            talk_bar := ScreenBar{
                listen_dot := RoundedView{width: 10 height: 10 margin: Inset{left: 8 right: 10} draw_bg +: {color: mod.translator.vermilion border_radius: 5.0 border_size: 0.0}}
                listen_title := Label{padding: 0 text: "Listening" draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 17}}}
                View{width: Fill}
                talk_copy := IconBtn{draw_icon +: {svg: crate_resource("self:resources/icons/copy.svg")}}
                talk_clear := IconBtn{draw_icon +: {svg: crate_resource("self:resources/icons/trash.svg")}}
            }
            talk_status := Label{flow: Right{wrap: true} visible: false max_lines: 2 text_overflow: Ellipsis width: Fill height: Fit padding: Inset{left: 4 right: 4} draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 12 line_spacing: 1.3}}}
            // Live: only the English, large, the newest line in focus at the
            // bottom, older lines dimmed above it.
            // A plain view (not a scroller): the lines wrap to the screen's width
            // and sit on its foot, the oldest running off the top.
            live_scroll := View{
                width: Fill height: Fill flow: Down spacing: 18 padding: Inset{left: 4 right: 4 top: 8 bottom: 24}
                // Pushes the lines onto the foot of the screen.
                View{width: Fill height: Fill}
                live_old := Label{flow: Right{wrap: true} width: Fill height: Fit padding: 0 draw_text +: {color: mod.translator.ink_faint text_style: Reg{font_size: 22 line_spacing: 1.35}}}
                live_new := Label{flow: Right{wrap: true} width: Fill height: Fit padding: 0 draw_text +: {color: mod.translator.ink text_style: Semi{font_size: 30 line_spacing: 1.3}}}
            }
            voice_act := ActLine{visible: false padding: Inset{left: 40 right: 40} draw_text +: {color: mod.translator.ink_dim}}
            voice_progress := Progress{margin: Inset{left: 40 right: 40}}
            talk_ctrl := View{
                width: Fill height: 116 flow: Right align: Align{x: 0.5 y: 0.5}
                View{width: Fill height: Fit}
                mic_col := View{
                    width: Fit height: Fit flow: Down align: Align{x: 0.5} spacing: 8
                    talk_mic := ButtonFlat{
                        width: 84 height: 84 margin: 0 padding: 0 text: ""
                        icon_walk: Walk{width: 34 height: 34}
                        draw_icon +: {svg: crate_resource("self:resources/icons/mic.svg") color: mod.translator.ink_dim}
                        draw_bg +: {color: mod.translator.surface_2 color_hover: mod.translator.surface_2 color_focus: mod.translator.surface_2 color_down: mod.translator.line border_size: 0.0 border_radius: 42.0}
                    }
                    mic_hint := Label{padding: 0 draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 12}}}
                }
                View{
                    width: Fill height: Fit align: Align{x: 0.5 y: 0.5}
                    level_box := RoundedView{
                        width: 88 height: 6 align: Align{y: 0.5}
                        draw_bg +: {color: mod.translator.surface_2 border_radius: 3.0 border_size: 0.0}
                        level := RoundedView{width: 4 height: 6 draw_bg +: {color: mod.translator.vermilion border_radius: 3.0 border_size: 0.0}}
                    }
                }
            }
        }

        // The camera screen: a viewfinder, a shutter, the translation laid over
        // the picture at the text's positions, and a list of the same lines.
        camera := View{
            visible: false
            width: Fill height: Fill flow: Down spacing: 6
            margin: Inset{top: mod.widgets.SAFE_INSET_PAD_TOP}
            cam_bar := View{
                width: Fill height: Fit flow: Right align: Align{x: 1.0 y: 0.5}
                // The camera picker is for desktops (several webcams); a phone uses its back camera.
                cam_device := SoftBtn{visible: false height: 36 padding: Inset{left: 14 right: 14} text: "" draw_text +: {text_style: Reg{font_size: 12}}}
            }
            cam_status := Label{flow: Right{wrap: true} visible: false width: Fill height: Fit padding: Inset{left: 4 right: 4} max_lines: 2 text_overflow: Ellipsis draw_text +: {color: mod.translator.ink_dim text_style: Reg{font_size: 13 line_spacing: 1.3}}}
            frame := RoundedView{
                width: Fill height: Fill flow: Overlay
                draw_bg +: {color: #0000 border_radius: 20 border_size: 0.0}
                // Sized and placed by fit_picture.
                cam_image := Image{width: 0 height: 0 fit: ImageFit.Stretch draw_bg +: {border_radius: 20.0}}
                no_picture := Label{flow: Right{wrap: true} width: Fill height: Fit padding: 24 draw_text +: {color: #a9b3c9 text_style: Reg{font_size: 15}}}
                ov0 := LineTag{}
                ov1 := LineTag{}
                ov2 := LineTag{}
                ov3 := LineTag{}
                ov4 := LineTag{}
                ov5 := LineTag{}
                ov6 := LineTag{}
                ov7 := LineTag{}
                ov8 := LineTag{}
                ov9 := LineTag{}
                // The result, as soon as it is ready: each translated line with its
                // original under it, right below the photo (no button to reveal it).
                cam_list := ScrollYView{
                    visible: false
                    // Laid over the photo's lower part (placed by fit_picture): the photo
                    // keeps its place and size when the result arrives.
                    width: Fill height: 210 flow: Down spacing: 4 padding: 8
                    l0 := LineRow{}
                    l1 := LineRow{}
                    l2 := LineRow{}
                    l3 := LineRow{}
                    l4 := LineRow{}
                    l5 := LineRow{}
                    l6 := LineRow{}
                    l7 := LineRow{}
                    l8 := LineRow{}
                    l9 := LineRow{}
                    l10 := LineRow{}
                    l11 := LineRow{}
                }
            }
            // While the shot is read and translated: what the models are doing,
            // in words, over a line at the photo's foot.
            cam_activity := RoundedView{
                visible: false
                width: Fill height: Fit flow: Down spacing: 5 padding: Inset{left: 14 right: 14 top: 7 bottom: 9} margin: Inset{left: 18 right: 18}
                draw_bg +: {color: #10141fcc border_radius: 12 border_size: 0.0}
                cam_act := ActLine{draw_text +: {color: #f6f1e8}}
                cam_progress := RoundedView{
                    width: Fill height: 4 flow: Right
                    draw_bg +: {color: #ffffff33 border_radius: 2.0 border_size: 0.0}
                    fill := RoundedView{width: 0 height: Fill draw_bg +: {color: mod.translator.vermilion border_radius: 2.0 border_size: 0.0}}
                }
            }
            // Live: the shutter alone. After a shot: Retake (at any time, even while
            // the model is still reading) and, once there is a result, the list.
            shutter_row := View{
                width: Fill height: 72 flow: Right align: Align{x: 0.5 y: 0.5}
                View{width: Fill height: Fit}
                shutter := ButtonFlat{
                    width: 66 height: 66 margin: 0 padding: 0 text: ""
                    icon_walk: Walk{width: 26 height: 26}
                    draw_icon +: {svg: crate_resource("self:resources/icons/camera.svg") color: mod.translator.on_vermilion}
                    draw_bg +: {
                        color: mod.translator.vermilion color_hover: mod.translator.vermilion color_focus: mod.translator.vermilion color_down: mod.translator.vermilion_down
                        border_size: 3.0 border_radius: 33.0
                        border_color: mod.translator.surface border_color_hover: mod.translator.surface border_color_down: mod.translator.surface border_color_focus: mod.translator.surface
                    }
                }
                retake := ButtonFlat{
                    visible: false width: Fit height: 48 margin: 0 padding: Inset{left: 24 right: 24} text: "Retake"
                    icon_walk: Walk{width: 20 height: 20}
                    draw_icon +: {svg: crate_resource("self:resources/icons/camera.svg") color: mod.translator.on_ink}
                    draw_bg +: {color: mod.translator.ink_card color_hover: mod.translator.ink_card color_focus: mod.translator.ink_card color_down: mod.translator.ink border_size: 0.0 border_radius: 24.0}
                    draw_text +: {color: mod.translator.on_ink color_hover: mod.translator.on_ink color_focus: mod.translator.on_ink color_down: mod.translator.on_ink text_style: Semi{font_size: 16}}
                }
                View{width: Fill height: Fit}
            }
            // Landscape: the picture fills the screen and these float over it --
            // the language pair on top, the other modes down the left edge (the
            // shutter takes the right edge, see apply_camera_layout).
            cam_overlay := View{
                visible: false
                width: Fill height: Fill flow: Overlay
                View{
                    width: Fill height: Fit align: Align{x: 0.5} padding: Inset{top: 10}
                    cam_langs := RoundedView{
                        width: Fit height: Fit flow: Right spacing: 10 align: Align{y: 0.5} padding: Inset{left: 16 right: 6 top: 4 bottom: 4}
                        draw_bg +: {color: #10141fb8 border_radius: 22 border_size: 0.0}
                        cam_from := Label{padding: 0 draw_text +: {color: #f6f1e8 text_style: Semi{font_size: 15}}}
                        cam_flip := ButtonFlat{
                            width: 40 height: 40 margin: 0 padding: 0 text: ""
                            icon_walk: Walk{width: 18 height: 18}
                            draw_icon +: {svg: crate_resource("self:resources/icons/swap.svg") color: #f6f1e8}
                            draw_bg +: {color: #ffffff22 color_hover: #ffffff22 color_focus: #ffffff22 color_down: #ffffff44 border_size: 0.0 border_radius: 20.0}
                        }
                        cam_to := Label{padding: Inset{right: 10} draw_text +: {color: #f6f1e8 text_style: Semi{font_size: 15}}}
                    }
                }
                View{
                    width: Fit height: Fill align: Align{y: 0.5} padding: Inset{left: 10}
                    cam_rail := RoundedView{
                        width: Fit height: Fit flow: Down spacing: 14 align: Align{x: 0.5} padding: Inset{left: 8 right: 8 top: 12 bottom: 12}
                        draw_bg +: {color: #10141fb8 border_radius: 28 border_size: 0.0}
                        cam_tab_text := RailBtn{draw_icon +: {svg: crate_resource("self:resources/icons/keyboard.svg")}}
                        cam_tab_cam := RailBtn{draw_icon +: {svg: crate_resource("self:resources/icons/camera.svg")} draw_bg +: {color: #f6f1e8 color_hover: #f6f1e8 color_focus: #f6f1e8} draw_icon +: {color: #1f2b46}}
                        cam_tab_listen := RailBtn{draw_icon +: {svg: crate_resource("self:resources/icons/ear.svg")}}
                    }
                }
            }
        }

        // The mode switch, always at the thumb: Translate (the two boxes), Voice
        // (conversation and live translation) and Camera. The chosen one is lit.
        // It stays above the keyboard; the boxes between give up the room.
        nav := View{
            visible: false
            width: Fill height: Fit flow: Right align: Align{x: 0.5 y: 0.0} spacing: 4
            padding: Inset{top: 2 bottom: 8}
            Action{
                tab_text := BigRound{draw_icon +: {svg: crate_resource("self:resources/icons/keyboard.svg")}}
                tab_text_label := BigLabel{text: "Translate"}
            }
            Action{
                tab_cam := BigRound{draw_icon +: {svg: crate_resource("self:resources/icons/camera.svg")}}
                tab_cam_label := BigLabel{text: "Camera"}
            }
            Action{
                tab_listen := BigRound{draw_icon +: {svg: crate_resource("self:resources/icons/ear.svg")}}
                tab_listen_label := BigLabel{text: "Listen"}
            }
        }
    }
}

/// Per status line: the step it shows and when that step began.
type ActSince = std::collections::HashMap<&'static str, (String, f64)>;

struct Pending {
    id: u64,
    from: Lang,
    to: Lang,
    source: String,
    target: String,
}

#[derive(Script, ScriptHook, Widget)]
pub struct TranslatorView {
    #[source]
    source: ScriptObjectRef,
    #[deref]
    view: View,
    #[rust]
    started: bool,
    #[rust]
    offline: bool,
    #[rust]
    saved: Saved,
    #[rust]
    storage: Option<StorageHandle>,
    #[rust]
    load_id: Option<StorageRequestId>,
    #[rust]
    save_id: Option<StorageRequestId>,
    #[rust]
    engine: Option<Engine>,
    #[rust]
    job_counter: u64,
    #[rust]
    debounce: Option<Timer>,
    /// The source language the person picked; `None` = detect.
    #[rust]
    pick: Option<Lang>,
    #[rust]
    source_text: String,
    #[rust]
    current: Option<Pending>,
    /// The id of the running pinyin job, 0 when none.
    #[rust]
    pinyin_id: u64,
    #[rust]
    pinyin_text: String,
    #[rust]
    entry_id: Option<u64>,
    #[rust]
    compact: bool,
    #[rust]
    history_page: bool,
    #[rust]
    favourites_only: bool,
    #[rust]
    last_size: Vec2d,
    #[rust]
    rows: Vec<Entry>,
    /// The running hub probe's job id (0: none) and the timer that repeats it.
    #[rust]
    probe_id: u64,
    #[rust]
    probe_timer: Option<Timer>,
    /// The hub's last status line (`model on host`, or why it is not reachable).
    #[rust]
    hub_line: String,
    #[rust]
    hub_ok: Option<bool>,
    #[rust]
    keyboard_up: bool,
    /// The camera laid out for landscape (picture full screen, controls over it).
    #[rust]
    cam_landscape: Option<bool>,
    /// The live status lines: what each said last and since when (for the elapsed time).
    #[rust]
    act_since: ActSince,
    /// The id of a translation whose model has answered (Route): before that it is loading.
    #[rust]
    route_seen: u64,
    /// When the running translation started, for its rate.
    #[rust]
    job_started: f64,
    #[rust]
    cam: CameraState,
    #[rust]
    talk: Talk,
    #[rust]
    models: Models,
    /// A dictated phrase is read aloud once translated.
    #[rust]
    speak_after_translate: bool,
    /// Frames for the activity bar's glide while a model works.
    #[rust]
    activity_frame: NextFrame,
}

/// The camera screen's state.
#[derive(Default)]
struct CameraState {
    open: bool,
    permission: Option<PermissionStatus>,
    input: Option<(VideoInputId, VideoFormatId)>,
    /// Every camera the platform listed, each with its best format and name.
    devices: Vec<(String, VideoInputId, VideoFormatId, usize)>,
    /// Quarter turns clockwise to upright, as the platform reports for the chosen camera.
    quarter_turns: usize,
    device: usize,
    latest: camera::LatestFrame,
    shown_seq: u64,
    texture: Option<Texture>,
    picture: Option<()>,
    picture_aspect: f64,
    /// Lines the reading pass has produced so far, and lines translated so far.
    read_lines: usize,
    /// The translated lines so far, shown while the rest are still coming.
    partial_lines: Vec<String>,
    /// The frame's size at the last layout, to refit the photo when it changes.
    frame_size: Vec2d,
    translated_lines: usize,
    picture_dims: (usize, usize),
    frozen: bool,
    /// The shot, kept while the hub reads it: read again on this phone if the hub cannot.
    read_frame: Option<Frame>,
    /// The text detector's boxes for the shot (on-device), and their channel.
    detecting: Option<std::sync::mpsc::Receiver<Vec<(f32, f32, f32, f32)>>>,
    detected: Option<Vec<(f32, f32, f32, f32)>>,
    job: u64,
    boxes: Vec<TextBox>,
    lines: Vec<String>,
}

impl TranslatorView {
    pub fn set_storage(&mut self, storage: StorageHandle) {
        self.storage = Some(storage);
    }

    fn ensure_started(&mut self, cx: &mut Cx) {
        if self.started {
            return;
        }
        self.started = true;
        self.saved = Saved::default();
        self.offline = std::env::var_os("MAKEPAD_TRANSLATOR_OFFLINE").is_some();
        if self.offline {
            // Language controls remain live without discovery, credentials,
            // microphone access or a fabricated translation.
            self.saved.model = crate::ondevice::MODEL.into();
            self.sync_settings_widgets(cx);
            self.sync_chrome(cx);
            self.view.label(cx, ids!(route)).set_text(cx, "Offline · choose languages");
            return;
        }
        self.engine = Some(Engine::start(Arc::new(|| SignalToUI::set_ui_signal())));
        if let Some(storage) = self.storage.clone() {
            self.load_id = Some(storage.get(cx, SAVE_KEY));
        }
        self.sync_settings_widgets(cx);
        self.sync_chrome(cx);
        self.probe_timer = Some(cx.start_interval(PROBE_SECS));
        self.start_models(cx);
        self.probe(cx);
    }

    // ------------------------------------------------------------ the hub

    /// Ask the hub whether it serves the model; the answer is the status.
    fn probe(&mut self, cx: &mut Cx) {
        if self.offline { return; }
        if self.probe_id != 0 {
            return;
        }
        let id = self.next_job_id();
        self.probe_id = id;
        // A probe must not cancel a translation: it runs only when idle.
        let camera_busy = self.cam.job != 0 && self.cam.lines.is_empty();
        if camera_busy || self.current.as_ref().is_some_and(|c| c.id != 0 && c.target.is_empty()) {
            self.probe_id = 0;
            return;
        }
        let job = Job { id, system: String::new(), text: String::new(), settings: self.hub_settings(), max_tokens: None, image: None };
        if let Some(engine) = &self.engine {
            engine.submit(job);
        }
        if self.hub_ok.is_none() {
            self.view.label(cx, ids!(route)).set_text(cx, "Connecting to the hub…");
        }
    }

    fn apply_credential(&self) {
        if !self.saved.credential.is_empty() {
            std::env::set_var("MAKEPAD_AI_HUB_CREDENTIAL", &self.saved.credential);
        }
        // The paired hub serves speech too, before chat has found it.
        if let Some(base) = crate::engine::hub_base(&self.saved.hub).filter(|b| b.starts_with("https://")) {
            crate::engine::pin_speech_hub(base.trim_start_matches("https://"));
        }
    }

    fn set_hub_status(&mut self, cx: &mut Cx, ok: bool, text: &str) {
        // A hub that is down is looked for again soon (a network change
        // makes the engine forget what was unreachable); a live one rarely.
        if self.hub_ok != Some(ok) {
            self.probe_timer = Some(cx.start_interval(if ok { PROBE_SECS } else { PROBE_DOWN_SECS }));
        }
        self.hub_ok = Some(ok);
        self.hub_line = text.to_string();
        self.view.label(cx, ids!(route)).set_text(cx, text);
        self.sync_run_on(cx);
        self.view.redraw(cx);
    }

    fn set_pair_status(&mut self, cx: &mut Cx, text: &str) {
        self.view.label(cx, ids!(settings.pair_status)).set_text(cx, text);
    }

    fn show_invite(&mut self, cx: &mut Cx) {
        let host = if self.saved.hub.trim().is_empty() { pair::lan_address() } else { Some(self.saved.hub.trim().to_string()) };
        let Some(host) = host else {
            self.set_pair_status(cx, "This machine has no LAN address to put in an invitation. Set the hub address above.");
            return;
        };
        let name = format!("phone-{}", Cx::time_now() as u64 % 100000);
        match pair::issue(&host, &name) {
            Ok(invite) => {
                let uri = invite.uri();
                self.view.text_input(cx, ids!(settings.invite)).set_text(cx, &uri);
                match crate::qr::encode(uri.as_bytes()) {
                    Some(qr) => {
                        let image = self.view.image(cx, ids!(settings.qr));
                        let _ = image.load_svg_from_data(cx, qr.svg().as_bytes());
                        image.set_visible(cx, true);
                    }
                    None => self.view.image(cx, ids!(settings.qr)).set_visible(cx, false),
                }
                self.set_pair_status(cx, &format!("Invitation for {}. Anyone holding it can use your hub: show it only to your phone.", invite.addr));
            }
            Err(why) => self.set_pair_status(cx, &why),
        }
    }

    fn connect_invite(&mut self, cx: &mut Cx) {
        let text = self.view.text_input(cx, ids!(settings.invite)).text();
        match Invite::parse(&text) {
            Ok(invite) => {
                log!("translator: paired with {}", invite.addr);
                self.saved.hub = invite.addr.clone();
                self.saved.credential = invite.credential;
                self.apply_credential();
                self.view.text_input(cx, ids!(settings.hub)).set_text(cx, &invite.addr);
                self.view.text_input(cx, ids!(settings.invite)).set_text(cx, "");
                self.persist(cx);
                // The model store fetches from the paired hub first: reopen it for this one.
                self.start_models(cx);
                self.set_pair_status(cx, &format!("Paired with {}. Checking the hub…", invite.addr));
                self.hub_ok = None;
                self.probe_id = 0;
                self.probe(cx);
            }
            Err(why) => self.set_pair_status(cx, &why),
        }
    }

    // ---------------------------------------------------------- the settings

    /// The remembered hub answered at another address with its own
    /// certificate: remember that address.
    pub(super) fn hub_moved(&mut self, cx: &mut Cx, addr: String) {
        if self.saved.hub.trim() == addr {
            return;
        }
        self.saved.hub = addr;
        self.view.text_input(cx, ids!(settings.hub)).set_text(cx, &self.saved.hub);
        self.persist(cx);
    }

    fn hub_settings(&self) -> HubSettings {
        let model = self.saved.model.trim();
        HubSettings {
            hub: self.saved.hub.clone(),
            model: if model.is_empty() { HubSettings::default().model } else { model.to_string() },
        }
    }

    fn style(&self) -> Style {
        Style { traditional: self.saved.traditional, glossary: self.saved.glossary.clone() }
    }

    fn sync_settings_widgets(&mut self, cx: &mut Cx) {
        self.view.text_input(cx, ids!(settings.hub)).set_text(cx, &self.saved.hub);
        self.view.text_input(cx, ids!(settings.model)).set_text(cx, &self.saved.model);
        self.view.text_input(cx, ids!(settings.glossary)).set_text(cx, &self.saved.glossary);
        self.view.check_box(cx, ids!(options.pinyin_on)).set_active(cx, self.saved.pinyin, Animate::No);
        self.view.check_box(cx, ids!(options.traditional_on)).set_active(cx, self.saved.traditional, Animate::No);
        self.sync_voice_buttons(cx);
        self.sync_run_on(cx);
    }

    /// Where the models run: this phone (when its weights are on it) or a hub.
    fn sync_run_on(&mut self, cx: &mut Cx) {
        // The chosen segment is a raised face on the track.
        let on_phone = self.saved.model == crate::ondevice::MODEL;
        let hub = match self.hub_ok {
            Some(true) => format!("Hub: {}", self.hub_line),
            Some(false) if !on_phone => format!("Hub: {}", self.hub_line),
            Some(false) => "Hub: not reachable".to_string(),
            None if on_phone => "Hub: not in use".to_string(),
            None => "Hub: connecting…".to_string(),
        };
        let phone = if crate::ondevice::available() {
            format!("On-device: {} on this phone", crate::ondevice::LABEL)
        } else {
            format!("On-device: {} is not on this phone (Offline models above)", crate::ondevice::LABEL)
        };
        self.view.label(cx, ids!(settings.run_on.hub_state)).set_text(cx, &hub);
        self.view.label(cx, ids!(settings.run_on.phone_state)).set_text(cx, &phone);
        for (path, chosen) in [(ids!(settings.run_on.run_phone), on_phone), (ids!(settings.run_on.run_hub), !on_phone)] {
            let mut segment = self.view.widget(cx, path);
            if chosen {
                script_apply_eval!(cx, segment, { draw_bg +: {color: mod.translator.surface color_hover: mod.translator.surface color_focus: mod.translator.surface} });
            } else {
                script_apply_eval!(cx, segment, { draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000} });
            }
        }
    }

    /// Switch where the models run: this phone or the hub.
    fn set_run_on_phone(&mut self, cx: &mut Cx, phone: bool) {
        if (self.saved.model == crate::ondevice::MODEL) == phone {
            return;
        }
        // On-device needs the models on the phone: the state line says so.
        if phone && !crate::ondevice::available() {
            self.sync_run_on(cx);
            return;
        }
        let hub_model = "qwen3.8-27b";
        self.saved.model = if phone { crate::ondevice::MODEL.to_string() } else { hub_model.to_string() };
        self.view.text_input(cx, ids!(settings.model)).set_text(cx, &self.saved.model);
        self.persist(cx);
        // Speech follows: the sessions start again on the hub or on the phone.
        self.drop_speech_sessions();
        self.hub_ok = None;
        self.probe_id = 0;
        self.probe(cx);
        self.sync_run_on(cx);
    }

    fn persist(&mut self, cx: &mut Cx) {
        let Some(storage) = self.storage.as_ref() else { return };
        let id = storage.set(cx, SAVE_KEY, self.saved.encode());
        self.save_id = Some(id);
    }

    fn on_storage(&mut self, cx: &mut Cx, responses: &[StorageResponse]) {
        if let Some(r) = responses.iter().find(|r| Some(r.request_id) == self.load_id) {
            self.load_id = None;
            if let Ok(StorageResult::Value(Some(bytes))) = &r.result {
                if let Some(saved) = Saved::decode(bytes) {
                    self.saved = saved;
                    self.apply_credential();
                    self.start_models(cx);
                    self.apply_credential();
                    self.probe_id = 0;
                    self.probe(cx);
                    self.sync_settings_widgets(cx);
                    self.rebuild_rows(cx);
                    self.sync_chrome(cx);
                }
            }
        }
        if let Some(r) = responses.iter().find(|r| Some(r.request_id) == self.save_id) {
            self.save_id = None;
            if let Err(e) = &r.result {
                log!("translator: could not save: {}", e);
            }
        }
    }

    // ------------------------------------------------------------- direction

    fn effective_from(&self) -> Lang {
        self.pick.or_else(|| detect(&self.source_text)).unwrap_or(Lang::En)
    }

    fn sync_chrome(&mut self, cx: &mut Cx) {
        let from = self.effective_from();
        let to = from.other();
        // The direction, in each language's own name.
        self.view.label(cx, ids!(langs.from_lang)).set_text(cx, from.native());
        self.view.label(cx, ids!(langs.to_lang)).set_text(cx, to.native());
        // Each box's mic names its language: either person holds theirs and speaks.
        self.view.button(cx, ids!(source_card.head.dictate)).set_text(cx, from.native());
        self.view.button(cx, ids!(target_card.head.dictate_other)).set_text(cx, to.native());
        // The boxes stay empty and bare until there is something to act on.
        let has_source = !self.source_text.trim().is_empty();
        let has_target = !self.view.text_input(cx, ids!(target_card.output)).text().trim().is_empty();
        for id in [ids!(source_card.head.clear), ids!(source_card.head.copy_src)] {
            self.view.widget(cx, id).set_visible(cx, has_source);
        }
        self.view.widget(cx, ids!(target_card.head.copy_dst)).set_visible(cx, has_target);

        let show_pinyin = self.saved.pinyin && to == Lang::Zh;
        self.view.label(cx, ids!(target_card.pinyin_text)).set_visible(cx, show_pinyin && !self.pinyin_text.is_empty());
        self.view.label(cx, ids!(target_card.pinyin_text)).set_text(cx, &self.pinyin_text);

        let settings_open = self.view.view(cx, ids!(settings)).visible();
        let cam = self.cam.open;
        let talking = self.talk.open;
        let away = cam || talking;
        // On a phone, settings and history are pages of their own.
        let page = self.compact && (settings_open || self.history_page);
        self.view.view(cx, ids!(main)).set_visible(cx, !away && !(self.compact && settings_open));
        self.view.view(cx, ids!(main.history_panel)).set_visible(cx, !self.compact || self.history_page);
        self.view.view(cx, ids!(main.panes)).set_visible(cx, !self.compact || !self.history_page);
        self.view.widget(cx, ids!(main.history_panel.history_head.history_close)).set_visible(cx, self.compact);
        self.apply_camera_layout(cx);
        let cam_land = self.cam_landscape == Some(true);
        self.view.view(cx, ids!(nav)).set_visible(cx, !page && !cam_land);
        self.view.widget(cx, ids!(camera.cam_bar)).set_visible(cx, !cam_land);
        self.view.label(cx, ids!(camera.cam_overlay.cam_from)).set_text(cx, from.native());
        self.view.label(cx, ids!(camera.cam_overlay.cam_to)).set_text(cx, to.native());
        // The lit tab is the mode on screen.
        for (tab, label, on) in [
            (ids!(nav.tab_text), ids!(nav.tab_text_label), !away),
            (ids!(nav.tab_listen), ids!(nav.tab_listen_label), talking),
            (ids!(nav.tab_cam), ids!(nav.tab_cam_label), cam),
        ] {
            let mut b = self.view.widget(cx, tab);
            let mut l = self.view.widget(cx, label);
            if on {
                // Lit in ink: vermilion stays for the action on each screen (mic, shutter, play).
                script_apply_eval!(cx, b, { draw_icon +: {color: mod.translator.on_ink} draw_bg +: {color: mod.translator.ink_card color_hover: mod.translator.ink_card color_focus: mod.translator.ink_card color_down: mod.translator.ink} });
                script_apply_eval!(cx, l, { draw_text +: {color: mod.translator.ink} });
            } else {
                script_apply_eval!(cx, b, { draw_icon +: {color: mod.translator.ink} draw_bg +: {color: mod.translator.surface_2 color_hover: mod.translator.surface_2 color_focus: mod.translator.surface_2 color_down: mod.translator.line} });
                script_apply_eval!(cx, l, { draw_text +: {color: mod.translator.ink_dim} });
            }
        }
        if page || away {
            self.view.widget(cx, ids!(offer)).set_visible(cx, false);
        }

        // The top row belongs to Translate: the language pair and the gear, or the
        // settings title. Voice and Camera have their own top rows.
        self.view.widget(cx, ids!(bar)).set_visible(cx, !away && !(self.compact && self.history_page));
        self.view.widget(cx, ids!(bar.title)).set_visible(cx, settings_open);
        self.view.widget(cx, ids!(bar.langs)).set_visible(cx, !settings_open);
        let mut gear = self.view.widget(cx, ids!(bar.settings_btn));
        if settings_open {
            script_apply_eval!(cx, gear, { draw_icon +: {color: mod.translator.vermilion} draw_bg +: {color: mod.translator.surface color_hover: mod.translator.surface color_focus: mod.translator.surface} });
        } else {
            script_apply_eval!(cx, gear, { draw_icon +: {color: mod.translator.ink_faint} draw_bg +: {color: #0000 color_hover: #0000 color_focus: #0000} });
        }
        // The route line only speaks up when something is wrong (or in settings).
        self.view.widget(cx, ids!(route)).set_visible(cx, !away && (self.hub_ok == Some(false) || settings_open));
        if settings_open {
            self.sync_run_on(cx);
        }
        if away {
            self.view.view(cx, ids!(settings)).set_visible(cx, false);
        }
        if self.keyboard_up && self.compact && !settings_open {
            self.view.widget(cx, ids!(route)).set_visible(cx, false);
        }
        self.view.view(cx, ids!(talk)).set_visible(cx, talking);
        self.view.view(cx, ids!(camera)).set_visible(cx, cam);
        self.view.view(cx, ids!(camera.frame)).set_visible(cx, cam);
        // The list shows what could not be laid over the photo (no detected place).
        let unplaced = self.cam.boxes.iter().zip(self.cam.lines.iter()).any(|(b, l)| b.w <= 0.0 && !l.is_empty());
        self.view.view(cx, ids!(camera.cam_list)).set_visible(cx, cam && self.cam.frozen && unplaced);
        let frozen = self.cam.frozen;
        // No hints: the stage bar says what is happening; the line below it only
        // speaks for a finished shot with nothing found or an error.
        let cam_busy = self.cam.job != 0 && self.cam.lines.is_empty();
        self.view.widget(cx, ids!(camera.cam_status)).set_visible(cx, cam && frozen && !cam_busy && self.cam.lines.is_empty());
        self.view.widget(cx, ids!(camera.shutter_row.shutter)).set_visible(cx, !frozen);
        self.view.widget(cx, ids!(camera.shutter_row.retake)).set_visible(cx, frozen);
        if cam {
            self.fit_picture(cx);
            if !self.cam.lines.is_empty() {
                self.place_overlays(cx);
            }
        }
        self.sync_activity(cx);
        self.view.redraw(cx);
    }

    /// What the models are doing now: the one value the top progress bar draws.
    pub fn activity(&self) -> crate::activity::Activity {
        let models = self.model_list();
        crate::activity::derive(&crate::activity::Inputs {
            models: &models,
            loading: self.probe_id != 0,
            reading: self.cam.open && self.cam.job != 0 && self.cam.boxes.is_empty(),
            translating: self.current.as_ref().is_some_and(|c| c.id != 0 && c.target.is_empty())
                || self.pinyin_id != 0
                || (self.cam.job != 0 && !self.cam.boxes.is_empty() && self.cam.lines.is_empty()),
            listening: self.mic_open() && !self.mic_failed() && ((self.talk.open && self.talk.listening) || self.talk.dictating),
            speaking: self.talk_speaking(),
        })
    }

    /// A progress line: `share` of the track when known, else a calm glide.
    /// Returns whether it needs the next frame.
    fn draw_progress(&mut self, cx: &mut Cx, track: &[LiveId], fill: &[LiveId], busy: bool, share: Option<f64>) -> bool {
        self.view.widget(cx, track).set_visible(cx, busy);
        if !busy {
            return false;
        }
        let width = self.view.widget(cx, track).area().rect(cx).size.x.max(1.0);
        let (left, w) = match share {
            Some(f) => (0.0, width * f.clamp(0.0, 1.0)),
            None => {
                // An eased sweep: a segment of a third of the track, 1.8 s per pass.
                let t = (Cx::time_now() / 1.8).fract();
                let t = t * t * (3.0 - 2.0 * t);
                let seg = width * 0.33;
                let x = (width + seg) * t - seg;
                let left = x.max(0.0);
                (left, ((x + seg).min(width) - left).max(0.0))
            }
        };
        let margin = Inset { left, ..Inset::default() };
        let mut bar = self.view.widget(cx, fill);
        script_apply_eval!(cx, bar, { width: #(w) margin: #(margin) });
        self.view.widget(cx, track).redraw(cx);
        true
    }

    /// Progress where the person is looking: on the translation card while it is
    /// translated and spoken, on the photo while it is read, in the voice area
    /// while speech is recognised and translated.
    fn sync_activity(&mut self, cx: &mut Cx) {
        let mut animate = false;
        // The translation card.
        let translating = self.current.as_ref().is_some_and(|c| c.id != 0 && c.target.is_empty()) || self.pinyin_id != 0;
        let speaking = self.talk_speaking() && !self.talk.open;
        let has_target = !self.view.text_input(cx, ids!(target_card.output)).text().trim().is_empty();
        self.view.widget(cx, ids!(target_card.play_row)).set_visible(cx, has_target || translating);
        self.view.widget(cx, ids!(target_card.play_row.play)).set_visible(cx, has_target && !speaking);
        self.view.widget(cx, ids!(target_card.play_row.stop_play)).set_visible(cx, has_target && speaking);
        let share = if speaking {
            let (played, queued) = self.speech_progress();
            (queued > 0.05).then(|| (played / queued).clamp(0.0, 1.0))
        } else {
            None
        };
        animate |= self.draw_progress(cx, ids!(target_card.play_row.play_track), ids!(target_card.play_row.play_track.play_bar), translating || speaking, share);
        // The photo.
        let cam = &self.cam;
        let cam_busy = cam.open && cam.frozen && cam.job != 0 && cam.lines.is_empty();
        let cam_share = if cam.boxes.is_empty() {
            None
        } else {
            let n = cam.boxes.len().max(1);
            Some(cam.translated_lines.min(n) as f64 / n as f64)
        };
        animate |= self.draw_progress(cx, ids!(camera.cam_activity.cam_progress), ids!(camera.cam_activity.cam_progress.fill), cam_busy, cam_share);
        // The voice area.
        let voice_busy = self.talk.open && (self.talk_working() || self.talk_speaking());
        animate |= self.draw_progress(cx, ids!(talk.voice_progress), ids!(talk.voice_progress.fill), voice_busy, None);

        // The words: what each model is doing right now, with its numbers.
        let card = self.card_activity(cx);
        let camera = self.camera_activity();
        let voice = self.talk_activity();
        animate |= self.show_act(cx, "card", ids!(target_card.card_act), None, card);
        animate |= self.show_act(cx, "camera", ids!(camera.cam_activity.cam_act), Some(ids!(camera.cam_activity)), camera);
        animate |= self.show_act(cx, "voice", ids!(talk.voice_act), None, voice);
        if animate {
            self.activity_frame = cx.new_next_frame();
        }
    }

    /// Show `text` (with the step's elapsed time once past 2 s, and a note when it
    /// runs long) in `label`; hide it (and `holder`) when idle. True while shown.
    fn show_act(&mut self, cx: &mut Cx, key: &'static str, label: &[LiveId], holder: Option<&[LiveId]>, text: Option<String>) -> bool {
        let Some(text) = text else {
            self.act_since.remove(key);
            self.view.widget(cx, label).set_visible(cx, false);
            if let Some(h) = holder {
                self.view.widget(cx, h).set_visible(cx, false);
            }
            return false;
        };
        let now = Cx::time_now();
        // A step is its words without the numbers that tick within it.
        let step: String = text.chars().take_while(|c| !c.is_ascii_digit() && *c != '·').collect();
        let since = match self.act_since.get(key) {
            Some((s, t)) if *s == step => *t,
            _ => {
                self.act_since.insert(key, (step, now));
                now
            }
        };
        let elapsed = now - since;
        let mut line = text;
        if elapsed >= 2.0 {
            line.push_str(&format!(" · {:.0} s", elapsed));
        }
        if elapsed >= 20.0 {
            line.push_str(" · taking longer than usual");
        }
        self.view.label(cx, label).set_text(cx, &line);
        self.view.widget(cx, label).set_visible(cx, true);
        if let Some(h) = holder {
            self.view.widget(cx, h).set_visible(cx, true);
        }
        true
    }

    fn model_name(&self) -> &'static str {
        if self.hub_settings().model == crate::ondevice::MODEL { "Qwen 3.5 4B" } else { "the hub's model" }
    }

    /// The translation card: loading the model, translating (characters so far and
    /// rate), writing pinyin, making the voice and playing it.
    fn card_activity(&self, cx: &mut Cx) -> Option<String> {
        if self.talk_speaking() && !self.talk.open {
            let (played, queued) = self.speech_progress();
            return Some(if queued < 0.05 { "Generating speech (Kokoro)…".to_string() } else { format!("Speaking {played:.1} of {queued:.1} s") });
        }
        if self.pinyin_id != 0 {
            return Some(format!("Writing pinyin · {} syllables", self.pinyin_text.split_whitespace().count()));
        }
        let current = self.current.as_ref().filter(|c| c.id != 0 && c.target.is_empty())?;
        if self.route_seen != current.id {
            return Some(if self.hub_settings().model == crate::ondevice::MODEL {
                format!("Loading {} into GPU memory…", self.model_name())
            } else {
                "Waiting for the hub…".to_string()
            });
        }
        let chars = self.view.text_input(cx, ids!(target_card.output)).text().chars().count();
        if chars == 0 {
            return Some(format!("{} is reading your {} characters…", self.model_name(), current.source.chars().count()));
        }
        let secs = (Cx::time_now() - self.job_started).max(0.1);
        Some(format!("Translating · {} characters · {:.0} chars/s", chars, chars as f64 / secs))
    }

    /// The photo: finding the text, reading it, translating it line by line.
    fn camera_activity(&self) -> Option<String> {
        let cam = &self.cam;
        if !(cam.open && cam.frozen) {
            return None;
        }
        let regions = match &cam.detected {
            Some(found) => format!(" · {} text regions found", found.len()),
            None if cam.detecting.is_some() => " · detecting text regions…".to_string(),
            None => String::new(),
        };
        if cam.job != 0 && cam.boxes.is_empty() {
            return Some(if cam.read_lines == 0 {
                format!("Reading the picture ({}, vision){}", self.model_name(), regions)
            } else {
                format!("Reading text · line {}{}", cam.read_lines, regions)
            });
        }
        if cam.job != 0 && cam.lines.is_empty() {
            let n = cam.boxes.len();
            return Some(format!("Translating line {} of {}", (cam.translated_lines + 1).min(n), n));
        }
        None
    }

    /// Listening and speaking (dictation into the boxes, and the Listen screen).
    fn talk_activity(&self) -> Option<String> {
        let t = &self.talk;
        if t.dictating && t.listening && !self.mic_failed() {
            return Some("Listening… speak now".to_string());
        }
        if t.finals_pending > 0 {
            return Some(format!("Whisper: transcribing {:.1} s of audio", t.heard_secs));
        }
        if t.job.is_some() {
            return Some(format!("Translating what was said ({})", self.model_name()));
        }
        if self.talk_speaking() {
            let (played, queued) = self.speech_progress();
            return Some(if queued < 0.05 { "Generating speech (Kokoro)…".to_string() } else { format!("Speaking {played:.1} of {queued:.1} s") });
        }
        if t.open && t.listening && !self.mic_failed() {
            return Some(if t.in_utterance { "Hearing speech…".to_string() } else { "Listening for speech…".to_string() });
        }
        None
    }

    /// Camera in landscape: the picture takes the whole screen and the controls
    /// float over it (shutter on the right edge, modes on the left, the language
    /// pair on top). Portrait keeps the stacked layout.
    fn apply_camera_layout(&mut self, cx: &mut Cx) {
        let landscape = self.cam.open && self.last_size.x > self.last_size.y;
        if self.cam_landscape == Some(landscape) {
            return;
        }
        self.cam_landscape = Some(landscape);
        let mut camera = self.view.widget(cx, ids!(camera));
        let mut row = self.view.widget(cx, ids!(camera.shutter_row));
        if landscape {
            script_apply_eval!(cx, camera, { flow: mod.turtle.Overlay margin.top: 0.0 });
            // A column on the right edge: the row spans the screen, its content is
            // pushed into the last 110 points and centred there.
            let left = (self.last_size.x - 24.0 - 110.0).max(0.0);
            script_apply_eval!(cx, row, { width: mod.turtle.Fill height: mod.turtle.Fill flow: mod.turtle.Down padding.left: #(left) padding.right: 0.0 spacing: 14.0 });
        } else {
            script_apply_eval!(cx, camera, { flow: mod.turtle.Down margin.top: mod.widgets.SAFE_INSET_PAD_TOP });
            script_apply_eval!(cx, row, { width: mod.turtle.Fill height: 72.0 flow: mod.turtle.Right padding.left: 0.0 padding.right: 0.0 spacing: 0.0 });
        }
        self.view.widget(cx, ids!(camera.cam_overlay)).set_visible(cx, landscape);
        let mut act = self.view.widget(cx, ids!(camera.cam_activity));
        if landscape {
            // Below the language pair, over the picture.
            script_apply_eval!(cx, act, { width: 420.0 margin.left: 90.0 margin.top: 62.0 margin.right: 0.0 });
        } else {
            script_apply_eval!(cx, act, { width: mod.turtle.Fill margin.left: 18.0 margin.top: 0.0 margin.right: 18.0 });
        }
        self.cam.frame_size = Vec2d::default();
        self.view.redraw(cx);
    }

    fn apply_layout(&mut self, cx: &mut Cx, size: Vec2d) {
        let compact = size.x < COMPACT_WIDTH;
        if compact == self.compact && self.last_size.x > 0.0 {
            return;
        }
        let mut panes = self.view.widget(cx, ids!(main.panes));
        let mut main = self.view.widget(cx, ids!(main));
        let mut history = self.view.widget(cx, ids!(main.history_panel));
        self.compact = compact;
        if compact {
            script_apply_eval!(cx, panes, { flow: mod.turtle.Down });
            script_apply_eval!(cx, main, { flow: mod.turtle.Down });
            script_apply_eval!(cx, history, { width: mod.turtle.Fill });
        } else {
            script_apply_eval!(cx, panes, { flow: mod.turtle.Right });
            script_apply_eval!(cx, main, { flow: mod.turtle.Right });
            script_apply_eval!(cx, history, { width: 320 });
            self.history_page = false;
        }
        self.sync_chrome(cx);
    }

    // ---------------------------------------------------------- translating

    fn next_job_id(&mut self) -> u64 {
        self.job_counter += 1;
        self.job_counter
    }

    fn set_status(&mut self, cx: &mut Cx, text: &str, error: bool) {
        let label = self.view.label(cx, ids!(target_card.status));
        label.set_text(cx, text);
        // Progress words are the activity bar's job; the card keeps errors and advice.
        label.set_visible(cx, error || self.activity().kind == crate::activity::Kind::Idle);
        let color = if error { vec4(0.86, 0.28, 0.25, 1.0) } else { vec4(0.5, 0.5, 0.55, 1.0) };
        label.set_text_color(cx, color);
    }

    fn set_output(&mut self, cx: &mut Cx, text: &str) {
        self.view.text_input(cx, ids!(target_card.output)).set_text(cx, text);
    }

    fn start_translate(&mut self, cx: &mut Cx) {
        let text = self.source_text.trim().to_string();
        let id = self.next_job_id();
        self.pinyin_id = 0;
        self.pinyin_text.clear();
        self.entry_id = None;
        if text.is_empty() {
            if let Some(engine) = &self.engine {
                engine.cancel(id);
            }
            self.current = None;
            self.set_output(cx, "");
            self.set_status(cx, "", false);
            self.sync_chrome(cx);
            return;
        }
        if self.offline {
            self.set_status(cx, "Translation is unavailable offline on this device.", false);
            return;
        }
        let from = self.effective_from();
        let to = from.other();
        self.current = Some(Pending { id, from, to, source: text.clone(), target: String::new() });
        self.job_started = Cx::time_now();
        self.set_status(cx, "Translating…", false);
        let job = Job { id, system: translate_system(from, to, &self.style()), text, settings: self.hub_settings(), max_tokens: None, image: None };
        if let Some(engine) = &self.engine {
            engine.submit(job);
        }
        self.sync_chrome(cx);
    }

    fn start_pinyin(&mut self, cx: &mut Cx) {
        let Some(current) = &self.current else { return };
        if current.to != Lang::Zh || current.target.is_empty() {
            return;
        }
        let text = current.target.clone();
        let id = self.next_job_id();
        self.pinyin_id = id;
        let job = Job { id, system: pinyin_system(), text, settings: self.hub_settings(), max_tokens: None, image: None };
        if let Some(engine) = &self.engine {
            engine.submit(job);
        }
        self.set_status(cx, "Writing pinyin…", false);
    }

    fn finish_entry(&mut self, cx: &mut Cx) {
        let Some(current) = &self.current else { return };
        if current.target.is_empty() {
            return;
        }
        let id = self.saved.remember(current.from.code(), current.to.code(), &current.source, &current.target, &self.pinyin_text);
        self.entry_id = Some(id);
        self.persist(cx);
        self.rebuild_rows(cx);
    }

    fn on_update(&mut self, cx: &mut Cx, update: Update) {
        if let Update::HubMoved { addr } = update {
            self.hub_moved(cx, addr);
            return;
        }
        if self.on_camera_update(cx, &update) {
            self.sync_chrome(cx);
            return;
        }
        self.on_update_inner(cx, update);
        self.sync_chrome(cx);
    }

    fn on_update_inner(&mut self, cx: &mut Cx, update: Update) {
        match update {
            Update::Route { id, label } => {
                self.route_seen = id;
                if self.probe_id == id || self.current.as_ref().is_some_and(|c| c.id == id) || self.pinyin_id == id {
                    if self.hub_settings().model == crate::ondevice::MODEL {
                        // the phone's own model: the route line says so, the hub's state stays the hub's
                        self.view.label(cx, ids!(route)).set_text(cx, &label);
                    } else {
                        self.set_hub_status(cx, true, &label);
                    }
                }
            }
            Update::Done { id, .. } if id == self.probe_id => {
                self.probe_id = 0;
            }
            Update::Failed { id, error } if id == self.probe_id => {
                self.probe_id = 0;
                log!("translator: hub probe failed: {}", error);
                let was_down = self.hub_ok == Some(false);
                self.set_hub_status(cx, false, &error);
                let _ = was_down;
            }
            Update::Partial { id, text } => {
                let text = clean(&text);
                if self.pinyin_id == id {
                    self.pinyin_text = text;
                    self.sync_chrome(cx);
                } else if self.current.as_ref().is_some_and(|c| c.id == id) {
                    if let Some(kept) = talk::looping(&text) {
                        // The model repeats itself: stop it and keep one copy.
                        log!("translator: translation looped ({} chars), stopped at one copy", text.chars().count());
                        if let Some(engine) = &self.engine {
                            engine.cancel(self.job_counter + 1);
                        }
                        self.on_update_inner(cx, Update::Done { id, text: kept });
                        return;
                    }
                    if !text.is_empty() {
                        self.set_output(cx, &text);
                    }
                }
            }
            Update::Done { id, text } => {
                let text = clean(&text);
                if self.pinyin_id == id {
                    self.pinyin_id = 0;
                    self.pinyin_text = text;
                    self.set_status(cx, "", false);
                    self.finish_entry(cx);
                    self.sync_chrome(cx);
                } else if self.current.as_ref().is_some_and(|c| c.id == id) {
                    self.set_output(cx, &text);
                    if self.speak_after_translate {
                        self.speak_after_translate = false;
                        let to = self.current.as_ref().map_or(Lang::Zh, |c| c.to);
                        self.speak(&text, to);
                    }
                    let wants_pinyin = {
                        let current = self.current.as_mut().unwrap();
                        current.target = text;
                        current.to == Lang::Zh && self.saved.pinyin
                    };
                    if wants_pinyin {
                        self.start_pinyin(cx);
                    } else {
                        self.set_status(cx, "", false);
                        self.finish_entry(cx);
                    }
                }
            }
            Update::Failed { id, error } => {
                if self.pinyin_id == id {
                    self.pinyin_id = 0;
                    self.finish_entry(cx);
                    self.set_status(cx, &format!("Pinyin failed: {error}"), true);
                } else if self.current.as_ref().is_some_and(|c| c.id == id) {
                    self.set_status(cx, &error, true);
                }
                // Only a hub can be unreachable; the phone's own model reports its own error.
                if self.hub_settings().model != crate::ondevice::MODEL {
                    self.set_hub_status(cx, false, "Hub unreachable");
                }
            }
            Update::HubMoved { addr } => self.hub_moved(cx, addr),
        }
    }

    // --------------------------------------------------------------- history

    fn rebuild_rows(&mut self, cx: &mut Cx) {
        self.rows = self.saved.history.iter().filter(|e| !self.favourites_only || e.favourite).cloned().collect();
        self.view.redraw(cx);
    }

    fn open_entry(&mut self, cx: &mut Cx, entry: &Entry) {
        let from = if entry.from == "zh" { Lang::Zh } else { Lang::En };
        let to = from.other();
        self.pick = Some(from);
        self.source_text = entry.source.clone();
        self.view.text_input(cx, ids!(source_card.input)).set_text(cx, &entry.source);
        self.set_output(cx, &entry.target);
        self.pinyin_text = entry.pinyin.clone();
        self.entry_id = Some(entry.id);
        self.current = Some(Pending { id: 0, from, to, source: entry.source.clone(), target: entry.target.clone() });
        self.history_page = false;
        self.set_status(cx, "", false);
        self.sync_chrome(cx);
    }

    fn draw_rows(&self, cx: &mut Cx2d, list: &mut PortalList) {
        let count = self.rows.len().max(1);
        list.set_item_range(cx, 0, count);
        while let Some(index) = list.next_visible_item(cx) {
            if index >= count {
                continue;
            }
            if self.rows.is_empty() {
                let item = list.item(cx, index, live_id!(Empty));
                item.set_text(cx, if self.favourites_only { "No favourites yet" } else { "Your translations appear here" });
                item.draw_all(cx, &mut Scope::empty());
                continue;
            }
            let entry = &self.rows[index];
            let item = list.item(cx, index, live_id!(Row));
            let one_line = |s: &str| s.lines().next().unwrap_or("").to_string();
            item.label(cx, ids!(open.texts.src)).set_text(cx, &one_line(&entry.source));
            item.label(cx, ids!(open.texts.dst)).set_text(cx, &entry.target.replace('\n', " "));
            let star = if entry.favourite { vec4(0.96, 0.72, 0.18, 1.0) } else { vec4(0.5, 0.5, 0.55, 1.0) };
            let _ = star;
            item.draw_all(cx, &mut Scope::empty());
        }
    }

    // ---------------------------------------------------------------- camera

    fn cam_status(&mut self, cx: &mut Cx, text: &str) {
        self.view.label(cx, ids!(camera.cam_status)).set_text(cx, text);
    }

    fn open_camera(&mut self, cx: &mut Cx) {
        self.drop_text_focus(cx);
        self.cam.open = true;
        self.cam.frozen = false;
        self.cam.picture = None;
        self.cam_status(cx, "Point at a menu or a sign, then press the shutter.");
        self.view.label(cx, ids!(camera.frame.no_picture)).set_text(cx, "Waiting for the camera…");
        self.cam.latest.clear();
        let input = self.cam.latest.input_fn();
        let mut input = Some(input);
        cx.camera_frame_input(0, move |frame| {
            if let Some(f) = input.as_mut() {
                f(frame);
            }
            SignalToUI::set_ui_signal();
        });
        cx.request_permission(Permission::Camera);
        cx.video_input(0, |_buf| {});
        self.sync_chrome(cx);
        self.drive_camera(cx);
    }

    fn close_camera(&mut self, cx: &mut Cx) {
        crate::engine::release_vision();
        self.cam.detecting = None;
        self.cam.detected = None;
        self.cancel_camera_jobs();
        self.cam.open = false;
        self.cam.frozen = false;
        self.cam.read_frame = None;
        self.cam.job = 0;
        self.cam.boxes.clear();
        self.cam.lines.clear();
        self.cam.picture = None;
        cx.camera_frame_input(0, |_frame| {});
        cx.use_video_input(&[]);
        self.show_overlays(cx);
        self.sync_chrome(cx);
    }

    /// Start the stream once permission and a camera are both known.
    fn drive_camera(&mut self, cx: &mut Cx) {
        if !self.cam.open || !matches!(self.cam.permission, Some(PermissionStatus::Granted)) {
            return;
        }
        if let Some(input) = self.cam.input {
            cx.use_video_input(&[input]);
        }
    }

    fn on_video_inputs(&mut self, cx: &mut Cx, ev: &VideoInputsEvent) {
        // Every camera, each in its best NV12/I420/YUY2/MJPEG format up to 1080p.
        let rank = |p: VideoPixelFormat| match p {
            VideoPixelFormat::NV12 => 4,
            VideoPixelFormat::YUV420 => 3,
            VideoPixelFormat::YUY2 => 2,
            VideoPixelFormat::MJPEG => 1,
            _ => 0,
        };
        self.cam.devices = ev
            .descs
            .iter()
            .filter_map(|desc| {
                desc.formats
                    .iter()
                    .filter(|f| rank(f.pixel_format) > 0)
                    .max_by_key(|f| (rank(f.pixel_format), f.width <= 1920 && f.height <= 1080, f.width * f.height))
                    .map(|f| (desc.name.clone(), desc.input_id, f.format_id, desc.sensor_orientation as usize / 90))
            })
            .collect();
        self.cam.device = self.cam.device.min(self.cam.devices.len().saturating_sub(1));
        // The phone always uses its back camera (no picker); the desktop keeps the chosen one.
        if cfg!(target_os = "android") {
            self.cam.device = self.cam.devices.iter().position(|d| d.0.to_lowercase().contains("back")).unwrap_or(0);
        }
        self.cam.input = self.cam.devices.get(self.cam.device).map(|d| (d.1, d.2));
        self.cam.quarter_turns = self.cam.devices.get(self.cam.device).map_or(0, |d| d.3);
        for (i, d) in self.cam.devices.iter().enumerate() {
            log!("translator: camera {i} {:?} sensor turns {}{}", d.0, d.3, if i == self.cam.device { " (chosen)" } else { "" });
        }
        if self.cam.input.is_none() && self.cam.open {
            self.cam_status(cx, "No usable camera found. The test picture still works.");
        }
        self.sync_device_button(cx);
        self.drive_camera(cx);
    }

    fn sync_device_button(&mut self, cx: &mut Cx) {
        let many = self.cam.devices.len() > 1 && !cfg!(target_os = "android");
        let name = self.cam.devices.get(self.cam.device).map(|d| d.0.clone()).unwrap_or_default();
        let b = self.view.button(cx, ids!(camera.cam_bar.cam_device));
        b.set_visible(cx, many);
        b.set_text(cx, &name);
    }

    fn next_device(&mut self, cx: &mut Cx) {
        if self.cam.devices.len() < 2 || cfg!(target_os = "android") {
            return;
        }
        self.cam.device = (self.cam.device + 1) % self.cam.devices.len();
        self.cam.input = self.cam.devices.get(self.cam.device).map(|d| (d.1, d.2));
        self.cam.quarter_turns = self.cam.devices.get(self.cam.device).map_or(0, |d| d.3);
        self.cam.shown_seq = 0;
        self.sync_device_button(cx);
        self.drive_camera(cx);
    }

    /// Show a picture in the viewfinder (a camera frame or the test picture).
    fn set_picture(&mut self, cx: &mut Cx, rgba: &camera::Rgba) {
        let words = rgba.bgra_words();
        // A new size (viewfinder step vs. full-size capture, rotated 90/270) gets a
        // texture of exactly that size, so the image never keeps stale dimensions.
        if self.cam.picture_dims != (rgba.width, rgba.height) {
            self.cam.texture = None;
            self.cam.picture_dims = (rgba.width, rgba.height);
        }
        let texture = match &self.cam.texture {
            Some(t) => t.clone(),
            None => {
                let t = Texture::new_with_format(
                    cx,
                    TextureFormat::VecBGRAu8_32 { width: rgba.width, height: rgba.height, data: Some(vec![]), updated: TextureUpdated::Full },
                );
                self.cam.texture = Some(t.clone());
                t
            }
        };
        texture.set_data_u32(cx, rgba.width, rgba.height, words);
        let image = self.view.image(cx, ids!(camera.frame.cam_image));
        image.set_texture(cx, Some(texture));
        self.cam.picture_aspect = rgba.width as f64 / rgba.height.max(1) as f64;
        self.fit_picture(cx);
        self.view.label(cx, ids!(camera.frame.no_picture)).set_text(cx, "");
        self.view.redraw(cx);
    }

    /// The picture as large as the viewfinder allows, its own aspect (no crop,
    /// no stretch), centred -- the same box the overlays are laid out in. Sized
    /// and placed here: a Fill image inside an aligned holder measured half its
    /// width on the phone.
    /// Quarter turns that make the chosen camera's frames upright as the phone
    /// is held now (the sensor against the display's current rotation).
    fn upright_turns(&self) -> usize {
        makepad_widgets::makepad_platform::video::camera_upright_quarter_turns(self.cam.quarter_turns as u16 * 90, false)
    }

    fn fit_picture(&mut self, cx: &mut Cx) {
        let frame = self.view.widget(cx, ids!(camera.frame)).area().rect(cx);
        let aspect = self.cam.picture_aspect;
        if frame.size.x < 1.0 || frame.size.y < 1.0 || aspect <= 0.0 {
            return;
        }
        let (w, h) = if frame.size.x / frame.size.y > aspect {
            (frame.size.y * aspect, frame.size.y)
        } else {
            (frame.size.x, frame.size.x / aspect)
        };
        // Centred across, sitting on the frame's foot: the progress line and the
        // result list follow right under the photo.
        let top = if self.cam_landscape == Some(true) { (frame.size.y - h) / 2.0 } else { frame.size.y - h };
        let margin = Inset { left: (frame.size.x - w) / 2.0, top, right: 0.0, bottom: 0.0 };
        let mut image = self.view.widget(cx, ids!(camera.frame.cam_image));
        script_apply_eval!(cx, image, { width: #(w) height: #(h) margin: #(margin) });
        // The result list sits over the photo's foot, as wide as the photo.
        let list_h = 210.0f64.min(h * 0.45);
        let list_margin = Inset { left: margin.left, top: frame.size.y - list_h, right: 0.0, bottom: 0.0 };
        let mut list = self.view.widget(cx, ids!(camera.frame.cam_list));
        script_apply_eval!(cx, list, { width: #(w) height: #(list_h) margin: #(list_margin) });
    }

    fn on_camera_event(&mut self, cx: &mut Cx, event: &Event) {
        match event {
            // The on-screen keyboard takes the bottom of the screen: the window body
            // reflows above it, and the optional rows (examples, the offline offer)
            // step aside so both boxes stay visible.
            Event::VirtualKeyboard(kb) => {
                let up = matches!(kb, VirtualKeyboardEvent::WillShow { height, .. } | VirtualKeyboardEvent::DidShow { height, .. } if *height > 0.0);
                if up != self.keyboard_up {
                    self.keyboard_up = up;
                    // The two boxes share the room above the keyboard (both Fill); the
                    // tabs ride on top of the keyboard (sync_chrome).
                    self.sync_chrome(cx);
                    self.refresh_models(cx);
                }
            }
            Event::Pause | Event::Background | Event::Shutdown => self.stop_all_recording(cx),
            Event::VideoInputs(ev) => self.on_video_inputs(cx, ev),
            Event::PermissionResult(r) if r.permission == Permission::Camera => {
                self.cam.permission = Some(r.status);
                if self.cam.open {
                    match r.status {
                        PermissionStatus::Granted => self.drive_camera(cx),
                        _ => self.cam_status(cx, "The camera needs permission (the test picture works without it)."),
                    }
                }
            }
            _ => {}
        }
        // The viewfinder: the newest camera frame as an image texture.
        if self.cam.open && !self.cam.frozen {
            let seq = self.cam.latest.refresh();
            if seq != self.cam.shown_seq {
                self.cam.shown_seq = seq;
                // Half size, turned upright for a phone held in portrait.
                if seq == 1 {
                    log!("translator: first viewfinder frame turned {} quarter(s) (display rotation {})", self.upright_turns(), makepad_widgets::makepad_platform::video::display_rotation());
                }
                if let Some(rgba) = self.cam.latest.rgba(2, self.upright_turns()) {
                    self.set_picture(cx, &rgba);
                }
            }
        }
        // The detector's boxes: kept for the read; a late answer places lines
        // already translated.
        if let Some(found) = self.cam.detecting.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.cam.detecting = None;
            if !self.cam.boxes.is_empty() {
                crate::detect::pair(&mut self.cam.boxes, &found);
                self.place_overlays(cx);
            }
            self.cam.detected = Some(found);
        }
    }

    /// Read the picture: the frame goes to the reader as RGBA.
    fn read_picture(&mut self, cx: &mut Cx, rgba: camera::Rgba) {
        self.cam.read_lines = 0;
        self.cam.translated_lines = 0;
        self.set_picture(cx, &rgba);
        self.cam.frozen = true;
        let on_device = self.hub_settings().model == crate::ondevice::MODEL;
        self.read_and_translate(cx, rgba.into(), on_device);
    }

    /// Drop the camera's running read/translate work: the worker sees a newer
    /// id and cancels the on-device turn (or abandons the hub request).
    fn cancel_camera_jobs(&mut self) {
        if self.cam.job != 0 {
            if let Some(engine) = &self.engine {
                engine.cancel(u64::MAX);
            }
            self.cam.job = 0;
        }
    }

    /// The language the camera results actually came out in (from the text, not the flip setting).
    fn result_direction(&self) -> &'static str {
        let joined = self.cam.lines.join(" ");
        detect(&joined).map_or(self.effective_from().other().native(), |l| l.native())
    }

    fn shutter(&mut self, cx: &mut Cx) {
        let busy = self.cam.job != 0 && self.cam.lines.is_empty();
        self.cancel_camera_jobs();
        self.cam.read_frame = None;
        if self.cam.frozen && !busy {
            // Back to the live view.
            self.cam.frozen = false;
            self.cam.job = 0;
            self.cam.boxes.clear();
            self.cam.lines.clear();
            self.cam.shown_seq = 0;
            self.show_overlays(cx);
            self.cam_status(cx, "Point at a menu or a sign, then press the shutter.");
            return;
        }
        // Full-size upright pixels of the newest frame.
        self.cam.latest.refresh();
        match self.cam.latest.rgba(1, self.upright_turns()) {
            Some(rgba) => self.read_picture(cx, rgba),
            None => self.cam_status(cx, "No camera frame yet."),
        }
    }

    fn translate_boxes(&mut self, cx: &mut Cx, boxes: Vec<TextBox>) {
        let id = self.next_job_id();
        self.cam.job = id;
        let job = Job {
            id,
            system: reader::numbered_system(self.effective_from().other(), &self.style()),
            text: reader::numbered_prompt(&boxes),
            settings: self.hub_settings(),
            max_tokens: None,
            image: None,
        };
        self.cam.boxes = boxes;
        self.cam.lines.clear();
        self.cam_status(cx, &format!("Translating {} lines…", self.cam.boxes.len()));
        if let Some(engine) = &self.engine {
            engine.submit(job);
        }
    }

    /// A vision model reads the photo's lines: the hub's (its chat model's
    /// vision tower) or, `on_device`, the phone's own (the picture downscaled to
    /// one fixed size, so one vision graph serves every photo). The lines then
    /// go through one numbered translation job.
    fn read_and_translate(&mut self, cx: &mut Cx, frame: Frame, on_device: bool) {
        // Where the lines are: the detector runs beside the vision model's read
        // (a fraction of a second) and its boxes place the translations.
        self.cam.detected = None;
        let (tx, rx) = std::sync::mpsc::channel();
        self.cam.detecting = Some(rx);
        let det_frame = Frame { width: frame.width, height: frame.height, rgba: frame.rgba.clone() };
        std::thread::spawn(move || {
            let _ = tx.send(crate::detect::lines(&det_frame));
            SignalToUI::set_ui_signal();
        });
        let id = self.next_job_id();
        self.cam.job = id;
        self.cam.boxes.clear();
        self.cam.lines.clear();
        let (settings, image, status) = if on_device {
            let mut settings = self.hub_settings();
            settings.model = crate::ondevice::MODEL.to_string();
            (settings, reader::fit_square(&frame, reader::ON_DEVICE_SIDE), "Reading the text on this phone…")
        } else {
            (self.hub_settings(), reader::fit_square(&frame, reader::HUB_SIDE), "Reading the text on the hub…")
        };
        // Kept for one fallback: the hub cannot read it -> the phone's own model.
        self.cam.read_frame = (!on_device && crate::ondevice::available()).then_some(frame);
        let job = Job {
            id,
            system: reader::on_device_read_system(),
            text: reader::ON_DEVICE_PROMPT.to_string(),
            settings,
            max_tokens: Some(400),
            image: Some(image),
        };
        self.cam_status(cx, status);
        if let Some(engine) = &self.engine {
            engine.submit(job);
        }
    }

    fn on_camera_update(&mut self, cx: &mut Cx, update: &Update) -> bool {
        match update {
            // The on-device picture pass answered with the lines it read: translate
            // exactly those lines (text turn), so each original keeps its own translation.
            Update::Done { id, text } if *id == self.cam.job && self.cam.boxes.is_empty() => {
                let mut boxes = reader::parse_read_lines(&clean(text));
                // Only real places go over the photo: the detector's boxes, or none.
                crate::detect::pair(&mut boxes, self.cam.detected.as_deref().unwrap_or(&[]));
                if boxes.is_empty() {
                    self.cam_status(cx, "No text found. Press the shutter to try again.");
                    return true;
                }
                self.translate_boxes(cx, boxes);
                true
            }
            Update::Done { id, text } if *id == self.cam.job => {
                self.cam.lines = reader::parse_numbered(&clean(text), self.cam.boxes.len());
                self.cam_status(cx, &format!("Translated → {}.", self.result_direction()));
                self.show_overlays(cx);
                true
            }
            Update::Failed { id, error } if *id == self.cam.job => {
                // The hub could not read the photo: the phone's own model reads it.
                if self.cam.boxes.is_empty() {
                    if let Some(frame) = self.cam.read_frame.take() {
                        log!("translator: camera: the hub could not read the photo ({error}); reading on this phone");
                        self.read_and_translate(cx, frame, true);
                        return true;
                    }
                    self.cam_status(cx, &format!("Could not read the picture: {error}"));
                } else {
                    self.cam_status(cx, error);
                }
                true
            }
            Update::Partial { id, text } if *id == self.cam.job => {
                let lines = clean(text).lines().filter(|l| !l.trim().is_empty()).count();
                if self.cam.boxes.is_empty() {
                    self.cam.read_lines = lines;
                    self.cam_status(cx, &format!("Reading the text… {lines} lines so far"));
                } else {
                    self.cam.translated_lines = lines;
                    // Lines appear on the photo as they are translated.
                    self.cam.partial_lines = reader::parse_numbered(&clean(text), self.cam.boxes.len());
                    self.place_overlays(cx);
                }
                true
            }
            Update::Route { id, .. } if *id == self.cam.job => true,
            _ => false,
        }
    }

    /// Lay the translated lines over the picture at the text's positions.
    fn show_overlays(&mut self, cx: &mut Cx) {
        self.place_overlays(cx);
        self.sync_chrome(cx);
    }

    fn place_overlays(&mut self, cx: &mut Cx) {
        let frame_rect = self.view.widget(cx, ids!(camera.frame)).area().rect(cx);
        // The picture is drawn to fit inside the frame, centred.
        let aspect = self.cam.picture_aspect.max(0.1);
        let (iw, ih) = if frame_rect.size.x / frame_rect.size.y.max(1.0) > aspect {
            (frame_rect.size.y * aspect, frame_rect.size.y)
        } else {
            (frame_rect.size.x, frame_rect.size.x / aspect)
        };
        let rect = Rect {
            pos: dvec2((frame_rect.size.x - iw) / 2.0, if self.cam_landscape == Some(true) { (frame_rect.size.y - ih) / 2.0 } else { frame_rect.size.y - ih }),
            size: dvec2(iw, ih),
        };
        let (w, h) = (rect.size.x, rect.size.y);
        for i in 0..10usize {
            let path = [live_id!(camera), live_id!(frame), LiveId::from_str(&format!("ov{i}"))];
            let mut card = self.view.widget(cx, &path);
            let lines = if self.cam.lines.is_empty() { &self.cam.partial_lines } else { &self.cam.lines };
            let shown = match (self.cam.boxes.get(i), lines.get(i)) {
                (Some(b), Some(line)) if !line.is_empty() && w > 1.0 && b.w > 0.0 => {
                    // From the line's start to the photo's right edge: the translation
                    // is usually longer than the original, and wraps there.
                    let x = ((b.x * w as f32) as f64).clamp(4.0, (w - 140.0).max(4.0));
                    let width = (w - x - 16.0).max(120.0);
                    let y = (b.y * h as f32) as f64;
                    let pos = dvec2(rect.pos.x + x, rect.pos.y + y); // relative to the frame
                    let margin = Inset { left: pos.x, top: pos.y, right: 0.0, bottom: 0.0 };
                    script_apply_eval!(cx, card, { width: #(width) margin: #(margin) });
                    card.label(cx, ids!(ov_text)).set_text(cx, line);
                    true
                }
                _ => false,
            };
            card.set_visible(cx, shown);
        }
        let list = self
            .cam
            .boxes
            .iter()
            .zip(self.cam.lines.iter())
            .map(|(b, l)| format!("{}\n{}", b.text, l))
            .collect::<Vec<_>>()
            .join("\n\n");
        let _ = list;
        for i in 0..12usize {
            let row = self.view.widget(cx, &[live_id!(camera), live_id!(cam_list), LiveId::from_str(&format!("l{i}"))]);
            match (self.cam.boxes.get(i), self.cam.lines.get(i)) {
                (Some(b), Some(line)) if !line.is_empty() && b.w <= 0.0 => {
                    row.label(cx, ids!(trans)).set_text(cx, line);
                    row.label(cx, ids!(orig)).set_text(cx, &b.text);
                    row.set_visible(cx, true);
                }
                _ => row.set_visible(cx, false),
            }
        }
        self.view.redraw(cx);
    }

    // --------------------------------------------------------------- actions

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if let Some(text) = self.view.text_input(cx, ids!(source_card.input)).changed(actions) {
            self.source_text = text;
            self.sync_chrome(cx);
            if let Some(t) = self.debounce.take() {
                cx.stop_timer(t);
            }
            self.debounce = Some(cx.start_timeout(DEBOUNCE_SECS));
        }
        if self.view.button(cx, ids!(settings.open_history)).clicked(actions) {
            self.view.view(cx, ids!(settings)).set_visible(cx, false);
            self.history_page = true;
            self.sync_chrome(cx);
        }
        if self.view.button(cx, ids!(main.history_panel.history_head.history_close)).clicked(actions) {
            self.history_page = false;
            self.refresh_models(cx);
            self.sync_chrome(cx);
        }
        if self.view.button(cx, ids!(camera.cam_overlay.cam_flip)).clicked(actions) {
            self.swap(cx);
        }
        if self.view.button(cx, ids!(camera.cam_overlay.cam_tab_text)).clicked(actions) {
            self.drop_text_focus(cx);
            self.close_camera_if_open(cx);
            self.sync_chrome(cx);
        }
        if self.view.button(cx, ids!(camera.cam_overlay.cam_tab_listen)).clicked(actions) {
            self.close_camera_if_open(cx);
            self.open_listen(cx);
        }
        if self.view.button(cx, ids!(langs.swap)).clicked(actions) {
            self.swap(cx);
        }
        if self.view.button(cx, ids!(source_card.head.clear)).clicked(actions) {
            self.source_text.clear();
            self.view.text_input(cx, ids!(source_card.input)).set_text(cx, "");
            self.start_translate(cx);
        }
        if self.view.button(cx, ids!(source_card.head.copy_src)).clicked(actions) {
            let text = self.source_text.clone();
            cx.copy_to_clipboard(&text);
        }
        if self.view.button(cx, ids!(target_card.head.copy_dst)).clicked(actions) {
            let text = self.view.text_input(cx, ids!(target_card.output)).text();
            cx.copy_to_clipboard(&text);
        }
        self.handle_talk_actions(cx, actions);
        self.handle_models_actions(cx, actions);
        // The mode switch. Leaving Camera stops it and cancels its job; leaving
        // Voice stops listening.
        for tab in [ids!(nav.tab_text), ids!(nav.tab_cam), ids!(nav.tab_listen)] {
            if self.view.button(cx, tab).clicked(actions) {
                self.drop_text_focus(cx);
            }
        }
        if self.view.button(cx, ids!(nav.tab_text)).clicked(actions) {
            self.close_camera_if_open(cx);
            if self.talk.open {
                self.close_talk(cx);
            }
            self.history_page = false;
            self.sync_chrome(cx);
        }
        // Listen: opens idle; the record tap starts listening, leaving stops it.
        if self.view.button(cx, ids!(nav.tab_listen)).clicked(actions) && !self.talk.open {
            self.close_camera_if_open(cx);
            self.history_page = false;
            self.open_listen(cx);
        }
        if self.view.button(cx, ids!(nav.tab_cam)).clicked(actions) && !self.cam.open {
            if self.talk.open {
                self.close_talk(cx);
            }
            self.history_page = false;
            self.open_camera(cx);
        }
        if self.view.button(cx, ids!(camera.cam_bar.cam_device)).clicked(actions) {
            self.next_device(cx);
        }
        if self.view.button(cx, ids!(camera.shutter_row.shutter)).clicked(actions) {
            self.shutter(cx);
            self.sync_chrome(cx);
        }
        // Retake: back to the live view at once (shutter() on a frozen picture).
        if self.view.button(cx, ids!(camera.shutter_row.retake)).clicked(actions) && self.cam.frozen {
            self.shutter(cx);
            self.sync_chrome(cx);
        }
        if self.view.button(cx, ids!(settings.pair_row.invite_btn)).clicked(actions) {
            self.show_invite(cx);
        }
        if self.view.button(cx, ids!(settings.pair_row.connect_btn)).clicked(actions) {
            self.connect_invite(cx);
        }
        if self.view.button(cx, ids!(settings.pair_row.copy_invite)).clicked(actions) {
            let text = self.view.text_input(cx, ids!(settings.invite)).text();
            cx.copy_to_clipboard(&text);
        }
        if self.view.button(cx, ids!(bar.settings_btn)).clicked(actions) {
            let shown = self.view.view(cx, ids!(settings)).visible();
            self.view.view(cx, ids!(settings)).set_visible(cx, !shown);
            self.refresh_models(cx);
            self.sync_chrome(cx);
        }
        if let Some(on) = self.view.check_box(cx, ids!(options.pinyin_on)).changed(actions) {
            self.saved.pinyin = on;
            self.persist(cx);
            let have = self.current.as_ref().is_some_and(|c| c.to == Lang::Zh && !c.target.is_empty());
            if on && have && self.pinyin_text.is_empty() {
                self.start_pinyin(cx);
            }
            self.sync_chrome(cx);
        }
        if let Some(on) = self.view.check_box(cx, ids!(options.traditional_on)).changed(actions) {
            self.saved.traditional = on;
            self.persist(cx);
            self.start_translate(cx);
        }
        if self.view.button(cx, ids!(settings.hub_link)).clicked(actions) {
            let open = self.view.view(cx, ids!(settings.hub_section)).visible();
            self.view.view(cx, ids!(settings.hub_section)).set_visible(cx, !open);
            self.view.redraw(cx);
        }
        if self.view.button(cx, ids!(settings.run_on.run_phone)).clicked(actions) {
            self.set_run_on_phone(cx, true);
        }
        if self.view.button(cx, ids!(settings.run_on.run_hub)).clicked(actions) {
            self.set_run_on_phone(cx, false);
        }
        let mut settings_changed = false;
        if let Some(t) = self.view.text_input(cx, ids!(settings.hub)).changed(actions) {
            self.saved.hub = t;
            settings_changed = true;
        }
        if let Some(t) = self.view.text_input(cx, ids!(settings.model)).changed(actions) {
            self.saved.model = t;
            settings_changed = true;
        }
        if let Some(t) = self.view.text_input(cx, ids!(settings.glossary)).changed(actions) {
            self.saved.glossary = t;
            settings_changed = true;
        }
        if settings_changed {
            self.persist(cx);
            self.hub_ok = None;
            self.probe_id = 0;
            self.probe(cx);
        }
        if self.view.button(cx, ids!(main.history_panel.history_head.only_fav)).clicked(actions) {
            self.favourites_only = !self.favourites_only;
            self.rebuild_rows(cx);
        }
        if self.view.button(cx, ids!(main.history_panel.history_head.clear_history)).clicked(actions) {
            self.saved.history.retain(|e| e.favourite);
            self.persist(cx);
            self.rebuild_rows(cx);
        }
        self.handle_row_clicks(cx, actions);
    }

    fn handle_row_clicks(&mut self, cx: &mut Cx, actions: &Actions) {
        let mut open: Option<Entry> = None;
        let mut star: Option<u64> = None;
        if let Some(mut list) = self.view.widget(cx, ids!(main.history_panel.rows)).borrow_mut::<PortalList>() {
            for (index, entry) in self.rows.iter().enumerate() {
                let item = list.item(cx, index, live_id!(Row));
                if item.button(cx, ids!(open.hit)).clicked(actions) {
                    open = Some(entry.clone());
                    break;
                }
                if item.button(cx, ids!(fav)).clicked(actions) {
                    star = Some(entry.id);
                    break;
                }
            }
        }
        if let Some(entry) = open {
            self.open_entry(cx, &entry);
        }
        if let Some(id) = star {
            if let Some(e) = self.saved.history.iter_mut().find(|e| e.id == id) {
                e.favourite = !e.favourite;
            }
            self.persist(cx);
            self.rebuild_rows(cx);
        }
    }

    /// No text field keeps the keyboard: a mode switch or a touch elsewhere ends typing.
    fn drop_text_focus(&mut self, cx: &mut Cx) {
        cx.set_key_focus(Area::Empty);
        cx.hide_text_ime();
    }

    /// Swap the directions: the translation becomes the text to translate.
    fn swap(&mut self, cx: &mut Cx) {
        let from = self.effective_from();
        let target = self.view.text_input(cx, ids!(target_card.output)).text();
        let new_from = from.other();
        self.pick = Some(new_from);
        if !target.trim().is_empty() {
            self.source_text = target.clone();
            self.view.text_input(cx, ids!(source_card.input)).set_text(cx, &target);
        }
        self.sync_chrome(cx);
        self.start_translate(cx);
    }
}

impl Widget for TranslatorView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.ensure_started(cx);
        // A style reload (LiveEdit: the safe-area insets arriving at start, a
        // new system sheet) re-applies the DSL, which puts the panes and the
        // camera back to their written layout: lay them out again for the real
        // size (a portrait start kept the side-by-side boxes until a turn).
        if matches!(event, Event::LiveEdit) {
            self.last_size = Vec2d::default();
            self.cam_landscape = None;
            self.view.redraw(cx);
        }
        // A touch outside every text field closes the keyboard.
        if let Event::TouchUpdate(te) = event {
            if self.keyboard_up {
                let down = te.touches.iter().find(|t| t.state == makepad_widgets::makepad_platform::event::TouchState::Start).map(|t| t.abs);
                if let Some(abs) = down {
                    let fields: [&[LiveId]; 5] = [ids!(source_card.input), ids!(settings.hub), ids!(settings.model), ids!(settings.invite), ids!(settings.glossary)];
                    let inside = fields.iter().any(|f| {
                        let w = self.view.widget(cx, *f);
                        w.visible() && w.area().rect(cx).contains(abs)
                    });
                    if !inside {
                        self.drop_text_focus(cx);
                    }
                }
            }
        }
        if let Event::Storage(responses) = event {
            self.on_storage(cx, responses);
        }
        if self.activity_frame.is_event(event).is_some() || matches!(event, Event::Signal) {
            self.sync_activity(cx);
        }
        self.on_camera_event(cx, event);
        self.on_talk_event(cx, event);
        self.on_models_event(cx, event);
        if self.probe_timer.as_ref().is_some_and(|t| t.is_event(event).is_some()) {
            self.probe(cx);
        }
        if self.debounce.as_ref().is_some_and(|t| t.is_event(event).is_some()) {
            self.debounce = None;
            self.start_translate(cx);
        }
        let updates = self.engine.as_ref().map(|e| e.poll()).unwrap_or_default();
        for update in updates {
            self.on_update(cx, update);
        }
        self.view.handle_event(cx, event, scope);
        if let Event::Actions(actions) = event {
            self.handle_actions(cx, actions);
            self.sync_activity(cx);
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.ensure_started(cx);
        let size = cx.turtle().rect().size;
        if size.x >= 1.0 && ((size.x - self.last_size.x).abs() > 0.5) {
            let first = self.last_size.x <= 0.0;
            self.last_size = size;
            if first {
                self.last_size = size;
                self.compact = !(size.x < COMPACT_WIDTH);
            }
            self.apply_layout(cx, size);
            self.apply_camera_layout(cx);
        }
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = step.as_portal_list().borrow_mut() {
                self.draw_rows(cx, &mut list);
            }
        }
        // The camera frame changed size (the result list came or went): refit the
        // photo and its overlays on the next frame.
        if self.cam.open {
            let size = self.view.widget(cx, ids!(camera.frame)).area().rect(cx).size;
            if size != self.cam.frame_size {
                self.cam.frame_size = size;
                self.fit_picture(cx);
                if !self.cam.lines.is_empty() {
                    self.place_overlays(cx);
                }
                self.view.redraw(cx);
            }
        }
        DrawStep::done()
    }
}
