use {
    crate::{
        makepad_platform::*,
        cx_2d::Cx2d,
    }
};

#[derive(Debug)]
pub struct Overlay { // draw info per UI element
    pub (crate) draw_list: DrawList,
}

impl LiveHook for Overlay {}
impl LiveNew for Overlay {
    fn new(cx: &mut Cx) -> Self {
        let draw_list = cx.draw_lists.alloc();
        cx.draw_lists[draw_list.id()].unclipped = true;
        Self {
            draw_list,
        }
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            live_ignore: true,
            fields: Vec::new(),
            type_name: LiveId::from_str("Overlay").unwrap()
        }
    }
}

impl LiveApply for Overlay {
    fn apply(&mut self, _cx: &mut Cx, _from: ApplyFrom, index: usize, nodes: &[LiveNode]) -> usize {
        nodes.skip_node(index)
    }
}

impl Overlay {
    pub fn begin(&self, cx:&mut Cx2d){
        // mark our overlay_id on cx
        cx.overlay_id = Some(self.draw_list.id());
    }
    
    pub fn end(&self, cx:&mut Cx2d){
        cx.overlay_id = None;
        let parent_id = cx.draw_list_stack.last().cloned().unwrap();
        let redraw_id = cx.redraw_id;
        cx.draw_lists[parent_id].append_sub_list(redraw_id, self.draw_list.id());
        
        // flush out all overlays that have a different redraw id than their parent
        // this means it didn't 
        for i in 0..cx.draw_lists[self.draw_list.id()].draw_items.len(){
            if let Some(sub_id) = cx.draw_lists[self.draw_list.id()].draw_items[i].sub_list(){
                let cfp = cx.draw_lists[sub_id].codeflow_parent_id.unwrap();
                if cx.draw_lists[cfp].redraw_id != cx.draw_lists[sub_id].redraw_id{
                    cx.draw_lists[self.draw_list.id()].remove_sub_list(sub_id);
                }
            }
        }
    }
}

