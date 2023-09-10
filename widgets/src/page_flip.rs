
use crate::{
    widget::*,
    makepad_derive_widget::*,
    makepad_draw::*,
};

live_design!{
    PageFlipBase = {{PageFlip}} {}
}

#[derive(Live)]
pub struct PageFlip {
    #[rust] area: Area,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[live(true)] on_demand: bool,
    #[live] active_page: LiveId,
    #[rust] draw_state: DrawStateWrap<Walk>,
    #[rust] pointers: ComponentMap<LiveId, LivePtr>,
    #[rust] pages: ComponentMap<LiveId, WidgetRef>,
}

impl LiveHook for PageFlip {
    fn before_live_design(cx: &mut Cx) {
        register_widget!(cx, PageFlip)
    }
    
    fn before_apply(&mut self, _cx: &mut Cx, from: ApplyFrom, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc {..} = from {
            self.pointers.clear();
        }
    }
    
    // hook the apply flow to collect our templates and apply to instanced childnodes
    fn apply_value_instance(&mut self, cx: &mut Cx, from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        let id = nodes[index].id;
        match from {
            ApplyFrom::NewFromDoc {file_id} | ApplyFrom::UpdateFromDoc {file_id} => {
                if nodes[index].origin.has_prop_type(LivePropType::Instance) {
                    let live_ptr = cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, index);
                    self.pointers.insert(id, live_ptr);
                    // find if we have the page and apply
                    if let Some(node) = self.pages.get_mut(&id){
                        node.apply(cx, from, index, nodes);
                    }
                }
                else {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                }
            }
            _ => ()
        }
        nodes.skip_node(index)
    }
}

impl PageFlip {
    
    pub fn page(&mut self, cx: &mut Cx, page_id: LiveId) -> Option<WidgetRef> {
        if let Some(ptr) = self.pointers.get(&page_id) {
            let entry = self.pages.get_or_insert(cx, page_id, | cx | {
                WidgetRef::new_from_ptr(cx, Some(*ptr))
            });
            return Some(entry.clone())
        }
        None
    }

    fn begin(&mut self, cx: &mut Cx2d, walk: Walk) {
        cx.begin_turtle(walk, self.layout);
    }
    
    fn end(&mut self, cx: &mut Cx2d) {
        cx.end_turtle_with_area(&mut self.area);
    }
}


impl Widget for PageFlip {
    
    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
    
    fn handle_widget_event_with(&mut self, cx: &mut Cx, event: &Event, dispatch_action: &mut dyn FnMut(&mut Cx, WidgetActionItem)) {
        let uid = self.widget_uid();
        
        if let Some(page) = self.pages.get_mut(&self.active_page){
            let item_uid = page.widget_uid();
            page.handle_widget_event_with(cx, event, &mut | cx, action | {
                dispatch_action(cx, action.with_container(uid).with_item(item_uid))
            });
        }
    }
    
    fn walk(&mut self, _cx:&mut Cx) -> Walk {
        self.walk
    }
    
    fn draw_walk_widget(&mut self, cx: &mut Cx2d, walk: Walk) -> WidgetDraw {
        if let Some(page) = self.page(cx, self.active_page){
            if self.draw_state.begin_with(cx, &(), |cx,_|{
                page.walk(cx)
            }){
                self.begin(cx, walk);
            }
            if let Some(walk) = self.draw_state.get() {
                page.draw_walk_widget(cx, walk)?;
            }
            self.end(cx);
        }
        else{
            self.begin(cx, walk);
            self.end(cx);
        }
        WidgetDraw::done()
    }
}

#[derive(Clone, Default, PartialEq, WidgetRef)]
pub struct PageFlipRef(WidgetRef);

impl PageFlipRef {
    pub fn set_active_page(&self, page: LiveId){
        if let Some(mut inner) = self.borrow_mut(){
            inner.active_page = page;
        }
    }
    pub fn set_active_page_and_redraw(&self, cx: &mut Cx, page: LiveId){
        if let Some(mut inner) = self.borrow_mut(){
            inner.redraw(cx);
            inner.active_page = page;
        }
    }
}

#[derive(Clone, Default, WidgetSet)]
pub struct PageFlipSet(WidgetSet);

impl PageFlipSet {
}
