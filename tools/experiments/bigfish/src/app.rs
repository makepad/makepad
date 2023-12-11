use crate::fish_doc::*;
use makepad_audio_widgets::*;
use makepad_widgets::*;

live_design! {
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import crate::fish_patch_editor::*;
    import crate::fish_block_editor::*;
    import crate::homescreen::BigFishHomeScreen;
    import crate::fish_theme::*;
    import crate::fish_connection_widget::*;

    App = {{App}} {

        ui: <Window> {
            show_bg: true
            width: Fill,
            height: Fill,
            padding: 0.,
            margin: 0.,
            draw_bg: {
                fn pixel(self) -> vec4 {
                    let Pos = floor(self.pos*self.rect_size *0.10);
                    let PatternMask = mod(Pos.x + mod(Pos.y, 2.0), 2.0);
                    return mix( vec4(0,0.15*self.pos.y,0.1,1), vec4(.05, 0.03, .23*self.pos.y, 1.0), PatternMask);
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
                mainscreentabs = Tabs{tabs:[homescreentab, patcheditortab, debugcontroltab], selected:1}
                homescreentab = Tab{
                    name: "Home"
                    kind: homescreen
                }
                patcheditortab = Tab{
                    name: "Patch Editor"
                    kind: patcheditorscreen
                }
                debugcontroltab = Tab{
                    name: "Debug"
                    kind: debugcontrolscreen
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

                homescreen = <BigFishHomeScreen>{}

                patcheditorscreen = <FishPatchEditor>{}

                debugcontrolscreen = <View>{
                    flow: Down
                    <View>{flow: Right
                        loadbutton = <Button>{text:"Load"}
                        savebutton = <Button>{text:"Save"}
                    }
                    <FishBlockEditor>{
                        flow: Overlay;
                    }
                    <FishConnectionWidget>{
                        flow: Overlay;
                        line_start = vec2(10,10);
                        line_end = vec2(1000,300);
                    }
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
                        flow: Down,
                        draw_bg: {fn pixel(self) -> vec4 { return #111}}
                        align:{x:0.5,y:0.5}
                        <Label>{text:"Fancy Beeping"
                        margin: 30
                            draw_text: {
                                color: #f,
                                text_style: <H2_TEXT_BOLD> {}
                            }
                        }
                        <Image> {
                            source: dep("crate://self/resources/colourfish.png"),
                            width: (431*0.25 ), height: (287*0.25), margin: { top: 0.0, right: 0.0, bottom: 0.0, left: 10.0  }

                            }
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

#[derive(Live, LiveHook)] 
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    counter: usize,
    #[rust(FishDoc::create_test_doc())]
    document: FishDoc,
}

impl LiveRegister for App {
    fn live_register(cx: &mut Cx) {
        crate::makepad_audio_widgets::live_design(cx);
        //crate::makepad_widgets::live_design(cx);
        crate::fish_patch_editor::live_design(cx);
        crate::block_header_button::live_design(cx);
        crate::fish_block_editor::live_design(cx);
        crate::fish_theme::live_design(cx);
        crate::homescreen::live_design(cx);
        crate::fish_connection_widget::live_design(cx);
    }
    // after_new_from_doc
}

impl MatchEvent for App {
    fn handle_startup(&mut self, _cx:&mut Cx){
        self.document = FishDoc::create_test_doc();
    }
    
    fn handle_actions(&mut self, cx:&mut Cx, actions:&Actions){
        if self.ui.button(id!(button1)).clicked(&actions) {
            self.counter += 1;
            let label = self.ui.label(id!(label1));
            label.set_text_and_redraw(cx, &format!("Counter: {}", self.counter));
        }
            
        if self.ui.button(id!(savebutton)).clicked(&actions) {
            let _ = self.document.save(&"testout.fish").is_ok();
        }
            
        if self.ui.button(id!(loadbutton)).clicked(&actions) {
            let _ = self.document.load(&"testout.fish").is_ok();
        }
    }
}

impl AppMain for App {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event) { 
        self.match_event(cx, event); 
        self.ui.handle_event(cx, event, &mut Scope::with_data(&mut self.document));
    }
}
