use makepad_widgets::*;
use makepad_platform::live_atomic::*;


live_design!{
    use link::theme::*;
    use link::shaders::*;
    use link::widgets::*;
    use makepad_widgets::vectorline::*;
    use makepad_example_ui_zoo::demofiletree::*;

    COLOR_CONTAINER = (THEME_COLOR_D_1)
    COLOR_ACCENT = (THEME_COLOR_MAKEPAD)

    DEMO_COLOR_1 = #8f0
    DEMO_COLOR_2 = #0f8
    DEMO_COLOR_3 = #80f

    ZooTitle = <View> {
        width: Fill, height: Fit,
        flow: Down,
        align: { x: 0.0, y: 0.5},
        margin: <THEME_MSPACE_3> {},
        spacing: 10.,
        show_bg: false,
        title = <H2> { text: "Makepad UI Zoo" }
    }

    ZooHeader = <View> {
        width: Fill, height: Fit,
        flow: Down,
        spacing: (THEME_SPACE_1),
         margin: <THEME_MSPACE_H_3> {}
        divider = <Hr> { }
        title = <H3> { text: "Header" }
    }

    ZooGroup = <RoundedView> {
        height: Fit, width: Fill,
        flow: Right,
        align: { x: 0.0, y: 0.5},
        margin: 0.,
        show_bg: false;
        draw_bg: { color: (COLOR_CONTAINER) }
    }

    ZooDesc = <P> { text: "" }

    ZooBlock = <RoundedView> {
        width: 50., height: 50.
        margin: 0.,
        spacing: 0.,

        show_bg: true;
        draw_bg: {
            fn get_color(self) -> vec4 {
                return mix(self.color, self.color*0.5, self.pos.y);
            }
            radius: (THEME_CONTAINER_CORNER_RADIUS)
        }
    }

    App = {{App}} {
        ui: <Window> {
            width: Fill, height: Fill,
            show_bg: true,
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return (THEME_COLOR_BG_APP);
                }
            }

            caption_bar = {
                visible: true,
                margin: {left: -100},
                caption_label = { label = {text: "Makepad UI Zoo "} }
            },

            body = <View> {
                width: Fill, height: Fill,
                flow: Down,
                spacing: 10.,
                margin: 0.,
                scroll_bars: <ScrollBars> {}

                <ZooTitle> {}

                <ZooHeader> {
                    title = {text: "Intro"}
                    <ZooDesc> {
                        text: "Intro."
                    }
                    <View> {
                        width: Fill, height: Fit,
                        flow: Down,
                        <P> { text: "- Shader-based: what does that mean for how things work." }
                        <P> { text: "- Inheritance mechanisms in the DSL." }
                        <P> { text: "- Introduction to the layout system." }
                        <P> { text: "- Base theme parameters." }
                        <P> { text: "- Typographic system. Base font-size and contrast." }
                        <P> { text: "- Space constants to control denseness of the design." }
                        <P> { text: "- Transparency mechanism of the widgets. Nesting for structure." }
                    }
                }

                <ZooHeader> {
                    title = {text: "Control Heights & Text Baselines"}
                    <ZooDesc> {
                        text: "Control heights and text baselines"
                    }
                    <View> {
                        width: Fill, height: Fit,
                        align: { x: 0., y: 0.}
                        flow: Right,
                        spacing: (THEME_SPACE_2)
                        <P> { text: "TestLabel", width: Fit}
                        <Vr> {} 
                        <LinkLabel> { text: "TestButton", width: Fit}
                        <CheckBox> { text: "TestButton"}
                        <CheckBoxToggle> { text: "TestButton"}
                        <ButtonFlat> { text: "TestButton"}
                        <Button> { text: "TestButton"}
                        <TextInput> { text: "TestButton"}
                        <DropDown> { }
                        <Slider> { text: "TestButton"}
                        <SliderBig> { text: "TestButton"}
                        // <RadioButton> { }
                        // <RadioButtonTextual> { }
                        // <RadioButtonTab> { }
                    }
                }

                <ZooHeader> {
                    title = {text: "Typography"}
                    <ZooDesc> {
                        text: "Typography."
                    }
                    <View> {
                        width: Fill, height: Fit,
                        flow: Down,

                        <H1> { text: "H1 headline" }
                        <H1italic> { text: "H1 italic headline" }
                        <H2> { text: "H2 headline" }
                        <H2italic> { text: "H2 italic headline" }
                        <H3> { text: "H3 headline" }
                        <H3italic> { text: "H3 italic headline" }
                        <H4> { text: "H4 headline" }
                        <H4italic> { text: "H4 italic headline" }
                        <P> { text: "P copy text" }
                        <Pitalic> { text: "P italic copy text" }
                        <Pbold> { text: "P bold copy text" }
                        <Pbolditalic> { text: "P bold italic copy text" }
                    }
                }

                <ZooHeader> {
                    title = {text: "<View>" }
                    <ZooDesc> {text:"This is a gray view with flow set to Right\nTo show the extend, the background has been enabled using show_bg and a gray pixelshader has been provided to draw_bg."}
                    <View> {
                        height: Fit
                        flow: Right,
                        show_bg: true,
                        draw_bg: { color: (COLOR_CONTAINER) }
                        padding: 10.
                        spacing: 10.
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_1)}}
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_3)}}
                    }

                    <ZooDesc> {text:"This utlizes a <Filler> to separate items."}
                    <View> {
                        height: Fit
                        flow: Right,
                        show_bg: true,
                        draw_bg: { color: (COLOR_CONTAINER) }
                        padding: 10.
                        spacing: 10.
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_1)}}
                        <Filler> {}
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_3)}}
                    }

                    <ZooDesc> { text:"This is a view with flow set to Down" }
                    <View> {
                        height: Fit,
                        flow: Down,
                        padding: 10.
                        spacing: 10.
                        show_bg: true,
                        draw_bg: { color: (COLOR_CONTAINER) }
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_1)}}
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_3)}}
                    }

                    <ZooDesc> {text:"This view is bigger on the inside"}
                    <View> {
                        width: 150, height: 150,
                        flow: Right,
                        padding: 10.
                        spacing: 10.

                        show_bg: true,
                        draw_bg: { color: (COLOR_CONTAINER) }
                        scroll_bars: <ScrollBars> {}

                        <View> {
                            width: Fit, height: Fit,
                            flow: Down,
                            show_bg: false,
                            spacing: 10
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_1)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_3)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                        }

                        <View> {
                            width: Fit, height: Fit,
                            flow: Down,
                            show_bg: false,
                            spacing: 10
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_1)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_3)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                        }

                        <View> {
                            width: Fit, height: Fit,
                            flow: Down,
                            show_bg: false,
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_1)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_3)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                        }

                        <View> {
                            width: Fit, height: Fit,
                            flow: Down,
                            show_bg: false,
                            spacing: 10
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_1)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_3)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                        }

                        <View> {
                            width: Fit, height: Fit,
                            flow: Down,
                            show_bg: false,
                            spacing: 10
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_1)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                            <ZooBlock> {draw_bg:{color: (DEMO_COLOR_3)}}
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"RoundedView"}
                    <ZooDesc> {
                        text:"This is a Rounded View. Please note that the radius has to be represented as a float value (with a decimal point) to work. Also note that instead of replacing the main pixel shader - you now replace get_color instead so the main shader can take care of rendering the radius."
                    }
                    <RoundedView> {
                        height: Fit
                        flow: Right,
                        padding: 10
                        spacing: 10
                        show_bg: true,
                        draw_bg: {
                            color: (COLOR_CONTAINER),
                            radius: 10.
                        }
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_1)}}
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_2)}}
                        <ZooBlock> {draw_bg:{color: (DEMO_COLOR_3)}}
                    }
                }

                <ZooHeader> {
                    title = {text:"<Button>"}
                    <ZooDesc> {text:"A small clickable region"}
                    <ZooGroup> {
                        flow: Down,
                        width: Fill, height: Fit,
                        align: { x: 0.0, y: 0.5 }
                        spacing: 10.,

                        <H4> { text: "Default"}
                        <Label> { text: "<Button>"}
                        basicbutton = <Button> { text: "I can be clicked" }

                        <H4> { text: "Button with an icon"}
                        <Label> { text: "<ButtonIcon>"}
                        iconbutton = <ButtonIcon> {
                            draw_icon: {
                                color: #f00,
                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                            }
                            text: "I can have a icon!"
                        }

                        <H4> { text: "Flat Mode"}
                        <Label> { text: "<ButtonFlat>"}
                        <View> {
                            flow: Right,
                            align: { x: 0., y: 0.5 }
                            width: Fill, height: Fit,
                            <ButtonFlat> {
                                draw_icon: {
                                    color: #f00,
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }
                                text: "I can have a lovely icon!"
                            }

                            <ButtonFlat> {
                                draw_icon: {
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }
                            }

                            <ButtonFlat> {
                                flow: Down,
                                icon_walk: { width: 15. }
                                draw_icon: {
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }
                                text: "Vertical Layout"
                            }
                        }

                        <H4> { text: "Freely styled button"}
                        <Label> { text: "<Button>"}
                        styledbutton = <Button> {
                            draw_bg: {
                                fn pixel(self) -> vec4 {
                                    return (THEME_COLOR_MAKEPAD) + self.pressed * vec4(1., 1., 1., 1.)
                                }
                            }
                            draw_text: {
                                fn get_color(self) -> vec4 {
                                    return (THEME_COLOR_U_5) - vec4(0., 0.1, 0.4, 0.) * self.hover - self.pressed * vec4(1., 1., 1., 0.);
                                }
                            }
                            text: "I can be styled!"
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<TextInput>"}
                    <ZooDesc> { text:"Simple 1 line textbox" }
                    <ZooGroup> {
                        flow: Down,
                        spacing: (THEME_SPACE_1)
                        <View> {
                            height: Fit, width: Fill,
                            spacing: (THEME_SPACE_2),
                            <H4> { text: "Default", width: 175.}
                            simpletextinput = <TextInput> { }
                            simpletextinput_outputbox = <P> {
                                text: "Output"
                            }
                        }
                        <View> {
                            height: Fit, width: Fill,
                            spacing: (THEME_SPACE_2),
                            <H4> { text: "Inline Label", width: 175.}
                            <TextInput> { empty_message: "Inline Label" }
                        }
                        // <View> {
                        //     height: Fit, width: Fill,
                        //     spacing: (THEME_SPACE_2),
                        //     <H4> { text: "Secret", width: 175.}
                        //     <TextInput> { text: "1234567", empty_message: "Password", secret: true }
                        // }
                        // <View> {
                        //     height: Fit, width: Fill,
                        //     spacing: (THEME_SPACE_2),
                        //     <H4> { text: "On focus select all", width: 175.}
                        //     <TextInput> { text: "Lorem Ipsum", empty_message: "Inline Label", on_focus_select_all: true }
                        // }
                        // <View> {
                        //     height: Fit, width: Fill,
                        //     spacing: (THEME_SPACE_2),
                        //     <H4> { text: "Read only", width: 175.}
                        //     <TextInput> { text: "You can't change me", read_only: true }
                        // }
                        // <View> {
                        //     height: Fit, width: Fill,
                        //     spacing: (THEME_SPACE_2),
                        //     <H4> { text: "ASCII only", width: 175.}
                        //     <TextInput> { empty_message: "No fancy characters", ascii_only: true }
                        // }
                        // <View> {
                        //     height: Fit, width: Fill,
                        //     spacing: (THEME_SPACE_2),
                        //     <H4> { text: "Double Tap start", width: 175.}
                        //     <TextInput> { empty_message: "Click twice", double_tap_start: TODO: UNCLEAR WHAT VALUE THIS NEEDS }
                        // }
                    }
                }

                <ZooHeader> {
                    title = {text:"<Label>"}
                    <ZooDesc> { text:"Default single line textbox" }
                    <ZooGroup> { <Label> { text: "This is a small line of text" } }
                    <ZooGroup> {
                        <Label> {
                            draw_text: {
                                color: (COLOR_ACCENT)
                                text_style: {
                                    font_size: 20,
                                }
                            },
                            text: "You can style text using colors and fonts"
                        }
                    }
                    <ZooGroup> {
                        <Label> {
                            draw_text: {
                                fn get_color(self) ->vec4{
                                    return mix((COLOR_ACCENT), (THEME_COLOR_U_HIDDEN), self.pos.x)
                                }
                                color: (THEME_COLOR_MAKEPAD)
                                text_style: {
                                    font_size: 40.,
                                }
                            },
                            text: "OR EVEN SOME PIXELSHADERS"
                        }
                    }
                }

                <ZooHeader> {
                    title = { text:"<Slider>" }
                    <ZooDesc> { text:"A parameter dragger" }
                    <ZooGroup> {
                        width: Fill, height: Fit,
                        flow: Right,
                        spacing: 10.0,
                        align: { x: 0., y: 0.}
                        <View> {
                            width: Fill, height: Fit,
                            flow: Down,
                            <Slider> { text: "Default" }
                            <Slider> { text: "label_align", label_align: { x: 0.5, y: 0. } }
                            <Slider> { text: "min/max", min: 0., max: 100. }
                            <Slider> { text: "precision", precision: 20 }
                            <Slider> { text: "stepped", step: 0.1 }
                        }
                        <View> {
                            width: Fill, height: Fit,
                            flow: Down,
                            <SliderBig> { text: "Default" }
                            <SliderBig> { text: "label_align", label_align: { x: 0.5, y: 0. } }
                            <SliderBig> { text: "min/max", min: 0., max: 100. }
                            <SliderBig> { text: "precision", precision: 20 }
                            <SliderBig> { text: "stepped", step: 0.1 }
                        }
                        <View> {
                            width: Fill, height: Fit,
                            flow: Down,
                            <SliderCompact> {
                                text: "Colored",
                                draw_slider: {
                                    val_color_a: (#FFCC00),
                                    val_color_b: #f00,
                                    handle_color_a: #0,
                                    handle_color_b: #0,
                               }
                            }
                            <SliderCompact> {
                                text: "Solid",
                                draw_text: {
                                    color: #0ff;
                                }
                                draw_slider: {
                                    val_color_a: #f08,
                                    val_color_b: #f08,
                                    handle_color_a: #FFFF,
                                    handle_color_b: #FFF0,
                                }
                            }
                            <SliderCompact> {
                                text: "Solid",
                                draw_slider: {
                                    val_color_a: #6,
                                    val_color_b: #6,
                                    handle_color_a: #0,
                                    handle_color_b: #C,
                                }
                            }
                            <SliderCompact> { text: "min/max", min: 0., max: 100. }
                            <SliderCompact> { text: "precision", precision: 20 }
                            <SliderCompact> { text: "stepped", step: 0.1 }
                            <SliderCompact> {
                                text: "label_size",
                                draw_slider: {label_size: 150. },
                            }
                        }
                        <View> {
                            width: Fill, height: Fit,
                            flow: Down,
                            <Rotary> {
                                text: "Colored",
                                draw_slider: {
                                    val_color_a: (#FFCC00),
                                    val_color_b: #f00,
                                    handle_color_a: #0,
                                    handle_color_b: #0,
                               }
                            }
                            <Rotary> {
                                text: "Solid",
                                draw_text: {
                                    color: #0ff;
                                }
                                draw_slider: {
                                    val_color_a: #f08,
                                    val_color_b: #f08,
                                    handle_color_a: #FFFF,
                                    handle_color_b: #FFF0,
                                }
                            }
                            <Rotary> {
                                text: "Solid",
                                draw_slider: {
                                    val_color_a: #6,
                                    val_color_b: #6,
                                    handle_color_a: #0,
                                    handle_color_b: #C,
                                }
                            }
                            <Rotary> { text: "min/max", min: 0., max: 100. }
                            <Rotary> { text: "precision", precision: 20 }
                            <Rotary> { text: "stepped", step: 0.1 }
                            <Rotary> {
                                text: "label_size",
                                draw_slider: {label_size: 150. },
                            }
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<DropDown>"}
                    title = {text:"<DropDown>"}
                    <ZooDesc> {text:"DropDown control. This control currently needs to be databound which needs some plumbing. In this sample there is a binding context struct in the main app struct - which gets bound on app start - and updated during handle_actions."}
                    <ZooGroup> {
                        dropdown = <DropDown> {
                            labels: ["Value One", "Value Two", "Thrice", "Fourth Value", "Option E", "Hexagons"],
                            values: [ValueOne, ValueTwo, Thrice, FourthValue, OptionE, Hexagons]
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<FileTree>"}
                    <ZooDesc> {text:"File Tree"}
                    <ZooGroup> {
                        <DemoFileTree> { file_tree:{ height: 400. } }
                    }
                }

                // <ZooHeader> {
                //     title = { text:"<FoldHeader>" }
                //     <ZooDesc> { text:"This widget allows you to have a header with a foldbutton (has to be named fold_button for the magic to work)" }
                //     <ZooGroup> {
                //         thefoldheader= <FoldHeader> {
                //             header: <View> {
                //                 height: Fit
                //                 align: {x: 0., y: 0.5}
                //                 fold_button = <FoldButton> {} <P> {text: "Fold me!"}
                //             }
                //             body: <View> {
                //                 width: Fill, height: Fit
                //                 show_bg: false,
                //                 padding: 5.0,
                //                 <P> { text:"This is the body that can be folded away" }
                //             }
                //         }
                //     }
                // }

                <ZooHeader> {
                    title = {text:"<Html>"}
                    <ZooDesc> {text:"HTML Widget"}
                    <ZooGroup> {
                        <Html> {
                            width:Fill, height:Fit,
                            body:"<H1>H1 Headline</H1><H2>H2 Headline</H2><H3>H3 Headline</H3><H4>H4 Headline</H4><H5>H5 Headline</H5><H6>H6 Headline</H6>This is <b>bold</b>&nbsp;and <i>italic text</i>.<sep><b><i>Bold italic</i></b>, <u>underlined</u>, and <s>strike through</s> text. <p>This is a paragraph</p> <code>A code block</code>. <br/> And this is a <a href='https://www.google.com/'>link</a><br/><ul><li>lorem</li><li>ipsum</li><li>dolor</li></ul><ol><li>lorem</li><li>ipsum</li><li>dolor</li></ol><br/> <blockquote>Blockquote</blockquote> <pre>pre</pre><sub>sub</sub><del>del</del>"
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<Markdown>"}
                    <ZooDesc> {text:"Markdown"}
                    <ZooGroup> {
                        <Markdown> {
                            width:Fill, height: Fit,
                            body:"# Headline 1 \n ## Headline 2 \n ### Headline 3 \n #### Headline 4 \n This is standard text with a  \n\n line break a short ~~strike through~~ demo.\n\n *Italic text* \n\n **Bold text** \n\n - Bullet\n - Another bullet\n\n - Third bullet\n\n 1. Numbered list Bullet\n 2. Another list entry\n\n 3. Third list entry\n\n `Monospaced text`\n\n> This is a quote.\n\nThis is `inline code`.\n\n ```code block```"
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<Image>"}
                    <ZooDesc> {text:"A static inline image from a resource."}
                    <ZooGroup> {
                        height: Fit, width: Fill,
                        spacing: (THEME_SPACE_2)
                        scroll_bars: <ScrollBars> {}
                        <View> {
                            width: Fit, height: Fit, flow: Down,
                            <View> {
                                show_bg: true, draw_bg: { color: (THEME_COLOR_BG_CONTAINER)}, width: 125, height: 250, flow: Down,
                                <Image> { source: dep("crate://self/resources/ducky.png" ) }
                            }
                            <P> { text: "Default" }
                        }
                        <View> {
                            width: Fit, height: Fit, flow: Down,
                            <View> {
                                show_bg: true, draw_bg: { color: (THEME_COLOR_BG_CONTAINER)}, width: 125, height: 250,
                                <Image> { height: Fill, source: dep("crate://self/resources/ducky.png" ), min_height: 100 }
                            }
                            <P> { text: "min_height: 100" } // TODO: get this to work correctly
                        }
                        <View> {
                            width: Fit, height: Fit, flow: Down,
                            <View> {
                                show_bg: true, draw_bg: { color: (THEME_COLOR_BG_CONTAINER)}, width: 125, height: 250,
                                <Image> { width: Fill, source: dep("crate://self/resources/ducky.png" ), width_scale: 1.1 }
                            }
                            <P> { text: "width_scale: 1.5" } // TODO: get this to work correctly
                        }
                        <View> {
                            width: Fit, height: Fit, flow: Down,
                            <View> {
                                show_bg: true, draw_bg: { color: (THEME_COLOR_BG_CONTAINER)}, width: 125, height: 250,
                                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png"), fit: Stretch }
                            }
                            <P> { text: "fit: Stretch" }
                        }
                        <View> {
                            width: Fit, height: Fit, flow: Down,
                            <View> {
                                show_bg: true, draw_bg: { color: (THEME_COLOR_BG_CONTAINER)}, width: 125, height: 250,
                                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Horizontal }
                            }
                            <P> { text: "fit: Horizontal" }
                        }
                        <View> {
                            width: Fit, height: Fit, flow: Down,
                            <View> {
                                show_bg: true, draw_bg: { color: (THEME_COLOR_BG_CONTAINER)}, width: 125, height: 250,
                                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Vertical }
                            }
                            <P> { text: "fit: Vertical" }
                        }
                        <View> {
                            width: Fit, height: Fit, flow: Down,
                            <View> {
                                show_bg: true, draw_bg: { color: (THEME_COLOR_BG_CONTAINER)}, width: 125, height: 250,
                                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Smallest }
                            }
                            <P> { text: "fit: Smallest" }
                        }
                        <View> {
                            width: Fit, height: Fit, flow: Down,
                            <View> {
                                show_bg: true, draw_bg: { color: (THEME_COLOR_BG_CONTAINER)}, width: 125, height: 250,
                                <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Biggest }
                            }
                            <P> { text: "fit: Biggest" }
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<LinkLabel>"}
                    <ZooDesc> {text:"Link Label"}
                    <ZooGroup> {
                        width: Fill, height: Fit,
                        flow: Down,
                        spacing: (THEME_SPACE_2)
                        <View> {
                            width: Fill, height: Fit,
                            spacing: (THEME_SPACE_2)
                            <LinkLabel> { text: "Click me!"}
                            <LinkLabel> { text: "Click me!"}
                            <LinkLabel> { text: "Click me!"}
                        }
                        <View> {
                            width: Fill, height: Fit,
                            spacing: (THEME_SPACE_2)
                            <LinkLabelIcon> {
                                text: "Click me!"
                                draw_icon: {
                                    color: #f00,
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }

                                icon_walk: {
                                    width: 12.5, height: Fit,
                                    margin: 0.0
                                }
                            }
                            <LinkLabelIcon> {
                                text: "Click me!"
                                draw_icon: {
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }

                                icon_walk: {
                                    width: 12.5,height: Fit,
                                    margin: 0.0
                                }
                            }
                            <LinkLabelIcon> {
                                text: "Click me!"
                                draw_icon: {
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }

                                icon_walk: {
                                    width: 12.5, height: Fit,
                                    margin: 0.0
                                }
                            }

                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<CheckBox>"}
                    <ZooDesc> {text:"Checkbox"}
                    <ZooGroup> {
                        height: Fit
                        spacing: (THEME_SPACE_2)
                        flow: Down,
                        <H4> { text: "Output demo"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5}
                            simplecheckbox = <CheckBox> {text:"Check me out!"}
                            simplecheckbox_output = <Label> { text:"hmm" }
                        }
                        <H4> { text: "Standard Mode"}
                        <View> {
                            height: Fit
                            flow: Right
                            spacing: (THEME_SPACE_1)
                            align: { x: 0.0, y: 0.5}
                            <CheckBox> {text:"Check me out!"}
                            <CheckBox> {text:"Check me out!"}
                            <CheckBox> {text:"Check me out!"}
                        }
                        <H4> { text: "Toggle Mode"}
                        <View> {
                            height: Fit
                            flow: Right
                            spacing: (THEME_SPACE_1)
                            align: { x: 0.0, y: 0.5}
                            <CheckBoxToggle> {text:"Check me out!" }
                            <CheckBoxToggle> {text:"Check me out!" }
                            <CheckBoxToggle> {text:"Check me out!" }
                        }
                        <H4> { text: "Custom Icon Mode"}
                        <View> {
                            height: Fit
                            flow: Right
                            spacing: (THEME_SPACE_1)
                            align: { x: 0.0, y: 0.5}
                            <CheckBoxCustom> {
                                text:"Check me out!"
                                draw_check: { check_type: None }
                                draw_icon: {
                                    color_active: #f00,
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }
                            }
                            <CheckBoxCustom> {
                                text:"Check me out!"
                                draw_check: { check_type: None }
                                draw_icon: {
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }
                            }
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<RadioButton>"}
                    <ZooDesc> {text:"Todo: List the different radio button templates."}
                    <ZooGroup> {
                        flow: Down,
                        spacing: (THEME_SPACE_2)
                        <H4> { text: "Default"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            radios_demo = <View> {
                                spacing: (THEME_SPACE_2)
                                width: Fit, height: Fit,
                                radio1 = <RadioButton> { text: "Option 1" }
                                radio2 = <RadioButton> { text: "Option 2" }
                                radio3 = <RadioButton> { text: "Option 3" }
                                radio4 = <RadioButton> { text: "Option 4" }
                            }
                        }

                        <H4> { text: "Custom Radios"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            iconradios_demo = <View> {
                                width: Fit, height: Fit,
                                spacing: (THEME_SPACE_2)
                                flow: Down,

                                radio1 = <RadioButtonCustom> {
                                    text: "Option 1"
                                    icon_walk: {
                                        width: 12.5, height: Fit,
                                    }
                                    draw_icon: {
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                }
                                radio2 = <RadioButtonCustom> {
                                    text: "Option 2"
                                    icon_walk: {
                                        width: 12.5, height: Fit,
                                    }
                                    draw_icon: {
                                        color_active: #0f0,
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                }
                                radio3 = <RadioButtonCustom> {
                                    text: "Option 3"
                                    icon_walk: {
                                        width: 12.5, height: Fit,
                                    }
                                    draw_icon: {
                                        color_active: #0ff,
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                }
                                radio4 = <RadioButtonCustom> {
                                    text: "Option 4"
                                    icon_walk: {
                                        width: 12.5, height: Fit,
                                    }
                                    draw_icon: {
                                        color_active: #f00,
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                }
                            }
                        }

                        <H4> { text: "Text only"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            textonlyradios_demo = <View> {
                                width: Fit, height: Fit,
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                radio1 = <RadioButtonTextual> { text: "Option 1" }
                                radio2 = <RadioButtonTextual> { text: "Option 2" }
                                radio3 = <RadioButtonTextual> { text: "Option 3" }
                                radio4 = <RadioButtonTextual> { text: "Option 4" }
                            }
                        }


                        <H4> { text: "Button Group"}
                        <ButtonGroup> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            radiotabs_demo = <View> {
                                width: Fit, height: Fit,
                                radio1 = <RadioButtonTab> { text: "Option 1" }
                                radio2 = <RadioButtonTab> { text: "Option 2" }
                                radio3 = <RadioButtonTab> { text: "Option 3" }
                                radio4 = <RadioButtonTab> { text: "Option 4" }
                            }
                        }

                        <H4> { text: "Media"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            mediaradios_demo = <View> {
                                width: Fit, height: Fit,
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                radio1 = <RadioButtonImage> {
                                    width: 50, height: 50,
                                    media: Image,
                                    image: <Image> { source: dep("crate://self/resources/ducky.png" ) }
                                }
                                radio2 = <RadioButtonImage> {
                                    width: 50, height: 50,
                                    media: Image,
                                    image: <Image> { source: dep("crate://self/resources/ducky.png" ) }
                                }
                                radio3 = <RadioButtonImage> {
                                    width: 50, height: 50,
                                    media: Image,
                                    image: <Image> { source: dep("crate://self/resources/ducky.png" ) }
                                }
                                radio4 = <RadioButtonImage> {
                                    width: 50, height: 50,
                                    media: Image,
                                    image: <Image> { source: dep("crate://self/resources/ducky.png" ) }
                                }
                            }
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<SlidesView>"}
                    width: Fill, height: Fit,
                    <ZooDesc> {text:"Slides View"}
                    <View> {
                        width: Fill, height: Fit,
                        <SlidesView> {
                            width: Fill, height: 400,

                            <SlideChapter> {
                                title = {text: "Hey!"},
                                <SlideBody> {text: "This is the 1st slide. Use your right\ncursor key to show the next slide."}
                            }

                            <Slide> {
                                title = {text: "Second slide"},
                                <SlideBody> {text: "This is the 2nd slide. Use your left\ncursor key to show the previous slide."}
                            }

                        }
                    }
                }

                // TODO: SHOW
                <ZooHeader> {
                    title = {text:"<Dock>"}
                    <ZooDesc> {text:"Dock"}
                    <CachedRoundedView> {
                        draw_bg: { radius: (THEME_CONTAINER_CORNER_RADIUS) }
                        width: Fill, height: Fit,
                            <View> {
                                height: Fit, width: Fill
                                show_bg: true,
                                draw_bg: { color: (THEME_COLOR_BG_CONTAINER) }
                                <Dock> {
                                    height: 500., width: Fill

                                    root = Splitter {
                                        axis: Horizontal,
                                        align: FromA(300.0),
                                        a: tab_set_1,
                                        b: tab_set_2
                                    }

                                    tab_set_1 = Tabs {
                                        tabs: [tab_a, tab_b],
                                        selected: 1
                                    }

                                    tab_set_2 = Tabs {
                                        tabs: [tab_c, tab_d, tab_e, tab_f],
                                        selected: 1
                                    }

                                    tab_a = Tab {
                                        name: "Tab A"
                                        template: PermanentTab,
                                        kind: Container_A
                                    }

                                    tab_b = Tab {
                                        name: "Tab B"
                                        template: PermanentTab,
                                        kind: Container_B
                                    }

                                    tab_c = Tab {
                                        name: "Tab C"
                                        template: CloseableTab,
                                        kind: Container_C
                                    }

                                    tab_d = Tab {
                                        name: "Tab D"
                                        template: CloseableTab,
                                        kind: Container_D
                                    }

                                    tab_e = Tab {
                                        name: "Tab E"
                                        template: CloseableTab,
                                        kind: Container_E
                                    }

                                    tab_f = Tab {
                                        name: "Tab F"
                                        template: CloseableTab,
                                        kind: Container_F
                                    }

                                    Container_A = <RectView> {
                                        height: Fill, width: Fill
                                        padding: 10.,
                                        <Label> {text: "Hallo"}
                                    }

                                    Container_B = <RectView> {
                                        height: Fill, width: Fill
                                        padding: 10.,
                                        <Label> {text: "Kuckuck"}
                                    }

                                    Container_C = <RectView> {
                                        height: Fill, width: Fill
                                        padding: 10.,
                                        <Label> {text: "Ahoy"}
                                    }

                                    Container_D = <RectView> {
                                        height: Fill, width: Fill
                                        padding: 10.,
                                        <Label> {text: "Hi"}
                                    }

                                    Container_E = <RectView> {
                                        height: Fill, width: Fill
                                        padding: 10.,
                                        <Label> {text: "Ahoy"}
                                    }

                                    Container_F = <RectView> {
                                        height: Fill, width: Fill
                                        padding: 10.,
                                        <Label> {text: "Hi"}
                                    }
                                }

                            }
                        }
                    }

                <ZooHeader> {
                    title = {text:"<DockMinimal>"}
                    <ZooDesc> {text:"DockMinimal"}
                    <CachedRoundedView> {
                        draw_bg: { radius: (THEME_CONTAINER_CORNER_RADIUS) }
                        width: Fill, height: Fit,
                            <View> {
                                height: Fit, width: Fill
                                show_bg: true,
                                draw_bg: { color: (THEME_COLOR_BG_CONTAINER) }
                                <DockMinimal> {
                                    height: 500., width: Fill

                                    root = Splitter {
                                        axis: Horizontal,
                                        align: FromA(300.0),
                                        a: tab_set_1,
                                        b: tab_set_2
                                    }

                                    tab_set_1 = Tabs {
                                        tabs: [tab_a, tab_b],
                                        selected: 1
                                    }

                                    tab_set_2 = Tabs {
                                        tabs: [tab_c, tab_d, tab_e, tab_f],
                                        selected: 1
                                    }

                                    tab_a = Tab {
                                        name: "Tab A"
                                        template: CloseableTab,
                                        kind: Container_A
                                    }

                                    tab_b = Tab {
                                        name: "Tab B"
                                        template: PermanentTab,
                                        kind: Container_B
                                    }

                                    tab_c = Tab {
                                        name: "Tab C"
                                        template: CloseableTab,
                                        kind: Container_C
                                    }

                                    tab_d = Tab {
                                        name: "Tab D"
                                        template: CloseableTab,
                                        kind: Container_D
                                    }

                                    tab_e = Tab {
                                        name: "Tab E"
                                        template: CloseableTab,
                                        kind: Container_E
                                    }

                                    tab_f = Tab {
                                        name: "Tab F"
                                        template: CloseableTab,
                                        kind: Container_F
                                    }

                                    Container_A = <RectView> {
                                        height: Fill, width: Fill
                                        padding: 10.,
                                        draw_bg: { color: (THEME_COLOR_D_HIDDEN)}
                                        <Label> {text: "Hallo"}
                                    }

                                    Container_B = <RectView> {
                                        height: Fill, width: Fill
                                        draw_bg: { color: (THEME_COLOR_D_HIDDEN)}
                                        padding: 10.,
                                        <Label> {text: "Kuckuck"}
                                    }

                                    Container_C = <RectView> {
                                        height: Fill, width: Fill
                                        draw_bg: { color: (THEME_COLOR_D_HIDDEN)}
                                        padding: 10.,
                                        <Label> {text: "Ahoy"}
                                    }

                                    Container_D = <RectView> {
                                        height: Fill, width: Fill
                                        draw_bg: { color: (THEME_COLOR_D_HIDDEN)}
                                        padding: 10.,
                                        <Label> {text: "Hi"}
                                    }

                                    Container_E = <RectView> {
                                        height: Fill, width: Fill
                                        draw_bg: { color: (THEME_COLOR_D_HIDDEN)}
                                        padding: 10.,
                                        <Label> {text: "Ahoy"}
                                    }

                                    Container_F = <RectView> {
                                        height: Fill, width: Fill
                                        draw_bg: { color: (THEME_COLOR_D_HIDDEN)}
                                        padding: 10.,
                                        <Label> {text: "Hi"}
                                    }
                                }

                            }
                        }
                    }

                <ZooHeader> {
                    title = {text:"Docs tests"}
                    <ZooDesc> {text:"Docs tests"}
                    <RoundedView> {
                        draw_bg: { radius: (THEME_CONTAINER_CORNER_RADIUS) }
                        width: Fill, height: Fit,
                            <View> {
                                height: Fit, width: Fill
                                show_bg: false,
                                margin: { bottom: 200.}
                                draw_bg: { color: (THEME_COLOR_BG_CONTAINER) }
                                spacing: 5.0,
                                flow: Down,
                                <H3> { text: "Button" }
                                <H4> { text: "Basic"}
                                <Button> { text: "I can be clicked" } // Default button with a custom label.

                                <H4> { text: "Typical"}
                                <Button> {
                                    text: "I can be clicked", // Text label.
                                    draw_bg: {
                                        bodytop: #3, // Set the hover-state color.
                                        bodybottom: #f00, // Set the pressed-state color.
                                    },
                                    height: Fit, // Element assumes height of its children.
                                    width: Fit, // Element expands to use all available horizontal space.
                                    margin: 0.0, // Homogenous spacing of 10.0 around the element.
                                    padding: 10.  // Homogenous spacing of 7.5 between all the element's
                                                // borders and its content.
                                }

                                <H4> { text: "Advanced"}
                                <Button> {
                                // Allows instantiation of customly styled elements as i.e. <MyButton> {}.

                                    // BUTTON SPECIFIC PROPERTIES

                                    draw_bg: { // Shader object that draws the bg.
                                            fn pixel(self) -> vec4 {
                                            return mix( // State transition animations.
                                                mix(
                                                    #800,
                                                    mix(#800, #f, 0.5),
                                                    self.hover
                                                ),
                                                #00f,
                                                self.pressed
                                            )
                                        }
                                    },

                                    draw_icon: { // Shader object that draws the icon.
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                        // Icon file dependency.

                                        fn get_color(self) -> vec4 { // Overwrite the shader's fill method.
                                            return mix( // State transition animations.
                                                mix(
                                                    #f0f,
                                                    #fff,
                                                    self.hover
                                                ),
                                                #000,
                                                self.pressed
                                            )
                                        }
                                    }

                                    // draw_text: { // Shader object that draws the icon.
                                    //     wrap: Word, // Wraps text between words.
                                    //     text_style: {
                                    //     // Controls the appearance of text.
                                    //         font: {path: dep("crate://self/resources/GoNotoKurrent-Bold.ttf")},
                                    //         // Font file dependency.

                                    //         font_size: 12.0, // Font-size of 12.0.
                                    //     }

                                    //     fn get_color(self) -> vec4 { // Overwrite the shader's fill method.
                                    //         return mix( // State transition animations.
                                    //             mix(
                                    //                 self.color,
                                    //                 self.bodytop,
                                    //                 self.bodybottom
                                    //             ),
                                    //             self.color_pressed,
                                    //             self.pressed
                                    //         )
                                    //     }
                                    // }

                                    grab_key_focus: true, // Keyboard gets focus when clicked.

                                    icon_walk: {
                                        margin: 10.,
                                        width: 16.,
                                        height: Fit
                                    }

                                    label_walk: {
                                        margin: 0.,
                                        width: Fit,
                                        height: Fit,
                                    }

                                    text: "I can be clicked", // Text label.

                                    animator: { // State change triggered animations.
                                        hover = { // State
                                            default: off // The state's starting point.
                                            off = { // Behavior when the animation is started to the off-state
                                                from: { // Behavior depending on the prior states
                                                    all: Forward {duration: 0.1}, // Default animation direction and speed in secs.
                                                    pressed: Forward {duration: 0.25} // Direction and speed for 'pressed' in secs.
                                                }
                                                apply: { // Shader methods to animate
                                                    draw_bg: { pressed: 0.0, hover: 0.0 } // Timeline target positions for the given states.
                                                    draw_icon: { pressed: 0.0, hover: 0.0 }
                                                    draw_text: { pressed: 0.0, hover: 0.0 }
                                                }
                                            }

                                            on = { // Behavior when the animation is started to the on-state
                                                from: {
                                                    all: Forward {duration: 0.1},
                                                    pressed: Forward {duration: 0.5}
                                                }
                                                apply: {
                                                    draw_bg: { pressed: 0.0, hover: [{time: 0.0, value: 1.0}] },
                                                    // pressed: 'pressed' timeline target position
                                                    // hover, time: Normalized timeline from 0.0 - 1.0. 'duration' then determines the actual playback duration of this animation in seconds.
                                                    // hover, value: target timeline position
                                                    draw_icon: { pressed: 0.0, hover: [{time: 0.0, value: 1.0}] },
                                                    draw_text: { pressed: 0.0, hover: [{time: 0.0, value: 1.0}] }
                                                }
                                            }
                                
                                            pressed = { // Behavior when the animation is started to the pressed-state
                                                from: {all: Forward {duration: 0.2}}
                                                apply: {
                                                    draw_bg: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0}, 
                                                    draw_icon: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0},
                                                    draw_text: {pressed: [{time: 0.0, value: 1.0}], hover: 1.0}
                                                }
                                            }
                                        }
                                    }

                                    // LAYOUT PROPERTIES

                                    height: Fit,
                                    // Element assumes the height of its children.

                                    width: Fill,
                                    // Element assumes the width of its children.

                                    margin: 5.0
                                    padding: { top: 3.0, right: 6.0, bottom: 3.0, left: 6.0 },
                                    // Individual space between the element's border and its content
                                    // for top and left.

                                    flow: Right,
                                    // Stacks children from left to right.

                                    spacing: 5.0,
                                    // A spacing of 10.0 between children.

                                    align: { x: 0.5, y: 0.5 },
                                    // Positions children at the left (x) bottom (y) corner of the parent.
                                }

                                <H4> { text: "Preset: ButtonFlat"}
                                <ButtonFlat> {
                                    text: "Flat Button"
                                }

                                <H4> { text: "Preset: ButtonFlatter"}
                                <ButtonFlatter> {
                                    draw_icon: {
                                        color: #f00,
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                    text: "Flatter Button"
                                }

                                <H3> { text: "Checkbox" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "ColorPicker" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Dock" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "DropDown" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "ExpandablePanel" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "FileTree" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "FlatList" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "FoldButton" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "FoldHeader" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Html" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Icon" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Image" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "ImageBlend" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Label" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "LinkLabel" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Markdown" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "NavControl" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "PageFlip" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Piano" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "PopupMenu" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "PortalList" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "RadioButton" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "RotatedImage" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "ScrollBar" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "ScrollBars" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "SlidePanel" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Slider" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "SlidesView" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Splitter" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "StackNavigation" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Tab" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "TabBar" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "TabCloseButton" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "TextInput" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "VectorLine" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "Video" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}

                                <H3> { text: "View" }
                                <H4> { text: "Basic"}
                                <H4> { text: "Typical"}
                                <H4> { text: "Advanced"}


                            }
                        }
                    }

                    // TODO: SHOW
                    // SEEMS NOT TO WORK WITHOUT DUMMY DATA
                    // <ZooHeader> {
                    //     title = {text:"Portal List"}
                    //     <ZooDesc> {text:"Portal List"}
                    //     <ZooGroup> { <PortalList> { width: Fill, height: 100.} }
                    // }

                    // TODO: SHOW
                    // <ZooHeader> {
                    //     title = {text:"Stack Navigation"}
                    //     <ZooDesc> {text:"Stack Navigation"}
                    //     <ZooGroup> { <StackNavigation> { width: Fill, height: 100.} }
                    // }

                    // TODO: SHOW
                    // <ZooHeader> {
                    //     title = {text:"Flat list"}
                    //     <ZooDesc> {text:"Flat list"}
                    //     <ZooGroup> { <FlatList> { width: Fill, height: 100.} }
                    // }

                    // TODO: SHOW
                    // REFERENCE: https://github.com/project-robius/makepad_wonderous/blob/main/src/timeline/timeline_screen.rs#L242-L264
                    // <ZooHeader> {
                    //     title = {text: "Expandable Panel"}
                    //     <ZooDesc> {text: "Expandable Panel"}
                    //     <ZooGroup> {
                    //         expandable_panel = <ExpandablePanel> {
                    //             body = {
                    //                 flow: Down,
                    //                 spacing: 10,
                    //                 header = <View> { height: 100., width: 100., show_bg: true, draw_bg: { color: (DEMO_COLOR_2)} }
                    //                 <View> { height: 100., width: 100., show_bg: true, draw_bg: { color: (DEMO_COLOR_1)} }
                    //             }

                    //             panel = {
                    //                 draw_bg: { color: (DEMO_COLOR_3) }

                    //                 scroll_handler = {
                    //                     draw_bg: {
                    //                         color: #aaa
                    //                         radius: 2.
                    //                     }
                    //                 }

                    //                 <View> { height: 100., width: 100., show_bg: true, draw_bg: { color: #ff0} }
                    //             }
                    //         }
                    //     }
                    // }

                }
            }
        }
    }

    app_main!(App);

    #[derive(Live, LiveHook, PartialEq, LiveAtomic, Debug, LiveRead)]
    pub enum DropDownEnum {
        #[pick]
        ValueOne,
        ValueTwo,
        Thrice,
        FourthValue,
        OptionE,
        Hexagons,
    }

    #[derive(Live, LiveHook, LiveRead, LiveRegister)]
    pub struct DataBindingsForApp {
        #[live] fnumber: f32,
        #[live] inumber: i32,
        #[live] dropdown: DropDownEnum,
    }
    #[derive(Live, LiveHook)]
    pub struct App {
        #[live] ui: WidgetRef,
        #[rust] counter: usize,
        #[rust(DataBindingsForApp::new(cx))] bindings: DataBindingsForApp
    }

    impl LiveRegister for App {
        fn live_register(cx: &mut Cx) {
            crate::makepad_widgets::live_design(cx);
            crate::demofiletree::live_design(cx);
        }
    }


    impl MatchEvent for App{
        fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions){
            let ui = self.ui.clone();

        ui.radio_button_set(ids!(radios_demo.radio1, radios_demo.radio2, radios_demo.radio3, radios_demo.radio4))
            .selected_to_visible(cx, &ui, actions, ids!(radios_demo.radio1, radios_demo.radio2, radios_demo.radio3, radios_demo.radio4));

        ui.radio_button_set(ids!(iconradios_demo.radio1, iconradios_demo.radio2, iconradios_demo.radio3, iconradios_demo.radio4))
            .selected_to_visible(cx, &ui, actions, ids!(iconradios_demo.radio1, iconradios_demo.radio2, iconradios_demo.radio3, iconradios_demo.radio4));

        ui.radio_button_set(ids!(radiotabs_demo.radio1, radiotabs_demo.radio2, radiotabs_demo.radio3, radiotabs_demo.radio4))
            .selected_to_visible(cx, &ui, actions, ids!(radiotabs_demo.radio1, radiotabs_demo.radio2, radiotabs_demo.radio3, radiotabs_demo.radio4));

        ui.radio_button_set(ids!(textonlyradios_demo.radio1, textonlyradios_demo.radio2, textonlyradios_demo.radio3, textonlyradios_demo.radio4))
            .selected_to_visible(cx, &ui, actions, ids!(textonlyradios_demo.radio1, textonlyradios_demo.radio2, textonlyradios_demo.radio3, textonlyradios_demo.radio4));

        ui.radio_button_set(ids!(mediaradios_demo.radio1, mediaradios_demo.radio2, mediaradios_demo.radio3, mediaradios_demo.radio4))
            .selected_to_visible(cx, &ui, actions, ids!(mediaradios_demo.radio1, mediaradios_demo.radio2, mediaradios_demo.radio3, mediaradios_demo.radio4));

        if let Some(txt) = self.ui.text_input(id!(simpletextinput)).changed(&actions){
            log!("TEXTBOX CHANGED {}", self.counter);
            self.counter += 1;
            let lbl = self.ui.label(id!(simpletextinput_outputbox));
            lbl.set_text_and_redraw(cx,&format!("{} {}" , self.counter, txt));
        }

        if self.ui.button(id!(basicbutton)).clicked(&actions) {
            log!("BASIC BUTTON CLICKED {}", self.counter);
            self.counter += 1;
            let btn = self.ui.button(id!(basicbutton));
            btn.set_text_and_redraw(cx,&format!("Clicky clicky! {}", self.counter));
        }

        if self.ui.button(id!(styledbutton)).clicked(&actions) {
            log!("STYLED BUTTON CLICKED {}", self.counter);
            self.counter += 1;
            let btn = self.ui.button(id!(styledbutton));
            btn.set_text_and_redraw(cx,&format!("Styled button clicked: {}", self.counter));
        }

        if self.ui.button(id!(iconbutton)).clicked(&actions) {
            log!("ICON BUTTON CLICKED {}", self.counter);
            self.counter += 1;
            let btn = self.ui.button(id!(iconbutton));
            btn.set_text_and_redraw(cx,&format!("Icon button clicked: {}", self.counter));
        }


        if let Some(check) = self.ui.check_box(id!(simplecheckbox)).changed(actions) {
            log!("CHECK BUTTON CLICKED {} {}", self.counter, check);
            self.counter += 1;
            let lbl = self.ui.label(id!(simplecheckbox_output));
            lbl.set_text_and_redraw(cx,&format!("{} {}" , self.counter, check));
        }

        if self.ui.fold_button(id!(folderbutton)).opening(actions) {
            log!("FOLDER BUTTON CLICKED {} {}", self.counter, 12);
//            self.ui.fold_header(id!(thefoldheader)).opened = true;

            self.counter += 1;
        }

        if self.ui.fold_button(id!(folderbutton)).closing(actions) {
            log!("FOLDER BUTTON CLICKED {} {}", self.counter, 12);



            self.counter += 1;
        }


        let mut db = DataBindingStore::new();
        db.data_bind(cx, actions, &self.ui, Self::data_bind);
        self.bindings.apply_over(cx, &db.nodes);

    }

    fn handle_startup(&mut self, cx: &mut Cx) {

        let ui = self.ui.clone();
        let db = DataBindingStore::from_nodes(self.bindings.live_read());
        Self::data_bind(db.data_to_widgets(cx, &ui));
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

impl App{
    pub fn data_bind(mut db: DataBindingMap) {
        db.bind(id!(dropdown), ids!(dropdown));
    }
}