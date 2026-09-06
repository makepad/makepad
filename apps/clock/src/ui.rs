use crate::*;

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    let Title = Label{padding: 0 draw_text +: {text_style: theme.font_bold{font_size: 24} color: theme.color_text}}
    let Caption = Label{padding: 0 draw_text +: {text_style: theme.font_regular{font_size: 11} color: theme.color_text_disabled}}
    let Readout = Label{padding: 0 draw_text +: {text_style: theme.font_regular{font_size: 46} color: theme.color_text}}
    let Key = Button{width: Fill{min: 64} height: 48 padding: 8}
    let Tabs = RadioButtonTabFlat{
        width: Fill height: 52 padding: 0
        label_walk +: {margin: 0}
        align: Align{x: 0.5 y: 0.5}
        draw_text.text_style: theme.font_regular{font_size: 10}
    }
    let Card = RoundedView{
        width: Fill height: Fit padding: 22 spacing: 18 flow: Down
        draw_bg +: {
            color: theme.color_bg_container
            border_radius: theme.container_corner_radius
            accent: instance(theme.color_focus)
            pixel: fn(){
                let sdf=Sdf2d.viewport(self.pos*self.rect_size)
                sdf.box(0.0,0.0,self.rect_size.x,self.rect_size.y,self.border_radius)
                let glow=exp(-length(self.pos-vec2(0.8,0.15))*3.0)*0.13
                sdf.fill(mix(self.color,self.accent,glow))
                return sdf.result
            }
        }
    }
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "Clock" window.inner_size: vec2(420,800)
                pass +: {clear_color: theme.color_bg_app}
                body +: {
                    padding: 0 spacing: 0
                    app_view := HostedView{
                        full: View{
                            width: Fill height: Fill flow: Down padding: 16 spacing: 12
                            show_bg: true draw_bg.color: theme.color_bg_app
                            header := View{
                                width: Fill height: Fit flow: Right align: Align{y: 0.5} spacing: 12
                                AppIcon{name: "clock" width: 38 height: 38}
                                View{width: Fill height: Fit flow: Down spacing: 3
                                    mode_title := Title{text: "Clock"}
                                    header_date := Caption{text: "Your local time"}
                                }
                            }
                            alert_panel := Card{
                                visible: false padding: 14 spacing: 10
                                alert_title := Label{text: "Time is up" draw_text.text_style: theme.font_bold{font_size: 15}}
                                View{width: Fill height: Fit spacing: 10
                                    alert_stop := Key{text: "Stop"}
                                    alert_snooze := Key{text: "Snooze 5 min"}
                                }
                            }
                            modes := PageFlip{
                                width: Fill height: Fill active_page: @clock_page
                                clock_page := ScrollYView{
                                    width: Fill height: Fill flow: Down spacing: 16
                                    clock_card := Card{
                                        align: Align{x: 0.5 y: 0.5}
                                        flow: Right{wrap: true}
                                        face_full := ClockFace{width: 268 height: 268}
                                        View{width: Fill{basis: 260 min: 170} height: Fit flow: Down spacing: 6 align: Align{x: 0.5}
                                            Caption{text: "LOCAL TIME"}
                                            time_full := Readout{text: "9:41"}
                                            date_full := Caption{}
                                        }
                                    }
                                    next_alarm := Card{padding: 18 spacing: 6
                                        Caption{text: "NEXT ALARM"}
                                        next_alarm_text := Label{text: "No alarm set" draw_text.text_style: theme.font_regular{font_size: 17}}
                                        open_alarm := ButtonFlat{text: "Set an alarm" height: 44}
                                    }
                                }
                                stopwatch_page := ScrollYView{
                                    width: Fill height: Fill flow: Down spacing: 16
                                    Card{align: Align{x: 0.5} padding: Inset{top: 52 bottom: 40 left: 20 right: 20}
                                        Caption{text: "ELAPSED TIME"}
                                        sw_readout := Readout{text: "00:00.0"}
                                        View{width: Fill height: Fit spacing: 10
                                            sw_reset := Key{text: "Reset"}
                                            sw_toggle := Key{text: "Start"}
                                        }
                                        sw_lap := ButtonFlat{text: "Lap" width: Fill height: 44}
                                    }
                                    Card{Caption{text: "LAPS"} lap_readout := Label{text: "Your laps will appear here" width: Fill draw_text.text_style: theme.font_regular{font_size: 13}}}
                                }
                                timer_page := ScrollYView{
                                    width: Fill height: Fill flow: Down spacing: 16
                                    Card{align: Align{x: 0.5} padding: Inset{top: 52 bottom: 40 left: 20 right: 20}
                                        Caption{text: "COUNTDOWN"}
                                        cd_readout := Readout{text: "05:00"}
                                        cd_status := Caption{}
                                        View{width: Fill height: Fit spacing: 10
                                            cd_reset := Key{text: "Reset"}
                                            cd_toggle := Key{text: "Start"}
                                        }
                                    }
                                    Card{Caption{text: "ADJUST DURATION"}
                                        View{width: Fill height: Fit flow: Right{wrap: true} spacing: 8
                                            cd_minus := Key{text: "−1 min"}
                                            cd_plus := Key{text: "+1 min"}
                                            cd_plus5 := Key{text: "+5 min"}
                                        }
                                    }
                                }
                                alarm_page := PageFlip{
                                    width: Fill height: Fill active_page: @alarm_list_page
                                    alarm_list_page := View{
                                        width: Fill height: Fill flow: Down spacing: 12
                                        View{width: Fill height: 48 align: Align{y: 0.5}
                                            Label{width: Fill text: "Your alarms" draw_text.text_style: theme.font_bold{font_size: 18}}
                                            alarm_add := ButtonFlat{text: "+ Add alarm" width: Fit height: 48}
                                        }
                                        alarm_list := AlarmList{}
                                        Caption{text: "Swipe left to delete · tap a time to edit" width: Fill}
                                        alarm_note := Caption{text: "Sounds while Clock is running." width: Fill}
                                    }
                                    alarm_edit_page := ScrollYView{
                                        width: Fill height: Fill flow: Down spacing: 16
                                        Card{padding: 18 spacing: 12
                                            alarm_editor_title := Label{text: "New alarm" draw_text.text_style: theme.font_bold{font_size: 18}}
                                            View{width: Fill height: Fit
                                                Caption{width: Fill text: "HOUR" align: Align{x: 0.5}}
                                                Caption{width: Fill text: "MINUTE" align: Align{x: 0.5}}
                                            }
                                            alarm_wheel := TimeWheel{}
                                            alarm_label := TextInput{width: Fill height: 48 empty_text: "Alarm label"}
                                            Caption{text: "Repeats every day"}
                                            View{width: Fill height: Fit spacing: 12
                                                alarm_cancel := Key{text: "Cancel"}
                                                alarm_save := Key{text: "Save alarm"}
                                            }
                                        }
                                    }
                                }
                            }
                            View{width: Fill height: 52 spacing: 4
                                tab_clock := Tabs{text: "Clock"}
                                tab_alarm := Tabs{text: "Alarm"}
                                tab_stopwatch := Tabs{text: "Stopwatch"}
                                tab_timer := Tabs{text: "Timer"}
                            }
                        }
                        tile: RectView{
                            width: Fill height: Fill flow: Right{wrap: true} padding: 12 spacing: 8
                            align: Align{x: 0.5 y: 0.5}
                            draw_bg +: {color: theme.color_bg_app}
                            face_tile := ClockFace{width: 112 height: 112}
                            View{width: Fill{basis: 110 min: 90} height: Fit flow: Down align: Align{x: 0.5}
                                time_tile := Label{visible: false}
                                date_tile := Caption{width: Fill align: Align{x: 0.5} draw_text.text_style.font_size: 9}
                                extra_tile := Caption{width: Fill align: Align{x: 0.5} draw_text.color: theme.color_focus draw_text.text_style.font_size: 9}
                            }
                        }
                    }
                }
            }
        }
    }
}
