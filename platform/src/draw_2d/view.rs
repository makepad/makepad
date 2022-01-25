pub use {
    std::{
        rc::Rc,
        cell::RefCell
    },
    crate::{
        makepad_live_compiler::*,
        platform::{
            CxPlatformDrawCall,
            CxPlatformView,
        },
        cx::{
            Cx,
        },
        area::{Area, DrawListArea, InstanceArea},
        live_traits::*,
        draw_2d::{
            cx_2d::Cx2d,
            turtle::{Layout, Width, Height, Walk, Rect},
        },
        cx_draw_shaders::{
            CxDrawShaderOptions,
            CxDrawShaderMapping,
            CxDrawShader,
            DrawShader,
        },
        draw_vars::{
            DrawVars,
            DRAW_CALL_USER_UNIFORMS,
            DRAW_CALL_TEXTURE_SLOTS
        },
        
        draw_list::{
            DrawList,
            DrawListDebug,
            DrawItem,
            DrawCall
        },
        geometry::Geometry
    }
};

pub type ViewRedraw = Result<(), ()>;

#[derive(Debug)]
pub struct View { // draw info per UI element
    pub draw_list_id: usize, //Option<usize>,
    pub layout: Layout,
    pub is_overlay: bool,
    pub always_redraw: bool,
    pub redraw_id: u64,
    pub draw_lists_free: Rc<RefCell<Vec<usize >> >,
}

impl Drop for View {
    fn drop(&mut self) {
        self.draw_lists_free.borrow_mut().push(self.draw_list_id)
    }
}

impl LiveHook for View {}
impl LiveNew for View {
    fn new(cx: &mut Cx) -> Self {
        let draw_lists_free = cx.draw_lists_free.clone();
        let draw_list_id = if let Some(draw_list_id) = draw_lists_free.borrow_mut().pop() {
            cx.draw_lists[draw_list_id].alloc_generation += 1;
            draw_list_id
        }
        else {
            let draw_list_id = cx.draw_lists.len();
            cx.draw_lists.push(DrawList::new());
            draw_list_id
        };
        
        Self {
            always_redraw: false,
            is_overlay: false,
            draw_lists_free: draw_lists_free,
            redraw_id: 0,
            layout: Layout::default(),
            draw_list_id,
        }
    }
    
    fn live_type_info(_cx:&mut Cx) -> LiveTypeInfo {
        LiveTypeInfo {
            module_id: LiveModuleId::from_str(&module_path!()).unwrap(),
            live_type: LiveType::of::<Self>(),
            fields: Vec::new(),
            //kind: LiveTypeKind::Object,
            type_name: LiveId::from_str("View").unwrap()
        }
    }
}
impl LiveApply for View {
    //fn type_id(&self) -> std::any::TypeId {std::any::TypeId::of::<Self>()}
    fn apply(&mut self, cx: &mut Cx, apply_from: ApplyFrom, start_index: usize, nodes: &[LiveNode]) -> usize {
        
        if !nodes[start_index].value.is_structy_type() {
            cx.apply_error_wrong_type_for_struct(live_error_origin!(), start_index, nodes, id!(View));
            return nodes.skip_node(start_index);
        }
        cx.draw_lists[self.draw_list_id].debug_id = nodes[start_index].id;
        let mut index = start_index + 1;
        loop {
            if nodes[index].value.is_close() {
                index += 1;
                break;
            }
            match nodes[index].id {
                id!(debug_id) => cx.draw_lists[self.draw_list_id].debug_id = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                id!(is_clipped) => cx.draw_lists[self.draw_list_id].is_clipped = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                id!(is_overlay) => self.is_overlay = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                id!(always_redraw) => self.always_redraw = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                id!(layout) => self.layout = LiveNew::new_apply_mut(cx, apply_from, &mut index, nodes),
                _ => {
                    cx.apply_error_no_matching_field(live_error_origin!(), index, nodes);
                    index = nodes.skip_node(index);
                }
            }
        }
        return index;
    }
}

impl View {
    
    pub fn set_is_clipped(&self, cx: &mut Cx, is_clipped: bool) {cx.draw_lists[self.draw_list_id].is_clipped = is_clipped;}
    //pub fn set_is_overlay(&self, cx: &mut Cx, is_overlay: bool) {cx.draw_lists[self.draw_list_id].is_overlay = is_overlay;}
    //pub fn set_always_redraw(&self, cx: &mut Cx, always_redraw: bool) {cx.draw_lists[self.draw_list_id].always_redraw = always_redraw;}
    /*
    pub fn set_layout(&self, cx: &mut Cx, layout: Layout) {
        cx.draw_lists[self.draw_list_id].layout = layout;
    }
    
    pub fn layout(&self, cx: &mut Cx) -> Layout {
        cx.draw_lists[self.draw_list_id].layout
    }
    */
    
