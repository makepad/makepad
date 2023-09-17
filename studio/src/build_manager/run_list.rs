use {
    crate::{
        makepad_platform::*,
        build_manager::{
            build_manager::*,
            build_protocol::*,
        },
        makepad_widgets::*,
    },
    std::{
        env,
    },
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    BuildItem = <RectView> {
        height: Fit,
        width: Fill
        padding: {top: 0, bottom: 0}
        
        draw_bg: {
            instance is_even: 0.0
            instance selected: 0.0
            instance hover: 0.0
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_BG_EDITOR,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    ),
                    THEME_COLOR_BG_SELECTED,
                    self.selected
                );
            }
        }
    }
    
    RunList = <FlatList> {
        grab_key_focus: true
        drag_scrolling: false
        height: Fill,
        width: Fill
        flow: Down
        Target = <BuildItem> {
            padding: {top: 0, bottom: 0}
            //label = <Label> {width: Fill, margin:{left:35}, padding:0, draw_text: {wrap: Word}}
            check = <CheckBox> {
                width: Fill,
                height: 25,
                margin: {left: 21},
                label_walk: {margin: {top: 7}}
                draw_check: {check_type: Radio}
                draw_text: {text_style: <THEME_FONT_LABEL> {}}
            }
        }
        Binary = <BuildItem> {
            padding: {top: 0, bottom: 0}
            flow: Right
            fold = <FoldButton> {animator: {open = {default: no}}, height: 25, width: 15 margin: {left: 5}}
            //label = <Label> {width: Fill, margin: {left: 20, top: 7}, padding: 0, draw_text: {wrap: Ellipsis}}
            check = <CheckBox> {
                width: Fill,
                height: 25,
                margin: {left: 1},
                label_walk: {margin: {top: 7}}
                draw_check: {check_type: Radio}
                draw_text: {text_style: <THEME_FONT_LABEL> {}}
            }
        }
        Empty = <BuildItem> {
            cursor: Default
            height: 20,
            width: Fill
        }
    }
}

pub enum RunListAction{
    Create(LiveId, String),
    Destroy(LiveId),
    None
}

impl BuildManager {
    
    pub fn draw_run_list(&self, cx: &mut Cx2d, list: &mut FlatList){
        let mut counter = 0u32;
        for (index, binary) in self.binaries.iter().enumerate() {
            let is_even = counter & 1 == 0;
            
            let item_id = LiveId::from_str(&binary.name);
            let item = list.item(cx, item_id, live_id!(Binary)).unwrap().as_view();
            item.apply_over(cx, live!{
                check = {text:(&binary.name)}
                draw_bg: {is_even: (if is_even {1.0} else {0.0})}
            });
            item.draw_widget_all(cx);
            counter += 1;
            
            if binary.open>0.001 {
                for i in 0..BuildTarget::len() {
                    let is_even = counter & 1 == 0;
                    let item_id = LiveId::from_str(&binary.name).bytes_append(&i.to_be_bytes());
                    let item = list.item(cx, item_id, live_id!(Target)).unwrap().as_view();
                    let height = 25.0 * binary.open;
                    item.apply_over(cx, live!{
                        height: (height)
                        draw_bg: {is_even: (if is_even {1.0} else {0.0})}
                        check = {text: (BuildTarget::name(i))}
                    });
                    item.draw_widget_all(cx);
                    counter += 1;
                }
            }
        }
        while list.space_left(cx)>0.0 {
            let is_even = counter & 1 == 0;
            let item_id = LiveId::from_str("empty").bytes_append(&counter.to_be_bytes());
            let item = list.item(cx, item_id, live_id!(Empty)).unwrap().as_view();
            let height = list.space_left(cx).min(20.0);
            item.apply_over(cx, live!{
                height: (height)
                draw_bg: {is_even: (if is_even {1.0} else {0.0})}
            });
            item.draw_widget_all(cx);
            counter += 1;
        }
    }
    
    pub fn handle_run_list(&mut self, cx: &mut Cx, item_id: LiveId, item: WidgetRef, actions: &WidgetActions)->RunListAction{
        // ok lets see if someone clicked our
        for binary in &mut self.binaries {
            let binary_name = binary.name.clone();
            let id = LiveId::from_str(&binary.name);
            if item_id == id{
                if let Some(v) = item.fold_button(id!(fold)).animating(actions) {
                    binary.open = v;
                    item.redraw(cx);
                }
                if let Some(change) = item.check_box(id!(check)).changed(actions) {
                    return self.toggle_active_build(cx, binary_name, 0, change)
                };
            }
            else{
                for i in 0..BuildTarget::len() {
                    let id = LiveId::from_str(&binary.name).bytes_append(&i.to_be_bytes());
                    if item_id == id{
                        if let Some(change) = item.check_box(id!(check)).changed(actions) {
                            return self.toggle_active_build(cx, binary_name, i, change)
                        }
                    }
                }
            }
        }
        RunListAction::None
    }
    
    
    pub fn toggle_active_build(&mut self, cx: &mut Cx, binary: String, tgt: u64, run: bool)->RunListAction {
        let target = match tgt {
            BuildTarget::RELEASE => BuildTarget::Release,
            BuildTarget::DEBUG => BuildTarget::Debug,
            BuildTarget::RELEASE_STUDIO => BuildTarget::ReleaseStudio,
            BuildTarget::DEBUG_STUDIO => BuildTarget::DebugStudio,
            BuildTarget::PROFILER => BuildTarget::Profiler,
            BuildTarget::IOS_SIM => BuildTarget::IosSim {
                org: "makepad".to_string(),
                app: "example".to_string()
            },
            BuildTarget::IOS_DEVICE => BuildTarget::IosDevice {
                org: "makepad".to_string(),
                app: "example".to_string()
            },
            BuildTarget::ANDROID => BuildTarget::Android,
            BuildTarget::WEBASSEMBLY => BuildTarget::WebAssembly,
            _ => panic!()
        };
        // alright so what buildtarget do we use
        let process = BuildProcess {
            binary,
            target
        };
        let build_id = process.as_id().into();
        self.log.clear();
        if run {
            let run_view_id = LiveId::unique();
            if self.active.builds.get(&build_id).is_none() {
                self.active.builds.insert(build_id, ActiveBuild {
                    process: process.clone(),
                    run_view_id,
                    cmd_id: Some(self.clients[0].send_cmd(BuildCmd::Run(process.clone()))),
                    texture: Texture::new(cx)
                });
            }
            // create the runview tab
            return RunListAction::Create(run_view_id, process.binary.clone())
        }
        else {
            if let Some(build) = self.active.builds.remove(&build_id) {
                if let Some(cmd_id) = build.cmd_id {
                    self.clients[0].send_cmd_with_id(cmd_id, BuildCmd::Stop);
                }
                return RunListAction::Destroy(build.run_view_id)
            }
        }
        RunListAction::None
    }
    
}
