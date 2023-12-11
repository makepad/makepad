
use {
    crate::{
        build_manager::{
            build_manager::*,
        },
        app::{AppData},
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
    
    Profiler = {{Profiler}}{
        height: Fill,
        width: Fill
    }
}

#[derive(Live, LiveHook, LiveRegisterWidget, WidgetRef, WidgetSet)]
struct Profiler{
    #[deref] view:View
}
 
impl Profiler{
    fn draw_profiler(&mut self, _cx: &mut Cx2d, _list:&mut PortalList, _build_manager:&mut BuildManager){
    }
}

impl Widget for Profiler {
    fn redraw(&mut self, cx: &mut Cx) {
        self.view.redraw(cx);
    }
    
    fn walk(&mut self, cx:&mut Cx) -> Walk {
        self.view.walk(cx)
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut WidgetScope, walk:Walk)->WidgetDraw{
        while let Some(next) = self.view.draw_walk(cx, scope, walk).hook_widget(){
            if let Some(mut list) = next.as_portal_list().borrow_mut(){
                self.draw_profiler(cx, &mut *list, &mut scope.data.get_mut::<AppData>().build_manager)
            }
        }
        WidgetDraw::done()
    }
    
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut WidgetScope){
    }
}