    pub fn lock_view_transform(&self, cx: &mut Cx, mat: &Mat4) {
        let draw_list = &mut cx.draw_lists[self.draw_list_id];
        draw_list.uniform_view_transform(mat);
        return draw_list.locked_view_transform = true;
    }
    
    pub fn set_view_transform(&self, cx: &mut Cx, mat: &Mat4) {
        
        fn set_view_transform_recur(draw_list_id: usize, cx: &mut Cx, mat: &Mat4) {
            if cx.draw_lists[draw_list_id].locked_view_transform {
                return
            }
            cx.draw_lists[draw_list_id].uniform_view_transform(mat);
            let draw_items_len = cx.draw_lists[draw_list_id].draw_items_len;
            for draw_item_id in 0..draw_items_len {
                if let Some(sub_view_id) = cx.draw_lists[draw_list_id].draw_items[draw_item_id].sub_view_id {
                    set_view_transform_recur(sub_view_id, cx, mat);
                }
            }
        }
        
        set_view_transform_recur(self.draw_list_id, cx, mat);
    }
    
    pub fn begin(&mut self, cx: &mut Cx2d) -> ViewRedraw {
        
        // check if we have a pass id parent
        let pass_id = cx.pass_id.expect("No pass found when begin_view");
        cx.cx.draw_lists[self.draw_list_id].pass_id = pass_id;
        
        let codeflow_parent_id = cx.draw_list_stack.last().cloned();
        
        let view_will_redraw = cx.view_will_redraw(self);
        
        let cxpass = &mut cx.cx.passes[pass_id];
        
        if cxpass.main_draw_list_id.is_none() {
            cxpass.main_draw_list_id = Some(self.draw_list_id);
            self.layout.abs_origin = Some(Vec2 {x: 0., y: 0.});
            self.layout.abs_size = Some(cxpass.pass_size);
        }
        
        // find the parent draw list id
        let parent_id = if self.is_overlay {
            if cxpass.main_draw_list_id.is_none() {
                panic!("Cannot make overlay inside window without root view")
            };
            let main_view_id = cxpass.main_draw_list_id.unwrap();
            Some(main_view_id)
        }
        else {
            cx.draw_list_stack.last().cloned()
        };
        
        // push ourselves up the parent draw_stack
        if let Some(parent_id) = parent_id {
            // copy the view transform
            
            if !cx.cx.draw_lists[self.draw_list_id].locked_view_transform {
                for i in 0..16 {
                    cx.cx.draw_lists[self.draw_list_id].draw_list_uniforms.view_transform[i] =
                    cx.cx.draw_lists[parent_id].draw_list_uniforms.view_transform[i];
                }
            }
            
            let parent = &mut cx.cx.draw_lists[parent_id];
            
            // see if we need to add a new one
            if parent.draw_items_len >= parent.draw_items.len() {
                parent.draw_items.push({
                    DrawItem {
                        draw_list_id: parent_id,
                        draw_item_id: parent.draw_items.len(),
                        redraw_id: cx.cx.redraw_id,
                        sub_view_id: Some(self.draw_list_id),
                        draw_call: None
                    }
                });
                parent.draw_items_len += 1;
            }
            else { // or reuse a sub list node
                let draw_item = &mut parent.draw_items[parent.draw_items_len];
                draw_item.sub_view_id = Some(self.draw_list_id);
                draw_item.redraw_id = cx.cx.redraw_id;
                parent.draw_items_len += 1;
            }
        }
        
        // set nesting draw list id for incremental repaint scanning
        cx.cx.draw_lists[self.draw_list_id].codeflow_parent_id = codeflow_parent_id;
        
        // check redraw status
        if !self.always_redraw
            && cx.cx.draw_lists[self.draw_list_id].draw_items_len != 0
            && !view_will_redraw {
            
            // walk the turtle because we aren't drawing
            let w = Width::Fixed(cx.draw_lists[self.draw_list_id].rect.size.x);
            let h = Height::Fixed(cx.draw_lists[self.draw_list_id].rect.size.y);
            cx.walk_turtle(Walk {width: w, height: h, margin: self.layout.walk.margin});
            return Err(());
        }
        
        if cxpass.main_draw_list_id.unwrap() == self.draw_list_id {
            cx.passes[pass_id].paint_dirty = true;
        }
        
        let cxview = &mut cx.cx.draw_lists[self.draw_list_id];
        
        // update redarw id
        let last_redraw_id = cxview.redraw_id;
        self.redraw_id = cx.cx.redraw_id;
        cxview.redraw_id = cx.cx.redraw_id;
        
        cxview.draw_items_len = 0;
        
        cx.draw_list_stack.push(self.draw_list_id);
        
        let old_area = Area::DrawList(DrawListArea {draw_list_id: self.draw_list_id, redraw_id: last_redraw_id});
        let new_area = Area::DrawList(DrawListArea {draw_list_id: self.draw_list_id, redraw_id: cx.redraw_id});
        
        cx.update_area_refs(old_area, new_area);
        cx.begin_turtle_with_guard(self.layout, new_area);
        
        Ok(())
    }
    
    
    pub fn end(&mut self, cx: &mut Cx2d) -> Area {
        // let view_id = self.view_id.unwrap();
        let view_area = Area::DrawList(DrawListArea {draw_list_id: self.draw_list_id, redraw_id: cx.redraw_id});
        let rect = cx.end_turtle_with_guard(view_area);
        let cxview = &mut cx.draw_lists[self.draw_list_id];
        cxview.rect = rect;
        cx.draw_list_stack.pop();
        view_area
    }
    
