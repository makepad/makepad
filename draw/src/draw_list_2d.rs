use {
    crate::{
        makepad_platform::*,
        nav::*,
        cx_2d::{Cx2d},
        turtle::{Walk,AlignEntry}
    }
};

 
#[derive(Debug)]
pub struct DrawList2d { // draw info per UI element
    pub (crate) draw_list: DrawList,
    pub (crate) dirty_check_rect: Rect,
}

impl LiveHook for DrawList2d {}
impl LiveNew for DrawList2d {
    fn new(cx: &mut Cx) -> Self {
        let draw_list = cx.draw_lists.alloc();
        Self {
            dirty_check_rect: Default::default(),
            draw_list,
        }
    }
    
    fn live_type_info(_cx: &mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            live_ignore: true,
            fields: Vec::new(),
            type_name: id_lut!(View)
        }
    }
}
impl LiveApply for DrawList2d {
    //fn type_id(&self) -> std::any::TypeId {std::any::TypeId::of::<Self>()}
    fn apply(&mut self, cx: &mut Cx, apply: &mut Apply, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, live_id!(View));
            return nodes.skip_node(start_index);
        }
        cx.draw_lists[self.draw_list.id()].debug_id = nodes[start_index].id;
        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id {
                live_id!(debug_id) => cx.draw_lists[self.draw_list.id()].debug_id = LiveNew::new_apply_mut_index(cx, apply, &mut index, nodes),
                _ => {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    index = nodes.skip_node(index);
                }
            }
        }
        return index;
    }
}

impl DrawList2d {
    
    pub fn draw_list_id(&self) -> DrawListId {self.draw_list.id()}
    
    
    pub fn set_view_transform(&self, cx: &mut Cx, mat: &Mat4) {
        
        fn set_view_transform_recur(draw_list_id: DrawListId, cx: &mut Cx, mat: &Mat4) {
            /*if cx.draw_lists[draw_list_id].locked_view_transform {
                return
            }*/
            cx.draw_lists[draw_list_id].uniform_view_transform(mat);
            let draw_items_len = cx.draw_lists[draw_list_id].draw_items.len();
            for draw_item_id in 0..draw_items_len {
                if let Some(sub_list_id) = cx.draw_lists[draw_list_id].draw_items[draw_item_id].sub_list() {
                    set_view_transform_recur(sub_list_id, cx, mat);
                }
            }
        }
        
        set_view_transform_recur(self.draw_list.id(), cx, mat);
    }

    pub fn begin_overlay_last(&mut self, cx: &mut Cx2d) {
        self.begin_overlay_inner(cx, true)
    }
    
    pub fn begin_overlay_reuse(&mut self, cx: &mut Cx2d) {
        self.begin_overlay_inner(cx, false)
    }
    
    pub fn begin_overlay_inner(&mut self, cx: &mut Cx2d, always_last:bool) {
        let pass_id = cx.pass_stack.last().unwrap().pass_id;
        let redraw_id = cx.cx.redraw_id;
        
        cx.draw_lists[self.draw_list.id()].pass_id = Some(pass_id);
        
        let codeflow_parent_id = cx.draw_list_stack.last().cloned().unwrap();
        
        let overlay_id = cx.overlay_id.unwrap();
        if always_last{
            cx.draw_lists[overlay_id].store_sub_list_last(redraw_id, self.draw_list.id());
        }
        else{
            cx.draw_lists[overlay_id].store_sub_list(redraw_id, self.draw_list.id());
        }
        
        cx.nav_list_item_push(codeflow_parent_id, NavItem::Child(self.draw_list.id()));
        
        cx.cx.draw_lists[self.draw_list.id()].codeflow_parent_id = Some(codeflow_parent_id);
        if cx.passes[pass_id].main_draw_list_id.unwrap() == self.draw_list.id() {
            cx.passes[pass_id].paint_dirty = true;
        }
        
        cx.cx.draw_lists[self.draw_list.id()].clear_draw_items(redraw_id);
        
        cx.nav_list_clear(self.draw_list.id());
        
        cx.draw_list_stack.push(self.draw_list.id());
    }
    
    
    pub fn begin_always(&mut self, cx: &mut Cx2d) {
        self.begin_maybe(cx, None).expect_redraw();
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d, walk: Walk) -> Redrawing {
        self.begin_maybe(cx, Some(walk))
    }
    
