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

        UIZooTab = <RectView> {
            height: Fill, width: Fill
            flow: Down,
            padding: 0
            spacing: 0.
        }
                            
        UIZooTabLayout_A = <View> {
            height: Fill, width: Fill
            flow: Right,
            padding: 0
            spacing: 0.

            desc = <View> {
                width: 300., height: Fill,
                flow: Down,
                spacing: (THEME_SPACE_2)
                padding: <THEME_MSPACE_3> {}
                scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
            }

            <Vr> {}

            demos = <View> {
                width: Fill, height: Fill,
                flow: Down,
                spacing: (THEME_SPACE_2)
                padding: <THEME_MSPACE_3> {}
                scroll_bars: <ScrollBars> {show_scroll_x: false, show_scroll_y: true}
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
                    spacing: 0.,
                    margin: 0.,

                    <View> {
                        width: Fill,
                        height: 40.
                        spacing: (THEME_SPACE_2)
                        flow: Right,

                        padding: <THEME_MSPACE_2> {}
                        margin: 0.
                        show_bg: true,
                        draw_bg: { color: (THEME_COLOR_U_1) }

                        <SliderAlt1> { text: "Spacing"}
                        <Vr> {}
                        <Pbold> { width: Fit, text: "Color", padding: { top: 1.5}}
                        <SliderAlt1> { text: "Contrast" }
                        <SliderAlt1> { text: "Tint Factor" }
                        <Vr> {}
                        <Pbold> { width: Fit, text: "Font", padding: { top: 1.5}}
                        <SliderAlt1> { text: "Scale" }
                        <SliderAlt1> { text: "Contrast"}
                        <Vr> {}
                        <CheckBoxToggle> { text: "Label Hover"}
                        <CheckBoxToggle> { text: "Light Theme"}
                    }

                    <Dock> {
                        height: Fill, width: Fill

                        root = Splitter {
                            axis: Horizontal,
                            align: FromA(0.0),
                            a: tab_set_1,
                            b: tab_set_2
                        }

                        tab_set_1 = Tabs {
                            tabs: [tab_a],
                            selected: 0
                        }

                        tab_set_2 = Tabs {
                            tabs: [
                                tTests,
                                tOverview,
                                tLayoutDemos,
                                tButton,
                                tCheckbox,
                                tColorPicker,
                                tCommandTextInput,
                                tDesktopButton,
                                tDropDown
                                tFiletree,
                                tFoldButton,
                                tHTML,
                                tIcon,
                                tImage,
                                tLabel,
                                tLinkLabel,
                                tMarkdown,
                                tRadioButton,
                                tScrollbar,
                                tSlider,
                                tSlidesView,
                                tTextInput,
                                tTooltip,
                                tView,

                            ],
                            selected: 0
                        }

                        tTests = Tab { name: "Tests", template: CloseableTab, kind: TabTests }
                        tOverview = Tab { name: "Widgetset Overview", template: PermanentTab, kind: TabOverview }
                        tLayoutDemos = Tab { name: "Layout Demos", template: PermanentTab, kind: TabLayoutDemos }
                        tIcon = Tab { name: "Icon", template: PermanentTab, kind: TabIcon }
                        tFoldButton = Tab { name: "FoldButton", template: PermanentTab, kind: TabFoldButton }
                        tDesktopButton = Tab { name: "DesktopButton", template: PermanentTab, kind: TabDesktopButton }
                        tButton = Tab { name: "Button", template: PermanentTab, kind: TabButton }
                        tTextInput = Tab { name: "TextInput", template: PermanentTab, kind: TabTextInput }
                        tTooltip = Tab { name: "Tooltip", template: PermanentTab, kind: TabTooltip }
                        tLabel = Tab { name: "Label", template: PermanentTab, kind: TabLabel }
                        tSlider = Tab { name: "Slider", template: PermanentTab, kind: TabSlider }
                        tHTML = Tab { name: "HTML", template: PermanentTab, kind: TabHTML }
                        tMarkdown = Tab { name: "Markdown", template: PermanentTab, kind: TabMarkdown }
                        tLinkLabel = Tab { name: "LinkLabel", template: PermanentTab, kind: TabLinkLabel }
                        tImage = Tab { name: "Image", template: PermanentTab, kind: TabImage }
                        tView = Tab { name: "View", template: PermanentTab, kind: TabView }
                        tScrollbar = Tab { name: "Scrollbar", template: PermanentTab, kind: TabScrollbar }
                        tFiletree = Tab { name: "FileTree", template: PermanentTab, kind: TabFiletree }
                        tCheckbox = Tab { name: "Checkbox", template: PermanentTab, kind: TabCheckbox }
                        tColorPicker = Tab { name: "ColorPicker", template: PermanentTab, kind: TabColorPicker }
                        tRadioButton = Tab { name: "RadioButton", template: PermanentTab, kind: TabRadioButton }
                        tSlidesView = Tab { name: "SlidesView", template: PermanentTab, kind: TabSlidesView }
                        tCommandTextInput = Tab { name: "CommandTextInput", template: PermanentTab, kind: TabCommandTextInput }
                        tDropDown = Tab { name: "DropDown & PopupMenu", template: PermanentTab, kind: TabDropDown }


                        TabTests = <UIZooTab> {

                            <View> {
                                flow: Right,
                                height: Fit,
                                <P> { text: "TestLabel", width: Fit}
                                <LinkLabel> { text: "TestButton", width: Fit}
                                <FoldButton> {
                                    height: 25, width: 15,
                                    margin: { left: (THEME_SPACE_2) }
                                    animator: { open = { default: off } },
                                }

                                <CheckBox> { text: "TestButton"}
                                <CheckBoxToggle> { text: "TestButton"}
                                <ButtonFlat> { text: "TestButton"}
                                <Button> { text: "TestButton, disabled", enabled: true}
                                <TextInput> { text: "TestButton"}
                                <Slider> { text: "TestButton"}
                                <SliderBig> { text: "TestButton"}
                            }
                            <Hr> {}
                            <H3> { text: "Missing"}
                            <Label> { text: "FoldHeader > Requires Rust code?" }
                            <Label> { text: "NavControl > OS level widget that's not suitable for UI zoo?" }
                            <Label> { text: "ImageBlend > Requires Rust code?" }
                            <Label> { text: "PortalList > not suited for UI Zoo?" }
                            <Label> { text: "Splitter > Can docks be nested to show this?" }
                            <Label> { text: "StackNavigation > not suited for UI Zoo?" }
                            <Label> { text: "SidePanel > Investigate Robrix / Moly for examples." }
                            <Label> { text: "TextFlow > Not a widget in that sense that would need to be part of UI Zoo or that needs to be styled? Is this rather a helper widget for HTML, Markdown and LogList?" }
                            <Label> { text: "TogglePanel > at least the example in experiments/toggle_panel_overlay seems to be broken" }
                            <Label> { text: "ColorPicker > broken?" }
                            <Label> { text: "Tooltip > Investigate Robrix / Moly for examples." }
                            <Label> { text: "VectorLine > example in experiments appears to be broken" }
                            <Label> { text: "VectorSpline > example in experiments appears to be broken" }
                        }

                        TabOverview = <UIZooTab> {
                            <View> {
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                height: Fit,
                                <Button> { text: "<Button>" }
                                <ButtonIcon> {
                                    draw_icon: {
                                        color: #f00,
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                    text: "<ButtonIcon>"
                                }
                                <ButtonFlat> { text: "<ButtonFlat>"}
                                <ButtonFlatter> { text: "<ButtonFlatter>"}
                                <ButtonGradientY> { text: "<ButtonGradientX>" }
                            }
                            <Hr> {}
                            <View> {
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                height: Fit,
                                <LinkLabel> { text: "<LinkLabel>", width: Fit}
                                <LinkLabelGradientX> { text: "<LinkLabelGradientX>", width: Fit}
                                <LinkLabelGradientY> { text: "<LinkLabelGradientY>", width: Fit}
                            }
                            <Hr> {}
                            <View> {
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                height: Fit,
                                <Icon> {
                                    icon_walk: { width: 50.  }
                                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                }

                                <IconGradientX> {
                                    icon_walk: { width: 50.  }
                                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                }
                                
                                <IconGradientY> {
                                    icon_walk: { width: 50.  }
                                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                }
                            }
                            <Hr> {}
                            <View> {
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                height: Fit,
                                <CheckBox> { text: "<CheckBox>"}
                                <CheckBoxToggle> { text: "<CheckBoxToggle>"}
                                <CheckBoxCustom> {
                                    text:"<CheckBoxCustom>"
                                    draw_bg: { check_type: None }
                                    draw_icon: {
                                        svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                    }
                                }
                                <CheckBoxCustom> {
                                    text:"<CheckBoxCustom>"
                                    draw_bg: { check_type: None }
                                    draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg") }
                                    draw_icon: {
                                        color: #300
                                        color_hover: #800
                                        color_active: #F00
                                    }
                                    icon_walk: { width: 13.0, height: Fit }
                                }
                            }
                            <Hr> {}
                            <View> {
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                height: Fit,
                                <DesktopButton> { draw_bg: { button_type: WindowsMax} }
                                <DesktopButton> { draw_bg: { button_type: WindowsMaxToggled} }
                                <DesktopButton> { draw_bg: { button_type: WindowsClose} }
                                <DesktopButton> { draw_bg: { button_type: XRMode} }
                                <DesktopButton> { draw_bg: { button_type: Fullscreen } }
                            }
                            <Hr> {}
                            <View> {
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                height: Fit,
                                dropdown = <DropDown> {
                                    labels: ["<DropDown>", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                                    values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                                }
                                dropdown_flat = <DropDownFlat> {
                                    labels: ["<DropDownFlat>", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                                    values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                                }
                                dropdown_gradient_x = <DropDownGradientX> {
                                    labels: ["<DropDownGradientX>", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                                    values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                                }
                                dropdown_gradient_y = <DropDownGradientY> {
                                    labels: ["<DropDownGradientY>", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                                    values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                                }
                            }
                            <Hr> {}
                            <View> {
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                height: Fit,
                                <TextInput> { empty_message: "<TextInput>"}
                                <TextInputGradientY> { empty_message: "<TextInputGradientY>"}
                            }
                            <Hr> {}
                            <View> {
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                height: Fit,
                                width: Fit,
                                <View> {
                                    flow: Down,
                                    radio1 = <RadioButton> { text: "<RadioButton>" }
                                    radio2 = <RadioButton> { text: "Option 2" }
                                    radio3 = <RadioButton> { text: "Option 3" }
                                    radio4 = <RadioButton> { text: "Option 4" }
                                }

                                iconradios_demo = <View> {
                                    width: Fit, height: Fit,
                                    spacing: (THEME_SPACE_2)
                                    flow: Down,

                                    radio1 = <RadioButtonCustom> {
                                        text: "<RadioButtonCustom> 1"
                                        icon_walk: { width: 12.5, height: Fit }
                                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                        
                                    }
                                    radio2 = <RadioButtonCustom> {
                                        text: "<RadioButtonCustom> 2"
                                        icon_walk: { width: 12.5, height: Fit, }
                                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                    }
                                    radio3 = <RadioButtonCustom> {
                                        text: "<RadioButtonCustom> 3"
                                        icon_walk: { width: 12.5, height: Fit, }
                                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                    }
                                    radio4 = <RadioButtonCustom> {
                                        text: "<RadioButtonCustom> 4"
                                        icon_walk: { width: 12.5, height: Fit, }
                                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                    }
                                }

                                textonlyradios_demo = <View> {
                                    width: Fit, height: Fit,
                                    flow: Down,
                                    spacing: (THEME_SPACE_2)
                                    radio1 = <RadioButtonTextual> { text: "<RadioButtonTextual> 1" }
                                    radio2 = <RadioButtonTextual> { text: "<RadioButtonTextual> 2" }
                                    radio3 = <RadioButtonTextual> { text: "<RadioButtonTextual> 3" }
                                    radio4 = <RadioButtonTextual> { text: "<RadioButtonTextual> 4" }
                                }

                                radiotabs_demo = <View> {
                                    spacing: 5.
                                    width: Fit, height: Fit,
                                    flow: Down,
                                    radio1 = <RadioButtonTab> { text: "<RadioButtonTab> 1" }
                                    radio2 = <RadioButtonTab> { text: "<RadioButtonTab> 2" }
                                    radio3 = <RadioButtonTab> { text: "<RadioButtonTab> 3" }
                                    radio4 = <RadioButtonTab> { text: "<RadioButtonTab> 4" }
                                }

                                <ButtonGroup> {
                                    height: Fit
                                    align: { x: 0.0, y: 0.5 }
                                    radiotabs_demo = <View> {
                                        flow: Down,
                                        spacing: 5.
                                        width: Fit, height: Fit,
                                        radio1 = <RadioButtonTabGradientY> { text: "<RadioButtonTabGradientY> 1" }
                                        radio2 = <RadioButtonTabGradientY> { text: "<RadioButtonTabGradientY> 2" }
                                        radio3 = <RadioButtonTabGradientY> { text: "<RadioButtonTabGradientY> 3" }
                                        radio4 = <RadioButtonTabGradientY> { text: "<RadioButtonTabGradientY> 4" }
                                    }
                                }
                            }
                            <Hr> {}
                            <View> {
                                flow: Right,
                                spacing: (THEME_SPACE_2)
                                height: Fit,
                                <Slider> { text: "TestButton"}
                                <SliderBig> { text: "TestButton"}
                                <SliderAlt1> { text: "TestButton"}
                                <Rotary> { text: "TestButton"}
                                <RotaryFlat> { text: "TestButton"}
                                <RotarySolid> { text: "TestButton"}
                            }
                        }

                        TabLayoutDemos = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<XXX>"}
                                }
                                demos = {
                                }
                            }
                        }

                        TabDesktopButton = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<DesktopButton>"}
                                }
                                demos = {
                                    <DesktopButton> { draw_bg: { button_type: WindowsMax} }
                                    <DesktopButton> { draw_bg: { button_type: WindowsMaxToggled} }
                                    <DesktopButton> { draw_bg: { button_type: WindowsClose} }
                                    <DesktopButton> { draw_bg: { button_type: XRMode} }
                                    <DesktopButton> { draw_bg: { button_type: Fullscreen } }
                                }
                            }
                        }


                        TabFoldButton = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<FoldButton>"}
                                }
                                demos = {
                                    <FoldButton> { }
                                }
                            }
                        }

                        TabScrollbar = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<ScrollBar>"}
                                }
                                demos = {
                                    <H1> { text: "Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up. Just some random Text to trigger the Scrollbar widget to show up."}
                                    scroll_bars: <ScrollBars> { }
                                }
                            }
                        }

                        TabImageBlend = <UIZooTab> {
                        }

                        TabIcon = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<Icon>"}
                                }
                                demos = {
                                    <Icon> {
                                        icon_walk: { width: 100.  }
                                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                    }

                                    <IconGradientX> {
                                        icon_walk: { width: 100.  }
                                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                    }
                                    
                                    <IconGradientY> {
                                        icon_walk: { width: 100.  }
                                        draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                    }
                                }
                            }

                        }

                        TabColorPicker = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<ColorPicker>"}
                                }
                                demos = {
                                    // <ColorPicker> {}
                                }
                            }

                        }

                        TabDropDown = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<CommandTextInput>"}
                                }
                                demos = {
                                    dropdown = <DropDown> {
                                    labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                                        values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                                    }
                                    dropdown_flat = <DropDownFlat> {
                                        labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                                        values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                                    }
                                    dropdown_gradient_x = <DropDownGradientX> {
                                        labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                                        values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                                    }
                                    dropdown_gradient_y = <DropDownGradientY> {
                                        labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                                        values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]
                                    }
                                    dropdown_custom = <DropDownGradientY> {
                                        labels: ["Value One", "Value Two", "Third", "Fourth Value", "Option E", "Hexagons"],
                                        values: [ValueOne, ValueTwo, Third, FourthValue, OptionE, Hexagons]

                                        
                                        draw_text: {
                                            color: #f00
                                            color_hover: #0f0
                                            color_focus: #0ff
                                        }

                                    
                                        draw_bg: {
                                            border_size: (THEME_BEVELING)
                                            border_radius: (THEME_CORNER_RADIUS)

                                            color_dither: 1.0

                                            color_1: (THEME_COLOR_CTRL_DEFAULT * 1.75)
                                            color_1_hover: #0
                                            color_1_focus: #2

                                            color_2: (THEME_COLOR_CTRL_DEFAULT)
                                            color_2_hover: #3
                                            color_2_focus: #4

                                            border_color_1: #8
                                            border_color_1_hover: #C
                                            border_color_1_focus: #A

                                            border_color_2: #8
                                            border_color_2_hover: #C
                                            border_color_2_focus: #A
                                        }

                                        popup_menu: <PopupMenuGradientY> {
                                            // menu_item: <PopupMenuItem> {}

                                            draw_bg: {
                                                color_dither: 1.0
                                                border_radius: 4.0
                                                border_size: (THEME_BEVELING)
                                                inset: vec4(0.0, 0.0, 0.0, 0.0),

                                                color_1: #4
                                                color_2: #2

                                                border_color_1: #C
                                                border_color_2: #C
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        TabCommandTextInput = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<CommandTextInput>"}
                                }
                                demos = {
                                    <CommandTextInput> {}
                                }
                            }
                        }
                        
                        TabSlidesView = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<SlidesView>"}
                                }
                                demos = {
                                    <SlidesView> {
                                        width: Fill, height: Fill,

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
                            
                        }

                        TabRadioButton = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<RadioButton>"}
                                }
                                demos = {
                                    <H4> { text: "Default"}
                                    <View> {
                                        height: Fit
                                        flow: Right
                                        align: { x: 0.0, y: 0.5 }
                                        radios_demo = <View> {
                                            spacing: (THEME_SPACE_2)
                                            width: Fit, height: Fit,
                                            radio1 = <RadioButton> {
                                                text: "Option 1"

                                                label_walk: {
                                                    width: Fit, height: Fit,
                                                    margin: { left: 20. }
                                                }

                                                label_align: { y: 0.0 }
                                                
                                                draw_bg: {
                                                    border_size: (THEME_BEVELING)

                                                    color_dither: 1.0

                                                    color_1: #F00
                                                    color_1_hover: #F44
                                                    color_1_active: #F00

                                                    color_2: #F80
                                                    color_2_hover: #FA4
                                                    color_2_active: #F80

                                                    border_color_1: #0
                                                    border_color_1_hover: #F
                                                    border_color_1_active: #8

                                                    border_color_2: #0
                                                    border_color_2_hover: #F
                                                    border_color_2_active: #8

                                                    mark_color: #FFF0
                                                    mark_color_hover: #FFFF
                                                    mark_color_active: #FFFC
                                                    
                                                }
                                                    
                                                draw_text: {
                                                    color: #A
                                                    color_hover: #F
                                                    color_active: #C

                                                    text_style: <THEME_FONT_REGULAR> {
                                                        font_size: (THEME_FONT_SIZE_P)
                                                    }
                                                }

                                                icon_walk: { width: 13.0, height: Fit }
                                                    
                                                draw_icon: {
                                                    color_1: #F00
                                                    color_1_hover: #F44
                                                    color_1_active: #F00

                                                    color_2: #F00
                                                    color_2_hover: #F44
                                                    color_2_active: #F00
                                                }
                                            }
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
                                                icon_walk: { width: 12.5, height: Fit }
                                                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }

                                                label_align: { y: 0.0 }
                                                
                                                draw_text: {
                                                    color: #A
                                                    color_hover: #F
                                                    color_active: #C

                                                    text_style: <THEME_FONT_REGULAR> {
                                                        font_size: (THEME_FONT_SIZE_P)
                                                    }
                                                }

                                                draw_icon: {
                                                    color_1: #000
                                                    color_1_hover: #F44
                                                    color_1_active: #F00

                                                    color_2: #F00
                                                    color_2_hover: #F44
                                                    color_2_active: #F00
                                                }
                                            }
                                            radio2 = <RadioButtonCustom> {
                                                text: "Option 2"
                                                icon_walk: {
                                                    width: 12.5, height: Fit,
                                                }
                                                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                            }
                                            radio3 = <RadioButtonCustom> {
                                                text: "Option 3"
                                                icon_walk: {
                                                    width: 12.5, height: Fit,
                                                }
                                                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
                                            }
                                            radio4 = <RadioButtonCustom> {
                                                text: "Option 4"
                                                icon_walk: {
                                                    width: 12.5, height: Fit,
                                                }
                                                draw_icon: { svg_file: dep("crate://self/resources/Icon_Favorite.svg"), }
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
                                            radio1 = <RadioButtonTextual> { 
                                                text: "Option 1"

                                                draw_text: {
                                                    color: #C80,
                                                    color_hover: #FC0,
                                                    color_active: #FF4,
                                                        
                                                    text_style: <THEME_FONT_REGULAR> {
                                                        font_size: (THEME_FONT_SIZE_P)
                                                    }
                                                }
                                            }
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
                                            spacing: 5.
                                            width: Fit, height: Fit,
                                            radio1 = <RadioButtonTab> {
                                                text: "Option 1"

                                                icon_walk: {
                                                    width: 12.5, height: Fit,
                                                }
                                                label_walk: {
                                                    margin: { left: 5. }
                                                }
                                                draw_icon: {
                                                    svg_file: dep("crate://self/resources/Icon_Favorite.svg")

                                                    color_1: #0
                                                    color_1_hover: #FF0
                                                    color_1_active: #BB0

                                                    color_2: #0
                                                    color_2_hover: #F00
                                                    color_2_active: #B00
                                                }

                                                draw_text: {
                                                    color: #0
                                                    color_hover: #C
                                                    color_active: #F
                                                }

                                                draw_bg: {
                                                    border_size: 1.,
                                                    border_radius: 4.,

                                                    color_dither: 1.0
                                                    color: #F00
                                                    color_hover: #F44
                                                    color_active: #300

                                                    border_color_1: #0
                                                    border_color_1_hover: #F
                                                    border_color_1_active: #8

                                                    border_color_2: #0
                                                    border_color_2_hover: #F
                                                    border_color_2_active: #8
                                                }
                                            }
                                            radio2 = <RadioButtonTab> { text: "Option 2" }
                                            radio3 = <RadioButtonTab> { text: "Option 3" }
                                            radio4 = <RadioButtonTab> { text: "Option 4" }
                                        }
                                    }

                                    <ButtonGroup> {
                                        height: Fit
                                        flow: Right
                                        align: { x: 0.0, y: 0.5 }
                                        radiotabs_demo = <View> {
                                            spacing: 5.
                                            width: Fit, height: Fit,
                                            radio1 = <RadioButtonTabGradientY> {
                                                text: "Option 1"

                                                draw_text: {
                                                    color: #0
                                                    color_hover: #C
                                                    color_active: #F
                                                }

                                                draw_bg: {
                                                    border_size: (THEME_BEVELING)
                                                    border_radius: 6.

                                                    color_dither: 1.0

                                                    color_1: #F00
                                                    color_1_hover: #F44
                                                    color_1_active: #300

                                                    color_2: #F80
                                                    color_2_hover: #FA4
                                                    color_2_active: #310

                                                    border_color_1: #0
                                                    border_color_1_hover: #F
                                                    border_color_1_active: #8

                                                    border_color_2: #0
                                                    border_color_2_hover: #F
                                                    border_color_2_active: #8
                                                }
                                            }
                                            radio2 = <RadioButtonTabGradientY> { text: "Option 2" }
                                            radio3 = <RadioButtonTabGradientY> { text: "Option 3" }
                                            radio4 = <RadioButtonTabGradientY> { text: "Option 4" }
                                        }
                                    }

                                    <ButtonGroup> {
                                        height: Fit
                                        flow: Right
                                        align: { x: 0.0, y: 0.5 }
                                        radiotabs_demo = <View> {
                                            spacing: 5.
                                            width: Fit, height: Fit,
                                            radio1 = <RadioButtonTabGradientX> {
                                                text: "Option 1"

                                                draw_text: {
                                                    color: #0
                                                    color_hover: #C
                                                    color_active: #F
                                                }

                                                draw_bg: {
                                                    border_size: (THEME_BEVELING)

                                                    color_dither: 1.0

                                                    color_1: #F00
                                                    color_1_hover: #F44
                                                    color_1_active: #300

                                                    color_2: #F80
                                                    color_2_hover: #FA4
                                                    color_2_active: #310

                                                    border_color_1: #0
                                                    border_color_1_hover: #F
                                                    border_color_1_active: #8

                                                    border_color_2: #0
                                                    border_color_2_hover: #F
                                                    border_color_2_active: #8
                                                }
                                            }
                                            radio2 = <RadioButtonTabGradientX> { text: "Option 2" }
                                            radio3 = <RadioButtonTabGradientX> { text: "Option 3" }
                                            radio4 = <RadioButtonTabGradientX> { text: "Option 4" }
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
                        }


                        TabCheckbox = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "Checkbox"}
                                    <P> {
                                        text: "The `CheckBox` widget provides a control for user input in the form of a checkbox. It allows users to select or deselect options."
                                    }

                                    <H4> { text: "Layouting"}
                                    <P> {
                                        text: "Complete layouting feature set support."
                                    }

                                    <H4> { text: "Draw Shaders"}
                                    <P> {
                                        text: "Complete layouting feature set support."
                                    }
                                }
                                demos = {
                                    <H3> { text: "Demos"}
                                    <Hr> {}
                                    <H4> { text: "Standard Mode"}
                                    <View> {
                                        height: Fit
                                        flow: Right
                                        spacing: (THEME_SPACE_1)
                                        align: { x: 0.0, y: 0.5}
                                        <CheckBox> {text:"Check me out!"}
                                    }
                                    <H4> { text: "Customized"}
                                    <View> {
                                        height: Fit
                                        flow: Right
                                        spacing: (THEME_SPACE_1)
                                        align: { x: 0.0, y: 0.5}
                                        CheckBoxCustomized = <CheckBox> {
                                            text:"Check me out!"

                                            label_walk: {
                                                width: Fit, height: Fit,
                                                margin: <THEME_MSPACE_H_1> { left: 12.5 }
                                            }

                                            draw_bg: {
                                                border_size: 1.0

                                                color_1: #F40
                                                color_1_hover: #F44
                                                color_1_active: #F00

                                                color_2: #F80
                                                color_2_hover: #FA4
                                                color_2_active: #F80

                                                border_color_1: #0
                                                border_color_1_hover: #F
                                                border_color_1_active: #8

                                                border_color_2: #0
                                                border_color_2_hover: #F
                                                border_color_2_active: #8

                                                mark_color: #FFF0
                                                mark_color_hover: #FFF0
                                                mark_color_active: #FFFC
                                            }  
                                        
                                            draw_text: {
                                                color: #A
                                                color_hover: #F
                                                color_active: #C

                                                text_style: <THEME_FONT_REGULAR> {
                                                    font_size: (THEME_FONT_SIZE_P)
                                                }
                                            }

                                            draw_icon: {
                                                color: #F00
                                                color_hover: #F44
                                                color_active: #F00
                                            }

                                            icon_walk: { width: 13.0, height: Fit }
                                        }

                                    }

                                    <Hr> {}

                                    <H4> { text: "Toggle Mode"}
                                    <View> {
                                        height: Fit
                                        flow: Right
                                        spacing: (THEME_SPACE_1)
                                        align: { x: 0.0, y: 0.5}
                                        <CheckBoxToggle> {text:"Check me out!" }
                                        <CheckBoxToggle> {text:"Check me out!" }
                                    }
                                    <H4> { text: "Toggle Customized"}
                                    <CheckBoxToggle> {
                                        text:"Check me out!"

                                        draw_bg: {
                                            border_size: 1.0

                                            color_1: #F00
                                            color_1_hover: #F44
                                            color_1_active: #F00

                                            color_2: #F80
                                            color_2_hover: #FA4
                                            color_2_active: #F80

                                            border_color_1: #0
                                            border_color_1_hover: #F
                                            border_color_1_active: #8

                                            border_color_2: #0
                                            border_color_2_hover: #F
                                            border_color_2_active: #8

                                            mark_color: #FFFF
                                            mark_color_hover: #FFFF
                                            mark_color_active: #FFFC
                                        }  
                                    
                                        draw_text: {
                                            color: #A
                                            color_hover: #F
                                            color_active: #C

                                            text_style: <THEME_FONT_REGULAR> {
                                                font_size: (THEME_FONT_SIZE_P)
                                            }
                                        }

                                        draw_icon: {
                                            color: #F00
                                            color_hover: #F44
                                            color_active: #F00
                                        }

                                        icon_walk: { width: 13.0, height: Fit }

                                    }
                                    <Hr> {}

                                    <H4> { text: "Custom Icon Mode"}
                                    <View> {
                                        height: Fit
                                        flow: Right
                                        spacing: (THEME_SPACE_1)
                                        align: { x: 0.0, y: 0.5}
                                        <CheckBoxCustom> {
                                            text:"Check me out!"
                                            draw_bg: { check_type: None }
                                            draw_icon: {
                                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                            }

                                            label_walk: {
                                                width: Fit, height: Fit,
                                                margin: <THEME_MSPACE_H_1> { left: 12.5 }
                                            }

                                            draw_bg: {
                                                border_size: 1.0

                                                color_1: #F00
                                                color_1_hover: #F44
                                                color_1_active: #F00

                                                color_2: #F80
                                                color_2_hover: #FA4
                                                color_2_active: #F80

                                                border_color_1: #0
                                                border_color_1_hover: #F
                                                border_color_1_active: #8

                                                border_color_2: #0
                                                border_color_2_hover: #F
                                                border_color_2_active: #8

                                                mark_color: #FFF0
                                                mark_color_hover: #FFFF
                                                mark_color_active: #FFFC
                                            }  
                                        
                                            draw_text: {
                                                color: #330
                                                color_hover: #8
                                                color_active: #F80

                                                text_style: <THEME_FONT_REGULAR> {
                                                    font_size: (THEME_FONT_SIZE_P)
                                                }
                                            }

                                            draw_icon: {
                                                color: #300
                                                color_hover: #800
                                                color_active: #F00
                                            }

                                            icon_walk: { width: 13.0, height: Fit }
                                        }
                                        <CheckBoxCustom> {
                                            text:"Check me out!"
                                            draw_bg: { check_type: None }
                                            draw_icon: {
                                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                            }
                                        }
                                    }
                                    <Hr> {} 
                                    <H4> { text: "Output demo"}
                                    <View> {
                                        height: Fit
                                        flow: Right
                                        align: { x: 0.0, y: 0.5}
                                        simplecheckbox = <CheckBox> {text:"Check me out!"}
                                        simplecheckbox_output = <Label> { text:"hmm" }
                                    }
                                }
                            }
                        }

                        TabFiletree = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<FileTree>"}
                                }
                                demos = {
                                    <DemoFileTree> { file_tree:{ width: Fill, height: Fill } }
                                }
                            }
                        }

                        TabView = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<View>"}
                                }
                                demos = {
                                    <View> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }
                                        <Label> { text: "<View>" }
                                    }
                                    
                                    <Vr> {
                                        draw_bg: {
                                            color_1: #000,
                                            color_2: #000,
                                        }
                                    }

                                    <Hr> {
                                        width: 25.,
                                        draw_bg: {
                                            color_1: #0,
                                            color_2: #5,
                                        }
                                    }

                                    <SolidView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }
                                        draw_bg: { color: #0 }
                                        <Label> { text: "<SolidView>" }
                                    }


                                    <RectView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }
                                        draw_bg: {
                                            color: #000,
                                            border_size: 2.,
                                            border_color: #8,
                                            border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                                        }
                                        <Label> { text: "<RectView>" }
                                    }

                                    <RectShadowView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color: #000,
                                            border_size: 2.0
                                            border_color: #8
                                            shadow_color: #0007
                                            shadow_offset: vec2(5.0,5.0)
                                            shadow_radius: 10.0
                                        }

                                        <Label> { text: "<RectShadowView>" }
                                    }

                                    <RoundedShadowView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color: #000,
                                            border_radius: 5.,
                                            border_color: #8
                                            border_size: 2.0
                                            shadow_color: #0007
                                            shadow_radius: 10.0
                                            shadow_offset: vec2(5.0,5.0)
                                        }

                                        <Label> { text: "<RoundedShadowView>" }
                                    }

                                    <RoundedView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color: #000,
                                            border_radius: 5.,
                                            border_size: 2.0
                                            border_color: #8
                                            border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                                        }

                                        <Label> { text: "<RoundedView>" }
                                    }

                                    <RoundedXView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color: #000,
                                            border_radius: vec2(1.0, 5.0),
                                            border_size: 2.0
                                            border_color: #8
                                            border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                                        }

                                        <Label> { text: "<RoundedXView>" }
                                    }

                                    <RoundedYView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color: #000,
                                            border_size: 2.0
                                            border_color: #8
                                            border_radius: vec2(1.0, 5.0),
                                            border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                                        }

                                        <Label> { text: "<RoundedYView>" }
                                    }

                                    <RoundedAllView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color: #000,
                                            border_size: 2.0
                                            border_color: #8
                                            border_radius: vec4(1.0, 5.0, 2.0, 3.0),
                                            border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                                        }

                                        <Label> { text: "<RoundedAllView>" }
                                    }

                                    <CircleView> {
                                        width: Fit, height: Fit, 
                                        padding: 15.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color: #0,
                                            border_size: 2.0
                                            border_color: #8
                                            border_radius: 30.,
                                            border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                                        }

                                        <Label> { text: "<CircleView>" }
                                    }

                                    <HexagonView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color: #0,
                                            border_size: 2.0
                                            border_color: #8
                                            border_inset: vec4(0.0, 0.0, 0.0, 0.0)
                                            border_radius: vec2(0.0, 0.0)
                            
                                        }

                                        <Label> { text: "<HexagonView>" }
                                    }

                                    <GradientXView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color_1: #f00,
                                            color_2: #f80,
                                            color_dither: 2.0
                                        }

                                        <Label> { text: "<GradientXView>" }
                                    }

                                    <GradientYView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        draw_bg: {
                                            color_1: #f00,
                                            color_2: #f80,
                                            color_dither: 2.0
                                        }

                                        <Label> { text: "<GradientYView>" }
                                    }

                                    <CachedView> {
                                        width: Fit, height: Fit, 
                                        padding: 10.,
                                        align: { x: 0.5, y: 0.5 }

                                        <View> {
                                            width: Fit, height: Fit,
                                            show_bg: true, 
                                            draw_bg: { color: #0 }

                                            <Label> { text: "<CachedView>" }
                                        }

                                    }

                                    <CachedRoundedView> {
                                        width: Fit, height: Fit, 
                                        padding: 0.,
                                        align: { x: 0.5, y: 0.5 }
                                        draw_bg: {
                                            border_size: 2.0
                                            border_color: #8
                                            border_inset: vec4(0., 0., 0., 0.)
                                            border_radius: 2.5
                                        }

                                        <View> {
                                            width: Fit, height: Fit,
                                            padding: 10.,
                                            show_bg: true, 
                                            draw_bg: { color: #0 }

                                            <Label> { text: "<CachedRoundedView>" }
                                        }

                                    }

                                    <CachedScrollXY> {
                                        width: 100, height: 100, 
                                        padding: 10.,
                                        align: { x: 0., y: 0. }

                                        <View> {
                                            width: 400., height: 400.,
                                            flow: Down,
                                            show_bg: true, 
                                            draw_bg: { color: #0 }

                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                            <Label> { text: "<CachedScrollXY> <CachedScrollXY> <CachedScrollXY>" }
                                        }
                                    }

                                    <CachedScrollX> {
                                        width: 100, height: 100, 
                                        padding: 10.,
                                        align: { x: 0., y: 0. }

                                        <View> {
                                            width: 400., height: 400.,
                                            flow: Down,
                                            show_bg: true, 
                                            draw_bg: { color: #0 }

                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                            <Label> { text: "<CachedScrollX> <CachedScrollX> <CachedScrollX>" }
                                        }
                                    }

                                    <CachedScrollY> {
                                        width: 100, height: 100, 
                                        padding: 10.,
                                        align: { x: 0., y: 0. }

                                        <View> {
                                            width: 400., height: 400.,
                                            flow: Down,
                                            show_bg: true, 
                                            draw_bg: { color: #0 }

                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                            <Label> { text: "<CachedScrollY> <CachedScrollY> <CachedScrollY>" }
                                        }
                                    }

                                    <ScrollXYView> {
                                        width: 100, height: 100, 
                                        padding: 10.,
                                        align: { x: 0., y: 0. }
                                        show_bg: true,
                                        draw_bg: {
                                            color: #8
                                        }

                                        <View> {
                                            width: 400., height: 400.,
                                            flow: Down,
                                            show_bg: true, 
                                            draw_bg: { color: #0 }

                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                            <Label> { text: "<ScrollXYView> <ScrollXYView> <ScrollXYView>" }
                                        }
                                    }

                                    <ScrollXView> {
                                        width: 100, height: 100, 
                                        padding: 10.,
                                        align: { x: 0., y: 0. }
                                        show_bg: true,
                                        draw_bg: {
                                            color: #8
                                        }

                                        <View> {
                                            width: 400., height: 400.,
                                            flow: Down,
                                            show_bg: true, 
                                            draw_bg: { color: #0 }

                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                            <Label> { text: "<ScrollXView> <ScrollXView> <ScrollXView>" }
                                        }
                                    }

                                    <ScrollYView> {
                                        width: 100, height: 100, 
                                        padding: 10.,
                                        align: { x: 0., y: 0. }
                                        show_bg: true,
                                        draw_bg: {
                                            color: #8
                                        }

                                        <View> {
                                            width: 400., height: 400.,
                                            flow: Down,
                                            show_bg: true, 
                                            draw_bg: { color: #0 }

                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                            <Label> { text: "<ScrollYView> <ScrollYView> <ScrollYView>" }
                                        }
                                    }
                                }
                            }
                        } 

                        TabImage = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<Image>"}
                                }
                                demos = {
                                    flow: Right,

                                    <View> {
                                        width: Fit, height: Fit, flow: Down,
                                        <View> {
                                            show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250, flow: Down,
                                            <Image> { source: dep("crate://self/resources/ducky.png" ) }
                                        }
                                        <P> { text: "Default" }
                                    }
                                    <View> {
                                        width: Fit, height: Fit, flow: Down,
                                        <View> {
                                            show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                                            <Image> { height: Fill, source: dep("crate://self/resources/ducky.png" ), min_height: 100 }
                                        }
                                        <P> { text: "min_height: 100" } // TODO: get this to work correctly
                                    }
                                    <View> {
                                        width: Fit, height: Fit, flow: Down,
                                        <View> {
                                            show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                                            <Image> { width: Fill, source: dep("crate://self/resources/ducky.png" ), width_scale: 1.1 }
                                        }
                                        <P> { text: "width_scale: 1.5" } // TODO: get this to work correctly
                                    }
                                    <View> {
                                        width: Fit, height: Fit, flow: Down,
                                        <View> {
                                            show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                                            <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png"), fit: Stretch }
                                        }
                                        <P> { text: "fit: Stretch" }
                                    }
                                    <View> {
                                        width: Fit, height: Fit, flow: Down,
                                        <View> {
                                            show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                                            <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Horizontal }
                                        }
                                        <P> { text: "fit: Horizontal" }
                                    }
                                    <View> {
                                        width: Fit, height: Fit, flow: Down,
                                        <View> {
                                            show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                                            <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Vertical }
                                        }
                                        <P> { text: "fit: Vertical" }
                                    }
                                    <View> {
                                        width: Fit, height: Fit, flow: Down,
                                        <View> {
                                            show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                                            <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Smallest }
                                        }
                                        <P> { text: "fit: Smallest" }
                                    }
                                    <View> {
                                        width: Fit, height: Fit, flow: Down,
                                        <View> {
                                            show_bg: true, draw_bg: { color: (THEME_COLOR_D_1)}, width: 125, height: 250,
                                            <Image> { width: Fill, height: Fill, source: dep("crate://self/resources/ducky.png" ), fit: Biggest }
                                        }
                                        <P> { text: "fit: Biggest" }
                                    }
                                }
                            }
                        }

                            TabLinkLabel = <UIZooTab> {
                                <UIZooTabLayout_A> {
                                    desc = {
                                        <H3> { text: "<LinkLabel>"}
                                    }
                                    demos = {
                                        <View> {
                                            width: Fill, height: Fit,
                                            spacing: (THEME_SPACE_2)
                                            <LinkLabel> {
                                                draw_bg: {
                                                    color: #0AA
                                                    color_hover: #0FF
                                                    color_down: #0
                                                }

                                                draw_text: {
                                                    color: #0AA
                                                    color_hover: #0FF
                                                    color_down: #0
                                                }

                                                text: "Click me!"
                                            }
                                            <LinkLabel> { text: "Click me!"}
                                            <LinkLabel> { text: "Click me!"}
                                        }
                                        <View> {
                                            width: Fill, height: Fit,
                                            spacing: (THEME_SPACE_2)
                                            <LinkLabelGradientY> { text: "<LinkLabelGradientY>"}
                                            <LinkLabelGradientX> { text: "<LinkLabelGradientX>"}
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
                            }

                            TabButton = <UIZooTab> {
                                <UIZooTabLayout_A> {
                                    desc = {
                                        <H3> { text: "<XXX>"}
                                    }
                                    demos = {
                                
                                        basicbutton = <Button> {

                                            draw_text: {
                                                color: (THEME_COLOR_TEXT_DEFAULT)
                                                color_hover: (THEME_COLOR_TEXT_HOVER)
                                                color_down: (THEME_COLOR_TEXT_PRESSED)
                                                text_style: <THEME_FONT_REGULAR> {
                                                    font_size: (THEME_FONT_SIZE_P)
                                                }
                                            }


                                            icon_walk: {
                                                width: (THEME_DATA_ICON_WIDTH), height: Fit,
                                            }

                                            draw_icon: {
                                                color: (THEME_COLOR_TEXT_DEFAULT)
                                                color_hover: (THEME_COLOR_TEXT_HOVER)
                                                color_down: (THEME_COLOR_TEXT_PRESSED)
                                            }

                                            draw_bg: {
                                                border_radius: (THEME_BEVELING)
                                                border_radius: (THEME_CORNER_RADIUS)

                                                color: (THEME_COLOR_CTRL_DEFAULT)
                                                color_hover: (THEME_COLOR_CTRL_HOVER)
                                                color_down: (THEME_COLOR_CTRL_PRESSED)

                                                border_color_1: (THEME_COLOR_BEVEL_LIGHT)
                                                border_color_1_hover: (THEME_COLOR_BEVEL_LIGHT)
                                                border_color_1_down: (THEME_COLOR_BEVEL_SHADOW)

                                                border_color_2: (THEME_COLOR_BEVEL_SHADOW)
                                                border_color_2_hover: (THEME_COLOR_BEVEL_SHADOW)
                                                border_color_2_down: (THEME_COLOR_BEVEL_LIGHT)
                                            }

                                            text: "<Button>"
                                        }

                                        iconbutton = <ButtonIcon> {
                                            draw_icon: {
                                                color: #f00,
                                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                            }
                                            text: "<ButtonIcon>"
                                        }

                                        <ButtonGradientX> { text: "<ButtonGradientX>" }
                                        <ButtonGradientX> {
                                            draw_bg: {
                                                border_radius: 1.0,
                                                border_radius: 4.0

                                                color_1: #C00
                                                color_1_hover: #F0F
                                                color_1_down: #800

                                                color_2: #0CC
                                                color_2_hover: #0FF
                                                color_2_down: #088

                                                border_color_1: #C
                                                border_color_1_hover: #F
                                                border_color_1_down: #0

                                                border_color_2: #3
                                                border_color_2_hover: #6
                                                border_color_2_down: #8

                                            }
                                            text: "<ButtonGradientX>"
                                        }

                                        <ButtonGradientY> { text: "<ButtonGradientY>" }
                                        <ButtonGradientY> {
                                            draw_bg: {
                                                border_radius: 1.0,
                                                border_radius: 4.0

                                                color_1: #C00
                                                color_1_hover: #F0F
                                                color_1_down: #800

                                                color_2: #0CC
                                                color_2_hover: #0FF
                                                color_2_down: #088

                                                border_color_1: #C
                                                border_color_1_hover: #F
                                                border_color_1_down: #0

                                                border_color_2: #3
                                                border_color_2_hover: #6
                                                border_color_2_down: #8

                                            }
                                            text: "<ButtonGradientY>"
                                        }

                                        <ButtonFlat> {
                                            draw_icon: {
                                                color: #f00,
                                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                            }
                                            text: "<ButtonFlat>"
                                        }

                                        <ButtonFlat> {
                                            flow: Down,
                                            icon_walk: { width: 15. }
                                            draw_icon: {
                                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                            }
                                            text: "<ButtonFlat> (Vertical)"
                                        }

                                        <ButtonFlatter> {
                                            draw_icon: {
                                                color: #f00,
                                                svg_file: dep("crate://self/resources/Icon_Favorite.svg"),
                                            }
                                            text: "<ButtonFlatter>"
                                        }

                                        styledbutton = <Button> {
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
                                                        self.down
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
                                                        self.down
                                                    )
                                                }
                                            }

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

                                            text: "Freely Styled <Button> clicked: 0", // Text label.

                                            animator: { // State change triggered animations.
                                                hover = { // State
                                                    default: off // The state's starting point.
                                                    off = { // Behavior when the animation is started to the off-state
                                                        from: { // Behavior depending on the prior states
                                                            all: Forward {duration: 0.1}, // Default animation direction and speed in secs.
                                                            down: Forward {duration: 0.25} // Direction and speed for 'pressed' in secs.
                                                        }
                                                        apply: { // Shader methods to animate
                                                            draw_bg: { down: 0.0, hover: 0.0 } // Timeline target positions for the given states.
                                                            draw_icon: { down: 0.0, hover: 0.0 }
                                                            draw_text: { down: 0.0, hover: 0.0 }
                                                        }
                                                    }

                                                    on = { // Behavior when the animation is started to the on-state
                                                        from: {
                                                            all: Forward {duration: 0.1},
                                                            pressed: Forward {duration: 0.5}
                                                        }
                                                        apply: {
                                                            draw_bg: { down: 0.0, hover: [{time: 0.0, value: 1.0}] },
                                                            // pressed: 'pressed' timeline target position
                                                            // hover, time: Normalized timeline from 0.0 - 1.0. 'duration' then determines the actual playback duration of this animation in seconds.
                                                            // hover, value: target timeline position
                                                            draw_icon: { down: 0.0, hover: [{time: 0.0, value: 1.0}] },
                                                            draw_text: { down: 0.0, hover: [{time: 0.0, value: 1.0}] }
                                                        }
                                                    }
                                        
                                                    pressed = { // Behavior when the animation is started to the pressed-state
                                                        from: {all: Forward {duration: 0.2}}
                                                        apply: {
                                                            draw_bg: {down: [{time: 0.0, value: 1.0}], hover: 1.0}, 
                                                            draw_icon: {down: [{time: 0.0, value: 1.0}], hover: 1.0},
                                                            draw_text: {down: [{time: 0.0, value: 1.0}], hover: 1.0}
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
                                }
                            }
                        }

                        TabTextInput = <UIZooTab> {

                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<TextInput>"}
                                }
                                demos = {
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
                                    <View> {
                                        height: Fit, width: Fill,
                                        spacing: (THEME_SPACE_2),
                                        <H4> { text: "TextInputGradientY", width: 175.}
                                        <TextInputGradientY> { empty_message: "Inline Label" }
                                    }
                                    <View> {
                                        height: Fit, width: Fill,
                                        spacing: (THEME_SPACE_2),
                                        <H4> { text: "TextInputGradientX", width: 175.}
                                        <TextInputGradientX> { empty_message: "Inline Label" }
                                    }

                                    <View> {
                                        height: Fit, width: Fill,
                                        spacing: (THEME_SPACE_2),
                                        <H4> { text: "Customized A", width: 175.}
                                        <TextInputGradientX> {
                                            draw_bg: {
                                                border_radius: 7.
                                                border_size: 1.5

                                                color_dither: 1.0

                                                color_1: #F
                                                color_1_hover: #F
                                                color_1_focus: #F

                                                color_2: #AA0
                                                color_2_hover: #FF0
                                                color_2_focus: #CC0

                                                border_color_1: (THEME_COLOR_BEVEL_SHADOW)
                                                border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
                                                border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

                                                border_color_2: (THEME_COLOR_BEVEL_LIGHT)
                                                border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
                                                border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)
                                            }

                                            draw_text: {
                                                color: #3
                                                color_hover: #484848
                                                color_focus: #0
                                                color_empty: #7
                                                color_empty_focus: #6

                                                wrap: Word,

                                                text_style: <THEME_FONT_REGULAR> {
                                                    line_spacing: (THEME_FONT_LINE_SPACING),
                                                    font_size: (THEME_FONT_SIZE_P)
                                                }

                                                fn get_color(self) -> vec4 {
                                                    return
                                                    mix(
                                                        mix(
                                                            mix(self.color, self.color_hover, self.hover),
                                                            self.color_focus,
                                                            self.focus
                                                        ),
                                                        mix(self.color_empty, self.color_empty_focus, self.hover),
                                                        self.is_empty
                                                    )
                                                }
                                            }

                                            draw_highlight: {
                                                color_1: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
                                                color_1_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.4)
                                                color_1_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.2)

                                                color_2: #0AA
                                                color_2_hover: #0FF
                                                color_2_focus: #0CC
                                            }

                                            draw_cursor: { color: #f00 }

                                            empty_message: "Inline Label"
                                        }

                                        <TextInputGradientY> {
                                            draw_bg: {
                                                border_radius: 7.
                                                border_size: 1.5

                                                color_dither: 1.0

                                                color_1: #F
                                                color_1_hover: #F
                                                color_1_focus: #F

                                                color_2: #AA0
                                                color_2_hover: #FF0
                                                color_2_focus: #CC0

                                                border_color_1: (THEME_COLOR_BEVEL_SHADOW)
                                                border_color_1_hover: (THEME_COLOR_BEVEL_SHADOW)
                                                border_color_1_focus: (THEME_COLOR_BEVEL_SHADOW)

                                                border_color_2: (THEME_COLOR_BEVEL_LIGHT)
                                                border_color_2_hover: (THEME_COLOR_BEVEL_LIGHT)
                                                border_color_2_focus: (THEME_COLOR_BEVEL_LIGHT)
                                            }

                                            draw_text: {
                                                color: #3
                                                color_hover: #484848
                                                color_focus: #0
                                                color_empty: #7
                                                color_empty_focus: #6

                                                wrap: Word,

                                                text_style: <THEME_FONT_REGULAR> {
                                                    line_spacing: (THEME_FONT_LINE_SPACING),
                                                    font_size: (THEME_FONT_SIZE_P)
                                                }

                                                fn get_color(self) -> vec4 {
                                                    return
                                                    mix(
                                                        mix(
                                                            mix(self.color, self.color_hover, self.hover),
                                                            self.color_focus,
                                                            self.focus
                                                        ),
                                                        mix(self.color_empty, self.color_empty_focus, self.hover),
                                                        self.is_empty
                                                    )
                                                }
                                            }

                                            draw_highlight: {
                                                color_1: (THEME_COLOR_BG_HIGHLIGHT_INLINE)
                                                color_1_hover: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.4)
                                                color_1_focus: (THEME_COLOR_BG_HIGHLIGHT_INLINE * 1.2)

                                                color_2: #0AA
                                                color_2_hover: #0FF
                                                color_2_focus: #0CC
                                            }

                                            draw_cursor: { color: #f00 }

                                            empty_message: "Inline Label"
                                        }
                                    }
                                }
                            }
                        }


                        TabTooltip = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<Tooltip>"}
                                }
                                demos = {
                                    <H4> { text: "Default", width: 175.}
                                    <TextInput> { }
                                }
                            }
                        }


                        TabLabel = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<Label>"}
                                }
                                demos = {
                                    <Label> { text:"Default single line text" }
                                    <Label> { <Label> { text: "This is a small line of text" } }
                                    <Label> {
                                        draw_text: {
                                            color: (COLOR_ACCENT)
                                            text_style: {
                                                font_size: 20,
                                            }
                                        },
                                        text: "You can style text using colors and fonts"
                                    }
                                    <LabelGradientX> {
                                        draw_text: {
                                            color_1: #0ff
                                            color_1: #088
                                            text_style: {
                                                font_size: 20,
                                            }
                                        },
                                        
                                        text: "<LabelGradientX>"
                                    }
                                    <LabelGradientX> { text: "<LabelGradientY>" }
                                    <LabelGradientY> {
                                        draw_text: {
                                            color_1: #0ff
                                            color_1: #088
                                            text_style: {
                                                font_size: 20,
                                            }
                                        },
                                        
                                        text: "<LabelGradientX>"
                                    }
                                    <LabelGradientY> { text: "<LabelGradientY>" }
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

                                    <Hr> {}

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
                        }

                        TabSlider = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<Slider>"}
                                }
                                demos = {
                                    flow: Right,
                                    <View> {
                                        flow: Down,
                                        <Slider> { text: "Default" }
                                        <Slider> { text: "label_align", label_align: { x: 0.5, y: 0. } }
                                        <Slider> { text: "min/max", min: 0., max: 100. }
                                        <Slider> { text: "precision", precision: 20 }
                                        <Slider> { text: "stepped", step: 0.1 }
                                        <SliderBig> { text: "Default" }
                                        <SliderBig> { text: "label_align", label_align: { x: 0.5, y: 0. } }
                                        <SliderBig> { text: "min/max", min: 0., max: 100. }
                                        <SliderBig> { text: "precision", precision: 20 }
                                        <SliderBig> { text: "stepped", step: 0.1 }
                                        <SliderAlt1> {
                                            text: "Colored",
                                            draw_slider: {
                                                val_color_1: #FFCC00
                                                val_color_1_hover: #FF9944
                                                val_color_1_focus: #FFCC44
                                                val_color_1_drag: #FFAA00

                                                val_color_2: #F00
                                                val_color_2_hover: #F00
                                                val_color_2_focus: #F00
                                                val_color_2_drag: #F00

                                                handle_color: #0000
                                                handle_color_hover: #0008
                                                handle_color_focus: #000C
                                                handle_color_drag: #000F
                                            }
                                        }
                                        <SliderAlt1> {
                                            text: "Solid",
                                            draw_text: {
                                                color: #0ff;
                                            }
                                            draw_slider: {
                                                val_color_1: #F08
                                                val_color_1_hover: #F4A
                                                val_color_1_focus: #C04
                                                val_color_1_drag: #F08

                                                val_color_2: #F08
                                                val_color_2_hover: #F4A
                                                val_color_2_focus: #C04
                                                val_color_2_drag: #F08

                                                handle_color: #F
                                                handle_color_hover: #F
                                                handle_color_focus: #F
                                                handle_color_drag: #F
                                            }
                                        }
                                        <SliderAlt1> {
                                            text: "Solid",
                                            draw_slider: {
                                                val_color_1: #6,
                                                val_color_2: #6,
                                                handle_color: #0,
                                            }
                                        }
                                        <SliderAlt1> { text: "min/max", min: 0., max: 100. }
                                        <SliderAlt1> { text: "precision", precision: 20 }
                                        <SliderAlt1> { text: "stepped", step: 0.1 }
                                        <SliderAlt1> {
                                            text: "label_size",
                                            draw_slider: {label_size: 150. },
                                        }
                                    }
                                    <View> {
                                        flow: Down,
                                        <View> {
                                            width: Fill, height: Fit,
                                            flow: Right,
                                            <Rotary> {
                                                width: 100, height: 100,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 90.,
                                                    val_size: 20.
                                                    val_padding: 2.,
                                                }
                                            }
                                            <Rotary> {
                                                width: 100, height: 200,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 60.,
                                                    val_size: 10.,
                                                    val_padding: 4.,
                                                }
                                            }
                                            <Rotary> {
                                                width: 200, height: 100,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 75.,
                                                    val_size: 20.
                                                    val_padding: 4,
                                                }
                                            }
                                            <Rotary> {
                                                width: 200, height: 150,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 90.,
                                                    val_size: 20.
                                                    val_padding: 4.,
                                                }
                                            }
                                            <Rotary> {
                                                width: Fill, height: 150,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 60.,
                                                    val_size: 20.
                                                    val_padding: 2.,
                                                }
                                            }
                                        }
                                        <View> {
                                            width: Fill, height: Fit,
                                            flow: Right,
                                            <Rotary> {
                                                width: 100., height: 100.,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 0.,
                                                    val_size: 20.
                                                    val_padding: 0.,
                                                }
                                            }
                                            <Rotary> {
                                                width: 120., height: 120.,
                                                text: "Solid",
                                                draw_text: {
                                                    color: #0ff;
                                                }
                                                draw_slider: {
                                                    val_color_1: #ff0,
                                                    val_color_2: #f00,
                                                    handle_color: #f,
                                                    gap: 180.,
                                                    val_size: 20.,
                                                    val_padding: 2.,
                                                }
                                            }
                                            <Rotary> {
                                                width: 120., height: 120.,
                                                text: "Solid",
                                                draw_slider: {
                                                    val_color_1: #0ff,
                                                    val_color_2: #ff0,
                                                    handle_color: #f,
                                                    gap: 90.,
                                                    val_size: 20.,
                                                    val_padding: 2.,
                                                }
                                            }
                                            <Rotary> {
                                                width: 100., height: 90.,
                                                text: "Solid",
                                                draw_slider: {
                                                    gap: 90.,
                                                    val_padding: 10.,
                                                    val_size: 20.,
                                                    val_padding: 2.
                                                    handle_color: #f0f,
                                                }
                                            }
                                            <Rotary> {
                                                width: 150., height: 150.,
                                                text: "Solid",
                                                draw_slider: {
                                                    val_color_1: #0ff,
                                                    val_color_2: #0ff,
                                                    gap: 180.,
                                                    val_padding: 4.,
                                                    val_size: 6.,
                                                }
                                            }
                                            <Rotary> {
                                                width: 150., height: 150.,
                                                text: "Solid",
                                                draw_slider: {
                                                    gap: 0.,
                                                    val_size: 10.0,
                                                    val_padding: 4.,
                                                }
                                            }
                                        }
                                        
                                        <View> {
                                            width: Fill, height: Fit,
                                            flow: Right,
                                            <RotaryFlat> {
                                                width: 100., height: 100.,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 0.,
                                                    width: 20.
                                                    val_padding: 0.,
                                                }
                                            }
                                            <RotaryFlat> {
                                                width: 120., height: 120.,
                                                text: "Solid",
                                                draw_text: {
                                                    color: #0ff;
                                                }
                                                draw_slider: {
                                                    val_color_1: #ff0,
                                                    val_color_2: #f00,
                                                    handle_color: #f,
                                                    gap: 180.,
                                                    width: 20.,
                                                    val_padding: 2.,
                                                }
                                            }
                                            <RotaryFlat> {
                                                width: 120., height: 120.,
                                                text: "Solid",
                                                draw_slider: {
                                                    val_color_1: #0ff,
                                                    val_color_2: #ff0,
                                                    handle_color: #f,
                                                    gap: 90.,
                                                    width: 20.,
                                                    val_padding: 2.,
                                                }
                                            }
                                            <RotaryFlat> {
                                                width: 100., height: 90.,
                                                text: "Solid",
                                                draw_slider: {
                                                    gap: 90.,
                                                    val_padding: 10.,
                                                    width: 20.,
                                                    handle_color: #f0f,
                                                }
                                            }
                                            <RotaryFlat> {
                                                width: 150., height: 150.,
                                                text: "Solid",
                                                draw_slider: {
                                                    val_color_1: #0ff,
                                                    val_color_2: #0ff,
                                                    gap: 180.,
                                                    val_padding: 4.,
                                                    width: 6.,
                                                }
                                            }
                                            <RotaryFlat> {
                                                width: Fill, height: 150.,
                                                text: "Solid",
                                                draw_slider: {
                                                    val_color_1: #8;
                                                    val_color_2: #ff0;
                                                    gap: 75.,
                                                    width: 40.0,
                                                    val_padding: 4.,
                                                }
                                            }
                                        }
                                        <View> {
                                            width: Fill, height: Fit,
                                            flow: Right,
                                            <RotarySolid> {
                                                width: 100, height: 100,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 90.,
                                                }
                                            }
                                            <RotarySolid> {
                                                width: 200, height: 150,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 180.,
                                                }
                                            }
                                            <RotarySolid> {
                                                width: Fill, height: 150,
                                                text: "Colored",
                                                draw_slider: {
                                                    gap: 60.,
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                        }

                        TabHTML = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<FoldButton>"}
                                }
                                demos = {
                                    <Html> {
                                        width:Fill, height:Fit,
                                        body:"<H1>H1 Headline</H1><H2>H2 Headline</H2><H3>H3 Headline</H3><H4>H4 Headline</H4><H5>H5 Headline</H5><H6>H6 Headline</H6>This is <b>bold</b>&nbsp;and <i>italic text</i>.<sep><b><i>Bold italic</i></b>, <u>underlined</u>, and <s>strike through</s> text. <p>This is a paragraph</p> <code>A code block</code>. <br/> And this is a <a href='https://www.google.com/'>link</a><br/><ul><li>lorem</li><li>ipsum</li><li>dolor</li></ul><ol><li>lorem</li><li>ipsum</li><li>dolor</li></ol><br/> <blockquote>Blockquote</blockquote> <pre>pre</pre><sub>sub</sub><del>del</del>"
                                    }
                                }
                            }
                        }

                        TabMarkdown = <UIZooTab> {
                            <UIZooTabLayout_A> {
                                desc = {
                                    <H3> { text: "<MarkDown>"}
                                }
                                demos = {
                                    <Markdown> {
                                        width:Fill, height: Fit,
                                        body:"# Headline 1 \n ## Headline 2 \n ### Headline 3 \n #### Headline 4 \n This is standard text with a  \n\n line break a short ~~strike through~~ demo.\n\n *Italic text* \n\n **Bold text** \n\n - Bullet\n - Another bullet\n\n - Third bullet\n\n 1. Numbered list Bullet\n 2. Another list entry\n\n 3. Third list entry\n\n `Monospaced text`\n\n> This is a quote.\n\nThis is `inline code`.\n\n ```code block```"
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

    #[derive(Live, LiveHook, PartialEq, LiveAtomic, Debug, LiveRead)]
    pub enum DropDownEnum {
        #[pick]
        ValueOne,
        ValueTwo,
        Third,
        FourthValue,
        OptionE,
        Hexagons,
    }

    #[derive(Live, LiveHook, LiveRead, LiveRegister)]
    pub struct DataBindingsForApp {
        #[live] fnumber: f32,
        #[live] inumber: i32,
        #[live] dropdown: DropDownEnum,
        #[live] dropdown_flat: DropDownEnum,
        #[live] dropdown_gradient_x: DropDownEnum,
        #[live] dropdown_gradient_y: DropDownEnum,
        #[live] dropdown_custom: DropDownEnum,
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
            lbl.set_text(cx,&format!("{} {}" , self.counter, txt));
        }

        if self.ui.button(id!(basicbutton)).clicked(&actions) {
            log!("BASIC BUTTON CLICKED {}", self.counter);
            self.counter += 1;
            let btn = self.ui.button(id!(basicbutton));
            btn.set_text(cx,&format!("Clicky clicky! {}", self.counter));
        }

        if self.ui.button(id!(styledbutton)).clicked(&actions) {
            log!("STYLED BUTTON CLICKED {}", self.counter);
            self.counter += 1;
            let btn = self.ui.button(id!(styledbutton));
            btn.set_text(cx,&format!("Styled button clicked: {}", self.counter));
        }

        if self.ui.button(id!(iconbutton)).clicked(&actions) {
            log!("ICON BUTTON CLICKED {}", self.counter);
            self.counter += 1;
            let btn = self.ui.button(id!(iconbutton));
            btn.set_text(cx,&format!("Icon button clicked: {}", self.counter));
        }


        if let Some(check) = self.ui.check_box(id!(simplecheckbox)).changed(actions) {
            log!("CHECK BUTTON CLICKED {} {}", self.counter, check);
            self.counter += 1;
            let lbl = self.ui.label(id!(simplecheckbox_output));
            lbl.set_text(cx,&format!("{} {}" , self.counter, check));
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
        db.bind(id!(dropdown_flat), ids!(dropdown_flat));
        db.bind(id!(dropdown_gradient_x), ids!(dropdown_gradient_x));
        db.bind(id!(dropdown_gradient_y), ids!(dropdown_gradient_y));
        db.bind(id!(dropdown_custom), ids!(dropdown_custom));
    }
}