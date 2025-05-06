
use crate::{
    widget::*,
    makepad_derive_widget::*,
    makepad_draw::*,
};

live_design!{
    link widgets;
    pub PageFlipBase = {{PageFlip}} {}
    pub PageFlip = <PageFlipBase>{
    }
}

#[derive(Live, LiveRegisterWidget, WidgetRef, WidgetSet)]
pub struct PageFlip {
    #[rust] area: Area,
    #[walk] walk: Walk,
    #[layout] layout: Layout,
    #[live(false)] lazy_init: bool,
    #[live] active_page: LiveId,
    #[rust] draw_state: DrawStateWrap<Walk>,
    #[rust] pointers: ComponentMap<LiveId, LivePtr>,
    #[rust] pages: ComponentMap<LiveId, WidgetRef>,
}

impl LiveHook for PageFlip {
    
    fn before_apply(&mut self, _cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        if let ApplyFrom::UpdateFromDoc {..} = apply.from {
            self.pointers.clear();
        }
    }
    
    fn after_apply(&mut self, cx: &mut Cx, apply: &mut Apply, _index: usize, _nodes: &[LiveNode]) {
        match apply.from {
            ApplyFrom::NewFromDoc {..} | ApplyFrom::UpdateFromDoc {..} => {
                if !self.lazy_init{
                    for (page_id, ptr) in self.pointers.iter(){
                        self.pages.get_or_insert(cx, *page_id, | cx | {
                            WidgetRef::new_from_ptr(cx, Some(*ptr))
                        });
                    }
                }
            }
            _=>()
        }
    }
        
    
    // hook the apply flow to collect our templates and apply to instanced childnodes
    fn apply_value_instance(&mut self, cx: &mut Cx, apply: &mut Apply, index: usize, nodes: &[LiveNode]) -> usize {
        let id = nodes[index].id;
        match apply.from {
            ApplyFrom::NewFromDoc {file_id} | ApplyFrom::UpdateFromDoc {file_id,..} => {
                if nodes[index].origin.has_prop_type(LivePropType::Instance) {
                    let live_ptr = cx.live_registry.borrow().file_id_index_to_live_ptr(file_id, index);
                    self.pointers.insert(id, live_ptr);
                    // find if we have the page and apply
                    if let Some(node) = self.pages.get_mut(&id) {
                        node.apply(cx, apply, index, nodes);
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

impl WidgetNode for PageFlip{
    fn walk(&mut self, _cx:&mut Cx) -> Walk{
        self.walk
    }
    fn area(&self)->Area{self.area}
    
    fn redraw(&mut self, cx: &mut Cx){
        self.area.redraw(cx)
    }
        
    fn find_widgets(&self, path: &[LiveId], cached: WidgetCache, results: &mut WidgetSet) {
        if let Some(page) = self.pages.get(&path[0]) {
            if path.len() == 1{
                results.push(page.clone());
            }
            else{
                page.find_widgets(&path[1..], cached, results);
            }
        }
        for page in self.pages.values() {
            page.find_widgets(path, cached, results);
        }
    }
    
    fn uid_to_widget(&self, uid:WidgetUid)->WidgetRef{
        for page in self.pages.values() {
            let x = page.uid_to_widget(uid);
            if !x.is_empty(){return x}
        }
        WidgetRef::empty()
    }
}        

impl Widget for PageFlip {
    
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        let uid = self.widget_uid();
        if event.requires_visibility(){
            if let Some(page) = self.pages.get_mut(&self.active_page) {
                let item_uid = page.widget_uid();
                cx.group_widget_actions(uid, item_uid, |cx|{
                    page.handle_event(cx, event, scope)
                });
            }
        }
        else{
            for page in self.pages.values(){
                let item_uid = page.widget_uid();
                cx.group_widget_actions(uid, item_uid, |cx|{
                    page.handle_event(cx, event, scope)
                });
            }
        }
    }
    
    fn draw_walk(&mut self, cx: &mut Cx2d, scope:&mut Scope, walk: Walk) -> DrawStep {
        if let Some(page) = self.page(cx, self.active_page) {
            if self.draw_state.begin_with(cx, &(), | cx, _ | {
                page.walk(cx)
            }) {
                self.begin(cx, walk);
            }
            if let Some(walk) = self.draw_state.get() {
                page.draw_walk(cx, scope, walk) ?;
            }
            self.end(cx);
        }
        else {
            self.begin(cx, walk);
            self.end(cx);
        }
        DrawStep::done()
    }
}

impl PageFlipRef {
    pub fn set_active_page(&self, cx: &mut Cx, page: LiveId) {
        if let Some(mut inner) = self.borrow_mut() {
            inner.redraw(cx);
            inner.active_page = page;
        }
    }
}

impl PageFlipSet {
}
