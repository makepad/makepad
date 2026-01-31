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
    
    let app = #(App::script_component(vm)){
        ui: Window{
            pass.clear_color: vec4(0.3 0.3 0.3 1.0)
            window.inner_size: vec2(800 600)
            $body +: {
                flow: Down
                padding: 20
                spacing: 10
                $test: TestDraw{}
                $view: RoundedView{
                    width: 250 
                    height: 100
                    draw_bg.color: #494
                }
                $spinner: LoadingSpinner{
                    width: 50
                    height: 50
                }
                $label: Label{
                    text: "Hello from Label!"
                    draw_text.color: #ff0
                }
                $heading: H1{
                    text: "This is a Heading"
                }
                $button: Button{
                    text: "Click Me!"
                }
                $flat_button: ButtonFlat{
                    text: "Flat Button"
                }
                $flatter_button: ButtonFlatter{
                    text: "Flatter Button"
                }
                $slider: Slider{
                    width: 200
                    text: "Volume"
                    min: 0.0
                    max: 100.0
                    default: 50.0
                }
                $icon_button: Button{
                    text: "Icon Button"
                    icon_walk: Walk{width: 20, height: 20}
                    draw_icon.color: #fff
                    draw_icon.svg: mod.res.crate("self:../../widgets2/resources/icons/icon_file.svg")
                }
                $test_image: Image{
                    width: 200
                    height: 150
                    fit: ImageFit.Stretch
                }
                $test_icon: Icon{
                    draw_icon.svg: mod.res.crate("self:../../widgets2/resources/icons/icon_file.svg")
                    draw_icon.color: #0ff
                    icon_walk: Walk{width: 50, height: 50}
                }
                
            }
        }
    }
    mod.res.load_all()
    app
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
