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
        flow: Right align: Align{y: 0.5} spacing: 10
        
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
    
    // Left panel with controls, inputs, and fold headers
    let LeftPanel = RoundedView{
        draw_bg.color: #335 width: Fill height: Fill
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 10 spacing: 10
            Label{text: "Left Panel" draw_text.color: #fff}
            $checkbox: CheckBox{text: "Enable feature"}
            $toggle: Toggle{text: "Dark mode"}
            $radio1: RadioButton{text: "Option A"}
            $radio2: RadioButton{text: "Option B"}
            $radio3: RadioButton{text: "Option C"}
            $link: LinkLabel{text: "Visit Makepad" url: "https://makepad.dev"}
            
            Label{text: "Username:" draw_text.color: #aaa}
            $username: TextInput{width: Fill height: Fit empty_text: "Enter your username"}
            Label{text: "Password:" draw_text.color: #aaa}
            $password: TextInput{width: Fill height: Fit empty_text: "Enter your password" is_password: true}
            
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
                    CheckBox{text: "Dark theme"}
                    Toggle{text: "Sounds"}
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
                    Label{text: "readme.md" draw_text.color: #8af}
                    Label{text: "main.rs" draw_text.color: #8af}
                }
            }
            FoldHeader{
                header: View{
                    width: Fill height: Fit flow: Right align: Align{y: 0.5}
                    padding: Inset{top: 5 bottom: 5} spacing: 8
                    FoldButton{}
                    Label{text: "More Options" draw_text.color: #fff draw_text.text_style.font_size: 11}
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
    
    // Top right panel with dropdown, custom draw, spinner, and Markdown
    let TopPanel = RoundedView{
        draw_bg.color: #353 width: Fill height: Fill
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 10 spacing: 10
            Label{text: "Top Panel" draw_text.color: #fff}
            $dropdown: DropDown{labels: ["Option A" "Option B" "Option C" "Option D"]}
            $test: TestDraw{}
            $view: RoundedView{width: 250 height: 100 draw_bg.color: #494}
            $spinner: LoadingSpinner{width: 50 height: 50}
            $label: Label{text: "Hello from Label!" draw_text.color: #ff0}
            
            // Markdown widget test
            $markdown: Markdown{
                width: Fill height: Fit
                body: "# Markdown Test\n\nThis is a **bold** and *italic* text.\n\n## Features\n\n- List item 1\n- List item 2\n- List item 3\n\n> This is a blockquote\n\nSome `inline code` here.\n\n```\nCode block example\n```"
            }
            
            // Html widget test
            $html: Html{
                width: Fill height: Fit
                body: "<h2>HTML Test</h2><p>This is a <b>bold</b> and <i>italic</i> paragraph.</p><ul><li>Item one</li><li>Item two</li><li>Item three</li></ul><blockquote>A quote block</blockquote><p>Here is some <code>inline code</code> and a <a href='https://makepad.dev'>link</a>.</p>"
            }
        }
    }
    
    // Bottom left panel with buttons, sliders, images
    let BottomLeftPanel = RoundedView{
        draw_bg.color: #533 width: Fill height: Fill
        ScrollYView{
            width: Fill height: Fill flow: Down padding: 10 spacing: 10
            Label{text: "Bottom Panel" draw_text.color: #fff}
            $heading: H1{text: "This is a Heading"}
            $button: Button{text: "Click Me!"}
            $flat_button: ButtonFlat{text: "Flat Button"}
            $flatter_button: ButtonFlatter{text: "Flatter Button"}
            $slider: Slider{width: 200 text: "Volume" min: 0.0 max: 100.0 default: 50.0}
            $icon_button: Button{
                text: "Icon Button"
                icon_walk: Walk{width: 20 height: 20}
                draw_icon.color: #fff
                draw_icon.svg: mod.res.crate("self:../../widgets2/resources/icons/icon_file.svg")
            }
            $test_image: Image{width: 200 height: 150 fit: ImageFit.Stretch}
            $test_icon: Icon{
                draw_icon.svg: mod.res.crate("self:../../widgets2/resources/icons/icon_file.svg")
                draw_icon.color: #0ff
                icon_walk: Walk{width: 50 height: 50}
            }
        }
    }
    
    // Bottom right panel with PortalList demo
    let BottomRightPanel = RoundedView{
        draw_bg.color: #353 width: Fill height: Fill
        $news_list: NewsListTest{}
    }
    
    mod.res.load_all() do #(App::script_component(vm)){
        ui: Window{
            pass.clear_color: vec4(0.3 0.3 0.3 1.0)
            window.inner_size: vec2(800 600)
            $body +: {
                $splitter: Splitter{
                    axis: SplitterAxis.Horizontal
                    align: SplitterAlign.Weighted(0.3)
                    a: LeftPanel{}
                    b: Splitter{
                        axis: SplitterAxis.Vertical
                        align: SplitterAlign.Weighted(0.6)
                        a: TopPanel{}
                        b: Splitter{
                            axis: SplitterAxis.Horizontal
                            align: SplitterAlign.Weighted(0.5)
                            a: BottomLeftPanel{}
                            b: BottomRightPanel{}
                        }
                    }
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
                        0 => id!($Header),
                        51 => id!($Footer),
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