    pub fn get_rect(&self, cx: &Cx) -> Rect {
        let cxview = &cx.draw_lists[self.draw_list_id];
        return cxview.rect
    }
    
    pub fn get_view_transform(&self, cx: &Cx) -> Mat4 {
        let cxview = &cx.draw_lists[self.draw_list_id];
        return cxview.get_view_transform()
    }
    
    pub fn set_view_debug(&self, cx: &mut Cx, debug: DrawListDebug) {
        let cxview = &mut cx.draw_lists[self.draw_list_id];
        cxview.debug = Some(debug);
    }
    
    pub fn redraw(&self, cx: &mut Cx) {
        cx.redraw_area(self.area());
    }
    
    pub fn redraw_self_and_children(&self, cx: &mut Cx) {
        cx.redraw_area_and_children(self.area());
    }
    
    pub fn area(&self) -> Area {
        Area::DrawList(DrawListArea {draw_list_id: self.draw_list_id, redraw_id: self.redraw_id})
    }
}


impl<'a> Cx2d<'a> {
    
    pub fn new_draw_call(&mut self, draw_vars: &DrawVars) -> Option<&mut DrawItem> {
        return self.get_draw_call(false, draw_vars);
    }
    
    pub fn append_to_draw_call(&mut self, draw_vars: &DrawVars) -> Option<&mut DrawItem> {
        return self.get_draw_call(true, draw_vars);
    }
    
    pub fn get_draw_call(&mut self, append: bool, draw_vars: &DrawVars) -> Option<&mut DrawItem> {
        
        if draw_vars.draw_shader.is_none(){
            return None
        }
        let draw_shader = draw_vars.draw_shader.unwrap();
        
        if draw_shader.draw_shader_generation != self.draw_shaders.generation{
            return None
        }
        
        let sh = &self.cx.draw_shaders[draw_shader.draw_shader_id];
        
        let current_draw_list_id = *self.draw_list_stack.last().unwrap();
        let draw_list = &mut self.cx.draw_lists[current_draw_list_id];
        let draw_item_id = draw_list.draw_items_len;
        
        if append && !sh.mapping.flags.draw_call_always {
            if let Some(index) = draw_list.find_appendable_drawcall(sh, draw_vars) {
                return Some(&mut draw_list.draw_items[index]);
            }
        }
        
        // add one
        draw_list.draw_items_len += 1;
        
        // see if we need to add a new one
        if draw_item_id >= draw_list.draw_items.len() {
            draw_list.draw_items.push(DrawItem {
                draw_item_id: draw_item_id,
                draw_list_id: current_draw_list_id,
                redraw_id: self.cx.redraw_id,
                sub_view_id: None,
                draw_call: Some(DrawCall::new(&sh.mapping, draw_vars))
            });
            return Some(&mut draw_list.draw_items[draw_item_id]);
        }
        // reuse an older one, keeping all GPU resources attached
        let mut draw_item = &mut draw_list.draw_items[draw_item_id];
        draw_item.sub_view_id = None;
        draw_item.redraw_id = self.cx.redraw_id;
        if let Some(dc) = &mut draw_item.draw_call {
            dc.reuse_in_place(&sh.mapping, draw_vars);
        }
        else {
            draw_item.draw_call = Some(DrawCall::new(&sh.mapping, draw_vars))
        }
        return Some(draw_item);
    }
    
