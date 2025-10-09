
use makepad_widgets::*;

live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
        
    App = {{App}} {
        ui: <Root>{
            main_window = <Window>{
                body = <View> {
                    flow: Down,
                    padding: {
                        top: 16.0,
                    }
                    scroll_bars: <ScrollBars> {}
                    <Label> {
                        text: "Weighted fill.\n\nThe green square uses a fill with double the weight of the other two, so it takes up twice as much space.",
                    }
                    <View> {
                        width: 400.0,
                        height: 100.0,
                        show_bg: true,
                        draw_bg: {
                            color: #888
                        },
                        <View> {
                            width: Fill {
                                weight: 1.0
                            }
                            height: Fill,
                            show_bg: true,
                            draw_bg: {
                                color: #F00
                            }
                        }
                        <View> {
                            width: Fill {
                                weight: 2.0
                            }
                            height: Fill
                            show_bg: true,
                            draw_bg: {
                                color: #0F0
                            },
                        }
                        <View> {
                            width: Fill {
                                weight: 1.0
                            }
                            height: Fill
                            show_bg: true,
                            draw_bg: {
                                color: #00F
                            }
                        }
                    }
                    <Label> {
                        text: "Fill with minimal size.\n\nThe red square uses a fill with a minimum size of 200, so it takes up more space than it normally would."
                    }
                    <View> {
                        width: 300.0,
                        height: 200.0,
                        show_bg: true,
                        draw_bg: {
                            color: #888
                        },
                        <View> {
                            width: Fill {
                                min: 200.0,
                            },
                            height: Fill {
                                min: 200.0,
                            },
                            show_bg: true,
                            draw_bg: {
                                color: #F00
                            }
                        }
                        <View> {
                            width: Fill,
                            height: Fill,
                            show_bg: true,
                            draw_bg: {
                                color: #0F0
                            }
                        }
                    }
                    <Label> {
                        text: "Fill with maximum size.\n\nThe red square uses a fill with a maximum size of 200, so it takes up less space than it normally woud."
                    }
                    <View> {
                        width: 300.0,
                        height: 200.0,
                        show_bg: true,
                        draw_bg: {
                            color: #888
                        },
                        <View> {
                            width: Fill {
                                max: 100.0
                            },
                            height: Fill {
                                max: 100.0
                            },
                            show_bg: true,
                            draw_bg: {
                                color: #F00
                            }
                        }
                        <View> {
                            width: Fill,
                            height: Fill,
                            show_bg: true,
                            draw_bg: {
                                color: #0F0
                            }
                        }
                    }
                    <Label> {
                        text: "Fit with minimal size. The gray square uses a fit with a minimum size of 200, so it takes up more space than it normally would.",
                    }
                    <View> {
                        width: Fit {
                            min: 200.0
                        },
                        height: Fit {
                            min: 200.0
                        },
                        show_bg: true,
                        draw_bg: {
                            color: #888
                        }
                        <View> {
                            width: 100.0,
                            height: 100.0,
                            show_bg: true,
                            draw_bg: {
                                color: #F00
                            }
                        }
                    }
                    <Label> {
                        text: "Fit with maximal size. The parent view uses a fit with a maximum size of 100, so the red square of size 200 is clipped.",
                    }
                    <View> {
                        width: Fit {
                            max: 100.0
                        },
                        height: Fit {
                            max: 100.0
                        },
                        show_bg: true,
                        draw_bg: {
                            color: #888
                        }
                        <View> {
                            width: 200.0,
                            height: 200.0,
                            show_bg: true,
                            draw_bg: {
                                color: #F00
                            }
                        }
                    }
                    <Label> {
                        text: "Fit with relative minimal size. The dark gray square uses a fit with a minimum of half its parent size, so it takes up more space than it normally would.",
                    }
                    <View> {
                        width: 400,
                        height: 400,
                        show_bg: true,
                        draw_bg: {
                            color: #888
                        }
                        <View> {
                            width: Fit {
                                min: Rel(0.5)
                            },
                            height: Fit {
                                min: Rel(0.5)
                            },
                            show_bg: true,
                            draw_bg: {
                                color: #444
                            }
                            <View> {
                                width: 100.0,
                                height: 100.0,
                                show_bg: true,
                                draw_bg: {
                                    color: #F00
                                }
                            }
                        }
                    }
                    <Label> {
                        text: "Fit with relative minimal size. The parent view uses a fit with a maximum of half its parent size, so the red square of size 300 is clipped.",
                    }
                    <View> {
                        width: 400,
                        height: 400,
                        show_bg: true,
                        draw_bg: {
                            color: #888
                        }
                        <View> {
                            width: Fit {
                                max: Rel(0.5)
                            },
                            height: Fit {
                                max: Rel(0.5)
                            },
                            show_bg: true,
                            draw_bg: {
                                color: #444
                            }
                            <View> {
                                width: 300.0,
                                height: 300.0,
                                show_bg: true,
                                draw_bg: {
                                    color: #F00
                                }
                            }
                        }
                    }
                    <Label> {
                        text: "Nested fits with relative minimal size. Both the parent and the grandparent view use a fit with a maximum of half the size of the outermost view, so the red square of size 300 is clipped.",
                    }
                    <View> {
                        width: 400,
                        height: 400,
                        show_bg: true,
                        draw_bg: {
                            color: #CCC
                        }
                        <View> {
                            width: Fit {
                                max: Rel(0.5)
                            },
                            height: Fit {
                                max: Rel(0.5)
                            },
                            show_bg: true,
                            draw_bg: {
                                color: #888
                            }
                            <View> {
                                width: Fit {
                                    max: Rel(0.5)
                                },
                                height: Fit {
                                    max: Rel(0.5)
                                },
                                show_bg: true,
                                draw_bg: {
                                    color: #444
                                }
                                <View> {
                                    width: 300.0,
                                    height: 300.0,
                                    show_bg: true,
                                    draw_bg: {
                                        color: #F00
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}  

app_main!(App); 
 
#[derive(Live, LiveHook)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
}
 
impl LiveRegister for App {
    fn live_register(cx: &mut Cx) { 
        crate::makepad_widgets::live_design(cx);
    }
}

impl MatchEvent for App{
    fn handle_startup(&mut self, _cx:&mut Cx){
    }
        
    fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
        if self.ui.button(id!(button_1)).clicked(&actions) {
            self.ui.button(id!(button_1)).set_text(cx, "Clicked 😀");
            log!("hi");
            self.counter += 1;
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::XrUpdate(_e) = event{
            //log!("{:?}", e.now.left.trigger.analog);
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}