    fn begin_maybe(&mut self, cx: &mut Cx2d, cache_check: Option<Walk>) -> Redrawing {
        
        // check if we have a pass id parent
        let pass_id = cx.pass_stack.last().unwrap().pass_id;
        let redraw_id = cx.cx.redraw_id;
        
        cx.draw_lists[self.draw_list.id()].pass_id = Some(pass_id);
        
        let codeflow_parent_id = cx.draw_list_stack.last().cloned();
        
        let will_redraw = cache_check.is_none() || cx.will_redraw(self, cache_check.unwrap());
        
        let is_main_draw_list = if cx.passes[pass_id].main_draw_list_id.is_none() {
            cx.passes[pass_id].main_draw_list_id = Some(self.draw_list.id());
            true
        }
        else{
            false
        };
        
        // find the parent draw list id
        if let Some(parent_id) = codeflow_parent_id {
            if !is_main_draw_list{
                let parent = &mut cx.cx.draw_lists[parent_id];
                parent.append_sub_list(redraw_id, self.draw_list.id());
                
                cx.nav_list_item_push(parent_id, NavItem::Child(self.draw_list.id()));
            }
        }
        
        // set nesting draw list id for incremental repaint scanning
        cx.cx.draw_lists[self.draw_list.id()].codeflow_parent_id = codeflow_parent_id;
        
        // check redraw status
        if cx.cx.draw_lists[self.draw_list.id()].draw_items.len() != 0 && !will_redraw {
            /*
            let w = Size::Fixed(cx.cx.draw_lists[self.draw_list.id()].rect.size.x);
            let h = Size::Fixed(cx.cx.draw_lists[self.draw_list.id()].rect.size.y);
            let walk = Walk {abs_pos: None, width: w, height: h, margin: walk.margin};
            //let pos = cx.peek_walk_pos(walk);
            //if pos == cx.cx.draw_lists[self.draw_list.id()].rect.pos {
             cx.walk_turtle(walk);*/
            return Redrawing::no();
            //}
        }
        
        if cx.passes[pass_id].main_draw_list_id.unwrap() == self.draw_list.id() {
            cx.passes[pass_id].paint_dirty = true;
        }
        
        cx.cx.draw_lists[self.draw_list.id()].clear_draw_items(redraw_id);
        
        cx.nav_list_clear(self.draw_list.id());
        
        cx.draw_list_stack.push(self.draw_list.id());
        
        Redrawing::yes()
    }
    
    pub fn end(&mut self, cx: &mut Cx2d) {
        let draw_list_id = cx.draw_list_stack.pop().unwrap();
        if draw_list_id != self.draw_list.id() {
            panic!("Mismatch in drawlist id in view.end, check your begin/end pairs");
        }
        if cx.cx.draw_lists[draw_list_id].redraw_id != cx.cx.redraw_id {
            panic!("calling end on a view that didnt get begin called this redraw cycle");
        }
    }
    
    pub fn get_view_transform(&self, cx: &Cx) -> Mat4 {
        let cxview = &cx.draw_lists[self.draw_list.id()];
        return cxview.get_view_transform()
    }
    
    pub fn redraw(&self, cx: &mut Cx) {
        cx.redraw_list(self.draw_list.id());
    }
    
    pub fn redraw_self_and_children(&self, cx: &mut Cx) {
        cx.redraw_list_and_children(self.draw_list.id());
    }
}


impl<'a> Cx2d<'a> {
    
