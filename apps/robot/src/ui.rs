use makepad_widgets::*;
use crate::App;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Row = View{width: Fill height: Fit flow: Right spacing: 8 align: Align{y: 0.5}}
    let Column = View{width: Fill height: Fit flow: Down spacing: 10}
    let Rule = RectView{width: Fill height: 1 draw_bg.color: #x364047}
    let Info = Label{width: Fill height: Fit draw_text +: {color: #xc5d1d4 text_style: theme.font_regular{font_size: 11.5}}}
    let Title = Label{width: Fill height: Fit draw_text +: {color: #xf0f4f0 text_style: theme.font_bold{font_size: 18}}}
    let Caption = Label{height: Fit draw_text +: {color: #x95bcb9 text_style: theme.font_bold{font_size: 10}}}
    let Action = ButtonFlat{width: Fill height: 36 padding: Inset{left: 8 right: 8} draw_text.text_style.font_size: 11}
    let Field = TextInput{width: Fill height: 36 draw_text.text_style.font_size: 11}
    let Joint = SliderFlat{
        width: Fill height: 34 min: -360 max: 360 step: 0.5 precision: 1
        draw_bg +: {handle_color: #x86c5c0 handle_color_hover: #xb3e3d9 handle_color_drag: #xc3f1dc}
    }
    let Tabs = SegmentedControlFlat{
        segment_height: 32 segment_padding: 10
        draw_bg +: {color: #x172227 border_color: #x435458 radius: 14.0}
        draw_pill +: {color: #x385452 radius: 12.0}
        draw_text +: {color: #xb8c9ca text_style: theme.font_regular{font_size: 11}}
        draw_text_selected +: {color: #xeff7ef text_style: theme.font_bold{font_size: 11}}
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Robot · UFACTORY 850"
                window.inner_size: vec2(1170, 766)
                body +: {
                    flow: Down
                    RectView{
                        width: Fill height: 58 flow: Right align: Align{y: 0.5} spacing: 16
                        padding: Inset{left: 22 right: 18} draw_bg.color: #x1b2529
                        Label{text: "Robot" draw_text +: {color: #xf5f5e9 text_style: theme.font_bold{font_size: 21}}}
                        Caption{text: "UFACTORY 850"}
                        View{width: Fill height: 1}
                        link_badge := Caption{text: "Offline preview"}
                    }
                    Rule{}
                    View{
                        width: Fill height: Fill flow: Right
                        RectView{
                            width: Fill height: Fill flow: Down draw_bg.color: #x1a2329
                            View{
                                width: Fill height: 52 flow: Right align: Align{y: 0.5} padding: Inset{left: 18 right: 14}
                                Caption{text: "Drag to orbit  ·  Scroll to zoom"}
                                View{width: Fill height: 1}
                                camera_tabs := Tabs{options: ["Studio" "Front" "Top"]}
                            }
                            scene := RobotScene{width: Fill height: Fill}
                            transport := View{visible: false width: Fill height: Fit flow: Down padding: Inset{left: 14 right: 14 top: 8} spacing: 5
                                Row{
                                    replay_button := Action{width: 88 text: "Play"}
                                    timecode := Info{text: "0:00.000"}
                                    event_steps := Row{width: Fit visible: false
                                        previous_event := Action{width: 100 text: "Previous event"}
                                        next_event := Action{width: 90 text: "Next event"}
                                    }
                                    speed := DropDown{width: 76 labels: ["0.25×" "0.5×" "1×" "2×"] selected_item: 2}
                                }
                                session_track := SessionTrack{}
                            }
                            View{width: Fill height: 50 flow: Right align: Align{y: 0.5} padding: Inset{left: 18 right: 12} spacing: 8
                                point_label := Info{text: "Flange"}
                            }
                        }
                        RectView{width: 1 height: Fill draw_bg.color: #x394349}
                        RectView{
                            width: 348 height: Fill flow: Down padding: 16 spacing: 12 draw_bg.color: #x20292e
                            tabs := Tabs{segment_padding: 7 options: ["Pose" "Path" "Sessions" "Zones" "Controller"]}
                            Rule{}
                            ScrollYView{
                                width: Fill height: Fill flow: Down spacing: 12
                                pose_panel := Column{
                                    Title{text: "Joint pose"}
                                    pose_hint := Info{text: "Drag a joint to pose the preview."}
                                    sliders := Column{
                                        preset := DropDown{width: Fill height: 34 labels: ["Studio pose" "Extended" "Folded" "Custom pose"]}
                                        joint1 := Joint{text: "J1 · Base"}
                                        joint2 := Joint{text: "J2 · Shoulder" min: -132 max: 132}
                                        joint3 := Joint{text: "J3 · Elbow" min: -242 max: 3.5}
                                        joint4 := Joint{text: "J4 · Wrist roll"}
                                        joint5 := Joint{text: "J5 · Wrist pitch" min: -124 max: 124}
                                        joint6 := Joint{text: "J6 · Flange"}
                                    }
                                    readout := Info{visible: false}
                                }
                                path_panel := Column{
                                    visible: false
                                    Title{text: "Pose sequence"}
                                    Info{text: "Record a series of poses, then scrub or play the simulated path. Clearance covers the bare arm and a 12 mm margin."}
                                    sequence := Info{text: "No poses recorded."}
                                    Row{
                                        record_pose := Action{text: "Record pose"}
                                        remove_pose := Action{text: "Undo pose"}
                                    }
                                    preview_path := Action{text: "Play preview"}
                                    scrub := Slider{width: Fill height: 34 text: "Scrub path" min: 0 max: 100 precision: 0}
                                }
                                sessions_panel := Column{
                                    visible: false spacing: 6
                                    Title{text: "Hand guiding"}
                                    capture_hint := Info{text: "Connect in Controller and enter Manual Mode in UFACTORY Studio to record."}
                                    capture_take := Action{text: "Record hand-guiding"}
                                    example := Action{text: "Open example study"}
                                    Rule{}
                                    Caption{text: "RECORDED SESSION"}
                                    take_list := DropDown{width: Fill height: 34 labels: ["No recordings"]}
                                    session_info := Info{}
                                    Rule{}
                                    Caption{text: "CLEARANCE EVENTS"}
                                    event_summary := Info{}
                                    event_controls := Column{
                                        event_list := DropDown{width: Fill height: 34 labels: ["Choose an event"]}
                                    }
                                }
                                zones_panel := Column{
                                    visible: false
                                    Title{text: "Workspace zones"}
                                    Info{text: "Enter opposite corners in base coordinates (mm), or capture the tool tip in Manual Mode. Zones apply to the 3D preview."}
                                    zone_kind := Tabs{options: ["Keep-out" "Enclosure"]}
                                    Caption{text: "CORNER A  ·  X  Y  Z"}
                                    Row{
                                        corner_a := Field{empty_text: "X, Y, Z"}
                                        capture_a := Action{width: 80 text: "Capture A"}
                                    }
                                    Caption{text: "CORNER B  ·  X  Y  Z"}
                                    Row{
                                        corner_b := Field{empty_text: "X, Y, Z"}
                                        capture_b := Action{width: 80 text: "Capture B"}
                                    }
                                    add_zone := Action{text: "Add zone to preview"}
                                    Rule{}
                                    zone_list := DropDown{width: Fill height: 34 labels: ["No zones"]}
                                    zone_coords := Info{}
                                    remove_zone := Action{text: "Remove selected zone"}
                                }
                                control_panel := Column{
                                    visible: false
                                    Title{text: "Controller"}
                                    Info{text: "Connect to mirror measured joint angles. Set mounting and payload in UFACTORY Studio before entering Manual Mode to teach by hand."}
                                    host := Field{empty_text: "Controller IP address"}
                                    connect := Action{text: "Connect over Ethernet"}
                                    details := Info{text: "Disconnected"}
                                    Rule{}
                                    Title{text: "Reduced speed"}
                                    limits := Info{text: "Connect to read the current limits."}
                                    slow := Action{text: "Apply slow limits"}
                                    stop := Action{text: "Request controller stop"}
                                    Info{text: "Use the physical emergency stop for an emergency."}
                                }
                            }
                            Rule{}
                            Row{
                                save := Action{text: "Save study"}
                                load := Action{text: "Open saved study"}
                            }
                            notice := Info{text: "Study stored locally."}
                        }
                    }
                    Rule{}
                    RectView{
                        width: Fill height: 36 flow: Right align: Align{y: 0.5}
                        padding: Inset{left: 18 right: 18} draw_bg.color: #x182226
                        clearance := Info{text: "No enclosure defined."}
                    }
                }
            }
        }
    }
}
