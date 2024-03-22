use makepad_widgets::*;
use makepad_platform::live_atomic::*;


live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_widgets::vectorline::*;
    import makepad_draw::shader::std::*;
    import makepad_example_ui_zoo::demofiletree::*;

    COLOR_CONTAINER = (THEME_COLOR_D_075)
    COLOR_ACCENT = (THEME_COLOR_MAKEPAD)

    ZooTitle = <View> {
        width: Fill, height: Fit,
        flow: Down,
        align: { x: 0.0, y: 0.5},
        margin: <THEME_MSPACE_3> {}, padding: 0.,
        spacing: 10.,
        show_bg: false,
        title = <H2> { text: "Makepad UI Zoo" }
    }

    ZooHeader = <View> {
        width: Fill, height: Fit,
        flow: Down,
        spacing: (THEME_SPACE_1),
        padding: 0., margin: <THEME_MSPACE_H_3> {}
        divider = <Hr> { }
        title = <H3> { text: "Header" }
    }

    ZooGroup = <RoundedView> {
        height: Fit, width: Fill,
        flow: Right,
        align: { x: 0.0, y: 0.5},
        margin: 0., padding: <THEME_MSPACE_2> {},
        show_bg: true;
        draw_bg: { color: (COLOR_CONTAINER) }
    }

    ZooDesc = <P> { text: "" }

    ZooBlock = <RoundedView> {
        width: 50., height: 50.
        margin: 0., padding: 0.,
        spacing: 0.,

        show_bg: true;
        draw_bg: {
            fn get_color(self) -> vec4 {
                return mix(self.color, self.color*0.5, self.pos.y);
            }
            radius: 5.
        }
    }

    App = {{App}} {
        ui: <Window> {
            width: Fill, height: Fill,
            show_bg: true,
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return (THEME_COLOR_BG);
                }
            }

            caption_bar = {
                visible: true,
                margin: {left: -100},
                caption_label = { label = {text: "Makepad UI Zoo"} }
            },

            body = <View> {
                width: Fill, height: Fill,
                flow: Down,
                spacing: 10.,
                margin: 0., padding: 0.
                scroll_bars: <ScrollBars> {}

                <ZooTitle> {}

                <ZooHeader> {
                    title = {text:"Typographic System"}
                    <ZooDesc> {
                        text:"Typographic System."
                    }
                    <View> {
                        width: Fill, height: Fit,
                        flow: Down,
                        show_bg: true,
                        draw_bg: { color: (COLOR_CONTAINER) }
                        padding: 20.,
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
                        <Meta> { text: "Meta information" }
                    }
                }

                <ZooHeader> {
                    title = {text: "View 1" }
                    <ZooDesc> {text:"This is a gray view with flow set to Right\nTo show the extend, the background has been enabled using show_bg and a gray pixelshader has been provided to draw_bg."}
                    <View> {
                        height: Fit
                        flow: Right,
                        show_bg: true,
                        draw_bg: { color: (COLOR_CONTAINER) }
                        padding: 10.
                        spacing: 10.
                        <ZooBlock> {draw_bg:{color: #f00}}
                        <ZooBlock> {draw_bg:{color: #0f0}}
                        <ZooBlock> {draw_bg:{color: #00f}}
                    }

                    <ZooDesc> { text:"This is a view with flow set to Down" }
                    <View> {
                        height: Fit,
                        flow: Down,
                        padding: 10.
                        spacing: 10.
                        show_bg: true,
                        draw_bg: { color: (THEME_COLOR_D_2) }
                        <ZooBlock> {draw_bg:{color: #f00}}
                        <ZooBlock> {draw_bg:{color: #0f0}}
                        <ZooBlock> {draw_bg:{color: #00f}}
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
                            padding: 0
                            spacing: 10
                            <ZooBlock> {draw_bg:{color: #f00}}
                            <ZooBlock> {draw_bg:{color: #0f0}}
                            <ZooBlock> {draw_bg:{color: #00f}}
                            <ZooBlock> {draw_bg:{color: #0f0}}
                        }

                        <View> {
                            width: Fit, height: Fit,
                            flow: Down,
                            show_bg: false,
                            padding: 0
                            spacing: 10
                            <ZooBlock> {draw_bg:{color: #f00}}
                            <ZooBlock> {draw_bg:{color: #0f0}}
                            <ZooBlock> {draw_bg:{color: #00f}}
                            <ZooBlock> {draw_bg:{color: #0f0}}
                        }

                        <View> {
                            width: Fit, height: Fit,
                            flow: Down,
                            show_bg: false,
                            padding: 0
                            spacing: 10
                            <ZooBlock> {draw_bg:{color: #f00}}
                            <ZooBlock> {draw_bg:{color: #0f0}}
                            <ZooBlock> {draw_bg:{color: #00f}}
                            <ZooBlock> {draw_bg:{color: #0f0}}
                        }

                        <View> {
                            width: Fit, height: Fit,
                            flow: Down,
                            show_bg: false,
                            padding: 0
                            spacing: 10
                            <ZooBlock> {draw_bg:{color: #f00}}
                            <ZooBlock> {draw_bg:{color: #0f0}}
                            <ZooBlock> {draw_bg:{color: #00f}}
                            <ZooBlock> {draw_bg:{color: #0f0}}
                        }

                        <View> {
                            width: Fit, height: Fit,
                            flow: Down,
                            show_bg: false,
                            padding: 0
                            spacing: 10
                            <ZooBlock> {draw_bg:{color: #f00}}
                            <ZooBlock> {draw_bg:{color: #0f0}}
                            <ZooBlock> {draw_bg:{color: #00f}}
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
                        <ZooBlock> {draw_bg:{color: #f00}}
                        <ZooBlock> {draw_bg:{color: #0f0}}
                        <ZooBlock> {draw_bg:{color: #00f}}
                    }
                }

                <ZooHeader> {
                    title = {text:"Button"}
                    <ZooDesc> {text:"A small clickable region"}
                    <View> {
                        flow: Right,
                        width: Fill, height: Fit,
                        align: { x: 0.0, y: 0.5 }
                        spacing: 10.,
                        basicbutton = <Button> { text: "I can be clicked" }

                        iconbutton = <Button> {
                            draw_icon: {
                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                            }
                            text: "I can have a lovely icon!"

                            icon_walk: {
                                width: 15, height: Fit,
                                margin: {top: 0., right: 0., bottom: 0., left: 7.5},
                            }
                        }

                        <Button> {
                            margin: 5., padding: 0.,
                            draw_icon: {
                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                            }

                            icon_walk: {
                                width: 15, height: Fit,
                                margin: 0.0
                            }
                            draw_bg: {
                                fn pixel(self) -> vec4 {
                                    return #00000000;
                                }
                            }
                        }

                        styledbutton = <Button> {
                            draw_bg: {
                                fn pixel(self) -> vec4 {
                                    return #f40 + self.pressed * vec4(1., 1., 1., 1.)
                                }
                            }
                            draw_text: {
                                fn get_color(self) -> vec4 {
                                    return #fff - vec4(0., 0.1, 0.4, 0.) * self.hover - self.pressed * vec4(1., 1., 1., 0.);
                                }
                            }
                            text: "I can be styled!"
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"TextInput"}
                    <ZooDesc> {text:"Simple 1 line textbox"}
                    <ZooGroup> {
                        simpletextinput= <TextInput> {
                            text: "This is inside a text input!"
                        }

                        simpletextinput_outputbox = <P> {
                            text: "Output"
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"Label"}
                    <ZooDesc> { text:"Simple 1 line textbox" }
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
                                    return mix((COLOR_ACCENT), (THEME_COLOR_U_0), self.pos.x)
                                }
                                color: #ffc
                                text_style: {
                                    font_size: 40.,
                                }
                            },
                            text: "OR EVEN SOME PIXELSHADERS"
                        }
                    }
                }

                <ZooHeader> {
                    title = { text:"Slider" }
                    <ZooDesc> { text:"A parameter dragger" }
                    <ZooGroup> {
                        width: Fill, height: Fit,
                        flow: Down,
                        <View> {
                            width: Fill, height: Fit,
                            <Slider> { text: "Parameter" }
                            <Slider> { text: "Parameter" }
                            <Slider> { text: "Parameter" }
                        } 
                        <View> {
                            width: Fill, height: Fit,
                            <Slider> { text: "Parameter" }
                            <Slider> { text: "Parameter" }
                            <Slider> { text: "Parameter" }
                        } 
                        <View> {
                            width: Fill, height: Fit,
                            <Slider> { text: "Parameter" }
                            <Slider> { text: "Parameter" }
                            <Slider> { text: "Parameter" }
                        } 
                    }
                }

                <ZooHeader> {
                    title = {text:"DropDown"}
                    <ZooDesc> {text:"DropDown control. This control currently needs to be databound which needs some plumbing. In this sample there is a binding context struct in the main app struct - which gets bound on app start - and updated during handle_actions."}
                    <ZooGroup> {
                        dropdown = <DropDown> {
                            labels: ["ValueOne", "ValueTwo", "Thrice", "FourthValue", "OptionE", "Hexagons"],
                            values: [ValueOne, ValueTwo, Thrice, FourthValue, OptionE, Hexagons]
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"DemoFileTree"}
                    <ZooDesc> {text:"DemoFileTree?"}
                    <ZooGroup> { <DemoFileTree> { file_tree:{ height: 400. } } }
                }

                <ZooHeader> {
                    title = { text:"FoldHeader" }
                    <ZooDesc> { text:"This widget allows you to have a header with a foldbutton (has to be named fold_button for the magic to work)" }
                    <ZooGroup> {
                        thefoldheader= <FoldHeader> {
                            header: <View> {
                                fold_button = <FoldButton> {} <Pbold> {text: "Fold me!"}
                            }
                            body: <View> {
                                width: Fill, height: Fit
                                show_bg: false,
                                padding: 5.0,
                                <P> { text:"This is the body that can be folded away" }
                            }
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"Html"}
                    <ZooDesc> {text:"HTML Widget"}
                    <ZooGroup> {
                        <Html> {
                            width:Fill, height:Fit,
                            // font_size: (THEME_FONT_SIZE_BASE),
                            // flow: RightWrap,
                            // padding: 5,
                            body:"This is <b>bold text</b>&nbsp;and&nbsp;<i>italic text</i>. <br/> <sep> <b><i>Bold italic</i></b>, <u>underlined</u>, and <s>strike through</s> text.<br/> <Button>Button</Button><br/> <block_quote>Blockquote<br/> <block_quote>Nested blockquote</block_quote> </block_quote><br/><code>This is a code block</code>"
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"Markdown"}
                    <ZooDesc> {text:"Markdown"}
                    <ZooGroup> {
                        <Markdown> {
                            width:Fill, height: Fit,
                            // font_size: (THEME_FONT_SIZE_BASE),
                            // flow: RightWrap,
                            // padding: 5,
                            body:"# Headline 1 \n ## Headline 2 \n ### Headline 3 \n #### Headline 4 \n This is standard text with a  \n\n line break a short ~~strike through~~ demo.\n\n *Italic text* \n\n **Bold text** \n\n - Bullet\n - Another bullet\n\n - Third bullet\n\n 1. Numbered list Bullet\n 2. Another list entry\n\n 3. Third list entry\n\n `Monospaced text`\n\n> This is a quote.\n\nThis is `inline code`.\n\n ```code block```"
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"Image"}
                    <ZooDesc> {text:"A static inline image from a resource."}
                    <ZooGroup> { <Image> { source: dep("crate://self/resources/ducky.png" ) } }
                }

                <ZooHeader> {
                    title = {text:"Link Label"}
                    <ZooDesc> {text:"Link Label"}
                    <ZooGroup> { <LinkLabel> { text: "Click me!"} }
                }

                <ZooHeader> {
                    title = {text:"CheckBox"}
                    <ZooDesc> {text:"Checkbox?"}
                    <ZooGroup> {
                        height: Fit
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
                            align: { x: 0.0, y: 0.5}
                            <CheckBox> {text:"Check me out!"}
                            <CheckBox> {text:"Check me out!"}
                            <CheckBox> {text:"Check me out!"}
                        }
                        <H4> { text: "Circular Checkbox Mode"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5}
                            <CheckBox> {text:"Check me out!", draw_check: { check_type: Radio } }
                            <CheckBox> {text:"Check me out!", draw_check: { check_type: Radio } }
                            <CheckBox> {text:"Check me out!", draw_check: { check_type: Radio } }
                        }
                        <H4> { text: "Toggle Mode"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5}
                            <CheckBox> {text:"Check me out!", draw_check: { check_type: Toggle } }
                            <CheckBox> {text:"Check me out!", draw_check: { check_type: Toggle } }
                            <CheckBox> {text:"Check me out!", draw_check: { check_type: Toggle } }
                        }
                        <H4> { text: "Text Mode"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5}
                            <CheckBox> {text:"Check me out!", draw_check: { check_type: None } }
                            <CheckBox> {text:"Check me out!", draw_check: { check_type: None } }
                            <CheckBox> {text:"Check me out!", draw_check: { check_type: None } }
                        }
                        <H4> { text: "Custom Icon Mode"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5}
                            <CheckBox> {
                                text:"Check me out!"
                                draw_check: { check_type: None }
                                draw_icon: {
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }
                            }
                            <CheckBox> {
                                text:"Check me out!"
                                draw_check: { check_type: None }
                                draw_icon: {
                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                }
                            }
                            <CheckBox> {
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
                    title = {text:"RadioButton"}
                    <ZooDesc> {text:"RadioButton?"}
                    <ZooGroup> {
                        flow: Down,
                        <H3> { text: "Radio Mode"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            radios_demo = <View> {
                                width: Fit, height: Fit,
                                radio1 = <RadioButton> { label: "Option 1: yey" }
                                radio2 = <RadioButton> { label: "Option 2: hah" }
                                radio3 = <RadioButton> { label: "Option 3: hmm" }
                                radio4 = <RadioButton> { label: "Option 4: all of the above" }
                            }
                        }
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            <View> {
                                width: Fit, height: Fit,
                                <RadioButton> { label: "Option 1: yey" }
                                <RadioButton> { label: "Option 2: hah" }
                                <RadioButton> { label: "Option 3: hmm" }
                                <RadioButton> { label: "Option 4: all of the above" }
                            }
                        }
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            <View> {
                                width: Fit, height: Fit,
                                <RadioButton> { label: "Option 1: yey" }
                                <RadioButton> { label: "Option 2: hah" }
                                <RadioButton> { label: "Option 3: hmm" }
                                <RadioButton> { label: "Option 4: all of the above" }
                            }
                        }

                        <H3> { text: "Tab Mode"}
                        <View> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            <View> {
                                width: Fit, height: Fit,
                                <RadioButton> { label: "Option 1: yey", draw_radio: { radio_type: Tab } }
                                <RadioButton> { label: "Option 1: yey", draw_radio: { radio_type: Tab } }
                                <RadioButton> { label: "Option 1: yey", draw_radio: { radio_type: Tab } }
                            }
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"Slides View"}
                    width: Fill, height: Fit,
                    <ZooDesc> {text:"Slides View"}
                    <ZooGroup> {
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
                    title = {text:"Dock"}
                    <ZooDesc> {text:"Dock"}
                    <ZooGroup> {
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
                                tabs: [tab_c, tab_d],
                                selected: 1
                            }

                            tab_a = Tab {
                                name: "Tab A"
                                kind: Container_A
                            }   

                            tab_b = Tab {
                                name: "Tab B"
                                kind: Container_B
                            }

                            tab_c = Tab {
                                name: "Tab C"
                                kind: Container_C
                            }   

                            tab_d = Tab {
                                name: "Tab D"
                                kind: Container_D
                            }

                            tab_e = Tab {
                                name: "Tab E"
                                kind: Container_E
                            }   

                            tab_f = Tab {
                                name: "Tab F"
                                kind: Container_F
                            }

                            Container_A = <RectView> {
                                height: Fill, width: Fill
                                padding: 10.,
                                draw_bg: { color: (THEME_COLOR_D_3) }
                                <Label> {text: "Hallo"}
                            }

                            Container_B = <RectView> {
                                height: Fill, width: Fill
                                padding: 10.,
                                draw_bg: { color: (THEME_COLOR_D_3) }

                                <Label> {text: "Kuckuck"}
                            }

                            Container_C = <RectView> {
                                height: Fill, width: Fill
                                padding: 10.,
                                draw_bg: { color: (THEME_COLOR_D_3) }

                                <Label> {text: "Ahoy"}
                            }

                            Container_D = <RectView> {
                                height: Fill, width: Fill
                                padding: 10.,
                                draw_bg: { color: (THEME_COLOR_D_3) }

                                <Label> {text: "Hi"}
                            }

                            Container_E = <RectView> {
                                height: Fill, width: Fill
                                padding: 10.,
                                draw_bg: { color: (THEME_COLOR_D_3) }

                                <Label> {text: "Ahoy"}
                            }

                            Container_F = <RectView> {
                                height: Fill, width: Fill
                                padding: 10.,
                                draw_bg: { color: (THEME_COLOR_D_3) }

                                <Label> {text: "Hi"}
                            }

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
                // SEEMS NOT TO WORK WITHOUT DUMMY DATA
                // <ZooHeader> {
                //     title = {text:"Portal List"}
                //     <ZooDesc> {text:"Portal List"}
                //     <ZooGroup> { <PortalList> { width: Fill, height: 100.} }
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
                //                 header = <View> { height: 100., width: 100., show_bg: true, draw_bg: { color: #0f0} }
                //                 <View> { height: 100., width: 100., show_bg: true, draw_bg: { color: #f00} }
                //             }

                //             panel = {
                //                 draw_bg: { color: #00f }

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

                //  TODO: Slidepanel appears to be buggy
                // <ZooHeader> {
                //     title = {text:"SlidePanel"}
                //     <ZooDesc> {text:"Slide panel?"}
                //     <ZooGroup> {
                //         <SlidePanel> {
                //             width: (1000 * 0.175), height: (1000 * 0.175),
                //             margin: 0.
                //             side: Right,
                //             <ZooHeader> {
                //                 title = {text:"Image"}
                //                 <ZooDesc> {text:"A static inline image from a resource."}
                //                 <ZooGroup> {
                //                     <Image> {
                //                         width: (1000 * 0.175), height: (1000 * 0.175),
                //                         margin: 0
                //                         source: dep("crate://self/resources/ducky.png" ),
                //                     }
                //                 }
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