    pub fn begin_many_instances(&mut self, draw_vars: &DrawVars) -> Option<ManyInstances> {
        
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none(){
            return None
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        let mut instances = None;
        
        std::mem::swap(&mut instances, &mut draw_call.instances);
        Some(ManyInstances {
            instance_area: InstanceArea {
                draw_list_id: draw_item.draw_list_id,
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
        if li.is_none(){
            return None;
        }
        li.as_mut().unwrap().aligned = Some(self.align_list.len());
        self.align_list.push(Area::Empty);
        li
    }
    
    pub fn end_many_instances(&mut self, many_instances: ManyInstances) -> Area {
        let mut ia = many_instances.instance_area;
        let draw_list = &mut self.draw_lists[ia.draw_list_id];
        let draw_item = &mut draw_list.draw_items[ia.draw_item_id];
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        
        let mut instances = Some(many_instances.instances);
        std::mem::swap(&mut instances, &mut draw_call.instances);
        ia.instance_count = (draw_call.instances.as_ref().unwrap().len() - ia.instance_offset) / draw_call.total_instance_slots;
        if let Some(aligned) = many_instances.aligned {
            self.align_list[aligned] = ia.clone().into();
        }
        ia.into()
    }
    
    pub fn add_instance(&mut self, draw_vars: &DrawVars) -> Area {
        let data = draw_vars.as_slice();
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none(){
            return Area::Empty
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        let instance_count = data.len() / draw_call.total_instance_slots;
        let check = data.len() % draw_call.total_instance_slots;
        if check > 0 {
            panic!("Data not multiple of total slots");
        }
        let ia = InstanceArea {
            draw_list_id: draw_item.draw_list_id,
            draw_item_id: draw_item.draw_item_id,
            instance_count: instance_count,
            instance_offset: draw_call.instances.as_ref().unwrap().len(),
            redraw_id: draw_item.redraw_id
        };
        draw_call.instances.as_mut().unwrap().extend_from_slice(data);
        ia.into()
    }
    
    pub fn add_aligned_instance(&mut self, draw_vars: &DrawVars) -> Area {
        let data = draw_vars.as_slice();
        let draw_item = self.append_to_draw_call(draw_vars);
        if draw_item.is_none(){
            return Area::Empty
        }
        let draw_item = draw_item.unwrap();
        let draw_call = draw_item.draw_call.as_mut().unwrap();
        let instance_count = data.len() / draw_call.total_instance_slots;
        let check = data.len() % draw_call.total_instance_slots;
        if check > 0 {
            println!("Data not multiple of total slots");
            return Area::Empty
        }
        let ia: Area = (InstanceArea {
            draw_list_id: draw_item.draw_list_id,
            draw_item_id: draw_item.draw_item_id,
            instance_count: instance_count,
            instance_offset: draw_call.instances.as_ref().unwrap().len(),
            redraw_id: draw_item.redraw_id
        }).into();
        draw_call.instances.as_mut().unwrap().extend_from_slice(data);
        self.align_list.push(ia.clone());
        ia
    }
    
    pub fn set_view_scroll_x(&mut self, draw_list_id: usize, scroll_pos: f32) {
        let pass_id = self.draw_lists[draw_list_id].pass_id;
        let fac = self.get_delegated_dpi_factor(pass_id);
        let draw_list = &mut self.cx.draw_lists[draw_list_id];
        draw_list.unsnapped_scroll.x = scroll_pos;
        let snapped = scroll_pos - scroll_pos % (1.0 / fac);
        if draw_list.snapped_scroll.x != snapped {
            draw_list.snapped_scroll.x = snapped;
            self.cx.passes[draw_list.pass_id].paint_dirty = true;
        }
    }
    
    
    pub fn set_view_scroll_y(&mut self, draw_list_id: usize, scroll_pos: f32) {
        let pass_id = self.draw_lists[draw_list_id].pass_id;
        let fac = self.get_delegated_dpi_factor(pass_id);
        let draw_list = &mut self.cx.draw_lists[draw_list_id];
        draw_list.unsnapped_scroll.y = scroll_pos;
        let snapped = scroll_pos - scroll_pos % (1.0 / fac);
        if draw_list.snapped_scroll.y != snapped {
            draw_list.snapped_scroll.y = snapped;
            self.cx.passes[draw_list.pass_id].paint_dirty = true;
        }
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