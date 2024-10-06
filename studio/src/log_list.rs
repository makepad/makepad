
use {
    crate::{
        build_manager::{
            build_manager::*,
            build_protocol::*,
        },
        makepad_platform::studio::JumpToFile,
        app::{AppAction, AppData},
        makepad_widgets::*,
        makepad_code_editor::code_view::*,
    },
    std::{
        env,
    },
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_code_editor::code_view::CodeView;
    
    Icon = <View> {
        width: 10, height: 10
        margin:{top:2, right: 10},
        show_bg: true,
    }
    
    LogItem = <View> {
        height: Fit, width: Fill
        padding: <THEME_MSPACE_2> {} // TODO: Fix. Changing this value to i.e. '0.' causes Makepad Studio to freeze when switching to the log tab.
        spacing: (THEME_SPACE_2)
        align: { x: 0.0, y: 0.0 }
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
        animator: {
            ignore_missing: true,
            hover = {
                default: off
                off = {
                    from: {all: Forward {duration: 0.1}}
                    apply: {
                        draw_bg: {hover: 0.0}
                    }
                }
                on = {
                    cursor: Hand
                    from: {all: Snap}
                    apply: {
                        draw_bg: {hover: 1.0}
                    },
                }
            }
            
            select = {
                default: off
                off = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 0.0}
                    }
                }
                on = {
                    from: {all: Snap}
                    apply: {
                        draw_bg: {selected: 1.0}
                    }
                }  
            }
        }
        flow = <TextFlow>{
            width: Fill,
            height: Fit
            width: Fill,
            height: Fit
            
            code_view = <CodeView>{
                editor:{
                    margin:{left:25}
                }
            }
            
            fold_button = <FoldButton>{
                animator:{
                    open={default:off}
                }
            }
            
            wait_icon = <Icon> {
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                        sdf.circle(5., 5., 4.)
                        sdf.fill(THEME_COLOR_TEXT_META)
                        sdf.move_to(3., 5.)
                        sdf.line_to(3., 5.)
                        sdf.move_to(5., 5.)
                        sdf.line_to(5., 5.)
                        sdf.move_to(7., 5.)
                        sdf.line_to(7., 5.)
                        sdf.stroke(#0, 0.8)
                        return sdf.result
                    }
                }
            },
            log_icon = <Icon> {
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                        sdf.circle(5., 5., 4.);
                        sdf.fill(THEME_COLOR_TEXT_META);
                        let sz = 1.;
                        sdf.move_to(5., 5.);
                        sdf.line_to(5., 5.);
                        sdf.stroke(#a, 0.8);
                        return sdf.result
                    }
                }
            }
            error_icon = <Icon> {
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                        sdf.circle(5., 5., 4.5);
                        sdf.fill(THEME_COLOR_ERROR);
                        let sz = 1.5;
                        sdf.move_to(5. - sz, 5. - sz);
                        sdf.line_to(5. + sz, 5. + sz);
                        sdf.move_to(5. - sz, 5. + sz);
                        sdf.line_to(5. + sz, 5. - sz);
                        sdf.stroke(#0, 0.8)
                        return sdf.result
                    }
                }
            },
            warning_icon = <Icon> {
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                        sdf.move_to(5., 1.);
                        sdf.line_to(9.25, 9.);
                        sdf.line_to(0.75, 9.);
                        sdf.close_path();
                        sdf.fill(THEME_COLOR_WARNING);
                        //  sdf.stroke(#be, 0.5);
                        sdf.move_to(5., 3.5);
                        sdf.line_to(5., 5.25);
                        sdf.stroke(#0, 1.0);
                        sdf.move_to(5., 7.25);
                        sdf.line_to(5., 7.5);
                        sdf.stroke(#0, 1.0);
                        return sdf.result
                    }
                }
            }
            panic_icon = <Icon> {
                draw_bg: {
                    fn pixel(self) -> vec4 {
                        let sdf = Sdf2d::viewport(self.pos * self.rect_size)
                        sdf.move_to(5., 1.);
                        sdf.line_to(9., 9.);
                        sdf.line_to(1., 9.);
                        sdf.close_path();
                        sdf.fill(THEME_COLOR_PANIC);
                        let sz = 1.;
                        sdf.move_to(5. - sz, 6.25 - sz);
                        sdf.line_to(5. + sz, 6.25 + sz);
                        sdf.move_to(5. - sz, 6.25 + sz);
                        sdf.line_to(5. + sz, 6.25 - sz);
                        sdf.stroke(#0, 0.8);
                        return sdf.result
                    }
                }
            }
        }
    }
    
    LogList = {{LogList}}{
        height: Fill, width: Fill,
        list = <PortalList> {
            capture_overload: false,
            grab_key_focus: false
            auto_tail: true
            drag_scrolling: false
            height: Fill, width: Fill,
            flow: Down
            LogItem = <LogItem> {
            }
            Empty = <LogItem> {
                cursor: Default
                width: Fill
                height: 25,
                body = <P> {  margin: 0, text: "" }
            }
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum LogListAction {
    JumpTo(JumpToFile),
    None
}

#[derive(Live, LiveHook, Widget)]
pub struct LogList{
    #[deref] view:View
}

#[derive(Clone, Debug, PartialEq)]
pub struct JumpToFileLink{item_id:usize}

impl LogList{
    fn draw_log(&mut self, cx: &mut Cx2d, list:&mut PortalList, build_manager:&mut BuildManager){
        list.set_item_range(cx, 0, build_manager.log.len());
        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even = item_id & 1 == 0;
            fn map_level_to_icon(level: LogLevel) -> LiveId {
                match level {
                    LogLevel::Warning => live_id!(warning_icon),
                    LogLevel::Error => live_id!(error_icon),
                    LogLevel::Log => live_id!(log_icon),
                    LogLevel::Wait => live_id!(wait_icon),
                    LogLevel::Panic => live_id!(panic_icon),
                }
            }
            let mut location = String::new();
            if let Some((build_id, log_item)) = build_manager.log.get(item_id as usize) {
                let _binary = if build_manager.active.builds.len()>1 {
                    if let Some(build) = build_manager.active.builds.get(&build_id) {
                        &build.log_index
                    }
                    else {""}
                }else {""};
                let mut item = list.item(cx, item_id, live_id!(LogItem)).as_view();
                item.apply_over(cx, live!{
                    draw_bg: {is_even: (if is_even {1.0} else {0.0})}
                });
                while let Some(step) = item.draw(cx, &mut Scope::empty()).step(){
                    if let Some(mut tf) = step.as_text_flow().borrow_mut(){
                        match log_item {
                            LogItem::Bare(msg) => {
                                tf.draw_item_counted(cx, map_level_to_icon(msg.level));
                                tf.draw_text(cx,&msg.line);
                            }
                            LogItem::Location(msg) => {
                                tf.draw_item_counted(cx, map_level_to_icon(msg.level));
                                let fold_button = if msg.explanation.is_some(){
                                    tf.draw_item_counted_ref(cx, live_id!(fold_button)).as_fold_button()
                                }
                                else{
                                    Default::default()
                                };
                                fmt_over!(location, "{}: {}:{}", msg.file_name, msg.start.line_index + 1, msg.start.byte_index + 1);
                                tf.draw_link(cx, live_id!(link), JumpToFileLink{item_id}, &location);
                                
                                tf.draw_text(cx, &msg.message);
                                if let Some(explanation) = &msg.explanation{
                                    let open = fold_button.open_float();
                                    if open > 0.0{
                                        cx.turtle_new_line();
                                        let code = tf.item_counted(cx, live_id!(code_view));
                                        code.set_text(explanation);
                                        code.as_code_view().borrow_mut().unwrap().editor.height_scale = open;
                                        code.draw_all_unscoped(cx);
                                    }
                                };
                            }
                            _ => {}
                        }
                    }
                }
                continue
            }
            let item = list.item(cx, item_id, live_id!(Empty)).as_view();
            item.apply_over(cx, live!{draw_bg: {is_even: (if is_even {1.0} else {0.0})}});
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for LogList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step(){
            if let Some(mut list) = step.as_portal_list().borrow_mut(){
                self.draw_log(cx, &mut *list, &mut scope.data.get_mut::<AppData>().unwrap().build_manager)
            }
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        let log_list = self.view.portal_list(id!(list));
        self.view.handle_event(cx, event, scope);
        let data = scope.data.get::<AppData>().unwrap();
        if let Event::Actions(actions) = event{
            
            if log_list.any_items_with_actions(&actions) {
                // alright lets figure out if someone clicked a link
                // alright so how do we now filter which link was clicked
                for jtf in actions.filter_actions_data::<JumpToFileLink>(){
                    // ok we have a JumpToFile link
                    if let Some((_build_id, log_item)) = data.build_manager.log.get(jtf.item_id) {
                        match log_item {
                            LogItem::Location(msg) => {
                                cx.action(AppAction::JumpTo(JumpToFile{
                                    file_name: msg.file_name.clone(), 
                                    line: msg.start.line_index as u32,
                                    column: msg.start.byte_index as u32
                                }));
                            }
                            _ => ()
                        }
                    }
                }
            }
        }
    }
}

impl LogListRef{
    pub fn reset_scroll(&self, cx:&mut Cx){
        if let Some(inner) = self.borrow_mut() {
            let log_list = inner.view.portal_list(id!(list));
            log_list.set_first_id_and_scroll(0,0.0);
            log_list.redraw(cx);
        }
    }
}