    pub fn new_draw_call(&mut self, draw_vars: &DrawVars) -> Option<&mut CxDrawItem> {
        return self.get_draw_call(false, draw_vars);
    }
    
    pub fn append_to_draw_call(&mut self, draw_vars: &DrawVars) -> Option<&mut CxDrawItem> {
        return self.get_draw_call(true, draw_vars);
    }
    
    pub fn get_current_draw_list_id(&self) -> Option<DrawListId> {
        self.draw_list_stack.last().cloned()
    }
    
    pub fn get_draw_call(&mut self, append: bool, draw_vars: &DrawVars) -> Option<&mut CxDrawItem> {
        
        if draw_vars.draw_shader.is_none() {
            return None
        }
        let draw_shader = draw_vars.draw_shader.unwrap();
        
        if draw_shader.draw_shader_generation != self.draw_shaders.generation {
            return None
        }
        
        let sh = &self.cx.draw_shaders[draw_shader.draw_shader_id];
        
        let current_draw_list_id = *self.draw_list_stack.last().unwrap();
        let draw_list = &mut self.cx.draw_lists[current_draw_list_id];
        
        if append && !sh.mapping.flags.draw_call_always {
            if let Some(index) = draw_list.find_appendable_drawcall(sh, draw_vars) {
                return Some(&mut draw_list.draw_items[index]);
            }
        }
        
        Some(draw_list.append_draw_call(self.cx.redraw_id, sh, draw_vars))
    }
    
    pub fn begin_many_instances(&mut self, draw_vars: &DrawVars) -> Option<ManyInstances> {
        
        let draw_list_id = self.get_current_draw_list_id().unwrap();
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none() {
            return None
        }
        let draw_item = draw_item.unwrap();
        //let draw_call = draw_item.kind.draw_call().unwrap();
        let mut instances = None;
        
        std::mem::swap(&mut instances, &mut draw_item.instances);
        Some(ManyInstances {
            instance_area: InstanceArea {
                draw_list_id,
                draw_item_id: draw_item.draw_item_id,
                instance_count: 0,
                instance_offset: instances.as_ref().unwrap().len(),
                redraw_id: draw_item.redraw_id
            },
            aligned: None,
            instances: instances.unwrap()
        })
    }
    
    pub fn begin_many_aligned_instances(&mut self, draw_vars: &DrawVars) -> Option<ManyInstances> {
        let mut li = self.begin_many_instances(draw_vars);
        if li.is_none() {
            return None;
        }
        li.as_mut().unwrap().aligned = Some(self.align_list.len());
        self.align_list.push(AlignEntry::Unset);
        li
    }
    
    pub fn end_many_instances(&mut self, many_instances: ManyInstances) -> Area {
        let mut ia = many_instances.instance_area;
        let draw_list = &mut self.draw_lists[ia.draw_list_id];
        let draw_item = &mut draw_list.draw_items[ia.draw_item_id];
        let draw_call = draw_item.kind.draw_call().unwrap();
        
        let mut instances = Some(many_instances.instances);
        std::mem::swap(&mut instances, &mut draw_item.instances);
        ia.instance_count = (draw_item.instances.as_ref().unwrap().len() - ia.instance_offset) / draw_call.total_instance_slots;
        if let Some(aligned) = many_instances.aligned {
            self.align_list[aligned] = AlignEntry::Area(ia.clone().into());
        }
        ia.into()
    }
    
