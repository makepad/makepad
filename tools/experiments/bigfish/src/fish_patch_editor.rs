use crate::{makepad_draw::*, makepad_widgets::*, fish_patch::FishPatch};

live_design!
{
    import makepad_widgets::theme_desktop_dark::*;
    import makepad_widgets::base::*;
    
    FishPatchEditor = {{FishPatchEditor}} {
        width: Fill,
        height: Fill,
        scroll_bars: <ScrollBars> {}      
      
        draw_bg: {
            fn pixel(self) -> vec4 {
                let Pos = floor(self.pos*self.rect_size *0.10);
                let PatternMask = mod(Pos.x + mod(Pos.y, 2.0), 2.0);
                return mix( vec4(0,0.15*self.pos.y,0.1,1), vec4(.05, 0.03, .23*self.pos.y, 1.0), PatternMask);
            }
        }
    }
}

#[derive(Live)]
pub struct FishPatchEditor{
    #[animator] animator: Animator,
    #[walk] walk: Walk,
    #[live] draw_ls: DrawLine,
    #[rust] area: Area,
    #[rust] draw_state: DrawStateWrap<Walk>,
    #[live] scroll_bars: ScrollBars,
    #[live] draw_bg: DrawColor,
    #[rust] unscrolled_rect:Rect,
}

impl Widget for FishPatchEditor {
    fn handle_widget_event_with(
        &mut self,
        cx: &mut Cx,
        event: &Event,
        dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem),
    ) {
        let uid = self.widget_uid();
        self.animator_handle_event(cx, event);
        self.scroll_bars.handle_event_with(cx, event, &mut | _, _ | {});
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.scroll_bars.redraw(cx)
    }

    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if self.draw_state.begin(cx, walk) {
            return WidgetDraw::hook_above();
        }
        self.draw_state.end();
        WidgetDraw::done()
    }
}


impl LiveHook for FishPatchEditor {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, FishPatchEditor)
    }

    fn after_new_from_doc(&mut self, _cx: &mut Cx) {}
}

impl FishPatchEditor {
    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) {
        // lets draw a bunch of quads
        let mut fullrect = cx.walk_turtle_with_area(&mut self.area, walk);        
    }

    fn walk(&mut self, _cx:&mut Cx) -> Walk {self.walk}
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        self.draw_walk(cx, walk);
        WidgetDraw::done()
    }

    pub fn draw(&mut self, cx: &mut Cx2d, patch: &mut FishPatch) {
        let walk: Walk = self.draw_state.get().unwrap();
        self.scroll_bars.begin(cx, walk, Layout::default());
        let turtle_rect = cx.turtle().rect();
        let scroll_pos = self.scroll_bars.get_scroll_pos();
        self.unscrolled_rect = cx.turtle().unscrolled_rect();

        for i in patch.blocks.iter() 
        {
            let item_id = LiveId::from_num(1, i.id);
            let item = list.item(cx, item_id, live_id!(PatchBlock)).unwrap().as_view();
            item.draw_widget_all(cx);

            println!("{:?} ({:?},{:?})", i.id, i.x,i.y);
        }
        
        
        self.draw_bg.draw_abs(cx, cx.turtle().unscrolled_rect());
        self.scroll_bars.end(cx);
    }
}


#[derive(Clone, PartialEq, WidgetRef)]
pub struct FishPatchEditorRef(WidgetRef);
