use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;

    FONT_SIZE_SUB = 55

    TEXT_FONT_SPEC = {
        font_size: (FONT_SIZE_SUB),
        font: {path: dep("crate://makepad-widgets/resources/DejavuSans.ttf")}
    }

    // https://github.com/lxgw/LxgwWenKai
    // 霞鹜文楷
    TEXT_LXG_BOLD_FONT = {
        font_size: (FONT_SIZE_SUB),
        font: {path: dep("crate://makepad-widgets/resources/LXGWWenKaiBold.ttf")}
    }

    TEXT_LXG_LIGHT_FONT = {
        font_size: (15),
        font: {path: dep("crate://makepad-widgets/resources/LXGWWenKaiLight.ttf")}
    }


    TEXT_LXG_MONO_LIGHT_FONT = {
        font_size: (15),
        font: {path: dep("crate://makepad-widgets/resources/LXGWWenKaiMonoLight.ttf")}
    }

    TEXT_LXG_REGULAR_FONT = {
        font_size: (15),
        font: {path: dep("crate://makepad-widgets/resources/LXGWWenKaiRegular.ttf")}
    }

    App = {{App}} {
        ui: <Root> {
            // 定义主窗口
            main_window = <Window>{
                flow: Down,
                spacing: 10,
                align: {x: 0.5, y: 0.0},
                // 显示背景
                show_bg: true
                width: Fill,
                height: Fill


                // 定义自定义背景绘制
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        // 获取几何位置
                        let st = vec2(
                            self.geom_pos.x,
                            self.geom_pos.y
                        );

                        // 计算颜色，基于 x 和 y 位置及时间
                        let color = vec3(st.x, st.y, abs(sin(self.time)));
                        return vec4(color, 1.0);
                    }
                }

                poem = <View> {
                    flow: Right
                    margin: 50

                    spacing: 10,
                    align: {x: 0.5, y: 0.0},
                    poem1 = <View> {
                        flow: Right,

                        luo = <Label> {
                            text: "落霞"
                            draw_text: {
                                color: #FF4500
                                text_style: <TEXT_LXG_BOLD_FONT> {}
                            },
                        }

                        yu = <Label> {
                            text: "
                            与"
                            draw_text: {
                                color: #87CEEB
                                text_style: <TEXT_LXG_BOLD_FONT> {}
                            },
                        }

                        fei = <Label> {
                            text: "
                            孤鹜齐飞 "

                            draw_text: {
                                color: #000000
                                text_style: <TEXT_LXG_BOLD_FONT> {}
                            },
                        }
                    }

                    poem2 = <View> {
                        flow: Right,
                        qiushui = <Label> {
                            text: "秋水"
                            draw_text: {
                                color: #1E90FF
                                text_style: <TEXT_LXG_BOLD_FONT> {}
                            },
                        }

                        gong = <Label> {
                            align: {x: 0.5, y: 0.5},
                            text: "
                            共"

                            draw_text: {
                                color: #8FBC8F
                                text_style: <TEXT_LXG_BOLD_FONT> {}
                            },
                        }

                        changtian = <Label> {
                            align: {x: 0.5, y: 0.5},
                            text: "
                            长天一色"

                            draw_text: {
                                color: #000000
                                text_style: <TEXT_LXG_BOLD_FONT> {}
                            },
                        }
                    }

                }


                poet_container = <ScrollXYView> {
                    flow: Down,
                    align: {x: 0.5, y: 0.5},



                    poem_info = <View> {
                        flow: Down,
                        align: {x: 0.5, y: 0.0},

                        poem3 = <Label> {
                            // align: {x: -0.5, y: 0.5},
                            text: "\n  〔唐〕  王勃 《滕王阁序》\n"
                            draw_text: {
                                color: #000000
                                text_style: <TEXT_LXG_REGULAR_FONT> {}
                            },
                        }

                        poem4 = <Label> {
                            text: "
                            「霞鹜文楷」 LXGW WenKai"
                            draw_text: {
                                color: #666666
                                text_style: <TEXT_LXG_MONO_LIGHT_FONT> {}
                            },
                        }

                        poem5 = <Label> {
                            text: "FONTWORKS 开源日文字体  klee One 衍生 CJK 字体"
                            draw_text: {
                                color: #666666
                                text_style: <TEXT_LXG_MONO_LIGHT_FONT> {}
                            },
                        }


                        poem6 = <Label> {
                            text: "
                            ䷀ ䷁ ䷜
                            "
                            draw_text: {
                                color: #666666
                                text_style: <TEXT_FONT_SPEC> {font_size: (9.5)}
                            },
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
    #[live]
    ui: WidgetRef,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        makepad_widgets::live_design(cx);
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
