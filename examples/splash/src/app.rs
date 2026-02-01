use makepad_widgets2::*;
use std::path::Path;

app_main!(App);

script_mod!{
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
    
    let AppDock = Dock{
        width: Fill height: Fill
                                
        // Dock structure - organized by widget type
        $root: DockSplitter{
            axis: SplitterAxis.Horizontal
            align: SplitterAlign.FromA(280.0)
            a: @$left_tabs
            b: @$split1
        }
                                
        $split1: DockSplitter{
            axis: SplitterAxis.Vertical
            align: SplitterAlign.FromB(250.0)
            a: @$center_tabs
            b: @$bottom_tabs
        }
                                
        // Left panel - input widgets
        $left_tabs: DockTabs{
            tabs: [@$toggles_tab, @$sliders_tab, @$text_tab, @$dropdowns_tab]
            selected: 0
            closable: false
        }
                                
        // Center panel - content widgets
        $center_tabs: DockTabs{
            tabs: [@$buttons_tab, @$markup_tab, @$media_tab]
            selected: 0
            closable: true
        }
                                
        // Bottom panel - containers/lists
        $bottom_tabs: DockTabs{
            tabs: [@$lists_tab, @$folds_tab]
            selected: 0
            closable: true
        }
                                
        // Individual tabs
        $buttons_tab: DockTab{
            name: "Buttons"
            template: @$CloseableTab
            kind: @$TabButtons
        }
                                
        $toggles_tab: DockTab{
            name: "Toggles"
            template: @$CloseableTab
            kind: @$TabToggles
        }
                                
        $sliders_tab: DockTab{
            name: "Sliders"
            template: @$CloseableTab
            kind: @$TabSliders
        }
                                
        $text_tab: DockTab{
            name: "Text"
            template: @$CloseableTab
            kind: @$TabText
        }
                                
        $dropdowns_tab: DockTab{
            name: "Selects"
            template: @$CloseableTab
            kind: @$TabDropdowns
        }
                                
        $markup_tab: DockTab{
            name: "Markup"
            template: @$CloseableTab
            kind: @$TabMarkup
        }
                                
        $folds_tab: DockTab{
            name: "Folds"
            template: @$CloseableTab
            kind: @$TabFolds
        }
                                
        $lists_tab: DockTab{
            name: "Lists"
            template: @$CloseableTab
            kind: @$TabLists
        }
                                
        $media_tab: DockTab{
            name: "Media"
            template: @$CloseableTab
            kind: @$TabMedia
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
    #[live] ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        // Load a test image into the Image widget
        let image_path = Path::new("tools/open_harmony/deveco/AppScope/resources/base/media/app_icon.png");
        if let Err(e) = self.ui.image(ids!($test_image)).load_image_file_by_path(cx, image_path) {
            log!("Failed to load image: {:?}", e);
        }
    }
    
    fn handle_actions(&mut self, _cx: &mut Cx, actions: &Actions) {
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
        if let Some(value) = self.ui.check_box(ids!($checkbox)).changed(actions) {
            log!("Checkbox changed: {}", value);
        }
        if let Some(value) = self.ui.check_box(ids!($toggle)).changed(actions) {
            log!("Toggle changed: {}", value);
        }
        if let Some(index) = self.ui.radio_button_set(ids_list!($radio1, $radio2, $radio3)).selected(_cx, actions) {
            log!("Radio button selected: {}", index);
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
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[redraw] #[live] draw_quad: DrawQuad,
    #[live] draw_text: DrawText,
    #[rust] area: Area,
}

impl Widget for TestDraw {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        
        let rect = cx.turtle().rect();
        
        // Draw the quad with our custom shader
        self.draw_quad.draw_abs(cx, Rect {
            pos: rect.pos,
            size: dvec2(100.0, 100.0)
        });
        
        // Draw text below the quad
        self.draw_text.draw_abs(cx, rect.pos + dvec2(0.0, 110.0), "Hello Splash!");
        
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
    
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope) {
    }
}

// NewsListTest widget demonstrating PortalList usage
#[derive(Script, ScriptHook, Widget)]
pub struct NewsListTest {
    #[deref] view: View,
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
