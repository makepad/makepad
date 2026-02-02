use makepad_widgets2::animator::Animate;
use makepad_widgets2::file_tree::FileTree;
use makepad_widgets2::*;
use std::path::Path;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*

    let TestDraw = #(TestDraw::register_widget(vm)) {
        width: 250
        height: 150
        draw_quad +: {
            pixel: fn(){
                let sdf = Sdf2d.viewport(self.pos*self.rect_size)
                sdf.circle(40 40 35)
                sdf.fill(mix(#0f0 #f00 self.pos.y))
                sdf.result
            }
        }
        draw_text.color: #0f0
    }

    // Item template for the PortalList
    let ListItem = RoundedView{
        width: Fill height: Fit
        margin: Inset{top: 2 bottom: 2 left: 5 right: 5}
        padding: Inset{top: 10 bottom: 10 left: 15 right: 15}
        draw_bg.color: #445
        draw_bg.radius: 5.0
        flow: Right align: HCenter spacing: 10

        View{
            width: Fill height: Fit flow: Down spacing: 4
            $title: Label{text: "Item Title" draw_text.color: #fff draw_text.text_style.font_size: 11}
            $subtitle: Label{text: "Item subtitle text" draw_text.color: #888 draw_text.text_style.font_size: 9}
        }
        $action_btn: ButtonFlatter{text: "View" draw_text.text_style.font_size: 9}
    }

    let ListHeader = View{
        width: Fill height: 40 padding: Inset{left: 10 right: 10} align: Align{y: 0.5}
        Label{text: "PortalList Demo" draw_text.color: #fff draw_text.text_style.font_size: 12}
    }

    let ListFooter = View{
        width: Fill height: 60 align: Center
        Label{text: "End of List" draw_text.color: #666}
    }

    // Custom NewsList widget that uses PortalList
    let NewsListTest = #(NewsListTest::register_widget(vm)) {
        width: Fill
        height: Fill
        $list: PortalList{
            width: Fill
            height: Fill
            flow: Down
            $Header: ListHeader{}
            $Item: ListItem{}
            $Footer: ListFooter{}
        }
    }

    // ===========================================
    // TAB CONTENT TEMPLATES BY WIDGET TYPE
    // ===========================================

    // Buttons tab - all button variants
    let TabButtons = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Overlay

        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "Button Variants" draw_text.color: #fff draw_text.text_style.font_size: 13}

            View{width: Fill height: Fit flow: Right spacing: 10 align: Align{y: 0.5}}
            $button: Button{text: "Standard"}
            $flat_button: ButtonFlat{text: "Flat"}
            $flatter_button: ButtonFlatter{text: "Flatter"}

            $icon_button: Button{
                text: "With Icon"
                icon_walk: Walk{width: 16 height: 16}
                draw_icon.color: #fff
                draw_icon.svg: crate_resource("self:../../widgets2/resources/icons/icon_file.svg")
            }

            Hr{}

            Label{text: "Icon Only" draw_text.color: #888 draw_text.text_style.font_size: 10}
            View{width: Fill height: Fit flow: Right spacing: 15}
            $test_icon: Icon{
                draw_icon.svg: crate_resource("self:../../widgets2/resources/icons/icon_file.svg")
                draw_icon.color: #0ff
                icon_walk: Walk{width: 32 height: 32}
            }
            Icon{
                draw_icon.svg: crate_resource("self:../../widgets2/resources/icons/icon_search.svg")
                draw_icon.color: #f80
                icon_walk: Walk{width: 32 height: 32}
            }

            Hr{}

            Label{text: "Tooltip Demo" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Click buttons to show tooltips, click elsewhere to hide" draw_text.color: #888 draw_text.text_style.font_size: 10}

            View{width: Fill height: Fit flow: Right spacing: 10}
            $tooltip_btn1: Button{text: "Show Tooltip 1"}
            $tooltip_btn2: Button{text: "Show Tooltip 2"}
            $tooltip_btn3: ButtonFlat{text: "Show Help Tip"}

            Hr{}

            Label{text: "Popup Notification Demo" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Click to show/hide notification popup" draw_text.color: #888 draw_text.text_style.font_size: 10}

            View{width: Fill height: Fit flow: Right spacing: 10}
            $show_popup_btn: Button{text: "Show Notification"}
            $hide_popup_btn: ButtonFlat{text: "Hide Notification"}
        }

        // Tooltip overlay
        $buttons_tooltip: Tooltip{}

        // Popup notification overlay
        $popup_notif: PopupNotification{
            align: Align{x: 1.0 y: 0.0}
            $content +: {
                margin: Inset{top: 10 right: 10}

                RoundedView{
                    width: 250
                    height: Fit
                    padding: 15
                    draw_bg +: {
                        color: uniform(#2a5)
                        radius: uniform(8.0)
                    }
                    flow: Down spacing: 8

                    Label{text: "Success!" draw_text.color: #fff draw_text.text_style.font_size: 12}
                    Label{text: "Your changes have been saved successfully." draw_text.color: #dfd draw_text.text_style.font_size: 10}
                }
            }
        }
    }

    // Toggles tab - checkboxes, toggles, radio buttons
    let TabToggles = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 10

            Label{text: "Checkboxes" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $checkbox: CheckBox{text: "Enable feature"}
            CheckBox{text: "Show notifications"}
            CheckBox{text: "Auto-save on exit"}

            Hr{}

            Label{text: "Toggles" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $toggle: Toggle{text: "Dark mode"}
            Toggle{text: "Compact view"}
            Toggle{text: "Developer mode"}

            Hr{}

            Label{text: "Radio Buttons" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $radio1: RadioButton{text: "Option A"}
            $radio2: RadioButton{text: "Option B"}
            $radio3: RadioButton{text: "Option C"}
        }
    }

    // Sliders tab - sliders and numeric inputs
    let TabSliders = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "Sliders" draw_text.color: #fff draw_text.text_style.font_size: 13}

            $slider: Slider{width: Fill text: "Volume" min: 0.0 max: 100.0 default: 50.0}
            Slider{width: Fill text: "Brightness" min: 0.0 max: 100.0 default: 75.0}
            Slider{width: Fill text: "Contrast" min: -50.0 max: 50.0 default: 0.0}
            Slider{width: Fill text: "Saturation" min: 0.0 max: 200.0 default: 100.0}

            Hr{}

            Label{text: "Fine Control" draw_text.color: #888 draw_text.text_style.font_size: 10}
            Slider{width: Fill text: "Font Size" min: 8.0 max: 24.0 default: 12.0}
            Slider{width: Fill text: "Line Height" min: 1.0 max: 3.0 default: 1.5}
        }
    }

    // Text tab - labels, headings, text inputs
    let TabText = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 10

            Label{text: "Headings" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $heading: H1{text: "Heading 1"}
            H2{text: "Heading 2"}
            H3{text: "Heading 3"}

            Hr{}

            Label{text: "Text Inputs" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Username:" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            $username: TextInput{width: Fill height: Fit empty_text: "Enter username"}
            Label{text: "Password:" draw_text.color: #aaa draw_text.text_style.font_size: 10}
            $password: TextInput{width: Fill height: Fit empty_text: "Enter password" is_password: true}

            Hr{}

            Label{text: "Links" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $link: LinkLabel{text: "Visit Makepad" url: "https://makepad.dev"}
        }
    }

    // Dropdowns tab - dropdown and selection widgets
    let TabDropdowns = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "Dropdown" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $dropdown: DropDown{labels: ["Option A" "Option B" "Option C" "Option D"]}

            Hr{}

            Label{text: "More Dropdowns" draw_text.color: #fff draw_text.text_style.font_size: 13}
            DropDown{labels: ["Small" "Medium" "Large" "Extra Large"]}
            DropDown{labels: ["Red" "Green" "Blue" "Yellow" "Purple"]}
        }
    }

    // HTML/Markdown tab
    let TabMarkup = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 15

            Label{text: "Markdown" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $markdown: Markdown{
                width: Fill height: Fit
                body: "# Heading\n\nThis is **bold** and *italic*.\n\n- List item 1\n- List item 2\n\n> Blockquote\n\n`inline code`"
            }

            Hr{}

            Label{text: "HTML" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $html: Html{
                width: Fill height: Fit
                body: "<h3>HTML Content</h3><p><b>Bold</b> and <i>italic</i> text.</p><ul><li>Item one</li><li>Item two</li></ul><p><a href='https://makepad.dev'>Link</a></p>"
            }
        }
    }

    // Expandable Panel tab
    let TabExpandable = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Down

        Label{text: "Expandable Panel Demo" draw_text.color: #fff draw_text.text_style.font_size: 13 padding: 15}
        Label{text: "Drag the panel up/down" draw_text.color: #888 draw_text.text_style.font_size: 10 padding: Inset{left: 15 bottom: 10}}

        $expandable: ExpandablePanel{
            width: Fill height: Fill
            initial_offset: 100.0

            // Background content (visible when panel is dragged down)
            SolidView{
                width: Fill height: Fill
                draw_bg.color: #224
                align: Center
                Label{text: "Background Content" draw_text.color: #88f draw_text.text_style.font_size: 16}
            }

            // The draggable panel
            $panel: RoundedView{
                width: Fill height: Fill
                draw_bg.color: #445
                draw_bg.radius: vec4(15.0 15.0 0.0 0.0)
                flow: Down padding: 20 spacing: 10

                // Drag handle indicator
                View{
                    width: Fill height: Fit align: Center padding: Inset{bottom: 10}
                    RoundedView{
                        width: 40 height: 4
                        draw_bg.color: #666
                        draw_bg.radius: 2.0
                    }
                }

                Label{text: "Draggable Panel" draw_text.color: #fff draw_text.text_style.font_size: 14}
                Label{text: "This panel can be dragged up and down. The initial_offset property controls the starting position." draw_text.color: #aaa draw_text.text_style.font_size: 10}

                Hr{}

                Label{text: "Panel Content" draw_text.color: #fff draw_text.text_style.font_size: 12}
                CheckBox{text: "Option 1"}
                CheckBox{text: "Option 2"}
                CheckBox{text: "Option 3"}

                View{height: Fill}

                $reset_btn: Button{text: "Reset Panel Position"}
            }
        }
    }

    // Fold Headers tab
    let TabFolds = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 10

            Label{text: "Fold Headers" draw_text.color: #fff draw_text.text_style.font_size: 13}

            FoldHeader{
                header: View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5}
                    padding: Inset{top: 5 bottom: 5} spacing: 8
                    FoldButton{}
                    Label{text: "Settings" draw_text.color: #fff draw_text.text_style.font_size: 11}
                }
                body: View{
                    width: Fill height: Fit flow: Down
                    padding: Inset{left: 23 top: 5 bottom: 10} spacing: 8
                    CheckBox{text: "Enable notifications"}
                    CheckBox{text: "Auto-save"}
                    Toggle{text: "Dark theme"}
                }
            }
            FoldHeader{
                header: View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5}
                    padding: Inset{top: 5 bottom: 5} spacing: 8
                    FoldButton{}
                    Label{text: "Recent Files" draw_text.color: #fff draw_text.text_style.font_size: 11}
                }
                body: View{
                    width: Fill height: Fit flow: Down
                    padding: Inset{left: 23 top: 5 bottom: 10} spacing: 5
                    Label{text: "document.txt" draw_text.color: #8af}
                    Label{text: "project.rs" draw_text.color: #8af}
                    Label{text: "config.toml" draw_text.color: #8af}
                }
            }
            FoldHeader{
                header: View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5}
                    padding: Inset{top: 5 bottom: 5} spacing: 8
                    FoldButton{}
                    Label{text: "Advanced" draw_text.color: #fff draw_text.text_style.font_size: 11}
                }
                body: View{
                    width: Fill height: Fit flow: Down
                    padding: Inset{left: 23 top: 5 bottom: 10} spacing: 8
                    Button{text: "Import..."}
                    Button{text: "Export..."}
                    Slider{width: Fill text: "Opacity" min: 0.0 max: 100.0 default: 75.0}
                }
            }
        }
    }

    // Lists tab - PortalList demo
    let TabLists = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        NewsListTest{}
    }

    // FileTree demo widget
    let FileTreeDemo = #(FileTreeDemo::register_widget(vm)){
        width: Fill
        height: Fill
        file_tree: FileTree{}
    }

    // FileTree tab - file tree demo
    let TabFileTree = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Down padding: 10 spacing: 10

        //Label{text: "FileTree Demo" draw_text.color: #fff draw_text.text_style.font_size: 13}
        //Label{text: "Displays file system hierarchy" draw_text.color: #888 draw_text.text_style.font_size: 10}
        View{
            optimize: ViewOptimize.DrawList
            FileTreeDemo{
                width: Fill height: Fill
            }
        }
    }

    // Media tab - images, spinners, custom draws
    let TabMedia = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "Images" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $test_image: Image{width: 180 height: 120 fit: ImageFit.Stretch}

            Hr{}

            Label{text: "Loading Spinner" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $spinner: LoadingSpinner{width: 40 height: 40}

            Hr{}

            Label{text: "Custom Shader" draw_text.color: #fff draw_text.text_style.font_size: 13}
            $test: TestDraw{}
        }
    }

    // Modal tab - modal dialog demos
    let TabModal = SolidView{
        width: Fill height: Fill
        draw_bg.color: #333
        flow: Overlay

        // Main content with buttons to trigger modals
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 15 spacing: 12

            Label{text: "Modal Dialogs" draw_text.color: #fff draw_text.text_style.font_size: 13}
            Label{text: "Click the buttons below to open different modal dialogs" draw_text.color: #888 draw_text.text_style.font_size: 10}

            Hr{}

            Label{text: "Basic Modal" draw_text.color: #fff draw_text.text_style.font_size: 11}
            $open_modal_btn: Button{text: "Open Modal"}

            Hr{}

            Label{text: "Confirmation Modal" draw_text.color: #fff draw_text.text_style.font_size: 11}
            $open_confirm_modal_btn: Button{text: "Open Confirmation Dialog"}

            Hr{}

            Label{text: "Non-dismissable Modal" draw_text.color: #fff draw_text.text_style.font_size: 11}
            Label{text: "This modal cannot be dismissed by clicking outside" draw_text.color: #888 draw_text.text_style.font_size: 9}
            $open_nodismiss_modal_btn: Button{text: "Open Non-dismissable Modal"}

            Hr{}

            $modal_status: Label{text: "Modal status: Closed" draw_text.color: #8f8 draw_text.text_style.font_size: 10}
        }

        // Basic Modal
        $test_modal: Modal{
            $content +: {
                width: 300
                height: Fit
                padding: 20
                spacing: 15
                align: Center

                RoundedView{
                    width: Fill height: Fit
                    draw_bg.color: #445
                    draw_bg.radius: 8.0
                    padding: 20 spacing: 12
                    flow: Down align: Center

                    Label{text: "Basic Modal" draw_text.color: #fff draw_text.text_style.font_size: 14}
                    Label{text: "This is a basic modal dialog. Click outside or press Escape to close." draw_text.color: #aaa draw_text.text_style.font_size: 10}

                    View{height: 10}

                    $close_modal_btn: Button{text: "Close Modal"}
                }
            }
        }

        // Confirmation Modal
        $confirm_modal: Modal{
            $content +: {
                width: 350
                height: Fit

                RoundedView{
                    width: Fill height: Fit
                    draw_bg.color: #445
                    draw_bg.radius: 8.0
                    padding: 25 spacing: 15
                    flow: Down

                    Label{text: "Confirm Action" draw_text.color: #fff draw_text.text_style.font_size: 14}
                    Label{text: "Are you sure you want to perform this action? This cannot be undone." draw_text.color: #aaa draw_text.text_style.font_size: 10}

                    View{height: 10}

                    View{
                        width: Fill height: Fit
                        flow: Right spacing: 10 align: Align{x: 1.0 y: 0.5}

                        $cancel_confirm_btn: ButtonFlat{text: "Cancel"}
                        $confirm_btn: Button{text: "Confirm"}
                    }
                }
            }
        }

        // Non-dismissable Modal
        $nodismiss_modal: Modal{
            can_dismiss: false
            $content +: {
                width: 320
                height: Fit

                RoundedView{
                    width: Fill height: Fit
                    draw_bg.color: #544
                    draw_bg.radius: 8.0
                    padding: 25 spacing: 15
                    flow: Down align: Center

                    Label{text: "Non-dismissable Modal" draw_text.color: #fff draw_text.text_style.font_size: 14}
                    Label{text: "This modal can only be closed by clicking the button below. Clicking outside or pressing Escape won't work." draw_text.color: #daa draw_text.text_style.font_size: 10}

                    View{height: 10}

                    $close_nodismiss_btn: Button{text: "I Understand, Close Modal"}
                }
            }
        }
    }

    let AppDock = Dock{
        width: Fill height: Fill

        // Dock structure - organized by widget type
        $root: DockSplitter{
            axis: SplitterAxis.Horizontal
            align: SplitterAlign.FromA(280.0)
            a: $left_tabs
            b: $split1
        }

        $split1: DockSplitter{
            axis: SplitterAxis.Vertical
            align: SplitterAlign.FromB(250.0)
            a: $center_tabs
            b: $bottom_tabs
        }

        // Left panel - input widgets
        $left_tabs: DockTabs{
            tabs: [$toggles_tab, $sliders_tab, $text_tab, $dropdowns_tab]
            selected: 0
            closable: false
        }

        // Center panel - content widgets
        $center_tabs: DockTabs{
            tabs: [$buttons_tab, $markup_tab, $media_tab, $modal_tab]
            selected: 0
            closable: true
        }

        // Bottom panel - containers/lists
        $bottom_tabs: DockTabs{
            tabs: [$filetree_tab $lists_tab, $folds_tab, $expandable_tab]
            selected: 0
            closable: true
        }

        // Individual tabs
        $buttons_tab: DockTab{
            name: "Buttons"
            template: $CloseableTab
            kind: $TabButtons
        }

        $toggles_tab: DockTab{
            name: "Toggles"
            template: $CloseableTab
            kind: $TabToggles
        }

        $sliders_tab: DockTab{
            name: "Sliders"
            template: $CloseableTab
            kind: $TabSliders
        }

        $text_tab: DockTab{
            name: "Text"
            template: $CloseableTab
            kind: $TabText
        }

        $dropdowns_tab: DockTab{
            name: "Selects"
            template: $CloseableTab
            kind: $TabDropdowns
        }

        $markup_tab: DockTab{
            name: "Markup"
            template: $CloseableTab
            kind: $TabMarkup
        }

        $folds_tab: DockTab{
            name: "Folds"
            template: $CloseableTab
            kind: $TabFolds
        }

        $lists_tab: DockTab{
            name: "Lists"
            template: $CloseableTab
            kind: $TabLists
        }

        $expandable_tab: DockTab{
            name: "Expandable"
            template: $CloseableTab
            kind: $TabExpandable
        }

        $media_tab: DockTab{
            name: "Media"
            template: $CloseableTab
            kind: $TabMedia
        }

        $filetree_tab: DockTab{
            name: "FileTree"
            template: $CloseableTab
            kind: $TabFileTree
        }

        $modal_tab: DockTab{
            name: "Modal"
            template: $CloseableTab
            kind: $TabModal
        }

        // Content templates by widget type
        $TabButtons: TabButtons{}
        $TabToggles: TabToggles{}
        $TabSliders: TabSliders{}
        $TabText: TabText{}
        $TabDropdowns: TabDropdowns{}
        $TabMarkup: TabMarkup{}
        $TabFolds: TabFolds{}
        $TabLists: TabLists{}
        $TabMedia: TabMedia{}
        $TabExpandable: TabExpandable{}
        $TabModal: TabModal{}
        $TabFileTree: TabFileTree{
        }
    }

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            $main_window: Window{
                pass.clear_color: vec4(0.3 0.3 0.3 1.0)
                window.inner_size: vec2(1000 700)
                $body +: {
                    padding: 4
                    $dock: AppDock{}
                }
            }
        }
    }
}

impl App {
    fn run(vm: &mut ScriptVm) -> Self {
        crate::makepad_widgets2::script_mod(vm);
        App::from_script_mod(vm, self::script_mod)
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // Load a test image into the Image widget
        let image_path =
            Path::new("tools/open_harmony/deveco/AppScope/resources/base/media/app_icon.png");
        if let Err(e) = self
            .ui
            .image(ids!($test_image))
            .load_image_file_by_path(cx, image_path)
        {
            log!("Failed to load image: {:?}", e);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(ids!($button)).clicked(actions) {
            log!("Button clicked!");
        }
        if self.ui.button(ids!($flat_button)).clicked(actions) {
            log!("Flat button clicked!");
        }
        if self.ui.button(ids!($flatter_button)).clicked(actions) {
            log!("Flatter button clicked!");
        }
        if self.ui.button(ids!($icon_button)).clicked(actions) {
            log!("Icon button clicked!");
        }

        // Tooltip demo - show tooltips on button click
        if self.ui.button(ids!($tooltip_btn1)).clicked(actions) {
            log!("Showing tooltip 1");
            self.ui.tooltip(ids!($buttons_tooltip)).show_with_options(
                cx,
                dvec2(350.0, 280.0),
                "This is the standard button. Click it to perform the primary action.",
            );
        }

        // Popup notification demo
        if self.ui.button(ids!($show_popup_btn)).clicked(actions) {
            log!("Showing popup notification");
            self.ui.popup_notification(ids!($popup_notif)).open(cx);
        }

        if let Some(value) = self.ui.check_box(ids!($checkbox)).changed(actions) {
            log!("Checkbox changed: {}", value);
        }
        if let Some(value) = self.ui.check_box(ids!($toggle)).changed(actions) {
            log!("Toggle changed: {}", value);
        }
        if let Some(index) = self
            .ui
            .radio_button_set(ids_list!($radio1, $radio2, $radio3))
            .selected(cx, actions)
        {
            log!("Radio button selected: {}", index);
        }

        // ExpandablePanel test
        if self.ui.button(ids!($reset_btn)).clicked(actions) {
            log!("Resetting expandable panel");
            self.ui.expandable_panel(ids!($expandable)).reset(cx);
        }

        if let Some(offset) = self
            .ui
            .expandable_panel(ids!($expandable))
            .scrolled_at(actions)
        {
            log!("ExpandablePanel scrolled to: {}", offset);
        }

        // Modal tests
        // Open basic modal
        if self.ui.button(ids!($open_modal_btn)).clicked(actions) {
            log!("Opening basic modal");
            self.ui.modal(ids!($test_modal)).open(cx);
            self.ui
                .label(ids!($modal_status))
                .set_text(cx, "Modal status: Basic Modal Open");
        }

        // Close basic modal
        if self.ui.button(ids!($close_modal_btn)).clicked(actions) {
            log!("Closing basic modal via button");
            self.ui.modal(ids!($test_modal)).close(cx);
            self.ui
                .label(ids!($modal_status))
                .set_text(cx, "Modal status: Closed via button");
        }

        // Check if basic modal was dismissed (clicked outside or pressed Escape)
        if self.ui.modal(ids!($test_modal)).dismissed(actions) {
            log!("Basic modal was dismissed");
            self.ui
                .label(ids!($modal_status))
                .set_text(cx, "Modal status: Dismissed (clicked outside or Escape)");
        }

        // Open confirmation modal
        if self
            .ui
            .button(ids!($open_confirm_modal_btn))
            .clicked(actions)
        {
            log!("Opening confirmation modal");
            self.ui.modal(ids!($confirm_modal)).open(cx);
            self.ui
                .label(ids!($modal_status))
                .set_text(cx, "Modal status: Confirmation Modal Open");
        }

        // Cancel confirmation
        if self.ui.button(ids!($cancel_confirm_btn)).clicked(actions) {
            log!("Confirmation cancelled");
            self.ui.modal(ids!($confirm_modal)).close(cx);
            self.ui
                .label(ids!($modal_status))
                .set_text(cx, "Modal status: Confirmation Cancelled");
        }

        // Confirm action
        if self.ui.button(ids!($confirm_btn)).clicked(actions) {
            log!("Action confirmed!");
            self.ui.modal(ids!($confirm_modal)).close(cx);
            self.ui
                .label(ids!($modal_status))
                .set_text(cx, "Modal status: Action Confirmed!");
        }

        // Check if confirmation modal was dismissed
        if self.ui.modal(ids!($confirm_modal)).dismissed(actions) {
            log!("Confirmation modal was dismissed");
            self.ui
                .label(ids!($modal_status))
                .set_text(cx, "Modal status: Confirmation dismissed");
        }

        // Open non-dismissable modal
        if self
            .ui
            .button(ids!($open_nodismiss_modal_btn))
            .clicked(actions)
        {
            log!("Opening non-dismissable modal");
            self.ui.modal(ids!($nodismiss_modal)).open(cx);
            self.ui
                .label(ids!($modal_status))
                .set_text(cx, "Modal status: Non-dismissable Modal Open");
        }

        // Close non-dismissable modal
        if self.ui.button(ids!($close_nodismiss_btn)).clicked(actions) {
            log!("Closing non-dismissable modal via button");
            self.ui.modal(ids!($nodismiss_modal)).close(cx);
            self.ui
                .label(ids!($modal_status))
                .set_text(cx, "Modal status: Non-dismissable closed via button");
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}

// TestDraw widget with draw_quad and draw_text shaders
#[derive(Script, ScriptHook, Widget)]
pub struct TestDraw {
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_quad: DrawQuad,
    #[live]
    draw_text: DrawText,
    #[rust]
    area: Area,
}

impl Widget for TestDraw {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);

        let rect = cx.turtle().rect();

        // Draw the quad with our custom shader
        self.draw_quad.draw_abs(
            cx,
            Rect {
                pos: rect.pos,
                size: dvec2(100.0, 100.0),
            },
);

        // Draw text below the quad
        self.draw_text
            .draw_abs(cx, rect.pos + dvec2(0.0, 110.0), "Hello Splash!");

        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {}
}

// NewsListTest widget demonstrating PortalList usage
#[derive(Script, ScriptHook, Widget)]
pub struct NewsListTest {
    #[deref]
    view: View,
}

impl Widget for NewsListTest {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.borrow_mut::<PortalList>() {
                // Set the item range (header + 50 items + footer)
                list.set_item_range(cx, 0, 52);

                while let Some(item_id) = list.next_visible_item(cx) {
                    // Determine which template to use based on item_id
                    let template = match item_id {
                        //0 => id!($Header),
                        //51 => id!($Footer),
                        _ => id!($Item),
                    };

                    let item = list.item(cx, item_id, template);

                    // Set content for Item template
                    if item_id > 0 && item_id < 51 {
                        let title = format!("Item #{}", item_id);
                        let subtitle = match item_id % 4 {
                            0 => "This is a longer description that shows how text wraps",
                            1 => "Short description",
                            2 => "Medium length subtitle text here",
                            _ => "Another item in the list",
                        };
                        item.label(ids!($title)).set_text(cx, &title);
                        item.label(ids!($subtitle)).set_text(cx, subtitle);
                    }

                    item.draw_all(cx, &mut Scope::empty());
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// FileTreeDemo widget demonstrating FileTree usage
#[derive(Script, ScriptHook, Widget)]
pub struct FileTreeDemo {
    #[redraw]
    #[live]
    file_tree: FileTree,
    #[walk]
    walk: Walk,
    #[rust]
    file_nodes: LiveIdMap<LiveId, FileNode>,
    #[rust]
    initialized: bool,
}

pub struct FileNode {
    pub name: String,
    pub child_edges: Option<Vec<FileEdge>>,
}

pub struct FileEdge {
    pub name: String,
    pub file_node_id: LiveId,
}

impl FileTreeDemo {
    fn draw_file_node(
        cx: &mut Cx2d,
        file_node_id: LiveId,
        file_tree: &mut FileTree,
        file_nodes: &LiveIdMap<LiveId, FileNode>,
    ) {
        if let Some(file_node) = file_nodes.get(&file_node_id) {
            match &file_node.child_edges {
                Some(child_edges) => {
                    if file_tree
                        .begin_folder(cx, file_node_id, &file_node.name)
                        .is_ok()
                    {
                        for child_edge in child_edges {
                            Self::draw_file_node(
                                cx,
                                child_edge.file_node_id,
                                file_tree,
                                file_nodes,
                            );
                        }
                        file_tree.end_folder();
                    }
                }
                None => {
                    file_tree.file(cx, file_node_id, &file_node.name);
                }
            }
        }
    }

    fn initialize_demo_tree(&mut self) {
        // Create a demo file tree structure
        let mut id_counter = 1u64;
        let mut next_id = || {
            let id = LiveId(id_counter);
            id_counter += 1;
            id
        };

        // Create some demo files and folders
        let file1_id = next_id();
        let file2_id = next_id();
        let file3_id = next_id();
        let subdir_id = next_id();
        let subfile1_id = next_id();
        let subfile2_id = next_id();
        let root_id = live_id!(root);

        // Files in subdirectory
        self.file_nodes.insert(
            subfile1_id,
            FileNode {
                name: "nested_file.rs".to_string(),
                child_edges: None,
            },
        );
        self.file_nodes.insert(
            subfile2_id,
            FileNode {
                name: "another_file.txt".to_string(),
                child_edges: None,
            },
);

        // Subdirectory
        self.file_nodes.insert(
            subdir_id,
            FileNode {
                name: "src".to_string(),
                child_edges: Some(vec![
                    FileEdge {
                        name: "nested_file.rs".to_string(),
                        file_node_id: subfile1_id,
                    },
                    FileEdge {
                        name: "another_file.txt".to_string(),
                        file_node_id: subfile2_id,
                    },
                ]),
            },
);

        // Root level files
        self.file_nodes.insert(
            file1_id,
            FileNode {
                name: "main.rs".to_string(),
                child_edges: None,
            },
        );
        self.file_nodes.insert(
            file2_id,
            FileNode {
                name: "Cargo.toml".to_string(),
                child_edges: None,
            },
        );
        self.file_nodes.insert(
            file3_id,
            FileNode {
                name: "README.md".to_string(),
                child_edges: None,
            },
);

        // Root folder
        self.file_nodes.insert(
            root_id,
            FileNode {
                name: "project".to_string(),
                child_edges: Some(vec![
                    FileEdge {
                        name: "src".to_string(),
                        file_node_id: subdir_id,
                    },
                    FileEdge {
                        name: "main.rs".to_string(),
                        file_node_id: file1_id,
                    },
                    FileEdge {
                        name: "Cargo.toml".to_string(),
                        file_node_id: file2_id,
                    },
                    FileEdge {
                        name: "README.md".to_string(),
                        file_node_id: file3_id,
                    },
                ]),
            },
        );
    }
}

impl Widget for FileTreeDemo {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.initialized {
            self.initialize_demo_tree();
            self.initialized = true;
        }
        while self.file_tree.draw_walk(cx, scope, walk).is_step() {
            self.file_tree
                .set_folder_is_open(cx, live_id!(root), true, Animate::No);
            Self::draw_file_node(cx, live_id!(root), &mut self.file_tree, &self.file_nodes);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.file_tree.handle_event(cx, event, scope);
    }
}
