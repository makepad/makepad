    use makepad_widgets::*;
    use makepad_platform::live_atomic::*;

    live_design!{
        use link::theme::*;
        use link::shaders::*;
        use link::widgets::*;
        use makepad_widgets::vectorline::*;
        use crate::layout_templates::*;

        use crate::tab_button::*;
        use crate::tab_checkbox::*;
        // use crate::tab_commandtextinput::*;
        // use crate::tab_desktopbutton::*;
        use crate::tab_dropdown::*;
        use crate::tab_filetree::*;
        // use crate::tab_foldbutton::*;
        use crate::tab_html::*;
        use crate::tab_icon::*;
        use crate::tab_iconset::*;
        use crate::tab_image::*;
        use crate::tab_imageblend::*;
        use crate::tab_label::*;
        use crate::tab_layout::*;
        use crate::tab_linklabel::*;
        use crate::tab_markdown::*;
        use crate::tab_pageflip::*;
        use crate::tab_portallist::*;
        use crate::tab_pageflip::*;
        use crate::tab_radiobutton::*;
        use crate::tab_rotary::*;
        use crate::tab_rotatedimage::*;
        use crate::tab_scrollbar::*;
        use crate::tab_slider::*;
        use crate::tab_slidesview::*;
        use crate::tab_textinput::*;
        use crate::tab_view::*;
        use crate::tab_widgetsoverview::*;

        UIZooTab = <RectView> {
            height: Fill, width: Fill
            flow: Down,
            padding: 0
            spacing: 0.
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

                    dock = <Dock> {
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
                                tOverview,
                                tLayoutDemos,
                                tButton,
                                tCheckBox,
                                // tCommandTextInput,
                                // tDesktopButton,
                                tDropDown,
                                tFiletree,
                                // tFoldButton,
                                tHTML,
                                tIcon,
                                tIconSet,
                                tImage,
                                tImageBlend,
                                tLabel,
                                tLinkLabel,
                                tMarkdown,
                                tPageFlip,
                                tPortalList,
                                tRadioButton,
                                tRotary,
                                tRotatedImage,
                                tScrollbar,
                                tSlider,
                                tSlidesView,
                                tTextInput,
                                tView,

                            ],
                            selected: 0
                        }

                        tOverview = Tab { name: "Intro", template: PermanentTab, kind: TabOverview }
                        tLayoutDemos = Tab { name: "Layout Demos", template: PermanentTab, kind: TabLayoutDemos }
                        tButton = Tab { name: "Button", template: PermanentTab, kind: TabButton }
                        tCheckBox = Tab { name: "CheckBox", template: PermanentTab, kind: TabCheckBox }
                        // tCommandTextInput = Tab { name: "CommandTextInput", template: PermanentTab, kind: TabCommandTextInput }
                        // tDesktopButton = Tab { name: "DesktopButton", template: PermanentTab, kind: TabDesktopButton }
                        tDropDown = Tab { name: "DropDown & PopupMenu", template: PermanentTab, kind: TabDropDown }
                        tFiletree = Tab { name: "FileTree", template: PermanentTab, kind: TabFiletree }
                        // tFoldButton = Tab { name: "FoldButton", template: PermanentTab, kind: TabFoldButton }
                        tHTML = Tab { name: "HTML", template: PermanentTab, kind: TabHTML }
                        tIcon = Tab { name: "Icon", template: PermanentTab, kind: TabIcon }
                        tIconSet = Tab { name: "IconSet", template: PermanentTab, kind: TabIconSet }
                        tImage = Tab { name: "Image", template: PermanentTab, kind: TabImage }
                        tImageBlend = Tab { name: "ImageBlend", template: PermanentTab, kind: TabImageBlend }
                        tLabel = Tab { name: "Label", template: PermanentTab, kind: TabLabel }
                        tLinkLabel = Tab { name: "LinkLabel", template: PermanentTab, kind: TabLinkLabel }
                        tMarkdown = Tab { name: "Markdown", template: PermanentTab, kind: TabMarkdown }
                        tPageFlip = Tab { name: "PageFlip", template: PermanentTab, kind: TabPageFlip }
                        tPortalList = Tab { name: "PortalList", template: PermanentTab, kind: TabPortalList }
                        tRadioButton = Tab { name: "RadioButton", template: PermanentTab, kind: TabRadioButton }
                        tRotary = Tab { name: "Rotary", template: PermanentTab, kind: TabRotary }
                        tRotatedImage = Tab { name: "RotatedImage", template: PermanentTab, kind: TabRotatedImage }
                        tScrollbar = Tab { name: "Scrollbar", template: PermanentTab, kind: TabScrollbar }
                        tSlider = Tab { name: "Slider", template: PermanentTab, kind: TabSlider }
                        tSlidesView = Tab { name: "SlidesView", template: PermanentTab, kind: TabSlidesView }
                        tTextInput = Tab { name: "TextInput", template: PermanentTab, kind: TabTextInput }
                        tView = Tab { name: "View", template: PermanentTab, kind: TabView }
                        
                        TabOverview = <UIZooTab> { <WidgetsOverview> {} }
                        TabLayoutDemos = <UIZooTab> { <DemoLayout> {} }
                        TabButton = <UIZooTab> { <DemoButton> {} }
                        TabCheckBox = <UIZooTab> { <DemoCheckBox> {} }
                        // TabCommandTextInput = <UIZooTab> { <DemoCommandTextInput> {} }
                        // TabDesktopButton = <UIZooTab> { <DemoDesktopButton> {} }
                        TabDropDown = <UIZooTab> { <DemoDropdown> {} }
                        TabFiletree = <UIZooTab> { <DemoFT> {} }
                        // TabFoldButton = <UIZooTab> { <DemoFoldButton> {} }
                        TabHTML = <UIZooTab> { <DemoHtml> {} }
                        TabIcon = <UIZooTab> { <DemoIcon> {} }
                        TabIconSet = <UIZooTab> { <DemoIconSet> {} }
                        TabImage = <UIZooTab> { <DemoImage> {} }
                        TabImageBlend = <UIZooTab> { <DemoImageBlend> {} }
                        TabLabel = <UIZooTab> { <DemoLabel> {} }
                        TabLinkLabel = <UIZooTab> { <DemoLinkLabel> {} } 
                        TabMarkdown = <UIZooTab> { <DemoMarkdown> {} } 
                        TabPageFlip = <UIZooTab> { <DemoPageFlip> {} } 
                        TabPortalList = <UIZooTab> { <DemoPortalList> {} } 
                        TabRadioButton = <UIZooTab> { <DemoRadioButton> {} }
                        TabRotary = <UIZooTab> { <DemoRotary> {} }
                        TabRotatedImage = <UIZooTab> { <DemoRotatedImage> {} }
                        TabScrollbar = <UIZooTab> { <DemoScrollBar> {} }
                        TabSlider = <UIZooTab> { <DemoSlider> {} }
                        TabSlidesView = <UIZooTab> { <DemoSlidesView> {} }
                        TabTextInput = <UIZooTab> { <DemoTextInput> {} }
                        TabView = <UIZooTab> { <DemoView> {} }



                    }

                    <View> {
                        width: Fill, height: Fit,
                        flow: Right,
                        spacing: (THEME_SPACE_2)
                        align: { x: 0.0, y: 0.0 }
                        padding: <THEME_MSPACE_2> {}
                        margin: { bottom: (THEME_SPACE_1) }

                        show_bg: true,

                        draw_bg: {
                            uniform color_dither: 1.0
                            uniform border_radius: 0.
                            uniform border_size: (THEME_BEVELING)
                            uniform color_1: (THEME_COLOR_BG_APP * 0.9);
                            uniform color_2: #282828;

                            fn pixel(self) -> vec4 {
                                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                                let dither = Math::random_2d(self.pos.xy) * 0.04 * self.color_dither;

                                sdf.rect(
                                    -2.,
                                    1.,
                                    self.rect_size.x + 2.,
                                    self.rect_size.y + 30.
                                )

                                sdf.fill_keep((THEME_COLOR_U_1));

                                sdf.stroke(
                                    mix((THEME_COLOR_BEVEL_OUTSET_1), #fff0, pow(self.pos.y, 0.1)), self.border_size
                                )
                                return sdf.result
                            }
                        }

                        theme_contrast = <Slider> { text: "Contrast", default: 1.0, min:0.8 max: 2.0 }
                        <View> {
                            flow: Down
                            spacing: 0.
                            <Label> { margin: {top: (THEME_SPACE_1)}, padding: 0., width: Fit, text: "Tint Color"}
                            <TextInput> { empty_text: "Hex color", text: "#ff0000" }
                        }
                        theme_tint_amount = <Slider> { text: "Tint Amount", default: 0.0, min:0.0 max: 1.0  }
                        <Vr> {}
                        <Pbold> {
                            width: Fit,
                            text: "Font"
                        }
                        theme_font_size = <Slider> { text: "Size", default: 7.5, min:3.0 max: 12.0  }
                        theme_font_size_contrast = <Slider> { text: "Size Contrast", default: 2.5, min:1.0 max: 5.0  }
                        <Vr> {}
                        theme_bevel = <Slider> { text: "Bevel", default: 0.75, min:0.0 max: 1.0   }
                        theme_rounding = <Slider> { text: "Rounding", default: 2.5, min:1.0 max: 5.0   }
                        theme_space = <Slider> { text: "Space", default: 6.0, min:1.0 max: 10.0   }
                        /*
                        reload_button = <ButtonFlatterIcon> {
                            height: Fill, width: Fit,
                            margin: { right: (THEME_SPACE_1) }
                            icon_walk: { width: 12.5, height: Fit, }
                            draw_icon: {
                                color: (THEME_COLOR_MAKEPAD),
                                color_hover: (THEME_COLOR_LABEL_INNER_HOVER),
                                color_down: (THEME_COLOR_LABEL_INNER_DOWN),
                                svg_file: dep("crate://self/resources/Icon_Reload.svg"),
                            }
                        }*/
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
        #[live] dropdown_customized: DropDownEnum,
        #[live] dropdown_below: DropDownEnum,
        #[live] dropdown_flat: DropDownEnum,
        #[live] dropdown_flat_below: DropDownEnum,
        #[live] dropdown_flatter: DropDownEnum,
        #[live] dropdown_flatter_below: DropDownEnum,
        #[live] dropdown_gradient_x: DropDownEnum,
        #[live] dropdown_gradient_x_below: DropDownEnum,
        #[live] dropdown_gradient_y: DropDownEnum,
        #[live] dropdown_gradient_y_below: DropDownEnum,
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
            cx.link(live_id!(theme), live_id!(theme_desktop_dark));

            crate::layout_templates::live_design(cx);
            crate::demofiletree::live_design(cx);

            crate::tab_button::live_design(cx);
            crate::tab_checkbox::live_design(cx);
            // crate::tab_commandtextinput::live_design(cx);
            // crate::tab_desktopbutton::live_design(cx);
            crate::tab_dropdown::live_design(cx);
            crate::tab_filetree::live_design(cx);
            // crate::tab_foldbutton::live_design(cx);
            crate::tab_html::live_design(cx);
            crate::tab_icon::live_design(cx);
            crate::tab_iconset::live_design(cx);
            crate::tab_image::live_design(cx);
            crate::tab_imageblend::live_design(cx);
            crate::tab_label::live_design(cx);
            crate::tab_layout::live_design(cx);
            crate::tab_linklabel::live_design(cx);
            crate::tab_markdown::live_design(cx);
            crate::tab_pageflip::live_design(cx);
            crate::tab_portallist::live_design(cx);
            crate::tab_radiobutton::live_design(cx);
            crate::tab_rotary::live_design(cx);
            crate::tab_rotatedimage::live_design(cx);
            crate::tab_scrollbar::live_design(cx);
            crate::tab_slider::live_design(cx);
            crate::tab_slidesview::live_design(cx);
            crate::tab_textinput::live_design(cx);
            crate::tab_view::live_design(cx);
            crate::tab_widgetsoverview::live_design(cx);
          }
    }


    impl MatchEvent for App{
        fn handle_actions(&mut self, cx: &mut Cx, actions:&Actions) {
            let ui = self.ui.clone();

            ui.radio_button_set(ids!(radios_demo_1.radio1, radios_demo_1.radio2, radios_demo_1.radio3, radios_demo_1.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_2.radio1, radios_demo_2.radio2, radios_demo_2.radio3, radios_demo_2.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_3.radio1, radios_demo_3.radio2, radios_demo_3.radio3, radios_demo_3.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_4.radio1, radios_demo_4.radio2, radios_demo_4.radio3, radios_demo_4.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_5.radio1, radios_demo_5.radio2, radios_demo_5.radio3, radios_demo_5.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_6.radio1, radios_demo_6.radio2, radios_demo_6.radio3, radios_demo_6.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_7.radio1, radios_demo_7.radio2, radios_demo_7.radio3, radios_demo_7.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_8.radio1, radios_demo_8.radio2)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_9.radio1, radios_demo_9.radio2, radios_demo_9.radio3, radios_demo_9.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_10.radio1, radios_demo_10.radio2, radios_demo_10.radio3, radios_demo_10.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_11.radio1, radios_demo_11.radio2, radios_demo_11.radio3, radios_demo_11.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_12.radio1, radios_demo_12.radio2, radios_demo_12.radio3, radios_demo_12.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_13.radio1, radios_demo_13.radio2, radios_demo_13.radio3, radios_demo_13.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_14.radio1, radios_demo_14.radio2, radios_demo_14.radio3, radios_demo_14.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_15.radio1, radios_demo_15.radio2, radios_demo_15.radio3, radios_demo_15.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_16.radio1, radios_demo_16.radio2, radios_demo_16.radio3, radios_demo_16.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_17.radio1, radios_demo_17.radio2, radios_demo_17.radio3, radios_demo_17.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_18.radio1, radios_demo_18.radio2, radios_demo_18.radio3, radios_demo_18.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_19.radio1, radios_demo_19.radio2, radios_demo_19.radio3, radios_demo_19.radio4)).selected(cx, actions);
            ui.radio_button_set(ids!(radios_demo_20.radio1, radios_demo_20.radio2, radios_demo_20.radio3, radios_demo_20.radio4)).selected(cx, actions);

            if let Some(txt) = self.ui.text_input(id!(simpletextinput)).changed(&actions){
                log!("TEXTBOX CHANGED {}", self.counter);
                self.counter += 1;
                let lbl = self.ui.label(id!(simpletextinput_outputbox));
                lbl.set_text(cx,&format!("{} {}" , self.counter, txt));
            }
            
            for slider in [
                (id!(theme_contrast), live_id!(THEME_COLOR_CONTRAST)),
                (id!(theme_tint_amount), live_id!(THEME_COLOR_TINT_AMOUNT)),
                (id!(theme_font_size), live_id!(THEME_FONT_SIZE_BASE)),
                (id!(theme_font_size_contrast), live_id!(THEME_FONT_SIZE_CONTRAST)),
                (id!(theme_bevel), live_id!(THEME_BEVELING)),
                (id!(theme_rounding), live_id!(THEME_CORNER_RADIUS)),
                (id!(theme_space), live_id!(THEME_SPACE_FACTOR)),
            ]{
                if let Some(value) = self.ui.slider(slider.0).end_slide(&actions){
                    cx.set_dsl_value(
                        live_id!(makepad_widgets),
                        live_id!(theme_desktop_dark),
                        slider.1,
                        LiveValue::Float64(value)
                    );
                    cx.reload_ui_dsl(); 
                }
            }
            
            use makepad_platform::makepad_live_tokenizer::colorhex::hex_bytes_to_u32;
            
            if let Some(txt) = self.ui.text_input(id!(theme_tint_color)).changed(&actions){
                cx.set_dsl_value(
                    live_id!(makepad_widgets),
                    live_id!(theme_desktop_dark),
                    live_id!(THEME_COLOR_TINT),
                    LiveValue::Color(hex_bytes_to_u32(&txt.into_bytes()).unwrap_or(0x777777ff))
                );
                cx.reload_ui_dsl(); 
            }

            if self.ui.button(id!(basicbutton)).clicked(&actions) {
                log!("BASIC BUTTON CLICKED {}", self.counter);
                self.counter += 1;
                let btn = self.ui.button(id!(basicbutton));
                btn.set_text(cx,&format!("Clicky clicky! {}", self.counter));
            }

            if self.ui.button(id!(blendbutton)).clicked(&actions) {
                self.ui.image_blend(id!(blendimage)).switch_image(cx);
            }

            if self.ui.button(id!(pageflipbutton_a)).clicked(&actions) {
                self.ui.page_flip(id!(page_flip)).set_active_page(cx, live_id!(page_a));
            }

            if self.ui.button(id!(pageflipbutton_b)).clicked(&actions) {
                self.ui.page_flip(id!(page_flip)).set_active_page(cx, live_id!(page_b));
            }

            if self.ui.button(id!(pageflipbutton_c)).clicked(&actions) {
                self.ui.page_flip(id!(page_flip)).set_active_page(cx, live_id!(page_c));
            }

            if self.ui.button(id!(styledbutton)).clicked(&actions) {
                log!("STYLED BUTTON CLICKED {}", self.counter);
                self.counter += 1;
                let btn = self.ui.button(id!(styledbutton));
                btn.set_text(cx,&format!("Styled button clicked: {}", self.counter));
            }

            if self.ui.button(id!(find)).clicked(&actions) {
                
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
        db.bind(id!(dropdown_disabled), ids!(dropdown_disabled));
        db.bind(id!(dropdown_demo), ids!(dropdown_demo));
        db.bind(id!(dropdown_flat), ids!(dropdown_flat));
        db.bind(id!(dropdown_flatter), ids!(dropdown_flatter));
        db.bind(id!(dropdown_gradient_x), ids!(dropdown_gradient_x));
        db.bind(id!(dropdown_gradient_y), ids!(dropdown_gradient_y));
        db.bind(id!(dropdown_custom), ids!(dropdown_custom));
    }
}
