use makepad_widgets::*;
use crate::fish_patch_editor::*;
use crate::homescreen::*;
use crate::fish_doc::*;
use crate::fish_block_editor::*;

live_design!{
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import crate::fish_patch_editor::*;
    import crate::fish_block_editor::*;
    import crate::homescreen::BigFishHomeScreen;
    
    App = {{App}} {

        ui: <Window> {
            show_bg: true
            width: Fill,
            height: Fill,
            padding: 0.,
            margin: 0.,
            draw_bg: {
                fn pixel(self) -> vec4 {
                    return mix(vec4(1.,1.,0.,1.),#0, abs(mod(self.pos.y*10.0,100.) - sin(self.pos.x*10.1))*100 )
                }
            }
           
            caption_bar = {      
                visible: true,            
                caption_label = {label ={text: "TiNRS BigFish" }}
            };

            window_menu = {
                main = Main {items: [app, file]}                
                app = Sub {name: "TiNRS BigFish", items: [about, line, settings, line, quit]}
                file = Sub {name: "File", items: [newfile]}
                about = Item {name: "About BigFish", enabled: true}
                settings = Item {name: "Settings", enabled: true}
                quit = Item {name: "Quit BigFish", key: KeyQ}
                newfile = Item {name: "New", key: KeyN}
                line = Line,
            }
            
            body = <View>{
                flow: Down;
                <Dock> {
                height: Fill,
                width: Fill
                margin: 0,
                padding: 0,
                  
                root = Splitter {
                    axis: Vertical,                    
                    align: FromB(320.0),
                    a: mainscreentabs,
                    b: split1
                }
                        
                split1 = Splitter {
                    axis: Horizontal,
                    align: Weighted(0.333),
                    a: left_view_tabs,
                    b: split2
                }
                split2 = Splitter {
                    axis: Horizontal,
                    align: Weighted(0.5),
                    a: middle_view_tabs,
                    b: right_view_tabs
                }
                mainscreentabs = Tabs{tabs:[mainscreentab]}
                mainscreentab = Tab{
                    name: "External"
                    kind: mainscreen
                }
                middle_view_tabs = Tabs{tabs:[middle_view_tab]}

                middle_view_tab = Tab{
                    name: "Center"
                    kind:middle_view
                }
                left_view_tabs = Tabs{tabs:[left_view_tab]}
                left_view_tab = Tab{
                    name: "Left"
                    kind:left_view
                }
                right_view_tabs = Tabs{tabs:[right_view_tab]}
                right_view_tab = Tab{
                    name: "Right"
                    kind:right_view
                }
                mainscreen = <View>{
                    flow: Down,
                
                    align: {
                        x: 0.5,
                        y: 0.5
                    }
                    loadbutton = <Button>{text:"Load"}
                    savebutton = <Button>{text:"Save"}
                   
                    home = <BigFishHomeScreen> {}
                    patch = <FishPatchEditor> {}
                   
                   
                }
                middle_view = <View>{
                    align: {
                        x: 0.5,
                        y: 0.5
                    }
                    show_bg: true,
                    draw_bg: {fn pixel(self) -> vec4 { return #000}}

                <View>{width:320, height: 320
                        show_bg: true,
                        draw_bg: {fn pixel(self) -> vec4 { return #111}}
                    }
                }

                left_view = <View>{
                    align: {
                        x: 0.5,
                        y: 0.5
                    }
                    show_bg: true,
                    draw_bg: {fn pixel(self) -> vec4 { return #000}}

                <View>{width:320, height: 320
                        show_bg: true,
                        draw_bg: {fn pixel(self) -> vec4 { return #111}}
                    }
                }
                right_view = <View>{
                    align: {
                        x: 0.5,
                        y: 0.5
                    }
                    show_bg: true,
                    draw_bg: {fn pixel(self) -> vec4 { return #000}}
                <View>{width:320, height: 320
                        show_bg: true,
                        draw_bg: {fn pixel(self) -> vec4 { return #111}}
                    }
                }
            }
        }  
        }
    }
}

app_main!(App);

#[derive(Live)]
pub struct App {
    #[live] ui: WidgetRef,
    #[rust] counter: usize,
    #[rust(FishDoc::create_test_doc())] document: FishDoc
}

impl LiveHook for App {
    fn before_live_design(cx: &mut Cx) {
        crate::makepad_widgets::live_design(cx);
        crate::fish_patch_editor::live_design(cx);
        crate::fish_block_editor::live_design(cx);
        crate::homescreen::live_design(cx);
    }
    // after_new_from_doc 
}

impl App{
    async fn _do_network_request(_cx:CxRef, _ui:WidgetRef, _url:&str)->String{
        "".to_string()
    }
}

impl AppMain for App{
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Event::Draw(event) = event {

          
            //let dt = profile_start();
            let cx = &mut Cx2d::new(cx, event);
            while let Some(next) = self.ui.draw_widget(cx).hook_widget() {
                                    
                    if let Some(mut patch_editor) = next.as_fish_patch_editor().borrow_mut() {
                    // lets fetch a session
                    //  let current_id = dock.drawing_item_id().unwrap();
                    patch_editor.draw(cx, &mut self.document.patches[0]);
                    
                }
            }
            //profile_end!(dt);
            return 
        }
        let actions = self.ui.handle_widget_event(cx, event);
  
        if self.ui.button(id!(button1)).clicked(&actions) {
            self.counter += 1;
            let label = self.ui.label(id!(label1));
            label.set_text_and_redraw(cx,&format!("Counter: {}", self.counter));
        }

        if self.ui.button(id!(savebutton)).clicked(&actions) {
            let _ = self.document.save(&"testout.fish").is_ok();
        }

        if self.ui.button(id!(loadbutton)).clicked(&actions) {
           let _ = self.document.load(&"testout.fish").is_ok();
        }

        if let Event::Construct = event {
            self.document = FishDoc::create_test_doc();
        }
        
    }
}