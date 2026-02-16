use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let UIZooTab = RectView{
        height: Fill width: Fill
        flow: Down
        padding: 0
        spacing: 0.
    }

    let AppDock = Dock{
        height: Fill width: Fill

        root := DockSplitter{
            axis: SplitterAxis.Horizontal
            align: SplitterAlign.FromA(0.0)
            a: @tab_set_1
            b: @tab_set_2
        }

        tab_set_1 := DockTabs{
            tabs: [@tab_a]
            selected: 0
            closable: false
        }

        tab_set_2 := DockTabs{
            tabs: [
                @tOverview
                @tLayoutDemos
                @tButton
                @tCheckBox
                @tDropDown
                @tFiletree
                @tSpinner
                @tHTML
                @tIcon
                @tIconSet
                @tImage
                @tImageBlend
                @tLabel
                @tLinkLabel
                @tMarkdown
                @tPageFlip
                @tPortalList
                @tRadioButton
                @tRotary
                @tRotatedImage
                @tScrollbar
                @tSlider
                @tSlidesView
                @tTextInput
                @tView
            ]
            selected: 0
            closable: false
        }

        tab_a := DockTab{
            name: "Welcome"
            template: @PermanentTab
            kind: @TabOverview
        }

        tOverview := DockTab{name: "Intro" template: @PermanentTab kind: @TabOverview}
        tLayoutDemos := DockTab{name: "Layout Demos" template: @PermanentTab kind: @TabLayoutDemos}
        tButton := DockTab{name: "Button" template: @PermanentTab kind: @TabButton}
        tCheckBox := DockTab{name: "CheckBox" template: @PermanentTab kind: @TabCheckBox}
        tDropDown := DockTab{name: "DropDown" template: @PermanentTab kind: @TabDropDown}
        tFiletree := DockTab{name: "FileTree" template: @PermanentTab kind: @TabFiletree}
        tSpinner := DockTab{name: "Spinner" template: @PermanentTab kind: @TabSpinner}
        tHTML := DockTab{name: "HTML" template: @PermanentTab kind: @TabHTML}
        tIcon := DockTab{name: "Icon" template: @PermanentTab kind: @TabIcon}
        tIconSet := DockTab{name: "IconSet" template: @PermanentTab kind: @TabIconSet}
        tImage := DockTab{name: "Image" template: @PermanentTab kind: @TabImage}
        tImageBlend := DockTab{name: "ImageBlend" template: @PermanentTab kind: @TabImageBlend}
        tLabel := DockTab{name: "Label" template: @PermanentTab kind: @TabLabel}
        tLinkLabel := DockTab{name: "LinkLabel" template: @PermanentTab kind: @TabLinkLabel}
        tMarkdown := DockTab{name: "Markdown" template: @PermanentTab kind: @TabMarkdown}
        tPageFlip := DockTab{name: "PageFlip" template: @PermanentTab kind: @TabPageFlip}
        tPortalList := DockTab{name: "PortalList" template: @PermanentTab kind: @TabPortalList}
        tRadioButton := DockTab{name: "RadioButton" template: @PermanentTab kind: @TabRadioButton}
        tRotary := DockTab{name: "Rotary" template: @PermanentTab kind: @TabRotary}
        tRotatedImage := DockTab{name: "RotatedImage" template: @PermanentTab kind: @TabRotatedImage}
        tScrollbar := DockTab{name: "Scrollbar" template: @PermanentTab kind: @TabScrollbar}
        tSlider := DockTab{name: "Slider" template: @PermanentTab kind: @TabSlider}
        tSlidesView := DockTab{name: "SlidesView" template: @PermanentTab kind: @TabSlidesView}
        tTextInput := DockTab{name: "TextInput" template: @PermanentTab kind: @TabTextInput}
        tView := DockTab{name: "View" template: @PermanentTab kind: @TabView}

        TabOverview := UIZooTab{WidgetsOverview{}}
        TabLayoutDemos := UIZooTab{DemoLayout{}}
        TabButton := UIZooTab{DemoButton{}}
        TabCheckBox := UIZooTab{DemoCheckBox{}}
        TabDropDown := UIZooTab{DemoDropdown{}}
        TabFiletree := UIZooTab{DemoFT{}}
        TabSpinner := UIZooTab{DemoSpinner{}}
        TabHTML := UIZooTab{DemoHtml{}}
        TabIcon := UIZooTab{DemoIcon{}}
        TabIconSet := UIZooTab{DemoIconSet{}}
        TabImage := UIZooTab{DemoImage{}}
        TabImageBlend := UIZooTab{DemoImageBlend{}}
        TabLabel := UIZooTab{DemoLabel{}}
        TabLinkLabel := UIZooTab{DemoLinkLabel{}}
        TabMarkdown := UIZooTab{DemoMarkdown{}}
        TabPageFlip := UIZooTab{DemoPageFlip{}}
        TabPortalList := UIZooTab{DemoPortalList{}}
        TabRadioButton := UIZooTab{DemoRadioButton{}}
        TabRotary := UIZooTab{DemoRotary{}}
        TabRotatedImage := UIZooTab{DemoRotatedImage{}}
        TabScrollbar := UIZooTab{DemoScrollBar{}}
        TabSlider := UIZooTab{DemoSlider{}}
        TabSlidesView := UIZooTab{DemoSlidesView{}}
        TabTextInput := UIZooTab{DemoTextInput{}}
        TabView := UIZooTab{DemoView{}}
    }

    mod.gc.set_static(AppDock)
    mod.gc.run()

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1200 800)
                body +: {
                    flow: Down
                    spacing: 0.
                    margin: 0.

                    dock := AppDock{}
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets::script_mod(vm);
        crate::layout_templates::script_mod(vm);
        crate::demofiletree::script_mod(vm);
        crate::tab_button::script_mod(vm);
        crate::tab_checkbox::script_mod(vm);
        crate::tab_dropdown::script_mod(vm);
        crate::tab_filetree::script_mod(vm);
        crate::tab_spinner::script_mod(vm);
        crate::tab_html::script_mod(vm);
        crate::tab_icon::script_mod(vm);
        crate::tab_iconset::script_mod(vm);
        crate::tab_image::script_mod(vm);
        crate::tab_imageblend::script_mod(vm);
        crate::tab_label::script_mod(vm);
        crate::tab_layout::script_mod(vm);
        crate::tab_linklabel::script_mod(vm);
        crate::tab_markdown::script_mod(vm);
        crate::tab_pageflip::script_mod(vm);
        crate::tab_portallist::script_mod(vm);
        crate::tab_radiobutton::script_mod(vm);
        crate::tab_rotary::script_mod(vm);
        crate::tab_rotatedimage::script_mod(vm);
        crate::tab_scrollbar::script_mod(vm);
        crate::tab_slider::script_mod(vm);
        crate::tab_slidesview::script_mod(vm);
        crate::tab_textinput::script_mod(vm);
        crate::tab_view::script_mod(vm);
        crate::tab_widgetsoverview::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    counter: usize,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let ui = self.ui.clone();

        ui.radio_button_set(
            cx,
            ids_array!(
                radios_demo_1.radio1,
                radios_demo_1.radio2,
                radios_demo_1.radio3,
                radios_demo_1.radio4
            ),
        )
        .selected(cx, actions);

        ui.radio_button_set(
            cx,
            ids_array!(
                radios_demo_2.radio1,
                radios_demo_2.radio2,
                radios_demo_2.radio3,
                radios_demo_2.radio4
            ),
        )
        .selected(cx, actions);

        ui.radio_button_set(
            cx,
            ids_array!(
                radios_demo_3.radio1,
                radios_demo_3.radio2,
                radios_demo_3.radio3,
                radios_demo_3.radio4
            ),
        )
        .selected(cx, actions);

        ui.radio_button_set(
            cx,
            ids_array!(
                radios_demo_11.radio1,
                radios_demo_11.radio2,
                radios_demo_11.radio3,
                radios_demo_11.radio4
            ),
        )
        .selected(cx, actions);

        ui.radio_button_set(
            cx,
            ids_array!(
                radios_demo_12.radio1,
                radios_demo_12.radio2,
                radios_demo_12.radio3,
                radios_demo_12.radio4
            ),
        )
        .selected(cx, actions);

        if let Some(txt) = self
            .ui
            .text_input(cx, ids!(simpletextinput))
            .changed(&actions)
        {
            log!("TEXTBOX CHANGED {}", self.counter);
            self.counter += 1;
            let lbl = self.ui.label(cx, ids!(simpletextinput_outputbox));
            lbl.set_text(cx, &format!("{} {}", self.counter, txt));
        }

        if self.ui.button(cx, ids!(basicbutton)).clicked(&actions) {
            log!("BASIC BUTTON CLICKED {}", self.counter);
            self.counter += 1;
            let btn = self.ui.button(cx, ids!(basicbutton));
            btn.set_text(cx, &format!("Clicky clicky! {}", self.counter));
        }

        if self.ui.button(cx, ids!(blendbutton)).clicked(&actions) {
            self.ui.image_blend(cx, ids!(blendimage)).switch_image(cx);
        }

        if self.ui.button(cx, ids!(pageflipbutton_a)).clicked(&actions) {
            self.ui
                .page_flip(cx, ids!(page_flip))
                .set_active_page(cx, live_id!(page_a));
        }

        if self.ui.button(cx, ids!(pageflipbutton_b)).clicked(&actions) {
            self.ui
                .page_flip(cx, ids!(page_flip))
                .set_active_page(cx, live_id!(page_b));
        }

        if self.ui.button(cx, ids!(pageflipbutton_c)).clicked(&actions) {
            self.ui
                .page_flip(cx, ids!(page_flip))
                .set_active_page(cx, live_id!(page_c));
        }

        if self.ui.button(cx, ids!(iconbutton)).clicked(&actions) {
            log!("ICON BUTTON CLICKED {}", self.counter);
            self.counter += 1;
            let btn = self.ui.button(cx, ids!(iconbutton));
            btn.set_text(cx, &format!("Icon button clicked: {}", self.counter));
        }

        if let Some(check) = self.ui.check_box(cx, ids!(simplecheckbox)).changed(actions) {
            log!("CHECK BUTTON CLICKED {} {}", self.counter, check);
            self.counter += 1;
            let lbl = self.ui.label(cx, ids!(simplecheckbox_output));
            lbl.set_text(cx, &format!("{} {}", self.counter, check));
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
