
use {
    crate::{
        makepad_draw::*,
        makepad_widgets::*,
    }
};

live_design!{
    import makepad_draw::shader::std::*;
    import makepad_widgets::theme::*;
    
    DrawVideo = {{DrawVideo}} {
        texture image: texture2d
        fn pixel(self) -> vec4 {
            return #f00
        }
    }
    
    VideoView = {{VideoView}} {
        walk: {
            margin: { top: 3, right: 10, bottom: 3, left: 10 },
            width: Fill,
            height: Fill
        }
    }
}

// TODO support a shared 'inputs' struct on drawshaders
#[derive(Live, LiveHook)]#[repr(C)]
struct DrawVideo {
    draw_super: DrawQuad,
    width: f32,
    height: f32
}

#[derive(Live, LiveHook)]
#[live_design_fn(widget_factory!(VideoView))]
pub struct VideoView {
    walk: Walk,
    draw_video: DrawVideo
}

impl VideoView {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        self.draw_video.draw_walk(cx, walk);
    }
}

impl Widget for VideoView {
    fn redraw(&mut self, cx: &mut Cx) {
        self.draw_video.redraw(cx);
    }
    
    fn widget_uid(&self) -> WidgetUid {return WidgetUid(self as *const _ as u64)}
    fn get_walk(&self) -> Walk {self.walk}
    
    fn draw_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }
}

#[derive(Clone, PartialEq, WidgetRef)]
pub struct VideoViewRef(WidgetRef);

impl VideoViewRef{
}

