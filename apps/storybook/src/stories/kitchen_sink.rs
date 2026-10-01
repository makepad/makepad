//! The kitchen sink: every common control on one screen, and the same
//! controls held still in each of their states.
//!
//! Both pages use the library's templates under their own names and dress
//! nothing, so whatever a style sheet does to a widget is what shows here.
//! They carry no prose, only a caption over each group, so one grab of the
//! canvas at the catalogue's own window size holds the whole page.
use crate::makepad_widgets::*;
use crate::registry::Story;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.storybook.*

    mod.storybook.SinkStillViewBase = #(SinkStillView::register_widget(vm))
    /** A view that passes no event to anything inside it, so whatever state its children were built in is the state they keep. */
    mod.storybook.SinkStillView = set_type_default() do mod.storybook.SinkStillViewBase{
        width: Fill
        height: Fit
        flow: Down
    }

    // The page itself: no scrolling wanted, and a tighter edge than a
    // documentation page, because every point of the canvas is spent.
    let SinkPage = StoryPage{
        padding: theme.mspace_2
        spacing: theme.space_3
    }

    let GroupCaption = Label{
        draw_text +: {text_style: theme.font_bold{} color: theme.color_on_surface_variant}
    }

    // One group: its caption over its controls.
    let Group = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_2
    }

    // A band of groups side by side, tops aligned.
    let Band = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_3
        align: Align{x: 0. y: 0.}
    }

    let Column = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_3
    }

    let Line = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0. y: 0.5}
    }

    let Stack = View{
        width: Fill
        height: Fit
        flow: Down
        spacing: theme.space_1
    }

    let KnobCell = View{
        width: Fit
        height: Fit
        flow: Down
        align: Align{x: 0.5 y: 0.}
    }

    mod.stories.KitchenSinkOverview = SinkPage{
        Group{
            GroupCaption{text: "Buttons"}
            View{
                width: Fill
                height: Fit
                flow: Flow.Right{wrap: true, row_align: RowAlign.Center}
                spacing: theme.space_2
                // A wrapped row stands as far under the row above as its
                // buttons stand apart, so no shadow lands on a neighbour.
                wrap_spacing: theme.space_2
                Button{text: "Default"}
                ButtonPrimary{text: "Primary"}
                ButtonSecondary{text: "Secondary"}
                ButtonTertiary{text: "Tertiary"}
                ButtonOutline{text: "Outline"}
                ButtonDanger{text: "Delete"}
                ButtonFlat{text: "Flat"}
                Button{text: "Favourite" draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
                ButtonIcon{draw_icon +: {svg: crate_resource("self:resources/mark_star.svg") color: theme.color_label_inner}}
                ButtonSm{text: "Small"}
                ButtonLg{text: "Large"}
                Button{text: "Disabled" animator +: {disabled: {default: @on}}}
            }
        }

        // Three columns of two groups each, weighed so that no column
        // runs much longer than the others under a sheet with large metrics.
        Band{
            Column{
                Group{
                    GroupCaption{text: "Selection"}
                    Line{
                        align: Align{x: 0. y: 0.}
                        Stack{
                            CheckBox{text: "Snap" active: true}
                            CheckBox{text: "Grid"}
                            CheckBox{text: "All layers" state: CheckState.Mixed}
                            CheckBox{text: "Locked" animator +: {disabled: {default: @on}}}
                        }
                        Stack{
                            Toggle{text: "Sync" active: true}
                            Toggle{text: "Loop"}
                            Toggle{text: "Remote" animator +: {disabled: {default: @on}}}
                        }
                    }
                    mod.widgets.RadioGroupRow{
                        RadioButton{text: "Low"}
                        RadioButton{text: "Mid"}
                        RadioButton{text: "High"}
                    }
                }
                Group{
                    GroupCaption{text: "Text"}
                    TextInput{width: Fill empty_text: "Name"}
                    TextInput{width: Fill text: "Studio monitor"}
                    TextInput{width: Fill text: "Read only" animator +: {disabled: {default: @on}}}
                    NumberField{width: Fill min: 0.0 max: 99.0 step: 1.0}
                }
            }
            Column{
                Group{
                    GroupCaption{text: "Sliders"}
                    Slider{width: Fill text: "Input" default: 0.15}
                    Slider{width: Fill text: "Mix" default: 0.5}
                    Slider{width: Fill text: "Output" default: 0.85}
                    SliderMinimal{width: Fill text: "Minimal" default: 0.4}
                    SliderRound{width: Fill text: "Round" default: 0.65}
                }
                Group{
                    GroupCaption{text: "Feedback"}
                    ProgressBar{width: Fill value: 0.35}
                    ProgressBar{width: Fill value: 0.8}
                    ProgressBar{width: Fill value: -1.0}
                    LevelMeter{width: Fill level: 0.55}
                    Line{
                        SpinnerFlat{}
                        Badge{count: 12}
                        Chip{text: "Filter"}
                    }
                }
            }
            Column{
                Group{
                    GroupCaption{text: "Knobs"}
                    Line{
                        align: Align{x: 0. y: 0.}
                        KnobCell{RotaryKnob{text: "LOW" default: 0.2}}
                        KnobCell{RotaryKnob{text: "MID" default: 0.5}}
                        KnobCell{RotaryKnob{text: "HIGH" default: 0.9}}
                    }
                    Line{
                        align: Align{x: 0. y: 1.}
                        SliderFaderY{height: 100. default: 0.7}
                        LevelMeterColumn{height: 100. level: 0.62}
                        Rotary{width: 80. height: 105. text: "Dial" default: 0.35}
                    }
                }
                Group{
                    GroupCaption{text: "Choice"}
                    DropDown{labels: ["Stereo" "Mono" "Surround"]}
                    ComboBox{width: Fill labels: ["Amber" "Azure" "Cobalt" "Coral"]}
                    SegmentedControl{options: ["Day" "Week" "Month"] selected: 1}
                    Tabs{width: Fill labels: ["Mixer" "Effects" "Routing"]}
                }
            }
        }

        Group{
            GroupCaption{text: "Surfaces"}
            Band{
                PanelView{
                    width: Fill
                    height: 100.
                    flow: Down
                    padding: theme.mspace_3
                    spacing: theme.space_1
                    GroupCaption{text: "Panel"}
                    Label{text: "A raised surface."}
                }
                InsetPanelView{
                    width: Fill
                    height: 100.
                    flow: Down
                    padding: theme.mspace_3
                    spacing: theme.space_1
                    GroupCaption{text: "Inset"}
                    Label{text: "A sunken well."}
                }
                Card{
                    width: Fill
                    header: CardHeader{H4{text: "Card"}}
                    body: CardBody{Label{text: "Title and body."}}
                }
                View{
                    width: Fill
                    height: 100.
                    ScrollYView{
                        width: Fill
                        height: Fill
                        flow: Down
                        // Held on screen: the stock bar fades out at rest,
                        // and a bar nobody can see is a bar no sheet is
                        // judged on.
                        scroll_bars +: {scroll_bar_y +: {auto_hide: false}}
                        ListItemOne{text: "Kick"}
                        ListItemOne{text: "Snare" selected: true}
                        ListItemOne{text: "Hats"}
                        ListItemOne{text: "Bass"}
                        ListItemOne{text: "Keys"}
                        ListItemOne{text: "Vocals"}
                    }
                }
            }
        }
    }

    let ScreenCaption = Label{
        draw_text +: {color: theme.color_screen_ink text_style: theme.font_regular{font_size: theme.font_size_p * 0.8}}
    }

    // A readout with its unit beside it, bottoms aligned, on one screen.
    let ScreenLine = View{
        width: Fit
        height: Fit
        flow: Right
        spacing: theme.space_1
        align: Align{x: 0. y: 1.}
    }

    // Each lamp in a cell of its own size, so the grid is the same under a
    // sheet that draws a halo and one that draws none (and so takes no room
    // for one).
    let SinkLamp = Lamp{width: 24. height: 24.}
    let SinkBar = LampBar{width: 40. height: 24.}

    let LampColumn = View{
        width: Fit
        height: Fit
        flow: Down
        spacing: theme.space_1
        align: Align{x: 0.5 y: 0.}
    }

    mod.stories.KitchenSinkInstruments = SinkPage{
        Band{
            Column{
                Group{
                    GroupCaption{text: "Readouts"}
                    ScreenView{
                        ScreenLine{Readout{text: "12:45:30" digit_height: 20.} ScreenCaption{text: "TIME"}}
                    }
                    ScreenView{
                        ScreenLine{Readout{text: "-6.5" cells: 5 digit_height: 20.} ScreenCaption{text: "dB"}}
                    }
                    ScreenView{
                        ScreenLine{Readout{text: "440.0" cells: 6 digit_height: 20.} ScreenCaption{text: "Hz"}}
                    }
                }
                Group{
                    GroupCaption{text: "Screen"}
                    ScreenView{
                        width: 190.
                        ScreenCaption{text: "OUTPUT"}
                        Readout{text: "-12.0" cells: 5 digit_height: 26.}
                        LevelMeter{width: Fill height: 6. lamp: false level: 0.68 draw_bg.segment: 4.}
                        LevelMeter{width: Fill height: 6. lamp: false level: 0.54 draw_bg.segment: 4.}
                    }
                }
            }
            Column{
                Group{
                    GroupCaption{text: "Lamps"}
                    Line{
                        align: Align{x: 0. y: 0.}
                        LampColumn{SinkLamp{intent: LampIntent.Accent lit: 1.0} SinkLamp{intent: LampIntent.Accent lit: 0.5} SinkLamp{intent: LampIntent.Accent}}
                        LampColumn{SinkLamp{intent: LampIntent.Success lit: 1.0} SinkLamp{intent: LampIntent.Success lit: 0.5} SinkLamp{intent: LampIntent.Success}}
                        LampColumn{SinkLamp{intent: LampIntent.Warning lit: 1.0} SinkLamp{intent: LampIntent.Warning lit: 0.5} SinkLamp{intent: LampIntent.Warning}}
                        LampColumn{SinkLamp{intent: LampIntent.Error lit: 1.0} SinkLamp{intent: LampIntent.Error lit: 0.5} SinkLamp{intent: LampIntent.Error}}
                        LampColumn{SinkLamp{intent: LampIntent.Plain lit: 1.0} SinkLamp{intent: LampIntent.Plain lit: 0.5} SinkLamp{intent: LampIntent.Plain}}
                        LampColumn{SinkBar{intent: LampIntent.Success lit: 1.0} SinkBar{intent: LampIntent.Warning lit: 0.5} SinkBar{intent: LampIntent.Error}}
                    }
                }
                Group{
                    GroupCaption{text: "Meter"}
                    NeedleMeter{value: 0.66 label: "VU"}
                }
            }
            Column{
                Group{
                    GroupCaption{text: "Switches"}
                    Line{
                        align: Align{x: 0. y: 0.}
                        Stack{
                            width: Fit
                            ToggleRocker{text: "Power" active: true}
                            ToggleRocker{text: "Mute"}
                        }
                        Stack{
                            width: Fit
                            ToggleSlide{text: "Link" active: true}
                            ToggleSlide{text: "Solo"}
                        }
                    }
                }
                Group{
                    GroupCaption{text: "Faders"}
                    Line{
                        align: Align{x: 0. y: 1.}
                        SliderFaderY{height: 110. default: 0.72}
                        SliderFaderY{height: 110. default: 0.45}
                        SliderFaderY{height: 110. default: 0.6}
                        SliderFaderY{height: 110. default: 0.3}
                    }
                }
                // A big knob and a small one, so a sheet's knob is seen at
                // the sizes a panel gives it.
                Group{
                    GroupCaption{text: "Knobs"}
                    Line{
                        align: Align{x: 0. y: 1.}
                        Rotary{width: 140. height: 165. text: "Gain" default: 0.6}
                        Rotary{width: 44. height: 69. text: "Pan" default: 0.4}
                    }
                }
            }
        }

        Band{
            Column{
                Group{
                    GroupCaption{text: "Inputs"}
                    RangeSlider{width: Fill text: "Band" min: 20.0 max: 20000.0 step: 10.0 default_start: 200.0 default_end: 5000.0 unit: " Hz" precision: 0}
                    NumberField{width: Fill min: 0.0 max: 99.0 step: 1.0}
                    SegmentedControl{options: ["Mono" "Stereo" "Wide"] selected: 1}
                }
            }
            Column{
                Group{
                    GroupCaption{text: "List"}
                    View{
                        width: Fill
                        height: 104.
                        ScrollYView{
                            width: Fill
                            height: Fill
                            flow: Down
                            scroll_bars +: {scroll_bar_y +: {auto_hide: false}}
                            ListItemOne{text: "Drums"}
                            ListItemOne{text: "Bass" selected: true}
                            ListItemOne{text: "Keys"}
                            ListItemOne{text: "Guitar"}
                            ListItemOne{text: "Vocals"}
                        }
                    }
                }
            }
        }

        // Three surfaces set on the window's ground with room between
        // them, so a sheet's panel texture and its ground both show.
        Group{
            GroupCaption{text: "Grounds"}
            Band{
                spacing: theme.space_6
                PanelView{
                    width: Fill
                    height: 70.
                    flow: Down
                    padding: theme.mspace_3
                    GroupCaption{text: "Panel"}
                }
                InsetPanelView{
                    width: Fill
                    height: 70.
                    flow: Down
                    padding: theme.mspace_3
                    GroupCaption{text: "Inset"}
                }
                RoundedView{
                    width: Fill
                    height: 70.
                    draw_bg +: {color: theme.color_surface_container}
                    flow: Down
                    padding: theme.mspace_3
                    GroupCaption{text: "Rounded"}
                }
            }
        }
    }

    // The matrix: a name column, then one column per state.
    let StateLine = View{
        width: Fill
        height: Fit
        flow: Right
        spacing: theme.space_2
        align: Align{x: 0.0 y: 0.5}
    }

    let StateName = Label{
        width: 88.
        draw_text +: {text_style: theme.font_bold{} color: theme.color_on_surface_variant}
    }

    let StateHead = Label{
        draw_text +: {text_style: theme.font_bold{} color: theme.color_on_surface_variant}
    }

    // Fit, not a fixed height: a sheet that grows a control's quad for its
    // shadow would have the shadow cut off at the edge of a fixed cell.
    let StateCell = View{
        width: Fill
        height: Fit
        align: Align{x: 0.0 y: 0.5}
    }

    let NoState = Label{
        text: "-"
        draw_text +: {color: theme.color_on_surface_variant}
    }

    mod.stories.KitchenSinkStates = SinkPage{
        mod.storybook.SinkStillView{
            spacing: theme.space_3
            StateLine{
                StateName{text: ""}
                StateCell{StateHead{text: "rest"}}
                StateCell{StateHead{text: "hover"}}
                StateCell{StateHead{text: "pressed"}}
                StateCell{StateHead{text: "focus"}}
                StateCell{StateHead{text: "active / on"}}
                StateCell{StateHead{text: "disabled"}}
            }
            StateLine{
                StateName{text: "Button"}
                StateCell{Button{text: "Save"}}
                StateCell{Button{text: "Save" animator +: {hover: {default: @on}}}}
                StateCell{Button{text: "Save" animator +: {hover: {default: @down}}}}
                StateCell{Button{text: "Save" animator +: {focus: {default: @on}}}}
                StateCell{NoState{}}
                StateCell{Button{text: "Save" animator +: {disabled: {default: @on}}}}
            }
            StateLine{
                StateName{text: "Toggle"}
                StateCell{Toggle{text: "Sync"}}
                StateCell{Toggle{text: "Sync" animator +: {hover: {default: @on}}}}
                StateCell{Toggle{text: "Sync" animator +: {hover: {default: @down}}}}
                StateCell{Toggle{text: "Sync" animator +: {focus: {default: @on}}}}
                StateCell{Toggle{text: "Sync" active: true}}
                StateCell{Toggle{text: "Sync" animator +: {disabled: {default: @on}}}}
            }
            StateLine{
                StateName{text: "Check box"}
                StateCell{CheckBox{text: "Snap"}}
                StateCell{CheckBox{text: "Snap" animator +: {hover: {default: @on}}}}
                StateCell{CheckBox{text: "Snap" animator +: {hover: {default: @down}}}}
                StateCell{CheckBox{text: "Snap" animator +: {focus: {default: @on}}}}
                StateCell{CheckBox{text: "Snap" active: true}}
                StateCell{CheckBox{text: "Snap" animator +: {disabled: {default: @on}}}}
            }
            StateLine{
                StateName{text: "Radio"}
                StateCell{RadioButton{text: "Mono"}}
                StateCell{RadioButton{text: "Mono" animator +: {hover: {default: @on}}}}
                StateCell{RadioButton{text: "Mono" animator +: {hover: {default: @down}}}}
                StateCell{RadioButton{text: "Mono" animator +: {focus: {default: @on}}}}
                StateCell{RadioButton{text: "Mono" animator +: {active: {default: @on}}}}
                StateCell{RadioButton{text: "Mono" animator +: {disabled: {default: @on}}}}
            }
            StateLine{
                StateName{text: "Slider"}
                StateCell{Slider{width: Fill text: "Mix" default: 0.5}}
                StateCell{Slider{width: Fill text: "Mix" default: 0.5 animator +: {hover: {default: @on}}}}
                StateCell{Slider{width: Fill text: "Mix" default: 0.5 animator +: {hover: {default: @on} drag: {default: @on}}}}
                StateCell{Slider{width: Fill text: "Mix" default: 0.5 animator +: {focus: {default: @on}}}}
                StateCell{NoState{}}
                StateCell{Slider{width: Fill text: "Mix" default: 0.5 animator +: {disabled: {default: @on}}}}
            }
            StateLine{
                StateName{text: "Text input"}
                StateCell{TextInput{width: Fill text: "Name"}}
                StateCell{TextInput{width: Fill text: "Name" animator +: {hover: {default: @on}}}}
                StateCell{TextInput{width: Fill text: "Name" animator +: {hover: {default: @down}}}}
                StateCell{TextInput{width: Fill text: "Name" animator +: {focus: {default: @on}}}}
                StateCell{NoState{}}
                StateCell{TextInput{width: Fill text: "Name" animator +: {disabled: {default: @on}}}}
            }
            StateLine{
                StateName{text: "Drop-down"}
                StateCell{DropDown{width: Fill labels: ["Stereo" "Mono"]}}
                StateCell{DropDown{width: Fill labels: ["Stereo" "Mono"] animator +: {hover: {default: @on}}}}
                StateCell{DropDown{width: Fill labels: ["Stereo" "Mono"] animator +: {hover: {default: @down}}}}
                StateCell{DropDown{width: Fill labels: ["Stereo" "Mono"] animator +: {focus: {default: @on}}}}
                StateCell{NoState{}}
                StateCell{DropDown{width: Fill labels: ["Stereo" "Mono"] animator +: {disabled: {default: @on}}}}
            }
            StateLine{
                StateName{text: "Tab"}
                StateCell{RadioButtonTab{text: "Mixer"}}
                StateCell{RadioButtonTab{text: "Mixer" animator +: {hover: {default: @on}}}}
                StateCell{RadioButtonTab{text: "Mixer" animator +: {hover: {default: @down}}}}
                StateCell{RadioButtonTab{text: "Mixer" animator +: {focus: {default: @on}}}}
                StateCell{RadioButtonTab{text: "Mixer" animator +: {active: {default: @on}}}}
                StateCell{RadioButtonTab{text: "Mixer" animator +: {disabled: {default: @on}}}}
            }
        }
    }
}

