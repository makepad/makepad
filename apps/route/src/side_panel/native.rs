use makepad_map_nav::search::SearchResult;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.TranscriptListBase = #(crate::TranscriptList::register_widget(vm))

    let PanelText = Label{
        width: Fill
        draw_text +: {
            color: #x22303c
            text_style: theme.font_regular{font_size: 9}
        }
    }

    let AppButton = Button{
        draw_text +: {
            color: #x223038
            color_hover: #x000000
            color_focus: #x223038
            color_down: #x000000
            text_style: theme.font_regular{font_size: 12}
        }
    }

    mod.widgets.RouteSidePanel = View{
        width: Fill
        height: Fill
        flow: Down
        align: Align{x: 1.0 y: 1.0}
        assistant_panel := mod.widgets.glass.Panel{
            visible: false
            flow: Down
            width: 380
            height: 620
            margin: Inset{right: 14, bottom: 6}
            padding: 10
            spacing: 0
            draw_bg +: {
                corner_radius: 9.0
                tint_color: #xf8fbff
                tint_alpha: 0.30
            }
            header_label := Label{
                draw_text +: {
                    color: #x223038
                    text_style: theme.font_bold{font_size: 11}
                }
                text: "Route Assistant"
            }
            intro_label := PanelText{
                height: Fit
                margin: Inset{top: 8}
                text: "Ask about a trip: destinations, stops, sights and chargers along the way, rain at a point. Type /help for direct tool commands."
            }
            transcript_list := mod.widgets.TranscriptListBase{
                width: Fill
                height: Fill
                margin: Inset{top: 8, bottom: 8}
                list := PortalList{
                    width: Fill
                    height: Fill
                    UserLine := View{
                        width: Fill
                        height: Fit
                        margin: Inset{top: 8}
                        line_label := Label{
                            width: Fill
                            draw_text +: {
                                color: #x101820
                                text_style: theme.font_bold{font_size: 9.5}
                            }
                        }
                    }
                    AssistantLine := View{
                        width: Fill
                        height: Fit
                        margin: Inset{top: 4}
                        line_label := Label{
                            width: Fill
                            draw_text +: {
                                color: #x2a3540
                                text_style: theme.font_regular{font_size: 9.5}
                            }
                        }
                    }
                    ToolLine := View{
                        width: Fill
                        height: Fit
                        margin: Inset{top: 3, left: 8}
                        line_label := Label{
                            width: Fill
                            draw_text +: {
                                color: #x7a8794
                                text_style: theme.font_regular{font_size: 8.5}
                            }
                        }
                    }
                    InfoLine := View{
                        width: Fill
                        height: Fit
                        margin: Inset{top: 3, left: 8}
                        line_label := Label{
                            width: Fill
                            draw_text +: {
                                color: #x93a0ad
                                text_style: theme.font_regular{font_size: 8.5}
                            }
                        }
                    }
                    TripLine := View{
                        width: Fill
                        height: Fit
                        flow: Right
                        spacing: 6
                        margin: Inset{top: 8}
                        apply_btn := Button{text: ">"}
                        line_label := Label{
                            width: Fill
                            margin: Inset{top: 4}
                            draw_text +: {
                                color: #x1d4ed8
                                text_style: theme.font_bold{font_size: 9.5}
                            }
                        }
                    }
                }
            }
            images_row := View{
                visible: false
                width: Fill
                height: Fit
                flow: Right
                spacing: 4
                margin: Inset{bottom: 6}
                img_0 := Image{width: 86, height: 64}
                img_1 := Image{width: 86, height: 64}
                img_2 := Image{width: 86, height: 64}
                img_3 := Image{width: 86, height: 64}
            }
            View{
                width: Fill
                height: Fit
                flow: Right
                spacing: 6
                align: Align{x: 0.0 y: 0.5}
                mic_button := AppButton{
                    padding: Inset{left: 12, right: 12, top: 8, bottom: 8}
                    text: "🎤"
                }
                speaker_button := AppButton{
                    padding: Inset{left: 12, right: 12, top: 8, bottom: 8}
                    text: "🔊"
                }
                mic_wave := VoiceWave{width: 0, height: 0}
                prompt_input := TextInput{
                    width: Fill
                    empty_text: "Plan a trip…"
                    draw_text +: {
                        color: #x16202a
                        color_hover: #x000000
                        color_focus: #x0b1218
                        color_down: #x000000
                        color_empty: #x6b7784
                        color_empty_hover: #x57626e
                        color_empty_focus: #x6b7784
                    }
                }
            }
            status_label := PanelText{
                margin: Inset{top: 6, left: 2}
                text: "starting…"
            }
        }
        assistant_button := AppButton{
            margin: Inset{right: 14, bottom: 16}
            padding: Inset{left: 16, right: 16, top: 12, bottom: 12}
            spacing: 0
            text: ""
            icon_walk: Walk{width: 18, height: 18}
            draw_icon +: {
                svg: crate_resource("self://resources/icons/assistant.svg")
                color: #x223038
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlongKind {
    Chargers,
    Museums,
}

#[derive(Debug, PartialEq)]
pub enum PanelAction {
    Search(String),
    SelectResult(usize),
    RouteHere(usize),
    Along(AlongKind),
    Rain(bool),
    Wind(bool),
}

#[derive(Default)]
pub struct PanelController;

impl PanelController {
    pub fn result(&self, _index: usize) -> Option<&SearchResult> {
        None
    }

    pub fn set_results(&mut self, _cx: &mut Cx, _ui: &WidgetRef, _results: Vec<SearchResult>) {}
    pub fn set_search_status(&self, _cx: &mut Cx, _ui: &WidgetRef, _text: &str) {}
    pub fn set_along_status(&self, _cx: &mut Cx, _ui: &WidgetRef, _text: &str) {}
    pub fn set_weather(&self, _cx: &mut Cx, _ui: &WidgetRef, _text: &str) {}

    pub fn actions(&mut self, cx: &mut Cx, ui: &WidgetRef, actions: &Actions) -> Vec<PanelAction> {
        ui.text_input(cx, ids!(prompt_input))
            .returned(actions)
            .into_iter()
            .filter_map(|(text, _)| {
                let text = text.trim();
                (!text.is_empty()).then(|| PanelAction::Search(text.to_string()))
            })
            .collect()
    }
}
