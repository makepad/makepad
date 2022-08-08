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
        
        // simply append us to the drawlist
        // but dont flush our internal view
        // we should 'gc' our drawlist here
    }
}

