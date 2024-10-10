use {
    crate::{
        build_manager::{
            build_manager::*,
            build_protocol::*,
            build_client::BuildClient
        },
        app::{AppData, AppAction}, 
        makepad_widgets::*,
    },
    std::env,
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
     
    BuildItem = <View> {
        width: Fill, height: Fit,
        show_bg: true,
        
        draw_bg: {
            instance is_even: 0.0
            instance selected: 0.0
            instance hover: 0.0
            fn pixel(self) -> vec4 {
                return mix(
                    mix(
                        THEME_COLOR_BG_EVEN,
                        THEME_COLOR_BG_ODD,
                        self.is_even
                    ),
                    THEME_COLOR_CTRL_SELECTED,
                    self.selected
                );
            }
        }
    }
    
    RunButton = <CheckBox> {
        width: Fill,
        
        draw_check: {
            uniform size: 3.5;
            instance open: 0.0
            uniform length: 3.0
            uniform width: 1.0

            fn pixel(self) -> vec4 {
                let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                match self.check_type {
                    CheckType::Check => {
                        let left = 3;
                        let sz = self.size;
                        let c = vec2(left + sz, self.rect_size.y * 0.5 - 1);
                        
                        // PAUSE
                        sdf.box(
                            sz * 0.5,
                            sz * 2.25,
                            sz * 0.9,
                            sz * 3.0,
                            1.0
                        );

                        sdf.box(
                            sz * 1.75,
                            sz * 2.25,
                            sz * 0.9,
                            sz * 3.0,
                            1.0
                        );

                        sdf.fill(mix(THEME_COLOR_U_HIDDEN, mix(THEME_COLOR_W, THEME_COLOR_TEXT_HOVER, self.hover), self.selected));

                        // PLAY
                        sdf.rotate(self.open * 0.5 * PI + 0.5 * PI, c.x, c.y);
                        sdf.move_to(c.x - sz, c.y + sz);
                        sdf.line_to(c.x, c.y - sz);
                        sdf.line_to(c.x + sz, c.y + sz);
                        sdf.close_path();
                        sdf.fill(mix(mix(THEME_COLOR_U_4, THEME_COLOR_TEXT_HOVER, self.hover), THEME_COLOR_U_HIDDEN, self.selected));

                    }
                }
                return sdf.result
            }
        }
    }
    
    
    RunList = {{RunList}}{
        width: Fill, height: Fill,

        list = <FlatList> {
            height: Fill, width: Fill,
            flow: Down,
            grab_key_focus: true,
            drag_scrolling: false,

            Target = <BuildItem> {
                padding: 0,
                check = <RunButton> { margin: {left: 23} }

                // <Image> {
                //     width: 20., height: 20.
                //     svg_file: dep("crate://self/resources/icons/Icon_Search.svg"),
                // }
                // platform = <Button> {
                //     width: 10., height: Fit,
                //     margin: { right: 5.0}
                //     align: { x: 0.5, y: 0.5 },
                //     padding: 0.0
                //     draw_icon: {
                //         svg_file: dep("crate://self/resources/icons/Icon_Search.svg"),
                //         fn get_color(self) -> vec4 { return #FFF3 }
                //     }
                //     icon_walk: {
                //         width: 10., height: Fit
                //     }
                //     draw_bg: {
                //         fn pixel(self) -> vec4 {
                //             let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                //             return sdf.result
                //         }
                //     }
                //     text: ""
                // }
                // mode = <Button> {
                //     width: 10., height: Fit,
                //     margin: { right: 15.0}
                //     align: { x: 0.5, y: 0.5 },
                //     padding: 0.0
                //     draw_icon: {
                //         svg_file: dep("crate://self/resources/icons/Icon_Search.svg"),
                //         fn get_color(self) -> vec4 { return #FFF3 }
                //     }
                //     icon_walk: {
                //         width: 10., height: Fit
                //     }
                //     draw_bg: {
                //         fn pixel(self) -> vec4 {
                //             let sdf = Sdf2d::viewport(self.pos * self.rect_size);
                //             return sdf.result
                //         }
                //     }
                //     text: ""
                // }
            }

            Binary = <BuildItem> {
                flow: Right

                fold = <FoldButton> {
                    height: 25, width: 15,
                    margin: { left: (THEME_SPACE_2) }
                    animator: { open = { default: off } },
                    draw_bg: {
                        uniform size: 3.75;
                        instance open: 0.0
                        
                        fn pixel(self) -> vec4 {
                            let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                            let left = 2;
                            let sz = self.size;
                            let c = vec2(left + sz, self.rect_size.y * 0.5);
                            
                            // PLUS
                            sdf.box(0.5, sz * 3.0, sz * 2.5, sz * 0.7, 1.0); // rounding = 3rd value
                            // vertical
                            sdf.fill_keep(mix((#6), #8, self.hover));
                            sdf.box(sz * 1.0, sz * 2.125, sz * 0.7, sz * 2.5, 1.0); // rounding = 3rd value
    
                            sdf.fill_keep(mix(mix((#6), #8, self.hover), #fff0, self.open))
    
                            return sdf.result
                        }
                    }
                }
                // label = <Label> {width: Fill, margin: {left: 20, top: 7}, padding: 0, draw_text: {wrap: Ellipsis}}
                check = <RunButton> {}
            }

            Empty = <BuildItem> {
                height: Fit, width: Fill,
                cursor: Default
            }
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum RunListAction{
    Create(LiveId, String),
    Destroy(LiveId),
    None
}

#[derive(Live, LiveHook, Widget)]
struct RunList{
    #[deref] view:View
}

impl RunList{
    fn draw_run_list(&mut self, cx: &mut Cx2d, list:&mut FlatList, build_manager:&mut BuildManager){
        let mut counter = 0u32;
        for binary in &build_manager.binaries { 
            let is_even = counter & 1 == 0;
                            
            let item_id = LiveId::from_str(&binary.name);
            let item = list.item(cx, item_id, live_id!(Binary)).unwrap().as_view();
            item.apply_over(cx, live!{
                check = {text:(&binary.name)}
                draw_bg: {is_even: (if is_even {1.0} else {0.0})}
            });
            item.check_box(id!(check)).set_selected(cx, build_manager.active.any_binary_active(&binary.name));
            item.draw_all(cx, &mut Scope::empty());
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
                        check = {text: (BuildTarget::from_id(i).name())}
                    });
                    item.check_box(id!(check)).set_selected(cx, build_manager.active.item_id_active(item_id));
                    item.draw_all(cx, &mut Scope::empty());
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
            item.draw_all(cx, &mut Scope::empty());
            counter += 1;
        }
            
    }
}

impl WidgetMatchEvent for RunList{
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions, scope: &mut Scope){
        let build_manager = &mut scope.data.get_mut::<AppData>().unwrap().build_manager;
        let run_list = self.view.flat_list(id!(list));
        for (item_id, item) in run_list.items_with_actions(&actions) {
            for binary in &mut build_manager.binaries {
                let binary_name = binary.name.clone();
                let id = LiveId::from_str(&binary.name);
                if item_id == id{
                    if let Some(v) = item.fold_button(id!(fold)).animating(&actions) {
                        binary.open = v;
                        item.redraw(cx);
                    }
                    if let Some(change) = item.check_box(id!(check)).changed(&actions) {
                        run_list.redraw(cx);
                        for i in 0..if change{1}else{BuildTarget::len()} {
                            if change{
                                BuildManager::start_active_build(cx, build_manager.studio_http.clone(), &mut build_manager.active, &build_manager.clients[0], &binary_name, i);
                            }
                            else{
                                BuildManager::stop_active_build(cx, &mut build_manager.active, &build_manager.clients[0], &binary_name, i);
                            }
                            cx.action(AppAction::ClearLog);
                        }
                    };
                }
                else{
                    for i in 0..BuildTarget::len() {
                        let id = LiveId::from_str(&binary.name).bytes_append(&i.to_be_bytes());
                        if item_id == id{
                            if let Some(change) = item.check_box(id!(check)).changed(&actions) {
                                run_list.redraw(cx);
                                if change{
                                    BuildManager::start_active_build(cx, build_manager.studio_http.clone(), &mut build_manager.active, &build_manager.clients[0], &binary_name, i);
                                }
                                else{
                                    BuildManager::stop_active_build(cx, &mut build_manager.active, &build_manager.clients[0], &binary_name, i);
                                }
                                cx.action(AppAction::ClearLog);
                            }
                        }
                    }
                }
            }
        }
    }
}

impl Widget for RunList {

    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step(){
            if let Some(mut list) = item.as_flat_list().borrow_mut(){
                self.draw_run_list(cx, &mut *list, &mut scope.data.get_mut::<AppData>().unwrap().build_manager)
            }
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        self.widget_match_event(cx, event, scope);
        self.view.handle_event(cx, event, scope);
    }
}

impl BuildManager {

    pub fn run_app(&mut self, cx:&mut Cx, binary_name:&str){
        Self::start_active_build(cx, self.studio_http.clone(), &mut self.active, &self.clients[0], &binary_name, 0);
    }
    
    pub fn start_active_build(cx:&mut Cx, studio_http:String, active:&mut ActiveBuilds, client:&BuildClient,  binary: &str, tgt: u64) {
        let target = BuildTarget::from_id(tgt);
        let process = BuildProcess {
            binary: binary.to_string(),
            target
        };
        let item_id = process.as_id();
        client.send_cmd_with_id(item_id, BuildCmd::Run(process.clone(),studio_http));
        //let run_view_id = LiveId::unique();
        if active.builds.get(&item_id).is_none() {
            let index = active.builds.len();
            active.builds.insert(item_id, ActiveBuild {
                log_index: format!("[{}]", index),
                process: process.clone(),
                app_area: Default::default(),
                swapchain: Default::default(),
                last_swapchain_with_completed_draws: Default::default(),
                aux_chan_host_endpoint: None,
            });
        }
        if process.target.runs_in_studio(){
            // create the runview tab
            cx.action(RunListAction::Create(item_id, process.binary.clone()))
        }
    }
    
    pub fn stop_active_build(cx:&mut Cx, active:&mut ActiveBuilds, client:&BuildClient, binary: &str, tgt: u64) {
        let target = BuildTarget::from_id(tgt);
        let process = BuildProcess {
            binary: binary.to_string(),
            target
        };
        let build_id = process.as_id().into();
       if let Some(_) = active.builds.remove(&build_id) {
            client.send_cmd_with_id(build_id, BuildCmd::Stop);
            if process.target.runs_in_studio(){
                cx.action(RunListAction::Destroy(build_id))
            }
        }
    }
    
}