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

    DEMO_COLOR_1 = #8f0
    DEMO_COLOR_2 = #0f8
    DEMO_COLOR_3 = #80f

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
                        text:"Explain: typographic sizes, base size and contrast."
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
                            padding: 0
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
                            padding: 0
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
                            padding: 0
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
                            padding: 0
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
                            padding: 0
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
                        simpletextinput= <TextInput> {
                            empty_message: "Inline Label",
                        }

                        simpletextinput_outputbox = <P> {
                            text: "Output"
                        }
                    }
                }

                <ZooHeader> {
                    title = {text:"<Label>"}
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
                        padding: 0.
                        show_bg: false,
                        <DemoFileTree> { file_tree:{ height: 400. } }
                    }
                }

                <ZooHeader> {
                    title = { text:"<FoldHeader>" }
                    <ZooDesc> { text:"This widget allows you to have a header with a foldbutton (has to be named fold_button for the magic to work)" }
                    <ZooGroup> {
                        thefoldheader= <FoldHeader> {
                            header: <View> {
                                height: Fit
                                align: {x: 0., y: 0.5}
                                fold_button = <FoldButton> {} <P> {text: "Fold me!"}
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
                    title = {text:"<Html>"}
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
                    title = {text:"<Markdown>"}
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
                    title = {text:"<Image>"}
                    <ZooDesc> {text:"A static inline image from a resource."}
                    <ZooGroup> { <Image> { source: dep("crate://self/resources/ducky.png" ) } }
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
                                radio1 = <RadioButton> { label: "Option 1" }
                                radio2 = <RadioButton> { label: "Option 2" }
                                radio3 = <RadioButton> { label: "Option 3" }
                                radio4 = <RadioButton> { label: "Option 4" }
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
                                    label: "Option 1"
                                    icon_walk: {
                                        width: 12.5, height: Fit,
                                    }
                                    draw_icon: {
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                }
                                radio2 = <RadioButtonCustom> {
                                    label: "Option 2"
                                    icon_walk: {
                                        width: 12.5, height: Fit,
                                    }
                                    draw_icon: {
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                }
                                radio3 = <RadioButtonCustom> {
                                    label: "Option 3"
                                    icon_walk: {
                                        width: 12.5, height: Fit,
                                    }
                                    draw_icon: {
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                }
                                radio4 = <RadioButtonCustom> {
                                    label: "Option 4"
                                    icon_walk: {
                                        width: 12.5, height: Fit,
                                    }
                                    draw_icon: {
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
                                radio1 = <RadioButtonTextual> { label: "Option 1" }
                                radio2 = <RadioButtonTextual> { label: "Option 2" }
                                radio3 = <RadioButtonTextual> { label: "Option 3" }
                                radio4 = <RadioButtonTextual> { label: "Option 4" }
                            }
                        }


                        <H4> { text: "Button Group"}
                        <ButtonGroup> {
                            height: Fit
                            flow: Right
                            align: { x: 0.0, y: 0.5 }
                            radiotabs_demo = <View> {
                                width: Fit, height: Fit,
                                radio1 = <RadioButtonTab> { label: "Option 1" }
                                radio2 = <RadioButtonTab> { label: "Option 2" }
                                radio3 = <RadioButtonTab> { label: "Option 3" }
                                radio4 = <RadioButtonTab> { label: "Option 4" }
                            }
                        }

                    }
                }

                <ZooHeader> {
                    title = {text:"<SlidesView>"}
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
                    title = {text:"<Dock>"}
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

        ui.radio_button_set(ids!(iconradios_demo.radio1, iconradios_demo.radio2, iconradios_demo.radio3, iconradios_demo.radio4))
            .selected_to_visible(cx, &ui, actions, ids!(iconradios_demo.radio1, iconradios_demo.radio2, iconradios_demo.radio3, iconradios_demo.radio4));

        ui.radio_button_set(ids!(radiotabs_demo.radio1, radiotabs_demo.radio2, radiotabs_demo.radio3, radiotabs_demo.radio4))
            .selected_to_visible(cx, &ui, actions, ids!(radiotabs_demo.radio1, radiotabs_demo.radio2, radiotabs_demo.radio3, radiotabs_demo.radio4));

        ui.radio_button_set(ids!(textonlyradios_demo.radio1, textonlyradios_demo.radio2, textonlyradios_demo.radio3, textonlyradios_demo.radio4))
            .selected_to_visible(cx, &ui, actions, ids!(textonlyradios_demo.radio1, textonlyradios_demo.radio2, textonlyradios_demo.radio3, textonlyradios_demo.radio4));

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