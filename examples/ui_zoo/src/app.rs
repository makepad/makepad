    use makepad_widgets::*;
    use makepad_platform::live_atomic::*;

    live_design!{
        use link::theme::*;
        use link::shaders::*;
        use link::widgets::*;
        use makepad_widgets::vectorline::*;
        use crate::layout_templates::*;

        use crate::tab_adaptiveview::*;
        use crate::tab_button::*;
        use crate::tab_checkbox::*;
        use crate::tab_colorpicker::*;
        use crate::tab_commandtextinput::*;
        use crate::tab_desktopbutton::*;
        use crate::tab_dropdown::*;
        use crate::tab_expandablepanel::*;
        use crate::tab_filetree::*;
        use crate::tab_foldbutton::*;
        use crate::tab_html::*;
        use crate::tab_icon::*;
        use crate::tab_image::*;
        use crate::tab_imageblend::*;
        use crate::tab_label::*;
        use crate::tab_layout::*;
        use crate::tab_linklabel::*;
        use crate::tab_markdown::*;
        use crate::tab_radiobutton::*;
        use crate::tab_rotatedimage::*;
        use crate::tab_scrollbar::*;
        use crate::tab_slider::*;
        use crate::tab_slidesview::*;
        use crate::tab_textinput::*;
        use crate::tab_tooltip::*;
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
                                tOverview,
                                tLayoutDemos,
                                tAdaptiveView,
                                tButton,
                                tCheckbox,
                                tColorPicker,
                                tCommandTextInput,
                                tDesktopButton,
                                tDropDown,
                                tExpandablePanel,
                                tFiletree,
                                tFoldButton,
                                tHTML,
                                tIcon,
                                tImage,
                                tImageBlend,
                                tLabel,
                                tLinkLabel,
                                tMarkdown,
                                tRadioButton,
                                tRotatedImage,
                                tScrollbar,
                                tSlider,
                                tSlidesView,
                                tTextInput,
                                tTooltip,
                                tView,

                            ],
                            selected: 0
                        }

                        tOverview = Tab { name: "Widgetset Overview", template: PermanentTab, kind: TabOverview }
                        tLayoutDemos = Tab { name: "Layout Demos", template: PermanentTab, kind: TabLayoutDemos }
                        tAdaptiveView = Tab { name: "Adaptive View", template: PermanentTab, kind: TabAdaptiveView }
                        tButton = Tab { name: "Button", template: PermanentTab, kind: TabButton }
                        tCheckbox = Tab { name: "Checkbox", template: PermanentTab, kind: TabCheckbox }
                        tColorPicker = Tab { name: "ColorPicker", template: PermanentTab, kind: TabColorPicker }
                        tCommandTextInput = Tab { name: "CommandTextInput", template: PermanentTab, kind: TabCommandTextInput }
                        tDesktopButton = Tab { name: "DesktopButton", template: PermanentTab, kind: TabDesktopButton }
                        tDropDown = Tab { name: "DropDown & PopupMenu", template: PermanentTab, kind: TabDropDown }
                        tExpandablePanel = Tab { name: "Expandable Panel" , template: PermanentTab, kind: TabExpandablePanel }
                        tFiletree = Tab { name: "FileTree", template: PermanentTab, kind: TabFiletree }
                        tFoldButton = Tab { name: "FoldButton", template: PermanentTab, kind: TabFoldButton }
                        tHTML = Tab { name: "HTML", template: PermanentTab, kind: TabHTML }
                        tIcon = Tab { name: "Icon", template: PermanentTab, kind: TabIcon }
                        tImage = Tab { name: "Image", template: PermanentTab, kind: TabImage }
                        tImageBlend = Tab { name: "ImageBlend", template: PermanentTab, kind: TabImageBlend }
                        tLabel = Tab { name: "Label", template: PermanentTab, kind: TabLabel }
                        tLinkLabel = Tab { name: "LinkLabel", template: PermanentTab, kind: TabLinkLabel }
                        tMarkdown = Tab { name: "Markdown", template: PermanentTab, kind: TabMarkdown }
                        tRadioButton = Tab { name: "RadioButton", template: PermanentTab, kind: TabRadioButton }
                        tRotatedImage = Tab { name: "RotatedImage", template: PermanentTab, kind: TabRotatedImage }
                        tScrollbar = Tab { name: "Scrollbar", template: PermanentTab, kind: TabScrollbar }
                        tSlider = Tab { name: "Slider", template: PermanentTab, kind: TabSlider }
                        tSlidesView = Tab { name: "SlidesView", template: PermanentTab, kind: TabSlidesView }
                        tTextInput = Tab { name: "TextInput", template: PermanentTab, kind: TabTextInput }
                        tTooltip = Tab { name: "Tooltip", template: PermanentTab, kind: TabTooltip }
                        tView = Tab { name: "View", template: PermanentTab, kind: TabView }
                        
                        TabOverview = <UIZooTab> { <WidgetsOverview> {} }
                        TabLayoutDemos = <UIZooTab> { <DemoLayout> {} }
                        TabAdaptiveView = <UIZooTab> { <DemoAdaptiveView> {} }
                        TabButton = <UIZooTab> { <DemoButton> {} }
                        TabCheckbox = <UIZooTab> { <DemoCheckbox> {} }
                        TabColorPicker = <UIZooTab> { <DemoColorpicker> {} }
                        TabCommandTextInput = <UIZooTab> { <DemoCommandTextInput> {} }
                        TabDesktopButton = <UIZooTab> { <DemoDesktopButton> {} }
                        TabDropDown = <UIZooTab> { <DemoDropdown> {} }
                        TabExpandablePanel = <UIZooTab> { <DemoExpandablePanel> {} }
                        TabFiletree = <UIZooTab> { <DemoFT> {} }
                        TabFoldButton = <UIZooTab> { <DemoFoldButton> {} }
                        TabHTML = <UIZooTab> { <DemoHtml> {} }
                        TabIcon = <UIZooTab> { <DemoIcon> {} }
                        TabImage = <UIZooTab> { <DemoImage> {} }
                        TabImageBlend = <UIZooTab> { <DemoImageBlend> {} }
                        TabLabel = <UIZooTab> { <DemoLabel> {} }
                        TabLinkLabel = <UIZooTab> {
                            <DemoLinkLabel> {} // TODO: FIX
                        } 
                        TabMarkdown = <UIZooTab> {
                            <DemoMarkdown> {}
                        } 
                        TabRadioButton = <UIZooTab> { <DemoRadioButton> {} }
                        TabRotatedImage = <UIZooTab> { <DemoRotatedImage> {} }
                        TabScrollbar = <UIZooTab> { <DemoScrollBar> {} }
                        TabSlider = <UIZooTab> { <DemoSlider> {} }
                        TabSlidesView = <UIZooTab> { <DemoSlidesView> {} }
                        TabTextInput = <UIZooTab> { <DemoTextInput> {} }
                        TabTooltip = <UIZooTab> { <DemoTooltip> {} }
                        TabView = <UIZooTab> { <DemoView> {} }



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
            crate::layout_templates::live_design(cx);
            crate::demofiletree::live_design(cx);

            crate::tab_adaptiveview::live_design(cx);
            crate::tab_button::live_design(cx);
            crate::tab_checkbox::live_design(cx);
            crate::tab_colorpicker::live_design(cx);
            crate::tab_commandtextinput::live_design(cx);
            crate::tab_desktopbutton::live_design(cx);
            crate::tab_dropdown::live_design(cx);
            crate::tab_expandablepanel::live_design(cx);
            crate::tab_filetree::live_design(cx);
            crate::tab_foldbutton::live_design(cx);
            crate::tab_html::live_design(cx);
            crate::tab_icon::live_design(cx);
            crate::tab_image::live_design(cx);
            crate::tab_imageblend::live_design(cx);
            crate::tab_label::live_design(cx);
            crate::tab_layout::live_design(cx);
            crate::tab_linklabel::live_design(cx);
            crate::tab_markdown::live_design(cx);
            crate::tab_radiobutton::live_design(cx);
            crate::tab_rotatedimage::live_design(cx);
            crate::tab_scrollbar::live_design(cx);
            crate::tab_slider::live_design(cx);
            crate::tab_slidesview::live_design(cx);
            crate::tab_textinput::live_design(cx);
            crate::tab_tooltip::live_design(cx);
            crate::tab_view::live_design(cx);
            crate::tab_widgetsoverview::live_design(cx);
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

        if self.ui.button(id!(blendbutton)).clicked(&actions) {
            self.ui.image_blend(id!(blendimage)).switch_image(cx);
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