
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
    
    ProfilerEventChart = {{ProfilerEventChart}}{
        height: Fill,
        width: Fill
        bg: {
            fn pixel(self)->vec4{
                return #f00
            }
        }
    }
    
    Profiler = {{Profiler}}{
        height: Fill,
        width: Fill
        <ProfilerEventChart>{
        }
    }
}

#[derive(Live, LiveHook, LiveRegisterWidget, WidgetRef, WidgetSet, WidgetRedraw)]
struct ProfilerEventChart{
    #[walk] walk:Walk,
    #[redraw] #[live] bg: DrawQuad,
    #[live] item: DrawQuad,
}

impl Widget for ProfilerEventChart {
    fn draw_walk(&mut self, cx: &mut Cx2d, _scope:&mut Scope, walk:Walk)->DrawStep{
        self.bg.begin(cx, walk, Layout::default());
        self.bg.end(cx);
        DrawStep::done()
    }
        
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope){
    }
}

#[derive(Live, LiveHook, LiveRegisterWidget, WidgetRef, WidgetSet, WidgetRedraw)]
struct Profiler{
    #[deref] view:View,
}

impl Widget for Profiler {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk:Walk)->DrawStep{
        self.view.draw_walk_all(cx, scope, walk)
    }
    
    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event, _scope: &mut Scope){
    }
}
