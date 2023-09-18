use {
    crate::{
        makepad_platform::*,
        build_manager::{
            build_manager::*,
            build_protocol::*,
            build_client::BuildClient
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
    
    RunButton = <CheckBox> {
        width: Fill,
        height: 25,
        margin: {left: 1},
        label_walk: {margin: {top: 7}}
        draw_check: {
            uniform size: 4.0;
            instance open: 0.0
            uniform length: 3.0
            uniform width: 1.0

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                match self.check_type {
                    CheckType::Check => {
                        let left = 2;
                        let sz = self.size;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        
                        // PAUSE
                        sdf.box(sz * 0.25, sz * 1.75, sz * 0.9, sz * 3.0, 1.0); // rounding = 3rd value
                        sdf.box(sz * 1.75, sz * 1.75, sz * 0.9, sz * 3.0, 1.0); // rounding = 3rd value

                        sdf.fill(mix(#fff0, mix(#A, #F, self.hover), self.selected));

                        // PLAY
                        sdf.rotate(self.open * 0.5 * PI + 0.5 * PI, c.x, c.y);
                        sdf.move_to(c.x - sz, c.y + sz);
                        sdf.line_to(c.x, c.y - sz);
                        sdf.line_to(c.x + sz, c.y + sz);
                        sdf.close_path();
                        sdf.fill(mix(mix(#44, #8, self.hover), #fff0, self.selected));

                    }
                }
                return sdf.result
            }
        }
        draw_text: {text_style: <THEME_FONT_LABEL> {}}
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
            check = <RunButton> { margin: {left: 21} }
        }
        Binary = <BuildItem> {
            padding: {top: 0, bottom: 0}
            flow: Right
            fold = <FoldButton> {
                animator: {open = {default: no}}, height: 25, width: 15 margin: {left: 5}
                draw_bg: {
                    uniform size: 4.0;
                    instance open: 0.0
                    uniform length: 3.0
                    uniform width: 1.0
                    
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                        let left = 2;
                        let sz = self.size;
                        let c = vec2(left + sz, self.rect_size.y * 0.5);
                        
                        // PLUS
                        sdf.box(0, sz * 3.0, sz * 2.5, sz * 0.5, 1.0); // rounding = 3rd value
                        // vertical
                        sdf.fill_keep(mix(#8F, #FF, self.hover));
                        sdf.box(sz, sz * 2.0, sz * 0.5, sz * 2.5, 1.0); // rounding = 3rd value

                        sdf.fill_keep(mix(mix(#8F, #FF, self.hover), #FFF0, self.open))

                        return sdf.result
                    }
                }
            }
            //label = <Label> {width: Fill, margin: {left: 20, top: 7}, padding: 0, draw_text: {wrap: Ellipsis}}
            check = <RunButton> {}
        }
        Empty = <BuildItem> {
            cursor: Default
            height: 24,
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
        for binary in &self.binaries {
            let is_even = counter & 1 == 0;
            
            let item_id = LiveId::from_str(&binary.name);
            let item = list.item(cx, item_id, live_id!(Binary)).unwrap().as_view();
            item.apply_over(cx, live!{
                check = {text:(&binary.name)}
                draw_bg: {is_even: (if is_even {1.0} else {0.0})}
            });
            item.check_box(id!(check)).set_selected(cx, self.active.any_binary_active(&binary.name));
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
                    item.check_box(id!(check)).set_selected(cx, self.active.item_id_active(item_id));
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
    
    pub fn handle_run_list(&mut self, cx: &mut Cx, run_list: &FlatListRef, item_id: LiveId, item: WidgetRef, actions: &WidgetActions)->Vec<RunListAction>{
        // ok lets see if someone clicked our
        let mut out = Vec::new();
        for binary in &mut self.binaries {
            let binary_name = binary.name.clone();
            let id = LiveId::from_str(&binary.name);
            if item_id == id{
                if let Some(v) = item.fold_button(id!(fold)).animating(actions) {
                    binary.open = v;
                    item.redraw(cx);
                }
                if let Some(change) = item.check_box(id!(check)).changed(actions) {
                    run_list.redraw(cx);
                    for i in 0..if change{1}else{BuildTarget::len()} {
                        let id = LiveId::from_str(&binary.name).bytes_append(&i.to_be_bytes());
                        Self::toggle_active_build(self.studio_http.clone(), &mut self.active, &self.clients[0],cx, id, &binary_name, i, change, &mut out);
                        self.log.clear();
                    }
                };
            }
            else{
                for i in 0..BuildTarget::len() {
                    let id = LiveId::from_str(&binary.name).bytes_append(&i.to_be_bytes());
                    if item_id == id{
                        if let Some(change) = item.check_box(id!(check)).changed(actions) {
                            run_list.redraw(cx);
                            Self::toggle_active_build(self.studio_http.clone(), &mut self.active, &self.clients[0], cx, item_id, &binary_name, i, change, &mut out);
                            self.log.clear();
                        }
                    }
                }
            }
        }
        out
    }
    
    pub fn target_id_to_target(tgt:u64)->BuildTarget{
        match tgt {
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
        }
    }
    
    pub fn toggle_active_build(studio_http:String, active:&mut ActiveBuilds, client:&BuildClient, cx: &mut Cx, item_id: LiveId, binary: &str, tgt: u64, run: bool, actions:&mut Vec<RunListAction>) {
        let target = Self::target_id_to_target(tgt);
        let process = BuildProcess {
            binary: binary.to_string(),
            target
        };
        let build_id = process.as_id().into();
        if run {
            let run_view_id = LiveId::unique();
            if active.builds.get(&build_id).is_none() {
                active.builds.insert(build_id, ActiveBuild {
                    item_id,
                    process: process.clone(),
                    run_view_id,
                    cmd_id: Some(client.send_cmd(BuildCmd::Run(process.clone(), studio_http))),
                    texture: Texture::new(cx)
                });
            }
            if process.target.runs_in_studio(){
                // create the runview tab
                actions.push(RunListAction::Create(run_view_id, process.binary.clone()))
            }
        }
        else {
            if let Some(build) = active.builds.remove(&build_id) {
                if let Some(cmd_id) = build.cmd_id {
                    client.send_cmd_with_id(cmd_id, BuildCmd::Stop);
                }
                if process.target.runs_in_studio(){
                     actions.push(RunListAction::Destroy(build.run_view_id))
                }
            }
        }
    }
    
}