    pub fn add_instance(&mut self, draw_vars: &DrawVars) -> Area {
        let data = draw_vars.as_slice();
        let draw_list_id = self.get_current_draw_list_id().unwrap();
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none() {
            return Area::Empty
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call().unwrap();
        let instance_count = data.len() / draw_call.total_instance_slots;
        let check = data.len() % draw_call.total_instance_slots;
        if check > 0 {
            panic!("Data not multiple of total slots");
        }
        let ia = InstanceArea {
            draw_list_id,
            draw_item_id: draw_item.draw_item_id,
            instance_count: instance_count,
            instance_offset: draw_item.instances.as_ref().unwrap().len(),
            redraw_id: draw_item.redraw_id
        };
        draw_item.instances.as_mut().unwrap().extend_from_slice(data);
        ia.into()
    }
    
    pub fn add_aligned_instance(&mut self, draw_vars: &DrawVars) -> Area {
        let data = draw_vars.as_slice();
        let draw_list_id = self.get_current_draw_list_id().unwrap();
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none() {
            return Area::Empty
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call().unwrap();
        let instance_count = data.len() / draw_call.total_instance_slots;
        let check = data.len() % draw_call.total_instance_slots;
        if check > 0 {
            error!("Data not multiple of total slots");
            return Area::Empty
        }
        let ia: Area = (InstanceArea {
            draw_list_id,
            draw_item_id: draw_item.draw_item_id,
            instance_count: instance_count,
            instance_offset: draw_item.instances.as_ref().unwrap().len(),
            redraw_id: draw_item.redraw_id
        }).into();
        draw_item.instances.as_mut().unwrap().extend_from_slice(data);
        self.align_list.push(AlignEntry::Area(ia.clone()));
        ia
    }
    
    pub fn add_aligned_rect_area(&mut self, area: &mut Area, rect: Rect) {
        let draw_list_id = *self.draw_list_stack.last().unwrap();
        let draw_list = &mut self.cx.draw_lists[draw_list_id];
        // ok so we have to add
        let rect_id = draw_list.rect_areas.len();
        draw_list.rect_areas.push(CxRectArea {
            rect,
            draw_clip:Default::default(),
        });
        
        let new_area = Area::Rect(RectArea {
            draw_list_id,
            redraw_id: self.redraw_id,
            rect_id
        });
        self.align_list.push(AlignEntry::Area(new_area));
        self.update_area_refs(*area, new_area);
        *area = new_area;
    }
}

#[derive(Debug)]
pub struct ManyInstances {
    pub instance_area: InstanceArea,
    pub aligned: Option<usize>,
    pub instances: Vec<f32>
}

#[derive(Clone)]
pub struct AlignedInstance {
    pub inst: InstanceArea,
    pub index: usize
}

pub type Redrawing = Result<(), ()>;

pub trait RedrawingApi {
    fn no() -> Redrawing {Result::Err(())}
    fn yes() -> Redrawing {Result::Ok(())}
    fn is_redrawing(&self) -> bool;
    fn is_not_redrawing(&self) -> bool;
    fn expect_redraw(&self);
}

impl RedrawingApi for Redrawing {
    fn is_redrawing(&self) -> bool {
        match *self {
            Result::Ok(_) => true,
            Result::Err(_) => false
        }
    }
    fn is_not_redrawing(&self) -> bool {
        match *self {
            Result::Ok(_) => false,
            Result::Err(_) => true
        }
    }
    fn expect_redraw(&self) {
        if !self.is_redrawing() {
            panic!("assume_redraw_yes it should redraw")
        }
    }
}

/*
pub enum ViewRedrawing {
    Yes,
    No
}

impl ViewRedrawing {
    pub fn assume_redrawing(&self){
        if !self.is_redrawing(){
            panic!("assume_redraw_yes it should redraw")
        }
    }
    
    pub fn not_redrawing(&self)->bool{
        !self.is_redrawing()
    }
    
    pub fn is_redrawing(&self) -> bool {
        match self {
            Self::Yes => true,
            _ => false
        }
    }
}

impl FromResidual for ViewRedrawing {
    fn from_residual(_: ()) -> Self {
        Self::No
    }
}

impl Try for ViewRedrawing {
    type Output = ();
    type Residual = ();
    
    fn from_output(_: Self::Output) -> Self {
        Self::Yes
    }
    
    fn branch(self) -> ControlFlow<Self::Residual,
    Self::Output> {
        match self {
            Self::Yes => ControlFlow::Continue(()),
            Self::No => ControlFlow::Break(())
        }
    }
}
*/