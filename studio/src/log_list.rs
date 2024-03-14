
use {
    crate::{
        build_manager::{
            build_manager::*,
            build_protocol::*,
        },
        app::{AppAction, AppData},
        makepad_widgets::*,
        makepad_code_editor::text::{Position},
    },
    std::{
        env,
    },
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::base::*;
    import makepad_widgets::theme_desktop_dark::*;
    
    Icon = <View> {
        show_bg: true,
        width: 10,
        height: 10
    }
    
    LogIcon = <PageFlip> {
        active_page: log
        lazy_init: true,
        width: Fit,
        height: Fit,
        margin: {top: 1, left: 5, right: 5}
        wait = <Icon> {
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
        log = <Icon> {
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
        error = <Icon> {
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
        warning = <Icon> {
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
        panic = <Icon> {
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
    
    LogItem = <RectView> {
        height: Fit,
        width: Fill
        padding: {top: 6, bottom: 6}
        
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
    }
    
    LogList = {{LogList}}{
        height: Fill,
        width: Fill
        list = <PortalList> {
            grab_key_focus: true
            auto_tail: true
            drag_scrolling: false
            height: Fill,
            width: Fill
            flow: Down
            Location = <LogItem> {
                icon = <LogIcon> {},
                binary = <Label> {draw_text: {color: #5}, width: Fit, margin: {right: 4}, padding: 0, draw_text: {wrap: Word}}
                location = <LinkLabel> {margin: 0, text: ""}
                body = <Label> {width: Fill, margin: {left: 5}, padding: 0, draw_text: {wrap: Word}}
            }
            Bare = <LogItem> {
                icon = <LogIcon> {},
                binary = <Label> {draw_text: {color: #5}, width: Fit, margin: {right: 4}, padding: 0, draw_text: {wrap: Word}}
                body = <Label> {width: Fill, margin: 0, padding: 0, draw_text: {wrap: Word}}
            }
            Empty = <LogItem> {
                cursor: Default
                height: 24,
                width: Fill
            }
        }
    }
}

#[derive(Clone, Debug, DefaultNone)]
pub enum LogListAction {
    JumpTo(JumpTo),
    None
}

#[derive(Clone, Debug)]
pub struct JumpTo{
    pub file_name:String, 
    pub start:Position
}

#[derive(Live, LiveHook, Widget)]
pub struct LogList{
    #[deref] view:View
}

impl LogList{
    fn draw_log(&mut self, cx: &mut Cx2d, list:&mut PortalList, build_manager:&mut BuildManager){
        list.set_item_range(cx, 0, build_manager.log.len());
                                
        while let Some(item_id) = list.next_visible_item(cx) {
            let is_even = item_id & 1 == 0;
            fn map_level_to_icon(level: LogLevel) -> LiveId {
                match level {
                    LogLevel::Warning => live_id!(warning),
                    LogLevel::Error => live_id!(error),
                    LogLevel::Log => live_id!(log),
                    LogLevel::Wait => live_id!(wait),
                    LogLevel::Panic => live_id!(panic),
                }
            }
            if let Some((build_id, log_item)) = build_manager.log.get(item_id as usize) {
                let binary = if build_manager.active.builds.len()>1 {
                    if let Some(build) = build_manager.active.builds.get(&build_id) {
                        &build.log_index
                    }
                    else {""}
                }else {""};
                                                                        
                match log_item {
                    LogItem::Bare(msg) => {
                        let item = list.item(cx, item_id, live_id!(Bare)).unwrap().as_view();
                        item.apply_over(cx, live!{
                            binary = {text: (&binary)}
                            icon = {active_page: (map_level_to_icon(msg.level))},
                            body = {text: (&msg.line)}
                            draw_bg: {is_even: (if is_even {1.0} else {0.0})}
                        });
                        item.draw_all(cx, &mut Scope::empty());
                    }
                    LogItem::Location(msg) => {
                        let item = list.item(cx, item_id, live_id!(Location)).unwrap().as_view();
                        item.apply_over(cx, live!{
                            binary = {text: (&binary)}
                            icon = {active_page: (map_level_to_icon(msg.level))},
                            body = {text: (&msg.message)}
                            location = {text: (format!("{}: {}:{}", msg.file_name, msg.start.line_index + 1, msg.start.byte_index + 1))}
                            draw_bg: {is_even: (if is_even {1.0} else {0.0})}
                        });
                        item.draw_all(cx, &mut Scope::empty());
                    }
                    _ => {}
                }
                continue
            }
            let item = list.item(cx, item_id, live_id!(Empty)).unwrap().as_view();
            item.apply_over(cx, live!{draw_bg: {is_even: (if is_even {1.0} else {0.0})}});
            item.draw_all(cx, &mut Scope::empty());
        }
    }
}

impl Widget for LogList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        while let Some(step) = self.view.draw_walk(cx, scope, walk).step(){
            if let Some(mut list) = step.as_portal_list().borrow_mut(){
                self.draw_log(cx, &mut *list, &mut scope.data.get_mut::<AppData>().build_manager)
            }
        }
        DrawStep::done()
    }
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope){
        let log_list = self.view.portal_list(id!(list));
        self.view.handle_event(cx, event, scope);
        let data = scope.data.get::<AppData>();
        if let Event::Actions(actions) = event{
            for (item_id, item) in log_list.items_with_actions(&actions) {
                if item.link_label(id!(location)).pressed(&actions) {
                    if let Some((_build_id, log_item)) = data.build_manager.log.get(item_id as usize) {
                        match log_item {
                            LogItem::Location(msg) => {
                                cx.action(AppAction::JumpTo(JumpTo{
                                    file_name:msg.file_name.clone(), 
                                    start:Position{
                                        line_index: msg.start.line_index,
                                        byte_index: msg.start.byte_index,
                                    },
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
