use crate::{makepad_live_id::*};
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::fs;
use std::time::{Instant, Duration};
use crate::database::*;
use crate::comfyui::*;

live_design!{
    import makepad_widgets::button::Button;
    import makepad_widgets::desktop_window::DesktopWindow;
    import makepad_widgets::multi_window::MultiWindow;
    import makepad_widgets::label::Label;
    import makepad_widgets::image::Image;
    import makepad_widgets::text_input::TextInput;
    import makepad_widgets::image::Image;
    import makepad_widgets::list_view::ListView;
    import makepad_widgets::drop_down::DropDown;
    import makepad_widgets::slide_panel::SlidePanel;
    import makepad_widgets::check_box::CheckBox;
    import makepad_widgets::view::*;
    import makepad_widgets::theme::*;
    import makepad_draw::shader::std::*;
    import makepad_widgets::dock::*;
    
    
    TEXT_BIG = 12.0
    
    COLOR_UP_0 = #xFFFFFF00
    COLOR_DOWN_2 = #x00000022
    FONT_SIZE_H2 = 10.0
    
    SSPACING_0 = 0.0
    SSPACING_1 = 4.0
    SSPACING_2 = (SSPACING_1 * 2)
    SSPACING_3 = (SSPACING_1 * 3)
    SSPACING_4 = (SSPACING_1 * 4)
    
    SPACING_0 = {top: (SSPACING_0), right: (SSPACING_0), bottom: (SSPACING_0), left: (SSPACING_0)}
    SPACING_1 = {top: (SSPACING_1), right: (SSPACING_1), bottom: (SSPACING_1), left: (SSPACING_1)}
    SPACING_2 = {top: (SSPACING_2), right: (SSPACING_2), bottom: (SSPACING_2), left: (SSPACING_2)}
    SPACING_3 = {top: (SSPACING_3), right: (SSPACING_3), bottom: (SSPACING_3), left: (SSPACING_3)}
    SPACING_4 = {top: (SSPACING_4), right: (SSPACING_4), bottom: (SSPACING_4), left: (SSPACING_4)}
    
    H2_TEXT_BOLD = {
        font_size: (FONT_SIZE_H2),
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}
    }
    
    H2_TEXT_REGULAR = {
        font_size: (FONT_SIZE_H2),
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-Text.ttf")}
    }
    
    TEXT_BOLD = {
        font_size: 10.0,
        font: {path: dep("crate://makepad-widgets/resources/IBMPlexSans-SemiBold.ttf")}
    }
    
    TEXT_MONO = {
        font_size: 10.0,
        font: {path: dep("crate://makepad-widgets/resources/LiberationMono-Regular.ttf")}
    }
    
    COLOR_PANEL_BG = (COLOR_DOWN_2)
    COLOR_TEXT_INPUT = (COLOR_DOWN_2)
    COLOR_LABEL = #xFFF9
    
    
    SdxlDropDown = <DropDown> {
        width: Fit
        padding: {top: (SSPACING_2), right: (SSPACING_4), bottom: (SSPACING_2), left: (SSPACING_2)}
        
        draw_label: {
            text_style: <H2_TEXT_REGULAR> {},
            fn get_color(self) -> vec4 {
                return mix(
                    mix(
                        mix(
                            (#xFFF8),
                            (#xFFF8),
                            self.focus
                        ),
                        (#xFFFF),
                        self.hover
                    ),
                    (#x000A),
                    self.pressed
                )
            }
        }
        
        popup_menu: {
            menu_item: {
                indent_width: 10.0
                width: Fill, height: Fit
                
                
                    padding: {left: (SSPACING_4), top: (SSPACING_2), bottom: (SSPACING_2), right: (SSPACING_4)
                }
                
                draw_bg: {
                    color: #x48,
                    color_selected: #x6
                }
            }
        }
        
        draw_bg: {
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                self.get_bg(sdf);
                // triangle
                let c = vec2(self.rect_size.x - 10.0, self.rect_size.y * 0.5)
                let sz = 2.5;
                
                sdf.move_to(c.x - sz, c.y - sz);
                sdf.line_to(c.x + sz, c.y - sz);
                sdf.line_to(c.x, c.y + sz * 0.75);
                sdf.close_path();
                
                sdf.fill(mix(#FFFA, #FFFF, self.hover));
                
                return sdf.result
            }
            
            fn get_bg(self, inout sdf: Sdf2d) {
                sdf.rect(
                    0,
                    0,
                    self.rect_size.x,
                    self.rect_size.y
                )
                sdf.fill((COLOR_UP_0))
            }
        }
    }
    
    BarLabel = <Label> {
        margin: {left: 10},
        label: "Workflow",
        draw_label: {
            text_style: <TEXT_BOLD> {},
            fn get_color(self) -> vec4 {
                return (COLOR_LABEL)
            }
        }
    }
    
    
    
    BarButton = <Button> {
        padding: {top: 5.0, right: 7.5, bottom: 5.0, left: 7.5}
        margin: {top: 5.0, right: 5.0, bottom: 5.0, left: 5.0}
        label: "Cancel"
        draw_label: {
            text_style: <TEXT_BOLD> {},
        }
        draw_bg: {
            instance hover: 0.0
            instance pressed: 0.0
            uniform border_radius: 3.0
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let grad_top = 5.0;
                let grad_bot = 1.0;
                let body = mix(mix(#53, #5c, self.hover), #33, self.pressed);
                let body_transp = vec4(body.xyz, 0.0);
                let top_gradient = mix(body_transp, mix(#6d, #1f, self.pressed), max(0.0, grad_top - sdf.pos.y) / grad_top);
                let bot_gradient = mix(
                    mix(body_transp, #5c, self.pressed),
                    top_gradient,
                    clamp((self.rect_size.y - grad_bot - sdf.pos.y - 1.0) / grad_bot, 0.0, 1.0)
                );
                
                // the little drop shadow at the bottom
                let shift_inward = self.border_radius + 4.0;
                sdf.move_to(shift_inward, self.rect_size.y - self.border_radius);
                sdf.line_to(self.rect_size.x - shift_inward, self.rect_size.y - self.border_radius);
                sdf.stroke(
                    mix(mix(#0006, #1f, self.hover), #0000, self.pressed),
                    self.border_radius
                )
                
                sdf.box(
                    1.,
                    1.,
                    self.rect_size.x - 2.0,
                    self.rect_size.y - 2.0,
                    self.border_radius
                )
                sdf.fill_keep(body)
                
                sdf.stroke(
                    bot_gradient,
                    1.0
                )
                
                return sdf.result
            }
        }
    }
    
    SettingsInput = <View> {
        width: 150, height: Fit, margin: {top: 10},
        label = <BarLabel> {
            width: Fit, margin: {left: 5},
            align: {x: 1.0}
        }
        input = <TextInput> {
            padding: 0
            height: Fit, width: 50, margin: {top: 1, left: 2}
            text: "1344"
        }
    }
    
    FillerH = <View> {
        width: Fill, height: Fit
    }
    
    FillerV = <View> {
        width: Fit, height: Fill
    }
    
    
    DividerV = <View> {
        flow: Down, spacing: 0.0
        margin: {top: 0.0, right: 0.0, bottom: 10.0, left: 0.0}
        width: Fill, height: Fit
        <RectView> {
            height: 2, width: Fill, margin: 0.0
            flow: Down, padding: 0.0
            draw_bg: {color: #x00000033}
        }
        <RectView> {
            height: 2, width: Fill, margin: 0.0
            flow: Down, padding: 0.0
            draw_bg: {color: #xFFFFFF18}
        }
    }
    
    DividerH = <View> {
        flow: Right, spacing: 0.0
        margin: {top: 0.0, right: 5.0, bottom: 0.0, left: 5.0}
        width: Fit, height: Fill
        <RectView> {
            height: Fill, width: 2, margin: 0.0
            flow: Down, padding: 0.0
            draw_bg: {color: #x00000033}
        }
        <RectView> {
            height: Fill, width: 2, margin: 0.0
            flow: Down, padding: 0.0
            draw_bg: {color: #xFFFFFF18}
        }
    }
    
    SdxlCheckBox = <CheckBox> {
        padding: {top: (SSPACING_0), right: 0, bottom: (SSPACING_0), left: 23}
        label_walk: {margin: {left: 20.0, top: 8, bottom: 8, right: 10}}
        animator: {
            selected = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_check: {selected: 0.0}}
                }
                on = {
                    cursor: Arrow,
                    from: {all: Forward {duration: 0.1}}
                    apply: {draw_check: {selected: 1.0}}
                }
            }
        }
        draw_check: {
            instance border_width: 1.0
            instance border_color: #x06
            instance border_color2: #xFFFFFF0A
            size: 8.5;
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                let sz = self.size;
                let left = sz + 1.;
                let c = vec2(left + sz, self.rect_size.y * 0.5);
                sdf.box(left, c.y - sz, sz * 3.0, sz * 2.0, 0.5 * sz);
                
                sdf.stroke_keep(#xFFF2, 1.25)
                
                sdf.fill(#xFFF0)
                let isz = sz * 0.65;
                sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                sdf.circle(left + sz + self.selected * sz, c.y - 0.5, 0.425 * isz);
                sdf.subtract();
                sdf.circle(left + sz + self.selected * sz, c.y - 0.5, isz);
                sdf.blend(self.selected)
                sdf.fill(#xFFF8);
                return sdf.result
            }
        }
        draw_label: {
            text_style: <TEXT_BOLD> {},
            fn get_color(self) -> vec4 {
                return (COLOR_LABEL)
            }
        }
        label: "Slideshow"
    }
    
    ProgressCircle = <View> {
        show_bg: true,
        width: 24, height: 24
        draw_bg: {
            instance progress: 0.0
            instance active: 0.0
            
            fn circle_pie(inout sdf: Sdf2d, x: float, y: float, r: float, s: float) {
                let c = sdf.pos - vec2(x, y);
                let len = sqrt(c.x * c.x + c.y * c.y) - r;
                let pi = 3.141592653589793;
                let ang = (pi - atan(c.x, c.y)) / (2.0 * pi);
                let ces = s * 0.5;
                let ang2 = clamp((abs(ang - ces) - ces) * -r * r * sdf.scale_factor, 0.0, 1.0);
                sdf.dist = len * ang2 / sdf.scale_factor;
                sdf.old_shape = sdf.shape;
                sdf.shape = min(sdf.shape, sdf.dist);
            }
            
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                sdf.circle(
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    self.rect_size.x * 0.4
                );
                sdf.fill(mix(#4, #575, self.active));
                circle_pie(
                    sdf,
                    self.rect_size.x * 0.5,
                    self.rect_size.y * 0.5,
                    self.rect_size.x * 0.4,
                    self.progress
                );
                sdf.fill(mix(#4, #8f8, self.active));
                return sdf.result;
            }
        }
    }
    
    PromptGroup = <RectView> {
        <DividerV> {}
        height: Fit, width: Fill, margin: {bottom: 10, top: 0}
        flow: Down, spacing: 0, padding: 0
        draw_bg: {
            instance hover: 0.0
            instance down: 0.0
            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                let body = mix(mix(#53, #5c, self.hover), #33, self.down);
                sdf.fill_keep(body)
                return sdf.result
            }
        }
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.5}}
                    ease: OutExp
                    apply: {
                        draw_bg: {hover: 0.0}
                        prompt = {draw_label: {hover: 0.0}}
                    }
                }
                on = {
                    ease: OutExp
                    from: {
                        all: Forward {duration: 0.2}
                    }
                    apply: {
                        draw_bg: {hover: 1.0}
                        prompt = {draw_label: {hover: 1.0}}
                    }
                }
            }
            down = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.5}}
                    ease: OutExp
                    apply: {
                        draw_bg: {down: 0.0}
                        prompt = {draw_label: {down: 0.0}}
                    }
                }
                on = {
                    ease: OutExp
                    from: {
                        all: Forward {duration: 0.2}
                    }
                    apply: {
                        draw_bg: {down: 1.0}
                        prompt = {draw_label: {down: 1.0}}
                    }
                }
            }
        }
        prompt = <Label> {
            width: Fill
            draw_label: {
                text_style: <TEXT_BOLD> {},
                instance hover: 0.0
                instance down: 0.0
                fn get_color(self) -> vec4 {
                    return mix(mix(#xFFFA, #xFFFF, self.hover), #xFFF8, self.down);
                }
                wrap: Word,
            }
            label: ""
        }
    }
    
    ImageTile = <View> {
        width: Fill, height: Fit
        cursor: Hand
        animator: {
            hover = {
                default: off,
                off = {
                    from: {all: Forward {duration: 0.5}}
                    ease: OutExp
                    apply: {
                        img = {draw_bg: {hover: 0.0}}
                    }
                }
                on = {
                    ease: OutExp
                    from: {
                        all: Forward {duration: 0.3}
                    }
                    apply: {
                        img = {draw_bg: {hover: 1.0}}
                    }
                }
            }
            down = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.5}}
                    ease: OutExp
                    apply: {
                        img = {draw_bg: {down: 0.0}}
                    }
                }
                on = {
                    ease: OutExp
                    from: {
                        all: Forward {duration: 0.3}
                    }
                    apply: {
                        img = {draw_bg: {down: 1.0}}
                    }
                }
            }
        }
        
        img = <Image> {
            width: Fill, height: Fill
            min_width: 1920,
            min_height: 1080,
            fit: Horizontal,
            draw_bg: {
                instance hover: 0.0
                instance down: 0.0
                fn pixel(self) -> vec4 {
                    let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                    sdf.box(1, 1, self.rect_size.x - 2, self.rect_size.y - 2, 4.0)
                    let max_scale = vec2(0.92);
                    let scale = mix(vec2(1.0), max_scale, self.hover);
                    let pan = mix(vec2(0.0), (vec2(1.0) - max_scale) * 0.5, self.hover);
                    let color = self.get_color_scale_pan(scale, pan) + mix(vec4(0.0), vec4(0.1), self.down);
                    sdf.fill_keep(color);
                    sdf.stroke(
                        mix(mix(#x0000, #x0006, self.hover), #xfff2, self.down),
                        1.0
                    )
                    
                    return sdf.result
                }
            }
        }
    }
    
    App = {{App}} {
        ui: <MultiWindow> {
            <DesktopWindow> {
                window: {inner_size: vec2(2000, 1024)},
                caption_bar = {visible: true, caption_label = {label = {label: "SDXL Surf"}}},
                
                <View> {
                    
                        flow: Overlay,
                    
                    
                        width: Fill,
                        height: Fill
                    
                    
                    dock = <Dock> {
                        height: Fill, width: Fill
                        
                        root = Splitter {
                            axis: Horizontal,
                            align: FromA(300.0),
                            a: image_library,
                            b: split1
                        }
                        
                        split1 = Splitter {
                            axis: Vertical,
                            align: FromB(200.0),
                            a: image_view,
                            b: input_panel
                        }
                        
                        image_library = Tab {
                            name: ""
                            kind: ImageLibrary
                        }
                        
                        input_panel = Tab {
                            name: ""
                            kind: InputPanel
                        }
                        
                        image_view = Tab {
                            name: ""
                            kind: ImageView
                        }
                        
                        ImageView = <RectView> {
                            draw_bg: {color: #2}
                            height: Fill, width: Fill
                            flow: Down, align: {x: 0.5, y: 0.5}
                            cursor: Hand,
                            image = <Image> {
                                fit: Smallest,
                                width: Fill, height: Fill
                            }
                        }
                        
                        InputPanel = <RectView> {
                            height: Fill, width: Fill
                            flow: Down, padding: 0.0
                            draw_bg: {color: (COLOR_PANEL_BG)}
                            <View> {
                                height: Fit, width: Fill
                                align: {x: 0.0, y: 0.5}
                                padding: 5
                                
                                <BarLabel> {
                                    label: "Workflow"
                                }
                                
                                workflow_dropdown = <SdxlDropDown> {}
                                
                                <BarLabel> {
                                    label: "Batch size"
                                }
                                batch_mode_dropdown = <SdxlDropDown> {
                                    selected_item: 5
                                    labels: ["1", "2", "3", "4", "5", "6", "10000"]
                                }
                                
                                <BarLabel> {
                                    label: "Seed"
                                }
                                seed_input = <TextInput> {
                                    draw_label: {text_style: <TEXT_BOLD> {}}
                                    height: Fit, width: Fit, margin: {bottom: 0, left: 0}
                                }
                                
                                render_batch = <BarButton> {
                                    label: "Batch"
                                }
                                render_single = <BarButton> {
                                    label: "Single"
                                }
                                cancel_todo = <BarButton> {
                                    label: "Cancel"
                                }
                                
                                <DividerH> {}
                                play_button = <BarButton> {
                                    label: "Play"
                                }
                                slide_show_check_box = <SdxlCheckBox> {
                                    label: "Slideshow"
                                }
                                
                                slide_show_dropdown = <SdxlDropDown> {
                                    selected_item: 5
                                    margin: 0
                                    labels: ["0", "1", "2", "3", "4", "5", "7", "10"]
                                }
                                
                                <DividerH> {}
                                
                                
                                <FillerH> {}
                                cluster_dropdown = <SdxlDropDown> {
                                    selected_item: 0
                                    margin: 0
                                    labels: ["All nodes", "Part 1", "Part 2"]
                                }
                                todo_label = <BarLabel> {
                                    margin: {right: 5.0}
                                    label: "Todo 0"
                                }
                                progress1 = <ProgressCircle> {}
                                progress2 = <ProgressCircle> {}
                                progress3 = <ProgressCircle> {}
                                progress4 = <ProgressCircle> {}
                                progress5 = <ProgressCircle> {}
                                progress6 = <ProgressCircle> {
                                    margin: {right: 5.0}
                                }
                                
                            }
                            <View> {
                                positive = <TextInput> {
                                    ascii_only: true,
                                    width: Fill, height: Fill, margin: {top: 0.0, left: 10.0, bottom: 10.0, right: 5.0},
                                    text: "Positive"
                                    draw_label: {
                                        text_style: <TEXT_MONO> {font_size: (TEXT_BIG)}
                                    }
                                    draw_bg: {
                                        color: (COLOR_TEXT_INPUT)
                                        border_width: 1.0
                                        border_color: #x00000044
                                    }
                                }
                                negative = <TextInput> {
                                    ascii_only: true,
                                    width: 200, height: Fill, margin: {top: 0.0, left: 5.0, bottom: 10.0, right: 10.0},
                                    draw_label: {text_style: <TEXT_MONO> {font_size: (TEXT_BIG)}}
                                    text: "text, watermark, cartoon"
                                    draw_bg: {
                                        color: (COLOR_TEXT_INPUT)
                                        border_width: 1.0
                                        border_color: #x00000044
                                    }
                                }
                                <View> {
                                    width: Fill, height: Fill
                                    flow: Right
                                    <View> {
                                        width: 100, height: Fit, margin: {top: 10},
                                        flow: Down
                                        settings_width = <SettingsInput> {label = {label: "width:"}, input = {text: "1344"}}
                                        settings_height = <SettingsInput> {label = {label: "height:"}, input = {text: "768"}}
                                        settings_steps = <SettingsInput> {label = {label: "steps:"}, input = {text: "20"}}
                                        settings_scale = <SettingsInput> {label = {label: "scale:"}, input = {text: "0.5"}}
                                        settings_total_steps = <SettingsInput> {label = {label: "total(0):"}, input = {text: "32"}}
                                    }
                                    <View> {
                                        width: Fit, height: Fit, margin: {top: 10},
                                        flow: Down
                                        settings_base_cfg = <SettingsInput> {label = {label: "base_cfg:"}, input = {text: "8.5"}}
                                        settings_refiner_cfg = <SettingsInput> {label = {label: "refiner_cfg:"}, input = {text: "9.5"}}
                                        settings_pos_score = <SettingsInput> {label = {label: "pos_score:"}, input = {text: "6"}}
                                        settings_neg_score = <SettingsInput> {label = {label: "neg_score:"}, input = {text: "2"}}
                                    }
                                    <View> {
                                        width: Fit, height: Fit, margin: {top: 10},
                                        flow: Down
                                        settings_base_start_step = <SettingsInput> {label = {label: "base_start_step:"}, input = {text: "0"}}
                                        settings_base_end_step = <SettingsInput> {label = {label: "base_end_step:"}, input = {text: "20"}}
                                        settings_refiner_start_step = <SettingsInput> {label = {label: "refiner_start_step:"}, input = {text: "20"}}
                                        settings_refiner_end_step = <SettingsInput> {label = {label: "refiner_end_step:"}, input = {text: "1000"}}
                                    }
                                    <View> {
                                        width: Fit, height: Fit, margin: {top: 10},
                                        flow: Down
                                        settings_upscale_steps = <SettingsInput> {label = {label: "upscale_steps:"}, input = {text: "31"}}
                                        settings_upscale_start_step = <SettingsInput> {label = {label: "upscale_start_step:"}, input = {text: "29"}}
                                        settings_upscale_end_step = <SettingsInput> {label = {label: "upscale_end_step:"}, input = {text: "1000"}}
                                    }
                                    /*
                                    <View> {
                                        width: Fill, height: Fit, margin: {top: 10},
                                        <BarLabel> {label: "base_cfg:"}
                                        base_cfg_input = <SettingsInput> {text: "1344"}
                                        <BarLabel> {label: "refiner_cfg:"}
                                        refiner_cfg_input = <SettingsInput> {text: "768"}
                                        <BarLabel> {label: "pos_score:"}
                                        positive_score_input = <SettingsInput> {text: "6.0"}
                                        <BarLabel> {label: "neg_score:"}
                                        negative_score_input = <SettingsInput> {text: "2.0"}
                                    }
                                    <View> {
                                        width: Fill, height: Fit, margin: {top: 10},
                                        <BarLabel> {label: "base_start_step:"}
                                        base_cfg_input = <SettingsInput> {text: "1344"}
                                        <BarLabel> {label: "base_end_step:"}
                                        refiner_cfg_input = <SettingsInput> {text: "768"}
                                        <BarLabel> {label: "refiner_start_step:"}
                                        positive_score_input = <SettingsInput> {text: "6.0"}
                                        <BarLabel> {label: "refiner_end_step:"}
                                        negative_score_input = <SettingsInput> {text: "2.0"}
                                    }*/
                                }
                            }
                        }
                        
                        ImageLibrary = <RectView> {
                            draw_bg: {color: (COLOR_PANEL_BG)}
                            height: Fill, width: Fill
                            flow: Down
                            <View> {
                                height: Fit, width: Fill
                                flow: Right, padding: {left: 10, right: 10, top: 10, bottom: 10},
                                search = <TextInput> {
                                    height: Fit, width: Fill, margin: {bottom: 0}
                                    empty_message: "Search"
                                    draw_bg: {
                                        color: (COLOR_TEXT_INPUT)
                                        border_width: 1.0
                                        border_color: #x00000044
                                    }
                                    draw_label: {
                                        text_style: {font_size: (TEXT_BIG)}
                                        fn get_color(self) -> vec4 {
                                            return
                                            mix(
                                                mix(
                                                    mix(
                                                        #xFFFFFF55,
                                                        #xFFFFFF88,
                                                        self.hover
                                                    ),
                                                    #xFFFFFFCC,
                                                    self.focus
                                                ),
                                                #xFFFFFF66,
                                                self.is_empty
                                            )
                                        }
                                    }
                                }
                            }
                            image_list = <ListView> {
                                height: Fill, width: Fill, margin: {top: 0}
                                flow: Down, padding: {top: 0, right: 10.0, bottom: 10.0, left: 10.0}
                                
                                PromptGroup = <PromptGroup> {}
                                
                                Empty = <View> {}
                                
                                ImageRow1 = <View> {
                                    height: Fit, width: Fill, margin: {bottom: 10}
                                    spacing: 20, flow: Right
                                    row1 = <ImageTile> {}
                                }
                                ImageRow2 = <View> {
                                    height: Fit, width: Fill, margin: {bottom: 10}
                                    spacing: 20, flow: Right
                                    row1 = <ImageTile> {}
                                    row2 = <ImageTile> {}
                                }
                                ImageRow3 = <View> {
                                    height: Fit, width: Fill, margin: {bottom: 10}
                                    spacing: 20, flow: Right
                                    row1 = <ImageTile> {}
                                    row2 = <ImageTile> {}
                                    row3 = <ImageTile> {}
                                }
                            }
                        }
                    }
                    
                    big_image = <RectView> {
                        visible: false,
                        draw_bg: {draw_depth: 10.0}
                        draw_bg: {color: #0}
                        height: All, width: All, abs_pos: vec2(0.0, 0.0)
                        flow: Overlay, align: {x: 0.5, y: 0.5}
                        image1 = <Image> {
                            draw_bg: {draw_depth: 11.0}
                            fit: Smallest,
                            width: Fill, height: Fill
                        }
                    }
                }
            }
            <DesktopWindow> {
                window: {inner_size: vec2(960, 540)},
                second_image = <RectView> {
                    draw_bg: {color: #0}
                    height: Fill, width: Fill
                    flow: Overlay, align: {x: 0.5, y: 0.5}
                    image1 = <Image> {
                        fit: Smallest,
                        width: Fill, height: Fill
                    }
                    prompt_frame = <View> {
                        width: Fill, height: Fill
                        align: {y: 1.0}
                        padding: {left: 120, bottom: 40,right:120}
                        prompt = <Label> {
                            width: Fill, height: Fit
                            draw_label: {
                                wrap: Word
                                text_style: <TEXT_BOLD> {font_size: 20}
                                
                                color: #c
                            },
                            label: "HELLO WORLD"
                        }
                    }
                }
            }
        }
    }
}

app_main!(App);

struct Machine {
    ip: String,
    id: LiveId,
    running: Option<RunningPrompt>,
    fetching: Option<RunningPrompt>
}

struct RunningPrompt {
    _started: Instant,
    steps_counter: usize,
    prompt_state: PromptState,
}

impl Machine {
    fn new(ip: &str, id: LiveId) -> Self {Self {
        ip: ip.to_string(),
        id,
        running: None,
        fetching: None
    }}
}

struct Workflow {
    name: String,
}
impl Workflow {
    fn new(name: &str) -> Self {Self {name: name.to_string()}}
}
/*
const KEYWORD_CLOUD:[(&'static str,&'static str);18]=[
   ("", "Beautiful woman"),
   ("", "Beautiful man"),
   ("", "Dog"),
   ("", "Ugly woman"),
   ("", "Ugly man"),
   ("", "wearing intricate tattoos"),
   ("", "wearing intricate lingerie"),
   ("", "as an android"),
   ("", "made of metal"),
   ("", "made of wood"),
   ("", "In a desert"),
   ("", "On the moon"),
   ("", "In a sunset"),
   ("", "At burningman"),
   ("", "covered in stars"),
   ("", "covered in rainbows"),
   ("", "photographic"),
   ("", "cinematic"),
];*/


#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust(vec![
        Machine::new("DESKTOP-1:8188", id_lut!(m1)),
        Machine::new("DESKTOP-2:8188", id_lut!(m2)),
        Machine::new("DESKTOP-3:8188", id_lut!(m3)),
        Machine::new("DESKTOP-4:8188", id_lut!(m4)),
        Machine::new("DESKTOP-7:8188", id_lut!(m5)),
        Machine::new("DESKTOP-8:8188", id_lut!(m6))/*
        Machine::new("192.168.0.69:8188", id_lut!(m1)),
        Machine::new("192.168.0.127:8188", id_lut!(m2)),
        Machine::new("192.168.0.116:8188", id_lut!(m3)),
        Machine::new("192.168.0.80:8188", id_lut!(m4)),
        Machine::new("192.168.0.81:8188", id_lut!(m5)),
        Machine::new("192.168.0.244:8188", id_lut!(m6)),*/
    ])] machines: Vec<Machine>,
    
    #[rust(vec![
        Workflow::new("hd")
    ])] workflows: Vec<Workflow>,
    
    #[rust] todo: Vec<PromptState>,
    
    #[rust(Database::new(cx))] db: Database,
    
    #[rust] filtered: FilteredDb,
    #[rust(10000u64)] last_seed: u64,
    
    #[rust] current_image: Option<ImageId>,
    
    #[rust(Instant::now())] last_flip: Instant
} 

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
    }
    
    fn after_new_from_doc(&mut self, cx: &mut Cx) {
        self.open_web_socket(cx);
        let _ = self.db.load_database();
        self.filtered.filter_db(&self.db, "", false);
        let workflows = self.workflows.iter().map( | v | v.name.clone()).collect();
        let dd = self.ui.get_drop_down(id!(workflow_dropdown));
        dd.set_labels(workflows);
        cx.start_interval(0.016);
        self.update_seed_display(cx);
    }
}

impl App {
    fn send_prompt(&mut self, cx: &mut Cx, prompt_state: PromptState) {
        // lets find a machine with the minimum queue size and
        for machine in &mut self.machines {
            if machine.running.is_some() {
                continue
            }
            let url = format!("http://{}/prompt", machine.ip);
            let mut request = HttpRequest::new(url, HttpMethod::POST);
            
            request.set_header("Content-Type".to_string(), "application/json".to_string());
            
            let ws = fs::read_to_string(format!("examples/sdxl/workspace_{}.json", prompt_state.prompt.preset.workflow)).unwrap();
            let ws = ws.replace("CLIENT_ID", "1234");
            let ws = ws.replace("TEXT_INPUT", &prompt_state.prompt.positive.replace("\n", "").replace("\"", ""));
            let ws = ws.replace("KEYWORD_INPUT", &prompt_state.prompt.positive.replace("\n", "").replace("\"", ""));
            let ws = ws.replace("NEGATIVE_INPUT", &format!("children, child, {}", prompt_state.prompt.negative.replace("\n", "").replace("\"", "")));
            let ws = ws.replace("11223344", &format!("{}", prompt_state.seed));
            
            let ws = ws.replace("1344", &format!("{}", prompt_state.prompt.preset.width));
            let ws = ws.replace("768", &format!("{}", prompt_state.prompt.preset.height));
            
            let ws = ws.replace("\"steps\": 23", &format!("\"steps\": {}", prompt_state.prompt.preset.steps));
            let ws = ws.replace("\"scale_by\": 0.7117466517857182", &format!("\"scale_by\": {}", prompt_state.prompt.preset.scale));
            let ws = ws.replace("\"cfg\": 8.5", &format!("\"cfg\": {}", prompt_state.prompt.preset.base_cfg));
            let ws = ws.replace("\"ascore\": 6", &format!("\"ascore\": {}", prompt_state.prompt.preset.positive_score));
            let ws = ws.replace("\"ascore\": 2", &format!("\"ascore\": {}", prompt_state.prompt.preset.negative_score));
            
            let ws = ws.replace("\"start_at_step\": 0", &format!("\"start_at_step\": {}", prompt_state.prompt.preset.base_start_step));
            let ws = ws.replace("\"end_at_step\": 27", &format!("\"end_at_step\": {}", prompt_state.prompt.preset.base_end_step));
            let ws = ws.replace("\"start_at_step\": 27", &format!("\"start_at_step\": {}", prompt_state.prompt.preset.refiner_start_step));
            let ws = ws.replace("\"end_at_step\": 1000", &format!("\"end_at_step\": {}", prompt_state.prompt.preset.refiner_end_step));
            
            let ws = ws.replace("\"steps\": 30", &format!("\"steps\": {}", prompt_state.prompt.preset.upscale_steps));
            let ws = ws.replace("\"start_at_step\": 29", &format!("\"start_at_step\": {}", prompt_state.prompt.preset.upscale_start_step));
            let ws = ws.replace("\"end_at_step\": 999", &format!("\"end_at_step\": {}", prompt_state.prompt.preset.upscale_end_step));
            
            
            // lets store that we queued this image
            request.set_metadata_id(machine.id);
            request.set_body(ws.as_bytes().to_vec());
            Self::update_progress(cx, &self.ui, machine.id, true, 0, 1);
            cx.http_request(live_id!(prompt), request);
            machine.running = Some(RunningPrompt {
                steps_counter: 0,
                prompt_state: prompt_state.clone(),
                _started: Instant::now(),
            });
            return
        }
        self.todo.insert(0, prompt_state);
    }
    
    fn clear_todo(&mut self, cx: &mut Cx) {
        for _ in 0..2 {
            self.todo.clear();
            for machine in &mut self.machines {
                let url = format!("http://{}/queue", machine.ip);
                let mut request = HttpRequest::new(url, HttpMethod::POST);
                let ws = "{\"clear\":true}";
                request.set_metadata_id(machine.id);
                request.set_body_string(ws);
                cx.http_request(live_id!(clear_queue), request);
                
                let url = format!("http://{}/interrupt", machine.ip);
                let mut request = HttpRequest::new(url, HttpMethod::POST);
                request.set_metadata_id(machine.id);
                cx.http_request(live_id!(interrupt), request);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    
    fn fetch_image(&self, cx: &mut Cx, machine_id: LiveId, image_name: &str) {
        let machine = self.machines.iter().find( | v | v.id == machine_id).unwrap();
        let url = format!("http://{}/view?filename={}&subfolder=&type=output", machine.ip, image_name);
        let mut request = HttpRequest::new(url, HttpMethod::GET);
        request.set_metadata_id(machine.id);
        cx.http_request(live_id!(image), request);
    }
    
    fn open_web_socket(&self, cx: &mut Cx) {
        for machine in &self.machines {
            let url = format!("ws://{}/ws?clientId={}", machine.ip, "1234");
            let request = HttpRequest::new(url, HttpMethod::GET);
            cx.web_socket_open(machine.id, request);
        }
    }
    
    fn update_progress(cx: &mut Cx, ui: &WidgetRef, machine: LiveId, active: bool, steps: usize, total: usize) {
        let progress_id = match machine {
            live_id!(m1) => id!(progress1),
            live_id!(m2) => id!(progress2),
            live_id!(m3) => id!(progress3),
            live_id!(m4) => id!(progress4),
            live_id!(m5) => id!(progress5),
            live_id!(m6) => id!(progress6),
            _ => panic!()
        };
        ui.get_view(progress_id).apply_over(cx, live!{
            draw_bg: {active: (if active {1.0}else {0.0}), progress: (steps as f64 / total as f64)}
        });
        ui.redraw(cx);
    }
    
    
    fn load_seed_from_current_image(&mut self, cx: &mut Cx) {
        if let Some(current_image) = &self.current_image {
            if let Some(image) = self.db.image_files.iter().find( | v | v.image_id == *current_image) {
                self.last_seed = image.seed;
                self.update_seed_display(cx);
            }
        }
    }
    
    fn prompt_hash_from_current_image(&mut self) -> LiveId {
        if let Some(current_image) = &self.current_image {
            if let Some(image) = self.db.image_files.iter().find( | v | v.image_id == *current_image) {
                return image.prompt_hash
            }
        }
        LiveId(0)
    }
    
    fn update_seed_display(&mut self, cx: &mut Cx) {
        self.ui.get_text_input(id!(seed_input)).set_text(&format!("{}", self.last_seed));
        self.ui.redraw(cx);
    }
    
    fn load_inputs_from_prompt_hash(&mut self, cx: &mut Cx, prompt_hash: LiveId) {
        if let Some(prompt_file) = self.db.prompt_files.iter().find( | v | v.prompt_hash == prompt_hash) {
            self.ui.get_text_input(id!(positive)).set_text(&prompt_file.prompt.positive);
            self.ui.get_text_input(id!(negative)).set_text(&prompt_file.prompt.negative);
            self.ui.redraw(cx);
            self.load_preset(&prompt_file.prompt.preset)
        }
    }
    
    fn set_current_image(&mut self, _cx: &mut Cx, image_id: ImageId) {
        self.current_image = Some(image_id);
        let prompt_hash = self.prompt_hash_from_current_image();
        if let Some(prompt_file) = self.db.prompt_files.iter().find( | v | v.prompt_hash == prompt_hash) {
            self.ui.get_label(id!(second_image.prompt)).set_label(&prompt_file.prompt.positive);
        }
    }
    
    fn select_next_image(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx);
        if let Some(current_image) = &self.current_image {
            if let Some(pos) = self.filtered.flat.iter().position( | v | *v == *current_image) {
                if pos + 1 < self.filtered.flat.len() {
                    self.set_current_image(cx, self.filtered.flat[pos + 1].clone());
                    self.last_flip = Instant::now();
                }
            }
        }
    }
    
    fn select_prev_image(&mut self, cx: &mut Cx) {
        self.ui.redraw(cx);
        if let Some(current_image) = &self.current_image {
            if let Some(pos) = self.filtered.flat.iter().position( | v | *v == *current_image) {
                if pos > 0 {
                    self.set_current_image(cx, self.filtered.flat[pos - 1].clone());
                    self.last_flip = Instant::now();
                }
            }
        }
    }
    
    fn set_current_image_by_item_id_and_row(&mut self, cx: &mut Cx, item_id: u64, row: usize) {
        self.ui.redraw(cx);
        if let Some(ImageListItem::ImageRow {prompt_hash: _, image_count, image_files}) = self.filtered.list.get(item_id as usize) {
            self.set_current_image(cx, image_files[row.min(*image_count)].clone());
            self.last_flip = Instant::now();
        }
    }
    
    fn update_todo_display(&mut self, cx: &mut Cx) {
        let mut todo = 0;
        for machine in &self.machines {
            if machine.running.is_some() {
                todo += 1;
            }
        }
        todo += self.todo.len();
        self.ui.get_label(id!(todo_label)).set_label(&format!("Todo {}", todo));
        self.ui.redraw(cx);
    }
    
    fn save_preset(&self) -> PromptPreset {
        PromptPreset {
            workflow: self.ui.get_drop_down(id!(workflow_dropdown)).get_selected_label(),
            width: self.ui.get_text_input(id!(settings_width.input)).get_text().parse::<u32>().unwrap_or(1344),
            height: self.ui.get_text_input(id!(settings_height.input)).get_text().parse::<u32>().unwrap_or(768),
            steps: self.ui.get_text_input(id!(settings_steps.input)).get_text().parse::<u32>().unwrap_or(20),
            base_cfg: self.ui.get_text_input(id!(settings_base_cfg.input)).get_text().parse::<f64>().unwrap_or(8.5),
            refiner_cfg: self.ui.get_text_input(id!(settings_refiner_cfg.input)).get_text().parse::<f64>().unwrap_or(8.5),
            positive_score: self.ui.get_text_input(id!(settings_pos_score.input)).get_text().parse::<f64>().unwrap_or(6.0),
            negative_score: self.ui.get_text_input(id!(settings_neg_score.input)).get_text().parse::<f64>().unwrap_or(2.0),
            base_start_step: self.ui.get_text_input(id!(settings_base_start_step.input)).get_text().parse::<u32>().unwrap_or(0),
            base_end_step: self.ui.get_text_input(id!(settings_base_end_step.input)).get_text().parse::<u32>().unwrap_or(20),
            refiner_start_step: self.ui.get_text_input(id!(settings_refiner_start_step.input)).get_text().parse::<u32>().unwrap_or(20),
            refiner_end_step: self.ui.get_text_input(id!(settings_refiner_end_step.input)).get_text().parse::<u32>().unwrap_or(1000),
            upscale_start_step: self.ui.get_text_input(id!(settings_upscale_start_step.input)).get_text().parse::<u32>().unwrap_or(20),
            upscale_end_step: self.ui.get_text_input(id!(settings_upscale_end_step.input)).get_text().parse::<u32>().unwrap_or(1000),
            upscale_steps: self.ui.get_text_input(id!(settings_upscale_steps.input)).get_text().parse::<u32>().unwrap_or(31),
            scale: self.ui.get_text_input(id!(settings_scale.input)).get_text().parse::<f64>().unwrap_or(0.5),
            total_steps: self.ui.get_text_input(id!(settings_total_steps.input)).get_text().parse::<u32>().unwrap_or(20),
        }
    }
    
    fn load_preset(&self, preset: &PromptPreset) {
        self.ui.get_drop_down(id!(workflow_dropdown)).set_selected_by_label(&preset.workflow);
        self.ui.get_text_input(id!(settings_width.input)).set_text(&format!("{}", preset.width));
        self.ui.get_text_input(id!(settings_height.input)).set_text(&format!("{}", preset.height));
        self.ui.get_text_input(id!(settings_steps.input)).set_text(&format!("{}", preset.steps));
        self.ui.get_text_input(id!(settings_base_cfg.input)).set_text(&format!("{}", preset.base_cfg));
        self.ui.get_text_input(id!(settings_refiner_cfg.input)).set_text(&format!("{}", preset.refiner_cfg));
        self.ui.get_text_input(id!(settings_pos_score.input)).set_text(&format!("{}", preset.positive_score));
        self.ui.get_text_input(id!(settings_neg_score.input)).set_text(&format!("{}", preset.negative_score));
        self.ui.get_text_input(id!(settings_base_start_step.input)).set_text(&format!("{}", preset.base_start_step));
        self.ui.get_text_input(id!(settings_base_end_step.input)).set_text(&format!("{}", preset.base_end_step));
        self.ui.get_text_input(id!(settings_refiner_start_step.input)).set_text(&format!("{}", preset.refiner_start_step));
        self.ui.get_text_input(id!(settings_refiner_end_step.input)).set_text(&format!("{}", preset.refiner_end_step));
        self.ui.get_text_input(id!(settings_upscale_start_step.input)).set_text(&format!("{}", preset.upscale_start_step));
        self.ui.get_text_input(id!(settings_upscale_end_step.input)).set_text(&format!("{}", preset.upscale_end_step));
        self.ui.get_text_input(id!(settings_upscale_steps.input)).set_text(&format!("{}", preset.upscale_steps));
        self.ui.get_text_input(id!(settings_scale.input)).set_text(&format!("{}", preset.scale));
        self.ui.get_text_input(id!(settings_total_steps.input)).set_text(&format!("{}", preset.total_steps));
    }
    
    fn render(&mut self, cx: &mut Cx, batch_size: usize) {
        let positive = self.ui.get_text_input(id!(positive)).get_text();
        let negative = self.ui.get_text_input(id!(negative)).get_text();
        
        //self.todo.clear();
        if batch_size != 1 {
            self.last_seed = LiveId::from_str(&format!("{:?}", Instant::now())).0;
            self.update_seed_display(cx);
        }
        for i in 0..batch_size {
            self.send_prompt(cx, PromptState {
                //total_steps: self.ui.get_text_input(id!(settings_total.input)).get_text().parse::<usize>().unwrap_or(32),
                prompt: Prompt {
                    positive: positive.clone(),
                    negative: negative.clone(),
                    preset: self.save_preset()
                },
                //workflow: workflow.clone(),
                seed: self.last_seed as u64
            });
            if batch_size != 1 {
                self.last_seed = LiveId::from_str(&format!("{:?}", Instant::now())).0 + i as u64;
                self.update_seed_display(cx);
            }
        }
        // lets update the queuedisplay
        self.update_todo_display(cx);
    }
    
    fn set_slide_show(&mut self, cx: &mut Cx, check: bool) {
        let check_box = self.ui.get_check_box(id!(slide_show_check_box));
        check_box.set_selected(cx, check);
        if check {
            self.last_flip = Instant::now();
        }
    }
    
    fn handle_slide_show(&mut self, cx: &mut Cx) {
        // lets get the slideshow values
        if self.ui.get_check_box(id!(slide_show_check_box)).selected(cx) {
            let time = self.ui.get_drop_down(id!(slide_show_dropdown)).get_selected_label().parse::<f64>().unwrap_or(0.0);
            // ok lets check our last-change instant
            if Instant::now() - self.last_flip > Duration::from_millis((time * 1000.0)as u64) {
                self.select_prev_image(cx);
            }
        }
    }
    
    fn update_render_todo(&mut self, cx: &mut Cx) {
        while self.machines.iter().find( | v | v.running.is_none()).is_some() && self.todo.len()>0 {
            let prompt = self.todo.pop().unwrap();
            self.send_prompt(cx, prompt);
        }
        self.update_todo_display(cx);
    }
    
    fn play(&mut self, cx: &mut Cx) {
        self.set_current_image_by_item_id_and_row(cx, 0, 0);
        self.ui.get_list_view(id!(image_list)).set_first_id_and_scroll(0, 0.0);
        self.set_slide_show(cx, true);
        
    }
    
    fn handle_network_response(&mut self, cx: &mut Cx, event: &Event) {
        let image_list = self.ui.get_list_view(id!(image_list));
        for event in event.network_responses() {
            match &event.response {
                NetworkResponse::WebSocketString(s) => {
                    if s.contains("execution_interrupted") {
                        
                    }
                    else if s.contains("execution_error") { // i dont care to expand the json def for this one
                        log!("Got execution error for {} {}", event.request_id, s);
                    }
                    else {
                        match ComfyUIMessage::deserialize_json(&s) {
                            Ok(data) => {
                                if data._type == "status" {
                                    if let Some(status) = data.data.status {
                                        if status.exec_info.queue_remaining == 0 {
                                            if let Some(machine) = self.machines.iter_mut().find( | v | {v.id == event.request_id}) {
                                                machine.running = None;
                                                Self::update_progress(cx, &self.ui, event.request_id, false, 0, 1);
                                            }
                                            self.update_render_todo(cx);
                                        }
                                    }
                                }
                                else if data._type == "executed" {
                                    if let Some(output) = &data.data.output {
                                        if let Some(image) = output.images.first() {
                                            if let Some(machine) = self.machines.iter_mut().find( | v | {v.id == event.request_id}) {
                                                if let Some(running) = machine.running.take() {
                                                    self.ui.get_text_input(id!(settings_total_steps.input)).set_text(&format!("{}", running.steps_counter));
                                                    machine.fetching = Some(running);
                                                    Self::update_progress(cx, &self.ui, event.request_id, false, 0, 1);
                                                    self.fetch_image(cx, event.request_id, &image.filename);
                                                }
                                                self.update_render_todo(cx);
                                            }
                                        }
                                    }
                                }
                                else if data._type == "progress" {
                                    // draw the progress bar / progress somewhere
                                    if let Some(machine) = self.machines.iter_mut().find( | v | {v.id == event.request_id}) {
                                        if let Some(running) = &mut machine.running {
                                            running.steps_counter += 1;
                                            Self::update_progress(cx, &self.ui, event.request_id, true, running.steps_counter, running.prompt_state.prompt.preset.total_steps as usize);
                                        }
                                    }
                                    //self.set_progress(cx, &format!("Step {}/{}", data.data.value.unwrap_or(0), data.data.max.unwrap_or(0)))
                                }
                            }
                            Err(err) => {
                                log!("Error parsing JSON {:?} {:?}", err, s);
                            }
                        }
                    }
                }
                NetworkResponse::WebSocketBinary(bin) => {
                    log!("Got Binary {}", bin.len());
                }
                NetworkResponse::HttpResponse(res) => {
                    // alright we got an image back
                    match event.request_id {
                        live_id!(prompt) => if let Some(_data) = res.get_string_body() { // lets check if the prompt executed
                        }
                        live_id!(image) => if let Some(data) = res.get_body() {
                            if let Some(machine) = self.machines.iter_mut().find( | v | {v.id == res.metadata_id}) {
                                if let Some(mut fetching) = machine.fetching.take() {
                                    
                                    // lets write our image to disk properly
                                    //self.current_image = Some(
                                    fetching.prompt_state.prompt.preset.total_steps = fetching.steps_counter as u32;
                                    let image_id = self.db.add_png_and_prompt(fetching.prompt_state, data);
                                    // scroll by one item
                                    let first_id = image_list.first_id();
                                    if first_id != 0 {
                                        image_list.set_first_id(first_id + 1);
                                    }
                                    
                                    self.filtered.filter_db(&self.db, "", false);
                                    
                                    if self.db.get_image_texture(&image_id).is_some() {
                                        self.ui.redraw(cx);
                                    }
                                    
                                }
                            }
                        }
                        live_id!(clear_queue) => {}
                        live_id!(interrupt) => {}
                        _ => panic!()
                    }
                }
                e => {
                    log!("{} {:?}", event.request_id, e)
                }
            }
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        let image_list = self.ui.get_list_view(id!(image_list));
        
        if self.db.handle_decoded_images(cx) {
            self.ui.redraw(cx);
        }
        
        if let Event::Timer(_te) = event {
            self.handle_slide_show(cx);
        }
        if let Event::Draw(event) = event {
            let cx = &mut Cx2d::new(cx, event);
            /*
        if let Some(change) = self.ui.get_text_input(id!(positive)).changed(&actions) {
            self.ui.get_label(id!(second_image.prompt)).set_label(&change);
            self.ui.redraw(cx);
        }*/
            
            
            if let Some(current_image) = &self.current_image {
                let tex = self.db.get_image_texture(current_image);
                if tex.is_some() {
                    self.ui.get_image(id!(image_view.image)).set_texture(tex.clone());
                    self.ui.get_image(id!(big_image.image1)).set_texture(tex.clone());
                    self.ui.get_image(id!(second_image.image1)).set_texture(tex);
                }
            }
            
            while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                
                if let Some(mut image_list) = image_list.has_widget(&next).borrow_mut() {
                    // alright now we draw the items
                    image_list.set_item_range(0, self.filtered.list.len() as u64, 1);
                    while let Some(item_id) = image_list.next_visible_item(cx) {
                        if let Some(item) = self.filtered.list.get(item_id as usize) {
                            match item {
                                ImageListItem::Prompt {prompt_hash} => {
                                    let group = self.db.prompt_files.iter().find( | v | v.prompt_hash == *prompt_hash).unwrap();
                                    let item = image_list.get_item(cx, item_id, live_id!(PromptGroup)).unwrap();
                                    item.get_label(id!(prompt)).set_label(&group.prompt.positive);
                                    item.draw_widget_all(cx);
                                }
                                ImageListItem::ImageRow {prompt_hash: _, image_count, image_files} => {
                                    let item = image_list.get_item(cx, item_id, id!(Empty.ImageRow1.ImageRow2)[*image_count]).unwrap();
                                    let rows = item.get_view_set(ids!(row1, row2, row3));
                                    for (index, row) in rows.iter().enumerate() {
                                        if index >= *image_count {break}
                                        // alright we need to query our png cache for an image.
                                        let tex = self.db.get_image_texture(&image_files[index]);
                                        row.get_image(id!(img)).set_texture(tex);
                                    }
                                    item.draw_widget_all(cx);
                                }
                            }
                        }
                    }
                }
            }
            return
        }
        
        self.handle_network_response(cx, event);
        
        let actions = self.ui.handle_widget_event(cx, event);
        
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::ReturnKey|KeyCode::NumpadEnter, modifiers, ..}) = event {
            if modifiers.logo || modifiers.control {
                self.clear_todo(cx);
            }
            if modifiers.shift {
                self.render(cx, 1);
            }
            else {
                let batch_size = self.ui.get_drop_down(id!(batch_mode_dropdown)).get_selected_label().parse::<usize>().unwrap();
                self.render(cx, batch_size);
            }
        }
        
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::Backspace, modifiers, ..}) = event {
            if modifiers.logo {
                let prompt_hash = self.prompt_hash_from_current_image();
                self.load_inputs_from_prompt_hash(cx, prompt_hash);
                self.load_seed_from_current_image(cx);
            }
        }
        
        
        
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::KeyC, modifiers, ..}) = event {
            if modifiers.control || modifiers.logo {
                self.clear_todo(cx);
            }
        }
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::KeyR, modifiers, ..}) = event {
            if modifiers.control || modifiers.logo {
                self.open_web_socket(cx);
            }
        }
        
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::KeyP, modifiers, ..}) = event {
            if modifiers.control || modifiers.logo {
                let prompt_frame = self.ui.get_view(id!(second_image.prompt_frame));
                if prompt_frame.visible() {
                    prompt_frame.set_visible(false);
                }
                else {
                    //cx.set_cursor(MouseCursor::Hidden);
                    prompt_frame.set_visible(true);
                }
                self.ui.redraw(cx);
            }
        }
        
        
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::Escape, ..}) = event {
            let big_image = self.ui.get_view(id!(big_image));
            if big_image.visible() {
                big_image.set_visible(false);
            }
            else {
                //cx.set_cursor(MouseCursor::Hidden);
                big_image.set_visible(true);
            }
            self.ui.redraw(cx);
        }
        
        if self.ui.get_button(id!(play_button)).clicked(&actions) {
            self.play(cx);
        }
        if let Event::KeyDown(KeyEvent {is_repeat: false, key_code: KeyCode::Home, modifiers, ..}) = event {
            if self.ui.get_view(id!(big_image)).visible() || modifiers.logo {
                self.play(cx);
            }
        }
        if let Event::KeyDown(KeyEvent {key_code: KeyCode::ArrowDown, modifiers, ..}) = event {
            if self.ui.get_view(id!(big_image)).visible() || modifiers.logo {
                self.select_next_image(cx);
                self.set_slide_show(cx, false);
            }
        }
        if let Event::KeyDown(KeyEvent {key_code: KeyCode::ArrowUp, modifiers, ..}) = event {
            if self.ui.get_view(id!(big_image)).visible() || modifiers.logo {
                self.select_prev_image(cx);
                self.set_slide_show(cx, false);
            }
        }
        
        if let Some(ke) = self.ui.get_view_set(ids!(image_view, big_image)).key_down(&actions) {
            match ke.key_code {
                KeyCode::ArrowDown => {
                    self.select_next_image(cx);
                    self.set_slide_show(cx, false);
                }
                KeyCode::ArrowUp => {
                    self.select_prev_image(cx);
                    self.set_slide_show(cx, false);
                }
                _ => ()
            }
        }
        
        
        if let Event::KeyDown(KeyEvent {key_code: KeyCode::ArrowLeft, modifiers, ..}) = event {
            if self.ui.get_view(id!(big_image)).visible() || modifiers.logo {
                self.set_slide_show(cx, false);
            }
        }
        if let Event::KeyDown(KeyEvent {key_code: KeyCode::ArrowRight, modifiers, ..}) = event {
            if self.ui.get_view(id!(big_image)).visible() || modifiers.logo {
                self.set_slide_show(cx, true);
            }
        }
        
        if self.ui.get_button(id!(render_batch)).clicked(&actions) {
            let batch_size = self.ui.get_drop_down(id!(batch_mode_dropdown)).get_selected_label().parse::<usize>().unwrap();
            self.render(cx, batch_size);
            self.ui.redraw(cx);
        }
        
        if self.ui.get_button(id!(render_single)).clicked(&actions) {
            self.render(cx, 1);
            self.ui.redraw(cx);
        }
        
        if self.ui.get_button(id!(cancel_todo)).clicked(&actions) {
            self.clear_todo(cx);
            self.ui.redraw(cx);
        }
        
        if let Some(change) = self.ui.get_text_input(id!(search)).changed(&actions) {
            self.filtered.filter_db(&self.db, &change, false);
            self.ui.redraw(cx);
            image_list.set_first_id_and_scroll(0, 0.0);
        }
        
        
        
        if let Some(e) = self.ui.get_view(id!(image_view)).finger_down(&actions) {
            if e.tap_count >1 {
                self.ui.get_view(id!(big_image)).set_visible(true);
                self.ui.redraw(cx);
            }
        }
        
        if let Some(e) = self.ui.get_view(id!(big_image)).finger_down(&actions) {
            if e.tap_count >1 {
                self.ui.get_view(id!(big_image)).set_visible(false);
                self.ui.redraw(cx);
            }
        }
        
        for (item_id, item) in image_list.items_with_actions(&actions) {
            // check for actions inside the list item
            let rows = item.get_view_set(ids!(row1, row2));
            for (row_index, row) in rows.iter().enumerate() {
                if let Some(fd) = row.finger_down(&actions) {
                    self.set_current_image_by_item_id_and_row(cx, item_id, row_index);
                    //self.set_slide_show(cx, false);
                    if fd.tap_count == 2 {
                        if let ImageListItem::ImageRow {prompt_hash, ..} = self.filtered.list[item_id as usize] {
                            self.load_seed_from_current_image(cx);
                            self.load_inputs_from_prompt_hash(cx, prompt_hash);
                        }
                    }
                }
            }
            if let Some(fd) = item.as_view().finger_down(&actions) {
                if fd.tap_count == 2 {
                    if let ImageListItem::Prompt {prompt_hash} = self.filtered.list[item_id as usize] {
                        self.load_inputs_from_prompt_hash(cx, prompt_hash);
                    }
                }
            }
        }
    }
}