/// A view that hands no event to its children. The state matrix builds each
/// control in the state it shows, and a pointer passing over one would play
/// that control's hover off and leave the cell showing rest. The same view
/// as the foundations page's: story files may not lean on each other.
#[derive(Script, ScriptHook, Widget)]
pub struct SinkStillView {
    #[deref]
    view: View,
}

impl Widget for SinkStillView {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        self.view.draw_walk(cx, scope, walk)
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

pub const STORIES: &[Story] = &[
    Story {
        key: "overview/kitchen-sink/overview",
        category: "Overview",
        component: "Kitchen sink",
        also: &[],
        name: "Overview",
        dsl: "KitchenSinkOverview",
        added: "2025-06-01",
        tags: &["style sheet review", "every control"],
        doc: "# Kitchen sink\n\nEvery common control on one screen, each under its stock name and dressed by nothing but the style sheet, so one grab shows what a sheet does to all of them.",
        subject: "",
        feature: None,
        controls: &[],
        on_actions: None,
    },
    Story {
        key: "overview/kitchen-sink/states",
        category: "Overview",
        component: "Kitchen sink",
        also: &[],
        name: "States",
        dsl: "KitchenSinkStates",
        added: "2025-06-01",
        tags: &["style sheet review", "state matrix"],
        doc: "# States\n\nEight controls held still in rest, hover, pressed, focus, active and disabled. The matrix passes no events to what is inside it, so the pointer cannot move a cell out of its state. A dash marks a state the control does not have.",
        subject: "",
        feature: None,
        controls: &[],
        on_actions: None,
    },
    Story {
        key: "overview/kitchen-sink/instruments",
        category: "Overview",
        component: "Kitchen sink",
        also: &[],
        name: "Instruments",
        dsl: "KitchenSinkInstruments",
        added: "2026-09-28",
        tags: &["style sheet review", "displays and meters"],
        doc: "# Instruments\n\nThe displays, lamps, meters and hardware controls on one screen, each under its stock name and dressed by nothing but the style sheet: readouts of a time, a level and a frequency, a screen holding a readout and a level ladder, lamps lit, half lit and out in every intent, a needle meter, rocker and slide switches on and off, a bank of four faders, a big knob and a small one, a range slider, a number field, a segmented group, a list with a selected row, and three surfaces standing on the window's ground.",
        subject: "",
        feature: None,
        controls: &[],
        on_actions: None,
    },
];
